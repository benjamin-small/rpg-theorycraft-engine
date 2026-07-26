//! BuildState — ONE candidate: raw stat values plus tagged contributions
//! into buckets. This is the only artifact that changes per permutation.

use serde::{Deserialize, Serialize};

/// ONE candidate: a full set of stat values plus every tagged contribution
/// it makes into the game's buckets. This is the only piece of config that
/// changes per permutation a search driver evaluates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildState {
    /// Values for the GameDef stat registry, by name (missing = 0.0).
    #[serde(default)]
    pub stats: std::collections::BTreeMap<String, f64>,
    /// Every value this build contributes into a bucket, each optionally
    /// gated by an event or a condition (missing = 0.0).
    #[serde(default)]
    pub contributions: Vec<Contribution>,
}

/// One value flowing into one bucket, with optional gating tags. Untagged
/// (`event: None, condition: None`) contributions always count; an
/// `event`-tagged one counts only in branches where that event fired; a
/// `condition`-tagged one scales by the active phase's uptime for that
/// condition.
/// `PartialEq` compares the raw `f64` `value` bitwise-per-IEEE (so two
/// `NaN` values are never equal) — these are config literals, never
/// computed results, so structural comparison is exactly what a
/// "did this parse to what I wrote?" test wants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    /// Name of the bucket this value folds into (must exist in the
    /// GameDef's bucket registry).
    pub bucket: String,
    /// The raw value contributed, before the bucket's fold is applied.
    pub value: f64,
    /// Counts only in branches where this event fired.
    #[serde(default)]
    pub event: Option<String>,
    /// Value scales by the phase's uptime for this condition (default 0).
    #[serde(default)]
    pub condition: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buildstate_parses_with_tags() {
        let b: BuildState = serde_json::from_str(
            r#"{ "stats": { "weapon": 100.0 },
                 "contributions": [
                   { "bucket": "additive", "value": 40.0 },
                   { "bucket": "additive", "value": 30.0, "event": "crit" },
                   { "bucket": "additive", "value": 20.0, "condition": "enraged" } ] }"#,
        )
        .unwrap();
        assert_eq!(b.stats["weapon"], 100.0);
        assert_eq!(b.contributions[1].event.as_deref(), Some("crit"));
        assert_eq!(b.contributions[2].condition.as_deref(), Some("enraged"));
    }
}
