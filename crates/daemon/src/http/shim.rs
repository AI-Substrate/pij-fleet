//! The **shim-originated** endpoints (plan 119).
//!
//! # Why these are separate routes rather than fields on `/v1/send` and `/v1/inbox`
//!
//! The `pij` CLI's generation shim posts one body shape for every routed verb —
//! `{argv, caller}` (`adapters/generation-router.ts`, `callRs`). `/v1/send` and
//! `/v1/inbox` speak a different language: `SendRequest` wants a caller-supplied
//! `from`, and `InboxQuery` wants a caller-supplied `seat`.
//!
//! The obvious repair — add `caller` to those structs and make `from`/`seat`
//! optional — was REJECTED by the reviewer, and its reason is the one that
//! matters and is not the obvious one. **Those structs tolerate unknown fields.**
//! Measured against the live daemon on 2026-09-01:
//!
//! ```text
//! POST /v1/send {"to":…,"body":…,"msg_id":…,"caller":{"tmuxPane":"%2159"}}
//!   -> 422 "missing field `from`"        <- it failed on `from`, NOT on `caller`
//! ```
//!
//! So an older daemon handed the additive shape does not reject it. It accepts
//! the asserted `from` and **silently discards the caller evidence that exists
//! to constrain it** — the tolerance that looks like forward-compatibility is
//! exactly what makes the additive path unsafe. Overloading fails QUIETLY.
//!
//! A distinct path fails LOUDLY in the direction that matters, and gracefully in
//! the direction that does not:
//!
//! * **Daemon older than this route** — axum answers 404 with an EMPTY body,
//!   which the shim classifies as route-absence and falls back to legacy. Skew
//!   across the endpoint's birth degrades safely, by itself.
//! * **Shim newer than this daemon** — [`ShimRequest`] is
//!   `deny_unknown_fields`, so a field this version does not understand is
//!   REFUSED rather than dropped. That is the direct answer to the reviewer's
//!   third ladder-is-wrong condition: *compatibility must never let a handler
//!   accept the request while ignoring caller evidence.*
//!
//! # What the ladder does here, and what it does not
//!
//! It is **subject resolution only** — never the routing mechanism, and never an
//! authority for the act. `identity::resolve_seat` takes the pane as its
//! strictest evidence, checks it against the live roster, and refuses when a
//! pane and an asserted id DISAGREE rather than preferring the claim. Leniency
//! about ABSENT evidence is not leniency about CONTRADICTED evidence.
//!
//! # Every refusal here answers 400 with an envelope
//!
//! Never a bare 404: the shim reads "404/405 with a body that does not decode as
//! a pij envelope" as ROUTE ABSENCE and falls back to legacy. A refusal wearing
//! that shape would silently re-home the caller into the other store while every
//! surface reported success.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::State;
use axum::response::Response;
use pij_core::model::{Harness, Msg, SeatDescriptor, SeatId};
use pij_core::ports::SeatFilter;

use super::identity::{self, CallerContext, Resolved};
use super::{AppState, envelope, internal, refused};
use crate::delivery::{NativeInboxIdentity, paneless_pull};

pub(crate) const SEND: &str = "pij send";
pub(crate) const INBOX: &str = "pij inbox";

pub(crate) const SESSIONS: &str = "pij sessions";

