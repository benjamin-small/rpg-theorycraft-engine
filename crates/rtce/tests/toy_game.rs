//! The P2 gate: the toy game end-to-end from JSON files — GameDef compiles,
//! BuildState evaluates against TWO scenarios (playbooks), objectives pinned
//! to hand-worked numbers (arithmetic in plan.rs unit tests).

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile;
use rtce::scenario::Scenario;
use rtce_testkit::assert_close;
use std::path::PathBuf;

fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/toy").join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

#[test]
fn toy_game_two_playbooks_pinned() {
    let def: GameDef = load("gamedef.json");
    let build: BuildState = load("build.json");
    let plan = compile(&def).unwrap();
    let mut scratch = plan.scratch();

    let arena: Scenario = load("scenario_arena.json");
    let r = plan.evaluate(&build, &arena, &mut scratch).unwrap();
    assert_close(r.objectives[0], 282.15, 1e-9, "arena dps");

    let dummy: Scenario = load("scenario_dummy.json");
    let r = plan.evaluate(&build, &dummy, &mut scratch).unwrap();
    assert_close(r.objectives[0], 374.34375, 1e-9, "dummy dps");
}
