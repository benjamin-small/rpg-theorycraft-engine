//! GameDef — the game's ALGORITHM as configuration: stat registry, bucket
//! fold declarations, probabilistic events, and the pipeline of stages.
//! Compiled once by `plan::compile`; never touched on the hot path.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameDef {
    /// Stat registry: names become slot offsets, in this order.
    pub stats: Vec<String>,
    /// Condition registry (uptime-gated contribution tags).
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketDef>,
    #[serde(default)]
    pub events: BTreeMap<String, EventDef>,
    pub pipeline: Vec<StageDef>,
    /// Stage names exported as EvalResult objectives.
    pub objectives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketDef {
    pub fold: FoldKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldKind {
    /// Σ of member values (the additive pool is a sum the pipeline wraps).
    Sum,
    /// 1 + Σv/100 — same-type multipliers SUM before multiplying.
    SummedGroup,
    /// Π(1 + v/100) — independent multipliers each their own factor.
    Product,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDef {
    /// Expression over stats; engine clamps the result to [0, 1].
    pub chance: String,
    /// Expression over stats/buckets (branch-recomputed); multiplied into
    /// `event_factors` when this event fires.
    pub factor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDef {
    pub name: String,
    pub expr: String,
    /// A branched stage is evaluated per event-branch and stores the
    /// probability-weighted EV. `event_factors` is only legal here.
    #[serde(default)]
    pub branched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamedef_parses_from_json() {
        let g: GameDef = serde_json::from_str(
            r#"{
              "stats": ["weapon", "crit_chance"],
              "conditions": ["enraged"],
              "buckets": { "additive": { "fold": "sum" },
                           "crit_group": { "fold": "summed_group" },
                           "indep": { "fold": "product" } },
              "events": { "crit": { "chance": "crit_chance / 100",
                                     "factor": "1.5 * crit_group" } },
              "pipeline": [
                { "name": "base", "expr": "weapon" },
                { "name": "hit", "expr": "base * event_factors", "branched": true }
              ],
              "objectives": ["hit"]
            }"#,
        )
        .unwrap();
        assert_eq!(g.stats, vec!["weapon", "crit_chance"]);
        assert_eq!(g.buckets["crit_group"].fold, FoldKind::SummedGroup);
        assert!(g.pipeline[1].branched && !g.pipeline[0].branched);
        assert_eq!(g.objectives, vec!["hit"]);
    }
}
