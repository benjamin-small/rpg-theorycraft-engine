//! SimDef — the game's SEQUENCING config: resources, actions, buffs, and
//! procs, plus the per-hit objective the timeline simulator accumulates.
//! Sits BESIDE the `GameDef`/`Plan` — nothing in `plan.rs` changes.
//! `sim::compile` turns a `SimDef` (with a `Rotation`) into a `SimPlan`;
//! see that module for the compiled form and the extended symbol space.

use crate::build::Contribution;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The game's SEQUENCING config, as data: resource/action/buff/proc
/// registries plus the `Plan` objective the sim accumulates per hit.
/// Compiled once (together with a [`Rotation`]) by `sim::compile`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimDef {
    /// Resource registry (mana, spirit, …), by name.
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceDef>,
    /// Action registry (casts the rotation can choose), by name.
    #[serde(default)]
    pub actions: BTreeMap<String, ActionDef>,
    /// Buff/debuff registry (timed contribution/condition windows), by
    /// name.
    #[serde(default)]
    pub buffs: BTreeMap<String, BuffDef>,
    /// Proc registry (chance-triggered effects), by name.
    #[serde(default)]
    pub procs: BTreeMap<String, ProcDef>,
    /// Name of the `Plan` objective the sim accumulates as
    /// `damage_objective × hits` on every completed damaging cast.
    pub damage_objective: String,
}

/// A literal number or an expression string, evaluated at a documented
/// instant.
///
/// Untagged: a JSON number deserializes to [`NumOrExpr::Num`] and a JSON
/// string to [`NumOrExpr::Expr`]. Every rtce 0.2.0 config — which only
/// ever wrote plain numbers in these positions — therefore PARSES
/// unchanged, and a literal reaches the executor as the identical `f64`
/// the old field held: `Num` is pre-baked into a constant at
/// `sim::compile`, so the literal path adds no evaluation and no rounding.
/// (That is a statement about THIS type only. The 0.3.0 release notes
/// carry two deliberate executor behavior FIXES landed alongside it, which
/// can move results for configs whose proc `chance` reads state another
/// proc mutates in the same trigger batch, or which have an `on_hit` proc
/// that changes crit chance — see the CHANGELOG.)
///
/// An `Expr` is parsed at `sim::compile` against the sim symbol space (see
/// the `sim` module docs), with the usual positioned, fail-closed error
/// for an unknown identifier or a syntax problem. Pipeline stages and
/// buckets are NOT in that space — naming one is a compile error, the same
/// as any other unresolved name.
///
/// # Evaluation instants
///
/// An expression is re-evaluated every time its field is USED, at the
/// instant named below — never once up front:
///
/// | Field | Evaluated |
/// |---|---|
/// | [`BuffDef::duration`] | at application (snapshotted onto that window) |
/// | [`ActionDef::cooldown`] | at cast start |
/// | [`ActionDef::cost`] values | at cast start — and at every decision that merely CHECKS affordability |
/// | [`ActionDef::gain`] values | at cast complete |
/// | [`ActionDamage::stats`] values | at cast complete |
///
/// A cost expression is therefore RE-CHECKED at each decision point, never
/// PREDICTED: the executor's resource-affordability wake time is solved
/// from the cost as evaluated at that decision instant, and if the
/// expression's value has changed by the time the wake fires, the wake
/// simply re-decides at the new value (see `sim::exec`'s `afford`).
///
/// Keep cost expressions CHEAP for that reason. A cost is the only one of
/// these fields evaluated on a hot path — once per rule per decision point
/// — where the others are evaluated once per cast or per buff
/// application. A literal costs nothing at all (it is a pre-baked
/// constant), so leave a fixed cost as a number.
///
/// # Fail-closed
///
/// At its evaluation instant a value that is not finite is a run error
/// naming the field and the instant; `duration`/`cooldown`/`cost`/`gain`
/// additionally reject a negative result. [`ActionDamage::stats`] values
/// may legitimately be negative (a stat is not a quantity of anything), so
/// only finiteness is enforced there.
///
/// Deliberately NOT `#[non_exhaustive]`, unlike the compiled
/// [`crate::sim::CompiledValue`]: this is a CONFIG type, "number or
/// expression" is the whole idea, and a caller building or inspecting a
/// `SimDef` in Rust should be able to match it exhaustively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumOrExpr {
    /// Literal value (backward compatible with 0.2.0 configs).
    Num(f64),
    /// Expression over the sim symbol space.
    Expr(String),
}

/// `0.0` — the same default the `f64` fields carried before these became
/// expression-valued, so `#[serde(default)]` on `cooldown`/`cost`/`gain`
/// keeps its 0.2.0 meaning.
impl Default for NumOrExpr {
    fn default() -> Self {
        NumOrExpr::Num(0.0)
    }
}

/// A number is always a literal: `NumOrExpr::from(40.0)` is
/// [`NumOrExpr::Num`], the pre-baked-constant path.
impl From<f64> for NumOrExpr {
    fn from(v: f64) -> Self {
        NumOrExpr::Num(v)
    }
}

/// A string is ALWAYS an expression, even when it looks like a number:
/// `NumOrExpr::from("40")` is [`NumOrExpr::Expr`] — a compiled program
/// that evaluates to 40, not the literal [`NumOrExpr::Num`]`(40.0)`. The
/// two agree on every value they produce (the grammar's numeric literals
/// are `f64`), so this is a representation difference, not a semantic
/// one — but it is the one place this type can be misread, and it does
/// cost a `Program` and an evaluation. Write `40.0.into()` for a
/// constant.
impl From<&str> for NumOrExpr {
    fn from(s: &str) -> Self {
        NumOrExpr::Expr(s.to_string())
    }
}

/// As [`From<&str>`][`NumOrExpr::from`]: a string is always an expression.
impl From<String> for NumOrExpr {
    fn from(s: String) -> Self {
        NumOrExpr::Expr(s)
    }
}

