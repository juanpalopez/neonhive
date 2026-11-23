//! World state: a reducer over the normalized event stream.

use crate::event::{Event, Kind, Tokens, ROOT};
use crate::health::{self, Facts, Health, Tracker};
use crate::pricing;
use crate::util::clean;
use std::collections::{HashMap, HashSet, VecDeque};

/// A running agent with no activity for this long is shown as idle.
const IDLE_MS: i64 = 300_000;
/// Event-stream lines kept for scroll-back. A line is a timestamp, an agent id
/// and at most 160 chars of text (see `say`), about 250 bytes on average and
/// under 1 KiB at worst, so 10,000 lines cost about 2.5 MB, 10 MB at worst.
/// That holds the whole stream of most sessions; on the largest real sessions
/// it is the last few hours. Appending is O(1) either way.
const LOG_CAP: usize = 10_000;

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
    /// Loop, error-streak and stall counters. See [`crate::health`].
    pub health: Tracker,
    /// Finished at least once and then acted again.
    pub resumed: bool,
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
    /// An agent was first flagged as looping, erroring or stalled.
    Health,
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
    /// Lines ever logged. `log[i]` is line number `log_seq - log.len() + i`,
    /// a number that stays put when old lines are dropped.
    pub log_seq: u64,
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
            log_seq: 0,
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
        self.agents
            .values()
            .fold(0u64, |n, a| n.saturating_add(a.shown_tokens()))
    }

    pub fn running(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.id != ROOT && a.status == Status::Running && !self.is_idle(a))
            .count()
    }

    /// Health flag of an agent as of the last check: the last event for it or
    /// the last [`World::sweep_health`]. See [`health::assess`].
    pub fn health(&self, a: &Agent) -> Health {
        a.health.flag()
    }

    /// Why an agent is flagged, or `None` when it is healthy.
    pub fn health_reason(&self, a: &Agent) -> Option<String> {
        let h = self.health(a);
        h.is_flagged()
            .then(|| a.health.reason(h, self.now_ts - a.last_ts))
    }

    /// Number of agents with a health flag.
    pub fn flagged(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.health.flag().is_flagged())
            .count()
    }

    fn facts(&self, a: &Agent, has_running_child: bool) -> Facts {
        Facts {
            running: a.status == Status::Running,
            is_root: a.id == ROOT,
            resumed: a.resumed,
            silent_ms: self.now_ts - a.last_ts,
            has_running_child,
        }
    }

    /// Re-check every running agent against the current clock. Stalls are a
    /// matter of time passing with no events, so `apply` alone cannot notice
    /// them; the app calls this after it moves `now_ts`. Writes one log line
    /// per new flag. Runs every frame, so it looks up parents with a running
    /// child once, and allocates nothing else unless a flag changes.
    pub fn sweep_health(&mut self) {
        let busy: HashSet<&str> = self
            .agents
            .values()
            .filter(|a| a.status == Status::Running)
            .filter_map(|a| a.parent.as_deref())
            .collect();
        let changed: Vec<(String, Health)> = self
            .order
            .iter()
            .filter_map(|id| {
                let a = self.agents.get(id)?;
                if a.status != Status::Running {
                    return None;
                }
                let h = health::assess(&a.health, &self.facts(a, busy.contains(id.as_str())));
                (h != a.health.flag()).then(|| (id.clone(), h))
            })
            .collect();
        for (id, h) in changed {
            self.record_health(&id, h, self.now_ts);
        }
    }

    /// Sweep health at every moment before `until` at which a running agent
    /// passes its stall limit, as a clock moving towards `until` would, but
    /// never earlier than `floor`: a replay skips dead time and first looks
    /// again at `floor`. For a world rebuilt from events without the clock.
    pub fn sweep_stalls_until(&mut self, until: i64, floor: i64) {
        loop {
            let next = self
                .agents
                .values()
                .filter(|a| a.status == Status::Running && a.health.flag() != Health::Stalled)
                .map(|a| (a.last_ts + health::stall_limit(&a.health) + 1).max(floor))
                .filter(|&t| t > self.now_ts && t < until)
                .min();
            let Some(t) = next else { break };
            self.now_ts = t;
            self.sweep_health();
        }
    }

    /// Re-check one agent after an event of its own.
    fn note_health(&mut self, id: &str, ts: i64) {
        let Some(a) = self.agents.get(id) else {
            return;
        };
        // An agent that just acted is rarely silent, so the scan for children
        // is almost never needed.
        let silent = self.now_ts - a.last_ts;
        let child = silent > health::STALL_MS.min(health::STALL_AFTER_CALL_MS)
            && self.has_running_child(id);
        let h = health::assess(&a.health, &self.facts(a, child));
        self.record_health(id, h, ts);
    }

    /// Store the flag now in force; log it if newly flagged.
    fn record_health(&mut self, id: &str, h: Health, ts: i64) {
        let now = self.now_ts;
        let Some(a) = self.agents.get_mut(id) else {
            return;
        };
        if a.health.report(h) {
            let reason = a.health.reason(h, now - a.last_ts);
            self.say(ts, id, LogKind::Health, reason);
        }
    }

    fn has_running_child(&self, id: &str) -> bool {
        self.agents
            .values()
            .any(|c| c.status == Status::Running && c.parent.as_deref() == Some(id))
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
                    health: Tracker::default(),
                    resumed: false,
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
            a.resumed = true;
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
        self.log_seq += 1;
    }

    /// Resolve a message recipient given as id, id prefix, label or type.
    pub fn resolve(&self, to: &str) -> Option<String> {
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
                a.health = Tracker::default();
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
                self.touch(from, ts).health.on_message();
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
                key,
            } => {
                let a = self.touch(agent, ts);
                a.tools += 1;
                a.last_tool = tool.clone();
                a.health.on_call(tool, *key, ts);
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
                // A result coming back is activity: silence is measured from
                // here, not from the call. Not `touch`: a late error does not
                // resume a finished agent.
                a.last_ts = a.last_ts.max(ts);
                a.errors += 1;
                let tool = a.last_tool.clone();
                a.health.on_error(&tool);
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
                a.health.on_text();
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
        let actor = match &ev.kind {
            Kind::Message { from: id, .. }
            | Kind::Tool { agent: id, .. }
            | Kind::ToolError { agent: id }
            | Kind::Usage { agent: id, .. }
            | Kind::Text { agent: id, .. } => Some(id),
            _ => None,
        };
        if let Some(id) = actor {
            self.note_health(id, ts);
        }
    }
}

