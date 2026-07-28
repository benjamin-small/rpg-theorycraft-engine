//! SimDef — the game's SEQUENCING config: resources, actions, buffs, and
//! procs, plus the per-hit objective the timeline simulator accumulates.
//! Sits BESIDE the `GameDef`/`Plan` — nothing in `plan.rs` changes.
//! `sim::compile` turns a `SimDef` (with a `Rotation`) into a `SimPlan`;
//! see that module for the compiled form and the extended symbol space.

use crate::build::Contribution;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The game's SEQUENCING config, as data: resource/action/buff/proc
/// registries plus the `Plan` objective the sim accumulates per hit.
/// Compiled once (together with a [`Rotation`]) by `sim::compile`.
///
/// # Unknown keys (P8a)
///
/// Every struct on this config surface collects keys it does not declare
/// into a public `extra` field (serde flatten), and `sim::compile` fails
/// closed on any collected key that does not start with `_` — a
/// positioned error naming the key, the entity it sits on, and the
/// nearest real field ("unknown field `tick_objectiv` on buff `poison` —
/// did you mean `tick_objective`?"). Keys starting with `_` are the
/// documented ANNOTATION NAMESPACE (`_source`, `_scope`, …): accepted at
/// every nesting level and carried through serde round-trips, so a
/// config's provenance notes survive a load-and-save.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SimDef {
    /// Package-wide semantic defaults (P8c's `measure`, with later knobs
    /// joining it) — see [`SimDefaults`]. Omitted = every knob at its
    /// 0.3.0-behavior value, which is what every 0.2.0/0.3.0 config gets
    /// without naming the block.
    #[serde(default, skip_serializing_if = "SimDefaults::is_vacuous")]
    pub defaults: SimDefaults,
    /// Resource registry (mana, spirit, …), by name.
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceDef>,
    /// Action registry (casts the rotation can choose), by name.
    #[serde(default)]
    pub actions: BTreeMap<String, ActionDef>,
    /// Buff/debuff registry (timed contribution/condition windows), by
    /// name.
    #[serde(default)]
    pub buffs: BTreeMap<String, BuffDef>,
    /// Proc registry (chance-triggered effects), by name.
    #[serde(default)]
    pub procs: BTreeMap<String, ProcDef>,
    /// Name of the `Plan` objective the sim accumulates as
    /// `damage_objective × hits` on every completed damaging cast.
    pub damage_objective: String,
    /// Unknown keys collected at parse — see the type-level "Unknown
    /// keys" section: `_`-prefixed annotations survive round-trips here;
    /// anything else fails closed at `sim::compile`.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl SimDef {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &[
        "defaults",
        "resources",
        "actions",
        "buffs",
        "procs",
        "damage_objective",
    ];
}

/// Package-wide semantic defaults — the P8 `defaults` block. Each field
/// is a small named enum whose default value reproduces the 0.3.0
/// behavior exactly, so an omitted block (every 0.2.0/0.3.0 config) is a
/// provable no-op; naming a field changes ONE semantic knob for the whole
/// `SimDef`, and per-entity overrides (e.g. [`ActionDef::measure`]) win
/// over it.
///
/// Later P8 slices add `proc_rolls` and `event_order` here; the block is
/// the intended home for every future knob of this kind.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SimDefaults {
    /// The instant a cast's world is measured at, for every action that
    /// does not override it — see [`Measure`] for the full semantics.
    /// Defaults to [`Measure::CastComplete`], the 0.3.0 behavior.
    #[serde(default)]
    pub measure: Measure,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming this block.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl SimDefaults {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &["measure"];

    /// `true` when serializing this block would write nothing a reader
    /// needs: every knob at its default AND no `_` annotations to carry
    /// (`default()` has an empty `extra`, so equality covers both). Used
    /// by [`SimDef`]'s `skip_serializing_if`, so a config that never
    /// wrote a `defaults` block round-trips without one — while a block
    /// holding only annotations still survives (annotations are the one
    /// content `extra` is FOR). Spelled as whole-struct equality, not a
    /// per-field predicate, so a knob added by a later task (P8d/P8e)
    /// cannot be silently DROPPED on serialize by an un-extended list.
    #[must_use]
    pub fn is_vacuous(&self) -> bool {
        *self == Self::default()
    }
}

/// The instant ONE cast's world is measured at — the moment the
/// executor captures the cast's world snapshot (its effective build and
/// effective phase, taken together), which every `Plan` evaluation in
/// that cast's completion transaction then reads: the damage overlay
/// ([`ActionDamage::stats`], whose [`NumOrExpr`] values evaluate AT the
/// measured instant), `hits_per_use`, the crit chance / EV `on_crit`
/// weight, and the tick capture of every `ApplyBuff` entry in the
/// action's [`ActionDef::effects`] list (build AND phase — one world per
/// cast, P8c).
///
/// Configured package-wide via [`SimDefaults::measure`] and overridden
/// per action via [`ActionDef::measure`]. **Default:
/// [`Measure::CastComplete`]** — the 0.3.0 instant, byte-identical for
/// every config that names neither field.
///
/// # Interactions worth knowing before switching
///
/// - **`casts.<self>`** in a `damage.stats` expression: under
///   `cast_complete` it INCLUDES the completing cast (counts from `1` on
///   the first cast — the P7b rule); under `cast_start` the in-flight
///   cast has not been counted yet, so it does NOT (counts from `0`).
///   The same shift applies to every sim-state read in the overlay: a
///   resource reads its cast-start amount (post-cost, pre-`gain`), not
///   its post-`gain` one.
/// - **Sim-FIELD expressions are NOT governed by this knob.**
///   `duration`, `cost`, `gain` and `cooldown` keep their own documented
///   instants ([`NumOrExpr`]'s table) and their live sequential
///   sim-state reads — "one world" governs `Plan` evaluations, never
///   sim-state reads. A pandemic-style `duration` still reads the live
///   `buff_remaining.<self>` at application.
/// - **A proc-fired free cast is measured at ITS OWN instant, live** —
///   it is never frozen to the triggering cast's snapshot, whatever
///   either action's `measure` says (a free cast begins and completes at
///   the firing proc's instant, so the two measures coincide there).
/// - **Instant casts** (`cast_time` `0`): an instant cast is ALWAYS
///   measured at the completion position, whatever `measure` says. Cast
///   start and cast complete share the wall-clock instant, but the
///   intra-instant positions differ — the completion capture runs
///   post-`gain`, post-`casts` increment — so under `cast_start` a
///   zero-time cast's `casts.<self>` counts from 1 while an
///   epsilon-time cast's counts from 0. That discontinuity is the
///   documented behavior, not an accident, and it is pinned
///   (`an_instant_cast_is_measured_at_the_completion_position_even_under_cast_start`).
///
/// `#[non_exhaustive]` for [`EffectDef`]'s reason: a third measurement
/// instant (a projectile-impact delay, say) is plausible and would have
/// to land here, so an exhaustive `match` downstream would make that a
/// breaking change for no gain. (Contrast [`NumOrExpr`], deliberately
/// exhaustive: "number or expression" is a closed set by construction; a
/// measurement-instant vocabulary is not.) Variants stay freely
/// constructible; only an exhaustive `match` needs a wildcard arm.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Measure {
    /// Measure in the completion transaction — after `gain` is credited
    /// and the cast is counted, before the action's effects and its proc
    /// rolls. The 0.3.0 instant, and the default.
    #[default]
    CastComplete,
    /// Measure when the cast BEGINS — after the cost is paid and the
    /// cooldown armed, i.e. against the world the cast leaves behind as
    /// it starts. The snapshot then rides the in-flight cast to its
    /// completion transaction, where every `Plan` evaluation reads it.
    CastStart,
}

/// A literal number or an expression string, evaluated at a documented
/// instant.
///
/// Untagged: a JSON number deserializes to [`NumOrExpr::Num`] and a JSON
/// string to [`NumOrExpr::Expr`]. Every rtce 0.2.0 config — which only
/// ever wrote plain numbers in these positions — therefore PARSES
/// unchanged, and a literal reaches the executor as the identical `f64`
/// the old field held: `Num` is pre-baked into a constant at
/// `sim::compile`, so the literal path adds no evaluation and no rounding.
/// (That is a statement about THIS type only. The 0.3.0 release notes
/// carry two deliberate executor behavior FIXES landed alongside it, which
/// can move results for configs whose proc `chance` reads state another
/// proc mutates in the same trigger batch, or which have an `on_hit` proc
/// that changes crit chance — see the CHANGELOG.)
///
/// An `Expr` is parsed at `sim::compile` against the sim symbol space (see
/// the `sim` module docs), with the usual positioned, fail-closed error
/// for an unknown identifier or a syntax problem. Pipeline stages and
/// buckets are NOT in that space — naming one is a compile error, the same
/// as any other unresolved name.
///
/// # Evaluation instants
///
/// An expression is re-evaluated every time its field is USED, at the
/// instant named below — never once up front:
///
/// | Field | Evaluated |
/// |---|---|
/// | [`BuffDef::duration`] | at application (snapshotted onto that window) |
/// | [`ActionDef::cooldown`] | at cast start |
/// | [`ActionDef::cost`] values | at cast start — and at every decision that merely CHECKS affordability |
/// | [`ActionDef::gain`] values | at cast complete |
/// | [`ActionDamage::stats`] values | at the action's MEASURED instant — cast complete by default, cast start under [`Measure::CastStart`] |
///
/// A cost expression is therefore RE-CHECKED at each decision point, never
/// PREDICTED: the executor's resource-affordability wake time is solved
/// from the cost as evaluated at that decision instant, and if the
/// expression's value has changed by the time the wake fires, the wake
/// simply re-decides at the new value (see `sim::exec`'s `afford`).
///
/// Keep cost expressions CHEAP for that reason. A cost is the only one of
/// these fields evaluated on a hot path — once per rule per decision point
/// — where the others are evaluated once per cast or per buff
/// application. A literal costs nothing at all (it is a pre-baked
/// constant), so leave a fixed cost as a number.
///
/// # Fail-closed
///
/// At its evaluation instant a value that is not finite is a run error
/// naming the field and the instant; `duration`/`cooldown`/`cost`/`gain`
/// additionally reject a negative result. [`ActionDamage::stats`] values
/// may legitimately be negative (a stat is not a quantity of anything), so
/// only finiteness is enforced there.
///
/// Deliberately NOT `#[non_exhaustive]`, unlike the compiled
/// [`crate::sim::CompiledValue`]: this is a CONFIG type, "number or
/// expression" is the whole idea, and a caller building or inspecting a
/// `SimDef` in Rust should be able to match it exhaustively.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NumOrExpr {
    /// Literal value (backward compatible with 0.2.0 configs).
    Num(f64),
    /// Expression over the sim symbol space.
    Expr(String),
}

