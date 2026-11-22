#!/usr/bin/env bash
# Check that every commit in <base>..<head> has a Conventional Commits subject.
# Usage: scripts/check-commit-messages.sh <base> <head>
#   e.g. scripts/check-commit-messages.sh origin/main HEAD
# Merge commits are skipped. Revert "...", fixup!, squash! and amend! subjects are allowed.
set -euo pipefail

base="${1:?usage: check-commit-messages.sh <base> <head>}"
head="${2:?usage: check-commit-messages.sh <base> <head>}"

types='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'
# <type>(<scope>)!: <subject>; subject starts lower-case, digit or backtick, no final period.
conventional="^(${types})(\\([a-z0-9._/-]+\\))?!?: [a-z0-9\`](.*[^.])?\$"
allowed='^(Revert "|(fixup|squash|amend)! )'

bad=0
while IFS=$'\t' read -r sha subject; do
  [[ -z "$sha" ]] && continue
  if [[ "$subject" =~ $conventional || "$subject" =~ $allowed ]]; then
    continue
  fi
  bad=$((bad + 1))
  echo "::error title=Commit message::${sha:0:7} \"${subject}\" is not a Conventional Commits subject. Use <type>(<optional scope>): <subject> with type one of ${types//|/, }; the subject starts lower-case and has no final period (example: fix(claude): count usage once). To reword: git rebase -i ${base} (mark the commit as reword), or for the latest commit git commit --amend; then git push --force-with-lease."
done < <(git log --no-merges --format='%H%x09%s' "${base}..${head}")

if ((bad > 0)); then
  echo "${bad} commit(s) need rewording."
  exit 1
fi
echo "All commit subjects in ${base}..${head} are OK."
