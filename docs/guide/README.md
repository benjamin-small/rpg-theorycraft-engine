# The rtce guide

A progressive walkthrough: one small RPG, built up one concept at a time,
from a single number to a Monte Carlo distribution over a sixty-second
fight.

Every chapter has a **runnable companion** in
[`crates/rtce/examples/`](../../crates/rtce/examples/) that carries the
chapter's numbers as assertions. The chapters grow strictly by addition, so
diffing one chapter's example against the next shows exactly what the new
concept cost:

```bash
diff crates/rtce/examples/guide_05_a_rotation.rs crates/rtce/examples/guide_06_buffs_and_uptime.rs
```

## Chapters

| # | Chapter | What it adds | The number |
|---|---|---|---|
| 1 | [One number](01-one-number.md) | The three config tiers; compile once, evaluate many | 120 |
| 2 | [Buckets and folds](02-buckets-and-folds.md) | Modifiers, and the three ways games stack them | 186 |
| 3 | [Events and branches](03-events-and-branches.md) | Crits, branched stages, modifiers gated on firing | 213.9 |
| 4 | [Conditions and scenarios](04-conditions-and-scenarios.md) | An enemy, a debuff window, weighted fight phases | 229.71 / 175.584 |
| 5 | [A rotation](05-a-rotation.md) | Resources, actions, a priority list, a real timeline | 129.766 dps |
| 6 | [Buffs and uptime](06-buffs-and-uptime.md) | Buffs — and the uptime that computes itself | 133.30 dps |
| 7 | [Monte Carlo](07-monte-carlo.md) | Sampling, spread, and a hand-derivable hard bound | 112.84 … 181.04 |

Chapters 1–4 are the **calc tier** — closed-form averages, microseconds per
candidate. Chapters 5–7 are the **sequencing tier**, where uptimes stop
being asserted and start being computed. The tiers share one `GameDef`:
chapters 5, 6, and 7 add nothing to it at all.

## Running along

```bash
cargo run -p rtce --example guide_01_one_number
```

…and so on through `guide_07_monte_carlo`. Each prints its results and then
asserts them, so a clean exit means the chapter's arithmetic still holds.

## On the numbers

Every figure printed in this guide is asserted in the example that printed
it, against a value derived by hand from the JSON — not copied from the
program's own output. A documentation number that was merely true when
someone wrote it is not evidence of anything.

The JSON in these chapters is not a transcription either. Each fenced block
is checked byte-for-byte against the file in
[`crates/rtce/tests/fixtures/guide/`](../../crates/rtce/tests/fixtures/guide/)
that the example actually runs, by
[`crates/rtce/tests/guide.rs`](../../crates/rtce/tests/guide.rs). Prose and
code cannot drift apart here.

(The configs live inside the crate rather than next to the prose because
`cargo package` only ships files under the crate root — an example that
reads config from `docs/` builds from a git checkout and fails for anyone
installing from crates.io.)

## Scope

This is a made-up game, chosen so every number hand-derives in a few lines.
Where a config value is tuned for that reason rather than for realism — the
zeroed stamina regen in chapter 5, for instance — the chapter says so.

The guide stops after Monte Carlo. Damage over time, stacking buffs,
snapshotting, procs, and the configurable-semantics `defaults` block are all
covered by the worked examples listed at the end of
[chapter 7](07-monte-carlo.md#where-to-go-next).

For the engine's own reference documentation, see
[docs.rs/rtce](https://docs.rs/rtce). For the design rationale, see
[`docs/superpowers/specs/2026-07-21-rtce-design.md`](../superpowers/specs/2026-07-21-rtce-design.md).

---

[Start with chapter 1 →](01-one-number.md)
