//! Rendering. Everything draws straight into the ratatui buffer.

use crate::app::{
    log_who, side_width, stream_height, App, Mode, Panel, Space, BODY_MIN_H, BORN_SECS, DYING_SECS,
    SIDE_MIN_TERM_W, STREAM_MIN_H,
};
use crate::caption::{self, Tone};
use crate::event::ROOT;
use crate::fx::{self, Rgb};
use crate::state::{Agent, LogKind, PacketKind, Status, World};
use crate::util::{
    clean, find_ci, fmt_clock, fmt_dur, fmt_tokens, fmt_usd, hash, needle, tail_fit,
};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const GLITCH: [char; 12] = ['▓', '▒', '░', '#', '%', '/', '\\', '0', '1', '$', '≡', '¦'];
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// Marks an agent flagged by `crate::health`. Single-width on every terminal,
/// unlike the warning sign emoji.
const HEALTH_GLYPH: &str = "▲";

fn fg(c: Rgb) -> Style {
    Style::default().fg(c.c())
}

fn bold(c: Rgb) -> Style {
    fg(c).add_modifier(Modifier::BOLD)
}

/// Product name as shown in the UI, taken from the crate name.
pub fn brand() -> String {
    format!(" ▓▒░ {} ░▒▓ ", env!("CARGO_PKG_NAME").to_uppercase())
}

pub fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(fg(fx::DIM))
        .title(Span::styled(title, bold(fx::CYAN)))
}

/// A panel frame that shows whether the panel has keyboard focus: a brighter
/// border and a `▸` before the title.
fn pane(title: &'static str, focused: bool) -> Block<'static> {
    if focused {
        Block::bordered()
            .border_type(BorderType::Plain)
            .border_style(fg(fx::CYAN.scale(0.75)))
            .title(Span::styled(format!("▸{title}"), bold(fx::CYAN)))
    } else {
        panel(title)
    }
}

/// `pane` for panel `p`, `area_w` cells wide, with status parts on the
/// right of the top border. Each part has a priority; parts that do not fit
/// beside the title are dropped, lowest priority first. A maximised panel
/// adds a restore hint at the lowest priority.
fn pane_for(
    title: &'static str,
    app: &App,
    p: Panel,
    area_w: u16,
    mut right: Vec<(u8, Span<'static>)>,
) -> Block<'static> {
    let focused = app.panes.focus == p;
    if focused && app.panes.maximised {
        right.insert(0, (0, Span::styled(" z restore ", fg(fx::DIM))));
    }
    let left = title.chars().count() + usize::from(focused);
    let mut room = (area_w as usize).saturating_sub(left + 4);
    let mut by_rank: Vec<usize> = (0..right.len()).collect();
    by_rank.sort_by_key(|&i| std::cmp::Reverse(right[i].0));
    let mut keep = vec![false; right.len()];
    for i in by_rank {
        let w = right[i].1.width();
        if w <= room {
            room -= w;
            keep[i] = true;
        }
    }
    let spans: Vec<Span> = right
        .into_iter()
        .zip(keep)
        .filter_map(|((_, s), k)| k.then_some(s))
        .collect();
    let block = pane(title, focused);
    if spans.is_empty() {
        block
    } else {
        block.title(Line::from(spans).right_aligned())
    }
}

pub fn agent_color(a: &Agent) -> Rgb {
    if a.id == ROOT {
        fx::CYAN
    } else {
        fx::type_color(&a.agent_type)
    }
}

fn status_style(world: &World, a: &Agent) -> (char, &'static str, Rgb) {
    match a.status {
        Status::Running if world.is_idle(a) => ('◌', "IDLE", fx::DIM),
        Status::Running => ('●', "RUNNING", fx::GREEN),
        Status::Done => ('✔', "DONE", fx::DIM),
        Status::Failed => ('✖', "FAILED", fx::RED),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let buf = f.buffer_mut();
    buf.set_style(area, Style::default().bg(fx::BG.c()).fg(fx::TEXT.c()));
    if area.width < 60 || area.height < 16 {
        buf.set_stringn(
            area.x,
            area.y,
            "terminal too small (need 60x16)",
            area.width as usize,
            fg(fx::RED),
        );
        return;
    }
    app.panes.term = (area.width, area.height);
    app.panes.side_shown = area.width >= SIDE_MIN_TERM_W;
    let mut log_h = stream_height(app.panes.stream_h, area.height);
    // Captions sit between the stream and the footer. Their two rows come
    // from the graph while it keeps BODY_MIN_H rows, then from the stream
    // while it keeps STREAM_MIN_H.
    let cap_h = if app.captions {
        let spare = area.height.saturating_sub(2 + log_h + BODY_MIN_H);
        let take = 2.min(spare + log_h.saturating_sub(STREAM_MIN_H));
        log_h -= take.saturating_sub(spare);
        take
    } else {
        0
    };
    let [header, body, log, cap, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(log_h),
        Constraint::Length(cap_h),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(buf, header, app);
    if app.panes.maximised {
        let full = body.union(log);
        match app.panes.focus {
            Panel::Graph => draw_graph(buf, full, app),
            Panel::Inspector => draw_inspector(buf, full, app),
            Panel::Roster => draw_roster(buf, full, app),
            Panel::Stream => draw_log(buf, full, app),
        }
    } else {
        if app.panes.side_shown {
            let side_w = side_width(app.panes.side_w, area.width);
            let [graph, side] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(side_w)]).areas(body);
            let [inspect, roster] =
                Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .areas(side);
            draw_graph(buf, graph, app);
            draw_inspector(buf, inspect, app);
            draw_roster(buf, roster, app);
        } else {
            draw_graph(buf, body, app);
        }
        draw_log(buf, log, app);
    }
    draw_caption(buf, cap, app);
    draw_footer(buf, footer, app);
    if app.help {
        draw_help(buf, area);
    }
}

/// The caption for the most recent event: what happened, then what it means.
fn draw_caption(buf: &mut Buffer, area: Rect, app: &App) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let found = app
        .caption_event()
        .and_then(|(ev, others)| Some((caption::describe(&app.world, app.applied(), ev)?, others)));
    let Some((c, others)) = found else {
        let hint =
            " ◆ captions on: play, or pause and step with . and , to see each event explained";
        buf.set_stringn(area.x, area.y, hint, area.width as usize, fg(fx::DIM));
        return;
    };
    let color = match c.tone {
        Tone::Spawn => fx::MAGENTA,
        Tone::Result => fx::GREEN,
        Tone::Fail => fx::RED,
        Tone::Message => fx::YELLOW,
        Tone::Tool => fx::CYAN,
        Tone::Text => fx::TEXT,
    };
    let mut spans = vec![Span::styled(" ◆ ", bold(color))];
    for p in &c.what {
        let style = if p.name {
            bold(fx::CYAN)
        } else {
            fg(fx::WHITE)
        };
        spans.push(Span::styled(p.text.clone(), style));
    }
    let width = area.width as usize;
    let line = Line::from(spans);
    let first = wrap_line(&line, width)
        .into_iter()
        .next()
        .unwrap_or_default();
    first.render(Rect::new(area.x, area.y, area.width, 1), buf);
    if area.height > 1 {
        let more = match others {
            0 => String::new(),
            1 => "+1 more event at this moment · ".to_string(),
            n => format!("+{n} more events at this moment · "),
        };
        let text = format!("   {more}{}", c.hint);
        buf.set_stringn(
            area.x,
            area.y + 1,
            head_fit(&text, width),
            width,
            fg(fx::DIM),
        );
    }
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App) {
    let p = &app.panes;
    let width = area.width as usize;
    if p.input {
        // `/query█`, keeping the end of a long query in view.
        let hint = "   enter search · esc cancel";
        let room = width
            .saturating_sub(2 + hint.chars().count())
            .max(width / 2);
        let line = Line::from(vec![
            Span::styled("/", bold(fx::CYAN)),
            Span::styled(tail_fit(&p.query, room).to_string(), bold(fx::WHITE)),
            Span::styled("█", fg(fx::CYAN)),
            Span::styled(hint, fg(fx::DIM)),
        ]);
        line.render(area, buf);
        return;
    }
    // The step keys do nothing in a live session, so they are not offered.
    let step = if app.can_step() { ". , step  " } else { "" };
    let keys = format!(" ? help  tab focus  z full  ↑↓ select/scroll  / search  [ ] < > resize  f follow  c cost  space pause  {step}e captions  +/- speed  r replay  l live  esc back  q quit");
    let mut spans = Vec::new();
    if !p.query.is_empty() {
        let grep = if p.filter { "& all lines" } else { "& grep" };
        spans.extend([
            Span::styled(" /", bold(fx::CYAN)),
            Span::styled(head_fit(&p.query, 24), bold(fx::WHITE)),
            Span::styled(
                format!("  n/N older/newer  {grep}  esc clear │"),
                fg(fx::DIM),
            ),
        ]);
    }
    spans.push(Span::styled(keys, fg(fx::DIM)));
    Line::from(spans).render(area, buf);
}