/// Hand-written (P8a) so a malformed value reports what was EXPECTED —
/// "expected a number (literal) or a string (expression)" — instead of
/// serde's "data did not match any variant of untagged enum". The
/// accepted inputs are exactly the untagged derive's: any JSON number →
/// [`NumOrExpr::Num`] (integers via the same `u64`/`i64` → `f64`
/// conversion the derive applied — lossy above 2^53, identically so),
/// any JSON string → [`NumOrExpr::Expr`].
/// Serialization is unchanged (still the untagged derive above).
impl<'de> Deserialize<'de> for NumOrExpr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = NumOrExpr;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a number (literal) or a string (expression)")
            }
            fn visit_f64<E>(self, v: f64) -> Result<NumOrExpr, E> {
                Ok(NumOrExpr::Num(v))
            }
            fn visit_u64<E>(self, v: u64) -> Result<NumOrExpr, E> {
                Ok(NumOrExpr::Num(v as f64))
            }
            fn visit_i64<E>(self, v: i64) -> Result<NumOrExpr, E> {
                Ok(NumOrExpr::Num(v as f64))
            }
            fn visit_str<E>(self, v: &str) -> Result<NumOrExpr, E> {
                Ok(NumOrExpr::Expr(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> Result<NumOrExpr, E> {
                Ok(NumOrExpr::Expr(v))
            }
        }
        d.deserialize_any(V)
    }
}

/// `0.0` — the same default the `f64` fields carried before these became
/// expression-valued, so `#[serde(default)]` on `cooldown`/`cost`/`gain`
/// keeps its 0.2.0 meaning.
impl Default for NumOrExpr {
    fn default() -> Self {
        NumOrExpr::Num(0.0)
    }
}

/// A number is always a literal: `NumOrExpr::from(40.0)` is
/// [`NumOrExpr::Num`], the pre-baked-constant path.
impl From<f64> for NumOrExpr {
    fn from(v: f64) -> Self {
        NumOrExpr::Num(v)
    }
}

/// A string is ALWAYS an expression, even when it looks like a number:
/// `NumOrExpr::from("40")` is [`NumOrExpr::Expr`] — a compiled program
/// that evaluates to 40, not the literal [`NumOrExpr::Num`]`(40.0)`. The
/// two agree on every value they produce (the grammar's numeric literals
/// are `f64`), so this is a representation difference, not a semantic
/// one — but it is the one place this type can be misread, and it does
/// cost a `Program` and an evaluation. Write `40.0.into()` for a
/// constant.
impl From<&str> for NumOrExpr {
    fn from(s: &str) -> Self {
        NumOrExpr::Expr(s.to_string())
    }
}

/// As [`From<&str>`][`NumOrExpr::from`]: a string is always an expression.
impl From<String> for NumOrExpr {
    fn from(s: String) -> Self {
        NumOrExpr::Expr(s)
    }
}

/// One resource (mana, spirit, fury, …): a capped pool that regenerates
/// continuously and is spent/gained by actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDef {
    /// Expression (over stats/conditions) for this resource's cap.
    pub max: String,
    /// Expression (over stats/conditions) for the per-second regen rate.
    pub regen_per_sec: String,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming this resource.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ResourceDef {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &["max", "regen_per_sec"];
}

/// One action the rotation can cast: timing, resource cost/gain, and an
/// optional damage effect.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionDef {
    /// Expression (over stats/conditions/sim-state; never pipeline
    /// stages/buckets) for the cast time in seconds. `"0"` = instant.
    pub cast_time: String,
    /// Cooldown in seconds, starting when the cast begins (`0.0` = none).
    /// Literal or expression; an expression is evaluated AT CAST START
    /// (before the cost is deducted) and must be finite and `>= 0` —
    /// see [`NumOrExpr`].
    #[serde(default)]
    pub cooldown: NumOrExpr,
    /// Resource cost paid when the cast begins, by resource name. Literal
    /// or expression; an expression is evaluated AT CAST START, and also
    /// at every decision point that checks whether this action is
    /// affordable — see [`NumOrExpr`] for why a cost expression is
    /// re-checked rather than predicted.
    #[serde(default)]
    pub cost: BTreeMap<String, NumOrExpr>,
    /// Resource gained when the cast completes, by resource name (e.g.
    /// basic/generator skills). Literal or expression; an expression is
    /// evaluated AT CAST COMPLETE and must be finite and `>= 0` — see
    /// [`NumOrExpr`].
    #[serde(default)]
    pub gain: BTreeMap<String, NumOrExpr>,
    /// This action's damage effect, if any (omit for utility-only casts).
    #[serde(default)]
    pub damage: Option<ActionDamage>,
    /// Per-action override of [`SimDefaults::measure`] — the instant THIS
    /// action's casts are measured at (see [`Measure`] for the semantics,
    /// the default, and the interactions). `None` (the default) defers to
    /// the `defaults` block; two actions in one rotation may resolve to
    /// different instants.
    ///
    /// Irrelevant for an action only ever cast FREE by a proc's
    /// `cast_action` effect: a free cast begins and completes at the
    /// firing proc's instant and is always measured live there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<Measure>,
    /// Buffs this action applies when its cast COMPLETES — one
    /// application per entry, in list order.
    ///
    /// DEPRECATED (kept for 0.x; prefer [`ActionDef::effects`]): sugar
    /// for an `effects` list of `{ "apply_buff": … }` entries, one per
    /// name, in this list's order — the desugared form compiles
    /// byte-for-byte identically, and EVERYTHING on
    /// [`ActionDef::effects`] (the instant, list order, repeats, the
    /// three-axes rule) applies to this field verbatim; the semantics
    /// are documented THERE, once. Setting this alongside an explicit
    /// `effects` list is a compile error (ambiguous order); migrate the
    /// sugar into the list.
    ///
    /// NB the ARITY differs from [`ProcDef::apply_buff`], which is a
    /// single `Option<String>` rather than a list. Same key, same
    /// concept, different shape: writing `"apply_buff": ["x"]` on a proc
    /// is a serde type error, and `"apply_buff": "x"` on an action is
    /// too. The harmonization the 0.3.0 docs deferred to ROADMAP is
    /// [`ActionDef::effects`]/[`ProcDef::effects`]: one list shape on
    /// both entities.
    ///
    /// Fail-closed at `sim::compile`: a name that is not a defined buff.
    #[serde(default)]
    pub apply_buff: Vec<String>,
    /// Ordered list of effects this action executes when its cast
    /// COMPLETES — each `{ "apply_buff": … }` entry is one application of
    /// the named buff, routed through its [`BuffDef::on_reapply`] policy
    /// exactly as a proc application is. Empty (the default) means this
    /// action applies nothing, which is every rtce 0.2.0 config; the
    /// deprecated [`ActionDef::apply_buff`] sugar desugars into this list
    /// (naming both on one action is an "ambiguous order" compile
    /// error).
    ///
    /// This is the first-class replacement for the "icd equals the gating
    /// action's cooldown" trick a 0.2.0 config needed in order to coerce
    /// per-action buff application out of a globally-triggered
    /// [`ProcDef`].
    ///
    /// A `{ "cast_action": … }` entry is NOT allowed here — proc-only.
    /// An action free-casting an action reopens the recursion the
    /// free-cast guard closed (A→B→A), and a bounded-depth chain design
    /// should be chosen by a config that needs one (see `ROADMAP.md`);
    /// `sim::compile` rejects it with exactly that explanation.
    ///
    /// # Where it lands
    ///
    /// AFTER this cast's own damage is measured and credited, and BEFORE
    /// any of this cast's proc rolls — see the [`crate::sim`] module docs
    /// for the full cast-complete order and what each step can see. The
    /// two consequences worth knowing here: the applying cast does NOT
    /// benefit from the buff it applies, and a proc rolled by that cast
    /// DOES see it.
    ///
    /// # Within one list
    ///
    /// Entries are applied in LIST order, and a buff repeated in the list
    /// is applied that many times — under `add_independent` that is two
    /// instances; under `refresh` the second application simply replaces
    /// the first.
    ///
    /// # What a later entry sees: TWO axes — one world per cast (P8c)
    ///
    /// Each axis is deliberate and each is pinned:
    ///
    /// - **Sim STATE is SEQUENTIAL.** The slot array is refreshed per
    ///   application, so a later entry sees earlier entries' live windows,
    ///   STACK COUNTS and resource amounts. A [`BuffDef::duration`]
    ///   expression reads this axis: `"2 * (1 + stacks.earlier)"` works
    ///   and means what it says.
    /// - **The measured WORLD is the cast's ONE snapshot** — the
    ///   effective build AND the effective phase, captured together at
    ///   the action's [`Measure`] instant, before the list runs. A
    ///   SNAPSHOT [`TickObjective`] magnitude reads this axis: an
    ///   `["empower", "ailment"]` list gives the ailment neither
    ///   `empower`'s bucket contribution nor a condition `empower`
    ///   drives — put `empower` on an earlier CAST if that is what you
    ///   want. LIST ORDER cannot change a captured rate.
    ///
    /// The second axis is P8c's one deliberate behavior fix. Through
    /// 0.3.0 the build was frozen but the PHASE stayed live, so for a
    /// snapshot `tick_objective` whose objective read a condition, list
    /// order alone doubled the captured rate — invisibly, since
    /// [`crate::sim::SimReport`]'s integrated columns (`uptime`,
    /// `avg_stacks`) were identical either way. Both orderings now
    /// capture the pre-list world; pinned — the equality AND the
    /// literal — by `a_same_list_snapshot_capture_reads_one_frozen_world`.
    ///
    /// What the snapshot buys, and what the P7d build-freeze was always
    /// for, still holds: the two action paths agree, in every list
    /// order. A DAMAGING action's world carries its
    /// [`ActionDamage::stats`] overlay (so an ailment inherits the
    /// magnitude of the hit that applied it — PoE2 semantics), and a
    /// UTILITY action's carries the plain effective build; the same list
    /// means the same thing on both. The PROC path is deliberately
    /// different and unchanged: a proc-applied buff captures the LIVE
    /// ambient world at the fire, with no action's overlay on it —
    /// sequential across the proc's own effects list (the P8b rule).
    ///
    /// # Elsewhere
    ///
    /// Run by a proc-triggered FREE cast of this action (a `cast_action`
    /// proc effect) too, under that free cast's own world — measured
    /// LIVE at the firing proc's instant, never the triggering cast's
    /// snapshot ([`Measure`]'s free-cast boundary). An effect here is an
    /// effect OF the action, like `gain` and `damage`, not part of the
    /// cast pipeline (cost, cooldown, further proc rolls) that the
    /// free-cast path deliberately skips. And because `cast_action`
    /// cannot appear in THIS list, a free cast never chains into
    /// another.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectDef>,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming this action.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ActionDef {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &[
        "cast_time",
        "cooldown",
        "cost",
        "gain",
        "damage",
        "measure",
        "apply_buff",
        "effects",
    ];
}

