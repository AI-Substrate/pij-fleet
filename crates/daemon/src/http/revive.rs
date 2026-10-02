//! Revive observes under the spawn lock, retires by CAS, then uses the shared launch.
use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, Event, Liveness, SeatDescriptor, SeatId, Seq};
use pij_core::wire;
use serde::Serialize;
use serde_json::{Value, json};

use super::identity::{Resolved, resolve_seat};
use super::lifecycle::{runtime_failure, tombstone_receipt};
use super::{
    AppState, ExistingSeatPolicy, ReviveRequest, SpawnRequest, envelope, internal,
    launch_seat_locked, system_time_ms,
};

const COMMAND: &str = "pij revive";

#[derive(Serialize)]
pub(super) struct Receipt {
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<Seq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assumed_dead_seq: Option<Seq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    legacy_tombstone_seq: Option<Seq>,
}

#[derive(Serialize)]
struct Observation<'a> {
    liveness: &'static str,
    pid: Option<u32>,
    proc_start: Option<u64>,
    pane: Option<&'a str>,
    pane_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn observe<'a>(state: &AppState, seat: &'a SeatDescriptor) -> Observation<'a> {
    let mut observation = Observation {
        liveness: "unknown",
        pid: seat.proc.map(|proc| proc.pid),
        proc_start: seat.proc.map(|proc| proc.proc_start),
        pane: seat.pane.as_deref(),
        pane_present: Some(false),
        observed_start: None,
        error: None,
    };
    match seat.proc {
        Some(proc) => match pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await
        {
            Ok(Liveness::Active) => observation.liveness = "active",
            Ok(Liveness::Dead { .. }) => observation.liveness = "dead",
            Ok(Liveness::Recycled { observed_start, .. }) => {
                observation.liveness = "recycled";
                observation.observed_start = Some(observed_start);
            }
            Err(error) => observation.error = Some(error.to_string()),
        },
        None => observation.error = Some("process identity unavailable".to_string()),
    }
    if let Some(recorded) = observation.pane {
        match state.services.tmux.list_panes().await {
            Ok(panes) => {
                observation.pane_present = Some(panes.iter().any(|pane| pane.id == recorded))
            }
            Err(error) => {
                observation.pane_present = None;
                let message = observation.error.get_or_insert_with(String::new);
                if !message.is_empty() {
                    message.push_str("; ");
                }
                message.push_str(&error.to_string());
            }
        }
    }
    observation
}

fn live_refusal(seat: &SeatId, observation: &Observation<'_>, reason: &str) -> Response {
    let override_command = format!("pij-rs revive {seat} --assume-dead --evidence \"<text>\"");
    let mut answer: Envelope<Value> = Envelope::refused(
        COMMAND,
        ErrorKind::Refused,
        format!(
            "E-RS-REVIVE-LIVE seat `{seat}` observation {}: {reason}. For recycled/unknown processes with an absent pane, the recorded parent (or prime for a parentless seat) may use `{override_command}`; active processes and present or unverifiable panes cannot be overridden.",
            observation.liveness
        ),
    );
    answer.details = Some(json!({
        "code":"E-RS-REVIVE-LIVE", "seat":seat, "observation":observation,
        "override_command":override_command,
    }));
    envelope(StatusCode::CONFLICT, &answer)
}

fn argument_failure(reason: String) -> Response {
    let mut answer: Envelope<Value> = Envelope::refused(COMMAND, ErrorKind::Refused, reason);
    answer.details = Some(json!({"code":"E-RS-ARG"}));
    envelope(StatusCode::BAD_REQUEST, &answer)
}

/// Which conversation the relaunch resumes, or why it must not launch at all.
async fn resume_decision(
    state: &AppState,
    prior: &SeatDescriptor,
    fresh: bool,
) -> Result<Option<String>, Box<Response>> {
    if fresh {
        return Ok(None);
    }
    let Some(session) = prior.harness_session.clone() else {
        return Err(Box::new(argument_failure(format!(
            "seat `{}` records no conversation to resume; `pij-rs revive {} --fresh` launches a new, blank one",
            prior.id, prior.id
        ))));
    };
    if let Some(pane) = conversation_live_elsewhere(state, prior, &session).await {
        let mut answer: Envelope<Value> = Envelope::refused(
            COMMAND,
            ErrorKind::Refused,
            format!(
                "E-RS-REVIVE-CONVERSATION-LIVE conversation `{session}` of `{}` is already running in pane {pane} — adopt there instead (`pij-rs adopt {pane} --harness {}`); relaunching it would fork the conversation",
                prior.id, prior.harness
            ),
        );
        answer.details = Some(json!({
            "code": "E-RS-REVIVE-CONVERSATION-LIVE", "seat": prior.id,
            "harness_session": session, "pane": pane,
        }));
        return Err(Box::new(envelope(StatusCode::CONFLICT, &answer)));
    }
    Ok(Some(session))
}

