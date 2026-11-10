//! `POST /v1/turn-end` (plan 226): run the `pij` block that ends a seat's reply.
//!
//! A harness hands the seat's final reply text here when its turn ends, and
//! only when the text ends with a `pij` fence (a cheap tail check on its side).
//! The block's commands are validated first, every one, by each verb's own
//! parser: if any fails, nothing runs. Then they run in order, as the calling
//! seat, through the same handlers `pij send`, `pij output` and `pij requests
//! close` reach (`/v1/shim/send`, `/v1/output`, `/v1/requests`), so the caller
//! ladder and every refusal are theirs, and the run stops at the first refusal.
//! The answer names each command ran, refused, invalid or not run, with one
//! line for the human and, on failure, the seat's follow-up text. Whether to
//! give the seat that follow-up turn is the harness's call, from its own
//! stop-hook state; the daemon never sees it.
//!
//! The run is spawned as its own task: a harness that gives up waiting cannot
//! cut a block off between two commands, so a block always ends ran in full or
//! stopped at a refusal.
//!
//! Cost: one request per turn end whose reply ends with a block, nothing
//! otherwise. Nothing is launched here; a command launches what its verb
//! launches (an output's writer commit), exactly as from the CLI.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind};
use pij_core::turn_end::{
    self, LineStatus, MAX_COMMANDS, Outcome, TurnEndLine, TurnEndResult, Verb,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::governance::GovernanceRequest;
use super::identity::CallerContext;
use super::shim::{SendCall, ShimRequest};
use super::{AppState, envelope};
use crate::delivery::role_send::RoleOptions;

/// The command name on every answer.
pub const TURN_END: &str = "pij turn-end";

/// The request: the seat's final reply text and its caller evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnEndRequest {
    /// The final assistant reply, whole.
    pub text: String,
    /// The caller's claims, resolved by each verb's own ladder.
    #[serde(default)]
    pub caller: CallerContext,
    /// Which hook is calling (`hook:claude`, `hook:copilot`, `hook:omp`,
    /// `hook:pi`); informational.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

pub(crate) async fn handle(State(state): State<AppState>, body: Bytes) -> Response {
    let request = match serde_json::from_slice::<TurnEndRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return envelope(
                StatusCode::BAD_REQUEST,
                &Envelope::<Value>::refused(
                    TURN_END,
                    ErrorKind::Refused,
                    format!(
                        "E-RS-ARG: this request does not match {TURN_END}'s contract \
                         {{text, caller, via}}: {error}"
                    ),
                ),
            );
        }
    };
    let task = tokio::spawn(async move { run(&state, &request).await });
    match task.await {
        Ok(result) => envelope(StatusCode::OK, &Envelope::ok(TURN_END, result)),
        Err(error) => envelope(
            StatusCode::INTERNAL_SERVER_ERROR,
            &Envelope::<Value>::refused(
                TURN_END,
                ErrorKind::Adapter,
                format!("the turn-end run failed: {error}"),
            ),
        ),
    }
}

/// Why a command failed validation, and its refusal code when it has one.
type Invalid = (String, Option<String>);

async fn run(state: &AppState, request: &TurnEndRequest) -> TurnEndResult {
    let Some(lines) = turn_end::block(&request.text) else {
        return TurnEndResult::none();
    };
    // First pass: every command, by its verb's own parser. Nothing runs yet.
    let checks: Vec<Result<(Verb, Vec<String>), Invalid>> = lines
        .iter()
        .map(|line| {
            if line.line > MAX_COMMANDS {
                return Err((
                    format!("a block holds at most {MAX_COMMANDS} commands"),
                    None,
                ));
            }
            let (verb, argv) = line.parsed.clone().map_err(|why| (why, None))?;
            validate(&request.caller, verb, &argv)?;
            Ok((verb, argv))
        })
        .collect();
    if checks.iter().any(Result::is_err) {
        let rows = lines
            .iter()
            .zip(&checks)
            .map(|(line, check)| match check {
                Ok((verb, _)) => row(line, Some(*verb), LineStatus::NotRun, "not run", None),
                Err((why, code)) => TurnEndLine {
                    code: code.clone(),
                    ..row(
                        line,
                        line.parsed.as_ref().ok().map(|(verb, _)| *verb),
                        LineStatus::Invalid,
                        why,
                        None,
                    )
                },
            })
            .collect();
        return TurnEndResult::from_lines(Outcome::Invalid, rows);
    }
    // Second pass: in order, as the caller, stopping at the first refusal.
    let mut rows = Vec::with_capacity(lines.len());
    let mut refused = false;
    for (line, check) in lines.iter().zip(checks) {
        let Ok((verb, argv)) = check else {
            unreachable!("every check passed")
        };
        if refused {
            rows.push(row(line, Some(verb), LineStatus::NotRun, "not run", None));
            continue;
        }
        let answer = read(dispatch(state, &request.caller, verb, argv).await).await;
        match turn_end::judge(
            answer.ok,
            answer.data.as_ref(),
            answer.meta.as_deref(),
            answer.details.as_ref(),
            answer.error.as_deref(),
        ) {
            Ok(detail) => rows.push(row(line, Some(verb), LineStatus::Ran, &detail, answer.data)),
            Err((detail, code)) => {
                refused = true;
                rows.push(TurnEndLine {
                    code,
                    ..row(line, Some(verb), LineStatus::Refused, &detail, answer.data)
                });
            }
        }
    }
    let outcome = if refused {
        Outcome::Refused
    } else {
        Outcome::Ran
    };
    TurnEndResult::from_lines(outcome, rows)
}

