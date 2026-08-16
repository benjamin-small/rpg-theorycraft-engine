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
/// error. `GameDef` and `BucketDef` retain exhaustive public fields for Rust
/// consumers; `StageDef` is an untagged enum so each stage shape remains
/// explicit.
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

/// One named stage of the pipeline. The untagged representation preserves the
/// original expression-stage JSON (`{ "name", "expr", "branched"? }`) and
/// adds dedicated solver (`{ "name", "solve": { ... } }`) and recurrence
/// (`{ "name", "recurrence": { ... } }`) shapes.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum StageDef {
    /// An ordinary expression stage.
    Expression(ExpressionStageDef),
    /// A deterministic bounded scalar-solve stage.
    Solve(SolveStageDef),
    /// A deterministic bounded state-recurrence stage.
    Recurrence(RecurrenceStageDef),
}

impl StageDef {
    /// This stage's declared name.
    pub fn name(&self) -> &str {
        match self {
            Self::Expression(stage) => &stage.name,
            Self::Solve(stage) => &stage.name,
            Self::Recurrence(stage) => &stage.name,
        }
    }

    /// Borrow this stage as an expression stage, or `None` otherwise.
    pub fn as_expression(&self) -> Option<&ExpressionStageDef> {
        match self {
            Self::Expression(stage) => Some(stage),
            Self::Solve(_) | Self::Recurrence(_) => None,
        }
    }

    /// Mutably borrow this stage as an expression stage, or `None` otherwise.
    pub fn as_expression_mut(&mut self) -> Option<&mut ExpressionStageDef> {
        match self {
            Self::Expression(stage) => Some(stage),
            Self::Solve(_) | Self::Recurrence(_) => None,
        }
    }

    /// Borrow this stage as a solve stage, or `None` otherwise.
    pub fn as_solve(&self) -> Option<&SolveStageDef> {
        match self {
            Self::Expression(_) | Self::Recurrence(_) => None,
            Self::Solve(stage) => Some(stage),
        }
    }

    /// Mutably borrow this stage as a solve stage, or `None` otherwise.
    pub fn as_solve_mut(&mut self) -> Option<&mut SolveStageDef> {
        match self {
            Self::Expression(_) | Self::Recurrence(_) => None,
            Self::Solve(stage) => Some(stage),
        }
    }

    /// Borrow this stage as a recurrence stage, or `None` otherwise.
    pub fn as_recurrence(&self) -> Option<&RecurrenceStageDef> {
        match self {
            Self::Recurrence(stage) => Some(stage),
            Self::Expression(_) | Self::Solve(_) => None,
        }
    }

    /// Mutably borrow this stage as a recurrence stage, or `None` otherwise.
    pub fn as_recurrence_mut(&mut self) -> Option<&mut RecurrenceStageDef> {
        match self {
            Self::Recurrence(stage) => Some(stage),
            Self::Expression(_) | Self::Solve(_) => None,
        }
    }
}

