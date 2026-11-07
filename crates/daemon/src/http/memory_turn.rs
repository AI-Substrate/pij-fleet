//! The memory turn (plan 201, AC2; blueprint c-memory-hook).
//!
//! Before pij compacts or closes a seat that holds a role, it asks that seat
//! to write what it learned into the role's memory and holds the compact or
//! close until the seat's turn ends, or [`BOUND`] passes, after which it
//! proceeds and says so. A seat holding no role is compacted or closed
//! directly.
//!
//! The caller is answered `held` at once and the action runs in the daemon,
//! so no client's request timeout (Copilot's 65 s, Node fetch's 300 s) can cut
//! the hold short, and a seat compacting or closing itself, which is mid-turn,
//! is handled the same way. The action is skipped if the seat was closed or
//! revived during the turn.
//!
//! The hold ends on facts the daemon already sees: the prompt's
//! `delivery.outcome` and then the seat's next `seat.activity` `idle`, which
//! every harness publishes at its turn boundary. An idle is only published on a
//! change from working, so the first idle after delivery is the end of the turn
//! that read the prompt, whether it started that turn or arrived mid-turn.
//!
//! Cost: one indexed holdings query per compact or close. For a holder being
//! compacted or closed, one live subscription while its turn is pending, one
//! delivery, one spine event and one deadline. No poll, no launch.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::Response;

use pij_core::events::EventFilter;
use pij_core::model::{
    DeliveryOutcome, Envelope, ErrorKind, Event, Msg, Receipt, SeatDescriptor, SeatId,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use super::AppState;
use crate::delivery::OUTCOME_EVENT_KIND;

/// How long a compact or close waits for the memory turn to end: the one
/// default, reported as `bound_ms`.
pub const BOUND: Duration = Duration::from_secs(5 * 60);
/// Overrides [`BOUND`] in seconds, read once at daemon start.
pub const BOUND_ENV: &str = "PIJ_RS_MEMORY_TURN_SECS";

/// The bound this daemon uses: [`BOUND_ENV`] when it is a positive integer
/// no larger than [`BOUND_MAX`] (a larger one is clamped), else [`BOUND`];
/// with a warning to print when the value was not used as given.
pub fn bound_from(value: Option<&str>) -> (Duration, Option<String>) {
    let Some(text) = value else {
        return (BOUND, None);
    };
    match text.trim().parse::<u64>() {
        Ok(secs) if secs > BOUND_MAX.as_secs() => (
            BOUND_MAX,
            Some(format!(
                "{BOUND_ENV}={secs} is above the {}s ceiling; using {}s",
                BOUND_MAX.as_secs(),
                BOUND_MAX.as_secs()
            )),
        ),
        Ok(secs) if secs > 0 => (Duration::from_secs(secs), None),
        _ => (
            BOUND,
            Some(format!(
                "{BOUND_ENV}={text:?} is not a positive whole number of seconds; using {}s",
                BOUND.as_secs()
            )),
        ),
    }
}

/// The ceiling on the bound: an override may shorten the wait, never lengthen
/// it past five minutes.
pub const BOUND_MAX: Duration = Duration::from_secs(5 * 60);
/// A second compact or close for a seat whose memory turn is still pending.
pub const E_PENDING: &str = "E-RS-MEMORY-TURN-PENDING";

/// The `held` value, and the held compact receipt's outcome reason.
pub const HELD: &str = "memory-turn";

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

impl Holding {
    /// `role/instance (project p)`, as every prompt names it.
    pub(crate) fn label(&self) -> String {
        let instance = if self.instance.is_empty() {
            String::new()
        } else {
            format!("/{}", self.instance)
        };
        format!("{}{instance} (project {})", self.role, self.project)
    }
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
    pub(crate) bound_ms: u64,
    /// In words, when the action proceeded without the turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    /// What happened to the action after the turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) then: Option<String>,
    /// When the bound passed while the prompt was still being admitted, how
    /// that admission settled after the action: `delivered-late`, `queued`,
    /// `refused` or `failed: …` (plan 209, #132 F005). Nothing is withdrawn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) admission: Option<String>,
    /// A held close's own receipt data (`tombstoned_at`, `directory`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) closed: Option<Value>,
}

/// The action a memory turn holds back.
pub(crate) enum Then {
    /// The authorized compact control, admitted after the turn.
    Compact(Msg),
    /// `pij close` by `actor`.
    Close {
        /// The closing seat.
        actor: SeatId,
        /// The close reason.
        reason: String,
    },
}

impl Then {
    fn action(&self) -> Action {
        match self {
            Self::Compact(_) => Action::Compact,
            Self::Close { .. } => Action::Close,
        }
    }
}

/// Hold `then` for `seat`'s memory turn: answer `held` now, and in the daemon
/// deliver the prompt, wait for the turn or the bound, run the action unless
/// the seat was closed or revived meanwhile, and record one spine event.
pub(crate) fn hold(
    state: &AppState,
    command: &str,
    seat: SeatDescriptor,
    holdings: Vec<Holding>,
    then: Then,
) -> Result<Value, Box<Response>> {
    // One pending memory turn per seat: a repeat compact or close is refused
    // by name, never a second turn or a second action.
    let pending = Pending::take(&state.services.memory_turns, &seat.id)
        .ok_or_else(|| Box::new(pending_refusal(command, &seat.id)))?;
    let bound = state.services.memory_turn_bound;
    // The bound runs from the hold, not from the prompt's admission.
    let held_at = Instant::now();
    let now = tokio::time::Instant::now();
    let deadline = now
        .checked_add(bound)
        .or_else(|| now.checked_add(BOUND_MAX))
        .unwrap_or(now);
    let action = then.action();
    // A held compact answers as a control receipt (the clients decode one, and
    // Copilot's checks its msg_id); a held close answers with the seat.
    let mut answer = match &then {
        Then::Compact(msg) => serde_json::to_value(Receipt {
            msg_id: msg.msg_id.clone(),
            outcome: DeliveryOutcome::Held {
                reason: HELD.to_string(),
            },
            at: now_ms(),
            cold_check: None,
            warning: None,
        })
        .unwrap_or_default(),
        Then::Close { .. } => json!({"seat": seat.id}),
    };
    answer["held"] = json!(HELD);
    answer["memory_turn"] = held(action, &holdings, bound);
    let state = state.clone();
    tokio::spawn(async move {
        let (mut report, late) =
            run(&state, &seat.id, holdings, action, bound, held_at, deadline).await;
        let (then, closed) = proceed(&state, &seat, then).await;
        // Free the seat before the event is visible, so a reader of the event
        // can always take the next turn.
        drop(pending);
        // The action ran at the bound; the late prompt's admission is still
        // the turn's, so the event says how it settled.
        if let Some(late) = late {
            report.admission = Some(settled(late.await));
        }
        report.then = Some(then);
        report.closed = closed;
        record(&state, &seat.id, &report).await;
    });
    Ok(answer)
}

/// The seats with a memory turn pending, one entry each; the entry goes when
/// its task ends, however it ends.
struct Pending {
    turns: Arc<Mutex<BTreeSet<String>>>,
    seat: String,
}

impl Pending {
    fn take(turns: &Arc<Mutex<BTreeSet<String>>>, seat: &SeatId) -> Option<Self> {
        let inserted = turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seat.to_string());
        inserted.then(|| Self {
            turns: Arc::clone(turns),
            seat: seat.to_string(),
        })
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.seat);
    }
}

