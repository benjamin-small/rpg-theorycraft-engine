//! JSON-oriented entry points shared by the native CLI and browser/Wasm demo.
//!
//! The engine crate intentionally exposes typed Rust APIs. This small adapter
//! owns the file/tool boundary: it parses the five configuration documents and
//! returns stable, versioned JSON envelopes suitable for a CLI, a browser, or
//! another language binding.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::scenario::Scenario;
use rtce::sim::{self, Mode};
use rtce::simdef::{Rotation, SimDef};
use serde_json::{json, Map, Value};
use std::fmt::{Display, Formatter};

/// Version of the JSON response envelope produced by this crate.
pub const SCHEMA_VERSION: u32 = 1;

/// Return the expression/config vocabulary shared by the CLI and browser
/// tutorial. Entries say whether a name comes from the user's config or is
/// supplied by the engine, so convenient context values never masquerade as
/// ordinary declarations.
pub fn lexicon() -> Value {
    let entries = vec![
        json!({ "term": "stat name", "kind": "declared", "scope": "GameDef and sim expressions", "meaning": "A value declared in GameDef.stats and supplied by the build or scenario.", "example": "attack_power * 2" }),
        json!({ "term": "condition name", "kind": "declared", "scope": "GameDef and sim expressions", "meaning": "A state declared in GameDef.conditions and supplied by scenario uptime or an active buff.", "example": "1 + focused * 0.25" }),
        json!({ "term": "bucket name", "kind": "declared", "scope": "GameDef expressions", "meaning": "A modifier group declared in GameDef.buckets and refolded from build contributions.", "example": "1 + additive / 100" }),
        json!({ "term": "earlier stage name", "kind": "declared", "scope": "GameDef pipeline expressions", "meaning": "The output of an earlier pipeline stage; forward references are rejected.", "example": "base_hit * crit_damage" }),
        json!({ "term": "resource name", "kind": "declared", "scope": "sim expressions", "meaning": "The current amount of a resource declared in SimDef.resources.", "example": "stamina >= 40" }),
        json!({ "term": "fold", "kind": "schema", "scope": "GameDef.buckets.<name>", "meaning": "How one bucket combines contributions: sum adds raw values, summed_group returns 1 + sum/100, and product multiplies each 1 + value/100 factor.", "example": "\"fold\": \"summed_group\"" }),
        json!({ "term": "branched", "kind": "schema", "scope": "GameDef.pipeline stage", "meaning": "Evaluate a stage once per fired/not-fired event combination, then store the probability-weighted average.", "example": "\"branched\": true" }),
        json!({ "term": "contribution.event", "kind": "schema", "scope": "BuildState.contributions[]", "meaning": "Gate this contribution behind a declared event. It is absent normally and folded in only when that event fires.", "example": "\"event\": \"crit\"" }),
        json!({ "term": "events.<name>.chance", "kind": "schema", "scope": "GameDef.events", "meaning": "Expression that sets an event's probability. The engine clamps its result to the 0-to-1 range.", "example": "\"chance\": \"crit_chance / 100\"" }),
        json!({ "term": "events.<name>.factor", "kind": "schema", "scope": "GameDef.events", "meaning": "Expression recorded for a fired branch and multiplied into event_multiplier when that optional shortcut is used.", "example": "\"factor\": \"crit_damage\"" }),
        json!({ "term": "+ - * /", "kind": "operator", "scope": "all expressions", "meaning": "Arithmetic operators; parentheses control grouping.", "example": "base_hit * (1 + additive / 100)" }),
        json!({ "term": "> < >= <= == !=", "kind": "operator", "scope": "all expressions", "meaning": "Comparisons returning exactly 1 for true or 0 for false.", "example": "stamina >= 40" }),
        json!({ "term": "min(a, b)", "kind": "function", "scope": "all expressions", "meaning": "Return the smaller value.", "example": "min(attack_speed, 2)" }),
        json!({ "term": "max(a, b)", "kind": "function", "scope": "all expressions", "meaning": "Return the larger value.", "example": "max(0, 1 - enemy_armor / 100)" }),
        json!({ "term": "clamp(x, lo, hi)", "kind": "function", "scope": "all expressions", "meaning": "Limit a value to an inclusive range.", "example": "clamp(crit_chance / 100, 0, 1)" }),
        json!({ "term": "floor(x)", "kind": "function", "scope": "all expressions", "meaning": "Round a value down to the nearest integer.", "example": "floor(stacks.combo / 3)" }),
        json!({ "term": "sqrt(x)", "kind": "function", "scope": "all expressions", "meaning": "Return the IEEE f64 square root. Negative inputs produce NaN.", "example": "sqrt(armour * pool)" }),
        json!({ "term": "pow(base, exponent)", "kind": "function", "scope": "all expressions", "meaning": "Raise base to a possibly fractional exponent with IEEE f64 semantics.", "example": "1 - pow(1 - crit_chance, max(stack_potential, 1))" }),
        json!({ "term": "and(a, b)", "kind": "function", "scope": "all expressions", "meaning": "Return 1 when both values are nonzero; this does not short-circuit.", "example": "and(stamina >= 40, buff.focus == 1)" }),
        json!({ "term": "or(a, b)", "kind": "function", "scope": "all expressions", "meaning": "Return 1 when either value is nonzero; this does not short-circuit.", "example": "or(time < 5, buff.burst == 1)" }),
        json!({ "term": "not(a)", "kind": "function", "scope": "all expressions", "meaning": "Return 1 when a value is zero, otherwise 0.", "example": "not(buff.focus_window)" }),
        json!({ "term": "event_multiplier", "kind": "engine", "scope": "branched GameDef stages only", "meaning": "Convenience product of every fired event's declared factor; 1 when none fire. Raw gated buckets are clearer when they can express the same rule. A config-declared name with this spelling keeps its declared meaning.", "example": "base_hit * event_multiplier", "aliases": ["event_factors"] }),
        json!({ "term": "time", "kind": "engine", "scope": "sim expressions", "meaning": "Current simulation clock in seconds.", "example": "time < 10" }),
        json!({ "term": "duration", "kind": "engine", "scope": "sim expressions", "meaning": "Total fight duration: the sum of scenario phase weights.", "example": "time < duration / 2" }),
        json!({ "term": "cooldown.<action>", "kind": "engine", "scope": "sim expressions", "meaning": "Seconds until a declared action is ready; 0 means ready.", "example": "cooldown.focus_fire == 0" }),
        json!({ "term": "buff.<buff>", "kind": "engine", "scope": "sim expressions", "meaning": "1 when at least one instance of a declared buff is active, otherwise 0.", "example": "not(buff.focus_window)" }),
        json!({ "term": "buff_remaining.<buff>", "kind": "engine", "scope": "sim expressions", "meaning": "Seconds remaining on the longest live instance of a declared buff; 0 when inactive.", "example": "buff_remaining.poison < 1" }),
        json!({ "term": "casts.<action>", "kind": "engine", "scope": "sim expressions", "meaning": "Number of completed casts of a declared action.", "example": "casts.power_shot >= 3" }),
        json!({ "term": "stacks.<buff>", "kind": "engine", "scope": "sim expressions", "meaning": "Number of live instances of a declared buff.", "example": "stacks.combo >= 3" }),
        json!({ "term": "hits_per_use", "kind": "convention", "scope": "action.damage.stats", "meaning": "Executor-handled hit count for one cast. It defaults to 1 and is not passed to the GameDef as a stat.", "example": "\"hits_per_use\": 3" }),
        json!({ "term": "crit", "kind": "convention", "scope": "GameDef event name", "meaning": "The event name used by on_crit proc behavior. Other event names still branch damage normally.", "example": "\"event\": \"crit\"" }),
        json!({ "term": "on_cast / on_hit / on_crit", "kind": "convention", "scope": "ProcDef.trigger", "meaning": "The three supported proc trigger values. on_crit uses the GameDef event named crit.", "example": "\"trigger\": \"on_crit\"" }),
        json!({ "term": "_source / _guide", "kind": "annotation", "scope": "config objects", "meaning": "Human-readable notes. Any underscore-prefixed key is ignored by the calculation.", "example": "\"_source\": \"Stormstring Bow\"" }),
    ];

    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "lexicon",
        "entries": entries,
    })
}

