//! Session list screen.

use crate::claude::SessionInfo;
use crate::fx;
use crate::ui::panel;
use crate::util::{clean, fmt_date, now_ms};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use ratatui::DefaultTerminal;
use std::time::Duration;

/// A session written to this recently is treated as running.
pub const LIVE_WINDOW_MS: i64 = 120_000;

/// Returns the index of the chosen session, or `None` to quit.
pub fn pick(
    term: &mut DefaultTerminal,
    sessions: &[SessionInfo],
    start: usize,
) -> anyhow::Result<Option<usize>> {
    let mut selected = start.min(sessions.len().saturating_sub(1));
    loop {
        term.draw(|f| {
            let area = f.area();
            let buf = f.buffer_mut();
            buf.set_style(area, Style::default().bg(fx::BG.c()).fg(fx::TEXT.c()));
            let heading = format!("{}// SELECT SESSION ", crate::ui::brand());
            let block = panel(&heading);
            let inner = block.inner(area);
            block.render(area, buf);
            if sessions.is_empty() {
                let msg = "no Claude Code sessions found in ~/.claude/projects";
                buf.set_stringn(
                    inner.x + 1,
                    inner.y + 1,
                    msg,
                    inner.width as usize,
                    Style::default().fg(fx::RED.c()),
                );
                return;
            }
            let rows = inner.height.saturating_sub(2) as usize;
            let top = selected
                .saturating_sub(rows / 2)
                .min(sessions.len().saturating_sub(rows));
            let now = now_ms();
            for (i, s) in sessions.iter().enumerate().skip(top).take(rows) {
                let live = now - s.mtime_ms < LIVE_WINDOW_MS;
                let on = i == selected;
                let base = if on { fx::WHITE } else { fx::TEXT.scale(0.75) };
                let style = Style::default().fg(base.c());
                let line = Line::from(vec![
                    Span::styled(
                        if on { " ▶ " } else { "   " },
                        Style::default().fg(fx::CYAN.c()),
                    ),
                    Span::styled(
                        if live { "● LIVE " } else { "       " },
                        Style::default()
                            .fg(fx::GREEN.c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{}  ", fmt_date(s.mtime_ms)),
                        Style::default().fg(fx::DIM.c()),
                    ),
                    Span::styled(
                        format!("{:>3} agents  ", s.agents),
                        Style::default().fg(if s.agents > 0 {
                            fx::MAGENTA.c()
                        } else {
                            fx::DIM.c()
                        }),
                    ),
                    Span::styled(
                        format!("{:<24} ", clean(&s.project, 24)),
                        Style::default().fg(fx::CYAN.c()),
                    ),
                    Span::styled(clean(&s.title, 80), style),
                ]);
                line.render(
                    Rect::new(inner.x, inner.y + (i - top) as u16, inner.width, 1),
                    buf,
                );
            }
            let keys = " ↑↓ select   enter open   q quit";
            buf.set_stringn(
                inner.x,
                inner.bottom() - 1,
                keys,
                inner.width as usize,
                Style::default().fg(fx::DIM.c()),
            );
        })?;

        if !event::poll(Duration::from_millis(500))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(sessions.len().saturating_sub(1));
            }
            KeyCode::PageUp => selected = selected.saturating_sub(10),
            KeyCode::PageDown => selected = (selected + 10).min(sessions.len().saturating_sub(1)),
            KeyCode::Enter if !sessions.is_empty() => return Ok(Some(selected)),
            _ => {}
        }
    }
}
