# Chapter 3 — Events and branches

> Runnable companion: [`examples/guide_03_events_and_branches.rs`](../../crates/rtce/examples/guide_03_events_and_branches.rs)
> · `cargo run -p rtce --example guide_03_events_and_branches`

Our archer hits for exactly 186, forever. Let's add crits.

A crit is not a modifier. It is an **event**: it has a probability, it has a
magnitude, and — the part that makes it structurally different — some
modifiers only exist *when it fires*. "+20% damage on critical strikes" is
not a +20% damage modifier that you can fold into the pool once and forget.

## Declaring the event

```json title=03-gamedef.json
{
  "stats": ["attack_power", "crit_chance"],
  "buckets": {
    "additive": { "fold": "sum" },
    "crit_damage": { "fold": "summed_group" }
  },
  "events": {
    "crit": {
      "_guide": "Chance selects the crit branch. Factor is shown in branch traces and powers optional event_multiplier; this lesson's hit formula reads crit_damage directly.",
      "chance": "crit_chance / 100",
      "factor": "crit_damage"
    }
  },
  "pipeline": [
    { "name": "base_hit", "expr": "attack_power" },
    {
      "name": "hit",
      "_guide": "crit_damage is a declared bucket: x1.0 normally, x1.5 when crit-tagged gear turns on.",
      "expr": "base_hit * (1 + additive / 100) * crit_damage",
      "branched": true
    }
  ],
  "objectives": ["hit"]
}
```

Four things are new.

**The `events` block.** An event is two expressions: a `chance` (clamped to
0..1 by the engine) and a `factor` it contributes when it fires. Both are
ordinary expressions over the same flat namespace as everything else — our
`chance` reads a stat, our `factor` reads a bucket.

**A `crit_damage` bucket, folding `summed_group`.** Crit damage modifiers
are the classic same-type-stacks-additively case: +50% and +50% crit damage
is +100%, not ×2.25. Note it uses a *different* fold from `additive`, in the
same game. That is the point of buckets.

The build tags that gear bonus with the declared `crit` event:

```json title=03-build.json
{
  "_guide": "Gear labels are notes for players. Within each contribution, bucket, value, event, and condition drive the math.",
  "stats": { "attack_power": 120.0, "crit_chance": 30.0 },
  "contributions": [
    { "_source": "Stormstring Bow · Serrated Edge affix", "bucket": "additive", "value": 30.0 },
    { "_source": "Trailseeker Gloves · Hunter's Tempo affix", "bucket": "additive", "value": 25.0 },
    { "_source": "Eagle Eye Amulet · Deadly Precision affix", "bucket": "crit_damage", "value": 50.0, "event": "crit" }
  ]
}
```

**`"branched": true` on the `hit` stage.** This says “evaluate me once per
combination of events fired/not-fired, refolding event-tagged bucket members
for each branch, then store the probability-weighted average.” With one event
that is two branches; with two events it would be four.

**There is no injected name in the formula.** `crit_damage` was declared in
`buckets`, and the amulet contribution names the declared `crit` event. An
empty `summed_group` is ×1.0, so the normal branch gets the identity. The crit
branch turns on the amulet and refolds the bucket to ×1.5. The generic
`event_multiplier` builtin remains available as an advanced shortcut for
event factors that are not naturally represented by gated buckets; the
tutorial does not need it.

**The event's `factor` field is still explicit.** It points at the same
declared `crit_damage` bucket, so the branch trace can report ×1.5 and the
optional `event_multiplier` shortcut would produce the same multiplier. This
lesson's stage expression does not consume that shortcut: the contribution's
`"event": "crit"` gate and the raw `crit_damage` bucket do the actual math.

**The pipeline also split in two.** `base_hit` is now just `attack_power`,
and the bucket wrap moved *into* the branched stage. That reshuffle looks
cosmetic and is not — see "Why the split matters" below.

## The branch table

`Plan::explain` runs the identical engine with tracing on. It is how you
check the engine's work — and it costs the hot path nothing, because
`evaluate` never takes that path.

```
  branch table (stage `hit`):
    fired      weight      trace factor      value
    —           0.700           1.000    186.000
    crit        0.300           1.500    279.000
```

Everything in chapter 3's answer is in that table:

- `crit_damage` is ×1.0 with its crit-tagged contribution off, then folds to
  `1 + 50/100` = **1.5** in the crit branch.
- The weights are `crit_chance / 100` = 0.3 and its complement 0.7.
- `120 × 1.55 × 1.0` = 186 and `120 × 1.55 × 1.5` = 279.
- Blended: `0.7 × 186 + 0.3 × 279` = **213.9**.

The example asserts that blending the branch table by weight reproduces
`evaluate`'s number exactly — so the trace is provably the same computation,
not a re-derivation that might disagree.

## Why the split matters

Now the contrast. Take the same gamedef and add one contribution:

```json title=03-build-oncrit.json
{
  "_guide": "Gear labels are notes for players. The event tag turns a contribution on only in that branch.",
  "stats": { "attack_power": 120.0, "crit_chance": 30.0 },
  "contributions": [
    { "_source": "Stormstring Bow · Serrated Edge affix", "bucket": "additive", "value": 30.0 },
    { "_source": "Trailseeker Gloves · Hunter's Tempo affix", "bucket": "additive", "value": 25.0 },
    { "_source": "Eagle Eye Amulet · Deadly Precision affix", "bucket": "crit_damage", "value": 50.0, "event": "crit" },
    { "_source": "Bullseye Quiver · Critical Ambush affix", "bucket": "additive", "value": 20.0, "event": "crit" }
  ]
}
```

That last line is a modifier tagged with an event. It reads "+20% damage,
but only on critical strikes", and it behaves accordingly:

```
  with `+20 additive, event: crit`: hit = 224.7000
```

```
no crit  weight 0.7   additive 55   120 × 1.55 × 1.0 = 186
crit     weight 0.3   additive 75   120 × 1.75 × 1.5 = 315
                                    0.7×186 + 0.3×315 = 224.7
```

**The no-crit branch is still exactly 186.** The example asserts that
specifically, because it is the entire claim: the gated modifier is
genuinely absent from the branch where its event did not fire.

And this is why `additive` had to move inside the branched stage. A bucket
containing event-gated members does not have *one* value — it has a
different value per branch. If `hit` had kept chapter 2's shape, with
`(1 + additive / 100)` folded once in an unbranched `base_hit`, there would
be nowhere for that +20 to be conditionally absent. Buckets read inside a
branched stage are re-folded per branch; buckets read outside one are not.

That is the rule to remember: **event-gated contributions are only
meaningful to expressions inside a `branched` stage.**

## Expected value, and what it hides

`hit = 213.9` is an *expected value*. Our archer never actually hits for
213.9 — it hits for 186 or 279. For comparing two candidate builds, the
average is exactly what you want, and it is why this whole tier costs
microseconds.

But it is an average, and averages hide things: burst thresholds, streaks,
the shape of the distribution. Chapter 7 gets that shape back with Monte
Carlo, over a real timeline. Everything between here and there is about
making the average itself more honest.

## What we can't do yet

Our archer's 213.9 is the same number against every enemy in the game. There
is no armor, no debuff window, no notion of a *fight* — chapter 1's
`Scenario` is still an empty ceremony.

That is chapter 4, and it is where the calc tier is finished.

---

[← Chapter 2](02-buckets-and-folds.md) · [Next: Chapter 4 — Conditions and scenarios →](04-conditions-and-scenarios.md)
