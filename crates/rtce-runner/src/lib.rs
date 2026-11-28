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
use std::io::Write;

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
        json!({ "term": "events.<name>.chance", "kind": "schema", "scope": "GameDef.events", "meaning": "Expression that sets an event's probability. The engine clamps its result to the 0-to-1 range. It may read stats, conditions, buckets and stages before the first branched stage.", "example": "\"chance\": \"crit_chance / 100\"" }),
        json!({ "term": "events.<name>.factor", "kind": "schema", "scope": "GameDef.events", "meaning": "Expression recorded for a fired branch and multiplied into event_multiplier when that optional shortcut is used. It may read stats, conditions, buckets (refolded for the branch) and stages before the first branched stage (their base values).", "example": "\"factor\": \"crit_damage\"" }),
        json!({ "term": "pipeline[].solve", "kind": "schema", "scope": "GameDef solve stages", "meaning": "Run deterministic bounded bisection and store its conservative feasible lower bound as this pipeline stage's value.", "example": "{ \"name\": \"root\", \"solve\": { ... } }" }),
        json!({ "term": "solve.variable", "kind": "schema", "scope": "solve residual", "meaning": "A collision-checked local identifier available only inside the solve residual expression.", "example": "incoming_hit" }),
        json!({ "term": "solve.residual", "kind": "schema", "scope": "GameDef solve stages", "meaning": "A monotone expression where values <= 0 are feasible and values > 0 exceed the constraint.", "example": "incoming_hit * incoming_hit - pool" }),
        json!({ "term": "solve.lower / solve.upper", "kind": "schema", "scope": "GameDef solve stages", "meaning": "Compiled bound expressions over stats, conditions, buckets, and earlier stages; they must bracket the root at evaluation time.", "example": "0 / max_hit_search_upper" }),
        json!({ "term": "solve.absolute_tolerance / solve.relative_tolerance", "kind": "schema", "scope": "GameDef solve stages", "meaning": "Finite non-negative bracket-width tolerances; at least one must be positive.", "example": "1e-7 / 1e-9" }),
        json!({ "term": "solve.max_iterations", "kind": "schema", "scope": "GameDef solve stages", "meaning": "Explicit per-evaluation bisection budget, from 1 through 4096.", "example": "128" }),
        json!({ "term": "pipeline[].recurrence", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "Run a deterministic bounded state machine and store its terminal result as this pipeline stage's value.", "example": "{ \"name\": \"ehp\", \"recurrence\": { ... } }" }),
        json!({ "term": "recurrence.state[]", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "A collision-checked local state slot with an initializer and a next expression. Every next expression reads the same previous state.", "example": "{ \"name\": \"life\", \"initial\": \"max_life\", \"next\": \"max(life - hit, 0)\" }" }),
        json!({ "term": "recurrence.until", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "Terminal predicate checked at iteration zero and after every simultaneous state transition. Zero continues; non-zero terminates.", "example": "life <= 0" }),
        json!({ "term": "recurrence.result", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "Expression over the terminal state that becomes the stage value.", "example": "hits - overkill / hit" }),
        json!({ "term": "recurrence.max_iterations", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "Explicit per-evaluation transition budget, from 1 through 100000.", "example": "1000" }),
        json!({ "term": "recurrence.locals[]", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "Ordered per-transition intermediate values. Each reads stage symbols, the previous state, the batch symbol, and earlier locals; every next expression reads them all. until and result never do. Defaults to empty.", "example": "{ \"name\": \"rest\", \"expr\": \"hit - take_es\" }" }),
        json!({ "term": "recurrence.batch", "kind": "schema", "scope": "GameDef recurrence stages", "meaning": "Engine-chosen batched transitions: symbol is the logical step count of the current transition (visible to locals and next), doubled while candidates stay non-terminal and halved afterwards; terminal candidates roll back unless the size is 1. Every candidate counts against max_iterations. max_size defaults to 1048576.", "example": "{ \"symbol\": \"n\", \"max_size\": 1048576 }" }),
        json!({ "term": "remove_buff", "kind": "schema", "scope": "effects lists (actions, segments, procs)", "meaning": "End every live instance of a buff now; a no-op when it is not up. An action or segment removal is exact in both modes. In expected-value mode, a proc-driven removal of a buff that a sim expression reads is an error: run Monte Carlo instead.", "example": "{ \"remove_buff\": \"hot_streak\" }" }),
        json!({ "term": "gcd", "kind": "schema", "scope": "SimDef.defaults and SimDef.actions.<name>", "meaning": "Global cooldown in seconds, armed when a cast begins and evaluated then. While it runs no other GCD-bound action may begin. The action's value overrides the defaults block. 0 makes an action off the GCD. Absent everywhere means no GCD.", "example": "\"gcd\": \"max(0.75, 1.5 / (1 + haste / 100))\"" }),
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
        json!({ "term": "crit", "kind": "convention", "scope": "GameDef event name", "meaning": "The event name used by on_crit and on_non_crit proc behavior. Other event names still branch damage normally.", "example": "\"event\": \"crit\"" }),
        json!({ "term": "on_cast / on_hit / on_crit / on_non_crit", "kind": "convention", "scope": "ProcDef.trigger", "meaning": "The four supported proc trigger values. on_crit and on_non_crit use the GameDef event named crit (with no such event, every hit is a non-crit).", "example": "\"trigger\": \"on_crit\"" }),
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

