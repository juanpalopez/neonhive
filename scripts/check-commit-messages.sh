#!/usr/bin/env bash
# Check that every commit in <base>..<head> has a Conventional Commits subject.
# Usage: scripts/check-commit-messages.sh <base> <head>
#   e.g. scripts/check-commit-messages.sh origin/main HEAD
# Env:   WARN_ONLY=1  report problems as warnings and exit 0 (used for fork PRs).
# Merge commits are skipped. Revert "...", fixup!, squash! and amend! subjects are allowed.
set -euo pipefail

base="${1:?usage: check-commit-messages.sh <base> <head>}"
head="${2:?usage: check-commit-messages.sh <base> <head>}"

# A range that cannot be resolved must fail, not pass with nothing checked.
for ref in "$base" "$head"; do
  if ! git rev-parse --verify --quiet "${ref}^{commit}" > /dev/null; then
    echo "::error title=Commit message::Cannot find commit ${ref}; the commit check did not run."
    exit 1
  fi
done

types='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'
# <type>(<scope>)!: <subject>; subject starts lower-case, digit or backtick, no final period.
conventional="^(${types})(\\([a-z0-9._/-]+\\))?!?: [a-z0-9\`](.*[^.])?\$"
allowed='^(Revert "|(fixup|squash|amend)! )'

level=error
[[ "${WARN_ONLY:-0}" == 1 ]] && level=warning

# Only commits the branch added: a base that moved on must not drag its own
# commits into the range.
start=$(git merge-base "$base" "$head")
log=$(git log --no-merges --format='%H%x1f%s' "${start}..${head}")

bad=0
while IFS= read -r line; do
  [[ -z "$line" ]] && continue
  sha="${line%%$'\x1f'*}"
  subject="${line#*$'\x1f'}"
  if [[ "$subject" =~ $conventional || "$subject" =~ $allowed ]]; then
    continue
  fi
  bad=$((bad + 1))
  # The subject is contributor-controlled. Control characters and "::" could
  # start a workflow command, so it is cleaned and printed outside the annotation.
  shown="${subject//[[:cntrl:]]/ }"
  shown="${shown//::/: :}"
  echo "::${level} title=Commit message::Commit ${sha:0:7} does not have a Conventional Commits subject."
  echo "  ${sha:0:7}  ${shown}"
done <<< "$log"

if ((bad > 0)); then
  echo
  echo "Use <type>(<optional scope>): <subject>, with type one of ${types//|/, }."
  echo "The subject starts lower-case and has no final period, for example:"
  echo "  fix(claude): count usage once"
  echo "To reword: git rebase -i <base branch> and mark the commit as reword, or for"
  echo "the latest commit git commit --amend; then git push --force-with-lease."
  echo "${bad} commit(s) need rewording."
  [[ "$level" == warning ]] && exit 0
  exit 1
fi
echo "All commit subjects in ${start:0:7}..${head} are OK."
