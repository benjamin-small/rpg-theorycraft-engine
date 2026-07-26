# P7 — PoE2 Test Bed + Instance Mechanics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Checkbox steps. Spec: `docs/superpowers/specs/2026-07-25-p7-poe2-instances-design.md` — read it first; its semantics are normative. Two repos are involved: `rtce` (this repo) and `../poe2-calcs` (sibling; consumes rtce as a path dep exactly like `../diablo4-calc` does).

**Goal:** poe2-calcs becomes rtce's second consumer (Level-1 parity harness, native math kept), and rtce 0.3.0 ships the instance runtime (stacks + snapshot DoTs), expression-valued sim fields, and action-scoped effects — each proven by toy TDD in rtce and a hand-verified PoE2 slice.

**Architecture:** see spec. Unified instance model: every buff is internally an instance list; `on_reapply` policies (`refresh` | `add_refresh_all` | `add_independent` | `strongest`) collapse it per mechanic; binary buffs are the `max_stacks: 1, refresh` degenerate case guarded by the existing D4 pins. Everything TDD red-first; every pinned number hand-worked in a comment; clippy `-D warnings` + fmt + `missing_docs` stay clean; every task commits.

**Standing invariant (assert after EVERY task):** `cargo test --workspace` green in rtce, `../diablo4-calc` (8,096.02 suite), and — once Task 1 lands — `../poe2-calcs` (native 124.53 / 129.51 untouched).

---

### Task 1 (P7a-T1): poe2 gamedef emitter + adapter + hit/DoT parity

