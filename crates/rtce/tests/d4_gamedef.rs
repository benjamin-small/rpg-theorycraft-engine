//! The P3 gate: Diablo 4's algorithm as config, reproducing d4-theory-crafting's
//! hand-worked T1–T10 through the generic engine. Every case file carries
//! its arithmetic in `source`. T9's crit-invariance is a separate test.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile;
use rtce::scenario::Scenario;
use rtce_testkit::{assert_close, for_each_fixture};
use std::path::PathBuf;

fn fixtures(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/d4")
        .join(sub)
}

fn d4_plan() -> rtce::plan::Plan {
    let raw = std::fs::read_to_string(fixtures("gamedef.json")).unwrap();
    compile(&serde_json::from_str::<GameDef>(&raw).unwrap()).unwrap()
}

fn scenario_from(uptimes: &serde_json::Value) -> Scenario {
    serde_json::from_value(serde_json::json!({
        "phases": [ { "name": "case", "weight": 1, "uptimes": uptimes } ]
    }))
    .unwrap()
}

#[test]
fn t_cases_reproduce_diablo4_calc_numbers() {
    let plan = d4_plan();
    let names: Vec<String> = plan
        .objective_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut scratch = plan.scratch();
    for_each_fixture(&fixtures("cases"), |case, v| {
        assert!(
            v["uptimes"].is_object(),
            "{case}: uptimes must be an explicit object"
        );
        let build: BuildState = serde_json::from_value(v["build"].clone()).unwrap();
        let scenario = scenario_from(&v["uptimes"]);
        let objectives = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
        let expect = v["expect"].as_object().unwrap();
        assert!(!expect.is_empty(), "{case}: empty expect");
        let tol = v["rel_tolerance"].as_f64().unwrap_or(1e-9);
        for (key, want) in expect {
            let i = names
                .iter()
                .position(|n| n == key)
                .unwrap_or_else(|| panic!("{case}: unknown objective `{key}`"));
            assert_close(
                objectives[i],
                want.as_f64().unwrap(),
                tol,
                &format!("{case}.{key}"),
            );
        }
    });
}

#[test]
fn t9_dot_is_crit_immune() {
    let plan = d4_plan();
    let mut scratch = plan.scratch();
    let raw = std::fs::read_to_string(fixtures("cases/t09_dot_stream.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let mut build: BuildState = serde_json::from_value(v["build"].clone()).unwrap();
    let scenario = scenario_from(&v["uptimes"]);

    let dot_i = plan
        .objective_names()
        .iter()
        .position(|n| *n == "dot_dps")
        .unwrap();
    let before = plan.evaluate(&build, &scenario, &mut scratch).unwrap()[dot_i];
    build.stats.insert("crit_chance".into(), 100.0);
    let after = plan.evaluate(&build, &scenario, &mut scratch).unwrap()[dot_i];
    assert_eq!(before, after, "dot_dps must not move with crit chance");
    assert!((before - 780.0).abs() < 1e-9);
}
