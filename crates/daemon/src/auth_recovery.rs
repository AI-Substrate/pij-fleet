//! Restart a Copilot seat whose CLI lost its login (F212).
//!
//! A running Copilot CLI that starts answering `401 authorization` (or the
//! credential-authority refusal, which has no code) keeps failing until it is
//! restarted: a fresh process re-links its enterprise login with no one
//! signing in, the old one never does (46 such errors over three days, the same
//! process failing for up to six hours). The pij Copilot extension reports the
//! lost login once the failed run has ended (`POST /v1/seat/auth-error`); this
//! module decides what pij does about it.
//!
//! POLICY, and what decides it: one `/restart` per seat per
//! [`RECOVERY_WINDOW_MS`], typed through the same pane control path as a
//! compact, so the control path's own brakes still hold it (a settled idle, an
//! empty composer, the hold limit). Copilot's `/restart` swaps the CLI process
//! and restores the same live session: same seat id, native session, holdings
//! and held FYIs (proved on a throwaway seat, F212).
//!
//! BRAKES, each of which can only turn a restart into a notice or nothing:
//! - the seat is not idle, or its composer is not empty or not readable (a
//!   draft, a human typing, a layout pij cannot read): no restart; its parent
//!   and the prime are told how to restart it instead;
//! - a restart already happened inside the window: never a second one. If the
//!   process it started lost its login too, parent and prime are told the login
//!   needs renewing; while the old process is still the one running, nothing.
//! - parent and prime are told at most once per window.
//!
//! Removing any brake types the same `/restart` sooner or more often; none of
//! them chooses a different action. A report from a seat whose native session
//! is not the one it is bound to is refused before this runs (the route's
//! binding evidence), so a restart never reaches a seat the error did not
//! come from.
//!
//! Cost: the route runs only on an auth error. It reads the seat's latest
//! recovery event (one indexed read) and, when it may restart, one composer
//! capture through the existing interaction gate; it launches nothing. The
//! restart is one queued control; a notice is one or two messages.

use pij_core::model::{Event, Msg, SeatDescriptor, SeatId, SystemState};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Services;
use crate::pa_watchdog::{bg_msg, outcome_text};

/// The spine event that records each recovery decision that acted.
pub(crate) const AUTH_RECOVERY_KIND: &str = "seat.auth_recovery";
/// One restart, and one notice, per seat inside this window.
pub(crate) const RECOVERY_WINDOW_MS: u64 = 30 * 60 * 1_000;

/// Which lost login the extension saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LostLogin {
    /// `session.error` with `errorType: authorization` and status 401.
    Authorization,
    /// The credential authority's `Sign in … again` refusal (no code).
    CredentialAuthority,
}

/// What pij did about one report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// `/restart` was queued on the seat's control path.
    Restart,
    /// The seat was busy or held a draft: parent and prime were told how to
    /// restart it.
    Notify,
    /// The process pij restarted lost its login too: parent and prime were
    /// told the login needs renewing.
    Relogin,
    /// A restart is queued and the old process is still the one running.
    Pending,
    /// Parent and prime were already told inside the window.
    Told,
}

/// A recovery that acted, as recorded on the spine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recorded {
    pub(crate) action: Action,
    pub(crate) at: u64,
    /// The process the seat ran when the recovery was decided.
    #[serde(default)]
    pub(crate) pid: Option<u32>,
}

/// The decision, from the last recovery and the seat now. `free` is read
/// only when the answer depends on it.
pub(crate) fn decide(
    last: Option<&Recorded>,
    now_ms: u64,
    pid: Option<u32>,
    free: impl FnOnce() -> bool,
) -> Action {
    match last.filter(|last| now_ms.saturating_sub(last.at) < RECOVERY_WINDOW_MS) {
        Some(Recorded {
            action: Action::Restart,
            pid: restarted,
            ..
        }) => {
            if pid.is_some() && pid == *restarted {
                Action::Pending
            } else {
                Action::Relogin
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

/// Act on one lost-login report from `seat`'s own extension.
///
/// # Errors
/// Store failures reading or recording the decision: nothing was sent.
pub(crate) async fn handle(
    services: &Services,
    seat: &SeatDescriptor,
    kind: LostLogin,
    now_ms: u64,
) -> pij_core::error::Result<Outcome> {
    let last = services
        .spine
        .latest_matching(&seat.id, &[AUTH_RECOVERY_KIND])
        .await?
        .and_then(|event| serde_json::from_str::<Recorded>(&event.payload).ok());
    let pid = seat.proc.map(|proc| proc.pid);
    let free = if last
        .as_ref()
        .is_some_and(|last| now_ms.saturating_sub(last.at) < RECOVERY_WINDOW_MS)
    {
        false
    } else {
        composer_free(services, seat).await
    };
    let mut action = decide(last.as_ref(), now_ms, pid, || free);
    let mut msg_id = None;
    if action == Action::Restart {
        let id = format!("auth-restart:{}:{now_ms}", seat.id);
        // Persist before mutate: the window starts before the control exists.
        record(services, seat, Action::Restart, kind, now_ms, Some(&id)).await?;
        let control = Msg {
            command: Some("restart".to_string()),
            ..bg_msg(&seat.id, String::new(), id.clone())
        };
        match services.delivery.accept_control(control).await {
            Ok(_) => msg_id = Some(id),
            Err(cause) => {
                eprintln!("auth recovery: restart of {} not queued: {cause}", seat.id);
                action = Action::Notify;
            }
        }
    }
    let told = match action {
        Action::Notify | Action::Relogin => {
            record(services, seat, action, kind, now_ms, None).await?;
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
                "pid": seat.proc.map(|proc| proc.pid),
                "kind": kind,
                "msg_id": msg_id,
            })
            .to_string(),
        })
        .await
        .map(|_| ())
}

/// The seat's live parent and the machine's designated prime, once each.
async fn recipients(services: &Services, seat: &SeatDescriptor) -> Vec<SeatId> {
    let mut told: Vec<SeatId> = Vec::new();
    let prime = services
        .directory
        .store()
        .prime()
        .await
        .ok()
        .flatten()
        .filter(|designation| designation.state == pij_core::orchestration::PrimeState::Current)
        .map(|designation| designation.seat);
    for candidate in seat.parent.iter().cloned().chain(prime) {
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
        _ => format!(
            "{id}'s Copilot lost its login ({what}), and pij did not restart it because it was busy or had text in its composer. When it is free, type /restart in {pane}, or run `pij send {id} --command restart`. Its session, roles and queue survive a /restart."
        ),
    }
}

#[cfg(test)]
#[path = "auth_recovery_tests.rs"]
mod tests;
