//! PoE2 slice 1 of 3 — **frenzy charges**: a buff that STACKS to a cap,
//! whose whole stack shares one expiry clock, whose duration is an
//! EXPRESSION reading its own stack count, and whose count gates a
//! rotation rule.
//!
//! Run: `cargo run -p rtce --example poe2_charges`
//! Siblings: `poe2_poison` (snapshot ailments), `poe2_triggers` (procs).
//!
//! **Scope, honestly** — the same disclaimer the D4 slices carry, and it
//! applies twice over here. The `GameDef` is
//! `tests/fixtures/poe2/gamedef.json`, a PoE2-*shaped* demonstration
//! slice: two damage types with their own scaling chains, PoE2's
//! `increased`/`more` split, per-type resistance and penetration, an
//! ailment as a condition. It is NOT Path of Exile 2's damage model and
//! NOT derived from game data — every coefficient below is
//! `representative`, picked so the arithmetic hand-derives. The real thing
//! is `../poe2-calcs`' GENERATED `gamedef/poe2.gamedef.json` (67 stats /
//! 73 buckets / 209 stages / 80 objectives; standing reference 124.53 dps
//! for a default Monk build); a 209-stage pipeline is not hand-derivable,
//! which is the whole reason this fixture is trimmed. The `SimDef` is
//! likewise a demonstration cadence, not PoE2 skill data.
//!
//! The three mechanics on display, each pinned below:
//!   - `on_reapply: add_refresh_all` + `max_stacks: 3` — PoE2 charges
//!   - `duration: "4.5 + stacks.frenzy_charge"` — P7b, read at application
//!   - `when: "stacks.frenzy_charge >= 3"` — the counted symbol as strategy

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile as plan_compile;
use rtce::scenario::Scenario;
use rtce::sim::{compile as sim_compile, run, Mode};
use rtce::simdef::{NumOrExpr, Rotation, SimDef};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn main() {
    // ── Tier 1: the algorithm, from the committed fixture ─────────────
    let gamedef_json = include_str!("../tests/fixtures/poe2/gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── Tier 2: one candidate character. `coeff_pct` is here only as a
    //    base — every damaging action below overrides it per cast via
    //    `damage.stats`, which is how a skill's damage effectiveness
    //    enters. Note there is NO `more_global` contribution: that bucket
    //    belongs to the charges alone, so whatever it folds to IS the
    //    stack count's effect and nothing else. ────────────────────────
    let build: BuildState = serde_json::from_str(
        r#"{
          "stats": {
            "weapon_avg": 100.0, "coeff_pct": 100.0,
            "crit_chance": 25.0, "crit_bonus_pct": 200.0,
            "poison_coeff_pct": 0.0, "pen_chaos": 0.0
          },
          "contributions": [
            { "bucket": "inc_phys", "value": 50.0 }
          ]
        }"#,
    )
    .expect("valid build");

    // ── Tier 3: SEQUENCING. Two skills and one charge buff — no
    //    resources and no procs at all.
    //
    //    `frenzy` is the GENERATOR: it hits for a little and carries
    //    `apply_buff: ["frenzy_charge"]`, so a charge is an effect OF the
    //    skill, landing at its cast-complete instant (P7d).
    //    `spender` is the payoff, at 3.3× frenzy's damage effectiveness,
    //    and the rotation only reaches for it at full charges.
    //
    //    `frenzy_charge` is the mechanic:
    //      `max_stacks: 3` + `add_refresh_all` — count up to 3, and on
    //         EVERY application all live instances' expiries reset to
    //         `now + duration`. One shared clock, so the whole stack falls
    //         off together: PoE2 charges, and NOT what `add_independent`
    //         (the poison policy — see `poe2_poison`) would do.
    //      `duration` is an EXPRESSION over sim state (P7b), evaluated at
    //         each application against the count BEFORE it. Chosen
    //         (`representative`) to make the shared clock VISIBLE: the
    //         three applications ask for 4.5s, 5.5s and 6.5s in turn, so a
    //         per-instance clock and a shared one land on measurably
    //         different expiries. The half-seconds are deliberate too, and
    //         load-bearing: they keep every expiry instant strictly BETWEEN
    //         cast completions. A `BuffExpire` sharing an instant with a
    //         `CastComplete` was scheduled earlier, so it carries the lower
    //         `seq` and resolves FIRST — the stack is already gone when the
    //         rule `when` and the damage measurement read it. At a flat
    //         `"4 + stacks.frenzy_charge"` that alone reshapes the whole
    //         cycle (15 frenzy / 25 spender instead of 12 / 28), which
    //         would make this slice a lesson in event ordering rather than
    //         in the shared clock. NOT a fight-horizon concern: a cast
    //         completing at exactly `duration` counts regardless (see
    //         `Sim::run_loop`'s "horizon rule").
    //      one contribution of `+10` to `more_global`, a `product` bucket.
    //         Per-stack, the VALUE is scaled by the live count, so 3
    //         charges fold as ×(1 + 3·10/100) = ×1.30 — read the pin
    //         comment before assuming ×1.10³.
    let simdef_json = r#"{
      "actions": {
        "frenzy": {
          "cast_time": "1", "cooldown": 0.0,
          "damage": { "stats": { "coeff_pct": 60.0 } },
          "apply_buff": ["frenzy_charge"]
        },
        "spender": {
          "cast_time": "1", "cooldown": 0.0,
          "damage": { "stats": { "coeff_pct": 200.0 } }
        }
      },
      "buffs": {
        "frenzy_charge": {
          "duration": "4.5 + stacks.frenzy_charge",
          "max_stacks": 3,
          "on_reapply": "add_refresh_all",
          "contributions": [ { "bucket": "more_global", "value": 10.0 } ]
        }
      },
      "damage_objective": "hit"
    }"#;
    let simdef: SimDef = serde_json::from_str(simdef_json).expect("valid simdef");

    // Priority rotation: spend at full charges, otherwise generate.
    // `stacks.frenzy_charge` is the COUNTED symbol; `buff.frenzy_charge`
    // is binary (0 or 1) and `>= 3` would never be true with it. That is
    // not a hypothetical — it is run and pinned as a contrast below.
    let rotation_json = r#"{ "rules": [
      { "action": "spender", "when": "stacks.frenzy_charge >= 3" },
      { "action": "frenzy" }
    ]}"#;
    let rotation: Rotation = serde_json::from_str(rotation_json).expect("valid rotation");

    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── Scenario: a 40s dummy with 20% physical resistance. No `shocked`
    //    uptime — this slice has no shock at all (see `poe2_triggers`).
    let dummy: Scenario = serde_json::from_str(
        r#"{ "phases": [ { "name": "dummy", "weight": 40,
              "stats": { "enemy_res_phys": 20.0, "enemy_res_chaos": 0.0 } } ] }"#,
    )
    .expect("valid scenario");

    let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!("PoE2 charges (P7e slice 1) — 40s dummy, EV mode");
    println!(
        "  {:<10} {:>6} {:>13} {:>8}",
        "action", "casts", "damage", "share"
    );
    for (name, a) in &report.actions {
        println!(
            "  {name:<10} {:>6} {:>13.4} {:>7.2}%",
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
        "  frenzy_charge: uptime {:.4}, avg_stacks {:.4}",
        report.buffs["frenzy_charge"].uptime, report.buffs["frenzy_charge"].avg_stacks
    );

    // ══════════════════ Hand-worked pins ══════════════════════════════
    //
    // ── The per-hit value ─────────────────────────────────────────────
    // Every factor outside `more_global` is constant here, and the
    // `representative` constants were picked so they cancel:
    //   crit_mult  = 1 + clamp(25,0,100)/100 × (200/100 − 1) = 1.25
    //   mit_phys   = max(0, 1 − 20/100)                      = 0.80
    //   shock_mult = 1 + 0 × 0.20                            = 1.00
    //   1.25 × 0.80 × 1.00 = 1.00, exactly.
    // So with `weapon_avg` = 100 and `inc_phys` = +50,
    //   hit = 100 × coeff_pct/100 × (1 + 50/100) × more_global × 1.25
    //             × 1.00 × 0.80
    //       = coeff_pct × 1.5 × more_global.
    //
    // `more_global` is a PRODUCT bucket holding exactly one contribution,
    // the charge's `+10`, whose VALUE is scaled by the live stack count
    // (see `simdef::BuffDef`): n charges fold as Π(1 + v/100) over ONE
    // member of value 10n, i.e. ×(1 + 0.10·n) — NOT ×1.10ⁿ.
    //   n = 0 → 1.00   n = 1 → 1.10   n = 2 → 1.20   n = 3 → 1.30
    //
    // ⚠ That is rtce's per-stack model, and it LINEARIZES what PoE2 calls
    // a "more" multiplier: three real frenzy charges at 10% more each
    // would be ×1.331, and the pins below say ×1.30. A genuinely
    // multiplicative per-charge effect is NOT expressible as one per-stack
    // contribution. Write it as "increased damage per charge" instead (a
    // `sum` bucket, where linear IS the correct fold), or take the
    // linearization knowingly. The 1.30 is pinned precisely so this stays
    // a stated choice rather than a silent one.
    //
    //   frenzy  (coeff  60): 60 × 1.5 = 90 at 0 charges
    //     → 90 / 99 / 108 at 0 / 1 / 2 charges
    //   spender (coeff 200): 200 × 1.5 = 300 at 0 charges
    //     → 390 at 3 charges
    //
    // ── The cadence ───────────────────────────────────────────────────
    // Both skills cast in exactly 1s and cost nothing, so decisions land
    // on every integer second (t = 0…39) and completions on t = 1…40.
    // Charges are applied at frenzy's COMPLETION, after its own damage is
    // measured (`sim`'s cast-complete order) — a frenzy never benefits
    // from the charge it generates.
    //
    //   t=0  0 charges → frenzy. Completes t=1: damage at 0 charges (90),
    //        then application #1. `duration` reads `stacks` BEFORE this
    //        application = 0 → 4.5 → this instance expires at 1 + 4.5 = 5.5.
    //   t=1  1 charge  → frenzy. Completes t=2: damage at 1 charge (99),
    //        then application #2. `stacks` = 1 → duration 5.5, and
    //        `add_refresh_all` resets EVERY instance to 2 + 5.5 = 7.5 —
    //        the first charge's 5.5 is overwritten.
    //   t=2  2 charges → frenzy. Completes t=3: damage at 2 (108), then
    //        application #3. `stacks` = 2 → duration 6.5, ALL THREE reset
    //        to 3 + 6.5 = 9.5.
    //   t=3…9  3 charges → spender (7 casts), completing t=4…10.
    //   t=9.5  all three expire together, mid-cast — the count drops
    //        straight from 3 to 0.
    //   t=10 the spender that started at t=9 completes with ZERO charges
    //        (300, not 390), and the decision at t=10 starts the next
    //        cycle with frenzy.
    //
    // The cycle is therefore exactly 10s wide — 3 frenzy + 7 spender — and
    // 40s is 4 whole cycles: 12 frenzy, 28 spender.
    //
    // Note what the shared clock did. With `add_independent` the three
    // instances would have kept the expiries 5.5, 7.5 and 9.5 they were
    // each born with, the count would have fallen below 3 at t=5.5, and
    // the rotation would have gone back to generating four seconds early.
    // The 10s cycle IS the shared clock; the three different durations
    // (4.5/5.5/6.5) are what make that visible instead of a coincidence.
    //
    // ── The numbers ───────────────────────────────────────────────────
    // Per 10s cycle:
    //   frenzy  = 90 + 99 + 108                      =  297
    //   spender = 6 × 390 (charged) + 1 × 300 (bare) = 2640
    //   cycle                                        = 2937
    //   × 4 cycles → 11748 ; dps = 11748 / 40 = 293.7
    //   frenzy total  = 4 × 297  =  1188
    //   spender total = 4 × 2640 = 10560
    //
    // Stacks integrated over one cycle [0,10):
    //   [0,1) 0 · [1,2) 1 · [2,3) 2 · [3,9.5) 3 · [9.5,10) 0
    //   ∫ = 0 + 1 + 2 + 3×6.5 + 0 = 22.5  →  4 cycles = 90
    //   avg_stacks = 90 / 40 = 2.25
    // Uptime is "≥1 charge live" = [1,9.5) = 8.5s per cycle:
    //   uptime = 4 × 8.5 / 40 = 0.85
    assert_eq!(report.actions["frenzy"].casts, 12);
    assert_eq!(report.actions["spender"].casts, 28);
    assert!(
        close(report.actions["frenzy"].damage, 1188.0),
        "got {}",
        report.actions["frenzy"].damage
    );
    assert!(
        close(report.actions["spender"].damage, 10560.0),
        "got {}",
        report.actions["spender"].damage
    );
    assert!(
        close(report.total.total_damage, 11748.0),
        "got {}",
        report.total.total_damage
    );
    assert!(close(report.total.dps, 293.7), "got {}", report.total.dps);
    assert!(
        close(report.buffs["frenzy_charge"].avg_stacks, 2.25),
        "got {}",
        report.buffs["frenzy_charge"].avg_stacks
    );
    assert!(
        close(report.buffs["frenzy_charge"].uptime, 0.85),
        "got {}",
        report.buffs["frenzy_charge"].uptime
    );
    println!("\n  EV pins hold: 11748 total / 293.7 dps / 2.25 avg_stacks / 0.85 uptime ✓");

    // ── Contrast: the counted symbol vs the binary one ────────────────
    //
    // "`stacks.X` is the COUNT, `buff.X` is 0-or-1" is worth exactly as
    // much as a run that shows the difference. Same config, one identifier
    // changed in the rotation's `when` — and the spender becomes
    // unreachable, because `buff.frenzy_charge` reads 1 at three charges
    // just as it does at one, and 1 >= 3 is false.
    //
    // Cadence then: frenzy every second, 40 casts, completing t=1…40, one
    // charge application per completion.
    //   applications #1/#2/#3 at t=1/2/3 as above (durations 4.5/5.5/6.5);
    //   from #4 on the buff is AT THE CAP, so `add_refresh_all` adds no
    //   instance — but it still evaluates `duration` (stacks = 3 → 7.5)
    //   and still resets the shared clock. The last application is at
    //   t=40, pushing expiry to 47.5, so the stack never falls off inside
    //   the fight.
    // Frenzy hits: 90 (0 charges), 99 (1), 108 (2), then 117 for the
    // remaining 37 casts — 117 = 60 × 1.5 × 1.30, the spender's ×1.30 at
    // the generator's own damage effectiveness.
    //   total = 90 + 99 + 108 + 37 × 117 = 297 + 4329 = 4626
    //   ∫ stacks = 0 + 1 + 2 + 3 × 37 = 114 → avg_stacks = 114/40 = 2.85
    //   uptime = 39/40 = 0.975
    let binary_rotation: Rotation = serde_json::from_str(
        r#"{ "rules": [
          { "action": "spender", "when": "buff.frenzy_charge >= 3" },
          { "action": "frenzy" }
        ]}"#,
    )
    .expect("valid rotation");
    let binary_plan = sim_compile(&plan, &simdef, &binary_rotation).expect("simdef compiles");
    let binary = run(&plan, &binary_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  with `buff.frenzy_charge >= 3` instead: {} spender casts, {:.4} total, \
         {:.4} avg_stacks",
        binary.actions["spender"].casts,
        binary.total.total_damage,
        binary.buffs["frenzy_charge"].avg_stacks
    );
    assert_eq!(
        binary.actions["spender"].casts, 0,
        "`buff.X` is binary — it can never reach 3"
    );
    assert_eq!(binary.actions["frenzy"].casts, 40);
    assert!(
        close(binary.total.total_damage, 4626.0),
        "got {}",
        binary.total.total_damage
    );
    assert!(
        close(binary.buffs["frenzy_charge"].avg_stacks, 2.85),
        "got {}",
        binary.buffs["frenzy_charge"].avg_stacks
    );
    assert!(
        close(binary.buffs["frenzy_charge"].uptime, 0.975),
        "got {}",
        binary.buffs["frenzy_charge"].uptime
    );
    println!("  contrast pins hold: 0 spender casts / 4626 total / 2.85 avg_stacks ✓");

    // ═══ Contrast: the charge clock ON the cast grid — ∫ goes blind ═══
    //
    // Cited by `sim`'s "a buff expiring on the cast grid" section and by
    // the 0.4.0 ordering question in ROADMAP; pinned here so those numbers
    // cannot rot. This is the sharper of the two illustrations — the other
    // is `poe2_triggers`, where damage moves 15.5% at an unchanged uptime.
    //
    // Drop the half-second: `"4 + stacks.frenzy_charge"`. The three
    // applications now ask 4s, 5s, 6s, so the shared clock lands on
    // 3 + 6 = t=9 — an integer, i.e. exactly a cast instant. The
    // `BuffExpire` was scheduled back at t=3 and the `CastComplete` at
    // t=8, so the expiry holds the lower `seq` and resolves FIRST.
    //
    // Re-derived cycle (9s, vs 10s above):
    //   t=1,2,3   frenzy ×3, applications ask 4/5/6 → all reset to t=9
    //   t=4…8     spender ×5, charged                   → 5 × 390 = 1950
    //   t=9       the stacks expire FIRST, so this spender — chosen at
    //             t=8, when the count was still 3 — lands BARE →    300
    //   t=9       the count is now 0, so the rotation generates again
    //   per cycle: 3 frenzy (90 + 99 + 108 = 297) + 6 spender (2250)
    //
    // 40s is 4 whole cycles (t=0…36) plus a 4-cast tail (frenzy at
    // t=36,37,38, then one CHARGED spender completing at t=40 — on the
    // horizon, and it counts; see `sim`'s "fight horizon"):
    //   frenzy  = 5 × 297           =  1485   (15 casts, vs 12)
    //   spender = 4 × 2250 + 390    =  9390   (25 casts, vs 28)
    //   total   =                     10875   (vs 11748)
    //
    // THE POINT — `avg_stacks` cannot see any of it:
    //   ∫ stacks dt per cycle = 0 + 1 + 2 + 3×6   =  21
    //   4 cycles + tail (0 + 1 + 2 + 3)           =  84 + 6 = 90
    //   avg_stacks = 90 / 40                      =  2.25
    // EXACTLY the 2.25 of the 4.5s build above, to the last bit, while
    // 7.4% of the damage and three casts' worth of rotation shape have
    // moved. `uptime` does budge here (0.85 → 0.875 — the gap is a full
    // second, not zero-width); it is `avg_stacks`, the measurement this
    // slice is actually about, that goes blind.
    let on_grid: SimDef = serde_json::from_str(&simdef_json.replace(
        r#""4.5 + stacks.frenzy_charge""#,
        r#""4 + stacks.frenzy_charge""#,
    ))
    .expect("valid simdef");
    assert!(
        matches!(&on_grid.buffs["frenzy_charge"].duration,
                 NumOrExpr::Expr(e) if e == "4 + stacks.frenzy_charge"),
        "the contrast must actually move the charge clock onto the grid"
    );
    let on_grid_plan = sim_compile(&plan, &on_grid, &rotation).expect("simdef compiles");
    let grid = run(&plan, &on_grid_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  with `\"4 + stacks\"` (clock on the cast grid): {} frenzy / {} spender, \
         {:.4} total — avg_stacks STILL {:.4}",
        grid.actions["frenzy"].casts,
        grid.actions["spender"].casts,
        grid.total.total_damage,
        grid.buffs["frenzy_charge"].avg_stacks
    );
    assert_eq!(grid.actions["frenzy"].casts, 15);
    assert_eq!(grid.actions["spender"].casts, 25);
    assert!(
        close(grid.actions["frenzy"].damage, 1485.0)
            && close(grid.actions["spender"].damage, 9390.0),
        "frenzy {} spender {}",
        grid.actions["frenzy"].damage,
        grid.actions["spender"].damage
    );
    assert!(
        close(grid.total.total_damage, 10875.0),
        "got {}",
        grid.total.total_damage
    );
    // The blind integral — identical to the 4.5s run, bit for bit.
    assert!(
        close(grid.buffs["frenzy_charge"].avg_stacks, 2.25),
        "avg_stacks must be unchanged: got {}",
        grid.buffs["frenzy_charge"].avg_stacks
    );
    assert_eq!(
        grid.buffs["frenzy_charge"].avg_stacks, report.buffs["frenzy_charge"].avg_stacks,
        "the two builds must agree on avg_stacks EXACTLY — that is the point"
    );
    println!("  footgun pins hold: 11748 → 10875 at IDENTICAL 2.25 avg_stacks ✓");

    // ── Monte Carlo ───────────────────────────────────────────────────
    //
    // This gamedef has no `events` block (crit is closed-form — see the
    // fixture's `_crit` note), the rotation is gated only on deterministic
    // state, and there are no procs, so there is NOTHING here for Monte
    // Carlo to sample: every iteration replays the identical timeline. The
    // honest assertion is therefore EXACT equality plus a ZERO spread, not
    // a tolerance band — and it is a real assertion, because it fails the
    // moment an RNG draw appears on a path that must stay deterministic
    // (buff application, `apply_buff`, or the rotation's own decisions).
    let mc = run(
        &plan,
        &sim_plan,
        &build,
        &dummy,
        Mode::MonteCarlo {
            iterations: 64,
            seed: 11,
        },
    )
    .expect("mc sim runs");
    let dist = mc.distribution.expect("MC mode reports a distribution");
    println!(
        "\nMonte Carlo (N=64, seed=11): mean {:.4}  std {:.4}  p10 {:.4}  p90 {:.4}",
        dist.mean, dist.std, dist.p10, dist.p90
    );
    assert!(
        close(dist.mean, 293.7),
        "MC mean {} — want the EV pin",
        dist.mean
    );
    assert!(
        close(dist.std, 0.0),
        "MC std {} — nothing in this config samples",
        dist.std
    );
    assert!(
        close(mc.buffs["frenzy_charge"].avg_stacks, 2.25),
        "MC avg_stacks {}",
        mc.buffs["frenzy_charge"].avg_stacks
    );
    println!("  MC reproduces EV exactly (std 0) ✓");
}
