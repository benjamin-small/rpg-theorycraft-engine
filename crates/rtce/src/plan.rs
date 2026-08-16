//! GameDef → Plan (compile once) and Plan × BuildState × Scenario →
//! objectives (hot path, borrowed from scratch — no allocation). One
//! unified slot array is shared by every compiled expression:
//! [stats | conditions | buckets | stages | event multiplier].

use crate::build::BuildState;
use crate::expr::{compile as compile_expr, ExprError, Program, Symbols};
use crate::gamedef::{FoldKind, GameDef, StageDef};
use crate::rng::Pcg32;
use crate::scenario::{Phase, Scenario};

/// Hard cap on events: 2^8 = 256 branches per branched stage.
pub const MAX_EVENTS: usize = 8;

/// Hard safety ceiling for one solve stage's configured bisection budget.
/// The configured `max_iterations` remains the tighter per-stage bound.
pub const MAX_SOLVE_ITERATIONS: u32 = 4096;

/// Hard safety ceiling for one recurrence stage's transition budget.
pub const MAX_RECURRENCE_ITERATIONS: u32 = 100_000;

/// Hard ceiling for one recurrence stage's local state width.
pub const MAX_RECURRENCE_STATE_SLOTS: usize = 256;

/// A `plan::compile` or `Plan::evaluate`/`explain` failure: bad GameDef
/// config (unknown/duplicate names, too many events, reserved names) or a
/// bad runtime input (unknown stat/bucket/event/condition reference,
/// non-finite/negative phase weight, empty scenario).
///
/// `#[non_exhaustive]`: the engine's to extend with more positional or
/// contextual detail; no consumer constructs one.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PlanError {
    /// Human-readable description of what went wrong.
    pub what: String,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.what)
    }
}
impl std::error::Error for PlanError {}

impl From<ExprError> for PlanError {
    fn from(e: ExprError) -> Self {
        PlanError {
            what: e.to_string(),
        }
    }
}

/// A `GameDef` compiled once into a flat, ready-to-evaluate form: every
/// expression parsed and slot-resolved, every name checked, laid out over
/// the unified slot array `[stats | conditions | buckets | stages |
/// event multiplier]`. Build one with `plan::compile` and reuse it for every
/// `evaluate`/`explain` call — that's the whole "compile once, evaluate
/// fast" contract.
#[derive(Debug)]
pub struct Plan {
    stat_names: Vec<String>,
    condition_names: Vec<String>,
    bucket_names: Vec<String>,
    bucket_folds: Vec<FoldKind>,
    events: Vec<CompiledEvent>,
    event_names: Vec<String>,
    stages: Vec<CompiledStage>,
    /// Indexes into `stages` exported as objectives.
    objective_stages: Vec<usize>,
    /// Slot layout offsets.
    n_stats: usize,
    n_conditions: usize,
    n_buckets: usize,
    n_stages: usize,
    max_recurrence_states: usize,
}

#[derive(Debug)]
struct CompiledEvent {
    chance: Program,
    factor: Program,
}

#[derive(Debug)]
struct CompiledStage {
    name: String,
    kind: CompiledStageKind,
}

#[derive(Debug)]
enum CompiledStageKind {
    Expression { program: Program, branched: bool },
    Solve(CompiledSolve),
    Recurrence(CompiledRecurrence),
}

#[derive(Debug)]
struct CompiledSolve {
    lower: Program,
    upper: Program,
    residual: Program,
    variable_slot: usize,
    absolute_tolerance: f64,
    relative_tolerance: f64,
    max_iterations: u32,
}

#[derive(Debug)]
struct CompiledRecurrence {
    state_names: Vec<String>,
    initial: Vec<Program>,
    next: Vec<Program>,
    until: Program,
    result: Program,
    state_slot_base: usize,
    max_iterations: u32,
}

/// Compile-time symbol table over the unified slot layout. The engine's
/// per-branch event-multiplier slot is exposed through the reserved legacy
/// `event_factors` name and the clearer `event_multiplier` alias. The alias is
/// a fallback: an existing config declaration with that name keeps its
/// original meaning. Engine context resolves only when `branched` is set.
struct StageSymbols<'a> {
    stats: &'a [String],
    conditions: &'a [String],
    buckets: &'a [String],
    prior_stages: &'a [String],
}

impl Symbols for StageSymbols<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        let n_stats = self.stats.len();
        let n_conditions = self.conditions.len();
        let n_buckets = self.buckets.len();
        if let Some(i) = self.stats.iter().position(|s| s == name) {
            return Some(i as u16);
        }
        if let Some(i) = self.conditions.iter().position(|c| c == name) {
            return Some((n_stats + i) as u16);
        }
        if let Some(i) = self.buckets.iter().position(|b| b == name) {
            return Some((n_stats + n_conditions + i) as u16);
        }
        if let Some(i) = self.prior_stages.iter().position(|s| s == name) {
            return Some((n_stats + n_conditions + n_buckets + i) as u16);
        }
        // The event multiplier lives after ALL stages; prior_stages grows per
        // stage but the slot is fixed using the FULL stage count, patched
        // by compile() via `total_stages`.
        None
    }
}

/// Wrapper that first preserves ordinary declared-name resolution, then makes
/// the engine event-multiplier names available in branched stages at the fixed
/// slot after all stages.
struct WithEventFactors<'a> {
    inner: StageSymbols<'a>,
    slot: u16,
    branched: bool,
    readable_alias_available: bool,
}

impl Symbols for WithEventFactors<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        if let Some(slot) = self.inner.slot(name) {
            return Some(slot);
        }
        if name == "event_factors" || (name == "event_multiplier" && self.readable_alias_available)
        {
            return self.branched.then_some(self.slot);
        }
        None
    }
}

/// Solver residual symbol table: normal stage symbols plus exactly one local
/// variable in the otherwise engine-private slot after all pipeline stages.
struct WithSolveVariable<'a> {
    inner: StageSymbols<'a>,
    variable: &'a str,
    slot: u16,
}

impl Symbols for WithSolveVariable<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        if name == self.variable {
            Some(self.slot)
        } else {
            self.inner.slot(name)
        }
    }
}

/// Recurrence expression symbol table: normal stage symbols plus every local
/// state slot. Local names are compile-time collision-checked first.
struct WithRecurrenceState<'a> {
    inner: StageSymbols<'a>,
    names: &'a [String],
    slot_base: usize,
}

impl Symbols for WithRecurrenceState<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        if let Some(slot) = self.inner.slot(name) {
            return Some(slot);
        }
        self.names
            .iter()
            .position(|candidate| candidate == name)
            .map(|index| (self.slot_base + index) as u16)
    }
}

fn is_plain_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The ONE source of truth for build finiteness: every `BuildState` stat
/// value and contribution value must be finite. Called from `Plan`'s
/// scenario-level resolution (`run`), its per-phase resolution
/// (`validate_and_resolve_build_for_phase`, the sim's per-cast path), and
/// `sim::run`'s entry walk — so the three levels agree on the message BY
/// CONSTRUCTION, not by copy-paste. Finiteness only: stat NAMES and
/// bucket references stay validated where they resolve.
pub(crate) fn validate_finite_build(build: &BuildState) -> Result<(), PlanError> {
    for (name, v) in &build.stats {
        if !v.is_finite() {
            return Err(PlanError {
                what: format!("build stat `{name}` must be finite, got {v}"),
            });
        }
    }
    // A NaN/inf value would FOLD (Σ or Π) into every branch and come back
    // as `Ok(NaN)` total damage — the 0.3.0 release review's standing
    // repro. Fail closed instead.
    for c in &build.contributions {
        if !c.value.is_finite() {
            return Err(PlanError {
                what: format!(
                    "contribution value into bucket `{}` must be finite, got {}",
                    c.bucket, c.value
                ),
            });
        }
    }
    Ok(())
}

/// The ONE source of truth for phase stat-override finiteness — the same
/// silent-NaN class as `validate_finite_build`, shared by the same three
/// call sites (plus phase uptimes, which stay inline where the condition
/// registry is at hand). Mirrors the `Phase.uptimes` "must be finite"
/// style.
pub(crate) fn validate_finite_phase_stats(phase: &Phase) -> Result<(), PlanError> {
    for (name, v) in &phase.stats {
        if !v.is_finite() {
            return Err(PlanError {
                what: format!(
                    "phase `{}` stat `{name}` must be finite, got {v}",
                    phase.name
                ),
            });
        }
    }
    Ok(())
}

