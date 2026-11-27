# Slice 5 — Player cancels (`interrupt`, `interrupt_if`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A rotation rule may let a higher-priority rule cancel its segmented cast at a segment boundary (Rolling Slam: cancellable after the first hit). Landed segments keep everything; unlanded ones contribute nothing. Each action's report gains a `cancelled` count.

**Architecture:**
- **What a cast remembers.** An in-flight cast records the index of the rule that started it.
- **Where the check runs.** `Sim::land_segment` checks for a cancel after each NON-LAST landing's whole transaction, and before `start_segment(next)`. Because the next segment is not scheduled yet, nothing is pending: no `SegmentLand` and no `cast_start` snapshot. So a cancel just ends the cast and begins the winning rule's action at the same instant.
- **How eligibility is decided.** It reuses `attempt_decision`'s per-rule check: `when`, cooldown ready, affordable now.

**Tech Stack:** Rust, `rtce` (`crates/rtce`).

**Spec:** `docs/superpowers/specs/2026-11-25-hit-level-casts-design.md` §5.3 (with §5.1–5.2 for segments and the landing order, and §7's matrix row for cancels).

## Global Constraints

- **Branch and PR.** Branch `feat/player-cancels`, cut from `main` (91012e4, rtce 0.9.1). The PR targets `main` with auto-merge. Never stage `README.md`.
- **Config (spec §5.3).**
  - `Rule` gains `interrupt: bool` (serde default `false`) and `interrupt_if: Option<String>` (default absent).
  - If `interrupt_if` is present and `interrupt` is absent, `interrupt` is implied.
  - `interrupt: false` with `interrupt_if` present is a positioned compile error (a contradiction, never guessed).
  - Add both keys to `Rule::KNOWN_KEYS`.
- **Fail closed (call, made in planning).** An interrupting rule whose action has fewer than 2 segments (unsegmented, or one segment) has no non-last boundary. That is a positioned compile error naming the rule's position and action.
- **When the check runs.** At each NON-LAST segment boundary of the in-flight cast, after that landing's whole transaction (gain is last-only; `casts` / `segments_done` / damage / effects / proc rolls), and before the next segment starts. The check runs only if the cast's starting rule has `interrupt` and its `interrupt_if` (if any) is nonzero.
- **What may cancel.** Only rules ranked ABOVE the starting rule (lower index) are evaluated, with the normal decision logic: `when`, then cooldown ready (`cooldown_ready_at <= now`), then afford now. The FIRST eligible one wins. Same-rule and lower rules never cancel. A higher rule naming the same action is allowed: it is a different rule.
- **What a cancel does.**
  - It does not start the next segment.
  - It ends the cast (`mid_cast = false`, `segments_done = segments_total = 0`).
  - It increments the cancelled action's `cancelled`.
  - It then calls `begin_cast(winner's action)` at the same instant, recording the winner's rule index.
- **Cancel cost rule (spec, fixed).**
  - Cost already paid stays paid.
  - The cooldown armed at cast start stays armed.
  - Landed segments keep their damage, effects and proc rolls.
  - Unlanded segments contribute nothing. Action-level `gain` and action-level `effects` (both last-landing-only) are NOT applied.
- **`interrupt_if`.**
  - **Instant:** the boundary, after the landing transaction.
  - **World:** the plain effective build and phase.
  - **Compiling:** it compiles through `compile_sim` with `ObjectiveReads::Legal`, so the `ev_uptime` forbidden-read probe applies automatically.
  - **Reading it:** it reads `segments_done` / `segments_remain` as of that boundary (for example `segments_done` = 1 after the first landing).
- **Modes.** Deterministic, so EV and MC behave identically.
- **Report.** `ActionReport.cancelled: u64`, always serialized. Under MC it is the pooled mean rounded to the nearest whole count, like `casts`.
- **Cast-id staleness guard.** NOT built in this slice. A cancel happens before the next `SegmentLand` is scheduled, so no stale event can exist. The guard is slice 7's (outside interruptions land mid-segment); say so in the rustdoc and in ROADMAP.
- **TDD and pins.** TDD red-first, with RED captured before implementing. Every pin is hand-worked, with the contrast in the commit message. Mutation-check each behavior pin.
- **P8f.** `interrupt` and `interrupt_if` rustdoc state DEFAULT, INSTANT and INTERACTIONS: segments only, higher rules only, the cost rule, no gain or effects on cancel, `segments_done` at the boundary, EV = MC. Every §7 cell `interrupt` false × true × `interrupt_if` truthy/falsy, higher rule eligible × not, and cancel at segment 0 vs a later boundary gets a discriminating test.
- **Standing gates before the PR:**
  - `cargo test --workspace`
  - clippy `-D warnings`
  - fmt `--check`
  - `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`
  - `diablo4_rotation` byte-identical to `main`
  - both consumers against this tree with the patch override:
    - poe2 is at `rtce = "0.9.x"` now: if its requirement doesn't accept 0.9.1, use a worktree of poe2's `origin/main`;
    - never run it in poe2's main checkout while another session works there;
    - pass: poe2 workspace green and `rtce_parity` 96 passed; d4 workspace green and `eval` `17574.299999999996`.

## Shared fixture (Task 2)

In `exec.rs`'s `mod segments` (reuse `seg_plan`, `seg_build(0.0)`, `fight`):
- `slam`: segments `[ {0.5, dmg 100}, {0.75, dmg 300} ]`.
- `jab`: `cast_time` 0.25, `cooldown` 2, dmg 50.
- Rules `[ jab, slam(+interrupt fields) ]`, weight 5, EV.

**With `interrupt: true`:**
- jab runs 0–0.25 (cooldown ready at 2).
- slam 0.25: lands 0.75 (jab not ready, no cancel), then 1.5.
- slam 1.5: lands 2.0, and jab is ready, so it is **cancelled**. jab runs 2.0–2.25 (ready at 4).
- slam 2.25: lands 2.75, then 3.5.
- slam 3.5: lands 4.0, and jab is ready, so it is **cancelled**. jab runs 4.0–4.25.
- slam 4.25: lands 4.75; its second segment at 5.5 is past the horizon.
- Totals: jab 3 × 50 = 150; slam segment 0 × 5 = 500 and segment 1 × 2 = 600. **Total 1250, `slam.casts` 5, `slam.cancelled` 2, `jab.casts` 3.**

**Without interrupt (the contrast):**
- jab 0–0.25.
- slam lands 0.75, 1.5, then 2.0, 2.75.
- jab 2.75–3.0 (ready at 5).
- slam lands 3.5, 4.25, then 4.75.
- Totals: jab 2 × 50 = 100; slam 4 × 100 + 3 × 300 = 1300. **Total 1400, `cancelled` 0.**

## Review Focus

- **A cancel that begins an INSTANT action.** If the winner has `cast_time` 0, the cast completes at the boundary instant. The rotation must then re-decide at that same instant: `run_loop`'s post-event `attempt_decision` must pick up the next cast, not stall until a later event. (Task 2: `a_cancel_into_an_instant_action_redecides_at_once`.)
- **Gain and action-level effects on a cancelled cast.** A cancelled slam with `gain` and an action-level `effects` list must credit neither. Its landed segment's own effects and procs must still fire. (Task 2: `a_cancelled_cast_keeps_landed_segments_only`.)
- **A cancel at the boundary of a zero-duration (instant) middle segment.** Instant segments land synchronously inside `land_segment`'s loop, and the check must run there too, after each one. (Task 2: `a_cancel_runs_between_synchronous_instant_segments`, with 3 segments `[0.5, 0, 0.75]`; a cancel after segment 1 (instant) is observable.)
- **MC run.** Cancels are deterministic. An MC run's `cancelled` equals EV's on a crit-free fixture, and the pooled `cancelled` is a whole count. (Task 2: `cancels_are_identical_in_mc`.)
- **The horizon.** A boundary AT the horizon instant may cancel, but the winner's cast must not begin at or after the horizon. This follows `run_loop`'s rule that no cast begins at the horizon. (Task 2: `no_cancel_begins_a_cast_at_the_horizon`. Pick a weight so that a boundary lands exactly at `duration`, and assert the winner is not cast there and `cancelled` does not count it.)

Ruling for the last line, made in planning: at the horizon instant a boundary does NOT cancel, because the replacement could not begin. So `cancelled` counts only cancels that started a replacement.

---

### Task 1: Config fields and compile-time checks

**Files:**
- Modify: `crates/rtce/src/simdef.rs` (`Rule` ~1967: the fields, their rustdoc, `KNOWN_KEYS`)
- Modify: `crates/rtce/src/sim/compile.rs` (`CompiledRule` ~482; the rule loop ~1501)
- Test: the `compile.rs` test module

**Interfaces:**
- Produces:
  - `Rule { interrupt: bool, interrupt_if: Option<String>, .. }`, with `#[serde(default)]` on both;
  - `CompiledRule { action, when, interrupt: bool, interrupt_if: Option<SimProgram> }`, where `interrupt` is already resolved (true if either is set).

**Steps:**

- [ ] **Step 1: Failing tests:**
  - `rule_interrupt_parses_and_defaults`: absent means `false` / `None`; `"interrupt": true` round-trips; `"interrupt_if": "segments_done >= 1"` alone compiles to `interrupt == true`.
  - `rule_interrupt_false_with_interrupt_if_is_an_error`: the message names the rule's position and action.
  - `rule_interrupt_on_an_unsegmented_action_is_an_error`, and the same for a 1-segment action: the message names the rule and says it needs ≥ 2 segments.
  - `rule_interrupt_if_reads_objectives_legally`: `interrupt_if: "objective.hit > 0"` compiles.
  - `rule_interrupt_if_unknown_symbol_is_positioned`: the error names "rotation rule (action `slam`) interrupt_if".
- [ ] **Step 2: Run** `cargo test -p rtce rule_interrupt`. Expected: compile errors on the new fields (RED).
- [ ] **Step 3: Implement** the fields, the checks and the P8f rustdoc. The rustdoc covers the semantics from Global Constraints, including that no cast-id guard is needed yet and why.
- [ ] **Step 4: Run** `cargo test -p rtce`, clippy and fmt. Expected: green.
- [ ] **Step 5: Commit.** Message: "Rules: interrupt / interrupt_if config and compile checks (slice 5)".

### Task 2: Executor cancel and the `cancelled` column

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - `attempt_decision` records the chosen rule index;
  - `begin_cast` takes it, via a new `Sim::cast_rule: Option<usize>` set before `begin_cast`;
  - `land_segment` runs the cancel check;
  - a new `Sim::try_cancel(&mut self, action: usize) -> Result<bool, PlanError>`;
  - the per-action runtime gains `cancelled: u64`, which the report building uses.
- Modify: `crates/rtce/src/sim/report.rs`: `ActionReport.cancelled: u64` with rustdoc, plus the MC pooled-mean aggregation next to `casts` (exec.rs ~607).
- Test: `exec.rs` `mod segments`.

**Interfaces:**
- Consumes: Task 1's `CompiledRule.interrupt` and `CompiledRule.interrupt_if`.
- Produces: `ActionReport.cancelled`.

**Steps:**

- [ ] **Step 1: Failing tests** (Shared fixture above):
  - `a_higher_rule_cancels_at_a_boundary`: `interrupt: true` gives total 1250, `slam.casts` 5, `slam.cancelled` 2, `jab.casts` 3. No interrupt gives 1400 and `cancelled` 0. Assert both.
  - `interrupt_if_gates_the_cancel`: `"segments_done >= 1"` gives 1250. `"segments_done >= 2"` gives 1400, because at the only non-last boundary `segments_done` = 1.
  - `a_lower_or_same_rule_never_cancels`: rules `[slam(interrupt), jab]`. slam is the top rule, so no rule ranks above it. It casts back-to-back at 0, 1.25, 2.5 and 3.75, giving 4 × 400 = **1600**, `cancelled` 0 and `jab.casts` 0 (the existing `a_segmented_cast_credits_each_landed_segment` timeline).
  - `a_higher_rule_that_is_not_eligible_does_not_cancel`: jab with `when: "0"` gives `cancelled` 0.
  - `a_cancel_at_segment_0_vs_later`: a 3-segment slam `[0.5/100, 0.5/200, 0.5/300]` with `interrupt_if: "segments_done == 2"` cancels only at the second boundary. Hand-work the totals, and contrast with `== 1`.
  - `a_cancelled_cast_keeps_landed_segments_only`: slam with `gain` and an action-level `effects` apply of `blip`, and segment 0 carrying its own `effects` apply of `fury`. On cancelled casts: no gain credited, no action-level `blip` applications, `fury` applications counted per landed segment 0. Assert from `resources` / `buffs[..].applications`.
  - The five Review Focus tests, named there.
- [ ] **Step 2: Run** `cargo test -p rtce segments::`. Expected: failures on `cancelled` / missing behavior (RED).
- [ ] **Step 3: Implement:**
  - `try_cancel`: return false unless the cast's rule interrupts and `interrupt_if` is truthy. Refresh the time-varying slots and fill objective slots if `interrupt_if` reads objectives (`ObjectiveWorld::Plain`, "at a segment boundary"). At the horizon instant, return false (ruling above). Otherwise scan rules `0..cast_rule` with the same eligibility as `attempt_decision`; factor that per-rule check into one shared helper rather than duplicating it. On the first eligible rule, end the cast, add `cancelled += 1`, and `begin_cast` the winner.
  - `land_segment`: call it after each non-last `land`. If it returns true, return immediately, since the new cast owns the timeline now.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: green. Byte-identity: `diablo4_rotation` output equals `main`'s (it has no segmented actions). Mutation-check:
  - scan `0..=cast_rule`, so the same rule cancels: a pin fails;
  - drop the horizon guard: `no_cancel_begins_a_cast_at_the_horizon` fails;
  - apply `gain` on cancel: the keeps-landed pin fails.
- [ ] **Step 5: Commit.** Message: "Player cancels at segment boundaries; ActionReport.cancelled (slice 5)", with the 1250 vs 1400 contrast.

### Task 3: Docs, CHANGELOG, ROADMAP, gates

**Files:**
- Modify:
  - the guide chapter that documents segments (find where `segments` is documented under `docs/guide/`): a "Cancelling a segmented cast" section with the fixture's 1250 vs 1400 (both pinned);
  - `CHANGELOG.md` Unreleased: Added, plus an Upgrade note (`Rule` struct literals need `interrupt: false, interrupt_if: None`; `ActionReport` is `#[non_exhaustive]`, so the new field is additive);
  - `ROADMAP.md`: move "Player cancels" from the second-half open list to done (Unreleased), and note the cast-id guard moves to the outside-interruptions entry.

**Steps:**

- [ ] **Step 1: Write the docs.** Every number must match a pin.
- [ ] **Step 2: Run the standing gates** (Global Constraints) and record the outputs.
- [ ] **Step 3: Commit** ("Player cancels: guide, changelog, roadmap"). Do not push: the controller opens the PR after the whole-branch review.
