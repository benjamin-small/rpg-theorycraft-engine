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

- **Buff stacks and reapply policies (P7c-T1).** A buff is now internally
  an INSTANCE LIST, and `BuffDef` gains `max_stacks` (default `1`; `0` =
  unbounded) and `on_reapply` (`refresh` | `add_refresh_all` |
  `add_independent` | `strongest`, default `refresh`). Every 0.2.0 config
  names neither field and gets exactly its old behavior: `refresh` with
  one instance IS the binary buff, and the `diablo4_rotation` pins hold
  byte for byte (225199.1088 / 3753.31848 / 0.4, MC block included).

  `add_refresh_all` counts up to the cap and resets EVERY instance's
  expiry (one shared clock — PoE2 charges); `add_independent` gives each
  instance its own duration and, at the cap, evicts the earliest-expiring
  one (PoE2 poison). `strongest` landed in P7c-T2 (below), and `refresh`
  alongside a `max_stacks` other than `1` is rejected rather than silently
  ignored.

  What a stack count scales, and what it deliberately does not:
  `contributions` fold with their VALUE multiplied by the count (3 stacks
  of `+10` in a `product` bucket read `×1.30`, not `×1.10³`) and a live
  `tick_objective` ticks at rate × count; `conditions` are driven at their
  full configured value while ANY instance is live and are never scaled by
  it. New symbol `stacks.<buff>` (the count) joins `buff.<buff>` (`1`
  while any instance is live) and `buff_remaining.<buff>`, which is now
  the LONGEST remaining window across live instances.

  `SimReport::buff_uptime` is REPLACED by `buffs: BTreeMap<String,
  BuffReport>` carrying `uptime` plus the new `avg_stacks` (the
  time-integrated mean stack count), matching how `actions`/`resources`
  have always reported per-entity results — buffs were the last entity
  with a bare parallel map. Every report type in `sim::report` is now
  `#[non_exhaustive]`, so later measurements stop being breaking changes
  for external constructors.

