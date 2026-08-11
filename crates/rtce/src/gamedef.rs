//! GameDef — the game's ALGORITHM as configuration: stat registry, bucket
//! fold declarations, probabilistic events, and the pipeline of stages.
//! Compiled once by `plan::compile`; never touched on the hot path.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The game's ALGORITHM, as configuration: the stat/condition/bucket
/// registries, the probabilistic events, and the ordered pipeline of
/// derived stages. `plan::compile` turns one of these into a `Plan` once;
/// nothing here is touched again on the hot evaluation path.
///
/// # Unknown keys (P8a)
///
/// A non-`_` key that names no field here is rejected at PARSE with a
/// did-you-mean error; keys starting with `_` are the documented
/// annotation namespace (the committed gamedefs carry a top-level
/// `_source`) and are accepted — and dropped — exactly as the derived
/// `Deserialize` always dropped them. The same applies to
/// [`BucketDef`]/[`StageDef`]; an [`EventDef`] instead collects unknowns
/// into [`EventDef::extra`] so `plan::compile` can name the event in the
/// error. These structs deliberately gain NO new field: both consumers
/// construct them in Rust with exhaustive struct literals.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GameDef {
    /// Stat registry: names become slot offsets, in this order.
    pub stats: Vec<String>,
    /// Condition registry (uptime-gated contribution tags).
    #[serde(default)]
    pub conditions: Vec<String>,
    /// Bucket registry: name → fold rule, keyed by bucket name.
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketDef>,
    /// Probabilistic event registry: name → chance/factor expressions.
    #[serde(default)]
    pub events: BTreeMap<String, EventDef>,
    /// The ordered stages of the damage/output pipeline; later stages may
    /// reference earlier ones by name.
    pub pipeline: Vec<StageDef>,
    /// Stage names exported as EvalResult objectives.
    pub objectives: Vec<String>,
}

/// A named bucket's fold rule — how the contributions tagged with this
/// bucket combine into a single slot value. Unknown non-`_` keys are
/// rejected at parse (see [`GameDef`]'s "Unknown keys" section).
#[derive(Debug, Clone, Serialize)]
pub struct BucketDef {
    /// How this bucket's contributions combine.
    pub fold: FoldKind,
}

/// How a bucket's tagged contributions combine into one slot value.
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

/// A probabilistic event (crit, proc, …): a chance of firing and a factor
/// it contributes to the engine-provided `event_multiplier` when it does.
/// (`event_factors` remains a compatibility alias.) Every combination of
/// events is enumerated as a branch inside a `branched` stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDef {
    /// Expression over declared stats, conditions, and buckets; the engine
    /// clamps the result to [0, 1].
    pub chance: String,
    /// Expression over declared stats, conditions, and buckets (with buckets
    /// refolded for this branch); multiplied into `event_multiplier` when this
    /// event fires.
    pub factor: String,
    /// Unknown keys collected at parse (P8a). `_`-prefixed keys are the
    /// annotation namespace: accepted at every nesting level and carried
    /// through serde round-trips. Anything else fails closed at
    /// `plan::compile`, which names this event and suggests the nearest
    /// real field.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl EventDef {
    /// The declared field names, for `plan::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &["chance", "factor"];
}

/// One named stage of the pipeline: an expression evaluated over every
/// slot defined so far (stats, conditions, buckets, earlier stages).
/// Unknown non-`_` keys are rejected at parse (see [`GameDef`]'s
/// "Unknown keys" section).
#[derive(Debug, Clone, Serialize)]
pub struct StageDef {
    /// This stage's name; later stages and `objectives` refer to it by
    /// this name.
    pub name: String,
    /// The stage's expression, evaluated over the unified slot layout.
    pub expr: String,
    /// A branched stage is evaluated per event-branch and stores the
    /// probability-weighted EV. Engine-provided `event_multiplier` (and its
    /// compatibility alias `event_factors`) is only legal here; a config that
    /// already declares `event_multiplier` keeps that ordinary declared name.
    #[serde(default)]
    pub branched: bool,
}

// ── P8a: hand-written `Deserialize` for the consumer-constructed structs
// (see `config_keys`'s module docs for why these three cannot simply grow
// an `extra` field): a parse-side mirror with `#[serde(flatten)]` collects
// leftover keys, and `config_keys::reject_unknown` fails closed on any
// non-`_` one right there, with the context the struct itself carries.
//
// The exhaustive `Ok(Struct { field: r.field, … })` literals are
// LOAD-BEARING, here and in all seven mirrors (`build.rs`,
// `scenario.rs` follow this same pattern): adding a struct field breaks
// them at compile time, forcing the mirror and its `known` list to be
// updated together. `..Default::default()` would compile straight
// through a new field — demoting that compile-time drift guard to a
// confusing RUNTIME rejection of a legitimate field.

impl<'de> Deserialize<'de> for GameDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            stats: Vec<String>,
            #[serde(default)]
            conditions: Vec<String>,
            #[serde(default)]
            buckets: BTreeMap<String, BucketDef>,
            #[serde(default)]
            events: BTreeMap<String, EventDef>,
            pipeline: Vec<StageDef>,
            objectives: Vec<String>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            "the gamedef",
            &[
                "stats",
                "conditions",
                "buckets",
                "events",
                "pipeline",
                "objectives",
            ],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(GameDef {
            stats: r.stats,
            conditions: r.conditions,
            buckets: r.buckets,
            events: r.events,
            pipeline: r.pipeline,
            objectives: r.objectives,
        })
    }
}

impl<'de> Deserialize<'de> for BucketDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            fold: FoldKind,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown("a bucket definition", &["fold"], &r.extra)
            .map_err(serde::de::Error::custom)?;
        Ok(BucketDef { fold: r.fold })
    }
}

impl<'de> Deserialize<'de> for StageDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            name: String,
            expr: String,
            #[serde(default)]
            branched: bool,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            &format!("stage `{}`", r.name),
            &["name", "expr", "branched"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(StageDef {
            name: r.name,
            expr: r.expr,
            branched: r.branched,
        })
    }
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
