//! GameDef → Plan (compile once) and Plan × BuildState × Scenario →
//! EvalResult (hot path). One unified slot array is shared by every
//! compiled expression: [stats | buckets | stages | event_factors].

use crate::build::BuildState;
use crate::expr::{compile as compile_expr, ExprError, Program, Symbols};
use crate::gamedef::{FoldKind, GameDef};
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
    n_buckets: usize,
    n_stages: usize,
}

#[derive(Debug)]
struct CompiledEvent {
    chance: Program,
    factor: Program,
}

#[derive(Debug)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct EvalResult {
    /// One value per Plan objective, in objective order.
    pub objectives: Vec<f64>,
}

/// Preallocated evaluation buffers — `evaluate` never allocates.
pub struct EvalScratch {
    slots: Vec<f64>,
    branch_slots: Vec<f64>,
    base_bucket_raw: Vec<f64>,
    branch_bucket_raw: Vec<f64>,
    objectives: Vec<f64>,
    stat_base: Vec<f64>,
}

impl Plan {
    pub fn scratch(&self) -> EvalScratch {
        let n = self.n_stats + self.n_buckets + self.n_stages + 1;
        EvalScratch {
            slots: vec![0.0; n],
            branch_slots: vec![0.0; n],
            base_bucket_raw: vec![0.0; self.n_buckets],
            branch_bucket_raw: vec![0.0; self.n_buckets],
            objectives: vec![0.0; self.objective_stages.len()],
            stat_base: vec![0.0; self.n_stats],
        }
    }

