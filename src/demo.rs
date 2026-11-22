//! Synthetic source: an endless, scripted agent swarm. Needs no agent tool
//! installed, so it doubles as a showcase and as a second `Source` adapter.

use crate::event::{Event, Kind, Source, Tokens, ROOT};
use crate::util::{hash, now_ms};

const TYPES: [&str; 6] = [
    "explorer",
    "code-reviewer",
    "test-runner",
    "architect",
    "researcher",
    "builder",
];
const LABELS: [&str; 10] = [
    "Map auth module call graph",
    "Review diff for race conditions",
    "Run integration test suite",
    "Design cache invalidation",
    "Survey rate limiter crates",
    "Patch flaky retry logic",
    "Trace memory leak in worker",
    "Audit SQL for injection",
    "Benchmark hot path",
    "Write migration plan",
];
const TOOLS: [(&str, &str); 8] = [
    ("Bash", "cargo test --workspace"),
    ("Read", "scheduler.rs"),
    ("Grep", "fn handle_.*request"),
    ("Edit", "retry.rs"),
    ("Bash", "git diff --stat"),
    ("WebSearch", "token bucket vs leaky bucket"),
    ("Read", "Cargo.toml"),
    ("Write", "plan.md"),
];
const TEXTS: [&str; 5] = [
    "Found the root cause: lock held across await.",
    "Three call sites need the new signature.",
    "Tests pass. Summarizing findings for the parent.",
    "Two candidates fit; comparing maintenance status.",
    "Reproduced the failure with a minimal case.",
];

struct Live {
    id: String,
    until: i64,
}

pub struct DemoSource {
    seed: u32,
    step: u32,
    now: i64,
    next: i64,
    spawned: u32,
    live: Vec<Live>,
}

impl DemoSource {
    pub fn new(seed: u32) -> Self {
        let now = now_ms();
        Self {
            seed,
            step: 0,
            now,
            next: now,
            spawned: 0,
            live: Vec::new(),
        }
    }

    fn rand(&mut self, n: u32) -> u32 {
        self.step += 1;
        hash(self.seed, self.step, 0x5EED) % n.max(1)
    }

    fn next_event(&mut self, ts: i64) -> Kind {
        if self.step == 0 {
            self.step = 1;
            return Kind::Prompt {
                agent: ROOT.into(),
                text: "Find and fix the flaky checkout test".into(),
            };
        }
        if let Some(i) = self.live.iter().position(|l| l.until <= ts) {
            let done = self.live.remove(i);
            let ok = self.rand(8) != 0;
            // Not every scripted agent emits usage; this total covers those.
            let tokens = Some(8_000 + self.rand(90_000) as u64);
            return Kind::Finish {
                id: done.id,
                ok,
                tokens,
            };
        }
        if self.live.len() < 3 || (self.live.len() < 9 && self.rand(7) == 0) {
            self.spawned += 1;
            let id = format!("demo{:04}", self.spawned);
            // One spawn in four is nested under a running subagent.
            let parent = match self.rand(4) {
                0 if !self.live.is_empty() => {
                    let i = self.rand(self.live.len() as u32) as usize;
                    self.live[i].id.clone()
                }
                _ => ROOT.to_string(),
            };
            let until = ts + 6_000 + self.rand(20_000) as i64;
            self.live.push(Live {
                id: id.clone(),
                until,
            });
            return Kind::Spawn {
                id,
                parent,
                agent_type: TYPES[self.rand(TYPES.len() as u32) as usize].into(),
                label: LABELS[self.rand(LABELS.len() as u32) as usize].into(),
            };
        }
        let i = self.rand(self.live.len() as u32) as usize;
        let agent = self.live[i].id.clone();
        match self.rand(20) {
            0 => Kind::ToolError { agent },
            1 => Kind::Message {
                from: ROOT.into(),
                to: agent,
                summary: "Narrow scope to the retry path".into(),
            },
            2 | 3 => Kind::Text {
                agent,
                text: TEXTS[self.rand(TEXTS.len() as u32) as usize].into(),
            },
            n @ 4..=7 => Kind::Usage {
                // The main agent and its subagents run on different models,
                // so the demo shows a cost split. Prices come from the
                // built-in table; these are real model ids.
                model: if n == 7 {
                    "claude-opus-5-5"
                } else {
                    "claude-sonnet-5-5"
                }
                .into(),
                agent: if n == 7 { ROOT.into() } else { agent },
                tokens: Tokens {
                    input: 40 + self.rand(300) as u64,
                    output: 300 + self.rand(2_000) as u64,
                    cache_read: 20_000 + self.rand(60_000) as u64,
                    cache_write: self.rand(4_000) as u64,
                },
            },
            8 => Kind::Tool {
                agent: ROOT.into(),
                tool: "Read".into(),
                detail: "checkout.test.ts".into(),
            },
            _ => {
                let (tool, detail) = TOOLS[self.rand(TOOLS.len() as u32) as usize];
                Kind::Tool {
                    agent,
                    tool: tool.into(),
                    detail: detail.into(),
                }
            }
        }
    }
}

impl Source for DemoSource {
    /// Each poll advances the script by 250 ms, matching the app's poll rate.
    /// Time is scripted, not read from the wall clock, so runs are repeatable.
    fn poll(&mut self) -> Vec<Event> {
        self.now += 250;
        let mut out = Vec::new();
        while self.next <= self.now {
            let ts = self.next;
            let kind = self.next_event(ts);
            out.push(Event { ts, kind });
            self.next += 60 + self.rand(420) as i64;
        }
        out
    }
}