fn draw_header(buf: &mut Buffer, area: Rect, app: &App) {
    let w = &app.world;
    let blink = ((app.clock * 2.0) as u64).is_multiple_of(2);
    let mode = match app.mode {
        Mode::Live => Span::styled(if blink { " ● LIVE " } else { " ○ LIVE " }, bold(fx::GREEN)),
        Mode::Replay if app.at_end() => Span::styled(" ■ REPLAY END ", bold(fx::YELLOW)),
        Mode::Replay if app.paused => Span::styled(" ⏸ PAUSED ", bold(fx::YELLOW)),
        Mode::Replay => Span::styled(format!(" ▶ REPLAY {}x ", app.speed()), bold(fx::YELLOW)),
    };
    let max = app.activity.iter().copied().max().unwrap_or(0).max(1);
    let spark: String = app
        .activity
        .iter()
        .map(|v| BARS[(*v * 7 / max) as usize])
        .collect();
    let stat = |k: &'static str, v: String| {
        vec![
            Span::styled(format!("  {k} "), fg(fx::DIM)),
            Span::styled(v, bold(fx::TEXT)),
        ]
    };
    let mut spans = vec![
        Span::styled(brand(), bold(fx::MAGENTA)),
        Span::styled("// ", fg(fx::DIM)),
        Span::styled(clean(&app.title, 48), fg(fx::CYAN)),
        Span::raw(" "),
        mode,
    ];
    spans.extend(stat(
        "AGENTS",
        format!("{}/{}", w.running(), w.order.len() - 1),
    ));
    spans.extend(stat("TOOLS", w.total_tools.to_string()));
    spans.extend(stat("TOK", fmt_tokens(w.total_tokens())));
    spans.extend(stat("COST", session_cost(w)));
    spans.extend(stat("MSG", w.total_msgs.to_string()));
    let flagged = w.flagged();
    if flagged > 0 {
        spans.push(Span::styled(
            format!("  {HEALTH_GLYPH} {flagged} FLAGGED"),
            bold(fx::ORANGE),
        ));
    }
    spans.push(Span::styled(
        format!("  {} ", fmt_clock(w.now_ts)),
        fg(fx::DIM),
    ));
    spans.push(Span::styled(spark, fg(fx::CYAN)));
    Line::from(spans).render(area, buf);
}

// -------------------------------------------------------------------- graph

/// Sub-cell drawing surface: 2x4 dots per terminal cell.
struct Braille {
    w: usize,
    h: usize,
    bits: Vec<u8>,
    color: Vec<Rgb>,
    prio: Vec<u8>,
}

impl Braille {
    fn new(w: u16, h: u16) -> Self {
        let n = w as usize * h as usize;
        Self {
            w: w as usize,
            h: h as usize,
            bits: vec![0; n],
            color: vec![fx::DIM; n],
            prio: vec![0; n],
        }
    }

    fn plot(&mut self, x: f32, y: f32, color: Rgb, prio: u8) {
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (dx, dy) = (x as usize, y as usize);
        let (cx, cy) = (dx / 2, dy / 4);
        if cx >= self.w || cy >= self.h {
            return;
        }
        const MASK: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let i = cy * self.w + cx;
        self.bits[i] |= MASK[dx % 2][dy % 4];
        if prio >= self.prio[i] {
            self.prio[i] = prio;
            self.color[i] = color;
        }
    }

    /// Line from a to b; `keep(i)` decides whether dot `i` is drawn (dashes).
    fn line(
        &mut self,
        a: (f32, f32),
        b: (f32, f32),
        color: Rgb,
        prio: u8,
        keep: impl Fn(i32) -> bool,
    ) {
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0);
        for i in 0..=steps as i32 {
            if keep(i) {
                let t = i as f32 / steps;
                self.plot(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, color, prio);
            }
        }
    }

    fn render(&self, buf: &mut Buffer, area: Rect) {
        for cy in 0..self.h {
            for cx in 0..self.w {
                let i = cy * self.w + cx;
                if self.bits[i] == 0 {
                    continue;
                }
                if let Some(cell) = buf.cell_mut((area.x + cx as u16, area.y + cy as u16)) {
                    cell.set_char(char::from_u32(0x2800 + self.bits[i] as u32).unwrap_or(' '));
                    cell.set_fg(self.color[i].c());
                }
            }
        }
    }
}

fn packet_color(kind: PacketKind) -> Rgb {
    match kind {
        PacketKind::Spawn => fx::MAGENTA,
        PacketKind::Result => fx::GREEN,
        PacketKind::Fail => fx::RED,
        PacketKind::Message => fx::YELLOW,
    }
}

