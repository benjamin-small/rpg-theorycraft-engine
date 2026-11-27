# EV expected buff uptime (`ev_uptime`) — design

Issue #33 item 4. Status: design, pending approval.

## Intent

In `Mode::Expected` (EV), a buff applied by a probabilistic proc is up
whenever the EV accumulator happens to refresh it in time. The
accumulator fires every 1/q rolls, so a buff with duration D is
reported 100% up whenever `gap / q < D`. The true expected uptime is
lower: for q = 0.23 per hit, 0.33 s hits, D = 4 s it is
1 − 0.77^(4/0.33) ≈ 96%, not 100%. Monte Carlo already gets it right.

EV is the mode consumers rank and search with: one deterministic,
noise-free number per build, pinnable by hand. poe2's CLI defaults to
it. So a "Recently" condition driven by real crit events would come out
wrong by default unless the consumer switches to MC. This design lets a
buff opt in to having EV carry its *probability of being up* instead of a
binary window. EV and MC then agree on that buff's uptime, and on
everything it drives.

## Goals

- A per-buff opt-in, `BuffDef::ev_uptime`, whose `expected` value makes
  EV track P(active) for that buff exactly, given the EV timeline.
- Everything the buff drives reads P: conditions, contributions, and a
  live `tick_objective`, plus the buff and condition uptime reports.
- EV and MC agree on such a buff's UPTIME (and `condition_uptime`),
  shown by a convergence regression.

**What "exact" promises (call, made after review).** P (the uptime) is
exact. What the buff DRIVES is EV's usual linearization: the Plan reads
the expected contribution/condition, which is `f(E[X])`, not `E[f(X)]`
when the Plan is non-linear in it (a `product` bucket, a condition
feeding a multiplicative chain). That is the same reading EV already
gives a static uptime and its blended crit damage, so it is not a new
approximation. But it is stated on the field's rustdoc, so it is not a
silent one. The convergence regression asserts uptime, not damage.

## Non-goals

- MC is unchanged. The knob is EV-only and MC ignores it. MC already
  samples real applications.
- No change to the EV accumulator's fire semantics for anything other
  than the opted-in buff's application.
- No default change. Absent means `discrete`, today's behavior, and every
  existing config and example stays byte-identical.
- No `defaults.ev_uptime`. The user chose per-buff opt-in (Approaches,
  below).

## Anti-goals

- Never approximate silently. Every case where the expected-state model
  isn't exact is a positioned compile error naming the buff, never a
  fallback (see Constraints).
- Never let an `expected` buff be read as if it were binary. A rule that
  branches on `buff.x` against a probability would silently mean
  something different from MC.

## Approaches considered

**Opt-in shape.** The user picked A.

- **A. Per buff: `ev_uptime: "expected"` (chosen).** It's explicit and has
  a small surface. It fits the fail-closed limits, because only buffs
  that satisfy them opt in.
- **B. `defaults.ev_uptime` plus a per-buff override,** the `crit_rolls`
  inheritance pattern. Rejected: turning the default on makes every
  stacking, snapshot or ICD-fed buff in a config a compile error until
  each one is opted back out.

**Mechanism.** One sensible way; the alternatives don't fit.

- **The issue's scalar recurrence,**
  P(up at next roll) = q + (1−q)·P(remaining > gap). It's exact only for
  constant cadence, and the timeline has variable cadence (haste,
  phases, rotation changes).
- **A consumer-supplied closed form** (`uptime = 1-(1-p)^(D/h)`). That's
  today's workaround, and it is blind to the rotation.
- **Chosen: an exact roll log** (below). It's exact for any cadence and
  needs only bounded state.

## The model

Every application of an `expected` buff is a **roll** `(t, q, D)`:

