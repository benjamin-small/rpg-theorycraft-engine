# EV expected buff uptime (`ev_uptime`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A buff with `ev_uptime: "expected"` carries its exact probability of being up, P, through an EV run. Its conditions, contributions, live DoT and uptime reports then read P, so EV and MC agree on its uptime (#33 item 4).

**Architecture:**
- **Pure math in a new module.** `sim/expected.rs` holds two things. The per-cast q derivation conditions on the crit model. The roll log stores latest-wins weights.
- **Executor (EV only).** It logs one entry per cast event for each expected buff, and skips that buff's application at an accumulator crossing. It keeps P cached per buff, and ages entries out through one rescheduled heap event that carries the stored `seq`. Readers scale by P.
- **Compile.** It enforces the design's constraints, positioned and naming the buff.
- **MC.** Unchanged.

**Tech Stack:** Rust, `rtce` (`crates/rtce`).

**Spec:** `docs/superpowers/specs/2026-11-27-ev-expected-uptime-design.md` (read it whole; it carries the formulas and the reasons).

## Global Constraints

- **Branch.** `feat/ev-expected-uptime`, which already has the design commit. The PR targets `main` with auto-merge. Never stage `README.md`.
- **Field.** `BuffDef::ev_uptime: EvUptime`, where `enum EvUptime { #[default] Discrete, Expected }`, with serde `snake_case` (`"discrete"` / `"expected"`). Add `"ev_uptime"` to `BuffDef::KNOWN_KEYS`. When the field is absent, everything is byte-identical.
- **MC.** It ignores the knob, but every compile check still runs in both modes. An MC run with `expected` must be byte-identical to the same run with `discrete`.
- **q.** It is the probability of at least one application of X during ONE cast event: one `land`/`complete` transaction, the call that rolls `OnCast`/`OnHit`/`OnCrit`/`OnNonCrit`. Each proc counts once, however many times its effects list names X. Chances are clamped to [0, 1]. q = 1 for an action's `apply_buff`/`effects` application.
- **D.** It is evaluated once per log entry, exactly as `apply_buff` evaluates `duration` today. That happens at the FIRST contributing proc of the cast, before that proc's accumulator step or fire. For an action application, it happens at the application.
- **Ordering.** Each entry consumes a `seq` at its roll. The age-out heap item reuses that stored seq and has `BuffExpire`'s ordering class under both `EventOrder`s.
- **Promise.** Uptime is exact. Everything downstream is EV's linearization. The rustdoc says so.
- **TDD and pins.** TDD red-first, with RED captured before implementing. A pin that passes on arrival gets a recorded failing mutation instead. Every pin is hand-worked, with the contrast in the commit message.
- **P8f.** The `ev_uptime` rustdoc states the DEFAULT, the INSTANT (q per cast event, D as above) and the INTERACTIONS: every constraint, MC ignoring it, the reader table and the linearization.
- **Standing gates before the PR (CLAUDE.md):**
  - `cargo test --workspace`
  - clippy `-D warnings`
  - fmt `--check`
  - `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`
  - `examples/diablo4_rotation.rs` byte-identical to `main`
  - poe2 and d4 run against this tree with the `--config patch.crates-io.rtce.path=…` override, restoring each `Cargo.lock` afterwards:
    - poe2: workspace green, and `rtce_parity` 96 passed;
    - d4: workspace green, and `eval` `17574.299999999996`.

## Review Focus

- **A proc applying X alongside other effects.** The other effects must keep firing at the same accumulator crossings as with `discrete`. Only X's application moves.
- **A segmented action.** Each landing is its own cast event, so q and the entry are per landing, not per whole cast.
- **A run where X is never applied.** P stays 0: uptime 0, conditions at their static value, no age-out events, no division by zero in the report.
- **Several rolls at one instant whose windows end at the same time.** Age-outs at equal `t_end` must drain in one event, without a zero-delay reschedule loop tripping `HORIZON_DRAIN_LIMIT`.
- **Phase boundaries.** A static uptime that changes at a boundary must re-blend `P·v + (1−P)·static` with the new static value.

Each line's test lives in the task named in parentheses after it below.

---

### Task 1: `ev_uptime` field and compile-time constraints

