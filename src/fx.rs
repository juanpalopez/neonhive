//! Palette and color math.

use ratatui::style::Color;
use std::sync::atomic::{AtomicBool, Ordering};

static TRUECOLOR: AtomicBool = AtomicBool::new(true);

pub fn set_truecolor(on: bool) {
    TRUECOLOR.store(on, Ordering::Relaxed);
}

pub fn truecolor() -> bool {
    TRUECOLOR.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn scale(self, f: f32) -> Rgb {
        let s = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
        Rgb(s(self.0), s(self.1), s(self.2))
    }

    pub fn mix(self, o: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        Rgb(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2))
    }

    /// Terminal color: 24-bit when supported, else nearest xterm-256 cube entry.
    pub fn c(self) -> Color {
        if truecolor() {
            Color::Rgb(self.0, self.1, self.2)
        } else {
            let q = |v: u8| (v as u16 * 5 / 255) as u8;
            Color::Indexed(16 + 36 * q(self.0) + 6 * q(self.1) + q(self.2))
        }
    }
}

pub const BG: Rgb = Rgb(7, 8, 16);
pub const GRID: Rgb = Rgb(26, 32, 56);
pub const DIM: Rgb = Rgb(58, 76, 108);
pub const TEXT: Rgb = Rgb(188, 208, 228);
pub const WHITE: Rgb = Rgb(240, 250, 255);
pub const CYAN: Rgb = Rgb(0, 240, 255);
pub const MAGENTA: Rgb = Rgb(255, 42, 190);
pub const YELLOW: Rgb = Rgb(252, 238, 10);
pub const GREEN: Rgb = Rgb(57, 255, 140);
pub const RED: Rgb = Rgb(255, 56, 80);
pub const ORANGE: Rgb = Rgb(255, 150, 40);
pub const VIOLET: Rgb = Rgb(160, 110, 255);

/// Stable color per agent type.
pub fn type_color(agent_type: &str) -> Rgb {
    const P: [Rgb; 5] = [MAGENTA, YELLOW, GREEN, ORANGE, VIOLET];
    P[(crate::util::str_hash(agent_type) % P.len() as u32) as usize]
}
