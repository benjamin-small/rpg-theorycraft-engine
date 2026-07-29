//! The timeline executor: a discrete-event stepper over a compiled
//! [`SimPlan`], producing a [`SimReport`] of COMPUTED uptimes/dps in place
//! of `Scenario`'s asserted ones (see the P6 design spec's "Executor —
//! one stepper, one substitution point" and "Scenarios — same schema,
//! Level-2 reading" sections). One decision loop drives
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
//! | `ActionDamage::stats` values | at the action's MEASURED instant (cast complete by default; cast start under `measure: "cast_start"` — P8c), ONCE per cast | [`Sim::capture_world`] |
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
//! contributions fold with their VALUE × the count, a LIVE
//! `tick_objective` ticks at rate × the count, and `stacks.<buff>` reads
//! the count — while `conditions` stay at their full configured value for
//! as long as ANY instance is live (a condition is an uptime fraction, not
//! a quantity, so scaling it by a stack count has no meaning).
//!
//! A SNAPSHOT `tick_objective` (P7c-T2) is the exception to the first
//! rule: each instance CAPTURES the objective's value at its own
//! application into [`BuffInstance::snapshot_rate`] and ticks that
//! unchanged to expiry, so the buff's rate is the plain SUM over
//! instances — the count is inherent in it and is never multiplied in
//! again. Nothing re-reads the `Plan` for such a buff, which is precisely
//! what makes an instance immune to every later stat, phase and buff
//! change (PoE2 ailment semantics), and what gives
//! [`crate::simdef::ReapplyPolicy::Strongest`] a per-instance magnitude to
//! compare candidates by.
//!
//! Expiry keeps the generation self-cancel pattern, with one event per
//! BUFF rather than per instance. The invariant: **at most one non-stale
//! [`Event::BuffExpire`] is on the heap per buff, at `min(expire_at)`**.
//! Every APPLICATION bumps the generation, so whatever was pending
//! self-cancels; the expiry sweep REUSES the current generation for its
//! own reschedule, since a sweep that leaves instances behind is still
//! the same generation's business. The handler drops every instance whose
//! window closed at `now` (`retain(expire_at > now)`, so a reschedule can
//! never land at or before `now`) and reschedules at the new earliest if
//! any survive — see [`Sim::handle_buff_expire`].
//!
//! The effective-fold transaction (flush the integrators → mutate →
//! refold) runs when the COUNT moves, and — for a snapshot
//! `tick_objective` only — also when the instance SET moves under a
//! standing count, since that moves the summed rate: a `refresh`
//! re-capture, an `add_independent` eviction at the cap, a `strongest`
//! replacement. A reapplication that changes neither — a live buff's
//! `refresh`, or any `add_refresh_all` already at `max_stacks` — moves
//! only expiries, and every remaining fold input is count-driven, so it
//! skips the transaction entirely (the 0.2.0 path, byte for byte). ONE
//! caveat, inherited unchanged from 0.2.0's refresh path rather than
//! introduced here: `resource_max`/
//! `resource_regen` are CACHED at fold points, so a resource whose
//! `max`/`regen_per_sec` expression names `buff_remaining.<b>` keeps the
//! pre-reapplication value until the next real fold. (The same already
//! holds for such an expression naming `time`, which no fold point
//! tracks either.) Deliberately unpinned: no fixture writes that config,
//! and pinning it would pin the staleness rather than fix it.
//!
//! # Action-scoped effects (P7d)
//!
//! Two config fields make an effect belong to ONE action instead of to
//! the whole timeline, and together they retire the `icd == cooldown`
//! trick a 0.2.0 config needed for either:
//!
//! - [`crate::simdef::ActionDef::effects`] (or its 0.x `apply_buff`
//!   sugar) — buffs the action itself applies at cast complete
//!   ([`Sim::apply_action_buffs`]), one application per list entry, each
//!   routed through the buff's own [`crate::simdef::ReapplyPolicy`]
//!   exactly like a proc application.
//! - [`crate::simdef::ProcDef::actions`] — a trigger filter
//!   ([`Sim::proc_considers`]); `None` is every action, the 0.2.0
//!   behavior.
//!
//! The resulting intra-instant order at cast complete is OBSERVABLE, and
//! therefore fixed. It is stated once, canonically, in [`super`]'s
//! "cast-complete order" section — with the diagram, which belongs in
//! PUBLIC docs since `mod exec` is private and never renders.
//! [`Sim::complete_cast`] and [`Sim::free_cast`] are its two
//! implementations, and [`Sim::apply_action_buffs`] documents the one
//! measured world ([`WorldSnapshot`], P8c) its snapshot half reads.
//! Deliberately not restated here: six near-copies of this paragraph is
//! how its wording drifted the first time.
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
//!   exactly and DRAINS that instant, INCLUDING a cast that completes AT
//!   the boundary (needed for the keystone cross-check — a cast starting
//!   at `duration − cast_time` must still count) even when a `BuffExpire`
//!   shares the instant with it. See [`Sim::run_loop`]'s "horizon rule"
//!   for the full statement, and [`super`]'s "fight horizon" section for
//!   the config author's version. Scheduling a sentinel `End` entry would
//!   have to out-rank same-instant real events via a second ordering key
//!   ON TOP OF `seq`, which the spec's own "seq tiebreaker" wording does
//!   not ask for — the duration check gets the identical result with one
//!   fewer moving part. (P8d later DID build that second key —
//!   [`QueueItem`]'s `rank`, driving
//!   [`crate::simdef::SimDefaults::event_order`] — but it exists to let
//!   CONFIG reorder real coincident events, and is a constant under the
//!   default; `End` stays folded into the duration check, which needs no
//!   queue entry at all.)
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
//! How many rolls one damaging cast PRESENTS is config since P8e
//! ([`crate::simdef::ProcRolls`], resolved per proc): one per cast by
//! default (`hits_per_use`-blind — the long-standing behavior, RNG
//! stream byte-identical), or one per measured hit under `per_hit` —
//! the EV accumulator fed per hit / one Bernoulli draw per hit, chance
//! evaluated once per cast, the ICD hard-gating between fires even
//! mid-cast (the ICD-at-one-instant rule — see the two roll methods'
//! doc comments and [`ProcRolls`]'s own).
//!
//! `CompiledEffect::CastAction` (a proc casting a free action, [`Sim::free_cast`])
//! is scoped identically in both modes: gains, damage, and the action's
//! own `apply_buff` list (P7d — under THIS cast's overlay), but no cost,
//! no cooldown, and no further proc rolls (avoids reentrancy). Its damage
//! is ALWAYS the EV/branch-blended value (even under `Mode::MonteCarlo` —
//! a v1 scope limit stated on the PUBLIC [`Mode::MonteCarlo`] docs and on
//! `free_cast`'s own doc comment, not an oversight) — pinned end-to-end by
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

use super::compile::{CompiledEffect, CompiledValue, SimPlan};
use super::report::{
    ActionReport, BuffReport, Distribution, PhaseReport, ResourceReport, SimReport, Totals,
};
use crate::build::BuildState;
use crate::plan::{EvalScratch, Plan, PlanError};
use crate::rng::{mix_seed, Pcg32};
use crate::scenario::{Phase, Scenario};
use crate::simdef::{EventOrder, Measure, ProcRolls, ReapplyPolicy, Trigger};

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
    /// accumulator) and a normal cast's damage/crits are SAMPLED rather
    /// than branch-blended. The resulting [`SimReport`] carries the POOLED
    /// MEAN of every per-iteration report field (see [`run`]'s docs) plus
    /// a [`Distribution`] over the `iterations` per-iteration `dps`
    /// values.
    ///
    /// **Two things are deliberately NOT sampled even here**, and both are
    /// v1 scope limits rather than oversights:
    ///
    /// - A proc-triggered FREE cast
    ///   ([`super::CompiledEffect::CastAction`]) always contributes its
    ///   EV/branch-blended damage, and its crit never feeds an `on_crit`
    ///   proc. Tightening this later is additive, not breaking.
    /// - A DoT [`crate::simdef::BuffDef::tick_objective`]'s rate — live or
    ///   snapshot — is always EV-blended, in BOTH modes. A tick is a
    ///   continuous rate, not an event to sample; this is inherited from
    ///   0.2.0 and is why the two modes agree so tightly on DoT totals.
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

