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

/// Run a deterministic timeline and include its resolved events.
#[wasm_bindgen]
pub fn simulate_expected_with_events(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
) -> Result<String, JsValue> {
    render(rtce_runner::simulate_with_events(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        SimulationMode::Expected,
        0,
    ))
}

/// Run sampled timelines and include the events of one zero-based iteration.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn simulate_monte_carlo_with_events(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    iterations: u32,
    seed: u64,
    event_iteration: u32,
) -> Result<String, JsValue> {
    render(rtce_runner::simulate_with_events(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        SimulationMode::MonteCarlo { iterations, seed },
        event_iteration,
    ))
}

#[wasm_bindgen]
extern "C" {
    /// A JavaScript callback that receives one JSON event per invocation.
    #[wasm_bindgen(typescript_type = "(eventJson: string) => void")]
    pub type EventCallback;

    #[wasm_bindgen(method, catch, js_name = call)]
    fn call_event(
        this: &EventCallback,
        this_arg: &JsValue,
        event_json: &str,
    ) -> Result<(), JsValue>;
}

/// Deliver each expected-value event synchronously and return the report JSON.
#[wasm_bindgen]
pub fn simulate_expected_stream(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    on_event: &EventCallback,
) -> Result<String, JsValue> {
    simulate_stream(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        SimulationMode::Expected,
        0,
        on_event,
    )
}

/// Deliver each event of one sampled iteration and return the pooled report JSON.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn simulate_monte_carlo_stream(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    iterations: u32,
    seed: u64,
    event_iteration: u32,
    on_event: &EventCallback,
) -> Result<String, JsValue> {
    simulate_stream(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        SimulationMode::MonteCarlo { iterations, seed },
        event_iteration,
        on_event,
    )
}

#[allow(clippy::too_many_arguments)]
fn simulate_stream(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    mode: SimulationMode,
    event_iteration: u32,
    on_event: &EventCallback,
) -> Result<String, JsValue> {
    let mut callback_error = None;
    let result = rtce_runner::stream_simulation_events(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        mode,
        event_iteration,
        |event| {
            if callback_error.is_none() {
                callback_error = on_event.call_event(&JsValue::UNDEFINED, event).err();
            }
        },
    );
    if let Some(error) = callback_error {
        return Err(error);
    }
    render(result)
}