/// Compile a `GameDef` into a `Plan`: validate names (no collisions across
/// stats/conditions/buckets/stages, no more than `MAX_EVENTS` events, no
/// stage seeing a later stage, the legacy `event_factors` name reserved, and
/// engine event context legal only in `branched` stages), parse and
/// slot-resolve every expression, and lay
/// out the unified slot array. This is the only place expressions get
/// parsed — do it once per `GameDef` and reuse the result.
pub fn compile(def: &GameDef) -> Result<Plan, PlanError> {
    // P8a: the unknown-key walk for the one GameDef-side struct that
    // stores its unknowns — an `EventDef`'s name is the key of the map it
    // sits in, only known here. (`GameDef`/`BucketDef`/`StageDef` reject
    // unknown keys at PARSE instead — see `config_keys`'s module docs.)
    for (name, ev) in &def.events {
        crate::config_keys::reject_unknown(
            &format!("event `{name}`"),
            crate::gamedef::EventDef::KNOWN_KEYS,
            &ev.extra,
        )?;
    }

    // `event_factors` has always been reserved for the injected per-branch
    // slot. `event_multiplier` is a new readable alias, so a pre-existing
    // config declaration with that name must keep winning rather than become
    // a compatibility break; in that uncommon case only the legacy spelling
    // exposes the engine slot.
    let bucket_names: Vec<String> = def.buckets.keys().cloned().collect();
    let stage_names: Vec<String> = def.pipeline.iter().map(|s| s.name().to_owned()).collect();
    let readable_alias_available = !def
        .stats
        .iter()
        .chain(&bucket_names)
        .chain(&stage_names)
        .chain(&def.conditions)
        .any(|name| name == "event_multiplier");
    for name in def
        .stats
        .iter()
        .chain(&bucket_names)
        .chain(&stage_names)
        .chain(&def.conditions)
    {
        if name == "event_factors" {
            return Err(PlanError {
                what: format!("`{name}` is reserved"),
            });
        }
    }

    // One flat namespace: stats, conditions, buckets, stages must not collide.
    let mut seen = std::collections::BTreeSet::new();
    for name in def
        .stats
        .iter()
        .chain(&def.conditions)
        .chain(&bucket_names)
        .chain(&stage_names)
    {
        if !seen.insert(name.clone()) {
            return Err(PlanError {
                what: format!("duplicate name `{name}`"),
            });
        }
    }
    if def.events.len() > MAX_EVENTS {
        return Err(PlanError {
            what: format!("{} events > max {MAX_EVENTS}", def.events.len()),
        });
    }

    let n_stats = def.stats.len();
    let n_conditions = def.conditions.len();
    let n_buckets = bucket_names.len();
    let n_stages = stage_names.len();
    let own_slot_width = n_stats + n_conditions + n_buckets + n_stages + 1;
    if own_slot_width > u16::MAX as usize + 1 {
        return Err(PlanError {
            what: "plan exceeds the expression slot limit".into(),
        });
    }
    let event_factors_slot = (own_slot_width - 1) as u16;
    let recurrence_state_slot_base = own_slot_width;
    let max_recurrence_states = def
        .pipeline
        .iter()
        .filter_map(StageDef::as_recurrence)
        .map(|stage| stage.recurrence.state.len())
        .max()
        .unwrap_or(0);
    if max_recurrence_states > MAX_RECURRENCE_STATE_SLOTS {
        return Err(PlanError {
            what: format!(
                "{max_recurrence_states} recurrence state slots > max {MAX_RECURRENCE_STATE_SLOTS}"
            ),
        });
    }
    if recurrence_state_slot_base
        .checked_add(max_recurrence_states)
        .is_none_or(|width| width > u16::MAX as usize + 1)
    {
        return Err(PlanError {
            what: "plan and recurrence state exceed the expression slot limit".into(),
        });
    }

    let event_syms = StageSymbols {
        stats: &def.stats,
        conditions: &def.conditions,
        buckets: &bucket_names,
        prior_stages: &[],
    };
    let mut events = Vec::new();
    let mut event_names = Vec::new();
    for (name, ev) in &def.events {
        events.push(CompiledEvent {
            chance: compile_expr(&ev.chance, &event_syms).map_err(|e| PlanError {
                what: format!("event `{name}` chance: {e}"),
            })?,
            factor: compile_expr(&ev.factor, &event_syms).map_err(|e| PlanError {
                what: format!("event `{name}` factor: {e}"),
            })?,
        });
        event_names.push(name.clone());
    }

    let mut stages: Vec<CompiledStage> = Vec::new();
    for (i, s) in def.pipeline.iter().enumerate() {
        let base_syms = StageSymbols {
            stats: &def.stats,
            conditions: &def.conditions,
            buckets: &bucket_names,
            prior_stages: &stage_names[..i],
        };
        let (name, kind) = match s {
            StageDef::Expression(s) => {
                let syms = WithEventFactors {
                    inner: base_syms,
                    slot: event_factors_slot,
                    branched: s.branched,
                    readable_alias_available,
                };
                let program = compile_expr(&s.expr, &syms).map_err(|e| PlanError {
                    what: format!("stage `{}`: {e}", s.name),
                })?;
                (
                    s.name.clone(),
                    CompiledStageKind::Expression {
                        program,
                        branched: s.branched,
                    },
                )
            }
            StageDef::Solve(s) => {
                let solve = &s.solve;
                if !is_plain_identifier(&solve.variable) {
                    return Err(PlanError {
                        what: format!(
                            "solve stage `{}` variable `{}` must be a plain ASCII identifier",
                            s.name, solve.variable
                        ),
                    });
                }
                if def
                    .stats
                    .iter()
                    .chain(&def.conditions)
                    .chain(&bucket_names)
                    .chain(&stage_names)
                    .any(|name| name == &solve.variable)
                    || matches!(
                        solve.variable.as_str(),
                        "event_factors" | "event_multiplier"
                    )
                {
                    return Err(PlanError {
                        what: format!(
                            "solve stage `{}` variable `{}` collides with a declared or engine name",
                            s.name, solve.variable
                        ),
                    });
                }
                if !solve.absolute_tolerance.is_finite()
                    || solve.absolute_tolerance < 0.0
                    || !solve.relative_tolerance.is_finite()
                    || solve.relative_tolerance < 0.0
                    || (solve.absolute_tolerance == 0.0 && solve.relative_tolerance == 0.0)
                {
                    return Err(PlanError {
                        what: format!(
                            "solve stage `{}` tolerances must be finite and non-negative, with at least one greater than zero",
                            s.name
                        ),
                    });
                }
                if solve.max_iterations == 0 || solve.max_iterations > MAX_SOLVE_ITERATIONS {
                    return Err(PlanError {
                        what: format!(
                            "solve stage `{}` max_iterations must be in 1..={MAX_SOLVE_ITERATIONS}, got {}",
                            s.name, solve.max_iterations
                        ),
                    });
                }
                let lower = compile_expr(&solve.lower, &base_syms).map_err(|e| PlanError {
                    what: format!("solve stage `{}` lower: {e}", s.name),
                })?;
                let upper = compile_expr(&solve.upper, &base_syms).map_err(|e| PlanError {
                    what: format!("solve stage `{}` upper: {e}", s.name),
                })?;
                let residual_syms = WithSolveVariable {
                    inner: base_syms,
                    variable: &solve.variable,
                    slot: event_factors_slot,
                };
                let residual =
                    compile_expr(&solve.residual, &residual_syms).map_err(|e| PlanError {
                        what: format!("solve stage `{}` residual: {e}", s.name),
                    })?;
                (
                    s.name.clone(),
                    CompiledStageKind::Solve(CompiledSolve {
                        lower,
                        upper,
                        residual,
                        variable_slot: event_factors_slot as usize,
                        absolute_tolerance: solve.absolute_tolerance,
                        relative_tolerance: solve.relative_tolerance,
                        max_iterations: solve.max_iterations,
                    }),
                )
            }
            StageDef::Recurrence(s) => {
                let recurrence = &s.recurrence;
                if recurrence.state.is_empty() {
                    return Err(PlanError {
                        what: format!(
                            "recurrence stage `{}` must declare at least one state slot",
                            s.name
                        ),
                    });
                }
                if recurrence.max_iterations == 0
                    || recurrence.max_iterations > MAX_RECURRENCE_ITERATIONS
                {
                    return Err(PlanError {
                        what: format!(
                            "recurrence stage `{}` max_iterations must be in 1..={MAX_RECURRENCE_ITERATIONS}, got {}",
                            s.name, recurrence.max_iterations
                        ),
                    });
                }

                let mut local_names = Vec::with_capacity(recurrence.state.len());
                let mut local_seen = std::collections::BTreeSet::new();
                for state in &recurrence.state {
                    if !is_plain_identifier(&state.name) {
                        return Err(PlanError {
                            what: format!(
                                "recurrence stage `{}` state `{}` must be a plain ASCII identifier",
                                s.name, state.name
                            ),
                        });
                    }
                    if !local_seen.insert(state.name.clone()) {
                        return Err(PlanError {
                            what: format!(
                                "recurrence stage `{}` has duplicate state `{}`",
                                s.name, state.name
                            ),
                        });
                    }
                    if def
                        .stats
                        .iter()
                        .chain(&def.conditions)
                        .chain(&bucket_names)
                        .chain(&stage_names)
                        .any(|name| name == &state.name)
                        || matches!(state.name.as_str(), "event_factors" | "event_multiplier")
                    {
                        return Err(PlanError {
                            what: format!(
                                "recurrence stage `{}` state `{}` collides with a declared or engine name",
                                s.name, state.name
                            ),
                        });
                    }
                    local_names.push(state.name.clone());
                }

                // Initializers are deliberately simultaneous too: they read
                // ordinary symbols only, never another local initializer.
                let mut initial = Vec::with_capacity(recurrence.state.len());
                for state in &recurrence.state {
                    initial.push(compile_expr(&state.initial, &base_syms).map_err(|e| {
                        PlanError {
                            what: format!(
                                "recurrence stage `{}` state `{}` initial: {e}",
                                s.name, state.name
                            ),
                        }
                    })?);
                }

                let recurrence_syms = WithRecurrenceState {
                    inner: base_syms,
                    names: &local_names,
                    slot_base: recurrence_state_slot_base,
                };
                let mut next = Vec::with_capacity(recurrence.state.len());
                for state in &recurrence.state {
                    next.push(compile_expr(&state.next, &recurrence_syms).map_err(|e| {
                        PlanError {
                            what: format!(
                                "recurrence stage `{}` state `{}` next: {e}",
                                s.name, state.name
                            ),
                        }
                    })?);
                }
                let until =
                    compile_expr(&recurrence.until, &recurrence_syms).map_err(|e| PlanError {
                        what: format!("recurrence stage `{}` until: {e}", s.name),
                    })?;
                let result =
                    compile_expr(&recurrence.result, &recurrence_syms).map_err(|e| PlanError {
                        what: format!("recurrence stage `{}` result: {e}", s.name),
                    })?;
                (
                    s.name.clone(),
                    CompiledStageKind::Recurrence(CompiledRecurrence {
                        state_names: local_names,
                        initial,
                        next,
                        until,
                        result,
                        state_slot_base: recurrence_state_slot_base,
                        max_iterations: recurrence.max_iterations,
                    }),
                )
            }
        };
        stages.push(CompiledStage { name, kind });
    }

    if def.objectives.is_empty() {
        return Err(PlanError {
            what: "no objectives".into(),
        });
    }
    let mut objective_stages = Vec::new();
    for o in &def.objectives {
        let idx = stage_names
            .iter()
            .position(|s| s == o)
            .ok_or_else(|| PlanError {
                what: format!("unknown objective `{o}`"),
            })?;
        objective_stages.push(idx);
    }

    Ok(Plan {
        stat_names: def.stats.clone(),
        condition_names: def.conditions.clone(),
        bucket_names,
        bucket_folds: def.buckets.values().map(|b| b.fold).collect(),
        events,
        event_names,
        stages,
        objective_stages,
        n_stats,
        n_conditions,
        n_buckets,
        n_stages,
        max_recurrence_states,
    })
}

/// Per-phase teaching trace — the "show your work" path. Allocates freely;
/// tracing is OFF on the evaluate() hot path.
///
/// `#[non_exhaustive]`: a read-only report the engine produces and no
/// consumer constructs — the same category as `sim::report`'s types, so
/// later measurements are additive rather than breaking.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct Explanation {
    /// Same values, same order, as `evaluate`'s objective slice.
    pub objectives: Vec<f64>,
    /// One trace per scenario phase, in scenario order.
    pub phases: Vec<PhaseTrace>,
}

