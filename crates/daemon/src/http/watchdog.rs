//! `pij watchdog on|off|status [<seat>] [--every <duration>]` (2026-10-09).
//!
//! PAs are watched by role and need no opt-in. Any other seat is watched only
//! while it is opted in, and any seat may turn any seat's watchdog on or off.
//! Because the change is someone else's, the subject is told: a held FYI
//! names who did it and how to undo it, so a seat never meets an unexplained
//! nudge. The change is persisted before it is announced.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, Event, Msg, SeatId};
use pij_core::watchdog::{PA_ROLE, WatchdogOptIn};
use serde_json::{Value, json};

use super::background::parse_duration_ms;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, system_time_ms};

const COMMAND: &str = "pij watchdog";
const USAGE: &str =
    "expected watchdog on [<seat>] [--every <duration>] | off [<seat>] | status [<seat>]";
/// The shortest interval a nudge can name honestly (it reports minutes).
const MIN_INTERVAL_SECS: u64 = 60;

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
    every_secs: Option<u64>,
    caller: CallerContext,
}

fn refuse(
    status: StatusCode,
    kind: ErrorKind,
    code: &str,
    message: String,
    extra: Value,
) -> Response {
    let mut details = json!({"code": code, "operation": "watchdog"});
    if let (Some(details), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        details.extend(extra.clone());
    }
    let mut answer: Envelope<Value> = Envelope::refused(COMMAND, kind, format!("{code} {message}"));
    answer.details = Some(details);
    envelope(status, &answer)
}

fn invalid(message: impl Into<String>) -> Response {
    refuse(
        StatusCode::BAD_REQUEST,
        ErrorKind::Refused,
        "E-RS-ARG",
        message.into(),
        Value::Null,
    )
}

fn store_failure(cause: impl std::fmt::Display) -> Response {
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorKind::Adapter,
        "E-RS-STORE",
        cause.to_string(),
        Value::Null,
    )
}

