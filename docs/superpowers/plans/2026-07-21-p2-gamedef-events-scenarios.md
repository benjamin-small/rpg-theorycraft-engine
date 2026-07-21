# P2 — GameDef, Buckets, Events, Pipeline, Scenarios Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The engine becomes a damage model: a GameDef (stats, buckets, events, pipeline) compiles into a `Plan`; a `BuildState` (stat values + tagged contributions) evaluates against a `Scenario` (phases with uptime fractions) into per-scenario objectives — proven by a toy game with two hand-worked pinned numbers (282.15 and 374.34375).

**Architecture:** One unified slot array `[stats | buckets | stages | event_factors]` shared by every compiled expression. Scalar stages evaluate once per phase; a `branched` stage triggers 2ⁿ event-branch enumeration (buckets recomputed with event-gated contributions, fired events' factors multiplied into the `event_factors` slot) and stores the probability-weighted EV. Condition-tagged contributions scale by the phase's uptime fraction (exact for each sum's EV; factor-independence assumed across products — the standard calculator approximation). Phase objectives blend by normalized weights.

**Key semantics (settled here, tests enforce them):**
- Fold kinds: `sum` (Σ — the additive pool is just a sum the pipeline wraps as `1 + x/100`), `summed_group` (bucket value = `1 + Σv/100` — the S13 rule), `product` (bucket value = `Π(1 + v/100)` — independent multipliers).
- Contribution tags: `event` (counts only in branches where that event fired), `condition` (value × phase uptime; uptime defaults to **0.0** when a phase omits the condition — fail-closed).
- Event `chance` expressions clamp to [0,1] engine-side; `factor` expressions evaluate with branch-recomputed buckets. Max 8 events (256 branches) — compile error beyond.
- `event_factors` identifier is available ONLY in `branched` stages (compile error elsewhere); equals Π of fired events' factors (1.0 when none fire).
- Phase weights normalize over their sum (sum ≤ 0 is a compile error).
- Hot path: `evaluate(&self, build, scenario, &mut EvalScratch)` — scratch preallocated via `plan.scratch()`, no allocation inside evaluate.

**Tech Stack:** Rust 2021. `rtce` gains `serde`(derive)+`serde_json` as real dependencies (GameDef/Scenario are JSON). TDD red-first; every task commits.

---

### Task 1: serde types — GameDef, BuildState, Scenario

**Files:**
- Modify: `crates/rtce/Cargo.toml` (add deps)
- Modify: `crates/rtce/src/lib.rs` (new modules)
- Create: `crates/rtce/src/gamedef.rs`
- Create: `crates/rtce/src/build.rs`
- Create: `crates/rtce/src/scenario.rs`

- [ ] **Step 1: Add dependencies**

In `crates/rtce/Cargo.toml` `[dependencies]`:
```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Write the three type files WITH their parse tests (tests first mentally — the types are pure serde, so RED here = the test file failing to compile until types exist; acceptable for pure data tasks)**

`crates/rtce/src/gamedef.rs`:
```rust
//! GameDef — the game's ALGORITHM as configuration: stat registry, bucket
//! fold declarations, probabilistic events, and the pipeline of stages.
//! Compiled once by `plan::compile`; never touched on the hot path.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameDef {
    /// Stat registry: names become slot offsets, in this order.
    pub stats: Vec<String>,
    /// Condition registry (uptime-gated contribution tags).
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketDef>,
    #[serde(default)]
    pub events: BTreeMap<String, EventDef>,
    pub pipeline: Vec<StageDef>,
    /// Stage names exported as EvalResult objectives.
    pub objectives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketDef {
    pub fold: FoldKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldKind {
    /// Σ of member values (the additive pool is a sum the pipeline wraps).
    Sum,
    /// 1 + Σv/100 — same-type multipliers SUM before multiplying.
    SummedGroup,
    /// Π(1 + v/100) — independent multipliers each their own factor.
    Product,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDef {
    /// Expression over stats; engine clamps the result to [0, 1].
    pub chance: String,
    /// Expression over stats/buckets (branch-recomputed); multiplied into
    /// `event_factors` when this event fires.
    pub factor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDef {
    pub name: String,
    pub expr: String,
    /// A branched stage is evaluated per event-branch and stores the
    /// probability-weighted EV. `event_factors` is only legal here.
    #[serde(default)]
    pub branched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamedef_parses_from_json() {
        let g: GameDef = serde_json::from_str(
            r#"{
              "stats": ["weapon", "crit_chance"],
              "conditions": ["enraged"],
              "buckets": { "additive": { "fold": "sum" },
                           "crit_group": { "fold": "summed_group" },
                           "indep": { "fold": "product" } },
              "events": { "crit": { "chance": "crit_chance / 100",
                                     "factor": "1.5 * crit_group" } },
              "pipeline": [
                { "name": "base", "expr": "weapon" },
                { "name": "hit", "expr": "base * event_factors", "branched": true }
              ],
              "objectives": ["hit"]
            }"#,
        )
        .unwrap();
        assert_eq!(g.stats, vec!["weapon", "crit_chance"]);
        assert_eq!(g.buckets["crit_group"].fold, FoldKind::SummedGroup);
        assert!(g.pipeline[1].branched && !g.pipeline[0].branched);
        assert_eq!(g.objectives, vec!["hit"]);
    }
}
```

`crates/rtce/src/build.rs`:
```rust
//! BuildState — ONE candidate: raw stat values plus tagged contributions
//! into buckets. This is the only artifact that changes per permutation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildState {
    /// Values for the GameDef stat registry, by name (missing = 0.0).
    #[serde(default)]
    pub stats: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub contributions: Vec<Contribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contribution {
    pub bucket: String,
    pub value: f64,
    /// Counts only in branches where this event fired.
    #[serde(default)]
    pub event: Option<String>,
    /// Value scales by the phase's uptime for this condition (default 0).
    #[serde(default)]
    pub condition: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buildstate_parses_with_tags() {
        let b: BuildState = serde_json::from_str(
            r#"{ "stats": { "weapon": 100.0 },
                 "contributions": [
                   { "bucket": "additive", "value": 40.0 },
                   { "bucket": "additive", "value": 30.0, "event": "crit" },
                   { "bucket": "additive", "value": 20.0, "condition": "enraged" } ] }"#,
        )
        .unwrap();
        assert_eq!(b.stats["weapon"], 100.0);
        assert_eq!(b.contributions[1].event.as_deref(), Some("crit"));
        assert_eq!(b.contributions[2].condition.as_deref(), Some("enraged"));
    }
}
```

`crates/rtce/src/scenario.rs`:
```rust
//! Scenario (playbook) — THE FIGHT being asked about, as configuration:
//! weighted phases with stat overrides and condition-uptime fractions.
//! Level-1 semantics (weighted-phase blending); the Level-2 timeline
//! simulator will share this schema (see design spec).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scenario {
    pub phases: Vec<Phase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phase {
    pub name: String,
    /// Relative weight (e.g. seconds); normalized over the scenario's sum.
    pub weight: f64,
    /// Condition → uptime fraction in [0,1]. Missing condition = 0.0.
    #[serde(default)]
    pub uptimes: BTreeMap<String, f64>,
    /// Stat overrides for this phase (enemy DR, target count, …).
    #[serde(default)]
    pub stats: BTreeMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_parses() {
        let s: Scenario = serde_json::from_str(
            r#"{ "phases": [
                  { "name": "boss", "weight": 60,
                    "uptimes": { "enraged": 0.5 },
                    "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();
        assert_eq!(s.phases[0].uptimes["enraged"], 0.5);
        assert_eq!(s.phases[0].stats["enemy_dr"], 20.0);
    }
}
```

In `crates/rtce/src/lib.rs` add after `pub mod expr;`:
```rust
pub mod build;
pub mod gamedef;
pub mod scenario;
```

- [ ] **Step 3: Run and verify green**

Run: `cargo test -p rtce`
Expected: previous 15 unit tests + 3 new = 18 passed (plus golden separately).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "P2: serde types — GameDef (stats/buckets/events/pipeline), BuildState contributions, Scenario phases"
```

---

### Task 2: Plan compilation

**Files:**
- Create: `crates/rtce/src/plan.rs`
- Modify: `crates/rtce/src/lib.rs` (add `pub mod plan;`)

- [ ] **Step 1: Write the failing compile tests + stubs**

`crates/rtce/src/plan.rs` — full skeleton with `compile` returning `todo!()` initially; tests included:

```rust
//! GameDef → Plan (compile once) and Plan × BuildState × Scenario →
//! EvalResult (hot path). One unified slot array is shared by every
//! compiled expression: [stats | buckets | stages | event_factors].

use crate::build::BuildState;
use crate::expr::{compile as compile_expr, ExprError, Program, Symbols};
use crate::gamedef::{FoldKind, GameDef};
use crate::scenario::Scenario;
use std::collections::BTreeMap;

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

struct CompiledEvent {
    chance: Program,
    factor: Program,
}

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
    branched: bool,
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

pub fn compile(def: &GameDef) -> Result<Plan, PlanError> {
    todo!("Task 2 Step 3")
}

impl Plan {
    pub fn stat_id(&self, name: &str) -> Option<usize> {
        self.stat_names.iter().position(|s| s == name)
    }
    pub fn objective_names(&self) -> Vec<&str> {
        self.objective_stages.iter().map(|&i| self.stages[i].name.as_str()).collect()
    }
}
```

NOTE for the implementer on `event_factors`: the cleanest implementation is
a wrapper symbol table used at stage-compile time:

```rust
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
```

`compile()` must:
1. Reject duplicate stat names, bucket names, stage names, and stat/bucket/stage name collisions (one flat namespace): `PlanError { what: format!("duplicate name `{name}`") }`.
2. Reject > MAX_EVENTS events.
3. Compile each event's `chance` and `factor` with a symbol table of stats+buckets (chance typically uses stats; factor may use buckets) — NOT stages, NOT event_factors.
4. Compile each stage IN ORDER with stats + buckets + PRIOR stage names + (`event_factors` iff `branched`), slot for event_factors = n_stats + n_buckets + total_stage_count.
5. Resolve objectives to stage indexes (unknown objective = error; empty objectives = error "no objectives").
6. Record bucket fold kinds in bucket-name order.

Tests (same file):
```rust
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
```

Add `pub mod plan;` to lib.rs.

- [ ] **Step 2: RED run** — `cargo test -p rtce plan` fails on todo!.

- [ ] **Step 3: Implement compile()** per the numbered requirements above. Suggested body:

```rust
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

    let event_syms = StageSymbols {
        stats: &def.stats,
        buckets: &bucket_names,
        prior_stages: &[],
        branched: false,
    };
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
                branched: s.branched,
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
```

(Note: `StageSymbols.branched` field becomes unused with the wrapper —
remove it from StageSymbols if clippy complains; keep the wrapper as the
single source of event_factors resolution.)

- [ ] **Step 4: Green** — `cargo test -p rtce plan` → 5 passed; whole crate green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P2: Plan compilation — unified slot layout, staged symbol scoping, fail-closed name/limit checks"
```

---

### Task 3: Evaluation — folds, scalar stages, event branches, phase blending

**Files:**
- Modify: `crates/rtce/src/plan.rs`

- [ ] **Step 1: Write the failing evaluation tests (hand-worked arithmetic in comments)**

Append to the plan.rs test module:

```rust
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
```

Add the API stubs to plan.rs (so the tests compile but fail on todo!):

```rust
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
        todo!("Task 3 Step 3")
    }
}
```

- [ ] **Step 2: RED** — `cargo test -p rtce plan` — 4 new tests fail on todo!.

- [ ] **Step 3: Implement evaluate()**

Semantics (implement exactly; allocation only via `scratch` and the final
`EvalResult`):

```rust
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
        if !(weight_sum > 0.0) {
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
```

(If the borrow checker objects to `scratch.slots[..n_stats].copy_from_slice(&scratch.stat_base)` — two disjoint fields of the same struct are fine; if a method split causes issues, destructure `let EvalScratch { slots, stat_base, .. } = scratch;`.)

- [ ] **Step 4: GREEN** — all 4 evaluation tests pass; hand-worked 282.15, 319.0275, 631.125 confirmed. Whole workspace green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P2: evaluate — folds, event-branch EV, uptime scaling, phase blending (toy 282.15 by hand)"
```

---

### Task 4: Golden fixtures — the toy game end-to-end through JSON

**Files:**
- Create: `crates/rtce/tests/fixtures/toy/gamedef.json`
- Create: `crates/rtce/tests/fixtures/toy/build.json`
- Create: `crates/rtce/tests/fixtures/toy/scenario_arena.json`
- Create: `crates/rtce/tests/fixtures/toy/scenario_dummy.json`
- Create: `crates/rtce/tests/toy_game.rs`

- [ ] **Step 1: Write the fixture files**

`gamedef.json` — exactly the `toy_def()` JSON from Task 2's tests, pretty-printed, plus `"_source": "hand-authored toy game 2026-07-21 — P2 gate"`. NOTE: GameDef has no `_source` field; serde ignores unknown fields by default only with `#[serde(deny_unknown_fields)]` absent — it is absent, so this passes through harmlessly.

`build.json` — exactly `toy_build()`'s JSON.

`scenario_arena.json` — the single-phase arena (enraged 0.5, dr 20).

`scenario_dummy.json`:
```json
{ "phases": [ { "name": "dummy", "weight": 1, "uptimes": { "enraged": 1.0 } } ] }
```

- [ ] **Step 2: Write the integration test**

`crates/rtce/tests/toy_game.rs`:
```rust
//! The P2 gate: the toy game end-to-end from JSON files — GameDef compiles,
//! BuildState evaluates against TWO scenarios (playbooks), objectives pinned
//! to hand-worked numbers (arithmetic in plan.rs unit tests).

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile;
use rtce::scenario::Scenario;
use rtce_testkit::assert_close;
use std::path::PathBuf;

fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/toy").join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

#[test]
fn toy_game_two_playbooks_pinned() {
    let def: GameDef = load("gamedef.json");
    let build: BuildState = load("build.json");
    let plan = compile(&def).unwrap();
    let mut scratch = plan.scratch();

    let arena: Scenario = load("scenario_arena.json");
    let r = plan.evaluate(&build, &arena, &mut scratch).unwrap();
    assert_close(r.objectives[0], 282.15, 1e-9, "arena dps");

    let dummy: Scenario = load("scenario_dummy.json");
    let r = plan.evaluate(&build, &dummy, &mut scratch).unwrap();
    assert_close(r.objectives[0], 374.34375, 1e-9, "dummy dps");
}
```

- [ ] **Step 3: Run** — `cargo test -p rtce --test toy_game` green.

- [ ] **Step 4: Mutation checks (mandatory, two axes)**

(a) Edit `build.json`: change the crit_group contribution 50.0 → 60.0; run; VERIFY the arena pin FAILS (record the line); restore.
(b) Edit `scenario_arena.json`: change enraged uptime 0.5 → 0.6; run; VERIFY failure; restore. Confirm green after restoration.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "P2 gate: toy game two playbooks pinned — arena 282.15 / dummy 374.34375, mutation-proven both axes"
```

---

### Task 5: Close out P2

**Files:**
- Modify: `docs/superpowers/specs/2026-07-21-rtce-design.md`

- [ ] **Step 1: Append to Done-since**

```markdown
- 2026-07-21 — P2 complete: GameDef/BuildState/Scenario serde tiers; Plan
  compilation (unified slot layout [stats|buckets|stages|event_factors],
  staged symbol scoping, flat-namespace + MAX_EVENTS=8 fail-closed checks);
  evaluate() — three fold kinds, 2ⁿ event-branch EV with branch-recomputed
  buckets, condition-uptime scaling (missing uptime = 0, fail-closed),
  normalized phase blending, scratch-buffer zero-alloc hot path. Toy game
  gate: two playbooks pinned end-to-end from JSON (arena 282.15 / dummy
  374.34375), mutation-proven on both the build and scenario axes.
```

- [ ] **Step 2: Full green** — `cargo test --workspace`; also `cargo clippy --workspace --all-targets` clean.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "P2 complete: the engine is a damage model — toy playbooks 282.15 / 374.34375"
```
