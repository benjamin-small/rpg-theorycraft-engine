//! PoE2 slice 4 — **ignite**: the `strongest` reapply policy, where an
//! incoming ailment instance replaces the incumbent ONLY when its
//! snapshotted magnitude is strictly higher, and a losing application is
//! discarded whole — it moves neither the rate nor the expiry.
//!
//! Run: `cargo run -p rtce --example poe2_ignite`
//! Siblings: `poe2_charges` (stacking buffs), `poe2_poison` (independent
//! snapshot stacks), `poe2_triggers` (procs).
//!
//! **Scope, honestly** — as in the siblings: the `GameDef` is
//! `tests/fixtures/poe2/gamedef.json`, a PoE2-*shaped* demonstration
//! slice, not Path of Exile 2's damage model and not derived from game
//! data. Every coefficient here is `representative`, picked so the
//! arithmetic hand-derives. The real thing is
//! `../poe2-theory-crafting`'s GENERATED
//! `model/gamedef/poe2.gamedef.json` (209 pipeline stages; standing
//! reference 124.53 dps for a default Monk build). One extra liberty this slice
//! takes: the fixture has exactly ONE ailment chain, named `poison_*`,
//! and the ignite borrows it as its tick objective — the POLICY is the
//! lesson here, not the element.
//!
//! **The one thing to take away.** `strongest` is the reapply policy with
//! the sharpest edge: a LOSING application changes nothing at all. Not
//! the rate — and not the expiry either, so a weak reapplication cannot
//! keep a strong ignite alive, and even a TIE loses ("strictly higher").
//! The contrast run at the bottom shows what the same falling-power
//! timeline does under `refresh` instead, which re-captures
//! unconditionally: the window extends, and the DoT gets WEAKER.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan::compile as plan_compile;
use rtce::scenario::Scenario;
use rtce::sim::{compile as sim_compile, run, Mode, SimReport};
use rtce::simdef::{Rotation, SimDef};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn main() {
    // ── Tier 1: the algorithm, from the committed fixture ─────────────
    let gamedef_json = include_str!("../tests/fixtures/poe2/gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── Tier 2: the same chaos-scaling character `poe2_poison` uses —
    //    deliberately, so the two slices' magnitudes hand-derive from one
    //    worked chain. ──────────────────────────────────────────────────
    let build: BuildState = serde_json::from_str(
        r#"{
          "stats": {
            "weapon_avg": 100.0, "coeff_pct": 100.0,
            "crit_chance": 25.0, "crit_bonus_pct": 200.0,
            "poison_coeff_pct": 25.0, "pen_chaos": 10.0
          },
          "contributions": [
            { "bucket": "inc_phys",   "value":  50.0 },
            { "bucket": "inc_chaos",  "value": 100.0 },
            { "bucket": "more_chaos", "value":  25.0 }
          ]
        }"#,
    )
    .expect("valid build");

    // ── Tier 3: SEQUENCING. One skill, one ailment, no procs.
    //
    //    `ignite` is the mechanic:
    //      `on_reapply: strongest` — ONE incumbent instance; an incoming
    //         application replaces it only when its snapshot rate is
    //         STRICTLY higher, and a losing application is discarded
    //         whole (rate AND expiry). `sim::compile` enforces the
    //         policy's two preconditions: `snapshot: true` (the contest
    //         needs per-instance magnitudes) and `max_stacks: 1`.
    //      `tick_objective: { objective: "poison_dps", snapshot: true }` —
    //         the instance captures the objective at its own application
    //         and ticks it unchanged to expiry.
    //
    //    `ignite_strike`'s 5s cooldown against a 1s cast time puts
    //    exactly two applications in a 10s fight, completing at t=1 and
    //    t=6 (cast begins t=0, cooldown arms at cast START, so the next
    //    begin is t=5; the t=10 ready instant is the horizon, where no
    //    new cast begins). Two applications, one per phase, is the whole
    //    experiment: what each CAPTURES is set by the phase it lands in,
    //    and the 8s duration keeps the incumbent's window (expiry t=9)
    //    strictly between completions, so no expiry ever collides with a
    //    cast instant (see `sim`'s docs, "A buff expiring on the cast
    //    grid").
    let simdef_json = r#"{
      "actions": {
        "ignite_strike": {
          "cast_time": "1", "cooldown": 5.0,
          "damage": { "stats": { "coeff_pct": 200.0 } },
          "apply_buff": ["ignite"]
        }
      },
      "buffs": {
        "ignite": {
          "duration": 8.0,
          "max_stacks": 1,
          "on_reapply": "strongest",
          "tick_objective": { "objective": "poison_dps", "snapshot": true }
        }
      },
      "damage_objective": "hit"
    }"#;
    let simdef: SimDef = serde_json::from_str(simdef_json).expect("valid simdef");
    let rotation: Rotation =
        serde_json::from_str(r#"{ "rules": [ { "action": "ignite_strike" } ] }"#)
            .expect("valid rotation");
    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── Scenarios: a 10s dummy in TWO 5s phases. The `poison_coeff_pct`
    //    override is the experiment's dial: it feeds ONLY the ailment
    //    chain (`poison_base` → `poison_dps`), never the hit, so the two
    //    runs differ in what each application CAPTURES and in nothing
    //    else. 25 → R = 150/s, 50 → 2R = 300/s (derivation below).
    let two_phases = |early: f64, late: f64| -> Scenario {
        serde_json::from_str(&format!(
            r#"{{ "phases": [
              {{ "name": "early", "weight": 5,
                 "stats": {{ "enemy_res_phys": 20.0, "enemy_res_chaos": 30.0,
                             "poison_coeff_pct": {early} }} }},
              {{ "name": "late", "weight": 5,
                 "stats": {{ "enemy_res_phys": 20.0, "enemy_res_chaos": 30.0,
                             "poison_coeff_pct": {late} }} }} ] }}"#
        ))
        .expect("valid scenario")
    };
    let rising = two_phases(25.0, 50.0); // weak first, strong second
    let falling = two_phases(50.0, 25.0); // strong first, weak second
    let tie = two_phases(25.0, 25.0); // same rate twice

    let ev = |scenario: &Scenario| -> SimReport {
        run(&plan, &sim_plan, &build, scenario, Mode::Expected).expect("ev sim runs")
    };
    let dot = |r: &SimReport| r.total.total_damage - r.actions["ignite_strike"].damage;

    let r_rising = ev(&rising);
    let r_falling = ev(&falling);
    let r_tie = ev(&tie);

    println!("PoE2 ignite (P8f slice) — 10s dummy in two 5s phases, EV mode");
    println!("  scenario   captures      DoT        uptime");
    for (name, r) in [
        ("rising ", &r_rising),
        ("falling", &r_falling),
        ("tie    ", &r_tie),
    ] {
        println!(
            "  {name}    t=1 then t=6   {:9.4}   {:.4}",
            dot(r),
            r.buffs["ignite"].uptime
        );
    }

    // ══════════════════ Hand-worked pins ══════════════════════════════
    //
    // ── The two magnitudes ────────────────────────────────────────────
    // The same worked chain as `poe2_poison` (constants picked to
    // cancel): crit_mult 1.25, mit_phys 0.80 (1.25 × 0.80 = 1),
    // mit_chaos 0.80, shock_mult 1, and the chaos chain
    // (1 + 100/100) × 1.25 × 0.80 = 2.00 exactly. Under the action's
    // `coeff_pct: 200` overlay:
    //   phys_scaled = 100 × 200/100 × 1.5 = 300 → hit = 300 per cast
    //   poison_dps  = 300 × poison_coeff_pct/100 × 2.00
    //               = 150/s at the weak 25, 300/s at the strong 50
    //
    // ── The cadence ───────────────────────────────────────────────────
    // Casts begin t=0 and t=5, complete t=1 and t=6 → 2 hits × 300 =
    // 600 hit damage in EVERY run (the dial never touches the hit).
    // Each application captures under the phase it COMPLETES in: t=1 in
    // `early`, t=6 in `late`. An instance applied at `a` runs [a, a+8),
    // clipped at the 10s end.
    //
    // ── The three scenarios ───────────────────────────────────────────
    // RISING  (150 then 300): t=6's 300 > 150 → REPLACES, new window
    //   [6,14) clipped at 10.
    //     DoT = 150×(6−1) + 300×(10−6) = 750 + 1200 = 1950
    //     uptime [1,10) = 0.9
    // FALLING (300 then 150): t=6's 150 loses → DISCARDED WHOLE. The
    //   incumbent keeps its rate AND its expiry, falls off at t=9, and
    //   the last second has no ignite at all.
    //     DoT = 300 × (9−1) = 2400,   uptime [1,9) = 0.8
    // TIE     (150 then 150): "strictly higher" fails on a tie, so the
    //   t=6 application loses too — same shape as FALLING.
    //     DoT = 150 × (9−1) = 1200,   uptime 0.8
    //
    // Each pin discriminates one mutation the others do not:
    //   RISING  = 1200 → the winner did not replace either
    //   FALLING = 2700 → the loser refreshed the expiry (300 × 9)
    //   FALLING = 2100 → the loser replaced anyway (= the `refresh`
    //                    number below: `strongest` degraded to `refresh`)
    //   TIE     = 1350 → the comparison is `>=`, not `>`
    // And the uptime column carries "the expiry did NOT move" on its
    // own: 0.8 where the loser was discarded, 0.9 where a window was
    // replaced or (below) refreshed.
    for (name, r) in [
        ("rising", &r_rising),
        ("falling", &r_falling),
        ("tie", &r_tie),
    ] {
        assert_eq!(r.actions["ignite_strike"].casts, 2, "{name}: 2 casts");
        assert!(
            close(r.actions["ignite_strike"].damage, 600.0),
            "{name}: hit damage must be 2 × 300 — got {}",
            r.actions["ignite_strike"].damage
        );
    }
    assert!(
        close(dot(&r_rising), 1950.0),
        "rising DoT: got {} — want 150×5 + 300×4 = 1950 (1200 would mean \
         the stronger application did not replace)",
        dot(&r_rising)
    );
    assert!(
        close(dot(&r_falling), 2400.0),
        "falling DoT: got {} — want 300×8 = 2400, the incumbent's ORIGINAL \
         window (2700 would mean the losing application refreshed the \
         expiry; 2100 that it replaced anyway)",
        dot(&r_falling)
    );
    assert!(
        close(dot(&r_tie), 1200.0),
        "tie DoT: got {} — want 150×8 = 1200; a tie is not STRICTLY \
         higher, so the incumbent stands (1350 would mean `>=`)",
        dot(&r_tie)
    );
    assert!(
        close(r_rising.buffs["ignite"].uptime, 0.9)
            && close(r_falling.buffs["ignite"].uptime, 0.8)
            && close(r_tie.buffs["ignite"].uptime, 0.8),
        "uptimes: got {} / {} / {} — want 0.9 / 0.8 / 0.8 (a loser moves \
         no expiry, so the falling/tie windows end at t=9)",
        r_rising.buffs["ignite"].uptime,
        r_falling.buffs["ignite"].uptime,
        r_tie.buffs["ignite"].uptime
    );
    println!("\n  EV pins hold: 1950 / 2400 / 1200 DoT, uptime 0.9 / 0.8 / 0.8 ✓");

    // ══════════════ Contrast: the same timeline under `refresh` ═══════
    //
    // One key changes: `on_reapply: "refresh"` (and nothing else — the
    // string replace below is the whole diff, asserted by the fact that
    // the RISING number does not move). `refresh` re-captures
    // UNCONDITIONALLY, so on the falling timeline the weak t=6
    // application replaces the strong incumbent: the window EXTENDS
    // (uptime 0.8 → 0.9) while the DoT gets WEAKER —
    //   DoT = 300×(6−1) + 150×(10−6) = 1500 + 600 = 2100  (< 2400)
    // which is exactly the trade `strongest` exists to refuse. On the
    // rising timeline the two policies agree (the incoming rate wins
    // either way): 1950 both — pinned as the control.
    let refresh_json = simdef_json.replace("\"strongest\"", "\"refresh\"");
    assert_ne!(refresh_json, simdef_json, "the replace must change one key");
    let refresh_simdef: SimDef = serde_json::from_str(&refresh_json).expect("valid simdef");
    let refresh_plan = sim_compile(&plan, &refresh_simdef, &rotation).expect("simdef compiles");
    let rf = run(&plan, &refresh_plan, &build, &falling, Mode::Expected).expect("ev sim runs");
    let rr = run(&plan, &refresh_plan, &build, &rising, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  under `refresh` instead: falling DoT {:.4} (uptime {:.4}), \
         rising DoT {:.4}",
        dot(&rf),
        rf.buffs["ignite"].uptime,
        dot(&rr)
    );
    assert!(
        close(dot(&rf), 2100.0),
        "refresh falling DoT: got {} — want 300×5 + 150×4 = 2100: the \
         re-capture in a weaker moment LOWERS the DoT, the opposite of \
         `strongest`",
        dot(&rf)
    );
    assert!(
        close(rf.buffs["ignite"].uptime, 0.9),
        "refresh falling uptime: got {} — want 0.9: the reapplication DOES \
         move the expiry here, which is exactly what `strongest` denied \
         the loser",
        rf.buffs["ignite"].uptime
    );
    assert!(
        close(dot(&rr), 1950.0),
        "refresh rising DoT: got {} — the control: when the incoming rate \
         is higher the two policies agree at 1950",
        dot(&rr)
    );
    println!(
        "  contrast pins hold: 2400 (strongest) vs 2100 (refresh) on the same falling timeline ✓"
    );

    // ── Monte Carlo: exact agreement, the siblings' gate ──────────────
    //
    // Nothing in this config samples (closed-form crit, no procs, a
    // deterministic `apply_buff`), and a snapshot capture is EV-blended
    // in both modes by design — so MC must reproduce EV to the BIT, with
    // zero spread. This is also the first Monte Carlo coverage the
    // `strongest` policy has had at all: the win/lose comparison runs
    // against branch-blended captures, never against sampled ones, so
    // WHICH instance wins can never depend on the seed.
    let mc = run(
        &plan,
        &sim_plan,
        &build,
        &falling,
        Mode::MonteCarlo {
            iterations: 64,
            seed: 11,
        },
    )
    .expect("mc sim runs");
    let dist = mc.distribution.expect("MC mode reports a distribution");
    println!(
        "\nMonte Carlo (N=64, seed=11), falling: mean {:.4}  std {:.4}",
        dist.mean, dist.std
    );
    assert!(close(dist.mean, 300.0), "MC mean {}", dist.mean);
    assert!(close(dist.std, 0.0), "MC std {}", dist.std);
    assert!(
        close(mc.total.total_damage, r_falling.total.total_damage),
        "MC total {} vs EV {}",
        mc.total.total_damage,
        r_falling.total.total_damage
    );
    assert!(
        close(mc.buffs["ignite"].uptime, 0.8),
        "MC uptime {}",
        mc.buffs["ignite"].uptime
    );
    println!("  MC reproduces EV exactly (std 0) ✓");
}
