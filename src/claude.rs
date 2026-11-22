//! Claude Code source adapter.
//!
//! Reads a session transcript `<session>.jsonl` and its subagent transcripts
//! `<session>/subagents/agent-<id>.jsonl` (+ `.meta.json`), tailing them for
//! new lines. The transcript format is not a documented contract, so parsing
//! is tolerant: unknown lines and fields are skipped.

use crate::event::{Event, Kind, Source, Tokens, ROOT};
use crate::util::{clean, parse_ts};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct ClaudeSource {
    main: Tailer,
    sub_dir: PathBuf,
    subs: Vec<Tailer>,
    known: HashSet<String>,
    /// tool_use id of an `Agent` call -> id of the agent that made the call.
    spawners: HashMap<String, String>,
}

impl ClaudeSource {
    pub fn open(path: &Path) -> Self {
        Self {
            main: Tailer::new(path.to_path_buf(), ROOT.to_string()),
            sub_dir: path.with_extension("").join("subagents"),
            subs: Vec::new(),
            known: HashSet::new(),
            spawners: HashMap::new(),
        }
    }

    fn discover(&mut self) {
        let Ok(rd) = fs::read_dir(&self.sub_dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(id) = stem.strip_prefix("agent-") else {
                continue;
            };
            if self.known.insert(id.to_string()) {
                self.subs.push(Tailer::new(path.clone(), id.to_string()));
            }
        }
    }
}

impl Source for ClaudeSource {
    fn poll(&mut self) -> Vec<Event> {
        self.discover();
        let mut events = Vec::new();
        self.main.read(&mut self.spawners, &mut events);
        for sub in &mut self.subs {
            sub.read(&mut self.spawners, &mut events);
        }
        // Announce subagents after all files are read, so the spawning
        // tool_use is known whichever file was read first.
        let mut out = Vec::new();
        for sub in &mut self.subs {
            let (false, Some(ts)) = (sub.announced, sub.first_ts) else {
                continue;
            };
            sub.announced = true;
            let meta = load_meta(&sub.path);
            let parent = meta
                .as_ref()
                .and_then(|m| self.spawners.get(&m.tool_use_id))
                .cloned()
                .unwrap_or_else(|| ROOT.to_string());
            out.push(Event {
                ts,
                kind: Kind::Spawn {
                    id: sub.agent.clone(),
                    parent,
                    agent_type: meta
                        .as_ref()
                        .map(|m| m.agent_type.clone())
                        .unwrap_or_else(|| "agent".into()),
                    label: meta.map(|m| m.description).unwrap_or_default(),
                },
            });
        }
        out.append(&mut events);
        out.sort_by_key(|e| e.ts); // stable: a spawn stays ahead of same-time events
        out
    }
}

struct Meta {
    agent_type: String,
    description: String,
    tool_use_id: String,
}

fn load_meta(jsonl: &Path) -> Option<Meta> {
    let text = fs::read_to_string(jsonl.with_extension("meta.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(Meta {
        agent_type: v["agentType"].as_str().unwrap_or("agent").to_string(),
        description: v["description"].as_str().unwrap_or("").to_string(),
        tool_use_id: v["toolUseId"].as_str().unwrap_or("").to_string(),
    })
}

struct Tailer {
    path: PathBuf,
    agent: String,
    offset: u64,
    carry: Vec<u8>,
    first_ts: Option<i64>,
    last_ts: i64,
    announced: bool,
    msg_id: String,
    msg_counted: Tokens,
}

impl Tailer {
    fn new(path: PathBuf, agent: String) -> Self {
        Self {
            path,
            agent,
            offset: 0,
            carry: Vec::new(),
            first_ts: None,
            last_ts: 0,
            announced: false,
            msg_id: String::new(),
            msg_counted: Tokens::default(),
        }
    }

