#!/usr/bin/env bash
# Lint the GitHub Actions workflows (actionlint) and the shell scripts (shellcheck).
# Usage: scripts/lint-workflows.sh
# Env:   CI=true  on Linux, download a pinned actionlint (SHA-256 verified) when it is not on PATH.
# Locally both tools must be installed: brew install actionlint shellcheck
# actionlint runs shellcheck on `run:` blocks too, when shellcheck is on PATH.
# Before linting, a self-test checks that actionlint reports findings (exit 1)
# on scripts/fixtures/bad-workflow.yml, so a skipped or broken linter cannot pass.
set -euo pipefail

ACTIONLINT_VERSION=1.7.12
# From actionlint_1.7.12_checksums.txt in the upstream release.
ACTIONLINT_SHA256=8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v shellcheck > /dev/null 2>&1; then
  echo "::error title=shellcheck missing::shellcheck is not on PATH. Install it with 'brew install shellcheck' (macOS) or 'apt-get install shellcheck' (Debian/Ubuntu)."
  exit 1
fi

if ! command -v actionlint > /dev/null 2>&1; then
  if [[ "${CI:-}" == true && "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]; then
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    archive="actionlint_${ACTIONLINT_VERSION}_linux_amd64.tar.gz"
    curl -fsSL --retry 3 -o "$dir/$archive" \
      "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/${archive}"
    if ! echo "${ACTIONLINT_SHA256}  $dir/$archive" | sha256sum --check --status; then
      echo "::error title=actionlint checksum::The downloaded ${archive} does not match the pinned SHA-256. Refusing to run it."
      exit 1
    fi
    tar -xzf "$dir/$archive" -C "$dir" actionlint
    export PATH="$dir:$PATH"
  else
    echo "::error title=actionlint missing::actionlint is not on PATH. Install it with 'brew install actionlint' (macOS) or see https://github.com/rhysd/actionlint/blob/main/docs/install.md."
    exit 1
  fi
fi

actionlint -version | head -n 1

# Self-test: findings must exit 1. 0 means the linter saw nothing wrong in a
# broken file; 3 or anything else means it failed to run.
fixture=scripts/fixtures/bad-workflow.yml
set +e
selftest_out="$(actionlint "$fixture" 2>&1)"
selftest_rc=$?
set -e
if [[ "$selftest_rc" -ne 1 ]]; then
  echo "$selftest_out"
  echo "::error title=actionlint self-test::actionlint exited ${selftest_rc} on ${fixture}; expected 1 (findings). The linter is not working, so a clean result would mean nothing."
  exit 1
fi
for needle in 'potentially untrusted' 'nosuchstep'; do
  if ! grep -q "$needle" <<< "$selftest_out"; then
    echo "$selftest_out"
    echo "::error title=actionlint self-test::actionlint did not report '${needle}' for ${fixture}."
    exit 1
  fi
done
echo "actionlint self-test OK: reported findings (exit 1) on ${fixture}."

status=0

echo "Running actionlint on .github/workflows"
actionlint -color || status=1

echo "Running shellcheck on scripts/*.sh"
shellcheck scripts/*.sh || status=1

if [[ "$status" -ne 0 ]]; then
  echo "::error title=Lint::actionlint or shellcheck reported problems; see the log above. Run scripts/lint-workflows.sh locally to reproduce."
  exit 1
fi
echo "Workflows and scripts are clean."
