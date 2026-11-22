//! Token prices, used to estimate what a session cost.
//!
//! The rest of the app only calls [`cost`] with a model id and a token count;
//! it does not know which vendor a model belongs to. The built-in table holds
//! model names as data. `--prices FILE` adds or replaces entries.
//!
//! Every figure is an estimate: discounts, batch pricing, fast mode, data
//! residency and long-context tiers are not visible in the token counts the
//! app receives, so they are not applied.

use crate::event::Tokens;
use std::path::Path;
use std::sync::OnceLock;

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 5-minute cache writes.
    pub cache_write: f64,
    /// 1-hour cache writes.
    pub cache_write_1h: f64,
}

const fn r(
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    cache_write_1h: f64,
) -> Rates {
    Rates {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h,
    }
}

/// Claude API list prices, taken on 2026-11-22 from
/// <https://platform.claude.com/docs/en/about-claude/pricing> (the same figures
/// as the Anthropic `claude-api` reference). Columns: base input, output,
/// cache hits, 5-minute cache writes (1.25x base input), 1-hour cache writes
/// (2x base input; the table lists 2x for every row).
///
/// Claude Haiku 5.5 uses its rates for prompts up to 100,000 tokens. Longer
/// prompts are billed at 5x those rates per request; token totals do not show
/// per-request prompt size, so that tier is not applied.
///
/// Keys are model families. A key matches the id itself and dated or
/// versioned snapshots of it (`claude-sonnet-4-5-20250929`,
/// `claude-opus-4-5@20251101`, `...-v1:0`), but not a newer release of the
/// same line: `claude-opus-5` does not match `claude-opus-5-7`.
const BUILTIN: &[(&str, Rates)] = &[
    ("claude-fable-5-2", r(8.0, 40.0, 0.20, 10.0, 16.0)),
    ("claude-mythos-5-2", r(8.0, 40.0, 0.20, 10.0, 16.0)),
    ("claude-fable-5-1", r(10.0, 50.0, 0.25, 12.50, 20.0)),
    ("claude-mythos-5-1", r(10.0, 50.0, 0.25, 12.50, 20.0)),
    ("claude-fable-5", r(10.0, 50.0, 1.0, 12.50, 20.0)),
    ("claude-mythos-5", r(10.0, 50.0, 1.0, 12.50, 20.0)),
    ("claude-opus-5-5", r(4.0, 20.0, 0.20, 5.0, 8.0)),
    ("claude-opus-5", r(5.0, 25.0, 0.50, 6.25, 10.0)),
    ("claude-opus-4-8", r(5.0, 25.0, 0.50, 6.25, 10.0)),
    ("claude-opus-4-7", r(5.0, 25.0, 0.50, 6.25, 10.0)),
    ("claude-opus-4-6", r(5.0, 25.0, 0.50, 6.25, 10.0)),
    ("claude-opus-4-5", r(5.0, 25.0, 0.50, 6.25, 10.0)),
    ("claude-opus-4-1", r(15.0, 75.0, 1.50, 18.75, 30.0)),
    ("claude-opus-4", r(15.0, 75.0, 1.50, 18.75, 30.0)),
    ("claude-sonnet-5-5", r(2.0, 10.0, 0.10, 2.50, 4.0)),
    ("claude-sonnet-5", r(2.0, 10.0, 0.20, 2.50, 4.0)),
    ("claude-sonnet-4-6", r(3.0, 15.0, 0.30, 3.75, 6.0)),
    ("claude-sonnet-4-5", r(3.0, 15.0, 0.30, 3.75, 6.0)),
    ("claude-sonnet-4", r(3.0, 15.0, 0.30, 3.75, 6.0)),
    ("claude-haiku-5-5", r(0.10, 0.50, 0.01, 0.125, 0.2)),
    ("claude-haiku-4-5", r(1.0, 5.0, 0.10, 1.25, 2.0)),
];

static OVERRIDES: OnceLock<Vec<(String, Rates)>> = OnceLock::new();

/// Load a `--prices` file. Call once, before any cost is computed.
pub fn load(path: &Path) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let table = parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    // A second call keeps the first table; main calls this once.
    let _ = OVERRIDES.set(table);
    Ok(())
}

