//! Restart a Copilot seat whose CLI lost its login (F212).
//!
//! A running Copilot CLI that starts answering `401 authorization` (or the
//! credential-authority refusal, which has no code) keeps failing until it is
//! restarted: a fresh process re-links its enterprise login with no one
//! signing in, the old one never does (46 such errors over three days, the same
//! process failing for up to six hours). The pij Copilot extension reports the
//! lost login once the failed run has ended (`POST /v1/seat/auth-error`), naming
//! the process it runs in; this module decides what pij does about it.
//!
//! POLICY, and what decides it: one `/restart` per seat per
//! [`RECOVERY_WINDOW`], typed through the same pane control path as a compact,
//! so the control path's own brakes still hold it (a settled idle, an empty
//! composer, the hold limit). Copilot's `/restart` swaps the CLI process and
//! restores the same live session: same seat id, native session, holdings and
//! held FYIs (proved on a throwaway seat, F212).
//!
//! BRAKES, each of which can only turn a restart into a notice or nothing:
//! - the seat is not idle, or its composer is not empty or not readable (a
//!   draft, a human typing, a layout pij cannot read): no restart; its live
//!   parent and its projects' designated primes are told how to restart it;
//! - a restart already happened inside the window: never a second one. While
//!   its control is still queued, nothing more; once it has settled with the
//!   same process still running (Copilot refused `/restart`, or it was never
//!   typed), they are told the restart did not take; if the process it started
//!   lost its login too, they are told the login needs renewing;
//! - they are told at most once per window.
//!
//! Removing any brake types the same `/restart` sooner or more often; none of
//! them chooses a different action.
//!
//! The window is measured on the daemon's monotonic clock, never the wall
//! clock, so a clock correction cannot end it early. Decisions for one seat run
//! one at a time under that seat's lock, so two reports in flight together
//! still make one decision each, in turn. Each decision that acts is also a
//! `seat.auth_recovery` spine event; after a daemon restart, when this
//! process has no memory of the seat, a recent event is the brake instead
//! (the only use of its wall-clock stamp, and it can only spare a seat).
//!
//! Cost: the route runs only on an auth error. It reads the seat's latest
//! recovery event once per daemon lifetime, the live payloads of the seat's
//! control kind while a restart may be pending, its holdings when someone is
//! told, and one composer capture through the existing interaction gate when a
//! restart is possible; it launches nothing. The restart is one queued control;
//! a notice is one message per recipient.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pij_core::model::{DeliveryOutcome, Event, Msg, SeatDescriptor, SeatId, SystemState};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Services;
use crate::pa_watchdog::{bg_msg, outcome_text};

/// The spine event that records each recovery decision that acted.
pub(crate) const AUTH_RECOVERY_KIND: &str = "seat.auth_recovery";
/// One restart, and one notice, per seat inside this window.
pub(crate) const RECOVERY_WINDOW: Duration = Duration::from_secs(30 * 60);

/// Which lost login the extension saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LostLogin {
    /// `session.error` with `errorType: authorization` and status 401.
    Authorization,
    /// The credential authority's `Sign in … again` refusal (no code).
    CredentialAuthority,
}

/// The process a report speaks for: the Copilot CLI's pid and start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incarnation {
    /// Process id.
    pub pid: u32,
    /// The process's observed start, as the registry records it.
    pub proc_start: u64,
}

/// What pij did about one report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// `/restart` was queued on the seat's control path.
    Restart,
    /// The seat was busy or held a draft, or the restart could not be queued:
    /// its parent and primes were told how to restart it.
    Notify,
    /// The restart settled and the same process still runs: they were told it
    /// did not take.
    NotRestarted,
    /// The process pij restarted lost its login too: they were told the
    /// login needs renewing.
    Relogin,
    /// A restart is queued and has not run yet.
    Pending,
    /// They were already told inside the window.
    Told,
}

/// The last decision that acted for one seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) action: Action,
    pub(crate) since: Instant,
    pub(crate) incarnation: Incarnation,
    pub(crate) msg_id: Option<String>,
}

/// Each seat's last acting decision, behind that seat's own lock.
#[derive(Default)]
pub struct Ledger {
    seats: Mutex<HashMap<SeatId, Arc<tokio::sync::Mutex<Option<Entry>>>>>,
}

impl Ledger {
    /// An empty ledger: nothing decided yet in this daemon.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, seat: &SeatId) -> Arc<tokio::sync::Mutex<Option<Entry>>> {
        Arc::clone(
            self.seats
                .lock()
                .expect("auth recovery ledger mutex")
                .entry(seat.clone())
                .or_default(),
        )
    }
}

