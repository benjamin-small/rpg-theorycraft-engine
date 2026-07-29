//! Sequencing: `SimDef` + `Rotation` compiled once by [`compile()`] into
//! an inert [`SimPlan`], which [`run`] then drives as a discrete-event
//! timeline under either [`Mode`]. `sim::compile` is the ONLY place sim
//! expressions are parsed, mirroring `plan::compile`'s "compile once,
//! evaluate fast" contract.
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
//! The remaining fields are cross-REFERENCES rather than expressions, and
//! `sim::compile` resolves them all to indices, fail-closed: the ordered
//! `effects` lists (P8b — [`crate::simdef::ActionDef::effects`] runs at
//! cast complete, [`crate::simdef::ProcDef::effects`] at proc fire; the
//! 0.x sugar fields `apply_buff`/`cast_action` desugar into them at
//! compile, and mixing sugar with an explicit list on one entity is an
//! "ambiguous order" compile error) and
//! [`crate::simdef::ProcDef::actions`] (a trigger filter naming the
//! actions whose casts this proc considers; `None` = all of them, the
//! 0.2.0 behavior). An unknown name in any of them — or an EMPTY
//! `actions` list, which would describe a proc that can never fire, or a
//! proc whose effects are empty AFTER desugar ("a proc must do
//! something"), or a `cast_action` effect on an ACTION (an action
//! free-casting an action reopens A→B→A recursion; proc-only, see
//! `ROADMAP.md`) — is a compile error.
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
//!   →  measure the cast's WORLD*, evaluate its damage, credit the hit
//!   →  the action's effects (apply_buff entries), in list order
//!   →  proc rolls: on_cast, then on_hit, then on_crit
//! ```
//!
//! *Under the default `measure: "cast_complete"`. An action measured at
//! `cast_start` ([`crate::simdef::Measure`], P8c) captured its world —
//! `damage.stats` overlay, `hits_per_use`, crit weight, effective build
//! and phase, all together — when the cast BEGAN; the transaction then
//! reads that snapshot at this same step instead of measuring afresh,
//! and everything else in the diagram is unchanged.
//!
//! Reading it off, in the order a config author trips over them:
//!
//! - A `damage.stats` expression sees a resource at its POST-`gain`
//!   amount, and `casts.<this action>` counts from `1` on the first cast
//!   (both shift to the cast-start readings under `cast_start` — see
//!   [`crate::simdef::Measure`]).
//! - The cast does NOT benefit from anything it applies — neither a buff
//!   in its own `effects` list nor one a proc it triggers applies. A hit
//!   cannot be changed by what it causes.
//! - A proc rolled by this cast DOES see the buffs the cast's `effects`
//!   applied (`buff.<applied>` reads `1` in its `chance`). Intrinsic
//!   effects of the action resolve before effects merely TRIGGERED by
//!   it, so the whole `effects` list precedes the whole proc batch and
//!   the two never interleave, whatever the procs' name order.
//! - Within one action's `effects` list, what a later entry sees splits
//!   across TWO axes (the full statement is on
//!   [`crate::simdef::ActionDef::effects`]): sim STATE is SEQUENTIAL
//!   (`stacks.*`, `buff.*`, resource amounts — a `duration` expression
//!   reads them fresh per entry), while the measured WORLD — build AND
//!   phase — is the cast's ONE snapshot (so a snapshot
//!   [`crate::simdef::TickObjective`] magnitude sees neither an earlier
//!   entry's `contributions` nor a condition it drives). One world per
//!   cast is P8c's deliberate fix: through 0.3.0 the build was frozen
//!   but the phase was live, and reordering a two-entry list could
//!   double a captured DoT at identical reported uptime.
//!
//! A firing PROC runs its own `effects` list in list order, each entry
//! against the sim state its predecessors left behind (P7b sequential
//! state — pinned by the 0.2/0.1 order contrast in `exec`'s
//! `effects_list` tests). A proc-triggered FREE cast (a `cast_action`
//! effect) runs `gain` → damage → the action's own `effects` at the
//! firing proc's instant, and skips the rest of the cast pipeline: no
//! cost, no cooldown, and no further proc rolls (which is what bounds
//! proc recursion — and since a `cast_action` effect cannot appear on an
//! ACTION, a free cast cannot chain into another).
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
//!   event there. Within the instant they resolve in the configured
//!   coincident-event order ([`crate::simdef::EventOrder`] — scheduling
//!   order, the `seq` tiebreaker, under the default), exactly as at any
//!   other instant: the knob decides which event at the horizon resolves
//!   FIRST, never whether one resolves.
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
//! # A buff expiring on the cast grid
//!
//! Under the default `event_order` ([`crate::simdef::EventOrder`]),
//! events sharing an instant resolve in SCHEDULING order (the `seq`
//! tiebreaker), at the horizon and everywhere else alike. One consequence
//! of that is the single most likely way to get a wrong number out of
//! this executor while every diagnostic looks healthy, so it is worth
//! reading even if the rest of this page is reference:
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
//! The loss appears only in damage. Both effects are pinned as contrast
//! runs in the examples, each changing exactly one duration:
//!
//! ```text
//!   poe2_triggers  `shock` refreshed by a bolt every 2s
//!     duration 2.5 →  uptime 0.95      bolt damage 2175.0
//!     duration 2.0 →  uptime 0.95      bolt damage 1837.5   (−15.5%)
//!
//!   poe2_charges   `frenzy_charge` (add_refresh_all) on a 1s cadence
//!     "4.5 + stacks" →  avg_stacks 2.25   total 11748.0
//!     "4 + stacks"   →  avg_stacks 2.25   total 10875.0     (−7.4%)
//! ```
//!
//! The `poe2_charges` row is the sharper of the two: the stack falls off
//! on a cast instant, the rotation's `when` reads the lower count and
//! reshapes the whole cycle (12 generators / 28 spenders becomes 15 / 25)
//! — and `avg_stacks` still reports 2.25 to the last digit.
//!
//! The guidance that falls out of it: if a buff duration lands exactly on
//! the cast grid, nudge it off — the examples' half-integer
//! `representative` durations are that nudge, and say so — or expect the
//! refreshing cast not to benefit from its own buff. If a damage number
//! looks low while the uptimes look perfect, this is the first thing to
//! check.
//!
//! **And here are the TWO configs that fix it** — one moves the
//! measurement, one moves the ordering; either alone restores the number.
//!
//! The measurement-level fix (P8c): measure the cast at the instant it
//! BEGINS instead of the instant it completes —
//!
//! ```json
//! { "defaults": { "measure": "cast_start" } }
//! ```
//!
//! (package-wide; or per action, `"measure": "cast_start"` on the
//! [`crate::simdef::ActionDef`]). The expiry-vs-completion race still
//! happens, but nothing is measured at completions anymore: every cast
//! after the first STARTS strictly inside the previous completion's
//! window, so the refreshing cast benefits from its own buff again.
//!
//! The ordering-level fix (P8d): keep the measurement where it is and
//! reorder the COLLISION instead —
//!
//! ```json
//! { "defaults": { "event_order": "completions_first" } }
//! ```
//!
//! (package-wide ONLY, by design — [`crate::simdef::EventOrder`]'s docs
//! say why a per-spell form would be incoherent). Every `CastComplete`
//! now outranks a coincident `BuffExpire`: the completing cast measures
//! WITH its still-live buff, and its reapplication makes the pending
//! expiry stale.
//!
//! `poe2_triggers` runs BOTH as contrasts against the same on-grid 2.0s
//! `shock`: each knob alone restores bolt damage 1837.5 → 2175, the
//! off-grid number, at the same 0.95 uptime. Before adopting either
//! wholesale, see [`crate::simdef::Measure`] for what else the
//! measurement knob moves (`casts.<self>`, resource readings) and
//! [`crate::simdef::EventOrder`] for what else the ordering knob moves
//! (the zero-weight-final-phase attribution flip).
//!
//! # Sim slot layout
//!
//! The executor ([`run`]) maintains one flat `&[f64]` slot array shaped
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
    compile, CompiledAction, CompiledBuff, CompiledEffect, CompiledProc, CompiledResource,
    CompiledRule, CompiledTick, CompiledValue, SimPlan,
};
pub use exec::{run, Mode, SimScratch};
pub use report::{
    ActionReport, BuffReport, Distribution, PhaseReport, ResourceReport, SimReport, Totals,
};