/// Per-cast stat overrides folded onto the `Plan`'s `BuildState` before the
/// `damage_objective` is evaluated for this cast. `hits_per_use` (default
/// `1.0` when absent) is read directly by the executor as the per-cast hit
/// count rather than fed into the `Plan` as a stat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionDamage {
    /// Stat name → override value, applied only while resolving this
    /// action's damage. Literal or expression; an expression is evaluated
    /// at the action's MEASURED instant ([`Measure`] — cast complete by
    /// default, cast start under `measure: "cast_start"`), ONCE per cast
    /// (the same evaluated overlay feeds the damage query and the
    /// `on_crit` proc weight), and need only be FINITE — a stat may
    /// legitimately be negative. `hits_per_use` lives in this map and
    /// follows the same rule. See [`NumOrExpr`].
    ///
    /// Under the DEFAULT measure the completion instant has internal
    /// ORDER (stated in full in the [`crate::sim`] module docs), and
    /// these expressions are evaluated at a fixed point within it: AFTER
    /// this action's [`ActionDef::gain`] is credited and after its own
    /// cast is counted, and BEFORE both this action's
    /// [`ActionDef::effects`] and any of this cast's proc rolls. So an
    /// expression here reads a resource at its POST-gain amount, and
    /// `casts.<this action>` INCLUDES the cast being resolved (`1` on the
    /// first cast, never `0`). Under `cast_start` both readings shift to
    /// the cast-start world: the resource is post-cost/pre-gain, and the
    /// in-flight cast is NOT counted — see [`Measure`].
    ///
    /// NEITHER kind of buff this cast applies is visible: not one applied
    /// by its own procs, and not one in its own
    /// [`ActionDef::effects`] list. Same reason for both, and it is the
    /// point rather than a limitation — a hit cannot be changed by what it
    /// causes. The effects-list case is the newer and more surprising one,
    /// since that buff is written on the action ITSELF and still does not
    /// reach the action's own damage.
    ///
    /// PRECEDENCE: a [`crate::scenario::Phase`] `stats` override for the
    /// same stat WINS over this overlay (phase > overlay > build — the
    /// phase is written last into the slot array). Overriding a stat here
    /// is therefore not a guarantee for the phases that also name it.
    #[serde(default)]
    pub stats: BTreeMap<String, NumOrExpr>,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming the owning action.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ActionDamage {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &["stats"];
}

/// What happens when a buff is applied while it is ALREADY active — the
/// per-mechanic collapse of the engine's unified instance list (see the
/// P7 design spec's "Stack model — Approach B" decision).
///
/// The default, [`ReapplyPolicy::Refresh`], is rtce 0.2.0's only behavior:
/// a buff is one window, and reapplying it resets that window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReapplyPolicy {
    /// One instance, its expiry reset to `now + duration`. The binary
    /// buff — and the degenerate case of the whole instance model, which
    /// is why it is the default and why [`BuffDef::max_stacks`] must be
    /// `1` alongside it.
    ///
    /// With a SNAPSHOT [`BuffDef::tick_objective`], the replacement
    /// re-captures the rate UNCONDITIONALLY — so a reapplication in a
    /// weaker moment LOWERS the DoT, which is exactly the opposite of
    /// [`ReapplyPolicy::Strongest`]. Pick deliberately between them.
    #[default]
    Refresh,
    /// Count `+1` up to [`BuffDef::max_stacks`], then EVERY instance's
    /// expiry is reset to `now + duration` — one shared clock, so the
    /// whole stack falls off together (PoE2 charges). At the cap no new
    /// instance is added, but the shared clock is still reset.
    ///
    /// With a SNAPSHOT [`BuffDef::tick_objective`], the shared clock moves
    /// EXPIRIES ONLY: an existing instance keeps the rate it captured,
    /// however long its window is subsequently extended for. Two
    /// consequences worth knowing before choosing this policy — at the cap
    /// the incoming application captures nothing at all (no instance is
    /// added, so its rate is discarded while the clock still resets), and
    /// a continuously-refreshed capped stack can therefore ride a rate
    /// captured arbitrarily long ago.
    AddRefreshAll,
    /// A new instance with its OWN duration, expiring independently
    /// (PoE2 poison). At [`BuffDef::max_stacks`] the earliest-expiring
    /// instance is evicted to make room — the earliest-EXPIRING, not the
    /// weakest, so with a SNAPSHOT [`BuffDef::tick_objective`] a capped
    /// stack can evict a strong instance in favour of a weak one.
    AddIndependent,
    /// A new instance replaces the incumbent only if its snapshot rate is
    /// STRICTLY higher (PoE2 ignite). It needs a magnitude to compare, so
    /// it requires a `tick_objective` with [`TickObjective::snapshot`] set
    /// — and, being a replacement rather than a stack, a
    /// [`BuffDef::max_stacks`] of `1`. Both are fail-closed compile
    /// errors, never silently-borrowed behavior from another policy.
    ///
    /// A LOSING application is discarded WHOLE: it changes neither the
    /// live instance's rate nor its expiry. A weak reapplication cannot
    /// extend a strong ailment — which is the mechanic's whole point, and
    /// the one place `strongest` differs observably from "replace and
    /// refresh, but keep the higher rate".
    Strongest,
}

/// Which `Plan` objective a buff DoT-ticks, and HOW each instance samples
/// that objective's rate.
///
/// Two JSON shapes, both accepted (untagged):
///
/// ```json
/// "tick_objective": "dot_dps"
/// "tick_objective": { "objective": "dot_dps", "snapshot": true }
/// ```
///
/// The bare string is rtce 0.2.0's form and means `snapshot: false` — so
/// every 0.2.0 config parses and behaves unchanged. An object with
/// `snapshot: false` means exactly the same thing as the bare string and
/// SERIALIZES back to it; the object form only carries information when
/// `snapshot` is `true`. An unrecognized key inside the object form is
/// rejected rather than ignored (a typo'd `"snapshots": true` silently
/// meaning "live" is precisely the silent wrong answer this crate refuses
/// to give) — with the P8a carve-out that `_`-prefixed keys are the
/// annotation namespace and pass here as at every other nesting level
/// (accepted and dropped; this struct stores no `extra`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "TickObjectiveRepr")]
pub struct TickObjective {
    /// The `Plan` objective whose value is this buff's DoT rate. Must name
    /// an objective of the plan the sim compiles against.
    pub objective: String,
    /// How the rate is sampled while the buff is live:
    ///
    /// - `false` (LIVE, the 0.2.0 behavior) — the objective is
    ///   re-evaluated on every state change and the buff ticks that value
    ///   × its live instance count. A stat or phase change moves the rate
    ///   of every live instance retroactively-from-now.
    /// - `true` (SNAPSHOT) — each instance CAPTURES the objective's value
    ///   at its own application instant and ticks that value unchanged to
    ///   expiry, whatever happens to the state afterwards. The buff's
    ///   total rate is the SUM over live instances, so the stack count is
    ///   already inherent in it and is never multiplied in a second time.
    ///   This is PoE2 ailment semantics, and what
    ///   [`ReapplyPolicy::Strongest`] compares instances by.
    ///
    /// A PROC-applied capture is taken at the application instant against
    /// the live state the instance LANDS ON — before this application's
    /// own effects fold in. So if the buff's own
    /// [`BuffDef::contributions`] feed the objective it ticks, it
    /// SELF-AMPLIFIES on reapplication, one application behind: the first
    /// instance captures the un-buffed rate, the second captures it with
    /// one stack live, and so on. Deliberate, and the same instant
    /// [`BuffDef::duration`] is evaluated at; if you want the first
    /// instance to see itself, it cannot be expressed here. (An
    /// ACTION-applied capture — an [`ActionDef::effects`] entry — reads
    /// the applying CAST's one measured world instead, at that action's
    /// [`Measure`] instant: build and phase together, P8c. The
    /// self-amplification story is the same, one application behind.)
    ///
    /// What each [`ReapplyPolicy`] does with a captured rate differs
    /// sharply — see the variants; `refresh` re-captures unconditionally,
    /// `add_refresh_all` never re-captures, and `strongest` re-captures
    /// only on an improvement.
    pub snapshot: bool,
}

