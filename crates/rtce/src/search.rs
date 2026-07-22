//! Search module — PRICING only. An external driver proposes candidates
//! (as reversible `Move` sequences over a baseline `BuildState`); this
//! module applies them, evaluates them per scenario, and ranks the
//! results. It never generates moves itself — that is the driver's job.
//! Interfaces are serialization-friendly (serde) so a driver can live
//! out-of-process (a separate binary, a service, a notebook).

use crate::build::{BuildState, Contribution};
use crate::plan::{EvalScratch, Plan, PlanError};
use crate::scenario::Scenario;

/// A reversible `BuildState` mutation. Names resolve at apply time against
/// the `Plan` in force — an unknown name is a fail-closed error, never a
/// silent no-op.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum Move {
    /// Overwrite one stat's value.
    SetStat {
        /// The stat to overwrite (must exist in the plan's registry).
        stat: String,
        /// The new value.
        value: f64,
    },
    /// Append a contribution to the build.
    AddContribution {
        /// The contribution to add.
        contribution: Contribution,
    },
    /// Remove the FIRST contribution matching bucket+value(+tags) exactly.
    RemoveContribution {
        /// The contribution to match and remove (bucket, value, event,
        /// and condition must all match exactly).
        contribution: Contribution,
    },
}

/// One priceable candidate: an id plus the reversible `Move`s that turn
/// the baseline `BuildState` into this candidate's build.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    /// Caller-assigned identifier, echoed back in `CandidateResult::id`.
    pub id: String,
    /// Moves applied, in order, to a fresh copy of the baseline build.
    pub moves: Vec<Move>,
}

/// One candidate's priced objectives, across every scenario it was
/// evaluated against.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CandidateResult {
    /// Echoes the `Candidate::id` this result was priced from.
    pub id: String,
    /// `objectives[scenario_index][objective_index]`
    pub objectives: Vec<Vec<f64>>,
}

fn contribution_matches(a: &Contribution, b: &Contribution) -> bool {
    a.bucket == b.bucket && a.value == b.value && a.event == b.event && a.condition == b.condition
}

fn apply_move(plan: &Plan, build: &mut BuildState, mv: &Move) -> Result<(), PlanError> {
    match mv {
        Move::SetStat { stat, value } => {
            plan.stat_id(stat).ok_or_else(|| PlanError {
                what: format!("unknown stat `{stat}`"),
            })?;
            build.stats.insert(stat.clone(), *value);
            Ok(())
        }
        Move::AddContribution { contribution } => {
            build.contributions.push(contribution.clone());
            Ok(())
        }
        Move::RemoveContribution { contribution } => {
            let pos = build
                .contributions
                .iter()
                .position(|c| contribution_matches(c, contribution));
            match pos {
                Some(i) => {
                    build.contributions.remove(i);
                    Ok(())
                }
                None => Err(PlanError {
                    what: "no matching contribution to remove".into(),
                }),
            }
        }
    }
}

/// Price every candidate against every scenario. `base` is NOT mutated;
/// each candidate applies its moves to a fresh working copy, evaluates
/// per scenario, then the working copy is dropped. Errors are per-call
/// fail-closed: one bad candidate/move fails the WHOLE call — the driver
/// must send valid candidates.
pub fn price(
    plan: &Plan,
    base: &BuildState,
    candidates: &[Candidate],
    scenarios: &[Scenario],
    scratch: &mut EvalScratch,
) -> Result<Vec<CandidateResult>, PlanError> {
    let mut out = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let mut working = base.clone();
        for mv in &candidate.moves {
            apply_move(plan, &mut working, mv)?;
        }
        let mut objectives = Vec::with_capacity(scenarios.len());
        for scenario in scenarios {
            let obj = plan.evaluate(&working, scenario, scratch)?;
            objectives.push(obj.to_vec());
        }
        out.push(CandidateResult {
            id: candidate.id.clone(),
            objectives,
        });
    }
    Ok(out)
}

