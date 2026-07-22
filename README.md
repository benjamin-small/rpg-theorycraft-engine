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
