//! The EV timeline executor: a discrete-event stepper over a compiled
//! [`SimPlan`], producing a [`SimReport`] of COMPUTED uptimes/dps in place
//! of `Scenario`'s asserted ones (see the design spec's "Executor" and
//! "Scenarios — Level-2 reading" sections). One decision loop drives
//! everything: walk the rotation's rules, begin the first eligible cast,
//! and let time advance to whatever happens next.
//!
//! # Effective build fold
//!
//! At any instant the sim evaluates the underlying [`Plan`] against an
//! EFFECTIVE view, refolded only when it actually changes (buff applied/
//! expired, phase boundary crossed) — never per cast:
//! - stats: the base [`BuildState`]'s own, plus the current phase's stat
//!   overrides (buffs never override raw stats, only buckets/conditions).
//! - bucket contributions: the base build's own, plus every currently
//!   active buff's `contributions`.
//! - conditions: the current phase's static uptimes, with any ACTIVE
//!   buff's `conditions` entries WINNING over them (spec precedence rule)
//!   — `condition_value` is the one place this precedence is decided.
//!
//! A per-cast damage evaluation additionally overlays the casting action's
//! own `damage.stats` onto the effective stats (a THIRD, per-cast-only
//! layer that never touches the cached effective view) — `hits_per_use`
//! is read out of that map directly rather than fed to the [`Plan`] (see
//! [`crate::simdef::ActionDamage`] docs).
//!
//! # The sim's own slot array
//!
//! Sim expressions (`cast_time`, rule `when`, proc `chance`, resource
//! `max`/`regen_per_sec`) are compiled against the extended symbol space
//! documented on [`super`] — a flat `[Plan's own slots | sim slots]`
//! array of width [`SimPlan::slot_width`]. This module keeps ONE such
//! array (in [`SimScratch`]) and refreshes it in two tiers: the STAT/
//! CONDITION prefix only on an effective-build change (via
//! [`Plan::write_stat_condition_slots`]), the time-varying tail (`time`,
//! resource amounts, `cooldown.*`, `buff.*`, `buff_remaining.*`,
//! `casts.*`) right before every expression evaluation.
//!
//! # Event queue and the spec's named event kinds
//!
//! The design spec names six event kinds (`CastComplete`, `BuffExpire`,
//! `CooldownReady`, `ProcIcdClear`, `PhaseBoundary`, `End`). This executor
//! realizes four of them as literal heap entries and folds the rest in:
//! - `CastComplete`, `BuffExpire`, `PhaseBoundary` are real, since each
//!   carries state a re-decision needs to react to.
//! - `CooldownReady` and the resource-starvation "schedule a wake" case
//!   from the spec's decision-point bullet are UNIFIED into one generic
//!   `Wake` event: both are deterministic linear-time computations
//!   (`cooldown_ready_at`, or the earliest time linear regen affords a
//!   cost), and re-running the full priority-ordered decision at whichever
//!   is earliest reproduces both without two near-identical variants.
//! - `ProcIcdClear` needs no event at all: an ICD is a passive gate
//!   checked only when a trigger fires, never something a rule waits on.
//! - `End` needs no event either: the run loop's own termination check
//!   (`heap top time > duration`, or heap empty) reaches `duration`
//!   exactly, INCLUDING a cast that completes AT the boundary (needed for
//!   the keystone cross-check — a cast starting at `duration −
//!   cast_time` must still count). Scheduling a sentinel `End` entry would
//!   have to out-rank same-instant real events via a second ordering key
//!   ON TOP OF `seq`, which the spec's own "seq tiebreaker" wording does
//!   not ask for — the duration check gets the identical result with one
//!   fewer moving part.
//!
//! Procs fire via the EV ACCUMULATOR method described in the design spec
//! (`acc += chance` per qualifying roll, subject to ICD; fires and resets
//! `acc -= 1.0` on crossing `1.0`) — with `chance == 1.0` this degenerates
//! to "fires every unblocked roll", which is all `Mode::Expected`
//! exercises today; `SimReport::proc_counts` already reports each proc's
//! fire count, but the accumulator's own DEDICATED pins (fractional
//! chances, fire-index hand-derivations) and `Mode::MonteCarlo` land in
//! P6d. `on_crit` never fires yet (EV mode has
//! no discrete crit EVENT to hang it on without full branch tracking —
//! left for P6d); `ProcEffect::CastAction` (a proc casting a free action)
//! is implemented conservatively (gains + damage, no cost/cooldown, no
//! further proc rolls, to avoid reentrancy) and untested — no fixture
//! here exercises it.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use super::compile::{ProcEffect, SimPlan};
use super::report::{ActionReport, PhaseReport, ResourceReport, SimReport, Totals};
use crate::build::BuildState;
use crate::plan::{EvalScratch, Plan, PlanError};
use crate::scenario::{Phase, Scenario};
use crate::simdef::Trigger;

/// Execution fidelity for [`run`]. `Expected` is the only mode today (the
/// deterministic branch-blended/accumulator engine described in the
/// module docs); `Mode::MonteCarlo { iterations, seed }` arrives in P6d as
/// a new variant, not a signature break — callers that already `match`
/// exhaustively will get a compile error pointing at the new arm, which is
/// the point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Deterministic branch-blended timeline: `Plan::evaluate`'s own
    /// engine driven once per cast/tick, procs via the accumulator method.
    Expected,
}

/// Preallocated executor buffers: a [`Plan`] [`EvalScratch`] (for every
/// `Plan::evaluate_phase` call the sim makes) plus the sim's own extended
/// slot array (see module docs). [`run`] builds one internally per call in
/// v1 — batch reuse across repeated `run` calls (mirroring
/// `Plan::scratch`'s role for `evaluate`) is a later phase, once a driver
/// actually needs to price many sims back to back.
pub struct SimScratch {
    eval: EvalScratch,
    slots: Vec<f64>,
}

impl SimScratch {
    /// Allocate scratch sized to `plan`/`sim_plan`'s layout.
    pub fn new(plan: &Plan, sim_plan: &SimPlan) -> Self {
        SimScratch {
            eval: plan.scratch(),
            slots: vec![0.0; sim_plan.slot_width],
        }
    }
}

