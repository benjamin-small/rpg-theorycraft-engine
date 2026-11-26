# Slice 3 — Per-hit crit sampling (`crit_rolls`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Monte Carlo rolls crit independently for each hit of a cast when the action's crit sampling resolves to `per_hit`, procs react to each hit's own crit roll, and EV weights change so the two modes still agree (#33 item 3).

**Architecture:** A new `ActionDef::crit_rolls: Option<ProcRolls>` resolves at `sim::compile` to `action.crit_rolls ?? defaults.proc_rolls` (whose own default stays `per_cast`) and is stored on `CompiledAction`.

Under a resolved `per_hit`:
- **Monte Carlo.** `complete_cast` draws `n` independent `evaluate_phase_sampled` samples (`hits = 1` each). It sums them for damage and counts the crit hits. The proc roll paths get that count instead of one shared crit bit. All of a cast's hits land at one instant with one `chance`, so the count is enough; order cannot matter.
- **EV.** The crit-gated weights switch to the independent-hit distribution.

Under a resolved `per_cast` every code path is today's.

**Tech Stack:** Rust, serde, `rtce` (`crates/rtce`).

**Spec:** `docs/superpowers/specs/2026-11-25-hit-level-casts-design.md` §5.5 and §6 slice 3.

## Global Constraints

- Branch `feat/per-hit-crit` cut from `main` (6708a83). The PR targets `main` with auto-merge on. Never stage `README.md` / `ROADMAP.md`.
- **Resolution, verbatim from the spec:** `action.crit_rolls ?? defaults.proc_rolls ?? per_cast`. `ProcDef::rolls` is unchanged and still governs only how many rolls a proc is presented.
- **Semantics under a resolved `per_hit` in `Mode::MonteCarlo` (spec):**
  - each hit is its own `evaluate_phase_sampled` draw with `hits = 1`, and the damage credited is the SUM;
  - per-hit procs roll once per hit against that hit's own mask;
  - a `per_cast` hit-trigger proc gates `on_crit` on ANY hit crit and `on_non_crit` on ANY hit not crit.
  - Under a resolved `per_cast`: one mask per cast, exactly as today.
- **EV under a resolved `per_hit` (spec).** Let `p` be the cast's crit chance and `n` the measured hits.
  - A `per_cast` `on_crit` proc's weight is `1 − (1 − p)^n`; for `on_non_crit` it is `1 − p^n`.
  - A `per_hit` proc with `icd > 0` fires with `1 − (1 − q)^n`, where `q = chance × p` (× `1 − p` for `on_non_crit`). That is ONE accumulation per cast; clamp `q` to `[0, 1]`.
  - A `per_hit` proc with `icd: 0` is unchanged: `n` accumulations of `chance × p` (or `× (1 − p)`).
  - `on_cast` and `on_hit` are untouched in both modes.
