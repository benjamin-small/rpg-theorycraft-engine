//! Sequencing: `SimDef` + `Rotation` compiled once into an inert
//! [`compile::SimPlan`] — the timeline simulator's config, ready for a
//! future discrete-event executor (P6c) to drive. `sim::compile` is the
//! ONLY place sim expressions are parsed, mirroring `plan::compile`'s
//! "compile once, evaluate fast" contract.
//!
//! # The extended sim symbol space
//!
//! Sim expressions (resource `max`/`regen_per_sec`, action `cast_time`,
//! proc `chance`, rule `when`) compile against the underlying
//! [`crate::plan::Plan`]'s own flat namespace (every stat and condition),
//! EXTENDED with sim-state
//! names (see the design spec's "Expression language v2" section):
//!
//! - `time`, `duration` — reserved words; the current sim clock and the
//!   scenario's total duration in seconds.
//! - each resource, by its bare name — the resource's current amount.
//! - `cooldown.<action>` — seconds remaining before `<action>` is off
//!   cooldown (`0` = ready).
//! - `buff.<buff>` — `1.0` while `<buff>` has at least one live instance,
//!   else `0.0`. Never the stack count, however many instances are live.
//! - `buff_remaining.<buff>` — seconds remaining on the LONGEST-lived of
//!   `<buff>`'s live instances (`0.0` when inactive). With one instance
//!   that is simply "seconds left"; with several independently-expiring
//!   ones it is the last to fall off, not the next.
//! - `casts.<action>` — number of times `<action>` has been cast so far.
//! - `stacks.<buff>` — `<buff>`'s live instance count as an `f64` (`0`
//!   when inactive). The counted companion to `buff.<buff>`; see
//!   [`crate::simdef::BuffDef`] for what a stack count scales.
//!
//! A buff's [`crate::simdef::BuffDef::tick_objective`] names a `Plan`
//! objective rather than an expression, and takes either of two shapes: a
//! bare name (LIVE — re-evaluated on every state change, × the stack
//! count) or `{ "objective": …, "snapshot": true }` (each instance
//! captures the rate at its own application and ticks it unchanged to
//! expiry; the buff's rate is the SUM over instances). See
//! [`crate::simdef::TickObjective`].
//!
//! Five further fields are expression-valued (P7b) and compile against
//! this SAME space, each evaluated at its own documented instant rather
//! than once up front: `BuffDef::duration`, `ActionDef::cooldown`, the
//! `ActionDef::cost`/`gain` amounts, and the `ActionDamage::stats` values.
//! Each accepts a plain number OR an expression string
//! ([`crate::simdef::NumOrExpr`], untagged) — every rtce 0.2.0 config,
//! which only ever wrote numbers there, parses and behaves unchanged.
//!
//! Two further fields are cross-REFERENCES rather than expressions, and
//! `sim::compile` resolves both to indices, fail-closed (P7d):
//! [`crate::simdef::ActionDef::apply_buff`] (buffs the action applies at
//! cast complete, before any of that cast's proc rolls) and
//! [`crate::simdef::ProcDef::actions`] (a trigger filter naming the
//! actions whose casts this proc considers; `None` = all of them, the
//! 0.2.0 behavior). An unknown name in either — or an EMPTY `actions`
//! list, which would describe a proc that can never fire — is a compile
//! error.
//!
//! Pipeline STAGES and buckets are deliberately absent from this space —
//! a sim expression referencing one is a fail-closed "unknown identifier"
//! compile error, the same as any other unresolved name. Resource/action/
//! buff names join the SAME flat namespace as stats/conditions: a name
//! colliding with an existing stat or condition, or reusing the reserved
//! `time`/`duration` words, is a compile error rather than a silent
//! shadow.
//!
//! # Sim slot layout
//!
//! A future executor maintains one flat `&[f64]` slot array shaped
//! `[Plan's own slot layout | sim slots]` — [`compile::SimPlan::sim_base`]
//! marks where the sim segment begins, [`compile::SimPlan::slot_width`]
//! its total width. The sim segment itself is laid out in EXACTLY this
//! order, each named sub-range sorted by name (`SimDef`'s registries are
//! `BTreeMap`s, which already iterate sorted):
//!
//! ```text
//! [ time, duration, resources…, cooldown.<action>…, buff.<buff>…,
//!   buff_remaining.<buff>…, casts.<action>…, stacks.<buff>… ]
//! ```

mod compile;
mod exec;
mod report;
pub use compile::{
    compile, CompiledAction, CompiledBuff, CompiledProc, CompiledResource, CompiledRule,
    CompiledTick, CompiledValue, ProcEffect, SimPlan,
};
pub use exec::{run, Mode, SimScratch};
pub use report::{
    ActionReport, BuffReport, Distribution, PhaseReport, ResourceReport, SimReport, Totals,
};
