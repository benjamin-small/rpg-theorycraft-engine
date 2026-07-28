//! The fail-closed unknown-key sweep, pinned SITE BY SITE (P8a
//! follow-up). The spec review found three surviving mutations: deleting
//! the whole `ResourceDef` walk, the `EventDef` walk, or the `BuildState`
//! `reject_unknown` call left the suite green, because only 5 of the 16
//! config structs had a rejection test. This table drives one typo'd key
//! through EVERY site — a walk (or parse-time rejection) that silently
//! disappears now turns exactly its row red.

use rtce::gamedef::GameDef;
use rtce::scenario::Scenario;
use rtce::simdef::{Rotation, SimDef};
use rtce::{build::BuildState, plan, sim};

/// The minimal plan the sim-side rows compile against.
fn tiny_plan() -> plan::Plan {
    let def: GameDef = serde_json::from_str(
        r#"{ "stats": ["w"],
             "pipeline": [ { "name": "hit", "expr": "w" } ],
             "objectives": ["hit"] }"#,
    )
    .unwrap();
    plan::compile(&def).unwrap()
}

/// Error string from parsing `json` as `T` (parse-time rejection sites).
fn parse_err<T: serde::de::DeserializeOwned>(json: &str) -> String {
    match serde_json::from_str::<T>(json) {
        Ok(_) => panic!("expected a parse error, got Ok: {json}"),
        Err(e) => e.to_string(),
    }
}

/// Error string from a GameDef that PARSES (its unknowns are stored) but
/// fails `plan::compile` — the `EventDef` walk.
fn gamedef_compile_err(json: &str) -> String {
    let def: GameDef = serde_json::from_str(json).unwrap();
    plan::compile(&def).unwrap_err().to_string()
}

/// Error string from a SimDef/Rotation pair that PARSES but fails
/// `sim::compile` — the SimDef-side walks.
fn sim_compile_err(simdef_json: &str, rotation_json: &str) -> String {
    let simdef: SimDef = serde_json::from_str(simdef_json).unwrap();
    let rotation: Rotation = serde_json::from_str(rotation_json).unwrap();
    sim::compile(&tiny_plan(), &simdef, &rotation)
        .unwrap_err()
        .to_string()
}

const EMPTY_ROTATION: &str = r#"{ "rules": [] }"#;

/// The number of config structs in the P8a fail-closed sweep. A task
/// adding a config struct bumps this AND adds the struct's row below —
/// the `cases.len()` assertion at the end of the table is what keeps the
/// two honest. (17th: P8b's `EffectDef`, a parse-time rejection site
/// like the hand-written mirrors. 18th: P8c's `SimDefaults` — the
/// `defaults` block — a stored-`extra` walk site like the rest of the
/// SimDef side.)
const CONFIG_STRUCT_COUNT: usize = 18;

