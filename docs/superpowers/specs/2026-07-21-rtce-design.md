# rtce — RPG Theorycraft Engine, v0 Design (2026-07-21)

**North star:** a generic, config-driven theorycrafting engine extracted from
the proven twin implementations `../diablo4-calc` and `../poe2-calcs`. The
game's ALGORITHM — stats, fold rules, probabilistic events, and the damage
pipeline itself — is configuration, compiled once at load into an efficient
evaluation plan. The primary workload is SEARCH: an external driver (e.g. a
knowledge-graph explorer) submits thousands to tens of thousands of config
permutations; the engine prices them and returns results.

Decisions locked with the user (2026-07-21):
- **Approach A — compiled IR** (config → compile once → flat-array
  evaluation), over closure composition and codegen/JIT.
- **Crates: `rtce` + `rtce-testkit`** — names verified AVAILABLE on
  crates.io 2026-07-21 (rtce, rtce-core, rtce-testkit all 404).
  Repo: `../rpg-theorycraft-engine`, Cargo workspace.
- **Consumers: diablo4-calc migrates first** (its parity suite is the
  migration oracle); poe2-calcs INFORMS the schema but does not migrate yet.
- **Knowledge graph: completely out of scope.** The engine's contract is
  only: accept candidate configs/mutations, evaluate, return results. Move
  GENERATION (what is legal to try) lives with the external driver.

## Architecture

```
GameDef  (config: stats, buckets, events, pipeline)  ── compile once ───► Plan (IR)
Scenario (config: phases, targets, uptimes)          ── compile once ───► phase table
BuildState (one candidate's mods/stats)              ── per candidate ──► f64 arrays
Plan.evaluate(&BuildState, &Scenario)  → EvalResult  (per-scenario objectives; hot path: no alloc/parse/hash)
Plan.explain(&BuildState, &Scenario)   → Breakdown   (per-stage traces, teaching path)
search::price(plan, base, moves, scenarios) → results (apply → eval → revert; top-K / Pareto)
```

### GameDef — the algorithm as configuration

Four schema-validated domains:

1. **Stats** — a registry of named stats (replaces hardcoded enums);
   compiles to array offsets.
2. **Buckets** — fold declarations per mod kind: `sum`, `additive_pool`
   (the shared one-big-bucket rule), `summed_group` (Σ within type, groups
   multiply), `independent_product`. New games declare combinations, not code.
3. **Events** — probabilistic branches: name, chance expression, baseline
   multiplier, buckets that OPEN only on that branch. The engine enumerates
   the 2ⁿ event product with weight expressions — D4's 4-branch crit/OP
   expected value is pure config; other games get the machinery free.
4. **Pipeline** — ordered named stages in a small expression language over
   stats, buckets, events, and prior stages: arithmetic, conditions, clamps,
   step functions (breakpoints), min/max. Example (D4 sketch):
   `stage base_hit = weapon_avg * skill_coeff/100 * (1 + mainstat/divisor)`.

### Two-tier compilation (the performance contract)

- GameDef compiles ONCE per session: expressions → compact op arrays, stat
  names → offsets, event branches pre-expanded, stage dependency graph
  recorded. Compilation may be arbitrarily thorough (validation, const
  folding); it is off the hot path.
- BuildState is the only per-candidate artifact: a stat/mod vector. Evaluate
  = a tight arithmetic loop over f64 arrays. Budget: single-digit
  microseconds per evaluation → 10k permutations in tens of milliseconds;
  million-candidate sweeps remain feasible.
- The recorded stage dependency graph is the hook for dirty-flag partial
  re-evaluation later. NOT built until search profiling proves the need.

### Scenarios (playbooks) — the encounter as configuration

The third config tier alongside GameDef and BuildState: a Scenario models
THE FIGHT being asked about — "1-minute single boss" vs "3-minute pack
clear" are different scenario files, not different engines.

- **Level 1 (ships with the core): weighted-phase blending.** A scenario =
  named phases with durations/weights and per-phase overrides: target
  profile (count, DR, elite), and CONDITION UPTIME FRACTIONS replacing
  today's booleans (vulnerable 0.65 of the time, not on/off). Evaluation =
  K phase-evals of the same Plan, blended — cost stays microseconds, so
  the search budget is untouched. `EvalResult` carries per-scenario
  objectives (`boss_dps`, `clear_dps`, …); collectors can Pareto across
  scenarios ("best boss build" vs "best clear build" falls out).
- **Level 2 (deferred, P6): timeline simulation.** Rotation/priority
  scripts, cooldowns, resource, procs, buff stacking computing its own
  uptimes over the scenario's duration — also config (the "emulation
  sequence"), sharing the SAME Scenario schema so fidelity upgrades don't
  change the interface. Cost moves to milliseconds/candidate; search still
  works with an adjusted budget. Not built until Level 1 proves the shape.

### Search module (pricing only — no generation)

