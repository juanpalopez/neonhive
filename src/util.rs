use chrono::{DateTime, Local, TimeZone};
use unicode_width::UnicodeWidthChar;

/// Collapse whitespace, replace characters that are not one cell wide, and
/// truncate to `max` characters with an ellipsis.
pub fn clean(s: &str, max: usize) -> String {
    let mut out = String::new();
    if max == 0 {
        return out;
    }
    let mut n = 0;
    let mut last_space = true;
    for ch in s.chars() {
        let c = if ch.is_whitespace() || ch.is_control() {
            ' '
        } else if ch.width() == Some(1) {
            ch
        } else {
            '·'
        };
        if c == ' ' && last_space {
            continue;
        }
        last_space = c == ' ';
        if n >= max {
            out.pop();
            out.push('…');
            break;
        }
        out.push(c);
        n += 1;
    }
    out.truncate(out.trim_end().len());
    out
}

pub fn hash(a: u32, b: u32, c: u32) -> u32 {
    let mut x =
        a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B) ^ c.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x
}

pub fn str_hash(s: &str) -> u32 {
    s.bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

pub fn parse_ts(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn fmt_clock(ts: i64) -> String {
    match Local.timestamp_millis_opt(ts).single() {
        Some(d) => d.format("%H:%M:%S").to_string(),
        None => "--:--:--".into(),
    }
}

pub fn fmt_date(ts: i64) -> String {
    match Local.timestamp_millis_opt(ts).single() {
        Some(d) => d.format("%Y-%m-%d %H:%M").to_string(),
        None => "-".into(),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub fn fmt_dur(ms: i64) -> String {
    let s = (ms.max(0)) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_collapses_truncates_and_replaces_wide_chars() {
        assert_eq!(clean("  a \n\t b  ", 10), "a b");
        assert_eq!(clean("abcdefgh", 5), "abcd…");
        assert_eq!(clean("abcde", 5), "abcde");
        assert_eq!(clean("日本", 10), "··");
        assert_eq!(clean("anything", 0), "");
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(45_867), "45.9k");
        assert_eq!(fmt_tokens(2_500_000), "2.5M");
        assert_eq!(fmt_dur(65_000), "01:05");
        assert_eq!(fmt_dur(3_725_000), "1:02:05");
        assert_eq!(fmt_dur(-5), "00:00");
    }

    #[test]
    fn parses_transcript_timestamps() {
        assert_eq!(parse_ts("1970-01-01T00:00:01.500Z"), Some(1_500));
        assert_eq!(parse_ts("not a date"), None);
    }
}