#[test]
fn every_config_struct_rejects_a_typoed_key_with_its_context_named() {
    // (site, error, typo'd key, required context fragment) — one row per
    // config struct in the P8a sweep (`CONFIG_STRUCT_COUNT` of them,
    // asserted below the table).
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "GameDef",
            parse_err::<GameDef>(
                r#"{ "stats": [], "pipeline": [], "objectives": [], "statss": [] }"#,
            ),
            "statss",
            "the gamedef",
        ),
        (
            "BucketDef",
            parse_err::<GameDef>(
                r#"{ "stats": [], "buckets": { "add": { "fold": "sum", "flod": "sum" } },
                     "pipeline": [], "objectives": [] }"#,
            ),
            "flod",
            "a bucket definition",
        ),
        (
            "EventDef",
            gamedef_compile_err(
                r#"{ "stats": [],
                     "events": { "crit": { "chance": "0", "factor": "1", "factr": "1" } },
                     "pipeline": [ { "name": "hit", "expr": "1" } ],
                     "objectives": ["hit"] }"#,
            ),
            "factr",
            "event `crit`",
        ),
        (
            "StageDef",
            parse_err::<GameDef>(
                r#"{ "stats": [],
                     "pipeline": [ { "name": "hit", "expr": "1", "brnched": true } ],
                     "objectives": ["hit"] }"#,
            ),
            "brnched",
            "stage `hit`",
        ),
        (
            "BuildState",
            parse_err::<BuildState>(r#"{ "stat": { "w": 1.0 } }"#),
            "stat",
            "the build state",
        ),
        (
            "Contribution",
            parse_err::<BuildState>(
                r#"{ "contributions": [ { "bucket": "add", "value": 1.0, "evnt": "crit" } ] }"#,
            ),
            "evnt",
            "a contribution into bucket `add`",
        ),
        (
            "Scenario",
            parse_err::<Scenario>(r#"{ "phases": [], "phasez": [] }"#),
            "phasez",
            "the scenario",
        ),
        (
            "Phase",
            parse_err::<Scenario>(
                r#"{ "phases": [ { "name": "boss", "weight": 1, "stat": {} } ] }"#,
            ),
            "stat",
            "phase `boss`",
        ),
        (
            "SimDef",
            sim_compile_err(
                r#"{ "actons": {}, "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "actons",
            "the simdef",
        ),
        (
            "SimDefaults",
            sim_compile_err(
                r#"{ "defaults": { "measur": "cast_start" },
                     "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "measur",
            "the defaults block",
        ),
        (
            "ResourceDef",
            sim_compile_err(
                r#"{ "resources": { "mana": { "max": "1", "regen_per_sec": "0",
                                              "regen": "0" } },
                     "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "regen",
            "resource `mana`",
        ),
        (
            "ActionDef",
            sim_compile_err(
                r#"{ "actions": { "bolt": { "cast_time": "1", "cast_tim": "1" } },
                     "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "cast_tim",
            "action `bolt`",
        ),
        (
            "ActionDamage",
            sim_compile_err(
                r#"{ "actions": { "bolt": { "cast_time": "1", "damage": { "stat": {} } } },
                     "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "stat",
            "action `bolt` damage",
        ),
        (
            "BuffDef",
            sim_compile_err(
                r#"{ "buffs": { "chill": { "duration": 1.0, "durration": 2.0 } },
                     "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "durration",
            "buff `chill`",
        ),
        (
            "ProcDef",
            sim_compile_err(
                r#"{ "procs": { "lucky": { "trigger": "on_cast", "chance": "1",
                                           "chanse": "1" } },
                     "damage_objective": "hit" }"#,
                EMPTY_ROTATION,
            ),
            "chanse",
            "proc `lucky`",
        ),
        (
            "EffectDef",
            parse_err::<SimDef>(
                r#"{ "procs": { "lucky": { "trigger": "on_cast", "chance": "1",
                                           "effects": [ { "apply_buf": "x" } ] } },
                     "damage_objective": "hit" }"#,
            ),
            "apply_buf",
            "an effect entry",
        ),
        (
            "Rotation",
            sim_compile_err(
                r#"{ "damage_objective": "hit" }"#,
                r#"{ "rules": [], "ruless": [] }"#,
            ),
            "ruless",
            "the rotation",
        ),
        (
            "Rule",
            sim_compile_err(
                r#"{ "damage_objective": "hit" }"#,
                r#"{ "rules": [ { "action": "bolt", "wen": "1" } ] }"#,
            ),
            "wen",
            "rotation rule 0 (action `bolt`)",
        ),
    ];

    assert_eq!(
        cases.len(),
        CONFIG_STRUCT_COUNT,
        "one row per config struct in the sweep — a new config struct \
         needs a row here (and a bump of CONFIG_STRUCT_COUNT)"
    );

    for (site, err, key, context) in cases {
        assert!(
            err.contains(&format!("unknown field `{key}`")),
            "{site}: expected the typo'd key `{key}` in the error, got: {err}"
        );
        assert!(
            err.contains(context),
            "{site}: expected the context `{context}` in the error, got: {err}"
        );
    }
}

// ----------------------------------------------------------------------
// P8a follow-up — integer `NumOrExpr` literals, pinned. Every committed
// fixture writes `3.0`, so deleting the hand-written visitor's
// `visit_u64`/`visit_i64` arms survived the suite. `"cooldown": 3` is
// 0.3.0-compatible input (the untagged derive accepted it) and must keep
// parsing as `Num(3.0)` AND running identically to `3.0`.
// ----------------------------------------------------------------------

#[test]
fn integer_num_or_expr_literals_parse_and_behave_as_their_float_spelling() {
    use rtce::sim::Mode;

    // Parse: u64 and (negative) i64 JSON integers land in `Num`, as f64.
    let def: SimDef = serde_json::from_str(
        r#"{ "actions": { "bolt": { "cast_time": "1", "cooldown": 3,
                                    "damage": { "stats": { "w": -2 } } } },
             "damage_objective": "hit" }"#,
    )
    .unwrap();
    use rtce::simdef::NumOrExpr;
    assert_eq!(def.actions["bolt"].cooldown, NumOrExpr::Num(3.0));
    assert_eq!(
        def.actions["bolt"].damage.as_ref().unwrap().stats["w"],
        NumOrExpr::Num(-2.0)
    );

    // Behavior: the integer spelling runs byte-identically to the float
    // one. Hand-worked cadence for `"cooldown": 3` over 10s (cast_time 1,
    // cooldown armed at cast START): starts t=0,3,6,9 → completes
    // t=1,4,7,10 (the horizon is drained) → 4 casts, damage 4 × w.
    let plan = tiny_plan();
    let run_with = |cooldown_json: &str| {
        let simdef: SimDef = serde_json::from_str(&format!(
            r#"{{ "actions": {{ "bolt": {{ "cast_time": "1", "cooldown": {cooldown_json},
                                           "damage": {{ "stats": {{}} }} }} }},
                  "damage_objective": "hit" }}"#
        ))
        .unwrap();
        let rotation: Rotation =
            serde_json::from_str(r#"{ "rules": [ { "action": "bolt" } ] }"#).unwrap();
        let sim_plan = sim::compile(&plan, &simdef, &rotation).unwrap();
        let build: BuildState = serde_json::from_str(r#"{ "stats": { "w": 100.0 } }"#).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();
        sim::run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap()
    };
    let int_run = run_with("3");
    let float_run = run_with("3.0");
    assert_eq!(int_run.actions["bolt"].casts, 4);
    assert_eq!(int_run.total.total_damage, 400.0);
    assert_eq!(
        int_run.total.total_damage, float_run.total.total_damage,
        "an integer literal must reach the executor as the identical f64"
    );
    assert_eq!(
        int_run.actions["bolt"].casts,
        float_run.actions["bolt"].casts
    );
}