fn pending_refusal(command: &str, seat: &SeatId) -> Response {
    let mut answer: Envelope<Value> = Envelope::refused(
        command,
        ErrorKind::Refused,
        format!(
            "{E_PENDING}: {seat} already has a memory turn pending; its held compact or close runs when that turn ends, so this request was not taken"
        ),
    );
    answer.details = Some(json!({"code": E_PENDING, "seat": seat}));
    super::envelope(StatusCode::CONFLICT, &answer)
}

/// Run the held action against the seat it was held for, and only that one:
/// what happened, and a close's receipt data.
async fn proceed(state: &AppState, held: &SeatDescriptor, then: Then) -> (String, Option<Value>) {
    // The brake: the action was authorized against the incarnation it was held
    // for. One read; a close then compares-and-sets against exactly this row,
    // so ordinary activity changes during the turn are allowed.
    let now = match state.services.registry.get(&held.id).await {
        Ok(Some(now)) => now,
        Ok(None) => return ("skipped: the seat no longer exists".to_string(), None),
        Err(error) => return (format!("failed: could not read the seat: {error}"), None),
    };
    if let Some(skip) = skip_reason(held, &now) {
        return (skip.to_string(), None);
    }
    // Authority is rechecked too: the closer must still be the seat or its
    // recorded parent. A seat reparented during the turn is not closed.
    if let Then::Close { actor, .. } = &then
        && actor != &now.id
        && now.parent.as_ref() != Some(actor)
    {
        return (
            format!(
                "skipped: {actor} no longer owns the seat (its parent changed during the memory turn)"
            ),
            None,
        );
    }
    match then {
        Then::Compact(msg) => match super::admit_control(state, msg).await {
            Ok(_) => ("compact admitted".to_string(), None),
            Err(error) => (format!("failed: {error}"), None),
        },
        Then::Close { actor, reason } => {
            let closed = super::lifecycle::close_seat(
                state,
                "pij close",
                &actor,
                held.id.clone(),
                reason,
                Some(now),
            )
            .await;
            match closed {
                Ok(data) => ("closed".to_string(), Some(data)),
                Err(refused) => (
                    format!("failed: the close answered {}", refused.status()),
                    None,
                ),
            }
        }
    }
}

