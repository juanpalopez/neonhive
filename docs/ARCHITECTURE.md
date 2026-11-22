# Architecture

neonhive is a pipeline. Each stage knows only about the stage before it.

```
transcript files ──▶ Source ──▶ Event ──▶ World ──▶ App ──▶ ui
                     claude.rs  event.rs  state.rs  app.rs  ui.rs
                     demo.rs
```

## Events (`src/event.rs`)

Everything the UI can show is one of eight events:

| Event | Meaning |
| --- | --- |
| `Spawn { id, parent, agent_type, label }` | A subagent started |
| `Finish { id, ok, tokens }` | A subagent returned or failed |
| `Message { from, to, summary }` | One agent sent a message to another |
| `Tool { agent, tool, detail }` | An agent called a tool |
| `ToolError { agent }` | A tool call failed |
| `Usage { agent, tokens, model }` | Tokens used by one model response |
| `Text { agent, text }` | An agent wrote text |
| `Prompt { agent, text }` | An agent received its prompt |

Each event carries a timestamp in Unix milliseconds. The top-level agent always
has the id `main` (`event::ROOT`).

## Sources

A source implements one method:

```rust
pub trait Source {
    fn poll(&mut self) -> Vec<Event>;
}
```

The first call returns the whole history of the session. Later calls return only
what is new. The app calls `poll` four times a second.

### Claude Code (`src/claude.rs`)

A Claude Code session is a set of JSON Lines files:

```
~/.claude/projects/<project>/<session>.jsonl                          main agent
~/.claude/projects/<project>/<session>/subagents/agent-<id>.jsonl     one per subagent
~/.claude/projects/<project>/<session>/subagents/agent-<id>.meta.json type, description, spawning tool call
```

The adapter keeps a byte offset per file and reads only appended bytes. A line
that is not yet complete is held back until the rest arrives.

How lines become events:

- An `assistant` line holds content blocks. A `tool_use` block becomes a `Tool` event, a `SendMessage` tool call becomes a `Message`, a `text` block becomes `Text`. An `Agent` tool call is remembered: its id tells us later who spawned whom.
- A new `agent-<id>.jsonl` file becomes a `Spawn`. The parent is whoever made the tool call named in the `.meta.json` file. This is how nested subagents get the right parent.
- A background subagent finishes with a task notification attached to its parent's transcript; a foreground subagent finishes with a tool result. Both become `Finish`.
- Token usage is repeated on every line of a multi-line model response, so it is counted once per response id.

This format is not a published contract. The adapter treats every field as
optional and skips lines it cannot parse.

### Demo (`src/demo.rs`)

A scripted generator. Its time advances by 250 ms per `poll` instead of following
the wall clock, so a given seed always produces the same frames. Tests and
screenshots rely on that.

## World (`src/state.rs`)

`World::apply` folds events into state: a map of agents, their spawn order, a
bounded log and running totals. It is tolerant of disorder:

- An event for an agent that has not spawned yet creates a placeholder; the real `Spawn` fills it in.
- A repeated `Finish` is ignored.
- Activity from a finished agent marks it running again (it was resumed).
- A `Finish` for an unknown id is dropped (background shell tasks send the same notification as agents).

`apply` also queues animation requests (`Fx`): a packet between two agents or a
flash on one. When history is loaded in bulk these are suppressed.

## App (`src/app.rs`)

The view model. It owns:

- **Playback.** In live mode every event is applied as it arrives. In replay mode a virtual clock advances at the chosen speed and events are applied when the clock passes them. Gaps longer than one second of wall time are skipped.
- **Visibility.** `choose_visible` picks which agents get a node: active ones first, then the most recently active, up to a cap derived from the panel size, plus any ancestors needed to draw edges.
- **Layout.** `layout` places nodes radially (leaves on the rim, a node with a subtree part-way out), then `separate` pushes overlapping boxes apart in cell space.
- **Animation.** Node positions ease toward their layout targets. New nodes start at their parent's position. Nodes that leave the visible set dissolve before removal.

All animation runs on one clock that `tick(dt)` advances, never on the wall clock.
That makes every frame reproducible in tests.

## Rendering (`src/ui.rs`)

Draws straight into the ratatui buffer. Edges and packet trails use a small
braille canvas (2x4 dots per cell) so lines have sub-cell resolution. Node boxes
are composed cell by cell, which is what lets the spawn and dissolve effects
replace individual characters.

## Adding an adapter for another agent tool

1. Add `src/<tool>.rs` with a type that implements `Source`.
2. Map the tool's records to the eight events. Use the agent's own id for `id`, and `event::ROOT` for the top-level agent.
3. Make `poll` incremental: return history on the first call and only new events afterwards.
4. Add session discovery if the tool stores sessions in a known place, and wire a flag or auto-detection into `main.rs`.
5. Add tests that build small synthetic transcripts in a temp directory. Do not commit real transcripts.

Nothing in `state.rs`, `app.rs` or `ui.rs` should need to change.
