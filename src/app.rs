//! View model: playback clock, graph layout and animation state.

use crate::event::{Event, Kind, Source, ROOT};
use crate::state::{Fx, LogLine, PacketKind, Status, World};
use crate::util::{clean, contains_ci, needle, now_ms};
use std::collections::{HashMap, VecDeque};
use std::f32::consts::{FRAC_PI_2, TAU};

/// Longest wall-clock wait between two replayed events, in seconds.
const MAX_GAP_SECS: f64 = 1.0;
const POLL_SECS: f64 = 0.25;
pub const SPEEDS: [f64; 8] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
pub const BORN_SECS: f64 = 0.6;
pub const DYING_SECS: f64 = 0.6;
const PACKET_SECS: f32 = 0.9;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Live,
    Replay,
}

pub struct NodeView {
    /// Position in normalized graph space, each axis in -1..1.
    pub x: f32,
    pub y: f32,
    pub born: f64,
    pub dying: Option<f64>,
    pub flash: f64,
}

pub struct Packet {
    pub from: String,
    pub to: String,
    pub t: f32,
    pub kind: PacketKind,
}

pub struct App {
    pub world: World,
    source: Box<dyn Source>,
    history: Vec<Event>,
    cursor: usize,
    /// Per applied event: whether it wrote a line to the event stream. Filled
    /// as events are applied; applying is deterministic, so it stays valid
    /// when the world is rebuilt.
    logged: Vec<bool>,
    root_label: String,
    pub title: String,
    pub mode: Mode,
    /// Replay position: unix ms on the session's own timeline.
    vt: f64,
    pub speed_idx: usize,
    pub paused: bool,
    /// Animation clock in seconds since the view opened.
    pub clock: f64,
    last_poll: f64,
    pub nodes: HashMap<String, NodeView>,
    pub packets: Vec<Packet>,
    pub selected: usize,
    pub follow: bool,
    pub activity: VecDeque<u32>,
    act_acc: u32,
    act_t: f64,
    /// Graph panel size in cells; set by the renderer.
    pub view: (u16, u16),
    /// Node box width in cells, chosen from how crowded the graph is.
    pub box_w: u16,
    /// Agents not on the graph, and how many of them are flagged.
    pub hidden: usize,
    pub hidden_flagged: usize,
    pub help: bool,
    /// The roster panel shows cost by agent type instead of agents.
    pub cost_view: bool,
    /// Focus, panel sizes, scroll positions and the stream search.
    pub panes: Panes,
    /// Show a plain-language caption for the most recent event.
    pub captions: bool,
    /// Live mode may pause into a replay. Set for scripted sources (the
    /// demo); a real live session cannot be paused or stepped.
    pub pausable: bool,
}

impl App {
    pub fn new(mut source: Box<dyn Source>, mode: Mode, root_label: &str, title: &str) -> Self {
        let history = source.poll();
        let mut app = Self {
            world: World::new(root_label),
            source,
            vt: history.first().map(|e| e.ts as f64).unwrap_or(0.0),
            history,
            cursor: 0,
            logged: Vec::new(),
            root_label: root_label.to_string(),
            title: title.to_string(),
            mode,
            speed_idx: 3,
            paused: false,
            clock: 0.0,
            last_poll: 0.0,
            nodes: HashMap::new(),
            packets: Vec::new(),
            selected: 0,
            follow: true,
            activity: VecDeque::from(vec![0; 24]),
            act_acc: 0,
            act_t: 0.0,
            view: (100, 30),
            box_w: 24,
            hidden: 0,
            hidden_flagged: 0,
            help: false,
            cost_view: false,
            panes: Panes::default(),
            captions: false,
            pausable: false,
        };
        if mode == Mode::Live {
            app.jump_to_end();
        }
        app
    }

    pub fn speed(&self) -> f64 {
        SPEEDS[self.speed_idx]
    }

    pub fn at_end(&self) -> bool {
        self.cursor >= self.history.len()
    }

    /// Apply everything not yet shown, without animation, and follow live.
    pub fn jump_to_end(&mut self) {
        while !self.at_end() {
            self.apply_next(false);
        }
        self.world.fx.clear();
        self.mode = Mode::Live;
        self.stream_tail();
        if let Some(last) = self.world.order.iter().rposition(|id| id != ROOT) {
            self.selected = last;
        }
    }

    pub fn restart(&mut self) {
        self.world = World::new(&self.root_label);
        self.cursor = 0;
        self.vt = self.history.first().map(|e| e.ts as f64).unwrap_or(0.0);
        self.mode = Mode::Replay;
        self.paused = false;
        self.nodes.clear();
        self.packets.clear();
        self.selected = 0;
        self.follow = true;
        self.stream_tail();
    }

    pub fn select(&mut self, delta: i32) {
        let n = self.world.order.len() as i32;
        self.selected = (self.selected as i32 + delta).clamp(0, (n - 1).max(0)) as usize;
        self.follow = false;
    }

    /// Apply the event at the cursor and move past it. Returns whether it
    /// wrote a line to the event stream.
    fn apply_next(&mut self, animate: bool) -> bool {
        let ev = self.history[self.cursor].clone();
        let seq = self.world.log_seq;
        self.world.apply(&ev, animate);
        let shown = self.world.log_seq != seq;
        match self.logged.get_mut(self.cursor) {
            Some(l) => *l = shown,
            None => self.logged.push(shown),
        }
        self.cursor += 1;
        if animate {
            self.act_acc += 1;
        }
        if self.follow {
            let id = match &ev.kind {
                Kind::Spawn { id, .. } => id,
                Kind::Tool { agent, .. } | Kind::Text { agent, .. } => agent,
                _ => return shown,
            };
            if id != ROOT {
                if let Some(a) = self.world.agents.get(id) {
                    self.selected = a.order;
                }
            }
        }
        shown
    }

    fn step(&mut self) {
        self.apply_next(true);
    }

    /// Space: pause or resume. A pausable live view (the demo) pauses into
    /// a replay at the current event; `l` goes live again.
    pub fn toggle_pause(&mut self) {
        if self.mode == Mode::Live && self.pausable {
            self.freeze();
        } else {
            self.paused = !self.paused;
        }
    }

    /// Stop a live view where it is and continue as a paused replay.
    fn freeze(&mut self) {
        self.mode = Mode::Replay;
        self.paused = true;
        if let Some(last) = self.cursor.checked_sub(1) {
            self.vt = self.history[last].ts as f64;
        }
    }

    /// Get ready to step: pause a replay, freeze a pausable live view.
    /// False for a live session, which cannot be stepped.
    fn enter_step(&mut self) -> bool {
        match self.mode {
            Mode::Live if self.pausable => self.freeze(),
            Mode::Live => return false,
            Mode::Replay => self.paused = true,
        }
        true
    }

    /// Step forward: apply every event of the next timestamp, and keep going
    /// one timestamp at a time until something wrote a line to the event
    /// stream. Events that write none (token usage, a subagent's prompt)
    /// ride along with the next one that does. Returns whether it moved.
    pub fn step_forward(&mut self) -> bool {
        if !self.enter_step() {
            return false;
        }
        let start = self.cursor;
        while !self.at_end() {
            let ts = self.history[self.cursor].ts;
            let mut shown = false;
            while !self.at_end() && self.history[self.cursor].ts == ts {
                shown |= self.apply_next(true);
            }
            self.vt = ts as f64;
            if shown {
                break;
            }
        }
        self.cursor > start
    }

    /// Cursor positions that stepping forward from the start stops at, up to
    /// `upto`, with 0 first: the end of every timestamp group that wrote a
    /// line to the event stream. Uses what was recorded when the events were
    /// applied, so it applies nothing.
    fn step_points(&self, upto: usize) -> Vec<usize> {
        let mut points = vec![0];
        let mut i = 0;
        while i < upto {
            let ts = self.history[i].ts;
            let mut shown = false;
            while i < upto && self.history[i].ts == ts {
                shown |= self.logged.get(i).copied().unwrap_or(false);
                i += 1;
            }
            if shown {
                points.push(i);
            }
        }
        points
    }

