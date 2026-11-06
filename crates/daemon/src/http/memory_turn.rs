//! The memory turn (plan 201, AC2; blueprint c-memory-hook).
//!
//! Before pij compacts or closes a seat that holds a role, it asks that seat
//! to write what it learned into the role's memory and holds the compact or
//! close until the seat's turn ends, or [`BOUND`] passes, after which it
//! proceeds and says so. A seat holding no role is compacted or closed
//! directly.
//!
//! The hold ends on facts the daemon already sees: the prompt's
//! `delivery.outcome` and then the seat's next `seat.activity` `idle`, which
//! every harness publishes at its turn boundary. An idle is only published on a
//! change from working, so the first idle after delivery is the end of the turn
//! that read the prompt, whether it started that turn or arrived mid-turn.
//!
//! Cost: one indexed holdings query per compact or close. For a holder, one
//! delivery, one live subscription, one spine event and one deadline. No poll,
//! no launch.

use std::time::{Duration, Instant};

use pij_core::events::EventFilter;
use pij_core::model::{Event, Msg, SeatId};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use super::AppState;
use crate::delivery::OUTCOME_EVENT_KIND;

/// How long a compact or close waits for the memory turn to end.
pub const BOUND: Duration = Duration::from_secs(5 * 60);
/// Overrides [`BOUND`] in seconds, read once at daemon start.
pub const BOUND_ENV: &str = "PIJ_RS_MEMORY_TURN_SECS";

/// The bound this daemon uses: [`BOUND_ENV`] when it is a positive integer,
/// else [`BOUND`].
pub fn bound_from(value: Option<&str>) -> Duration {
    value
        .and_then(|secs| secs.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(BOUND, Duration::from_secs)
}

/// The spine event recording each memory turn and how it ended.
pub const EVENT_KIND: &str = "memory.turn";
const ACTIVITY: &str = "seat.activity";

/// What the memory turn holds back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Action {
    /// A compact control.
    Compact,
    /// `pij close`.
    Close,
}

/// How a memory turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Ended {
    /// The seat's turn ended after it got the prompt.
    TurnEnd,
    /// [`BOUND`] passed first; the action proceeded without the turn.
    Bound,
    /// The prompt was refused or could not be sent; the action proceeded.
    Undelivered,
}

/// One seat's role holding, as the prompt names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Holding {
    pub(crate) project: String,
    pub(crate) role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) instance: String,
}

/// What a compact or close receipt says about its memory turn.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) action: Action,
    pub(crate) holdings: Vec<Holding>,
    /// The prompt's msg_id.
    pub(crate) prompt: String,
    pub(crate) ended: Ended,
    pub(crate) waited_ms: u64,
    /// In words, when the action proceeded without the turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// The seat's open holdings: one indexed query. Empty for a non-holder, or
/// when the store cannot say (the action then runs as it always did).
pub(crate) async fn holdings(state: &AppState, seat: &SeatId) -> Vec<Holding> {
    let mut holdings: Vec<Holding> = state
        .services
        .directory
        .store()
        .open_assignments(seat.as_str())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| Holding {
            project: row.project,
            role: row.role,
            instance: row.instance,
        })
        .collect();
    holdings.sort_by(|a, b| {
        (&a.project, &a.role, &a.instance).cmp(&(&b.project, &b.role, &b.instance))
    });
    holdings.dedup();
    holdings
}

/// The prompt a holder gets before `action`.
pub(crate) fn prompt(action: Action, holdings: &[Holding], bound: Duration) -> String {
    let verb = match action {
        Action::Compact => "compact",
        Action::Close => "close",
    };
    let held: Vec<String> = holdings
        .iter()
        .map(|holding| {
            let instance = if holding.instance.is_empty() {
                String::new()
            } else {
                format!("/{}", holding.instance)
            };
            format!("{}{instance} (project {})", holding.role, holding.project)
        })
        .collect();
    let mut text = format!(
        "pij memory turn: pij is about to {verb} this seat, and you hold {}.\n\n\
         Before that, write down what you learned that the next holder of the role should know: \
         one note per lesson, short, with a hook that says when to open it. Read the role's notes first \
         so you add rather than repeat.\n",
        held.join(", ")
    );
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for holding in holdings {
        let key = (holding.project.as_str(), holding.role.as_str());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let (project, role) = key;
        text.push_str(&format!(
            "\n  pij directory memory {role} --project {project}\n  \
             pij directory memory add {role} --project {project} --title \"<title>\" --hook \"<when to open it>\" \
             --tags \"<a,b>\" --body-file - <<'PIJ'\n  <what you learned>\n  PIJ\n"
        ));
    }
    text.push_str(&format!(
        "\nThen end your turn. The {verb} runs when this turn ends, or after {} minutes without it.",
        bound.as_secs().div_ceil(60)
    ));
    text
}