**Files:**
- Modify: `crates/rtce/src/simdef.rs` (the `BuffDef` field, the new `EvUptime` enum next to `ReapplyPolicy`, `KNOWN_KEYS`)
- Modify: `crates/rtce/src/sim/compile.rs`:
  - `CompiledBuff::ev_uptime: EvUptime`;
  - the checks;
  - the forbidden-read probe in `compile_sim`, extending `ObjectiveProbe` with a `forbidden` lookup;
  - `SimSymbols` gains `forbidden: BTreeMap<String, String>` (read name → expected buff name).
- Test: the `compile.rs` test module.

**Interfaces:**
- Produces:
  - `pub enum EvUptime { Discrete, Expected }`, re-exported where `ReapplyPolicy` is;
  - `CompiledBuff.ev_uptime`;
  - `CompiledProc.expected_buffs: Vec<usize>`: the expected buff indices in this proc's `effects`, deduped, in index order;
  - `SimPlan.condition_expected: BTreeMap<String, usize>`: condition name → the expected buff driving it.

**Steps:**

- [ ] **Step 1: Failing tests.** Write one test per positioned error. Each asserts that the message names the buff and the reason:
  - `ev_uptime_expected_rejects_stacking`: `max_stacks: 3` with `add_independent`, and separately `add_refresh_all`.
  - `ev_uptime_expected_rejects_snapshot_tick`
  - `ev_uptime_expected_rejects_icd_proc`: a proc with `icd: 2` applying X. The error names the proc too.
  - `ev_uptime_expected_rejects_free_cast_application`: X in the buff list of an action reached through a proc's `CastAction`.
  - `ev_uptime_expected_rejects_buff_word_reads`: `buff.x`, `buff_remaining.x` and `stacks.x`, each in a rule `when`, a proc `chance` and a buff `duration`. The error names the field.
  - `ev_uptime_expected_rejects_driven_condition_reads`: a rule `when: "recently >= 1"` where X drives `recently`. The same test asserts that an `objective.<o>` read compiles.
  - `ev_uptime_expected_rejects_shared_condition`: X and another buff both drive `recently`.

  Also write `ev_uptime_parses_and_defaults`:
  - absent means `Discrete`;
  - `"expected"` round-trips;
  - `"expectd"` is a serde error.

  And write `ev_uptime_expected_compiles_for_a_legal_buff`. It asserts `expected_buffs`, with X named twice in one proc's effects giving a list of length 1, and asserts `condition_expected`.
- [ ] **Step 2: Run** `cargo test -p rtce ev_uptime`. Expected: compile failure on `EvUptime`, which is the RED.
- [ ] **Step 3: Implement.**
  - The field, with its full P8f rustdoc. Model it on `ReapplyPolicy` and `ProcRolls`, and include the reader table from the spec.
  - The checks in `compile`, after buffs and procs resolve.
  - Seed `forbidden` before any sim expression compiles, then compile every sim expression through the probe.
  - A forbidden read errors as: "`<what>` reads `<name>`, which buff `<x>` (`ev_uptime: expected`) carries as a probability, not a state: …".
- [ ] **Step 4: Run** `cargo test -p rtce`, plus clippy and fmt. Expected: green.
- [ ] **Step 5: Commit.** Message: "ev_uptime: field and fail-closed constraints (#33 item 4)".

### Task 2: Pure math (`sim/expected.rs`)

