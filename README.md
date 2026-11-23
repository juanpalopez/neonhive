<h1 align="center">neonhive</h1>

<p align="center">
  <b>See your AI agents and subagents at work.</b><br>
  A cyberpunk terminal UI that visualizes agents and subagents: who spawned whom, what each one is doing, how they talk, and what it cost. Live or as a replay.<br>
  Works with Claude Code and OpenAI Codex CLI today. More agent tools are planned.
</p>

<p align="center">
  <a href="https://github.com/juanpalopez/neonhive/actions/workflows/ci.yml"><img src="https://github.com/juanpalopez/neonhive/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/neonhive"><img src="https://img.shields.io/crates/v/neonhive.svg?color=fcee0a" alt="crates.io"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-ff2abe.svg" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20ratatui-00f0ff.svg" alt="Built with Rust and ratatui">
</p>

<p align="center">
  <img src="docs/demo.svg" alt="neonhive showing a swarm of AI subagents as an animated graph in the terminal" width="100%">
</p>

When a coding agent fans out into ten subagents, the terminal shows you one spinner.
neonhive shows you the swarm. Every agent and subagent is a node. Every spawn, message
and result is a pulse travelling along an edge. Every tool call scrolls past in an
event stream. Select any agent to see its task, its model, its tokens and its cost.

