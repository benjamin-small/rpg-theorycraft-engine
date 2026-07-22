# rpg-theorycraft-engine (rtce)

A generic, config-driven theorycrafting engine. The game's algorithm —
stats, fold rules, probabilistic events, damage pipeline — is
configuration, compiled once into a fast evaluation plan. Extracted from
the proven patterns of `diablo4-calc` and `poe2-calcs`.

- Design: `docs/superpowers/specs/2026-07-21-rtce-design.md`
- Test: `cargo test --workspace`

Crates: `rtce` (engine), `rtce-testkit` (fixture harness, dev-dependency).

## Status

Parity-proven against its first consumer, `../diablo4-calc`: all 7 of its
archetype builds are bit-identical through rtce (the standing numbers
8,096.02 … 6,769.10). As of the P4c switchover, diablo4-calc runs
**solely** on rtce in production — its native damage math is deleted, and
`calc::evaluate` is a thin shim over an rtce-compiled plan (including in
the browser, via WASM).
