//! `neonhive watch`: a headless watchdog. The core is pure: [`Watcher::poll`]
//! folds events into a `World`, moves the world clock to the time it is given
//! and returns the alerts that are new. It never reads the wall clock, never
//! sleeps and knows nothing about any agent tool, so tests drive it by hand.

use crate::event::{Event, ROOT};
use crate::health::Health;
use crate::state::{Agent, Status, World};
use crate::util::{fmt_clock, fmt_tokens};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Exit code: the session ended cleanly (or `--once` found nothing).
pub const EXIT_OK: i32 = 0;
/// Exit code: at least one alert fired.
pub const EXIT_ALERTS: i32 = 1;
/// Exit code: usage error, unreadable session or other I/O error.
pub const EXIT_ERROR: i32 = 2;

/// Lowest accepted `--interval`, in milliseconds.
pub const MIN_INTERVAL_MS: u64 = 100;

#[derive(clap::Args, Debug, Clone)]
#[command(after_help = "\
EXIT CODES:
  0  the session ended cleanly, or --once found nothing
  1  at least one alert fired
  2  usage error, unreadable session or other I/O error

A live session counts as ended when its transcript produced no new events for
--idle-exit seconds. Without --idle-exit, watch runs until interrupted (Ctrl-C
ends it the way the shell ends any interrupted program).

With --once the clock is the session's own (its last event), so stalls that
only show with the passing of real time are not reported.

JSON lines carry the same keys on every alert, null where they do not apply:
  {\"ts\":1700000000000,\"kind\":\"looping\",\"agent\":\"a1\",\"label\":\"Fix it\",
   \"message\":\"agent loop: Bash x4\",\"value\":null,\"limit\":null,\"unpriced\":null}
kind is one of failed, looping, erroring, stalled, recovered, max_tokens,
max_cost. Budget alerts fill value and limit; max_cost also fills unpriced.")]
pub struct Opts {
    /// Alert once when the session's tokens (input + output + cache writes) pass N.
    #[arg(long, value_name = "N")]
    pub max_tokens: Option<u64>,
    /// Alert once when the estimated cost passes USD.
    #[arg(long, value_name = "USD")]
    pub max_cost: Option<f64>,
    /// Print one JSON object per line instead of text.
    #[arg(long)]
    pub json: bool,
    /// Process the existing transcript, print current alerts and exit.
    #[arg(long)]
    pub once: bool,
    /// Poll interval in milliseconds (lowest 100).
    #[arg(long, value_name = "MS", default_value_t = 1000,
        value_parser = clap::value_parser!(u64).range(MIN_INTERVAL_MS..=3_600_000))]
    pub interval: u64,
    /// Exit when no new events arrived for this many seconds. Off by default.
    #[arg(long, value_name = "SECONDS")]
    pub idle_exit: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Failed,
    Looping,
    Erroring,
    Stalled,
    /// A health flag went away. Informational: not a problem.
    Recovered,
    MaxTokens,
    MaxCost,
}

impl AlertKind {
    pub fn name(self) -> &'static str {
        match self {
            AlertKind::Failed => "failed",
            AlertKind::Looping => "looping",
            AlertKind::Erroring => "erroring",
            AlertKind::Stalled => "stalled",
            AlertKind::Recovered => "recovered",
            AlertKind::MaxTokens => "max_tokens",
            AlertKind::MaxCost => "max_cost",
        }
    }

    /// Counts toward the "alerts fired" exit code.
    pub fn is_problem(self) -> bool {
        self != AlertKind::Recovered
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    /// World time in Unix milliseconds.
    pub ts: i64,
    pub kind: AlertKind,
    /// Agent id; `None` for session-wide (budget) alerts.
    pub agent: Option<String>,
    pub label: String,
    pub message: String,
    pub value: Option<f64>,
    pub limit: Option<f64>,
    /// Budget cost alerts: agents that could not be priced.
    pub unpriced: Option<usize>,
}

impl Alert {
    pub fn line(&self) -> String {
        let who = match &self.agent {
            Some(id) if self.label.is_empty() => id.clone(),
            Some(id) => format!("{id} ({})", self.label),
            None => "session".into(),
        };
        format!(
            "{} {} {}: {}",
            fmt_clock(self.ts),
            self.kind.name().to_uppercase(),
            who,
            self.message
        )
    }

