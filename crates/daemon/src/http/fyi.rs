//! `POST /v1/fyi/claim`, `POST /v1/fyi/read` and `POST /v1/activity` (plans 158
//! and 159), and the warm flush of a held pile (plan 159).
//!
//! Both are writes a seat's own harness makes about itself: a typed-turn hook
//! claiming its held FYIs, and a turn boundary publishing busy/idle. The machine
//! bearer does not identify a seat, so each request also carries binding
//! evidence (the pane or the harness-native session), which must match the seat.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::PijError;
use pij_core::fyi::{FLUSH_AT, HOOK_VIAS, Lead, render_block, render_read};
use pij_core::model::{Envelope, ErrorKind, SeatDescriptor, SeatId, SystemState};
use pij_core::ports::SeatFilter;
use serde::{Deserialize, Serialize};

use super::{AppState, envelope, internal, refused};

const CLAIM: &str = "pij fyi-claim";
const READ: &str = "pij fyi-read";
const ACTIVITY: &str = "pij activity";

/// Who is asking, with the evidence that binds the request to the seat.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimRequest {
    #[serde(default)]
    seat: Option<SeatId>,
    #[serde(default)]
    pane: Option<String>,
    #[serde(default)]
    native_session: Option<String>,
    via: String,
}

/// The claimed FYIs, already rendered by the one block definition.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimResponse {
    /// The seat the FYIs were held for.
    pub seat: SeatId,
    /// How many were claimed.
    pub count: usize,
    /// The golden block, or empty when nothing was held.
    pub block: String,
    /// The claimed FYI ids, oldest first.
    pub ids: Vec<String>,
}

pub(crate) async fn claim(
    State(state): State<AppState>,
    Json(request): Json<ClaimRequest>,
) -> Response {
    if !HOOK_VIAS.contains(&request.via.as_str()) {
        return refused(
            CLAIM,
            format!("via must be one of {}", HOOK_VIAS.join(", ")),
        );
    }
    let seat = match bound_seat(
        &state,
        CLAIM,
        request.seat,
        request.pane,
        request.native_session,
    )
    .await
    {
        Ok(seat) => seat,
        Err(response) => return *response,
    };
    let (fyis, claimed_at_ms) = match state
        .services
        .delivery
        .claim_fyis(&seat.id, &request.via)
        .await
    {
        Ok(claimed) => claimed,
        Err(error) => return internal(CLAIM, error),
    };
    let offset = pij_harnesses::proc::local_utc_offset_minutes().unwrap_or(0);
    envelope(
        StatusCode::OK,
        &Envelope::ok(
            CLAIM,
            ClaimResponse {
                seat: seat.id,
                count: fyis.len(),
                block: render_block(&fyis, Lead::Also, claimed_at_ms, offset),
                ids: fyis.into_iter().map(|fyi| fyi.id).collect(),
            },
        ),
    )
}

/// One delivered batch to read in full: the seat and when it was claimed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadRequest {
    seat: SeatId,
    claimed_at_ms: u64,
}

/// A delivered batch, in full (plan 159's digest names the command).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadResponse {
    /// The seat the FYIs were delivered to.
    pub seat: SeatId,
    /// How many that claim delivered.
    pub count: usize,
    /// Every one of them, oldest first, bodies in full; empty when none match.
    pub block: String,
}

/// Read the FYIs one claim delivered. Read-only: nothing changes state, so
/// a digest's reader can see everything it summarised as often as it likes.
pub(crate) async fn read(
    State(state): State<AppState>,
    Json(request): Json<ReadRequest>,
) -> Response {
    let fyis = match state
        .services
        .delivery
        .read_claimed_fyis(&request.seat, request.claimed_at_ms)
        .await
    {
        Ok(fyis) => fyis,
        Err(error) => return internal(READ, error),
    };
    let offset = pij_harnesses::proc::local_utc_offset_minutes().unwrap_or(0);
    envelope(
        StatusCode::OK,
        &Envelope::ok(
            READ,
            ReadResponse {
                seat: request.seat,
                count: fyis.len(),
                block: render_read(&fyis, offset),
            },
        ),
    )
}

/// After an FYI is held (plan 159): warn when it looks like a question, then
/// flush the recipient's pile if this hold brought it to [`FLUSH_AT`] and its
/// cache is known warm at the hold's own time. Only here: a seat starting a
/// turn gets the pile free with that turn, so a turn boundary never flushes.
pub(crate) async fn after_hold(
    state: &AppState,
    recipient: &SeatId,
    question: bool,
    receipt: &mut pij_core::model::Receipt,
) {
    if question {
        receipt.warning = Some(pij_core::fyi::QUESTION_WARNING.to_string());
    }
    flush_if_warm(state, recipient, receipt.at).await;
}