/// Indices of the k best results by (scenario_idx, objective_idx),
/// descending. Ties are broken by ascending index (stable sort). NaN
/// objectives (IEEE division, e.g. a zero-weight scenario or a 0/0 stage,
/// can produce one) compare as equal to everything and so rank
/// arbitrarily among themselves; drivers should validate objectives
/// before ranking if that matters to them.
pub fn top_k(
    results: &[CandidateResult],
    scenario: usize,
    objective: usize,
    k: usize,
) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..results.len()).collect();
    idx.sort_by(|&a, &b| {
        let va = results[a].objectives[scenario][objective];
        let vb = results[b].objectives[scenario][objective];
        vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.truncate(k);
    idx
}

/// Indices of the Pareto-optimal results over the given (scenario,
/// objective) axes, maximizing all axes. `a` dominates `b` iff `a` is `>=`
/// `b` on every axis and strictly `>` on at least one; a result survives
/// iff no other result dominates it. Deterministic order (ascending index).
pub fn pareto(results: &[CandidateResult], axes: &[(usize, usize)]) -> Vec<usize> {
    let val = |i: usize, ax: &(usize, usize)| results[i].objectives[ax.0][ax.1];
    let dominates = |a: usize, b: usize| {
        let mut any_greater = false;
        for ax in axes {
            let va = val(a, ax);
            let vb = val(b, ax);
            if va < vb {
                return false;
            }
            if va > vb {
                any_greater = true;
            }
        }
        any_greater
    };
    (0..results.len())
        .filter(|&i| !(0..results.len()).any(|j| j != i && dominates(j, i)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{BuildState, Contribution};
    use crate::gamedef::GameDef;
    use crate::plan;
    use crate::scenario::Scenario;

    // Copied from plan.rs's toy fixtures (kept tiny and local by design —
    // this module tests through the public API only).
    fn toy_def() -> GameDef {
        serde_json::from_str(
            r#"{
              "stats": ["weapon", "power", "crit_chance", "enemy_dr"],
              "conditions": ["enraged"],
              "buckets": { "additive": { "fold": "sum" },
                           "crit_group": { "fold": "summed_group" },
                           "indep": { "fold": "product" } },
              "events": { "crit": { "chance": "crit_chance / 100",
                                     "factor": "1.5 * crit_group" } },
              "pipeline": [
                { "name": "base", "expr": "weapon * (1 + power / 100)" },
                { "name": "hit",
                  "expr": "base * (1 + additive / 100) * event_factors * indep",
                  "branched": true },
                { "name": "dps", "expr": "hit * (1 - enemy_dr / 100)" }
              ],
              "objectives": ["dps"]
            }"#,
        )
        .unwrap()
    }

    fn toy_build() -> BuildState {
        serde_json::from_str(
            r#"{ "stats": { "weapon": 100.0, "power": 50.0, "crit_chance": 25.0 },
                 "contributions": [
                   { "bucket": "additive", "value": 40.0 },
                   { "bucket": "additive", "value": 30.0, "event": "crit" },
                   { "bucket": "additive", "value": 20.0, "condition": "enraged" },
                   { "bucket": "crit_group", "value": 50.0 },
                   { "bucket": "indep", "value": 10.0 } ] }"#,
        )
        .unwrap()
    }

    fn arena() -> Scenario {
        serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 1,
                   "uptimes": { "enraged": 0.5 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap()
    }

    #[test]
    fn price_matches_fresh_evaluation() {
        let plan = plan::compile(&toy_def()).unwrap();
        let base = toy_build();
        let scenarios = vec![arena()];

        let candidates = vec![
            Candidate {
                id: "set_stat".into(),
                moves: vec![Move::SetStat {
                    stat: "crit_chance".into(),
                    value: 100.0,
                }],
            },
            Candidate {
                id: "add_contrib".into(),
                moves: vec![Move::AddContribution {
                    contribution: Contribution {
                        bucket: "crit_group".into(),
                        value: 50.0,
                        event: None,
                        condition: None,
                    },
                }],
            },
            Candidate {
                id: "remove_contrib".into(),
                moves: vec![Move::RemoveContribution {
                    contribution: Contribution {
                        bucket: "indep".into(),
                        value: 10.0,
                        event: None,
                        condition: None,
                    },
                }],
            },
        ];

        let mut scratch = plan.scratch();
        let results = price(&plan, &base, &candidates, &scenarios, &mut scratch).unwrap();
        assert_eq!(results.len(), 3);

        // Expected 1: SetStat crit_chance 100.
        let mut expected1 = base.clone();
        expected1.stats.insert("crit_chance".into(), 100.0);
        let mut s = plan.scratch();
        let obj1 = plan
            .evaluate(&expected1, &arena(), &mut s)
            .unwrap()
            .to_vec();
        assert_eq!(results[0].id, "set_stat");
        assert_eq!(results[0].objectives, vec![obj1]);

        // Expected 2: AddContribution crit_group +50.
        let mut expected2 = base.clone();
        expected2.contributions.push(Contribution {
            bucket: "crit_group".into(),
            value: 50.0,
            event: None,
            condition: None,
        });
        let mut s = plan.scratch();
        let obj2 = plan
            .evaluate(&expected2, &arena(), &mut s)
            .unwrap()
            .to_vec();
        assert_eq!(results[1].objectives, vec![obj2]);

        // Expected 3: RemoveContribution of the indep +10.
        let mut expected3 = base.clone();
        let pos = expected3
            .contributions
            .iter()
            .position(|c| c.bucket == "indep" && c.value == 10.0)
            .unwrap();
        expected3.contributions.remove(pos);
        let mut s = plan.scratch();
        let obj3 = plan
            .evaluate(&expected3, &arena(), &mut s)
            .unwrap()
            .to_vec();
        assert_eq!(results[2].objectives, vec![obj3]);
    }

    #[test]
    fn baseline_is_never_mutated() {
        let plan = plan::compile(&toy_def()).unwrap();
        let base = toy_build();
        let pristine = serde_json::to_string(&base).unwrap();
        let scenarios = vec![arena()];
        let candidates = vec![
            Candidate {
                id: "a".into(),
                moves: vec![Move::SetStat {
                    stat: "crit_chance".into(),
                    value: 999.0,
                }],
            },
            Candidate {
                id: "b".into(),
                moves: vec![Move::AddContribution {
                    contribution: Contribution {
                        bucket: "additive".into(),
                        value: 5.0,
                        event: None,
                        condition: None,
                    },
                }],
            },
            Candidate {
                id: "c".into(),
                moves: vec![Move::RemoveContribution {
                    contribution: Contribution {
                        bucket: "indep".into(),
                        value: 10.0,
                        event: None,
                        condition: None,
                    },
                }],
            },
        ];
        let mut scratch = plan.scratch();
        let _ = price(&plan, &base, &candidates, &scenarios, &mut scratch).unwrap();
        assert_eq!(
            serde_json::to_string(&base).unwrap(),
            pristine,
            "base must be untouched"
        );
    }

    #[test]
    fn bad_moves_fail_closed() {
        let plan = plan::compile(&toy_def()).unwrap();
        let base = toy_build();
        let scenarios = vec![arena()];
        let mut scratch = plan.scratch();

        let bad_stat = vec![Candidate {
            id: "x".into(),
            moves: vec![Move::SetStat {
                stat: "mystery".into(),
                value: 1.0,
            }],
        }];
        let err = price(&plan, &base, &bad_stat, &scenarios, &mut scratch).unwrap_err();
        assert!(err.what.contains("mystery"), "got: {}", err.what);

        let bad_remove = vec![Candidate {
            id: "y".into(),
            moves: vec![Move::RemoveContribution {
                contribution: Contribution {
                    bucket: "indep".into(),
                    value: 999.0,
                    event: None,
                    condition: None,
                },
            }],
        }];
        let err = price(&plan, &base, &bad_remove, &scenarios, &mut scratch).unwrap_err();
        assert!(
            err.what.contains("no matching contribution"),
            "got: {}",
            err.what
        );
    }

    fn synthetic_results() -> Vec<CandidateResult> {
        // Single scenario, two objectives. r0/r1 incomparable; r1/r2 tie
        // (neither dominates); r3 dominated by both r0 and r1.
        vec![
            CandidateResult {
                id: "r0".into(),
                objectives: vec![vec![10.0, 5.0]],
            },
            CandidateResult {
                id: "r1".into(),
                objectives: vec![vec![8.0, 8.0]],
            },
            CandidateResult {
                id: "r2".into(),
                objectives: vec![vec![8.0, 8.0]],
            },
            CandidateResult {
                id: "r3".into(),
                objectives: vec![vec![3.0, 3.0]],
            },
        ]
    }

    #[test]
    fn top_k_and_pareto_hand_worked() {
        let results = synthetic_results();

        // objective 0 values: [10, 8, 8, 3] -> best is r0, then r1/r2 tie
        // (stable sort keeps ascending index order for ties).
        assert_eq!(top_k(&results, 0, 0, 2), vec![0, 1]);
        assert_eq!(top_k(&results, 0, 0, 4), vec![0, 1, 2, 3]);
        assert_eq!(top_k(&results, 0, 0, 0), Vec::<usize>::new());

        // objective 1 values: [5, 8, 8, 3] -> best is r1/r2 tie, then r0.
        assert_eq!(top_k(&results, 0, 1, 1), vec![1]);

        // Pareto over both axes: r3 is strictly dominated by r0 and r1
        // (both axes >=, at least one axis >), so it's excluded. r0 and r1
        // are incomparable (neither >= the other on both axes). r1 and r2
        // are equal, so neither dominates the other — both survive.
        let front = pareto(&results, &[(0, 0), (0, 1)]);
        assert_eq!(front, vec![0, 1, 2]);
    }

    #[test]
    fn moves_round_trip_serde() {
        let mv = Move::SetStat {
            stat: "crit_chance".into(),
            value: 42.0,
        };
        let json = serde_json::to_value(&mv).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"op": "set_stat", "stat": "crit_chance", "value": 42.0})
        );
        let back: Move = serde_json::from_value(json).unwrap();
        match back {
            Move::SetStat { stat, value } => {
                assert_eq!(stat, "crit_chance");
                assert_eq!(value, 42.0);
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let mv2 = Move::AddContribution {
            contribution: Contribution {
                bucket: "additive".into(),
                value: 10.0,
                event: None,
                condition: None,
            },
        };
        let json2 = serde_json::to_value(&mv2).unwrap();
        assert_eq!(json2["op"], "add_contribution");
        let back2: Move = serde_json::from_value(json2).unwrap();
        assert!(matches!(back2, Move::AddContribution { .. }));

        let mv3 = Move::RemoveContribution {
            contribution: Contribution {
                bucket: "additive".into(),
                value: 10.0,
                event: None,
                condition: None,
            },
        };
        let json3 = serde_json::to_value(&mv3).unwrap();
        assert_eq!(json3["op"], "remove_contribution");
        let back3: Move = serde_json::from_value(json3).unwrap();
        assert!(matches!(back3, Move::RemoveContribution { .. }));
    }
}