**Files:**
- Create: `crates/rtce/src/sim/expected.rs`, plus `mod expected;` in `sim/mod.rs`
- Test: in-file `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub(crate) struct NoFire { indep: f64, crit_per_hit: f64, crit_per_cast: f64, nc_per_hit: f64, nc_per_cast: f64 }`
    - Each field is a product of `(1 − c)` over the matching procs.
    - `NoFire::ONE` has every field 1.0.
  - `NoFire::add(&mut self, trigger: Trigger, rolls: ProcRolls, chance: f64, n: u32)`: clamps `chance` to [0, 1] and multiplies `(1 − c)` into one field.
    - `OnCast`/`OnHit` go to `indep`: as `(1 − c)` for `per_cast`, `(1 − c)^n` for `per_hit`.
    - `OnCrit`/`OnNonCrit` go to `crit_*`/`nc_*` by `rolls`, stored per roll. `q_of` raises the `*_per_hit` fields to the realized hit count.
  - `pub(crate) enum CritModel { None, Shared { p: f64, n: u32 }, Independent { p: f64, n: u32 } }`
  - `pub(crate) fn q_of(nf: &NoFire, crit: CritModel) -> f64` returns `1 − P(no application)`:
    - **Shared:** `indep · (p·crit_per_hit^n·crit_per_cast + (1−p)·nc_per_hit^n·nc_per_cast)`. One crit outcome per cast, and a gated `per_hit` proc rolls n times.
    - **Independent:** `indep · Σ_{k=0..n} C(n,k) p^k (1−p)^{n−k} · crit_per_hit^k · crit_per_cast^{[k>0]} · nc_per_hit^{n−k} · nc_per_cast^{[k<n]}`. Compute it in O(n) with a running binomial term, no factorials and no allocation.
    - **None** (a non-damaging cast): `1 − indep`. The crit-gated fields are ignored.
  - `pub(crate) struct RollLog { entries: VecDeque<Entry> }`, where `Entry { t_end: f64, weight: f64, seq: u64 }`, and:
    - `push(&mut self, t_end: f64, q: f64, seq: u64)`: multiplies every existing weight by `(1 − q)`, then appends;
    - `p_at(&self, s: f64) -> f64`: `Σ weight` over entries with `t_end > s`;
    - `drop_expired(&mut self, s: f64)`: removes entries with `t_end <= s`;
    - `next_age_out(&self) -> Option<(f64, u64)>`: the minimum `t_end` and its seq, with ties going to the smallest seq;
    - `clear()`: capacity is reused.

**Steps:**

- [ ] **Step 1: Failing tests** (hand-worked; exact `assert_eq!` where the arithmetic is binary-exact, `close` at 1e-12 otherwise):
  - `q_shared_on_crit_per_hit_n_hits`: p 0.4, c 0.5, n 3, `per_hit` `on_crit`. q = 0.4·(1 − 0.5³) = 0.35, not the independent 1 − (1 − 0.2)³ = 0.488.
  - `q_on_crit_plus_on_non_crit_are_exclusive`: Shared, p 0.25, c₁ 0.5 `on_crit`, c₂ 0.5 `on_non_crit`, `per_cast`. q = 0.25·0.5 + 0.75·0.5 = 0.5, not 1 − 0.875·0.625.
  - `q_independent_mixes_per_cast_and_per_hit`: n 2, p 0.5, `on_crit` `per_cast` c 1.0 (fires iff k > 0). q = 1 − P(k=0) = 0.75.
  - `q_indep_only_for_utility_cast`: `CritModel::None` with `on_cast` c 0.3. q = 0.3.
  - `q_chance_clamps_above_one`: c 1.7 gives q = 1.0.
  - `log_constant_d_matches_product`: push (q 0.5, D 2.5) at t = 1, 2, 3. `p_at(3.2)` = 0.875; after `drop_expired(3.5)`, `p_at(3.6)` = 0.75.
  - `log_variable_d_latest_wins_drop_is_exact`: (t 0, q 0.5, D 10), then (t 1, q 0.5, D 1). `p_at(1.5)` = 0.75. After `drop_expired(2.0)`, `p_at(5.0)` = 0.25. This is the reviewer's case; the naive answer is 0.5.
  - `log_q_one_is_binary`: a q-1 push sets P = 1 and zeroes every older weight.
- [ ] **Step 2: Run** `cargo test -p rtce expected::`. Expected: unresolved items (RED).
- [ ] **Step 3: Implement** the signatures above.
- [ ] **Step 4: Run** the same command. Expected: green. Mutation-check `log_variable_d_latest_wins_drop_is_exact` by making `drop_expired` renormalize, and confirm the pin fails.
- [ ] **Step 5: Commit.** Message: "ev_uptime: per-cast q and latest-wins roll log".

