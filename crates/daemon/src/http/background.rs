//! Both CLI generations and REST clients use the same caller-derived job operations.
use axum::extract::{Json, Path, Query, State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::PijError;
use pij_core::model::Envelope;
use serde::Deserialize;
use serde_json::json;

use super::{AppState, CallerContext, envelope, identity, internal, refused};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    #[serde(default)]
    argv: Option<Vec<String>>,
    #[serde(default)]
    caller: CallerContext,
    title: Option<String>,
    command: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadQuery {
    seat: Option<String>,
    pane: Option<String>,
    #[serde(default)]
    all: bool,
    lines: Option<usize>,
}

enum Call {
    Create { title: String, command: String },
    List { all: bool },
    Tail { job: String, lines: usize },
    Kill { job: String },
}

impl Call {
    const fn name(&self) -> &'static str {
        match self {
            Self::Create { .. } => "pij bg create",
            Self::List { .. } => "pij bg list",
            Self::Tail { .. } => "pij bg tail",
            Self::Kill { .. } => "pij bg kill",
        }
    }
}

fn parse(argv: &[String]) -> Result<Call, String> {
    let mut args = argv.iter().map(String::as_str).peekable();
    while args.peek() == Some(&"--json") {
        args.next();
    }
    if args.next() != Some("bg") {
        return Err("expected bg create|list|tail|kill".into());
    }
    while args.peek() == Some(&"--json") {
        args.next();
    }
    let leaf = args.next().ok_or("expected bg create|list|tail|kill")?;
    let (mut title, mut command, mut job, mut lines) = (None, None, None, None);
    let mut all = false;
    while let Some(arg) = args.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg, None), |(flag, value)| (flag, Some(value)));
        match flag {
            "--json" if inline.is_none() => {}
            "--all" if leaf == "list" && inline.is_none() => all = true,
            "--title" if leaf == "create" && title.is_none() => {
                title = Some(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--title requires a value")?
                        .to_owned(),
                )
            }
            "--command" if leaf == "create" && command.is_none() => {
                command = Some(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--command requires a value")?
                        .to_owned(),
                )
            }
            "--lines" if leaf == "tail" && lines.is_none() => {
                lines = Some(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--lines requires a value")?
                        .parse::<usize>()
                        .map_err(|_| "--lines must be an integer in 1..=1000")?,
                );
            }
            _ if !arg.starts_with('-') && matches!(leaf, "tail" | "kill") && job.is_none() => {
                job = Some(arg.to_owned())
            }
            _ => return Err(format!("unexpected argument `{arg}` for bg {leaf}")),
        }
    }
    match leaf {
        "create" => Ok(Call::Create {
            title: title.ok_or("--title is required")?,
            command: command.ok_or("--command is required")?,
        }),
        "list" => Ok(Call::List { all }),
        "tail" => Ok(Call::Tail {
            job: job.ok_or("bg tail requires a job id")?,
            lines: checked_lines(lines)?,
        }),
        "kill" => Ok(Call::Kill {
            job: job.ok_or("bg kill requires a job id")?,
        }),
        _ => Err(format!(
            "unknown bg command `{leaf}`; use create|list|tail|kill"
        )),
    }
}

fn checked_lines(lines: Option<usize>) -> Result<usize, String> {
    match lines.unwrap_or(40) {
        lines @ 1..=1000 => Ok(lines),
        _ => Err("--lines must be an integer in 1..=1000".into()),
    }
}

pub(super) async fn post(
    State(state): State<AppState>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(request)) => request,
        Err(error) => return refused("pij bg", error.body_text()),
    };
    let call = if let Some(argv) = request.argv {
        if request.title.is_some() || request.command.is_some() {
            return refused("pij bg", "argv cannot be mixed with title/command fields");
        }
        parse(&argv)
    } else {
        match (request.title, request.command) {
            (Some(title), Some(command)) => Ok(Call::Create { title, command }),
            _ => Err("bg create requires title and command".into()),
        }
    };
    match call {
        Ok(call) => execute(&state, request.caller, call).await,
        Err(error) => refused("pij bg", error),
    }
}