    fn read(&mut self, spawners: &mut HashMap<String, String>, out: &mut Vec<Event>) {
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
                self.parse(line, spawners, out);
            }
            start += pos + 1;
        }
        buf.drain(..start);
        self.carry = buf; // incomplete last line, finished on a later poll
    }

    fn parse(&mut self, line: &str, spawners: &mut HashMap<String, String>, out: &mut Vec<Event>) {
        // Cheap prefilter: most lines are bookkeeping with nothing to show.
        if !line.contains("\"message\"") && !line.contains("task-notification") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let ts = v["timestamp"]
            .as_str()
            .and_then(parse_ts)
            .unwrap_or(self.last_ts);
        self.last_ts = ts;
        self.first_ts.get_or_insert(ts);
        let agent = self.agent.clone();
        let mut push = |kind: Kind| out.push(Event { ts, kind });

        match v["type"].as_str().unwrap_or("") {
            "assistant" => {
                let msg = &v["message"];
                let id = msg["id"].as_str().unwrap_or("");
                let n = |key: &str| msg["usage"][key].as_u64().unwrap_or(0);
                let total = Tokens {
                    input: n("input_tokens"),
                    output: n("output_tokens"),
                    cache_read: n("cache_read_input_tokens"),
                    cache_write: n("cache_creation_input_tokens"),
                };
                // One API message spans several lines, each repeating its
                // usage (the output count can grow); emit only what is new.
                if id != self.msg_id {
                    self.msg_id = id.to_string();
                    self.msg_counted = Tokens::default();
                }
                let new = total.minus(&self.msg_counted);
                if !new.is_zero() {
                    push(Kind::Usage {
                        agent: agent.clone(),
                        tokens: new,
                        model: msg["model"].as_str().unwrap_or("").to_string(),
                    });
                    self.msg_counted.add(&new);
                }
                for block in msg["content"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    match block["type"].as_str().unwrap_or("") {
                        "text" => {
                            let text = clean(block["text"].as_str().unwrap_or(""), 400);
                            if !text.is_empty() {
                                push(Kind::Text {
                                    agent: agent.clone(),
                                    text,
                                });
                            }
                        }
                        "tool_use" => {
                            let name = block["name"].as_str().unwrap_or("?");
                            let input = &block["input"];
                            match name {
                                "Agent" | "Task" => {
                                    if let Some(id) = block["id"].as_str() {
                                        spawners.insert(id.to_string(), agent.clone());
                                    }
                                }
                                "SendMessage" => {
                                    let to = input["to"]
                                        .as_str()
                                        .or(input["recipient"].as_str())
                                        .unwrap_or("");
                                    let summary = input["summary"]
                                        .as_str()
                                        .or(input["message"].as_str())
                                        .or(input["content"].as_str())
                                        .unwrap_or("");
                                    push(Kind::Message {
                                        from: agent.clone(),
                                        to: to.to_string(),
                                        summary: clean(summary, 120),
                                    });
                                }
                                _ => push(Kind::Tool {
                                    agent: agent.clone(),
                                    tool: name.to_string(),
                                    detail: tool_detail(name, input),
                                }),
                            }
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                let content = &v["message"]["content"];
                let prompt = |text: &str| {
                    let t = text.trim_start();
                    // Skip injected reminders and command wrappers.
                    (!t.is_empty() && !t.starts_with('<')).then(|| Kind::Prompt {
                        agent: agent.clone(),
                        text: clean(t, 400),
                    })
                };
                if let Some(kind) = content.as_str().and_then(prompt) {
                    push(kind);
                }
                for block in content.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    match block["type"].as_str().unwrap_or("") {
                        "text" => {
                            if let Some(kind) = prompt(block["text"].as_str().unwrap_or("")) {
                                push(kind);
                            }
                        }
                        "tool_result" if block["is_error"].as_bool() == Some(true) => {
                            push(Kind::ToolError {
                                agent: agent.clone(),
                            });
                        }
                        _ => {}
                    }
                }
                // Foreground subagent: the tool result carries the final state.
                let r = &v["toolUseResult"];
                if let (Some(id), Some("completed")) = (r["agentId"].as_str(), r["status"].as_str())
                {
                    push(Kind::Finish {
                        id: id.to_string(),
                        ok: true,
                        tokens: r["totalTokens"].as_u64(),
                    });
                }
            }
            "attachment" => {
                // Background subagent: completion arrives as a task notification.
                let a = &v["attachment"];
                let text = match a["prompt"].as_str() {
                    Some(s) => s.to_string(),
                    None => a["prompt"].to_string(),
                };
                if let Some(id) = tag(&text, "task-id") {
                    let status = tag(&text, "status").unwrap_or("");
                    if status != "running" && !status.is_empty() {
                        push(Kind::Finish {
                            id: id.to_string(),
                            ok: status == "completed",
                            tokens: a["usage"]["totalTokens"].as_u64(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>"))?;
    Some(text[start..start + end].trim())
}

fn tool_detail(name: &str, input: &Value) -> String {
    let base = |v: &Value| {
        v.as_str()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
    };
    let s = match name {
        "Bash" => input["description"]
            .as_str()
            .or(input["command"].as_str())
            .map(str::to_string),
        "Read" | "Write" | "Edit" | "NotebookEdit" => base(&input["file_path"]),
        "Grep" | "Glob" => input["pattern"].as_str().map(str::to_string),
        "WebSearch" => input["query"].as_str().map(str::to_string),
        "WebFetch" => input["url"].as_str().map(str::to_string),
        _ => None,
    };
    let s = s.or_else(|| {
        input
            .as_object()?
            .values()
            .find_map(|v| v.as_str().map(str::to_string))
    });
    clean(&s.unwrap_or_default(), 80)
}

// ---------------------------------------------------------------- discovery

pub struct SessionInfo {
    pub path: PathBuf,
    pub project: String,
    pub title: String,
    pub mtime_ms: i64,
    pub agents: usize,
}

/// All Claude Code sessions on this machine, most recent first.
pub fn sessions() -> Vec<SessionInfo> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let root = PathBuf::from(home).join(".claude").join("projects");
    let mut found = Vec::new();
    for dir in fs::read_dir(&root).into_iter().flatten().flatten() {
        for entry in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            found.push((
                path,
                mtime_ms,
                meta.len(),
                dir.file_name().to_string_lossy().to_string(),
            ));
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.1));
    found.truncate(200);
    found
        .into_iter()
        .map(|(path, mtime_ms, len, dir_name)| {
            let tail = read_tail(&path, len, 256 * 1024);
            let project = last_json_string(&tail, "cwd")
                .map(|cwd| cwd.rsplit('/').next().unwrap_or(&cwd).to_string())
                .unwrap_or(dir_name);
            let agents = fs::read_dir(path.with_extension("").join("subagents"))
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                        .count()
                })
                .unwrap_or(0);
            SessionInfo {
                title: last_json_string(&tail, "aiTitle").unwrap_or_default(),
                path,
                project,
                mtime_ms,
                agents,
            }
        })
        .collect()
}

fn read_tail(path: &Path, len: u64, max: u64) -> String {
    let mut buf = Vec::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
        let _ = f.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Last value of `"key":"..."` in raw JSON text.
fn last_json_string(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = text.rfind(&pat)? + pat.len();
    let mut end = start;
    let bytes = text.as_bytes();
    while end < bytes.len() && bytes[end] != b'"' {
        end += if bytes[end] == b'\\' { 2 } else { 1 };
    }
    let raw = text.get(start..end.min(bytes.len()))?;
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A scratch session directory, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("neonhive-test-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("s1/subagents")).unwrap();
            Self(dir)
        }
        fn main(&self) -> PathBuf {
            self.0.join("s1.jsonl")
        }
        fn append(&self, rel: &str, text: &str) {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.0.join(rel))
                .unwrap();
            f.write_all(text.as_bytes()).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const T: &str = "2026-01-01T00:00:";

    fn assistant(sec: u32, msg_id: &str, block: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{T}{sec:02}.000Z","message":{{"id":"{msg_id}","model":"m1","usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}},"content":[{block}]}}}}"#
        ) + "\n"
    }

    fn spawn_call(tool_use_id: &str) -> String {
        format!(
            r#"{{"type":"tool_use","id":"{tool_use_id}","name":"Agent","input":{{"description":"d","prompt":"p"}}}}"#
        )
    }

    fn sub_files(s: &Scratch, id: &str, tool_use_id: &str, sec: u32) {
        s.append(
            &format!("s1/subagents/agent-{id}.meta.json"),
            &format!(r#"{{"agentType":"explorer","description":"Map the code","toolUseId":"{tool_use_id}"}}"#),
        );
        s.append(
            &format!("s1/subagents/agent-{id}.jsonl"),
            &format!(r#"{{"type":"user","timestamp":"{T}{sec:02}.000Z","message":{{"role":"user","content":"Map the code please"}}}}"#),
        );
        s.append(&format!("s1/subagents/agent-{id}.jsonl"), "\n");
    }

    fn kinds(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| match &e.kind {
                Kind::Spawn {
                    id,
                    parent,
                    agent_type,
                    ..
                } => format!("spawn {id}<{parent} {agent_type}"),
                Kind::Finish { id, ok, tokens } => format!("finish {id} {ok} {tokens:?}"),
                Kind::Message { from, to, summary } => format!("msg {from}>{to} {summary}"),
                Kind::Tool {
                    agent,
                    tool,
                    detail,
                } => format!("tool {agent} {tool} {detail}"),
                Kind::ToolError { agent } => format!("error {agent}"),
                Kind::Usage { agent, tokens, .. } => format!("usage {agent} {}", tokens.fresh()),
                Kind::Text { agent, text } => format!("text {agent} {text}"),
                Kind::Prompt { agent, text } => format!("prompt {agent} {text}"),
            })
            .collect()
    }

    #[test]
    fn reads_session_with_nested_subagents() {
        let s = Scratch::new("nested");
        s.append("s1.jsonl", r#"{"type":"mode","mode":"x"}"#);
        s.append("s1.jsonl", "\nnot json at all \"message\"\n");
        s.append(
            "s1.jsonl",
            &format!(r#"{{"type":"user","timestamp":"{T}01.000Z","message":{{"role":"user","content":"Fix the bug"}}}}"#),
        );
        s.append("s1.jsonl", "\n");
        s.append("s1.jsonl", &assistant(2, "m1", &spawn_call("tu1")));
        sub_files(&s, "aaa", "tu1", 3);
        s.append(
            "s1/subagents/agent-aaa.jsonl",
            &assistant(4, "m2", &spawn_call("tu2")),
        );
        sub_files(&s, "bbb", "tu2", 5);
        s.append(
            "s1/subagents/agent-bbb.jsonl",
            &assistant(
                6,
                "m3",
                r#"{"type":"tool_use","id":"x","name":"Read","input":{"file_path":"/a/b/lib.rs"}}"#,
            ),
        );

        let mut src = ClaudeSource::open(&s.main());
        let got = kinds(&src.poll());
        assert_eq!(
            got,
            vec![
                "prompt main Fix the bug",
                "usage main 35",
                "spawn aaa<main explorer",
                "prompt aaa Map the code please",
                "usage aaa 35",
                "spawn bbb<aaa explorer",
                "prompt bbb Map the code please",
                "usage bbb 35",
                "tool bbb Read lib.rs",
            ]
        );
        assert!(src.poll().is_empty(), "second poll must not repeat history");
    }

    #[test]
    fn tails_appended_lines_and_waits_for_complete_line() {
        let s = Scratch::new("tail");
        s.append(
            "s1.jsonl",
            &assistant(1, "m1", r#"{"type":"text","text":"hello"}"#),
        );
        let mut src = ClaudeSource::open(&s.main());
        assert_eq!(kinds(&src.poll()), vec!["usage main 35", "text main hello"]);

        let line = assistant(
            2,
            "m2",
            r#"{"type":"tool_use","id":"x","name":"Bash","input":{"command":"ls -la"}}"#,
        );
        let (head, tail) = line.split_at(40);
        s.append("s1.jsonl", head);
        assert!(src.poll().is_empty(), "half a line is not an event yet");
        s.append("s1.jsonl", tail);
        assert_eq!(
            kinds(&src.poll()),
            vec!["usage main 35", "tool main Bash ls -la"]
        );
    }

    #[test]
    fn usage_is_counted_once_per_api_message() {
        let s = Scratch::new("usage");
        // One API message written as two lines, one content block each.
        s.append(
            "s1.jsonl",
            &assistant(1, "m1", r#"{"type":"text","text":"a"}"#),
        );
        s.append(
            "s1.jsonl",
            &assistant(1, "m1", r#"{"type":"text","text":"b"}"#),
        );
        let got = kinds(&ClaudeSource::open(&s.main()).poll());
        assert_eq!(got.iter().filter(|k| k.starts_with("usage")).count(), 1);
    }

    #[test]
    fn usage_is_split_by_token_kind() {
        let s = Scratch::new("usage-split");
        s.append(
            "s1.jsonl",
            &assistant(1, "m1", r#"{"type":"text","text":"a"}"#),
        );
        let events = ClaudeSource::open(&s.main()).poll();
        let got = events.iter().find_map(|e| match &e.kind {
            Kind::Usage { tokens, model, .. } => Some((*tokens, model.as_str())),
            _ => None,
        });
        let want = Tokens {
            input: 10,
            output: 5,
            cache_read: 100,
            cache_write: 20,
        };
        assert_eq!(got, Some((want, "m1")));
    }

    #[test]
    fn task_notification_finishes_agent() {
        let s = Scratch::new("notify");
        for (sec, id, status) in [(1, "aaa", "completed"), (2, "bbb", "failed")] {
            let prompt = format!(
                "<task-notification>\\n<task-id>{id}</task-id>\\n<status>{status}</status>"
            );
            s.append(
                "s1.jsonl",
                &format!(
                    r#"{{"type":"attachment","timestamp":"{T}{sec:02}.000Z","attachment":{{"type":"queued_command","prompt":"{prompt}","usage":{{"totalTokens":4200}}}}}}"#
                ),
            );
            s.append("s1.jsonl", "\n");
        }
        let got = kinds(&ClaudeSource::open(&s.main()).poll());
        assert_eq!(
            got,
            vec!["finish aaa true Some(4200)", "finish bbb false Some(4200)"]
        );
    }

    #[test]
    fn messages_errors_and_injected_text() {
        let s = Scratch::new("misc");
        s.append(
            "s1.jsonl",
            &assistant(1, "m1", r#"{"type":"tool_use","id":"x","name":"SendMessage","input":{"to":"aaa","summary":"Narrow scope"}}"#),
        );
        s.append(
            "s1.jsonl",
            &format!(
                r#"{{"type":"user","timestamp":"{T}02.000Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"x","is_error":true}},{{"type":"text","text":"<system-reminder>ignore me</system-reminder>"}}]}}}}"#
            ),
        );
        s.append("s1.jsonl", "\n");
        let got = kinds(&ClaudeSource::open(&s.main()).poll());
        assert_eq!(
            got,
            vec!["usage main 35", "msg main>aaa Narrow scope", "error main"]
        );
    }

    #[test]
    fn missing_files_yield_nothing() {
        let s = Scratch::new("missing");
        assert!(ClaudeSource::open(&s.0.join("nope.jsonl"))
            .poll()
            .is_empty());
    }

    #[test]
    fn extracts_last_json_string() {
        let text = r#"{"cwd":"/a/b"} {"aiTitle":"Fix \"quoted\" bug","cwd":"/x/proj"}"#;
        assert_eq!(last_json_string(text, "cwd").as_deref(), Some("/x/proj"));
        assert_eq!(
            last_json_string(text, "aiTitle").as_deref(),
            Some("Fix \"quoted\" bug")
        );
        assert_eq!(last_json_string(text, "nope"), None);
    }
}
