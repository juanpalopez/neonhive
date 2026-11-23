#!/usr/bin/env bash
# Build a release binary for one target and package it with its SHA-256 checksum.
# Usage: scripts/build-release-archive.sh <target> [out-dir]
#   e.g. scripts/build-release-archive.sh aarch64-apple-darwin
# Writes <out-dir>/neonhive-v<version>-<target>.tar.gz (default out-dir: dist) holding
# neonhive-v<version>-<target>/{neonhive,LICENSE,README.md}, and next to it the same name
# plus .sha256, in the "<hash>  <file>" format that `sha256sum -c` and `shasum -a 256 -c` read.
# The version comes from Cargo.toml. The target's Rust standard library must be installed
# (rustup target add <target>). Release.yml runs this same script on every target.
# Env:   SKIP_BUILD=1  package an existing target/<target>/release/neonhive without building.
set -euo pipefail

target="${1:?usage: build-release-archive.sh <target> [out-dir]}"
out="${2:-dist}"

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [[ -z "$version" ]]; then
  echo "::error title=Release archive::version not found in Cargo.toml." >&2
  exit 1
fi

if [[ "${SKIP_BUILD:-}" != 1 ]]; then
  # --locked: build exactly the dependency versions in Cargo.lock, as cargo install --locked does.
  # Symbols are stripped to shrink the download; the crate's own profile is left alone.
  CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --release --locked --target "$target"
fi

bin="target/${target}/release/neonhive"
if [[ ! -x "$bin" ]]; then
  echo "::error title=Release archive::${bin} does not exist; the build did not produce it." >&2
  exit 1
fi

name="neonhive-v${version}-${target}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
cp "$bin" LICENSE README.md "$stage/$name/"

mkdir -p "$out"
out="$(cd "$out" && pwd)"
archive="${name}.tar.gz"
# COPYFILE_DISABLE keeps macOS tar from adding ._ resource-fork files.
COPYFILE_DISABLE=1 tar -czf "$out/$archive" -C "$stage" "$name"

if command -v sha256sum > /dev/null 2>&1; then
  hash="$(sha256sum "$out/$archive" | cut -d ' ' -f 1)"
else
  hash="$(shasum -a 256 "$out/$archive" | cut -d ' ' -f 1)"
fi
printf '%s  %s\n' "$hash" "$archive" > "$out/$archive.sha256"

echo "Wrote $out/$archive"
cat "$out/$archive.sha256"