    /// Step back to where the previous forward step stopped: the state just
    /// before the most recent event that wrote a stream line. The world is
    /// rebuilt from the start of the session, without animation, so the cost
    /// grows with the position in the session. Returns whether it moved.
    pub fn step_back(&mut self) -> bool {
        if !self.enter_step() || self.cursor == 0 {
            return false;
        }
        let points = self.step_points(self.cursor);
        let target = points[points.len().saturating_sub(2)];
        self.rebuild(target);
        true
    }

    /// Rebuild the world from the start up to (not including) event `target`.
    fn rebuild(&mut self, target: usize) {
        self.world = World::new(&self.root_label);
        self.cursor = 0;
        // Stalls are found by the clock, not by events: sweep where playback
        // would have, at each stall limit passed before the next event, or
        // where playback lands after skipping dead time.
        let max_gap = (MAX_GAP_SECS * 1000.0 * self.speed()) as i64;
        while self.cursor < target.min(self.history.len()) {
            let ts = self.history[self.cursor].ts;
            if self.cursor > 0 {
                self.world.sweep_stalls_until(ts, ts - max_gap);
            }
            self.apply_next(false);
        }
        self.world.fx.clear();
        self.packets.clear();
        self.vt = match self.cursor.checked_sub(1) {
            Some(last) => self.history[last].ts as f64,
            None => self.history.first().map(|e| e.ts as f64).unwrap_or(0.0),
        };
        self.world.now_ts = self.vt as i64;
        self.world.sweep_health();
        self.selected = self.selected.min(self.world.order.len() - 1);
        // Keep the stream scroll and the current match while their lines
        // still exist; follow the tail when they are gone.
        let seq = self.world.log_seq;
        if self.panes.anchor.is_some_and(|a| a >= seq) {
            self.panes.anchor = None;
        }
        self.panes.paused_at = self.panes.paused_at.min(seq);
        if self.panes.current.is_some_and(|c| c >= seq) {
            self.panes.current = None;
        }
    }

    /// Whether `.` and `,` do anything: a replay, or a live view that can be
    /// paused (the demo).
    pub fn can_step(&self) -> bool {
        self.mode == Mode::Replay || self.pausable
    }

    /// The most recent event that wrote a stream line, and how many other
    /// such events share its timestamp: what a caption describes.
    pub fn caption_event(&self) -> Option<(&Event, usize)> {
        let last = (0..self.cursor)
            .rev()
            .find(|&i| self.logged.get(i) == Some(&true))?;
        let ev = &self.history[last];
        let others = (0..last)
            .rev()
            .take_while(|&i| self.history[i].ts == ev.ts)
            .filter(|&i| self.logged.get(i) == Some(&true))
            .count();
        Some((ev, others))
    }

    /// Events before the one a caption describes, newest last; a caption
    /// may look back for context (the call a tool error belongs to).
    pub fn applied(&self) -> &[Event] {
        &self.history[..self.cursor]
    }

    pub fn tick(&mut self, dt: f64) {
        self.clock += dt;
        if self.clock - self.last_poll >= POLL_SECS {
            self.last_poll = self.clock;
            let mut fresh = self.source.poll();
            self.history.append(&mut fresh);
        }

        match self.mode {
            Mode::Live => {
                while !self.at_end() {
                    self.step();
                }
                self.world.now_ts = self.world.now_ts.max(now_ms());
            }
            Mode::Replay => {
                if !self.paused && !self.at_end() {
                    self.vt += dt * 1000.0 * self.speed();
                    // Skip dead time: never wait longer than MAX_GAP_SECS.
                    let next = self.history[self.cursor].ts as f64;
                    let max_gap = MAX_GAP_SECS * 1000.0 * self.speed();
                    if next - self.vt > max_gap {
                        self.vt = next - max_gap;
                    }
                    while !self.at_end() && self.history[self.cursor].ts as f64 <= self.vt {
                        self.step();
                    }
                }
                self.world.now_ts = self.vt as i64;
            }
        }
        self.world.sweep_health();

        for fx in std::mem::take(&mut self.world.fx) {
            match fx {
                Fx::Packet { from, to, kind } => {
                    if self.packets.len() < 300 {
                        self.packets.push(Packet {
                            from,
                            to,
                            t: 0.0,
                            kind,
                        });
                    }
                }
                Fx::Flash(id) => {
                    if let Some(n) = self.nodes.get_mut(&id) {
                        n.flash = self.clock;
                    }
                }
            }
        }
        for p in &mut self.packets {
            p.t += dt as f32 / PACKET_SECS;
        }
        self.packets.retain(|p| p.t < 1.0);

        self.update_nodes(dt);

        self.act_t += dt;
        if self.act_t >= 0.5 {
            self.act_t = 0.0;
            self.activity.pop_front();
            self.activity.push_back(self.act_acc);
            self.act_acc = 0;
        }
        if self.panes.inspect_for != self.selected {
            self.panes.inspect_for = self.selected;
            self.panes.inspect_scroll = 0;
        }
    }

    fn update_nodes(&mut self, dt: f64) {
        // Half the non-overlapping box slots of the panel, so edges stay readable.
        let (w, h) = (self.view.0 as usize, self.view.1 as usize);
        let cap = ((w / 20) * (h / 4) / 2).clamp(3, 16);
        // Only a selection the user made is pinned: while following, the
        // selection jumps to whichever agent acted last, and pinning it would
        // bring back the churn that spawn order removes.
        let pinned = (!self.follow)
            .then(|| self.world.order.get(self.selected))
            .flatten();
        let bound = node_bound(self.view);
        let visible = choose_visible(&self.world, cap, bound, pinned.map(String::as_str));
        self.hidden = self.world.order.len() - visible.len();
        self.hidden_flagged = self
            .world
            .agents
            .values()
            .filter(|a| a.health.flag().is_flagged() && !visible.contains(&a.id))
            .count();
        self.box_w = fit_box_width(self.view, visible.len());
        let space = Space::new(self.view, self.box_w);
        let targets = layout(&self.world, &visible, &space);
        let ease = 1.0 - (-dt as f32 * 6.0).exp();
        for id in &visible {
            let (tx, ty) = targets[id];
            if !self.nodes.contains_key(id) {
                // New nodes emerge from their parent.
                let (x, y) = self
                    .world
                    .agents
                    .get(id)
                    .and_then(|a| a.parent.as_ref())
                    .and_then(|p| self.nodes.get(p))
                    .map(|n| (n.x, n.y))
                    .unwrap_or((tx, ty));
                self.nodes.insert(
                    id.clone(),
                    NodeView {
                        x,
                        y,
                        born: self.clock,
                        dying: None,
                        flash: -1.0,
                    },
                );
            }
            let n = self.nodes.get_mut(id).unwrap();
            if n.dying.take().is_some() {
                n.born = self.clock;
            }
            n.x += (tx - n.x) * ease;
            n.y += (ty - n.y) * ease;
        }
        let clock = self.clock;
        for (id, n) in &mut self.nodes {
            if n.dying.is_none() && !visible.contains(id) {
                n.dying = Some(clock);
            }
        }
        self.nodes
            .retain(|_, n| n.dying.is_none_or(|d| clock - d < DYING_SECS));
    }
}

// ------------------------------------------------------------------ panes

/// A panel that can take keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Panel {
    #[default]
    Graph,
    Inspector,
    Roster,
    Stream,
}

impl Panel {
    const ORDER: [Panel; 4] = [Panel::Graph, Panel::Inspector, Panel::Roster, Panel::Stream];

    /// The panel `delta` steps away in focus order. Without the side column
    /// (narrow terminal) the inspector and roster are skipped.
    pub fn step(self, delta: i32, with_side: bool) -> Panel {
        let usable = |p: &Panel| with_side || matches!(p, Panel::Graph | Panel::Stream);
        let ring: Vec<Panel> = Self::ORDER.iter().copied().filter(usable).collect();
        let at = ring.iter().position(|p| *p == self).unwrap_or(0) as i32;
        ring[(at + delta).rem_euclid(ring.len() as i32) as usize]
    }
}