    pub fn evaluate(
        &self,
        build: &BuildState,
        scenario: &Scenario,
        scratch: &mut EvalScratch,
    ) -> Result<EvalResult, PlanError> {
        if scenario.phases.is_empty() {
            return Err(PlanError { what: "scenario has no phases".into() });
        }
        let weight_sum: f64 = scenario.phases.iter().map(|p| p.weight).sum();
        // Fail-closed on NaN too: `weight_sum > 0.0` is false for NaN, so
        // negating it would accept NaN; check both sides explicitly instead
        // (clippy::neg_cmp_op_on_partial_ord).
        if weight_sum.is_nan() || weight_sum <= 0.0 {
            return Err(PlanError { what: "phase weights must sum > 0".into() });
        }

        // Resolve + validate build ONCE: stats and contribution tags.
        for slot in scratch.stat_base.iter_mut() {
            *slot = 0.0;
        }
        for (name, v) in &build.stats {
            let i = self
                .stat_id(name)
                .ok_or_else(|| PlanError { what: format!("unknown stat `{name}`") })?;
            scratch.stat_base[i] = *v;
        }
        // Contribution tags validate per call (cheap linear scans over
        // small registries; index resolution caches are a P5 concern).
        for c in &build.contributions {
            if !self.bucket_names.iter().any(|b| b == &c.bucket) {
                return Err(PlanError { what: format!("unknown bucket `{}`", c.bucket) });
            }
            if let Some(e) = &c.event {
                if !self.event_names.iter().any(|n| n == e) {
                    return Err(PlanError { what: format!("unknown event `{e}`") });
                }
            }
            if let Some(cd) = &c.condition {
                if !self.condition_names.iter().any(|n| n == cd) {
                    return Err(PlanError { what: format!("unknown condition `{cd}`") });
                }
            }
        }

        for o in scratch.objectives.iter_mut() {
            *o = 0.0;
        }

        for phase in &scenario.phases {
            let w = phase.weight / weight_sum;

            // Stats: build values + phase overrides.
            let n_stats = self.n_stats;
            scratch.slots[..n_stats].copy_from_slice(&scratch.stat_base);
            for (name, v) in &phase.stats {
                let i = self
                    .stat_id(name)
                    .ok_or_else(|| PlanError { what: format!("unknown stat `{name}`") })?;
                scratch.slots[i] = *v;
            }

            // Base bucket raw sums/products: event-gated contribs EXCLUDED,
            // condition-tagged scaled by uptime (missing = 0 — fail-closed).
            self.fold_buckets(build, phase, None, &mut scratch.base_bucket_raw)?;
            self.write_bucket_slots(&scratch.base_bucket_raw, n_stats, &mut scratch.slots);

            // Stages in order.
            for (si, stage) in self.stages.iter().enumerate() {
                let out_slot = n_stats + self.n_buckets + si;
                if !stage.branched {
                    scratch.slots[out_slot] = stage.program.eval(&scratch.slots);
                    continue;
                }
                // Branch enumeration over 2^n events.
                let n_ev = self.events.len();
                let mut ev_acc = 0.0;
                for mask in 0u32..(1 << n_ev) {
                    // Weight = Π fired ? p : 1-p (chances from PHASE slots).
                    let mut weight = 1.0;
                    for (ei, ev) in self.events.iter().enumerate() {
                        let p = ev.chance.eval(&scratch.slots).clamp(0.0, 1.0);
                        weight *= if mask & (1 << ei) != 0 { p } else { 1.0 - p };
                    }
                    if weight == 0.0 {
                        continue;
                    }
                    // Branch slots: buckets recomputed with fired-event
                    // contributions included; event_factors = Π factors.
                    scratch.branch_slots.copy_from_slice(&scratch.slots);
                    self.fold_buckets(build, phase, Some(mask), &mut scratch.branch_bucket_raw)?;
                    self.write_bucket_slots(
                        &scratch.branch_bucket_raw,
                        n_stats,
                        &mut scratch.branch_slots,
                    );
                    let mut factors = 1.0;
                    for (ei, ev) in self.events.iter().enumerate() {
                        if mask & (1 << ei) != 0 {
                            factors *= ev.factor.eval(&scratch.branch_slots);
                        }
                    }
                    let ef_slot = n_stats + self.n_buckets + self.n_stages;
                    scratch.branch_slots[ef_slot] = factors;
                    ev_acc += weight * stage.program.eval(&scratch.branch_slots);
                }
                scratch.slots[out_slot] = ev_acc;
            }

            for (oi, &si) in self.objective_stages.iter().enumerate() {
                scratch.objectives[oi] += w * scratch.slots[n_stats + self.n_buckets + si];
            }
        }

        Ok(EvalResult { objectives: scratch.objectives.clone() })
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
            let bi = self.bucket_names.iter().position(|b| b == &c.bucket).unwrap();
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
    fn write_bucket_slots(&self, raw: &[f64], n_stats: usize, slots: &mut [f64]) {
        for (bi, fold) in self.bucket_folds.iter().enumerate() {
            slots[n_stats + bi] = match fold {
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
        let r = plan.evaluate(&toy_build(), &arena(), &mut scratch).unwrap();
        assert!((r.objectives[0] - 282.15).abs() < 1e-9, "got {}", r.objectives[0]);
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
        let r = plan.evaluate(&toy_build(), &scenario, &mut scratch).unwrap();
        assert!((r.objectives[0] - 319.0275).abs() < 1e-9, "got {}", r.objectives[0]);
    }

    #[test]
    fn unknown_refs_in_build_and_scenario_are_eval_errors() {
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();

        let mut b = toy_build();
        b.contributions[0].bucket = "nope".into();
        assert!(plan.evaluate(&b, &arena(), &mut scratch).unwrap_err().what.contains("nope"));

        let mut b = toy_build();
        b.stats.insert("mystery".into(), 1.0);
        assert!(plan.evaluate(&b, &arena(), &mut scratch).unwrap_err().what.contains("mystery"));

        let bad: Scenario =
            serde_json::from_str(r#"{ "phases": [] }"#).unwrap();
        assert!(plan
            .evaluate(&toy_build(), &bad, &mut scratch)
            .unwrap_err()
            .what
            .contains("phase"));
    }

    #[test]
    fn chance_clamps_and_zero_uptime_is_fail_closed() {
        // crit_chance 400 → clamp to 1.0: EV = crit branch only.
        let mut b = toy_build();
        b.stats.insert("crit_chance".into(), 400.0);
        // no-uptime phase: enraged contribution contributes 0.
        let s: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#,
        )
        .unwrap();
        // additive crit = 40 + 30 = 70 → 1.7 ; hit = 150×1.7×2.25×1.1 = 631.125
        let plan = compile(&toy_def()).unwrap();
        let mut scratch = plan.scratch();
        let r = plan.evaluate(&b, &s, &mut scratch).unwrap();
        assert!((r.objectives[0] - 631.125).abs() < 1e-9, "got {}", r.objectives[0]);
    }
}
