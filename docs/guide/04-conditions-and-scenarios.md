# Chapter 4 — Conditions and scenarios

> Runnable companion: [`examples/guide_04_conditions_and_scenarios.rs`](../../crates/rtce/examples/guide_04_conditions_and_scenarios.rs)
> · `cargo run -p rtce --example guide_04_conditions_and_scenarios`

Our archer does 213.9 damage. Against what?

So far the answer has been "nothing in particular", and that is the last gap
in the calc tier. Real theorycrafting questions are always *about a fight*:
this enemy has armor, that debuff is up half the time, the boss spends a
quarter of the encounter in a burn phase. This chapter gives the archer an
enemy, and gives `Scenario` its job.

## Conditions

A **condition** is a named state of the world that an expression can read
and a modifier can be gated on. Our archer gets one — `focused`, a "focus
fire" window — plus an `enemy_armor` stat and a stage to spend it:

```json title=04-gamedef.json
{
  "stats": ["attack_power", "crit_chance", "enemy_armor"],
  "conditions": ["focused"],
  "buckets": {
    "additive": { "fold": "sum" },
    "crit_damage": { "fold": "summed_group" }
  },
  "events": {
    "crit": { "chance": "crit_chance / 100", "factor": "crit_damage" }
  },
  "pipeline": [
    { "name": "base_hit", "expr": "attack_power" },
    {
      "name": "hit",
      "expr": "base_hit * (1 + additive / 100) * event_factors",
      "branched": true
    },
    { "name": "hit_after_armor", "expr": "hit * (1 - enemy_armor / 100)" }
  ],
  "objectives": ["hit_after_armor", "hit"]
}
```

Conditions and events look similar and are not the same thing:

| | Event | Condition |
|---|---|---|
| Resolves per | **hit** | **stretch of time** |
| Value | fired / didn't | an **uptime**, 0..1 |
| Comes from | `chance`, in the `GameDef` | the `Scenario` (or, from chapter 6, the sim) |
| Costs | a branch | nothing |

A crit is a coin flip on each hit, so it branches. "Is the focus window up?"
is a property of the fight you are asking about, so it is a fraction.

Note `objectives` now lists two stages. You will usually want more than one
number back — and stages you *don't* list still compute, they just aren't
handed over.

## Gating a modifier on a condition

```json title=04-build.json
{
  "stats": { "attack_power": 120.0, "crit_chance": 30.0 },
  "contributions": [
    { "bucket": "additive", "value": 30.0 },
    { "bucket": "additive", "value": 25.0 },
    { "bucket": "crit_damage", "value": 50.0 },
    { "bucket": "crit_damage", "value": 50.0, "condition": "focused" }
  ]
}
```

The archer has +50% crit damage always, and another +50% *while focused*.

Notice `enemy_armor` is declared in the `GameDef` but absent from the build.
That is fine — an unsupplied stat is zero — and it is the right place for it
to be missing, because armor is not a property of our archer. The scenario
supplies it.

## The fight

Two framings of the same character:

```json title=04-scenario-burst.json
{
  "phases": [
    {
      "name": "armor_break",
      "weight": 1,
      "uptimes": { "focused": 1.0 },
      "stats": { "enemy_armor": 5.0 }
    }
  ]
}
```

```json title=04-scenario-sustained.json
{
  "phases": [
    {
      "name": "average",
      "weight": 1,
      "uptimes": { "focused": 0.2 },
      "stats": { "enemy_armor": 20.0 }
    }
  ]
}
```

`uptimes` sets conditions; `stats` overrides stats for this phase only.

```
  scenario      focused  armor        hit  hit_after_armor
  burst            1.00      5   241.8000         229.7100
  sustained        0.20     20   219.4800         175.5840
```

**One build, two answers, both true.** This is the shape of every real
question the engine exists to answer — an optimizer prices a candidate
against several scenarios and collects the front, because the ring that wins
on the training dummy is often not the ring that wins on the boss.

## Where the numbers came from

```
burst — focused = 1.0, both crit_damage members fully in:
  crit_damage      = 1 + (50 + 1.0 × 50)/100 = 2.0
  hit              = 120 × 1.55 × (0.7 × 1.0 + 0.3 × 2.0) = 186 × 1.3  = 241.8
  hit_after_armor  = 241.8 × (1 - 5/100)                               = 229.71

sustained — focused = 0.2, the gated member counts for a fifth:
  crit_damage      = 1 + (50 + 0.2 × 50)/100 = 1.6
  hit              = 120 × 1.55 × (0.7 × 1.0 + 0.3 × 1.6) = 186 × 1.18 = 219.48
  hit_after_armor  = 219.48 × (1 - 20/100)                             = 175.584
```

Look closely at the `crit_damage` line in the sustained case. An uptime of
0.2 made the gated +50 count as +10 — the condition **scales the
contribution linearly**. It did not make the modifier fully present 20% of
the time; it made it 20% present all of the time.

For a bucket that folds `sum` or `summed_group`, those are the same thing,
which is why this is a perfectly good average. Hold onto the distinction
anyway. Chapter 6 is where it stops being free.

## Weighted phases

A `Scenario` is a *list*, and each phase carries a weight:

```json title=04-scenario-mixed.json
{
  "phases": [
    {
      "name": "armor_break",
      "weight": 1,
      "uptimes": { "focused": 1.0 },
      "stats": { "enemy_armor": 5.0 }
    },
    {
      "name": "average",
      "weight": 3,
      "uptimes": { "focused": 0.2 },
      "stats": { "enemy_armor": 20.0 }
    }
  ]
}
```

```
  mixed (1× armor_break + 3× average): hit_after_armor = 189.1155
```

Weights normalise, so this is `(1 × 229.71 + 3 × 175.584) / 4`. The example
asserts that identity against the two single-phase runs as well as against
the constant — so the pin fails if the engine ever stops normalising.

"A quarter of this fight is an armor-break window" is now one config object
rather than something the caller averages by hand.

## That's the calc tier

Four chapters in, the archer has stats, additive and multiplicative modifier
pools, a crit system with event-gated modifiers, an armor-bearing enemy, and
fights made of weighted phases. `Plan::evaluate` answers all of it in
microseconds, which is what makes it usable inside a search loop over
thousands of candidates.

> **Note for readers of older versions.** Chapters 1–4 replace what used to
> be `examples/your_own_game.rs`, which showed all three tiers at once. Its
> pins (148.20 / 113.28) are superseded by chapter 4's 229.71 / 175.584 —
> the game grew an `additive` pool in chapter 2 along the way.

## What we can't do yet

Look back at `"focused": 0.2`.

Where did 0.2 come from? We made it up. Someone has to sit down, work out
how often the archer's focus window is actually up given its cooldown and
duration and how the rotation actually plays out, and type the result into a
config file by hand. And then keep it correct as the build changes.

That is a real number about the game that we asserted instead of computed —
and every asserted uptime is a place the model can quietly be wrong.

Chapter 5 starts building the machine that computes it.

---

[← Chapter 3](03-events-and-branches.md) · [Next: Chapter 5 — A rotation →](05-a-rotation.md)
