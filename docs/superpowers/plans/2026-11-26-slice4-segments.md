# Slice 4 — Segments (hit-level casts, no cancels yet) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An action may be a timed sequence of SEGMENTS (strikes). Each segment lands its own hits, with its own damage overlay and effects, at its own instant, as a real queue event. There are no cancels yet; that is slice 5.

**Architecture:**
- `ActionDef` gains `segments: Vec<SegmentDef>`, and `cast_time` becomes optional.
- At compile, a segmented action yields `CompiledAction::segments`.
- In the executor, a segmented cast schedules one `Event::SegmentLand { action, segment }` per segment, in turn. Each landing runs the GENERALIZED cast-complete order (spec §5.2), and the last landing ends the cast.
- Unsegmented actions keep today's code path, with the same events, `seq` numbers and RNG draws. A one-segment action is pinned to give the same numbers as its unsegmented twin.

**Tech Stack:** Rust, serde, `rtce` (`crates/rtce`).

**Spec:** `docs/superpowers/specs/2026-11-25-hit-level-casts-design.md` §5.1, §5.2, §6 slice 4. The landing order is §5.2's generalized diagram, as corrected after the builder check.

## Global Constraints

- Branch `feat/segments` (already created from `main` @ 2562715; its first commit `478e2f5` is the CLAUDE.md consumer correction). The PR targets `main` with auto-merge on. Never stage `README.md` / `ROADMAP.md`.
- **`SegmentDef` fields (spec §5.1):**
  - `duration: NumOrExpr`: seconds from the previous boundary (cast start for segment 0) to this segment landing. It is evaluated AT THE SEGMENT'S START and must be finite and `>= 0`.
  - `damage: Option<ActionDamage>`: absent means a wind-up with no hits.
  - `effects: Vec<EffectDef>`: same rules as `ActionDef::effects`, and no `cast_action`.
  - `measure: Option<Measure>`.
  - Unknown keys fail closed, did-you-mean, and accept `_` annotations, like every other config struct.
- **Compile errors** (positioned: action name, plus segment index where relevant):
  - `segments` non-empty together with `cast_time`, which is ambiguous;
  - neither `cast_time` nor `segments`;
  - `segments` together with action-level `damage`;
  - a segmented action named by any `cast_action` effect;
  - a segment `effects` entry that is `cast_action`.
