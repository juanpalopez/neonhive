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
use std::collections::{HashMap, HashSet};
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
            let escaped = escape(text);
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

/// `text` as XML character data. Markup characters are escaped; anything
/// XML 1.0 does not allow in a document (most control characters, U+FFFE,
/// U+FFFF) becomes U+FFFD, and tab, newline and return become a space so
/// the run keeps its cells.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(' '),
            '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}' => {
                out.push(ch);
            }
            _ => out.push('\u{FFFD}'),
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

/// An SVG being built one frame at a time. Each frame is cut into runs as
/// it is pushed and only the runs are kept: a run still showing stays open,
/// a run that is gone becomes a `Span`.
#[derive(Default)]
pub struct Animation {
    /// Size in cells, from the first frame.
    size: (u16, u16),
    frames: usize,
    open: HashMap<Run, usize>,
    spans: Vec<Span>,
}

impl Animation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, buf: &Buffer) {
        if self.frames == 0 {
            self.size = (buf.area.width, buf.area.height);
        }
        let i = self.frames;
        let now: HashSet<Run> = runs(buf).into_iter().collect();
        let mut closed = Vec::new();
        self.open.retain(|run, start| {
            let keep = now.contains(run);
            if !keep {
                closed.push(Span {
                    start: *start,
                    len: i - *start,
                    run: run.clone(),
                });
            }
            keep
        });
        self.spans.extend(closed);
        for run in now {
            self.open.entry(run).or_insert(i);
        }
        self.frames += 1;
    }

    /// Every run with the unbroken stretches of frames it shows in, ordered
    /// by start, length and position so the output does not depend on
    /// hashing.
    fn into_spans(self) -> (usize, (u16, u16), Vec<Span>) {
        let n = self.frames;
        let mut spans = self.spans;
        for (run, start) in self.open {
            spans.push(Span {
                start,
                len: n - start,
                run,
            });
        }
        spans.sort_by_key(|s| (s.start, s.len, s.run.y, s.run.x));
        (n, self.size, spans)
    }

    /// The complete SVG document. More than one frame loops at `fps`.
    pub fn finish(self, fps: f32) -> String {
        let (n, size, spans) = self.into_spans();
        if n == 0 {
            return String::new();
        }
        let (w, h) = (size.0 as f32 * CELL_W, size.1 as f32 * CELL_H);
        let fx::Rgb(r, g, b) = fx::BG;
        let mut out = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" data-frames="{n}">
<style>
text {{ font: 14px "SF Mono", Menlo, Monaco, "Cascadia Mono", "DejaVu Sans Mono", Consolas, monospace; white-space: pre; }}
"#
        );
        if n > 1 {
            // One keyframe rule per distinct length: visible for `len`
            // frames of the loop, starting at the group's animation delay.
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
}

/// A complete SVG document of `frames`. More than one frame loops at `fps`.
#[cfg(test)]
pub fn document(frames: &[Buffer], fps: f32) -> String {
    let mut anim = Animation::new();
    for f in frames {
        anim.push(f);
    }
    anim.finish(fps)
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

    /// Frames whose second row changes in text and place, with runs that
    /// come back after a gap, and first and last frames that differ.
    fn varied() -> Vec<Buffer> {
        let texts = ["a", "b", "a", "a", "c", "b", "b", "a", "d"];
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| buf_with(t, (i % 3) as u16))
            .collect()
    }

    /// Decoding the spans gives back every frame exactly.
    #[test]
    fn spans_rebuild_every_frame() {
        let bufs = varied();
        let frames: Vec<Vec<Run>> = bufs.iter().map(runs).collect();
        let mut anim = Animation::new();
        for b in &bufs {
            anim.push(b);
        }
        let (_, _, spans) = anim.into_spans();
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

    /// Text between `start` and the next `end` after it.
    fn between<'a>(s: &'a str, start: &str, end: &str) -> &'a str {
        let from = s.find(start).unwrap() + start.len();
        let len = s[from..].find(end).unwrap();
        &s[from..from + len]
    }

    /// x, y and text of one text element, as written.
    type Cell = (String, String, String);

    /// Reads the CSS back the way a browser would and checks, at the middle
    /// of every frame of two loops, that exactly that frame's text shows.
    #[test]
    fn css_shows_each_frame_at_its_time() {
        let bufs = varied();
        let n = bufs.len();
        for fps in [1.0f32, 3.0, 5.0, 30.0] {
            let svg = document(&bufs, fps);
            // len -> (loop seconds, visible share in percent)
            let mut keys: HashMap<usize, (f32, f32)> = HashMap::new();
            // (delay and len of the group, or None when static, x, y, text)
            let mut texts: Vec<(Option<(f32, usize)>, Cell)> = Vec::new();
            let mut group = None;
            for line in svg.lines() {
                if let Some(rest) = line.strip_prefix(".k") {
                    let len: usize = rest[..rest.find(' ').unwrap()].parse().unwrap();
                    let total: f32 = between(line, &format!("animation: k{len} "), "s ")
                        .parse()
                        .unwrap();
                    let pct: f32 = between(line, "visible; } ", "%").parse().unwrap();
                    keys.insert(len, (total, pct));
                } else if line.starts_with("<g class=\"k") {
                    let len = between(line, "class=\"k", "\"").parse().unwrap();
                    let delay = between(line, "animation-delay:", "s\"").parse().unwrap();
                    group = Some((delay, len));
                } else if line == "</g>" {
                    group = None;
                } else if line.starts_with("<text ") {
                    texts.push((
                        group,
                        (
                            between(line, "x=\"", "\"").to_string(),
                            between(line, "y=\"", "\"").to_string(),
                            between(line, ">", "</text>").to_string(),
                        ),
                    ));
                }
            }
            for k in 0..2 * n {
                let t = (k as f32 + 0.5) / fps;
                let mut shown: Vec<Cell> = texts
                    .iter()
                    .filter(|(g, ..)| match g {
                        None => true,
                        Some((delay, len)) => {
                            let (total, pct) = keys[len];
                            t >= *delay && (t - delay) % total < total * pct / 100.0
                        }
                    })
                    .map(|(_, c)| c.clone())
                    .collect();
                shown.sort();
                let mut want: Vec<Cell> = runs(&bufs[k % n])
                    .iter()
                    .map(|r| {
                        (
                            format!("{:.1}", r.x as f32 * CELL_W),
                            format!("{:.1}", (r.y as f32 + 0.75) * CELL_H),
                            r.text.clone(),
                        )
                    })
                    .collect();
                want.sort();
                assert_eq!(shown, want, "fps {fps}, frame {k}");
            }
        }
    }

    #[test]
    fn characters_xml_forbids_are_replaced() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 1));
        for (x, sym) in ["a", "\u{FFFE}", "\u{1}", "\u{FFFF}", "\u{1B}", "b", "\t"]
            .iter()
            .enumerate()
        {
            buf[(x as u16, 0)].set_symbol(sym);
        }
        let svg = document(std::slice::from_ref(&buf), 5.0);
        for bad in ['\u{FFFE}', '\u{FFFF}', '\u{1}', '\u{1B}', '\t'] {
            assert!(!svg.contains(bad), "{bad:?} left in");
        }
        assert!(svg.contains("a\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}b"), "{svg}");
        // Every char is allowed by the XML 1.0 Char production.
        assert!(svg.chars().all(|c| matches!(c,
            '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')));
    }
}
