# Agent instructions

## Purpose

Make focused, reviewable changes to rpg-theorycraft-engine. Preserve existing behavior unless the issue or pull request explicitly authorizes a change.

## Setup

Follow `README.md` for repository-specific setup.

## Validation

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features
cargo test --all-features
```

## Constraints

- Do not commit credentials, generated secrets, or local environment files.
- Keep documentation and tests synchronized with behavior changes.
- Do not overwrite unrelated work in a dirty working tree.