// --------------------------------------------------------------------- cost

impl Agent {
    /// Estimated cost in USD, summed over the models this agent used. None
    /// when any of them has no known price, or when only a reported total is
    /// known (it has no model split to price).
    pub fn cost(&self) -> Option<f64> {
        if self.tokens.is_zero() && self.reported_tokens.is_some() {
            return None;
        }
        let mut usd = 0.0;
        for (model, t) in &self.by_model {
            if !t.is_zero() {
                usd += pricing::cost(model, t)?;
            }
        }
        Some(usd)
    }
}

/// Session cost split between the top-level agent and its subagents. Agents
/// without a price are left out of the sums and counted in `unpriced`.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CostSummary {
    pub main: f64,
    pub subagents: f64,
    /// Agents that used tokens of a model with no known price.
    pub unpriced: usize,
}

impl CostSummary {
    pub fn total(&self) -> f64 {
        self.main + self.subagents
    }

    /// Share of the priced total spent by subagents, 0..1.
    pub fn subagent_share(&self) -> Option<f64> {
        (self.total() > 0.0).then(|| self.subagents / self.total())
    }
}

/// Totals for one agent type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeCost {
    pub agent_type: String,
    pub agents: usize,
    pub tokens: u64,
    /// Sum over agents of this type that have a price.
    pub usd: f64,
    pub unpriced: usize,
}

impl World {
    pub fn cost_summary(&self) -> CostSummary {
        let mut s = CostSummary::default();
        for a in self.agents.values() {
            match a.cost() {
                Some(c) if a.id == ROOT => s.main += c,
                Some(c) => s.subagents += c,
                None => s.unpriced += 1,
            }
        }
        s
    }

    /// One row per agent type, most expensive first; ties keep spawn order.
    pub fn cost_by_type(&self) -> Vec<TypeCost> {
        let mut rows: Vec<TypeCost> = Vec::new();
        for id in &self.order {
            let a = &self.agents[id];
            let i = match rows.iter().position(|r| r.agent_type == a.agent_type) {
                Some(i) => i,
                None => {
                    rows.push(TypeCost {
                        agent_type: a.agent_type.clone(),
                        agents: 0,
                        tokens: 0,
                        usd: 0.0,
                        unpriced: 0,
                    });
                    rows.len() - 1
                }
            };
            let row = &mut rows[i];
            row.agents += 1;
            row.tokens += a.shown_tokens();
            match a.cost() {
                Some(c) => row.usd += c,
                None => row.unpriced += 1,
            }
        }
        rows.sort_by(|a, b| b.usd.total_cmp(&a.usd));
        rows
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
            key: 1,
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
        // Line numbers keep counting past the cap (a loop flag adds one).
        assert!(w.log_seq >= LOG_CAP as u64 + 50);
    }

