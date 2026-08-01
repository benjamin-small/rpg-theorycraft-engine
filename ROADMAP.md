# Roadmap

## Proven
- P1 expression engine · P2 damage model · P3 D4-as-config · P4 parity
  proof (7 real builds identical through diablo4-calc's native engine and
  rtce — the standing numbers 8,096.02 … 6,769.10) · **P4c diablo4-calc
  switchover** — native `calc.rs`/`stats.rs` math deleted, `calc::evaluate`
  is a Breakdown shim over `rtce_adapter` + the compiled `gamedef/`
  stage objectives, WASM/web re-verified at 8,096.02 (rtce proven on
  wasm32, not just native), consumer's full suite (120 tests/21 suites)
  green.
- **Maturation** (before any announcement) — explain() per-stage traces;
  P5 search module (Move, batch pricing, top-K/Pareto); rustdoc pass
  (`#![warn(missing_docs)]` clean, crate-level doctest); a runnable
  `examples/your_own_game.rs` walkthrough; MIT OR Apache-2.0 licensing
  with crates.io-ready package metadata (`cargo publish --dry-run`
  clean for both crates); GitHub Actions CI (test + clippy + fmt).
  (`your_own_game.rs` was retired in the Unreleased docs guide — its
  walkthrough is now chapters 1–4 of `docs/guide/`, grown one concept at
  a time, with pins superseded 148.20/113.28 → 229.71/175.584.)
- **P6 — sequencing, DONE (0.2.0).** Expression predicates (comparisons +
  `and`/`or`/`not`); `SimDef`/`Rotation` config + `sim::compile`'s
  fail-closed extended symbol space; a discrete-event `sim::run` sharing
  ONE decision loop across `Mode::Expected` (keystone-proven to agree
  with `Plan::evaluate` exactly) and `Mode::MonteCarlo` (seeded, a new
  in-crate PCG32, and per-cast sampled evaluation inside `Plan`);
  `SimReport` with COMPUTED
  buff/condition uptimes and resource health in place of `Scenario`'s
  asserted ones; `examples/diablo4_rotation.rs` (mana, a spender/generator
  pair, a proc-gated buff window, hand-worked EV + Monte Carlo pins),
  CI-run. Published at `rtce` 0.2.0 (`cargo publish --dry-run` clean).
