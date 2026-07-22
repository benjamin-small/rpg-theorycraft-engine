# rtce — RPG theorycraft engine

A generic, config-driven theorycrafting engine. A game's DPS/theorycraft
algorithm — its stats, how contributions fold into buckets, its
probabilistic events (crits, procs, …), and the pipeline of derived
stages — is data, not Rust code. You write that algorithm once as JSON,
`rtce` compiles it into a flat evaluation plan, and every candidate build
then evaluates against that plan in microseconds with zero heap
allocation.

## The three config tiers

1. **`GameDef`** — the ALGORITHM: stat/condition/bucket registries,
   probabilistic events, and the ordered pipeline of stages. Compiled
   once (`plan::compile`) into a `Plan`.
2. **`BuildState`** — ONE candidate: stat values plus tagged
   contributions into buckets.
3. **`Scenario`** — THE FIGHT being asked about: weighted phases with
   stat overrides and condition-uptime fractions.

## Quickstart

```rust
use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

let def: GameDef = serde_json::from_value(serde_json::json!({
    "stats": ["weapon_damage"],
    "pipeline": [{ "name": "dps", "expr": "weapon_damage * 2" }],
    "objectives": ["dps"]
}))
.unwrap();
let plan = plan::compile(&def).unwrap();

let build: BuildState = serde_json::from_value(serde_json::json!({
    "stats": { "weapon_damage": 100.0 }
}))
.unwrap();
let scenario: Scenario = serde_json::from_value(serde_json::json!({
    "phases": [{ "name": "single_target", "weight": 1.0 }]
}))
.unwrap();

let mut scratch = plan.scratch();
let objectives = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
assert_eq!(objectives[0], 200.0);
```

For a fuller walkthrough (multiple stats, a crit event, burst vs
sustained scenarios, and `explain()` output), see
[`examples/your_own_game.rs`](examples/your_own_game.rs) — run it with:

```
cargo run -p rtce --example your_own_game
```

## Status

Parity-proven against its first consumer, `diablo4-calc`: all 7 of its
archetype builds are bit-identical through `rtce` (the standing numbers
8,096.02 … 6,769.10). `diablo4-calc` runs solely on `rtce` in production,
including in the browser via WASM.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE)
or [MIT license](../../LICENSE-MIT) at your option.
