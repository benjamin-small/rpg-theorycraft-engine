//! GameDef → Plan (compile once) and Plan × BuildState × Scenario →
//! EvalResult (hot path). One unified slot array is shared by every
//! compiled expression: [stats | buckets | stages | event_factors].

// BuildState/Scenario are consumed by `evaluate()` (Task 3, not yet
// implemented); the imports are wired up now since they're part of this
// module's declared surface.
#[allow(unused_imports)]
use crate::build::BuildState;
use crate::expr::{compile as compile_expr, ExprError, Program, Symbols};
use crate::gamedef::{FoldKind, GameDef};
#[allow(unused_imports)]
use crate::scenario::Scenario;

/// Hard cap on events: 2^8 = 256 branches per branched stage.
pub const MAX_EVENTS: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct PlanError {
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
        PlanError { what: e.to_string() }
    }
}

// Several fields below are read only by `evaluate()` (Task 3, not yet
// implemented) — compile() populates them now so Task 3 needs no rework.
#[derive(Debug)]
#[allow(dead_code)]
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
    n_buckets: usize,
    n_stages: usize,
}

#[derive(Debug)]
#[allow(dead_code)]
struct CompiledEvent {
    chance: Program,
    factor: Program,
}

#[derive(Debug)]
#[allow(dead_code)]
struct CompiledStage {
    name: String,
    program: Program,
    branched: bool,
}

/// Compile-time symbol table over the unified slot layout. `event_factors`
/// resolves only when `branched` is set (the engine's per-branch slot).
struct StageSymbols<'a> {
    stats: &'a [String],
    buckets: &'a [String],
    prior_stages: &'a [String],
}

impl Symbols for StageSymbols<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        let n_stats = self.stats.len();
        let n_buckets = self.buckets.len();
        if let Some(i) = self.stats.iter().position(|s| s == name) {
            return Some(i as u16);
        }
        if let Some(i) = self.buckets.iter().position(|b| b == name) {
            return Some((n_stats + i) as u16);
        }
        if let Some(i) = self.prior_stages.iter().position(|s| s == name) {
            return Some((n_stats + n_buckets + i) as u16);
        }
        // event_factors lives after ALL stages; prior_stages grows per
        // stage but the slot is fixed using the FULL stage count, patched
        // by compile() via `total_stages`.
        None
    }
}

/// Wrapper that makes `event_factors` resolvable only in branched stages,
/// at the fixed slot after all stages.
struct WithEventFactors<'a> {
    inner: StageSymbols<'a>,
    slot: u16,
    enabled: bool,
}

impl Symbols for WithEventFactors<'_> {
    fn slot(&self, name: &str) -> Option<u16> {
        if name == "event_factors" {
            return self.enabled.then_some(self.slot);
        }
        self.inner.slot(name)
    }
}

pub fn compile(def: &GameDef) -> Result<Plan, PlanError> {
    // One flat namespace: stats, buckets, stages must not collide.
    let mut seen = std::collections::BTreeSet::new();
    let bucket_names: Vec<String> = def.buckets.keys().cloned().collect();
    let stage_names: Vec<String> = def.pipeline.iter().map(|s| s.name.clone()).collect();
    for name in def.stats.iter().chain(&bucket_names).chain(&stage_names) {
        if !seen.insert(name.clone()) {
            return Err(PlanError { what: format!("duplicate name `{name}`") });
        }
    }
    if def.events.len() > MAX_EVENTS {
        return Err(PlanError {
            what: format!("{} events > max {MAX_EVENTS}", def.events.len()),
        });
    }

    let n_stats = def.stats.len();
    let n_buckets = bucket_names.len();
    let n_stages = stage_names.len();
    let event_factors_slot = (n_stats + n_buckets + n_stages) as u16;

    let event_syms = StageSymbols { stats: &def.stats, buckets: &bucket_names, prior_stages: &[] };
    let mut events = Vec::new();
    let mut event_names = Vec::new();
    for (name, ev) in &def.events {
        events.push(CompiledEvent {
            chance: compile_expr(&ev.chance, &event_syms)
                .map_err(|e| PlanError { what: format!("event `{name}` chance: {e}") })?,
            factor: compile_expr(&ev.factor, &event_syms)
                .map_err(|e| PlanError { what: format!("event `{name}` factor: {e}") })?,
        });
        event_names.push(name.clone());
    }

    let mut stages: Vec<CompiledStage> = Vec::new();
    for (i, s) in def.pipeline.iter().enumerate() {
        let syms = WithEventFactors {
            inner: StageSymbols {
                stats: &def.stats,
                buckets: &bucket_names,
                prior_stages: &stage_names[..i],
            },
            slot: event_factors_slot,
            enabled: s.branched,
        };
        let program = compile_expr(&s.expr, &syms)
            .map_err(|e| PlanError { what: format!("stage `{}`: {e}", s.name) })?;
        stages.push(CompiledStage { name: s.name.clone(), program, branched: s.branched });
    }

    if def.objectives.is_empty() {
        return Err(PlanError { what: "no objectives".into() });
    }
    let mut objective_stages = Vec::new();
    for o in &def.objectives {
        let idx = stage_names
            .iter()
            .position(|s| s == o)
            .ok_or_else(|| PlanError { what: format!("unknown objective `{o}`") })?;
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
        n_buckets,
        n_stages,
    })
}

impl Plan {
    pub fn stat_id(&self, name: &str) -> Option<usize> {
        self.stat_names.iter().position(|s| s == name)
    }
    pub fn objective_names(&self) -> Vec<&str> {
        self.objective_stages.iter().map(|&i| self.stages[i].name.as_str()).collect()
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
    fn event_factors_is_illegal_outside_branched_stages() {
        let mut def = toy_def();
        def.pipeline[0].expr = "weapon * event_factors".into();
        let e = compile(&def).unwrap_err();
        assert!(e.what.contains("event_factors"), "got: {}", e.what);
    }

    #[test]
    fn unknown_names_and_duplicates_are_compile_errors() {
        let mut def = toy_def();
        def.pipeline[2].expr = "hit * mystery".into();
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
        def.pipeline[0].expr = "dps + 1".into(); // forward reference
        assert!(compile(&def).unwrap_err().what.contains("dps"));
    }

    #[test]
    fn too_many_events_rejected() {
        let mut def = toy_def();
        for i in 0..9 {
            def.events.insert(
                format!("e{i}"),
                crate::gamedef::EventDef { chance: "0".into(), factor: "1".into() },
            );
        }
        assert!(compile(&def).unwrap_err().what.contains("events"));
    }
}
