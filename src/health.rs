//! Stuck and loop detection.
//!
//! Each agent carries a [`Tracker`] that `World::apply` feeds with the agent's
//! tool calls, tool errors and text. The tracker keeps two counters: how often
//! the same call was repeated, and how many calls failed in a row.
//! [`assess`] turns those counters, plus how long the agent has been silent,
//! into a [`Health`] flag. Time is always the event or world clock
//! (`World::now_ts`), never the wall clock, so a replay flags the same agents
//! at the same moments every time.
//!
//! Nothing here knows tool names: a call is a name for display and an opaque
//! key from the adapter that is equal exactly when two calls have the same
//! tool and the same full input.

use crate::util::{clean, fmt_dur};

/// The same call this many times in a row, each within [`REPEAT_WINDOW_MS`]
/// of the one before, is a loop.
pub const LOOP_REPEATS: u32 = 4;
/// An identical call only counts as a repeat when it is issued within this
/// long of the previous one (issue to issue). Identical calls further apart
/// are waiting: polling a build, sleeping between checks, re-running a slow
/// suite. On real transcripts the two cases are far apart: identical calls
/// that the agent re-issued without waiting followed each other within 2 to
/// 10 seconds, while every poll that waited took at least 256 seconds between
/// calls (most about 10 minutes). A minute sits between both with margin for
/// a slow model turn. The cost: a poll that waits under a minute between
/// identical calls counts as a loop, and a loop slower than a minute per turn
/// goes unflagged.
pub const REPEAT_WINDOW_MS: i64 = 60_000;
/// This many failed calls in a row, with no call known to have succeeded
/// between, is an error streak.
pub const ERROR_STREAK: u32 = 3;
/// A running agent whose last activity was its own model turn (text) and that
/// has been silent this long is stalled. Twice the idle threshold: every
/// stalled agent is also idle, not every idle one stalled.
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
    /// Key of the last call, from the adapter. Covers the whole input, unlike
    /// the display detail, so different commands that share a prefix and
    /// different edits to one file are different calls.
    key: Option<u64>,
    /// When the last call was issued.
    call_ts: i64,
    tool: String,
    /// How many times the last call was made in a row, each within
    /// [`REPEAT_WINDOW_MS`] of the previous.
    pub repeats: u32,
    /// Failed calls in a row.
    pub err_streak: u32,
    err_tool: String,
    /// A call was made and no error has arrived since.
    pending: bool,
    /// The agent's last activity was a call (tool or message), so silence may
    /// mean it is waiting on something outside itself.
    waiting: bool,
    /// The flag in force as of the last check, so each flag is logged once.
    /// Also what the views show, so they do not re-assess every frame.
    flag: Health,
}

impl Tracker {
    /// A tool call issued at `ts`. An error, if any, arrives after the call it
    /// belongs to, so a call that is followed by another call or by text with
    /// no error between succeeded, and ends an error streak.
    ///
    /// Success is never reported directly, only inferred that way, which
    /// parallel calls blur in both directions. With calls A and B in flight,
    /// issuing B counts as proof that A succeeded, which can delay a flag. And
    /// in a batch where A fails, B succeeds and C fails, nothing marks B's
    /// success between the two errors, so the streak counts 2: failures from
    /// one batch add up even when a sibling succeeded.
    pub fn on_call(&mut self, tool: &str, key: u64, ts: i64) {
        if self.pending {
            self.err_streak = 0;
        }
        self.pending = true;
        self.waiting = true;
        if self.key == Some(key) && ts - self.call_ts <= REPEAT_WINDOW_MS {
            self.repeats += 1;
        } else {
            self.key = Some(key);
            if self.tool != tool {
                self.tool = tool.to_string();
            }
            self.repeats = 1;
        }
        self.call_ts = ts;
    }