#[derive(serde::Serialize)]
struct SessionRows {
    rows: Vec<SessionRow>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionRow {
    pij_id: SeatId,
    harness: Harness,
    harness_session_id: Option<String>,
    git_common_dir: String,
    lifecycle: Option<String>,
    bound_model: Option<String>,
    spawned_by: Option<SeatId>,
    transcript_path: Option<String>,
    generation: &'static str,
}

/// Return rs seats in the frozen legacy session-row shape.
///
/// # Composition recipe
///
/// Add a GET `Endpoint::ShimSessions` row at `/v1/shim/sessions` and route it
/// directly to this handler. The TypeScript composer unions these rows with its
/// legacy rows; rs does not translate them again.
pub(crate) async fn shim_sessions(State(state): State<AppState>) -> Response {
    let seats = match state.services.registry.list(SeatFilter::default()).await {
        Ok(seats) => seats,
        Err(error) => return internal(SESSIONS, error),
    };
    let mut git_common_dirs = HashMap::new();
    let rows = seats
        .into_iter()
        .filter(|seat| seat.tombstoned_at.is_none())
        .map(|seat| {
            let git_common_dir = git_common_dirs
                .entry(seat.folder.clone())
                .or_insert_with(|| git_common_dir(Path::new(&seat.folder)))
                .clone();
            SessionRow {
                git_common_dir,
                pij_id: seat.id,
                harness: seat.harness,
                harness_session_id: seat.harness_session,
                lifecycle: None,
                bound_model: seat.model,
                spawned_by: seat.parent,
                transcript_path: None,
                generation: "rs",
            }
        })
        .collect();
    envelope(
        axum::http::StatusCode::OK,
        &pij_core::model::Envelope::ok(SESSIONS, SessionRows { rows }),
    )
}

fn git_common_dir(folder: &Path) -> String {
    discover_git_common_dir(folder)
        .unwrap_or_else(|| folder.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn discover_git_common_dir(folder: &Path) -> Option<PathBuf> {
    for directory in folder.ancestors() {
        let dot_git = directory.join(".git");
        if dot_git.is_dir() {
            return Some(std::fs::canonicalize(&dot_git).unwrap_or(dot_git));
        }
        if !dot_git.is_file() {
            continue;
        }
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let git_dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        let git_dir = resolve_path(directory, Path::new(git_dir));
        let common = std::fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| resolve_path(&git_dir, Path::new(value)))
            })
            .unwrap_or(git_dir);
        return Some(std::fs::canonicalize(&common).unwrap_or(common));
    }
    None
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// The one body every shim-originated route accepts.
///
/// `deny_unknown_fields` is load-bearing — see the module header. It is the
/// property that makes this endpoint fail loudly where an overload would fail
/// quietly, so it is not tidy-up and must not be removed.
///
/// KNOWN RESIDUAL, stated rather than hidden: [`CallerContext`] itself stays
/// field-tolerant, because it is shared with the `/v1` identity routes and
/// giving this endpoint a private copy would be two names for one wire object —
/// the split-brain this whole generation split exists to remove. So a *newer*
/// shim that adds a field INSIDE `caller` still has it silently ignored. The
/// versioned path is the primary guarantee; this is the secondary one, and it
/// covers the outer envelope only.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShimRequest {
    /// `process.argv.slice(2)`, verbatim, exactly as the caller typed it.
    pub argv: Vec<String>,
    /// Forwarded environment. A CLAIM, never an identity: every field is checked
    /// against something the daemon can observe.
    #[serde(default)]
    pub caller: Option<CallerContext>,
    /// The literal body bytes for `pij send <id> --body-file <path|->`.
    ///
    /// Read by the CALLER and sent whole, because only the caller can read it:
    /// the path is relative to the caller's cwd, `-` is the caller's stdin, and
    /// the daemon may not share either. The body is NEVER a token this parser
    /// sees — it does not pass through argv at all — which is deliberate: on the
    /// TypeScript side a body re-appended to argv and re-parsed made a body
    /// starting `--` into a FLAG, and `--wait` silently swallowed a whole file
    /// (`cli.ts`, plan 093 D4). Same defect, avoided the same way.
    #[serde(default)]
    pub body_literal: Option<String>,
}

/// One parsed `pij send` invocation, in the only shapes rs serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SendCall {
    pub to: SeatId,
    pub body: String,
    pub command: Option<String>,
    pub in_reply_to: Option<String>,
    /// Hold for the recipient's next real turn (plan 158).
    pub fyi: bool,
    /// Wake a cold recipient anyway (plan 157 phase 2).
    pub force: bool,
    /// Why the forced wake is worth it.
    pub reason: Option<String>,
}

