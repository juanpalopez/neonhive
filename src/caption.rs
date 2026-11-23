//! Plain-language captions for events, for talks, posts and onboarding.
//!
//! Built from `Event` alone (plus the world, to name agents), so they work for
//! every source. A caption never shows more of an event than the event stream
//! does: the same fields, cut to the same lengths or shorter.

use crate::event::{Event, Kind, ROOT};
use crate::state::World;
use crate::util::clean;

/// How far back a tool error looks for the call it belongs to.
const LOOKBACK: usize = 2_000;

/// A piece of caption text. `name` pieces are agent names, drawn highlighted.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub text: String,
    pub name: bool,
}

/// What kind of thing happened, for the caption's marker colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Spawn,
    Result,
    Fail,
    Message,
    Tool,
    Text,
}

/// Two lines: what happened, and what that means for someone who has never
/// seen the tool.
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub tone: Tone,
    pub what: Vec<Piece>,
    pub hint: &'static str,
}

#[cfg(test)]
impl Caption {
    /// The first line as plain text.
    pub fn plain(&self) -> String {
        self.what.iter().map(|p| p.text.as_str()).collect()
    }
}

/// The name an agent is shown by: its type, as in the event stream.
fn name(world: &World, id: &str) -> String {
    let n = world.agents.get(id).map_or(id, |a| a.agent_type.as_str());
    clean(n, 24)
}

fn who(text: String) -> Piece {
    Piece { text, name: true }
}

fn say(text: impl Into<String>) -> Piece {
    Piece {
        text: text.into(),
        name: false,
    }
}