/// Parse `{"model-prefix": {"input": 3.0, "output": 15.0, "cache_read": 0.3,
/// "cache_write": 3.75, "cache_write_1h": 6.0}, ...}`. The first four rates
/// are required, in USD per million tokens. `cache_write_1h` is optional and
/// defaults to that entry's `cache_write`, so files written before it existed
/// price 1-hour writes as they always did.
pub fn parse(text: &str) -> anyhow::Result<Vec<(String, Rates)>> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| anyhow::anyhow!("not valid JSON: {e}"))?;
    let obj = json
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected an object of model prefixes"))?;
    let mut out = Vec::new();
    for (model, v) in obj {
        anyhow::ensure!(!model.is_empty(), "empty model prefix");
        let rate = |k: &str| -> anyhow::Result<f64> {
            let n = v
                .get(k)
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| anyhow::anyhow!("{model}: missing number \"{k}\""))?;
            anyhow::ensure!(n.is_finite() && n >= 0.0, "{model}: \"{k}\" must be >= 0");
            Ok(n)
        };
        let cache_write = rate("cache_write")?;
        let cache_write_1h = if v.get("cache_write_1h").is_some() {
            rate("cache_write_1h")?
        } else {
            cache_write
        };
        out.push((
            model.clone(),
            Rates {
                input: rate("input")?,
                output: rate("output")?,
                cache_read: rate("cache_read")?,
                cache_write,
                cache_write_1h,
            },
        ));
    }
    Ok(out)
}

/// Estimated cost in USD, or None when the model has no known price.
pub fn cost(model: &str, t: &Tokens) -> Option<f64> {
    let overrides = OVERRIDES.get().map_or(&[][..], Vec::as_slice);
    rates_in(overrides, model).map(|r| apply(&r, t))
}

fn apply(r: &Rates, t: &Tokens) -> f64 {
    (t.input as f64 * r.input
        + t.output as f64 * r.output
        + t.cache_read as f64 * r.cache_read
        + t.cache_write_5m as f64 * r.cache_write
        + t.cache_write_1h as f64 * r.cache_write_1h)
        / 1e6
}

/// The most specific (longest) matching key wins, across both tables; on a tie
/// the override wins. Built-in keys match a model family and its snapshots
/// only. Override keys are prefixes, so `claude-opus-5` in a `--prices` file
/// also covers a later `claude-opus-5-7`, but not `claude-opus-5-5`, which
/// has a longer built-in key of its own.
fn rates_in(overrides: &[(String, Rates)], model: &str) -> Option<Rates> {
    if model.is_empty() {
        return None;
    }
    let mut best: Option<(usize, Rates)> = None;
    for (k, r) in BUILTIN {
        if family_match(k, model) && best.is_none_or(|(n, _)| k.len() > n) {
            best = Some((k.len(), *r));
        }
    }
    for (k, r) in overrides {
        if prefix_match(k, model) && best.is_none_or(|(n, _)| k.len() >= n) {
            best = Some((k.len(), *r));
        }
    }
    best.map(|(_, r)| r)
}

/// Where `key` starts in `model`: at the beginning or right after a provider
/// prefix (`us.anthropic.`, `anthropic/`). Case-insensitive.
fn key_end(key: &str, model: &str) -> Option<usize> {
    let key = key.to_ascii_lowercase();
    let at = model.find(&key)?;
    let before_ok = at == 0 || matches!(model.as_bytes()[at - 1], b'.' | b'/' | b':');
    before_ok.then_some(at + key.len())
}

fn prefix_match(key: &str, model: &str) -> bool {
    key_end(key, &model.to_ascii_lowercase()).is_some()
}

/// `key` names `model`, possibly behind a provider prefix and followed by a
/// snapshot suffix.
fn family_match(key: &str, model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    key_end(key, &model).is_some_and(|end| snapshot_suffix(&model[end..]))
}

