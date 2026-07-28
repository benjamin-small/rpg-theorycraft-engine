# P8 — Configurable Semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Checkbox steps. Spec: `docs/superpowers/specs/2026-07-26-p8-configurable-semantics-design.md` — read it first; its semantics are normative, including the standing invariant (**every serde default reproduces 0.3.0 byte-for-byte; ONE deliberate behavior change: one-world capture**).

**Goal:** the `defaults` block (`measure` / `proc_rolls` / `event_order`) with per-entity overrides, the effects list, the fail-closed config sweep, one-world measurement, coverage/API debt closed — released as rtce 0.4.0.

**Architecture:** six slices, dependency-ordered. P8a's unknown-key machinery leads because every later slice adds config fields that must get the same treatment. Each knob is a named enum whose default is a provable no-op (the byte-identical `diablo4_rotation` MC block is the standing proof for anything that could touch RNG or event order). Everything TDD red-first; every pinned number hand-worked in a comment; every behavior change mutation-proven (the P7 record — a surviving mutation on documented-but-unpinned semantics in five consecutive tasks — is the named risk); clippy `-D warnings` + fmt + `missing_docs` + `publish --dry-run` clean per task; both consumers re-verified per task (diablo4-calc `8096.023984663315`; poe2-calcs 63 `rtce_parity`). Every task commits.

**Tech stack:** pure Rust, zero deps beyond serde/serde_json. Consumers by path: `../diablo4-calc`, `../poe2-calcs` (workspace at `engine/`).

---

### Task 1 (P8a): fail-closed config sweep + validation debt

**Files:** new `crates/rtce/src/config_keys.rs` (shared unknown-key walk + did-you-mean); modify `crates/rtce/src/{gamedef,build,scenario,simdef}.rs` (unknown-key collection on every config struct), `crates/rtce/src/plan.rs` + `crates/rtce/src/sim/compile.rs` (validation calls; finiteness checks), `crates/rtce/src/simdef.rs` (hand-written `Deserialize` for `NumOrExpr` and the `TickObjective` repr).

Mechanism: each config struct gains `#[serde(flatten)] extra: BTreeMap<String, serde_json::Value>` (skip-serializing-if-empty so round-trips are clean; `_`-prefixed keys DO re-serialize — annotations survive). `config_keys::reject_unknown(context: &str, known: &[&str], extra: &BTreeMap<..>) -> Result<(), PlanError>`: ignores keys starting with `_`; otherwise errors naming the key, its context, and the nearest known field by a hand-rolled edit-distance (≤2, ~20 lines, zero-dep). `Plan::compile` walks GameDef-side structs; `sim::compile` walks SimDef-side. `PartialEq`-bearing structs: `extra` participates; the structural default guards still hold (both sides empty).

- [ ] **Step 1 — RED tests** (each fails for the right reason before implementing):
  - `{"tick_objectiv": "dot_dps"}` on a buff → error containing `unknown field \`tick_objectiv\``, the buff name, and `did you mean \`tick_objective\``.
  - `_source`/`_scope` keys at GameDef top level AND on a nested struct still parse and compile (pin: the committed d4 + poe2 fixtures compile unchanged — they already carry `_` keys).
  - Typos on `ActionDamage`, `Rule`, `ProcDef`, `Phase` each rejected with their context named.
  - `"cooldown": true` (malformed `NumOrExpr`) → error containing `expected a number (literal) or a string (expression)` — NOT serde's "did not match any variant". Rewrite the existing brittle test that pins serde's message.
  - `{"objective": "dot_dps", "snapshots": true}` → error naming `snapshots` and listing `objective`, `snapshot` (hand-written map visitor replaces the `TickObjectiveObj` + `deny_unknown_fields` machinery; keep the bare-string arm and the serialize-back-to-bare-string canonicalization — those tests stay green).
  - **Finiteness**: `Contribution { value: f64::NAN }` fails closed in BOTH halves — via `BuildState.contributions` at `Plan` build resolution ("contribution value into bucket `boost` must be finite, got NaN") and via `BuffDef.contributions` at `sim::compile`. Repro from the 0.3.0 release review: today both return `Ok(NaN)`. Also: `BuildState.stats` values and `Phase.stats` override values must be finite (same silent-NaN class) — one test each.
- [ ] **Step 2:** run, confirm each fails right (the finiteness ones fail by NOT erroring). **Step 3:** implement. **Step 4:** full suite + both consumers green; committed fixtures unchanged. **Step 5:** mutation-check: remove the `_` exemption → the fixture-compile test goes red; break the edit-distance → the did-you-mean assertion goes red. **Step 6:** commit `"P8a: unknown config keys fail closed (did-you-mean, _ namespace); contribution/stat finiteness — NaN no longer Ok(NaN)"`.