    pub fn json(&self) -> Value {
        json!({
            "ts": self.ts,
            "kind": self.kind.name(),
            "agent": self.agent,
            "label": self.label,
            "message": self.message,
            "value": self.value,
            "limit": self.limit,
            "unpriced": self.unpriced,
        })
    }

    pub fn render(&self, as_json: bool) -> String {
        if as_json {
            self.json().to_string()
        } else {
            self.line()
        }
    }
}

pub struct Watcher {
    max_tokens: Option<u64>,
    max_cost: Option<f64>,
    failed: HashSet<String>,
    flags: HashMap<String, Health>,
    tokens_fired: bool,
    cost_fired: bool,
}

impl Watcher {
    pub fn new(max_tokens: Option<u64>, max_cost: Option<f64>) -> Self {
        Watcher {
            max_tokens,
            max_cost,
            failed: HashSet::new(),
            flags: HashMap::new(),
            tokens_fired: false,
            cost_fired: false,
        }
    }

    /// Fold `events` into `world`, then return the alerts that are new.
    /// `now` moves the world clock forward (live watching, so stalls show with
    /// no events arriving); `None` leaves it at the last event (`--once`).
    pub fn poll(&mut self, world: &mut World, events: &[Event], now: Option<i64>) -> Vec<Alert> {
        for ev in events {
            world.apply(ev, false);
        }
        if let Some(now) = now {
            world.now_ts = world.now_ts.max(now);
        }
        world.sweep_health();
        self.check(world)
    }

    fn check(&mut self, world: &World) -> Vec<Alert> {
        let ts = world.now_ts;
        let mut out = Vec::new();
        for id in &world.order {
            let Some(a) = world.agents.get(id) else {
                continue;
            };
            let agent_alert = |kind, message: String| Alert {
                ts,
                kind,
                agent: Some(id.clone()),
                label: a.label.clone(),
                message,
                value: None,
                limit: None,
                unpriced: None,
            };
            if a.status == Status::Failed {
                if self.failed.insert(id.clone()) {
                    out.push(agent_alert(
                        AlertKind::Failed,
                        format!("{} failed", a.agent_type),
                    ));
                }
            } else {
                self.failed.remove(id);
            }
            let now = world.health(a);
            let before = self.flags.insert(id.clone(), now).unwrap_or(Health::Ok);
            if now == before {
                continue;
            }
            out.push(match now {
                Health::Ok => agent_alert(AlertKind::Recovered, "no longer flagged".into()),
                Health::Looping => agent_alert(AlertKind::Looping, reason(world, a)),
                Health::Erroring => agent_alert(AlertKind::Erroring, reason(world, a)),
                Health::Stalled => agent_alert(AlertKind::Stalled, reason(world, a)),
            });
        }
        if let (Some(limit), false) = (self.max_tokens, self.tokens_fired) {
            let used = world.total_tokens();
            if used > limit {
                self.tokens_fired = true;
                out.push(Alert {
                    ts,
                    kind: AlertKind::MaxTokens,
                    agent: None,
                    label: String::new(),
                    message: format!(
                        "{} tokens passed the limit of {}",
                        fmt_tokens(used),
                        fmt_tokens(limit)
                    ),
                    value: Some(used as f64),
                    limit: Some(limit as f64),
                    unpriced: None,
                });
            }
        }
        if let (Some(limit), false) = (self.max_cost, self.cost_fired) {
            let c = world.cost_summary();
            if c.total() > limit {
                self.cost_fired = true;
                let note = match c.unpriced {
                    0 => String::new(),
                    n => format!(
                        "; {n} agent{} could not be priced, so the real cost is higher",
                        if n == 1 { "" } else { "s" }
                    ),
                };
                out.push(Alert {
                    ts,
                    kind: AlertKind::MaxCost,
                    agent: None,
                    label: String::new(),
                    message: format!(
                        "estimated cost ~${:.2} passed the limit of ${limit:.2}{note}",
                        c.total()
                    ),
                    value: Some(c.total()),
                    limit: Some(limit),
                    unpriced: Some(c.unpriced),
                });
            }
        }
        out
    }
}

fn reason(world: &World, a: &Agent) -> String {
    let who = if a.id == ROOT { "main agent" } else { "agent" };
    let why = world.health_reason(a).unwrap_or_default();
    format!("{who} {why}")
}

