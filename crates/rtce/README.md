# rtce — RPG theorycraft engine

A generic, config-driven theorycrafting engine. A game's DPS/theorycraft
algorithm — its stats, how contributions fold into buckets, its
probabilistic events (crits, procs, …), and the pipeline of derived
stages — is data, not Rust code. You write that algorithm once as JSON,
`rtce` compiles it into a flat evaluation plan, and every candidate build
then evaluates against that plan with ZERO heap allocation on the hot
path — all the parsing, name resolution and layout work is paid once, at
compile time, so pricing a candidate is a flat walk over a preallocated
slot array. (No benchmark ships with the crate yet; the allocation claim
is structural, not a measured throughput number.)

On top of that same plan sits a discrete-event TIMELINE simulator, so one
config family answers both "what does this build average, given ASSERTED
uptimes?" and "what actually happens over 60 seconds of a priority-list
rotation — and what uptimes does that produce?"

## Three fidelity levels

| | What it computes | Uptimes | Cost |
|---|---|---|---|
| `Plan::evaluate` | closed-form average | inputs (asserted) | ~µs |
| `sim::run`, `Mode::Expected` | one deterministic, branch-blended timeline | **computed** | ~ms |
| `sim::run`, `Mode::MonteCarlo` | N seeded sampled timelines + a `dps` distribution | **computed** | ~ms×N |

All three run on the same compiled `Plan`, and where they overlap they are
required to agree: `sim`'s keystone test reproduces `Plan::evaluate`'s
number EXACTLY on a degenerate config with nothing for the timeline to
add. `Plan::explain` re-runs the identical engine with per-phase,
per-stage and per-branch tracing turned on when you need to see WHY a
number came out the way it did.

## The config tiers

Evaluation needs three artifacts, each with a different lifetime:

