//! The `_` annotation namespace pin (P8a). Unknown config keys fail
//! closed everywhere — EXCEPT keys starting with `_`, which are the
//! documented annotation namespace (`_source`, `_scope`, `_shape`, …) and
//! are accepted at every nesting level. The committed fixtures already
//! carry such keys, so "the fixtures still parse and compile" IS the pin:
//! remove the `_` exemption and this file goes red.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::scenario::Scenario;
use rtce::{plan, sim};
use std::path::PathBuf;

#[test]
fn the_committed_fixtures_with_underscore_annotations_parse_and_compile() {
    for sub in ["d4", "poe2", "toy"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(sub)
            .join("gamedef.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        // Guard the pin's premise: the fixture actually carries `_` keys
        // (top-level `_source` on d4/toy; `_scope`/`_shape`/`_crit`/
        // `_hits_per_use` on poe2). If a fixture edit dropped them, this
        // test would pass vacuously.
        assert!(
            raw.contains("\"_"),
            "premise: the {sub} fixture carries `_` annotation keys"
        );
        let def: GameDef = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{sub} fixture must PARSE with its annotations: {e}"));
        plan::compile(&def)
            .unwrap_or_else(|e| panic!("{sub} fixture must COMPILE with its annotations: {e}"));
    }
}

#[test]
fn underscore_keys_are_accepted_at_every_gamedef_and_scenario_nesting_level() {
    // GameDef: top level, bucket, event, stage.
    let def: GameDef = serde_json::from_str(
        r#"{
          "_source": "hand-written for the P8a namespace pin",
          "stats": ["w", "crit_chance"],
          "buckets": { "add": { "fold": "sum", "_why": "the additive pool" } },
          "events": { "crit": { "chance": "crit_chance / 100", "factor": "1.5",
                                "_baseline": "UNVERIFIED x1.5" } },
          "pipeline": [
            { "name": "hit", "expr": "(w + add) * event_factors", "branched": true,
              "_derivation": "base times the enumerated event factors" }
          ],
          "objectives": ["hit"]
        }"#,
    )
    .unwrap();
    let plan = plan::compile(&def).unwrap();

    // BuildState: top level and inside a contribution.
    let build: BuildState = serde_json::from_str(
        r#"{ "_fit": "budget build",
             "stats": { "w": 10.0 },
             "contributions": [
               { "bucket": "add", "value": 5.0, "_src": "affix 123" } ] }"#,
    )
    .unwrap();

    // Scenario: top level and inside a phase.
    let scenario: Scenario = serde_json::from_str(
        r#"{ "_playbook": "training dummy",
             "phases": [ { "name": "p", "weight": 1, "_span": "whole fight" } ] }"#,
    )
    .unwrap();

    // …and the annotated config still EVALUATES: crit chance 0 leaves the
    // single unfired branch, so hit = (10 + 5) × 1 = 15.
    let mut scratch = plan.scratch();
    let out = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
    assert!((out[0] - 15.0).abs() < 1e-9, "got {}", out[0]);
}

#[test]
fn underscore_keys_are_accepted_across_the_simdef_side_too() {
    // A minimal end-to-end slice: the SAME annotated GameDef, plus an
    // annotated SimDef/Rotation, through `sim::compile`. (Per-struct
    // coverage and round-trip survival live in `sim::compile`'s unit
    // tests; this is the integration-level "nothing in the chain trips
    // on an annotation" pin.)
    let def: GameDef = serde_json::from_str(
        r#"{ "_source": "P8a pin", "stats": ["w"],
             "pipeline": [ { "name": "hit", "expr": "w", "_note": "flat" } ],
             "objectives": ["hit"] }"#,
    )
    .unwrap();
    let plan = plan::compile(&def).unwrap();
    let simdef: rtce::simdef::SimDef = serde_json::from_str(
        r#"{ "_source": "P8a pin",
             "actions": { "bolt": { "cast_time": "1", "_rank": "1",
                                    "damage": { "stats": {}, "_note": "plain" } } },
             "damage_objective": "hit" }"#,
    )
    .unwrap();
    let rotation: rtce::simdef::Rotation = serde_json::from_str(
        r#"{ "_style": "spam", "rules": [ { "action": "bolt", "_prio": "only" } ] }"#,
    )
    .unwrap();
    sim::compile(&plan, &simdef, &rotation).unwrap();
}
