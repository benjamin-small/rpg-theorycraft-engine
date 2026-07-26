# P7 — PoE2 Test Bed + Instance Mechanics (2026-07-25)

**North star:** poe2-calcs becomes rtce's second consumer, and the port is the
live test bed for the engine's next capability tier — counted, snapshotted,
instanced state. Released as **rtce 0.3.0**.

Decisions locked with the user (2026-07-25):
- **Combined arc**: engine gaps + poe2 port in one phase; the port is the test
  bed. Each engine feature is proven twice — toy-config TDD in rtce, then a
  hand-verified PoE2 slice on the generated poe2 gamedef.
- **Gap scope**: (1) stacks + snapshot DoTs, (2) expression-valued sim fields,
  (3) action-scoped effects. **Deferred**: damage-roll distributions /
  hit-size-dependent mitigation (armour, stun, freeze buildup), multi-target —
  next phase's charter.
- **Port depth**: parity harness only (D4's P4 stage). poe2-calcs gains an
  rtce path dep + generated gamedef + adapter + digit-exact parity pins;
  **native math stays**. Switchover is a later, separate decision.
- **Stack model — Approach B, unified instances**: every buff is internally an
  instance list; config policies collapse it per mechanic. Binary buffs are
  the degenerate case. (Rejected: A — separate stack counter + separate
  ailment list, two overlapping concepts forever; C — charges as bounded
  resources, which can't expire or carry contributions.)

Ground truth for the gap analysis: the 2026-07-25 session's two-catalog
comparison (poe2-calcs mechanics catalog vs rtce 0.2.0 capability inventory).
Standing invariant: **all existing pins stay green through every commit** —
keystone 282.15, d4 rotation 225199.1088 / 3753.31848 / 0.4, diablo4-calc's
7-build parity suite (8,096.02 …), poe2-calcs' native 124.53 / 129.51.

## P7a — poe2 parity harness (Level-1, native kept)

Lives in `../poe2-calcs` (mirrors diablo4-calc's P4 layout):

- **`emit-gamedef` bin** generates `gamedef/poe2.gamedef.json` — committed
  output + drift-guard test (their existing `emit-vocab` pattern). Generated,
  not hand-written: 5 damage-type chains make it ~10× d4's gamedef.
  Contents: per-type increased (`summed_group`) and more (`product`) buckets,
  global/attack-vs-spell/projectile/elemental/DoT buckets, conversion's
  "both buckets" rule as explicit pipeline stages, gain-as-extra, crit
  closed-form (`1 + c·(m−1)`), per-type resist − pen − exposure mitigation
  floored at 0, shock, cycle-time strike weighting, skill DoT + ignite
  stream; minion path and defence/EHP readouts as additional objectives.
- **`rtce_adapter`** maps `Build` (52 StatIds × Added/Increased/More) →
  `BuildState` — the mirror of diablo4-calc's adapter. Resolver untouched:
  rtce never sees gems, tree, or items.
- **Parity gate, digit-exact**: 124.53 (default Monk), 129.51 (node 10364),
  793.76 (Fireball +2 proj), plus the calc.rs unit-test cases (conversion,
  gain-as-extra, crit/aps, shock, ignite, minion, resist/pen/exposure,
  defence/EHP) replayed through rtce. Full `evaluate` output map is the
  target; if the minion/defence objectives prove gnarly they may land as a
  second slice within P7a, but the phase does not close without them.
- Native math is NOT deleted, no shim, no WASM changes.

## P7b — expression-valued sim fields

`BuffDef.duration`, `ActionDef.cooldown`, `cost`/`gain` amounts, and
`ActionDamage.stats` values accept expressions. Serde: untagged
number-or-string — **every existing config parses unchanged**.

Evaluation instants are fixed and documented:

| Field | Evaluated | Notes |
|---|---|---|
| `duration` | at application | snapshotted per instance |
| `cooldown` | at cast start | |
| `cost` | at cast start | must be payable at the evaluated value |
| `gain` | at cast complete | |
| `damage.stats` values | at cast complete | overlaid onto the Plan build |

All compile at sim-compile against the sim symbol space; non-finite or (where
required) negative results at the evaluation instant are positioned run
errors, fail-closed. **BuildState contribution values stay literal `f64`** —
gear-static formulas are the resolver's job, and "X per charge" is covered by
per-stack contributions (P7c).

## P7c — the instance runtime (the core)

`BuffRt` becomes a per-buff **instance list**; instances carry
`applied_at / expire_at / generation / snapshot_rate?`.

Config surface on `BuffDef`:
- `max_stacks: u32` — default 1; `0` = unbounded (poison).
- `on_reapply`: `refresh` | `add_refresh_all` | `add_independent` | `strongest`
  - `refresh` — today's binary buff: one instance, duration resets. The
    degenerate case; the D4 rotation pins guard it byte-for-byte.
  - `add_refresh_all` — PoE2 charges: count +1 up to `max_stacks`, ALL
    instances' expiry reset to now + duration (shared clock).
  - `add_independent` — PoE2 poison: new instance with its own duration; at
    `max_stacks`, the earliest-expiring instance is evicted.
  - `strongest` — PoE2 ignite: new instance replaces the incumbent only if
    its snapshot rate is higher; compile error unless the buff has a
    `snapshot: true` tick_objective (strongest needs a magnitude to compare).
- `tick_objective` gains `snapshot: bool` (default false):
  - `false` — current behavior: rate re-evaluated on every state change,
    piecewise-constant live (× stack count).
  - `true` — each instance captures its rate at application and ticks it
    unchanged to expiry: PoE2 ailment semantics. Instances tick
    independently; total DoT = Σ instance snapshot rates.

Semantics:
- Contributions fold **× active stack count** while ≥1 instance active.
- Conditions driven while ≥1 instance active (unchanged precedence rule).
- Symbol space: `stacks.<buff>` (current instance count) joins
  `buff.<buff>` (≥1 active) and `buff_remaining.<buff>` (**longest**
  remaining across instances).
- `SimReport` gains `avg_stacks` per buff (time-integrated mean).
- Stale-expiry events keep the generation-counter self-cancel pattern,
  extended per instance.

EV/MC agreement is a gate, same discipline as P6's proc fix: steady-state
stack counts and snapshot-DoT totals must converge between modes (EV builds
instances deterministically via the accumulator; MC rolls exactly).

## P7d — action-scoped effects

- `ActionDef.apply_buff: Vec<String>` — one application per listed buff on
  cast complete (feeds the buff's `on_reapply` policy like any application).
- `ProcDef.actions: Option<Vec<String>>` — trigger filter; `None` = all
  actions (today's behavior, unchanged).
- Fail-closed: unknown buff in `apply_buff`, unknown action in a proc filter.
- The d4 `diablo4_rotation` example is rewritten off the load-bearing
  icd==cooldown trick (ROADMAP debt closes; pins re-derived by hand if the
  cadence shifts — they shouldn't, but the mutation-check decides).

## P7e — three PoE2 sequencing slices + release 0.3.0

Engine-level examples/tests on the generated poe2 gamedef, each with
hand-worked pins in comments (house rule; simplify the SimDef until the
arithmetic is hand-derivable). Coefficients not datamine-sourced are labeled
`representative`, exactly like the d4 rotation slice.

1. **Frenzy-charge rotation** — `add_refresh_all`, `max_stacks: 3`,
   per-stack more-multiplier contribution; pinned average stacks + dps
   (exercises P7b expr duration + P7c stacks + `stacks.X` in a rotation
   `when`).
2. **Poison build** — `add_independent`, unbounded, `snapshot: true`;
   pinned steady-state stack count (= application rate × duration) and dps
   (exercises snapshot instances; MC convergence asserted).
3. **Trigger setup** — action-scoped proc (`actions` filter) casting a
   secondary skill via `cast_action`, plus `apply_buff` on a primary
   (exercises P7d end-to-end).

Then: CHANGELOG 0.3.0 entry, ROADMAP/README updates (sequencing section
gains stacks/ailments, honestly scoped), rustdoc for every new public item,
`cargo publish --dry-run` clean. **Final gate**: standing-reviewer round on
the whole P7 diff → fixes → APPROVED → publish rtce 0.3.0 → push both repos.

## Testing discipline (unchanged house style)

TDD red-first everywhere; pinned numbers hand-worked in comments and
mutation-checked (break an input, watch the pin fail, restore); clippy
`-D warnings` + fmt + `missing_docs` stay clean; every task commits with its
number; fail-closed over guessed defaults, always with positioned errors.

## Out of scope (P7)

- Damage-roll distributions (min..max, lucky) and hit-size-dependent
  mitigation (armour DR, stun/freeze buildup) — next phase; poe2 armour/EHP
  stay readout-only exactly as poe2-calcs has them today.
- Multi-target / AoE / multi-actor (minions stay count × dps at Level-1).
- poe2-calcs switchover (native-math deletion, shim, WASM) — separate
  future decision, D4's P4c playbook applies when taken.
- Proc-chain recursion (proc-cast actions triggering further procs) — the
  guard stays; bounded recursion is a candidate for the damage-roll phase.
- Rotation/SimDef mutation in `search::Move` — driver-side loops remain the
  answer for now.
