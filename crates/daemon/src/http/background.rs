//! Both CLI generations and REST clients use the same caller-derived job operations.
use axum::extract::{Json, Path, Query, State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::PijError;
use pij_core::model::Envelope;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path as StdPath, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::background::{
    CreateOptions, DEFAULT_INLINE_MAX, DEFAULT_MIN_INTERVAL_MS, EventsOptions, human_duration,
};
use axum::http::HeaderMap;
use pij_core::background::{BackgroundJob, BackgroundKind, BackgroundState, EventStats};
use pij_core::model::ErrorKind;
use pij_store::background::EmitOutcome;

/// The event hook. Routed OUTSIDE the daemon-key ring: its own per-job token is
/// the credential, so a source needs no daemon key to fire.
pub(super) const EMIT_PATH: &str = "/v1/bg/{job}/emit";

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
    Create {
        title: String,
        command: String,
        cwd: Option<String>,
        timeout_ms: Option<u64>,
        events: Option<EventsOptions>,
    },
    List {
        all: bool,
    },
    Tail {
        job: String,
        lines: usize,
    },
    Kill {
        job: String,
    },
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
    let (mut cwd, mut timeout_ms) = (None, None);
    let (mut events, mut fyi, mut min_interval_ms, mut inline_max) = (false, false, None, None);
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
            "--cwd" if leaf == "create" && cwd.is_none() => {
                cwd = Some(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--cwd requires a value")?
                        .to_owned(),
                )
            }
            "--timeout" if leaf == "create" && timeout_ms.is_none() => {
                timeout_ms = Some(parse_duration_ms(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--timeout requires a value")?,
                    "--timeout",
                )?)
            }
            "--events" if leaf == "create" && inline.is_none() => events = true,
            "--fyi" if leaf == "create" && inline.is_none() => fyi = true,
            "--min-interval" if leaf == "create" && min_interval_ms.is_none() => {
                min_interval_ms = Some(parse_duration_ms(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--min-interval requires a value")?,
                    "--min-interval",
                )?)
            }
            "--inline-max" if leaf == "create" && inline_max.is_none() => {
                inline_max = Some(
                    inline
                        .or_else(|| args.next())
                        .ok_or("--inline-max requires a value")?
                        .parse::<u64>()
                        .map_err(|_| "--inline-max must be a whole number")?,
                );
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
            cwd,
            timeout_ms,
            events: if events {
                Some(EventsOptions {
                    fyi,
                    min_interval_ms: min_interval_ms.unwrap_or(DEFAULT_MIN_INTERVAL_MS),
                    inline_max: inline_max.unwrap_or(DEFAULT_INLINE_MAX),
                })
            } else if fyi || min_interval_ms.is_some() || inline_max.is_some() {
                return Err("--fyi, --min-interval and --inline-max need --events".into());
            } else {
                None
            },
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

/// Parse `90`, `90s`, `5m`, `2h`, `1d` or a sum such as `1h30m` into milliseconds.
/// A bare number is seconds.
fn parse_duration_ms(text: &str, flag: &str) -> Result<u64, String> {
    let invalid = || format!("{flag} must be a duration such as 90s, 5m, 1h30m or 2d");
    let text = text.trim();
    if text.is_empty() {
        return Err(invalid());
    }
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        return text
            .parse::<u64>()
            .ok()
            .and_then(|seconds| seconds.checked_mul(1000))
            .ok_or_else(invalid);
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit: u64 = match ch {
            's' => 1000,
            'm' => 60_000,
            'h' => 3_600_000,
            'd' => 86_400_000,
            _ => return Err(invalid()),
        };
        let value: u64 = digits.parse().map_err(|_| invalid())?;
        digits.clear();
        total = value
            .checked_mul(unit)
            .and_then(|part| total.checked_add(part))
            .ok_or_else(invalid)?;
    }
    if !digits.is_empty() {
        return Err(invalid());
    }
    Ok(total)
}

/// `--cwd` wins and may be relative to the caller's directory; otherwise the
/// caller's own directory; otherwise (no caller cwd) the owner's recorded folder.
fn resolve_cwd(flag: Option<String>, caller: Option<&str>) -> Result<Option<PathBuf>, String> {
    match (flag, caller) {
        (Some(flag), _) if StdPath::new(&flag).is_absolute() => Ok(Some(PathBuf::from(flag))),
        (Some(flag), Some(caller)) => Ok(Some(StdPath::new(caller).join(flag))),
        (Some(_), None) => Err("a relative --cwd needs the caller's working directory".into()),
        (None, caller) => Ok(caller.map(PathBuf::from)),
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
            (Some(title), Some(command)) => Ok(Call::Create {
                title,
                command,
                cwd: None,
                timeout_ms: None,
                events: None,
            }),
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
    let caller_cwd = caller.cwd.clone();
    let owner = match identity::resolve_seat(state, name, caller.session_id, caller.pane).await {
        identity::Resolved::Seat(owner, _) => owner,
        identity::Resolved::Refusal(response) => return response,
    };
    let jobs = &state.services.background;
    let result = match call {
        Call::Create { title, command, cwd, timeout_ms, events } => {
            let cwd = match resolve_cwd(cwd, caller_cwd.as_deref()) {
                Ok(cwd) => cwd,
                Err(error) => return refused(name, error),
            };
            let options = CreateOptions { cwd, timeout_ms, events };
            jobs.create(&owner, &title, &command, options).await.map(|job| {
            let arrives = if job.kind == BackgroundKind::Events { "events arrive in batches, and the end as a final turn, from pij-bg; fire with `pij bg emit` (PIJ_BG_JOB/PIJ_BG_TOKEN are in its environment)" } else { "result will arrive as an injected turn from pij-bg" };
            let line = format!("bg started — {} (job {}, pid {}); {arrives}; full output at {}", job.title, job.job_id, job.pid.unwrap_or_default(), job.out_path);
            json!({"job":job.job_id,"title":job.title,"pid":job.pid,"outPath":job.out_path,"line":line})
        })
        }
        Call::List { all } => match jobs.list(&owner.id, all).await {
            Ok(rows) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX));
                let mut lines = Vec::with_capacity(rows.len());
                let mut values = Vec::with_capacity(rows.len());
                let mut failure = None;
                for job in &rows {
                    let stats = if job.kind == BackgroundKind::Events {
                        match jobs.event_stats(&job.job_id).await {
                            Ok(stats) => Some(stats),
                            Err(error) => {
                                failure = Some(error);
                                break;
                            }
                        }
                    } else {
                        None
                    };
                    lines.push(list_line(job, stats, now));
                    let mut value = json!(job);
                    if let Some(stats) = stats {
                        value["events"] = json!(stats);
                    }
                    values.push(value);
                }
                match failure {
                    Some(error) => Err(error),
                    None => {
                        let line = if lines.is_empty() { "no bg jobs".to_owned() } else { lines.join("\n") };
                        Ok(json!({"jobs":values,"line":line}))
                    }
                }
            }
            Err(error) => Err(error),
        },
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmitRequest {
    text: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// `POST /v1/bg/{job}/emit` with `Authorization: Bearer <PIJ_BG_TOKEN>`.
pub(super) async fn emit(
    State(state): State<AppState>,
    Path(job): Path<String>,
    headers: HeaderMap,
    body: Result<Json<EmitRequest>, JsonRejection>,
) -> Response {
    const NAME: &str = "pij bg emit";
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let request = match body {
        Ok(Json(request)) => request,
        Err(error) => return refused(NAME, error.body_text()),
    };
    match state
        .services
        .background
        .emit(&job, token, &request.text, request.data.as_ref())
        .await
    {
        Ok(EmitOutcome::Accepted { seq }) => envelope(
            StatusCode::OK,
            &Envelope::ok(
                NAME,
                json!({"job": job, "seq": seq, "line": format!("event {seq} fired for {job}")}),
            ),
        ),
        Ok(EmitOutcome::Dropped { dropped }) => envelope(
            StatusCode::OK,
            &Envelope::ok(
                NAME,
                json!({"job": job, "dropped": dropped, "line": format!(
                    "event dropped: {job} already has the maximum pending events ({dropped} dropped since its last batch)"
                )}),
            ),
        ),
        Ok(EmitOutcome::Refused) => refused(NAME, "this source no longer accepts events"),
        Err(PijError::Adapter { adapter, message }) if adapter == "background/auth" => envelope(
            StatusCode::UNAUTHORIZED,
            &Envelope::<()>::refused(NAME, ErrorKind::Auth, message),
        ),
        Err(PijError::Adapter { adapter, message }) if adapter == "background/refused" => {
            refused(NAME, message)
        }
        Err(error) => internal(NAME, error),
    }
}

/// One `bg list` row: running time for live jobs, duration for finished ones.
fn list_line(job: &BackgroundJob, stats: Option<EventStats>, now: u64) -> String {
    let exit = job
        .exit_code
        .map_or_else(String::new, |code| format!(" (exit {code})"));
    let timing = match (job.state, job.finished_at) {
        (BackgroundState::Queued, _) => "queued".to_owned(),
        (BackgroundState::Running, _) => {
            format!(
                "running {}",
                human_duration(now.saturating_sub(job.started_at))
            )
        }
        (_, Some(at)) => format!("took {}", human_duration(at.saturating_sub(job.started_at))),
        (_, None) => "took ?".to_owned(),
    };
    let timeout = if job.timed_out && job.state == BackgroundState::Killed {
        " TIMEOUT"
    } else {
        ""
    };
    let events = stats.map_or_else(String::new, |stats| {
        let last = stats.last_fire_at.map_or_else(
            || "never fired".to_owned(),
            |at| format!("last {} ago", human_duration(now.saturating_sub(at))),
        );
        format!(
            "  events: {} fired, {} pending, {last}",
            stats.fired, stats.pending
        )
    });
    format!(
        "{}  {:?}{exit}{timeout}  {timing}{events}  {}",
        job.job_id, job.state, job.title
    )
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
        let argv = [
            "bg",
            "create",
            "--title",
            "t",
            "--timeout=1h30m",
            "--cwd",
            "sub",
            "--command",
            "true",
        ]
        .map(str::to_owned);
        assert!(matches!(
            parse(&argv).unwrap(),
            Call::Create { timeout_ms: Some(5_400_000), cwd: Some(cwd), .. } if cwd == "sub"
        ));
        for lines in ["0", "1001", "-1", "x"] {
            let argv = ["bg", "tail", "job", "--lines", lines].map(str::to_owned);
            assert!(parse(&argv).is_err());
        }
    }

    #[test]
    fn durations_accept_units_sums_and_bare_seconds_only() {
        for (text, ms) in [
            ("90", 90_000),
            ("90s", 90_000),
            ("5m", 300_000),
            ("1h30m", 5_400_000),
            ("2d", 172_800_000),
        ] {
            assert_eq!(parse_duration_ms(text, "--timeout"), Ok(ms), "{text}");
        }
        for text in ["", "5x", "m", "1h30", "-5s", "99999999999999999999d"] {
            assert!(parse_duration_ms(text, "--timeout").is_err(), "{text}");
        }
    }

    #[test]
    fn cwd_flag_wins_and_is_relative_to_the_caller() {
        assert_eq!(resolve_cwd(None, None), Ok(None));
        assert_eq!(resolve_cwd(None, Some("/c")), Ok(Some(PathBuf::from("/c"))));
        assert_eq!(
            resolve_cwd(Some("sub".into()), Some("/c")),
            Ok(Some(PathBuf::from("/c/sub")))
        );
        assert_eq!(
            resolve_cwd(Some("/abs".into()), Some("/c")),
            Ok(Some(PathBuf::from("/abs")))
        );
        assert!(resolve_cwd(Some("sub".into()), None).is_err());
    }
}