/// Hard bound on events processed at the fight horizon (`t == duration`)
/// before [`Sim::run_loop`]'s drain fails closed rather than hang — the
/// same fail-closed shape as [`INSTANT_CHAIN_LIMIT`], deliberately a
/// SEPARATE constant rather than a reuse of it.
///
/// Separate because the two bound different things and would want
/// different tuning: `INSTANT_CHAIN_LIMIT` bounds ONE rotation decision's
/// zero-time cast chain (a config with no finite dps at all), while this
/// bounds how many already-scheduled events may resolve at the LAST
/// instant of the fight. Aliasing them would make a future retune of
/// either silently retune the other, for no shared reason beyond both
/// being "some big number".
///
/// It bounds the HORIZON INSTANT ONLY. The run loop is deliberately
/// unbounded at every other instant — it has to be, since a long fight is
/// legitimately many events — and [`INSTANT_CHAIN_LIMIT`] inside
/// [`Sim::attempt_decision`] is the only guard there. Do not read this
/// constant as "the loop is bounded".
///
/// It IS reachable, by a config rather than by a livelock:
/// [`Sim::new`] schedules every inter-phase boundary upfront, and each
/// TRAILING ZERO-WEIGHT PHASE contributes a boundary at exactly
/// `acc == duration`. A scenario ending in more than
/// `HORIZON_DRAIN_LIMIT` zero-weight phases therefore piles that many
/// `PhaseBoundary` events onto the horizon and trips this bound —
/// pathological, but constructible, and pinned by
/// `too_many_zero_weight_phases_at_the_horizon_fails_closed`. Note what
/// that case is NOT: those events were all scheduled at construction, so
/// nothing rescheduled anything, which is why the error text names the
/// pile-up rather than accusing an effect of re-arming.
///
/// A genuine same-instant RESCHEDULING loop is a different animal, and no
/// current code path produces one: [`Sim::handle_buff_expire`] retains
/// only instances with `expire_at > now`, so it can never re-arm at
/// `now`, and [`Sim::attempt_decision`] (the only source of new casts and
/// `Wake`s) is skipped at the horizon. This bound covers that future too
/// — a policy that CAN re-arm at the same instant fails closed here
/// instead of hanging.
const HORIZON_DRAIN_LIMIT: u32 = 10_000;

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
/// per-action `casts`/`damage`, per-buff `uptime`/`avg_stacks`,
/// `condition_uptime`, per-resource `time_capped`/`time_starved`,
/// `proc_counts`) is the
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
/// once the executor's per-instant decision-chain bound is exceeded
/// (P6 review/C1 — see `Sim::attempt_decision`'s doc comment in the
/// source).
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
        // Same silent-NaN class as the weight (P8a follow-up): a
        // utility-only rotation completes with ZERO `Plan` evaluations,
        // so `validate_and_resolve_build_for_phase`'s per-evaluation
        // checks never run on that route, while the NaN still flows
        // through `write_stat_condition_slots` into rule gates and
        // resource regen. Validate once here, before the event loop —
        // through the SAME validators `Plan` resolution calls, so the
        // two levels agree on the message by construction.
        crate::plan::validate_finite_phase_stats(phase)?;
    }
    crate::plan::validate_finite_build(build)?;
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
    let mut buff_stacks_sum: BTreeMap<String, f64> = BTreeMap::new();
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
        for (name, b) in &report.buffs {
            *buff_uptime_sum.entry(name.clone()).or_insert(0.0) += b.uptime;
            *buff_stacks_sum.entry(name.clone()).or_insert(0.0) += b.avg_stacks;
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

    // Both per-buff fields are pooled as plain means, like every other
    // scalar in this report (see `run`'s docs) — the key set is identical
    // across iterations, so one map is built from both accumulators.
    let mut buffs = BTreeMap::new();
    for (name, uptime_sum) in &buff_uptime_sum {
        buffs.insert(
            name.clone(),
            BuffReport {
                uptime: uptime_sum / n,
                avg_stacks: buff_stacks_sum.get(name).copied().unwrap_or(0.0) / n,
            },
        );
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
        buffs,
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
    /// `buff`'s earliest live instance expires at this time, IF the buff
    /// is still at `generation` (an APPLICATION since this was scheduled
    /// bumps it, making this event stale — processed as a no-op; see
    /// [`Sim::handle_buff_expire`]).
    BuffExpire { buff: usize, generation: u64 },
    /// The scenario crosses into phase index `phase` at this time.
    PhaseBoundary { phase: usize },
    /// Nothing inherently happens — this entry exists purely to force a
    /// fresh decision attempt at a computed time (cooldown clearing or a
    /// resource becoming affordable; see [`Sim::attempt_decision`]).
    Wake,
}

/// The ordering CLASS of `event` under `order` — the middle key of
/// [`QueueItem`]'s `(time, class_rank, seq)` order, computed ONCE when
/// the item is pushed (rank-at-push: `Ord` has no access to config, and
/// the rank is a pure function of the event kind and the run's
/// [`EventOrder`], which cannot change mid-run — so baking it onto the
/// item keeps the comparator self-contained at the cost of one `u8` per
/// entry, 8 bytes with alignment padding).
///
/// Under [`EventOrder::Scheduled`] this is a CONSTANT — deliberately not
/// a per-kind table — so the order degenerates to the 0.3.0 `(time,
/// seq)` order bit-identically (the untouched suite plus the
/// byte-identical `diablo4_rotation` MC block are the proof). Under
/// [`EventOrder::CompletionsFirst`], `CastComplete` outranks everything
/// else coincident; `seq` still breaks all residual ties, so seeded MC
/// stays deterministic under every setting.
fn class_rank(event: &Event, order: EventOrder) -> u8 {
    match order {
        EventOrder::Scheduled => 0,
        // No wildcard arm on purpose, in EITHER match: a new `Event`
        // kind (or a new `EventOrder` policy) must choose its class HERE,
        // as a semantic decision — [`EventOrder`]'s docs, the CHANGELOG,
        // and ROADMAP enumerate the outranked kinds BY NAME, so a
        // classification that falls through a `_` would silently
        // contradict all three. The rest class is deliberately ONE rank:
        // within it `seq` still decides (pinned by
        // `within_the_rest_class_seq_still_decides_under_completions_first`
        // — sub-ranking the rest class is NOT behavior-preserving).
        EventOrder::CompletionsFirst => match event {
            Event::CastComplete { .. } => 0,
            Event::BuffExpire { .. } | Event::PhaseBoundary { .. } | Event::Wake => 1,
        },
    }
}

/// A heap entry: `(time, class_rank, seq, event)`, ordered so the
/// EARLIEST time pops first, same-time ties break by ascending
/// `class_rank` (the configured [`EventOrder`], baked in at push — see
/// [`class_rank`]), and residual ties by ascending `seq` (first
/// scheduled, first processed) — `BinaryHeap` is a max-heap, so all
/// three comparisons are reversed.
struct QueueItem {
    time: FTime,
    /// [`class_rank`] of `event`, computed at push. Constant under the
    /// default order, which makes this field a no-op tiebreak there.
    rank: u8,
    seq: u64,
    event: Event,
}
impl PartialEq for QueueItem {
    fn eq(&self, other: &Self) -> bool {
        // `rank` included for `Eq`/`Ord` consistency, though `seq` is
        // unique per run and decides alone in practice. `event` excluded
        // — it carries no `Ord` (so `cmp` never reads it), and `seq`
        // uniqueness makes it redundant here too.
        self.time == other.time && self.rank == other.rank && self.seq == other.seq
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
        // Every key is compared other-to-self: the max-heap then pops
        // the MINIMUM `(time, rank, seq)` first.
        other
            .time
            .cmp(&self.time)
            .then_with(|| other.rank.cmp(&self.rank))
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
    /// The instant this instance expires.
    ///
    /// It MOVES for an instance that is refreshed in place under
    /// [`ReapplyPolicy::AddRefreshAll`] (the shared clock) — which is
    /// exactly why it is the only mutable half of an instance: an expiry
    /// is a property of the WINDOW, and a window can be extended.
    expire_at: f64,
    /// The `tick_objective` rate this instance captured at ITS OWN
    /// application, ticked unchanged to expiry — the DoT half of PoE2
    /// ailment semantics (see [`crate::simdef::TickObjective::snapshot`]).
    ///
    /// Immutable for the life of the instance, including across an
    /// `add_refresh_all` refresh that moves `expire_at`: a captured rate
    /// belongs to the APPLICATION, and no later event re-captures it. The
    /// only way to get a new rate is a new instance.
    ///
    /// `0.0`, and never read, for a live `tick_objective` or a buff that
    /// does not tick at all — [`Sim::snapshot_total`] is the sole reader
    /// and only a snapshot buff reaches it.
    snapshot_rate: f64,
}

/// Per-buff runtime state. A buff is ACTIVE exactly while `instances` is
/// non-empty; the instance list is the single source of truth for the
/// `buff.<name>`/`buff_remaining.<name>` symbols and the effective fold.
struct BuffRt {
    /// Every live application, in application order. Empty = inactive.
    instances: Vec<BuffInstance>,
    /// Bumped on every APPLICATION — which lets a `BuffExpire` scheduled
    /// against a superseded instance set recognize itself as stale and
    /// no-op. The expiry sweep reuses the CURRENT generation for its
    /// reschedule rather than bumping (see [`Sim::handle_buff_expire`]),
    /// which is what keeps the invariant exact: at most one non-stale
    /// `BuffExpire` on the heap per buff, at `min(expire_at)`.
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

/// ONE cast's world, captured whole at the action's resolved
/// [`Measure`] instant (see [`Sim::capture_world`]) so every `Plan`
/// evaluation in that cast's completion transaction reads the same
/// world: the damage query, `hits_per_use`, the EV `on_crit` weight, and
/// the tick capture of every `ApplyBuff` entry in the action's effects
/// list (P8c — build AND phase, one world per cast).
struct WorldSnapshot {
    /// The build every `Plan` evaluation of this cast reads: the
    /// effective damage build with this action's evaluated `damage.stats`
    /// overlaid (a damaging action), or the plain effective build (a
    /// utility action — captured only when its effects list will read
    /// it).
    build: BuildState,
    /// The effective phase at the measured instant — the ONE-WORLD half
    /// (P8c): this cast's damage query and its `ApplyBuff` tick captures
    /// read THIS phase, never the live one, so a condition an earlier
    /// effects-list entry drives cannot leak into a later entry's
    /// capture (the 0.3.0 400-vs-800 list-reorder incoherence).
    phase: Phase,
    /// The damage half of the measurement — `Some` exactly when the
    /// action deals damage, so this field IS the "is this cast damaging"
    /// discriminant everywhere a snapshot is in hand (one spelling, not
    /// two; `None` for a utility action).
    damage: Option<DamageMeasure>,
}

/// The damage-specific half of a [`WorldSnapshot`]: what a damaging
/// cast's queries need beyond the world itself, measured at the same
/// instant.
struct DamageMeasure {
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
///
/// `value` is CLAMPED to `[0, 1]` on the way in — by one LIVE site, in
/// [`Sim::refresh_after_change`] (the seeding clamp in [`Sim::new`] is
/// belt-and-braces: no buff is active yet there). A condition is an uptime
/// FRACTION, and `Plan` clamps it to that range on the way into its slots,
/// so an out-of-range `BuffDef::conditions` value (`marked: 5.0`) folds as
/// `1.0` — but [`Sim::condition_value`] returns the buff's raw number, and
/// integrating THAT would report an uptime of 4.1667 for a condition the
/// math treated as fully up. The scenario branch of `condition_value`
/// already clamps; this makes the buff branch agree, so the diagnostic
/// never disagrees with the value actually used. Report-only: nothing
/// downstream of the report reads this field. Pinned by
/// `a_buff_driven_condition_uptime_is_clamped_like_the_value_that_folds`.
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
    /// The [`WorldSnapshot`] captured at cast START for the single
    /// in-flight cast, when its resolved measure is
    /// [`Measure::CastStart`] — set by [`Sim::begin_cast`] when it
    /// schedules a timed cast, consumed (`take`n) by
    /// [`Sim::complete_cast`]. `None` while no cast is in flight, for
    /// every `cast_complete`-measured action, and for instant casts
    /// (which always measure in the completion transaction — the two
    /// instants coincide there, see [`Measure`]). At most one cast is
    /// ever in flight (`mid_cast` gates the rotation) and nothing that
    /// resolves between a begin and its completion (buff expiries, phase
    /// boundaries, wakes) can start another, so ONE slot is the whole
    /// storage story — the queue's `CastComplete` entry stays `Copy` and
    /// never carries the snapshot.
    pending_snapshot: Option<WorldSnapshot>,

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
            pending_snapshot: None,
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
            // Belt-and-braces, and DEAD as written: `active_buff_set` was
            // seeded empty just above, so `condition_value` can only take
            // its scenario branch here — and that branch already clamps.
            // The one LIVE clamp for `CondAccum::value` is the one in
            // `refresh_after_change`, where a buff CAN be driving it.
            let v = sim.condition_value(&name).clamp(0.0, 1.0);
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
    ///
    /// PRECONDITION: `time >= self.time` — nothing is ever scheduled into
    /// the past. This is what makes the executor's clock MONOTONE, which
    /// [`Sim::run_loop`]'s `at_horizon` flag depends on (an event pushed
    /// behind the clock would sort ahead of it and flip that flag back to
    /// false mid-drain). It is upheld at every call site rather than
    /// re-checked here, and the checks that uphold it are load-bearing:
    ///
    /// - the phase boundaries in [`Sim::new`] run off a non-decreasing
    ///   `acc` from `self.time == 0.0`, since a negative phase weight is
    ///   rejected by [`run`];
    /// - `now + duration` in [`Sim::apply_buff`] and `now + cooldown`
    ///   go through [`Sim::eval_quantity`], which is
    ///   [`Sim::eval_field`] with `nonneg` — a negative buff duration is a
    ///   fail-closed run error at application, never an expiry behind the
    ///   clock;
    /// - `now + ct` in [`Sim::begin_cast`] rejects `ct < 0.0` explicitly;
    /// - [`Sim::handle_buff_expire`] reschedules only at an `expire_at >
    ///   now` it just retained;
    /// - the `Wake` in [`Sim::attempt_decision`] is `max(cd_ready,
    ///   resource_time)` where `resource_time` is `now` or a future
    ///   affordability crossing.
    ///
    /// So a redundant `time < self.time` check here would be unreachable
    /// code guarding an invariant already enforced where it can be
    /// enforced with a MEANINGFUL message (naming the field and the
    /// instant, rather than "the queue went backwards").
    fn schedule(&mut self, time: f64, event: Event) -> Result<(), PlanError> {
        if !time.is_finite() || time < 0.0 {
            return Err(PlanError {
                what: format!("sim: scheduled a non-finite/negative time ({time}) for {event:?}"),
            });
        }
        self.seq += 1;
        self.heap.push(QueueItem {
            time: FTime(time),
            rank: class_rank(&event, self.sim_plan.event_order),
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
            // Clamped: see `CondAccum`. The UNCLAMPED value still drives
            // `effective_phase` above, where `Plan` clamps it itself.
            let v = self.condition_value(&name).clamp(0.0, 1.0);
            let e = self
                .condition_accum
                .get_mut(&name)
                .expect("seeded in Sim::new for every tracked condition");
            e.value = v;
            e.since = now;
        }
        let active = self.active_buff_set.clone();
        for bi in active {
            if let Some(tick) = self.sim_plan.buffs[bi].tick_objective {
                let rate = if tick.snapshot {
                    // Each instance ticks the rate IT captured, so the
                    // buff's rate is the plain SUM — the stack count is
                    // already inherent in it, and multiplying by the count
                    // again would integrate `∫ stacks² dt`. Nothing here
                    // touches the `Plan`: that is the whole point of a
                    // snapshot, and it is what makes this refold (driven by
                    // a phase boundary or some OTHER buff) leave a snapshot
                    // DoT's rate exactly where it was.
                    self.snapshot_total(bi)
                } else {
                    // × stack count: k independent instances of a LIVE DoT
                    // tick the same re-evaluated rate k times over.
                    let val = self.eval_objective(tick.objective, None)?;
                    val * self.buffs[bi].instances.len() as f64
                };
                let b = &mut self.buffs[bi];
                b.tick_rate = rate;
                b.tick_last_eval = now;
            }
        }
        Ok(())
    }

    /// Evaluate the `Plan` objective at index `obj` — the one place a
    /// tick objective is read, so a live rate and a freshly-captured
    /// snapshot rate are always sampled the same way.
    ///
    /// `world` is the world to evaluate against. `None` means the LIVE
    /// one — the current effective build (base + every live buff's
    /// contributions) against the current effective phase; `Some(w)`
    /// evaluates against the snapshot's build AND phase instead (P8c: one
    /// world per measured cast, both halves from the same instant).
    ///
    /// Which caller passes which is a deliberate SPLIT (P7d for the
    /// build, P8c for the phase):
    ///
    /// - a LIVE tick rate's refold ([`Sim::refresh_after_change`]) passes
    ///   `None` — no cast is in the picture at all; it is the ambient
    ///   rate, by definition.
    /// - an action-effects ([`crate::simdef::ActionDef::effects`])
    ///   application passes `Some(the completing cast's snapshot)` — the
    ///   hit's overlaid build (or the plain effective build for a UTILITY
    ///   action) and the phase of the measured instant, so an ailment
    ///   inherits the magnitude of the hit that applied it and no list
    ///   entry's condition leaks into a later entry's capture.
    /// - a PROC application passes `None`, deliberately UNCHANGED in P7d
    ///   AND P8c: a proc-applied capture reads the live ambient world at
    ///   the fire — sequential across a proc's own effects list, per the
    ///   P8b rule. Switching it is a behavior change with its own numbers
    ///   (it is load-bearing for the P7c-T2 snapshot pins), not a cleanup
    ///   — and a proc has no single cast whose magnitude it obviously
    ///   inherits in the first place (an `on_cast` proc fires from
    ///   utility actions with no damage at all).
    ///
    /// The split is pinned as its own control by
    /// `an_action_applied_snapshot_captures_the_overlay_and_a_proc_applied_one_does_not`,
    /// and the phase half by
    /// `a_same_list_snapshot_capture_reads_one_frozen_world`.
    ///
    /// EV-blended in BOTH modes: this calls `Plan::evaluate_phase`, never
    /// `evaluate_phase_sampled`, so a rate captured during a Monte Carlo
    /// run is the branch-blended expectation, not a sampled branch. That
    /// is inherited from 0.2.0's DoT integration (a tick is a continuous
    /// rate, not an event to sample), and it is WHY the modes agree so
    /// tightly on snapshot-DoT totals: they differ in WHEN instances are
    /// applied, never in what each captures. "Fixing" it would put an RNG
    /// draw on the buff-application path and break same-seed determinism
    /// against every pin in this file.
    fn eval_objective(
        &mut self,
        obj: usize,
        world: Option<&WorldSnapshot>,
    ) -> Result<f64, PlanError> {
        // `plan`, `effective_*` and `scratch` are DISJOINT fields of
        // `self`, which is why none of this needs a clone.
        let (build, phase) = match world {
            Some(w) => (&w.build, &w.phase),
            None => (&self.effective_damage_build, &self.effective_phase),
        };
        let objs = self
            .plan
            .evaluate_phase(build, phase, &mut self.scratch.eval)?;
        Ok(objs[obj])
    }

    /// Σ of `bi`'s live instances' captured rates — a SNAPSHOT buff's total
    /// tick rate. `0.0` when inactive, and meaningless (never read) for a
    /// buff whose `tick_objective` is live.
    fn snapshot_total(&self, bi: usize) -> f64 {
        self.buffs[bi]
            .instances
            .iter()
            .map(|i| i.snapshot_rate)
            .sum()
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
    /// - `strongest` — replace the single incumbent, but ONLY if the
    ///   incoming instance's snapshot rate is strictly higher. A losing
    ///   application is discarded whole: it moves neither the rate nor the
    ///   expiry. (`sim::compile` guarantees such a buff has a snapshot
    ///   `tick_objective` and `max_stacks == 1`.)
    ///
    /// No policy ever closes the active span: `activated_at` is set only
    /// on the 0→1 transition, and only a drop to ZERO instances (in
    /// [`Sim::handle_buff_expire`]) closes it. The flush/refold
    /// transaction runs when the instance COUNT moves — that is what the
    /// effective fold and the condition integrators read — and, for a
    /// SNAPSHOT `tick_objective` only, also when the instance SET moves
    /// under a standing count (a `refresh` re-capture, an
    /// `add_independent` eviction at the cap, a `strongest` replacement),
    /// since that moves the summed tick rate. A `refresh` of an
    /// already-active buff with no snapshot tick therefore still does no
    /// work at all beyond resetting its expiry.
    ///
    /// The buff's `duration` is evaluated HERE — at the application
    /// instant — and SNAPSHOTTED onto the instance this call starts (or,
    /// for `refresh`/`add_refresh_all`, onto the window(s) it resets):
    /// nothing later reads the field again for it, so a stat/phase change
    /// afterwards cannot retroactively move an expiry already on the heap.
    ///
    /// A snapshot `tick_objective`'s RATE is captured at the same instant,
    /// in the same way, against the same state — and, unlike the expiry,
    /// is never moved by any later application: `add_refresh_all` pushes
    /// an existing instance's expiry out while leaving the rate it
    /// captured alone (pinned by
    /// `add_refresh_all_moves_the_expiry_but_never_the_snapshot_rate`).
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
    ///
    /// `world` is forwarded verbatim to [`Sim::eval_objective`] for a
    /// SNAPSHOT capture, and is unused for everything else. `None` — the
    /// live effective build and phase — is what every PROC application
    /// passes and what rtce 0.2.0 always did; `Some(snapshot)` is what an
    /// action-effects ([`crate::simdef::ActionDef::effects`]) application
    /// passes, so an ailment inherits the ONE world its cast measured
    /// (P8c): the hit's build overlay and the phase of the measured
    /// instant together (see [`Sim::apply_action_buffs`]). It
    /// deliberately does NOT reach `duration`, which reads sim STATE
    /// through the slot array rather than a build, and is therefore live
    /// and sequential on both paths — the P8c scope boundary.
    fn apply_buff(&mut self, bi: usize, world: Option<&WorldSnapshot>) -> Result<(), PlanError> {
        let now = self.time;
        self.refresh_time_varying_slots();
        let duration = self.eval_quantity(&self.sim_plan.buffs[bi].duration, || {
            format!(
                "buff `{}` duration at application (t={now})",
                self.sim_plan.buffs[bi].name
            )
        })?;
        let expire_at = now + duration;

        // The incoming instance's SNAPSHOT rate, captured here — at the
        // same instant, and against the same live state, as `duration`
        // above (see this method's docs for what "live state" means on
        // each application path; in particular, on a REAPPLICATION the
        // buff's own currently-live instances are still folded in, so a
        // poison whose contributions feed its own tick objective captures
        // a rate that includes its outgoing stacks).
        //
        // `0.0` for a live tick objective and for a buff that does not
        // tick at all: `snapshot_rate` is only ever READ through
        // `Sim::snapshot_total`, which only a snapshot buff reaches.
        let tick = self.sim_plan.buffs[bi].tick_objective;
        let snapshot = tick.is_some_and(|t| t.snapshot);
        let incoming_rate = match tick {
            Some(t) if t.snapshot => self.eval_objective(t.objective, world)?,
            _ => 0.0,
        };

        let policy = self.sim_plan.buffs[bi].on_reapply;
        let max_stacks = self.sim_plan.buffs[bi].max_stacks;
        let before = self.buffs[bi].instances.len();
        // `strongest`'s whole decision, taken BEFORE anything mutates:
        // the incoming instance lands only if it is STRICTLY stronger than
        // the incumbent (a tie leaves the incumbent alone — an equal
        // reapplication is not an improvement). Meaningless for the other
        // policies, which never consult it.
        //
        // `sim::compile` guarantees a `strongest` buff has `max_stacks ==
        // 1` and a snapshot `tick_objective`, so `first()` IS the
        // incumbent and `incoming_rate` is a real captured rate.
        let strongest_wins = match self.buffs[bi].instances.first() {
            None => true,
            Some(incumbent) => incoming_rate > incumbent.snapshot_rate,
        };
        // Two SEPARATE questions, deliberately not one flag:
        //
        // `at_cap` is a property of the instance list, and it is what the
        // stacking arms below branch on — "room for another" vs "make
        // room". `max_stacks == 0` is unbounded, so never at the cap.
        let at_cap = max_stacks != 0 && before >= max_stacks as usize;
        // `count_changes` is the FOLD GATE: whether `instances.len()`
        // itself moves, which is the only thing the effective fold, the
        // condition integrators and the tick rate care about. It has to
        // be decided BEFORE the mutation because the transaction brackets
        // it (flush the old count's elapsed seconds → mutate → refold at
        // the new one), and when it is false the whole transaction is
        // skipped — that is the 0.2.0 refresh path, byte for byte.
        //
        // The two are NOT the same question: `add_independent` at the cap
        // changes the instance SET (one evicted, one pushed) while the
        // COUNT stands still. The `debug_assert` after the match keeps
        // this prediction honest against what the arms actually did.
        let count_changes = match policy {
            ReapplyPolicy::Refresh => before != 1,
            ReapplyPolicy::AddRefreshAll | ReapplyPolicy::AddIndependent => !at_cap,
            // A replacement: the count moves only on the FIRST application.
            ReapplyPolicy::Strongest => before == 0,
        };
        // A SNAPSHOT buff has a second way for its total tick rate to move:
        // the instance SET can change under a standing COUNT, and the two
        // instances swapped will not in general carry the same captured
        // rate. A live buff cannot hit this — its rate is a function of the
        // count and the world, both of which the gate above already covers
        // — so this is `false` for every 0.2.0 config, and the refresh path
        // stays byte-for-byte what it was.
        let snapshot_set_changes = snapshot
            && match policy {
                // The one instance is REPLACED, so it re-captures.
                ReapplyPolicy::Refresh => before == 1,
                // At the cap nothing is added or removed — only expiries
                // move, and an expiry never moves a captured rate.
                ReapplyPolicy::AddRefreshAll => false,
                // At the cap: evict one, push one.
                ReapplyPolicy::AddIndependent => at_cap,
                // A winner replaces; a loser changes nothing at all.
                ReapplyPolicy::Strongest => before == 1 && strongest_wins,
            };
        // The flush/refold transaction runs when EITHER can move the
        // effective state. `count_changes` alone still decides what the
        // `debug_assert`s below check, because it alone is a statement
        // about the count.
        let transaction = count_changes || snapshot_set_changes;
        if transaction {
            self.flush_before_change();
        }
        self.flush_stacks(bi);
        self.buffs[bi].generation += 1;
        let generation = self.buffs[bi].generation;

        let fresh = BuffInstance {
            expire_at,
            snapshot_rate: incoming_rate,
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
                if !at_cap {
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
                if at_cap {
                    let (victim, _) = self.earliest_expiry(bi).expect(
                        "at the cap, so at least one instance is live \
                         (max_stacks 0 is unbounded and never reaches here)",
                    );
                    self.buffs[bi].instances.remove(victim);
                }
                self.buffs[bi].instances.push(fresh);
            }
            // The incumbent is REPLACED — window and all — only by a
            // strictly stronger application. A LOSING application leaves
            // the instance list untouched: not the rate, and NOT the
            // expiry. That is the mechanic (a weak reapplication cannot
            // extend a strong ailment), and it is what separates
            // `strongest` from "replace but keep the higher rate".
            //
            // It still takes the uniform tail below — generation bump and
            // reschedule — which lands the replacement `BuffExpire` at the
            // same INSTANT the cancelled one held, since the earliest
            // expiry did not move. Not literally a no-op: the new event
            // carries a higher `seq`, so it sorts after anything else
            // already queued at that instant. That property is
            // `add_independent`'s too and predates this task; no fixture
            // distinguishes it, and `strongest` does not make it newly
            // reachable.
            ReapplyPolicy::Strongest => {
                if strongest_wins {
                    self.buffs[bi].instances.clear();
                    self.buffs[bi].instances.push(fresh);
                }
            }
        }
        // The fold gate is a PREDICTION made before the mutation; these
        // keep it honest, so a future policy that forgets to update one
        // of the two halves trips here instead of silently leaving the
        // effective fold stale (a divergence nothing else would catch).
        debug_assert_eq!(
            count_changes,
            before != self.buffs[bi].instances.len(),
            "the fold gate must predict the actual count movement"
        );
        debug_assert!(
            before != 0 || count_changes,
            "a 0→n application must refold: it is what puts `bi` into \
             `active_buff_set`"
        );
        // The snapshot half of the same honesty check: when the
        // transaction is SKIPPED, the buff's total tick rate must be
        // exactly what the last refold left in `tick_rate` — otherwise the
        // integrator would keep billing a rate the instance list no longer
        // supports, silently. (Only meaningful while active, and `before ==
        // 0` always runs the transaction.)
        //
        // The comparison is EXACT `f64` equality on purpose: a skipped
        // transaction means the same `f64`s summed in the same order, so
        // anything but bit equality is a real change — including a policy
        // that REORDERS `instances` without changing membership, which
        // moves the sum's rounding and which no other check would catch.
        debug_assert!(
            transaction || !snapshot || self.snapshot_total(bi) == self.buffs[bi].tick_rate,
            "a snapshot buff's total tick rate moved without a refold"
        );

        if before == 0 {
            self.buffs[bi].activated_at = now;
            self.active_buff_set.push(bi);
            self.active_buff_set.sort_unstable();
        }
        if transaction {
            // The instances are committed BEFORE the refold, so anything
            // the refold evaluates (a resource `max`/`regen_per_sec`,
            // notably) sees `buff.<this>`/`stacks.<this>` and a
            // `buff_remaining.<this>` that agree with each other, rather
            // than a half-applied window.
            self.refresh_after_change()?;
        }
        // One pending expiry per buff, at the EARLIEST live expiry — the
        // generation bumped above cancels whatever was pending before.
        let (_, next) = self
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

    /// Run `action`'s compiled effects list — every buff it names in
    /// [`crate::simdef::ActionDef::effects`] (or the
    /// [`crate::simdef::ActionDef::apply_buff`] sugar, which desugars to
    /// the same compiled list) — in LIST order, at this cast's completion
    /// instant. The first-class replacement for the `icd == cooldown`
    /// proc trick a 0.2.0 config needed to get a buff out of ONE specific
    /// action. Only `ApplyBuff` entries can occur here: `sim::compile`
    /// rejects a `cast_action` effect on an action (recursion — the
    /// A→B→A chain the free-cast guard exists to close), which is also
    /// why a FREE cast running this list still cannot recurse.
    ///
    /// # Where this sits in the completion instant
    ///
    /// Called by [`Sim::complete_cast`] (and [`Sim::free_cast`]) AFTER the
    /// cast's damage has been measured and credited, and BEFORE any of
    /// this cast's proc rolls. Both halves are deliberate:
    ///
    /// - The applying cast does not benefit from the buff it applies —
    ///   the same rule [`Sim::capture_world`] already states for procs.
    /// - A proc rolled by this cast SEES the buff (`buff.<applied>` reads
    ///   `1` in its `chance`). Intrinsic effects of the action resolve
    ///   before effects TRIGGERED by it, which also means the whole list
    ///   precedes the whole proc batch — an action-applied buff and a
    ///   proc-applied one never interleave, whatever the procs' name
    ///   order.
    ///
    /// Both are pinned, by
    /// `an_action_applied_buff_does_not_amplify_the_cast_that_applied_it`
    /// and `a_procs_chance_sees_the_buff_the_same_cast_applied`.
    ///
    /// # The world a snapshot capture reads: ONE, the cast's own (P8c)
    ///
    /// `world` is the cast's [`WorldSnapshot`], captured at the action's
    /// resolved [`Measure`] instant by [`Sim::capture_world`] — the
    /// hit's `damage.stats` overlay folded onto the effective build (or
    /// the plain effective build for a UTILITY action, which runs no
    /// damage query), together with the effective PHASE of that same
    /// instant. It matters only to a SNAPSHOT `tick_objective`: a PoE2
    /// ailment takes the magnitude of the hit that applied it, so an
    /// action-applied capture reads the hit's world rather than the
    /// ambient one. (The PROC path is deliberately left on the live
    /// ambient world — see [`Sim::eval_objective`].)
    ///
    /// Capturing ONCE, before the list runs, is the whole point: every
    /// entry in one list, on either action path (damaging or utility),
    /// captures against the SAME world. Left live, a list's second entry
    /// would see the first entry's `contributions` and driven conditions,
    /// so the same list would mean different things depending on entry
    /// order — the 0.3.0 400-vs-800 incoherence, where the build was
    /// frozen but the phase was not and a pure reorder of
    /// `["mark", "poison"]` doubled the DoT at identical reported uptime.
    ///
    /// # What the snapshot does NOT freeze
    ///
    /// `Plan` evaluations, and only those. [`Sim::apply_buff`] still
    /// flushes and refolds per entry, and a [`BuffDef::duration`]
    /// expression still reads the LIVE slot array — sim state stays
    /// SEQUENTIAL across the list (`"2 * (1 + stacks.earlier)"` works and
    /// means what it says). Two axes, stated canonically on
    /// [`crate::simdef::ActionDef::effects`]:
    ///
    /// - sim STATE (slot array): sequential — refreshed per entry.
    /// - the measured WORLD (build + phase): frozen — this snapshot.
    ///
    /// Both are pinned:
    /// `apply_buff_applies_the_list_in_order_and_a_repeat_applies_twice`,
    /// `a_snapshot_capture_is_frozen_across_the_list_on_both_action_paths`,
    /// and `a_same_list_snapshot_capture_reads_one_frozen_world` (which
    /// also pins that the two ACTION PATHS agree in both list orders).
    ///
    /// The snapshot was captured only because this list is non-empty (or
    /// the action deals damage), so an action that applies nothing —
    /// every action in every 0.2.0 config — costs exactly the early
    /// return.
    ///
    /// [`Sim::apply_buff`] is self-bracketing (flush → mutate → refold →
    /// reschedule), so this loop is just N applications; nothing here has
    /// to know what any policy does.
    ///
    /// [`BuffDef::duration`]: crate::simdef::BuffDef::duration
    fn apply_action_buffs(
        &mut self,
        action: usize,
        world: Option<&WorldSnapshot>,
    ) -> Result<(), PlanError> {
        // `sim_plan` is a `&'a` field — reading it out decouples the
        // iteration from `self`'s own borrow (see `Sim`'s docs).
        let sim_plan = self.sim_plan;
        if sim_plan.actions[action].effects.is_empty() {
            return Ok(());
        }
        let world = world.expect(
            "a cast with a non-empty effects list always has a snapshot — \
             capture_world returns Some whenever the list is non-empty",
        );
        for effect in &sim_plan.actions[action].effects {
            match *effect {
                CompiledEffect::ApplyBuff(bi) => self.apply_buff(bi, Some(world))?,
                // `sim::compile` rejects a `cast_action` effect on an
                // action (an action free-casting an action reopens the
                // A→B→A recursion the free-cast guard closed), so no
                // `SimPlan` it produced can reach this arm — and
                // `compile` is the only constructor of one.
                CompiledEffect::CastAction(_) => {
                    unreachable!("sim::compile rejects `cast_action` effects on actions")
                }
            }
        }
        Ok(())
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
        // The generation check is the load-bearing half: an application
        // since this was scheduled bumped it and scheduled its own event.
        // The `is_empty` half is belt-and-braces — the one-event
        // invariant means a matching generation always has instances (the
        // only path to zero is a sweep, which bumps nothing but leaves
        // nothing scheduled either) — kept so a future policy that can
        // empty the list some other way degrades to a no-op rather than
        // to an `expect` on the reschedule.
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
        if let Some((_, next)) = self.earliest_expiry(bi) {
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

    /// `(index, expiry)` of `bi`'s earliest-expiring instance — the next
    /// [`Event::BuffExpire`] time, and the eviction victim at the cap
    /// under [`ReapplyPolicy::AddIndependent`]. Ties resolve to the
    /// oldest by application order (`min_by` keeps the first). `None`
    /// when inactive. ONE scan, so the "earliest" rule is defined in one
    /// place rather than once per caller.
    fn earliest_expiry(&self, bi: usize) -> Option<(usize, f64)> {
        self.buffs[bi]
            .instances
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.expire_at
                    .partial_cmp(&b.expire_at)
                    .expect("expiries are finite (see Sim::schedule)")
            })
            .map(|(i, inst)| (i, inst.expire_at))
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
            // An instant cast is ALWAYS measured at the completion
            // position, whatever the resolved `measure` says: the two
            // share the wall-clock instant, and the capture keeps the
            // completion transaction's documented intra-instant position
            // (post-`gain`, post-`casts` increment) — see [`Measure`]
            // for the `casts.<self>` discontinuity this implies.
            self.complete_cast(action)
        } else {
            if self.sim_plan.actions[action].measure == Measure::CastStart {
                debug_assert!(
                    self.pending_snapshot.is_none(),
                    "one cast in flight means at most one pending snapshot"
                );
                // The cast-start world: cost paid, cooldown armed — the
                // world this cast leaves behind as it starts (`casts.<self>`
                // not yet counted, `gain` not yet credited).
                self.pending_snapshot = self.capture_world(action, true)?;
            }
            self.mid_cast = true;
            self.schedule(now + ct, Event::CastComplete { action })
        }
    }

    fn complete_cast(&mut self, action: usize) -> Result<(), PlanError> {
        let now = self.time;
        // A `cast_start` snapshot captured by `begin_cast`, if any —
        // taken FIRST, so nothing later in this transaction (a proc's
        // free cast, notably) could ever see a stale one.
        let pending = self.pending_snapshot.take();
        // The consumer half of `begin_cast`'s producer assert: a pending
        // snapshot can only belong to THIS cast (one in flight), and
        // this cast only produced one if its resolved measure says so.
        debug_assert!(
            pending.is_none() || self.sim_plan.actions[action].measure == Measure::CastStart,
            "a pending snapshot must belong to a cast_start-measured cast"
        );
        self.apply_gain(action, now)?;
        self.actions[action].casts += 1;

        let mut is_crit = false;
        let snap = match pending {
            Some(s) => Some(s),
            // The default instant (`cast_complete`): measure here, in the
            // transaction — after `gain` and the `casts` increment,
            // before this cast's effects and proc rolls.
            None => self.capture_world(action, true)?,
        };
        if let Some(s) = &snap {
            if let Some(d) = &s.damage {
                let dmg = if self.rng.is_some() {
                    let (dmg, crit) =
                        self.eval_action_damage_sampled(&s.build, &s.phase, d.hits)?;
                    is_crit = crit;
                    dmg
                } else {
                    self.eval_action_damage(&s.build, &s.phase, d.hits)?
                };
                self.total_damage += dmg;
                self.phase_damage[self.current_phase] += dmg;
                self.actions[action].damage += dmg;
            }
        }

        self.mid_cast = false;
        // The action's OWN effects, before anything this cast merely
        // triggers — see `Sim::apply_action_buffs` for the ordering and
        // for the one measured world its captures read.
        self.apply_action_buffs(action, snap.as_ref())?;
        // ONE spelling of "was this cast damaging": the measurement's own
        // damage half (a damaging action always has a snapshot, so `snap`
        // being `None` — a utility action without effects — answers no).
        let damage_measure = snap.as_ref().and_then(|s| s.damage.as_ref());
        // `hits` rides along to the roll paths for `ProcRolls::PerHit`
        // procs (P8e) — the MEASURED count, from the same snapshot the
        // damage was multiplied by. `None` for the `OnCast` roll (a cast
        // is one event, not a hit count — see [`ProcRolls`]'s scope
        // section) and, vacuously, for a utility cast (no damage half →
        // no hit-trigger roll at all, the long-standing rule).
        if self.rng.is_some() {
            self.roll_procs_mc(Trigger::OnCast, true, action, None)?;
            if let Some(d) = damage_measure {
                let hits = d.hits;
                self.roll_procs_mc(Trigger::OnHit, true, action, Some(hits))?;
                self.roll_procs_mc(Trigger::OnCrit, is_crit, action, Some(hits))?;
            }
        } else {
            self.roll_procs_ev(Trigger::OnCast, 1.0, action, None)?;
            if let Some(d) = damage_measure {
                let crit_chance = d.crit_chance.expect(
                    "capture_world(.., true) fills crit_chance for a \
                     damaging action in EV mode",
                );
                let hits = d.hits;
                self.roll_procs_ev(Trigger::OnHit, 1.0, action, Some(hits))?;
                self.roll_procs_ev(Trigger::OnCrit, crit_chance, action, Some(hits))?;
            }
        }
        Ok(())
    }

    /// Capture `action`'s [`WorldSnapshot`] at the CURRENT instant — the
    /// measured world every `Plan` evaluation of this cast will read.
    /// `None` when the cast will run no `Plan` evaluation at all (a
    /// utility action with an empty effects list), so the 0.2.0 default
    /// path pays nothing it did not already pay.
    ///
    /// WHEN this runs is the action's resolved [`Measure`]:
    /// [`Sim::complete_cast`] calls it in the completion transaction
    /// (`cast_complete`, the default), and [`Sim::begin_cast`] calls it
    /// at cast start for a SCHEDULED `cast_start` cast. An instant cast
    /// is always measured at the completion position, whatever `measure`
    /// says (see [`Measure`] for the discontinuity this implies).
    ///
    /// Everything is taken TOGETHER, here: `damage.stats` (and
    /// `hits_per_use`) are evaluated once into one overlay, the effective
    /// PHASE is cloned beside it, and — when `needs_crit_chance` and the
    /// run is `Mode::Expected` — EV's `on_crit` weight is read off that
    /// same overlay and phase. Deferring any of these to its point of use
    /// would read a LATER world (this cast's own effects and procs can
    /// move buffs and resources in between), so one cast's `Plan` queries
    /// could disagree about the world the cast landed in — and a proc
    /// triggered BY this hit cannot retroactively change whether this hit
    /// crit. Pinned by
    /// `ev_on_crit_weight_is_measured_before_this_casts_own_procs`.
    ///
    /// `needs_crit_chance` is `false` for a proc-triggered free cast,
    /// which rolls no procs and would otherwise pay for a `Plan` query
    /// nothing reads.
    ///
    /// # Intra-instant ordering under the default measure
    ///
    /// Under `cast_complete` the completion instant has internal order
    /// (stated in full in [`super`]'s "cast-complete order" section), and
    /// a `damage.stats` expression sees the state AT THIS POINT in it —
    /// AFTER [`Sim::apply_gain`] and this cast's own `casts` increment,
    /// and BEFORE both this cast's own
    /// [`crate::simdef::ActionDef::effects`] and any of its proc rolls.
    /// Concretely: a resource named in a `damage.stats` expression reads
    /// its POST-gain amount, `casts.<this action>` INCLUDES the cast being
    /// measured (so it counts from 1 on the first cast, never 0), and
    /// NEITHER a buff this cast applies itself NOR one applied by a proc
    /// it triggers is visible — a hit cannot be changed by what it causes.
    /// Under `cast_start` both sim-state readings shift to the cast-start
    /// world instead (post-cost, pre-gain, in-flight cast uncounted). All
    /// of it is documented on [`crate::simdef::ActionDamage`] and
    /// [`Measure`].
    fn capture_world(
        &mut self,
        action: usize,
        needs_crit_chance: bool,
    ) -> Result<Option<WorldSnapshot>, PlanError> {
        let now = self.time;
        let damaging = self.sim_plan.actions[action].damage.is_some();
        if !damaging && self.sim_plan.actions[action].effects.is_empty() {
            return Ok(None);
        }
        self.refresh_time_varying_slots();
        let phase = self.effective_phase.clone();
        let (build, damage) = if damaging {
            let build = self.overlay_build_for_action(action, now)?;
            let hits = self.eval_hits_per_use(action, now)?;
            let crit_chance = if needs_crit_chance && self.rng.is_none() {
                Some(self.eval_action_crit_chance(&build, &phase)?)
            } else {
                None
            };
            (build, Some(DamageMeasure { hits, crit_chance }))
        } else {
            // A utility action with effects: its captures read the plain
            // effective build — the frozen-build rule P7d set, captured
            // HERE so both action paths share one code path (and, P8c,
            // one phase).
            (self.effective_damage_build.clone(), None)
        };
        Ok(Some(WorldSnapshot {
            build,
            phase,
            damage,
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
    /// measured world — both halves from its [`WorldSnapshot`], so a
    /// `cast_start`-measured cast is priced in the phase it was measured
    /// in, not the one it happens to complete in.
    fn eval_action_damage(
        &mut self,
        build: &BuildState,
        phase: &Phase,
        hits: f64,
    ) -> Result<f64, PlanError> {
        let objs = self
            .plan
            .evaluate_phase(build, phase, &mut self.scratch.eval)?;
        Ok(objs[self.sim_plan.damage_objective] * hits)
    }

    /// EV mode only: the probability the `"crit"` event fires for one hit
    /// of this cast — see [`Plan::crit_chance`]'s docs for the naming
    /// convention and the fail-soft `0.0` when this game has no `"crit"`
    /// event. Used to weight `on_crit` proc accumulation (see
    /// [`Sim::roll_procs_ev`]'s doc comment). Like its two damage-query
    /// siblings, it takes the measured world's phase explicitly — the
    /// caller ([`Sim::capture_world`]) has the snapshot's clone in hand,
    /// so the one-world invariant is mechanized rather than resting on a
    /// single-caller prose claim.
    fn eval_action_crit_chance(
        &mut self,
        build: &BuildState,
        phase: &Phase,
    ) -> Result<f64, PlanError> {
        self.plan.crit_chance(build, phase, &mut self.scratch.eval)
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
        phase: &Phase,
        hits: f64,
    ) -> Result<(f64, bool), PlanError> {
        let plan = self.plan;
        let rng = self.rng.as_mut().expect("caller checked rng.is_some()");
        let (objs, mask) =
            plan.evaluate_phase_sampled(build, phase, rng, &mut self.scratch.eval)?;
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
    ///
    /// `action` is the cast that produced this event, matched against each
    /// proc's [`crate::simdef::ProcDef::actions`] filter (see
    /// [`Sim::proc_considers`]). A filtered-out cast is not this proc's
    /// event at all: it banks NOTHING, exactly like an ICD-gated roll.
    ///
    /// # `per_hit` rolling (P8e)
    ///
    /// `hits` is the cast's MEASURED `hits_per_use` (from the same
    /// [`WorldSnapshot`] its damage was multiplied by), `None` for the
    /// `OnCast` roll and never present for a utility cast. A proc whose
    /// resolved [`ProcRolls`] is `per_hit` accumulates ONCE PER HIT
    /// instead of once per cast — [`Sim::proc_roll_count`] turns `hits`
    /// into the literal loop count (fail-closed on a fractional value).
    /// Inside the loop:
    ///
    /// - `chance × weight` is evaluated ONCE, before the loop: the hits
    ///   are simultaneous and land in one measured world, so a fire
    ///   mid-loop (its buff, its free cast) is never visible to its own
    ///   sibling hits' chance — it IS visible to later procs in the
    ///   batch (the per-proc refresh below) and to later casts. Pinned
    ///   by `chance_is_evaluated_once_per_cast_not_once_per_hit`.
    /// - The ICD hard gate applies BETWEEN hits exactly as it applies
    ///   between casts: a fire arms `icd_ready_at = now + icd`, and
    ///   since every remaining hit shares this `now`, any `icd > 0`
    ///   gates them all — one fire per cast, the ICD-at-one-instant
    ///   rule ([`ProcRolls`]'s docs; banking the gated hits' mass
    ///   instead would re-create exactly the EV-over-MC inflation I1
    ///   removed — see
    ///   `ev_procs_match_mc_in_the_per_hit_icd_bound_regime_regression`).
    ///   `icd: 0` never gates, so multiple crossings per cast can fire.
    fn roll_procs_ev(
        &mut self,
        trigger: Trigger,
        weight: f64,
        action: usize,
        hits: Option<f64>,
    ) -> Result<(), PlanError> {
        let now = self.time;
        for pi in 0..self.sim_plan.procs.len() {
            if self.sim_plan.procs[pi].trigger != trigger {
                continue;
            }
            if !self.proc_considers(pi, action) {
                continue;
            }
            // Before the ICD gate, so a per_hit/fractional-hits config
            // contradiction surfaces on the FIRST qualifying cast, not
            // whenever the ICD happens to be open.
            let rolls = self.proc_roll_count(pi, action, hits)?;
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
            // `casts.*`, telling the same story). Once per CAST, not per
            // hit — see the doc comment.
            self.refresh_time_varying_slots();
            let chance = self.sim_plan.procs[pi].chance.eval(&self.scratch.slots) * weight;
            for _ in 0..rolls {
                if now < self.procs[pi].icd_ready_at {
                    // A fire earlier in THIS loop armed the ICD; `now` is
                    // constant across the loop, so every remaining hit is
                    // gated — `break` and per-hit `continue` are provably
                    // the same here, and `break` says so.
                    break;
                }
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
        }
        Ok(())
    }

    /// How many rolls proc `pi` presents for ONE qualifying event (P8e):
    /// `1` under [`ProcRolls::PerCast`] (the default — hits-blind, the
    /// long-standing behavior) and for any roll that carries no hit
    /// count (`hits: None` — the `OnCast` roll, whose event is the cast
    /// itself); the measured hit count under [`ProcRolls::PerHit`].
    ///
    /// Fail-closed: `per_hit` rolls a LITERAL count, so a measured
    /// `hits_per_use` that is not a whole number `>= 0` (a fractional
    /// value is an EV averaging device with no per-hit reading) is a
    /// positioned error naming the proc, the action, and the value. The
    /// integer test tolerates float noise the same absolute hair
    /// `PROC_FIRE_EPSILON` does — an expression like `2 + 0.1 * 10`
    /// lands within 1e-9 of its intended whole number, a genuine 2.5
    /// never does.
    ///
    /// The match is deliberately EXHAUSTIVE over [`ProcRolls`] (in-crate,
    /// where `#[non_exhaustive]` does not force a wildcard): a third
    /// policy must be classified HERE, not silently fall through to one
    /// roll — the `class_rank` discipline (P8d).
    fn proc_roll_count(
        &self,
        pi: usize,
        action: usize,
        hits: Option<f64>,
    ) -> Result<u64, PlanError> {
        match (self.sim_plan.procs[pi].rolls, hits) {
            (ProcRolls::PerCast, _) | (ProcRolls::PerHit, None) => Ok(1),
            (ProcRolls::PerHit, Some(h)) => {
                let n = h.round();
                if !h.is_finite() || n < 0.0 || (h - n).abs() > PROC_FIRE_EPSILON {
                    return Err(PlanError {
                        what: format!(
                            "proc `{}` rolls per_hit, but action `{}` measured \
                             hits_per_use {h} at t={} — per-hit rolling needs a \
                             whole number >= 0",
                            self.sim_plan.procs[pi].name,
                            self.sim_plan.actions[action].name,
                            self.time
                        ),
                    });
                }
                Ok(n as u64)
            }
        }
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
    ///
    /// `action` is the cast that produced this event, matched against each
    /// proc's [`crate::simdef::ProcDef::actions`] filter (see
    /// [`Sim::proc_considers`]) BEFORE the RNG is touched — a filtered-out
    /// cast consumes no draw, so adding a filter genuinely removes rolls
    /// from the stream rather than rolling and discarding.
    ///
    /// # `per_hit` rolling (P8e)
    ///
    /// `hits` as on [`Sim::roll_procs_ev`]. A `per_hit` proc presents
    /// one Bernoulli draw PER MEASURED HIT ([`Sim::proc_roll_count`]),
    /// with the same two loop rules as EV's: `chance` evaluated once
    /// per cast (before the loop — the hits share one world), and the
    /// ICD hard-gating the remaining hits after a mid-loop fire (a
    /// gated hit consumes NO draw, exactly as a gated cast consumes
    /// none — so at `icd > 0` the per-cast and per-hit RNG streams
    /// coincide, and the RNG draw count changes ONLY under `icd: 0`
    /// `per_hit` configs). Under the default `per_cast` the loop runs
    /// once and the draw stream is byte-identical to 0.3.0's — proven
    /// by the untouched suite and the byte-identical `diablo4_rotation`
    /// MC block. For `on_crit` procs the hits also share the cast's ONE
    /// sampled crit mask (`qualifies` gates the whole loop): the hits
    /// are simultaneous, so they cannot disagree about whether the cast
    /// crit — which is exactly what makes EV's per-hit `weight × hits`
    /// accumulation the expectation of this path (pinned by
    /// `ev_and_mc_agree_under_per_hit_on_crit_regression`).
    fn roll_procs_mc(
        &mut self,
        trigger: Trigger,
        qualifies: bool,
        action: usize,
        hits: Option<f64>,
    ) -> Result<(), PlanError> {
        if !qualifies {
            return Ok(());
        }
        let now = self.time;
        for pi in 0..self.sim_plan.procs.len() {
            if self.sim_plan.procs[pi].trigger != trigger {
                continue;
            }
            if !self.proc_considers(pi, action) {
                continue;
            }
            // Before the ICD gate — see `roll_procs_ev`.
            let rolls = self.proc_roll_count(pi, action, hits)?;
            if self.procs[pi].icd_ready_at > now {
                continue; // hard gate — no accumulation, no memory.
            }
            // Per-proc — see `roll_procs_ev` for why. Once per CAST, not
            // per hit — also see `roll_procs_ev`.
            self.refresh_time_varying_slots();
            let chance = self.sim_plan.procs[pi].chance.eval(&self.scratch.slots);
            for _ in 0..rolls {
                if self.procs[pi].icd_ready_at > now {
                    // A fire earlier in THIS loop armed the ICD; `now` is
                    // constant, so every remaining hit is gated and draws
                    // nothing (see `roll_procs_ev`'s twin comment).
                    break;
                }
                // A fresh short-lived borrow of `self.rng`, released
                // before `run_proc_effects` below needs `&mut self` in
                // full (calling `self.apply_buff`/`self.free_cast`).
                let roll = self
                    .rng
                    .as_mut()
                    .expect("caller checked rng.is_some()")
                    .next_f64();
                if roll < chance {
                    self.procs[pi].fire_count += 1;
                    self.procs[pi].icd_ready_at = now + self.sim_plan.procs[pi].icd;
                    self.run_proc_effects(pi)?;
                }
            }
        }
        Ok(())
    }

    /// Whether proc `pi`'s trigger CONSIDERS a cast of `action` — its
    /// [`crate::simdef::ProcDef::actions`] filter, where `None` means
    /// every action (rtce 0.2.0's behavior, and the only one it had).
    ///
    /// Checked FIRST in both roll paths: before the ICD gate, before the
    /// `chance` evaluation, and before any RNG draw. A cast this proc does
    /// not consider is not an event for it at all — it must not bank EV
    /// accumulator mass (which an ICD-gated roll also refuses) and must
    /// not consume a Monte Carlo draw (which an ICD-gated roll also
    /// refuses). `sim::compile` guarantees a `Some` list is non-empty, so
    /// this can never be a proc that silently never fires.
    fn proc_considers(&self, pi: usize, action: usize) -> bool {
        match &self.sim_plan.procs[pi].actions {
            None => true,
            Some(list) => list.contains(&action),
        }
    }

    /// Fire proc `pi` at `now`: consume the accumulator, start the ICD,
    /// run the effects. EV mode only — MC mode's [`Sim::roll_procs_mc`]
    /// has no accumulator to consume, so it starts its own ICD and calls
    /// [`Sim::run_proc_effects`] directly; the effect execution itself is
    /// that ONE shared path in both modes.
    fn fire_proc(&mut self, pi: usize, now: f64) -> Result<(), PlanError> {
        self.procs[pi].acc -= 1.0;
        self.procs[pi].fire_count += 1;
        self.procs[pi].icd_ready_at = now + self.sim_plan.procs[pi].icd;
        self.run_proc_effects(pi)
    }

    /// Execute proc `pi`'s compiled effects, in LIST order — the shared
    /// tail of an EV fire ([`Sim::fire_proc`]) and an MC fire
    /// ([`Sim::roll_procs_mc`]), so the two modes cannot drift.
    ///
    /// Sim state is SEQUENTIAL between entries (the P7b rule, same as the
    /// action-side list): an `apply_buff` is self-bracketing (flush →
    /// mutate → refold → reschedule), and a `cast_action` free cast bumps
    /// `casts.<name>` and lands its gains/damage/own-ApplyBuff before the
    /// next entry runs — so a later entry's `duration` expression reads
    /// all of it (the 0.2/0.1 order pin in `mod effects_list`). A repeated
    /// entry applies that many times (the P7d list precedent).
    fn run_proc_effects(&mut self, pi: usize) -> Result<(), PlanError> {
        // `sim_plan` is a `&'a` field — reading it out decouples the
        // iteration from `self`'s own borrow (see `Sim`'s docs).
        let sim_plan = self.sim_plan;
        for effect in &sim_plan.procs[pi].effects {
            match *effect {
                // The PROC path keeps `None` — a proc-applied snapshot
                // captures the ambient effective build, not any cast's
                // overlay (see `Sim::eval_objective`). After an earlier
                // entry in THIS list, "ambient" is the refolded state
                // that entry left behind — sequential, not frozen.
                CompiledEffect::ApplyBuff(bi) => self.apply_buff(bi, None)?,
                CompiledEffect::CastAction(ai) => self.free_cast(ai)?,
            }
        }
        Ok(())
    }

    /// A proc-triggered free cast: gains + damage + the action's own
    /// [`crate::simdef::ActionDef::effects`] (P7d), and NOT cost,
    /// cooldown, or any further proc roll (which avoids reentrancy). That
    /// split is the line between an effect OF the action and the cast
    /// PIPELINE around it — the effects list is the former, so omitting it
    /// here would make the same action mean two different things
    /// depending on who cast it, silently. Applying a buff cannot recurse
    /// (only a cast rolls procs), so it costs the reentrancy guard
    /// nothing.
    ///
    /// Same scope in EV and MC mode alike: damage is ALWAYS
    /// `eval_action_damage` (EV/branch-blended),
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
        // `damage.stats` are both evaluated at `now`, against the LIVE
        // ambient world. The P8c boundary, stated outright: a free cast
        // is measured at ITS OWN instant, never frozen to the snapshot of
        // the outer cast whose proc fired it — an earlier effect in the
        // firing proc's list (an `apply_buff`, say) IS visible here, per
        // the P8b sequential rule. Pinned by
        // `a_free_cast_measures_live_ambient_not_the_outer_casts_snapshot`.
        // No crit chance: this path rolls no procs (see this method's doc
        // comment).
        let snap = self.capture_world(action, false)?;
        if let Some(s) = &snap {
            if let Some(d) = &s.damage {
                let dmg = self.eval_action_damage(&s.build, &s.phase, d.hits)?;
                self.total_damage += dmg;
                self.phase_damage[self.current_phase] += dmg;
                self.actions[action].damage += dmg;
            }
        }
        // Under THIS free cast's own world — see the doc comment.
        self.apply_action_buffs(action, snap.as_ref())?;
        Ok(())
    }

    /// The discrete-event loop: pop the earliest event, advance the clock
    /// to it, resolve it, re-decide. Terminates when the heap is empty or
    /// its next event lies strictly PAST the fight horizon.
    ///
    /// # The horizon rule
    ///
    /// The horizon is `t == duration`, and the rule (stated for config
    /// authors in [`super`]'s "fight horizon" section) is:
    ///
    /// - No cast may BEGIN at or after `duration` — `attempt_decision` is
    ///   skipped once the clock reaches the horizon, so the rotation makes
    ///   no new commitments there.
    /// - EVERY event already scheduled AT `duration` is processed. The
    ///   loop DRAINS the instant rather than stopping after the first one.
    /// - Therefore a cast completing exactly at `duration` counts — its
    ///   `casts`, its damage and its `apply_buff` all land.
    ///
    /// The drain is the load-bearing part. Before it, the loop broke on
    /// `time >= duration` immediately after handling ONE event, so any
    /// other event queued at that same instant was silently discarded and
    /// WHICH one survived was decided by the heap's `(time, seq)`
    /// tie-break: a `BuffExpire` landing on the horizon would swallow the
    /// `CastComplete` there, dropping that cast whole. A cast at the
    /// horizon already counted when it was ALONE on the instant, so this
    /// was never "the horizon excludes its boundary" — it was
    /// order-dependent silent damage loss. Pinned by
    /// `cast_completing_at_the_horizon_counts_even_when_a_buff_expires_there`.
    ///
    /// The drain is bounded by [`HORIZON_DRAIN_LIMIT`] and fails closed
    /// naming the looping event — see that constant for why nothing can
    /// reach it today and why it is nonetheless there.
    fn run_loop(&mut self) -> Result<(), PlanError> {
        self.attempt_decision()?;
        let mut horizon_events: u32 = 0;
        while let Some(top) = self.heap.peek() {
            if top.time.0 > self.duration {
                break;
            }
            let item = self.heap.pop().expect("just peeked Some");
            self.time = item.time.0;
            // The clock is monotone, so once this is true every remaining
            // event that passes the `> duration` guard above sits at
            // exactly `duration`: this flag IS "we are draining the
            // horizon instant". Monotonicity is a PRECONDITION of
            // `Sim::schedule` (see its doc comment for why every call site
            // upholds it), not a property the heap could supply on its
            // own — the heap orders what it is given, and an event pushed
            // into the past would sort ahead of `self.time` and flip this
            // flag back to false.
            let at_horizon = self.time >= self.duration;
            if at_horizon {
                horizon_events += 1;
                if horizon_events > HORIZON_DRAIN_LIMIT {
                    return Err(self.horizon_drain_error(&item));
                }
            }
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
            // No cast BEGINS at or after the horizon — only already
            // committed ones complete.
            if !at_horizon {
                self.attempt_decision()?;
            }
        }
        self.finalize();
        Ok(())
    }

    /// [`HORIZON_DRAIN_LIMIT`]'s fail-closed error, naming the CONFIG
    /// entity `item` refers to rather than just its variant — the whole
    /// point of the bound is to say WHAT is piling up.
    ///
    /// The wording is deliberately non-committal about CAUSE. The only
    /// reachable case today is a scenario ending in more than
    /// `HORIZON_DRAIN_LIMIT` zero-weight phases, whose boundaries were all
    /// scheduled at construction — nothing rescheduled anything there, so
    /// an error accusing an effect of re-arming itself would misdiagnose
    /// the one case a user can actually hit. Both causes are named, in
    /// likelihood order.
    fn horizon_drain_error(&self, item: &QueueItem) -> PlanError {
        let culprit = match item.event {
            Event::CastComplete { action } => format!(
                "cast completion of action `{}`",
                self.sim_plan.actions[action].name
            ),
            Event::BuffExpire { buff, .. } => {
                format!("expiry of buff `{}`", self.sim_plan.buffs[buff].name)
            }
            Event::PhaseBoundary { phase } => {
                format!("phase boundary into `{}`", self.scenario.phases[phase].name)
            }
            Event::Wake => "a rotation wake".to_string(),
        };
        PlanError {
            what: format!(
                "horizon drain bound exceeded: {culprit} at t={} — more than \
                 {HORIZON_DRAIN_LIMIT} events are scheduled at the fight horizon \
                 (duration={}). Either the scenario piles that many events onto the \
                 last instant (a run of trailing zero-weight phases is the usual way \
                 — each one schedules a boundary at exactly `duration`), or some \
                 effect is rescheduling itself there. See `Sim::run_loop`'s doc \
                 comment.",
                self.time, self.duration
            ),
        }
    }

    fn finalize(&mut self) {
        let now = self.duration;
        self.time = now;
        self.flush_conditions(now);
        self.flush_ticks(now);
        // Index iteration rather than `iter_mut` so the `∫ stacks dt`
        // formula lives in `flush_stacks` alone — the borrow checker was
        // the only reason it was ever written out twice.
        for bi in 0..self.buffs.len() {
            self.flush_stacks(bi);
            let b = &mut self.buffs[bi];
            if !b.instances.is_empty() {
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

        let mut buffs = BTreeMap::new();
        for (bi, b) in self.sim_plan.buffs.iter().enumerate() {
            let rt = &self.buffs[bi];
            let (uptime, avg_stacks) = if self.duration > 0.0 {
                (
                    rt.active_seconds / self.duration,
                    rt.stack_seconds / self.duration,
                )
            } else {
                (0.0, 0.0)
            };
            buffs.insert(b.name.clone(), BuffReport { uptime, avg_stacks });
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
            buffs,
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
        Rule, SimDef, TickObjective,
    };
    use std::collections::BTreeMap;

    // Fixtures shared by the nested test modules below. They live
    // HERE, in the parent, so any group can reach them through its
    // own `use super::*` without a cross-module path.

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
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: BTreeMap::new(),
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "proc_buff".to_string(),
            BuffDef {
                extra: Default::default(),
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
            extra: Default::default(),
            defaults: Default::default(),
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        }
    }

    fn filler_rotation() -> Rotation {
        Rotation {
            extra: Default::default(),
            rules: vec![Rule {
                extra: Default::default(),
                action: "filler".into(),
                when: None,
            }],
        }
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
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: BTreeMap::new(),
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "window".to_string(),
            BuffDef {
                extra: Default::default(),
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
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnCast,
                chance: "1".into(),
                icd,
                apply_buff: Some("window".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
            },
        );
        (
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            },
            Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "filler".into(),
                    when: None,
                }],
            },
        )
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
                extra: Default::default(),
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
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: cost_map,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: BTreeMap::new(),
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        (
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources,
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "spender".into(),
                    when: None,
                }],
            },
        )
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
                extra: Default::default(),
                measure: None,
                cast_time: "0".into(),
                cooldown,
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: BTreeMap::new(),
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        (
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "nova".into(),
                    when: None,
                }],
            },
        )
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
                extra: Default::default(),
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
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: cost_map,
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: BTreeMap::new(),
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        actions.insert(
            "generator".to_string(),
            ActionDef {
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: gain_map,
                damage: None,
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        (
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources,
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "spender".into(),
                        when: None,
                    },
                    Rule {
                        extra: Default::default(),
                        action: "generator".into(),
                        when: None,
                    },
                ],
            },
        )
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
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats,
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        (
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            },
            Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "beam".into(),
                    when: None,
                }],
            },
        )
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
    // The icd==cooldown coincidence here is deliberate and STAYS after
    // P7d. This fixture is about STACKS, not about how a buff gets
    // applied; it reaches for a proc because until P7d that was the only
    // mechanism, and `ActionDef::apply_buff` cannot express its varying
    // `chance` in any case. Keeping it also keeps the PROC application
    // path covered — `mod action_scoped` pins the action path separately.
    // ══════════════════════════════════════════════════════════════════

    fn stack_plan() -> Plan {
        let def: GameDef = serde_json::from_str(
            r#"{ "stats": ["dmg"],
                 "conditions": ["focused"],
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
                extra: Default::default(),
                measure: None,
                cast_time: "0".into(),
                cooldown: NumOrExpr::Num(gen_cooldown),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        actions.insert(
            "filler".to_string(),
            ActionDef {
                extra: Default::default(),
                measure: None,
                cast_time: "1".into(),
                cooldown: NumOrExpr::Num(0.0),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats: BTreeMap::new(),
                }),
                apply_buff: Vec::new(),
                effects: Vec::new(),
            },
        );
        let mut buffs = BTreeMap::new();
        buffs.insert(
            "charge".to_string(),
            BuffDef {
                extra: Default::default(),
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
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnCast,
                chance: pulse_chance.into(),
                icd: pulse_icd,
                apply_buff: Some("charge".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
            },
        );
        SimDef {
            extra: Default::default(),
            defaults: Default::default(),
            resources: BTreeMap::new(),
            actions,
            buffs,
            procs,
            damage_objective: "hit".into(),
        }
    }

    fn stack_rotation() -> Rotation {
        Rotation {
            extra: Default::default(),
            rules: vec![
                Rule {
                    extra: Default::default(),
                    action: "charge_gen".into(),
                    when: None,
                },
                Rule {
                    extra: Default::default(),
                    action: "filler".into(),
                    when: None,
                },
            ],
        }
    }

    fn twenty_second_dummy() -> Scenario {
        serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap()
    }

    /// Level-2 timeline mechanics: cadence, waiting, uptime, phases.
    mod timeline {
        use super::*;

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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

            let mut resources = BTreeMap::new();
            resources.insert(
                "mana".to_string(),
                ResourceDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost,
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources,
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(10.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "power_up".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "1".into(),
                    icd: 10.0,
                    apply_buff: Some("power_up".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "empower".into(),
                        when: None,
                    },
                    Rule {
                        extra: Default::default(),
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
                close(report.buffs["power_up"].uptime, 0.4),
                "got {}",
                report.buffs["power_up"].uptime
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
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 20 } ] }"#).unwrap();

            let mut buffs = BTreeMap::new();
            buffs.insert(
                "x".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "instant_nop".into(),
                    when: None,
                }],
            };
            let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

            let err = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).expect_err(
                "zero cast_time + zero cooldown + free cost must fail closed, not hang",
            );
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(10.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut conditions = BTreeMap::new();
            conditions.insert("enraged".to_string(), 1.0);
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "enrage_window".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "1".into(),
                    icd: 10.0,
                    apply_buff: Some("enrage_window".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "empower".into(),
                        when: None,
                    },
                    Rule {
                        extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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

    /// The fight horizon: what happens to events scheduled at exactly
    /// `t == duration` (P7b review — see [`Sim::run_loop`]'s doc comment
    /// for the rule these pin).
    mod horizon {
        use super::*;

        /// `filler`: 1s cast, spammed, applying a `refresh` buff of the
        /// given duration at each cast complete. Nothing here reads the
        /// buff — it exists purely to put a `BuffExpire` on the heap.
        fn horizon_fixture(buff_duration: f64) -> (SimDef, Rotation) {
            let mut actions = BTreeMap::new();
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: vec!["window".into()],
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "window".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(buff_duration),
                    max_stacks: 1,
                    on_reapply: ReapplyPolicy::Refresh,
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: None,
                },
            );
            (
                SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs: BTreeMap::new(),
                    damage_objective: "hit".into(),
                },
                Rotation {
                    extra: Default::default(),
                    rules: vec![Rule {
                        extra: Default::default(),
                        action: "filler".into(),
                        when: None,
                    }],
                },
            )
        }

        // ------------------------------------------------------------------
        // A cast completing AT the horizon counts — and keeps counting when
        // a `BuffExpire` happens to land on the same instant.
        //
        // Hand-worked cadence (10s fight, 1s cast, spammed from t=0): casts
        // complete at t=1,2,…,10 — TEN of them, the tenth landing exactly on
        // the horizon. Each completion refreshes `window`, so the buff's
        // pending expiry is always `(last cast) + buff_duration`; a stale
        // event from application `k` sits on the heap at `k + duration` and
        // no-ops when popped (generation cancel).
        //
        // What each `buff_duration` puts at t=10 alongside `CastComplete`:
        //   2   → BuffExpire(10) from the t=8 application (stale at t=9)
        //   5   → BuffExpire(10) from the t=5 application (stale at t=6)
        //   9   → BuffExpire(10) from the t=1 application (stale at t=2)
        //   9.5 → nothing: expiries land at 10.5, 11.5, … — never at 10
        //
        // In all four cases the tenth cast completes at t=10 and must be
        // credited: 10 casts × 100 dmg = 1000 over 10s = 100 dps.
        //
        // Before the horizon drain this returned NINE casts for 2/5/9 (the
        // `BuffExpire` carried the lower `seq`, so it popped first, and the
        // loop broke on `time >= duration` before ever reaching the
        // `CastComplete`) and TEN for 9.5 — silent, order-dependent damage
        // loss decided by a heap tie-break.
        // ------------------------------------------------------------------
        #[test]
        fn cast_completing_at_the_horizon_counts_even_when_a_buff_expires_there() {
            let plan = minimal_plan();
            let build = minimal_build();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();

            for buff_duration in [2.0, 5.0, 9.0, 9.5] {
                let (simdef, rotation) = horizon_fixture(buff_duration);
                let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
                let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

                assert_eq!(
                    report.actions["filler"].casts, 10,
                    "buff duration {buff_duration}"
                );
                assert!(
                    close(report.total.total_damage, 1000.0),
                    "buff duration {buff_duration}: got {}",
                    report.total.total_damage
                );
                assert!(
                    close(report.total.dps, 100.0),
                    "buff duration {buff_duration}: got {}",
                    report.total.dps
                );
            }
        }

        // ------------------------------------------------------------------
        // A ZERO-WEIGHT FINAL PHASE puts a `PhaseBoundary` on the horizon,
        // and the drain makes that instant's ordering OBSERVABLE for the
        // first time. This pins what it currently does.
        //
        // Fixture: phases `[main: 10, epilogue: 0]`, so `duration` is 10 and
        // the boundary into `epilogue` is scheduled at exactly t=10. A 1s
        // `filler` is spammed, so casts complete at t=1,2,…,10. `epilogue`
        // overrides `dmg` to 250 (vs the build's 100), which makes "which
        // phase's effective state did the 10th cast measure under?" visible
        // in the damage number rather than only in the attribution.
        //
        // Both events sit at t=10. The boundary was scheduled during
        // `Sim::new` (phase weights are static config), the `CastComplete`
        // at t=9 by `begin_cast` — so the boundary holds the far lower `seq`
        // and resolves FIRST. Therefore:
        //   main     = casts at t=1..9  = 9 × 100 =  900, dps 900/10 = 90
        //   epilogue = the t=10 cast    = 1 × 250 =  250, dps 0.0
        //              (weight 0 — `PhaseReport::dps` guards the divide)
        //   total    =                              1150 over 10s = 115 dps
        //   casts    = 10
        //
        // The attribution follows the `seq` rule and nothing else: it is a
        // CONSEQUENCE of draining the horizon in scheduling order, not a
        // designed choice about what a zero-width phase should own. Pinned
        // so that a future phase wanting different attribution has to change
        // this number deliberately instead of drifting into it. (P8d made
        // the other attribution AVAILABLE rather than changing this one:
        // under `defaults.event_order: "completions_first"` the completion
        // outranks the boundary and the cell flips to 1000 / 0 by design —
        // `mod event_order`'s
        // `a_zero_weight_final_phase_cast_flips_to_the_old_phase_under_completions_first`.
        // THIS pin is the `scheduled` cell, and stays.)
        //
        // Also discriminating for the drain itself: before it, the boundary
        // popped first and the loop broke, dropping the 10th cast entirely
        // (9 casts, 900 total, epilogue 0).
        // ------------------------------------------------------------------
        #[test]
        fn a_zero_weight_final_phase_takes_the_horizon_cast_by_the_seq_rule() {
            let plan = minimal_plan();
            let build = minimal_build();
            let scenario: Scenario = serde_json::from_str(
                r#"{ "phases": [ { "name": "main", "weight": 10 },
                                 { "name": "epilogue", "weight": 0,
                                   "stats": { "dmg": 250.0 } } ] }"#,
            )
            .unwrap();

            let mut actions = BTreeMap::new();
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "filler".into(),
                    when: None,
                }],
            };
            let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

            assert_eq!(report.actions["filler"].casts, 10);
            // The boundary resolved BEFORE the cast: the 10th cast is
            // measured under `epilogue` (250, not 100) and credited to it.
            assert!(
                close(report.phases[0].total_damage, 900.0),
                "main: got {}",
                report.phases[0].total_damage
            );
            assert!(
                close(report.phases[0].dps, 90.0),
                "got {}",
                report.phases[0].dps
            );
            assert!(
                close(report.phases[1].total_damage, 250.0),
                "epilogue: got {}",
                report.phases[1].total_damage
            );
            // Weight 0 — the report guards the divide rather than emitting inf.
            assert_eq!(report.phases[1].duration, 0.0);
            assert!(
                close(report.phases[1].dps, 0.0),
                "got {}",
                report.phases[1].dps
            );
            assert!(
                close(report.total.total_damage, 1150.0),
                "got {}",
                report.total.total_damage
            );
            assert!(close(report.total.dps, 115.0), "got {}", report.total.dps);
        }

        /// `main` (10s) followed by `n` zero-weight phases — every one of
        /// their boundaries is scheduled at construction, at exactly
        /// `acc == duration == 10`, so `n` controls how many events pile
        /// onto the horizon instant.
        fn trailing_zero_weight_phases(n: usize) -> Scenario {
            let mut phases = String::from(r#"{ "name": "main", "weight": 10 }"#);
            for i in 0..n {
                phases.push_str(&format!(r#", {{ "name": "z{i}", "weight": 0 }}"#));
            }
            serde_json::from_str(&format!(r#"{{ "phases": [ {phases} ] }}"#)).unwrap()
        }

        // ------------------------------------------------------------------
        // The horizon drain's fail-closed bound, on the one case a CONFIG can
        // actually reach.
        //
        // `Sim::new` schedules every inter-phase boundary upfront, and a
        // TRAILING ZERO-WEIGHT phase's boundary lands at `acc == duration`.
        // So `n` trailing zero-weight phases put `n` `PhaseBoundary` events
        // on the horizon, and the drain — which must process every event at
        // `duration` — walks all of them. Past `HORIZON_DRAIN_LIMIT` it fails
        // closed instead of grinding.
        //
        // Note what this case is NOT: those boundaries were all scheduled at
        // construction. Nothing rescheduled anything, which is why the error
        // text names the pile-up as the likely cause rather than accusing an
        // effect of re-arming itself. The doc comment on the constant
        // originally claimed this bound was UNREACHABLE — it named
        // `handle_buff_expire` as the only same-instant scheduler and missed
        // `Sim::new` entirely. This test is that claim's correction.
        //
        // Two-sided on purpose: at the limit the run must still SUCCEED (a
        // bound that fires early would silently cap legitimate scenarios), and
        // past it must fail closed naming the phase.
        //
        // RED evidence (recorded, not merely asserted): with the
        // `horizon_events`/`HORIZON_DRAIN_LIMIT` block in `run_loop` deleted
        // outright — counter, check, and `horizon_drain_error` call — the
        // `expect_err` arm fails with "the drain must fail closed ... got a
        // report", while every other test in the crate stays green. That is
        // the surviving mutation this test kills; see the commit message for
        // the transcript.
        // ------------------------------------------------------------------
        #[test]
        fn too_many_zero_weight_phases_at_the_horizon_fails_closed() {
            let plan = minimal_plan();
            let build = minimal_build();

            let mut actions = BTreeMap::new();
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "filler".into(),
                    when: None,
                }],
            };
            let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();

            // AT the bound: `HORIZON_DRAIN_LIMIT` boundaries plus the 10th
            // cast's own `CastComplete`. The counter increments once per
            // horizon event and trips on `> LIMIT`, so exactly `LIMIT` events
            // must still run — and the 10 casts must all be there, since the
            // whole point of the drain is that the horizon cast counts.
            let at_limit = HORIZON_DRAIN_LIMIT as usize - 1;
            let scenario = trailing_zero_weight_phases(at_limit);
            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected)
                .expect("at the bound the drain must still complete");
            assert_eq!(report.actions["filler"].casts, 10);
            assert!(
                close(report.total.total_damage, 1000.0),
                "got {}",
                report.total.total_damage
            );

            // PAST the bound: one more boundary than the drain will take.
            let scenario = trailing_zero_weight_phases(HORIZON_DRAIN_LIMIT as usize + 5);
            // `let else` rather than `expect_err`: the Ok payload is a
            // `SimReport` carrying 10,006 `PhaseReport`s, and `expect_err`
            // would `Debug`-print all of it into the failure output.
            let Err(err) = run(&plan, &sim_plan, &build, &scenario, Mode::Expected) else {
                panic!("the drain must fail closed past its bound, got a report");
            };
            // Names the offending PHASE, not just the event variant.
            assert!(
                err.what.contains("phase boundary into `z"),
                "error must name the phase boundary that piled up, got: {}",
                err.what
            );
            assert!(
                err.what.contains("horizon"),
                "error must say where it happened, got: {}",
                err.what
            );
            // And must NOT misdiagnose: nothing rescheduled anything here.
            assert!(
                err.what.contains("zero-weight phases"),
                "error must name the pile-up as a likely cause, got: {}",
                err.what
            );
        }
    }

    /// Procs — the EV accumulator, its ICD hard gate, and Monte Carlo.
    mod procs {
        use super::*;

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
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnHit,
                chance: "0.3".into(),
                icd: 0.0,
                apply_buff: Some("proc_buff".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
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
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnHit,
                chance: "0.3".into(),
                icd: 4.0,
                apply_buff: Some("proc_buff".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
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
                close(report.buffs["proc_buff"].uptime, 0.05),
                "got {} — the second fire must land at hit10 (t=10, == duration, \
                 truncating its 0.5s window to zero), not hit8 (t=8, which would \
                 read 0.1 — that's the pre-fix accumulate-through-ICD behavior)",
                report.buffs["proc_buff"].uptime
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
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 200 } ] }"#)
                    .unwrap();
            let simdef = filler_simdef(ProcDef {
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnHit,
                chance: "0.3".into(),
                icd: 5.0,
                apply_buff: Some("proc_buff".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "proc_buff".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCrit,
                    chance: "1".into(),
                    icd: 0.0,
                    apply_buff: Some("proc_buff".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
        // `CompiledEffect::CastAction`: an `on_cast` proc (chance 1, icd 0) fires
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            actions.insert(
                "nuke".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut procs = BTreeMap::new();
            procs.insert(
                "free_nuke".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "1".into(),
                    icd: 0.0,
                    apply_buff: None,
                    effects: Vec::new(),
                    cast_action: Some("nuke".into()),
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs,
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "dps".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnHit,
                chance: "0.3".into(),
                icd: 0.0,
                apply_buff: Some("proc_buff".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            // Proc-cast only (never in the rotation), and deliberately inert:
            // no gain, no damage, so nothing on the free-cast path refreshes
            // the slot tail as a side effect.
            actions.insert(
                "ping".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "y".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "1".into(),
                    icd: 0.0,
                    apply_buff: None,
                    effects: Vec::new(),
                    cast_action: Some("ping".into()),
                    actions: None,
                },
            );
            procs.insert(
                "b_gated".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    // Reads sim state `a_cast` moves in this same batch.
                    chance: "casts.ping".into(),
                    icd: 0.0,
                    apply_buff: Some("y".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "crit_chance": 50.0 } }"#)
                    .unwrap();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 2 } ] }"#).unwrap();

            let mut actions = BTreeMap::new();
            actions.insert(
                "strike".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            let mut empowered = BTreeMap::new();
            empowered.insert("empowered".to_string(), 1.0);
            buffs.insert(
                "focus".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
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
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnHit,
                    chance: "1".into(),
                    icd: 100.0, // fires on the first hit only
                    apply_buff: Some("focus".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            procs.insert(
                "crit_proc".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCrit,
                    chance: "1".into(),
                    icd: 0.0,
                    apply_buff: Some("marker".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
    }

    /// P7b — expression-valued sim fields and their evaluation instants.
    mod expr_fields {
        use super::*;

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
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
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
                close(report.buffs["window"].uptime, 0.4),
                "got {}",
                report.buffs["window"].uptime
            );
            // …and the literal 4.0 gives the byte-identical answer.
            let (lit_def, lit_rot) = expr_duration_fixture(NumOrExpr::Num(4.0), 10.0);
            let lit_plan = sim_compile(&plan, &lit_def, &lit_rot).unwrap();
            let lit = run(&plan, &lit_plan, &build, &scenario, Mode::Expected).unwrap();
            assert_eq!(
                report.buffs["window"].uptime, lit.buffs["window"].uptime,
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
                close(report.buffs["window"].uptime, 0.2),
                "got {}",
                report.buffs["window"].uptime
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
                close(report.buffs["window"].uptime, 0.6),
                "got {} (0.8 = re-evaluated live, 0.4 = frozen at start)",
                report.buffs["window"].uptime
            );
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
                close(report.buffs["window"].uptime, 0.75),
                "got {} (0.95 = the refresh path read `buff.window` as 0)",
                report.buffs["window"].uptime
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
                    extra: Default::default(),
                    max: "50 + buff.boost * 50".into(),
                    regen_per_sec: "10".into(),
                },
            );
            let mut actions = BTreeMap::new();
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "boost".to_string(),
                BuffDef {
                    extra: Default::default(),
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
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "1".into(),
                    icd: 100.0,
                    apply_buff: Some("boost".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources,
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
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
    }

    /// P7c — the buff instance runtime: stacks and reapply policies.
    mod stacks {
        use super::*;

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
                close(report.buffs["charge"].avg_stacks, 2.7),
                "avg_stacks: got {} — want (1·2 + 2·2 + 3·16)/20 = 2.7",
                report.buffs["charge"].avg_stacks
            );
            assert!(
                close(report.buffs["charge"].uptime, 1.0),
                "buff_uptime: got {} — the shared clock resets every 2s against \
                 a 5s duration, so `charge` never falls off",
                report.buffs["charge"].uptime
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
        // The icd==cooldown coincidence here is deliberate and STAYS after
        // P7d. This fixture is about STACKS, not about how a buff gets
        // applied; it reaches for a proc because until P7d that was the only
        // mechanism, and `ActionDef::apply_buff` cannot express its varying
        // `chance` in any case. Keeping it also keeps the PROC application
        // path covered — `mod action_scoped` pins the action path separately.
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
                close(report.buffs["charge"].uptime, 0.45),
                "buff_uptime: got {} — all three instances share one clock and \
                 expire together at 4+5=9, so 9/20 = 0.45",
                report.buffs["charge"].uptime
            );
            assert!(
                close(report.buffs["charge"].avg_stacks, 1.05),
                "avg_stacks: got {} — want (1·2 + 2·2 + 3·5)/20 = 1.05",
                report.buffs["charge"].avg_stacks
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
                close(report.buffs["charge"].avg_stacks, 2.76),
                "avg_stacks: got {} — want (1·2 + 2·2 + 3·21)/25 = 2.76 (2.64 \
                 would mean the final open span went uncounted)",
                report.buffs["charge"].avg_stacks
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
        // This fixture applies its buff from a PROC, and stays that way after
        // P7d — note there is no icd==cooldown coincidence here at all (see
        // the parameters below). It is about STACKS/DoTs, not about how a buff
        // gets applied; it reaches for a proc because until P7d that was the
        // only mechanism, and `ActionDef::apply_buff` cannot express its
        // `chance` gate in any case. Keeping it also keeps the PROC
        // application path covered — `mod action_scoped` pins the action path
        // separately.
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
                close(report.buffs["charge"].avg_stacks, 0.5),
                "avg_stacks: got {} — want (1·1 + 2·4 + 1·1)/20 = 0.5 (0.45 \
                 would mean the NEWEST instance was evicted; 0.6 would mean the \
                 cap was ignored)",
                report.buffs["charge"].avg_stacks
            );
            assert!(
                close(report.buffs["charge"].uptime, 0.3),
                "buff_uptime: got {} — the last instance expires at 2+4=6, so 6/20 = 0.3",
                report.buffs["charge"].uptime
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
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(1000.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: nuke_stats,
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "charge_gen".into(),
                        when: None,
                    },
                    Rule {
                        extra: Default::default(),
                        action: "nuke".into(),
                        when: Some("stacks.charge >= 3".into()),
                    },
                    Rule {
                        extra: Default::default(),
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
            simdef.buffs.get_mut("charge").unwrap().tick_objective =
                Some(TickObjective::live("dot"));
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
        // `buff.X` and `buff_remaining.X` under MULTIPLE live instances — the
        // two symbols whose meaning only becomes observable once a buff can
        // stack, and which every other fixture in this file leaves ambiguous
        // by having exactly one instance.
        //
        // `add_independent`, cap 3 (never reached), duration 4, applications
        // at t=0 and t=1 only (icd 1 with a `"time <= 1"` chance gate;
        // `charge_gen` parked on a 100s cooldown supplies t=0 and the filler
        // supplies t=1):
        //   instance A applied t=0 → expires 4
        //   instance B applied t=1 → expires 5
        //
        // Two instant probe actions, each cast exactly once (1000s cooldown)
        // and gated `when: "time >= 2"`, read a symbol back as DAMAGE by
        // overriding `dmg` with it — at t=2, with 2 stacks live, `boost` is
        // 1 + 20/100 = 1.2:
        //   `probe_remaining`  dmg = buff_remaining.charge
        //       LONGEST: max(4−2, 5−2) = 3 → 3 × 1.2 = 3.6
        //       (soonest would be min(2, 3) = 2 → 2.4)
        //   `probe_flag`       dmg = buff.charge
        //       FLAG: 1 (any instance live) → 1 × 1.2 = 1.2
        //       (a count would be 2 → 2.4; `stacks.charge` is the counted one)
        // ------------------------------------------------------------------
        #[test]
        fn buff_flag_is_binary_and_buff_remaining_is_the_longest_window() {
            let plan = stack_plan();
            let build = stack_build();
            let mut simdef = stack_simdef(
                ReapplyPolicy::AddIndependent,
                3,
                4.0,
                100.0,
                "time <= 1",
                1.0,
            );
            for (name, expr) in [
                ("probe_remaining", "buff_remaining.charge"),
                ("probe_flag", "buff.charge"),
            ] {
                let mut stats = BTreeMap::new();
                stats.insert("dmg".to_string(), NumOrExpr::Expr(expr.into()));
                simdef.actions.insert(
                    name.to_string(),
                    ActionDef {
                        extra: Default::default(),
                        measure: None,
                        cast_time: "0".into(),
                        cooldown: NumOrExpr::Num(1000.0),
                        cost: BTreeMap::new(),
                        gain: BTreeMap::new(),
                        damage: Some(ActionDamage {
                            extra: Default::default(),
                            stats,
                        }),
                        apply_buff: Vec::new(),
                        effects: Vec::new(),
                    },
                );
            }
            let probe = |action: &str| Rule {
                extra: Default::default(),
                action: action.into(),
                when: Some("time >= 2".into()),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "charge_gen".into(),
                        when: None,
                    },
                    probe("probe_remaining"),
                    probe("probe_flag"),
                    Rule {
                        extra: Default::default(),
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

            assert_eq!(report.actions["probe_remaining"].casts, 1);
            assert!(
                close(report.actions["probe_remaining"].damage, 3.6),
                "buff_remaining.charge read back as {} — want the LONGEST live \
                 window, max(2, 3) = 3, × boost 1.2 = 3.6 (2.4 would mean the \
                 SOONEST expiry)",
                report.actions["probe_remaining"].damage
            );
            assert_eq!(report.actions["probe_flag"].casts, 1);
            assert!(
                close(report.actions["probe_flag"].damage, 1.2),
                "buff.charge read back as {} — want the binary flag 1 × boost \
                 1.2 = 1.2 (2.4 would mean it had drifted into a stack COUNT)",
                report.actions["probe_flag"].damage
            );
        }

        // ------------------------------------------------------------------
        // Conditions are driven at their FULL configured value while ANY
        // instance is live, and are NEVER scaled by the stack count — the
        // spec's most emphatic "deliberately not", and the one thing a
        // stacking buff's `conditions` map could plausibly be read to mean.
        // A condition is an uptime FRACTION, not a quantity: "3 stacks of
        // 70%-uptime" has no meaning.
        //
        // The `add_refresh_all` fixture from the 2.7 pin, with `charge` also
        // driving `focused = 1.0`. `charge` is live for the whole 20s (the
        // shared clock resets every 2s against a 5s duration), so the
        // computed blend is a flat
        //   condition_uptime[focused] = 1.0 × 20 / 20 = 1.0
        // A stack-scaled condition would instead integrate 1, 2 and 3 over
        // the very trajectory `avg_stacks` measures and report 2.7.
        //
        // NB this is observable on the REPORTING surface, not through damage:
        // `Plan` clamps every condition uptime into [0, 1] before folding, so
        // a scaled 3.0 would fold identically to 1.0 and leave the dps pins
        // silent. That clamp is why the assertion below is the one that has
        // to exist.
        // ------------------------------------------------------------------
        #[test]
        fn a_stacking_buffs_condition_is_not_scaled_by_the_stack_count() {
            let plan = stack_plan();
            let build = stack_build();
            let mut simdef = stack_simdef(ReapplyPolicy::AddRefreshAll, 3, 5.0, 2.0, "1", 2.0);
            simdef
                .buffs
                .get_mut("charge")
                .unwrap()
                .conditions
                .insert("focused".to_string(), 1.0);
            let sim_plan = sim_compile(&plan, &simdef, &stack_rotation()).unwrap();

            let report = run(
                &plan,
                &sim_plan,
                &build,
                &twenty_second_dummy(),
                Mode::Expected,
            )
            .unwrap();

            // The trajectory a scaled condition would follow, pinned here so
            // the contrast is explicit rather than implied.
            assert!(close(report.buffs["charge"].avg_stacks, 2.7));
            assert!(
                close(report.condition_uptime["focused"], 1.0),
                "condition_uptime[focused]: got {} — a buff drives its \
                 condition at its configured 1.0 for as long as ANY instance \
                 is live; 2.7 would mean the stack count had scaled it",
                report.condition_uptime["focused"]
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
                close(ev.buffs["charge"].avg_stacks, 2.7)
                    && close(mc.buffs["charge"].avg_stacks, 2.7),
                "EV {} vs MC {} — both must be 2.7",
                ev.buffs["charge"].avg_stacks,
                mc.buffs["charge"].avg_stacks
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

            let (a, b) = (ev.buffs["charge"].avg_stacks, mc.buffs["charge"].avg_stacks);
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
                extra: Default::default(),
                rolls: None,
                trigger: Trigger::OnHit,
                chance: "0.3".into(),
                icd: 4.0,
                apply_buff: Some("proc_buff".into()),
                effects: Vec::new(),
                cast_action: None,
                actions: None,
            });
            let sim_plan = sim_compile(&plan, &simdef, &filler_rotation()).unwrap();

            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

            assert!(
                close(report.buffs["proc_buff"].avg_stacks, 0.05),
                "got {} — one instance at a time, so this must equal the \
                 hand-worked 0.05 uptime",
                report.buffs["proc_buff"].avg_stacks
            );
            assert!(close(
                report.buffs["proc_buff"].avg_stacks,
                report.buffs["proc_buff"].uptime
            ));
        }
    }

    /// P7c-T2 — snapshot DoTs, `strongest`, and EV/MC agreement on
    /// instance TOTALS (the stack-count agreement lives in `stacks`).
    mod snapshot {
        use super::*;

        // ══════════════════════════════════════════════════════════════════
        // The shared snapshot fixture, hand-traced once here.
        //
        //   plan    `hit = dmg × boost`, `dot = dmg × 0.5 × boost × dot_scale`
        //           — TWO independent handles on the DoT rate, because the
        //           fixtures below need to move it two different ways.
        //           `boost` is a PRODUCT bucket, so a `+100` CONTRIBUTION
        //           doubles it (what a buff can do mid-fight); `dot_scale` is
        //           a stat a PHASE can override, and it appears in the `dot`
        //           stage ONLY. That last part is deliberate: a phase `stats`
        //           override wins over an action's `damage.stats` overlay, so
        //           overriding a stat the HIT reads would un-zero the
        //           filler's damage (it did, in the first draft of this
        //           fixture — 1200 of hit damage where the fixture promises
        //           0).
        //   build   `dmg = 100`, `dot_scale = 1` → the dot objective is 50/s
        //           at boost 1. R = 50 is the per-instance snapshot rate
        //           every pin below is quoted in.
        //   opener  cast_time 0, cooldown 1000 — one instant cast at t=0,
        //           whose only job is to put an application AT t=0.
        //   filler  cast_time 1, `damage.stats { dmg: 0 }` — one cast per
        //           second completing at t=1,2,…, and its hit damage is ZERO
        //           by construction. `damage_objective` is `hit`, so
        //           `total_damage` IS the DoT total and every pin is a pure
        //           DoT number. (The overlay is per-cast: it zeroes the
        //           HIT's `dmg`, never the buff's tick objective, which reads
        //           the un-overlaid effective build.)
        //   poison_proc  on_cast, icd 0, `chance` per fixture — applies the
        //           buff under test. icd 0 + chance 1 = one application per
        //           cast: t=0 (opener) and t=1…20 (filler).
        // This fixture applies its buff from a PROC, and stays that way after
        // P7d — note there is no icd==cooldown coincidence here at all (see
        // the parameters below). It is about STACKS/DoTs, not about how a buff
        // gets applied; it reaches for a proc because until P7d that was the
        // only mechanism, and `ActionDef::apply_buff` cannot express its
        // `chance` gate in any case. Keeping it also keeps the PROC
        // application path covered — `mod action_scoped` pins the action path
        // separately.
        // ══════════════════════════════════════════════════════════════════

        fn dot_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg", "dot_scale"],
                     "buckets": { "boost": { "fold": "product" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * boost" },
                                   { "name": "dot", "expr": "dmg * 0.5 * boost * dot_scale" } ],
                     "objectives": ["hit", "dot"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        fn dot_build() -> BuildState {
            serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "dot_scale": 1.0 } }"#).unwrap()
        }

        /// The fixture described above. `tick` is the buff's
        /// `tick_objective`, so one call site can run the SAME layout live
        /// and snapshotted and pin the difference.
        fn dot_simdef(
            on_reapply: ReapplyPolicy,
            max_stacks: u32,
            duration: f64,
            chance: &str,
            tick: Option<TickObjective>,
        ) -> SimDef {
            let mut actions = BTreeMap::new();
            actions.insert(
                "opener".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "0".into(),
                    cooldown: NumOrExpr::Num(1000.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut filler_stats = BTreeMap::new();
            filler_stats.insert("dmg".to_string(), NumOrExpr::Num(0.0));
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: filler_stats,
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "poison".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(duration),
                    max_stacks,
                    on_reapply,
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: tick,
                },
            );
            let mut procs = BTreeMap::new();
            procs.insert(
                "poison_proc".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: chance.into(),
                    icd: 0.0,
                    apply_buff: Some("poison".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            }
        }

        fn dot_rotation() -> Rotation {
            Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "opener".into(),
                        when: None,
                    },
                    Rule {
                        extra: Default::default(),
                        action: "filler".into(),
                        when: None,
                    },
                ],
            }
        }

        /// Add an `empower` buff that DOUBLES the tick objective (a `+100`
        /// contribution to the `product` bucket `boost`), applied from t=10
        /// onwards and lasting past the end of the fight.
        ///
        /// Proc ORDER matters at t=10, and is deterministic: `SimDef::procs`
        /// is a `BTreeMap` and `sim::compile` preserves its (name) order, so
        /// `empower_proc` rolls BEFORE `poison_proc` at every cast. The t=10
        /// poison application therefore lands in a world where `empower` is
        /// already live, and snapshots the DOUBLED rate.
        fn with_empower(simdef: &mut SimDef) {
            simdef.buffs.insert(
                "empower".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(100.0),
                    contributions: vec![Contribution {
                        bucket: "boost".into(),
                        value: 100.0,
                        event: None,
                        condition: None,
                    }],
                    ..BuffDef::default()
                },
            );
            simdef.procs.insert(
                "empower_proc".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "time >= 10".into(),
                    icd: 0.0,
                    apply_buff: Some("empower".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
        }

        /// Give `poison` itself a `+100` contribution to `boost` — the
        /// product bucket its OWN tick objective reads. Each live stack
        /// then doubles what the NEXT application captures, which is what
        /// makes the capture INSTANT (before or after this application's
        /// refold) observable at all.
        fn with_self_feeding_contribution(simdef: &mut SimDef) {
            simdef.buffs.get_mut("poison").unwrap().contributions = vec![Contribution {
                bucket: "boost".into(),
                value: 100.0,
                event: None,
                condition: None,
            }];
        }

        /// Every fixture here zeroes the filler's `dmg`, so `total_damage`
        /// is the DoT alone and each pin is a pure DoT number. Asserted
        /// rather than assumed, in every test: a phase `stats` override
        /// that names a stat the HIT reads silently un-zeroes it (see the
        /// fixture header — an early draft of this module did exactly
        /// that), and the failure would otherwise show up as an
        /// off-by-hundreds DoT total with no hint of where it came from.
        fn assert_pure_dot(report: &SimReport) {
            assert!(
                close(report.actions["filler"].damage, 0.0),
                "the filler's hit damage must be 0 — got {}, so `total_damage` \
                 is NOT the DoT alone. A phase `stats` override of a stat the \
                 `hit` stage reads (`dmg`) beats the action's `damage.stats` \
                 overlay; override `dot_scale` instead.",
                report.actions["filler"].damage
            );
        }

        // ------------------------------------------------------------------
        // The poison-cadence pin: a snapshot DoT accrues one instance-second
        // of its CAPTURED rate per instance-second live, and the stack count
        // is inherent in the sum of the instances' rates — never multiplied
        // in a second time.
        //
        // `add_independent`, UNBOUNDED (max_stacks 0), duration 4, chance 1
        // with no icd, over a 20s fight: one application per cast, at
        // t=0 (opener) and t=1…20 (filler). Nothing moves the rate, so every
        // instance captures the same R = dmg × 0.5 × boost = 100 × 0.5 × 1
        // = 50/s.
        //
        // Instance-seconds (an instance applied at `a` is live on
        // `[a, a+4)`, clipped at the 20s end):
        //   a = 0…16   17 instances × 4s = 68
        //   a = 17,18,19            3 + 2 + 1 = 6
        //   a = 20     lands exactly at the end and integrates to ZERO
        //   ∫ stacks dt = 74  →  avg_stacks = 74/20 = 3.7
        //   DoT total   = 74 × R = 74 × 50 = 3700, dps = 185
        //
        // The mutation this catches beyond the cadence: a total tick rate of
        // Σ(snapshot rates) × len — the "× stack count" a LIVE objective
        // legitimately needs — would integrate R × ∫ stacks² dt instead.
        // The count is 1, 2, 3 on the first three seconds and a flat 4 from
        // t=3 on, so ∫ stacks² dt = 1 + 4 + 9 + 16×17 = 286 → 14300.
        // ------------------------------------------------------------------
        #[test]
        fn a_snapshot_dot_accrues_74_instance_seconds_of_its_captured_rate() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::AddIndependent,
                0,
                4.0,
                "1",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();

            let report = run(
                &plan,
                &sim_plan,
                &build,
                &twenty_second_dummy(),
                Mode::Expected,
            )
            .unwrap();

            assert_pure_dot(&report);
            assert!(
                close(report.buffs["poison"].avg_stacks, 3.7),
                "avg_stacks: got {} — want 74/20 = 3.7",
                report.buffs["poison"].avg_stacks
            );
            assert!(
                close(report.total.total_damage, 3700.0),
                "DoT total: got {} — want 74 instance-seconds × R=50 = 3700 \
                 (14300 would mean the summed rate was ALSO multiplied by the \
                 stack count)",
                report.total.total_damage
            );
            assert!(
                close(report.total.dps, 185.0),
                "dps: got {} — want 3700/20 = 185",
                report.total.dps
            );
        }

        // ------------------------------------------------------------------
        // Snapshot IMMUNITY, against its live control — the pair that proves
        // the flag changes exactly this and nothing else. Same fixture as the
        // 74-instance-second pin, plus `empower` (× 2 on the tick objective)
        // live from t=10.
        //
        // SNAPSHOT — an instance ticks the rate it captured, for ever:
        //   a = 0…9   (10 instances) × 4s × R    = 40 R
        //   a = 10…16  (7 instances) × 4s × 2R   = 56 R
        //   a = 17,18,19  (3+2+1 = 6s) × 2R      = 12 R
        //   total = 108 R = 108 × 50 = 5400
        //
        // LIVE — every live instance ticks the CURRENT rate from t=10 on, so
        // the split is by TIME rather than by instance:
        //   ∫ stacks dt over [0,10)  = 7×4 + 3 + 2 + 1 = 34
        //   ∫ stacks dt over [10,20] = 74 − 34         = 40
        //   total = 34 R + 40 × 2R = 114 R = 5700
        //
        // The two differ by exactly 114 R − 108 R = 6 R — the doubling that
        // instances applied before t=10 collect from t=10 on under live
        // semantics and refuse under snapshot ones. BOTH totals are pinned:
        // either one alone could equally be produced by a fixture that
        // simply never changed the rate.
        // ------------------------------------------------------------------
        #[test]
        fn a_snapshot_instance_ignores_a_later_rate_change_and_the_live_one_does_not() {
            let plan = dot_plan();
            let build = dot_build();
            let run_with = |tick: TickObjective| {
                let mut simdef = dot_simdef(ReapplyPolicy::AddIndependent, 0, 4.0, "1", Some(tick));
                with_empower(&mut simdef);
                let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
                let report = run(
                    &plan,
                    &sim_plan,
                    &build,
                    &twenty_second_dummy(),
                    Mode::Expected,
                )
                .unwrap();
                assert_pure_dot(&report);
                report
            };

            let snap = run_with(TickObjective::snapshot("dot"));
            let live = run_with(TickObjective::live("dot"));

            assert!(
                close(snap.total.total_damage, 5400.0),
                "snapshot total: got {} — want 40R + 56R + 12R = 108 × 50 = \
                 5400 (5700 would mean instances applied before t=10 were \
                 retroactively doubled)",
                snap.total.total_damage
            );
            assert!(
                close(live.total.total_damage, 5700.0),
                "live total: got {} — want 34R + 40×2R = 114 × 50 = 5700 \
                 (5400 would mean the LIVE rate stopped being re-evaluated)",
                live.total.total_damage
            );
            // Same instance trajectory in both: only the RATE differs.
            assert!(
                close(snap.buffs["poison"].avg_stacks, 3.7)
                    && close(live.buffs["poison"].avg_stacks, 3.7),
                "the flag must not move the cadence: {} vs {}",
                snap.buffs["poison"].avg_stacks,
                live.buffs["poison"].avg_stacks
            );
        }

        // ------------------------------------------------------------------
        // Design decision, pinned: under `add_refresh_all` an existing
        // instance's EXPIRY moves to the shared clock but its SNAPSHOT RATE
        // does not. A snapshot rate is captured once, with the window it was
        // captured for, and "ticks unchanged to expiry" means unchanged even
        // when that expiry is later pushed out.
        //
        // `add_refresh_all`, cap 3, duration 100 (nothing expires inside the
        // fight), snapshot dot, applications at t=1 and t=6 only
        // (`chance: "or(time == 1, time == 6)"`). A 10s fight in TWO phases:
        // `early` (5s, the build's `dot_scale` = 1) then `late` (5s, phase
        // stat override `dot_scale` = 2) — so the tick objective is 50/s
        // before t=5 and 100/s after.
        //
        //   t=1  push instance A, rate R = 50            → total rate 50
        //   t=5  phase boundary: the objective is now 2R. A is IMMUNE.
        //   t=6  push instance B, rate 2R = 100, and reset BOTH expiries to
        //        106 — A keeps its 50                    → total rate 150
        //   total = 50 × (6−1) + 150 × (10−6) = 250 + 600 = 850
        //
        // Mutation contrasts: re-snapshotting a refreshed instance would read
        // 100 + 100 = 200/s after t=6 → 1050; live semantics → 200 + 100 +
        // 800 = 1100; Σ(rates) × len → 250 + 1200 = 1450.
        // ------------------------------------------------------------------
        #[test]
        fn add_refresh_all_moves_the_expiry_but_never_the_snapshot_rate() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::AddRefreshAll,
                3,
                100.0,
                "or(time == 1, time == 6)",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let scenario: Scenario = serde_json::from_str(
                r#"{ "phases": [ { "name": "early", "weight": 5 },
                                 { "name": "late",  "weight": 5,
                                   "stats": { "dot_scale": 2.0 } } ] }"#,
            )
            .unwrap();

            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

            assert_pure_dot(&report);
            assert!(
                close(
                    report.buffs["poison"].avg_stacks,
                    (1.0 * 5.0 + 2.0 * 4.0) / 10.0
                ),
                "avg_stacks: got {} — 1 instance on [1,6), 2 on [6,10]",
                report.buffs["poison"].avg_stacks
            );
            assert!(
                close(report.total.total_damage, 850.0),
                "DoT total: got {} — want 50×5 + 150×4 = 850 (1050 would mean \
                 the refreshed instance re-snapshotted; 1100 live semantics)",
                report.total.total_damage
            );
        }

        // ------------------------------------------------------------------
        // The capture INSTANT: a snapshot rate is taken BEFORE this
        // application's own refold, against the world the instance is
        // landing on — not the world it creates. Everywhere else in the
        // module the distinction is invisible, because no other snapshot
        // buff has `contributions`; here `poison` feeds its own tick
        // objective (a `+100` contribution to `boost`, the product bucket
        // `dot` reads), so each live stack doubles what the next
        // application captures.
        //
        // Unbounded `add_independent`, duration 100 (nothing expires inside
        // the fight), applications at t=1 and t=2 only, over 10s:
        //   t=1  0 stacks live → boost 1 → capture R  =  50   rate  50
        //   t=2  1 stack  live → boost 2 → capture 2R = 100   rate 150
        //   total = 50 × (2−1) + 150 × (10−2) = 50 + 1200 = 1250
        //
        // Capturing AFTER the refold instead — i.e. letting the instance
        // see its own contribution — reads 100 at t=1 (1 stack) and 150 at
        // t=2 (2 stacks) for 100 + 250×8 = 2100. That is the mutation this
        // test exists for: the semantics were documented on `apply_buff`
        // and pinned by nothing, exactly as `duration`'s two application
        // paths would have been without their own test.
        //
        // This is also the config-author-facing warning made concrete: a
        // buff whose contributions feed its own tick objective SELF-
        // AMPLIFIES on reapplication, and the amplification is one
        // application behind.
        // ------------------------------------------------------------------
        #[test]
        fn a_snapshot_rate_is_captured_before_its_own_application_folds_in() {
            let plan = dot_plan();
            let build = dot_build();
            let mut simdef = dot_simdef(
                ReapplyPolicy::AddIndependent,
                0,
                100.0,
                "or(time == 1, time == 2)",
                Some(TickObjective::snapshot("dot")),
            );
            with_self_feeding_contribution(&mut simdef);
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();

            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

            assert_pure_dot(&report);
            assert!(
                close(report.buffs["poison"].avg_stacks, 1.7),
                "avg_stacks: got {} — 1 instance on [1,2), 2 on [2,10]",
                report.buffs["poison"].avg_stacks
            );
            assert!(
                close(report.total.total_damage, 1250.0),
                "DoT total: got {} — want 50×1 + 150×8 = 1250, each rate \
                 captured against the stacks live BEFORE its own application \
                 (2100 would mean the capture happened after the refold, with \
                 the new instance already folded in)",
                report.total.total_damage
            );
        }

        // ------------------------------------------------------------------
        // `add_refresh_all` AT THE CAP with a snapshot tick: the application
        // is discarded RATE-WISE (no instance is added, so nothing captures)
        // while the shared clock still resets. A capped stack can therefore
        // ride an old snapshot indefinitely — the most surprising corner of
        // this policy, and the one arm of the fold gate the other fixtures
        // never reach.
        //
        // Cap 1 (so the second application is at the cap), duration 8,
        // applications at t=1 and t=6, two 5s phases (`dot_scale` 1 → 2):
        //   t=1  push instance A, rate R = 50, expires 9
        //   t=5  phase boundary: the objective is now 2R. A is immune.
        //   t=6  AT CAP — nothing is pushed, so the 2R the application would
        //        have captured is DISCARDED; A's expiry moves to 14
        //   total = 50 × (10−1) = 450
        //
        // Contrasts, all measured:
        //   650  the at-cap application re-captured onto the standing
        //        instance AND refolded — A would tick 50 on [1,6) and 100
        //        on [6,10]
        //   450  (silently correct-looking!) the same re-capture WITHOUT a
        //        refold: `tick_rate` is cached, and at the cap nothing
        //        refolds it, so the total never moves. Only the
        //        `snapshot buff's total tick rate moved without a refold`
        //        assertion in `apply_buff` catches that one — it fires in
        //        debug, and this test passes in release. That assertion is
        //        the guard here, not the number.
        //   400  the shared clock did NOT reset at the cap: A falls off at
        //        its original 9. Caught one assertion earlier, by
        //        `avg_stacks` reading 0.8 against 0.9.
        //
        // NB the fold gate's `AddRefreshAll => false` arm is deliberately
        // NOT a mutation target: flipping it to `true` runs a flush/refold
        // that recomputes the SAME sum, so it is an equivalent mutation.
        // `false` there is a cost decision, not a correctness one — what
        // needs pinning is the SEMANTIC above, which is what this test does.
        // ------------------------------------------------------------------
        #[test]
        fn add_refresh_all_at_the_cap_discards_the_rate_and_keeps_the_old_snapshot() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::AddRefreshAll,
                1,
                8.0,
                "or(time == 1, time == 6)",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let scenario: Scenario = serde_json::from_str(
                r#"{ "phases": [ { "name": "early", "weight": 5 },
                                 { "name": "late",  "weight": 5,
                                   "stats": { "dot_scale": 2.0 } } ] }"#,
            )
            .unwrap();

            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

            assert_pure_dot(&report);
            assert!(
                close(report.buffs["poison"].avg_stacks, 0.9),
                "avg_stacks: got {} — one instance on [1,10], never two",
                report.buffs["poison"].avg_stacks
            );
            assert!(
                close(report.total.total_damage, 450.0),
                "DoT total: got {} — want 50 × 9 = 450: at the cap the \
                 incoming 2R is discarded but the shared clock still resets \
                 (650 would mean it re-captured and refolded; 400 that the \
                 expiry did not move)",
                report.total.total_damage
            );
        }

        // ------------------------------------------------------------------
        // `refresh` — the 0.2.0 binary buff — with a SNAPSHOT tick objective:
        // the single instance is REPLACED on every application, so it
        // re-captures the rate, up or down. This is the fold-gate case a
        // count-only gate misses: the count stands still at 1 while the
        // summed rate moves.
        //
        // Duration 8, applications at t=1 and t=6, the same two 5s phases
        // whose `dot_scale` override sets what each application captures:
        //   RISING  (1 → 2): 50 × (6−1) + 100 × (10−6) = 250 + 400 = 650
        //   FALLING (2 → 1): 100 × 5    + 50 × 4       = 500 + 200 = 700
        //
        // Mutations:
        //   650 → 450  the instance-SET change went unrefolded, so the
        //              integrator keeps billing the old 50/s to the end.
        //              (In a debug build the `snapshot buff's total tick
        //              rate moved without a refold` assertion in
        //              `apply_buff` fires first; 450 is what a release
        //              build reads.)
        //   700 → 800  `refresh` kept the incumbent's higher rate — i.e. it
        //              had quietly become `strongest`, which is exactly the
        //              number that policy's FALLING pin reads on this very
        //              layout
        // ------------------------------------------------------------------
        #[test]
        fn refresh_recaptures_the_snapshot_rate_on_every_reapplication() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::Refresh,
                1,
                8.0,
                "or(time == 1, time == 6)",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let total = |early: f64, late: f64| {
                let scenario: Scenario = serde_json::from_str(&format!(
                    r#"{{ "phases": [ {{ "name": "early", "weight": 5,
                                         "stats": {{ "dot_scale": {early} }} }},
                                      {{ "name": "late",  "weight": 5,
                                         "stats": {{ "dot_scale": {late} }} }} ] }}"#
                ))
                .unwrap();
                let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
                assert_pure_dot(&report);
                report.total.total_damage
            };

            let rising = total(1.0, 2.0);
            let falling = total(2.0, 1.0);
            assert!(
                close(rising, 650.0),
                "rising: got {rising} — want 50×5 + 100×4 = 650 (450 would \
                 mean the re-capture never reached the tick integrator)"
            );
            assert!(
                close(falling, 700.0),
                "falling: got {falling} — want 100×5 + 50×4 = 700; `refresh` \
                 replaces UNCONDITIONALLY (800 would mean it kept the higher \
                 rate, i.e. behaved like `strongest`)"
            );
        }

        // ------------------------------------------------------------------
        // `add_independent` AT THE CAP with a snapshot tick: the eviction
        // swaps one captured rate for another while the COUNT stands still —
        // the second fold-gate case, and the one where WHICH instance is
        // evicted becomes visible in the damage total rather than only in
        // `avg_stacks`.
        //
        // Cap 2, duration 8, applications at t=1, t=2 and t=6; two 5s phases
        // (`dot_scale` 1 → 2, so applications before t=5 capture R=50 and
        // after it 2R=100):
        //   t=1  push A(rate 50, expires 9)                 → rate  50
        //   t=2  push B(rate 50, expires 10) — now AT CAP    → rate 100
        //   t=6  at the cap: evict the EARLIEST-EXPIRING (A, at 9), push
        //        C(rate 100, expires 14)                     → rate 150
        //   total = 50×(2−1) + 100×(6−2) + 150×(10−6)
        //         = 50 + 400 + 600 = 1050
        //
        // Mutations:
        //   1050 → 850   the eviction went unrefolded (rate stuck at 100)
        //   1050 → 1000  evicting the NEWEST (B) instead of the
        //                earliest-expiring: the summed rate is the same 150
        //                (A and B both captured 50), but A then falls off at
        //                its own expiry of 9 and the last second ticks 100
        //                rather than 150.
        // ------------------------------------------------------------------
        #[test]
        fn add_independent_refolds_the_summed_rate_when_it_evicts_at_the_cap() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::AddIndependent,
                2,
                8.0,
                "or(time == 1, or(time == 2, time == 6))",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let scenario: Scenario = serde_json::from_str(
                r#"{ "phases": [ { "name": "early", "weight": 5 },
                                 { "name": "late",  "weight": 5,
                                   "stats": { "dot_scale": 2.0 } } ] }"#,
            )
            .unwrap();

            let report = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();

            assert_pure_dot(&report);
            assert!(
                close(report.total.total_damage, 1050.0),
                "DoT total: got {} — want 50 + 400 + 600 = 1050 (850 would \
                 mean the eviction at the cap never refolded the summed rate)",
                report.total.total_damage
            );
        }

        // ------------------------------------------------------------------
        // `strongest` (PoE2 ignite): an application replaces the incumbent
        // only when its snapshot rate is STRICTLY higher; a loser is
        // discarded whole — rate AND expiry.
        //
        // One layout, three scenarios. Duration 8, applications at t=1 and
        // t=6 (`chance: "or(time == 1, time == 6)"`), a 10s fight in two 5s
        // phases whose `dot_scale` override sets what each application
        // captures (NOT `dmg` — see the fixture header: a phase override of
        // a stat the `hit` stage reads would un-zero the filler's damage):
        //
        //   RISING  (1 → 2): t=1 captures R=50 (expires 9); at t=6 the
        //     incoming 2R=100 wins and REPLACES — new window, expires 14.
        //     total = 50×(6−1) + 100×(10−6) = 250 + 400 = 650
        //   FALLING (2 → 1): t=1 captures 2R=100 (expires 9); at t=6 the
        //     incoming 50 LOSES and is discarded — the incumbent keeps both
        //     its rate and its expiry, so it falls off at 9 and the last
        //     second of the fight has no DoT at all.
        //     total = 100 × (9−1) = 800
        //   TIE     (no change):  both applications capture R=50. "Strictly
        //     higher" fails on a tie, so the t=6 application loses and the
        //     window still ends at 9.
        //     total = 50 × (9−1) = 400
        //
        // Each pin discriminates one mutation the others do not:
        //   FALLING = 900  → a losing application refreshed the expiry
        //                    (window to 14, clipped at the 10s end)
        //   FALLING = 700  → the loser replaced anyway (100×5 + 50×4)
        //   TIE     = 450  → the comparison is `>=`, not `>`
        //   RISING  = 400  → the winner did NOT replace either, leaving the
        //                    t=1 instance to tick 50 to its own expiry at 9
        // ------------------------------------------------------------------
        #[test]
        fn strongest_replaces_only_on_a_strictly_higher_rate_and_a_loser_keeps_nothing() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::Strongest,
                1,
                8.0,
                "or(time == 1, time == 6)",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let two_phases = |early: &str, late: &str| -> Scenario {
                serde_json::from_str(&format!(
                    r#"{{ "phases": [ {{ "name": "early", "weight": 5, "stats": {early} }},
                                      {{ "name": "late",  "weight": 5, "stats": {late} }} ] }}"#
                ))
                .unwrap()
            };
            let total = |scenario: &Scenario| {
                let report = run(&plan, &sim_plan, &build, scenario, Mode::Expected).unwrap();
                assert_pure_dot(&report);
                report.total.total_damage
            };

            let rising = total(&two_phases(
                r#"{ "dot_scale": 1.0 }"#,
                r#"{ "dot_scale": 2.0 }"#,
            ));
            let falling = total(&two_phases(
                r#"{ "dot_scale": 2.0 }"#,
                r#"{ "dot_scale": 1.0 }"#,
            ));
            let tie = total(&two_phases(
                r#"{ "dot_scale": 1.0 }"#,
                r#"{ "dot_scale": 1.0 }"#,
            ));

            assert!(
                close(rising, 650.0),
                "rising: got {rising} — want 50×5 + 100×4 = 650 (400 would \
                 mean the stronger application did not replace)"
            );
            assert!(
                close(falling, 800.0),
                "falling: got {falling} — want 100×8 = 800, the incumbent's \
                 ORIGINAL window (900 would mean the losing application \
                 refreshed the expiry; 700 that it replaced anyway)"
            );
            assert!(
                close(tie, 400.0),
                "tie: got {tie} — want 50×8 = 400; a tie is not STRICTLY \
                 higher, so the incumbent stands (450 would mean `>=`)"
            );
        }

        // ------------------------------------------------------------------
        // `Sim::eval_objective`'s `world` parameter, pinned directly at the
        // unit level (a parameter whose interesting branch no test reaches
        // is the documented-but-unpinned hazard the P7 record names).
        //
        // Against the same fixture: `dot = dmg × 0.5 × boost × dot_scale`,
        // so the effective build (dmg 100) in the live phase reads 50. A
        // snapshot world REPLACES BOTH halves (P8c — one world): its build
        // (dmg 300 → 150 in the same phase) AND its phase (a `dot_scale: 2`
        // phase override → 300) — either half read live instead of from
        // the snapshot breaks one of the two literals.
        // ------------------------------------------------------------------
        #[test]
        fn eval_objective_reads_the_snapshot_world_when_given_one_and_the_live_one_when_not() {
            let plan = dot_plan();
            let build = dot_build();
            let simdef = dot_simdef(
                ReapplyPolicy::AddIndependent,
                0,
                4.0,
                "0",
                Some(TickObjective::snapshot("dot")),
            );
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 10 } ] }"#).unwrap();
            let scratch = SimScratch::new(&plan, &sim_plan);
            let mut sim = Sim::new(
                &plan, &sim_plan, &build, &scenario, 10.0, scratch, None, // EV mode
            )
            .unwrap();
            let obj = sim_plan.buffs[0]
                .tick_objective
                .expect("the fixture's only buff ticks `dot`")
                .objective;

            assert!(
                close(sim.eval_objective(obj, None).unwrap(), 50.0),
                "`None` must read the live world: dmg 100 × 0.5 = 50"
            );

            let overlaid: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 300.0, "dot_scale": 1.0 } }"#).unwrap();
            let same_phase = WorldSnapshot {
                build: overlaid.clone(),
                phase: sim.effective_phase.clone(),
                damage: None,
            };
            assert!(
                close(sim.eval_objective(obj, Some(&same_phase)).unwrap(), 150.0),
                "the snapshot's BUILD must be read: dmg 300 × 0.5 = 150 (50 \
                 would mean the world was accepted and ignored)"
            );

            let boosted_phase: Phase = serde_json::from_str(
                r#"{ "name": "p", "weight": 10, "stats": { "dot_scale": 2.0 } }"#,
            )
            .unwrap();
            let other_phase = WorldSnapshot {
                build: overlaid,
                phase: boosted_phase,
                damage: None,
            };
            assert!(
                close(sim.eval_objective(obj, Some(&other_phase)).unwrap(), 300.0),
                "the snapshot's PHASE must be read too (P8c): dmg 300 × 0.5 \
                 × dot_scale 2 = 300 (150 would mean the phase half is \
                 still live)"
            );
        }

        // ------------------------------------------------------------------
        // EV/MC agreement on instance TOTALS (the P6 discipline, and the P7
        // spec's gate for snapshot DoTs). Unbounded `add_independent`,
        // duration 4, snapshot dot, applied by an `on_hit` proc at chance 0.5
        // against the filler's 1 hit/s cadence over 40s.
        //
        // EV, exactly — the accumulator crosses on every second hit, so
        // applications land at t=2,4,…,40 and an instance applied at `a` is
        // live on `[a, a+4)`:
        //   a = 2…36 (18 instances) × 4s = 72, a = 38 → 2, a = 40 → 0
        //   ∫ stacks dt = 74  →  avg_stacks 1.85 (the same 74 the P7c-T1
        //   steady-state pin hand-works), total = 74 × R = 74 × 50 = 3700.
        //
        // MC rolls each hit exactly and ramps in faster (its first
        // application can land at t=1), so its expected integral is 75 →
        // 3750, 1.35% above EV. The gate is 3%: a real divergence of the
        // kind the P6 review's ICD-bound regime showed (+48%) fails it.
        //
        // The rate never moves here, so EV/MC agreement on the TOTAL is
        // agreement on the instance trajectory times a constant — which is
        // exactly the claim, and it is checked in the mode where the
        // instances are rolled rather than accumulated.
        // ------------------------------------------------------------------
        #[test]
        fn snapshot_dot_totals_agree_between_ev_and_monte_carlo() {
            let plan = dot_plan();
            let build = dot_build();
            let scenario: Scenario =
                serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 40 } ] }"#).unwrap();
            let mut simdef = dot_simdef(
                ReapplyPolicy::AddIndependent,
                0,
                4.0,
                "0.5",
                Some(TickObjective::snapshot("dot")),
            );
            simdef.procs.get_mut("poison_proc").unwrap().trigger = Trigger::OnHit;
            let sim_plan = sim_compile(&plan, &simdef, &dot_rotation()).unwrap();
            let mc = || {
                run(
                    &plan,
                    &sim_plan,
                    &build,
                    &scenario,
                    Mode::MonteCarlo {
                        iterations: 2_000,
                        seed: 7,
                    },
                )
                .unwrap()
            };

            let ev = run(&plan, &sim_plan, &build, &scenario, Mode::Expected).unwrap();
            assert_pure_dot(&ev);
            let a = ev.total.total_damage;
            assert!(
                close(a, 3700.0) && close(ev.buffs["poison"].avg_stacks, 1.85),
                "EV total {a} / avg_stacks {} — want the hand-worked 74 \
                 instance-seconds × R=50 = 3700 at 74/40 = 1.85",
                ev.buffs["poison"].avg_stacks
            );

            let first = mc();
            let b = first.total.total_damage;
            let rel_err = (b - a).abs() / a;
            assert!(
                rel_err < 0.03,
                "EV total {a} vs MC mean {b}, relative error {rel_err} — the \
                 modes must agree on the snapshot-DoT total"
            );
            // Same seed, byte-identical: no RNG draw was reordered.
            assert_eq!(
                b,
                mc().total.total_damage,
                "same-seed MC must be byte-identical"
            );
        }
    }
    // ══════════════════════════════════════════════════════════════════
    // P7d — action-scoped effects: `ActionDef::apply_buff` and the
    // `ProcDef::actions` trigger filter.
    //
    // THREE plans, each earning its place — every pin below names which:
    //   `scoped_plan`  hit = dmg × boost,  dot = dmg × 0.5 × dot_scale.
    //       `boost` is a PRODUCT bucket, so one `+100` contribution
    //       DOUBLES the hit; `dot` deliberately does NOT read it, so a
    //       capture's arithmetic is independent of what is live. No
    //       events, so EV and MC agree on damage by construction and the
    //       MC fixture's RNG stream is attributable to proc rolls alone.
    //       build: dmg = 100, dot_scale = 1 → hit 100, dot rate R = 50/s.
    //   `boosted_dot_plan`  the same, except dot = dmg × 0.5 × boost — the
    //       one shape in which a `boost` contribution moves a CAPTURED
    //       rate, which is what the frozen-overlay pins need.
    //   `crit_plan`  hit = dmg × event_factors, branched, one `crit` event
    //       — the only plan here that samples, needed for `on_crit`
    //       triggers and for making an RNG draw observable.
    //
    // Fixture style: EXHAUSTIVE struct literals, matching the other ~50 in
    // this file rather than `..Default::default()`. The point is the one
    // `simdef::tests::action_def_serde_defaults_and_derived_default_agree_field_for_field`
    // makes — a new config field should force every fixture to be re-read,
    // and `..default()` quietly opts a fixture out of that.
    // ══════════════════════════════════════════════════════════════════
    mod action_scoped {
        use super::*;

        fn scoped_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg", "dot_scale"],
                     "conditions": ["focused"],
                     "buckets": { "boost": { "fold": "product" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * boost" },
                                   { "name": "dot", "expr": "dmg * 0.5 * dot_scale" } ],
                     "objectives": ["hit", "dot"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        fn scoped_build() -> BuildState {
            serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "dot_scale": 1.0 } }"#).unwrap()
        }

        /// As `scoped_plan`, but the DoT objective reads the `boost`
        /// bucket — so a `+100` contribution doubles the rate a snapshot
        /// instance captures. That is the only way to observe WHICH build
        /// a capture read, which is what the frozen-overlay pins are
        /// about.
        fn boosted_dot_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg"],
                     "buckets": { "boost": { "fold": "product" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * boost" },
                                   { "name": "dot", "expr": "dmg * 0.5 * boost" } ],
                     "objectives": ["hit", "dot"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        fn boosted_dot_build() -> BuildState {
            serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap()
        }

        /// As `boosted_dot_plan`, but the DoT rate ALSO reads a
        /// `marked` CONDITION. That is the third axis: a capture reads a
        /// frozen BUILD (so `boost` is frozen) against the LIVE effective
        /// phase (so `marked` is not). Nothing else can tell the two
        /// apart, which is why this fixture exists.
        fn marked_dot_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg"],
                     "conditions": ["marked"],
                     "buckets": { "boost": { "fold": "product" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * boost" },
                                   { "name": "dot",
                                     "expr": "dmg * 0.5 * boost * (1 + marked)" } ],
                     "objectives": ["hit", "dot"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        /// Drives BOTH axes at once: `+100` to the product bucket `boost`
        /// (a contribution — frozen for a same-list capture) and the
        /// condition `marked = 1.0` (live for one). Same magnitude on
        /// each, so `marked_dot_plan`'s rate doubles per axis and the two
        /// are individually readable off the total.
        fn mark_buff(duration: f64) -> BuffDef {
            BuffDef {
                extra: Default::default(),
                duration: NumOrExpr::Num(duration),
                contributions: vec![Contribution {
                    bucket: "boost".into(),
                    value: 100.0,
                    event: None,
                    condition: None,
                }],
                conditions: [("marked".to_string(), 1.0)].into_iter().collect(),
                tick_objective: None,
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
            }
        }

        /// A BRANCHED plan: `hit = dmg × event_factors` with a `crit`
        /// event whose factor is 2. Two fixtures need it — `on_crit` has
        /// no meaning without a crit event (`Plan::crit_chance` fail-softs
        /// to `0.0`), and an RNG draw is only OBSERVABLE when something
        /// samples.
        fn crit_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg", "crit_chance"],
                     "events": { "crit": { "chance": "crit_chance / 100",
                                            "factor": "2" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * event_factors",
                                     "branched": true } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        /// `crit_chance` 100 makes EV's `on_crit` weight exactly `1.0`, so
        /// an `on_crit` proc's fire count is the plain cast count and the
        /// trigger-filter pin stays hand-workable.
        fn crit_build() -> BuildState {
            serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "crit_chance": 100.0 } }"#).unwrap()
        }

        fn dummy(seconds: u32) -> Scenario {
            serde_json::from_str(&format!(
                r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
            ))
            .unwrap()
        }

        fn only(action: &str) -> Rotation {
            Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: action.into(),
                    when: None,
                }],
            }
        }

        fn ev_with(
            plan: &Plan,
            build: &BuildState,
            simdef: &SimDef,
            rotation: &Rotation,
            seconds: u32,
        ) -> SimReport {
            let sim_plan = sim_compile(plan, simdef, rotation).unwrap();
            run(plan, &sim_plan, build, &dummy(seconds), Mode::Expected).unwrap()
        }

        fn ev(plan: &Plan, simdef: &SimDef, rotation: &Rotation, seconds: u32) -> SimReport {
            ev_with(plan, &scoped_build(), simdef, rotation, seconds)
        }

        /// An action with no damage effect — the shape `apply_buff` exists
        /// for, and the one the D4 example's `frost_nova` now uses.
        fn utility(cast_time: &str, cooldown: f64, apply_buff: Vec<String>) -> ActionDef {
            ActionDef {
                extra: Default::default(),
                measure: None,
                cast_time: cast_time.into(),
                cooldown: NumOrExpr::Num(cooldown),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: None,
                apply_buff,
                effects: Vec::new(),
            }
        }

        /// A damaging action. `stats` is its `damage.stats` overlay —
        /// empty means "no override", so the overlay build equals the
        /// effective build.
        fn damaging(
            cast_time: &str,
            cooldown: f64,
            stats: BTreeMap<String, NumOrExpr>,
            apply_buff: Vec<String>,
        ) -> ActionDef {
            ActionDef {
                extra: Default::default(),
                measure: None,
                cast_time: cast_time.into(),
                cooldown: NumOrExpr::Num(cooldown),
                cost: BTreeMap::new(),
                gain: BTreeMap::new(),
                damage: Some(ActionDamage {
                    extra: Default::default(),
                    stats,
                }),
                apply_buff,
                effects: Vec::new(),
            }
        }

        /// A plain timed buff: no contributions, no conditions, no tick.
        fn plain_buff(duration: f64) -> BuffDef {
            BuffDef {
                extra: Default::default(),
                duration: NumOrExpr::Num(duration),
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: None,
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
            }
        }

        /// `+100` to the PRODUCT bucket `boost` — doubles whatever reads
        /// it, for as long as it is live.
        fn empower_buff(duration: f64) -> BuffDef {
            BuffDef {
                extra: Default::default(),
                duration: NumOrExpr::Num(duration),
                contributions: vec![Contribution {
                    bucket: "boost".into(),
                    value: 100.0,
                    event: None,
                    condition: None,
                }],
                conditions: BTreeMap::new(),
                tick_objective: None,
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
            }
        }

        /// A snapshot DoT: each instance ticks the `dot` rate it captured
        /// at its own application.
        fn snapshot_buff(duration: f64) -> BuffDef {
            BuffDef {
                extra: Default::default(),
                duration: NumOrExpr::Num(duration),
                contributions: Vec::new(),
                conditions: BTreeMap::new(),
                tick_objective: Some(TickObjective::snapshot("dot")),
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
            }
        }

        // ------------------------------------------------------------------
        // (a) The headline: a buff window driven by an action ALONE, with no
        // proc anywhere in the config — the thing rtce 0.2.0 could not
        // express, and the reason the `icd == cooldown` trick existed.
        //
        // `pulse` is instant (cast_time 0) with a 10s cooldown and is the
        // only rule, so it casts at t=0 and t=10 (the decision the t=20 wake
        // would drive never happens — the run loop stops at `duration`, see
        // the module docs on the `End` boundary). Each cast opens a 4s
        // `window`:
        //   [0,4) and [10,14)  →  uptime = (4 + 4) / 20 = 0.4
        // The buff drives `focused`, and the scenario asserts no static
        // uptime for it, so the condition's 0.4 is COMPUTED end-to-end —
        // exactly the shape `examples/diablo4_rotation.rs` now uses.
        //
        // Mutation contrast: an `apply_buff` the executor ignores leaves all
        // three numbers at 0.0.
        // ------------------------------------------------------------------
        #[test]
        fn apply_buff_alone_drives_a_hand_worked_uptime_with_no_proc_defined() {
            let plan = scoped_plan();
            let mut actions = BTreeMap::new();
            actions.insert(
                "pulse".to_string(),
                utility("0", 10.0, vec!["window".into()]),
            );
            let mut window = plain_buff(4.0);
            window.conditions.insert("focused".to_string(), 1.0);
            let mut buffs = BTreeMap::new();
            buffs.insert("window".to_string(), window);
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };

            let report = ev(&plan, &simdef, &only("pulse"), 20);

            assert_eq!(report.actions["pulse"].casts, 2, "casts at t=0 and t=10");
            assert!(
                report.proc_counts.is_empty(),
                "this config defines no proc at all — that is the point"
            );
            assert!(
                close(report.buffs["window"].uptime, 0.4),
                "uptime: got {} — want (4 + 4) / 20 = 0.4",
                report.buffs["window"].uptime
            );
            assert!(
                close(report.buffs["window"].avg_stacks, 0.4),
                "avg_stacks: got {} — one instance for 8 of 20 seconds",
                report.buffs["window"].avg_stacks
            );
            assert!(
                close(report.condition_uptime["focused"], 0.4),
                "condition uptime: got {} — the buff is `focused`'s only source",
                report.condition_uptime["focused"]
            );
        }

        // ------------------------------------------------------------------
        // Ordering, half 1: an action-applied buff lands AFTER this cast's
        // own damage has been measured and credited — a cast never benefits
        // from the buff it applies (the same rule `ActionDamage::stats`
        // states for procs).
        //
        // `strike` (1s cast, hit = dmg × boost = 100) applies `empower`,
        // whose `+100` contribution to the PRODUCT bucket `boost` doubles
        // the hit. Casts complete at t=1..5 (the 5th starts at t=4 and
        // completes exactly at `duration`), and `empower` lasts 100s, so:
        //   t=1  unbuffed  100
        //   t=2..5  buffed  4 × 200 = 800
        //   total = 900, dps = 900/5 = 180
        // Applied BEFORE the damage instead, every cast would read 200 →
        // 1000.
        // ------------------------------------------------------------------
        #[test]
        fn an_action_applied_buff_does_not_amplify_the_cast_that_applied_it() {
            let plan = scoped_plan();
            let mut actions = BTreeMap::new();
            actions.insert(
                "strike".to_string(),
                damaging("1", 0.0, BTreeMap::new(), vec!["empower".into()]),
            );
            let mut buffs = BTreeMap::new();
            buffs.insert("empower".to_string(), empower_buff(100.0));
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };

            let report = ev(&plan, &simdef, &only("strike"), 5);

            assert_eq!(report.actions["strike"].casts, 5);
            assert!(
                close(report.total.total_damage, 900.0),
                "total: got {} — want 100 (first cast, unbuffed) + 4 × 200 = \
                 900; 1000 would mean the buff landed before its own cast's \
                 damage",
                report.total.total_damage
            );
            assert!(close(report.total.dps, 180.0), "got {}", report.total.dps);
        }

        // ------------------------------------------------------------------
        // Ordering, half 2 (the intra-instant decision this task owns): an
        // action-applied buff lands BEFORE any of this cast's proc rolls, so
        // a proc's `chance` expression SEES it. Intrinsic effects resolve
        // before triggered ones, which also means the whole `apply_buff` list
        // precedes the whole proc batch and never interleaves with the procs'
        // (BTreeMap name) order.
        //
        // `strike` is a 1s cast on a 1000s cooldown — exactly ONE cast,
        // completing at t=1 in a 3s fight — and applies `window`. The proc
        // `gate` rolls `on_cast` with `chance` = `buff.window`, so its whole
        // behavior is the ordering question:
        //   apply_buff first → chance 1 → gate fires once
        //   procs first      → chance 0, and there is no second cast to try
        //                      again on → gate never fires
        // ------------------------------------------------------------------
        #[test]
        fn a_procs_chance_sees_the_buff_the_same_cast_applied() {
            let plan = scoped_plan();
            let mut actions = BTreeMap::new();
            actions.insert(
                "strike".to_string(),
                damaging("1", 1000.0, BTreeMap::new(), vec!["window".into()]),
            );
            let mut buffs = BTreeMap::new();
            buffs.insert("window".to_string(), plain_buff(100.0));
            buffs.insert("mark".to_string(), plain_buff(100.0));
            let mut procs = BTreeMap::new();
            procs.insert(
                "gate".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "buff.window".into(),
                    icd: 0.0,
                    apply_buff: Some("mark".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            };

            let report = ev(&plan, &simdef, &only("strike"), 3);

            assert_eq!(report.actions["strike"].casts, 1, "one cast, at t=1");
            assert_eq!(
                report.proc_counts["gate"], 1,
                "the proc's `chance` reads `buff.window`, which the SAME cast \
                 applied — 0 would mean procs roll before `apply_buff`"
            );
        }

        // ------------------------------------------------------------------
        // Ordering, half 3: the list is applied in SOURCE order, and a name
        // repeated in it is applied that many times.
        //
        // This is also HALF ONE of the asymmetry pinned by
        // `a_snapshot_capture_is_frozen_across_the_list_on_both_action_paths`
        // below: a `duration` expression reads sim STATE through the slot
        // array, which `Sim::apply_buff` refreshes per entry, so it IS
        // sequential — a later entry sees earlier entries' STACK COUNTS. A
        // snapshot MAGNITUDE reads a BUILD, and that build is frozen for the
        // whole list. Same list, two different rules, both deliberate.
        //
        // `pulse` (instant, 1000s cooldown → one cast at t=0) lists
        // `["gate", "gate", "timed"]` — deliberately NOT a palindrome, so
        // reversing the traversal is observable:
        //   `gate`  duration 3, `add_independent`, unbounded
        //   `timed` duration `"2 * (1 + stacks.gate)"` — an expression whose
        //           value is exactly "how many `gate` instances were already
        //           live when I landed"
        // In source order: gate, gate (2 instances, both expiring at t=3),
        // then `timed` reads stacks.gate = 2 and takes duration 6. Over a
        // 10s fight:
        //   gate   uptime 3/10 = 0.3, avg_stacks 2 × 3 / 10 = 0.6
        //   timed  uptime 6/10 = 0.6
        // Three mutations this catches, each landing on a DIFFERENT number:
        // deduping the list → gate avg_stacks 0.3 (and `timed` 4/10 = 0.4);
        // reversing the traversal → `timed` lands first at stacks.gate = 0,
        // duration 2 → uptime 0.2; dropping the repeat entirely → both.
        // ------------------------------------------------------------------
        #[test]
        fn apply_buff_applies_the_list_in_order_and_a_repeat_applies_twice() {
            let plan = scoped_plan();
            let mut actions = BTreeMap::new();
            actions.insert(
                "pulse".to_string(),
                utility(
                    "0",
                    1000.0,
                    vec!["gate".into(), "gate".into(), "timed".into()],
                ),
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "gate".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(3.0),
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: None,
                    max_stacks: 0,
                    on_reapply: ReapplyPolicy::AddIndependent,
                },
            );
            buffs.insert(
                "timed".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Expr("2 * (1 + stacks.gate)".into()),
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: None,
                    max_stacks: 1,
                    on_reapply: ReapplyPolicy::Refresh,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };

            let report = ev(&plan, &simdef, &only("pulse"), 10);

            assert!(
                close(report.buffs["gate"].avg_stacks, 0.6),
                "gate avg_stacks: got {} — want 2 instances × 3s / 10 = 0.6 \
                 (0.3 would mean the repeated name was applied once)",
                report.buffs["gate"].avg_stacks
            );
            assert!(
                close(report.buffs["gate"].uptime, 0.3),
                "gate uptime: got {}",
                report.buffs["gate"].uptime
            );
            assert!(
                close(report.buffs["timed"].uptime, 0.6),
                "timed uptime: got {} — its duration read `stacks.gate` = 2, \
                 so 6/10 = 0.6 (0.2 would mean it was applied BEFORE the two \
                 `gate`s that precede it in the list; 0.4 that they were \
                 deduped to one)",
                report.buffs["timed"].uptime
            );
        }

        // ------------------------------------------------------------------
        // Decisions 1 and 2, pinned as each other's control.
        //
        // An action-applied SNAPSHOT DoT captures under the CASTING ACTION's
        // overlay — the effective build with that action's `damage.stats`
        // applied — because a PoE2 ailment inherits the magnitude of the hit
        // that applied it. The PROC path is deliberately left on the plain
        // effective build (P7c-T2's `None`, still load-bearing for its own
        // pins).
        //
        // Same fixture both ways: `strike` is a 2s cast on a 1000s cooldown
        // (exactly one cast, completing at t=2 of a 10s fight) whose
        // `damage.stats` overrides `dmg` to 300. `ail` snapshots `dot` and
        // lasts 100s, so it ticks its captured rate over [2, 10] = 8s. This
        // is `scoped_plan`, where `dot = dmg × 0.5 × dot_scale` and so reads
        // the OVERLAID `dmg` but nothing else:
        //   hit  = dmg × boost = 300 × 1                       = 300
        //   ACTION-applied  R = 300 × 0.5 × 1 = 150 → 8 × 150   = 1200
        //     total 1500
        //   PROC-applied    R = 100 × 0.5 × 1 =  50 → 8 ×  50   =  400
        //     total 700
        // The two totals ARE the mutation contrast: 700 from the action path
        // would mean the overlay was dropped, 1500 from the proc path would
        // mean the proc path was switched to the overlay behind P7c-T2's
        // back.
        // ------------------------------------------------------------------
        #[test]
        fn an_action_applied_snapshot_captures_the_overlay_and_a_proc_applied_one_does_not() {
            let plan = scoped_plan();
            let build_simdef = |by_action: bool| {
                let mut stats = BTreeMap::new();
                stats.insert("dmg".to_string(), NumOrExpr::Num(300.0));
                let mut actions = BTreeMap::new();
                actions.insert(
                    "strike".to_string(),
                    damaging(
                        "2",
                        1000.0,
                        stats,
                        if by_action {
                            vec!["ail".into()]
                        } else {
                            Vec::new()
                        },
                    ),
                );
                let mut buffs = BTreeMap::new();
                buffs.insert("ail".to_string(), snapshot_buff(100.0));
                let mut procs = BTreeMap::new();
                if !by_action {
                    procs.insert(
                        "ail_proc".to_string(),
                        ProcDef {
                            extra: Default::default(),
                            rolls: None,
                            trigger: Trigger::OnCast,
                            chance: "1".into(),
                            icd: 0.0,
                            apply_buff: Some("ail".into()),
                            effects: Vec::new(),
                            cast_action: None,
                            actions: None,
                        },
                    );
                }
                SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs,
                    damage_objective: "hit".into(),
                }
            };

            let by_action = ev(&plan, &build_simdef(true), &only("strike"), 10);
            let by_proc = ev(&plan, &build_simdef(false), &only("strike"), 10);

            // Same cast, same window, same hit — only the captured rate can
            // differ between the two runs.
            for (label, r) in [("action", &by_action), ("proc", &by_proc)] {
                assert_eq!(r.actions["strike"].casts, 1, "{label}");
                assert!(
                    close(r.actions["strike"].damage, 300.0),
                    "{label}: the hit itself is the overlaid 300 either way, got {}",
                    r.actions["strike"].damage
                );
                assert!(close(r.buffs["ail"].uptime, 0.8), "{label}: [2,10] of 10s");
            }

            assert!(
                close(by_action.total.total_damage, 1500.0),
                "action-applied: got {} — want 300 hit + 8s × R=150 (the \
                 overlay's dmg 300) = 1500; 700 would mean the capture read \
                 the un-overlaid effective build",
                by_action.total.total_damage
            );
            assert!(
                close(by_proc.total.total_damage, 700.0),
                "proc-applied: got {} — want 300 hit + 8s × R=50 (the \
                 effective build) = 700; 1500 would mean the PROC path \
                 started passing an overlay, which P7d deliberately did not \
                 change",
                by_proc.total.total_damage
            );
        }

        // ------------------------------------------------------------------
        // The build a snapshot capture reads is FROZEN for the whole
        // `apply_buff` list, and — this is the half that was wrong before the
        // P7d review — frozen identically on BOTH action paths.
        //
        // `boosted_dot_plan`, where `dot = dmg × 0.5 × boost`, so a `+100`
        // contribution to the PRODUCT bucket `boost` DOUBLES a captured rate.
        // Both actions are 2s casts on a 1000s cooldown (one cast, completing
        // at t=2 of a 10s fight) and both list `["empower", "ail"]` — a
        // contribution followed by a snapshot DoT that would read it:
        //   `empower` +100 to `boost`, 100s
        //   `ail`     snapshot `dot`, 100s → ticks over [2,10] = 8s
        //
        // FROZEN (what this pins): `ail` captures against the world the CAST
        // found, before `empower` folded in — R = 100 × 0.5 × 1 = 50, so 8 ×
        // 50 = 400 of DoT on either path.
        //   damaging  total = 100 hit (boost was still 1 when it was
        //                     measured) + 400 = 500
        //   utility   total = 0 + 400 = 400
        //
        // Two mutations, one per path, both previously ALIVE:
        //   - a damaging action re-reading the live build per entry → `ail`
        //     captures 100 → 800 of DoT → total 900.
        //   - a utility action left on `None` (the pre-review behavior, where
        //     `Sim::apply_buff` resolved the ambient build per entry) → the
        //     same 800 → total 800. That is the asymmetry this test exists
        //     for: the two paths gave different answers to the identical
        //     config, so the DoT totals are asserted EQUAL as well as
        //     individually pinned.
        //
        // The complementary half — that `duration` IS sequential across the
        // list — is pinned by
        // `apply_buff_applies_the_list_in_order_and_a_repeat_applies_twice`.
        // ------------------------------------------------------------------
        #[test]
        fn a_snapshot_capture_is_frozen_across_the_list_on_both_action_paths() {
            let plan = boosted_dot_plan();
            let build = boosted_dot_build();
            let list = || vec!["empower".into(), "ail".into()];
            let simdef = |action: ActionDef| {
                let mut actions = BTreeMap::new();
                actions.insert("caster".to_string(), action);
                let mut buffs = BTreeMap::new();
                buffs.insert("empower".to_string(), empower_buff(100.0));
                buffs.insert("ail".to_string(), snapshot_buff(100.0));
                SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs: BTreeMap::new(),
                    damage_objective: "hit".into(),
                }
            };

            let dmg_run = ev_with(
                &plan,
                &build,
                &simdef(damaging("2", 1000.0, BTreeMap::new(), list())),
                &only("caster"),
                10,
            );
            let util_run = ev_with(
                &plan,
                &build,
                &simdef(utility("2", 1000.0, list())),
                &only("caster"),
                10,
            );

            assert!(
                close(dmg_run.actions["caster"].damage, 100.0),
                "the hit is measured before `empower` folds in: got {}",
                dmg_run.actions["caster"].damage
            );
            let dmg_dot = dmg_run.total.total_damage - dmg_run.actions["caster"].damage;
            let util_dot = util_run.total.total_damage;

            assert!(
                close(dmg_dot, 400.0),
                "damaging path DoT: got {dmg_dot} — want 8s × R=50, the rate \
                 frozen before `empower` (800 would mean the capture re-read \
                 the live build after the earlier list entry)"
            );
            assert!(
                close(util_dot, 400.0),
                "utility path DoT: got {util_dot} — want the SAME 8s × R=50 \
                 (800 would mean the utility path resolved the ambient build \
                 per entry instead of freezing it, which is what it did \
                 before the P7d review)"
            );
            assert!(
                close(dmg_dot, util_dot),
                "the two action paths must agree on the captured rate for an \
                 identical `apply_buff` list: {dmg_dot} vs {util_dot}"
            );
            assert!(
                close(dmg_run.total.total_damage, 500.0),
                "damaging total: got {} — want 100 hit + 400 DoT",
                dmg_run.total.total_damage
            );
        }

        // ------------------------------------------------------------------
        // THE ONE-WORLD FIX (P8c — the phase's single deliberate behavior
        // change). Through 0.3.0 a same-list snapshot capture read a FROZEN
        // BUILD against the LIVE EFFECTIVE PHASE: contributions frozen,
        // conditions not, so reordering `["mark", "poison"]` DOUBLED the DoT
        // (800 vs 400) at identical reported uptime — pinned by this test's
        // previous incarnation,
        // `a_same_list_snapshot_capture_reads_a_frozen_build_but_a_live_phase`,
        // and flagged there as the open 0.4.0 question. P8c answers it: a
        // capture reads the cast's ONE measured world — build AND phase both
        // from the snapshot — so both orderings now capture the pre-list
        // world.
        //
        // `marked_dot_plan`, where `dot = dmg × 0.5 × boost × (1 + marked)`.
        // One `mark` buff drives BOTH axes at `+100` / `1.0`. A 2s cast on a
        // 1000s cooldown completes at t=2 of a 10s fight; both buffs last
        // 100s, so the DoT ticks [2,10] = 8s. Only the LIST ORDER differs
        // between the two runs — and it no longer matters:
        //
        //   either order: poison captures the world the cast MEASURED,
        //     before any list entry ran — boost 1, marked 0
        //     R = 100 × 0.5 × 1 × (1 + 0) = 50  →  8 × 50 = 400 DoT
        //
        // 400 is the old poison-first value: the fix collapses the pair onto
        // the ordering that already captured the pre-list world, it does not
        // invent a third number.
        //
        // What a later entry sees is now TWO axes, not three: sim STATE is
        // sequential (`duration` still reads earlier entries' live windows —
        // the 0.2/0.1 order pin in `mod effects_list` and the pandemic 0.75
        // pin in `mod expr_fields` are byte-identical, since "one world"
        // governs Plan evaluations and never sim-FIELD reads), and the
        // measured WORLD (build + phase alike) is the snapshot's.
        //
        // Mutations, both run: restoring the live-phase read sends
        // ["mark","poison"] back to 8 × (100 × 0.5 × 1 × 2) = 800; leaving
        // the build live too sends it to 8 × (100 × 0.5 × 2 × 2) = 1600.
        // ------------------------------------------------------------------
        #[test]
        fn a_same_list_snapshot_capture_reads_one_frozen_world() {
            let plan = marked_dot_plan();
            let build = boosted_dot_build();
            let simdef = |action: ActionDef| {
                let mut actions = BTreeMap::new();
                actions.insert("caster".to_string(), action);
                let mut buffs = BTreeMap::new();
                buffs.insert("mark".to_string(), mark_buff(100.0));
                buffs.insert("poison".to_string(), snapshot_buff(100.0));
                SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs: BTreeMap::new(),
                    damage_objective: "hit".into(),
                }
            };
            // DoT only: the utility path deals no hit damage, and the
            // damaging path's hit is measured before the list runs, so
            // subtracting it leaves the same quantity on both.
            let dot_of = |action: ActionDef| {
                let r = ev_with(&plan, &build, &simdef(action.clone()), &only("caster"), 10);
                let hit = r.actions["caster"].damage;
                (r.total.total_damage - hit, r.buffs["poison"].uptime)
            };
            let list = |a: &str, b: &str| vec![a.to_string(), b.to_string()];

            let (mark_first_dmg, up_a) = dot_of(damaging(
                "2",
                1000.0,
                BTreeMap::new(),
                list("mark", "poison"),
            ));
            let (mark_first_util, _) = dot_of(utility("2", 1000.0, list("mark", "poison")));
            let (poison_first_dmg, up_b) = dot_of(damaging(
                "2",
                1000.0,
                BTreeMap::new(),
                list("poison", "mark"),
            ));
            let (poison_first_util, _) = dot_of(utility("2", 1000.0, list("poison", "mark")));

            // The EQUALITY is the fix: list order no longer moves a capture.
            assert!(
                close(mark_first_dmg, poison_first_dmg),
                "one world per cast — the two orderings of one two-entry \
                 list must capture the same rate: {mark_first_dmg} vs \
                 {poison_first_dmg} (800 vs 400 is the 0.3.0 live-phase \
                 incoherence this fix removes)"
            );
            // And the LITERAL pins which world that is: the pre-list one.
            assert!(
                close(mark_first_dmg, 400.0),
                "[mark, poison]: got {mark_first_dmg} — want 8s × R=50, the \
                 capture reading the cast's measured world from BEFORE the \
                 list ran (800 would mean the phase went live again; 1600 \
                 would mean the build did too)"
            );
            assert!(
                close(poison_first_dmg, 400.0),
                "[poison, mark]: got {poison_first_dmg} — want the unchanged \
                 old poison-first value; the fix moves mark-first DOWN, it \
                 does not move this ordering at all"
            );

            // The stated design goal, still met: the two action paths agree.
            assert!(
                close(mark_first_dmg, mark_first_util),
                "damaging vs utility path disagree on [mark, poison]: \
                 {mark_first_dmg} vs {mark_first_util}"
            );
            assert!(
                close(poison_first_dmg, poison_first_util),
                "damaging vs utility path disagree on [poison, mark]: \
                 {poison_first_dmg} vs {poison_first_util}"
            );

            // And the reason this needs a pin: the integrated column a
            // reader would reach for cannot see any of it.
            assert!(
                close(up_a, up_b) && close(up_a, 0.8),
                "poison uptime is [2,10] of 10s = 0.8 in BOTH orderings — \
                 the swing is invisible there: {up_a} vs {up_b}"
            );
        }

        // ------------------------------------------------------------------
        // `ProcDef::icd` is a bare literal rather than a `NumOrExpr`, so it
        // never passed through P7b's fail-closed evaluation checks and was
        // the last unvalidated number in the sim config. Both bad values
        // failed SILENTLY, and NaN failed in the WORST direction: the ICD
        // gate is `now < icd_ready_at`, which is false for every `now` once
        // that deadline is NaN — so `icd: NaN` DELETED the internal cooldown
        // rather than tightening it, turning a gated proc into an ungated
        // one with no error anywhere. A negative icd is merely "no ICD"
        // spelled confusingly, and is rejected for the same reason
        // `cooldown`/`cost`/`gain` reject negatives.
        // ------------------------------------------------------------------
        #[test]
        fn a_non_finite_or_negative_proc_icd_is_a_compile_error() {
            let plan = scoped_plan();
            let compile_with = |icd: f64| {
                let mut actions = BTreeMap::new();
                actions.insert(
                    "strike".to_string(),
                    damaging("1", 0.0, BTreeMap::new(), Vec::new()),
                );
                let mut buffs = BTreeMap::new();
                buffs.insert("ail".to_string(), plain_buff(1.0));
                let mut procs = BTreeMap::new();
                procs.insert(
                    "gated".to_string(),
                    ProcDef {
                        extra: Default::default(),
                        rolls: None,
                        trigger: Trigger::OnCast,
                        chance: "1".into(),
                        icd,
                        apply_buff: Some("ail".into()),
                        effects: Vec::new(),
                        cast_action: None,
                        actions: None,
                    },
                );
                let simdef = SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs,
                    damage_objective: "hit".into(),
                };
                sim_compile(&plan, &simdef, &only("strike"))
            };

            for bad in [f64::NAN, f64::INFINITY, -5.0] {
                let e = compile_with(bad).unwrap_err();
                assert!(e.what.contains("gated"), "got: {}", e.what);
                assert!(e.what.contains("icd"), "got: {}", e.what);
            }
            // The boundary and a normal value both still compile.
            assert!(compile_with(0.0).is_ok());
            assert!(compile_with(2.5).is_ok());
        }

        // ------------------------------------------------------------------
        // `Trigger::OnHit` rolls once per damaging CAST, not once per HIT.
        //
        // The name says otherwise and nothing else in the crate said so
        // before 0.3.0, which makes it a silent modeling trap for lucky-hit
        // style procs: `Sim::complete_cast` presents this trigger with
        // exactly ONE roll per completing cast, while `hits_per_use` is read
        // separately by `Sim::eval_hits_per_use` and only ever multiplies
        // DAMAGE.
        //
        // `scoped_plan`; `strike` is a 1s cast with `hits_per_use: 5` over a
        // 10s fight → 10 casts, 50 hits. The damage pin proves the fixture
        // really does land 50 hits; the proc counts prove only 10 are ever
        // offered to the trigger.
        //
        // Both modes are asserted, because each is blind to the other's
        // plausible per-hit implementation:
        //
        //   EV, `chance: "0.2"`. The accumulator fires at most ONCE per
        //   CALL, so a per-hit reading spelled as "weight the roll by hits"
        //   is invisible at `chance: "1"` — this fixture is fractional on
        //   purpose. acc += 0.2 per cast, crossing 1.0 at casts 5 and 10:
        //     fires = 2.  Weighting the roll by `hits` gives acc += 1.0 per
        //     cast → 10. Looping the roll 5× per cast also gives 10.
        //
        //   MC, `chance: "1"`. No accumulator: one draw per roll, and a
        //   certain chance fires on every one, so the count IS the roll
        //   count and is seed-independent.
        //     fires = 10.  A per-hit loop gives 50.
        //
        // Whether it SHOULD scale with `hits_per_use` is an open 0.4.0
        // question in ROADMAP.md; this pins today's answer so that changing
        // it has to be deliberate.
        // ------------------------------------------------------------------
        #[test]
        fn on_hit_rolls_once_per_cast_not_once_per_hit() {
            let plan = scoped_plan();
            let build = scoped_build();
            let run_with = |chance: &str, mode: Mode| {
                let mut stats = BTreeMap::new();
                stats.insert("hits_per_use".to_string(), NumOrExpr::Num(5.0));
                let mut actions = BTreeMap::new();
                actions.insert("strike".to_string(), damaging("1", 0.0, stats, Vec::new()));
                let mut buffs = BTreeMap::new();
                buffs.insert("ail".to_string(), plain_buff(0.5));
                let mut procs = BTreeMap::new();
                procs.insert(
                    "per_hit".to_string(),
                    ProcDef {
                        extra: Default::default(),
                        rolls: None,
                        trigger: Trigger::OnHit,
                        chance: chance.into(),
                        icd: 0.0,
                        apply_buff: Some("ail".into()),
                        effects: Vec::new(),
                        cast_action: None,
                        actions: None,
                    },
                );
                let simdef = SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs,
                    damage_objective: "hit".into(),
                };
                let sim_plan = sim_compile(&plan, &simdef, &only("strike")).unwrap();
                run(&plan, &sim_plan, &build, &dummy(10), mode).unwrap()
            };

            let ev_run = run_with("0.2", Mode::Expected);
            assert_eq!(ev_run.actions["strike"].casts, 10);
            assert!(
                close(ev_run.actions["strike"].damage, 5000.0),
                "10 casts × hits_per_use 5 × hit 100 = 5000, so the fixture \
                 genuinely lands 50 hits: got {}",
                ev_run.actions["strike"].damage
            );
            assert_eq!(
                ev_run.proc_counts["per_hit"], 2,
                "EV at chance 0.2: one roll per CAST accumulates to 1.0 at \
                 casts 5 and 10. 10 would mean the roll had started scaling \
                 with hits_per_use"
            );

            let mc_run = run_with(
                "1",
                Mode::MonteCarlo {
                    iterations: 4,
                    seed: 7,
                },
            );
            assert_eq!(
                mc_run.proc_counts["per_hit"], 10,
                "MC at chance 1: one certain draw per CAST, seed-independent. \
                 50 would mean a draw per HIT"
            );
        }

        // ------------------------------------------------------------------
        // When two live buffs drive the SAME condition, the winner is the
        // one whose NAME sorts first. `Sim::condition_value` returns the
        // first match in `active_buff_set`, which holds BUFF INDICES, and
        // `sim::compile` assigns those in name-sorted order — so the rule is
        // alphabetical. Not "strongest", not "most recently applied", not
        // summed. Renaming a buff therefore changes the number, which is why
        // this is pinned rather than left to a private comment.
        //
        // `marked_dot_plan` (it is the plan here with a condition). Two
        // utility buffs drive `marked` at DIFFERENT values and nothing else;
        // one instant cast at t=0 opens both windows for the whole 10s
        // fight, so the tie-break is the only thing that can decide the
        // reported uptime.
        //   "a_chill" → 0.25   "z_frost" → 1.0
        //   reported `marked` uptime = 0.25 — the alphabetically FIRST
        //   buff's value, though `z_frost` is stronger AND was applied
        //   second.
        //
        // The rename control is the proof: swapping ONLY the two names, so
        // the strong one now sorts first, flips the report to 1.0 with an
        // otherwise byte-identical config.
        // ------------------------------------------------------------------
        #[test]
        fn two_buffs_driving_one_condition_resolve_by_buff_name_order() {
            let plan = marked_dot_plan();
            let build = boosted_dot_build();
            let cond = |v: f64| BuffDef {
                extra: Default::default(),
                duration: NumOrExpr::Num(100.0),
                contributions: Vec::new(),
                conditions: [("marked".to_string(), v)].into_iter().collect(),
                tick_objective: None,
                max_stacks: 1,
                on_reapply: ReapplyPolicy::Refresh,
            };
            // The WEAK one is always applied first; only the NAMES differ
            // between the two runs.
            let uptime_with = |weak: &str, strong: &str| {
                let mut actions = BTreeMap::new();
                actions.insert(
                    "opener".to_string(),
                    utility("0", 1000.0, vec![weak.to_string(), strong.to_string()]),
                );
                let mut buffs = BTreeMap::new();
                buffs.insert(weak.to_string(), cond(0.25));
                buffs.insert(strong.to_string(), cond(1.0));
                let simdef = SimDef {
                    extra: Default::default(),
                    defaults: Default::default(),
                    resources: BTreeMap::new(),
                    actions,
                    buffs,
                    procs: BTreeMap::new(),
                    damage_objective: "hit".into(),
                };
                ev_with(&plan, &build, &simdef, &only("opener"), 10).condition_uptime["marked"]
            };

            let weak_sorts_first = uptime_with("a_chill", "z_frost");
            let strong_sorts_first = uptime_with("z_chill", "a_frost");

            assert!(
                close(weak_sorts_first, 0.25),
                "got {weak_sorts_first} — the alphabetically FIRST live buff \
                 wins, so the weaker 0.25 is reported even though the \
                 stronger 1.0 is up and was applied later (1.0 would mean \
                 strongest-wins, 1.25 summed)"
            );
            assert!(
                close(strong_sorts_first, 1.0),
                "got {strong_sorts_first} — renaming the SAME two buffs so \
                 the strong one sorts first must change the answer; that it \
                 does is the point of this pin"
            );
        }

        // ------------------------------------------------------------------
        // A buff-driven condition's REPORTED uptime is clamped to [0,1] —
        // the same clamp `Plan` applies where the value actually folds.
        //
        // `BuffDef::conditions` is not range-checked at compile time, and
        // `Sim::condition_value`'s buff branch returns the raw number (its
        // scenario branch has always clamped). Integrating the raw value
        // made the DIAGNOSTIC disagree with the math: `marked: 5.0` folds as
        // 1.0 but used to report 5 × its live fraction.
        //
        // One instant cast at t=0 opens a 5s window on a 10s fight, so the
        // condition is up for exactly half the fight:
        //   clamped (now):    1.0 × 0.5 = 0.5
        //   un-clamped (was): 5.0 × 0.5 = 2.5
        // Report-only: the value the math used is unchanged either way.
        // ------------------------------------------------------------------
        #[test]
        fn a_buff_driven_condition_uptime_is_clamped_like_the_value_that_folds() {
            let plan = marked_dot_plan();
            let build = boosted_dot_build();
            let mut actions = BTreeMap::new();
            actions.insert(
                "opener".to_string(),
                utility("0", 1000.0, vec!["over".into()]),
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "over".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(5.0),
                    contributions: Vec::new(),
                    conditions: [("marked".to_string(), 5.0)].into_iter().collect(),
                    tick_objective: None,
                    max_stacks: 1,
                    on_reapply: ReapplyPolicy::Refresh,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let r = ev_with(&plan, &build, &simdef, &only("opener"), 10);

            assert!(
                close(r.buffs["over"].uptime, 0.5),
                "the WINDOW is [0,5) of 10s either way: got {}",
                r.buffs["over"].uptime
            );
            assert!(
                close(r.condition_uptime["marked"], 0.5),
                "got {} — a condition is an uptime FRACTION, so `5.0` folds \
                 as 1.0 and must report as 1.0 × 0.5 = 0.5 (2.5 is the \
                 pre-0.3.0 un-clamped integral, a diagnostic disagreeing \
                 with the value the math used)",
                r.condition_uptime["marked"]
            );
        }

        // ------------------------------------------------------------------
        // `apply_buff` is an effect OF the action — like `gain` and `damage`,
        // and unlike the cast pipeline (cost, cooldown, further proc rolls)
        // that a proc-triggered FREE cast deliberately skips. So a free cast
        // applies it too, WITH that free cast's own overlay; anything else
        // would make the same action mean two different things depending on
        // who cast it, silently.
        //
        // `strike` (1s cast, 1000s cooldown → one cast, completing at t=1,
        // hit = 100) triggers `echo`, which free-casts `bonus`. `bonus`
        // overrides `dmg` to 300 and applies a 4s `window` that snapshots
        // `dot`. The free cast happens AT the firing proc's instant, so:
        //   bonus hit = 300 × boost 1                     = 300
        //   window captures R = 300 × 0.5 × 1 = 150 over [1,5) = 4s → 600
        //   total = 100 + 300 + 600 = 1000, uptime 4/20 = 0.2
        // `bonus` is not in the rotation at all, so the ONLY way it can cast
        // is the proc. Two mutations: a free cast that skips `apply_buff`
        // leaves uptime 0 and total 400; one that applies the buff but drops
        // the OVERLAY captures R = 50 → 200 of DoT → total 600.
        // ------------------------------------------------------------------
        #[test]
        fn a_proc_free_cast_applies_that_actions_own_apply_buff_under_its_own_overlay() {
            let plan = scoped_plan();
            let mut bonus_stats = BTreeMap::new();
            bonus_stats.insert("dmg".to_string(), NumOrExpr::Num(300.0));
            let mut actions = BTreeMap::new();
            actions.insert(
                "strike".to_string(),
                damaging("1", 1000.0, BTreeMap::new(), Vec::new()),
            );
            actions.insert(
                "bonus".to_string(),
                damaging("0", 0.0, bonus_stats, vec!["window".into()]),
            );
            let mut buffs = BTreeMap::new();
            buffs.insert("window".to_string(), snapshot_buff(4.0));
            let mut procs = BTreeMap::new();
            procs.insert(
                "echo".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "1".into(),
                    icd: 1000.0,
                    apply_buff: None,
                    effects: Vec::new(),
                    cast_action: Some("bonus".into()),
                    actions: None,
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            };

            let report = ev(&plan, &simdef, &only("strike"), 20);

            assert_eq!(report.actions["bonus"].casts, 1, "free-cast once, at t=1");
            assert!(
                close(report.buffs["window"].uptime, 0.2),
                "uptime: got {} — want the free cast's window [1,5) of 20s = \
                 0.2 (0 would mean a free cast skips `apply_buff`)",
                report.buffs["window"].uptime
            );
            assert!(
                close(report.total.total_damage, 1000.0),
                "total: got {} — want 100 (strike) + 300 (bonus, overlaid) + \
                 4s × R=150 = 1000; 600 would mean the free cast applied the \
                 buff but dropped its own overlay (R=50)",
                report.total.total_damage
            );
        }

        // ══════════════════════════════════════════════════════════════════
        // The `ProcDef::actions` trigger filter.
        //
        // Two damaging 1s-cast actions in strict alternation, forced by
        // `casts.a == casts.b` on the first rule:
        //   t=0  0 == 0 → a, completing t=1
        //   t=1  1 != 0 → b, completing t=2
        //   t=2  1 == 1 → a … and so on
        // Over 20s: `a` completes at t=1,3,…,19 (10 casts) and `b` at
        // t=2,4,…,20 (10 casts, the last landing exactly at `duration`).
        // With `chance` 1 and no icd, a proc therefore fires on every
        // completion it is allowed to consider: 20 unfiltered, 10 when
        // filtered to `["a"]`.
        // ══════════════════════════════════════════════════════════════════

        fn alternating_simdef(
            filter: Option<Vec<String>>,
            trigger: Trigger,
            chance: &str,
        ) -> SimDef {
            let mut actions = BTreeMap::new();
            for name in ["a", "b"] {
                actions.insert(
                    name.to_string(),
                    damaging("1", 0.0, BTreeMap::new(), Vec::new()),
                );
            }
            let mut buffs = BTreeMap::new();
            // Inert on purpose: the proc's only observable effect is its
            // fire COUNT, so nothing the filter does can leak into damage.
            buffs.insert("tag".to_string(), plain_buff(0.5));
            let mut procs = BTreeMap::new();
            procs.insert(
                "spark".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger,
                    chance: chance.into(),
                    icd: 0.0,
                    apply_buff: Some("tag".into()),
                    effects: Vec::new(),
                    cast_action: None,
                    actions: filter,
                },
            );
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            }
        }

        fn alternating_rotation() -> Rotation {
            Rotation {
                extra: Default::default(),
                rules: vec![
                    Rule {
                        extra: Default::default(),
                        action: "a".into(),
                        when: Some("casts.a == casts.b".into()),
                    },
                    Rule {
                        extra: Default::default(),
                        action: "b".into(),
                        when: None,
                    },
                ],
            }
        }

        #[test]
        fn a_proc_action_filter_counts_only_the_listed_actions_casts() {
            let plan = scoped_plan();
            let rotation = alternating_rotation();

            let unfiltered = ev(
                &plan,
                &alternating_simdef(None, Trigger::OnCast, "1"),
                &rotation,
                20,
            );
            let filtered = ev(
                &plan,
                &alternating_simdef(Some(vec!["a".into()]), Trigger::OnCast, "1"),
                &rotation,
                20,
            );

            // The cadence itself, pinned once: the filter must not change it.
            for (label, r) in [("unfiltered", &unfiltered), ("filtered", &filtered)] {
                assert_eq!(r.actions["a"].casts, 10, "{label}");
                assert_eq!(r.actions["b"].casts, 10, "{label}");
            }

            assert_eq!(
                unfiltered.proc_counts["spark"], 20,
                "`actions: None` is every action — 10 `a` + 10 `b`"
            );
            assert_eq!(
                filtered.proc_counts["spark"], 10,
                "`actions: [\"a\"]` considers only `a`'s 10 casts — 20 would \
                 mean the filter was accepted and ignored"
            );
        }

        // The filter is documented as applying to ALL THREE triggers, since
        // all three are events OF a cast. Pinned for all three rather than
        // for `on_cast` alone: a filter wired into one arm of the trigger
        // match and not the others is exactly the shape a single-trigger
        // fixture cannot see.
        //
        // `crit_plan` with `crit_chance` 100: EV's `on_crit` weight is then
        // `Plan::crit_chance` = 1.0, so an `on_crit` proc at `chance` 1 fires
        // once per hit exactly as `on_cast`/`on_hit` do, and all three
        // triggers share the one hand-worked 20/10 count. (`scoped_plan`
        // could not host this: with no `crit` event at all, `Plan::crit_chance`
        // fail-softs to 0.0 and an `on_crit` proc never fires under any
        // filter — the test would pass vacuously.)
        #[test]
        fn a_proc_action_filter_applies_to_every_trigger_not_just_on_cast() {
            let plan = crit_plan();
            let build = crit_build();
            let rotation = alternating_rotation();

            for trigger in [Trigger::OnCast, Trigger::OnHit, Trigger::OnCrit] {
                let unfiltered = ev_with(
                    &plan,
                    &build,
                    &alternating_simdef(None, trigger, "1"),
                    &rotation,
                    20,
                );
                let filtered = ev_with(
                    &plan,
                    &build,
                    &alternating_simdef(Some(vec!["a".into()]), trigger, "1"),
                    &rotation,
                    20,
                );
                assert_eq!(
                    unfiltered.proc_counts["spark"], 20,
                    "{trigger:?} unfiltered: every one of the 20 casts qualifies"
                );
                assert_eq!(
                    filtered.proc_counts["spark"], 10,
                    "{trigger:?} filtered to [\"a\"]: only `a`'s 10 casts — 20 \
                     would mean the filter is wired into `on_cast` alone"
                );
            }
        }

        // A cast the filter excludes banks NOTHING in the EV accumulator —
        // exactly like an ICD-gated roll, and for the same reason: it is not
        // this proc's event, so its probability mass is not this proc's to
        // carry.
        //
        // Invisible at `chance` 1 (where every qualifying roll fires on the
        // spot and banking cannot change a count), so this fixture runs the
        // same alternation at `chance` 0.5 with no icd:
        //   unfiltered  20 rolls × 0.5 → the accumulator crosses 1.0 on every
        //               second roll → 10 fires
        //   filtered    10 rolls × 0.5 → 5 fires
        // A filter that gated only the FIRE while still accumulating would
        // bank all 20 rolls' worth of mass and fire far more than 5.
        #[test]
        fn a_cast_the_filter_excludes_banks_no_ev_accumulator_mass() {
            let plan = scoped_plan();
            let rotation = alternating_rotation();

            let unfiltered = ev(
                &plan,
                &alternating_simdef(None, Trigger::OnCast, "0.5"),
                &rotation,
                20,
            );
            let filtered = ev(
                &plan,
                &alternating_simdef(Some(vec!["a".into()]), Trigger::OnCast, "0.5"),
                &rotation,
                20,
            );

            assert_eq!(
                unfiltered.proc_counts["spark"], 10,
                "20 qualifying rolls × 0.5 = 10 crossings"
            );
            assert_eq!(
                filtered.proc_counts["spark"], 5,
                "only `a`'s 10 rolls × 0.5 = 5 crossings — anything above 5 \
                 means the excluded casts' mass was banked rather than \
                 discarded"
            );
        }

        // A filter that EXCLUDES nothing must be a no-op, and a filtered-out
        // cast must be skipped BEFORE the roll rather than rolled and
        // discarded. In `Mode::MonteCarlo` those two claims are one
        // observation: proc rolls and damage samples draw from the SAME
        // per-iteration `Pcg32`, so a roll that happens at all shifts every
        // later draw.
        //
        // The buff is inert, so the only way the filter can move
        // `total_damage` is through the RNG stream:
        //   listing both actions  → the same rolls as `None` → IDENTICAL
        //   listing only `a`      → 10 rolls instead of 20 → the stream
        //                           shifts → DIFFERENT
        // A filter implemented as "roll, then discard" would draw 20 times in
        // all three runs and make the third comparison identical too.
        #[test]
        fn a_proc_action_filter_consumes_no_monte_carlo_draw_for_a_cast_it_excludes() {
            let plan = crit_plan();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "crit_chance": 50.0 } }"#)
                    .unwrap();
            let rotation = alternating_rotation();
            let mc = |filter: Option<Vec<String>>| {
                let simdef = alternating_simdef(filter, Trigger::OnCast, "1");
                let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
                run(
                    &plan,
                    &sim_plan,
                    &build,
                    &dummy(20),
                    Mode::MonteCarlo {
                        iterations: 8,
                        seed: 11,
                    },
                )
                .unwrap()
            };

            let none = mc(None);
            let both = mc(Some(vec!["a".into(), "b".into()]));
            let only_a = mc(Some(vec!["a".into()]));

            assert_eq!(
                none.total.total_damage, both.total.total_damage,
                "a filter that excludes nothing must be a no-op, RNG stream \
                 included"
            );
            assert_eq!(none.proc_counts["spark"], 20);
            assert_eq!(both.proc_counts["spark"], 20);
            assert_eq!(only_a.proc_counts["spark"], 10);
            assert_ne!(
                none.total.total_damage, only_a.total.total_damage,
                "excluding `b` must remove 10 RNG draws and shift the stream \
                 — equality here would mean the filter rolls and discards"
            );
        }
    }

    // ==================================================================
    // P8b — the ORDERED effects list on procs: list order is execution
    // order, sim state is sequential between entries, and repeats apply
    // that many times. (The action-side list is the P7d `apply_buff`
    // machinery under a new field name — its order/repeat/freeze pins in
    // `mod action_scoped` all still run — so what needs NEW pins here is
    // the PROC side, where a list, and a `cast_action` mid-list, are
    // genuinely new.)
    // ==================================================================
    mod effects_list {
        use super::*;
        use crate::simdef::EffectDef;

        /// `hit = dmg` and nothing else — DoT-free, crit-free; the pins
        /// below are all uptime/stack shaped.
        fn flat_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg"],
                     "pipeline": [ { "name": "hit", "expr": "dmg" } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        fn flat_build() -> BuildState {
            serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap()
        }

        fn dummy(seconds: u32) -> Scenario {
            serde_json::from_str(&format!(
                r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
            ))
            .unwrap()
        }

        /// A 1s-cast damaging `filler` (the rotation's only rule, so hit N
        /// completes at t=N), a zero-damage utility `ping` that only the
        /// proc can cast, and one proc slot firing ONCE at t=1 (`chance`
        /// `"time == 1"`, on_cast — rolled at cast COMPLETE, so it sees
        /// exactly one instant where `time == 1`) whose `effects` list the
        /// test supplies. `timed`'s duration reads `casts.ping` LIVE at
        /// application — the probe for whether a `cast_action` earlier in
        /// the same list already landed.
        fn one_shot_simdef(effects: Vec<EffectDef>) -> SimDef {
            let mut actions = BTreeMap::new();
            actions.insert(
                "filler".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: Some(ActionDamage {
                        extra: Default::default(),
                        stats: BTreeMap::new(),
                    }),
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            actions.insert(
                "ping".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let mut buffs = BTreeMap::new();
            buffs.insert(
                "timed".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Expr("1 + casts.ping".into()),
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: None,
                    max_stacks: 1,
                    on_reapply: ReapplyPolicy::Refresh,
                },
            );
            let mut procs = BTreeMap::new();
            procs.insert(
                "echo".to_string(),
                ProcDef {
                    extra: Default::default(),
                    rolls: None,
                    trigger: Trigger::OnCast,
                    chance: "time == 1".into(),
                    icd: 0.0,
                    apply_buff: None,
                    effects,
                    cast_action: None,
                    actions: None,
                },
            );
            SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs,
                procs,
                damage_objective: "hit".into(),
            }
        }

        fn run_mode(simdef: &SimDef, mode: Mode) -> SimReport {
            let plan = flat_plan();
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "filler".into(),
                    when: None,
                }],
            };
            let sim_plan = sim_compile(&plan, simdef, &rotation).unwrap();
            run(&plan, &sim_plan, &flat_build(), &dummy(10), mode).unwrap()
        }

        fn run_ev(simdef: &SimDef) -> SimReport {
            run_mode(simdef, Mode::Expected)
        }

        // --------------------------------------------------------------
        // THE ORDER PIN. One proc fire at t=1, effects
        // `[cast_action ping, apply_buff timed]`, `timed.duration =
        // "1 + casts.ping"`, 10s fight. Hand-worked:
        //
        //   cast-first:  free-cast `ping` lands FIRST → casts.ping = 1
        //                → `timed` applies with duration 1 + 1 = 2
        //                → window [1,3) → uptime 2/10 = 0.2
        //   reversed:    `timed` applies FIRST, casts.ping still 0
        //                → duration 1 + 0 = 1 → window [1,2)
        //                → uptime 1/10 = 0.1
        //
        // The REORDER IS THE MUTATION: reversing the executor's effect
        // loop turns 0.2 into 0.1 and vice versa — nothing else in the
        // report moves (`ping` casts once either way).
        // --------------------------------------------------------------
        #[test]
        fn proc_effects_execute_in_list_order_and_a_later_entry_sees_an_earlier_cast() {
            let cast_first = run_ev(&one_shot_simdef(vec![
                EffectDef::CastAction("ping".into()),
                EffectDef::ApplyBuff("timed".into()),
            ]));
            assert_eq!(cast_first.proc_counts["echo"], 1, "one fire, at t=1");
            assert_eq!(cast_first.actions["ping"].casts, 1);
            assert!(
                close(cast_first.buffs["timed"].uptime, 0.2),
                "cast-first: got {} — want 0.2 (duration 1 + casts.ping = 2 \
                 over 10s; 0.1 would mean the apply ran before the free cast)",
                cast_first.buffs["timed"].uptime
            );

            let buff_first = run_ev(&one_shot_simdef(vec![
                EffectDef::ApplyBuff("timed".into()),
                EffectDef::CastAction("ping".into()),
            ]));
            assert_eq!(buff_first.proc_counts["echo"], 1);
            assert_eq!(buff_first.actions["ping"].casts, 1);
            assert!(
                close(buff_first.buffs["timed"].uptime, 0.1),
                "buff-first: got {} — want 0.1 (casts.ping is still 0 when \
                 `timed` applies; 0.2 would mean the list order was ignored)",
                buff_first.buffs["timed"].uptime
            );
        }

        // --------------------------------------------------------------
        // The SAME multi-entry lists under `Mode::MonteCarlo` — the
        // anti-drift pin for `run_proc_effects` being the ONE shared
        // execution path of both modes. Without this test, forking the
        // MC fire site to run only `effects.first()` left the ENTIRE
        // workspace green (P8b review probe): every pre-existing MC
        // fixture carries a one-entry sugar list, and the EV order pin
        // above never touches the MC path.
        //
        // Deterministic despite the RNG, so the EV hand-working carries
        // over exactly: `chance` is `"time == 1"`, so the Bernoulli draw
        // compares against 1.0 at t=1 (every roll in [0,1) fires) and
        // against 0.0 everywhere else (no roll fires) — the fire pattern
        // is seed-independent — and `flat_plan` has no events to sample,
        // so every iteration is the identical timeline. Pins are the EV
        // test's, re-asserted per order: cast-first 0.2 / reversed 0.1,
        // one `ping` cast each. Under a first-effect-only fork BOTH
        // halves go red: cast-first never applies `timed` (uptime 0.0),
        // reversed never free-casts `ping` (casts 0).
        // --------------------------------------------------------------
        #[test]
        fn mc_mode_executes_the_whole_effects_list_in_order() {
            let mc = Mode::MonteCarlo {
                iterations: 3,
                seed: 7,
            };
            let cast_first = run_mode(
                &one_shot_simdef(vec![
                    EffectDef::CastAction("ping".into()),
                    EffectDef::ApplyBuff("timed".into()),
                ]),
                mc,
            );
            assert_eq!(cast_first.proc_counts["echo"], 1, "one fire, at t=1");
            assert_eq!(
                cast_first.actions["ping"].casts, 1,
                "the cast_action entry must run under MC"
            );
            assert!(
                close(cast_first.buffs["timed"].uptime, 0.2),
                "MC cast-first: got {} — want the EV pin 0.2 (0.0 would mean \
                 MC ran only the FIRST effect; 0.1 that it ignored order)",
                cast_first.buffs["timed"].uptime
            );

            let buff_first = run_mode(
                &one_shot_simdef(vec![
                    EffectDef::ApplyBuff("timed".into()),
                    EffectDef::CastAction("ping".into()),
                ]),
                mc,
            );
            assert_eq!(buff_first.proc_counts["echo"], 1);
            assert_eq!(
                buff_first.actions["ping"].casts, 1,
                "the SECOND entry must run under MC — 0 means the list was \
                 truncated after the first effect"
            );
            assert!(
                close(buff_first.buffs["timed"].uptime, 0.1),
                "MC buff-first: got {} — want the EV pin 0.1",
                buff_first.buffs["timed"].uptime
            );
        }

        // --------------------------------------------------------------
        // Two DISTINCT `apply_buff` entries: both land, from the one
        // fire. `timed` (duration "1 + casts.ping" = 1, no ping in this
        // list) → [1,2) → 0.1; `steady` (duration 3) → [1,4) → 0.3.
        // --------------------------------------------------------------
        #[test]
        fn two_distinct_apply_buff_entries_both_apply() {
            let mut simdef = one_shot_simdef(vec![
                EffectDef::ApplyBuff("timed".into()),
                EffectDef::ApplyBuff("steady".into()),
            ]);
            simdef.buffs.insert(
                "steady".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(3.0),
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: None,
                    max_stacks: 1,
                    on_reapply: ReapplyPolicy::Refresh,
                },
            );
            let report = run_ev(&simdef);
            assert!(
                close(report.buffs["timed"].uptime, 0.1),
                "timed: got {}",
                report.buffs["timed"].uptime
            );
            assert!(
                close(report.buffs["steady"].uptime, 0.3),
                "steady: got {} — 0.0 would mean only the first entry ran",
                report.buffs["steady"].uptime
            );
        }

        // --------------------------------------------------------------
        // A REPEATED entry applies twice (the P7d list precedent, now on
        // the proc side). `stack` is add_independent/unbounded, duration
        // 2: one fire at t=1 applying it twice → 2 instances over [1,3) →
        // uptime 2/10 = 0.2, avg_stacks 2 × 2 / 10 = 0.4. DEDUP is the
        // mutation: one instance → avg_stacks 0.2.
        // --------------------------------------------------------------
        #[test]
        fn a_repeated_apply_buff_entry_applies_twice() {
            let mut simdef = one_shot_simdef(vec![
                EffectDef::ApplyBuff("stack".into()),
                EffectDef::ApplyBuff("stack".into()),
            ]);
            simdef.buffs.insert(
                "stack".to_string(),
                BuffDef {
                    extra: Default::default(),
                    duration: NumOrExpr::Num(2.0),
                    contributions: Vec::new(),
                    conditions: BTreeMap::new(),
                    tick_objective: None,
                    max_stacks: 0,
                    on_reapply: ReapplyPolicy::AddIndependent,
                },
            );
            let report = run_ev(&simdef);
            assert!(
                close(report.buffs["stack"].uptime, 0.2),
                "stack uptime: got {}",
                report.buffs["stack"].uptime
            );
            assert!(
                close(report.buffs["stack"].avg_stacks, 0.4),
                "stack avg_stacks: got {} — want 2 instances × 2s / 10 = 0.4 \
                 (0.2 would mean the repeated entry was deduped)",
                report.buffs["stack"].avg_stacks
            );
        }
    }

    // ==================================================================
    // P8c — WorldSnapshot measurement: `defaults.measure` /
    // `ActionDef::measure` pick the instant a cast's world is captured
    // at (`cast_complete`, the 0.3.0 default, or `cast_start`), and
    // every `Plan` evaluation in that cast's completion transaction
    // reads the ONE captured world. The scope boundary — sim-FIELD
    // expressions (`duration`/`cost`/`gain`) keep their live P7b
    // instants — is pinned by `mod expr_fields` staying byte-identical,
    // and the one-world fix itself is re-pinned in `mod action_scoped`'s
    // `a_same_list_snapshot_capture_reads_one_frozen_world`.
    // ==================================================================
    mod measurement {
        use super::*;
        use crate::scenario::Scenario;

        fn dummy(seconds: u32) -> Scenario {
            serde_json::from_str(&format!(
                r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
            ))
            .unwrap()
        }

        /// Parse-and-run helper: every fixture in this module is written
        /// as the JSON a config author would write, since the knob under
        /// test IS config surface.
        fn ev_json(
            plan: &Plan,
            build: &BuildState,
            simdef_json: &str,
            rotation_json: &str,
            seconds: u32,
        ) -> SimReport {
            let simdef: SimDef = serde_json::from_str(simdef_json).unwrap();
            let rotation: Rotation = serde_json::from_str(rotation_json).unwrap();
            let sim_plan = sim_compile(plan, &simdef, &rotation).unwrap();
            run(plan, &sim_plan, build, &dummy(seconds), Mode::Expected).unwrap()
        }

        // --------------------------------------------------------------
        // The instant itself, isolated through the sim clock: `beam` is
        // a 1s cast whose overlay sets `dmg = 10 * time`, cast
        // back-to-back over a 5s fight — starts t=0..4, completions
        // t=1..5 (the horizon drains the last one).
        //
        //   cast_complete (default):  10 × (1+2+3+4+5) = 150
        //   cast_start:               10 × (0+1+2+3+4) = 100
        //
        // The explicit `cast_complete` spelling is asserted equal to the
        // omitted-block run — the default × override discipline's
        // identity cell. The mutation is capturing at the other instant:
        // the two literals swap.
        // --------------------------------------------------------------
        #[test]
        fn the_measured_instant_moves_a_time_reading_overlay_stat() {
            let plan = minimal_plan();
            let build = minimal_build();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{ "beam": {{ "cast_time": "1",
                             "damage": {{ "stats": {{ "dmg": "10 * time" }} }} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "beam" } ] }"#;

            let omitted = ev_json(&plan, &build, &with_defaults(""), rot, 5);
            assert_eq!(omitted.actions["beam"].casts, 5);
            assert!(
                close(omitted.total.total_damage, 150.0),
                "default (cast_complete): got {} — want 10×(1+2+3+4+5)",
                omitted.total.total_damage
            );

            let explicit = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "measure": "cast_complete" },"#),
                rot,
                5,
            );
            assert!(
                close(explicit.total.total_damage, omitted.total.total_damage),
                "an explicit cast_complete must be the omitted default: {} vs {}",
                explicit.total.total_damage,
                omitted.total.total_damage
            );

            let at_start = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "measure": "cast_start" },"#),
                rot,
                5,
            );
            assert_eq!(
                at_start.actions["beam"].casts, 5,
                "the cadence is untouched"
            );
            assert!(
                close(at_start.total.total_damage, 100.0),
                "cast_start: got {} — want 10×(0+1+2+3+4), each cast measured \
                 at the instant it BEGAN",
                at_start.total.total_damage
            );
        }

        // --------------------------------------------------------------
        // The per-action override, both instants live in ONE run.
        // `early` overrides to cast_start; `late` says nothing and gets
        // the (omitted) default. Both are 1s casts on a 4s cooldown over
        // a 5s fight, rules [early, late]:
        //   t=0 early begins (cd→4), completes t=1
        //   t=1 late  begins (cd→5), completes t=2
        //   t=2 both on cooldown → Wake at 4
        //   t=4 early begins, completes t=5 (horizon, drained)
        // With `dmg = 10 * time`:
        //   early (cast_start):    10 × (0 + 4) = 40
        //   late  (cast_complete): 10 × 2       = 20
        // Mutations: resolving the override backwards gives early 60
        // (its completions 1+5); applying it globally gives late 10.
        // --------------------------------------------------------------
        #[test]
        fn a_per_action_measure_override_coexists_with_the_default() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef = r#"{
              "actions": {
                "early": { "cast_time": "1", "cooldown": 4.0,
                           "measure": "cast_start",
                           "damage": { "stats": { "dmg": "10 * time" } } },
                "late":  { "cast_time": "1", "cooldown": 4.0,
                           "damage": { "stats": { "dmg": "10 * time" } } }
              },
              "damage_objective": "hit" }"#;
            let rot = r#"{ "rules": [ { "action": "early" }, { "action": "late" } ] }"#;
            let report = ev_json(&plan, &build, simdef, rot, 5);
            assert_eq!(report.actions["early"].casts, 2);
            assert_eq!(report.actions["late"].casts, 1);
            assert!(
                close(report.actions["early"].damage, 40.0),
                "early (cast_start): got {} — want 10×(0+4)",
                report.actions["early"].damage
            );
            assert!(
                close(report.actions["late"].damage, 20.0),
                "late (default): got {} — want 10×2, measured at completion",
                report.actions["late"].damage
            );
        }

        // --------------------------------------------------------------
        // `casts.<self>` at the two instants. Under the default the
        // overlay evaluates AFTER the completing cast is counted (P7b:
        // counts from 1, never 0); under cast_start the in-flight cast
        // has not been counted yet. `pulse` is a 1s cast over 3s, `dmg =
        // 100 * casts.pulse`:
        //   cast_complete: 100 × (1+2+3) = 600
        //   cast_start:    100 × (0+1+2) = 300
        // --------------------------------------------------------------
        #[test]
        fn casts_self_excludes_the_in_flight_cast_under_cast_start() {
            let plan = minimal_plan();
            let build = minimal_build();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{ "pulse": {{ "cast_time": "1",
                             "damage": {{ "stats": {{ "dmg": "100 * casts.pulse" }} }} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "pulse" } ] }"#;
            let complete = ev_json(&plan, &build, &with_defaults(""), rot, 3);
            assert!(
                close(complete.total.total_damage, 600.0),
                "default: got {} — want 100×(1+2+3), the completing cast \
                 included (the documented P7b reading)",
                complete.total.total_damage
            );
            let start = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "measure": "cast_start" },"#),
                rot,
                3,
            );
            assert!(
                close(start.total.total_damage, 300.0),
                "cast_start: got {} — want 100×(0+1+2), the in-flight cast \
                 NOT yet counted",
                start.total.total_damage
            );
        }

        // --------------------------------------------------------------
        // The free-cast boundary: a proc-fired `cast_action` free cast
        // inside a measured cast's transaction measures at ITS OWN
        // instant, live — never the outer cast's snapshot.
        //
        // `hit = dmg * boost` (product bucket). `striker` is a one-shot
        // 1s cast measured at CAST START (t=0 — nothing live, boost 1).
        // Its completion transaction at t=1 rolls `gift`, whose effects
        // list first applies `empower` (+100 to `boost` → ×2) and THEN
        // free-casts `comet`:
        //   striker = 100 × 1 = 100   (its own snapshot, from t=0)
        //   comet   = 100 × 2 = 200   (live ambient at the fire, WITH
        //                              empower — sequential proc effects,
        //                              the P8b rule)
        // Mutation: freezing the free cast to the OUTER cast's snapshot
        // reads boost 1 → comet 100.
        // --------------------------------------------------------------
        #[test]
        fn a_free_cast_measures_live_ambient_not_the_outer_casts_snapshot() {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg"],
                     "buckets": { "boost": { "fold": "product" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * boost" } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            let plan = plan::compile(&def).unwrap();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
            let simdef = r#"{
              "actions": {
                "striker": { "cast_time": "1", "cooldown": 1000.0,
                             "measure": "cast_start",
                             "damage": { "stats": {} } },
                "comet":   { "cast_time": "0", "damage": { "stats": {} } }
              },
              "buffs": {
                "empower": { "duration": 100.0,
                             "contributions": [ { "bucket": "boost", "value": 100.0 } ] }
              },
              "procs": {
                "gift": { "trigger": "on_cast", "chance": "1",
                          "actions": ["striker"],
                          "effects": [ { "apply_buff": "empower" },
                                       { "cast_action": "comet" } ] }
              },
              "damage_objective": "hit" }"#;
            let rot = r#"{ "rules": [ { "action": "striker" } ] }"#;
            let report = ev_json(&plan, &build, simdef, rot, 10);
            assert_eq!(report.actions["striker"].casts, 1);
            assert_eq!(report.actions["comet"].casts, 1);
            assert!(
                close(report.actions["striker"].damage, 100.0),
                "striker: got {} — its cast-start snapshot predates empower",
                report.actions["striker"].damage
            );
            assert!(
                close(report.actions["comet"].damage, 200.0),
                "comet: got {} — a free cast measures the LIVE ambient world \
                 at its own instant (100 would mean it was frozen to the \
                 outer cast's snapshot)",
                report.actions["comet"].damage
            );
        }

        // --------------------------------------------------------------
        // `hits_per_use` is part of the measurement, at the SAME instant
        // as the rest of it (fix-round pin: re-evaluating it at
        // completion survived every other test, because no fixture gave
        // it a time-dependent expression). `volley` is a 1s cast with a
        // LITERAL `dmg: 10` and `hits_per_use: "time"` over 5s — the
        // hits channel is the only thing that can move:
        //   cast_complete (default): 10 × (1+2+3+4+5) = 150
        //   cast_start:              10 × (0+1+2+3+4) = 100
        // Mutation: read `hits_per_use` at completion under cast_start →
        // the 100 becomes 150.
        // --------------------------------------------------------------
        #[test]
        fn hits_per_use_is_measured_at_the_same_instant_as_the_overlay() {
            let plan = minimal_plan();
            let build = minimal_build();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{ "volley": {{ "cast_time": "1",
                             "damage": {{ "stats": {{ "dmg": 10.0,
                                                      "hits_per_use": "time" }} }} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "volley" } ] }"#;
            let complete = ev_json(&plan, &build, &with_defaults(""), rot, 5);
            assert!(
                close(complete.total.total_damage, 150.0),
                "default: got {} — want 10×(1+2+3+4+5), hits read at the \
                 completion instants",
                complete.total.total_damage
            );
            let start = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "measure": "cast_start" },"#),
                rot,
                5,
            );
            assert!(
                close(start.total.total_damage, 100.0),
                "cast_start: got {} — want 10×(0+1+2+3+4); the hit count is \
                 part of the ONE measurement, taken at the measured instant \
                 (150 would mean hits_per_use was re-read at completion)",
                start.total.total_damage
            );
        }

        // --------------------------------------------------------------
        // The cast-start capture's INTRA-instant position: AFTER the cost
        // is paid (and the cooldown armed) — the world the cast leaves
        // behind as it starts, exactly as `Measure::CastStart`'s rustdoc
        // states. `caster` costs 40 mana of a 100-cap pool regenerating
        // 5/s, casts once (1000s cooldown), and its overlay reads the
        // paying resource: `dmg = mana`.
        //   cast_start:  measured at t=0, post-cost → 100 − 40 = 60
        //   pre-cost mutant:                          100
        //   cast_complete (the default contrast): t=1, post-cost plus 1s
        //     of regen → 60 + 5 = 65
        // The three values are pairwise distinct, so this pins the
        // position within the instant AND (again) the instant itself.
        // --------------------------------------------------------------
        #[test]
        fn the_cast_start_capture_is_taken_after_the_cost_is_paid() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef = |measure: &str| {
                format!(
                    r#"{{ "resources": {{ "mana": {{ "max": "100",
                                                     "regen_per_sec": "5" }} }},
                         "actions": {{ "caster": {{ "cast_time": "1",
                             "cooldown": 1000.0, {measure}
                             "cost": {{ "mana": 40.0 }},
                             "damage": {{ "stats": {{ "dmg": "mana" }} }} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "caster" } ] }"#;
            let start = ev_json(
                &plan,
                &build,
                &simdef(r#""measure": "cast_start","#),
                rot,
                10,
            );
            assert_eq!(start.actions["caster"].casts, 1);
            assert!(
                close(start.actions["caster"].damage, 60.0),
                "cast_start: got {} — want 100 − 40, the POST-cost pool \
                 (100 would mean the capture ran before pay_cost; 65 that \
                 it ran at completion)",
                start.actions["caster"].damage
            );
            let complete = ev_json(&plan, &build, &simdef(""), rot, 10);
            assert!(
                close(complete.actions["caster"].damage, 65.0),
                "default: got {} — want 60 + 5×1s regen at completion",
                complete.actions["caster"].damage
            );
        }

        // --------------------------------------------------------------
        // The DAMAGE query's phase half under cast_start, in-suite (the
        // example-level 1837.5 → 2175 contrast in `poe2_triggers` was
        // the only witness before this pin — every other measurement
        // fixture here is condition-free). `tag` (instant, utility)
        // applies `mark` (drives `marked`, 0.5s) and the decision chain
        // then begins `beam` (1s, cast_start) at the SAME t=0 — so mark
        // is live at beam's cast start and EXPIRED by its completion.
        //   hit = dmg × (1 + marked), dmg 100:
        //   cast_start:              measured at t=0, marked 1 → 200
        //   cast_complete (default): measured at t=1, marked 0 → 100
        // Mutation: evaluating beam's damage against the LIVE phase at
        // completion (instead of the snapshot's) sends the 200 to 100.
        // --------------------------------------------------------------
        #[test]
        fn the_damage_querys_phase_half_reads_the_snapshot_under_cast_start() {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg"],
                     "conditions": ["marked"],
                     "pipeline": [ { "name": "hit", "expr": "dmg * (1 + marked)" } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            let plan = plan::compile(&def).unwrap();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
            let simdef = |measure: &str| {
                format!(
                    r#"{{ "actions": {{
                           "tag":  {{ "cast_time": "0", "cooldown": 1000.0,
                                      "effects": [ {{ "apply_buff": "mark" }} ] }},
                           "beam": {{ "cast_time": "1", "cooldown": 1000.0, {measure}
                                      "damage": {{ "stats": {{}} }} }} }},
                         "buffs": {{ "mark": {{ "duration": 0.5,
                                                "conditions": {{ "marked": 1.0 }} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "tag" }, { "action": "beam" } ] }"#;
            let start = ev_json(
                &plan,
                &build,
                &simdef(r#""measure": "cast_start","#),
                rot,
                10,
            );
            assert!(
                close(start.actions["beam"].damage, 200.0),
                "cast_start: got {} — want 100 × (1 + 1), the snapshot's \
                 phase carrying `marked` from t=0 (100 would mean the \
                 damage query read the LIVE phase at completion, where \
                 mark is long expired)",
                start.actions["beam"].damage
            );
            let complete = ev_json(&plan, &build, &simdef(""), rot, 10);
            assert!(
                close(complete.actions["beam"].damage, 100.0),
                "default: got {} — mark expired at t=0.5, before the \
                 completion measurement",
                complete.actions["beam"].damage
            );
        }

        // --------------------------------------------------------------
        // An INSTANT cast is measured at the completion position even
        // under cast_start — the two share the wall-clock instant, and
        // the completion transaction's intra-instant position wins
        // (post-`gain`, post-`casts` increment). The discriminating
        // observable is `casts.<self>` counting from 1: `pulse` is a
        // ZERO-time cast on a 1s cooldown over 3s (casts at t=0,1,2 —
        // the t=3 wake is at the horizon, where nothing begins), with
        // `dmg = 100 * casts.pulse`:
        //   either measure: 100 × (1+2+3) = 600
        // and the equality with the default run is asserted alongside
        // the literal. Mutation: capturing at the begin position for an
        // instant cast (pre-increment) → 100 × (0+1+2) = 300. NB the
        // epsilon-cast_time discontinuity is deliberate and documented
        // on `Measure`: at cast_time 1 this same expression pins 300
        // (`casts_self_excludes_the_in_flight_cast_under_cast_start`).
        // --------------------------------------------------------------
        #[test]
        fn an_instant_cast_is_measured_at_the_completion_position_even_under_cast_start() {
            let plan = minimal_plan();
            let build = minimal_build();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{ "pulse": {{ "cast_time": "0", "cooldown": 1.0,
                             "damage": {{ "stats": {{ "dmg": "100 * casts.pulse" }} }} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "pulse" } ] }"#;
            let start = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "measure": "cast_start" },"#),
                rot,
                3,
            );
            assert_eq!(start.actions["pulse"].casts, 3);
            assert!(
                close(start.total.total_damage, 600.0),
                "instant + cast_start: got {} — want 100×(1+2+3), the \
                 completion position (300 would mean the capture ran at \
                 the begin position, pre-increment)",
                start.total.total_damage
            );
            let complete = ev_json(&plan, &build, &with_defaults(""), rot, 3);
            assert!(
                close(start.total.total_damage, complete.total.total_damage),
                "for an instant cast the two measures must be the same \
                 number: {} vs {}",
                start.total.total_damage,
                complete.total.total_damage
            );
        }

        // --------------------------------------------------------------
        // The EV `on_crit` weight under cast_start: part of the ONE
        // measurement, read off the snapshot's build and phase at the
        // measured instant. `zapper`'s overlay sets
        // `crit_chance = 100 − 100 * time`, so at its cast start (t=0)
        // the hit is a CERTAIN crit and at its completion (t=1) it
        // cannot crit at all; the base build's crit_chance is 0.
        //   damage: 100 × (1 + P(crit)×(factor−1)) = 100 × 2 = 200
        //   `surge` (on_crit, chance 1): acc += 1 × weight 1 → fires → 1
        // Mutation: evaluating the crit weight at completion against the
        // live world → weight 0 → surge never fires (and 100 damage
        // would mean the damage query moved too).
        // --------------------------------------------------------------
        #[test]
        fn the_ev_on_crit_weight_is_part_of_the_cast_start_measurement() {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg", "crit_chance"],
                     "events": { "crit": { "chance": "crit_chance / 100",
                                            "factor": "2" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * event_factors",
                                     "branched": true } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            let plan = plan::compile(&def).unwrap();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "crit_chance": 0.0 } }"#)
                    .unwrap();
            let simdef = r#"{
              "actions": {
                "zapper": { "cast_time": "1", "cooldown": 1000.0,
                            "measure": "cast_start",
                            "damage": { "stats": { "crit_chance": "100 - 100 * time" } } }
              },
              "buffs": { "spark": { "duration": 1.0 } },
              "procs": {
                "surge": { "trigger": "on_crit", "chance": "1",
                           "effects": [ { "apply_buff": "spark" } ] }
              },
              "damage_objective": "hit" }"#;
            let rot = r#"{ "rules": [ { "action": "zapper" } ] }"#;
            let report = ev_json(&plan, &build, simdef, rot, 10);
            assert!(
                close(report.actions["zapper"].damage, 200.0),
                "zapper: got {} — want 100 × 2, a certain crit in the \
                 cast-start world",
                report.actions["zapper"].damage
            );
            assert_eq!(
                report.proc_counts["surge"], 1,
                "surge must fire once: the on_crit weight is the \
                 snapshot's (0 fires would mean the weight was read at \
                 completion, where the overlay's crit_chance is 0)"
            );
        }
    }

    // ==================================================================
    // P8d — `defaults.event_order`: which of two COINCIDENT queue
    // entries resolves first. `scheduled` (default) is the 0.3.0
    // `(time, seq)` order, bit-identical — proven by the untouched
    // suite plus the byte-identical `diablo4_rotation` MC block;
    // `completions_first` ranks every `CastComplete` ahead of every
    // coincident `BuffExpire`/`PhaseBoundary`/`Wake`, `seq` breaking
    // ties within a class. Implementation is rank-at-push: `QueueItem`
    // carries a `class_rank` computed from (event kind, the SimPlan's
    // order) when the item is pushed, and `Ord` is `(time, class_rank,
    // seq)` — see `class_rank`'s doc comment.
    // ==================================================================
    mod event_order {
        use super::*;
        use crate::scenario::Scenario;

        fn dummy(seconds: u32) -> Scenario {
            serde_json::from_str(&format!(
                r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
            ))
            .unwrap()
        }

        /// Parse-and-run helper (EV mode) — as in `mod measurement`,
        /// every fixture is the JSON a config author would write, since
        /// the knob under test IS config surface.
        fn ev_json(
            plan: &Plan,
            build: &BuildState,
            simdef_json: &str,
            rotation_json: &str,
            scenario: &Scenario,
        ) -> SimReport {
            let simdef: SimDef = serde_json::from_str(simdef_json).unwrap();
            let rotation: Rotation = serde_json::from_str(rotation_json).unwrap();
            let sim_plan = sim_compile(plan, &simdef, &rotation).unwrap();
            run(plan, &sim_plan, build, scenario, Mode::Expected).unwrap()
        }

        /// `hit = dmg * boost` — a product bucket a buff can double, so
        /// "did the cast measure WITH its buff?" is a factor of 2 in the
        /// damage number.
        fn boost_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg"],
                     "buckets": { "boost": { "fold": "product" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * boost" } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        // --------------------------------------------------------------
        // THE CAST-GRID COLLISION, ordering flavor — the exec-level twin
        // of `examples/poe2_triggers.rs`' shock-at-2.0 contrast (P7e
        // footgun; `sim` module docs, "A buff expiring on the cast
        // grid").
        //
        // `bolt` (1s cast, 2s cooldown) applies `charge` (duration 2.0 —
        // EXACTLY the cast cadence; ×2 via the product bucket) at each
        // completion, under the default `cast_complete` measure. Bolts
        // begin t=0,2,4,6,8 and complete t=1,3,5,7,9 (10s fight, 5
        // casts); the `charge` applied at completion t expires at t+2 —
        // the NEXT completion's instant. Both queue entries share that
        // instant, and the `BuffExpire` was scheduled at the PREVIOUS
        // application:
        //
        //   scheduled (default): the expiry holds the lower `seq` and
        //     resolves first — every bolt measures BARE, then re-applies
        //     the buff it just lost:            5 × 100        =  500
        //   completions_first: the completion outranks the expiry — the
        //     bolt measures WITH its still-live charge, and its
        //     reapplication bumps the buff generation, so the coincident
        //     expiry is STALE (a no-op):        100 + 4 × 200  =  900
        //
        // The uptime is 0.9 in BOTH runs (live [1, 10] — the window
        // never visibly lapses either way: zero-width gap under
        // `scheduled`, stale expiry under `completions_first`). That
        // restates the footgun's signature — the integrated columns
        // cannot see the collision; only damage moves.
        //
        // The explicit `"scheduled"` run is asserted equal to the
        // omitted-block run — the default × override identity cell.
        // Mutations this kills: flip the rank table (completions rank 1,
        // the rest 0) and the 900 collapses to 500; drop the rank from
        // `QueueItem::cmp` and likewise.
        // --------------------------------------------------------------
        #[test]
        fn completions_first_lets_a_cast_measure_the_buff_it_refreshes_on_the_grid() {
            let plan = boost_plan();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{ "bolt": {{ "cast_time": "1", "cooldown": 2.0,
                             "damage": {{ "stats": {{}} }},
                             "apply_buff": ["charge"] }} }},
                         "buffs": {{ "charge": {{ "duration": 2.0,
                             "contributions": [ {{ "bucket": "boost", "value": 100.0 }} ] }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "bolt" } ] }"#;
            let ten = dummy(10);

            let omitted = ev_json(&plan, &build, &with_defaults(""), rot, &ten);
            assert_eq!(omitted.actions["bolt"].casts, 5);
            assert!(
                close(omitted.total.total_damage, 500.0),
                "default (scheduled): got {} — want 5×100, every bolt \
                 measured bare (the expiry resolved first)",
                omitted.total.total_damage
            );

            let explicit = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "event_order": "scheduled" },"#),
                rot,
                &ten,
            );
            assert!(
                close(explicit.total.total_damage, omitted.total.total_damage),
                "an explicit `scheduled` must be the omitted default: {} vs {}",
                explicit.total.total_damage,
                omitted.total.total_damage
            );

            let cf = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "event_order": "completions_first" },"#),
                rot,
                &ten,
            );
            assert_eq!(cf.actions["bolt"].casts, 5, "the cadence is untouched");
            assert!(
                close(cf.total.total_damage, 900.0),
                "completions_first: got {} — want 100 + 4×200, every bolt \
                 after the first measured WITH its still-live charge",
                cf.total.total_damage
            );
            for (label, r) in [("scheduled", &omitted), ("completions_first", &cf)] {
                assert!(
                    close(r.buffs["charge"].uptime, 0.9),
                    "{label}: uptime must be 0.9 — the integrated column \
                     cannot see the collision in EITHER order: got {}",
                    r.buffs["charge"].uptime
                );
            }
        }

        // --------------------------------------------------------------
        // THE ZERO-WEIGHT-FINAL-PHASE FLIP — the `completions_first`
        // cell of `mod horizon`'s
        // `a_zero_weight_final_phase_takes_the_horizon_cast_by_the_seq_rule`
        // (whose 900 / 250 / 1150 pin stays green untouched: that IS the
        // `scheduled` cell).
        //
        // Same fixture: phases `[main: 10, epilogue: 0]`, epilogue
        // overriding `dmg` to 250, a 1s `filler` spammed, so a
        // `PhaseBoundary` (scheduled at construction — the far lower
        // `seq`) and the 10th `CastComplete` (scheduled at t=9) share
        // t=10. Under `completions_first` the completion outranks the
        // boundary regardless of `seq`, so the 10th cast is measured
        // under — and attributed to — `main`:
        //
        //   main     = casts at t=1..10 = 10 × 100 = 1000, dps 100
        //   epilogue = nothing          =        0, dps 0
        //   total    = 1000 over 10s    = 100 dps, 10 casts
        //
        // The 0.3.0 pin called the old attribution "a CONSEQUENCE of
        // draining the horizon in scheduling order, not a designed
        // choice". THIS attribution is the designed one: the knob's
        // whole meaning is that a completion beats a coincident
        // boundary, and the flipped cell is pinned as such (spec P8d).
        // Horizon-drain semantics are unchanged — the cast still
        // RESOLVES either way; only its measuring phase moves.
        // --------------------------------------------------------------
        #[test]
        fn a_zero_weight_final_phase_cast_flips_to_the_old_phase_under_completions_first() {
            let plan = minimal_plan();
            let build = minimal_build();
            let scenario: Scenario = serde_json::from_str(
                r#"{ "phases": [ { "name": "main", "weight": 10 },
                                 { "name": "epilogue", "weight": 0,
                                   "stats": { "dmg": 250.0 } } ] }"#,
            )
            .unwrap();
            let simdef = r#"{
              "defaults": { "event_order": "completions_first" },
              "actions": { "filler": { "cast_time": "1",
                                       "damage": { "stats": {} } } },
              "damage_objective": "hit" }"#;
            let rot = r#"{ "rules": [ { "action": "filler" } ] }"#;
            let report = ev_json(&plan, &build, simdef, rot, &scenario);

            assert_eq!(report.actions["filler"].casts, 10);
            assert!(
                close(report.phases[0].total_damage, 1000.0),
                "main: got {} — want 10×100, the horizon cast measured \
                 under the OLD phase (900 would mean the boundary still \
                 resolved first)",
                report.phases[0].total_damage
            );
            assert!(
                close(report.phases[0].dps, 100.0),
                "got {}",
                report.phases[0].dps
            );
            assert!(
                close(report.phases[1].total_damage, 0.0),
                "epilogue: got {} — want 0, nothing lands in the \
                 zero-width phase under this ordering",
                report.phases[1].total_damage
            );
            assert!(
                close(report.phases[1].dps, 0.0),
                "got {}",
                report.phases[1].dps
            );
            assert!(
                close(report.total.total_damage, 1000.0),
                "got {}",
                report.total.total_damage
            );
            assert!(close(report.total.dps, 100.0), "got {}", report.total.dps);
        }

        // --------------------------------------------------------------
        // SEEDED MC DETERMINISM under `completions_first`: `seq` still
        // breaks every residual tie (within a class), so the event
        // order — and therefore the RNG draw order — stays a pure
        // function of (config, seed). The fixture is the cast-grid
        // collision above PLUS a fractional-chance proc, so the run has
        // real Bernoulli draws AND exercises the reordered instants.
        // Same seed twice → byte-identical serialized report.
        // --------------------------------------------------------------
        #[test]
        fn mc_same_seed_twice_is_byte_identical_under_completions_first() {
            let plan = boost_plan();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0 } }"#).unwrap();
            let simdef_json = r#"{
              "defaults": { "event_order": "completions_first" },
              "actions": { "bolt": { "cast_time": "1", "cooldown": 2.0,
                  "damage": { "stats": {} },
                  "apply_buff": ["charge"] } },
              "buffs": { "charge": { "duration": 2.0,
                  "contributions": [ { "bucket": "boost", "value": 100.0 } ] },
                         "glow": { "duration": 0.5 } },
              "procs": { "spark": { "trigger": "on_cast", "chance": "0.5",
                                    "apply_buff": "glow" } },
              "damage_objective": "hit" }"#;
            let simdef: SimDef = serde_json::from_str(simdef_json).unwrap();
            let rotation: Rotation =
                serde_json::from_str(r#"{ "rules": [ { "action": "bolt" } ] }"#).unwrap();
            let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
            let mc = || {
                run(
                    &plan,
                    &sim_plan,
                    &build,
                    &dummy(10),
                    Mode::MonteCarlo {
                        iterations: 16,
                        seed: 7,
                    },
                )
                .unwrap()
            };
            let a = serde_json::to_string(&mc()).unwrap();
            let b = serde_json::to_string(&mc()).unwrap();
            assert_eq!(a, b, "same seed twice must be byte-identical");
        }

        // --------------------------------------------------------------
        // WITHIN THE REST CLASS, `seq` STILL DECIDES — the pin behind
        // the "within a class, `seq` still decides" sentence the docs
        // state in three places ([`EventOrder`], `class_rank`, the
        // CHANGELOG). Without it, splitting the rest class into
        // sub-ranks (`BuffExpire → 1, PhaseBoundary → 2, Wake → 3`)
        // passed the ENTIRE suite: no fixture made two REST-class
        // events at one instant observably order-dependent under
        // `completions_first` (review probe — the 8th consecutive
        // surviving mutation on documented-but-unpinned semantics).
        //
        // The fixture makes a `BuffExpire` and a `PhaseBoundary` share
        // an instant, with an INSTANT cast whose eligibility flips at
        // the expiry and whose damage names the phase it measured
        // under:
        //
        //   phases [p1: 5, p2: 5], p2 overriding `dmg` 100 → 250
        //   `opener` (instant, cd 1000): applies `x` (duration 5) at t=0
        //   `spike`  (instant, cd 1000, `when: buff.x == 0`): damage
        //
        //   t=0  opener casts instantly, applies x → BuffExpire at t=5
        //        (seq AFTER the construction-scheduled boundary's);
        //        spike gated (buff.x is 1); idle otherwise.
        //   t=5  the coincidence, both events rest-class (rank 1) under
        //        `completions_first`, so `seq` decides:
        //          boundary (lower seq) → phase p2 becomes current; the
        //            retry still finds spike gated — `buff.x` reads the
        //            INSTANCE LIST, which the expiry event has not yet
        //            dropped (event-driven, not a time comparison);
        //          expiry → x drops → retry: spike casts instantly,
        //            measured under p2 → 250, attributed to p2.
        //
        //   → spike 250, phases [0, 250], total 250 over 10s = 25 dps.
        //
        // The `scheduled` run is asserted EQUAL: within the rest class
        // the `completions_first` order IS the scheduled order — that
        // equality is the claim under pin. Under the rank split above,
        // the expiry (sub-rank 1) beats the boundary (sub-rank 2), the
        // spike fires under p1 → 100, attributed to p1 — both the
        // literal and the equality die. Mutation-proven with exactly
        // that split.
        // --------------------------------------------------------------
        #[test]
        fn within_the_rest_class_seq_still_decides_under_completions_first() {
            let plan = minimal_plan();
            let build = minimal_build();
            let scenario: Scenario = serde_json::from_str(
                r#"{ "phases": [ { "name": "p1", "weight": 5 },
                                 { "name": "p2", "weight": 5,
                                   "stats": { "dmg": 250.0 } } ] }"#,
            )
            .unwrap();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{
                           "opener": {{ "cast_time": "0", "cooldown": 1000.0,
                                        "apply_buff": ["x"] }},
                           "spike":  {{ "cast_time": "0", "cooldown": 1000.0,
                                        "damage": {{ "stats": {{}} }} }}
                         }},
                         "buffs": {{ "x": {{ "duration": 5.0 }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "opener" },
                                      { "action": "spike",
                                        "when": "buff.x == 0" } ] }"#;
            let cf = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "event_order": "completions_first" },"#),
                rot,
                &scenario,
            );
            assert_eq!(cf.actions["spike"].casts, 1);
            assert!(
                close(cf.actions["spike"].damage, 250.0),
                "spike: got {} — want 250: the boundary (lower seq) must \
                 resolve before the coincident expiry, so the expiry's \
                 retry casts spike under p2 (100 would mean the rest \
                 class was sub-ranked and the expiry jumped the queue)",
                cf.actions["spike"].damage
            );
            assert!(
                close(cf.phases[0].total_damage, 0.0) && close(cf.phases[1].total_damage, 250.0),
                "attribution must follow: p1 {} / p2 {}",
                cf.phases[0].total_damage,
                cf.phases[1].total_damage
            );
            // Within the rest class, `completions_first` IS the
            // scheduled order — the equality is the documented claim.
            let sched = ev_json(&plan, &build, &with_defaults(""), rot, &scenario);
            assert_eq!(
                serde_json::to_string(&sched).unwrap(),
                serde_json::to_string(&cf).unwrap(),
                "two coincident rest-class events must resolve by seq \
                 under BOTH settings"
            );
        }

        // --------------------------------------------------------------
        // THE `Wake` CLASS ASSIGNMENT — stated honestly: it has NO
        // observable consequence, and this test pins WHY-shaped
        // evidence, not a behavioral difference, because none can be
        // constructed. A `Wake` coincident with a `CastComplete` means a
        // cast is in flight at that instant (it completes there), so
        // `mid_cast` is true until the completion is processed:
        //
        //   - Wake first (scheduled): its decision retry hits the
        //     `mid_cast` early-return in `attempt_decision` — a no-op.
        //   - Completion first (completions_first): the completion's own
        //     post-event retry already ran; the Wake's retry re-runs the
        //     identical decision against unchanged state (a `Wake`
        //     mutates nothing) — idempotent.
        //
        // So the assignment `Wake → rank 1` is pinned as HARMLESS rather
        // than observable: a fixture that engineers the coincidence
        // produces byte-identical reports under both orders. The
        // completions-first-beats-Wake claim is real but invisible;
        // what IS behaviorally visible for the knob is the
        // BuffExpire/PhaseBoundary evidence in the two tests above.
        //
        // The coincidence, hand-worked (10s fight, `hit = dmg` = 100):
        //   `nova`   1s cast, 6s cooldown, applies `gate` (duration 1)
        //   `filler` 4s cast, `when: buff.gate == 0`
        //   t=0  decision: nova ready → begin (cd ready at 6)
        //   t=1  nova completes (100), applies gate (expires t=2) →
        //        retry: nova on cd (candidate 6), filler `when` false →
        //        Wake SCHEDULED AT t=6            (the lower seq)
        //   t=2  gate expires → retry: filler eligible → begin, 4s cast
        //        → CastComplete SCHEDULED AT t=6  (the higher seq)
        //   t=6  the coincidence: Wake vs filler's completion (100).
        //        Either order: the completion's retry begins nova
        //        (cd_ready 6 <= 6), the Wake's retry no-ops.
        //   t=7  nova completes (100), applies gate → wake at cd 12
        //   t=8  gate expires → filler begins, would complete t=12 —
        //        past the horizon, never counted.
        //   → nova 2 casts, filler 1, total 300, both orders.
        // --------------------------------------------------------------
        #[test]
        fn a_wake_coinciding_with_a_completion_is_ordering_invisible_by_construction() {
            let plan = minimal_plan();
            let build = minimal_build();
            let with_defaults = |block: &str| {
                format!(
                    r#"{{ {block}
                         "actions": {{
                           "nova":   {{ "cast_time": "1", "cooldown": 6.0,
                                        "damage": {{ "stats": {{}} }},
                                        "apply_buff": ["gate"] }},
                           "filler": {{ "cast_time": "4",
                                        "damage": {{ "stats": {{}} }} }}
                         }},
                         "buffs": {{ "gate": {{ "duration": 1.0 }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let rot = r#"{ "rules": [ { "action": "nova" },
                                      { "action": "filler",
                                        "when": "buff.gate == 0" } ] }"#;
            let ten = dummy(10);
            let scheduled = ev_json(&plan, &build, &with_defaults(""), rot, &ten);
            let cf = ev_json(
                &plan,
                &build,
                &with_defaults(r#""defaults": { "event_order": "completions_first" },"#),
                rot,
                &ten,
            );
            // The derived timeline actually ran (nova's second cast can
            // only begin at exactly t=6 — the engineered instant).
            assert_eq!(scheduled.actions["nova"].casts, 2);
            assert_eq!(scheduled.actions["filler"].casts, 1);
            assert!(
                close(scheduled.total.total_damage, 300.0),
                "got {}",
                scheduled.total.total_damage
            );
            assert_eq!(
                serde_json::to_string(&scheduled).unwrap(),
                serde_json::to_string(&cf).unwrap(),
                "a Wake/CastComplete coincidence must be invisible to the \
                 ordering knob — the wake's retry is mid_cast-gated before \
                 the completion and idempotent after it"
            );
        }
    }

    // ==================================================================
    // P8e — `proc_rolls` (`per_cast` | `per_hit`): how a proc's chance
    // is rolled against one damaging cast's hits. Every fixture is the
    // JSON a config author would write (the `mod event_order` style —
    // the knob under test IS config surface). The shared cadence: `nova`
    // (1s cast, `hits_per_use` 5) spammed for `weight` seconds → casts
    // complete at t=1..weight, 5 measured hits each.
    // ==================================================================
    mod proc_rolls {
        use super::*;
        use crate::scenario::Scenario;

        fn dummy(seconds: u32) -> Scenario {
            serde_json::from_str(&format!(
                r#"{{ "phases": [ {{ "name": "p", "weight": {seconds} }} ] }}"#
            ))
            .unwrap()
        }

        fn compile_json(plan: &Plan, simdef_json: &str, rotation_json: &str) -> SimPlan {
            let simdef: SimDef = serde_json::from_str(simdef_json).unwrap();
            let rotation: Rotation = serde_json::from_str(rotation_json).unwrap();
            sim_compile(plan, &simdef, &rotation).unwrap()
        }

        fn ev_json(
            plan: &Plan,
            build: &BuildState,
            simdef_json: &str,
            rotation_json: &str,
            scenario: &Scenario,
        ) -> SimReport {
            let sim_plan = compile_json(plan, simdef_json, rotation_json);
            run(plan, &sim_plan, build, scenario, Mode::Expected).unwrap()
        }

        const NOVA_ROT: &str = r#"{ "rules": [ { "action": "nova" } ] }"#;

        /// The shared 5-hit spam fixture: `nova` under `minimal_plan`
        /// (`hit = dmg`), one `glow` buff for procs to apply, and the
        /// caller's `defaults` block + `procs` map spliced in.
        fn nova_simdef(defaults: &str, procs: &str) -> String {
            format!(
                r#"{{ {defaults}
                     "actions": {{ "nova": {{ "cast_time": "1",
                         "damage": {{ "stats": {{ "hits_per_use": 5 }} }} }} }},
                     "buffs": {{ "glow": {{ "duration": 0.5 }} }},
                     "procs": {procs},
                     "damage_objective": "hit" }}"#
            )
        }

        /// A branched plan for the `on_crit` fixtures: `hit = dmg ×
        /// event_factors`, one `crit` event whose chance is the bare
        /// `cc` stat (so the build sets P(crit) directly).
        fn crit_plan() -> Plan {
            let def: GameDef = serde_json::from_str(
                r#"{ "stats": ["dmg", "cc"],
                     "events": { "crit": { "chance": "cc", "factor": "1.5" } },
                     "pipeline": [ { "name": "hit", "expr": "dmg * event_factors",
                                     "branched": true } ],
                     "objectives": ["hit"] }"#,
            )
            .unwrap();
            plan::compile(&def).unwrap()
        }

        // --------------------------------------------------------------
        // THE EV FRACTIONAL PIN (the P7 vacuity lesson: chance 1 cannot
        // discriminate accumulator semantics). on_hit chance 0.2, icd 0,
        // hits_per_use 5, 20 casts (t=1..20):
        //
        //   per_cast — ONE roll per cast, hits-blind: acc += 0.2/cast,
        //     crossings at casts 5, 10, 15, 20            →  4 fires
        //   per_hit  — one roll per HIT: acc += 0.2/hit = +1.0/cast,
        //     a crossing at the 5th hit of EVERY cast     → 20 fires
        //
        //   (f64 footnote: the 0.2 chain is EXACT here — five additions
        //   of the f64 nearest 0.2 land on exactly 1.0, `acc -= 1.0`
        //   returns exactly 0.0, and the sequence repeats without drift
        //   across all 100 per-hit additions, so `PROC_FIRE_EPSILON` is
        //   never even consulted; verified by walking the chain in f64.)
        //
        // The explicit `per_cast` run is asserted byte-equal to the
        // omitted-default run — the default × override identity cell.
        // Mutation this kills: dropping the per-hit loop (roll count
        // forced to 1) collapses 20 → 4.
        // --------------------------------------------------------------
        #[test]
        fn ev_per_hit_feeds_the_accumulator_once_per_measured_hit() {
            let plan = minimal_plan();
            let build = minimal_build();
            let spark = |rolls: &str| {
                nova_simdef(
                    "",
                    &format!(
                        r#"{{ "spark": {{ "trigger": "on_hit", "chance": "0.2",
                                          "apply_buff": "glow"{rolls} }} }}"#
                    ),
                )
            };
            let twenty = dummy(20);

            let omitted = ev_json(&plan, &build, &spark(""), NOVA_ROT, &twenty);
            assert_eq!(omitted.actions["nova"].casts, 20);
            assert_eq!(
                omitted.proc_counts["spark"], 4,
                "per_cast (default): got {:?} — want crossings at casts 5/10/15/20",
                omitted.proc_counts
            );

            let explicit = ev_json(
                &plan,
                &build,
                &spark(r#", "rolls": "per_cast""#),
                NOVA_ROT,
                &twenty,
            );
            assert_eq!(
                serde_json::to_string(&explicit).unwrap(),
                serde_json::to_string(&omitted).unwrap(),
                "an explicit `per_cast` must be the omitted default"
            );

            let per_hit = ev_json(
                &plan,
                &build,
                &spark(r#", "rolls": "per_hit""#),
                NOVA_ROT,
                &twenty,
            );
            assert_eq!(
                per_hit.actions["nova"].casts, 20,
                "the cadence is untouched"
            );
            assert_eq!(
                per_hit.proc_counts["spark"], 20,
                "per_hit: got {:?} — want one crossing at the 5th hit of \
                 every cast (acc += 0.2 × 5 per cast)",
                per_hit.proc_counts
            );
        }

        // --------------------------------------------------------------
        // MC: chance 1, icd 0 — every draw fires, so the fire COUNT is
        // the RNG draw count made visible: per_cast → one draw per cast
        // (20), per_hit → one draw per HIT (100). Chance 1 is fine HERE
        // (the vacuity lesson is about accumulator semantics; MC has no
        // accumulator — what's under test is the number of Bernoulli
        // draws). Same-seed determinism is asserted in BOTH settings
        // (serialized byte-equality, two runs).
        // --------------------------------------------------------------
        #[test]
        fn mc_per_hit_draws_once_per_measured_hit() {
            let plan = minimal_plan();
            let build = minimal_build();
            let spark = |rolls: &str| {
                nova_simdef(
                    "",
                    &format!(
                        r#"{{ "spark": {{ "trigger": "on_hit", "chance": "1",
                                          "apply_buff": "glow"{rolls} }} }}"#
                    ),
                )
            };
            let twenty = dummy(20);
            let mc = |simdef_json: &str| {
                let sim_plan = compile_json(&plan, simdef_json, NOVA_ROT);
                run(
                    &plan,
                    &sim_plan,
                    &build,
                    &twenty,
                    Mode::MonteCarlo {
                        iterations: 1,
                        seed: 7,
                    },
                )
                .unwrap()
            };

            let per_cast = mc(&spark(""));
            assert_eq!(per_cast.actions["nova"].casts, 20);
            assert_eq!(
                per_cast.proc_counts["spark"], 20,
                "per_cast: got {:?} — one certain draw per cast",
                per_cast.proc_counts
            );

            let per_hit = mc(&spark(r#", "rolls": "per_hit""#));
            assert_eq!(
                per_hit.proc_counts["spark"], 100,
                "per_hit: got {:?} — one certain draw per hit, 5 × 20",
                per_hit.proc_counts
            );

            for simdef_json in [spark(""), spark(r#", "rolls": "per_hit""#)] {
                let a = serde_json::to_string(&mc(&simdef_json)).unwrap();
                let b = serde_json::to_string(&mc(&simdef_json)).unwrap();
                assert_eq!(a, b, "same seed twice must be byte-identical");
            }
        }

        // --------------------------------------------------------------
        // THE ICD-AT-ONE-INSTANT RULE, pinned as an EQUALITY: all hits
        // of one cast land at the same instant, so any icd > 0 caps
        // fires at ONE per cast even under per_hit — per_hit at icd 3.0
        // must EQUAL per_cast at icd 3.0, fire for fire.
        //
        // Hand-worked (chance 1, icd 3.0, casts complete t=1..20):
        //   t=1  first hit fires (acc/draw certain), arms icd_ready=4;
        //        hits 2..5 of the SAME instant are ICD-gated (now=1 < 4)
        //   t=2, t=3  every hit gated
        //   t=4  ready (4 < 4 is false) → first hit fires, ready=7 …
        //   → fires at t = 1, 4, 7, 10, 13, 16, 19  =  7 fires
        //
        // Asserted in BOTH modes (chance 1 makes MC deterministic).
        // Mutation this kills: dropping the mid-loop ICD gate lets hits
        // 2..5 of the firing cast roll too — per_hit inflates (t=1
        // alone would fire 5×) and the equality breaks.
        // --------------------------------------------------------------
        #[test]
        fn any_positive_icd_caps_per_hit_at_one_fire_per_cast() {
            let plan = minimal_plan();
            let build = minimal_build();
            let spark = |rolls: &str| {
                nova_simdef(
                    "",
                    &format!(
                        r#"{{ "spark": {{ "trigger": "on_hit", "chance": "1",
                                          "icd": 3.0,
                                          "apply_buff": "glow"{rolls} }} }}"#
                    ),
                )
            };
            let twenty = dummy(20);

            let ev_per_cast = ev_json(&plan, &build, &spark(""), NOVA_ROT, &twenty);
            let ev_per_hit = ev_json(
                &plan,
                &build,
                &spark(r#", "rolls": "per_hit""#),
                NOVA_ROT,
                &twenty,
            );
            assert_eq!(
                ev_per_hit.proc_counts["spark"], 7,
                "EV per_hit: got {:?} — want fires at t=1,4,7,10,13,16,19",
                ev_per_hit.proc_counts
            );
            assert_eq!(
                ev_per_hit.proc_counts["spark"], ev_per_cast.proc_counts["spark"],
                "the ICD-at-one-instant rule IS this equality: one cast's \
                 hits share one instant, so icd 3.0 caps both policies at \
                 one fire per open cast"
            );

            let mc = |simdef_json: &str| {
                let sim_plan = compile_json(&plan, simdef_json, NOVA_ROT);
                run(
                    &plan,
                    &sim_plan,
                    &build,
                    &twenty,
                    Mode::MonteCarlo {
                        iterations: 1,
                        seed: 11,
                    },
                )
                .unwrap()
            };
            let mc_per_cast = mc(&spark(""));
            let mc_per_hit = mc(&spark(r#", "rolls": "per_hit""#));
            assert_eq!(mc_per_hit.proc_counts["spark"], 7);
            assert_eq!(
                mc_per_hit.proc_counts["spark"], mc_per_cast.proc_counts["spark"],
                "MC: the gate skips the firing cast's remaining draws, so \
                 the equality holds exactly at chance 1"
            );
        }

        // --------------------------------------------------------------
        // THE PER-PROC OVERRIDE, all four (default × override) cells in
        // two runs — two procs on ONE trigger in ONE batch, chance 0.2,
        // icd 0, the 4-vs-20 numbers from the fractional pin:
        //
        //   run A, defaults omitted (per_cast):
        //     `steady` (no `rolls`)          → per_cast →  4
        //     `flurry` (`rolls: "per_hit"`)  → per_hit  → 20
        //   run B, `defaults.proc_rolls: per_hit`:
        //     `steady` (`rolls: "per_cast"`) → per_cast →  4
        //     `flurry` (no `rolls`)          → per_hit  → 20
        //
        // Also the multi-proc batch path: both procs roll on every one
        // of the same cast's hit events, independently.
        // --------------------------------------------------------------
        #[test]
        fn the_per_proc_override_wins_over_the_defaults_block_in_both_directions() {
            let plan = minimal_plan();
            let build = minimal_build();
            let twenty = dummy(20);

            let run_a = ev_json(
                &plan,
                &build,
                &nova_simdef(
                    "",
                    r#"{ "steady": { "trigger": "on_hit", "chance": "0.2",
                                     "apply_buff": "glow" },
                         "flurry": { "trigger": "on_hit", "chance": "0.2",
                                     "apply_buff": "glow", "rolls": "per_hit" } }"#,
                ),
                NOVA_ROT,
                &twenty,
            );
            assert_eq!(
                (run_a.proc_counts["steady"], run_a.proc_counts["flurry"]),
                (4, 20),
                "defaults omitted: got {:?}",
                run_a.proc_counts
            );

            let run_b = ev_json(
                &plan,
                &build,
                &nova_simdef(
                    r#""defaults": { "proc_rolls": "per_hit" },"#,
                    r#"{ "steady": { "trigger": "on_hit", "chance": "0.2",
                                     "apply_buff": "glow", "rolls": "per_cast" },
                         "flurry": { "trigger": "on_hit", "chance": "0.2",
                                     "apply_buff": "glow" } }"#,
                ),
                NOVA_ROT,
                &twenty,
            );
            assert_eq!(
                (run_b.proc_counts["steady"], run_b.proc_counts["flurry"]),
                (4, 20),
                "defaults per_hit: got {:?}",
                run_b.proc_counts
            );
        }

        // --------------------------------------------------------------
        // on_crit × per_hit, EV: the crit-probability weight applies PER
        // HIT. Branched plan (`crit` event, chance = the `cc` stat), cc
        // 0.5, proc chance 0.4, hits 5, icd 0, 20 casts:
        //
        //   per-hit accumulation = 0.4 × 0.5 = 0.2 (exact in f64: the
        //   ×0.5 is a pure exponent step) — the SAME chain as the
        //   fractional pin:
        //     per_cast → +0.2/cast → 4 fires
        //     per_hit  → +1.0/cast → 20 fires
        //
        // EV/MC agreement statement (documented on `ProcRolls`, asserted
        // by the pooled-mean regression below): MC samples ONE crit mask
        // per cast — the hits are simultaneous and share it — so under
        // per_hit a crit cast presents 5 draws at 0.4 and a non-crit
        // cast presents none: E[fires] = 20 × 0.5 × 5 × 0.4 = 20 = EV.
        // --------------------------------------------------------------
        #[test]
        fn ev_on_crit_weight_applies_per_hit() {
            let plan = crit_plan();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "cc": 0.5 } }"#).unwrap();
            let spark = |rolls: &str| {
                nova_simdef(
                    "",
                    &format!(
                        r#"{{ "spark": {{ "trigger": "on_crit", "chance": "0.4",
                                          "apply_buff": "glow"{rolls} }} }}"#
                    ),
                )
            };
            let twenty = dummy(20);

            let per_cast = ev_json(&plan, &build, &spark(""), NOVA_ROT, &twenty);
            assert_eq!(
                per_cast.proc_counts["spark"], 4,
                "per_cast: got {:?} — acc += 0.4 × 0.5 per cast",
                per_cast.proc_counts
            );

            let per_hit = ev_json(
                &plan,
                &build,
                &spark(r#", "rolls": "per_hit""#),
                NOVA_ROT,
                &twenty,
            );
            assert_eq!(
                per_hit.proc_counts["spark"], 20,
                "per_hit: got {:?} — acc += 0.4 × 0.5 per HIT, five per cast",
                per_hit.proc_counts
            );
        }

        // --------------------------------------------------------------
        // on_crit × per_hit, MC: the hits of one cast share ONE sampled
        // crit mask (simultaneous hits cannot disagree about whether the
        // cast crit), so at chance 1 / icd 0 every CRIT cast contributes
        // EXACTLY 5 fires and every non-crit cast exactly 0 — the fire
        // count is a multiple of 5, whatever the seed. Sampling a fresh
        // crit per HIT would break that invariant almost surely
        // (binomial hits per cast), which is what this pin
        // discriminates. cc 0.5 keeps both outcomes live (the count
        // lands strictly between the 0-crit and all-crit extremes for
        // any seed that samples both — asserted loosely).
        // --------------------------------------------------------------
        #[test]
        fn mc_per_hit_draws_share_the_casts_one_sampled_crit_mask() {
            let plan = crit_plan();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "cc": 0.5 } }"#).unwrap();
            let simdef_json = nova_simdef(
                "",
                r#"{ "spark": { "trigger": "on_crit", "chance": "1",
                                "apply_buff": "glow", "rolls": "per_hit" } }"#,
            );
            let sim_plan = compile_json(&plan, &simdef_json, NOVA_ROT);
            let report = run(
                &plan,
                &sim_plan,
                &build,
                &dummy(20),
                Mode::MonteCarlo {
                    iterations: 1,
                    seed: 3,
                },
            )
            .unwrap();
            let fires = report.proc_counts["spark"];
            assert_eq!(
                fires % 5,
                0,
                "got {fires} — one mask per cast means crit casts contribute \
                 exactly 5 fires each at chance 1"
            );
            assert!(
                fires > 0 && fires < 100,
                "got {fires} — cc 0.5 over 20 casts should sample both \
                 branches (0 or 100 would mean the mask never varied)"
            );
        }

        // --------------------------------------------------------------
        // EV/MC AGREEMENT under per_hit + on_crit (the design invariant
        // — a genuine divergence here would be a design flaw, not a
        // tuning problem): the EV pin above says 20; the pooled MC mean
        // over 2000 iterations must land on it. E[fires] = 20 casts ×
        // P(crit)=0.5 × 5 draws × 0.4 = 20. (Per-iteration σ ≈ 5.7, so
        // the pooled mean's σ ≈ 0.13 — the 10% band is ~15σ wide.)
        // --------------------------------------------------------------
        #[test]
        fn ev_and_mc_agree_under_per_hit_on_crit_regression() {
            let plan = crit_plan();
            let build: BuildState =
                serde_json::from_str(r#"{ "stats": { "dmg": 100.0, "cc": 0.5 } }"#).unwrap();
            let simdef_json = nova_simdef(
                "",
                r#"{ "spark": { "trigger": "on_crit", "chance": "0.4",
                                "apply_buff": "glow", "rolls": "per_hit" } }"#,
            );
            let sim_plan = compile_json(&plan, &simdef_json, NOVA_ROT);
            let twenty = dummy(20);

            let ev = run(&plan, &sim_plan, &build, &twenty, Mode::Expected).unwrap();
            assert_eq!(ev.proc_counts["spark"], 20);

            let mc = run(
                &plan,
                &sim_plan,
                &build,
                &twenty,
                Mode::MonteCarlo {
                    iterations: 2_000,
                    seed: 20260729,
                },
            )
            .unwrap();
            let mc_count = mc.proc_counts["spark"] as f64;
            let rel_err = (mc_count - 20.0).abs() / 20.0;
            assert!(
                rel_err < 0.10,
                "mc mean proc count {mc_count} vs ev count 20, relative \
                 error {rel_err} — EV's per-hit crit weight times the hit \
                 count must match MC's one-mask-per-cast expectation"
            );
        }

        // --------------------------------------------------------------
        // EV/MC AGREEMENT in the per_hit ICD-BOUND regime — the I1
        // regression's per-hit sibling, and the pin that justifies the
        // HARD-GATE choice for hits after a mid-cast fire (banking their
        // mass, as the pre-I1 executor did per cast, over-fires exactly
        // like it did then: all 0.6/s of hit mass would fire the moment
        // each ICD cleared, ~30 fires here instead of 20).
        //
        // chance 0.3, hits_per_use 2, icd 2.0, 60 casts (t=1..60), EV
        // hand-worked (h1/h2 = the cast's two hits; a fire arms
        // icd_ready = t+2, gating the NEXT cast but not the one at t+2):
        //   t=1  h1 .3   h2 .6
        //   t=2  h1 .9   h2 1.2 → FIRE (acc .2), ready=4
        //   t=3  gated
        //   t=4  h1 .5   h2 .8
        //   t=5  h1 1.1 → FIRE (acc .1), ready=7; h2 GATED (discarded)
        //   t=6  gated
        //   t=7  h1 .4   h2 .7
        //   t=8  h1 1.0 → FIRE (acc .0), ready=10; h2 GATED
        //        (f64: .7 + .3 = 0.9999999999999999 — THIS crossing is
        //        the one `PROC_FIRE_EPSILON` exists for)
        //   t=9  gated
        //   t=10 h1 .3   h2 .6   — the t=1 state, 9s later
        //   → a 3-fire / 9s cycle: fires at t = 2, 5, 8, 11, …, 59
        //     = 20 fires in 60s.
        // MC mean: an open cast fires w.p. 1−0.7² = 0.51; a fire at t
        // gates t+1 and reopens t+2, so the renewal interval is
        // 1 + Geom(0.51) ≈ 2.96s → ≈20.6 expected fires. Same 15% band
        // as the per-cast I1 regression.
        // --------------------------------------------------------------
        #[test]
        fn ev_procs_match_mc_in_the_per_hit_icd_bound_regime_regression() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef_json = r#"{
              "actions": { "nova": { "cast_time": "1",
                  "damage": { "stats": { "hits_per_use": 2 } } } },
              "buffs": { "glow": { "duration": 0.5 } },
              "procs": { "spark": { "trigger": "on_hit", "chance": "0.3",
                  "icd": 2.0, "apply_buff": "glow", "rolls": "per_hit" } },
              "damage_objective": "hit" }"#;
            let sim_plan = compile_json(&plan, simdef_json, NOVA_ROT);
            let sixty = dummy(60);

            let ev = run(&plan, &sim_plan, &build, &sixty, Mode::Expected).unwrap();
            assert_eq!(ev.actions["nova"].casts, 60);
            assert_eq!(
                ev.proc_counts["spark"], 20,
                "got {:?} — the hand-worked 3-fire/9s cycle above says 20 \
                 (banking gated-hit mass instead would inflate this)",
                ev.proc_counts
            );

            let mc = run(
                &plan,
                &sim_plan,
                &build,
                &sixty,
                Mode::MonteCarlo {
                    iterations: 2_000,
                    seed: 20260728,
                },
            )
            .unwrap();
            let mc_count = mc.proc_counts["spark"] as f64;
            let rel_err = (mc_count - 20.0).abs() / 20.0;
            assert!(
                rel_err < 0.15,
                "mc mean proc count {mc_count} vs ev count 20, relative \
                 error {rel_err} — the per-hit hard gate is what keeps the \
                 two modes agreeing in the ICD-bound regime"
            );
        }

        // --------------------------------------------------------------
        // THE MID-LOOP ICD CASE at icd > 0, hand-worked one level finer
        // than the equality pin: chance 0.6, hits 2, icd 3.0, 8 casts.
        //   t=1  h1 .6   h2 1.2 → FIRE (acc .2), ready=4
        //        (the CROSSING hit's own accumulation is what fired —
        //        `acc -= 1.0` keeps its fraction, nothing is zeroed)
        //   t=2, t=3  gated (acc stays .2)
        //   t=4  h1 .8   h2 1.4 → FIRE (acc .4), ready=7
        //   t=5, t=6  gated
        //   t=7  h1 1.0 → FIRE (acc .0), ready=10; h2 GATED
        //   t=8  gated
        //   → 3 fires, at t = 1, 4, 7.
        // The glow uptime makes the TIMING observable, as in the I1
        // pin: three full 0.5s windows inside 8s → uptime 1.5/8 =
        // 0.1875.
        // --------------------------------------------------------------
        #[test]
        fn a_mid_cast_fire_arms_the_icd_against_the_casts_remaining_hits() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef_json = r#"{
              "actions": { "nova": { "cast_time": "1",
                  "damage": { "stats": { "hits_per_use": 2 } } } },
              "buffs": { "glow": { "duration": 0.5 } },
              "procs": { "spark": { "trigger": "on_hit", "chance": "0.6",
                  "icd": 3.0, "apply_buff": "glow", "rolls": "per_hit" } },
              "damage_objective": "hit" }"#;
            let report = ev_json(&plan, &build, simdef_json, NOVA_ROT, &dummy(8));
            assert_eq!(
                report.proc_counts["spark"], 3,
                "got {:?} — want fires at t=1, 4, 7",
                report.proc_counts
            );
            assert!(
                close(report.buffs["glow"].uptime, 0.1875),
                "got {} — three full 0.5s windows in 8s (a shifted fire \
                 timeline would move this)",
                report.buffs["glow"].uptime
            );
        }

        // --------------------------------------------------------------
        // CHANCE IS EVALUATED ONCE PER PROC PER CAST — the designed
        // rule (one measured world per cast, P8c: a fire mid-cast is
        // not visible to its own sibling hits' chance), pinned where it
        // DISCRIMINATES: icd 0, the proc's own effect feeding its own
        // chance expression. chance "0.4 + buff.glow * 0.6", hits 5,
        // glow duration 0.5 (expired again by the next cast, 1s later),
        // 3 casts. Once-per-cast, every cast identical (starts acc ≈ 0,
        // glow down → chance 0.4):
        //   h1 .4  h2 .8  h3 1.2 → FIRE (.2)  h4 .6  h5 1.0 → FIRE (.0)
        //   → 2 fires/cast × 3 casts = 6.
        // Re-evaluating per hit instead would read glow=1 from h4 on
        // (chance 1.0): h4 → 1.6 FIRE (.6), h5 → 1.6 FIRE (.6) — four
        // fires on cast 1 alone. The 6 is the once-per-cast signature.
        // --------------------------------------------------------------
        #[test]
        fn chance_is_evaluated_once_per_cast_not_once_per_hit() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef_json = nova_simdef(
                "",
                r#"{ "spark": { "trigger": "on_hit",
                                "chance": "0.4 + buff.glow * 0.6",
                                "apply_buff": "glow", "rolls": "per_hit" } }"#,
            );
            let report = ev_json(&plan, &build, &simdef_json, NOVA_ROT, &dummy(3));
            assert_eq!(report.actions["nova"].casts, 3);
            assert_eq!(
                report.proc_counts["spark"], 6,
                "got {:?} — 2 fires per cast at the once-per-cast chance 0.4; \
                 more means a mid-cast fire leaked into its siblings' chance",
                report.proc_counts
            );
        }

        // --------------------------------------------------------------
        // SCOPE: an `on_cast` proc's event is the CAST, not a hit — it
        // rolls once per cast under either policy (the instant-cast ×
        // `measure` precedent: documented behavior, not an error), and a
        // utility cast (no damage) presents no on_hit roll under either
        // policy. One run, defaults per_hit, both procs at chance 0.2 /
        // icd 0 so a per-hit reading would be LOUD. The 1s `shout`
        // (utility, cooldown 2, first priority) and the 5-hit `nova`
        // alternate — shout completes at t=1,3,…,19 (10 casts), nova at
        // t=2,4,…,20 (10 casts; the t=20 horizon completion counts):
        //   `on_every_cast` (on_cast, unfiltered): 20 completions ×
        //     0.2/CAST → crossings at completions 5/10/15/20 → 4 fires
        //     (nova's 5 hits leaking in would add +1.0 per nova cast)
        //   `never` (on_hit, filtered to `shout`): 0 fires — shout
        //     casts produce NO hit event at all, per_hit or not.
        // --------------------------------------------------------------
        #[test]
        fn on_cast_rolls_per_cast_and_utility_casts_present_no_hit_roll_under_per_hit() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef_json = r#"{
              "defaults": { "proc_rolls": "per_hit" },
              "actions": {
                "nova":  { "cast_time": "1",
                           "damage": { "stats": { "hits_per_use": 5 } } },
                "shout": { "cast_time": "1", "cooldown": 2.0 }
              },
              "buffs": { "glow": { "duration": 0.5 } },
              "procs": {
                "on_every_cast": { "trigger": "on_cast", "chance": "0.2",
                                   "apply_buff": "glow" },
                "never": { "trigger": "on_hit", "chance": "0.2",
                           "actions": ["shout"], "apply_buff": "glow" }
              },
              "damage_objective": "hit" }"#;
            let rot = r#"{ "rules": [ { "action": "shout" },
                                      { "action": "nova" } ] }"#;
            let report = ev_json(&plan, &build, simdef_json, rot, &dummy(20));
            assert_eq!(report.actions["shout"].casts, 10);
            assert_eq!(report.actions["nova"].casts, 10);
            assert_eq!(
                report.proc_counts["on_every_cast"], 4,
                "got {:?} — on_cast is cast-shaped under per_hit too: \
                 acc += 0.2 per CAST, crossings at completions 5/10/15/20",
                report.proc_counts
            );
            assert_eq!(
                report.proc_counts["never"], 0,
                "got {:?} — a utility cast presents no hit event under \
                 either policy",
                report.proc_counts
            );
        }

        // --------------------------------------------------------------
        // FAIL-CLOSED: per_hit rolls a literal count, so a fractional
        // measured `hits_per_use` (2.5 — a legal EV averaging device
        // under per_cast) is a positioned run error naming the proc,
        // the action, and the value. The same config under per_cast
        // runs fine (the roll is hits-blind).
        // --------------------------------------------------------------
        #[test]
        fn a_fractional_hits_per_use_fails_closed_under_per_hit_only() {
            let plan = minimal_plan();
            let build = minimal_build();
            let simdef = |rolls: &str| {
                format!(
                    r#"{{ "actions": {{ "nova": {{ "cast_time": "1",
                             "damage": {{ "stats": {{ "hits_per_use": 2.5 }} }} }} }},
                         "buffs": {{ "glow": {{ "duration": 0.5 }} }},
                         "procs": {{ "spark": {{ "trigger": "on_hit", "chance": "0.2",
                             "apply_buff": "glow"{rolls} }} }},
                         "damage_objective": "hit" }}"#
                )
            };
            let five = dummy(5);

            let ok = ev_json(&plan, &build, &simdef(""), NOVA_ROT, &five);
            assert_eq!(ok.actions["nova"].casts, 5, "per_cast is hits-blind");

            let sim_plan = compile_json(&plan, &simdef(r#", "rolls": "per_hit""#), NOVA_ROT);
            let err = run(&plan, &sim_plan, &build, &five, Mode::Expected).unwrap_err();
            for needle in ["spark", "nova", "2.5", "per_hit"] {
                assert!(
                    err.what.contains(needle),
                    "error must name `{needle}`: {}",
                    err.what
                );
            }
        }
    }

    // ==================================================================
    // P8a follow-up — the ZERO-EVALUATION hole (spec-review probe): a
    // utility-only rotation completes without a single `Plan`
    // evaluation, so the per-evaluation validation in
    // `validate_and_resolve_build_for_phase` never runs, and a
    // non-finite build came back `Ok(dps = 0)` SILENTLY — while Level-1
    // `evaluate` on the SAME build fails closed. Meanwhile the NaN
    // flowed through `write_stat_condition_slots` into rule gates and
    // resource regen. The fix: `run` validates the build's stats and
    // contribution values, and each phase's stat overrides, ONCE at
    // entry — before the event loop, on every route.
    // ==================================================================
    mod run_entry_validation {
        use super::*;

        /// One utility action (no `damage`), spammed — the executor
        /// never queries the `Plan`, which is exactly the route that
        /// dodged every per-evaluation check.
        fn utility_only() -> (Plan, SimPlan) {
            let plan = minimal_plan();
            let mut actions = BTreeMap::new();
            actions.insert(
                "shout".to_string(),
                ActionDef {
                    extra: Default::default(),
                    measure: None,
                    cast_time: "1".into(),
                    cooldown: NumOrExpr::Num(0.0),
                    cost: BTreeMap::new(),
                    gain: BTreeMap::new(),
                    damage: None,
                    apply_buff: Vec::new(),
                    effects: Vec::new(),
                },
            );
            let simdef = SimDef {
                extra: Default::default(),
                defaults: Default::default(),
                resources: BTreeMap::new(),
                actions,
                buffs: BTreeMap::new(),
                procs: BTreeMap::new(),
                damage_objective: "hit".into(),
            };
            let rotation = Rotation {
                extra: Default::default(),
                rules: vec![Rule {
                    extra: Default::default(),
                    action: "shout".into(),
                    when: None,
                }],
            };
            let sim_plan = sim_compile(&plan, &simdef, &rotation).unwrap();
            (plan, sim_plan)
        }

        fn five_seconds() -> Scenario {
            serde_json::from_str(r#"{ "phases": [ { "name": "p", "weight": 5 } ] }"#).unwrap()
        }

        #[test]
        fn a_non_finite_build_stat_is_rejected_at_run_entry_with_zero_evaluations() {
            let (plan, sim_plan) = utility_only();
            let mut build = minimal_build();
            build.stats.insert("dmg".into(), f64::NAN);
            let err = run(&plan, &sim_plan, &build, &five_seconds(), Mode::Expected).expect_err(
                "a NaN build stat must fail closed even when no cast ever \
                 evaluates the Plan",
            );
            assert!(
                err.what.contains("build stat `dmg` must be finite"),
                "got: {}",
                err.what
            );
        }

        #[test]
        fn a_non_finite_contribution_value_is_rejected_at_run_entry_too() {
            let (plan, sim_plan) = utility_only();
            let mut build = minimal_build();
            build.contributions.push(crate::build::Contribution {
                bucket: "nope".into(),
                value: f64::INFINITY,
                event: None,
                condition: None,
            });
            let err = run(&plan, &sim_plan, &build, &five_seconds(), Mode::Expected).unwrap_err();
            assert!(
                err.what
                    .contains("contribution value into bucket `nope` must be finite"),
                "got: {}",
                err.what
            );
        }

        #[test]
        fn a_non_finite_phase_stat_override_is_rejected_at_run_entry_too() {
            let (plan, sim_plan) = utility_only();
            let mut scenario = five_seconds();
            scenario.phases[0].stats.insert("dmg".into(), f64::NAN);
            let err = run(
                &plan,
                &sim_plan,
                &minimal_build(),
                &scenario,
                Mode::Expected,
            )
            .unwrap_err();
            assert!(
                err.what.contains("phase `p` stat `dmg` must be finite"),
                "got: {}",
                err.what
            );
        }

        // The finite path through the same fixture still runs to
        // completion — the entry walk rejects non-finite VALUES, nothing
        // else. (Cadence: cast_time 1, no damage → 5 casts, 0 damage.)
        #[test]
        fn a_finite_utility_only_rotation_still_completes() {
            let (plan, sim_plan) = utility_only();
            let report = run(
                &plan,
                &sim_plan,
                &minimal_build(),
                &five_seconds(),
                Mode::Expected,
            )
            .unwrap();
            assert_eq!(report.actions["shout"].casts, 5);
            assert_eq!(report.total.total_damage, 0.0);
        }
    }
}
