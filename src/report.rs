//! Session report: a static summary of a [`World`] as Markdown or JSON.
//!
//! Both renderers are pure. They read only what the world keeps: labels, tool
//! names, counts, timestamps, costs and health flags. Prompts, messages and
//! tool details are never included, so a report is safer to share than a
//! transcript. It can still name files or tasks through labels, so it should
//! be read before sharing.

use crate::event::{Event, Tokens, ROOT};
use crate::health::Health;
use crate::state::{Agent, Status, World};
use crate::util::{clean, fmt_date, fmt_dur, fmt_tokens, fmt_usd};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Version of the JSON layout. Bumped when a field is removed or changes meaning.
const SCHEMA: u32 = 1;
const TOP_TOOLS: usize = 10;
/// Longest label in Markdown, to keep the agent table readable.
const MD_LABEL: usize = 60;
/// Longest label in JSON. The world keeps labels at this length, so JSON
/// carries them as kept; scripts get the most it can give.
const JSON_LABEL: usize = 80;

/// Fold a whole transcript into `world`, the way `neonhive watch --once`
/// does: every event, then one health check at the session's own clock (its
/// last event). `Watcher::poll` with no clock does the same, so both
/// commands flag the same agents.
pub fn load(world: &mut World, events: &[Event]) {
    for ev in events {
        world.apply(ev, false);
    }
    world.sweep_health();
}

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

/// When the session started: the earliest agent start. `None` when no event
/// carried a time.
fn start_ms(w: &World) -> Option<i64> {
    w.agents
        .values()
        .map(|a| a.spawned_ts)
        .filter(|t| *t > 0)
        .min()
}

/// Measured from the session start only; never from 0.
fn session_duration_ms(w: &World) -> Option<i64> {
    start_ms(w).map(|s| (w.now_ts - s).max(0))
}

/// When the agent started. An agent with no activity of its own (a root whose
/// transcript is empty) has no start time; the session start stands in.
fn agent_start_ms(w: &World, a: &Agent) -> Option<i64> {
    match a.spawned_ts {
        0 => start_ms(w),
        t => Some(t),
    }
}

fn duration_ms(w: &World, a: &Agent) -> Option<i64> {
    agent_start_ms(w, a).map(|s| (a.finished_ts.unwrap_or(w.now_ts) - s).max(0))
}

/// Session cost with the same honesty as the UI header: `usd` is `None` when
/// nothing could be priced, and `lower_bound` is set when some agents could
/// not be priced and are left out of `usd`.
struct SessionCost {
    usd: Option<f64>,
    lower_bound: bool,
    unpriced: usize,
}

fn session_cost(w: &World) -> SessionCost {
    let s = w.cost_summary();
    SessionCost {
        usd: (s.total() > 0.0 || s.unpriced == 0).then(|| s.total()),
        lower_bound: s.unpriced > 0,
        unpriced: s.unpriced,
    }
}

fn health_name(h: Health) -> &'static str {
    match h {
        Health::Ok => "ok",
        Health::Looping => "looping",
        Health::Erroring => "erroring",
        Health::Stalled => "stalled",
    }
}

