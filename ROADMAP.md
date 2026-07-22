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
- [ ] `ActionDef.apply_buff` (or an action-scoped proc) — 0.3.0. Today the
      only way to apply a buff from a specific action is the "icd equals
      the gating action's cooldown" trick `examples/diablo4_rotation.rs`'s
      `nova_pulse` relies on (a proc rolls on EVERY action's on-cast event;
      matching its icd to one action's cooldown is how the config coerces
      "only Frost Nova drives this buff" out of a trigger that's really
      global). That trick is load-bearing for the current example and
      correct for it, but it's a config-author trap for any other
      cadence — a first-class per-action buff application removes the
      need for it entirely.
- [ ] Sim per-cast allocation trims (P6 review, non-functional). An
      overlay-build cache for actions whose `damage.stats` is empty (no
      overlay to build — `overlay_build_for_action` still clones the full
      effective build today); folding the EV `on_crit` crit-chance query
      (`eval_action_crit_chance`) into the same `Plan` call
      `eval_action_damage` already makes, instead of a second pass.

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
