use rtce::build::{BuildState, Contribution};
use rtce::gamedef::GameDef;
use rtce::plan::{compile, Plan};
use rtce::scenario::Scenario;

fn fixture() -> (Plan, BuildState, Scenario) {
    let def: GameDef =
        serde_json::from_str(include_str!("fixtures/applied_dot/gamedef.json")).unwrap();
    let build: BuildState =
        serde_json::from_str(include_str!("fixtures/applied_dot/build.json")).unwrap();
    let scenario: Scenario =
        serde_json::from_str(include_str!("fixtures/applied_dot/scenario.json")).unwrap();
    (compile(&def).unwrap(), build, scenario)
}

fn evaluate(plan: &Plan, build: &BuildState, scenario: &Scenario) -> Vec<f64> {
    let mut scratch = plan.scratch();
    plan.evaluate(build, scenario, &mut scratch)
        .unwrap()
        .to_vec()
}

fn output(plan: &Plan, values: &[f64], name: &str) -> f64 {
    let index = plan
        .objective_names()
        .iter()
        .position(|candidate| *candidate == name)
        .unwrap();
    values[index]
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "got {actual}, expected {expected}"
    );
}

#[test]
fn neutral_one_stack_poison_exposes_the_full_breakdown() {
    let (plan, build, scenario) = fixture();
    let values = evaluate(&plan, &build, &scenario);

    close(output(&plan, &values, "poison_chance"), 1.0);
    close(output(&plan, &values, "poison_duration"), 2.0);
    close(output(&plan, &values, "poison_stacks"), 1.0);
    close(output(&plan, &values, "poison_eff_mult"), 1.0);
    close(output(&plan, &values, "poison_dps_per_stack"), 20.0);
    close(output(&plan, &values, "poison_dps"), 20.0);
    close(output(&plan, &values, "poison_damage"), 40.0);
    close(output(&plan, &values, "dot_dps"), 27.0);
}

#[test]
fn target_damage_taken_is_owned_once_by_the_effective_multiplier() {
    let (plan, mut build, scenario) = fixture();
    build.stats.insert("enemy_chaos_taken_pct".into(), 10.0);
    let values = evaluate(&plan, &build, &scenario);

    close(output(&plan, &values, "poison_eff_mult"), 1.1);
    close(output(&plan, &values, "poison_dps"), 22.0);
    close(output(&plan, &values, "poison_damage"), 44.0);
}

#[test]
fn resistance_penetration_and_taken_categories_each_apply_once() {
    let (plan, mut build, scenario) = fixture();
    build
        .stats
        .insert("enemy_chaos_resistance_pct".into(), 25.0);
    build.stats.insert("chaos_penetration_pct".into(), 5.0);
    build.stats.insert("enemy_damage_taken_pct".into(), 10.0);
    build.stats.insert("enemy_dot_taken_pct".into(), 15.0);
    build.stats.insert("enemy_chaos_taken_pct".into(), 20.0);
    build.stats.insert("enemy_chaos_dot_taken_pct".into(), 5.0);
    let values = evaluate(&plan, &build, &scenario);

    close(output(&plan, &values, "poison_eff_mult"), 0.8 * 1.5);
    close(output(&plan, &values, "poison_dps"), 24.0);
}

#[test]
fn chance_duration_rate_and_additive_stack_cap_are_independent_inputs() {
    let (plan, mut build, scenario) = fixture();
    build.stats.insert("poison_chance_on_hit_pct".into(), 50.0);
    build.stats.insert("applications_per_second".into(), 2.0);
    build.contributions.push(Contribution {
        bucket: "additional_max_poisons".into(),
        value: 2.0,
        event: None,
        condition: None,
    });
    let values = evaluate(&plan, &build, &scenario);
    close(output(&plan, &values, "poison_stacks"), 2.0);

    build.stats.insert("applications_per_second".into(), 20.0);
    let capped = evaluate(&plan, &build, &scenario);
    close(output(&plan, &capped, "poison_stacks"), 3.0);
}

#[test]
fn duration_and_magnitude_modifiers_change_their_own_outputs() {
    let (plan, mut build, scenario) = fixture();
    build.contributions.extend([
        Contribution {
            bucket: "poison_duration_pct".into(),
            value: 50.0,
            event: None,
            condition: None,
        },
        Contribution {
            bucket: "poison_magnitude_pct".into(),
            value: 25.0,
            event: None,
            condition: None,
        },
    ]);
    let values = evaluate(&plan, &build, &scenario);

    close(output(&plan, &values, "poison_duration"), 3.0);
    close(output(&plan, &values, "poison_dps"), 25.0);
    close(output(&plan, &values, "poison_damage"), 75.0);
}

#[test]
fn physical_and_chaos_source_damage_contribute_but_other_damage_does_not() {
    let (plan, mut build, scenario) = fixture();
    build.stats.insert("chaos_source_min".into(), 40.0);
    build.stats.insert("chaos_source_max".into(), 60.0);
    let values = evaluate(&plan, &build, &scenario);

    close(output(&plan, &values, "poison_dps"), 30.0);
}
