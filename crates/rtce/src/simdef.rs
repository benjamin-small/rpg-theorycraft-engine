//! SimDef — the game's SEQUENCING config: resources, actions, buffs, and
//! procs, plus the per-hit objective the timeline simulator accumulates.
//! Sits BESIDE the `GameDef`/`Plan` — nothing in `plan.rs` changes.
//! `sim::compile` turns a `SimDef` (with a `Rotation`) into a `SimPlan`;
//! see that module for the compiled form and the extended symbol space.

use crate::build::Contribution;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The game's SEQUENCING config, as data: resource/action/buff/proc
/// registries plus the `Plan` objective the sim accumulates per hit.
/// Compiled once (together with a [`Rotation`]) by `sim::compile`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimDef {
    /// Resource registry (mana, spirit, …), by name.
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceDef>,
    /// Action registry (casts the rotation can choose), by name.
    #[serde(default)]
    pub actions: BTreeMap<String, ActionDef>,
    /// Buff/debuff registry (timed contribution/condition windows), by
    /// name.
    #[serde(default)]
    pub buffs: BTreeMap<String, BuffDef>,
    /// Proc registry (chance-triggered effects), by name.
    #[serde(default)]
    pub procs: BTreeMap<String, ProcDef>,
    /// Name of the `Plan` objective the sim accumulates as
    /// `damage_objective × hits` on every completed damaging cast.
    pub damage_objective: String,
}

/// A literal number or an expression string, evaluated at a documented
/// instant.
///
/// Untagged: a JSON number deserializes to [`NumOrExpr::Num`] and a JSON
/// string to [`NumOrExpr::Expr`]. Every rtce 0.2.0 config — which only
/// ever wrote plain numbers in these positions — therefore PARSES
/// unchanged, and a literal reaches the executor as the identical `f64`
/// the old field held: `Num` is pre-baked into a constant at
/// `sim::compile`, so the literal path adds no evaluation and no rounding.
/// (That is a statement about THIS type only. The 0.3.0 release notes
/// carry two deliberate executor behavior FIXES landed alongside it, which
/// can move results for configs whose proc `chance` reads state another
/// proc mutates in the same trigger batch, or which have an `on_hit` proc
/// that changes crit chance — see the CHANGELOG.)
///
/// An `Expr` is parsed at `sim::compile` against the sim symbol space (see
/// the `sim` module docs), with the usual positioned, fail-closed error
/// for an unknown identifier or a syntax problem. Pipeline stages and
/// buckets are NOT in that space — naming one is a compile error, the same
/// as any other unresolved name.
///
/// # Evaluation instants
///
/// An expression is re-evaluated every time its field is USED, at the
/// instant named below — never once up front:
///
/// | Field | Evaluated |
/// |---|---|
/// | [`BuffDef::duration`] | at application (snapshotted onto that window) |
/// | [`ActionDef::cooldown`] | at cast start |
/// | [`ActionDef::cost`] values | at cast start — and at every decision that merely CHECKS affordability |
/// | [`ActionDef::gain`] values | at cast complete |
/// | [`ActionDamage::stats`] values | at cast complete |
///
/// A cost expression is therefore RE-CHECKED at each decision point, never
/// PREDICTED: the executor's resource-affordability wake time is solved
/// from the cost as evaluated at that decision instant, and if the
/// expression's value has changed by the time the wake fires, the wake
/// simply re-decides at the new value (see `sim::exec`'s `afford`).
///
/// Keep cost expressions CHEAP for that reason. A cost is the only one of
/// these fields evaluated on a hot path — once per rule per decision point
/// — where the others are evaluated once per cast or per buff
/// application. A literal costs nothing at all (it is a pre-baked
/// constant), so leave a fixed cost as a number.
///
/// # Fail-closed
///
/// At its evaluation instant a value that is not finite is a run error
/// naming the field and the instant; `duration`/`cooldown`/`cost`/`gain`
/// additionally reject a negative result. [`ActionDamage::stats`] values
/// may legitimately be negative (a stat is not a quantity of anything), so
/// only finiteness is enforced there.
///
/// Deliberately NOT `#[non_exhaustive]`, unlike the compiled
/// [`crate::sim::CompiledValue`]: this is a CONFIG type, "number or
/// expression" is the whole idea, and a caller building or inspecting a
/// `SimDef` in Rust should be able to match it exhaustively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumOrExpr {
    /// Literal value (backward compatible with 0.2.0 configs).
    Num(f64),
    /// Expression over the sim symbol space.
    Expr(String),
}