neonhive is not tied to one agent tool. It reads the session files that
[Claude Code](https://claude.com/claude-code) and the OpenAI
[Codex CLI](https://github.com/openai/codex) write to your disk, and the core knows
nothing about either of them. Each tool is one source adapter, so adding another
does not touch the UI. See [Supported agent tools](#supported-agent-tools) for what
each one shows.

Try it in ten seconds, no agent sessions needed:

```sh
cargo install neonhive
neonhive --demo
```

## Features

### See the agents

- **Live agent graph.** Main agent in the centre, subagents around it, nested subagents further out. Nodes glitch into existence, pulse while they work and dim when they finish.
- **Message flow.** Coloured packets travel along edges: magenta for a spawn, green for a returned result, yellow for a message between agents, red for a failure.
- **Inspector.** Select an agent to see its prompt, parent, model, tokens, cost, tool calls, errors and last output.
- **Event stream.** A running log of spawns, tool calls, messages and results, coloured by agent. Scroll back through the last 10,000 lines; the stream pauses while you read and counts what arrived meanwhile. Press `/` to search it, `n` and `N` to move between matches, and `&` to show matching lines only.
- **Panels your size.** `tab` moves focus between the graph, inspector, roster and stream. Resize the stream and the side column, or press `z` to show the focused panel full size; the full-size inspector shows the whole prompt and last output.
- **Replay.** Open any past session and watch it again at 1x to 128x. Dead time is skipped, so a three-hour session takes minutes.
- **Scales to big sessions.** Hundreds of subagents stay readable: active agents hold the screen, finished ones dissolve into an archive counter.

### Understand what they did

- **Cost estimates.** Estimated cost per agent, per agent type and for the whole session, and how much of it the subagents spent. See [Cost estimates](#cost-estimates).
- **Stuck and loop detection.** Agents that repeat the exact same tool call in quick succession (polling with pauses between calls does not count), fail several calls in a row or go silent mid-task are flagged in orange on the graph, in the roster and in the inspector, which says why. Works in replay too.
- **Session reports.** `neonhive report` prints a Markdown or JSON summary of a session: tokens by kind, estimated cost, an agent table, most used tools, failures and health flags. Paste it into a pull request or feed it to a script.
- **Watchdog.** `neonhive watch` follows a session with no UI and prints one line per alert: an agent failed, an agent is looping, erroring or stalled, or the session passed a token or cost budget. Exit codes make it usable in scripts and CI. See [Watching a session](#watching-a-session).

### Safe to run

- **Read-only and local.** neonhive only reads transcript files that the agent tool already writes to your disk. It sends nothing over the network and changes no files.
- **Works anywhere.** 24-bit colour where the terminal supports it, a 256-colour palette where it does not.

## Install

neonhive needs a [Rust toolchain](https://rustup.rs) (1.87 or newer).

```sh
# from crates.io
cargo install neonhive

# latest from GitHub
cargo install --git https://github.com/juanpalopez/neonhive

# or from a checkout
git clone https://github.com/juanpalopez/neonhive
cd neonhive
cargo install --path .
```

## Usage

```sh
neonhive                    # pick a session from a list (Claude Code and Codex)
neonhive --latest           # open the most recent session
neonhive path/to/session.jsonl
neonhive ~/.codex/sessions/2026/11/22/rollout-...jsonl   # a Codex session
neonhive --demo             # endless synthetic swarm
```

The session list shows sessions from both tools, labelled `claude` or `codex`. A file
given on the command line is recognised by its name or its first line.

A session that was written to in the last two minutes opens **live** and follows
new activity. An older session opens as a **replay** from its start. Force either with
`--live` or `--replay`, and set the replay speed with `--speed 32`.

Start your agent tool (Claude Code or Codex) in one terminal, run `neonhive --latest`
in another, and give the agent a task that uses subagents.

### Watching a session

`neonhive watch [SESSION]` follows a session without opening the UI and prints one
line per alert to stdout. Without an argument it watches the most recent session.

```sh
neonhive watch                                  # follow the latest session
neonhive watch --max-tokens 2000000 --max-cost 5
neonhive watch --json | jq -r .message          # one JSON object per line
neonhive watch --once path/to/session.jsonl     # print current alerts and exit
neonhive watch --idle-exit 300                  # stop after 5 quiet minutes
```

Alerts: an agent failed; an agent was flagged looping, erroring or stalled (same
rules as the UI; one alert when the flag appears, and a `recovered` line when it
goes away); the session passed `--max-tokens N` (input + output + cache writes) or
`--max-cost USD` (the estimate from [Cost estimates](#cost-estimates); if some
agents could not be priced the alert says so, since the real cost is then higher).
Each budget alert fires once. `--prices FILE` works as everywhere else.
`--interval MS` sets the poll interval (default 1000, lowest 100).

With `--json` every line has the same keys, `null` where they do not apply:

```json
{"ts":1700000000000,"kind":"looping","agent":"a1","label":"Fix it","message":"agent loop: Bash x4","value":null,"limit":null,"unpriced":null}
{"ts":1700000060000,"kind":"max_cost","agent":null,"label":"","message":"estimated cost ~$5.20 passed the limit of $5.00","value":5.2,"limit":5.0,"unpriced":0}
```

`kind` is `failed`, `looping`, `erroring`, `stalled`, `recovered`, `max_tokens` or
`max_cost`. `ts` is Unix milliseconds.

Exit codes: `0` when the session ended cleanly or `--once` found nothing, `1` when
at least one alert fired (`recovered` lines do not count), `2` for a usage error or
an unreadable session. Transcripts have no end marker, so a live session counts as
ended when no new events arrive for `--idle-exit SECONDS`. That is off by default:
without it, watch runs until you stop it. Press Ctrl-C to stop; neonhive installs no
signal handler, so the shell reports the usual interrupted status. `--once` uses the
session's own clock (its last event), so it cannot report stalls that only show as
real time passes. Alerts name agents by id and label, never prompts or tool input.

### Session reports

`neonhive report` summarizes a session on stdout and does not open the UI. Without
an argument it reports the most recent session.

```sh
neonhive report                          # Markdown, most recent session
neonhive report path/to/session.jsonl    # a specific session
neonhive report --json > report.json     # JSON for scripts
neonhive report >> "$GITHUB_STEP_SUMMARY"
```

The Markdown has a header (start, duration, agents, tool calls, errors, tokens,
estimated cost), a token table split into input, output, cache write (5-minute and
1-hour) and cache read, an agent table in spawn order with nesting (`↳`) and a cost
per agent, the ten most used tools, a list of failed agents and agents with tool
errors, and the agents flagged as looping, erroring or stalled, with the reason. It
renders on GitHub.

- **Start and duration** run from the first prompt to the last event. A session
  with no timestamps shows `-` (`null` in JSON).
- **Tool calls** counts the calls shown as tool calls in the event stream. A call
  that the adapter turns into a subagent spawn or a message to another agent is
  shown as that instead and is not counted; Claude Code's subagent and message
  calls are such calls. Where an adapter sees such a call only as an ordinary call,
  it is counted.
- **Cost** is the estimate from [Cost estimates](#cost-estimates), and
  `--prices FILE` changes it. An agent on a model with no known price shows `-`
  (`null`), never 0, and the session total is then marked as a lower bound (`+`
  and `lower_bound: true`).
- **Health flags** are the same as the UI's and as `neonhive watch --once`: the
  rules run once at the session's own clock (its last event), so a stall that only
  shows as real time passes is not reported. A reason names a tool and a count, or
  a silence; never tool input.

A report is read in one go, so a last line with no trailing newline still counts.
A file that yields no events at all (empty, not a transcript, a corrupt `.zst`, or
a session with no prompt or reply yet) is an error: `report` prints a message on
stderr and exits with status 1.

A report never includes prompts, messages or tool arguments. It does hold:

- the **title**: the project directory name and, when the agent tool wrote one, its
  generated session title, which is a short summary of the first prompt;
- **agent labels**: the description each subagent was spawned with, which can name
  files, tasks or internal projects;
- tool names, counts, tokens and costs.

So **review a report before you share it in public.**

The JSON layout is stable within a `schema` version. Keys are sorted, durations and
times are milliseconds, and `start_ms` is Unix time. An abridged example:

```json
{
  "agent_count": 2,
  "agents": [
    {
      "cost_usd": 0.0012, "depth": 0, "duration_ms": 60000, "errors": 0,
      "health": "looping", "id": "main", "label": "myproject", "models": ["model-a"],
      "parent": null, "start_ms": 1700000000000, "status": "running", "tool_calls": 3,
      "tokens": { "cache_read": 1000, "cache_write": 20, "cache_write_1h": 8, "cache_write_5m": 12, "input": 100, "output": 50, "total": 170 },
      "type": "main"
    },
    {
      "cost_usd": null, "depth": 1, "duration_ms": 4000, "errors": 1,
      "health": "ok", "id": "a1", "label": "Map the code", "models": [],
      "parent": "main", "start_ms": 1700000001000, "status": "failed", "tool_calls": 2,
      "tokens": { "cache_read": 0, "cache_write": 0, "cache_write_1h": 0, "cache_write_5m": 0, "input": 0, "output": 0, "total": 0 },
      "type": "explorer"
    }
  ],
  "cost": { "lower_bound": true, "unpriced_agents": 1, "usd": 0.0012 },
  "duration_ms": 60000,
  "errors": 1,
  "failures": [
    { "errors": 1, "failed": true, "id": "a1", "label": "Map the code", "type": "explorer" }
  ],
  "flags": [
    { "health": "looping", "id": "main", "label": "myproject", "reason": "loop: Bash x4", "type": "main" }
  ],
  "schema": 1,
  "start_ms": 1700000000000,
  "title": "myproject",
  "tokens": { "cache_read": 1000, "cache_write": 20, "cache_write_1h": 8, "cache_write_5m": 12, "input": 100, "output": 50, "total": 170 },
  "tool_calls": 5,
  "tools": [ { "calls": 3, "tool": "Read" }, { "calls": 2, "tool": "Bash" } ]
}
```

`tokens.total` is the headline number shown in the UI: input, output and cache
writes, without cache reads. `cache_write` is the total of both write kinds, as it
always was; `cache_write_5m` and `cache_write_1h` split it. An agent's `status` is `running` if its session never
recorded a result for it. `start_ms` and `duration_ms` are `null` when the session
has no timestamps. `cost.usd` and `cost_usd` are USD, `null` when nothing could be
priced; `cost.unpriced_agents` counts agents on a model with no known price.
`health` is `ok`, `looping`, `erroring` or `stalled`; `flags` lists the agents that
are not `ok`. Labels are cut at 80 characters in JSON and 60 in Markdown, to keep
the table readable.

### Keys

| Key | Action |
| --- | --- |
| `tab` `shift-tab` | Move focus between graph, inspector, roster and event stream |
| `↑` `↓` or `j` `k` | Graph or roster focused: select an agent. Stream or inspector focused: scroll a line |
| `PgUp` `PgDn` | The same, ten agents or a page at a time |
| `g` `G` or `Home` `End` | Stream: oldest line, back to the newest (follow again). Inspector focused: top, bottom |
| `z` | Show the focused panel full size, or restore it |
| `[` `]` | Shorter or taller event stream (starts at about a third of the terminal height) |
| `<` `>` | Narrower or wider side column (inspector and roster) |
| `/` | Search the event stream: type, `enter` to keep, `esc` to drop |
| `n` `N` | Older or newer match |
| `&` | Show matching lines only, or all lines |
| `f` | Follow the most recently active agent |
| `c` | Switch the roster between agents and cost by agent type |
| `space` | Pause or resume a replay |
| `+` `-` | Replay speed |
| `r` | Replay the session from its start |
| `l` | Jump to the end and follow live |
| `?` | Help |
| `esc` | One step back: close the search line, leave full size, clear the search, then back to the session list |
| `q` | Quit |

Search is a case-insensitive substring match on the agent type and the text of each
line. While the search line is open every key goes to it, so `q` types a `q`.
Scrolling the stream back pauses it: the title shows `PAUSED` and how many lines
arrived since; `G` or scrolling down to the last line follows again. Panel sizes
are kept within limits that leave every panel usable and adapt when the terminal is
resized.

### Cost estimates

The header shows `COST ~$1.24` next to the token count, the inspector shows the
selected agent's cost, and the main agent's inspector shows what its subagents spent
and their share of the session. Press `c` for cost by agent type.

These are **estimates**, marked with `~`. They multiply the token counts in the
transcript by list prices built into neonhive (taken 2026-11-22 from Anthropic's
pricing page). They do not know about discounts, batch or fast mode, data residency
or long-context tiers. Cache writes are priced by cache lifetime: the 5-minute
rate (1.25x input) and the 1-hour rate (2x input), using the split Claude Code records
in each usage entry. An entry with no split is priced as 5-minute. An agent
whose model has no known price shows tokens only and a dash for cost; a trailing `+`
on a total means some agents could not be priced.

There are no OpenAI prices in the built-in table yet, so Codex sessions show tokens
and a dash for cost. Add your own with `--prices` (below) if you want an estimate.

To use your own prices, pass a JSON file of model-id prefixes and USD per million
tokens. Entries replace or extend the built-in table. A key matches every model id
that starts with it, also behind a provider prefix such as `us.anthropic.`, and the
longest matching key wins, whether it is yours or built in:

```sh
neonhive --latest --prices prices.json
```

```json
{
  "claude-sonnet-4-5": { "input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75, "cache_write_1h": 6.0 },
  "my-local-model":    { "input": 0,   "output": 0,    "cache_read": 0,   "cache_write": 0 }
}
```

`cache_write_1h` is optional. When it is missing, 1-hour writes are priced at that
entry's `cache_write` rate, so price files written before it existed keep working.

neonhive never fetches prices from the network.

### Screenshots without a terminal

neonhive can render a frame headlessly, as text or as SVG. The images in this README
are made this way.

```sh
neonhive --demo --snapshot 20                              # text to stdout
neonhive --demo --snapshot 20 --svg shot.svg               # one frame
neonhive --demo --snapshot 14 --svg loop.svg --frames 30   # animated loop
```

## How it works

Agent tools already record their sessions on disk. A source adapter for each tool
tails those files, turns each line into a small normalized event (spawn, tool call,
message, finish), and feeds the events to a renderer that knows nothing about the
tool they came from.

Claude Code records every session as JSON Lines under `~/.claude/projects/`. Each
subagent gets its own transcript beside the main one.

Codex CLI writes one "rollout" file per thread under
`~/.codex/sessions/YYYY/MM/DD/` (or `$CODEX_HOME/sessions`), and compresses files idle
for a week to `.jsonl.zst`. A Codex subagent is a thread with its own rollout file
that names its parent. neonhive reads plain and compressed rollouts the same way.

```
transcript files ──▶ source adapter ──▶ events ──▶ world state ──▶ layout + animation ──▶ terminal
```

That split is deliberate. Supporting another agent tool means writing one adapter,
not touching the UI. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Two things to know:

- The transcript formats are internal to Claude Code and Codex and can change between versions. The parser skips what it does not understand; if a new version breaks something, please [open an issue](https://github.com/juanpalopez/neonhive/issues).
- Claude Code deletes old transcripts after a retention period (the `cleanupPeriodDays` setting). A session that is gone from disk cannot be replayed.

## Supported agent tools

| Tool | Sessions read from | Subagents | Tokens | Cost | Limits |
| --- | --- | --- | --- | --- | --- |
| Claude Code | `~/.claude/projects/` | Yes, with messages between agents | Input, output, cache read, cache write (5-minute and 1-hour) | Estimated from the built-in Claude price table | Old transcripts are deleted after `cleanupPeriodDays` |
| OpenAI Codex CLI | `$CODEX_HOME/sessions/` (default `~/.codex/sessions/`), plain `.jsonl` or `.jsonl.zst` | Yes, as threads that name their parent. Not yet tested on a real multi-agent session | Input, output, cached input | A dash: no OpenAI prices built in yet. Add them with `--prices` | Rollouts written in 2025 have no timestamps or token counts, so they replay as one instant with no tokens |
| Demo (`--demo`) | Built in, scripted | Yes | Yes | Yes | Synthetic data only |

More adapters (Gemini CLI, OpenCode, LangGraph, OpenTelemetry traces) are planned
in [issue #2](https://github.com/juanpalopez/neonhive/issues/2) and open to
contributions. An adapter implements one trait; see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Roadmap

Planned work is tracked as issues on the [roadmap board](https://github.com/users/juanpalopez/projects/5).

- Adapters for more agent tools (Codex CLI is done; Gemini CLI and OpenCode are open)
- Timeline view: which agents ran in parallel and where the time went
- Timeline scrubbing and jump-to-error in replays
- Compare two sessions
- Audit trail: commands run and files touched, per agent
- Zoom and pan for very wide swarms
- Themes
- Prebuilt binaries and Homebrew

## FAQ

**How do I see what my Claude Code or Codex subagents are doing?**
Run `neonhive --latest` while the agent works. Each subagent appears as a node the moment it starts, with its current tool call and token count.

**Does it slow down or interfere with my agent?**
No. It reads files that are already being written and never writes to them.

**Does my data leave my machine?**
No. There is no network code in neonhive.

**Which terminals work?**
Any terminal with Unicode and at least 256 colours, 60x16 cells or larger. It looks best at 140x40 or more with 24-bit colour: iTerm2, Ghostty, Kitty, WezTerm, Alacritty, Windows Terminal. If colours look wrong, try `--ansi256`.

**Which agent tools does it work with?**
Claude Code and the OpenAI Codex CLI today. Sessions from both appear in the same list. The core is tool-agnostic, so other tools need only an adapter; see [Supported agent tools](#supported-agent-tools) and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Contributions are welcome.

## Contributing

Bug reports, adapters and ideas are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)

neonhive is an independent project. It is not affiliated with or endorsed by Anthropic or OpenAI.
