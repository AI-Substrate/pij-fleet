//! One conversation running as two seats (plan 174, pij-fleet#55).
//!
//! After the 2026-10-15 reboot pij revived `pij-different-smelt` in pane %5,
//! and 49 s later the same conversation was resumed by hand in pane %7. The
//! daemon already KNEW: adopt from %7 was refused, naming %5. But that text
//! reached only `hook.log`, every other verb from %7 said "not registered", and
//! %5 was never told. This module is the one place that detection lives, so the
//! callers that refuse with [`CONVERSATION_LIVE_CODE`] cannot drift apart:
//!
//! - **adopt** refuses to make one conversation a second seat;
//! - **pane resolution** (the identity ladder, and the Claude hooks' `bound_seat`)
//!   explains a pane no running seat owns when that pane is running a second
//!   copy of a live seat's conversation.
//!
//! The first such refusal for a (live seat, stray process) pair tells the live
//! seat once. Revive's pre-launch guard (`E-RS-REVIVE-CONVERSATION-LIVE`) keeps
//! its own original rule (plan 174 non-goal); it does not call this module.
//!
//! Evidence is OBSERVED, never claimed: a Claude process's own record under
//! `<claude home>/sessions/<pid>.json` counts only when the pid's observed start
//! matches the start it recorded, so a recycled pid's stale file derives nothing.

use std::cmp::Reverse;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::Response;
use pij_core::BG_ACTOR;
use pij_core::double_seat::{CONVERSATION_LIVE_CODE, DETECTED_KIND};
use pij_core::error::Result;
use pij_core::model::{
    Envelope, ErrorKind, Event, Harness, Msg, ProcIdentity, SeatDescriptor, SeatId,
};
use pij_core::ports::SeatFilter;
use pij_harnesses::proc::{
    ClaudeSessionRecord, claude_session_records, local_utc_offset_minutes,
    process_start_matches_utc_record,
};
use serde_json::{Value, json};

use super::identity::{is_live, process_gone};
use super::{AppState, envelope, system_time_ms};

/// A registered seat holding a conversation in a process that is not known to
/// be gone.
#[derive(Clone, Debug)]
pub(crate) struct LiveCopy {
    /// The seat holding the conversation.
    pub(crate) seat: SeatId,
    /// Its pane, when it has one.
    pub(crate) pane: Option<String>,
    /// Its recorded process.
    pub(crate) proc: ProcIdentity,
}

/// A conversation live as one seat, running a second time in another process.
#[derive(Clone, Debug)]
pub(crate) struct Double {
    /// The seat that holds the conversation.
    pub(crate) live: LiveCopy,
    /// Its harness, which names the adopt fix.
    pub(crate) harness: Harness,
    /// The conversation.
    pub(crate) session: String,
    /// The pane running the second copy.
    pub(crate) stray_pane: String,
    /// The second copy's process.
    pub(crate) stray: ProcIdentity,
}

/// The live seat holding `session`, other than any seat `skip` names.
///
/// THE shared helper: adopt and the stray-pane path ask this one question, so
/// the rule cannot differ between them (revive keeps its own, see above). A
/// row counts when it is live (not tombstoned), records a process, and that
/// process is not observed gone. An unknown liveness answer counts as running:
/// every caller REFUSES on a hit, so the unknown case errs toward not forking a
/// conversation. Newest process first, then id, as adopt has always chosen.
///
/// # Errors
/// The registry list failed.
pub(crate) async fn conversation_live_as_seat(
    state: &AppState,
    harness: Harness,
    session: &str,
    skip: impl Fn(&SeatDescriptor) -> bool,
) -> Result<Option<LiveCopy>> {
    let mut holders: Vec<SeatDescriptor> = state
        .services
        .registry
        .list(SeatFilter::default())
        .await?
        .into_iter()
        .filter(|seat| {
            is_live(seat)
                && seat.harness == harness
                && seat.harness_session.as_deref() == Some(session)
                && seat.proc.is_some()
                && !skip(seat)
        })
        .collect();
    holders.sort_by_key(|seat| {
        (
            Reverse(seat.proc.map(|proc| proc.proc_start)),
            seat.id.clone(),
        )
    });
    for seat in holders {
        if !process_gone(state, &seat).await {
            return Ok(Some(LiveCopy {
                proc: seat.proc.expect("filtered to seats recording a process"),
                seat: seat.id,
                pane: seat.pane,
            }));
        }
    }
    Ok(None)
}

/// Claude processes running now whose own record passes `keep`, each with the
/// identity the daemon observed for it.
///
/// The session-record witness the stray-pane path reads. A
/// record counts only when its pid's observed start matches the recorded one.
pub(crate) async fn live_claude_records(
    state: &AppState,
    keep: impl Fn(&ClaudeSessionRecord) -> bool,
) -> Vec<(ClaudeSessionRecord, ProcIdentity)> {
    let Ok(offset) = local_utc_offset_minutes() else {
        return Vec::new();
    };
    let homes = Arc::clone(&state.services.claude_homes);
    let records = tokio::task::spawn_blocking(move || claude_session_records(&homes))
        .await
        .unwrap_or_default();
    let mut live = Vec::new();
    for record in records.into_iter().filter(|record| keep(record)) {
        let Ok(Some(observed)) = state.services.liveness.proc_start(record.pid).await else {
            continue;
        };
        if process_start_matches_utc_record(&record.proc_start, observed, offset).unwrap_or(false) {
            let proc = ProcIdentity {
                pid: record.pid,
                proc_start: observed,
            };
            live.push((record, proc));
        }
    }
    live
}

