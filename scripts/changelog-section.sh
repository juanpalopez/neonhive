#!/usr/bin/env bash
# Print the body of one version's section from CHANGELOG.md (Keep a Changelog).
# Usage: scripts/changelog-section.sh <version> [changelog-file]
#   e.g. scripts/changelog-section.sh 0.1.0 > notes.md
# The section runs from its "## [<version>]" heading (heading excluded) to the next
# "## [" heading, the link reference definitions at the bottom, or the end of file.
# Leading and trailing blank lines are dropped. Exits 1 if the section is missing or empty.
set -euo pipefail

version="${1:?usage: changelog-section.sh <version> [changelog-file]}"
file="${2:-CHANGELOG.md}"

[[ -f "$file" ]] || { echo "::error title=Changelog::$file not found." >&2; exit 1; }

# Version is compared as a plain string, so dots are never regex wildcards.
body="$(
  awk -v want="$version" '
    /^## \[/ {
      if (on) exit
      head = $0
      sub(/^## \[/, "", head)
      sub(/\].*$/, "", head)
      on = (head == want)
      next
    }
    on && /^\[[^]]+\]:[ \t]/ { exit }
    on { lines[++n] = $0 }
    END {
      first = 1
      while (first <= n && lines[first] ~ /^[ \t\r]*$/) first++
      last = n
      while (last >= first && lines[last] ~ /^[ \t\r]*$/) last--
      for (i = first; i <= last; i++) print lines[i]
    }
  ' "$file"
)"

if [[ -z "$body" ]]; then
  echo "::error title=Changelog::No non-empty section '## [${version}]' in ${file}. Move the [Unreleased] entries under a heading '## [${version}] - YYYY-MM-DD'." >&2
  exit 1
fi

printf '%s\n' "$body"
