//! The `remove_buff` effect pins. Every pin is hand-derived from the
//! schedule in its comment, with a contrast run that only drops the
//! removal.
//!
//! The fixture is a one-stat game, `hit = dmg × (1 + marked)`, where the
//! `marked` condition is driven by the `mark` buff, so damage shows
//! whether the mark was up when a cast was measured.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::scenario::Scenario;
use rtce::simdef::{Rotation, SimDef};
use rtce::{plan, sim};

fn game() -> GameDef {
    serde_json::from_str(
        r#"{ "stats": ["dmg"], "conditions": ["marked"],
             "pipeline": [ { "name": "hit", "expr": "dmg * (1 + marked)" } ],
             "objectives": ["hit"] }"#,
    )
    .unwrap()
}

fn compile(simdef: &str, rotation: &str) -> Result<(plan::Plan, sim::SimPlan), String> {
    let plan = plan::compile(&game()).unwrap();
    let simdef: SimDef = serde_json::from_str(simdef).map_err(|e| e.to_string())?;
    let rotation: Rotation = serde_json::from_str(rotation).map_err(|e| e.to_string())?;
    let sp = sim::compile(&plan, &simdef, &rotation).map_err(|e| e.to_string())?;
    Ok((plan, sp))
}

fn try_run(
    simdef: &str,
    rotation: &str,
    seconds: f64,
    mode: sim::Mode,
) -> Result<sim::SimReport, String> {
    let (plan, sp) = compile(simdef, rotation)?;
    let build: BuildState = serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
    let scenario: Scenario = serde_json::from_str(&format!(
        r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
    ))
    .unwrap();
    sim::run(&plan, &sp, &build, &scenario, mode).map_err(|e| e.to_string())
}

const EV: sim::Mode = sim::Mode::Expected;
const MC: sim::Mode = sim::Mode::MonteCarlo {
    iterations: 8,
    seed: 11,
};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// `apply` marks (10s), `consume` spends the mark, both 1s casts;
/// rotation: consume while marked, else apply. 6s:
/// t0 apply → mark at 1; t1 consume (lands 2, removes); t2 apply → 3;
/// t3 consume → 4; t4 apply → 5; t5 consume → 6. 3 and 3; the mark is
/// up [1,2] [3,4] [5,6] = 3s → uptime 0.5. `consume` is measured at its
/// completion, BEFORE its own effects run, so each consume hits marked
/// (200) and each apply unmarked (100) → 900.
fn spender(consume_effects: &str) -> String {
    format!(
        r#"{{ "actions": {{
               "apply":   {{ "cast_time": "1", "damage": {{}}, "effects": [ {{ "apply_buff": "mark" }} ] }},
               "consume": {{ "cast_time": "1", "damage": {{}}, "effects": {consume_effects} }} }},
             "buffs": {{ "mark": {{ "duration": 10.0, "conditions": {{ "marked": 1.0 }} }} }},
             "damage_objective": "hit" }}"#
    )
}

const SPEND: &str =
    r#"{ "rules": [ { "action": "consume", "when": "buff.mark" }, { "action": "apply" } ] }"#;

