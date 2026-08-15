# Testing

The test suite covers the Rust engine, its JSON-facing interfaces, and the
configuration examples used by the documentation. Run the repository checks
from the workspace root:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features
cargo test --all-features
```

CI additionally runs every documented Rust example, exercises the native CLI,
and builds the Rust/Wasm plus TypeScript browser tutorial.

## Coverage baseline

Measured on 2026-08-15 at commit `8297934` with Rust's source-based coverage:

| Metric | Coverage |
| --- | ---: |
| Lines | 96.35% |
| Regions | 96.50% |
| Functions | 93.15% |

Reproduce the measurement with:

```sh
cargo install cargo-llvm-cov --locked
rustup component add llvm-tools-preview
cargo llvm-cov --workspace --all-features --summary-only
```

The baseline includes all native Rust workspace targets. The Wasm adapter is
compiled but its exported browser calls are not executed by the native
coverage run, so its lines correctly appear uncovered in that report.

## What the suite exercises

- expression lexing, parsing, compilation, evaluation, and error positions;
- bucket folds, event branches, scenarios, objective traces, and search;
- timeline resources, rotations, buffs, stacks, procs, snapshotting, event
  ordering, expected-value mode, seeded Monte Carlo, and RNG invariants;
- fail-closed JSON validation and the underscore annotation namespace;
- pinned Diablo 4, Path of Exile 2, toy-game, and tutorial fixture results;
- JSON runner envelopes and native CLI success and failure paths;
- synchronization between guide Markdown, committed JSON, and Rust examples;
- a production build of the TypeScript/Wasm browser tutorial.

The suite does not claim end-to-end browser interaction coverage. Visual and
terminal interactions are manually smoke-tested against the local Vite server
after UI changes.
