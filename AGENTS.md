# AGENTS.md

Rules for AI coding agents working in neonhive: a Rust terminal UI that animates AI agents and subagents (Claude Code and Codex CLI are the real sources today). Human contributors: see [CONTRIBUTING.md](CONTRIBUTING.md). Where this file and the repo disagree, the repo wins; fix this file.

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

Run the first three before every push, and run the whole `cargo test`, not a subset: a subset hides failures elsewhere. Quote a test count in a pull request only from that run. `unsafe` is forbidden (`unsafe_code = "forbid"`). Do not silence a lint to get green; fix the code, or say why in the pull request.

Do not weaken a check to get green: no lowered limit, dropped step, or edit to `scripts/check-*.sh` or `.github/workflows` without saying why in the pull request. A human must read such a change, because those checks run from the pull request's own copy and could otherwise approve themselves. Move a finding a tool could catch into CI (a clippy lint in `[lints]`, a script in `scripts/`) rather than repeating it in review.

Actions in workflows are pinned by full commit SHA with the version as a trailing comment (`uses: actions/checkout@<sha> # v4.4.0`), so Dependabot can update both; resolve a new SHA from the tag on GitHub, never from memory. (`ci.yml` and `release.yml` are pinned; the other workflows follow in a later change.)

## Workflow

- Never commit to `main`. It is protected: pull request required, checks `lint`, `test (ubuntu-latest)`, `test (macos-latest)`, `conventional title`, `branch name`, `commit messages` and `changelog` must pass, linear history, no force push, admins included. Work on a branch, open a pull request, squash merge.
- Branch name: `<type>/<short-kebab-description>`, for example `fix/usage-double-count`. Lower-case letters, digits, single hyphens. CI checks it (`branch name`).
- Types: `feat fix docs style refactor perf test build ci chore revert`.
- Pull request titles and every commit subject: `<type>(<optional scope>): <subject>`, `!` after the type for breaking changes. The subject starts lower-case (or a digit) and has no final period. The squash merge makes the title the commit on `main`, so the title must stand alone. Commit bodies are free-form. Only pull requests authored by `dependabot[bot]` are exempt from the lower-case subject rule (title still needs a type, commit subjects are not checked); never key an exemption on a branch name.
- Pull request text: fill in `.github/PULL_REQUEST_TEMPLATE.md`, in normal prose. Tick a box only for a check you ran; leave the rest unchecked and say so. State plainly what was and was not verified.
- Planned work is tracked as GitHub issues on the roadmap project (https://github.com/users/juanpalopez/projects/5). A pull request that completes one says `Closes #N`. For anything large, open an issue first.
- Stacked pull requests are fine; merge them bottom-up. Before merging a parent with branch deletion, retarget every child to `main` (`gh pr edit <N> --base main`) and confirm it. Reason: GitHub closes a pull request whose base branch is deleted, and a closed one cannot be reopened once its base is gone (two pull requests had to be recreated). After a squash merge of the parent, merge `origin/main` into the child and push before reading its checks. Reason: the squash rewrites the parent's commits, so the child conflicts with `main` and its old check results describe a different base.
- Shared machine: stop only a job you started, by its task id or recorded PID; never `pkill -f` or `killall`. Reason: several agents run here and a pattern matches their processes too.
- Subagents: work in your own worktree and do not delete or edit outside it. Do not use `git add -A` (stray untracked files exist); add paths by name. Do not merge pull requests unless the maintainer asked for it in the current session.
- `.claude/` (worktrees, local settings) is local and ignored by git. Never commit it.

## Changelog and releases

- A pull request of type `feat`, `fix`, `perf`, or any with `!`, adds one user-facing line under `## [Unreleased]` in `CHANGELOG.md` (`### Added`, `### Changed`, `### Fixed` or `### Removed`). The file is hand-written ([Keep a Changelog](https://keepachangelog.com/en/1.1.0/)). Other types need no entry. Only a maintainer adds the `skip-changelog` label.
- Releases are tag-driven; see [docs/RELEASING.md](docs/RELEASING.md). Never tag, push tags, run `cargo publish` or create a GitHub release unless the maintainer asks for it in the current session: a published crate version cannot be overwritten.

## Architecture rules

Pipeline: transcript files -> source adapter (`Source`) -> `Event` -> `World` -> `App` -> `ui`. Each stage knows only the stage before it. Details and how to add an adapter: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

- Nothing outside a source adapter knows about a specific agent tool. Claude-specific names, paths and formats stay in `src/claude.rs`, and Codex-specific ones in `src/codex.rs`. Exceptions: the model-price table in `src/pricing.rs` is data, and `src/main.rs` and `src/picker.rs` wire the adapter in. Reason: a second adapter must need no change to `state.rs`, `app.rs` or `ui.rs`.
- Parse defensively. Transcript formats are not a contract: treat every field as optional, skip what you do not recognise, never panic on input.
- Never panic at any terminal size. `ui::tests::never_panics_at_any_terminal_size` sweeps sizes; keep it passing and keep new drawing code inside it.
- No network code and no writes to session files (see [SECURITY.md](SECURITY.md)). Reason: transcripts hold private prompts and code, and users trust the tool because it only reads.
- Animation and health use the app or world clock (`App::tick`, `World::now_ts`), never wall time. Reason: replays and `--demo --snapshot` must give identical frames and identical flags every run.
- Do not add a dependency without stating the reason in the pull request. The tree is small on purpose (see `[dependencies]` in `Cargo.toml`).
- Keep `rust-version` (1.87) working; do not use newer language or std features.
- A health or cost rule tuned on synthetic data is unproven. Replay real sessions locally and report counts, never content. Reason: the first loop detector passed its unit tests and was almost all false positives on real sessions.

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
| `src/codex.rs` | Codex CLI adapter: rollout parsing (plain and `.zst`), tailing, subagent threads, session discovery |
| `src/demo.rs` | Synthetic scripted adapter |
| `src/state.rs` | `World`: events folded into agents, log, totals, cost, health |
| `src/health.rs` | Stuck and loop detection (`Tracker`, flags) |
| `src/pricing.rs` | Token price table and `cost()`; `--prices FILE` overrides |
| `src/app.rs` | Playback clock, layout, animation state |
| `src/ui.rs` | Rendering into the ratatui buffer |
| `src/picker.rs` | Session list screen |
| `src/report.rs` | `neonhive report`: Markdown or JSON summary, never prompts |
| `src/watch.rs` | `neonhive watch`: alert logic (`Watcher`, `Idle`), pure and clock-free |
| `src/svg.rs` | SVG export |
| `src/fx.rs`, `src/util.rs` | Palette and color math; text and time helpers |
| `scripts/` | CI checks and release helpers (archives, Homebrew formula), runnable locally (see Commands) |
| `packaging/homebrew/` | Homebrew formula, source of truth for the `juanpalopez/homebrew-tap` copy |
| `docs/` | `ARCHITECTURE.md`, `RELEASING.md`, `demo.gif` (README recording) and `demo.tape` (its VHS script, run by `scripts/record-demo.sh`) |
