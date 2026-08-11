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
}
