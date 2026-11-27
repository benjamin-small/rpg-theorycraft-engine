//! `SimDef`/`Rotation` → `SimPlan`: validate every cross-reference
//! fail-closed, then parse and slot-resolve every expression over the
//! extended sim symbol space (see `sim` module docs). This is the only
//! place sim expressions are parsed — do it once per `SimDef`/`Rotation`
//! pair and reuse the result.

use crate::build::Contribution;
use crate::expr::{compile as compile_expr, Program, Symbols};
use crate::plan::{Plan, PlanError};
use crate::simdef::{
    EffectDef, EvUptime, EventOrder, Measure, NumOrExpr, ProcRolls, ReapplyPolicy, Rotation,
    SimDef, Trigger,
};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// A compiled sim expression plus whether it reads any `objective.<name>`
/// slot (slice 6). The executor evaluates an expression whose
/// `reads_objectives` is `false` exactly as before — no extra Plan
/// evaluation, no RNG change — so the flag is what keeps every existing
/// config byte-identical.
///
/// Used for every field where an objective read is LEGAL: action
/// `cast_time`, proc `chance`, rotation rule `when`, and
/// [`CompiledValue::Expr`] (cooldown/cost/gain/segment `duration`/buff
/// `duration`). `damage.stats` values and resource `max`/`regen_per_sec`
/// reject the read at compile time instead.
#[derive(Debug, PartialEq)]
pub struct SimProgram {
    /// The compiled expression.
    pub program: Program,
    /// Whether `program` loads any `objective.<name>` slot.
    pub reads_objectives: bool,
}

/// Whether a field may read `objective.<name>`, and if not, WHY — the
/// reason the positioned compile error names (each illegal field gets
/// only its own reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectiveReads {
    /// A legal field (spec §5.6's list).
    Legal,
    /// `damage.stats` (action or segment level): the overlay is the world
    /// the objective is evaluated in, so the read would be circular.
    Circular,
    /// `damage.stats.hits_per_use`: the executor reads it directly, before
    /// any `Plan` evaluation (it never reaches the Plan), so no objective
    /// exists yet to read.
    HitsPerUse,
    /// Resource `max`/`regen_per_sec`: folded at every state-change
    /// refold, outside any one field's evaluation instant.
    RefoldTime,
}

impl SimProgram {
    /// Evaluate against the combined slot array; see [`Program::eval`].
    pub fn eval(&self, slots: &[f64]) -> f64 {
        self.program.eval(slots)
    }
}

/// One compiled [`NumOrExpr`]: a literal is pre-baked into a constant (no
/// per-evaluation cost at all — the 0.2.0 fast path is unchanged), an
/// expression into a [`Program`] over the sim symbol space.
///
/// Compiling says nothing about WHEN this gets evaluated — that is fixed
/// per field and documented on [`NumOrExpr`]; the executor calls
/// [`CompiledValue::eval`] at exactly that instant and validates the
/// result fail-closed there.
///
/// `#[non_exhaustive]`: the COMPILED representation is the engine's to
/// extend (a future variant could pre-fold a common shape), and no
/// consumer needs to match on it — `eval` is the whole interface.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum CompiledValue {
    /// A literal from the config, pre-baked.
    Const(f64),
    /// A compiled expression over the sim symbol space.
    Expr(SimProgram),
}

impl CompiledValue {
    /// Whether this value is an expression that reads an
    /// `objective.<name>` slot (a literal never does).
    pub(crate) fn reads_objectives(&self) -> bool {
        matches!(self, CompiledValue::Expr(p) if p.reads_objectives)
    }

    /// This value at the instant `slots` describes.
    ///
    /// `slots` is the executor's combined `[plan slots | sim slots]` array
    /// (see [`SimPlan::slot_width`] and the `sim` module's slot-layout
    /// docs); the executor owns it and keeps it current — there is no
    /// supported way for a consumer to build one, so in practice this is
    /// called by `sim::run` and read by no one else.
    ///
    /// Never validates: the caller does that at the field's documented
    /// evaluation instant, so the error can name the field and the instant
    /// (see `sim::exec`'s `Sim::eval_quantity`/`Sim::eval_stat`).
    pub fn eval(&self, slots: &[f64]) -> f64 {
        match self {
            CompiledValue::Const(v) => *v,
            CompiledValue::Expr(p) => p.eval(slots),
        }
    }
}

/// Compile one [`NumOrExpr`]; `what` labels the field in the positioned
/// error an unparseable/unresolvable expression produces (invoked only on
/// the error path).
fn compile_value(
    v: &NumOrExpr,
    syms: &SimSymbols<'_>,
    what: impl FnOnce() -> String,
) -> Result<CompiledValue, PlanError> {
    match v {
        NumOrExpr::Num(n) => Ok(CompiledValue::Const(*n)),
        NumOrExpr::Expr(src) => Ok(CompiledValue::Expr(compile_sim(
            src,
            syms,
            what,
            ObjectiveReads::Legal,
        )?)),
    }
}

/// Compile one sim expression, tracking `objective.<name>` reads.
///
/// `objectives` is the field's legality: anything but
/// [`ObjectiveReads::Legal`] makes an objective read a positioned compile
/// error naming the field (`what`) and that field's one reason. An
/// unknown `objective.<name>` is a did-you-mean error against the Plan's
/// objective names, wherever it appears.
fn compile_sim(
    src: &str,
    syms: &SimSymbols<'_>,
    what: impl FnOnce() -> String,
    objectives: ObjectiveReads,
) -> Result<SimProgram, PlanError> {
    let probe = ObjectiveProbe {
        inner: syms,
        read: Cell::new(false),
        unknown: RefCell::new(None),
        forbidden: RefCell::new(None),
        watched: RefCell::new(None),
    };
    match compile_expr(src, &probe) {
        Ok(program) => {
            if let Some((read, buff)) = probe.forbidden.borrow().as_ref() {
                return Err(PlanError {
                    what: format!(
                        "{} reads `{read}`, which buff `{buff}` (`ev_uptime: expected`) \
                         carries as a probability, not a state: under EV the buff is up \
                         with probability P, so a boolean read of it (or of a condition \
                         it drives, which becomes a blend) would silently diverge from MC \
                         — read an `objective.<name>` instead, or drop `ev_uptime`",
                        what()
                    ),
                });
            }
            let reads_objectives = probe.read.get();
            let reason = match objectives {
                ObjectiveReads::Legal => None,
                ObjectiveReads::Circular => Some(
                    "`damage.stats` is the world the objective is evaluated in, so it \
                     would read its own result (a circular read)",
                ),
                ObjectiveReads::HitsPerUse => Some(
                    "`hits_per_use` is read by the executor before any Plan evaluation, \
                     so no objective value exists yet to read",
                ),
                ObjectiveReads::RefoldTime => Some(
                    "a resource's `max`/`regen_per_sec` is folded at every state change, \
                     outside any one evaluation instant",
                ),
            };
            if let (true, Some(reason)) = (reads_objectives, reason) {
                return Err(PlanError {
                    what: format!(
                        "{}: an `objective.<name>` read is not allowed in this field — {reason}",
                        what()
                    ),
                });
            }
            if let Some((read, buff, via)) = probe.watched.borrow().as_ref() {
                let mut hit = syms.ev_removal_hit.borrow_mut();
                if hit.is_none() {
                    *hit = Some(format!(
                        "this config cannot run in expected-value mode: {} reads `{read}`, \
                         and buff `{buff}` is removed by {via}. EV fires procs on an \
                         averaged schedule, so a rotation reading a proc-removed buff \
                         would branch on a state one deterministic timeline cannot \
                         represent. Run it in Monte Carlo mode, which is exact here",
                        what()
                    ));
                }
            }
            Ok(SimProgram {
                program,
                reads_objectives,
            })
        }
        Err(e) => {
            if let Some(name) = probe.unknown.borrow().as_deref() {
                return Err(PlanError {
                    what: format!(
                        "{}: unknown objective `{name}`{}",
                        what(),
                        syms.objective_hint(name)
                    ),
                });
            }
            Err(PlanError {
                what: format!("{}: {e}", what()),
            })
        }
    }
}

/// Recording wrapper over [`SimSymbols`]: remembers whether any
/// resolved slot fell inside the objective region, and the first
/// `objective.<name>` that failed to resolve.
struct ObjectiveProbe<'a, 'b> {
    inner: &'b SimSymbols<'a>,
    read: Cell<bool>,
    unknown: RefCell<Option<String>>,
    /// The first read of a name an `ev_uptime: expected` buff forbids:
    /// (name read, the buff).
    forbidden: RefCell<Option<(String, String)>>,
    /// The first read of a name in [`SimSymbols::ev_removal_watch`]:
    /// (name read, the buff, how a proc removes it).
    watched: RefCell<Option<(String, String, String)>>,
}

impl Symbols for ObjectiveProbe<'_, '_> {
    fn slot(&self, name: &str) -> Option<u16> {
        let slot = self.inner.slot(name);
        if let Some(w) = self.inner.ev_removal_watch.get(name) {
            let mut r = self.watched.borrow_mut();
            if r.is_none() {
                *r = Some((name.to_string(), w.0.clone(), w.1.clone()));
            }
        }
        if let Some(buff) = self.inner.forbidden.get(name) {
            let mut f = self.forbidden.borrow_mut();
            if f.is_none() {
                *f = Some((name.to_string(), buff.clone()));
            }
        }
        match slot {
            Some(s) if self.inner.is_objective_slot(s as usize) => self.read.set(true),
            None => {
                if let Some(rest) = name.strip_prefix("objective.") {
                    let mut u = self.unknown.borrow_mut();
                    if u.is_none() {
                        *u = Some(rest.to_string());
                    }
                }
            }
            _ => {}
        }
        slot
    }
}

/// Compile a whole `name -> NumOrExpr` map (damage.stats),
/// preserving the source map's (sorted) order.
fn compile_value_map(
    map: &std::collections::BTreeMap<String, NumOrExpr>,
    syms: &SimSymbols<'_>,
    what: impl Fn(&str) -> String,
    objectives: ObjectiveReads,
) -> Result<std::collections::BTreeMap<String, CompiledValue>, PlanError> {
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in map {
        let cv = match v {
            NumOrExpr::Num(n) => CompiledValue::Const(*n),
            NumOrExpr::Expr(src) => {
                let field = if k == "hits_per_use" && objectives == ObjectiveReads::Circular {
                    ObjectiveReads::HitsPerUse
                } else {
                    objectives
                };
                CompiledValue::Expr(compile_sim(src, syms, || what(k), field)?)
            }
        };
        out.insert(k.clone(), cv);
    }
    Ok(out)
}

/// One compiled [`crate::simdef::ResourceDef`]: cap/regen expressions
/// ready to evaluate against the combined `[plan slots | sim slots]`
/// array.
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`].
#[derive(Debug)]
#[non_exhaustive]
pub struct CompiledResource {
    /// This resource's name — also its bare-identifier sim-slot label.
    pub name: String,
    /// Compiled `max` expression.
    pub max: Program,
    /// Compiled `regen_per_sec` expression.
    pub regen_per_sec: Program,
}

/// One compiled [`crate::simdef::ActionDef`]: timing/cost/gain/damage,
/// with resource references resolved to indices into `SimPlan::resources`.
///
/// `#[non_exhaustive]`: the COMPILED representation is the engine's to
/// extend — every sequencing phase so far has added a field to it — and
/// no consumer constructs one (only [`compile`] does). Same category as
/// [`CompiledValue`]; the CONFIG types it mirrors are deliberately NOT
/// marked, since a caller building a [`crate::simdef::SimDef`] in Rust
/// should be able to write a struct literal.
#[derive(Debug)]
#[non_exhaustive]
pub struct CompiledAction {
    /// This action's name.
    pub name: String,
    /// Compiled `cast_time` expression; `None` for a segmented action
    /// (its cast time is the sum of its segments' durations).
    pub cast_time: Option<SimProgram>,
    /// Cooldown in seconds, starting when the cast begins — evaluated at
    /// cast start (see [`crate::simdef::NumOrExpr`]).
    pub cooldown: CompiledValue,
    /// The RESOLVED global cooldown this action arms when it begins —
    /// [`crate::simdef::ActionDef::gcd`] where the config named one, else
    /// [`crate::simdef::SimDefaults::gcd`]. `None` = OFF the GCD (resolved
    /// absent, or the literal `0`): the action neither waits for nor arms
    /// the lock. `Some` = GCD-bound, the value evaluated at cast start.
    pub gcd: Option<CompiledValue>,
    /// Resource cost paid on cast begin: `(resource index, amount)`, the
    /// amount evaluated at cast start (and at every affordability check).
    pub cost: Vec<(usize, CompiledValue)>,
    /// Resource gain on cast complete: `(resource index, amount)`, the
    /// amount evaluated at cast complete.
    pub gain: Vec<(usize, CompiledValue)>,
    /// Compiled per-cast stat override map (from `ActionDamage::stats`),
    /// if this action deals damage; every value is evaluated at cast
    /// complete. `hits_per_use` (default `1.0` if absent) is read directly
    /// out of this map by the executor rather than fed into the `Plan` as
    /// a stat — see `simdef::ActionDamage` docs.
    pub damage: Option<std::collections::BTreeMap<String, CompiledValue>>,
    /// Effects this action executes at cast complete, resolved to indices
    /// and kept in the CONFIG's order, which is the execution order (see
    /// [`crate::simdef::ActionDef::effects`] for where in the
    /// completion instant they land and what a repeat means). Both the
    /// deprecated `apply_buff` sugar and an explicit `effects` list
    /// desugar HERE, so the executor has one path. Only
    /// [`CompiledEffect::ApplyBuff`] and [`CompiledEffect::RemoveBuff`]
    /// can appear — [`compile`] rejects a `cast_action` effect on an
    /// action (recursion; see that error).
    pub effects: Vec<CompiledEffect>,
    /// The RESOLVED measurement instant for this action's casts (P8c):
    /// [`crate::simdef::ActionDef::measure`] where the config named one,
    /// else [`crate::simdef::SimDefaults::measure`] — the executor never
    /// consults the `defaults` block again. See [`Measure`] for what the
    /// instant governs and its interactions.
    pub measure: Measure,
    /// The RESOLVED crit-sampling policy for this action's hits:
    /// [`crate::simdef::ActionDef::crit_rolls`] where the config named
    /// one, else [`crate::simdef::SimDefaults::proc_rolls`]. See the
    /// config field for the semantics.
    pub crit_rolls: ProcRolls,
    /// The compiled segments, in order; empty for an unsegmented action.
    /// See [`crate::simdef::ActionDef::segments`].
    pub segments: Vec<CompiledSegment>,
}

/// One compiled [`crate::simdef::SegmentDef`]: duration, damage and
/// effects with references resolved, and the measurement instant
/// RESOLVED as `segment.measure ?? action.measure ?? defaults.measure`.
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`].
#[derive(Debug)]
#[non_exhaustive]
pub struct CompiledSegment {
    /// Seconds from the previous boundary to this landing, evaluated at
    /// this segment's start.
    pub duration: CompiledValue,
    /// Compiled `damage.stats`, each evaluated at the resolved measure
    /// instant; `None` is a wind-up segment with no hits.
    pub damage: Option<std::collections::BTreeMap<String, CompiledValue>>,
    /// Effects executed at this segment's landing, in list order. Only
    /// [`CompiledEffect::ApplyBuff`] and [`CompiledEffect::RemoveBuff`]
    /// can appear.
    pub effects: Vec<CompiledEffect>,
    /// The RESOLVED measurement instant for this segment.
    pub measure: Measure,
}

/// One compiled [`crate::simdef::BuffDef`].
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`], and
/// with the sharpest evidence for it: P7c added `max_stacks`/`on_reapply`
/// here and P7c-T2 changed `tick_objective`'s TYPE
/// (`Option<usize>` → `Option<CompiledTick>`).
#[derive(Debug)]
#[non_exhaustive]
pub struct CompiledBuff {
    /// This buff's name.
    pub name: String,
    /// Duration in seconds once applied — evaluated at EACH application
    /// and snapshotted onto the instance it starts (see
    /// [`crate::simdef::NumOrExpr`]).
    pub duration: CompiledValue,
    /// Bucket contributions active while this buff is up; each value is
    /// folded MULTIPLIED BY the live stack count (see
    /// [`crate::simdef::BuffDef`]).
    pub contributions: Vec<Contribution>,
    /// Condition name → value while this buff is active (wins over the
    /// scenario's static uptime for that condition while active). NOT
    /// scaled by the stack count — see [`crate::simdef::BuffDef`].
    pub conditions: std::collections::BTreeMap<String, f64>,
    /// The resolved `tick_objective`, if this buff DoT-ticks.
    pub tick_objective: Option<CompiledTick>,
    /// Maximum live instances; `0` = unbounded (see
    /// [`crate::simdef::BuffDef::max_stacks`]).
    pub max_stacks: u32,
    /// What an application does when this buff is already active (see
    /// [`ReapplyPolicy`]). [`ReapplyPolicy::Strongest`] reaching here
    /// implies a snapshot `tick_objective` and `max_stacks == 1` —
    /// [`compile`] rejects it otherwise.
    pub on_reapply: ReapplyPolicy,
    /// How EV mode tracks this buff's uptime (see
    /// [`crate::simdef::BuffDef::ev_uptime`]). [`EvUptime::Expected`]
    /// reaching here implies every compile-time constraint listed there
    /// held — [`compile`] rejects the config otherwise.
    pub ev_uptime: EvUptime,
}