/// Smallest default event-stream height in rows, borders included. Taller
/// terminals get more: see `default_stream_height`.
pub const STREAM_H: u16 = 9;
/// Smallest event stream: two lines of text.
pub const STREAM_MIN_H: u16 = 4;
/// Rows the graph keeps however tall the stream gets.
pub const BODY_MIN_H: u16 = 8;
/// Default side column (inspector and roster) width.
pub const SIDE_W: u16 = 40;
/// Narrowest side column that still shows a readable inspector.
pub const SIDE_MIN_W: u16 = 30;
/// Columns the graph keeps however wide the side column gets.
pub const GRAPH_MIN_W: u16 = 60;
/// Terminals narrower than this show the graph without a side column.
pub const SIDE_MIN_TERM_W: u16 = 104;

/// Stream height before the user resizes it: 35% of the rows between header
/// and footer, and at least `STREAM_H`. A fixed 9 rows left the stream at
/// seven lines of text however tall the terminal was.
pub fn default_stream_height(term_h: u16) -> u16 {
    let share = u32::from(term_h.saturating_sub(2)) * 35 / 100;
    (share as u16).max(STREAM_H)
}

/// Stream height used in a terminal `term_h` rows tall: the preference (or
/// the default when there is none), clamped so header, footer and
/// `BODY_MIN_H` rows of graph still fit.
pub fn stream_height(pref: Option<u16>, term_h: u16) -> u16 {
    let pref = pref.unwrap_or_else(|| default_stream_height(term_h));
    let avail = term_h.saturating_sub(2);
    let max = avail
        .saturating_sub(BODY_MIN_H)
        .max(STREAM_MIN_H)
        .min(avail);
    pref.clamp(STREAM_MIN_H.min(max), max)
}

/// Side column width used in a terminal `term_w` columns wide.
pub fn side_width(pref: u16, term_w: u16) -> u16 {
    let max = term_w.saturating_sub(GRAPH_MIN_W).max(SIDE_MIN_W);
    pref.clamp(SIDE_MIN_W, max)
}

/// View state that is not about the agents: focus, sizes, scroll positions
/// and the event-stream search. The renderer reads it and writes back only
/// what it measured (`term`, `stream_rows`, `inspect_*`, `side_shown`).
pub struct Panes {
    pub focus: Panel,
    /// The focused panel fills the whole body.
    pub maximised: bool,
    /// Preferred event-stream height and side-column width. The size used is
    /// clamped to the terminal (`stream_height`, `side_width`); the key
    /// handlers clamp the preference to the current terminal, and a terminal
    /// that shrinks and grows back gets the same sizes again. `stream_h` is
    /// `None` until the user resizes the stream: the height then follows the
    /// terminal (`default_stream_height`).
    pub stream_h: Option<u16>,
    pub side_w: u16,
    /// Line number (`World::log_seq` numbering) of the bottom line shown in
    /// the event stream; `None` follows the tail.
    pub anchor: Option<u64>,
    /// `World::log_seq` when the stream stopped following.
    paused_at: u64,
    pub inspect_scroll: usize,
    /// The selection the inspector scroll belongs to.
    inspect_for: usize,
    /// Search query; empty when there is no search.
    pub query: String,
    /// The query line is open and takes every key.
    pub input: bool,
    /// Show only the lines that match the query.
    pub filter: bool,
    /// Line number of the match that `n` / `N` moved to.
    pub current: Option<u64>,
    /// Measured by the renderer: terminal size, text rows of the stream
    /// panel, largest useful inspector scroll, whether the side column shows.
    pub term: (u16, u16),
    pub stream_rows: usize,
    pub inspect_rows: usize,
    pub inspect_max: usize,
    pub side_shown: bool,
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            focus: Panel::Graph,
            maximised: false,
            stream_h: None,
            side_w: SIDE_W,
            anchor: None,
            paused_at: 0,
            inspect_scroll: 0,
            inspect_for: 0,
            query: String::new(),
            input: false,
            filter: false,
            current: None,
            term: (140, 40),
            stream_rows: STREAM_H as usize - 2,
            inspect_rows: 10,
            inspect_max: 0,
            side_shown: true,
        }
    }
}

/// The agent column of a stream line, as shown and as searched.
pub fn log_who(world: &World, l: &LogLine) -> String {
    let who = world
        .agents
        .get(&l.agent)
        .map_or(&l.agent, |a| &a.agent_type);
    clean(who, 14)
}

impl App {
    pub fn cycle_focus(&mut self, delta: i32) {
        let with_side = self.panes.side_shown || self.panes.maximised;
        self.panes.focus = self.panes.focus.step(delta, with_side);
    }

    pub fn resize_stream(&mut self, delta: i32) {
        let p = &mut self.panes;
        let now = stream_height(p.stream_h, p.term.1) as i32;
        p.stream_h = Some(stream_height(Some((now + delta).max(0) as u16), p.term.1));
    }

    pub fn resize_side(&mut self, delta: i32) {
        let p = &mut self.panes;
        let now = side_width(p.side_w, p.term.0) as i32;
        p.side_w = side_width((now + delta).max(0) as u16, p.term.0);
    }

    /// Line number of `log[i]`.
    pub fn log_no(&self, i: usize) -> u64 {
        self.world.log_seq - self.world.log.len() as u64 + i as u64
    }

    /// Whether the stream shows only matching lines right now.
    pub fn filtering(&self) -> bool {
        self.panes.filter && !self.panes.query.is_empty()
    }

    /// Indices into `world.log` of the lines matching the query.
    pub fn matches(&self) -> Vec<usize> {
        let n = needle(&self.panes.query);
        if n.is_empty() {
            return Vec::new();
        }
        let w = &self.world;
        // The agent column depends only on the agent: test it once each.
        let mut who: HashMap<&str, bool> = HashMap::new();
        (0..w.log.len())
            .filter(|&i| {
                let l = &w.log[i];
                contains_ci(&l.text, &n)
                    || *who
                        .entry(l.agent.as_str())
                        .or_insert_with(|| contains_ci(&log_who(w, l), &n))
            })
            .collect()
    }

    /// Indices into `world.log` the stream can show, oldest first: every
    /// line, or only the matches while filtering.
    pub fn stream_list(&self) -> Vec<usize> {
        if self.filtering() {
            self.matches()
        } else {
            (0..self.world.log.len()).collect()
        }
    }

    /// Position in `list` of the bottom line shown.
    pub fn stream_bottom(&self, list: &[usize]) -> Option<usize> {
        let last = list.len().checked_sub(1)?;
        let Some(anchor) = self.panes.anchor else {
            return Some(last);
        };
        // The last listed line at or before the anchor; the oldest one if the
        // anchor line has been dropped from the log.
        let n = list.partition_point(|&i| self.log_no(i) <= anchor);
        Some(n.saturating_sub(1).min(last))
    }

    /// Lines logged since the stream stopped following.
    pub fn new_lines(&self) -> u64 {
        match self.panes.anchor {
            Some(_) => self.world.log_seq.saturating_sub(self.panes.paused_at),
            None => 0,
        }
    }

    fn pin_stream(&mut self, line: u64) {
        if self.panes.anchor.is_none() {
            self.panes.paused_at = self.world.log_seq;
        }
        self.panes.anchor = Some(line);
    }

    /// Follow new lines again.
    pub fn stream_tail(&mut self) {
        self.panes.anchor = None;
        self.panes.current = None;
    }

    /// Move the stream view by `delta` rows; negative is back in time.
    /// Reaching the newest line follows the tail again.
    pub fn scroll_stream(&mut self, delta: i64) {
        let list = self.stream_list();
        let Some(bottom) = self.stream_bottom(&list) else {
            return;
        };
        let last = list.len() - 1;
        let lowest = self.panes.stream_rows.max(1).saturating_sub(1).min(last);
        let to = (bottom as i64).saturating_add(delta);
        let to = to.clamp(lowest as i64, last as i64) as usize;
        if to == last {
            self.panes.anchor = None;
        } else {
            self.pin_stream(self.log_no(list[to]));
        }
    }

    /// Oldest line at the top of the panel.
    pub fn stream_top(&mut self) {
        self.scroll_stream(i64::MIN);
    }