/// One resource (mana, spirit, fury, …): a capped pool that regenerates
/// continuously and is spent/gained by actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDef {
    /// Expression (over stats/conditions) for this resource's cap.
    pub max: String,
    /// Expression (over stats/conditions) for the per-second regen rate.
    pub regen_per_sec: String,
}

/// One action the rotation can cast: timing, resource cost/gain, and an
/// optional damage effect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionDef {
    /// Expression (over stats/conditions/sim-state; never pipeline
    /// stages/buckets) for the cast time in seconds. `"0"` = instant.
    pub cast_time: String,
    /// Cooldown in seconds, starting when the cast begins (`0.0` = none).
    /// Literal or expression; an expression is evaluated AT CAST START
    /// (before the cost is deducted) and must be finite and `>= 0` —
    /// see [`NumOrExpr`].
    #[serde(default)]
    pub cooldown: NumOrExpr,
    /// Resource cost paid when the cast begins, by resource name. Literal
    /// or expression; an expression is evaluated AT CAST START, and also
    /// at every decision point that checks whether this action is
    /// affordable — see [`NumOrExpr`] for why a cost expression is
    /// re-checked rather than predicted.
    #[serde(default)]
    pub cost: BTreeMap<String, NumOrExpr>,
    /// Resource gained when the cast completes, by resource name (e.g.
    /// basic/generator skills). Literal or expression; an expression is
    /// evaluated AT CAST COMPLETE and must be finite and `>= 0` — see
    /// [`NumOrExpr`].
    #[serde(default)]
    pub gain: BTreeMap<String, NumOrExpr>,
    /// This action's damage effect, if any (omit for utility-only casts).
    #[serde(default)]
    pub damage: Option<ActionDamage>,
    /// Buffs this action applies when its cast COMPLETES — one
    /// application per entry, routed through each buff's
    /// [`BuffDef::on_reapply`] policy exactly as a proc application is.
    /// Empty (the default) means this action applies nothing, which is
    /// every rtce 0.2.0 config.
    ///
    /// This is the first-class replacement for the "icd equals the gating
    /// action's cooldown" trick a 0.2.0 config needed in order to coerce
    /// per-action buff application out of a globally-triggered
    /// [`ProcDef`].
    ///
    /// # Where it lands
    ///
    /// AFTER this cast's own damage is measured and credited, and BEFORE
    /// any of this cast's proc rolls — see the [`crate::sim`] module docs
    /// for the full cast-complete order and what each step can see. The
    /// two consequences worth knowing here: the applying cast does NOT
    /// benefit from the buff it applies, and a proc rolled by that cast
    /// DOES see it.
    ///
    /// # Within one list
    ///
    /// Entries are applied in LIST order, and a name repeated in the list
    /// is applied that many times — under `add_independent` that is two
    /// instances; under `refresh` the second application simply replaces
    /// the first.
    ///
    /// Two things a later entry can see, and one it cannot. This
    /// asymmetry is real, deliberate, and the most surprising thing on
    /// this field:
    ///
    /// - A [`BuffDef::duration`] EXPRESSION is SEQUENTIAL. It reads sim
    ///   state, which is refreshed per application, so a later entry sees
    ///   earlier entries' live windows and STACK COUNTS —
    ///   `"2 * (1 + stacks.earlier)"` works and means what it says.
    /// - A SNAPSHOT [`TickObjective`] magnitude is FROZEN. It reads a
    ///   BUILD, and that build is captured ONCE before the list runs, so a
    ///   later entry does NOT see earlier entries' `contributions`. An
    ///   `["empower", "ailment"]` list does not give the ailment
    ///   `empower`'s multiplier; put `empower` on an earlier CAST if that
    ///   is what you want.
    ///
    /// The frozen build is the whole cast's, which makes the two action
    /// paths agree: a DAMAGING action freezes its
    /// [`ActionDamage::stats`] overlay (so an ailment inherits the
    /// magnitude of the hit that applied it — PoE2 semantics), and a
    /// UTILITY action, which runs no damage query, freezes the plain
    /// effective build. The PROC path is deliberately different and
    /// unchanged: a proc-applied buff captures the ambient effective
    /// build, with no action's overlay on it.
    ///
    /// # Elsewhere
    ///
    /// Applied by a proc-triggered FREE cast of this action
    /// ([`ProcDef::cast_action`]) too, under that free cast's own overlay
    /// — `apply_buff` is an effect OF the action, like `gain` and
    /// `damage`, not part of the cast pipeline (cost, cooldown, further
    /// proc rolls) that the free-cast path deliberately skips.
    ///
    /// NB the ARITY differs from [`ProcDef::apply_buff`], which is a
    /// single `Option<String>` rather than a list. Same key, same
    /// concept, different shape: writing `"apply_buff": ["x"]` on a proc
    /// is a serde type error, and `"apply_buff": "x"` on an action is
    /// too. Harmonizing them means accepting both spellings in both
    /// places, which is a config-compatibility change; it is tracked in
    /// ROADMAP for 0.4.0 rather than smuggled in here.
    ///
    /// Fail-closed at `sim::compile`: a name that is not a defined buff.
    #[serde(default)]
    pub apply_buff: Vec<String>,
}

