//! `pij requests` and `pij requests withdraw` (plan 212, F59).
//!
//! The role queue had no read verb: neither a request's sender, its holder nor
//! the PA could see whether a request was still queued, and a mistaken one
//! could only be cleared by assigning a holder, who then got it. This lists a
//! project's requests by state and lets the requester, the PA or the prime
//! close a queued one. Store reads and one write; nothing is launched.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, SeatId};
use pij_core::orchestration::PrimeState;
use pij_store::{
    CloseOutcome, ClosedOrOpen, ClosedRoleRequest, REQUESTS_LIST_LIMIT, RequestState, RoleRequest,
};
use serde_json::{Value, json};

use super::directory::project_and_repo;
use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, system_time_ms};
use crate::community::Refusal;

/// The request a holder already has: it cannot be withdrawn.
pub const E_REQUEST_NOT_QUEUED: &str = "E-RS-REQUEST-NOT-QUEUED";
/// The request was closed before.
pub const E_REQUEST_CLOSED: &str = "E-RS-REQUEST-CLOSED";

const LIST_USAGE: &str =
    "pij requests [--role <id>] [--state queued|delivering|delivered|closed] [--project <slug>]";
const WITHDRAW_USAGE: &str = "pij requests withdraw <id> --why <text>";

/// `POST /v1/requests`: argv is `requests ...`.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let args = request.argv.get(1..).unwrap_or_default();
    let withdrawing = args.first().is_some_and(|leaf| leaf == "withdraw");
    let command = if withdrawing {
        "pij requests withdraw"
    } else {
        "pij requests"
    };
    if pij_core::directory::wants_help(args) {
        return envelope(
            StatusCode::OK,
            &Envelope::ok(command, json!({"help": pij_core::requests::HELP})),
        );
    }
    let actor = match resolve_seat(
        &state,
        command,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat.id,
        Resolved::Refusal(response) => return response,
    };
    let result = if withdrawing {
        withdraw(&state, &actor, &args[1..]).await
    } else {
        list(&state, &request.caller, args).await
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(refusal) => refused(command, &refusal),
    }
}

/// Positions, and `--name value` (or `--name=value`) pairs.
type Parsed = (Vec<String>, Vec<(String, String)>);

/// Parse a verb's tokens; `--json` is ignored.
fn parse(tokens: &[String], allowed: &[&str], usage: &str) -> Result<Parsed, Refusal> {
    let (mut positions, mut flags) = (Vec::new(), Vec::<(String, String)>::new());
    let mut tokens = tokens.iter();
    while let Some(token) = tokens.next() {
        let Some(flag) = token.strip_prefix("--") else {
            positions.push(token.clone());
            continue;
        };
        if flag == "json" {
            continue;
        }
        let (name, value) = match flag.split_once('=') {
            Some((name, value)) => (name.to_string(), value.to_string()),
            None => (
                flag.to_string(),
                tokens
                    .next()
                    .ok_or_else(|| Refusal::new("E-RS-ARG", format!("--{flag} requires a value")))?
                    .clone(),
            ),
        };
        if !allowed.contains(&name.as_str()) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("unsupported --{name}; usage: {usage}"),
            ));
        }
        if flags.iter().any(|(seen, _)| *seen == name) {
            return Err(Refusal::new("E-RS-ARG", format!("duplicate --{name}")));
        }
        flags.push((name, value));
    }
    Ok((positions, flags))
}

fn flag(flags: &[(String, String)], name: &str) -> Option<String> {
    flags
        .iter()
        .find(|(seen, value)| seen == name && !value.trim().is_empty())
        .map(|(_, value)| value.clone())
}

async fn list(state: &AppState, caller: &CallerContext, args: &[String]) -> Result<Value, Refusal> {
    let (positions, flags) = parse(args, &["role", "state", "project"], LIST_USAGE)?;
    if !positions.is_empty() {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {LIST_USAGE}")));
    }
    let wanted = flag(&flags, "state")
        .map(|text| {
            RequestState::parse(&text).ok_or_else(|| {
                Refusal::new(
                    "E-RS-ARG",
                    format!("--state takes queued, delivering, delivered or closed, not `{text}`"),
                )
            })
        })
        .transpose()?;
    let role = flag(&flags, "role");
    let (project, _) = project_and_repo(state, caller, flag(&flags, "project")).await?;
    let rows = state
        .services
        .governance
        .store()
        .list_role_requests(&project, role.as_deref(), wanted)
        .await
        .map_err(store_refusal)?;
    let requests: Vec<Value> = rows
        .iter()
        .map(|row| match row {
            ClosedOrOpen::Open(request) => listed(request),
            ClosedOrOpen::Closed(closed) => closed_entry(closed),
        })
        .collect();
    Ok(json!({
        "project": project,
        "role": role,
        "state": wanted.map_or("open", RequestState::as_str),
        "limit": REQUESTS_LIST_LIMIT,
        "requests": requests,
    }))
}