    /// One page of stream rows.
    pub fn stream_page(&self) -> i64 {
        self.panes.stream_rows.saturating_sub(1).max(1) as i64
    }

    /// Open the query line for a new search.
    pub fn search_open(&mut self) {
        let p = &mut self.panes;
        p.input = true;
        p.query.clear();
        p.current = None;
        p.focus = Panel::Stream;
    }

    /// Close the query line and go to the newest match on screen or above.
    pub fn search_confirm(&mut self) {
        self.panes.input = false;
        if self.panes.query.is_empty() {
            self.search_clear();
        } else {
            self.search_step(true);
        }
    }

    pub fn search_clear(&mut self) {
        let p = &mut self.panes;
        p.input = false;
        p.query.clear();
        p.filter = false;
        p.current = None;
    }

    /// Go to the next older or newer match, wrapping around, and scroll it
    /// into view.
    pub fn search_step(&mut self, older: bool) {
        let nos: Vec<u64> = self.matches().iter().map(|&i| self.log_no(i)).collect();
        let (Some(&first), Some(&last)) = (nos.first(), nos.last()) else {
            self.panes.current = None;
            return;
        };
        let from = match self.panes.current.filter(|c| nos.contains(c)) {
            Some(c) => c,
            None => {
                // Start from the bottom line shown; it is itself a candidate
                // for the first older match.
                let list = self.stream_list();
                let bottom = self.stream_bottom(&list).map(|b| self.log_no(list[b]));
                let at = bottom.unwrap_or(last);
                if older {
                    at.saturating_add(1)
                } else {
                    at
                }
            }
        };
        let pick = if older {
            nos.iter()
                .rev()
                .find(|&&n| n < from)
                .copied()
                .unwrap_or(last)
        } else {
            nos.iter().find(|&&n| n > from).copied().unwrap_or(first)
        };
        self.panes.current = Some(pick);
        self.reveal(pick);
    }

    /// Scroll the stream so line `no` shows, centred, if it is off screen.
    fn reveal(&mut self, no: u64) {
        let list = self.stream_list();
        let Some(bottom) = self.stream_bottom(&list) else {
            return;
        };
        let Some(pos) = list.iter().position(|&i| self.log_no(i) == no) else {
            return;
        };
        let rows = self.panes.stream_rows.max(1);
        if pos <= bottom && pos + rows > bottom {
            return; // already on screen; a following stream keeps following
        }
        let to = (pos + rows / 2).clamp((rows - 1).min(list.len() - 1), list.len() - 1);
        self.pin_stream(self.log_no(list[to]));
    }

    /// Scroll the inspector by `delta` rows. Stops following, so the agent
    /// being read does not change underneath.
    pub fn scroll_inspector(&mut self, delta: i64) {
        let p = &mut self.panes;
        let to = (p.inspect_scroll as i64).saturating_add(delta);
        p.inspect_scroll = to.clamp(0, p.inspect_max as i64) as usize;
        self.follow = false;
    }
}

/// Node box width in cells for a graph of `n` nodes.
pub fn box_width(n: usize) -> u16 {
    match n {
        0..=6 => 24,
        7..=10 => 20,
        _ => 16,
    }
}

/// Pick which agents get a node, each with its ancestors, in this order:
///
/// 1. the root;
/// 2. `pinned` (the agent the user selected), then every flagged agent in
///    spawn order. They do not wait for one of the `cap` picks, but when
///    `bound` is 3 or more they leave room for one more node, so a healthy
///    agent can still show;
/// 3. the rest of the `cap` picks (at least one): running agents that are
///    not idle, where a third of the picks (at least one, when there are two
///    or more) go to the most recently spawned and the others to the oldest
///    spawned; then the other agents, most recently active first.
///
/// Only picks of step 2 that made it into the set count against `cap`. No
/// chain is added that would take the set past `bound` nodes (see
/// [`node_bound`]), so every node can be drawn clear of the others; a pinned
/// or flagged agent left off that way counts as archived, and the archived
/// label counts its flag.
///
/// Running agents are ranked by spawn, not by their last event, so with more
/// of them than picks the set changes when one starts, finishes or goes
/// idle, never on a tool call. Splitting the picks between the oldest and
/// the newest keeps long-lived agents from holding every slot.
pub fn choose_visible(
    world: &World,
    cap: usize,
    bound: usize,
    pinned: Option<&str>,
) -> Vec<String> {
    let mut ids: Vec<String> = vec![ROOT.to_string()];
    // Add `id` and the ancestors not yet shown if they all fit within
    // `limit`; true when `id` is in the set afterwards.
    let add = |ids: &mut Vec<String>, id: &str, limit: usize| {
        let mut chain = Vec::new();
        let mut cur = Some(id.to_string());
        while let Some(id) = cur {
            if ids.contains(&id) || chain.contains(&id) {
                break;
            }
            cur = world.agents.get(&id).and_then(|x| x.parent.clone());
            chain.push(id);
        }
        if ids.len() + chain.len() > limit {
            return false;
        }
        ids.extend(chain);
        true
    };

    let mut flagged: Vec<_> = world
        .agents
        .values()
        .filter(|a| a.id != ROOT && world.health(a).is_flagged())
        .collect();
    flagged.sort_by_key(|a| a.order);
    let pinned = pinned.filter(|id| *id != ROOT && world.agents.contains_key(*id));
    let must: Vec<&str> = pinned
        .into_iter()
        .chain(
            flagged
                .iter()
                .map(|a| a.id.as_str())
                .filter(|id| Some(*id) != pinned),
        )
        .collect();
    // Leave one node for a healthy agent, unless that is all there is.
    let must_limit = if bound >= 3 { bound - 1 } else { bound };
    let mut shown = 0;
    for id in &must {
        shown += usize::from(add(&mut ids, id, must_limit));
    }

    let (mut running, mut others): (Vec<_>, Vec<_>) = world
        .agents
        .values()
        .filter(|a| a.id != ROOT && !must.contains(&a.id.as_str()))
        .partition(|a| a.status == Status::Running && !world.is_idle(a));
    running.sort_by_key(|a| a.order);
    others.sort_by_key(|a| (std::cmp::Reverse(a.last_ts), a.order));

    let picks = cap.saturating_sub(shown).max(1);
    let mut ranked: Vec<&str> = Vec::new();
    if running.len() > picks {
        let newest = if picks >= 2 { (picks / 3).max(1) } else { 0 };
        let oldest = picks - newest;
        ranked.extend(running[..oldest].iter().map(|a| a.id.as_str()));
        ranked.extend(
            running[running.len() - newest..]
                .iter()
                .map(|a| a.id.as_str()),
        );
    } else {
        ranked.extend(running.iter().map(|a| a.id.as_str()));
        ranked.extend(others.iter().map(|a| a.id.as_str()));
    }
    for id in ranked.into_iter().take(picks) {
        add(&mut ids, id, bound);
    }
    ids.sort_by_key(|id| world.agents.get(id).map(|a| a.order).unwrap_or(0));
    ids
}

/// Graph panel geometry in cells.
pub struct Space {
    /// Largest offset of a node centre from the panel centre, per axis.
    pub rx: f32,
    pub ry: f32,
    pub box_w: f32,
    /// Panel size in cells.
    pub view: (u16, u16),
}

pub const BOX_H: f32 = 3.0;

impl Space {
    pub fn new(view: (u16, u16), box_w: u16) -> Self {
        Self {
            rx: ((view.0 as f32 - box_w as f32) / 2.0 - 1.0).max(1.0),
            ry: ((view.1 as f32 - BOX_H) / 2.0 - 0.5).max(1.0),
            box_w: box_w as f32,
            view,
        }
    }

    /// Top-left cell of a box centred at `(x, y)` cells from the panel
    /// centre, clamped into the panel the way `ui` draws it.
    fn cell(&self, (x, y): (f32, f32)) -> (i32, i32) {
        let (w, h) = (self.view.0 as f32, self.view.1 as f32);
        let bw = self.box_w;
        let left = (w / 2.0 + x - bw / 2.0).round() as i32;
        let top = (h / 2.0 + y - BOX_H / 2.0).round() as i32;
        (
            left.clamp(0, (w - bw).max(0.0) as i32),
            top.clamp(0, (h - BOX_H).max(0.0) as i32),
        )
    }

