//! The global cooldown (GCD) pins: `SimDefaults::gcd` and
//! `ActionDef::gcd`. Every pin is a cast count hand-derived from the
//! schedule in its comment, and each comes with a contrast run that only
//! changes the GCD setting.
//!
//! The fixture is a one-stat game, `hit = dmg`, so damage is
//! `casts × 100` throughout; the counts carry the meaning.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::scenario::Scenario;
use rtce::simdef::{Rotation, SimDef};
use rtce::{plan, sim};

fn game() -> GameDef {
    serde_json::from_str(
        r#"{ "stats": ["dmg", "haste"], "pipeline": [ { "name": "hit", "expr": "dmg" } ],
             "objectives": ["hit"] }"#,
    )
    .unwrap()
}

fn build(haste: f64) -> BuildState {
    serde_json::from_str(&format!(
        r#"{{ "stats": {{ "dmg": 100.0, "haste": {haste} }} }}"#
    ))
    .unwrap()
}

fn fight(seconds: f64) -> Scenario {
    serde_json::from_str(&format!(
        r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
    ))
    .unwrap()
}

fn try_run(
    simdef: &str,
    rotation: &str,
    haste: f64,
    seconds: f64,
) -> Result<sim::SimReport, String> {
    let plan = plan::compile(&game()).unwrap();
    let simdef: SimDef = serde_json::from_str(simdef).map_err(|e| e.to_string())?;
    let rotation: Rotation = serde_json::from_str(rotation).unwrap();
    let sim_plan = sim::compile(&plan, &simdef, &rotation).map_err(|e| e.to_string())?;
    sim::run(
        &plan,
        &sim_plan,
        &build(haste),
        &fight(seconds),
        sim::Mode::Expected,
    )
    .map_err(|e| e.to_string())
}

fn run(simdef: &str, rotation: &str, haste: f64, seconds: f64) -> sim::SimReport {
    try_run(simdef, rotation, haste, seconds).unwrap()
}

fn casts(report: &sim::SimReport, action: &str) -> u64 {
    report.actions[action].casts
}

const JAB_ONLY: &str = r#"{ "rules": [ { "action": "jab" } ] }"#;

/// An instant filler with no cooldown has no finite DPS without a GCD:
/// the executor's instant-chain guard fails it closed. That is the
/// contrast for the first pin below.
#[test]
fn without_a_gcd_an_instant_filler_is_a_livelock() {
    let err = try_run(
        r#"{ "actions": { "jab": { "cast_time": "0", "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        15.0,
    )
    .unwrap_err();
    assert!(err.contains("instant-cast livelock"), "{err}");
}

/// `defaults.gcd: 1.5` paces the same instant filler: casts begin at
/// t = 0, 1.5, …, 13.5 (10 casts), and none begins at the 15s horizon.
#[test]
fn the_defaults_gcd_paces_an_instant_filler() {
    let r = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": { "jab": { "cast_time": "0", "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        15.0,
    );
    assert_eq!(casts(&r, "jab"), 10);
    assert!(
        (r.total.total_damage - 1000.0).abs() < 1e-9,
        "{}",
        r.total.total_damage
    );
}

/// A cast longer than the GCD is paced by its cast time: a 2s cast
/// under a 1.5s GCD in 10s begins at 0, 2, 4, 6, 8 and the last one
/// completes on the horizon, so 5 casts with or without the GCD.
#[test]
fn a_cast_longer_than_the_gcd_is_paced_by_its_cast_time() {
    let with = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": { "bolt": { "cast_time": "2", "damage": {} } },
             "damage_objective": "hit" }"#,
        r#"{ "rules": [ { "action": "bolt" } ] }"#,
        0.0,
        10.0,
    );
    let without = run(
        r#"{ "actions": { "bolt": { "cast_time": "2", "damage": {} } },
             "damage_objective": "hit" }"#,
        r#"{ "rules": [ { "action": "bolt" } ] }"#,
        0.0,
        10.0,
    );
    assert_eq!(casts(&with, "bolt"), 5);
    assert_eq!(casts(&without, "bolt"), 5);
}

/// A cast SHORTER than the GCD leaves the actor idle until the GCD ends:
/// a 0.5s cast under a 1.5s GCD in 6s begins at 0, 1.5, 3, 4.5 (4
/// casts). Contrast: without the GCD it chains every 0.5s, 12 casts.
#[test]
fn a_cast_shorter_than_the_gcd_waits_for_the_gcd() {
    let with = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": { "flick": { "cast_time": "0.5", "damage": {} } },
             "damage_objective": "hit" }"#,
        r#"{ "rules": [ { "action": "flick" } ] }"#,
        0.0,
        6.0,
    );
    let without = run(
        r#"{ "actions": { "flick": { "cast_time": "0.5", "damage": {} } },
             "damage_objective": "hit" }"#,
        r#"{ "rules": [ { "action": "flick" } ] }"#,
        0.0,
        6.0,
    );
    assert_eq!(casts(&with, "flick"), 4);
    assert_eq!(casts(&without, "flick"), 12);
}