- **Who sees a number move:** only actions that resolve to `per_hit` AND have a `"crit"` event AND `n > 1`. With `n = 1`, or with no events in the plan, the per-hit path draws exactly what the per-cast path draws, so it is byte-identical; that is pinned. No example or consumer sets `proc_rolls` (verified 2026-11-26).
- **Whole hit count:** a resolved `per_hit` damaging action whose measured `hits_per_use` is not a whole number `≥ 0`, or is over `PER_HIT_ROLL_LIMIT` (10 000), is a run error in BOTH modes. The error names the action, says `crit_rolls` resolves to `per_hit`, and gives the measured value and the measured-at instant. It uses the same `HITS_WHOLE_EPSILON` rule as `proc_roll_count`. `n = 0`: zero damage, no samples, no crit-gated rolls.
- **Out of scope:** proc-fired free casts. They keep their existing unsampled (expected) damage path, so state it in the `crit_rolls` rustdoc as an interaction, and leave them unchanged.
- TDD red-first. Every pin is hand-worked and mutation-checked, with the contrast in the commit message. P8f docs: the `crit_rolls` rustdoc states DEFAULT (absent = inherit), EVALUATION INSTANT (the cast's measured hit count at its `Measure` instant), and INTERACTIONS (`defaults.proc_rolls`, `ProcDef::rolls`, each trigger, `icd`, `n = 1`, no `"crit"` event, free casts).
- Zero allocation on the run loop: count crit hits, never store per-hit masks.
- Standing gates before the PR: `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all --check` · `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` · `examples/diablo4_rotation.rs` output byte-identical to `main` · `../d4-theory-crafting`: `cargo test --workspace` green, and `cargo run -q -p d4-core --example eval | grep total_dps` shows `17574.299999999996` · `../poe2-theory-crafting`: `cargo test --workspace` green, and `cargo test -p poe2-core --test rtce_parity` gives `96 passed`.

## Review Focus

1. **`n = 1` and no-event plans resolved `per_hit`**: the whole report must be byte-identical (EV and MC) to the `per_cast` run. (Test in Task 2 and Task 3.)
2. **The two `rolls` axes crossing.** A `per_hit` proc on a `per_cast`-sampled action keeps today's shared-mask behaviour, and a `per_cast` proc on a `per_hit`-sampled action gates on any-hit. (Tests in Tasks 2 and 3.)
3. **Fractional `hits_per_use` with no procs at all** still fails closed under a resolved `per_hit`, because damage sampling needs a whole count. (Test in Task 2.)
4. **Existing tests that set `defaults.proc_rolls: "per_hit"`** with a `"crit"` event and `n > 1` change numbers. Each one is re-pinned under the new distribution, with a `crit_rolls: "per_cast"` twin keeping the old number, and listed in the report. (Task 3.)
5. **ICD at one instant under independent hits.** At most one fire per cast. EV's `1 − (1 − q)^n` must not be multiplied by `n`. (Test in Task 3.)

---

### Task 1: `crit_rolls` config field and its compile-time resolution

**Files:**
- Modify: `crates/rtce/src/simdef.rs` (`ActionDef` ~line 549 and `KNOWN_KEYS`; `ProcRolls` docs ~line 300–372)
- Modify: `crates/rtce/src/sim/compile.rs` (`CompiledAction` ~line 115; action compilation, mirroring `rolls: p.rolls.unwrap_or(simdef.defaults.proc_rolls)` at ~line 927)
- Modify: every `ActionDef { … }` struct literal in `crates/rtce/src/sim/exec.rs`, `simdef.rs` and `compile.rs` tests, adding `crit_rolls: None`.

**Interfaces:**
- Produces:
  - `pub crit_rolls: Option<ProcRolls>` on `ActionDef`, serde `"crit_rolls"`, `#[serde(default, skip_serializing_if = "Option::is_none")]`.
  - `pub crit_rolls: ProcRolls` on `CompiledAction`, the resolved value. Tasks 2 and 3 read `self.sim_plan.actions[action].crit_rolls`.

- [ ] **Step 1: Write the failing tests** (in `sim/compile.rs` tests, JSON fixtures)

```rust
// The four resolution cells, one compile each, asserting CompiledAction.crit_rolls:
// crit_rolls absent × defaults absent          → PerCast
// crit_rolls absent × defaults.proc_rolls per_hit → PerHit
// crit_rolls per_cast × defaults per_hit       → PerCast
// crit_rolls per_hit × defaults absent         → PerHit
#[test]
fn crit_rolls_inherits_defaults_proc_rolls_and_the_action_override_wins() { … }

// Serde: `"crit_rolls": "per_hit"` parses; an ActionDef without it re-serializes without the key.
#[test]
fn crit_rolls_parses_and_is_omitted_when_absent() { … }
```

- [ ] **Step 2: Run to verify they fail.** Run `cargo test -p rtce --lib crit_rolls`. Expected: compile error, `no field crit_rolls`.
- [ ] **Step 3: Implement.** Add the field and its full P8f rustdoc (see Global Constraints). Add it to `KNOWN_KEYS`. Resolve it in compile. Add `crit_rolls: None` to every struct literal.
- [ ] **Step 4: Run to verify they pass.** Run `cargo test -p rtce --lib`, which must show all passing with no number changes. Mutation check: resolve with `.unwrap_or(ProcRolls::PerCast)`, ignoring defaults. The second cell must fail. Restore.
- [ ] **Step 5: Commit.** `git commit -m "Add ActionDef::crit_rolls (inherits defaults.proc_rolls; 4 resolution cells pinned)"` (stage the changed source files by path).

### Task 2: Monte Carlo per-hit sampling

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - `complete_cast` MC branch, ~line 2410
  - `eval_action_damage_sampled`, ~line 2671
  - `roll_procs_mc`, ~line 2917
  - `proc_roll_count`, ~line 2826, reused for the whole-count rule
- Test: `crates/rtce/src/sim/exec.rs` `mod proc_rolls` (helpers `crit_plan()` with `cc` as P(crit) and factor 1.5, `nova_simdef`, which is 5 hits and 1 s casts, plus `NOVA_ROT`, `dummy`, `mc_json`, `ev_json`)

**Interfaces:**
- Consumes: `CompiledAction::crit_rolls` (Task 1).
- Produces, all private:
  - `enum CritOutcome { Shared(bool), Counted { crit_hits: u64, hits: u64 } }`. It is passed to `roll_procs_mc` in place of the `qualifies: bool` for `OnCrit`/`OnNonCrit`. `OnCast`/`OnHit` pass `Shared(true)`.
  - The rules:
    - A `per_cast` proc qualifies on `crit_hits > 0` (`OnCrit`) or `crit_hits < hits` (`OnNonCrit`).
    - A `per_hit` proc rolls `crit_hits` or `hits − crit_hits` times.
    - `Shared(b)` is exactly today's behaviour.
  - `fn whole_hits(&self, action: usize, m: &DamageMeasure, why: &str) -> Result<u64, PlanError>` checks the whole-count rule. It is shared by `proc_roll_count` and the new sampling, so the check is never copied.

- [ ] **Step 1: Write the failing tests** (`mod proc_rolls`). The build is `dmg` 100 and `cc` 0.25. The action is `nova` with `"crit_rolls": "per_hit"`, 20 casts, and `iterations: 1` for exact counts.

```rust
// Procs: `c` on_crit per_hit chance 1 icd 0, `nc` on_non_crit per_hit chance 1 icd 0.
// Exact, seed-independent relations:
//   c + nc == 100 (every hit is exactly one of the two)
//   total_damage == 100 × 100 + 50 × c  (each crit hit adds 50)
//   c % 5 != 0 for the chosen seed (a shared mask would make c a multiple of 5 —
//     pick a seed where this holds and say so in a comment)
#[test]
fn mc_per_hit_crit_rolls_each_hit_independently() { … }

// Add `any` on_crit per_cast chance 1 icd 0 to the same run:
//   any <= 20, any <= c, 5 * any >= c   (fires iff ≥1 crit among the cast's 5 hits)
#[test]
fn mc_a_per_cast_proc_gates_on_any_hit_crit_under_per_hit_sampling() { … }

// n = 1 (hits_per_use 1) and, separately, minimal_plan (no events) with crit_rolls per_hit:
// serde_json::to_string(report) equal to the per_cast run, MC iterations 200 seed 7.
#[test]
fn mc_per_hit_sampling_is_byte_identical_when_it_cannot_matter() { … }

// hits_per_use 2.5, crit_rolls per_hit, NO procs: run returns Err containing
// "crit_rolls", "per_hit", "nova", "2.5"; the same config with crit_rolls per_cast runs.
#[test]
fn fractional_hits_fail_closed_under_per_hit_crit_sampling() { … }
```

- [ ] **Step 2: Run to verify they fail.** Run `cargo test -p rtce --lib proc_rolls::mc_per_hit` and `fractional_hits_fail_closed`. Expected:
  - `c % 5 != 0` fails, because the shared mask makes `c` a multiple of 5.
  - The any-hit relation fails.
  - The fractional test runs Ok instead of erroring.
- [ ] **Step 3: Implement.** When the action resolves to `per_hit` in MC:
  1. Get `n` from `whole_hits`.
  2. Loop `n` times over `evaluate_phase_sampled`, summing `objs[damage_objective]` and counting `is_crit_bit_set`.
  3. Pass `Counted` to the `OnCrit` and `OnNonCrit` rolls.

  For `per_cast`, keep the existing single call and `Shared(is_crit)`. Update the rustdoc of `roll_procs_mc`, `ProcRolls`, and `Trigger::OnCrit`/`OnNonCrit`, whose "ONE mask per cast" text becomes conditional on the resolved `crit_rolls`.
- [ ] **Step 4: Run to verify they pass.** Run `cargo test -p rtce --lib`. Mutation check: pass `Shared(crit_hits > 0)` instead of `Counted`. `mc_per_hit_crit_rolls_each_hit_independently` must fail on `c % 5 != 0` or on the damage relation. Restore.
- [ ] **Step 5: Commit.** `git commit -m "MC samples crit per hit under crit_rolls per_hit (c+nc=100, dmg=10000+50c; shared-mask contrast fails c%5)"`

### Task 3: EV weights, the re-pins, docs, CHANGELOG

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - the `complete_cast` EV branch, ~line 2450
  - `roll_procs_ev`, ~line 2744
  - `mod proc_rolls` tests
- Modify: `crates/rtce/src/sim/mod.rs` (the cast-complete order notes on proc rolls)
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

**Interfaces:**
- Consumes: `CompiledAction::crit_rolls` (Task 1) and `whole_hits` (Task 2).
- Produces: a private `enum CritWeight { Shared(f64), Independent { p_hit: f64, hits: u64 } }`, passed to `roll_procs_ev` in place of `weight: f64`.
  - `OnCast`/`OnHit` pass `Shared(1.0)`.
  - Under `per_cast` sampling, `OnCrit` passes `Shared(p)` and `OnNonCrit` passes `Shared(1 − p)`. This is today's behaviour.
  - Under `per_hit` sampling, they pass `Independent { p_hit: p, … }` and `Independent { p_hit: 1 − p, … }`. The per-proc rule for those is in Global Constraints.

- [ ] **Step 1: Write the failing tests** (`mod proc_rolls`, EV; the build is `cc` 0.25, `nova` 5 hits, 20 casts; all values are exact in binary because 0.75⁵ = 243/1024):

```rust
// per_cast on_crit chance 1 icd 0: weight 1 − 0.75^5 = 781/1024 per cast → 20 casts
// Σ = 15.2539… → 15 fires. Contrast crit_rolls per_cast: 0.25/cast → 5.
#[test]
fn ev_per_cast_on_crit_under_per_hit_sampling_weights_any_hit_crit() { /* 15 */ }

// per_cast on_non_crit chance 1 icd 0: weight 1 − 0.25^5 = 1023/1024 → Σ 19.98 → 19.
// Contrast per_cast sampling: 0.75/cast → 15.
#[test]
fn ev_per_cast_on_non_crit_under_per_hit_sampling_weights_any_hit_non_crit() { /* 19 */ }

// per_hit on_crit chance 1 icd 0.5 (clears before the next 1 s cast): ONE accumulation
// of 1 − (1 − 0.25)^5 = 781/1024 per cast → 15. Contrast per_cast sampling (shared
// mask, today's per-hit accumulation hits 1.0 on the 4th hit every cast) → 20.
#[test]
fn ev_per_hit_icd_proc_under_per_hit_sampling_fires_with_one_minus_one_minus_q_to_the_n() { /* 15 */ }

// per_hit on_crit chance 1 icd 0: n × 0.25 = 1.25/cast → 25 under BOTH samplings.
#[test]
fn ev_per_hit_icd0_proc_is_unchanged_by_crit_sampling() { /* 25 and 25 */ }

// The four resolution cells end to end (EV, the per_cast on_crit proc from the first
// test): defaults absent / crit absent → 5; defaults per_hit / crit absent → 15;
// defaults per_hit / crit per_cast → 5; defaults absent / crit per_hit → 15.
#[test]
fn ev_crit_rolls_resolution_cells_are_each_visible() { … }

// n = 1 and no-event plans: EV report byte-identical per_hit vs per_cast.
#[test]
fn ev_per_hit_sampling_is_byte_identical_when_it_cannot_matter() { … }

// The spec's P(≥1 crit) contrast, EV vs MC agreement: per_cast on_crit chance 1, icd 0,
// per_hit sampling, MC iterations 2000 seed 20261126 → mean within 5% of 15.25
// (and EV 15); per_cast sampling MC mean within 10% of 5.
#[test]
fn five_strike_at_least_one_crit_agrees_between_ev_and_mc() { … }
```

- [ ] **Step 2: Run to verify they fail.** Run `cargo test -p rtce --lib proc_rolls::ev_` and `five_strike`. Expected: under per_hit sampling, EV still gives the shared-mask numbers: 5 / 15 / 20 against the pinned 15 / 19 / 15.
- [ ] **Step 3: Implement.** Build `CritWeight` in `complete_cast`'s EV branch from the resolved `crit_rolls`. Under `per_hit`, compute `n` with `whole_hits`. Apply the per-proc rule in `roll_procs_ev`, keeping the ICD hard gate and the `actions` filter order unchanged. Then:
  - **Re-pin.** Run the full suite. For every pre-existing test that now fails (only `defaults.proc_rolls: "per_hit"` × a `"crit"` event × `n > 1` can), re-derive its new number by hand and assert it. Add a twin with `"crit_rolls": "per_cast"` on the action that asserts the old number. List each one in the report.
  - **Docs.** Cover `sim/mod.rs` and the `roll_procs_ev` weight docs.
  - **CHANGELOG `### Changed`:** "Under `defaults.proc_rolls: "per_hit"`, Monte Carlo now rolls crit independently per hit and EV crit-gated weights follow (#33). Only actions with a `"crit"` event and more than one hit move. `"crit_rolls": "per_cast"` on an action restores the old behaviour."
  - **CHANGELOG `### Added`:** "`ActionDef::crit_rolls`."
  - **CHANGELOG `### Upgrade notes`:** "Rust `ActionDef` struct literals need `crit_rolls: None`. A fractional `hits_per_use` now fails under a resolved `per_hit` even with no `per_hit` proc."
- [ ] **Step 4: Run to verify they pass.** Run `cargo test --workspace`. Mutation checks:
  - Multiply the icd>0 accumulation by `n`. `ev_per_hit_icd_proc…` must fail at `20` vs `15`.
  - Use `Shared(p)` under per_hit. The first test must fail at `5` vs `15`.
  - Restore after each.
- [ ] **Step 5: Commit.** `git commit -m "EV crit weights follow per-hit sampling (any-crit 15 vs 5, any-non-crit 19 vs 15, icd 1−(1−q)^n 15 vs 20)"`

### Task 4: Standing gates and consumers

- [ ] **Step 1: Byte identity.**
  1. Run `git worktree add ../rtce-main main`.
  2. Run `cargo run -q -p rtce --example diablo4_rotation`, redirecting to `../rtce-d4rot-main.txt` inside the worktree and to `../rtce-d4rot-branch.txt` here.
  3. `diff` the two files. The diff must be empty.
  4. Run `git worktree remove ../rtce-main` and delete both txt files.
- [ ] **Step 2: Engine gates.** All four standing commands must be clean.
- [ ] **Step 3: Consumers.** Run them per Global Constraints: d4 `17574.299999999996`, poe2 `96 passed`.
- [ ] **Step 4: Commit the plan file** (no push):

```bash
git add docs/superpowers/plans/2026-11-26-slice3-per-hit-crit.md
git commit -m "Plan: slice 3 (per-hit crit sampling)"
```