- **`cast_time: String` → `cast_time: Option<String>`.** JSON without it now parses; the missing-both case is the compile error above. `CompiledAction::cast_time` becomes `Option<Program>` (`None` for a segmented action). Every Rust struct literal changes. CHANGELOG Upgrade note: `cast_time: "1".into()` → `cast_time: Some("1".into())`.
- **Action-level `gain` and `effects` run only when the LAST segment lands** (full completion).
- **Landing order** (a one-segment action reproduces today's `complete_cast` exactly):

```text
[last segment]  apply action `gain`
[first segment] casts.<action> += 1
                segments_done += 1   (every landing; see Task 3)
  → measure this segment's world, evaluate its damage, credit it
  → this segment's effects, in list order
[last segment]  → action `effects`, in list order
  → proc rolls: [first segment] on_cast, then this segment's on_hit, on_crit, on_non_crit
  → [non-last] evaluate the next segment's duration at this instant, capture its cast_start snapshot if that segment measures at cast_start, schedule its SegmentLand
  → [last] end the cast (mid_cast = false at the same position today's completion clears it)
```

- **Cost and cooldown** are paid and armed at cast start, exactly as today.
- **`rolls: "per_cast"`** hit-trigger procs roll once per DAMAGING segment. `on_cast` rolls once per cast, at the first landing.
- **`crit_rolls`** applies to each segment's own measured hits.
- **Instant segment (`duration` 0):** it lands synchronously at its start instant, like an instant cast. It is measured at the landing position whatever `measure` says, following the instant-cast rule. A segmented cast whose every segment is 0 is an instant cast and is subject to `INSTANT_CHAIN_LIMIT`, with the same livelock error.
- **`EventOrder::CompletionsFirst`:** `SegmentLand` ranks with `CastComplete` (class 0).
- **Horizon:** a landing at `t == duration` is drained like a completion. Segments that land after the horizon never land. Damage, effects and proc rolls of segments that did land stand.
- **Measure resolution per segment:** `segment.measure ?? action.measure ?? defaults.measure`. `cast_complete` measures at the segment's landing and `cast_start` at the segment's start.
- **Deferred to slice 5:** the `cancelled` report column, the cast-id staleness guard, and every cancel. This slice has no cancels, so those would be untestable here.
- **Out of scope:** free casts of segmented actions (a compile error).
- TDD red-first, with the failing run captured BEFORE implementation and pasted into the report. Every pin is hand-worked and mutation-checked, with the contrast in the commit message. P8f: every new field's rustdoc states DEFAULT, EVALUATION INSTANT and INTERACTIONS.
- Zero allocation added on the run loop beyond what a cast already does. The existing per-cast overlay `BuildState` clone happens once per segment instead of once per cast; that is acceptable and must be documented.
- **Standing gates before the PR:**
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo fmt --all --check`
  - `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`
  - `examples/diablo4_rotation.rs` output byte-identical to `main`
  - poe2 against THIS tree: from `../poe2-theory-crafting`, back up `Cargo.lock`, run `cargo test --workspace --config 'patch.crates-io.rtce.path="/Users/bsmall/dev/rpg-theorycraft-engine/crates/rtce"'` (green) and `cargo test -p poe2-core --test rtce_parity` with the same `--config` (`96 passed`), then restore `Cargo.lock`.
  - d4: not applicable (it pins `rtce = "0.4"`).

## Shared test fixture (Tasks 2–3)

`seg_plan()`:
- stats `["dmg", "cc"]`
- bucket `bonus` (fold `sum`)
- event `crit { chance: "cc", factor: "1.5" }`
- pipeline `[{ name: "hit", expr: "dmg * (1 + bonus / 100) * event_factors", branched: true }]`
- objectives `["hit"]`

The build is `dmg 0, cc 0`, so P(crit) = 0 and EV is deterministic unless a test sets `cc`. Segment overlays set `dmg`.

The `slam` action has segments `[{ duration: 0.5, damage: { stats: { dmg: 100 } } }, { duration: 0.75, damage: { stats: { dmg: 300 } } }]`. One cast is 1.25 s (dyadic, so times are exact), and the rotation is a single rule `slam`. Buff `fury`: contribution `bonus +50`, `on_reapply: refresh`.

## Review Focus

1. **A one-segment action against its unsegmented twin**: byte-identical reports in EV and MC, with a crit event, an `on_cast` proc and an `on_hit` proc present. (Task 2.)
2. **A buff applied by segment 0 is seen by segment 1**, and is never seen by segment 0 itself. (Task 2.)
3. **A segment landing coincident with a buff expiry** resolves by `event_order`. (Task 2.)
4. **An all-instant segmented action** with no cooldown hits the livelock guard rather than looping. (Task 2.)
5. **`segments_done` / `segments_remain` read 0 at a rotation decision** (idle), so a `when` that reads them cannot wedge the rotation. (Task 3.)

---

### Task 1: Config surface and compile-time validation (no behaviour change)

**Files:**
- Modify: `crates/rtce/src/simdef.rs`:
  - `ActionDef`, ~549: `cast_time` → `Option<String>`, new `segments`, `KNOWN_KEYS`
  - new `pub struct SegmentDef`, with `KNOWN_KEYS` and P8f rustdoc
- Modify: `crates/rtce/src/sim/compile.rs`:
  - `CompiledAction`: `cast_time: Option<Program>` and `segments: Vec<CompiledSegment>`
  - new `pub struct CompiledSegment { pub duration: CompiledValue, pub damage: Option<BTreeMap<String, CompiledValue>>, pub effects: Vec<CompiledEffect>, pub measure: Measure }`, `#[non_exhaustive]`
  - the validations
  - the new reserved names `segments_done` / `segments_remain`
- Modify: every `ActionDef { … }` / `cast_time:` literal (≈50, in exec.rs, compile.rs and simdef.rs tests), plus any example or rtce-runner code that builds or reads `cast_time`.
- Modify: `crates/rtce/src/sim/exec.rs`, only where it reads `cast_time`: `begin_cast` uses `.as_ref().expect(…)` for unsegmented actions. Behaviour is unchanged.

**Interfaces:**
- Produces:
  - `SegmentDef { duration: NumOrExpr, damage: Option<ActionDamage>, effects: Vec<EffectDef>, measure: Option<Measure>, extra }`
  - `ActionDef::segments: Vec<SegmentDef>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`)
  - `CompiledSegment`, as above, with `measure` RESOLVED
  - `CompiledAction::segments`
  - `segments_done` / `segments_remain`: reserved sim words, compiled to two new sim slots (`SimSymbols`). The executor writes them in Task 3; they stay 0 until then.
  - Using either as a resource, action, buff or stat name is a compile error, mirroring `time`/`duration`.