/// Accepts what follows a model family when it only pins a snapshot: nothing,
/// a date (`-20250929`), a version tag (`-v1:0`), `@date` or `[1m]`.
/// Rejects `-5`, which would be a newer model of the same line.
fn snapshot_suffix(rest: &str) -> bool {
    if rest.is_empty() || rest.starts_with(['@', '[', ':']) {
        return true;
    }
    let Some(seg) = rest.strip_prefix('-') else {
        return false;
    };
    let seg = seg.split(['-', ':', '[']).next().unwrap_or("");
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(seg) && seg.len() >= 6) || seg.strip_prefix('v').is_some_and(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rates(model: &str) -> Option<Rates> {
        rates_in(&[], model)
    }

    #[test]
    fn dated_and_versioned_ids_resolve_to_their_family() {
        let sonnet45 = rates("claude-sonnet-4-5").unwrap();
        for id in [
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-5@20250929",
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "claude-sonnet-4-5[1m]",
        ] {
            assert_eq!(rates(id), Some(sonnet45), "{id}");
        }
        assert_eq!(rates("claude-opus-4-20250514").unwrap().input, 15.0);
        assert_eq!(rates("claude-opus-4-1-20250805").unwrap().input, 15.0);
        assert_eq!(rates("claude-opus-4-5-20251101").unwrap().input, 5.0);
    }

    #[test]
    fn a_family_does_not_swallow_newer_releases() {
        // Longest key wins: opus-5-5 is not priced as opus-5.
        assert_eq!(rates("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(rates("claude-opus-5").unwrap().input, 5.0);
        // An unreleased version must not inherit an older price.
        assert_eq!(rates("claude-opus-5-7"), None);
        assert_eq!(rates("claude-opus-4-9"), None);
    }

    #[test]
    fn unknown_models_have_no_price() {
        for id in [
            "",
            "demo-model",
            "gpt-5",
            "claude-3-5-haiku-20241022",
            "xclaude-opus-5",
        ] {
            assert_eq!(rates(id), None, "{id}");
            assert_eq!(cost(id, &Tokens::default()), None);
        }
    }

    #[test]
    fn cost_adds_the_five_rates() {
        let t = Tokens {
            input: 1_000_000,
            output: 100_000,
            cache_read: 2_000_000,
            cache_write_5m: 400_000,
            cache_write_1h: 100_000,
        };
        // Sonnet 4.6: 3 + 1.5 + 0.6 + 1.5 + 0.6
        let usd = cost("claude-sonnet-4-6-20260101", &t).unwrap();
        assert!((usd - 7.2).abs() < 1e-9, "{usd}");
        assert_eq!(cost("claude-opus-5-5", &Tokens::default()), Some(0.0));
    }

    #[test]
    fn override_file_parses_and_wins() {
        let table = parse(
            r#"{"claude-sonnet-4-5": {"input": 1, "output": 2, "cache_read": 0.5, "cache_write": 4},
                "local-": {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0}}"#,
        )
        .unwrap();
        assert_eq!(table.len(), 2);
        let got = rates_in(&table, "claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(got, r(1.0, 2.0, 0.5, 4.0, 4.0));
        assert_eq!(rates_in(&table, "local-llama").unwrap().input, 0.0);
        // Entries not overridden keep the built-in price.
        assert_eq!(rates_in(&table, "claude-opus-5-5").unwrap().input, 4.0);
    }

    #[test]
    fn a_short_override_does_not_hide_a_more_specific_built_in_price() {
        let table = vec![("claude-opus-5".to_string(), r(99.0, 99.0, 99.0, 99.0, 99.0))];
        assert_eq!(rates_in(&table, "claude-opus-5").unwrap().input, 99.0);
        assert_eq!(rates_in(&table, "claude-opus-5-5").unwrap().input, 4.0);
        // No built-in key for this one, so the prefix the user gave applies.
        assert_eq!(rates_in(&table, "claude-opus-5-7").unwrap().input, 99.0);
    }

    #[test]
    fn overrides_apply_behind_a_provider_prefix() {
        let table = vec![("claude-sonnet-4-5".to_string(), r(1.0, 1.0, 1.0, 1.0, 1.0))];
        let id = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
        assert_eq!(rates_in(&table, id).unwrap().input, 1.0);
    }

    #[test]
    fn longest_override_prefix_wins() {
        let table = parse(
            r#"{"m": {"input": 1, "output": 1, "cache_read": 1, "cache_write": 1},
                "m-big": {"input": 9, "output": 9, "cache_read": 9, "cache_write": 9}}"#,
        )
        .unwrap();
        assert_eq!(rates_in(&table, "m-big-2").unwrap().input, 9.0);
        assert_eq!(rates_in(&table, "m-small").unwrap().input, 1.0);
    }

    #[test]
    fn one_hour_rate_is_optional_and_defaults_to_the_five_minute_rate() {
        let table = parse(
            r#"{"old": {"input": 1, "output": 2, "cache_read": 0.5, "cache_write": 4},
                "new": {"input": 1, "output": 2, "cache_read": 0.5, "cache_write": 4, "cache_write_1h": 7}}"#,
        )
        .unwrap();
        assert_eq!(rates_in(&table, "old-x").unwrap().cache_write_1h, 4.0);
        let new = rates_in(&table, "new-x").unwrap();
        assert_eq!((new.cache_write, new.cache_write_1h), (4.0, 7.0));
    }

    #[test]
    fn built_in_one_hour_rate_is_twice_the_input_rate() {
        for (k, r) in BUILTIN {
            assert!((r.cache_write_1h - 2.0 * r.input).abs() < 1e-9, "{k}");
        }
    }

    #[test]
    fn bad_override_files_are_rejected() {
        for bad in [
            "not json",
            "[1, 2]",
            r#"{"m": {"input": 1, "output": 1, "cache_read": 1}}"#,
            r#"{"m": {"input": -1, "output": 1, "cache_read": 1, "cache_write": 1}}"#,
            r#"{"m": {"input": "3", "output": 1, "cache_read": 1, "cache_write": 1}}"#,
            r#"{"": {"input": 1, "output": 1, "cache_read": 1, "cache_write": 1}}"#,
            r#"{"m": {"input": 1, "output": 1, "cache_read": 1, "cache_write": 1, "cache_write_1h": -2}}"#,
            r#"{"m": {"input": 1, "output": 1, "cache_read": 1, "cache_write": 1, "cache_write_1h": "6"}}"#,
            r#"{"m": {"input": 1, "output": 1, "cache_read": 1, "cache_write": 1, "cache_write_1h": null}}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