/// Parse a body send or one allowlisted, bodyless control command.
///
/// Derived from the TypeScript surface callers actually hit
/// (`.pi/extensions/pij/cli.ts:354`), not from a description of it:
///
/// ```text
/// pij send <id> "<text>" | <id> --body-file <path|-> | --to <id> --to <id> "<text>"
///          | <id> --command <name> [--wait] | <id> --fyi "<text>"
///          | <id> --force --reason "<why>" "<text>"
/// ```
///
/// Unsupported broadcast, attachment and wait shapes refuse by name.
pub(crate) fn parse_send(argv: &[String], body_literal: Option<&str>) -> Result<SendCall, String> {
    let mut tokens = argv.iter().map(String::as_str);
    match tokens.next() {
        Some("send") => {}
        Some(other) => return Err(format!("'{other}' is not the send verb")),
        None => return Err("usage: pij send <id> \"<text>\"".to_string()),
    }

    let mut positionals: Vec<&str> = Vec::new();
    let mut saw_body_file = false;
    let mut command = None;
    let mut in_reply_to = None;
    let mut fyi = false;
    let mut force = false;
    let mut reason = None;
    let mut rest = tokens.collect::<Vec<_>>().into_iter().peekable();
    while let Some(token) = rest.next() {
        let flag = token
            .strip_prefix("--")
            .map(|name| match name.split_once('=') {
                Some((head, _)) => head,
                None => name,
            });
        let Some(name) = flag else {
            positionals.push(token);
            continue;
        };
        match name {
            // Served, and its VALUE is consumed here but never used: the bytes
            // travel in `body_literal`, read by the caller.
            "body-file" => {
                saw_body_file = true;
                if !token.contains('=') {
                    rest.next();
                }
            }
            // `--json` is answered by the shim's own projection layer, not here.
            "json" => {}
            "fyi" => fyi = true,
            "force" => force = true,
            "reason" => {
                if reason.is_some() {
                    return Err("--reason may be supplied only once".to_string());
                }
                let value = token
                    .split_once('=')
                    .map(|(_, value)| value)
                    .or_else(|| rest.next())
                    .ok_or_else(|| "--reason needs the text saying why".to_string())?;
                reason = Some(value.to_string());
            }
            "to" => {
                return Err(
                    "rs cannot serve a broadcast `pij send --to <id> --to <id>`: it fans one \
                     typed message out to many recipients as several sends, and rs's endpoint \
                     accepts exactly one destination. Send them individually, or use \
                     PIJ_DAEMON_GENERATION=legacy."
                        .to_string(),
                );
            }
            "command" => {
                if command.is_some() {
                    return Err(
                        "E-RS-CONTROL-INVALID: --command may be supplied only once".to_string()
                    );
                }
                let value = token
                    .split_once('=')
                    .map(|(_, value)| value)
                    .or_else(|| rest.next());
                command = Some(
                    value
                        .ok_or_else(|| "E-RS-CONTROL-INVALID: --command needs a name".to_string())?
                        .to_string(),
                );
            }
            "in-reply-to" => {
                if in_reply_to.is_some() {
                    return Err("--in-reply-to may be supplied only once".to_string());
                }
                let value = token
                    .split_once('=')
                    .map(|(_, value)| value)
                    .or_else(|| rest.next())
                    .filter(|value| !value.trim().is_empty() && !value.starts_with("--"))
                    .ok_or_else(|| "--in-reply-to needs a message id".to_string())?;
                in_reply_to = Some(value.to_string());
            }
            "file" | "caption" => {
                return Err(format!(
                    "rs cannot serve `pij send --{name}`: it attaches a path BY REFERENCE and \
                     only a pull/telegram peer renders it. rs has no attachment model, and \
                     dropping the attachment while reporting the send as sent would be worse \
                     than refusing. Use --body-file to send a file's CONTENTS."
                ));
            }
            "wait" => {
                return Err(
                    "rs cannot serve `pij send --wait`: it blocks the sender on a reply, which \
                     is a legacy inbox affordance rs has no equivalent for."
                        .to_string(),
                );
            }
            other => return Err(format!("unknown send flag `--{other}`")),
        }
    }

    let Some(to) = positionals.first() else {
        return Err("usage: pij send <id> \"<text>\" (no recipient was given)".to_string());
    };

    if fyi && command.is_some() {
        return Err("--fyi holds a message; a --command control cannot be held".to_string());
    }
    if let Some(command) = command {
        if saw_body_file {
            return Err(
                "E-RS-CONTROL-BODY: --command and --body-file are mutually exclusive".to_string(),
            );
        }
        if positionals.len() > 2 {
            return Err(
                "E-RS-CONTROL-BODY: --command cannot carry extra positional text".to_string(),
            );
        }
        pij_core::control::validate_command(
            &command,
            positionals.get(1).copied().unwrap_or_default(),
        )?;
        if let Some(body) = body_literal {
            pij_core::control::validate_command(&command, body)?;
        }
        return Ok(SendCall {
            to: SeatId::from(*to),
            body: String::new(),
            command: Some(command),
            in_reply_to,
            fyi: false,
            force: false,
            reason: None,
        });
    }
    // The two body channels are exclusive, and BOTH must be refused rather than
    // silently preferring one — a caller who typed both has a mistaken model of
    // which one is being sent, and picking for them hides it.
    let inline = positionals.get(1).copied();
    let body = match (saw_body_file, body_literal, inline) {
        (true, _, Some(_)) => {
            return Err(
                "--body-file replaces the body — drop the inline text (pij send <id> \
                 --body-file <path>)"
                    .to_string(),
            );
        }
        (true, Some(literal), None) => literal.to_string(),
        (true, None, None) => {
            return Err(
                "--body-file was typed but no body reached rs. The caller reads the file and \
                 forwards its bytes; this request carried none."
                    .to_string(),
            );
        }
        (false, _, Some(text)) => text.to_string(),
        (false, _, None) => {
            return Err("usage: pij send <id> \"<text>\" (no body was given)".to_string());
        }
    };

    if positionals.len() > 2 {
        return Err(format!(
            "pij send takes one recipient and one body; {} positional arguments were given. \
             Quote the body.",
            positionals.len()
        ));
    }

    Ok(SendCall {
        to: SeatId::from(*to),
        body,
        command: None,
        in_reply_to,
        fyi,
        force,
        reason,
    })
}

