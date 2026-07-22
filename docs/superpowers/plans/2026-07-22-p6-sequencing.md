# P6 — Sequencing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Checkbox steps. Spec: `docs/superpowers/specs/2026-07-22-p6-sequencing-design.md` — read it first; its semantics are normative.

**Goal:** the timeline simulator — SimDef + Rotation config, discrete-event executor with configurable EV/MC modes, computed uptimes, `SimReport` — ending in a D4 rotation example and an 0.2.0 release.

**Architecture:** see spec. One stepper; the EV/MC difference is a single substitution point (`Plan::evaluate_sampled` + exact-vs-accumulator procs). Everything TDD red-first; every pinned number hand-worked in a comment; clippy `-D warnings` + fmt stay clean; every task commits.

---

### Task 1 (P6a): expression predicates

**Files:** `crates/rtce/src/expr/lexer.rs`, `parser.rs`, `compiler.rs` (+ mod.rs if exports change)

- [ ] **Step 1 — RED tests** (compiler tests, verbatim):
```rust
    #[test]
    fn comparisons_and_boolean_functions() {
        let s = syms(&["a", "b"]);
        let e = |src: &str, slots: &[f64]| compile(src, &s).unwrap().eval(slots);
        // Comparisons return exactly 0/1.
        assert_eq!(e("a > b", &[3.0, 2.0]), 1.0);
        assert_eq!(e("a > b", &[2.0, 3.0]), 0.0);
        assert_eq!(e("a >= b", &[2.0, 2.0]), 1.0);
        assert_eq!(e("a < b", &[2.0, 3.0]), 1.0);
        assert_eq!(e("a <= b", &[3.0, 2.0]), 0.0);
        assert_eq!(e("a == b", &[2.0, 2.0]), 1.0);
        assert_eq!(e("a != b", &[2.0, 2.0]), 0.0);
        // Precedence: arithmetic binds tighter than comparison.
        assert_eq!(e("a + 1 > b * 2", &[3.0, 2.0]), 0.0); // 4 > 4 → 0
        // Boolean functions: strict 0/1 out, nonzero-truthy in.
        assert_eq!(e("and(a, b)", &[1.0, 0.0]), 0.0);
        assert_eq!(e("and(a, b)", &[2.0, -1.0]), 1.0);
        assert_eq!(e("or(a, b)", &[0.0, 2.0]), 1.0);
        assert_eq!(e("or(a, b)", &[0.0, 0.0]), 0.0);
        assert_eq!(e("not(a)", &[0.0]), 1.0);
        assert_eq!(e("not(a)", &[3.0]), 0.0);
        // Composability: the P6 rotation shape.
        assert_eq!(e("and(a >= 40, not(b))", &[40.0, 0.0]), 1.0);
    }
```
- [ ] **Step 2 — implement**: lexer gains two-char/one-char comparison tokens (reuse the pattern from d4's importer if helpful — but this codebase already lexes `!`-less; add `> < >= <= == !=`); parser gains a comparison level BETWEEN `expr`(add) and the top: `pred := add (cmpop add)?` (single comparison, no chaining — a second cmpop at the same level is a positioned error "chained comparison"); `Func` gains `And`/`Or`/`Not` (arity 2/2/1); `Op` gains `Gt, Lt, Ge, Le, Eq, Ne, And, Or, Not` with eval semantics: comparisons `(l op r) as 0/1`; `And` = `(l != 0 && r != 0) as 0/1`; `Or`, `Not` likewise. `simulate_depth` arms updated. Docs on the module updated (truthiness: nonzero; booleans normalize to 0/1).
- [ ] **Step 3 — green + gates + commit** `"P6a: expression predicates — comparisons + and/or/not, single-comparison rule"`.

### Task 2 (P6b): SimDef + Rotation schemas and compilation

**Files:** new `crates/rtce/src/simdef.rs` (serde types), new `crates/rtce/src/sim/mod.rs` + `crates/rtce/src/sim/compile.rs`; `lib.rs` gains `pub mod simdef; pub mod sim;`

Serde types EXACTLY per the spec's JSON examples: `SimDef { resources: BTreeMap<String, ResourceDef>, actions: BTreeMap<String, ActionDef>, buffs: BTreeMap<String, BuffDef>, procs: BTreeMap<String, ProcDef>, damage_objective: String }`, `ResourceDef { max: String, regen_per_sec: String }`, `ActionDef { cast_time: String, cooldown: f64, cost: BTreeMap<String, f64>, gain: BTreeMap<String, f64>, damage: Option<ActionDamage> }`, `ActionDamage { stats: BTreeMap<String, f64> }`, `BuffDef { duration: f64, contributions: Vec<Contribution>, conditions: BTreeMap<String, f64>, tick_objective: Option<String> }`, `ProcDef { trigger: Trigger (on_cast|on_hit|on_crit, snake_case), chance: String, icd: f64, apply_buff: Option<String>, cast_action: Option<String> }` — all `#[serde(default)]` where sensible, rotation `Rotation { rules: Vec<Rule> }`, `Rule { action: String, when: Option<String> }`.

`sim::compile(plan: &Plan, simdef: &SimDef, rotation: &Rotation) -> Result<SimPlan, PlanError>` builds the extended symbol table (spec: stats + conditions + `time`, `duration`, resource names, `cooldown.<action>`, `buff.<buff>`, `buff_remaining.<buff>`, `casts.<action>`) laid out as one sim-slot array appended after the Plan's stat/condition space (document the layout), and compiles every expression (resource max/regen, cast_times, proc chances, rule whens). Fail-closed checks, each with a RED test: unknown action in a rule; unknown buff/action in a proc effect (and exactly one of apply_buff/cast_action set); unknown resource in cost/gain; `damage_objective` not a Plan objective; `tick_objective` not a Plan objective; sim names colliding with stats/conditions (flat namespace extends); reserved words `time`/`duration`.

- [ ] RED tests (schema parse round-trip of the spec's exact JSON; each fail-closed case) → implement → gates → commit `"P6b: SimDef/Rotation compile — sim symbol space, fail-closed references"`.

### Task 3 (P6c): EV executor — skeleton, resource, buffs

**Files:** new `crates/rtce/src/sim/exec.rs`, `crates/rtce/src/sim/report.rs`

`sim::run(plan, sim_plan, build, scenario, Mode::Expected, &mut SimScratch) -> Result<SimReport, PlanError>`. Discrete-event queue per spec (BinaryHeap of (time, seq, Event) — the `seq` tiebreaker makes ordering deterministic). Decision loop per spec: hard gates then `when != 0`, first wins; ineligible-by-resource computes the earliest affordability wake time; nothing eligible ever → advance to next event; queue empty and t < duration → idle to duration (report will show it). Effective-build fold is EVENT-DRIVEN (recompute contributions only when the buff set changes). Casts complete → accumulate `damage_objective × hits` (hits from action override stats, default 1). Buffs: refresh-on-reapply (reset duration; document). `tick_objective` buffs accrue `objective_value × active_seconds` (integrate on state change and expiry). Phase boundaries swap stat overrides + scenario uptimes at Σ-weight boundaries. `SimReport` per spec (per-phase + total; per-action casts/damage/share; computed buff/condition uptimes; resource time-capped/time-starved; proc counts (0 for now)).

Tests, all hand-worked in comments, all RED-first (toy-game base):
- [ ] **Keystone**: one action, cast_time "1", no cost, damage stats = toy defaults; arena phase weight 10 → 10 casts; `report.total.dps == plan.evaluate(...)'s dps objective` EXACTLY (282.15).
- [ ] **Resource starvation**: spender cost 50 mana, regen 10/s, max 100, start full, cast_time 1: casts at t=0,1,5,10,15 → exactly 5 casts in a 20s phase (hand-worked cadence in the comment); dps = 5×hit/20; report shows starved time > 0.
- [ ] **Computed buff uptime**: buff duration 4 applied on cast of a cooldown-10 utility action (+ a spammable filler): applications at t=0,10 → uptime exactly 0.4 in 20s; while active, its contribution moves the filler's damage (pin both windows' per-hit values).
- [ ] **Waiting is modeled**: a rotation whose only rule has `when: "buff.x == 1"` and nothing applies x → 0 casts, dps 0, no infinite loop (duration reached).
- [ ] **Condition precedence**: a buff setting `conditions: {"vulnerable": 1.0}` WINS over the scenario's static 0.4 uptime while active, reverts on expiry (spec rule) — pin the two per-hit values and the report's computed vulnerable uptime for a hand-worked window layout.
- [ ] Gates + commit `"P6c: EV executor — keystone 282.15 agrees with Level-1; starvation and uptime pinned"`.

### Task 4 (P6d): procs, sampling, Monte Carlo

**Files:** `crates/rtce/src/sim/exec.rs`, `crates/rtce/src/plan.rs` (evaluate_sampled), new `crates/rtce/src/rng.rs`

- [ ] `rng.rs`: a small seeded PCG32 (`new(seed: u64)`, `next_f64() -> [0,1)`), zero deps, 2-3 unit tests (determinism, range).
- [ ] `Plan::evaluate_sampled(&self, build, scenario, rng, &mut EvalScratch) -> Result<&[f64], PlanError>`: same internal `run` path, new mode: each branched stage picks ONE mask (sample each event independently by its chance) and uses that branch's value; expose which events fired (needed for on_crit) via a small out-param or scratch field. RED: seeded sample over the toy crit case at chance 1.0 must equal the crit branch value; chance 0.0 the base; a 10k-sample mean within 1% of `evaluate`'s EV (fixed seed, documented statistical test).
- [ ] EV procs — accumulator per spec: per qualifying hit `acc += chance`; fires when `acc >= 1` (acc -= 1) outside ICD. RED pin: chance 0.3, 1 hit/s, icd 0, 10 hits → exactly 3 fires at the hand-worked hit indices.
- [ ] MC mode: `Mode::MonteCarlo { iterations, seed }` — N full runs with per-iteration seeds derived from the master seed; procs roll exactly (`rng < chance`, ICD respected); on_crit fires from sampled crits. `SimReport` gains `distribution: Option<Distribution { mean, std, p10, p50, p90 }>` over dps. RED: same seed twice → identical report (assert_eq on serialized JSON); EV-vs-MC convergence (crit-only toy case, N=10_000: |mc.mean − ev.dps| / ev.dps < 0.02, fixed seed).
- [ ] Gates + commit `"P6d: procs (accumulator) + evaluate_sampled + Monte Carlo — deterministic, convergent"`.

### Task 5 (P6e): D4 slice example, docs, release 0.2.0

- [ ] `crates/rtce/examples/diablo4_rotation.rs`: the committed d4 gamedef + an inline SimDef slice (fireball spender 40 mana / firebolt generator +12, mana 100 regen 5/s, a frost_nova utility applying a 4s `vuln_window` buff on a 10s cooldown, one on_crit proc) + a priority rotation; run EV mode 60s dummy playbook, print the SimReport table (casts, computed vuln uptime, resource starvation), then MC mode N=1000 printing mean/p10/p90. Hand-worked pins for the EV dps and the computed uptime (do the arithmetic in comments; if the cadence is too gnarly to hand-derive exactly, SIMPLIFY THE SIMDEF until it isn't — the pin discipline is the point). Assert pins; add to CI.
- [ ] Docs: crate-level doc + README section "Sequencing: from average to timeline" (three-fidelity table from the spec; scoped honestly — the D4 SimDef is a demonstration slice, not the game); rustdoc for every new public item (missing_docs stays clean); CHANGELOG 0.2.0 entry; ROADMAP updated.
- [ ] Version bumps: rtce 0.2.0 (new API, 0.x minor); rtce-testkit unchanged unless touched; rtce's dev-dep on testkit gains `version = "0.1.0"` alongside path (now valid — testkit is live). `cargo publish --dry-run` both crates clean. Do NOT publish in this task — publishing is the coordinator's explicit final step after review.
- [ ] Gates + commit `"P6e: diablo4_rotation example + docs — sequencing ships at 0.2.0 (dry-run clean)"`.

**Final gate (coordinator):** standing-reviewer round on the whole P6 diff → fixes → APPROVED → publish 0.2.0 → push.