const BURST_THEN_JAB: &str = r#"{ "rules": [ { "action": "burst" }, { "action": "jab" } ] }"#;

/// `gcd: 0` on an action makes it OFF the GCD: it neither waits for the
/// lock nor arms it. 6s, GCD 1.5, burst on a 3s cooldown:
/// t=0 burst then jab (same instant), t=1.5 jab, t=3 burst then jab,
/// t=4.5 jab → burst 2, jab 4.
#[test]
fn an_action_with_gcd_zero_is_off_the_gcd() {
    let r = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": {
               "burst": { "cast_time": "0", "cooldown": 3.0, "gcd": 0, "damage": {} },
               "jab":   { "cast_time": "0", "damage": {} } },
             "damage_objective": "hit" }"#,
        BURST_THEN_JAB,
        0.0,
        6.0,
    );
    assert_eq!(casts(&r, "burst"), 2);
    assert_eq!(casts(&r, "jab"), 4);
}

/// Contrast for the off-GCD pin: the same burst ON the GCD costs a GCD
/// of its own. t=0 burst, t=1.5 jab, t=3 burst, t=4.5 jab → 2 and 2.
#[test]
fn the_same_burst_on_the_gcd_costs_a_jab_each_time() {
    let r = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": {
               "burst": { "cast_time": "0", "cooldown": 3.0, "damage": {} },
               "jab":   { "cast_time": "0", "damage": {} } },
             "damage_objective": "hit" }"#,
        BURST_THEN_JAB,
        0.0,
        6.0,
    );
    assert_eq!(casts(&r, "burst"), 2);
    assert_eq!(casts(&r, "jab"), 2);
}

/// A per-action `gcd` works with no `defaults.gcd` at all, and it binds
/// only the action that names it. 5s: jab (gcd 1) begins at 0, 1, 2, 3,
/// 4 → 5 casts.
#[test]
fn a_per_action_gcd_needs_no_defaults_block() {
    let r = run(
        r#"{ "actions": { "jab": { "cast_time": "0", "gcd": 1.0, "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        5.0,
    );
    assert_eq!(casts(&r, "jab"), 5);
}

/// A per-action `gcd` overrides the defaults value. defaults 1.5, jab
/// 1.0, 5s → 5 casts (the defaults value alone would give 4: 0, 1.5, 3,
/// 4.5).
#[test]
fn a_per_action_gcd_overrides_the_defaults_value() {
    let over = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": { "jab": { "cast_time": "0", "gcd": 1.0, "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        5.0,
    );
    let inherit = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": { "jab": { "cast_time": "0", "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        5.0,
    );
    assert_eq!(casts(&over, "jab"), 5);
    assert_eq!(casts(&inherit, "jab"), 4);
}

const HASTED: &str = r#"{ "defaults": { "gcd": "max(0.75, 1.5 / (1 + haste / 100))" },
     "actions": { "jab": { "cast_time": "0", "damage": {} } },
     "damage_objective": "hit" }"#;

/// A haste-scaled GCD expression, WoW's shape: 1.5s divided by
/// (1 + haste), floored at 0.75s. 50% haste → 1.0s → 5 casts in 5s;
/// 200% haste → floored 0.75s → 0, 0.75, …, 4.5 = 7 casts.
#[test]
fn a_gcd_expression_scales_with_haste_and_honors_its_floor() {
    assert_eq!(casts(&run(HASTED, JAB_ONLY, 50.0, 5.0), "jab"), 5);
    assert_eq!(casts(&run(HASTED, JAB_ONLY, 200.0, 5.0), "jab"), 7);
}

/// A GCD that evaluates negative fails closed, naming the action.
#[test]
fn a_negative_gcd_fails_closed() {
    let err = try_run(
        r#"{ "defaults": { "gcd": "0 - 1" },
             "actions": { "jab": { "cast_time": "0", "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        5.0,
    )
    .unwrap_err();
    assert!(err.contains("jab") && err.contains("gcd"), "{err}");
}

/// A typo'd key fails closed with a did-you-mean, on both levels.
#[test]
fn a_misspelled_gcd_key_fails_closed() {
    let action = try_run(
        r#"{ "actions": { "jab": { "cast_time": "0", "gdc": 1.0, "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        5.0,
    )
    .unwrap_err();
    assert!(action.contains("did you mean `gcd`"), "{action}");
    let defaults = try_run(
        r#"{ "defaults": { "gdc": 1.5 },
             "actions": { "jab": { "cast_time": "0", "gcd": 1.0, "damage": {} } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        5.0,
    )
    .unwrap_err();
    assert!(defaults.contains("did you mean `gcd`"), "{defaults}");
}

/// A proc's free cast ignores the GCD: it neither waits for the lock nor
/// arms it. Every jab (GCD 1.5) fires `echo` as a free cast; 6s → jab at
/// 0, 1.5, 3, 4.5 → 4 jabs, 4 echoes, and the jab count is unchanged
/// from a run with no proc.
#[test]
fn a_free_cast_ignores_the_gcd() {
    let r = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": {
               "jab":  { "cast_time": "0", "damage": {} },
               "echo": { "cast_time": "0", "damage": {} } },
             "procs": { "ring": { "trigger": "on_cast", "chance": "1",
                                  "actions": ["jab"], "cast_action": "echo" } },
             "damage_objective": "hit" }"#,
        JAB_ONLY,
        0.0,
        6.0,
    );
    assert_eq!(casts(&r, "jab"), 4);
    assert_eq!(casts(&r, "echo"), 4);
}

