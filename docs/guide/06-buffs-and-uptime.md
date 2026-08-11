# Chapter 6 — Buffs, and the uptime you no longer assert

> Runnable companion: [`examples/guide_06_buffs_and_uptime.rs`](../../crates/rtce/examples/guide_06_buffs_and_uptime.rs)
> · `cargo run -p rtce --example guide_06_buffs_and_uptime`

Back in chapter 4 we wrote `"focused": 0.2` and admitted we had made it up.
This chapter deletes it and makes the engine work it out — and then finds
that the answer is more interesting than a single number.

## One action and one buff

```json title=06-simdef.json
{
  "resources": {
    "stamina": { "max": "100", "regen_per_sec": "0" }
  },
  "actions": {
    "focus_fire": {
      "cast_time": "0",
      "cooldown": 10.0,
      "apply_buff": ["focus_window"]
    },
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
  "buffs": {
    "focus_window": { "duration": 2.5, "conditions": { "focused": 1.0 } }
  },
  "damage_objective": "hit_after_armor"
}
```

`focus_fire` is instant, has a ten-second cooldown, deals no damage, and
does one thing: applies `focus_window`. The buff lasts 2.5 seconds and
drives the `focused` condition to 1.0 while it is up.

That is the whole bridge between the two tiers. **A buff's `conditions` map
is what connects a timeline event to the damage model** — `focused` was
declared in chapter 4's `GameDef` and gated a `crit_damage` contribution
there, and none of that changes. The `GameDef` and `BuildState` files are
still byte-identical to chapter 4's.

It goes at the top of the priority list, since there is never a reason to
delay it:

```json title=06-rotation.json
{
  "rules": [
    { "action": "focus_fire" },
    { "action": "power_shot", "when": "stamina >= 40" },
    { "action": "quick_shot" }
  ]
}
```

No `when` on `focus_fire`. The automatic hard gate — off cooldown — already
says everything we mean.

## The result

```
  action        casts         damage    share
  focus_fire        6         0.0000    0.00%
  power_shot       31      5460.9600   68.28%
  quick_shot       29      2537.0400   31.72%
  total: 7998.0000 damage over 60s = 133.3000 dps
  focus_window buff uptime: 0.2500   focused condition uptime: 0.2500
```

**Nothing anywhere in the config says 0.25.** It falls out of a 2.5-second
window recast every ten seconds over a sixty-second fight. That is the
entire point of the sequencing tier: change the cooldown, change the fight
length, change the rotation so `focus_fire` sometimes gets delayed, and the
uptime re-derives itself.

Note also that `focus_fire` being instant (`cast_time: "0"`) means it never
consumes a decision slot — chapter 5's 31/29 alternation is untouched, which
makes this chapter a clean contrast against that one.

## Now look harder at 0.25

Here is the chapter's real lesson, and it is the reason this guide has a
chapter 6 rather than stopping at 5.

The example reconstructs, from the damage numbers alone, how many casts
*actually measured the buff as up*:

```
  the same window, measured two ways:
    time-weighted uptime         0.2500   6 windows × 2.5s / 60s
    cast-weighted uptime         0.2000   12 of 60 completions measured inside one
```

Those disagree, and not by rounding.

Casts complete on the integer-second grid. A window opening at `t=0` runs to
`t=2.5` and contains exactly **two** completions — at `t=1` and `t=2` — not
two and a half. You cannot buy half a hit. Each subsequent window behaves
identically:

```
s=0   completions inside [0, 2.5):  1,  2      (2)
s=10  completions inside [10, 12.5): 11, 12    (2)
s=20  …                                        (2)
```

Twelve of sixty, and 12/60 = 0.20.

### The `t=10` subtlety

That second line deserves scrutiny. Why isn't `t=10` itself inside the
window that opens at `t=10`?

Because two things complete at that instant: a damaging cast that started at
`t=9`, and `focus_fire` coming off cooldown. Coincident events resolve in
**scheduling order** by default, and the damaging cast was scheduled first —
so it measures its world *before* the buff lands. It counts as a buff-less
hit.

This is a documented sharp edge, not an accident. The `sim` module docs
cover it under **"A buff expiring on the cast grid"**, and rtce lets you move
it: the `defaults` block's `measure` chooses *when* a cast reads its world,
and `event_order` chooses which coincident event resolves first.

### The knob does not fix it

It is tempting to reach for `measure: "cast_start"` here and assume the
discrepancy goes away. Chapter 6's example runs that config too, so you can
see what it actually buys:

