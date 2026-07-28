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
///
/// # Unknown keys (P8a)
///
/// A non-`_` key that names no field here (or on a [`Phase`]) is rejected
/// at PARSE with a did-you-mean error; `_`-prefixed keys are the
/// annotation namespace, accepted and dropped exactly as the derived
/// `Deserialize` always dropped them. Neither struct gains a field: both
/// consumers construct them in Rust with exhaustive struct literals.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Scenario {
    /// The phases making up this scenario; blended by relative weight.
    pub phases: Vec<Phase>,
}

/// One weighted slice of a `Scenario` (e.g. "boss burst window", "add
/// phase"), with its own condition uptimes and stat overrides.
#[derive(Debug, Clone, Serialize)]
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
    /// Stat overrides for this phase (enemy DR, target count, …). Every
    /// value must be finite — `Plan` rejects `NaN`/`inf` at its build
    /// resolution, mirroring the uptime rule above.
    #[serde(default)]
    pub stats: BTreeMap<String, f64>,
}

// P8a: hand-written `Deserialize` — parse-side mirror + shared
// unknown-key rejection (see `config_keys`'s module docs for why these
// two structs cannot simply grow an `extra` field). A `Phase` carries its
// own name, so the parse error can say which phase the typo sits on.

impl<'de> Deserialize<'de> for Scenario {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            phases: Vec<Phase>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown("the scenario", &["phases"], &r.extra)
            .map_err(serde::de::Error::custom)?;
        Ok(Scenario { phases: r.phases })
    }
}

impl<'de> Deserialize<'de> for Phase {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            name: String,
            weight: f64,
            #[serde(default)]
            uptimes: BTreeMap<String, f64>,
            #[serde(default)]
            stats: BTreeMap<String, f64>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            &format!("phase `{}`", r.name),
            &["name", "weight", "uptimes", "stats"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Phase {
            name: r.name,
            weight: r.weight,
            uptimes: r.uptimes,
            stats: r.stats,
        })
    }
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

    // P8a: a typo'd key on a phase used to be silently ignored — here
    // `"uptime"` would silently mean "no uptimes at all", the quiet wrong
    // answer this crate refuses. Rejected at parse (the phase's own name
    // is inside the struct, so the error can carry its context), with the
    // nearest real field suggested.
    #[test]
    fn a_typoed_key_on_a_phase_is_rejected_with_a_did_you_mean() {
        let e = serde_json::from_str::<Scenario>(
            r#"{ "phases": [
                  { "name": "boss", "weight": 60,
                    "uptime": { "enraged": 0.5 } } ] }"#,
        )
        .unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("unknown field `uptime`"), "got: {msg}");
        assert!(msg.contains("phase `boss`"), "got: {msg}");
        assert!(msg.contains("did you mean `uptimes`"), "got: {msg}");
    }

    // The `_` namespace stays open at this level too: annotations on the
    // scenario, a phase — parsed and ignored, never an error.
    #[test]
    fn underscore_annotations_on_scenario_and_phase_parse() {
        let s: Scenario = serde_json::from_str(
            r#"{ "_playbook": "boss opener, hand-timed 2026-07",
                 "phases": [
                   { "name": "boss", "weight": 60, "_span": "0-60s",
                     "uptimes": { "enraged": 0.5 } } ] }"#,
        )
        .unwrap();
        assert_eq!(s.phases[0].uptimes["enraged"], 0.5);
    }
}
