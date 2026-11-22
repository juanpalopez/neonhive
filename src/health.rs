//! Stuck and loop detection.
//!
//! Each agent carries a [`Tracker`] that `World::apply` feeds with the agent's
//! tool calls, tool errors and text. The tracker keeps two counters: how often
//! the same call was repeated, and how many calls failed in a row.
//! [`assess`] turns those counters, plus how long the agent has been silent,
//! into a [`Health`] flag. Silence is measured against the world clock
//! (`World::now_ts`), never the wall clock, so a replay flags the same agents at
//! the same moments every time.
//!
//! Nothing here knows tool names: a call is just a name and a detail string.

use crate::util::{clean, fmt_dur};

/// The same call this many times in a row is a loop.
pub const LOOP_REPEATS: u32 = 4;
/// This many failed calls in a row, with no successful call between, is an
/// error streak.
pub const ERROR_STREAK: u32 = 3;
/// A running agent whose last activity was its own model turn (text, an error
/// result coming back) and that has been silent this long is stalled. Twice the
/// idle threshold: every stalled agent is also idle, not every idle one stalled.
pub const STALL_MS: i64 = 600_000;
/// Silence allowed after a tool call or message: the agent may be waiting on a
/// long build, a test suite or another agent.
pub const STALL_AFTER_CALL_MS: i64 = 1_800_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Health {
    #[default]
    Ok,
    Looping,
    Erroring,
    Stalled,
}

impl Health {
    pub fn is_flagged(self) -> bool {
        self != Health::Ok
    }
}

/// Per-agent counters behind the health flag.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// Last call as `tool` + NUL + `detail`. The detail is the adapter's
    /// truncated summary of the input, so two calls that differ only past the
    /// truncation point count as the same call. That is acceptable: a loop
    /// detector that is slightly eager on long, near-identical inputs is still
    /// pointing at the right agent.
    call: String,
    tool: String,
    /// How many times `call` was made in a row.
    pub repeats: u32,
    /// Failed calls in a row.
    pub err_streak: u32,
    err_tool: String,
    /// A call was made and no error has arrived since.
    pending: bool,
    /// The agent's last activity was a call (tool or message), so silence may
    /// mean it is waiting on something outside itself.
    waiting: bool,
    /// The flag last written to the event stream, so each flag is logged once.
    reported: Health,
}

impl Tracker {
    /// A tool call. An error, if any, arrives after the call it belongs to, so
    /// a call that is followed by another call or by text with no error between
    /// succeeded, and ends an error streak. Parallel calls blur this: with two
    /// calls in flight, the second one counts as proof that the first
    /// succeeded. That only ever delays a flag, never raises a false one.
    pub fn on_call(&mut self, tool: &str, detail: &str) {
        if self.pending {
            self.err_streak = 0;
        }
        self.pending = true;
        self.waiting = true;
        let call = format!("{tool}\0{detail}");
        if call == self.call {
            self.repeats += 1;
        } else {
            self.call = call;
            self.tool = tool.to_string();
            self.repeats = 1;
        }
    }

    /// A tool error for this agent's most recent call to `tool`.
    pub fn on_error(&mut self, tool: &str) {
        self.err_streak += 1;
        self.err_tool = tool.to_string();
        self.pending = false;
        self.waiting = false;
    }

    /// The agent wrote text: its last call came back. Text alone does not break
    /// a loop; "let me try again" between identical calls is still a loop.
    pub fn on_text(&mut self) {
        if self.pending {
            self.err_streak = 0;
        }
        self.pending = false;
        self.waiting = false;
    }

    /// The agent sent a message. Like a tool call, it may now wait for a reply.
    pub fn on_message(&mut self) {
        self.waiting = true;
    }

    /// Record the flag now in force. True when it is a new flag, which is the
    /// moment to write one line to the event stream.
    pub fn report(&mut self, h: Health) -> bool {
        let new = h.is_flagged() && h != self.reported;
        self.reported = h;
        new
    }

    /// Short reason, for the inspector and the event stream.
    pub fn reason(&self, h: Health, silent_ms: i64) -> String {
        match h {
            Health::Ok => "ok".into(),
            Health::Looping => format!("loop: {} x{}", clean(&self.tool, 24), self.repeats),
            Health::Erroring => format!(
                "errors: {} failed calls in a row ({})",
                self.err_streak,
                clean(&self.err_tool, 24)
            ),
            Health::Stalled => format!("stalled: silent {}", fmt_dur(silent_ms)),
        }
    }

    /// Very short form for a node box, which can be 16 cells wide.
    pub fn short(&self, h: Health) -> String {
        match h {
            Health::Ok => String::new(),
            Health::Looping => format!("loop x{}", self.repeats),
            Health::Erroring => format!("err x{}", self.err_streak),
            Health::Stalled => "stalled".into(),
        }
    }
}