- `Move`: a reversible BuildState mutation (add/remove/replace a mod, set a
  scalar). Apply → evaluate → revert without re-resolving.
- Batch evaluator over candidate streams; collectors: top-K by objective,
  Pareto over multiple objectives.
- The external driver (knowledge-graph explorer or anything else) decides
  WHAT to try; this engine only answers "what is it worth". The interface
  must stay serialization-friendly so drivers can live out-of-process.

### Breakdown stays first-class

`explain()` runs the same Plan with per-stage tracing on: every stage value,
bucket contents, branch table. This is the teaching identity of the parent
projects (the web formula explorer, the CLI hand-check artifact) and the
migration hand-shake. Tracing is OFF on the search path.

### Data plumbing (moved from the games — already generic)

- Two-axis versioning: manifest {live/latest, per-version meta}, DATA_SCHEMA
  warn-never-crash.
- Structured diagnostics: Errors withhold results, Warnings adjust exactly
  one thing and say so.
- `rtce-testkit` (dev-dependency): the golden-fixture runner (refuses an
  empty dir; source URL + date per fixture) and property-test helpers
  (order independence, sum≠product, EV bounds) — consumer games inherit the
  testing discipline.

## What diablo4-calc keeps vs sheds (migration, parity-gated)

Keeps: catalogs + importers (fail-closed, Blizzard-IP hygiene), the vocab
grammar (`parse_field` now mapping to registry stat NAMES), resolution
semantics (BuildConfig × catalog → BuildState), web UI + WASM adapter shape,
Neo4j emitters. Sheds: `stats.rs` + `calc.rs` math, replaced by a committed
`gamedef/` (the D4 pipeline as rtce config).

**The gate is absolute:** all 7 parity builds (Rapid Fire 8,096.02 …
fireball_endgame 6,769.10), T1–T10, and the property suite must reproduce
TO THE DIGIT through rtce before the old math is deleted.

## Testing (built first, per house style)

- rtce's own closed-form unit tests with hand-worked arithmetic; proptest
  invariants on the fold/event machinery; a TOY GAME GameDef fixture proving
  generality independent of D4.
- Golden fixtures via rtce-testkit; a pinned default number per phase.
- TDD red-first throughout; mutation-checks for pinned numbers.

## Phasing (each phase its own plan/session; small verified slices)

- **P1** — repo scaffold, `rtce` + `rtce-testkit` crates, expression
  language + compiler (parse → IR → eval), TDD'd like diablo4-calc M1/M2.
- **P2** — stat registry, bucket combinators, event enumeration, pipeline
  stages, and the Scenario schema with Level-1 phase blending; the toy game
  evaluates end-to-end against two scenarios with pinned numbers.
- **P3** — D4 GameDef: T1–T10 reproduce through rtce.
- **P4** — full parity (7 builds) + diablo4-calc switchover to the path
  dependency; delete the duplicated math.
- **P5** — search module (Move, batch pricing, collectors, multi-scenario
  Pareto) + out-of-process driver interface; subsumes diablo4-calc's M9
  optimizer.
- **P6** — Level-2 timeline simulation (rotation scripts, cooldowns,
  resource, procs) behind the same Scenario schema; built only after
  Level-1 blending has proven the shape on real questions.

## Dependency mechanics (no submodules)

Development: `rtce = { path = "../rpg-theorycraft-engine/crates/rtce" }` in
diablo4-calc. After first push: git dependency; crates.io publication
optional later (names reserved by checking availability, not squatting).

## Out of scope

- Knowledge-graph construction/exploration (external driver's job).
- poe2-calcs migration (second-consumer proof, later project).
- Dirty-flag incremental evaluation (hook recorded, build on evidence).
- JIT/codegen (rejected: complexity + WASM-hostile; IR is fast enough).

## Done since

- 2026-07-21 — P1 complete: workspace (rtce + rtce-testkit), expression
  language end-to-end (lexer → parser → postfix Program → stack evaluator;
  min/max/clamp/floor; positioned fail-closed errors; compile-time symbol
  resolution and depth guard, zero-alloc eval). Testkit enforces the
  empty-glob and provenance rules. Pinned handshake: d4 base_hit
  8,573.0184 via golden fixture, mutation-proven.
- 2026-07-21 — P2 complete: GameDef/BuildState/Scenario serde tiers; Plan
  compilation (unified slot layout [stats|buckets|stages|event_factors],
  staged symbol scoping, flat-namespace + MAX_EVENTS=8 fail-closed checks);
  evaluate() — three fold kinds, 2ⁿ event-branch EV with branch-recomputed
  buckets, condition-uptime scaling (missing uptime = 0, fail-closed),
  normalized phase blending, scratch-buffer zero-alloc hot path. Toy game
  gate: two playbooks pinned end-to-end from JSON (arena 282.15 / dummy
  374.34375), mutation-proven on both the build and scenario axes.
