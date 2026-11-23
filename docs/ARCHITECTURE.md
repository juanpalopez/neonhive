# Architecture

neonhive is a pipeline. Each stage knows only about the stage before it. Only the
source adapters know which agent tool wrote a session (Claude Code and Codex CLI
today, plus the synthetic demo), so the rest is the same for every tool.

```
transcript files ──▶ Source ──▶ Event ──▶ World ──▶ App ──▶ ui
                     claude.rs  event.rs  state.rs  app.rs  ui.rs
                     codex.rs
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
| `Usage { agent, tokens, model }` | Tokens used by one model response, split into input, output, cache read and cache write (5-minute and 1-hour kept apart) |
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
- A background subagent finishes with a task notification in its parent's transcript, written as an attachment, as a queue operation, as a user turn that is the notification, or several of these at once; a foreground subagent finishes with a tool result. All become `Finish`. A notification only counts when it opens the text, so one quoted inside a file or command output does not.
- Every `Tool` event carries a `key`: a stable 64-bit hash of the tool name and the full input JSON. The display `detail` is lossy (a basename, a truncated command), so health compares keys, never details.
- Token usage is repeated on every line of a multi-line model response, so it is counted once per response id.
- Cache writes come as a combined count plus an optional 5-minute / 1-hour split (`cache_creation`). Both parts are deduplicated per response id like the rest. A line with no split counts as 5-minute; when the parts disagree with the combined count, the combined count wins and the 1-hour part is capped at it (`Tokens::split_cache_write`).
- The token count shown for an agent is input + output + cache writes. Cache reads are tracked but left out of that number: a session re-reads the same cached context on every turn, so adding reads would count it once per turn. The inspector shows all four.

This format is not a published contract. The adapter treats every field as
optional and skips lines it cannot parse.

### Codex CLI (`src/codex.rs`)

Codex writes one "rollout" file per thread:

```
$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread id>.jsonl       CODEX_HOME defaults to ~/.codex
$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread id>.jsonl.zst   the same file once idle for 7 days
$CODEX_HOME/session_index.jsonl                                        {id, thread_name, updated_at}, last one wins
```

The format was learned from rollout files written by Codex CLI 0.163 to 0.171
(2026) and by releases from August to October 2025, and from the open-source
Codex repository (`codex-rs/protocol`, `codex-rs/rollout`). Two line formats
exist, and the adapter handles both:

- **Legacy** (2025): the first line is `{id, timestamp, instructions, git}`; every later line is a bare response item (`message`, `reasoning`, `function_call`, `function_call_output`) or a `{record_type: "state"}` marker. Lines have no timestamp of their own and there is no token usage, so every event takes the header's time and the session shows no tokens.
- **Envelope**: every line is `{timestamp, type, payload}`. `session_meta` opens the file (thread id, cwd, source, parent thread), `turn_context` carries the model, `response_item` is model input and output, `event_msg` holds events. Within this format, `session_meta.history_mode` is `legacy` (events such as `user_message` and `agent_message`) or `paginated` (events are `item_completed` with a typed item). All local 2026 files are paginated; the legacy history mode is handled from the source code only.

The adapter is picked per file in `main.rs`: a name `rollout-*.jsonl`, a `.jsonl.zst`
name, or a first line that is `session_meta` or a legacy header selects Codex;
anything else is Claude Code. Plain files are tailed like Claude transcripts
(byte offset, carry buffer for a partial line). Compressed files are cold, so they
are decoded once with `ruzstd`.

How records become events:

| Record | Event |
| --- | --- |
| `event_msg` `item_completed` `UserMessage`; `event_msg` `user_message`; legacy bare `message` with role `user` (skipping injected `<environment_context>` and `<user_instructions>`) | `Prompt` |
| `response_item` `message` with role `assistant` | `Text` |
| `function_call`, `custom_tool_call`, `local_shell_call`, `web_search_call` | `Tool`. `shell` shows the script of `sh -lc <script>`; a patch, through the shell or the `apply_patch` tool, shows the first file and how many more; anything else shows its first string argument. MCP calls are named `<namespace>.<name>`. |
| Code-mode `exec` call (a `custom_tool_call` whose input is a script) | Held back. The commands, patches and MCP calls the script makes arrive as `item_completed` items between the call and its output, and those are shown. A script with no such item is shown as `exec` when its output arrives, at the time of the call. |
| `item_completed` `CommandExecution`, `FileChange`, `McpToolCall`, `DynamicToolCall`, `WebSearch`, `ImageView`, `ImageGeneration`, `Extension` (`web.search`, `image_gen.generation`), `CollabAgentToolCall` | `Tool`. A command whose `parsed_cmd` is a read, a search or a listing is shown as `read`, `search` or `list_files`, otherwise `shell`. An item whose id is a call already shown (a direct MCP call) adds no second `Tool`. |
| Item `status` `failed` or `declined`, a non-zero `exit_code`, MCP `isError` or `error`, `success: false`, an extension `failure`; a script output starting `Script failed`; a legacy shell output whose `metadata.exit_code` is not zero | `ToolError` |
| `event_msg` `token_count` | `Usage`, see below |
| A rollout file whose `session_meta` names a parent thread | `Spawn` |
| `event_msg` `task_complete` / `turn_aborted` in a subagent's file | `Finish` (`ok` true / false). The top-level agent waits for the human and never finishes. |
| `inter_agent_communication`, `response_item` `agent_message` | `Message`, when both agent paths are known; counted once if it is written in both agents' files |

Reasoning (encrypted, with optional summaries), `world_state`, `compacted`,
`token_usage_record`, `thread_settings_applied` and `additional_tools` lines are
skipped, as is everything unknown.

Every `Tool` carries a `key` hashed from the full input: the tool name and the
arguments JSON for calls, the script text for `exec`, and for completed items the
item with its id, timing, status and results removed. Two runs of the same
command therefore match even though their ids and outcomes differ.

**Tokens.** `token_count.info.total_token_usage` is cumulative for the thread, and
the same total is often written twice (a rate-limit update repeats it). The adapter
emits the increase since the previous total; when a total goes down it uses
`last_token_usage` instead. Codex counts cached tokens inside `input_tokens` (its
own display uses input minus cached as the uncached part; locally,
`total_tokens` = input + output in 319 of 321 records), and reasoning tokens inside
`output_tokens`. So `Tokens.input` = `input_tokens` - `cached_input_tokens`,
`Tokens.cache_read` = `cached_input_tokens`, `Tokens.output` = `output_tokens`.
Codex also records `cache_write_input_tokens`. It is zero in every local file, its
relation to `input_tokens` is not documented, and OpenAI's cache lifetimes (30
minutes, 12 hours) do not match the 5-minute / 1-hour split in `Tokens`, so cache
writes are left at zero. Summed increases matched the final cumulative total of
every local session exactly. `token_usage_record` lines repeat the same usage per
response and are skipped so nothing is counted twice.

**Subagents.** Codex has them. A subagent is a thread of its own with its own
rollout file in the same date tree. Its `session_meta` names the parent in
`parent_thread_id` or in `source.subagent.thread_spawn.parent_thread_id`, and
carries `agent_role`, `agent_nickname` and `agent_path` (`/root/<name>`; the
top-level agent is `/root`). Every 2 seconds the adapter reads the first line of
rollout files dated on or after the session's day; a thread whose parent is the
session or one of its subagents becomes a `Spawn` (id: the thread id, type: the
role, label: the nickname or path). Its file is then tailed like the root. The
session list hides subagent threads and counts them toward the session they
descend from. None of the local sessions used subagents, so this path is built
from the source code and tested with synthetic files only. Not handled yet:
`CollabAgentToolCall` states (`agents_states`) as a second source of `Finish`,
forks (`forked_from_id`), and review or compaction threads that name no parent.

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

Cost is derived, not stored: `Agent::cost` prices each entry of the agent's
per-model token split with `pricing::cost`, and `World::cost_summary` and
`World::cost_by_type` add those up. `src/pricing.rs` holds the price table as
data (model id families and their rates), merged with an optional `--prices`
file; nothing else knows which vendor a model belongs to.
### Health (`src/health.rs`)

Each agent carries a small `Tracker` that `apply` feeds with its tool calls,
tool errors, text and messages. `World::health` combines it with the world clock
into a flag: `Erroring` (3 failed calls in a row), `Looping` (the same call key
4 times in a row, each repeat issued within 60 seconds of the previous one) or
`Stalled` (running but silent for 10 minutes after text, or 30 after a tool
call or message, which may be a long build). The repeat window separates loops
from polling: on real transcripts, identical calls re-issued without waiting
came 2 to 10 seconds apart, while polls that waited were at least 256 seconds
apart. Silence counts from the last activity, tool errors included, and an
error does not end the wait for a call: a sibling call may still be running.
The main agent, which waits for the human, any agent with a running subagent,
and an agent that finished once and was resumed (its second completion is often
not written) are never stalled. Errors arrive after the call they belong to, so
a call followed by another call or by text with no error between counts as a
success and ends a streak; with parallel calls a sibling's success between two
errors is not seen, so failures from one batch add up. Time is
`World::now_ts` or event time, never the wall clock, so replays flag the same
agents at the same moments. `apply` re-checks the agent an event belongs to and
logs a line when it is first flagged; stalls happen with no event at all, so the
app also calls `sweep_health` after it moves the clock. The flag is stored on
the agent, so views read it instead of re-assessing every frame.

`apply` also queues animation requests (`Fx`): a packet between two agents or a
flash on one. When history is loaded in bulk these are suppressed.

## App (`src/app.rs`)

The view model. It owns:

- **Playback.** In live mode every event is applied as it arrives. In replay mode a virtual clock advances at the chosen speed and events are applied when the clock passes them. Gaps longer than one second of wall time are skipped.
- **Visibility.** `choose_visible` picks which agents get a node: active ones first, then the most recently active, up to a cap derived from the panel size, plus any ancestors needed to draw edges.
- **Layout.** `layout` places nodes radially (leaves on the rim, a node with a subtree part-way out), then `separate` pushes overlapping boxes apart in cell space.
- **Panes.** Focus, panel sizes, full size, scroll positions and the event-stream search (`Panes`). The stream scroll position is a line number (`World::log_seq` numbering), so it stays on the same line while new lines arrive and old ones are dropped. Keys are mapped to these in `main.rs` (`handle_key`), which tests drive without a terminal; `ui` only reads them and writes back what it measured.
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
4. Add session discovery if the tool stores sessions in a known place (return `claude::SessionInfo` with your tool's label in `tool`), add it to `all_sessions` in `main.rs`, and teach `source()` there to recognise your files by name or first line.
5. Add tests that build small synthetic transcripts in a temp directory. Do not commit real transcripts.

Nothing in `state.rs`, `app.rs` or `ui.rs` should need to change.