/// The health of one agent.
///
/// Only running agents are flagged; finishing clears every flag. In order of
/// precedence:
///
/// 1. **Erroring**: [`ERROR_STREAK`] failed calls in a row.
/// 2. **Looping**: the same tool and detail [`LOOP_REPEATS`] times in a row.
/// 3. **Stalled**: silent for [`STALL_MS`], or [`STALL_AFTER_CALL_MS`] when the
///    last activity was a tool call or message (it may be waiting on a long
///    build or a reply). Never the main agent, which spends most of its life
///    waiting for the human, and never an agent with a running subagent: a
///    parent waiting on children is waiting, not stalled. If the child is the
///    one stuck, the child gets the flag.
///
/// `has_running_child` is only called when the agent has been silent long
/// enough to be stalled, so it may be slow.
pub fn assess(
    t: &Tracker,
    running: bool,
    is_root: bool,
    silent_ms: i64,
    has_running_child: impl FnOnce() -> bool,
) -> Health {
    if !running {
        return Health::Ok;
    }
    if t.err_streak >= ERROR_STREAK {
        return Health::Erroring;
    }
    if t.repeats >= LOOP_REPEATS {
        return Health::Looping;
    }
    let limit = if t.waiting {
        STALL_AFTER_CALL_MS
    } else {
        STALL_MS
    };
    if !is_root && silent_ms > limit && !has_running_child() {
        return Health::Stalled;
    }
    Health::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, Kind, ROOT};
    use crate::state::World;

    fn ev(ts: i64, kind: Kind) -> Event {
        Event { ts, kind }
    }
    fn spawn(id: &str, parent: &str) -> Kind {
        Kind::Spawn {
            id: id.into(),
            parent: parent.into(),
            agent_type: "builder".into(),
            label: "Fix it".into(),
        }
    }
    fn call(agent: &str, tool: &str, detail: &str) -> Kind {
        Kind::Tool {
            agent: agent.into(),
            tool: tool.into(),
            detail: detail.into(),
        }
    }
    fn err(agent: &str) -> Kind {
        Kind::ToolError {
            agent: agent.into(),
        }
    }
    fn text(agent: &str) -> Kind {
        Kind::Text {
            agent: agent.into(),
            text: "hmm".into(),
        }
    }
    fn finish(id: &str) -> Kind {
        Kind::Finish {
            id: id.into(),
            ok: true,
            tokens: None,
        }
    }
    fn world_with(agents: &[(&str, &str)]) -> World {
        let mut w = World::new("proj");
        for (id, parent) in agents {
            w.apply(&ev(1_000, spawn(id, parent)), false);
        }
        w
    }
    fn health(w: &World, id: &str) -> Health {
        w.health(&w.agents[id])
    }
    fn flag_lines(w: &World) -> usize {
        w.log
            .iter()
            .filter(|l| l.kind == crate::state::LogKind::Health)
            .count()
    }

    #[test]
    fn repeated_call_is_a_loop_and_logged_once() {
        let mut w = world_with(&[("a1", ROOT)]);
        for i in 0..(LOOP_REPEATS as i64 - 1) {
            w.apply(&ev(2_000 + i, call("a1", "Bash", "make")), false);
            // Text between identical calls does not break the loop.
            w.apply(&ev(2_000 + i, text("a1")), false);
        }
        assert_eq!(health(&w, "a1"), Health::Ok);
        w.apply(&ev(3_000, call("a1", "Bash", "make")), false);
        assert_eq!(health(&w, "a1"), Health::Looping);
        w.apply(&ev(3_001, call("a1", "Bash", "make")), false);
        assert_eq!(
            w.health_reason(&w.agents["a1"]).as_deref(),
            Some("loop: Bash x5")
        );
        assert_eq!(flag_lines(&w), 1);
        assert_eq!(w.flagged(), 1);
    }

    #[test]
    fn loop_clears_on_a_different_call_or_finish() {
        let mut w = world_with(&[("a1", ROOT), ("a2", ROOT)]);
        for id in ["a1", "a2"] {
            for i in 0..LOOP_REPEATS as i64 {
                w.apply(&ev(2_000 + i, call(id, "Read", "x.rs")), false);
            }
            assert_eq!(health(&w, id), Health::Looping);
        }
        // Same tool, different detail: different progress.
        w.apply(&ev(3_000, call("a1", "Read", "y.rs")), false);
        assert_eq!(health(&w, "a1"), Health::Ok);
        w.apply(&ev(3_000, finish("a2")), false);
        assert_eq!(health(&w, "a2"), Health::Ok);
        assert_eq!(w.flagged(), 0);
    }

    #[test]
    fn consecutive_failed_calls_are_an_error_streak() {
        let mut w = world_with(&[("a1", ROOT)]);
        let tools = ["Bash", "Edit", "Bash"];
        for (i, t) in tools.iter().enumerate() {
            let ts = 2_000 + i as i64 * 10;
            w.apply(&ev(ts, call("a1", t, &format!("try {i}"))), false);
            w.apply(&ev(ts + 1, err("a1")), false);
        }
        assert_eq!(health(&w, "a1"), Health::Erroring);
        assert_eq!(
            w.health_reason(&w.agents["a1"]).as_deref(),
            Some("errors: 3 failed calls in a row (Bash)")
        );
        assert_eq!(flag_lines(&w), 1);
    }

    #[test]
    fn a_successful_call_breaks_the_error_streak() {
        let mut w = world_with(&[("a1", ROOT)]);
        let mut ts = 2_000;
        let mut step = |w: &mut World, k: Kind| {
            ts += 1;
            w.apply(&ev(ts, k), false);
        };
        step(&mut w, call("a1", "Bash", "a"));
        step(&mut w, err("a1"));
        step(&mut w, call("a1", "Bash", "b"));
        step(&mut w, err("a1"));
        step(&mut w, call("a1", "Read", "c")); // no error follows: success
        step(&mut w, call("a1", "Bash", "d"));
        step(&mut w, err("a1"));
        assert_eq!(w.agents["a1"].health.err_streak, 1);
        assert_eq!(health(&w, "a1"), Health::Ok);
        // Errors from the streak are cleared by text after a good call too.
        step(&mut w, call("a1", "Bash", "e"));
        step(&mut w, err("a1"));
        step(&mut w, call("a1", "Bash", "f"));
        step(&mut w, err("a1"));
        assert_eq!(health(&w, "a1"), Health::Erroring);
        step(&mut w, call("a1", "Bash", "g"));
        step(&mut w, text("a1"));
        assert_eq!(health(&w, "a1"), Health::Ok);
    }

    #[test]
    fn successful_calls_never_flag() {
        let mut w = world_with(&[("a1", ROOT)]);
        for i in 0..10 {
            w.apply(&ev(2_000 + i, call("a1", "Grep", &format!("q{i}"))), false);
        }
        w.apply(&ev(3_000, err("a1")), false);
        assert_eq!(health(&w, "a1"), Health::Ok);
    }

    #[test]
    fn silent_agent_stalls_and_clears_on_activity() {
        let mut w = world_with(&[("a1", ROOT)]);
        w.apply(&ev(2_000, text("a1")), false);
        w.now_ts = 2_000 + STALL_MS - 1;
        w.sweep_health();
        assert_eq!(health(&w, "a1"), Health::Ok);
        assert!(w.is_idle(&w.agents["a1"])); // idle first, stalled later
        w.now_ts = 2_000 + STALL_MS + 1;
        w.sweep_health();
        w.sweep_health();
        assert_eq!(health(&w, "a1"), Health::Stalled);
        assert_eq!(flag_lines(&w), 1);
        let t = w.now_ts + 1;
        w.apply(&ev(t, text("a1")), false);
        assert_eq!(health(&w, "a1"), Health::Ok);
    }

    #[test]
    fn waiting_on_a_tool_gets_a_longer_grace() {
        let mut w = world_with(&[("a1", ROOT)]);
        w.apply(
            &ev(2_000, call("a1", "Bash", "cargo build --release")),
            false,
        );
        w.now_ts = 2_000 + STALL_MS + 1;
        assert_eq!(health(&w, "a1"), Health::Ok);
        w.now_ts = 2_000 + STALL_AFTER_CALL_MS + 1;
        assert_eq!(health(&w, "a1"), Health::Stalled);
    }

    #[test]
    fn parent_waiting_on_children_is_not_stalled() {
        let mut w = world_with(&[("lead", ROOT), ("kid", "lead")]);
        w.apply(&ev(2_000, text("lead")), false);
        // The child keeps working; the parent says nothing for a long time.
        let quiet = 2_000 + STALL_AFTER_CALL_MS * 2;
        w.apply(&ev(quiet, call("kid", "Bash", "test")), false);
        w.now_ts = quiet + 1;
        w.sweep_health();
        assert_eq!(health(&w, "lead"), Health::Ok);
        assert_eq!(health(&w, "kid"), Health::Ok);
        // Once the child is done, a silent parent is stalled.
        w.apply(&ev(quiet + 2, finish("kid")), false);
        w.sweep_health();
        assert_eq!(health(&w, "lead"), Health::Stalled);
    }

    #[test]
    fn main_agent_waiting_for_the_human_is_not_stalled() {
        let mut w = World::new("proj");
        w.apply(&ev(1_000, text(ROOT)), false);
        w.now_ts = 1_000 + STALL_AFTER_CALL_MS * 10;
        w.sweep_health();
        assert_eq!(health(&w, ROOT), Health::Ok);
        assert_eq!(w.flagged(), 0);
    }

    #[test]
    fn finished_agents_are_never_flagged() {
        let mut w = world_with(&[("a1", ROOT)]);
        w.apply(&ev(2_000, finish("a1")), false);
        w.now_ts = 2_000 + STALL_AFTER_CALL_MS * 10;
        assert_eq!(health(&w, "a1"), Health::Ok);
    }
}
