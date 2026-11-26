# Slice 6 — Sim fields read Plan objectives (`objective.<name>`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sim timing and gating expressions may read a `Plan` objective (`objective.hit_cycle_time`), evaluated against the measured world at the field's own instant. Cadence then follows the build and the conditions the sim drives (#33 item 2).

**Architecture:**
- **Compile.** `objective.<name>` resolves to a new region of the sim slot array, one slot per Plan objective. A compiled sim program records whether it reads any objective slot.
- **Evaluation.** Before such a program is evaluated, the executor runs ONE deterministic `Plan::evaluate_phase` against the field's world and writes every objective into its slot. Then it evaluates the program as today.
- Programs that read no objective are untouched, so every existing config is byte-identical.

**Tech Stack:** Rust, `rtce` (`crates/rtce`).

**Spec:** `docs/superpowers/specs/2026-11-25-hit-level-casts-design.md` §5.6.

## Global Constraints

- Branch `feat/objective-reads`, cut from `feat/segments` (PR #37, auto-merge on); `main` is merged in once #37 lands, before this PR opens. The PR targets `main` with auto-merge. Never stage `README.md` / `ROADMAP.md`.
- **Legal fields** (spec §5.6):
  - action `cast_time`, segment `duration`, `cooldown`, `cost`, `gain`;
  - buff `duration`;
  - proc `chance`;
  - rotation rule `when`.
- **Compile errors**, positioned and naming the field:
  - in `damage.stats`, at action or segment level (a circular read);
  - in resource `max` / `regen_per_sec` (refold-time fields, not in the spec's list);
  - an unknown objective, which gets a did-you-mean against the Plan's objective names.
- **Value, both modes (spec, verbatim intent):** the DETERMINISTIC `evaluate_phase` value. It is the branch-weighted expected objective, never sampled, and it never consumes RNG draws. An objective downstream of a branched stage therefore reads its expected value *by definition*; document that on the symbol.
- **World (spec):**
  - Inside an action's or a segment's field (`cast_time`, segment `duration`, `cooldown`, `cost`, `gain`), the world is the effective build WITH that action's or segment's `damage.stats` overlay, evaluated at the same instant, plus the effective phase.
  - A segmented action's action-level fields have no action overlay, so they use the plain effective build.
  - A utility action (no `damage`) uses the plain effective build.
  - Everywhere else (buff `duration`, proc `chance`, rule `when`) the world is the plain effective build plus the effective phase at that instant.
- **Instant:** each field's existing documented instant (the `NumOrExpr` instants table; `cast_time` at cast start; segment `duration` at segment start; `cost` at cast start AND at every affordability check). It is a read at an instant, not a fixed point: a buff the sim later applies changes the NEXT read.
- **Cost:** one zero-allocation `evaluate_phase` per read, borrowing `scratch.eval`. The action-field overlay reuses the existing per-cast overlay construction (one `BuildState` clone), which must be documented. No memoization in this slice.
- **Unchanged:** existing programs that read no objective take exactly today's path. No extra Plan evaluation, no RNG change, so the bytes are identical.
- TDD red-first: capture RED before implementing; for a pin that passes on arrival, record a failing mutation instead. Every pin is hand-worked, with the contrast in the commit message. P8f: the new symbol's rustdoc and the instants table state DEFAULT, INSTANT and INTERACTIONS; every legal field gets a discriminating pin.
- **Standing gates before the PR:**
  - `cargo test --workspace`
  - clippy `-D warnings`
  - fmt `--check`
  - `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`
  - `examples/diablo4_rotation.rs` byte-identical to `main`
  - poe2 AND d4 against this tree with the `--config patch.crates-io.rtce.path=…` override from CLAUDE.md, restoring each `Cargo.lock` afterwards:
    - poe2: workspace green and `rtce_parity` 96 passed;
    - d4: workspace green and `eval` `total_dps` `17574.299999999996`.

## Shared test fixture

`haste_plan()`:
- stats `["aps", "dmg"]`
- conditions `["fast"]`
- pipeline:
  - `hct = 1 / (aps * (1 + fast))` (non-branched)
  - `hit = dmg`
- objectives `["hit", "hct"]`

The build is `aps 1, dmg 100`. The `hct` objective is 1.0 with `fast` = 0 and 0.5 with `fast` = 1. Buff `haste` has conditions `{ fast: 1 }` and duration 10.

## Review Focus

1. **A field reading an objective while another proc or buff changes the world at the same instant** sees the world as of its own evaluation, not a stale cached one. (Task 2, the haste cadence pin.)
2. **`cost` with an objective read** is re-evaluated at every affordability check, so a wake time that depends on it stays consistent. (Task 2.)
3. **MC byte identity:** a seeded MC run whose field reads an objective that equals a literal is byte-identical to the literal version, so no RNG draw was added. (Task 2.)
4. **An objective that cannot be evaluated at an instant** (for example a stage that divides by zero, giving a non-finite value) fails closed with the field's existing instant error, not a NaN cadence. (Task 2.)
5. **Did-you-mean** for an unknown objective name. (Task 1.)

---

### Task 1: Compile `objective.<name>` (no behaviour change)

**Files:**
- Modify: `crates/rtce/src/sim/compile.rs`:
  - `SimSymbols::slot` (~378) resolves `objective.<name>` to `objective_base + objective_index`, a new region appended after the segment slots;
  - a recording wrapper reports whether a program read any objective slot;
  - field legality and errors.
- Modify: the compiled types, which carry the flag.
  - Introduce `pub struct SimProgram { pub program: Program, pub reads_objectives: bool }`, and use it for `CompiledAction::cast_time`, proc `chance`, rule `when`, and `CompiledValue::Expr`.
  - Update every construction and use site mechanically.
  - `CompiledResource::max` / `regen_per_sec` stay `Program`, because objective reads are rejected there.
- Test: `crates/rtce/src/sim/compile.rs` tests.

**Interfaces:**
- Produces:
  - `SimProgram`;
  - the objective slot region, whose width equals the Plan's objective count;
  - `SimPlan`/`SimSymbols` know `objective_base`.
- Task 2 writes the slots. Until then they stay 0 and nothing reads them in existing configs.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test] fn objective_reads_compile_in_every_legal_field()          // cast_time, segment duration, cooldown, cost, gain, buff duration, proc chance, rule when → reads_objectives == true; a field without one → false
#[test] fn an_objective_read_in_damage_stats_is_a_compile_error()  // action and segment level; msg names action (+segment), "damage.stats", "objective"
#[test] fn an_objective_read_in_a_resource_is_a_compile_error()    // max and regen_per_sec
#[test] fn an_unknown_objective_suggests_the_nearest()             // "objective.hct_" → did-you-mean "hct"
```

- [ ] **Step 2: Capture RED.** Run `cargo test -p rtce --lib objective_read` against the unchanged engine and paste the output.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: GREEN.** Run the full suite; no existing test may change. Mutation: make `reads_objectives` always false. `objective_reads_compile_in_every_legal_field` must fail. Restore.
- [ ] **Step 5: Commit.** `"Compile objective.<name> sim reads (legal-field set, positioned errors; no behaviour change)"`

### Task 2: Evaluate objective reads at each field's instant

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - a new private `fn fill_objective_slots(&mut self, world: ObjectiveWorld) -> Result<(), PlanError>`;
  - `enum ObjectiveWorld { Plain, Part(usize /*action*/, Part) }`;
  - call it immediately before every `SimProgram` evaluation whose `reads_objectives` is true:
    - `begin_cast` (`cast_time`, `cooldown`, `cost`);
    - `start_segment` (`duration`);
    - `apply_gain`;
    - `apply_buff` (`duration`);
    - `roll_procs_*` (`chance`);
    - `attempt_decision` (`when`, and `cost` via `afford`).
- Test: `crates/rtce/src/sim/exec.rs` `mod objective_reads`.

**Interfaces:**
- Consumes: Task 1's `SimProgram` and `objective_base`.
- Produces: `fill_objective_slots`.
  - It evaluates `Plan::evaluate_phase` once.
  - It uses the overlay build for `Part`, via the existing overlay construction, or the plain effective build for `Plain`. The phase is the effective phase.
  - It writes all objective slots.
  - A non-finite objective value is left for the field's own finite check to report at its instant.

- [ ] **Step 1: Write the failing tests** (EV unless stated). Hand derivations:

```rust
// strike: cast_time "objective.hct", damage, effects [apply_buff haste]; weight 5.5:
//   cast 1 starts t=0 with fast 0 → hct 1 → completes 1.0, applies haste;
//   casts 2.. start with fast 1 → hct 0.5 → complete 1.5, 2.0, …, 5.5 (drained) → 9 more.
//   → casts 10, total 1000. Contrast cast_time "1" → casts 5, total 500.
#[test] fn cast_time_follows_an_objective_the_sim_drives()

// segment duration "objective.hct" with the segment's damage.stats { aps: 2 } overlay → 0.5 s,
// vs the plain build (aps 1) → 1.0 s. Single segment, weight 2 → casts 4 vs 2 (if the world were
// the plain build).
#[test] fn a_segment_duration_reads_its_own_overlay_world()

// One pin per remaining legal field, each reading an objective whose value differs from the
// field's literal twin (hand-derive each; one test or a table test):
//   cooldown "objective.hct" · cost { mana: "10 * objective.hct" } · gain · buff duration
//   "objective.hct" · proc chance "objective.hct / 2" · rule when "objective.hct < 0.75"
#[test] fn every_legal_field_reads_its_objective_world()

// MC byte identity: cast_time "objective.hct" with haste never applied (hct = 1) vs cast_time "1",
// a crit-free plan, MC iterations 50 seed 9: serde_json::to_string(report) equal.
#[test] fn objective_reads_consume_no_rng_draws()

// aps 0 → hct = 1/0 = inf → cast_time read fails closed with the existing cast_time error.
#[test] fn a_non_finite_objective_fails_closed_at_the_field()
```

- [ ] **Step 2: Capture RED** against Task 1's head (every read is 0 today) and paste it.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: GREEN + mutations.**
  - Use the plain world for action/segment fields: the overlay pin must fail (4 vs 2).
  - Skip the fill before `attempt_decision`: the rule `when` pin must fail.
  - Restore after each.
- [ ] **Step 5: Commit.** `"Evaluate objective reads at each field's instant (cadence 10 casts vs 5; overlay 4 vs 2)"`

### Task 3: Docs, CHANGELOG

**Files:**
- Modify: `crates/rtce/src/sim/mod.rs`, the symbol-space list. Add `objective.<name>`. Rewrite the "Pipeline STAGES and buckets are deliberately absent" paragraph: stages and buckets are still absent, objectives are now readable in the legal fields, and the value is deterministic and expected.
- Modify: `crates/rtce/src/simdef.rs`, the `NumOrExpr` instants table. Add an "objective reads" row and column note giving the world each field uses.
- Modify: `CHANGELOG.md` (`## [Unreleased]` → Added: `objective.<name>` in the legal fields, closing #33 item 2. Upgrade note: the compiled sim types now hold `SimProgram`, which affects only consumers that inspect `SimPlan` internals).

- [ ] **Step 1:** Make the edits.
- [ ] **Step 2:** Run `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` and the full suite.
- [ ] **Step 3: Commit.** `"Document objective reads (symbol space, instants table, CHANGELOG)"`

### Task 4: Standing gates

- [ ] Byte identity: `diablo4_rotation` against `main`, using a worktree.
- [ ] The engine gates.
- [ ] poe2 and d4 via the override, per Global Constraints, restoring each `Cargo.lock`.
- [ ] Commit the plan file (no push).