impl TickObjective {
    /// A LIVE tick objective (`snapshot: false`) — the 0.2.0 semantics,
    /// and what a bare `"name"` in JSON parses to.
    #[must_use]
    pub fn live(objective: impl Into<String>) -> Self {
        TickObjective {
            objective: objective.into(),
            snapshot: false,
        }
    }

    /// A SNAPSHOT tick objective (`snapshot: true`) — each instance ticks
    /// the rate it captured at its own application.
    #[must_use]
    pub fn snapshot(objective: impl Into<String>) -> Self {
        TickObjective {
            objective: objective.into(),
            snapshot: true,
        }
    }
}

/// The two JSON shapes [`TickObjective`] SERIALIZES to, as an untagged
/// enum — the serialize-only representation `TickObjective` converts
/// through, so the struct itself stays a plain two-field record
/// everywhere else. (Deserialization is the hand-written visitor below,
/// which replaced 0.3.0's `TickObjectiveObj` + `deny_unknown_fields`
/// machinery in P8a — the crate-wide unknown-key policy that machinery's
/// docs wished for now exists, and this type follows it.)
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum TickObjectiveRepr {
    /// `"dot_dps"` — the 0.2.0 shape, always live.
    Name(String),
    /// `{ "objective": "dot_dps", "snapshot": true }`.
    Object { objective: String, snapshot: bool },
}

impl From<TickObjective> for TickObjectiveRepr {
    /// A live objective serializes back to the BARE STRING it most likely
    /// came from, so round-tripping a 0.2.0 config never rewrites its
    /// `tick_objective` into the object form.
    fn from(t: TickObjective) -> Self {
        if t.snapshot {
            TickObjectiveRepr::Object {
                objective: t.objective,
                snapshot: true,
            }
        } else {
            TickObjectiveRepr::Name(t.objective)
        }
    }
}

/// Hand-written (P8a): a bare string is the live 0.2.0 form; a map takes
/// exactly `objective` (required) and `snapshot` (default `false`), plus
/// `_`-prefixed annotation keys, which are skipped. Any other key is the
/// fail-closed error "unknown field `…`, expected `objective` or
/// `snapshot`" — positioned, instead of serde's untagged "did not match
/// any variant".
impl<'de> Deserialize<'de> for TickObjective {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = TickObjective;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an objective name or `{ \"objective\": …, \"snapshot\": … }` object")
            }
            fn visit_str<E>(self, v: &str) -> Result<TickObjective, E> {
                Ok(TickObjective::live(v))
            }
            fn visit_string<E>(self, v: String) -> Result<TickObjective, E> {
                Ok(TickObjective::live(v))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<TickObjective, A::Error> {
                use serde::de::Error as _;
                let mut objective: Option<String> = None;
                let mut snapshot: Option<bool> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "objective" => {
                            if objective.is_some() {
                                return Err(A::Error::duplicate_field("objective"));
                            }
                            objective = Some(map.next_value()?);
                        }
                        "snapshot" => {
                            if snapshot.is_some() {
                                return Err(A::Error::duplicate_field("snapshot"));
                            }
                            snapshot = Some(map.next_value()?);
                        }
                        _ if key.starts_with('_') => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                        _ => {
                            return Err(A::Error::custom(format!(
                                "unknown field `{key}`, expected `objective` or `snapshot`"
                            )));
                        }
                    }
                }
                let objective = objective.ok_or_else(|| A::Error::missing_field("objective"))?;
                Ok(TickObjective {
                    objective,
                    snapshot: snapshot.unwrap_or(false),
                })
            }
        }
        d.deserialize_any(V)
    }
}

/// One buff/debuff: a timed window that, while active, contributes to
/// buckets, drives condition values, and/or accrues a DoT objective.
///
/// A buff is internally an INSTANCE LIST (P7c): every application pushes,
/// refreshes, or replaces an instance per [`BuffDef::on_reapply`], and the
/// live instance count is the buff's stack count. What that count scales,
/// and what it deliberately does NOT:
///
/// - **`contributions`** — each contribution's VALUE is multiplied by the
///   stack count. In a `summed_group` bucket 3 stacks of `+10` fold as
///   `+30`; in a `product` bucket they fold as `×(1 + 30/100)`, NOT
///   `×(1 + 10/100)³`. A per-stack effect is written once, at its
///   per-stack magnitude.
/// - **`conditions`** — driven at their FULL configured value while at
///   least one instance is live, and never scaled by the stack count. A
///   condition is an uptime fraction (`vulnerable = 1.0`), not a
///   quantity, so "3 stacks of vulnerable" has no meaning; the precedence
///   rule (an active buff wins over the scenario's static uptime) is
///   unchanged from 0.2.0.
/// - **`tick_objective`** — a LIVE one ticks at its re-evaluated rate
///   multiplied by the stack count; a SNAPSHOT one ticks the SUM of the
///   rates its instances captured at their own applications, where the
///   count is already inherent in the sum. See [`TickObjective`].
/// - **`buff.<name>`** — `1` while ANY instance is live (never the count;
///   `stacks.<name>` is the count). **`buff_remaining.<name>`** — the
///   LONGEST remaining window across live instances, which under
///   [`ReapplyPolicy::AddIndependent`] is the newest instance's, not the
///   one that expires next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuffDef {
    /// Seconds this buff lasts once applied (refresh-on-reapply resets
    /// the remaining duration back to this value).
    ///
    /// Literal or expression. An expression is evaluated AT EACH
    /// APPLICATION and SNAPSHOTTED onto the window it starts (or
    /// refreshes): a stat/phase change afterwards never retroactively
    /// lengthens or shortens a window already in flight — the NEXT
    /// application re-evaluates and gets the new value. Must be finite and
    /// `>= 0` — see [`NumOrExpr`].
    ///
    /// It reads the LIVE state at that instant, which differs between the
    /// two application paths — deliberately, and this buff's own effects
    /// are never un-folded to hide the difference:
    ///
    /// - **First application** (this buff not currently active):
    ///   `buff.<self>` is `0`, `buff_remaining.<self>` is `0`, and a
    ///   condition this buff drives reads its non-buff value.
    /// - **Refresh** (this buff already active): the outgoing window is
    ///   still in force, so `buff.<self>` is `1`, a condition this buff
    ///   drives reads its BUFF-DRIVEN value, and `buff_remaining.<self>`
    ///   is the time left on the window being REPLACED.
    ///
    /// The refresh reading is what makes pandemic-style refreshes
    /// expressible as data — `"min(12, buff_remaining.x + 8)"` extends by
    /// 8s up to a 12s cap. Bucket CONTRIBUTIONS are invisible on both
    /// paths: buckets are not in the sim symbol space at all.
    ///
    /// NB: the reserved sim symbol `duration` is the SCENARIO's total
    /// length in seconds, not this field. An expression here that names
    /// `duration` reads the fight length.
    pub duration: NumOrExpr,
    /// Bucket contributions active while this buff is up, folded into the
    /// effective build alongside the base `BuildState`'s own.
    #[serde(default)]
    pub contributions: Vec<Contribution>,
    /// Condition name → value while this buff is active. Per the spec's
    /// precedence rule, an active buff driving a condition WINS over the
    /// scenario's static uptime for that condition; the static uptime
    /// applies again once the buff expires.
    ///
    /// # Two buffs driving one condition
    ///
    /// The winner is the one whose NAME sorts first, and that is the whole
    /// rule — there is no "strongest wins", no "most recently applied
    /// wins", and no summing. Buffs are compiled in name-sorted order and
    /// the executor takes the first live match in that order, so a config
    /// with `chill` and `frost` both driving `slowed` reports `chill`'s
    /// value whenever both are up, and **renaming a buff can change the
    /// number**. Pinned by
    /// `two_buffs_driving_one_condition_resolve_by_buff_name_order`.
    ///
    /// Prefer not to arrange it: give each buff its own condition, or
    /// drive the shared one from a single buff. If you do rely on it, the
    /// dependence on naming is worth a comment in your own config.
    ///
    /// # Range
    ///
    /// A condition is an uptime FRACTION. Values outside `[0, 1]` are
    /// clamped where they fold (by `Plan`) and, since 0.3.0, where they
    /// are REPORTED (see [`crate::sim::SimReport::condition_uptime`]), so
    /// `1.0` and `5.0` behave identically. Not rejected at compile time —
    /// but nothing is gained by writing more than `1.0`.
    #[serde(default)]
    pub conditions: BTreeMap<String, f64>,
    /// The `Plan` objective this buff DoT-ticks, if any: while it is
    /// active, that objective's value × active-seconds accrues into the
    /// sim's damage total.
    ///
    /// Written either as a bare objective name (LIVE — the rate is
    /// re-evaluated on every state change and multiplied by the STACK
    /// COUNT) or as `{ "objective": …, "snapshot": true }` (each instance
    /// ticks the rate it captured at its own application, and the buff's
    /// rate is the SUM over instances). See [`TickObjective`].
    #[serde(default)]
    pub tick_objective: Option<TickObjective>,
    /// How many instances may be live at once. `0` means UNBOUNDED (a
    /// poison stream that only expiry ever trims). Defaults to `1`, which
    /// with the default [`ReapplyPolicy::Refresh`] is exactly rtce
    /// 0.2.0's binary buff — so every 0.2.0 config keeps its behavior
    /// without naming either field.
    ///
    /// `refresh` keeps exactly one instance by definition, so a
    /// `max_stacks` other than `1` alongside it is a fail-closed compile
    /// error rather than a silently-ignored number.
    #[serde(default = "default_max_stacks")]
    pub max_stacks: u32,
    /// What an application does when this buff is already active — see
    /// [`ReapplyPolicy`]. Defaults to [`ReapplyPolicy::Refresh`].
    #[serde(default)]
    pub on_reapply: ReapplyPolicy,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming this buff (the
    /// misspelled `tick_objectiv` that 0.3.0 silently read as "no DoT"
    /// is exactly what this catches).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl BuffDef {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &[
        "duration",
        "contributions",
        "conditions",
        "tick_objective",
        "max_stacks",
        "on_reapply",
    ];
}