/// Decides when a live session counts as ended: no new events for a while.
/// Time is passed in, so tests need no clock.
pub struct Idle {
    limit_ms: Option<i64>,
    since: i64,
}

impl Idle {
    pub fn new(limit_secs: Option<u64>, now: i64) -> Self {
        Idle {
            limit_ms: limit_secs.map(|s| i64::try_from(s.saturating_mul(1000)).unwrap_or(i64::MAX)),
            since: now,
        }
    }

    /// Call after a poll with the number of events it returned.
    pub fn note(&mut self, events: usize, now: i64) {
        if events > 0 {
            self.since = now;
        }
    }

    pub fn expired(&self, now: i64) -> bool {
        self.limit_ms.is_some_and(|l| now - self.since >= l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Kind, Tokens};
    use crate::health::STALL_AFTER_CALL_MS;

    fn ev(ts: i64, kind: Kind) -> Event {
        Event { ts, kind }
    }
    fn spawn(ts: i64, id: &str) -> Event {
        ev(
            ts,
            Kind::Spawn {
                id: id.into(),
                parent: ROOT.into(),
                agent_type: "builder".into(),
                label: "Fix it".into(),
            },
        )
    }
    fn call(ts: i64, id: &str, key: u64) -> Event {
        ev(
            ts,
            Kind::Tool {
                agent: id.into(),
                tool: "Bash".into(),
                detail: "ls".into(),
                key,
            },
        )
    }
    fn err(ts: i64, id: &str) -> Event {
        ev(ts, Kind::ToolError { agent: id.into() })
    }
    fn usage(ts: i64, id: &str, model: &str, output: u64) -> Event {
        ev(
            ts,
            Kind::Usage {
                agent: id.into(),
                tokens: Tokens {
                    output,
                    ..Tokens::default()
                },
                model: model.into(),
            },
        )
    }
    fn finish(ts: i64, id: &str, ok: bool) -> Event {
        ev(
            ts,
            Kind::Finish {
                id: id.into(),
                ok,
                tokens: None,
            },
        )
    }
    fn kinds(a: &[Alert]) -> Vec<&'static str> {
        a.iter().map(|a| a.kind.name()).collect()
    }
    fn setup() -> (World, Watcher) {
        (World::new("t"), Watcher::new(None, None))
    }

    #[test]
    fn failed_agent_alerts_once() {
        let (mut w, mut t) = setup();
        let a = t.poll(
            &mut w,
            &[spawn(1000, "a1"), finish(2000, "a1", false)],
            None,
        );
        assert_eq!(kinds(&a), ["failed"]);
        assert_eq!(a[0].agent.as_deref(), Some("a1"));
        assert!(t.poll(&mut w, &[], None).is_empty());
        let ok = t.poll(&mut w, &[spawn(3000, "a2"), finish(4000, "a2", true)], None);
        assert!(ok.is_empty());
    }

    #[test]
    fn loop_flag_alerts_once_then_recovers_on_finish() {
        let (mut w, mut t) = setup();
        let mut evs = vec![spawn(1000, "a1")];
        evs.extend((0..4).map(|i| call(2000 + i * 1000, "a1", 7)));
        let a = t.poll(&mut w, &evs, None);
        assert_eq!(kinds(&a), ["looping"]);
        assert!(a[0].message.contains("Bash x4"), "{}", a[0].message);
        assert!(t.poll(&mut w, &[call(7000, "a1", 7)], None).is_empty());
        let a = t.poll(&mut w, &[finish(8000, "a1", true)], None);
        assert_eq!(kinds(&a), ["recovered"]);
        assert!(!a[0].kind.is_problem());
    }

    #[test]
    fn error_streak_alerts_once() {
        let (mut w, mut t) = setup();
        let mut evs = vec![spawn(1000, "a1")];
        for i in 0..3 {
            evs.push(call(2000 + i * 1000, "a1", i as u64));
            evs.push(err(2500 + i * 1000, "a1"));
        }
        let a = t.poll(&mut w, &evs, None);
        assert_eq!(kinds(&a), ["erroring"]);
        assert!(t.poll(&mut w, &[], None).is_empty());
    }

