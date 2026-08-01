//! Guide chapter 7 — Monte Carlo.
//!
//! The final chapter. Chapter 6's config, byte for byte, run in
//! `Mode::MonteCarlo` instead of `Mode::Expected`: the same average, plus
//! the shape around it. Closes with a hand-derived HARD bound — every
//! sampled fight in this config must land between two numbers you can
//! compute on paper.
//!
//! Read along: `docs/guide/07-monte-carlo.md`
//!
//! Run: `cargo run -p rtce --example guide_07_monte_carlo`

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
    let def: GameDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/07-gamedef.json"))
            .expect("valid gamedef");
    let build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/07-build.json"))
            .expect("valid build");
    let simdef: SimDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/07-simdef.json"))
            .expect("valid simdef");
    let rotation: Rotation =
        serde_json::from_str(include_str!("../tests/fixtures/guide/07-rotation.json"))
            .expect("valid rotation");
    let dummy: Scenario =
        serde_json::from_str(include_str!("../tests/fixtures/guide/07-scenario.json"))
            .expect("valid scenario");

    let plan = plan_compile(&def).expect("gamedef compiles");
    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── The EV run, unchanged from chapter 6 — the thing MC is measured
    //    against.
    let ev = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");
    assert!(close(ev.total.dps, 133.3), "ev dps: got {}", ev.total.dps);

    println!("Guide chapter 7 — Monte Carlo (60s training dummy)");
    println!("  EV mode:  {:.4} dps  (chapter 6's answer)", ev.total.dps);

    // ── The same config, sampled.
    let mc = run(
        &plan,
        &sim_plan,
        &build,
        &dummy,
        Mode::MonteCarlo {
            iterations: 1000,
            seed: 7,
        },
    )
    .expect("mc sim runs");
    let dist = mc.distribution.expect("MC mode reports a distribution");

    println!(
        "  MC mode:  mean {:.4}   std {:.4}   p10 {:.4}   p50 {:.4}   p90 {:.4}   (N=1000, seed=7)",
        dist.mean, dist.std, dist.p10, dist.p50, dist.p90
    );

    // The cast sequence never depends on the RNG here — there are no
    // procs, and stamina/cooldowns are deterministic — so every one of
    // the 1000 iterations casts exactly chapter 6's 6/31/29 and differs
    // ONLY in which hits happened to crit.
    assert_eq!(mc.actions["focus_fire"].casts, 6);
    assert_eq!(mc.actions["power_shot"].casts, 31);
    assert_eq!(mc.actions["quick_shot"].casts, 29);

    // Uptimes are timeline facts, not sampled ones — chapter 6's 0.25
    // survives the switch to MC exactly.
    assert!(
        close(mc.condition_uptime["focused"], 0.25),
        "focused uptime under MC: got {}",
        mc.condition_uptime["focused"]
    );

    // ── Distribution sanity. Loose and statistically justified rather
    //    than hand-worked: MC's exact seeded output is only reproducible
    //    by running the RNG itself, so pinning `mean` to a constant would
    //    pin the RNG's stream rather than the model. 60 quasi-independent
    //    Bernoulli(0.3) draws per iteration over 1000 iterations puts the
    //    standard error of the reported mean two orders of magnitude
    //    below the EV pin, making a 2% band effectively never flaky.
    let tolerance = 0.02 * ev.total.dps;
    assert!(
        (dist.mean - ev.total.dps).abs() < tolerance,
        "MC mean {} strayed >{tolerance} from EV dps {}",
        dist.mean,
        ev.total.dps
    );
    assert!(dist.p10 <= dist.p50, "p10 {} > p50 {}", dist.p10, dist.p50);
    assert!(dist.p50 <= dist.p90, "p50 {} > p90 {}", dist.p50, dist.p90);

    // Unlike the `poe2_*` examples — whose configs sample nothing, and
    // which therefore assert MC reproduces EV with std EXACTLY 0 — this
    // archer really does flip a coin on every hit. A zero std here would
    // mean the crit event stopped sampling.
    assert!(dist.std > 1.0, "expected real spread, got std {}", dist.std);
    println!("\n  MC pins hold: same 6/31/29 cadence, uptime still 0.25, mean within 2% of EV, real spread ✓");

    // ══════════════ the hard bound: every fight lands in here ═══════════
    //
    // Hand-worked, and much stronger than a tolerance band. In the
    // NO-CRIT branch `event_factors` is 1, so `crit_damage` — and with it
    // the whole `focused` buff — drops out entirely:
    //   no-crit  power_shot 120 × 1.55 × 1.0 × 0.8 = 148.8  (buff or not)
    //            quick_shot                        =  74.4
    // In the ALL-CRIT case the buff matters, at chapter 6's 7/24 and 5/24
    // active/inactive split:
    //   crit     power_shot active   120 × 1.55 × 2.0 × 0.8 = 297.6
    //            power_shot inactive 120 × 1.55 × 1.5 × 0.8 = 223.2
    //            quick_shot active                          = 148.8
    //            quick_shot inactive                        = 111.6
    //
    //   worst possible fight (nothing crits):
    //     31 × 148.8 + 29 × 74.4 = 4612.8 + 2157.6 = 6770.4 -> 112.84 dps
    //   best possible fight (everything crits):
    //     7 × 297.6 + 24 × 223.2 + 5 × 148.8 + 24 × 111.6
    //       = 2083.2 + 5356.8 + 744.0 + 2678.4 = 10862.4 -> 181.04 dps
    //
    // Both endpoints are astronomically unlikely (0.7^60 and 0.3^60), so
    // no percentile should come close to either — but NOTHING the sampler
    // produces may fall outside them, and the EV answer must sit between
    // them. That is a claim about the model, not about the seed.
    let floor_dps = (31.0 * 148.8 + 29.0 * 74.4) / 60.0;
    let ceil_dps = (7.0 * 297.6 + 24.0 * 223.2 + 5.0 * 148.8 + 24.0 * 111.6) / 60.0;
    assert!(close(floor_dps, 112.84), "floor: got {floor_dps}");
    assert!(close(ceil_dps, 181.04), "ceiling: got {ceil_dps}");

    println!("\n  every sampled fight must land in [{floor_dps:.2}, {ceil_dps:.2}] dps:");
    println!("    {:<20} {:>10.4}", "floor (nothing crits)", floor_dps);
    println!("    {:<20} {:>10.4}", "observed p10", dist.p10);
    println!("    {:<20} {:>10.4}", "EV / observed p50", dist.p50);
    println!("    {:<20} {:>10.4}", "observed p90", dist.p90);
    println!("    {:<20} {:>10.4}", "ceiling (all crit)", ceil_dps);

    assert!(
        dist.p10 > floor_dps && dist.p90 < ceil_dps,
        "percentiles {} / {} escaped the hard bound [{floor_dps}, {ceil_dps}]",
        dist.p10,
        dist.p90
    );
    assert!(
        ev.total.dps > floor_dps && ev.total.dps < ceil_dps,
        "EV {} escaped the hard bound",
        ev.total.dps
    );
    println!("  hard-bound pins hold: 112.84 / 181.04, and the whole distribution is inside ✓");
}
