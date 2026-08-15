# Test suite index

This virtual Cargo workspace keeps tests next to the crate whose public
boundary they exercise:

- `crates/rtce/src/**` contains focused unit tests for engine internals.
- `crates/rtce/tests/**` contains integration, fixture, parity, and guide-sync
  tests.
- `crates/rtce-cli/tests/**` covers the native command-line interface.
- `crates/rtce-runner/src/lib.rs` and `crates/rtce-testkit/src/lib.rs` contain
  their packages' unit tests.

See [`../TESTING.md`](../TESTING.md) for commands, scope, and the measured
coverage baseline.