/// Plan 159's warm flush: once [`FLUSH_AT`] FYIs wait for `recipient` and its
/// cache is known warm, deliver them now as one message. Cold or unknown keeps
/// holding, so a flush can never be a cold wake. Best effort: a failure leaves
/// the FYIs pending for the next real turn and never fails the caller.
async fn flush_if_warm(state: &AppState, recipient: &SeatId, now_ms: u64) {
    let delivery = &state.services.delivery;
    match delivery.pending_fyi_count(recipient).await {
        Ok(count) if count >= FLUSH_AT => {}
        Ok(_) => return,
        Err(error) => {
            eprintln!("pij-rs could not count FYIs for {recipient}; not flushing: {error}");
            return;
        }
    }
    let seat = match state.services.registry.get(recipient).await {
        Ok(Some(seat)) if seat.tombstoned_at.is_none() => seat,
        Ok(_) => return,
        Err(error) => {
            eprintln!("pij-rs could not read {recipient} for an FYI flush: {error}");
            return;
        }
    };
    if !super::cold_wake::is_known_warm(state, &seat, now_ms).await {
        return;
    }
    if let Err(error) = delivery.flush_fyis(&seat).await {
        eprintln!("pij-rs could not flush FYIs to {recipient}; they stay held: {error}");
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityRequest {
    /// Optional when `pane` resolves a live seat (the Claude hooks know only the pane).
    #[serde(default)]
    seat: Option<SeatId>,
    #[serde(default)]
    pane: Option<String>,
    #[serde(default)]
    native_session: Option<String>,
    state: String,
}

/// The seat's mechanical state after the publication.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityResponse {
    /// The seat.
    pub seat: SeatId,
    /// `working` or `idle`.
    pub state: String,
    /// Whether this publication changed the stored state.
    pub changed: bool,
}

pub(crate) async fn activity(
    State(state): State<AppState>,
    Json(request): Json<ActivityRequest>,
) -> Response {
    let observed = match request.state.as_str() {
        "working" => SystemState::Working,
        "idle" => SystemState::Idle,
        other => {
            return refused(
                ACTIVITY,
                format!("state must be `working` or `idle`, not `{other}`"),
            );
        }
    };
    let seat = match bound_seat(
        &state,
        ACTIVITY,
        request.seat,
        request.pane,
        request.native_session,
    )
    .await
    {
        Ok(seat) => seat,
        Err(response) => return *response,
    };
    // A targeted state write, never a put of the row read above: a tombstone,
    // role or declaration landing in between must survive (review MEDIUM-2).
    let changed = match state
        .services
        .registry
        .set_activity(&seat.id, observed, None)
        .await
    {
        Ok(seq) => seq.is_some(),
        Err(error) => return internal(ACTIVITY, error),
    };
    envelope(
        StatusCode::OK,
        &Envelope::ok(
            ACTIVITY,
            ActivityResponse {
                seat: seat.id,
                state: observed.as_str().to_string(),
                changed,
            },
        ),
    )
}

/// Resolve the live seat a hook speaks for, and check its binding evidence.
async fn bound_seat(
    state: &AppState,
    command: &str,
    seat: Option<SeatId>,
    pane: Option<String>,
    native_session: Option<String>,
) -> std::result::Result<SeatDescriptor, Box<Response>> {
    if pane.is_none() && native_session.is_none() {
        return Err(Box::new(refused(
            command,
            "binding evidence required: pass the seat's pane or native_session",
        )));
    }
    let registry = &state.services.registry;
    let descriptor = match (&seat, &pane) {
        (Some(seat), _) => registry
            .get(seat)
            .await
            .map_err(|error| Box::new(internal(command, error)))?,
        (None, Some(pane)) => {
            let live: Vec<SeatDescriptor> = registry
                .list(SeatFilter::default())
                .await
                .map_err(|error| Box::new(internal(command, error)))?
                .into_iter()
                .filter(|row| row.tombstoned_at.is_none() && row.pane.as_ref() == Some(pane))
                .collect();
            if live.len() > 1 {
                return Err(Box::new(refused(
                    command,
                    format!(
                        "pane {pane} is bound to {} live seats; name the seat",
                        live.len()
                    ),
                )));
            }
            live.into_iter().next()
        }
        (None, None) => {
            return Err(Box::new(refused(
                command,
                "name the seat, or pass the pane that is bound to it",
            )));
        }
    };
    let Some(descriptor) = descriptor else {
        return Err(Box::new(envelope(
            StatusCode::NOT_FOUND,
            &Envelope::<()>::refused(
                command,
                ErrorKind::NotFound,
                match (seat, pane) {
                    (Some(seat), _) => format!("no seat `{seat}` in this store"),
                    (None, pane) => {
                        format!("no live seat is bound to pane {}", pane.unwrap_or_default())
                    }
                },
            ),
        )));
    };
    if descriptor.tombstoned_at.is_some() {
        return Err(Box::new(envelope(
            StatusCode::GONE,
            &Envelope::<()>::refused(
                command,
                ErrorKind::Refused,
                PijError::SeatIsGone {
                    seat: descriptor.id,
                    tombstone_reason: descriptor.tombstone_reason,
                }
                .to_string(),
            ),
        )));
    }
    if let Some(pane) = &pane
        && descriptor.pane.as_ref() != Some(pane)
    {
        return Err(Box::new(refused(
            command,
            format!("pane {pane} is not bound to seat `{}`", descriptor.id),
        )));
    }
    if let Some(session) = &native_session
        && descriptor.harness_session.as_ref() != Some(session)
    {
        return Err(Box::new(refused(
            command,
            format!(
                "native session {session} is not bound to seat `{}`",
                descriptor.id
            ),
        )));
    }
    Ok(descriptor)
}
