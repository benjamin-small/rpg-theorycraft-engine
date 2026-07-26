//! The timeline executor: a discrete-event stepper over a compiled
//! [`SimPlan`], producing a [`SimReport`] of COMPUTED uptimes/dps in place
//! of `Scenario`'s asserted ones (see the design spec's "Executor" and
//! "Scenarios — Level-2 reading" sections). One decision loop drives
//! everything: walk the rotation's rules, begin the first eligible cast,
//! and let time advance to whatever happens next — [`Mode::Expected`] and
//! [`Mode::MonteCarlo`] share this ENTIRE loop (rotation logic is never
//! randomized; only damage/proc OUTCOMES differ by mode — see [`Sim::rng`]'s
//! doc comment and the "Procs and Monte Carlo (P6d)" section below).
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
//! # Expression-valued fields and their evaluation instants (P7b)
//!
//! Five config fields accept a literal OR an expression
//! ([`crate::simdef::NumOrExpr`]); a literal is pre-baked into a constant
//! at compile time, so a 0.2.0 config's numbers reach this executor as the
//! identical `f64` they always did. An EXPRESSION is re-evaluated at a
//! fixed instant in this loop's control flow, against the sim slot array
//! as of that instant, and validated fail-closed there (see
//! [`eval_field`]):
//!
//! | Field | Instant | Where |
//! |---|---|---|
//! | `BuffDef::duration` | at application, SNAPSHOTTED onto that window | [`Sim::apply_buff`] |
//! | `ActionDef::cooldown` | at cast start, before the cost is deducted | [`Sim::begin_cast`] |
//! | `ActionDef::cost` values | at cast start — and at every decision that CHECKS affordability | [`Sim::begin_cast`], [`Sim::afford`] |
//! | `ActionDef::gain` values | at cast complete | [`Sim::apply_gain`] |
//! | `ActionDamage::stats` values | at cast complete, ONCE per cast | [`Sim::overlay_build_for_action`] |
//!
//! Two consequences worth stating outright:
//!
//! - A cost expression is RE-CHECKED, never PREDICTED. The
//!   resource-starvation wake time [`Sim::afford`] solves for is
//!   the earliest instant linear regen affords the cost AS EVALUATED AT
//!   THAT DECISION; if the expression's value has changed by the time the
//!   wake fires, the wake just re-decides at the new value (possibly
//!   scheduling another wake). No event scheduling changed for this: a
//!   `Wake` was always a "re-run the decision here" marker with no state
//!   of its own, and it still is. Cooldown-ready wake times likewise stay
//!   precomputable, because `cooldown_ready_at` is a concrete instant
//!   fixed when the cast STARTS, not a formula re-read later.
//! - A buff's duration is snapshotted onto the window it starts: a stat or
//!   phase change afterwards never retroactively moves an expiry already
//!   on the heap. (This is also what makes P7c's snapshot DoTs coherent.)
//!
//! # The buff instance runtime (P7c)
//!
//! Every buff is an INSTANCE LIST ([`BuffRt`]), collapsed per mechanic by
//! its [`crate::simdef::ReapplyPolicy`]; a binary buff is the degenerate
//! one-instance `refresh` case and takes the same path it always did. The
//! count drives three things and deliberately not a fourth: bucket
//! contributions fold with their VALUE × the count, a `tick_objective`
//! ticks at rate × the count, and `stacks.<buff>` reads the count — while
//! `conditions` stay at their full configured value for as long as ANY
//! instance is live (a condition is an uptime fraction, not a quantity,
//! so scaling it by a stack count has no meaning).
//!
//! Expiry keeps the generation self-cancel pattern, with one event per
//! BUFF rather than per instance: at most one non-stale
//! [`Event::BuffExpire`] is on the heap per buff, scheduled at the
//! EARLIEST live expiry, and any instance-set mutation bumps the
//! generation so whatever was pending self-cancels. The handler sweeps
//! every instance whose window closed at `now` (`retain(expire_at > now)`,
//! so a reschedule can never land at or before `now`) and reschedules at
//! the new earliest if any survive — see [`Sim::handle_buff_expire`].
//!
//! The effective-fold transaction (flush the integrators → mutate →
//! refold) runs exactly when the COUNT moves. A reapplication that leaves
//! the count where it was — a `refresh`, or an `add_refresh_all` already
//! at `max_stacks` — changes only expiries, which nothing the fold reads
//! depends on, so it does no fold work at all.
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
//! # Procs and Monte Carlo (P6d, re-pinned P6 review/I1)
//!
//! [`Mode::Expected`] procs fire via the EV ACCUMULATOR method (see
//! [`Sim::roll_procs_ev`]'s doc comment for the full semantics — an ICD is
//! a HARD GATE, matching [`Mode::MonteCarlo`]'s: while a proc is on ICD, a
//! qualifying roll contributes NOTHING to the accumulator and no crossing
//! can occur, exactly the discarded mass MC's hard gate throws away; see
//! `ev_accumulator_icd_gate_discards_hits_during_icd_is_hand_worked` and
//! the ICD-bound convergence regression
//! `ev_procs_match_mc_in_icd_bound_regime_regression`) and `on_crit` procs
//! by weighting each hit's contribution by that hit's `"crit"` EVENT
//! probability rather than firing outright (see [`Plan::crit_chance`] and
//! `ev_on_crit_weights_by_crit_probability`) — the EV-consistent choice
//! that makes the accumulator's long-run fire rate agree with
//! `Mode::MonteCarlo`'s.
//!
//! Earlier (pre-review) revisions of this executor let accumulation
//! CONTINUE through an ICD and queued one deferred fire for a crossing that
//! happened mid-ICD — plausible-looking, but WRONG: it let the accumulator
//! keep banking qualifying-hit mass that MC's hard gate discards outright,
//! measurably inflating the EV fire rate in any ICD-BOUND regime (the
//! reviewer's probe: chance 0.3/hit, 1s cadence, icd 5.0, 200s — EV 40 vs
//! MC mean 27, +48%). The hard-gate semantics above are what actually makes
//! the two modes agree, in BOTH the open regime (icd short enough to never
//! bind) and the ICD-bound regime (icd long enough to routinely gate hits)
//! — see the regression test for the hand-worked trace.
//!
//! [`Mode::MonteCarlo`] procs instead ROLL exactly ([`Sim::roll_procs_mc`]:
//! `rng.next_f64() < chance`, ICD a hard gate, no accumulator — MC mode has
//! no analogue of the EV accumulator's carry-over by design), and
//! `on_crit` fires only on hits whose SAMPLED branch (via
//! [`Plan::evaluate_phase_sampled`]) actually rolled a crit (see
//! [`Sim::eval_action_damage_sampled`]) — exact, not probabilistic.
//!
//! `ProcEffect::CastAction` (a proc casting a free action, [`Sim::free_cast`])
//! is scoped identically in both modes: gains + damage only, no cost/
//! cooldown, no further proc rolls (avoids reentrancy), and its damage is
//! ALWAYS the EV/branch-blended value (even under `Mode::MonteCarlo` —
//! documented as a v1 scope limit on `free_cast`'s own doc comment, not an
//! oversight) — pinned end-to-end by
//! `proc_effect_cast_action_fires_a_free_instant_cast`.
//!
//! [`SimReport::distribution`] is `Mode::MonteCarlo`-only: `mean`/`std`
//! (population, not sample — every sample IS the reported population) and
//! `p10`/`p50`/`p90` (nearest-rank estimator) over the `iterations`
//! per-iteration `dps` values — see [`super::report::Distribution`]'s doc
//! comment. Every other `SimReport` field under `Mode::MonteCarlo` is the
//! POOLED ARITHMETIC MEAN of that field across iterations (u64 fields
//! rounded to the nearest whole count) — see [`run`]'s doc comment for why
//! "pooled means" is unambiguous here (duration is never sampled).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use super::compile::{CompiledValue, ProcEffect, SimPlan};
use super::report::{ActionReport, Distribution, PhaseReport, ResourceReport, SimReport, Totals};
use crate::build::BuildState;
use crate::plan::{EvalScratch, Plan, PlanError};
use crate::rng::{mix_seed, Pcg32};
use crate::scenario::{Phase, Scenario};
use crate::simdef::{ReapplyPolicy, Trigger};

/// Execution fidelity for [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Deterministic branch-blended timeline: `Plan::evaluate`'s own
    /// engine driven once per cast/tick, procs via the EV accumulator
    /// method (see module docs).
    Expected,
    /// `iterations` full independent timeline runs, each seeded off
    /// `seed` (see `mix_seed`'s doc comment for the derivation): procs
    /// ROLL exactly (`rng.next_f64() < chance`, ICD a hard gate, no
    /// accumulator) and damage/crits are SAMPLED
    /// (`Plan::evaluate_phase_sampled`) rather than branch-blended. The
    /// resulting [`SimReport`] carries the POOLED MEAN of every
    /// per-iteration report field (see [`run`]'s docs) plus a
    /// [`Distribution`] over the `iterations` per-iteration `dps` values.
    MonteCarlo {
        /// Number of independent timeline runs (must be `> 0`).
        iterations: u32,
        /// Master seed; iteration `i` derives its own `Pcg32` seed via
        /// `mix_seed(seed, i)`.
        seed: u64,
    },
}

/// Floating-point tolerance for the EV accumulator's `acc >= 1.0` crossing
/// check (see [`Sim::roll_procs_ev`]) — a mathematically-exact crossing can
/// land a hair below `1.0` in `f64` after repeated `+=` of a value with no
/// exact binary representation (`0.3`, notably). Absolute, not relative:
/// this makes the tolerance itself capable of a SPURIOUS early fire only
/// for a `chance` scale down around `~1e-10` (an `acc` that would need on
/// the order of `1e9` qualifying rolls to legitimately reach `1.0` could
/// instead cross the `1.0 - 1e-9` line one roll early) — harmless at every
/// `chance` scale this engine actually models (percent-ish proc rates).
const PROC_FIRE_EPSILON: f64 = 1e-9;

/// Hard bound on consecutive zero-time (`cast_time == 0.0`, `cooldown ==
/// 0.0`, cost payable) casts chained within ONE decision instant before
/// [`Sim::attempt_decision`] fails closed rather than hang (P6 review/C1)
/// — see that method's doc comment for why such a config has no finite
/// answer to compute in the first place.
const INSTANT_CHAIN_LIMIT: u32 = 10_000;

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

/// Run `sim_plan`'s rotation against `build` in `scenario` under `mode`,
/// producing a [`SimReport`] of computed uptimes/dps. `Mode::Expected` runs
/// once (a single [`SimScratch`], owned internally for v1 — batch reuse
/// across repeated `run` calls is a later phase). `Mode::MonteCarlo` runs
/// `iterations` independent timelines (a FRESH `SimScratch`/`Sim`/`Pcg32`
/// each — nothing carries across iterations except the derived seed) and
/// POOLS them: every scalar field in the returned report (`total_damage`,
/// per-action `casts`/`damage`, `buff_uptime`, `condition_uptime`,
/// per-resource `time_capped`/`time_starved`, `proc_counts`) is the
/// ARITHMETIC MEAN of that field across all `iterations` reports — chosen
/// over "return one representative iteration" because a mean is what a
/// reader actually wants from "run this fight 1000 times" (a single
/// iteration is exactly as arbitrary as its own seed), and chosen over
/// "recompute pooled `dps` from pooled `total_damage`" only in APPEARANCE:
/// since every iteration shares the identical `duration` (a deterministic
/// function of `scenario`, never sampled), `mean(total_damage) / duration`
/// and `mean(dps)` are the SAME number — the two framings coincide exactly
/// here, so "pooled means" is unambiguous. `u64` fields (`casts`,
/// `proc_counts`) round their mean to the nearest whole count. The `dps`
/// DISTRIBUTION itself (mean/std/percentiles across iterations, not
/// pooled into a single number) is reported separately via
/// [`SimReport::distribution`] — `Mode::Expected` leaves that `None` (a
/// single deterministic run has no distribution to report).
///
/// A config with UNBOUNDED zero-time casting — some action whose
/// `cast_time` evaluates to `0`, whose `cooldown` is `0.0`, and whose cost
/// stays payable forever — has no finite `dps` to compute: the rotation
/// would legitimately recast it infinitely many times without time ever
/// advancing. This function does not hang on such a config; it fails
/// closed with a [`PlanError`] naming the offending action and instant
/// once [`Sim::attempt_decision`]'s per-instant chain bound is exceeded
/// (P6 review/C1 — see that method's doc comment).
pub fn run(
    plan: &Plan,
    sim_plan: &SimPlan,
    build: &BuildState,
    scenario: &Scenario,
    mode: Mode,
) -> Result<SimReport, PlanError> {
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

    match mode {
        Mode::Expected => {
            let scratch = SimScratch::new(plan, sim_plan);
            let mut sim = Sim::new(plan, sim_plan, build, scenario, duration, scratch, None)?;
            sim.run_loop()?;
            Ok(sim.into_report())
        }
        Mode::MonteCarlo { iterations, seed } => {
            run_monte_carlo(plan, sim_plan, build, scenario, duration, iterations, seed)
        }
    }
}