#[test]
fn an_action_consumes_a_buff_and_ev_and_mc_agree_exactly() {
    for mode in [EV, MC] {
        let r = try_run(
            &spender(r#"[ { "remove_buff": "mark" } ]"#),
            SPEND,
            6.0,
            mode,
        )
        .unwrap();
        assert_eq!(r.actions["apply"].casts, 3);
        assert_eq!(r.actions["consume"].casts, 3);
        assert!(
            close(r.buffs["mark"].uptime, 0.5),
            "{}",
            r.buffs["mark"].uptime
        );
        assert!(
            close(r.total.total_damage, 900.0),
            "{}",
            r.total.total_damage
        );
    }
}

/// Contrast: without the removal the mark never drops after t=1, so
/// after one apply every cast is a marked consume: apply 1, consume 5,
/// uptime 5/6, damage 100 + 5 × 200 = 1100.
#[test]
fn without_the_removal_the_mark_stays_up() {
    let r = try_run(&spender("[]"), SPEND, 6.0, EV).unwrap();
    assert_eq!(r.actions["apply"].casts, 1);
    assert_eq!(r.actions["consume"].casts, 5);
    assert!(close(r.buffs["mark"].uptime, 5.0 / 6.0));
    assert!(close(r.total.total_damage, 1100.0));
}

/// A removal must stale the removed window's pending expiry. `mark` lasts
/// 2.5s here. t0 apply → mark [1, 3.5); t1 consume → removed at 2;
/// t2 apply → mark [3, 5.5); then `wait` to 6. Uptime (1 + 2.5) / 6.
/// A stale expiry firing at 3.5 would cut the second window to 0.5s.
#[test]
fn a_removal_stales_the_removed_windows_expiry() {
    let simdef = r#"{ "actions": {
           "apply":   { "cast_time": "1", "effects": [ { "apply_buff": "mark" } ] },
           "consume": { "cast_time": "1", "effects": [ { "remove_buff": "mark" } ] },
           "wait":    { "cast_time": "1" } },
         "buffs": { "mark": { "duration": 2.5, "conditions": { "marked": 1.0 } } },
         "damage_objective": "hit" }"#;
    let rotation = r#"{ "rules": [
           { "action": "consume", "when": "and(buff.mark, casts.consume < 1)" },
           { "action": "apply", "when": "casts.apply < 2" },
           { "action": "wait" } ] }"#;
    let r = try_run(simdef, rotation, 6.0, EV).unwrap();
    assert_eq!(r.actions["apply"].casts, 2);
    assert_eq!(r.actions["consume"].casts, 1);
    assert!(
        close(r.buffs["mark"].uptime, 3.5 / 6.0),
        "{}",
        r.buffs["mark"].uptime
    );
}

/// Removing a buff that is not up changes nothing: `consume` alone for
/// 3s, never marked → 3 casts of 100, uptime 0.
#[test]
fn removing_an_inactive_buff_is_a_no_op() {
    let r = try_run(
        &spender(r#"[ { "remove_buff": "mark" } ]"#),
        r#"{ "rules": [ { "action": "consume" } ] }"#,
        3.0,
        EV,
    )
    .unwrap();
    assert_eq!(r.actions["consume"].casts, 3);
    assert_eq!(r.buffs["mark"].uptime, 0.0);
    assert!(close(r.total.total_damage, 300.0));
}

/// A segment's effects can remove too: a two-segment `consume` (0.5s
/// each) removes the mark at its FIRST landing, so its second segment is
/// unmarked. 4s: t0 apply → mark at 1; t1 consume: seg 1 lands at 1.5
/// marked (200) and removes, seg 2 at 2.0 unmarked (100); t2 apply → 3;
/// t3 consume: 200 + 100 at 3.5 / 4.0. Total 100 + 300 + 100 + 300 = 800.
#[test]
fn a_segment_effect_removes_at_its_landing() {
    let simdef = r#"{ "actions": {
           "apply":   { "cast_time": "1", "damage": {}, "effects": [ { "apply_buff": "mark" } ] },
           "consume": { "segments": [
               { "duration": 0.5, "damage": {}, "effects": [ { "remove_buff": "mark" } ] },
               { "duration": 0.5, "damage": {} } ] } },
         "buffs": { "mark": { "duration": 10.0, "conditions": { "marked": 1.0 } } },
         "damage_objective": "hit" }"#;
    let r = try_run(simdef, SPEND, 4.0, EV).unwrap();
    assert_eq!(r.actions["consume"].casts, 2);
    assert!(
        close(r.total.total_damage, 800.0),
        "{}",
        r.total.total_damage
    );
}

/// A proc that removes: every `apply` cast also fires `wipe` (on_cast,
/// chance 1), which removes the mark right after the cast applied it.
/// Effects then procs: the mark is applied and immediately wiped, so it
/// is never up for any positive time → uptime 0, and every consume rule
/// is skipped: 3 applies in 3s.
const WIPE: &str = r#"{ "actions": {
       "apply":   { "cast_time": "1", "damage": {}, "effects": [ { "apply_buff": "mark" } ] },
       "consume": { "cast_time": "1", "damage": {} } },
     "buffs": { "mark": { "duration": 10.0, "conditions": { "marked": 1.0 } } },
     "procs": { "wipe": { "trigger": "on_cast", "chance": "1", "actions": ["apply"],
                          "effects": [ { "remove_buff": "mark" } ] } },
     "damage_objective": "hit" }"#;

