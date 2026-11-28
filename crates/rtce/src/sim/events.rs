//! Public records for one resolved simulation timeline.

use serde::Serialize;

/// JSON schema version for the simulation event records.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// A record emitted by [`super::run_with_events`]. Names borrow the compiled
/// configuration. Records at the same instant arrive in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SimEvent<'a> {
    /// Simulation time in seconds.
    pub time: f64,
    /// ID of the actor that caused the event, when the engine can attribute it.
    pub source_id: Option<&'a str>,
    /// ID of the actor or opponent directly affected by the event, when known.
    pub target_id: Option<&'a str>,
    /// The operation that occurred at this instant.
    #[serde(flatten)]
    pub kind: SimEventKind<'a>,
}

impl<'a> SimEvent<'a> {
    pub(super) fn new(time: f64, kind: SimEventKind<'a>) -> Self {
        let player = Some("player");
        let opponent = Some("target");
        let (source_id, target_id) = match kind {
            SimEventKind::CastLanded {
                damage: Some(_), ..
            }
            | SimEventKind::BuffDamage { .. } => (player, opponent),
            SimEventKind::CastStarted { .. }
            | SimEventKind::SegmentStarted { .. }
            | SimEventKind::CastLanded { damage: None, .. }
            | SimEventKind::CastCancelled { .. }
            | SimEventKind::ProcFired { .. } => (player, None),
            SimEventKind::BuffChanged { cast_id, .. }
            | SimEventKind::ExpectedBuffChanged { cast_id, .. } => (cast_id.and(player), None),
            SimEventKind::ResourceChanged { cast_id, .. } => (cast_id.and(player), player),
            SimEventKind::SimulationStarted { .. }
            | SimEventKind::PhaseEntered { .. }
            | SimEventKind::SimulationEnded { .. } => (None, None),
        };
        Self {
            time,
            source_id,
            target_id,
            kind,
        }
    }
}

/// An operation in the resolved timeline. JSON uses a flat `type` tag.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(missing_docs)] // Field semantics are documented alongside each operation.
pub enum SimEventKind<'a> {
    /// The timeline began. Duration is the scenario's horizon in seconds.
    SimulationStarted { duration: f64 },
    /// The active scenario phase changed, including the initial phase.
    PhaseEntered { phase: &'a str, phase_index: usize },
    /// A cast began. Cast IDs start at one and are local to this timeline.
    /// A rotation rule index is zero-based. Free casts name their proc and
    /// have no rotation rule or cooldown. GCD time is present when armed.
    CastStarted {
        action: &'a str,
        cast_id: u64,
        rule: Option<usize>,
        triggered_by: Option<&'a str>,
        cooldown_ready_at: Option<f64>,
        gcd_ready_at: Option<f64>,
    },
    /// A zero-based segment began. Its duration is the resolved value in seconds.
    SegmentStarted {
        cast_id: u64,
        segment: usize,
        duration: f64,
    },
    /// One landing credited damage (if any), before its effects and proc rolls.
    /// A null segment denotes an unsegmented action. Zero damage on a damaging
    /// landing is distinct from a utility landing, whose damage is null.
    CastLanded {
        cast_id: u64,
        segment: Option<usize>,
        completes_cast: bool,
        damage: Option<f64>,
        hits: Option<f64>,
        crit: Option<SimCrit>,
    },
    /// A rotation rule cancelled a segmented cast after this many landings.
    CastCancelled {
        cast_id: u64,
        segments_landed: u32,
        replacement: &'a str,
    },
    /// A proc fired from a landing, immediately before its effects execute.
    ProcFired {
        proc: &'a str,
        cast_id: u64,
        icd_ready_at: f64,
    },
    /// A discrete buff changed. The stack count and next expiry describe the
    /// state after the change. Cast IDs are present for action/proc effects.
    BuffChanged {
        buff: &'a str,
        change: BuffChange,
        stacks: usize,
        next_expiry: Option<f64>,
        cast_id: Option<u64>,
    },
    /// An expected-uptime buff recorded a roll or aged one out. The probability
    /// is the resulting chance of being active. Roll fields accompany applications.
    ExpectedBuffChanged {
        buff: &'a str,
        probability: f64,
        roll_chance: Option<f64>,
        roll_expires_at: Option<f64>,
        cast_id: Option<u64>,
    },
    /// Continuous buff damage credited over the interval from `since` to the
    /// event's time, using the outgoing phase and tick rate.
    BuffDamage {
        buff: &'a str,
        since: f64,
        damage: f64,
    },
    /// A resource's visible amount, cap, and regeneration rate after an update.
    /// Regeneration is continuous between these records.
    ResourceChanged {
        resource: &'a str,
        change: ResourceChange,
        amount: f64,
        max: f64,
        regen_per_second: f64,
        cast_id: Option<u64>,
    },
    /// The horizon finished processing, with the selected timeline's total damage.
    SimulationEnded { total_damage: f64 },
}

/// Critical outcome or weight already computed for a damaging landing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimCrit {
    /// The chance used in expected-value mode.
    Expected {
        /// Probability of a crit.
        probability: f64,
    },
    /// One sampled outcome shared by the whole landing.
    Shared {
        /// Whether the shared roll crit.
        critical: bool,
    },
    /// Independently sampled hits summarized as a count.
    Counted {
        /// Number of hits that crit.
        critical_hits: u64,
        /// Number of hits sampled independently.
        sampled_hits: u64,
    },
}

/// Reason for a discrete buff record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum BuffChange {
    /// An application left a new or updated active instance.
    Applied,
    /// A weaker application left an existing stronger instance in place.
    ApplicationIgnored,
    /// One or more instances reached their scheduled expiry.
    Expired,
    /// An effect explicitly removed the active instances.
    Removed,
}

/// Reason for a resource record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum ResourceChange {
    /// The initial resource state.
    Initial,
    /// An action paid a resource cost.
    Cost,
    /// An action credited a resource gain.
    Gain,
    /// The cap or regeneration rate changed.
    Limits,
}