/// One compiled [`crate::simdef::TickObjective`]: the objective RESOLVED
/// to its index in the `Plan`'s objective slice, plus how the executor
/// samples it.
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CompiledTick {
    /// Index into the `Plan`'s objective slice.
    pub objective: usize,
    /// `true` — each instance ticks the rate it captured at its own
    /// application, and the buff's rate is the SUM over instances;
    /// `false` — the rate is re-evaluated live and multiplied by the
    /// instance count. See [`crate::simdef::TickObjective::snapshot`].
    pub snapshot: bool,
}

/// One compiled [`crate::simdef::EffectDef`], resolved to an index —
/// one entry of a [`CompiledProc::effects`] / [`CompiledAction::effects`]
/// list (P8b renamed this from `ProcEffect` when actions gained the same
/// list; the deprecated sugar fields desugar into these lists at
/// [`compile`], so the executor has exactly one effect path).
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`]: a later
/// phase adding a third effect kind should not be a breaking change for a
/// downstream `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompiledEffect {
    /// Apply the buff at this index in `SimPlan::buffs`.
    ApplyBuff(usize),
    /// Cast the action at this index in `SimPlan::actions`.
    CastAction(usize),
    /// End every live instance of the buff at this index in
    /// `SimPlan::buffs` (see [`crate::simdef::EffectDef::RemoveBuff`]).
    RemoveBuff(usize),
}

/// One compiled [`crate::simdef::ProcDef`].
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`].
#[derive(Debug)]
#[non_exhaustive]
pub struct CompiledProc {
    /// This proc's name.
    pub name: String,
    /// Which event this proc rolls its chance against.
    pub trigger: Trigger,
    /// Compiled `chance` expression.
    pub chance: SimProgram,
    /// Internal cooldown in seconds after firing.
    pub icd: f64,
    /// What firing this proc does: its effects, resolved to indices, in
    /// EXECUTION order. Never empty — [`compile`] rejects a proc with
    /// nothing to do ("a proc must do something"). A one-entry list is
    /// what either deprecated sugar field desugars to.
    pub effects: Vec<CompiledEffect>,
    /// Trigger filter, resolved to indices into [`SimPlan::actions`]:
    /// `None` = every action (the 0.2.0 behavior), `Some(list)` = only
    /// casts of those actions produce a qualifying event. Never
    /// `Some(empty)` — [`compile`] rejects that (see
    /// [`crate::simdef::ProcDef::actions`]).
    pub actions: Option<Vec<usize>>,
    /// The RESOLVED roll policy for this proc (P8e):
    /// [`crate::simdef::ProcDef::rolls`] where the config named one, else
    /// [`crate::simdef::SimDefaults::proc_rolls`] — the executor never
    /// consults the `defaults` block again. See [`ProcRolls`] for what
    /// the policy means.
    pub rolls: ProcRolls,
    /// Indices into [`SimPlan::buffs`] of the [`EvUptime::Expected`]
    /// buffs this proc's `effects` apply: deduped (a buff named twice is
    /// one event), in index order. Empty for a proc applying none.
    pub expected_buffs: Vec<usize>,
}

/// One compiled [`crate::simdef::Rule`]: the action index it casts and its
/// optional `when` predicate.
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`].
#[derive(Debug)]
#[non_exhaustive]
pub struct CompiledRule {
    /// Index into `SimPlan::actions`.
    pub action: usize,
    /// Compiled `when` predicate, if present (absent = always eligible,
    /// subject to the executor's automatic hard gates).
    pub when: Option<SimProgram>,
    /// Whether a cast this rule starts is player-cancellable at its
    /// non-last segment boundaries. Already resolved: true if the rule's
    /// `interrupt` or its `interrupt_if` is set.
    pub interrupt: bool,
    /// Compiled `interrupt_if` predicate, if present (absent = cancellable
    /// at every non-last boundary when `interrupt`).
    pub interrupt_if: Option<SimProgram>,
}

/// A `SimDef` + `Rotation` compiled once against a [`Plan`]: every
/// expression parsed and slot-resolved over the extended sim symbol space,
/// every cross-reference validated fail-closed (see `sim` module docs),
/// ready for [`crate::sim::run`] to drive. Inert data — no execution logic
/// lives here.
///
/// `#[non_exhaustive]` for the same reason as [`CompiledAction`]: every
/// sequencing phase so far has added to it, and only [`compile`]
/// constructs one.
#[derive(Debug)]
#[non_exhaustive]
pub struct SimPlan {
    /// `Some(message)` when this plan cannot run in `Mode::Expected`: a
    /// sim expression reads a buff that a proc removes (see
    /// [`crate::simdef::EffectDef::RemoveBuff`]). [`crate::sim::run`]
    /// returns the message as its error in EV; Monte Carlo ignores it.
    pub ev_inexact_removal: Option<String>,
    /// The slot offset where the sim-state segment begins — equal to the
    /// underlying `Plan`'s own unified slot width
    /// (`[stats | conditions | buckets | stages | event_factors]`). Sim
    /// expressions load plan stats/conditions from slots below this
    /// offset and sim state from slots at/above it.
    pub sim_base: usize,
    /// Total width of the combined `[plan slots | sim slots]` array the
    /// executor allocates (see [`crate::sim::run`]): the plan prefix,
    /// the sim-state words, the two segment words, and LAST the
    /// `objective.<name>` region — so `slot_width == objective_base +`
    /// the Plan's objective count (see the `sim` module's slot-layout
    /// docs).
    pub slot_width: usize,
    /// The `segments_done` word's slot; `segments_remain` is the next
    /// one (`segments_done_slot + 1`). Both sit just before
    /// [`SimPlan::objective_base`].
    pub segments_done_slot: usize,
    /// First slot of the `objective.<name>` region: one slot per Plan
    /// objective in `Plan::objective_names()` order, appended after the
    /// segment words. The executor writes the WHOLE region (one
    /// deterministic `Plan::evaluate_phase` against the field's world)
    /// immediately before evaluating any program whose
    /// [`SimProgram::reads_objectives`] is set; nothing else reads or
    /// writes it.
    pub objective_base: usize,
    /// Resources, compiled, in name-sorted order — also their bare
    /// identifier's sim-slot order (`sim_base + 2 + i`).
    pub resources: Vec<CompiledResource>,
    /// Actions, compiled, in name-sorted order — also the
    /// `cooldown.<name>`/`casts.<name>` sim-slot order.
    pub actions: Vec<CompiledAction>,
    /// Buffs, compiled, in name-sorted order — also the
    /// `buff.<name>`/`buff_remaining.<name>` sim-slot order.
    pub buffs: Vec<CompiledBuff>,
    /// Procs, compiled, in name-sorted order, with resolved effect
    /// targets.
    pub procs: Vec<CompiledProc>,
    /// Condition name → index into [`SimPlan::buffs`] of the
    /// [`EvUptime::Expected`] buff that drives it. [`compile`] guarantees
    /// at most one driver per such condition.
    pub condition_expected: BTreeMap<String, usize>,
    /// Rotation rules, compiled, in priority (source) order, with
    /// resolved action indices.
    pub rules: Vec<CompiledRule>,
    /// Index into the `Plan`'s objective slice for `damage_objective`.
    pub damage_objective: usize,
    /// The coincident-event ordering for the whole run (P8d) —
    /// [`crate::simdef::SimDefaults::event_order`], carried whole rather
    /// than resolved per entity (it is SimDef-global by design; see
    /// [`EventOrder`] for why a per-entity override would be
    /// incoherent). The executor reads THIS field at every event push,
    /// never the `defaults` block.
    pub event_order: EventOrder,
}

/// Compile-time symbol table over the extended sim space: the `Plan`'s own
/// flat namespace (stats + conditions ONLY — buckets/stages stay
/// invisible), plus `time`/`duration`/resources/`cooldown.*`/`buff.*`/
/// `buff_remaining.*`/`casts.*`/`stacks.*`, laid out in the documented
/// order (see `sim` module docs).
struct SimSymbols<'a> {
    plan: &'a Plan,
    time_slot: usize,
    duration_slot: usize,
    segments_done_slot: usize,
    segments_remain_slot: usize,
    objective_names: Vec<&'a str>,
    objective_base: usize,
    resource_names: Vec<&'a str>,
    resource_base: usize,
    action_names: Vec<&'a str>,
    cooldown_base: usize,
    buff_names: Vec<&'a str>,
    buff_base: usize,
    buff_remaining_base: usize,
    casts_base: usize,
    stacks_base: usize,
    /// Names no sim expression may read: for each `ev_uptime: expected`
    /// buff, its `buff.`/`buff_remaining.`/`stacks.` words and every
    /// condition it drives, each mapped to that buff's name.
    forbidden: BTreeMap<String, String>,
    /// Names whose read makes EV inexact: for each buff a PROC removes
    /// (directly, or via the free cast its `cast_action` runs), its
    /// `buff.`/`buff_remaining.`/`stacks.` words and every condition it
    /// drives, each mapped to `(buff, "proc `p`" or "proc `p`'s free cast
    /// `a`")`.
    ev_removal_watch: BTreeMap<String, (String, String)>,
    /// The first watched read [`compile_sim`] saw, as the full error
    /// message [`crate::sim::run`] returns in EV.
    ev_removal_hit: RefCell<Option<String>>,
}

impl<'a> SimSymbols<'a> {
    fn new(plan: &'a Plan, simdef: &'a SimDef) -> Self {
        let base = plan.own_slot_width();
        let resource_names: Vec<&str> = simdef.resources.keys().map(String::as_str).collect();
        let action_names: Vec<&str> = simdef.actions.keys().map(String::as_str).collect();
        let buff_names: Vec<&str> = simdef.buffs.keys().map(String::as_str).collect();
        let resource_base = base + 2;
        let cooldown_base = resource_base + resource_names.len();
        let buff_base = cooldown_base + action_names.len();
        let buff_remaining_base = buff_base + buff_names.len();
        let casts_base = buff_remaining_base + buff_names.len();
        let stacks_base = casts_base + action_names.len();
        // The two segment words sit AFTER the `stacks.*` range so every
        // pre-existing slot keeps its index.
        let segments_done_slot = stacks_base + buff_names.len();
        let mut forbidden = BTreeMap::new();
        for (name, b) in &simdef.buffs {
            if b.ev_uptime == EvUptime::Expected {
                for prefix in ["buff.", "buff_remaining.", "stacks."] {
                    forbidden.insert(format!("{prefix}{name}"), name.clone());
                }
                for c in b.conditions.keys() {
                    forbidden.insert(c.clone(), name.clone());
                }
            }
        }
        let mut ev_removal_watch = BTreeMap::new();
        let removes = |effects: &[EffectDef], b: &str| {
            effects
                .iter()
                .any(|e| matches!(e, EffectDef::RemoveBuff(x) if x == b))
        };
        for (pname, p) in &simdef.procs {
            for (bname, b) in &simdef.buffs {
                let mut via = removes(&p.effects, bname).then(|| format!("proc `{pname}`"));
                if via.is_none() {
                    via = p
                        .cast_action
                        .iter()
                        .chain(p.effects.iter().filter_map(|e| match e {
                            EffectDef::CastAction(a) => Some(a),
                            _ => None,
                        }))
                        .find(|a| {
                            simdef
                                .actions
                                .get(*a)
                                .is_some_and(|d| removes(&d.effects, bname))
                        })
                        .map(|a| format!("proc `{pname}`'s free cast `{a}`"));
                }
                if let Some(via) = via {
                    for prefix in ["buff.", "buff_remaining.", "stacks."] {
                        ev_removal_watch
                            .entry(format!("{prefix}{bname}"))
                            .or_insert_with(|| (bname.clone(), via.clone()));
                    }
                    for c in b.conditions.keys() {
                        ev_removal_watch
                            .entry(c.clone())
                            .or_insert_with(|| (bname.clone(), via.clone()));
                    }
                }
            }
        }
        SimSymbols {
            ev_removal_watch,
            ev_removal_hit: RefCell::new(None),
            forbidden,
            plan,
            time_slot: base,
            duration_slot: base + 1,
            segments_done_slot,
            segments_remain_slot: segments_done_slot + 1,
            objective_base: segments_done_slot + 2,
            objective_names: plan.objective_names(),
            resource_names,
            resource_base,
            action_names,
            cooldown_base,
            buff_names,
            buff_base,
            buff_remaining_base,
            casts_base,
            stacks_base,
        }
    }

    fn sim_base(&self) -> usize {
        self.time_slot
    }

    /// The objective region is appended AFTER the segment words, one slot
    /// per Plan objective in `Plan::objective_names()` order, so every
    /// pre-existing slot keeps its index.
    fn slot_width(&self) -> usize {
        self.objective_base + self.objective_names.len()
    }

    fn is_objective_slot(&self, slot: usize) -> bool {
        slot >= self.objective_base && slot < self.objective_base + self.objective_names.len()
    }