async fn withdraw(state: &AppState, actor: &SeatId, args: &[String]) -> Result<Value, Refusal> {
    let (positions, flags) = parse(args, &["why"], WITHDRAW_USAGE)?;
    let [id] = positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {WITHDRAW_USAGE}")));
    };
    let why =
        flag(&flags, "why").ok_or_else(|| Refusal::new("E-RS-ARG", "--why <text> is required"))?;
    let store = state.services.governance.store();
    let request = match store.role_request(id).await.map_err(store_refusal)? {
        Some(request) => request,
        None => match store.closed_role_request(id).await.map_err(store_refusal)? {
            Some(closed) => return Err(already_closed(&closed)),
            None => return Err(unknown(id)),
        },
    };
    if !may_withdraw(state, actor, &request).await? {
        return Err(Refusal::new(
            "E-RS-OWNERSHIP",
            format!(
                "request {id} was sent by {}; only its requester, the PA of {} or the prime withdraws it",
                request.requester, request.project
            ),
        ));
    }
    let at = system_time_ms().map_err(store_refusal)?;
    match store
        .close_role_request(id, actor.as_str(), &why, at)
        .await
        .map_err(store_refusal)?
    {
        CloseOutcome::Closed(closed) => {
            let mut data = closed_entry(&closed);
            data["line"] = json!(format!("request {id} withdrawn by {actor}: {why}"));
            Ok(data)
        }
        CloseOutcome::NotQueued(request) => Err(not_queued(&request)),
        CloseOutcome::AlreadyClosed(closed) => Err(already_closed(&closed)),
        CloseOutcome::Unknown => Err(unknown(id)),
    }
}

/// The requester; a seat holding the request's project's `pa` role; or the
/// prime: the project's prime, the current designation, or a `prime` seat.
async fn may_withdraw(
    state: &AppState,
    actor: &SeatId,
    request: &RoleRequest,
) -> Result<bool, Refusal> {
    if request.requester == actor.as_str() {
        return Ok(true);
    }
    let store = state.services.governance.store();
    let pa = store
        .assignments(&request.project)
        .await
        .map_err(store_refusal)?
        .iter()
        .any(|row| row.to.is_none() && row.role == "pa" && row.seat == actor.as_str());
    if pa {
        return Ok(true);
    }
    let project_prime = store
        .list_projects()
        .await
        .map_err(store_refusal)?
        .into_iter()
        .find(|project| project.slug == request.project)
        .and_then(|project| project.prime_id);
    if project_prime.as_ref() == Some(actor) {
        return Ok(true);
    }
    let designated = store
        .prime()
        .await
        .map_err(store_refusal)?
        .is_some_and(|prime| prime.state == PrimeState::Current && prime.seat == *actor);
    if designated {
        return Ok(true);
    }
    Ok(store
        .list_roles()
        .await
        .map_err(store_refusal)?
        .iter()
        .any(|row| row.role == "prime" && row.seat == *actor))
}

/// A request as listed: no body.
fn listed(request: &RoleRequest) -> Value {
    json!({
        "id": request.id,
        "project": request.project,
        "role": request.role,
        "instance": request.instance,
        "requester": request.requester,
        "reply": request.reply.as_str(),
        "fyi": request.fyi,
        "limit_ms": request.limit_ms,
        "state": request.state.as_str(),
        "holder": request.holder,
        "created_at": request.created_at,
        "delivered_at": request.delivered_at,
    })
}

fn closed_entry(closed: &ClosedRoleRequest) -> Value {
    let mut entry = listed(&closed.request);
    entry["closed_at"] = json!(closed.closed_at);
    entry["closed_by"] = json!(closed.closed_by);
    entry["why"] = json!(closed.why);
    entry
}

fn unknown(id: &str) -> Refusal {
    Refusal::new("E-RS-NOT-FOUND", format!("no role request {id}"))
        .with_next("List them: pij requests --state queued")
}

fn not_queued(request: &RoleRequest) -> Refusal {
    Refusal::new(
        E_REQUEST_NOT_QUEUED,
        format!(
            "request {} is {} to {}; only a queued request can be withdrawn",
            request.id,
            request.state.as_str(),
            request.holder.as_deref().unwrap_or("its holder")
        ),
    )
}

fn already_closed(closed: &ClosedRoleRequest) -> Refusal {
    Refusal::new(
        E_REQUEST_CLOSED,
        format!(
            "request {} was already closed by {}: {}",
            closed.request.id, closed.closed_by, closed.why
        ),
    )
}

fn store_refusal(error: pij_core::error::PijError) -> Refusal {
    Refusal::new("E-RS-STORE", error.to_string())
}

fn refused(command: &str, refusal: &Refusal) -> Response {
    let status = match refusal.code {
        "E-RS-ARG" => StatusCode::BAD_REQUEST,
        "E-RS-OWNERSHIP" => StatusCode::FORBIDDEN,
        "E-RS-NOT-FOUND" => StatusCode::NOT_FOUND,
        "E-RS-STORE" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
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
