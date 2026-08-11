//! Guide chapter 6 — buffs, and the uptime you no longer assert.
//!
//! One action and one buff added to chapter 5's SimDef. Chapter 4's
//! hand-typed `"focused": 0.2` is now COMPUTED — and it computes to two
//! different numbers depending on what you mean by "uptime", which is
//! this chapter's real subject.
//!
//! Read along: `docs/guide/06-buffs-and-uptime.md`
//!
//! Run: `cargo run -p rtce --example guide_06_buffs_and_uptime`

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
    // ── Tiers 1 and 2 — still chapter 4's, byte for byte.
    let def: GameDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/06-gamedef.json"))
            .expect("valid gamedef");
    let build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/06-build.json"))
            .expect("valid build");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── Chapter 5's SimDef plus `focus_fire` (instant, 10s cooldown) and
    //    the `focus_window` buff it applies (2.5s, drives `focused` to 1).
    let simdef: SimDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/06-simdef.json"))
            .expect("valid simdef");
    let rotation: Rotation =
        serde_json::from_str(include_str!("../tests/fixtures/guide/06-rotation.json"))
            .expect("valid rotation");
    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    let dummy: Scenario =
        serde_json::from_str(include_str!("../tests/fixtures/guide/06-scenario.json"))
            .expect("valid scenario");

    let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!("Guide chapter 6 — buffs and uptime (60s training dummy, EV mode)");
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
        "  focus_window buff uptime: {:.4}   focused condition uptime: {:.4}",
        report.buffs["focus_window"].uptime, report.condition_uptime["focused"]
    );

    // ── Hand-worked pins ────────────────────────────────────────────────
    //
    // Per-hit damage, now in two flavours. `focus_window` drives
    // `focused` to 1.0 while up, which opens the build's gated +50
    // crit_damage. The normal branch keeps its ×1 identity throughout:
    //   focused ACTIVE   crit-branch crit_damage = 1 + (50+50)/100 = 2.0
    //     power_shot 120 × 1.55 × (0.7 + 0.3 × 2.0) = 186 × 1.3  = 241.8
    //                × 0.8 armor                                 = 193.44
    //     quick_shot (half attack power)                         =  96.72
    //   focused INACTIVE crit-branch crit_damage = 1 + 50/100 = 1.5
    //     power_shot                                             = 171.12
    //     quick_shot                                             =  85.56
    //
    // Cadence. `focus_fire` is instant (cast_time 0) so it never consumes
    // a decision slot — the 31/29 power/quick alternation from chapter 5
    // is UNCHANGED. It fires the moment it is off cooldown: t=0, 10, 20,
    // 30, 40, 50 — six casts, each opening a window [s, s+2.5).
    //
    //   TIME-weighted uptime = 6 × 2.5 / 60 = 0.25
    //
    // But damage is measured per CAST, and casts complete on the integer
    // grid t=1..60. Which completions land inside a window [s, s+2.5)?
    //   s=0  -> completions at 1, 2                        (2)
    //   s=10 -> the completion AT t=10 was scheduled at t=9, BEFORE
    //           focus_fire's coincident completion at t=10, and
    //           `event_order: "scheduled"` (the default) resolves them in
    //           that order — so it measures WITHOUT the buff. 11, 12  (2)
    //   … and identically for s=20, 30, 40, 50.
    //   12 active completions out of 60.
    //
    //   CAST-weighted uptime = 12 / 60 = 0.20
    //
    // Reading off which action landed on each active completion (a
    // completion at t started at t-1; power_shot at t=0 and every odd t,
    // quick_shot at every even t >= 2):
    //   active at t = 1, 2, 11, 12, 21, 22, 31, 32, 41, 42, 51, 52
    //   started at   0, 1, 10, 11, 20, 21, 30, 31, 40, 41, 50, 51
    //   -> power_shot (t-1 = 0 or odd): 0,1,11,21,31,41,51  = 7 active
    //   -> quick_shot (t-1 even >= 2):  10,20,30,40,50      = 5 active
    //   totals check: 7 + 24 = 31 power_shot, 5 + 24 = 29 quick_shot ✓
    //
    //   total = 7 × 193.44 + 24 × 171.12 + 5 × 96.72 + 24 × 85.56
    //         = 1354.08 + 4106.88 + 483.60 + 2053.44
    //         = 7998.00
    //   dps   = 7998.00 / 60 = 133.30
    assert_eq!(report.actions["focus_fire"].casts, 6);
    assert_eq!(report.actions["power_shot"].casts, 31);
    assert_eq!(report.actions["quick_shot"].casts, 29);
    assert!(
        close(report.actions["power_shot"].damage, 5460.96),
        "power_shot damage: got {}",
        report.actions["power_shot"].damage
    );
    assert!(
        close(report.actions["quick_shot"].damage, 2537.04),
        "quick_shot damage: got {}",
        report.actions["quick_shot"].damage
    );
    assert!(
        close(report.total.total_damage, 7998.0),
        "total: got {}",
        report.total.total_damage
    );
    assert!(
        close(report.total.dps, 133.3),
        "dps: got {}",
        report.total.dps
    );

    // The headline: chapter 4's hand-typed 0.2 is gone, replaced by a
    // number that FALLS OUT of a 2.5s window recast every 10s.
    assert!(
        close(report.buffs["focus_window"].uptime, 0.25),
        "buff uptime: got {}",
        report.buffs["focus_window"].uptime
    );
    assert!(
        close(report.condition_uptime["focused"], 0.25),
        "condition uptime: got {}",
        report.condition_uptime["focused"]
    );
    println!("\n  pins hold: 7998.00 total / 133.30 dps / 0.25 uptime ✓");

    // ══════ the lesson: 0.25 is not the uptime the damage experienced ═══
    //
    // Reconstruct the cast-weighted uptime from the damage itself. Each
    // action's total is (active count × active value) + (inactive count ×
    // inactive value), so solving for the active count is exact algebra
    // on numbers the report already gave us — no re-simulation.
    let power_active = (report.actions["power_shot"].damage - 31.0 * 171.12) / (193.44 - 171.12);
    let quick_active = (report.actions["quick_shot"].damage - 29.0 * 85.56) / (96.72 - 85.56);
    let cast_weighted = (power_active + quick_active) / 60.0;

    println!("\n  the same window, measured two ways:");
    println!(
        "    {:<26} {:>8.4}   6 windows × 2.5s / 60s",
        "time-weighted uptime", report.condition_uptime["focused"]
    );
    println!(
        "    {:<26} {:>8.4}   12 of 60 completions measured inside one",
        "cast-weighted uptime", cast_weighted
    );

    // ── Hand-worked contrast pins. These two numbers disagree, and the
    //    gap is not rounding: a 2.5-second window straddles two integer
    //    completions, not two and a half. Damage follows the CAST-weighted
    //    0.20; the integrated 0.25 is a true statement about seconds that
    //    is the wrong statement about hits.
    assert!(
        close(power_active, 7.0),
        "active power_shots: got {power_active}"
    );
    assert!(
        close(quick_active, 5.0),
        "active quick_shots: got {quick_active}"
    );
    assert!(
        close(cast_weighted, 0.2),
        "cast-weighted uptime: got {cast_weighted}"
    );
    assert!(
        (report.condition_uptime["focused"] - cast_weighted).abs() > 0.04,
        "the two uptimes were expected to DISAGREE, got {} and {cast_weighted}",
        report.condition_uptime["focused"]
    );
    println!("  contrast pins hold: 0.25 integrated vs 0.20 experienced — 7 and 5 active casts ✓");

    // And the practical consequence, which is why this is worth a
    // chapter: feeding the reported 0.25 back into chapter 4's calc tier
    // does NOT reproduce the sim's answer.
    //   crit-branch crit_damage = 1 + (50 + 0.25 × 50)/100 = 1.625
    //   power_shot  = 120 × 1.55 × (0.7 + 0.3 × 1.625) × 0.8 = 176.7
    //   naive total = 31 × 176.7 + 29 × 88.35 = 5477.7 + 2562.15 = 8039.85
    // — 41.85 too high, because it credits the window for 2.5 completions
    // when it only ever bought 2.
    let naive_total = 31.0 * 176.7 + 29.0 * 88.35;
    assert!(close(naive_total, 8039.85), "got {naive_total}");
    assert!(
        close(naive_total - report.total.total_damage, 41.85),
        "gap: got {}",
        naive_total - report.total.total_damage
    );
    println!(
        "  feeding 0.25 back into the calc tier gives {naive_total:.2} — {:.2} too high ✓",
        naive_total - report.total.total_damage
    );

    // ═══ contrast: the `measure` knob does not RECONCILE the two ════════
    //
    // The same config with `defaults: { measure: "cast_start" }` — the
    // documented fix for measuring on a collision instant. It moves the
    // measurement, and it is worth seeing exactly what that buys.
    let cast_start: SimDef = serde_json::from_str(include_str!(
        "../tests/fixtures/guide/06-simdef-cast-start.json"
    ))
    .expect("valid simdef");
    let cs_plan = sim_compile(&plan, &cast_start, &rotation).expect("simdef compiles");
    let cs = run(&plan, &cs_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    let cs_power_active = (cs.actions["power_shot"].damage - 31.0 * 171.12) / (193.44 - 171.12);
    let cs_quick_active = (cs.actions["quick_shot"].damage - 29.0 * 85.56) / (96.72 - 85.56);
    let cs_cast_weighted = (cs_power_active + cs_quick_active) / 60.0;

    println!("\n  the same fight under `measure: \"cast_start\"`:");
    println!(
        "    {:<26} {:>8.4}   unchanged — a timeline fact, not a measurement one",
        "time-weighted uptime", cs.condition_uptime["focused"]
    );
    println!(
        "    {:<26} {:>8.4}   now OVERSHOOTS 0.25 instead of undershooting",
        "cast-weighted uptime", cs_cast_weighted
    );
    println!(
        "    {:<26} {:>8.4}   vs {:.4} under the default",
        "dps", cs.total.dps, report.total.dps
    );

    // ── Hand-worked contrast pins. Measuring at cast START, a cast begun
    //    at t is inside the window [s, s+2.5) when s <= t < s+2.5 — so
    //    t = s, s+1, s+2: THREE per window, 18 of 60, cast-weighted 0.30.
    //      window at s=0:  power(0), power(1), quick(2)  -> 2 power, 1 quick
    //      windows s=10..50: quick(even), power(odd), quick(even)
    //                                                    -> 1 power, 2 quick
    //      totals: 2 + 5 = 7 power, 1 + 10 = 11 quick = 18 active
    //      total = 7 × 193.44 + 24 × 171.12 + 11 × 96.72 + 18 × 85.56
    //            = 1354.08 + 4106.88 + 1063.92 + 1540.08 = 8064.96
    //      dps   = 8064.96 / 60 = 134.416
    //
    //    The lesson the knob teaches by NOT fixing this: 0.30 is as wrong
    //    as 0.20, in the other direction. The time-weighted 0.25 is
    //    unreachable because a 2.5s window cannot contain 2.5 casts. The
    //    `measure` and `event_order` knobs choose WHICH WAY you are wrong
    //    about a boundary; they cannot make hits divisible.
    assert!(
        close(cs.total.total_damage, 8064.96),
        "cast_start total: got {}",
        cs.total.total_damage
    );
    assert!(
        close(cs.total.dps, 134.416),
        "cast_start dps: got {}",
        cs.total.dps
    );
    assert!(
        close(cs.condition_uptime["focused"], 0.25),
        "cast_start moved the TIME uptime, which it must not: got {}",
        cs.condition_uptime["focused"]
    );
    assert!(
        close(cs_power_active, 7.0) && close(cs_quick_active, 11.0),
        "cast_start active split: got {cs_power_active} power / {cs_quick_active} quick"
    );
    assert!(
        close(cs_cast_weighted, 0.3),
        "cast_start cast-weighted uptime: got {cs_cast_weighted}"
    );
    // The two knobs bracket the integrated value rather than hitting it.
    assert!(
        cast_weighted < 0.25 && cs_cast_weighted > 0.25,
        "expected 0.20 and 0.30 to BRACKET the integrated 0.25, got {cast_weighted} and {cs_cast_weighted}"
    );
    println!(
        "  contrast pins hold: 8064.96 / 134.416, cast-weighted 0.30 — 0.20 and 0.30 bracket 0.25 ✓"
    );
}
