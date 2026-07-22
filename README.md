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

## A complete game in config: Diablo 4

Three tiers of configuration, nothing else. **Tier 1 — the GameDef** is the
game's algorithm. This is the (abridged) real one from
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

## Status

Parity-proven against its first consumer, `../diablo4-calc`: all 7 of its
archetype builds reproduced to <1e-9 relative during the P4 cross-engine
proof through rtce (the standing numbers 8,096.02 … 6,769.10). As of the
P4c switchover, diablo4-calc runs
**solely** on rtce in production — its native damage math is deleted, and
`calc::evaluate` is a thin shim over an rtce-compiled plan (including in
the browser, via WASM).

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work by you shall be
dual-licensed as above, without any additional terms or conditions.