/// `0.0` — the same default the `f64` fields carried before these became
/// expression-valued, so `#[serde(default)]` on `cooldown`/`cost`/`gain`
/// keeps its 0.2.0 meaning.
impl Default for NumOrExpr {
    fn default() -> Self {
        NumOrExpr::Num(0.0)
    }
}

/// A number is always a literal: `NumOrExpr::from(40.0)` is
/// [`NumOrExpr::Num`], the pre-baked-constant path.
impl From<f64> for NumOrExpr {
    fn from(v: f64) -> Self {
        NumOrExpr::Num(v)
    }
}

/// A string is ALWAYS an expression, even when it looks like a number:
/// `NumOrExpr::from("40")` is [`NumOrExpr::Expr`] — a compiled program
/// that evaluates to 40, not the literal [`NumOrExpr::Num`]`(40.0)`. The
/// two agree on every value they produce (the grammar's numeric literals
/// are `f64`), so this is a representation difference, not a semantic
/// one — but it is the one place this type can be misread, and it does
/// cost a `Program` and an evaluation. Write `40.0.into()` for a
/// constant.
impl From<&str> for NumOrExpr {
    fn from(s: &str) -> Self {
        NumOrExpr::Expr(s.to_string())
    }
}

/// As [`From<&str>`][`NumOrExpr::from`]: a string is always an expression.
impl From<String> for NumOrExpr {
    fn from(s: String) -> Self {
        NumOrExpr::Expr(s)
    }
}

/// One resource (mana, spirit, fury, …): a capped pool that regenerates
/// continuously and is spent/gained by actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDef {
    /// Expression (over stats/conditions) for this resource's cap.
    pub max: String,
    /// Expression (over stats/conditions) for the per-second regen rate.
    pub regen_per_sec: String,
}

/// One action the rotation can cast: timing, resource cost/gain, and an
/// optional damage effect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionDef {
    /// Expression (over stats/conditions/sim-state; never pipeline
    /// stages/buckets) for the cast time in seconds. `"0"` = instant.
    pub cast_time: String,
    /// Cooldown in seconds, starting when the cast begins (`0.0` = none).
    /// Literal or expression; an expression is evaluated AT CAST START
    /// (before the cost is deducted) and must be finite and `>= 0` —
    /// see [`NumOrExpr`].
    #[serde(default)]
    pub cooldown: NumOrExpr,
    /// Resource cost paid when the cast begins, by resource name. Literal
    /// or expression; an expression is evaluated AT CAST START, and also
    /// at every decision point that checks whether this action is
    /// affordable — see [`NumOrExpr`] for why a cost expression is
    /// re-checked rather than predicted.
    #[serde(default)]
    pub cost: BTreeMap<String, NumOrExpr>,
    /// Resource gained when the cast completes, by resource name (e.g.
    /// basic/generator skills). Literal or expression; an expression is
    /// evaluated AT CAST COMPLETE and must be finite and `>= 0` — see
    /// [`NumOrExpr`].
    #[serde(default)]
    pub gain: BTreeMap<String, NumOrExpr>,
    /// This action's damage effect, if any (omit for utility-only casts).
    #[serde(default)]
    pub damage: Option<ActionDamage>,
}

