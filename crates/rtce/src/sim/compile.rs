//! `SimDef`/`Rotation` → `SimPlan`: validate every cross-reference
//! fail-closed, then parse and slot-resolve every expression over the
//! extended sim symbol space (see `sim` module docs). This is the only
//! place sim expressions are parsed — do it once per `SimDef`/`Rotation`
//! pair and reuse the result.

use crate::build::Contribution;
use crate::expr::{compile as compile_expr, Program, Symbols};
use crate::plan::{Plan, PlanError};
use crate::simdef::{NumOrExpr, ReapplyPolicy, Rotation, SimDef, Trigger};

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
    Expr(Program),
}

impl CompiledValue {
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
        NumOrExpr::Expr(src) => match compile_expr(src, syms) {
            Ok(p) => Ok(CompiledValue::Expr(p)),
            Err(e) => Err(PlanError {
                what: format!("{}: {e}", what()),
            }),
        },
    }
}

/// Compile a whole `name -> NumOrExpr` map (cost/gain/damage.stats),
/// preserving the source map's (sorted) order.
fn compile_value_map(
    map: &std::collections::BTreeMap<String, NumOrExpr>,
    syms: &SimSymbols<'_>,
    what: impl Fn(&str) -> String,
) -> Result<std::collections::BTreeMap<String, CompiledValue>, PlanError> {
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in map {
        out.insert(k.clone(), compile_value(v, syms, || what(k))?);
    }
    Ok(out)
}

/// One compiled [`crate::simdef::ResourceDef`]: cap/regen expressions
/// ready to evaluate against the combined `[plan slots | sim slots]`
/// array.
#[derive(Debug)]
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
    /// Compiled `cast_time` expression.
    pub cast_time: Program,
    /// Cooldown in seconds, starting when the cast begins — evaluated at
    /// cast start (see [`crate::simdef::NumOrExpr`]).
    pub cooldown: CompiledValue,
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
    /// Buffs this action applies at cast complete, resolved to indices
    /// into [`SimPlan::buffs`] and kept in the CONFIG's list order, which
    /// is the application order (see
    /// [`crate::simdef::ActionDef::apply_buff`] for where in the
    /// completion instant they land and what a repeat means).
    pub apply_buff: Vec<usize>,
}

/// One compiled [`crate::simdef::BuffDef`].
#[derive(Debug)]
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
}

/// One compiled [`crate::simdef::TickObjective`]: the objective RESOLVED
/// to its index in the `Plan`'s objective slice, plus how the executor
/// samples it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledTick {
    /// Index into the `Plan`'s objective slice.
    pub objective: usize,
    /// `true` — each instance ticks the rate it captured at its own
    /// application, and the buff's rate is the SUM over instances;
    /// `false` — the rate is re-evaluated live and multiplied by the
    /// instance count. See [`crate::simdef::TickObjective::snapshot`].
    pub snapshot: bool,
}

/// What a firing [`crate::simdef::ProcDef`] does, resolved to an index —
/// exactly one of `apply_buff`/`cast_action` was set in the source
/// `ProcDef` (validated at compile time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcEffect {
    /// Apply the buff at this index in `SimPlan::buffs`.
    ApplyBuff(usize),
    /// Cast the action at this index in `SimPlan::actions`.
    CastAction(usize),
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
    pub chance: Program,
    /// Internal cooldown in seconds after firing.
    pub icd: f64,
    /// What firing this proc does, resolved to an index.
    pub effect: ProcEffect,
    /// Trigger filter, resolved to indices into [`SimPlan::actions`]:
    /// `None` = every action (the 0.2.0 behavior), `Some(list)` = only
    /// casts of those actions produce a qualifying event. Never
    /// `Some(empty)` — [`compile`] rejects that (see
    /// [`crate::simdef::ProcDef::actions`]).
    pub actions: Option<Vec<usize>>,
}

/// One compiled [`crate::simdef::Rule`]: the action index it casts and its
/// optional `when` predicate.
#[derive(Debug)]
pub struct CompiledRule {
    /// Index into `SimPlan::actions`.
    pub action: usize,
    /// Compiled `when` predicate, if present (absent = always eligible,
    /// subject to the executor's automatic hard gates).
    pub when: Option<Program>,
}

