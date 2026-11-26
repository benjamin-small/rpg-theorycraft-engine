# Slice 1 — `on_non_crit` trigger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `Trigger::OnNonCrit` (`"on_non_crit"`), the complement of `on_crit`, so a proc can open a "dealt a Non-Critical Hit Recently" window (#33 item 5).

**Architecture:** One new enum variant, rolled in `Sim::complete_cast` right after `on_crit` in both modes: Monte Carlo qualifies it with `!is_crit` from the SAME sampled mask `on_crit` uses; EV weights it by `1 − crit_chance`. All proc machinery (`rolls`, `icd`, `actions` filter, ICD-at-one-instant) is shared and unchanged. No RNG draw happens for a config without an `on_non_crit` proc, so every existing number is byte-identical.

**Tech Stack:** Rust, serde, the `rtce` crate (`crates/rtce`).

**Spec:** `docs/superpowers/specs/2026-11-25-hit-level-casts-design.md` §5.7 (first bullet) and §6 slice 1.

## Global Constraints

- Work on branch `feat/on-non-crit`, cut from `design/hit-level-casts` (it carries the approved spec). Never stage `README.md` / `ROADMAP.md` (unrelated local edits).
- TDD red-first: write the test, watch it fail for the RIGHT reason, implement, green. Pinned numbers are mutation-checked: break the code path, watch the specific pin fail, restore, record the contrast in the commit message.
- Zero allocation on the run loop.
- P8f docs discipline: the new variant's rustdoc states its DEFAULT behavior, its EVALUATION INSTANT (where it rolls in the cast-complete order), and its INTERACTIONS (`rolls`, `icd`, `actions`, the no-`"crit"`-event fallback, utility and free casts).
- Standing gates, all clean, before the PR: `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all --check` · `cargo doc` zero warnings · `examples/diablo4_rotation.rs` EV and MC blocks byte-identical to `main` · `../d4-theory-crafting`: `cargo test --workspace` green and `cargo run -q -p d4-core --example eval` prints `17574.299999999996` · `../poe2-theory-crafting`: `cargo test --workspace` green and `cargo test -p poe2-core --test rtce_parity` → `63 passed`.
- `Trigger` is NOT `#[non_exhaustive]`; adding a variant is a 0.x minor-level break for exhaustive downstream `match`es. Neither consumer matches on `Trigger` (verified 2026-11-25). Note it in CHANGELOG; do not add `#[non_exhaustive]` in this slice.

## Review Focus

1. **No `"crit"` event in the plan** — every damaging hit is a non-crit: EV weight `1.0`, MC always qualifies. (Test in Task 1 and Task 2.)
2. **`rolls: "per_hit"` in MC** — hits still share ONE mask in this slice (per-hit sampling is slice 3), so a 5-hit cast presents 5 non-crit rolls or 0. (Test in Task 2.)
3. **Utility casts** (no `damage`) present no `on_non_crit` roll, in either mode. (Test in Task 1.)
4. **`actions` filter** applies to `on_non_crit` like every other trigger. (Test in Task 1.)
5. **Byte-identity** of every config without an `on_non_crit` proc — no extra RNG draw. (Gate: `diablo4_rotation` + full suite, Task 3.)

---

### Task 1: `Trigger::OnNonCrit` — parse + Expected-mode rolling

**Files:**
- Modify: `crates/rtce/src/simdef.rs` (enum `Trigger`, ~line 1181; `ProcRolls` scope docs ~line 314)
- Modify: `crates/rtce/src/sim/exec.rs` (`complete_cast` EV branch ~line 2450; the `roll_procs_ev` weight doc ~line 2686)
- Modify: `crates/rtce/src/sim/mod.rs` (the "cast-complete order" diagram and its trigger list)
- Test: `crates/rtce/src/simdef.rs` tests module; `crates/rtce/src/sim/exec.rs` tests module

**Interfaces:**
- Produces: `Trigger::OnNonCrit`, serde name `"on_non_crit"`. Task 2 uses it in the MC path.

- [ ] **Step 1: Write the failing tests**

