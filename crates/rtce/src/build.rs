//! BuildState — ONE candidate: raw stat values plus tagged contributions
//! into buckets. This is the only artifact that changes per permutation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// ONE candidate: a full set of stat values plus every tagged contribution
/// it makes into the game's buckets. This is the only piece of config that
/// changes per permutation a search driver evaluates.
///
/// # Unknown keys (P8a)
///
/// A non-`_` key that names no field here (or on a [`Contribution`]) is
/// rejected at PARSE with a did-you-mean error; `_`-prefixed keys are the
/// annotation namespace, accepted and dropped exactly as the derived
/// `Deserialize` always dropped them. Neither struct gains a field: both
/// consumers construct them in Rust with exhaustive struct literals.
///
/// Every stat VALUE must be finite — `NaN`/`inf` would otherwise
/// propagate through the folds and come back as an `Ok(NaN)` objective;
/// `Plan` rejects them at build resolution, as it does a non-finite
/// [`Contribution::value`].
#[derive(Debug, Clone, Default, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contribution {
    /// Name of the bucket this value folds into (must exist in the
    /// GameDef's bucket registry).
    pub bucket: String,
    /// The raw value contributed, before the bucket's fold is applied.
    /// Must be FINITE: `NaN`/`inf` is rejected where the contribution is
    /// resolved (`Plan`'s build resolution for a [`BuildState`]'s, and
    /// `sim::compile` for a `BuffDef`'s), never folded into an `Ok(NaN)`.
    pub value: f64,
    /// Counts only in branches where this event fired.
    #[serde(default)]
    pub event: Option<String>,
    /// Value scales by the phase's uptime for this condition (default 0).
    #[serde(default)]
    pub condition: Option<String>,
}

// P8a: hand-written `Deserialize` — parse-side mirror + shared
// unknown-key rejection (see `config_keys`'s module docs for why these
// two structs cannot simply grow an `extra` field).

impl<'de> Deserialize<'de> for BuildState {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            #[serde(default)]
            stats: BTreeMap<String, f64>,
            #[serde(default)]
            contributions: Vec<Contribution>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            "the build state",
            &["stats", "contributions"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(BuildState {
            stats: r.stats,
            contributions: r.contributions,
        })
    }
}

impl<'de> Deserialize<'de> for Contribution {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            bucket: String,
            value: f64,
            #[serde(default)]
            event: Option<String>,
            #[serde(default)]
            condition: Option<String>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            &format!("a contribution into bucket `{}`", r.bucket),
            &["bucket", "value", "event", "condition"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(Contribution {
            bucket: r.bucket,
            value: r.value,
            event: r.event,
            condition: r.condition,
        })
    }
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

    #[test]
    fn player_facing_gear_annotations_do_not_change_the_build() {
        let b: BuildState = serde_json::from_str(
            r#"{
                "_guide": "notes beginning with an underscore are for humans",
                "stats": { "weapon": 100.0 },
                "contributions": [
                    {
                        "_source": "Stormstring Bow · Serrated Edge affix",
                        "bucket": "additive",
                        "value": 30.0
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(b.stats["weapon"], 100.0);
        assert_eq!(b.contributions.len(), 1);
        assert_eq!(b.contributions[0].bucket, "additive");
        assert_eq!(b.contributions[0].value, 30.0);
    }
}
