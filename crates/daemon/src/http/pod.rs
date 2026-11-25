//! `pij pod plan|up|show|confirm` (PRD 008 s02, plan 248; blueprint 005
//! c-pod-verbs, c-pod-checks).
//!
//! `plan` reads the template, fills its parameters, builds each slot's first
//! turn and runs the checks: it writes and launches nothing. `up` does the
//! same, commits the pod's record, starts each slot through the one spawn path
//! `pij spawn --as` takes, commits the members, and sends each slot that names
//! tools one canary message. One task then waits, bounded, for every canary
//! answer on a subscription and commits the outcome once. Every check warns
//! and is logged; nothing refuses on one (decision d-12).
//!
//! Cost: plan and show launch nothing (in-process reads and indexed store
//! reads). up: one spawn per slot (as `spawn --as`), three writer commits (the
//! record, its members, the canary outcome), one canary message per slot that
//! names tools and one bounded wait for them all. No loop, poll or timer.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::events::EventFilter;
use pij_core::model::{Envelope, Event, Harness, Msg, SeatId};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use super::directory::{project_and_repo, refused_as};
use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::types::{AsRole, SpawnRequest};
use super::{AppState, ExistingSeatPolicy, envelope};
use crate::community::first_turn::{self, Creator};
use crate::community::layout::RolePaths;
use crate::community::pod::{self, PodPaths, SlotPlan, Template, Warning};
use crate::community::writer::{Existing, NewFile};
use crate::community::{Refusal, holders, role, root};

const COMMAND: &str = "pij pod";

/// A seat's answer to its tool canary (`pij pod confirm`).
pub const CANARY_EVENT_KIND: &str = "pod.canary";

/// How long `up` waits for the canary answers, from the canary's send. Long
/// enough for a seat to bind, take its first turn and answer.
pub const CANARY_BOUND: Duration = Duration::from_secs(15 * 60);

/// A pod that already has a record.
pub const E_POD_EXISTS: &str = "E-RS-POD-EXISTS";

/// One parsed `pij pod` call.
#[derive(Debug, Default, PartialEq, Eq)]
struct Call {
    verb: String,
    positions: Vec<String>,
    sets: Vec<(String, String)>,
    name: Option<String>,
    session: Option<String>,
    limit: Option<usize>,
    fresh: bool,
    no_start: bool,
    tools: Vec<String>,
}

fn parse(argv: &[String]) -> Result<Call, Refusal> {
    let mut args = argv.iter().skip(1);
    let verb = args
        .next()
        .filter(|verb| !verb.starts_with('-'))
        .cloned()
        .ok_or_else(|| Refusal::new("E-RS-ARG", "name a verb: pij pod --help"))?;
    let mut call = Call {
        verb,
        ..Call::default()
    };
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| Refusal::new("E-RS-ARG", format!("{flag} needs a value")))
        };
        match arg.as_str() {
            "--set" => {
                let set = value("--set")?;
                let (name, given) = set
                    .split_once('=')
                    .ok_or_else(|| Refusal::new("E-RS-ARG", "--set takes <name>=<value>"))?;
                call.sets.push((name.trim().to_string(), given.to_string()));
            }
            "--name" => call.name = Some(value("--name")?),
            "--session" => call.session = Some(value("--session")?),
            "--limit" => {
                call.limit = Some(
                    value("--limit")?
                        .parse()
                        .ok()
                        .filter(|limit| *limit > 0)
                        .ok_or_else(|| {
                            Refusal::new("E-RS-ARG", "--limit takes a whole number above 0")
                        })?,
                );
            }
            "--tool" => call.tools.push(value("--tool")?),
            "--fresh" => call.fresh = true,
            "--no-start" => call.no_start = true,
            "--json" => {}
            flag if flag.starts_with('-') => {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("pij pod {}: unknown flag {flag}; pij pod --help", call.verb),
                ));
            }
            position => call.positions.push(position.to_string()),
        }
    }
    Ok(call)
}

/// `POST /v1/pod`.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let args = request.argv.get(1..).unwrap_or_default();
    if pij_core::directory::wants_help(args) {
        let verb = pij_core::directory::verb(args);
        return match pij_core::pod::help(verb) {
            Some(text) => envelope(
                StatusCode::OK,
                &Envelope::ok(COMMAND, json!({"help": text})),
            ),
            None => refused(&Refusal::new(
                "E-RS-UNPORTED",
                format!("pod {}: unknown command", verb.unwrap_or_default()),
            )),
        };
    }
    let call = match parse(&request.argv) {
        Ok(call) => call,
        Err(refusal) => return refused(&refusal),
    };
    let actor = match resolve_seat(
        &state,
        COMMAND,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat.id,
        Resolved::Refusal(response) => return response,
    };
    let answer = match call.verb.as_str() {
        "plan" => plan(&state, &call, &request.caller, &actor)
            .await
            .map(|planned| planned.answer()),
        "up" => up(&state, &call, &request.caller, &actor).await,
        "show" => show(&state, &call, &request.caller).await,
        "confirm" => confirm(&state, &call, &actor).await,
        "set" => set(&state, &call, &request.caller, &actor).await,
        "down" => down(&state, &call, &request.caller, &actor).await,
        "add" => add(&state, &call, &request.caller, &actor).await,
        "release" => release(&state, &call, &request.caller, &actor).await,
        "sessions" => sessions(&state, &call, &request.caller).await,
        verb => Err(Refusal::new(
            "E-RS-UNPORTED",
            format!("pod {verb}: unknown command; pij pod --help"),
        )),
    };
    match answer {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(COMMAND, data)),
        Err(refusal) => refused(&refusal),
    }
}

fn refused(refusal: &Refusal) -> Response {
    refused_as(COMMAND, refusal)
}

/// A pod read and checked before any write or launch.
struct Planned {
    template: Template,
    shown: String,
    filled: Vec<(String, String)>,
    project: String,
    repo: PathBuf,
    worktree: PathBuf,
    pod: String,
    paths: PodPaths,
    subject: Option<String>,
    work_folder: Option<String>,
    fresh: bool,
    plans: Vec<SlotPlan>,
    live: BTreeSet<String>,
    warnings: Vec<Warning>,
}

