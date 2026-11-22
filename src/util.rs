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

/// One char, case-folded. Only the first char of a multi-char lowercase form is
/// kept, so folding never changes a string's length in chars and match
/// positions in the folded text are positions in the original.
fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// A search query, case-folded once.
pub fn needle(query: &str) -> Vec<char> {
    query.chars().map(fold).collect()
}

/// Case-insensitive substring search. Returns the non-overlapping matches of
/// `needle` in `hay` as char ranges `start..end`. Ranges are whole chars, so a
/// match never splits a wide character.
pub fn find_ci(hay: &str, needle: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let hay: Vec<char> = hay.chars().map(fold).collect();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Whether `hay` contains `needle`, ignoring case. See [`find_ci`].
pub fn contains_ci(hay: &str, needle: &[char]) -> bool {
    let Some(&first) = needle.first() else {
        return false;
    };
    // No allocation: this runs over every stream line each frame.
    hay.char_indices().any(|(i, c)| {
        fold(c) == first && {
            let mut rest = hay[i + c.len_utf8()..].chars();
            needle[1..]
                .iter()
                .all(|n| rest.next().map(fold) == Some(*n))
        }
    })
}

/// The tail of `s` that fits in `width` display cells.
pub fn tail_fit(s: &str, width: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices().rev() {
        used += c.width().unwrap_or(0);
        if used > width {
            return &s[i + c.len_utf8()..];
        }
    }
    s
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

/// 64-bit FNV-1a over `parts`, each followed by a NUL so that `["ab", "c"]`
/// and `["a", "bc"]` differ. Stable across runs and platforms, unlike the std
/// hasher, so a replay computes the same values every time.
pub fn str_hash64(parts: &[&str]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        for b in p.bytes().chain([0]) {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
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

/// An estimated amount in USD, prefixed with `~`.
pub fn fmt_usd(usd: f64) -> String {
    if usd >= 1_000.0 {
        format!("~${usd:.0}")
    } else if usd >= 100.0 {
        format!("~${usd:.1}")
    } else {
        format!("~${usd:.2}")
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
        assert_eq!(fmt_usd(1.236), "~$1.24");
        assert_eq!(fmt_usd(0.0), "~$0.00");
        assert_eq!(fmt_usd(123.45), "~$123.5");
        assert_eq!(fmt_usd(4_321.0), "~$4321");
    }

    #[test]
    fn str_hash64_is_stable_and_separates_parts() {
        // FNV-1a 64 of the empty input is its offset basis.
        assert_eq!(str_hash64(&[]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(str_hash64(&["a", "b"]), str_hash64(&["a", "b"]));
        assert_ne!(str_hash64(&["ab", "c"]), str_hash64(&["a", "bc"]));
    }

    #[test]
    fn case_insensitive_search_keeps_char_positions() {
        let n = needle("READ");
        assert_eq!(find_ci("Read: a.rs, read b", &n), vec![(0, 4), (12, 16)]);
        assert!(contains_ci("xxreadxx", &n));
        assert!(!contains_ci("rea", &n));
        assert!(find_ci("anything", &[]).is_empty());
        // Wide characters count as one char each: ranges never split them.
        assert_eq!(find_ci("日本語の本", &needle("本")), vec![(1, 2), (4, 5)]);
        // A char whose lowercase form is two chars still folds to one.
        assert_eq!(find_ci("İx", &needle("x")), vec![(1, 2)]);
        assert_eq!(find_ci("aaaa", &needle("aa")), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn tail_fit_counts_display_width() {
        assert_eq!(tail_fit("abcdef", 3), "def");
        assert_eq!(tail_fit("abc", 10), "abc");
        assert_eq!(tail_fit("日本語", 4), "本語");
        assert_eq!(tail_fit("日本語", 3), "語");
        assert_eq!(tail_fit("x", 0), "");
    }

    #[test]
    fn parses_transcript_timestamps() {
        assert_eq!(parse_ts("1970-01-01T00:00:01.500Z"), Some(1_500));
        assert_eq!(parse_ts("not a date"), None);
    }
}
