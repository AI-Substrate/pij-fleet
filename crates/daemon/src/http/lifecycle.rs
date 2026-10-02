//! Close is an owner-only tombstone. Reap is conservative stale-record repair.
//! Both native argv and typed HTTP callers enter this single semantic parser.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::{PijError, Result};
use pij_core::model::{Envelope, ErrorKind, SeatDescriptor, SeatId, Seq};
use pij_core::ports::Spine;
use serde::Serialize;
use serde_json::{Value, json};

use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, internal};
use crate::reaper;

#[derive(Clone, Copy)]
enum Operation {
    Close,
    Reap,
}
impl Operation {
    fn verb(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Reap => "reap",
        }
    }
    fn command(self) -> &'static str {
        match self {
            Self::Close => "pij close",
            Self::Reap => "pij reap",
        }
    }
}

struct Call {
    caller: CallerContext,
    attribution: Vec<SeatId>,
    seat: Option<SeatId>,
    reason: String,
    dry_run: bool,
}

fn nonempty(value: &Value, name: &str) -> std::result::Result<String, String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} must be a nonempty string"))
}

fn parse(body: Value, operation: Operation) -> std::result::Result<Call, String> {
    let object = body
        .as_object()
        .ok_or("lifecycle request must be an object")?;
    for key in object.keys() {
        let allowed = matches!(key.as_str(), "argv" | "caller" | "actor")
            || match operation {
                Operation::Close => matches!(key.as_str(), "seat" | "reason"),
                Operation::Reap => key == "dry_run",
            };
        if !allowed {
            return Err(format!("unknown {} request field: {key}", operation.verb()));
        }
    }
    let caller = object
        .get("caller")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut call = Call {
        caller,
        attribution: Vec::new(),
        seat: None,
        reason: "owner-close".to_string(),
        dry_run: false,
    };
    if let Some(actor) = object.get("actor") {
        call.attribution.push(nonempty(actor, "actor")?.into());
    }
    if let Some(argv) = object.get("argv") {
        if ["seat", "reason", "dry_run"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return Err("use typed fields or argv, not both".to_string());
        }
        let argv: Vec<String> =
            serde_json::from_value(argv.clone()).map_err(|error| error.to_string())?;
        if argv.first().map(String::as_str) != Some(operation.verb()) {
            return Err(format!("argv must begin with {}", operation.verb()));
        }
        let mut args = argv.iter().skip(1);
        let mut reason_seen = false;
        let mut actor_seen = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--json" => {}
                "--actor" if !actor_seen => {
                    actor_seen = true;
                    let actor = args
                        .next()
                        .filter(|text| !text.trim().is_empty() && !text.starts_with('-'))
                        .ok_or("--actor requires a seat")?;
                    call.attribution.push(actor.as_str().into());
                }
                "--reason" if matches!(operation, Operation::Close) && !reason_seen => {
                    reason_seen = true;
                    call.reason = args
                        .next()
                        .filter(|text| !text.trim().is_empty() && !text.starts_with('-'))
                        .ok_or("--reason requires a reason")?
                        .clone();
                }
                "--dry-run" if matches!(operation, Operation::Reap) && !call.dry_run => {
                    call.dry_run = true
                }
                value
                    if matches!(operation, Operation::Close)
                        && !value.starts_with('-')
                        && !value.trim().is_empty()
                        && call.seat.is_none() =>
                {
                    call.seat = Some(value.into())
                }
                _ => {
                    return Err(format!(
                        "unknown or repeated {} argument: {arg}",
                        operation.verb()
                    ));
                }
            }
        }
    } else {
        match operation {
            Operation::Close => {
                call.seat = object
                    .get("seat")
                    .map(|value| nonempty(value, "seat").map(SeatId::from))
                    .transpose()?;
                if let Some(reason) = object.get("reason") {
                    call.reason = nonempty(reason, "reason")?;
                }
            }
            Operation::Reap => {
                if let Some(value) = object.get("dry_run") {
                    call.dry_run = value.as_bool().ok_or("dry_run must be boolean")?;
                }
            }
        }
    }
    if matches!(operation, Operation::Close) && call.seat.is_none() {
        return Err("close requires a seat".to_string());
    }
    Ok(call)
}

fn argument_failure(command: &str, reason: String) -> Response {
    let mut answer: Envelope<Value> = Envelope::refused(command, ErrorKind::Refused, reason);
    answer.details = Some(json!({"code":"E-RS-ARG"}));
    envelope(StatusCode::BAD_REQUEST, &answer)
}

fn ownership_failure(
    command: &str,
    caller: &SeatId,
    seat: &SeatId,
    parent: Option<&SeatId>,
) -> Response {
    let operation = command.strip_prefix("pij ").unwrap_or(command);
    let mut answer: Envelope<Value> = Envelope::refused(
        command,
        ErrorKind::Refused,
        format!("E-RS-OWNERSHIP {operation}: {caller} is neither {seat} nor its recorded parent"),
    );
    answer.details = Some(
        json!({"code":"E-RS-OWNERSHIP","operation":operation,"caller":caller,"seat":seat,"parent":parent}),
    );
    envelope(StatusCode::FORBIDDEN, &answer)
}

