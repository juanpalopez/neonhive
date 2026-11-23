//! SVG export of rendered frames, for screenshots and documentation.
//!
//! Several frames become one looping, CSS-animated SVG. Each frame is cut
//! into runs of same-styled text; a run that stays put over several frames
//! is written once, with one animation that shows it for those frames, so a
//! long animation costs about what changes between frames, not a full frame
//! each.

use crate::fx;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::collections::HashMap;
use std::fmt::Write;

const CELL_W: f32 = 8.4;
const CELL_H: f32 = 17.0;

fn hex(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => {
            let fx::Rgb(r, g, b) = fx::TEXT;
            format!("#{r:02x}{g:02x}{b:02x}")
        }
    }
}

/// A run of same-styled, non-blank cells on one row, text already escaped.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Run {
    y: u16,
    x: u16,
    color: Color,
    bold: bool,
    text: String,
    cells: usize,
}

/// One frame as runs, in reading order.
fn runs(buf: &Buffer) -> Vec<Run> {
    let mut out = Vec::new();
    let area = buf.area;
    for y in 0..area.height {
        let mut x = 0;
        while x < area.width {
            let cell = &buf[(x, y)];
            if cell.symbol() == " " {
                x += 1;
                continue;
            }
            let (color, bold) = (cell.fg, cell.modifier.contains(Modifier::BOLD));
            let (start, mut text) = (x, String::new());
            while x < area.width {
                let c = &buf[(x, y)];
                if c.fg != color || c.modifier.contains(Modifier::BOLD) != bold {
                    break;
                }
                text.push_str(c.symbol());
                x += 1;
            }
            let text = text.trim_end();
            let cells = text.chars().count().max(1);
            let mut escaped = String::with_capacity(text.len());
            for ch in text.chars() {
                match ch {
                    '&' => escaped.push_str("&amp;"),
                    '<' => escaped.push_str("&lt;"),
                    '>' => escaped.push_str("&gt;"),
                    c => escaped.push(c),
                }
            }
            out.push(Run {
                y,
                x: start,
                color,
                bold,
                text: escaped,
                cells,
            });
        }
    }
    out
}