/// An expression evaluated over stats, conditions, buckets, and earlier
/// pipeline stages.
#[derive(Debug, Clone, Serialize)]
pub struct ExpressionStageDef {
    /// This stage's name; later stages and `objectives` refer to it by name.
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

/// A named stage whose value is produced by a bounded scalar solve.
#[derive(Debug, Clone, Serialize)]
pub struct SolveStageDef {
    /// This stage's name; later stages and `objectives` refer to it by name.
    pub name: String,
    /// The deterministic bisection configuration.
    pub solve: SolveDef,
}

/// Configuration for deterministic conservative bisection.
#[derive(Debug, Clone, Serialize)]
pub struct SolveDef {
    /// Solver-local identifier available only inside `residual`.
    pub variable: String,
    /// Monotone residual expression. Values `<= 0` are feasible; values
    /// `> 0` exceed the modeled pool or constraint.
    pub residual: String,
    /// Inclusive lower-bound expression over normal stage symbols.
    pub lower: String,
    /// Inclusive upper-bound expression over normal stage symbols.
    pub upper: String,
    /// Absolute bracket-width tolerance; finite and non-negative.
    pub absolute_tolerance: f64,
    /// Relative bracket-width tolerance; finite and non-negative.
    pub relative_tolerance: f64,
    /// Hard per-evaluation iteration budget.
    pub max_iterations: u32,
}

/// A named stage whose value is produced by a bounded state recurrence.
#[derive(Debug, Clone, Serialize)]
pub struct RecurrenceStageDef {
    /// This stage's name; later stages and `objectives` refer to it by name.
    pub name: String,
    /// The deterministic recurrence configuration.
    pub recurrence: RecurrenceDef,
}

/// Configuration for a small, bounded state machine.
#[derive(Debug, Clone, Serialize)]
pub struct RecurrenceDef {
    /// Named local state slots. Initializers read ordinary stage symbols;
    /// every `next` expression reads the complete previous state.
    pub state: Vec<RecurrenceStateDef>,
    /// Numeric terminal predicate. Zero continues; non-zero terminates.
    pub until: String,
    /// Value returned by the stage once `until` is non-zero.
    pub result: String,
    /// Hard transition budget for one evaluation.
    pub max_iterations: u32,
}

/// One local state slot in a recurrence stage.
#[derive(Debug, Clone, Serialize)]
pub struct RecurrenceStateDef {
    /// Recurrence-local plain identifier.
    pub name: String,
    /// Initial value, evaluated once before iteration zero.
    pub initial: String,
    /// Next value, evaluated from the previous iteration's complete state.
    pub next: String,
}

// ── P8a: hand-written `Deserialize` for the public config structs and stage
// variants. A parse-side mirror with `#[serde(flatten)]` collects
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
            #[serde(default)]
            expr: Option<String>,
            #[serde(default)]
            branched: Option<bool>,
            #[serde(default)]
            solve: Option<SolveDef>,
            #[serde(default)]
            recurrence: Option<RecurrenceDef>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            &format!("stage `{}`", r.name),
            &["name", "expr", "branched", "solve", "recurrence"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        match (r.expr, r.solve, r.recurrence, r.branched) {
            (Some(expr), None, None, branched) => Ok(StageDef::Expression(ExpressionStageDef {
                name: r.name,
                expr,
                branched: branched.unwrap_or(false),
            })),
            (None, Some(solve), None, None) => Ok(StageDef::Solve(SolveStageDef {
                name: r.name,
                solve,
            })),
            (None, None, Some(recurrence), None) => Ok(StageDef::Recurrence(RecurrenceStageDef {
                name: r.name,
                recurrence,
            })),
            (None, Some(_), None, Some(_)) => Err(serde::de::Error::custom(format!(
                "solve stage `{}` cannot declare `branched`",
                r.name
            ))),
            (None, None, Some(_), Some(_)) => Err(serde::de::Error::custom(format!(
                "recurrence stage `{}` cannot declare `branched`",
                r.name
            ))),
            _ => Err(serde::de::Error::custom(format!(
                "stage `{}` must declare exactly one of `expr`, `solve`, or `recurrence`",
                r.name
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for SolveDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            variable: String,
            residual: String,
            lower: String,
            upper: String,
            absolute_tolerance: f64,
            relative_tolerance: f64,
            max_iterations: u32,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            "a solve definition",
            &[
                "variable",
                "residual",
                "lower",
                "upper",
                "absolute_tolerance",
                "relative_tolerance",
                "max_iterations",
            ],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(SolveDef {
            variable: r.variable,
            residual: r.residual,
            lower: r.lower,
            upper: r.upper,
            absolute_tolerance: r.absolute_tolerance,
            relative_tolerance: r.relative_tolerance,
            max_iterations: r.max_iterations,
        })
    }
}

impl<'de> Deserialize<'de> for RecurrenceDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            #[serde(default)]
            state: Option<Vec<RecurrenceStateDef>>,
            #[serde(default)]
            until: Option<String>,
            #[serde(default)]
            result: Option<String>,
            #[serde(default)]
            max_iterations: Option<u32>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            "a recurrence definition",
            &["state", "until", "result", "max_iterations"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(RecurrenceDef {
            state: r
                .state
                .ok_or_else(|| serde::de::Error::missing_field("state"))?,
            until: r
                .until
                .ok_or_else(|| serde::de::Error::missing_field("until"))?,
            result: r
                .result
                .ok_or_else(|| serde::de::Error::missing_field("result"))?,
            max_iterations: r
                .max_iterations
                .ok_or_else(|| serde::de::Error::missing_field("max_iterations"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RecurrenceStateDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            initial: Option<String>,
            #[serde(default)]
            next: Option<String>,
            #[serde(flatten)]
            extra: BTreeMap<String, serde_json::Value>,
        }
        let r = Repr::deserialize(d)?;
        crate::config_keys::reject_unknown(
            &format!(
                "recurrence state `{}`",
                r.name.as_deref().unwrap_or("<unnamed>")
            ),
            &["name", "initial", "next"],
            &r.extra,
        )
        .map_err(serde::de::Error::custom)?;
        Ok(RecurrenceStateDef {
            name: r
                .name
                .ok_or_else(|| serde::de::Error::missing_field("name"))?,
            initial: r
                .initial
                .ok_or_else(|| serde::de::Error::missing_field("initial"))?,
            next: r
                .next
                .ok_or_else(|| serde::de::Error::missing_field("next"))?,
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
        assert!(g.pipeline[1].as_expression().unwrap().branched);
        assert!(!g.pipeline[0].as_expression().unwrap().branched);
        assert_eq!(g.objectives, vec!["hit"]);
    }

    #[test]
    fn solve_stage_parses_and_round_trips_without_tagging_expression_stages() {
        let expression_json = serde_json::json!({
            "name": "base",
            "expr": "weapon",
            "branched": false
        });
        let expression: StageDef = serde_json::from_value(expression_json.clone()).unwrap();
        assert!(matches!(expression, StageDef::Expression(_)));
        assert_eq!(serde_json::to_value(&expression).unwrap(), expression_json);

        let solve_json = serde_json::json!({
            "name": "root",
            "solve": {
                "variable": "x",
                "residual": "x * x - 2",
                "lower": "0",
                "upper": "2",
                "absolute_tolerance": 1e-7,
                "relative_tolerance": 1e-9,
                "max_iterations": 128
            }
        });
        let solve: StageDef = serde_json::from_value(solve_json.clone()).unwrap();
        assert!(matches!(solve, StageDef::Solve(_)));
        assert_eq!(serde_json::to_value(&solve).unwrap(), solve_json);
    }

    #[test]
    fn specialized_stage_shapes_and_nested_keys_fail_closed() {
        let both = serde_json::from_value::<StageDef>(serde_json::json!({
            "name": "ambiguous",
            "expr": "1",
            "solve": {
                "variable": "x", "residual": "x", "lower": "0", "upper": "1",
                "absolute_tolerance": 1e-7, "relative_tolerance": 1e-9,
                "max_iterations": 128
            }
        }))
        .unwrap_err();
        assert!(
            both.to_string()
                .contains("exactly one of `expr`, `solve`, or `recurrence`"),
            "got: {both}"
        );

        let typo = serde_json::from_value::<StageDef>(serde_json::json!({
            "name": "root",
            "solve": {
                "variable": "x", "residual": "x", "lower": "0", "upper": "1",
                "absolute_tolerence": 1e-7, "absolute_tolerance": 1e-7,
                "relative_tolerance": 1e-9, "max_iterations": 128
            }
        }))
        .unwrap_err();
        assert!(
            typo.to_string()
                .contains("unknown field `absolute_tolerence`")
                && typo
                    .to_string()
                    .contains("did you mean `absolute_tolerance`"),
            "got: {typo}"
        );
    }
}
