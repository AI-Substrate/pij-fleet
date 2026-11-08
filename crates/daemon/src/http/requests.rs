//! `pij requests`, `pij requests withdraw` and `pij requests close` (plan
//! 212, F59, F83).
//!
//! The role queue had no read verb: neither a request's sender, its holder nor
//! the PA could see whether a request was still queued, and a mistaken one
//! could only be cleared by assigning a holder, who then got it. This lists a
//! project's requests by state and lets the requester, the PA or the prime
//! close a queued one. `close` does the same for a delivered request that was
//! answered or abandoned, which stops its time limit (F83). Store reads, one
//! write and at most one FYI; nothing is launched.

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, Event, Msg, SeatId};
use pij_store::{
    CloseOutcome, ClosedOrOpen, ClosedRoleRequest, REQUESTS_LIST_LIMIT, RequestState, RoleRequest,
};
use serde_json::{Value, json};

use super::directory::project_and_repo;
use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, ResolvedBy, resolve_seat};
use super::{AppState, envelope, system_time_ms};
use crate::community::Refusal;

/// The request a holder already has: it cannot be withdrawn.
pub const E_REQUEST_NOT_QUEUED: &str = "E-RS-REQUEST-NOT-QUEUED";
/// The request was closed before.
pub const E_REQUEST_CLOSED: &str = "E-RS-REQUEST-CLOSED";
/// Only a delivered request can be closed with `close`.
pub const E_REQUEST_NOT_DELIVERED: &str = "E-RS-REQUEST-NOT-DELIVERED";

const LIST_USAGE: &str =
    "pij requests [--role <id>] [--state queued|delivering|delivered|closed] [--project <slug>]";
const WITHDRAW_USAGE: &str = "pij requests withdraw <id> --why <text>";
const CLOSE_USAGE: &str = "pij requests close <id> [--why <text>] [--quiet]";
/// The close's `why` when none is given.
const CLOSE_WHY: &str = "answered or abandoned";

/// `POST /v1/requests`: argv is `requests ...`.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let args = request.argv.get(1..).unwrap_or_default();
    let leaf = args.first().map(String::as_str);
    let command = match leaf {
        Some("withdraw") => "pij requests withdraw",
        Some("close") => "pij requests close",
        _ => "pij requests",
    };
    if pij_core::directory::wants_help(args) {
        return envelope(
            StatusCode::OK,
            &Envelope::ok(command, json!({"help": pij_core::requests::HELP})),
        );
    }
    // A pane is checked by the daemon (and an asserted id that disagrees with
    // it is refused by the ladder); an asserted id alone is only known to
    // exist (security review of #149, S003, F88).
    let (actor, caller) = match resolve_seat(
        &state,
        command,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, by) => (
            seat.id,
            Caller {
                by,
                pane: request.caller.pane.clone(),
                session: request.caller.session_id.clone(),
            },
        ),
        Resolved::Refusal(response) => return response,
    };
    let result = match leaf {
        Some("withdraw") => withdraw(&state, &actor, &caller, &args[1..]).await,
        Some("close") => close(&state, &actor, &caller, &args[1..]).await,
        _ => list(&state, &request.caller, args).await,
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(refusal) => refused(command, &refusal),
    }
}

/// The audit event every withdraw and close writes, in the same transaction
/// as the close (security review of #149, S003).
pub const REQUEST_CLOSED_EVENT: &str = "request.closed_by_actor";

/// How the caller of a mutating verb was identified (S003, F88).
struct Caller {
    by: ResolvedBy,
    pane: Option<String>,
    session: Option<String>,
}

impl Caller {
    /// Bound by a pane the daemon checked, not only an id that exists.
    fn bound(&self) -> bool {
        matches!(self.by, ResolvedBy::Pane)
    }
}

