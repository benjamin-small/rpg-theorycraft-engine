# P6 — Sequencing: the Timeline Simulator (2026-07-22)

**North star:** the third leg of the original charter — the *emulation
sequence* as configuration. One config family, three fidelity levels, all
answering "what is this build worth?":

| | What | Uptimes | Cost | Status |
|---|---|---|---|---|
| `Plan::evaluate` | closed-form average (Level 1) | inputs (asserted) | ~µs | shipped |
| `sim::run` EV mode | deterministic timeline | **computed** | ~ms | P6 |
| `sim::run` MC mode | sampled timelines ×N | computed + distributions | ~ms×N | P6 |

Decisions locked with the user (2026-07-22):
- **Full timeline simulation** (not rotation-feasibility-only).
- **Execution mode is configurable per call**: the same sequence config runs
  as an expected-value timeline or as seeded Monte Carlo (`iterations`,
  `seed` parameters). One semantic, two executors sharing one stepper.
- **Rotations are priority lists** (SimC-style), pure config, conditions in
  the existing expression language.
- **v1 mechanics**: cooldowns + cast times (skeleton), resource
  (gen/spend/cap), buff/debuff windows (timed contribution/condition
  application — turns guessed uptimes into measurements), procs/lucky-hit
  (chance + internal cooldown). **Multi-target deferred** (packs stay
  approximated by target-profile stats).
- **Approach A — discrete-event simulator** over B (fixed ticks: aliasing,
  wasted work) and C (analytic steady-state: cannot express fight-length
  effects or phases).

## Config surface — two NEW artifacts, nothing existing changes

### SimDef (game-domain, sits beside the GameDef, references its pipeline)

```jsonc
{
  "resources": {
    "mana": { "max": "max_mana", "regen_per_sec": "mana_regen" }   // exprs over stats
  },
  "actions": {
    "fireball": {
      "cast_time": "1.0 / base_aps",   // expr over STATS/conditions/sim-state (never pipeline stages); "0" = instant
      "cooldown": 0.0,
      "cost": { "mana": 40.0 },
      "gain": {},                           // e.g. generators: {"mana": 12}
      "damage": {                           // omit for utility casts
        "stats": { "coeff_pct": 200.0, "hits_per_use": 1.0 }  // per-cast overrides onto the Plan
      }
    }
  },
  "buffs": {
    "vuln_window": { "duration": 4.0, "conditions": { "vulnerable": 1.0 } },
    "combustion":  { "duration": 8.0,
                     "contributions": [{ "bucket": "indep", "value": 25.0 }] },
    "burning":     { "duration": 6.0, "tick_objective": "dot_dps" }  // DoT: objective accrues per active second
  },
  "procs": {
    "conflagrate": { "trigger": "on_crit",              // on_cast | on_hit | on_crit
                     "chance": "lucky_hit_chance / 100 * 0.3",
                     "icd": 2.0,
                     "apply_buff": "combustion" }        // or "cast_action": "<action>"
  },
  "damage_objective": "hit_after_dr"   // the per-hit objective the sim accumulates
}
```

