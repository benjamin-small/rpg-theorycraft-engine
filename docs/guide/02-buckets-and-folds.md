# Chapter 2 — Buckets and folds

> Runnable companion: [`examples/guide_02_buckets_and_folds.rs`](../../crates/rtce/examples/guide_02_buckets_and_folds.rs)
> · `cargo run -p rtce --example guide_02_buckets_and_folds`

Our archer has 120 attack power and no way to improve. Time for modifiers.

Here is the question that makes damage models interesting. A character has
two sources of +damage: a passive worth +30% and a ring worth +25%. Is that
character doing **+55%** damage, or **×1.30 × 1.25 = +62.5%**?

Both answers ship in real games. Often in the *same* game, for different
categories of modifier — that split is the core of Path of Exile's
`increased` vs `more`, and of Diablo 4's single additive pool vs its
separate multiplier groups. So `rtce` does not pick one. You declare
**buckets**, and each bucket declares its **fold rule**.

## Adding a bucket

```json title=02-gamedef.json
{
  "stats": ["attack_power"],
  "buckets": {
    "additive": { "fold": "sum" }
  },
  "pipeline": [
    { "name": "hit", "expr": "attack_power * (1 + additive / 100)" }
  ],
  "objectives": ["hit"]
}
```

Two things changed. There is a `buckets` block declaring one bucket named
`additive`, and the `hit` expression now reads that bucket by name.

That is the trick worth noticing: **a bucket is just another name a pipeline
expression can read.** Stats, buckets, conditions, and earlier stages all
live in one flat namespace. `additive` in an expression means "whatever this
build's contributions to the `additive` bucket folded to".

## Contributing to it

```json title=02-build.json
{
  "_guide": "Gear labels are notes for players. Within each contribution, only bucket and value change this lesson's math.",
  "stats": { "attack_power": 120.0 },
  "contributions": [
    { "_source": "Stormstring Bow · Serrated Edge affix", "bucket": "additive", "value": 30.0 },
    { "_source": "Trailseeker Gloves · Hunter's Tempo affix", "bucket": "additive", "value": 25.0 }
  ]
}
```

A **contribution** is one modifier: a value and the bucket it lands in. One
line per passive, per gear affix, per skill point.

Read the first line like a gamer: the **Serrated Edge affix on the
Stormstring Bow grants +30% damage**, and that bonus goes into the shared
`additive` bucket. The gloves add another +25% to the same pool, so the gear
screen's two bonuses become +55% before the hit formula uses them. Keys that
begin with `_`, such as `_source` and `_guide`, are human-readable notes. The
engine deliberately ignores them, so they can explain a config without
changing its result.

This list is the part of the character that actually changes. Swapping a
ring edits one entry here; an optimizer comparing ten thousand gear
combinations is varying this list and nothing else, against the same
`GameDef` every time.

Contributions can also carry an `event` or a `condition` tag, which is how a
modifier becomes conditional. Chapters 3 and 4.

## The three folds

| `fold` | Formula | Means |
|---|---|---|
| `sum` | `Σv` | Raw total. The pipeline applies its own wrap. |
| `summed_group` | `1 + Σv/100` | Same-type multipliers **sum**, then multiply once. |
| `product` | `Π(1 + v/100)` | Each modifier is its **own independent** factor. |

The example runs all three against the same two contributions:

```
  the same +30 and +25, through each fold:
    fold                 bucket        hit
    sum                 55.0000   186.0000
    summed_group         1.5500   186.0000
    product              1.6250   195.0000
```

Read that table carefully, because it contains the chapter's real lesson.

**`sum` and `summed_group` produce the same answer.** They are the same
rule; they differ only in *who* applies the `1 + x/100` wrap. `sum` hands
back a raw 55 and the pipeline writes `1 + additive / 100` itself;
`summed_group` hands back 1.55 ready to multiply. Use `sum` when several
different stages need to wrap the same pool differently — that is why our
archer's `additive` is a `sum` — and `summed_group` when the bucket only
ever means one multiplier.

**`product` does not agree, and the gap is not an error.** 195 − 186 = 9,
which is exactly `0.30 × 0.25 × 120` — the cross term that additive stacking
throws away and independent stacking keeps. That 9 is asserted in the
example, not just observed, because it is the whole distinction: *additive
modifiers dilute each other; independent ones compound.* Any real
theorycrafting question of the form "is this ring better than that one?"
depends on which bucket the ring's modifier lands in, and it is not
answerable without knowing.

## Where the number came from

```
additive folds `sum`   →  Σv = 30 + 25 = 55
pipeline wraps it      →  hit = 120 × (1 + 55/100) = 120 × 1.55 = 186
```

That is chapter 2's pin: **186**. Chapters 3 through 7 all build on it, so
it is worth being sure of.

## What we can't do yet

Our archer hits for exactly 186 every single time. No game works that way —
sometimes you crit. A crit is not a modifier, it is an *event*: it has a
probability, it has a magnitude, and crucially some modifiers only apply
*when it fires*.

That is chapter 3.

---

[← Chapter 1](01-one-number.md) · [Next: Chapter 3 — Events and branches →](03-events-and-branches.md)