/// Close `request` from `from` with its audit event, appended in the close's
/// own transaction and published once it commits. The event names the
/// request, the verb, the claimed actor and the caller's evidence; no body.
#[allow(clippy::too_many_arguments)]
async fn close_audited(
    state: &AppState,
    actor: &SeatId,
    caller: &Caller,
    request: &RoleRequest,
    from: RequestState,
    why: &str,
    notice: Option<&'static str>,
    verb: &str,
    at: u64,
) -> Result<CloseOutcome, Refusal> {
    let audit = Event {
        seq: None,
        v: 1,
        at,
        kind: REQUEST_CLOSED_EVENT.into(),
        seat: Some(actor.clone()),
        payload: json!({
            "request": request.id,
            "project": request.project,
            "verb": verb,
            "actor": actor,
            "bound": caller.by.as_str(),
            "pane": caller.pane,
            "session": caller.session,
            "quiet": notice == Some("quiet"),
        })
        .to_string(),
    };
    let store = state.services.governance.store().clone();
    let (id, by, why) = (request.id.clone(), actor.to_string(), why.to_string());
    let bound = caller.by.as_str();
    state
        .services
        .event_bus
        .publish_committed_batch(async move {
            store
                .close_role_request_audited(
                    &id,
                    from,
                    &by,
                    &why,
                    at,
                    notice,
                    Some(bound),
                    Some(audit),
                )
                .await
        })
        .await
        .map_err(store_refusal)
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

async fn withdraw(
    state: &AppState,
    actor: &SeatId,
    caller: &Caller,
    args: &[String],
) -> Result<Value, Refusal> {
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
    let authorised = if caller.bound() {
        may_withdraw(state, actor, &request).await?
    } else {
        request.requester == actor.as_str()
    };
    if !authorised {
        return Err(Refusal::new(
            "E-RS-OWNERSHIP",
            format!(
                "request {id} was sent by {}; only its requester, the PA of {} or the prime withdraws it",
                request.requester, request.project
            ),
        ));
    }
    let at = system_time_ms().map_err(store_refusal)?;
    match close_audited(
        state,
        actor,
        caller,
        &request,
        RequestState::Queued,
        &why,
        None,
        "withdraw",
        at,
    )
    .await?
    {
        CloseOutcome::Closed(closed) => {
            let mut data = closed_entry(&closed);
            data["line"] = json!(format!("request {id} withdrawn by {actor}: {why}"));
            Ok(data)
        }
        CloseOutcome::OtherState(request) => Err(not_queued(&request)),
        CloseOutcome::AlreadyClosed(closed) => Err(already_closed(&closed)),
        CloseOutcome::Unknown => Err(unknown(id)),
    }
}

/// `pij requests close <id> [--why] [--quiet]` (F83): close a delivered
/// request that was answered or abandoned, so its time limit stops. Withdraw's
/// authority plus the request's holder. The close records what it owes the
/// requester (`quiet`, or an `owed` FYI) in the same write; the FYI is then
/// sent by an owned task and marked `told` once admitted, so cancelling the
/// caller never abandons it, and a failure is in the printed line. Closing an
/// already-closed request whose FYI is still owed sends it then; the FYI's
/// msg id is the request's, so a retry is never a second message (review of
/// #149, F005).
async fn close(
    state: &AppState,
    actor: &SeatId,
    caller: &Caller,
    args: &[String],
) -> Result<Value, Refusal> {
    let quiet = args.iter().any(|arg| arg == "--quiet");
    let args: Vec<String> = args
        .iter()
        .filter(|arg| *arg != "--quiet")
        .cloned()
        .collect();
    let (positions, flags) = parse(&args, &["why"], CLOSE_USAGE)?;
    let [id] = positions.as_slice() else {
        return Err(Refusal::new("E-RS-ARG", format!("usage: {CLOSE_USAGE}")));
    };
    let why = flag(&flags, "why").unwrap_or_else(|| CLOSE_WHY.to_string());
    let store = state.services.governance.store();
    let request = match store.role_request(id).await.map_err(store_refusal)? {
        Some(request) => request,
        None => match store.closed_role_request(id).await.map_err(store_refusal)? {
            Some(closed) if closed.notice.as_deref() == Some("owed") => {
                // A retry replays the close as it was recorded and audited:
                // its closer, binding and reason, sent by the daemon. The
                // retrying caller only needs the authority to trigger it, and
                // its own --why is not used (ruling C32; review of #149).
                may_close(state, actor, caller, &closed.request).await?;
                let told = notify_owned(state, &closed).await;
                let mut data = closed_entry(&closed);
                let ignored = if flag(&flags, "why").is_some() {
                    " (the close's own reason is sent; this --why is not used)"
                } else {
                    ""
                };
                data["line"] = json!(match &told {
                    Ok(_) => format!(
                        "request {id} was already closed by {}; its notice to {} is sent now{ignored}",
                        closed.closed_by, closed.request.requester
                    ),
                    Err(cause) => not_told_line(
                        &format!("request {id} was already closed by {}", closed.closed_by),
                        &closed,
                        cause,
                    ),
                });
                data["told"] = told_value(&closed, told);
                return Ok(data);
            }
            Some(closed) => return Err(already_closed(&closed)),
            None => return Err(unknown(id)),
        },
    };
    may_close(state, actor, caller, &request).await?;
    let notice = if quiet || request.requester == actor.as_str() {
        "quiet"
    } else {
        "owed"
    };
    let at = system_time_ms().map_err(store_refusal)?;
    let closed = match close_audited(
        state,
        actor,
        caller,
        &request,
        RequestState::Delivered,
        &why,
        Some(notice),
        "close",
        at,
    )
    .await?
    {
        CloseOutcome::Closed(closed) => closed,
        CloseOutcome::OtherState(request) => return Err(not_delivered(&request)),
        CloseOutcome::AlreadyClosed(closed) => return Err(already_closed(&closed)),
        CloseOutcome::Unknown => return Err(unknown(id)),
    };
    let mut data = closed_entry(&closed);
    let line = format!("request {id} closed by {actor}: {why}");
    if notice == "quiet" {
        data["line"] = json!(line);
        data["told"] = Value::Null;
        return Ok(data);
    }
    let told = notify_owned(state, &closed).await;
    data["line"] = json!(match &told {
        Ok(_) => line,
        Err(cause) => not_told_line(&line, &closed, cause),
    });
    data["told"] = told_value(&closed, told);
    Ok(data)
}

/// Withdraw's authority plus the request's holder. A caller known only by
/// an asserted id acts only as the request's requester or holder; PA and
/// prime authority need a caller the daemon bound (S003).
async fn may_close(
    state: &AppState,
    actor: &SeatId,
    caller: &Caller,
    request: &RoleRequest,
) -> Result<(), Refusal> {
    let own =
        request.holder.as_deref() == Some(actor.as_str()) || request.requester == actor.as_str();
    if own || (caller.bound() && may_withdraw(state, actor, request).await?) {
        return Ok(());
    }
    Err(Refusal::new(
        "E-RS-OWNERSHIP",
        format!(
            "request {} was sent by {} to {}; only its requester, its holder, the PA of {} or the prime closes it",
            request.id,
            request.requester,
            request.holder.as_deref().unwrap_or("no holder yet"),
            request.project
        ),
    ))
}

/// Send the close's owed FYI from a task of its own, and mark it `told` once
/// admitted. The caller awaits the task, but cancelling the caller does not
/// cancel it.
async fn notify_owned(
    state: &AppState,
    closed: &ClosedRoleRequest,
) -> Result<pij_core::model::DeliveryOutcome, String> {
    let (state, closed) = (state.clone(), closed.clone());
    tokio::spawn(async move {
        let outcome = tell_requester(&state, &closed).await?;
        state
            .services
            .governance
            .store()
            .close_notice_told(&closed.request.id)
            .await
            .map_err(|cause| format!("the FYI was held, but marking it told failed: {cause}"))?;
        Ok(outcome)
    })
    .await
    .unwrap_or_else(|cause| Err(format!("the notice task failed: {cause}")))
}

fn told_value(
    closed: &ClosedRoleRequest,
    told: Result<pij_core::model::DeliveryOutcome, String>,
) -> Value {
    match told {
        Ok(outcome) => json!({"seat": closed.request.requester, "outcome": outcome}),
        Err(cause) => {
            json!({"seat": closed.request.requester, "outcome": format!("not told: {cause}")})
        }
    }
}

fn not_told_line(head: &str, closed: &ClosedRoleRequest, cause: &str) -> String {
    format!(
        "{head}; but {} was NOT told ({cause}). Run {} again to send the notice",
        closed.request.requester,
        crate::community::picture::command(&["pij", "requests", "close", &closed.request.id])
    )
}

/// One FYI to the requester that its request was closed, from the daemon,
/// never as the closer (ruling C32 (1)): its body names who closed it, how
/// that caller was bound, and the reason recorded at the close. Its msg id is
/// the request's, so a repeat is one message.
async fn tell_requester(
    state: &AppState,
    closed: &ClosedRoleRequest,
) -> Result<pij_core::model::DeliveryOutcome, String> {
    let request = &closed.request;
    let address = crate::delivery::role_send::RoleAddress {
        role: request.role.clone(),
        instance: request.instance.clone(),
    }
    .display();
    let bound = match closed.bound.as_deref() {
        Some("pane (derived)") => "bound by pane",
        Some(_) => "asserted seat id",
        None => "binding not recorded",
    };
    let msg = Msg {
        from: SeatId::from(pij_core::BG_ACTOR),
        from_machine: None,
        to: SeatId::from(request.requester.as_str()),
        body: format!(
            "[pij request closed] your request {} to {address} in {} was closed by {} ({bound}), who wrote: {}",
            pij_core::render::msg_id(&request.id),
            request.project,
            pij_core::render::seat(&closed.closed_by),
            pij_core::render::fenced(&closed.why, pij_core::render::FENCE_MAX)
        ),
        msg_id: pij_core::notice_id::NoticeKind::RequestClosed.id(&request.id),
        in_reply_to: Some(request.id.clone()),
        command: None,
    };
    state
        .services
        .delivery
        .hold_fyi(msg)
        .await
        .map(|receipt| receipt.outcome)
        .map_err(|error| error.to_string())
}

/// The requester; a seat holding the request's project's `pa` role; or the
/// project's recorded prime (`projects.prime_id`).
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
    // Prime authority is the project's recorded prime, as `pij output
    // --answers` reads it; never a role or label a caller can give itself
    // (review of #149, F001; security: HIGH).
    Ok(project_prime.as_ref() == Some(actor))
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
    entry["notice"] = json!(closed.notice);
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

fn not_delivered(request: &RoleRequest) -> Refusal {
    let refusal = Refusal::new(
        E_REQUEST_NOT_DELIVERED,
        format!(
            "request {} is {}; only a delivered request can be closed",
            request.id,
            request.state.as_str()
        ),
    );
    if request.state == RequestState::Queued {
        refusal.with_next(crate::community::picture::command(&[
            "pij",
            "requests",
            "withdraw",
            &request.id,
            "--why",
            "<why>",
        ]))
    } else {
        refusal
    }
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
