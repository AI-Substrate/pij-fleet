//! The cold-wake guard on `pij send` (plan 157 phase 2, ruling #446).
//!
//! A BRAKE: it can only refuse. It reads the recipient's session facts through
//! `SessionStatusPort` and the daemon's own busy/idle state, and it never
//! changes what is delivered. Anything it cannot establish in time allows the
//! send and says why in the receipt's `cold_check`. The decision itself is
//! `pij_core::cold_wake::check`, pure and tested there.

use std::time::Duration;

use axum::http::StatusCode;
use axum::response::Response;
use pij_core::cold_wake::{
    COLD_IDLE_MS, COLD_WAKE_CODE, COLD_WAKE_FORCED_KIND, ColdCheck, check, refusal,
};
use pij_core::model::{Envelope, ErrorKind, Event, SeatDescriptor, SeatId};
use pij_core::session_status::SessionStatusBlock;

use super::{AppState, envelope, internal, refused, session_status_block, system_time_ms};
use crate::Services;

/// How long a send waits for the recipient's session facts before allowing it.
/// A cold first read of a very large transcript can take longer; the brake
/// then stays off for that send rather than delaying it.
const STATUS_WAIT: Duration = Duration::from_secs(3);

/// The `seat.activity` reason when the guard corrects a stale `working`.
const STALE_WORKING_REASON: &str = "stale working (esc)";

/// How long one pane idle probe may take before it counts as failed.
const PANE_PROBE_WAIT: Duration = Duration::from_secs(1);

/// Has the seat said `working` for longer than the cold idle threshold? Read
/// from its latest `seat.activity` fact. A turn that began recently is live,
/// whatever its pane shows: mid-stream Claude shows no spinner, so a seat just
/// woken after a long idle looks idle, with cold facts, until its reply is
/// written. An Esc-stale `working` is always at least as old as the last call.
/// A missing, unreadable or non-`working` fact is `false`.
async fn working_is_old(services: &Services, seat: &SeatId, now_ms: u64) -> bool {
    let Ok(Some(event)) = services
        .spine
        .latest_matching(seat, &["seat.activity"])
        .await
    else {
        return false;
    };
    let working = serde_json::from_str::<serde_json::Value>(&event.payload)
        .is_ok_and(|payload| payload["state"] == "working");
    working && now_ms.saturating_sub(event.at) > COLD_IDLE_MS
}

/// Does the seat's live pane show POSITIVE evidence of an idle prompt
/// (`HarnessPort::idle`)? `false` for no pane, a failed or slow probe, or any
/// frame that is not a vetted idle one: empty, blank, truncated, dialog, busy.
///
/// The real capture is a synchronous `tmux` child inside an `async fn`, which a
/// timeout cannot preempt. So the probe runs on the blocking pool, and the send
/// stops waiting at the bound. A wedged capture finishes, or not, on its own.
async fn pane_is_idle(services: &Services, seat: &pij_core::model::SeatDescriptor) -> bool {
    let Some(pane) = seat.pane.clone() else {
        return false;
    };
    let harness = services.harnesses.get(seat.harness).clone();
    let runtime = tokio::runtime::Handle::current();
    let probe = tokio::task::spawn_blocking(move || runtime.block_on(harness.idle(&pane)));
    matches!(
        tokio::time::timeout(PANE_PROBE_WAIT, probe).await,
        Ok(Ok(Ok(true)))
    )
}

/// Is `seat`'s prompt cache known warm at `now_ms` (plan 159's FYI flush,
/// `pij_core::cold_wake::is_warm`)? Its session facts get the same bounded wait
/// a send gets; no answer in time is unknown, and unknown is not warm.
pub(crate) async fn is_known_warm(state: &AppState, seat: &SeatDescriptor, now_ms: u64) -> bool {
    let Ok(block) = tokio::time::timeout(
        STATUS_WAIT,
        session_status_block(
            state.services.session_status.as_ref(),
            &seat.id,
            seat.harness,
            seat.harness_session.clone(),
            now_ms,
        ),
    )
    .await
    else {
        return false;
    };
    pij_core::cold_wake::is_warm(&block, now_ms)
}