/// Why the held action must not run against `now`: the seat closed, or a
/// revive put another incarnation (process or pane) in it, during the turn.
/// A one-directional brake: removing it only lets more actions run.
pub(crate) fn skip_reason(held: &SeatDescriptor, now: &SeatDescriptor) -> Option<&'static str> {
    if now.tombstoned_at.is_some() {
        Some("skipped: the seat was closed during its memory turn")
    } else if now.proc != held.proc || now.pane != held.pane {
        Some("skipped: the seat was revived during its memory turn")
    } else {
        None
    }
}

async fn record(state: &AppState, seat: &SeatId, report: &Report) {
    let event = Event {
        seq: None,
        v: 1,
        at: now_ms(),
        kind: EVENT_KIND.to_string(),
        seat: Some(seat.clone()),
        payload: serde_json::to_string(report).unwrap_or_else(|_| "{}".to_string()),
    };
    if let Err(error) = state.services.event_bus.publish(event).await {
        eprintln!("pij-rs could not record the memory turn for {seat}: {error}");
    }
}

/// The seat's open holdings: one indexed query. Empty for a non-holder, or
/// when the store cannot say (the action then runs as it always did).
pub(crate) async fn holdings(state: &AppState, seat: &SeatId) -> Vec<Holding> {
    holdings_of(&state.services, seat).await
}