/// A contextual parse, compile, or evaluation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerError {
    message: String,
}

impl RunnerError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for RunnerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RunnerError {}

/// Simulation fidelity requested by a CLI or language binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationMode {
    /// One deterministic, branch-blended timeline.
    Expected,
    /// Seeded sampled timelines and a DPS distribution.
    MonteCarlo {
        /// Number of independent timelines to sample.
        iterations: u32,
        /// Master seed used to derive each iteration's RNG stream.
        seed: u64,
    },
}

fn parse<T: serde::de::DeserializeOwned>(kind: &str, source: &str) -> Result<T, RunnerError> {
    serde_json::from_str(source)
        .map_err(|error| RunnerError::new(format!("invalid {kind} JSON: {error}")))
}

fn inputs(
    gamedef: &str,
    build: &str,
    scenario: &str,
) -> Result<(GameDef, BuildState, Scenario), RunnerError> {
    Ok((
        parse("gamedef", gamedef)?,
        parse("build", build)?,
        parse("scenario", scenario)?,
    ))
}

/// Compile and evaluate one build against one scenario.
pub fn evaluate(gamedef: &str, build: &str, scenario: &str) -> Result<Value, RunnerError> {
    let (gamedef, build, scenario) = inputs(gamedef, build, scenario)?;
    let plan = rtce::plan::compile(&gamedef)
        .map_err(|error| RunnerError::new(format!("gamedef did not compile: {error}")))?;
    let mut scratch = plan.scratch();
    let values = plan
        .evaluate(&build, &scenario, &mut scratch)
        .map_err(|error| RunnerError::new(format!("evaluation failed: {error}")))?;
    let objectives: Map<String, Value> = plan
        .objective_names()
        .into_iter()
        .zip(values.iter().copied())
        .map(|(name, value)| (name.to_string(), json!(value)))
        .collect();

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "evaluation",
        "objectives": objectives,
    }))
}

