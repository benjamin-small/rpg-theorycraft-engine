use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn rtce() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtce"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rtce/tests/fixtures/guide")
        .join(name)
}

#[test]
fn bundled_calc_demo_emits_named_versioned_json() {
    let output = rtce().args(["--compact", "demo", "calc"]).output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["kind"], "evaluation");
    assert_eq!(value["objectives"]["hit_after_armor"], 171.12);
}

#[test]
fn evaluate_reads_the_three_config_files() {
    let output = rtce()
        .arg("evaluate")
        .arg("--game")
        .arg(fixture("01-gamedef.json"))
        .arg("--build")
        .arg(fixture("01-build.json"))
        .arg("--scenario")
        .arg(fixture("01-scenario.json"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["objectives"]["hit"], 120.0);
}

#[test]
fn a_bad_path_is_a_clean_error_and_nonzero_exit() {
    let output = rtce()
        .args([
            "evaluate",
            "--game",
            "/definitely/missing.json",
            "--build",
            "/also/missing.json",
            "--scenario",
            "/still/missing.json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("could not read gamedef"), "got: {stderr}");
}

#[test]
fn lexicon_labels_engine_supplied_names() {
    let output = rtce().args(["--compact", "lexicon"]).output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["kind"], "lexicon");
    let entries = value["entries"].as_array().unwrap();
    assert!(entries
        .iter()
        .any(|entry| { entry["term"] == "event_multiplier" && entry["kind"] == "engine" }));
}

fn simulate_archer() -> Command {
    let mut command = rtce();
    command.arg("simulate");
    for (flag, file) in [
        ("--game", "07-gamedef.json"),
        ("--build", "07-build.json"),
        ("--scenario", "07-scenario.json"),
        ("--sim", "07-simdef.json"),
        ("--rotation", "07-rotation.json"),
    ] {
        command.arg(flag).arg(fixture(file));
    }
    command
}

#[test]
fn simulate_streams_one_sampled_timeline_as_jsonl() {
    let mut command = simulate_archer();
    command.args([
        "--mode",
        "monte-carlo",
        "--iterations",
        "2",
        "--seed",
        "18446744073709551615",
    ]);
    let plain = command.output().unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    let plain: Value = serde_json::from_slice(&plain.stdout).unwrap();
    assert_eq!(plain["report"]["distribution"]["iterations"], 2);

    let traced = command.arg("--events=1").output().unwrap();
    assert!(
        traced.status.success(),
        "{}",
        String::from_utf8_lossy(&traced.stderr)
    );
    assert!(traced.stdout.ends_with(b"\n"));
    let events: Vec<Value> = std::str::from_utf8(&traced.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let start = &events[0];
    assert_eq!(start["schema_version"], 1);
    assert_eq!(start["mode"], "monte_carlo");
    assert_eq!(start["iteration"], 1);
    assert_eq!(start["seed"], "18446744073709551615");
    assert_eq!(start["type"], "simulation_started");
    assert_eq!(start["time"], 0.0);
    assert_eq!(events.last().unwrap()["type"], "simulation_ended");
    assert!(events.iter().all(|event| event["time"].is_number()));
    let compact = command.arg("--compact").output().unwrap();
    assert!(compact.status.success());
    assert_eq!(traced.stdout, compact.stdout);
}

#[test]
fn simulate_events_defaults_to_iteration_zero_and_rejects_an_invalid_iteration() {
    let traced = simulate_archer().arg("--events").output().unwrap();
    assert!(traced.status.success());
    let first_line = traced.stdout.split(|byte| *byte == b'\n').next().unwrap();
    let start: Value = serde_json::from_slice(first_line).unwrap();
    assert_eq!(start["mode"], "expected");
    assert_eq!(start["iteration"], 0);
    assert_eq!(start["seed"], Value::Null);

    let invalid = simulate_archer().arg("--events=1").output().unwrap();
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("event iteration 1 is out of range"));
}
