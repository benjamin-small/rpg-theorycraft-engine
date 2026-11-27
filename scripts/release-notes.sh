#!/bin/sh
# Print the GitHub release body for an rtce version, built from that
# version's CHANGELOG.md section plus an Upgrade snippet and a link to the
# full changelog. Used by .github/workflows/release.yml.
#
# Usage: scripts/release-notes.sh <version>   (e.g. 0.10.0, no leading v)
#
# Fails closed: a missing heading or an empty section is an error, so a
# release never ships with a blank body.
set -eu

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: $0 <version>" >&2
  exit 2
fi
version=$1

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
changelog="$repo_root/CHANGELOG.md"
repo_url=${RELEASE_REPO_URL:-https://github.com/benjamin-small/rpg-theorycraft-engine}

heading=$(grep -m1 -F "## [$version]" "$changelog" || true)
case "$heading" in
  "## [$version]"*) ;;
  *)
    echo "error: CHANGELOG.md has no '## [$version]' heading" >&2
    exit 1
    ;;
esac

# GitHub's heading anchor: lowercase, drop everything but alphanumerics,
# spaces and hyphens, then spaces to hyphens.
# "## [0.10.0] — 2026-11-27" -> "0100--2026-11-27".
anchor=$(printf '%s' "${heading#\#\# }" |
  tr '[:upper:]' '[:lower:]' |
  LC_ALL=C sed 's/[^a-z0-9 -]//g' |
  tr ' ' '-')

section=$(awk -v h="## [$version]" '
  index($0, h) == 1 { found = 1; next }
  found && /^## \[/ { exit }
  found { print }
' "$changelog")

if [ -z "$(printf '%s' "$section" | tr -d '[:space:]')" ]; then
  echo "error: CHANGELOG.md section for $version is empty" >&2
  exit 1
fi

printf '%s\n' "$section"
printf '\n## Upgrade\n\n```toml\nrtce = "%s"\n```\n\n' "$version"
printf 'See the [full changelog](%s/blob/v%s/CHANGELOG.md#%s).\n' \
  "$repo_url" "$version" "$anchor"