/// `1` — one live instance, the 0.2.0 binary buff (see
/// [`BuffDef::max_stacks`]). A free function because `#[serde(default)]`
/// on an integer field would otherwise mean `0`, which this field spells
/// "unbounded".
fn default_max_stacks() -> u32 {
    1
}

/// `max_stacks: 1`, `on_reapply: refresh` — the 0.2.0 binary buff, matching
/// what serde fills in for a config that names neither field. Hand-written
/// rather than derived precisely because a derived `u32` default would be
/// `0` ("unbounded").
impl Default for BuffDef {
    fn default() -> Self {
        BuffDef {
            duration: NumOrExpr::default(),
            contributions: Vec::new(),
            conditions: BTreeMap::new(),
            tick_objective: None,
            max_stacks: default_max_stacks(),
            on_reapply: ReapplyPolicy::default(),
            extra: BTreeMap::new(),
        }
    }
}

/// What event a [`ProcDef`] rolls its chance against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Rolls once per cast begun.
    OnCast,
    /// Rolls ONCE per completing cast of a DAMAGING action — not once per
    /// hit.
    ///
    /// The distinction matters and is easy to misread from the name: an
    /// action whose `damage.stats` sets `hits_per_use: 5` puts five hits'
    /// worth of damage into the total but presents this trigger with
    /// exactly ONE roll — weight `1.0` in [`crate::sim::Mode::Expected`],
    /// one RNG draw in [`crate::sim::Mode::MonteCarlo`]. A lucky-hit-style
    /// proc that should scale with a multi-hit skill is therefore NOT
    /// expressible today; fold the per-hit rate into `chance` by hand if
    /// you need it. Pinned by
    /// `on_hit_rolls_once_per_cast_not_once_per_hit`; whether it SHOULD
    /// scale with `hits_per_use` is an open 0.4.0 question in `ROADMAP.md`.
    ///
    /// An action with no `damage` presents no roll at all.
    OnHit,
    /// Rolls once per completing cast of a damaging action, weighted by
    /// the probability that cast crit ([`crate::sim::Mode::Expected`]) or
    /// gated on whether the sampled branch actually crit
    /// ([`crate::sim::Mode::MonteCarlo`]). Per CAST, not per hit — see
    /// [`Trigger::OnHit`].
    OnCrit,
}

/// One effect of an action completing or a proc firing. Externally tagged:
/// `{ "apply_buff": "shock" }` / `{ "cast_action": "comet" }`.
///
/// `#[non_exhaustive]`, for [`crate::sim::CompiledEffect`]'s reason: a
/// later phase adding a third effect kind must land on BOTH enums, so
/// leaving this one exhaustive would make that a breaking change anyway
/// and defeat the compiled side's allowance. (Contrast [`NumOrExpr`],
/// which is deliberately NOT marked: "number or expression" is a closed
/// set by construction; an effect vocabulary is not — the ROADMAP's
/// combo-chains entry already sketches growth.) Variants stay freely
/// constructible downstream; only an exhaustive `match` needs a wildcard
/// arm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EffectDef {
    /// One application of the named buff (through its `on_reapply` policy).
    ApplyBuff(String),
    /// Free-cast the named action (gains + damage + its OWN ApplyBuff
    /// effects; no cost, no cooldown, no proc rolls — the P7d free-cast
    /// rules).
    CastAction(String),
}

/// Hand-written (P8a discipline, anticipated by the P8 spec): the derived
/// externally-tagged deserializer already rejected an unknown tag, but in
/// serde's own vocabulary ("unknown variant `apply_buf`…") — without the
/// crate's did-you-mean, and rejecting `_`-prefixed ANNOTATION keys that
/// every other nesting level accepts. This visitor speaks the crate's
/// shared `config_keys` wording instead ("unknown field `apply_buf` on
/// an effect entry — did you mean `apply_buff`?"), skips `_` keys, and
/// insists on exactly ONE effect key per entry — an entry IS one effect,
/// so `{}` and `{ "apply_buff": …, "cast_action": … }` are both spelled
/// out rather than half-read. Accepted inputs are otherwise exactly the
/// derive's; serialization is unchanged (the derive above).
impl<'de> Deserialize<'de> for EffectDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// The two effect keys, in declaration order — the `known` list
        /// for the did-you-mean.
        const KNOWN: &[&str] = &["apply_buff", "cast_action"];
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = EffectDef;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an effect entry: `{ \"apply_buff\": … }` or `{ \"cast_action\": … }`")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<EffectDef, A::Error> {
                use serde::de::Error as _;
                let mut effect: Option<EffectDef> = None;
                while let Some(key) = map.next_key::<String>()? {
                    let parsed = match key.as_str() {
                        "apply_buff" => EffectDef::ApplyBuff(map.next_value()?),
                        "cast_action" => EffectDef::CastAction(map.next_value()?),
                        _ if key.starts_with('_') => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                            continue;
                        }
                        _ => {
                            return Err(A::Error::custom(crate::config_keys::unknown_key_message(
                                &key,
                                "an effect entry",
                                KNOWN,
                            )));
                        }
                    };
                    if effect.is_some() {
                        return Err(A::Error::custom(
                            "an effect entry takes exactly one of `apply_buff` or \
                             `cast_action`, got more than one",
                        ));
                    }
                    effect = Some(parsed);
                }
                effect.ok_or_else(|| {
                    A::Error::custom(
                        "an effect entry needs exactly one of `apply_buff` or `cast_action`",
                    )
                })
            }
        }
        d.deserialize_map(V)
    }
}

/// One proc: a chance-triggered, ICD-gated ordered list of effects
/// ([`ProcDef::effects`] — apply buffs, cast free actions) rolled on a
/// trigger event. The 0.x sugar fields [`ProcDef::apply_buff`] /
/// [`ProcDef::cast_action`] each spell a one-entry list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcDef {
    /// Which event this proc rolls its chance against.
    pub trigger: Trigger,
    /// Expression (over stats/conditions/sim-state) for the fire chance
    /// per qualifying roll.
    pub chance: String,
    /// Internal cooldown in seconds after firing (`0.0` = none).
    ///
    /// Must be finite and `>= 0`; anything else is a fail-closed
    /// `sim::compile` error. NaN in particular is rejected rather than
    /// tolerated — the executor gates on `now < icd_ready_at`, which is
    /// false for every `now` once that deadline is NaN, so a NaN `icd`
    /// would DELETE the internal cooldown instead of tightening it.
    #[serde(default)]
    pub icd: f64,
    /// Buff to apply when this proc fires.
    ///
    /// DEPRECATED (kept for 0.x; prefer [`ProcDef::effects`]): sugar for
    /// a one-entry `effects` list — `"apply_buff": "x"` desugars at
    /// `sim::compile` into `"effects": [ { "apply_buff": "x" } ]`,
    /// byte-for-byte the same compiled form. Setting it alongside an
    /// explicit `effects` list is a compile error (ambiguous order), and
    /// setting BOTH sugar fields at once stays the error it always was.
    /// A proc must end up with at least one effect after desugar.
    ///
    /// NB the sugar applies AT MOST ONE buff, where
    /// [`ActionDef::apply_buff`] takes a LIST — so `["x"]` here is a
    /// serde type error, and a bare `"x"` on an action is too. The
    /// harmonization the 0.3.0 docs deferred to ROADMAP is `effects`
    /// itself: one list shape, both entities, any number of entries.
    #[serde(default)]
    pub apply_buff: Option<String>,
    /// Action to cast for free (does not consume the rotation's decision
    /// slot) when this proc fires.
    ///
    /// DEPRECATED (kept for 0.x; prefer [`ProcDef::effects`]): sugar for
    /// a one-entry `effects` list, under exactly the rules on
    /// [`ProcDef::apply_buff`]. Unlike the buff effect, `cast_action` is
    /// PROC-only in the list form too — see [`ActionDef::effects`] for
    /// why an action cannot free-cast an action.
    #[serde(default)]
    pub cast_action: Option<String>,
    /// Trigger filter: this proc's [`ProcDef::trigger`] only considers
    /// casts of the LISTED actions. `None` (the default) means every
    /// action — rtce 0.2.0's behavior, and what every 0.2.0 config gets
    /// without naming the field.
    ///
    /// Applies to all three triggers alike, because all three are events
    /// of a cast: the filter names the action whose cast produced the
    /// event, so `on_hit`/`on_crit` are filtered by the action that HIT.
    /// A filtered-out cast contributes NOTHING — in [`Trigger::OnHit`]'s
    /// EV accumulator it is not banked any more than an ICD-gated roll
    /// is, and in Monte Carlo mode it consumes no RNG draw.
    ///
    /// A proc-triggered free cast ([`ProcDef::cast_action`]) rolls no
    /// procs at all, so no event this filter could match ever originates
    /// there.
    ///
    /// # What it cannot express
    ///
    /// An inclusive list of casting actions, and nothing else. There is
    /// no negation and no "every action except" — an exclusion has to be
    /// written as the complementary list, which then has to be kept in
    /// step by hand as actions are added. And because the filter always
    /// matches the CASTING action, "on_hit, but only hits of actions
    /// other than this one" is not expressible at all. Left out
    /// deliberately for 0.3.0 rather than guessed at; noted in ROADMAP as
    /// a 0.4.0 candidate, where a real config should decide the shape.
    ///
    /// # Fail-closed
    ///
    /// At `sim::compile`: an unknown action name, and an EMPTY list —
    /// `actions: []` describes a proc that can never fire, which is a
    /// config mistake rather than a way to disable one. Write `None`
    /// (omit the key) for "every action".
    #[serde(default)]
    pub actions: Option<Vec<String>>,
    /// Ordered list of effects this proc executes when it FIRES — the
    /// first-class spelling of what [`ProcDef::apply_buff`] /
    /// [`ProcDef::cast_action`] each express one entry of. Executed in
    /// LIST order at the firing instant; a repeated entry applies that
    /// many times (the [`ActionDef::apply_buff`] list precedent). Between
    /// entries the sim-state tail is SEQUENTIAL (P7b): an `apply_buff`
    /// refolds the effective state, a `cast_action` free cast bumps
    /// `casts.<name>` and lands its gains/damage/own-ApplyBuff — so a
    /// later entry's `duration` expression sees all of it.
    ///
    /// Default: empty — but a proc must DO something, so a proc whose
    /// list is empty after the sugar fields desugar is a fail-closed
    /// `sim::compile` error. Setting this alongside either sugar field is
    /// an "ambiguous order" compile error: migrate the sugar into the
    /// list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectDef>,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming this proc.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ProcDef {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &[
        "trigger",
        "chance",
        "icd",
        "apply_buff",
        "cast_action",
        "actions",
        "effects",
    ];
}