/// A segmented (channelled) cast arms the GCD at its start like any
/// cast; a 0.5s two-segment channel under a 1.5s GCD in 3s begins at 0
/// and 1.5 → 2 casts (6 without the GCD).
#[test]
fn a_segmented_cast_arms_the_gcd_at_its_start() {
    let channel = r#""chan": { "segments": [ { "duration": 0.25, "damage": {} },
                                              { "duration": 0.25, "damage": {} } ] }"#;
    let with = run(
        &format!(
            r#"{{ "defaults": {{ "gcd": 1.5 }}, "actions": {{ {channel} }},
                  "damage_objective": "hit" }}"#
        ),
        r#"{ "rules": [ { "action": "chan" } ] }"#,
        0.0,
        3.0,
    );
    let without = run(
        &format!(r#"{{ "actions": {{ {channel} }}, "damage_objective": "hit" }}"#),
        r#"{ "rules": [ { "action": "chan" } ] }"#,
        0.0,
        3.0,
    );
    assert_eq!(casts(&with, "chan"), 2);
    assert_eq!(casts(&without, "chan"), 6);
}

/// The literal `0` and an EXPRESSION evaluating to `0` are different
/// cells: the literal is off the GCD, the expression is GCD-bound with a
/// zero-length lock. 6s, GCD 1.5, burst on a 2s cooldown.
///
/// Literal `0`, burst weaves inside jab's lock: burst 0, 2, 4 and jab
/// 0, 1.5, 3, 4.5 → 3 and 4.
///
/// Expression `"0"`, burst waits for the lock: t=0 burst then jab,
/// t=1.5 jab (burst not ready until 2), t=3 burst then jab, t=4.5 jab;
/// burst is ready at 5 but jab's lock holds until the 6s horizon →
/// 2 and 4.
#[test]
fn a_zero_valued_gcd_expression_is_still_gcd_bound() {
    let simdef = |gcd: &str| {
        format!(
            r#"{{ "defaults": {{ "gcd": 1.5 }},
                 "actions": {{
                   "burst": {{ "cast_time": "0", "cooldown": 2.0, "gcd": {gcd}, "damage": {{}} }},
                   "jab":   {{ "cast_time": "0", "damage": {{}} }} }},
                 "damage_objective": "hit" }}"#
        )
    };
    let literal = run(&simdef("0"), BURST_THEN_JAB, 0.0, 6.0);
    let expr = run(&simdef(r#""0""#), BURST_THEN_JAB, 0.0, 6.0);
    assert_eq!((casts(&literal, "burst"), casts(&literal, "jab")), (3, 4));
    assert_eq!((casts(&expr, "burst"), casts(&expr, "jab")), (2, 4));
}

/// A player cancel's replacement is gated by the lock like any decision.
/// GCD 1.5; `chan` is four 0.5s segments (interruptible); `nuke` is a
/// one-shot instant that becomes wanted at t=0.5. 2.5s fight:
/// chan begins at 0 (lock → 1.5); the boundaries at 0.5 and 1.0 cannot
/// cancel into nuke (lock held); at 1.5 they can → chan landed 3
/// segments (300 damage), nuke fires at 1.5 (lock → 3.0, past the
/// horizon, so chan never restarts).
///
/// Contrast (what an ungated cancel would do): cancel at 0.5 after one
/// segment, nuke locks to 2.0, chan restarts at 2.0 and lands one more
/// segment at 2.5 → 200.
#[test]
fn a_cancel_waits_for_the_gcd_before_starting_its_replacement() {
    let r = run(
        r#"{ "defaults": { "gcd": 1.5 },
             "actions": {
               "nuke": { "cast_time": "0", "cooldown": 100.0, "damage": {} },
               "chan": { "segments": [
                 { "duration": 0.5, "damage": {} }, { "duration": 0.5, "damage": {} },
                 { "duration": 0.5, "damage": {} }, { "duration": 0.5, "damage": {} } ] } },
             "damage_objective": "hit" }"#,
        r#"{ "rules": [ { "action": "nuke", "when": "time >= 0.5" },
                        { "action": "chan", "interrupt": true } ] }"#,
        0.0,
        2.5,
    );
    assert_eq!(r.actions["chan"].cancelled, 1);
    assert_eq!(casts(&r, "nuke"), 1);
    assert!(
        (r.actions["chan"].damage - 300.0).abs() < 1e-9,
        "{}",
        r.actions["chan"].damage
    );
}
