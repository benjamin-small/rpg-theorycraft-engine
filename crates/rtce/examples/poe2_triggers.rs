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
use rtce::simdef::{EventOrder, Measure, NumOrExpr, Rotation, SimDef};

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
    //    Both durations are half-integer `representative` values, and that
    //    is load-bearing: it keeps every expiry strictly BETWEEN cast
    //    completions. When a buff's window closes on exactly the instant a
    //    cast completes, the `BuffExpire` was scheduled earlier and so
    //    carries the lower `seq` — it resolves FIRST, and the cast measures
    //    itself WITHOUT the buff it is about to refresh. That is not
    //    hypothetical here: `shock` at a flat `2.0` (refreshed by a bolt
    //    every 2s) still reports 0.95 uptime, but bolt damage falls
    //    2175 → 1837.5, because every bolt after the first loses its own
    //    shock bonus. 2.5 sidesteps that, so the pins below measure the
    //    TRIGGER mechanic and not an intra-instant ordering artifact.
    //
    //    This is a mid-fight ordering property and has nothing to do with
    //    the fight horizon: a cast completing at exactly `duration` counts,
    //    whatever else is queued at that instant (see `Sim::run_loop`'s
    //    "horizon rule").
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

    // ══════ Contrast: shock on the cast grid — the invisible loss ══════
    //
    // This is the measurement behind `sim`'s "a buff expiring on the cast
    // grid" section, pinned here so the number those docs quote cannot
    // rot. (It also settled ROADMAP's ordering question: the answer
    // became P8d's `event_order` knob, run as the LAST contrast below.)
    //
    // Move `shock` from 2.5s to a flat 2.0s and it expires exactly on the
    // bolt cadence (bolts complete at t = 1,3,…,19; a shock applied at t
    // then expires at t+2, which is the NEXT bolt's instant). Both events
    // share that instant, and the `BuffExpire` was scheduled two seconds
    // earlier — at the previous application — so it holds the lower `seq`
    // and resolves FIRST. Every bolt from t=3 on therefore measures itself
    // UNSHOCKED, then immediately re-applies the buff it just lost.
    //
    //   bolt t=1     bare, as before                  →       150
    //   bolt t=3…19  surged but NOT shocked (9 casts) → 9 × 187.5 = 1687.5
    //   bolt total                                    =      1837.5
    //
    // (187.5 = 150 × 1.25: power_surge still applies — it is on a 4.5s
    // clock off the COMET cadence and never lands on a bolt instant. The
    // 225 above was 150 × 1.2 × 1.25, so it is exactly the ×1.2 shock that
    // goes missing, nine times: 9 × 37.5 = 337.5.)
    //
    // Nothing else moves. slam casts on EVEN t, where shock is live
    // (3375), and each comet is free-cast by the proc roll that follows
    // its bolt's `apply_buff`, so it sees shock back up (4320). Total
    // 1837.5 + 3375 + 4320 = 9532.5, dps 476.625.
    //
    // THE POINT: the gap is zero-width — expiry and reapplication are the
    // same instant — so every INTEGRATED measurement is unchanged. shock
    // uptime still 0.9500, `shocked` condition uptime still 0.9500. 15.5%
    // of bolt damage vanishes and not one uptime column blinks. A config
    // author who writes "2s shock, refreshed by a bolt every 2s" gets a
    // plausible-looking report and a wrong number.
    let on_grid: SimDef =
        serde_json::from_str(&simdef_json.replace(r#""duration": 2.5"#, r#""duration": 2.0"#))
            .expect("valid simdef");
    assert!(
        matches!(on_grid.buffs["shock"].duration, NumOrExpr::Num(d) if d == 2.0),
        "the contrast must actually move shock onto the cast grid"
    );
    let on_grid_plan = sim_compile(&plan, &on_grid, &rotation).expect("simdef compiles");
    let grid = run(&plan, &on_grid_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  with `shock` at 2.0 (on the bolt cadence): bolt {:.4}, total {:.4}, \
         dps {:.4} — shock uptime STILL {:.4}",
        grid.actions["bolt"].damage,
        grid.total.total_damage,
        grid.total.dps,
        grid.buffs["shock"].uptime
    );
    assert!(
        close(grid.actions["bolt"].damage, 1837.5),
        "bolt on the grid: got {}",
        grid.actions["bolt"].damage
    );
    // The uptimes are the whole point: identical to the 2.5 run.
    assert!(
        close(grid.buffs["shock"].uptime, 0.95),
        "shock uptime must be unchanged: got {}",
        grid.buffs["shock"].uptime
    );
    assert!(
        close(grid.condition_uptime["shocked"], 0.95),
        "shocked condition uptime must be unchanged: got {}",
        grid.condition_uptime["shocked"]
    );
    // Everything NOT on the grid is untouched.
    assert!(
        close(grid.actions["slam"].damage, 3375.0) && close(grid.actions["comet"].damage, 4320.0),
        "only bolt may move: slam {} comet {}",
        grid.actions["slam"].damage,
        grid.actions["comet"].damage
    );
    assert!(
        close(grid.total.total_damage, 9532.5),
        "got {}",
        grid.total.total_damage
    );
    // The loss, stated the way the docs state it.
    assert!(
        close(
            report.actions["bolt"].damage - grid.actions["bolt"].damage,
            337.5
        ),
        "the gap must be exactly the nine missing shock multipliers"
    );
    println!("  footgun pins hold: 2175 → 1837.5 bolt damage at IDENTICAL 0.95 uptime ✓");

    // ══════ Contrast: the config that FIXES the cast-grid footgun ══════
    //
    // P8c's measurement knob. Keep `shock` at the on-grid 2.0 and add
    //
    //     "defaults": { "measure": "cast_start" }
    //
    // — every cast is now measured at the instant it BEGINS, and the
    // grid collision dissolves: the expiry-vs-completion race happens at
    // completions, but nothing is measured there anymore.
    //
    //   bolt N casts at t = 2(N−1). The previous bolt's completion at
    //   t = 2N−3 applied shock for [2N−3, 2N−1) — so every bolt from the
    //   SECOND on starts strictly inside the previous completion's
    //   window and measures shocked (power_surge is live from t=1 on and
    //   never lapses, so it is surged too):
    //     bolt t=0            unbuffed  →       150
    //     bolt t=2,4,…,18     shocked + surged (9 casts) → 9 × 225 = 2025
    //     bolt total                                     =      2175
    //
    //   Restored to the off-grid number — same 10 casts, same 0.95
    //   uptimes. slam starts at odd t ≥ 1, always inside both windows
    //   (10 × 337.5 = 3375). comet is a proc-triggered FREE cast and is
    //   deliberately NOT governed by the knob: it begins and completes
    //   at the firing proc's instant and measures the live world there
    //   (720 + 4 × 900 = 4320, unchanged). Total 9870 — the 2.5s run's
    //   number, recovered by config instead of by nudging the duration.
    let fixed_json = simdef_json.replace(r#""duration": 2.5"#, r#""duration": 2.0"#);
    let fixed_json = fixed_json.replacen(
        r#"{
      "actions""#,
        r#"{
      "defaults": { "measure": "cast_start" },
      "actions""#,
        1,
    );
    let fixed: SimDef = serde_json::from_str(&fixed_json).expect("valid simdef");
    assert!(
        matches!(fixed.buffs["shock"].duration, NumOrExpr::Num(d) if d == 2.0),
        "the fix run must keep shock ON the cast grid"
    );
    assert_eq!(
        fixed.defaults.measure,
        Measure::CastStart,
        "the injection must actually set the knob — a silent no-op replace \
         would re-pin the footgun numbers and call them fixed"
    );
    let fixed_plan = sim_compile(&plan, &fixed, &rotation).expect("simdef compiles");
    let unfooted = run(&plan, &fixed_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  with `shock` at 2.0 AND `defaults.measure: \"cast_start\"`: bolt {:.4}, \
         total {:.4} — the 2.5 run's numbers, restored by config",
        unfooted.actions["bolt"].damage, unfooted.total.total_damage
    );
    assert_eq!(unfooted.actions["bolt"].casts, 10);
    assert!(
        close(unfooted.actions["bolt"].damage, 2175.0),
        "bolt under cast_start on the grid: got {} — want 150 + 9×225, \
         every bolt after the first measured inside the previous \
         completion's shock window",
        unfooted.actions["bolt"].damage
    );
    assert!(
        close(unfooted.actions["comet"].damage, 4320.0),
        "comet must be untouched — a free cast measures live at its own \
         instant, outside this knob: got {}",
        unfooted.actions["comet"].damage
    );
    assert!(
        close(unfooted.total.total_damage, 9870.0),
        "got {}",
        unfooted.total.total_damage
    );
    assert!(
        close(unfooted.buffs["shock"].uptime, 0.95),
        "uptime stays 0.95 in every one of these runs: got {}",
        unfooted.buffs["shock"].uptime
    );
    println!("  measurement pins hold: 1837.5 → 2175 bolt damage, by `defaults.measure` ✓");

    // ══════ Contrast: the OTHER config that fixes it (P8d) ════════════
    //
    // Same on-grid 2.0s `shock`, and the measure stays at its default —
    // this time move the COLLISION instead of the measurement:
    //
    //     "defaults": { "event_order": "completions_first" }
    //
    // Every `CastComplete` now outranks a coincident `BuffExpire`
    // (package-wide by design — ordering is a property of the QUEUE, and
    // a collision involves two entities, so there is deliberately no
    // per-spell form). The bolt at t=3,5,…,19 resolves BEFORE the shock
    // expiry sharing its instant, measures WITH the still-live shock,
    // and its reapplication bumps the buff generation — the pending
    // expiry is stale, a no-op. Same casts, same 0.95 uptimes, and bolt
    // is restored to 150 + 9 × 225 = 2175 (total 9870), the 2.5s run's
    // numbers again. comet is a proc-fired FREE cast at the firing
    // proc's instant — no queue entry of its own, nothing coincident —
    // so it is untouched by this knob too (4320).
    //
    // The two fixes, side by side (`shock` at the on-grid 2.0
    // throughout; the no-knob row is the footgun):
    //
    //   defaults                          bolt      total   what moved
    //   (none)                          1837.5     9532.5   —
    //   measure: "cast_start"           2175.0     9870.0   the measurement
    //   event_order:
    //     "completions_first"           2175.0     9870.0   the ordering
    //
    // One knob moves the MEASUREMENT off the collision (nothing is
    // measured at completions anymore); the other moves the COLLISION
    // itself (the completion wins it). Same number by two mechanisms —
    // see `Measure` and `EventOrder` for what else each knob moves
    // before adopting either wholesale (`casts.<self>` and resource
    // readings for the former; zero-weight-phase attribution for the
    // latter).
    let reordered_json = simdef_json.replace(r#""duration": 2.5"#, r#""duration": 2.0"#);
    let reordered_json = reordered_json.replacen(
        r#"{
      "actions""#,
        r#"{
      "defaults": { "event_order": "completions_first" },
      "actions""#,
        1,
    );
    let reordered: SimDef = serde_json::from_str(&reordered_json).expect("valid simdef");
    assert!(
        matches!(reordered.buffs["shock"].duration, NumOrExpr::Num(d) if d == 2.0),
        "the fix run must keep shock ON the cast grid"
    );
    assert_eq!(
        reordered.defaults.event_order,
        EventOrder::CompletionsFirst,
        "the injection must actually set the knob — a silent no-op replace \
         would re-pin the footgun numbers and call them fixed"
    );
    assert_eq!(
        reordered.defaults.measure,
        Measure::CastComplete,
        "the measure must stay at its default — this run is the ORDERING \
         fix, not the measurement fix again"
    );
    let reordered_plan = sim_compile(&plan, &reordered, &rotation).expect("simdef compiles");
    let requeued =
        run(&plan, &reordered_plan, &build, &dummy, Mode::Expected).expect("ev sim runs");

    println!(
        "\n  with `shock` at 2.0 AND `defaults.event_order: \"completions_first\"`: \
         bolt {:.4}, total {:.4} — the same restoration, by the ORDERING knob",
        requeued.actions["bolt"].damage, requeued.total.total_damage
    );
    assert_eq!(requeued.actions["bolt"].casts, 10);
    assert!(
        close(requeued.actions["bolt"].damage, 2175.0),
        "bolt under completions_first on the grid: got {} — want 150 + \
         9×225, every completion resolving before the coincident expiry",
        requeued.actions["bolt"].damage
    );
    assert!(
        close(requeued.actions["comet"].damage, 4320.0),
        "comet must be untouched — a free cast has no queue entry to \
         reorder: got {}",
        requeued.actions["comet"].damage
    );
    assert!(
        close(requeued.total.total_damage, 9870.0),
        "got {}",
        requeued.total.total_damage
    );
    assert!(
        close(requeued.buffs["shock"].uptime, 0.95),
        "uptime stays 0.95 in every one of these runs: got {}",
        requeued.buffs["shock"].uptime
    );
    assert!(
        close(requeued.total.total_damage, unfooted.total.total_damage),
        "the two knobs must land on the SAME number: ordering {} vs \
         measurement {}",
        requeued.total.total_damage,
        unfooted.total.total_damage
    );
    println!("  ordering pins hold: 1837.5 → 2175 bolt damage, by `defaults.event_order` ✓");

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