/// Agents with a health flag, in tree order, with the flag and its reason.
/// A reason names a tool and a count, or a silence; never tool input.
fn flagged(w: &World) -> Vec<(&Agent, Health, String)> {
    tree(w)
        .into_iter()
        .filter_map(|(a, _)| Some((a, w.health(a), w.health_reason(a)?)))
        .collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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

fn name_max(a: &Agent, max: usize) -> String {
    let n = if a.label.is_empty() {
        &a.agent_type
    } else {
        &a.label
    };
    clean(n, max)
}

fn name(a: &Agent) -> String {
    name_max(a, MD_LABEL)
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
    let mut o = String::new();
    let dash = || "-".to_string();

    o.push_str(&format!(
        "# Session report: {}\n\n",
        esc(&name(&w.agents[ROOT]))
    ));
    o.push_str(&format!(
        "- **Started:** {}\n",
        start_ms(w).map_or_else(dash, fmt_date)
    ));
    o.push_str(&format!(
        "- **Duration:** {}\n",
        session_duration_ms(w).map_or_else(dash, fmt_dur)
    ));
    o.push_str(&format!(
        "- **Agents:** {} ({})\n",
        rows.len(),
        plural(rows.len().saturating_sub(1), "subagent", "subagents")
    ));
    o.push_str(&format!("- **Tool calls:** {}\n", w.total_tools));
    o.push_str(&format!("- **Tool errors:** {}\n", errors(w)));
    o.push_str(&format!("- **Tokens:** {}\n", fmt_tokens(w.total_tokens())));
    let c = session_cost(w);
    let unpriced = plural(c.unpriced, "agent", "agents");
    let cost = match (c.usd, c.lower_bound) {
        (Some(usd), false) => fmt_usd(usd),
        (Some(usd), true) => format!(
            "{}+ (lower bound: {unpriced} on a model with no known price)",
            fmt_usd(usd)
        ),
        (None, _) => format!("- ({unpriced} on a model with no known price)"),
    };
    o.push_str(&format!("- **Estimated cost:** {cost}\n"));

    o.push_str("\n## Tokens\n\n| Kind | Count |\n| --- | ---: |\n");
    for (k, n) in [
        ("Input", t.input),
        ("Output", t.output),
        ("Cache write (5 min)", t.cache_write_5m),
        ("Cache write (1 h)", t.cache_write_1h),
        ("Cache read", t.cache_read),
    ] {
        o.push_str(&format!("| {k} | {n} |\n"));
    }

    o.push_str("\n## Agents\n\n");
    o.push_str("| Agent | Type | Parent | Status | Duration | Tokens | Cost | Tools | Errors |\n");
    o.push_str("| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: |\n");
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
            "| {indent}{} | {} | {parent} | {} | {} | {} | {} | {} | {} |\n",
            esc(&name(a)),
            esc(&a.agent_type),
            status_name(a.status),
            duration_ms(w, a).map_or_else(dash, fmt_dur),
            fmt_tokens(a.shown_tokens()),
            a.cost().map_or_else(dash, fmt_usd),
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

    o.push_str("\n## Health flags\n\n");
    let flags = flagged(w);
    if flags.is_empty() {
        o.push_str("No agent flagged as looping, erroring or stalled.\n");
    }
    for (a, h, reason) in flags {
        o.push_str(&format!(
            "- **{}** ({}): {}, {}\n",
            esc(&name(a)),
            esc(&a.agent_type),
            health_name(h),
            esc(&reason)
        ));
    }
    o
}

fn tokens_json(t: &Tokens, shown: u64) -> Value {
    json!({
        "input": t.input,
        "output": t.output,
        // `cache_write` stays the total of both kinds, as before.
        "cache_write": t.cache_write(),
        "cache_write_5m": t.cache_write_5m,
        "cache_write_1h": t.cache_write_1h,
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
                "label": clean(&a.label, JSON_LABEL),
                "status": status_name(a.status),
                "start_ms": agent_start_ms(w, a),
                "duration_ms": duration_ms(w, a),
                "cost_usd": a.cost(),
                "health": health_name(w.health(a)),
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
                "label": clean(&a.label, JSON_LABEL),
                "type": a.agent_type,
                "failed": a.status == Status::Failed,
                "errors": a.errors,
            })
        })
        .collect();
    let flags: Vec<Value> = flagged(w)
        .iter()
        .map(|(a, h, reason)| {
            json!({
                "id": a.id,
                "label": clean(&a.label, JSON_LABEL),
                "type": a.agent_type,
                "health": health_name(*h),
                "reason": reason,
            })
        })
        .collect();
    let c = session_cost(w);
    let doc = json!({
        "schema": SCHEMA,
        "title": name_max(&w.agents[ROOT], JSON_LABEL),
        "start_ms": start_ms(w),
        "duration_ms": session_duration_ms(w),
        "cost": {
            "usd": c.usd,
            "lower_bound": c.lower_bound,
            "unpriced_agents": c.unpriced,
        },
        "flags": flags,
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
                        cache_write_5m: 12,
                        cache_write_1h: 8,
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
        // Nine columns have ten separators; escaped pipes are not separators.
        assert_eq!(row.replace(r"\|", "").matches('|').count(), 10, "{row}");
    }

    #[test]
    fn empty_session_renders() {
        let w = World::new("empty");
        let md = markdown(&w);
        assert!(md.contains("No tool calls."));
        assert!(md.contains("No failed agents and no tool errors."));
        assert!(md.contains("No agent flagged as looping, erroring or stalled."));
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["agent_count"], 1);
    }

    fn prompt(ts: i64) -> Event {
        ev(
            ts,
            Kind::Prompt {
                agent: ROOT.into(),
                text: "SECRET-PROMPT".into(),
            },
        )
    }

    fn usage(agent: &str, model: &str, input: u64) -> Kind {
        Kind::Usage {
            agent: agent.into(),
            tokens: Tokens {
                input,
                ..Tokens::default()
            },
            model: model.into(),
        }
    }

    #[test]
    fn unknown_start_is_a_dash_or_null_never_1970() {
        // Events with no time: the world clock never moves off 0.
        let mut w = World::new("proj");
        w.apply(&ev(0, tool(ROOT, "Read")), false);
        let md = markdown(&w);
        assert!(md.contains("- **Started:** -\n"), "{md}");
        assert!(md.contains("- **Duration:** -\n"), "{md}");
        assert!(!md.contains("1970"));
        assert!(md.contains("| proj | main | - | running | - |"), "{md}");
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert!(v["start_ms"].is_null() && v["duration_ms"].is_null());
        assert!(v["agents"][0]["start_ms"].is_null());
        assert!(v["agents"][0]["duration_ms"].is_null());
    }

    #[test]
    fn a_prompt_alone_starts_the_session() {
        // One user line and no reply yet.
        let mut w = World::new("proj");
        w.apply(&prompt(1_700_000_000_000), false);
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["start_ms"], 1_700_000_000_000_i64);
        assert_eq!(v["duration_ms"], 0);
        assert!(markdown(&w).contains("- **Duration:** 00:00\n"));
    }

    #[test]
    fn duration_runs_from_the_first_prompt() {
        let mut w = World::new("proj");
        w.apply(&prompt(10_000), false);
        w.apply(&ev(40_000, tool(ROOT, "Read")), false);
        w.apply(&ev(70_000, tool(ROOT, "Read")), false);
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["start_ms"], 10_000);
        assert_eq!(v["duration_ms"], 60_000);
        assert_eq!(v["agents"][0]["duration_ms"], 60_000);
    }

    #[test]
    fn one_subagent_is_singular() {
        let mut w = World::new("proj");
        w.apply(&ev(1, spawn("a1", ROOT, "t", "x")), false);
        assert!(markdown(&w).contains("- **Agents:** 2 (1 subagent)\n"));
    }

    #[test]
    fn labels_are_cut_at_60_in_markdown_and_80_in_json() {
        let long = "x".repeat(100);
        let mut w = World::new("proj");
        w.apply(&ev(1, spawn("a1", ROOT, "t", &long)), false);
        let md = markdown(&w);
        let cut = |n: usize| format!("{}…", "x".repeat(n - 1));
        assert!(md.contains(&format!("| ↳ {} |", cut(MD_LABEL))), "{md}");
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["agents"][1]["label"], cut(JSON_LABEL));
    }

    #[test]
    fn cost_is_shown_per_agent_and_for_the_session() {
        let mut w = World::new("proj");
        w.apply(&prompt(1_000), false);
        w.apply(&ev(1_000, spawn("a1", ROOT, "explorer", "Map")), false);
        // $4 per million input tokens.
        w.apply(&ev(2_000, usage(ROOT, "claude-opus-5-5", 1_000_000)), false);
        w.apply(&ev(3_000, usage("a1", "claude-opus-5-5", 500_000)), false);
        let md = markdown(&w);
        assert!(md.contains("- **Estimated cost:** ~$6.00\n"), "{md}");
        assert!(md.contains("| Cost |"));
        assert!(md.contains("| 1.0M | ~$4.00 |"), "{md}");
        assert!(md.contains("| 500.0k | ~$2.00 |"), "{md}");
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["cost"]["usd"], 6.0);
        assert_eq!(v["cost"]["lower_bound"], false);
        assert_eq!(v["cost"]["unpriced_agents"], 0);
        assert_eq!(v["agents"][0]["cost_usd"], 4.0);
        assert_eq!(v["agents"][1]["cost_usd"], 2.0);
    }

    #[test]
    fn unpriced_agents_show_a_dash_and_make_the_total_a_lower_bound() {
        let mut w = World::new("proj");
        w.apply(&ev(1_000, spawn("a1", ROOT, "explorer", "Map")), false);
        w.apply(&ev(2_000, usage(ROOT, "claude-opus-5-5", 1_000_000)), false);
        w.apply(&ev(3_000, usage("a1", "no-such-model", 500)), false);
        let md = markdown(&w);
        assert!(
            md.contains(
                "- **Estimated cost:** ~$4.00+ (lower bound: 1 agent on a model with no known price)\n"
            ),
            "{md}"
        );
        assert!(md.contains("| 500 | - |"), "{md}");
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert_eq!(v["cost"]["usd"], 4.0);
        assert_eq!(v["cost"]["lower_bound"], true);
        assert_eq!(v["cost"]["unpriced_agents"], 1);
        assert!(v["agents"][1]["cost_usd"].is_null());
    }

    #[test]
    fn nothing_priced_is_a_dash_not_zero() {
        let mut w = World::new("proj");
        w.apply(&ev(1_000, usage(ROOT, "no-such-model", 500)), false);
        let md = markdown(&w);
        assert!(
            md.contains("- **Estimated cost:** - (1 agent on a model with no known price)\n"),
            "{md}"
        );
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        assert!(v["cost"]["usd"].is_null());
        assert_eq!(v["cost"]["lower_bound"], true);
        assert!(v["agents"][0]["cost_usd"].is_null());
    }

    /// A session with one looping, one erroring and one stalled agent, as of
    /// its own last event.
    fn troubled() -> Vec<Event> {
        use crate::health::STALL_AFTER_CALL_MS;
        let call = |ts: i64, agent: &str, name: &str, key: u64| {
            ev(
                ts,
                Kind::Tool {
                    agent: agent.into(),
                    tool: name.into(),
                    detail: "SECRET-DETAIL".into(),
                    key,
                },
            )
        };
        let mut evs = vec![
            prompt(1_000),
            ev(1_000, spawn("loop", ROOT, "worker", "Loops")),
            ev(1_000, spawn("err", ROOT, "worker", "Fails")),
            ev(1_000, spawn("quiet", ROOT, "worker", "Waits")),
            ev(1_000, spawn("fine", ROOT, "worker", "Works")),
        ];
        evs.extend((0..4).map(|i| call(2_000 + i * 1_000, "loop", "Bash", 7)));
        for i in 0..3 {
            evs.push(call(2_000 + i * 1_000, "err", "Edit", i as u64));
            evs.push(ev(
                2_500 + i * 1_000,
                Kind::ToolError {
                    agent: "err".into(),
                },
            ));
        }
        evs.push(call(2_000, "quiet", "Read", 1));
        evs.push(call(2_000, "fine", "Read", 2));
        evs.push(ev(
            3_000,
            Kind::Finish {
                id: "fine".into(),
                ok: true,
                tokens: None,
            },
        ));
        // Later activity moves the session clock past the stall threshold.
        // Erroring and looping outrank stalled, so those two stay as they are.
        evs.push(call(2_000 + STALL_AFTER_CALL_MS + 1, ROOT, "Read", 3));
        evs
    }

    #[test]
    fn health_flags_match_watch_once() {
        let evs = troubled();
        let mut w = World::new("proj");
        load(&mut w, &evs);
        let v: Value = serde_json::from_str(&json(&w)).unwrap();
        let mut from_report: Vec<(String, String, String)> = v["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let s = |k: &str| f[k].as_str().unwrap().to_string();
                (s("id"), s("health"), s("reason"))
            })
            .collect();

        // `neonhive watch --once`: one poll with no clock.
        let mut ww = World::new("proj");
        let alerts = crate::watch::Watcher::new(None, None).poll(&mut ww, &evs, None);
        let mut from_watch: Vec<(String, String, String)> = alerts
            .iter()
            .filter(|a| matches!(a.kind.name(), "looping" | "erroring" | "stalled"))
            .map(|a| {
                // Watch prefixes the reason with who it is about.
                let reason = a.message.trim_start_matches("agent ").to_string();
                (a.agent.clone().unwrap(), a.kind.name().to_string(), reason)
            })
            .collect();
        from_report.sort();
        from_watch.sort();
        assert_eq!(from_report, from_watch);
        let kinds: Vec<&str> = from_report.iter().map(|f| f.1.as_str()).collect();
        assert_eq!(kinds, ["erroring", "looping", "stalled"], "{from_report:?}");

        let md = markdown(&w);
        assert!(
            md.contains("- **Loops** (worker): looping, loop: Bash x4\n"),
            "{md}"
        );
        assert!(
            md.contains("- **Waits** (worker): stalled, stalled: silent 30:00\n"),
            "{md}"
        );
        assert!(
            md.contains("- **Fails** (worker): erroring, errors: 3 failed calls in a row (Edit)"),
            "{md}"
        );
        assert_eq!(v["agents"][1]["health"], "looping");
        assert_eq!(v["agents"][4]["health"], "ok");
        for out in [md, json(&w)] {
            assert!(!out.contains("SECRET"));
        }
    }

    #[test]
    fn json_keeps_every_old_key_and_adds_the_new_ones() {
        let v: Value = serde_json::from_str(&json(&world())).unwrap();
        let keys = |v: &Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
        assert_eq!(
            keys(&v),
            [
                "agent_count",
                "agents",
                "cost",
                "duration_ms",
                "errors",
                "failures",
                "flags",
                "schema",
                "start_ms",
                "title",
                "tokens",
                "tool_calls",
                "tools"
            ]
        );
        assert_eq!(
            keys(&v["agents"][0]),
            [
                "cost_usd",
                "depth",
                "duration_ms",
                "errors",
                "health",
                "id",
                "label",
                "models",
                "parent",
                "start_ms",
                "status",
                "tokens",
                "tool_calls",
                "type"
            ]
        );
        assert_eq!(keys(&v["cost"]), ["lower_bound", "unpriced_agents", "usd"]);
    }
}