The sim OWNS time: the pipeline's own APS/DPS stages are unused on the sim
path (`damage_objective` names the per-hit value; `hits_per_use` comes from
the action's overrides).

### Rotation (candidate-domain — drivers may search over rotations like gear)

```jsonc
{ "rules": [
  { "action": "frost_nova", "when": "cooldown.frost_nova == 0 and buff.vuln_window == 0" },
  { "action": "fireball",   "when": "mana >= 40" },
  { "action": "basic_bolt" }                         // no `when` = always willing
]}
```

First eligible rule wins. The engine enforces HARD gates automatically
(off cooldown, cost payable, not mid-cast); `when` adds strategy on top.
No rule eligible → advance time to the next event (waiting is modeled).

## Expression language v2 — predicates (prerequisite)

Rule/SimDef conditions need comparisons. The language grows:
`> < >= <= == !=` (returning 0/1) and `and(a,b)` / `or(a,b)` / `not(a)`
functions — available EVERYWHERE (gamedefs may use them too). Truthiness:
nonzero. Positioned fail-closed errors as always.

Sim expressions compile against an extended symbol space: all GameDef
stats + conditions, plus `time`, `duration`, each resource by name,
`cooldown.<action>` (seconds remaining), `buff.<buff>` (0/1),
`buff_remaining.<buff>`, `casts.<action>`. Dotted names already lex.

## Executor — one stepper, one substitution point

Discrete-event queue: CastComplete, BuffExpire, CooldownReady, ProcIcdClear,
PhaseBoundary, End. At each decision point: walk rules → begin the first
eligible cast (pay cost, lock for cast_time). On completion: apply gains,
fold the EFFECTIVE build (base BuildState + active buffs' contributions +
the action's stat overrides + buff-driven condition values), evaluate the
Plan, accumulate `damage_objective × hits`.

- **EV mode**: branch-blended evaluation exactly as `evaluate` does today.
  Procs use the deterministic ACCUMULATOR method: each qualifying hit adds
  its chance; the proc fires when the accumulator crosses 1 (respecting
  ICD). Reproducible; documented as an approximation of the MC mean.
- **MC mode**: a new `Plan::evaluate_sampled(…, rng)` picks ONE mask per
  branched stage by the branch chances (same single internal code path as
  `explain` — parameterized, never forked); procs roll exactly; sampled
  crits feed `on_crit` naturally. RNG: a tiny in-crate seeded PCG (the
  zero-dependency rule holds). `iterations` + `seed` parameterize the run.

Buff set changes re-fold contributions event-driven (not per-tick).

## Scenarios — same schema, Level-2 reading

Phase `weight` is read as SECONDS; boundaries swap phase stat overrides
mid-sim; total duration = Σ weights. Condition precedence: an active buff
that drives a condition WINS while active; otherwise the scenario's static
uptime applies — assumed and computed uptimes can honestly coexist during
migration.

## Output — `SimReport` (serde)

Per phase and total: duration, total_damage, dps; per-action casts /
damage / share; COMPUTED uptimes per buff and condition; resource stats
(time capped, time starved); proc fire counts. MC wraps it: N iterations →
mean/std/p10/p50/p90 of dps, seed-deterministic. v1 API is single-candidate
`sim::run(plan, sim_plan, rotation, build, scenario, mode, …) -> SimReport`;
batch/search integration is a later phase by design (the driver loops).

## Testing (house style: hand-worked, pinned, mutation-checked)

- **Keystone cross-check**: a degenerate config (one spammable action, no
  resource pressure, no procs) must reproduce Level-1 `total_dps` EXACTLY —
  where the fidelity levels overlap they are required to agree.
- Hand-worked pins: resource starvation cadence (generator/spender cycle),
  a buff cycle's computed uptime, the proc accumulator's fire times,
  a two-phase boundary swap.
- MC: same-seed determinism (identical SimReport); MC-vs-EV convergence on
  a crit-only case (N=10k mean within statistical tolerance of EV, fixed
  seed).
- Toy game first; then a D4 slice (Fireball / Frost Nova / mana) as the
  runnable example with pins.

## Phasing (each phase its own plan; small verified slices)

- **P6a** — expression predicates (comparisons + and/or/not), TDD.
- **P6b** — SimDef/Rotation schemas + compilation (symbol space, fail-closed
  reference checks, reserved names).
- **P6c** — EV executor: skeleton (queue, casts, cooldowns) + resource +
  buffs; keystone cross-check + starvation/uptime pins.
- **P6d** — procs (accumulator) + `evaluate_sampled` + MC executor +
  report statistics; determinism + convergence tests.
- **P6e** — D4 sim slice example (`diablo4_rotation`), docs, README section
  (scoped honestly as a slice), release 0.2.0.

## Out of scope (v1)

- Multi-target/AoE (deferred; pack playbooks stay target-profile
  approximations).
- Batch sim pricing inside `search::price` (later phase; drivers loop).
- Movement/mechanics scripting beyond phase boundaries.
- Gear/paragon interactions beyond what contributions already express.
