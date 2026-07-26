//! PoE2 slice 3 of 3 — **a trigger setup**: a meta-gem that free-casts a
//! secondary skill, but only off ONE of the primary skills, while that
//! primary applies an ailment of its own — P7d's two action-scoped
//! features end to end.
//!
//! Run: `cargo run -p rtce --example poe2_triggers`
//! Siblings: `poe2_charges` (stacking buffs), `poe2_poison` (ailments).
//!
//! **Scope, honestly** — as in the sibling slices: the `GameDef` is
//! `tests/fixtures/poe2/gamedef.json`, a PoE2-*shaped* demonstration
//! slice, not Path of Exile 2's damage model and not derived from game
//! data. Every coefficient here is `representative`, picked so the
//! arithmetic hand-derives. The real thing is `../poe2-calcs`' GENERATED
//! `gamedef/poe2.gamedef.json` (209 pipeline stages; standing reference
//! 124.53 dps for a default Monk build).
//!
//! The three things on display, each pinned below:
//!   - `ProcDef::actions` — the trigger only considers `bolt`'s casts.
//!     Deleting the filter is run as a contrast, not merely described.
//!   - `ActionDef::apply_buff` on a primary — `bolt` shocks the enemy.
//!   - a proc FREE CAST applying its own action's `apply_buff` — `comet`
//!     is never in the rotation, yet its `power_surge` drives most of the
//!     fight's damage.

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
    // ── Tier 1: the algorithm, from the committed fixture ─────────────
    let gamedef_json = include_str!("../tests/fixtures/poe2/gamedef.json");
    let def: GameDef = serde_json::from_str(gamedef_json).expect("valid gamedef");
    let plan = plan_compile(&def).expect("gamedef compiles");

    // ── Tier 2: a physical attacker. As in the sibling slices there is
    //    NO `more_global` contribution on the build — that bucket belongs
    //    entirely to the triggered skill's `power_surge`, so whatever it
    //    folds to is that buff's doing and nothing else. ──────────────
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

    // ── Tier 3: SEQUENCING. Three actions, two buffs, one proc.
    //
    //    `bolt`  — the primary. 2s cooldown, and `apply_buff: ["shock"]`,
    //              so every bolt shocks (P7d: an ailment as an effect OF
    //              the skill, at its cast-complete instant).
    //    `slam`  — the filler the rotation falls through to. It exists so
    //              the trigger filter has something to EXCLUDE.
    //    `comet` — the triggered secondary. It is in no rotation rule at
    //              all; the only thing that ever casts it is the proc. Its
    //              `cast_time` is irrelevant on that path (a free cast
    //              begins and completes at the firing proc's instant) and
    //              is written `"0"` to say so. It carries its OWN
    //              `apply_buff: ["power_surge"]`, which a free cast
    //              applies — an effect OF the action, not part of the cast
    //              PIPELINE (cost, cooldown, further proc rolls) that the
    //              free-cast path skips.
    //
    //    `trigger_gem` is the mechanic: `on_cast`, `chance: "1"` with a 3s
    //    internal cooldown (a PoE2 meta-gem is a deterministic trigger on
    //    a timer, not a lucky-hit roll — and a certain proc keeps the pins
    //    exact), and `actions: ["bolt"]` so a slam's cast is not an event
    //    for it at all.
    //
    //    Both durations are half-integer `representative` values, which
    //    keeps every expiry strictly between cast completions so no pin
    //    leans on a same-instant tie-break.
    let simdef_json = r#"{
      "actions": {
        "bolt": {
          "cast_time": "1", "cooldown": 2.0,
          "damage": { "stats": { "coeff_pct": 100.0 } },
          "apply_buff": ["shock"]
        },
        "slam": {
          "cast_time": "1", "cooldown": 0.0,
          "damage": { "stats": { "coeff_pct": 150.0 } }
        },
        "comet": {
          "cast_time": "0", "cooldown": 0.0,
          "damage": { "stats": { "coeff_pct": 400.0 } },
          "apply_buff": ["power_surge"]
        }
      },
      "buffs": {
        "shock":       { "duration": 2.5, "conditions": { "shocked": 1.0 } },
        "power_surge": { "duration": 4.5,
                         "contributions": [ { "bucket": "more_global", "value": 25.0 } ] }
      },
      "procs": {
        "trigger_gem": { "trigger": "on_cast", "chance": "1", "icd": 3.0,
                         "actions": ["bolt"], "cast_action": "comet" }
      },
      "damage_objective": "hit"
    }"#;
    let simdef: SimDef = serde_json::from_str(simdef_json).expect("valid simdef");

    // Priority rotation: bolt whenever it is off cooldown (the hard gate
    // alone says "when ready"), else slam. `comet` appears nowhere.
    let rotation: Rotation =
        serde_json::from_str(r#"{ "rules": [ { "action": "bolt" }, { "action": "slam" } ] }"#)
            .expect("valid rotation");
    let sim_plan = sim_compile(&plan, &simdef, &rotation).expect("simdef compiles");

    // ── Scenario: a 20s dummy with 20% physical resistance. NO static
    //    `shocked` uptime — every point of it below is COMPUTED from
    //    bolt's buff window.
    let dummy: Scenario = serde_json::from_str(
        r#"{ "phases": [ { "name": "dummy", "weight": 20,
              "stats": { "enemy_res_phys": 20.0, "enemy_res_chaos": 0.0 } } ] }"#,
    )
    .expect("valid scenario");

    let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!("PoE2 triggers (P7e slice 3) — 20s dummy, EV mode");
    println!(
        "  {:<8} {:>6} {:>12} {:>8}",
        "action", "casts", "damage", "share"
    );
    for (name, a) in &report.actions {
        println!(
            "  {name:<8} {:>6} {:>12.4} {:>7.2}%",
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
        "  shock uptime {:.4} (shocked condition {:.4}), power_surge uptime {:.4}",
        report.buffs["shock"].uptime,
        report.condition_uptime["shocked"],
        report.buffs["power_surge"].uptime
    );

    // ══════════════════ Hand-worked pins ══════════════════════════════
    //
    // ── The per-hit value ─────────────────────────────────────────────
    // Constant factors (`representative`, picked to cancel):
    //   crit_mult = 1 + 25/100 × (200/100 − 1) = 1.25
    //   mit_phys  = max(0, 1 − 20/100)         = 0.80   (1.25 × 0.80 = 1)
    // So with `weapon_avg` = 100 and `inc_phys` = +50,
    //   hit = coeff_pct × 1.5 × more_global × shock_mult
    // where `more_global` = 1.25 while `power_surge` is live (a single
    // `+25` contribution in a product bucket, one instance) and
    // `shock_mult` = 1 + 0.20 = 1.20 while `shocked` is driven.
    //
    //   bolt  (coeff 100): 150 bare · 225 shocked+surged
    //   slam  (coeff 150): 225 bare · 337.5 shocked+surged
    //   comet (coeff 400): 600 bare · 720 shocked · 900 shocked+surged
    //
    // ── The cadence ───────────────────────────────────────────────────
    // Both rotation actions cast in 1s and cost nothing, so decisions land
    // on every integer second (t = 0…19) and completions on t = 1…20.
    // Bolt's 2s cooldown starts at CAST START, so:
    //   t=0  bolt (ready again at t=2) → completes t=1
    //   t=1  bolt has 1s left → slam    → completes t=2
    //   t=2  bolt ready → bolt          → completes t=3   …and so on.
    // Bolt casts on every EVEN t (10 of them, completing on odd t=1…19);
    // slam on every ODD t (10, completing on even t=2…20).
    //
    // The trigger rolls at cast complete, considers ONLY bolt (so the ten
    // slam completions are not events for it), and its 3s ICD then thins
    // bolt's ten completions down:
    //   t=1  fire (ICD until 4) · t=3 gated · t=5 fire (until 8)
    //   t=7  gated · t=9 fire (until 12) · t=11 gated · t=13 fire (16)
    //   t=15 gated · t=17 fire (until 20) · t=19 gated
    // → 5 comets, at t = 1, 5, 9, 13, 17.
    //
    // ── The two buff windows ──────────────────────────────────────────
    // `shock` is applied at every bolt completion (t = 1,3,…,19) and lasts
    // 2.5s under the default `refresh` policy, so its window is
    // continuously live from t=1 to 19 + 2.5 = 21.5, clipped at the fight
    // end: uptime = 19/20 = 0.95, and the `shocked` CONDITION uptime is
    // the same 0.95 (nothing else drives it).
    //
    // `power_surge` is applied by comet's free cast at t = 1,5,9,13,17 and
    // lasts 4.5s, refreshed each time: live from t=1 to 17 + 4.5 = 21.5,
    // clipped — also 0.95. The two coincide by arithmetic, not by
    // mechanism (2.5s refreshed every 2s vs 4.5s refreshed every 4s), and
    // both are pinned because either could move alone.
    //
    // ── Reading the cast-complete ORDER off the timeline ──────────────
    // At t=1 exactly, in `sim`'s fixed order:
    //   bolt's damage is measured  → it is NOT shocked (its own shock has
    //     not landed) and NOT surged (no comet yet): 150.
    //   bolt's `apply_buff` runs   → shock is now live.
    //   the trigger rolls          → comet free-casts, and it IS shocked
    //     (720) because a proc rolled by this cast sees the cast's
    //     `apply_buff`. It is not yet surged — its own buff lands after
    //     its own damage, exactly as bolt's did.
    //   comet's `apply_buff` runs  → power_surge is live from t=1.
    //
    // Everything after t=1 is inside both windows:
    //   bolt   t=3…19 (9 casts)   shocked + surged  → 9 × 225   = 2025
    //   bolt   t=1                bare              →     150   =  150
    //   slam   t=2…20 (10 casts)  shocked + surged  → 10 × 337.5 = 3375
    //   comet  t=1                shocked only      →     720   =  720
    //   comet  t=5,9,13,17        shocked + surged  → 4 × 900   = 3600
    // (Worth checking the two near-misses by hand: comet at t=5 refreshes
    // power_surge, but the window it is joining — opened at t=1, expiring
    // at 5.5 — is still live, so that comet IS surged; and bolt at t=5,
    // measured earlier in the same instant, is surged for the same reason.
    // That is why the duration is 4.5 and not 4.)
    //
    //   total = 150 + 2025 + 3375 + 720 + 3600 = 9870
    //   dps   = 9870 / 20 = 493.5
    //
    // Mutation worth naming: if a proc FREE CAST did not apply its own
    // action's `apply_buff`, `more_global` would never leave 1.0 and the
    // total would be 150 + 9×180 + 10×270 + 5×720 = 8070. The 9870 pin is
    // what says free casts carry their own effects.
    assert_eq!(report.actions["bolt"].casts, 10);
    assert_eq!(report.actions["slam"].casts, 10);
    assert_eq!(report.actions["comet"].casts, 5);
    assert!(
        close(report.actions["bolt"].damage, 2175.0),
        "bolt: got {}",
        report.actions["bolt"].damage
    );
    assert!(
        close(report.actions["slam"].damage, 3375.0),
        "slam: got {}",
        report.actions["slam"].damage
    );
    assert!(
        close(report.actions["comet"].damage, 4320.0),
        "comet: got {}",
        report.actions["comet"].damage
    );
    assert!(
        close(report.total.total_damage, 9870.0),
        "got {}",
        report.total.total_damage
    );
    assert!(close(report.total.dps, 493.5), "got {}", report.total.dps);
    assert!(
        close(report.buffs["shock"].uptime, 0.95),
        "got {}",
        report.buffs["shock"].uptime
    );
    assert!(
        close(report.condition_uptime["shocked"], 0.95),
        "got {}",
        report.condition_uptime["shocked"]
    );
    assert!(
        close(report.buffs["power_surge"].uptime, 0.95),
        "got {}",
        report.buffs["power_surge"].uptime
    );
    println!("\n  EV pins hold: 9870 total / 493.5 dps / 5 comets / 0.95 shock uptime ✓");

    // ══════════ Contrast: the same config without the filter ══════════
    //
    // Drop `actions: ["bolt"]` and the proc reverts to rtce 0.2.0's
    // behavior — every action's cast is an event for it. Completions now
    // arrive every second (bolt on odd t, slam on even t), so the 3s ICD
    // thins them differently:
    //   t=1 fire (until 4) · t=4 fire (7) · t=7 fire (10) · t=10 fire (13)
    //   t=13 fire (16) · t=16 fire (19) · t=19 fire (22)
    // → 7 comets, at t = 1,4,7,10,13,16,19 — and three of them are
    // triggered by a SLAM, which is precisely what the filter existed to
    // prevent.
    //
    // power_surge is refreshed at those seven instants instead (expiries
    // 5.5, 8.5, 11.5, 14.5, 17.5, 20.5, 23.5), so it is still live
    // continuously from t=1 and the uptime is unchanged at 0.95 — bolt and
    // slam therefore hit for exactly what they hit for above. Only the
    // comets move:
    //   comet t=1               shocked only      →     720
    //   comet t=4,7,10,13,16,19 shocked + surged  → 6 × 900 = 5400
    //   comet total = 6120  (vs 4320 with the filter)
    //   total = 2175 + 3375 + 6120 = 11670 ; dps = 583.5
    // The difference is exactly two extra surged comets: 1800.
    let unfiltered: SimDef =
        serde_json::from_str(&simdef_json.replace(r#""actions": ["bolt"], "#, ""))
            .expect("valid simdef");
    assert!(
        unfiltered.procs["trigger_gem"].actions.is_none(),
        "the contrast must actually drop the filter"
    );
    let unfiltered_plan = sim_compile(&plan, &unfiltered, &rotation).expect("simdef compiles");
    let no_filter =
        run(&plan, &unfiltered_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  without `actions: [\"bolt\"]`: {} comets, {:.4} comet damage, \
         {:.4} total, {:.4} dps",
        no_filter.actions["comet"].casts,
        no_filter.actions["comet"].damage,
        no_filter.total.total_damage,
        no_filter.total.dps
    );
    assert_eq!(no_filter.actions["comet"].casts, 7);
    assert_eq!(no_filter.actions["bolt"].casts, 10);
    assert_eq!(no_filter.actions["slam"].casts, 10);
    assert!(
        close(no_filter.actions["bolt"].damage, 2175.0)
            && close(no_filter.actions["slam"].damage, 3375.0),
        "the primaries must be untouched: bolt {} slam {}",
        no_filter.actions["bolt"].damage,
        no_filter.actions["slam"].damage
    );
    assert!(
        close(no_filter.actions["comet"].damage, 6120.0),
        "comet: got {}",
        no_filter.actions["comet"].damage
    );
    assert!(
        close(no_filter.total.total_damage, 11670.0),
        "got {}",
        no_filter.total.total_damage
    );
    assert!(
        close(
            no_filter.total.total_damage - report.total.total_damage,
            1800.0
        ),
        "the gap must be exactly two surged comets"
    );
    println!("  contrast pins hold: 7 comets / 11670 total — 1800 more, all of it comet ✓");

    // ── Monte Carlo ───────────────────────────────────────────────────
    //
    // Nothing here samples either: no `events` block in the gamedef, and a
    // proc at `chance: "1"` fires on every roll in BOTH modes (EV's
    // accumulator crosses on every qualifying event; MC's `next_f64()` is
    // in [0,1) and so is always < 1). Exact equality is therefore the
    // right gate, and it is the one that would catch the trigger filter
    // being applied on only one of the two roll paths — the EV
    // accumulator and the MC draw are separate code, and a filter that
    // reached only one of them would show up here as a cast-count
    // mismatch.
    let mc = run(
        &plan,
        &sim_plan,
        &build,
        &dummy,
        Mode::MonteCarlo {
            iterations: 128,
            seed: 3,
        },
    )
    .expect("mc sim runs");
    let dist = mc.distribution.expect("MC mode reports a distribution");
    println!(
        "\nMonte Carlo (N=128, seed=3): mean {:.4}  std {:.4}",
        dist.mean, dist.std
    );
    assert_eq!(
        mc.actions["comet"].casts, 5,
        "the filter must hold in MC too"
    );
    assert!(close(dist.mean, 493.5), "MC mean {}", dist.mean);
    assert!(close(dist.std, 0.0), "MC std {}", dist.std);
    assert!(
        close(mc.total.total_damage, report.total.total_damage),
        "MC total {} vs EV {}",
        mc.total.total_damage,
        report.total.total_damage
    );
    println!("  MC reproduces EV exactly (std 0), filter included ✓");
}
