//! rtce — RPG theorycraft engine.
//!
//! The game's ALGORITHM (stats, fold rules, events, pipeline) is
//! configuration, compiled once into a flat evaluation plan; candidates
//! (BuildStates) evaluate in microseconds so external drivers can price
//! tens of thousands of permutations. Design:
//! docs/superpowers/specs/2026-07-21-rtce-design.md.

pub mod build;
pub mod expr;
pub mod gamedef;
pub mod plan;
pub mod scenario;
pub mod search;