pub(super) async fn list(
    State(state): State<AppState>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let caller = CallerContext {
        session_id: query.seat,
        pane: query.pane,
        ..CallerContext::default()
    };
    execute(&state, caller, Call::List { all: query.all }).await
}

pub(super) async fn tail(
    State(state): State<AppState>,
    Path(job): Path<String>,
    Query(query): Query<ReadQuery>,
) -> Response {
    let lines = match checked_lines(query.lines) {
        Ok(lines) => lines,
        Err(error) => return refused("pij bg tail", error),
    };
    let caller = CallerContext {
        session_id: query.seat,
        pane: query.pane,
        ..CallerContext::default()
    };
    execute(&state, caller, Call::Tail { job, lines }).await
}

pub(super) async fn kill(
    State(state): State<AppState>,
    Path(job): Path<String>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let request = match body {
        Ok(Json(request)) => request,
        Err(error) => return refused("pij bg kill", error.body_text()),
    };
    if request.argv.is_some() || request.title.is_some() || request.command.is_some() {
        return refused("pij bg kill", "kill accepts only caller context");
    }
    execute(&state, request.caller, Call::Kill { job }).await
}

async fn execute(state: &AppState, caller: CallerContext, call: Call) -> Response {
    let name = call.name();
    let owner = match identity::resolve_seat(state, name, caller.session_id, caller.pane).await {
        identity::Resolved::Seat(owner, _) => owner,
        identity::Resolved::Refusal(response) => return response,
    };
    let jobs = &state.services.background;
    let result = match call {
        Call::Create { title, command } => jobs.create(&owner, &title, &command).await.map(|job| {
            let line = format!("bg started — {} (job {}, pid {}); result will arrive as an injected turn from pij-bg; full output at {}", job.title, job.job_id, job.pid.unwrap_or_default(), job.out_path);
            json!({"job":job.job_id,"title":job.title,"pid":job.pid,"outPath":job.out_path,"line":line})
        }),
        Call::List { all } => jobs.list(&owner.id, all).await.map(|rows| {
            let line = if rows.is_empty() { "no bg jobs".to_owned() } else { rows.iter().map(|job| format!("{}  {:?}{}  {}", job.job_id, job.state, job.exit_code.map_or_else(String::new, |code| format!(" (exit {code})")), job.title)).collect::<Vec<_>>().join("\n") };
            json!({"jobs":rows,"line":line})
        }),
        Call::Tail { job, lines } => jobs.tail(&owner.id, &job, lines).await.map(|(job, lines)| {
            let line = format!("{}  {:?}  {}\n{}\n\n{}", job.job_id, job.state, job.title, job.out_path, lines.join("\n"));
            json!({"job":job.job_id,"state":job.state,"lines":lines,"line":line})
        }),
        Call::Kill { job } => jobs.kill(&owner.id, &job).await.map(|job| {
            let line = format!("bg kill requested — {} (job {}); result will arrive as an injected turn from pij-bg", job.title, job.job_id);
            json!({"job":job.job_id,"state":job.state,"line":line})
        }),
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(name, data)),
        Err(PijError::Adapter { adapter, message }) if adapter == "background/refused" => {
            refused(name, message)
        }
        Err(error) => internal(name, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_parser_keeps_command_values_and_rejects_unbounded_tails() {
        let argv = [
            "bg",
            "create",
            "--title",
            "literal",
            "--command",
            "--json",
            "--json",
        ]
        .map(str::to_owned);
        assert!(
            matches!(parse(&argv).unwrap(), Call::Create { command, .. } if command == "--json")
        );
        for lines in ["0", "1001", "-1", "x"] {
            let argv = ["bg", "tail", "job", "--lines", lines].map(str::to_owned);
            assert!(parse(&argv).is_err());
        }
    }
}
