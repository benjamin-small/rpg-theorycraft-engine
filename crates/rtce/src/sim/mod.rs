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
//! cast complete) and [`crate::simdef::ProcDef::actions`] (a trigger
//! filter naming the actions whose casts this proc considers; `None` =
//! all of them, the 0.2.0 behavior). An unknown name in either — or an
//! EMPTY `actions` list, which would describe a proc that can never
//! fire — is a compile error.
//!
//! Pipeline STAGES and buckets are deliberately absent from this space —
//! a sim expression referencing one is a fail-closed "unknown identifier"
//! compile error, the same as any other unresolved name. Resource/action/
//! buff names join the SAME flat namespace as stats/conditions: a name
//! colliding with an existing stat or condition, or reusing the reserved
//! `time`/`duration` words, is a compile error rather than a silent
//! shadow.
//!
//! # The cast-complete order
//!
//! A completing cast resolves several effects at ONE instant, and their
//! order is observable — a `damage.stats` expression, a proc `chance`,
//! and a snapshot capture can each read state an earlier step wrote. It
//! is therefore fixed, and this is the canonical statement of it (every
//! other mention in this crate links here):
//!
//! ```text
//! apply `gain`  →  casts.<action> += 1
//!   →  evaluate `damage.stats`, measure and credit the hit
//!   →  apply_buff, in list order
//!   →  proc rolls: on_cast, then on_hit, then on_crit
//! ```
//!
//! Reading it off, in the order a config author trips over them:
//!
//! - A `damage.stats` expression sees a resource at its POST-`gain`
//!   amount, and `casts.<this action>` counts from `1` on the first cast.
//! - The cast does NOT benefit from anything it applies — neither its own
//!   `apply_buff` nor a buff one of its procs applies. A hit cannot be
//!   changed by what it causes.
//! - A proc rolled by this cast DOES see the cast's `apply_buff`
//!   (`buff.<applied>` reads `1` in its `chance`). Intrinsic effects of
//!   the action resolve before effects merely TRIGGERED by it, so the
//!   whole `apply_buff` list precedes the whole proc batch and the two
//!   never interleave, whatever the procs' name order.
//! - Within one `apply_buff` list, a `duration` expression IS sequential
//!   (it reads sim state, so a later entry sees earlier entries' STACK
//!   COUNTS) while a snapshot [`crate::simdef::TickObjective`] magnitude
//!   is FROZEN at the world the cast found (it reads a build, and that
//!   build is captured once before the list runs, so a later entry does
//!   NOT see earlier entries' CONTRIBUTIONS). Both halves are pinned.
//!
//! A proc-triggered FREE cast ([`crate::simdef::ProcDef::cast_action`])
//! runs `gain` → damage → `apply_buff` at the firing proc's instant, and
//! skips the rest of the cast pipeline: no cost, no cooldown, and no
//! further proc rolls (which is what bounds proc recursion).
//!
//! # The fight horizon
//!
//! The fight runs on `[0, duration]`, where `duration` is the sum of the
//! scenario's phase weights, and the last instant of it — `t == duration`,
//! the HORIZON — obeys three rules:
//!
//! - **No cast BEGINS at or after `duration`.** The rotation makes no new
//!   commitments at the horizon: nothing is chosen, no cost is paid, no
//!   cooldown is armed.
//! - **Every event already scheduled AT `duration` is processed.** The
//!   executor drains the whole instant; it does not stop after the first
//!   event there. Within the instant they resolve in scheduling order
//!   (the `seq` tiebreaker), exactly as at any other instant.
//! - **Therefore a cast completing exactly at `duration` counts** — its
//!   `casts`, its damage and its `apply_buff` all land. A cast started at
//!   `duration − cast_time` is a full cast, not a truncated one.
//!
//! Read together: the horizon is CLOSED for things already in flight and
//! OPEN for nothing new. A 10s fight of 1s casts is ten casts, the tenth
//! completing on the buzzer, whether or not a buff happens to expire on
//! that same instant.
//!
//! Buff windows are integrated the same way: a window closing at
//! `duration` is credited its full span, whether it is closed by its own
//! `BuffExpire` at the horizon or by the end-of-fight flush.
//!
//! # A buff expiring on the cast grid (read this one)
//!
//! Events sharing an instant resolve in SCHEDULING order (the `seq`
//! tiebreaker), at the horizon and everywhere else alike. One consequence
//! is worth calling out on its own, because it costs damage silently:
//!
//! > When a buff's window closes at exactly the instant the cast that
//! > would refresh it completes, the `BuffExpire` was scheduled EARLIER —
//! > back when the buff was last applied — so it carries the lower `seq`
//! > and resolves FIRST. The buff is already down when that cast measures
//! > itself. The cast re-applies it immediately afterwards, so the window
//! > never visibly lapses; only that cast's own damage is short.
//!
//! This bites whenever a buff's `duration` is an exact multiple of the
//! cadence of the action that refreshes it — which is exactly what a
//! config author is most likely to write by hand ("2s shock, refreshed by
//! a bolt every 2s").
//!
//! **The uptime column will not tell you.** The expiry and the
//! reapplication share an instant, so the gap is zero-width and every
//! INTEGRATED measurement — [`BuffReport::uptime`], `avg_stacks`,
//! `SimReport::condition_uptime` — reads exactly as if nothing happened.
//! The loss appears only in damage. Measured on
//! `examples/poe2_triggers.rs`, whose `shock` is refreshed by a bolt every
//! 2s, changing nothing but the duration:
//!
//! ```text
//!     shock duration      shock uptime      bolt damage
//!                2.5              0.95           2175.0
//!                2.0              0.95           1837.5
//! ```
//!
//! Identical uptime, 15% less damage. The guidance that falls out of it:
//! if a buff duration lands exactly on the cast grid, nudge it off — the
//! examples' half-integer `representative` durations are that nudge, and
//! say so — or expect the refreshing cast not to benefit from its own
//! buff. If a damage number looks low while the uptimes look perfect,
//! this is the first thing to check.
//!
//! Whether this ordering is the RIGHT semantics is an open question for a
//! later version, recorded in `ROADMAP.md`: a `CastComplete` arguably
//! ought to resolve before a coincident `BuffExpire`, since a cast that
//! refreshes a buff at the instant it lapses should plausibly keep it up.
//! Deliberately NOT decided here — the ordering is long-standing behavior,
//! orthogonal to the horizon rule above, and changing it would move
//! numbers for every config that hits it.
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