/// Teaching trace for one scenario phase: every condition/bucket/stage
/// value it resolved to, plus every event-branch a `branched` stage
/// enumerated.
///
/// `#[non_exhaustive]`: a read-only report the engine produces and no
/// consumer constructs — the same category as `sim::report`'s types, so
/// later measurements are additive rather than breaking.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct PhaseTrace {
    /// This phase's name.
    pub name: String,
    /// This phase's weight, already normalized (weight / sum of weights).
    pub weight: f64,
    /// Condition name → resolved uptime (post-clamp), in registry order.
    pub conditions: Vec<(String, f64)>,
    /// Bucket name → resolved (base, non-branched) slot value, in
    /// registry order.
    pub buckets: Vec<(String, f64)>,
    /// Stage name → resolved value (the branch-weighted EV for a
    /// `branched` stage), in pipeline order.
    pub stages: Vec<(String, f64)>,
    /// Every event-branch enumerated by every `branched` stage in this
    /// phase, in stage-then-mask order.
    pub branches: Vec<BranchTrace>,
}

/// One event-branch of one `branched` stage: which events fired, that
/// branch's probability weight, and the value it contributed.
///
/// `#[non_exhaustive]`: a read-only report the engine produces and no
/// consumer constructs — the same category as `sim::report`'s types, so
/// later measurements are additive rather than breaking.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
pub struct BranchTrace {
    /// Name of the `branched` stage this branch belongs to.
    pub stage: String,
    /// Names of the events that fired on this branch (empty = none fired).
    pub fired: Vec<String>,
    /// This branch's probability weight (product of fired chances and
    /// unfired 1-chances); branches summing to zero weight are skipped.
    pub weight: f64,
    /// The `event_multiplier` for this branch: the product of every fired
    /// event's factor expression (1.0 if none fired). The serialized field
    /// retains its original `event_factors` name for API compatibility.
    pub event_factors: f64,
    /// This branch's stage expression value.
    pub value: f64,
}

impl Plan {
    /// Look up a stat's slot index by name, or `None` if it isn't in this
    /// plan's stat registry.
    pub fn stat_id(&self, name: &str) -> Option<usize> {
        self.stat_names.iter().position(|s| s == name)
    }
    /// The names of the stages exported as objectives, in the order their
    /// values appear in `evaluate`'s/`explain`'s objective slice.
    pub fn objective_names(&self) -> Vec<&str> {
        self.objective_stages
            .iter()
            .map(|&i| self.stages[i].name.as_str())
            .collect()
    }

    /// pub(crate): a condition's slot index within this plan's OWN unified
    /// layout (`[stats | conditions | buckets | stages | event multiplier]`),
    /// or `None` if it isn't in this plan's condition registry. Mirrors
    /// `stat_id`; `sim::compile` uses both to extend the flat namespace
    /// without exposing the plan's internal layout publicly.
    pub(crate) fn condition_id(&self, name: &str) -> Option<usize> {
        self.condition_names
            .iter()
            .position(|c| c == name)
            .map(|i| self.n_stats + i)
    }

    /// pub(crate): the total width of this plan's own unified slot array
    /// (`n_stats + n_conditions + n_buckets + n_stages + 1` for the
    /// event multiplier). `sim::compile` appends its own sim-state slots
    /// immediately after this offset — see `sim` module docs for the
    /// documented order.
    pub(crate) fn own_slot_width(&self) -> usize {
        self.n_stats + self.n_conditions + self.n_buckets + self.n_stages + 1
    }
}

/// Preallocated evaluation buffers — `evaluate` performs no heap allocation;
/// results are read from the scratch's objective buffer. A scratch must come
/// from the SAME `Plan` that later calls `evaluate` with it (buffer lengths
/// are sized to that plan's slot layout; `evaluate` debug-asserts this).
pub struct EvalScratch {
    slots: Vec<f64>,
    branch_slots: Vec<f64>,
    recurrence_next: Vec<f64>,
    base_bucket_raw: Vec<f64>,
    branch_bucket_raw: Vec<f64>,
    objectives: Vec<f64>,
    stat_base: Vec<f64>,
}

/// How [`Plan::eval_phase`] resolves each `branched` stage — the ONE
/// parameterization point `evaluate`/`explain`/`evaluate_phase`/
/// `evaluate_phase_sampled` all share (never a forked copy of the engine;
/// see the design spec's "same internal run/eval path parameterized"
/// wording for `evaluate_sampled`).
enum EvalKind<'a> {
    /// The hot path: full branch enumeration, probability-weighted EV, no
    /// tracing. What `evaluate`/`evaluate_phase` use.
    Hot,
    /// Full branch enumeration (same math as `Hot`) PLUS per-phase/
    /// per-branch `PhaseTrace`/`BranchTrace` collection. What `explain`
    /// uses.
    Trace,
    /// Monte Carlo: each `branched` stage samples exactly ONE mask via an
    /// independent Bernoulli draw per event against `rng`, instead of
    /// enumerating every mask. What `evaluate_phase_sampled` uses.
    Sample(&'a mut Pcg32),
}

impl Plan {
    /// Allocate scratch buffers sized to THIS plan's slot layout. The
    /// result must only be passed to `evaluate` on this same `Plan` —
    /// `evaluate` debug-asserts the buffer lengths match.
    pub fn scratch(&self) -> EvalScratch {
        let n = self.own_slot_width() + self.max_recurrence_states;
        EvalScratch {
            slots: vec![0.0; n],
            branch_slots: vec![0.0; n],
            recurrence_next: vec![0.0; self.max_recurrence_states],
            base_bucket_raw: vec![0.0; self.n_buckets],
            branch_bucket_raw: vec![0.0; self.n_buckets],
            objectives: vec![0.0; self.objective_stages.len()],
            stat_base: vec![0.0; self.n_stats],
        }
    }

