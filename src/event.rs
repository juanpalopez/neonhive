//! Normalized event stream. Every source adapter (Claude Code today, Codex and
//! others later) emits these; the rest of the app never sees a source format.

/// Id of the top-level agent of a session.
pub const ROOT: &str = "main";

#[derive(Debug, Clone)]
pub struct Event {
    /// Unix time in milliseconds.
    pub ts: i64,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Spawn {
        id: String,
        parent: String,
        agent_type: String,
        label: String,
    },
    Finish {
        id: String,
        ok: bool,
        tokens: Option<u64>,
    },
    Message {
        from: String,
        to: String,
        summary: String,
    },
    Tool {
        agent: String,
        tool: String,
        detail: String,
    },
    ToolError {
        agent: String,
    },
    Usage {
        agent: String,
        tokens: u64,
        model: String,
    },
    Text {
        agent: String,
        text: String,
    },
    Prompt {
        agent: String,
        text: String,
    },
}

/// A source of agent events. `poll` returns the events that arrived since the
/// previous call; the first call returns the whole history.
pub trait Source {
    fn poll(&mut self) -> Vec<Event>;
}