- `t`: the instant it happens.
- `q`: the probability the application happens **at least once during
  that cast event**. One log entry per cast (or per tick/event), never
  one per proc roll (call, made after review). The rolls inside one
  cast are not independent:
  - Under `crit_rolls: per_cast` (`CritWeight::Shared`) every crit-gated
    proc shares one crit outcome.
  - `on_crit` and `on_non_crit` are mutually exclusive.
  - An effects list naming `ApplyBuff X` twice is one event, not two.

  So q must be computed per cast by conditioning on the crit model,
  exactly as MC realizes it. With independent (non-crit-gated) procs,
  `P(no application) = Π_indep (1 − aᵢ)`, multiplied by the crit-model
  term:
  - Shared: `p·Π_crit (1−cᵢ)^{rᵢ} + (1−p)·Π_noncrit (1−cⱼ)^{rⱼ}`.
  - Independent (per-hit crits): a sum over the crit count k ~
    Binomial(n, p) of the per-k no-fire product (O(n)), so `per_cast`
    and `per_hit` proc rolls mix correctly.

  Here `rᵢ` is the proc's roll count (1 or n per `ProcRolls`), the rolls
  are clamped to [0, 1] as MC's single fire at `chance > 1` does, and
  each proc counts once however many times its effects list names X.
  The q-derivation is a pure function of (proc set, crit model, n, p,
  chances), so it gets its own hand-worked unit pins, including the
  reviewer's three shapes (Shared n-hit `on_crit`; `on_crit` +
  `on_non_crit`; X listed twice). For a guaranteed application (an
  action's `apply_buff` / `effects`) `q = 1`.
- `D`: the duration, evaluated at that application exactly as today
  (snapshotted, live state).

D is evaluated once per log entry, at the roll instant, BEFORE any of
the crossing's discrete effects run (a crossing proc whose list puts
other effects before `ApplyBuff X` does not let them leak into X's D).
That's documented on the field (call).

With refresh-on-reapply, the buff is up at time `s` iff the *latest
successful* application before `s` still covers `s`. So:

    P(s) = Σⱼ qⱼ · Πₖ₍ₜₖ>ₜⱼ, ₜₖ≤ₛ₎ (1 − qₖ) · [tⱼ + Dⱼ > s]

When every D is equal, this reduces to `1 − Π (1 − qⱼ)` over the rolls
with `tⱼ > s − D`, the formula in the issue's text. The general
(latest-wins) form is required because D may be an expression, and a
refresh with a shorter D shortens the window. Match the existing
expiry-instant convention exactly: the same `>` vs `>=` as a discrete
`BuffExpire` at `tⱼ + Dⱼ`.

Store each entry's latest-wins WEIGHT `wⱼ = qⱼ · Π_{k>j}(1 − qₖ)`:
appending a roll multiplies every older weight by `(1 − q_new)`. Then
`P(s) = Σ_{j: tⱼ+Dⱼ>s} wⱼ`. An entry whose window has passed is
dropped WITH its weight (it can't cover any later s), and the later
rolls' failure factors stay inside the older live weights, so dropping
is exact even under variable D. (The naive "drop and recompute the
product" is wrong under latest-wins. Reviewer's case: rolls at t=0
(q .5, D 10) and t=1 (q .5, D 1). True P(5) = 0.25; dropping t=1 and
recomputing gives 0.5. That case is a pin.) The log holds only entries
whose window is live, so it is bounded, and its storage must be reused
(no allocation per roll once warmed).

**P is piecewise constant.** It steps up at each roll and down when an
entry ages out. Each change is a state change, which is the same
transaction (`flush_before_change` → mutate → `refresh_after_change`) a
discrete application or expiry already goes through. Conditions,
`tick_objective` and the reports therefore integrate exactly with the
existing machinery. Age-outs must be on the heap, so that resource
`max`/`regen` and anything else reading conditions between casts is
never stale. Do it the way `BuffExpire` does: one live entry per buff,
at the next age-out, generation-guarded and rescheduled. Its event
ordering class is the same as `BuffExpire`.

**Coincident instants (call, after review).** Ordering must match the
discrete/MC realization exactly, not just be documented. Each log entry
consumes a `seq` at its roll, as a discrete application does. The
rescheduled age-out heap item carries the STORED seq of the entry it
ages out, not a fresh one, so `(time, rank, seq)` orders it against a
same-instant `CastComplete` exactly as that roll's discrete
`BuffExpire` would under every `EventOrder`. One live heap entry per
buff still suffices. A pin at a coincident cadence (an age-out landing
exactly on a cast completion, under both `EventOrder` values) is
required.

## What reads P

When the buff's `ev_uptime` is `expected` and the mode is EV:

| Reader | Today (binary active) | `expected` |
|---|---|---|
| condition `c = v` | `v` while active, else static uptime | `P·v + (1 − P)·static` |
| contribution value | `value × stacks` (1) | `value × P` |
| live `tick_objective` | `rate × stacks` | `rate × P` |
| `BuffReport.uptime` | ∫active / duration | ∫P / duration |
| `BuffReport.avg_stacks` | ∫stacks / duration | ∫P / duration (max one stack) |
| `condition_uptime` | ∫value | ∫(blended value), unchanged machinery |
| `applications` count | discrete applications | number of rolls (a builder call: document it) |

The condition row is the expectation of today's precedence rule (active
buff wins, else static), assuming the roll outcome is independent of the
static uptime. The contribution row scales the value linearly, which is
exact for one buff by linearity of expectation, and is the same reading
`stacks` scaling already uses.

## Constraints (fail closed at `sim::compile`, positioned, naming the buff)

An `ev_uptime: "expected"` buff must:

1. have `on_reapply: refresh` and `max_stacks: 1`. Stacking policies need
   a distribution over stack counts, not one P.
2. have no snapshot `tick_objective`. A snapshot rate per instance has no
   expected-state reading here. A live one is fine.
3. not be applied by any proc with `icd > 0`. Gated rolls aren't
   independent, so the product formula isn't exact.
3b. not be applied by a free cast (an action reached via a proc's
   `CastAction`). In EV a free cast happens only at an accumulator
   crossing, so its application is itself the binary-window artifact
   (call, after review). A config wanting it applies X from the proc
   directly. This also makes the ICD question transitive-free.
4. not be read by any sim expression as `buff.<name>`,
   `buff_remaining.<name>` or `stacks.<name>`, anywhere: rules' `when`,
   chances, durations, costs, resources, and so on. A probability read as
   a boolean silently diverges from MC.
4b. not drive a condition that any sim expression reads (decided by the
   user: forbid). Under EV that condition is a blend (e.g. 0.96), so a
   rule `when: "recently >= 1"` would never fire where MC fires 96% of
   the time. The Plan's own reads of the condition (damage) are the
   point and stay legal. `objective.<name>` reads stay legal even when
   the objective depends on X: they are the same EV-averaged readings
   damage already uses (see the linearization note under Goals). The
   check reuses the resolver walk for constraint 4.
5. not share a driven condition with any other buff. The name-order
   precedence rule has no exact fractional form, and fail-closed beats
   an independence guess.

Under MC, all of these are still checked, so a config valid in one mode is
valid in both. The knob itself does nothing in MC.

A proc whose effects list contains `ApplyBuff` of an `expected` buff
alongside other effects stays legal. Its other effects keep firing
discretely via the accumulator. Only the expected buff's application is
taken off the accumulator's crossing and logged per roll.

## Docs and pins (P8f)

- The `BuffDef::ev_uptime` rustdoc states the default (`discrete`), the
  evaluation instant (q at each roll, the same instant the accumulator
  adds its mass; D at application as today) and the interactions (the
  table and all five constraints, MC ignoring it).
- A hand-worked pin: a constant cadence chosen so P is checkable on
  paper, for example q = 0.5, 1 s gap, D = 2.5 s, giving steady-state
  P = 1 − 0.5³ = 0.875 between rolls, with the exact warm-up steps.
  Mutation-check it.
- A variable-D pin exercising the latest-wins form, where a shorter
  refresh shortens the window.
- An EV-vs-MC convergence regression on the issue's shape (q = 0.23,
  0.33 s, D = 4), asserting EV within MC's tolerance and asserting
  `discrete` EV is not.
- A discriminating test for each `(default × override)` cell:
  `discrete` vs `expected`, crossed with each reader in the table.
- A test for each compile error.
- `examples/diablo4_rotation.rs` stays byte-identical, and the consumer
  gates stay green under the patch override.
- ROADMAP: remove the "#33 item 4 EV uptime" open-debt line. Close #33
  when this ships.
- CHANGELOG Unreleased entry. A guide section in
  `docs/guide/03-events-and-branches.md` next to the EV/MC proc text.

## Left to the builder

- Data structure for the roll log (ring buffer or `VecDeque`, reused).
- Whether to coalesce same-instant rolls.
- Exact field name of the internal state, and the report's
  `applications` reading (documented either way).
- Where the checks live in `compile.rs`, reusing the existing
  symbol-resolution walk for constraint 4.
- Slicing into commits. A reasonable order: field, parse and
  compile-time checks → roll log and P plus the age-out event → readers
  → reports → pins and docs.
