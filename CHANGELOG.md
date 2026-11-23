# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Teaching mode for presenting a session. While a replay or the demo is paused, `.` steps to the next event and `,` back to the previous one (a step while playing pauses first; the demo pauses into a replay). `e` or `--captions` shows a two-line caption above the footer that explains the most recent event in plain words (a spawn, a result, a message, a tool call or a failed one). `--svg FILE --from A --to B` exports a looping animated SVG of a range of replay, at `--fps` frames per second (a divisor of 30, up to 1,800 frames), and warns on stderr that an export of a real session can contain private prompts, code and file paths. Animated SVGs now write text that stays put across frames only once, so they are several times smaller.
- OpenAI Codex CLI sessions. The session list, `--latest` and `neonhive report` include Codex rollouts from `~/.codex/sessions` (or `$CODEX_HOME`), labelled `codex`, plain or zstd-compressed; a file given on the command line is recognised by its name or first line. Prompts, assistant text, tool calls (shell commands, patches, MCP and web search), failed calls and tokens (input, output, cached input) are shown, and subagent threads appear under the agent that spawned them. There are no OpenAI prices yet, so Codex sessions show a dash for cost.
- Panel focus, resize, full size, and a scrollable, searchable event stream. `tab` moves focus between graph, inspector, roster and stream; `[` `]` and `<` `>` resize the stream and the side column (the stream starts at about a third of the terminal height, at least 9 rows, and follows the terminal until you resize it); `z` shows the focused panel full size (the full-size inspector shows the whole prompt and last output, wrapped and scrollable). The stream scrolls back through its history (now 10,000 lines, was 400), pauses while scrolled back and counts new lines, and `G` returns to the tail. `/` searches it (case-insensitive), `n` `N` move between matches and `&` shows matching lines only. Arrows, `j` `k` and page keys act on the focused panel; `esc` steps back one layer at a time.
- `neonhive report [SESSION] [--json]`: prints a Markdown or JSON summary of a session (header, token split, estimated cost with a cost per agent, agent table, most used tools, failures, and agents flagged as looping, erroring or stalled with the reason) without opening the UI. Defaults to the most recent session that has events. Cost follows `--prices`; an agent on a model with no known price shows a dash and makes the total a lower bound. Health flags match `neonhive watch --once`. A file with no session events (empty, not a transcript, corrupt) is an error. Reports contain the session title, labels and tool names, never prompts.
- Cost estimates: per agent in the inspector, for the session in the header (`COST ~$…`), the subagents' share in the main agent's inspector, and a cost-by-agent-type view of the roster (`c`). Prices come from a built-in table of Claude list prices (taken 2026-11-22); `--prices FILE` replaces or extends it. Agents on a model with no known price show tokens only.
- Stuck and loop detection. An agent is flagged when it makes the same tool call (same tool, same full input) 4 times in a row with each repeat issued within a minute of the previous one, has 3 failed tool calls in a row, or stays silent while running (10 minutes after text, or 30 after a tool call or message; a tool error counts as activity and does not end the wait). Identical calls more than a minute apart are treated as polling, not a loop. The main agent, a parent with running subagents, and an agent that finished and was resumed are never flagged as stalled. Flags show on the node, in the roster, in the inspector (`HEALTH`) and once in the event stream; the header counts flagged agents. The demo scripts a looping agent.

- `neonhive watch [SESSION]`: a headless watchdog that prints one line per alert (a failed agent; a looping, erroring or stalled one; a passed `--max-tokens` or `--max-cost` budget) as a session runs. `--json` prints one object per line, `--once` reports the existing transcript and exits, `--interval` and `--idle-exit` control polling and when a quiet session counts as ended. Exit code 0 when clean, 1 when alerts fired, 2 on usage or I/O errors.

### Changed

- Token counts now include prompt-cache writes and are tracked per kind (input, output, cache read, cache write) and per model. The inspector shows the split. Earlier counts ignored cache tokens and were far too low on long sessions.

### Fixed

- The main agent's elapsed time in the inspector runs from the session's first prompt. It started at the first reply, and a session with a prompt but no reply yet showed a time of hundreds of thousands of hours.
- Cost estimates price 1-hour prompt-cache writes at their own rate (2x input) instead of the 5-minute rate (1.25x). The split comes from the transcript; the inspector and `neonhive report` show both kinds, and the report JSON gains `cache_write_5m` and `cache_write_1h` (`cache_write` is still the total). `--prices` files may add an optional `cache_write_1h`, which defaults to `cache_write`.
- Background subagents whose completion is written only as a queued task notification or as a user turn that is the notification now finish. They used to stay running forever.
- The graph keeps flagged agents and the agent you selected (with their parents) on screen, as many as fit without boxes covering each other, and still leaves room for one healthy agent. With more running agents than the graph has room for, it shows the earliest and the latest started ones and changes only when an agent starts or finishes, rather than swapping nodes on every tool call. `+N archived` counts the agents not on the graph even while nodes fade, and adds how many of them are flagged (`+54 archived · 3 ▲`), shortening to fit narrow graphs.

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