    /// Suffix for an unknown-objective error: the nearest objective name
    /// as a did-you-mean, else the full list.
    fn objective_hint(&self, name: &str) -> String {
        match crate::config_keys::nearest(name, &self.objective_names) {
            Some(best) => format!(" — did you mean `{best}`?"),
            None => format!(
                "; expected one of: {}",
                self.objective_names
                    .iter()
                    .map(|o| format!("`{o}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl Symbols for SimSymbols<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        if name == "time" {
            return Some(self.time_slot as u16);
        }
        if name == "duration" {
            return Some(self.duration_slot as u16);
        }
        if name == "segments_done" {
            return Some(self.segments_done_slot as u16);
        }
        if name == "segments_remain" {
            return Some(self.segments_remain_slot as u16);
        }
        if let Some(i) = self.resource_names.iter().position(|r| *r == name) {
            return Some((self.resource_base + i) as u16);
        }
        if let Some(rest) = name.strip_prefix("objective.") {
            return self
                .objective_names
                .iter()
                .position(|o| *o == rest)
                .map(|i| (self.objective_base + i) as u16);
        }
        if let Some(rest) = name.strip_prefix("cooldown.") {
            return self
                .action_names
                .iter()
                .position(|a| *a == rest)
                .map(|i| (self.cooldown_base + i) as u16);
        }
        if let Some(rest) = name.strip_prefix("buff_remaining.") {
            return self
                .buff_names
                .iter()
                .position(|b| *b == rest)
                .map(|i| (self.buff_remaining_base + i) as u16);
        }
        if let Some(rest) = name.strip_prefix("buff.") {
            return self
                .buff_names
                .iter()
                .position(|b| *b == rest)
                .map(|i| (self.buff_base + i) as u16);
        }
        if let Some(rest) = name.strip_prefix("casts.") {
            return self
                .action_names
                .iter()
                .position(|a| *a == rest)
                .map(|i| (self.casts_base + i) as u16);
        }
        if let Some(rest) = name.strip_prefix("stacks.") {
            return self
                .buff_names
                .iter()
                .position(|b| *b == rest)
                .map(|i| (self.stacks_base + i) as u16);
        }
        // Fall back to the plan's OWN flat namespace: stats + conditions
        // only. Buckets and pipeline stages are never registered here, so
        // a sim expression naming one falls through to `None` below —
        // fail-closed, same as any other unknown identifier.
        if let Some(i) = self.plan.stat_id(name) {
            return Some(i as u16);
        }
        if let Some(i) = self.plan.condition_id(name) {
            return Some(i as u16);
        }
        None
    }
}

/// Compile a `SimDef` + `Rotation` against an already-compiled `Plan`:
/// validate every cross-reference fail-closed (unknown action/buff/
/// resource references, proc effect arity, objective membership, reserved/
/// colliding names — see `sim` module docs), then parse and slot-resolve
/// every expression over the extended sim symbol space. This is the only
/// place sim expressions get parsed — do it once and reuse the result.
pub fn compile(plan: &Plan, simdef: &SimDef, rotation: &Rotation) -> Result<SimPlan, PlanError> {
    // P8a: unknown config keys fail closed FIRST — a typo'd key silently
    // meaning "field absent" is the root cause every later check would
    // only see the symptom of (`tick_objectiv` silently meaning "no DoT"
    // was the standing example). Each entity's registry name is the error
    // context; `_`-prefixed keys are the annotation namespace and pass.
    crate::config_keys::reject_unknown("the simdef", SimDef::KNOWN_KEYS, &simdef.extra)?;
    crate::config_keys::reject_unknown(
        "the defaults block",
        crate::simdef::SimDefaults::KNOWN_KEYS,
        &simdef.defaults.extra,
    )?;
    for (name, r) in &simdef.resources {
        crate::config_keys::reject_unknown(
            &format!("resource `{name}`"),
            crate::simdef::ResourceDef::KNOWN_KEYS,
            &r.extra,
        )?;
    }
    for (name, a) in &simdef.actions {
        crate::config_keys::reject_unknown(
            &format!("action `{name}`"),
            crate::simdef::ActionDef::KNOWN_KEYS,
            &a.extra,
        )?;
        if let Some(dmg) = &a.damage {
            crate::config_keys::reject_unknown(
                &format!("action `{name}` damage"),
                crate::simdef::ActionDamage::KNOWN_KEYS,
                &dmg.extra,
            )?;
        }
        for (i, seg) in a.segments.iter().enumerate() {
            crate::config_keys::reject_unknown(
                &format!("action `{name}` segment {i}"),
                crate::simdef::SegmentDef::KNOWN_KEYS,
                &seg.extra,
            )?;
            if let Some(dmg) = &seg.damage {
                crate::config_keys::reject_unknown(
                    &format!("action `{name}` segment {i} damage"),
                    crate::simdef::ActionDamage::KNOWN_KEYS,
                    &dmg.extra,
                )?;
            }
        }
    }
    for (name, b) in &simdef.buffs {
        crate::config_keys::reject_unknown(
            &format!("buff `{name}`"),
            crate::simdef::BuffDef::KNOWN_KEYS,
            &b.extra,
        )?;
    }
    for (name, p) in &simdef.procs {
        crate::config_keys::reject_unknown(
            &format!("proc `{name}`"),
            crate::simdef::ProcDef::KNOWN_KEYS,
            &p.extra,
        )?;
    }
    crate::config_keys::reject_unknown(
        "the rotation",
        crate::simdef::Rotation::KNOWN_KEYS,
        &rotation.extra,
    )?;
    for (i, rule) in rotation.rules.iter().enumerate() {
        crate::config_keys::reject_unknown(
            &format!("rotation rule {i} (action `{}`)", rule.action),
            crate::simdef::Rule::KNOWN_KEYS,
            &rule.extra,
        )?;
    }

    // Reserved words + flat-namespace collisions: every resource/action/
    // buff name must be neither a reserved sim word nor already a stat/
    // condition name in the underlying plan. This codebase treats stats,
    // conditions, and sim names as ONE flat namespace (mirrors plan.rs's
    // own stats/conditions/buckets/stages collision check).
    for name in simdef
        .resources
        .keys()
        .chain(simdef.actions.keys())
        .chain(simdef.buffs.keys())
    {
        if name == "time"
            || name == "duration"
            || name == "segments_done"
            || name == "segments_remain"
        {
            return Err(PlanError {
                what: format!("`{name}` is reserved and cannot be used as a sim name"),
            });
        }
        if plan.stat_id(name).is_some() || plan.condition_id(name).is_some() {
            return Err(PlanError {
                what: format!("sim name `{name}` collides with an existing stat/condition"),
            });
        }
    }

    // damage_objective must name a plan objective.
    let objective_names = plan.objective_names();
    let damage_objective = objective_names
        .iter()
        .position(|o| *o == simdef.damage_objective)
        .ok_or_else(|| PlanError {
            what: format!(
                "damage_objective `{}` is not a plan objective",
                simdef.damage_objective
            ),
        })?;

    // Rotation rules: the action they cast must be defined.
    for (i, rule) in rotation.rules.iter().enumerate() {
        if !simdef.actions.contains_key(&rule.action) {
            return Err(PlanError {
                what: format!("rotation rule {i}: unknown action `{}`", rule.action),
            });
        }
    }

    // Player cancels: explicit `interrupt: false` with `interrupt_if`
    // contradicts; an interrupting rule needs a non-last segment boundary
    // (>= 2 segments). Fail closed, naming the rule's position and action.
    for (i, rule) in rotation.rules.iter().enumerate() {
        if rule.interrupt == Some(false) && rule.interrupt_if.is_some() {
            return Err(PlanError {
                what: format!(
                    "rotation rule {i} (action `{}`): `interrupt: false` contradicts \
                     `interrupt_if` (which implies `interrupt: true`) — drop one",
                    rule.action
                ),
            });
        }
        if rule.interrupt == Some(true) || rule.interrupt_if.is_some() {
            let segs = simdef.actions[&rule.action].segments.len();
            if segs < 2 {
                return Err(PlanError {
                    what: format!(
                        "rotation rule {i} (action `{}`): `interrupt` needs an action \
                         with at least 2 segments (a non-last boundary to cancel at), \
                         but `{}` has {segs}",
                        rule.action, rule.action
                    ),
                });
            }
        }
    }

    // Actions: cost/gain resource references and effect targets must be
    // defined. The deprecated `apply_buff` sugar and the `effects` list
    // are validated under their OWN spellings (the messages a config's
    // author actually wrote), and combining them is refused outright —
    // desugaring would have to pick where the sugar lands relative to
    // the list, and a guessed order is a silent wrong answer.
    for (name, action) in &simdef.actions {
        // Segments: the cast time is EITHER one `cast_time` OR the sum of
        // the segments, and damage lives on the segments — each ambiguous
        // combination is refused rather than guessed at.
        match (&action.cast_time, action.segments.is_empty()) {
            (Some(_), false) => {
                return Err(PlanError {
                    what: format!(
                        "action `{name}`: both `segments` and `cast_time` are set — \
                         ambiguous cast time; a segmented action's cast time is the sum \
                         of its segment durations, so drop `cast_time`"
                    ),
                });
            }
            (None, true) => {
                return Err(PlanError {
                    what: format!(
                        "action `{name}`: neither `cast_time` nor `segments` is set — an \
                         action needs one of them"
                    ),
                });
            }
            _ => {}
        }
        if !action.segments.is_empty() && action.damage.is_some() {
            return Err(PlanError {
                what: format!(
                    "action `{name}`: `segments` together with action-level `damage` is \
                     ambiguous — put the damage on the segments"
                ),
            });
        }
        for (i, seg) in action.segments.iter().enumerate() {
            for effect in &seg.effects {
                match effect {
                    EffectDef::CastAction(_) => {
                        return Err(PlanError {
                            what: format!(
                                "action `{name}` segment {i}: a `cast_action` effect is not \
                                 allowed on a segment — an action free-casting an action \
                                 reopens recursion (A→B→A)"
                            ),
                        });
                    }
                    EffectDef::ApplyBuff(b) | EffectDef::RemoveBuff(b) => {
                        if !simdef.buffs.contains_key(b) {
                            return Err(PlanError {
                                what: format!(
                                    "action `{name}` segment {i}: unknown buff `{b}` in effects"
                                ),
                            });
                        }
                    }
                }
            }
        }
        for r in action.cost.keys().chain(action.gain.keys()) {
            if !simdef.resources.contains_key(r) {
                return Err(PlanError {
                    what: format!("action `{name}`: unknown resource `{r}` in cost/gain"),
                });
            }
        }
        if !action.apply_buff.is_empty() && !action.effects.is_empty() {
            return Err(PlanError {
                what: format!(
                    "action `{name}`: the deprecated `apply_buff` sugar AND an explicit \
                     `effects` list are both set — ambiguous order; migrate the sugar \
                     into the `effects` list"
                ),
            });
        }
        for b in &action.apply_buff {
            if !simdef.buffs.contains_key(b) {
                return Err(PlanError {
                    what: format!("action `{name}`: unknown buff `{b}` in apply_buff"),
                });
            }
        }
        for effect in &action.effects {
            match effect {
                // `cast_action` stays PROC-only: an action free-casting
                // an action reopens the recursion the free-cast guard
                // closed (a free cast rolls no procs, so today's chains
                // are one link long by construction). A bounded-depth
                // combo design should be chosen by a config that needs
                // one — see ROADMAP.md — not guessed at here.
                EffectDef::CastAction(_) => {
                    return Err(PlanError {
                        what: format!(
                            "action `{name}`: a `cast_action` effect is not allowed on an \
                             action — an action free-casting an action reopens recursion \
                             (A→B→A), which the free-cast guard exists to close; a \
                             bounded-depth chain design is tracked in ROADMAP.md"
                        ),
                    });
                }
                EffectDef::ApplyBuff(b) | EffectDef::RemoveBuff(b) => {
                    if !simdef.buffs.contains_key(b) {
                        return Err(PlanError {
                            what: format!("action `{name}`: unknown buff `{b}` in effects"),
                        });
                    }
                }
            }
        }
    }

    // Buffs: tick_objective must name a plan objective, the stack policy
    // must be one this engine actually honors, and every contribution
    // value must be finite. The finiteness check is the sim half of the
    // shared `Contribution` type (`Plan` guards its own half at build
    // resolution): a NaN here would fold into every capture and come back
    // `Ok(NaN)` — the 0.3.0 release review's standing repro.
    for (name, buff) in &simdef.buffs {
        for c in &buff.contributions {
            if !c.value.is_finite() {
                return Err(PlanError {
                    what: format!(
                        "buff `{name}`: contribution value into bucket `{}` must be \
                         finite, got {}",
                        c.bucket, c.value
                    ),
                });
            }
        }
        if let Some(t) = &buff.tick_objective {
            if !objective_names.iter().any(|o| *o == t.objective) {
                return Err(PlanError {
                    what: format!(
                        "buff `{name}`: tick_objective `{}` is not a plan objective",
                        t.objective
                    ),
                });
            }
        }
        // `strongest` compares candidate instances BY THEIR SNAPSHOT RATE,
        // and replaces rather than stacks. Both preconditions are checked
        // here so the executor's arm can rely on them: a live
        // `tick_objective` has a rate but not a per-INSTANCE one, and a cap
        // above 1 would describe a stack this policy never builds.
        if buff.on_reapply == ReapplyPolicy::Strongest {
            if !buff.tick_objective.as_ref().is_some_and(|t| t.snapshot) {
                return Err(PlanError {
                    what: format!(
                        "buff `{name}`: on_reapply `strongest` compares instances by \
                         their snapshot rate, so it requires a tick_objective with \
                         `snapshot: true` (got {})",
                        match &buff.tick_objective {
                            None => "none".to_string(),
                            Some(t) => format!("the live objective `{}`", t.objective),
                        }
                    ),
                });
            }
            if buff.max_stacks != 1 {
                return Err(PlanError {
                    what: format!(
                        "buff `{name}`: on_reapply `strongest` REPLACES the live \
                         instance, so max_stacks must be 1 (got {}) — use \
                         `add_independent` to stack independently-expiring instances",
                        buff.max_stacks
                    ),
                });
            }
        }
        // `refresh` keeps exactly one instance by construction, so any
        // other `max_stacks` alongside it is a config the author got
        // wrong — say so rather than silently ignore the number.
        if buff.on_reapply == ReapplyPolicy::Refresh && buff.max_stacks != 1 {
            return Err(PlanError {
                what: format!(
                    "buff `{name}`: on_reapply `refresh` keeps exactly one instance, \
                     so max_stacks must be 1 (got {}) — use `add_refresh_all` or \
                     `add_independent` to stack",
                    buff.max_stacks
                ),
            });
        }
    }

    // Procs: the effects list, under the same rules as the action side —
    // sugar and the explicit list validated under their own spellings,
    // combining them refused — plus the proc-specific floor: a proc with
    // NOTHING to do after desugar is a config mistake, not a way to
    // disable one (the old "exactly one of apply_buff/cast_action"
    // check, generalized; BOTH sugar fields at once stays an error with
    // its long-standing message).
    for (name, p) in &simdef.procs {
        if p.apply_buff.is_some() && p.cast_action.is_some() {
            return Err(PlanError {
                what: format!("proc `{name}`: exactly one of apply_buff/cast_action, got both"),
            });
        }
        if (p.apply_buff.is_some() || p.cast_action.is_some()) && !p.effects.is_empty() {
            return Err(PlanError {
                what: format!(
                    "proc `{name}`: the deprecated `apply_buff`/`cast_action` sugar AND an \
                     explicit `effects` list are both set — ambiguous order; migrate the \
                     sugar into the `effects` list"
                ),
            });
        }
        if p.apply_buff.is_none() && p.cast_action.is_none() && p.effects.is_empty() {
            return Err(PlanError {
                what: format!(
                    "proc `{name}`: no effects — a proc must do something; give it an \
                     `effects` list (or one of the deprecated `apply_buff`/`cast_action` \
                     sugar fields)"
                ),
            });
        }
        if let Some(b) = &p.apply_buff {
            if !simdef.buffs.contains_key(b) {
                return Err(PlanError {
                    what: format!("proc `{name}`: unknown buff `{b}`"),
                });
            }
        }
        if let Some(a) = &p.cast_action {
            if !simdef.actions.contains_key(a) {
                return Err(PlanError {
                    what: format!("proc `{name}`: unknown action `{a}`"),
                });
            }
        }
        // A free cast has no cast window to segment: free-casting a
        // segmented action is out of scope and refused, not approximated.
        for a in p
            .cast_action
            .iter()
            .chain(p.effects.iter().filter_map(|e| match e {
                EffectDef::CastAction(a) => Some(a),
                _ => None,
            }))
        {
            if simdef
                .actions
                .get(a)
                .is_some_and(|d| !d.segments.is_empty())
            {
                return Err(PlanError {
                    what: format!(
                        "proc `{name}`: `cast_action` names `{a}`, a segmented action — a \
                         free cast of a segmented action is not supported"
                    ),
                });
            }
        }
        for effect in &p.effects {
            match effect {
                EffectDef::ApplyBuff(b) | EffectDef::RemoveBuff(b) => {
                    if !simdef.buffs.contains_key(b) {
                        return Err(PlanError {
                            what: format!("proc `{name}`: unknown buff `{b}` in effects"),
                        });
                    }
                }
                EffectDef::CastAction(a) => {
                    if !simdef.actions.contains_key(a) {
                        return Err(PlanError {
                            what: format!("proc `{name}`: unknown action `{a}` in effects"),
                        });
                    }
                }
            }
        }
        // The trigger filter. `None` is "every action" (0.2.0); an EMPTY
        // list is the config mistake that reads like `None` and means the
        // opposite — a proc that can never fire.
        match &p.actions {
            None => {}
            Some(list) if list.is_empty() => {
                return Err(PlanError {
                    what: format!(
                        "proc `{name}`: the `actions` trigger filter is empty, so this \
                         proc could never fire — omit the key (or write null) for \
                         `every action`"
                    ),
                })
            }
            Some(list) => {
                for a in list {
                    if !simdef.actions.contains_key(a) {
                        return Err(PlanError {
                            what: format!(
                                "proc `{name}`: unknown action `{a}` in the `actions` \
                                 trigger filter"
                            ),
                        });
                    }
                }
            }
        }
    }

    // `ev_uptime: expected` constraints (see `BuffDef::ev_uptime`). The
    // two READ constraints (buff words, driven conditions) are enforced
    // where every sim expression compiles (`compile_sim`'s probe).
    for (name, buff) in &simdef.buffs {
        if buff.ev_uptime != EvUptime::Expected {
            continue;
        }
        if buff.on_reapply != ReapplyPolicy::Refresh || buff.max_stacks != 1 {
            return Err(PlanError {
                what: format!(
                    "buff `{name}`: `ev_uptime: expected` requires on_reapply `refresh` \
                     and max_stacks 1 (got {:?}, max_stacks {}) — stacking needs a \
                     distribution over stack counts, not one probability",
                    buff.on_reapply, buff.max_stacks
                ),
            });
        }
        if buff.tick_objective.as_ref().is_some_and(|t| t.snapshot) {
            return Err(PlanError {
                what: format!(
                    "buff `{name}`: `ev_uptime: expected` cannot have a snapshot \
                     tick_objective — a rate captured per instance has no expected-state \
                     reading (a live tick_objective is fine)"
                ),
            });
        }
        for c in buff.conditions.keys() {
            if let Some(other) = simdef
                .buffs
                .iter()
                .find(|(n, b)| *n != name && b.conditions.contains_key(c))
                .map(|(n, _)| n)
            {
                return Err(PlanError {
                    what: format!(
                        "buff `{name}`: `ev_uptime: expected` cannot share its driven \
                         condition `{c}` with buff `{other}` — the name-order precedence \
                         rule has no exact fractional form"
                    ),
                });
            }
        }
        for (i, c) in buff.contributions.iter().enumerate() {
            if let Some(cond) = c.condition.as_deref() {
                if buff.conditions.contains_key(cond) {
                    return Err(PlanError {
                        what: format!(
                            "buff `{name}`: `ev_uptime: expected` cannot gate its \
                             contribution {i} (bucket `{}`) on condition `{cond}`, which \
                             this buff drives — under EV the gate reads the blended \
                             P·v + (1 − P)·static, so the contribution would count \
                             P × blend where MC counts it only while up (P × v): a \
                             self-correlation with no exact expected form",
                            c.bucket
                        ),
                    });
                }
            }
        }
        let removed_by = |effects: &[EffectDef]| {
            effects
                .iter()
                .any(|e| matches!(e, EffectDef::RemoveBuff(x) if x == name))
        };
        let remover = simdef
            .actions
            .iter()
            .find(|(_, d)| {
                removed_by(&d.effects) || d.segments.iter().any(|sg| removed_by(&sg.effects))
            })
            .map(|(a, _)| format!("action `{a}`"))
            .or_else(|| {
                simdef
                    .procs
                    .iter()
                    .find(|(_, p)| removed_by(&p.effects))
                    .map(|(p, _)| format!("proc `{p}`"))
            });
        if let Some(remover) = remover {
            return Err(PlanError {
                what: format!(
                    "buff `{name}`: `ev_uptime: expected` cannot be the target of a \
                     `remove_buff` effect ({remover} removes it) — a removal has no \
                     defined meaning on the expected-uptime roll log yet"
                ),
            });
        }
        for (pname, p) in &simdef.procs {
            let applies = |b: &str| {
                p.apply_buff.as_deref() == Some(b)
                    || p.effects
                        .iter()
                        .any(|e| matches!(e, EffectDef::ApplyBuff(x) if x == b))
            };
            if p.icd > 0.0 && applies(name) {
                return Err(PlanError {
                    what: format!(
                        "buff `{name}`: `ev_uptime: expected` cannot be applied by proc \
                         `{pname}`, which has icd {} > 0 — gated rolls are not \
                         independent, so the product formula would not be exact",
                        p.icd
                    ),
                });
            }
            for a in p
                .cast_action
                .iter()
                .chain(p.effects.iter().filter_map(|e| match e {
                    EffectDef::CastAction(a) => Some(a),
                    _ => None,
                }))
            {
                let free_applies = simdef.actions.get(a).is_some_and(|d| {
                    d.apply_buff.iter().any(|b| b == name)
                        || d.effects
                            .iter()
                            .any(|e| matches!(e, EffectDef::ApplyBuff(x) if x == name))
                });
                if free_applies {
                    return Err(PlanError {
                        what: format!(
                            "buff `{name}`: `ev_uptime: expected` cannot be applied by a \
                             free cast: proc `{pname}` free-casts action `{a}`, which \
                             applies it — apply it from the proc directly"
                        ),
                    });
                }
            }
        }
    }

    // Every cross-reference is validated — build the extended sim symbol
    // table and compile every expression.
    let syms = SimSymbols::new(plan, simdef);

    let resource_index = |name: &str| -> usize {
        simdef
            .resources
            .keys()
            .position(|r| r == name)
            .expect("resource reference validated above")
    };
    let action_index = |name: &str| -> usize {
        simdef
            .actions
            .keys()
            .position(|a| a == name)
            .expect("action reference validated above")
    };
    let buff_index = |name: &str| -> usize {
        simdef
            .buffs
            .keys()
            .position(|b| b == name)
            .expect("buff reference validated above")
    };

    let mut resources = Vec::new();
    for (name, r) in &simdef.resources {
        let max = compile_sim(
            &r.max,
            &syms,
            || format!("resource `{name}` max"),
            ObjectiveReads::RefoldTime,
        )?
        .program;
        let regen_per_sec = compile_sim(
            &r.regen_per_sec,
            &syms,
            || format!("resource `{name}` regen_per_sec"),
            ObjectiveReads::RefoldTime,
        )?
        .program;
        resources.push(CompiledResource {
            name: name.clone(),
            max,
            regen_per_sec,
        });
    }

    let mut actions = Vec::new();
    for (name, a) in &simdef.actions {
        let cast_time = match &a.cast_time {
            Some(ct) => Some(compile_sim(
                ct,
                &syms,
                || format!("action `{name}` cast_time"),
                ObjectiveReads::Legal,
            )?),
            None => None,
        };
        let action_measure = a.measure.unwrap_or(simdef.defaults.measure);
        let mut segments = Vec::new();
        for (i, seg) in a.segments.iter().enumerate() {
            let duration = compile_value(&seg.duration, &syms, || {
                format!("action `{name}` segment {i} duration")
            })?;
            let damage = match &seg.damage {
                Some(d) => Some(compile_value_map(
                    &d.stats,
                    &syms,
                    |k| format!("action `{name}` segment {i} damage.stats `{k}`"),
                    ObjectiveReads::Circular,
                )?),
                None => None,
            };
            let effects = seg
                .effects
                .iter()
                .map(|e| match e {
                    EffectDef::ApplyBuff(b) => CompiledEffect::ApplyBuff(buff_index(b)),
                    EffectDef::RemoveBuff(b) => CompiledEffect::RemoveBuff(buff_index(b)),
                    EffectDef::CastAction(_) => {
                        unreachable!("`cast_action` on a segment rejected above")
                    }
                })
                .collect();
            segments.push(CompiledSegment {
                duration,
                damage,
                effects,
                measure: seg.measure.unwrap_or(action_measure),
            });
        }
        let cooldown = compile_value(&a.cooldown, &syms, || format!("action `{name}` cooldown"))?;
        let gcd = match a.gcd.as_ref().or(simdef.defaults.gcd.as_ref()) {
            None => None,
            Some(NumOrExpr::Num(n)) if *n == 0.0 => None,
            Some(v) => Some(compile_value(v, &syms, || format!("action `{name}` gcd"))?),
        };
        let mut cost = Vec::new();
        for (r, v) in &a.cost {
            let v = compile_value(v, &syms, || format!("action `{name}` cost `{r}`"))?;
            cost.push((resource_index(r), v));
        }
        let mut gain = Vec::new();
        for (r, v) in &a.gain {
            let v = compile_value(v, &syms, || format!("action `{name}` gain `{r}`"))?;
            gain.push((resource_index(r), v));
        }
        let damage = match &a.damage {
            Some(d) => Some(compile_value_map(
                &d.stats,
                &syms,
                |k| format!("action `{name}` damage.stats `{k}`"),
                ObjectiveReads::Circular,
            )?),
            None => None,
        };
        // Desugar: each `apply_buff` sugar entry is one ApplyBuff effect,
        // in list order. Mixing the sugar with an explicit list was
        // rejected above, so "prepend the sugar" and "use whichever is
        // set" are the same operation.
        let effects: Vec<CompiledEffect> = if !a.apply_buff.is_empty() {
            a.apply_buff
                .iter()
                .map(|b| CompiledEffect::ApplyBuff(buff_index(b)))
                .collect()
        } else {
            a.effects
                .iter()
                .map(|e| match e {
                    EffectDef::ApplyBuff(b) => CompiledEffect::ApplyBuff(buff_index(b)),
                    EffectDef::RemoveBuff(b) => CompiledEffect::RemoveBuff(buff_index(b)),
                    EffectDef::CastAction(_) => {
                        unreachable!("`cast_action` on an action rejected above")
                    }
                })
                .collect()
        };
        actions.push(CompiledAction {
            name: name.clone(),
            cast_time,
            cooldown,
            gcd,
            cost,
            gain,
            damage,
            effects,
            measure: action_measure,
            crit_rolls: a.crit_rolls.unwrap_or(simdef.defaults.proc_rolls),
            segments,
        });
    }

    let mut buffs = Vec::new();
    for (name, b) in &simdef.buffs {
        let tick_objective = b.tick_objective.as_ref().map(|t| CompiledTick {
            objective: objective_names
                .iter()
                .position(|o| *o == t.objective)
                .expect("tick_objective validated above"),
            snapshot: t.snapshot,
        });
        let duration = compile_value(&b.duration, &syms, || format!("buff `{name}` duration"))?;
        buffs.push(CompiledBuff {
            name: name.clone(),
            duration,
            contributions: b.contributions.clone(),
            conditions: b.conditions.clone(),
            tick_objective,
            max_stacks: b.max_stacks,
            on_reapply: b.on_reapply,
            ev_uptime: b.ev_uptime,
        });
    }

    let mut procs = Vec::new();
    for (name, p) in &simdef.procs {
        let chance = compile_sim(
            &p.chance,
            &syms,
            || format!("proc `{name}` chance"),
            ObjectiveReads::Legal,
        )?;
        // `icd` is a bare literal, not a `NumOrExpr`, so its fail-closed
        // check lands here rather than at an evaluation instant. Both
        // rejected values used to fail SILENTLY, in the worst direction:
        // the executor gates on `now < icd_ready_at`, which is false for
        // every `now` once that deadline is NaN — so `icd: NaN` DELETED
        // the internal cooldown rather than tightening it. A negative one
        // is just "no ICD", spelled confusingly.
        if !p.icd.is_finite() || p.icd < 0.0 {
            return Err(PlanError {
                what: format!("proc `{name}` icd must be finite and >= 0, got {}", p.icd),
            });
        }
        // Desugar: either sugar field is a one-entry effects list (mixing
        // sugar with an explicit list was rejected above, so this is the
        // whole story; the "must do something" check above guarantees the
        // result is non-empty).
        let effects: Vec<CompiledEffect> = if let Some(b) = &p.apply_buff {
            vec![CompiledEffect::ApplyBuff(buff_index(b))]
        } else if let Some(a) = &p.cast_action {
            vec![CompiledEffect::CastAction(action_index(a))]
        } else {
            p.effects
                .iter()
                .map(|e| match e {
                    EffectDef::ApplyBuff(b) => CompiledEffect::ApplyBuff(buff_index(b)),
                    EffectDef::CastAction(a) => CompiledEffect::CastAction(action_index(a)),
                    EffectDef::RemoveBuff(b) => CompiledEffect::RemoveBuff(buff_index(b)),
                })
                .collect()
        };
        let mut expected_buffs: Vec<usize> = effects
            .iter()
            .filter_map(|e| match e {
                CompiledEffect::ApplyBuff(i) if buffs[*i].ev_uptime == EvUptime::Expected => {
                    Some(*i)
                }
                _ => None,
            })
            .collect();
        expected_buffs.sort_unstable();
        expected_buffs.dedup();
        procs.push(CompiledProc {
            name: name.clone(),
            trigger: p.trigger,
            chance,
            icd: p.icd,
            effects,
            actions: p
                .actions
                .as_ref()
                .map(|list| list.iter().map(|a| action_index(a)).collect()),
            rolls: p.rolls.unwrap_or(simdef.defaults.proc_rolls),
            expected_buffs,
        });
    }

    let mut rules = Vec::new();
    for rule in &rotation.rules {
        let when = match &rule.when {
            Some(w) => Some(compile_sim(
                w,
                &syms,
                || format!("rotation rule (action `{}`) when", rule.action),
                ObjectiveReads::Legal,
            )?),
            None => None,
        };
        let interrupt_if = match &rule.interrupt_if {
            Some(w) => Some(compile_sim(
                w,
                &syms,
                || format!("rotation rule (action `{}`) interrupt_if", rule.action),
                ObjectiveReads::Legal,
            )?),
            None => None,
        };
        rules.push(CompiledRule {
            action: action_index(&rule.action),
            when,
            interrupt: rule.interrupt == Some(true) || rule.interrupt_if.is_some(),
            interrupt_if,
        });
    }

    let mut condition_expected = BTreeMap::new();
    for (i, b) in buffs.iter().enumerate() {
        if b.ev_uptime == EvUptime::Expected {
            for c in b.conditions.keys() {
                condition_expected.insert(c.clone(), i);
            }
        }
    }

    let ev_inexact_removal = syms.ev_removal_hit.borrow().clone();
    Ok(SimPlan {
        ev_inexact_removal,
        condition_expected,
        sim_base: syms.sim_base(),
        segments_done_slot: syms.segments_done_slot,
        objective_base: syms.objective_base,
        slot_width: syms.slot_width(),
        resources,
        actions,
        buffs,
        procs,
        rules,
        damage_objective,
        event_order: simdef.defaults.event_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedef::GameDef;
    use crate::plan;
    use crate::simdef::{
        ActionDamage, ActionDef, BuffDef, EffectDef, NumOrExpr, ProcDef, ReapplyPolicy,
        ResourceDef, Rule, TickObjective,
    };
    use std::collections::BTreeMap;

    /// A small toy `Plan` with enough stats/conditions/objectives to
    /// exercise every sim cross-reference: `hit_after_dr` and `dot_dps`
    /// are exported objectives; `hidden_stage` is a pipeline stage that is
    /// deliberately NOT exported, used to prove stages stay invisible to
    /// sim expressions.
    fn toy_plan() -> Plan {
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": ["weapon", "max_mana", "mana_regen", "lucky_hit_chance",
                        "base_aps", "coeff_pct"],
              "conditions": ["vulnerable"],
              "pipeline": [
                { "name": "hit_after_dr", "expr": "weapon" },
                { "name": "dot_dps", "expr": "weapon * 0.1" },
                { "name": "hidden_stage", "expr": "weapon * 2" }
              ],
              "objectives": ["hit_after_dr", "dot_dps"]
            }"#,
        )
        .unwrap();
        plan::compile(&def).unwrap()
    }

    /// A valid, fully-wired `SimDef` mirroring the design spec's example
    /// (same shapes, toy_plan()-compatible names) — the happy-path
    /// fixture every fail-closed test mutates one field of.
    fn valid_simdef() -> SimDef {
        let mut resources = BTreeMap::new();
        resources.insert(
            "mana".to_string(),
            ResourceDef {
                extra: Default::default(),
                max: "max_mana".into(),
                regen_per_sec: "mana_regen".into(),
            },
        );

        let mut actions = BTreeMap::new();
        let mut cost = BTreeMap::new();
        cost.insert("mana".to_string(), NumOrExpr::Num(40.0));
        let mut dmg_stats = BTreeMap::new();
        dmg_stats.insert("coeff_pct".to_string(), NumOrExpr::Num(200.0));
        dmg_stats.insert("hits_per_use".to_string(), NumOrExpr::Num(1.0));
        actions.insert(
            "fireball".to_string(),
            ActionDef {
                gcd: None,
                crit_rolls: None,
                extra: Default::default(),
                measure: None,
                cast_time: Some("1.0 / base_aps".into()),
                segments: Vec::new(),
                cooldown: NumOrExpr::Num(0.0),
                cost,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: dmg_stats,
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        actions.insert(
            "frost_nova".to_string(),
            ActionDef {
                gcd: None,
                crit_rolls: None,
                extra: Default::default(),
                measure: None,
                cast_time: Some("0".into()),
                segments: Vec::new(),
                cooldown: NumOrExpr::Num(10.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );

        let mut buffs = BTreeMap::new();
        let mut vuln_conditions = BTreeMap::new();
        vuln_conditions.insert("vulnerable".to_string(), 1.0);
        buffs.insert(
            "vuln_window".to_string(),
            BuffDef {
                extra: Default::default(),
                ev_uptime: Default::default(),
                duration: NumOrExpr::Num(4.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: vuln_conditions,
                tick_objective: None,
            },
        );
        buffs.insert(
            "combustion".to_string(),
            BuffDef {
                extra: Default::default(),
                ev_uptime: Default::default(),
                duration: NumOrExpr::Num(8.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: vec![Contribution {
                    bucket: "indep".into(),
                    value: 25.0,
                    event: None,
                    condition: None,
                }],
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        buffs.insert(
            "burning".to_string(),
            BuffDef {
                extra: Default::default(),
                ev_uptime: Default::default(),
                duration: NumOrExpr::Num(6.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: Some(TickObjective::live("dot_dps")),
            },
        );

        let mut procs = BTreeMap::new();
        procs.insert(
            "conflagrate".to_string(),
            ProcDef {
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnCrit,
                chance: "lucky_hit_chance / 100 * 0.3".into(),
                icd: 2.0,
                apply_buff: Some("combustion".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
            },
        );

        SimDef {
            extra: Default::default(),
            defaults: Default::default(),
            resources,
            actions,
            buffs,
            procs,
            damage_objective: "hit_after_dr".into(),
        }
    }

    fn valid_rotation() -> Rotation {
        // NB: the design spec's own Rotation prose example writes infix
        // `and` (`"... == 0 and buff... == 0"`), but the actual grammar
        // (P6a) only supports the boolean FUNCTION form `and(a, b)` — the
        // spec's JSON round-trips fine as plain data (see
        // simdef.rs's verbatim test, which only deserializes the string,
        // never compiles it), but a `when` string actually compiled here
        // must use the real grammar, so this fixture uses `and(...)`.
        Rotation {
            extra: Default::default(),
            rules: vec![
                Rule {
                    interrupt: None,
                    interrupt_if: None,
                    extra: Default::default(),
                    action: "frost_nova".into(),
                    when: Some("and(cooldown.frost_nova == 0, buff.vuln_window == 0)".into()),
                },
                Rule {
                    interrupt: None,
                    interrupt_if: None,
                    extra: Default::default(),
                    action: "fireball".into(),
                    when: Some("mana >= 40".into()),
                },
            ],
        }
    }

    #[test]
    fn happy_path_compiles_and_resolves_indices() {
        let plan = toy_plan();
        let sp = compile(&plan, &valid_simdef(), &valid_rotation()).unwrap();

        assert_eq!(sp.resources.len(), 1);
        assert_eq!(sp.resources[0].name, "mana");

        assert_eq!(sp.actions.len(), 2);
        // BTreeMap order: "fireball" < "frost_nova".
        assert_eq!(sp.actions[0].name, "fireball");
        assert_eq!(sp.actions[0].cost.len(), 1);
        assert_eq!(sp.actions[0].cost[0].0, 0); // "mana"
                                                // A literal is pre-baked into a constant — no Program at all.
        assert_eq!(sp.actions[0].cost[0].1, CompiledValue::Const(40.0));
        assert_eq!(sp.actions[1].cooldown, CompiledValue::Const(10.0));
        assert_eq!(sp.buffs[0].duration, CompiledValue::Const(6.0));
        assert!(sp.actions[0].damage.is_some());
        assert_eq!(sp.actions[1].name, "frost_nova");

        assert_eq!(sp.buffs.len(), 3);
        // BTreeMap order: "burning" < "combustion" < "vuln_window".
        assert_eq!(sp.buffs[0].name, "burning");
        assert_eq!(
            sp.buffs[0].tick_objective,
            Some(CompiledTick {
                objective: 1, // "dot_dps"
                snapshot: false,
            })
        );
        assert_eq!(sp.buffs[1].name, "combustion");
        assert_eq!(sp.buffs[2].name, "vuln_window");

        assert_eq!(sp.procs.len(), 1);
        // The sugar desugars to a one-entry compiled effects list.
        assert_eq!(sp.procs[0].effects, vec![CompiledEffect::ApplyBuff(1)]); // "combustion"

        assert_eq!(sp.rules.len(), 2);
        assert_eq!(sp.rules[0].action, 1); // "frost_nova"
        assert_eq!(sp.rules[1].action, 0); // "fireball"

        assert_eq!(sp.damage_objective, 0); // "hit_after_dr"

        assert_eq!(sp.sim_base, plan.own_slot_width());
        // sim segment: time, duration, mana(1), cooldown.*(2),
        // buff.*(3), buff_remaining.*(3), casts.*(2), stacks.*(3)
        //   = 2+1+2+3+3+2+3 = 16  (P7c appended the `stacks.*` sub-range),
        // + segments_done, segments_remain = 18 (slice 4, appended last)
        assert_eq!(sp.slot_width, sp.sim_base + 18 + 2); // + 2 objective slots (slice 6)
    }

    #[test]
    fn unknown_action_in_rotation_rule_is_rejected() {
        let plan = toy_plan();
        let mut rotation = valid_rotation();
        rotation.rules.push(Rule {
            interrupt: None,
            interrupt_if: None,
            extra: Default::default(),
            action: "mystery_action".into(),
            when: None,
        });
        let e = compile(&plan, &valid_simdef(), &rotation).unwrap_err();
        assert!(e.what.contains("mystery_action"), "got: {}", e.what);
    }

    #[test]
    fn proc_referencing_unknown_buff_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().apply_buff = Some("nope".into());
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("nope"), "got: {}", e.what);
    }

    #[test]
    fn proc_referencing_unknown_action_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let p = simdef.procs.get_mut("conflagrate").unwrap();
        p.apply_buff = None;
        p.cast_action = Some("nope".into());
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("nope"), "got: {}", e.what);
    }

    #[test]
    fn proc_with_neither_apply_buff_nor_cast_action_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().apply_buff = None;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("conflagrate"), "got: {}", e.what);
    }

    #[test]
    fn proc_with_both_apply_buff_and_cast_action_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().cast_action = Some("fireball".into());
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("conflagrate"), "got: {}", e.what);
    }

    // ==================================================================
    // P7d — action-scoped effects: `ActionDef::apply_buff` and the
    // `ProcDef::actions` trigger filter. Both are cross-references, so
    // both resolve to indices here and both fail closed on a name that
    // does not exist.
    // ==================================================================

    #[test]
    fn action_apply_buff_referencing_an_unknown_buff_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.actions.get_mut("frost_nova").unwrap().apply_buff =
            vec!["vuln_window".into(), "nope".into()];
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("frost_nova"), "got: {}", e.what);
        assert!(e.what.contains("nope"), "got: {}", e.what);
        assert!(e.what.contains("apply_buff"), "got: {}", e.what);
    }

    #[test]
    fn proc_actions_filter_referencing_an_unknown_action_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().actions =
            Some(vec!["fireball".into(), "nope".into()]);
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("conflagrate"), "got: {}", e.what);
        assert!(e.what.contains("nope"), "got: {}", e.what);
        assert!(e.what.contains("actions"), "got: {}", e.what);
    }

