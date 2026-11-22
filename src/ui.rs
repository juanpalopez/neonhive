//! Rendering. Everything draws straight into the ratatui buffer.

use crate::app::{App, Mode, Space, BORN_SECS, DYING_SECS};
use crate::event::ROOT;
use crate::fx::{self, Rgb};
use crate::state::{Agent, LogKind, PacketKind, Status, World};
use crate::util::{clean, fmt_clock, fmt_dur, fmt_tokens, fmt_usd, hash};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;
use std::collections::HashMap;

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
    let [header, body, log, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(9),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(buf, header, app);
    if area.width >= 104 {
        let [graph, side] =
            Layout::horizontal([Constraint::Min(60), Constraint::Length(40)]).areas(body);
        let [inspect, roster] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(side);
        draw_graph(buf, graph, app);
        draw_inspector(buf, inspect, app);
        draw_roster(buf, roster, app);
    } else {
        draw_graph(buf, body, app);
    }
    draw_log(buf, log, app);
    let keys = " ↑↓ select  f follow  c cost  space pause  +/- speed  r replay  l live  ? help  esc sessions  q quit";
    buf.set_stringn(footer.x, footer.y, keys, footer.width as usize, fg(fx::DIM));
    if app.help {
        draw_help(buf, area);
    }
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
    let block = panel(" NEURAL TOPOLOGY ");
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
    let off_screen = (app.world.order.len()).saturating_sub(app.nodes.len());
    if off_screen > 0 {
        let msg = format!(" +{off_screen} archived ");
        let x = inner.right().saturating_sub(msg.chars().count() as u16);
        buf.set_stringn(
            x,
            inner.bottom() - 1,
            &msg,
            inner.width as usize,
            fg(fx::DIM),
        );
    }
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
        // "loop x12" when it fits, else just "loop": never a cut-off count.
        let short = a.health.short(health);
        let full = format!(" {HEALTH_GLYPH} {short} ");
        let word = short.split(' ').next().unwrap_or("");
        let text = if full.chars().count() <= budget {
            full
        } else {
            format!(" {HEALTH_GLYPH} {word} ")
        };
        (text, fx::ORANGE)
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

fn draw_inspector(buf: &mut Buffer, area: Rect, app: &App) {
    let block = panel(" INSPECT ");
    let w = &app.world;
    let Some(a) = w.order.get(app.selected).and_then(|id| w.agents.get(id)) else {
        block.render(area, buf);
        return;
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
        kv(
            "CACHE",
            format!(
                "write {}   read {}",
                fmt_tokens(a.tokens.cache_write),
                fmt_tokens(a.tokens.cache_read)
            ),
        ),
        kv("COST", a.cost().map_or_else(|| "—".to_string(), fmt_usd)),
    ];
    if a.id == ROOT {
        lines.push(kv("SUBS", subagent_share(w)));
    }
    lines.extend([
        rule("PROMPT"),
        Line::from(Span::styled(clean(&a.prompt, 150), fg(fx::TEXT.scale(0.8)))),
        rule("LAST OUTPUT"),
        Line::from(Span::styled(
            clean(&a.last_text, 150),
            fg(fx::TEXT.scale(0.8)),
        )),
        rule("TOOLS"),
    ]);
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
            Span::styled(
                clean(t, area.width.saturating_sub(5) as usize),
                fg(fx::TEXT.scale(0.8)),
            ),
        ]));
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .block(block)
        .render(area, buf);
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
    let block = panel(follow);
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
    let block = panel(" COST BY TYPE ");
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

fn draw_log(buf: &mut Buffer, area: Rect, app: &App) {
    let block = panel(" EVENT STREAM ");
    let inner = block.inner(area);
    block.render(area, buf);
    let w = &app.world;
    let rows = inner.height as usize;
    let skip = w.log.len().saturating_sub(rows);
    for (i, l) in w.log.iter().skip(skip).enumerate() {
        let Some(a) = w.agents.get(&l.agent) else {
            continue;
        };
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
        let age = (rows - (w.log.len() - skip - i)) as f32 / rows.max(1) as f32;
        let fade = 0.55 + 0.45 * age; // older lines recede
        let line = Line::from(vec![
            Span::styled(format!("{} ", fmt_clock(l.ts)), fg(fx::DIM)),
            Span::styled(
                format!("{:<14} ", clean(&a.agent_type, 14)),
                fg(agent_color(a).scale(fade)),
            ),
            Span::styled(format!("{mark} "), fg(color)),
            Span::styled(l.text.as_str(), fg(fx::TEXT.scale(fade))),
        ]);
        line.render(Rect::new(inner.x, inner.y + i as u16, inner.width, 1), buf);
    }
}

fn draw_help(buf: &mut Buffer, area: Rect) {
    let lines = [
        "↑ ↓ / j k    select agent",
        "f            follow most recent activity",
        "c            roster: cost by agent type",
        "space        pause or resume replay",
        "+ / -        replay speed",
        "r            replay session from start",
        "l            jump to end, follow live",
        "esc          back to session list",
        "q            quit",
        "",
        "◆ magenta packet   spawn",
        "◆ green packet     result returned",
        "◆ yellow packet    message between agents",
        "◆ red packet       failure",
    ];
    let (w, h) = (46u16.min(area.width), lines.len() as u16 + 2);
    let rect = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    );
    Clear.render(rect, buf);
    buf.set_style(rect, Style::default().bg(fx::BG.c()));
    let text: Vec<Line> = lines
        .iter()
        .map(|l| Line::from(Span::styled(format!(" {l}"), fg(fx::TEXT))))
        .collect();
    Paragraph::new(text)
        .block(panel(" HELP ").border_style(fg(fx::CYAN)))
        .render(rect, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoSource;
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
        for w in (1..220).step_by(7) {
            for h in (1..70).step_by(3) {
                frame(&mut app, w, h);
                app.tick(1.0 / 30.0);
                app.cost_view = !app.cost_view;
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
    fn small_terminal_shows_notice() {
        let mut app = busy_app();
        assert!(frame(&mut app, 40, 10).contains("terminal too small"));
    }
}