/// Per-cast stat overrides folded onto the `Plan`'s `BuildState` before the
/// `damage_objective` is evaluated for this cast. `hits_per_use` (default
/// `1.0` when absent) is read directly by the executor as the per-cast hit
/// count rather than fed into the `Plan` as a stat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionDamage {
    /// Stat name → override value, applied only while resolving this
    /// action's damage. Literal or expression; an expression is evaluated
    /// AT CAST COMPLETE, ONCE per cast (the same evaluated overlay feeds
    /// the damage query and the `on_crit` proc weight), and need only be
    /// FINITE — a stat may legitimately be negative. `hits_per_use` lives
    /// in this map and follows the same rule. See [`NumOrExpr`].
    ///
    /// The completion instant has internal ORDER, and these expressions
    /// are evaluated at a fixed point within it: AFTER this action's
    /// [`ActionDef::gain`] is credited and after its own cast is counted,
    /// and BEFORE any of this cast's proc rolls. So an expression here
    /// reads a resource at its POST-gain amount, and `casts.<this action>`
    /// INCLUDES the cast being resolved (`1` on the first cast, never
    /// `0`). A buff applied by this cast's own procs is NOT visible —
    /// which is the point: a proc triggered by a hit cannot change what
    /// that hit was.
    #[serde(default)]
    pub stats: BTreeMap<String, NumOrExpr>,
}

/// One buff/debuff: a timed window that, while active, contributes to
/// buckets, drives condition values, and/or accrues a DoT objective.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuffDef {
    /// Seconds this buff lasts once applied (refresh-on-reapply resets
    /// the remaining duration back to this value).
    ///
    /// Literal or expression. An expression is evaluated AT EACH
    /// APPLICATION and SNAPSHOTTED onto the window it starts (or
    /// refreshes): a stat/phase change afterwards never retroactively
    /// lengthens or shortens a window already in flight — the NEXT
    /// application re-evaluates and gets the new value. Must be finite and
    /// `>= 0` — see [`NumOrExpr`].
    ///
    /// It reads the LIVE state at that instant, which differs between the
    /// two application paths — deliberately, and this buff's own effects
    /// are never un-folded to hide the difference:
    ///
    /// - **First application** (this buff not currently active):
    ///   `buff.<self>` is `0`, `buff_remaining.<self>` is `0`, and a
    ///   condition this buff drives reads its non-buff value.
    /// - **Refresh** (this buff already active): the outgoing window is
    ///   still in force, so `buff.<self>` is `1`, a condition this buff
    ///   drives reads its BUFF-DRIVEN value, and `buff_remaining.<self>`
    ///   is the time left on the window being REPLACED.
    ///
    /// The refresh reading is what makes pandemic-style refreshes
    /// expressible as data — `"min(12, buff_remaining.x + 8)"` extends by
    /// 8s up to a 12s cap. Bucket CONTRIBUTIONS are invisible on both
    /// paths: buckets are not in the sim symbol space at all.
    ///
    /// NB: the reserved sim symbol `duration` is the SCENARIO's total
    /// length in seconds, not this field. An expression here that names
    /// `duration` reads the fight length.
    pub duration: NumOrExpr,
    /// Bucket contributions active while this buff is up, folded into the
    /// effective build alongside the base `BuildState`'s own.
    #[serde(default)]
    pub contributions: Vec<Contribution>,
    /// Condition name → value while this buff is active. Per the spec's
    /// precedence rule, an active buff driving a condition WINS over the
    /// scenario's static uptime for that condition; the static uptime
    /// applies again once the buff expires.
    #[serde(default)]
    pub conditions: BTreeMap<String, f64>,
    /// A `Plan` objective name: while this buff is active, its value ×
    /// active-seconds accrues into the sim's damage total (DoT ticking).
    #[serde(default)]
    pub tick_objective: Option<String>,
}

/// What event a [`ProcDef`] rolls its chance against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Rolls once per cast begun.
    OnCast,
    /// Rolls once per damaging hit.
    OnHit,
    /// Rolls once per critical hit.
    OnCrit,
}

/// One proc: a chance-triggered effect (apply a buff, or cast a free
/// action) rolled on a trigger event, subject to an internal cooldown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcDef {
    /// Which event this proc rolls its chance against.
    pub trigger: Trigger,
    /// Expression (over stats/conditions/sim-state) for the fire chance
    /// per qualifying roll.
    pub chance: String,
    /// Internal cooldown in seconds after firing (`0.0` = none).
    #[serde(default)]
    pub icd: f64,
    /// Buff to apply when this proc fires. Exactly one of `apply_buff` /
    /// `cast_action` must be set — zero or both is a compile error.
    #[serde(default)]
    pub apply_buff: Option<String>,
    /// Action to cast for free (does not consume the rotation's decision
    /// slot) when this proc fires. Exactly one of `apply_buff` /
    /// `cast_action` must be set — zero or both is a compile error.
    #[serde(default)]
    pub cast_action: Option<String>,
}

