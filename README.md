<h1 align="center">neonhive</h1>

<p align="center">
  <b>Watch your AI agents think.</b><br>
  A cyberpunk terminal UI that visualizes how Claude Code agents and subagents spawn, talk and finish, live or as a replay.
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
neonhive shows you the swarm: every agent is a node, every spawn, message and result
is a pulse travelling along an edge, and every tool call scrolls past in an event stream.

Try it in ten seconds, no agent sessions needed:

```sh
cargo install neonhive
neonhive --demo
```

## Features

- **Live agent graph.** Main agent in the centre, subagents around it, nested subagents further out. Nodes glitch into existence, pulse while they work and dim when they finish.
- **Message flow.** Coloured packets travel along edges: magenta for a spawn, green for a returned result, yellow for a message between agents, red for a failure.
- **Replay.** Open any past session and watch it again at 1x to 128x. Dead time is skipped, so a three-hour session takes minutes.
- **Inspector.** Select an agent to see its prompt, model, token count, tool calls, errors and last output.
- **Event stream.** A running log of spawns, tool calls, messages and results, coloured by agent.
- **Scales to big sessions.** Hundreds of subagents stay readable: active agents hold the screen, finished ones dissolve into an archive counter.
- **Read-only and local.** neonhive only reads transcript files that Claude Code already writes to your disk. It sends nothing over the network and changes no files.
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
neonhive                    # pick a session from a list
neonhive --latest           # open the most recent session
neonhive path/to/session.jsonl
neonhive --demo             # endless synthetic swarm
```

A session that Claude Code wrote to in the last two minutes opens **live** and follows
new activity. An older session opens as a **replay** from its start. Force either with
`--live` or `--replay`, and set the replay speed with `--speed 32`.

Start Claude Code in one terminal, run `neonhive --latest` in another, and ask Claude
to do something that uses subagents.

### Keys

| Key | Action |
| --- | --- |
| `↑` `↓` or `j` `k` | Select an agent |
| `f` | Follow the most recently active agent |
| `space` | Pause or resume a replay |
| `+` `-` | Replay speed |
| `r` | Replay the session from its start |
| `l` | Jump to the end and follow live |
| `?` | Help |
| `esc` | Back to the session list |
| `q` | Quit |

### Screenshots without a terminal

neonhive can render a frame headlessly, as text or as SVG. The images in this README
are made this way.

```sh
neonhive --demo --snapshot 20                              # text to stdout
neonhive --demo --snapshot 20 --svg shot.svg               # one frame
neonhive --demo --snapshot 14 --svg loop.svg --frames 30   # animated loop
```

## How it works

Claude Code records every session as JSON Lines under `~/.claude/projects/`. Each
subagent gets its own transcript beside the main one. neonhive tails those files,
turns each line into a small normalized event (spawn, tool call, message, finish),
and feeds the events to a renderer that knows nothing about Claude Code.

```
transcript files ──▶ source adapter ──▶ events ──▶ world state ──▶ layout + animation ──▶ terminal
```

That split is deliberate. Supporting another agent tool means writing one adapter,
not touching the UI. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Two things to know:

- The transcript format is internal to Claude Code and can change between versions. The parser skips what it does not understand; if a new version breaks something, please [open an issue](https://github.com/juanpalopez/neonhive/issues).
- Claude Code deletes old transcripts after a retention period (the `cleanupPeriodDays` setting). A session that is gone from disk cannot be replayed.

## Supported agent tools

| Tool | Status |
| --- | --- |
| Claude Code | Supported |
| OpenAI Codex CLI | Planned |
| Others (Gemini CLI, OpenCode, LangGraph, OpenTelemetry traces) | Open to contributions |

## Roadmap

- Adapters for more agent tools, starting with Codex CLI
- Zoom and pan for very wide swarms
- Timeline scrubbing in replays
- Themes
- Prebuilt binaries and Homebrew

## FAQ

**How do I see what my Claude Code subagents are doing?**
Run `neonhive --latest` while Claude Code works. Each subagent appears as a node the moment it starts, with its current tool call and token count.

**Does it slow down or interfere with my agent?**
No. It reads files that are already being written and never writes to them.

**Does my data leave my machine?**
No. There is no network code in neonhive.

**Which terminals work?**
Any terminal with Unicode and at least 256 colours, 60x16 cells or larger. It looks best at 140x40 or more with 24-bit colour: iTerm2, Ghostty, Kitty, WezTerm, Alacritty, Windows Terminal. If colours look wrong, try `--ansi256`.

**Can I use it with agents other than Claude Code?**
Not yet. The core is tool-agnostic, and an adapter is about 300 lines. Contributions are welcome.

## Contributing

Bug reports, adapters and ideas are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)

neonhive is an independent project. It is not affiliated with or endorsed by Anthropic.
