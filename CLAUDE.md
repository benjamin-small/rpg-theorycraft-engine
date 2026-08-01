# rpg-theorycraft-engine

Generic config-driven theorycrafting engine (crates `rtce`, `rtce-testkit`).
See `docs/superpowers/specs/2026-07-21-rtce-design.md` (Done-since = ground
truth) and `docs/superpowers/plans/` for the active plan.

## Commands

    cargo test --workspace     # the whole gate — must be green to commit

## Consumers

Both consume `rtce` as a PATH dependency (`../rpg-theorycraft-engine`), so
a change here reaches them immediately — and both were renamed and
restructured in 2026-08:

| now | was | consumes |
|---|---|---|
| `../d4-theory-crafting` | `diablo4-calc` | `plan` (production: its native math is deleted) |
| `../poe2-theory-crafting` | `poe2-calcs` | `plan` + `sim` + `search` (parity proof; its `calc.rs` stays normative) |

Older entries in CHANGELOG and `docs/superpowers/` use the previous names
and are left alone — they record what was true when written.

## Conventions (inherited from d4-theory-crafting — non-negotiable)

- Small verified slices; every commit carries a hand-checked number where
  one exists (P1 handshake: base_hit 8,573.0184).
- TDD red-first: stub → watch the test fail for the right reason →
  implement → green. Pinned numbers get mutation-checked instead (break
  the input or the code path, watch the SPECIFIC pin fail, restore,
  report the contrast).
- Zero allocation on evaluation hot paths; compilation may be expensive.
- Consumers: d4-theory-crafting first (its parity suite gates migration);
  knowledge-graph drivers are OUT OF SCOPE here — we only price
  candidates. (`poe2-search` is such a driver and lives in the consumer
  repo, which is exactly where this line says it belongs.)
- Fail closed, with positioned errors. Never guess at a config's intent.

## Docs discipline (P8f — binding on every phase)

The P7 record — a surviving mutation on documented-but-unpinned semantics
in five consecutive tasks — is the named risk these three rules answer.
Docs are a deliverable with the same gate as code:

1. **Every config field's rustdoc states its DEFAULT, its EVALUATION
   INSTANT, and its INTERACTIONS** with other fields. (The models:
   `simdef::Measure` / `EventOrder` / `ProcRolls`, and `NumOrExpr`'s
   instants table.)
2. **Every doc claim carrying a NUMBER ships with a contrast-run pin** —
   an asserted run somewhere in tests or CI-run examples, not a figure
   that was true when written.
3. **Every shipped `(default × override)` cell gets a discriminating
   test.** Configurability multiplies the semantics matrix; a cell no
   test can tell apart from its neighbor is where the next silent wrong
   answer lives. A cell that is deliberately NOT pinned is listed as
   open debt (ROADMAP), never left implicit.

The release-staging commit re-sweeps ROADMAP's version pointers: cutting
CHANGELOG's Unreleased into `[X.Y.Z]` stales every "(Unreleased)"
cross-reference in the same commit (the 0.4.0 staging proved it).

## Standing gates (all clean, per task)

- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt
  --all --check` · `#![warn(missing_docs)]` · `cargo doc` zero warnings
- Both consumers re-verified (each from its own repo root):
  - `../d4-theory-crafting` — `cargo test --workspace` green, and
    `cargo run -q -p d4-core --example eval` (no args) golden
    `17574.299999999996`.
  - `../poe2-theory-crafting` — `cargo test --workspace` green, and
    `cargo test -p poe2-core --test rtce_parity` → 63 passed.
- `examples/diablo4_rotation.rs` EV **and** MC blocks byte-identical
  under serde defaults (the standing proof no RNG draw or event
  reordering leaked into the default path).
- Every behavior change mutation-proven; releases stage with
  `cargo publish -p rtce --dry-run` clean.