- [ ] **Step 1: Write the failing tests** (`sim/compile.rs` tests, JSON):

```rust
#[test] fn a_segmented_action_compiles_with_resolved_segment_measures() // seg 0 inherits action/default measure, seg 1 override wins
#[test] fn segments_and_cast_time_together_is_a_compile_error()          // msg names action, "segments", "cast_time"
#[test] fn neither_segments_nor_cast_time_is_a_compile_error()
#[test] fn segments_with_action_level_damage_is_a_compile_error()
#[test] fn free_casting_a_segmented_action_is_a_compile_error()          // proc cast_action → segmented action
#[test] fn a_cast_action_effect_inside_a_segment_is_a_compile_error()
#[test] fn an_unknown_segment_key_fails_with_did_you_mean()              // "duraton" → "duration"; "_note" accepted
#[test] fn segments_done_is_reserved()                                    // as a resource name → "reserved"
```

- [ ] **Step 2: Capture RED.** Run `cargo test -p rtce --lib segment` against the unchanged engine: compile errors or failures. Paste them into the report.
- [ ] **Step 3: Implement.** Add the types, the migration, the validation, the reserved slots and their rustdoc.
- [ ] **Step 4: GREEN.** Run `cargo test --workspace`. Every pre-existing test must be unchanged. Mutation: drop the both-present check, and `segments_and_cast_time_together…` must fail. Restore.
- [ ] **Step 5: Commit.** `"Segments config surface: SegmentDef, optional cast_time, compile validation (no behaviour change)"`

### Task 2: Executing segments

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - `Event`, ~693, gains `SegmentLand { action: usize, segment: usize }`
  - the event-class match, ~737
  - `run_loop`, ~3320, dispatches the new event
  - `begin_cast`, ~2377
  - new `fn land_segment(&mut self, action: usize, segment: usize) -> Result<(), PlanError>`
  - `capture_world`, `overlay_build_for_action`, `eval_hits_per_use` and `apply_action_buffs` take a part selector
  - tests

**Interfaces:**
- Consumes: Task 1's `CompiledSegment` and `CompiledAction::segments`.
- Produces: a private `enum Part { Whole, Segment(usize) }`. Every damage, measure and effects helper reads its `damage`, `effects` and `measure` through `Part`, so the code is never copied. Task 3 extends `land_segment`.

- [ ] **Step 1: Write the failing tests** (`mod segments` in exec.rs tests, using the shared fixture, EV unless stated). Hand-derived pins:

