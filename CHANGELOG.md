# Changelog

All notable changes to `rtce` and `rtce-testkit` are documented here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows [SemVer](https://semver.org/) once published (0.x
until then, per semver's "anything goes" pre-1.0 clause).

## [Unreleased]

Nothing yet.

## [0.3.0] — 2026-07-26

**P7 — PoE2 test bed + instance mechanics.** A second, independent
consumer proves the engine is not shaped around one game, and the
sequencing tier grows the counted/snapshotted state an ARPG actually asks
for: buff stacks with reapply policies, snapshot DoTs, action-scoped
buffs and procs, and expression-valued sim fields.

`poe2-calcs` is that second consumer (the harness lives in that repo, not
this one): a generated 209-stage `GameDef` + adapter reproduces its native
calculator to 1e-9 across 63 parity tests — standing references 124.53 /
129.51 / 793.76 dps — with a 156-pair `(StatId, ModKind)` sweep guarding
against silent routing drift. Its native math is untouched; this is a
proof, not a switchover.

### Upgrading from 0.2.0 — read this first

Every 0.2.0 JSON config parses unchanged, and a field that only ever held
a number still reaches the executor as the identical `f64`. Two things
can still surprise you:

**Four behavior fixes can move numbers**, each for a narrow class of
config. Full detail in "Fixed" below; the short form:

| Fix | Moves numbers for |
|---|---|
| the fight horizon is DRAINED | any config where a `BuffExpire` can land on the fight's exact end instant — in practice, INTEGER buff durations against an integer `duration`. The final cast was being dropped whole. |
| EV `on_crit` weight measured before the cast's own procs | configs with an `on_cast`/`on_hit` proc that changes crit chance, run under `Mode::Expected`. |
| a proc's effect is visible to a later proc in the same batch | configs whose proc `chance` reads sim state (`casts.*`, resources, `buff*.*`) that another proc mutates in the same trigger batch. |
| a resource's `max`/`regen_per_sec` re-derived against the state that caused the refold | configs whose resource `max`/`regen_per_sec` names sim state rather than plain stats/conditions. |

(A fifth fix, the frozen `apply_buff` overlay, is listed too — but it
lives entirely on `apply_buff`, which is new in 0.3.0, so no 0.2.0 config
can have been measuring it.)

Nothing in this repo moved: `examples/diablo4_rotation.rs` holds byte for
byte at 225199.1088 total / 3753.31848 dps / 0.4 vuln uptime, and both
downstream consumers are byte-identical.

**Rust source-breaking (permitted under 0.x, but you deserve the
warning).** New fields on `ActionDef` (`apply_buff`), `ProcDef`
(`actions`) and `BuffDef` (`max_stacks`, `on_reapply`) break any
exhaustive struct literal constructing them in Rust — this repo's own
53-literal churn is the proof. JSON is unaffected. The fix: add the new
fields explicitly with their 0.2.0-equivalent values —
`apply_buff: Vec::new()` on `ActionDef`, `actions: None` on `ProcDef`,
`max_stacks: 1` and `on_reapply: ReapplyPolicy::Refresh` on `BuffDef` —
or stop writing them exhaustively (`ActionDef` derives `Default`, so
`ActionDef { cast_time: "1".into(), ..Default::default() }` works;
`BuffDef` and `ProcDef` do not).

Field TYPES also changed. `BuffDef::tick_objective` is now
`Option<TickObjective>` rather than `Option<String>`, and the five newly
expression-capable fields — `BuffDef::duration`, `ActionDef::cooldown`,
and the values of `cost` / `gain` / `ActionDamage::stats` — hold
`NumOrExpr` where they held `f64` (both spellings still deserialize;
`NumOrExpr: From<f64>` covers Rust constructors). On the compiled
side, `CompiledBuff::tick_objective` is `Option<CompiledTick>` rather
than `Option<usize>`, and `SimReport::buff_uptime` is REPLACED by
`buffs: BTreeMap<String, BuffReport>` (`.uptime` plus the new
`.avg_stacks`). `CompiledAction`, `CompiledProc`, `CompiledValue` and
every `sim::report` type are now `#[non_exhaustive]`, so later
measurements stop being breaking changes for external constructors; the
CONFIG types are deliberately NOT marked, so a caller building a `SimDef`
in Rust can still write a struct literal.

### Added

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

- **Three PoE2 worked examples (P7e)**, plus the committed fixture they
  run on. If you are upgrading FOR stacks, snapshot DoTs, or action-scoped
  procs, these are the fastest way in — each is a runnable slice with
  every pin hand-derived in a comment and asserted in CI, and each carries
  contrast runs that show what a plausible WRONG spelling of the same
  config produces:

  | Example | Mechanic | Pins |
  |---|---|---|
  | `poe2_charges` | `add_refresh_all`, `max_stacks: 3`, expression `duration`, `stacks.X` gating a rotation rule | 11748 damage / 293.7 dps / 2.25 avg stacks |
  | `poe2_poison` | `add_independent` unbounded, `snapshot: true`, applied by the skill's own `apply_buff` | 6000 hit + 11625 DoT / 881.25 dps / 3.875 avg stacks |
  | `poe2_triggers` | `ProcDef::actions` filter + `cast_action`, `apply_buff` on both a primary and a free-cast secondary | 9870 damage / 493.5 dps / 5 triggered casts |

  **Scope, honestly:** `crates/rtce/tests/fixtures/poe2/gamedef.json` is a
  PoE2-*shaped* demonstration slice — not Path of Exile 2's damage model
  and not derived from game data. Every coefficient is `representative`,
  chosen so each pin hand-derives. It exists to exercise the mechanics
  above end to end, not to price a real character.

  Note also what has NO example: `strongest` is pinned in the test suite
  only (`sim::exec`'s `mod snapshot`), and it is the reapply policy with
  the sharpest edge — a losing reapplication changes neither the magnitude
  nor the expiry.

  And note what these three do NOT exercise: **sampling.** Nothing in
  their configs samples — the fixture's crit is closed form
  (`1 + c·(m−1)`, the same choice `poe2-calcs`' generated gamedef makes)
  and every proc they define is `chance: "1"` — so each asserts that
  `Mode::MonteCarlo` reproduces its EV number EXACTLY, with zero spread,
  rather than within a tolerance band. That is deliberately the STRONGER claim (it fails if an
  RNG draw ever appears on a path that must stay deterministic), but it is
  a claim about exactness, not about Monte Carlo's distribution machinery.
  `examples/diablo4_rotation.rs` remains the only example that actually
  samples, and the only one reporting a non-degenerate spread.

### Fixed

- **The fight horizon is DRAINED, so a cast completing at
  `t == duration` is never silently dropped.** The run loop processed
  at most ONE event at the horizon: it popped an event, advanced the
  clock, handled it, then broke on `time >= duration` — so any other event
  already queued at that same instant was discarded, and WHICH one
  survived was decided by the heap's `(time, seq)` tiebreak. Concretely, a
  `BuffExpire` landing on the horizon carried the lower `seq`, popped
  first, and swallowed the `CastComplete` there — dropping that cast
  whole: its count, its damage, and its `apply_buff`. A cast at the
  horizon DID count when it was alone on the instant, so this was never
  "the horizon excludes its boundary"; it was order-dependent silent
  damage loss.

  The horizon rule is now stated in the public `sim` module docs and
  pinned: no cast BEGINS at or after `duration`; every event already
  scheduled AT `duration` is processed, in the usual `seq` order; a cast
  completing exactly at `duration` counts. The drain is bounded
  (`HORIZON_DRAIN_LIMIT`) and fails closed naming the looping event,
  matching the instant-cast livelock guard's shape.

  **Upgrading from 0.2.0: this can move your numbers.** Any config with
  INTEGER buff durations will eventually land an expiry exactly on an
  integer fight's horizon, and every such config was losing its final
  cast. The symptom is an off-by-one cast count that depends on the buff
  duration: a 10s fight of 1s casts applying a `refresh` buff reported 9
  casts at durations 2 / 5 / 9, and 10 at 9.5. Nothing in this repo moved
  — no `sim::exec` pin, and `diablo4_rotation` holds byte for byte
  (225199.1088 / 3753.31848 / 0.4, MC block included), because its 60th
  cast was already alone at the horizon and its `vuln_window` expiries
  land at 4, 14, …, 54 — never at 60. A config of YOURS that does move was
  measuring the bug; the new number is the correct one.

  One edge becomes newly reachable and is pinned rather than left to
  drift: a ZERO-WEIGHT FINAL PHASE puts a `PhaseBoundary` on the horizon
  too, and since it was scheduled at sim construction it holds the lower
  `seq` and resolves first — so a cast completing at `duration` is
  measured under, and credited to, that zero-width phase. That follows the
  `seq` rule and is a consequence of draining the instant, not a designed
  statement about what a zero-width phase should own.

  **One NEW failure mode**, from that same edge. The drain must walk every
  event at the horizon, so a scenario ending in more than
  `HORIZON_DRAIN_LIMIT` (10,000) zero-weight phases — each of which
  schedules a boundary at exactly `duration` — now returns a fail-closed
  `PlanError` naming the offending phase, where 0.2.0 returned a report.
  Pathological (it takes ten thousand trailing zero-weight phases), but it
  is a config that used to produce a number and no longer does, so it
  belongs in an upgrade note and not only in the code. The bound covers
  the HORIZON INSTANT ONLY — the run loop is deliberately unbounded at
  every other instant.

- **A proc's effect is now visible to a later proc in
  the same trigger batch.** Proc `chance` expressions were evaluated
  against a slot array whose time-varying tail (`buff.*`,
  `buff_remaining.*`, resource amounts, `casts.*`) was refreshed once per
  BATCH, while the stat/condition prefix already refolded whenever an
  effect fired — so a chance could read `casts.x == 0` for an action a
  previous proc in the same batch had just free-cast, while a condition
  driven by that same effect already read its new value. The tail is now
  refreshed per proc. Affects only configs whose proc `chance` reads sim
  state another proc mutates in the same batch.

- **EV mode's `on_crit` weight is measured before the
  cast's own procs.** `Mode::Expected` weights `on_crit` accumulation by
  the probability the hit crit; that query used to be deferred to its
  point of use, i.e. AFTER this cast's `on_cast`/`on_hit` procs had
  already fired — so a proc triggered BY a hit could retroactively raise
  that hit's crit weight even though its DAMAGE had been computed off the
  pre-proc build. One cast is now measured once, up front:
  `damage.stats`, `hits_per_use`, and the crit weight all come from the
  same overlay and the same effective phase. Affects only configs with an
  `on_cast`/`on_hit` proc that changes crit chance.

- **A resource's `max`/`regen_per_sec` is re-derived
  against the state that caused the refold.** Those expressions were
  evaluated against a slot tail whose freshness depended on which caller
  last happened to update it; `refresh_effective_state` now refreshes it
  itself, uniformly at every call site (buff applied, buff expired, phase
  boundary, initial fold). Affects only configs whose resource `max`/
  `regen_per_sec` names sim state rather than plain stats/conditions.

- **An `apply_buff` snapshot capture is frozen on BOTH action paths.**
  Found by the P7d review, and entirely within 0.3.0's own new surface —
  no 0.2.0 config can have been measuring it. A DAMAGING action froze its
  `damage.stats` overlay before running its `apply_buff` list, but a
  UTILITY action resolved the live effective build per entry, so the same
  two-entry list captured a different rate on each path and
  `TickObjective::snapshot`'s "before this application's own effects fold
  in" was false on the damaging one. The effective build is now captured
  ONCE before the list on both paths, and the clone is paid only when the
  list is non-empty. The asymmetry that REMAINS is deliberate and pinned
  both ways: within one list a `duration` EXPRESSION is sequential (it
  reads sim state, so a later entry sees earlier entries' stack counts)
  while a snapshot MAGNITUDE is frozen (it reads a build, captured once).

### Docs

- **A buff expiring on the cast grid.** New `sim` module-docs
  section on a long-standing (0.2.0, unchanged) consequence of `seq`
  ordering that costs damage silently: a `BuffExpire` sharing an instant
  with the `CastComplete` that would refresh it resolves FIRST, so the
  refreshing cast does not benefit from its own buff. The integrated
  column a reader would reach for does not show it. Both effects are now
  pinned as contrast runs, each changing exactly one duration:
  `poe2_triggers` at `shock: 2.0` instead of `2.5` reports the same 0.95
  uptime with bolt damage down 2175 → 1837.5; `poe2_charges` at `"4 +
  stacks"` instead of `"4.5 + stacks"` reshapes the cycle from 12/28 to
  15/25 casts and drops the total 11748 → 10875 while `avg_stacks` still
  reads exactly 2.25. Worth checking whenever a buff duration is an exact
  multiple of the refreshing action's cadence. Whether the ordering itself
  should change (a `CastComplete` arguably ought to out-rank a coincident
  `BuffExpire`) is an open 0.4.0 question in `ROADMAP.md`, explicitly not
  decided here.

- **The crate README describes the engine that exists.** It had not moved
  since 0.1.0: it presented `rtce` as evaluate-only, named three config
  tiers, and linked one example. It now covers all three fidelity levels,
  the sequencing tier, stacks/snapshot DoTs, and all six examples — the
  page crates.io and docs.rs actually render. The crate-level rustdoc
  gained the same material, and two module docs stopped describing the
  executor as future work.

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
