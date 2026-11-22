//! Normalized event stream. Every source adapter (Claude Code today, Codex and
//! others later) emits these; the rest of the app never sees a source format.

/// Id of the top-level agent of a session.
pub const ROOT: &str = "main";

/// Tokens of one or more model responses, split by how they are billed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input tokens that were not served from or written to a cache.
    pub input: u64,
    pub output: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
}

impl Tokens {
    /// Tokens the model processed for the first time: everything except cache
    /// reads. A long session reads the same cached context on every turn, so
    /// including reads would count that context once per turn.
    pub fn fresh(&self) -> u64 {
        self.input + self.output + self.cache_write
    }

    pub fn is_zero(&self) -> bool {
        *self == Tokens::default()
    }

    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }

    /// Field-wise `self - o`, never below zero.
    pub fn minus(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(o.input),
            output: self.output.saturating_sub(o.output),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            cache_write: self.cache_write.saturating_sub(o.cache_write),
        }
    }
}

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
        /// Total reported by the tool itself. Used only when the agent has no
        /// `Usage` events of its own.
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
        tokens: Tokens,
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