/// Is `pane`, which no live seat owns, running a second copy of a live seat's
/// conversation?
///
/// Called ONLY on the refusal path of pane resolution (rk-0001): a resolved
/// pane never reads the session records. Any lookup failure answers `None`, so
/// the caller keeps today's not-registered text.
pub(crate) async fn stray_pane(state: &AppState, pane: &str) -> Option<Double> {
    for (record, stray) in live_claude_records(state, |record| record.pane() == Some(pane)).await {
        let live = conversation_live_as_seat(state, Harness::Claude, &record.session_id, |seat| {
            seat.proc == Some(stray) || seat.pane.as_deref() == Some(pane)
        })
        .await
        .ok()
        .flatten();
        if let Some(live) = live {
            return Some(Double {
                live,
                harness: Harness::Claude,
                session: record.session_id,
                stray_pane: pane.to_string(),
                stray,
            });
        }
    }
    None
}

/// The one refusal text, for adopt and every pane-resolved verb alike.
fn refusal_text(double: &Double) -> String {
    format!(
        "{CONVERSATION_LIVE_CODE} conversation `{}` is live as seat `{}` in another process \
         (pane {}); close one copy, then run `pij-rs adopt {} --harness {}` from the copy you \
         keep — one conversation cannot be two seats",
        double.session,
        double.live.seat,
        double.live.pane.as_deref().unwrap_or("none"),
        double.stray_pane,
        double.harness,
    )
}

/// Refuse `command` for a detected double, telling the live seat first (once).
pub(crate) async fn refused(state: &AppState, command: &str, double: &Double) -> Response {
    notify_once(state, double).await;
    let mut answer: Envelope<Value> =
        Envelope::refused(command, ErrorKind::Refused, refusal_text(double));
    answer.details = Some(json!({
        "code": CONVERSATION_LIVE_CODE,
        "seat": double.live.seat,
        "pane": double.live.pane,
        "harness_session": double.session,
        "pid": double.live.proc.pid,
        "stray_pane": double.stray_pane,
        "stray_pid": double.stray.pid,
    }));
    envelope(StatusCode::CONFLICT, &answer)
}

/// The notice's id, which is also its dedupe key: one per (live seat, stray
/// process). A pid alone is recycled at boot, so the start is part of the key.
fn notice_id(double: &Double) -> String {
    format!(
        "double-seat-{}-{}-{}",
        double.live.seat, double.stray.pid, double.stray.proc_start
    )
}

fn notice_body(double: &Double) -> String {
    format!(
        "[pij] double seat: your conversation `{}` is also running in pane {} (pid {}), outside \
         pij. Peers reach you here while the human may be typing there. Close one copy, then run \
         `pij-rs adopt <pane> --harness {}` from the copy you keep.",
        double.session, double.stray_pane, double.stray.pid, double.harness
    )
}

/// Tell the live seat about this pair, at most once, ever.
///
/// The `seat.double-detected` spine event is the dedupe record, so a daemon
/// restart does not resend. The dedupe is a BRAKE, not a policy: removing it
/// only sends MORE notices, never different ones. So an unreadable spine counts
/// as already sent, and a failed audit write sends nothing. The event is
/// written before the delivery (persist before mutate), and one lock makes the
/// read-then-write atomic against concurrent refusals (a SessionStart adopt and
/// a status-line whoami arrive together).
async fn notify_once(state: &AppState, double: &Double) {
    let _serial = state.double_seat_lock.lock().await;
    let id = notice_id(double);
    let seat = &double.live.seat;
    match state
        .services
        .event_bus
        .latest_matching_message(seat, DETECTED_KIND, &id)
        .await
    {
        Ok(None) => {}
        Ok(Some(_)) => return,
        Err(error) => {
            eprintln!(
                "pij-rs double-seat notice for {seat}: dedupe read failed, not sending: {error}"
            );
            return;
        }
    }
    let published = async {
        let at = system_time_ms()?;
        state
            .services
            .event_bus
            .publish(Event {
                seq: None,
                v: pij_core::wire::EVENT_VERSION,
                at,
                kind: DETECTED_KIND.to_string(),
                seat: Some(seat.clone()),
                payload: json!({
                    "msg_id": id,
                    "seat": seat,
                    "pane": double.live.pane,
                    "pid": double.live.proc.pid,
                    "stray_pane": double.stray_pane,
                    "stray_pid": double.stray.pid,
                    "proc_start": double.stray.proc_start,
                    "harness_session": double.session,
                })
                .to_string(),
            })
            .await
    }
    .await;
    if let Err(error) = published {
        eprintln!("pij-rs double-seat notice for {seat}: audit write failed, not sending: {error}");
        return;
    }
    let delivered = async {
        let Some(recipient) = state.services.registry.get(seat).await? else {
            return Ok(None);
        };
        let msg = Msg {
            from: SeatId::from(BG_ACTOR),
            to: seat.clone(),
            body: notice_body(double),
            msg_id: id.clone(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        };
        crate::park_notice::send_or_hold(&state.services, &recipient, msg)
            .await
            .map(Some)
    }
    .await;
    if let Err(error) = delivered {
        eprintln!("pij-rs double-seat notice to {seat} failed after its audit: {error}");
    }
}