/// The complete cold-wake verdict for one live seat: the pure `check()` over
/// its session facts, plus the stale-`working` correction. This is the one
/// decision both an ordinary `pij send` and a daemon park notice obey
/// (plan 167); nothing else re-derives it.
///
/// A seat can SAY working after its turn ended unseen (a Claude Esc interrupt
/// fires no hook). Only in the rare case that the facts alone say cold AND the
/// `working` itself is older than the idle threshold, the live pane decides: one
/// idle probe of that one pane. Only positive idle evidence means the `working`
/// was stale: the verdict is cold, and the seat's activity is corrected. Anything
/// else keeps trusting `working`, so the brake stays off rather than refuse on a
/// guess.
///
/// # Errors
/// Correcting the stale activity failed.
pub(crate) async fn verdict(
    services: &Services,
    seat: &SeatDescriptor,
    now_ms: u64,
) -> pij_core::error::Result<ColdCheck> {
    let block = tokio::time::timeout(
        STATUS_WAIT,
        session_status_block(
            services.session_status.as_ref(),
            &seat.id,
            seat.harness,
            seat.harness_session.clone(),
            now_ms,
        ),
    )
    .await
    .unwrap_or_else(|_| SessionStatusBlock::Failed {
        error: format!("no answer within {}s", STATUS_WAIT.as_secs()),
    });
    let verdict = check(seat.state, &block, now_ms);
    if verdict == ColdCheck::Busy {
        let as_idle = check(pij_core::model::SystemState::Idle, &block, now_ms);
        if matches!(as_idle, ColdCheck::Cold { .. })
            && working_is_old(services, &seat.id, now_ms).await
            && pane_is_idle(services, seat).await
        {
            services
                .registry
                .set_activity(
                    &seat.id,
                    pij_core::model::SystemState::Idle,
                    Some(STALE_WORKING_REASON),
                )
                .await?;
            return Ok(as_idle);
        }
    }
    Ok(verdict)
}

/// What the sender asked for about a cold recipient.
pub(crate) struct Override<'a> {
    pub(crate) force: bool,
    pub(crate) reason: Option<&'a str>,
    /// The paired machine a forwarded send came from, `None` for a local one.
    /// The guard applies the same rule either way (plan 164 ruling 6); this
    /// only names the sender's machine in the forced-wake audit.
    pub(crate) from_machine: Option<&'a str>,
}

/// The cold refusal, with the cold facts as data so a forwarding daemon can
/// relay them to the sender without reading them back out of the prose.
fn cold_refusal(command: &str, to: &SeatId, message: String, verdict: &ColdCheck) -> Response {
    let mut refusal = Envelope::<()>::refused(command, ErrorKind::Refused, message);
    refusal.details = Some(serde_json::json!({
        "code": COLD_WAKE_CODE,
        "seat": to,
        "cold": verdict,
    }));
    envelope(StatusCode::BAD_REQUEST, &refusal)
}

/// Check one local recipient before a real (non-FYI, non-control) send.
///
/// `Ok(label)` allows the send; the label goes on the receipt (`None` when the
/// recipient is absent or retired, which delivery refuses on its own). `Err`
/// is the refusal to return instead, and nothing has been sent.
pub(crate) async fn guard(
    state: &AppState,
    command: &str,
    from: &SeatId,
    to: &SeatId,
    msg_id: &str,
    over: Override<'_>,
) -> std::result::Result<Option<String>, Box<Response>> {
    let reason = over
        .reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty());
    if over.force && reason.is_none() {
        return Err(Box::new(refused(
            command,
            format!(
                "{COLD_WAKE_CODE}: --force needs a non-empty --reason saying why the wake is worth it"
            ),
        )));
    }
    // An absent or retired recipient is delivery's refusal to make, not ours.
    let seat = match state.services.registry.get(to).await {
        Ok(Some(seat)) if seat.tombstoned_at.is_none() => seat,
        Ok(_) => return Ok(None),
        Err(error) => return Err(Box::new(internal(command, error))),
    };
    let now_ms = system_time_ms().map_err(|error| Box::new(internal(command, error)))?;
    let verdict = verdict(&state.services, &seat, now_ms)
        .await
        .map_err(|error| Box::new(internal(command, error)))?;
    if let Some(message) = refusal(to, &verdict)
        && !over.force
    {
        return Err(Box::new(cold_refusal(command, to, message, &verdict)));
    }
    if let ColdCheck::Cold {
        context_tokens,
        idle_ms,
        model,
        estimate_usd,
    } = &verdict
    {
        // Persist the audit before the wake it records.
        let event = Event {
            seq: None,
            v: pij_core::wire::EVENT_VERSION,
            at: now_ms,
            kind: COLD_WAKE_FORCED_KIND.to_string(),
            seat: Some(to.clone()),
            payload: serde_json::json!({
                "from": from,
                "from_machine": over.from_machine,
                "msg_id": msg_id,
                "reason": reason,
                "context_tokens": context_tokens,
                "idle_ms": idle_ms,
                "model": model,
                "estimate_usd": estimate_usd,
            })
            .to_string(),
        };
        if let Err(error) = state.services.event_bus.publish(event).await {
            return Err(Box::new(internal(command, error)));
        }
    }
    Ok(Some(verdict.receipt_label(over.force)))
}