fn draw_graph(buf: &mut Buffer, area: Rect, app: &mut App) {
    let block = pane_for(
        " NEURAL TOPOLOGY ",
        app,
        Panel::Graph,
        area.width,
        Vec::new(),
    );
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.width < 30 || inner.height < 8 {
        return;
    }
    app.view = (inner.width, inner.height);
    let bw = app.box_w;
    let space = Space::new(app.view, bw);
    let clock = app.clock;
    let frame = (clock * 30.0) as u32;

    // Backdrop: dot grid with a slow twinkle.
    for y in (inner.y + 1..inner.bottom()).step_by(3) {
        for x in (inner.x + 2..inner.right()).step_by(6) {
            let lit = hash(x as u32, y as u32, frame / 20).is_multiple_of(23);
            let color = if lit { fx::DIM } else { fx::GRID };
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char('·').set_fg(color.c());
            }
        }
    }

    // Node centres in cell coordinates.
    let (rx, ry) = (space.rx, space.ry);
    let cx = inner.x as f32 + inner.width as f32 / 2.0;
    let cy = inner.y as f32 + inner.height as f32 / 2.0;
    let pos: HashMap<&str, (f32, f32)> = app
        .nodes
        .iter()
        .map(|(id, n)| (id.as_str(), (cx + n.x * rx, cy + n.y * ry)))
        .collect();
    let dot = |p: (f32, f32)| ((p.0 - inner.x as f32) * 2.0, (p.1 - inner.y as f32) * 4.0);

    // Edges: dashes flow from parent to child while the child is working.
    let mut br = Braille::new(inner.width, inner.height);
    let phase = (clock * 24.0) as i32;
    for (id, nv) in &app.nodes {
        let Some(a) = app.world.agents.get(id) else {
            continue;
        };
        let Some(parent) = a.parent.as_deref() else {
            continue;
        };
        let from = pos.get(parent).or(pos.get(ROOT));
        let (Some(from), Some(to)) = (from, pos.get(id.as_str())) else {
            continue;
        };
        let active = a.status == Status::Running && !app.world.is_idle(a) && nv.dying.is_none();
        if active {
            let color = agent_color(a).scale(0.6);
            br.line(dot(*from), dot(*to), color, 1, |i| {
                (i - phase).rem_euclid(8) < 5
            });
        } else {
            br.line(dot(*from), dot(*to), fx::DIM.scale(0.8), 0, |i| i % 3 == 0);
        }
    }

    // Packets: a bright head with a fading trail.
    let mut heads = Vec::new();
    for p in &app.packets {
        let (Some(from), Some(to)) = (pos.get(p.from.as_str()), pos.get(p.to.as_str())) else {
            continue;
        };
        let color = packet_color(p.kind);
        let at = |t: f32| {
            let e = t * t * (3.0 - 2.0 * t); // smoothstep
            (from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e)
        };
        for k in 1..14 {
            let t = p.t - k as f32 * 0.012;
            if t > 0.0 {
                let d = dot(at(t));
                br.plot(d.0, d.1, color.scale(1.0 - k as f32 / 16.0), 2);
            }
        }
        heads.push((at(p.t), color));
    }
    br.render(buf, inner);
    for ((x, y), color) in heads {
        let (x, y) = (x as u16, y as u16);
        if inner.contains((x, y).into()) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char('◆')
                    .set_style(bold(color.mix(fx::WHITE, 0.4)));
            }
        }
    }

    // Nodes: finished first, active on top, selection last.
    let selected_id = app
        .world
        .order
        .get(app.selected)
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<&String> = app.nodes.keys().collect();
    ids.sort_by_key(|id| {
        let a = app.world.agents.get(*id);
        let running = a.is_some_and(|a| a.status == Status::Running);
        (
            **id == selected_id,
            running,
            a.map(|a| a.order).unwrap_or(0),
        )
    });
    for id in ids {
        let (Some(a), Some(nv), Some(p)) = (
            app.world.agents.get(id),
            app.nodes.get(id),
            pos.get(id.as_str()),
        ) else {
            continue;
        };
        let x = ((p.0 - bw as f32 / 2.0).round() as i32)
            .clamp(inner.x as i32, (inner.right() - bw) as i32) as u16;
        let y =
            ((p.1 - 1.5).round() as i32).clamp(inner.y as i32, (inner.bottom() - 3) as i32) as u16;
        let born = ((clock - nv.born) / BORN_SECS).min(1.0) as f32;
        let dying = nv
            .dying
            .map(|d| ((clock - d) / DYING_SECS) as f32)
            .unwrap_or(0.0);
        let flash = (1.0 - (clock - nv.flash) / 0.3).clamp(0.0, 1.0) as f32;
        draw_node(
            buf,
            x,
            y,
            bw as usize,
            a,
            &app.world,
            *id == selected_id,
            clock,
            born,
            dying,
            flash,
        );
    }

    // Scanline sweep.
    if fx::truecolor() {
        let y = inner.y + ((clock * 7.0) as u16 % inner.height);
        for x in inner.x..inner.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                if let Color::Rgb(r, g, b) = cell.fg {
                    cell.set_fg(Rgb(r, g, b).mix(fx::WHITE, 0.3).c());
                }
            }
        }
    }

    if app.world.order.len() <= 1 {
        let msg = "NO SUBAGENTS YET // AWAITING SPAWN";
        let x = inner.x + inner.width.saturating_sub(msg.len() as u16) / 2;
        buf.set_stringn(
            x,
            (inner.bottom() - 2).max(inner.y),
            msg,
            inner.width as usize,
            fg(fx::DIM),
        );
    }
    // Agents outside the visible set (`nodes` also holds the ones fading
    // out), and how many of them are flagged.
    let (count, flags) = archived_label(app.hidden, app.hidden_flagged, inner.width as usize);
    let width = (count.chars().count() + flags.chars().count()) as u16;
    let x = inner.right().saturating_sub(width);
    let y = inner.bottom() - 1;
    let (x, _) = buf.set_stringn(x, y, &count, inner.width as usize, fg(fx::DIM));
    buf.set_stringn(x, y, &flags, inner.width as usize, bold(fx::ORANGE));
}

/// The "+N archived" label in two parts, count and flags, shortened to fit
/// `width` cells: " +54 archived · 3 ▲ ", then " +54 · 3 ▲ ", then " 3 ▲ ".
/// Flags outlast the count, since nothing else may show them on a small
/// screen. Both parts are empty when there is nothing to say or no room.
fn archived_label(hidden: usize, flagged: usize, width: usize) -> (String, String) {
    let flags = |sep: &str| match flagged {
        0 => String::new(),
        n => format!("{sep}{n} {HEALTH_GLYPH} "),
    };
    if hidden == 0 {
        return (String::new(), String::new());
    }
    let mut tries = vec![
        (format!(" +{hidden} archived "), flags("· ")),
        (format!(" +{hidden} "), flags("· ")),
    ];
    if flagged > 0 {
        tries.push((" ".to_string(), flags("")));
    }
    tries
        .into_iter()
        .find(|(c, f)| c.chars().count() + f.chars().count() <= width)
        .unwrap_or_default()
}

