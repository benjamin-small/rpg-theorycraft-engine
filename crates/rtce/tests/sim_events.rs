use rtce::plan::{self, PlanError};
use rtce::sim::{self, Mode, SimEvent};
use serde_json::{json, Value};

const GAME: &str = r#"{
    "stats": ["hits_per_use"],
    "events": { "crit": { "chance": "0.5", "factor": "2" } },
    "pipeline": [
        { "name": "hit", "expr": "10 * event_multiplier", "branched": true },
        { "name": "tick", "expr": "3" }
    ],
    "objectives": ["hit", "tick"]
}"#;

fn run(
    simdef: &str,
    rotation: &str,
    scenario: Value,
    mode: Mode,
    iteration: u32,
    sink: impl FnMut(SimEvent<'_>),
) -> Result<(Value, Value), PlanError> {
    let plan = plan::compile(&serde_json::from_str(GAME).unwrap()).unwrap();
    let sim_plan = sim::compile(
        &plan,
        &serde_json::from_str(simdef).unwrap(),
        &serde_json::from_str(rotation).unwrap(),
    )
    .unwrap();
    let scenario = serde_json::from_value(scenario).unwrap();
    let build = serde_json::from_value(json!({"stats": {"hits_per_use": 1}})).unwrap();
    let traced = sim::run_with_events(&plan, &sim_plan, &build, &scenario, mode, iteration, sink)?;
    let plain = sim::run(&plan, &sim_plan, &build, &scenario, mode)?;
    Ok((json!(traced), json!(plain)))
}

#[test]
fn events_follow_the_executor_through_a_cancel_and_buff_removal() {
    let simdef = r#"{
      "defaults": { "gcd": 1 },
      "resources": { "mana": { "max": "10", "regen_per_sec": "0" } },
      "actions": {
        "prime": {
          "cast_time": "1", "cost": { "mana": 2 }, "gain": { "mana": 1 }, "cooldown": 9,
          "effects": [{ "apply_buff": "mark" }]
        },
        "channel": { "segments": [{ "duration": 1, "damage": {} }, { "duration": 1, "damage": {} }] },
        "consume": { "cast_time": "0", "damage": {}, "effects": [{ "remove_buff": "mark" }] }
      },
      "buffs": { "mark": { "duration": 2, "tick_objective": "tick" } },
      "damage_objective": "hit"
    }"#;
    let rotation = r#"{ "rules": [
      { "action": "consume", "when": "segments_done == 1" },
      { "action": "channel", "when": "and(casts.prime > 0, casts.channel == 0)", "interrupt": true },
      { "action": "prime" }
    ] }"#;
    let scenario =
        json!({ "phases": [{ "name": "first", "weight": 2 }, { "name": "second", "weight": 2 }] });
    let mut events = Vec::new();
    let (traced, plain) = run(simdef, rotation, scenario, Mode::Expected, 0, |event| {
        events.push(json!(event));
    })
    .unwrap();

    // At t=2 the phase was scheduled first. The channel's first landing
    // then cancels into the instant spender, which deals damage before removing the buff.
    let order: Vec<_> = events
        .iter()
        .map(|event| {
            (
                event["time"].as_f64().unwrap(),
                event["type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            (0.0, "simulation_started"),
            (0.0, "phase_entered"),
            (0.0, "resource_changed"),
            (0.0, "cast_started"),
            (0.0, "resource_changed"),
            (1.0, "resource_changed"),
            (1.0, "cast_landed"),
            (1.0, "buff_changed"),
            (1.0, "cast_started"),
            (1.0, "segment_started"),
            (2.0, "buff_damage"),
            (2.0, "phase_entered"),
            (2.0, "cast_landed"),
            (2.0, "cast_cancelled"),
            (2.0, "cast_started"),
            (2.0, "cast_landed"),
            (2.0, "buff_changed"),
            (4.0, "simulation_ended"),
        ]
    );
    let participants: Vec<_> = events
        .iter()
        .map(|event| (event["source_id"].as_str(), event["target_id"].as_str()))
        .collect();
    let player = Some("player");
    let target = Some("target");
    assert_eq!(
        participants,
        [
            (None, None),     // Start.
            (None, None),     // Initial phase.
            (None, player),   // Initial resource.
            (player, None),   // Cast start.
            (player, player), // Cost.
            (player, player), // Gain.
            (player, None),   // Utility landing.
            (player, None),   // Buff application.
            (player, None),   // Next cast.
            (player, None),   // Segment start.
            (player, target), // Buff damage.
            (None, None),     // Phase boundary.
            (player, target), // Segment damage.
            (player, None),   // Cancel.
            (player, None),   // Replacement cast.
            (player, target), // Replacement damage.
            (player, None),   // Buff removal.
            (None, None),     // End.
        ]
    );
    assert_eq!(events[3]["gcd_ready_at"], 1.0);
    assert_eq!(events[4]["amount"], 8.0);
    assert_eq!(events[5]["amount"], 9.0);
    assert_eq!(
        events[10],
        json!({
            "type": "buff_damage", "time": 2.0, "source_id": "player", "target_id": "target",
            "buff": "mark", "since": 1.0, "damage": 3.0
        })
    );
    assert_eq!(events[12]["cast_id"], 2);
    assert_eq!(events[12]["segment"], 0);
    assert_eq!(events[13]["cast_id"], 2);
    assert_eq!(events[13]["segments_landed"], 1);
    assert_eq!(events[15]["damage"], 15.0);
    assert_eq!(events[16]["change"], "removed");
    assert_eq!(events[16]["cast_id"], 3);
    assert_eq!(events[17]["total_damage"], 33.0);
    assert_eq!(traced, plain);
}

#[test]
fn sampled_events_select_the_requested_iteration_without_changing_the_report() {
    let simdef = r#"{
      "actions": {
        "shot": { "cast_time": "1", "damage": { "stats": { "hits_per_use": 2 } }, "crit_rolls": "per_hit" },
        "echo": { "cast_time": "0", "damage": {} }
      },
      "procs": {
        "repeat": { "trigger": "on_cast", "chance": "1", "actions": ["shot"],
                    "effects": [{ "cast_action": "echo" }] }
      },
      "damage_objective": "hit"
    }"#;
    let rotation = r#"{ "rules": [{ "action": "shot" }] }"#;
    let scenario = json!({ "phases": [{ "name": "dummy", "weight": 16 }] });
    let mode = Mode::MonteCarlo {
        iterations: 3,
        seed: 7,
    };
    let collect = |iteration| {
        let mut events = Vec::new();
        let (traced, plain) = run(
            simdef,
            rotation,
            scenario.clone(),
            mode,
            iteration,
            |event| {
                events.push(json!(event));
            },
        )
        .unwrap();
        assert_eq!(traced, plain);
        assert_eq!(traced["distribution"]["iterations"], 3);
        events
    };
    let selected = collect(1);
    assert_eq!(selected, collect(1));
    assert_ne!(selected, collect(0));
    assert_eq!(selected[3]["crit"]["sampled_hits"], 2);
    assert_eq!(selected[4]["type"], "proc_fired");
    assert_eq!(selected[4]["proc"], "repeat");
    assert_eq!(selected[4]["cast_id"], 1);
    assert_eq!(selected[5]["triggered_by"], "repeat");
    assert_eq!(selected[5]["cast_id"], 2);
    assert_eq!(selected[6]["crit"], Value::Null);
    let total = selected
        .iter()
        .filter_map(|event| event["damage"].as_f64())
        .sum::<f64>();
    assert_eq!(selected.last().unwrap()["total_damage"], total);

    let error = run(simdef, rotation, scenario, mode, 3, |_| {
        panic!("bad iteration emitted an event")
    })
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("event iteration 3 is out of range"));
}