impl SimulationMode {
    fn engine_mode(self) -> Mode {
        match self {
            Self::Expected => Mode::Expected,
            Self::MonteCarlo { iterations, seed } => Mode::MonteCarlo { iterations, seed },
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Expected => "expected",
            Self::MonteCarlo { .. } => "monte_carlo",
        }
    }

    fn seed(self) -> Option<String> {
        match self {
            Self::Expected => None,
            Self::MonteCarlo { seed, .. } => Some(seed.to_string()),
        }
    }
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
    simulate_internal(gamedef, build, scenario, simdef, rotation, mode, None)
}

/// Compile and run a simulation with JSON events for one zero-based iteration.
pub fn simulate_with_events(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    mode: SimulationMode,
    iteration: u32,
) -> Result<Value, RunnerError> {
    simulate_internal(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        mode,
        Some(iteration),
    )
}

/// Write the resolved events of one iteration as newline-delimited JSON.
///
/// The first event also identifies the stream version, mode, iteration and
/// seed. Output can contain a partial timeline if the simulation fails.
#[allow(clippy::too_many_arguments)]
pub fn simulate_events_jsonl(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    mode: SimulationMode,
    iteration: u32,
    mut output: impl Write,
) -> Result<(), RunnerError> {
    #[derive(serde::Serialize)]
    struct StartRecord<'a> {
        schema_version: u32,
        mode: &'static str,
        iteration: u32,
        seed: Option<&'a str>,
        #[serde(flatten)]
        event: sim::SimEvent<'a>,
    }

    let seed = mode.seed();
    let mut written = Ok(());
    let result = run_simulation(
        gamedef,
        build,
        scenario,
        simdef,
        rotation,
        mode,
        Some((iteration, &mut |event: sim::SimEvent<'_>| {
            if written.is_ok() {
                written = match event.kind {
                    sim::SimEventKind::SimulationStarted { .. } => serde_json::to_writer(
                        &mut output,
                        &StartRecord {
                            schema_version: sim::EVENT_SCHEMA_VERSION,
                            mode: mode.name(),
                            iteration,
                            seed: seed.as_deref(),
                            event,
                        },
                    ),
                    _ => serde_json::to_writer(&mut output, &event),
                }
                .map_err(std::io::Error::from)
                .and_then(|()| output.write_all(b"\n"))
                .and_then(|()| output.flush());
            }
        })),
    );
    written.map_err(|error| RunnerError::new(format!("could not write event stream: {error}")))?;
    result.map(|_| ())
}

