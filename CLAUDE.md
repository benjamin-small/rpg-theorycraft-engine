# rpg-theorycraft-engine

Generic config-driven theorycrafting engine (crates `rtce`, `rtce-testkit`).
See `docs/superpowers/specs/2026-07-21-rtce-design.md` (Done-since = ground
truth) and `docs/superpowers/plans/` for the active plan.

## Commands

    cargo test --workspace     # the whole gate — must be green to commit

## Conventions (inherited from ../diablo4-calc — non-negotiable)

- Small verified slices; every commit carries a hand-checked number where
  one exists (P1 handshake: base_hit 8,573.0184).
- TDD red-first: stub → watch the test fail for the right reason →
  implement → green. Pinned numbers get mutation-checked instead.
- Zero allocation on evaluation hot paths; compilation may be expensive.
- Consumers: diablo4-calc first (its parity suite gates migration);
  knowledge-graph drivers are OUT OF SCOPE — we only price candidates.