1. **`GameDef`** — the ALGORITHM: stat/condition/bucket registries, bucket
   fold rules (`sum` / `summed_group` / `product`), probabilistic events,
   and the ordered pipeline of named stages. Compiled once by
   `plan::compile` into a `Plan` — the only place a `GameDef`'s
   expressions are parsed. (`sim::compile` below is the corresponding
   single parse point for a `SimDef`'s.)
2. **`BuildState`** — ONE candidate: stat values plus tagged contributions
   into buckets, each optionally gated by an event or a condition. The
   only artifact that changes per permutation a search driver compares.
3. **`Scenario`** — THE FIGHT being asked about: weighted phases with stat
   overrides and condition-uptime fractions.

Sequencing adds two more, compiled by `sim::compile` against an existing
`Plan`:

4. **`SimDef`** — resources (capped pools with regen), actions (cast time,
   cooldown, resource cost/gain, an optional per-cast `damage.stats`
   overlay, and `apply_buff`), buffs (timed contribution/condition windows
   with stack policies and optional DoT ticks), and procs
   (chance-triggered, ICD-gated, optionally filtered to named actions).
5. **`Rotation`** — a SimC-style priority list; the first eligible rule
   wins. Hard gates ("off cooldown", "cost payable") are automatic; a
   rule's `when` predicate adds strategy on top.

Sim expressions compile against the `Plan`'s own stats and conditions
EXTENDED with sim state: `time`, `duration`, each resource's amount,
`cooldown.<action>`, `buff.<buff>`, `buff_remaining.<buff>`,
`stacks.<buff>`, `casts.<action>`. Cast times, proc chances and rotation
`when` predicates are always expressions; a buff's `duration`, an action's
`cooldown`, its `cost`/`gain` amounts and its `damage.stats` values each
take a plain number OR an expression, evaluated at a documented instant.
That is what makes a pandemic-style refresh data:
`"min(12, buff_remaining.x + 8)"`. Pipeline stages and buckets are
deliberately absent from this space — naming one is a fail-closed compile
error, as is any other unresolved name.

## Quickstart

```rust
use rtce::{build::BuildState, gamedef::GameDef, plan, scenario::Scenario};

let def: GameDef = serde_json::from_value(serde_json::json!({
    "stats": ["weapon_damage"],
    "pipeline": [{ "name": "dps", "expr": "weapon_damage * 2" }],
    "objectives": ["dps"]
}))
.unwrap();
let plan = plan::compile(&def).unwrap();

let build: BuildState = serde_json::from_value(serde_json::json!({
    "stats": { "weapon_damage": 100.0 }
}))
.unwrap();
let scenario: Scenario = serde_json::from_value(serde_json::json!({
    "phases": [{ "name": "single_target", "weight": 1.0 }]
}))
.unwrap();

let mut scratch = plan.scratch();
let objectives = plan.evaluate(&build, &scenario, &mut scratch).unwrap();
assert_eq!(objectives[0], 200.0);
```

## Counted and snapshotted state

A buff in `rtce` is internally an INSTANCE LIST. Config collapses it per
mechanic, via `max_stacks` (default `1`; `0` = unbounded) and
`on_reapply`:

- `refresh` — one instance, replaced on reapplication. The degenerate
  binary buff, and every pre-0.3.0 config's behavior.
- `add_refresh_all` — count up to the cap; every instance shares ONE
  expiry clock, reset by each application (ARPG charges).
- `add_independent` — each instance carries its own duration; at the cap
  the earliest-expiring one is evicted (stacking ailments).
- `strongest` — the incoming instance replaces the incumbent only when its
  magnitude is STRICTLY higher; a losing reapplication changes neither the
  rate nor the expiry.

A `tick_objective` DoT is either LIVE (re-evaluated on every state change,
× the stack count) or `{ "objective": …, "snapshot": true }`, where each
instance captures its rate at its own application and ticks it unchanged
to expiry. `SimReport` reports each buff's computed `uptime` and its
time-integrated `avg_stacks` alongside per-action casts/damage/share,
per-resource starvation and cap time, and proc fire counts.

`contributions` fold with their value MULTIPLIED by the stack count — 3
stacks of `+10` in a `product` bucket read `×1.30`, not `×1.10³`. That is
a deliberate linearization, pinned by an example so it stays a stated
choice. `conditions` are driven at their full configured value while ANY
instance is live and are never scaled by the count.

## Examples

Six runnable walkthroughs, each carrying hand-worked pins in comments,
asserted and run in CI. Run any of them with
`cargo run -p rtce --example <name>`.

| Example | What it teaches |
|---|---|
| [`your_own_game`](examples/your_own_game.rs) | The smallest starting point: the three closed-form config tiers on a made-up archer game in ~40 lines of JSON, two scenarios, and `explain()` output. Level 1 only — it never calls `sim::run`. |
| [`diablo4_basics`](examples/diablo4_basics.rs) | One build priced against two fights on a real game's damage slice, with the branch table behind the crit expectation. |
| [`diablo4_rotation`](examples/diablo4_rotation.rs) | Sequencing end to end: mana, a spender/generator pair, a cooldown-gated buff window whose `vulnerable` uptime FALLS OUT of the timeline, in both EV and Monte Carlo mode. |
| [`poe2_charges`](examples/poe2_charges.rs) | `add_refresh_all` with `max_stacks: 3`, an expression `duration`, and `stacks.X` gating a rotation rule. |
| [`poe2_poison`](examples/poe2_poison.rs) | Unbounded `add_independent` snapshot DoTs, applied by the skill's own `apply_buff` — run again via a proc as a contrast. |
| [`poe2_triggers`](examples/poe2_triggers.rs) | A `ProcDef::actions` trigger filter plus `cast_action`, with `apply_buff` on both a primary and the free-cast secondary. |

Note what has NO example: `strongest`. Its coverage is the test suite, not
a runnable slice — worth knowing before reaching for it, because it is
also the policy with the sharpest edge.

**Scope, honestly.** The `diablo4_*` examples run on a thin SLICE of
Diablo 4's damage formula (crit/overpower branching, the shared additive
pool, summed multiplier groups, vulnerable, DoT, attack speed) — not the
game; `diablo4_rotation`'s `SimDef` is a demonstration cadence, not real
skill data. `tests/fixtures/poe2/gamedef.json` is a PoE2-*shaped*
demonstration slice, not Path of Exile 2's damage model and not derived
from game data — every coefficient in it is `representative`, chosen so
each pin hand-derives in a comment. What the slices demonstrate is the
SHAPE: a real game's algorithm expressed entirely as data, exact enough
that a production calculator runs on it.

Nothing in the three PoE2 configs samples — their crit is closed form and
every proc they define is `chance: "1"` — so Monte Carlo mode is asserted
there to reproduce EV *exactly*, with zero spread, rather than within a
tolerance band. That is the stronger claim, but it is a claim about
exactness: `diablo4_rotation` is the one example that actually exercises
sampling.

## Status

Parity-proven against two independent consumers.

`diablo4-calc`: all 7 of its archetype builds reproduced to <1e-9 relative
during the P4 cross-engine proof (the standing numbers 8,096.02 …
6,769.10). As of its P4c switchover it runs **solely** on `rtce` in
production — native damage math deleted, `calc::evaluate` a thin shim over
an rtce-compiled plan — including in the browser via WASM.

`poe2-calcs`: a generated 209-stage `GameDef` reproduces that
calculator's native math to 1e-9 across 63 parity tests (standing
references 124.53 / 129.51 / 793.76 dps), with a 156-pair
`(StatId, ModKind)` sweep guarding against silent routing drift. That
consumer's own math is untouched — the harness is a proof, not a
switchover.

## License

Licensed under either of MIT OR Apache-2.0 at your option. License texts
ship with this crate (`LICENSE-MIT`, `LICENSE-APACHE`); canonical copies
are in the repository root at
https://github.com/benjamin-small/rpg-theorycraft-engine.