- **P7 — PoE2 test bed + instance mechanics, DONE (0.3.0).** A SECOND
  consumer proves the engine is not shaped around one game: `poe2-calcs`
  carries a generated 209-stage `GameDef` + adapter reproducing its native
  calculator to 1e-9 across 63 parity tests (124.53 / 129.51 / 793.76),
  with a 156-pair `(StatId, ModKind)` sweep against silent routing drift.
  That harness lives in THAT repo and its native math is untouched — a
  proof, not a switchover (P7a).

  Engine growth, all of it config-visible: `NumOrExpr` expression-valued
  sim fields with documented evaluation instants (P7b); buffs as INSTANCE
  LISTS — `max_stacks`, `on_reapply` (`refresh` / `add_refresh_all` /
  `add_independent` / `strongest`), the `stacks.<buff>` symbol, and
  `SimReport::buffs` carrying `uptime` + `avg_stacks` (P7c-T1); snapshot
  DoTs, where an instance captures its rate at application and ticks it
  unchanged to expiry (P7c-T2); `ActionDef::apply_buff` and
  `ProcDef::actions` trigger filters, which together retire the "icd
  equals the gating action's cooldown" trick the ROADMAP itself had called
  a config-author trap — `examples/diablo4_rotation.rs` is off it with EV
  pins byte-identical (P7d); and three PoE2 slices on a committed trimmed
  PoE2-shaped fixture, `poe2_charges` / `poe2_poison` / `poe2_triggers`,
  each with hand-derived pins and contrast runs (P7e).

  Seven behavior fixes, all CHANGELOG'd with what kind of config each
  affects: the horizon drain (below), EV's `on_crit` weight measured
  before the cast's own procs, per-proc slot refresh, resource
  `max`/`regen_per_sec` re-derived at every refold, the `apply_buff`
  snapshot overlay frozen on both action paths, `ProcDef::icd` validated
  fail-closed (NaN used to DELETE the internal cooldown silently), and
  `SimReport::condition_uptime` clamped to `[0,1]` for buff-driven values.

  Plus a release-review pass: `#[non_exhaustive]` swept across every
  engine-produced type (the last free window for it), and three
  previously-undocumented composition rules pinned — `on_hit` rolls per
  CAST not per hit, two buffs on one condition resolve by buff NAME, and
  a same-list `apply_buff` capture reads a frozen BUILD against a LIVE
  PHASE. 181 tests green (173 in the `rtce` lib),
  `cargo publish -p rtce --dry-run` clean at 0.3.0.

  **The horizon-drain bug (found P7e, fixed P7e-T2).**
  `sim::exec::run_loop` processed at most ONE event at `t == duration`: it
  popped an event, set `self.time`, handled it, then broke on
  `self.time >= self.duration` — so any OTHER event already queued for
  that instant was silently discarded, and which one survived was decided
  by the heap's `(time, seq)` tie-break. In practice a `BuffExpire`
  scheduled at the fight's end swallowed the `CastComplete` there,
  dropping that cast whole: its count, its damage, and its `apply_buff`. A
  cast completing at `duration` DID count when it was alone on the instant
  (`diablo4_rotation`'s 60th cast is), so this was never "the horizon
  excludes its boundary" — it was order-dependent silent damage loss.

  Repro, now the regression pin — one 1s filler applying a `refresh` buff,
  10s fight: 9 casts at buff durations 2 / 5 / 9, 10 at 9.5. (An earlier
  wording of this entry said "9 casts for ANY integer duration", which was
  never right: at a duration ≥ 10 no expiry lands on t=10 at all, and the
  count is a correct 10.)

  OUTCOME: the horizon is now DRAINED — no cast BEGINS at or after
  `duration`, every event already scheduled AT `duration` is processed, so
  a cast completing exactly at `duration` counts. Zero pins moved anywhere
  in the repo, `diablo4_rotation` byte-identical (225199.1088 /
  3753.31848 / 0.4, MC block included), both consumers byte-identical.
  Mutation evidence: reinstating the break drops both horizon pins to 9
  casts; removing the "no cast begins at the horizon" guard fails three
  pre-existing pins. The drain's bound (`HORIZON_DRAIN_LIMIT`) is
  reachable via trailing zero-weight phases and pinned by
  `too_many_zero_weight_phases_at_the_horizon_fails_closed`.

  The three PoE2 slices KEEP their half-integer buff durations, but for
  the OTHER reason: measurement showed the rationale was the mid-fight
  `seq` ordering (see the 0.4.0 question below), not this bug. Integer
  durations would reshape `poe2_charges`' cycle and cost `poe2_triggers`
  15.5% of bolt damage — three lessons in event ordering instead of three
  lessons in charges/poison/triggers.

## Next
- [x] ~~**Publish 0.3.0** to crates.io~~ — DONE: `rtce` 0.3.0 is live on
      the registry (`cargo search` confirms), alongside `rtce-testkit`
      0.1.0. The mechanics recorded here carry forward to every release:
      rtce's dev-dependency pins testkit by `version` alongside its
      path, so a published tarball's tests resolve testkit from the
      registry (locally the path wins); testkit bumps only when a commit
      actually touches `crates/rtce-testkit/` (none since before P7 —
      it stays 0.1.0 for 0.4.0 too).
- [ ] **Publish 0.4.0** (staged by P8f: version bumped, CHANGELOG cut,
      `cargo publish -p rtce --dry-run` clean). Publishes after the
      whole-phase P8 review round — the coordinator's step, not a task's.
- [ ] Sim per-cast allocation trims (P6 review, non-functional). An
      overlay-build cache for actions whose `damage.stats` is empty (no
      overlay to build — `overlay_build_for_action` still clones the full
      effective build today). The second half of this item is DONE as a
      side effect of P7b: the EV `on_crit` crit-chance query no longer
      builds a second overlay — `Sim::capture_world` (né `measure_cast`)
      builds ONE per cast and both queries read it. P8c added one `Phase`
      clone per measured cast (the `WorldSnapshot`'s phase half) —
      accepted: the utility action's build clone MOVED into the same
      capture rather than multiplying, and the phase clone is what buys
      the one-world invariant; fold it into a future cache pass with the
      overlay-build one if a bench ever surfaces it. It is still a second `Plan` call (folding
      it into `eval_action_damage`'s call remains open), but it is no
      longer a second overlay, and it now happens at the cast's own
      instant rather than after the cast's procs — a correctness fix, not
      just a trim (see the 0.3.0 notes).
- [ ] Cache a buff's `min`/`max` instance expiry on `BuffRt` (P7c-T1
      review, non-functional). `Sim::earliest_expiry` and
      `Sim::longest_remaining` each scan the whole instance list, the
      latter on every `refresh_time_varying_slots` — i.e. before every
      expression evaluation, for every buff. Deferred out of P7c-T1 to
      P7c-T2 on the theory that the unbounded poison fixture would be the
      witness that made it matter. **It was not.** That fixture applies
      one instance per second against a 4s duration, so the list never
      holds more than 4: the length is bounded by duration ÷ application
      cadence, and `max_stacks: 0` does not by itself make a list long.
      P7c-T2 added a third whole-list scan (`Sim::snapshot_total`, once
      per refold — and a second time per APPLICATION in debug builds, via
      the fold-gate assertion in `Sim::apply_buff`) on the same tiny
      lists. Left deferred, and now deliberately WITHOUT a nominated
      witness — revisit when a config actually drives a long-duration,
      high-frequency stack (hundreds of instances). Measure first: there
      is no bench harness in this repo yet, so the first step is a
      `benches/` entry, not an optimization.

## Open for 0.4.0
These accumulated during P7 and its release review, and were deliberately
NOT decided in 0.3.0. Three groups:

- **Config-compatibility changes** (the `apply_buff` arity, the
  `ProcDef::actions` shape, the `deny_unknown_fields` sweep) — they
  reject or reshape configs that parse today, so they want ONE slice with
  a single migration note.
- **Open design questions** (the `CastComplete`/`BuffExpire` ordering,
  the per-stack `product` fold, the frozen-vs-live capture phase, whether
  `on_hit` scales with `hits_per_use`) — today's answer is documented and
  pinned in each case, which is not the same as being right. None should
  be guessed at without a config that needs the answer.
- **API and coverage debt** (the unvalidated contribution VALUE,
  `SimScratch`, the `ProcDef` effect arity, the untested
  `refresh`+live-DoT path, `expr::MAX_STACK`).

- [x] ~~Harmonize `apply_buff`'s ARITY between `ActionDef` (a
      `Vec<String>` list) and `ProcDef` (a single `Option<String>`)~~ —
      **landed in P8b** (0.4.0), by a different shape than the
      untagged string-or-list this entry guessed at: the ordered
      `effects` list (`"effects": [ { "apply_buff": … }, … ]`) is one
      list form on BOTH entities, and the old fields stay as 0.x sugar
      that desugars into it at `sim::compile` (mixing sugar with an
      explicit list on one entity is an "ambiguous order" compile
      error). See the CHANGELOG's 0.4.0 entry.
- [ ] `cast_action` effects on ACTIONS (combo chains) — deferred, and the
      `sim::compile` error for one points at this entry. A proc-fired
      free cast cannot recurse because a free cast rolls no procs; an
      ACTION free-casting an action has no such natural bound (A→B→A),
      so allowing it means choosing a bounded-recursion design — depth
      cap, visited-set, or an explicit chain type — and that choice
      should be driven by a real combo config (PoE2 trigger chains are
      the likely candidate), not guessed at. Until then `cast_action`
      stays proc-only, enforced fail-closed on `ActionDef::effects`.
- [ ] Remove the 0.x effect sugar (`ProcDef::apply_buff`,
      `ProcDef::cast_action`, `ActionDef::apply_buff`) — 1.0 at the
      earliest: P8b's "kept for 0.x" is a promise with an expiry, and
      this entry is what tracks it. Removal is config-breaking (every
      0.2.0/0.3.0 config is written in the sugar), so it wants its own
      migration note; the desugar makes migration mechanical
      (`"apply_buff": ["a", "b"]` becomes
      `"effects": [ { "apply_buff": "a" }, { "apply_buff": "b" } ]`).
      Recorded decision (P8b review): NO `#[deprecated]` attribute in
      the meantime — the crate's own tests and serde's derived code read
      the fields (every site would need `#[allow(deprecated)]`), and the
      deprecation is a JSON-config-surface concern rustc cannot warn a
      config author about anyway; the rustdoc DEPRECATED notes plus the
      CHANGELOG are the advertised channel.
- [ ] `ProcDef::actions` expressiveness (P7d review) — 0.4.0 candidate,
      and only if a real config asks. Today the filter is an inclusive
      list of CASTING actions: no negation, no "every action except"
      (which has to be spelled as the complementary list and kept in step
      by hand), and no way to say "on_hit, but only hits of actions other
      than this one". Deliberately not guessed at in 0.3.0 — the right
      shape should be chosen by a config that needs it, most likely one
      of the PoE2 trigger slices.
- [x] ~~Crate-wide `deny_unknown_fields` on the config structs~~ —
      **landed in P8a** (0.4.0), though NOT at the serde layer:
      unknown keys are collected and rejected at `plan::compile`/
      `sim::compile` with a did-you-mean (or at parse via hand-written
      `Deserialize` for the seven structs both consumers construct in
      Rust with exhaustive literals), and `_`-prefixed keys are the
      documented annotation namespace at every nesting level. The
      untagged-enum half landed too: `NumOrExpr` and the
      `tick_objective` object form now report what was expected via
      hand-written `Deserialize` (the `TickObjectiveObj` +
      `deny_unknown_fields` machinery is gone). See the CHANGELOG's
      0.4.0 migration note.
- [x] ~~**Should `CastComplete` out-rank a coincident `BuffExpire`?**~~
      (P7e-T2 review) — **answered BY CONFIG in P8d** (0.4.0):
      both orders are legitimate semantics, so neither was hardcoded —
      `defaults.event_order` picks one. `"scheduled"` (the default) is
      the honest name for the long-standing behavior: same-instant
      events resolve in scheduling (`seq`) order, so a buff whose window
      closes exactly when the cast that would refresh it completes
      expires FIRST (it was scheduled back at the last application, so
      it holds the lower `seq`) — and that cast measures itself without
      the buff it is about to re-apply. `"completions_first"` is the
      other reading: every `CastComplete` outranks every coincident
      `BuffExpire`/`PhaseBoundary`/`Wake`, `seq` still breaking ties
      within a class (seeded MC stays deterministic under both). The
      "second ordering key on the queue" the 0.3.0 note below said this
      would need is exactly what was built: `QueueItem` orders by
      `(time, class_rank, seq)`, the rank computed at push and CONSTANT
      under the default (bit-identical — the byte-identical
      `diablo4_rotation` MC block is the proof). Pinned consequence,
      stated as designed: the zero-weight-final-phase horizon cast,
      whose `scheduled` attribution (boundary first — 900/250/1150) the
      0.3.0 pin recorded as incidental, flips to 1000/0 under
      `completions_first` (measured under, and attributed to, the OLD
      phase). `poe2_triggers` runs the on-grid 2.0s shock under BOTH
      fixes — `measure: "cast_start"` (P8c) and `event_order:
      "completions_first"` (P8d) — each restoring 1837.5 → 2175 alone;
      the PoE2 slices keep their half-integer `representative`
      durations (the default order is still the default). The
      original case for deciding, kept below because its table still
      describes the DEFAULT:

      What makes it worth deciding, rather than leaving to config hygiene,
      is that the cost is INVISIBLE in the integrated column a reader would
      reach for. Both effects are PINNED as contrast runs (not merely
      measured — these numbers ship to docs.rs, so they carry tests), each
      changing exactly one duration:

      | example         | change                | integral            | damage           |
      | --------------- | --------------------- | ------------------- | ---------------- |
      | `poe2_triggers` | shock 2.5 → 2.0       | uptime 0.95 → 0.95  | bolt 2175 → 1837.5 |
      | `poe2_charges`  | `"4.5 + stacks"` → `"4 + stacks"` | avg_stacks 2.25 → 2.25 | total 11748 → 10875 |

      `poe2_charges` is the sharper case: the stack falls off on a cast
      instant, the rotation's `when` reads the lower count, and the whole
      cycle reshapes (12 generators / 28 spenders → 15 / 25) while
      `avg_stacks` still reports 2.25 to the last bit. (Its `uptime` does
      move, 0.85 → 0.875, since that gap is a full second rather than
      zero-width — it is the stack integral that goes blind there, and the
      uptime integral in the triggers case.)

      The three PoE2 slices' half-integer `representative` durations dodge
      this deliberately and now say so; `sim`'s module docs carry the
      warning under "A buff expiring on the cast grid".

      NOT changed in 0.3.0 on purpose: the ordering is long-standing 0.2.0
      behavior, it is orthogonal to the P7e-T2 horizon-drain fix (which is
      about WHICH events resolve at `t == duration`, not their order), and
      reordering would move numbers across the suite. If it changes it
      wants its own slice, a mutation-proven pin, and a migration note —
      `CastComplete`-before-`BuffExpire` cannot be expressed by `seq`
      alone and needs a second ordering key on the queue, the same
      machinery the P6 design notes declined for `End`. (That is the
      slice P8d became; the default DID NOT change, so no number moved.)
- [ ] **A per-stack `product` fold mode** (P7c-T1/P7e) — 0.4.0 candidate,
      and only if a real config asks. Today a stacked contribution scales
      its VALUE by the count, so 3 stacks of `+10` in a `product` bucket
      fold as `×1.30`, not `×1.10³`. That is CORRECT for "increased damage
      per charge" and it is documented on `BuffDef` and pinned by
      `poe2_charges` precisely so it stays a stated choice — but it
      linearizes a genuinely multiplicative per-charge effect (PoE2 would
      call three 10% frenzy charges `×1.331`), and such an effect is not
      expressible as one per-stack contribution at all. The workaround is
      to write it as a `sum` bucket, where linear IS the correct fold. A
      real fix means a fold mode that raises a member to the stack power,
      which is a `GameDef`-level addition (new `FoldKind`, or a per-
      contribution flag) rather than a `SimDef` one — so the shape should
      be chosen by a config that actually needs `×1.331`, not guessed at.
- [x] ~~**Should a same-list `apply_buff` capture freeze the PHASE
      too?**~~ — **answered YES in P8c** (0.4.0): a cast's
      `ApplyBuff` captures read the cast's ONE world snapshot — build
      AND phase, captured together at the action's resolved `measure`
      instant — so both orderings of `["mark", "poison"]` now capture
      the pre-list world. The 400-vs-800 pin became an
      equality-plus-literal pin,
      `a_same_list_snapshot_capture_reads_one_frozen_world` (both 400,
      the old poison-first value). This is the P8 phase's single
      deliberate behavior change; sim-FIELD expressions (`duration` et
      al.) keep their live sequential reads, and the proc path keeps its
      live ambient capture. See the CHANGELOG migration note for exactly
      which configs move.
- [x] ~~**Should `Trigger::OnHit` scale with `hits_per_use`?**~~ (0.3.0
      release review) — **answered BY CONFIG in P8e** (0.4.0): both
      readings are legitimate semantics, so neither was hardcoded —
      `defaults.proc_rolls` / `ProcDef.rolls` picks one. `"per_cast"`
      (the default) keeps the long-standing hits-blind roll,
      bit-identical (RNG stream included) and still pinned by
      `on_hit_rolls_once_per_cast_not_once_per_hit`; `"per_hit"` LOOPS
      the roll over the measured hit count — the loop spelling, chosen
      over "weight the accumulator by hits" precisely because the two
      are NOT equivalent under an ICD (the 0.3.0 note below was right):
      all hits of one cast share one instant, so any `icd > 0` caps
      fires at one per cast (per_hit == per_cast, pinned as an
      equality), while `icd: 0` permits multiple crossings per cast.
      Chance is evaluated once per proc per cast (one measured world);
      a fractional measured `hits_per_use` fails closed under
      `per_hit`. `ProcRolls`'s rustdoc is the canonical statement;
      `mod proc_rolls` in `sim::exec` carries the pins (4-vs-20
      fractional EV contrast, the 7==7 ICD equality, the per-hit
      ICD-bound EV/MC agreement regression).
- [x] ~~**`Contribution::value` is never checked for finiteness**~~ —
      **landed in P8a** (0.4.0): rejected on BOTH halves of the
      shared type (`Plan` build resolution for `BuildState.contributions`,
      `sim::compile` for `BuffDef.contributions`), alongside the same
      guard for `BuildState.stats` and `Phase.stats` values. The original
      note, kept for the repro: (0.3.0
      release review) — the one number in the sim config that P7b's
      evaluation-instant sweep and P7's `ProcDef::icd` fix both missed,
      and it SHIPS in 0.3.0. It is a bare `f64` on
      `crate::build::Contribution`; `sim::compile` clones
      `BuffDef::contributions` straight through into `CompiledBuff`
      (`crates/rtce/src/sim/compile.rs`, the `contributions:
      b.contributions.clone()` line), and `Plan` checks phase weights and
      phase uptimes only (`plan.rs`, the three `is_finite` sites) — so
      nothing on either level looks at the value.

      Repro, probed directly at the 0.3.0 release review: a one-stat
      `GameDef` (`hit = dmg * (1 + additive / 100)`, one `sum` bucket), a
      spammable action applying a buff whose single contribution is
      `{ "bucket": "additive", "value": <v> }`, run through `sim::run` in
      `Mode::Expected` over a 5s single-phase scenario. `v = 50.0` gives
      `Ok(700)`; `v = f64::NAN` gives `Ok(NaN)`; `v = f64::INFINITY` gives
      `Ok(inf)`. All three are `Ok` — the bad two produce a poisoned
      `SimReport` with no error anywhere, which is a strictly worse
      failure than the `icd` NaN that WAS fixed. For contrast, the
      neighbouring `BuffDef::conditions` map IS caught, with "phase `p`
      uptime `marked` must be finite, got NaN".

      Note the fix has two halves, because the type is shared: the same
      `Contribution` is also `BuildState::contributions`, i.e. Level-1
      config that `Plan::evaluate` reads. Validating only the sim half
      would leave `Plan::evaluate` accepting the same NaN. Not fixed in
      0.3.0 purely because it is a behavior change caught at release-review
      time; it is not a design question and wants no config to justify it.
- [x] ~~`sim::SimScratch` is public, constructible, and accepted by
      nothing~~ — **REMOVED from the public API in P8f** (0.4.0):
      "remove" won over "give `run` a `_with_scratch` variant", because
      no driver has asked for batch scratch reuse and dead surface
      should not wait for one. The type itself survives crate-internally
      (`pub(crate)`) — `run` still builds one per call — and its doc
      names the re-publication condition: if batch reuse ever earns a
      `run_with_scratch`, the type comes back AS a parameter something
      accepts. Grep-verified unused in both consumers before removal;
      CHANGELOG'd as the 0.4.0 breaking note.
- [x] ~~A `ProcDef` can do exactly ONE of `apply_buff` / `cast_action`,
      and the limitation is enforced but not stated on the type (0.3.0
      release review)~~ — **landed in P8b** (0.4.0): "allow both"
      won, as the ordered `ProcDef::effects` list — a proc that applies a
      buff AND free-casts is now one list with two entries, order
      explicit and pinned (the 0.2/0.1 order contrast). The exactly-one
      check generalized to "a proc must do something" (empty after
      desugar is the error; both SUGAR fields at once is still refused).
- [x] ~~Coverage gaps recorded rather than closed in 0.3.0~~ — **all
      three closed in P8f** (0.4.0), and none was hiding a bug:
      `refresh` + LIVE `tick_objective` is pinned behaviorally
      (`sim::exec`'s `mod live_dot` — the 500/0.4 two-window pin, plus a
      mid-window refold contrast that discriminates live from snapshot
      INSIDE the policy, 875 vs 750); a live tick runs under
      `Mode::MonteCarlo` with same-seed byte-determinism AND exact EV
      equality pinned against a deliberately BRANCHED tick objective
      (sampling it instead reads 487.5 — the mutation was run); and
      `strongest` ships `examples/poe2_ignite.rs` (CI-run, hand-worked
      1950/2400/1200 pins, the `refresh` re-capture contrast, and its
      first Monte Carlo coverage). With the `refresh`×live cell filled,
      every `(reapply policy × tick mode)` cell now has a discriminating
      test — the CLAUDE.md docs-discipline rule 3 audit is what
      confirmed the matrix.
- [x] ~~`expr::MAX_STACK` is unreachable in practice~~ — **documented
      honestly in P8f** (0.4.0). What was true: the constant is
      `pub` only inside the PRIVATE `expr::compiler` module — never
      re-exported, so unreachable as public API — while
      `Program::max_depth`'s public doc named it as though a reader
      could go look. What the entry got wrong: the failure mode itself
      IS reachable, at compile time — ~64 right-nested levels trip the
      positioned "expression too deep (stack > 64)" error (eval never
      checks depth precisely BECAUSE compile rejects first).
      `Program::max_depth` now states the actual relationship without
      naming the private constant, and the exact boundary is pinned:
      63 levels compile with `max_depth == 64`, 64 levels fail closed
      (`depth_guard_boundary_is_exactly_max_stack`).
- [ ] 0.5.0: `search::Candidate`/`search::Move` still silently IGNORE
      unknown keys (P8a spec review). Outside P8a's 16-struct config
      sweep — today every driver constructs them in-process, where Rust's
      field checking already applies — but the same silent-typo class the
      sweep closed opens again the moment a driver goes out-of-process
      and ships candidate sets as JSON. Give them the P8a treatment
      (stored `extra` + walk, or parse-time mirror) when that happens.

## Deferred out of P6 (v1 sequencing scope)
- **Multi-target/AoE.** Packs stay approximated by target-profile stats;
  the timeline executor has no notion of more than one target.
- **Batch sim pricing inside `search`.** `search::price` still only
  batches `Plan::evaluate`; pricing a candidate set through `sim::run`
  (EV or MC) is a driver-side loop today, not a first-class search API.
- **Movement/mechanics scripting** beyond phase boundaries (e.g.
  in-fight repositioning, scripted add waves) — phases remain the only
  way to vary conditions mid-fight.

## Explicitly out of scope
- Knowledge-graph construction (external drivers).