/// The pane where `session` is already running in some other process, if any.
///
/// Two observations, either sufficient: another seat holding the conversation
/// whose process is alive, or (Claude) a live process's own session record
/// under `<claude home>/sessions/<pid>.json`, the hand-resumed-but-unregistered
/// case the 2026-09-27 reboot produced. The record counts only when its pid's
/// observed start matches the start it recorded.
async fn conversation_live_elsewhere(
    state: &AppState,
    prior: &SeatDescriptor,
    session: &str,
) -> Option<String> {
    let seats = state
        .services
        .registry
        .list(pij_core::ports::SeatFilter::default())
        .await
        .ok()?;
    for seat in seats.iter().filter(|seat| {
        seat.id != prior.id
            && seat.harness == prior.harness
            && seat.harness_session.as_deref() == Some(session)
    }) {
        if let Some(proc) = seat.proc
            && matches!(
                pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await,
                Ok(Liveness::Active)
            )
        {
            return Some(seat.pane.clone().unwrap_or_else(|| "none".to_string()));
        }
    }
    if prior.harness != pij_core::model::Harness::Claude {
        return None;
    }
    let offset = pij_harnesses::proc::local_utc_offset_minutes().ok()?;
    for record in pij_harnesses::proc::claude_session_records(&state.services.claude_homes)
        .into_iter()
        .filter(|record| record.session_id == session)
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
            return Some(record.pane().map_or_else(
                || format!("(pid {}, no tmux pane)", record.pid),
                str::to_string,
            ));
        }
    }
    None
}