/// `Mode::MonteCarlo`'s own loop — see [`run`]'s doc comment for the
/// pooling contract this builds.
#[allow(clippy::too_many_arguments)]
fn run_monte_carlo(
    plan: &Plan,
    sim_plan: &SimPlan,
    build: &BuildState,
    scenario: &Scenario,
    duration: f64,
    iterations: u32,
    seed: u64,
) -> Result<SimReport, PlanError> {
    if iterations == 0 {
        return Err(PlanError {
            what: "Mode::MonteCarlo requires iterations > 0".into(),
        });
    }
    let n = f64::from(iterations);

    let mut dps_samples: Vec<f64> = Vec::with_capacity(iterations as usize);
    let mut phase_damage_sum: Vec<f64> = vec![0.0; scenario.phases.len()];
    let mut total_damage_sum = 0.0;
    let mut action_casts_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut action_damage_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut buff_uptime_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut avg_stacks_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut condition_uptime_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut resource_capped_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut resource_starved_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut proc_count_sum: BTreeMap<String, f64> = BTreeMap::new();

    for i in 0..iterations {
        let iter_seed = mix_seed(seed, u64::from(i));
        let rng = Pcg32::new(iter_seed);
        let scratch = SimScratch::new(plan, sim_plan);
        let mut sim = Sim::new(
            plan,
            sim_plan,
            build,
            scenario,
            duration,
            scratch,
            Some(rng),
        )?;
        sim.run_loop()?;
        let report = sim.into_report();

        dps_samples.push(report.total.dps);
        total_damage_sum += report.total.total_damage;
        for (idx, p) in report.phases.iter().enumerate() {
            phase_damage_sum[idx] += p.total_damage;
        }
        for (name, a) in &report.actions {
            *action_casts_sum.entry(name.clone()).or_insert(0.0) += a.casts as f64;
            *action_damage_sum.entry(name.clone()).or_insert(0.0) += a.damage;
        }
        for (name, v) in &report.buff_uptime {
            *buff_uptime_sum.entry(name.clone()).or_insert(0.0) += v;
        }
        for (name, v) in &report.avg_stacks {
            *avg_stacks_sum.entry(name.clone()).or_insert(0.0) += v;
        }
        for (name, v) in &report.condition_uptime {
            *condition_uptime_sum.entry(name.clone()).or_insert(0.0) += v;
        }
        for (name, r) in &report.resources {
            *resource_capped_sum.entry(name.clone()).or_insert(0.0) += r.time_capped;
            *resource_starved_sum.entry(name.clone()).or_insert(0.0) += r.time_starved;
        }
        for (name, c) in &report.proc_counts {
            *proc_count_sum.entry(name.clone()).or_insert(0.0) += *c as f64;
        }
    }

    let phases: Vec<PhaseReport> = scenario
        .phases
        .iter()
        .zip(phase_damage_sum.iter())
        .map(|(p, &sum)| {
            let dmg = sum / n;
            PhaseReport {
                name: p.name.clone(),
                duration: p.weight,
                total_damage: dmg,
                dps: if p.weight > 0.0 { dmg / p.weight } else { 0.0 },
            }
        })
        .collect();

    let total_damage_mean = total_damage_sum / n;
    let total = Totals {
        duration,
        total_damage: total_damage_mean,
        dps: if duration > 0.0 {
            total_damage_mean / duration
        } else {
            0.0
        },
    };

    let mut actions = BTreeMap::new();
    for (name, casts_sum) in &action_casts_sum {
        let dmg = action_damage_sum.get(name).copied().unwrap_or(0.0) / n;
        actions.insert(
            name.clone(),
            ActionReport {
                casts: (casts_sum / n).round() as u64,
                damage: dmg,
                share: if total_damage_mean > 0.0 {
                    dmg / total_damage_mean
                } else {
                    0.0
                },
            },
        );
    }

    let mut buff_uptime = BTreeMap::new();
    for (name, v) in &buff_uptime_sum {
        buff_uptime.insert(name.clone(), v / n);
    }
    let mut avg_stacks = BTreeMap::new();
    for (name, v) in &avg_stacks_sum {
        avg_stacks.insert(name.clone(), v / n);
    }
    let mut condition_uptime = BTreeMap::new();
    for (name, v) in &condition_uptime_sum {
        condition_uptime.insert(name.clone(), v / n);
    }
    let mut resources = BTreeMap::new();
    for (name, capped) in &resource_capped_sum {
        let starved = resource_starved_sum.get(name).copied().unwrap_or(0.0);
        resources.insert(
            name.clone(),
            ResourceReport {
                time_capped: capped / n,
                time_starved: starved / n,
            },
        );
    }
    let mut proc_counts = BTreeMap::new();
    for (name, sum) in &proc_count_sum {
        proc_counts.insert(name.clone(), (sum / n).round() as u64);
    }

    Ok(SimReport {
        phases,
        total,
        actions,
        buff_uptime,
        avg_stacks,
        condition_uptime,
        resources,
        proc_counts,
        distribution: Some(Distribution::from_samples(&dps_samples)),
    })
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

/// What one pass over an action's cost map found at a decision instant —
/// the two answers [`Sim::attempt_decision`]'s rule walk needs, produced
/// together so a cost EXPRESSION is evaluated once per decision rather
/// than once per question (see [`Sim::afford`]).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Afford {
    /// Every cost is payable right now.
    Now,
    /// Not payable yet; linear regen affords the whole map at this time,
    /// which is strictly later than the decision instant.
    At(f64),
    /// Never payable from here: some cost sits on a resource with
    /// zero/negative regen while short, or exceeds that resource's cap.
    Never,
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

/// ONE live application of a buff — the unit a [`BuffRt`]'s instance list
/// holds. A binary (non-stacking) buff is the degenerate case: at most one
/// of these, replaced in place on every reapplication.
struct BuffInstance {
    /// The instant this instance was applied (or, for a refresh-in-place,
    /// last reapplied).
    #[allow(dead_code)] // read by P7c-T2's snapshot tick_objective.
    applied_at: f64,
    /// The instant this instance expires.
    expire_at: f64,
    /// P7c-T2 (snapshot DoTs): the `tick_objective` rate captured at THIS
    /// instance's application, ticked unchanged to expiry. Carried but
    /// never read yet — every buff still ticks the live, re-evaluated rate.
    #[allow(dead_code)] // read by P7c-T2's snapshot tick_objective.
    snapshot_rate: f64,
}

/// Per-buff runtime state. A buff is ACTIVE exactly while `instances` is
/// non-empty; the instance list is the single source of truth for the
/// `buff.<name>`/`buff_remaining.<name>` symbols and the effective fold.
struct BuffRt {
    /// Every live application, in application order. Empty = inactive.
    instances: Vec<BuffInstance>,
    /// Bumped on every instance-set MUTATION (application or expiry
    /// sweep) — lets a stale `BuffExpire` (scheduled against an
    /// instance set that has since changed) recognize itself as stale and
    /// no-op. At most one non-stale `BuffExpire` is ever on the heap per
    /// buff, at `min(expire_at)` (see [`Sim::handle_buff_expire`]).
    generation: u64,
    /// Start of the CURRENT continuous active span (unchanged by a
    /// refresh or a stack-count change — only a drop to ZERO instances
    /// closes the span).
    activated_at: f64,
    /// Seconds accumulated across every CLOSED active span.
    active_seconds: f64,
    /// `tick_objective` integration: the value/time last flushed.
    tick_last_eval: f64,
    tick_rate: f64,
    /// `∫ stacks dt` over every span already absorbed — the numerator of
    /// the report's `avg_stacks` (see [`Sim::flush_stacks`]).
    stack_seconds: f64,
    /// Start of the span `stack_seconds` has not yet absorbed.
    stack_since: f64,
}

/// Per-proc runtime state. `acc` is EV-accumulator-only (untouched in MC
/// mode, which rolls exactly instead — see [`Sim::roll_procs_mc`]);
/// `icd_ready_at`/`fire_count` are shared by both modes.
struct ProcRt {
    /// EV mode only: the accumulator (`+= chance` per qualifying roll NOT
    /// gated by ICD, `-= 1.0` on a fire — see [`Sim::roll_procs_ev`]). A
    /// roll made while `now < icd_ready_at` skips this entirely (ICD is a
    /// hard gate in EV mode too, since P6 review/I1) — `acc` only ever
    /// holds LEFTOVER mass from a previous fire (`< 1.0`, since `>= 1.0`
    /// always fires immediately) plus whatever's accumulated since the ICD
    /// most recently cleared.
    acc: f64,
    icd_ready_at: f64,
    fire_count: u64,
}

/// Everything about ONE cast, measured together at its completion instant
/// (see [`Sim::measure_cast`]) so every `Plan` query for that cast reads
/// the same world.
struct CastMeasurement {
    /// The effective damage build with this action's evaluated
    /// `damage.stats` overlaid.
    build: BuildState,
    /// Evaluated `hits_per_use` (`1.0` when the map omits it).
    hits: f64,
    /// `Mode::Expected` only, and only when the caller rolls procs: the
    /// probability THIS hit crits, used to weight `on_crit` accumulation.
    /// `None` in `Mode::MonteCarlo` (the branch is sampled outright) and
    /// for a proc-triggered free cast (which rolls no procs).
    crit_chance: Option<f64>,
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

    /// `Some` in `Mode::MonteCarlo` (this iteration's own [`Pcg32`], never
    /// shared across iterations); `None` in `Mode::Expected`. Every method
    /// that branches on execution fidelity does so via `self.rng.is_some()`
    /// / `self.rng.as_mut()` rather than a separate `Mode` field — the RNG's
    /// presence IS the mode, by construction (see [`Sim::new`]).
    rng: Option<Pcg32>,

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
        rng: Option<Pcg32>,
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
                    instances: Vec::new(),
                    generation: 0,
                    activated_at: 0.0,
                    active_seconds: 0.0,
                    tick_last_eval: 0.0,
                    tick_rate: 0.0,
                    stack_seconds: 0.0,
                    stack_since: 0.0,
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
            rng,
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
        // A resource's `max`/`regen_per_sec` may legally name sim state
        // (`time`, `buff.<b>`, another resource…), not just stats and
        // conditions, so refresh the time-varying tail before re-deriving
        // them: they are then evaluated against the state that CAUSED
        // this refold, uniformly at every call site (buff applied, buff
        // expired, phase boundary, and `Sim::new`'s initial fold, where
        // the tail is all-zero by construction). Without this, the
        // freshness of the tail here would depend on which caller last
        // happened to refresh it — silently, since a stale read is just
        // a wrong number. See `Sim::apply_buff` for why the buff flags are
        // already correct by this point.
        self.refresh_time_varying_slots();
        for (ri, r) in self.sim_plan.resources.iter().enumerate() {
            self.resource_max[ri] = r.max.eval(&self.scratch.slots);
            self.resource_regen[ri] = r.regen_per_sec.eval(&self.scratch.slots);
        }

        let mut contributions = self.build.contributions.clone();
        for &bi in &self.active_buff_set {
            // Per-stack: the VALUE is scaled by the live instance count,
            // rather than the contribution being repeated `stacks` times.
            // The two agree in a `sum`/`summed_group` bucket and differ in
            // a `product` one — 3 stacks of `+10` fold as `×1.30`, not
            // `×1.10³` — and scaling the value is the reading a config
            // author gets by writing the per-stack magnitude once (see
            // `simdef::BuffDef`). A one-instance buff multiplies by
            // exactly `1.0`, which is the identity on every `f64`.
            let stacks = self.buffs[bi].instances.len() as f64;
            contributions.extend(self.sim_plan.buffs[bi].contributions.iter().map(|c| {
                let mut c = c.clone();
                c.value *= stacks;
                c
            }));
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
                // × stack count: k independent instances of a DoT tick k
                // times over. (A `snapshot: true` tick_objective — where
                // each instance ticks the rate it captured — is P7c-T2;
                // today every instance ticks the same live rate, so the
                // total is a plain multiple.)
                b.tick_rate = val * b.instances.len() as f64;
                b.tick_last_eval = now;
            }
        }
        Ok(())
    }

    /// ONE application of `bi`: pay-free, instantaneous, and collapsed
    /// into the instance list by the buff's
    /// [`crate::simdef::ReapplyPolicy`]:
    ///
    /// - `refresh` — one instance, its expiry reset. rtce 0.2.0's binary
    ///   buff, and the path every existing pin exercises.
    /// - `add_refresh_all` — push (up to `max_stacks`; AT the cap no new
    ///   instance is added), then reset EVERY instance's expiry to
    ///   `now + duration`: one shared clock, so the whole stack falls off
    ///   together.
    /// - `add_independent` — push an instance with its own expiry; at
    ///   `max_stacks` the earliest-expiring one is evicted first.
    /// - `strongest` — rejected at compile (P7c-T2); a fail-closed run
    ///   error here for a hand-assembled `SimPlan`.
    ///
    /// No policy ever closes the active span: `activated_at` is set only
    /// on the 0→1 transition, and only a drop to ZERO instances (in
    /// [`Sim::handle_buff_expire`]) closes it. The flush/refold
    /// transaction runs exactly when the instance COUNT moves — that is
    /// what the effective fold, the condition integrators and the tick
    /// rate all read — so a `refresh` of an already-active buff still
    /// does no work at all beyond resetting its expiry.
    ///
    /// The buff's `duration` is evaluated HERE — at the application
    /// instant — and SNAPSHOTTED onto the instance this call starts (or,
    /// for `refresh`/`add_refresh_all`, onto the window(s) it resets):
    /// nothing later reads the field again for it, so a stat/phase change
    /// afterwards cannot retroactively move an expiry already on the heap.
    ///
    /// It is evaluated against the LIVE state at that instant, whatever
    /// that state happens to be — there is no special-casing, and in
    /// particular no attempt to un-fold this buff's own effects. That
    /// means the two application paths legitimately see DIFFERENT worlds:
    ///
    /// - **First application** (buff inactive): `buff.<self>` reads `0`,
    ///   `buff_remaining.<self>` reads `0`, and any condition this buff
    ///   drives reads its non-buff value. The expression sees the world
    ///   the buff is landing on.
    /// - **Reapplication** (buff already active): the existing instances
    ///   are still fully in force, so `buff.<self>` reads `1`,
    ///   `stacks.<self>` the count BEFORE this application, any condition
    ///   this buff drives reads its BUFF-DRIVEN value, and
    ///   `buff_remaining.<self>` the longest remaining of the windows
    ///   about to be replaced or joined (the OLD expiries — nothing is
    ///   committed until after this).
    ///
    /// The refresh reading is the deliberate one, not an accident of
    /// ordering: it is what makes pandemic-style refreshes expressible as
    /// data — `"min(12, buff_remaining.x + 8)"` extends a window by 8s up
    /// to a 12s cap — which a duration blind to its own window could not
    /// express. Bucket CONTRIBUTIONS are never visible either way: buckets
    /// are not in the sim symbol space at all.
    ///
    /// Both paths are pinned by
    /// `expr_duration_reads_the_live_state_on_both_application_paths`.
    fn apply_buff(&mut self, bi: usize) -> Result<(), PlanError> {
        let now = self.time;
        self.refresh_time_varying_slots();
        let duration = self.eval_quantity(&self.sim_plan.buffs[bi].duration, || {
            format!(
                "buff `{}` duration at application (t={now})",
                self.sim_plan.buffs[bi].name
            )
        })?;
        let expire_at = now + duration;

        let policy = self.sim_plan.buffs[bi].on_reapply;
        let max_stacks = self.sim_plan.buffs[bi].max_stacks;
        let before = self.buffs[bi].instances.len();
        // `sim::compile` rejects `strongest` outright (P7c-T2), so this is
        // reachable only from a hand-assembled `SimPlan` — fail closed
        // rather than silently borrow some other policy's behavior. This
        // guard is what makes the two `unreachable!` arms below true.
        if policy == ReapplyPolicy::Strongest {
            return Err(PlanError {
                what: format!(
                    "buff `{}`: on_reapply `strongest` is not implemented (see P7c-T2) \
                     — `sim::compile` rejects it, so this `SimPlan` did not come from there",
                    self.sim_plan.buffs[bi].name
                ),
            });
        }
        // Whether the COUNT will move — decided before the mutation
        // because the fold/tick transaction has to bracket it (flush the
        // old count's elapsed seconds, mutate, refold at the new one).
        // When it does not move, nothing the effective fold reads has
        // changed and the whole transaction is skipped: that is the
        // 0.2.0 refresh path, byte for byte.
        let will_change = match policy {
            ReapplyPolicy::Refresh => before != 1,
            ReapplyPolicy::AddRefreshAll | ReapplyPolicy::AddIndependent => {
                max_stacks == 0 || before < max_stacks as usize
            }
            ReapplyPolicy::Strongest => {
                unreachable!("`strongest` is rejected at sim::compile (P7c-T2)")
            }
        };
        if will_change {
            self.flush_before_change();
        }
        self.flush_stacks(bi);
        self.buffs[bi].generation += 1;
        let generation = self.buffs[bi].generation;

        let fresh = BuffInstance {
            applied_at: now,
            expire_at,
            snapshot_rate: 0.0,
        };
        match policy {
            // One instance, its expiry reset — the binary buff.
            ReapplyPolicy::Refresh => {
                self.buffs[bi].instances.clear();
                self.buffs[bi].instances.push(fresh);
            }
            // Count +1 up to the cap, then ONE shared clock: every
            // instance's expiry is reset, including at the cap (where no
            // new instance is added but the stack still refreshes).
            ReapplyPolicy::AddRefreshAll => {
                if will_change {
                    self.buffs[bi].instances.push(fresh);
                }
                for inst in self.buffs[bi].instances.iter_mut() {
                    inst.expire_at = expire_at;
                }
            }
            // Own clock per instance; at the cap the EARLIEST-EXPIRING
            // instance is evicted (ties: the oldest of them, by
            // application order) to make room.
            ReapplyPolicy::AddIndependent => {
                if !will_change {
                    let victim = self.earliest_expiry_index(bi).expect(
                        "at the cap, so at least one instance is live \
                         (max_stacks 0 is unbounded and never reaches here)",
                    );
                    self.buffs[bi].instances.remove(victim);
                }
                self.buffs[bi].instances.push(fresh);
            }
            ReapplyPolicy::Strongest => {
                unreachable!("`strongest` is rejected at sim::compile (P7c-T2)")
            }
        }

        if before == 0 {
            self.buffs[bi].activated_at = now;
            self.active_buff_set.push(bi);
            self.active_buff_set.sort_unstable();
        }
        if will_change {
            // The instances are committed BEFORE the refold, so anything
            // the refold evaluates (a resource `max`/`regen_per_sec`,
            // notably) sees `buff.<this>`/`stacks.<this>` and a
            // `buff_remaining.<this>` that agree with each other, rather
            // than a half-applied window.
            self.refresh_after_change()?;
        }
        // One pending expiry per buff, at the EARLIEST live expiry — the
        // generation bumped above cancels whatever was pending before.
        let next = self
            .earliest_expiry(bi)
            .expect("an application always leaves at least one instance");
        self.schedule(
            next,
            Event::BuffExpire {
                buff: bi,
                generation,
            },
        )?;
        Ok(())
    }

    /// Index of `bi`'s earliest-expiring instance (the eviction victim at
    /// the cap under [`ReapplyPolicy::AddIndependent`]); ties resolve to
    /// the oldest by application order. `None` when inactive.
    fn earliest_expiry_index(&self, bi: usize) -> Option<usize> {
        self.buffs[bi]
            .instances
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.expire_at
                    .partial_cmp(&b.expire_at)
                    .expect("expiries are finite (see schedule)")
            })
            .map(|(i, _)| i)
    }

    /// The scheduled expiry sweep for `bi`: drop every instance whose
    /// window has closed at `now`, then reschedule at the new earliest
    /// expiry if any instance survives.
    ///
    /// `generation` is the self-cancel: an application since this event was
    /// scheduled bumped the counter and scheduled its own event, so this
    /// one is stale and no-ops. A reschedule from HERE reuses the SAME
    /// generation — the invariant is "at most one non-stale `BuffExpire`
    /// per buff on the heap, at `min(expire_at)`", and a sweep that leaves
    /// instances behind is still the same generation's sweep.
    fn handle_buff_expire(&mut self, bi: usize, generation: u64) -> Result<(), PlanError> {
        if self.buffs[bi].instances.is_empty() || self.buffs[bi].generation != generation {
            return Ok(()); // stale — the instance set changed since.
        }
        let now = self.time;
        let expiring = self.buffs[bi]
            .instances
            .iter()
            .filter(|i| i.expire_at <= now)
            .count();
        if expiring > 0 {
            self.flush_before_change();
            self.flush_stacks(bi);
            // `retain` leaves only instances with `expire_at > now`, so the
            // reschedule below can never land at or before `now` — no
            // same-instant expiry loop is possible.
            self.buffs[bi].instances.retain(|i| i.expire_at > now);
            if self.buffs[bi].instances.is_empty() {
                self.buffs[bi].active_seconds += now - self.buffs[bi].activated_at;
                self.active_buff_set.retain(|&x| x != bi);
            }
            self.refresh_after_change()?;
        }
        if let Some(next) = self.earliest_expiry(bi) {
            self.schedule(
                next,
                Event::BuffExpire {
                    buff: bi,
                    generation,
                },
            )?;
        }
        Ok(())
    }

    /// Absorb `bi`'s elapsed span into `∫ stacks dt` at the CURRENT stack
    /// count, then restart the span at `now`. Must run BEFORE any
    /// mutation of `bi`'s instance list, so the elapsed seconds are
    /// credited to the count that was actually live during them.
    /// Idempotent: a second call at the same instant absorbs a zero-length
    /// span and adds nothing.
    fn flush_stacks(&mut self, bi: usize) {
        let now = self.time;
        let b = &mut self.buffs[bi];
        b.stack_seconds += (now - b.stack_since) * b.instances.len() as f64;
        b.stack_since = now;
    }

    /// `bi`'s earliest instance expiry, or `None` when it is inactive.
    fn earliest_expiry(&self, bi: usize) -> Option<f64> {
        self.buffs[bi]
            .instances
            .iter()
            .map(|i| i.expire_at)
            .min_by(|a, b| {
                a.partial_cmp(b)
                    .expect("expiries are finite (see schedule)")
            })
    }

    /// `bi`'s LONGEST remaining window in seconds — the value
    /// `buff_remaining.<buff>` reads. `0.0` when inactive.
    fn longest_remaining(&self, bi: usize, now: f64) -> f64 {
        self.buffs[bi]
            .instances
            .iter()
            .map(|i| (i.expire_at - now).max(0.0))
            .fold(0.0_f64, f64::max)
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

    /// Evaluate one expression-valued sim field
    /// ([`crate::simdef::NumOrExpr`]) that is a QUANTITY — seconds of
    /// duration/cooldown, or an amount of a resource. Fail-closed: the
    /// result must be finite and `>= 0`.
    ///
    /// PRECONDITION: the caller has refreshed the slot array's
    /// time-varying tail for the current instant
    /// ([`Sim::refresh_time_varying_slots`]). A stale tail is SILENT —
    /// wrong numbers, no panic — so debug builds assert it.
    fn eval_quantity(
        &self,
        value: &CompiledValue,
        what: impl FnOnce() -> String,
    ) -> Result<f64, PlanError> {
        self.eval_field(value, true, what)
    }

    /// Evaluate one expression-valued sim field that is a STAT (a
    /// `damage.stats` entry, `hits_per_use` included). Fail-closed on
    /// non-finite only: a stat is not a quantity of anything and may
    /// legitimately be negative.
    ///
    /// PRECONDITION: as [`Sim::eval_quantity`].
    fn eval_stat(
        &self,
        value: &CompiledValue,
        what: impl FnOnce() -> String,
    ) -> Result<f64, PlanError> {
        self.eval_field(value, false, what)
    }

    /// Shared body of [`Sim::eval_quantity`]/[`Sim::eval_stat`] — never
    /// guesses a default and never clamps. `what` labels the field AND the
    /// instant, and is invoked only on the error path, so the happy path
    /// allocates nothing (the cost fields are evaluated inside the
    /// rotation's per-decision rule walk).
    fn eval_field(
        &self,
        value: &CompiledValue,
        nonneg: bool,
        what: impl FnOnce() -> String,
    ) -> Result<f64, PlanError> {
        debug_assert_eq!(
            self.scratch.slots[self.sim_plan.sim_base], self.time,
            "sim expression evaluated against a STALE slot tail — call \
             `refresh_time_varying_slots` first (see `Sim::eval_quantity`)"
        );
        let x = value.eval(&self.scratch.slots);
        if !x.is_finite() || (nonneg && x < 0.0) {
            return Err(PlanError {
                what: format!(
                    "{}: evaluated to {x} (must be finite{})",
                    what(),
                    if nonneg { " and >= 0" } else { "" }
                ),
            });
        }
        Ok(x)
    }

    /// Label for one `cost`/`gain` entry's fail-closed error — names the
    /// field AND the instant, since `cost` is evaluated at two DIFFERENT
    /// instants (merely considering a rule, versus actually casting) and a
    /// reader needs to tell them apart. Built only on the error path.
    ///
    /// In practice a BAD cost surfaces `"at a rotation decision"`: the
    /// executor never commits to a cast without checking affordability
    /// first, at the same instant and against the same slots, so
    /// [`Sim::begin_cast`]'s own `"at cast start"` evaluation cannot be
    /// reached with a value [`Sim::afford`] has not already rejected. The
    /// cast-start label exists because that evaluation genuinely happens
    /// there — it is the one that gets SPENT.
    fn resource_field_label(
        &self,
        action: usize,
        ri: usize,
        kind: &str,
        instant: &str,
        now: f64,
    ) -> String {
        format!(
            "action `{}` {kind} `{}` {instant} (t={now})",
            self.sim_plan.actions[action].name, self.sim_plan.resources[ri].name
        )
    }

    /// ONE pass over `action`'s cost map at the current instant, answering
    /// both of the rule walk's questions at once (see [`Afford`]): every
    /// amount is evaluated exactly once per decision, and a cost
    /// EXPRESSION is therefore never evaluated two or three times to
    /// answer "payable?" and then "when?".
    ///
    /// An expression cost is taken at its value AS OF now — this solves
    /// "when does linear regen reach the cost the rotation is looking at
    /// right now", it does NOT predict where a time-varying cost is
    /// heading. If the value has moved by the time the scheduled `Wake`
    /// fires, that decision re-evaluates and, if still short, schedules
    /// the next wake from there (see the module docs).
    ///
    /// PRECONDITION: as [`Sim::eval_quantity`].
    fn afford(&self, action: usize, now: f64, instant: &str) -> Result<Afford, PlanError> {
        let mut latest = now;
        let mut never = false;
        for (ri, amt) in &self.sim_plan.actions[action].cost {
            let ri = *ri;
            // Every entry is evaluated even once `never` is known, so a
            // fail-closed value in a LATER entry is never masked by an
            // unaffordable earlier one.
            let amt = self.eval_quantity(amt, || {
                self.resource_field_label(action, ri, "cost", instant, now)
            })?;
            if never {
                continue;
            }
            let cur = self.resource_amount_now(ri, now);
            if cur >= amt {
                continue;
            }
            let regen = self.resource_regen[ri];
            if regen <= 0.0 || amt > self.resource_max[ri] {
                never = true;
                continue;
            }
            let t = now + (amt - cur) / regen;
            if t > latest {
                latest = t;
            }
        }
        Ok(if never {
            Afford::Never
        } else if latest > now {
            Afford::At(latest)
        } else {
            Afford::Now
        })
    }

    /// `action`'s cost map evaluated at the current instant, ready to
    /// spend. Materialized into a `Vec` so the amounts are fixed BEFORE
    /// any resource is mutated (a cost expression that reads a resource
    /// must not see a sibling cost entry's deduction).
    ///
    /// PRECONDITION: as [`Sim::eval_quantity`].
    fn eval_costs(
        &self,
        action: usize,
        now: f64,
        instant: &str,
    ) -> Result<Vec<(usize, f64)>, PlanError> {
        self.sim_plan.actions[action]
            .cost
            .iter()
            .map(|(ri, amt)| {
                let amt = self.eval_quantity(amt, || {
                    self.resource_field_label(action, *ri, "cost", instant, now)
                })?;
                Ok((*ri, amt))
            })
            .collect()
    }

    /// `action`'s gain map evaluated at the current instant (cast
    /// complete), materialized before any resource is credited for the
    /// same reason as [`Sim::eval_costs`].
    ///
    /// PRECONDITION: as [`Sim::eval_quantity`].
    fn eval_gains(&self, action: usize, now: f64) -> Result<Vec<(usize, f64)>, PlanError> {
        self.sim_plan.actions[action]
            .gain
            .iter()
            .map(|(ri, amt)| {
                let amt = self.eval_quantity(amt, || {
                    self.resource_field_label(action, *ri, "gain", "at cast complete", now)
                })?;
                Ok((*ri, amt))
            })
            .collect()
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

    /// Spend `costs` — already evaluated by the caller at the cast-start
    /// instant (see [`Sim::eval_costs`]).
    fn pay_cost(&mut self, costs: &[(usize, f64)], now: f64) {
        for &(ri, amt) in costs {
            self.settle_resource(ri, now);
            self.resources[ri].amount -= amt;
            self.clear_starved(ri, now);
        }
    }

    /// Credit `action`'s gains AT CAST COMPLETE — the amounts are
    /// evaluated here, at that instant (see the module docs' table).
    fn apply_gain(&mut self, action: usize, now: f64) -> Result<(), PlanError> {
        if self.sim_plan.actions[action].gain.is_empty() {
            return Ok(());
        }
        self.refresh_time_varying_slots();
        let gains = self.eval_gains(action, now)?;
        for (ri, amt) in gains {
            self.settle_resource(ri, now);
            let max = self.resource_max[ri];
            self.resources[ri].amount = (self.resources[ri].amount + amt).min(max);
        }
        Ok(())
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
            self.scratch.slots[buff_base + bi] = if self.buffs[bi].instances.is_empty() {
                0.0
            } else {
                1.0
            };
        }
        let buff_remaining_base = buff_base + sim_plan.buffs.len();
        for bi in 0..sim_plan.buffs.len() {
            self.scratch.slots[buff_remaining_base + bi] = self.longest_remaining(bi, now);
        }
        let casts_base = buff_remaining_base + sim_plan.buffs.len();
        for ai in 0..sim_plan.actions.len() {
            self.scratch.slots[casts_base + ai] = self.actions[ai].casts as f64;
        }
        let stacks_base = casts_base + sim_plan.actions.len();
        for bi in 0..sim_plan.buffs.len() {
            self.scratch.slots[stacks_base + bi] = self.buffs[bi].instances.len() as f64;
        }
    }

    /// Walk the rotation, chaining instant (`cast_time == 0`) casts, until
    /// the character is mid-cast, nothing is eligible, or a wake has been
    /// scheduled for the earliest moment something WILL become eligible.
    ///
    /// A config where some action's `cast_time` evaluates to `0`, whose
    /// `cooldown` is `0.0`, and whose cost is (and stays) payable has NO
    /// FINITE ANSWER — the rotation would legitimately cast that action
    /// infinitely many times at the same instant, so its true `dps` is
    /// unbounded/undefined, not some number this executor merely failed to
    /// compute. Rather than hang (P6 review/C1), this loop counts
    /// consecutive zero-time completions chained at THIS decision instant
    /// (never across a real time advance — a fresh call to
    /// `attempt_decision` gets a fresh counter) and, past
    /// [`INSTANT_CHAIN_LIMIT`], fails closed with a [`PlanError`] naming the
    /// offending action and instant rather than spinning forever.
    fn attempt_decision(&mut self) -> Result<(), PlanError> {
        let mut instant_chain: u32 = 0;
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
                let afford = self.afford(action, now, "at a rotation decision")?;
                if cd_ready <= now && afford == Afford::Now {
                    chosen = Some(action);
                    break;
                }
                let resource_time = match afford {
                    Afford::Now => Some(now),
                    Afford::At(t) => Some(t),
                    Afford::Never => None,
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
                instant_chain += 1;
                if instant_chain > INSTANT_CHAIN_LIMIT {
                    return Err(PlanError {
                        what: format!(
                            "instant-cast livelock: action `{}` at t={now} — zero \
                             cast time, zero cooldown, payable cost — {INSTANT_CHAIN_LIMIT} \
                             consecutive zero-time completions without time advancing; \
                             this config has no finite dps (see `Sim::attempt_decision`'s \
                             doc comment)",
                            self.sim_plan.actions[action].name
                        ),
                    });
                }
                continue; // instant cast — chain, retry at the same `now`.
            }

            if let Some((t, ridx)) = wake {
                let action = self.sim_plan.rules[ridx].action;
                let cd_ready = self.actions[action].cooldown_ready_at;
                if cd_ready <= now {
                    // Cooldown isn't the blocker — whatever's short here IS
                    // resource starvation.
                    let costs = self.eval_costs(action, now, "at a rotation decision")?;
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

    /// Start `action`'s cast at `self.time`. Both expression-valued
    /// cast-start fields — `cost` and `cooldown` — are evaluated HERE, in
    /// the pre-payment state (an expression reading a resource sees what
    /// the character has BEFORE this cast spends it), and only then is the
    /// cost deducted and the cooldown armed. `cast_time` keeps its P6
    /// instant: evaluated after payment, so a `cast_time` expression still
    /// sees the post-cost resource level.
    fn begin_cast(&mut self, action: usize) -> Result<(), PlanError> {
        let now = self.time;
        self.refresh_time_varying_slots();
        let costs = self.eval_costs(action, now, "at cast start")?;
        let cooldown = self.eval_quantity(&self.sim_plan.actions[action].cooldown, || {
            format!(
                "action `{}` cooldown at cast start (t={now})",
                self.sim_plan.actions[action].name
            )
        })?;
        self.pay_cost(&costs, now);
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
        self.apply_gain(action, now)?;
        self.actions[action].casts += 1;

        let mut is_crit = false;
        let measured = self.measure_cast(action, now, true)?;
        if let Some(m) = &measured {
            let dmg = if self.rng.is_some() {
                let (dmg, crit) = self.eval_action_damage_sampled(&m.build, m.hits)?;
                is_crit = crit;
                dmg
            } else {
                self.eval_action_damage(&m.build, m.hits)?
            };
            self.total_damage += dmg;
            self.phase_damage[self.current_phase] += dmg;
            self.actions[action].damage += dmg;
        }

        self.mid_cast = false;
        if self.rng.is_some() {
            self.roll_procs_mc(Trigger::OnCast, true)?;
            if measured.is_some() {
                self.roll_procs_mc(Trigger::OnHit, true)?;
                self.roll_procs_mc(Trigger::OnCrit, is_crit)?;
            }
        } else {
            self.roll_procs_ev(Trigger::OnCast, 1.0)?;
            if let Some(m) = &measured {
                let crit_chance = m
                    .crit_chance
                    .expect("measure_cast(.., true) fills crit_chance in EV mode");
                self.roll_procs_ev(Trigger::OnHit, 1.0)?;
                self.roll_procs_ev(Trigger::OnCrit, crit_chance)?;
            }
        }
        Ok(())
    }

    /// Measure `action`'s completing cast at `now` — `None` when the
    /// action deals no damage and there is nothing to measure.
    ///
    /// Everything is taken TOGETHER, here: `damage.stats` (and
    /// `hits_per_use`) are evaluated once into one overlay, and — when
    /// `needs_crit_chance` and the run is `Mode::Expected` — EV's
    /// `on_crit` weight is read off that same overlay and the same
    /// effective phase the damage query will use. Deferring the crit query
    /// to its point of use would read a LATER state (this cast's own
    /// `on_cast`/`on_hit` procs can apply buffs and spend resources in
    /// between), so one hit's two `Plan` queries could disagree about the
    /// world the hit landed in — and a proc triggered BY this hit cannot
    /// retroactively change whether this hit crit. Pinned by
    /// `ev_on_crit_weight_is_measured_before_this_casts_own_procs`.
    ///
    /// `needs_crit_chance` is `false` for a proc-triggered free cast,
    /// which rolls no procs and would otherwise pay for a `Plan` query
    /// nothing reads.
    ///
    /// # Intra-instant ordering at cast complete
    ///
    /// The completion instant has internal order, and a `damage.stats`
    /// expression sees the state AT THIS POINT in it — which is AFTER
    /// [`Sim::apply_gain`] and after this cast's own `casts` increment,
    /// and BEFORE any of this cast's proc rolls. Concretely: a resource
    /// named in a `damage.stats` expression reads its POST-gain amount,
    /// and `casts.<this action>` INCLUDES the cast being measured (so it
    /// counts from 1 on the first cast, never 0). Both are documented on
    /// [`crate::simdef::ActionDamage`].
    fn measure_cast(
        &mut self,
        action: usize,
        now: f64,
        needs_crit_chance: bool,
    ) -> Result<Option<CastMeasurement>, PlanError> {
        if self.sim_plan.actions[action].damage.is_none() {
            return Ok(None);
        }
        self.refresh_time_varying_slots();
        let build = self.overlay_build_for_action(action, now)?;
        let hits = self.eval_hits_per_use(action, now)?;
        let crit_chance = if needs_crit_chance && self.rng.is_none() {
            Some(self.eval_action_crit_chance(&build)?)
        } else {
            None
        };
        Ok(Some(CastMeasurement {
            build,
            hits,
            crit_chance,
        }))
    }

    /// The per-cast overlay build: the effective damage build (base +
    /// active buffs' contributions) with `action`'s own `damage.stats`
    /// overlaid on top (`hits_per_use` excluded — read directly by
    /// [`Sim::eval_hits_per_use`], never fed to the `Plan`; see
    /// [`crate::simdef::ActionDamage`]). Built ONCE per cast, at cast
    /// complete, and shared by every per-cast `Plan` query that cast needs
    /// (EV damage, EV crit chance, sampled damage+mask) so all of them see
    /// the identical stats.
    ///
    /// Each value is evaluated at THIS instant (the caller refreshes the
    /// slot array's time-varying tail first) and need only be FINITE — a
    /// stat may legitimately be negative.
    fn overlay_build_for_action(&self, action: usize, now: f64) -> Result<BuildState, PlanError> {
        let damage_stats = self.sim_plan.actions[action]
            .damage
            .as_ref()
            .expect("caller checked damage.is_some()");
        let mut build = self.effective_damage_build.clone();
        for (k, v) in damage_stats {
            if k == "hits_per_use" {
                continue;
            }
            let v = self.eval_stat(v, || {
                format!(
                    "action `{}` damage.stats `{k}` at cast complete (t={now})",
                    self.sim_plan.actions[action].name
                )
            })?;
            build.stats.insert(k.clone(), v);
        }
        Ok(build)
    }

    /// `action`'s per-cast hit count — `damage.stats`'s `hits_per_use`
    /// entry (default `1.0` when absent), evaluated at the same cast-
    /// complete instant and under the same finite-only rule as the rest of
    /// that map.
    fn eval_hits_per_use(&self, action: usize, now: f64) -> Result<f64, PlanError> {
        match self.sim_plan.actions[action]
            .damage
            .as_ref()
            .expect("caller checked damage.is_some()")
            .get("hits_per_use")
        {
            Some(v) => self.eval_stat(v, || {
                format!(
                    "action `{}` damage.stats `hits_per_use` at cast complete (t={now})",
                    self.sim_plan.actions[action].name
                )
            }),
            None => Ok(1.0),
        }
    }

    /// `damage_objective × hits` for one completed cast, EV mode:
    /// `Plan::evaluate_phase`'s branch-blended value over the cast's
    /// already-built overlay (see [`Sim::overlay_build_for_action`]).
    fn eval_action_damage(&mut self, build: &BuildState, hits: f64) -> Result<f64, PlanError> {
        let phase = self.effective_phase.clone();
        let objs = self
            .plan
            .evaluate_phase(build, &phase, &mut self.scratch.eval)?;
        Ok(objs[self.sim_plan.damage_objective] * hits)
    }

    /// EV mode only: the probability the `"crit"` event fires for one hit
    /// of this cast — see [`Plan::crit_chance`]'s docs for the naming
    /// convention and the fail-soft `0.0` when this game has no `"crit"`
    /// event. Used to weight `on_crit` proc accumulation (see
    /// [`Sim::roll_procs_ev`]'s doc comment).
    fn eval_action_crit_chance(&mut self, build: &BuildState) -> Result<f64, PlanError> {
        let phase = self.effective_phase.clone();
        self.plan.crit_chance(build, &phase, &mut self.scratch.eval)
    }

    /// MC mode only: `damage_objective × hits` for one completed cast,
    /// SAMPLED (`Plan::evaluate_phase_sampled`) — returns the damage AND
    /// whether the sampled branch fired the `"crit"` event
    /// (`Plan::is_crit_bit_set`), which `complete_cast` feeds straight
    /// into the `on_crit` proc roll (no separate crit-probability query
    /// needed in MC mode — the coin was already flipped).
    fn eval_action_damage_sampled(
        &mut self,
        build: &BuildState,
        hits: f64,
    ) -> Result<(f64, bool), PlanError> {
        let phase = self.effective_phase.clone();
        let plan = self.plan;
        let rng = self.rng.as_mut().expect("caller checked rng.is_some()");
        let (objs, mask) =
            plan.evaluate_phase_sampled(build, &phase, rng, &mut self.scratch.eval)?;
        let dmg = objs[self.sim_plan.damage_objective] * hits;
        let is_crit = plan.is_crit_bit_set(mask);
        Ok((dmg, is_crit))
    }

    /// EV mode: the accumulator method (see module docs). `weight`
    /// multiplies each qualifying roll's `chance` BEFORE accumulation —
    /// `1.0` for `OnCast`/`OnHit` (every cast/hit qualifies outright), and
    /// (the EV-consistent choice this task pins, since the design spec is
    /// silent on it) `crit_chance` for `OnCrit`: a hit isn't "certainly" a
    /// crit in EV mode, so an `on_crit` proc's per-hit contribution is
    /// `proc_chance × P(crit)`, not `proc_chance` outright — this is
    /// exactly what makes the EV accumulator's LONG-RUN fire rate agree
    /// with MC mode's (which only rolls `on_crit` procs on hits that
    /// actually sampled a crit).
    ///
    /// ICD semantics (P6 review/I1 — supersedes the original P6d choice of
    /// "accumulate through ICD, defer one fire": that let the accumulator
    /// keep banking qualifying-hit mass while gated, which MC's hard gate
    /// discards outright, and measurably over-fired in any ICD-bound
    /// regime — see module docs). An ICD is now a HARD GATE, exactly
    /// mirroring [`Sim::roll_procs_mc`]: while `now < icd_ready_at`, this
    /// roll contributes NOTHING to `acc` and is skipped outright — no
    /// accumulation, no crossing, no deferred fire (a crossing simply
    /// CANNOT occur mid-ICD anymore, so there is nothing to defer). Once
    /// the ICD clears, accumulation resumes from wherever `acc` was left
    /// by the last fire (`acc -= 1.0`, never reset to `0.0`, so a fire
    /// that overshot `1.0` keeps its leftover fraction) and a crossing
    /// fires immediately — there's never a case where `now >= icd_ready_at`
    /// and `acc` has crossed `1.0` without firing on the spot, so "queue
    /// and resolve at the next qualifying roll" from the old design is
    /// gone along with the deferred flag.
    fn roll_procs_ev(&mut self, trigger: Trigger, weight: f64) -> Result<(), PlanError> {
        let now = self.time;
        for pi in 0..self.sim_plan.procs.len() {
            if self.sim_plan.procs[pi].trigger != trigger {
                continue;
            }
            if now < self.procs[pi].icd_ready_at {
                // ICD hard gate (I1): this roll's mass is discarded, not
                // banked — matches `roll_procs_mc`'s "skip outright".
                continue;
            }
            // Per-proc, not once per batch: an EARLIER proc in this same
            // batch may have fired and applied a buff / free-cast, and a
            // `chance` expression must see that (the stat/condition prefix
            // already refolded on such a change — this keeps the
            // time-varying tail, `buff.*`/`buff_remaining.*`/resources/
            // `casts.*`, telling the same story).
            self.refresh_time_varying_slots();
            let chance = self.sim_plan.procs[pi].chance.eval(&self.scratch.slots) * weight;
            self.procs[pi].acc += chance;
            // `PROC_FIRE_EPSILON` tolerance: a mathematically-exact
            // crossing (e.g. 10 additions of 0.3, which sums to exactly
            // 3.0 in decimal) can land a hair BELOW 1.0 in `f64` (`0.3`
            // itself has no exact binary representation, and repeated
            // `+=` compounds the rounding) — without the tolerance this
            // crossing would be silently missed, breaking the hand-worked
            // pin `ev_accumulator_fractional_chance_fires_at_hand_worked_hit_indices`
            // documents (10th hit lands at `0.9999999999999998`, not
            // `1.0`, without this).
            if self.procs[pi].acc >= 1.0 - PROC_FIRE_EPSILON {
                self.fire_proc(pi, now)?;
            }
        }
        Ok(())
    }

    /// MC mode: procs ROLL exactly — `rng.next_f64() < chance`, no
    /// accumulator, ICD a HARD gate (a roll blocked by ICD is simply
    /// SKIPPED, not deferred or remembered — MC mode has no analogue of
    /// the EV accumulator's carry-over, by design: each iteration is an
    /// independent sample of what actually happens, and "an ICD-gated
    /// near-miss quietly banks itself for later" is exactly the kind of
    /// EV-only smoothing MC mode exists to NOT do). `qualifies` gates the
    /// whole roll: `true` for `OnCast`/`OnHit` (every cast/hit qualifies),
    /// and — mirroring `roll_procs_ev`'s `on_crit` weighting, but exactly
    /// rather than probabilistically — whether THIS hit's sampled branch
    /// actually fired the `"crit"` event for `OnCrit` (see
    /// [`Sim::eval_action_damage_sampled`]).
    fn roll_procs_mc(&mut self, trigger: Trigger, qualifies: bool) -> Result<(), PlanError> {
        if !qualifies {
            return Ok(());
        }
        let now = self.time;
        for pi in 0..self.sim_plan.procs.len() {
            if self.sim_plan.procs[pi].trigger != trigger {
                continue;
            }
            if self.procs[pi].icd_ready_at > now {
                continue; // hard gate — no accumulation, no memory.
            }
            // Per-proc — see `roll_procs_ev` for why.
            self.refresh_time_varying_slots();
            let chance = self.sim_plan.procs[pi].chance.eval(&self.scratch.slots);
            // A fresh short-lived borrow of `self.rng`, released before
            // the match arms below need `&mut self` in full (calling
            // `self.apply_buff`/`self.free_cast`).
            let roll = self
                .rng
                .as_mut()
                .expect("caller checked rng.is_some()")
                .next_f64();
            if roll < chance {
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

    /// Fire proc `pi` at `now`: consume the accumulator (EV mode only —
    /// MC mode's `roll_procs_mc` never calls this, it applies the effect
    /// inline), start the ICD, apply the effect.
    fn fire_proc(&mut self, pi: usize, now: f64) -> Result<(), PlanError> {
        self.procs[pi].acc -= 1.0;
        self.procs[pi].fire_count += 1;
        self.procs[pi].icd_ready_at = now + self.sim_plan.procs[pi].icd;
        match self.sim_plan.procs[pi].effect {
            ProcEffect::ApplyBuff(bi) => self.apply_buff(bi)?,
            ProcEffect::CastAction(ai) => self.free_cast(ai)?,
        }
        Ok(())
    }

    /// A proc-triggered free cast: gains + damage only, no cost/cooldown,
    /// no further proc rolls (avoids reentrancy) — same scope in EV and MC
    /// mode alike: damage is ALWAYS `eval_action_damage` (EV/branch-blended),
    /// even when the firing proc itself came from a MC roll. This is a
    /// DELIBERATE v1 scope limit, not an oversight: no fixture in this
    /// crate yet drives a proc-triggered free cast under `Mode::MonteCarlo`,
    /// so sampling its damage (and feeding ITS crit back into further
    /// `on_crit` procs) is future work once a config actually needs it —
    /// tightening this later is additive, not a breaking change.
    fn free_cast(&mut self, action: usize) -> Result<(), PlanError> {
        let now = self.time;
        self.apply_gain(action, now)?;
        self.actions[action].casts += 1;
        // Same instants as a normal completion — a free cast BEGINS and
        // COMPLETES at the firing proc's instant, so `gain` and
        // `damage.stats` are both evaluated at `now`. No crit chance: this
        // path rolls no procs (see this method's doc comment).
        if let Some(m) = self.measure_cast(action, now, false)? {
            let dmg = self.eval_action_damage(&m.build, m.hits)?;
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
            if !b.instances.is_empty() {
                b.active_seconds += now - b.activated_at;
            }
            b.stack_seconds += (now - b.stack_since) * b.instances.len() as f64;
            b.stack_since = now;
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
        let mut avg_stacks = BTreeMap::new();
        for (bi, b) in self.sim_plan.buffs.iter().enumerate() {
            let seconds = self.buffs[bi].active_seconds;
            let stack_seconds = self.buffs[bi].stack_seconds;
            let (uptime, avg) = if self.duration > 0.0 {
                (seconds / self.duration, stack_seconds / self.duration)
            } else {
                (0.0, 0.0)
            };
            buff_uptime.insert(b.name.clone(), uptime);
            avg_stacks.insert(b.name.clone(), avg);
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
            avg_stacks,
            condition_uptime,
            resources,
            proc_counts,
            // A single `Sim::run` (one seed, one timeline) has no
            // distribution to report — `run_monte_carlo` builds its OWN
            // `SimReport` from `iterations` of these raw reports rather
            // than reusing this method (see that function).
            distribution: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Contribution;
    use crate::gamedef::GameDef;
    use crate::plan::{self, Plan};
    use crate::sim::compile as sim_compile;
    use crate::sim::CompiledValue;
    use crate::simdef::{
        ActionDamage, ActionDef, BuffDef, NumOrExpr, ProcDef, ReapplyPolicy, ResourceDef, Rotation,
        Rule, SimDef,
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
                cooldown: NumOrExpr::Num(0.0),
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
        cost.insert("mana".to_string(), NumOrExpr::Num(50.0));
        let mut actions = BTreeMap::new();
        actions.insert(
            "spender".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
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
                cooldown: NumOrExpr::Num(10.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
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
                duration: NumOrExpr::Num(4.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
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
                duration: NumOrExpr::Num(4.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
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
                cooldown: NumOrExpr::Num(0.0),
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
    // C1 (P6 review): an action with `cast_time` 0, `cooldown` 0.0, and no
    // cost at all (permanently payable) is chosen, completes instantly,
    // and is immediately eligible again — nothing in this config ever
    // advances `self.time`, so `attempt_decision`'s instant-chain loop
    // (see its doc comment) would spin forever pre-fix. This config has NO
    // FINITE dps to compute (the rotation legitimately wants to cast
    // `instant_nop` infinitely many times at t=0), so the correct/only
    // sound behavior is failing closed, not returning any number.
    //
    // RED evidence (recorded, not merely asserted): with the
    // `instant_chain`/`INSTANT_CHAIN_LIMIT` guard in `attempt_decision`
    // temporarily reverted to the pre-fix `continue`-only loop, `timeout
    // 30 cargo test -p rtce instant_cast_livelock_fails_closed` TIMES OUT
    // (exit 124, no test output — the process is still spinning inside
    // `attempt_decision` after 30s) rather than the test completing either
    // way; restoring the guard makes it pass in well under a second. See
    // this task's commit message for the literal transcript.
    // ------------------------------------------------------------------
    #[test]
    fn instant_cast_livelock_fails_closed() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "instant_nop".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs: BTreeMap::new(),
            procs: BTreeMap::new(),
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "instant_nop".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let err = run(&plan, &sim_plan, &build, &scenario, Mode::Expected)
            .expect_err("zero cast_time + zero cooldown + free cost must fail closed, not hang");
        assert!(
            err.what.contains("instant_nop"),
            "error must name the offending action, got: {}",
            err.what
        );
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
                cooldown: NumOrExpr::Num(10.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
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
                duration: NumOrExpr::Num(4.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
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
                cooldown: NumOrExpr::Num(0.0),
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

    // ------------------------------------------------------------------
    // A minimal SimDef shared by the P6d proc/MC fixtures below: a bare
    // one-stat plan (`hit = dmg`, no branching), a spammable 1s-cast
    // `filler` action, and one `apply_buff` proc slot the caller fills in
    // with its own trigger/chance/icd — every fixture below only differs
    // in that proc's config, so the CADENCE (one hit per second, hit N
    // completing at t=N) is identical and hand-worked once, here.
    // ------------------------------------------------------------------
    fn minimal_plan() -> Plan {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg"],
                 "pipeline": [ { "name": "hit", "expr": "dmg" } ],
                 "objectives": ["hit"] }"#,
        )
        .unwrap();
        plan::compile(&def).unwrap()
    }

    fn minimal_build() -> BuildState {
        serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap()
    }

    fn filler_simdef(proc: ProcDef) -> SimDef {
        let mut actions = BTreeMap::new();
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "proc_buff".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(0.5),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert("spark".to_string(), proc);
        SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        }
    }

    fn filler_rotation() -> Rotation {
        Rotation {
            rules: vec![Rule {
                action: "filler".into(),
                when: None,
            }],
        }
    }

    // ------------------------------------------------------------------
    // EV accumulator, ICD 0: chance 0.3/hit, 1 hit/s, 10 hits. Hand-worked
    // (acc starts 0, `-=1.0` on every crossing, `>=1.0` fires):
    //   hit1 .3  hit2 .6  hit3 .9  hit4 1.2→FIRE(.2)  hit5 .5  hit6 .8
    //   hit7 1.1→FIRE(.1)  hit8 .4  hit9 .7  hit10 1.0→FIRE(.0)
    // Exactly 3 fires, at hits 4, 7, 10.
    // ------------------------------------------------------------------
    #[test]
    fn ev_accumulator_fractional_chance_fires_at_hand_worked_hit_indices() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();
        let simdef = filler_simdef(ProcDef {
            trigger: Trigger::OnHit,
            chance: "0.3".into(),
            icd: 0.0,
            apply_buff: Some("proc_buff".into()),
            cast_action: None,
        });
        let sim_plan = sim_compile(&plan, &simdef, &filler_rotation()).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["filler"].casts, 10);
        assert_eq!(
            report.proc_counts["spark"], 3,
            "got {:?}",
            report.proc_counts
        );
    }

    // ------------------------------------------------------------------
    // EV accumulator, ICD as a HARD GATE (P6 review/I1 — re-derived from
    // the pre-fix `ev_accumulator_deferred_fire_during_icd_is_hand_worked`,
    // same chance (0.3/hit), cadence (1 hit/s), and icd (4.0), NEW
    // semantics: while `now < icd_ready_at`, a qualifying hit contributes
    // NOTHING to `acc` — no accumulation, no crossing, no deferred fire.
    // Hand-worked:
    //   hit1 t=1: now(1)>=icd_ready(0) → acc=.3
    //   hit2 t=2: acc=.6
    //   hit3 t=3: acc=.9
    //   hit4 t=4: acc=1.2 → FIRE (now=4>=icd_ready=0) → acc=.2, icd_ready=8
    //   hit5 t=5: now(5)<icd_ready(8) → GATED, acc untouched (.2)
    //   hit6 t=6: GATED (.2)
    //   hit7 t=7: GATED (.2)
    //   hit8 t=8: now(8)>=icd_ready(8) → acc=.2+.3=.5
    //   hit9 t=9: acc=.5+.3=.8
    //   hit10 t=10: acc=.8+.3=1.1 → FIRE (now=10>=icd_ready=8) → acc=.1,
    //         icd_ready=14
    // Exactly 2 fires — AT t=4 and t=10 (not t=8, as the pre-fix
    // accumulate-through-ICD-plus-deferral semantics landed the second
    // fire; the count alone (2) doesn't distinguish the two semantics,
    // only the TIMING does — see the `proc_buff` uptime assertion below).
    //
    // Mutation-check (recorded, not just asserted): restoring the old
    // accumulate-through/deferred-fire branch in `roll_procs_ev` while
    // keeping this test's NEW expectations makes `buff_uptime["proc_buff"]`
    // read `0.1` (old semantics' fire lands at t=8, a full window inside
    // `duration=10`) instead of the `0.05` this test now pins — i.e. this
    // test FAILS under the old semantics, proving it still distinguishes
    // them post-re-derivation.
    //
    // `proc_buff`'s duration (0.5s) makes the TIMING observable: the t=4
    // fire's window [4, 4.5) is entirely inside `duration=10` and counts
    // in full (0.5s); the t=10 fire's window [10, 10.5) is entirely AFTER
    // the sim ends (fires exactly AT `duration`), so `finalize` credits it
    // ZERO active seconds. One full 0.5s window → uptime 0.5/10 = 0.05.
    // ------------------------------------------------------------------
    #[test]
    fn ev_accumulator_icd_gate_discards_hits_during_icd_is_hand_worked() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();
        let simdef = filler_simdef(ProcDef {
            trigger: Trigger::OnHit,
            chance: "0.3".into(),
            icd: 4.0,
            apply_buff: Some("proc_buff".into()),
            cast_action: None,
        });
        let sim_plan = sim_compile(&plan, &simdef, &filler_rotation()).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["filler"].casts, 10);
        assert_eq!(
            report.proc_counts["spark"], 2,
            "got {:?} — hits at t=4 and t=10 fire; hits gated by ICD (t=5..7) \
             must contribute nothing",
            report.proc_counts
        );
        assert!(
            close(report.buff_uptime["proc_buff"], 0.05),
            "got {} — the second fire must land at hit10 (t=10, == duration, \
             truncating its 0.5s window to zero), not hit8 (t=8, which would \
             read 0.1 — that's the pre-fix accumulate-through-ICD behavior)",
            report.buff_uptime["proc_buff"]
        );
    }

    // ------------------------------------------------------------------
    // I1 regression: EV vs MC agreement in the ICD-BOUND regime (chance
    // 0.3/hit, 1 hit/s cadence, icd 5.0, 200s) — the exact case the
    // pre-publish review flagged: the pre-fix "accumulate through ICD,
    // defer one fire" semantics measured EV=40 vs MC mean≈27 here (+48%),
    // because it kept banking qualifying-hit mass through the whole ICD
    // window instead of discarding it the way MC's hard gate does.
    //
    // Hand-worked EV count under the NEW (hard-gate) semantics: after a
    // fire at `t_fire`, `icd_ready_at = t_fire + 5`, so hits at
    // `t_fire+1..+4` are gated out (4 gated hits — the gate reopens
    // exactly ON the hit at `t_fire+5`, where `now < icd_ready_at` is
    // false), and accumulation resumes from the LEFTOVER `acc` the
    // previous fire left behind (`acc -= 1.0`, never reset to `0.0`).
    // Tracking that leftover `L` across fires (`k` = hits needed past the
    // gate to reach `1.0` = `ceil((1-L)/0.3)`, next leftover
    // `L' = L + 0.3k - 1.0`, gap from one fire to the next = `4 + k`):
    //   L=0.0 (start)  k=4  gap=4  → fire1 @ t=4,  L'=0.2
    //   L=0.2          k=3  gap=7  → fire2 @ t=11, L'=0.1
    //   L=0.1          k=3  gap=7  → fire3 @ t=18, L'=0.0
    //   L=0.0          k=4  gap=8  → fire4 @ t=26, L'=0.2   (cycle repeats:
    //     L walks 0.0→0.2→0.1→0.0 every 3 fires / 22s, average interval
    //     22/3 ≈ 7.333s/fire, i.e. ≈0.1364 fires/s)
    // 200s × 3/22 ≈ 27.27 expected fires; walking the exact recurrence out
    // to `duration=200` (fire1 @ t=4 through the 27th fire @ t=194, with
    // no 28th fire landing before t=200 — the next would be at t=201 or
    // t=202) lands EXACTLY 27, matching the reviewer's independently
    // measured MC mean (≈27) almost exactly — this is what "the two modes
    // now agree, including where the old ones didn't" looks like
    // numerically.
    // ------------------------------------------------------------------
    #[test]
    fn ev_procs_match_mc_in_icd_bound_regime_regression() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 200 } ] }"#).unwrap();
        let simdef = filler_simdef(ProcDef {
            trigger: Trigger::OnHit,
            chance: "0.3".into(),
            icd: 5.0,
            apply_buff: Some("proc_buff".into()),
            cast_action: None,
        });
        let sim_plan = sim_compile(&plan, &simdef, &filler_rotation()).unwrap();

        let ev = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        assert_eq!(ev.actions["filler"].casts, 200);
        assert_eq!(
            ev.proc_counts["spark"], 27,
            "got {:?} — hand-worked recurrence above says 27 (pre-fix semantics \
             gave 40, +48% vs MC's ≈27)",
            ev.proc_counts
        );

        let mc = run(
            &plan,
            &sim_plan,
            &build,
            &scenario,
            Mode::MonteCarlo {
                iterations: 2_000,
                seed: 20260722,
            },
        )
        .unwrap();
        let mc_count = mc.proc_counts["spark"] as f64;
        let rel_err = (mc_count - 27.0).abs() / 27.0;
        assert!(
            rel_err < 0.15,
            "mc mean proc count {mc_count} vs ev count 27, relative error {rel_err} \
             — pre-fix this regime measured EV=40 vs MC≈27 (+48%)"
        );
    }

    // ------------------------------------------------------------------
    // EV `on_crit` weighting: `toy_plan`'s "crit" event at crit_chance=25
    // (0.25 probability). A crit-triggered proc at chance 1.0/icd 0.0
    // therefore accumulates 1.0×0.25 = 0.25 per hit (the EV-consistent
    // choice this task pins — see `roll_procs_ev`'s doc comment).
    // 10 hits (same 1s cadence as the keystone test): acc = i×0.25,
    // crossing exactly at hit 4 (1.0) and hit 8 (2.0) — both exact in
    // binary (0.25 = 2⁻²) so there is no floating-point ambiguity about
    // which hit crosses. Exactly 2 fires.
    // ------------------------------------------------------------------
    #[test]
    fn ev_on_crit_weights_by_crit_probability() {
        let plan = toy_plan();
        let build = toy_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "spam".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "proc_buff".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(0.5),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "crit_proc".to_string(),
            ProcDef {
                trigger: Trigger::OnCrit,
                chance: "1".into(),
                icd: 0.0,
                apply_buff: Some("proc_buff".into()),
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
            rules: vec![Rule {
                action: "spam".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["spam"].casts, 10);
        assert_eq!(
            report.proc_counts["crit_proc"], 2,
            "got {:?}",
            report.proc_counts
        );
    }

    // ------------------------------------------------------------------
    // `ProcEffect::CastAction`: an `on_cast` proc (chance 1, icd 0) fires
    // a free "nuke" cast (instant, no cost/cooldown) on every one of
    // "trigger"'s 5 casts (1s cast time, 5s scenario → completions at
    // t=1..5). `free_cast`'s documented scope: gains + damage only, no
    // cost/cooldown paid, no further proc rolls. nuke's damage is the
    // bare `dmg=100` stat (minimal_plan's `hit = dmg`), hits_per_use
    // defaulting to 1 — so nuke's total damage = 5 × 100 = 500.
    // ------------------------------------------------------------------
    #[test]
    fn proc_effect_cast_action_fires_a_free_instant_cast() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 5 } ] }"#).unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "trigger".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        actions.insert(
            "nuke".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "free_nuke".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd: 0.0,
                apply_buff: None,
                cast_action: Some("nuke".into()),
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs: BTreeMap::new(),
            procs,
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "trigger".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["trigger"].casts, 5);
        assert_eq!(report.actions["nuke"].casts, 5);
        assert!(
            close(report.actions["nuke"].damage, 500.0),
            "got {}",
            report.actions["nuke"].damage
        );
        assert_eq!(report.proc_counts["free_nuke"], 5);
    }

    // ------------------------------------------------------------------
    // Monte Carlo determinism: the SAME seed, run twice, must produce a
    // BYTE-IDENTICAL serialized `SimReport` — the whole reproducibility
    // contract `Mode::MonteCarlo` exists to provide.
    // ------------------------------------------------------------------
    #[test]
    fn monte_carlo_same_seed_twice_is_byte_identical() {
        let plan = toy_plan();
        let build = toy_build();
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 10,
                   "uptimes": { "enraged": 0.5 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();
        let mut actions = BTreeMap::new();
        actions.insert(
            "spam".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
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
        let mode = Mode::MonteCarlo {
            iterations: 50,
            seed: 20260722,
        };

        let a = run(&plan, &sim_plan, &build, &scenario, mode).unwrap();
        let b = run(&plan, &sim_plan, &build, &scenario, mode).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    // ------------------------------------------------------------------
    // EV-vs-MC convergence, crit-only (no procs): the same keystone
    // fixture as `keystone_matches_level_1_exactly` (dps 282.15), run
    // under `Mode::MonteCarlo` with N=10_000 at a fixed seed. Statistical
    // assertion (not exact): relative error under 2%.
    // ------------------------------------------------------------------
    #[test]
    fn monte_carlo_converges_to_ev_on_crit_only_case() {
        let plan = toy_plan();
        let build = toy_build();
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "arena", "weight": 10,
                   "uptimes": { "enraged": 0.5 },
                   "stats": { "enemy_dr": 20.0 } } ] }"#,
        )
        .unwrap();
        let mut actions = BTreeMap::new();
        actions.insert(
            "spam".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
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

        let ev = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        assert!(close(ev.total.dps, 282.15), "got {}", ev.total.dps);

        let mc = run(
            &plan,
            &sim_plan,
            &build,
            &scenario,
            Mode::MonteCarlo {
                iterations: 10_000,
                seed: 42,
            },
        )
        .unwrap();
        let dist = mc.distribution.expect("MC mode always sets distribution");
        let rel_err = (dist.mean - ev.total.dps).abs() / ev.total.dps;
        assert!(
            rel_err < 0.02,
            "mc mean {} vs ev dps {}, relative error {rel_err}",
            dist.mean,
            ev.total.dps
        );
    }

    // ------------------------------------------------------------------
    // EV-vs-MC convergence, procs: the fractional-chance accumulator
    // fixture (chance 0.3/hit, icd 0, EV fire count 3) run under MC at
    // N=2_000, fixed seed. Loose statistical bound (15%) — MC's proc
    // count is a Binomial(10, 0.3)-per-iteration draw pooled/rounded
    // across N iterations, not expected to match the EV accumulator's
    // exact integer count, only to be IN THE NEIGHBORHOOD of it.
    // ------------------------------------------------------------------
    #[test]
    fn monte_carlo_proc_count_is_near_ev_accumulator_count() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();
        let simdef = filler_simdef(ProcDef {
            trigger: Trigger::OnHit,
            chance: "0.3".into(),
            icd: 0.0,
            apply_buff: Some("proc_buff".into()),
            cast_action: None,
        });
        let sim_plan = sim_compile(&plan, &simdef, &filler_rotation()).unwrap();

        let ev = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        let ev_count = ev.proc_counts["spark"];
        assert_eq!(ev_count, 3);

        let mc = run(
            &plan,
            &sim_plan,
            &build,
            &scenario,
            Mode::MonteCarlo {
                iterations: 2_000,
                seed: 7,
            },
        )
        .unwrap();
        let mc_count = mc.proc_counts["spark"] as f64;
        let rel_err = (mc_count - ev_count as f64).abs() / ev_count as f64;
        assert!(
            rel_err < 0.15,
            "mc mean proc count {mc_count} vs ev count {ev_count}, relative error {rel_err}"
        );
    }

    // ==================================================================
    // P7b — expression-valued sim fields (`NumOrExpr`)
    //
    // Five fields accept a literal OR an expression: `BuffDef::duration`,
    // `ActionDef::cooldown`, the `cost`/`gain` amounts, and the
    // `ActionDamage::stats` values. What these tests pin is not "an
    // expression can be written" but WHEN each one is evaluated (the
    // instants table on `simdef::NumOrExpr`) and that a bad value fails
    // closed AT that instant.
    // ==================================================================

    /// A minimal `Plan` for the P7b fixtures: `hit = dmg`, plus a
    /// `bonus_dur` stat for the duration cases and a `hidden_stage`
    /// pipeline stage that is deliberately NOT an objective (used to prove
    /// stages stay invisible to expression-valued fields too).
    /// `toy_plan`'s crit machinery is irrelevant here — the question under
    /// test is WHEN a field's expression is evaluated, not what the damage
    /// formula does with the result.
    fn flat_plan() -> Plan {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg", "bonus_dur"],
                 "pipeline": [ { "name": "hit", "expr": "dmg" },
                               { "name": "hidden_stage", "expr": "dmg * 2" } ],
                 "objectives": ["hit"] }"#,
        )
        .unwrap();
        plan::compile(&def).unwrap()
    }

    fn flat_build() -> BuildState {
        serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "bonus_dur": 2.0 } }"#).unwrap()
    }

    // ------------------------------------------------------------------
    // BACKWARD COMPATIBILITY (the hard requirement — rtce 0.2.0 is
    // published). `simdef::P6_SPEC_SIMDEF_JSON` is the P6 design spec's
    // "Config surface" example verbatim — the SAME text `simdef.rs`'s
    // parse test uses, shared rather than copied — in which every
    // now-expression-valued position is a plain JSON NUMBER. Untagged
    // serde must keep reading them as `NumOrExpr::Num`, and the executor
    // must keep treating them exactly as the old `f64` fields.
    // ------------------------------------------------------------------

    #[test]
    fn p6_numeric_simdef_json_parses_and_compiles_to_constants() {
        let def: SimDef = serde_json::from_str(crate::simdef::P6_SPEC_SIMDEF_JSON).unwrap();
        assert_eq!(def.actions["fireball"].cooldown, NumOrExpr::Num(0.0));
        assert_eq!(def.actions["fireball"].cost["mana"], NumOrExpr::Num(40.0));
        assert_eq!(def.buffs["burning"].duration, NumOrExpr::Num(6.0));

        // …and compiles against a plan carrying the spec's names, with
        // every literal pre-baked into a `Const` (no Program at all).
        let plan_def: GameDef = serde_json::from_str(
            r#"{ "stats": ["max_mana", "mana_regen", "base_aps", "lucky_hit_chance",
                           "coeff_pct", "weapon"],
                 "conditions": ["vulnerable"],
                 "buckets": { "indep": { "fold": "product" } },
                 "pipeline": [ { "name": "hit_after_dr", "expr": "weapon * indep" },
                               { "name": "dot_dps", "expr": "weapon * 0.1" } ],
                 "objectives": ["hit_after_dr", "dot_dps"] }"#,
        )
        .unwrap();
        let plan = plan::compile(&plan_def).unwrap();
        let sp = sim_compile(&plan, &def, &Rotation::default()).unwrap();
        assert!(matches!(sp.actions[0].cooldown, CompiledValue::Const(v) if v == 0.0));
        assert!(matches!(sp.actions[0].cost[0].1, CompiledValue::Const(v) if v == 40.0));
        for b in &sp.buffs {
            assert!(matches!(b.duration, CompiledValue::Const(_)));
        }
    }

    // The P6c starvation pin, rebuilt from JSON with NUMERIC literals in
    // `cost` — byte-for-byte the same cadence
    // `resource_starvation_cadence_is_hand_worked` derives (casts at
    // t=0,1,5,10,15; 5 casts / 20s; 15s starved). This is the BEHAVIORAL
    // half of the compatibility contract: `NumOrExpr::Num` must reach the
    // executor as the identical number the `f64` field used to.
    #[test]
    fn p6c_starvation_pin_reproduces_from_numeric_json() {
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
        let simdef: SimDef = serde_json::from_str(
            r#"{ "resources": { "mana": { "max": "100", "regen_per_sec": "10" } },
                 "actions": { "spender": { "cast_time": "1", "cooldown": 0.0,
                                           "cost": { "mana": 50.0 },
                                           "damage": { "stats": {} } } },
                 "damage_objective": "hit" }"#,
        )
        .unwrap();
        let rotation: Rotation =
            serde_json::from_str(r#"{ "rules": [ { "action": "spender" } ] }"#).unwrap();
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["spender"].casts, 5);
        assert!(close(report.total.total_damage, 500.0));
        assert!(close(report.total.dps, 25.0));
        assert!(close(report.resources["mana"].time_starved, 15.0));
    }

    // ------------------------------------------------------------------
    // EXPR DURATION — evaluated at APPLICATION.
    //
    // Fixture: `filler` (1s cast) completes every second t=1..20. An
    // on_cast proc (chance 1, icd 10) applies `window`, whose duration is
    // `"2 + bonus_dur"`. On_cast procs roll at cast COMPLETE, so:
    //   t=1   icd clear → fire → apply, duration = 2 + 2 = 4 → expires t=5
    //   t=2..10  ICD-gated (icd_ready_at = 11), no roll banked
    //   t=11  fire → apply, duration 4 → expires t=15
    //   t=21  would be the next fire — past the 20s duration
    // uptime = (4 + 4) / 20 = 0.4
    // ------------------------------------------------------------------
    fn expr_duration_fixture(duration: NumOrExpr, icd: f64) -> (SimDef, Rotation) {
        let mut actions = BTreeMap::new();
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "window".to_string(),
            BuffDef {
                duration,
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "pulse".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd,
                apply_buff: Some("window".into()),
                cast_action: None,
            },
        );
        (
            SimDef {
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            },
            Rotation {
                rules: vec![Rule {
                    action: "filler".into(),
                    when: None,
                }],
            },
        )
    }

    #[test]
    fn expr_duration_reads_the_stat_at_application() {
        let plan = flat_plan();
        let build = flat_build(); // bonus_dur = 2
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) =
            expr_duration_fixture(NumOrExpr::Expr("2 + bonus_dur".into()), 10.0);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        // Windows [1,5) and [11,15) — see the hand-worked trace above.
        assert!(
            close(report.buff_uptime["window"], 0.4),
            "got {}",
            report.buff_uptime["window"]
        );
        // …and the literal 4.0 gives the byte-identical answer.
        let (lit_def, lit_rot) = expr_duration_fixture(NumOrExpr::Num(4.0), 10.0);
        let lit_plan = sim_compile(&plan, &lit_def, &lit_rot).unwrap();
        let lit = run(&plan, &lit_plan, &build, &scenario, Mode::Expected).unwrap();
        assert_eq!(
            report.buff_uptime["window"], lit.buff_uptime["window"],
            "expr `2 + bonus_dur` must behave EXACTLY as the literal 4.0"
        );
    }

    // ------------------------------------------------------------------
    // The reserved sim symbol `duration` is the SCENARIO's total length,
    // NOT the buff field of the same name (the one documented ambiguity in
    // `simdef::NumOrExpr`). A 20s fight with duration `"duration / 10"`
    // therefore gives 2s windows, not a self-reference:
    //   t=1  apply → expires t=3   |   t=11 apply → expires t=13
    // uptime = (2 + 2) / 20 = 0.2
    // ------------------------------------------------------------------
    #[test]
    fn buff_duration_expr_naming_duration_reads_the_fight_length() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) =
            expr_duration_fixture(NumOrExpr::Expr("duration / 10".into()), 10.0);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        assert!(
            close(report.buff_uptime["window"], 0.2),
            "got {}",
            report.buff_uptime["window"]
        );
    }

    // ------------------------------------------------------------------
    // EXPR DURATION — SNAPSHOTTED at application (the semantics Task 5's
    // snapshot DoTs rest on). Same fixture, two phases:
    //   phase 1: t ∈ [0,4),  bonus_dur = 2 (from the build)
    //   phase 2: t ∈ [4,20), bonus_dur = 6 (phase stat override)
    // Hand-worked:
    //   t=1   apply — duration snapshots as 2 + 2 = 4 → expires t=5
    //   t=4   PHASE BOUNDARY raises bonus_dur to 6. The window already in
    //         flight is UNAFFECTED: it still expires at t=5, not t=9.
    //   t=5   expire. Active span [1,5) = 4s.
    //   t=11  ICD clear → apply — duration re-evaluates NOW: 2 + 6 = 8
    //         → expires t=19. Active span [11,19) = 8s.
    //   t=19  expire. (Next fire would be t=21, past duration.)
    // uptime = (4 + 8) / 20 = 12/20 = 0.6
    // A duration that re-evaluated live off the current stat would give
    // the first window 8s too → 16/20 = 0.8; one frozen at compile/start
    // time would give both windows 4s → 8/20 = 0.4. Only the snapshot
    // reading is 0.6.
    // ------------------------------------------------------------------
    #[test]
    fn expr_duration_snapshots_at_application_across_a_phase_boundary() {
        let plan = flat_plan();
        let build = flat_build(); // bonus_dur = 2
        let scenario: Scenario = serde_json::from_str(
            r#"{ "phases": [ { "name": "early", "weight": 4 },
                             { "name": "late",  "weight": 16,
                               "stats": { "bonus_dur": 6.0 } } ] }"#,
        )
        .unwrap();
        let (simdef, rotation) =
            expr_duration_fixture(NumOrExpr::Expr("2 + bonus_dur".into()), 10.0);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert!(
            close(report.buff_uptime["window"], 0.6),
            "got {} (0.8 = re-evaluated live, 0.4 = frozen at start)",
            report.buff_uptime["window"]
        );
    }

    // ------------------------------------------------------------------
    // EXPR COST — evaluated at cast start / at each affordability check.
    // `"20 + 10"` must behave EXACTLY as the literal 30. Hand-worked
    // cadence (mana max 100, regen 10/s, starting full, 1s cast, cost 30):
    //   t=0  100→70, completes t=1
    //   t=1   80→50, completes t=2
    //   t=2   60→30, completes t=3
    //   t=3   40→10, completes t=4
    //   t=4  mana 20 < 30 — starved from t=4, wake at 4 + 10/10 = 5
    //   t=5   30→0,  completes t=6
    //   t=6  mana 10 < 30 — starved from t=6, wake at 6 + 20/10 = 8
    //   t=8   30→0,  completes t=9
    //   t=9  starved from t=9,  wake t=11
    //   t=11  30→0,  completes t=12
    //   t=12 starved from t=12, wake t=14
    //   t=14  30→0,  completes t=15
    //   t=15 starved from t=15, wake t=17
    //   t=17  30→0,  completes t=18
    //   t=18 starved from t=18, wake t=20 = duration (no further cast)
    // 9 casts × 100 dmg = 900 over 20s = 45 dps.
    // starved = (5-4)+(8-6)+(11-9)+(14-12)+(17-15)+(20-18) = 1+2+2+2+2+2 = 11s
    // ------------------------------------------------------------------
    fn expr_cost_fixture(cost: NumOrExpr) -> (SimDef, Rotation) {
        let mut resources = BTreeMap::new();
        resources.insert(
            "mana".to_string(),
            ResourceDef {
                max: "100".into(),
                regen_per_sec: "10".into(),
            },
        );
        let mut cost_map = BTreeMap::new();
        cost_map.insert("mana".to_string(), cost);
        let mut actions = BTreeMap::new();
        actions.insert(
            "spender".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: cost_map,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        (
            SimDef {
                resources,
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                rules: vec![Rule {
                    action: "spender".into(),
                    when: None,
                }],
            },
        )
    }

    #[test]
    fn expr_cost_behaves_exactly_as_the_literal() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

        let (expr_def, rot) = expr_cost_fixture(NumOrExpr::Expr("20 + 10".into()));
        let expr_plan = sim_compile(&plan, &expr_def, &rot).unwrap();
        let expr_report = run(&plan, &expr_plan, &build, &scenario, Mode::Expected).unwrap();

        let (lit_def, lit_rot) = expr_cost_fixture(NumOrExpr::Num(30.0));
        let lit_plan = sim_compile(&plan, &lit_def, &lit_rot).unwrap();
        let lit_report = run(&plan, &lit_plan, &build, &scenario, Mode::Expected).unwrap();

        // The hand-worked cadence above.
        assert_eq!(expr_report.actions["spender"].casts, 9);
        assert!(
            close(expr_report.total.total_damage, 900.0),
            "got {}",
            expr_report.total.total_damage
        );
        assert!(close(expr_report.total.dps, 45.0));
        assert!(
            close(expr_report.resources["mana"].time_starved, 11.0),
            "got {}",
            expr_report.resources["mana"].time_starved
        );

        // …and identical to the literal, field for field.
        assert_eq!(
            expr_report.actions["spender"].casts,
            lit_report.actions["spender"].casts
        );
        assert_eq!(
            expr_report.total.total_damage,
            lit_report.total.total_damage
        );
        assert_eq!(
            expr_report.resources["mana"].time_starved,
            lit_report.resources["mana"].time_starved
        );
    }

    // ------------------------------------------------------------------
    // EXPR COOLDOWN — evaluated at cast start. `"5 + 5"` == literal 10.
    // Hand-worked: `nova` is instant (cast_time 0) with a 10s cooldown and
    // is the only rule, so it casts at t=0, the decision loop schedules a
    // wake at its cooldown-ready time, and it recasts at t=10. The wake at
    // t=20 lands exactly ON `duration`, where the run loop stops before
    // deciding again (see module docs on the `End` boundary):
    //   casts at t=0 and t=10 → 2 × 100 = 200 damage over 20s = 10 dps.
    // ------------------------------------------------------------------
    fn expr_cooldown_fixture(cooldown: NumOrExpr) -> (SimDef, Rotation) {
        let mut actions = BTreeMap::new();
        actions.insert(
            "nova".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        (
            SimDef {
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                rules: vec![Rule {
                    action: "nova".into(),
                    when: None,
                }],
            },
        )
    }

    #[test]
    fn expr_cooldown_behaves_exactly_as_the_literal() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

        let (expr_def, rot) = expr_cooldown_fixture(NumOrExpr::Expr("5 + 5".into()));
        let expr_plan = sim_compile(&plan, &expr_def, &rot).unwrap();
        let expr_report = run(&plan, &expr_plan, &build, &scenario, Mode::Expected).unwrap();

        let (lit_def, lit_rot) = expr_cooldown_fixture(NumOrExpr::Num(10.0));
        let lit_plan = sim_compile(&plan, &lit_def, &lit_rot).unwrap();
        let lit_report = run(&plan, &lit_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(expr_report.actions["nova"].casts, 2);
        assert!(
            close(expr_report.total.total_damage, 200.0),
            "got {}",
            expr_report.total.total_damage
        );
        assert!(close(expr_report.total.dps, 10.0));
        assert_eq!(
            expr_report.actions["nova"].casts,
            lit_report.actions["nova"].casts
        );
        assert_eq!(
            expr_report.total.total_damage,
            lit_report.total.total_damage
        );
    }

    // ------------------------------------------------------------------
    // EXPR GAIN — evaluated at CAST COMPLETE. `"50 + 50"` == literal 100.
    // Hand-worked (mana max 100, NO regen, starts full; `spender` costs
    // 100 and deals 100 damage, `generator` gains 100 on completion):
    //   t=0  spender (100→0), completes t=1
    //   t=1  mana 0 — spender unaffordable and unreachable (no regen), so
    //        the priority list falls through to generator, completing t=2
    //        and crediting +100 AT COMPLETION
    //   t=2  spender again … alternating forever
    // spends at t=0,2,4,…,18 → 10 casts × 100 = 1000 over 20s = 50 dps.
    // ------------------------------------------------------------------
    fn expr_gain_fixture(gain: NumOrExpr) -> (SimDef, Rotation) {
        let mut resources = BTreeMap::new();
        resources.insert(
            "mana".to_string(),
            ResourceDef {
                max: "100".into(),
                regen_per_sec: "0".into(),
            },
        );
        let mut cost_map = BTreeMap::new();
        cost_map.insert("mana".to_string(), NumOrExpr::Num(100.0));
        let mut gain_map = BTreeMap::new();
        gain_map.insert("mana".to_string(), gain);
        let mut actions = BTreeMap::new();
        actions.insert(
            "spender".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: cost_map,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        actions.insert(
            "generator".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: gain_map,
                damage: None,
            },
        );
        (
            SimDef {
                resources,
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                rules: vec![
                    Rule {
                        action: "spender".into(),
                        when: None,
                    },
                    Rule {
                        action: "generator".into(),
                        when: None,
                    },
                ],
            },
        )
    }

    #[test]
    fn expr_gain_behaves_exactly_as_the_literal() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

        let (expr_def, rot) = expr_gain_fixture(NumOrExpr::Expr("50 + 50".into()));
        let expr_plan = sim_compile(&plan, &expr_def, &rot).unwrap();
        let expr_report = run(&plan, &expr_plan, &build, &scenario, Mode::Expected).unwrap();

        let (lit_def, lit_rot) = expr_gain_fixture(NumOrExpr::Num(100.0));
        let lit_plan = sim_compile(&plan, &lit_def, &lit_rot).unwrap();
        let lit_report = run(&plan, &lit_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(expr_report.actions["spender"].casts, 10);
        assert_eq!(expr_report.actions["generator"].casts, 10);
        assert!(
            close(expr_report.total.total_damage, 1000.0),
            "got {}",
            expr_report.total.total_damage
        );
        assert!(close(expr_report.total.dps, 50.0));
        assert_eq!(
            expr_report.total.total_damage,
            lit_report.total.total_damage
        );
    }

    // ------------------------------------------------------------------
    // EXPR damage.stats — evaluated at CAST COMPLETE (not at cast start).
    // `beam` is a 1s cast whose `dmg` override is `"time * 10"` and whose
    // `hits_per_use` is the expression `"2"`. Over a 5s fight the casts
    // COMPLETE at t=1,2,3,4,5 (the 5th starts at t=4 and completes exactly
    // at duration — see module docs on the `End` boundary), so:
    //   dmg per hit = 10, 20, 30, 40, 50   (× hits_per_use 2)
    //   total = 2 × (10+20+30+40+50) = 2 × 150 = 300 over 5s = 60 dps
    // Evaluated at cast START instead, `time` would read 0,1,2,3,4 and the
    // total would be 2 × 100 = 200 — the instant is what this pins.
    // ------------------------------------------------------------------
    /// A one-action fixture whose only variable is `damage.stats` — the
    /// counterpart of the `expr_{duration,cost,cooldown,gain}_fixture`
    /// helpers above.
    fn beam_fixture(stats: BTreeMap<String, NumOrExpr>) -> (SimDef, Rotation) {
        let mut actions = BTreeMap::new();
        actions.insert(
            "beam".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage { stats }),
            },
        );
        (
            SimDef {
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                rules: vec![Rule {
                    action: "beam".into(),
                    when: None,
                }],
            },
        )
    }

    #[test]
    fn expr_damage_stats_are_evaluated_at_cast_complete() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 5 } ] }"#).unwrap();

        let mut stats = BTreeMap::new();
        stats.insert("dmg".to_string(), NumOrExpr::Expr("time * 10".into()));
        stats.insert("hits_per_use".to_string(), NumOrExpr::Expr("2".into()));
        let (simdef, rotation) = beam_fixture(stats);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["beam"].casts, 5);
        assert!(
            close(report.total.total_damage, 300.0),
            "got {} (200 = evaluated at cast START)",
            report.total.total_damage
        );
        assert!(close(report.total.dps, 60.0), "got {}", report.total.dps);
    }

    // ------------------------------------------------------------------
    // FAIL-CLOSED — at the evaluation instant, never a guessed default.
    // ------------------------------------------------------------------

    #[test]
    fn negative_expr_duration_fails_closed_at_application() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) = expr_duration_fixture(NumOrExpr::Expr("0 - 5".into()), 10.0);
        // Compiling is FINE — `0 - 5` is a perfectly good expression. The
        // error belongs to the application instant (t=1, the first proc
        // fire), and must name the buff.
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let e = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap_err();
        assert!(
            e.what
                .contains("buff `window` duration at application (t=1)"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("-5"), "got: {}", e.what);
    }

    #[test]
    fn non_finite_expr_duration_fails_closed_at_application() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) = expr_duration_fixture(NumOrExpr::Expr("1 / 0".into()), 10.0);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let e = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap_err();
        assert!(
            e.what
                .contains("buff `window` duration at application (t=1)"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("inf"), "got: {}", e.what);
    }

    #[test]
    fn expr_cost_referencing_an_unknown_symbol_is_a_compile_error() {
        let plan = flat_plan();
        let (simdef, rotation) = expr_cost_fixture(NumOrExpr::Expr("mystery_stat + 1".into()));
        let e = sim_compile(&plan, &simdef, &rotation).unwrap_err();
        assert!(e.what.contains("mystery_stat"), "got: {}", e.what);
        assert!(e.what.contains("spender"), "got: {}", e.what);
        // Positioned, like every other expression error in this engine.
        assert!(e.what.contains("at byte"), "got: {}", e.what);
    }

    #[test]
    fn expr_duration_referencing_a_pipeline_stage_is_a_compile_error() {
        // Stages/buckets stay invisible to the sim symbol space — an
        // expression-valued field is no exception.
        let plan = flat_plan();
        let (simdef, rotation) =
            expr_duration_fixture(NumOrExpr::Expr("hidden_stage".into()), 10.0);
        let e = sim_compile(&plan, &simdef, &rotation).unwrap_err();
        assert!(e.what.contains("hidden_stage"), "got: {}", e.what);
        assert!(e.what.contains("window"), "got: {}", e.what);
    }

    #[test]
    fn negative_expr_cooldown_fails_closed_at_cast_start() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) = expr_cooldown_fixture(NumOrExpr::Expr("0 - 1".into()));
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let e = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap_err();
        assert!(
            e.what.contains("action `nova` cooldown at cast start"),
            "got: {}",
            e.what
        );
        assert!(
            e.what.contains("must be finite and >= 0"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn negative_expr_cost_fails_closed_at_the_decision_instant() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) = expr_cost_fixture(NumOrExpr::Expr("0 - 5".into()));
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let e = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap_err();
        assert!(
            e.what
                .contains("action `spender` cost `mana` at a rotation decision"),
            "got: {}",
            e.what
        );
        assert!(
            e.what.contains("must be finite and >= 0"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn negative_expr_gain_fails_closed_at_cast_complete() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) = expr_gain_fixture(NumOrExpr::Expr("0 - 5".into()));
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let e = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap_err();
        assert!(
            e.what
                .contains("action `generator` gain `mana` at cast complete"),
            "got: {}",
            e.what
        );
        assert!(
            e.what.contains("must be finite and >= 0"),
            "got: {}",
            e.what
        );
    }

    #[test]
    fn non_finite_expr_damage_stat_fails_closed_at_cast_complete() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 5 } ] }"#).unwrap();

        let mut stats = BTreeMap::new();
        // 0/0 = NaN — a stat may legitimately be NEGATIVE, so finiteness
        // is the only check here (see `simdef::NumOrExpr`).
        stats.insert("dmg".to_string(), NumOrExpr::Expr("0 / 0".into()));
        let (simdef, rotation) = beam_fixture(stats);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let e = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap_err();
        assert!(
            e.what
                .contains("action `beam` damage.stats `dmg` at cast complete"),
            "got: {}",
            e.what
        );
        assert!(e.what.contains("NaN"), "got: {}", e.what);
    }

    // A NEGATIVE damage stat is legal (only finiteness is enforced) —
    // `dmg` = -100 over a 5s fight yields 5 casts × -100 = -500. Pinned so
    // the finite-only rule can't silently tighten into a >= 0 check.
    #[test]
    fn negative_expr_damage_stat_is_allowed() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 5 } ] }"#).unwrap();

        let mut stats = BTreeMap::new();
        stats.insert("dmg".to_string(), NumOrExpr::Expr("0 - 100".into()));
        let (simdef, rotation) = beam_fixture(stats);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        assert_eq!(report.actions["beam"].casts, 5);
        assert!(
            close(report.total.total_damage, -500.0),
            "got {}",
            report.total.total_damage
        );
    }

    // ==================================================================
    // P7b review — the two DELIBERATE behavior changes that landed with
    // the expression-valued fields. Both were previously held in place by
    // nothing (reverting either left the suite 110/110 green), which in
    // this file is exactly how a 48% EV/MC divergence got in once before.
    // ==================================================================

    // ------------------------------------------------------------------
    // (1) The slot array's time-varying tail is refreshed PER PROC inside
    // a trigger batch, not once for the batch — so a proc's `chance` sees
    // what an EARLIER proc in the same batch already did. Before the fix
    // the stat/condition PREFIX refolded on a buff application but the
    // tail did not, so a `chance` could read pre-batch sim state while a
    // CONDITION driven by the same effect already read its new value — an
    // inconsistency, not a designed snapshot.
    //
    // The discriminating effect is one that changes ONLY the tail:
    // `a_cast` free-casts `ping`, an action with no gain and no damage, so
    // nothing on that path refreshes the slots on its own (a buff
    // application would, via `apply_buff`'s own refresh, and would hide
    // the difference). All it moves is `casts.ping` — which is exactly
    // what `b_gated`'s chance reads.
    //
    // One 1s cast in a 1s fight, so there is exactly ONE trigger batch and
    // no "it fires on the next cast instead" escape hatch. Procs roll in
    // name order:
    //   with the per-proc refresh: casts.ping = 1 → b_gated fires once
    //   batch-start snapshot:      casts.ping = 0 → b_gated never fires
    // ------------------------------------------------------------------
    #[test]
    fn a_procs_effect_is_visible_to_a_later_proc_in_the_same_batch() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 1 } ] }"#).unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        // Proc-cast only (never in the rotation), and deliberately inert:
        // no gain, no damage, so nothing on the free-cast path refreshes
        // the slot tail as a side effect.
        actions.insert(
            "ping".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "y".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(1.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "a_cast".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd: 0.0,
                apply_buff: None,
                cast_action: Some("ping".into()),
            },
        );
        procs.insert(
            "b_gated".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                // Reads sim state `a_cast` moves in this same batch.
                chance: "casts.ping".into(),
                icd: 0.0,
                apply_buff: Some("y".into()),
                cast_action: None,
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "filler".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(
            report.actions["filler"].casts, 1,
            "exactly one trigger batch"
        );
        assert_eq!(report.proc_counts["a_cast"], 1);
        assert_eq!(report.actions["ping"].casts, 1);
        assert_eq!(
            report.proc_counts["b_gated"], 1,
            "0 = `chance` read a batch-start snapshot in which casts.ping was still 0"
        );
    }

    // ------------------------------------------------------------------
    // (2) EV mode's `on_crit` weight is the probability THIS hit crit,
    // measured with the rest of the cast BEFORE the cast's own procs run.
    // Previously the crit query was deferred to its point of use, i.e.
    // AFTER `on_cast`/`on_hit` procs had already fired — so a proc
    // triggered by a hit could retroactively raise that hit's crit
    // weight, while its DAMAGE had already been computed off the old
    // build. One hit, two `Plan` queries, two different worlds.
    //
    // Fixture: crit chance is `(crit_chance + empowered * 50) / 100` with
    // `crit_chance` = 50, so it is 0.5 while `empowered` is 0 and 1.0
    // once the buff drives it. `focus_proc` (on_hit, chance 1, icd 100)
    // applies that buff on the very first hit; `crit_proc` (on_crit,
    // chance 1) accumulates the weight. Two 1s casts complete, t=1 and 2:
    //   measured-before (correct): acc = 0.5 (t=1, empowered still 0)
    //                              acc = 0.5 + 1.0 = 1.5 at t=2 → 1 fire
    //   measured-after (old):      acc = 1.0 at t=1 → fire, acc 0
    //                              acc = 1.0 at t=2 → fire  → 2 fires
    // ------------------------------------------------------------------
    #[test]
    fn ev_on_crit_weight_is_measured_before_this_casts_own_procs() {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg", "crit_chance"],
                 "conditions": ["empowered"],
                 "events": { "crit": { "chance": "(crit_chance + empowered * 50) / 100",
                                        "factor": "2" } },
                 "pipeline": [ { "name": "hit", "expr": "dmg * event_factors",
                                 "branched": true } ],
                 "objectives": ["hit"] }"#,
        )
        .unwrap();
        let plan = plan::compile(&def).unwrap();
        let build: BuildState =
            serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "crit_chance": 50.0 } }"#).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 2 } ] }"#).unwrap();

        let mut actions = BTreeMap::new();
        actions.insert(
            "strike".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut buffs = BTreeMap::new();
        let mut empowered = BTreeMap::new();
        empowered.insert("empowered".to_string(), 1.0);
        buffs.insert(
            "focus".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(100.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: empowered,
                tick_objective: None,
            },
        );
        buffs.insert(
            "marker".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(1.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "focus_proc".to_string(),
            ProcDef {
                trigger: Trigger::OnHit,
                chance: "1".into(),
                icd: 100.0, // fires on the first hit only
                apply_buff: Some("focus".into()),
                cast_action: None,
            },
        );
        procs.insert(
            "crit_proc".to_string(),
            ProcDef {
                trigger: Trigger::OnCrit,
                chance: "1".into(),
                icd: 0.0,
                apply_buff: Some("marker".into()),
                cast_action: None,
            },
        );
        let simdef = SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "strike".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.actions["strike"].casts, 2);
        assert_eq!(report.proc_counts["focus_proc"], 1);
        assert_eq!(
            report.proc_counts["crit_proc"], 1,
            "2 = the first hit's on_crit weight was measured AFTER its own \
             on_hit proc raised crit chance"
        );
    }

    // ------------------------------------------------------------------
    // (3) `BuffDef::duration` reads the LIVE state at the application
    // instant — which differs between the two application paths, and
    // deliberately so (see `Sim::apply_buff`'s doc comment; Task 4's
    // reapply policies build on this).
    //   first application (buff down): `buff.window` reads 0
    //   refresh        (buff up):      `buff.window` reads 1
    // duration `"5 - buff.window * 4"` therefore opens a 5s window and
    // every refresh CUTS it to 1s. With `icd` 2 the proc fires on alternate
    // filler completions, so hand-worked:
    //   t=1  apply  (down → 5) → expires t=6 ; icd clear at 3
    //   t=3  refresh (up  → 1) → expires t=4
    //   t=4  expire.  window [1,4)  = 3s      ; icd clear at 5
    //   t=5  apply  (down → 5) → expires t=10
    //   t=7  refresh (up  → 1) → expires t=8
    //   t=8  expire.  window [5,8)  = 3s
    //   …repeating every 4s: [9,12), [13,16), [17,20)
    // uptime = 5 × 3 / 20 = 15/20 = 0.75
    // If BOTH paths read `buff.window` as 0 the duration is always 5, every
    // refresh extends, and the buff never lapses → 19/20 = 0.95.
    // ------------------------------------------------------------------
    #[test]
    fn expr_duration_reads_the_live_state_on_both_application_paths() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();
        let (simdef, rotation) =
            expr_duration_fixture(NumOrExpr::Expr("5 - buff.window * 4".into()), 2.0);
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert!(
            close(report.buff_uptime["window"], 0.75),
            "got {} (0.95 = the refresh path read `buff.window` as 0)",
            report.buff_uptime["window"]
        );
    }

    // ------------------------------------------------------------------
    // (4) A resource's `max`/`regen_per_sec` is re-derived on every
    // effective-state change, against the slot array AS OF that change —
    // tail included, uniformly at every call site. `refresh_effective_state`
    // owns that refresh so the freshness never depends on which caller
    // last happened to update the tail (a stale read there is silent).
    //
    // Fixture: `mana` has `max` = `"50 + buff.boost * 50"`, regen 10/s, no
    // costs. Hand-worked over a 20s fight:
    //   t=0   initial fold, boost down → max 50, pool starts full at 50
    //   t=1   filler completes, proc applies `boost` → refold → max 100
    //         (pool stays 50 and starts refilling at 10/s)
    //   t=6   pool reaches the new cap of 100 and pins there
    //   time_capped = [0,1) + [6,20) … but the pool is settled lazily, so
    //   the single settle at t=20 computes it directly: amount 50 < max
    //   100, t_reach = (100-50)/10 = 5s, capped = 20 - 5 = 15s.
    // If the refold read a stale tail (boost still down) the cap stays 50,
    // the pool is at cap the whole fight, and time_capped = 20.
    // ------------------------------------------------------------------
    #[test]
    fn resource_max_expr_refolds_against_the_applying_buff() {
        let plan = flat_plan();
        let build = flat_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

        let mut resources = BTreeMap::new();
        resources.insert(
            "mana".to_string(),
            ResourceDef {
                max: "50 + buff.boost * 50".into(),
                regen_per_sec: "10".into(),
            },
        );
        let mut actions = BTreeMap::new();
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "boost".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(100.0),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "boost_proc".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd: 100.0,
                apply_buff: Some("boost".into()),
                cast_action: None,
            },
        );
        let simdef = SimDef {
            resources,
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        };
        let rotation = Rotation {
            rules: vec![Rule {
                action: "filler".into(),
                when: None,
            }],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert_eq!(report.proc_counts["boost_proc"], 1);
        assert!(
            close(report.resources["mana"].time_capped, 15.0),
            "got {} (20 = the refold read a stale tail and kept max at 50)",
            report.resources["mana"].time_capped
        );
    }

    // ------------------------------------------------------------------
    // Symbol-space invisibility holds at ALL FIVE expression-valued sites,
    // not just `duration`: pipeline stages and buckets are absent from the
    // sim symbol space by design, so naming one is the same fail-closed,
    // positioned "unknown identifier" any other unresolved name gets. All
    // five route through the one `compile_value` helper, which is exactly
    // why this is table-driven rather than five hand-written tests.
    // ------------------------------------------------------------------
    #[test]
    fn every_expression_valued_field_rejects_a_pipeline_stage() {
        let plan = flat_plan();
        let stage = || NumOrExpr::Expr("hidden_stage".into());
        let mut damage_stats = BTreeMap::new();
        damage_stats.insert("dmg".to_string(), stage());

        let cases: Vec<(&str, (SimDef, Rotation))> = vec![
            ("duration", expr_duration_fixture(stage(), 10.0)),
            ("cooldown", expr_cooldown_fixture(stage())),
            ("cost", expr_cost_fixture(stage())),
            ("gain", expr_gain_fixture(stage())),
            ("damage.stats", beam_fixture(damage_stats)),
        ];
        for (field, (simdef, rotation)) in cases {
            let e = match sim_compile(&plan, &simdef, &rotation) {
                Err(e) => e,
                Ok(_) => panic!("`{field}` accepted a pipeline stage"),
            };
            assert!(
                e.what.contains("hidden_stage"),
                "`{field}` error should name the stage, got: {}",
                e.what
            );
            assert!(
                e.what.contains(field),
                "`{field}` error should name the field, got: {}",
                e.what
            );
            assert!(
                e.what.contains("at byte"),
                "`{field}` error should be positioned, got: {}",
                e.what
            );
        }
    }

    // ══════════════════════════════════════════════════════════════════
    // P7c-T1 — the instance runtime: stacks and reapply policies.
    //
    // ONE toy base for every trajectory below, so the cadence is
    // hand-worked once, here:
    //
    //   plan     `hit = dmg * boost`, `boost` a PRODUCT bucket (Π(1+v/100))
    //            and `dmg` = 100 from the build. No events, no conditions
    //            — every hit's value is a pure function of the live stack
    //            count, which is the point.
    //   filler   cast_time 1, no cost/cooldown, always eligible (last
    //            rule) — one hit per second, hit N completing at t=N.
    //   charge_gen  cast_time 0, cooldown `gen_cooldown` (first rule) —
    //            casts at t=0 and then whenever its cooldown is up.
    //   charge   the buff under test: one contribution of +10 to `boost`,
    //            so k stacks read `boost = 1 + 10k/100` (NOT 1.1^k — a
    //            per-stack contribution scales its VALUE by the count,
    //            see `simdef::BuffDef`).
    //   charge_pulse  on_cast, chance `pulse_chance`, icd `pulse_icd`,
    //            apply_buff `charge`.
    //
    // Application cadence with `pulse_chance` = "1", `pulse_icd` = 2, and
    // `gen_cooldown` = 2 — the icd==cooldown trick, hand-traced:
    //   t=0  charge_gen completes (instant, first rule) → on_cast roll,
    //        icd clear → APPLY, icd_ready=2. filler then begins.
    //   t=1  filler completes → on_cast roll, 1 < 2 → gated.
    //   t=2  filler completes → on_cast roll, 2 >= 2 → APPLY,
    //        icd_ready=4; THEN the decision casts charge_gen (off
    //        cooldown at 2), whose own on_cast roll is now gated.
    //   … so exactly ONE application every 2s: t=0,2,4,…,18 in a 20s sim
    //   (the t=20 application lands exactly at the end and integrates to
    //   zero seconds).
    // Note the application at t=2 is physically the FILLER's on_cast, not
    // the generator's — the trick only fixes the CADENCE at the
    // generator's cooldown, which is all these pins need.
    // icd==cooldown trick; Task 6 replaces this with ActionDef.apply_buff
    // ══════════════════════════════════════════════════════════════════

    fn stack_plan() -> Plan {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg"],
                 "buckets": { "boost": { "fold": "product" } },
                 "pipeline": [ { "name": "hit", "expr": "dmg * boost" },
                               { "name": "dot", "expr": "dmg * 0.5" } ],
                 "objectives": ["hit", "dot"] }"#,
        )
        .unwrap();
        plan::compile(&def).unwrap()
    }

    fn stack_build() -> BuildState {
        serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap()
    }

    /// The shared fixture described above. `charge` carries one `+10`
    /// contribution to the `boost` product bucket.
    fn stack_simdef(
        on_reapply: ReapplyPolicy,
        max_stacks: u32,
        buff_duration: f64,
        gen_cooldown: f64,
        pulse_chance: &str,
        pulse_icd: f64,
    ) -> SimDef {
        let mut actions = BTreeMap::new();
        actions.insert(
            "charge_gen".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(gen_cooldown),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
            },
        );
        actions.insert(
            "filler".to_string(),
            ActionDef {
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    stats: BTreeMap::new(),
                }),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "charge".to_string(),
            BuffDef {
                duration: NumOrExpr::Num(buff_duration),
                max_stacks,
                on_reapply,
                contributions: vec![Contribution {
                    bucket: "boost".into(),
                    value: 10.0,
                    event: None,
                    condition: None,
                }],
                conditions: BTreeMap::new(),
                tick_objective: None,
            },
        );
        let mut procs = BTreeMap::new();
        procs.insert(
            "charge_pulse".to_string(),
            ProcDef {
                trigger: Trigger::OnCast,
                chance: pulse_chance.into(),
                icd: pulse_icd,
                apply_buff: Some("charge".into()),
                cast_action: None,
            },
        );
        SimDef {
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        }
    }

    fn stack_rotation() -> Rotation {
        Rotation {
            rules: vec![
                Rule {
                    action: "charge_gen".into(),
                    when: None,
                },
                Rule {
                    action: "filler".into(),
                    when: None,
                },
            ],
        }
    }

    fn twenty_second_dummy() -> Scenario {
        serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap()
    }

    // ------------------------------------------------------------------
    // `add_refresh_all` (PoE2 charges): count +1 up to `max_stacks`, and
    // EVERY instance's expiry reset to `now + duration` — one shared
    // clock. duration 5, max_stacks 3, applications t=0,2,4,…,18.
    //
    //   t=0   push          → 1 instance,  all expire 5
    //   t=2   push          → 2 instances, all expire 7
    //   t=4   push          → 3 instances, all expire 9
    //   t=6+  AT CAP: no new instance, but the shared clock still resets
    //         (all expire 11, 13, …, 23) — so the stack never falls off
    //         inside the 20s sim.
    // Stack trajectory: 1 on [0,2), 2 on [2,4), 3 on [4,20].
    //   avg_stacks = (1·2 + 2·2 + 3·16) / 20 = (2 + 4 + 48) / 20 = 2.7
    //   buff_uptime = 20/20 = 1.0
    //
    // Per-hit (`boost` is a PRODUCT bucket, one +10 contribution scaled by
    // the stack count): 1 stack → 100 × (1 + 10/100) = 110; 3 stacks →
    // 100 × (1 + 30/100) = 130 (NOT 100 × 1.1³ = 133.1 — the count scales
    // the contribution's VALUE, it does not repeat the contribution).
    //
    // Which hit sees which count: a cast's damage is measured BEFORE that
    // cast's own procs roll, so the hit AT an application instant still
    // reads the pre-application count.
    //   t=1: 1 stack → 110      t=2: 1 stack → 110
    //   t=3: 2 stacks → 120     t=4: 2 stacks → 120
    //   t=5..20 (16 hits): 3 stacks → 130
    //   total = 110 + 110 + 120 + 120 + 16×130 = 460 + 2080 = 2540
    // ------------------------------------------------------------------
    #[test]
    fn add_refresh_all_stacks_to_the_cap_on_one_shared_clock() {
        let plan = stack_plan();
        let build = stack_build();
        let simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "1", 2.0);
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();

        let report = run(
            &plan,
            &sim_plan,
            &build,
            &twenty_second_dummy(),
            Mode::Expected,
        )
        .unwrap();

        assert_eq!(report.actions["filler"].casts, 20);
        assert!(
            close(report.avg_stacks["charge"], 2.7),
            "avg_stacks: got {} — want (1·2 + 2·2 + 3·16)/20 = 2.7",
            report.avg_stacks["charge"]
        );
        assert!(
            close(report.buff_uptime["charge"], 1.0),
            "buff_uptime: got {} — the shared clock resets every 2s against \
             a 5s duration, so `charge` never falls off",
            report.buff_uptime["charge"]
        );
        assert!(
            close(report.total.total_damage, 2540.0),
            "total damage: got {} — want 110+110+120+120+16×130 = 2540 \
             (per-hit 110 at 1 stack, 130 at 3 — NOT 133.1)",
            report.total.total_damage
        );
    }

    // ------------------------------------------------------------------
    // The shared clock's other half: when applications STOP, all three
    // instances fall off TOGETHER at (last application + duration).
    //
    // Same fixture, except applications stop after t=4. Stopping them is
    // what needs a workaround today: the `charge_pulse` proc is `on_cast`
    // and any action's cast triggers it, so lengthening `charge_gen`'s
    // cooldown alone would NOT stop the filler from carrying the same
    // cadence. A `chance` of `"time <= 4"` gates the proc off instead —
    // 1 while the clock is ≤ 4, 0 after, so the EV accumulator stops
    // banking anything at all.
    // icd==cooldown trick; Task 6 replaces this with ActionDef.apply_buff
    //
    //   t=0 push → 1 instance,  all expire 5
    //   t=2 push → 2 instances, all expire 7
    //   t=4 push → 3 instances, all expire 9
    //   t=6 chance 0 → no application ever again
    //   t=9 ALL THREE expire at once → 0 stacks
    // Stack trajectory: 1 on [0,2), 2 on [2,4), 3 on [4,9), 0 on [9,20].
    //   avg_stacks  = (1·2 + 2·2 + 3·5) / 20 = (2 + 4 + 15) / 20 = 1.05
    //   buff_uptime = 9 / 20 = 0.45
    //
    // If instances kept their OWN clocks instead (i.e. `add_independent`
    // semantics leaking in) they would expire at 5, 7 and 9 — 3 stacks
    // only on [4,5) — and avg_stacks would read (1·2+2·2+3·1+2·2+1·2)/20
    // = 0.75 against the same 0.45 uptime.
    // ------------------------------------------------------------------
    #[test]
    fn add_refresh_all_expires_every_instance_together() {
        let plan = stack_plan();
        let build = stack_build();
        let simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "time <= 4", 2.0);
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();

        let report = run(
            &plan,
            &sim_plan,
            &build,
            &twenty_second_dummy(),
            Mode::Expected,
        )
        .unwrap();

        assert!(
            close(report.buff_uptime["charge"], 0.45),
            "buff_uptime: got {} — all three instances share one clock and \
             expire together at 4+5=9, so 9/20 = 0.45",
            report.buff_uptime["charge"]
        );
        assert!(
            close(report.avg_stacks["charge"], 1.05),
            "avg_stacks: got {} — want (1·2 + 2·2 + 3·5)/20 = 1.05",
            report.avg_stacks["charge"]
        );
    }

    // ------------------------------------------------------------------
    // The span still OPEN when the sim ends counts: `finalize` closes the
    // stack integral at `duration` exactly the way it closes
    // `buff_uptime`'s active span.
    //
    // The same `add_refresh_all` fixture over 25s instead of 20s. The
    // last application lands at t=24 (applications are every 2s and the
    // proc's icd then runs to 26, so the t=25 hit is gated) — so the
    // final second is absorbed by nothing but `finalize`.
    //   ∫ stacks dt = 1·2 + 2·2 + 3·21 = 2 + 4 + 63 = 69
    //   avg_stacks  = 69 / 25 = 2.76
    // Dropping `finalize`'s flush would silently report 66/25 = 2.64.
    // ------------------------------------------------------------------
    #[test]
    fn avg_stacks_integrates_the_span_still_open_when_the_sim_ends() {
        let plan = stack_plan();
        let build = stack_build();
        let simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "1", 2.0);
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 25 } ] }"#).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert!(
            close(report.avg_stacks["charge"], 2.76),
            "avg_stacks: got {} — want (1·2 + 2·2 + 3·21)/25 = 2.76 (2.64 \
             would mean the final open span went uncounted)",
            report.avg_stacks["charge"]
        );
    }

    // ------------------------------------------------------------------
    // `add_independent` (PoE2 poison): every instance keeps its OWN
    // duration, and at `max_stacks` the EARLIEST-EXPIRING instance is
    // evicted to make room for the new one.
    //
    // max_stacks 2, duration 4, applications at t=0,1,2 (icd 1, and the
    // same `"time <= 2"` gate as above stops them after t=2; `charge_gen`
    // is parked on a 100s cooldown so it contributes only the t=0
    // application and the filler carries t=1 and t=2).
    // icd==cooldown trick; Task 6 replaces this with ActionDef.apply_buff
    //
    //   t=0 push       → [exp 4]           len 1
    //   t=1 push       → [exp 4, exp 5]    len 2
    //   t=2 AT CAP: evict the earliest-expiring (exp 4), push exp 6
    //                  → [exp 5, exp 6]    len 2
    //   t=5 exp-5 instance expires → [exp 6]  len 1
    //   t=6 exp-6 instance expires → []       len 0
    // Stack trajectory: 1 on [0,1), 2 on [1,5), 1 on [5,6), 0 on [6,20].
    //   avg_stacks  = (1·1 + 2·4 + 1·1) / 20 = (1 + 8 + 1) / 20 = 0.5
    //   buff_uptime = 6 / 20 = 0.3
    //
    // Evicting the NEWEST instead would leave [exp 4, exp 6]: same 0.3
    // uptime, but avg_stacks (1·1 + 2·3 + 1·2)/20 = 0.45. Not evicting at
    // all (cap ignored) would leave [4,5,6]: uptime 0.3, avg_stacks
    // (1·1 + 2·1 + 3·2 + 2·1 + 1·1)/20 = 0.6. So `avg_stacks` is the
    // assertion that actually pins the eviction rule.
    // ------------------------------------------------------------------
    #[test]
    fn add_independent_evicts_the_earliest_expiring_at_the_cap() {
        let plan = stack_plan();
        let build = stack_build();
        let simdef = stack_simdef(
            ReapplyPolicy::AddIndependent,
            2,
            4.0,
            100.0,
            "time <= 2",
            1.0,
        );
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();

        let report = run(
            &plan,
            &sim_plan,
            &build,
            &twenty_second_dummy(),
            Mode::Expected,
        )
        .unwrap();

        assert!(
            close(report.avg_stacks["charge"], 0.5),
            "avg_stacks: got {} — want (1·1 + 2·4 + 1·1)/20 = 0.5 (0.45 \
             would mean the NEWEST instance was evicted; 0.6 would mean the \
             cap was ignored)",
            report.avg_stacks["charge"]
        );
        assert!(
            close(report.buff_uptime["charge"], 0.3),
            "buff_uptime: got {} — the last instance expires at 2+4=6, so 6/20 = 0.3",
            report.buff_uptime["charge"]
        );
    }

    // ------------------------------------------------------------------
    // `stacks.<buff>` in a rotation `when`: a rule gated on
    // `stacks.charge >= 3` cannot fire before the third instance lands.
    //
    // Same `add_refresh_all` fixture as the 2.7 pin (stacks reach 3 at
    // t=4), plus a `nuke` action: instant, 1000s cooldown so it casts
    // EXACTLY ONCE, and a `damage.stats` override of `dmg` to the
    // expression `time` — so `actions["nuke"].damage` reads back the
    // instant it cast, multiplied by the boost live at that instant.
    //
    // At t=4 the filler's completion applies the third instance, and only
    // THEN does the decision loop run: `charge_gen` (first rule, off
    // cooldown at 4) casts, then `nuke`'s `when` sees stacks = 3 and
    // fires. Its damage is `time × (1 + 30/100)` = 4 × 1.3 = 5.2.
    // A rule that ignored the gate would fire at t=0 (damage 0 × 1.0 =
    // 0); one that mis-read `stacks` as `buff` (1 while active) likewise.
    // ------------------------------------------------------------------
    #[test]
    fn stacks_symbol_gates_a_rotation_rule_until_the_third_instance() {
        let plan = stack_plan();
        let build = stack_build();
        let mut simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "1", 2.0);
        let mut nuke_stats = BTreeMap::new();
        nuke_stats.insert("dmg".to_string(), NumOrExpr::Expr("time".into()));
        simdef.actions.insert(
            "nuke".to_string(),
            ActionDef {
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(1000.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage { stats: nuke_stats }),
            },
        );
        let rotation = Rotation {
            rules: vec![
                Rule {
                    action: "charge_gen".into(),
                    when: None,
                },
                Rule {
                    action: "nuke".into(),
                    when: Some("stacks.charge >= 3".into()),
                },
                Rule {
                    action: "filler".into(),
                    when: None,
                },
            ],
        };
        let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

        let report = run(
            &plan,
            &sim_plan,
            &build,
            &twenty_second_dummy(),
            Mode::Expected,
        )
        .unwrap();

        assert_eq!(report.actions["nuke"].casts, 1, "1000s cooldown, 20s sim");
        assert!(
            close(report.actions["nuke"].damage, 5.2),
            "nuke damage: got {} — `dmg` is the expression `time`, so this \
             reads back t × boost = 4 × 1.3 = 5.2 (a t=0 cast would read 0)",
            report.actions["nuke"].damage
        );
    }

    // ------------------------------------------------------------------
    // A stacked DoT ticks at rate × STACK COUNT: k instances of the same
    // non-snapshot `tick_objective` tick k times over.
    //
    // Same `add_refresh_all` fixture as the 2.7 pin, with `charge` also
    // ticking the toy plan's `dot` objective (`dmg × 0.5` = 50/s at one
    // stack — deliberately independent of `boost`, so the rate is a clean
    // constant per stack).
    //
    //   DoT damage = 50 × ∫ stacks dt = 50 × (avg_stacks × duration)
    //              = 50 × (2.7 × 20) = 50 × 54 = 2700
    //   hits are unchanged at 2540, so total = 2540 + 2700 = 5240.
    // A rate that ignored the stack count would tick a flat 50/s for the
    // 20 active seconds = 1000, for a total of 3540.
    // ------------------------------------------------------------------
    #[test]
    fn a_stacked_dot_ticks_at_rate_times_stack_count() {
        let plan = stack_plan();
        let build = stack_build();
        let mut simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "1", 2.0);
        simdef.buffs.get_mut("charge").unwrap().tick_objective = Some("dot".into());
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();

        let report = run(
            &plan,
            &sim_plan,
            &build,
            &twenty_second_dummy(),
            Mode::Expected,
        )
        .unwrap();

        let hits: f64 = report.actions["filler"].damage;
        assert!(close(hits, 2540.0), "hit damage moved: {hits}");
        assert!(
            close(report.total.total_damage - hits, 2700.0),
            "DoT damage: got {} — want 50/s × 54 stack-seconds = 2700 \
             (1000 would mean the rate ignored the stack count)",
            report.total.total_damage - hits
        );
    }

    // ------------------------------------------------------------------
    // EV/MC agreement gate (P7 spec): the stack trajectory must be the
    // SAME in both modes. With `chance: "1"` every application is
    // certain, so this is exact rather than statistical — MC's per-roll
    // `rng.next_f64() < 1.0` is always true (`next_f64` is in [0,1)), and
    // the plan has no events for `evaluate_phase_sampled` to branch on.
    // Both modes must therefore land on the same 2.7.
    // ------------------------------------------------------------------
    #[test]
    fn stack_trajectory_agrees_between_ev_and_monte_carlo() {
        let plan = stack_plan();
        let build = stack_build();
        let simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "1", 2.0);
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();
        let scenario = twenty_second_dummy();

        let ev = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        let mc = run(
            &plan,
            &sim_plan,
            &build,
            &scenario,
            Mode::MonteCarlo {
                iterations: 8,
                seed: 7,
            },
        )
        .unwrap();

        assert!(
            close(ev.avg_stacks["charge"], 2.7) && close(mc.avg_stacks["charge"], 2.7),
            "EV {} vs MC {} — both must be 2.7",
            ev.avg_stacks["charge"],
            mc.avg_stacks["charge"]
        );
        assert!(
            close(ev.total.total_damage, mc.total.total_damage),
            "EV {} vs MC {} total damage",
            ev.total.total_damage,
            mc.total.total_damage
        );
    }

    // ------------------------------------------------------------------
    // The EV/MC gate again, this time in the STOCHASTIC regime the spec
    // actually names — a steady-state stack COUNT, not a deterministic
    // trajectory. `add_independent`, UNBOUNDED (max_stacks 0), duration
    // 4, applied by an `on_hit` proc at chance 0.5 with no ICD against
    // the filler's 1 hit/s cadence.
    //
    // Steady state, both modes: applications arrive at 0.5/s and each
    // instance lives 4s, so by Little's law the mean live count is
    //   L = λ × W = 0.5 × 4 = 2.
    //
    // EV, exactly (the accumulator crosses on every second hit, so
    // applications land at t=2,4,…,40 and an instance applied at t is
    // live on [t, t+4)):
    //   [0,2)  0 stacks   [2,4)  1 stack    [4,6)  2 stacks
    //   [6,40] 2 stacks — at every t≥6 exactly the two applications in
    //          (t−4, t] are live (at t=6 the t=2 instance expires BEFORE
    //          the t=6 hit applies its own: same instant, lower seq).
    //   ∫ stacks dt = 1·2 + 2·2 + 2·34 = 2 + 4 + 68 = 74
    //   avg_stacks  = 74 / 40 = 1.85
    //
    // MC rolls each hit exactly, so it reaches the same steady-state 2
    // but ramps in faster — its first application can land at t=1, where
    // EV's accumulator defers to t=2. Its expected integral is
    //   0 + 0.5 + 1.0 + 1.5 + 2.0×36 = 75 → 1.875,
    // i.e. 1.4% above EV's 1.85 from the RAMP alone, with the
    // steady-state level identical. Hence: EV pinned exactly, and the two
    // required within 5% of each other. If they ever diverge the way the
    // P6 review's ICD-bound proc regime did (+48%), this is the test that
    // says so.
    // ------------------------------------------------------------------
    #[test]
    fn steady_state_stack_count_converges_between_ev_and_monte_carlo() {
        let plan = stack_plan();
        let build = stack_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 40 } ] }"#).unwrap();
        // Unbounded `add_independent`, applied by an on_hit proc at 0.5.
        let mut simdef = stack_simdef(ReapplyPolicy::AddIndependent, 0, 4.0, 100.0, "0.5", 0.0);
        simdef.procs.get_mut("charge_pulse").unwrap().trigger = Trigger::OnHit;
        let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();

        let ev = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
        let mc = run(
            &plan,
            &sim_plan,
            &build,
            &scenario,
            Mode::MonteCarlo {
                iterations: 2_000,
                seed: 7,
            },
        )
        .unwrap();

        let (a, b) = (ev.avg_stacks["charge"], mc.avg_stacks["charge"]);
        assert!(
            close(a, 1.85),
            "EV avg_stacks {a} — want the hand-worked 74/40 = 1.85 \
             (steady-state 2 with the accumulator's t=0..2 ramp)"
        );
        let rel_err = (b - a).abs() / a;
        assert!(
            rel_err < 0.05,
            "EV avg_stacks {a} vs MC {b}, relative error {rel_err} — the \
             modes must agree on the steady-state stack count"
        );
    }

    // ------------------------------------------------------------------
    // Degenerate guard, stated as a test rather than left implicit: a
    // 0.2.0 binary buff (`max_stacks: 1`, `on_reapply: refresh` — the
    // serde defaults) is exactly a one-instance buff, so its `avg_stacks`
    // and its `buff_uptime` are the SAME number. Reuses the P6d proc
    // fixture whose 0.05 uptime is already hand-worked above.
    // ------------------------------------------------------------------
    #[test]
    fn a_binary_buffs_avg_stacks_equals_its_uptime() {
        let plan = minimal_plan();
        let build = minimal_build();
        let scenario: Scenario =
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();
        let simdef = filler_simdef(ProcDef {
            trigger: Trigger::OnHit,
            chance: "0.3".into(),
            icd: 4.0,
            apply_buff: Some("proc_buff".into()),
            cast_action: None,
        });
        let sim_plan = sim_compile(&plan, &simdef, &filler_rotation()).unwrap();

        let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

        assert!(
            close(report.avg_stacks["proc_buff"], 0.05),
            "got {} — one instance at a time, so this must equal the \
             hand-worked 0.05 uptime",
            report.avg_stacks["proc_buff"]
        );
        assert!(close(
            report.avg_stacks["proc_buff"],
            report.buff_uptime["proc_buff"]
        ));
    }
}
