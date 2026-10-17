//! Plan 174: a double seat, one conversation running in two panes.
//!
//! Detection is not new. Adopt has always refused a second process for a live
//! seat's conversation, and revive refuses to relaunch one already running. What
//! this module adds is **visibility**: a stable code, `E-RS-CONVERSATION-LIVE`,
//! that every pane-resolved verb from the stray copy returns, and one notice to
//! the live seat per stray process.
//!
//! The stray check runs only where a verb was about to refuse anyway (the pane
//! resolved to no seat), so a registered pane never reads a process record.
//! It fails open: any read or probe that errors yields today's refusal.
//!
//! **The notice dedupe is a brake, not a policy.** The `seat.double-detected`
//! spine event is the dedupe record, keyed by (live seat, stray pid + start).
//! Removing the lookup could only send MORE notices, never different ones, so a
//! failed lookup counts as already sent.

use axum::http::StatusCode;
use axum::response::Response;
use pij_core::BG_ACTOR;
use pij_core::error::Result;
use pij_core::model::{
    DeliveryOutcome, Envelope, ErrorKind, Event, Harness, Liveness, Msg, ProcIdentity,
    SeatDescriptor, SeatId,
};
use pij_core::ports::SeatFilter;
use pij_harnesses::proc::ClaudeSessionRecord;
use serde_json::{Value, json};

use super::identity::is_live;
use super::{AppState, envelope, system_time_ms};
use crate::Services;

/// The refusal every verb from a stray copy returns, and what scripts branch on.
pub(crate) const CONVERSATION_LIVE: &str = "E-RS-CONVERSATION-LIVE";
/// The spine record of one notice to the live seat; also its dedupe record.
pub(crate) const DOUBLE_DETECTED_KIND: &str = "seat.double-detected";

/// The copy of the conversation that is NOT the seat.
pub(super) struct Stray<'a> {
    pub(super) pane: Option<&'a str>,
    pub(super) proc: Option<ProcIdentity>,
    pub(super) session: &'a str,
}

/// The first seat `admit` accepts that holds `session` for `harness` and whose
/// recorded process is alive.
pub(super) async fn active_seat_holding(
    state: &AppState,
    harness: Harness,
    session: &str,
    admit: impl Fn(&SeatDescriptor) -> bool,
) -> Result<Option<SeatDescriptor>> {
    let seats = state.services.registry.list(SeatFilter::default()).await?;
    for seat in seats.into_iter().filter(|seat| {
        seat.harness == harness && seat.harness_session.as_deref() == Some(session) && admit(seat)
    }) {
        if let Some(proc) = seat.proc
            && matches!(
                pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await,
                Ok(Liveness::Active)
            )
        {
            return Ok(Some(seat));
        }
    }
    Ok(None)
}

/// The first Claude process record `pick` accepts whose pid is alive with the
/// start it recorded, with that process's identity. A stale file for a
/// recycled pid verifies nothing.
pub(super) async fn verified_claude_record(
    state: &AppState,
    pick: impl Fn(&ClaudeSessionRecord) -> bool,
) -> Option<(ClaudeSessionRecord, ProcIdentity)> {
    let offset = pij_harnesses::proc::local_utc_offset_minutes().ok()?;
    for record in pij_harnesses::proc::claude_session_records(&state.services.claude_homes)
        .into_iter()
        .filter(|record| pick(record))
    {
        let Ok(Some(observed)) = state.services.liveness.proc_start(record.pid).await else {
            continue;
        };
        if pij_harnesses::proc::process_start_matches_utc_record(
            &record.proc_start,
            observed,
            offset,
        )
        .unwrap_or(false)
        {
            let proc = ProcIdentity {
                pid: record.pid,
                proc_start: observed,
            };
            return Some((record, proc));
        }
    }
    None
}

/// The coded refusal for a verb called from `pane`, when no seat owns that pane
/// but a verified Claude process there runs a conversation that is live as
/// another seat. `None` keeps the caller's own refusal.
pub(super) async fn stray_refusal(state: &AppState, command: &str, pane: &str) -> Option<Response> {
    let (record, proc) =
        verified_claude_record(state, |record| record.pane() == Some(pane)).await?;
    let live = active_seat_holding(state, Harness::Claude, &record.session_id, |seat| {
        is_live(seat) && seat.machine.is_none() && seat.proc != Some(proc)
    })
    .await
    .ok()??;
    let stray = Stray {
        pane: Some(pane),
        proc: Some(proc),
        session: &record.session_id,
    };
    Some(refusal(state, command, &live, &stray).await)
}

