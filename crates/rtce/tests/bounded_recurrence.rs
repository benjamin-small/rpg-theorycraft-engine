use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan;
use rtce::scenario::Scenario;

fn empty_build() -> BuildState {
    serde_json::from_str("{}").unwrap()
}

fn scenario() -> Scenario {
    serde_json::from_str(r#"{ "phases": [{ "name": "test", "weight": 1 }] }"#).unwrap()
}

fn evaluate(def: &GameDef, build: &BuildState) -> Result<Vec<f64>, plan::PlanError> {
    let plan = plan::compile(def)?;
    let mut scratch = plan.scratch();
    Ok(plan.evaluate(build, &scenario(), &mut scratch)?.to_vec())
}

fn recurrence_def(
    state: serde_json::Value,
    until: &str,
    result: &str,
    max_iterations: u32,
) -> GameDef {
    serde_json::from_value(serde_json::json!({
        "stats": [],
        "pipeline": [{
            "name": "answer",
            "recurrence": {
                "state": state,
                "until": until,
                "result": result,
                "max_iterations": max_iterations
            }
        }],
        "objectives": ["answer"]
    }))
    .unwrap()
}

#[test]
fn linear_pool_terminates_with_fractional_final_step_accounting() {
    let def = recurrence_def(
        serde_json::json!([
            { "name": "remaining", "initial": "10", "next": "max(remaining - 3, 0)" },
            { "name": "steps", "initial": "0", "next": "steps + 1" },
            { "name": "overkill", "initial": "0", "next": "max(3 - remaining, 0)" }
        ]),
        "remaining <= 0",
        "steps - overkill / 3",
        10,
    );
    let plan = plan::compile(&def).unwrap();
    let mut first_scratch = plan.scratch();
    let first = plan
        .evaluate(&empty_build(), &scenario(), &mut first_scratch)
        .unwrap()[0];
    let mut second_scratch = plan.scratch();
    let second = plan
        .evaluate(&empty_build(), &scenario(), &mut second_scratch)
        .unwrap()[0];
    let mut explain_scratch = plan.scratch();
    let explanation = plan
        .explain(&empty_build(), &scenario(), &mut explain_scratch)
        .unwrap();

    assert_eq!(first.to_bits(), second.to_bits());
    assert_eq!(first.to_bits(), explanation.objectives[0].to_bits());
    assert!((first - 10.0 / 3.0).abs() < 1e-12, "got {first}");
}

#[test]
fn all_state_updates_read_the_same_previous_state() {
    let def = recurrence_def(
        serde_json::json!([
            { "name": "a", "initial": "1", "next": "a + 1" },
            { "name": "b", "initial": "10", "next": "b + a" },
            { "name": "steps", "initial": "0", "next": "steps + 1" }
        ]),
        "steps >= 2",
        "a * 100 + b",
        2,
    );

    // Simultaneous: (a,b) = (1,10) -> (2,11) -> (3,13). An ordered
    // implementation that exposed new `a` to `b` would incorrectly yield 315.
    assert_eq!(evaluate(&def, &empty_build()).unwrap(), vec![313.0]);
}

#[test]
fn recurrence_runtime_errors_name_stage_state_and_iteration() {
    let cases = [
        (
            serde_json::json!([{ "name": "x", "initial": "1 / 0", "next": "x" }]),
            "x > 0",
            "x",
            "state `x` is non-finite at iteration 0",
        ),
        (
            serde_json::json!([{ "name": "x", "initial": "0", "next": "1 / x" }]),
            "x > 1",
            "x",
            "state `x` is non-finite at iteration 1",
        ),
        (
            serde_json::json!([{ "name": "x", "initial": "0", "next": "x + 1" }]),
            "1 / x > 0",
            "x",
            "terminal predicate is non-finite at iteration 0",
        ),
        (
            serde_json::json!([{ "name": "x", "initial": "1", "next": "x" }]),
            "1",
            "1 / 0",
            "result is non-finite at iteration 0",
        ),
    ];

    for (state, until, result, expected) in cases {
        let error = evaluate(&recurrence_def(state, until, result, 4), &empty_build()).unwrap_err();
        assert!(
            error.what.contains("recurrence stage `answer`") && error.what.contains(expected),
            "got: {error}"
        );
    }
}

#[test]
fn non_terminating_recurrence_exhausts_its_budget() {
    let def = recurrence_def(
        serde_json::json!([{ "name": "x", "initial": "0", "next": "x + 1" }]),
        "0",
        "x",
        3,
    );
    let error = evaluate(&def, &empty_build()).unwrap_err();
    assert!(
        error.what.contains("did not terminate within 3 iterations"),
        "got: {error}"
    );
}

#[test]
fn recurrence_identifiers_dependencies_and_bounds_fail_at_compile_time() {
    let duplicate = recurrence_def(
        serde_json::json!([
            { "name": "x", "initial": "0", "next": "x" },
            { "name": "x", "initial": "1", "next": "x" }
        ]),
        "1",
        "x",
        1,
    );
    assert!(plan::compile(&duplicate)
        .unwrap_err()
        .what
        .contains("duplicate state `x`"));

    let collision: GameDef = serde_json::from_value(serde_json::json!({
        "stats": ["x"],
        "pipeline": [{
            "name": "answer",
            "recurrence": {
                "state": [{ "name": "x", "initial": "0", "next": "x" }],
                "until": "1", "result": "x", "max_iterations": 1
            }
        }],
        "objectives": ["answer"]
    }))
    .unwrap();
    assert!(plan::compile(&collision)
        .unwrap_err()
        .what
        .contains("state `x` collides"));

    let invalid_name = recurrence_def(
        serde_json::json!([{ "name": "not.a.name", "initial": "0", "next": "0" }]),
        "1",
        "0",
        1,
    );
    assert!(plan::compile(&invalid_name)
        .unwrap_err()
        .what
        .contains("plain ASCII identifier"));

    let local_in_initial = recurrence_def(
        serde_json::json!([{ "name": "x", "initial": "x", "next": "x" }]),
        "1",
        "x",
        1,
    );
    let error = plan::compile(&local_in_initial).unwrap_err();
    assert!(
        error.what.contains("initial") && error.what.contains("unknown identifier `x`"),
        "got: {error}"
    );

    let forward: GameDef = serde_json::from_value(serde_json::json!({
        "stats": [],
        "pipeline": [
            {
                "name": "answer",
                "recurrence": {
                    "state": [{ "name": "x", "initial": "0", "next": "x + later" }],
                    "until": "x > 1", "result": "x", "max_iterations": 2
                }
            },
            { "name": "later", "expr": "1" }
        ],
        "objectives": ["answer"]
    }))
    .unwrap();
    let error = plan::compile(&forward).unwrap_err();
    assert!(
        error.what.contains("next") && error.what.contains("unknown identifier `later`"),
        "got: {error}"
    );

    let empty = recurrence_def(serde_json::json!([]), "1", "0", 1);
    assert!(plan::compile(&empty)
        .unwrap_err()
        .what
        .contains("at least one state slot"));

    for budget in [0, plan::MAX_RECURRENCE_ITERATIONS + 1] {
        let def = recurrence_def(
            serde_json::json!([{ "name": "x", "initial": "0", "next": "x" }]),
            "1",
            "x",
            budget,
        );
        assert!(plan::compile(&def)
            .unwrap_err()
            .what
            .contains("max_iterations"));
    }

    let state = (0..=plan::MAX_RECURRENCE_STATE_SLOTS)
        .map(|index| {
            serde_json::json!({
                "name": format!("state_{index}"),
                "initial": "0",
                "next": format!("state_{index}")
            })
        })
        .collect();
    let too_wide = recurrence_def(state, "1", "0", 1);
    assert!(plan::compile(&too_wide)
        .unwrap_err()
        .what
        .contains("recurrence state slots > max"));
}

#[test]
fn recurrence_schema_round_trips_and_rejects_typoed_keys_or_branched_mode() {
    let def = recurrence_def(
        serde_json::json!([{ "name": "x", "initial": "0", "next": "x + 1" }]),
        "x >= 1",
        "x",
        1,
    );
    let encoded = serde_json::to_string(&def).unwrap();
    let decoded: GameDef = serde_json::from_str(&encoded).unwrap();
    assert!(decoded.pipeline[0].as_recurrence().is_some());

    let typo = serde_json::from_str::<GameDef>(
        r#"{
          "stats": [],
          "pipeline": [{
            "name": "answer",
            "recurrence": {
              "state": [{ "name": "x", "initial": "0", "nxt": "x" }],
              "until": "1", "result": "x", "max_iterations": 1
            }
          }],
          "objectives": ["answer"]
        }"#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        typo.contains("recurrence state `x`") && typo.contains("nxt"),
        "got: {typo}"
    );

    let typo_budget = serde_json::from_str::<GameDef>(
        r#"{
          "stats": [],
          "pipeline": [{
            "name": "answer",
            "recurrence": {
              "state": [{ "name": "x", "initial": "0", "next": "x" }],
              "until": "1", "result": "x", "max_iteratons": 1
            }
          }],
          "objectives": ["answer"]
        }"#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        typo_budget.contains("a recurrence definition") && typo_budget.contains("max_iteratons"),
        "got: {typo_budget}"
    );

    let branched = serde_json::from_str::<GameDef>(
        r#"{
          "stats": [],
          "pipeline": [{
            "name": "answer", "branched": true,
            "recurrence": {
              "state": [{ "name": "x", "initial": "0", "next": "x" }],
              "until": "1", "result": "x", "max_iterations": 1
            }
          }],
          "objectives": ["answer"]
        }"#,
    )
    .unwrap_err()
    .to_string();
    assert!(
        branched.contains("recurrence stage `answer` cannot declare `branched`"),
        "got: {branched}"
    );
}

#[test]
fn pinned_pob_delayed_loss_oracle_is_entirely_config_driven() {
    let def: GameDef = serde_json::from_str(include_str!(
        "fixtures/poe2/issue22-recurrence-gamedef.json"
    ))
    .unwrap();
    let scenario: Scenario =
        serde_json::from_str(include_str!("fixtures/poe2/issue22-scenario.json")).unwrap();
    let cases = [
        (
            include_str!("fixtures/poe2/issue22-base-build.json"),
            17_582.417_582_418,
            0.0,
        ),
        (
            include_str!("fixtures/poe2/issue22-block-build.json"),
            19_008.019_008_019,
            10.0,
        ),
    ];

    let plan = plan::compile(&def).unwrap();
    let mut observed = Vec::new();
    for (build_json, expected_ehp, expected_block) in cases {
        let build: BuildState = serde_json::from_str(build_json).unwrap();
        let mut scratch = plan.scratch();
        let values = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
        assert!((values[0] - expected_ehp).abs() < 0.01, "got {}", values[0]);
        assert!(
            (values[1] - expected_block).abs() < 0.01,
            "got {}",
            values[1]
        );
        observed.push(values[0]);
    }
    assert!(observed[1] > observed[0]);
}
