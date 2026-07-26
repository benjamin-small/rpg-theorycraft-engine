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
      3753.31848 / 0.4) byte-identical. (The trick still appears in two
      `sim::exec` test fixtures, where it is deliberate — those fixtures
      exist to exercise the PROC path.)
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