    /// Whether boxes at these two cells share a cell.
    fn overlap(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        (a.0 - b.0).abs() < self.box_w as i32 && (a.1 - b.1).abs() < BOX_H as i32
    }

    /// Box centres, in cells from the panel centre, of a grid where no two
    /// boxes share a cell and the middle one is the root's: columns
    /// `box_w + 2` apart and rows `BOX_H + 1` apart, the outermost pulled in
    /// to the panel edge when there is room for the box but not the gap.
    pub fn slots(&self) -> Vec<(f32, f32)> {
        let axis = |r: f32, size: f32, step: f32| -> Vec<f32> {
            let side = if r >= size {
                ((r - size) / step) as usize + 1
            } else {
                0
            };
            let mut at = vec![0.0];
            for k in 1..=side {
                let d = (k as f32 * step).min(r);
                at.push(d);
                at.push(-d);
            }
            at
        };
        let xs = axis(self.rx, self.box_w, self.box_w + 2.0);
        let ys = axis(self.ry, BOX_H, BOX_H + 1.0);
        ys.iter()
            .flat_map(|&y| xs.iter().map(move |&x| (x, y)))
            .collect()
    }
}

/// Box widths a node can have, widest first.
const BOX_WIDTHS: [u16; 3] = [24, 20, 16];

/// Box width for `n` nodes in a graph panel of `view` cells: the usual width
/// for that count ([`box_width`]), narrowed while the panel has fewer
/// [`Space::slots`] than nodes. A panel too narrow for three columns of wide
/// boxes can still hold three columns of narrow ones.
pub fn fit_box_width(view: (u16, u16), n: usize) -> u16 {
    let narrowest = BOX_WIDTHS[BOX_WIDTHS.len() - 1];
    BOX_WIDTHS
        .into_iter()
        .filter(|&w| w <= box_width(n))
        .find(|&w| n <= Space::new(view, w).slots().len())
        .unwrap_or(narrowest)
}

/// Most nodes a graph panel of `view` cells can show with no two boxes
/// sharing a cell: the number of [`Space::slots`] at the narrowest box width
/// (`fit_box_width` narrows the boxes when a count needs it). At least 1,
/// the root.
pub fn node_bound(view: (u16, u16)) -> usize {
    let narrowest = BOX_WIDTHS[BOX_WIDTHS.len() - 1];
    Space::new(view, narrowest).slots().len().max(1)
}

/// Radial tree layout, then overlap removal. Returns a position per visible
/// id, each axis in -1..1 (scale by `Space::rx`/`ry` for cells).
pub fn layout(world: &World, visible: &[String], space: &Space) -> HashMap<String, (f32, f32)> {
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in visible.iter().filter(|id| *id != ROOT) {
        let parent = world
            .agents
            .get(id)
            .and_then(|a| a.parent.as_deref())
            .filter(|p| visible.iter().any(|v| v == p))
            .unwrap_or(ROOT);
        children.entry(parent).or_default().push(id);
    }

    fn leaves(id: &str, ch: &HashMap<&str, Vec<&str>>) -> usize {
        ch.get(id)
            .map(|c| c.iter().map(|k| leaves(k, ch)).sum())
            .unwrap_or(1)
    }
    fn depth(id: &str, ch: &HashMap<&str, Vec<&str>>) -> usize {
        ch.get(id)
            .map(|c| 1 + c.iter().map(|k| depth(k, ch)).max().unwrap_or(0))
            .unwrap_or(0)
    }
    fn place(
        id: &str,
        ch: &HashMap<&str, Vec<&str>>,
        (start, span): (f32, f32),
        level: usize,
        out: &mut HashMap<String, (f32, f32)>,
    ) {
        let Some(kids) = ch.get(id) else { return };
        let total: usize = kids.iter().map(|k| leaves(k, ch)).sum();
        let mut at = start;
        for kid in kids {
            let share = span * leaves(kid, ch) as f32 / total as f32;
            let angle = at + share / 2.0;
            // Leaves sit on the rim; a node with a subtree sits part-way out.
            let r = level as f32 / (level + depth(kid, ch)) as f32;
            out.insert(kid.to_string(), (angle.cos() * r, angle.sin() * r));
            place(kid, ch, (at, share), level + 1, out);
            at += share;
        }
    }

    let mut out = HashMap::new();
    out.insert(ROOT.to_string(), (0.0, 0.0));
    place(ROOT, &children, (-FRAC_PI_2, TAU), 1, &mut out);
    separate(visible, &mut out, space);
    settle(visible, &mut out, space);
    out
}

/// Move every box that still shares cells with an earlier one to the
/// nearest free grid slot, so up to [`node_bound`] nodes never hide each
/// other. `separate` alone can leave a box under another when the panel is
/// tight: it pushes along one axis into the clamp and stops. If a box finds
/// no free slot (boxes left between slots block them), every box but the
/// root is snapped to a slot instead, which always fits `node_bound` boxes.
fn settle(order: &[String], pos: &mut HashMap<String, (f32, f32)>, space: &Space) {
    let slots = space.slots();
    for snap_all in [false, true] {
        let mut out = pos.clone();
        let mut placed: Vec<(i32, i32)> = Vec::new();
        let mut stuck = false;
        for id in order {
            let Some(&(nx, ny)) = pos.get(id) else {
                continue;
            };
            let at = (nx * space.rx, ny * space.ry);
            let cell = space.cell(at);
            let clear = !placed.iter().any(|&p| space.overlap(p, cell));
            if id == ROOT || (clear && !snap_all) {
                placed.push(cell);
                continue;
            }
            let free = slots
                .iter()
                .filter(|&&s| {
                    let c = space.cell(s);
                    !placed.iter().any(|&p| space.overlap(p, c))
                })
                .min_by(|a, b| {
                    let d = |s: &(f32, f32)| (s.0 - at.0).powi(2) + ((s.1 - at.1) * 2.0).powi(2);
                    d(a).total_cmp(&d(b))
                });
            match free {
                Some(&s) => {
                    placed.push(space.cell(s));
                    out.insert(id.clone(), (s.0 / space.rx, s.1 / space.ry));
                }
                None => {
                    stuck = true;
                    placed.push(cell);
                }
            }
        }
        if !stuck || snap_all {
            *pos = out;
            return;
        }
    }
}

