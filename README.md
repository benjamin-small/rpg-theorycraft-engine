# rpg-theorycraft-engine (rtce)

[![CI](https://github.com/benjamin-small/rpg-theorycraft-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/benjamin-small/rpg-theorycraft-engine/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/rtce.svg)](https://crates.io/crates/rtce)
[![docs.rs](https://docs.rs/rtce/badge.svg)](https://docs.rs/rtce)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

A generic, config-driven theorycrafting engine. The game's algorithm —
stats, fold rules, probabilistic events, damage pipeline — is
configuration, compiled once into a fast evaluation plan. Extracted from
the proven patterns of `diablo4-calc` and `poe2-calcs`.

- Design: `docs/superpowers/specs/2026-07-21-rtce-design.md`
- Test: `cargo test --workspace`

Crates: `rtce` (engine), `rtce-testkit` (fixture harness, dev-dependency).

## A real game's damage model in config: a Diablo 4 slice

To be clear about scope: this is a **thin slice** of Diablo 4 — the core
damage formula (crit/overpower branching, the shared additive pool, summed
multiplier groups, vulnerable, DoT, attack speed) — not the game. There is
no resource model, no proc system, no buff timeline in the `GameDef`
below; those live one tier up, in the `SimDef` covered under "Sequencing".
Defenses are a resolver-side concern in the consumer and are not modeled
at all. What the slice demonstrates is the *shape*: a real game's damage
algorithm expressed entirely as data, exact enough that a production
calculator runs on it.

Three tiers of configuration and nothing else answer the closed-form
question. **Tier 1 — the GameDef** is the slice's algorithm. This is the (abridged) real one from
[`crates/rtce/tests/fixtures/d4/gamedef.json`](crates/rtce/tests/fixtures/d4/gamedef.json),
the same file the test suite pins against the `diablo4-calc` production
calculator:

```jsonc
{
  "stats": ["weapon_avg", "coeff_pct", "mainstat", "mainstat_divisor",
             "crit_chance", "op_chance", "op_baseline",
             "base_aps", "hits_per_use", "enemy_dr", "dot_coeff_pct"],
  "conditions": ["vulnerable", "close"],
  "buckets": {
    "additive":   { "fold": "sum" },           // ONE shared +% pool
    "crit_group": { "fold": "summed_group" },  // x% mults SUM, then multiply
    "vuln_group": { "fold": "summed_group" },
    "indep":      { "fold": "product" },       // aspects: each its own factor
    "as_sum":     { "fold": "sum" }            // (dot/op/element buckets elided)
  },
  "events": {
    "crit":      { "chance": "crit_chance / 100", "factor": "1.5 * crit_group" },
    "overpower": { "chance": "op_chance / 100",   "factor": "op_baseline * op_group" }
  },
  "pipeline": [
    { "name": "base", "expr": "weapon_avg * coeff_pct / 100 * (1 + mainstat / mainstat_divisor)" },
    { "name": "vuln_factor", "expr": "1 + vulnerable * (1.2 * vuln_group - 1)" },
    { "name": "hit", "branched": true,
      "expr": "base * (1 + additive / 100) * event_factors * vuln_factor * indep" },
    { "name": "hit_after_dr", "expr": "hit * (1 - clamp(enemy_dr, 0, 100) / 100)" },
    { "name": "raw_aps", "expr": "base_aps * (1 + min(as_sum, 100) / 100)" },
    { "name": "total_dps", "expr": "hit_after_dr * hits_per_use * raw_aps" }
  ],
  "objectives": ["total_dps"]
}
```

The `branched` stage is where Diablo 4's 4-branch crit/overpower expected
value comes from: the engine enumerates every event combination, opens the
event-gated bucket members per branch, and blends by probability — no
special-case code, just the `events` block.

**Tier 2 — a BuildState**, one candidate character (weapon 1000, 200%
skill, 800 Int, 20% crit, and a spread of typical rolls):

```json
{
  "stats": { "weapon_avg": 1000, "coeff_pct": 200, "mainstat": 800,
             "mainstat_divisor": 800, "crit_chance": 20, "op_baseline": 1.5,
             "base_aps": 1.0, "hits_per_use": 1 },
  "contributions": [
    { "bucket": "additive",   "value": 30 },
    { "bucket": "additive",   "value": 25, "event": "crit" },
    { "bucket": "crit_group", "value": 20 },
    { "bucket": "vuln_group", "value": 20 },
    { "bucket": "indep",      "value": 15 },
    { "bucket": "as_sum",     "value": 20 }
  ]
}
```

**Tier 3 — Scenarios**, the fights being asked about:

```json
{ "phases": [ { "name": "dummy", "weight": 1,
    "uptimes": { "vulnerable": 1.0 }, "stats": { "enemy_dr": 25 } } ] }
```

```json
{ "phases": [ { "name": "boss", "weight": 1,
    "uptimes": { "vulnerable": 0.6 }, "stats": { "enemy_dr": 90 } } ] }
```

Run the whole set (the example carries its hand-worked pins, per house rule):

```
$ cargo run -p rtce --example diablo4_basics
Diablo 4 basics — one build, two playbooks
  training dummy (vuln 100%, 25% DR):    9526.6368 dps
  raid boss      (vuln  60%, 90% DR):    1114.9693 dps

  dummy branch table (stage `hit`):
    —            weight  0.80  event_factors  1.00  hit     8611.200
    crit         weight  0.20  event_factors  1.80  hit    18480.960
```

Same build, two playbooks, two truths — which is the point: an external
driver (an optimizer, a knowledge-graph explorer) calls
`search::price(plan, base, candidates, scenarios, …)` and collects the
Pareto front across fights. For the smallest-possible starting point, see
`cargo run -p rtce --example your_own_game`.

## Sequencing: from average to timeline

Everything above answers "what does this build average, given ASSERTED
uptimes?" — fast, but every buff window, resource squeeze, and proc has to
be flattened into a `Scenario`'s static numbers by hand. `SimDef` +
`Rotation` + `sim::run` answer a related question over an actual TIMELINE:
given a priority-list rotation, resources, cooldowns, buff windows, and
procs, what really happens over N seconds — and what uptimes does that
produce?

| | What | Uptimes | Cost |
|---|---|---|---|
| `Plan::evaluate` | closed-form average | inputs (asserted) | ~µs |
| `sim::run` EV mode | deterministic timeline | **computed** | ~ms |
| `sim::run` MC mode | sampled timelines ×N | computed + distributions | ~ms×N |

One config family, three fidelity levels, all built on the same `Plan` —
where they overlap they're required to agree (`sim::exec`'s keystone test
reproduces `Plan::evaluate`'s number EXACTLY on a degenerate config with
nothing for the timeline to add). EV mode and MC mode agree on procs too,
in BOTH regimes: an internal cooldown is a hard gate in EV mode exactly as
it is in MC mode, so the two modes' long-run fire rates converge whether a
proc's ICD never binds or routinely gates hits — EV's accumulator fires
deterministically at the expected interval, MC's rolls add sampling
variance around that same mean.

To be clear about scope, same as the D4 slice above: `examples/diablo4_rotation.rs`'s
`SimDef` is a DEMONSTRATION slice, not Diablo 4's real cadence data —
mana regen is zeroed and Firebolt's mana gain is set equal to Fireball's
cost purely so the cast sequence hand-verifies cleanly (see the pin
comments in the example itself). A production rotation would tune these
from real skill data the same way `diablo4_basics`'s `GameDef` slice was
transcribed from `diablo4-calc`.

**SimDef** (trimmed — the full version adds Firebolt; see the example):

```jsonc
{
  "resources": { "mana": { "max": "100", "regen_per_sec": "0" } },
  "actions": {
    "frost_nova": { "cast_time": "0", "cooldown": 10.0, "cost": {}, "gain": {},
                    "apply_buff": ["vuln_window"] },
    "fireball": {
      "cast_time": "1", "cooldown": 0.0,
      "cost": { "mana": 40.0 }, "gain": {},
      "damage": { "stats": { "coeff_pct": 200.0 } }
    }
  },
  "buffs": {
    "vuln_window": { "duration": 4.0, "conditions": { "vulnerable": 1.0 } }
  },
  "damage_objective": "hit_after_dr"
}
```

**Rotation** — priority list, first eligible rule wins (hard gates like
"off cooldown"/"cost payable" are automatic; `when` adds strategy on top):

```json
{ "rules": [
  { "action": "frost_nova" },
  { "action": "fireball", "when": "mana >= 40" },
  { "action": "firebolt" }
]}
```

Run it (EV mode's `SimReport`, then a 1000-iteration Monte Carlo run):

```
$ cargo run -p rtce --example diablo4_rotation
Diablo 4 rotation (P6 sequencing) — 60s training dummy, EV mode
  action        casts         damage    share
  fireball         31    187886.4480   83.43%
  firebolt         29     37312.6608   16.57%
  frost_nova        6         0.0000    0.00%
  total: 225199.1088 damage over 60s = 3753.31848 dps
  vuln_window buff uptime: 0.4000   vulnerable condition uptime: 0.4000
  mana: 0.0000s starved, 0.0000s capped

  EV pins hold: 225199.1088 total / 3753.31848 dps / 0.4 vuln uptime ✓

Diablo 4 rotation — 60s training dummy, Monte Carlo mode (N=1000, seed=42)
  mean 3746.6413   std 211.4556   p10 3492.7294   p50 3740.1588   p90 4019.8020
  MC sanity holds: mean within 2% of the EV pin, p10 ≤ p50 ≤ p90 ✓
```

The `vulnerable` uptime (0.4) was never asserted anywhere in the
config — it FALLS OUT of Frost Nova's 4-second buff window recast every
10 seconds. That's Level-2's whole point.

## Counted and snapshotted state: three PoE2 slices

A buff in rtce is internally an INSTANCE LIST. Config collapses it per
mechanic — the binary buff above is the degenerate one-instance case, and
three more policies cover what an ARPG actually asks for: charges that
share one expiry clock (`add_refresh_all`), ailments that stack
independently and each tick the magnitude they were born with
(`add_independent`), and "strongest wins" replacement (`strongest`).

TWO of those three are carried by worked examples, below, each with
hand-worked pins in comments, asserted and run in CI. `strongest` is not
among them: its coverage is the test suite (`sim::exec`'s `mod
snapshot`), not a runnable slice. Worth knowing before reaching for it,
because it is also the policy with the sharpest edge — a LOSING
reapplication changes nothing at all, neither the magnitude nor the
expiry, so a weak reapplication cannot extend a strong ailment.

| Example | Mechanic | Pins |
|---|---|---|
| `poe2_charges` | `add_refresh_all`, `max_stacks: 3`, expression `duration`, `stacks.X` in a rotation `when` | 11748 damage / 293.7 dps / 2.25 avg stacks |
| `poe2_poison` | `add_independent`, unbounded, `snapshot: true`, applied by the skill's own `apply_buff` | 6000 hit + 11625 DoT / 881.25 dps / 3.875 avg stacks |
| `poe2_triggers` | `ProcDef::actions` filter + `cast_action`, `apply_buff` on a primary and on the free-cast secondary | 9870 damage / 493.5 dps / 5 triggered casts |

**Scope, honestly** — and this disclaimer is doing more work than the D4
one. `crates/rtce/tests/fixtures/poe2/gamedef.json` is a PoE2-*shaped*
demonstration slice, not Path of Exile 2's damage model and not derived
from any game data: two damage types with their own scaling chains,
PoE2's `increased`(additive pool)/`more`(independent multiplier) split,
per-type resistance with penetration, an ailment as a condition, and a DoT
objective derived from the same pre-mitigation magnitude as the hit. Every
coefficient in it is `representative`, chosen so each pin hand-derives in
a comment. The real thing lives in `../poe2-calcs` as a GENERATED
`gamedef/poe2.gamedef.json` — 67 stats, 73 buckets, 209 pipeline stages,
80 objectives, pinned to that repo's native `calc.rs` at 1e-9, standing
reference 124.53 dps for a default Monk build. A 209-stage pipeline is not
hand-derivable, which is exactly why the fixture here is trimmed rather
than vendored.

Two things the slices teach that are easy to get wrong, and both are RUN
as contrasts rather than merely asserted in prose:

- **An action-applied snapshot inherits the applying hit; a proc-applied
  one does not.** `poe2_poison` runs the identical config both ways: via
  `apply_buff` on the skill the poison captures under that cast's
  `damage.stats` overlay (rate 150/s), via a proc it captures the ambient
  build (75/s) — same cast count, same hit damage, same stack trajectory,
  exactly half the DoT.
- **A per-stack contribution is LINEAR in the count.** Three charges of
  `+10` in a `product` bucket fold as ×1.30, not ×1.10³. That is correct
  for "increased damage per charge" and a deliberate linearization of a
  true "more" multiplier; `poe2_charges` pins the 1.30 so it stays a
  stated choice.

```
$ cargo run -p rtce --example poe2_poison
PoE2 poison (P7e slice 2) — 20s dummy, EV mode
  viper_strike: 20 casts, 6000.0000 hit damage
  poison: uptime 0.9500, avg_stacks 3.8750, 11625.0000 DoT damage
  total: 17625.0000 damage over 20s = 881.2500 dps

  EV pins hold: 6000 hit + 11625 DoT = 17625 / 881.25 dps / 3.875 stacks ✓

Monte Carlo (N=128, seed=5): mean 881.2500  std 0.0000
  MC reproduces EV exactly (std 0) ✓

  applied by a PROC instead: 5812.5000 DoT (11812.5000 total, 590.6250 dps), avg_stacks 3.8750
  contrast pins hold: 5812.5 DoT — exactly half the action-applied 11625 ✓
```

Nothing in these three configs samples — the fixture's crit is closed
form (`1 + c·(m−1)`, the same choice poe2-calcs' generated gamedef makes)
and every proc they define is `chance: "1"` — so Monte Carlo mode is
asserted to reproduce EV *exactly*, with zero spread, rather than within a
tolerance band. That is the stronger claim: it fails if an RNG draw ever
appears on a path that must stay deterministic. But it is a claim about
exactness, not about MC's distribution machinery: `diablo4_rotation`
remains the only example that actually samples.

## Status

Parity-proven against two independent consumers.

**`../diablo4-calc`** — all 7 of its archetype builds reproduced to <1e-9
relative during the P4 cross-engine proof through rtce (the standing
numbers 8,096.02 … 6,769.10). As of the P4c switchover, diablo4-calc runs
**solely** on rtce in production — its native damage math is deleted, and
`calc::evaluate` is a thin shim over an rtce-compiled plan (including in
the browser, via WASM).

**`../poe2-calcs`** — a GENERATED `gamedef/poe2.gamedef.json` (67 stats,
73 buckets, 209 pipeline stages, 80 objectives) plus an adapter reproduce
that calculator's native math to 1e-9 across 63 parity tests, standing
references 124.53 / 129.51 / 793.76 dps, with a 156-pair
`(StatId, ModKind)` sweep guarding against silent routing drift. Its
native math is deliberately UNTOUCHED: this is a proof that the engine
generalizes past one game, not a second switchover. The harness lives in
that repo.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work by you shall be
dual-licensed as above, without any additional terms or conditions.