/// `POST /v1/shim/send` — one message, sender DERIVED.
///
/// The sender is never asserted. `from` on `/v1/send` is a caller-supplied field
/// with no derivation path, which is the same hole plan 117 closed on `report`:
/// a hand-started seat has no `PIJ_SESSION_ID` to assert, so the verb refused
/// the very seats that most needed it. Here the subject comes from the ladder.
pub(crate) async fn shim_send(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let request = match decode(&body, SEND) {
        Decoded::Request(request) => request,
        Decoded::Refusal(response) => return response,
    };
    if request
        .argv
        .first()
        .is_some_and(|verb| verb == "compact-self")
    {
        if request.argv.iter().skip(1).any(|arg| arg != "--json") || request.body_literal.is_some()
        {
            return refused(
                "pij compact-self",
                "E-RS-CONTROL-BODY: compact-self takes no body or additional arguments",
            );
        }
        return super::send_control(
            &state,
            "pij compact-self",
            super::ControlRequest {
                to: None,
                asserted_from: None,
                body: String::new(),
                command: "compact".to_string(),
                msg_id: state.services.delivery.next_message_id(),
                in_reply_to: None,
                caller: request.caller,
            },
        )
        .await;
    }
    let call = match parse_send(&request.argv, request.body_literal.as_deref()) {
        Ok(call) => call,
        Err(why) => return refused(SEND, why),
    };
    if let Some(command) = call.command {
        return super::send_control(
            &state,
            SEND,
            super::ControlRequest {
                to: Some(call.to),
                asserted_from: None,
                body: call.body,
                command,
                msg_id: state.services.delivery.next_message_id(),
                in_reply_to: call.in_reply_to,
                caller: request.caller,
            },
        )
        .await;
    }
    let from = match subject(&state, SEND, request.caller.as_ref()).await {
        Ok(seat) => seat.id,
        Err(response) => return *response,
    };
    let msg_id = state.services.delivery.next_message_id();
    // FYIs never wake a seat, so only a real send meets the cold-wake guard.
    let cold_check = if call.fyi {
        None
    } else {
        match super::cold_wake::guard(
            &state,
            SEND,
            &from,
            &call.to,
            &msg_id,
            super::cold_wake::Override {
                force: call.force,
                reason: call.reason.as_deref(),
            },
        )
        .await
        {
            Ok(label) => label,
            Err(response) => return *response,
        }
    };
    let msg = Msg {
        from,
        to: call.to,
        body: call.body,
        msg_id,
        from_machine: None,
        in_reply_to: call.in_reply_to,
        command: None,
    };
    let question = call.fyi && pij_core::fyi::looks_like_a_question(&msg.body);
    let recipient = msg.to.clone();
    let accepted = if call.fyi {
        state.services.delivery.hold_fyi(msg).await
    } else {
        state.services.delivery.accept(msg).await
    };
    match accepted {
        Ok(mut receipt) => {
            receipt.cold_check = cold_check;
            if call.fyi {
                super::fyi::after_hold(&state, &recipient, question, &mut receipt).await;
            }
            super::envelope(
                axum::http::StatusCode::OK,
                &pij_core::model::Envelope::ok(SEND, receipt),
            )
        }
        // Every failure below is a REFUSAL with an envelope, never a bare status:
        // the shim classifies an undecodable 404/405 as route absence and falls
        // back to legacy, which would send the message from the other store.
        Err(error) => super::send_failure(SEND, error),
    }
}