/// Per-cast stat overrides folded onto the `Plan`'s `BuildState` before the
/// `damage_objective` is evaluated for this cast. `hits_per_use` (default
/// `1.0` when absent) is read directly by the executor as the per-cast hit
/// count rather than fed into the `Plan` as a stat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionDamage {
    /// Stat name → override value, applied only while resolving this
    /// action's damage. Literal or expression; an expression is evaluated
    /// AT CAST COMPLETE, ONCE per cast (the same evaluated overlay feeds
    /// the damage query and the `on_crit` proc weight), and need only be
    /// FINITE — a stat may legitimately be negative. `hits_per_use` lives
    /// in this map and follows the same rule. See [`NumOrExpr`].
    ///
    /// The completion instant has internal ORDER (stated in full in the
    /// [`crate::sim`] module docs), and these expressions are evaluated at
    /// a fixed point within it: AFTER this action's [`ActionDef::gain`] is
    /// credited and after its own cast is counted, and BEFORE both this
    /// action's [`ActionDef::apply_buff`] and any of this cast's proc
    /// rolls. So an expression here reads a resource at its POST-gain
    /// amount, and `casts.<this action>` INCLUDES the cast being resolved
    /// (`1` on the first cast, never `0`).
    ///
    /// NEITHER kind of buff this cast applies is visible: not one applied
    /// by its own procs, and not one in its own
    /// [`ActionDef::apply_buff`] list. Same reason for both, and it is the
    /// point rather than a limitation — a hit cannot be changed by what it
    /// causes. The `apply_buff` case is the newer and more surprising one,
    /// since that buff is written on the action ITSELF and still does not
    /// reach the action's own damage.
    ///
    /// PRECEDENCE: a [`crate::scenario::Phase`] `stats` override for the
    /// same stat WINS over this overlay (phase > overlay > build — the
    /// phase is written last into the slot array). Overriding a stat here
    /// is therefore not a guarantee for the phases that also name it.
    #[serde(default)]
    pub stats: BTreeMap<String, NumOrExpr>,
}

/// What happens when a buff is applied while it is ALREADY active — the
/// per-mechanic collapse of the engine's unified instance list (see the
/// P7 design spec's "Stack model — Approach B" decision).
///
/// The default, [`ReapplyPolicy::Refresh`], is rtce 0.2.0's only behavior:
/// a buff is one window, and reapplying it resets that window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReapplyPolicy {
    /// One instance, its expiry reset to `now + duration`. The binary
    /// buff — and the degenerate case of the whole instance model, which
    /// is why it is the default and why [`BuffDef::max_stacks`] must be
    /// `1` alongside it.
    ///
    /// With a SNAPSHOT [`BuffDef::tick_objective`], the replacement
    /// re-captures the rate UNCONDITIONALLY — so a reapplication in a
    /// weaker moment LOWERS the DoT, which is exactly the opposite of
    /// [`ReapplyPolicy::Strongest`]. Pick deliberately between them.
    #[default]
    Refresh,
    /// Count `+1` up to [`BuffDef::max_stacks`], then EVERY instance's
    /// expiry is reset to `now + duration` — one shared clock, so the
    /// whole stack falls off together (PoE2 charges). At the cap no new
    /// instance is added, but the shared clock is still reset.
    ///
    /// With a SNAPSHOT [`BuffDef::tick_objective`], the shared clock moves
    /// EXPIRIES ONLY: an existing instance keeps the rate it captured,
    /// however long its window is subsequently extended for. Two
    /// consequences worth knowing before choosing this policy — at the cap
    /// the incoming application captures nothing at all (no instance is
    /// added, so its rate is discarded while the clock still resets), and
    /// a continuously-refreshed capped stack can therefore ride a rate
    /// captured arbitrarily long ago.
    AddRefreshAll,
    /// A new instance with its OWN duration, expiring independently
    /// (PoE2 poison). At [`BuffDef::max_stacks`] the earliest-expiring
    /// instance is evicted to make room — the earliest-EXPIRING, not the
    /// weakest, so with a SNAPSHOT [`BuffDef::tick_objective`] a capped
    /// stack can evict a strong instance in favour of a weak one.
    AddIndependent,
    /// A new instance replaces the incumbent only if its snapshot rate is
    /// STRICTLY higher (PoE2 ignite). It needs a magnitude to compare, so
    /// it requires a `tick_objective` with [`TickObjective::snapshot`] set
    /// — and, being a replacement rather than a stack, a
    /// [`BuffDef::max_stacks`] of `1`. Both are fail-closed compile
    /// errors, never silently-borrowed behavior from another policy.
    ///
    /// A LOSING application is discarded WHOLE: it changes neither the
    /// live instance's rate nor its expiry. A weak reapplication cannot
    /// extend a strong ailment — which is the mechanic's whole point, and
    /// the one place `strongest` differs observably from "replace and
    /// refresh, but keep the higher rate".
    Strongest,
}

/// Which `Plan` objective a buff DoT-ticks, and HOW each instance samples
/// that objective's rate.
///
/// Two JSON shapes, both accepted (untagged):
///
/// ```json
/// "tick_objective": "dot_dps"
/// "tick_objective": { "objective": "dot_dps", "snapshot": true }
/// ```
///
/// The bare string is rtce 0.2.0's form and means `snapshot: false` — so
/// every 0.2.0 config parses and behaves unchanged. An object with
/// `snapshot: false` means exactly the same thing as the bare string and
/// SERIALIZES back to it; the object form only carries information when
/// `snapshot` is `true`. An unrecognized key inside the object form is
/// rejected rather than ignored (a typo'd `"snapshots": true` silently
/// meaning "live" is precisely the silent wrong answer this crate refuses
/// to give).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "TickObjectiveRepr", into = "TickObjectiveRepr")]
pub struct TickObjective {
    /// The `Plan` objective whose value is this buff's DoT rate. Must name
    /// an objective of the plan the sim compiles against.
    pub objective: String,
    /// How the rate is sampled while the buff is live:
    ///
    /// - `false` (LIVE, the 0.2.0 behavior) — the objective is
    ///   re-evaluated on every state change and the buff ticks that value
    ///   × its live instance count. A stat or phase change moves the rate
    ///   of every live instance retroactively-from-now.
    /// - `true` (SNAPSHOT) — each instance CAPTURES the objective's value
    ///   at its own application instant and ticks that value unchanged to
    ///   expiry, whatever happens to the state afterwards. The buff's
    ///   total rate is the SUM over live instances, so the stack count is
    ///   already inherent in it and is never multiplied in a second time.
    ///   This is PoE2 ailment semantics, and what
    ///   [`ReapplyPolicy::Strongest`] compares instances by.
    ///
    /// A capture is taken at the application instant against the state the
    /// instance LANDS ON — before this application's own effects fold in.
    /// So if the buff's own [`BuffDef::contributions`] feed the objective
    /// it ticks, it SELF-AMPLIFIES on reapplication, one application
    /// behind: the first instance captures the un-buffed rate, the second
    /// captures it with one stack live, and so on. Deliberate, and the
    /// same instant [`BuffDef::duration`] is evaluated at; if you want the
    /// first instance to see itself, it cannot be expressed here.
    ///
    /// What each [`ReapplyPolicy`] does with a captured rate differs
    /// sharply — see the variants; `refresh` re-captures unconditionally,
    /// `add_refresh_all` never re-captures, and `strongest` re-captures
    /// only on an improvement.
    pub snapshot: bool,
}

