//! Session report: a static summary of a [`World`] as Markdown or JSON.
//!
//! Both renderers are pure. They read only what the world keeps: labels, tool
//! names, counts and timestamps. Prompts, messages and tool details are never
//! included, so a report is safer to share than a transcript. It can still
//! name files or tasks through labels, so it should be read before sharing.

use crate::event::{Tokens, ROOT};
use crate::state::{Agent, Status, World};
use crate::util::{clean, fmt_date, fmt_dur, fmt_tokens};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Version of the JSON layout. Bumped when a field is removed or changes meaning.
const SCHEMA: u32 = 1;
const TOP_TOOLS: usize = 10;

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Running => "running",
        Status::Done => "done",
        Status::Failed => "failed",
    }
}

/// Agents in spawn order with children directly under their parent.
fn tree(w: &World) -> Vec<(&Agent, usize)> {
    fn visit<'a>(
        w: &'a World,
        id: &str,
        depth: usize,
        seen: &mut HashSet<String>,
        out: &mut Vec<(&'a Agent, usize)>,
    ) {
        if !seen.insert(id.to_string()) {
            return;
        }
        let Some(a) = w.agents.get(id) else { return };
        out.push((a, depth));
        for child in &w.order {
            if w.agents[child].parent.as_deref() == Some(id) {
                visit(w, child, depth + 1, seen, out);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    visit(w, ROOT, 0, &mut seen, &mut out);
    // Anything not reachable from the root (a broken parent link) still counts.
    for id in &w.order {
        visit(w, id, 1, &mut seen, &mut out);
    }
    out
}

fn start_ms(w: &World) -> i64 {
    w.agents
        .values()
        .map(|a| a.spawned_ts)
        .filter(|t| *t > 0)
        .min()
        .unwrap_or(0)
}

/// When the agent started. An agent with no activity of its own (a root whose
/// transcript is empty) has no start time; the session start stands in.
fn agent_start_ms(w: &World, a: &Agent) -> i64 {
    match a.spawned_ts {
        0 => start_ms(w),
        t => t,
    }
}

fn duration_ms(w: &World, a: &Agent) -> i64 {
    (a.finished_ts.unwrap_or(w.now_ts) - agent_start_ms(w, a)).max(0)
}

fn total(w: &World) -> Tokens {
    let mut t = Tokens::default();
    for a in w.agents.values() {
        t.add(&a.tokens);
    }
    t
}

fn errors(w: &World) -> u64 {
    w.agents.values().map(|a| u64::from(a.errors)).sum()
}

fn name(a: &Agent) -> String {
    let n = if a.label.is_empty() {
        &a.agent_type
    } else {
        &a.label
    };
    clean(n, 60)
}

/// Tool names by use, most used first; ties by name so output is stable.
fn tools(w: &World) -> Vec<(&str, u64)> {
    let mut v: Vec<_> = w
        .tool_counts
        .iter()
        .map(|(k, n)| (k.as_str(), *n))
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v
}

fn failures(w: &World) -> Vec<&Agent> {
    tree(w)
        .into_iter()
        .map(|(a, _)| a)
        .filter(|a| a.status == Status::Failed || a.errors > 0)
        .collect()
}

/// Text that is safe inside a Markdown table cell or list item, on one line.
fn esc(s: &str) -> String {
    let mut out = String::new();
    for c in clean(s, 200).chars() {
        if matches!(c, '|' | '\\' | '`' | '<' | '>' | '*' | '_' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn markdown(w: &World) -> String {
    let rows = tree(w);
    let t = total(w);
    let start = start_ms(w);
    let mut o = String::new();

    o.push_str(&format!(
        "# Session report: {}\n\n",
        esc(&name(&w.agents[ROOT]))
    ));
    o.push_str(&format!("- **Started:** {}\n", fmt_date(start)));
    o.push_str(&format!(
        "- **Duration:** {}\n",
        fmt_dur((w.now_ts - start).max(0))
    ));
    o.push_str(&format!(
        "- **Agents:** {} ({} subagents)\n",
        rows.len(),
        rows.len().saturating_sub(1)
    ));
    o.push_str(&format!("- **Tool calls:** {}\n", w.total_tools));
    o.push_str(&format!("- **Tool errors:** {}\n", errors(w)));
    o.push_str(&format!("- **Tokens:** {}\n", fmt_tokens(w.total_tokens())));

    o.push_str("\n## Tokens\n\n| Kind | Count |\n| --- | ---: |\n");
    for (k, n) in [
        ("Input", t.input),
        ("Output", t.output),
        ("Cache write", t.cache_write),
        ("Cache read", t.cache_read),
    ] {
        o.push_str(&format!("| {k} | {n} |\n"));
    }

    o.push_str("\n## Agents\n\n");
    o.push_str("| Agent | Type | Parent | Status | Duration | Tokens | Tools | Errors |\n");
    o.push_str("| --- | --- | --- | --- | ---: | ---: | ---: | ---: |\n");
    for (a, depth) in &rows {
        let indent = match depth {
            0 => String::new(),
            d => format!("{}↳ ", "· ".repeat(d - 1)),
        };
        let parent = a
            .parent
            .as_ref()
            .and_then(|p| w.agents.get(p))
            // The root's name is the session title; it would repeat on every row.
            .map_or_else(
                || "-".into(),
                |p| {
                    if p.id == ROOT {
                        ROOT.into()
                    } else {
                        esc(&name(p))
                    }
                },
            );
        o.push_str(&format!(
            "| {indent}{} | {} | {parent} | {} | {} | {} | {} | {} |\n",
            esc(&name(a)),
            esc(&a.agent_type),
            status_name(a.status),
            fmt_dur(duration_ms(w, a)),
            fmt_tokens(a.shown_tokens()),
            a.tools,
            a.errors,
        ));
    }

    o.push_str("\n## Most used tools\n\n");
    let top = tools(w);
    if top.is_empty() {
        o.push_str("No tool calls.\n");
    } else {
        o.push_str("| Tool | Calls |\n| --- | ---: |\n");
        for (tool, n) in top.iter().take(TOP_TOOLS) {
            o.push_str(&format!("| {} | {n} |\n", esc(tool)));
        }
    }

    o.push_str("\n## Failures\n\n");
    let failed = failures(w);
    if failed.is_empty() {
        o.push_str("No failed agents and no tool errors.\n");
    }
    for a in failed {
        let mut why = Vec::new();
        if a.status == Status::Failed {
            why.push("failed".to_string());
        }
        if a.errors > 0 {
            let s = if a.errors == 1 { "" } else { "s" };
            why.push(format!("{} tool error{s}", a.errors));
        }
        o.push_str(&format!(
            "- **{}** ({}): {}\n",
            esc(&name(a)),
            esc(&a.agent_type),
            why.join(", ")
        ));
    }
    o
}

fn tokens_json(t: &Tokens, shown: u64) -> Value {
    json!({
        "input": t.input,
        "output": t.output,
        "cache_write": t.cache_write,
        "cache_read": t.cache_read,
        "total": shown,
    })
}

pub fn json(w: &World) -> String {
    let rows = tree(w);
    let agents: Vec<Value> = rows
        .iter()
        .map(|(a, depth)| {
            json!({
                "id": a.id,
                "parent": a.parent,
                "depth": depth,
                "type": a.agent_type,
                "label": clean(&a.label, 80),
                "status": status_name(a.status),
                "start_ms": agent_start_ms(w, a),
                "duration_ms": duration_ms(w, a),
                "tokens": tokens_json(&a.tokens, a.shown_tokens()),
                "models": a.by_model.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(),
                "tool_calls": a.tools,
                "errors": a.errors,
            })
        })
        .collect();
    let top: Vec<Value> = tools(w)
        .iter()
        .map(|(tool, n)| json!({ "tool": tool, "calls": n }))
        .collect();
    let failed: Vec<Value> = failures(w)
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "label": clean(&a.label, 80),
                "type": a.agent_type,
                "failed": a.status == Status::Failed,
                "errors": a.errors,
            })
        })
        .collect();
    let start = start_ms(w);
    let doc = json!({
        "schema": SCHEMA,
        "title": name(&w.agents[ROOT]),
        "start_ms": start,
        "duration_ms": (w.now_ts - start).max(0),
        "agent_count": rows.len(),
        "tool_calls": w.total_tools,
        "errors": errors(w),
        "tokens": tokens_json(&total(w), w.total_tokens()),
        "agents": agents,
        "tools": top,
        "failures": failed,
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, Kind};

    fn ev(ts: i64, kind: Kind) -> Event {
        Event { ts, kind }
    }
    fn spawn(id: &str, parent: &str, ty: &str, label: &str) -> Kind {
        Kind::Spawn {
            id: id.into(),
            parent: parent.into(),
            agent_type: ty.into(),
            label: label.into(),
        }
    }
    fn tool(agent: &str, name: &str) -> Kind {
        Kind::Tool {
            agent: agent.into(),
            tool: name.into(),
            detail: "SECRET-DETAIL".into(),
            key: 0,
        }
    }

    fn world() -> World {
        let mut w = World::new("proj");
        let events = vec![
            ev(
                1_000,
                Kind::Prompt {
                    agent: ROOT.into(),
                    text: "SECRET-PROMPT".into(),
                },
            ),
            ev(1_000, spawn("a1", ROOT, "explorer", "Map code")),
            ev(2_000, spawn("a2", "a1", "worker", "Fix it")),
            ev(3_000, tool("a1", "Read")),
            ev(3_100, tool("a1", "Read")),
            ev(3_200, tool("a2", "Bash")),
            ev(3_300, Kind::ToolError { agent: "a2".into() }),
            ev(
                4_000,
                Kind::Usage {
                    agent: "a1".into(),
                    tokens: Tokens {
                        input: 100,
                        output: 50,
                        cache_read: 1_000,
                        cache_write: 20,
                    },
                    model: "m".into(),
                },
            ),
            ev(
                5_000,
                Kind::Finish {
                    id: "a2".into(),
                    ok: false,
                    tokens: None,
                },
            ),
            ev(
                61_000,
                Kind::Finish {
                    id: "a1".into(),
                    ok: true,
                    tokens: None,
                },
            ),
        ];
        for e in &events {
            w.apply(e, false);
        }
        w
    }

    #[test]
    fn markdown_has_key_lines() {
        let md = markdown(&world());
        assert!(md.starts_with("# Session report: proj\n"));
        assert!(md.contains("- **Duration:** 01:00\n"));
        assert!(md.contains("- **Agents:** 3 (2 subagents)\n"));
        assert!(md.contains("- **Tool calls:** 3\n"));
        assert!(md.contains("- **Tool errors:** 1\n"));
        assert!(md.contains("| Cache read | 1000 |"));
        assert!(md.contains("| Read | 2 |"));
        // Nesting: a2 sits under a1, which sits under main.
        assert!(md.contains("| ↳ Map code | explorer | main | done |"));
        assert!(md.contains("| · ↳ Fix it | worker | Map code | failed |"));
        assert!(md.contains("- **Fix it** (worker): failed, 1 tool error\n"));
        let at = |s: &str| md.find(s).unwrap();
        assert!(at("↳ Map code") < at("· ↳ Fix it"));
    }

    #[test]
    fn report_never_contains_prompts_or_tool_details() {
        let w = world();
        for out in [markdown(&w), json(&w)] {
            assert!(!out.contains("SECRET"));
        }
    }

    #[test]
    fn json_round_trips_to_the_documented_fields() {
        let v: Value = serde_json::from_str(&json(&world())).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["title"], "proj");
        assert_eq!(v["duration_ms"], 60_000);
        assert_eq!(v["agent_count"], 3);
        assert_eq!(v["tool_calls"], 3);
        assert_eq!(v["errors"], 1);
        assert_eq!(v["tokens"]["input"], 100);
        assert_eq!(v["tokens"]["cache_read"], 1_000);
        assert_eq!(v["tokens"]["total"], 170);
        let a = &v["agents"];
        assert_eq!(a[0]["id"], "main");
        assert!(a[0]["parent"].is_null());
        assert_eq!(a[2]["id"], "a2");
        assert_eq!(a[2]["parent"], "a1");
        assert_eq!(a[2]["depth"], 2);
        assert_eq!(a[2]["status"], "failed");
        assert_eq!(a[1]["models"][0], "m");
        assert_eq!(v["tools"][0]["tool"], "Read");
        assert_eq!(v["tools"][0]["calls"], 2);
        assert_eq!(v["failures"][0]["id"], "a2");
        assert_eq!(v["failures"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn pipes_and_newlines_stay_inside_their_table_cell() {
        let mut w = World::new("a|b");
        w.apply(&ev(1, spawn("x", ROOT, "t|y", "one | two\nthree")), false);
        w.apply(&ev(2, tool("x", "mcp|tool")), false);
        let md = markdown(&w);
        assert!(md.contains(r"one \| two three"));
        assert!(md.contains(r"mcp\|tool"));
        let row = md.lines().find(|l| l.contains("two three")).unwrap();
        // Eight columns have nine separators; escaped pipes are not separators.
        assert_eq!(row.replace(r"\|", "").matches('|').count(), 9, "{row}");
    }

    #[test]
    fn empty_session_renders() {
        let w = World::new("empty");
        let md = markdown(&w);
        assert!(md.contains("No tool calls."));
        assert!(md.contains("No failed agents and no tool errors."));
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["agent_count"], 1);
    }
}
