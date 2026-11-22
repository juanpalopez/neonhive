#!/usr/bin/env bash
# Check that a release tag matches Cargo.toml and CHANGELOG.md.
# Usage: scripts/check-release-version.sh <tag>      e.g. scripts/check-release-version.sh v0.2.0
# Fails unless: the tag is vX.Y.Z (optionally -prerelease), Cargo.toml [package] version is
# the same X.Y.Z, and CHANGELOG.md has "## [X.Y.Z] - YYYY-MM-DD".
# Env: CARGO_TOML and CHANGELOG override the file paths (used for testing).
set -euo pipefail

tag="${1:?usage: check-release-version.sh <tag>}"
cargo_toml="${CARGO_TOML:-Cargo.toml}"
changelog="${CHANGELOG:-CHANGELOG.md}"

fail() {
  echo "::error title=Release::$*"
  exit 1
}

semver='^v([0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?)$'
[[ "$tag" =~ $semver ]] || fail "Tag '${tag}' is not of the form vX.Y.Z or vX.Y.Z-suffix."
version="${BASH_REMATCH[1]}"

# First `version = "..."` inside the [package] table.
crate_version="$(awk '
  /^\[/ { in_pkg = ($0 == "[package]"); next }
  in_pkg && /^version[ \t]*=/ { gsub(/^[^"]*"|".*$/, ""); print; exit }
' "$cargo_toml")"
[[ -n "$crate_version" ]] || fail "Could not read the package version from ${cargo_toml}."

[[ "$crate_version" == "$version" ]] ||
  fail "Tag ${tag} says ${version} but ${cargo_toml} has version ${crate_version}. Bump the version in a release pull request, merge it, then tag the merge commit."

heading="^## \\[${version//./\\.}\\] - [0-9]{4}-[0-9]{2}-[0-9]{2}\$"
grep -Eq "$heading" "$changelog" ||
  fail "${changelog} has no heading '## [${version}] - YYYY-MM-DD'. Move the [Unreleased] entries under a dated heading in the release pull request."

echo "Release ${tag}: ${cargo_toml} and ${changelog} agree on ${version}."
