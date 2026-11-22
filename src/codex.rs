//! OpenAI Codex CLI source adapter.
//!
//! Codex writes one "rollout" file per thread under
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread id>.jsonl`
//! (`CODEX_HOME` defaults to `~/.codex`). Files that have been idle for a week
//! are compressed in place to `.jsonl.zst`. Two line formats exist:
//!
//! - **Legacy** (Codex CLI 0.2x/0.3x, 2025): the first line is
//!   `{id, timestamp, instructions, git}` and every later line is a bare
//!   response item (`message`, `function_call`, ...) or `{record_type}`. Lines
//!   carry no timestamp and there is no token usage.
//! - **Envelope** (later releases): every line is
//!   `{timestamp, type, payload}`. `session_meta` opens the file, model output
//!   arrives as `response_item`, and `event_msg` holds user messages, token
//!   counts, turn ends and (in "paginated" history) completed tool items.
//!
//! A subagent is a thread of its own with its own rollout file; its
//! `session_meta` names the parent thread. The format is not a published
//! contract, so parsing is tolerant: unknown lines and fields are skipped.

use crate::claude::SessionInfo;
use crate::event::{Event, Kind, Source, Tokens, ROOT};
use crate::util::{clean, parse_ts, str_hash64};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Label shown in the session list.
pub const TOOL: &str = "codex";

/// Polls between scans for new subagent files (the app polls 4 times a second).
const SCAN_EVERY: u32 = 8;

/// Codex's agent path for the top-level agent.
const ROOT_PATH: &str = "/root";

pub struct CodexSource {
    root: Agent,
    children: Vec<Agent>,
    /// `sessions` directory and the `YYYY/MM/DD` of the root file, when the
    /// root lives in the usual tree. Subagent files are only looked for there.
    tree: Option<(PathBuf, String)>,
    /// Rollout files already read for subagent metadata.
    examined: HashMap<PathBuf, Option<Meta>>,
    /// Thread id -> agent id.
    threads: HashMap<String, String>,
    /// Codex agent path (`/root/worker`) -> agent id.
    paths: HashMap<String, String>,
    spawns: Vec<Event>,
    messages: HashSet<u64>,
    polls: u32,
}

impl CodexSource {
    pub fn open(path: &Path) -> Self {
        let tree = path.parent().and_then(|day| {
            let parts: Vec<&std::ffi::OsStr> = day.iter().collect();
            let n = parts.len();
            (n >= 4 && parts[n - 4] == std::ffi::OsStr::new("sessions")).then(|| {
                let date = parts[n - 3..]
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                (day.ancestors().nth(3).unwrap_or(day).to_path_buf(), date)
            })
        });
        let mut paths = HashMap::new();
        paths.insert(ROOT_PATH.to_string(), ROOT.to_string());
        Self {
            root: Agent::new(path.to_path_buf(), ROOT.to_string()),
            children: Vec::new(),
            tree,
            examined: HashMap::new(),
            threads: HashMap::new(),
            paths,
            spawns: Vec::new(),
            messages: HashSet::new(),
            polls: 0,
        }
    }

    /// Looks for rollout files of threads spawned by this session's agents.
    fn scan(&mut self) {
        let (Some((dir, from)), Some(root_id)) = (&self.tree, &self.root.st.thread_id) else {
            return;
        };
        self.threads
            .entry(root_id.clone())
            .or_insert_with(|| ROOT.to_string());
        for path in rollout_files(dir, from) {
            if path == self.root.lines.path || self.examined.contains_key(&path) {
                continue;
            }
            // A file whose first line is not complete yet is tried again later.
            if let Some(line) = first_line(&path) {
                let meta = parse_meta(&line).filter(|m| m.parent.is_some());
                self.examined.insert(path, meta);
            }
        }
        // Attach children whose parent is known, until nothing changes, so a
        // grandchild found before its parent still gets attached.
        loop {
            let ready: Vec<(PathBuf, Meta, String)> = self
                .examined
                .iter()
                .filter_map(|(path, meta)| {
                    let m = meta.as_ref()?;
                    let parent = self.threads.get(m.parent.as_deref()?)?;
                    (!self.threads.contains_key(&m.thread_id))
                        .then(|| (path.clone(), m.clone(), parent.clone()))
                })
                .collect();
            if ready.is_empty() {
                break;
            }
            for (path, m, parent) in ready {
                let id = m.thread_id.clone();
                self.threads.insert(id.clone(), id.clone());
                if !m.agent_path.is_empty() {
                    self.paths.insert(m.agent_path.clone(), id.clone());
                }
                let label = if m.nickname.is_empty() {
                    m.agent_path.clone()
                } else {
                    m.nickname.clone()
                };
                self.spawns.push(Event {
                    ts: m.ts.unwrap_or(self.root.st.last_ts),
                    kind: Kind::Spawn {
                        id: id.clone(),
                        parent,
                        agent_type: if m.role.is_empty() {
                            "agent".into()
                        } else {
                            m.role
                        },
                        label,
                    },
                });
                self.children.push(Agent::new(path, id));
            }
        }
    }
}

impl Source for CodexSource {
    fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        let mut ctx = Ctx {
            paths: &self.paths,
            messages: &mut self.messages,
        };
        self.root.read(&mut ctx, &mut out);
        if self.polls.is_multiple_of(SCAN_EVERY) {
            self.scan();
        }
        self.polls = self.polls.wrapping_add(1);
        let mut ctx = Ctx {
            paths: &self.paths,
            messages: &mut self.messages,
        };
        for child in &mut self.children {
            child.read(&mut ctx, &mut out);
        }
        let mut events = std::mem::take(&mut self.spawns);
        events.append(&mut out);
        events.sort_by_key(|e| e.ts); // stable: a spawn stays ahead of same-time events
        events
    }
}

/// What every file of a session shares while lines are parsed.
struct Ctx<'a> {
    paths: &'a HashMap<String, String>,
    /// Inter-agent messages already emitted; one may be written in the
    /// sender's and in the recipient's file.
    messages: &'a mut HashSet<u64>,
}

struct Agent {
    lines: Lines,
    st: State,
}

impl Agent {
    fn new(path: PathBuf, id: String) -> Self {
        Self {
            lines: Lines::new(path),
            st: State::new(id),
        }
    }

    fn read(&mut self, ctx: &mut Ctx, out: &mut Vec<Event>) {
        let st = &mut self.st;
        self.lines.read(|line| st.parse(line, ctx, out));
    }
}

// ---------------------------------------------------------------- reading

/// Incremental line reader. Plain files are tailed from a byte offset and an
/// incomplete last line is held back; compressed files are cold (Codex only
/// compresses idle sessions), so they are decoded once.
struct Lines {
    path: PathBuf,
    offset: u64,
    carry: Vec<u8>,
    done: bool,
}

impl Lines {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            carry: Vec::new(),
            done: false,
        }
    }

    fn read(&mut self, mut each: impl FnMut(&str)) {
        if is_compressed(&self.path) {
            if !self.done {
                self.done = true;
                let buf = decompress(&self.path);
                for line in buf.split(|&b| b == b'\n') {
                    if let Ok(line) = std::str::from_utf8(line) {
                        each(line);
                    }
                }
            }
            return;
        }
        let Ok(mut file) = File::open(&self.path) else {
            return;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0;
            self.carry.clear();
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = std::mem::take(&mut self.carry);
        let Ok(n) = file.read_to_end(&mut buf) else {
            self.carry = buf;
            return;
        };
        self.offset += n as u64;
        let mut start = 0;
        while let Some(pos) = buf[start..].iter().position(|&b| b == b'\n') {
            if let Ok(line) = std::str::from_utf8(&buf[start..start + pos]) {
                each(line);
            }
            start += pos + 1;
        }
        buf.drain(..start);
        self.carry = buf; // incomplete last line, finished on a later poll
    }
}

fn is_compressed(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("zst")
}

/// Decodes every zstd frame of a file. A corrupt or truncated file yields
/// what could be decoded before the damage.
fn decompress(path: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    let Ok(file) = File::open(path) else {
        return out;
    };
    let mut src = BufReader::new(file);
    while let Ok(mut dec) = ruzstd::decoding::StreamingDecoder::new(&mut src) {
        if dec.read_to_end(&mut out).is_err() {
            break;
        }
    }
    out
}

/// The first complete line of a rollout file, read without loading the rest.
fn first_line(path: &Path) -> Option<String> {
    const MAX: usize = 4 << 20; // session_meta embeds the base instructions
    let file = File::open(path).ok()?;
    let mut src: Box<dyn Read> = if is_compressed(path) {
        Box::new(ruzstd::decoding::StreamingDecoder::new(BufReader::new(file)).ok()?)
    } else {
        Box::new(file)
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = src.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        let had = buf.len();
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf[had..].iter().position(|&b| b == b'\n') {
            buf.truncate(had + pos);
            return String::from_utf8(buf).ok();
        }
        if buf.len() > MAX {
            return None;
        }
    }
}

// ---------------------------------------------------------------- parsing

/// Per-file parse state.
struct State {
    agent: String,
    thread_id: Option<String>,
    last_ts: i64,
    model: String,
    /// Last cumulative token count, to turn running totals into increments.
    total: Option<Raw>,
    /// Ids of calls already shown as a `Tool`.
    shown: HashSet<String>,
    /// Code-mode `exec` calls waiting for their output. Such a call runs a
    /// script; the commands, patches and MCP calls it makes arrive as
    /// completed items and are shown instead. An `exec` with no such item is
    /// shown when its output arrives.
    execs: HashMap<String, Pending>,
    /// The latest `exec` call still running: completed items belong to it.
    open_exec: Option<String>,
}

struct Pending {
    ts: i64,
    detail: String,
    key: u64,
    effects: bool,
}

/// Token counts as Codex reports them: `input` includes `cached`.
#[derive(Clone, Copy, Default, PartialEq)]
struct Raw {
    input: u64,
    cached: u64,
    output: u64,
}

impl Raw {
    fn from(v: &Value) -> Option<Raw> {
        v.as_object()?;
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        Some(Raw {
            input: n("input_tokens"),
            cached: n("cached_input_tokens"),
            output: n("output_tokens"),
        })
    }

    /// `self - prev`, or `None` when any count went down (a reset).
    fn since(&self, prev: &Raw) -> Option<Raw> {
        Some(Raw {
            input: self.input.checked_sub(prev.input)?,
            cached: self.cached.checked_sub(prev.cached)?,
            output: self.output.checked_sub(prev.output)?,
        })
    }

    /// Codex counts cached tokens inside `input_tokens` (as the OpenAI API
    /// does), and reasoning tokens inside `output_tokens`.
    fn tokens(&self) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(self.cached),
            output: self.output,
            cache_read: self.cached.min(self.input),
            ..Tokens::default()
        }
    }
}

impl State {
    fn new(agent: String) -> Self {
        Self {
            agent,
            thread_id: None,
            last_ts: 0,
            model: String::new(),
            total: None,
            shown: HashSet::new(),
            execs: HashMap::new(),
            open_exec: None,
        }
    }

    fn parse(&mut self, line: &str, ctx: &mut Ctx, out: &mut Vec<Event>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if !v.is_object() {
            return;
        }
        if let (Some(kind), true) = (v["type"].as_str(), v["payload"].is_object()) {
            // Envelope format.
            if let Some(ts) = v["timestamp"].as_str().and_then(parse_ts) {
                self.last_ts = ts;
            }
            let p = &v["payload"];
            match kind {
                "session_meta" => {
                    if let Some(id) = p["id"].as_str() {
                        self.thread_id.get_or_insert_with(|| id.to_string());
                    }
                }
                "turn_context" => {
                    if let Some(m) = p["model"].as_str() {
                        self.model = m.to_string();
                    }
                }
                "response_item" => self.item(p, true, ctx, out),
                "event_msg" => self.event(p, out),
                "inter_agent_communication" => self.message(p, ctx, out),
                _ => {}
            }
        } else if v["type"].is_string() {
            // Legacy format: a bare response item, with no time of its own.
            self.item(&v, false, ctx, out);
        } else if let (Some(id), Some(ts)) =
            (v["id"].as_str(), v["timestamp"].as_str().and_then(parse_ts))
        {
            // Legacy session header.
            self.thread_id.get_or_insert_with(|| id.to_string());
            self.last_ts = ts;
        }
    }

    fn push(&self, out: &mut Vec<Event>, kind: Kind) {
        out.push(Event {
            ts: self.last_ts,
            kind,
        });
    }

    fn tool(&self, out: &mut Vec<Event>, ts: i64, tool: String, detail: &str, key: u64) {
        out.push(Event {
            ts,
            kind: Kind::Tool {
                agent: self.agent.clone(),
                tool,
                detail: clean(detail, 80),
                key,
            },
        });
    }

    fn error(&self, out: &mut Vec<Event>) {
        self.push(
            out,
            Kind::ToolError {
                agent: self.agent.clone(),
            },
        );
    }

    /// A model output item: a message, a tool call or a tool result.
    fn item(&mut self, p: &Value, envelope: bool, ctx: &mut Ctx, out: &mut Vec<Event>) {
        let call_id = p["call_id"].as_str().unwrap_or("").to_string();
        match p["type"].as_str().unwrap_or("") {
            "message" => {
                let role = p["role"].as_str().unwrap_or("");
                for block in p["content"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let text = block["text"].as_str().unwrap_or("").trim_start();
                    if text.is_empty() {
                        continue;
                    }
                    match (role, block["type"].as_str().unwrap_or("")) {
                        ("assistant", "output_text" | "text") => self.push(
                            out,
                            Kind::Text {
                                agent: self.agent.clone(),
                                text: clean(text, 400),
                            },
                        ),
                        // The envelope format records what the user typed as
                        // an event; its user items also hold injected context.
                        // Legacy files only have the item: skip the injected
                        // `<environment_context>` and `<user_instructions>`.
                        ("user", "input_text") if !envelope && !text.starts_with('<') => self.push(
                            out,
                            Kind::Prompt {
                                agent: self.agent.clone(),
                                text: clean(text, 400),
                            },
                        ),
                        _ => {}
                    }
                }
            }
            "function_call" => {
                let name = p["name"].as_str().unwrap_or("?");
                let raw = p["arguments"].as_str().unwrap_or("");
                let args: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
                let full = match p["namespace"].as_str() {
                    Some(ns) if !ns.is_empty() => format!("{ns}.{name}"),
                    _ => name.to_string(),
                };
                let (tool, detail) = match (name, args["command"].as_array()) {
                    ("shell" | "container.exec", Some(_)) => shell_tool(&strings(&args["command"])),
                    _ => (full.clone(), first_string(&args)),
                };
                let input = if args.is_null() {
                    raw.to_string()
                } else {
                    args.to_string()
                };
                self.shown.insert(call_id);
                self.tool(
                    out,
                    self.last_ts,
                    tool,
                    &detail,
                    str_hash64(&[&full, &input]),
                );
            }
            "custom_tool_call" => {
                let name = p["name"].as_str().unwrap_or("?");
                let input = p["input"].as_str().unwrap_or("");
                let key = str_hash64(&[name, input]);
                if name == "exec" {
                    let detail = input.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    self.execs.insert(
                        call_id.clone(),
                        Pending {
                            ts: self.last_ts,
                            detail: detail.trim().to_string(),
                            key,
                            effects: false,
                        },
                    );
                    self.open_exec = Some(call_id);
                    return;
                }
                let (tool, detail) = if name == "apply_patch" {
                    ("apply_patch".to_string(), patch_files(input))
                } else {
                    (name.to_string(), clean(input, 80))
                };
                self.shown.insert(call_id);
                self.tool(out, self.last_ts, tool, &detail, key);
            }
            "local_shell_call" => {
                let cmd = strings(&p["action"]["command"]);
                let (tool, detail) = shell_tool(&cmd);
                self.shown.insert(call_id);
                let key = str_hash64(&["shell", &p["action"]["command"].to_string()]);
                self.tool(out, self.last_ts, tool, &detail, key);
            }
            "web_search_call" => {
                let a = &p["action"];
                let q = a["query"]
                    .as_str()
                    .or(a["queries"][0].as_str())
                    .unwrap_or("");
                self.tool(
                    out,
                    self.last_ts,
                    "web_search".into(),
                    q,
                    str_hash64(&["web_search", &a.to_string()]),
                );
            }
            "function_call_output" | "custom_tool_call_output" => {
                let text = output_text(&p["output"]);
                if let Some(call) = self.execs.remove(&call_id) {
                    if self.open_exec.as_deref() == Some(call_id.as_str()) {
                        self.open_exec = None;
                    }
                    if !call.effects {
                        self.tool(out, call.ts, "exec".into(), &call.detail, call.key);
                    }
                    if text.starts_with("Script failed") {
                        self.error(out);
                    }
                    return;
                }
                // Legacy shell output: `{"output": ..., "metadata": {"exit_code": N}}`.
                let failed = text.starts_with('{')
                    && serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|o| o["metadata"]["exit_code"].as_i64())
                        .is_some_and(|c| c != 0);
                if failed {
                    self.error(out);
                }
            }
            "agent_message" => self.message(p, ctx, out),
            _ => {}
        }
    }

    /// An `event_msg` payload.
    fn event(&mut self, p: &Value, out: &mut Vec<Event>) {
        match p["type"].as_str().unwrap_or("") {
            "user_message" => {
                let text = p["message"].as_str().unwrap_or("").trim();
                if !text.is_empty() {
                    self.push(
                        out,
                        Kind::Prompt {
                            agent: self.agent.clone(),
                            text: clean(text, 400),
                        },
                    );
                }
            }
            "item_completed" => self.completed(&p["item"], out),
            "token_count" => {
                let info = &p["info"];
                let Some(total) = Raw::from(&info["total_token_usage"]) else {
                    return;
                };
                // Totals are cumulative and the same count can be written
                // again (for a rate-limit update); emit only the increase.
                let new = match self.total {
                    None => Some(total),
                    Some(prev) => total.since(&prev),
                }
                .or_else(|| Raw::from(&info["last_token_usage"]))
                .unwrap_or_default();
                self.total = Some(total);
                let tokens = new.tokens();
                if !tokens.is_zero() {
                    self.push(
                        out,
                        Kind::Usage {
                            agent: self.agent.clone(),
                            tokens,
                            model: self.model.clone(),
                        },
                    );
                }
            }
            "task_complete" | "turn_aborted" if self.agent != ROOT => {
                let ok = p["type"] == "task_complete";
                self.push(
                    out,
                    Kind::Finish {
                        id: self.agent.clone(),
                        ok,
                        tokens: None,
                    },
                );
            }
            _ => {}
        }
    }

    /// A completed turn item (paginated history): the user's message, or the
    /// effect of a tool call.
    fn completed(&mut self, item: &Value, out: &mut Vec<Event>) {
        let ty = item["type"].as_str().unwrap_or("");
        if ty == "UserMessage" {
            let text: Vec<&str> = item["content"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect();
            let text = text.join(" ");
            if !text.trim().is_empty() {
                self.push(
                    out,
                    Kind::Prompt {
                        agent: self.agent.clone(),
                        text: clean(text.trim(), 400),
                    },
                );
            }
            return;
        }
        let Some((tool, detail, failed)) = effect(ty, item) else {
            return;
        };
        // An item for a call already shown (a direct MCP call) only adds
        // whether it failed.
        let id = item["id"].as_str().unwrap_or("");
        if id.is_empty() || !self.shown.contains(id) {
            if let Some(call) = self.open_exec.as_ref().and_then(|c| self.execs.get_mut(c)) {
                call.effects = true;
            }
            self.tool(
                out,
                self.last_ts,
                tool.clone(),
                &detail,
                item_key(&tool, item),
            );
        }
        if failed {
            self.error(out);
        }
    }

    /// A message between agents.
    fn message(&mut self, p: &Value, ctx: &mut Ctx, out: &mut Vec<Event>) {
        let resolve = |path: &str| ctx.paths.get(path).cloned();
        let from = p["author"]
            .as_str()
            .and_then(resolve)
            .unwrap_or_else(|| self.agent.clone());
        let Some(to) = p["recipient"].as_str().and_then(resolve) else {
            return;
        };
        let content = match &p["content"] {
            Value::String(s) => s.clone(),
            v => output_text(v),
        };
        if !ctx.messages.insert(str_hash64(&[&from, &to, &content])) {
            return;
        }
        self.push(
            out,
            Kind::Message {
                from,
                to,
                summary: clean(&content, 120),
            },
        );
    }
}

/// Tool name, display detail and failure of a completed item, or `None` for
/// items that are not tool activity (messages, reasoning, compaction).
fn effect(ty: &str, it: &Value) -> Option<(String, String, bool)> {
    let s = |k: &str| it[k].as_str().unwrap_or("").to_string();
    let status = it["status"].as_str().unwrap_or("");
    let mut failed = matches!(status, "failed" | "declined");
    let (tool, detail) = match ty {
        "CommandExecution" => {
            failed |= it["exit_code"].as_i64().is_some_and(|c| c != 0);
            let cmd = strings(&it["command"]);
            let parsed = it["parsed_cmd"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let kind = parsed
                .first()
                .and_then(|c| c["type"].as_str())
                .unwrap_or("");
            let same = parsed.iter().all(|c| c["type"].as_str() == Some(kind));
            let c = parsed.first().unwrap_or(&Value::Null);
            match kind {
                "read" if same => (
                    "read".to_string(),
                    base(c["path"].as_str().or(c["name"].as_str()).unwrap_or("")),
                ),
                "search" if same => (
                    "search".to_string(),
                    c["query"].as_str().unwrap_or("").to_string(),
                ),
                "list_files" if same => (
                    "list_files".to_string(),
                    c["path"].as_str().map(base).unwrap_or_default(),
                ),
                _ => ("shell".to_string(), shell_text(&cmd)),
            }
        }
        "FileChange" => {
            let files: Vec<&str> = it["changes"]
                .as_object()
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            ("apply_patch".to_string(), file_list(&files))
        }
        "McpToolCall" => {
            failed |= it["result"]["isError"].as_bool() == Some(true) || !it["error"].is_null();
            (
                format!("{}.{}", s("server"), s("tool")),
                first_string(&it["arguments"]),
            )
        }
        "DynamicToolCall" => {
            failed |= it["success"].as_bool() == Some(false);
            let name = match it["namespace"].as_str() {
                Some(ns) if !ns.is_empty() => format!("{ns}.{}", s("tool")),
                _ => s("tool"),
            };
            (name, first_string(&it["arguments"]))
        }
        "CollabAgentToolCall" => (s("tool"), s("prompt")),
        "WebSearch" => ("web_search".to_string(), s("query")),
        "ImageView" => ("view_image".to_string(), base(&s("path"))),
        "ImageGeneration" => ("image_gen".to_string(), String::new()),
        "Extension" => {
            failed |= !it["failure"].is_null();
            match it["kind"].as_str().unwrap_or("") {
                "web.search" => ("web_search".to_string(), s("query")),
                "image_gen.generation" => ("image_gen".to_string(), String::new()),
                "" => return None,
                k => (k.to_string(), String::new()),
            }
        }
        _ => return None,
    };
    Some((tool, detail, failed))
}

/// Repeat key of a completed item: the tool and the item without its id,
/// timing, status and results, so two runs of the same input match.
/// `Value`'s text form is canonical (serde_json keeps object keys sorted).
fn item_key(tool: &str, item: &Value) -> u64 {
    const VOLATILE: &[&str] = &[
        "id",
        "aggregated_output",
        "duration",
        "exit_code",
        "status",
        "process_id",
        "result",
        "error",
        "results",
        "stdout",
        "stderr",
        "success",
        "content_items",
        "agents_states",
        "receiver_thread_ids",
        "receiver_agents",
        "failure",
        "savedPath",
        "saved_path",
        "revisedPrompt",
        "revised_prompt",
        "generationId",
    ];
    let mut v = item.clone();
    if let Some(m) = v.as_object_mut() {
        for k in VOLATILE {
            m.remove(*k);
        }
    }
    str_hash64(&[tool, &v.to_string()])
}

/// The strings of a JSON array (a command's argv).
fn strings(v: &Value) -> Vec<&str> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The command line a shell call runs: the script of `sh -lc <script>`.
fn shell_text(cmd: &[&str]) -> String {
    match cmd {
        [_, flag, script, ..] if matches!(*flag, "-lc" | "-c") => script.to_string(),
        _ => cmd.join(" "),
    }
}

/// A shell call, or a patch applied through the shell (legacy files).
fn shell_tool(cmd: &[&str]) -> (String, String) {
    if let ["apply_patch", patch, ..] = cmd {
        return ("apply_patch".into(), patch_files(patch));
    }
    let text = shell_text(cmd);
    if text.trim_start().starts_with("apply_patch") {
        return ("apply_patch".into(), patch_files(&text));
    }
    ("shell".into(), text)
}

/// Files named in an `apply_patch` body.
fn patch_files(patch: &str) -> String {
    let files: Vec<&str> = patch
        .lines()
        .filter_map(|l| {
            ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                .iter()
                .find_map(|p| l.strip_prefix(p))
        })
        .collect();
    file_list(&files)
}

/// `lib.rs`, or `lib.rs +2` for three files.
fn file_list(files: &[&str]) -> String {
    match files {
        [] => String::new(),
        [one] => base(one),
        [first, rest @ ..] => format!("{} +{}", base(first), rest.len()),
    }
}

fn base(path: &str) -> String {
    path.trim().rsplit('/').next().unwrap_or(path).to_string()
}

/// First string value of a JSON object (a call's main argument, usually).
fn first_string(v: &Value) -> String {
    v.as_object()
        .and_then(|m| m.values().find_map(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default()
}

/// Text of a tool output: a string, or the text parts of a content list.
fn output_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- discovery

/// Session metadata from the first line of a rollout file.
#[derive(Clone, Default)]
struct Meta {
    thread_id: String,
    /// Thread that spawned this one, for a subagent.
    parent: Option<String>,
    /// Internal or subagent thread: not listed as a session of its own.
    hidden: bool,
    ts: Option<i64>,
    role: String,
    nickname: String,
    agent_path: String,
    project: String,
}

fn parse_meta(line: &str) -> Option<Meta> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] == "session_meta" {
        let p = &v["payload"];
        let spawn = &p["source"]["subagent"]["thread_spawn"];
        let pick = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| p[*k].as_str().or(spawn[*k].as_str()))
                .unwrap_or("")
                .to_string()
        };
        let parent = Some(pick(&["parent_thread_id"])).filter(|s| !s.is_empty());
        let source = &p["source"];
        Some(Meta {
            thread_id: p["id"].as_str()?.to_string(),
            hidden: parent.is_some()
                || !source["subagent"].is_null()
                || !source["internal"].is_null()
                || p["thread_source"].as_str().is_some_and(|s| s != "user"),
            parent,
            ts: p["timestamp"]
                .as_str()
                .or(v["timestamp"].as_str())
                .and_then(parse_ts),
            role: pick(&["agent_role", "agent_type"]),
            nickname: pick(&["agent_nickname"]),
            agent_path: pick(&["agent_path"]),
            project: p["cwd"].as_str().map(base).unwrap_or_default(),
        })
    } else if v.get("type").is_none() && v.get("instructions").is_some() {
        // Legacy header: no cwd, but often the git remote.
        let repo = v["git"]["repository_url"].as_str().unwrap_or("");
        Some(Meta {
            thread_id: v["id"].as_str()?.to_string(),
            ts: v["timestamp"].as_str().and_then(parse_ts),
            project: base(repo).trim_end_matches(".git").to_string(),
            ..Meta::default()
        })
    } else {
        None
    }
}

/// True when `path` is a Codex rollout file: by its name, or by its first line.
pub fn is_rollout(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    if name.ends_with(".jsonl.zst") || (name.starts_with("rollout-") && name.ends_with(".jsonl")) {
        return true;
    }
    first_line(path).and_then(|l| parse_meta(&l)).is_some()
}

/// Rollout files under `sessions/YYYY/MM/DD`, in day directories on or after
/// `from` (`YYYY/MM/DD`; empty for all).
fn rollout_files(sessions: &Path, from: &str) -> Vec<PathBuf> {
    let dirs = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        v.sort();
        v
    };
    let mut out = Vec::new();
    for year in dirs(sessions) {
        for month in dirs(&year) {
            for day in dirs(&month) {
                let date = day
                    .strip_prefix(sessions)
                    .map(|d| d.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if date.as_str() < from {
                    continue;
                }
                for entry in fs::read_dir(&day).into_iter().flatten().flatten() {
                    let path = entry.path();
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("rollout-")
                        && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
                    {
                        out.push(path);
                    }
                }
            }
        }
    }
    out
}

fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(h) if !h.is_empty() => Some(PathBuf::from(h)),
        _ => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")),
    }
}

/// All Codex sessions on this machine, most recent first.
pub fn sessions() -> Vec<SessionInfo> {
    codex_home().map(|h| sessions_in(&h)).unwrap_or_default()
}

fn sessions_in(home: &Path) -> Vec<SessionInfo> {
    let mut files: Vec<(PathBuf, i64)> = rollout_files(&home.join("sessions"), "")
        .into_iter()
        .map(|p| {
            let mtime = fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            (p, mtime)
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files.truncate(1000);
    let metas: Vec<Option<Meta>> = files
        .iter()
        .map(|(p, _)| first_line(p).and_then(|l| parse_meta(&l)))
        .collect();
    // Subagents count toward the top-level thread they descend from.
    let parent: HashMap<&str, &str> = metas
        .iter()
        .flatten()
        .filter_map(|m| Some((m.thread_id.as_str(), m.parent.as_deref()?)))
        .collect();
    let mut agents: HashMap<&str, usize> = HashMap::new();
    for m in metas.iter().flatten().filter(|m| m.parent.is_some()) {
        let mut top = m.thread_id.as_str();
        for _ in 0..32 {
            match parent.get(top) {
                Some(p) => top = p,
                None => break,
            }
        }
        *agents.entry(top).or_default() += 1;
    }
    let names = thread_names(home);
    files
        .iter()
        .zip(&metas)
        .filter(|(_, m)| !m.as_ref().is_some_and(|m| m.hidden))
        .take(200)
        .map(|((path, mtime_ms), m)| {
            let id = m.as_ref().map(|m| m.thread_id.as_str()).unwrap_or("");
            SessionInfo {
                path: path.clone(),
                project: m
                    .as_ref()
                    .map(|m| m.project.clone())
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| TOOL.to_string()),
                title: names.get(id).cloned().unwrap_or_default(),
                mtime_ms: *mtime_ms,
                agents: agents.get(id).copied().unwrap_or(0),
                tool: TOOL,
            }
        })
        .collect()
}

/// Thread titles from `session_index.jsonl` (`{id, thread_name, updated_at}`
/// lines, appended on rename; the last one wins).
fn thread_names(home: &Path) -> HashMap<String, String> {
    let text = fs::read_to_string(home.join("session_index.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| {
            Some((
                v["id"].as_str()?.to_string(),
                v["thread_name"].as_str()?.to_string(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A scratch `CODEX_HOME`, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("neonhive-codex-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("sessions/2026/01/01")).unwrap();
            Self(dir)
        }
        fn path(&self, file: &str) -> PathBuf {
            self.0.join("sessions/2026/01/01").join(file)
        }
        fn append(&self, file: &str, text: &str) {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path(file))
                .unwrap();
            f.write_all(text.as_bytes()).unwrap();
        }
        fn lines(&self, file: &str, lines: &[String]) {
            for l in lines {
                self.append(file, &format!("{l}\n"));
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ROOT_FILE: &str = "rollout-2026-01-01T00-00-00-t-root.jsonl";

    fn env(sec: u32, kind: &str, payload: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-01-01T00:00:{sec:02}.000Z","type":"{kind}","payload":{payload}}}"#
        )
    }

    fn meta(id: &str, extra: &str) -> String {
        env(
            0,
            "session_meta",
            &format!(
                r#"{{"id":"{id}","session_id":"s","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/work/proj","source":"cli","thread_source":"user"{extra}}}"#
            ),
        )
    }

    fn item(sec: u32, item: &str) -> String {
        env(
            sec,
            "event_msg",
            &format!(r#"{{"type":"item_completed","thread_id":"t","turn_id":"u","item":{item}}}"#),
        )
    }

    fn tokens(sec: u32, input: u64, cached: u64, output: u64) -> String {
        let u = format!(
            r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":0,"total_tokens":0}}"#
        );
        env(
            sec,
            "event_msg",
            &format!(
                r#"{{"type":"token_count","info":{{"total_token_usage":{u},"last_token_usage":{u}}}}}"#
            ),
        )
    }

    fn assistant(sec: u32, text: &str) -> String {
        env(
            sec,
            "response_item",
            &format!(
                r#"{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{text}"}}]}}"#
            ),
        )
    }

    fn kinds(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match &e.kind {
                Kind::Spawn {
                    id,
                    parent,
                    agent_type,
                    label,
                } => format!("spawn {id}<{parent} {agent_type} {label}"),
                Kind::Finish { id, ok, .. } => format!("finish {id} {ok}"),
                Kind::Message { from, to, summary } => format!("msg {from}>{to} {summary}"),
                Kind::Tool {
                    agent,
                    tool,
                    detail,
                    ..
                } => format!("tool {agent} {tool} {detail}"),
                Kind::ToolError { agent } => format!("error {agent}"),
                Kind::Usage { agent, tokens, .. } => format!("usage {agent} {}", tokens.fresh()),
                Kind::Text { agent, text } => format!("text {agent} {text}"),
                Kind::Prompt { agent, text } => format!("prompt {agent} {text}"),
            })
            .collect()
    }

    #[test]
    fn reads_paginated_envelope_session() {
        let s = Scratch::new("paginated");
        s.lines(
            ROOT_FILE,
            &[
                meta("t-root", ""),
                env(1, "turn_context", r#"{"model":"gpt-x","cwd":"/work/proj"}"#),
                // Injected context is a user item too: never a prompt.
                env(1, "response_item", r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}"#),
                item(1, r#"{"type":"UserMessage","id":"m1","content":[{"type":"text","text":"Fix the bug","text_elements":[]}]}"#),
                assistant(2, "Looking"),
                // A code-mode script that ran commands and a patch: those are
                // shown, the script itself is not.
                env(3, "response_item", r#"{"type":"custom_tool_call","name":"exec","call_id":"c1","input":"const r = await run('cargo test');"}"#),
                item(4, r#"{"type":"CommandExecution","id":"exec-1","command":["/bin/zsh","-lc","cargo test"],"cwd":"/work/proj","parsed_cmd":[{"type":"unknown","cmd":"cargo test"}],"status":"failed","exit_code":101}"#),
                item(4, r#"{"type":"CommandExecution","id":"exec-2","command":["/bin/zsh","-lc","sed -n 1,5p src/lib.rs"],"cwd":"/work/proj","parsed_cmd":[{"type":"read","cmd":"sed","name":"lib.rs","path":"src/lib.rs"}],"status":"completed","exit_code":0}"#),
                item(4, r#"{"type":"FileChange","id":"exec-3","changes":{"/work/proj/src/a.rs":{"type":"update"},"/work/proj/src/b.rs":{"type":"add"}},"status":"completed"}"#),
                env(5, "response_item", r#"{"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_text","text":"Script completed"}]}"#),
                // A script with no tool activity is shown itself, at its call time.
                env(6, "response_item", r#"{"type":"custom_tool_call","name":"exec","call_id":"c2","input":"\n  text('hi');"}"#),
                env(7, "response_item", r#"{"type":"custom_tool_call_output","call_id":"c2","output":"Script failed"}"#),
                // A direct MCP call and its item: one tool call, plus the failure.
                env(8, "response_item", r#"{"type":"function_call","name":"js","namespace":"mcp__repl","call_id":"call_9","arguments":"{\"code\":\"1+1\"}"}"#),
                item(9, r#"{"type":"McpToolCall","id":"call_9","server":"repl","tool":"js","arguments":{"code":"1+1"},"status":"completed","result":{"content":[],"isError":true}}"#),
                item(9, r#"{"type":"Extension","id":"exec-4","kind":"web.search","query":"rust zstd","status":"completed","failure":null}"#),
                item(9, r#"{"type":"Reasoning","id":"rs_1","summary_text":[],"raw_content":[]}"#),
                item(9, r#"{"type":"AgentMessage","id":"m2","content":[{"type":"text","text":"Looking"}],"phase":"commentary"}"#),
                tokens(10, 1000, 800, 50),
                // The same total again (a rate-limit update): nothing new.
                tokens(10, 1000, 800, 50),
                tokens(11, 1500, 1200, 80),
                // The top-level agent waits for the human; it never finishes.
                env(12, "event_msg", r#"{"type":"task_complete","turn_id":"u"}"#),
            ],
        );
        let mut src = CodexSource::open(&s.path(ROOT_FILE));
        let events = src.poll();
        assert_eq!(
            kinds(&events),
            vec![
                "prompt main Fix the bug",
                "text main Looking",
                "tool main shell cargo test",
                "error main",
                "tool main read lib.rs",
                "tool main apply_patch a.rs +1",
                "tool main exec text('hi');",
                "error main",
                "tool main mcp__repl.js 1+1",
                "error main",
                "tool main web_search rust zstd",
                "usage main 250",
                "usage main 130",
            ]
        );
        let first = events.iter().find_map(|e| match &e.kind {
            Kind::Usage { tokens, model, .. } => Some((*tokens, model.as_str())),
            _ => None,
        });
        let want = Tokens {
            input: 200,
            output: 50,
            cache_read: 800,
            ..Tokens::default()
        };
        assert_eq!(first, Some((want, "gpt-x")));
        assert!(src.poll().is_empty(), "second poll must not repeat history");
    }

    #[test]
    fn reads_legacy_envelope_session() {
        // History mode "legacy": what the user typed is an event, tool
        // activity is only the model's calls.
        let s = Scratch::new("legacy-envelope");
        s.lines(
            ROOT_FILE,
            &[
                meta("t-root", ""),
                env(1, "event_msg", r#"{"type":"user_message","message":"Add a test"}"#),
                env(1, "response_item", r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"Add a test"}]}"#),
                env(2, "response_item", r#"{"type":"function_call","name":"shell","call_id":"a","arguments":"{\"command\":[\"bash\",\"-lc\",\"ls -la\"]}"}"#),
                env(3, "response_item", r#"{"type":"local_shell_call","call_id":"b","status":"completed","action":{"type":"exec","command":["apply_patch","*** Begin Patch\n*** Update File: src/x.rs\n*** End Patch"]}}"#),
                env(4, "response_item", r#"{"type":"web_search_call","status":"completed","action":{"type":"search","query":"ratatui"}}"#),
                env(5, "response_item", r#"{"type":"custom_tool_call","name":"apply_patch","call_id":"c","input":"*** Begin Patch\n*** Add File: a/new.rs\n*** End Patch"}"#),
                env(6, "event_msg", r#"{"type":"agent_message","message":"Done"}"#),
            ],
        );
        let got = kinds(&CodexSource::open(&s.path(ROOT_FILE)).poll());
        assert_eq!(
            got,
            vec![
                "prompt main Add a test",
                "tool main shell ls -la",
                "tool main apply_patch x.rs",
                "tool main web_search ratatui",
                "tool main apply_patch new.rs",
            ]
        );
    }

    #[test]
    fn reads_legacy_bare_session() {
        let s = Scratch::new("legacy-bare");
        let file = "rollout-2025-08-30T00-00-00-t-old.jsonl";
        let lines: Vec<String> = [
            r#"{"id":"t-old","timestamp":"2025-08-30T00:00:05.000Z","instructions":null,"git":{"repository_url":"git@example.com:me/proj.git"}}"#,
            r#"{"record_type":"state"}"#,
            r#"{"type":"message","id":null,"role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}"#,
            r#"{"type":"message","id":null,"role":"user","content":[{"type":"input_text","text":"Speed it up"}]}"#,
            r#"{"type":"reasoning","id":"r","summary":[],"content":null,"encrypted_content":"x"}"#,
            r#"{"type":"function_call","name":"shell","call_id":"a","arguments":"{\"command\":[\"bash\",\"-lc\",\"make\"]}"}"#,
            r#"{"type":"function_call_output","call_id":"a","output":"{\"output\":\"\",\"metadata\":{\"exit_code\":2,\"duration_seconds\":0.1}}"}"#,
            r#"{"type":"function_call","name":"shell","call_id":"b","arguments":"{\"command\":[\"apply_patch\",\"*** Begin Patch\\n*** Update File: src/m.rs\\n*** End Patch\"]}"}"#,
            r#"{"type":"function_call_output","call_id":"b","output":"{\"output\":\"Done\",\"metadata\":{\"exit_code\":0}}"}"#,
            r#"{"type":"function_call","name":"update_plan","call_id":"c","arguments":"{\"plan\":[]}"}"#,
            r#"{"type":"function_call_output","call_id":"c","output":"Plan updated"}"#,
            r#"{"type":"message","id":"m","role":"assistant","content":[{"type":"output_text","text":"Done"}]}"#,
        ]
        .map(String::from)
        .to_vec();
        s.lines(file, &lines);
        let events = CodexSource::open(&s.path(file)).poll();
        assert_eq!(
            kinds(&events),
            vec![
                "prompt main Speed it up",
                "tool main shell make",
                "error main",
                "tool main apply_patch m.rs",
                "tool main update_plan ",
                "text main Done",
            ]
        );
        // Lines have no time of their own: everything sits at the header time.
        let t0 = parse_ts("2025-08-30T00:00:05.000Z").unwrap();
        assert!(events.iter().all(|e| e.ts == t0));
    }

    #[test]
    fn skips_malformed_lines_and_tails_partial_lines() {
        let s = Scratch::new("tail");
        s.lines(
            ROOT_FILE,
            &[
                meta("t-root", ""),
                "not json".into(),
                "[1,2,3]".into(),
                r#"{"type":"event_msg","payload":"not an object"}"#.into(),
                env(1, "event_msg", r#"{"type":"token_count","info":null}"#),
                env(1, "event_msg", r#"{"type":"token_count","info":{"total_token_usage":"x"}}"#),
                env(1, "event_msg", r#"{"type":"item_completed","item":{"type":"CommandExecution"}}"#),
                env(1, "response_item", r#"{"type":"function_call","name":"shell","arguments":"not json"}"#),
                r#"{"timestamp":"garbage","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"still here"}]}}"#.into(),
                env(2, "something_new", r#"{"type":"x"}"#),
            ],
        );
        let mut src = CodexSource::open(&s.path(ROOT_FILE));
        assert_eq!(
            kinds(&src.poll()),
            vec![
                "tool main shell ",
                "tool main shell ",
                "text main still here"
            ]
        );
        let line = assistant(3, "hello") + "\n";
        let (head, tail) = line.split_at(30);
        s.append(ROOT_FILE, head);
        assert!(src.poll().is_empty(), "half a line is not an event yet");
        s.append(ROOT_FILE, tail);
        assert_eq!(kinds(&src.poll()), vec!["text main hello"]);
        assert!(src.poll().is_empty());
    }

    #[test]
    fn token_totals_that_go_down_fall_back_to_the_last_response() {
        let s = Scratch::new("reset");
        s.lines(
            ROOT_FILE,
            &[
                meta("t-root", ""),
                tokens(1, 1000, 0, 100),
                tokens(2, 10, 0, 5),
                tokens(3, u64::MAX, u64::MAX, 5),
            ],
        );
        let got = kinds(&CodexSource::open(&s.path(ROOT_FILE)).poll());
        assert_eq!(
            got,
            vec!["usage main 1100", "usage main 15", "usage main 0"]
        );
    }

    #[test]
    fn reads_compressed_rollout_once() {
        let s = Scratch::new("zst");
        let text = format!("{}\n{}\n", meta("t-root", ""), assistant(1, "packed"));
        let packed = ruzstd::encoding::compress_to_vec(
            text.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let file = "rollout-2026-01-01T00-00-00-t-root.jsonl.zst";
        fs::write(s.path(file), packed).unwrap();
        assert!(is_rollout(&s.path(file)));
        assert_eq!(first_line(&s.path(file)), Some(meta("t-root", "")));
        let mut src = CodexSource::open(&s.path(file));
        assert_eq!(kinds(&src.poll()), vec!["text main packed"]);
        assert!(src.poll().is_empty());
        // Not zstd at all: nothing, and no panic.
        fs::write(s.path("rollout-bad.jsonl.zst"), b"garbage").unwrap();
        assert!(CodexSource::open(&s.path("rollout-bad.jsonl.zst"))
            .poll()
            .is_empty());
    }

    #[test]
    fn subagent_threads_become_spawns_with_their_parents() {
        let s = Scratch::new("subagents");
        let note = r#"{"author":"/root","recipient":"/root/worker","other_recipients":[],"content":"Map the parser","trigger_turn":true}"#;
        s.lines(
            ROOT_FILE,
            &[
                meta("t-root", ""),
                env(1, "inter_agent_communication", note),
            ],
        );
        // A child, named by the top-level parent field. The message is in its
        // file too, and counts once.
        s.lines(
            "rollout-2026-01-01T00-00-02-t-kid.jsonl",
            &[
                meta(
                    "t-kid",
                    r#","parent_thread_id":"t-root","agent_nickname":"Ada","agent_role":"explorer","agent_path":"/root/worker""#,
                )
                .replace(r#""thread_source":"user""#, r#""thread_source":"subagent""#),
                env(3, "inter_agent_communication", note),
                env(3, "response_item", r#"{"type":"function_call","name":"shell","call_id":"a","arguments":"{\"command\":[\"bash\",\"-lc\",\"ls\"]}"}"#),
                env(4, "event_msg", r#"{"type":"task_complete","turn_id":"u"}"#),
            ],
        );
        // A grandchild, named only by its source, found before its parent
        // would be if files were read in another order.
        s.lines(
            "rollout-2026-01-01T00-00-01-t-grand.jsonl",
            &[
                meta("t-grand", "").replace(
                    r#""source":"cli""#,
                    r#""source":{"subagent":{"thread_spawn":{"parent_thread_id":"t-kid","depth":2,"agent_path":"/root/worker/sub","agent_nickname":null,"agent_role":null}}}"#,
                ),
                env(6, "event_msg", r#"{"type":"turn_aborted","reason":"interrupted"}"#),
            ],
        );
        // An unrelated session, and a file still empty.
        s.lines(
            "rollout-2026-01-01T00-00-09-t-other.jsonl",
            &[meta("t-other", "")],
        );
        s.append("rollout-2026-01-01T00-00-10-t-empty.jsonl", "");

        let mut src = CodexSource::open(&s.path(ROOT_FILE));
        assert_eq!(
            kinds(&src.poll()),
            vec![
                "spawn t-kid<main explorer Ada",
                "spawn t-grand<t-kid agent /root/worker/sub",
                "msg main>t-kid Map the parser",
                "tool t-kid shell ls",
                "finish t-kid true",
                "finish t-grand false",
            ]
        );
        // A subagent that starts later, even on another day, is found on a
        // later scan.
        fs::create_dir_all(s.0.join("sessions/2026/01/02")).unwrap();
        fs::write(
            s.0.join("sessions/2026/01/02/rollout-2026-01-02T00-00-00-t-late.jsonl"),
            meta("t-late", r#","parent_thread_id":"t-root""#) + "\n",
        )
        .unwrap();
        let mut later = Vec::new();
        for _ in 0..SCAN_EVERY {
            later.extend(src.poll());
        }
        assert_eq!(kinds(&later), vec!["spawn t-late<main agent "]);
    }

    #[test]
    fn lists_sessions_with_titles_and_subagent_counts() {
        let s = Scratch::new("discovery");
        s.lines(ROOT_FILE, &[meta("t-root", "")]);
        s.lines(
            "rollout-2026-01-01T00-00-02-t-kid.jsonl",
            &[meta("t-kid", r#","parent_thread_id":"t-root""#)],
        );
        s.lines(
            "rollout-2026-01-01T00-00-03-t-grand.jsonl",
            &[meta("t-grand", r#","parent_thread_id":"t-kid""#)],
        );
        s.lines(
            "rollout-2025-08-30T00-00-00-t-old.jsonl",
            &[r#"{"id":"t-old","timestamp":"2025-08-30T00:00:05.000Z","instructions":null,"git":{"repository_url":"https://example.com/me/oldproj.git"}}"#.to_string()],
        );
        fs::write(s.path("notes.txt"), "ignored").unwrap();
        fs::write(
            s.0.join("session_index.jsonl"),
            "{\"id\":\"t-root\",\"thread_name\":\"First name\",\"updated_at\":\"x\"}\nbad line\n{\"id\":\"t-root\",\"thread_name\":\"Renamed\",\"updated_at\":\"y\"}\n",
        )
        .unwrap();
        let mut got: Vec<(String, String, usize, &str)> = sessions_in(&s.0)
            .into_iter()
            .map(|i| (i.project, i.title, i.agents, i.tool))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("oldproj".to_string(), String::new(), 0, "codex"),
                ("proj".to_string(), "Renamed".to_string(), 2, "codex"),
            ]
        );
        assert!(sessions_in(&s.0.join("missing")).is_empty());
    }

    #[test]
    fn detects_rollouts_by_name_or_first_line() {
        let s = Scratch::new("detect");
        s.lines("copied.jsonl", &[meta("t", "")]);
        s.lines(
            "claude.jsonl",
            &[r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"role":"user","content":"hi"}}"#.to_string()],
        );
        s.lines(ROOT_FILE, &[]);
        assert!(is_rollout(&s.path("copied.jsonl")));
        assert!(is_rollout(&s.path(ROOT_FILE)));
        assert!(!is_rollout(&s.path("claude.jsonl")));
        assert!(!is_rollout(&s.path("missing.jsonl")));
    }

    #[test]
    fn call_keys_follow_the_full_input() {
        let s = Scratch::new("keys");
        let long =
            "cd /a/very/long/path/that/goes/on/and/on/and/fills/the/eighty/character/summary";
        let shell = |cmd: &str| {
            env(
                1,
                "response_item",
                &format!(
                    r#"{{"type":"function_call","name":"shell","call_id":"x","arguments":"{{\"command\":[\"bash\",\"-lc\",\"{cmd}\"]}}"}}"#
                ),
            )
        };
        let run = |id: &str, cmd: &str, code: i32| {
            item(
                2,
                &format!(
                    r#"{{"type":"CommandExecution","id":"{id}","command":["zsh","-lc","{cmd}"],"cwd":"/w","parsed_cmd":[],"status":"completed","exit_code":{code},"duration":{{"secs":{code},"nanos":0}}}}"#
                ),
            )
        };
        s.lines(
            ROOT_FILE,
            &[
                meta("t-root", ""),
                shell(&format!("{long} && make a")),
                shell(&format!("{long} && make b")),
                shell(&format!("{long} && make a")),
                // Same command with another item id, timing and outcome.
                run("exec-1", "make", 0),
                run("exec-2", "make", 1),
                run("exec-3", "make all", 0),
            ],
        );
        let tools: Vec<(String, u64)> = CodexSource::open(&s.path(ROOT_FILE))
            .poll()
            .into_iter()
            .filter_map(|e| match e.kind {
                Kind::Tool { detail, key, .. } => Some((detail, key)),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 6);
        assert_eq!(tools[0].0, tools[1].0, "the display detail is truncated");
        assert_ne!(tools[0].1, tools[1].1);
        assert_eq!(tools[0].1, tools[2].1);
        assert_eq!(tools[3].1, tools[4].1);
        assert_ne!(tools[3].1, tools[5].1);
    }

    #[test]
    fn missing_files_yield_nothing() {
        let s = Scratch::new("missing");
        assert!(CodexSource::open(&s.path("nope.jsonl")).poll().is_empty());
    }
}
