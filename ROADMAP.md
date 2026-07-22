# Roadmap

## Proven
- P1 expression engine · P2 damage model · P3 D4-as-config · P4 parity
  proof (7 real builds identical through diablo4-calc's native engine and
  rtce — the standing numbers 8,096.02 … 6,769.10).

## Next: maturation (before any announcement)
- [ ] P4c — diablo4-calc switchover: delete calc.rs/stats.rs math, Breakdown
      shim over stage objectives, WASM/web re-verified at 8,096.02.
- [ ] explain(): per-stage traces (the Breakdown teaching path, spec'd).
- [ ] P5 — search module (Move, batch pricing, top-K/Pareto).
- [ ] rustdoc pass: every pub item documented, crate-level guide, deny(missing_docs).
- [ ] examples/: a runnable toy-game walkthrough + a "your own game in 30
      lines of JSON" example; README quickstart tested by CI.
- [ ] CI: GitHub Actions — test + clippy + fmt on push.
- [ ] Publish: GitHub repo, then crates.io (`rtce`, `rtce-testkit`) once the
      API survives the P4c switchover (semver honesty: 0.x until then).

## Explicitly out of scope
- Knowledge-graph construction (external drivers).
- Level-2 timeline simulation until Level-1 proves the shape (P6).