/// Run `sim_plan`'s rotation once against `build` in `scenario`, producing
/// a [`SimReport`] of computed uptimes/dps. Owns its [`SimScratch`]
/// internally for v1 (see that type's docs).
pub fn run(
    plan: &Plan,
    sim_plan: &SimPlan,
    build: &BuildState,
    scenario: &Scenario,
    mode: Mode,
) -> Result<SimReport, PlanError> {
    match mode {
        Mode::Expected => {}
    }
    if scenario.phases.is_empty() {
        return Err(PlanError {
            what: "scenario has no phases".into(),
        });
    }
    for phase in &scenario.phases {
        if !phase.weight.is_finite() || phase.weight < 0.0 {
            return Err(PlanError {
                what: format!(
                    "phase `{}` weight must be finite and non-negative, got {}",
                    phase.name, phase.weight
                ),
            });
        }
    }
    let duration: f64 = scenario.phases.iter().map(|p| p.weight).sum();
    if !duration.is_finite() || duration <= 0.0 {
        return Err(PlanError {
            what: "phase weights must sum > 0 (Level-2 reads weight as seconds)".into(),
        });
    }

    let scratch = SimScratch::new(plan, sim_plan);
    let mut sim = Sim::new(plan, sim_plan, build, scenario, duration, scratch)?;
    sim.run_loop()?;
    Ok(sim.into_report())
}

/// A finite `f64` wrapper with a total order — event times are validated
/// finite at scheduling time (see [`Sim::schedule`]), so `Ord` can never
/// observe a `NaN`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FTime(f64);
impl Eq for FTime {}
impl PartialOrd for FTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .partial_cmp(&other.0)
            .expect("event times are finite by construction — see Sim::schedule")
    }
}

/// One entry the discrete-event queue can hold — see module docs for why
/// this list is shorter than the design spec's six named kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Event {
    /// `action`'s in-flight cast finishes at this time.
    CastComplete { action: usize },
    /// `buff` expires at this time, IF it is still at `generation` (a
    /// refresh since this was scheduled bumps the generation, making the
    /// event stale — processed as a no-op; see [`Sim::handle_buff_expire`]).
    BuffExpire { buff: usize, generation: u64 },
    /// The scenario crosses into phase index `phase` at this time.
    PhaseBoundary { phase: usize },
    /// Nothing inherently happens — this entry exists purely to force a
    /// fresh decision attempt at a computed time (cooldown clearing or a
    /// resource becoming affordable; see [`Sim::attempt_decision`]).
    Wake,
}

/// A heap entry: `(time, seq, event)`, ordered so the EARLIEST time pops
/// first and same-time ties break by ascending `seq` (first scheduled,
/// first processed) — `BinaryHeap` is a max-heap, so both comparisons are
/// reversed.
struct QueueItem {
    time: FTime,
    seq: u64,
    event: Event,
}
impl PartialEq for QueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time && self.seq == other.seq
    }
}
impl Eq for QueueItem {}
impl PartialOrd for QueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueueItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time
            .cmp(&self.time)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

/// Per-action runtime state.
struct ActionRt {
    cooldown_ready_at: f64,
    casts: u64,
    damage: f64,
}

/// Per-resource runtime state: a capped pool with continuous linear regen,
/// lazily settled (see [`Sim::settle_resource`]) rather than ticked.
struct ResourceRt {
    amount: f64,
    last_update: f64,
    time_capped: f64,
    time_starved: f64,
    /// Set when an otherwise-eligible rule is blocked purely by THIS
    /// resource; cleared (flushing the elapsed span into `time_starved`)
    /// the next time this resource is actually spent.
    starved_since: Option<f64>,
}

/// Per-buff runtime state.
struct BuffRt {
    active: bool,
    expire_at: f64,
    /// Bumped on every apply/refresh — lets a stale `BuffExpire` (from a
    /// window that was refreshed before it fired) recognize itself as
    /// stale and no-op.
    generation: u64,
    /// Start of the CURRENT continuous active span (unchanged by a
    /// refresh — only a real expiry closes the span).
    activated_at: f64,
    /// Seconds accumulated across every CLOSED active span.
    active_seconds: f64,
    /// `tick_objective` integration: the value/time last flushed.
    tick_last_eval: f64,
    tick_rate: f64,
}

/// Per-proc runtime state (EV accumulator method).
struct ProcRt {
    acc: f64,
    icd_ready_at: f64,
    fire_count: u64,
}

/// Running integral of one tracked condition's effective value over time
/// (buff-driven while a buff is active, else the current phase's static
/// uptime — see [`Sim::condition_value`]).
struct CondAccum {
    seconds: f64,
    value: f64,
    since: f64,
}

/// All executor state for one `run` call. Holds `&'a` references to its
/// immutable inputs (`plan`/`sim_plan`/`build`/`scenario` are all
/// `Copy`-able reference fields — reading one out, e.g. `let sim_plan =
/// self.sim_plan;`, decouples it from `self`'s own borrow, which is how
/// this module avoids fighting the borrow checker over `&mut self`
/// methods that also need to iterate the compiled config).
struct Sim<'a> {
    plan: &'a Plan,
    sim_plan: &'a SimPlan,
    build: &'a BuildState,
    scenario: &'a Scenario,

    time: f64,
    duration: f64,
    seq: u64,
    heap: BinaryHeap<QueueItem>,
    mid_cast: bool,

    actions: Vec<ActionRt>,
    resources: Vec<ResourceRt>,
    resource_max: Vec<f64>,
    resource_regen: Vec<f64>,
    buffs: Vec<BuffRt>,
    active_buff_set: Vec<usize>,
    procs: Vec<ProcRt>,

    current_phase: usize,
    phase_damage: Vec<f64>,

    /// Current phase's stats, with uptimes overridden by the buff
    /// precedence rule — refreshed only on a state change (see
    /// [`Sim::refresh_after_change`]).
    effective_phase: Phase,
    /// Base build's stats + every active buff's bucket contributions.
    effective_damage_build: BuildState,

    scratch: SimScratch,

    /// Every condition name that ever appears in a scenario phase's
    /// uptimes or a buff's `conditions` map — the reporting surface for
    /// `condition_uptime`.
    condition_names: Vec<String>,
    condition_accum: BTreeMap<String, CondAccum>,

    total_damage: f64,
}

