//! Normalized event stream. Every source adapter (Claude Code, Codex CLI and
//! the demo today) emits these; the rest of the app never sees a source format.

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
    /// Input tokens written to the 5-minute prompt cache (1.25x input price).
    /// A usage record without a 5m/1h split lands here in full.
    pub cache_write_5m: u64,
    /// Input tokens written to the 1-hour prompt cache (2x input price).
    pub cache_write_1h: u64,
}

impl Tokens {
    /// All cache writes, whichever cache lifetime they used.
    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m.saturating_add(self.cache_write_1h)
    }

    /// Split a combined cache-write count into its 5-minute and 1-hour parts.
    ///
    /// `combined` is `cache_creation_input_tokens`; `split` is the optional
    /// `(ephemeral_5m, ephemeral_1h)` pair. The combined number is the one
    /// the headline count has always used, so it wins when the parts
    /// disagree: the 1-hour part is kept (capped at `combined`) and the
    /// 5-minute part is the remainder. With no split, everything is
    /// 5-minute. When the parts sum to `combined` nothing changes. Never
    /// panics and the two results never sum to more than `combined`.
    pub fn split_cache_write(combined: u64, split: Option<(u64, u64)>) -> (u64, u64) {
        let one_hour = split.map_or(0, |(_, h)| h).min(combined);
        (combined - one_hour, one_hour)
    }

    /// Tokens the model processed for the first time: everything except cache
    /// reads. A long session reads the same cached context on every turn, so
    /// including reads would count that context once per turn.
    pub fn fresh(&self) -> u64 {
        // Saturating: counts come from files and a corrupt one must not panic.
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_write_5m)
            .saturating_add(self.cache_write_1h)
    }

    pub fn is_zero(&self) -> bool {
        *self == Tokens::default()
    }

    pub fn add(&mut self, o: &Tokens) {
        self.input = self.input.saturating_add(o.input);
        self.output = self.output.saturating_add(o.output);
        self.cache_read = self.cache_read.saturating_add(o.cache_read);
        self.cache_write_5m = self.cache_write_5m.saturating_add(o.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.saturating_add(o.cache_write_1h);
    }

    /// Field-wise `self - o`, never below zero.
    pub fn minus(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(o.input),
            output: self.output.saturating_sub(o.output),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            cache_write_5m: self.cache_write_5m.saturating_sub(o.cache_write_5m),
            cache_write_1h: self.cache_write_1h.saturating_sub(o.cache_write_1h),
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
        /// Short display summary of the input. Lossy: never compare it.
        detail: String,
        /// Identity of the call: equal for two calls with the same tool and
        /// the same full input, and (barring hash collisions) different
        /// otherwise. Used only to spot repeated calls.
        key: u64,
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
