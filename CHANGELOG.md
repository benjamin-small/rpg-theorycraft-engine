# Changelog

All notable changes to `rtce` and `rtce-testkit` are documented here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows [SemVer](https://semver.org/) once published (0.x
until then, per semver's "anything goes" pre-1.0 clause).

## [0.1.0] — 2026-07-22

Initial release. Extracted from the proven patterns of `diablo4-calc`
and `poe2-calcs`; parity-proven against `diablo4-calc`'s 7 archetype
builds (bit-identical, the standing numbers 8,096.02 … 6,769.10), which
now runs solely on `rtce` in production, including in the browser via
WASM.

- **P1 — expression engine.** A small arithmetic language (`+ - * /`,
  unary minus, `min`/`max`/`clamp`/`floor`, parenthesized grouping)
  compiled through lex → parse → emit into a flat postfix `Program`
  evaluated over a slot array with a fixed-size stack — zero allocation
  on the hot path. Fail-closed throughout: unknown identifiers and
  syntax errors carry a byte position and never guess.
- **P2 — damage model.** `GameDef` (stat/bucket/event/pipeline registry),
  `BuildState` (one candidate's stat values and tagged contributions),
  and `Scenario` (weighted phases with condition uptimes) as `serde`
  types; `plan::compile` turns a `GameDef` into a `Plan`; `Plan::evaluate`
  folds contributions into buckets (sum / summed-group / product),
  enumerates event branches, and blends phases by weight — allocation-free
  once `EvalScratch` is set up.
- **P3 — a real game runs on config.** `diablo4-calc`'s Sorcerer damage
  model (T1–T10) reproduced entirely as `rtce` configuration, mutation-proven
  on both the independent and vulnerable-fold-gate axes.
- **P4 — parity proof.** All 7 of `diablo4-calc`'s archetype builds
  evaluate bit-identical through `rtce` and through `diablo4-calc`'s
  native engine; `diablo4-calc` then switches over (P4c) to run
  **solely** on `rtce` — its native damage math deleted, `calc::evaluate`
  a thin shim over an `rtce`-compiled plan, re-verified on wasm32.
- **P5 — search module.** `search::price` applies reversible `Move`
  sequences (`SetStat`, `AddContribution`, `RemoveContribution`) to a
  baseline `BuildState` without mutating it, evaluates each resulting
  candidate against a batch of scenarios, and `search::top_k` /
  `search::pareto` rank the results. Serialization-friendly by design so
  a driver can generate candidates out-of-process.
- **`Plan::explain()`.** A teaching path running the identical evaluation
  engine as `evaluate` with per-phase/per-stage/per-branch tracing turned
  on — allocates freely; the hot `evaluate` path never takes this branch.
- **Maturation.** `rustfmt` baseline; `#![warn(missing_docs)]` clean
  across both crates with a crate-level doctest; a runnable
  `examples/your_own_game.rs` walkthrough; MIT OR Apache-2.0 licensing
  with crates.io-ready package metadata; GitHub Actions CI (test +
  clippy + fmt).

[0.1.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.1.0