    fn spawn_typed(id: &str, agent_type: &str) -> Kind {
        Kind::Spawn {
            id: id.into(),
            parent: ROOT.into(),
            agent_type: agent_type.into(),
            label: String::new(),
        }
    }

    fn million_in(agent: &str, model: &str) -> Kind {
        usage(agent, model, 1_000_000, 0)
    }

    #[test]
    fn agent_cost_sums_over_models() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), false);
        w.apply(&ev(11, million_in("a1", "claude-opus-5-5")), false); // $4
        w.apply(&ev(12, million_in("a1", "claude-sonnet-5-5")), false); // $2
        let c = w.agents["a1"].cost().unwrap();
        assert!((c - 6.0).abs() < 1e-9, "{c}");
        // One unpriced model makes the whole agent unpriced.
        w.apply(&ev(13, million_in("a1", "mystery")), false);
        assert_eq!(w.agents["a1"].cost(), None);
    }

    #[test]
    fn one_hour_cache_writes_cost_more_than_five_minute_ones() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), false);
        let writes = |five, hour| Kind::Usage {
            agent: "a1".into(),
            model: "claude-opus-5-5".into(),
            tokens: Tokens {
                cache_write_5m: five,
                cache_write_1h: hour,
                ..Tokens::default()
            },
        };
        w.apply(&ev(11, writes(1_000_000, 0)), false); // $5
        w.apply(&ev(12, writes(0, 1_000_000)), false); // $8
        let c = w.agents["a1"].cost().unwrap();
        assert!((c - 13.0).abs() < 1e-9, "{c}");
    }

    #[test]
    fn agent_with_only_a_reported_total_has_no_cost() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), false);
        assert_eq!(w.agents["a1"].cost(), Some(0.0));
        let finish = Kind::Finish {
            id: "a1".into(),
            ok: true,
            tokens: Some(5_000),
        };
        w.apply(&ev(11, finish), false);
        assert_eq!(w.agents["a1"].cost(), None);
    }

    #[test]
    fn session_cost_splits_main_and_subagents() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn("a1", ROOT)), false);
        w.apply(&ev(10, spawn("a2", ROOT)), false);
        w.apply(&ev(11, million_in(ROOT, "claude-opus-5-5")), false); // $4
        w.apply(&ev(12, million_in("a1", "claude-opus-5-5")), false); // $4
        w.apply(&ev(13, million_in("a1", "claude-opus-5-5")), false); // $4
        w.apply(&ev(14, million_in("a2", "local-model")), false);
        let s = w.cost_summary();
        assert!((s.main - 4.0).abs() < 1e-9);
        assert!((s.subagents - 8.0).abs() < 1e-9);
        assert_eq!(s.unpriced, 1);
        let share = s.subagent_share().unwrap();
        assert!((share - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(World::new("p").cost_summary().subagent_share(), None);
    }

    #[test]
    fn cost_aggregates_per_agent_type() {
        let mut w = World::new("proj");
        w.apply(&ev(10, spawn_typed("e1", "explorer")), false);
        w.apply(&ev(10, spawn_typed("e2", "explorer")), false);
        w.apply(&ev(10, spawn_typed("r1", "reviewer")), false);
        w.apply(&ev(11, million_in("e1", "claude-sonnet-5-5")), false); // $2
        w.apply(&ev(12, million_in("e2", "claude-sonnet-5-5")), false); // $2
        w.apply(&ev(13, million_in("r1", "claude-opus-5-5")), false); // $4
        w.apply(&ev(14, million_in("e2", "local-model")), false);
        let rows = w.cost_by_type();
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.agent_type.as_str(), r.agents, r.unpriced, r.usd))
            .collect();
        assert_eq!(
            got,
            vec![
                ("reviewer", 1, 0, 4.0),
                ("explorer", 2, 1, 2.0),
                ("main", 1, 0, 0.0),
            ]
        );
        assert_eq!(rows[1].tokens, 3_000_000);
    }
}
