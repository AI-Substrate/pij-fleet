//! The HTTP side of `pij send role:<id>` (plan 199): `/v1/send` and the shim
//! both call [`route`] before their own seat path, so a role send is resolved,
//! refused, queued or delivered in one place.

use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{DeliveryOutcome, Destination, Envelope, ErrorKind, Msg, SeatId};
use serde_json::{Value, json};

use super::identity::CallerContext;
use super::{AppState, cold_wake, envelope, fyi, send_failure, system_time_ms};
use crate::community::Refusal;
use crate::delivery::role_send::{
    self, E_ROLE_ADDRESS, RoleAddress, RoleOptions, RoleSend, Routed, replay_receipt, request_json,
};
use pij_store::{RequestState, RoleRequest, SqliteOrchestration};

/// One send as either surface parsed it.
pub(crate) struct RoleSendCall<'a> {
    pub(crate) command: &'a str,
    pub(crate) from: &'a SeatId,
    pub(crate) to: &'a Destination,
    pub(crate) body: &'a str,
    pub(crate) msg_id: &'a str,
    pub(crate) in_reply_to: Option<&'a str>,
    pub(crate) fyi: bool,
    pub(crate) control: bool,
    pub(crate) force: bool,
    pub(crate) reason: Option<&'a str>,
    pub(crate) forwarded: bool,
    pub(crate) project: Option<String>,
    pub(crate) options: RoleOptions,
    pub(crate) caller: Option<&'a CallerContext>,
}

/// `None` when this is an ordinary send to a seat: the caller carries on.
/// Otherwise the whole answer: a refusal, a queued request, or the holder's
/// receipt with the request on it.
pub(crate) async fn route(state: &AppState, call: RoleSendCall<'_>) -> Option<Response> {
    let Some(address) = RoleAddress::parse(call.to.seat.as_str()) else {
        if let Err(refusal) = call.options.refuse_on_seat_send() {
            return Some(refused(call.command, &refusal));
        }
        if call.project.is_some() {
            return Some(refused(
                call.command,
                &Refusal::new(
                    role_send::E_ROLE_ONLY,
                    "--project names the directory for a send to role:<id>; a send to a seat has none",
                ),
            ));
        }
        return None;
    };
    Some(match address {
        Err(refusal) => refused(call.command, &refusal),
        Ok(address) => send(state, call, address).await,
    })
}