#[test]
fn a_proc_removal_runs_in_monte_carlo() {
    let r = try_run(WIPE, SPEND, 3.0, MC).unwrap();
    assert_eq!(r.actions["apply"].casts, 3);
    assert_eq!(r.buffs["mark"].uptime, 0.0);
}

/// The same config in EV fails closed: a proc removes a buff that a
/// rotation rule reads, so EV's averaged proc schedule would branch the
/// rotation on a state it cannot represent.
#[test]
fn a_proc_removal_of_a_buff_a_rule_reads_fails_closed_in_ev() {
    let err = try_run(WIPE, SPEND, 3.0, EV).unwrap_err();
    assert!(
        err.contains("wipe") && err.contains("mark") && err.contains("Monte Carlo"),
        "{err}"
    );
}

/// A rule reading the CONDITION the buff drives is the same read.
#[test]
fn reading_the_driven_condition_also_fails_closed_in_ev() {
    let err = try_run(
        WIPE,
        r#"{ "rules": [ { "action": "consume", "when": "marked >= 1" }, { "action": "apply" } ] }"#,
        3.0,
        EV,
    )
    .unwrap_err();
    assert!(err.contains("marked"), "{err}");
}

/// With no rule reading the buff, EV runs the proc removal on its usual
/// averaged schedule. Same 0 uptime: chance 1 fires every cast.
#[test]
fn a_proc_removal_no_rule_reads_runs_in_ev() {
    let r = try_run(WIPE, r#"{ "rules": [ { "action": "apply" } ] }"#, 3.0, EV).unwrap();
    assert_eq!(r.actions["apply"].casts, 3);
    assert_eq!(r.buffs["mark"].uptime, 0.0);
}

/// A free cast's removal is proc-driven too.
#[test]
fn a_free_cast_removal_of_a_read_buff_fails_closed_in_ev() {
    let simdef = r#"{ "actions": {
           "apply":   { "cast_time": "1", "damage": {}, "effects": [ { "apply_buff": "mark" } ] },
           "consume": { "cast_time": "1", "damage": {} },
           "purge":   { "cast_time": "0", "effects": [ { "remove_buff": "mark" } ] } },
         "buffs": { "mark": { "duration": 10.0, "conditions": { "marked": 1.0 } } },
         "procs": { "echo": { "trigger": "on_cast", "chance": "1", "actions": ["apply"],
                              "cast_action": "purge" } },
         "damage_objective": "hit" }"#;
    let err = try_run(simdef, SPEND, 3.0, EV).unwrap_err();
    assert!(err.contains("echo") && err.contains("purge"), "{err}");
    assert!(try_run(simdef, SPEND, 3.0, MC).is_ok());
}

#[test]
fn bad_removals_fail_closed() {
    let unknown = compile(&spender(r#"[ { "remove_buff": "makr" } ]"#), SPEND).unwrap_err();
    assert!(unknown.contains("unknown buff `makr`"), "{unknown}");

    let typo = compile(&spender(r#"[ { "remove_buf": "mark" } ]"#), SPEND).unwrap_err();
    assert!(typo.contains("did you mean `remove_buff`"), "{typo}");

    let expected = compile(
        r#"{ "actions": {
               "apply":   { "cast_time": "1", "damage": {} },
               "consume": { "cast_time": "1", "effects": [ { "remove_buff": "mark" } ] } },
             "buffs": { "mark": { "duration": 10.0, "ev_uptime": "expected",
                                  "conditions": { "marked": 1.0 } } },
             "procs": { "p": { "trigger": "on_hit", "chance": "0.5", "apply_buff": "mark" } },
             "damage_objective": "hit" }"#,
        r#"{ "rules": [ { "action": "consume", "when": "time > 100" }, { "action": "apply" } ] }"#,
    )
    .unwrap_err();
    assert!(
        expected.contains("ev_uptime: expected") && expected.contains("remove_buff"),
        "{expected}"
    );
}