    /// Hot path: no allocation, no tracing. Returns the objective slice
    /// borrowed from `scratch`.
    pub fn evaluate<'s>(
        &self,
        build: &BuildState,
        scenario: &Scenario,
        scratch: &'s mut EvalScratch,
    ) -> Result<&'s [f64], PlanError> {
        self.run(build, scenario, scratch, None)?;
        Ok(&scratch.objectives)
    }

    /// Teaching path: runs the SAME engine as `evaluate` with per-phase,
    /// per-stage, per-branch tracing turned on. Allocates freely (one
    /// `Explanation` tree); the hot `evaluate` path never takes this branch.
    pub fn explain(
        &self,
        build: &BuildState,
        scenario: &Scenario,
        scratch: &mut EvalScratch,
    ) -> Result<Explanation, PlanError> {
        let mut explanation = Explanation {
            objectives: Vec::new(),
            phases: Vec::new(),
        };
        self.run(build, scenario, scratch, Some(&mut explanation))?;
        explanation.objectives = scratch.objectives.clone();
        Ok(explanation)
    }

    /// The single evaluation engine. `trace: None` is the hot path (no
    /// allocation beyond what the caller already provided via `scratch`);
    /// `trace: Some(_)` pushes a `PhaseTrace`/`BranchTrace` per phase/branch
    /// as it goes — all of that bookkeeping is gated behind `if let
    /// Some(..)` so it costs nothing when tracing is off.
    fn run(
        &self,
        build: &BuildState,
        scenario: &Scenario,
        scratch: &mut EvalScratch,
        mut trace: Option<&mut Explanation>,
    ) -> Result<(), PlanError> {
        let n = self.own_slot_width() + self.max_recurrence_states;
        debug_assert_eq!(scratch.slots.len(), n, "scratch must come from this plan");
        debug_assert_eq!(
            scratch.branch_slots.len(),
            n,
            "scratch must come from this plan"
        );
        debug_assert_eq!(
            scratch.recurrence_next.len(),
            self.max_recurrence_states,
            "scratch must come from this plan"
        );
        debug_assert_eq!(
            scratch.base_bucket_raw.len(),
            self.n_buckets,
            "scratch must come from this plan"
        );
        debug_assert_eq!(
            scratch.objectives.len(),
            self.objective_stages.len(),
            "scratch must come from this plan"
        );
        debug_assert_eq!(
            scratch.stat_base.len(),
            self.n_stats,
            "scratch must come from this plan"
        );
        if scenario.phases.is_empty() {
            return Err(PlanError {
                what: "scenario has no phases".into(),
            });
        }
        // Fail-closed per-phase: a single negative/non-finite weight could
        // otherwise hide inside a positive sum, and a non-finite uptime
        // would silently poison a fold via NaN propagation instead of
        // erroring.
        for phase in &scenario.phases {
            if !phase.weight.is_finite() || phase.weight < 0.0 {
                return Err(PlanError {
                    what: format!(
                        "phase `{}` weight must be finite and non-negative, got {}",
                        phase.name, phase.weight
                    ),
                });
            }
            for (cond, v) in &phase.uptimes {
                if !self.condition_names.iter().any(|n| n == cond) {
                    return Err(PlanError {
                        what: format!(
                            "phase `{}`: unknown condition `{cond}` in phase uptimes",
                            phase.name
                        ),
                    });
                }
                if !v.is_finite() {
                    return Err(PlanError {
                        what: format!(
                            "phase `{}` uptime `{cond}` must be finite, got {v}",
                            phase.name
                        ),
                    });
                }
            }
            validate_finite_phase_stats(phase)?;
        }
        let weight_sum: f64 = scenario.phases.iter().map(|p| p.weight).sum();
        // Fail-closed on NaN too: `weight_sum > 0.0` is false for NaN, so
        // negating it would accept NaN; check both sides explicitly instead
        // (clippy::neg_cmp_op_on_partial_ord).
        if weight_sum.is_nan() || weight_sum <= 0.0 {
            return Err(PlanError {
                what: "phase weights must sum > 0".into(),
            });
        }

        // Resolve + validate build ONCE: finiteness (the shared walk),
        // then stats and contribution tags.
        validate_finite_build(build)?;
        for slot in scratch.stat_base.iter_mut() {
            *slot = 0.0;
        }
        for (name, v) in &build.stats {
            let i = self.stat_id(name).ok_or_else(|| PlanError {
                what: format!("unknown stat `{name}`"),
            })?;
            scratch.stat_base[i] = *v;
        }
        // Contribution tags validate per call (cheap linear scans over
        // small registries; index resolution caches are a P5 concern).
        for c in &build.contributions {
            if !self.bucket_names.iter().any(|b| b == &c.bucket) {
                return Err(PlanError {
                    what: format!("unknown bucket `{}`", c.bucket),
                });
            }
            if let Some(e) = &c.event {
                if !self.event_names.iter().any(|n| n == e) {
                    return Err(PlanError {
                        what: format!("unknown event `{e}`"),
                    });
                }
            }
            if let Some(cd) = &c.condition {
                if !self.condition_names.iter().any(|n| n == cd) {
                    return Err(PlanError {
                        what: format!("unknown condition `{cd}`"),
                    });
                }
            }
        }

        for o in scratch.objectives.iter_mut() {
            *o = 0.0;
        }

        for phase in &scenario.phases {
            let w = phase.weight / weight_sum;
            let kind = if trace.is_some() {
                EvalKind::Trace
            } else {
                EvalKind::Hot
            };
            let (pt, _fired_mask) = self.eval_phase(build, phase, w, scratch, kind)?;
            if let Some(pt) = pt {
                trace
                    .as_mut()
                    .expect("phase_trace is Some only when trace is Some")
                    .phases
                    .push(pt);
            }
        }

        Ok(())
    }

    /// Evaluate ONE phase, accumulating `w * stage_value` into
    /// `scratch.objectives` per objective — the per-phase body shared by
    /// `run`'s scenario loop (`w` = that phase's normalized weight,
    /// `stat_base`/contributions resolved ONCE by the caller for the whole
    /// scenario) and [`Plan::evaluate_phase`]/[`Plan::evaluate_phase_sampled`]
    /// (`w = 1.0`, resolved fresh per call — see those methods' docs).
    /// `kind` picks how each `branched` stage resolves (see [`EvalKind`]).
    /// Returns the built `PhaseTrace` when `kind` is [`EvalKind::Trace`]
    /// (else `None`, no allocation) PLUS the union, over every `branched`
    /// stage in this phase, of [`EvalKind::Sample`]'s per-stage sampled
    /// fired-event mask (`0` for `Hot`/`Trace`, which don't sample) — see
    /// [`Plan::evaluate_phase_sampled`]'s docs for why a union and not
    /// "the last stage's mask".
    fn eval_phase(
        &self,
        build: &BuildState,
        phase: &Phase,
        w: f64,
        scratch: &mut EvalScratch,
        mut kind: EvalKind<'_>,
    ) -> Result<(Option<PhaseTrace>, u32), PlanError> {
        let want_trace = matches!(kind, EvalKind::Trace);
        let mut phase_trace: Option<PhaseTrace> = want_trace.then(|| PhaseTrace {
            name: phase.name.clone(),
            weight: w,
            conditions: Vec::new(),
            buckets: Vec::new(),
            stages: Vec::new(),
            branches: Vec::new(),
        });
        let mut fired_mask_union: u32 = 0;

        // Stats/conditions/base-bucket prefix — shared with `crit_chance`.
        self.fill_prefix_slots(build, phase, scratch)?;

        let n_stats = self.n_stats;
        let bucket_base = n_stats + self.n_conditions;
        if let Some(pt) = phase_trace.as_mut() {
            for (ci, name) in self.condition_names.iter().enumerate() {
                pt.conditions
                    .push((name.clone(), scratch.slots[n_stats + ci]));
            }
            for (bi, name) in self.bucket_names.iter().enumerate() {
                pt.buckets
                    .push((name.clone(), scratch.slots[bucket_base + bi]));
            }
        }

        // Stages in order.
        for (si, stage) in self.stages.iter().enumerate() {
            let out_slot = bucket_base + self.n_buckets + si;
            let (program, branched) = match &stage.kind {
                CompiledStageKind::Expression { program, branched } => (program, *branched),
                CompiledStageKind::Solve(solve) => {
                    scratch.slots[out_slot] =
                        Self::eval_solve(&stage.name, solve, &mut scratch.slots)?;
                    continue;
                }
                CompiledStageKind::Recurrence(recurrence) => {
                    scratch.slots[out_slot] = Self::eval_recurrence(
                        &stage.name,
                        recurrence,
                        &mut scratch.slots,
                        &mut scratch.recurrence_next,
                    )?;
                    continue;
                }
            };
            if !branched {
                scratch.slots[out_slot] = program.eval(&scratch.slots);
                continue;
            }
            // Chances depend only on PHASE slots (not on the branch), so
            // evaluate each event's chance once per stage rather than once
            // per mask/sample.
            let n_ev = self.events.len();
            let mut chances = [0.0f64; MAX_EVENTS];
            for (ei, ev) in self.events.iter().enumerate() {
                chances[ei] = ev.chance.eval(&scratch.slots).clamp(0.0, 1.0);
            }

            match &mut kind {
                EvalKind::Sample(rng) => {
                    // Monte Carlo: ONE mask, sampled by an independent
                    // Bernoulli draw per event — the branch this hit
                    // actually rolled, not a probability blend.
                    let mut mask = 0u32;
                    for (ei, &p) in chances.iter().enumerate().take(n_ev) {
                        if rng.next_f64() < p {
                            mask |= 1 << ei;
                        }
                    }
                    let (value, _factors) =
                        self.eval_branch(build, phase, mask, bucket_base, program, scratch)?;
                    scratch.slots[out_slot] = value;
                    fired_mask_union |= mask;
                }
                EvalKind::Hot | EvalKind::Trace => {
                    // EV: full 2^n branch enumeration, probability-weighted.
                    let mut ev_acc = 0.0;
                    for mask in 0u32..(1 << n_ev) {
                        // Weight = Π fired ? p : 1-p.
                        let mut weight = 1.0;
                        for (ei, &p) in chances.iter().enumerate().take(n_ev) {
                            weight *= if mask & (1 << ei) != 0 { p } else { 1.0 - p };
                        }
                        if weight == 0.0 {
                            continue;
                        }
                        let (branch_value, factors) =
                            self.eval_branch(build, phase, mask, bucket_base, program, scratch)?;
                        ev_acc += weight * branch_value;
                        if let Some(pt) = phase_trace.as_mut() {
                            let fired: Vec<String> = (0..n_ev)
                                .filter(|&ei| mask & (1 << ei) != 0)
                                .map(|ei| self.event_names[ei].clone())
                                .collect();
                            pt.branches.push(BranchTrace {
                                stage: stage.name.clone(),
                                fired,
                                weight,
                                event_factors: factors,
                                value: branch_value,
                            });
                        }
                    }
                    scratch.slots[out_slot] = ev_acc;
                }
            }
        }
        if let Some(pt) = phase_trace.as_mut() {
            for (si, stage) in self.stages.iter().enumerate() {
                pt.stages.push((
                    stage.name.clone(),
                    scratch.slots[bucket_base + self.n_buckets + si],
                ));
            }
        }

        for (oi, &si) in self.objective_stages.iter().enumerate() {
            scratch.objectives[oi] += w * scratch.slots[bucket_base + self.n_buckets + si];
        }

        Ok((phase_trace, fired_mask_union))
    }

    /// Fill `scratch.slots[0 .. bucket_base + n_buckets]` from
    /// `scratch.stat_base` (already resolved by the caller) and `phase`:
    /// stats (base + phase override), conditions (phase uptime, clamped),
    /// and BASE bucket raw folds (event-gated contributions excluded — see
    /// [`Plan::fold_buckets`]) written into bucket slots. This is the exact
    /// prefix [`Plan::eval_phase`] then evaluates stages/branches on top
    /// of, and the exact prefix a branched stage's own `chance`
    /// expressions read — shared with [`Plan::crit_chance`], which reads
    /// no further than this.
    fn fill_prefix_slots(
        &self,
        build: &BuildState,
        phase: &Phase,
        scratch: &mut EvalScratch,
    ) -> Result<(), PlanError> {
        let n_stats = self.n_stats;
        scratch.slots[..n_stats].copy_from_slice(&scratch.stat_base);
        for (name, v) in &phase.stats {
            let i = self.stat_id(name).ok_or_else(|| PlanError {
                what: format!("unknown stat `{name}`"),
            })?;
            scratch.slots[i] = *v;
        }
        for (ci, name) in self.condition_names.iter().enumerate() {
            scratch.slots[n_stats + ci] = phase
                .uptimes
                .get(name)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
        }
        let bucket_base = n_stats + self.n_conditions;
        self.fold_buckets(build, phase, None, &mut scratch.base_bucket_raw)?;
        self.write_bucket_slots(&scratch.base_bucket_raw, bucket_base, &mut scratch.slots);
        Ok(())
    }

    /// Evaluate one branch (`mask`) of a `branched` stage into
    /// `scratch.branch_slots`, leaving `scratch.slots` (the base prefix)
    /// untouched: bucket refold with this mask's event-gated contributions
    /// included, `event_factors` = Π fired events' factors, then the
    /// stage's own expression. Returns `(value, event_factors)`. The ONE
    /// per-branch computation both [`EvalKind`] strategies read from (full
    /// enumeration and single-sample alike) — never forked.
    fn eval_branch(
        &self,
        build: &BuildState,
        phase: &Phase,
        mask: u32,
        bucket_base: usize,
        program: &Program,
        scratch: &mut EvalScratch,
    ) -> Result<(f64, f64), PlanError> {
        scratch.branch_slots.copy_from_slice(&scratch.slots);
        self.fold_buckets(build, phase, Some(mask), &mut scratch.branch_bucket_raw)?;
        self.write_bucket_slots(
            &scratch.branch_bucket_raw,
            bucket_base,
            &mut scratch.branch_slots,
        );
        let mut factors = 1.0;
        for (ei, ev) in self.events.iter().enumerate() {
            if mask & (1 << ei) != 0 {
                factors *= ev.factor.eval(&scratch.branch_slots);
            }
        }
        let ef_slot = bucket_base + self.n_buckets + self.n_stages;
        scratch.branch_slots[ef_slot] = factors;
        let value = program.eval(&scratch.branch_slots);
        Ok((value, factors))
    }

    /// Evaluate one configured scalar solve. The invariant is always
    /// `residual(lo) <= 0 <= residual(hi)`; returning `lo` is therefore a
    /// deterministic conservative answer even when the iteration budget is
    /// exactly exhausted at the tolerance boundary.
    fn eval_solve(
        stage_name: &str,
        solve: &CompiledSolve,
        slots: &mut [f64],
    ) -> Result<f64, PlanError> {
        let mut lo = solve.lower.eval(slots);
        if !lo.is_finite() {
            return Err(PlanError {
                what: format!("solve stage `{stage_name}` lower bound must be finite, got {lo}"),
            });
        }
        let mut hi = solve.upper.eval(slots);
        if !hi.is_finite() {
            return Err(PlanError {
                what: format!("solve stage `{stage_name}` upper bound must be finite, got {hi}"),
            });
        }
        if lo > hi {
            return Err(PlanError {
                what: format!(
                    "solve stage `{stage_name}` bounds are inverted: lower {lo} > upper {hi}"
                ),
            });
        }

        let eval_residual = |x: f64, slots: &mut [f64]| -> Result<f64, PlanError> {
            slots[solve.variable_slot] = x;
            let residual = solve.residual.eval(slots);
            if residual.is_finite() {
                Ok(residual)
            } else {
                Err(PlanError {
                    what: format!(
                        "solve stage `{stage_name}` residual is non-finite at {x}: {residual}"
                    ),
                })
            }
        };

        let lo_residual = eval_residual(lo, slots)?;
        if lo_residual > 0.0 {
            return Err(PlanError {
                what: format!(
                    "solve stage `{stage_name}` root is unbracketed: residual(lower={lo}) = {lo_residual} > 0"
                ),
            });
        }
        let hi_residual = eval_residual(hi, slots)?;
        if hi_residual < 0.0 {
            return Err(PlanError {
                what: format!(
                    "solve stage `{stage_name}` root is unbracketed: residual(upper={hi}) = {hi_residual} < 0"
                ),
            });
        }
        if hi_residual == 0.0 {
            return Ok(hi);
        }

        let converged = |lo: f64, hi: f64| -> Result<bool, PlanError> {
            let width = hi - lo;
            let tolerance =
                solve.absolute_tolerance + solve.relative_tolerance * lo.abs().max(hi.abs());
            if !tolerance.is_finite() {
                return Err(PlanError {
                    what: format!(
                        "solve stage `{stage_name}` effective tolerance is non-finite for [{lo}, {hi}]"
                    ),
                });
            }
            // Two finite opposite-sign extremes can have an infinite
            // representable width. They are simply not converged yet; the
            // overflow-safe midpoint below narrows them normally.
            Ok(width.is_finite() && width <= tolerance)
        };

        for _ in 0..solve.max_iterations {
            if converged(lo, hi)? {
                slots[solve.variable_slot] = lo;
                return Ok(lo);
            }
            // Halving before addition avoids overflowing `hi - lo` while
            // retaining deterministic IEEE arithmetic on native and Wasm.
            let mid = lo * 0.5 + hi * 0.5;
            if mid == lo || mid == hi {
                slots[solve.variable_slot] = lo;
                return Ok(lo);
            }
            let residual = eval_residual(mid, slots)?;
            if residual <= 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }

        if converged(lo, hi)? {
            slots[solve.variable_slot] = lo;
            Ok(lo)
        } else {
            Err(PlanError {
                what: format!(
                    "solve stage `{stage_name}` did not converge within {} iterations; final bracket [{lo}, {hi}]",
                    solve.max_iterations
                ),
            })
        }
    }

    /// Evaluate one configured bounded state recurrence. Initializers and all
    /// `next` programs are compiled once. Every transition is simultaneous:
    /// all next values read the same previous state, land in `next_state`,
    /// and are copied into the local slots only after every value is finite.
    fn eval_recurrence(
        stage_name: &str,
        recurrence: &CompiledRecurrence,
        slots: &mut [f64],
        next_state: &mut [f64],
    ) -> Result<f64, PlanError> {
        let state_len = recurrence.state_names.len();
        debug_assert!(next_state.len() >= state_len);
        let state_range = recurrence.state_slot_base..recurrence.state_slot_base + state_len;

        for (index, ((name, initial), slot)) in recurrence
            .state_names
            .iter()
            .zip(&recurrence.initial)
            .zip(state_range.clone())
            .enumerate()
        {
            let value = initial.eval_finite(slots).map_err(|value| PlanError {
                    what: format!(
                        "recurrence stage `{stage_name}` state `{name}` is non-finite at iteration 0: {value}"
                    ),
                })?;
            slots[slot] = value;
            debug_assert_eq!(index, slot - recurrence.state_slot_base);
        }

        for iteration in 0..=recurrence.max_iterations {
            let terminal = recurrence.until.eval_finite(slots).map_err(|terminal| PlanError {
                    what: format!(
                        "recurrence stage `{stage_name}` terminal predicate is non-finite at iteration {iteration}: {terminal}"
                    ),
                })?;
            if terminal != 0.0 {
                let result = recurrence.result.eval_finite(slots).map_err(|result| PlanError {
                        what: format!(
                            "recurrence stage `{stage_name}` result is non-finite at iteration {iteration}: {result}"
                        ),
                    })?;
                return Ok(result);
            }
            if iteration == recurrence.max_iterations {
                return Err(PlanError {
                    what: format!(
                        "recurrence stage `{stage_name}` did not terminate within {} iterations",
                        recurrence.max_iterations
                    ),
                });
            }

            let next_iteration = iteration + 1;
            for (index, (name, next)) in recurrence
                .state_names
                .iter()
                .zip(&recurrence.next)
                .enumerate()
            {
                let value = next.eval_finite(slots).map_err(|value| PlanError {
                        what: format!(
                            "recurrence stage `{stage_name}` state `{name}` is non-finite at iteration {next_iteration}: {value}"
                        ),
                    })?;
                next_state[index] = value;
            }
            slots[state_range.clone()].copy_from_slice(&next_state[..state_len]);
        }

        unreachable!("bounded recurrence loop always returns")
    }

    /// Shared by [`Plan::evaluate_phase`]/[`Plan::evaluate_phase_sampled`]/
    /// [`Plan::crit_chance`]: validate `phase`'s own uptime keys/
    /// finiteness (no scenario to check — see `evaluate_phase`'s docs),
    /// resolve `build`'s stat base fresh into `scratch.stat_base`, and
    /// validate its contribution tags. Never touches `scratch.objectives`
    /// — callers that report objectives zero it themselves.
    fn validate_and_resolve_build_for_phase(
        &self,
        build: &BuildState,
        phase: &Phase,
        scratch: &mut EvalScratch,
    ) -> Result<(), PlanError> {
        for (cond, v) in &phase.uptimes {
            if !self.condition_names.iter().any(|n| n == cond) {
                return Err(PlanError {
                    what: format!(
                        "phase `{}`: unknown condition `{cond}` in phase uptimes",
                        phase.name
                    ),
                });
            }
            if !v.is_finite() {
                return Err(PlanError {
                    what: format!(
                        "phase `{}` uptime `{cond}` must be finite, got {v}",
                        phase.name
                    ),
                });
            }
        }
        validate_finite_phase_stats(phase)?;
        validate_finite_build(build)?;

        for slot in scratch.stat_base.iter_mut() {
            *slot = 0.0;
        }
        for (name, v) in &build.stats {
            let i = self.stat_id(name).ok_or_else(|| PlanError {
                what: format!("unknown stat `{name}`"),
            })?;
            scratch.stat_base[i] = *v;
        }
        for c in &build.contributions {
            if !self.bucket_names.iter().any(|b| b == &c.bucket) {
                return Err(PlanError {
                    what: format!("unknown bucket `{}`", c.bucket),
                });
            }
            if let Some(e) = &c.event {
                if !self.event_names.iter().any(|n| n == e) {
                    return Err(PlanError {
                        what: format!("unknown event `{e}`"),
                    });
                }
            }
            if let Some(cd) = &c.condition {
                if !self.condition_names.iter().any(|n| n == cd) {
                    return Err(PlanError {
                        what: format!("unknown condition `{cd}`"),
                    });
                }
            }
        }
        Ok(())
    }

    /// pub(crate): evaluate a SINGLE phase directly, weight `1.0`, no
    /// scenario-level blending — for `sim::exec`'s per-cast/per-tick
    /// evaluations, which construct a synthetic one-phase view of the
    /// CURRENT sim phase (current phase's stats/uptimes, with buff-driven
    /// condition overrides folded in — see `sim::exec` module docs) and
    /// would otherwise need to allocate a throwaway single-phase
    /// `Scenario` on every cast just to reuse `evaluate`. Unlike
    /// `evaluate`, there is no scenario to validate (non-empty phases,
    /// weight-sum > 0) — only this one phase's own uptime keys/finiteness
    /// are checked (its `weight` field is ignored; `w` is fixed at `1.0`).
    /// Resolves `build`'s stat base and validates its contributions fresh
    /// on every call (unlike `evaluate`, which does this once for a whole
    /// multi-phase scenario) — the sim passes a DIFFERENT build each call
    /// (base + active buffs' contributions + this cast's stat overrides),
    /// so there is nothing to cache across calls in v1.
    pub(crate) fn evaluate_phase<'s>(
        &self,
        build: &BuildState,
        phase: &Phase,
        scratch: &'s mut EvalScratch,
    ) -> Result<&'s [f64], PlanError> {
        self.validate_and_resolve_build_for_phase(build, phase, scratch)?;
        for o in scratch.objectives.iter_mut() {
            *o = 0.0;
        }
        self.eval_phase(build, phase, 1.0, scratch, EvalKind::Hot)?;
        Ok(&scratch.objectives)
    }

    /// pub(crate): the Monte Carlo counterpart of [`Plan::evaluate_phase`]
    /// — the SAME engine (`eval_phase` with [`EvalKind::Sample`] instead of
    /// [`EvalKind::Hot`]), so every `branched` stage samples ONE mask
    /// (independent Bernoulli draw per event, via `rng`) instead of
    /// enumerating every mask and blending by probability. Returns the
    /// objective slice (as `evaluate_phase` does) PLUS the union, over
    /// every `branched` stage this phase's pipeline has, of that stage's
    /// own sampled fired-event mask (bit `i` set ⇒ event `i` fired on AT
    /// LEAST ONE branched stage). Every `GameDef` in this crate (toy and
    /// D4 alike) has exactly one branched stage, so "union" and "that one
    /// stage's own mask" coincide today; a future multi-branched-stage
    /// `GameDef` would need this documented — a union is the conservative
    /// v1 choice (an event counts as "fired this hit" if it fired
    /// ANYWHERE in the pipeline), rather than picking one stage's mask
    /// arbitrarily and silently dropping information a caller like
    /// `sim::exec`'s `on_crit` proc trigger might need from a different
    /// stage.
    pub(crate) fn evaluate_phase_sampled<'s>(
        &self,
        build: &BuildState,
        phase: &Phase,
        rng: &mut Pcg32,
        scratch: &'s mut EvalScratch,
    ) -> Result<(&'s [f64], u32), PlanError> {
        self.validate_and_resolve_build_for_phase(build, phase, scratch)?;
        for o in scratch.objectives.iter_mut() {
            *o = 0.0;
        }
        let (_, fired_mask) = self.eval_phase(build, phase, 1.0, scratch, EvalKind::Sample(rng))?;
        Ok((&scratch.objectives, fired_mask))
    }

    /// pub(crate): the probability the event literally named `"crit"`
    /// fires for one hit against `phase`, evaluated against the exact
    /// STAT/CONDITION/base-BUCKET prefix a branched stage's own event
    /// `chance` expressions read (see [`Plan::fill_prefix_slots`]).
    /// Returns `0.0` if this Plan defines no event named `"crit"` (no crit
    /// concept in this game — fail-soft, not fail-closed, since "no crit"
    /// is a legitimate game shape, not a config error).
    ///
    /// `sim::exec`'s EV executor binds its `on_crit` proc trigger to this
    /// specific literal event name — the design spec is silent on how a
    /// generic engine should recognize "the crit event" out of an
    /// arbitrary user-named event registry, and every `GameDef` fixture/
    /// example in this crate already calls its crit event `"crit"` (see
    /// `plan.rs`'s own `toy_def`, `search.rs`, `gamedef.rs`,
    /// `sim::exec`'s own toy fixtures, and the `diablo4_basics`/
    /// `guide_03_events_and_branches` examples) — a documented v1
    /// convention, not a
    /// silent guess.
    pub(crate) fn crit_chance(
        &self,
        build: &BuildState,
        phase: &Phase,
        scratch: &mut EvalScratch,
    ) -> Result<f64, PlanError> {
        let Some(ei) = self.event_names.iter().position(|n| n == "crit") else {
            return Ok(0.0);
        };
        self.validate_and_resolve_build_for_phase(build, phase, scratch)?;
        self.fill_prefix_slots(build, phase, scratch)?;
        Ok(self.events[ei].chance.eval(&scratch.slots).clamp(0.0, 1.0))
    }

    /// pub(crate): whether the bit for the event named `"crit"` is set in
    /// an [`Plan::evaluate_phase_sampled`] fired-mask — `false` if this
    /// Plan has no such event (mirrors [`Plan::crit_chance`]'s "no crit
    /// concept" fallback; see that method's docs for the naming
    /// convention this rests on).
    pub(crate) fn is_crit_bit_set(&self, mask: u32) -> bool {
        self.event_names
            .iter()
            .position(|n| n == "crit")
            .is_some_and(|i| mask & (1 << i) != 0)
    }

    /// pub(crate): fill `out[0 .. n_stats + n_conditions]` with this
    /// build's stat values (overridden by `phase.stats`) and this phase's
    /// condition uptimes (clamped to `[0, 1]`) — the SAME prefix
    /// `evaluate`/`evaluate_phase` build internally, exposed standalone
    /// for `sim::exec`'s own extended slot array (see `sim` module docs:
    /// sim expressions load plan stats/conditions from exactly the indices
    /// `stat_id`/`condition_id` resolve, below `SimPlan::sim_base`). Never
    /// touches buckets/stages/`event_factors` — sim expressions can never
    /// reference them (rejected at `sim::compile` time) — so `out` may be
    /// sized to `SimPlan::slot_width` and left untouched past this prefix.
    pub(crate) fn write_stat_condition_slots(
        &self,
        build: &BuildState,
        phase: &Phase,
        out: &mut [f64],
    ) -> Result<(), PlanError> {
        let n_stats = self.n_stats;
        for slot in out[..n_stats].iter_mut() {
            *slot = 0.0;
        }
        for (name, v) in &build.stats {
            let i = self.stat_id(name).ok_or_else(|| PlanError {
                what: format!("unknown stat `{name}`"),
            })?;
            out[i] = *v;
        }
        for (name, v) in &phase.stats {
            let i = self.stat_id(name).ok_or_else(|| PlanError {
                what: format!("unknown stat `{name}`"),
            })?;
            out[i] = *v;
        }
        for (ci, name) in self.condition_names.iter().enumerate() {
            out[n_stats + ci] = phase
                .uptimes
                .get(name)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
        }
        Ok(())
    }

    /// Raw fold per bucket: Sum→Σ; SummedGroup→Σ (wrapped on write);
    /// Product→Π(1+v/100). `fired_mask: None` = base (no event contribs);
    /// Some(mask) = include contribs whose event bit is set.
    fn fold_buckets(
        &self,
        build: &BuildState,
        phase: &crate::scenario::Phase,
        fired_mask: Option<u32>,
        out: &mut [f64],
    ) -> Result<(), PlanError> {
        for (bi, fold) in self.bucket_folds.iter().enumerate() {
            out[bi] = match fold {
                FoldKind::Product => 1.0,
                _ => 0.0,
            };
        }
        for c in &build.contributions {
            let bi = self
                .bucket_names
                .iter()
                .position(|b| b == &c.bucket)
                .unwrap();
            if let Some(e) = &c.event {
                let ei = self.event_names.iter().position(|n| n == e).unwrap();
                match fired_mask {
                    Some(mask) if mask & (1 << ei) != 0 => {}
                    _ => continue,
                }
            }
            let mut v = c.value;
            if let Some(cond) = &c.condition {
                let uptime = phase.uptimes.get(cond).copied().unwrap_or(0.0);
                v *= uptime.clamp(0.0, 1.0);
            }
            match self.bucket_folds[bi] {
                FoldKind::Sum | FoldKind::SummedGroup => out[bi] += v,
                FoldKind::Product => out[bi] *= 1.0 + v / 100.0,
            }
        }
        Ok(())
    }

    /// Write raw folds into slots, applying the SummedGroup wrap 1+Σ/100.
    /// `base` is the bucket segment's offset (n_stats + n_conditions).
    fn write_bucket_slots(&self, raw: &[f64], base: usize, slots: &mut [f64]) {
        for (bi, fold) in self.bucket_folds.iter().enumerate() {
            slots[base + bi] = match fold {
                FoldKind::SummedGroup => 1.0 + raw[bi] / 100.0,
                _ => raw[bi],
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toy_def() -> GameDef {
        serde_json::from_str(
            r#"{
              "stats": ["weapon", "power", "crit_chance", "enemy_dr"],
              "conditions": ["enraged"],
              "buckets": { "additive": { "fold": "sum" },
                           "crit_group": { "fold": "summed_group" },
                           "indep": { "fold": "product" } },
              "events": { "crit": { "chance": "crit_chance / 100",
                                     "factor": "1.5 * crit_group" } },
              "pipeline": [
                { "name": "base", "expr": "weapon * (1 + power / 100)" },
                { "name": "hit",
                  "expr": "base * (1 + additive / 100) * event_factors * indep",
                  "branched": true },
                { "name": "dps", "expr": "hit * (1 - enemy_dr / 100)" }
              ],
              "objectives": ["dps"]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn toy_gamedef_compiles() {
        let p = compile(&toy_def()).unwrap();
        assert_eq!(p.objective_names(), vec!["dps"]);
        assert_eq!(p.stat_id("crit_chance"), Some(2));
    }

    #[test]
    fn event_multiplier_is_a_gamer_readable_alias_for_event_factors() {
        let legacy = compile(&toy_def()).unwrap();
        let mut readable_def = toy_def();
        let readable_stage = readable_def.pipeline[1].as_expression_mut().unwrap();
        readable_stage.expr = readable_stage
            .expr
            .replace("event_factors", "event_multiplier");
        let readable = compile(&readable_def).unwrap();

        let mut legacy_scratch = legacy.scratch();
        let mut readable_scratch = readable.scratch();
        assert_eq!(
            legacy
                .evaluate(&toy_build(), &arena(), &mut legacy_scratch)
                .unwrap(),
            readable
                .evaluate(&toy_build(), &arena(), &mut readable_scratch)
                .unwrap()
        );
    }

    #[test]
    fn event_factors_is_illegal_outside_branched_stages() {
        let mut def = toy_def();
        def.pipeline[0].as_expression_mut().unwrap().expr = "weapon * event_factors".into();
        let e = compile(&def).unwrap_err();
        assert!(e.what.contains("event_factors"), "got: {}", e.what);

        let mut def = toy_def();
        def.pipeline[0].as_expression_mut().unwrap().expr = "weapon * event_multiplier".into();
        let e = compile(&def).unwrap_err();
        assert!(e.what.contains("event_multiplier"), "got: {}", e.what);
    }

    #[test]
    fn unknown_names_and_duplicates_are_compile_errors() {
        let mut def = toy_def();
        def.pipeline[2].as_expression_mut().unwrap().expr = "hit * mystery".into();
        assert!(compile(&def).unwrap_err().what.contains("mystery"));

        let mut def = toy_def();
        def.objectives = vec!["nope".into()];
        assert!(compile(&def).unwrap_err().what.contains("nope"));

        let mut def = toy_def();
        def.stats.push("additive".into()); // collides with bucket name
        assert!(compile(&def).unwrap_err().what.contains("duplicate"));
    }

    #[test]
    fn later_stages_see_earlier_ones_but_not_vice_versa() {
        let mut def = toy_def();
        def.pipeline[0].as_expression_mut().unwrap().expr = "dps + 1".into(); // forward reference
        assert!(compile(&def).unwrap_err().what.contains("dps"));
    }

    #[test]
    fn event_factors_name_is_reserved() {
        let mut def = toy_def();
        def.stats.push("event_factors".into());
        assert!(compile(&def).unwrap_err().what.contains("reserved"));
        let mut def = toy_def();
        def.conditions.push("event_factors".into());
        assert!(compile(&def).unwrap_err().what.contains("reserved"));
    }

    #[test]
    fn declared_event_multiplier_shadows_the_new_readable_alias() {
        let mut def = toy_def();
        def.buckets.insert(
            "event_multiplier".into(),
            crate::gamedef::BucketDef {
                fold: FoldKind::Sum,
            },
        );
        def.pipeline[1].as_expression_mut().unwrap().expr = "event_multiplier".into();
        def.objectives = vec!["hit".into()];

        let plan = compile(&def).unwrap();
        let mut build = toy_build();
        build.contributions.push(crate::build::Contribution {
            bucket: "event_multiplier".into(),
            value: 7.0,
            event: None,
            condition: None,
        });
        let mut scratch = plan.scratch();
        let objectives = plan.evaluate(&build, &arena(), &mut scratch).unwrap();
        assert!((objectives[0] - 7.0).abs() < 1e-9, "got {}", objectives[0]);
    }

    #[test]
    fn too_many_events_rejected() {
        let mut def = toy_def();
        for i in 0..9 {
            def.events.insert(
                format!("e{i}"),
                crate::gamedef::EventDef {
                    extra: Default::default(),
                    chance: "0".into(),
                    factor: "1".into(),
                },
            );
        }
        assert!(compile(&def).unwrap_err().what.contains("events"));
    }

    fn toy_build() -> BuildState {
        serde_json::from_str(
            r#"{ "stats": { "weapon": 100.0, "power": 50.0, "crit_chance": 25.0 },
                 "contributions": [
                   { "bucket": "additive", "value": 40.0 },
                   { "bucket": "additive", "value": 30.0, "event": "crit" },
                   { "bucket": "additive", "value": 20.0, "condition": "enraged" },
                   { "bucket": "crit_group", "value": 50.0 },
                   { "bucket": "indep", "value": 10.0 } ] }"#,
        )
        .unwrap()
    }

    fn arena() -> Scenario {
        serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 1,
                   "uptimes": { "enraged": 0.5 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap()
    }

    #[test]
    fn toy_game_hand_worked_single_phase() {
        // base    = 100 × 1.5                       = 150
        // additive (no-crit) = 40 + 20×0.5          = 50   → ×1.5
        // indep   = 1.10 ; crit_group = 1.5
        // crit branch additive = 50 + 30 = 80       → ×1.8
        // event_factors (crit) = 1.5 × 1.5          = 2.25
        // no-crit hit = 150 × 1.5  × 1    × 1.1     = 247.5
        // crit hit    = 150 × 1.8  × 2.25 × 1.1     = 668.25
        // EV = 0.75×247.5 + 0.25×668.25             = 352.6875
        // dps = 352.6875 × (1 − 0.20)               = 282.15
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let objectives = plan.evaluate(&toy_build(), &arena(), &mut scratch).unwrap();
        assert!(
            (objectives[0] - 282.15).abs() < 1e-9,
            "got {}",
            objectives[0]
        );
    }

    #[test]
    fn phase_blending_weights_normalize() {
        // Phase A = arena above (dps 282.15).
        // Phase B: enraged 1.0, dr 0:
        //   additive nc = 60 → ×1.6 ; crit = 90 → ×1.9
        //   nc  = 150×1.6×1.1        = 264
        //   crit= 150×1.9×2.25×1.1   = 705.375
        //   EV  = .75×264 + .25×705.375 = 374.34375 ; dps = 374.34375
        // Weights 60/40 → 0.6×282.15 + 0.4×374.34375 = 319.0275
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [
                  { "name": "a", "weight": 60, "uptimes": { "enraged": 0.5 },
                    "stats": { "enemy_dr": 20.0 } },
                  { "name": "b", "weight": 40, "uptimes": { "enraged": 1.0 } } ] }"#,
        )
        .unwrap();
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let objectives = plan
            .evaluate(&toy_build(), &scenario, &mut scratch)
            .unwrap();
        assert!(
            (objectives[0] - 319.0275).abs() < 1e-9,
            "got {}",
            objectives[0]
        );
    }

    #[test]
    fn unknown_refs_in_build_and_scenario_are_eval_errors() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();

        let mut b = toy_build();
        b.contributions[0].bucket = "nope".into();
        assert!(plan
            .evaluate(&b, &arena(), &mut scratch)
            .unwrap_err()
            .what
            .contains("nope"));

        let mut b = toy_build();
        b.stats.insert("mystery".into(), 1.0);
        assert!(plan
            .evaluate(&b, &arena(), &mut scratch)
            .unwrap_err()
            .what
            .contains("mystery"));

        let bad: Scenario = serde_json::from_str(r#"{ "phases": [] }"#).unwrap();
        assert!(plan
            .evaluate(&toy_build(), &bad, &mut scratch)
            .unwrap_err()
            .what
            .contains("phase"));
    }

    #[test]
    fn three_event_enumeration_hand_worked() {
        // E[factor] per event: e1 .5→2: 1.5 ; e2 .25→3: 1.5 ; e3 .1→5: 1.4
        // Independent ⇒ EV = 100 × 1.5 × 1.5 × 1.4 = 315 exactly (the full
        // 8-branch enumeration must telescope to this product).
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": ["base_v", "c1", "c2", "c3"],
              "events": { "e1": { "chance": "c1 / 100", "factor": "2" },
                          "e2": { "chance": "c2 / 100", "factor": "3" },
                          "e3": { "chance": "c3 / 100", "factor": "5" } },
              "pipeline": [ { "name": "hit", "expr": "base_v * event_factors", "branched": true } ],
              "objectives": ["hit"]
            }"#,
        )
        .unwrap();
        let build: BuildState = serde_json::from_str(
            r#"{ "stats": { "base_v": 100.0, "c1": 50.0, "c2": 25.0, "c3": 10.0 } }"#,
        )
        .unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();
        let obj = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
        assert!((obj[0] - 315.0).abs() < 1e-9, "got {}", obj[0]);
    }

    #[test]
    fn chance_clamps_and_zero_uptime_is_fail_closed() {
        // crit_chance 400 → clamp to 1.0: EV = crit branch only.
        let mut b = toy_build();
        b.stats.insert("crit_chance".into(), 400.0);
        // no-uptime phase: enraged contribution contributes 0.
        let s: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();
        // additive crit = 40 + 30 = 70 → 1.7 ; hit = 150×1.7×2.25×1.1 = 631.125
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let objectives = plan.evaluate(&b, &s, &mut scratch).unwrap();
        assert!(
            (objectives[0] - 631.125).abs() < 1e-9,
            "got {}",
            objectives[0]
        );
    }

    #[test]
    fn negative_individual_phase_weight_is_rejected() {
        // Sum (4) is positive, but phase `a`'s own weight is negative —
        // must be caught per-phase, not just via the aggregate sum check.
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "a", "weight": -1 }, { "name": "b", "weight": 5 } ] }"#,
        )
        .unwrap();
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let err = plan
            .evaluate(&toy_build(), &scenario, &mut scratch)
            .unwrap_err();
        assert!(err.what.contains("weight"), "got: {}", err.what);
    }

    // ------------------------------------------------------------------
    // P8a validation debt (from the 0.3.0 release review): a non-finite
    // number in a build or phase used to sail straight through the folds
    // and come back as `Ok(NaN)` / `Ok(inf)` total damage — the silent
    // wrong answer this crate exists to refuse. Each of the three inputs
    // is rejected with a positioned error, mirroring the existing
    // `Phase.uptimes` "must be finite" style.
    // ------------------------------------------------------------------
    #[test]
    fn non_finite_contribution_value_is_rejected_not_folded() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();

        let mut b = toy_build();
        b.contributions[0].value = f64::NAN; // bucket "additive"
        let e = plan.evaluate(&b, &arena(), &mut scratch).unwrap_err();
        assert!(
            e.what
                .contains("contribution value into bucket `additive` must be finite"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("NaN"), "got: {}", e.what);

        let mut b = toy_build();
        b.contributions[0].value = f64::INFINITY;
        let e = plan.evaluate(&b, &arena(), &mut scratch).unwrap_err();
        assert!(e.what.contains("must be finite"), "got: {}", e.what);
        assert!(e.what.contains("inf"), "got: {}", e.what);
    }

    #[test]
    fn non_finite_build_stat_is_rejected() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let mut b = toy_build();
        b.stats.insert("weapon".into(), f64::NAN);
        let e = plan.evaluate(&b, &arena(), &mut scratch).unwrap_err();
        assert!(
            e.what.contains("build stat `weapon` must be finite"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn non_finite_phase_stat_override_is_rejected() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let mut s = arena();
        s.phases[0].stats.insert("enemy_dr".into(), f64::NAN);
        let e = plan.evaluate(&toy_build(), &s, &mut scratch).unwrap_err();
        assert!(
            e.what
                .contains("phase `arena` stat `enemy_dr` must be finite"),
            "got: {}",
            e.what
        );
    }

    // The single-phase entry point (the sim's per-cast path) applies the
    // same three rejections — it resolves the build fresh per call, so a
    // gap here would be a gap in every `sim::run` evaluation.
    #[test]
    fn non_finite_inputs_are_rejected_on_the_single_phase_path_too() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let phase = arena().phases[0].clone();

        let mut b = toy_build();
        b.contributions[0].value = f64::NAN;
        let e = plan.evaluate_phase(&b, &phase, &mut scratch).unwrap_err();
        assert!(e.what.contains("must be finite"), "got: {}", e.what);

        let mut b = toy_build();
        b.stats.insert("weapon".into(), f64::NAN);
        let e = plan.evaluate_phase(&b, &phase, &mut scratch).unwrap_err();
        assert!(e.what.contains("must be finite"), "got: {}", e.what);

        let mut phase = phase;
        phase.stats.insert("enemy_dr".into(), f64::NAN);
        let e = plan
            .evaluate_phase(&toy_build(), &phase, &mut scratch)
            .unwrap_err();
        assert!(e.what.contains("must be finite"), "got: {}", e.what);
    }

    #[test]
    fn non_finite_uptime_is_rejected() {
        // serde_json can't express NaN, so build the Scenario in Rust.
        let mut s = arena();
        s.phases[0].uptimes.insert("enraged".into(), f64::NAN);
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let err = plan.evaluate(&toy_build(), &s, &mut scratch).unwrap_err();
        assert!(err.what.contains("uptime"), "got: {}", err.what);
    }

    #[test]
    fn product_bucket_negative_contribution_flips_sign() {
        // Single product bucket, one −150 contribution: 1 + (−150)/100 = −0.5.
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": [],
              "buckets": { "indep": { "fold": "product" } },
              "pipeline": [ { "name": "indep_stage", "expr": "indep" } ],
              "objectives": ["indep_stage"]
            }"#,
        )
        .unwrap();
        let build: BuildState = serde_json::from_str(
            r#"{ "contributions": [ { "bucket": "indep", "value": -150.0 } ] }"#,
        )
        .unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();
        let obj = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
        assert!((obj[0] - -0.5).abs() < 1e-9, "got {}", obj[0]);
    }

    #[test]
    fn uptime_clamps_at_both_edges() {
        // Sum bucket with one conditioned +100 contribution: uptime 2.5
        // clamps to 1.0 (objective 100); uptime −3 clamps to 0.0 (objective 0).
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": [],
              "conditions": ["c"],
              "buckets": { "add": { "fold": "sum" } },
              "pipeline": [ { "name": "s", "expr": "add" } ],
              "objectives": ["s"]
            }"#,
        )
        .unwrap();
        let build: BuildState = serde_json::from_str(
            r#"{ "contributions": [ { "bucket": "add", "value": 100.0, "condition": "c" } ] }"#,
        )
        .unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();

        let hi: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1, "uptimes": { "c": 2.5 } } ] }"#,
        )
        .unwrap();
        let obj = plan.evaluate(&build, &hi, &mut scratch).unwrap();
        assert!((obj[0] - 100.0).abs() < 1e-9, "got {}", obj[0]);

        let lo: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1, "uptimes": { "c": -3.0 } } ] }"#,
        )
        .unwrap();
        let obj = plan.evaluate(&build, &lo, &mut scratch).unwrap();
        assert!((obj[0] - 0.0).abs() < 1e-9, "got {}", obj[0]);
    }

    #[test]
    fn branched_stage_with_zero_events_evaluates() {
        // No events defined: single mask (0), weight and factors both the
        // empty product = 1.0.
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": ["base_v"],
              "pipeline": [ { "name": "hit", "expr": "base_v * event_factors", "branched": true } ],
              "objectives": ["hit"]
            }"#,
        )
        .unwrap();
        let build: BuildState =
            serde_json::from_str(r#"{ "stats": { "base_v": 100.0 } }"#).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();
        let obj = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
        assert!((obj[0] - 100.0).abs() < 1e-9, "got {}", obj[0]);
    }

    #[test]
    fn conditions_are_readable_in_expressions() {
        // Uptime is an expression value: 1 + enraged*2 at uptime 0.5 → 2.0.
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["base_v"], "conditions": ["enraged"],
                 "pipeline": [ { "name": "out", "expr": "base_v * (1 + enraged * 2)" } ],
                 "objectives": ["out"] }"#,
        )
        .unwrap();
        let build: BuildState =
            serde_json::from_str(r#"{ "stats": { "base_v": 100.0 } }"#).unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();

        let s: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1, "uptimes": { "enraged": 0.5 } } ] }"#,
        )
        .unwrap();
        let obj = plan.evaluate(&build, &s, &mut scratch).unwrap();
        assert!((obj[0] - 200.0).abs() < 1e-9, "got {}", obj[0]);

        // Missing uptime = 0 (fail-closed): factor collapses to 1.
        let s: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();
        let obj = plan.evaluate(&build, &s, &mut scratch).unwrap();
        assert!((obj[0] - 100.0).abs() < 1e-9, "got {}", obj[0]);
    }

    #[test]
    fn condition_names_join_the_flat_namespace() {
        let mut def = toy_def();
        def.conditions.push("weapon".into()); // collides with a stat
        assert!(compile(&def).unwrap_err().what.contains("duplicate"));
    }

    #[test]
    fn unknown_uptime_keys_are_rejected() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let s: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1, "uptimes": { "enrged": 0.5 } } ] }"#,
        )
        .unwrap();
        let e = plan.evaluate(&toy_build(), &s, &mut scratch).unwrap_err();
        assert!(e.what.contains("enrged"), "got: {}", e.what);
    }

    #[test]
    fn explain_matches_evaluate_and_traces_the_hand_worked_numbers() {
        // Same fixtures/hand numbers as toy_game_hand_worked_single_phase.
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let objectives = plan
            .evaluate(&toy_build(), &arena(), &mut scratch)
            .unwrap()
            .to_vec();

        let mut scratch = plan.scratch();
        let ex = plan.explain(&toy_build(), &arena(), &mut scratch).unwrap();

        assert_eq!(ex.objectives, objectives);
        assert!(
            (ex.objectives[0] - 282.15).abs() < 1e-9,
            "got {:?}",
            ex.objectives
        );

        assert_eq!(ex.phases.len(), 1);
        let p = &ex.phases[0];
        assert!((p.weight - 1.0).abs() < 1e-9);
        assert!(
            p.conditions
                .iter()
                .any(|(n, v)| n == "enraged" && (v - 0.5).abs() < 1e-9),
            "got {:?}",
            p.conditions
        );
        assert!(
            p.buckets
                .iter()
                .any(|(n, v)| n == "additive" && (v - 50.0).abs() < 1e-9),
            "got {:?}",
            p.buckets
        );
        assert!(
            p.stages
                .iter()
                .any(|(n, v)| n == "base" && (v - 150.0).abs() < 1e-9),
            "got {:?}",
            p.stages
        );

        let hit_branches: Vec<&BranchTrace> =
            p.branches.iter().filter(|b| b.stage == "hit").collect();
        assert_eq!(hit_branches.len(), 2);
        let weight_sum: f64 = hit_branches.iter().map(|b| b.weight).sum();
        assert!((weight_sum - 1.0).abs() < 1e-9, "got {weight_sum}");

        let unfired = hit_branches.iter().find(|b| b.fired.is_empty()).unwrap();
        assert!(
            (unfired.weight - 0.75).abs() < 1e-9,
            "got {}",
            unfired.weight
        );
        assert!(
            (unfired.value - 247.5).abs() < 1e-9,
            "got {}",
            unfired.value
        );
        assert!((unfired.event_factors - 1.0).abs() < 1e-9);

        let fired = hit_branches.iter().find(|b| !b.fired.is_empty()).unwrap();
        assert_eq!(fired.fired, vec!["crit".to_string()]);
        assert!((fired.weight - 0.25).abs() < 1e-9, "got {}", fired.weight);
        assert!((fired.value - 668.25).abs() < 1e-9, "got {}", fired.value);
        assert!((fired.event_factors - 2.25).abs() < 1e-9);
    }

    #[test]
    fn explain_with_zero_events_has_a_single_all_unfired_branch_entry() {
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": ["base_v"],
              "pipeline": [ { "name": "hit", "expr": "base_v * event_factors", "branched": true } ],
              "objectives": ["hit"]
            }"#,
        )
        .unwrap();
        let build: BuildState =
            serde_json::from_str(r#"{ "stats": { "base_v": 100.0 } }"#).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();
        let ex = plan.explain(&build, &scenario, &mut scratch).unwrap();
        assert!((ex.objectives[0] - 100.0).abs() < 1e-9);

        let branches = &ex.phases[0].branches;
        assert_eq!(branches.len(), 1, "got {:?}", branches);
        assert!(branches[0].fired.is_empty());
        assert!((branches[0].weight - 1.0).abs() < 1e-9);
        assert!((branches[0].event_factors - 1.0).abs() < 1e-9);
        assert!((branches[0].value - 100.0).abs() < 1e-9);
    }

    #[test]
    fn explain_scalar_only_pipeline_has_no_branches() {
        // conditions_are_readable_in_expressions' def has no branched stage.
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["base_v"], "conditions": ["enraged"],
                 "pipeline": [ { "name": "out", "expr": "base_v * (1 + enraged * 2)" } ],
                 "objectives": ["out"] }"#,
        )
        .unwrap();
        let build: BuildState =
            serde_json::from_str(r#"{ "stats": { "base_v": 100.0 } }"#).unwrap();
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1, "uptimes": { "enraged": 0.5 } } ] }"#,
        )
        .unwrap();
        let plan = compile(&def).unwrap();
        let mut scratch = plan.scratch();
        let ex = plan.explain(&build, &scenario, &mut scratch).unwrap();
        assert!((ex.objectives[0] - 200.0).abs() < 1e-9);
        assert!(
            ex.phases[0].branches.is_empty(),
            "got {:?}",
            ex.phases[0].branches
        );
    }

    // ------------------------------------------------------------------
    // evaluate_phase_sampled: at chance 1.0 (crit_chance clamped to 100),
    // the "crit" event is certain, so the single sampled branch must be
    // EXACTLY the crit branch's own value (668.25 pre-dr, hand-worked in
    // `toy_game_hand_worked_single_phase`'s doc comment) — dps = ×0.8 =
    // 534.6 — and the fired mask must have bit 0 (the only event) set.
    // ------------------------------------------------------------------
    #[test]
    fn evaluate_phase_sampled_at_chance_one_equals_crit_branch_exactly() {
        let mut b = toy_build();
        b.stats.insert("crit_chance".into(), 100.0); // clamps to 1.0
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let mut rng = crate::rng::Pcg32::new(1);
        let phase = arena().phases[0].clone();
        let (objs, mask) = plan
            .evaluate_phase_sampled(&b, &phase, &mut rng, &mut scratch)
            .unwrap();
        assert!((objs[0] - 534.6).abs() < 1e-9, "got {}", objs[0]);
        assert_eq!(mask, 1, "the sole event (\"crit\", bit 0) must be set");
    }

    // ------------------------------------------------------------------
    // At chance 0.0, the "crit" event never fires — the single sampled
    // branch must be EXACTLY the no-crit branch's value (247.5 pre-dr;
    // dps = ×0.8 = 198.0), mask 0.
    // ------------------------------------------------------------------
    #[test]
    fn evaluate_phase_sampled_at_chance_zero_equals_base_branch_exactly() {
        let mut b = toy_build();
        b.stats.insert("crit_chance".into(), 0.0);
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let mut rng = crate::rng::Pcg32::new(1);
        let phase = arena().phases[0].clone();
        let (objs, mask) = plan
            .evaluate_phase_sampled(&b, &phase, &mut rng, &mut scratch)
            .unwrap();
        assert!((objs[0] - 198.0).abs() < 1e-9, "got {}", objs[0]);
        assert_eq!(mask, 0);
    }

    // ------------------------------------------------------------------
    // Statistical convergence: 10_000 independent samples (fixed seed) at
    // the toy game's real 25% crit chance must average within 1% of
    // `evaluate`'s own EV (282.15, the keystone pin) — this is a
    // STATISTICAL assertion (not exact), documented as such; the 1%
    // tolerance at N=10_000 against a two-branch Bernoulli mixture is
    // comfortably wide (binomial std of the branch indicator at p=0.25,
    // n=10_000 is ~0.0043, i.e. ~0.4% relative on the branch-selection
    // frequency — 1% leaves ample margin against test flakiness while
    // still catching a real wiring bug).
    // ------------------------------------------------------------------
    #[test]
    fn evaluate_phase_sampled_ten_thousand_samples_converge_to_evaluate_ev() {
        let plan = compile(&toy_def()).unwrap();
        let build = toy_build();
        let phase = arena().phases[0].clone();
        let mut scratch = plan.scratch();

        let mut sum = 0.0;
        let mut rng = crate::rng::Pcg32::new(12345);
        const N: u32 = 10_000;
        for _ in 0..N {
            let (objs, _mask) = plan
                .evaluate_phase_sampled(&build, &phase, &mut rng, &mut scratch)
                .unwrap();
            sum += objs[0];
        }
        let mean = sum / f64::from(N);

        let ev = 282.15; // the keystone pin, re-derived independently below.
        let mut ev_scratch = plan.scratch();
        let evaluated = plan.evaluate(&build, &arena(), &mut ev_scratch).unwrap()[0];
        assert!((evaluated - ev).abs() < 1e-9, "got {evaluated}");

        let rel_err = (mean - ev).abs() / ev;
        assert!(
            rel_err < 0.01,
            "sampled mean {mean} vs EV {ev}, relative error {rel_err}"
        );
    }
}