    #[test]
    fn stall_shows_with_clock_but_not_without() {
        let (mut w, mut t) = setup();
        let a = t.poll(&mut w, &[spawn(1000, "a1"), call(2000, "a1", 1)], None);
        assert!(a.is_empty());
        // --once style: no clock movement, still nothing.
        assert!(t.poll(&mut w, &[], None).is_empty());
        let late = 2000 + STALL_AFTER_CALL_MS + 1;
        let a = t.poll(&mut w, &[], Some(late));
        assert_eq!(kinds(&a), ["stalled"]);
        assert!(t.poll(&mut w, &[], Some(late + 5000)).is_empty());
        let a = t.poll(&mut w, &[finish(late + 6000, "a1", true)], None);
        assert_eq!(kinds(&a), ["recovered"]);
    }

    #[test]
    fn token_budget_fires_once() {
        let mut w = World::new("t");
        let mut t = Watcher::new(Some(1000), None);
        assert!(t
            .poll(&mut w, &[usage(1000, "main", "m", 900)], None)
            .is_empty());
        let a = t.poll(&mut w, &[usage(2000, "main", "m", 200)], None);
        assert_eq!(kinds(&a), ["max_tokens"]);
        assert_eq!(a[0].value, Some(1100.0));
        assert_eq!(a[0].limit, Some(1000.0));
        assert!(a[0].agent.is_none());
        assert!(t
            .poll(&mut w, &[usage(3000, "main", "m", 900)], None)
            .is_empty());
    }

    #[test]
    fn cost_budget_fires_once_and_notes_unpriced() {
        let mut w = World::new("t");
        let mut t = Watcher::new(None, Some(0.0001));
        let evs = [
            spawn(500, "a1"),
            usage(1000, "main", "claude-sonnet-4-5", 100_000),
            usage(1000, "a1", "no-such-model-xyz", 5),
        ];
        let a = t.poll(&mut w, &evs, None);
        assert_eq!(kinds(&a), ["max_cost"]);
        assert_eq!(a[0].unpriced, Some(1));
        assert!(
            a[0].message.contains("could not be priced"),
            "{}",
            a[0].message
        );
        let more = [usage(2000, "main", "claude-sonnet-4-5", 1000)];
        assert!(t.poll(&mut w, &more, None).is_empty());
    }

    #[test]
    fn cost_alert_without_unpriced_has_no_note() {
        let mut w = World::new("t");
        let mut t = Watcher::new(None, Some(0.0001));
        let evs = [usage(1000, "main", "claude-sonnet-4-5", 100_000)];
        let a = t.poll(&mut w, &evs, None);
        assert_eq!(a[0].unpriced, Some(0));
        assert!(!a[0].message.contains("priced"));
    }

    #[test]
    fn json_shape_is_stable() {
        let a = Alert {
            ts: 5,
            kind: AlertKind::Looping,
            agent: Some("a1".into()),
            label: "Fix it".into(),
            message: "loop".into(),
            value: None,
            limit: None,
            unpriced: None,
        };
        let line = a.render(true);
        assert!(!line.contains('\n'));
        let v: Value = serde_json::from_str(&line).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["agent", "kind", "label", "limit", "message", "ts", "unpriced", "value"]
        );
        assert_eq!(v["kind"], "looping");
        assert!(v["value"].is_null());
        assert!(a.render(false).contains("LOOPING a1 (Fix it): loop"));
    }

    #[test]
    fn once_semantics_report_current_state_and_stop() {
        // --once is one poll with no clock: a second identical poll is silent,
        // and a fresh watcher over the same events reports the same alerts.
        let evs = [spawn(1000, "a1"), finish(2000, "a1", false)];
        let (mut w1, mut t1) = setup();
        let (mut w2, mut t2) = setup();
        let a1 = t1.poll(&mut w1, &evs, None);
        let a2 = t2.poll(&mut w2, &evs, None);
        assert_eq!(a1, a2);
        assert_eq!(a1.iter().filter(|a| a.kind.is_problem()).count(), 1);
    }

    #[test]
    fn idle_rule() {
        let mut i = Idle::new(Some(10), 0);
        assert!(!i.expired(9_999));
        i.note(0, 9_000);
        assert!(i.expired(10_000));
        i.note(3, 9_500);
        assert!(!i.expired(10_000));
        assert!(i.expired(19_500));
        assert!(!Idle::new(None, 0).expired(i64::MAX));
    }
}
