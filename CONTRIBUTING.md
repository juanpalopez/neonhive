# Contributing to neonhive

Thanks for helping. Bug reports, fixes, new source adapters and visual ideas are all welcome.

## Setup

You need a [Rust toolchain](https://rustup.rs), 1.87 or newer.

```sh
git clone https://github.com/juanpalopez/neonhive
cd neonhive
cargo run -- --demo
```

`--demo` plays a synthetic swarm, so you can work on the UI without any agent sessions.

## Before you open a pull request

CI runs these three commands and fails on any warning. Run them first:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Lint levels are set in the `[lints]` table of `Cargo.toml`, so `cargo clippy` on
your machine reports the same things as CI.

## Pull request titles

Pull requests are squash-merged, and the title becomes the commit message on
`main`. Titles follow [Conventional Commits](https://www.conventionalcommits.org/),
and CI checks them:

```
<type>(<optional scope>): <subject>
```

| Type | Use for |
| --- | --- |
| `feat` | A new feature |
| `fix` | A bug fix |
| `docs` | Documentation only |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `perf` | Performance improvement |
| `test` | Tests only |
| `build` | Dependencies, packaging |
| `ci` | CI configuration |
| `style` | Formatting, no code change |
| `chore` | Maintenance |
| `revert` | Reverting a commit |

The subject starts lower-case and has no final period. Add `!` after the type for
a breaking change. Examples:

```
feat(ui): add timeline bar to replay
fix(claude): count usage once per response
feat!: rename --latest to --last
```

## Commit messages

Every commit in your pull request uses the same format as the title, and CI checks
the subject line of each one (the `commit messages` check):

```
feat(ui): add timeline bar to replay
fix(claude): count usage once per response
```

Merge commits, `Revert "..."`, and `fixup!` / `squash!` / `amend!` commits are
allowed. The body of a commit is free-form. To check before you push:

```sh
scripts/check-commit-messages.sh origin/main HEAD
```

To fix a subject, run `git rebase -i origin/main`, mark the commit as `reword`, then
`git push --force-with-lease`. For the latest commit, `git commit --amend` is enough.

## Branch names

Name your branch `<type>/<short-kebab-description>`, with the same types as the
titles above. The description uses lower-case letters, digits and single hyphens:

```
feat/cost-accounting
fix/usage-double-count
ci/branch-and-commit-checks
```

CI checks this (the `branch name` check). Bot branches (`dependabot/`, `renovate/`)
and `release/` branches are exempt. Pull requests from a fork only get a warning,
since a branch called `main` or `patch-1` is common there and does not reach history
after a squash merge. To rename a branch:

```sh
git branch -m feat/new-name
git push origin -u feat/new-name
git push origin --delete old-name
```

Then open the pull request again from the renamed branch. To check a name locally:
`scripts/check-branch-name.sh "$(git branch --show-current)"`.

## Changelog

`CHANGELOG.md` is written by hand ([Keep a Changelog](https://keepachangelog.com/en/1.1.0/)),
and each release's notes on GitHub are copied from it. If your pull request title
has type `feat`, `fix` or `perf`, or has a `!` (breaking), add one line under
`## [Unreleased]`, in the matching `### Added`, `### Changed`, `### Fixed` or
`### Removed` group, written for a user rather than a reviewer. Other types
(`docs`, `ci`, `chore`, ...) do not need an entry. CI checks this (the `changelog`
check). If a `feat`/`fix`/`perf` change has no visible effect, ask a maintainer to
add the `skip-changelog` label. To check locally:
`scripts/check-changelog.sh "feat: your title" origin/main HEAD`.

Maintainers: see [docs/RELEASING.md](docs/RELEASING.md) for how a release is cut.

## Looking at your change

You cannot attach a terminal to a pull request, but you can attach a frame:

```sh
cargo run -- --demo --snapshot 20                  # text
cargo run -- --demo --snapshot 20 --svg shot.svg   # image
```

The demo is scripted, not random, so the same command gives the same frame before
and after your change. For visual changes, please include a before and after.

## Where things are

| File | Role |
| --- | --- |
| `src/event.rs` | Normalized events and the `Source` trait |
| `src/claude.rs` | Claude Code adapter: transcript parsing, tailing, session discovery |
| `src/demo.rs` | Synthetic adapter |
| `src/state.rs` | World state, built by applying events |
| `src/app.rs` | Playback clock, graph layout, animation state |
| `src/ui.rs` | Rendering |
| `src/picker.rs` | Session list screen |
| `src/svg.rs` | SVG export |

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains how they fit together and how to add an adapter for another agent tool.

## Guidelines

- **Keep the core tool-agnostic.** Nothing outside a source adapter should know what Claude Code is.
- **Parse defensively.** Transcript formats are not stable contracts. Skip what you do not recognise; never panic on input.
- **Test what you add.** Parser tests build small transcript files in a temp directory; see the tests at the bottom of `src/claude.rs`. Never commit real transcripts: they contain private prompts and code.
- **No panics at any terminal size.** `ui::tests::never_panics_at_any_terminal_size` sweeps sizes; keep it passing.
- **No network, no writes.** neonhive reads local files and draws. Changes that send data anywhere or modify session files will not be merged.
- **Small pull requests** are reviewed faster. For anything large, open an issue first so we can agree on the approach.

## Reporting bugs

Open an issue with your OS, terminal, neonhive version (`neonhive --version`), the
Claude Code version if the bug is about a session, and a `--snapshot` frame if it is
visual. Remove anything private from snapshots before posting.

AI coding agents: [AGENTS.md](AGENTS.md) holds the rules for them (Claude Code reads it through `CLAUDE.md`).

## Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