### Task 3: Executor, logging, P and age-out

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - `BuffRt` gains `log: RollLog`, `p: f64`, `p_seconds: f64`, `p_since: f64` and `age_generation: u64`;
  - new `Event::ExpectedAgeOut { buff: usize, generation: u64 }`, with a `class_rank` arm equal to `BuffExpire`'s and a `horizon_drain_error` arm;
  - a new `Sim::schedule_with_seq(time, event, seq)`, which `schedule` delegates to.
- Test: the `exec.rs` test module.

**Interfaces:**
- Consumes: Task 1's `expected_buffs` and `ev_uptime`; Task 2's `NoFire`, `CritModel`, `q_of` and `RollLog`.
- Produces:
  - `Sim::log_expected(&mut self, bi: usize, q: f64, duration: f64) -> Result<(), PlanError>`. It takes a seq and pushes the entry, wrapped in the `flush_before_change` / mutate `p` / `refresh_after_change` transaction, then reschedules the age-out.
  - `Sim::handle_expected_age_out(bi, generation)`. It drops every entry with `t_end <= now` in one call, recomputes `p` under the same transaction, and reschedules at `next_age_out()` with that entry's stored seq.
  - `BuffRt.p`: the cached current P that Task 4 reads.

**Wiring (EV only, `self.rng.is_none()`):**
- In the cast-event roll block (the `else` branch around `roll_procs_ev`), keep one `NoFire` per expected buff for the duration of the block. Use a small fixed-capacity scratch indexed by buff, reused, with no allocation per cast. `roll_procs_ev` adds each considered proc's `(trigger, rolls, clamped base, n)` into the buffs in its `expected_buffs`.
  - Capture D at the first contributor, before the accumulator step.
  - After the four trigger loops, call `log_expected(bi, q_of(..), d)` for each buff touched, where the `CritModel` is built from the same `crit_chance` and `independent_n` the block already computed.
- `run_proc_effects`: under EV, skip `ApplyBuff(bi)` when the buff is expected. Under MC, leave it unchanged.
- `apply_buff` on an action path, under EV with an expected buff: evaluate D as today, then call `log_expected(bi, 1.0, d)` instead of the instance logic.
- `flush_before_change`: integrate `p_seconds += (now − p_since)·p` for each expected buff.
- `finalize` and the report: for an expected buff, `uptime = avg_stacks = p_seconds / duration`, and `applications` = the number of log pushes, documented on `BuffReport`.

**Steps:**

- [ ] **Step 1: Failing tests:**
  - `ev_uptime_expected_hand_worked_uptime`: a 1 s `cast_time` damaging action, an `on_hit` proc with chance 0.5 applying X, D 2.5, duration 10.
    - Expected mode: `buffs["x"].uptime` = 0.69375, because ∫P = 0.5 + 0.75 + 7·0.8125 = 6.9375.
    - The same config with `discrete`: 0.8 (fires at 2, 4, 6, 8, 10, giving a continuous window from 2 to 10). Assert both.
  - `ev_uptime_expected_q_one_equals_discrete`: chance 1.0 with D 2.0 and 1 s casts, so an age-out lands exactly on a cast completion. Run under `EventOrder::Scheduled` and `CompletionsFirst`. Under each order, the `expected` report equals the `discrete` report field for field (`uptime`, `condition_uptime`, damage). Give X a condition feeding damage, so the coincident ordering is observable.
  - `ev_uptime_other_proc_effects_unmoved`: a proc applying X and granting a resource gain. The resource's total gained, and the proc `fire_count`, are identical between `expected` and `discrete`. (Review Focus 1.)
  - `ev_uptime_never_applied_is_zero`: no proc ever qualifies, so uptime is 0 and no `ExpectedAgeOut` is scheduled. (Review Focus 3.)
  - `ev_uptime_equal_t_end_drains_once`: two cast events at the same instant whose entries end together. A single age-out handles both, and the run stays well under the horizon drain limit. (Review Focus 4.)
  - `ev_uptime_segments_log_per_landing`: a 2-segment action with an `on_hit` proc applying X, giving two entries per cast at the two landing instants. Hand-work the uptime. (Review Focus 2.)
  - `ev_uptime_ignored_in_mc`: an MC run with 50 iterations and a fixed seed has an identical `SimReport` under `expected` and `discrete`.
