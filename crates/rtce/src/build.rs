//! BuildState — ONE candidate: raw stat values plus tagged contributions
//! into buckets. This is the only artifact that changes per permutation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildState {
    /// Values for the GameDef stat registry, by name (missing = 0.0).
    #[serde(default)]
    pub stats: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub contributions: Vec<Contribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contribution {
    pub bucket: String,
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