fn row(
    line: &turn_end::Line,
    verb: Option<Verb>,
    status: LineStatus,
    detail: &str,
    result: Option<Value>,
) -> TurnEndLine {
    TurnEndLine {
        line: line.line,
        command: line.text.clone(),
        verb,
        status,
        detail: detail.to_string(),
        code: None,
        result,
    }
}

/// A route's refusal as an invalid line: its first line, and its code.
fn invalid(refusal: crate::community::Refusal) -> Invalid {
    let text = refusal.to_string();
    (
        text.lines().next().unwrap_or_default().to_string(),
        Some(refusal.code.to_string()),
    )
}

/// Each verb's own argument checks, with nothing written or sent.
fn validate(caller: &CallerContext, verb: Verb, argv: &[String]) -> Result<(), Invalid> {
    match verb {
        Verb::Send => {
            if argv.iter().any(|arg| arg.starts_with("--body-file")) {
                return Err((
                    "a block cannot read a body file; write the body inline, quoted".to_string(),
                    None,
                ));
            }
            let call = super::shim::parse_send(argv, None).map_err(|why| (why, None))?;
            send_rules(caller, &call)
        }
        Verb::Output => super::output::validate(caller, &argv[1..]).map_err(invalid),
        Verb::RequestsClose => super::requests::parse_close(&argv[2..])
            .map(|_| ())
            .map_err(invalid),
    }
}

/// The send path's rules that read nothing but the line, each the function
/// the send path itself calls, so a line it would refuse on its arguments
/// alone runs nothing (review of #212, F002). A line they refuse, the send
/// path refuses too, only later, after earlier lines had run. The one rule
/// stricter here: `--force` needs its reason on every non-FYI send, a role
/// send that would only queue included, as `pij send --help` says.
fn send_rules(caller: &CallerContext, call: &SendCall) -> Result<(), Invalid> {
    if let Some(attachment) = &call.attachment {
        return super::shim::attachment_path(&attachment.path, Some(caller))
            .map(|_| ())
            .map_err(|why| (why, None));
    }
    // A control goes straight to its seat; parse_send has checked all of it.
    if call.command.is_some() {
        return Ok(());
    }
    let options = RoleOptions {
        reply: call.reply.clone(),
        limit: call.limit.clone(),
    };
    let role = super::role_send::check(&call.to, false, false, call.project.as_deref(), &options)
        .map_err(invalid)?;
    if role.is_some() {
        super::role_send::check_values(&options).map_err(invalid)?;
    }
    if !call.fyi {
        super::cold_wake::forced_reason(call.force, call.reason.as_deref())
            .map_err(|why| (why, Some(pij_core::cold_wake::COLD_WAKE_CODE.to_string())))?;
    }
    Ok(())
}

/// Run one command through its verb's own handler.
async fn dispatch(
    state: &AppState,
    caller: &CallerContext,
    verb: Verb,
    argv: Vec<String>,
) -> Response {
    match verb {
        Verb::Send => {
            super::shim::send_as_caller(
                state,
                ShimRequest {
                    argv,
                    caller: Some(caller.clone()),
                    body_literal: None,
                },
            )
            .await
        }
        Verb::Output => {
            super::output::handle(
                State(state.clone()),
                Json(GovernanceRequest {
                    argv,
                    caller: caller.clone(),
                }),
            )
            .await
        }
        Verb::RequestsClose => {
            super::requests::handle(
                State(state.clone()),
                Json(GovernanceRequest {
                    argv,
                    caller: caller.clone(),
                }),
            )
            .await
        }
    }
}

/// A verb route's answer, as the fields [`turn_end::judge`] reads.
struct Answer {
    ok: bool,
    data: Option<Value>,
    meta: Option<String>,
    details: Option<Value>,
    error: Option<String>,
}

async fn read(response: Response) -> Answer {
    let status = response.status();
    let unreadable = |why: String| Answer {
        ok: false,
        data: None,
        meta: Some(format!(
            "the route's answer (HTTP {status}) was unreadable: {why}"
        )),
        details: None,
        error: None,
    };
    let bytes = match axum::body::to_bytes(response.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => return unreadable(error.to_string()),
    };
    match serde_json::from_slice::<Envelope<Value>>(&bytes) {
        Ok(envelope) => Answer {
            ok: envelope.ok,
            data: envelope.data,
            meta: envelope.meta,
            details: envelope.details,
            error: envelope
                .error
                .and_then(|kind| serde_json::to_value(kind).ok())
                .and_then(|value| value.as_str().map(str::to_string)),
        },
        Err(error) => unreadable(error.to_string()),
    }
}
