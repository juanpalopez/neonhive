#!/usr/bin/env bash
# Require a CHANGELOG.md change for user-facing pull requests.
# Usage: scripts/check-changelog.sh <pr-title> <base-ref> <head-ref>
#   e.g. scripts/check-changelog.sh "feat(ui): add timeline" origin/main HEAD
# Titles of type feat, fix or perf, or with "!" (breaking), must change CHANGELOG.md.
# Other types are exempt. Env: SKIP_CHANGELOG=1 opts out (the `skip-changelog` label).
# The diff is <base>...<head>: changes on the head side since the merge base.
set -euo pipefail

title="${1:?usage: check-changelog.sh <pr-title> <base-ref> <head-ref>}"
base="${2:?usage: check-changelog.sh <pr-title> <base-ref> <head-ref>}"
head="${3:?usage: check-changelog.sh <pr-title> <base-ref> <head-ref>}"

header='^([a-z]+)(\([^)]*\))?(!)?:'
if [[ ! "$title" =~ $header ]]; then
  # Title format is enforced by the PR title check; do not double-report here.
  echo "Title '${title}' has no Conventional Commits prefix; nothing to check."
  exit 0
fi
type="${BASH_REMATCH[1]}"
breaking="${BASH_REMATCH[3]}"

if [[ -z "$breaking" && "$type" != feat && "$type" != fix && "$type" != perf ]]; then
  echo "Type '${type}' does not need a changelog entry."
  exit 0
fi

if [[ "${SKIP_CHANGELOG:-0}" == 1 ]]; then
  echo "Label skip-changelog is set; skipping."
  exit 0
fi

if git diff --name-only "${base}...${head}" | grep -qx 'CHANGELOG.md'; then
  echo "CHANGELOG.md is updated."
  exit 0
fi

echo "::error title=Changelog::A '${type}${breaking}' pull request must add an entry under [Unreleased] in CHANGELOG.md. See the Changelog section of CONTRIBUTING.md. If it truly has no user-visible effect, ask a maintainer to add the skip-changelog label."
exit 1