In `simdef.rs` tests:
```rust
#[test]
fn on_non_crit_trigger_parses() {
    let t: Trigger = serde_json::from_str(r#""on_non_crit""#).unwrap();
    assert_eq!(t, Trigger::OnNonCrit);
    assert_eq!(serde_json::to_string(&t).unwrap(), r#""on_non_crit""#);
}
```

In `exec.rs` tests, next to `ev_on_crit_weights_by_crit_probability` (reuse its fixture shape: `toy_plan()`, crit 0.25, one 1 s `spam` action, 10 s phase, proc `chance: "1"`, `icd: 0.0`, `rolls: None`):
```rust
// weight 1 − 0.25 = 0.75 per hit (exact in binary); acc crosses at hits
// 2,3,4,6,7,8,10 → 7 fires. on_crit on the same fixture gives 2.
#[test]
fn ev_on_non_crit_weights_by_one_minus_crit_probability() {
    // … same fixture with trigger: Trigger::OnNonCrit …
    assert_eq!(ev.proc_counts["non_crit_proc"], 7);
}

// scoped_plan(): no "crit" event → crit_chance 0 → weight 1.0 → 10 fires
#[test]
fn ev_on_non_crit_fires_every_hit_without_a_crit_event() { /* == 10 */ }

// utility action (damage: None), 10 casts, on_non_crit chance 1 → 0 fires
#[test]
fn a_utility_cast_presents_no_on_non_crit_roll() { /* == 0, EV and MC(iterations 1) */ }

// scoped_plan(), two actions alternating (alternating_rotation shape),
// actions: Some(vec!["a"]) → 10 of 20; None → 20
#[test]
fn the_actions_filter_applies_to_on_non_crit() { /* 20 / 10 */ }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rtce --lib on_non_crit`
Expected: compile error `no variant named OnNonCrit` (the right reason). After adding only the variant (Step 3a), the EV tests must fail with `left: 0, right: 7` / `right: 10` — the trigger exists but is never rolled.

- [ ] **Step 3: Implement**

a. Add `OnNonCrit` to `Trigger` after `OnCrit`, with rustdoc covering: rolls exactly where `OnCrit` does (after it, in the same proc batch); EV weight `1 − crit_chance`; MC qualifies on the cast's sampled mask having the crit bit CLEAR (same draw as `OnCrit` — the two are exact complements per cast); `rolls`/`icd`/`actions` identical to `OnCrit`; no `"crit"` event ⇒ every damaging hit qualifies; utility casts and proc-fired free casts present no roll.
b. In `complete_cast`'s EV branch, after the `OnCrit` call: `self.roll_procs_ev(Trigger::OnNonCrit, 1.0 - crit_chance, action, Some(d))?;`
c. Update the docs that enumerate triggers: `sim/mod.rs` diagram becomes `proc rolls: on_cast, then on_hit, then on_crit, then on_non_crit`; `ProcRolls` scope section lists `OnNonCrit` among the hit-driven triggers; the `roll_procs_ev` weight doc adds `1 − crit_chance` for `OnNonCrit`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rtce --lib on_non_crit` → the EV tests PASS (the utility test's MC half may still pass vacuously; Task 2 adds the MC-specific pins).
Mutation check: change `1.0 - crit_chance` to `crit_chance` → `ev_on_non_crit_weights_by_one_minus_crit_probability` fails `left: 2, right: 7`; restore.

- [ ] **Step 5: Commit**

```bash
git add crates/rtce/src/simdef.rs crates/rtce/src/sim/exec.rs crates/rtce/src/sim/mod.rs
git commit -m "Add on_non_crit trigger (EV: weight 1 − crit_chance; 7 fires at p=0.25, contrast 2 under crit_chance)"
```

### Task 2: Monte Carlo rolling + CHANGELOG

**Files:**
- Modify: `crates/rtce/src/sim/exec.rs` (`complete_cast` MC branch ~line 2446; the `roll_procs_mc` qualifies doc ~line 2878)
- Modify: `CHANGELOG.md` (`## [Unreleased]`)
- Test: `crates/rtce/src/sim/exec.rs` tests module

**Interfaces:**
- Consumes: `Trigger::OnNonCrit` (Task 1).

- [ ] **Step 1: Write the failing tests**