/// [`holdings`], from the services alone (the watchdog round's form).
pub(crate) async fn holdings_of(services: &crate::Services, seat: &SeatId) -> Vec<Holding> {
    let mut holdings: Vec<Holding> = services
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
    let held: Vec<String> = holdings.iter().map(Holding::label).collect();
    let mut text = format!(
        "pij memory turn: pij is about to {verb} this seat, and you hold {}.\n\n\
         Before that, write one note with what you learned that the next holder of the role should know \
         (more only if they are truly separate): short, with a hook that says when to open it. Read the \
         role's notes first so you add rather than repeat.\n",
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
    /// Unarmed: only the prompt's own `delivery.outcome`, read in stream order,
    /// arms it, so an idle buffered before delivery never ends the turn, even
    /// when the send's receipt already says delivered.
    pub(crate) fn new(prompt: &str) -> Self {
        Self {
            prompt: prompt.to_string(),
            delivered: false,
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

/// The prompt's admission, owned by its turn.
type Admission = tokio::task::JoinHandle<pij_core::error::Result<Receipt>>;

/// How a late admission settled, for [`Report::admission`].
fn settled(
    joined: std::result::Result<pij_core::error::Result<Receipt>, tokio::task::JoinError>,
) -> String {
    match joined {
        Ok(Ok(receipt)) => match receipt.outcome {
            DeliveryOutcome::Delivered { .. } => "delivered-late".to_string(),
            DeliveryOutcome::Queued { .. } | DeliveryOutcome::Held { .. } => "queued".to_string(),
            DeliveryOutcome::Refused { .. } => "refused".to_string(),
        },
        Ok(Err(error)) => format!("failed: {error}"),
        Err(error) => format!("failed: {error}"),
    }
}

/// Deliver the memory prompt to `seat` and wait until its turn ends or
/// `bound` passes. When the bound passes during the prompt's admission, the
/// admission comes back with the report, still running.
async fn run(
    state: &AppState,
    seat: &SeatId,
    holdings: Vec<Holding>,
    action: Action,
    bound: Duration,
    started: Instant,
    deadline: tokio::time::Instant,
) -> (Report, Option<Admission>) {
    let services = &state.services;
    // Subscribed before the send, so neither fact can slip past.
    let mut events = services
        .event_bus
        .subscribe_live(EventFilter::kinds([OUTCOME_EVENT_KIND, ACTIVITY]).for_seat(seat.clone()));
    let at = now_ms();
    let msg_id = format!("memory-turn-{seat}-{at}");
    // Admission runs under the same deadline as the wait: a transport hold
    // (Claude's socket grace) must not outlast the bound. The turn owns the
    // admission as a task, so past the bound it keeps it and records how it
    // settled (plan 209, #132 F005).
    let delivery = Arc::clone(&services.delivery);
    let msg = Msg {
        from: SeatId::from(pij_core::BG_ACTOR),
        to: seat.clone(),
        body: prompt(action, &holdings, bound),
        msg_id: msg_id.clone(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };
    let mut admission: Admission = tokio::spawn(async move { delivery.accept(msg).await });
    let sent = match tokio::time::timeout_at(deadline, &mut admission).await {
        Err(_elapsed) => Err(admission),
        Ok(Ok(sent)) => Ok(sent),
        Ok(Err(join)) => Ok(Err(pij_core::error::PijError::Adapter {
            adapter: "daemon/memory-turn".to_string(),
            message: format!("the prompt's admission task failed: {join}"),
        })),
    };
    let mut late = None;
    let (ended, why) = match sent {
        Err(admission) => {
            late = Some(admission);
            (
                Ended::Bound,
                Some("the memory prompt was still being admitted".to_string()),
            )
        }
        Ok(Err(error)) => (
            Ended::Undelivered,
            Some(format!("the memory prompt could not be sent: {error}")),
        ),
        Ok(Ok(receipt)) => {
            if matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) {
                (
                    Ended::Undelivered,
                    Some("the seat refused the memory prompt".to_string()),
                )
            } else {
                let mut watch = Watch::new(&msg_id);
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
            "the seat's turn did not end within {}s{}; the {} proceeded without its memory turn",
            bound.as_secs(),
            why.map(|why| format!(" ({why})")).unwrap_or_default(),
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
        bound_ms: millis(bound),
        note,
        then: None,
        closed: None,
        admission: None,
    };
    (report, late)
}

/// The `memory_turn` a holder's compact or close answers with at once.
fn held(action: Action, holdings: &[Holding], bound: Duration) -> Value {
    json!({
        "action": action,
        "holdings": holdings,
        "bound_ms": millis(bound),
        "note": format!(
            "this seat holds a role, so pij asked it to write its memory; the {} runs when its turn ends, or after {}s",
            verb(action),
            bound.as_secs()
        ),
    })
}

fn verb(action: Action) -> &'static str {
    match action {
        Action::Compact => "compact",
        Action::Close => "close",
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        let mut watch = Watch::new("p");
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
    fn a_refused_prompt_ends_the_wait() {
        let refused = event(
            OUTCOME_EVENT_KIND,
            json!({"msg_id": "p", "outcome": {"outcome": "refused", "reason": "no"}}),
        );
        assert_eq!(Watch::new("p").on(&refused), Some(Ended::Undelivered));
    }

    /// Review F003: subscribe; the old turn's idle is buffered; the prompt is
    /// delivered directly and its outcome published. Read in stream order, the
    /// old idle comes first and must not end the prompt's turn.
    #[test]
    fn an_idle_buffered_before_the_prompts_delivery_does_not_end_its_turn() {
        let mut watch = Watch::new("p");
        let idle = event(ACTIVITY, json!({"state": "idle"}));
        let delivered = event(
            OUTCOME_EVENT_KIND,
            json!({"msg_id": "p", "outcome": {"outcome": "delivered", "origin": "injected_to_transport"}}),
        );
        assert_eq!(watch.on(&idle), None, "the old turn's idle");
        assert_eq!(watch.on(&delivered), None);
        assert_eq!(watch.on(&idle), Some(Ended::TurnEnd), "the prompt's turn");
    }

    #[test]
    fn a_held_action_skips_a_seat_closed_or_revived_during_the_turn() {
        let mut held = SeatDescriptor::new("s", pij_core::model::Harness::Omp, "/tree");
        held.proc = Some(pij_core::model::ProcIdentity {
            pid: 1,
            proc_start: 7,
        });
        assert_eq!(skip_reason(&held, &held.clone()), None);
        let mut revived = held.clone();
        revived.proc = Some(pij_core::model::ProcIdentity {
            pid: 2,
            proc_start: 9,
        });
        assert!(skip_reason(&held, &revived).unwrap().contains("revived"));
        let mut moved = held.clone();
        moved.pane = Some("%9".into());
        assert!(skip_reason(&held, &moved).unwrap().contains("revived"));
        let mut working = held.clone();
        working.state = pij_core::model::SystemState::Working;
        assert_eq!(
            skip_reason(&held, &working),
            None,
            "activity is not a new incarnation"
        );
        let mut closed = held.clone();
        closed.tombstoned_at = Some(1);
        assert!(skip_reason(&held, &closed).unwrap().contains("closed"));
    }

    #[test]
    fn the_bound_is_overridden_only_by_a_positive_integer_up_to_the_ceiling() {
        assert_eq!(bound_from(Some("5")), (Duration::from_secs(5), None));
        assert_eq!(bound_from(None), (BOUND, None));
        let (clamped, warning) = bound_from(Some("86400"));
        assert_eq!(clamped, BOUND_MAX);
        assert!(warning.unwrap().contains("ceiling"));
        for value in ["", "0", "-1", "5m"] {
            let (bound, warning) = bound_from(Some(value));
            assert_eq!(bound, BOUND, "{value:?}");
            assert!(warning.is_some(), "{value:?}");
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
