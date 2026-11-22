//! SVG export of rendered frames, for screenshots and documentation.
//! Several frames become one looping, CSS-animated SVG.

use crate::fx;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
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

/// One frame as SVG text elements: one element per run of same-styled cells.
fn frame(buf: &Buffer) -> String {
    let mut out = String::new();
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
                match c.symbol() {
                    "&" => text.push_str("&amp;"),
                    "<" => text.push_str("&lt;"),
                    ">" => text.push_str("&gt;"),
                    s => text.push_str(s),
                }
                x += 1;
            }
            let cells = text.trim_end().chars().count().max(1);
            // textLength pins every run to the cell grid, whatever the font.
            let _ = writeln!(
                out,
                r#"<text x="{:.1}" y="{:.1}" fill="{}" textLength="{:.1}" lengthAdjust="spacingAndGlyphs"{}>{}</text>"#,
                start as f32 * CELL_W,
                (y as f32 + 0.75) * CELL_H,
                hex(color),
                cells as f32 * CELL_W,
                if bold { r#" font-weight="bold""# } else { "" },
                text.trim_end(),
            );
        }
    }
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
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}">
<style>
text {{ font: 14px "SF Mono", Menlo, Monaco, "Cascadia Mono", "DejaVu Sans Mono", Consolas, monospace; white-space: pre; }}
"#
    );
    if n > 1 {
        let total = n as f32 / fps;
        let _ = writeln!(
            out,
            ".f {{ visibility: hidden; animation: show {total:.2}s steps(1) infinite; }}\n\
             @keyframes show {{ 0% {{ visibility: visible; }} {:.3}% {{ visibility: hidden; }} }}",
            100.0 / n as f32
        );
    }
    let _ = writeln!(
        out,
        "</style>\n<rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>"
    );
    for (i, buf) in frames.iter().enumerate() {
        let delay = i as f32 / fps;
        let _ = writeln!(out, r#"<g class="f" style="animation-delay:{delay:.2}s">"#);
        out.push_str(&frame(buf));
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

    #[test]
    fn exports_escaped_colored_runs() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        buf.set_string(1, 0, "a<b", Style::default().fg(Color::Rgb(255, 0, 16)));
        let svg = document(&[buf.clone()], 5.0);
        assert!(svg.contains(r##"fill="#ff0010""##));
        assert!(svg.contains("a&lt;b"));
        assert!(!svg.contains("animation:"), "single frame is static");
        assert!(document(&[buf.clone(), buf], 5.0).contains("animation-delay:0.20s"));
    }
}
