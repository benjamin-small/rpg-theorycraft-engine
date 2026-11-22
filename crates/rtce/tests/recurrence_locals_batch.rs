//! Ordered step locals (#29) and batched galloping transitions (#30) for
//! recurrence stages.

use rtce::build::BuildState;
use rtce::gamedef::GameDef;
use rtce::plan;
use rtce::scenario::Scenario;

fn empty_build() -> BuildState {
    serde_json::from_str("{}").unwrap()
}

fn scenario() -> Scenario {
    serde_json::from_str(r#"{ "phases": [{ "name": "test", "weight": 1 }] }"#).unwrap()
}

fn evaluate(def: &GameDef) -> Result<f64, plan::PlanError> {
    let plan = plan::compile(def)?;
    let mut scratch = plan.scratch();
    Ok(plan.evaluate(&empty_build(), &scenario(), &mut scratch)?[0])
}

fn def_with(stats: &[&str], recurrence: serde_json::Value) -> GameDef {
    serde_json::from_value(serde_json::json!({
        "stats": stats,
        "pipeline": [{ "name": "answer", "recurrence": recurrence }],
        "objectives": ["answer"]
    }))
    .unwrap()
}

fn compile_error(def: &GameDef) -> String {
    plan::compile(def).unwrap_err().what
}

// ── Locals (#29) ────────────────────────────────────────────────────────

/// A demand of 5 per step drains `a` (12) first, and the remainder spills
/// into `b` (7):
/// step 1: a 12→7; step 2: a 7→2; step 3: a takes 2, b takes 3 (b 7→4);
/// step 4: b takes 4 of 5 (b→0) with 1 left over → 4 − 1/5 = 3.8 steps.
fn cascade() -> serde_json::Value {
    serde_json::json!({
        "state": [
            { "name": "a", "initial": "12", "next": "a - take_a" },
            { "name": "b", "initial": "7", "next": "b - take_b" },
            { "name": "steps", "initial": "0", "next": "steps + 1" },
            { "name": "overkill", "initial": "0", "next": "rest - take_b" }
        ],
        "locals": [
            { "name": "take_a", "expr": "min(5, a)" },
            { "name": "rest", "expr": "5 - take_a" },
            { "name": "take_b", "expr": "min(rest, b)" }
        ],
        "until": "b <= 0",
        "result": "steps - overkill / 5",
        "max_iterations": 10
    })
}

#[test]
fn ordered_locals_cascade_a_demand_through_two_pools() {
    let got = evaluate(&def_with(&[], cascade())).unwrap();
    assert!((got - 3.8).abs() < 1e-12, "got {got}");
}

#[test]
fn locals_read_the_previous_state_and_updates_stay_simultaneous() {
    // `seen` reads the OLD `a`; `b.next` reads the local, so b tracks the
    // previous a, never the just-computed one: a 1,2,3 / b 0,1,2.
    let def = def_with(
        &[],
        serde_json::json!({
            "state": [
                { "name": "a", "initial": "1", "next": "a + 1" },
                { "name": "b", "initial": "0", "next": "seen" }
            ],
            "locals": [{ "name": "seen", "expr": "a" }],
            "until": "a >= 3",
            "result": "b",
            "max_iterations": 10
        }),
    );
    assert_eq!(evaluate(&def).unwrap(), 2.0);
}

#[test]
fn locals_may_read_stage_symbols() {
    let def = def_with(
        &["demand"],
        serde_json::json!({
            "state": [{ "name": "x", "initial": "10", "next": "x - d" }],
            "locals": [{ "name": "d", "expr": "demand + 1" }],
            "until": "x <= 0",
            "result": "x",
            "max_iterations": 10
        }),
    );
    let plan = plan::compile(&def).unwrap();
    let mut scratch = plan.scratch();
    let build: BuildState = serde_json::from_str(r#"{ "stats": { "demand": 3 } }"#).unwrap();
    // 10 → 6 → 2 → −2.
    assert_eq!(
        plan.evaluate(&build, &scenario(), &mut scratch).unwrap()[0],
        -2.0
    );
}

#[test]
fn locals_reject_forward_self_and_terminal_references_at_compile_time() {
    let mut forward = cascade();
    forward["locals"][0]["expr"] = serde_json::json!("min(5, a) + rest");
    let error = compile_error(&def_with(&[], forward));
    assert!(
        error.contains("recurrence stage `answer` local `take_a`")
            && error.contains("unknown identifier `rest`"),
        "got: {error}"
    );

    let mut self_ref = cascade();
    self_ref["locals"][1]["expr"] = serde_json::json!("rest");
    let error = compile_error(&def_with(&[], self_ref));
    assert!(
        error.contains("local `rest`") && error.contains("unknown identifier `rest`"),
        "got: {error}"
    );

    let mut until_ref = cascade();
    until_ref["until"] = serde_json::json!("take_b > 99");
    let error = compile_error(&def_with(&[], until_ref));
    assert!(
        error.contains("until") && error.contains("unknown identifier `take_b`"),
        "got: {error}"
    );

    let mut result_ref = cascade();
    result_ref["result"] = serde_json::json!("rest");
    let error = compile_error(&def_with(&[], result_ref));
    assert!(
        error.contains("result") && error.contains("unknown identifier `rest`"),
        "got: {error}"
    );
}

#[test]
fn local_names_are_validated_and_collision_checked() {
    let mut duplicate = cascade();
    duplicate["locals"][1]["name"] = serde_json::json!("take_a");
    assert!(compile_error(&def_with(&[], duplicate)).contains("duplicate local `take_a`"),);

    let mut state_clash = cascade();
    state_clash["locals"][0]["name"] = serde_json::json!("a");
    let error = compile_error(&def_with(&[], state_clash));
    assert!(error.contains("local `a` collides"), "got: {error}");

    let mut stat_clash = cascade();
    stat_clash["locals"][0]["name"] = serde_json::json!("demand");
    let error = compile_error(&def_with(&["demand"], stat_clash));
    assert!(error.contains("local `demand` collides"), "got: {error}");

    let mut engine_clash = cascade();
    engine_clash["locals"][0]["name"] = serde_json::json!("event_multiplier");
    let error = compile_error(&def_with(&[], engine_clash));
    assert!(
        error.contains("local `event_multiplier` collides"),
        "got: {error}"
    );

    let mut invalid = cascade();
    invalid["locals"][0]["name"] = serde_json::json!("not.a.name");
    let error = compile_error(&def_with(&[], invalid));
    assert!(error.contains("plain ASCII identifier"), "got: {error}");

    let locals: Vec<_> = (0..=plan::MAX_RECURRENCE_LOCALS)
        .map(|index| serde_json::json!({ "name": format!("l_{index}"), "expr": "0" }))
        .collect();
    let error = compile_error(&def_with(
        &[],
        serde_json::json!({
            "state": [{ "name": "x", "initial": "0", "next": "x" }],
            "locals": locals,
            "until": "1", "result": "x", "max_iterations": 1
        }),
    ));
    assert!(error.contains("recurrence locals > max"), "got: {error}");
}

#[test]
fn a_non_finite_local_names_stage_local_and_iteration() {
    // Transition 1 reads x = 2 (q = 1, x → 1); transition 2 reads x = 1.
    let error = evaluate(&def_with(
        &[],
        serde_json::json!({
            "state": [{ "name": "x", "initial": "2", "next": "x - 1 + 0 * q" }],
            "locals": [{ "name": "q", "expr": "1 / (x - 1)" }],
            "until": "x <= 0",
            "result": "x",
            "max_iterations": 10
        }),
    ))
    .unwrap_err();
    assert!(
        error
            .what
            .contains("recurrence stage `answer` local `q` is non-finite at iteration 2"),
        "got: {error}"
    );
}

#[test]
fn locals_and_batch_are_omitted_when_absent_and_round_trip_when_present() {
    let plain = def_with(
        &[],
        serde_json::json!({
            "state": [{ "name": "x", "initial": "0", "next": "x + 1" }],
            "until": "x >= 1", "result": "x", "max_iterations": 1
        }),
    );
    let encoded = serde_json::to_string(&plain).unwrap();
    assert!(
        !encoded.contains("locals") && !encoded.contains("batch"),
        "got: {encoded}"
    );

    let rich = def_with(
        &[],
        drain(10.0, Some(serde_json::json!({ "symbol": "n" })), 100),
    );
    let encoded = serde_json::to_value(&rich).unwrap();
    let recurrence = &encoded["pipeline"][0]["recurrence"];
    assert_eq!(recurrence["locals"][0]["name"], "spent");
    assert_eq!(recurrence["batch"]["symbol"], "n");
    // The default batch ceiling is materialized on serialization.
    assert_eq!(recurrence["batch"]["max_size"], 1_048_576);
    let decoded: GameDef = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        evaluate(&decoded).unwrap().to_bits(),
        evaluate(&rich).unwrap().to_bits()
    );

    let mut typo_local = cascade();
    typo_local["locals"][0] = serde_json::json!({ "name": "take_a", "exp": "1" });
    let error = serde_json::from_value::<GameDef>(serde_json::json!({
        "stats": [], "objectives": ["answer"],
        "pipeline": [{ "name": "answer", "recurrence": typo_local }]
    }))
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("recurrence local `take_a`") && error.contains("exp"),
        "got: {error}"
    );

    let mut typo_batch = drain(10.0, Some(serde_json::json!({ "symbol": "n" })), 100);
    typo_batch["batch"] = serde_json::json!({ "symbol": "n", "max": 4 });
    let error = serde_json::from_value::<GameDef>(serde_json::json!({
        "stats": [], "objectives": ["answer"],
        "pipeline": [{ "name": "answer", "recurrence": typo_batch }]
    }))
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("a recurrence batch") && error.contains("max"),
        "got: {error}"
    );
}

