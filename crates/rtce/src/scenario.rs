//! Scenario (playbook) — THE FIGHT being asked about, as configuration:
//! weighted phases with stat overrides and condition-uptime fractions.
//! Level-1 semantics (weighted-phase blending); the Level-2 timeline
//! simulator ([`crate::sim::run`], shipped in 0.2.0) SHARES this schema —
//! it reads the same phases and stat overrides, and COMPUTES a condition's
//! uptime only where a live buff drives it. The precedence is the one
//! `crate::sim` states: an active buff's `conditions` entry WINS while that
//! buff lasts, and otherwise the phase's ASSERTED uptime below is what the
//! sim uses — so for a condition no buff drives, `SimReport`'s reported
//! uptime is exactly the number written here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// THE FIGHT being asked about, as configuration: a set of weighted
/// phases whose objective values are blended together (weight / total).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scenario {
    /// The phases making up this scenario; blended by relative weight.
    pub phases: Vec<Phase>,
}

/// One weighted slice of a `Scenario` (e.g. "boss burst window", "add
/// phase"), with its own condition uptimes and stat overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phase {
    /// This phase's name (shown in `explain()` traces).
    pub name: String,
    /// Relative weight (e.g. seconds); normalized over the scenario's sum.
    pub weight: f64,
    /// Condition → uptime fraction in `[0,1]`. Missing condition = 0.0.
    /// Fractional uptimes blend condition effects INDEPENDENTLY (each
    /// effect scales by u; correlations between effects of the same
    /// condition are dropped) — exact at 0 and 1, the Level-1
    /// approximation between; Level-2 timeline simulation is the fidelity
    /// path.
    #[serde(default)]
    pub uptimes: BTreeMap<String, f64>,
    /// Stat overrides for this phase (enemy DR, target count, …).
    #[serde(default)]
    pub stats: BTreeMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_parses() {
        let s: Scenario = serde_json::from_str(
            r#"{ "phases": [
                  { "name": "boss", "weight": 60,
                    "uptimes": { "enraged": 0.5 },
                    "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();
        assert_eq!(s.phases[0].uptimes["enraged"], 0.5);
        assert_eq!(s.phases[0].stats["enemy_dr"], 20.0);
    }
}