fn parse(body: Value) -> std::result::Result<Call, String> {
    let object = body
        .as_object()
        .ok_or_else(|| "watchdog request must be an object".to_string())?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "argv" | "caller"))
    {
        return Err(format!("unknown watchdog request field: {key}"));
    }
    let caller = match object.get("caller") {
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| error.to_string())?,
        None => CallerContext::default(),
    };
    let argv: Vec<String> =
        serde_json::from_value(object.get("argv").cloned().unwrap_or(Value::Null))
            .map_err(|_| USAGE.to_string())?;
    let mut tokens = argv.iter().map(String::as_str);
    if tokens.next() != Some("watchdog") {
        return Err(USAGE.into());
    }
    let action = match tokens.next() {
        Some("on") => Action::On,
        Some("off") => Action::Off,
        Some("status") => Action::Status,
        _ => return Err(USAGE.into()),
    };
    let (mut seat, mut every_secs) = (None, None);
    while let Some(token) = tokens.next() {
        match token {
            "--json" => {}
            "--every" if action == Action::On && every_secs.is_none() => {
                let text = tokens.next().ok_or_else(|| USAGE.to_string())?;
                let secs = parse_duration_ms(text, "--every")? / 1_000;
                if secs < MIN_INTERVAL_SECS {
                    return Err("--every must be at least 1m".into());
                }
                every_secs = Some(secs);
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown or repeated watchdog flag: {flag}"));
            }
            value if seat.is_none() && !value.trim().is_empty() => {
                seat = Some(SeatId::from(value.to_string()));
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Call {
        action,
        seat,
        every_secs,
        caller,
    })
}

/// Record an attempted change that changed nothing: attributed, and kept
/// distinct from a real change, but it notifies nobody (review of #38).
async fn audit_unchanged(
    state: &AppState,
    actor: &SeatId,
    target: &SeatId,
    action: &str,
    now: u64,
) {
    let event = Event {
        seq: None,
        v: 1,
        at: now,
        kind: "watchdog.unchanged".into(),
        seat: Some(target.clone()),
        payload: json!({"seat": target, "actor": actor, "action": action}).to_string(),
    };
    if let Err(cause) = state.services.event_bus.publish(event).await {
        eprintln!("watchdog: audit for {target} failed: {cause}");
    }
}

pub(crate) async fn watchdog(
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
    let services = &state.services;
    let store = services.watchdogs();

    if call.action == Action::Status {
        let mut optins = match store.list_watchdogs().await {
            Ok(optins) => optins,
            Err(cause) => return store_failure(cause),
        };
        let mut pas: Vec<SeatId> = match store.list_roles().await {
            Ok(roles) => roles
                .into_iter()
                .filter(|row| row.role == PA_ROLE)
                .map(|row| row.seat)
                .collect(),
            Err(cause) => return store_failure(cause),
        };
        if let Some(seat) = &call.seat {
            optins.retain(|optin| &optin.seat == seat);
            pas.retain(|pa| pa == seat);
        }
        return envelope(
            StatusCode::OK,
            &Envelope::ok(COMMAND, json!({"pas": pas, "optins": optins})),
        );
    }

    let target_id = call.seat.clone().unwrap_or_else(|| actor.id.clone());
    let target = match services.registry.get(&target_id).await {
        Ok(Some(seat)) if seat.tombstoned_at.is_none() => seat,
        Ok(_) => {
            return refuse(
                StatusCode::NOT_FOUND,
                ErrorKind::Refused,
                "E-RS-NO-SEAT",
                format!("{target_id} is not a live seat"),
                json!({"seat": target_id}),
            );
        }
        Err(cause) => return store_failure(cause),
    };
    match services.roles.read_role(&target.id).await {
        Ok(Some(role)) if role == PA_ROLE => {
            return refuse(
                StatusCode::CONFLICT,
                ErrorKind::Refused,
                "E-RS-WATCHDOG-PA",
                format!(
                    "{} is a PA: PAs always have the watchdog, so it cannot be turned on or off",
                    target.id
                ),
                json!({"seat": target.id}),
            );
        }
        Ok(_) => {}
        Err(cause) => return store_failure(cause),
    }
    let now = match system_time_ms() {
        Ok(now) => now,
        Err(cause) => return store_failure(cause),
    };

    let (receipt, notice) = if call.action == Action::On {
        let interval_secs = call.every_secs.unwrap_or(services.watchdog_default_secs());
        let current = match store.list_watchdogs().await {
            Ok(optins) => optins.into_iter().find(|optin| optin.seat == target.id),
            Err(cause) => return store_failure(cause),
        };
        // Already on at this interval: a no-op notifies nobody and records
        // nothing (review of #38).
        if let Some(current) = current.filter(|current| current.interval_secs == interval_secs) {
            audit_unchanged(&state, &actor.id, &target.id, "on", now).await;
            return envelope(
                StatusCode::OK,
                &Envelope::ok(
                    COMMAND,
                    json!({"seat": current.seat, "enabled": true, "changed": false,
                           "interval_secs": current.interval_secs, "set_by": current.set_by}),
                ),
            );
        }
        let optin = WatchdogOptIn {
            seat: target.id.clone(),
            interval_secs,
            set_by: actor.id.clone(),
            set_at_ms: now,
        };
        if let Err(cause) = store.set_watchdog(&optin).await {
            return store_failure(cause);
        }
        let every = pij_core::cold_wake::human_duration(optin.interval_secs * 1_000);
        (
            json!({"seat": optin.seat, "enabled": true, "changed": true,
                   "interval_secs": optin.interval_secs, "set_by": optin.set_by}),
            format!("{} turned it on (every {every})", actor.id),
        )
    } else {
        let was_on = match store.clear_watchdog(&target.id).await {
            Ok(was_on) => was_on,
            Err(cause) => return store_failure(cause),
        };
        // Already off: a no-op notifies nobody and records nothing.
        if !was_on {
            audit_unchanged(&state, &actor.id, &target.id, "off", now).await;
            return envelope(
                StatusCode::OK,
                &Envelope::ok(
                    COMMAND,
                    json!({"seat": target.id, "enabled": false, "changed": false}),
                ),
            );
        }
        (
            json!({"seat": target.id, "enabled": false, "changed": true, "set_by": actor.id}),
            format!("{} turned it off", actor.id),
        )
    };

    let kind = if call.action == Action::On {
        "watchdog.on"
    } else {
        "watchdog.off"
    };
    if let Err(cause) = services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at: now,
            kind: kind.into(),
            seat: Some(target.id.clone()),
            payload: receipt.to_string(),
        })
        .await
    {
        eprintln!("watchdog: audit for {} failed: {cause}", target.id);
    }

    // Tell the subject when someone else changed its watchdog. Held, so it
    // opens no turn. Coalesced: until the seat reads its FYIs, every control
    // notice shares one id (the store ignores a repeated id), so on/off churn
    // leaves at most one pending notice per seat. The notice names the latest
    // change but sends the seat to `status` for the current setting.
    let mut receipt = receipt;
    if target.id != actor.id {
        let epoch = match services
            .spine
            .latest_matching(&target.id, &["fyi.delivered"])
            .await
        {
            Ok(read) => read.and_then(|event| event.seq).map_or(0, |seq| seq.0),
            Err(cause) => return store_failure(cause),
        };
        let told = services
            .delivery
            .hold_fyi(Msg {
                from: actor.id.clone(),
                to: target.id.clone(),
                body: format!(
                    "[pij watchdog] your watchdog was changed ({notice}). See the current setting: \
                     `pij watchdog status {}`. Stop it any time: `pij watchdog off`.",
                    target.id
                ),
                msg_id: format!("watchdog-control:{}:{epoch}", target.id),
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
    fn grammar_accepts_on_off_status_with_optional_seat_and_interval() {
        let on = call(&["watchdog", "on", "pij-x", "--every", "30m", "--json"]).unwrap();
        assert_eq!(on.action, Action::On);
        assert_eq!(on.seat.as_ref().map(SeatId::as_str), Some("pij-x"));
        assert_eq!(on.every_secs, Some(1_800));
        let off = call(&["watchdog", "off"]).unwrap();
        assert_eq!(
            (off.action, off.seat, off.every_secs),
            (Action::Off, None, None)
        );
        assert_eq!(
            call(&["watchdog", "status"]).unwrap().action,
            Action::Status
        );
    }

    #[test]
    fn grammar_refuses_what_it_cannot_honour() {
        for (argv, why) in [
            (&["watchdog"][..], "no action"),
            (&["watchdog", "pause"][..], "old TS leaf"),
            (
                &["watchdog", "off", "--every", "5m"][..],
                "--every only with on",
            ),
            (&["watchdog", "on", "--every", "30s"][..], "under a minute"),
            (&["watchdog", "on", "a", "b"][..], "two seats"),
            (
                &["watchdog", "on", "--every", "5m", "--every", "9m"][..],
                "repeated flag",
            ),
        ] {
            assert!(call(argv).is_err(), "{why}: {argv:?}");
        }
    }
}