/// `POST /v1/shim/inbox` — claim this seat's mail, reader DERIVED.
///
/// `GET /v1/inbox?seat=<id>` is never issued straight from shim argv. The reader
/// is resolved from the caller's pane, and a seat supplied alongside it must
/// AGREE — the ladder refuses a disagreement rather than preferring the claim.
pub(crate) async fn shim_inbox(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let request = match decode(&body, INBOX) {
        Decoded::Request(request) => request,
        Decoded::Refusal(response) => return response,
    };
    let wait = match parse_inbox(&request.argv) {
        Ok(wait) => wait,
        Err(why) => return refused(INBOX, why),
    };
    let reader = match subject(&state, INBOX, request.caller.as_ref()).await {
        Ok(seat) => seat,
        Err(response) => return *response,
    };
    let pull = paneless_pull(&reader);
    if wait != InboxWait::NoWait && !pull {
        return refused(
            INBOX,
            "inbox --wait requires a verified paneless pull seat; pushed seats cannot wait",
        );
    }
    let identity = inbox_identity(request.caller.as_ref(), &reader);
    if wait != InboxWait::NoWait && !identity.matches(&reader) {
        return refused(
            INBOX,
            "inbox --wait requires the registered native session and live host tuple",
        );
    }
    let manual = match manual_native_identity(request.caller.as_ref(), &reader) {
        Ok(identity) => identity,
        Err(reason) => return refused(INBOX, reason),
    };
    let result = if let Some(identity) = manual {
        state
            .services
            .delivery
            .claim_manual_native_inbox(&reader.id, &identity)
            .await
    } else if pull && identity.native_session.is_some() {
        state
            .services
            .delivery
            .claim_pull_inbox(
                &reader.id,
                wait != InboxWait::NoWait,
                match wait {
                    InboxWait::Timeout(duration) => Some(duration),
                    _ => None,
                },
                &identity,
                state.services.liveness.as_ref(),
            )
            .await
    } else {
        state
            .services
            .delivery
            .claim_native_inbox(&reader.id, false, &identity)
            .await
    };
    super::native_inbox_reply(&state, &reader.id, result).await
}

