use rtce::build::BuildState;
use rtce::expr;
use rtce::gamedef::GameDef;
use rtce::plan;
use rtce::scenario::Scenario;
use rtce::sim::{self, Mode};
use rtce::simdef::{Rotation, SimDef};

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "got {actual}, expected {expected}"
    );
}

#[test]
fn sqrt_pow_and_the_strongest_roll_formula_evaluate_through_plan() {
    let def: GameDef = serde_json::from_str(
        r#"{
          "stats": ["crit", "stack_potential"],
          "pipeline": [
            { "name": "root", "expr": "sqrt(4)" },
            { "name": "strongest_crit_chance",
              "expr": "1 - pow(1 - crit, max(stack_potential, 1))" }
          ],
          "objectives": ["root", "strongest_crit_chance"]
        }"#,
    )
    .unwrap();
    let build: BuildState = serde_json::from_str(
        r#"{ "stats": { "crit": 0.07, "stack_potential": 1.0666666666667 } }"#,
    )
    .unwrap();
    let scenario: Scenario =
        serde_json::from_str(r#"{ "phases": [{ "name": "dummy", "weight": 1 }] }"#).unwrap();
    let plan = plan::compile(&def).unwrap();
    let mut scratch = plan.scratch();
    let values = plan.evaluate(&build, &scenario, &mut scratch).unwrap();

    close(values[0], 2.0);
    close(values[1], 1.0 - 0.93_f64.powf(1.0666666666667_f64.max(1.0)));
}

#[test]
fn sqrt_and_pow_compile_in_sim_expression_fields_too() {
    let def: GameDef = serde_json::from_str(
        r#"{
          "stats": ["power"],
          "pipeline": [{ "name": "hit", "expr": "power" }],
          "objectives": ["hit"]
        }"#,
    )
    .unwrap();
    let build: BuildState = serde_json::from_str(r#"{ "stats": { "power": 10 } }"#).unwrap();
    let scenario: Scenario =
        serde_json::from_str(r#"{ "phases": [{ "name": "dummy", "weight": 4 }] }"#).unwrap();
    let simdef: SimDef = serde_json::from_str(
        r#"{
          "actions": {
            "strike": {
              "cast_time": "sqrt(4)",
              "cooldown": "pow(0, 2)",
              "damage": { "stats": { "power": "pow(10, 1)" } }
            }
          },
          "damage_objective": "hit"
        }"#,
    )
    .unwrap();
    let rotation: Rotation =
        serde_json::from_str(r#"{ "rules": [{ "action": "strike" }] }"#).unwrap();
    let plan = plan::compile(&def).unwrap();
    let sim_plan = sim::compile(&plan, &simdef, &rotation).unwrap();
    let report = sim::run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

    assert_eq!(report.actions["strike"].casts, 2);
    close(report.actions["strike"].damage, 20.0);
}

#[test]
fn arity_and_domain_behavior_are_public_contracts() {
    let symbols = std::collections::BTreeMap::<String, u16>::new();
    let sqrt_error = expr::compile("sqrt(1, 2)", &symbols).unwrap_err();
    assert!(sqrt_error
        .to_string()
        .contains("`sqrt` expects 1 argument(s), got 2"));
    let pow_error = expr::compile("pow(2)", &symbols).unwrap_err();
    assert!(pow_error
        .to_string()
        .contains("`pow` expects 2 argument(s), got 1"));

    assert!(expr::compile("sqrt(-1)", &symbols)
        .unwrap()
        .eval(&[])
        .is_nan());
    assert!(expr::compile("pow(-1, 0.5)", &symbols)
        .unwrap()
        .eval(&[])
        .is_nan());
}

#[test]
fn non_finite_math_is_returned_by_plan_but_rejected_for_sim_quantities() {
    let non_finite_def: GameDef = serde_json::from_str(
        r#"{
          "stats": [],
          "pipeline": [{ "name": "result", "expr": "sqrt(-1)" }],
          "objectives": ["result"]
        }"#,
    )
    .unwrap();
    let empty_build: BuildState = serde_json::from_str("{}").unwrap();
    let scenario: Scenario =
        serde_json::from_str(r#"{ "phases": [{ "name": "dummy", "weight": 1 }] }"#).unwrap();
    let non_finite_plan = plan::compile(&non_finite_def).unwrap();
    let mut scratch = non_finite_plan.scratch();
    assert!(non_finite_plan
        .evaluate(&empty_build, &scenario, &mut scratch)
        .unwrap()[0]
        .is_nan());

    let finite_def: GameDef = serde_json::from_str(
        r#"{
          "stats": [],
          "pipeline": [{ "name": "hit", "expr": "1" }],
          "objectives": ["hit"]
        }"#,
    )
    .unwrap();
    let simdef: SimDef = serde_json::from_str(
        r#"{
          "actions": { "strike": { "cast_time": "sqrt(-1)", "damage": {} } },
          "damage_objective": "hit"
        }"#,
    )
    .unwrap();
    let rotation: Rotation =
        serde_json::from_str(r#"{ "rules": [{ "action": "strike" }] }"#).unwrap();
    let finite_plan = plan::compile(&finite_def).unwrap();
    let sim_plan = sim::compile(&finite_plan, &simdef, &rotation).unwrap();
    let error = sim::run(
        &finite_plan,
        &sim_plan,
        &empty_build,
        &scenario,
        Mode::Expected,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cast_time evaluated to NaN (must be finite and >= 0)"),
        "got: {error}"
    );
}
