//! Sequencing: `SimDef` + `Rotation` compiled once into an inert
//! [`compile::SimPlan`] — the timeline simulator's config, ready for a
//! future discrete-event executor (P6c) to drive. `sim::compile` is the
//! ONLY place sim expressions are parsed, mirroring `plan::compile`'s
//! "compile once, evaluate fast" contract.
//!
//! # The extended sim symbol space
//!
//! Sim expressions (resource `max`/`regen_per_sec`, action `cast_time`,
//! proc `chance`, rule `when`) compile against the underlying [`Plan`]'s
//! own flat namespace (every stat and condition), EXTENDED with sim-state
//! names (see the design spec's "Expression language v2" section):
//!
//! - `time`, `duration` — reserved words; the current sim clock and the
//!   scenario's total duration in seconds.
//! - each resource, by its bare name — the resource's current amount.
//! - `cooldown.<action>` — seconds remaining before `<action>` is off
//!   cooldown (`0` = ready).
//! - `buff.<buff>` — `1.0` while `<buff>` is active, else `0.0`.
//! - `buff_remaining.<buff>` — seconds remaining on `<buff>`'s duration.
//! - `casts.<action>` — number of times `<action>` has been cast so far.
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
//!   buff_remaining.<buff>…, casts.<action>… ]
//! ```

mod compile;
mod exec;
mod report;
pub use compile::{
    compile, CompiledAction, CompiledBuff, CompiledProc, CompiledResource, CompiledRule,
    ProcEffect, SimPlan,
};
pub use exec::{run, Mode, SimScratch};
pub use report::{ActionReport, Distribution, PhaseReport, ResourceReport, SimReport, Totals};