/// The acknowledgement half of a routed inbox read.
///
/// `InboxClaim` is a two-step contract: a claim moves the job to `running` and
/// records nothing, and the CLIENT acknowledges only after it has received and
/// DECODED the message (`delivery/mod.rs`). The native rs client does exactly
/// that (`crates/cli/src/client.rs`). Without this the routed read left the job
/// `running`, recorded no `ReaderRead`, and let the same message be claimed
/// again after the lease expired — rendering "claimed" honestly while the
/// operation was half-finished.
///
/// Why a shim path rather than reusing `/v1/inbox/ack`: that route takes a
/// caller-supplied `seat`, and this endpoint family exists precisely so a seat
/// is never asserted. Here the reader is DERIVED, exactly as the claim was, so
/// the two halves of one read are attributed the same way. (The existing route
/// already ignores its `seat` field for attribution — it keys on the job's
/// serial key — so nothing is weakened by not sending one.)
///
/// External pull readers forward the same native session and host incarnation
/// used at claim time; the service rechecks it before retiring the queue row.
pub(crate) async fn shim_inbox_ack(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Response {
    let request = match decode_ack(&body) {
        DecodedAck::Request(request) => request,
        DecodedAck::Refusal(response) => return response,
    };
    // The reader is derived BEFORE the ack, so an unresolvable caller cannot
    // retire another seat's message.
    let reader = match subject(&state, INBOX, request.caller.as_ref()).await {
        Ok(reader) => reader,
        Err(response) => return *response,
    };
    let identity = match manual_native_identity(request.caller.as_ref(), &reader) {
        Ok(Some(identity)) => identity,
        Ok(None) => inbox_identity(request.caller.as_ref(), &reader),
        Err(reason) => return refused(INBOX, reason),
    };
    match state
        .services
        .delivery
        .acknowledge_inbox(&reader.id, request.job_id, &identity, None)
        .await
    {
        Ok(acknowledged) => super::envelope(
            axum::http::StatusCode::OK,
            &pij_core::model::Envelope::ok(INBOX, acknowledged.origin),
        ),
        Err(error) => super::inbox_failure(error),
    }
}

/// A decoded acknowledgement, or the refusal. Same shape and same reason as
/// [`Decoded`]: an axum `Response` is a large error variant.
enum DecodedAck {
    Request(Box<ShimAckRequest>),
    Refusal(Response),
}

/// The ack body. `deny_unknown_fields` for the same reason [`ShimRequest`] has
/// it: a field this daemon does not understand must be refused, never dropped.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShimAckRequest {
    #[serde(default)]
    pub caller: Option<CallerContext>,
    pub job_id: pij_core::model::JobId,
}

fn decode_ack(body: &[u8]) -> DecodedAck {
    match serde_json::from_slice::<ShimAckRequest>(body) {
        Ok(request) => DecodedAck::Request(Box::new(request)),
        Err(error) => DecodedAck::Refusal(refused(
            INBOX,
            format!(
                "this acknowledgement does not match the shim wire contract: {error}. The \
                 contract is {{caller, job_id}} and unknown fields are REFUSED on purpose."
            ),
        )),
    }
}

/// Optional inbox wait; the only finite completion without a claim is timeout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InboxWait {
    NoWait,
    Infinite,
    Timeout(Duration),
}

/// Registration has its own native `/v1/register` contract, never this argv route.
pub(crate) fn parse_inbox(argv: &[String]) -> Result<InboxWait, String> {
    let mut tokens = argv.iter().map(String::as_str).peekable();
    match tokens.next() {
        Some("inbox") => {}
        Some(other) => return Err(format!("'{other}' is not the inbox verb")),
        None => return Err("usage: pij inbox [check] [--wait [ms]]".to_string()),
    }
    let mut wait = InboxWait::NoWait;
    while let Some(token) = tokens.next() {
        match token {
            "check" | "--json" => {}
            "register" => return Err("pij inbox register uses the native /v1/register endpoint, not the shim inbox route".to_string()),
            token if token == "--wait" || token.starts_with("--wait=") => {
                if wait != InboxWait::NoWait {
                    return Err("--wait may be supplied only once".to_string());
                }
                let value = token.split_once('=').map(|(_, value)| value).or_else(|| {
                    tokens.peek().filter(|next| !next.starts_with("--")).copied()
                        .and_then(|_| tokens.next())
                });
                wait = match value {
                    None => InboxWait::Infinite,
                    Some(value) => {
                        let ms = value.parse::<u64>().ok().filter(|ms| *ms > 0)
                            .ok_or_else(|| "--wait needs a positive integer duration in milliseconds".to_string())?;
                        InboxWait::Timeout(Duration::from_millis(ms))
                    }
                };
            }
            other => return Err(format!("unknown inbox argument `{other}`")),
        }
    }
    Ok(wait)
}

fn inbox_identity(caller: Option<&CallerContext>, seat: &SeatDescriptor) -> NativeInboxIdentity {
    if seat.harness != Harness::Copilot && !paneless_pull(seat) {
        return NativeInboxIdentity::default();
    }
    let Some(caller) = caller else {
        return NativeInboxIdentity::default();
    };
    let native_session = match seat.harness {
        Harness::Claude => caller.claude_session.as_ref(),
        Harness::Copilot => caller.copilot_session.as_ref(),
        Harness::Codex => caller.codex_session.as_ref(),
        _ => None,
    }
    .or(caller.harness_session.as_ref())
    .cloned();
    // Existing non-Copilot asserted-id callers remain machine-grade. A PID
    // without native session evidence is still diagnostic, not a pull identity.
    if seat.harness != Harness::Copilot && native_session.is_none() {
        return NativeInboxIdentity::default();
    }
    NativeInboxIdentity {
        native_session,
        pid: caller.pid,
        proc_start: caller.proc_start,
    }
}