/// Folds the target's events into the moment its memory turn ends.
#[derive(Debug)]
pub(crate) struct Watch {
    prompt: String,
    delivered: bool,
}

impl Watch {
    pub(crate) fn new(prompt: &str, delivered: bool) -> Self {
        Self {
            prompt: prompt.to_string(),
            delivered,
        }
    }

    /// `Some` once `event` ends the turn: the first idle after the prompt is
    /// delivered, or the prompt's refusal.
    pub(crate) fn on(&mut self, event: &Event) -> Option<Ended> {
        let payload: Value = serde_json::from_str(&event.payload).unwrap_or(Value::Null);
        match event.kind.as_str() {
            OUTCOME_EVENT_KIND if payload["msg_id"] == self.prompt.as_str() => {
                match payload["outcome"]["outcome"].as_str() {
                    Some("delivered") => self.delivered = true,
                    Some("refused") => return Some(Ended::Undelivered),
                    _ => {}
                }
                None
            }
            ACTIVITY if self.delivered && payload["state"] == "idle" => Some(Ended::TurnEnd),
            _ => None,
        }
    }
}

/// Deliver the memory prompt to `seat` and wait until its turn ends or
/// `bound` passes. Records one [`EVENT_KIND`] spine event either way.
pub(crate) async fn run(
    state: &AppState,
    seat: &SeatId,
    holdings: Vec<Holding>,
    action: Action,
    bound: Duration,
) -> Report {
    let started = Instant::now();
    let services = &state.services;
    // Subscribed before the send, so neither fact can slip past.
    let mut events = services
        .event_bus
        .subscribe_live(EventFilter::kinds([OUTCOME_EVENT_KIND, ACTIVITY]).for_seat(seat.clone()));
    let at = now_ms();
    let msg_id = format!("memory-turn-{seat}-{at}");
    let sent = services
        .delivery
        .accept(Msg {
            from: SeatId::from(pij_core::BG_ACTOR),
            to: seat.clone(),
            body: prompt(action, &holdings, bound),
            msg_id: msg_id.clone(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        })
        .await;
    let (ended, why) = match sent {
        Err(error) => (
            Ended::Undelivered,
            Some(format!("the memory prompt could not be sent: {error}")),
        ),
        Ok(receipt) => {
            let delivered = matches!(
                receipt.outcome,
                pij_core::model::DeliveryOutcome::Delivered { .. }
            );
            if matches!(
                receipt.outcome,
                pij_core::model::DeliveryOutcome::Refused { .. }
            ) {
                (
                    Ended::Undelivered,
                    Some("the seat refused the memory prompt".to_string()),
                )
            } else {
                let mut watch = Watch::new(&msg_id, delivered);
                let deadline = tokio::time::Instant::now() + bound;
                loop {
                    match tokio::time::timeout_at(deadline, events.next()).await {
                        Ok(Some(event)) => {
                            if let Some(ended) = watch.on(&event) {
                                break (
                                    ended,
                                    (ended == Ended::Undelivered)
                                        .then(|| "the seat refused the memory prompt".to_string()),
                                );
                            }
                        }
                        Ok(None) | Err(_) => break (Ended::Bound, None),
                    }
                }
            }
        }
    };
    let waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let note = match ended {
        Ended::TurnEnd => None,
        Ended::Bound => Some(format!(
            "the seat's turn did not end within {}s; the {} proceeded without its memory turn",
            bound.as_secs(),
            verb(action)
        )),
        Ended::Undelivered => Some(format!(
            "{}; the {} proceeded without its memory turn",
            why.unwrap_or_else(|| "the memory prompt was not delivered".to_string()),
            verb(action)
        )),
    };
    let report = Report {
        action,
        holdings,
        prompt: msg_id,
        ended,
        waited_ms,
        note,
    };
    let event = Event {
        seq: None,
        v: 1,
        at: now_ms(),
        kind: EVENT_KIND.to_string(),
        seat: Some(seat.clone()),
        payload: serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string()),
    };
    if let Err(error) = services.event_bus.publish(event).await {
        eprintln!("pij-rs could not record the memory turn for {seat}: {error}");
    }
    report
}

