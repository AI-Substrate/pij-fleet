//! `pij steer on|off|status [<seat>]` (plan 214, ruling C31).
//!
//! A native Copilot seat steers each normal send into a turn its extension
//! started, as OMP and Pi do, unless its steering is off; then those sends wait
//! for the turn to end, as before C31. On by default. Any seat may switch any
//! seat, as with the watchdog; the change is audited, and a seat changed by
//! another is told in a held FYI. The extension reads the setting with the
//! claim it makes during such a turn, so a change needs no restart.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, Event, Msg, SeatId};
use serde_json::{Value, json};

use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, system_time_ms};

const COMMAND: &str = "pij steer";
const USAGE: &str = "expected steer on [<seat>] | off [<seat>] | status [<seat>]";

/// The audit event for a real change.
pub const STEER_EVENT_KIND: &str = crate::delivery::STEERING_CHANGED_KIND;

#[derive(Debug, PartialEq, Eq)]
enum Action {
    On,
    Off,
    Status,
}

#[derive(Debug)]
struct Call {
    action: Action,
    seat: Option<SeatId>,
    caller: CallerContext,
}

fn refuse(status: StatusCode, kind: ErrorKind, code: &str, message: String) -> Response {
    let mut answer: Envelope<Value> = Envelope::refused(COMMAND, kind, format!("{code} {message}"));
    answer.details = Some(json!({"code": code, "operation": "steer"}));
    envelope(status, &answer)
}

fn invalid(message: impl Into<String>) -> Response {
    refuse(
        StatusCode::BAD_REQUEST,
        ErrorKind::Refused,
        "E-RS-ARG",
        message.into(),
    )
}

fn store_failure(cause: impl std::fmt::Display) -> Response {
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorKind::Adapter,
        "E-RS-STORE",
        cause.to_string(),
    )
}

fn parse(body: Value) -> std::result::Result<Call, String> {
    let object = body
        .as_object()
        .ok_or_else(|| "steer request must be an object".to_string())?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "argv" | "caller"))
    {
        return Err(format!("unknown steer request field: {key}"));
    }
    let caller = match object.get("caller") {
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| error.to_string())?,
        None => CallerContext::default(),
    };
    let argv: Vec<String> =
        serde_json::from_value(object.get("argv").cloned().unwrap_or(Value::Null))
            .map_err(|_| USAGE.to_string())?;
    let mut tokens = argv.iter().map(String::as_str);
    if tokens.next() != Some("steer") {
        return Err(USAGE.into());
    }
    let action = match tokens.next() {
        Some("on") => Action::On,
        Some("off") => Action::Off,
        Some("status") => Action::Status,
        _ => return Err(USAGE.into()),
    };
    let mut seat = None;
    for token in tokens {
        match token {
            "--json" => {}
            flag if flag.starts_with('-') => return Err(format!("unknown steer flag: {flag}")),
            value if seat.is_none() && !value.trim().is_empty() => {
                seat = Some(SeatId::from(value.to_string()));
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Call {
        action,
        seat,
        caller,
    })
}

pub(crate) async fn steer(
    State(state): State<AppState>,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return invalid(error.body_text()),
    };
    let call = match parse(body) {
        Ok(call) => call,
        Err(message) => return invalid(message),
    };
    let actor = match resolve_seat(&state, COMMAND, call.caller.session_id, call.caller.pane).await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    let store = state.services.governance.store();
    let target_id = call.seat.clone().unwrap_or_else(|| actor.id.clone());

    if call.action == Action::Status {
        let settings = match store.steering_settings().await {
            Ok(settings) => settings,
            Err(cause) => return store_failure(cause),
        };
        let off: Vec<Value> = settings
            .iter()
            .filter(|setting| !setting.steer)
            .filter(|setting| call.seat.as_ref().is_none_or(|seat| seat.as_str() == setting.seat))
            .map(|setting| {
                json!({"seat": setting.seat, "set_by": setting.set_by, "set_at_ms": setting.set_at_ms})
            })
            .collect();
        let mut data = json!({"default": "on", "off": off});
        if call.seat.is_some() {
            match store.mid_turn_steering(target_id.as_str()).await {
                Ok(steer) => {
                    data["seat"] = json!(target_id);
                    data["steer"] = json!(steer);
                }
                Err(cause) => return store_failure(cause),
            }
        }
        return envelope(StatusCode::OK, &Envelope::ok(COMMAND, data));
    }

    let target = match state.services.registry.get(&target_id).await {
        Ok(Some(seat)) if seat.tombstoned_at.is_none() => seat,
        Ok(_) => {
            return refuse(
                StatusCode::NOT_FOUND,
                ErrorKind::Refused,
                "E-RS-NO-SEAT",
                format!("{target_id} is not a live seat"),
            );
        }
        Err(cause) => return store_failure(cause),
    };
    let now = match system_time_ms() {
        Ok(now) => now,
        Err(cause) => return store_failure(cause),
    };
    let steer = call.action == Action::On;
    let changed = match store
        .set_mid_turn_steering(target.id.as_str(), steer, actor.id.as_str(), now)
        .await
    {
        Ok(changed) => changed,
        Err(cause) => return store_failure(cause),
    };
    let mut receipt = json!({
        "seat": target.id, "steer": steer, "changed": changed, "set_by": actor.id,
        "harness": target.harness,
    });
    if !changed {
        return envelope(StatusCode::OK, &Envelope::ok(COMMAND, receipt));
    }
    if let Err(cause) = state
        .services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at: now,
            kind: STEER_EVENT_KIND.into(),
            seat: Some(target.id.clone()),
            payload: receipt.to_string(),
        })
        .await
    {
        eprintln!("steer: audit for {} failed: {cause}", target.id);
    }
    if target.id != actor.id {
        let told = state
            .services
            .delivery
            .hold_fyi(Msg {
                from: actor.id.clone(),
                to: target.id.clone(),
                body: format!(
                    "[pij steer] {} turned your mid-turn steering {}: {}. \
                     See it with `pij steer status {}`; change it with `pij steer on|off`.",
                    actor.id,
                    if steer { "on" } else { "off" },
                    if steer {
                        "a normal send that arrives during a turn a pij message started now \
                         goes into that turn"
                    } else {
                        "after a pij message starts a turn, the next one waits for that turn \
                         to end, as before C31; a first message into a turn you started \
                         yourself still goes in at once"
                    },
                    target.id
                ),
                msg_id: format!("steer-control:{}:{now}", target.id),
                from_machine: None,
                in_reply_to: None,
                command: None,
            })
            .await;
        receipt["subject_told"] = json!(match told {
            Ok(_) => "held (fyi)".to_string(),
            Err(cause) => format!("error: {cause}"),
        });
    }
    envelope(StatusCode::OK, &Envelope::ok(COMMAND, receipt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(argv: &[&str]) -> std::result::Result<Call, String> {
        parse(json!({"argv": argv}))
    }

    #[test]
    fn grammar_accepts_on_off_status_with_an_optional_seat() {
        let off = call(&["steer", "off", "pij-x", "--json"]).unwrap();
        assert_eq!(off.action, Action::Off);
        assert_eq!(off.seat.as_ref().map(SeatId::as_str), Some("pij-x"));
        assert_eq!(call(&["steer", "on"]).unwrap().seat, None);
        assert_eq!(call(&["steer", "status"]).unwrap().action, Action::Status);
        for argv in [
            &["steer"][..],
            &["steer", "pause"][..],
            &["steer", "on", "a", "b"][..],
            &["steer", "off", "--every", "5m"][..],
        ] {
            assert!(call(argv).is_err(), "{argv:?}");
        }
    }
}
