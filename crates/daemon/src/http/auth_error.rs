//! `POST /v1/seat/auth-error` (F212): a Copilot seat's own extension reports
//! that its CLI lost its login. The decision is [`crate::auth_recovery`]'s.
//!
//! Binding evidence is required: the native session the report came from,
//! which must be the one the seat is bound to, and the process it runs in,
//! which must be the seat's current one. `/restart` keeps the session and
//! replaces the process, so a late report from the process a restart replaced
//! is refused, never read as the new process losing its login.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, Harness, SeatId};
use serde::Deserialize;

use super::{AppState, envelope, internal, refused, system_time_ms};
use crate::auth_recovery::{self, Cause, Incarnation, LostLogin};

const COMMAND: &str = "pij auth-error";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthErrorRequest {
    seat: SeatId,
    native_session: String,
    pid: u32,
    proc_start: u64,
    kind: LostLogin,
}

pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<AuthErrorRequest>,
) -> Response {
    if request.native_session.trim().is_empty() {
        return refused(
            COMMAND,
            "binding evidence required: the reporting native_session",
        );
    }
    let seat = match super::fyi::bound_seat(
        &state,
        COMMAND,
        Some(request.seat),
        None,
        Some(request.native_session),
    )
    .await
    {
        Ok(seat) => seat,
        Err(response) => return *response,
    };
    if seat.harness != Harness::Copilot {
        return refused(
            COMMAND,
            format!(
                "seat `{}` is not a Copilot seat; only Copilot reports a lost login",
                seat.id
            ),
        );
    }
    let reporter = Incarnation {
        pid: request.pid,
        proc_start: request.proc_start,
    };
    if seat
        .proc
        .is_none_or(|proc| proc.pid != reporter.pid || proc.proc_start != reporter.proc_start)
    {
        return envelope(
            StatusCode::CONFLICT,
            &Envelope::<()>::refused(
                COMMAND,
                ErrorKind::Refused,
                format!(
                    "E-RS-AUTH-STALE process {} is not seat `{}`'s current process; nothing was done",
                    reporter.pid, seat.id
                ),
            ),
        );
    }
    let now = match system_time_ms() {
        Ok(now) => now,
        Err(error) => return internal(COMMAND, error),
    };
    match auth_recovery::handle(
        &state.services,
        &seat,
        reporter,
        Cause::Reported(request.kind),
        now,
    )
    .await
    {
        Ok(outcome) => envelope(StatusCode::OK, &Envelope::ok(COMMAND, outcome)),
        Err(error) => internal(COMMAND, error),
    }
}
