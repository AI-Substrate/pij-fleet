//! `POST /v1/seat/auth-error` (F212): a Copilot seat's own extension reports
//! that its CLI lost its login. The decision is [`crate::auth_recovery`]'s.
//!
//! Binding evidence is required and must be the native session: the report
//! names the session it came from, which must be the one the seat is bound to,
//! so a restart never reaches a seat the error did not come from.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, Harness, SeatId};
use serde::Deserialize;

use super::{AppState, envelope, internal, refused, system_time_ms};
use crate::auth_recovery::{self, LostLogin};

const COMMAND: &str = "pij auth-error";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthErrorRequest {
    seat: SeatId,
    native_session: String,
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
    let now = match system_time_ms() {
        Ok(now) => now,
        Err(error) => return internal(COMMAND, error),
    };
    match auth_recovery::handle(&state.services, &seat, request.kind, now).await {
        Ok(outcome) => envelope(StatusCode::OK, &Envelope::ok(COMMAND, outcome)),
        Err(error) => internal(COMMAND, error),
    }
}
