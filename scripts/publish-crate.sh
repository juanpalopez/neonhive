#!/usr/bin/env bash
# Publish the crate to crates.io, safely.
# Usage: scripts/publish-crate.sh
# Env:   CARGO_REGISTRY_TOKEN  required to publish; when empty or unset the script skips and exits 0.
# Re-running for a version that is already on crates.io is not an error.
set -euo pipefail

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" ]]; then
  echo "::notice title=crates.io::CARGO_REGISTRY_TOKEN is not set; skipping cargo publish."
  exit 0
fi

out="$(mktemp)"
trap 'rm -f "$out"' EXIT

# pipefail makes the if test cargo's exit status, not tee's.
if cargo publish --locked 2>&1 | tee "$out"; then
  exit 0
fi

if grep -q 'already exists' "$out"; then
  echo "::notice title=crates.io::This version is already published; nothing to do."
  exit 0
fi
echo "::error title=crates.io::cargo publish failed; see the log above."
exit 1