// ── Batched galloping transitions (#30) ─────────────────────────────────

/// A pool drained 3 per logical step, written for `n` steps per transition
/// when `batch` is present (n is 1 otherwise). Fractional final step:
/// `pool / 3`.
fn drain(pool: f64, batch: Option<serde_json::Value>, max_iterations: u32) -> serde_json::Value {
    let n = if batch.is_some() { "n" } else { "1" };
    let mut recurrence = serde_json::json!({
        "state": [
            { "name": "remaining", "initial": pool.to_string(), "next": "max(remaining - spent, 0)" },
            { "name": "steps", "initial": "0", "next": format!("steps + {n}") },
            { "name": "overkill", "initial": "0", "next": "max(spent - remaining, 0)" }
        ],
        "locals": [{ "name": "spent", "expr": format!("3 * {n}") }],
        "until": "remaining <= 0",
        "result": "steps - overkill / 3",
        "max_iterations": max_iterations
    });
    if let Some(batch) = batch {
        recurrence["batch"] = batch;
    }
    recurrence
}

#[test]
fn batching_reaches_a_million_unit_pool_in_37_transitions() {
    let batch = || Some(serde_json::json!({ "symbol": "n" }));
    // 1,000,000 / 3 = 333,334 logical steps. Galloping: 18 accepted growth
    // candidates (1 … 2^17), one rejected 2^18, then 18 descent candidates
    // (2^17 … 1): 37 transitions exactly.
    let got = evaluate(&def_with(&[], drain(1_000_000.0, batch(), 37))).unwrap();
    assert!((got - 1_000_000.0 / 3.0).abs() < 1e-6, "got {got}");
    let error = evaluate(&def_with(&[], drain(1_000_000.0, batch(), 36))).unwrap_err();
    assert!(
        error
            .what
            .contains("did not terminate within 36 iterations"),
        "got: {error}"
    );
}

