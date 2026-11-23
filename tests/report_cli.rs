//! `neonhive report` and `neonhive watch --once` run as a user would, on
//! small synthetic transcripts in a scratch directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory, removed on drop. It is also `HOME`, so no real
/// session is ever listed.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("neonhive-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_neonhive"))
            .args(args)
            .env("HOME", &self.0)
            .env("CODEX_HOME", self.0.join(".codex"))
            .output()
            .unwrap()
    }
    fn report(&self, path: &Path, extra: &[&str]) -> Output {
        let mut args = vec!["report", path.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.run(&args)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json(out: &Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

const USER: &str = r#"{"type":"user","timestamp":"2026-11-01T10:00:00.000Z","message":{"role":"user","content":"hi"}}"#;
/// One million input tokens of a priced model: $3 at list price.
const ASSISTANT: &str = r#"{"type":"assistant","timestamp":"2026-11-01T10:00:30.000Z","message":{"id":"m1","model":"claude-sonnet-4-5","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1000000,"output_tokens":0}}}"#;

fn session() -> String {
    format!("{USER}\n{ASSISTANT}\n")
}

#[test]
fn input_with_no_events_fails_with_a_message() {
    let s = Scratch::new("empty");
    let cases = [
        s.file("empty.jsonl", b""),
        s.file("garbage.jsonl", b"not json\n{]\n"),
        // A zstd magic number followed by junk.
        s.file("corrupt.jsonl.zst", b"\x28\xb5\x2f\xfd\x00\x00junk"),
    ];
    for path in &cases {
        for extra in [&[][..], &["--json"][..]] {
            let out = s.report(path, extra);
            assert!(!out.status.success(), "{}", path.display());
            assert!(out.stdout.is_empty(), "no report on stdout");
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(err.contains("no session events"), "{err}");
        }
    }
}

#[test]
fn a_prompt_alone_is_a_valid_report() {
    let s = Scratch::new("prompt");
    let path = s.file("s.jsonl", format!("{USER}\n").as_bytes());
    let v = json(&s.report(&path, &["--json"]));
    assert_eq!(v["duration_ms"], 0);
    assert!(v["start_ms"].as_i64().unwrap() > 1_700_000_000_000);
}

#[test]
fn the_last_line_counts_without_a_trailing_newline() {
    let s = Scratch::new("nonl");
    let path = s.file("s.jsonl", session().trim_end().as_bytes());
    let v = json(&s.report(&path, &["--json"]));
    assert_eq!(v["tokens"]["input"], 1_000_000);
    assert_eq!(v["duration_ms"], 30_000);
}

#[test]
fn prices_flag_changes_the_report_cost() {
    let s = Scratch::new("prices");
    let path = s.file("s.jsonl", session().as_bytes());
    let list = json(&s.report(&path, &["--json"]));
    assert_eq!(list["cost"]["usd"], 3.0);
    let prices = s.file(
        "prices.json",
        br#"{"claude-sonnet-4-5": {"input": 10, "output": 0, "cache_read": 0, "cache_write": 0}}"#,
    );
    let p = prices.to_str().unwrap();
    let custom = json(&s.report(&path, &["--json", "--prices", p]));
    assert_eq!(custom["cost"]["usd"], 10.0);
    assert_eq!(custom["agents"][0]["cost_usd"], 10.0);
    let md = s.report(&path, &["--prices", p]);
    let md = String::from_utf8_lossy(&md.stdout);
    assert!(md.contains("- **Estimated cost:** ~$10.00\n"), "{md}");
}

#[test]
fn report_cost_equals_the_watch_cost_alert() {
    let s = Scratch::new("watch-cost");
    let path = s.file("s.jsonl", session().as_bytes());
    let v = json(&s.report(&path, &["--json"]));
    let out = s.run(&[
        "watch",
        path.to_str().unwrap(),
        "--once",
        "--json",
        "--max-cost",
        "0.000001",
    ]);
    let line = String::from_utf8_lossy(&out.stdout);
    let alert: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(alert["kind"], "max_cost");
    assert_eq!(alert["value"], v["cost"]["usd"]);
}