/// The decision, from the last acting one and the report now. `settled`
/// (the queued restart is no longer live) and `free` (idle, composer empty)
/// are read only when the answer depends on them.
pub(crate) fn decide(
    last: Option<&Entry>,
    now: Instant,
    reporter: Incarnation,
    settled: impl FnOnce() -> bool,
    free: impl FnOnce() -> bool,
) -> Action {
    match last.filter(|last| now.saturating_duration_since(last.since) < RECOVERY_WINDOW) {
        Some(Entry {
            action: Action::Restart,
            incarnation,
            ..
        }) => {
            if *incarnation != reporter {
                Action::Relogin
            } else if settled() {
                Action::NotRestarted
            } else {
                Action::Pending
            }
        }
        Some(_) => Action::Told,
        None if free() => Action::Restart,
        None => Action::Notify,
    }
}

/// What one report did, for the route's answer and the audit.
#[derive(Debug, Serialize)]
pub(crate) struct Outcome {
    pub(crate) action: Action,
    /// The restart control's msg_id, when one was queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) msg_id: Option<String>,
    /// Who was told, and each delivery's outcome word.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) told: Vec<(SeatId, String)>,
}

/// Act on one lost-login report from `seat`'s own extension, running in
/// `reporter` (the route has checked it is the seat's current process).
///
/// # Errors
/// Store failures reading or recording the decision: nothing was sent.
pub(crate) async fn handle(
    services: &Services,
    seat: &SeatDescriptor,
    reporter: Incarnation,
    kind: LostLogin,
    now_ms: u64,
) -> pij_core::error::Result<Outcome> {
    let slot = services.auth_recovery.slot(&seat.id);
    let mut last = slot.lock().await;
    let now = Instant::now();
    if last.is_none() {
        *last = remembered(services, &seat.id, now, now_ms).await?;
    }
    let in_window = last
        .as_ref()
        .filter(|entry| now.saturating_duration_since(entry.since) < RECOVERY_WINDOW);
    let pending_restart = in_window
        .filter(|entry| entry.action == Action::Restart && entry.incarnation == reporter)
        .and_then(|entry| entry.msg_id.clone());
    let settled = match pending_restart {
        Some(msg_id) => !control_live(services, &seat.id, &msg_id).await,
        None => false,
    };
    let free = if in_window.is_none() {
        composer_free(services, seat).await
    } else {
        false
    };
    let mut action = decide(last.as_ref(), now, reporter, || settled, || free);
    let mut msg_id = None;
    if action == Action::Restart {
        let id = format!("auth-restart:{}:{now_ms}", seat.id);
        // Persist before mutate: the window starts before the control exists.
        record(
            services,
            seat,
            reporter,
            Action::Restart,
            kind,
            now_ms,
            Some(&id),
        )
        .await?;
        *last = Some(Entry {
            action: Action::Restart,
            since: now,
            incarnation: reporter,
            msg_id: Some(id.clone()),
        });
        let control = Msg {
            command: Some("restart".to_string()),
            ..bg_msg(&seat.id, String::new(), id.clone())
        };
        match services.delivery.accept_control(control).await {
            Ok(receipt) if !matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) => {
                msg_id = Some(id);
            }
            Ok(_) => action = Action::Notify,
            Err(cause) => {
                eprintln!("auth recovery: restart of {} not queued: {cause}", seat.id);
                action = Action::Notify;
            }
        }
    }
    let told = match action {
        Action::Notify | Action::NotRestarted | Action::Relogin => {
            record(services, seat, reporter, action, kind, now_ms, None).await?;
            *last = Some(Entry {
                action,
                since: now,
                incarnation: reporter,
                msg_id: None,
            });
            tell(services, seat, action, kind, now_ms).await
        }
        Action::Restart | Action::Pending | Action::Told => Vec::new(),
    };
    Ok(Outcome {
        action,
        msg_id,
        told,
    })
}

/// The seat's last acting decision from the spine, for a daemon that has none
/// in memory (it restarted). Its wall-clock age places it on this daemon's
/// monotonic clock; that only ever spares the seat.
async fn remembered(
    services: &Services,
    seat: &SeatId,
    now: Instant,
    now_ms: u64,
) -> pij_core::error::Result<Option<Entry>> {
    #[derive(Deserialize)]
    struct Recorded {
        action: Action,
        at: u64,
        pid: Option<u32>,
        proc_start: Option<u64>,
        msg_id: Option<String>,
    }
    let Some(event) = services
        .spine
        .latest_matching(seat, &[AUTH_RECOVERY_KIND])
        .await?
    else {
        return Ok(None);
    };
    let Ok(recorded) = serde_json::from_str::<Recorded>(&event.payload) else {
        return Ok(None);
    };
    let (Some(pid), Some(proc_start)) = (recorded.pid, recorded.proc_start) else {
        return Ok(None);
    };
    let age = Duration::from_millis(now_ms.saturating_sub(recorded.at));
    let Some(since) = now.checked_sub(age.min(RECOVERY_WINDOW)) else {
        return Ok(None);
    };
    Ok(Some(Entry {
        action: recorded.action,
        since,
        incarnation: Incarnation { pid, proc_start },
        msg_id: recorded.msg_id,
    }))
}