```rust
// slam over weight 5: casts at 0, 1.25, 2.5, 3.75; seg 1 lands 1.25/2.5/3.75/5.0 (horizon, drained)
//   → total 4 × 400 = 1600, casts 4.
// weight 4.75: cast 4's seg 0 lands 4.25, seg 1 (5.0) never does → 3 × 400 + 100 = 1300, casts 4.
//   Contrast: unsegmented `cast_time 1.25, damage dmg 400` at 4.75 → 3 casts, 1200.
#[test] fn a_segmented_cast_credits_each_landed_segment()

// One segment {duration 1.25, damage dmg 400} vs unsegmented {cast_time 1.25, damage dmg 400},
// with build cc 0.25, an on_cast proc and an on_hit proc (chance 0.5, apply a 0.1 s buff):
// serde_json::to_string(report) equal, EV and MC (iterations 50, seed 3), weight 10.
#[test] fn a_one_segment_action_is_identical_to_its_unsegmented_twin()

// seg 0 effects [apply_buff fury] (fury duration 10), weight 5:
//   cast 1: 100 + 450; casts 2–4: 150 + 450 → 550 + 3 × 600 = 2350.
//   Contrast: the same effect at ACTION level (last landing) → 400 + 3 × 600 = 2200.
#[test] fn a_buff_applied_by_one_segment_reaches_the_next()

// on_cast proc + on_hit proc (rolls per_cast), chance 1, icd 0, each applying a 0.1 s buff; weight 5:
//   slam → on_cast 4, on_hit 8.
//   Wind-up variant (seg 0 has no damage) → on_cast 4, on_hit 4.
#[test] fn on_cast_rolls_once_per_cast_and_per_cast_hit_procs_once_per_damaging_segment()

// fury duration 0.75 applied by seg 0 at 0.5 expires at 1.25 = seg 1's landing; weight 1.25:
//   event_order scheduled → expiry first → seg 1 = 300 → total 400;
//   completions_first → landing first → 450 → total 550.
#[test] fn a_segment_landing_coincident_with_a_buff_expiry_follows_event_order()

// segments [{duration 0, dmg 100}, {duration 1, dmg 300}], weight 1 → 400, casts 1.
#[test] fn an_instant_segment_lands_at_its_start_instant()
// every segment duration 0, no cooldown → Err containing the instant-cast livelock message.
#[test] fn an_all_instant_segmented_action_hits_the_livelock_guard()
// seg 1 duration "-1" → Err naming the action, "segment 1", "duration".
#[test] fn a_negative_segment_duration_fails_closed()
```

Also pin action-level `gain` at the last landing, using a resource. The relation to pin: `gain` is credited once per COMPLETED cast, before the last landing's measure. A cast cut by the horizon after segment 0 credits nothing.

Choose an observable: for example, segment 0's `dmg` is `"100 + rage"` and segment 1's is `"300 + rage"` (action `gain: { rage: 10 }`), so each landing's damage reads `rage` at its measure. Derive the number by hand from the resource's documented starting amount, and write the derivation in a comment. The mutation is crediting `gain` at the FIRST landing, which must change segment 0's reading. Name it `action_gain_is_credited_at_the_last_landing_only`. If you cannot find a discriminating observable, report NEEDS_CONTEXT.

- [ ] **Step 2: Capture RED.** Run `cargo test -p rtce --lib segments::` on Task 1's head. The segmented tests fail because segmented actions don't execute; paste the output.
- [ ] **Step 3: Implement.**
  - `begin_cast` for a segmented action: cost and cooldown exactly as today. Then evaluate segment 0's `duration`, and capture segment 0's `cast_start` snapshot if it resolves `cast_start`. Then set `mid_cast` and schedule `SegmentLand`, or land synchronously when the duration is 0.
  - `land_segment` follows the Global Constraints order verbatim. Instant segments land synchronously in a loop, not by recursion.
- [ ] **Step 4: GREEN + mutations.**
  - Run `cargo test --workspace`; every pre-existing test must be unchanged.
  - Mutation 1: move segment effects after the proc rolls. Expected: the order or buff-carry pins fail.
  - Mutation 2: roll `on_cast` at every landing. `on_cast` must give 8, not 4.
  - Mutation 3: rank `SegmentLand` as class 1. The `completions_first` cell must give 400.
  - Record each contrast and restore.
- [ ] **Step 5: Commit.** `"Execute segments: SegmentLand events and the generalized landing order (1600/1300 vs 1200; buff carry 2350 vs 2200)"`

### Task 3: Per-segment measure, `segments_done` / `segments_remain`, crit sampling per segment, docs, CHANGELOG

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - writing the two slots: `segments_done` incremented at each landing BEFORE the measure, `segments_remain = n − segments_done` while a segmented cast is in flight, both reset to 0 when the cast ends
  - unsegmented actions: always 0
  - per-segment measure