impl Planned {
    fn answer(&self) -> Value {
        json!({
            "pod": self.pod,
            "project": self.project,
            "template": {
                "source": self.shown,
                "name": self.template.name,
                "version": self.template.version,
                "sha256": self.template.hash,
            },
            "parameters": self.filled.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>(),
            "slots": self.plans.iter().map(slot_answer).collect::<Vec<_>>(),
            "consulted": self.template.consulted.iter().map(|consulted| json!({
                "role": consulted.role,
                "before_up": consulted.before_up,
                "live_holder": self.live.contains(&consulted.role),
            })).collect::<Vec<_>>(),
            "deliverables": self.template.deliverables.iter().map(|deliverable| json!({
                "id": deliverable.id,
                "owner": deliverable.owner,
                "stages": deliverable.stages,
                "write_back": deliverable.write_back,
            })).collect::<Vec<_>>(),
            "warnings": warnings(&self.warnings),
            "writes": "nothing: pij pod up <template> ... starts it",
        })
    }
}

fn slot_answer(plan: &SlotPlan) -> Value {
    let mut slot = json!({
        "slot": plan.slot.id,
        "role": plan.slot.role,
        "instance": (!plan.slot.instance.is_empty()).then_some(&plan.slot.instance),
        "harness": plan.harness.as_deref().unwrap_or("omp"),
        "model": plan.model,
        "tools": plan.slot.tools,
        "set": plan.sets,
        "keep_open": plan.slot.keep_open,
    });
    match &plan.first_turn {
        Ok(text) => slot["first_turn"] = json!(text),
        Err(why) => slot["first_turn_error"] = json!(why),
    }
    slot
}

fn warnings(found: &[Warning]) -> Value {
    json!(
        found
            .iter()
            .map(|warning| json!({
                "about": warning.about,
                "text": warning.text,
                "fix": warning.fix,
                "line": warning.line(),
            }))
            .collect::<Vec<_>>()
    )
}

/// The template's path: absolute, or relative to the caller's folder.
fn template_path(given: &str, caller: &CallerContext) -> Result<PathBuf, Refusal> {
    let path = Path::new(given);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    caller
        .cwd
        .as_deref()
        .filter(|cwd| Path::new(cwd).is_absolute())
        .map(|cwd| Path::new(cwd).join(path))
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                "a relative template path needs the caller's folder; give an absolute path",
            )
        })
}

