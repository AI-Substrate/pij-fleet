//! `pij output` and `pij outputs` (plan 200, roadmap 001 s06): write a role's
//! output through the single writer, index it, and tell whoever asked.
//!
//! The argv/caller wire is governance's ([`GovernanceRequest`]).
//!
//! Cost: one writer commit per output (git 7, ddocs 2), one store write, and
//! one delivery to the one seat it notifies. Reads launch nothing. No timer or
//! poll.

use std::collections::BTreeMap;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, Msg, SeatDescriptor, SeatId};
use pij_store::{CommunityOutput, ReplyMode, RoleRequest};
use serde_json::{Value, json};

use super::cold_wake;
use super::directory::{caller_path, project_and_repo, refused};
use super::governance::GovernanceRequest;
use super::identity::{Resolved, resolve_seat};
use super::{AppState, envelope};
use crate::community::output::{Evidence, OutputFields};
use crate::community::{Refusal, pointer};

const OUTPUT: &str = "pij output";
const OUTPUTS: &str = "pij outputs";
const SINGLE: [&str; 11] = [
    "role",
    "subject",
    "head",
    "result",
    "link",
    "answers",
    "body",
    "body-file",
    "project",
    "actor",
    "limit",
];
const REPEATED: [&str; 2] = ["note-owed", "evidence"];

#[derive(Debug, Default)]
struct Arguments {
    positions: Vec<String>,
    single: BTreeMap<String, String>,
    repeated: BTreeMap<String, Vec<String>>,
}

impl Arguments {
    fn parse(tokens: &[String]) -> Result<Self, Refusal> {
        let mut call = Self::default();
        let mut tokens = tokens.iter();
        while let Some(token) = tokens.next() {
            let Some(flag) = token.strip_prefix("--") else {
                call.positions.push(token.clone());
                continue;
            };
            if flag == "json" {
                continue;
            }
            let (name, value) = match flag.split_once('=') {
                Some((name, value)) => (name.to_string(), value.to_string()),
                None => {
                    let value = tokens.next().ok_or_else(|| {
                        Refusal::new("E-RS-ARG", format!("--{flag} requires a value"))
                    })?;
                    (flag.to_string(), value.clone())
                }
            };
            if REPEATED.contains(&name.as_str()) {
                call.repeated.entry(name).or_default().push(value);
            } else if SINGLE.contains(&name.as_str()) {
                if call.single.insert(name.clone(), value).is_some() {
                    return Err(Refusal::new("E-RS-ARG", format!("duplicate --{name}")));
                }
            } else {
                return Err(Refusal::new("E-RS-ARG", format!("unsupported --{name}")));
            }
        }
        Ok(call)
    }

    fn get(&self, name: &str) -> Option<String> {
        self.single.get(name).cloned()
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.repeated.get(name).cloned().unwrap_or_default()
    }
}

/// `POST /v1/output` and `POST /v1/outputs`.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let family = request.argv.first().cloned().unwrap_or_default();
    let command = if family == "outputs" { OUTPUTS } else { OUTPUT };
    if request
        .argv
        .iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
    {
        return envelope(
            StatusCode::OK,
            &Envelope::ok(command, json!({"help": pij_core::outputs::help(&family)})),
        );
    }
    let call = match Arguments::parse(request.argv.get(1..).unwrap_or_default()) {
        Ok(call) => call,
        Err(refusal) => return refused(&refusal),
    };
    let actor = match resolve_seat(
        &state,
        command,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    if call
        .get("actor")
        .is_some_and(|claimed| claimed != actor.id.as_str())
    {
        return refused(&Refusal::new(
            "E-RS-OWNERSHIP",
            "--actor does not match the daemon-resolved caller",
        ));
    }
    let result = match family.as_str() {
        "output" => write(&state, &actor, &request.caller, &call).await,
        "outputs" => read(&state, &request.caller, &call).await,
        _ => Err(Refusal::new("E-RS-ARG", "usage: pij output <kind> …")),
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(refusal) => refused(&refusal),
    }
}