/// Compile and evaluate one build with per-phase, per-stage branch traces.
pub fn explain(gamedef: &str, build: &str, scenario: &str) -> Result<Value, RunnerError> {
    let (gamedef, build, scenario) = inputs(gamedef, build, scenario)?;
    let plan = rtce::plan::compile(&gamedef)
        .map_err(|error| RunnerError::new(format!("gamedef did not compile: {error}")))?;
    let mut scratch = plan.scratch();
    let trace = plan
        .explain(&build, &scenario, &mut scratch)
        .map_err(|error| RunnerError::new(format!("explanation failed: {error}")))?;

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "explanation",
        "objective_names": plan.objective_names(),
        "trace": trace,
    }))
}

/// Compile and run a timeline simulation.
pub fn simulate(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    mode: SimulationMode,
) -> Result<Value, RunnerError> {
    let (gamedef, build, scenario) = inputs(gamedef, build, scenario)?;
    let simdef: SimDef = parse("simdef", simdef)?;
    let rotation: Rotation = parse("rotation", rotation)?;
    let plan = rtce::plan::compile(&gamedef)
        .map_err(|error| RunnerError::new(format!("gamedef did not compile: {error}")))?;
    let sim_plan = sim::compile(&plan, &simdef, &rotation)
        .map_err(|error| RunnerError::new(format!("simulation config did not compile: {error}")))?;
    let (engine_mode, mode_name) = match mode {
        SimulationMode::Expected => (Mode::Expected, "expected"),
        SimulationMode::MonteCarlo { iterations, seed } => {
            (Mode::MonteCarlo { iterations, seed }, "monte_carlo")
        }
    };
    let report = sim::run(&plan, &sim_plan, &build, &scenario, engine_mode)
        .map_err(|error| RunnerError::new(format!("simulation failed: {error}")))?;

    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "simulation",
        "mode": mode_name,
        "report": report,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAMEDEF: &str = r#"{
        "stats": ["power"],
        "pipeline": [{"name": "hit", "expr": "power * 2"}],
        "objectives": ["hit"]
    }"#;
    const BUILD: &str = r#"{"stats": {"power": 21}}"#;
    const SCENARIO: &str = r#"{"phases": [{"name": "dummy", "weight": 1}]}"#;

    #[test]
    fn evaluation_has_a_versioned_named_objective() {
        let output = evaluate(GAMEDEF, BUILD, SCENARIO).unwrap();
        assert_eq!(output["schema_version"], 1);
        assert_eq!(output["kind"], "evaluation");
        assert_eq!(output["objectives"]["hit"], 42.0);
    }

    #[test]
    fn errors_name_the_document_that_failed() {
        let error = evaluate("{}", BUILD, SCENARIO).unwrap_err().to_string();
        assert!(error.starts_with("invalid gamedef JSON:"), "got: {error}");
    }

    #[test]
    fn lexicon_distinguishes_declared_names_from_engine_context() {
        let output = lexicon();
        assert_eq!(output["kind"], "lexicon");
        let entries = output["entries"].as_array().unwrap();
        assert!(entries
            .iter()
            .any(|entry| { entry["term"] == "bucket name" && entry["kind"] == "declared" }));
        assert!(entries
            .iter()
            .any(|entry| { entry["term"] == "event_multiplier" && entry["kind"] == "engine" }));
        assert!(entries
            .iter()
            .any(|entry| { entry["term"] == "contribution.event" && entry["kind"] == "schema" }));
        for term in ["sqrt(x)", "pow(base, exponent)"] {
            assert!(
                entries
                    .iter()
                    .any(|entry| entry["term"] == term && entry["kind"] == "function"),
                "missing expression function `{term}` from the shared lexicon"
            );
        }
    }
}