pub(super) fn runtime_failure(command: &str, error: PijError) -> Response {
    match &error {
        PijError::GovernanceRefused { code, record } => {
            let mut answer: Envelope<Value> =
                Envelope::refused(command, ErrorKind::Refused, error.to_string());
            answer.details = Some(json!({"code":code,"record":record}));
            envelope(StatusCode::CONFLICT, &answer)
        }
        PijError::NoRegistryEntry { .. } => envelope(
            StatusCode::NOT_FOUND,
            &Envelope::<Value>::refused(command, ErrorKind::Refused, error.to_string()),
        ),
        _ => internal(command, error),
    }
}

#[derive(Serialize)]
pub(super) struct CloseReceipt {
    seat: SeatId,
    tombstoned_at: u64,
    reason: String,
    pub(super) seq: Seq,
}

pub(super) async fn tombstone_receipt(
    spine: &dyn Spine,
    seat: &SeatId,
    expected_seq: Option<Seq>,
    existing: Option<&SeatDescriptor>,
) -> Result<Option<CloseReceipt>> {
    let inconsistent = || PijError::Adapter {
        adapter: "lifecycle/tombstone-receipt".to_string(),
        message: format!("no matching durable tombstone event for {seat}"),
    };
    let Some(event) = spine.latest_matching(seat, &["seat.tombstone"]).await? else {
        return Ok(None);
    };
    let seq = event.seq.ok_or_else(inconsistent)?;
    let payload: Value = serde_json::from_str(&event.payload).map_err(|_| inconsistent())?;
    let reason = payload["reason"].as_str().ok_or_else(inconsistent)?;
    if expected_seq.is_some_and(|expected| expected != seq) {
        return Err(inconsistent());
    }
    if existing.is_some_and(|raw| {
        raw.tombstoned_at != Some(event.at) || raw.tombstone_reason.as_deref() != Some(reason)
    }) {
        return Ok(None);
    }
    Ok(Some(CloseReceipt {
        seat: seat.clone(),
        tombstoned_at: event.at,
        reason: reason.to_string(),
        seq,
    }))
}

pub(crate) async fn close(
    State(state): State<AppState>,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    handle(state, body, Operation::Close).await
}

pub(crate) async fn reap(
    State(state): State<AppState>,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    handle(state, body, Operation::Reap).await
}

async fn handle(
    state: AppState,
    body: std::result::Result<Json<Value>, JsonRejection>,
    operation: Operation,
) -> Response {
    let command = operation.command();
    let call = match body
        .map_err(|error| error.body_text())
        .and_then(|Json(body)| parse(body, operation))
    {
        Ok(call) => call,
        Err(reason) => return argument_failure(command, reason),
    };
    let actor = match resolve_seat(&state, command, call.caller.session_id, call.caller.pane).await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    if call.attribution.iter().any(|claimed| claimed != &actor.id) {
        return ownership_failure(
            command,
            &actor.id,
            call.seat.as_ref().unwrap_or(&actor.id),
            actor.parent.as_ref(),
        );
    }
    match operation {
        Operation::Reap => match reaper::reap(
            state.services.registry.as_ref(),
            state.services.liveness.as_ref(),
            state.services.tmux.as_ref(),
            call.dry_run,
        )
        .await
        {
            Ok(receipt) => envelope(StatusCode::OK, &Envelope::ok(command, receipt)),
            Err(error) => runtime_failure(command, error),
        },
        Operation::Close => {
            let Some(target) = call.seat else {
                return argument_failure(command, "close requires a seat".to_string());
            };
            let expected = match state.services.registry.get(&target).await {
                Ok(Some(expected)) => expected,
                Ok(None) => {
                    return runtime_failure(
                        command,
                        PijError::NoRegistryEntry {
                            seat: target,
                            store: "the daemon's configured registry".to_string(),
                        },
                    );
                }
                Err(error) => return runtime_failure(command, error),
            };
            if actor.id != expected.id && expected.parent.as_ref() != Some(&actor.id) {
                return ownership_failure(command, &actor.id, &target, expected.parent.as_ref());
            }
            let receipt = if expected.tombstoned_at.is_some() {
                tombstone_receipt(
                    state.services.spine.as_ref(),
                    &target,
                    None,
                    Some(&expected),
                )
                .await
            } else {
                match state
                    .services
                    .registry
                    .tombstone_if_unchanged(expected, call.reason)
                    .await
                {
                    Ok(seq) => {
                        tombstone_receipt(state.services.spine.as_ref(), &target, Some(seq), None)
                            .await
                    }
                    Err(error) => Err(error),
                }
            };
            match receipt {
                Ok(Some(receipt)) => envelope(StatusCode::OK, &Envelope::ok(command, receipt)),
                Ok(None) => runtime_failure(
                    command,
                    PijError::Adapter {
                        adapter: "lifecycle/tombstone-receipt".to_string(),
                        message: format!("no matching durable tombstone event for {target}"),
                    },
                ),
                Err(error) => runtime_failure(command, error),
            }
        }
    }
}