async fn write(
    state: &AppState,
    actor: &SeatDescriptor,
    caller: &super::identity::CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let [kind] = call.positions.as_slice() else {
        return Err(Refusal::new(
            "E-RS-ARG",
            "usage: pij output verdict|deliverable|note|handover --role <id> --subject <address> …",
        ));
    };
    if call.get("limit").is_some() {
        return Err(Refusal::new("E-RS-ARG", "--limit belongs to `pij outputs`"));
    }
    let body = match (call.get("body"), call.get("body-file")) {
        (Some(_), Some(_)) => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "give the body as --body or --body-file, not both",
            ));
        }
        (Some(body), None) => body,
        (None, Some(file)) if file == "-" => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "--body-file - reads stdin, which only the pij CLI can do; pass --body or a file path",
            ));
        }
        (None, Some(file)) => {
            let path = caller_path(caller, &file)?;
            std::fs::read_to_string(&path).map_err(|error| {
                Refusal::new(
                    "E-RS-ARG",
                    format!("cannot read {}: {error}", path.display()),
                )
            })?
        }
        (None, None) => String::new(),
    };
    let evidence = call
        .all("evidence")
        .iter()
        .map(|text| evidence(text))
        .collect::<Result<Vec<_>, _>>()?;
    let mut fields = OutputFields {
        kind: kind.clone(),
        role: call.get("role").unwrap_or_default(),
        subject: call.get("subject").unwrap_or_default(),
        head: call.get("head"),
        result: call.get("result"),
        notes_owed: call.all("note-owed"),
        evidence,
        link: call.get("link"),
        answers: call.get("answers"),
        requester: None,
        body,
    };
    crate::community::output::check(&fields)?;
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let store = state.services.directory.store().clone();
    let store_error =
        |error: pij_core::error::PijError| Refusal::new("E-RS-STORE", error.to_string());
    let request = match &fields.answers {
        Some(id) => Some(
            store
                .role_request(id)
                .await
                .map_err(store_error)?
                .ok_or_else(|| {
                    Refusal::new(
                        "E-RS-NOT-FOUND",
                        format!("--answers {id}: no request with that id"),
                    )
                    .with_next("Name the request id the role send printed (its msg_id).")
                })?,
        ),
        None => None,
    };
    let holdings = store
        .open_assignments(actor.id.as_str())
        .await
        .map_err(store_error)?;
    if let Some(request) = &request {
        // pm-001's ruling (security S001 on #137): anyone may publish, but
        // only the request's recorded holder, a live holder of its role or its
        // project's prime may answer it. Checked before anything is written,
        // stamped or sent.
        let recorded = request.holder.as_deref() == Some(actor.id.as_str());
        let holder = holdings
            .iter()
            .any(|row| row.project == request.project && row.role == request.role);
        let prime = !recorded
            && !holder
            && store
                .project(&request.project)
                .await
                .map_err(store_error)?
                .and_then(|record| record.prime_id)
                .is_some_and(|prime| prime == actor.id);
        if !(recorded || holder || prime) {
            return Err(Refusal::new(
                "E-RS-OUTPUT-NOT-HOLDER",
                format!(
                    "--answers {}: {} is not the request's holder, a live holder of {} in project {}, or that project's prime, so it may not answer it; nothing was written",
                    request.id, actor.id, request.role, request.project
                ),
            )
            .with_next(format!(
                "Get assigned ({}), or drop --answers to publish the output without answering",
                crate::community::picture::command(&[
                    "pij",
                    "directory",
                    "assign",
                    &request.role,
                    "<seat>",
                    "--why",
                    "<text>",
                    "--project",
                    &request.project,
                ])
            )));
        }
        fields.requester = Some(request.requester.clone());
    }
    let holds_role = holdings
        .iter()
        .any(|row| row.project == project && row.role == fields.role);
    let (written, path) = state
        .services
        .directory
        .output_add(&project, repo, fields.clone(), &actor.id, holds_role)
        .await?;
    let pointers = pointer::for_write(&project, &written.paths, &written.commit);
    store
        .record_community_pointers(&pointers)
        .await
        .map_err(|error| committed_but(&written.commit, &error))?;
    let now = now_ms();
    store
        .record_output(&CommunityOutput {
            path: path.clone(),
            project: project.clone(),
            role: fields.role.clone(),
            kind: fields.kind.clone(),
            subject: fields.subject.trim().to_string(),
            head: fields.head.clone(),
            result: fields.result.clone(),
            writer: actor.id.to_string(),
            holds_role,
            answers: fields.answers.clone(),
            commit: written.commit.clone(),
            created_at: now,
        })
        .await
        .map_err(|error| committed_but(&written.commit, &error))?;
    if let Some(request) = &request {
        store
            .mark_role_request_answered(&request.id, now)
            .await
            .map_err(|error| committed_but(&written.commit, &error))?;
    }
    let notified = notify(
        state,
        actor,
        &fields,
        request.as_ref(),
        &path,
        &notice_id(&project, &written.commit),
    )
    .await;
    let mut data = json!({
        "project": project,
        "role": fields.role,
        "kind": fields.kind,
        "path": path,
        "commit": written.commit,
        "paths": written.paths,
        "writer_holds_role": holds_role,
        "notified": notified,
    });
    if !holds_role {
        data["warning"] = json!(format!(
            "{} does not hold role {} in project {}; the output records writer_holds_role false",
            actor.id,
            fields.role,
            data["project"].as_str().unwrap_or_default()
        ));
    }
    Ok(data)
}

