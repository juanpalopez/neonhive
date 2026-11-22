# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `neonhive report [SESSION] [--json]`: prints a Markdown or JSON summary of a session (header, token split, agent table, most used tools, failures) without opening the UI. Defaults to the most recent session. Reports contain labels and tool names, never prompts.
- Cost estimates: per agent in the inspector, for the session in the header (`COST ~$…`), the subagents' share in the main agent's inspector, and a cost-by-agent-type view of the roster (`c`). Prices come from a built-in table of Claude list prices (taken 2026-11-22); `--prices FILE` replaces or extends it. Agents on a model with no known price show tokens only.

### Changed

- Token counts now include prompt-cache writes and are tracked per kind (input, output, cache read, cache write) and per model. The inspector shows the split. Earlier counts ignored cache tokens and were far too low on long sessions.

## [0.1.0] - 2026-11-22

### Added

- Animated agent graph: radial layout with overlap removal, spawn and dissolve effects, packets for spawns, results, messages and failures.
- Claude Code source adapter: reads session and subagent transcripts, follows nested subagents, tails files for live sessions.
- Live mode and replay mode with speeds from 1x to 128x and dead-time skipping.
- Inspector, roster and event stream panels.
- Session list screen.
- `--demo`: scripted synthetic swarm.
- `--snapshot`: headless frame as text, static SVG or animated SVG.
- 24-bit colour with a 256-colour fallback.

[Unreleased]: https://github.com/juanpalopez/neonhive/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/juanpalopez/neonhive/releases/tag/v0.1.0
