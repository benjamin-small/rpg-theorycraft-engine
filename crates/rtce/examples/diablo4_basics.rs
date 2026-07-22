//! A COMPLETE real-game config set: Diablo 4's damage algorithm (the same
//! committed GameDef the test suite pins against diablo4-calc), a basic
//! Sorcerer build, and two playbooks — a training dummy and a high-DR boss
//! with partial Vulnerable uptime.
//!
//! Run: `cargo run -p rtce --example diablo4_basics`
//!
//! The three tiers on display:
//!   GameDef  (tests/fixtures/d4/gamedef.json) — the ALGORITHM, compiled once
//!   BuildState (inline below)                 — one candidate character
//!   Scenario × 2 (inline below)               — the fights being asked about

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile;
use rtce::scenario::Scenario;

fn main() {
    // ── Tier 1: the game's algorithm, straight from the committed fixture ──
    let gamedef_json = include_str!("../tests/fixtures/d4/gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");
    let plan = compile(&def).expect("gamedef compiles");

    // ── Tier 2: a basic Sorcerer — weapon 1000, 200% skill, 800 Int (×2
    //    mainstat), 20% crit; a spread of typical rolls ──────────────────
    let build: BuildState = serde_json::from_str(
        r#"{
          "stats": {
            "weapon_avg": 1000.0, "coeff_pct": 200.0,
            "mainstat": 800.0, "mainstat_divisor": 800.0,
            "crit_chance": 20.0, "op_chance": 0.0, "op_baseline": 1.5,
            "base_aps": 1.0, "hits_per_use": 1.0,
            "enemy_dr": 0.0, "dot_coeff_pct": 0.0
          },
          "contributions": [
            { "bucket": "additive",   "value": 30.0 },
            { "bucket": "additive",   "value": 25.0, "event": "crit" },
            { "bucket": "crit_group", "value": 20.0 },
            { "bucket": "vuln_group", "value": 20.0 },
            { "bucket": "indep",      "value": 15.0 },
            { "bucket": "as_sum",     "value": 20.0 }
          ]
        }"#,
    )
    .expect("valid build");

    // ── Tier 3: two playbooks ──────────────────────────────────────────
    let dummy: Scenario = serde_json::from_str(
        r#"{ "phases": [ { "name": "dummy", "weight": 1,
              "uptimes": { "vulnerable": 1.0 },
              "stats": { "enemy_dr": 25.0 } } ] }"#,
    )
    .expect("valid scenario");
    let boss: Scenario = serde_json::from_str(
        r#"{ "phases": [ { "name": "boss", "weight": 1,
              "uptimes": { "vulnerable": 0.6 },
              "stats": { "enemy_dr": 90.0 } } ] }"#,
    )
    .expect("valid scenario");

    let mut scratch = plan.scratch();
    let names = plan.objective_names().to_vec();
    let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    let dps_at = |objectives: &[f64]| {
        let i = names.iter().position(|n| n == "total_dps").unwrap();
        objectives[i]
    };

    let dummy_dps = dps_at(plan.evaluate(&build, &dummy, &mut scratch).expect("eval"));
    let boss_dps = dps_at(plan.evaluate(&build, &boss, &mut scratch).expect("eval"));

    println!("Diablo 4 basics — one build, two playbooks");
    println!("  training dummy (vuln 100%, 25% DR): {dummy_dps:>12.4} dps");
    println!("  raid boss      (vuln  60%, 90% DR): {boss_dps:>12.4} dps");

    // The branch table for the dummy fight, via the teaching path.
    let ex = plan.explain(&build, &dummy, &mut scratch).expect("explain");
    println!("\n  dummy branch table (stage `hit`):");
    for b in ex.phases[0].branches.iter().filter(|b| b.stage == "hit") {
        let fired = if b.fired.is_empty() {
            "—".to_string()
        } else {
            b.fired.join("+")
        };
        println!(
            "    {:<12} weight {:>5.2}  event_factors {:>5.2}  hit {:>12.3}",
            fired, b.weight, b.event_factors, b.value
        );
    }

    // Hand-worked pins (the house rule — every example carries its number):
    //   base = 1000 × 2.00 × (1 + 800/800)             = 4000
    //   dummy: vuln_factor = 1 + 1.0×(1.2×1.2 − 1)     = 1.44
    //     no-crit: 4000 × 1.30 × 1.44 × 1.15           = 8611.2
    //     crit:    4000 × 1.55 × (1.5×1.2) × 1.44 × 1.15 = 18480.96
    //     EV = 0.8×8611.2 + 0.2×18480.96               = 10585.152
    //     dps = EV × 0.75 DR × 1.2 APS                 = 9526.6368
    //   boss:  vuln_factor = 1 + 0.6×0.44              = 1.264
    //     EV = 0.8×7558.72 + 0.2×16222.176             = 9291.4112
    //     dps = EV × 0.10 × 1.2                        = 1114.969344
    assert!(
        (dummy_dps - 9526.6368).abs() < 1e-9,
        "dummy pin: {dummy_dps}"
    );
    assert!(
        (boss_dps - 1114.969344).abs() < 1e-9,
        "boss pin: {boss_dps}"
    );
    println!("\n  pins hold: 9526.6368 / 1114.969344 ✓");
}