/// Who an output tells, and how (req-08). One seat at most: the requester in
/// its stored reply mode (a block verdict or a failed result always wakes), or
/// for an output nobody asked for, the seat above the writer as an FYI.
async fn notify(
    state: &AppState,
    writer: &SeatDescriptor,
    fields: &OutputFields,
    request: Option<&RoleRequest>,
    path: &str,
    msg_id: &str,
) -> Value {
    let (target, mode) = match request {
        Some(request) => {
            let mode = if fields.always_wakes() {
                ReplyMode::Wake
            } else {
                request.reply
            };
            (Some(SeatId::from(request.requester.as_str())), mode)
        }
        None => {
            let mode = if fields.always_wakes() {
                ReplyMode::Wake
            } else {
                ReplyMode::Fyi
            };
            match above(state, writer).await {
                Ok(seat) => (seat, mode),
                Err(error) => {
                    return json!({"seat": null, "mode": mode.as_str(), "outcome": format!("not delivered: finding the seat above failed: {error}")});
                }
            }
        }
    };
    let Some(target) = target else {
        return json!({"seat": null, "mode": mode.as_str(), "outcome": "no seat above the writer"});
    };
    if mode == ReplyMode::None || target == writer.id {
        return json!({"seat": target, "mode": mode.as_str(), "outcome": "not told"});
    }
    let msg_id = msg_id.to_string();
    let msg = Msg {
        from: writer.id.clone(),
        from_machine: None,
        to: target.clone(),
        body: notice(writer, fields, path),
        msg_id: msg_id.clone(),
        in_reply_to: fields.answers.clone(),
        command: None,
    };
    let outcome = if mode == ReplyMode::Fyi {
        state
            .services
            .delivery
            .hold_fyi(msg)
            .await
            .map(|receipt| json!(receipt.outcome))
    } else {
        // C4: an answer the requester asked to be woken for wakes it even when
        // cold, as an audited forced wake.
        let reason = format!("output {path} answers the request");
        match cold_wake::guard(
            state,
            OUTPUT,
            &writer.id,
            &target,
            &msg_id,
            cold_wake::Override {
                force: true,
                reason: Some(&reason),
                from_machine: None,
            },
        )
        .await
        {
            Ok(_) => state
                .services
                .delivery
                .accept(msg)
                .await
                .map(|receipt| json!(receipt.outcome)),
            // Nothing is sent after a refused or failed audit; say why.
            Err(refused) => Ok(json!(format!(
                "not delivered: the forced wake was refused: {}",
                refused_reason(*refused).await
            ))),
        }
    };
    json!({
        "seat": target,
        "mode": mode.as_str(),
        "msg_id": msg_id,
        "outcome": outcome.unwrap_or_else(|error| json!(format!("not delivered: {error}"))),
    })
}

/// Why the cold-wake guard refused: its envelope's message, else its status.
async fn refused_reason(response: Response) -> String {
    let status = response.status();
    let message = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|envelope| {
            ["meta", "error"]
                .iter()
                .find_map(|key| envelope[*key].as_str().map(str::to_string))
        });
    message.map_or_else(
        || status.to_string(),
        |message| format!("{status}: {message}"),
    )
}

/// The notice's message id, from the committed output's identity (review of
/// #137, F002): one commit holds one output, so two outputs never share it,
/// while a replay of one notice keeps the same id and stays idempotent.
fn notice_id(project: &str, commit: &str) -> String {
    format!("output-{project}-{commit}")
}

/// The seat above the writer: its recorded parent when live, else the
/// project's current prime. A failed lookup is an error, not "no seat"
/// (review of #137, N001).
async fn above(
    state: &AppState,
    writer: &SeatDescriptor,
) -> Result<Option<SeatId>, pij_core::error::PijError> {
    let live = |seat: &Option<SeatDescriptor>| {
        seat.as_ref()
            .is_some_and(|seat| seat.tombstoned_at.is_none())
    };
    if let Some(parent) = &writer.parent {
        let seat = state.services.registry.get(parent).await?;
        if live(&seat) {
            return Ok(Some(parent.clone()));
        }
    }
    let Some(prime) = state
        .services
        .directory
        .store()
        .prime()
        .await?
        .filter(|designation| designation.state == pij_core::orchestration::PrimeState::Current)
    else {
        return Ok(None);
    };
    let seat = state.services.registry.get(&prime.seat).await?;
    Ok(live(&seat).then_some(prime.seat))
}

