# Changelog

All notable changes to `rtce` and `rtce-testkit` are documented here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/);
versioning follows [SemVer](https://semver.org/) once published (0.x
until then, per semver's "anything goes" pre-1.0 clause).

## [Unreleased]

### Added

- GameDef pipelines now support deterministic bounded scalar-solve stages.
  Bounds and residuals compile once, bisection returns the greatest known
  feasible lower bound, configuration fixes both tolerances and an iteration
  budget, and invalid brackets or non-finite samples fail with `PlanError`.
- GameDef pipelines now support deterministic bounded state-recurrence
  stages. Configuration declares collision-checked local state, simultaneous
  next-state expressions, a terminal predicate, a result expression, and a
  hard transition budget. Expressions compile once, reusable scratch keeps
  evaluation allocation-free, and non-finite or non-terminating runs fail
  with stage-and-iteration context on native and Wasm.

### Changed

- The public Rust `StageDef` struct is now an untagged enum over
  `ExpressionStageDef`, `SolveStageDef`, and `RecurrenceStageDef`. Existing
  expression-stage JSON is unchanged; Rust callers constructing stages
  directly must wrap the new expression variant.

## [0.5.1] — 2026-08-15

**Self-contained nonlinear expression math.** Version 0.5.1 lets GameDef and
SimDef configuration own formulas that require roots or fractional powers,
removing the need for downstream adapters to precompute poison-selection and
closed-form armour values.

### Added

- The shared expression VM now supports `sqrt(x)` and fractional
  `pow(base, exponent)` in every GameDef and SimDef expression location. Both
  use IEEE `f64` semantics, retain compile-time arity and stack-depth checks,
  and unblock config-owned strongest-roll and closed-form armour formulas.

## [0.5.0] — 2026-08-15

**CLI, browser lab, and applied-DoT observability.** Version 0.5.0 makes
RTCE usable without writing a Rust integration first. The same JSON runner now
powers a native command, a Docker-first demo, and a TypeScript/Wasm tutorial
that walks a gamer from one stat-sheet number through bucket math, critical-hit
branches, rotations, buffs, and Monte Carlo simulation. The library also gains
the missing diagnostics for applied damage-over-time mechanics: a config-owned
stat-sheet recipe and direct per-buff application, damage, and DPS reporting
from the timeline.

### Highlights

- **Run RTCE from a shell.** The private `rtce-cli` binary provides
  `evaluate`, `explain`, `simulate`, `lexicon`, and bundled demo commands.
  Native and browser clients share versioned JSON envelopes from
  `rtce-runner`; neither interface reimplements engine math.
- **Start with Docker.** The multi-target Dockerfile defaults to the CLI demo,
  serves the interactive tutorial from a second target, and can export a
  self-contained page that opens directly from a `file://` URL.
- **Learn the engine interactively.** The seven-step field guide combines live
  JSON editors, structured result cards, and a real browser terminal. Its Run
  button enters the corresponding CLI-shaped command, streams report-derived
  activity, and updates the GUI from the same result.
- **Diagnose applied DoTs.** `BuffReport` now identifies how many times an
  effect was applied and how much damage/DPS its tick objective contributed.
  The library-owned applied-DoT fixture exposes chance, duration, expected
  stacks, target multiplier, per-stack DPS, total DPS, and damage per
  application while proving target damage-taken modifiers are owned once.
- **Make config vocabulary explicit.** `rtce lexicon` and the browser
  dictionary distinguish schema fields, user-declared names, expression
  functions/operators, annotations, conventions, and engine-supplied context.

### Added — interfaces and tutorial

- A private `rtce-cli` workspace binary with `evaluate`, `explain`, and
  `simulate` commands, plus bundled `demo calc|sim|monte-carlo` runs. Outputs
  are versioned JSON envelopes shared with the browser interface.
- A private `rtce-runner` JSON adapter and `rtce-wasm` binding so native and
  browser interfaces compile and execute the same engine paths.
- A multi-target Dockerfile. The default `cli` target is the primary command
  demo; the `tutorial` target serves a static TypeScript/Wasm walkthrough.
- A seven-step browser tutorial powered by
  `@benjamin-small/browser-terminal`, with live editors for the committed guide
  fixtures and structured, pipeable `rtce` commands.
- A self-contained `rtce-field-guide.html` build that embeds both Wasm engines,
  JavaScript, and CSS so the tutorial also runs directly from a `file://` URL.
- Terminal-driven lesson runs: the GUI Run button enters the real command at
  the browser-terminal prompt, streams compilation, branch, cast, hit, buff,
  and distribution playback there, and updates the workbench from the same
  structured command result.
- Browser tutorial commands mirror the native CLI's `--game`, `--build`,
  `--scenario`, `--sim`, and `--rotation` arguments. Browser-terminal injects
  each live editor document as `$game`, `$build`, `$scenario`, `$sim`, or
  `$rotation` in place of a filesystem path.
- A shared `rtce lexicon` dictionary with searchable, lesson-specific browser
  examples.

### Added — library and reports

- Damaging buffs now report their application count, directly attributed
  damage, and DPS alongside uptime and average stacks. A library-owned
  applied-DoT fixture pins a complete poison-shaped stat-sheet breakdown,
  including single ownership of target-side damage multipliers.
- Monte Carlo `Distribution` reports now include `iterations`, `min`, and `max`
  alongside mean, population standard deviation, and percentiles, making it
  explicit how many fights ran and what the observed bounds were.
- `event_multiplier` is a readable alias for the engine's branched-stage event
  factor. The legacy `event_factors` spelling remains supported and retains its
  serialized trace field; a user-declared `event_multiplier` keeps its original
  meaning and shadows the alias.

### Changed

- The crit lesson now multiplies a declared, event-gated `crit_damage` bucket
  directly. `event_multiplier` and its `event_factors` compatibility alias
  remain available, but are presented honestly as advanced engine context.
- Build examples use `_`-prefixed annotations to identify the gear or rule that
  supplied a contribution without changing calculation semantics.
- Tutorial callouts now explain the actual config and engine behavior they sit
  beside, including buff conditions and simulation context, rather than using
  disconnected placeholder expressions.
- The lesson workspace keeps its header and tabs fixed while the config and
  terminal panes scroll independently; CLI demo controls remain visible at the
  top of each lesson without truncating the command or button label.

### Documentation and project infrastructure

- A GitHub Pages workflow builds and publishes the split TypeScript/Wasm
  tutorial from `main` with project-path-safe relative assets.
- Added the canonical license, testing guide, configuration reference, fixture
  guidance, repository metadata, CodeQL scanning, and protected-branch status
  checks.
- Added an applied damage-over-time guide covering the stat-sheet/timeline
  boundary, source-versus-target modifier ownership, snapshot objectives,
  stack policies, and the new report fields.

### Upgrading from 0.4.0

- Existing GameDef, BuildState, Scenario, SimDef, and Rotation JSON remains
  valid. `event_multiplier` is additive syntax; `event_factors` continues to
  work. A config that already declares `event_multiplier` retains that declared
  value rather than being captured by the alias.
- Serialized reports gain additive fields. `Distribution` adds `iterations`,
  `min`, and `max`; each `BuffReport` adds `applications`, `damage`, and `dps`.
  Consumers using strict external schemas should accept these fields before
  upgrading. Rust consumers are unaffected because these engine-produced
  report structs are `#[non_exhaustive]`.
- The CLI, runner, and Wasm crates are private workspace interfaces; only
  `rtce` and `rtce-testkit` remain part of the crates.io release path.

## [0.4.0] — 2026-08-01

**P8 — configurable semantics + one-world measurement.** A generic
engine should not hard-code the semantic choices real games make
differently: *when a cast measures its world*, *how a proc rolls against
a multi-hit cast*, *which of two coincident events resolves first*. Each
is now configuration — a `defaults` block (`measure` / `event_order` /
`proc_rolls`) with per-entity overrides where coherent, every knob a
named enum whose serde default reproduces 0.3.0 byte for byte (RNG
stream included; `diablo4_rotation`'s EV and Monte Carlo blocks and both
consumers are the standing proof). Around the knobs: what an action or a
proc DOES generalizes into one ordered `effects` list, unknown config
keys fail closed with a did-you-mean, non-finite config numbers stop
coming back `Ok(NaN)`, and the ONE deliberate behavior change fixes the
semantics the 0.3.0 release review flagged as arguably wrong — a cast
now measures one world, build AND phase. The phase closes with the
`refresh`+live-DoT coverage debt paid, a `strongest` worked example
(`poe2_ignite`), and the docs discipline codified as standing rules. Shipping
alongside it, and the reason this version is dated later than it was
staged: `docs/guide/`, a seven-chapter walkthrough that builds one small
RPG from a single stat to a Monte Carlo distribution.

### Upgrading from 0.3.0 — read this first

**One deliberate behavior change: one world per measured cast (P8c).**
Numbers move ONLY for a config where one action's `apply_buff`/`effects`
list contains BOTH a buff that drives a condition AND a later snapshot
`tick_objective` whose objective reads that condition — the captured
rate drops to the pre-list value (through 0.3.0 the capture read a
frozen build against the LIVE phase, so a pure list reorder could double
a DoT). Neither consumer qualifies; full detail under "Fixed" below.

**A typo'd config that parsed in 0.3.0 now FAILS CLOSED (P8a).** A key
no config struct declares was silently ignored — `"tick_objectiv"`
silently meaning "no DoT" — and is now a positioned error carrying its
own remedy: ``unknown field `tick_objectiv` on buff `poison` — did you
mean `tick_objective`?``. Rename the key the error names (when nothing
is within edit distance 2, the error lists the valid fields instead).
Keys starting with `_` remain the documented annotation namespace, at
every nesting level. Configs with no typos — including both consumers'
committed gamedefs — pass unmodified.

**The `icd: NaN` class of rejections widens (P8a).** 0.3.0 rejected a
non-finite/negative `ProcDef::icd` because NaN silently DELETED the
internal cooldown; 0.4.0 extends the same fail-closed treatment to the
values that silently poisoned results instead: `Contribution::value` (on
both halves of the shared type — `BuildState` and `BuffDef`
contributions), `BuildState.stats` values, and `Phase.stats` overrides.
A NaN/inf there used to return `Ok(NaN)`/`Ok(inf)` with no error
anywhere (including `Ok(dps = 0)` on a utility-only rotation); each is
now a positioned error. If this rejects your config, the numbers you had
were wrong.

**`sim::SimScratch` is REMOVED from the public API (P8f).** It spent
0.2.0–0.3.0 constructible, re-exported, and accepted by NOTHING —
`sim::run` builds its scratch internally, as it always did. If you
constructed one, delete the call; nothing could have consumed it. (If
batch scratch reuse ever earns a `run_with_scratch` variant, the type
returns as that variant's parameter.)

**Rust source breaks, consolidated** (permitted under 0.x; the JSON
surface is unaffected except as above). Neither consumer constructs any
affected type, and both were re-verified byte-identical per task:

- Nine existing config structs gained a public `extra` field (serde
  flatten, the P8a unknown-key collection): `SimDef`, `ResourceDef`,
  `ActionDef`, `ActionDamage`, `BuffDef`, `ProcDef`, `Rotation`, `Rule`,
  `EventDef`. Exhaustive struct literals need
  `extra: Default::default()`. (The seven structs consumers DO construct
  exhaustively — `GameDef`, `BucketDef`, `StageDef`, `BuildState`,
  `Contribution`, `Scenario`, `Phase` — deliberately gained no field;
  they reject unknown keys at parse instead.)
- New public config fields: `SimDef::defaults` (`SimDefaults`, itself
  new with `measure`/`event_order`/`proc_rolls`), `ActionDef::measure`
  (`Option<Measure>`), `ProcDef::rolls` (`Option<ProcRolls>`), and
  `ActionDef::effects`/`ProcDef::effects` (`Vec<EffectDef>`).
- Compiled-side renames: `sim::ProcEffect` is now `sim::CompiledEffect`;
  `CompiledProc.effect`/`CompiledAction.apply_buff` are replaced by
  `effects: Vec<CompiledEffect>`; `CompiledAction` carries the resolved
  `measure`, `CompiledProc` the resolved roll policy, `SimPlan` the
  resolved event order (all `#[non_exhaustive]`, engine-constructed).
- The new enums — `Measure`, `EventOrder`, `ProcRolls`, `EffectDef` —
  are `#[non_exhaustive]` from birth: `match` them with a wildcard arm.
  (`NumOrExpr` stays exhaustive by design.)

### Added — a progressive guide (docs)

- **A progressive guide — `docs/guide/`.** Seven markdown chapters
  building one made-up archer RPG concept by concept, from a single stat
  (chapter 1, `hit = 120`) to a sampled distribution over a 60s fight
  (chapter 7). Each chapter has a runnable, CI-gated companion,
  `examples/guide_01_one_number.rs` … `guide_07_monte_carlo.rs`, carrying
  the chapter's numbers as hand-derived assertions. The examples grow
  strictly by addition, so `diff`ing chapter N against N+1 shows exactly
  what a concept cost.

  Chapters 1–4 are the calc tier; 5–7 the sequencing tier, sharing one
  `GameDef` unchanged from chapter 4. **Chapter 6 names a divergence that
  was previously derived but unnamed**: a buff's *time-weighted* uptime
  (0.25 here) is not the *cast-weighted* uptime the damage actually
  experienced (0.20), because a 2.5s window straddles two integer cast
  completions rather than two and a half — and because the completion
  coincident with the buff's application is scheduled first and measures
  without it. Both uptimes are pinned, in both directions, along with the
  41.85-damage error a naive round-trip through the calc tier produces.
  The same divergence is latent in `examples/diablo4_rotation.rs`
  (0.4 integrated, 3 active completions per 10 slots).

  The chapter also runs `defaults: { measure: "cast_start" }` as a
  contrast and pins what it actually does: the cast-weighted uptime moves
  to **0.30**, overshooting rather than converging (8064.96 damage /
  134.416 dps, a 7-and-11 active split). `measure` and `event_order`
  choose which side of a boundary a model errs on; they cannot make the
  integrated 0.25 reachable, because a 2.5s window cannot contain two and
  a half casts. The example asserts that 0.20 and 0.30 *bracket* 0.25
  rather than converging on it.

- **`crates/rtce/tests/guide.rs` — the guide's drift gate.** Every fenced
  ```json block in a chapter must carry `title=<file>` and be
  byte-identical to that file in `tests/fixtures/guide/`, which is what
  the chapter's example `include_str!`s. Untitled blocks are rejected
  (the gate would otherwise be bypassable by omitting the title),
  orphaned configs are rejected, and the block count is floored so the
  scanner cannot pass vacuously.

  A fourth test, `no_example_includes_from_outside_the_crate`, closes a
  **blind spot in the release gate** found while staging this work. The
  guide's configs were first written under `docs/guide/configs/`, which
  builds fine from a git checkout and produces a crate that *cannot
  compile its own examples* once installed from crates.io — `cargo
  package` ships only files under the crate root. `cargo publish
  --dry-run` does not catch it, because its verify step builds the
  library and not the examples. The configs now live in
  `tests/fixtures/guide/` alongside the `d4`/`poe2` fixtures, and the
  test rejects any example whose `include_str!` escapes the crate.

### Changed — `your_own_game.rs` retired into the guide (docs)

- **`examples/your_own_game.rs` is retired** — chapters 1–4 of the guide
  are it, grown one concept at a time. Its pins **148.20 / 113.28 are
  superseded** by chapter 4's **229.71 / 175.584**: the game picks up an
  `additive` modifier pool in chapter 2 that `your_own_game` never had,
  so the numbers legitimately move. No engine behavior changed —
  `crates/rtce/src/` carries doc updates only, and `diablo4_rotation`'s
  EV and Monte Carlo blocks are byte-identical.
- Crate-level docs: the examples list now names all thirteen examples,
  and "the one example that exercises sampling" is corrected to two —
  `guide_07_monte_carlo` samples too, and asserts a non-zero spread plus
  a hand-derived hard bound `[112.84, 181.04]` dps that no sampled fight
  may escape. The same correction lands in `README.md`.

### Added — the `strongest` worked example (P8f)

`examples/poe2_ignite.rs` — the fourth PoE2 slice, retiring the 0.3.0
release notes' "note what has NO example: `strongest`" caveat. One
ignite over rising, falling and TIED phase power (two applications, one
per 5s phase, on the committed PoE2-shaped fixture), every pin
hand-derived and CI-run: the stronger application REPLACES (DoT 1950);
a weaker one is DISCARDED WHOLE — rate and expiry, so the ailment
lapses at its original t=9 and the uptime column itself shows the
refusal (0.8 vs 0.9); a TIE loses too ("strictly higher", DoT 1200 not
1350). The contrast run is the same falling timeline under `refresh`,
whose unconditional re-capture trades 300 of DoT for a longer window
(2100 at uptime 0.9 vs `strongest`'s 2400 at 0.8) — exactly the trade
the policy exists to refuse. Also the policy's first Monte Carlo
coverage: MC reproduces EV to the bit (a win/lose comparison runs
against branch-blended captures, so WHICH instance wins can never
depend on the seed).

### Covered (behavior unchanged, but it was not pinned) — the 0.2.0 default DoT shape (P8f)

Not a behavior change — the closing of a recorded hole. The 0.3.0
release review counted the `(reapply policy × tick mode)` cells and
found `refresh` + LIVE `tick_objective` — the 0.2.0 DEFAULT DoT shape —
had ZERO behavioral coverage, and no live tick had ever run under
`Mode::MonteCarlo`. Now pinned (`sim::exec`'s `mod live_dot`), with no
bug found hiding:

- A utility on a 10s cooldown applying a duration-4 live-tick buff over
  20s accrues exactly its two windows at the branch-blended rate
  (8s × 62.5 = 500, uptime 0.4) — the tick stops at expiry and resumes
  on the cooldown recast.
- Under Monte Carlo a live tick is same-seed byte-deterministic AND
  equals EV EXACTLY — pinned against a deliberately BRANCHED tick
  objective, so "the rate is branch-blended in both modes, never
  sampled" is the assertion, not a tolerance band (mutating the tick to
  sample reads 487.5 against the pinned 500.0).
- A MID-WINDOW refold (another buff doubling the objective at t=2,
  strictly inside a [0,4) window) moves a live rate at that instant and
  a snapshot rate not at all — 875 vs 750 on the same layout under the
  same `refresh` policy, the gap exactly the 2s × 62.5 doubling. With
  this, every `(policy × tick)` cell has a discriminating test.
- From the P8f docs audit: the `measure` knob's override was pinned in
  only ONE direction (override against an omitted block); the other —
  a per-action override winning over a NON-default block — is now
  pinned too, mutation-proven both ways
  (`a_per_action_override_wins_over_a_non_default_defaults_block`),
  matching the two-direction coverage `proc_rolls` already had.
- From the whole-phase review: the LAST doc-claimed snapshot consumer
  with only one cell pinned — `Measure`'s rustdoc promises the snapshot
  feeds the tick capture of every `ApplyBuff` entry, but every
  tick-capture pin ran under the default measure. Now pinned in both
  cells on one single-cast fixture
  (`a_snapshot_tick_capture_reads_the_cast_start_world_under_cast_start`):
  a `rate = 100 × (1 + time)` overlay captured at t=1 under the default
  (rate 200 → DoT 600) and at t=0 under `cast_start` (rate 100 → DoT
  300); discarding the pending snapshot and re-measuring at completion
  reddens exactly the `cast_start` literal at 600.

### Fixed — positioned-error labels tell the truth under `cast_start` (P8f review)

The `damage.stats`/`hits_per_use` error contexts hardcoded "at cast
complete (t={now})" — under `measure: "cast_start"` a NaN stat at t=0
therefore errored "at cast complete (t=0)", where t=0 IS the cast start:
self-contradictory. The label is now the capture position ("at cast
start" / "at cast complete", supplied by the capture's caller — an
instant cast correctly keeps the completion label under `cast_start`).
Same family: `proc_roll_count`'s "measured hits_per_use {h} at t=…"
stamped the ROLL instant (the completion) on a value measured at cast
start; the stamp is now the MEASURED instant, which the cast's
measurement carries. Default-path wording is untouched (its long-standing
pins stay green); the `cast_start`-path wording has its own pin now
(`measured_value_errors_name_the_cast_start_instant_under_cast_start` —
all three error sites), so the labels cannot regress.

### Changed — Rust API (P8f)

- **`sim::SimScratch` is REMOVED from the public API** (see the
  upgrade section). The type survives crate-internally; the re-export
  and the public constructor are gone.
- `expr`'s `Program::max_depth` docs now state the ACTUAL depth-bound
  relationship instead of naming a constant the public API does not
  expose (`MAX_STACK` is `pub` only inside the private
  `expr::compiler` module): `max_depth <= 64` by construction, because
  `compile` rejects a deeper program with a positioned "expression too
  deep" error — reachable at ~64 right-nested levels, and now pinned at
  the EXACT boundary (63 levels compile with `max_depth == 64`; 64
  levels fail closed).

### Docs (P8f)

- **The front pages describe the 0.4.0 engine** (the P7e-T3 lesson,
  applied before release this time): the root README, the crate README
  (the crates.io page) and the crate-level rustdoc gain a "Configurable
  semantics" section — the `defaults` block, one sentence per knob, the
  cast-grid footgun's two config fixes — plus the unknown-key
  fail-closed story, the per-hit half of the EV/MC proc-agreement
  paragraph, and `poe2_ignite` in the examples tables (the "no
  `strongest` example" caveat retires everywhere).
- **The docs discipline is codified as standing rules** in the repo's
  `CLAUDE.md`, binding on future phases: every config field's rustdoc
  states its default, its evaluation instant and its interactions; every
  numeric doc claim ships with a contrast-run pin; every shipped
  `(default × override)` cell gets a discriminating test. The P8f audit
  against those rules produced the override-direction test and the
  `max_depth` boundary pin above; the whole-phase review added a
  corollary (now also in `CLAUDE.md`): the release-staging commit
  re-sweeps ROADMAP's version pointers, since cutting Unreleased into a
  version stales every "(Unreleased)" cross-reference in the same
  commit.
- Two review clarifications on the knob docs, each verified against the
  executor: `Measure` now states the measurement/attribution split (a
  boundary-spanning `cast_start` cast is PRICED against the start
  instant's world but CREDITED to the completion instant's per-phase
  row), and `ProcRolls`' once-per-cast chance rule now says outright
  that `chance` evaluates LIVE at the roll instant — the rule is about
  the count of evaluations, not their world; the `Measure` snapshot
  feeds only `Plan` evaluations.

### Added — `proc_rolls` (P8e)

```jsonc
{ "defaults": { "proc_rolls": "per_cast" } }     // | "per_hit"
// or per proc (the override lives on the proc — rolling is the proc's
// semantics; the hit count is already the action's):
{ "procs": { "spark": { "trigger": "on_hit", "chance": "0.2",
                        "rolls": "per_hit", "apply_buff": "glow" } } }
```

How a proc's chance is rolled against one damaging cast's hits.
Governs the hit-driven triggers (`on_hit`, `on_crit`); an `on_cast`
proc's event is the cast itself and rolls once per cast under either
setting (the instant-cast × `measure` precedent: documented, not an
error).

- **`per_cast`** (default) — one roll per damaging cast,
  `hits_per_use`-blind: the long-standing behavior, bit-identical
  (including the Monte Carlo RNG draw stream) for every config that
  names neither field, proven by the untouched suite and the
  byte-identical `diablo4_rotation` MC block.
- **`per_hit`** — one roll per MEASURED hit (the snapshot's
  `hits_per_use`, the same value the damage is multiplied by): the EV
  accumulator is fed once per hit (the `on_crit` crit-probability
  weight applies per hit) and multiple crossings can fire per cast at
  `icd: 0`; MC draws one Bernoulli per hit. The pinned fractional
  contrast: chance 0.2 × 5 hits × 20 casts → 4 fires per-cast vs 20
  per-hit.
- **The ICD-at-one-instant rule, stated and pinned:** all hits of one
  cast land at the same instant, so any `icd > 0` caps fires at one
  per cast even under `per_hit` — pinned as an EQUALITY (per_hit ==
  per_cast == 7 fires at chance 1 / icd 3 / 20 casts, both modes). A
  mid-cast fire hard-gates the cast's remaining hits (their mass is
  discarded, not banked — banking is exactly the EV-over-MC inflation
  the P6-review I1 fix removed; the per-hit ICD-bound EV/MC agreement
  regression pins 20 EV fires against the pooled MC mean). In MC a
  gated hit consumes NO RNG draw — the gate precedes the draw, pinned
  by full-report byte-equality: at chance 1 / `icd > 0` the per-hit
  stream collapses to the per-cast stream exactly, downstream samples
  included.
- **Chance is evaluated once per proc per cast** — the hits are
  simultaneous and share one measured world (P8c), so a fire mid-cast
  is never visible to its own sibling hits' `chance` (it IS visible to
  later procs in the batch and to later casts) — pinned by a
  self-feeding `chance` contrast.
- **Fail-closed, twice:** `per_hit` rolls a literal count, so a
  fractional measured `hits_per_use` (legal under the hits-blind
  `per_cast`) is a positioned run error naming the proc, the action,
  and the value — and the count is capped at 10,000 rolls per cast
  (`PER_HIT_ROLL_LIMIT`, golden-tested at the boundary): one config
  line must not hang the run loop, in an engine consumers run as WASM
  in a browser tab. Above the cap the same positioned error names the
  limit; the cap also closes the two large-float edges (values `≥2^53`
  are trivially integral, `as u64` would saturate) before they matter.
- Unchanged rules, restated where a reader will ask: a utility cast
  presents no hit roll under either setting, and a proc-triggered free
  cast rolls no procs at all (P7d).

### Changed — Rust API (P8e)

- `SimDefaults` gains the public `proc_rolls: ProcRolls` field and
  `ProcDef` the public `rolls: Option<ProcRolls>` field
  (source-breaking for exhaustive struct literals; neither consumer
  constructs either). `ProcRolls` is `#[non_exhaustive]` from birth,
  for the `Measure`/`EventOrder`/`EffectDef` reason — a third policy
  (a per-projectile-chain roll, a capped per-hit variant) is plausible
  and would land on this enum.
- `sim::CompiledProc` (already `#[non_exhaustive]`) carries the
  resolved policy per proc; the executor reads it at every roll, never
  the `defaults` block.

### Added — `event_order` (P8d)

```jsonc
{ "defaults": { "event_order": "scheduled" } }   // | "completions_first"
```

Which of two COINCIDENT queue events resolves first is now config.
SimDef-global ONLY, by design — ordering is a property of the queue, and
a collision involves two entities, so a per-spell tie-break would be
incoherent (the rationale lives on the `EventOrder` type).

- **`scheduled`** (default) — the honest name for the long-standing
  behavior: coincident events resolve in scheduling (`seq`) order.
  "Expiry before completion" was never a rule, only incidental — an
  expiry usually holds the lower `seq` because it was scheduled at the
  buff's application, earlier than the completion. Bit-identical to
  0.3.0: the rank the queue now carries is a CONSTANT under this
  setting (`QueueItem` orders by `(time, class_rank, seq)`, the rank
  computed at push), proven by the untouched suite and the
  byte-identical `diablo4_rotation` MC block.
- **`completions_first`** — every `CastComplete` outranks every
  coincident `BuffExpire`/`PhaseBoundary`/`Wake`; within a class, `seq`
  still decides, so seeded Monte Carlo stays deterministic under every
  setting.
- **The cast-grid footgun gains its second config fix** (`sim` module
  docs, "A buff expiring on the cast grid"): P8c's
  `measure: "cast_start"` moves the MEASUREMENT off the collision;
  `event_order: "completions_first"` moves the COLLISION itself — the
  completing cast measures WITH its still-live buff and its
  reapplication makes the pending expiry stale. `poe2_triggers` runs
  both against the same on-grid 2.0s shock: each alone restores bolt
  damage 1837.5 → 2175 at the same 0.95 uptime.
- **Pinned consequence, stated as designed:** the horizon cast of a
  zero-weight final phase — whose `scheduled` attribution (boundary first:
  900 / 250 / 1150) the 0.3.0 pin recorded as an incidental consequence
  of `seq` order — flips under `completions_first` to 1000 / 0: the
  completion resolves before the boundary and is measured under, and
  attributed to, the OLD phase. Horizon-drain semantics (P7e-T2) are
  unchanged: the knob decides which event at `t == duration` resolves
  first, never whether it resolves.
- **Within a class, `seq` still decides — pinned, not just documented:**
  a buff expiry and a phase boundary sharing an instant under
  `completions_first` resolve in scheduling order, observable through an
  instant cast whose eligibility flips at the expiry and whose damage
  names the phase it measured under
  (`within_the_rest_class_seq_still_decides_under_completions_first` —
  sub-ranking the rest class is NOT behavior-preserving).

### Changed — Rust API (P8d)

- `SimDefaults` gains the public `event_order: EventOrder` field
  (source-breaking for exhaustive struct literals; neither consumer
  constructs one). `EventOrder` is `#[non_exhaustive]` from birth, for
  `Measure`'s reason — a third policy (an explicit expiries-first, a
  per-class rank table) is plausible and would land on this enum.
- `sim::SimPlan` (already `#[non_exhaustive]`) carries the resolved
  order; the executor reads it at every event push, never the
  `defaults` block.

### Fixed — one world per measured cast (P8c: the phase's ONE deliberate behavior change)

Through 0.3.0, a snapshot `tick_objective` captured by an ACTION's
`apply_buff`/`effects` list read a FROZEN build against the LIVE
effective phase — so one buff driving both a bucket contribution and a
condition landed on a later entry's capture through one axis and not the
other, and a pure reorder of `["mark", "poison"]` DOUBLED the DoT at an
identical reported uptime (the 0.3.0 pin
`a_same_list_snapshot_capture_reads_a_frozen_build_but_a_live_phase`,
400 vs 800, flagged there as an open question).

Now every `Plan` evaluation in one cast's completion transaction reads
the cast's ONE `WorldSnapshot` — effective build AND effective phase,
captured together at the action's measured instant: the damage query,
`hits_per_use`, the EV `on_crit` weight, and every `ApplyBuff` entry's
tick capture. Both orderings of the list above now capture the pre-list
world; the pin is re-derived as an EQUALITY plus the literal
(`a_same_list_snapshot_capture_reads_one_frozen_world`: both 400 — the
old poison-first value; the fix moves mark-first DOWN, it invents no
third number).

Scope, precisely:

- **Sim-FIELD expressions are untouched.** `duration`/`cost`/`gain`
  keep their P7b instants and live sequential sim-state reads (the
  pandemic `buff_remaining` idiom, the P8b 0.2/0.1 effects-order pin,
  and the whole `expr_fields` net are byte-identical). "One world"
  governs `Plan` evaluations, never sim-state reads.
- **The proc path is untouched.** A proc-applied buff still captures
  the live ambient world at the fire, sequentially across the proc's
  own effects list; a proc-fired `cast_action` FREE cast is measured
  live at its own instant, never frozen to the triggering cast's
  snapshot (pinned by
  `a_free_cast_measures_live_ambient_not_the_outer_casts_snapshot`).

**Migration note — who moves:** numbers change ONLY for a config where
one action's `apply_buff`/`effects` list contains BOTH a buff that
drives a condition AND a later snapshot `tick_objective` whose objective
reads that condition (the captured rate drops to the pre-list value).
Neither consumer qualifies: `../diablo4-calc` and `../poe2-calcs` define
no `tick_objective` at all in any committed config, and both were
re-verified byte-identical (d4 default eval `17574.299999999996`, full
workspace green; poe2 `rtce_parity` 63/63). Every rtce example is
byte-identical, `diablo4_rotation`'s EV and MC blocks included.

### Added — the `defaults` block and `measure` (P8c)

```jsonc
{
  "defaults": { "measure": "cast_complete" },   // | "cast_start"
  "actions": { "bolt": { "measure": "cast_start", ... } }  // per-action
}
```

- **`defaults`** — package-wide semantic defaults, the new home for
  P8's knobs (`event_order` joined in P8d, above; `proc_rolls` joins in
  a later slice).
  Omitted (every 0.2.0/0.3.0 config) = every knob at its 0.3.0-behavior
  value; the block round-trips away unless it carries content, and it
  gets the full P8a unknown-key treatment ("unknown field `measur` on
  the defaults block — did you mean `measure`?").
- **`measure`** — the instant a cast's world is captured at:
  `cast_complete` (default — today's instant, byte-identical) or
  `cast_start` (the world the cast leaves behind as it begins: cost
  paid, cooldown armed, `casts.<self>` NOT yet counted, `gain` not yet
  credited). `ActionDef.measure` overrides per action. An INSTANT cast
  is always measured at the completion position, whatever `measure`
  says — the two share the wall-clock instant, and the intra-instant
  discontinuity this implies for `casts.<self>` (a zero-time cast
  counts from 1, an epsilon-time one from 0) is documented on `Measure`
  and pinned. The snapshot rides the in-flight cast; the completion
  transaction reads it instead of measuring afresh, and everything else
  about the transaction's order is unchanged.
- Teaching contrast in `examples/poe2_triggers.rs`: the cast-grid
  footgun (`shock` at an on-cadence 2.0s, bolt damage 2175 → 1837.5 at
  identical 0.95 uptime) is now FIXABLE by config —
  `defaults.measure: "cast_start"` restores 2175 (150 + 9×225,
  hand-worked: every bolt after the first starts strictly inside the
  previous completion's shock window). The `sim` module's footgun
  section now ends with that config.

### Changed — Rust API (P8c)

- `SimDef` gains the public `defaults: SimDefaults` field and
  `ActionDef` the public `measure: Option<Measure>` field
  (source-breaking for exhaustive struct literals of those types;
  neither consumer constructs them). `Measure` is `#[non_exhaustive]`
  from birth, for `EffectDef`'s reason: a third measurement instant
  (projectile impact, say) is plausible, and it would land on this enum.
- `sim::CompiledAction` gains `measure: Measure`, the RESOLVED instant
  (`#[non_exhaustive]`, only `compile` constructs it).

### Added — ordered `effects` list on actions and procs (P8b)

What an action does at cast complete and what a proc does when it fires
are now ONE ordered list each — `ActionDef::effects` /
`ProcDef::effects`, externally-tagged entries
(`{ "apply_buff": "shock" }` / `{ "cast_action": "comet" }`):

```jsonc
"procs": {
  "trigger_gem": {
    "trigger": "on_cast", "chance": "1", "icd": 3.0,
    "effects": [ { "apply_buff": "shock" },
                 { "cast_action": "comet" },
                 { "apply_buff": "shock" } ]   // repeats apply twice
  }
}
```

- **List order is execution order**, and sim state is SEQUENTIAL between
  entries (the P7b rule): a `cast_action` free cast bumps `casts.<name>`
  and lands its gains/damage/own-ApplyBuff before the next entry runs,
  so a later `apply_buff`'s `duration` expression sees it — pinned by a
  0.2-vs-0.1 uptime contrast where reversing a two-entry list is the
  only change. A repeated entry applies that many times (the P7d list
  precedent).
- **A proc can now do several things** — the 0.3.0 "exactly one of
  `apply_buff`/`cast_action`" limitation is gone, generalized to "a
  proc must do something" (an empty list after desugar is the compile
  error; BOTH sugar fields at once is still refused with its
  long-standing message).
- **`cast_action` stays proc-only.** On an `ActionDef` it is a
  positioned compile error: an action free-casting an action reopens
  the A→B→A recursion the free-cast guard closed (a free cast rolls no
  procs, so today's chains are one link long by construction); a
  bounded-depth chain design is tracked in `ROADMAP.md` for a config
  that actually needs one.
- **DEPRECATED (kept for 0.x): `ProcDef::apply_buff`,
  `ProcDef::cast_action`, `ActionDef::apply_buff`.** Each is sugar for a
  one-entry (per name) `effects` list and desugars at `sim::compile`
  into the IDENTICAL compiled form — every 0.3.0 config parses and runs
  byte-for-byte unchanged (the `diablo4_rotation` EV and MC blocks are
  the standing proof). Mixing sugar with an explicit `effects` list on
  ONE entity is a compile error ("ambiguous order — migrate the sugar
  into the `effects` list"). This also settles the `apply_buff` arity
  wart the 0.3.0 docs deferred: one list shape, both entities.
- **Typos inside an effect entry fail closed in the P8a voice**:
  `{ "apply_buf": "x" }` → ``unknown field `apply_buf` on an effect
  entry — did you mean `apply_buff`?`` (hand-written `Deserialize`
  replacing serde's "unknown variant" default); `_`-prefixed annotation
  keys are accepted inside an entry like everywhere else, and an entry
  must hold exactly ONE effect key.
### Changed — Rust API (P8b)

- `sim::ProcEffect` is renamed `sim::CompiledEffect`, and the compiled
  `CompiledProc.effect` / `CompiledAction.apply_buff` fields are
  replaced by `effects: Vec<CompiledEffect>` on both (the structs are
  `#[non_exhaustive]` and only `compile` constructs them —
  source-breaking only for code matching the old names; neither
  consumer does).
- `ActionDef` / `ProcDef` gain the public `effects` field
  (source-breaking for exhaustive struct literals of those two types;
  neither consumer constructs them).
- `EffectDef` is `#[non_exhaustive]` from birth, for the same reason
  `CompiledEffect` is: a later effect kind must land on both enums, so
  an exhaustive config enum would make it breaking anyway. Downstream
  `match`es over it need a wildcard arm; construction is unrestricted.

### Changed — unknown config keys now fail closed (P8a)

Every config struct now REJECTS a key it does not declare, instead of
silently ignoring it. In 0.3.0 a typo'd key parsed and silently meant
"field absent" — `"tick_objectiv"` silently meaning "no DoT" was the
standing example — which for search-loop consumers is a silent wrong
answer priced across thousands of configs. The error is positioned and
carries its own remedy:

```
unknown field `tick_objectiv` on buff `poison` — did you mean `tick_objective`?
```

**Migration:** rename the key the error names (the did-you-mean is the
fix in every observed case); when nothing is within edit distance 2 the
error lists the valid fields instead. Configs with no typos — including
both consumers' committed gamedefs — pass unmodified.

- **The `_` namespace is the documented annotation escape hatch.** Keys
  starting with `_` (`_source`, `_scope`, `_shape`, …) are accepted at
  EVERY nesting level, exactly as the committed fixtures already use
  them. On the `SimDef`-side structs they are collected into a new
  public `extra` field (serde flatten) and survive serde round-trips; on
  the structs below they are accepted and dropped at parse, which is the
  same fate the 0.3.0 derived `Deserialize` gave them.
- **Where rejection happens.** `SimDef`, `ResourceDef`, `ActionDef`,
  `ActionDamage`, `BuffDef`, `ProcDef`, `Rotation`, `Rule` collect
  unknowns and fail at `sim::compile`; `EventDef` likewise at
  `plan::compile` — in all nine cases the error names the entity's
  registry name. `GameDef`, `BucketDef`, `StageDef`, `BuildState`,
  `Contribution`, `Scenario`, `Phase` reject at PARSE via hand-written
  `Deserialize` impls instead (with the context the struct itself
  carries, e.g. ``phase `boss` ``): each of these seven is constructed
  in Rust with exhaustive struct literals in at least one consumer (the
  union across the two consumers covers all seven), so they deliberately
  gain **no** new field.
- **Rust source compatibility:** adding the public `extra` field to the
  nine collect-side structs is source-breaking for exhaustive struct
  literals of THOSE types (add `extra: Default::default()`); neither
  consumer constructs any of them, and the seven structs consumers do
  construct are unchanged.
- **`NumOrExpr` reports what it expected.** A malformed value (e.g.
  `"cooldown": true`) now errors with `expected a number (literal) or a
  string (expression)` instead of serde's "data did not match any
  variant of untagged enum".
- **`tick_objective` object form.** The hand-written map visitor
  replaces the `TickObjectiveObj` + `deny_unknown_fields` machinery; an
  unknown key now reads ``unknown field `snapshots`, expected
  `objective` or `snapshot` `` (0.3.0's error was serde's untagged "did
  not match any variant"), `_` keys are accepted there too, and the
  bare-string form plus the serialize-live-back-to-bare-string
  canonicalization are unchanged.

### Fixed — non-finite config numbers no longer come back `Ok(NaN)` (P8a)

Validation debt from the 0.3.0 release review: a `NaN`/`inf` in the
inputs below used to propagate through the folds and return as an
`Ok(NaN)` / `Ok(inf)` objective, silently. Each is now a positioned
error, mirroring the existing `Phase.uptimes` "must be finite" rule:

- `Contribution::value`, on BOTH halves of the shared type — a
  `BuildState` contribution at `Plan` build resolution ("contribution
  value into bucket `boost` must be finite, got NaN") and a
  `BuffDef::contributions` entry at `sim::compile` (named with its
  buff).
- `BuildState.stats` values, at `Plan` build resolution.
- `Phase.stats` override values, alongside the existing uptime check.
- The same three, ONCE at `sim::run` entry (before the event loop):
  a utility-only rotation completes with zero `Plan` evaluations, so the
  per-evaluation checks above never fired on that route and a NaN build
  used to come back `Ok(dps = 0)` silently while the NaN flowed into
  rule gates and resource regen.

No numeric path changed: `diablo4_rotation`'s EV and Monte Carlo blocks
are byte-identical, and both consumers' pinned numbers are untouched.

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
a number still reaches the executor as the identical `f64`. Three things
can still surprise you.

**Four behavior fixes can move numbers**, each for a narrow class of
config. Full detail in "Fixed" below; the short form:

| Fix | Moves numbers for |
|---|---|
| the fight horizon is DRAINED | any config where a SECOND event (usually a `BuffExpire`, but a `PhaseBoundary` too) lands on the fight's exact end instant alongside a completing cast — most often integer buff durations against an integer `duration`. The cast there was dropped whole: count, damage and `apply_buff`. |
| EV `on_crit` weight measured before the cast's own procs | configs with an `on_cast`/`on_hit` proc that changes crit chance, run under `Mode::Expected`. |
| a proc's effect is visible to a later proc in the same batch | configs whose proc `chance` reads sim state (`casts.*`, resources, `buff*.*`) that another proc mutates in the same trigger batch. |
| a resource's `max`/`regen_per_sec` re-derived against the state that caused the refold | configs whose resource `max`/`regen_per_sec` names sim state rather than plain stats/conditions. |

Three further fixes are in "Fixed" but are NOT in that table, because
none of them can move a 0.2.0 config's damage or dps: the frozen
`apply_buff` overlay (that surface is new in 0.3.0), the
`condition_uptime` clamp (report-only), and the `ProcDef::icd` validation
(which REJECTS a config rather than re-scoring it — see below).

Nothing in this repo moved: `examples/diablo4_rotation.rs` holds byte for
byte at 225199.1088 total / 3753.31848 dps / 0.4 vuln uptime, and both
downstream consumers are byte-identical.

**One config that compiled in 0.2.0 no longer does.** `ProcDef::icd` is
now required to be finite and `>= 0`. That can only reject a config whose
`icd` was NaN, infinite, or negative — and a NaN one was silently running
with NO internal cooldown at all, so if this error fires, the numbers you
had were wrong. (A second, far more pathological rejection is described
under the horizon drain: a scenario ending in more than 10,000
zero-weight phases.)

**Rust source-breaking (permitted under 0.x, but you deserve the
warning).** New fields on `ActionDef` (`apply_buff`), `ProcDef`
(`actions`) and `BuffDef` (`max_stacks`, `on_reapply`) break any
exhaustive struct literal constructing them in Rust. JSON is unaffected.
The fix: add the new fields explicitly with their 0.2.0-equivalent
values — `apply_buff: Vec::new()` on `ActionDef`, `actions: None` on
`ProcDef`, `max_stacks: 1` and `on_reapply: ReapplyPolicy::Refresh` on
`BuffDef` — or stop writing them exhaustively. `ActionDef` and `BuffDef`
both have a `Default` (`BuffDef`'s is hand-written precisely so
`max_stacks` defaults to `1` rather than `u32`'s `0`), so
`ActionDef { cast_time: "1".into(), ..Default::default() }` and
`BuffDef { duration: 4.0.into(), ..Default::default() }` both compile and
both give you 0.2.0 behavior. `ProcDef` has no `Default`; name
`actions: None` there.

Field TYPES also changed. `BuffDef::tick_objective` is now
`Option<TickObjective>` rather than `Option<String>`, and the five newly
expression-capable fields — `BuffDef::duration`, `ActionDef::cooldown`,
and the values of `cost` / `gain` / `ActionDamage::stats` — hold
`NumOrExpr` where they held `f64` (both spellings still deserialize;
`NumOrExpr: From<f64>` covers Rust constructors). On the compiled
side, `CompiledBuff::tick_objective` is `Option<CompiledTick>` rather
than `Option<usize>`, and `SimReport::buff_uptime` is REPLACED by
`buffs: BTreeMap<String, BuffReport>` (`.uptime` plus the new
`.avg_stacks`).

**`#[non_exhaustive]` swept across every engine-produced type.** Adding the
attribute is itself a breaking change, so it wants a release that permits
one; under 0.x every minor bump does, and this is the first one after the
compiled and report surfaces settled. Now marked: every COMPILED type
(`SimPlan`, `CompiledAction`, `CompiledBuff`, `CompiledProc`,
`CompiledResource`, `CompiledRule`, `CompiledTick`, `CompiledValue`, and
the `ProcEffect` enum), plus `expr::Op` — the compiled instruction enum
reachable through `Program::ops()`, which P6 grew by nine variants — every
read-only REPORT type (all seven in `sim::report`, plus
`plan::Explanation` / `PhaseTrace` / `BranchTrace` and
`search::CandidateResult`), and both ERROR types (`PlanError`,
`ExprError`). The rationale is uniform: the engine constructs them, no
consumer does, and every sequencing phase so far has added a field or a
variant to one — so later measurements should be additive rather than
breaking. If you construct or exhaustively destructure any of them, you
cannot after 0.3.0; read them field by field instead, and `match` on
`ProcEffect` and `Op` with a `_` arm.

The CONFIG types are deliberately NOT marked, so a caller building a
`GameDef`/`SimDef`/`BuildState`/`Scenario` in Rust can still write a
struct literal.

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
  distribution, different phase: the new mean sits 0.18% from the EV pin
  and the old one 0.27%, both well inside the example's own 2% band.
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

  Within one `apply_buff` list, what a later entry sees splits across
  THREE axes — not the two an earlier draft of these docs claimed, and the
  third is a genuine trap:

  - **sim STATE is SEQUENTIAL.** The slot array is refreshed per entry, so
    a `duration` expression sees earlier entries' stack counts and live
    windows.
  - **the BUILD is FROZEN**, captured once before the list runs, so a
    snapshot magnitude does NOT see earlier entries' `contributions`.
  - **CONDITIONS are LIVE.** They are not part of the frozen build; the
    effective phase is rebuilt from every live buff on every application.
    So a snapshot magnitude DOES see a condition an earlier entry drives.

  The consequence: for a snapshot `tick_objective` whose objective reads a
  condition, LIST ORDER alone changes the captured rate, and no integrated
  report column shows it. Pinned by
  `a_same_list_snapshot_capture_reads_a_frozen_build_but_a_live_phase`,
  where reordering a two-entry list doubles the DoT (400 → 800) at an
  identical 0.8 reported uptime. Whether the phase should be frozen
  alongside the build is an open 0.4.0 question in `ROADMAP.md` — a
  behavior change, deliberately not made here.

  What the freeze DOES buy is intact, and is also pinned in both list
  orders: the damaging and utility paths agree. A damaging action freezes
  its overlay, a utility action freezes the plain effective build, and the
  same list means the same thing on both.

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
  literal — this repo had to touch every fixture in the file. Permitted under
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

  **Upgrading from 0.2.0: this can move your numbers.** Not every config
  with integer buff durations — the condition is narrower, and stating it
  as an absolute would be wrong: a SECOND event has to land on the
  horizon alongside the cast completing there. A buff whose expiries
  happen to fall on `duration` does it; one whose duration is long enough
  that no expiry reaches the horizon at all does not, and neither does
  one whose expiries simply miss it. The symptom is an off-by-one cast
  count that depends on the buff duration: a 10s fight of 1s casts
  applying a `refresh` buff reported 9 casts at durations 2 / 5 / 9 —
  and a CORRECT 10 at duration 9.5, and a correct 10 at any duration ≥
  10, where no expiry lands on t=10.

  A `BuffExpire` is only the most likely second event; a `PhaseBoundary`
  from a trailing zero-weight phase does it too. The bug was never
  specific to buffs — it was "at most one event resolves at the
  horizon". Nothing in this repo moved
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
  That partition is two-way only if you leave CONDITIONS out of it, and
  they are a third axis — LIVE, not frozen. See "Frozen at the world the
  cast found" below for the correction.

- **`ProcDef::icd` is validated fail-closed at `sim::compile`.** It is a
  bare literal rather than a `NumOrExpr`, so it never went through P7b's
  evaluation-instant checks. Both bad values failed SILENTLY, and NaN
  failed in the worst direction: the ICD gate is `now < icd_ready_at`,
  false for every `now` once that deadline is NaN — so `icd: NaN` DELETED
  the internal cooldown instead of tightening it, turning a gated proc
  into an ungated one with no error anywhere. `icd` must now be finite
  and `>= 0`, with the same positioned error its neighbours produce.
  **Upgrade note:** this rejects a config that compiled in 0.2.0, but
  only one whose `icd` was NaN, infinite, or negative — none of which can
  be deliberate. Pinned by
  `a_non_finite_or_negative_proc_icd_is_a_compile_error`.

  **This does NOT close out the sim config's numeric validation**, and the
  entry should not be read that way. `BuffDef::contributions[].value` is
  still a bare `f64` that nothing checks — `sim::compile` clones it
  straight through, and `Plan` validates phase weights and phase uptimes
  but not contribution values — so a NaN there still returns `Ok(NaN)`
  total damage and an infinity still returns `Ok(inf)`, both silently.
  That gap ships in 0.3.0; it is recorded with its repro under 0.4.0 in
  `ROADMAP.md`. What `icd` was is the last unvalidated number that failed
  in the SAFETY-OFF direction (NaN deleting a gate rather than poisoning
  a result).

- **`SimReport::condition_uptime` is clamped to `[0, 1]` for buff-driven
  values.** A condition is an uptime FRACTION, and `Plan` clamps it to
  that range where it actually folds — but the executor's integrator read
  `BuffDef::conditions`' raw number, so a buff writing `{ "marked": 5.0 }`
  folded as `1.0` and REPORTED `5 ×` its live fraction. The diagnostic
  disagreed with the value the math used. `Sim::condition_value`'s
  scenario branch had always clamped; the buff branch now matches.
  **Report-only** — no damage, uptime or dps number moves, only the
  `condition_uptime` map, and only for a config that wrote a value outside
  `[0, 1]` in the first place. Pinned by
  `a_buff_driven_condition_uptime_is_clamped_like_the_value_that_folds`.

### Documented (behavior unchanged, but it was not written down)

Three composition rules that a config could hit in 0.2.0 and that no
public doc stated. None of them changes here; all three are now pinned so
that changing them later has to be deliberate.

- **`Trigger::OnHit` rolls once per damaging CAST, not once per hit.** The
  name says otherwise, and `hits_per_use: 5` still presents exactly one
  roll — weight `1.0` in EV, one draw in MC. A lucky-hit-style proc that
  should scale with a multi-hit skill is not expressible; fold the
  per-hit rate into `chance` by hand. Pinned in both modes by
  `on_hit_rolls_once_per_cast_not_once_per_hit`, and whether it SHOULD
  scale is an open 0.4.0 question in `ROADMAP.md`.
- **Two live buffs driving the same condition resolve by BUFF NAME
  order.** The alphabetically first live buff wins — not the strongest,
  not the most recently applied, and they are never summed. Renaming a
  buff can therefore change your number. Pinned, rename control included,
  by `two_buffs_driving_one_condition_resolve_by_buff_name_order`.
- **A proc-triggered free cast, and every DoT tick, stay EV-blended under
  `Mode::MonteCarlo`.** Both were v1 scope limits recorded only in a
  PRIVATE module doc, which never rendered on docs.rs while the public
  `Mode::MonteCarlo` said damage and crits are sampled with no exception.
  The caveats now live on the public type.

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

- **"Frozen at the world the cast found" was too strong, and is
  corrected.** The `apply_buff` capture semantics were documented as a
  two-way split (sim state sequential / build frozen). There is a THIRD
  axis: CONDITIONS are LIVE, because `refresh_effective_state` rebuilds
  the effective phase on every application while the build clone stays
  put. So a snapshot capture DOES see a condition an earlier list entry
  drives, and list ORDER alone can change a captured DoT rate — pinned at
  a 2× swing with an identical reported uptime. Corrected in four places
  (`sim` module docs, `ActionDef::apply_buff`, `Sim::apply_action_buffs`,
  and the P7d entry above). Behavior is unchanged, and the property the
  freeze exists for is intact and now pinned in both list orders: the
  damaging and utility action paths agree.

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
  runs via a new zero-dependency in-crate PCG32 and `Plan`'s internal
  sampled per-phase evaluation,
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

[Unreleased]: https://github.com/benjamin-small/rpg-theorycraft-engine/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/benjamin-small/rpg-theorycraft-engine/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.4.0
[0.3.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.3.0
[0.2.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.2.0
[0.1.0]: https://github.com/benjamin-small/rpg-theorycraft-engine/releases/tag/v0.1.0
