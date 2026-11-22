# AGENTS.md

Rules for AI coding agents working in neonhive: a Rust terminal UI that animates AI agents and subagents (Claude Code is the only real source today). Human contributors: see [CONTRIBUTING.md](CONTRIBUTING.md). Where this file and the repo disagree, the repo wins; fix this file.

## Commands

```sh
cargo fmt --check                                   # CI: lint
cargo clippy --all-targets -- -D warnings           # CI: lint (levels live in Cargo.toml [lints])
cargo test                                          # CI: test on ubuntu and macos
cargo run -- --demo                                 # scripted synthetic swarm, needs no sessions
cargo run -- --demo --snapshot 20 --size 140x40     # headless text frame (20 s into the demo)
cargo run -- --demo --snapshot 20 --svg shot.svg    # same frame as SVG
scripts/check-branch-name.sh "$(git branch --show-current)"
scripts/check-commit-messages.sh origin/main HEAD
scripts/check-changelog.sh "feat: your title" origin/main HEAD
```

Run the first three before every push. `unsafe` is forbidden (`unsafe_code = "forbid"`). Do not silence a lint to get green; fix the code, or say why in the pull request.

## Workflow

- Never commit to `main`. It is protected: pull request required, checks `lint`, `test (ubuntu-latest)` and `test (macos-latest)` must pass, linear history, no force push, admins included. Work on a branch, open a pull request, squash merge.
- Branch name: `<type>/<short-kebab-description>`, for example `fix/usage-double-count`. Lower-case letters, digits, single hyphens. CI checks it (`branch name`).
- Types: `feat fix docs style refactor perf test build ci chore revert`.
- Pull request titles and every commit subject: `<type>(<optional scope>): <subject>`, `!` after the type for breaking changes. The subject starts lower-case (or a digit) and has no final period. The squash merge makes the title the commit on `main`, so the title must stand alone. Commit bodies are free-form.
- Pull request text: fill in `.github/PULL_REQUEST_TEMPLATE.md`, in normal prose. Tick a box only for a check you ran; leave the rest unchecked and say so. State plainly what was and was not verified.
- Planned work is tracked as GitHub issues on the roadmap project (https://github.com/users/juanpalopez/projects/5). A pull request that completes one says `Closes #N`. For anything large, open an issue first.
- Stacked pull requests are fine; merge them bottom-up so each one targets a branch that still exists.
- `.claude/` (worktrees, local settings) is local and ignored by git. Never commit it.

## Changelog and releases

- A pull request of type `feat`, `fix`, `perf`, or any with `!`, adds one user-facing line under `## [Unreleased]` in `CHANGELOG.md` (`### Added`, `### Changed`, `### Fixed` or `### Removed`). The file is hand-written ([Keep a Changelog](https://keepachangelog.com/en/1.1.0/)). Other types need no entry. Only a maintainer adds the `skip-changelog` label.
- Releases are tag-driven; see [docs/RELEASING.md](docs/RELEASING.md). Never tag, push tags, run `cargo publish` or create a GitHub release unless the maintainer asks for it in the current session: a published crate version cannot be overwritten.

## Architecture rules

Pipeline: transcript files -> source adapter (`Source`) -> `Event` -> `World` -> `App` -> `ui`. Each stage knows only the stage before it. Details and how to add an adapter: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

- Nothing outside a source adapter knows about a specific agent tool. Claude-specific names, paths and formats stay in `src/claude.rs`. Exceptions: the model-price table in `src/pricing.rs` is data, and `src/main.rs` and `src/picker.rs` wire the adapter in. Reason: a second adapter must need no change to `state.rs`, `app.rs` or `ui.rs`.
- Parse defensively. Transcript formats are not a contract: treat every field as optional, skip what you do not recognise, never panic on input.
- Never panic at any terminal size. `ui::tests::never_panics_at_any_terminal_size` sweeps sizes; keep it passing and keep new drawing code inside it.
- No network code and no writes to session files (see [SECURITY.md](SECURITY.md)). Reason: transcripts hold private prompts and code, and users trust the tool because it only reads.
- Animation and health use the app or world clock (`App::tick`, `World::now_ts`), never wall time. Reason: replays and `--demo --snapshot` must give identical frames and identical flags every run.
- Do not add a dependency without stating the reason in the pull request. The tree is small on purpose (see `[dependencies]` in `Cargo.toml`).
- Keep `rust-version` (1.87) working; do not use newer language or std features.

## Privacy

- Never commit real transcripts, and never paste their content into issues, pull requests, tests, docs or snapshots you share. Tests build small synthetic transcripts in a temp directory (see the tests at the bottom of `src/claude.rs`).
- Commits use the GitHub noreply address already in the repo's git config. Do not set a personal email, and do not put the maintainer's personal details in anything sent to an outside service.

## Verifying visual changes

Tests do not show what the screen looks like. For any change to `ui`, `app` layout or `svg`, render the frame headless before and after, with `--demo --snapshot N --size WxH` (text) or `--svg FILE`, and try a small size and a large one. The demo is scripted, so the same command gives the same frame. Attach before and after to the pull request, and say what you did not look at.

## File map

| Path | Role |
| --- | --- |
| `src/main.rs` | CLI flags, `report` subcommand, wiring |
| `src/event.rs` | `Event` enum and the `Source` trait |
| `src/claude.rs` | Claude Code adapter: parsing, tailing, session discovery |
| `src/demo.rs` | Synthetic scripted adapter |
| `src/state.rs` | `World`: events folded into agents, log, totals, cost, health |
| `src/health.rs` | Stuck and loop detection (`Tracker`, flags) |
| `src/pricing.rs` | Token price table and `cost()`; `--prices FILE` overrides |
| `src/app.rs` | Playback clock, layout, animation state |
| `src/ui.rs` | Rendering into the ratatui buffer |
| `src/picker.rs` | Session list screen |
| `src/report.rs` | `neonhive report`: Markdown or JSON summary, never prompts |
| `src/svg.rs` | SVG export |
| `src/fx.rs`, `src/util.rs` | Palette and color math; text and time helpers |
| `scripts/` | CI checks, runnable locally (see Commands) |
| `docs/` | `ARCHITECTURE.md`, `RELEASING.md`, `demo.svg` |

`src/health.rs` arrives with pull request #23; until that merges it exists only on `feat/stuck-detection`.
