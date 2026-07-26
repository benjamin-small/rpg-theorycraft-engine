# Changelog

All notable changes to `rtce` and `rtce-testkit` are documented here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows [SemVer](https://semver.org/) once published (0.x
until then, per semver's "anything goes" pre-1.0 clause).

## [Unreleased]

**P7 — PoE2 test bed + instance mechanics** (in progress; folded into the
0.3.0 entry when the phase closes).

- **Expression-valued sim fields (P7b).** `BuffDef::duration`,
  `ActionDef::cooldown`, the `cost`/`gain` amounts, and the
  `ActionDamage::stats` values now accept a plain number OR an expression
  string over the sim symbol space (`simdef::NumOrExpr`, untagged serde).
  **Compatibility, precisely scoped:** every 0.2.0 config PARSES
  unchanged, and a literal reaches the executor as the identical `f64` the
  old field held — a `Num` is pre-baked into a constant at `sim::compile`,
  so the literal path adds no evaluation and no rounding (pinned by a test
  that replays the P6 spec's exact numeric JSON and reproduces the P6c
  starvation cadence). That is a claim about the FIELDS. Two deliberate
  executor behavior fixes below can move results for configs that hit
  them; everything else is unchanged, including the `diablo4_rotation`
  pins.

  An expression is parsed at `sim::compile` with the usual positioned
  fail-closed errors (pipeline stages/buckets stay invisible — pinned at
  all five sites). Evaluation instants are fixed and documented per field:
  `duration` at application (SNAPSHOTTED onto that window — a later
  stat/phase change never moves an expiry already in flight), `cooldown`
  and `cost` at cast start, `gain` and `damage.stats` at cast complete. A
  non-finite result is a run error at that instant naming the field and
  the instant; `duration`/`cooldown`/`cost`/`gain` additionally reject
  negatives. A `duration` expression reads the LIVE state at application,
  which differs between a first application and a REFRESH (on a refresh
  the outgoing window is still in force, so `buff.<self>` is 1 and
  `buff_remaining.<self>` is the time left on the window being replaced) —
  that is what makes pandemic-style refreshes, `"min(12,
  buff_remaining.x + 8)"`, expressible as data.

  `sim::CompiledValue` is new public API (`#[non_exhaustive]`), and the
  `CompiledAction`/`CompiledBuff` fields for these five hold it instead of
  `f64`.

- **Fixed (behavior): a proc's effect is now visible to a later proc in
  the same trigger batch.** Proc `chance` expressions were evaluated
  against a slot array whose time-varying tail (`buff.*`,
  `buff_remaining.*`, resource amounts, `casts.*`) was refreshed once per
  BATCH, while the stat/condition prefix already refolded whenever an
  effect fired — so a chance could read `casts.x == 0` for an action a
  previous proc in the same batch had just free-cast, while a condition
  driven by that same effect already read its new value. The tail is now
  refreshed per proc. Affects only configs whose proc `chance` reads sim
  state another proc mutates in the same batch.

- **Fixed (behavior): EV mode's `on_crit` weight is measured before the
  cast's own procs.** `Mode::Expected` weights `on_crit` accumulation by
  the probability the hit crit; that query used to be deferred to its
  point of use, i.e. AFTER this cast's `on_cast`/`on_hit` procs had
  already fired — so a proc triggered BY a hit could retroactively raise
  that hit's crit weight even though its DAMAGE had been computed off the
  pre-proc build. One cast is now measured once, up front:
  `damage.stats`, `hits_per_use`, and the crit weight all come from the
  same overlay and the same effective phase. Affects only configs with an
  `on_cast`/`on_hit` proc that changes crit chance.

- **Fixed (behavior): a resource's `max`/`regen_per_sec` is re-derived
  against the state that caused the refold.** Those expressions were
  evaluated against a slot tail whose freshness depended on which caller
  last happened to update it; `refresh_effective_state` now refreshes it
  itself, uniformly at every call site (buff applied, buff expired, phase
  boundary, initial fold). Affects only configs whose resource `max`/
  `regen_per_sec` names sim state rather than plain stats/conditions.

## [0.2.0] — 2026-07-22

**P6 — sequencing: the timeline simulator.** A second way to answer "what
is this build worth?" alongside `Plan::evaluate`'s closed-form average: a
discrete-event executor over a priority-list rotation, ending in a
runnable Diablo 4 slice and computed (not asserted) uptimes.

- **Expression predicates.** `> < >= <= == !=` (returning exactly `0`/`1`)
  and `and(a,b)` / `or(a,b)` / `not(a)` functions, available everywhere
  the expression language already was — a comparison level sits between
  arithmetic and the top of the grammar, single-comparison only (no
  chaining), fail-closed positioned errors throughout.
- **`SimDef` + `Rotation`.** Two new config artifacts sitting beside
  `GameDef`: resources (capped pools with regen), actions (cast time,
  cooldown, resource cost/gain, an optional damage effect overriding
  `Plan` stats per cast), buffs (timed contribution/condition windows,
  optional DoT-tick objectives), and procs (chance-triggered, on an
  internal cooldown, applying a buff or casting a free action) — plus a
  SimC-style priority-list `Rotation`. `sim::compile` builds the extended
  sim symbol space (`time`, `duration`, resources, `cooldown.<action>`,
  `buff.<buff>`, `buff_remaining.<buff>`, `casts.<action>`, layered over
  the `Plan`'s own stats/conditions) and validates every cross-reference
  fail-closed (unknown actions/buffs/resources, proc effect arity,
  `damage_objective`/`tick_objective` objective membership, reserved
  words, flat-namespace collisions).
- **`sim::run` — one stepper, two modes.** A discrete-event queue
  (`CastComplete`, `BuffExpire`, `PhaseBoundary`, a unified `Wake` for
  cooldown/resource-affordability waits) drives the SAME decision loop
  under both `Mode::Expected` (deterministic, branch-blended exactly like
  `evaluate` — proven to agree with it EXACTLY on a degenerate config) and
  `Mode::MonteCarlo { iterations, seed }` (seeded independent timeline
  runs via a new zero-dependency in-crate PCG32 and `Plan::evaluate_sampled`,
  pooled into mean-field reports plus a `dps` `Distribution`). Only
  per-cast damage/proc OUTCOMES differ by mode; rotation timing never
  does. Procs fire via a deterministic accumulator in EV mode
  (`acc += chance`, fires at `acc >= 1`, ICD-gated) and by exact roll in
  MC mode.
- **`SimReport` — computed, not asserted.** Per-phase and total
  damage/dps, per-action casts/damage/share, COMPUTED buff and condition
  uptimes (an active buff's condition value wins over a scenario's static
  uptime while up, per the documented precedence rule), per-resource
  `time_starved`/`time_capped`, and proc fire counts — `Mode::MonteCarlo`
  adds a `Distribution` (mean/population-std/p10/p50/p90, nearest-rank
  percentiles) over the `iterations` per-iteration `dps` values.
- **`examples/diablo4_rotation.rs`.** A runnable Diablo 4 slice on top of
  the same committed gamedef `diablo4_basics` uses: mana, a Fireball
  spender / Firebolt generator pair, and a Frost Nova whose proc opens a
  computed `vulnerable` window — 60s EV run plus a 1000-iteration Monte
  Carlo run, hand-worked pins on both the cast cadence and the computed
  uptime (documented as a demonstration slice, not real game data).

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

[0.2.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.2.0
[0.1.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.1.0
