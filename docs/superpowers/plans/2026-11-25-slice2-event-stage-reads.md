# Slice 2 — Event chances read pre-branch stages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let `EventDef::chance` and `factor` read pipeline stages that come before the first branched stage, so a multi-stage crit-chance chain can drive the `crit` event directly (#33 item 1).

**Architecture:** `plan::compile` compiles events against `stage_names[..prefix]`, where `prefix` is the index of the first branched stage (or the whole pipeline when there is none), and stores `prefix` on `Plan`. `Plan::evaluate*` already has those slots filled when a branched stage computes chances, so evaluation needs no change. `Plan::crit_chance` (the sim's EV crit weight) is the one reader that runs outside the stage loop: it must now evaluate the prefix stages after `fill_prefix_slots`, through a helper shared with the stage loop.

**Tech Stack:** Rust, serde, `rtce` (`crates/rtce`), `rtce-runner` (lexicon text).

**Spec:** `docs/superpowers/specs/2026-11-25-hit-level-casts-design.md` §5.7 second bullet, §6 slice 2.

## Global Constraints

- Branch `feat/event-stage-reads`, cut from `feat/on-non-crit` (PR #34 is open; this PR stacks on it, and its description says "depends on #34"). Never stage `README.md` / `ROADMAP.md`.
- The rule, verbatim from the spec: events "compile against the stages BEFORE the FIRST branched stage of the pipeline (those are branch-independent); a reference to any later stage is a compile error naming the event and stage. `Plan::crit_chance` evaluates that pipeline prefix instead of only `fill_prefix_slots`. No change for configs that read no stage."
- Ruling carried into this plan: a pipeline with NO branched stage makes every stage readable by events (the prefix is the whole pipeline). Events then matter only to the sim's crit weight, which evaluates that prefix.
- TDD red-first; pins mutation-checked (break the input or the code path, watch the specific pin fail, restore, record the contrast in the commit message).
- Zero allocation on evaluation hot paths (`crit_chance` is called per cast in EV).
- P8f docs: `EventDef::chance`/`factor` rustdoc states the DEFAULT (no stage reads needed; existing configs unchanged), the EVALUATION INSTANT (once per branched stage, after the prefix stages, against base slots), and INTERACTIONS (a `factor` sees this branch's refolded BUCKETS but the prefix STAGES' base values, because prefix stages are computed before any branching; the sim's EV `on_crit`/`on_non_crit` weight reads the same prefix).
- Standing gates before the PR: `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all --check` · `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` · `examples/diablo4_rotation.rs` output byte-identical to `main` · `../d4-theory-crafting`: `cargo test --workspace` green, `cargo run -q -p d4-core --example eval` prints `17574.299999999996` · `../poe2-theory-crafting`: `cargo test --workspace` green, `cargo test -p poe2-core --test rtce_parity` → `96 passed`.

## Review Focus

1. **Stale stage slots in `crit_chance`.** `EvalScratch` is reused across calls. A `crit_chance` that skipped the prefix would read whatever a previous evaluation left in those slots and look correct in a single-call test. (Test in Task 2.)
2. **An event naming the first branched stage itself, or a stage after it.** Both must be compile errors naming the event, the stage, and the first branched stage. (Test in Task 1.)
3. **A `factor` reading a prefix stage** sees the base value, not a branch-refolded one, and says so in the docs. (Test in Task 1.)
4. **A pipeline with no branched stage** makes every stage event-readable, and the sim's crit weight evaluates all of them. (Test in Task 2.)
5. **Solve and recurrence stages in the prefix** evaluate in `crit_chance` exactly as in the stage loop (shared helper, not a copy). (Test in Task 2.)

---

### Task 1: Compile events against the pre-branch prefix

**Files:**
- Modify: `crates/rtce/src/plan.rs` (event compilation ~line 408; `Plan` struct fields ~line 70; tests module near `toy_def` ~line 1865)
- Modify: `crates/rtce/src/gamedef.rs` (`EventDef::chance`/`factor` rustdoc ~line 72)

**Interfaces:**
- Produces: `Plan::event_stage_prefix: usize` (private field): the number of leading stages events may read, which is the index of the first branched stage, or `stages.len()` if none. Task 2 reads it.

- [ ] **Step 1: Write the failing tests** (in `plan.rs` tests)

Fixture `prefix_def()`: stats `["crit_base"]`, no buckets needed beyond what the parser requires, event `crit { chance: "eff_crit / 100", factor: "2" }`, pipeline `[{ name: "eff_crit", expr: "crit_base * 2" }, { name: "hit", expr: "100 * event_factors", branched: true }]`, objectives `["hit"]`. Build `crit_base: 10`, one phase of weight 1.

```rust
// eff_crit = 20 → p = 0.2 → hit = 100 × (0.8·1 + 0.2·2) = 120.
// Contrast (input mutation): eff_crit expr "crit_base * 1" → p = 0.1 → 110.
#[test]
fn an_event_chance_reads_a_stage_before_the_first_branched_stage() {
    assert!(close(objective("hit"), 120.0));
}

#[test]
fn an_event_reading_the_first_branched_stage_is_a_compile_error() {
    // chance: "hit / 1000"
    let msg = compile(&def).unwrap_err().what;
    assert!(msg.contains("event `crit`") && msg.contains("`hit`"), "{msg}");
    assert!(msg.contains("first branched stage"), "{msg}");
}

#[test]
fn an_event_reading_a_stage_after_the_first_branched_stage_is_a_compile_error() {
    // add stage { name: "dps", expr: "hit" } after hit; chance: "dps / 1000"
    // same three assertions, naming `dps` and the first branched stage `hit`
}

// factor: "1 + eff_crit / 10" (= 3 at eff_crit 20), chance "0.5":
// hit = 100 × (0.5·1 + 0.5·3) = 200.
#[test]
fn an_event_factor_reads_a_prefix_stage() { assert!(close(objective("hit"), 200.0)); }

// No branched stage: pipeline [eff_crit, total = eff_crit + 1], event chance
// "total / 100" compiles (every stage readable).
#[test]
fn without_a_branched_stage_events_may_read_every_stage() { assert!(compile(&def).is_ok()); }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rtce --lib event_` (and the `without_a_branched_stage` name).
Expected: the three "reads" tests fail at `compile` with the existing generic unknown-identifier error for `eff_crit` / `total`. The two error tests fail on the `"first branched stage"` assertion: the error exists, but it doesn't name the reason.

- [ ] **Step 3: Implement**

Compute `event_stage_prefix` = position of the first `StageDef::Expression` with `branched: true`, else `def.pipeline.len()`, before compiling events. Compile `chance` and `factor` with `prior_stages: &stage_names[..event_stage_prefix]`. For the error, wrap the event symbols so that a miss on a name in `stage_names[event_stage_prefix..]` is reported as `event `<e>` chance: stage `<s>` is not readable here: events may read only stages before the first branched stage `<b>``. Use `factor` instead of `chance` when the factor is at fault. Keep the generic unknown-identifier error for every other miss. Store the field on `Plan`. Update the `EventDef::chance`/`factor` rustdoc per Global Constraints.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rtce --lib` → all PASS (existing event tests unchanged).
Mutation: set the `eff_crit` expr to `crit_base * 1` → `an_event_chance_reads…` fails `110 vs 120`. Restore.

- [ ] **Step 5: Commit**

```bash
git add crates/rtce/src/plan.rs crates/rtce/src/gamedef.rs
git commit -m "Events read stages before the first branched stage (crit via eff_crit: 120, contrast 110)"
```

### Task 2: `Plan::crit_chance` evaluates the prefix; docs, guide, lexicon, CHANGELOG

**Files:**
- Modify: `crates/rtce/src/plan.rs` (`eval_phase` stage loop ~line 1186; `crit_chance` ~line 1738)
- Modify: `crates/rtce/src/sim/exec.rs` tests (sim-level pin)
- Modify: `docs/guide/03-events-and-branches.md` (the "`events` block" paragraph ~line 44)
- Modify: `crates/rtce-runner/src/lib.rs` (lexicon `events.<name>.chance` entry, line ~33)
- Modify: `CHANGELOG.md` (`## [Unreleased]` → `### Added`)

**Interfaces:**
- Consumes: `Plan::event_stage_prefix` (Task 1).
- Produces: `fn eval_unbranched_stage(&self, si: usize, scratch: &mut EvalScratch) -> Result<(), PlanError>` (private). It evaluates stage `si`, which is not branched: an expression, solve, or recurrence. The result goes into its output slot in `scratch.slots`. `eval_phase` calls it for every non-branched stage, and `crit_chance` calls it for `0..event_stage_prefix`. A branched stage passed in is a `debug_assert!` failure.

- [ ] **Step 1: Write the failing tests**

In `plan.rs` tests (prefix_def from Task 1, `crit_base` 10 → p 0.2):
```rust
#[test]
fn crit_chance_evaluates_prefix_stages() {
    assert!(close(plan.crit_chance(&build, &phase, &mut fresh_scratch)?, 0.2));
}

// Stale-slot guard: evaluate build A (crit_base 40 → 0.8) with the SAME
// scratch, then crit_chance for build B (crit_base 10) must be 0.2, not 0.8.
#[test]
fn crit_chance_does_not_read_stale_stage_slots() { /* 0.2 */ }

// Prefix holds a solve or recurrence stage feeding eff_crit (reuse an existing
// small solve/recurrence fixture from plan.rs tests). crit_chance equals the
// chance implied by the stage values Plan::explain reports for the same build.
#[test]
fn crit_chance_evaluates_solve_and_recurrence_prefix_stages() { /* equality */ }

// No branched stage: pipeline [eff_crit, total = eff_crit + 5],
// chance "total / 100", crit_base 10 → 0.25.
#[test]
fn crit_chance_without_a_branched_stage_evaluates_the_whole_pipeline() { /* 0.25 */ }
```
In `sim/exec.rs` tests: the `ev_on_crit_weights_by_crit_probability` shape on a plan whose crit chance is `eff_crit / 100`, with `eff_crit = crit_base * 2` and `crit_base` 25, so p = 0.5. An `on_crit` proc at chance 1 and icd 0 over 10 hits gives **5** fires. The contrast is chance `crit_base / 100`, which gives p 0.25 and 2 fires.
```rust
#[test]
fn ev_on_crit_weight_reads_a_stage_derived_crit_chance() { assert_eq!(ev.proc_counts["crit_proc"], 5); }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rtce --lib crit_chance` and `cargo test -p rtce --lib stage_derived`
Expected: `crit_chance_evaluates_prefix_stages` returns `0.0`, because the prefix slots are never written on fresh scratch. The stale-slot test returns `0.8`. The sim pin gives `0` fires, not 5.

- [ ] **Step 3: Implement**

Extract `eval_unbranched_stage` from `eval_phase`'s loop: the solve, recurrence and non-branched expression arms. Keep trace behavior and results byte-identical. Make `eval_phase` call it. In `crit_chance`, after `fill_prefix_slots`, run it for `si in 0..self.event_stage_prefix`, then evaluate the chance as today. Update the `crit_chance` rustdoc to say it evaluates the pre-branch prefix.

Text:
- **Guide paragraph:** "Both are ordinary expressions over the same flat namespace as everything else — stats, conditions, buckets, and any stage that comes before the first branched stage."
- **Lexicon `events.<name>.chance` meaning:** append "It may read stats, conditions, buckets and stages before the first branched stage."
- **CHANGELOG Added:** "Event `chance`/`factor` may read pipeline stages before the first branched stage (#33). A later stage is a compile error naming it. The sim's EV crit weight evaluates the same prefix. Configs that read no stage are unchanged."

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rtce --lib` → all PASS.
Mutation: drop the prefix loop from `crit_chance`. Then `crit_chance_does_not_read_stale_stage_slots` fails `0.8 vs 0.2`, and `ev_on_crit_weight_reads_a_stage_derived_crit_chance` fails `0 vs 5`. Restore.

- [ ] **Step 5: Commit**

```bash
git add crates/rtce/src/plan.rs crates/rtce/src/sim/exec.rs docs/guide/03-events-and-branches.md crates/rtce-runner/src/lib.rs CHANGELOG.md
git commit -m "crit_chance evaluates the pre-branch prefix (EV on_crit 5 fires at stage-derived p=0.5, contrast 2)"
```

### Task 3: Standing gates and consumers

**Files:** none new.

- [ ] **Step 1: Byte-identity.** `git worktree add ../rtce-main main`. Run `cargo run -q -p rtce --example diablo4_rotation > ../rtce-d4rot-main.txt` inside it, and `> ../rtce-d4rot-branch.txt` here. `diff` the two files, which must show no output. Then `git worktree remove ../rtce-main` and delete both txt files.
- [ ] **Step 2: Engine gates.** Run `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check && RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`. All must be clean.
- [ ] **Step 3: Consumers** (from each repo root):
  - `../d4-theory-crafting`: `cargo test --workspace` green, and `cargo run -q -p d4-core --example eval | grep total_dps` shows `17574.299999999996`.
  - `../poe2-theory-crafting`: `cargo test --workspace` green, and `cargo test -p poe2-core --test rtce_parity` gives `96 passed`.
- [ ] **Step 4: Commit the plan file** (no push; the controller opens the PR after the final review).

```bash
git add docs/superpowers/plans/2026-11-25-slice2-event-stage-reads.md
git commit -m "Plan: slice 2 (event stage reads)"
```