/// A priority-list rotation (SimC-style): pure config, drivers may search
/// over rotations the same way they search over gear. The first eligible
/// [`Rule`] wins; hard gates (off cooldown, cost payable, not mid-cast)
/// are enforced by the engine automatically, `when` adds strategy on top.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Rotation {
    /// Priority-ordered rules; the first eligible one wins each decision
    /// point. No rule eligible → time advances to the next event (waiting
    /// is modeled, never an infinite loop).
    pub rules: Vec<Rule>,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Rotation {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &["rules"];
}

/// One rotation rule: cast `action` when its hard gates pass and (if
/// present) `when` evaluates truthy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// The action this rule casts when eligible.
    pub action: String,
    /// Optional extra eligibility predicate over the sim symbol space
    /// (hard gates are automatic and need not be repeated here). Absent =
    /// always willing (subject to hard gates).
    #[serde(default)]
    pub when: Option<String>,
    /// Unknown keys collected at parse — see [`SimDef`]'s "Unknown keys"
    /// section: `_`-prefixed annotations survive round-trips; anything
    /// else fails closed at `sim::compile`, naming this rule's position
    /// and action.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Rule {
    /// The declared field names, for `sim::compile`'s unknown-key walk.
    /// Staleness here only degrades the did-you-mean, never
    /// correctness — see `config_keys`' module docs ("Staleness").
    pub(crate) const KNOWN_KEYS: &'static [&'static str] = &["action", "when"];
}

