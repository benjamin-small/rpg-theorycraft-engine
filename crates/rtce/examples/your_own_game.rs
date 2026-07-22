//! Define your own game in ~40 lines of JSON.
//!
//! A self-contained walkthrough of all three `rtce` config tiers using a
//! tiny made-up archer game: attack power, a chance to crit, and an armor
//! debuff window. Run it with:
//!
//! ```text
//! cargo run -p rtce --example your_own_game
//! ```

use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

fn main() {
    // ---------------------------------------------------------------
    // TIER 1 — GameDef: the game's ALGORITHM, written once as data.
    // ---------------------------------------------------------------
    // - stats: three raw numbers every build supplies.
    // - conditions: uptime-gated tags an expression can read directly
    //   ("focused" — a "focus fire" buff window, 0..1 fraction of time up).
    // - buckets: one `summed_group` bucket. Same-type modifiers (crit
    //   damage %) SUM before turning into a multiplier: 1 + Σv/100.
    // - events: "crit" — a chance (stat-driven) and a factor (the folded
    //   crit_damage bucket) multiplied into `event_factors` when it fires.
    // - pipeline: three stages, each seeing every stat/condition/bucket
    //   and every EARLIER stage by name. `hit` is `branched`: it is
    //   evaluated once per combination of fired/unfired events and stored
    //   as the probability-weighted expected value.
    let def: GameDef = serde_json::from_str(
        r#"{
          "stats": ["attack_power", "crit_chance", "enemy_armor"],
          "conditions": ["focused"],
          "buckets": {
            "crit_damage": { "fold": "summed_group" }
          },
          "events": {
            "crit": { "chance": "crit_chance / 100", "factor": "crit_damage" }
          },
          "pipeline": [
            { "name": "base_hit", "expr": "attack_power" },
            { "name": "hit", "expr": "base_hit * event_factors", "branched": true },
            { "name": "dps", "expr": "hit * (1 - enemy_armor / 100)" }
          ],
          "objectives": ["dps"]
        }"#,
    )
    .expect("gamedef parses");

    // Compile ONCE. Every expression is parsed and slot-resolved here;
    // `plan` below is reused for every scenario, every candidate.
    let plan = plan::compile(&def).expect("gamedef compiles");

    // ---------------------------------------------------------------
    // TIER 2 — BuildState: ONE candidate. Raw stat values plus tagged
    // contributions into buckets. This is what a search driver would
    // vary across thousands of permutations; everything above stays
    // fixed.
    // ---------------------------------------------------------------
    let build: BuildState = serde_json::from_str(
        r#"{
          "stats": { "attack_power": 120.0, "crit_chance": 30.0, "enemy_armor": 15.0 },
          "contributions": [
            { "bucket": "crit_damage", "value": 50.0 },
            { "bucket": "crit_damage", "value": 50.0, "condition": "focused" }
          ]
        }"#,
    )
    .expect("build parses");

    // ---------------------------------------------------------------
    // TIER 3 — Scenario: THE FIGHT being asked about. Two framings of
    // the SAME build: a burst window (armor freshly broken, focus fire
    // fully up) and a sustained average (armor intact, focus fire only
    // occasionally up). Each phase can override stats and set condition
    // uptimes independently.
    // ---------------------------------------------------------------
    let burst: Scenario = serde_json::from_str(
        r#"{ "phases": [
              { "name": "armor_break", "weight": 1,
                "uptimes": { "focused": 1.0 },
                "stats": { "enemy_armor": 5.0 } } ] }"#,
    )
    .expect("burst scenario parses");

    let sustained: Scenario = serde_json::from_str(
        r#"{ "phases": [
              { "name": "average", "weight": 1,
                "uptimes": { "focused": 0.2 },
                "stats": { "enemy_armor": 20.0 } } ] }"#,
    )
    .expect("sustained scenario parses");

    // ---------------------------------------------------------------
    // Evaluate both scenarios against the same build — the hot path,
    // no allocation beyond the reused `scratch` buffers.
    // ---------------------------------------------------------------
    let mut scratch = plan.scratch();
    let burst_objectives = plan
        .evaluate(&build, &burst, &mut scratch)
        .expect("burst evaluates")
        .to_vec();
    let sustained_objectives = plan
        .evaluate(&build, &sustained, &mut scratch)
        .expect("sustained evaluates")
        .to_vec();

    println!("Objectives ({}):", plan.objective_names().join(", "));
    println!("{:<12} {:>10}", "scenario", "dps");
    println!("{:<12} {:>10.2}", "burst", burst_objectives[0]);
    println!("{:<12} {:>10.2}", "sustained", sustained_objectives[0]);

    // Hand-worked pins: burst = 120 * (0.7*1 + 0.3*2) * (1 - 5/100) = 148.20;
    // sustained = 120 * (0.8*1 + 0.2*2) * (1 - 20/100) = 113.28. These are
    // computed by hand from the JSON above, not copied from the program's
    // own output, so they catch a regression in the engine itself.
    let burst_dps = burst_objectives[0];
    let sustained_dps = sustained_objectives[0];
    assert!((burst_dps - 148.20).abs() < 1e-9);
    assert!((sustained_dps - 113.28).abs() < 1e-9);

    // ---------------------------------------------------------------
    // explain() runs the IDENTICAL engine with per-phase/per-stage/
    // per-branch tracing turned on — the "show your work" path. It
    // allocates freely (an `Explanation` tree); `evaluate` above never
    // takes this branch, so tracing costs nothing on the hot path.
    // ---------------------------------------------------------------
    let mut scratch = plan.scratch();
    let explanation = plan
        .explain(&build, &burst, &mut scratch)
        .expect("burst explains");

    println!("\nexplain() — burst scenario:");
    for phase in &explanation.phases {
        println!("  phase `{}` (weight {:.3})", phase.name, phase.weight);
        for (name, v) in &phase.conditions {
            println!("    condition {name:<10} = {v:.3}");
        }
        for (name, v) in &phase.buckets {
            println!("    bucket    {name:<10} = {v:.3}");
        }
        for (name, v) in &phase.stages {
            println!("    stage     {name:<10} = {v:.3}");
        }

        println!("\n  branch table ({} branches):", phase.branches.len());
        println!(
            "    {:<8} {:<10} {:>8} {:>14} {:>10}",
            "stage", "fired", "weight", "event_factors", "value"
        );
        for b in &phase.branches {
            let fired = if b.fired.is_empty() {
                "-".to_string()
            } else {
                b.fired.join("+")
            };
            println!(
                "    {:<8} {:<10} {:>8.3} {:>14.3} {:>10.3}",
                b.stage, fired, b.weight, b.event_factors, b.value
            );
        }
    }
}
