# Applied damage over time

RTCE models an applied damage-over-time mechanic at two fidelity levels:

- the `GameDef` pipeline calculates a fast stat-sheet expectation;
- a snapshot `tick_objective` on a stacking `SimDef` buff runs the actual
  applications and expiries on the timeline.

The executor does not contain a poison, bleed, or ignite formula. Games declare
those rules as stages, then reuse the resulting per-instance DPS objective in
the simulator. This keeps game-specific damage types and coefficients in data
while the library owns the application, snapshot, stack, expiry, attribution,
and reporting semantics.

## Stat-sheet contract

The committed [`applied_dot` fixture](../crates/rtce/tests/fixtures/applied_dot/gamedef.json)
uses poison as a worked example and exports:

| Objective | Meaning |
|---|---|
| `poison_chance` | landed-hit chance × on-hit application chance, each capped to 100% |
| `poison_duration` | base duration after increased/reduced duration |
| `poison_stacks` | application rate × chance × duration, capped by maximum concurrent instances |
| `poison_eff_mult` | resistance/penetration × one combined target damage-taken pool |
| `poison_dps_per_stack` | one instance's DPS; the reusable timeline tick objective |
| `poison_dps` | expected DPS across active instances |
| `poison_damage` | total damage from one instance over its duration |
| `dot_dps` | existing skill DoT plus the applied effect exactly once |

Run it through the ordinary CLI:

```sh
cargo run -p rtce-cli -- evaluate \
  --game crates/rtce/tests/fixtures/applied_dot/gamedef.json \
  --build crates/rtce/tests/fixtures/applied_dot/build.json \
  --scenario crates/rtce/tests/fixtures/applied_dot/scenario.json
```

The neutral fixture produces 100% chance, 2 seconds, one expected stack, a
1.0 effective multiplier, 20 DPS, 40 damage per application, and 27 combined
DoT DPS (7 skill + 20 poison).

## Source and target ownership

An applied effect should split its pipeline at an explicit pre-mitigation
source stage:

1. Finish player-side hit scaling for every damage type that can contribute.
2. Build the effect source from only those unmitigated endpoints.
3. Apply effect magnitude modifiers.
4. Apply resistance, penetration, and target damage-taken modifiers once in
   the effect's effective-multiplier stage.

The fixture's `poison_source_avg` reads physical and chaos endpoints but not
the deliberately large `other_source_*` values. Its four target damage-taken
inputs are added once inside `poison_eff_mult`; no source stage can see them.
Tests pin +10% chaos damage taken at exactly 1.1 rather than 1.21.

This boundary is the reusable rule. Which source types contribute, the base
magnitude, the base duration, the resistance type, and the target buckets are
all game configuration.

## Timeline contract

Use the stat-sheet's per-instance DPS stage as a buff tick objective:

```json
{
  "buffs": {
    "poison": {
      "duration": 2,
      "max_stacks": 1,
      "on_reapply": "add_independent",
      "tick_objective": {
        "objective": "poison_dps_per_stack",
        "snapshot": true
      }
    }
  }
}
```

Use `max_stacks: 0` for an unbounded effect, or a positive integer for a hard
cap. `add_independent` gives every instance its own expiry and captured rate;
`strongest` models a single instance where only a strictly stronger roll can
replace the current one. See `poe2_poison` and `poe2_ignite` for complete,
hand-worked timeline examples.

Every buff entry in `SimReport::buffs` now reports:

- `applications` — application events, including refreshes and at-cap
  replacements;
- `damage` and `dps` — damage attributed directly to that buff's tick
  objective;
- `uptime` and `avg_stacks` — time-integrated presence and stack count.

In Monte Carlo mode, damage, DPS, uptime, and average stacks are arithmetic
means across iterations. Like action casts and proc counts, applications are
the rounded mean count. The per-iteration DPS distribution remains available
in `SimReport::distribution`.
