# rtce-testkit

Golden-fixture test harness for `rtce` and its consumer games. Depend on
it as a dev-dependency.

It carries no test logic of its own — just the house rules an
`rtce`-based game's test suite is expected to share, inherited from
`d4-theory-crafting`'s M1 milestone:

- **Golden fixtures live in JSON files, one case per file.**
  `for_each_fixture` walks a directory, parses each `*.json` as a
  `serde_json::Value`, and hands it to your callback in sorted-by-name
  order (deterministic test output, deterministic diffs).
- **An empty suite is a bug, not a pass.** If the fixture directory is
  missing or yields zero `*.json` files, `for_each_fixture` panics
  instead of silently iterating zero times — the empty-glob rule.
- **Every fixture is provenance-tagged.** Each JSON file must carry a
  `name` and a `source` key (where the expected number came from — a
  hand-worked calculation, an in-game screenshot, another calculator)
  so a reviewer can trace any pinned number back to its origin.
- **Comparisons are relative-tolerance, not exact-float.** `assert_close`
  compares `actual` against `expected` within a relative tolerance,
  because floating-point pipelines rarely reproduce bit-for-bit across
  platforms/optimization levels.

## License

Licensed under either of MIT OR Apache-2.0 at your option. License texts
ship with this crate (`LICENSE-MIT`, `LICENSE-APACHE`); canonical copies
are in the repository root at
https://github.com/benjamin-small/rpg-theorycraft-engine.