fn simulate_internal(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    mode: SimulationMode,
    event_iteration: Option<u32>,
) -> Result<Value, RunnerError> {
    let mut events = Vec::new();
    let mut collect = |event: sim::SimEvent<'_>| events.push(json!(event));
    let trace = event_iteration.map(|iteration| (iteration, &mut collect as EventSink<'_>));
    let report = run_simulation(gamedef, build, scenario, simdef, rotation, mode, trace)?;

    let mut response = json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "simulation",
        "mode": mode.name(),
        "report": report,
    });
    if let Some(iteration) = event_iteration {
        response["event_stream"] = json!({
            "schema_version": sim::EVENT_SCHEMA_VERSION,
            "iteration": iteration,
            "seed": mode.seed(),
            "events": events,
        });
    }
    Ok(response)
}

type EventSink<'a> = &'a mut dyn FnMut(sim::SimEvent<'_>);

fn run_simulation(
    gamedef: &str,
    build: &str,
    scenario: &str,
    simdef: &str,
    rotation: &str,
    mode: SimulationMode,
    trace: Option<(u32, EventSink<'_>)>,
) -> Result<sim::SimReport, RunnerError> {
    let (gamedef, build, scenario) = inputs(gamedef, build, scenario)?;
    let simdef: SimDef = parse("simdef", simdef)?;
    let rotation: Rotation = parse("rotation", rotation)?;
    let plan = rtce::plan::compile(&gamedef)
        .map_err(|error| RunnerError::new(format!("gamedef did not compile: {error}")))?;
    let sim_plan = sim::compile(&plan, &simdef, &rotation)
        .map_err(|error| RunnerError::new(format!("simulation config did not compile: {error}")))?;
    let mode = mode.engine_mode();
    match trace {
        Some((iteration, sink)) => {
            sim::run_with_events(&plan, &sim_plan, &build, &scenario, mode, iteration, sink)
        }
        None => sim::run(&plan, &sim_plan, &build, &scenario, mode),
    }
    .map_err(|error| RunnerError::new(format!("simulation failed: {error}")))
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
    fn bounded_solve_runs_through_the_shared_cli_and_wasm_json_path() {
        let gamedef = r#"{
          "stats": [],
          "pipeline": [{
            "name": "root",
            "solve": {
              "variable": "x", "residual": "x * x - 2",
              "lower": "0", "upper": "2",
              "absolute_tolerance": 1e-12,
              "relative_tolerance": 1e-12,
              "max_iterations": 128
            }
          }],
          "objectives": ["root"]
        }"#;
        let output = evaluate(gamedef, "{}", SCENARIO).unwrap();
        let root = output["objectives"]["root"].as_f64().unwrap();
        assert!(root * root <= 2.0);
        assert!((root - 2.0_f64.sqrt()).abs() <= 3e-12);
    }

    #[test]
    fn bounded_recurrence_runs_through_the_shared_cli_and_wasm_json_path() {
        let gamedef = r#"{
          "stats": [],
          "pipeline": [{
            "name": "fractional_hits",
            "recurrence": {
              "state": [
                { "name": "pool", "initial": "10", "next": "max(pool - 3, 0)" },
                { "name": "hits", "initial": "0", "next": "hits + 1" },
                { "name": "overkill", "initial": "0", "next": "max(3 - pool, 0)" }
              ],
              "until": "pool <= 0",
              "result": "hits - overkill / 3",
              "max_iterations": 10
            }
          }],
          "objectives": ["fractional_hits"]
        }"#;
        let output = evaluate(gamedef, "{}", SCENARIO).unwrap();
        let hits = output["objectives"]["fractional_hits"].as_f64().unwrap();
        assert!((hits - 10.0 / 3.0).abs() < 1e-12);
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
        assert!(entries
            .iter()
            .any(|entry| { entry["term"] == "pipeline[].solve" && entry["kind"] == "schema" }));
        assert!(entries.iter().any(|entry| {
            entry["term"] == "pipeline[].recurrence" && entry["kind"] == "schema"
        }));
        let trigger = entries
            .iter()
            .find(|entry| entry["scope"] == "ProcDef.trigger")
            .unwrap();
        assert!(
            trigger["term"].as_str().unwrap().contains("on_non_crit")
                && trigger["meaning"].as_str().unwrap().contains("four"),
            "trigger lexicon entry must list on_non_crit: {trigger}"
        );
        let crit = entries.iter().find(|e| e["term"] == "crit").unwrap();
        assert!(crit["meaning"].as_str().unwrap().contains("on_non_crit"));
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