#[test]
fn batching_is_bit_identical_to_single_stepping_on_an_exact_batched_step() {
    for pool in [1.0, 2.0, 3.0, 10.0, 1000.0, 12_345.0] {
        let single = evaluate(&def_with(&[], drain(pool, None, 100_000))).unwrap();
        let batched = evaluate(&def_with(
            &[],
            drain(pool, Some(serde_json::json!({ "symbol": "n" })), 100),
        ))
        .unwrap();
        assert_eq!(single.to_bits(), batched.to_bits(), "pool {pool}");
    }
}

#[test]
fn max_size_caps_the_batch() {
    let capped = |max_size: u32, budget: u32| {
        evaluate(&def_with(
            &[],
            drain(
                1000.0,
                Some(serde_json::json!({ "symbol": "n", "max_size": max_size })),
                budget,
            ),
        ))
    };
    // 334 logical steps: 17 transitions uncapped, 87 at max_size 4, and
    // max_size 1 is plain single stepping (334).
    assert!(capped(1 << 20, 17).is_ok());
    assert!(capped(1 << 20, 16).is_err());
    assert!(capped(4, 87).is_ok());
    assert!(capped(4, 86).is_err());
    assert!(capped(1, 334).is_ok());
    assert!(capped(1, 333).is_err());
}