fn text_element(out: &mut String, r: &Run) {
    // textLength pins every run to the cell grid, whatever the font. Runs of
    // one or two cells start on the grid and are too short to drift off it,
    // so they go without; they are most of what changes between frames.
    let fit = if r.cells > 2 {
        format!(
            r#" textLength="{:.1}" lengthAdjust="spacingAndGlyphs""#,
            r.cells as f32 * CELL_W
        )
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        r#"<text x="{:.1}" y="{:.1}" fill="{}"{fit}{}>{}</text>"#,
        r.x as f32 * CELL_W,
        (r.y as f32 + 0.75) * CELL_H,
        hex(r.color),
        if r.bold { r#" font-weight="bold""# } else { "" },
        r.text,
    );
}

/// A run shown from frame `start` for `len` frames.
#[derive(Debug, PartialEq)]
struct Span {
    start: usize,
    len: usize,
    run: Run,
}

/// Every run with the unbroken stretches of frames it shows in, ordered by
/// start, length and position so the output does not depend on hashing.
fn spans(frames: &[Vec<Run>]) -> Vec<Span> {
    let mut open: HashMap<&Run, usize> = HashMap::new();
    let mut out = Vec::new();
    for (i, frame) in frames.iter().enumerate() {
        let now: std::collections::HashSet<&Run> = frame.iter().collect();
        open.retain(|run, start| {
            let keep = now.contains(run);
            if !keep {
                out.push(Span {
                    start: *start,
                    len: i - *start,
                    run: (*run).clone(),
                });
            }
            keep
        });
        for run in frame {
            open.entry(run).or_insert(i);
        }
    }
    for (run, start) in open {
        out.push(Span {
            start,
            len: frames.len() - start,
            run: run.clone(),
        });
    }
    out.sort_by_key(|s| (s.start, s.len, s.run.y, s.run.x));
    out
}

/// A complete SVG document. More than one frame loops at `fps`.
pub fn document(frames: &[Buffer], fps: f32) -> String {
    let Some(first) = frames.first() else {
        return String::new();
    };
    let (w, h) = (
        first.area.width as f32 * CELL_W,
        first.area.height as f32 * CELL_H,
    );
    let fx::Rgb(r, g, b) = fx::BG;
    let n = frames.len();
    let all: Vec<Vec<Run>> = frames.iter().map(runs).collect();
    let spans = spans(&all);
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" data-frames="{n}">
<style>
text {{ font: 14px "SF Mono", Menlo, Monaco, "Cascadia Mono", "DejaVu Sans Mono", Consolas, monospace; white-space: pre; }}
"#
    );
    if n > 1 {
        // One keyframe rule per distinct length: visible for `len` frames
        // of the loop, starting at the group's animation delay.
        let total = n as f32 / fps;
        let _ = writeln!(out, "g {{ visibility: hidden; }}");
        let mut lens: Vec<usize> = spans.iter().map(|s| s.len).filter(|&l| l < n).collect();
        lens.sort_unstable();
        lens.dedup();
        for len in lens {
            let _ = writeln!(
                out,
                ".k{len} {{ animation: k{len} {total:.3}s steps(1) infinite; }} \
                 @keyframes k{len} {{ 0% {{ visibility: visible; }} {:.4}% {{ visibility: hidden; }} }}",
                100.0 * len as f32 / n as f32
            );
        }
    }
    let _ = writeln!(
        out,
        "</style>\n<rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>"
    );
    // Runs shown in every frame need no animation.
    for s in spans.iter().filter(|s| s.len == n) {
        text_element(&mut out, &s.run);
    }
    let mut group: Option<(usize, usize)> = None;
    for s in spans.iter().filter(|s| s.len < n) {
        if group != Some((s.start, s.len)) {
            if group.is_some() {
                out.push_str("</g>\n");
            }
            let delay = s.start as f32 / fps;
            let _ = writeln!(
                out,
                r#"<g class="k{}" style="animation-delay:{delay:.3}s">"#,
                s.len
            );
            group = Some((s.start, s.len));
        }
        text_element(&mut out, &s.run);
    }
    if group.is_some() {
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    fn buf_with(text: &str, x: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        buf.set_string(0, 0, "static", Style::default().fg(Color::Rgb(1, 2, 3)));
        buf.set_string(x, 1, text, Style::default().fg(Color::Rgb(255, 0, 16)));
        buf
    }

    #[test]
    fn exports_escaped_colored_runs() {
        let buf = buf_with("a<b", 1);
        let svg = document(std::slice::from_ref(&buf), 5.0);
        assert!(svg.contains(r##"fill="#ff0010""##));
        assert!(svg.contains("a&lt;b"));
        assert!(!svg.contains("animation"), "single frame is static");
        assert!(svg.contains(r#"data-frames="1""#));
        // Identical frames need no animation either.
        assert!(!document(&[buf.clone(), buf], 5.0).contains("animation"));
    }

    #[test]
    fn runs_that_stay_put_are_written_once() {
        let frames: Vec<Buffer> = (0..10).map(|i| buf_with("moving", i)).collect();
        let svg = document(&frames, 5.0);
        assert_eq!(svg.matches(">static</text>").count(), 1);
        assert_eq!(svg.matches(">moving</text>").count(), 10);
        assert!(svg.contains(r#"data-frames="10""#));
        assert!(svg.contains("animation-delay:0.200s"));
        assert!(svg.contains("animation: k1 2.000s"));
    }

    /// Decoding the spans gives back every frame exactly.
    #[test]
    fn spans_rebuild_every_frame() {
        let texts = ["a", "b", "a", "a", "c", "b", "b", "a"];
        let frames: Vec<Vec<Run>> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| runs(&buf_with(t, (i % 3) as u16)))
            .collect();
        let spans = spans(&frames);
        for (i, want) in frames.iter().enumerate() {
            let mut got: Vec<&Run> = spans
                .iter()
                .filter(|s| s.start <= i && i < s.start + s.len)
                .map(|s| &s.run)
                .collect();
            got.sort_by_key(|r| (r.y, r.x));
            let want: Vec<&Run> = want.iter().collect();
            assert_eq!(got, want, "frame {i}");
        }
    }

    #[test]
    fn document_is_well_formed() {
        let frames: Vec<Buffer> = (0..6).map(|i| buf_with("x&y", i)).collect();
        let svg = document(&frames, 5.0);
        assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>\n"));
        assert_eq!(svg.matches("<g ").count(), svg.matches("</g>").count());
        assert_eq!(
            svg.matches("<text ").count(),
            svg.matches("</text>").count()
        );
        assert!(!svg.contains("x&y"), "ampersand escaped");
    }
}