fn manual_native_identity(
    caller: Option<&CallerContext>,
    seat: &SeatDescriptor,
) -> Result<Option<NativeInboxIdentity>, &'static str> {
    if seat.harness != Harness::Copilot || seat.pane.is_none() || !seat.native_extension_delivery {
        return Ok(None);
    }
    let caller =
        caller.ok_or("manual native inbox requires the registered pane and Copilot session")?;
    if caller.pane != seat.pane
        || caller
            .copilot_session
            .as_ref()
            .or(caller.harness_session.as_ref())
            != seat.harness_session.as_ref()
        || seat.harness_session.as_deref().is_none_or(str::is_empty)
    {
        return Err(
            "native incarnation mismatch: manual inbox pane and Copilot session must match the registered seat",
        );
    }
    // The shared ladder already verified the pane. The registry, never a
    // self-asserted/diagnostic CLI PID, supplies the binding at claim and ACK.
    Ok(Some(NativeInboxIdentity {
        native_session: seat.harness_session.clone(),
        pid: seat.proc.map(|proc| proc.pid),
        proc_start: seat.proc.map(|proc| proc.proc_start),
    }))
}

/// A decode's two answers.
///
/// A plain enum rather than `Result<_, Response>`, for the reason
/// [`Resolved`] gives one module over: an axum `Response` is a large error
/// variant, and the two arms are not success-and-failure anyway — a refusal is
/// an answer this route gives on purpose.
enum Decoded {
    Request(Box<ShimRequest>),
    Refusal(Response),
}

/// Decode the shim envelope, naming a shape mismatch instead of letting axum
/// answer a bare 422 the shim would read as a hard rs error with no reason.
fn decode(body: &[u8], command: &'static str) -> Decoded {
    match serde_json::from_slice::<ShimRequest>(body) {
        Ok(request) => Decoded::Request(Box::new(request)),
        Err(error) => Decoded::Refusal(refused(
            command,
            format!(
                "this request does not match the shim wire contract for {command}: {error}. The \
                 contract is {{argv, caller, body_literal}} and unknown fields are REFUSED on \
                 purpose — a field this daemon does not understand must not be silently \
                 dropped, because dropping caller evidence is how an asserted identity goes \
                 unchallenged."
            ),
        )),
    }
}

/// Resolve the acting seat through the shared identity ladder.
///
/// Pane first, checked against the live roster; an asserted id second; a
/// DISAGREEMENT between them refused rather than resolved in either direction.
async fn subject(
    state: &AppState,
    command: &'static str,
    caller: Option<&CallerContext>,
) -> std::result::Result<SeatDescriptor, Box<Response>> {
    let asserted = caller.and_then(|caller| caller.session_id.clone());
    let pane = caller.and_then(|caller| caller.pane.clone());
    match identity::resolve_seat(state, command, asserted, pane).await {
        Resolved::Seat(descriptor, _) => {
            let native = inbox_identity(caller, &descriptor);
            if paneless_pull(&descriptor)
                && (descriptor.harness == Harness::Copilot || native.native_session.is_some())
            {
                if !native.matches(&descriptor) {
                    return Err(Box::new(refused(
                        command,
                        "native incarnation mismatch: caller session and host tuple must match the registered paneless pull seat",
                    )));
                }
                let proc = descriptor.proc.expect("paneless pull has a host identity");
                match state.services.liveness.proc_start(proc.pid).await {
                    Ok(Some(start)) if start == proc.proc_start => {}
                    Ok(_) => {
                        return Err(Box::new(refused(
                            command,
                            "paneless pull host is no longer live at its registered process start",
                        )));
                    }
                    Err(error) => return Err(Box::new(internal(command, error))),
                }
            }
            Ok(descriptor)
        }
        Resolved::Refusal(response) => Err(Box::new(response)),
    }
}