#[test]
fn without_batch_every_transition_is_one_step() {
    assert!(evaluate(&def_with(&[], drain(1000.0, None, 334))).is_ok());
    let error = evaluate(&def_with(&[], drain(1000.0, None, 333))).unwrap_err();
    assert!(
        error.what.contains("did not terminate within 333"),
        "got: {error}"
    );
}

#[test]
fn batch_symbol_is_scoped_and_validated_at_compile_time() {
    let batched = |batch: serde_json::Value| def_with(&["demand"], drain(10.0, Some(batch), 100));

    let mut until_ref = drain(10.0, Some(serde_json::json!({ "symbol": "n" })), 100);
    until_ref["until"] = serde_json::json!("remaining <= n");
    let error = compile_error(&def_with(&[], until_ref));
    assert!(
        error.contains("until") && error.contains("unknown identifier `n`"),
        "got: {error}"
    );

    let mut result_ref = drain(10.0, Some(serde_json::json!({ "symbol": "n" })), 100);
    result_ref["result"] = serde_json::json!("n");
    let error = compile_error(&def_with(&[], result_ref));
    assert!(
        error.contains("result") && error.contains("unknown identifier `n`"),
        "got: {error}"
    );

    for clash in ["remaining", "spent", "demand", "event_factors"] {
        let error = compile_error(&batched(serde_json::json!({ "symbol": clash })));
        assert!(
            error.contains(&format!("batch symbol `{clash}` collides")),
            "{clash}: {error}"
        );
    }

    let error = compile_error(&batched(serde_json::json!({ "symbol": "2n" })));
    assert!(error.contains("plain ASCII identifier"), "got: {error}");

    for max_size in [0u64, (1 << 30) + 1] {
        let error = compile_error(&batched(
            serde_json::json!({ "symbol": "n", "max_size": max_size }),
        ));
        assert!(
            error.contains("max_size must be in 1..="),
            "{max_size}: {error}"
        );
    }
}

#[test]
fn batch_symbol_is_visible_to_locals_and_next() {
    // The local reads n; next reads it directly too: steps advance by n.
    let def = def_with(
        &[],
        serde_json::json!({
            "state": [
                { "name": "steps", "initial": "0", "next": "steps + n" },
                { "name": "doubled", "initial": "0", "next": "doubled + twice" }
            ],
            "locals": [{ "name": "twice", "expr": "2 * n" }],
            "until": "steps >= 100",
            "result": "doubled",
            "batch": { "symbol": "n" },
            "max_iterations": 100
        }),
    );
    assert_eq!(evaluate(&def).unwrap(), 200.0);
}