#[test]
fn scheduled_expiry_and_expected_uptime_age_out_are_distinct() {
    let simdef = r#"{
      "actions": {
        "apply": { "cast_time": "1", "cooldown": 10, "effects": [{ "apply_buff": "solid" }] }
      },
      "buffs": {
        "solid": { "duration": 1 },
        "weighted": { "duration": 1, "ev_uptime": "expected" }
      },
      "procs": {
        "chance": { "trigger": "on_cast", "chance": "0.25", "apply_buff": "weighted" }
      },
      "damage_objective": "hit"
    }"#;
    let rotation = r#"{ "rules": [{ "action": "apply" }] }"#;
    let scenario = json!({ "phases": [{ "name": "dummy", "weight": 3 }] });
    let mut changes = Vec::new();
    let (traced, plain) = run(simdef, rotation, scenario, Mode::Expected, 0, |event| {
        if matches!(
            event.kind,
            sim::SimEventKind::BuffChanged { .. } | sim::SimEventKind::ExpectedBuffChanged { .. }
        ) {
            changes.push(json!(event));
        }
    })
    .unwrap();
    assert_eq!(traced, plain);
    assert_eq!(changes.len(), 4);
    assert_eq!(changes[0]["change"], "applied");
    assert_eq!(changes[1]["probability"], 0.25);
    assert_eq!(changes[1]["roll_expires_at"], 2.0);
    assert_eq!(changes[2]["change"], "expired");
    assert_eq!(changes[2]["cast_id"], Value::Null);
    assert_eq!(changes[3]["time"], 2.0);
    assert_eq!(changes[3]["probability"], 0.0);
    assert_eq!(changes[3]["roll_chance"], Value::Null);
}