    // `actions: []` describes a proc that can NEVER fire. That is a config
    // mistake, not a supported way to switch one off — and it is exactly
    // the shape that reads like `None` at a glance while meaning the
    // opposite.
    #[test]
    fn proc_with_an_empty_actions_filter_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().actions = Some(Vec::new());
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("conflagrate"), "got: {}", e.what);
        assert!(e.what.contains("empty"), "got: {}", e.what);
    }

    // The positive case: both resolve to INDICES into `SimPlan`'s
    // name-sorted `buffs`/`actions`, keeping the SOURCE list's order —
    // which for `apply_buff` is the application order (see
    // `simdef::ActionDef::apply_buff`).
    #[test]
    fn action_scoping_resolves_to_indices_in_source_order() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        // buffs sort "burning"(0) < "combustion"(1) < "vuln_window"(2);
        // actions sort "fireball"(0) < "frost_nova"(1).
        simdef.actions.get_mut("frost_nova").unwrap().apply_buff =
            vec!["vuln_window".into(), "burning".into()];
        simdef.procs.get_mut("conflagrate").unwrap().actions = Some(vec!["frost_nova".into()]);
        let sp = compile(&plan, &simdef, &valid_rotation()).unwrap();
        assert_eq!(sp.actions[1].name, "frost_nova");
        assert_eq!(
            sp.actions[1].effects,
            vec![CompiledEffect::ApplyBuff(2), CompiledEffect::ApplyBuff(0)],
            "list order is application order, NOT the buffs' sorted order"
        );
        assert!(sp.actions[0].effects.is_empty());
        assert_eq!(sp.procs[0].actions.as_deref(), Some([1].as_slice()));
    }

    #[test]
    fn unknown_resource_in_cost_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef
            .actions
            .get_mut("fireball")
            .unwrap()
            .cost
            .insert("stamina".into(), NumOrExpr::Num(10.0));
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("stamina"), "got: {}", e.what);
    }

    #[test]
    fn unknown_resource_in_gain_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef
            .actions
            .get_mut("fireball")
            .unwrap()
            .gain
            .insert("stamina".into(), NumOrExpr::Num(10.0));
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("stamina"), "got: {}", e.what);
    }

    #[test]
    fn damage_objective_not_a_plan_objective_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.damage_objective = "hidden_stage".into();
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("hidden_stage"), "got: {}", e.what);
    }

    // P7c-T2: `strongest` compares instances by their SNAPSHOT rate, so a
    // buff without one gives it nothing to compare — including a buff
    // whose `tick_objective` is live, which has a rate but not a
    // per-instance one. Both are rejected rather than silently falling
    // back to another policy.
    #[test]
    fn strongest_without_a_snapshot_tick_objective_is_rejected() {
        let plan = toy_plan();
        // No `tick_objective` at all.
        let mut simdef = valid_simdef();
        simdef.buffs.get_mut("combustion").unwrap().on_reapply = ReapplyPolicy::Strongest;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("combustion"), "got: {}", e.what);
        assert!(e.what.contains("strongest"), "got: {}", e.what);
        assert!(e.what.contains("snapshot"), "got: {}", e.what);

        // A LIVE `tick_objective` is not enough either: its rate belongs to
        // the buff, not to an instance, so there is nothing to compare.
        let mut simdef = valid_simdef();
        let b = simdef.buffs.get_mut("burning").unwrap();
        assert_eq!(b.tick_objective, Some(TickObjective::live("dot_dps")));
        b.on_reapply = ReapplyPolicy::Strongest;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("burning"), "got: {}", e.what);
        assert!(e.what.contains("snapshot"), "got: {}", e.what);
    }

    // `strongest` REPLACES the incumbent, so like `refresh` it keeps
    // exactly one instance — a `max_stacks` other than 1 alongside it is a
    // config mistake, not a number to ignore.
    #[test]
    fn strongest_with_a_max_stacks_other_than_one_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let b = simdef.buffs.get_mut("burning").unwrap();
        b.tick_objective = Some(TickObjective::snapshot("dot_dps"));
        b.on_reapply = ReapplyPolicy::Strongest;
        b.max_stacks = 2;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("burning"), "got: {}", e.what);
        assert!(e.what.contains("max_stacks"), "got: {}", e.what);
        // `0` (unbounded) is no more honorable than 2 — and must be
        // rejected by THIS rule, not incidentally by another one.
        simdef.buffs.get_mut("burning").unwrap().max_stacks = 0;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("burning"), "got: {}", e.what);
        assert!(e.what.contains("max_stacks"), "got: {}", e.what);
    }

    // The positive case: with both preconditions met, `strongest` compiles
    // and the snapshot flag reaches the executor.
    #[test]
    fn strongest_with_a_snapshot_tick_objective_compiles() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let b = simdef.buffs.get_mut("burning").unwrap();
        b.tick_objective = Some(TickObjective::snapshot("dot_dps"));
        b.on_reapply = ReapplyPolicy::Strongest;
        let sp = compile(&plan, &simdef, &valid_rotation()).unwrap();
        assert_eq!(sp.buffs[0].name, "burning");
        assert_eq!(sp.buffs[0].on_reapply, ReapplyPolicy::Strongest);
        assert_eq!(
            sp.buffs[0].tick_objective,
            Some(CompiledTick {
                objective: 1, // "dot_dps"
                snapshot: true,
            })
        );
    }

    // `refresh` keeps exactly one instance by construction, so a
    // `max_stacks` other than 1 alongside it is a config mistake — say so
    // rather than accept the number and ignore it.
    #[test]
    fn refresh_with_a_max_stacks_other_than_one_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.buffs.get_mut("combustion").unwrap().max_stacks = 5;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("combustion"), "got: {}", e.what);
        assert!(e.what.contains("max_stacks"), "got: {}", e.what);
        // `0` means unbounded, which `refresh` can honor even less.
        simdef.buffs.get_mut("combustion").unwrap().max_stacks = 0;
        assert!(compile(&plan, &simdef, &valid_rotation()).is_err());
    }

    // A stacking policy takes any cap, INCLUDING 0 (= unbounded).
    #[test]
    fn stacking_policies_accept_a_cap_and_unbounded() {
        let plan = toy_plan();
        for (policy, cap) in [
            (ReapplyPolicy::AddRefreshAll, 3),
            (ReapplyPolicy::AddIndependent, 0),
        ] {
            let mut simdef = valid_simdef();
            let b = simdef.buffs.get_mut("combustion").unwrap();
            b.on_reapply = policy;
            b.max_stacks = cap;
            let sp = compile(&plan, &simdef, &valid_rotation()).unwrap();
            assert_eq!(sp.buffs[1].name, "combustion");
            assert_eq!(sp.buffs[1].max_stacks, cap);
            assert_eq!(sp.buffs[1].on_reapply, policy);
        }
    }

    // `stacks.<buff>` joins the same prefixed sub-space as `buff.<buff>`;
    // an unknown buff behind the prefix stays fail-closed.
    #[test]
    fn stacks_symbol_resolves_for_a_known_buff_only() {
        let plan = toy_plan();
        let mut rotation = valid_rotation();
        rotation.rules[1].when = Some("stacks.combustion >= 2".into());
        assert!(compile(&plan, &valid_simdef(), &rotation).is_ok());

        rotation.rules[1].when = Some("stacks.nonesuch >= 2".into());
        let e = compile(&plan, &valid_simdef(), &rotation).unwrap_err();
        assert!(e.what.contains("stacks.nonesuch"), "got: {}", e.what);
    }

    #[test]
    fn tick_objective_not_a_plan_objective_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.buffs.get_mut("burning").unwrap().tick_objective =
            Some(TickObjective::live("hidden_stage"));
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("hidden_stage"), "got: {}", e.what);
    }

    #[test]
    fn resource_name_colliding_with_a_stat_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let r = simdef.resources.remove("mana").unwrap();
        simdef.resources.insert("weapon".into(), r); // "weapon" is a plan stat
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("weapon"), "got: {}", e.what);
    }

    #[test]
    fn action_name_colliding_with_a_condition_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let a = simdef.actions.remove("frost_nova").unwrap();
        simdef.actions.insert("vulnerable".into(), a); // "vulnerable" is a plan condition
        let mut rotation = valid_rotation();
        rotation.rules[0].action = "vulnerable".into();
        let e = compile(&plan, &simdef, &rotation).unwrap_err();
        assert!(e.what.contains("vulnerable"), "got: {}", e.what);
    }

    #[test]
    fn buff_name_colliding_with_a_stat_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let b = simdef.buffs.remove("combustion").unwrap();
        simdef.buffs.insert("coeff_pct".into(), b); // "coeff_pct" is a plan stat
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("coeff_pct"), "got: {}", e.what);
    }

    #[test]
    fn reserved_time_as_a_resource_name_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let r = simdef.resources.remove("mana").unwrap();
        simdef.resources.insert("time".into(), r);
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("reserved"), "got: {}", e.what);
    }

    #[test]
    fn reserved_duration_as_an_action_name_is_rejected() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        let a = simdef.actions.remove("frost_nova").unwrap();
        simdef.actions.insert("duration".into(), a);
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("reserved"), "got: {}", e.what);
    }

    #[test]
    fn rule_when_referencing_a_pipeline_stage_is_rejected() {
        let plan = toy_plan();
        let mut rotation = valid_rotation();
        // "hidden_stage" is a real pipeline stage but NOT visible to sim
        // expressions (stages/buckets are excluded from the sim symbol
        // space by design) — must fail exactly like any unknown name.
        rotation.rules[1].when = Some("hidden_stage > 0".into());
        let e = compile(&plan, &valid_simdef(), &rotation).unwrap_err();
        assert!(e.what.contains("hidden_stage"), "got: {}", e.what);
    }

    // ==================================================================
    // P8a — unknown config keys fail closed at `sim::compile`, with the
    // entity's registry name as context and a did-you-mean suggestion.
    // The `_` prefix is the documented annotation namespace and stays
    // open at every nesting level.
    // ==================================================================

    // The headline case: `tick_objectiv` (missing final `e`) used to
    // parse and silently mean "no DoT" — exactly the quiet wrong answer
    // the 0.3.0 `TickObjectiveObj` docs called "the bigger hole".
    #[test]
    fn a_typoed_key_on_a_buff_is_rejected_with_a_did_you_mean() {
        let plan = toy_plan();
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": { "poison": { "duration": 8.0, "tick_objectiv": "dot_dps" } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        let e = compile(&plan, &def, &Rotation::default()).unwrap_err();
        assert!(
            e.what.contains("unknown field `tick_objectiv`"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("buff `poison`"), "got: {}", e.what);
        assert!(
            e.what.contains("did you mean `tick_objective`"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn a_typoed_key_on_action_damage_is_rejected_with_its_action_named() {
        let plan = toy_plan();
        let def: SimDef = serde_json::from_str(
            r#"{
              "actions": { "fireball": { "cast_time": "1",
                                         "damage": { "stat": { "coeff_pct": 200.0 } } } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        let e = compile(&plan, &def, &Rotation::default()).unwrap_err();
        assert!(e.what.contains("unknown field `stat`"), "got: {}", e.what);
        assert!(
            e.what.contains("action `fireball` damage"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("did you mean `stats`"), "got: {}", e.what);
    }

    #[test]
    fn a_typoed_key_on_a_proc_is_rejected_with_a_did_you_mean() {
        let plan = toy_plan();
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": { "combustion": { "duration": 8.0 } },
              "procs": { "conflagrate": { "trigger": "on_cast", "chance": "1",
                                          "idc": 2.0, "apply_buff": "combustion" } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        let e = compile(&plan, &def, &Rotation::default()).unwrap_err();
        assert!(e.what.contains("unknown field `idc`"), "got: {}", e.what);
        assert!(e.what.contains("proc `conflagrate`"), "got: {}", e.what);
        assert!(e.what.contains("did you mean `icd`"), "got: {}", e.what);
    }

    #[test]
    fn a_typoed_crit_rolls_key_on_an_action_is_rejected_with_a_did_you_mean() {
        let plan = toy_plan();
        let def: SimDef = serde_json::from_str(
            r#"{
              "actions": { "fireball": { "cast_time": "1", "crit_roll": "per_hit" } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        let e = compile(&plan, &def, &Rotation::default()).unwrap_err();
        assert!(
            e.what.contains("unknown field `crit_roll`"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("action `fireball`"), "got: {}", e.what);
        assert!(
            e.what.contains("did you mean `crit_rolls`"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn a_typoed_key_on_a_rotation_rule_is_rejected_with_a_did_you_mean() {
        let plan = toy_plan();
        let rotation: Rotation = serde_json::from_str(
            r#"{ "rules": [ { "action": "fireball", "whn": "mana >= 40" } ] }"#,
        )
        .unwrap();
        let e = compile(&plan, &valid_simdef(), &rotation).unwrap_err();
        assert!(e.what.contains("unknown field `whn`"), "got: {}", e.what);
        assert!(
            e.what.contains("rotation rule 0 (action `fireball`)"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("did you mean `when`"), "got: {}", e.what);
    }

    // `crit_rolls` resolution: `action.crit_rolls ?? defaults.proc_rolls`.
    // Four cells, one compile each; the `proc_rolls`-absent cells and the
    // `crit_rolls`-present cells are what tell the operands apart.
    #[test]
    fn crit_rolls_inherits_defaults_proc_rolls_and_the_action_override_wins() {
        let plan = toy_plan();
        let resolved = |defaults: &str, crit_rolls: &str| {
            let raw = format!(
                r#"{{
                  "defaults": {{ {defaults} }},
                  "actions": {{ "fireball": {{ "cast_time": "1", {crit_rolls}
                      "damage": {{ "stats": {{ "coeff_pct": 200.0 }} }} }} }},
                  "damage_objective": "hit_after_dr"
                }}"#
            );
            let def: SimDef = serde_json::from_str(&raw).unwrap();
            let sp = compile(&plan, &def, &Rotation::default()).unwrap();
            sp.actions[0].crit_rolls
        };
        let per_hit_default = r#""proc_rolls": "per_hit""#;
        assert_eq!(resolved("", ""), ProcRolls::PerCast);
        assert_eq!(resolved(per_hit_default, ""), ProcRolls::PerHit);
        assert_eq!(
            resolved(per_hit_default, r#""crit_rolls": "per_cast","#),
            ProcRolls::PerCast
        );
        assert_eq!(
            resolved("", r#""crit_rolls": "per_hit","#),
            ProcRolls::PerHit
        );
    }

    #[test]
    fn crit_rolls_parses_and_is_omitted_when_absent() {
        let with: ActionDef =
            serde_json::from_str(r#"{ "cast_time": "1", "crit_rolls": "per_hit" }"#).unwrap();
        assert_eq!(with.crit_rolls, Some(ProcRolls::PerHit));
        assert!(serde_json::to_string(&with)
            .unwrap()
            .contains(r#""crit_rolls":"per_hit""#));
        let without: ActionDef = serde_json::from_str(r#"{ "cast_time": "1" }"#).unwrap();
        assert_eq!(without.crit_rolls, None);
        assert!(!serde_json::to_string(&without)
            .unwrap()
            .contains("crit_rolls"));
    }

    // The `_` namespace on the SimDef side: annotations at the top level
    // and on every nested entity parse, compile, AND survive a serde
    // round-trip (they re-serialize; only non-`_` unknowns are rejected).
    #[test]
    fn underscore_annotations_on_simdef_structs_compile_and_survive_round_trips() {
        let plan = toy_plan();
        let raw = r#"{
          "_source": "hand-written for the P8a namespace pin",
          "resources": { "mana": { "max": "max_mana", "regen_per_sec": "mana_regen",
                                   "_unit": "points" } },
          "actions": { "fireball": { "cast_time": "1", "_rank": "5/5",
                                     "damage": { "stats": { "coeff_pct": 200.0 },
                                                 "_coeff_src": "tooltip" } } },
          "buffs": { "combustion": { "duration": 8.0, "_icon": "flame" } },
          "procs": { "conflagrate": { "trigger": "on_cast", "chance": "1",
                                      "apply_buff": "combustion", "_src": "aspect" } },
          "damage_objective": "hit_after_dr"
        }"#;
        let def: SimDef = serde_json::from_str(raw).unwrap();
        let rotation: Rotation = serde_json::from_str(
            r#"{ "_style": "single button", "rules": [
                  { "action": "fireball", "_prio": "why not", "when": "mana >= 40" } ] }"#,
        )
        .unwrap();
        compile(&plan, &def, &rotation).unwrap();

        let json = serde_json::to_string(&def).unwrap();
        for key in ["_source", "_unit", "_rank", "_coeff_src", "_icon", "_src"] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "`{key}` must survive the round-trip: {json}"
            );
        }
        let json = serde_json::to_string(&rotation).unwrap();
        for key in ["_style", "_prio"] {
            assert!(
                json.contains(&format!("\"{key}\"")),
                "`{key}` must survive the round-trip: {json}"
            );
        }
    }

    // ==================================================================
    // P8a validation debt: `BuffDef::contributions` is the sim half of
    // the shared `Contribution` type — a non-finite value used to fold
    // into every capture and come back as `Ok(NaN)`. The `Plan` half is
    // pinned in `plan.rs`; this is the `sim::compile` half.
    // ==================================================================
    #[test]
    fn a_non_finite_buff_contribution_value_is_a_compile_error() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.buffs.get_mut("combustion").unwrap().contributions[0].value = f64::NAN;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("buff `combustion`"), "got: {}", e.what);
        assert!(
            e.what
                .contains("contribution value into bucket `indep` must be finite"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("NaN"), "got: {}", e.what);

        let mut simdef = valid_simdef();
        simdef.buffs.get_mut("combustion").unwrap().contributions[0].value = f64::NEG_INFINITY;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("must be finite"), "got: {}", e.what);
    }

    // ==================================================================
    // P8b — the effects list at `sim::compile`: the deprecated sugar
    // fields desugar into it, five shapes fail closed, and the compiled
    // representation is IDENTICAL whichever spelling the config used.
    // ==================================================================

    // The compat proof at the compiled layer: a sugar config and its
    // explicit-`effects` spelling produce byte-for-byte the same compiled
    // effect lists — so the executor cannot tell which spelling a config
    // used, and every 0.3.0 sugar config keeps its exact behavior.
    #[test]
    fn sugar_and_explicit_effects_compile_to_identical_effect_lists() {
        let plan = toy_plan();

        // Sugar spelling: `valid_simdef` as committed — proc `conflagrate`
        // uses `apply_buff: Some("combustion")`, and we give `frost_nova`
        // the P7d two-entry action sugar.
        let mut sugar = valid_simdef();
        sugar.actions.get_mut("frost_nova").unwrap().apply_buff =
            vec!["vuln_window".into(), "burning".into()];

        // Explicit spelling: the same config with each sugar field
        // migrated into a one-entry-per-application `effects` list.
        let mut explicit = valid_simdef();
        {
            let p = explicit.procs.get_mut("conflagrate").unwrap();
            p.apply_buff = None;
            p.effects = vec![EffectDef::ApplyBuff("combustion".into())];
        }
        {
            let a = explicit.actions.get_mut("frost_nova").unwrap();
            a.effects = vec![
                EffectDef::ApplyBuff("vuln_window".into()),
                EffectDef::ApplyBuff("burning".into()),
            ];
        }

        let sp_sugar = compile(&plan, &sugar, &valid_rotation()).unwrap();
        let sp_explicit = compile(&plan, &explicit, &valid_rotation()).unwrap();

        // Not vacuous: assert the CONTENT once, then the equality.
        // Buffs sort "burning"(0) < "combustion"(1) < "vuln_window"(2).
        assert_eq!(
            sp_sugar.procs[0].effects,
            vec![CompiledEffect::ApplyBuff(1)]
        );
        assert_eq!(
            sp_sugar.actions[1].effects,
            vec![CompiledEffect::ApplyBuff(2), CompiledEffect::ApplyBuff(0)]
        );
        assert_eq!(sp_sugar.procs[0].effects, sp_explicit.procs[0].effects);
        assert_eq!(sp_sugar.actions[1].effects, sp_explicit.actions[1].effects);

        // A `cast_action` sugar proc desugars the same way.
        let mut sugar_cast = valid_simdef();
        {
            let p = sugar_cast.procs.get_mut("conflagrate").unwrap();
            p.apply_buff = None;
            p.cast_action = Some("frost_nova".into());
        }
        let mut explicit_cast = valid_simdef();
        {
            let p = explicit_cast.procs.get_mut("conflagrate").unwrap();
            p.apply_buff = None;
            p.effects = vec![EffectDef::CastAction("frost_nova".into())];
        }
        let a = compile(&plan, &sugar_cast, &valid_rotation()).unwrap();
        let b = compile(&plan, &explicit_cast, &valid_rotation()).unwrap();
        assert_eq!(a.procs[0].effects, vec![CompiledEffect::CastAction(1)]);
        assert_eq!(a.procs[0].effects, b.procs[0].effects);
    }

    // Fail-closed 1a: sugar + an explicit `effects` list on one PROC.
    // Desugaring would have to pick where the sugar lands relative to the
    // list — ambiguous, so it is refused with the migration named.
    #[test]
    fn proc_sugar_plus_an_explicit_effects_list_is_an_ambiguous_order_error() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().effects =
            vec![EffectDef::ApplyBuff("combustion".into())];
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("proc `conflagrate`"), "got: {}", e.what);
        assert!(e.what.contains("ambiguous order"), "got: {}", e.what);
        assert!(
            e.what.contains("migrate the sugar into the `effects` list"),
            "got: {}",
            e.what
        );
    }

    // Fail-closed 1b: the same on an ACTION.
    #[test]
    fn action_sugar_plus_an_explicit_effects_list_is_an_ambiguous_order_error() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        {
            let a = simdef.actions.get_mut("frost_nova").unwrap();
            a.apply_buff = vec!["vuln_window".into()];
            a.effects = vec![EffectDef::ApplyBuff("burning".into())];
        }
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("action `frost_nova`"), "got: {}", e.what);
        assert!(e.what.contains("ambiguous order"), "got: {}", e.what);
        assert!(
            e.what.contains("migrate the sugar into the `effects` list"),
            "got: {}",
            e.what
        );
    }

    // Fail-closed 2: a proc with NO effects after desugar — the old
    // "exactly one of apply_buff/cast_action, got neither" generalizes to
    // this (and both sugar fields at once stays an error, pinned by the
    // long-standing `proc_with_both_apply_buff_and_cast_action_is_rejected`).
    #[test]
    fn a_proc_with_no_effects_after_desugar_must_do_something() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.procs.get_mut("conflagrate").unwrap().apply_buff = None;
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("proc `conflagrate`"), "got: {}", e.what);
        assert!(
            e.what.contains("a proc must do something"),
            "got: {}",
            e.what
        );
    }

    // Fail-closed 3 + 4: unknown names INSIDE an effects list, each
    // positioned on its entity and naming the list.
    #[test]
    fn unknown_names_inside_an_effects_list_are_rejected() {
        let plan = toy_plan();
        // 3a: unknown buff in a proc's list.
        let mut simdef = valid_simdef();
        {
            let p = simdef.procs.get_mut("conflagrate").unwrap();
            p.apply_buff = None;
            p.effects = vec![EffectDef::ApplyBuff("nope".into())];
        }
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("proc `conflagrate`"), "got: {}", e.what);
        assert!(
            e.what.contains("unknown buff `nope` in effects"),
            "got: {}",
            e.what
        );
        // 4: unknown action in a proc's list.
        let mut simdef = valid_simdef();
        {
            let p = simdef.procs.get_mut("conflagrate").unwrap();
            p.apply_buff = None;
            p.effects = vec![EffectDef::CastAction("nope".into())];
        }
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("proc `conflagrate`"), "got: {}", e.what);
        assert!(
            e.what.contains("unknown action `nope` in effects"),
            "got: {}",
            e.what
        );
        // 3b: unknown buff in an ACTION's list.
        let mut simdef = valid_simdef();
        simdef.actions.get_mut("frost_nova").unwrap().effects =
            vec![EffectDef::ApplyBuff("nope".into())];
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("action `frost_nova`"), "got: {}", e.what);
        assert!(
            e.what.contains("unknown buff `nope` in effects"),
            "got: {}",
            e.what
        );
    }

    // Fail-closed 5: `cast_action` stays PROC-only. On an action it is
    // refused with the recursion rationale spelled out — an action
    // free-casting an action reopens the A→B→A recursion the free-cast
    // guard closed, and a bounded-depth chain design should be chosen by
    // a config that needs one (ROADMAP), not guessed at here.
    #[test]
    fn a_cast_action_effect_on_an_action_is_rejected_with_the_recursion_rationale() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.actions.get_mut("frost_nova").unwrap().effects =
            vec![EffectDef::CastAction("fireball".into())];
        let e = compile(&plan, &simdef, &valid_rotation()).unwrap_err();
        assert!(e.what.contains("action `frost_nova`"), "got: {}", e.what);
        assert!(e.what.contains("recursion"), "got: {}", e.what);
        assert!(e.what.contains("A→B→A"), "got: {}", e.what);
        assert!(e.what.contains("ROADMAP"), "got: {}", e.what);
    }

    #[test]
    fn rule_with_no_when_is_always_eligible_and_compiles() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.actions.insert(
            "basic_bolt".to_string(),
            ActionDef {
                gcd: None,
                crit_rolls: None,
                extra: Default::default(),
                measure: None,
                cast_time: Some("1".into()),
                segments: Vec::new(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        let mut rotation = valid_rotation();
        rotation.rules.push(Rule {
            interrupt: None,
            interrupt_if: None,
            extra: Default::default(),
            action: "basic_bolt".into(),
            when: None,
        });
        let sp = compile(&plan, &simdef, &rotation).unwrap();
        assert!(sp.rules.last().unwrap().when.is_none());
    }

    // ==================================================================
    // Slice 4 (#33) — segments: config surface and compile validation.
    // ==================================================================

    fn segment_compile(raw: &str) -> Result<SimPlan, PlanError> {
        let def: SimDef = serde_json::from_str(raw).unwrap();
        compile(&toy_plan(), &def, &Rotation::default())
    }

    #[test]
    fn a_segmented_action_compiles_with_resolved_segment_measures() {
        // defaults.measure cast_start: seg 0 (no override, no action
        // measure) inherits it; seg 1's override wins.
        let sp = segment_compile(
            r#"{
              "defaults": { "measure": "cast_start" },
              "actions": { "slam": { "segments": [
                  { "duration": 0.5, "damage": { "stats": { "coeff_pct": 100.0 } } },
                  { "duration": "0.25", "measure": "cast_complete",
                    "damage": { "stats": { "coeff_pct": 300.0 } } }
              ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        let a = &sp.actions[0];
        assert!(a.cast_time.is_none(), "segmented action has no cast_time");
        assert!(a.damage.is_none());
        assert_eq!(a.segments.len(), 2);
        assert_eq!(a.segments[0].measure, Measure::CastStart);
        assert_eq!(a.segments[1].measure, Measure::CastComplete);
        assert!(a.segments[0].damage.is_some());

        // Action-level measure beats the default; segment override beats both.
        let sp = segment_compile(
            r#"{
              "actions": { "slam": { "measure": "cast_start", "segments": [
                  { "duration": 0.5 },
                  { "duration": 0.5, "measure": "cast_complete" }
              ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        let a = &sp.actions[0];
        assert_eq!(a.segments[0].measure, Measure::CastStart);
        assert_eq!(a.segments[1].measure, Measure::CastComplete);
        assert!(
            a.segments[0].damage.is_none(),
            "no damage = a wind-up segment"
        );
    }

    // ------------------------------------------------------------------
    // Slice 5 — `interrupt` / `interrupt_if`: config and compile checks.
    // ------------------------------------------------------------------

    const CANCEL_SIMDEF: &str = r#"{
      "actions": {
        "slam": { "segments": [ { "duration": 0.5 }, { "duration": 0.5 } ] },
        "one":  { "segments": [ { "duration": 0.5 } ] },
        "jab":  { "cast_time": "0.5" }
      },
      "damage_objective": "hit_after_dr"
    }"#;

    fn cancel_compile(rules: &str) -> Result<SimPlan, PlanError> {
        let def: SimDef = serde_json::from_str(CANCEL_SIMDEF).unwrap();
        let rot: Rotation = serde_json::from_str(&format!(r#"{{ "rules": {rules} }}"#)).unwrap();
        compile(&toy_plan(), &def, &rot)
    }

    #[test]
    fn rule_interrupt_parses_and_defaults() {
        let sp = cancel_compile(r#"[ { "action": "slam" } ]"#).unwrap();
        assert!(!sp.rules[0].interrupt);
        assert!(sp.rules[0].interrupt_if.is_none());

        let sp = cancel_compile(r#"[ { "action": "slam", "interrupt": true } ]"#).unwrap();
        assert!(sp.rules[0].interrupt);
        assert!(sp.rules[0].interrupt_if.is_none());

        // interrupt_if alone implies interrupt.
        let sp =
            cancel_compile(r#"[ { "action": "slam", "interrupt_if": "segments_done >= 1" } ]"#)
                .unwrap();
        assert!(sp.rules[0].interrupt);
        assert!(sp.rules[0].interrupt_if.is_some());

        // Explicit false alone is the default.
        let sp = cancel_compile(r#"[ { "action": "slam", "interrupt": false } ]"#).unwrap();
        assert!(!sp.rules[0].interrupt);

        // Round-trips through serde.
        let rot: Rotation =
            serde_json::from_str(r#"{ "rules": [ { "action": "slam", "interrupt": true } ] }"#)
                .unwrap();
        let back: Rotation = serde_json::from_str(&serde_json::to_string(&rot).unwrap()).unwrap();
        assert_eq!(back.rules[0].interrupt, Some(true));
    }

    #[test]
    fn rule_without_interrupt_fields_serializes_without_them() {
        let rot: Rotation =
            serde_json::from_str(r#"{ "rules": [ { "action": "slam" } ] }"#).unwrap();
        let out = serde_json::to_string(&rot).unwrap();
        assert!(!out.contains("interrupt"), "got: {out}");
    }

    #[test]
    fn rule_misspelled_interrupt_gets_a_did_you_mean() {
        let e = cancel_compile(r#"[ { "action": "slam", "interupt": true } ]"#).unwrap_err();
        assert!(
            e.what.contains("did you mean `interrupt`"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn rule_interrupt_false_with_interrupt_if_is_an_error() {
        let e = cancel_compile(
            r#"[ { "action": "jab" },
                 { "action": "slam", "interrupt": false, "interrupt_if": "1" } ]"#,
        )
        .unwrap_err();
        assert!(e.what.contains("rotation rule 1"), "got: {}", e.what);
        assert!(e.what.contains("action `slam`"), "got: {}", e.what);
        assert!(e.what.contains("interrupt"), "got: {}", e.what);
    }

    #[test]
    fn rule_interrupt_on_an_unsegmented_action_is_an_error() {
        let e = cancel_compile(r#"[ { "action": "jab", "interrupt": true } ]"#).unwrap_err();
        assert!(e.what.contains("rotation rule 0"), "got: {}", e.what);
        assert!(e.what.contains("action `jab`"), "got: {}", e.what);
        assert!(e.what.contains("at least 2 segments"), "got: {}", e.what);

        // interrupt_if alone implies interrupt, so it is refused too.
        let e = cancel_compile(r#"[ { "action": "jab", "interrupt_if": "1" } ]"#).unwrap_err();
        assert!(e.what.contains("at least 2 segments"), "got: {}", e.what);

        // And a 1-segment action has no non-last boundary either.
        let e = cancel_compile(r#"[ { "action": "one", "interrupt": true } ]"#).unwrap_err();
        assert!(e.what.contains("rotation rule 0"), "got: {}", e.what);
        assert!(e.what.contains("action `one`"), "got: {}", e.what);
        assert!(e.what.contains("at least 2 segments"), "got: {}", e.what);
    }

    #[test]
    fn rule_interrupt_if_reads_objectives_legally() {
        let sp = cancel_compile(
            r#"[ { "action": "slam", "interrupt_if": "objective.hit_after_dr > 0" } ]"#,
        );
        assert!(sp.is_ok(), "got: {:?}", sp.err().map(|e| e.what));
    }

    #[test]
    fn rule_interrupt_if_unknown_symbol_is_positioned() {
        let e = cancel_compile(r#"[ { "action": "slam", "interrupt_if": "nonesuch > 0" } ]"#)
            .unwrap_err();
        assert!(
            e.what
                .contains("rotation rule (action `slam`) interrupt_if"),
            "got: {}",
            e.what
        );
    }

    // `interrupt_if` is a sim expression like `when`, so an
    // `ev_uptime: expected` buff's words are forbidden in it too; the
    // error names the field. Contrast: the same read on a discrete buff
    // compiles.
    #[test]
    fn rule_interrupt_if_rejects_an_expected_buffs_words() {
        let compile_with = |ev_uptime: &str| {
            let def: SimDef = serde_json::from_str(&CANCEL_SIMDEF.replacen(
                r#""damage_objective""#,
                &format!(
                    r#""buffs": {{ "x": {{ "duration": 1 {ev_uptime} }} }}, "damage_objective""#
                ),
                1,
            ))
            .unwrap();
            let rot: Rotation = serde_json::from_str(
                r#"{ "rules": [ { "action": "slam", "interrupt_if": "buff.x" } ] }"#,
            )
            .unwrap();
            compile(&toy_plan(), &def, &rot)
        };
        let e = compile_with(r#", "ev_uptime": "expected""#).unwrap_err();
        assert_names(
            &e.what,
            &[
                "rotation rule (action `slam`) interrupt_if",
                "buff.x",
                "buff `x`",
                "ev_uptime: expected",
            ],
        );
        assert!(compile_with("").is_ok());
    }

    #[test]
    fn segments_and_cast_time_together_is_a_compile_error() {
        let e = segment_compile(
            r#"{
              "actions": { "slam": { "cast_time": "1", "segments": [ { "duration": 1 } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(e.what.contains("action `slam`"), "got: {}", e.what);
        assert!(e.what.contains("segments"), "got: {}", e.what);
        assert!(e.what.contains("cast_time"), "got: {}", e.what);
    }

    #[test]
    fn neither_segments_nor_cast_time_is_a_compile_error() {
        let e = segment_compile(
            r#"{
              "actions": { "slam": { "cooldown": 1.0 } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(e.what.contains("action `slam`"), "got: {}", e.what);
        assert!(e.what.contains("segments"), "got: {}", e.what);
        assert!(e.what.contains("cast_time"), "got: {}", e.what);
    }

    #[test]
    fn segments_with_action_level_damage_is_a_compile_error() {
        let e = segment_compile(
            r#"{
              "actions": { "slam": {
                  "damage": { "stats": { "coeff_pct": 100.0 } },
                  "segments": [ { "duration": 1 } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(e.what.contains("action `slam`"), "got: {}", e.what);
        assert!(e.what.contains("segments"), "got: {}", e.what);
        assert!(e.what.contains("damage"), "got: {}", e.what);
    }

    #[test]
    fn free_casting_a_segmented_action_is_a_compile_error() {
        let e = segment_compile(
            r#"{
              "actions": { "slam": { "segments": [ { "duration": 1 } ] },
                           "bolt": { "cast_time": "1" } },
              "procs": { "echo": { "trigger": "on_hit", "chance": "1",
                                   "effects": [ { "cast_action": "slam" } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(e.what.contains("proc `echo`"), "got: {}", e.what);
        assert!(e.what.contains("slam"), "got: {}", e.what);
        assert!(e.what.contains("segment"), "got: {}", e.what);
    }

    #[test]
    fn a_cast_action_effect_inside_a_segment_is_a_compile_error() {
        let e = segment_compile(
            r#"{
              "actions": { "bolt": { "cast_time": "1" },
                           "slam": { "segments": [
                  { "duration": 1 },
                  { "duration": 1, "effects": [ { "cast_action": "bolt" } ] } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(e.what.contains("action `slam`"), "got: {}", e.what);
        assert!(e.what.contains("segment 1"), "got: {}", e.what);
        assert!(e.what.contains("cast_action"), "got: {}", e.what);
    }

    #[test]
    fn an_unknown_segment_key_fails_with_did_you_mean() {
        let e = segment_compile(
            r#"{
              "actions": { "slam": { "segments": [ { "duration": 1, "duraton": 1 } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(
            e.what.contains("unknown field `duraton`"),
            "got: {}",
            e.what
        );
        assert!(
            e.what.contains("action `slam` segment 0"),
            "got: {}",
            e.what
        );
        assert!(
            e.what.contains("did you mean `duration`"),
            "got: {}",
            e.what
        );

        // The `_` namespace stays open on a segment, and on its damage.
        segment_compile(
            r#"{
              "actions": { "slam": { "segments": [
                  { "duration": 1, "_note": "wind-up" } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn a_segment_damage_block_rejects_unknown_keys_and_accepts_annotations() {
        let e = segment_compile(
            r#"{
              "actions": { "slam": { "segments": [
                  { "duration": 1, "damage": { "stat": { "coeff_pct": 100.0 } } } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(e.what.contains("unknown field `stat`"), "got: {}", e.what);
        assert!(e.what.contains("did you mean `stats`"), "got: {}", e.what);

        segment_compile(
            r#"{
              "actions": { "slam": { "segments": [
                  { "duration": 1, "damage": { "_note": "x",
                                               "stats": { "coeff_pct": 100.0 } } } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn segments_done_is_reserved() {
        for word in ["segments_done", "segments_remain"] {
            let raw = format!(
                r#"{{ "resources": {{ "{word}": {{ "max": "1", "regen_per_sec": "0" }} }},
                    "damage_objective": "hit_after_dr" }}"#
            );
            let e = segment_compile(&raw).unwrap_err();
            assert!(e.what.contains("reserved"), "{word}: got: {}", e.what);
            assert!(e.what.contains(word), "{word}: got: {}", e.what);
            // action and buff names too, like `time`/`duration`.
            let raw = format!(
                r#"{{ "actions": {{ "{word}": {{ "cast_time": "1" }} }},
                    "damage_objective": "hit_after_dr" }}"#
            );
            assert!(segment_compile(&raw).unwrap_err().what.contains("reserved"));
            let raw = format!(
                r#"{{ "buffs": {{ "{word}": {{ "duration": 1.0 }} }},
                    "damage_objective": "hit_after_dr" }}"#
            );
            assert!(segment_compile(&raw).unwrap_err().what.contains("reserved"));
        }
    }

    #[test]
    fn segment_symbols_resolve_in_sim_expressions_to_two_new_slots() {
        // Both words compile in a `when`, resolve to distinct slots at the
        // END of the sim segment (after stacks.*), and widen it by 2.
        let plan = toy_plan();
        let def: SimDef = serde_json::from_str(
            r#"{ "actions": { "bolt": { "cast_time": "1" } },
                 "damage_objective": "hit_after_dr" }"#,
        )
        .unwrap();
        let rotation: Rotation = serde_json::from_str(
            r#"{ "rules": [ { "action": "bolt",
                  "when": "and(segments_done == 0, segments_remain == 0)" } ] }"#,
        )
        .unwrap();
        let sp = compile(&plan, &def, &rotation).unwrap();
        // time, duration, cooldown.bolt, casts.bolt, + the two new words.
        assert_eq!(sp.slot_width, sp.sim_base + 6 + 2); // + 2 objective slots (slice 6)
    }

    // ==================================================================
    // Slice 6, Task 1: `objective.<name>` reads (compile only)
    // ==================================================================

    /// Slice-6 shared fixture: `hct = 1 / (aps * (1 + fast))`, `hit = dmg`,
    /// objectives `["hit", "hct"]`.
    fn haste_plan() -> Plan {
        let gd: GameDef = serde_json::from_str(
            r#"{ "stats": ["aps", "dmg"], "conditions": ["fast"],
                 "pipeline": [
                   { "name": "hct", "expr": "1 / (aps * (1 + fast))" },
                   { "name": "hit", "expr": "dmg" } ],
                 "objectives": ["hit", "hct"] }"#,
        )
        .unwrap();
        plan::compile(&gd).unwrap()
    }

    fn haste_compile(sim: &str, rot: &str) -> Result<SimPlan, PlanError> {
        let def: SimDef = serde_json::from_str(sim).unwrap();
        let rotation: Rotation = serde_json::from_str(rot).unwrap();
        compile(&haste_plan(), &def, &rotation)
    }

    const NO_RULES: &str = r#"{ "rules": [] }"#;

    fn is_obj_expr(v: &CompiledValue) -> bool {
        matches!(v, CompiledValue::Expr(p) if p.reads_objectives)
    }

    #[test]
    fn objective_reads_compile_in_every_legal_field() {
        let sp = haste_compile(
            r#"{ "resources": { "mana": { "max": "100", "regen_per_sec": "0" } },
                 "actions": {
                   "bolt": { "cast_time": "objective.hct", "cooldown": "objective.hct",
                             "cost": { "mana": "objective.hct" },
                             "gain": { "mana": "objective.hct" } },
                   "plain": { "cast_time": "1 + aps", "cooldown": "aps",
                              "cost": { "mana": "aps" }, "gain": { "mana": "aps" } },
                   "seg": { "segments": [ { "duration": "objective.hct" } ] } },
                 "buffs": { "haste": { "duration": "objective.hct" },
                            "calm": { "duration": "aps" } },
                 "procs": { "p": { "trigger": "on_cast", "chance": "objective.hct",
                                   "apply_buff": "haste" },
                            "q": { "trigger": "on_cast", "chance": "aps / 10",
                                   "apply_buff": "calm" } },
                 "damage_objective": "hit" }"#,
            r#"{ "rules": [ { "action": "bolt", "when": "objective.hct > 0.7" },
                             { "action": "plain", "when": "aps > 0" } ] }"#,
        )
        .unwrap();
        let a = |n: &str| sp.actions.iter().find(|a| a.name == n).unwrap();
        // Every legal field, reading an objective -> true.
        assert!(a("bolt").cast_time.as_ref().unwrap().reads_objectives);
        assert!(is_obj_expr(&a("bolt").cooldown));
        assert!(is_obj_expr(&a("bolt").cost[0].1));
        assert!(is_obj_expr(&a("bolt").gain[0].1));
        assert!(is_obj_expr(&a("seg").segments[0].duration));
        let b = |n: &str| sp.buffs.iter().find(|b| b.name == n).unwrap();
        assert!(is_obj_expr(&b("haste").duration));
        let p = |n: &str| sp.procs.iter().find(|p| p.name == n).unwrap();
        assert!(p("p").chance.reads_objectives);
        assert!(sp.rules[0].when.as_ref().unwrap().reads_objectives);
        // The same fields NOT reading one -> false.
        assert!(!a("plain").cast_time.as_ref().unwrap().reads_objectives);
        assert!(!matches!(&a("plain").cooldown, CompiledValue::Expr(p) if p.reads_objectives));
        assert!(!matches!(&a("plain").cost[0].1, CompiledValue::Expr(p) if p.reads_objectives));
        assert!(!matches!(&a("plain").gain[0].1, CompiledValue::Expr(p) if p.reads_objectives));
        assert!(!matches!(&b("calm").duration, CompiledValue::Expr(p) if p.reads_objectives));
        assert!(!p("q").chance.reads_objectives);
        assert!(!sp.rules[1].when.as_ref().unwrap().reads_objectives);
        // The objective region is appended after the segment words and is
        // as wide as the Plan's objective count (2).
        assert_eq!(sp.slot_width, sp.objective_base + 2);
    }

    #[test]
    fn an_objective_read_in_damage_stats_is_a_compile_error() {
        let e = haste_compile(
            r#"{ "actions": { "bolt": { "cast_time": "1",
                   "damage": { "stats": { "dmg": "objective.hct" } } } },
                 "damage_objective": "hit" }"#,
            NO_RULES,
        )
        .unwrap_err();
        for needle in ["bolt", "damage.stats", "objective", "circular"] {
            assert!(e.what.contains(needle), "{needle}: got: {}", e.what);
        }
        // Only its own reason (Task 2 review route).
        assert!(!e.what.contains("regen_per_sec"), "got: {}", e.what);
        let e = haste_compile(
            r#"{ "actions": { "bolt": { "segments": [ { "duration": 1,
                   "damage": { "stats": { "dmg": "objective.hit" } } } ] } },
                 "damage_objective": "hit" }"#,
            NO_RULES,
        )
        .unwrap_err();
        for needle in ["bolt", "segment 0", "damage.stats", "objective"] {
            assert!(e.what.contains(needle), "{needle}: got: {}", e.what);
        }
    }

    #[test]
    fn an_objective_read_in_hits_per_use_names_its_own_reason() {
        let e = haste_compile(
            r#"{ "actions": { "bolt": { "cast_time": "1",
                   "damage": { "stats": { "hits_per_use": "objective.hit" } } } },
                 "damage_objective": "hit" }"#,
            NO_RULES,
        )
        .unwrap_err();
        for needle in [
            "bolt",
            "hits_per_use",
            "objective",
            "before any Plan evaluation",
        ] {
            assert!(e.what.contains(needle), "{needle}: got: {}", e.what);
        }
        assert!(!e.what.contains("circular"), "got: {}", e.what);
    }

    #[test]
    fn an_objective_read_in_a_resource_is_a_compile_error() {
        for field in ["max", "regen_per_sec"] {
            let (max, regen) = if field == "max" {
                ("objective.hct", "0")
            } else {
                ("100", "objective.hct")
            };
            let raw = format!(
                r#"{{ "resources": {{ "mana": {{ "max": "{max}", "regen_per_sec": "{regen}" }} }},
                     "damage_objective": "hit" }}"#
            );
            let e = haste_compile(&raw, NO_RULES).unwrap_err();
            for needle in ["mana", field, "objective", "folded at every state change"] {
                assert!(e.what.contains(needle), "{field}/{needle}: got: {}", e.what);
            }
            // Only its own reason (Task 2 review route).
            assert!(!e.what.contains("circular"), "{field}: got: {}", e.what);
        }
    }

    #[test]
    fn an_unknown_objective_suggests_the_nearest() {
        let e = haste_compile(
            r#"{ "actions": { "bolt": { "cast_time": "objective.hct_" } },
                 "damage_objective": "hit" }"#,
            NO_RULES,
        )
        .unwrap_err();
        assert!(e.what.contains("hct_"), "got: {}", e.what);
        assert!(e.what.contains("did you mean `hct`"), "got: {}", e.what);
        // Nothing close -> lists the Plan's objectives instead.
        let e = haste_compile(
            r#"{ "actions": { "bolt": { "cast_time": "objective.zzzzzz" } },
                 "damage_objective": "hit" }"#,
            NO_RULES,
        )
        .unwrap_err();
        assert!(
            e.what.contains("`hit`") && e.what.contains("`hct`"),
            "got: {}",
            e.what
        );
    }
    // ---- ev_uptime (#33 item 4, task 1): field + fail-closed constraints ----

    use crate::simdef::EvUptime;

    /// The fixture with `combustion` opted in, legal on every axis: no
    /// ICD on its proc, `vuln_window` (the fixture's only condition
    /// driver) kept, and a rotation that reads no buff words.
    fn ev_simdef() -> SimDef {
        let mut sd = valid_simdef();
        sd.buffs.get_mut("combustion").unwrap().ev_uptime = EvUptime::Expected;
        sd.procs.get_mut("conflagrate").unwrap().icd = 0.0;
        sd
    }

    fn ev_rotation(when: &str) -> Rotation {
        Rotation {
            extra: Default::default(),
            rules: vec![Rule {
                interrupt: None,
                interrupt_if: None,
                extra: Default::default(),
                action: "fireball".into(),
                when: Some(when.into()),
            }],
        }
    }

    fn ev_err(sd: &SimDef, rot: &Rotation) -> String {
        compile(&toy_plan(), sd, rot).unwrap_err().what
    }

    fn assert_names(e: &str, needles: &[&str]) {
        for n in needles {
            assert!(e.contains(n), "expected `{n}` in: {e}");
        }
    }

    #[test]
    fn ev_uptime_parses_and_defaults() {
        let absent: BuffDef = serde_json::from_str(r#"{ "duration": 4 }"#).unwrap();
        assert_eq!(absent.ev_uptime, EvUptime::Discrete);
        let exp: BuffDef =
            serde_json::from_str(r#"{ "duration": 4, "ev_uptime": "expected" }"#).unwrap();
        assert_eq!(exp.ev_uptime, EvUptime::Expected);
        let back = serde_json::to_string(&exp).unwrap();
        assert!(back.contains(r#""ev_uptime":"expected""#), "{back}");
        let again: BuffDef = serde_json::from_str(&back).unwrap();
        assert_eq!(again.ev_uptime, EvUptime::Expected);
        // A typo is a serde error, not a silent Discrete.
        assert!(
            serde_json::from_str::<BuffDef>(r#"{ "duration": 4, "ev_uptime": "expectd" }"#)
                .is_err()
        );
        // And the key is known, so it is not an unknown-key error.
        assert!(BuffDef::KNOWN_KEYS.contains(&"ev_uptime"));
    }

    #[test]
    fn ev_uptime_expected_compiles_for_a_legal_buff() {
        let mut sd = ev_simdef();
        // X drives its own condition; the fixture's other driver moves off it.
        sd.buffs
            .get_mut("combustion")
            .unwrap()
            .conditions
            .insert("vulnerable".into(), 1.0);
        sd.buffs.get_mut("vuln_window").unwrap().conditions.clear();
        // X named twice in one proc's effects.
        let p = sd.procs.get_mut("conflagrate").unwrap();
        p.apply_buff = None;
        p.effects = vec![
            EffectDef::ApplyBuff("combustion".into()),
            EffectDef::ApplyBuff("vuln_window".into()),
            EffectDef::ApplyBuff("combustion".into()),
        ];
        // `objective.<o>` reads stay legal.
        let sp = compile(&toy_plan(), &sd, &ev_rotation("objective.hit_after_dr > 0")).unwrap();
        // BTreeMap order: burning 0, combustion 1, vuln_window 2.
        assert_eq!(sp.buffs[1].ev_uptime, EvUptime::Expected);
        assert_eq!(sp.buffs[0].ev_uptime, EvUptime::Discrete);
        assert_eq!(sp.procs[0].expected_buffs, vec![1]);
        assert_eq!(
            sp.condition_expected,
            BTreeMap::from([("vulnerable".to_string(), 1usize)])
        );
    }

    #[test]
    fn ev_uptime_expected_rejects_stacking() {
        for policy in [ReapplyPolicy::AddIndependent, ReapplyPolicy::AddRefreshAll] {
            let mut sd = ev_simdef();
            let b = sd.buffs.get_mut("combustion").unwrap();
            b.max_stacks = 3;
            b.on_reapply = policy;
            let e = ev_err(&sd, &ev_rotation("mana >= 40"));
            assert_names(
                &e,
                &["buff `combustion`", "ev_uptime", "refresh", "max_stacks"],
            );
        }
    }

    #[test]
    fn ev_uptime_expected_rejects_snapshot_tick() {
        let mut sd = ev_simdef();
        sd.buffs.get_mut("combustion").unwrap().tick_objective =
            Some(TickObjective::snapshot("dot_dps"));
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(&e, &["buff `combustion`", "ev_uptime", "snapshot"]);
        // A live one is fine.
        sd.buffs.get_mut("combustion").unwrap().tick_objective =
            Some(TickObjective::live("dot_dps"));
        compile(&toy_plan(), &sd, &ev_rotation("mana >= 40")).unwrap();
    }

    #[test]
    fn ev_uptime_expected_rejects_icd_proc() {
        let mut sd = ev_simdef();
        sd.procs.get_mut("conflagrate").unwrap().icd = 2.0;
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(
            &e,
            &[
                "buff `combustion`",
                "ev_uptime",
                "proc `conflagrate`",
                "icd",
            ],
        );
    }

    #[test]
    fn ev_uptime_expected_rejects_free_cast_application() {
        let mut sd = ev_simdef();
        sd.actions.get_mut("frost_nova").unwrap().apply_buff = vec!["combustion".into()];
        let mut p = sd.procs["conflagrate"].clone();
        p.apply_buff = None;
        p.cast_action = Some("frost_nova".into());
        sd.procs.insert("freecast".into(), p);
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(
            &e,
            &[
                "buff `combustion`",
                "ev_uptime",
                "proc `freecast`",
                "frost_nova",
                "free cast",
            ],
        );
        // Same through the explicit `effects` list on the action.
        let a = sd.actions.get_mut("frost_nova").unwrap();
        a.apply_buff.clear();
        a.effects = vec![EffectDef::ApplyBuff("combustion".into())];
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(&e, &["buff `combustion`", "frost_nova", "free cast"]);
    }

    #[test]
    fn ev_uptime_expected_rejects_buff_word_reads() {
        for word in [
            "buff.combustion",
            "buff_remaining.combustion",
            "stacks.combustion",
        ] {
            // Rule `when`.
            let sd = ev_simdef();
            let e = ev_err(&sd, &ev_rotation(&format!("{word} > 0")));
            assert_names(
                &e,
                &[
                    "rotation rule",
                    "when",
                    word,
                    "buff `combustion`",
                    "ev_uptime",
                ],
            );
            // Proc `chance`.
            let mut sd = ev_simdef();
            sd.procs.get_mut("conflagrate").unwrap().chance = format!("{word} * 0.1");
            let e = ev_err(&sd, &ev_rotation("mana >= 40"));
            assert_names(
                &e,
                &["proc `conflagrate` chance", word, "buff `combustion`"],
            );
            // Another buff's `duration`.
            let mut sd = ev_simdef();
            sd.buffs.get_mut("burning").unwrap().duration = NumOrExpr::Expr(format!("2 + {word}"));
            let e = ev_err(&sd, &ev_rotation("mana >= 40"));
            assert_names(&e, &["buff `burning` duration", word, "buff `combustion`"]);
        }
        // The buff's OWN duration reading itself is forbidden too.
        let mut sd = ev_simdef();
        sd.buffs.get_mut("combustion").unwrap().duration =
            NumOrExpr::Expr("min(12, buff_remaining.combustion + 8)".into());
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(
            &e,
            &["buff `combustion` duration", "buff_remaining.combustion"],
        );
        // A DISCRETE buff's words stay readable.
        let mut sd = ev_simdef();
        sd.buffs.get_mut("combustion").unwrap().ev_uptime = EvUptime::Discrete;
        compile(&toy_plan(), &sd, &ev_rotation("buff.combustion > 0")).unwrap();
    }

    #[test]
    fn ev_uptime_expected_rejects_driven_condition_reads() {
        // vuln_window drives `vulnerable`.
        let mut sd = ev_simdef();
        sd.buffs.get_mut("vuln_window").unwrap().ev_uptime = EvUptime::Expected;
        let e = ev_err(&sd, &ev_rotation("vulnerable >= 1"));
        assert_names(
            &e,
            &[
                "rotation rule",
                "vulnerable",
                "buff `vuln_window`",
                "ev_uptime",
            ],
        );
        // Also in a proc chance.
        sd.procs.get_mut("conflagrate").unwrap().chance = "vulnerable * 0.5".into();
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(&e, &["proc `conflagrate` chance", "vulnerable"]);
        // `objective.<o>` reads stay legal even though the objective may
        // depend on the condition.
        sd.procs.get_mut("conflagrate").unwrap().chance = "0.3".into();
        compile(&toy_plan(), &sd, &ev_rotation("objective.hit_after_dr > 0")).unwrap();
    }

    #[test]
    fn ev_uptime_expected_rejects_contribution_gated_on_own_condition() {
        // combustion drives `vulnerable` (vuln_window moved off it) and
        // gates its own contribution on it: under EV the gate reads the
        // blend P·1 + (1 − P)·static, so the contribution counts P × blend
        // where MC counts P × 1 — a self-correlation with no exact form.
        let mut sd = ev_simdef();
        sd.buffs.get_mut("vuln_window").unwrap().conditions.clear();
        let x = sd.buffs.get_mut("combustion").unwrap();
        x.conditions.insert("vulnerable".into(), 1.0);
        x.contributions[0].condition = Some("vulnerable".into());
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(
            &e,
            &[
                "buff `combustion`",
                "ev_uptime",
                "bucket `indep`",
                "condition `vulnerable`",
                "drives",
            ],
        );
        // Gating on a condition ANOTHER (discrete) buff drives stays legal.
        let mut sd = ev_simdef();
        sd.buffs.get_mut("combustion").unwrap().contributions[0].condition =
            Some("vulnerable".into());
        compile(&toy_plan(), &sd, &ev_rotation("mana >= 40")).unwrap();
        // And a Discrete buff gating on its own condition is untouched.
        let mut sd = valid_simdef();
        let v = sd.buffs.get_mut("vuln_window").unwrap();
        v.contributions.push(Contribution {
            bucket: "indep".into(),
            value: 10.0,
            event: None,
            condition: Some("vulnerable".into()),
        });
        compile(&toy_plan(), &sd, &ev_rotation("mana >= 40")).unwrap();
    }

    #[test]
    fn ev_uptime_expected_rejects_shared_condition() {
        let mut sd = ev_simdef();
        sd.buffs.get_mut("vuln_window").unwrap().ev_uptime = EvUptime::Expected;
        sd.buffs
            .get_mut("combustion")
            .unwrap()
            .conditions
            .insert("vulnerable".into(), 1.0);
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(
            &e,
            &[
                "vulnerable",
                "vuln_window",
                "combustion",
                "ev_uptime",
                "share",
            ],
        );
        // Sharing with a Discrete buff is equally refused (X is the one
        // that opted in).
        sd.buffs.get_mut("combustion").unwrap().ev_uptime = EvUptime::Discrete;
        let e = ev_err(&sd, &ev_rotation("mana >= 40"));
        assert_names(&e, &["vulnerable", "vuln_window", "combustion", "share"]);
    }
}