/// Push overlapping boxes apart, in cell space. The root stays centred.
fn separate(order: &[String], pos: &mut HashMap<String, (f32, f32)>, space: &Space) {
    let (min_dx, min_dy) = (space.box_w + 2.0, BOX_H + 1.0);
    let mut p: Vec<(f32, f32)> = order
        .iter()
        .map(|id| {
            pos.get(id)
                .map(|p| (p.0 * space.rx, p.1 * space.ry))
                .unwrap_or((0.0, 0.0))
        })
        .collect();
    let fixed: Vec<bool> = order.iter().map(|id| id == ROOT).collect();
    for _ in 0..48 {
        let mut moved = false;
        for i in 0..p.len() {
            for j in i + 1..p.len() {
                let (dx, dy) = (p[j].0 - p[i].0, p[j].1 - p[i].1);
                let (ox, oy) = (min_dx - dx.abs(), min_dy - dy.abs());
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }
                moved = true;
                // Resolve along the axis that needs the smaller relative move.
                let along_x = ox / min_dx < oy / min_dy;
                let sign = |d: f32| {
                    if d > 0.0 || (d == 0.0 && (i + j) % 2 == 0) {
                        1.0
                    } else {
                        -1.0
                    }
                };
                let (push, s) = if along_x {
                    (ox, sign(dx))
                } else {
                    (oy, sign(dy))
                };
                let (wi, wj) = match (fixed[i], fixed[j]) {
                    (true, _) => (0.0, 1.0),
                    (_, true) => (1.0, 0.0),
                    _ => (0.5, 0.5),
                };
                if along_x {
                    p[i].0 -= s * push * wi;
                    p[j].0 += s * push * wj;
                } else {
                    p[i].1 -= s * push * wi;
                    p[j].1 += s * push * wj;
                }
            }
        }
        for (q, is_fixed) in p.iter_mut().zip(&fixed) {
            if !is_fixed {
                q.0 = q.0.clamp(-space.rx, space.rx);
                q.1 = q.1.clamp(-space.ry, space.ry);
            }
        }
        if !moved {
            break;
        }
    }
    for (id, q) in order.iter().zip(p) {
        pos.insert(id.clone(), (q.0 / space.rx, q.1 / space.ry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoSource;

    fn spawn(world: &mut World, id: &str, parent: &str, ts: i64) {
        let kind = Kind::Spawn {
            id: id.into(),
            parent: parent.into(),
            agent_type: "t".into(),
            label: id.into(),
        };
        world.apply(&Event { ts, kind }, false);
    }

    #[test]
    fn layout_keeps_root_centred_and_boxes_apart() {
        for n in 1..=16 {
            let mut world = World::new("root");
            for i in 0..n {
                // Every fourth agent is nested under the previous one.
                let parent = if i % 4 == 3 {
                    format!("a{}", i - 1)
                } else {
                    ROOT.to_string()
                };
                spawn(&mut world, &format!("a{i}"), &parent, i as i64);
            }
            let visible = choose_visible(&world, 16, 64, None);
            let space = Space::new((150, 44), 16);
            let pos = layout(&world, &visible, &space);
            assert_eq!(pos[ROOT], (0.0, 0.0));
            let cells: Vec<(f32, f32)> = visible
                .iter()
                .map(|id| (pos[id].0 * space.rx, pos[id].1 * space.ry))
                .collect();
            for (i, a) in cells.iter().enumerate() {
                assert!(
                    a.0.abs() <= space.rx + 0.01 && a.1.abs() <= space.ry + 0.01,
                    "n={n}: out of bounds"
                );
                for b in &cells[i + 1..] {
                    let apart = (a.0 - b.0).abs() >= space.box_w || (a.1 - b.1).abs() >= BOX_H;
                    assert!(apart, "n={n}: boxes overlap at {a:?} and {b:?}");
                }
            }
        }
    }

    #[test]
    fn visible_set_respects_cap_and_keeps_ancestors() {
        let mut world = World::new("root");
        for i in 0..30 {
            spawn(&mut world, &format!("a{i}"), ROOT, i);
            let kind = Kind::Finish {
                id: format!("a{i}"),
                ok: true,
                tokens: None,
            };
            world.apply(&Event { ts: 50, kind }, false);
        }
        // The only running agent, under a finished one.
        spawn(&mut world, "child", "a0", 100);
        let visible = choose_visible(&world, 5, 64, None);
        assert!(visible.contains(&ROOT.to_string()));
        assert!(visible.contains(&"child".to_string()));
        assert!(
            visible.contains(&"a0".to_string()),
            "parent of a visible node must be visible"
        );
        assert!(visible.len() <= 5 + 2);
    }

    fn tool(world: &mut World, agent: &str, key: u64, ts: i64) {
        let kind = Kind::Tool {
            agent: agent.into(),
            tool: "Bash".into(),
            detail: "ls".into(),
            key,
        };
        world.apply(&Event { ts, kind }, false);
    }

    fn has(visible: &[String], id: &str) -> bool {
        visible.iter().any(|v| v == id)
    }

    /// 30 running agents, then `p` and its child `f`, then `late`; all
    /// spawned in that order, so by spawn order `f` would get no slot.
    fn crowded() -> World {
        let mut world = World::new("root");
        for i in 0..30 {
            spawn(&mut world, &format!("a{i}"), ROOT, i);
        }
        spawn(&mut world, "p", ROOT, 40);
        spawn(&mut world, "f", "p", 41);
        spawn(&mut world, "late", ROOT, 42);
        for i in 0..30 {
            tool(&mut world, &format!("a{i}"), i as u64, 100 + i);
        }
        world
    }

    #[test]
    fn flagged_agent_and_its_ancestors_stay_visible() {
        let mut world = crowded();
        assert!(!has(&choose_visible(&world, 5, 64, None), "f"));
        // Four identical calls in a row: a loop.
        for t in 0..4 {
            tool(&mut world, "f", 999, 200 + t);
        }
        // More recent activity everywhere else.
        for i in 0..30 {
            tool(&mut world, &format!("a{i}"), 1000 + i as u64, 300 + i);
        }
        assert!(world.health(&world.agents["f"]).is_flagged());
        let visible = choose_visible(&world, 5, 64, None);
        assert!(has(&visible, "f"), "flagged agent hidden: {visible:?}");
        assert!(has(&visible, "p"), "its parent must show too");
        assert!(has(&visible, ROOT));
        // The newest running agent keeps its reserved pick.
        assert!(has(&visible, "late"), "{visible:?}");
    }

    #[test]
    fn selected_agent_and_its_ancestors_stay_visible() {
        let world = crowded();
        let visible = choose_visible(&world, 5, 64, Some("f"));
        assert!(has(&visible, "f"), "selected agent hidden: {visible:?}");
        assert!(has(&visible, "p"));
        assert!(has(&visible, ROOT));
        // The pin takes one of the cap's picks.
        assert_eq!(visible.len(), 1 + 2 + 4);
        // The root and unknown ids pin nothing.
        assert_eq!(choose_visible(&world, 5, 64, Some(ROOT)).len(), 1 + 5);
        assert_eq!(choose_visible(&world, 5, 64, Some("nope")).len(), 1 + 5);
    }

    #[test]
    fn must_show_agents_past_the_bound_are_left_off() {
        let mut world = World::new("root");
        for i in 0..20 {
            spawn(&mut world, &format!("a{i}"), ROOT, i);
            for t in 0..4 {
                tool(&mut world, &format!("a{i}"), 7, 100 + i * 10 + t);
            }
        }
        spawn(&mut world, "healthy", ROOT, 500);
        assert_eq!(world.flagged(), 20);
        let visible = choose_visible(&world, 3, 7, Some("a19"));
        assert_eq!(visible.len(), 7, "{visible:?}");
        // The selection first, then flagged agents oldest first, and one node
        // left for a healthy running agent.
        for id in ["a19", "a0", "a1", "a2", "a3", "healthy"] {
            assert!(has(&visible, id), "{id} missing: {visible:?}");
        }
    }

    #[test]
    fn running_agents_past_the_cap_do_not_churn() {
        let mut world = World::new("root");
        for i in 0..40 {
            spawn(&mut world, &format!("a{i}"), ROOT, i);
        }
        let before = choose_visible(&world, 10, 64, None);
        assert_eq!(before.len(), 11);
        // The newest agents act most, which used to pull them in.
        for round in 0..3 {
            for i in (0..40).rev() {
                let ts = 1000 + round * 100 + (40 - i);
                tool(&mut world, &format!("a{i}"), (round * 100 + i) as u64, ts);
                assert_eq!(choose_visible(&world, 10, 64, None), before, "a{i}");
            }
        }
        // Seven picks for the oldest running agents, three for the newest.
        let want: Vec<String> = std::iter::once(ROOT.to_string())
            .chain([0, 1, 2, 3, 4, 5, 6, 37, 38, 39].map(|i| format!("a{i}")))
            .collect();
        assert_eq!(before, want);
        // A finish frees a slot for the next running agent by spawn order.
        let kind = Kind::Finish {
            id: "a0".into(),
            ok: true,
            tokens: None,
        };
        world.apply(&Event { ts: 2000, kind }, false);
        let after = choose_visible(&world, 10, 64, None);
        assert!(!has(&after, "a0") && has(&after, "a7"), "{after:?}");
    }

    #[test]
    fn long_lived_agents_do_not_hold_every_slot() {
        let mut world = World::new("root");
        // Ten agents that run all session, then 30 short ones in turn.
        for i in 0..10 {
            spawn(&mut world, &format!("long{i}"), ROOT, i);
        }
        let mut seen = std::collections::HashSet::new();
        for j in 0..30 {
            let id = format!("short{j}");
            spawn(&mut world, &id, ROOT, 100 + j * 10);
            for i in 0..10 {
                tool(
                    &mut world,
                    &format!("long{i}"),
                    j as u64 * 100 + i,
                    101 + j * 10,
                );
            }
            let visible = choose_visible(&world, 10, 64, None);
            assert!(has(&visible, &id), "short{j} never shows: {visible:?}");
            seen.extend(visible);
            let kind = Kind::Finish {
                id,
                ok: true,
                tokens: None,
            };
            world.apply(
                &Event {
                    ts: 105 + j * 10,
                    kind,
                },
                false,
            );
        }
        assert!(seen.len() >= 1 + 7 + 30, "{}", seen.len());
    }

    struct NoEvents;

    impl Source for NoEvents {
        fn poll(&mut self) -> Vec<Event> {
            Vec::new()
        }
    }

    /// Cell rectangles of the nodes not fading out, placed as `ui` draws
    /// them.
    fn boxes(app: &App) -> Vec<(String, i32, i32)> {
        let space = Space::new(app.view, app.box_w);
        let (w, h) = (app.view.0 as f32, app.view.1 as f32);
        let bw = app.box_w as i32;
        app.nodes
            .iter()
            .filter(|(_, n)| n.dying.is_none())
            .map(|(id, n)| {
                let x = ((w / 2.0 + n.x * space.rx - bw as f32 / 2.0).round() as i32)
                    .clamp(0, (w as i32 - bw).max(0));
                let y = ((h / 2.0 + n.y * space.ry - 1.5).round() as i32)
                    .clamp(0, (h as i32 - 3).max(0));
                (id.clone(), x, y)
            })
            .collect()
    }

    #[test]
    fn boxes_narrow_so_the_panel_is_used() {
        // 113x35 terminal with the side column: wide boxes fit one column,
        // narrow ones three. The bound must count the narrow ones.
        let view = (71, 21);
        assert_eq!(Space::new(view, 24).slots().len(), 5);
        assert!(node_bound(view) >= 9, "{}", node_bound(view));
        for view in [(30, 8), (58, 13), (71, 21), (78, 11), (98, 16), (160, 40)] {
            let bound = node_bound(view);
            for n in 1..=bound {
                let w = fit_box_width(view, n);
                assert!(w <= box_width(n), "{view:?} n={n}");
                assert!(n <= Space::new(view, w).slots().len(), "{view:?} n={n}");
            }
        }
    }

    #[test]
    fn must_show_boxes_never_overlap() {
        // Every fourth agent loops; graph panels of 80x24, 100x30 and
        // 140x40 terminals and a few others.
        for view in [(78, 11), (58, 13), (98, 16), (98, 24), (158, 38), (30, 8)] {
            let mut app = App::new(Box::new(NoEvents), Mode::Replay, "root", "t");
            for i in 0..60 {
                spawn(&mut app.world, &format!("a{i}"), ROOT, i);
                let calls = if i % 4 == 0 { 4 } else { 1 };
                for c in 0..calls {
                    let key = if calls > 1 { 7 } else { 100 + i as u64 };
                    tool(&mut app.world, &format!("a{i}"), key, 100 + i * 10 + c);
                }
            }
            app.follow = false;
            app.selected = app.world.agents["a59"].order;
            app.view = view;
            for _ in 0..120 {
                app.tick(1.0 / 30.0);
            }
            let shown = boxes(&app);
            assert!(shown.len() <= node_bound(view), "{view:?}");
            if node_bound(view) >= 2 {
                assert!(shown.iter().any(|b| b.0 == "a59"), "{view:?}: pin missing");
            }
            let bw = app.box_w as i32;
            for (i, a) in shown.iter().enumerate() {
                for b in &shown[i + 1..] {
                    let apart = (a.1 - b.1).abs() >= bw || (a.2 - b.2).abs() >= BOX_H as i32;
                    assert!(apart, "{view:?}: {} and {} overlap", a.0, b.0);
                }
            }
        }
    }

    #[test]
    fn replay_plays_history_then_restarts() {
        let mut app = App::new(Box::new(DemoSource::new(1)), Mode::Replay, "demo", "demo");
        for _ in 0..600 {
            app.tick(1.0 / 30.0);
        }
        assert!(
            app.world.order.len() > 3,
            "replay should have spawned agents"
        );
        assert!(!app.nodes.is_empty());
        app.restart();
        assert_eq!(app.world.order.len(), 1);
        assert!(app.nodes.is_empty());
    }

    #[test]
    fn paused_stream_survives_lines_dropping_off_the_log() {
        let mut app = App::new(Box::new(DemoSource::new(2)), Mode::Live, "demo", "demo");
        let text = |i: i64| Kind::Text {
            agent: ROOT.into(),
            text: format!("line {i}"),
        };
        for i in 0..100 {
            app.world.apply(
                &Event {
                    ts: i,
                    kind: text(i),
                },
                false,
            );
        }
        app.stream_top();
        let anchor = app.panes.anchor.expect("scrolled back");
        // Push enough lines that the anchored one is dropped.
        for i in 100..20_100 {
            app.world.apply(
                &Event {
                    ts: i,
                    kind: text(i),
                },
                false,
            );
        }
        assert!(anchor < app.log_no(0), "anchor line is gone");
        let list = app.stream_list();
        assert_eq!(app.stream_bottom(&list), Some(0), "shows the oldest kept");
        assert!(app.new_lines() >= 20_000);
        app.scroll_stream(-1);
        assert_eq!(
            app.stream_bottom(&app.stream_list()),
            Some(app.panes.stream_rows - 1),
            "clamped to a full panel"
        );
        app.scroll_stream(i64::MAX);
        assert_eq!(app.panes.anchor, None);
    }

    #[test]
    fn panel_sizes_clamp_to_the_terminal() {
        assert_eq!(stream_height(Some(STREAM_H), 40), STREAM_H);
        assert_eq!(stream_height(Some(100), 40), 40 - 2 - BODY_MIN_H);
        assert_eq!(stream_height(Some(0), 40), STREAM_MIN_H);
        // No preference: the height follows the terminal.
        assert_eq!(stream_height(None, 24), STREAM_H);
        assert_eq!(stream_height(None, 40), 13);
        assert_eq!(stream_height(None, 60), 20);
        assert_eq!(stream_height(None, 16), 16 - 2 - BODY_MIN_H);
        // The preference outlives a small terminal.
        assert_eq!(
            stream_height(Some(20), 16),
            STREAM_MIN_H.max(16 - 2 - BODY_MIN_H)
        );
        assert_eq!(stream_height(Some(20), 60), 20);
        for h in 0..80 {
            assert!(stream_height(Some(9), h) <= h.saturating_sub(2), "h={h}");
            assert!(stream_height(None, h) <= h.saturating_sub(2), "h={h}");
        }
        assert_eq!(side_width(SIDE_W, 140), SIDE_W);
        assert_eq!(side_width(500, 140), 140 - GRAPH_MIN_W);
        assert_eq!(side_width(0, 140), SIDE_MIN_W);
        assert_eq!(side_width(SIDE_W, SIDE_MIN_TERM_W), SIDE_W);
    }

    /// What stepping and playing must agree on: position, agents, stream
    /// lines, tokens and tool calls.
    fn state(app: &App) -> (usize, usize, u64, u64, u64) {
        let w = &app.world;
        (
            app.cursor,
            w.order.len(),
            w.log_seq,
            w.total_tokens(),
            w.total_tools,
        )
    }

    /// A paused demo replay with `secs` seconds of script loaded and nothing
    /// applied yet.
    fn loaded(seed: u32, secs: usize) -> App {
        let mut app = App::new(
            Box::new(DemoSource::new(seed)),
            Mode::Replay,
            "demo",
            "demo",
        );
        app.paused = true;
        for _ in 0..secs * 30 {
            app.tick(1.0 / 30.0);
        }
        assert_eq!(app.cursor, 0);
        assert!(app.history.len() > 30);
        app
    }

    #[test]
    fn stepping_forward_lands_where_playing_does() {
        let mut stepped = loaded(5, 20);
        let mut at = HashMap::new();
        while stepped.step_forward() {
            stepped.tick(0.0);
            assert!(stepped.paused);
            at.insert(stepped.cursor, state(&stepped));
        }
        assert!(stepped.at_end());
        assert!(at.len() > 50, "{} steps", at.len());
        // The same script, played at the default speed.
        let mut played = App::new(Box::new(DemoSource::new(5)), Mode::Replay, "demo", "demo");
        let mut seen = 0;
        for _ in 0..20 * 30 {
            played.tick(1.0 / 30.0);
            if let Some(s) = at.get(&played.cursor) {
                assert_eq!(state(&played), *s, "at event {}", played.cursor);
                seen += 1;
            }
        }
        assert!(seen > 20, "compared {seen} times");
    }

    #[test]
    fn stepping_back_then_forward_returns_to_the_same_state() {
        let mut app = loaded(6, 20);
        for _ in 0..40 {
            assert!(app.step_forward());
        }
        app.tick(0.0);
        let mut trail = vec![state(&app)];
        for _ in 0..15 {
            assert!(app.step_back());
            app.tick(0.0);
            let s = state(&app);
            assert!(s.0 < trail.last().unwrap().0, "moved back");
            trail.push(s);
            // The same as a world built straight from the events.
            let mut w = World::new("demo");
            for ev in &app.history[..app.cursor] {
                w.apply(ev, false);
            }
            let fresh = (w.order.len(), w.log_seq, w.total_tokens(), w.total_tools);
            assert_eq!((s.1, s.2, s.3, s.4), fresh);
        }
        trail.pop();
        while let Some(want) = trail.pop() {
            assert!(app.step_forward());
            app.tick(0.0);
            assert_eq!(state(&app), want);
        }
        // Back to the start, and no further.
        while app.step_back() {}
        assert_eq!(state(&app), (0, 1, 0, 0, 0));
        assert!(app.paused);
    }

    #[test]
    fn steps_pause_a_replay_and_live_sessions_do_not_step() {
        let mut replay = App::new(Box::new(DemoSource::new(2)), Mode::Replay, "d", "d");
        for _ in 0..300 {
            replay.tick(1.0 / 30.0);
        }
        assert!(!replay.paused);
        let at = replay.cursor;
        assert!(replay.step_back());
        assert!(replay.paused && replay.cursor < at);

        let mut live = App::new(Box::new(DemoSource::new(2)), Mode::Live, "d", "d");
        for _ in 0..300 {
            live.tick(1.0 / 30.0);
        }
        let before = state(&live);
        assert!(!live.step_forward());
        assert!(!live.step_back());
        assert!(live.mode == Mode::Live);
        assert_eq!(state(&live), before);
        // A scripted live view (the demo) pauses into a replay instead.
        live.pausable = true;
        live.toggle_pause();
        assert!(live.mode == Mode::Replay && live.paused);
        assert_eq!(state(&live), before);
        assert!(live.step_back());
        assert!(live.cursor < before.0);
        live.jump_to_end();
        assert!(live.mode == Mode::Live);
    }

    /// A source that hands over a fixed list of events on the first poll.
    struct Fixed(Vec<Event>);

    impl Source for Fixed {
        fn poll(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.0)
        }
    }

    /// A session with two stalls: `quiet` goes silent after text while
    /// `busy` keeps calling tools every 5 s (the stall limit passes between
    /// two events), then `busy` waits on a call through a 40 minute gap
    /// that playback skips.
    fn stalling() -> Vec<Event> {
        let t0 = 1_700_000_000_000;
        let spawn = |id: &str| Kind::Spawn {
            id: id.into(),
            parent: ROOT.into(),
            agent_type: id.into(),
            label: "work".into(),
        };
        let call = |n: i64| Kind::Tool {
            agent: "busy".into(),
            tool: "Read".into(),
            detail: format!("f{n}"),
            key: n as u64,
        };
        let mut evs = vec![
            Event {
                ts: t0,
                kind: spawn("quiet"),
            },
            Event {
                ts: t0,
                kind: spawn("busy"),
            },
            Event {
                ts: t0 + 1_000,
                kind: Kind::Text {
                    agent: "quiet".into(),
                    text: "thinking".into(),
                },
            },
        ];
        for n in 1..=150 {
            evs.push(Event {
                ts: t0 + n * 5_000,
                kind: call(n),
            });
        }
        let late = t0 + 150 * 5_000 + 40 * 60_000;
        evs.push(Event {
            ts: late,
            kind: call(1_000),
        });
        evs.push(Event {
            ts: late + 1_000,
            kind: Kind::Text {
                agent: ROOT.into(),
                text: "done".into(),
            },
        });
        evs
    }

    fn lines(app: &App) -> Vec<(i64, String, String)> {
        app.world
            .log
            .iter()
            .map(|l| (l.ts, l.agent.clone(), l.text.clone()))
            .collect()
    }

    #[test]
    fn rebuilt_log_keeps_the_stall_lines_playback_wrote() {
        let mut played = App::new(Box::new(Fixed(stalling())), Mode::Replay, "s", "s");
        // Played logs at every step stop, captured as playback reaches it.
        let mut at: HashMap<usize, Vec<(i64, String, String)>> = HashMap::new();
        while !played.at_end() {
            played.tick(1.0 / 30.0);
            at.entry(played.cursor).or_insert_with(|| lines(&played));
        }
        for _ in 0..30 {
            played.tick(1.0 / 30.0);
        }
        let stalls = played
            .world
            .log
            .iter()
            .filter(|l| l.kind == crate::state::LogKind::Health)
            .count();
        assert_eq!(stalls, 2, "both agents stalled while playing");

        let mut rebuilt = App::new(Box::new(Fixed(stalling())), Mode::Replay, "s", "s");
        rebuilt.paused = true;
        rebuilt.tick(0.0);
        rebuilt.jump_to_end();
        rebuilt.mode = Mode::Replay;
        let points = rebuilt.step_points(rebuilt.history.len());
        let mut compared = 0;
        for &p in &points {
            let Some(want) = at.get(&p) else { continue };
            rebuilt.rebuild(p);
            rebuilt.tick(0.0);
            let got = lines(&rebuilt);
            assert_eq!(got.len(), want.len(), "lines at event {p}");
            for (g, w) in got.iter().zip(want) {
                assert_eq!((&g.1, &g.2), (&w.1, &w.2), "at event {p}");
                // Playback notices a stall within one frame (8x: 267 ms).
                assert!((g.0 - w.0).abs() <= 300, "at event {p}: {g:?} {w:?}");
            }
            compared += 1;
        }
        assert!(compared > 20, "compared {compared} stops");
        // Stepping back from the end keeps both stall lines too.
        rebuilt.rebuild(rebuilt.history.len());
        assert!(rebuilt.step_back());
        let stalls = rebuilt
            .world
            .log
            .iter()
            .filter(|l| l.kind == crate::state::LogKind::Health)
            .count();
        assert_eq!(stalls, 2);
    }

    #[test]
    fn step_back_keeps_the_stream_scroll_and_match() {
        let mut app = loaded(9, 30);
        for _ in 0..60 {
            app.step_forward();
        }
        app.scroll_stream(-5);
        let anchor = app.panes.anchor.expect("scrolled back");
        app.panes.current = Some(anchor);
        app.step_back();
        assert_eq!(app.panes.anchor, Some(anchor), "line still there");
        assert_eq!(app.panes.current, Some(anchor));
        // Back past the anchored line: follow the tail again.
        while app.world.log_seq > anchor {
            app.step_back();
        }
        assert_eq!(app.panes.anchor, None);
        assert_eq!(app.panes.current, None);
    }

    #[test]
    fn caption_event_is_the_latest_one_the_stream_shows() {
        let mut app = loaded(8, 10);
        assert!(app.caption_event().is_none());
        app.step_forward();
        let (ev, _) = app.caption_event().expect("one event shown");
        assert!(!matches!(ev.kind, Kind::Usage { .. }));
        assert_eq!(ev.ts, app.history[app.cursor - 1].ts);
    }

    #[test]
    fn live_mode_applies_everything_at_once() {
        let mut app = App::new(Box::new(DemoSource::new(2)), Mode::Live, "demo", "demo");
        for _ in 0..300 {
            app.tick(1.0 / 30.0);
        }
        assert!(app.at_end());
        assert!(app.world.total_tools > 0);
    }
}