```rust
// toy_plan (p=0.25), one proc on_crit + one proc on_non_crit, both chance 1,
// icd 0, MonteCarlo { iterations: 1, seed: 42 }: exactly one fires per cast.
#[test]
fn mc_on_crit_and_on_non_crit_are_exact_complements_per_cast() {
    assert_eq!(mc.proc_counts["crit_proc"] + mc.proc_counts["non_crit_proc"], 10);
    assert!(mc.proc_counts["non_crit_proc"] > 0 && mc.proc_counts["crit_proc"] > 0,
        "seed 42 must exercise both branches — pick another seed if not, and say so");
}

// scoped_plan (no crit event), MC iterations 1: 10 fires.
#[test]
fn mc_on_non_crit_fires_every_hit_without_a_crit_event() { /* == 10 */ }

// toy_plan, action damage.stats hits_per_use 5, proc rolls: Some(PerHit),
// chance 1, icd 0, 10 casts, MC iterations 1: count is a multiple of 5
// (one shared mask per cast — per-hit sampling is slice 3), and EV gives
// 10 × 5 × 0.75 = 37.5 → 37 fires.
#[test]
fn per_hit_on_non_crit_shares_the_cast_mask_in_mc() {
    assert_eq!(mc.proc_counts["non_crit_proc"] % 5, 0);
    assert_eq!(ev.proc_counts["non_crit_proc"], 37);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rtce --lib on_non_crit`
Expected: the three MC tests FAIL with a count of `0` for `non_crit_proc` (never rolled in MC); the EV half of the per-hit test passes already.

- [ ] **Step 3: Implement**

In `complete_cast`'s MC branch, after the `OnCrit` call: `self.roll_procs_mc(Trigger::OnNonCrit, !is_crit, action, Some(d))?;`. Update the `roll_procs_mc` `qualifies` doc to name `!is_crit` for `OnNonCrit`.

CHANGELOG `## [Unreleased]` → `### Added`: **`on_non_crit` proc trigger** (#33) — complement of `on_crit`; EV weight `1 − crit_chance`, MC rolls when the cast's sampled crit bit is clear; same `rolls`/`icd`/`actions` semantics; no `"crit"` event ⇒ every hit qualifies. `### Upgrade notes`: `Trigger` gained a variant — an exhaustive downstream `match` needs an arm. JSON and every existing config are unchanged.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rtce --lib on_non_crit` → all PASS.
Mutation check: change `!is_crit` to `is_crit` → `mc_on_crit_and_on_non_crit_are_exact_complements_per_cast` fails (sum `2 × crit count`, not 10) and `mc_on_non_crit_fires_every_hit_without_a_crit_event` fails `left: 0, right: 10`; restore.

- [ ] **Step 5: Commit**

```bash
git add crates/rtce/src/sim/exec.rs CHANGELOG.md
git commit -m "Roll on_non_crit in Monte Carlo from the on_crit draw (complements sum to 10/10 casts)"
```

### Task 3: Standing gates, consumers, PR

**Files:** none new (fix whatever a gate reports, in the task that owns it).

- [ ] **Step 1: Byte-identity proof**

Run: `git worktree add ../rtce-main main`, then `cargo run -q -p rtce --example diablo4_rotation > target/d4rot-branch.txt` here and the same command in `../rtce-main` into `target/d4rot-main.txt`; `diff target/d4rot-branch.txt ../rtce-main/target/d4rot-main.txt`. Expected: no output. Then `git worktree remove ../rtce-main`.

- [ ] **Step 2: Engine gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check && RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`
Expected: all clean.

- [ ] **Step 3: Consumers** (each from its own root)

`../d4-theory-crafting`: `cargo test --workspace` green; `cargo run -q -p d4-core --example eval` → `17574.299999999996`.
`../poe2-theory-crafting`: `cargo test --workspace` green; `cargo test -p poe2-core --test rtce_parity` → `63 passed`.

- [ ] **Step 4: Commit the plan, push, open the PR**

```bash
git add docs/superpowers/plans/2026-11-25-slice1-on-non-crit.md
git commit -m "Plan: slice 1 (on_non_crit)"
git push -u origin feat/on-non-crit
gh pr create --title "Add on_non_crit proc trigger (#33 item 5)" --body "<summary, pins and mutation contrasts from the commits, gate results; 'Part of #33' — not 'Closes'>"
```
