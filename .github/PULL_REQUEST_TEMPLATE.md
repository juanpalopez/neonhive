<!--
Title and every commit subject must follow Conventional Commits, e.g. "feat(ui): add timeline bar".
Branch name: <type>/<short-kebab-description>, e.g. "feat/timeline-bar".
See CONTRIBUTING.md.
-->

## What this changes

## Why

## How I checked it

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] Changelog: an entry under `## [Unreleased]` in `CHANGELOG.md` for `feat`, `fix`, `perf` or breaking changes (or ask a maintainer for the `skip-changelog` label)
- [ ] For visual changes: before and after frames (`--demo --snapshot 20`, as text or `--svg`)
