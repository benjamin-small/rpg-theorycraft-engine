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
