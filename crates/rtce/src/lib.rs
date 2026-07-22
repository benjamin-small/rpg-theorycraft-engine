//! `rtce` — RPG theorycraft engine.
//!
//! A game's DPS/theorycraft algorithm — its stats, how contributions fold
//! into buckets, its probabilistic events (crits, procs, …), and the
//! pipeline of derived stages — is data, not Rust code. You write that
//! algorithm once as JSON (or any `serde`-compatible source), `rtce`
//! compiles it into a flat evaluation [`plan::Plan`], and every candidate
//! build then evaluates against that plan in microseconds. This is what
//! lets an external driver price tens of thousands of gear permutations
//! per second: the expensive parsing/compiling work happens once, and the
//! hot path (`Plan::evaluate`) allocates nothing.
//!
//! # The three config tiers
//!
//! An `rtce` game is described by three separate pieces of configuration,
//! each with a different lifetime:
//!
//! 1. [`gamedef::GameDef`] — the ALGORITHM. Stat names, bucket fold rules
//!    ([`gamedef::FoldKind`]: sum / summed-group / product), event chance
//!    and factor expressions, and the ordered pipeline of named stages.
//!    Written once per game, changes rarely. Turned into a [`plan::Plan`]
//!    by [`plan::compile`] — this is the "compile once" half of the
//!    contract, and it's the only place expressions are parsed.
//! 2. [`build::BuildState`] — ONE candidate. Raw stat values plus a list
//!    of tagged [`build::Contribution`]s into buckets (each optionally
//!    gated by an event or a condition). This is the only artifact that
//!    changes per permutation a search driver is comparing.
//! 3. [`scenario::Scenario`] — THE FIGHT being asked about: weighted
//!    phases, each with its own stat overrides and condition-uptime
//!    fractions (e.g. "50% boss enraged"). Lets one build be priced
//!    against "burst" and "sustained" framings without rewriting the
//!    algorithm.
//!
//! # Compile once, evaluate fast
//!
//! [`plan::compile`] is the only step that parses expressions, validates
//! names, and allocates the plan's internal layout — do it once per
//! `GameDef` and keep the resulting [`plan::Plan`] around. [`plan::Plan::evaluate`]
//! is the hot path: pass it a [`build::BuildState`], a [`scenario::Scenario`],
//! and a reusable [`plan::EvalScratch`] (from [`plan::Plan::scratch`]), and it
//! returns the plan's objective values with zero heap allocation. When you
//! need to see WHY a number came out the way it did, [`plan::Plan::explain`]
//! runs the identical engine with per-phase/per-stage/per-branch tracing
//! turned on (it allocates freely — it is a teaching path, never the hot
//! one). [`search`] builds on `evaluate` to price and rank whole batches of
//! candidates expressed as reversible [`search::Move`] sequences.
//!
//! See `examples/your_own_game.rs` for a full walkthrough (a ~40-line JSON
//! game, two scenarios, an objectives table, and `explain()` output), and
//! `docs/superpowers/specs/2026-07-21-rtce-design.md` for the design log.
//!
//! # Example
//!
//! ```
//! use rtce::{build::BuildState, gamedef::GameDef, scenario::Scenario, plan};
//!
//! // Tier 1: GameDef — the algorithm as data. One stat, one stage.
//! let def: GameDef = serde_json::from_value(serde_json::json!({
//!     "stats": ["weapon_damage"],
//!     "pipeline": [{ "name": "dps", "expr": "weapon_damage * 2" }],
//!     "objectives": ["dps"]
//! }))
//! .unwrap();
//!
//! // Compile once into a flat evaluation Plan.
//! let plan = plan::compile(&def).unwrap();
//!
//! // Tier 2: BuildState — one candidate's stat values.
//! let build: BuildState = serde_json::from_value(serde_json::json!({
//!     "stats": { "weapon_damage": 100.0 }
//! }))
//! .unwrap();
//!
//! // Tier 3: Scenario — the fight being asked about.
//! let scenario: Scenario = serde_json::from_value(serde_json::json!({
//!     "phases": [{ "name": "single_target", "weight": 1.0 }]
//! }))
//! .unwrap();
//!
//! // Evaluate: allocation-free once `scratch` is set up.
//! let mut scratch = plan.scratch();
//! let objectives = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
//! assert_eq!(objectives[0], 200.0);
//! ```

#![warn(missing_docs)]

pub mod build;
pub mod expr;
pub mod gamedef;
pub mod plan;
pub mod scenario;
pub mod search;