```diff
  {
+   "defaults": { "measure": "cast_start" },
    "resources": { "stamina": { "max": "100", "regen_per_sec": "0" } },
```

```
  the same fight under `measure: "cast_start"`:
    time-weighted uptime         0.2500   unchanged — a timeline fact, not a measurement one
    cast-weighted uptime         0.3000   now OVERSHOOTS 0.25 instead of undershooting
    dps                        134.4160   vs 133.3000 under the default
```

Measuring at cast *start*, a cast begun at `t` is inside `[s, s+2.5)` for
`t = s, s+1, s+2` — **three** per window, 18 of 60, a cast-weighted **0.30**.

So the knob did not reconcile anything. It moved the answer from 0.20 to
0.30, and **0.30 is exactly as wrong as 0.20, in the other direction.** The
example asserts that the two bracket the integrated 0.25 rather than
converging on it.

The integrated 0.25 is *unreachable*, and no configuration can reach it: a
2.5-second window cannot contain two and a half casts. `measure` and
`event_order` choose which way you are wrong about a boundary. They cannot
make hits divisible.

That is the useful form of this lesson. Not "there is a setting for it", but
"know which side of the boundary your model sits on, and why".

### Which number is right?

Both. They answer different questions.

- **0.25 is a true statement about seconds.** The condition really was
  active for a quarter of the fight. If you were asking "how much of this
  fight is the debuff up for the raid", it is the number you want.
- **0.20 is what the damage experienced.** Our archer measures its world
  once per cast, so what it *got* was 12 buffed hits out of 60.

The mistake is assuming they are the same, and the report only shows you the
first one. The example asserts both, in both directions — including an
explicit assertion that they *disagree* — precisely because a reader's first
instinct on seeing 0.2500 and 0.2000 in one output is that something is
broken.

### Why it matters in practice

Take the reported 0.25 and feed it back into chapter 4's calc tier, the way
you would if you were using the sim to source uptimes for a fast closed-form
sweep:

```
crit branch  crit_damage = 1 + (50 + 0.25 × 50)/100        = 1.625
power_shot   = 120 × 1.55 × (0.7 + 0.3 × 1.625) × 0.8      = 176.7
naive total  = 31 × 176.7 + 29 × 88.35                     = 8039.85
```

The sim says 7998.00. The round-trip is **41.85 too high** — about half a
percent — because it credits the window for 2.5 completions when it only
ever bought 2. Both of those numbers are pinned.

Half a percent is small. It is also *systematic*, it grows as your cast
times grow relative to your buff durations, and it is invisible in every
column the report prints. That combination — small, systematic, invisible —
is what makes it worth a chapter.

## Where the numbers came from

```
focused ACTIVE     crit branch crit_damage = 1 + (50+50)/100 = 2.0
  power_shot  120 × 1.55 × (0.7 + 0.3 × 2.0) × 0.8 = 193.44
  quick_shot                                       =  96.72
focused INACTIVE   crit branch crit_damage = 1 + 50/100 = 1.5
  power_shot                                       = 171.12
  quick_shot                                       =  85.56
```

The normal branch keeps the event-gated bucket at its ×1 identity in both
cases; the weighted formulas above apply the larger value only to the 30%
crit branch.

A completion at `t` started at `t-1`; `power_shot` runs at `t=0` and every
odd second, `quick_shot` at every even second from 2. Reading off the twelve
active completions:

```
active at    1, 2, 11, 12, 21, 22, 31, 32, 41, 42, 51, 52
started at   0, 1, 10, 11, 20, 21, 30, 31, 40, 41, 50, 51
  power_shot (started at 0 or an odd t):  7
  quick_shot (started at an even t ≥ 2):  5
  checks out: 7 + 24 = 31, 5 + 24 = 29 ✓

total = 7 × 193.44 + 24 × 171.12 + 5 × 96.72 + 24 × 85.56 = 7998.00
dps   = 7998.00 / 60                                       = 133.30
```

The 7-and-5 split is asserted directly, because it is the only pin in this
guide that depends on the coincident-event ordering described above. If that
ordering ever changed, this is the number that would notice — and the
`cast_start` contrast's 7-and-11 split is asserted for the same reason.

## What's left

We have a timeline that computes its own uptimes. But `sim::run` in EV mode
gives one deterministic answer — our archer "crits 30% of the time" by
having every hit be 30% of a crit.

Real fights are not averages. Chapter 7 asks what the *distribution* looks
like.

---

[← Chapter 5](05-a-rotation.md) · [Next: Chapter 7 — Monte Carlo →](07-monte-carlo.md)
