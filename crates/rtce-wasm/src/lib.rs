//! Browser bindings for the JSON runner.

use rtce_runner::SimulationMode;
use serde_json::Value;
use wasm_bindgen::prelude::*;

fn render(result: Result<Value, rtce_runner::RunnerError>) -> Result<String, JsValue> {
    let value = result.map_err(|error| JsValue::from_str(&error.to_string()))?;
    serde_json::to_string(&value).map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Return the config and expression lexicon used by the CLI and tutorial.
#[wasm_bindgen]
pub fn lexicon() -> Result<String, JsValue> {
    serde_json::to_string(&rtce_runner::lexicon())
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Evaluate a stat-sheet configuration and return a JSON response envelope.
#[wasm_bindgen]
pub fn evaluate(gamedef: &str, build: &str, scenario: &str) -> Result<String, JsValue> {
    render(rtce_runner::evaluate(gamedef, build, scenario))
}

/// Evaluate with a detailed trace and return a JSON response envelope.
#[wasm_bindgen]
pub fn explain(gamedef: &str, build: &str, scenario: &str) -> Result<String, JsValue> {
    render(rtce_runner::explain(gamedef, build, scenario))
}

/// Run a deterministic timeline and return a JSON response envelope.
#[wasm_bindgen]
pub fn simulate_expected(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
) -> Result<String, JsValue> {
    render(rtce_runner::simulate(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        SimulationMode::Expected,
    ))
}

/// Run sampled timelines and return a JSON response envelope.
#[wasm_bindgen]
pub fn simulate_monte_carlo(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    iterations: u32,
    seed: u64,
) -> Result<String, JsValue> {
    render(rtce_runner::simulate(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        SimulationMode::MonteCarlo { iterations, seed },
    ))
}