async fn send(state: &AppState, call: RoleSendCall<'_>, address: RoleAddress) -> Response {
    let command = call.command;
    if call.to.machine.is_some() || call.forwarded {
        return refused(
            command,
            &Refusal::new(
                E_ROLE_ADDRESS,
                format!(
                    "{} is resolved by this machine's directory and is never forwarded; send to the remote seat instead",
                    address.display()
                ),
            ),
        );
    }
    if call.control {
        return refused(
            command,
            &Refusal::new(E_ROLE_ADDRESS, "a control goes to a seat, not to a role"),
        );
    }
    let caller = call.caller.cloned().unwrap_or_default();
    let (project, repo) =
        match super::directory::project_and_repo(state, &caller, call.project.clone()).await {
            Ok(found) => found,
            Err(refusal) => return refused(command, &refusal),
        };
    let now_ms = match system_time_ms() {
        Ok(now) => now,
        Err(error) => return send_failure(command, error),
    };
    let routed = role_send::route(
        &state.services,
        RoleSend {
            from: call.from.clone(),
            address,
            project,
            repo,
            body: call.body.to_string(),
            msg_id: call.msg_id.to_string(),
            in_reply_to: call.in_reply_to.map(str::to_string),
            fyi: call.fyi,
            options: call.options.clone(),
        },
        now_ms,
    )
    .await;
    let store = state.services.governance.store().clone();
    match routed {
        Err(refusal) => refused(command, &refusal),
        Ok(Routed::Queued {
            receipt,
            request,
            woke,
        }) => {
            let mut data = serde_json::to_value(&receipt).unwrap_or(Value::Null);
            data["request"] = request_json(&request);
            data["woke"] = woke;
            envelope(StatusCode::OK, &Envelope::ok(command, data))
        }
        Ok(Routed::Replay { request }) => replay_response(command, &request, now_ms),
        Ok(Routed::Holder {
            seat,
            body,
            mut request,
            recorded,
        }) => {
            // Persist before dispatch: the claim names the holder, so a retry
            // after a rotation resumes to it instead of resolving another.
            if !recorded && let Err(error) = store.record_role_request(&request).await {
                return match store.role_request(&request.id).await {
                    Ok(Some(existing)) if existing.state != RequestState::Delivering => {
                        replay_response(command, &existing, now_ms)
                    }
                    _ => send_failure(command, error),
                };
            }
            let msg = Msg {
                from: call.from.clone(),
                from_machine: None,
                to: seat.clone(),
                body,
                msg_id: call.msg_id.to_string(),
                in_reply_to: call.in_reply_to.map(str::to_string),
                command: None,
            };
            let delivered = if call.fyi {
                let question = pij_core::fyi::looks_like_a_question(call.body);
                match state.services.delivery.hold_fyi(msg).await {
                    Ok(mut receipt) => {
                        fyi::after_hold(state, &seat, question, &mut receipt).await;
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            } else {
                let cold_check = match cold_wake::guard(
                    state,
                    command,
                    call.from,
                    &seat,
                    call.msg_id,
                    cold_wake::Override {
                        force: call.force,
                        reason: call.reason,
                        from_machine: None,
                    },
                )
                .await
                {
                    Ok(label) => label,
                    Err(response) => {
                        abandon(&store, &request).await;
                        return *response;
                    }
                };
                state
                    .services
                    .delivery
                    .accept(msg)
                    .await
                    .map(|mut receipt| {
                        receipt.cold_check = cold_check;
                        receipt
                    })
            };
            let receipt = match delivered {
                Ok(receipt) => receipt,
                Err(error) => {
                    abandon(&store, &request).await;
                    return send_failure(command, error);
                }
            };
            let mut data = serde_json::to_value(&receipt).unwrap_or(Value::Null);
            if matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) {
                abandon(&store, &request).await;
                data["request"] = request_json(&request);
                data["request_warning"] =
                    json!("the holder refused the delivery; no request was kept");
                return envelope(StatusCode::OK, &Envelope::ok(command, data));
            }
            match store
                .mark_role_request_delivered(&request.id, seat.as_str(), now_ms)
                .await
            {
                Ok(_) => {
                    request.state = RequestState::Delivered;
                    request.delivered_at = Some(now_ms);
                }
                Err(cause) => data["request_warning"] = json!(cause.to_string()),
            }
            data["request"] = request_json(&request);
            envelope(StatusCode::OK, &Envelope::ok(command, data))
        }
    }
}

/// A send that did not go through keeps no request, so the sender's retry
/// resolves afresh.
async fn abandon(store: &SqliteOrchestration, request: &RoleRequest) {
    if let Some(holder) = request.holder.as_deref()
        && let Err(cause) = store.abandon_role_request(&request.id, holder).await
    {
        eprintln!("pij-rs: role request {} not abandoned: {cause}", request.id);
    }
}

/// A retry of a request already queued or delivered: the stored request, and
/// a receipt saying nothing was sent again.
fn replay_response(command: &str, request: &RoleRequest, now_ms: u64) -> Response {
    let mut data = serde_json::to_value(replay_receipt(request, now_ms)).unwrap_or(Value::Null);
    data["request"] = request_json(request);
    data["replay"] = json!(true);
    envelope(StatusCode::OK, &Envelope::ok(command, data))
}

fn refused(command: &str, refusal: &Refusal) -> Response {
    let status = match refusal.code {
        "E-RS-STORE" | crate::community::E_GOV_GIT => StatusCode::INTERNAL_SERVER_ERROR,
        "E-RS-NOT-FOUND" => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut body = Envelope::<Value>::refused(
        command,
        if status.is_server_error() {
            ErrorKind::Adapter
        } else {
            ErrorKind::Refused
        },
        refusal.to_string(),
    );
    body.details = Some(json!({"code": refusal.code, "next": refusal.next}));
    envelope(status, &body)
}
