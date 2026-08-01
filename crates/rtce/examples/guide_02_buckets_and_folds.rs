//! Guide chapter 2 — buckets and folds.
//!
//! Chapter 1's archer had nowhere to put a modifier. This chapter adds
//! the `additive` bucket and a build that contributes to it, then runs
//! the SAME two contributions through all three fold rules side by side
//! as a contrast — the fold is the config decision that says whether
//! "+30% and +25%" means +55% or ×1.625.
//!
//! Read along: `docs/guide/02-buckets-and-folds.md`
//!
//! Run: `cargo run -p rtce --example guide_02_buckets_and_folds`

use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

fn main() {
    // ── The archer, now with an `additive` pool (fold `sum`) that the
    //    pipeline wraps itself: `1 + additive / 100`.
    let def: GameDef =
        serde_json::from_str(include_str!("../tests/fixtures/guide/02-gamedef.json"))
            .expect("valid gamedef");
    let build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/02-build.json"))
            .expect("valid build");
    let scenario: Scenario =
        serde_json::from_str(include_str!("../tests/fixtures/guide/02-scenario.json"))
            .expect("valid scenario");

    let plan = plan::compile(&def).expect("gamedef compiles");
    let mut scratch = plan.scratch();
    let objectives = plan
        .evaluate(&build, &scenario, &mut scratch)
        .expect("evaluates");

    println!("Guide chapter 2 — buckets and folds");
    println!("  hit = {:.4}", objectives[0]);

    // ── Hand-worked pin: the `additive` bucket folds `sum`, so its two
    //    members give Σv = 30 + 25 = 55. The pipeline wraps that itself:
    //    hit = 120 × (1 + 55/100) = 120 × 1.55 = 186.
    assert!(
        (objectives[0] - 186.0).abs() < 1e-9,
        "got {}",
        objectives[0]
    );
    println!("  pin holds: 186 ✓");

    // ══════════════════ contrast: the same +30 and +25, three folds ═════
    //
    // Three buckets, identical members, one config word different each
    // time. This is the whole point of the fold rule, run rather than
    // asserted in prose.
    let folds_def: GameDef = serde_json::from_str(include_str!(
        "../tests/fixtures/guide/02-gamedef-folds.json"
    ))
    .expect("valid gamedef");
    let folds_build: BuildState =
        serde_json::from_str(include_str!("../tests/fixtures/guide/02-build-folds.json"))
            .expect("valid build");

    let folds_plan = plan::compile(&folds_def).expect("gamedef compiles");
    let mut folds_scratch = folds_plan.scratch();
    let folds = folds_plan
        .evaluate(&folds_build, &scenario, &mut folds_scratch)
        .expect("evaluates");

    println!("\n  the same +30 and +25, through each fold:");
    println!("    {:<16} {:>10} {:>10}", "fold", "bucket", "hit");
    println!("    {:<16} {:>10.4} {:>10.4}", "sum", 55.0, folds[0]);
    println!(
        "    {:<16} {:>10.4} {:>10.4}",
        "summed_group", 1.55, folds[1]
    );
    println!("    {:<16} {:>10.4} {:>10.4}", "product", 1.625, folds[2]);

    // ── Hand-worked pins for the contrast:
    //      sum          Σv               = 30 + 25          = 55
    //                   wrapped by the pipeline: 120 × 1.55 = 186
    //      summed_group 1 + Σv/100       = 1 + 55/100       = 1.55
    //                   already a factor:        120 × 1.55 = 186
    //      product      Π(1 + v/100)     = 1.30 × 1.25      = 1.625
    //                   already a factor:       120 × 1.625 = 195
    //    `sum` and `summed_group` AGREE here — they are the same rule,
    //    differing only in who applies the `1 + x/100` wrap. `product`
    //    does not, and the 9-point gap is exactly the cross term
    //    (0.30 × 0.25 × 120 = 9).
    assert!((folds[0] - 186.0).abs() < 1e-9, "sum: got {}", folds[0]);
    assert!(
        (folds[1] - 186.0).abs() < 1e-9,
        "summed_group: got {}",
        folds[1]
    );
    assert!((folds[2] - 195.0).abs() < 1e-9, "product: got {}", folds[2]);
    assert!(
        (folds[2] - folds[0] - 9.0).abs() < 1e-9,
        "cross term: got {}",
        folds[2] - folds[0]
    );
    println!("\n  contrast pins hold: 186 / 186 / 195, the 9-point gap is the cross term ✓");
}