/// Caption for `ev`. `before` holds the events applied before it, oldest
/// first. None for an event that the event stream does not show either
/// (token usage, a subagent's own prompt).
pub fn describe(world: &World, before: &[Event], ev: &Event) -> Option<Caption> {
    let (tone, what, hint) = match &ev.kind {
        Kind::Spawn {
            parent,
            agent_type,
            label,
            ..
        } => {
            let mut what = vec![
                who(name(world, parent)),
                say(" spawned a subagent "),
                who(clean(agent_type, 24)),
            ];
            let label = clean(label, 80);
            if !label.is_empty() {
                what.push(say(format!(" to work on: {label}")));
            }
            (
                Tone::Spawn,
                what,
                "A subagent is a separate agent with its own context, started to do one task (magenta packet).",
            )
        }
        Kind::Finish { id, ok, .. } => {
            let parent = world
                .agents
                .get(id)
                .and_then(|a| a.parent.clone())
                .unwrap_or_else(|| ROOT.to_string());
            let (tone, verb, hint) = if *ok {
                (
                    Tone::Result,
                    " finished and returned its result to ",
                    "The result flows back to the agent that started it (green packet).",
                )
            } else {
                (
                    Tone::Fail,
                    " failed and reported the failure to ",
                    "The parent gets the failure back (red packet) and decides what to do next.",
                )
            };
            (
                tone,
                vec![who(name(world, id)), say(verb), who(name(world, &parent))],
                hint,
            )
        }
        Kind::Message { from, to, summary } => {
            let target = world
                .resolve(to)
                .map_or_else(|| clean(to, 24), |t| name(world, &t));
            let mut what = vec![
                who(name(world, from)),
                say(" sent a message to "),
                who(target),
            ];
            let summary = clean(summary, 80);
            if !summary.is_empty() {
                what.push(say(format!(": {summary}")));
            }
            (
                Tone::Message,
                what,
                "Agents can talk to each other while they run (yellow packet).",
            )
        }
        Kind::Tool {
            agent,
            tool,
            detail,
            ..
        } => {
            let mut what = vec![
                who(name(world, agent)),
                say(format!(" ran the tool {}", clean(tool, 40))),
            ];
            let detail = clean(detail, 80);
            if !detail.is_empty() {
                what.push(say(format!(": {detail}")));
            }
            (
                Tone::Tool,
                what,
                "Agents act through tools: reading files, running commands, searching.",
            )
        }
        Kind::ToolError { agent } => {
            let tool = before
                .iter()
                .rev()
                .take(LOOKBACK)
                .find_map(|e| match &e.kind {
                    Kind::Tool { agent: a, tool, .. } if a == agent => Some(tool.as_str()),
                    _ => None,
                });
            let what = match tool {
                Some(t) => vec![
                    who(name(world, agent)),
                    say(format!(" ran the tool {} and it failed", clean(t, 40))),
                ],
                None => vec![
                    say("A tool call by "),
                    who(name(world, agent)),
                    say(" failed"),
                ],
            };
            (
                Tone::Fail,
                what,
                "The agent sees the error and usually tries something else.",
            )
        }
        Kind::Text { agent, text } => (
            Tone::Text,
            vec![
                who(name(world, agent)),
                say(format!(" wrote: {}", clean(text, 160))),
            ],
            "Text the agent wrote, for the person using it or for its parent agent.",
        ),
        Kind::Prompt { agent, text } if agent == ROOT => (
            Tone::Text,
            vec![
                who(name(world, agent)),
                say(format!(" received a prompt: {}", clean(text, 160))),
            ],
            "The instruction the session works from.",
        ),
        Kind::Prompt { .. } | Kind::Usage { .. } => return None,
    };
    Some(Caption { tone, what, hint })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Tokens;

    fn ev(ts: i64, kind: Kind) -> Event {
        Event { ts, kind }
    }

    /// A world with `main` and one subagent `explore`, and the events that
    /// built it.
    fn world() -> (World, Vec<Event>) {
        let mut w = World::new("proj");
        let spawn = ev(
            1,
            Kind::Spawn {
                id: "a1".into(),
                parent: ROOT.into(),
                agent_type: "explore".into(),
                label: "Search the codebase".into(),
            },
        );
        w.apply(&spawn, false);
        (w, vec![spawn])
    }

    fn caption(w: &World, before: &[Event], kind: Kind) -> Caption {
        describe(w, before, &ev(2, kind)).expect("captioned")
    }

    #[test]
    fn spawn_names_parent_type_and_task() {
        let (w, evs) = world();
        let c = describe(&w, &[], &evs[0]).unwrap();
        assert_eq!(
            c.plain(),
            "main spawned a subagent explore to work on: Search the codebase"
        );
        assert_eq!(c.tone, Tone::Spawn);
        let names: Vec<_> = c.what.iter().filter(|p| p.name).map(|p| &p.text).collect();
        assert_eq!(names, ["main", "explore"]);
    }

    #[test]
    fn finish_says_who_gets_the_result() {
        let (w, evs) = world();
        let done = |ok| Kind::Finish {
            id: "a1".into(),
            ok,
            tokens: None,
        };
        let c = caption(&w, &evs, done(true));
        assert_eq!(
            c.plain(),
            "explore finished and returned its result to main"
        );
        assert_eq!(c.tone, Tone::Result);
        let c = caption(&w, &evs, done(false));
        assert_eq!(c.plain(), "explore failed and reported the failure to main");
        assert_eq!(c.tone, Tone::Fail);
    }

    #[test]
    fn message_resolves_the_recipient() {
        let (w, evs) = world();
        let c = caption(
            &w,
            &evs,
            Kind::Message {
                from: ROOT.into(),
                to: "a1".into(),
                summary: "Narrow the scope".into(),
            },
        );
        assert_eq!(
            c.plain(),
            "main sent a message to explore: Narrow the scope"
        );
        assert_eq!(c.tone, Tone::Message);
    }

    #[test]
    fn tool_and_tool_error_name_the_tool() {
        let (w, mut evs) = world();
        let call = Kind::Tool {
            agent: "a1".into(),
            tool: "Bash".into(),
            detail: "cargo test".into(),
            key: 1,
        };
        let c = caption(&w, &evs, call.clone());
        assert_eq!(c.plain(), "explore ran the tool Bash: cargo test");
        let err = Kind::ToolError { agent: "a1".into() };
        let c = caption(&w, &evs, err.clone());
        assert_eq!(c.plain(), "A tool call by explore failed", "no call seen");
        evs.push(ev(2, call));
        let c = caption(&w, &evs, err);
        assert_eq!(c.plain(), "explore ran the tool Bash and it failed");
        assert_eq!(c.tone, Tone::Fail);
    }

    #[test]
    fn text_and_main_prompt_are_cut_like_the_stream() {
        let (w, evs) = world();
        let long = "word ".repeat(100);
        let c = caption(
            &w,
            &evs,
            Kind::Text {
                agent: "a1".into(),
                text: long,
            },
        );
        assert!(c.plain().starts_with("explore wrote: word word"));
        assert!(c.plain().chars().count() <= "explore wrote: ".len() + 160);
        let c = caption(
            &w,
            &evs,
            Kind::Prompt {
                agent: ROOT.into(),
                text: "Fix\nthe test".into(),
            },
        );
        assert_eq!(c.plain(), "main received a prompt: Fix the test");
    }

    #[test]
    fn events_the_stream_does_not_show_have_no_caption() {
        let (w, evs) = world();
        let usage = Kind::Usage {
            agent: "a1".into(),
            tokens: Tokens::default(),
            model: "m".into(),
        };
        let sub_prompt = Kind::Prompt {
            agent: "a1".into(),
            text: "secret task details".into(),
        };
        for kind in [usage, sub_prompt] {
            assert!(describe(&w, &evs, &ev(2, kind)).is_none());
        }
    }

    #[test]
    fn every_caption_has_a_hint() {
        let (w, evs) = world();
        let c = describe(&w, &[], &evs[0]).unwrap();
        assert!(!c.hint.is_empty());
    }
}