impl TickObjective {
    /// A LIVE tick objective (`snapshot: false`) — the 0.2.0 semantics,
    /// and what a bare `"name"` in JSON parses to.
    #[must_use]
    pub fn live(objective: impl Into<String>) -> Self {
        TickObjective {
            objective: objective.into(),
            snapshot: false,
        }
    }

    /// A SNAPSHOT tick objective (`snapshot: true`) — each instance ticks
    /// the rate it captured at its own application.
    #[must_use]
    pub fn snapshot(objective: impl Into<String>) -> Self {
        TickObjective {
            objective: objective.into(),
            snapshot: true,
        }
    }
}

/// The two JSON shapes [`TickObjective`] accepts, as an untagged enum —
/// the serde-only representation `TickObjective` converts through, so the
/// struct itself stays a plain two-field record everywhere else.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum TickObjectiveRepr {
    /// `"dot_dps"` — the 0.2.0 shape, always live.
    Name(String),
    /// `{ "objective": "dot_dps", "snapshot": true }`.
    Object(TickObjectiveObj),
}

/// The object arm of [`TickObjectiveRepr`], as its own struct because
/// `deny_unknown_fields` is a struct-level serde attribute — it does not
/// exist on an enum VARIANT, and silently accepting a typo'd key is not an
/// option here.
///
/// Note the guard is LOCAL to this object, not a crate-wide policy: a
/// misspelled `tick_objectiv` on [`BuffDef`] itself is still silently
/// ignored (and silently means "no DoT"), which is the bigger hole. Making
/// every config struct `deny_unknown_fields` is a crate-wide hygiene
/// change with its own compatibility question, tracked in ROADMAP for
/// 0.4.0 rather than smuggled in here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TickObjectiveObj {
    objective: String,
    #[serde(default)]
    snapshot: bool,
}

impl From<TickObjectiveRepr> for TickObjective {
    fn from(r: TickObjectiveRepr) -> Self {
        match r {
            TickObjectiveRepr::Name(objective) => TickObjective::live(objective),
            TickObjectiveRepr::Object(o) => TickObjective {
                objective: o.objective,
                snapshot: o.snapshot,
            },
        }
    }
}

impl From<TickObjective> for TickObjectiveRepr {
    /// A live objective serializes back to the BARE STRING it most likely
    /// came from, so round-tripping a 0.2.0 config never rewrites its
    /// `tick_objective` into the object form.
    fn from(t: TickObjective) -> Self {
        if t.snapshot {
            TickObjectiveRepr::Object(TickObjectiveObj {
                objective: t.objective,
                snapshot: true,
            })
        } else {
            TickObjectiveRepr::Name(t.objective)
        }
    }
}

