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
    #[serde(default)]
    pub cooldown: f64,
    /// Resource cost paid when the cast begins, by resource name.
    #[serde(default)]
    pub cost: BTreeMap<String, f64>,
    /// Resource gained when the cast completes, by resource name (e.g.
    /// basic/generator skills).
    #[serde(default)]
    pub gain: BTreeMap<String, f64>,
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
    /// action's damage.
    #[serde(default)]
    pub stats: BTreeMap<String, f64>,
}

/// One buff/debuff: a timed window that, while active, contributes to
/// buckets, drives condition values, and/or accrues a DoT objective.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuffDef {
    /// Seconds this buff lasts once applied (refresh-on-reapply resets
    /// the remaining duration back to this value).
    pub duration: f64,
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

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim from docs/superpowers/specs/2026-07-22-p6-sequencing-design.md
    // "Config surface" section (jsonc comments stripped — serde_json has no
    // comment support; every key/value is copied unchanged, including the
    // `cast_time` formula `"1.0 / base_aps"`).
    const SIMDEF_JSON: &str = r#"{
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

    // Verbatim from the spec's "Rotation (candidate-domain …)" section.
    const ROTATION_JSON: &str = r#"{ "rules": [
      { "action": "frost_nova", "when": "cooldown.frost_nova == 0 and buff.vuln_window == 0" },
      { "action": "fireball",   "when": "mana >= 40" },
      { "action": "basic_bolt" }
    ]}"#;

    #[test]
    fn simdef_round_trips_the_spec_example_verbatim() {
        let def: SimDef = serde_json::from_str(SIMDEF_JSON).unwrap();

        assert_eq!(def.resources["mana"].max, "max_mana");
        assert_eq!(def.resources["mana"].regen_per_sec, "mana_regen");

        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cast_time, "1.0 / base_aps");
        assert_eq!(fireball.cooldown, 0.0);
        assert_eq!(fireball.cost["mana"], 40.0);
        assert!(fireball.gain.is_empty());
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], 200.0);
        assert_eq!(dmg.stats["hits_per_use"], 1.0);

        assert_eq!(def.buffs["vuln_window"].duration, 4.0);
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