    /// A tool error for this agent's most recent call to `tool`. It does not
    /// end the wait: with parallel calls a sibling may still be running, so
    /// only text or the next call does.
    pub fn on_error(&mut self, tool: &str) {
        self.err_streak += 1;
        if self.err_tool != tool {
            self.err_tool = tool.to_string();
        }
        self.pending = false;
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

    /// The flag in force as of the last [`Tracker::report`].
    pub fn flag(&self) -> Health {
        self.flag
    }

    /// Record the flag now in force. True when it is a new flag, which is the
    /// moment to write one line to the event stream.
    pub fn report(&mut self, h: Health) -> bool {
        let new = h.is_flagged() && h != self.flag;
        self.flag = h;
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

/// What [`assess`] needs to know about an agent besides its [`Tracker`].
pub struct Facts {
    pub running: bool,
    pub is_root: bool,
    /// The agent finished once and then acted again.
    pub resumed: bool,
    /// Time since the agent's last activity, tool errors included.
    pub silent_ms: i64,
    pub has_running_child: bool,
}

/// Silence after which [`assess`] can call an agent with this tracker
/// stalled: [`STALL_AFTER_CALL_MS`] while it waits on a call or message,
/// [`STALL_MS`] otherwise.
pub fn stall_limit(t: &Tracker) -> i64 {
    if t.waiting {
        STALL_AFTER_CALL_MS
    } else {
        STALL_MS
    }
}

/// The health of one agent.
///
/// Only running agents are flagged; finishing clears every flag. In order of
/// precedence:
///
/// 1. **Erroring**: [`ERROR_STREAK`] failed calls in a row.
/// 2. **Looping**: the same call (same tool, same full input)
///    [`LOOP_REPEATS`] times in a row, each within [`REPEAT_WINDOW_MS`] of the
///    previous.
/// 3. **Stalled**: silent for [`STALL_MS`] after text, or
///    [`STALL_AFTER_CALL_MS`] when the last call or message has not been
///    followed by text (it may be waiting on a long build or a reply; an error
///    for one call does not end that wait). Never:
///    - the main agent, which spends most of its life waiting for the human;
///    - an agent with a running subagent: a parent waiting on children is
///      waiting, not stalled. If the child is the one stuck, the child gets
///      the flag;
///    - a resumed agent, one that finished and then acted again. Its second
///      completion is often never written down, so after its last event it
///      looks exactly like an agent that hangs, forever. Missing a real hang
///      in a resumed agent is the cheaper mistake.
pub fn assess(t: &Tracker, f: &Facts) -> Health {
    if !f.running {
        return Health::Ok;
    }
    if t.err_streak >= ERROR_STREAK {
        return Health::Erroring;
    }
    if t.repeats >= LOOP_REPEATS {
        return Health::Looping;
    }
    let limit = stall_limit(t);
    if !f.is_root && !f.resumed && f.silent_ms > limit && !f.has_running_child {
        return Health::Stalled;
    }
    Health::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, Kind, ROOT};
    use crate::state::World;
    use crate::util::str_hash64;

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
    /// A call whose full input is `input`, shown as `detail`.
    fn call_with(agent: &str, tool: &str, detail: &str, input: &str) -> Kind {
        Kind::Tool {
            agent: agent.into(),
            tool: tool.into(),
            detail: detail.into(),
            key: str_hash64(&[tool, input]),
        }
    }
    /// A call whose display detail is its whole input.
    fn call(agent: &str, tool: &str, detail: &str) -> Kind {
        call_with(agent, tool, detail, detail)
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
    /// Move the world clock and re-check, as the app does every frame.
    fn at(w: &mut World, ts: i64) -> Health {
        w.now_ts = ts;
        w.sweep_health();
        w.health(&w.agents["a1"])
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
        // Same tool, different input: different progress.
        w.apply(&ev(3_000, call("a1", "Read", "y.rs")), false);
        assert_eq!(health(&w, "a1"), Health::Ok);
        w.apply(&ev(3_000, finish("a2")), false);
        assert_eq!(health(&w, "a2"), Health::Ok);
        assert_eq!(w.flagged(), 0);
    }

    #[test]
    fn different_inputs_with_the_same_display_detail_do_not_loop() {
        let mut w = world_with(&[("a1", ROOT)]);
        // Successive edits to one file, and commands that share a long
        // prefix: the adapter shows them alike, but they are different calls.
        for i in 0..10 {
            let ts = 2_000 + i * 1_000;
            let edit = format!("{{\"file\":\"lib.rs\",\"old\":\"v{i}\"}}");
            w.apply(&ev(ts, call_with("a1", "Edit", "lib.rs", &edit)), false);
            let cmd = format!("cd /a/very/long/path/that/fills/the/summary && step {i}");
            w.apply(
                &ev(ts + 1, call_with("a1", "Bash", "cd /a/very/long…", &cmd)),
                false,
            );
        }
        assert_eq!(health(&w, "a1"), Health::Ok);
        assert_eq!(flag_lines(&w), 0);
    }

    #[test]
    fn identical_calls_far_apart_are_polling_not_a_loop() {
        let mut w = world_with(&[("a1", ROOT)]);
        // A wait-and-check call issued again every few minutes.
        let gap = REPEAT_WINDOW_MS + 1;
        for i in 0..10 {
            w.apply(
                &ev(2_000 + i * gap, call("a1", "Bash", "sleep 300; status")),
                false,
            );
            w.apply(&ev(2_000 + i * gap + 1, text("a1")), false);
        }
        assert_eq!(health(&w, "a1"), Health::Ok);
        // Quick re-issues of the same call do loop, and a long pause in the
        // middle starts the count over.
        let t = 2_000 + 10 * gap;
        for i in 0..(LOOP_REPEATS as i64 - 1) {
            w.apply(&ev(t + i * 2_000, call("a1", "Monitor", "watch")), false);
        }
        let late = t + (LOOP_REPEATS as i64 - 2) * 2_000 + gap;
        w.apply(&ev(late, call("a1", "Monitor", "watch")), false);
        assert_eq!(w.agents["a1"].health.repeats, 1);
        for i in 1..LOOP_REPEATS as i64 {
            w.apply(
                &ev(late + i * REPEAT_WINDOW_MS, call("a1", "Monitor", "watch")),
                false,
            );
        }
        assert_eq!(health(&w, "a1"), Health::Looping);
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
    fn a_sibling_success_in_a_parallel_batch_is_not_seen() {
        // The documented limitation: A fails, B succeeds, C fails. Nothing
        // reports B's success, so the two failures add up.
        let mut w = world_with(&[("a1", ROOT)]);
        for (i, t) in ["A", "B", "C"].iter().enumerate() {
            w.apply(&ev(2_000 + i as i64, call("a1", "Bash", t)), false);
        }
        w.apply(&ev(2_010, err("a1")), false);
        w.apply(&ev(2_011, err("a1")), false);
        assert_eq!(w.agents["a1"].health.err_streak, 2);
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
        assert_eq!(at(&mut w, 2_000 + STALL_MS - 1), Health::Ok);
        assert!(w.is_idle(&w.agents["a1"])); // idle first, stalled later
        assert_eq!(at(&mut w, 2_000 + STALL_MS + 1), Health::Stalled);
        w.sweep_health();
        assert_eq!(flag_lines(&w), 1);
        assert_eq!(w.flagged(), 1);
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
        assert_eq!(at(&mut w, 2_000 + STALL_MS + 1), Health::Ok);
        assert_eq!(at(&mut w, 2_000 + STALL_AFTER_CALL_MS + 1), Health::Stalled);
    }

    #[test]
    fn an_error_after_a_long_call_is_not_an_instant_stall() {
        let mut w = world_with(&[("a1", ROOT)]);
        w.apply(&ev(2_000, call("a1", "Bash", "slow test suite")), false);
        // The call fails after 11 minutes: silence starts over from there.
        let back = 2_000 + STALL_MS + 60_000;
        w.now_ts = back;
        w.apply(&ev(back, err("a1")), false);
        assert_eq!(at(&mut w, back), Health::Ok);
        assert_eq!(at(&mut w, back + STALL_MS + 1), Health::Ok);
        assert_eq!(at(&mut w, back + STALL_AFTER_CALL_MS + 1), Health::Stalled);
    }

    #[test]
    fn a_sibling_error_keeps_the_grace_of_a_call_in_flight() {
        let mut w = world_with(&[("a1", ROOT)]);
        // Two calls in parallel: a quick one that fails, a long test suite.
        w.apply(&ev(2_000, call("a1", "Bash", "full test suite")), false);
        w.apply(&ev(2_000, call("a1", "Read", "missing.rs")), false);
        w.apply(&ev(2_100, err("a1")), false);
        assert_eq!(at(&mut w, 2_100 + STALL_MS + 1), Health::Ok);
        assert_eq!(at(&mut w, 2_100 + STALL_AFTER_CALL_MS + 1), Health::Stalled);
    }

    #[test]
    fn a_resumed_agent_is_never_stalled() {
        let mut w = world_with(&[("a1", ROOT)]);
        w.apply(&ev(2_000, finish("a1")), false);
        // Resumed, works a little, and no second completion is ever written.
        w.apply(&ev(3_000, text("a1")), false);
        assert!(w.agents["a1"].resumed);
        assert_eq!(at(&mut w, 3_000 + STALL_AFTER_CALL_MS * 10), Health::Ok);
        // Loops are still caught.
        for i in 0..LOOP_REPEATS as i64 {
            let t = w.now_ts + i;
            w.apply(&ev(t, call("a1", "Bash", "make")), false);
        }
        assert_eq!(health(&w, "a1"), Health::Looping);
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
        w.apply(&ev(2_000, text("a1")), false);
        assert_eq!(at(&mut w, 2_000 + STALL_MS + 1), Health::Stalled);
        w.apply(&ev(w.now_ts, finish("a1")), false);
        assert_eq!(at(&mut w, 2_000 + STALL_AFTER_CALL_MS * 10), Health::Ok);
        assert_eq!(w.flagged(), 0);
    }
}
