//! Guide chapter 3 — events and branches.
//!
//! Chapter 2's archer hit for exactly 186 every time. This chapter adds
//! the `crit` event, marks `hit` as `branched`, and shows the branch
//! table `explain()` produces — then contrasts a build carrying an
//! event-gated modifier ("+20% damage on critical strikes") against one
//! without.
//!
//! Read along: `docs/guide/03-events-and-branches.md`
//!
//! Run: `cargo run -p rtce --example guide_03_events_and_branches`

use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

fn main() {
    let def: GameDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/03-gamedef.json"))
            .expect("valid gamedef");
    let build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/03-build.json"))
            .expect("valid build");
    let scenario: Scenario =
        serde_json::from_str(include_str!("../tests/fixtures/guide/03-scenario.json"))
            .expect("valid scenario");

    let plan = plan::compile(&def).expect("gamedef compiles");
    let mut scratch = plan.scratch();
    let objectives = plan
        .evaluate(&build, &scenario, &mut scratch)
        .expect("evaluates");

    println!("Guide chapter 3 — events and branches");
    println!("  hit (expected value) = {:.4}", objectives[0]);

    // ── Hand-worked pin: the `crit_damage` member is tagged `event: crit`.
    //    Its `summed_group` is therefore the ×1 identity in the no-crit
    //    branch and 1 + 50/100 = 1.5 in the crit branch. `additive` is
    //    unchanged at 55, so the branched stage evaluates twice:
    //      no crit  weight 1 - 30/100 = 0.7   120 × 1.55 × 1.0 = 186
    //      crit     weight     30/100 = 0.3   120 × 1.55 × 1.5 = 279
    //    hit = 0.7 × 186 + 0.3 × 279 = 130.2 + 83.7 = 213.9
    assert!(
        (objectives[0] - 213.9).abs() < 1e-9,
        "got {}",
        objectives[0]
    );
    println!("  pin holds: 213.9 ✓");

    // ══════════════════ explain(): the branch table ═════════════════════
    //
    // The IDENTICAL engine with per-phase/per-stage/per-branch tracing
    // turned on. `evaluate` above never takes this path, so tracing costs
    // the hot path nothing — but it is how you check the engine's work.
    let mut scratch = plan.scratch();
    let explanation = plan
        .explain(&build, &scenario, &mut scratch)
        .expect("explains");

    println!("\n  branch table (stage `hit`):");
    println!(
        "    {:<8} {:>8} {:>15} {:>10}",
        "fired", "weight", "trace factor", "value"
    );
    for phase in &explanation.phases {
        for b in &phase.branches {
            let fired = if b.fired.is_empty() {
                "—".to_string()
            } else {
                b.fired.join("+")
            };
            println!(
                "    {fired:<8} {:>8.3} {:>15.3} {:>10.3}",
                b.weight, b.event_factors, b.value
            );
        }
    }

    // The two branches are the pin above, taken apart: their weights are
    // the crit chance and its complement, and weight-blending their
    // values reproduces `evaluate`'s single number exactly.
    let branches = &explanation.phases[0].branches;
    assert_eq!(branches.len(), 2, "one event ⇒ two branches");
    let blended: f64 = branches.iter().map(|b| b.weight * b.value).sum();
    assert!(
        (blended - objectives[0]).abs() < 1e-9,
        "branch blend {blended} != evaluate {}",
        objectives[0]
    );
    println!("  branch pins hold: 2 branches, weight-blend reproduces 213.9 exactly ✓");

    // ══════════════ contrast: a modifier that only applies on crit ══════
    //
    // Same gamedef, one extra contribution: `+20` into `additive`, tagged
    // `"event": "crit"`. It is INVISIBLE in the no-crit branch and opens
    // in the crit branch — which is why event-gated modifiers have to be
    // read INSIDE a `branched` stage, not folded once beforehand.
    let oncrit_build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/03-build-oncrit.json"))
            .expect("valid build");

    let mut scratch = plan.scratch();
    let oncrit = plan
        .evaluate(&oncrit_build, &scenario, &mut scratch)
        .expect("evaluates");

    println!(
        "\n  with `+20 additive, event: crit`: hit = {:.4}",
        oncrit[0]
    );

    // ── Hand-worked contrast pin:
    //      no crit  weight 0.7   additive 55   120 × 1.55 × 1.0 = 186
    //      crit     weight 0.3   additive 75   120 × 1.75 × 1.5 = 315
    //    hit = 0.7 × 186 + 0.3 × 315 = 130.2 + 94.5 = 224.7
    //    The no-crit branch is UNCHANGED at 186 — that is the whole
    //    claim of an event gate, and it is what a single pre-folded
    //    `additive` could not express.
    assert!((oncrit[0] - 224.7).abs() < 1e-9, "got {}", oncrit[0]);

    let mut scratch = plan.scratch();
    let oncrit_expl = plan
        .explain(&oncrit_build, &scenario, &mut scratch)
        .expect("explains");
    let nocrit_branch = oncrit_expl.phases[0]
        .branches
        .iter()
        .find(|b| b.fired.is_empty())
        .expect("a no-crit branch exists");
    assert!(
        (nocrit_branch.value - 186.0).abs() < 1e-9,
        "no-crit branch moved: got {}",
        nocrit_branch.value
    );
    println!("  contrast pins hold: 224.7, and the no-crit branch is still exactly 186 ✓");
}