/// Read, fill and check the pod a `plan` or `up` names, before any write.
async fn plan(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Planned, Refusal> {
    let given = call.positions.first().ok_or_else(|| {
        Refusal::new(
            "E-RS-ARG",
            format!("pij pod {} <template> [--set <name>=<value>]...", call.verb),
        )
    })?;
    let template = pod::read(&template_path(given, caller)?, given)?;
    let filled = pod::fill(&template, &call.sets)?;
    let (project, repo) = project_and_repo(state, caller, None).await?;
    let worktree = root::find(&repo)?.worktree;
    let pod = call.name.clone().unwrap_or_else(|| template.name.clone());
    let paths = PodPaths::new(&pod)?;
    let value = |name: &str| {
        filled
            .iter()
            .find(|(given, _)| given == name)
            .map(|(_, value)| value.clone())
    };
    let subject = value("subject");
    let plans = template
        .slots
        .iter()
        .map(|slot| {
            let sets = pod::slot_sets(slot, &filled);
            let pairs: Vec<(String, String)> = sets
                .iter()
                .filter_map(|set| set.split_once('='))
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            let started = first_turn::start(
                &worktree,
                &slot.role,
                &slot.instance,
                &pairs,
                &Creator {
                    seat: actor.to_string(),
                    focus: None,
                    packet: None,
                    subject: subject.clone(),
                    fresh: call.fresh,
                },
            );
            let (harness, model, first_turn) = match started {
                Ok(start) => (
                    start.default_seat["harness"].as_str().map(str::to_string),
                    start.default_seat["model"].as_str().map(str::to_string),
                    Ok(start.first_turn),
                ),
                Err(refusal) => (None, None, Err(refusal.to_string())),
            };
            SlotPlan {
                slot: slot.clone(),
                sets,
                harness,
                model,
                first_turn,
            }
        })
        .collect::<Vec<_>>();
    let live = live_roles(state, &project).await?;
    let exists =
        |id: &str| RolePaths::new(id).is_ok_and(|paths| worktree.join(paths.document).is_file());
    let warnings = pod::checks(&template, &plans, &|role| live.contains(role), &exists);
    Ok(Planned {
        shown: recorded_source(given, &repo, caller),
        work_folder: value("work-folder"),
        template,
        filled,
        project,
        repo,
        worktree,
        pod,
        paths,
        subject,
        fresh: call.fresh,
        plans,
        live,
        warnings,
    })
}

/// How the pod records its template: relative to the repo when it lies inside
/// it, so the record carries no machine's home folder; else as given.
fn recorded_source(given: &str, repo: &Path, caller: &CallerContext) -> String {
    let Ok(path) = template_path(given, caller) else {
        return given.to_string();
    };
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical(&path)
        .strip_prefix(canonical(repo))
        .map(|inside| inside.display().to_string())
        .unwrap_or_else(|_| given.to_string())
}

/// The roles in `project` with an open holding by a live seat: one indexed
/// read of the project's holdings, one registry read per open holder.
async fn live_roles(state: &AppState, project: &str) -> Result<BTreeSet<String>, Refusal> {
    let rows = state
        .services
        .directory
        .store()
        .assignments(project)
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    let mut live = BTreeSet::new();
    for row in rows.into_iter().filter(|row| row.to.is_none()) {
        if live.contains(&row.role) {
            continue;
        }
        if seat_live(state, &row.seat).await {
            live.insert(row.role);
        }
    }
    Ok(live)
}

async fn seat_live(state: &AppState, seat: &str) -> bool {
    matches!(
        state.services.registry.get(&SeatId::from(seat)).await,
        Ok(Some(row)) if row.tombstoned_at.is_none()
    )
}

/// The open holders of `role` in `project` that are live.
async fn holders_of(state: &AppState, project: &str, role: &str) -> Vec<String> {
    let rows = state
        .services
        .directory
        .store()
        .assignments(project)
        .await
        .unwrap_or_default();
    let mut found = Vec::new();
    for row in rows
        .into_iter()
        .filter(|row| row.to.is_none() && row.role == role)
    {
        if seat_live(state, &row.seat).await {
            found.push(row.seat);
        }
    }
    found
}

fn pod_file(document: &Value, paths: &PodPaths, existing: Existing) -> NewFile {
    NewFile {
        path: paths.document.clone(),
        bytes: role::to_bytes(document),
        document: true,
        existing,
    }
}

async fn commit(
    state: &AppState,
    planned_project: &str,
    repo: &Path,
    file: NewFile,
    message: String,
) -> Result<String, Refusal> {
    let (written, _) = state
        .services
        .directory
        .write(planned_project, repo.to_path_buf(), vec![file], message)
        .await?;
    Ok(written.commit)
}

/// `pij pod up`: the record, then each slot started as `spawn --as` starts
/// one, then its members; the canary runs on in one task.
async fn up(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    if let Some(name) = call.positions.first()
        && PodPaths::new(name).is_ok()
        && let Ok(opened) = open(state, caller, name).await
    {
        if pod::meta(&opened.document)["state"] == "up" {
            return Err(Refusal::new(
                E_POD_EXISTS,
                format!("pod {name} is up; see it with pij pod show {name}"),
            ));
        }
        return revive(state, call, caller, actor, opened).await;
    }
    let planned = plan(state, call, caller, actor).await?;
    if planned.worktree.join(&planned.paths.document).exists() {
        return Err(Refusal::new(
            E_POD_EXISTS,
            format!(
                "pod {} already has a record; see it with pij pod show {}, or name a new one with --name <pod>",
                planned.pod, planned.pod
            ),
        ));
    }
    let at = holders::now();
    let mut document = pod::document(
        &planned.template,
        &planned.filled,
        &pod::NewPod {
            pod: planned.pod.clone(),
            subject: planned.subject.clone(),
            work_folder: planned.work_folder.clone(),
            fresh: planned.fresh,
            by: actor.to_string(),
            at: at.clone(),
        },
    );
    for warning in &planned.warnings {
        document = pod::logged(
            document,
            &at,
            actor.as_str(),
            &format!("warning: {}", warning.line()),
        );
    }
    let created = commit(
        state,
        &planned.project,
        &planned.repo,
        pod_file(
            &document,
            &planned.paths,
            Existing::Refuse(Refusal::new(
                E_POD_EXISTS,
                format!("pod {} already has a record", planned.pod),
            )),
        ),
        format!("pij pod up {}: the record (by {actor})", planned.pod),
    )
    .await?;
    let mut answer = planned.answer();
    answer.as_object_mut().map(|answer| answer.remove("writes"));
    answer["commit"] = json!(created);
    if call.no_start {
        answer["members"] = json!([]);
        answer["started"] = json!("nothing (--no-start)");
        return Ok(answer);
    }
    let folder = planned
        .work_folder
        .clone()
        .filter(|folder| Path::new(folder).is_absolute() && Path::new(folder).is_dir())
        .or_else(|| caller.cwd.clone())
        .unwrap_or_default();
    let mut members = Vec::new();
    let mut late: Vec<Warning> = Vec::new();
    let mut started = Vec::new();
    for plan in planned.plans.iter().filter(|plan| plan.first_turn.is_ok()) {
        match start_slot(
            state,
            plan,
            &planned,
            caller,
            actor,
            &folder,
            call.session.clone(),
        )
        .await
        {
            Ok((seat, spawn_warnings)) => {
                late.extend(spawn_warnings.into_iter().map(|text| Warning {
                    about: format!("slot {}", plan.slot.id),
                    text,
                    fix: "as the warning says".to_string(),
                }));
                let canary = if plan.slot.tools.is_empty() {
                    "none asked"
                } else {
                    "pending"
                };
                members.push(pod::member(&plan.slot, &seat, canary));
                started.push((plan.slot.clone(), seat));
            }
            Err(why) => late.push(Warning {
                about: format!("slot {}", plan.slot.id),
                text: format!("it did not start: {why}"),
                fix: format!(
                    "pij spawn --as role:{} {} --parent {actor}",
                    address(&plan.slot),
                    plan.harness.as_deref().unwrap_or("omp")
                ),
            }),
        }
    }
    let at = holders::now();
    document = pod::with_members(document, members.clone());
    for warning in &late {
        document = pod::logged(
            document,
            &at,
            actor.as_str(),
            &format!("warning: {}", warning.line()),
        );
    }
    let seats: Vec<&str> = started.iter().map(|(_, seat)| seat.as_str()).collect();
    document = pod::logged(
        document,
        &at,
        actor.as_str(),
        &format!(
            "started {}",
            if seats.is_empty() {
                "no seat".to_string()
            } else {
                seats.join(", ")
            }
        ),
    );
    match commit(
        state,
        &planned.project,
        &planned.repo,
        pod_file(&document, &planned.paths, Existing::Replace),
        format!("pij pod up {}: its members (by {actor})", planned.pod),
    )
    .await
    {
        Ok(commit) => answer["members_commit"] = json!(commit),
        Err(refusal) => late.push(Warning {
            about: format!("pod {}", planned.pod),
            text: format!("its seats started but pij could not record its members ({refusal})"),
            fix: format!(
                "pij pod show {} lists what was recorded; start no slot twice",
                planned.pod
            ),
        }),
    }
    let mut all = planned.warnings.clone();
    all.extend(late);
    answer["warnings"] = warnings(&all);
    answer["members"] = json!(members);
    let asked: Vec<(pod::Slot, String)> = started
        .into_iter()
        .filter(|(slot, _)| !slot.tools.is_empty())
        .collect();
    answer["canary"] = if asked.is_empty() {
        json!("no slot names a tool")
    } else {
        json!(format!(
            "asked {}; answers are read for {} minutes, then the outcome is in pij pod show {}",
            asked
                .iter()
                .map(|(_, seat)| seat.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            CANARY_BOUND.as_secs() / 60,
            planned.pod
        ))
    };
    if !asked.is_empty() {
        canary(
            state,
            Record {
                pod: planned.pod.clone(),
                project: planned.project.clone(),
                repo: planned.repo.clone(),
                worktree: planned.worktree.clone(),
                paths: planned.paths.clone(),
            },
            actor,
            asked,
        )
        .await;
    }
    Ok(answer)
}

fn address(slot: &pod::Slot) -> String {
    if slot.instance.is_empty() {
        slot.role.clone()
    } else {
        format!("{}/{}", slot.role, slot.instance)
    }
}

/// Start one slot through the spawn path `pij spawn --as` takes: its seat id
/// and the spawn's warnings, or why it did not start.
async fn start_slot(
    state: &AppState,
    plan: &SlotPlan,
    planned: &Planned,
    caller: &CallerContext,
    actor: &SeatId,
    folder: &str,
    session: Option<String>,
) -> Result<(String, Vec<String>), String> {
    let harness = plan
        .harness
        .as_deref()
        .and_then(Harness::parse)
        .unwrap_or(Harness::Omp);
    let request = SpawnRequest {
        id: None,
        harness,
        allow_retired: false,
        executable: None,
        model: None,
        effort: None,
        cwd: folder.to_string(),
        session,
        caller_pane: caller.pane.clone(),
        name: None,
        parent: Some(actor.clone()),
        accept_inbound: false,
        wait_seconds: None,
        no_wait: true,
        resume: None,
        role: None,
        caller: Some(caller.clone()),
        no_cache_proxy: false,
        trust_folder: false,
        spawn_id: None,
        alt: None,
        as_role: Some(AsRole {
            role: plan.slot.role.clone(),
            instance: plan.slot.instance.clone(),
            set: plan.sets.clone(),
            focus: plan.slot.seed.clone(),
            packet: None,
            subject: planned.subject.clone(),
            fresh: planned.fresh,
            pod: None,
        }),
    };
    let response = super::launch_seat(
        state,
        request,
        ExistingSeatPolicy { allow_absent: true },
        "pij pod up",
    )
    .await;
    let answer = answer_of(response).await?;
    if answer["ok"] != true {
        return Err(answer["meta"]
            .as_str()
            .unwrap_or("the spawn refused")
            .to_string());
    }
    let seat = answer["data"]["id"]
        .as_str()
        .ok_or("the spawn answered no seat id")?
        .to_string();
    let spawn_warnings = answer["details"]["warnings"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok((seat, spawn_warnings))
}

/// Send each asked seat its canary, then, in one task, wait for every answer
/// until [`CANARY_BOUND`] and commit the outcome once. The subscription is
/// taken before the first send, so no answer can slip past.
/// Where a pod's record lives, for the work that outlives a request.
#[derive(Clone)]
struct Record {
    pod: String,
    project: String,
    repo: PathBuf,
    worktree: PathBuf,
    paths: PodPaths,
}

async fn canary(state: &AppState, record: Record, actor: &SeatId, asked: Vec<(pod::Slot, String)>) {
    let mut events = state
        .services
        .event_bus
        .subscribe_live(EventFilter::kinds([CANARY_EVENT_KIND]));
    for (slot, seat) in &asked {
        if let Err(error) = state
            .services
            .delivery
            .send(
                SeatId::from(pij_core::BG_ACTOR),
                SeatId::from(seat.as_str()),
                pod::canary_prompt(&record.pod, slot),
            )
            .await
        {
            eprintln!("pij-rs could not send {seat} its pod canary: {error}");
        }
    }
    let state = state.clone();
    let actor = actor.clone();
    tokio::spawn(async move {
        let pod = record.pod.clone();
        let deadline = tokio::time::Instant::now() + CANARY_BOUND;
        let mut confirmed: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        while confirmed.len() < asked.len() {
            match tokio::time::timeout_at(deadline, events.next()).await {
                Ok(Some(event)) => {
                    let payload: Value = serde_json::from_str(&event.payload).unwrap_or_default();
                    let Some(seat) = event.seat.as_ref().map(SeatId::to_string) else {
                        continue;
                    };
                    if payload["pod"] == pod.as_str()
                        && asked.iter().any(|(_, asked)| asked == &seat)
                    {
                        let tools = payload["tools"]
                            .as_array()
                            .map(|tools| {
                                tools
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();
                        confirmed.insert(seat, tools);
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        settle(&state, &record, &actor, &asked, &confirmed).await;
    });
}

/// Record each asked seat's canary outcome in one commit, and tell the seat
/// that ran `up` which slots are degraded and how to fix them.
async fn settle(
    state: &AppState,
    record: &Record,
    actor: &SeatId,
    asked: &[(pod::Slot, String)],
    confirmed: &std::collections::BTreeMap<String, Vec<String>>,
) {
    let (project, repo, paths, pod_name) = (
        record.project.as_str(),
        record.repo.as_path(),
        &record.paths,
        record.pod.as_str(),
    );
    let Ok(bytes) = std::fs::read(record.worktree.join(&paths.document)) else {
        eprintln!("pij-rs could not read pod {pod_name}'s record to settle its canary");
        return;
    };
    let mut document: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let at = holders::now();
    let mut degraded = Vec::new();
    for (slot, seat) in asked {
        let missing = pod::missing(slot, confirmed.get(seat).map_or(&[][..], Vec::as_slice));
        let outcome = if missing.is_empty() {
            "confirmed".to_string()
        } else {
            let line = pod::degraded(pod_name, &missing, seat);
            degraded.push(format!("slot {} ({seat}): {line}", slot.id));
            line
        };
        document = pod::with_canary(document, seat, &outcome);
        document = pod::logged(
            document,
            &at,
            pij_core::BG_ACTOR,
            &format!("canary {seat}: {outcome}"),
        );
    }
    if let Err(refusal) = commit(
        state,
        project,
        repo,
        pod_file(&document, paths, Existing::Replace),
        format!("pij pod up {pod_name}: its tool canary"),
    )
    .await
    {
        eprintln!("pij-rs could not record pod {pod_name}'s canary: {refusal}");
    }
    if !degraded.is_empty() {
        let body = format!(
            "pij: pod {pod_name}'s tool canary left {} degraded (pij hands them no pod work; your messages still reach them):\n{}",
            degraded.len(),
            degraded.join("\n")
        );
        if let Err(error) = state
            .services
            .delivery
            .hold_fyi(Msg {
                from: SeatId::from(pij_core::BG_ACTOR),
                to: actor.clone(),
                body,
                msg_id: state.services.delivery.next_message_id(),
                from_machine: None,
                in_reply_to: None,
                command: None,
            })
            .await
        {
            eprintln!("pij-rs could not tell {actor} about pod {pod_name}'s canary: {error}");
        }
    }
}

/// A pod's record, read for a verb that changes it.
struct Opened {
    record: Record,
    document: Value,
}

async fn open(state: &AppState, caller: &CallerContext, pod_name: &str) -> Result<Opened, Refusal> {
    let (project, repo) = project_and_repo(state, caller, None).await?;
    let worktree = root::find(&repo)?.worktree;
    let document = recorded(&worktree, &project, pod_name)?;
    Ok(Opened {
        record: Record {
            pod: pod_name.to_string(),
            paths: PodPaths::new(pod_name)?,
            project,
            repo,
            worktree,
        },
        document,
    })
}

async fn save(
    state: &AppState,
    record: &Record,
    document: &Value,
    what: &str,
) -> Result<String, Refusal> {
    commit(
        state,
        &record.project,
        &record.repo,
        pod_file(document, &record.paths, Existing::Replace),
        format!("pij pod {} {what}", record.pod),
    )
    .await
}

fn position<'a>(call: &'a Call, index: usize, usage: &str) -> Result<&'a str, Refusal> {
    call.positions
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| Refusal::new("E-RS-ARG", usage.to_string()))
}

/// `pij pod set <pod> <name>=<value>` (req-05): record the value, then send
/// it to each member whose slot uses it and only them. A degraded member is
/// not sent it (the dispatch brake) and is named.
async fn set(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    const USAGE: &str = "pij pod set <pod> <name>=<value>";
    let pod_name = position(call, 0, USAGE)?;
    let (name, value) = position(call, 1, USAGE)?
        .split_once('=')
        .ok_or_else(|| Refusal::new("E-RS-ARG", USAGE))?;
    let opened = open(state, caller, pod_name).await?;
    let template = template_of(&opened.record.repo, &opened.document)?;
    pod::fill(
        &Template {
            parameters: template
                .parameters
                .iter()
                .map(|(known, _)| (known.clone(), false))
                .collect(),
            ..template.clone()
        },
        &[(name.to_string(), value.to_string())],
    )?;
    let at = holders::now();
    let document = pod::logged(
        pod::with_parameter(opened.document, name, value),
        &at,
        actor.as_str(),
        &format!("set {name}={value}"),
    );
    let committed = save(
        state,
        &opened.record,
        &document,
        &format!("set {name} (by {actor})"),
    )
    .await?;
    let mut sent = Vec::new();
    let mut warnings = Vec::new();
    for member in pod::records(&document, "members") {
        let seat = member["seat"].as_str().unwrap_or_default();
        let uses = template.slots.iter().any(|slot| {
            member["slot"] == slot.id.as_str() && slot.parameters.iter().any(|used| used == name)
        });
        if !uses {
            continue;
        }
        if !pod::may_dispatch(member) {
            warnings.push(format!(
                "{seat} is degraded, so pij did not send it {name}; send it yourself once it has its tools: pij send {seat} ..."
            ));
            continue;
        }
        match state
            .services
            .delivery
            .send(
                actor.clone(),
                SeatId::from(seat),
                format!("pij pod {pod_name}: the parameter {name} is now {value}"),
            )
            .await
        {
            Ok(_) => sent.push(seat.to_string()),
            Err(error) => warnings.push(format!("{seat} could not be sent {name}: {error}")),
        }
    }
    Ok(
        json!({"pod": pod_name, "set": {name: value}, "sent": sent, "warnings": warnings, "commit": committed}),
    )
}

/// `pij pod down <pod>` (req-06, req-10, req-14): the freeze record, then
/// each member that is not keep-open through the same handler `pij close
/// <seat> --stop` reaches, once per member. A consulted role is never
/// touched. The pod reads as closing until every such member is down.
async fn down(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    let pod_name = position(call, 0, "pij pod down <pod>")?;
    let opened = open(state, caller, pod_name).await?;
    let at = holders::now();
    let mut document = pod::with_state(opened.document, "closing");
    let mut warnings = Vec::new();
    let freeze = pod::meta(&document)["work_folder"]
        .as_str()
        .filter(|folder| Path::new(folder).is_absolute())
        .map(|folder| match pod::freeze(Path::new(folder)) {
            Ok(freeze) => {
                if freeze.uncommitted > 0 {
                    warnings.push(format!(
                        "the run is unfrozen: {folder} has {} uncommitted path(s); commit them to freeze it",
                        freeze.uncommitted
                    ));
                }
                freeze.line(folder)
            }
            Err(why) => {
                warnings.push(format!("pij could not read {folder} with git, so no freeze is recorded: {why}"));
                format!("freeze: not recorded ({why})")
            }
        });
    if let Some(line) = &freeze {
        document = pod::logged(document, &at, actor.as_str(), line);
    }
    let mut members = Vec::new();
    for member in pod::records(&document, "members").to_vec() {
        let seat = member["seat"].as_str().unwrap_or_default().to_string();
        let role_id = member["role"].as_str().unwrap_or_default();
        let outcome = if pod::keeps_open(
            &member,
            role_type(&opened.record.worktree, role_id).as_deref(),
        ) {
            warnings.push(format!(
                "{seat} is keep-open, so it was left running and out of the closed pod; take it down with pij close {seat} --stop"
            ));
            "left running (keep-open)".to_string()
        } else if !seat_live(state, &seat).await {
            "already down".to_string()
        } else {
            close_stop(state, caller, &seat, pod_name).await
        };
        members.push(json!({"slot": member["slot"], "seat": seat, "outcome": outcome}));
    }
    for warning in &warnings {
        document = pod::logged(
            document,
            &at,
            actor.as_str(),
            &format!("warning: {warning}"),
        );
    }
    document = pod::logged(
        document,
        &at,
        actor.as_str(),
        &format!(
            "down: {}",
            members
                .iter()
                .map(|member| format!(
                    "{} {}",
                    text_of(&member["seat"]),
                    text_of(&member["outcome"])
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    );
    let committed = save(
        state,
        &opened.record,
        &document,
        &format!("down (by {actor})"),
    )
    .await?;
    Ok(json!({
        "pod": pod_name,
        "state": "closing",
        "members": members,
        "freeze": freeze,
        "warnings": warnings,
        "commit": committed,
        "next": format!("pij pod show {pod_name} reads closed once every member is down"),
    }))
}

fn text_of(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// One member through the close route's own handler with `--stop`, as the
/// caller: what it answered.
async fn close_stop(
    state: &AppState,
    caller: &CallerContext,
    seat: &str,
    pod_name: &str,
) -> String {
    let body: std::result::Result<Json<Value>, axum::extract::rejection::JsonRejection> =
        Ok(Json(json!({
            "seat": seat,
            "reason": format!("pij pod down {pod_name}"),
            "stop": true,
            "caller": caller,
        })));
    let response = super::lifecycle::close(State(state.clone()), body).await;
    match answer_of(response).await {
        Ok(answer) if answer["ok"] == true => {
            if answer["data"]["held"].is_string() {
                "held for its memory turn; closed and stopped once its note is written, else left running and the prime told".to_string()
            } else if answer["data"]["stopped"]["stopped"] == true {
                "closed and stopped".to_string()
            } else {
                "closed".to_string()
            }
        }
        Ok(answer) => format!("not closed: {}", text_of(&answer["meta"])),
        Err(why) => format!("not closed: {why}"),
    }
}

/// `pij pod up <pod>` on a closing or closed pod (req-11): each member that
/// is down is revived through revive's own handler, one at a time, under its
/// rules (resume, cheap start, the cold refusal with its price).
async fn revive(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
    opened: Opened,
) -> Result<Value, Refusal> {
    let pod_name = opened.record.pod.clone();
    let mut members = Vec::new();
    let mut warnings = Vec::new();
    for member in pod::records(&opened.document, "members") {
        let seat = member["seat"].as_str().unwrap_or_default().to_string();
        let outcome = if seat_live(state, &seat).await {
            "running".to_string()
        } else {
            let request: super::types::ReviveRequest = serde_json::from_value(json!({
                "id": seat,
                "caller": caller,
                "caller_pane": caller.pane,
                "session": call.session,
            }))
            .map_err(|error| Refusal::new("E-RS-ARG", error.to_string()))?;
            let response =
                super::revive::revive_seat(State(state.clone()), Ok(Json(request))).await;
            match answer_of(response).await {
                Ok(answer) if answer["ok"] == true => "revived".to_string(),
                Ok(answer) => {
                    let why = text_of(&answer["meta"]).to_string();
                    warnings.push(format!("{seat} was not revived: {why}"));
                    format!("not revived: {why}")
                }
                Err(why) => {
                    warnings.push(format!("{seat} was not revived: {why}"));
                    format!("not revived: {why}")
                }
            }
        };
        members.push(json!({"slot": member["slot"], "seat": seat, "outcome": outcome}));
    }
    let at = holders::now();
    let mut document = pod::with_state(opened.document, "up");
    for warning in &warnings {
        document = pod::logged(
            document,
            &at,
            actor.as_str(),
            &format!("warning: {warning}"),
        );
    }
    document = pod::logged(document, &at, actor.as_str(), "up again by name");
    let committed = save(
        state,
        &opened.record,
        &document,
        &format!("up again (by {actor})"),
    )
    .await?;
    Ok(
        json!({"pod": pod_name, "state": "up", "members": members, "warnings": warnings, "commit": committed}),
    )
}

/// `pij pod add <pod> role:<id>[/<instance>] [--set ...] [--session <tmux>]`
/// starts a member as `spawn --as --pod` does; `pij pod add <pod> <seat>`
/// adds a seat already running (req-08). Logged; the template is unchanged.
async fn add(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    const USAGE: &str = "pij pod add <pod> role:<id>[/<instance>] | <seat>";
    let pod_name = position(call, 0, USAGE)?;
    let target = position(call, 1, USAGE)?;
    let opened = open(state, caller, pod_name).await?;
    if let Some(address) = target.strip_prefix("role:") {
        let (role_id, instance) = address.split_once('/').unwrap_or((address, ""));
        let harness = RolePaths::new(role_id)
            .ok()
            .and_then(|paths| std::fs::read(opened.record.worktree.join(paths.document)).ok())
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|document| {
                document["sections"]
                    .as_array()?
                    .iter()
                    .find(|section| section["name"] == "default_seat")?["value"]["harness"]
                    .as_str()
                    .and_then(Harness::parse)
            })
            .unwrap_or(Harness::Omp);
        let mut request: SpawnRequest = serde_json::from_value(json!({
            "harness": harness,
            "cwd": pod::meta(&opened.document)["work_folder"]
                .as_str()
                .filter(|folder| Path::new(folder).is_dir())
                .or(caller.cwd.as_deref())
                .unwrap_or_default(),
            "session": call.session,
            "caller_pane": caller.pane,
            "parent": actor,
            "no_wait": true,
            "caller": caller,
        }))
        .map_err(|error| Refusal::new("E-RS-ARG", error.to_string()))?;
        request.as_role = Some(AsRole {
            role: role_id.to_string(),
            instance: instance.to_string(),
            set: call
                .sets
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect(),
            focus: None,
            packet: None,
            subject: pod::meta(&opened.document)["subject"]
                .as_str()
                .map(str::to_string),
            fresh: pod::meta(&opened.document)["fresh"] == "yes",
            pod: Some(pod_name.to_string()),
        });
        let response = super::launch_seat(
            state,
            request,
            ExistingSeatPolicy { allow_absent: true },
            "pij pod add",
        )
        .await;
        let answer = answer_of(response)
            .await
            .map_err(|why| Refusal::new("E-RS-ADAPTER", why))?;
        if answer["ok"] != true {
            return Err(Refusal::new(
                "E-RS-POD-ADD",
                format!("{target} did not start: {}", text_of(&answer["meta"])),
            ));
        }
        return Ok(
            json!({"pod": pod_name, "added": answer["data"]["id"], "joined": answer["details"]["pod"], "warnings": answer["details"]["warnings"]}),
        );
    }
    let seat = state
        .services
        .registry
        .get(&SeatId::from(target))
        .await
        .ok()
        .flatten()
        .filter(|row| row.tombstoned_at.is_none())
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-NOT-FOUND",
                format!("no live seat {target}; start one into the pod with pij pod add {pod_name} role:<id>"),
            )
        })?;
    if pod::records(&opened.document, "members")
        .iter()
        .any(|member| member["seat"] == target)
    {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("{target} is already a member of pod {pod_name}"),
        ));
    }
    let holding = state
        .services
        .directory
        .store()
        .open_assignments(target)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|row| row.project == opened.record.project);
    let slot = pod::Slot {
        id: target.to_string(),
        role: holding
            .as_ref()
            .map_or_else(|| "none".to_string(), |row| row.role.clone()),
        instance: holding.map(|row| row.instance).unwrap_or_default(),
        parameters: Vec::new(),
        tools: Vec::new(),
        not_model_of: None,
        keep_open: None,
        seed: None,
    };
    let at = holders::now();
    let document = pod::logged(
        pod::with_members(
            opened.document,
            vec![pod::member(&slot, seat.id.as_str(), "none asked")],
        ),
        &at,
        actor.as_str(),
        &format!("added {target}, already running, in role {}", slot.role),
    );
    let committed = save(
        state,
        &opened.record,
        &document,
        &format!("add {target} (by {actor})"),
    )
    .await?;
    Ok(json!({"pod": pod_name, "added": target, "role": slot.role, "commit": committed}))
}

/// `pij pod release <pod> <seat>` (req-08): the seat leaves the pod and keeps
/// running. Logged.
async fn release(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    const USAGE: &str = "pij pod release <pod> <seat>";
    let pod_name = position(call, 0, USAGE)?;
    let target = position(call, 1, USAGE)?;
    let opened = open(state, caller, pod_name).await?;
    let (document, removed) = pod::without_member(opened.document, target);
    if removed.is_none() {
        return Err(Refusal::new(
            "E-RS-NOT-FOUND",
            format!("{target} is not a member of pod {pod_name}"),
        ));
    }
    let at = holders::now();
    let document = pod::logged(
        document,
        &at,
        actor.as_str(),
        &format!("released {target}; it keeps running"),
    );
    let committed = save(
        state,
        &opened.record,
        &document,
        &format!("release {target} (by {actor})"),
    )
    .await?;
    Ok(json!({
        "pod": pod_name,
        "released": target,
        "commit": committed,
        "next": format!("{target} keeps running; close it with pij close {target} --stop"),
    }))
}

/// `pij pod sessions <pod> [--limit <n>]` (req-13): each member's sessions,
/// as `pij sessions --history --seat <seat>` lists them. Reads only.
async fn sessions(state: &AppState, call: &Call, caller: &CallerContext) -> Result<Value, Refusal> {
    let pod_name = position(call, 0, "pij pod sessions <pod> [--limit <n>]")?;
    let opened = open(state, caller, pod_name).await?;
    let limit = call.limit.unwrap_or(crate::session_history::DEFAULT_LIMIT);
    let mut members = Vec::new();
    for member in pod::records(&opened.document, "members") {
        let seat = SeatId::from(member["seat"].as_str().unwrap_or_default());
        let mut history = super::shim::seat_history(state, &seat, limit).await;
        history["slot"] = member["slot"].clone();
        members.push(history);
    }
    Ok(json!({"pod": pod_name, "sessions": members}))
}

/// The template a pod's record names: relative to the repo, or absolute.
///
/// # Errors
/// Whatever reading it refuses with.
fn template_of(repo: &Path, document: &Value) -> Result<Template, Refusal> {
    let source = pod::meta(document)["template"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let file = if Path::new(&source).is_absolute() {
        PathBuf::from(&source)
    } else {
        repo.join(&source)
    };
    pod::read(&file, &source)
}

/// An in-process handler's answer as JSON.
async fn answer_of(response: Response) -> Result<Value, String> {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .map_err(|error| format!("its answer could not be read: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("its answer was not JSON: {error}"))
}

/// The type a role in `worktree` names, if its document says.
fn role_type(worktree: &Path, role_id: &str) -> Option<String> {
    let paths = RolePaths::new(role_id).ok()?;
    let document: Value =
        serde_json::from_slice(&std::fs::read(worktree.join(paths.document)).ok()?).ok()?;
    pod::meta(&document)["type"].as_str().map(str::to_string)
}

/// The pod's record in `worktree`, or why `spawn --as --pod` names no pod.
///
/// # Errors
/// `E-RS-NOT-FOUND` naming how to create it.
pub(super) fn recorded(worktree: &Path, project: &str, pod_name: &str) -> Result<Value, Refusal> {
    let paths = PodPaths::new(pod_name)?;
    let bytes = std::fs::read(worktree.join(&paths.document)).map_err(|_| {
        Refusal::new(
            "E-RS-NOT-FOUND",
            format!(
                "project {project} has no pod {pod_name}; create it with pij pod up <template> --no-start"
            ),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        Refusal::new(
            "E-RS-NOT-FOUND",
            format!("pod {pod_name}'s record is not JSON: {error}"),
        )
    })
}

/// `pij spawn --as role:<id> --pod <pod>` (req-04): record the started seat
/// as the member for the template slot its role and instance fill, in one
/// commit, and send its canary when the slot names tools. A role that fills
/// no slot is recorded as a member of its own, with a warning.
///
/// # Errors
/// A refusal reading or writing the record; the seat stays as it started.
#[allow(clippy::too_many_arguments)]
pub(super) async fn join(
    state: &AppState,
    project: &str,
    repo: &Path,
    pod_name: &str,
    role_id: &str,
    instance: &str,
    seat: &SeatId,
    creator: &SeatId,
) -> Result<Value, Refusal> {
    let worktree = root::find(repo)?.worktree;
    let paths = PodPaths::new(pod_name)?;
    let document = recorded(&worktree, project, pod_name)?;
    let mut warning = None;
    let found = match template_of(repo, &document) {
        Ok(template) => template
            .slots
            .into_iter()
            .find(|slot| slot.role == role_id && slot.instance == instance),
        Err(refusal) => {
            warning = Some(format!(
                "its template could not be read ({refusal}), so no tools were checked"
            ));
            None
        }
    };
    let slot = match found {
        Some(slot) => slot,
        None => {
            warning.get_or_insert_with(|| {
                format!(
                    "role {role_id} fills no slot of the pod's template; it is recorded as a member of its own"
                )
            });
            pod::Slot {
                id: if instance.is_empty() {
                    role_id.to_string()
                } else {
                    format!("{role_id}-{instance}")
                },
                role: role_id.to_string(),
                instance: instance.to_string(),
                parameters: Vec::new(),
                tools: Vec::new(),
                not_model_of: None,
                keep_open: None,
                seed: None,
            }
        }
    };
    let asked = !slot.tools.is_empty();
    let at = holders::now();
    let mut document = pod::with_members(
        document,
        vec![pod::member(
            &slot,
            seat.as_str(),
            if asked { "pending" } else { "none asked" },
        )],
    );
    document = pod::logged(
        document,
        &at,
        creator.as_str(),
        &format!("started {seat} as slot {} (pij spawn --as --pod)", slot.id),
    );
    if let Some(warning) = &warning {
        document = pod::logged(
            document,
            &at,
            creator.as_str(),
            &format!("warning: {warning}"),
        );
    }
    let committed = commit(
        state,
        project,
        repo,
        pod_file(&document, &paths, Existing::Replace),
        format!(
            "pij pod {pod_name}: {seat} joins as slot {} (by {creator})",
            slot.id
        ),
    )
    .await?;
    if asked {
        canary(
            state,
            Record {
                pod: pod_name.to_string(),
                project: project.to_string(),
                repo: repo.to_path_buf(),
                worktree,
                paths,
            },
            creator,
            vec![(slot.clone(), seat.to_string())],
        )
        .await;
    }
    Ok(json!({
        "pod": pod_name,
        "slot": slot.id,
        "commit": committed,
        "canary": if asked { "asked" } else { "none asked" },
        "warning": warning,
    }))
}

/// `pij pod confirm <pod> --tool <tool>...`: one spine event the canary's
/// wait reads. Open to any seat: an answer from a seat not asked is ignored.
async fn confirm(state: &AppState, call: &Call, actor: &SeatId) -> Result<Value, Refusal> {
    let pod = call
        .positions
        .first()
        .ok_or_else(|| Refusal::new("E-RS-ARG", "pij pod confirm <pod> --tool <tool>..."))?;
    PodPaths::new(pod)?;
    let event = Event {
        seq: None,
        v: 1,
        at: super::system_time_ms().unwrap_or_default(),
        kind: CANARY_EVENT_KIND.to_string(),
        seat: Some(actor.clone()),
        payload: json!({"pod": pod, "tools": call.tools}).to_string(),
    };
    state
        .services
        .event_bus
        .publish(event)
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    Ok(json!({"pod": pod, "seat": actor, "confirmed": call.tools}))
}

/// `pij pod show <pod>`: the record, with each member's state from the
/// database and each consulted and owner role's live holders.
async fn show(state: &AppState, call: &Call, caller: &CallerContext) -> Result<Value, Refusal> {
    let pod_name = call
        .positions
        .first()
        .ok_or_else(|| Refusal::new("E-RS-ARG", "pij pod show <pod>"))?;
    let paths = PodPaths::new(pod_name)?;
    let (project, repo) = project_and_repo(state, caller, None).await?;
    let worktree = root::find(&repo)?.worktree;
    let bytes = std::fs::read(worktree.join(&paths.document)).map_err(|_| {
        Refusal::new(
            "E-RS-NOT-FOUND",
            format!(
                "project {project} has no pod {pod_name}; start one with pij pod up <template>"
            ),
        )
    })?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        Refusal::new(
            "E-RS-NOT-FOUND",
            format!("pod {pod_name}'s record is not JSON: {error}"),
        )
    })?;
    let mut members = Vec::new();
    let mut degraded = Vec::new();
    let mut running = 0;
    for member in pod::records(&document, "members") {
        let seat = member["seat"].as_str().unwrap_or_default();
        let row = state
            .services
            .registry
            .get(&SeatId::from(seat))
            .await
            .ok()
            .flatten();
        let keeps = pod::keeps_open(
            member,
            role_type(&worktree, member["role"].as_str().unwrap_or_default()).as_deref(),
        );
        if !keeps && row.as_ref().is_some_and(|row| row.tombstoned_at.is_none()) {
            running += 1;
        }
        let mut shown = member.clone();
        shown["state"] = match &row {
            Some(row) if row.tombstoned_at.is_some() => json!("closed"),
            Some(row) => json!(row.state),
            None => json!("unknown"),
        };
        if let Some(row) = &row {
            shown["harness"] = json!(row.harness);
            shown["model"] = json!(row.model);
        }
        if member["canary"]
            .as_str()
            .is_some_and(|canary| canary.starts_with("degraded"))
        {
            degraded
                .push(json!({"slot": member["slot"], "seat": seat, "canary": member["canary"]}));
        }
        members.push(shown);
    }
    let mut consulted = Vec::new();
    for row in pod::records(&document, "consulted") {
        let role = row["role"].as_str().unwrap_or_default();
        consulted.push(json!({
            "role": role,
            "before_up": row["before_up"] == "yes",
            "live_holders": holders_of(state, &project, role).await,
        }));
    }
    let mut deliverables = Vec::new();
    for row in pod::records(&document, "deliverables") {
        let mut shown = row.clone();
        shown["owner_holders"] =
            json!(holders_of(state, &project, row["owner"].as_str().unwrap_or_default()).await);
        deliverables.push(shown);
    }
    // req-06: a pod taken down reads closing until every member that is not
    // keep-open is down.
    let recorded_state = pod::meta(&document)["state"]
        .as_str()
        .unwrap_or("up")
        .to_string();
    let pod_state = if recorded_state == "closing" && running == 0 {
        "closed".to_string()
    } else {
        recorded_state
    };
    Ok(json!({
        "pod": pod_name,
        "project": project,
        "state": pod_state,
        "meta": pod::meta(&document),
        "parameters": pod::records(&document, "parameters"),
        "members": members,
        "consulted": consulted,
        "deliverables": deliverables,
        "degraded": degraded,
        "log": pod::records(&document, "log"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn a_call_reads_its_verb_sets_and_switches_and_refuses_an_unknown_flag() {
        let call = parse(&argv(&[
            "pod",
            "up",
            "pods/build",
            "--set",
            "step=s01",
            "--set",
            "subject=a=b",
            "--fresh",
            "--name",
            "build-1",
        ]))
        .unwrap();
        assert_eq!(call.verb, "up");
        assert_eq!(call.positions, ["pods/build"]);
        assert_eq!(call.sets[1], ("subject".to_string(), "a=b".to_string()));
        assert!(call.fresh && !call.no_start);
        assert_eq!(call.name.as_deref(), Some("build-1"));
        let refused = parse(&argv(&["pod", "up", "x", "--force"])).unwrap_err();
        assert!(
            refused.message.contains("unknown flag --force"),
            "{refused}"
        );
    }
}
