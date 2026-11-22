#!/usr/bin/env bash
# Check a branch name against the neonhive convention: <type>/<short-kebab-description>.
# Usage: scripts/check-branch-name.sh <branch>
# Env:   WARN_ONLY=1  report problems as warnings and exit 0 (used for fork PRs).
set -euo pipefail

branch="${1:?usage: check-branch-name.sh <branch>}"

types='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'
pattern="^(${types})/[a-z0-9]+(-[a-z0-9]+)*\$"

# Bot and release branches are not named by contributors.
case "$branch" in
  dependabot/* | renovate/* | release/*)
    echo "Branch '$branch' is exempt (bot or release branch)."
    exit 0
    ;;
esac

if [[ "$branch" =~ $pattern ]]; then
  echo "Branch name '$branch' is OK."
  exit 0
fi

level=error
[[ "${WARN_ONLY:-0}" == 1 ]] && level=warning

echo "::${level} title=Branch name::Branch '${branch}' does not match <type>/<short-kebab-description>. Allowed types: ${types//|/, }. The description uses lower-case letters, digits and single hyphens, for example feat/cost-accounting or fix/usage-double-count. To rename: git branch -m <new-name>, then git push origin -u <new-name>, delete the old remote branch (git push origin --delete '${branch}') and open the pull request again from the new branch."

[[ "$level" == warning ]] && exit 0
exit 1
