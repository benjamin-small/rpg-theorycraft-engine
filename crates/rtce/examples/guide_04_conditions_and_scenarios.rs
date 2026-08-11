//! Guide chapter 4 — conditions and scenarios.
//!
//! The last chapter of the calc tier. `enemy_armor` and the `focused`
//! condition arrive, `Scenario` stops being ceremony, and one build
//! produces two different — both correct — answers. Closes with a
//! weighted two-phase scenario.
//!
//! Read along: `docs/guide/04-conditions-and-scenarios.md`
//!
//! Run: `cargo run -p rtce --example guide_04_conditions_and_scenarios`

use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn main() {
    let def: GameDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/04-gamedef.json"))
            .expect("valid gamedef");
    let build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/04-build.json"))
            .expect("valid build");
    let burst: Scenario = serde_json::from_str(include_str!(
        "../tests/fixtures/guide/04-scenario-burst.json"
    ))
    .expect("valid scenario");
    let sustained: Scenario = serde_json::from_str(include_str!(
        "../tests/fixtures/guide/04-scenario-sustained.json"
    ))
    .expect("valid scenario");

    let plan = plan::compile(&def).expect("gamedef compiles");
    let mut scratch = plan.scratch();

    let burst_obj = plan
        .evaluate(&build, &burst, &mut scratch)
        .expect("burst evaluates")
        .to_vec();
    let sustained_obj = plan
        .evaluate(&build, &sustained, &mut scratch)
        .expect("sustained evaluates")
        .to_vec();

    println!("Guide chapter 4 — conditions and scenarios");
    println!("  objectives: {}", plan.objective_names().join(", "));
    println!(
        "  {:<12} {:>8} {:>6} {:>10} {:>16}",
        "scenario", "focused", "armor", "hit", "hit_after_armor"
    );
    println!(
        "  {:<12} {:>8.2} {:>6} {:>10.4} {:>16.4}",
        "burst", 1.0, 5, burst_obj[1], burst_obj[0]
    );
    println!(
        "  {:<12} {:>8.2} {:>6} {:>10.4} {:>16.4}",
        "sustained", 0.2, 20, sustained_obj[1], sustained_obj[0]
    );

    // ── Hand-worked pins. A condition's uptime scales the value of every
    //    contribution gated on it — LINEARLY, which is the modelling
    //    assumption this tier makes and chapter 6 comes back to.
    //
    //    burst — focused = 1.0, so BOTH crit_damage members are fully in
    //    on the crit branch (the normal branch keeps the ×1 identity):
    //      crit_damage = 1 + (50 + 1.0 × 50)/100 = 2.0
    //      hit = 120 × 1.55 × (0.7 × 1.0 + 0.3 × 2.0) = 186 × 1.3 = 241.8
    //      hit_after_armor = 241.8 × (1 - 5/100) = 241.8 × 0.95 = 229.71
    //
    //    sustained — focused = 0.2, so the condition-gated member counts
    //    for a fifth on the crit branch:
    //      crit_damage = 1 + (50 + 0.2 × 50)/100 = 1.6
    //      hit = 120 × 1.55 × (0.7 × 1.0 + 0.3 × 1.6) = 186 × 1.18 = 219.48
    //      hit_after_armor = 219.48 × (1 - 20/100) = 219.48 × 0.8 = 175.584
    assert!(
        close(burst_obj[1], 241.8),
        "burst hit: got {}",
        burst_obj[1]
    );
    assert!(
        close(burst_obj[0], 229.71),
        "burst hit_after_armor: got {}",
        burst_obj[0]
    );
    assert!(
        close(sustained_obj[1], 219.48),
        "sustained hit: got {}",
        sustained_obj[1]
    );
    assert!(
        close(sustained_obj[0], 175.584),
        "sustained hit_after_armor: got {}",
        sustained_obj[0]
    );
    println!("\n  pins hold: 229.71 burst / 175.584 sustained ✓");

    // ══════════════ contrast: one fight, two weighted phases ════════════
    //
    // A Scenario is a LIST of phases, each with its own weight. The
    // engine evaluates every phase and blends by normalised weight — so
    // "25% of this fight is an armor-break window" is one config object,
    // not two runs the caller has to average itself.
    let mixed: Scenario = serde_json::from_str(include_str!(
        "../tests/fixtures/guide/04-scenario-mixed.json"
    ))
    .expect("valid scenario");

    let mut scratch = plan.scratch();
    let mixed_obj = plan
        .evaluate(&build, &mixed, &mut scratch)
        .expect("mixed evaluates")
        .to_vec();

    println!(
        "\n  mixed (1× armor_break + 3× average): hit_after_armor = {:.4}",
        mixed_obj[0]
    );

    // ── Hand-worked contrast pin: the phases are exactly the two
    //    scenarios above, at weights 1 and 3. Weights normalise, so
    //      (1 × 229.71 + 3 × 175.584) / 4
    //        = (229.71 + 526.752) / 4 = 756.462 / 4 = 189.1155
    //    Asserted BOTH ways — against the hand-worked constant and
    //    against a blend of the two runs above — so the pin fails if the
    //    engine ever stops normalising weights.
    assert!(close(mixed_obj[0], 189.1155), "mixed: got {}", mixed_obj[0]);
    let blended = (1.0 * burst_obj[0] + 3.0 * sustained_obj[0]) / 4.0;
    assert!(
        close(mixed_obj[0], blended),
        "mixed {} != weight-blend of the two single-phase runs {blended}",
        mixed_obj[0]
    );
    println!("  contrast pin holds: 189.1155 = (1×229.71 + 3×175.584) / 4 ✓");
}
