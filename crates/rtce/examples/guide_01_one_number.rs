//! Guide chapter 1 — one number.
//!
//! The smallest thing `rtce` can do: one stat, one pipeline stage, one
//! answer. Every config below is the committed file the chapter shows,
//! included verbatim — the prose and this program cannot drift.
//!
//! Read along: `docs/guide/01-one-number.md`
//!
//! Run: `cargo run -p rtce --example guide_01_one_number`

use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

fn main() {
    // ── Tier 1 — the GameDef: the game's ALGORITHM, written as data.
    //    One stat the build must supply, one pipeline stage naming a
    //    number worth computing, and one objective saying which stage's
    //    value to hand back.
    let gamedef_json = include_str!("../tests/fixtures/guide/01-gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");

    // Compile ONCE. Every expression is parsed and slot-resolved here, so
    // the evaluation below allocates nothing. In a real driver this plan
    // is built at startup and reused across thousands of candidates.
    let plan = plan::compile(&def).expect("gamedef compiles");

    // ── Tier 2 — the BuildState: ONE candidate. Just raw stat values so
    //    far; chapter 2 adds the second half (contributions).
    let build_json = include_str!("../tests/fixtures/guide/01-build.json");
    let build: BuildState = serde_json::from_str(build_json).expect("valid build");

    // ── Tier 3 — the Scenario: THE FIGHT being asked about. Nothing to
    //    say about it yet, but it is not optional: the engine always
    //    evaluates a build AGAINST something. One phase, weight 1.
    let scenario_json = include_str!("../tests/fixtures/guide/01-scenario.json");
    let scenario: Scenario = serde_json::from_str(scenario_json).expect("valid scenario");

    let mut scratch = plan.scratch();
    let objectives = plan
        .evaluate(&build, &scenario, &mut scratch)
        .expect("evaluates");

    println!("Guide chapter 1 — one number");
    println!("  objectives: {}", plan.objective_names().join(", "));
    println!("  hit = {:.4}", objectives[0]);

    // ── Hand-worked pin (house rule: every example carries its number).
    //    `hit` is the expression `attack_power`, and the build supplies
    //    attack_power = 120. There is no bucket, no event and no phase
    //    override in this config, so nothing can modify it: 120.
    assert!(
        (objectives[0] - 120.0).abs() < 1e-9,
        "got {}",
        objectives[0]
    );
    println!("\n  pin holds: 120 ✓");
}