/// The receipt a self-compact or self-close answers with: the action waits,
/// in the background, for the turn the caller is in to end.
pub(crate) fn held(action: Action, seat: &SeatId, holdings: &[Holding], bound: Duration) -> Value {
    json!({
        "seat": seat,
        "held": "memory-turn",
        "memory_turn": {
            "action": action,
            "holdings": holdings,
            "bound_secs": bound.as_secs(),
            "note": format!(
                "this seat holds a role, so pij asked it to write its memory; the {} runs when this turn ends, or after {}s",
                verb(action),
                bound.as_secs()
            ),
        },
    })
}

fn verb(action: Action) -> &'static str {
    match action {
        Action::Compact => "compact",
        Action::Close => "close",
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, payload: Value) -> Event {
        Event {
            seq: None,
            v: 1,
            at: 0,
            kind: kind.to_string(),
            seat: Some("s".into()),
            payload: payload.to_string(),
        }
    }

    fn holding(role: &str, instance: &str) -> Holding {
        Holding {
            project: "fleet".to_string(),
            role: role.to_string(),
            instance: instance.to_string(),
        }
    }

    #[test]
    fn the_turn_ends_on_the_first_idle_after_the_prompt_is_delivered() {
        let mut watch = Watch::new("p", false);
        let idle = event(ACTIVITY, json!({"state": "idle"}));
        assert_eq!(
            watch.on(&idle),
            None,
            "an idle before delivery is the previous turn"
        );
        let queued = event(
            OUTCOME_EVENT_KIND,
            json!({"msg_id": "p", "outcome": {"outcome": "queued"}}),
        );
        assert_eq!(watch.on(&queued), None);
        let other = event(
            OUTCOME_EVENT_KIND,
            json!({"msg_id": "x", "outcome": {"outcome": "delivered"}}),
        );
        assert_eq!(watch.on(&other), None);
        assert_eq!(
            watch.on(&idle),
            None,
            "another message's delivery is not the prompt's"
        );
        let delivered = event(
            OUTCOME_EVENT_KIND,
            json!({"msg_id": "p", "outcome": {"outcome": "delivered", "origin": "x"}}),
        );
        assert_eq!(watch.on(&delivered), None);
        assert_eq!(
            watch.on(&event(ACTIVITY, json!({"state": "working"}))),
            None
        );
        assert_eq!(watch.on(&idle), Some(Ended::TurnEnd));
    }

    #[test]
    fn a_refused_prompt_ends_the_wait_and_a_delivered_receipt_counts() {
        let refused = event(
            OUTCOME_EVENT_KIND,
            json!({"msg_id": "p", "outcome": {"outcome": "refused", "reason": "no"}}),
        );
        assert_eq!(
            Watch::new("p", false).on(&refused),
            Some(Ended::Undelivered)
        );
        let idle = event(ACTIVITY, json!({"state": "idle"}));
        assert_eq!(Watch::new("p", true).on(&idle), Some(Ended::TurnEnd));
    }

    #[test]
    fn the_bound_is_overridden_only_by_a_positive_integer() {
        assert_eq!(bound_from(Some("5")), Duration::from_secs(5));
        for value in [None, Some(""), Some("0"), Some("-1"), Some("5m")] {
            assert_eq!(bound_from(value), BOUND, "{value:?}");
        }
    }

    #[test]
    fn the_prompt_names_each_role_once_with_its_project_and_the_bound() {
        let text = prompt(
            Action::Close,
            &[
                holding("reviewer", "114"),
                holding("reviewer", "115"),
                holding("pm-001", ""),
            ],
            BOUND,
        );
        assert!(text.contains("about to close this seat"), "{text}");
        assert!(
            text.contains(
                "reviewer/114 (project fleet), reviewer/115 (project fleet), pm-001 (project fleet)"
            ),
            "{text}"
        );
        assert_eq!(
            text.matches("pij directory memory add reviewer --project fleet")
                .count(),
            1
        );
        assert!(text.contains("pij directory memory add pm-001 --project fleet"));
        assert!(text.ends_with("after 5 minutes without it."), "{text}");
    }
}
