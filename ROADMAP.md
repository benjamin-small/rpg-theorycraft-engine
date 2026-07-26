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
- **P6 — sequencing, DONE (0.2.0).** Expression predicates (comparisons +
  `and`/`or`/`not`); `SimDef`/`Rotation` config + `sim::compile`'s
  fail-closed extended symbol space; a discrete-event `sim::run` sharing
  ONE decision loop across `Mode::Expected` (keystone-proven to agree
  with `Plan::evaluate` exactly) and `Mode::MonteCarlo` (seeded, a new
  in-crate PCG32, `Plan::evaluate_sampled`); `SimReport` with COMPUTED
  buff/condition uptimes and resource health in place of `Scenario`'s
  asserted ones; `examples/diablo4_rotation.rs` (mana, a spender/generator
  pair, a proc-gated buff window, hand-worked EV + Monte Carlo pins),
  CI-run. Published at `rtce` 0.2.0 (`cargo publish --dry-run` clean).

## Next
- [x] **BUG: `sim::exec::run_loop` processed at most ONE event at `t ==
      duration`** — found in P7e, FIXED in P7e-T2 (0.3.0). The loop popped
      an event, set `self.time`, handled it, then broke on `self.time >=
      self.duration` — so any OTHER event already queued for that same
      instant was silently discarded, and which one survived was decided
      by the heap's `(time, seq)` tie-break (first scheduled wins). In
      practice a `BuffExpire` scheduled at exactly the fight's end
      swallowed the `CastComplete` there, dropping that cast whole: its
      `casts` count, its damage, and its `apply_buff`. A cast completing
      at `duration` DID count when it was the only event there
      (`diablo4_rotation`'s 60th cast is), so this was never "the horizon
      excludes its boundary" — it was order-dependent silent damage loss.

      Repro, now the regression pin — one 1s filler applying a `refresh`
      buff, 10s fight: 9 casts at buff durations 2 / 5 / 9, 10 at 9.5.
      (The earlier wording here said "9 casts for ANY integer duration",
      which was never right: at a duration ≥ 10 no expiry lands on t=10 at
      all, and the count is a correct 10.)

      OUTCOME: the horizon is now DRAINED — no cast BEGINS at or after
      `duration`, every event already scheduled AT `duration` is
      processed, so a cast completing exactly at `duration` counts. Zero
      pins moved anywhere in the repo, `diablo4_rotation` byte-identical
      (225199.1088 / 3753.31848 / 0.4, MC block included), both consumers
      byte-identical. Mutation evidence: reinstating the break drops both
      horizon pins to 9 casts; removing the "no cast begins at the
      horizon" guard fails three pre-existing pins. The drain's bound
      (`HORIZON_DRAIN_LIMIT`) is reachable via trailing zero-weight phases
      and pinned by
      `too_many_zero_weight_phases_at_the_horizon_fails_closed`.

      The three PoE2 slices KEEP their half-integer buff durations, but
      for the other reason: measurement showed the rationale was the
      mid-fight `seq` ordering below, not this bug. Integer durations
      would reshape `poe2_charges`' cycle and cost `poe2_triggers` 15.5%
      of bolt damage — three lessons in event ordering instead of three
      lessons in charges/poison/triggers.
- [ ] Publish: GitHub repo, then crates.io (`rtce`, `rtce-testkit`) — the
      API survived the P4c switchover; semver honesty: 0.x until publish.
      Publish `rtce-testkit` first (no rtce-workspace deps of its own);
      once it exists on crates.io, consider pinning
      `rtce-testkit = { path = ..., version = "0.1.0" }` as rtce's
      dev-dependency so `cargo test` also builds from the published
      tarball (path-only today because that version pin can't resolve
      against the registry before the first real publish).
- [x] `ActionDef.apply_buff` + action-scoped procs — DONE in P7d
      (0.3.0). An action applies its own buffs at cast complete, and a
      `ProcDef` can name the actions its trigger considers, so the "icd
      equals the gating action's cooldown" trick is no longer the only
      way to bind an effect to one action. `examples/diablo4_rotation.rs`
      is off it: `frost_nova` carries `apply_buff: ["vuln_window"]` and
      the `nova_pulse` proc is deleted, with the EV pins (225199.1088 /
      3753.31848 / 0.4) byte-identical, and the trick's own leak
      documented (its icd let a SEVENTH application through at the fight
      boundary, on an action that was never Frost Nova). Two `sim::exec`
      fixture BUILDERS still apply their buff from a proc, deliberately —
      they are about stacks/DoTs, `apply_buff` cannot express their
      varying `chance`, and keeping them keeps the proc application path
      covered.
- [ ] Sim per-cast allocation trims (P6 review, non-functional). An
      overlay-build cache for actions whose `damage.stats` is empty (no
      overlay to build — `overlay_build_for_action` still clones the full
      effective build today). The second half of this item is DONE as a
      side effect of P7b: the EV `on_crit` crit-chance query no longer
      builds a second overlay — `Sim::measure_cast` builds ONE per cast
      and both queries read it. It is still a second `Plan` call (folding
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
- [ ] Harmonize `apply_buff`'s ARITY between `ActionDef` (a
      `Vec<String>` list) and `ProcDef` (a single `Option<String>`) —
      0.4.0, alongside the `deny_unknown_fields` sweep below, since both
      are config-compatibility changes. Same key, same concept, different
      shape: a config author who learns the list form on an action gets a
      serde type error writing it on a proc, and vice versa. Fixing it
      means accepting BOTH spellings in BOTH places via an untagged
      string-or-list, which is additive for JSON but changes the Rust
      field types.
- [ ] `ProcDef::actions` expressiveness (P7d review) — 0.4.0 candidate,
      and only if a real config asks. Today the filter is an inclusive
      list of CASTING actions: no negation, no "every action except"
      (which has to be spelled as the complementary list and kept in step
      by hand), and no way to say "on_hit, but only hits of actions other
      than this one". Deliberately not guessed at in 0.3.0 — the right
      shape should be chosen by a config that needs it, most likely one
      of the PoE2 trigger slices.
- [ ] Crate-wide `deny_unknown_fields` on the config structs (P7c-T2
      review, fail-closed hygiene). P7c-T2 put the guard on
      `TickObjectiveObj` only. The larger hole is one level up: a
      misspelled `tick_objectiv` on `BuffDef` is silently ignored and
      silently means "this buff has no DoT" — a quieter wrong answer than
      the typo the local guard catches. Applying it to `GameDef`/
      `SimDef`/`BuildState`/`Scenario` and friends is a compatibility
      decision (it rejects configs that parse today), so it wants its own
      slice and a 0.4.0 note. Alongside it: untagged enums (`NumOrExpr`,
      `TickObjectiveRepr`) report "data did not match any variant" rather
      than the inner field error, which is positioned but unhelpful —
      worth a hand-written `Deserialize` if the sweep happens.
- [ ] **Should `CastComplete` out-rank a coincident `BuffExpire`?**
      (P7e-T2 review) — 0.4.0 question, OPEN, deliberately not decided in
      0.3.0. Same-instant events resolve in scheduling (`seq`) order, so a
      buff whose window closes exactly when the cast that would refresh it
      completes expires FIRST (it was scheduled back at the last
      application, so it holds the lower `seq`) — and that cast measures
      itself without the buff it is about to re-apply. Arguably wrong: a
      cast that refreshes a buff at the instant it lapses should plausibly
      keep it up, and a player would never describe that frame as a gap.

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
      machinery the P6 design notes declined for `End`.

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
