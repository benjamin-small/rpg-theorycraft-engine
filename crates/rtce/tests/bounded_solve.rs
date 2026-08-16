use rtce::build::BuildState;
use rtce::gamedef::{GameDef, StageDef};
use rtce::plan;
use rtce::scenario::Scenario;

fn empty_build() -> BuildState {
    serde_json::from_str("{}").unwrap()
}

fn build_with_stats_and_bucket() -> BuildState {
    serde_json::from_str(
        r#"{
          "stats": { "armour": 10000, "pool": 1000 },
          "contributions": [{ "bucket": "taken", "value": 0 }]
        }"#,
    )
    .unwrap()
}

fn scenario() -> Scenario {
    serde_json::from_str(r#"{ "phases": [{ "name": "dummy", "weight": 1 }] }"#).unwrap()
}

fn evaluate(def: &GameDef, build: &BuildState) -> Result<Vec<f64>, plan::PlanError> {
    let plan = plan::compile(def)?;
    let mut scratch = plan.scratch();
    Ok(plan.evaluate(build, &scenario(), &mut scratch)?.to_vec())
}

fn sqrt_def(max_iterations: u32) -> GameDef {
    serde_json::from_value(serde_json::json!({
        "stats": [],
        "pipeline": [{
            "name": "root",
            "solve": {
                "variable": "x",
                "residual": "x * x - 2",
                "lower": "0",
                "upper": "2",
                "absolute_tolerance": 1e-12,
                "relative_tolerance": 1e-12,
                "max_iterations": max_iterations
            }
        }],
        "objectives": ["root"]
    }))
    .unwrap()
}

#[test]
fn solve_stage_finds_sqrt_two_as_a_repeatable_conservative_lower_bound() {
    let def = sqrt_def(128);
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
    assert!(first * first <= 2.0, "result must stay feasible: {first}");
    assert!((first - 2.0_f64.sqrt()).abs() <= 3e-12, "got {first}");
}

fn routed_residual(x: f64, armour: f64, pool: f64) -> f64 {
    0.2 * x * (1.0 - armour / (armour + 2.0 * x)) + 0.8 * x * (1.0 - armour / (armour + 8.0 * x))
        - pool
}

fn independent_bisection(mut lo: f64, mut hi: f64, armour: f64, pool: f64) -> f64 {
    for _ in 0..128 {
        let tolerance = 1e-7 + 1e-9 * lo.abs().max(hi.abs());
        if hi - lo <= tolerance {
            break;
        }
        let mid = (lo + hi) / 2.0;
        if routed_residual(mid, armour, pool) <= 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

#[test]
fn two_component_rational_solve_reads_stats_bucket_and_prior_stage() {
    let def: GameDef = serde_json::from_value(serde_json::json!({
        "stats": ["armour", "pool"],
        "buckets": { "taken": { "fold": "sum" } },
        "pipeline": [
            { "name": "search_upper", "expr": "pool * 100" },
            {
                "name": "max_hit",
                "solve": {
                    "variable": "incoming_hit",
                    "residual": "(0.2 * incoming_hit * (1 - armour / (armour + 2 * incoming_hit)) + 0.8 * incoming_hit * (1 - armour / (armour + 8 * incoming_hit))) * (1 + taken / 100) - pool",
                    "lower": "0",
                    "upper": "search_upper",
                    "absolute_tolerance": 1e-7,
                    "relative_tolerance": 1e-9,
                    "max_iterations": 128
                }
            },
            { "name": "whole_hit", "expr": "floor(max_hit)" }
        ],
        "objectives": ["max_hit", "whole_hit"]
    }))
    .unwrap();

    let values = evaluate(&def, &build_with_stats_and_bucket()).unwrap();
    let reference = independent_bisection(0.0, 100_000.0, 10_000.0, 1_000.0);
    let tolerance = 1e-7 + 1e-9 * reference.abs().max(values[0].abs());
    assert!((values[0] - reference).abs() <= tolerance);
    assert!(routed_residual(values[0], 10_000.0, 1_000.0) <= 0.0);
    assert_eq!(values[1], values[0].floor());
}

fn one_stage_def(residual: &str, lower: &str, upper: &str) -> GameDef {
    serde_json::from_value(serde_json::json!({
        "stats": [],
        "pipeline": [{
            "name": "root",
            "solve": {
                "variable": "x",
                "residual": residual,
                "lower": lower,
                "upper": upper,
                "absolute_tolerance": 1e-9,
                "relative_tolerance": 1e-9,
                "max_iterations": 128
            }
        }],
        "objectives": ["root"]
    }))
    .unwrap()
}

#[test]
fn inverted_unbracketed_and_non_finite_evaluations_are_errors() {
    let cases = [
        ("x - 1", "2", "0", "bounds are inverted"),
        ("x * x + 1", "0", "2", "root is unbracketed"),
        ("x - 5", "0", "2", "residual(upper=2)"),
        ("x - 1", "1 / 0", "2", "lower bound must be finite"),
        ("1 / (x - 1)", "0", "2", "residual is non-finite at 1"),
    ];
    for (residual, lower, upper, expected) in cases {
        let error = evaluate(&one_stage_def(residual, lower, upper), &empty_build()).unwrap_err();
        assert!(error.what.contains(expected), "got: {error}");
    }
}

#[test]
fn solver_configuration_and_dependencies_fail_at_compile_time() {
    let collision: GameDef = serde_json::from_value(serde_json::json!({
        "stats": ["x"],
        "pipeline": [{
            "name": "root",
            "solve": {
                "variable": "x", "residual": "x - 1", "lower": "0", "upper": "2",
                "absolute_tolerance": 1e-9, "relative_tolerance": 1e-9,
                "max_iterations": 128
            }
        }],
        "objectives": ["root"]
    }))
    .unwrap();
    let error = plan::compile(&collision).unwrap_err();
    assert!(error.what.contains("variable `x` collides"), "got: {error}");

    let forward: GameDef = serde_json::from_value(serde_json::json!({
        "stats": [],
        "pipeline": [
            {
                "name": "root",
                "solve": {
                    "variable": "x", "residual": "x - later", "lower": "0", "upper": "2",
                    "absolute_tolerance": 1e-9, "relative_tolerance": 1e-9,
                    "max_iterations": 128
                }
            },
            { "name": "later", "expr": "1" }
        ],
        "objectives": ["root"]
    }))
    .unwrap();
    let error = plan::compile(&forward).unwrap_err();
    assert!(
        error.what.contains("residual") && error.what.contains("unknown identifier `later`"),
        "got: {error}"
    );

    let local_in_bound = one_stage_def("x - 1", "x", "2");
    let error = plan::compile(&local_in_bound).unwrap_err();
    assert!(
        error.what.contains("lower") && error.what.contains("unknown identifier `x`"),
        "got: {error}"
    );

    let mut invalid_tolerance = sqrt_def(128);
    let StageDef::Solve(stage) = &mut invalid_tolerance.pipeline[0] else {
        panic!("expected solve stage")
    };
    stage.solve.absolute_tolerance = f64::NAN;
    let error = plan::compile(&invalid_tolerance).unwrap_err();
    assert!(
        error.what.contains("tolerances must be finite"),
        "got: {error}"
    );

    let too_many = sqrt_def(plan::MAX_SOLVE_ITERATIONS + 1);
    let error = plan::compile(&too_many).unwrap_err();
    assert!(error.what.contains("max_iterations"), "got: {error}");

    let mut zero_iterations = sqrt_def(1);
    zero_iterations.pipeline[0]
        .as_solve_mut()
        .unwrap()
        .solve
        .max_iterations = 0;
    let error = plan::compile(&zero_iterations).unwrap_err();
    assert!(error.what.contains("max_iterations"), "got: {error}");

    let mut invalid_variable = sqrt_def(128);
    invalid_variable.pipeline[0]
        .as_solve_mut()
        .unwrap()
        .solve
        .variable = "not.a.local".into();
    let error = plan::compile(&invalid_variable).unwrap_err();
    assert!(
        error.what.contains("plain ASCII identifier"),
        "got: {error}"
    );
}

#[test]
fn exhausted_iteration_budget_is_a_defined_error() {
    let error = evaluate(&sqrt_def(1), &empty_build()).unwrap_err();
    assert!(
        error.what.contains("did not converge within 1 iterations"),
        "got: {error}"
    );
}
