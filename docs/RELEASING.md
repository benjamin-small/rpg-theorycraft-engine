# Releasing `rtce` and `rtce-testkit`

Releases are tag-driven. Pushing a `vX.Y.Z` tag runs
`.github/workflows/release.yml`, verifies the repository, and publishes
workspace crate versions that are not already on crates.io.

The tag version names the primary `rtce` crate. `rtce-testkit` versions
independently and is published first when its manifest version is missing
from crates.io.

## One-time trusted-publisher setup

Both crates already exist on crates.io, so no bootstrap token is needed.
For each crate, open its settings page and add a trusted publisher:

- <https://crates.io/crates/rtce/settings>
- <https://crates.io/crates/rtce-testkit/settings>

Use these exact values:

- Repository owner: `benjamin-small`
- Repository name: `rpg-theorycraft-engine`
- Workflow filename: `release.yml`
- Environment name: `crates-io`

The GitHub environment is restricted to `v*` tags. The workflow grants
`id-token: write` only to the publishing job, binds that job to the
`crates-io` environment, and uses
`rust-lang/crates-io-auth-action@v1` to exchange GitHub's OIDC identity
for a short-lived crates.io token. No `CARGO_REGISTRY_TOKEN` repository
secret is required.

## Release checklist

1. Update the version in `crates/rtce/Cargo.toml`. Update
   `crates/rtce-testkit/Cargo.toml` only when that crate has changed.
2. Promote the `CHANGELOG.md` entry to a
   `## [X.Y.Z] — YYYY-MM-DD` heading.
3. Run:

   ```sh
   cargo fmt --all --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   cargo publish -p rtce-testkit --dry-run
   cargo publish -p rtce --dry-run
   ```

4. Merge the release commit to `main`, then tag that exact commit:

   ```sh
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```

5. Watch the `Release` workflow through completion. The workflow queries
   crates.io first, skips any workspace crate version that is already present,
   and publishes only missing versions through trusted publishing.

## Retry and recovery

The workflow checks each crate and version through the crates.io API before
publishing. Re-running a partially or fully successful release is safe:
already-published versions are skipped.

| Failure | Resolution |
| --- | --- |
| Tag does not match `rtce` | Delete the incorrect local and remote tag, fix the manifest, and tag again. |
| Changelog heading is missing | Delete the tag, add the matching heading on `main`, and tag the corrected commit. |
| OIDC authentication fails | Confirm both trusted-publisher entries use the exact owner, repository, `release.yml` filename, and `crates-io` environment above. |
| Packaging fails | Fix the named manifest or packaged files in a pull request, then re-run with a new version if anything was already published. |
| One crate published before another failed | Fix the failure and re-run the same workflow; the published crate is skipped. |

Crates.io versions are immutable. Never move a tag to reuse a version that
was already published; bump the affected crate instead.