- Modify: `crates/rtce/src/simdef.rs`:
  - rustdoc for `ActionDef::segments`, `SegmentDef` fields, `Measure` (per-segment reading), `ProcRolls` ("per damaging segment")
  - `Trigger::OnCast`: fix "rolls once per cast begun" to "at the cast's first landing (completion for an unsegmented action)"
- Modify: `crates/rtce/src/sim/mod.rs`:
  - the canonical cast-complete order diagram, generalized as in Global Constraints
  - the symbol-space list, adding the two new symbols
- Modify: `CHANGELOG.md` (`## [Unreleased]`):
  - Added: segments, the two symbols
  - Upgrade notes: the `cast_time` Option migration, plus new reserved words that conflict with existing stat, condition, resource, action or buff names

- [ ] **Step 1: Write the failing tests** (`mod segments`; weight 1.25 = one cast, EV):

```rust
// fury duration 0.5 applied by seg 0 effects at 0.5 → expires 1.0; seg 1 lands at 1.25.
//   (i)   defaults                                    → 100 + 300 = 400
//   (ii)  seg 1 measure cast_start (measured at 0.5)  → 100 + 450 = 550
//   (iii) action measure cast_start, no seg override  → 550
//   (iv)  action cast_start, seg 1 override cast_complete → 400
#[test] fn per_segment_measure_resolution_cells()

// seg 0 dmg "100 * segments_done", seg 1 dmg "100 * segments_done" → 100 + 200 = 300.
//   Contrast (increment after measure) → 0 + 100 = 100.
#[test] fn segments_done_counts_the_landing_being_measured()
// seg 0 dmg "100 * segments_remain", seg 1 the same → 100 + 0 = 100.
#[test] fn segments_remain_counts_segments_still_to_land()
// rotation rule when "segments_done == 0 && segments_remain == 0" → casts proceed (weight 5 → 4 casts).
#[test] fn the_segment_symbols_read_zero_at_a_rotation_decision()

// crit_rolls per segment: build cc 0.25, both slam segments hits_per_use 5, crit_rolls per_hit,
// per_cast on_crit proc chance 1 icd 0, weight 5 (4 casts):
//   per segment weight 1 − 0.75^5 = 781/1024, 8 damaging segments → Σ 6.1015625 → 6 fires.
//   Contrast crit_rolls per_cast: 8 × 0.25 = 2.
#[test] fn crit_rolls_applies_to_each_segments_own_hits()
```

- [ ] **Step 2: Capture RED** against Task 2's head. Paste the failing output.
- [ ] **Step 3: Implement**, plus the docs and CHANGELOG listed above. The P8f rustdoc must cover every new field and symbol. Document the per-segment overlay clone as a cost note.
- [ ] **Step 4: GREEN + mutations.**
  - Ignore the segment-level `measure` override; cell (iv) must give 550.
  - Increment `segments_done` after the measure; the pin must give 100.
  - Restore after each.
- [ ] **Step 5: Commit.** `"Per-segment measure and segment symbols (cells 400/550/550/400; segments_done 300 vs 100)"`

### Task 4: Standing gates

- [ ] **Step 1: Byte identity.**
  1. `git worktree add ../rtce-main main`.
  2. Run `cargo run -q -p rtce --example diablo4_rotation` inside it, redirected to `/Users/bsmall/dev/rtce-d4rot-main.txt`, and the same command here, redirected to `/Users/bsmall/dev/rtce-d4rot-branch.txt`.
  3. `diff` the two files; the result must be empty.
  4. Remove the worktree and both files.
- [ ] **Step 2: Engine gates.** All four standing commands must be clean.
- [ ] **Step 3: poe2 against this tree.** Use the override in Global Constraints (`435`-test workspace green, `96 passed`), and restore `Cargo.lock`.
- [ ] **Step 4: Commit the plan file** (no push):

```bash
git add docs/superpowers/plans/2026-11-26-slice4-segments.md
git commit -m "Plan: slice 4 (segments)"
```
