#!/usr/bin/env bash
# Check that a commit is on main, so a release tag cannot publish a branch commit.
# Usage: scripts/check-tag-on-main.sh <commit-sha>      e.g. scripts/check-tag-on-main.sh "$GITHUB_SHA"
# Fetches origin main, then fails unless the commit is an ancestor of (or equal to) it.
# Fails closed: an unknown commit, a failed fetch or a shallow clone is an error, never a pass.
# Needs full history (actions/checkout with fetch-depth: 0), or the ancestry test is unreliable.
set -euo pipefail

sha="${1:?usage: check-tag-on-main.sh <commit-sha>}"

fail() {
  echo "::error title=Release::$*"
  exit 1
}

if [[ "$(git rev-parse --is-shallow-repository)" == "true" ]]; then
  fail "The checkout is shallow, so ancestry cannot be tested. Use actions/checkout with fetch-depth: 0."
fi

git fetch --quiet --no-tags origin '+refs/heads/main:refs/remotes/origin/main' ||
  fail "Could not fetch origin main, so the tagged commit cannot be checked. Re-run the workflow; if it keeps failing, check that the repository and its main branch are reachable."

commit="$(git rev-parse --verify --quiet "${sha}^{commit}")" ||
  fail "Commit ${sha} is not in this repository. Check that the tag points at a commit that was pushed."

git merge-base --is-ancestor "$commit" refs/remotes/origin/main ||
  fail "Commit ${commit} is not on main. Release tags go on main only. Delete the tag (git push origin :refs/tags/vX.Y.Z), merge the release pull request, then tag the merge commit on an up-to-date main."

echo "Commit ${commit} is on main."
