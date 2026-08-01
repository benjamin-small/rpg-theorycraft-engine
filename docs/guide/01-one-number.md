# Chapter 1 — One number

> Runnable companion: [`examples/guide_01_one_number.rs`](../../crates/rtce/examples/guide_01_one_number.rs)
> · `cargo run -p rtce --example guide_01_one_number`

We are going to build a small RPG. Not a game you can play — a game you can
*ask questions about*. By the end of this guide it will have a crit system,
gear modifiers, a debuff window, a stamina economy, a rotation, and a Monte
Carlo distribution over a sixty-second fight. It starts here, with a single
number.

## The three tiers

The one idea to hold onto: in `rtce` a game's algorithm is **data**, not Rust
code. You describe it once, `rtce` compiles it, and then every character you
want to compare evaluates against that compiled plan.

That description splits into three pieces, each with a different lifetime:

| Tier | What it is | Changes |
|---|---|---|
| `GameDef` | The **rules of the game** — stats, how modifiers fold, the pipeline of derived values | Once per game |
| `BuildState` | **The character** — one player's gear, stats and modifiers | Per character |
| `Scenario` | **The fight** being asked about — its phases and their conditions | Per question |

In plain terms: the `GameDef` is the rulebook, the `BuildState` is a
character sheet, and the `Scenario` is the monster you are pointing that
character at.

Keeping them separate is what makes the engine cheap. The expensive work
(parsing expressions, resolving names to slots) happens once when the
`GameDef` compiles. After that, evaluating a character is a walk over a
preallocated array.

## Tier 1 — the GameDef (the rulebook)

Our archer, at its very simplest. One stat, one stage, one answer:

```json title=01-gamedef.json
{
  "stats": ["attack_power"],
  "pipeline": [
    { "name": "hit", "expr": "attack_power" }
  ],
  "objectives": ["hit"]
}
```

Three fields are doing three different jobs:

- **`stats`** declares the raw numbers a build is allowed to supply. This is
  a closed list — a build that sets `attak_power` gets an error, not a
  silent zero. (Fail-closed is a theme; you will meet it repeatedly.)
- **`pipeline`** is an ordered list of named stages. Each stage has an
  expression, and each stage can read every stat and every *earlier* stage
  by name. Right now there is one stage and its expression is just a stat.
- **`objectives`** names the stages you actually want handed back. A
  pipeline usually has more stages than you care to read; objectives are the
  ones you do.

A `GameDef` also accepts `conditions`, `buckets`, and `events`. All three
default to empty, which is why they are absent above — we add them in
chapters 2, 3, and 4.

## Tier 2 — the BuildState (the character sheet)

This is the player. One specific archer, with whatever gear and stats they
are currently carrying — the thing you edit when you swap a ring, and the
thing you copy when you want to ask "what if I did it this way instead?"

```json title=01-build.json
{
  "stats": { "attack_power": 120.0 }
}
```

One archer, 120 attack power. That is the whole character so far.

A `BuildState` also carries a `contributions` list — every modifier the
character has, from gear, passives and skills. It defaults to empty and we
have nowhere to put modifiers yet, so it is absent here. Chapter 2 fills it
in, and from then on it is the bulk of the file.

> **About the name.** "Build" is the ARPG player's word for a character
> configuration — a *lightning sorcerer build*, a *bow build*. `rtce` also
> calls one a **candidate**, because the engine exists to compare a great
> many of them: an optimizer generates thousands of `BuildState`s that
> differ by one ring and prices them all against the same `GameDef`. Same
> object, two vocabularies. When this guide says "the build", it means your
> character.

## Tier 3 — the Scenario (the fight)

```json title=01-scenario.json
{
  "phases": [
    { "name": "dummy", "weight": 1 }
  ]
}
```

This one looks like pure ceremony, and at this stage it is — there is
nothing about the fight that our algorithm can read. But it is *required*,
and that is deliberate: `rtce` never evaluates a build in the abstract, only
against a stated fight. A scenario is a list of weighted **phases**, and
each phase can override stats and set condition uptimes independently. One
build, priced against "training dummy" and "raid boss", gives two different
answers — and both are true. Chapter 4 makes that concrete.

## Putting it together

```rust
let def: GameDef = serde_json::from_str(gamedef_json)?;
let plan = plan::compile(&def)?;          // compile ONCE

let mut scratch = plan.scratch();
let objectives = plan.evaluate(&build, &scenario, &mut scratch)?;
```

`plan::compile` is the boundary between "expensive, done once" and "cheap,
done constantly". `scratch` is the reusable buffer that lets `evaluate`
allocate nothing at all — you make one and hand it back on every call.

`evaluate` returns the objective values in the order `objectives` declared
them, so `objectives[0]` is `hit`.

## Run it

```
$ cargo run -p rtce --example guide_01_one_number
Guide chapter 1 — one number
  objectives: hit
  hit = 120.0000

  pin holds: 120 ✓
```

That `pin holds` line is a house rule, and it is worth explaining once
because every chapter has one. Every number this guide prints is also
**asserted** in the example that printed it, against a value derived by hand
from the JSON above rather than copied from the program's own output. A
documentation number that was merely true when someone wrote it is not
evidence of anything. Here the derivation is not much of a derivation —
`hit` is the expression `attack_power`, the build says 120, and no bucket,
event, or phase override exists that could modify it — but the discipline
starts now, because by chapter 6 the arithmetic is genuinely load-bearing.

## What we can't do yet

Everything interesting about an RPG's damage model is *modifiers*: +25%
damage from a passive, +50% crit damage from a ring, a multiplier from a
buff. Our archer has nowhere to put any of them, and the reason is that
"add up the percentages" is not one rule — real games have several, and
which one applies is exactly what makes a damage model worth modelling.

That is chapter 2.

---

[Next: Chapter 2 — Buckets and folds →](02-buckets-and-folds.md)