/// The flag part of a node's bottom row, at most `budget` cells: "loop x12"
/// when it fits, else the word alone ("loop"), else only the glyph. Never a
/// cut-off word or count.
fn flag_status(short: &str, budget: usize) -> String {
    let word = short.split(' ').next().unwrap_or("");
    [
        format!(" {HEALTH_GLYPH} {short} "),
        format!(" {HEALTH_GLYPH} {word} "),
        format!(" {HEALTH_GLYPH} {word}"),
        format!(" {HEALTH_GLYPH} "),
        format!(" {HEALTH_GLYPH}"),
    ]
    .into_iter()
    .find(|s| s.chars().count() <= budget)
    .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    buf: &mut Buffer,
    x0: u16,
    y0: u16,
    w: usize,
    a: &Agent,
    world: &World,
    selected: bool,
    clock: f64,
    born: f32,
    dying: f32,
    flash: f32,
) {
    let root = a.id == ROOT;
    let idle = world.is_idle(a);
    let base = agent_color(a);
    let pulse = 0.8 + 0.2 * ((clock * 5.0 + a.order as f64).sin() as f32);
    let mut border = match a.status {
        Status::Running if idle => base.scale(0.5),
        Status::Running => base.scale(pulse),
        Status::Done => base.scale(0.35),
        Status::Failed => {
            if hash(a.order as u32, (clock * 12.0) as u32, 7).is_multiple_of(4) {
                fx::RED.scale(0.4)
            } else {
                fx::RED
            }
        }
    };
    let health = world.health(a);
    if health.is_flagged() {
        border = fx::ORANGE.scale(pulse);
    }
    border = border.mix(fx::WHITE, flash * 0.8);
    if selected {
        border = border.mix(fx::WHITE, 0.45);
    }
    let dim = a.status != Status::Running || idle;
    let text = if dim { fx::TEXT.scale(0.55) } else { fx::TEXT };

    let (tl, tr, bl, bx, h, v) = if root || selected {
        ('╔', '╗', '╚', '╝', '═', '║')
    } else {
        ('╭', '╮', '╰', '╯', '─', '│')
    };

    // Three rows of (char, color, bold).
    type Row = Vec<(char, Rgb, bool)>;
    let fill = |row: &mut Row, to: usize| {
        while row.len() < to {
            row.push((h, border, false));
        }
    };
    let put = |row: &mut Row, s: &str, c: Rgb, b: bool| row.extend(s.chars().map(|ch| (ch, c, b)));

    let mut top: Row = vec![(tl, border, false), (h, border, false)];
    let glyph = if root { '◈' } else { '◆' };
    let name = if root {
        "MAIN".to_string()
    } else {
        clean(&a.agent_type, w.saturating_sub(8))
    };
    put(
        &mut top,
        &format!(" {glyph} {name} "),
        base.mix(fx::WHITE, if dim { 0.0 } else { 0.25 }),
        true,
    );
    fill(&mut top, w - 1);
    top.push((tr, border, false));

    let mut mid: Row = vec![(v, border, false), (' ', text, false)];
    let label = if a.label.is_empty() { &a.id } else { &a.label };
    put(&mut mid, &clean(label, w - 4), text, false);
    while mid.len() < w - 1 {
        mid.push((' ', text, false));
    }
    mid.push((v, border, false));

    let mut bot: Row = vec![(bl, border, false), (h, border, false)];
    // Whole thousands here: the box is narrow.
    let tokens = match a.shown_tokens() {
        n if n >= 1_000_000 => format!(" {:.1}M ", n as f64 / 1e6),
        n if n >= 1_000 => format!(" {}k ", n / 1_000),
        n => format!(" {n} "),
    };
    let budget = w.saturating_sub(4 + tokens.chars().count());
    let (status, sc) = match a.status {
        Status::Running if idle => (" ◌ idle".to_string(), fx::DIM),
        Status::Running => {
            let spin = SPINNER[((clock * 12.0) as usize + a.order) % SPINNER.len()];
            let what = if a.last_tool.is_empty() {
                "thinking"
            } else {
                &a.last_tool
            };
            (
                format!(" {spin} {} ", clean(what, budget.saturating_sub(4))),
                fx::CYAN.mix(fx::WHITE, flash),
            )
        }
        Status::Done => (" ✔ done".to_string(), fx::GREEN.scale(0.6)),
        Status::Failed => (" ✖ fail".to_string(), fx::RED),
    };
    let (status, sc) = if health.is_flagged() {
        (flag_status(&a.health.short(health), budget), fx::ORANGE)
    } else {
        (status, sc)
    };
    let status: String = status.chars().take(budget).collect();
    put(&mut bot, &status, sc, false);
    fill(&mut bot, w.saturating_sub(tokens.chars().count() + 2));
    put(&mut bot, &tokens, text, false);
    fill(&mut bot, w - 1);
    bot.push((bx, border, false));

    let seed = (clock * 30.0) as u32;
    for (dy, row) in [top, mid, bot].into_iter().enumerate() {
        for (dx, (mut ch, mut color, is_bold)) in row.into_iter().take(w).enumerate() {
            let noise = hash(dx as u32, dy as u32 + a.order as u32 * 3, seed);
            if born < 1.0 && dx as f32 > born * w as f32 {
                // Materialize: text decodes left to right out of noise.
                ch = GLITCH[noise as usize % GLITCH.len()];
                color = base.mix(fx::WHITE, 0.5);
            }
            if dying > 0.0 {
                // Dissolve: cells drop out at random.
                let gone = hash(dx as u32, dy as u32, a.order as u32) % 100;
                if (gone as f32) < dying * 100.0 {
                    ch = if (gone as f32) < dying * 60.0 {
                        ' '
                    } else {
                        '░'
                    };
                }
                color = color.scale(1.0 - dying * 0.7);
            }
            if let Some(cell) = buf.cell_mut((x0 + dx as u16, y0 + dy as u16)) {
                let style = if is_bold { bold(color) } else { fg(color) };
                cell.set_char(ch).set_style(style.bg(fx::BG.c()));
            }
        }
    }
}

// --------------------------------------------------------------- side panes

/// Longest prompt or output the full-size inspector shows, in chars.
const FULL_TEXT_MAX: usize = 100_000;

/// Prompt or output text for the inspector. Normal size: one cleaned line.
/// Full size: the whole text, line breaks kept.
fn inspect_text(text: &str, full: bool) -> Vec<Line<'static>> {
    let style = fg(fx::TEXT.scale(0.8));
    if !full {
        return vec![Line::from(Span::styled(clean(text, 150), style))];
    }
    let mut text = text;
    if let Some((cut, _)) = text.char_indices().nth(FULL_TEXT_MAX) {
        text = &text[..cut];
    }
    text.lines()
        .map(|l| Line::from(Span::styled(clean(l, FULL_TEXT_MAX), style)))
        .collect()
}

fn draw_inspector(buf: &mut Buffer, area: Rect, app: &mut App) {
    let rows = inspector_rows(area, app);
    let inner_h = area.height.saturating_sub(2) as usize;
    let max = rows.len().saturating_sub(inner_h);
    app.panes.inspect_rows = inner_h;
    app.panes.inspect_max = max;
    let scroll = app.panes.inspect_scroll.min(max);
    let mut right = Vec::new();
    if max > 0 {
        let pos = format!(" ↕ {}/{} ", scroll + inner_h.min(rows.len()), rows.len());
        right.push((1, Span::styled(pos, fg(fx::DIM))));
    }
    let block = pane_for(" INSPECT ", app, Panel::Inspector, area.width, right);
    let inner = block.inner(area);
    block.render(area, buf);
    for (i, row) in rows
        .iter()
        .skip(scroll)
        .take(inner.height as usize)
        .enumerate()
    {
        row.render(Rect::new(inner.x, inner.y + i as u16, inner.width, 1), buf);
    }
}

/// The inspector's content, wrapped to the panel's inner width.
fn inspector_rows(area: Rect, app: &App) -> Vec<Line<'static>> {
    let width = area.width.saturating_sub(2) as usize;
    let full = app.panes.maximised && app.panes.focus == Panel::Inspector;
    let w = &app.world;
    let Some(a) = w.order.get(app.selected).and_then(|id| w.agents.get(id)) else {
        return Vec::new();
    };
    let color = agent_color(a);
    let (glyph, status, sc) = status_style(w, a);
    let elapsed = a.finished_ts.unwrap_or(w.now_ts) - a.spawned_ts;
    let kv = |k: &'static str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<8}"), fg(fx::DIM)),
            Span::styled(v, fg(fx::TEXT)),
        ])
    };
    let rule = |t: &'static str| Line::from(Span::styled(format!("── {t} ──"), fg(fx::DIM)));
    let or_dash = |s: &str| {
        if s.is_empty() {
            "—".to_string()
        } else {
            s.to_string()
        }
    };

    let mut lines = vec![
        Line::from(Span::styled(format!("◆ {}", a.agent_type), bold(color))),
        Line::from(Span::styled(or_dash(&a.label), fg(fx::TEXT))),
        Line::from(vec![
            Span::styled("STATUS  ", fg(fx::DIM)),
            Span::styled(format!("{glyph} {status} "), bold(sc)),
            Span::styled(fmt_dur(elapsed), fg(fx::TEXT)),
        ]),
        kv("ID", clean(&a.id, 24)),
        kv("PARENT", or_dash(a.parent.as_deref().unwrap_or(""))),
        kv("MODEL", or_dash(&a.model)),
        kv(
            "TOKENS",
            format!(
                "{}   tools {}   err {}",
                fmt_tokens(a.shown_tokens()),
                a.tools,
                a.errors
            ),
        ),
        kv(
            "IN/OUT",
            format!(
                "{} / {}",
                fmt_tokens(a.tokens.input),
                fmt_tokens(a.tokens.output)
            ),
        ),
        // Two short lines: the inspector is 38 cells wide, 28 for the value.
        kv(
            "CACHE",
            format!(
                "write 5m {}  1h {}",
                fmt_tokens(a.tokens.cache_write_5m),
                fmt_tokens(a.tokens.cache_write_1h)
            ),
        ),
        kv("", format!("read {}", fmt_tokens(a.tokens.cache_read))),
        kv("COST", a.cost().map_or_else(|| "—".to_string(), fmt_usd)),
    ];
    if a.id == ROOT {
        lines.push(kv("SUBS", subagent_share(w)));
    }
    lines.push(rule("PROMPT"));
    lines.extend(inspect_text(&a.prompt, full));
    lines.push(rule("LAST OUTPUT"));
    lines.extend(inspect_text(&a.last_text, full));
    lines.push(rule("TOOLS"));
    if let Some(reason) = w.health_reason(a) {
        let line = Line::from(vec![
            Span::styled(format!("{:<8}", "HEALTH"), fg(fx::DIM)),
            Span::styled(format!("{HEALTH_GLYPH} {reason}"), bold(fx::ORANGE)),
        ]);
        lines.insert(3, line);
    }
    for t in a.recent_tools.iter().rev() {
        lines.push(Line::from(vec![
            Span::styled("▸ ", fg(fx::CYAN)),
            Span::styled(clean(t, width.saturating_sub(3)), fg(fx::TEXT.scale(0.8))),
        ]));
    }
    lines.iter().flat_map(|l| wrap_line(l, width)).collect()
}