/// Verbatim from docs/superpowers/specs/2026-07-22-p6-sequencing-design.md
/// "Config surface" section (jsonc comments stripped — serde_json has no
/// comment support; every key/value is copied unchanged, including the
/// `cast_time` formula `"1.0 / base_aps"`). Every position that P7b made
/// expression-valued is a plain JSON NUMBER here, which is exactly what
/// makes this the 0.2.0 compatibility fixture — shared by this module's
/// parse test and `sim::exec`'s behavioral one, so there is ONE copy and
/// no unchecked claim of byte-identity between two.
#[cfg(test)]
pub(crate) const P6_SPEC_SIMDEF_JSON: &str = r#"{
      "resources": {
        "mana": { "max": "max_mana", "regen_per_sec": "mana_regen" }
      },
      "actions": {
        "fireball": {
          "cast_time": "1.0 / base_aps",
          "cooldown": 0.0,
          "cost": { "mana": 40.0 },
          "gain": {},
          "damage": {
            "stats": { "coeff_pct": 200.0, "hits_per_use": 1.0 }
          }
        }
      },
      "buffs": {
        "vuln_window": { "duration": 4.0, "conditions": { "vulnerable": 1.0 } },
        "combustion":  { "duration": 8.0,
                         "contributions": [{ "bucket": "indep", "value": 25.0 }] },
        "burning":     { "duration": 6.0, "tick_objective": "dot_dps" }
      },
      "procs": {
        "conflagrate": { "trigger": "on_crit",
                         "chance": "lucky_hit_chance / 100 * 0.3",
                         "icd": 2.0,
                         "apply_buff": "combustion" }
      },
      "damage_objective": "hit_after_dr"
    }"#;

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim from the spec's "Rotation (candidate-domain …)" section.
    const ROTATION_JSON: &str = r#"{ "rules": [
      { "action": "frost_nova", "when": "cooldown.frost_nova == 0 and buff.vuln_window == 0" },
      { "action": "fireball",   "when": "mana >= 40" },
      { "action": "basic_bolt" }
    ]}"#;

    #[test]
    fn simdef_round_trips_the_spec_example_verbatim() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();

        assert_eq!(def.resources["mana"].max, "max_mana");
        assert_eq!(def.resources["mana"].regen_per_sec, "mana_regen");

        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cast_time, "1.0 / base_aps");
        // P7b: these four positions became `NumOrExpr`. Untagged serde
        // must keep reading the spec's plain JSON NUMBERS as `Num` — this
        // is the 0.2.0 backward-compatibility contract at the parse layer
        // (the behavioral half is pinned in `sim::exec`'s tests).
        assert_eq!(fireball.cooldown, NumOrExpr::Num(0.0));
        assert_eq!(fireball.cost["mana"], NumOrExpr::Num(40.0));
        assert!(fireball.gain.is_empty());
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], NumOrExpr::Num(200.0));
        assert_eq!(dmg.stats["hits_per_use"], NumOrExpr::Num(1.0));

        assert_eq!(def.buffs["vuln_window"].duration, NumOrExpr::Num(4.0));
        assert_eq!(def.buffs["vuln_window"].conditions["vulnerable"], 1.0);
        assert_eq!(def.buffs["combustion"].contributions.len(), 1);
        assert_eq!(def.buffs["combustion"].contributions[0].bucket, "indep");
        assert_eq!(def.buffs["combustion"].contributions[0].value, 25.0);
        // P7c-T2: `tick_objective` became an object, and the spec's BARE
        // STRING must still mean exactly what it meant in 0.2.0 — the
        // live rate, never a snapshot.
        assert_eq!(
            def.buffs["burning"].tick_objective,
            Some(TickObjective::live("dot_dps"))
        );

        let proc = &def.procs["conflagrate"];
        assert_eq!(proc.trigger, Trigger::OnCrit);
        assert_eq!(proc.chance, "lucky_hit_chance / 100 * 0.3");
        assert_eq!(proc.icd, 2.0);
        assert_eq!(proc.apply_buff.as_deref(), Some("combustion"));
        assert_eq!(proc.cast_action, None);

        assert_eq!(def.damage_objective, "hit_after_dr");

        // Round-trip through serde again (idempotence).
        let reparsed: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(reparsed.damage_objective, def.damage_objective);
    }

    // P7b: the same five positions, written as STRINGS this time — the
    // untagged enum's other arm. Both arms round-trip, so a config may mix
    // them freely (`"cost": { "mana": 40.0, "rage": "10 + rage_cost" }`).
    const EXPR_SIMDEF_JSON: &str = r#"{
      "actions": {
        "fireball": {
          "cast_time": "1.0 / base_aps",
          "cooldown": "5 + 5",
          "cost": { "mana": "20 + 10" },
          "gain": { "mana": 40.0 },
          "damage": { "stats": { "coeff_pct": "200 * 2", "hits_per_use": 1.0 } }
        }
      },
      "buffs": { "vuln_window": { "duration": "2 + bonus_dur" } },
      "damage_objective": "hit_after_dr"
    }"#;

    #[test]
    fn expression_valued_fields_deserialize_and_round_trip() {
        let def: SimDef = serde_json::from_str(EXPR_SIMDEF_JSON).unwrap();
        let fireball = &def.actions["fireball"];
        assert_eq!(fireball.cooldown, NumOrExpr::Expr("5 + 5".into()));
        assert_eq!(fireball.cost["mana"], NumOrExpr::Expr("20 + 10".into()));
        assert_eq!(fireball.gain["mana"], NumOrExpr::Num(40.0));
        let dmg = fireball.damage.as_ref().unwrap();
        assert_eq!(dmg.stats["coeff_pct"], NumOrExpr::Expr("200 * 2".into()));
        assert_eq!(dmg.stats["hits_per_use"], NumOrExpr::Num(1.0));
        assert_eq!(
            def.buffs["vuln_window"].duration,
            NumOrExpr::Expr("2 + bonus_dur".into())
        );

        // Serializing puts each arm back in its own JSON shape (number
        // stays a number, expression stays a string) — round-tripping a
        // config never rewrites a literal as `"40"`.
        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(round.actions["fireball"].cooldown, fireball.cooldown);
        assert_eq!(round.actions["fireball"].gain["mana"], NumOrExpr::Num(40.0));
    }

    // P7c backward-compatibility contract at the parse layer: a 0.2.0
    // config names neither stack field, and must come out as exactly the
    // binary buff — ONE instance, refreshed on reapplication.
    #[test]
    fn omitted_stack_fields_default_to_one_instance_and_refresh() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();
        for name in ["vuln_window", "combustion", "burning"] {
            assert_eq!(def.buffs[name].max_stacks, 1, "{name}");
            assert_eq!(def.buffs[name].on_reapply, ReapplyPolicy::Refresh, "{name}");
        }
    }

    // STRUCTURAL guard, not a per-field one: serde's `#[serde(default)]`
    // attributes and the hand-written `impl Default` are two independent
    // spellings of "what a config that says nothing means", and nothing
    // in the language ties them together. A struct literal catches a
    // MISSING field; only this catches a field whose two defaults
    // DISAGREE — which would silently split Rust-constructed configs from
    // JSON-parsed ones, exactly the bug `max_stacks` was hand-defaulted
    // to avoid (`#[serde(default)]` on a `u32` is `0`, which this field
    // spells "unbounded"). Add every new field to BOTH.
    #[test]
    fn serde_defaults_and_impl_default_agree_field_for_field() {
        let bare: BuffDef = serde_json::from_str(r#"{ "duration": 0.0 }"#).unwrap();
        assert_eq!(
            bare,
            BuffDef::default(),
            "serde's per-field defaults must agree with `impl Default` — \
             a new BuffDef field belongs in BOTH"
        );
    }

    #[test]
    fn stack_fields_parse_and_round_trip_in_snake_case() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": {
                "charge":  { "duration": 5.0, "max_stacks": 3,
                             "on_reapply": "add_refresh_all" },
                "poison":  { "duration": 8.0, "max_stacks": 0,
                             "on_reapply": "add_independent" },
                "ignite":  { "duration": 4.0, "on_reapply": "strongest" }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(def.buffs["charge"].max_stacks, 3);
        assert_eq!(def.buffs["charge"].on_reapply, ReapplyPolicy::AddRefreshAll);
        assert_eq!(def.buffs["poison"].max_stacks, 0); // 0 = unbounded
        assert_eq!(
            def.buffs["poison"].on_reapply,
            ReapplyPolicy::AddIndependent
        );
        // `strongest` PARSES — the vocabulary is complete; `sim::compile`
        // is where it is honestly rejected (see that module's tests).
        assert_eq!(def.buffs["ignite"].on_reapply, ReapplyPolicy::Strongest);
        assert_eq!(def.buffs["ignite"].max_stacks, 1);

        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(
            round.buffs["charge"].on_reapply,
            ReapplyPolicy::AddRefreshAll
        );
        assert_eq!(round.buffs["poison"].max_stacks, 0);
    }

    // P7c-T2: the two `tick_objective` shapes. The bare string is the
    // 0.2.0 form and MUST stay live; the object form is the only way to
    // ask for snapshot semantics, and `snapshot` defaults to `false`
    // there too, so the two spellings of "live" agree.
    #[test]
    fn tick_objective_parses_both_the_bare_name_and_the_object_form() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": {
                "burning": { "duration": 6.0, "tick_objective": "dot_dps" },
                "poison":  { "duration": 8.0,
                             "tick_objective": { "objective": "dot_dps",
                                                 "snapshot": true } },
                "bleed":   { "duration": 5.0,
                             "tick_objective": { "objective": "dot_dps" } }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(
            def.buffs["burning"].tick_objective,
            Some(TickObjective::live("dot_dps"))
        );
        assert_eq!(
            def.buffs["poison"].tick_objective,
            Some(TickObjective::snapshot("dot_dps"))
        );
        assert_eq!(
            def.buffs["bleed"].tick_objective,
            Some(TickObjective::live("dot_dps")),
            "`snapshot` omitted inside the object form must default to false"
        );

        // Round-trip: a LIVE objective goes back out as the bare string it
        // came from (a 0.2.0 config is never rewritten into the object
        // form), and a snapshot one keeps its object.
        let json = serde_json::to_string(&def).unwrap();
        assert!(
            json.contains(r#""tick_objective":"dot_dps""#),
            "a live tick_objective must serialize back to the bare string: {json}"
        );
        let round: SimDef = serde_json::from_str(&json).unwrap();
        assert_eq!(
            round.buffs["poison"].tick_objective,
            def.buffs["poison"].tick_objective
        );
        assert_eq!(
            round.buffs["bleed"].tick_objective,
            def.buffs["bleed"].tick_objective
        );
    }

    // Fail-closed: `"snapshots": true` is a typo, not a config that means
    // "live". Silently ignoring the key would silently give the WRONG DoT
    // semantics — the exact class of quiet wrong answer this crate exists
    // to refuse. P8a: the hand-written map visitor names the key and both
    // valid fields, instead of serde's "did not match any variant".
    #[test]
    fn an_unknown_key_inside_the_tick_objective_object_is_rejected() {
        let e = serde_json::from_str::<SimDef>(
            r#"{
              "buffs": { "poison": { "duration": 8.0,
                                     "tick_objective": { "objective": "dot_dps",
                                                         "snapshots": true } } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("unknown field `snapshots`, expected `objective` or `snapshot`"),
            "got: {e}"
        );
    }

    // …but the `_` annotation namespace is open INSIDE the object form
    // too, like at every other nesting level (P8a).
    #[test]
    fn an_underscore_key_inside_the_tick_objective_object_is_accepted() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "buffs": { "poison": { "duration": 8.0,
                                     "tick_objective": { "objective": "dot_dps",
                                                         "snapshot": true,
                                                         "_why": "PoE2 ailment" } } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(
            def.buffs["poison"].tick_objective,
            Some(TickObjective::snapshot("dot_dps"))
        );
    }

    // P8a: a malformed `NumOrExpr` names what was expected instead of
    // serde's "data did not match any variant of untagged enum NumOrExpr".
    #[test]
    fn a_malformed_num_or_expr_names_what_was_expected() {
        let e = serde_json::from_str::<SimDef>(
            r#"{ "actions": { "a": { "cast_time": "1", "cooldown": true } },
                 "damage_objective": "hit" }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("expected a number (literal) or a string (expression)"),
            "got: {e}"
        );
    }

    // P7d backward-compatibility contract at the parse layer: a 0.2.0
    // config names neither new field, and must come out as exactly the
    // 0.2.0 behavior — an action that applies nothing, and a proc that
    // considers EVERY action.
    #[test]
    fn omitted_action_scoping_fields_default_to_the_020_behavior() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();
        assert!(
            def.actions["fireball"].apply_buff.is_empty(),
            "an action that names no apply_buff applies nothing"
        );
        assert_eq!(
            def.procs["conflagrate"].actions, None,
            "a proc that names no `actions` filter considers every action"
        );
    }

    #[test]
    fn action_scoping_fields_parse_and_round_trip() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "actions": {
                "nova": { "cast_time": "0", "cooldown": 10.0,
                          "apply_buff": ["vuln_window", "chill"] },
                "bolt": { "cast_time": "1" }
              },
              "buffs": { "vuln_window": { "duration": 4.0 },
                         "chill":       { "duration": 2.0 } },
              "procs": {
                "scoped": { "trigger": "on_hit", "chance": "1",
                            "apply_buff": "chill", "actions": ["bolt"] },
                "global": { "trigger": "on_hit", "chance": "1",
                            "apply_buff": "chill" }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        // Source ORDER is preserved (it is the application order — see
        // `ActionDef::apply_buff`), so this is a Vec comparison, not a set
        // one.
        assert_eq!(def.actions["nova"].apply_buff, ["vuln_window", "chill"]);
        assert!(def.actions["bolt"].apply_buff.is_empty());
        assert_eq!(
            def.procs["scoped"].actions.as_deref(),
            Some(["bolt".to_string()].as_slice())
        );
        assert_eq!(def.procs["global"].actions, None);

        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(
            round.actions["nova"].apply_buff,
            def.actions["nova"].apply_buff
        );
        assert_eq!(round.procs["scoped"].actions, def.procs["scoped"].actions);
        assert_eq!(round.procs["global"].actions, None);
    }

    // The same STRUCTURAL guard `BuffDef` carries, for the type P7d gave a
    // new field to. `ActionDef` derives `Default`, so its two spellings of
    // "what a config that says nothing means" are serde's `#[serde(default)]`
    // attributes and that derive — independent, and nothing in the language
    // ties them together. Add every new field to BOTH. (`ProcDef` has no
    // `Default` at all — `trigger`/`chance` are required — so there is no
    // second spelling of its defaults to disagree with.)
    #[test]
    fn action_def_serde_defaults_and_derived_default_agree_field_for_field() {
        let bare: ActionDef = serde_json::from_str(r#"{ "cast_time": "" }"#).unwrap();
        let derived = ActionDef::default();
        assert_eq!(bare.cast_time, derived.cast_time);
        assert_eq!(bare.cooldown, derived.cooldown);
        assert_eq!(bare.cost, derived.cost);
        assert_eq!(bare.gain, derived.gain);
        assert!(bare.damage.is_none() && derived.damage.is_none());
        assert_eq!(
            bare.measure, derived.measure,
            "a config that names no `measure` defers to the defaults \
             block (P8c) in both spellings of the default"
        );
        assert!(
            bare.measure.is_none(),
            "the deferred spelling is None, not a baked-in Measure value \
             — resolution against `defaults.measure` happens at sim::compile"
        );
        assert_eq!(
            bare.apply_buff, derived.apply_buff,
            "serde's per-field defaults must agree with the derived \
             `Default` — a new ActionDef field belongs in BOTH"
        );
        assert_eq!(
            bare.effects, derived.effects,
            "a config that names no `effects` means the empty list (P8b) \
             in both spellings of the default"
        );
        assert_eq!(
            bare.extra, derived.extra,
            "a config that says nothing collects nothing (P8a) — both \
             spellings of the default must be the empty map"
        );
    }

    #[test]
    fn omitted_cooldown_defaults_to_zero() {
        let def: SimDef = serde_json::from_str(
            r#"{ "actions": { "a": { "cast_time": "1" } }, "damage_objective": "hit" }"#,
        )
        .unwrap();
        assert_eq!(def.actions["a"].cooldown, NumOrExpr::Num(0.0));
        assert!(def.actions["a"].cost.is_empty());
    }

    // ==================================================================
    // P8b — the effects list at the parse layer. The JSON shape is the
    // design spec's, verbatim: externally tagged entries, list order
    // preserved, a repeated entry kept (repeats apply twice — the
    // executor half is pinned in `sim::exec`'s `effects_list` module).
    // ==================================================================
    #[test]
    fn effects_lists_parse_and_round_trip_the_spec_json() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "actions": {
                "frost_nova": { "cast_time": "0",
                                "effects": [ { "apply_buff": "vuln_window" } ] },
                "comet":      { "cast_time": "1" }
              },
              "buffs": { "vuln_window": { "duration": 4.0 },
                         "shock":       { "duration": 2.0 } },
              "procs": {
                "trigger_gem": { "trigger": "on_cast", "chance": "1", "icd": 3.0,
                                 "effects": [ { "apply_buff": "shock" },
                                              { "cast_action": "comet" },
                                              { "apply_buff": "shock" } ] }
              },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(
            def.procs["trigger_gem"].effects,
            vec![
                EffectDef::ApplyBuff("shock".into()),
                EffectDef::CastAction("comet".into()),
                EffectDef::ApplyBuff("shock".into()),
            ],
            "list order and the repeat are the config's own — preserved verbatim"
        );
        assert_eq!(
            def.actions["frost_nova"].effects,
            vec![EffectDef::ApplyBuff("vuln_window".into())]
        );
        assert!(def.actions["comet"].effects.is_empty());
        assert_eq!(def.procs["trigger_gem"].apply_buff, None);
        assert_eq!(def.procs["trigger_gem"].cast_action, None);

        // Round-trip: the externally-tagged shape serializes back as the
        // same single-key objects, and reparses to the same lists.
        let json = serde_json::to_string(&def).unwrap();
        assert!(
            json.contains(r#"{"apply_buff":"shock"}"#),
            "externally tagged, snake_case: {json}"
        );
        assert!(
            json.contains(r#"{"cast_action":"comet"}"#),
            "externally tagged, snake_case: {json}"
        );
        let round: SimDef = serde_json::from_str(&json).unwrap();
        assert_eq!(
            round.procs["trigger_gem"].effects,
            def.procs["trigger_gem"].effects
        );
        assert_eq!(
            round.actions["frost_nova"].effects,
            def.actions["frost_nova"].effects
        );
        // An EMPTY list does not serialize at all (skip-serializing-if),
        // so round-tripping a pre-P8b config never grows an `effects` key.
        assert!(
            !json.contains(r#""comet":{"cast_time":"1","effects""#),
            "an empty effects list must not appear in the output: {json}"
        );
        let sugar: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();
        assert!(
            !serde_json::to_string(&sugar).unwrap().contains("effects"),
            "a config that never wrote `effects` round-trips without it"
        );
    }

    // Fail-closed INSIDE an effect entry (P8a discipline): a typo'd key
    // must not be serde's unhelpful default. Before the hand-written
    // visitor, serde's derived externally-tagged enum said "unknown
    // variant `apply_buf`, expected `apply_buff` or `cast_action`" —
    // serviceable, but off-vocabulary ("variant") and without the shared
    // did-you-mean; this pins the P8a wording instead.
    #[test]
    fn a_typoed_key_inside_an_effect_entry_is_rejected_with_a_did_you_mean() {
        let e = serde_json::from_str::<SimDef>(
            r#"{
              "procs": { "lucky": { "trigger": "on_cast", "chance": "1",
                                    "effects": [ { "apply_buf": "x" } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("unknown field `apply_buf`"),
            "got: {e}"
        );
        assert!(e.to_string().contains("an effect entry"), "got: {e}");
        assert!(
            e.to_string().contains("did you mean `apply_buff`"),
            "got: {e}"
        );
    }

    // …and the `_` annotation namespace stays open inside an effect
    // entry, like at every other nesting level (P8a).
    #[test]
    fn an_underscore_key_inside_an_effect_entry_is_accepted() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "procs": { "lucky": { "trigger": "on_cast", "chance": "1",
                                    "effects": [ { "apply_buff": "x",
                                                   "_src": "aspect" } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap();
        assert_eq!(
            def.procs["lucky"].effects,
            vec![EffectDef::ApplyBuff("x".into())]
        );
    }

    // An effect entry is exactly one effect: an empty object (or one
    // holding only annotations) and a two-effect object are both rejected
    // with the expectation spelled out.
    #[test]
    fn an_effect_entry_takes_exactly_one_effect_key() {
        let e = serde_json::from_str::<SimDef>(
            r#"{
              "procs": { "lucky": { "trigger": "on_cast", "chance": "1",
                                    "effects": [ { "_note": "oops" } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("an effect entry needs exactly one of `apply_buff` or `cast_action`"),
            "got: {e}"
        );
        let e = serde_json::from_str::<SimDef>(
            r#"{
              "procs": { "lucky": { "trigger": "on_cast", "chance": "1",
                                    "effects": [ { "apply_buff": "x",
                                                   "cast_action": "y" } ] } },
              "damage_objective": "hit_after_dr"
            }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("an effect entry takes exactly one of `apply_buff` or `cast_action`, got more than one"),
            "got: {e}"
        );
    }

    // ==================================================================
    // P8c — the `defaults` block and `measure` at the parse layer.
    // ==================================================================

    // Backward-compatibility contract at the parse layer: a config that
    // never wrote `defaults` (every 0.2.0/0.3.0 config) gets every knob
    // at its 0.3.0-behavior value, and round-trips WITHOUT the block.
    #[test]
    fn an_omitted_defaults_block_means_cast_complete_and_never_reappears() {
        let def: SimDef = serde_json::from_str(P6_SPEC_SIMDEF_JSON).unwrap();
        assert_eq!(def.defaults.measure, Measure::CastComplete);
        assert_eq!(def.defaults, SimDefaults::default());
        assert_eq!(def.actions["fireball"].measure, None);
        let json = serde_json::to_string(&def).unwrap();
        assert!(
            !json.contains("defaults") && !json.contains("measure"),
            "a config that never wrote the block round-trips without it: {json}"
        );
    }

    #[test]
    fn the_defaults_block_and_per_action_measure_parse_and_round_trip() {
        let def: SimDef = serde_json::from_str(
            r#"{
              "defaults": { "measure": "cast_start" },
              "actions": {
                "bolt": { "cast_time": "1", "measure": "cast_complete" },
                "beam": { "cast_time": "1" }
              },
              "damage_objective": "hit"
            }"#,
        )
        .unwrap();
        assert_eq!(def.defaults.measure, Measure::CastStart);
        assert_eq!(def.actions["bolt"].measure, Some(Measure::CastComplete));
        assert_eq!(def.actions["beam"].measure, None, "deferral is None");

        let round: SimDef = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
        assert_eq!(round.defaults, def.defaults);
        assert_eq!(round.actions["bolt"].measure, def.actions["bolt"].measure);
        assert_eq!(round.actions["beam"].measure, None);
    }

    // A `defaults` block holding ONLY `_` annotations is not vacuous —
    // annotations must survive a load-and-save, like at every other
    // nesting level (P8a).
    #[test]
    fn a_defaults_block_holding_only_annotations_survives_round_trips() {
        let def: SimDef = serde_json::from_str(
            r#"{ "defaults": { "_why": "cast-start package" },
                 "damage_objective": "hit" }"#,
        )
        .unwrap();
        assert_eq!(def.defaults.measure, Measure::CastComplete);
        let json = serde_json::to_string(&def).unwrap();
        assert!(
            json.contains(r#""_why":"cast-start package""#),
            "annotations survive: {json}"
        );
    }

    // A malformed `measure` VALUE is a parse error naming the variants —
    // serde's derived enum message, the same voice `on_reapply` and
    // `trigger` already speak (an unknown KEY inside the block is the
    // sim::compile walk's job instead — see `tests/unknown_keys.rs`).
    #[test]
    fn a_malformed_measure_value_names_the_two_variants() {
        let e = serde_json::from_str::<SimDef>(
            r#"{ "defaults": { "measure": "cast_startt" },
                 "damage_objective": "hit" }"#,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("expected `cast_complete` or `cast_start`"),
            "got: {e}"
        );
    }

    #[test]
    fn rotation_round_trips_the_spec_example_verbatim() {
        let r: Rotation = serde_json::from_str(ROTATION_JSON).unwrap();
        assert_eq!(r.rules.len(), 3);
        assert_eq!(r.rules[0].action, "frost_nova");
        assert_eq!(
            r.rules[0].when.as_deref(),
            Some("cooldown.frost_nova == 0 and buff.vuln_window == 0")
        );
        assert_eq!(r.rules[1].action, "fireball");
        assert_eq!(r.rules[1].when.as_deref(), Some("mana >= 40"));
        assert_eq!(r.rules[2].action, "basic_bolt");
        assert_eq!(r.rules[2].when, None);
    }
}
