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

Commits inside the pull request can be titled however you like.

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
| `src/pricing.rs` | Price table and cost estimates |
| `src/app.rs` | Playback clock, graph layout, animation state |
| `src/ui.rs` | Rendering |
| `src/picker.rs` | Session list screen |
| `src/svg.rs` | SVG export |
| `src/report.rs` | `neonhive report`: Markdown and JSON summary of a `World` |

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

## Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
