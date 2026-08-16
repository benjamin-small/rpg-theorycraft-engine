//! `rtce` — RPG theorycraft engine.
//!
//! A game's DPS/theorycraft algorithm — its stats, how contributions fold
//! into buckets, its probabilistic events (crits, procs, …), and the
//! pipeline of derived stages — is data, not Rust code. You write that
//! algorithm once as JSON (or any `serde`-compatible source), `rtce`
//! compiles it into a flat evaluation [`plan::Plan`], and every candidate
//! build then evaluates against that plan in microseconds. This is what
//! lets an external driver price whole candidate sets cheaply: the
//! expensive parsing/compiling work happens once, and the hot path
//! ([`plan::Plan::evaluate`]) allocates nothing, walking a preallocated
//! slot array instead. That is a structural claim about the design, not a
//! measured throughput figure — no benchmark harness ships with the crate
//! yet.
//!
//! # Three fidelity levels
//!
//! One config family answers the same question at three costs, all on the
//! same compiled [`plan::Plan`]:
//!
//! | | What it computes | Uptimes | Cost |
//! |---|---|---|---|
//! | [`plan::Plan::evaluate`] | closed-form average | inputs (asserted) | ~µs |
//! | [`sim::run`] + [`sim::Mode::Expected`] | one deterministic, branch-blended timeline | COMPUTED | ~ms |
//! | [`sim::run`] + [`sim::Mode::MonteCarlo`] | N seeded sampled timelines + a `dps` distribution | COMPUTED | ~ms×N |
//!
//! Where they overlap they are required to agree: `sim::exec`'s keystone
//! test reproduces `evaluate`'s number EXACTLY on a degenerate config with
//! nothing for the timeline to add. Levels 1 and 2 are described below;
//! see the [`sim`] module docs for the executor's own rules (the
//! cast-complete order, the fight horizon, `seq` ordering).
//!
//! # The three config tiers
//!
//! An `rtce` game is described by three separate pieces of configuration,
//! each with a different lifetime:
//!
//! 1. [`gamedef::GameDef`] — the ALGORITHM. Stat names, bucket fold rules
//!    ([`gamedef::FoldKind`]: sum / summed-group / product), event chance
//!    and factor expressions, and the ordered pipeline of named expression or
//!    deterministic bounded-solve stages.
//!    Written once per game, changes rarely. Turned into a [`plan::Plan`]
//!    by [`plan::compile`] — this is the "compile once" half of the
//!    contract, and it's the only place a `GameDef`'s expressions are
//!    parsed ([`sim::compile`] is the matching single parse point for a
//!    [`simdef::SimDef`]'s).
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
//! Across all of it, an unknown config key FAILS CLOSED with a
//! did-you-mean ("unknown field `tick_objectiv` on buff `poison` — did
//! you mean `tick_objective`?") — a typo never silently means "field
//! absent" — while keys starting with `_` are the documented annotation
//! namespace (`_source`, `_scope`, …), accepted at every nesting level.
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
//! See the guide at
//! <https://github.com/benjamin-small/rpg-theorycraft-engine/blob/main/docs/guide/README.md>
//! for a progressive walkthrough (one made-up game built up over seven
//! chapters, from a single stat to a Monte Carlo distribution, each
//! chapter backed by a runnable example), and
//! <https://github.com/benjamin-small/rpg-theorycraft-engine/blob/main/docs/superpowers/specs/2026-07-21-rtce-design.md>
//! for the design log.
//!
//! # Sequencing: from average to timeline
//!
//! [`plan::Plan::evaluate`] above answers "what does this build average,
//! given ASSERTED uptimes?" — fast, but every buff window, resource
//! squeeze, and proc has to be flattened into a `Scenario`'s static
//! numbers by hand. [`simdef::SimDef`] + [`simdef::Rotation`] + [`sim::run`]
//! answer a related question over an actual TIMELINE instead: given a
//! priority-list rotation, resources, cooldowns, buff windows, and procs,
//! what really happens over N seconds, and what uptimes does that produce?
//!
//! Those are the two further config tiers, compiled by [`sim::compile`]
//! against an existing `Plan`:
//!
//! 4. [`simdef::SimDef`] — resources (capped pools with regen),
//!    [`simdef::ActionDef`]s (cast time, cooldown, resource cost/gain, an
//!    optional per-cast `damage.stats` overlay, and an ordered
//!    [`simdef::ActionDef::effects`] list), [`simdef::BuffDef`]s (timed
//!    contribution/condition windows, stack policies, optional DoT ticks),
//!    [`simdef::ProcDef`]s (chance-triggered, ICD-gated, optionally
//!    filtered to named actions, and rolled once per damaging cast or
//!    once per measured hit — [`simdef::ProcRolls`]), plus the
//!    [`simdef::SimDefaults`] block of package-wide semantic knobs (see
//!    "Configurable semantics" below).
//! 5. [`simdef::Rotation`] — a SimC-style priority list; the first
//!    eligible [`simdef::Rule`] wins. Hard gates (off cooldown, cost
//!    payable, not mid-cast) are automatic; a rule's `when` predicate adds
//!    strategy on top.
//!
//! [`sim::run`]'s [`sim::SimReport`] reports COMPUTED
//! buff/condition uptimes, per-buff applications/`avg_stacks`/damage/DPS,
//! per-action casts/damage/share, per-resource starvation and cap time, and
//! proc fire counts — with a `dps` [`sim::Distribution`] added in
//! [`sim::Mode::MonteCarlo`].
//!
//! # Counted and snapshotted state
//!
//! A buff is internally an INSTANCE LIST, collapsed per mechanic by
//! [`simdef::BuffDef::max_stacks`] (default `1`; `0` = unbounded) and
//! [`simdef::ReapplyPolicy`] — `refresh` (the degenerate binary buff, and
//! every pre-0.3.0 config), `add_refresh_all` (counted, one shared expiry
//! clock), `add_independent` (each instance its own duration, evicting the
//! earliest-expiring at the cap), or `strongest` (replace only on a
//! strictly higher magnitude). A [`simdef::TickObjective`] DoT is either
//! LIVE (re-evaluated on every state change, × the count) or
//! `snapshot: true`, in which case each instance captures its rate at its
//! own application and ticks it unchanged to expiry. What a stack count
//! does and does not scale is documented on [`simdef::BuffDef`].
//!
//! # Configurable semantics: the `defaults` block
//!
//! Real games disagree about semantics a generic engine is tempted to
//! hard-code — WHEN a cast measures its world, HOW a proc rolls against
//! a multi-hit cast, WHICH of two same-instant events resolves first.
//! Each is configuration ([`simdef::SimDefaults`]): a package-wide
//! `defaults` block — `{ "defaults": { "measure": "cast_start" } }` —
//! plus per-entity overrides ([`simdef::ActionDef::measure`],
//! [`simdef::ProcDef::rolls`]), every knob a small named enum whose
//! serde default reproduces the pre-0.4.0 behavior byte for byte:
//!
//! - [`simdef::Measure`] — `cast_complete` (default) | `cast_start`: the
//!   instant a cast's WORLD (effective build and phase together) is
//!   captured.
//! - [`simdef::EventOrder`] — `scheduled` (default) |
//!   `completions_first`: which of two COINCIDENT queue events resolves
//!   first. SimDef-global only, by design.
//! - [`simdef::ProcRolls`] — `per_cast` (default) | `per_hit`: how a
//!   proc's chance is rolled against one damaging cast's hits.
//!
//! Each type documents its default, its instant, and its interactions;
//! the [`sim`] module docs' "A buff expiring on the cast grid" section
//! shows the footgun that `measure` and `event_order` each fix from a
//! different end (both contrasts are RUN in `examples/poe2_triggers.rs`).
//!
//! # Examples
//!
//! Thirteen runnable walkthroughs, each with hand-worked pins in
//! comments, asserted and run in CI. Run any with
//! `cargo run -p rtce --example <name>`:
//!
//! - `guide_01_one_number` … `guide_07_monte_carlo` — the seven chapters
//!   of the guide, one made-up archer game grown from a single stat to a
//!   sampled distribution. Chapters 1–4 are Level 1 only; chapter 5 is
//!   the first to call [`sim::run`]. Read alongside `docs/guide/`.
//! - `diablo4_basics` — one build priced against two fights on a real
//!   game's damage slice, plus the branch table behind the crit
//!   expectation.
//! - `diablo4_rotation` — sequencing end to end: mana, a spender/generator
//!   pair, a cooldown-gated buff window whose `vulnerable` uptime falls
//!   OUT of the timeline, in both EV and Monte Carlo mode. One of the two
//!   examples that exercise sampling (`guide_07_monte_carlo` is the
//!   other; the `poe2_*` configs sample nothing and pin MC to reproduce
//!   EV with std exactly zero).
//! - `poe2_charges` — `add_refresh_all` at `max_stacks: 3`, an expression
//!   `duration`, and `stacks.<buff>` gating a rotation rule.
//! - `poe2_poison` — unbounded `add_independent` snapshot DoTs applied by
//!   the skill's own `apply_buff`, re-run via a proc as a contrast.
//! - `poe2_triggers` — a [`simdef::ProcDef::actions`] trigger filter plus
//!   `cast_action`, with `apply_buff` on both a primary and the free-cast
//!   secondary — and the cast-grid footgun's two config fixes as
//!   contrast runs.
//! - `poe2_ignite` — [`simdef::ReapplyPolicy::Strongest`] over rising,
//!   falling and TIED phase power: the win, the discarded loser (whose
//!   expiry does NOT move), and the same falling timeline under
//!   `refresh`'s unconditional re-capture as the contrast.
//!
//! The `diablo4_*` examples run on a thin SLICE of Diablo 4's damage
//! formula, not the game, and `diablo4_rotation`'s cadence is a
//! demonstration rather than real skill data; the `poe2_*` examples run on
//! a PoE2-*shaped* fixture whose every coefficient is `representative`,
//! not Path of Exile 2's damage model. What they demonstrate is the shape.
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
/// Fail-closed unknown-config-key rejection (P8a): the shared
/// did-you-mean walk plus the `_` annotation-namespace carve-out.
/// Internal — its behavior surfaces through `plan::compile`,
/// `sim::compile`, and the config structs' `Deserialize` impls.
mod config_keys;
pub mod expr;
pub mod gamedef;
pub mod plan;
/// A tiny in-crate seeded PCG32 — internal to `plan`'s sampled evaluation
/// and `sim`'s Monte Carlo mode. Deliberately NOT `pub`: the
/// zero-dependency RNG is an implementation detail, never part of this
/// crate's public API (see `rng` module docs).
mod rng;
pub mod scenario;
pub mod search;
pub mod sim;
pub mod simdef;