/// One buff/debuff: a timed window that, while active, contributes to
/// buckets, drives condition values, and/or accrues a DoT objective.
///
/// A buff is internally an INSTANCE LIST (P7c): every application pushes,
/// refreshes, or replaces an instance per [`BuffDef::on_reapply`], and the
/// live instance count is the buff's stack count. What that count scales,
/// and what it deliberately does NOT:
///
/// - **`contributions`** — each contribution's VALUE is multiplied by the
///   stack count. In a `summed_group` bucket 3 stacks of `+10` fold as
///   `+30`; in a `product` bucket they fold as `×(1 + 30/100)`, NOT
///   `×(1 + 10/100)³`. A per-stack effect is written once, at its
///   per-stack magnitude.
/// - **`conditions`** — driven at their FULL configured value while at
///   least one instance is live, and never scaled by the stack count. A
///   condition is an uptime fraction (`vulnerable = 1.0`), not a
///   quantity, so "3 stacks of vulnerable" has no meaning; the precedence
///   rule (an active buff wins over the scenario's static uptime) is
///   unchanged from 0.2.0.
/// - **`tick_objective`** — a LIVE one ticks at its re-evaluated rate
///   multiplied by the stack count; a SNAPSHOT one ticks the SUM of the
///   rates its instances captured at their own applications, where the
///   count is already inherent in the sum. See [`TickObjective`].
/// - **`buff.<name>`** — `1` while ANY instance is live (never the count;
///   `stacks.<name>` is the count). **`buff_remaining.<name>`** — the
///   LONGEST remaining window across live instances, which under
///   [`ReapplyPolicy::AddIndependent`] is the newest instance's, not the
///   one that expires next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuffDef {
    /// Seconds this buff lasts once applied (refresh-on-reapply resets
    /// the remaining duration back to this value).
    ///
    /// Literal or expression. An expression is evaluated AT EACH
    /// APPLICATION and SNAPSHOTTED onto the window it starts (or
    /// refreshes): a stat/phase change afterwards never retroactively
    /// lengthens or shortens a window already in flight — the NEXT
    /// application re-evaluates and gets the new value. Must be finite and
    /// `>= 0` — see [`NumOrExpr`].
    ///
    /// It reads the LIVE state at that instant, which differs between the
    /// two application paths — deliberately, and this buff's own effects
    /// are never un-folded to hide the difference:
    ///
    /// - **First application** (this buff not currently active):
    ///   `buff.<self>` is `0`, `buff_remaining.<self>` is `0`, and a
    ///   condition this buff drives reads its non-buff value.
    /// - **Refresh** (this buff already active): the outgoing window is
    ///   still in force, so `buff.<self>` is `1`, a condition this buff
    ///   drives reads its BUFF-DRIVEN value, and `buff_remaining.<self>`
    ///   is the time left on the window being REPLACED.
    ///
    /// The refresh reading is what makes pandemic-style refreshes
    /// expressible as data — `"min(12, buff_remaining.x + 8)"` extends by
    /// 8s up to a 12s cap. Bucket CONTRIBUTIONS are invisible on both
    /// paths: buckets are not in the sim symbol space at all.
    ///
    /// NB: the reserved sim symbol `duration` is the SCENARIO's total
    /// length in seconds, not this field. An expression here that names
    /// `duration` reads the fight length.
    pub duration: NumOrExpr,
    /// Bucket contributions active while this buff is up, folded into the
    /// effective build alongside the base `BuildState`'s own.
    #[serde(default)]
    pub contributions: Vec<Contribution>,
    /// Condition name → value while this buff is active. Per the spec's
    /// precedence rule, an active buff driving a condition WINS over the
    /// scenario's static uptime for that condition; the static uptime
    /// applies again once the buff expires.
    #[serde(default)]
    pub conditions: BTreeMap<String, f64>,
    /// The `Plan` objective this buff DoT-ticks, if any: while it is
    /// active, that objective's value × active-seconds accrues into the
    /// sim's damage total.
    ///
    /// Written either as a bare objective name (LIVE — the rate is
    /// re-evaluated on every state change and multiplied by the STACK
    /// COUNT) or as `{ "objective": …, "snapshot": true }` (each instance
    /// ticks the rate it captured at its own application, and the buff's
    /// rate is the SUM over instances). See [`TickObjective`].
    #[serde(default)]
    pub tick_objective: Option<TickObjective>,
    /// How many instances may be live at once. `0` means UNBOUNDED (a
    /// poison stream that only expiry ever trims). Defaults to `1`, which
    /// with the default [`ReapplyPolicy::Refresh`] is exactly rtce
    /// 0.2.0's binary buff — so every 0.2.0 config keeps its behavior
    /// without naming either field.
    ///
    /// `refresh` keeps exactly one instance by definition, so a
    /// `max_stacks` other than `1` alongside it is a fail-closed compile
    /// error rather than a silently-ignored number.
    #[serde(default = "default_max_stacks")]
    pub max_stacks: u32,
    /// What an application does when this buff is already active — see
    /// [`ReapplyPolicy`]. Defaults to [`ReapplyPolicy::Refresh`].
    #[serde(default)]
    pub on_reapply: ReapplyPolicy,
}

/// `1` — one live instance, the 0.2.0 binary buff (see
/// [`BuffDef::max_stacks`]). A free function because `#[serde(default)]`
/// on an integer field would otherwise mean `0`, which this field spells
/// "unbounded".
fn default_max_stacks() -> u32 {
    1
}

/// `max_stacks: 1`, `on_reapply: refresh` — the 0.2.0 binary buff, matching
/// what serde fills in for a config that names neither field. Hand-written
/// rather than derived precisely because a derived `u32` default would be
/// `0` ("unbounded").
impl Default for BuffDef {
    fn default() -> Self {
        BuffDef {
            duration: NumOrExpr::default(),
            contributions: Vec::new(),
            conditions: BTreeMap::new(),
            tick_objective: None,
            max_stacks: default_max_stacks(),
            on_reapply: ReapplyPolicy::default(),
        }
    }
}

/// What event a [`ProcDef`] rolls its chance against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Rolls once per cast begun.
    OnCast,
    /// Rolls once per damaging hit.
    OnHit,
    /// Rolls once per critical hit.
    OnCrit,
}

/// One proc: a chance-triggered effect (apply a buff, or cast a free
/// action) rolled on a trigger event, subject to an internal cooldown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcDef {
    /// Which event this proc rolls its chance against.
    pub trigger: Trigger,
    /// Expression (over stats/conditions/sim-state) for the fire chance
    /// per qualifying roll.
    pub chance: String,
    /// Internal cooldown in seconds after firing (`0.0` = none).
    #[serde(default)]
    pub icd: f64,
    /// Buff to apply when this proc fires. Exactly one of `apply_buff` /
    /// `cast_action` must be set — zero or both is a compile error.
    ///
    /// NB a proc applies AT MOST ONE buff, where
    /// [`ActionDef::apply_buff`] takes a LIST. Same key, same concept,
    /// different arity — so `["x"]` here is a serde type error, and a
    /// bare `"x"` on an action is too. Harmonizing the two is a
    /// config-compatibility change (it needs an untagged accept-both) and
    /// is tracked in ROADMAP for 0.4.0.
    #[serde(default)]
    pub apply_buff: Option<String>,
    /// Action to cast for free (does not consume the rotation's decision
    /// slot) when this proc fires. Exactly one of `apply_buff` /
    /// `cast_action` must be set — zero or both is a compile error.
    #[serde(default)]
    pub cast_action: Option<String>,
    /// Trigger filter: this proc's [`ProcDef::trigger`] only considers
    /// casts of the LISTED actions. `None` (the default) means every
    /// action — rtce 0.2.0's behavior, and what every 0.2.0 config gets
    /// without naming the field.
    ///
    /// Applies to all three triggers alike, because all three are events
    /// of a cast: the filter names the action whose cast produced the
    /// event, so `on_hit`/`on_crit` are filtered by the action that HIT.
    /// A filtered-out cast contributes NOTHING — in [`Trigger::OnHit`]'s
    /// EV accumulator it is not banked any more than an ICD-gated roll
    /// is, and in Monte Carlo mode it consumes no RNG draw.
    ///
    /// A proc-triggered free cast ([`ProcDef::cast_action`]) rolls no
    /// procs at all, so no event this filter could match ever originates
    /// there.
    ///
    /// # What it cannot express
    ///
    /// An inclusive list of casting actions, and nothing else. There is
    /// no negation and no "every action except" — an exclusion has to be
    /// written as the complementary list, which then has to be kept in
    /// step by hand as actions are added. And because the filter always
    /// matches the CASTING action, "on_hit, but only hits of actions
    /// other than this one" is not expressible at all. Left out
    /// deliberately for 0.3.0 rather than guessed at; noted in ROADMAP as
    /// a 0.4.0 candidate, where a real config should decide the shape.
    ///
    /// # Fail-closed
    ///
    /// At `sim::compile`: an unknown action name, and an EMPTY list —
    /// `actions: []` describes a proc that can never fire, which is a
    /// config mistake rather than a way to disable one. Write `None`
    /// (omit the key) for "every action".
    #[serde(default)]
    pub actions: Option<Vec<String>>,
}

