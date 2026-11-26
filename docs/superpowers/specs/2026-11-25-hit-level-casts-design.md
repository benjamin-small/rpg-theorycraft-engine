# Hit-level casts: segments, cancels, interruptions (and issue #33)

Status: DRAFT for approval · 2026-11-25 · target release `rtce` 0.8.0

## 1. Intent

`sim` today is a discrete-event timeline whose smallest unit is the
CAST: every hit of a cast lands at one `CastComplete` instant, crit is a
branch inside one `Plan` evaluation (Monte Carlo samples one mask per
cast), and nothing can happen between a cast's start and its end. Two
needs collide with that:

- **Derived "Recently" conditions** ([#33]). poe2-theory-crafting wants
  conditions like `crit_recently` / `non_crit_recently` to come from the
  simulated hit stream instead of being asserted. #33 lists five engine
  limits that block it.
- **Multi-strike, cancellable skills.** PoE2's Rolling Slam is two
  strikes with different numbers; the player may cancel after the
  first. Channels, enemy interrupts, stuns and forced movement are the
  same shape. None of these can be modelled honestly while a cast is
  atomic.

The decision (2026-11-25 conversation): make the engine hit-level now,
with SimulationCraft as the prior art. Player cancels come first,
outside interruptions second. #33's #3 (per-hit crit) and #2
(cadence from the build) fold into this work; #5 and #1 ship first as
independent slices.

[#33]: https://github.com/benjamin-small/rpg-theorycraft-engine/issues/33

### Prior art: SimulationCraft

- Cancels live on the ACTION-LIST LINE, checked at TICK boundaries:
  `interrupt=1` cancels a channel when a higher-priority action becomes
  available; `interrupt_if=<expr>` adds a condition (and requires the GCD
  to have elapsed). What it cancels INTO is whatever the priority list
  picks. Rules read the channel's position via `ticks_remain`,
  `tick_time`, `channeling`.
- Outside interruptions are fight-level `raid_events` (`movement`
  interrupts casts; `stun` locks the actor), periodic via `first`,
  `last`, `cooldown`, `duration`, `timestamps`, `player_chance`, with
  `cooldown_stddev` / `duration_stddev` jitter.
- SimC is Monte-Carlo-only. rtce also has `Mode::Expected`, which is
  where this design diverges (§5.6).

## 2. Goals, non-goals, anti-goals

**Goals**

1. A cast may be a timed sequence of SEGMENTS (strikes), each landing
   its own hits with its own damage overlay at its own instant, each a
   real queue event.
2. The player may cancel a cast at a segment boundary into a
   higher-priority rotation rule (SimC `interrupt` / `interrupt_if`).
3. The fight may interrupt the actor from outside: cancel the in-flight
   cast and lock out new casts for a duration.
4. Monte Carlo rolls crit PER HIT under `rolls: "per_hit"`, and procs
   react to each hit's own roll (#33 #3).
5. Sim timing fields can read `Plan` objectives, so cadence follows the
   build and the conditions the sim drives (#33 #2).
6. `on_non_crit` trigger (#33 #5) and event `chance`/`factor` reading
   pipeline stages (#33 #1).

**Non-goals (ROADMAP open debt, each named there)**

- SimC `chain` / `early_chain_if` (re-casting a channel into itself).
- Refund-on-cancel or cooldown-reset-on-cancel knobs. v1 has one fixed
  rule (§5.4); a knob is added when a game needs it.
- Random (jittered / chance-gated) interruptions in `Mode::Expected`.
- #33 #4 (EV expected uptime for Bernoulli-driven buffs).
- A general `{ on_event, fired }` trigger for arbitrary events.
- Hits spread over time WITHIN one segment (projectile travel, per-tick
  channels with dozens of ticks are modelled as that many segments).
- Free-casting (`cast_action`) a segmented action — compile error in v1.

**Anti-goals (would be a failure even if it "works")**

- Any number changing on the default path. Every config that names none
  of the new fields must be byte-identical — `examples/diablo4_rotation.rs`
  EV **and** MC blocks, both consumers' goldens, poe2 `rtce_parity` 63.
  The single deliberate exception is goal 4 for configs whose
  `defaults.proc_rolls` is `per_hit` (approved 2026-11-25, §5.5), which
  is CHANGELOG'd as a behavior change.
- Making every segment boundary a decision point for every action.
  Boundaries are decision points only for casts whose rule opted in.
- EV silently approximating something stochastic. Anything EV cannot do
  exactly fails closed with a positioned error.
- Allocation on the run loop.

## 3. Constraints

- CLAUDE.md conventions bind every slice: red-first TDD, hand-worked
  pins with contrast runs, mutation proof for behavior changes, fail
  closed with positioned errors, P8f docs discipline (every new field's
  rustdoc states DEFAULT, EVALUATION INSTANT, INTERACTIONS; every
  numbered claim pinned; every shipped default × override cell has a
  discriminating test or is listed as ROADMAP debt).
- Standing gates per slice: workspace tests, clippy `-D warnings`, fmt,
  `missing_docs`, `cargo doc` clean, both consumers re-verified.
- Field-type changes to public config structs are breaking for 0.x
  consumers that build them with struct literals; acceptable for 0.8.0
  but CHANGELOG'd with the migration.

## 4. Approaches considered

**Player cancels**

- **A. SimC shape — opt-in on the rotation rule, boundaries only,
  cancel into a higher-priority rule. (CHOSEN.)** Matches how players
  and SimC users think ("cancel slam 2 if Boneshatter is ready"), keeps
  the default path free of extra decision points, and needs no
  new "what next" vocabulary — the priority list already answers it.
- B. Action-level `stop_after` / `continue_when`. Simpler, but cannot
  express cancelling INTO something; a stop with nothing better to do is
  just idle time. Rejected.
- C. Rotation re-decides at every boundary for every cast. General, but
  makes every multi-segment cast pay a decision per boundary and changes
  behavior for configs that never asked. Rejected.

**Outside interruptions**

- **A. A fight-level schedule on `SimDef` (SimC `raid_events`).
  (CHOSEN.)** Deterministic schedules work in both modes; jitter is
  MC-only and fails closed in EV.
- B. An `interrupt` proc/buff effect. Couples fight script to the
  build's proc machinery; deferred.

**Per-hit crit under `per_hit` (MC)**

- **A. Sample the branched evaluation once per hit and SUM. (CHOSEN.)**
  The executor already multiplies the damage objective by
  `hits_per_use` (`exec.rs` `objs[damage_objective] * hits`), so a
  per-hit sample with `hits = 1` summed over `n` hits has exactly the
  EV expectation `n · E[obj]` — the EV/MC keystone holds for any
  pipeline shape. Cost: `n` sampled evaluations per segment, only under
  `per_hit`.
- B. Keep one shared mask (status quo). This is the bug #33 reports.

## 5. Design

### 5.1 Segments

`ActionDef` gains `segments: Vec<SegmentDef>` (default empty).

```jsonc
"rolling_slam": {
  "cost": { "mana": 12 },
  "segments": [
    { "duration": "objective.slam1_time",
      "damage": { "stats": { "strike": 0, "hits_per_use": 1 } } },
    { "duration": "objective.slam2_time",
      "damage": { "stats": { "strike": 1, "hits_per_use": 1 } },
      "effects": [ { "apply_buff": "aftershock" } ] }
  ]
}
```

`SegmentDef` fields:

- `duration: NumOrExpr` — seconds from the previous boundary (cast
  start for segment 0) to this segment landing. Evaluated AT THE
  SEGMENT'S START. Must be finite and `>= 0`.
- `damage: Option<ActionDamage>` — exactly `ActionDamage` today
  (overlay + `hits_per_use`), scoped to this segment. Absent = the
  segment lands no hits (a wind-up).
- `effects: Vec<EffectDef>` — run when this segment lands, same rules
  as `ActionDef::effects` (no `cast_action`).
- `measure: Option<Measure>` — per-segment override, see below.

Rules (all `sim::compile` errors, positioned at the action/segment):

- `segments` non-empty ⇒ `cast_time` and action-level `damage` must be
  absent (ambiguous). `cast_time` therefore becomes optional at parse;
  "no `cast_time` and no `segments`" is an error. *Builder's call
  confirmed here: `cast_time: String` → `Option<String>` is the breaking
  struct change §3 allows.*
- Action-level `gain` and `effects` remain legal and run ONLY when the
  LAST segment lands (full completion). Anything that must survive a
  cancel goes on an earlier segment. *(My call: one simple rule beats
  a per-effect "on cancel" flag.)*
- A segmented action named in a `cast_action` effect is an error.

**Measure, generalized.** `Measure` keeps its meaning per segment:
`cast_complete` measures at the segment's LANDING, `cast_start` at the
segment's START. Default unchanged (`cast_complete`). An action-level
`measure` applies to every segment unless the segment overrides it.

**What does NOT change for an unsegmented action.** It is executed by
today's code path, not by a one-element segment list, so its event
sequence, `seq` numbers and RNG draw order are untouched. *(Builder may
unify the paths only if the byte-identical gates prove it.)*

### 5.2 Segment events and the landing order

A new queue event `SegmentLand { action, segment, cast_id }` (`cast_id`
the generation-style staleness guard a cancel bumps, as `BuffExpire`
uses). Under `EventOrder::CompletionsFirst` it ranks with
`CastComplete`.

Order at a segment landing (the §"cast-complete order" diagram in
`sim/mod.rs` gains this, as its canonical home):

This GENERALIZES today's cast-complete order (`gain` → `casts += 1` →
measure/damage → action `effects` → procs `on_cast`, `on_hit`,
`on_crit`) rather than replacing it:

```text
[last segment]  apply action `gain`
[first segment] casts.<action> += 1
  → measure this segment's world, evaluate its damage, credit it
  → this segment's effects, in list order
[last segment]  → action `effects`, in list order
  → proc rolls: [first segment] on_cast, then this segment's
    on_hit, on_crit, on_non_crit
  → [non-last segment] cancel check (§5.3) → schedule next / end cast
```

For a one-segment action every bracket is true and the order is
EXACTLY today's — so a one-segment action and the same action written
without `segments` give identical numbers (a pinned equivalence), and
the documented rules hold unchanged: a hit is never changed by what it
causes, intrinsic effects resolve before triggered ones, and
`damage.stats` on the last segment reads the post-`gain` resource.
*(My calls: the cast count and `on_cast` belong to the FIRST landing so
a cast cancelled after slam 1 still "happened"; `gain` stays at full
completion, so a cancelled generator gains nothing.)*

**`rolls: "per_cast"` on a segmented action** rolls once per DAMAGING
SEGMENT (strike), not once per cast: under `per_cast` the segment is the
unit one crit mask is drawn for, so it is the unit one hit-trigger roll
is presented for. `on_cast` is still once per cast. *(My call; a
segmented action is new config, so no existing number moves. The
`ProcRolls::PerCast` rustdoc is reworded to "once per damaging cast, or
per damaging segment of a segmented action".)*

Also fixed in passing: `Trigger::OnCast`'s rustdoc says "rolls once per
cast begun"; it actually rolls at completion. Slice 4 corrects the doc
to describe the first-landing rule.

New sim symbols (reserved words, `0` when idle): `segments_done` (how
many segments of the in-flight cast have landed) and `segments_remain`.
They are the SimC `ticks_remain` analogue for `when` / `interrupt_if`.

Reporting: `ActionReport` gains `cancelled: u64` (casts cut short by a
player cancel or an interruption). Damage of landed segments is
credited to the phase each segment lands in (measurement-vs-attribution
rule unchanged).

Horizon: a segment landing at `t == duration` is drained like a cast
completion; a cast cut off by the horizon keeps the segments that
landed.

### 5.3 Player cancels (`interrupt`, `interrupt_if`)

`Rule` gains `interrupt: bool` (default `false`) and
`interrupt_if: Option<String>` (default absent; presence implies
`interrupt`).

- The in-flight cast remembers the INDEX of the rule that started it.
- At each NON-LAST segment boundary of that cast, after the landing
  order above, if the rule has `interrupt` and (`interrupt_if` absent or
  truthy), the executor evaluates the rules ranked ABOVE it with the
  normal decision logic (hard gates as if not mid-cast, plus `when`).
- If one is eligible, the cast is cancelled (`cast_id` bumped, pending
  `SegmentLand` stale, `cancelled += 1`) and that action begins at the
  same instant. Otherwise the cast continues.
- Same-rule or lower-priority rules never cancel (SimC semantics; no
  self-chain — that is `chain`, a non-goal).
- Deterministic, so EV and MC treat it identically.

Cancel cost rule (v1, fixed): cost already paid stays paid; the
cooldown armed at cast start stays armed; landed segments keep their
damage, effects and proc rolls; unlanded segments contribute nothing.

### 5.4 Outside interruptions

`SimDef` gains `interruptions: BTreeMap<String, InterruptionDef>`
(default empty). *(On `SimDef`, not `Scenario`: `Plan::evaluate` would
have to ignore a `Scenario` field, which is guessing.)*

`InterruptionDef`:

- `at: Vec<f64>` (SimC `timestamps`) **or** `first`, `period`, `last`
  (periodic; `last` default = fight duration). Exactly one form.
- `duration: f64` (default `0`) — lockout after the hit; `0` = a pure
  cast interrupt (SimC `casting`/`movement` blip), `> 0` = stun/movement.
- `period_stddev`, `duration_stddev`, `chance` — MC ONLY (defaults `0`,
  `0`, `1`). Nonzero / non-1 under `Mode::Expected` is a run error
  naming the interruption and field.

Semantics at an interruption instant: an in-flight cast is cancelled
(the current segment has NOT landed — it contributes nothing); the actor
may not begin casts until `t + duration` (a `Wake` there resumes the
rotation); resources, buffs, DoTs and cooldowns keep running.

**Coincident instants — one fixed rule under BOTH event orders** *(my
call)*: at an instant `t`, every `SegmentLand`/`CastComplete` at `t`
resolves first (the hit landed), then every interruption at `t`, and
only then does the rotation decide. So an interruption never cancels a
cast that began at its own instant for nothing, and the outcome does not
depend on `seq` (interruptions are queued at construction and would
otherwise always pop first under `scheduled`). The builder implements
this as a dedicated rank for interruption events plus deferring the
post-event decision while an interruption is pending at `t`.

- Overlapping lockouts: the lockout ends at the LATER end time
  (no stacking).
- A cast interrupted before its first landing counts in `cancelled`
  (and, as today, not in `casts`).
- `SimReport` gains per-interruption `count` (fires that occurred) and
  `locked_time` (the union of lockout time, clipped to the horizon).

### 5.5 Per-hit crit (#33 #3)

**Which knob (decided 2026-11-25): inherit, then override.** The crit
sampling unit of an action's damage is

```text
action.crit_rolls  ??  defaults.proc_rolls  ??  per_cast
```

- `ActionDef::crit_rolls: Option<ProcRolls>` — new, default absent
  (inherit). Applies to every segment of the action. Same layering as
  `measure` (package default, per-action override).
- `defaults.proc_rolls` keeps its existing meaning for procs AND now
  supplies the crit-sampling default; its own default stays `per_cast`.
- `ProcDef::rolls` is unchanged and still governs only how many rolls a
  proc is presented.

Under a resolved `per_hit` in `Mode::MonteCarlo`, each of a segment's
`n` hits (unsegmented action: the cast's `n`) is its own
`evaluate_phase_sampled` draw with `hits = 1`; damage credited is the
SUM; per-hit procs roll once per hit against that hit's own mask; a
`per_cast` hit-trigger proc on that action gates `on_crit` on "ANY hit
crit" and `on_non_crit` on "ANY hit did not crit". Under a resolved
`per_cast`, one mask per cast/segment, exactly as today (a `per_hit`
proc then rolls per hit against the shared mask, today's behavior).

**EV follows so the keystone holds.** When an action resolves to
`per_hit` crit sampling, EV weights match the independent-hit
distribution: a `per_cast` `on_crit` proc's weight becomes
`1 − (1 − p)^n` (any-crit; `on_non_crit`: `1 − p^n`); a `per_hit` proc
with `icd > 0` (at most one fire per cast, the ICD-at-one-instant rule)
fires with `1 − (1 − q)^n`, `q = chance × p` (× `1 − p` for
`on_non_crit`); `icd: 0` per-hit is unchanged (`n · q`).
`ev_procs_match_mc_in_the_per_hit_icd_bound_regime_regression` is
re-pinned under the new distribution and a resolved-`per_cast` twin
keeps the old one.

**Who sees a number move.** Only configs whose `defaults.proc_rolls` is
`per_hit` (MC damage variance and crit-gated proc rates; EV crit-gated
proc rates under ICD). A config that sets `per_hit` only on individual
procs, or nothing, is byte-identical. CHANGELOG "Changed", pinned by the
5-strike P(≥1 crit) contrast, with the one-line migration
(`"crit_rolls": "per_cast"` on an action restores the old behavior).

### 5.6 Objective reads (#33 #2)

Sim expressions may read `objective.<name>` — a `Plan` objective
evaluated against the measured world AT THE FIELD'S OWN INSTANT. Legal
in: action `cast_time`, segment `duration`, `cooldown`, `cost`, `gain`,
buff `duration`, proc `chance`, rule `when` / `interrupt_if`.
**Illegal in `damage.stats`** (an overlay that reads an objective
computed under that overlay is circular) — compile error.

World used: inside an action's or segment's field, the effective build
WITH that action's/segment's damage overlay evaluated at the same
instant (so a strike-specific attack-speed stat reaches its own
duration); elsewhere, the plain effective build. Evaluation is a read at
an instant, not a fixed point: a speed buff the sim later drives
changes the NEXT read.

Cost: one zero-allocation `Plan` evaluation per read (scratch-borrowed);
memoize per (instant, world) only if profiling asks.

**Definition, both modes** *(my call)*: an `objective.<name>` read is
the DETERMINISTIC `evaluate_phase` value — the branch-weighted expected
objective at that world — in EV and MC alike. It never consumes RNG
draws, so adding a read cannot shift a seeded MC run's draws. For an
objective downstream of a branched stage this is the expected value by
DEFINITION (documented on the symbol), not an EV approximation of
something MC samples, so the "EV silently approximating" anti-goal does
not apply. Timing objectives (hit time) are normally branch-independent
anyway. Interaction with #1: objectives
whose stages feed event chances are evaluated with the same rules.

### 5.7 `on_non_crit` (#33 #5) and event chances reading stages (#33 #1)

- `Trigger::OnNonCrit`: MC rolls on hits whose mask has the crit bit
  clear (same draw as `on_crit`); EV weights by `1 − crit_chance`;
  `per_hit`, ICD and `actions` filter semantics identical to `OnCrit`.
  No `"crit"` event in the plan ⇒ every hit is a non-crit (mirrors
  `crit_chance` returning `0`).
- `EventDef::chance` / `factor` compile against the stages BEFORE the
  FIRST branched stage of the pipeline (those are branch-independent);
  a reference to any later stage is a compile error naming the event and
  stage. `Plan::crit_chance` evaluates that pipeline prefix instead of
  only `fill_prefix_slots`. No change for configs that read no stage.

## 6. Delivery slices (each its own PR, CHANGELOG, gates)

1. `on_non_crit` (§5.7).
2. Event chances read pre-branch stages (§5.7).
3. Per-hit MC crit under `per_hit` (§5.5) — behavior change.
4. Segments + `SegmentLand` + landing order + `segments_done`/`remain`
   + `cancelled` report column, no cancels yet (§5.1–5.2).
5. Player cancels (§5.3).
6. Objective reads (§5.6).
7. Outside interruptions (§5.4).
8. A Rolling-Slam-shaped CI-run example with hand-worked EV pins and
   MC contrast; ROADMAP debt entries for every §2 non-goal; release
   staging for 0.8.0 (ROADMAP version-pointer sweep).

Slices 1–3 are independent of the cast model. 5–7 depend on 4.

## 7. Matrix cells that need discriminating tests

- `segments` empty × present; `measure` action × segment override ×
  default, at a segment that reads a buff applied by the prior segment.
- `interrupt` false × true × `interrupt_if` truthy/falsy; higher rule
  eligible × not; cancel at segment 0 vs a later boundary.
- Crit sampling resolution: `crit_rolls` absent × per_cast × per_hit,
  against `defaults.proc_rolls` absent × per_cast × per_hit (each cell
  must resolve visibly), × mode EV/MC × segmented/unsegmented, × a
  proc's own `rolls` per_cast/per_hit, × `icd` 0 / > 0.
- `event_order` scheduled × completions_first for `SegmentLand` vs
  `BuffExpire` and vs an interruption at one instant.
- Interruption `at` × periodic; `duration` 0 × > 0; jitter in EV
  (error) × MC.
- `objective.*` in each legal field; in `damage.stats` (error).
- `on_non_crit` with and without a `"crit"` event.

## 8. Left to the builder

- Internal representation (whether segments and the legacy path share
  code), the `cast_id` mechanism, memoization of objective reads.
- Exact error wording (positioned, did-you-mean where a name is wrong).
- Test and fixture layout; which example the Rolling Slam pins live in.
- Whether `segments_done`/`segments_remain` also get per-action forms
  (`segments_done.<action>`) if a slice needs them.
