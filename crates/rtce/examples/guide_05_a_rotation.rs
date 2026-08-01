//! Guide chapter 5 — a rotation.
//!
//! The second tier. The GameDef and BuildState are chapter 4's, byte for
//! byte; what is new is a `SimDef` (a stamina economy and two shots) and
//! a `Rotation` (a priority list), run over a 60-second timeline by
//! `sim::run` in EV mode.
//!
//! **Scope, honestly:** stamina's regen is zeroed and `quick_shot`'s gain
//! is set exactly equal to `power_shot`'s cost so the cast sequence
//! resolves to a clean, hand-verifiable alternation rather than the
//! non-periodic mess continuous regen would produce. That is a teaching
//! choice, not a claim about how a real game's resource economy looks —
//! `examples/diablo4_rotation.rs` makes the same disclosure for the same
//! reason.
//!
//! Read along: `docs/guide/05-a-rotation.md`
//!
//! Run: `cargo run -p rtce --example guide_05_a_rotation`

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile as plan_compile;
use rtce::scenario::Scenario;
use rtce::sim::{compile as sim_compile, run, Mode};
use rtce::simdef::{Rotation, SimDef};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn main() {
    // ── Tiers 1 and 2 — unchanged from chapter 4.
    let def: GameDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/05-gamedef.json"))
            .expect("valid gamedef");
    let build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/05-build.json"))
            .expect("valid build");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── The SEQUENCING config: what the archer can do, and what it costs.
    let simdef: SimDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/05-simdef.json"))
            .expect("valid simdef");
    let rotation: Rotation =
        serde_json::from_str(include_str!("../tests/fixtures/guide/05-rotation.json"))
            .expect("valid rotation");
    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── The fight: 60 seconds of training dummy. `weight` is the phase's
    //    DURATION in seconds once a Scenario is driven by the sim.
    let dummy: Scenario =
        serde_json::from_str(include_str!("../tests/fixtures/guide/05-scenario.json"))
            .expect("valid scenario");

    let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!("Guide chapter 5 — a rotation (60s training dummy, EV mode)");
    println!(
        "  {:<12} {:>6} {:>14} {:>8}",
        "action", "casts", "damage", "share"
    );
    for (name, a) in &report.actions {
        println!(
            "  {name:<12} {:>6} {:>14.4} {:>7.2}%",
            a.casts,
            a.damage,
            a.share * 100.0
        );
    }
    println!(
        "  total: {:.4} damage over {:.0}s = {:.4} dps",
        report.total.total_damage, report.total.duration, report.total.dps
    );
    println!(
        "  stamina: {:.4}s starved, {:.4}s capped",
        report.resources["stamina"].time_starved, report.resources["stamina"].time_capped
    );
    println!(
        "  condition uptimes reported: {}",
        if report.condition_uptime.is_empty() {
            "(none — nothing in this config drives one)".to_string()
        } else {
            report
                .condition_uptime
                .iter()
                .map(|(k, v)| format!("{k} {v:.4}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    // ── Hand-worked pins ────────────────────────────────────────────────
    //
    // Per-hit damage. NOTHING drives `focused` in this chapter — the
    // scenario asserts no uptime and there are no buffs yet — so it is 0
    // and the build's `focused`-gated +50 crit_damage contributes
    // nothing: crit_damage = 1 + 50/100 = 1.5, exactly chapter 3's value.
    //   power_shot (attack_power 120, per `damage.stats`):
    //     120 × 1.55 × (0.7 × 1.0 + 0.3 × 1.5) = 186 × 1.15 = 213.9
    //     × (1 - 20/100) armor                              = 171.12
    //   quick_shot (attack_power 60) — every other factor identical, so
    //     exactly half: 106.95 × 0.8                        =  85.56
    //
    // Cadence. Stamina starts full at 100 with NO regen; power_shot
    // costs 40 and quick_shot gains 40, and both cast in exactly 1s, so a
    // decision lands on every integer second:
    //   t=0: stamina=100 -> power_shot (100-40=60)
    //   t=1: stamina=60  -> power_shot (60-40=20)
    //   t=2: stamina=20<40 -> quick_shot (20+40=60)
    //   t=3: stamina=60  -> power_shot (60-40=20)
    //   t=4: stamina=20<40 -> quick_shot (20+40=60)  … and so on.
    // From t=1 onward stamina alternates 60/20, so power_shot takes every
    // ODD t plus t=0 itself (two in a row only at the start, because
    // stamina begins full) and quick_shot every EVEN t >= 2 — 60 slots
    // (t=0..59) split 31 power_shot / 29 quick_shot.
    //   total = 31 × 171.12 + 29 × 85.56
    //         = 5304.72 + 2481.24 = 7785.96
    //   dps   = 7785.96 / 60 = 129.766
    // Stamina never falls below quick_shot's unconditional fallback and
    // never regens past its 100 cap via the 20/60 oscillation, so both
    // `time_starved` and `time_capped` are exactly 0.
    assert_eq!(report.actions["power_shot"].casts, 31);
    assert_eq!(report.actions["quick_shot"].casts, 29);
    assert!(
        close(report.actions["power_shot"].damage, 5304.72),
        "power_shot damage: got {}",
        report.actions["power_shot"].damage
    );
    assert!(
        close(report.actions["quick_shot"].damage, 2481.24),
        "quick_shot damage: got {}",
        report.actions["quick_shot"].damage
    );
    assert!(
        close(report.total.total_damage, 7785.96),
        "total: got {}",
        report.total.total_damage
    );
    assert!(close(report.total.duration, 60.0));
    assert!(
        close(report.total.dps, 129.766),
        "dps: got {}",
        report.total.dps
    );
    assert!(close(report.resources["stamina"].time_starved, 0.0));
    assert!(close(report.resources["stamina"].time_capped, 0.0));

    // The one that matters for chapter 6. `condition_uptime` reports the
    // conditions the TIMELINE drove, and this config drives none — so the
    // map is EMPTY rather than carrying `focused: 0.0`. `focused` still
    // folds as 0 in the math (that is why crit_damage was 1.5 above and
    // the build's `focused`-gated +50 vanished), it simply has no driver
    // for the sim to integrate. Chapter 6 gives it one.
    assert!(
        report.condition_uptime.is_empty(),
        "expected no driven conditions, got {:?}",
        report.condition_uptime
    );
    println!("\n  pins hold: 31/29 casts, 7785.96 total, 129.766 dps, no condition driven ✓");
}
