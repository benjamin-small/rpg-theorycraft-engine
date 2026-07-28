# P8 — Configurable Semantics + One-World Measurement (2026-07-26)

**North star:** a generic engine should not hard-code the semantic choices
real games make differently — *when a cast measures its world*, *how procs
roll against multi-hit casts*, *which of two coincident events resolves
first*. P8 makes each of these configuration: a SimDef-wide default plus
per-entity overrides, with every serde default reproducing 0.3.0
byte-for-byte. Released as **rtce 0.4.0**.

Decisions locked with the user (2026-07-26):
- **Scope**: the discussed charter (measurement instant, proc rolling,
  effects list, fail-closed config sweep, docs discipline) **plus** the
  validation/API debt group **plus** the coverage debt group. The
  ask-driven items stay deferred (per-stack `product` fold, `ProcDef`
  `actions` negation) — per the ROADMAP's own "only if a real config
  asks" discipline.
- **Tie-break configured too** (not subsumed by measurement): the event
  ordering gets its own knob, requiring the second ordering key on the
  queue.
- **One world — fix the default**: an `apply_buff` capture reads a frozen
  build × frozen phase. This is the phase's ONE deliberate behavior
  change, CHANGELOG'd with a migration note exactly like the P7
  horizon-drain fix. (Rejected: preserving frozen-build × live-phase as
  the default — it would carry the release review's "arguably wrong"
  semantics forward as the engine's permanent default.)
- **Configurability shape — Approach A**: a `defaults` block with named
  knobs + per-entity overrides. (Rejected: B, named semantic profiles —
  a profile is just a saved defaults block, and shipping opinions about
  other games' semantics is what rtce refuses to do; C, per-entity only —
  verbose, and the user explicitly wants package-wide defaults.)
- **`deny_unknown_fields` is a hard stop, not a warning** — rtce's
  consumers are search loops pricing thousands of configs; nobody reads
  warnings in a loop, so a warning is a silent wrong answer with extra
  steps. But NOT at the serde layer (see P8a).
- **Docs discipline is a deliverable, not an intention** (see P8f).

Standing invariant: **every serde default reproduces 0.3.0 behavior
byte-for-byte.** All existing pins stay green except the ONE re-derived
on purpose (the 400-vs-800 reorder pin, which becomes an equality pin).
Both consumers (`../diablo4-calc` solely on rtce; `../poe2-calcs` in
parity) stay numerically untouched. `diablo4_rotation`'s EV **and** MC
blocks stay byte-identical under defaults — the MC block is the standing
evidence no RNG draw is reordered.

## The `defaults` block (config surface)

```jsonc
{
  "defaults": {                       // NEW, all fields optional
    "measure": "cast_complete",       // | "cast_start"        (P8c)
    "proc_rolls": "per_cast",         // | "per_hit"           (P8e)
    "event_order": "scheduled"        // | "completions_first" (P8d)
  },
  "actions": {
    "fireball": { "measure": "cast_start", ... }   // per-action override
  },
  "procs": {
    "lucky": { "rolls": "per_hit", ... }           // per-proc override
  }
}
```

- Omitted `defaults` (every 0.2.0/0.3.0 config) = all three at their
  0.3.0-behavior values.
- `event_order` is **SimDef-global only** — no per-entity override.
  Ordering is a property of the queue; a collision involves two entities,
  so a per-spell tie-break is incoherent.
- Every knob is a small named enum; every knob is documented and pinned
  independently (see P8f's discipline).

## P8a — fail-closed config sweep + validation debt (lands FIRST)

Everything later in the phase adds config fields; they must all get this
treatment, so it leads.

- **Unknown-key rejection at compile/validation time, not serde time.**
  Every config struct (`GameDef`, `BucketDef`, `EventDef`, `StageDef`,
  `BuildState`, `Contribution`, `Scenario`, `Phase`, `SimDef`,
  `ResourceDef`, `ActionDef`, `ActionDamage`, `BuffDef`, `ProcDef`,
  `Rotation`, `Rule`, the new `defaults` block) gains unknown-key
  collection (`#[serde(flatten)] extra: BTreeMap<String, Value>` or a
  hand-written equivalent). `compile`/`Plan::compile` validation then
  fails closed on any collected key that does not start with `_`, with a
  positioned error naming the key, its location, and the nearest valid
  field ("unknown field `tick_objectiv` on buff `poison` — did you mean
  `tick_objective`?").
- **The `_` prefix is the documented annotation namespace** — our own
  committed gamedefs carry `_source`/`_scope`/`_shape`/`_crit`, which is
  why a blanket serde-level `deny_unknown_fields` is wrong: it would
  reject rtce's own fixtures. Allowed at every nesting level.
- **Hand-written `Deserialize` for the untagged enums** (`NumOrExpr`,
  the `TickObjective` repr, P8b's `EffectDef`) so a malformed value
  reports what was expected and which field is wrong, instead of serde's
  "data did not match any variant of untagged enum".
- **Validation debt**: `Contribution::value` must be finite — enforced on
  BOTH halves of the shared type (Level-1: `Plan`'s build resolution;
  sim: `sim::compile` for `BuffDef::contributions`). Known repro from the
  0.3.0 release review: `NaN` → `Ok(NaN)` total damage, `inf` →
  `Ok(inf)`, silently. Plus a sweep for remaining bare-`f64` gaps
  (e.g. `Phase::stats` override values) — each either validated or
  documented-and-pinned as deliberately unvalidated.
- Compatibility note: this REJECTS configs that parse today (typo'd
  keys). That is the point. CHANGELOG migration note; both consumers'
  committed configs must pass unmodified.

## P8b — the effects list

```jsonc
"procs": {
  "trigger_gem": {
    "trigger": "on_cast", "chance": "1", "icd": 3.0,
    "effects": [
      { "apply_buff": "shock" },
      { "cast_action": "comet" },
      { "apply_buff": "shock" }     // repeats apply twice (P7d precedent)
    ]
  }
},
"actions": {
  "frost_nova": { "effects": [ { "apply_buff": "vuln_window" } ], ... }
}
```

- `EffectDef` — externally-tagged serde enum, `rename_all = "snake_case"`:
  `ApplyBuff(String)` | `CastAction(String)`. The JSON shape falls out of
  the type.
- `ProcDef.effects: Vec<EffectDef>` (compile error if empty and no sugar
  present — a proc must do something). `ActionDef.effects:
  Vec<EffectDef>` (empty fine).
- **Execution order = list order** (matches P7d's `apply_buff` list
  precedent). Proc effects run at fire, in order; action effects run in
  the cast-complete transaction where `apply_buff` runs today.
- **Old fields become compile-time sugar**: `ProcDef.apply_buff`,
  `ProcDef.cast_action`, `ActionDef.apply_buff` parse unchanged and
  desugar into `effects` at `sim::compile` (deprecated in rustdoc, kept
  for 0.x). Mixing sugar with an explicit `effects` list on the SAME
  entity is a compile error — the merged order would be ambiguous.
- **`cast_action` stays proc-only.** On an `ActionDef` it is a positioned
  compile error explaining why: an action free-casting an action reopens
  the recursion the free-cast guard closed (A→B→A), and a bounded-depth
  design should be chosen by a config that needs chains, not guessed at.
  ROADMAP entry.
- Free casts execute their action's `ApplyBuff` effects (P7d behavior,
  unchanged) — and, having no `CastAction` effects by the rule above,
  still cannot recurse.

## P8c — WorldSnapshot measurement + the one-world fix

- `defaults.measure`: `cast_complete` (default — today's instant) |
  `cast_start`. `ActionDef.measure` overrides per action. Instant casts:
  the two are identical by construction.
- A **`WorldSnapshot { build, phase }`** is captured at the configured
  instant (in `begin_cast` for `cast_start`; in the completion
  transaction for `cast_complete`) and carried on the in-flight cast.
  **Every `Plan` evaluation in that cast's completion transaction reads
  the one snapshot**: damage, `hits_per_use`, crit chance / EV `on_crit`
  weight, and the tick-objective captures of every `apply_buff` effect in
  the list.
- **The one-world fix (the phase's single deliberate behavior change):**
  today an `apply_buff` capture reads a frozen build × LIVE phase, so
  `["mark", "poison"]` vs `["poison", "mark"]` doubles a DoT at identical
  reported uptime (pinned 400 vs 800 in
  `a_same_list_snapshot_capture_reads_a_frozen_build_but_a_live_phase`).
  Under P8c the capture reads the snapshot's phase too. The pin is
  re-derived to an **equality** pin: both orderings produce the same
  number, and the equality itself is the assertion. CHANGELOG'd with a
  migration note naming exactly which configs move (a condition-driving
  buff and a snapshot DoT applied in one list). Neither consumer's
  numbers move (verified in the plan: d4's rotation has no
  condition-reading snapshot DoT; the poe2 slices' buff lists don't
  drive conditions read by their DoTs).
- **Scope boundary, stated precisely:** sim-FIELD expressions keep their
  P7b instants and semantics unchanged — `duration` still reads LIVE
  state at the application instant (the pandemic idiom and the 0.75 pin
  are untouched), `cost`/`gain` at their documented instants, and the
  sim symbol tail (`stacks.*`, `buff_remaining.*`, resources) stays
  sequential within the list. "One world" governs `Plan` evaluations
  (damage and captures), not sim-state reads.
- Teaching payoff: `poe2_triggers` gains a contrast run — at the
  integer duration where the bolt loses its own shock (2175 → 1837.5,
  pinned in 0.3.0), `measure: "cast_start"` restores 2175. The footgun
  section in `sim`'s docs gains "and here is the config that fixes it".

## P8d — configurable event ordering

- `defaults.event_order`: `scheduled` (default) | `completions_first`.
  SimDef-global only.
- `scheduled` is the honest name for today's behavior: coincident events
  resolve in scheduling (`seq`) order. "Expiry first" was always
  incidental — an expiry usually holds the lower `seq` because it was
  scheduled at the application, earlier than the completion.
- `completions_first`: every `CastComplete` outranks every coincident
  `BuffExpire` / `PhaseBoundary` / `Wake`; within a class, `seq` still
  decides. Consequences to document and pin: a cast completing exactly
  when its buff lapses now measures WITH the buff (the cast-grid footgun
  becomes config-fixable at the ordering level, complementing P8c's
  measurement-level fix); the zero-weight-final-phase cast measures under
  the OLD phase (the 0.3.0 pin documented that attribution as an
  incidental consequence of `seq` — under `completions_first` it flips,
  and the flipped cell gets its own pin, stated as designed).
- Implementation: `QueueItem` ordering becomes `(time, class_rank, seq)`.
  `class_rank` is a constant function under `scheduled` (bit-identical
  ordering, proven by the byte-identical MC block), and derived from the
  configured order otherwise. `seq` always breaks residual ties, so
  seeded MC determinism holds under every setting. This is the mechanism
  the P6 notes declined for `End` — now built deliberately, behind a
  config default that makes it a no-op.
- The horizon-drain semantics (P7e-T2) are unchanged: ordering says which
  event at `t == duration` resolves first, never whether it resolves.

## P8e — configurable proc rolling

- `defaults.proc_rolls`: `per_cast` (default — today: one roll per
  damaging cast, `hits_per_use`-blind) | `per_hit`. `ProcDef.rolls`
  overrides per proc — the override lives on the PROC because rolling is
  the proc's semantics; the hit count is already the action's.
- `per_hit` semantics: the hit count comes from the measurement snapshot.
  EV — the accumulator is fed once per hit (`acc += chance` × the
  `on_crit` weight where applicable), multiple crossings can fire, the
  ICD gates between fires. MC — one Bernoulli draw per hit, ICD gate
  between draws. RNG draw count changes ONLY under non-default config
  (the default's MC block stays byte-identical).
- **The ICD-at-one-instant rule, stated and pinned, not discovered:** all
  hits of one cast land at the same instant in the current model, so any
  `icd > 0` caps fires at one per cast even under `per_hit`; `icd: 0`
  permits multiple fires per cast. A pin discriminates `per_cast` from
  `per_hit` in BOTH modes at a fractional chance (the P7 lesson: a
  chance-1 pin is vacuous for accumulator semantics).

## P8f — coverage, docs discipline, release 0.4.0

- **Coverage debt closed**: behavioral tests for `refresh` + a LIVE
  `tick_objective` (the 0.2.0 default DoT shape — currently zero
  coverage) and for live ticks under Monte Carlo; a `strongest`/ignite
  example slice (small, `poe2_ignite.rs` or a section in an existing
  slice — plan's choice) so the README's "no strongest example" caveat
  retires.
- **API debt**: `SimScratch` REMOVED (dead public API — constructible,
  re-exported, accepted by nothing; breaking-but-0.x, CHANGELOG'd).
  `expr::MAX_STACK` reachability documented honestly.
- **The docs discipline, codified as standing rules** (in the repo's
  CLAUDE.md working conventions, applied in this phase and binding on
  future ones):
  1. Every config field's rustdoc states its **default, its evaluation
     instant, and its interactions** with other fields.
  2. Every doc claim carrying a NUMBER ships with a contrast-run pin.
  3. Every `(default × override)` cell that ships gets a discriminating
     test — configurability multiplies the semantics matrix, and the
     P7 record (a surviving mutation on documented-but-unpinned semantics
     in five consecutive tasks) is the named risk this rule answers.
- CHANGELOG 0.4.0: an "Upgrading from 0.3.0" section with the one-world
  behavior change, the unknown-key rejection, the `SimScratch` removal,
  and the Rust source-breaking notes (new fields). README/lib.rs/crate
  README updated together (the P7e-T3 lesson: the crates.io front page is
  a deliverable, not an afterthought). `cargo doc` zero warnings.
- **Final gate**: standing-reviewer round on the whole P8 diff → fixes →
  APPROVED → publish rtce 0.4.0 → push.

## Testing discipline (unchanged house style, plus the named risk)

TDD red-first; every pinned number hand-worked in a comment; every
behavior change mutation-proven (break it, watch the specific test fail,
restore, report the contrast); fail-closed with positioned errors;
clippy `-D warnings` + fmt + `missing_docs` + `publish --dry-run` clean
per task; both consumers re-verified per task (diablo4-calc
`8096.023984663315`; poe2-calcs 63 `rtce_parity`).

## Out of scope (P8)

- Per-stack `product` fold mode (×1.331 charges) and `ProcDef::actions`
  negation/expressiveness — deferred until a real config asks.
- `cast_action` on actions (combo chains) — needs a bounded-recursion
  design chosen by a driving config; ROADMAP.
- The min/max-expiry cache — still has no witness; needs a `benches/`
  harness first.
- poe2-calcs switchover; multi-target; damage-roll distributions — the
  standing larger items, unchanged.
