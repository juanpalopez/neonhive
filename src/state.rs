//! World state: a reducer over the normalized event stream.

use crate::event::{Event, Kind, Tokens, ROOT};
use crate::util::clean;
use std::collections::{HashMap, VecDeque};

/// A running agent with no activity for this long is shown as idle.
const IDLE_MS: i64 = 300_000;
const LOG_CAP: usize = 400;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Running,
    Done,
    Failed,
}

pub struct Agent {
    pub id: String,
    pub parent: Option<String>,
    pub agent_type: String,
    pub label: String,
    pub model: String,
    pub status: Status,
    pub spawned_ts: i64,
    pub finished_ts: Option<i64>,
    pub last_ts: i64,
    /// Sum of this agent's `Usage` events.
    pub tokens: Tokens,
    /// The same, split by model. An agent can change model mid-session.
    pub by_model: Vec<(String, Tokens)>,
    /// Total reported when the agent finished; a fallback for agents whose
    /// own usage was never seen.
    pub reported_tokens: Option<u64>,
    pub tools: u32,
    pub errors: u32,
    pub prompt: String,
    pub last_text: String,
    pub last_tool: String,
    pub recent_tools: VecDeque<String>,
    /// Spawn order; keeps the layout stable.
    pub order: usize,
}

impl Agent {
    /// The token count to show for this agent. See [`Tokens::fresh`].
    pub fn shown_tokens(&self) -> u64 {
        match self.tokens.fresh() {
            0 => self.reported_tokens.unwrap_or(0),
            n => n,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum LogKind {
    Spawn,
    Done,
    Fail,
    Tool,
    Error,
    Msg,
    Text,
    Prompt,
}

pub struct LogLine {
    pub ts: i64,
    pub agent: String,
    pub kind: LogKind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PacketKind {
    Spawn,
    Result,
    Fail,
    Message,
}

/// Animation requests produced by events, consumed by the view.
pub enum Fx {
    Packet {
        from: String,
        to: String,
        kind: PacketKind,
    },
    Flash(String),
}

pub struct World {
    pub agents: HashMap<String, Agent>,
    pub order: Vec<String>,
    pub log: VecDeque<LogLine>,
    pub now_ts: i64,
    pub total_tools: u64,
    pub total_msgs: u64,
    /// Tool calls by tool name, over all agents.
    pub tool_counts: HashMap<String, u64>,
    pub fx: Vec<Fx>,
}

impl World {
    pub fn new(root_label: &str) -> Self {
        let mut w = Self {
            agents: HashMap::new(),
            order: Vec::new(),
            log: VecDeque::new(),
            now_ts: 0,
            total_tools: 0,
            total_msgs: 0,
            tool_counts: HashMap::new(),
            fx: Vec::new(),
        };
        w.ensure(ROOT, 0);
        let root = w.agents.get_mut(ROOT).unwrap();
        root.parent = None;
        root.agent_type = "main".into();
        root.label = root_label.to_string();
        w
    }

    pub fn is_idle(&self, a: &Agent) -> bool {
        a.status == Status::Running && self.now_ts - a.last_ts > IDLE_MS
    }

    pub fn total_tokens(&self) -> u64 {
        self.agents.values().map(Agent::shown_tokens).sum()
    }

    pub fn running(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.id != ROOT && a.status == Status::Running && !self.is_idle(a))
            .count()
    }

    fn ensure(&mut self, id: &str, ts: i64) -> &mut Agent {
        if !self.agents.contains_key(id) {
            self.order.push(id.to_string());
            self.agents.insert(
                id.to_string(),
                Agent {
                    id: id.to_string(),
                    parent: Some(ROOT.to_string()),
                    agent_type: "agent".into(),
                    label: String::new(),
                    model: String::new(),
                    status: Status::Running,
                    spawned_ts: ts,
                    finished_ts: None,
                    last_ts: ts,
                    tokens: Tokens::default(),
                    by_model: Vec::new(),
                    reported_tokens: None,
                    tools: 0,
                    errors: 0,
                    prompt: String::new(),
                    last_text: String::new(),
                    last_tool: String::new(),
                    recent_tools: VecDeque::new(),
                    order: self.order.len() - 1,
                },
            );
        }
        self.agents.get_mut(id).unwrap()
    }

    /// Activity on an agent; a finished agent that acts again was resumed.
    fn touch(&mut self, id: &str, ts: i64) -> &mut Agent {
        let a = self.ensure(id, ts);
        if a.spawned_ts == 0 {
            a.spawned_ts = ts;
        }
        a.last_ts = ts;
        if a.status != Status::Running {
            a.status = Status::Running;
            a.finished_ts = None;
        }
        a
    }

    fn say(&mut self, ts: i64, agent: &str, kind: LogKind, text: String) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogLine {
            ts,
            agent: agent.to_string(),
            kind,
            text,
        });
    }

    /// Resolve a message recipient given as id, id prefix, label or type.
    fn resolve(&self, to: &str) -> Option<String> {
        if self.agents.contains_key(to) {
            return Some(to.to_string());
        }
        self.order
            .iter()
            .rev()
            .find(|id| {
                let a = &self.agents[*id];
                (!to.is_empty() && id.starts_with(to)) || a.label == to || a.agent_type == to
            })
            .cloned()
    }

    /// Apply one event. `animate` is false while loading history in bulk.
    pub fn apply(&mut self, ev: &Event, animate: bool) {
        let ts = ev.ts;
        self.now_ts = self.now_ts.max(ts);
        match &ev.kind {
            Kind::Spawn {
                id,
                parent,
                agent_type,
                label,
            } => {
                self.ensure(parent, ts);
                let a = self.ensure(id, ts);
                a.parent = Some(parent.clone());
                a.agent_type = agent_type.clone();
                a.label = clean(label, 80);
                a.spawned_ts = ts;
                let text = format!("spawn {} :: {}", agent_type, clean(label, 80));
                self.say(ts, id, LogKind::Spawn, text);
                if animate {
                    self.fx.push(Fx::Packet {
                        from: parent.clone(),
                        to: id.clone(),
                        kind: PacketKind::Spawn,
                    });
                }
            }
            Kind::Finish { id, ok, tokens } => {
                // Notifications also fire for background shell tasks; those ids
                // are not agents.
                let Some(a) = self.agents.get_mut(id) else {
                    return;
                };
                if a.status != Status::Running {
                    return; // repeated notification
                }
                a.status = if *ok { Status::Done } else { Status::Failed };
                a.finished_ts = Some(ts);
                a.reported_tokens = tokens.filter(|t| *t > 0);
                let parent = a.parent.clone().unwrap_or_else(|| ROOT.to_string());
                let (kind, pk, text) = if *ok {
                    (
                        LogKind::Done,
                        PacketKind::Result,
                        "finished, result returned",
                    )
                } else {
                    (LogKind::Fail, PacketKind::Fail, "failed")
                };
                self.say(ts, id, kind, text.to_string());
                if animate {
                    self.fx.push(Fx::Packet {
                        from: id.clone(),
                        to: parent,
                        kind: pk,
                    });
                }
            }
            Kind::Message { from, to, summary } => {
                self.touch(from, ts);
                self.total_msgs += 1;
                let target = self.resolve(to);
                let name = target
                    .as_ref()
                    .map(|t| self.agents[t].agent_type.clone())
                    .unwrap_or_else(|| to.clone());
                self.say(
                    ts,
                    from,
                    LogKind::Msg,
                    format!("message to {name}: {summary}"),
                );
                if let (true, Some(to)) = (animate, target) {
                    self.fx.push(Fx::Packet {
                        from: from.clone(),
                        to,
                        kind: PacketKind::Message,
                    });
                }
            }
            Kind::Tool {
                agent,
                tool,
                detail,
            } => {
                let a = self.touch(agent, ts);
                a.tools += 1;
                a.last_tool = tool.clone();
                let line = if detail.is_empty() {
                    tool.clone()
                } else {
                    format!("{tool}: {detail}")
                };
                if a.recent_tools.len() >= 8 {
                    a.recent_tools.pop_front();
                }
                a.recent_tools.push_back(line.clone());
                self.total_tools += 1;
                *self.tool_counts.entry(tool.clone()).or_insert(0) += 1;
                self.say(ts, agent, LogKind::Tool, line);
                if animate {
                    self.fx.push(Fx::Flash(agent.clone()));
                }
            }
            Kind::ToolError { agent } => {
                let a = self.ensure(agent, ts);
                a.errors += 1;
                let text = format!("tool error ({})", a.last_tool);
                self.say(ts, agent, LogKind::Error, text);
            }
            Kind::Usage {
                agent,
                tokens,
                model,
            } => {
                let a = self.touch(agent, ts);
                a.tokens.add(tokens);
                match a.by_model.iter_mut().find(|(m, _)| m == model) {
                    Some((_, t)) => t.add(tokens),
                    None => a.by_model.push((model.clone(), *tokens)),
                }
                if !model.is_empty() {
                    a.model = model.clone();
                }
            }
            Kind::Text { agent, text } => {
                let a = self.touch(agent, ts);
                a.last_text = text.clone();
                a.last_tool.clear();
                self.say(ts, agent, LogKind::Text, clean(text, 160));
            }
            Kind::Prompt { agent, text } => {
                let a = self.ensure(agent, ts);
                if agent == ROOT {
                    a.prompt = text.clone();
                    a.last_ts = ts;
                    self.say(ts, agent, LogKind::Prompt, clean(text, 160));
                } else if a.prompt.is_empty() {
                    a.prompt = text.clone();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: i64, kind: Kind) -> Event {
        Event { ts, kind }
    }
    fn spawn(id: &str, parent: &str) -> Kind {
        Kind::Spawn {
            id: id.into(),
            parent: parent.into(),
            agent_type: "explorer".into(),
            label: "Map code".into(),
        }
    }
    fn tool(agent: &str) -> Kind {
        Kind::Tool {
            agent: agent.into(),
            tool: "Bash".into(),
            detail: "ls".into(),
        }
    }

    fn usage(agent: &str, model: &str, input: u64, cache_read: u64) -> Kind {
        Kind::Usage {
            agent: agent.into(),
            tokens: Tokens {
                input,
                cache_read,
                ..Tokens::default()
            },
            model: model.into(),
        }
    }

    #[test]
    fn usage_adds_up_per_agent_and_per_model() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), true);
        w.apply(&ev(11, usage("a1", "big", 100, 1_000)), true);
        w.apply(&ev(12, usage("a1", "small", 10, 0)), true);
        w.apply(&ev(13, usage("a1", "big", 5, 2_000)), true);
        let a = &w.agents["a1"];
        assert_eq!((a.tokens.input, a.tokens.cache_read), (115, 3_000));
        // Cache reads are not part of the headline number.
        assert_eq!(a.shown_tokens(), 115);
        let per: Vec<_> = a
            .by_model
            .iter()
            .map(|(m, t)| (m.as_str(), t.input))
            .collect();
        assert_eq!(per, vec![("big", 105), ("small", 10)]);
        assert_eq!(a.model, "big"); // the most recent one
    }

    #[test]
    fn own_usage_wins_over_the_total_reported_on_finish() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), true);
        w.apply(&ev(11, usage("a1", "big", 100, 0)), true);
        let finish = Kind::Finish {
            id: "a1".into(),
            ok: true,
            tokens: Some(9_999),
        };
        w.apply(&ev(12, finish), true);
        assert_eq!(w.agents["a1"].shown_tokens(), 100);
    }

    #[test]
    fn spawn_work_finish() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), true);
        w.apply(&ev(20, tool("a1")), true);
        w.apply(
            &ev(
                30,
                Kind::Finish {
                    id: "a1".into(),
                    ok: true,
                    tokens: Some(500),
                },
            ),
            true,
        );
        let a = &w.agents["a1"];
        assert_eq!(a.status, Status::Done);
        assert_eq!(
            (a.tools, a.shown_tokens(), a.finished_ts),
            (1, 500, Some(30))
        );
        assert_eq!(a.parent.as_deref(), Some(ROOT));
        assert_eq!(w.total_tools, 1);
        // spawn packet, tool flash, result packet
        assert_eq!(w.fx.len(), 3);
    }

    #[test]
    fn bulk_load_produces_no_animation() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), false);
        w.apply(&ev(20, tool("a1")), false);
        assert!(w.fx.is_empty());
    }

    #[test]
    fn finish_for_unknown_id_is_ignored() {
        let mut w = World::new("proj");
        w.apply(
            &ev(
                10,
                Kind::Finish {
                    id: "bg-shell-1".into(),
                    ok: true,
                    tokens: None,
                },
            ),
            true,
        );
        assert_eq!(w.order, vec![ROOT.to_string()]);
    }

    #[test]
    fn repeated_finish_is_ignored_and_activity_resumes_agent() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), false);
        w.apply(
            &ev(
                20,
                Kind::Finish {
                    id: "a1".into(),
                    ok: false,
                    tokens: None,
                },
            ),
            false,
        );
        w.apply(
            &ev(
                30,
                Kind::Finish {
                    id: "a1".into(),
                    ok: true,
                    tokens: None,
                },
            ),
            false,
        );
        assert_eq!(w.agents["a1"].status, Status::Failed);
        w.apply(&ev(40, tool("a1")), false);
        assert_eq!(w.agents["a1"].status, Status::Running);
        assert_eq!(w.agents["a1"].finished_ts, None);
    }

    #[test]
    fn events_for_unseen_agent_create_placeholder() {
        let mut w = World::new("proj");
        w.apply(&ev(10, tool("ghost")), false);
        assert_eq!(w.agents["ghost"].parent.as_deref(), Some(ROOT));
        // The real spawn arrives later and fills in the details.
        w.apply(&ev(11, spawn("ghost", ROOT)), false);
        assert_eq!(w.agents["ghost"].agent_type, "explorer");
        assert_eq!(w.order.len(), 2);
    }

    #[test]
    fn message_resolves_recipient_by_id_prefix_or_type() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("abc123", ROOT)), false);
        for to in ["abc123", "abc", "explorer"] {
            w.apply(
                &ev(
                    20,
                    Kind::Message {
                        from: ROOT.into(),
                        to: to.into(),
                        summary: "hi".into(),
                    },
                ),
                true,
            );
            assert!(matches!(w.fx.pop(), Some(Fx::Packet { to, .. }) if to == "abc123"));
        }
        w.apply(
            &ev(
                30,
                Kind::Message {
                    from: ROOT.into(),
                    to: "nobody".into(),
                    summary: "hi".into(),
                },
            ),
            true,
        );
        assert!(w.fx.is_empty());
        assert_eq!(w.total_msgs, 4);
    }

    #[test]
    fn idle_after_long_silence() {
        let mut w = World::new("proj");
        w.apply(&ev(1_000, spawn("a1", ROOT)), false);
        w.now_ts = 1_000 + IDLE_MS + 1;
        assert!(w.is_idle(&w.agents["a1"]));
        assert_eq!(w.running(), 0);
    }

    #[test]
    fn log_is_bounded() {
        let mut w = World::new("proj");
        for i in 0..(LOG_CAP as i64 + 50) {
            w.apply(&ev(i, tool(ROOT)), false);
        }
        assert_eq!(w.log.len(), LOG_CAP);
    }
}
