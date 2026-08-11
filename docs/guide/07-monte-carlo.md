# Chapter 7 — Monte Carlo

> Runnable companion: [`examples/guide_07_monte_carlo.rs`](../../crates/rtce/examples/guide_07_monte_carlo.rs)
> · `cargo run -p rtce --example guide_07_monte_carlo`

Chapter 6's archer does 133.30 dps. It has never once done 133.30 dps.

Every number in this guide so far has been an expected value. In EV mode our
archer does not crit 30% of the time — every hit is 30% of a crit, blended
by probability. That is exactly the right model for "is this ring better
than that one", and it is why the whole thing costs milliseconds.

It is the wrong model for "how often does this build fall short". For that
you need the shape, and the shape needs sampling.

## The change

```rust
run(&plan, &sim_plan, &build, &dummy, Mode::MonteCarlo { iterations: 1000, seed: 7 })
```

That is the entire diff. **No configuration changes at all** — chapter 7's
five config files are byte-identical to chapter 6's. The mode is a
parameter, not a different engine: the same decision loop drives both, and
only the treatment of probabilistic events differs.

The seed is explicit and required. Runs are reproducible.

## The result

```
Guide chapter 7 — Monte Carlo (60s training dummy)
  EV mode:  133.3000 dps  (chapter 6's answer)
  MC mode:  mean 133.1723   std 4.6130   p10 127.1000   p50 133.3000   p90 138.8800   (N=1000, seed=7)
```

Three things to read out of that line.

**The mean tracks EV.** 133.17 against 133.30 — the gap is sampling error,
and it shrinks as `iterations` grows. This is the agreement the engine is
built to guarantee: where the two modes overlap they must agree, and
`sim::exec`'s keystone test proves it exactly on a degenerate config.

**The cadence didn't move.** Still 6 / 31 / 29 casts, still `focused` at
0.25 uptime — both asserted. Nothing in this config makes the *sequence*
random: there are no procs, and stamina and cooldowns are deterministic. All
1000 iterations cast the same shots in the same order and differ only in
which hits crit. That is worth checking whenever you switch modes, because a
cadence that *does* move under sampling means something in your rotation is
reacting to chance.

**There is real spread.** std ≈ 4.6 dps, and p10 to p90 spans about 11.8.
The `poe2_*` examples make the opposite assertion — their configs sample
nothing, so they pin MC to reproduce EV with std *exactly* zero. Ours flips
a genuine coin on every hit, so this example asserts `std > 1.0`: a zero
here would mean the crit event had quietly stopped sampling.

## The hard bound

Tolerance bands are unsatisfying. A 2% window around the EV pin passes for
lots of subtly wrong models. So this chapter closes with a bound you can
derive on paper and that no sample may ever escape.

Start from an observation that is easy to miss: **in the no-crit branch,
the event-tagged `crit_damage` contributions are absent, so that
`summed_group` is its identity value of 1 — and the entire `focus_window`
buff drops out with them.** A hit that doesn't crit does not care whether the
archer was focused.

```
worst possible fight — nothing crits (buff irrelevant):
  power_shot  120 × 1.55 × 1.0 × 0.8 = 148.8
  quick_shot                         =  74.4
  31 × 148.8 + 29 × 74.4 = 6770.4  →  112.84 dps

best possible fight — everything crits, at chapter 6's 7/24 and 5/24 split:
  power_shot  active 297.6   inactive 223.2
  quick_shot  active 148.8   inactive 111.6
  7 × 297.6 + 24 × 223.2 + 5 × 148.8 + 24 × 111.6 = 10862.4  →  181.04 dps
```

```
  every sampled fight must land in [112.84, 181.04] dps:
    floor (nothing crits)   112.8400
    observed p10           127.1000
    EV / observed p50      133.3000
    observed p90           138.8800
    ceiling (all crit)     181.0400
```

Both endpoints are pinned, and so is the claim that the observed
percentiles and the EV answer all sit strictly inside them. Unlike the
2% band, this asserts something about the *model* rather than about the
seed — it would survive a change to the RNG and fail on a change to the
damage pipeline.

Notice how far inside the bound the actual distribution sits. The extremes
need all sixty coins to land the same way — 0.7⁶⁰ and 0.3⁶⁰, numbers with
tens of zeros after the decimal point. Sixty hits is already enough
averaging that the realistic outcomes cluster in a narrow band, which is the
practical argument for why EV mode is usually enough. Run a five-second
burst instead of a sixty-second fight and that stops being true, and MC
stops being optional.

## When to use which

| | Use it when |
|---|---|
| `Plan::evaluate` | Comparing many candidates. Uptimes are known or asserted. Microseconds each. |
| `sim::run` EV | You need uptimes, resource health, or cast counts *computed*. One answer. |
| `sim::run` MC | You need spread, percentiles, or the probability of falling short. N× the cost. |

They are three costs for one config family, not three engines. Anything you
learn about your `GameDef` in chapter 1 still holds in chapter 7.

## You're done

Seven chapters ago the archer was `attack_power = 120`. It now has:

- an additive damage pool and a `summed_group` crit-damage pool, with the
  fold rules chosen per bucket
- a crit event that branches the pipeline, and modifiers gated on it firing
- an armored enemy, a `focused` condition, and fights made of weighted phases
- a stamina economy, two shots and a cooldown'd buff, driven by a priority
  rotation over a sixty-second timeline
- an uptime that computes itself — and the knowledge that the reported
  version is not quite the one the damage felt
- a sampled distribution with a hand-derivable hard bound

None of it required writing any game logic in Rust. The `GameDef` grew from
five lines of JSON to about thirty, and that is the whole model.

## Where to go next

The guide stops here, but the engine doesn't. These are the worked examples
that pick up where chapter 7 leaves off, each with hand-derived pins of its
own:

| Example | What it adds |
|---|---|
| [`poe2_charges`](../../crates/rtce/examples/poe2_charges.rs) | Stacking buffs — `max_stacks`, `add_refresh_all`, `stacks.X` in a rotation gate |
| [`poe2_poison`](../../crates/rtce/examples/poe2_poison.rs) | Damage over time, independent stacks, and snapshotting |
| [`poe2_triggers`](../../crates/rtce/examples/poe2_triggers.rs) | Procs, trigger filters, and free casts |
| [`poe2_ignite`](../../crates/rtce/examples/poe2_ignite.rs) | "Strongest wins" reapplication, against `refresh` as a contrast |
| [`diablo4_basics`](../../crates/rtce/examples/diablo4_basics.rs) · [`diablo4_rotation`](../../crates/rtce/examples/diablo4_rotation.rs) | A real game's damage model, transcribed as config |

And two pieces of reading rather than running: the `defaults` block, which
configures the semantics chapter 6 ran into head-first (`measure`,
`event_order`, `proc_rolls`), and the `sim` module docs' **"A buff expiring
on the cast grid"** section, which is the general form of that same trap.

---

[← Chapter 6](06-buffs-and-uptime.md) · [Back to the index](README.md)