### Task 2 (P8b): the effects list

**Files:** `crates/rtce/src/simdef.rs` (`EffectDef`, `effects` fields, sugar), `crates/rtce/src/sim/compile.rs` (desugar + 5 fail-closed checks), `crates/rtce/src/sim/exec.rs` (ordered execution; `ProcEffect` → `Vec<CompiledEffect>`).

```rust
/// One effect of an action completing or a proc firing. Externally tagged:
/// `{ "apply_buff": "shock" }` / `{ "cast_action": "comet" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDef {
    /// One application of the named buff (through its `on_reapply` policy).
    ApplyBuff(String),
    /// Free-cast the named action (gains + damage + its OWN ApplyBuff
    /// effects; no cost, no cooldown, no proc rolls — the P7d free-cast rules).
    CastAction(String),
}
```

`ProcDef.effects: Vec<EffectDef>` + `ActionDef.effects: Vec<EffectDef>`, both `#[serde(default)]`. Desugar at `sim::compile`: `ProcDef.apply_buff`/`cast_action` and `ActionDef.apply_buff` (deprecated in rustdoc, kept for 0.x) prepend into `effects`. Fail-closed: sugar + explicit `effects` on one entity ("ambiguous order — migrate the sugar into the list"); empty proc effects after desugar ("a proc must do something"); unknown buff/action names inside effects; **`CastAction` inside `ActionDef.effects`** ("an action free-casting an action reopens recursion (A→B→A); see ROADMAP" — positioned). Execution: list order, repeats apply twice; each effect's execution updates the slot tail before the next (P7b sequential sim-state, unchanged).

- [ ] RED tests: serde round-trip of the JSON above; a sugar config and its explicit-`effects` spelling compile to identical `SimPlan` effect lists (assert on the compiled representation); **order pin** — a proc firing once at t=1 (chance `"time == 1"`) with `effects: [{cast_action: "ping"}, {apply_buff: "timed"}]` where `ping` is a zero-damage utility and `timed.duration = "1 + casts.ping"`: cast-first → duration 2 → `timed` uptime **0.2** over 10s; reversed list → duration 1 → **0.1** (hand-worked: window `[1,3)` vs `[1,2)`; the mutation IS the reorder); two distinct `apply_buff` entries → both buffs active; all five compile errors. Existing suite green UNCHANGED (every existing config uses sugar — that is the compat proof); `diablo4_rotation` byte-identical incl. MC.
- [ ] Commit `"P8b: ordered effects list on actions and procs — old fields desugar, cast_action stays proc-only (0.2/0.1 order pin)"`.

### Task 3 (P8c): WorldSnapshot measurement + the one-world fix

**Files:** `crates/rtce/src/simdef.rs` (`SimDefaults { measure: Measure }` with `#[serde(default)]` everywhere, `SimDef.defaults`, `ActionDef.measure: Option<Measure>`; `Measure::{CastComplete, CastStart}` snake_case, default `CastComplete`), `crates/rtce/src/sim/compile.rs` (resolve per-action), `crates/rtce/src/sim/exec.rs` (`WorldSnapshot { build: BuildState, phase: Phase }`; capture in `begin_cast` under `CastStart`, in the completion transaction under `CastComplete`; carried on the in-flight cast).

Semantics (normative in the spec — restate in rustdoc): every `Plan` evaluation in one cast's completion transaction reads the one snapshot — damage overlay (its `NumOrExpr` stats evaluate AT the measured instant; under `cast_start`, `casts.<self>` therefore does NOT include the in-flight cast), `hits_per_use`, crit chance / EV `on_crit` weight, and every `ApplyBuff` effect's tick capture. **The one-world fix:** captures read the snapshot's PHASE as well as its build (today: frozen build × live phase). **Scope boundary:** sim-FIELD expressions (`duration`, `cost`, `gain`) keep their P7b instants and live sim-tail reads — the 0.75 pandemic pin and every P7b pin must stay byte-identical.

- [ ] RED tests:
  - **Instant pin**: action `cast_time 1`, `damage.stats: { "dmg": "10 * time" }`, 5 casts completing t=1..5. `cast_complete` (default): total `10×(1+2+3+4+5) = 150`. `measure: "cast_start"`: starts t=0..4 → `10×(0+1+2+3+4) = 100`. Pin both; the mutation is evaluating at the other instant.
  - **Per-action override**: two actions in one rotation, one overridden `cast_start`, one default — both pins hold in a single run.
  - **The one-world re-pin**: `a_same_list_snapshot_capture_reads_a_frozen_build_but_a_live_phase` is REWRITTEN (rename to `..._reads_one_frozen_world`): both orderings of `["mark","poison"]` now capture the pre-list world → both equal the old poison-first value (**400**); assert the equality AND the literal. Mutation: restore the live-phase read → mark-first returns to 800.
  - **Teaching contrast** in `examples/poe2_triggers.rs`: at the integer shock duration (2.0) where 0.3.0 pinned bolt damage 1837.5, add a `measure: "cast_start"` run → **2175** restored (hand-worked in the plan: bolt N≥2 starts while the previous completion's window `[c, c+2)` is live; bolt 1 starts unbuffed → `150 + 9×225`). Update the `sim` docs' footgun section: "and here is the config that fixes it".
  - P7b regression net: the full `expr_fields` module + 0.75 pandemic pin byte-identical; `diablo4_rotation` EV + MC byte-identical (default path untouched).
- [ ] CHANGELOG (Unreleased): the one-world entry under a "behavior fix" heading with the migration note (moves numbers ONLY for a condition-driving buff + snapshot DoT applied in one list; neither consumer qualifies — verify and state).
- [ ] Commit `"P8c: WorldSnapshot measurement (cast_start|cast_complete) — one world per cast; 400/800 becomes an equality pin"`.

### Task 4 (P8d): configurable event ordering

**Files:** `crates/rtce/src/simdef.rs` (`SimDefaults.event_order: EventOrder` — `Scheduled` default | `CompletionsFirst`, snake_case; SimDef-GLOBAL, no per-entity field), `crates/rtce/src/sim/compile.rs`, `crates/rtce/src/sim/exec.rs` (`QueueItem` ordering becomes `(time, class_rank, seq)`).

`class_rank(event, order) -> u8`: under `Scheduled`, constant `0` (ordering bit-identical to 0.3.0 — the MC block proves it); under `CompletionsFirst`, `CastComplete → 0`, `BuffExpire | PhaseBoundary | Wake → 1`; `seq` breaks all residual ties (seeded MC stays deterministic under every setting). Horizon-drain semantics unchanged (ordering decides which event at `t == duration` goes first, never whether it resolves).

- [ ] RED tests:
  - **Default is a no-op**: full suite green untouched; `diablo4_rotation` MC block byte-identical.
  - **Cast-grid fix, ordering flavor**: the P7e footgun fixture (buff duration an exact multiple of cast cadence, applied on completion) under `completions_first` — the completing cast now measures WITH the buff. Pin via the `poe2_triggers`-shaped exec fixture: shock-style 2.0 duration, `cast_complete` measure, `completions_first` → bolt damage **2175** (same number as Task 3's fix, different knob — pin both in one table-style test and state the symmetry).
  - **Zero-weight-phase flip**: `phases [main:10, epilogue:0]`, epilogue `dmg` override 250 (the 0.3.0 pin's fixture): `scheduled` → 900 / 250 / 1150 (existing pin, still green); `completions_first` → the t=10 cast resolves BEFORE the boundary → measured under `main` → **1000 / 0 / 1000**, 10 casts. Pin, stated as DESIGNED under this knob (the 0.3.0 comment called the old attribution incidental).
  - **MC determinism** under `completions_first`: same seed twice → byte-identical report.
  - Mutation: flip the rank table → both pins fail; make `Scheduled` non-constant → the byte-identity test fails.
- [ ] Commit `"P8d: event_order (scheduled|completions_first) — (time, class_rank, seq); zero-weight flip 1000/0 pinned as designed"`.

### Task 5 (P8e): configurable proc rolling

**Files:** `crates/rtce/src/simdef.rs` (`SimDefaults.proc_rolls: ProcRolls` — `PerCast` default | `PerHit`; `ProcDef.rolls: Option<ProcRolls>` — the override lives on the PROC), `crates/rtce/src/sim/compile.rs`, `crates/rtce/src/sim/exec.rs` (`roll_procs_ev` / `roll_procs_mc` loop the snapshot's hit count under `PerHit`).

Semantics: hit count from the measurement snapshot. EV `per_hit`: the accumulator is fed once per hit; multiple crossings can fire; the ICD gates between fires. MC `per_hit`: one Bernoulli per hit; ICD gate between draws. `on_crit` weight applies per hit. **ICD-at-one-instant rule (stated, then pinned):** all hits of one cast land at the same instant, so `icd > 0` caps fires at one per cast even under `per_hit`; `icd: 0` permits multiple.

- [ ] RED tests (fixture: action `hits_per_use 5`, cast_time 1, 20s → 20 casts; on_hit proc, `apply_buff` a counter buff or use `proc_counts`):
  - **EV fractional pin (the P7 vacuity lesson — NOT chance 1)**: chance `"0.2"`, `icd 0`: `per_cast` → acc +0.2/cast → fires at casts 5,10,15,20 = **4**; `per_hit` → acc +1.0/cast → **20**. Pin both; the mutation is the loop.
  - **MC draw-count**: chance 1, `icd 0`: `per_cast` → 20 fires / 20 draws; `per_hit` → **100** fires / 100 draws (assert fires; the draw count shows as a changed downstream sample — assert same-seed determinism in both settings).
  - **ICD cap**: chance 1, `icd 3.0`, `per_hit` → fires t=1,4,7,10,13,16,19 = **7**, EXACTLY equal to `per_cast` at the same settings (hand-worked: fire arms a 3s ICD; the next completion at ready-time fires again; the 5 hits per cast are simultaneous so the cap binds). Assert the equality — it IS the at-one-instant rule.
  - Default no-op: suite + MC block byte-identical.
- [ ] Commit `"P8e: proc_rolls (per_cast|per_hit) — 4/20 EV pin, ICD-at-one-instant 7==7"`.

### Task 6 (P8f): coverage debt, docs discipline, release 0.4.0

**Files:** `crates/rtce/src/sim/exec.rs` tests; new `crates/rtce/examples/poe2_ignite.rs`; DELETE `SimScratch` from `crates/rtce/src/sim/{exec,mod}.rs`; `crates/rtce/src/expr/compiler.rs` (MAX_STACK doc); new or extended `CLAUDE.md` at repo root; `CHANGELOG.md`, `README.md`, `crates/rtce/README.md`, `crates/rtce/src/lib.rs`, `ROADMAP.md`; `crates/rtce/Cargo.toml` → 0.4.0; `.github/workflows/ci.yml` (+ ignite example).

- [ ] **Coverage debt**, RED-first with hand-worked pins: `refresh` + LIVE `tick_objective` (the 0.2.0 default DoT shape, zero coverage today) — utility cooldown 10 applying a duration-4 live-tick buff over 20s: active `[0,4)∪[10,14)` = 8s × rate R (derive R from the toy fixture) — pin total and uptime 0.4; live tick under Monte Carlo — same-seed determinism AND exact equality with EV (a live tick rate is branch-blended in both modes; the equality is the pin); a live-tick rate CHANGES when a mid-window buff refolds the world (discriminates live from snapshot in the refresh policy, closing the (policy × tick) cells the 0.3.0 review counted open — cover the cells, not one).
- [ ] **`poe2_ignite.rs`**: the `strongest` policy as a teaching slice (rising/falling phase override on the committed poe2 fixture, mirroring the P7c-T2 test shape): pins for strongest-wins, loser-discarded (expiry NOT refreshed), and the re-capture contrast vs `refresh`. CI-run. README retires the "no strongest example" caveat.
- [ ] **API debt**: remove `SimScratch` (grep consumers first — nothing uses it; CHANGELOG breaking note); `expr::MAX_STACK` reachability documented honestly on `Program::max_depth`.
- [ ] **Docs discipline codified**: repo-root `CLAUDE.md` gains the three binding rules (field docs = default + instant + interactions; numeric doc claims carry contrast-run pins; every shipped `(default × override)` cell gets a discriminating test) alongside the standing gates (workspace test, clippy `-D warnings`, fmt, `missing_docs`, both consumers, byte-identical example MC blocks under defaults). Audit THIS phase against all three rules before closing.
- [ ] **Release**: CHANGELOG 0.4.0 cut with "Upgrading from 0.3.0" (one-world behavior fix; unknown-key rejection — typo'd configs that parsed now fail, with the did-you-mean remedy; `SimScratch` removal; Rust source-breaking notes for new fields incl. `extra` flattening and `#[non_exhaustive]` additions if any). README + crate README + lib.rs updated TOGETHER (the P7e-T3 lesson: the crates.io front page is a deliverable). Version 0.4.0; `cargo publish --dry-run` clean; `cargo doc` zero warnings. Do NOT publish in this task.
- [ ] Commit `"P8f: refresh+live-DoT covered, poe2_ignite ships strongest, SimScratch removed — 0.4.0 staged (dry-run clean)"`.

**Final gate (coordinator):** standing-reviewer round on the whole P8 diff (spec + quality per task has already run; this is the whole-phase pass incl. a doc-truth sweep and a surviving-mutation hunt) → fixes → APPROVED → publish rtce 0.4.0 → push → consumer lockfile commits.