- **Snapshot DoTs and `strongest` (P7c-T2).** `BuffDef::tick_objective`
  becomes `Option<TickObjective>` and accepts two JSON shapes: the 0.2.0
  bare name (LIVE — re-evaluated on every state change, × the stack
  count, and what a live objective serializes back to) or
  `{ "objective": …, "snapshot": true }`. A SNAPSHOT instance captures the
  objective's value at its own application and ticks that rate unchanged
  to expiry; the buff's total rate is the SUM over live instances, with
  the stack count inherent in the sum rather than multiplied in again.
  Nothing re-reads the `Plan` for such a buff, so an instance is immune to
  every later stat, phase and buff change — PoE2 ailment semantics.

  A rate is captured against the state the instance LANDS ON — before its
  own application folds in, the same instant `duration` is evaluated at.
  A buff whose own `contributions` feed the objective it ticks therefore
  SELF-AMPLIFIES, one application behind.

  A captured rate belongs to the APPLICATION and is never re-captured:
  `add_refresh_all` pushes an existing instance's EXPIRY out onto the
  shared clock while leaving its rate alone, and only a new instance
  (`refresh`'s replacement, an `add_independent` push, a `strongest` win)
  ever carries a new one. The policies differ sharply in what they do with
  a captured rate, and each variant documents its own case: `refresh`
  re-captures unconditionally, so a reapplication in a weaker moment
  LOWERS the DoT (the opposite of `strongest`); `add_refresh_all` never
  re-captures, and AT THE CAP discards the incoming rate entirely while
  still resetting the shared clock, so a capped stack can ride an old
  snapshot indefinitely; `add_independent` evicts the earliest-EXPIRING
  instance at the cap, not the weakest.

  `on_reapply: strongest` (PoE2 ignite) is now honored: the incoming
  instance replaces the incumbent only when its snapshot rate is STRICTLY
  higher, and a losing application is discarded whole — it moves neither
  the rate nor the expiry, so a weak reapplication cannot extend a strong
  ailment. It requires `snapshot: true` and `max_stacks: 1`; both are
  positioned, fail-closed compile errors otherwise (a LIVE tick objective
  is not enough — its rate belongs to the buff, not to an instance).

  Backward compatibility is unchanged: a 0.2.0 config writes a bare name,
  gets live semantics, and skips the fold transaction on a refresh exactly
  as before — the `diablo4_rotation` pins hold byte for byte, MC block
  included. New public API: `simdef::TickObjective` and
  `sim::CompiledTick`; `CompiledBuff::tick_objective` is now
  `Option<CompiledTick>` rather than `Option<usize>`.

  A snapshot rate is EV-blended in both modes (the capture calls
  `Plan::evaluate_phase`, never `evaluate_phase_sampled`), inherited from
  0.2.0's DoT integration: a tick is a continuous rate, not an event to
  sample. That is why EV and Monte Carlo agree so tightly on snapshot-DoT
  totals — they differ in when instances are applied, never in what each
  captures.

- **Action-scoped effects (P7d).** `ActionDef` gains `apply_buff:
  Vec<String>` — buffs the action itself applies when its cast COMPLETES,
  one application per list entry, each routed through the buff's own
  `on_reapply` policy exactly like a proc application. `ProcDef` gains
  `actions: Option<Vec<String>>` — a trigger filter naming the actions
  whose casts this proc considers; `None` (the default, and every 0.2.0
  config) is every action. Between them they retire the "icd equals the
  gating action's cooldown" trick that was previously the ONLY way to
  bind an effect to one action.

  The completion instant now has this fixed internal order: `gain` →
  `casts += 1` → measure and credit damage → `apply_buff` (list order) →
  proc rolls. So the applying cast never benefits from the buff it
  applies (the rule `damage.stats` already stated for procs), and a proc
  rolled by that cast always SEES it — an action's intrinsic effects
  resolve before the effects it merely triggers, so the whole
  `apply_buff` list precedes the whole proc batch and never interleaves
  with the procs' name order. A repeated name in `apply_buff` is applied
  that many times. A proc-triggered FREE cast applies its action's
  `apply_buff` too: it is an effect OF the action, like `gain` and
  `damage`, not part of the cast pipeline the free-cast path skips.

  A SNAPSHOT `tick_objective` applied by `apply_buff` captures under the
  CASTING ACTION's overlay — the effective build with that action's
  `damage.stats` folded on — so an ailment inherits the magnitude of the
  hit that applied it (a utility action has no overlay and captures the
  plain effective build). The PROC application path is deliberately
  UNCHANGED and still captures the ambient effective build; the two are
  pinned against each other as controls.

  Fail-closed at `sim::compile`: an unknown buff in `apply_buff`, an
  unknown action in an `actions` filter, and an EMPTY `actions: []` —
  which describes a proc that could never fire, and reads like `None`
  while meaning the opposite. A filtered-out cast is skipped before the
  ICD gate, before the `chance` evaluation and before any RNG draw, so in
  Monte Carlo mode a filter genuinely removes rolls from the stream
  rather than rolling and discarding.

  `examples/diablo4_rotation.rs` is rewritten off the trick — `frost_nova`
  carries `apply_buff: ["vuln_window"]` and the `nova_pulse` proc is
  DELETED, leaving the example with no procs at all. The cadence is
  unchanged and the EV pins are byte-identical (225199.1088 /
  3753.31848 / 0.4, and each action's own damage). Its Monte Carlo block
  DID move — mean 3743.0759 / std 210.1306 → 3746.6413 / 211.4556 —
  because the deleted proc used to consume one RNG draw per off-ICD
  `on_cast` roll (SEVEN per iteration; six of them stream-relevant, the
  seventh landing at `duration` with nothing sampling after it), and
  removing them re-phases which crit sample lands on which cast. Same
  distribution, different phase: both means sit within 0.2% of the EV
  pin.

  That seventh fire is worth naming, because it IS the trap: the old
  proc fired six times on Frost Nova as intended and a seventh time at
  t=60 on the Fireball/Firebolt completing exactly at `duration`, whose
  ICD had just cleared. The old config therefore applied `vuln_window`
  seven times where `apply_buff` applies it six, and the uptime pin only
  survived because the seventh window opened at the fight boundary and
  integrated to zero seconds. "icd equals the gating action's cooldown"
  never meant "only that action" — it meant "at most one per
  cooldown-length, whoever happens to trigger it".

  Within one `apply_buff` list there is a deliberate asymmetry, and it is
  the surprising part: a `duration` EXPRESSION is sequential (it reads sim
  state, so a later entry sees earlier entries' stack counts) while a
  snapshot magnitude is FROZEN at the world the cast found (it reads a
  build, captured once before the list runs, so a later entry does not see
  earlier entries' `contributions`). Freezing is what makes the damaging
  and utility paths agree — a damaging action freezes its overlay, a
  utility action freezes the plain effective build, and the same list now
  means the same thing on both.

  New public API: `CompiledAction::apply_buff` (`Vec<usize>`) and
  `CompiledProc::actions` (`Option<Vec<usize>>`), both resolved to
  indices; `CompiledAction` and `CompiledProc` are now
  `#[non_exhaustive]`, joining `CompiledValue` and the `sim::report`
  types (the CONFIG types they mirror are deliberately not marked — a
  caller building a `SimDef` in Rust should be able to write a struct
  literal).

  Every 0.2.0 config parses and behaves unchanged — an action that names
  no `apply_buff` applies nothing, and a proc that names no `actions`
  considers every action. **Caveat for Rust downstreams:** that is a
  statement about JSON. Adding a field to `ActionDef` and `ProcDef` is
  SOURCE-breaking for anyone constructing either with an exhaustive struct
  literal (this repo's own 53-literal churn is the proof). Permitted under
  0.x, but an upgrader deserves the warning; add `apply_buff: Vec::new()`
  / `actions: None`, or switch to `..Default::default()` on `ActionDef`.

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