pub(super) async fn revive_seat(
    State(state): State<AppState>,
    body: std::result::Result<Json<ReviveRequest>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(request)) => request,
        Err(error) => return argument_failure(error.body_text()),
    };
    if request.assume_dead
        && request
            .evidence
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
    {
        return argument_failure(
            "--assume-dead requires nonempty --evidence \"<text>\"".to_string(),
        );
    }
    if !request.assume_dead && request.evidence.is_some() {
        return argument_failure("--evidence requires --assume-dead".to_string());
    }

    // No pre-lock registry read: a second revive must observe the new incarnation,
    // not the same dead predecessor that the first request is about to replace.
    let spawn_guard = state.spawn_lock.lock().await;
    let prior = match state.services.registry.get(&request.id).await {
        Ok(Some(prior)) => prior,
        Ok(None) => {
            return envelope(
                StatusCode::NOT_FOUND,
                &Envelope::<Value>::refused(
                    COMMAND,
                    ErrorKind::NotFound,
                    format!(
                        "seat `{}` does not exist — revive requires an existing id",
                        request.id
                    ),
                ),
            );
        }
        Err(error) => return internal(COMMAND, error),
    };
    // Plan 156 rule 3: revive resumes the recorded conversation. Decided after
    // the liveness refusals and before any write, so every refusal leaves the
    // seat exactly as it was.
    let resume;
    let receipt = if prior.tombstoned_at.is_some() {
        resume = match resume_decision(&state, &prior, request.fresh).await {
            Ok(resume) => resume,
            Err(refusal) => return *refusal,
        };
        match tombstone_receipt(state.services.spine.as_ref(), &prior.id, None, Some(&prior)).await
        {
            Ok(Some(receipt)) => Receipt {
                seq: Some(receipt.seq),
                assumed_dead_seq: None,
                legacy_tombstone_seq: None,
            },
            Ok(None) => {
                // A registry tombstone is sufficient even when an older native
                // retirement never published its own seat.tombstone event.
                let at = match system_time_ms() {
                    Ok(at) => at,
                    Err(error) => return internal(COMMAND, error),
                };
                let seq = match state
                    .services
                    .event_bus
                    .publish(Event {
                        seq: None,
                        v: wire::EVENT_VERSION,
                        at,
                        kind: "revive.legacy-tombstone".to_string(),
                        seat: Some(prior.id.clone()),
                        payload: json!({
                            "tombstoned_at": prior.tombstoned_at,
                            "reason": prior.tombstone_reason,
                        })
                        .to_string(),
                    })
                    .await
                {
                    Ok(seq) => seq,
                    Err(error) => return internal(COMMAND, error),
                };
                Receipt {
                    seq: None,
                    assumed_dead_seq: None,
                    legacy_tombstone_seq: Some(seq),
                }
            }
            Err(error) => return internal(COMMAND, error),
        }
    } else {
        let observation = observe(&state, &prior).await;
        if observation.liveness == "active" || observation.pane_present != Some(false) {
            return live_refusal(
                &prior.id,
                &observation,
                "the two-witness safety check does not establish death",
            );
        }
        let actor = if observation.liveness == "dead" {
            None
        } else {
            if !request.assume_dead {
                return live_refusal(
                    &prior.id,
                    &observation,
                    "explicit assumption and evidence are required",
                );
            }
            let actor = match resolve_seat(
                &state,
                COMMAND,
                request.caller.session_id,
                request.caller.pane,
            )
            .await
            {
                Resolved::Seat(actor, _) => actor,
                Resolved::Refusal(response) if response.status().is_server_error() => {
                    return response;
                }
                Resolved::Refusal(_) => {
                    return live_refusal(
                        &prior.id,
                        &observation,
                        "caller identity could not be resolved",
                    );
                }
            };
            let authorized = match prior.parent.as_ref() {
                Some(parent) => parent == &actor.id,
                None => match state.services.roles.read_role(&actor.id).await {
                    Ok(role) => role.as_deref() == Some("prime"),
                    Err(error) => return internal(COMMAND, error),
                },
            };
            if !authorized {
                return live_refusal(
                    &prior.id,
                    &observation,
                    "caller is not the recorded parent or an authoritative prime for a parentless seat",
                );
            }
            Some(actor.id)
        };
        resume = match resume_decision(&state, &prior, request.fresh).await {
            Ok(resume) => resume,
            Err(refusal) => return *refusal,
        };
        let reason = if actor.is_some() {
            "revive-assumed-dead"
        } else {
            "revive-observed-dead"
        };
        // Persist the caller's assumption before retiring anything. If the CAS
        // later refuses, this remains an audit of the attempt, not a death claim.
        let assumed_dead_seq = if let Some(actor) = actor {
            let at = match system_time_ms() {
                Ok(at) => at,
                Err(error) => return internal(COMMAND, error),
            };
            match state
                .services
                .event_bus
                .publish(Event {
                    seq: None,
                    v: wire::EVENT_VERSION,
                    at,
                    kind: "revive.assumed-dead".to_string(),
                    seat: Some(prior.id.clone()),
                    payload: json!({
                        "caller":actor, "evidence":request.evidence,
                        "observation":observation,
                    })
                    .to_string(),
                })
                .await
            {
                Ok(seq) => Some(seq),
                Err(error) => return internal(COMMAND, error),
            }
        } else {
            None
        };
        let seq = match state
            .services
            .registry
            .tombstone_if_unchanged(prior.clone(), reason.to_string())
            .await
        {
            Ok(seq) => seq,
            Err(error) => return runtime_failure(COMMAND, error),
        };
        Receipt {
            seq: Some(seq),
            assumed_dead_seq,
            legacy_tombstone_seq: None,
        }
    };

    // Preserve durable launch intent, not old process/port ownership. The existing
    // launch path still owns post-mortem publication and pane rollback on failure.
    // Reviving an existing seat is an explicit operator override of retirement.
    let spawn_request = SpawnRequest {
        id: Some(prior.id),
        harness: prior.harness,
        allow_retired: true,
        executable: None,
        model: prior.model,
        effort: prior.effort,
        cwd: prior.folder,
        session: request.session,
        caller_pane: request.caller_pane,
        name: request.name,
        parent: prior.parent,
        accept_inbound: prior.cross_session_inbound_accept == Some(true),
        wait_seconds: None,
        no_wait: true,
        resume,
    };
    launch_seat_locked(
        &state,
        spawn_request,
        ExistingSeatPolicy {
            allow_absent: false,
        },
        COMMAND,
        spawn_guard,
        Some(receipt),
    )
    .await
}
