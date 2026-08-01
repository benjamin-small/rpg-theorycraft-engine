# Chapter 5 — A rotation

> Runnable companion: [`examples/guide_05_a_rotation.rs`](../../crates/rtce/examples/guide_05_a_rotation.rs)
> · `cargo run -p rtce --example guide_05_a_rotation`

Chapter 4 ended on an uncomfortable number: `"focused": 0.2`. We made it up.

To compute it instead, the engine needs to know things it has had no way to
express — what the archer can *do*, what those things cost, how long they
take, and which one it picks at any given moment. That is a second body of
configuration, and it buys a second fidelity level:

| | What it computes | Uptimes | Cost |
|---|---|---|---|
| `Plan::evaluate` | closed-form average | **asserted** | ~µs |
| `sim::run` EV mode | one deterministic timeline | **computed** | ~ms |
| `sim::run` MC mode | N sampled timelines + a distribution | computed | ~ms×N |

Same `GameDef`, same `BuildState`, same compiled `Plan`. Chapters 5 and 6
add nothing to tier 1 at all — the files are byte-identical to chapter 4's.

## What the archer can do

```json title=05-simdef.json
{
  "resources": {
    "stamina": { "max": "100", "regen_per_sec": "0" }
  },
  "actions": {
    "power_shot": {
      "cast_time": "1",
      "cost": { "stamina": 40.0 },
      "damage": { "stats": { "attack_power": 120.0 } }
    },
    "quick_shot": {
      "cast_time": "1",
      "gain": { "stamina": 40.0 },
      "damage": { "stats": { "attack_power": 60.0 } }
    }
  },
  "damage_objective": "hit_after_armor"
}
```

A **spender** and a **generator**, which is the oldest shape in the genre.

- **`resources`** — pools with a cap and a regen rate. Both are
  *expressions*, not just numbers, so a resource can scale off the build.
- **`damage.stats`** — a per-cast overlay on the build. `power_shot` swings
  at the archer's full 120 attack power; `quick_shot` at 60. Everything else
  about the damage calculation — crit, the additive pool, armor — comes from
  the `GameDef` we already have, unchanged.
- **`damage_objective`** — which pipeline stage the sim treats as "the
  damage this cast did". Ours is `hit_after_armor`, the stage chapter 4
  added.

> **Scope, honestly.** Stamina's regen is zeroed and `quick_shot`'s gain is
> set exactly equal to `power_shot`'s cost so the cast sequence comes out a
> clean alternation you can verify by hand. Real resource economies are not
> this tidy, and the resulting cadence is a teaching artifact rather than a
> claim about game design. `examples/diablo4_rotation.rs` carries the same
> disclosure for the same reason.

## What it picks

```json title=05-rotation.json
{
  "rules": [
    { "action": "power_shot", "when": "stamina >= 40" },
    { "action": "quick_shot" }
  ]
}
```

A **priority list**: first eligible rule wins. This is how players actually
describe rotations, and it is deliberately not a script — there is no
sequence, no loop, no state machine. At every decision point the engine
walks the list top to bottom and casts the first thing it can.

Two kinds of gate stack up here:

- **Hard gates are automatic.** Off cooldown, cost payable. You never write
  them; an action that costs 40 stamina is simply not eligible at 20.
- **`when` adds strategy on top.** An expression over resources, buff
  stacks, and the rest of the sim's symbol space.

Our `when: "stamina >= 40"` is, strictly, redundant — the hard gate already
enforces it. It is written out because that is the honest shape of a real
priority list, and because the interesting version ("only spend above 60, so
there's always a reserve") is one character away.

## The fight, again

```json title=05-scenario.json
{
  "phases": [
    {
      "name": "dummy",
      "weight": 60,
      "stats": { "enemy_armor": 20.0 }
    }
  ]
}
```

Same `Scenario` type as chapter 4, one new meaning: **under the sim, a
phase's `weight` is its duration in seconds.** Sixty seconds of training
dummy. And note there is no `uptimes` block any more — we have stopped
asserting.

## Running it

```rust
let sim_plan = sim_compile(&plan, &simdef, &rotation)?;
let report = run(&plan, &sim_plan, &build, &dummy, Mode::Expected)?;
```

`sim::compile` is the sequencing tier's equivalent of `plan::compile` — the
single point where a `SimDef`'s expressions get parsed, against a symbol
space extended with resources and buff stacks. A typo in a `when` clause
fails here, with a position, not at runtime.

```
Guide chapter 5 — a rotation (60s training dummy, EV mode)
  action        casts         damage    share
  power_shot       31      5304.7200   68.13%
  quick_shot       29      2481.2400   31.87%
  total: 7785.9600 damage over 60s = 129.7660 dps
  stamina: 0.0000s starved, 0.0000s capped
  condition uptimes reported: (none — nothing in this config drives one)
```

## Where the numbers came from

Nothing drives `focused`, so it is 0 and the build's `focused`-gated +50
crit damage contributes nothing — `crit_damage` folds to 1.5, exactly
chapter 3's value.

```
power_shot  120 × 1.55 × (0.7 × 1.0 + 0.3 × 1.5) = 186 × 1.15 = 213.9
            × (1 - 20/100) armor                              = 171.12
quick_shot  half the attack power, every other factor identical =  85.56
```

The cadence. Stamina starts full at 100, never regens, and both shots take
exactly 1s — so a decision lands on every integer second:

```
t=0: stamina=100    -> power_shot (100-40=60)
t=1: stamina=60     -> power_shot (60-40=20)
t=2: stamina=20<40  -> quick_shot (20+40=60)
t=3: stamina=60     -> power_shot (60-40=20)
t=4: stamina=20<40  -> quick_shot (20+40=60)   … and so on
```

From `t=1` onward stamina alternates 60/20, so `power_shot` takes every odd
second plus `t=0` itself — two in a row only at the very start, because
stamina begins full. Sixty slots split **31 / 29**:

```
total = 31 × 171.12 + 29 × 85.56 = 5304.72 + 2481.24 = 7785.96
dps   = 7785.96 / 60                                  = 129.766
```

`time_starved` and `time_capped` are both 0: stamina never drops below what
the unconditional fallback needs, and the 20/60 oscillation never pins it at
the cap. Those two diagnostics are how you notice a rotation that is quietly
wasting a resource.

## The empty line at the bottom

```
  condition uptimes reported: (none — nothing in this config drives one)
```

`SimReport::condition_uptime` reports conditions the **timeline drove**. Our
config drives none, so the map is *empty* — not `focused: 0.0`, but no entry
at all. The example asserts that emptiness, because it is the precise
statement of what is still missing.

`focused` does still fold as 0 in the math — that is why `crit_damage` came
out 1.5. It simply has no driver for the sim to integrate.

## What we can't do yet

We have a timeline, and it computes an uptime of nothing. One config concept
is missing: something that makes `focused` *true for a while*.

That is chapter 6, and it is where the guide's sharpest lesson lives.

---

[← Chapter 4](04-conditions-and-scenarios.md) · [Next: Chapter 6 — Buffs and uptime →](06-buffs-and-uptime.md)