/// Word-wraps a styled line to `width` display cells. Breaks at the last
/// space that fits, or inside a word longer than the width; drops the space
/// at a break. Never splits a character.
fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    let cw = |c: char| c.width().unwrap_or(0);
    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for (c, st) in cells {
        while !cur.is_empty() && cur.iter().map(|x| cw(x.0)).sum::<usize>() + cw(c) > width {
            match cur.iter().rposition(|x| x.0 == ' ').filter(|&i| i > 0) {
                Some(sp) => {
                    let rest = cur.split_off(sp + 1);
                    cur.pop();
                    rows.push(std::mem::replace(&mut cur, rest));
                }
                None => rows.push(std::mem::take(&mut cur)),
            }
            // A row that starts after a break has no leading space.
            while cur.first().is_some_and(|x| x.0 == ' ') {
                cur.remove(0);
            }
        }
        if c == ' ' && cur.is_empty() && !rows.is_empty() {
            continue;
        }
        cur.push((c, st));
    }
    rows.push(cur);
    rows.into_iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (c, st) in row {
                match spans.last_mut() {
                    Some(s) if s.style == st => s.content.to_mut().push(c),
                    _ => spans.push(Span::styled(c.to_string(), st)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn draw_roster(buf: &mut Buffer, area: Rect, app: &App) {
    if app.cost_view {
        draw_cost_by_type(buf, area, app);
        return;
    }
    let follow = if app.follow {
        " ROSTER · follow "
    } else {
        " ROSTER "
    };
    let block = pane_for(follow, app, Panel::Roster, area.width, Vec::new());
    let inner = block.inner(area);
    block.render(area, buf);
    let w = &app.world;
    let rows = inner.height as usize;
    let start = app
        .selected
        .saturating_sub(rows / 2)
        .min(w.order.len().saturating_sub(rows));
    for (row, id) in w.order.iter().enumerate().skip(start).take(rows) {
        let a = &w.agents[id];
        let (glyph, _, sc) = status_style(w, a);
        let (glyph, sc) = if w.health(a).is_flagged() {
            ('▲', fx::ORANGE)
        } else {
            (glyph, sc)
        };
        let y = inner.y + (row - start) as u16;
        let selected = row == app.selected;
        let name = if a.label.is_empty() {
            a.agent_type.clone()
        } else {
            format!("{} · {}", a.agent_type, a.label)
        };
        let line = Line::from(vec![
            Span::styled(if selected { "▶ " } else { "  " }, bold(fx::CYAN)),
            Span::styled(format!("{glyph} "), fg(sc)),
            Span::styled(
                clean(&name, inner.width.saturating_sub(4) as usize),
                if selected {
                    bold(fx::WHITE)
                } else {
                    fg(agent_color(a).scale(0.75))
                },
            ),
        ]);
        line.render(Rect::new(inner.x, y, inner.width, 1), buf);
    }
}

/// Header figure: the priced total, `+` when some agents could not be priced,
/// a dash when none could.
fn session_cost(w: &World) -> String {
    let s = w.cost_summary();
    match (s.total() > 0.0, s.unpriced > 0) {
        (false, true) => "—".to_string(),
        (true, true) => format!("{}+", fmt_usd(s.total())),
        _ => fmt_usd(s.total()),
    }
}

fn subagent_share(w: &World) -> String {
    let s = w.cost_summary();
    match s.subagent_share() {
        // Same marker as the header: some agents could not be priced.
        Some(p) if s.unpriced > 0 => {
            format!("{}+  {:.0}% of priced", fmt_usd(s.subagents), p * 100.0)
        }
        Some(p) => format!("{}  {:.0}% of session", fmt_usd(s.subagents), p * 100.0),
        None => "—".to_string(),
    }
}

fn draw_cost_by_type(buf: &mut Buffer, area: Rect, app: &App) {
    let block = pane_for(" COST BY TYPE ", app, Panel::Roster, area.width, Vec::new());
    let inner = block.inner(area);
    block.render(area, buf);
    let rows = inner.height as usize;
    if rows == 0 {
        return;
    }
    // Columns: type, agent count, tokens, cost.
    let name_w = (inner.width as usize).saturating_sub(23).max(4);
    let header = format!("{:<name_w$} {:>3} {:>7} {:>9}", "TYPE", "N", "TOK", "COST");
    Line::from(Span::styled(header, fg(fx::DIM)))
        .render(Rect::new(inner.x, inner.y, inner.width, 1), buf);
    for (i, r) in app.world.cost_by_type().iter().take(rows - 1).enumerate() {
        let color = if r.agent_type == "main" {
            fx::CYAN
        } else {
            fx::type_color(&r.agent_type)
        };
        let cost = match (r.usd > 0.0, r.unpriced > 0) {
            (false, true) => "—".to_string(),
            (true, true) => format!("{}+", fmt_usd(r.usd)),
            _ => fmt_usd(r.usd),
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{:<name_w$} ", clean(&r.agent_type, name_w)),
                fg(color.scale(0.85)),
            ),
            Span::styled(
                format!("{:>3} {:>7} ", r.agents, fmt_tokens(r.tokens)),
                fg(fx::TEXT.scale(0.8)),
            ),
            Span::styled(format!("{cost:>9}"), bold(fx::TEXT)),
        ]);
        let y = inner.y + 1 + i as u16;
        line.render(Rect::new(inner.x, y, inner.width, 1), buf);
    }
}

fn draw_log(buf: &mut Buffer, area: Rect, app: &mut App) {
    let rows = area.height.saturating_sub(2) as usize;
    app.panes.stream_rows = rows;
    let p = &app.panes;
    // Right of the title: paused state, then the search.
    let mut status: Vec<(u8, Span)> = Vec::new();
    if p.anchor.is_some() {
        let new = app.new_lines();
        let text = if new > 0 {
            format!(" ⏸ PAUSED +{new} new ")
        } else {
            " ⏸ PAUSED ".to_string()
        };
        status.push((3, Span::styled(text, bold(fx::YELLOW))));
        status.push((1, Span::styled("· G tail ", fg(fx::YELLOW))));
    }
    let n = needle(&p.query);
    let matches = if p.query.is_empty() {
        Vec::new()
    } else {
        app.matches()
    };
    if !p.query.is_empty() {
        let at = p
            .current
            .and_then(|c| matches.iter().position(|&i| app.log_no(i) == c))
            .map_or("-".to_string(), |i| (i + 1).to_string());
        let grep = if app.filtering() { " grep" } else { "" };
        status.push((
            2,
            Span::styled(
                format!(" /{} {at}/{}{grep} ", head_fit(&p.query, 20), matches.len()),
                bold(fx::MAGENTA),
            ),
        ));
    }
    let block = pane_for(" EVENT STREAM ", app, Panel::Stream, area.width, status);
    let inner = block.inner(area);
    block.render(area, buf);

    let w = &app.world;
    let list = if app.filtering() {
        matches
    } else {
        (0..w.log.len()).collect()
    };
    let Some(bottom) = app.stream_bottom(&list) else {
        return;
    };
    let start = (bottom + 1).saturating_sub(rows);
    let shown = &list[start..=bottom];
    // Older lines recede while following; reading back, all are bright.
    let fade_on = p.anchor.is_none() && !p.maximised;
    let hit = bold(fx::BG).bg(fx::MAGENTA.scale(0.8).c());
    let current_hit = bold(fx::BG).bg(fx::YELLOW.c());
    for (row, &i) in shown.iter().enumerate() {
        let l = &w.log[i];
        let (mark, color) = match l.kind {
            LogKind::Spawn => ("✦", fx::MAGENTA),
            LogKind::Done => ("✔", fx::GREEN),
            LogKind::Fail => ("✖", fx::RED),
            LogKind::Tool => ("▸", fx::CYAN),
            LogKind::Error => ("!", fx::RED),
            LogKind::Msg => ("✉", fx::YELLOW),
            LogKind::Text => ("»", fx::TEXT.scale(0.7)),
            LogKind::Prompt => ("❯", fx::WHITE),
            LogKind::Health => (HEALTH_GLYPH, fx::ORANGE),
        };
        let age = (rows - (shown.len() - row)) as f32 / rows.max(1) as f32;
        let fade = if fade_on { 0.55 + 0.45 * age } else { 1.0 };
        let who_color = w.agents.get(&l.agent).map_or(fx::DIM, agent_color);
        let hl = if p.current == Some(app.log_no(i)) {
            current_hit
        } else {
            hit
        };
        let mut spans = vec![Span::styled(format!("{} ", fmt_clock(l.ts)), fg(fx::DIM))];
        let who = format!("{:<14} ", log_who(w, l));
        spans.extend(highlight(&who, &n, fg(who_color.scale(fade)), hl));
        spans.push(Span::styled(format!("{mark} "), fg(color)));
        spans.extend(highlight(&l.text, &n, fg(fx::TEXT.scale(fade)), hl));
        let y = inner.y + row as u16;
        Line::from(spans).render(Rect::new(inner.x, y, inner.width, 1), buf);
    }
}

/// `text` as spans in `base` style, with each case-insensitive match of
/// `needle` in `hit` style. Splits only between chars.
fn highlight(text: &str, needle: &[char], base: Style, hit: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at = 0; // char position
    let mut rest = text;
    for (a, b) in find_ci(text, needle) {
        let (before, tail) = split_chars(rest, a - at);
        let (word, after) = split_chars(tail, b - a);
        if !before.is_empty() {
            out.push(Span::styled(before.to_string(), base));
        }
        out.push(Span::styled(word.to_string(), hit));
        rest = after;
        at = b;
    }
    if !rest.is_empty() || out.is_empty() {
        out.push(Span::styled(rest.to_string(), base));
    }
    out
}

/// The start of `s` that fits in `width` display cells, with `…` when cut.
/// Wide characters are kept whole, unlike `clean`, which replaces them.
fn head_fit(s: &str, width: usize) -> String {
    if s.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// Splits after `n` chars.
fn split_chars(s: &str, n: usize) -> (&str, &str) {
    let cut = s.char_indices().nth(n).map_or(s.len(), |(i, _)| i);
    s.split_at(cut)
}

const HELP: [&str; 34] = [
    "tab / S-tab  focus next / previous panel",
    "z            full size focused panel, again",
    "             or esc to restore",
    "↑ ↓ / j k    graph, roster: select agent",
    "             stream, inspector: scroll",
    "PgUp PgDn    the same, a page at a time",
    "g G          stream oldest / newest line",
    "Home End     (inspector when it has focus)",
    "[ ]          shorter / taller event stream",
    "< >          narrower / wider side column",
    "",
    "/            search the event stream:",
    "             agent type or text, ignoring",
    "             case; enter keeps, esc drops",
    "n N          older / newer match",
    "&            show matching lines only",
    "",
    "f            follow most recent activity",
    "c            roster: cost by agent type",
    "space        pause or resume replay or demo",
    ". ,          paused: step to next / previous",
    "             event (pauses a playing replay)",
    "e            captions: explain each event",
    "+ / -        replay speed",
    "r            replay session from start",
    "l            jump to end, follow live",
    "esc          one step back: query line, full",
    "             size, search, then session list",
    "q            quit",
    "",
    "◆ magenta packet   spawn",
    "◆ green packet     result returned",
    "◆ yellow packet    message between agents",
    "◆ red packet       failure",
];

fn draw_help(buf: &mut Buffer, area: Rect) {
    // One column when it fits, else two side by side, else cut at the bottom.
    let col_w = 46u16;
    let cols = if HELP.len() as u16 + 2 > area.height && area.width >= col_w * 2 + 2 {
        2
    } else {
        1
    };
    let per = HELP.len().div_ceil(cols as usize);
    let w = (col_w * cols + 2).min(area.width);
    let h = (per as u16 + 2).min(area.height);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    Clear.render(rect, buf);
    buf.set_style(rect, Style::default().bg(fx::BG.c()));
    let block = panel(" HELP ").border_style(fg(fx::CYAN));
    let inner = block.inner(rect);
    block.render(rect, buf);
    for (c, chunk) in HELP.chunks(per).enumerate() {
        let x = inner.x + c as u16 * col_w;
        if x >= inner.right() {
            break;
        }
        let col = Rect::new(x, inner.y, (inner.right() - x).min(col_w), inner.height);
        let text: Vec<Line> = chunk
            .iter()
            .map(|l| Line::from(Span::styled(format!(" {l}"), fg(fx::TEXT))))
            .collect();
        Paragraph::new(text).render(col, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoSource;
    use crate::event::{Event, Kind};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn frame(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect()
    }

    fn busy_app() -> App {
        let mut app = App::new(Box::new(DemoSource::new(3)), Mode::Live, "demo", "demo");
        for _ in 0..450 {
            app.tick(1.0 / 30.0);
        }
        app
    }

    #[test]
    fn renders_all_panels() {
        let mut app = busy_app();
        frame(&mut app, 140, 40); // first frame sizes the graph
        app.tick(1.0 / 30.0);
        let text = frame(&mut app, 140, 40);
        for part in [
            "NEURAL TOPOLOGY",
            "INSPECT",
            "ROSTER",
            "EVENT STREAM",
            "MAIN",
            "LIVE",
        ] {
            assert!(text.contains(part), "missing {part}");
        }
    }

    #[test]
    fn never_panics_at_any_terminal_size() {
        let mut app = busy_app();
        app.help = true;
        app.captions = true;
        for w in (1..220).step_by(7) {
            for h in (1..70).step_by(3) {
                frame(&mut app, w, h);
                app.tick(1.0 / 30.0);
                app.cost_view = !app.cost_view;
            }
        }
    }

    /// Puts the panes in view state number `k`: every focus, normal and full
    /// size, stream at the tail, the top or a bogus line, inspector scrolled
    /// past its end, with and without a search (wide chars too) and grep.
    fn pane_state(app: &mut App, k: usize) {
        const FOCUS: [Panel; 4] = [Panel::Graph, Panel::Inspector, Panel::Roster, Panel::Stream];
        app.panes.focus = FOCUS[k % 4];
        app.panes.maximised = (k / 4) % 2 == 1;
        app.stream_tail();
        match (k / 8) % 3 {
            0 => {}
            1 => app.stream_top(),
            _ => app.panes.anchor = Some(if k.is_multiple_of(2) { 0 } else { u64::MAX }),
        }
        app.panes.inspect_scroll = if k.is_multiple_of(3) { usize::MAX } else { 0 };
        app.search_clear();
        match (k / 24) % 4 {
            0 => {}
            1 => app.panes.query = "read".into(),
            2 => {
                app.panes.query = "本e".into();
                app.panes.input = true;
            }
            _ => {
                app.panes.query = "e".into();
                app.panes.filter = true;
                app.search_step(true);
            }
        }
        app.panes.stream_h = [None, Some(9), Some(1), Some(200)][k % 4];
        app.panes.side_w = [40, 0, 500][(k / 3) % 3];
    }

    struct NoEvents;

    impl crate::event::Source for NoEvents {
        fn poll(&mut self) -> Vec<Event> {
            Vec::new()
        }
    }

    fn apply(app: &mut App, ts: i64, kind: Kind) {
        app.world.apply(&Event { ts, kind }, true);
    }

    /// A crowded synthetic session: `n` agents three levels deep (every
    /// third under the previous agent), all running but every fourth
    /// finished, and every tenth looping.
    fn crowded_app(n: usize) -> App {
        let mut app = App::new(Box::new(NoEvents), Mode::Replay, "crowd", "crowd");
        for i in 0..n {
            let parent = match i % 3 {
                0 => ROOT.to_string(),
                _ => format!("a{}", i - 1),
            };
            let kind = Kind::Spawn {
                id: format!("a{i}"),
                parent,
                agent_type: format!("type-{}", i % 7),
                label: format!("task {i}"),
            };
            apply(&mut app, i as i64, kind);
        }
        for i in 0..n {
            let id = format!("a{i}");
            let calls = if i % 10 == 5 { 4 } else { 1 };
            for c in 0..calls {
                let kind = Kind::Tool {
                    agent: id.clone(),
                    tool: "Read".into(),
                    detail: "src/main.rs".into(),
                    key: if calls > 1 { 7 } else { i as u64 + c },
                };
                apply(&mut app, 1000 + i as i64, kind);
            }
            if i % 4 == 3 {
                let kind = Kind::Finish {
                    id,
                    ok: true,
                    tokens: None,
                };
                apply(&mut app, 1000 + i as i64, kind);
            }
        }
        app
    }

    #[test]
    fn archived_count_ignores_fading_nodes() {
        let mut app = crowded_app(30);
        frame(&mut app, 140, 40); // sizes the graph
        app.tick(1.0 / 30.0);
        let shown = |app: &App| app.nodes.values().filter(|n| n.dying.is_none()).count();
        assert_eq!(app.hidden, app.world.order.len() - shown(&app));
        // Finish every running agent on the graph: others take their place
        // and the finished ones fade out.
        let on_graph: Vec<String> = app.nodes.keys().filter(|id| *id != ROOT).cloned().collect();
        for id in on_graph {
            let kind = Kind::Finish {
                id,
                ok: true,
                tokens: None,
            };
            apply(&mut app, 2000, kind);
        }
        app.tick(1.0 / 30.0);
        assert!(
            app.nodes.values().any(|n| n.dying.is_some()),
            "nothing fades"
        );
        let hidden = app.world.order.len() - shown(&app);
        assert_eq!(app.hidden, hidden);
        assert!(app.nodes.len() > shown(&app));
        let text = frame(&mut app, 140, 40);
        assert!(
            text.contains(&format!("+{hidden} archived")),
            "want +{hidden} archived:\n{text}"
        );
    }

    #[test]
    fn archived_label_shortens_and_keeps_flags() {
        let join = |(c, f): (String, String)| c + &f;
        assert_eq!(join(archived_label(54, 3, 80)), " +54 archived · 3 ▲ ");
        assert_eq!(join(archived_label(54, 0, 80)), " +54 archived ");
        assert_eq!(join(archived_label(54, 3, 12)), " +54 · 3 ▲ ");
        assert_eq!(join(archived_label(54, 3, 5)), " 3 ▲ ");
        assert_eq!(join(archived_label(54, 0, 5)), " +54 ");
        assert_eq!(join(archived_label(54, 3, 2)), "");
        assert_eq!(join(archived_label(0, 0, 80)), "");
        for w in 0..30 {
            let s = join(archived_label(1234, 56, w));
            assert!(s.chars().count() <= w, "{w}: {s:?}");
        }
    }

    #[test]
    fn small_graph_counts_hidden_flags() {
        // 150 agents with every tenth looping; at 80x24 most flags are off
        // the graph and the header has no room for its flag count.
        let mut app = crowded_app(150);
        frame(&mut app, 80, 24);
        app.tick(1.0 / 30.0);
        let text = frame(&mut app, 80, 24);
        assert!(app.hidden_flagged > 0);
        assert!(!text.contains("FLAGGED"), "{text}");
        let want = format!("+{} archived · {} ▲", app.hidden, app.hidden_flagged);
        assert!(text.contains(&want), "want {want}:\n{text}");
        // Healthy running agents still get a node.
        let healthy = app.nodes.keys().filter(|id| {
            let a = &app.world.agents[*id];
            *id != ROOT && a.status == Status::Running && !a.health.flag().is_flagged()
        });
        assert!(healthy.count() >= 1, "{text}");
    }

    #[test]
    fn never_panics_with_a_crowded_graph() {
        let mut app = crowded_app(150);
        assert!(app.world.flagged() >= 5);
        assert!(app.world.running() >= 40);
        app.follow = false;
        app.selected = app.world.order.len() - 1;
        for w in (1..220).step_by(7) {
            for h in (1..70).step_by(3) {
                frame(&mut app, w, h);
                app.tick(1.0 / 30.0);
                app.selected = (app.selected + 37) % app.world.order.len();
            }
        }
    }

    #[test]
    fn never_panics_in_any_pane_state_at_any_size() {
        let mut app = busy_app();
        // Text with wide and zero-width characters, in the stream and the
        // inspector.
        let wide = "日本語のテキスト 🦀 e\u{301} wide";
        for (i, kind) in [
            Kind::Text {
                agent: ROOT.into(),
                text: wide.into(),
            },
            Kind::Prompt {
                agent: ROOT.into(),
                text: format!("{wide}\n\nline two {}", "long ".repeat(400)),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let ts = app.world.now_ts + i as i64;
            app.world.apply(&Event { ts, kind }, false);
        }
        app.selected = 0;
        let mut k = 0;
        for w in (1..220).step_by(7) {
            for h in (1..70).step_by(3) {
                for _ in 0..2 {
                    pane_state(&mut app, k);
                    app.help = k.is_multiple_of(5);
                    app.captions = k.is_multiple_of(3);
                    frame(&mut app, w, h);
                    k += 1;
                }
                app.tick(1.0 / 30.0);
            }
        }
        // 96 states, each at about 15 different sizes.
        assert!(k >= 96 * 15, "every pane state ran");
    }

    #[test]
    fn wrap_line_respects_width_and_never_splits_a_char() {
        let style = Style::default();
        let line = Line::from(vec![
            Span::styled("hello wide ", style),
            Span::styled("日本語日本語", bold(fx::CYAN)),
            Span::styled(" averyveryverylongword end", style),
        ]);
        for width in 1..30 {
            let rows = wrap_line(&line, width);
            let text: String = rows
                .iter()
                .flat_map(|r| r.spans.iter().map(|s| s.content.to_string()))
                .collect();
            for r in &rows {
                assert!(r.width() <= width.max(2), "width {width}: {r:?}");
            }
            // Only break spaces go missing.
            assert_eq!(
                text.replace(' ', ""),
                "hellowide日本語日本語averyveryverylongwordend"
            );
        }
        let rows = wrap_line(&line, 12);
        assert_eq!(rows[0].to_string(), "hello wide");
        assert!(wrap_line(&line, 0).is_empty());
        assert_eq!(wrap_line(&Line::from(""), 10).len(), 1, "blank lines stay");
    }

    #[test]
    fn head_fit_keeps_wide_chars_whole() {
        assert_eq!(head_fit("read", 20), "read");
        assert_eq!(head_fit("日本語のテキスト", 7), "日本語…");
        assert_eq!(head_fit("abcdef", 4), "abc…");
        for w in 1..12 {
            assert!(Span::raw(head_fit("日本語のテキスト", w)).width() <= w.max(1));
        }
    }

    #[test]
    fn highlight_marks_whole_chars() {
        let (base, hit) = (fg(fx::TEXT), fg(fx::RED));
        let spans = highlight("Read 日本 read", &needle("本 r"), base, hit);
        let parts: Vec<(&str, bool)> = spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style == hit))
            .collect();
        assert_eq!(
            parts,
            vec![("Read 日", false), ("本 r", true), ("ead", false)]
        );
        let none = highlight("abc", &[], base, hit);
        assert_eq!(none.len(), 1);
        assert_eq!(highlight("", &needle("x"), base, hit).len(), 1);
    }

    /// Frames for review: `cargo test review_frames -- --nocapture` prints
    /// the default view, the stream focused and scrolled back, the stream
    /// full size with a search, and the inspector full size.
    #[test]
    fn review_frames() {
        for (w, h) in [(140, 40), (100, 30), (60, 16)] {
            let mut app = busy_app();
            frame(&mut app, w, h);
            app.tick(1.0 / 30.0);
            let default = frame(&mut app, w, h);
            assert!(default.contains("▸ NEURAL TOPOLOGY"), "graph has focus");

            app.panes.focus = Panel::Stream;
            app.scroll_stream(-app.stream_page());
            app.scroll_stream(-3);
            for _ in 0..90 {
                app.tick(1.0 / 30.0);
            }
            let scrolled = frame(&mut app, w, h);
            assert!(scrolled.contains("PAUSED"), "{scrolled}");

            app.stream_tail();
            app.panes.maximised = true;
            app.search_open();
            app.panes.query = "read".into();
            app.search_confirm();
            app.panes.filter = true;
            frame(&mut app, w, h);
            let search = frame(&mut app, w, h);
            assert!(search.contains("/read"), "{search}");
            assert!(search.contains("z restore") || w < 100, "{search}");

            app.search_clear();
            app.panes.focus = Panel::Inspector;
            app.follow = false;
            app.selected = 0;
            let prompt = "Refactor the retry module.\n\nKeep the public API. Run the whole \
                          test suite after each step and stop at the first failure; report \
                          which call sites changed and why.\n- step one\n- step two";
            let kind = Kind::Prompt {
                agent: ROOT.into(),
                text: prompt.into(),
            };
            app.world.apply(&Event { ts: 0, kind }, false);
            let inspector = frame(&mut app, w, h);
            assert!(inspector.contains("INSPECT"));
            if h >= 30 {
                assert!(
                    inspector.contains("- step two"),
                    "whole prompt: {inspector}"
                );
            }
            app.panes.maximised = false;
            app.search_open();
            app.panes.query = "日本 read".into();
            let typing = frame(&mut app, w, h);
            assert!(typing.contains("enter search"), "{typing}");
            app.search_clear();
            app.help = true;
            let help = frame(&mut app, w, h);
            assert!(help.contains("HELP"));
            for (name, f) in [
                ("default", default),
                ("stream focused, scrolled back", scrolled),
                ("stream full size, search \"read\" + grep", search),
                ("inspector full size", inspector),
                ("typing a query", typing),
                ("help", help),
            ] {
                println!("--- {w}x{h} {name}\n{f}");
            }
        }
    }

    #[test]
    fn shows_cost_in_header_and_cost_by_type_view() {
        let mut app = busy_app();
        frame(&mut app, 140, 40);
        let text = frame(&mut app, 140, 40);
        assert!(text.contains("COST ~$"), "header cost missing");
        app.cost_view = true;
        let text = frame(&mut app, 140, 40);
        assert!(text.contains("COST BY TYPE"));
        assert!(!text.contains(" ROSTER "));
        // The main agent's inspector shows what its subagents spent.
        app.follow = false;
        app.selected = 0;
        let text = frame(&mut app, 140, 40);
        assert!(text.contains("% of session"), "subagent share missing");
    }

    #[test]
    fn flag_status_never_cuts_a_word() {
        assert_eq!(flag_status("loop x12", 20), " ▲ loop x12 ");
        assert_eq!(flag_status("loop x12", 9), " ▲ loop ");
        assert_eq!(flag_status("stalled", 10), " ▲ stalled");
        // A 16-cell box with a token count leaves about 7 cells.
        assert_eq!(flag_status("stalled", 7), " ▲ ");
        assert_eq!(flag_status("loop x5", 7), " ▲ loop");
        assert_eq!(flag_status("err x3", 6), " ▲ err");
        assert_eq!(flag_status("err x3", 1), "");
        for budget in 0..20 {
            for short in ["loop x12", "err x3", "stalled"] {
                let s = flag_status(short, budget);
                assert!(s.chars().count() <= budget);
                let word = s.trim().trim_start_matches(HEALTH_GLYPH).trim();
                assert!(word.is_empty() || short.starts_with(word), "{s:?}");
                assert!(word.is_empty() || short.split(' ').any(|w| word.ends_with(w)));
            }
        }
    }

    #[test]
    fn captions_sit_above_the_footer_and_keep_the_graph() {
        for (w, h) in [(140, 40), (80, 24), (60, 16)] {
            let mut app = busy_app();
            frame(&mut app, w, h);
            let off = frame(&mut app, w, h);
            app.captions = true;
            let on = frame(&mut app, w, h);
            let rows: Vec<&str> = on.lines().collect();
            assert!(rows[h as usize - 3].starts_with(" ◆ "), "{w}x{h}:\n{on}");
            assert!(on.contains("NEURAL TOPOLOGY") && on.contains("EVENT STREAM"));
            let off_rows: Vec<&str> = off.lines().collect();
            assert!(!off_rows[h as usize - 3].starts_with(" ◆ "), "{off}");
        }
    }

    #[test]
    fn footer_offers_step_keys_only_where_they_work() {
        let mut app = busy_app();
        assert!(app.mode == Mode::Live && !app.pausable);
        assert!(!frame(&mut app, 220, 40).contains(". , step"));
        app.pausable = true;
        assert!(frame(&mut app, 220, 40).contains(". , step"));
        app.pausable = false;
        app.restart();
        assert!(frame(&mut app, 220, 40).contains(". , step"), "replay");
    }

    #[test]
    fn small_terminal_shows_notice() {
        let mut app = busy_app();
        assert!(frame(&mut app, 40, 10).contains("terminal too small"));
    }
}