/// A priority-list rotation (SimC-style): pure config, drivers may search
/// over rotations the same way they search over gear. The first eligible
/// [`Rule`] wins; hard gates (off cooldown, cost payable, not mid-cast)
/// are enforced by the engine automatically, `when` adds strategy on top.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rotation {
    /// Priority-ordered rules; the first eligible one wins each decision
    /// point. No rule eligible → time advances to the next event (waiting
    /// is modeled, never an infinite loop).
    pub rules: Vec<Rule>,
}

/// One rotation rule: cast `action` when its hard gates pass and (if
/// present) `when` evaluates truthy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// The action this rule casts when eligible.
    pub action: String,
    /// Optional extra eligibility predicate over the sim symbol space
    /// (hard gates are automatic and need not be repeated here). Absent =
    /// always willing (subject to hard gates).
    #[serde(default)]
    pub when: Option<String>,
}

/// Verbatim from docs/superpowers/specs/2026-07-22-p6-sequencing-design.md
/// "Config surface" section (jsonc comments stripped — serde_json has no
/// comment support; every key/value is copied unchanged, including the
/// `cast_time` formula `"1.0 / base_aps"`). Every position that P7b made
/// expression-valued is a plain JSON NUMBER here, which is exactly what
/// makes this the 0.2.0 compatibility fixture — shared by this module's
/// parse test and `sim::exec`'s behavioral one, so there is ONE copy and
/// no unchecked claim of byte-identity between two.
#[cfg(test)]
pub(crate) const P6_SPEC_SIMDEF_JSON: &str = r#"{
      "resources": {
        "mana": { "max": "max_mana", "regen_per_sec": "mana_regen" }
      },
      "actions": {
        "fireball": {
          "cast_time": "1.0 / base_aps",
          "cooldown": 0.0,
          "cost": { "mana": 40.0 },
          "gain": {},
          "damage": {
            "stats": { "coeff_pct": 200.0, "hits_per_use": 1.0 }
          }
        }
      },
      "buffs": {
        "vuln_window": { "duration": 4.0, "conditions": { "vulnerable": 1.0 } },
        "combustion":  { "duration": 8.0,
                         "contributions": [{ "bucket": "indep", "value": 25.0 }] },
        "burning":     { "duration": 6.0, "tick_objective": "dot_dps" }
      },
      "procs": {
        "conflagrate": { "trigger": "on_crit",
                         "chance": "lucky_hit_chance / 100 * 0.3",
                         "icd": 2.0,
                         "apply_buff": "combustion" }
      },
      "damage_objective": "hit_after_dr"
    }"#;

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim from the spec's "Rotation (candidate-domain …)" section.
    const ROTATION_JSON: &str = r#"{ "rules": [
      { "action": "frost_nova", "when": "cooldown.frost_nova == 0 and buff.vuln_window == 0" },
      { "action": "fireball",   "when": "mana >= 40" },
      { "action": "basic_bolt" }
    ]}"#;

    #[test]
    fn simdef_round_trips_the_spec_example_verbatim() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();

        assert_eq!(def.resources["mana"].max, "max_mana");
        assert_eq!(def.resources["mana"].regen_per_sec, "mana_regen");

        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cast_time, "1.0 / base_aps");
        // P7b: these four positions became `NumOrExpr`. Untagged serde
        // must keep reading the spec's plain JSON NUMBERS as `Num` — this
        // is the 0.2.0 backward-compatibility contract at the parse layer
        // (the behavioral half is pinned in `sim::exec`'s tests).
        assert_eq!(fireball.cooldown, NumOrExpr::Num(0.0));
        assert_eq!(fireball.cost["mana"], NumOrExpr::Num(40.0));
        assert!(fireball.gain.is_empty());
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], NumOrExpr::Num(200.0));
        assert_eq!(dmg.stats["hits_per_use"], NumOrExpr::Num(1.0));

        assert_eq!(def.buffs["vuln_window"].duration, NumOrExpr::Num(4.0));
        assert_eq!(def.buffs["vuln_window"].conditions["vulnerable"], 1.0);
        assert_eq!(def.buffs["combustion"].contributions.len(), 1);
        assert_eq!(def.buffs["combustion"].contributions[0].bucket, "indep");
        assert_eq!(def.buffs["combustion"].contributions[0].value, 25.0);
        assert_eq!(
            def.buffs["burning"].tick_objective.as_deref(),
            Some("dot_dps")
        );

        let proc = &def.procs["conflagrate"];
        assert_eq!(proc.trigger, Trigger::OnCrit);
        assert_eq!(proc.chance, "lucky_hit_chance / 100 * 0.3");
        assert_eq!(proc.icd, 2.0);
        assert_eq!(proc.apply_buff.as_deref(), Some("combustion"));
        assert_eq!(proc.cast_action, None);

        assert_eq!(def.damage_objective, "hit_after_dr");

        // Round-trip through serde again (idempotence).
        let reparsed: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(reparsed.damage_objective, def.damage_objective);
    }

    // P7b: the same five positions, written as STRINGS this time — the
    // untagged enum's other arm. Both arms round-trip, so a config may mix
    // them freely (`"cost": { "mana": 40.0, "rage": "10 + rage_cost" }`).
    const EXPR_SIMDEF_JSON: &str = r#"{
      "actions": {
        "fireball": {
          "cast_time": "1.0 / base_aps",
          "cooldown": "5 + 5",
          "cost": { "mana": "20 + 10" },
          "gain": { "mana": 40.0 },
          "damage": { "stats": { "coeff_pct": "200 * 2", "hits_per_use": 1.0 } }
        }
      },
      "buffs": { "vuln_window": { "duration": "2 + bonus_dur" } },
      "damage_objective": "hit_after_dr"
    }"#;

    #[test]
    fn expression_valued_fields_deserialize_and_round_trip() {
        let def: SimDef = serde_json::from_str(EXPR_SIMDEF_JSON).unwrap();
        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cooldown, NumOrExpr::Expr("5 + 5".into()));
        assert_eq!(fireball.cost["mana"], NumOrExpr::Expr("20 + 10".into()));
        assert_eq!(fireball.gain["mana"], NumOrExpr::Num(40.0));
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], NumOrExpr::Expr("200 * 2".into()));
        assert_eq!(dmg.stats["hits_per_use"], NumOrExpr::Num(1.0));
        assert_eq!(
            def.buffs["vuln_window"].duration,
            NumOrExpr::Expr("2 + bonus_dur".into())
        );

        // Serializing puts each arm back in its own JSON shape (number
        // stays a number, expression stays a string) — round-tripping a
        // config never rewrites a literal as `"40"`.
        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(round.actions["fireball"].cooldown, fireball.cooldown);
        assert_eq!(round.actions["fireball"].gain["mana"], NumOrExpr::Num(40.0));
    }

    #[test]
    fn omitted_cooldown_defaults_to_zero() {
        let def: SimDef = serde_json::from_str(
            r#"{ "actions": { "a": { "cast_time": "1" } }, "damage_objective": "hit" }"#,
        )
        .unwrap();
        assert_eq!(def.actions["a"].cooldown, NumOrExpr::Num(0.0));
        assert!(def.actions["a"].cost.is_empty());
    }

    #[test]
    fn rotation_round_trips_the_spec_example_verbatim() {
        let r: Rotation = serde_json::from_str(ROTATION_JSON).unwrap();
        assert_eq!(r.rules.len(), 3);
        assert_eq!(r.rules[0].action, "frost_nova");
        assert_eq!(
            r.rules[0].when.as_deref(),
            Some("cooldown.frost_nova == 0 and buff.vuln_window == 0")
        );
        assert_eq!(r.rules[1].action, "fireball");
        assert_eq!(r.rules[1].when.as_deref(), Some("mana >= 40"));
        assert_eq!(r.rules[2].action, "basic_bolt");
        assert_eq!(r.rules[2].when, None);
    }
}