- [ ] **Step 2: Run** `cargo test -p rtce ev_uptime_`. Expected: the hand-worked test fails (it reports 0.8, which is the RED).
- [ ] **Step 3: Implement** the wiring above.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: green, and `diablo4_rotation` byte-identical. Mutation-check the q-1 pin: give the age-out a fresh seq instead of the stored one, and confirm it fails under `Scheduled`.
- [ ] **Step 5: Commit.** Message: "ev_uptime: EV roll log, P and age-out events", carrying the hand-worked 0.69375 vs 0.8 contrast.

### Task 4: Readers (conditions, contributions, live DoT)

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs`:
  - `condition_value`: if `condition_expected` names an expected buff `bi`, return `p·v + (1−p)·static`, where `static` is the current phase uptime, clamped as today;
  - `refresh_effective_state`: an expected buff's contributions are folded with `value × p`. It is not in `active_buff_set`;
  - the live tick rate in `refresh_after_change` is `eval_objective × p`, and `flush_ticks` includes expected buffs with a live tick.
- Test: the `exec.rs` test module.

**Interfaces:**
- Consumes: Task 3's `BuffRt.p` and the `log_expected` transaction (which already refreshes, so readers update at each roll and age-out).

**Steps:**

- [ ] **Step 1: Failing tests:** one discriminating test per reader cell, each against Task 3's 0.5 / 1 s / D 2.5 timeline. Name the expected values from P's piecewise values (0.5, 0.75, 0.875, 0.75 …), and assert that `discrete` differs:
  - `ev_uptime_condition_blends_with_static`: X drives `recently = 1` and the static uptime is 0.2. `condition_uptime["recently"]` = (6.9375 + 0.2·(10 − 6.9375)) / 10 = 0.755.
  - `ev_uptime_condition_reblends_at_phase_boundary`: the static uptime moves from 0.2 to 0.6 at t = 5. Hand-work it. (Review Focus 5.)
  - `ev_uptime_contribution_scales_by_p`: X adds `+100` to a `sum` bucket feeding damage. Each cast's damage at completion uses the P at that instant. Hand-work the total from the per-cast P sequence.
  - `ev_uptime_live_tick_scales_by_p`: X ticks a live objective of rate 10, so the DoT damage is 10 · 6.9375 = 69.375.
- [ ] **Step 2: Run.** Expected: they fail (the readers ignore P).
- [ ] **Step 3: Implement** the three readers.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: green. Mutation-check the condition pin by dropping the `(1−p)·static` term.
- [ ] **Step 5: Commit.** Message: "ev_uptime: conditions, contributions and live DoT read P".

### Task 5: EV-vs-MC regression, docs, release notes

**Files:**
- Test: `crates/rtce/src/sim/exec.rs` test module, or the existing EV/MC regression location: wherever `ev_and_mc_agree_under_per_hit_on_crit_regression` lives.
- Modify:
  - `docs/guide/03-events-and-branches.md`: a short section next to the EV/MC proc text, with the issue's example and when to use the field;
  - `CHANGELOG.md` (Unreleased);
  - `ROADMAP.md`: remove the "#33 item 4" open-debt line, and add any deliberately unpinned cell as debt.

**Steps:**

- [ ] **Step 1: Regression** `ev_and_mc_agree_on_expected_uptime_regression`, the issue's shape:
  - `on_crit` proc with chance 1.0, one hit per cast, crit p 0.23, a 0.33 s cast, D 4, duration 300.
  - Assert that EV `expected` uptime is within 3σ of MC's mean, using 2,000 iterations and the MC distribution's std / √N, or the existing regression's tolerance idiom.
  - Assert that EV `discrete` uptime is outside that band (it reports about 1.0).
  - Assert that the steady state is about 1 − 0.77^(4/0.33).
  - Run it with RED captured against `discrete`.
- [ ] **Step 2: Docs and notes** as listed. Every number in the guide must match a pin.
- [ ] **Step 3: Run the standing gates** (Global Constraints), all of them, with the exact outputs recorded in the PR body.
- [ ] **Step 4: Commit** ("ev_uptime: EV/MC regression, guide, changelog"), push, and open the PR with auto-merge. Close #33 in the PR body ("Closes #33").