/// Refuse with [`CONVERSATION_LIVE`], naming the live seat and the fix, and
/// tell the live seat once.
pub(super) async fn refusal(
    state: &AppState,
    command: &str,
    live: &SeatDescriptor,
    stray: &Stray<'_>,
) -> Response {
    let mut answer: Envelope<Value> = Envelope::refused(
        command,
        ErrorKind::Refused,
        format!(
            "{CONVERSATION_LIVE} conversation `{}` is live as seat `{}` in pane {}; close one \
             copy, then run `pij-rs adopt {} --harness {}` from the copy you keep (one \
             conversation cannot be two seats)",
            stray.session,
            live.id,
            live.pane.as_deref().unwrap_or("none"),
            stray.pane.unwrap_or("<pane>"),
            live.harness,
        ),
    );
    answer.details = Some(json!({
        "code": CONVERSATION_LIVE,
        "seat": live.id,
        "pane": live.pane,
        "harness_session": stray.session,
        "pid": live.proc.map(|proc| proc.pid),
    }));
    if let Some(proc) = stray.proc {
        notify_once(state, live, stray, proc).await;
    }
    envelope(StatusCode::CONFLICT, &answer)
}

/// The stable id of the one notice per (live seat, stray process).
fn notice_id(live: &SeatId, stray: ProcIdentity) -> String {
    format!("double-seat-{live}-{}-{}", stray.pid, stray.proc_start)
}

fn notice_body(live: &SeatDescriptor, stray: &Stray<'_>, proc: ProcIdentity) -> String {
    let pane = stray.pane.unwrap_or("(no tmux pane)");
    format!(
        "[pij] double seat: your conversation `{}` is also running in pane {pane} (pid {}), \
         which is not a seat. One conversation cannot be two seats: close one copy. To keep \
         {pane} instead of this pane, close this one, then run `pij-rs adopt {pane} --harness \
         {}` there.",
        stray.session, proc.pid, live.harness,
    )
}

/// Record the double and message the live seat, at most once per stray process.
async fn notify_once(
    state: &AppState,
    live: &SeatDescriptor,
    stray: &Stray<'_>,
    proc: ProcIdentity,
) {
    let services = &state.services;
    let id = notice_id(&live.id, proc);
    // A brake: an unknown answer counts as already sent.
    match services
        .event_bus
        .latest_matching_message(&live.id, DOUBLE_DETECTED_KIND, &id)
        .await
    {
        Ok(None) => {}
        Ok(Some(_)) => return,
        Err(error) => {
            eprintln!("pij-rs double-seat notice {id}: dedupe read failed, not sending: {error}");
            return;
        }
    }
    let body = notice_body(live, stray, proc);
    let at = match system_time_ms() {
        Ok(at) => at,
        Err(error) => {
            eprintln!("pij-rs double-seat notice {id}: {error}");
            return;
        }
    };
    let recorded = services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at,
            kind: DOUBLE_DETECTED_KIND.into(),
            seat: Some(live.id.clone()),
            payload: json!({
                "msg_id": id,
                "seat": live.id,
                "stray_pane": stray.pane,
                "stray_pid": proc.pid,
                "proc_start": proc.proc_start,
                "harness_session": stray.session,
                "body": body,
            })
            .to_string(),
        })
        .await;
    if let Err(error) = recorded {
        // No record means no dedupe: sending now could repeat on every verb.
        eprintln!("pij-rs double-seat notice {id}: record failed, not sending: {error}");
        return;
    }
    let msg = Msg {
        from: SeatId::from(BG_ACTOR),
        to: live.id.clone(),
        body,
        msg_id: id,
        from_machine: None,
        in_reply_to: None,
        command: None,
    };
    let services = services.clone();
    let live = live.clone();
    tokio::spawn(async move {
        let id = msg.msg_id.clone();
        if let Err(error) = deliver(&services, &live, msg).await {
            eprintln!("pij-rs double-seat notice {id}: delivery failed: {error}");
        }
    });
}

/// The ordinary path, cold-wake rules included: where a `pij send` would be
/// refused as a cold wake, or delivery is refused, the notice is held as an FYI
/// for the live seat's next turn under the same id.
async fn deliver(services: &Services, live: &SeatDescriptor, msg: Msg) -> Result<()> {
    let now = system_time_ms()?;
    let cold = crate::http::cold_wake::verdict(services, live, now).await?;
    if pij_core::cold_wake::refusal(&live.id, &cold).is_some() {
        services.delivery.hold_fyi(msg).await?;
        return Ok(());
    }
    let receipt = services.delivery.accept(msg.clone()).await?;
    if matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) {
        services.delivery.hold_fyi(msg).await?;
    }
    Ok(())
}