**Repo:** `../poe2-calcs`. **Files:** new `engine/core/src/bin/emit_gamedef.rs`, new `engine/core/src/rtce_adapter.rs`, new committed `gamedef/poe2.gamedef.json`, new `engine/core/tests/rtce_parity.rs`; `engine/core/Cargo.toml` gains `rtce = { path = "../../../rpg-theorycraft-engine/crates/rtce" }` (verify the relative depth against the actual workspace layout before committing); CLAUDE.md gains the sibling-clone build prerequisite note (copy diablo4-calc's wording).

**The gamedef is GENERATED, never hand-edited.** `emit_gamedef` builds the `rtce::gamedef::GameDef` in Rust (serde types) and writes pretty JSON to `gamedef/poe2.gamedef.json`. Drift guard: a test in `rtce_parity.rs` re-emits in-memory and `assert_eq!` against `include_str!` of the committed file.

**Modeling scheme** (mirror `calc.rs::evaluate` — it is the normative reference; read it fully first):
- **Stats:** one rtce stat per scalar input: weapon (`weapon_phys_min/max`, `weapon_crit`, `weapon_aps`), skill (`base_damage`, `damage_type` one-hot as 5 stats `skill_is_phys/fire/cold/light/chaos`, `is_spell`, `dot_base` + 5-way `dot_is_*`, `ignite_pct`), enemy (`enemy_res_phys/fire/cold/light/chaos`, `enemy_shock_pct`), conversion (`conv_pct` + 5-way target one-hot), `always_crit`, and **3 strike columns**: `strike{1,2,3}_eff_pct`, `strike{1,2,3}_hits`, `strike{1,2,3}_speed_pct` (unused strike ⇒ hits = 0; its time term is gated by `(strike_i_hits > 0)` so it contributes 0 to cycle time — the closed expression grammar has comparisons for exactly this).
- **Buckets:** per relevant StatId, `inc_<stat>` = `summed_group` (Increased: `1+Σ/100`) and `more_<stat>` = `product` (More: each `(1+v/100)`); `added_<stat>` = `sum` for flat Added (AddedXToAttacks/Spells, CritDamageBonus, penetration, exposure, GainAsExtra, AdditionalProjectiles). Emit ONLY the buckets `calc.rs` actually reads (52 StatIds ≠ 52×3 buckets — YAGNI).
- **Pipeline stages**, in `calc.rs` order: per-type base (spell vs attack via `is_spell` gating, `damage_eff_pct`), gain-as-extra (`base_total × added_gain_x/100`), conversion with the **both-buckets rule** (`converted × (1 + inc_phys + inc_target + global_inc) × more_phys × more_target × global_more`; `global_inc = inc_damage + is_spell·inc_spell + (1−is_spell)·inc_attack + is_proj·inc_proj`), per-type native scaling (+ elemental bucket for fire/cold/light), crit closed-form (`eff_crit = always_crit>0 ? 1 : clamp(base_crit·(1+inc_crit)/100,0,1)`; `crit_mult = 1 + eff_crit·((150+added_critdmg)/100 − 1)`), per-type mitigation (`max(0, 1−(res−pen−expo)/100)`) composition-weighted, shock (`1+max(0,shock)/100`), strike-weighted `sustained_dps = Σ_i hit_i·hits_i / Σ_i time_i`, DoT chain (`dot_scale`, ignite = `ignite_pct/100 × fire_total × dot factors × fire_mitigation`), `total_dps = sustained_dps + dot_dps`. Objectives: `total_dps`, `sustained_dps`, `dot_dps`, per-type hit stages (for unit-case parity).
- **Adapter** (`rtce_adapter.rs`): `to_build_state(&Build) -> rtce::build::BuildState` — walk `build.mods` + allocated-node mods; `(StatId, Added)` → flat stat or `added_<stat>` contribution, `(StatId, Increased)` → `inc_<stat>` contribution, `(StatId, More)` → `more_<stat>` contribution; skill/weapon/enemy scalars → stats. NO math in the adapter beyond routing (the D4 rule). `@enemy_stunned`-style conditions map to rtce conditions with uptime 1/0 from `config.conditions`.

- [ ] **Step 1 — RED:** write `rtce_parity.rs` with (a) the drift-guard test (fails: no gamedef file yet), (b) parity tests: resolve the default Monk config → native `evaluate` → rtce `plan.evaluate(to_build_state(..), ..)`; assert `|native.total_dps − rtce| / native < 1e-9`. Cases: default Monk (native ≈ 124.53), Monk + node `10364` (≈ 129.51), Fireball +2 projectiles (≈ 793.76), plus replays of the calc.rs unit cases for conversion, gain-as-extra, crit/aps, shock, ignite, resist/pen/exposure (transcribe each unit case's Build inputs; assert the same stage/objective to 1e-9 relative).
- [ ] **Step 2:** run → confirm every test fails for the right reason (missing file / unresolved symbols).
- [ ] **Step 3:** implement emitter + adapter; run `cargo run -p engine-core --bin emit_gamedef` (adjust `-p` to the actual crate name); iterate to green. Where native and rtce disagree, the NATIVE number wins — fix the gamedef/adapter, never the assertion.
- [ ] **Step 4:** full poe2-calcs suite green (native tests untouched); rtce + diablo4-calc suites still green.
- [ ] **Step 5:** commit (poe2-calcs) `"P7a-T1: rtce parity harness — generated poe2 gamedef, hit/DoT path to 1e-9 (124.53 / 129.51 / 793.76)"`.

### Task 2 (P7a-T2): minion + defence/EHP objectives

**Repo:** `../poe2-calcs`. **Files:** `emit_gamedef.rs`, `rtce_adapter.rs`, `rtce_parity.rs`, regenerate `gamedef/poe2.gamedef.json`.

Extend the gamedef with the minion chain (`minion_dps` objective: minion base × minion count from `free + floor(spirit/spirit_per_minion)` × minion inc/more buckets — mirror `calc.rs::minion` path) and defence readouts as objectives: `life`, `es`, `pool`, capped per-type res (`min(res, max_res_cap)` chain incl. `MaxResistance` raise, hard cap 90), `armour_dr = armour/(armour + 10·ref_hit)` capped 0.90 (`ref_hit` is a stat), `ehp_<type> = pool/(1−capped_res/100)`, `evade` formula. These are readout objectives — they do NOT feed `total_dps` (matching native).

- [ ] RED: parity replays of the minion and defence/EHP unit cases from `calc.rs` (same 1e-9 rule) → confirm fail → implement → green (all three repos) → commit (poe2-calcs) `"P7a-T2: minion + defence/EHP objectives — full evaluate surface at parity"`.

### Task 3 (P7b): expression-valued sim fields

**Repo:** rtce. **Files:** `crates/rtce/src/simdef.rs`, `crates/rtce/src/sim/compile.rs`, `crates/rtce/src/sim/exec.rs`.

New serde type in `simdef.rs`:
```rust
/// A literal number or an expression string, evaluated at a documented instant.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum NumOrExpr {
    /// Literal value (backward compatible with 0.2.0 configs).
    Num(f64),
    /// Expression over the sim symbol space.
    Expr(String),
}
```
Adopt it for: `BuffDef.duration`, `ActionDef.cooldown`, `ActionDef.cost` / `gain` map values, `ActionDamage.stats` map values. Compile: `Num` becomes a pre-baked constant; `Expr` compiles against the sim symbol space at `sim::compile` (positioned errors as always). Evaluation instants (spec table, restate in rustdoc): duration → at application (snapshotted per instance); cooldown + cost → at cast start; gain + damage.stats → at cast complete. Fail-closed at the instant: non-finite ⇒ `PlanError`; duration/cooldown/cost/gain additionally reject negative. Resource-affordability wake times use the cost evaluated at the decision instant (document: an expr cost is re-checked at each decision, not predicted).

- [ ] **Step 1 — RED tests** in `exec.rs`'s test module (toy-game base, same harness as the P6c tests):
  - Backward compat: the P6 spec's exact numeric SimDef JSON still parses and the P6c starvation pin (casts t=0,1,5,10,15; 5 casts/20s) reproduces identically.
  - Expr duration: buff duration `"2 + bonus_dur"` with stat `bonus_dur = 2` → 4s windows; then a two-phase scenario where phase 2 overrides `bonus_dur = 6`: an application at t=1 (phase 1) expires at t=5 even though the boundary at t=4 raised the stat — duration snapshots at application. Hand-work the uptime in a comment.
  - Expr cost: cost `"20 + 10"` behaves exactly as literal 30 (same starvation cadence re-derived).
  - Fail-closed: duration `"0 - 5"` → `PlanError` naming the buff at application time; cost referencing an unknown symbol → compile error.
- [ ] **Step 2:** confirm each fails for the right reason. **Step 3:** implement. **Step 4:** rtce + both consumer suites green. **Step 5:** commit `"P7b: expression-valued sim fields — NumOrExpr, documented evaluation instants, 0.2.0 configs parse unchanged"`.

### Task 4 (P7c-T1): instance runtime — stacks and reapply policies

**Repo:** rtce. **Files:** `crates/rtce/src/simdef.rs`, `sim/compile.rs`, `sim/exec.rs`, `sim/mod.rs` (symbol docs), `sim/report.rs`.

`BuffDef` gains `max_stacks: u32` (serde default 1; `0` = unbounded) and `on_reapply: ReapplyPolicy` (serde default `Refresh`; snake_case variants `refresh | add_refresh_all | add_independent | strongest`). `strongest` is deferred to Task 5 — in THIS task `sim::compile` rejects it with "strongest requires a snapshot tick_objective (see P7c-T2)" so the enum is complete but honest.

Runtime rewrite in `exec.rs`:
```rust
struct BuffInstance { applied_at: f64, expire_at: f64, snapshot_rate: f64 } // snapshot_rate unused until Task 5
struct BuffRt { instances: Vec<BuffInstance>, generation: u64, /* existing uptime/tick integration fields */ }
```
One pending `BuffExpire{buff, generation}` per buff, scheduled at `min(expire_at)`. The invariant that matters: **at most one non-stale `BuffExpire` per buff on the heap, at `min(expire_at)`**. An APPLICATION bumps `generation` so the pending event self-cancels; the expiry handler sweeps every instance with `expire_at <= now` and reschedules at the new min under the SAME generation (it is itself the non-stale event, so it needs no bump). Policies on application: `refresh` — truncate to 1 instance, reset its expiry; `add_refresh_all` — push (up to `max_stacks`; at cap, no new instance) then set ALL expiries to `now + duration` (shared clock); `add_independent` — push own-duration instance; at cap evict the earliest-expiring. Semantics: contributions fold × `instances.len()`; conditions driven while len ≥ 1 (value NOT scaled by stacks — document); non-snapshot `tick_objective` rate × stack count; symbols `buff.X` = len ≥ 1, `buff_remaining.X` = max expire − now, NEW `stacks.X` = len (extend the resolver in `sim/compile.rs` + docs in `sim/mod.rs`). `SimReport` buffs gain `avg_stacks: f64` (time-integrated mean, same flush pattern as `buff_uptime`).

- [ ] **Step 1 — RED tests** (toy base; hand-worked arithmetic verbatim in comments):
  - Degenerate guard: every existing sim test green UNCHANGED before any new test is added (pure refactor first — commit checkpoint allowed).
  - `add_refresh_all` trajectory: filler cast_time 1 (always eligible, second rule) + generator action cast_time 0 cooldown 2 (first rule) applying buff `charge`. Action-scoped application does not exist yet (Task 6), so use the icd==cooldown trick ONE last time: proc `charge_gen`, trigger `on_cast`, chance `"1"`, `icd: 2.0`, `apply_buff: "charge"` — with the generator on a 2s cooldown the icd admits exactly one application per generator cast and gates out the filler's casts in between. Add the comment `// icd==cooldown trick; Task 6 replaces this with ActionDef.apply_buff`. Cadence: applications t=0,2,4,…,18; duration 5, max_stacks 3 → stacks 1@[0,2), 2@[2,4), 3@[4,20] (refreshed before every expiry). `avg_stacks = (1·2 + 2·2 + 3·16)/20 = 2.7`; while 3 stacks, a per-stack contribution of +10 in a product bucket moves the filler's per-hit by ×(1+30/100) vs ×(1+10/100) at 1 stack — pin both per-hit values from toy defaults.
  - Shared-clock expiry: same setup but generator stops after t=4 (cooldown 100): all 3 instances expire together at 4+5=9 → stacks 3→0 at t=9, `buff_uptime = 9/20 = 0.45`, `avg_stacks = (1·2 + 2·2 + 3·5)/20 = 1.05`.
  - `add_independent` cap eviction: max_stacks 2, duration 4, applications t=0,1,2: at t=2 the t=0 instance (expiring 4) is evicted for the new one → stacks stay 2, earliest expiry now 5. Pin the stack trajectory and uptime.
  - `stacks.X` in a rotation `when`: a rule `when: "stacks.charge >= 3"` only fires from t=4 — pin first-cast time.
- [ ] **Step 2:** fail-for-right-reason. **Step 3:** implement (refactor first, then policies). **Step 4:** rtce + diablo4-calc + poe2-calcs suites green; d4 rotation pins byte-identical. **Step 5:** commit `"P7c-T1: instance runtime — stacks, refresh/add_refresh_all/add_independent, avg_stacks 2.7/1.05 pinned"`.

### Task 5 (P7c-T2): snapshot DoTs + strongest + EV/MC agreement

**Repo:** rtce. **Files:** `simdef.rs` (tick_objective shape), `sim/compile.rs`, `sim/exec.rs`.

`BuffDef.tick_objective` becomes an object while keeping the old form parsing: accept `"name"` (live, today's semantics) or `{ "objective": "name", "snapshot": true }` (serde untagged helper). Snapshot semantics: at each application, evaluate the objective against the CURRENT effective state and store it in that instance's `snapshot_rate`; the instance ticks that rate unchanged to expiry; buff total tick rate = Σ instance `snapshot_rate` (stack count is inherent in the sum — do NOT also multiply by len); live buffs keep rate × len from Task 4. `strongest` policy unlocked: compile requires `snapshot: true` AND `max_stacks == 1`; on application, evaluate the incoming snapshot rate and replace the incumbent only if strictly higher (loser fully discarded; expiry NOT refreshed on a losing application — document).

- [ ] **Step 1 — RED tests** (hand-worked comments):
  - Poison cadence: filler cast_time 1, chance-1 icd-0 on_cast proc applying `poison` (`add_independent`, max_stacks 0, duration 4, snapshot tick at rate R = a hand-derived toy per-hit stage value; zero the filler's direct damage so dot is the whole objective). 20s phase, applications t=0..19: instances applied t≤16 tick a full 4s (17·4=68), t=17,18,19 tick 3+2+1=6 → total `74·R` damage; `avg_stacks = 74/20 = 3.7`. Pin total damage, dps, avg_stacks exactly.
  - Snapshot immunity: a second buff activating at t=10 that doubles the tick objective — instances applied before t=10 keep rate R, after t=10 rate 2R. Hand-work the split total; the LIVE (non-snapshot) control variant of the same layout must instead retroactively tick 2R from t=10 for ALL active instances — pin both totals to prove the flag changes exactly this.
  - Strongest: two applications with different snapshot rates (stat raised between them via phase boundary): weaker-after-stronger is discarded (rate stays high, expiry unchanged); stronger-after-weaker replaces. Pin the tick totals both ways. Compile errors: `strongest` without snapshot; `strongest` with `max_stacks: 2`.
  - EV/MC agreement (the P6 discipline): poison proc at chance 0.5, N=2000 fixed seed — MC mean total damage within 3% of EV; same-seed determinism still byte-identical.
- [ ] **Steps 2–4:** fail-right → implement → all three suites green. **Step 5:** commit `"P7c-T2: snapshot DoTs (74R pinned) + strongest — EV/MC agree on instance totals"`.

### Task 6 (P7d): action-scoped effects

**Repo:** rtce. **Files:** `simdef.rs`, `sim/compile.rs`, `sim/exec.rs`, `crates/rtce/examples/diablo4_rotation.rs`.

`ActionDef` gains `apply_buff: Vec<String>` (serde default empty): on cast complete, one application per listed buff, routed through the buff's `on_reapply` policy exactly like a proc application. `ProcDef` gains `actions: Option<Vec<String>>` (default `None` = all actions, today's behavior): the proc's trigger only considers casts of listed actions. Fail-closed at `sim::compile`: unknown buff name in `apply_buff`; unknown action name in a proc filter; empty `actions: []` is an error (write `None` or omit). Then rewrite `diablo4_rotation.rs`: `frost_nova` gets `apply_buff: ["vuln_window"]` and the `nova_pulse` icd==cooldown proc is DELETED (the ROADMAP trap closes). The cadence is unchanged (nova still casts every 10s, buff still 4s) so the pins 225199.1088 / 3753.31848 / 0.4 must hold identically — if they don't, that's a bug in apply_buff, not a re-pin.

- [ ] RED: (a) apply_buff uptime pin — utility action cooldown 10 + `apply_buff` duration 4 → uptime 0.4 with NO proc defined; (b) proc filter — two damaging actions, proc `actions: ["a"]`, rotation alternating: `proc_counts` counts only `a`'s casts (pin the count); (c) the three compile errors. → fail-right → implement → example pins hold unchanged → all suites green → commit `"P7d: ActionDef.apply_buff + proc action filters — icd trick deleted, d4 pins unchanged"`.

### Task 7 (P7e-T1): three PoE2 sequencing slices

**Repo:** `../poe2-calcs`. **Files:** new `engine/core/tests/rtce_sequencing.rs` (uses the committed poe2 gamedef via `include_str!` + inline SimDef/Rotation JSON).

Three tests, each with the full hand-worked derivation in comments (house rule: SIMPLIFY the SimDef until the arithmetic is hand-derivable — zero regen, chance-1 procs, round numbers; label non-datamined coefficients `representative`):
1. **Frenzy charges:** attack skill filler + charge generator on cooldown 2; buff `frenzy` (`add_refresh_all`, max_stacks 3, duration expr `"4 + inc_charge_dur/25"` exercising P7b — pick stat values making it exactly 5), per-stack `more_damage` contribution. Reuse the Task-4 cadence: `avg_stacks = 2.7` over 20s; pin dps hand-derived from the poe2 gamedef's per-hit value at 1/2/3 stacks.
2. **Poison:** spammable attack, `apply_buff: ["poison"]` (P7d — no proc needed), poison `add_independent` unbounded, duration 4, snapshot tick on a per-hit poison objective (add a `poison_dps_per_stack`-style stage to the emitted gamedef if needed — regenerate + drift guard). Task-5 cadence: 74·R total over 20s, `avg_stacks 3.7`; pin total/dps with R hand-derived. Assert MC (N=1000, fixed seed) mean within 3% of EV.
3. **Trigger:** primary skill + a proc `actions: ["primary"]`, chance 1, icd 4, `cast_action: "secondary"`; pin secondary fire times (t=0,4,8,12,16 in 20s → 5 free casts) and the damage split per action.
- [ ] RED (pins computed by hand BEFORE running — write the comment first) → fail-right → implement/tune → green in all three repos → commit (poe2-calcs) `"P7e-T1: PoE2 sequencing slices — charges 2.7 stacks, poison 74R, trigger 5 casts, all hand-pinned"`.

### Task 8 (P7e-T2): docs + 0.3.0 (dry-run only)

**Repo:** rtce. **Files:** `CHANGELOG.md`, `ROADMAP.md`, `README.md`, `crates/rtce/Cargo.toml`, rustdoc on every new public item.

- [ ] CHANGELOG 0.3.0 entry (instance runtime, NumOrExpr fields, action-scoped effects, second consumer at parity); ROADMAP: close the apply_buff item, promote damage-rolls + multi-target as the next-phase charter, note poe2 switchover as a standing option; README: sequencing section gains a short stacks/ailments paragraph (honestly scoped — representative coefficients, parity harness not switchover) + mention the second consumer in Status. Version: rtce `0.3.0` (testkit unchanged unless touched). `cargo publish --dry-run -p rtce` clean. Do NOT publish — that is the coordinator's explicit final step after review.
- [ ] Gates + commit `"P7e-T2: docs + 0.3.0 — second consumer at parity, instance mechanics ship (dry-run clean)"`.

**Final gate (coordinator):** standing-reviewer round on the whole P7 diff (both repos) → fixes → APPROVED → publish rtce 0.3.0 → push rtce AND poe2-calcs.
