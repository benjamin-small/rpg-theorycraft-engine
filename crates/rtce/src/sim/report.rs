//! `SimReport` — the timeline simulator's output (see the design spec's
//! "Output — `SimReport`" section). Every number here is COMPUTED by
//! walking the timeline, in contrast to `Scenario`'s asserted uptimes —
//! this is Level-2's whole point.

use std::collections::BTreeMap;

/// One completed `sim::run`: per-phase and total damage/dps, per-action
/// cast/damage accounting, computed buff/condition uptimes, resource
/// health, and proc fire counts.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SimReport {
    /// One entry per scenario phase, in scenario order.
    pub phases: Vec<PhaseReport>,
    /// Totals across the whole sim (all phases combined).
    pub total: Totals,
    /// Per-action cast/damage accounting, keyed by action name.
    pub actions: BTreeMap<String, ActionReport>,
    /// Computed fraction of the sim's total duration each buff was
    /// active, keyed by buff name (`active_seconds / total duration`).
    pub buff_uptime: BTreeMap<String, f64>,
    /// Computed fraction-weighted value each condition held over the
    /// sim's total duration, keyed by condition name. While a buff drives
    /// a condition it WINS over the scenario's static uptime for that
    /// condition (see design spec); this is the resulting blended
    /// average, the Level-2 analogue of a `Scenario` phase's asserted
    /// uptime.
    pub condition_uptime: BTreeMap<String, f64>,
    /// Per-resource health: time spent starved (blocked from an
    /// otherwise-eligible action purely by insufficient resource) and
    /// time spent pinned at cap (regen wasted because the pool was full),
    /// keyed by resource name.
    pub resources: BTreeMap<String, ResourceReport>,
    /// Proc fire counts, keyed by proc name (the EV accumulator method —
    /// see `sim::exec` module docs). Dedicated fire-index/fractional-chance
    /// pins and Monte Carlo's exact-roll variant land in P6d.
    pub proc_counts: BTreeMap<String, u64>,
}

/// One scenario phase's damage/dps totals.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PhaseReport {
    /// This phase's name (matches the `Scenario` phase it came from).
    pub name: String,
    /// This phase's duration in seconds (its weight, read as seconds).
    pub duration: f64,
    /// Damage accumulated while this phase was active (casts completing
    /// during it, plus any DoT ticks integrated during it).
    pub total_damage: f64,
    /// `total_damage / duration` (`0.0` if `duration` is `0.0`).
    pub dps: f64,
}

/// Whole-sim totals (all phases combined).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Totals {
    /// Sum of every phase's duration — the sim's total simulated time.
    pub duration: f64,
    /// Sum of every phase's `total_damage`.
    pub total_damage: f64,
    /// `total_damage / duration` (`0.0` if `duration` is `0.0`).
    pub dps: f64,
}

/// One action's cast/damage accounting over the whole sim.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct ActionReport {
    /// Number of times this action was cast (rotation-driven or
    /// proc-driven free casts alike).
    pub casts: u64,
    /// Damage this action's completed casts accumulated
    /// (`damage_objective × hits`, summed).
    pub damage: f64,
    /// `damage / total.total_damage` (`0.0` if total damage is `0.0`).
    pub share: f64,
}

/// One resource's health over the whole sim.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct ResourceReport {
    /// Seconds this resource sat pinned at its cap (regen wasted).
    pub time_capped: f64,
    /// Seconds an otherwise-eligible action was blocked purely by this
    /// resource being insufficient (hard gates + `when` all passed; only
    /// cost failed).
    pub time_starved: f64,
}