/// A `SimDef` + `Rotation` compiled once against a [`Plan`]: every
/// expression parsed and slot-resolved over the extended sim symbol space,
/// every cross-reference validated fail-closed (see `sim` module docs),
/// ready for a future executor to drive. Inert data — no execution logic
/// lives here (that's P6c).
#[derive(Debug)]
pub struct SimPlan {
    /// The slot offset where the sim-state segment begins — equal to the
    /// underlying `Plan`'s own unified slot width
    /// (`[stats | conditions | buckets | stages | event_factors]`). Sim
    /// expressions load plan stats/conditions from slots below this
    /// offset and sim state from slots at/above it.
    pub sim_base: usize,
    /// Total width of the combined `[plan slots | sim slots]` array a
    /// future executor must allocate.
    pub slot_width: usize,
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
    /// Rotation rules, compiled, in priority (source) order, with
    /// resolved action indices.
    pub rules: Vec<CompiledRule>,
    /// Index into the `Plan`'s objective slice for `damage_objective`.
    pub damage_objective: usize,
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
    resource_names: Vec<&'a str>,
    resource_base: usize,
    action_names: Vec<&'a str>,
    cooldown_base: usize,
    buff_names: Vec<&'a str>,
    buff_base: usize,
    buff_remaining_base: usize,
    casts_base: usize,
    stacks_base: usize,
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
        SimSymbols {
            plan,
            time_slot: base,
            duration_slot: base + 1,
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

    fn slot_width(&self) -> usize {
        self.stacks_base + self.buff_names.len()
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
        if let Some(i) = self.resource_names.iter().position(|r| *r == name) {
            return Some((self.resource_base + i) as u16);
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
        if name == "time" || name == "duration" {
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

    // Actions: cost/gain resource references and `apply_buff` targets
    // must be defined.
    for (name, action) in &simdef.actions {
        for r in action.cost.keys().chain(action.gain.keys()) {
            if !simdef.resources.contains_key(r) {
                return Err(PlanError {
                    what: format!("action `{name}`: unknown resource `{r}` in cost/gain"),
                });
            }
        }
        for b in &action.apply_buff {
            if !simdef.buffs.contains_key(b) {
                return Err(PlanError {
                    what: format!("action `{name}`: unknown buff `{b}` in apply_buff"),
                });
            }
        }
    }

    // Buffs: tick_objective must name a plan objective, and the stack
    // policy must be one this engine actually honors.
    for (name, buff) in &simdef.buffs {
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

    // Procs: exactly one of apply_buff/cast_action, and it must exist.
    for (name, p) in &simdef.procs {
        match (&p.apply_buff, &p.cast_action) {
            (Some(_), Some(_)) => {
                return Err(PlanError {
                    what: format!("proc `{name}`: exactly one of apply_buff/cast_action, got both"),
                })
            }
            (None, None) => {
                return Err(PlanError {
                    what: format!(
                        "proc `{name}`: exactly one of apply_buff/cast_action, got neither"
                    ),
                })
            }
            (Some(b), None) if !simdef.buffs.contains_key(b) => {
                return Err(PlanError {
                    what: format!("proc `{name}`: unknown buff `{b}`"),
                })
            }
            (None, Some(a)) if !simdef.actions.contains_key(a) => {
                return Err(PlanError {
                    what: format!("proc `{name}`: unknown action `{a}`"),
                })
            }
            _ => {}
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
        let max = compile_expr(&r.max, &syms).map_err(|e| PlanError {
            what: format!("resource `{name}` max: {e}"),
        })?;
        let regen_per_sec = compile_expr(&r.regen_per_sec, &syms).map_err(|e| PlanError {
            what: format!("resource `{name}` regen_per_sec: {e}"),
        })?;
        resources.push(CompiledResource {
            name: name.clone(),
            max,
            regen_per_sec,
        });
    }

    let mut actions = Vec::new();
    for (name, a) in &simdef.actions {
        let cast_time = compile_expr(&a.cast_time, &syms).map_err(|e| PlanError {
            what: format!("action `{name}` cast_time: {e}"),
        })?;
        let cooldown = compile_value(&a.cooldown, &syms, || format!("action `{name}` cooldown"))?;
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
            Some(d) => Some(compile_value_map(&d.stats, &syms, |k| {
                format!("action `{name}` damage.stats `{k}`")
            })?),
            None => None,
        };
        actions.push(CompiledAction {
            name: name.clone(),
            cast_time,
            cooldown,
            cost,
            gain,
            damage,
            apply_buff: a.apply_buff.iter().map(|b| buff_index(b)).collect(),
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
        });
    }

    let mut procs = Vec::new();
    for (name, p) in &simdef.procs {
        let chance = compile_expr(&p.chance, &syms).map_err(|e| PlanError {
            what: format!("proc `{name}` chance: {e}"),
        })?;
        let effect = if let Some(b) = &p.apply_buff {
            ProcEffect::ApplyBuff(buff_index(b))
        } else {
            ProcEffect::CastAction(action_index(
                p.cast_action
                    .as_ref()
                    .expect("exactly-one-of validated above"),
            ))
        };
        procs.push(CompiledProc {
            name: name.clone(),
            trigger: p.trigger,
            chance,
            icd: p.icd,
            effect,
            actions: p
                .actions
                .as_ref()
                .map(|list| list.iter().map(|a| action_index(a)).collect()),
        });
    }

    let mut rules = Vec::new();
    for rule in &rotation.rules {
        let when = match &rule.when {
            Some(w) => Some(compile_expr(w, &syms).map_err(|e| PlanError {
                what: format!("rotation rule (action `{}`) when: {e}", rule.action),
            })?),
            None => None,
        };
        rules.push(CompiledRule {
            action: action_index(&rule.action),
            when,
        });
    }

    Ok(SimPlan {
        sim_base: syms.sim_base(),
        slot_width: syms.slot_width(),
        resources,
        actions,
        buffs,
        procs,
        rules,
        damage_objective,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedef::GameDef;
    use crate::plan;
    use crate::simdef::{
        ActionDamage, ActionDef, BuffDef, NumOrExpr, ProcDef, ReapplyPolicy, ResourceDef, Rule,
        TickObjective,
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
                cast_time: "1.0 / base_aps".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage { stats: dmg_stats }),
                apply_buff: Vec::new(),
            },
        );
        actions.insert(
            "frost_nova".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(10.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
                apply_buff: Vec::new(),
            },
        );

        let mut buffs = BTreeMap::new();
        let mut vuln_conditions = BTreeMap::new();
        vuln_conditions.insert("vulnerable".to_string(), 1.0);
        buffs.insert(
            "vuln_window".to_string(),
            BuffDef {
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
                trigger: Trigger::OnCrit,
                chance: "lucky_hit_chance / 100 * 0.3".into(),
                icd: 2.0,
                apply_buff: Some("combustion".into()),
                cast_action: None,
                actions: None,
            },
        );

        SimDef {
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
            rules: vec![
                Rule {
                    action: "frost_nova".into(),
                    when: Some("and(cooldown.frost_nova == 0, buff.vuln_window == 0)".into()),
                },
                Rule {
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
        assert_eq!(sp.procs[0].effect, ProcEffect::ApplyBuff(1)); // "combustion"

        assert_eq!(sp.rules.len(), 2);
        assert_eq!(sp.rules[0].action, 1); // "frost_nova"
        assert_eq!(sp.rules[1].action, 0); // "fireball"

        assert_eq!(sp.damage_objective, 0); // "hit_after_dr"

        assert_eq!(sp.sim_base, plan.own_slot_width());
        // sim segment: time, duration, mana(1), cooldown.*(2),
        // buff.*(3), buff_remaining.*(3), casts.*(2), stacks.*(3)
        //   = 2+1+2+3+3+2+3 = 16  (P7c appended the `stacks.*` sub-range)
        assert_eq!(sp.slot_width, sp.sim_base + 16);
    }

    #[test]
    fn unknown_action_in_rotation_rule_is_rejected() {
        let plan = toy_plan();
        let mut rotation = valid_rotation();
        rotation.rules.push(Rule {
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
            sp.actions[1].apply_buff,
            vec![2, 0],
            "list order is application order, NOT the buffs' sorted order"
        );
        assert!(sp.actions[0].apply_buff.is_empty());
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

    #[test]
    fn rule_with_no_when_is_always_eligible_and_compiles() {
        let plan = toy_plan();
        let mut simdef = valid_simdef();
        simdef.actions.insert(
            "basic_bolt".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
                apply_buff: Vec::new(),
            },
        );
        let mut rotation = valid_rotation();
        rotation.rules.push(Rule {
            action: "basic_bolt".into(),
            when: None,
        });
        let sp = compile(&plan, &simdef, &rotation).unwrap();
        assert!(sp.rules.last().unwrap().when.is_none());
    }
}