/// Whether the restart control `msg_id` is still queued for `seat`. A read
/// that fails counts as queued: it can only hold a notice back.
async fn control_live(services: &Services, seat: &SeatId, msg_id: &str) -> bool {
    let kind = format!("delivery-control:{seat}");
    match services.queue.live_payloads(&[kind]).await {
        Ok(payloads) => payloads.iter().any(|payload| {
            serde_json::from_str::<serde_json::Value>(payload)
                .ok()
                .is_some_and(|value| value["msg_id"] == msg_id)
        }),
        Err(_) => true,
    }
}

/// Idle in the registry, and the composer recognized and empty now. Any read
/// that fails is not free: a brake.
async fn composer_free(services: &Services, seat: &SeatDescriptor) -> bool {
    if seat.state != SystemState::Idle {
        return false;
    }
    let Some(pane) = seat.pane.as_deref() else {
        return false;
    };
    services
        .interaction
        .fresh_injection_verdict(pane)
        .await
        .is_ok_and(|verdict| verdict.permitted && verdict.composer_idle)
}

async fn record(
    services: &Services,
    seat: &SeatDescriptor,
    reporter: Incarnation,
    action: Action,
    kind: LostLogin,
    at: u64,
    msg_id: Option<&str>,
) -> pij_core::error::Result<()> {
    services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at,
            kind: AUTH_RECOVERY_KIND.to_string(),
            seat: Some(seat.id.clone()),
            payload: json!({
                "action": action,
                "at": at,
                "pid": reporter.pid,
                "proc_start": reporter.proc_start,
                "kind": kind,
                "msg_id": msg_id,
            })
            .to_string(),
        })
        .await
        .map(|_| ())
}

/// The seat's live parent, and the designated prime of each project it holds
/// a role in, once each and never the seat itself (as the idle nudge resolves
/// them, `idle_nudge.rs`).
async fn recipients(services: &Services, seat: &SeatDescriptor) -> Vec<SeatId> {
    let mut candidates: Vec<SeatId> = seat.parent.iter().cloned().collect();
    match crate::http::memory_turn::try_holdings_of(services, &seat.id).await {
        Ok(holdings) => {
            let projects: std::collections::BTreeSet<String> =
                holdings.into_iter().map(|held| held.project).collect();
            for project in projects {
                match services.watchdogs().project(&project).await {
                    Ok(Some(record)) => candidates.extend(record.prime_id),
                    Ok(None) => {}
                    Err(cause) => eprintln!("auth recovery: project {project}: {cause}"),
                }
            }
        }
        Err(cause) => eprintln!("auth recovery: holdings of {}: {cause}", seat.id),
    }
    let mut told: Vec<SeatId> = Vec::new();
    for candidate in candidates {
        if candidate == seat.id || told.contains(&candidate) {
            continue;
        }
        if let Ok(Some(row)) = services.registry.get(&candidate).await
            && row.tombstoned_at.is_none()
        {
            told.push(candidate);
        }
    }
    told
}

async fn tell(
    services: &Services,
    seat: &SeatDescriptor,
    action: Action,
    kind: LostLogin,
    now_ms: u64,
) -> Vec<(SeatId, String)> {
    let body = notice(seat, action, kind);
    let mut told = Vec::new();
    for to in recipients(services, seat).await {
        let id = format!("auth-recovery:{}:{to}:{now_ms}", seat.id);
        let delivered = services
            .delivery
            .accept(bg_msg(&to, body.clone(), id))
            .await
            .map(|receipt| receipt.outcome)
            .map_err(|cause| cause.to_string());
        told.push((to, outcome_text(&delivered)));
    }
    told
}

/// The one-line fix, in the daemon's own words: only the seat id and pane,
/// each rendered by its grammar.
pub(crate) fn notice(seat: &SeatDescriptor, action: Action, kind: LostLogin) -> String {
    let id = pij_core::render::seat(&seat.id.0);
    let pane = seat
        .pane
        .as_deref()
        .filter(|pane| {
            pane.strip_prefix('%').is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map_or_else(|| "its pane".to_string(), |pane| format!("pane {pane}"));
    let what = match kind {
        LostLogin::Authorization => "a 401 authorization error",
        LostLogin::CredentialAuthority => "a credential-authority sign-in error",
    };
    match action {
        Action::Relogin => format!(
            "{id}'s Copilot lost its login again ({what}) after pij restarted it, so a restart is not enough: its login needs renewing. Type /login in {pane}, then /restart."
        ),
        Action::NotRestarted => format!(
            "{id}'s Copilot lost its login ({what}); pij typed /restart in {pane} but the same process is still running, so the restart did not take (Copilot may not offer /restart there). Quit it with /exit in {pane}, then run `pij revive {id}`: it resumes the same session."
        ),
        _ => format!(
            "{id}'s Copilot lost its login ({what}), and pij did not restart it: it was busy, had text in its composer, or the restart could not be queued. When it is free, type /restart in {pane}, or run `pij send {id} --command restart`. Its session, roles and queue survive a /restart."
        ),
    }
}

#[cfg(test)]
#[path = "auth_recovery_tests.rs"]
mod tests;
