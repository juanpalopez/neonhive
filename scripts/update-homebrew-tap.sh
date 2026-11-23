#!/usr/bin/env bash
# Commit the formula for one version to the Homebrew tap repository.
# Usage: scripts/update-homebrew-tap.sh <version>
# Env:   HOMEBREW_TAP_TOKEN  token that may push to the tap; when empty or unset the
#                            script skips with a warning and exits 0.
#        TAP_REPO            tap repository (default juanpalopez/homebrew-tap).
#        DRY_RUN=1           render and commit in a temporary clone, but do not push;
#                            works without a token (the tap is cloned anonymously).
# Renders the formula with scripts/render-homebrew-formula.sh, writes it to
# Formula/neonhive.rb in the tap and pushes one commit to the tap's default branch.
# Re-running for a version that is already in the tap is not an error.
set -euo pipefail

version="${1:?usage: update-homebrew-tap.sh <version>}"
version="${version#v}"
tap="${TAP_REPO:-juanpalopez/homebrew-tap}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dry_run="${DRY_RUN:-}"
token="${HOMEBREW_TAP_TOKEN:-}"

# Prereleases are not offered through the tap.
if [[ "$version" == *-* ]]; then
  echo "::notice title=Homebrew tap::${version} is a prerelease; skipping the tap update."
  exit 0
fi

if [[ -z "$token" && "$dry_run" != 1 ]]; then
  echo "::warning title=Homebrew tap not updated::HOMEBREW_TAP_TOKEN is not set, so ${tap} still has the previous version (or does not exist). See docs/RELEASING.md, Homebrew tap, to set it up or update the tap by hand."
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if [[ -n "$token" ]]; then
  # The token goes in an http.extraheader set through the environment, the way
  # actions/checkout passes it: never written to .git/config, never in argv or a
  # remote URL. In Actions the token is masked as a secret; mask its encoded form too.
  basic="$(printf 'x-access-token:%s' "$token" | base64 | tr -d '\n')"
  [[ "${GITHUB_ACTIONS:-}" == true ]] && echo "::add-mask::${basic}"
  export GIT_CONFIG_COUNT=1
  export GIT_CONFIG_KEY_0="http.https://github.com/.extraheader"
  export GIT_CONFIG_VALUE_0="AUTHORIZATION: basic ${basic}"
fi

git clone --quiet --depth 1 "https://github.com/${tap}.git" "$work/tap"

mkdir -p "$work/tap/Formula"
"$here/render-homebrew-formula.sh" "$version" > "$work/formula.rb"
mv "$work/formula.rb" "$work/tap/Formula/neonhive.rb"

cd "$work/tap"
git add Formula/neonhive.rb
if git diff --cached --quiet; then
  echo "::notice title=Homebrew tap::${tap} already has neonhive ${version}; nothing to do."
  exit 0
fi

git -c user.name="github-actions[bot]" \
    -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
    commit --quiet -m "neonhive ${version}"

if [[ "$dry_run" == 1 ]]; then
  echo "DRY_RUN=1: not pushing. The commit would be:"
  git show --stat HEAD
  exit 0
fi

git push --quiet origin HEAD
echo "Pushed neonhive ${version} to ${tap}."