impl<'a> Sim<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        plan: &'a Plan,
        sim_plan: &'a SimPlan,
        build: &'a BuildState,
        scenario: &'a Scenario,
        duration: f64,
        scratch: SimScratch,
    ) -> Result<Self, PlanError> {
        let n_actions = sim_plan.actions.len();
        let n_resources = sim_plan.resources.len();
        let n_buffs = sim_plan.buffs.len();
        let n_procs = sim_plan.procs.len();

        let mut condition_set = BTreeSet::new();
        for p in &scenario.phases {
            for k in p.uptimes.keys() {
                condition_set.insert(k.clone());
            }
        }
        for b in &sim_plan.buffs {
            for k in b.conditions.keys() {
                condition_set.insert(k.clone());
            }
        }
        let condition_names: Vec<String> = condition_set.into_iter().collect();

        let mut sim = Sim {
            plan,
            sim_plan,
            build,
            scenario,
            time: 0.0,
            duration,
            seq: 0,
            heap: BinaryHeap::new(),
            mid_cast: false,
            actions: (0..n_actions)
                .map(|_| ActionRt {
                    cooldown_ready_at: 0.0,
                    casts: 0,
                    damage: 0.0,
                })
                .collect(),
            resources: (0..n_resources)
                .map(|_| ResourceRt {
                    amount: 0.0,
                    last_update: 0.0,
                    time_capped: 0.0,
                    time_starved: 0.0,
                    starved_since: None,
                })
                .collect(),
            resource_max: vec![0.0; n_resources],
            resource_regen: vec![0.0; n_resources],
            buffs: (0..n_buffs)
                .map(|_| BuffRt {
                    active: false,
                    expire_at: 0.0,
                    generation: 0,
                    activated_at: 0.0,
                    active_seconds: 0.0,
                    tick_last_eval: 0.0,
                    tick_rate: 0.0,
                })
                .collect(),
            active_buff_set: Vec::new(),
            procs: (0..n_procs)
                .map(|_| ProcRt {
                    acc: 0.0,
                    icd_ready_at: 0.0,
                    fire_count: 0,
                })
                .collect(),
            current_phase: 0,
            phase_damage: vec![0.0; scenario.phases.len()],
            effective_phase: scenario.phases[0].clone(),
            effective_damage_build: build.clone(),
            scratch,
            condition_names,
            condition_accum: BTreeMap::new(),
            total_damage: 0.0,
        };

        for name in sim.condition_names.clone() {
            let v = sim.condition_value(&name);
            sim.condition_accum.insert(
                name,
                CondAccum {
                    seconds: 0.0,
                    value: v,
                    since: 0.0,
                },
            );
        }

        sim.refresh_effective_state()?;
        for ri in 0..n_resources {
            sim.resources[ri].amount = sim.resource_max[ri];
            sim.resources[ri].last_update = 0.0;
        }

        // Phase boundaries are known upfront (weights are static config) —
        // schedule every boundary BETWEEN phases now (the last phase's end
        // is just `duration`, needing no event; see module docs on `End`).
        let mut acc = 0.0;
        for (i, p) in scenario.phases.iter().enumerate() {
            acc += p.weight;
            if i + 1 < scenario.phases.len() {
                sim.schedule(acc, Event::PhaseBoundary { phase: i + 1 })?;
            }
        }

        Ok(sim)
    }

    /// Push `event` at `time`, validating finiteness fail-closed (cast
    /// times/durations are expression-derived, never guessed to be sane).
    fn schedule(&mut self, time: f64, event: Event) -> Result<(), PlanError> {
        if !time.is_finite() || time < 0.0 {
            return Err(PlanError {
                what: format!("sim: scheduled a non-finite/negative time ({time}) for {event:?}"),
            });
        }
        self.seq += 1;
        self.heap.push(QueueItem {
            time: FTime(time),
            seq: self.seq,
            event,
        });
        Ok(())
    }

    /// The effective value of condition `name` right now: an active buff
    /// driving it WINS over the current phase's static uptime (spec
    /// precedence rule); first match in buff-index order if more than one
    /// active buff drives the same name.
    fn condition_value(&self, name: &str) -> f64 {
        for &bi in &self.active_buff_set {
            if let Some(v) = self.sim_plan.buffs[bi].conditions.get(name) {
                return *v;
            }
        }
        self.scenario.phases[self.current_phase]
            .uptimes
            .get(name)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }

    /// Recompute everything derived from `(current_phase, active_buff_set)`:
    /// `effective_phase`, the sim slot array's stat/condition prefix, the
    /// resources' cached `max`/`regen_per_sec`, and `effective_damage_build`.
    /// Call between [`Sim::flush_before_change`] and
    /// [`Sim::refresh_after_change`] whenever either input actually
    /// changes — never per cast/tick.
    fn refresh_effective_state(&mut self) -> Result<(), PlanError> {
        let phase = &self.scenario.phases[self.current_phase];
        let mut uptimes = phase.uptimes.clone();
        for name in &self.condition_names {
            uptimes.insert(name.clone(), self.condition_value(name));
        }
        self.effective_phase = Phase {
            name: phase.name.clone(),
            weight: 1.0,
            uptimes,
            stats: phase.stats.clone(),
        };

        self.plan.write_stat_condition_slots(
            self.build,
            &self.effective_phase,
            &mut self.scratch.slots,
        )?;
        for (ri, r) in self.sim_plan.resources.iter().enumerate() {
            self.resource_max[ri] = r.max.eval(&self.scratch.slots);
            self.resource_regen[ri] = r.regen_per_sec.eval(&self.scratch.slots);
        }

        let mut contributions = self.build.contributions.clone();
        for &bi in &self.active_buff_set {
            contributions.extend(self.sim_plan.buffs[bi].contributions.iter().cloned());
        }
        self.effective_damage_build = BuildState {
            stats: self.build.stats.clone(),
            contributions,
        };
        Ok(())
    }

    /// Flush condition-uptime and `tick_objective` integration up to `now`
    /// USING THE STATE BEFORE the caller's upcoming change (must run
    /// before `current_phase`/`active_buff_set` are mutated, so elapsed
    /// time is attributed to the OLD phase/buff set).
    fn flush_before_change(&mut self) {
        let now = self.time;
        self.flush_conditions(now);
        self.flush_ticks(now);
    }

    fn flush_conditions(&mut self, now: f64) {
        for e in self.condition_accum.values_mut() {
            let elapsed = now - e.since;
            e.seconds += elapsed * e.value;
            e.since = now;
        }
    }

    fn flush_ticks(&mut self, now: f64) {
        for &bi in &self.active_buff_set {
            if self.sim_plan.buffs[bi].tick_objective.is_some() {
                let b = &mut self.buffs[bi];
                let elapsed = now - b.tick_last_eval;
                let dmg = elapsed * b.tick_rate;
                b.tick_last_eval = now;
                self.total_damage += dmg;
                self.phase_damage[self.current_phase] += dmg;
            }
        }
    }

    /// The other half of a state-change transaction: after the caller has
    /// mutated `current_phase`/`active_buff_set`, refold everything
    /// derived from them and reset the condition/tick integrators to
    /// start counting the NEW value from `now`.
    fn refresh_after_change(&mut self) -> Result<(), PlanError> {
        let now = self.time;
        self.refresh_effective_state()?;
        for name in self.condition_names.clone() {
            let v = self.condition_value(&name);
            let e = self
                .condition_accum
                .get_mut(&name)
                .expect("seeded in Sim::new for every tracked condition");
            e.value = v;
            e.since = now;
        }
        let active = self.active_buff_set.clone();
        for bi in active {
            if let Some(obj) = self.sim_plan.buffs[bi].tick_objective {
                let build = self.effective_damage_build.clone();
                let phase = self.effective_phase.clone();
                let objs = self
                    .plan
                    .evaluate_phase(&build, &phase, &mut self.scratch.eval)?;
                let val = objs[obj];
                let b = &mut self.buffs[bi];
                b.tick_rate = val;
                b.tick_last_eval = now;
            }
        }
        Ok(())
    }

    /// Apply (or refresh, if already active) `bi`: pay-free, instantaneous.
    /// Refresh-on-reapply resets `expire_at` but leaves the active span's
    /// `activated_at`/effective-state untouched (see [`BuffRt`] docs).
    fn apply_buff(&mut self, bi: usize) -> Result<(), PlanError> {
        let now = self.time;
        if !self.buffs[bi].active {
            self.flush_before_change();
            self.buffs[bi].active = true;
            self.buffs[bi].activated_at = now;
            self.active_buff_set.push(bi);
            self.active_buff_set.sort_unstable();
            self.refresh_after_change()?;
        }
        self.buffs[bi].generation += 1;
        self.buffs[bi].expire_at = now + self.sim_plan.buffs[bi].duration;
        let generation = self.buffs[bi].generation;
        self.schedule(
            self.buffs[bi].expire_at,
            Event::BuffExpire {
                buff: bi,
                generation,
            },
        )?;
        Ok(())
    }

    fn handle_buff_expire(&mut self, bi: usize, generation: u64) -> Result<(), PlanError> {
        if !self.buffs[bi].active || self.buffs[bi].generation != generation {
            return Ok(()); // stale — refreshed since this was scheduled.
        }
        let now = self.time;
        self.flush_before_change();
        self.buffs[bi].active_seconds += now - self.buffs[bi].activated_at;
        self.buffs[bi].active = false;
        self.active_buff_set.retain(|&x| x != bi);
        self.refresh_after_change()
    }

    /// Settle `ri`'s continuous linear regen up to `now`, crediting any
    /// time spent pinned at cap into `time_capped`.
    fn settle_resource(&mut self, ri: usize, now: f64) {
        let max = self.resource_max[ri];
        let regen = self.resource_regen[ri];
        let r = &mut self.resources[ri];
        let dt = now - r.last_update;
        if dt > 0.0 {
            let projected = r.amount + regen * dt;
            if projected > max {
                if r.amount >= max {
                    r.time_capped += dt;
                } else if regen > 0.0 {
                    let t_reach = (max - r.amount) / regen;
                    r.time_capped += (dt - t_reach).max(0.0);
                }
                r.amount = max;
            } else {
                r.amount = projected;
            }
            r.last_update = now;
        }
    }

    fn resource_amount_now(&self, ri: usize, now: f64) -> f64 {
        let r = &self.resources[ri];
        let dt = now - r.last_update;
        if dt <= 0.0 {
            return r.amount.min(self.resource_max[ri]);
        }
        (r.amount + self.resource_regen[ri] * dt).min(self.resource_max[ri])
    }

    fn cost_payable(&self, action: usize, now: f64) -> bool {
        self.sim_plan.actions[action]
            .cost
            .iter()
            .all(|&(ri, amt)| self.resource_amount_now(ri, now) >= amt)
    }

    /// Earliest time every cost in `action`'s cost map is simultaneously
    /// payable (linear regen — solvable exactly), or `None` if some cost
    /// can never be met (zero/negative regen while short, or the amount
    /// exceeds the resource's own cap).
    fn earliest_afford(&self, action: usize, now: f64) -> Option<f64> {
        let mut latest = now;
        for &(ri, amt) in &self.sim_plan.actions[action].cost {
            let cur = self.resource_amount_now(ri, now);
            if cur >= amt {
                continue;
            }
            let regen = self.resource_regen[ri];
            if regen <= 0.0 || amt > self.resource_max[ri] {
                return None;
            }
            let t = now + (amt - cur) / regen;
            if t > latest {
                latest = t;
            }
        }
        Some(latest)
    }

    fn mark_starved(&mut self, ri: usize, now: f64) {
        if self.resources[ri].starved_since.is_none() {
            self.resources[ri].starved_since = Some(now);
        }
    }

    fn clear_starved(&mut self, ri: usize, now: f64) {
        if let Some(since) = self.resources[ri].starved_since.take() {
            self.resources[ri].time_starved += now - since;
        }
    }

    fn pay_cost(&mut self, action: usize, now: f64) {
        let costs = self.sim_plan.actions[action].cost.clone();
        for (ri, amt) in costs {
            self.settle_resource(ri, now);
            self.resources[ri].amount -= amt;
            self.clear_starved(ri, now);
        }
    }

    fn apply_gain(&mut self, action: usize, now: f64) {
        let gains = self.sim_plan.actions[action].gain.clone();
        for (ri, amt) in gains {
            self.settle_resource(ri, now);
            let max = self.resource_max[ri];
            self.resources[ri].amount = (self.resources[ri].amount + amt).min(max);
        }
    }

    /// Refresh the sim slot array's TIME-VARYING tail (everything past the
    /// stat/condition prefix — see module docs). Cheap; called before
    /// every `when`/`chance`/`cast_time` evaluation so those always see
    /// the current instant.
    fn refresh_time_varying_slots(&mut self) {
        let now = self.time;
        let sim_plan = self.sim_plan;
        let sb = sim_plan.sim_base;
        self.scratch.slots[sb] = now;
        self.scratch.slots[sb + 1] = self.duration;

        let resource_base = sb + 2;
        for ri in 0..sim_plan.resources.len() {
            self.scratch.slots[resource_base + ri] = self.resource_amount_now(ri, now);
        }
        let cooldown_base = resource_base + sim_plan.resources.len();
        for ai in 0..sim_plan.actions.len() {
            self.scratch.slots[cooldown_base + ai] =
                (self.actions[ai].cooldown_ready_at - now).max(0.0);
        }
        let buff_base = cooldown_base + sim_plan.actions.len();
        for bi in 0..sim_plan.buffs.len() {
            self.scratch.slots[buff_base + bi] = if self.buffs[bi].active { 1.0 } else { 0.0 };
        }
        let buff_remaining_base = buff_base + sim_plan.buffs.len();
        for bi in 0..sim_plan.buffs.len() {
            self.scratch.slots[buff_remaining_base + bi] = if self.buffs[bi].active {
                (self.buffs[bi].expire_at - now).max(0.0)
            } else {
                0.0
            };
        }
        let casts_base = buff_remaining_base + sim_plan.buffs.len();
        for ai in 0..sim_plan.actions.len() {
            self.scratch.slots[casts_base + ai] = self.actions[ai].casts as f64;
        }
    }

    /// Walk the rotation, chaining instant (`cast_time == 0`) casts, until
    /// the character is mid-cast, nothing is eligible, or a wake has been
    /// scheduled for the earliest moment something WILL become eligible.
    fn attempt_decision(&mut self) -> Result<(), PlanError> {
        loop {
            if self.mid_cast {
                return Ok(());
            }
            let now = self.time;
            self.refresh_time_varying_slots();

            let sim_plan = self.sim_plan;
            let mut chosen: Option<usize> = None;
            let mut wake: Option<(f64, usize)> = None;

            for (ridx, rule) in sim_plan.rules.iter().enumerate() {
                if let Some(w) = &rule.when {
                    if w.eval(&self.scratch.slots) == 0.0 {
                        continue;
                    }
                }
                let action = rule.action;
                let cd_ready = self.actions[action].cooldown_ready_at;
                let cost_ok = self.cost_payable(action, now);
                if cd_ready <= now && cost_ok {
                    chosen = Some(action);
                    break;
                }
                let resource_time = if cost_ok {
                    Some(now)
                } else {
                    self.earliest_afford(action, now)
                };
                if let Some(rt) = resource_time {
                    let candidate = rt.max(cd_ready);
                    if wake.is_none_or(|(t, _)| candidate < t) {
                        wake = Some((candidate, ridx));
                    }
                }
            }

            if let Some(action) = chosen {
                self.begin_cast(action)?;
                if self.mid_cast {
                    return Ok(());
                }
                continue; // instant cast — chain, retry at the same `now`.
            }

            if let Some((t, ridx)) = wake {
                let action = self.sim_plan.rules[ridx].action;
                let cd_ready = self.actions[action].cooldown_ready_at;
                if cd_ready <= now {
                    // Cooldown isn't the blocker — whatever's short here IS
                    // resource starvation.
                    let costs = self.sim_plan.actions[action].cost.clone();
                    for (ri, amt) in costs {
                        if self.resource_amount_now(ri, now) < amt {
                            self.mark_starved(ri, now);
                        }
                    }
                }
                self.schedule(t, Event::Wake)?;
            }
            return Ok(());
        }
    }

    fn begin_cast(&mut self, action: usize) -> Result<(), PlanError> {
        let now = self.time;
        self.pay_cost(action, now);
        let cooldown = self.sim_plan.actions[action].cooldown;
        self.actions[action].cooldown_ready_at = now + cooldown;

        self.refresh_time_varying_slots();
        let ct = self.sim_plan.actions[action]
            .cast_time
            .eval(&self.scratch.slots);
        if !ct.is_finite() || ct < 0.0 {
            return Err(PlanError {
                what: format!(
                    "action `{}`: cast_time evaluated to {ct} (must be finite and >= 0)",
                    self.sim_plan.actions[action].name
                ),
            });
        }
        if ct == 0.0 {
            self.complete_cast(action)
        } else {
            self.mid_cast = true;
            self.schedule(now + ct, Event::CastComplete { action })
        }
    }

    fn complete_cast(&mut self, action: usize) -> Result<(), PlanError> {
        let now = self.time;
        self.apply_gain(action, now);
        self.actions[action].casts += 1;

        let has_damage = self.sim_plan.actions[action].damage.is_some();
        if has_damage {
            let dmg = self.eval_action_damage(action)?;
            self.total_damage += dmg;
            self.phase_damage[self.current_phase] += dmg;
            self.actions[action].damage += dmg;
        }

        self.mid_cast = false;
        self.roll_procs(Trigger::OnCast)?;
        if has_damage {
            self.roll_procs(Trigger::OnHit)?;
        }
        Ok(())
    }

    /// `damage_objective × hits` for one completed cast of `action`,
    /// evaluated against the effective build with `action`'s own
    /// `damage.stats` overlaid on top (`hits_per_use` excluded — read
    /// directly, never fed to the `Plan`; see [`crate::simdef::ActionDamage`]).
    fn eval_action_damage(&mut self, action: usize) -> Result<f64, PlanError> {
        let damage_stats = self.sim_plan.actions[action]
            .damage
            .as_ref()
            .expect("caller checked damage.is_some()")
            .clone();
        let hits = damage_stats.get("hits_per_use").copied().unwrap_or(1.0);
        let mut build = self.effective_damage_build.clone();
        for (k, v) in &damage_stats {
            if k == "hits_per_use" {
                continue;
            }
            build.stats.insert(k.clone(), *v);
        }
        let phase = self.effective_phase.clone();
        let objs = self
            .plan
            .evaluate_phase(&build, &phase, &mut self.scratch.eval)?;
        Ok(objs[self.sim_plan.damage_objective] * hits)
    }

    fn roll_procs(&mut self, trigger: Trigger) -> Result<(), PlanError> {
        let now = self.time;
        self.refresh_time_varying_slots();
        for pi in 0..self.sim_plan.procs.len() {
            if self.sim_plan.procs[pi].trigger != trigger {
                continue;
            }
            if self.procs[pi].icd_ready_at > now {
                continue;
            }
            let chance = self.sim_plan.procs[pi].chance.eval(&self.scratch.slots);
            self.procs[pi].acc += chance;
            if self.procs[pi].acc >= 1.0 {
                self.procs[pi].acc -= 1.0;
                self.procs[pi].fire_count += 1;
                self.procs[pi].icd_ready_at = now + self.sim_plan.procs[pi].icd;
                match self.sim_plan.procs[pi].effect {
                    ProcEffect::ApplyBuff(bi) => self.apply_buff(bi)?,
                    ProcEffect::CastAction(ai) => self.free_cast(ai)?,
                }
            }
        }
        Ok(())
    }

    /// A proc-triggered free cast: gains + damage only, no cost/cooldown,
    /// no further proc rolls (avoids reentrancy). Conservative and
    /// UNTESTED — no fixture in this task exercises `ProcEffect::CastAction`;
    /// tightening this is a P6d concern once a config needs it.
    fn free_cast(&mut self, action: usize) -> Result<(), PlanError> {
        let now = self.time;
        self.apply_gain(action, now);
        self.actions[action].casts += 1;
        if self.sim_plan.actions[action].damage.is_some() {
            let dmg = self.eval_action_damage(action)?;
            self.total_damage += dmg;
            self.phase_damage[self.current_phase] += dmg;
            self.actions[action].damage += dmg;
        }
        Ok(())
    }

    fn run_loop(&mut self) -> Result<(), PlanError> {
        self.attempt_decision()?;
        loop {
            if self.time >= self.duration {
                break;
            }
            let Some(top) = self.heap.peek() else { break };
            if top.time.0 > self.duration {
                break;
            }
            let item = self.heap.pop().expect("just peeked Some");
            self.time = item.time.0;
            match item.event {
                Event::CastComplete { action } => self.complete_cast(action)?,
                Event::BuffExpire { buff, generation } => {
                    self.handle_buff_expire(buff, generation)?
                }
                Event::PhaseBoundary { phase } => {
                    self.flush_before_change();
                    self.current_phase = phase;
                    self.refresh_after_change()?;
                }
                Event::Wake => {}
            }
            if self.time >= self.duration {
                break;
            }
            self.attempt_decision()?;
        }
        self.finalize();
        Ok(())
    }

    fn finalize(&mut self) {
        let now = self.duration;
        self.time = now;
        self.flush_conditions(now);
        self.flush_ticks(now);
        for b in self.buffs.iter_mut() {
            if b.active {
                b.active_seconds += now - b.activated_at;
            }
        }
        for ri in 0..self.resources.len() {
            self.settle_resource(ri, now);
            self.clear_starved(ri, now);
        }
    }

    fn into_report(self) -> SimReport {
        let phases: Vec<PhaseReport> = self
            .scenario
            .phases
            .iter()
            .zip(self.phase_damage.iter())
            .map(|(p, &dmg)| PhaseReport {
                name: p.name.clone(),
                duration: p.weight,
                total_damage: dmg,
                dps: if p.weight > 0.0 { dmg / p.weight } else { 0.0 },
            })
            .collect();

        let total = Totals {
            duration: self.duration,
            total_damage: self.total_damage,
            dps: if self.duration > 0.0 {
                self.total_damage / self.duration
            } else {
                0.0
            },
        };

        let mut actions = BTreeMap::new();
        for (ai, a) in self.sim_plan.actions.iter().enumerate() {
            let rt = &self.actions[ai];
            actions.insert(
                a.name.clone(),
                ActionReport {
                    casts: rt.casts,
                    damage: rt.damage,
                    share: if total.total_damage > 0.0 {
                        rt.damage / total.total_damage
                    } else {
                        0.0
                    },
                },
            );
        }

        let mut buff_uptime = BTreeMap::new();
        for (bi, b) in self.sim_plan.buffs.iter().enumerate() {
            let seconds = self.buffs[bi].active_seconds;
            buff_uptime.insert(
                b.name.clone(),
                if self.duration > 0.0 {
                    seconds / self.duration
                } else {
                    0.0
                },
            );
        }

        let mut condition_uptime = BTreeMap::new();
        for (name, acc) in &self.condition_accum {
            condition_uptime.insert(
                name.clone(),
                if self.duration > 0.0 {
                    acc.seconds / self.duration
                } else {
                    0.0
                },
            );
        }

        let mut resources = BTreeMap::new();
        for (ri, r) in self.sim_plan.resources.iter().enumerate() {
            let rt = &self.resources[ri];
            resources.insert(
                r.name.clone(),
                ResourceReport {
                    time_capped: rt.time_capped,
                    time_starved: rt.time_starved,
                },
            );
        }

        let mut proc_counts = BTreeMap::new();
        for (pi, p) in self.sim_plan.procs.iter().enumerate() {
            proc_counts.insert(p.name.clone(), self.procs[pi].fire_count);
        }

        SimReport {
            phases,
            total,
            actions,
            buff_uptime,
            condition_uptime,
            resources,
            proc_counts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedef::GameDef;
    use crate::plan::{self, Plan};
    use crate::sim::compile as sim_compile;
    use crate::simdef::{
        ActionDamage, ActionDef, BuffDef, ProcDef, ResourceDef, Rotation, Rule, SimDef,
    };
    use std::collections::BTreeMap;

    /// Verbatim from `plan.rs`'s own `toy_def()`/`toy_build()` (private to
    /// that module's tests) — duplicated here so this module's tests are
    /// self-contained; see `plan.rs`'s `toy_game_hand_worked_single_phase`
    /// for the shared hand-derivation these numbers rest on.
    fn toy_plan() -> Plan {
        let def: GameDef = serde_json::from_str(
            r#"{
              "stats": ["weapon", "power", "crit_chance", "enemy_dr"],
              "conditions": ["enraged"],
              "buckets": { "additive": { "fold": "sum" },
                           "crit_group": { "fold": "summed_group" },
                           "indep": { "fold": "product" } },
              "events": { "crit": { "chance": "crit_chance / 100",
                                     "factor": "1.5 * crit_group" } },
              "pipeline": [
                { "name": "base", "expr": "weapon * (1 + power / 100)" },
                { "name": "hit",
                  "expr": "base * (1 + additive / 100) * event_factors * indep",
                  "branched": true },
                { "name": "dps", "expr": "hit * (1 - enemy_dr / 100)" }
              ],
              "objectives": ["dps"]
            }"#,
        )
        .unwrap();
        plan::compile(&def).unwrap()
    }

    fn toy_build() -> BuildState {
        serde_json::from_str(
            r#"{ "stats": { "weapon": 100.0, "power": 50.0, "crit_chance": 25.0 },
                 "contributions": [
                   { "bucket": "additive", "value": 40.0 },
                   { "bucket": "additive", "value": 30.0, "event": "crit" },
                   { "bucket": "additive", "value": 20.0, "condition": "enraged" },
                   { "bucket": "crit_group", "value": 50.0 },
                   { "bucket": "indep", "value": 10.0 } ] }"#,
        )
        .unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ------------------------------------------------------------------
    // Keystone: EV timeline must reproduce Level-1's blended dps EXACTLY
    // when there's nothing for the timeline to add (no resource pressure,
    // no procs, a spammable 1s-cast action) — where the fidelity levels
    // overlap, they are REQUIRED to agree (design spec's "Testing"
    // section).
    // ------------------------------------------------------------------
    #[test]
    fn keystone_matches_level_1_exactly() {
        let plan = toy_plan();
        let build = toy_build();
        // Same single phase as plan.rs's `arena()`, but weight=10 — Level-2
        // reads phase weight as SECONDS, so this is a 10s fight.
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 10,
                   "uptimes": { "enraged": 0.5 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();

        // Level-1 oracle: plan.evaluate's own dps (weight is irrelevant to
        // a single-phase scenario — it normalizes to 1.0 regardless).
        let mut scratch = plan.scratch();
        let level1_dps = plan.evaluate(&build, &scenario, &mut scratch).unwrap()[0];
        assert!(close(level1_dps, 282.15), "got {level1_dps}");

        let mut actions = BTreeMap::new();
        actions.insert(
            "spam".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: 0.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs: BTreeMap::new(),
            procs: BTreeMap::new(),
            damage_objective: "dps".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "spam".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        // Hand-worked: cast_time 1s, no cost/cooldown → one cast per
        // second, t=0..9 starting (10 starts), completing t=1..10 — the
        // 10th cast starts at t=9 and completes EXACTLY at duration=10,
        // and must still count (see module docs on the `End` boundary).
        // total_damage = 10 × 282.15 = 2821.5 ; dps = 2821.5 / 10 = 282.15.
        assert_eq!(report.actions["spam"].casts, 10);
        assert!(
            close(report.total.total_damage, 2821.5),
            "got {}",
            report.total.total_damage
        );
        assert!(close(report.total.duration, 10.0));
        assert!(
            close(report.total.dps, level1_dps),
            "got {}",
            report.total.dps
        );
        assert!(close(report.total.dps, 282.15), "got {}", report.total.dps);
    }

    // ------------------------------------------------------------------
    // Resource starvation: a 50-cost spender against 100 max / 10-per-sec
    // regen, starting full, cast_time 1s. Hand-worked cadence (see the
    // module docs' `settle_resource`/`mark_starved`/`clear_starved` for
    // the mechanics this traces):
    //   t=0  cast #1 (mana 100→50), completes t=1
    //   t=1  mana=60≥50, cast #2 (→10), completes t=2
    //   t=2  mana=20<50 — starved from t=2, wake at t=5 (20+10·3=50)
    //   t=5  cast #3 (→0), completes t=6
    //   t=6  mana=10<50 — starved from t=6, wake at t=10
    //   t=10 cast #4 (→0), completes t=11
    //   t=11 mana=10<50 — starved from t=11, wake at t=15
    //   t=15 cast #5 (→0), completes t=16
    //   t=16 mana=10<50 — starved from t=16, wake at t=20 = duration (no 6th cast)
    // 5 casts in 20s; starved seconds = (5-2)+(10-6)+(15-11)+(20-16) = 3+4+4+4 = 15.
    // ------------------------------------------------------------------
    #[test]
    fn resource_starvation_cadence_is_hand_worked() {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg"],
                 "pipeline": [ { "name": "hit", "expr": "dmg" } ],
                 "objectives": ["hit"] }"#,
        )
        .unwrap();
        let plan = plan::compile(&def).unwrap();
        let build: BuildState = serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

        let mut resources = BTreeMap::new();
        resources.insert(
            "mana".to_string(),
            ResourceDef {
                max: "100".into(),
                regen_per_sec: "10".into(),
            },
        );
        let mut cost = BTreeMap::new();
        cost.insert("mana".to_string(), 50.0);
        let mut actions = BTreeMap::new();
        actions.insert(
            "spender".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: 0.0,
                cost,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let simdef = SimDef {
            resources,
            actions,
            buffs: BTreeMap::new(),
            procs: BTreeMap::new(),
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "spender".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["spender"].casts, 5);
        assert!(
            close(report.total.total_damage, 500.0),
            "got {}",
            report.total.total_damage
        );
        assert!(close(report.total.dps, 25.0), "got {}", report.total.dps);
        assert!(
            close(report.resources["mana"].time_starved, 15.0),
            "got {}",
            report.resources["mana"].time_starved
        );
    }

    // ------------------------------------------------------------------
    // Computed buff uptime: an on_cast proc (chance 1, icd 10) applies a
    // 4s buff (+25% indep, on top of the toy build's own indep=1.10 →
    // 1.10×1.25=1.375 while active). A cooldown-10/instant "empower"
    // action feeds the proc; a cooldown-0/cast_time-1 "filler" spams
    // in between. Because empower's cooldown (10) exactly matches the
    // proc's icd (10) and empower always wins priority the instant it's
    // off cooldown, the buff applies at EXACTLY t=0 and t=10 — windows
    // [0,4) and [10,14) — matching the plan's "applications at t=0,10"
    // pin regardless of which action's completion coincidentally rolls
    // the proc at each instant (see module docs on the accumulator).
    //   uptime = (4+4)/20 = 0.4
    // Per-hit dps: indep scales the WHOLE branch-weighted EV linearly
    // (it's a plain multiplier in both crit/no-crit branches), so
    //   dps_active = dps_inactive × (1.375/1.10) = 282.15 × 1.25 = 352.6875
    // (282.15 is the SAME keystone pin: arena, enraged=0.5, dr=20%).
    // Filler completes every second t=1..20 (same cadence as keystone).
    // Active windows land on completions t=1,2,3 and t=11,12,13 (buff
    // expires exactly ON t=4/t=14, and BuffExpire is scheduled BEFORE
    // that instant's filler completion — see module docs' seq tie-break
    // — so t=4/t=14 land INACTIVE): 6 active, 14 inactive.
    //   total_damage = 6×352.6875 + 14×282.15 = 2116.125 + 3950.1 = 6066.225
    // ------------------------------------------------------------------
    #[test]
    fn computed_buff_uptime_is_hand_worked() {
        let plan = toy_plan();
        let build = toy_build();
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 20,
                   "uptimes": { "enraged": 0.5 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "empower".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: 10.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: 0.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "power_up".to_string(),
            BuffDef {
                duration: 4.0,
                contributions: vec![crate::build::Contribution {
                    bucket: "indep".into(),
                    value: 25.0,
                    event: None,
                    condition: None,
                }],
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "empower_proc".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd: 10.0,
                apply_buff: Some("power_up".into()),
                cast_action: None,
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "dps".into(),
        };
        let rotation = Rotation {
            rules: vec![
                Rule {
                    action: "empower".into(),
                    when: None,
                },
                Rule {
                    action: "filler".into(),
                    when: None,
                },
            ],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["empower"].casts, 2);
        assert_eq!(report.actions["filler"].casts, 20);
        assert!(
            close(report.buff_uptime["power_up"], 0.4),
            "got {}",
            report.buff_uptime["power_up"]
        );
        assert!(
            close(report.total.total_damage, 6066.225),
            "got {}",
            report.total.total_damage
        );
    }

    // ------------------------------------------------------------------
    // Waiting is modeled: the rotation's only rule needs `buff.x == 1`,
    // and nothing in this SimDef ever applies buff `x` (no proc
    // references it) — the rule is permanently ineligible, there is
    // nothing to compute a wake for (a `when` predicate isn't solvable
    // like linear regen), so the sim must idle straight to `duration`
    // rather than loop forever.
    // ------------------------------------------------------------------
    #[test]
    fn waiting_is_modeled_and_terminates() {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg"],
                 "pipeline": [ { "name": "hit", "expr": "dmg" } ],
                 "objectives": ["hit"] }"#,
        )
        .unwrap();
        let plan = plan::compile(&def).unwrap();
        let build: BuildState = serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

        let mut buffs = BTreeMap::new();
        buffs.insert(
            "x".to_string(),
            BuffDef {
                duration: 4.0,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut actions = BTreeMap::new();
        actions.insert(
            "a".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: 0.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs: BTreeMap::new(),
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "a".into(),
                when: Some("buff.x == 1".into()),
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        // This call must RETURN (the test would hang/timeout otherwise) —
        // that itself is the "no infinite loop" proof.
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["a"].casts, 0);
        assert!(close(report.total.total_damage, 0.0));
        assert!(close(report.total.dps, 0.0));
        assert!(close(report.total.duration, 20.0));
    }

    // ------------------------------------------------------------------
    // Condition precedence: a buff drives `enraged=1.0` while active,
    // winning over the scenario's static `enraged=0.4` (spec precedence
    // rule). Same empower/filler/icd-10 cadence as the buff-uptime test
    // above, so the SAME window layout applies: active windows [0,4) and
    // [10,14), 6 active completions / 14 inactive, uptime 0.4.
    //   dps(enraged=1.0), dr=20%:
    //     additive_nc=60→×1.60 ; additive_crit=90→×1.90
    //     hit_nc=150×1.60×1×1.10=264 ; hit_crit=150×1.90×2.25×1.10=705.375
    //     EV=0.75×264+0.25×705.375=374.34375 ; dps=×0.8=299.475
    //   dps(enraged=0.4), dr=20%:
    //     additive_nc=48→×1.48 ; additive_crit=78→×1.78
    //     hit_nc=150×1.48×1.10=244.2 ; hit_crit=150×1.78×2.25×1.10=660.825
    //     EV=0.75×244.2+0.25×660.825=348.35625 ; dps=×0.8=278.685
    // Computed condition uptime: buff active 8/20=0.4 of the time (value
    // 1.0), inactive 0.6 of the time (value 0.4, the static uptime):
    //   0.4×1.0 + 0.6×0.4 = 0.64
    // ------------------------------------------------------------------
    #[test]
    fn condition_precedence_buff_wins_while_active() {
        let plan = toy_plan();
        let build = toy_build();
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 20,
                   "uptimes": { "enraged": 0.4 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();

        // Independent oracle: the two per-hit values, via the SAME public
        // `Plan::evaluate` used by Level-1 — no dependency on the sim's
        // own internals for these two numbers.
        let active_phase: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1,
                   "uptimes": { "enraged": 1.0 }, "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();
        let inactive_phase: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "p", "weight": 1,
                   "uptimes": { "enraged": 0.4 }, "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();
        let mut scratch = plan.scratch();
        let active_val = plan.evaluate(&build, &active_phase, &mut scratch).unwrap()[0];
        let inactive_val = plan
            .evaluate(&build, &inactive_phase, &mut scratch)
            .unwrap()[0];
        assert!(close(active_val, 299.475), "got {active_val}");
        assert!(close(inactive_val, 278.685), "got {inactive_val}");

        let mut actions = BTreeMap::new();
        actions.insert(
            "empower".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: 10.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: 0.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut conditions = BTreeMap::new();
        conditions.insert("enraged".to_string(), 1.0);
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "enrage_window".to_string(),
            BuffDef {
                duration: 4.0,
                contributions: Vec::new(),
                conditions,
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "empower_proc".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd: 10.0,
                apply_buff: Some("enrage_window".into()),
                cast_action: None,
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "dps".into(),
        };
        let rotation = Rotation {
            rules: vec![
                Rule {
                    action: "empower".into(),
                    when: None,
                },
                Rule {
                    action: "filler".into(),
                    when: None,
                },
            ],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert!(
            close(report.condition_uptime["enraged"], 0.64),
            "got {}",
            report.condition_uptime["enraged"]
        );
        let expected_total = 6.0 * active_val + 14.0 * inactive_val;
        assert!(
            close(report.total.total_damage, expected_total),
            "got {} expected {expected_total}",
            report.total.total_damage
        );
    }

    // ------------------------------------------------------------------
    // Phase boundary (own construction): two phases with different
    // enemy_dr/enraged, boundary deliberately off any cast-completion
    // integer second (10.5) so there's no same-instant tie to reason
    // about. Same spammable cast_time-1 action as the keystone test.
    //   Phase A (weight 10.5, dr=20%, enraged=0.5): completions t=1..10
    //     use phase A's per-hit value 282.15 (the keystone pin) — 10 casts.
    //   Phase B (weight 9.5, dr=0%, enraged=1.0): completions t=11..20
    //     use phase B's per-hit value 374.34375 (plan.rs's
    //     `phase_blending_weights_normalize` phase-b pin) — 10 casts.
    //   total_damage = 10×282.15 + 10×374.34375 = 2821.5 + 3743.4375 = 6564.9375
    //   duration = 10.5 + 9.5 = 20
    // ------------------------------------------------------------------
    #[test]
    fn phase_boundary_blends_two_phases() {
        let plan = toy_plan();
        let build = toy_build();
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [
                  { "name": "a", "weight": 10.5, "uptimes": { "enraged": 0.5 },
                    "stats": { "enemy_dr": 20.0 } },
                  { "name": "b", "weight": 9.5, "uptimes": { "enraged": 1.0 } } ] }"#,
        )
        .unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "spam".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: 0.0,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs: BTreeMap::new(),
            procs: BTreeMap::new(),
            damage_objective: "dps".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "spam".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["spam"].casts, 20);
        assert!(
            close(report.phases[0].total_damage, 2821.5),
            "got {}",
            report.phases[0].total_damage
        );
        assert!(
            close(report.phases[1].total_damage, 3743.4375),
            "got {}",
            report.phases[1].total_damage
        );
        assert!(
            close(report.total.total_damage, 6564.9375),
            "got {}",
            report.total.total_damage
        );
        assert!(close(report.total.duration, 20.0));
    }
}
