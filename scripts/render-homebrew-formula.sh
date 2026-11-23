#!/usr/bin/env bash
# Print the Homebrew formula for one released version.
# Usage: scripts/render-homebrew-formula.sh <version> [formula-file]
#   e.g. scripts/render-homebrew-formula.sh 0.2.0 > Formula/neonhive.rb
# Reads packaging/homebrew/neonhive.rb (or formula-file), points its url at the GitHub
# source tarball of tag v<version> and sets sha256 to that tarball's SHA-256. Everything
# else in the formula is printed unchanged. The tag must already be pushed.
# Env:   SHA256=<hex>  use this checksum instead of downloading the tarball (offline use).
#        REPO=owner/name  source repository (default juanpalopez/neonhive).
set -euo pipefail

version="${1:?usage: render-homebrew-formula.sh <version> [formula-file]}"
version="${version#v}"
formula="${2:-$(dirname "${BASH_SOURCE[0]}")/../packaging/homebrew/neonhive.rb}"
repo="${REPO:-juanpalopez/neonhive}"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "::error title=Homebrew formula::'${version}' is not a version like 1.2.3." >&2
  exit 1
fi
[[ -f "$formula" ]] || { echo "::error title=Homebrew formula::$formula not found." >&2; exit 1; }

url="https://github.com/${repo}/archive/refs/tags/v${version}.tar.gz"

sha="${SHA256:-}"
if [[ -z "$sha" ]]; then
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' EXIT
  if ! curl -fsSL --retry 3 -o "$tmp" "$url"; then
    echo "::error title=Homebrew formula::Could not download ${url}. Is the tag v${version} pushed?" >&2
    exit 1
  fi
  if command -v sha256sum > /dev/null 2>&1; then
    sha="$(sha256sum "$tmp" | cut -d ' ' -f 1)"
  else
    sha="$(shasum -a 256 "$tmp" | cut -d ' ' -f 1)"
  fi
fi
if [[ ! "$sha" =~ ^[0-9a-f]{64}$ ]]; then
  echo "::error title=Homebrew formula::'${sha}' is not a SHA-256 in lower-case hex." >&2
  exit 1
fi

# Exactly one url and one sha256 line at two-space indent (the stable spec); fail
# rather than print a formula that still names the old version.
for key in url sha256; do
  count="$(grep -c "^  ${key} \"" "$formula" || true)"
  if [[ "$count" -ne 1 ]]; then
    echo "::error title=Homebrew formula::Expected one '  ${key} \"...\"' line in ${formula}, found ${count}." >&2
    exit 1
  fi
done

sed -e "s|^  url \".*\"\$|  url \"${url}\"|" \
    -e "s|^  sha256 \".*\"\$|  sha256 \"${sha}\"|" \
    "$formula"