/// A priority-list rotation (SimC-style): pure config, drivers may search
/// over rotations the same way they search over gear. The first eligible
/// [`Rule`] wins; hard gates (off cooldown, cost payable, not mid-cast)
/// are enforced by the engine automatically, `when` adds strategy on top.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rotation {
    /// Priority-ordered rules; the first eligible one wins each decision
    /// point. No rule eligible → time advances to the next event (waiting
    /// is modeled, never an infinite loop).
    pub rules: Vec<Rule>,
}

/// One rotation rule: cast `action` when its hard gates pass and (if
/// present) `when` evaluates truthy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// The action this rule casts when eligible.
    pub action: String,
    /// Optional extra eligibility predicate over the sim symbol space
    /// (hard gates are automatic and need not be repeated here). Absent =
    /// always willing (subject to hard gates).
    #[serde(default)]
    pub when: Option<String>,
}

/// Verbatim from docs/superpowers/specs/2026-07-22-p6-sequencing-design.md
/// "Config surface" section (jsonc comments stripped — serde_json has no
/// comment support; every key/value is copied unchanged, including the
/// `cast_time` formula `"1.0 / base_aps"`). Every position that P7b made
/// expression-valued is a plain JSON NUMBER here, which is exactly what
/// makes this the 0.2.0 compatibility fixture — shared by this module's
/// parse test and `sim::exec`'s behavioral one, so there is ONE copy and
/// no unchecked claim of byte-identity between two.
#[cfg(test)]
pub(crate) const P6_SPEC_SIMDEF_JSON: &str = r#"{
      "resources": {
        "mana": { "max": "max_mana", "regen_per_sec": "mana_regen" }
      },
      "actions": {
        "fireball": {
          "cast_time": "1.0 / base_aps",
          "cooldown": 0.0,
          "cost": { "mana": 40.0 },
          "gain": {},
          "damage": {
            "stats": { "coeff_pct": 200.0, "hits_per_use": 1.0 }
          }
        }
      },
      "buffs": {
        "vuln_window": { "duration": 4.0, "conditions": { "vulnerable": 1.0 } },
        "combustion":  { "duration": 8.0,
                         "contributions": [{ "bucket": "indep", "value": 25.0 }] },
        "burning":     { "duration": 6.0, "tick_objective": "dot_dps" }
      },
      "procs": {
        "conflagrate": { "trigger": "on_crit",
                         "chance": "lucky_hit_chance / 100 * 0.3",
                         "icd": 2.0,
                         "apply_buff": "combustion" }
      },
      "damage_objective": "hit_after_dr"
    }"#;

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim from the spec's "Rotation (candidate-domain …)" section.
    const ROTATION_JSON: &str = r#"{ "rules": [
      { "action": "frost_nova", "when": "cooldown.frost_nova == 0 and buff.vuln_window == 0" },
      { "action": "fireball",   "when": "mana >= 40" },
      { "action": "basic_bolt" }
    ]}"#;

    #[test]
    fn simdef_round_trips_the_spec_example_verbatim() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();

        assert_eq!(def.resources["mana"].max, "max_mana");
        assert_eq!(def.resources["mana"].regen_per_sec, "mana_regen");

        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cast_time, "1.0 / base_aps");
        // P7b: these four positions became `NumOrExpr`. Untagged serde
        // must keep reading the spec's plain JSON NUMBERS as `Num` — this
        // is the 0.2.0 backward-compatibility contract at the parse layer
        // (the behavioral half is pinned in `sim::exec`'s tests).
        assert_eq!(fireball.cooldown, NumOrExpr::Num(0.0));
        assert_eq!(fireball.cost["mana"], NumOrExpr::Num(40.0));
        assert!(fireball.gain.is_empty());
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], NumOrExpr::Num(200.0));
        assert_eq!(dmg.stats["hits_per_use"], NumOrExpr::Num(1.0));

        assert_eq!(def.buffs["vuln_window"].duration, NumOrExpr::Num(4.0));
        assert_eq!(def.buffs["vuln_window"].conditions["vulnerable"], 1.0);
        assert_eq!(def.buffs["combustion"].contributions.len(), 1);
        assert_eq!(def.buffs["combustion"].contributions[0].bucket, "indep");
        assert_eq!(def.buffs["combustion"].contributions[0].value, 25.0);
        // P7c-T2: `tick_objective` became an object, and the spec's BARE
        // STRING must still mean exactly what it meant in 0.2.0 — the
        // live rate, never a snapshot.
        assert_eq!(
            def.buffs["burning"].tick_objective,
            Some(TickObjective::live("dot_dps"))
        );

        let proc = &def.procs["conflagrate"];
        assert_eq!(proc.trigger, Trigger::OnCrit);
        assert_eq!(proc.chance, "lucky_hit_chance / 100 * 0.3");
        assert_eq!(proc.icd, 2.0);
        assert_eq!(proc.apply_buff.as_deref(), Some("combustion"));
        assert_eq!(proc.cast_action, None);

        assert_eq!(def.damage_objective, "hit_after_dr");

        // Round-trip through serde again (idempotence).
        let reparsed: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(reparsed.damage_objective, def.damage_objective);
    }

    // P7b: the same five positions, written as STRINGS this time — the
    // untagged enum's other arm. Both arms round-trip, so a config may mix
    // them freely (`"cost": { "mana": 40.0, "rage": "10 + rage_cost" }`).
    const EXPR_SIMDEF_JSON: &str = r#"{
      "actions": {
        "fireball": {
          "cast_time": "1.0 / base_aps",
          "cooldown": "5 + 5",
          "cost": { "mana": "20 + 10" },
          "gain": { "mana": 40.0 },
          "damage": { "stats": { "coeff_pct": "200 * 2", "hits_per_use": 1.0 } }
        }
      },
      "buffs": { "vuln_window": { "duration": "2 + bonus_dur" } },
      "damage_objective": "hit_after_dr"
    }"#;

    #[test]
    fn expression_valued_fields_deserialize_and_round_trip() {
        let def: SimDef = serde_json::from_str(EXPR_SIMDEF_JSON).unwrap();
        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cooldown, NumOrExpr::Expr("5 + 5".into()));
        assert_eq!(fireball.cost["mana"], NumOrExpr::Expr("20 + 10".into()));
        assert_eq!(fireball.gain["mana"], NumOrExpr::Num(40.0));
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], NumOrExpr::Expr("200 * 2".into()));
        assert_eq!(dmg.stats["hits_per_use"], NumOrExpr::Num(1.0));
        assert_eq!(
            def.buffs["vuln_window"].duration,
            NumOrExpr::Expr("2 + bonus_dur".into())
        );

        // Serializing puts each arm back in its own JSON shape (number
        // stays a number, expression stays a string) — round-tripping a
        // config never rewrites a literal as `"40"`.
        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(round.actions["fireball"].cooldown, fireball.cooldown);
        assert_eq!(round.actions["fireball"].gain["mana"], NumOrExpr::Num(40.0));
    }

    // P7c backward-compatibility contract at the parse layer: a 0.2.0
    // config names neither stack field, and must come out as exactly the
    // binary buff — ONE instance, refreshed on reapplication.
    #[test]
    fn omitted_stack_fields_default_to_one_instance_and_refresh() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();
        for name in ["vuln_window", "combustion", "burning"] {
            assert_eq!(def.buffs[name].max_stacks, 1, "{name}");
            assert_eq!(def.buffs[name].on_reapply, ReapplyPolicy::Refresh, "{name}");
        }
    }

    // STRUCTURAL guard, not a per-field one: serde's `#[serde(default)]`
    // attributes and the hand-written `impl Default` are two independent
    // spellings of "what a config that says nothing means", and nothing
    // in the language ties them together. A struct literal catches a
    // MISSING field; only this catches a field whose two defaults
    // DISAGREE — which would silently split Rust-constructed configs from
    // JSON-parsed ones, exactly the bug `max_stacks` was hand-defaulted
    // to avoid (`#[serde(default)]` on a `u32` is `0`, which this field
    // spells "unbounded"). Add every new field to BOTH.
    #[test]
    fn serde_defaults_and_impl_default_agree_field_for_field() {
        let bare: BuffDef = serde_json::from_str(r#"{ "duration": 0.0 }"#).unwrap();
        assert_eq!(
            bare,
            BuffDef::default(),
            "serde's per-field defaults must agree with `impl Default` — \
             a new BuffDef field belongs in BOTH"
        );
    }

    #[test]
    fn stack_fields_parse_and_round_trip_in_snake_case() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": {
                "charge":  { "duration": 5.0, "max_stacks": 3,
                             "on_reapply": "add_refresh_all" },
                "poison":  { "duration": 8.0, "max_stacks": 0,
                             "on_reapply": "add_independent" },
                "ignite":  { "duration": 4.0, "on_reapply": "strongest" }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(def.buffs["charge"].max_stacks, 3);
        assert_eq!(def.buffs["charge"].on_reapply, ReapplyPolicy::AddRefreshAll);
        assert_eq!(def.buffs["poison"].max_stacks, 0); // 0 = unbounded
        assert_eq!(
            def.buffs["poison"].on_reapply,
            ReapplyPolicy::AddIndependent
        );
        // `strongest` PARSES — the vocabulary is complete; `sim::compile`
        // is where it is honestly rejected (see that module's tests).
        assert_eq!(def.buffs["ignite"].on_reapply, ReapplyPolicy::Strongest);
        assert_eq!(def.buffs["ignite"].max_stacks, 1);

        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(
            round.buffs["charge"].on_reapply,
            ReapplyPolicy::AddRefreshAll
        );
        assert_eq!(round.buffs["poison"].max_stacks, 0);
    }

    // P7c-T2: the two `tick_objective` shapes. The bare string is the
    // 0.2.0 form and MUST stay live; the object form is the only way to
    // ask for snapshot semantics, and `snapshot` defaults to `false`
    // there too, so the two spellings of "live" agree.
    #[test]
    fn tick_objective_parses_both_the_bare_name_and_the_object_form() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": {
                "burning": { "duration": 6.0, "tick_objective": "dot_dps" },
                "poison":  { "duration": 8.0,
                             "tick_objective": { "objective": "dot_dps",
                                                 "snapshot": true } },
                "bleed":   { "duration": 5.0,
                             "tick_objective": { "objective": "dot_dps" } }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(
            def.buffs["burning"].tick_objective,
            Some(TickObjective::live("dot_dps"))
        );
        assert_eq!(
            def.buffs["poison"].tick_objective,
            Some(TickObjective::snapshot("dot_dps"))
        );
        assert_eq!(
            def.buffs["bleed"].tick_objective,
            Some(TickObjective::live("dot_dps")),
            "`snapshot` omitted inside the object form must default to false"
        );

        // Round-trip: a LIVE objective goes back out as the bare string it
        // came from (a 0.2.0 config is never rewritten into the object
        // form), and a snapshot one keeps its object.
        let json = serde_json::to_string(&def).unwrap();
        assert!(
            json.contains(r#""tick_objective":"dot_dps""#),
            "a live tick_objective must serialize back to the bare string: {json}"
        );
        let round: SimDef = serde_json::from_str(&json).unwrap();
        assert_eq!(
            round.buffs["poison"].tick_objective,
            def.buffs["poison"].tick_objective
        );
        assert_eq!(
            round.buffs["bleed"].tick_objective,
            def.buffs["bleed"].tick_objective
        );
    }

    // Fail-closed: `"snapshots": true` is a typo, not a config that means
    // "live". Silently ignoring the key would silently give the WRONG DoT
    // semantics — the exact class of quiet wrong answer this crate exists
    // to refuse.
    #[test]
    fn an_unknown_key_inside_the_tick_objective_object_is_rejected() {
        let e = serde_json::from_str::<SimDef>(
            r#"{
              "buffs": { "poison": { "duration": 8.0,
                                     "tick_objective": { "objective": "dot_dps",
                                                         "snapshots": true } } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        // An untagged enum reports "no variant matched" rather than the
        // inner `deny_unknown_fields` message — but it is POSITIONED at
        // the offending value, which is what makes it actionable.
        assert!(
            e.to_string().contains("TickObjectiveRepr") && e.to_string().contains("line 4"),
            "expected a positioned no-variant-matched error, got: {e}"
        );
    }

    // P7d backward-compatibility contract at the parse layer: a 0.2.0
    // config names neither new field, and must come out as exactly the
    // 0.2.0 behavior — an action that applies nothing, and a proc that
    // considers EVERY action.
    #[test]
    fn omitted_action_scoping_fields_default_to_the_020_behavior() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();
        assert!(
            def.actions["fireball"].apply_buff.is_empty(),
            "an action that names no apply_buff applies nothing"
        );
        assert_eq!(
            def.procs["conflagrate"].actions, None,
            "a proc that names no `actions` filter considers every action"
        );
    }

    #[test]
    fn action_scoping_fields_parse_and_round_trip() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "actions": {
                "nova": { "cast_time": "0", "cooldown": 10.0,
                          "apply_buff": ["vuln_window", "chill"] },
                "bolt": { "cast_time": "1" }
              },
              "buffs": { "vuln_window": { "duration": 4.0 },
                         "chill":       { "duration": 2.0 } },
              "procs": {
                "scoped": { "trigger": "on_hit", "chance": "1",
                            "apply_buff": "chill", "actions": ["bolt"] },
                "global": { "trigger": "on_hit", "chance": "1",
                            "apply_buff": "chill" }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        // Source ORDER is preserved (it is the application order — see
        // `ActionDef::apply_buff`), so this is a Vec comparison, not a set
        // one.
        assert_eq!(def.actions["nova"].apply_buff, ["vuln_window", "chill"]);
        assert!(def.actions["bolt"].apply_buff.is_empty());
        assert_eq!(
            def.procs["scoped"].actions.as_deref(),
            Some(["bolt".to_string()].as_slice())
        );
        assert_eq!(def.procs["global"].actions, None);

        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(
            round.actions["nova"].apply_buff,
            def.actions["nova"].apply_buff
        );
        assert_eq!(round.procs["scoped"].actions, def.procs["scoped"].actions);
        assert_eq!(round.procs["global"].actions, None);
    }

    // The same STRUCTURAL guard `BuffDef` carries, for the type P7d gave a
    // new field to. `ActionDef` derives `Default`, so its two spellings of
    // "what a config that says nothing means" are serde's `#[serde(default)]`
    // attributes and that derive — independent, and nothing in the language
    // ties them together. Add every new field to BOTH. (`ProcDef` has no
    // `Default` at all — `trigger`/`chance` are required — so there is no
    // second spelling of its defaults to disagree with.)
    #[test]
    fn action_def_serde_defaults_and_derived_default_agree_field_for_field() {
        let bare: ActionDef = serde_json::from_str(r#"{ "cast_time": "" }"#).unwrap();
        let derived = ActionDef::default();
        assert_eq!(bare.cast_time, derived.cast_time);
        assert_eq!(bare.cooldown, derived.cooldown);
        assert_eq!(bare.cost, derived.cost);
        assert_eq!(bare.gain, derived.gain);
        assert!(bare.damage.is_none() && derived.damage.is_none());
        assert_eq!(
            bare.apply_buff, derived.apply_buff,
            "serde's per-field defaults must agree with the derived \
             `Default` — a new ActionDef field belongs in BOTH"
        );
    }

    #[test]
    fn omitted_cooldown_defaults_to_zero() {
        let def: SimDef = serde_json::from_str(
            r#"{ "actions": { "a": { "cast_time": "1" } }, "damage_objective": "hit" }"#,
        )
        .unwrap();
        assert_eq!(def.actions["a"].cooldown, NumOrExpr::Num(0.0));
        assert!(def.actions["a"].cost.is_empty());
    }

    #[test]
    fn rotation_round_trips_the_spec_example_verbatim() {
        let r: Rotation = serde_json::from_str(ROTATION_JSON).unwrap();
        assert_eq!(r.rules.len(), 3);
        assert_eq!(r.rules[0].action, "frost_nova");
        assert_eq!(
            r.rules[0].when.as_deref(),
            Some("cooldown.frost_nova == 0 and buff.vuln_window == 0")
        );
        assert_eq!(r.rules[1].action, "fireball");
        assert_eq!(r.rules[1].when.as_deref(), Some("mana >= 40"));
        assert_eq!(r.rules[2].action, "basic_bolt");
        assert_eq!(r.rules[2].when, None);
    }
}
