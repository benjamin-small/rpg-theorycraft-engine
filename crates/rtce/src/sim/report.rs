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
    /// "Active" means AT LEAST ONE live instance — a 3-stack buff and a
    /// 1-stack buff both read `1.0` here; [`SimReport::avg_stacks`] is
    /// where the count shows up.
    pub buff_uptime: BTreeMap<String, f64>,
    /// Computed TIME-INTEGRATED mean stack count of each buff over the
    /// sim's total duration, keyed by buff name (`∫ stacks dt / total
    /// duration`) — the counted companion to [`SimReport::buff_uptime`],
    /// integrated the same way and over the same whole-sim window (not
    /// per phase, and not conditioned on the buff being up: seconds at
    /// zero stacks drag the mean down exactly as they should).
    ///
    /// For a buff that never stacks this equals its `buff_uptime`. For a
    /// stacking one the two answer different questions: a buff up the
    /// whole fight at 3 stacks reads `buff_uptime` `1.0` and `avg_stacks`
    /// `3.0`.
    pub avg_stacks: BTreeMap<String, f64>,
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
    /// Proc fire counts, keyed by proc name — the EV accumulator method's
    /// fire count in `Mode::Expected`; the MEAN (rounded to the nearest
    /// whole fire) across iterations in `Mode::MonteCarlo` (see
    /// `sim::exec` module docs).
    pub proc_counts: BTreeMap<String, u64>,
    /// `Some` only in `Mode::MonteCarlo`: the distribution of per-iteration
    /// `dps` across every iteration (`None` in `Mode::Expected`, which
    /// runs exactly once and has no distribution to report).
    pub distribution: Option<Distribution>,
}

/// Monte Carlo's summary of one run's per-iteration `dps` samples: mean,
/// population standard deviation, and three percentiles. Percentiles use
/// the NEAREST-RANK estimator (no interpolation between order statistics —
/// `rank = ceil(p/100 * n)`, 1-indexed, clamped into `[1, n]`): simple,
/// deterministic, and exact on the sorted sample itself (no rounding
/// choice needed for GAME numbers, which is the point). `std` is the
/// POPULATION standard deviation (divides by `n`, not `n - 1`) — every
/// sample IS the full population this report describes (there is no
/// larger population `n` is estimating from), so the Bessel correction
/// (which exists to de-bias a SAMPLE drawn from a larger population) does
/// not apply here.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Distribution {
    /// Arithmetic mean of every iteration's `dps`.
    pub mean: f64,
    /// Population standard deviation of every iteration's `dps`.
    pub std: f64,
    /// 10th percentile `dps` (nearest-rank estimator).
    pub p10: f64,
    /// 50th percentile `dps` (nearest-rank estimator; the median).
    pub p50: f64,
    /// 90th percentile `dps` (nearest-rank estimator).
    pub p90: f64,
}

impl Distribution {
    /// Summarize `samples` (one `dps` value per Monte Carlo iteration).
    /// pub(crate): `sim::exec` is the only caller — a `SimReport`'s
    /// `distribution` is only ever built from a real MC run's samples.
    pub(crate) fn from_samples(samples: &[f64]) -> Self {
        let n = samples.len();
        assert!(
            n > 0,
            "Distribution::from_samples requires at least one sample"
        );
        let mean = samples.iter().sum::<f64>() / n as f64;
        let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
        let std = variance.sqrt();
        let mut sorted: Vec<f64> = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("dps samples are finite"));
        Distribution {
            mean,
            std,
            p10: percentile(&sorted, 10.0),
            p50: percentile(&sorted, 50.0),
            p90: percentile(&sorted, 90.0),
        }
    }
}

/// Nearest-rank percentile of an already-SORTED-ascending slice — see
/// [`Distribution`]'s docs for why this estimator (no interpolation).
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    let rank = ((p / 100.0) * n as f64).ceil() as i64; // 1-indexed
    let idx = rank.clamp(1, n as i64) - 1;
    sorted[idx as usize]
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
