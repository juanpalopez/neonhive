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
    pub hidden: usize,
    pub help: bool,
    /// The roster panel shows cost by agent type instead of agents.
    pub cost_view: bool,
    /// Focus, panel sizes, scroll positions and the stream search.
    pub panes: Panes,
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
            help: false,
            cost_view: false,
            panes: Panes::default(),
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
        while self.cursor < self.history.len() {
            self.world.apply(&self.history[self.cursor], false);
            self.cursor += 1;
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

    fn step(&mut self) {
        let ev = self.history[self.cursor].clone();
        self.cursor += 1;
        self.world.apply(&ev, true);
        self.act_acc += 1;
        if self.follow {
            let id = match &ev.kind {
                Kind::Spawn { id, .. } => id,
                Kind::Tool { agent, .. } | Kind::Text { agent, .. } => agent,
                _ => return,
            };
            if id != ROOT {
                if let Some(a) = self.world.agents.get(id) {
                    self.selected = a.order;
                }
            }
        }
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
        let visible = choose_visible(&self.world, cap);
        self.hidden = self.world.order.len() - visible.len();
        self.box_w = match visible.len() {
            0..=6 => 24,
            7..=10 => 20,
            _ => 16,
        };
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

/// Default event-stream height in rows, borders included.
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

/// Stream height used in a terminal `term_h` rows tall: the preference,
/// clamped so header, footer and `BODY_MIN_H` rows of graph still fit.
pub fn stream_height(pref: u16, term_h: u16) -> u16 {
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
    /// that shrinks and grows back gets the same sizes again.
    pub stream_h: u16,
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
            stream_h: STREAM_H,
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
        p.stream_h = stream_height((now + delta).max(0) as u16, p.term.1);
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

/// Pick which agents get a node: the root, then the most relevant subagents
/// up to `cap` (active first, then most recently active), plus ancestors.
pub fn choose_visible(world: &World, cap: usize) -> Vec<String> {
    let mut subs: Vec<_> = world.agents.values().filter(|a| a.id != ROOT).collect();
    subs.sort_by_key(|a| {
        let active = a.status == Status::Running && !world.is_idle(a);
        (!active, std::cmp::Reverse(a.last_ts), a.order)
    });
    let mut ids: Vec<String> = vec![ROOT.to_string()];
    for a in subs.into_iter().take(cap) {
        let mut cur = Some(a.id.clone());
        while let Some(id) = cur {
            if ids.contains(&id) {
                break;
            }
            cur = world.agents.get(&id).and_then(|x| x.parent.clone());
            ids.push(id);
        }
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
}

pub const BOX_H: f32 = 3.0;

impl Space {
    pub fn new(view: (u16, u16), box_w: u16) -> Self {
        Self {
            rx: ((view.0 as f32 - box_w as f32) / 2.0 - 1.0).max(1.0),
            ry: ((view.1 as f32 - BOX_H) / 2.0 - 0.5).max(1.0),
            box_w: box_w as f32,
        }
    }
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
    out
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
            let visible = choose_visible(&world, 16);
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
        }
        spawn(&mut world, "child", "a0", 100);
        let visible = choose_visible(&world, 5);
        assert!(visible.contains(&ROOT.to_string()));
        assert!(visible.contains(&"child".to_string()));
        assert!(
            visible.contains(&"a0".to_string()),
            "parent of a visible node must be visible"
        );
        assert!(visible.len() <= 5 + 2);
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
        assert_eq!(stream_height(STREAM_H, 40), STREAM_H);
        assert_eq!(stream_height(100, 40), 40 - 2 - BODY_MIN_H);
        assert_eq!(stream_height(0, 40), STREAM_MIN_H);
        // The preference outlives a small terminal.
        assert_eq!(stream_height(20, 16), STREAM_MIN_H.max(16 - 2 - BODY_MIN_H));
        assert_eq!(stream_height(20, 60), 20);
        for h in 0..80 {
            assert!(stream_height(9, h) <= h.saturating_sub(2), "h={h}");
        }
        assert_eq!(side_width(SIDE_W, 140), SIDE_W);
        assert_eq!(side_width(500, 140), 140 - GRAPH_MIN_W);
        assert_eq!(side_width(0, 140), SIDE_MIN_W);
        assert_eq!(side_width(SIDE_W, SIDE_MIN_TERM_W), SIDE_W);
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