fn notice(writer: &SeatDescriptor, fields: &OutputFields, path: &str) -> String {
    let head = fields
        .head
        .as_deref()
        .map_or_else(String::new, |head| format!(" at {head}"));
    let result = fields
        .result
        .as_deref()
        .map_or_else(String::new, |result| format!(": {result}"));
    let answers = fields
        .answers
        .as_deref()
        .map_or_else(String::new, |id| format!(" (answers request {id})"));
    format!(
        "[pij output] {} wrote a {} for role {} on {}{head}{result}{answers}.\nRead it: {path}",
        writer.id,
        fields.kind,
        fields.role,
        fields.subject.trim()
    )
}

/// `pij outputs --subject <address> [--head <sha>]` or `--role <id>`.
async fn read(
    state: &AppState,
    caller: &super::identity::CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let allowed = ["subject", "head", "role", "project", "actor", "limit"];
    if let Some(flag) = call
        .single
        .keys()
        .chain(call.repeated.keys())
        .find(|flag| !allowed.contains(&flag.as_str()))
    {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("outputs: unsupported --{flag}"),
        ));
    }
    if !call.positions.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "usage: pij outputs --subject <address> [--head <sha>] | --role <id> [--limit <n>]",
        ));
    }
    let (project, _) = project_and_repo(state, caller, call.get("project")).await?;
    let store = state.services.directory.store();
    let store_error =
        |error: pij_core::error::PijError| Refusal::new("E-RS-STORE", error.to_string());
    let rows = match (call.get("subject"), call.get("role")) {
        (Some(subject), None) => store
            .outputs_for_subject(&project, subject.trim(), call.get("head").as_deref())
            .await
            .map_err(store_error)?,
        (None, Some(role)) if call.get("head").is_none() => {
            let limit = match call.get("limit") {
                Some(text) => text.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    Refusal::new("E-RS-ARG", "--limit takes a positive whole number")
                })?,
                None => 50,
            };
            store
                .outputs_for_role(&project, &role, limit)
                .await
                .map_err(store_error)?
        }
        _ => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "give --subject <address> [--head <sha>] or --role <id>",
            ));
        }
    };
    let outputs: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "path": row.path, "role": row.role, "kind": row.kind, "subject": row.subject,
                "head": row.head, "result": row.result, "writer": row.writer,
                "writer_holds_role": row.holds_role, "answers": row.answers,
                "commit": row.commit, "at": row.created_at,
            })
        })
        .collect();
    Ok(json!({"project": project, "head": call.get("head"), "outputs": outputs}))
}

/// One `--evidence` row: `{"command": …, "exit_code": …, "log": …}`.
fn evidence(text: &str) -> Result<Evidence, Refusal> {
    let bad = || {
        Refusal::new(
            "E-RS-ARG",
            format!(
                "--evidence takes JSON like {{\"command\":\"just rust-check\",\"exit_code\":0,\"log\":\"<path>\"}}, not {text}"
            ),
        )
    };
    let value: Value = serde_json::from_str(text).map_err(|_| bad())?;
    Ok(Evidence {
        command: value["command"]
            .as_str()
            .filter(|command| !command.trim().is_empty())
            .ok_or_else(bad)?
            .to_string(),
        exit_code: value["exit_code"].as_i64().ok_or_else(bad)?,
        log: value["log"].as_str().map(str::to_string),
    })
}

fn committed_but(commit: &str, error: &pij_core::error::PijError) -> Refusal {
    Refusal::new(
        "E-RS-STORE",
        format!("committed {commit} but could not index it: {error}"),
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    #[test]
    fn flags_repeat_only_for_notes_owed_and_evidence() {
        let call = Arguments::parse(&argv(&[
            "verdict",
            "--note-owed",
            "a",
            "--note-owed=b",
            "--evidence",
            r#"{"command":"x","exit_code":1}"#,
            "--head",
            "abc",
        ]))
        .unwrap();
        assert_eq!(call.all("note-owed"), ["a", "b"]);
        assert_eq!(call.positions, ["verdict"]);
        assert!(Arguments::parse(&argv(&["verdict", "--head", "a", "--head", "b"])).is_err());
        assert!(Arguments::parse(&argv(&["verdict", "--force", "x"])).is_err());
    }

    #[test]
    fn evidence_is_json_with_a_command_and_an_exit_code() {
        let row = evidence(r#"{"command":"just rust-check","exit_code":0,"log":"l"}"#).unwrap();
        assert_eq!(row.exit_code, 0);
        assert_eq!(row.log.as_deref(), Some("l"));
        for bad in ["x", r#"{"command":"x"}"#, r#"{"exit_code":0}"#] {
            assert_eq!(evidence(bad).unwrap_err().code, "E-RS-ARG", "{bad}");
        }
    }
}
