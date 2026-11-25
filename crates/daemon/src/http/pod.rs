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
use crate::community::pod_templates::{self, Shipped};
use crate::community::writer::{Existing, NewFile};
use crate::community::{Refusal, holders, role, root};

const COMMAND: &str = "pij pod";

/// A seat's answer to its tool canary (`pij pod confirm`).
pub const CANARY_EVENT_KIND: &str = "pod.canary";

/// A canary settlement for a pod no longer up: nothing is written or sent.
const E_POD_NOT_UP: &str = "E-RS-POD-NOT-UP";
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
    since: Option<String>,
    output: Option<String>,
    from: Vec<String>,
    fresh: bool,
    no_start: bool,
    tools: Vec<String>,
    skip_close_procedure: bool,
    close_procedure_ran: bool,
    into: Option<String>,
    why: Option<String>,
    seeds: Option<String>,
    templates: Vec<String>,
    as_template: Option<String>,
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
            "--since" => call.since = Some(value("--since")?),
            "--output" => call.output = Some(value("--output")?),
            "--from" => call.from.push(value("--from")?),
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
            "--into" => call.into = Some(value("--into")?),
            "--as-template" => call.as_template = Some(value("--as-template")?),
            "--why" => call.why = Some(value("--why")?),
            "--seeds" => call.seeds = Some(value("--seeds")?),
            "--templates" => call.templates.extend(
                value("--templates")?
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            ),
            "--fresh" => call.fresh = true,
            "--no-start" => call.no_start = true,
            "--skip-close-procedure" => call.skip_close_procedure = true,
            // Set by the pij CLI once it has run the template's close
            // procedure to its end (PRD 010 req-01); recorded, never checked.
            "--close-procedure-ran" => call.close_procedure_ran = true,
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
    let community = request.argv.first().map(String::as_str) == Some("community");
    if community && (args.is_empty() || pij_core::directory::wants_help(args)) {
        return envelope(
            StatusCode::OK,
            &Envelope::ok(
                "pij community",
                json!({"help": pij_core::pod::COMMUNITY_HELP}),
            ),
        );
    }
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
        "init" if community => community_init(&state, &call, &request.caller, &actor).await,
        _ if community => Err(Refusal::new(
            "E-RS-UNPORTED",
            format!(
                "community {}: unknown command; pij community --help",
                call.verb
            ),
        )),
        "templates" => Ok(pod_templates::listing()),
        "adopt" => adopt(&state, &call, &request.caller, &actor).await,
        "template-diff" => template_diff(&call, &request.caller),
        "export" => export(&state, &call, &request.caller, &actor).await,
        "plan" => plan(&state, &call, &request.caller, &actor)
            .await
            .map(|planned| planned.answer()),
        "up" => up(&state, &call, &request.caller, &actor).await,
        "show" => show(&state, &call, &request.caller).await,
        "confirm" => confirm(&state, &call, &request.caller, &actor).await,
        "set" => set(&state, &call, &request.caller, &actor).await,
        "down" => down(&state, &call, &request.caller, &actor).await,
        "add" => add(&state, &call, &request.caller, &actor).await,
        "release" => release(&state, &call, &request.caller, &actor).await,
        "sessions" => sessions(&state, &call, &request.caller).await,
        "activity" => activity(&state, &call, &request.caller).await,
        "stage" => stage(&state, &call, &request.caller, &actor).await,
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
            "wiring": wiring_answer(&self.template),
            "procedures": procedures_answer(&self.template, &self.repo),
            "warnings": warnings(&self.warnings),
            "writes": "nothing: pij pod up <template> ... starts it",
        })
    }
}

/// How the slots feed each other, as the template says (req-01).
fn wiring_answer(template: &Template) -> Vec<Value> {
    template
        .wiring
        .iter()
        .map(|(from, to, what)| json!({"from": from, "to": to, "what": what}))
        .collect()
}

/// The procedures a template names, each with where it lives and whether it
/// is there (PRD 010 req-01).
fn procedures_answer(template: &Template, repo: &Path) -> Vec<Value> {
    template
        .procedures
        .iter()
        .map(|procedure| {
            json!({
                "id": procedure.id,
                "from": match procedure.from {
                    pod::ProcedureFrom::Template => "template",
                    pod::ProcedureFrom::Repo => "repo",
                },
                "use": procedure.used_for,
                "present": pod::procedure_present(template, repo, procedure),
            })
        })
        .collect()
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

/// The template a command names: a shipped one (`shipped:<name>[@v<n>]`,
/// PRD 010 req-02), else the folder or file at that path.
///
/// # Errors
/// What reading it refuses with.
fn load_template(given: &str, caller: &CallerContext) -> Result<Template, Refusal> {
    match pod_templates::named(given) {
        Some(shipped) => pod_templates::read(shipped?),
        None => pod::read(&template_path(given, caller)?, given),
    }
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
    pod::refuse_secret_parameters(&call.sets)?;
    let template = load_template(given, caller)?;
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
                    // The same focus up gives the slot's spawn (#346 R1-10).
                    focus: slot.seed.clone(),
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
    let mut warnings = pod::checks(&template, &plans, &|role| live.contains(role), &exists);
    warnings.extend(pod::procedure_checks(&template, &|procedure| {
        pod::procedure_present(&template, &repo, procedure)
    }));
    Ok(Planned {
        shown: if pod_templates::named(given).is_some() {
            template.source.clone()
        } else {
            recorded_source(given, &repo, caller)
        },
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

/// Apply `change` to the pod's record as it is inside the writer's
/// serialized build, never to a snapshot read before it (#346 B346-R1-02):
/// the commit, the record written and `change`'s own value.
///
/// # Errors
/// `change`'s refusal, a missing record, or any writer refusal.
async fn mutate<T: Send + 'static>(
    state: &AppState,
    record: &Record,
    what: &str,
    change: impl FnOnce(&Path, Value) -> Result<(Value, T), Refusal> + Send + 'static,
) -> Result<(String, Value, T), Refusal> {
    let (paths, pod_name, project) = (
        record.paths.clone(),
        record.pod.clone(),
        record.project.clone(),
    );
    let (written, (document, value)) = state
        .services
        .directory
        .write_built(
            &record.project,
            record.repo.clone(),
            format!("pij pod {} {what}", record.pod),
            move |root| {
                let current = recorded(&root.worktree, &project, &pod_name)?;
                let (document, value) = change(&root.worktree, current)?;
                let file = pod_file(&document, &paths, Existing::Replace);
                Ok((vec![file], (document, value)))
            },
        )
        .await?;
    Ok((written.commit, document, value))
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
    // B346-R1-04: record the template as resolved, so a later verb reads the
    // same one wherever it is asked from.
    let resolved = Template {
        source: planned.shown.clone(),
        ..planned.template.clone()
    };
    let mut document = pod::document(
        &resolved,
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
    let seats: Vec<String> = started.iter().map(|(_, seat)| seat.clone()).collect();
    let mut lines: Vec<String> = late
        .iter()
        .map(|warning| format!("warning: {}", warning.line()))
        .collect();
    lines.push(format!(
        "started {}",
        if seats.is_empty() {
            "no seat".to_string()
        } else {
            seats.join(", ")
        }
    ));
    let (by, new_members) = (actor.to_string(), members.clone());
    match mutate(
        state,
        &record_of(&planned),
        &format!("up: its members (by {actor})"),
        move |_, document| {
            let mut document = pod::with_members(document, new_members);
            for line in &lines {
                document = pod::logged(document, &at, &by, line);
            }
            Ok((document, ()))
        },
    )
    .await
    {
        Ok((commit, _, ())) => answer["members_commit"] = json!(commit),
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
        canary(state, record_of(&planned), actor, asked).await;
    }
    Ok(answer)
}

fn record_of(planned: &Planned) -> Record {
    Record {
        pod: planned.pod.clone(),
        project: planned.project.clone(),
        repo: planned.repo.clone(),
        worktree: planned.worktree.clone(),
        paths: planned.paths.clone(),
    }
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
/// that ran `up` which slots are degraded and how to fix them. A pod taken
/// down before the answers settle gets nothing written and nothing sent
/// (#346 B346-R1-09).
async fn settle(
    state: &AppState,
    record: &Record,
    actor: &SeatId,
    asked: &[(pod::Slot, String)],
    confirmed: &std::collections::BTreeMap<String, Vec<String>>,
) {
    let pod_name = record.pod.clone();
    let at = holders::now();
    let (asked_owned, confirmed_owned, name) =
        (asked.to_vec(), confirmed.clone(), pod_name.clone());
    let settled = mutate(state, record, "up: its tool canary", move |_, document| {
        if pod::meta(&document)["state"] != "up" {
            return Err(Refusal::new(E_POD_NOT_UP, "the pod was taken down first"));
        }
        let mut document = document;
        let mut degraded = Vec::new();
        for (slot, seat) in &asked_owned {
            let missing = pod::missing(
                slot,
                confirmed_owned.get(seat).map_or(&[][..], Vec::as_slice),
            );
            let outcome = if missing.is_empty() {
                "confirmed".to_string()
            } else {
                let line = pod::degraded(&name, &missing, seat);
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
        Ok((document, degraded))
    })
    .await;
    let degraded = match settled {
        Ok((_, _, degraded)) => degraded,
        Err(refusal) if refusal.code == E_POD_NOT_UP => return,
        Err(refusal) => {
            eprintln!("pij-rs could not record pod {pod_name}'s canary: {refusal}");
            return;
        }
    };
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
    pod::refuse_secret_parameters(&[(name.to_string(), value.to_string())])?;
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
    let (by, name_owned, value_owned) = (actor.to_string(), name.to_string(), value.to_string());
    let (committed, document, ()) = mutate(
        state,
        &opened.record,
        &format!("set {name} (by {actor})"),
        move |_, current| {
            let document = pod::logged(
                pod::with_parameter(current, &name_owned, &value_owned),
                &at,
                &by,
                &format!("set {name_owned}={value_owned}"),
            );
            Ok((document, ()))
        },
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
    let document = opened.document;
    let mut warnings = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    // PRD 010 req-01: the pij CLI runs the template's close procedure in the
    // caller's process before it asks for down; this records what it did.
    // B347-R1-01: a template pij cannot read may name a close procedure, so
    // down refuses rather than assume none, unless the caller skips it (a
    // brake: removing it only closes more). The skip is logged either way.
    let template = match template_of(&opened.record.repo, &document) {
        Ok(template) => Some(template),
        Err(refusal) if call.skip_close_procedure => {
            lines.push(format!(
                "close procedure unknown, skipped (--skip-close-procedure): the template cannot be read ({refusal})"
            ));
            None
        }
        Err(refusal) => {
            return Err(Refusal::new(
                pod::E_TEMPLATE,
                format!(
                    "pod {pod_name}: its template cannot be read ({refusal}), so its close procedure is unknown; nothing was closed. Restore the template, or go on without it: pij pod down {pod_name} --skip-close-procedure"
                ),
            ));
        }
    };
    if let Some(template) = &template
        && let Some(procedure) = template.close_procedure()
    {
        let key = format!("pod-{pod_name}");
        let line = if call.close_procedure_ran {
            format!("close procedure {} ran (key {key})", procedure.id)
        } else if call.skip_close_procedure {
            format!(
                "close procedure {} skipped (--skip-close-procedure)",
                procedure.id
            )
        } else {
            warnings.push(format!(
                "the template's close procedure {} did not run: down was asked for without the pij CLI, which runs it first. Run it: pij proc {} --template {} --key {key}",
                procedure.id,
                procedure.id,
                template.folder.display()
            ));
            format!("close procedure {} not run", procedure.id)
        };
        lines.push(line);
    }
    let work_folder = pod::work_folder(&document);
    let freeze = work_folder
        .as_deref()
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
        lines.push(line.clone());
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
    lines.extend(warnings.iter().map(|warning| format!("warning: {warning}")));
    lines.push(format!(
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
    ));
    let by = actor.to_string();
    let (committed, _, ()) = mutate(
        state,
        &opened.record,
        &format!("down (by {actor})"),
        move |_, current| {
            let mut document = pod::with_state(current, "closing");
            for line in &lines {
                document = pod::logged(document, &at, &by, line);
            }
            Ok((document, ()))
        },
    )
    .await?;
    Ok(json!({
        "pod": pod_name,
        "project": opened.record.project,
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
    let (by, logged_warnings) = (actor.to_string(), warnings.clone());
    let (committed, _, ()) = mutate(
        state,
        &opened.record,
        &format!("up again (by {actor})"),
        move |_, current| {
            let mut document = pod::with_state(current, "up");
            for warning in &logged_warnings {
                document = pod::logged(document, &at, &by, &format!("warning: {warning}"));
            }
            Ok((pod::logged(document, &at, &by, "up again by name"), ()))
        },
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
    pod::refuse_secret_parameters(&call.sets)?;
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
            "cwd": pod::work_folder(&opened.document)
                .filter(|folder| Path::new(folder).is_dir())
                .or_else(|| caller.cwd.clone())
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
    let (by, seat_id, pod_owned, row) = (
        actor.to_string(),
        seat.id.to_string(),
        pod_name.to_string(),
        pod::member(&slot, seat.id.as_str(), "none asked"),
    );
    let line = format!("added {target}, already running, in role {}", slot.role);
    let (committed, _, ()) = mutate(
        state,
        &opened.record,
        &format!("add {target} (by {actor})"),
        move |worktree, current| {
            if pod::records(&current, "members")
                .iter()
                .any(|member| member["seat"] == seat_id.as_str())
            {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("{seat_id} is already a member of pod {pod_owned}"),
                ));
            }
            pod::refuse_second_pod(worktree, &pod_owned, &seat_id)?;
            let document = pod::logged(pod::with_members(current, vec![row]), &at, &by, &line);
            Ok((document, ()))
        },
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
    let at = holders::now();
    let (by, seat, pod_owned) = (actor.to_string(), target.to_string(), pod_name.to_string());
    let (committed, _, ()) = mutate(
        state,
        &opened.record,
        &format!("release {target} (by {actor})"),
        move |_, current| {
            let (document, removed) = pod::without_member(current, &seat);
            if removed.is_none() {
                return Err(Refusal::new(
                    "E-RS-NOT-FOUND",
                    format!("{seat} is not a member of pod {pod_owned}"),
                ));
            }
            let line = format!("released {seat}; it keeps running");
            Ok((pod::logged(document, &at, &by, &line), ()))
        },
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

/// The event kinds `pod activity` reads for a member, in one indexed query.
const ACTIVITY_KINDS: [&str; 6] = [
    "seat.activity",
    "report.now",
    "message.pushed",
    "memory.note",
    "spawn.launched",
    "seat.tombstone",
];

/// The first line of `body`, at most 120 characters, as a send's summary.
fn summary(body: &str) -> String {
    let line = body.lines().next().unwrap_or_default();
    let mut shown: String = line.chars().take(120).collect();
    if line.chars().count() > 120 || body.lines().nth(1).is_some() {
        shown.push('…');
    }
    shown
}

/// One activity line for a member's event, or `None` for one not shown.
fn activity_line(kind: &str, seat: &str, payload: &Value) -> Option<String> {
    Some(match kind {
        "seat.activity" => format!("{seat} is {}", payload["state"].as_str()?),
        "report.now" => format!(
            "{seat} reported: {}{}",
            payload["did"].as_str().unwrap_or("-"),
            payload["next"]
                .as_str()
                .map(|next| format!("; next: {next}"))
                .unwrap_or_default()
        ),
        "message.pushed" => format!(
            "{} -> {seat}: {}",
            payload["from"].as_str().unwrap_or("?"),
            summary(payload["body"].as_str().unwrap_or_default())
        ),
        "memory.note" => format!(
            "{seat} wrote a memory note for {}",
            payload["role"].as_str().unwrap_or("?")
        ),
        "spawn.launched" => format!("{seat} started"),
        "seat.tombstone" => format!("{seat} closed"),
        _ => return None,
    })
}

/// When `--since` names: milliseconds since the epoch, or a time as holders
/// documents write it (`2026-11-25T04:00:00.000Z`).
fn since_ms(given: &str) -> Result<u64, Refusal> {
    if let Ok(ms) = given.parse::<u64>() {
        return Ok(ms);
    }
    holders::parse_time(given)
        .and_then(|at| u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok())
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                "--since takes milliseconds since the epoch or a time like 2026-11-25T04:00:00.000Z",
            )
        })
}

/// `pij pod activity <pod> [--since <time>]` (req-15): what the pod did, in
/// time order: its members' state changes, reports, memory notes, outputs and
/// the sends they received, and the sends members made to its consulted
/// roles' holders; then where each member's transcript is. Open to every
/// seat: no membership or role test. Reads only.
async fn activity(state: &AppState, call: &Call, caller: &CallerContext) -> Result<Value, Refusal> {
    let pod_name = position(call, 0, "pij pod activity <pod> [--since <time>]")?;
    let since = call
        .since
        .as_deref()
        .map(since_ms)
        .transpose()?
        .unwrap_or(0);
    let opened = open(state, caller, pod_name).await?;
    let members: Vec<(String, String, String)> = pod::records(&opened.document, "members")
        .iter()
        .map(|member| {
            (
                member["seat"].as_str().unwrap_or_default().to_string(),
                member["slot"].as_str().unwrap_or_default().to_string(),
                member["role"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let mut lines: Vec<(u64, Value)> = Vec::new();
    let mut transcripts = Vec::new();
    for (seat, slot, role_id) in &members {
        let id = SeatId::from(seat.as_str());
        let events = state
            .services
            .spine
            .all_matching(&id, &ACTIVITY_KINDS)
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
        for event in events.iter().filter(|event| event.at >= since) {
            let payload: Value = serde_json::from_str(&event.payload).unwrap_or(Value::Null);
            if let Some(line) = activity_line(&event.kind, seat, &payload) {
                lines.push((
                    event.at,
                    json!({"at": event.at, "slot": slot, "seat": seat, "line": line}),
                ));
            }
        }
        let outputs = state
            .services
            .directory
            .store()
            .outputs_for_role(&opened.record.project, role_id, 50)
            .await
            .unwrap_or_default();
        for output in outputs
            .into_iter()
            .filter(|output| &output.writer == seat && output.created_at >= since)
        {
            lines.push((
                output.created_at,
                json!({"at": output.created_at, "slot": slot, "seat": seat,
                       "line": format!("{seat} wrote {} {} on {}: {}", output.role, output.kind, output.subject, output.path)}),
            ));
        }
        let history = super::shim::seat_history(state, &id, 1).await;
        transcripts.push(json!({
            "slot": slot,
            "seat": seat,
            "session": history["sessions"][0]["session"],
            "transcript": history["sessions"][0]["transcript"],
        }));
    }
    for consulted in pod::records(&opened.document, "consulted") {
        let role_id = consulted["role"].as_str().unwrap_or_default();
        for holder in holders_of(state, &opened.record.project, role_id).await {
            let events = state
                .services
                .spine
                .all_matching(&SeatId::from(holder.as_str()), &["message.pushed"])
                .await
                .unwrap_or_default();
            for event in events.iter().filter(|event| event.at >= since) {
                let payload: Value = serde_json::from_str(&event.payload).unwrap_or(Value::Null);
                let from = payload["from"].as_str().unwrap_or_default();
                if let Some((_, slot, _)) = members.iter().find(|(seat, _, _)| seat == from) {
                    let line = format!(
                        "{from} -> {holder} (consulted {role_id}): {}",
                        summary(payload["body"].as_str().unwrap_or_default())
                    );
                    lines.push((
                        event.at,
                        json!({"at": event.at, "slot": slot, "seat": from, "line": line}),
                    ));
                }
            }
        }
    }
    lines.sort_by_key(|(at, _)| *at);
    Ok(json!({
        "pod": pod_name,
        "since": since,
        "activity": lines.into_iter().map(|(_, line)| line).collect::<Vec<_>>(),
        "transcripts": transcripts,
    }))
}

/// `pij pod stage <pod> <deliverable> <stage> --output <path> [--from <slot>]...`
/// (req-16, req-17): record one stage with the slots whose work it folds in
/// (all the stage needs, unless `--from` names fewer). A stage still waiting
/// on a slot is recorded with a warning naming it: a late member never blocks
/// it, and its work is folded into a later stage. When the deliverable names
/// a write-back role, the stage is sent to it as one request (a send to
/// role:<id>), which stays with that role after the pod goes down. One
/// commit per stage.
async fn stage(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    const USAGE: &str =
        "pij pod stage <pod> <deliverable> <stage> --output <path> [--from <slot>]...";
    let pod_name = position(call, 0, USAGE)?;
    let deliverable_id = position(call, 1, USAGE)?;
    let stage_name = position(call, 2, USAGE)?;
    let output = call.output.clone().ok_or_else(|| {
        Refusal::new(
            "E-RS-ARG",
            format!("{USAGE}: --output names the stage's result"),
        )
    })?;
    let opened = open(state, caller, pod_name).await?;
    let deliverable = pod::records(&opened.document, "deliverables")
        .iter()
        .find(|row| row["id"] == deliverable_id)
        .cloned()
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-NOT-FOUND",
                format!("pod {pod_name} has no deliverable {deliverable_id}"),
            )
        })?;
    let stages: Vec<&str> = deliverable["stages"]
        .as_array()
        .map(|stages| stages.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !stages.contains(&stage_name) {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "deliverable {deliverable_id} has stages {}, not {stage_name}",
                stages.join(", ")
            ),
        ));
    }
    let recorded = pod::records(&opened.document, "stages").to_vec();
    if recorded
        .iter()
        .any(|row| row["deliverable"] == deliverable_id && row["stage"] == stage_name)
    {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "stage {stage_name} of {deliverable_id} is already recorded; record the next stage"
            ),
        ));
    }
    let members = pod::records(&opened.document, "members").to_vec();
    let needs = pod::stage_needs(&deliverable, stage_name, &members);
    let from = if call.from.is_empty() {
        needs.clone()
    } else {
        call.from.clone()
    };
    let mut warnings = Vec::new();
    let view = pod::stage_view(&deliverable, &recorded, &members);
    let waiting: Vec<String> = view
        .iter()
        .find(|row| row["stage"] == stage_name)
        .and_then(|row| row["waits_on"].as_array())
        .map(|slots| {
            slots
                .iter()
                .filter_map(Value::as_str)
                .filter(|slot| !from.iter().any(|given| given == slot))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !waiting.is_empty() {
        warnings.push(format!(
            "stage {stage_name} of {deliverable_id} is recorded without the work of {}; fold it into a later stage with pij pod stage {pod_name} {deliverable_id} <stage> --output <path> --from <slot>",
            waiting.join(", ")
        ));
    }
    // B346-R1-07: the write-back is pij's own dispatch, so a role held by a
    // degraded member of this pod is not sent it (the same one-way brake as
    // set; removing it only sends more). Direct messages are untouched.
    let degraded_holder = match deliverable["write_back"].as_str() {
        None => None,
        Some(role_id) => {
            let holders = holders_of(state, &opened.record.project, role_id).await;
            members
                .iter()
                .filter(|member| !pod::may_dispatch(member))
                .filter_map(|member| member["seat"].as_str())
                .find(|seat| holders.iter().any(|holder| holder == seat))
                .map(|seat| (role_id.to_string(), seat.to_string()))
        }
    };
    if let Some((role_id, seat)) = &degraded_holder {
        warnings.push(format!(
            "the write-back to role {role_id} was not sent: its holder {seat} is a degraded member of this pod, so pij hands it no pod work; send it yourself once it has its tools: pij send role:{role_id} ..."
        ));
    }
    let write_back = match deliverable["write_back"].as_str() {
        None => None,
        Some(_) if degraded_holder.is_some() => None,
        Some(role_id) => Some(
            write_back(
                state,
                caller,
                actor,
                &opened.record,
                role_id,
                deliverable_id,
                stage_name,
                &output,
            )
            .await,
        ),
    };
    if let Some(Err(why)) = &write_back {
        warnings.push(why.clone());
    }
    let request = write_back
        .as_ref()
        .and_then(|sent| sent.as_ref().ok())
        .cloned();
    let at = holders::now();
    let mut row = json!({
        "deliverable": deliverable_id,
        "stage": stage_name,
        "output": output,
        "from": from,
        "by": actor,
        "at": at,
    });
    if let Some(request) = &request {
        row["write_back"] = json!(request);
    }
    let mut lines = vec![format!(
        "stage {stage_name} of {deliverable_id} recorded: {output}{}",
        request
            .as_ref()
            .map(|request| format!("; written back as {request}"))
            .unwrap_or_default()
    )];
    lines.extend(warnings.iter().map(|warning| format!("warning: {warning}")));
    let (by, deliverable_owned, stage_owned) = (
        actor.to_string(),
        deliverable_id.to_string(),
        stage_name.to_string(),
    );
    let (committed, _, ()) = mutate(
        state,
        &opened.record,
        &format!("stage {stage_name} of {deliverable_id} (by {actor})"),
        move |_, current| {
            if pod::records(&current, "stages")
                .iter()
                .any(|row| row["deliverable"] == deliverable_owned.as_str() && row["stage"] == stage_owned.as_str())
            {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("stage {stage_owned} of {deliverable_owned} is already recorded; record the next stage"),
                ));
            }
            let mut document = pod::with_stage(current, row);
            for line in &lines {
                document = pod::logged(document, &at, &by, line);
            }
            Ok((document, ()))
        },
    )
    .await?;
    Ok(json!({
        "pod": pod_name,
        "deliverable": deliverable_id,
        "stage": stage_name,
        "from": from,
        "write_back": request,
        "warnings": warnings,
        "commit": committed,
    }))
}

/// Send a recorded stage to `role_id` as one request, through the route a
/// `pij send role:<id>` takes: the request's id, or why it was not sent.
#[allow(clippy::too_many_arguments)]
async fn write_back(
    state: &AppState,
    caller: &CallerContext,
    actor: &SeatId,
    record: &Record,
    role_id: &str,
    deliverable_id: &str,
    stage_name: &str,
    output: &str,
) -> Result<String, String> {
    let msg_id = state.services.delivery.next_message_id();
    let to = pij_core::model::Destination {
        seat: SeatId::from(format!("role:{role_id}")),
        machine: None,
    };
    let body = format!(
        "pij pod {}: stage {stage_name} of deliverable {deliverable_id} is recorded: {output}. Write back what it teaches into role {role_id}'s knowledge.",
        record.pod
    );
    let routed = super::role_send::route(
        state,
        super::role_send::RoleSendCall {
            command: "pij pod stage",
            from: actor,
            to: &to,
            body: &body,
            msg_id: &msg_id,
            in_reply_to: None,
            fyi: false,
            control: false,
            force: false,
            reason: None,
            forwarded: false,
            project: Some(record.project.clone()),
            options: crate::delivery::role_send::RoleOptions::default(),
            caller: Some(caller),
        },
    )
    .await
    .ok_or_else(|| format!("role:{role_id} is not a role address"))?;
    let answer = answer_of(routed).await?;
    if answer["ok"] != true {
        return Err(format!(
            "the write-back to role {role_id} was not sent: {}",
            text_of(&answer["meta"])
        ));
    }
    Ok(match &answer["data"]["request"]["id"] {
        Value::String(id) => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => msg_id,
    })
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
    if let Some(shipped) = pod_templates::named(&source) {
        return pod_templates::read(shipped?);
    }
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
    let mut warning: Option<String> = None;
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
    let record = Record {
        pod: pod_name.to_string(),
        project: project.to_string(),
        repo: repo.to_path_buf(),
        worktree,
        paths,
    };
    let row = pod::member(
        &slot,
        seat.as_str(),
        if asked { "pending" } else { "none asked" },
    );
    let mut lines = vec![format!(
        "started {seat} as slot {} (pij spawn --as --pod)",
        slot.id
    )];
    lines.extend(warning.iter().map(|warning| format!("warning: {warning}")));
    let (by, seat_owned, pod_owned) = (creator.to_string(), seat.to_string(), pod_name.to_string());
    let (committed, _, ()) = mutate(
        state,
        &record,
        &format!("{seat} joins as slot {} (by {creator})", slot.id),
        move |worktree, current| {
            pod::refuse_second_pod(worktree, &pod_owned, &seat_owned)?;
            let mut document = pod::with_members(current, vec![row]);
            for line in &lines {
                document = pod::logged(document, &at, &by, line);
            }
            Ok((document, ()))
        },
    )
    .await?;
    if asked {
        canary(
            state,
            record,
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
async fn confirm(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
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
    let recovered = recover(state, caller, pod, actor, &call.tools).await;
    Ok(json!({"pod": pod, "seat": actor, "confirmed": call.tools, "canary": recovered}))
}

/// A confirmation after the canary settled (#346 B346-R1-03): a degraded
/// member's canary is worked out again from the tools it names now and
/// recorded through the writer, so the slot recovers once its tools are
/// there. A member still within the first wait is left to it. The brake
/// stays one-way: only this member's own confirmation lifts it.
async fn recover(
    state: &AppState,
    caller: &CallerContext,
    pod_name: &str,
    seat: &SeatId,
    tools: &[String],
) -> Option<String> {
    let opened = open(state, caller, pod_name).await.ok()?;
    let template = template_of(&opened.record.repo, &opened.document).ok();
    let member = pod::records(&opened.document, "members")
        .iter()
        .find(|member| member["seat"] == seat.as_str())?
        .clone();
    if pod::may_dispatch(&member) {
        return None;
    }
    let missing = template
        .as_ref()
        .and_then(|template| {
            template
                .slots
                .iter()
                .find(|slot| member["slot"] == slot.id.as_str())
        })
        .map(|slot| pod::missing(slot, tools))
        .unwrap_or_default();
    let outcome = if missing.is_empty() {
        "confirmed".to_string()
    } else {
        pod::degraded(pod_name, &missing, seat.as_str())
    };
    let (at, seat_owned, written) = (holders::now(), seat.to_string(), outcome.clone());
    let saved = mutate(
        state,
        &opened.record,
        &format!("canary {seat} again"),
        move |_, current| {
            let still_degraded = pod::records(&current, "members")
                .iter()
                .any(|member| member["seat"] == seat_owned.as_str() && !pod::may_dispatch(member));
            if !still_degraded {
                return Err(Refusal::new(E_POD_NOT_UP, "nothing to recover"));
            }
            let document = pod::with_canary(current, &seat_owned, &written);
            let line = format!("canary {seat_owned} (confirmed again): {written}");
            Ok((pod::logged(document, &at, &seat_owned, &line), ()))
        },
    )
    .await;
    saved.ok().map(|_| outcome)
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
            shown["folder"] = json!(row.folder);
        }
        shown["type"] = json!(role_type(
            &worktree,
            member["role"].as_str().unwrap_or_default()
        ));
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
        shown["stages"] = json!(pod::stage_view(
            row,
            pod::records(&document, "stages"),
            pod::records(&document, "members"),
        ));
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
        "wiring": template_of(&repo, &document).map(|template| wiring_answer(&template)).unwrap_or_default(),
        "close_procedure": close_procedure(&repo, &document, pod_name),
        "log": pod::records(&document, "log"),
    }))
}

/// The procedure `pij pod down` runs first, from the template the record
/// names, with what the pij CLI needs to run it (PRD 010 req-01); null when
/// the template names none or cannot be read.
fn close_procedure(repo: &Path, document: &Value, pod_name: &str) -> Value {
    let template = match template_of(repo, document) {
        Ok(template) => template,
        // B347-R1-01: unreadable is not "none"; the pij CLI refuses down on it.
        Err(refusal) => {
            return json!({"error": refusal.to_string(), "key": format!("pod-{pod_name}")});
        }
    };
    template.close_procedure().map_or(Value::Null, |procedure| {
        json!({
            "id": procedure.id,
            // A shipped template has no folder: its procedures come from the
            // repo or the pij skill, so the CLI runs them without one.
            "template": if template.folder.as_os_str().is_empty() { Value::Null } else { json!(template.folder.display().to_string()) },
            "key": format!("pod-{pod_name}"),
            "present": pod::procedure_present(&template, repo, procedure),
        })
    })
}

// --- Shipped templates, adopt, diff and init (PRD 010 s02, plan 254)

fn why_of(call: &Call, usage: &str) -> Result<String, Refusal> {
    call.why
        .clone()
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                format!("{usage}: --why <text> is required; it goes in the commit"),
            )
        })
}

/// Where copies go: governance (one writer commit for all of them) or a
/// caller-relative path in the repo (written as files for the repo to
/// commit). `folder_of` names each copy's folder. A copy already there is
/// kept and listed, never overwritten.
struct Copies {
    wrote: Vec<Value>,
    kept: Vec<Value>,
    governance: Vec<NewFile>,
}

fn place(
    copies: &mut Copies,
    shipped: &Shipped,
    governance_root: &Path,
    into: Option<&Path>,
    folder: PathBuf,
) -> Result<(), Refusal> {
    let shown = folder.display().to_string();
    let exists = match into {
        Some(_) => folder.join(pod::TEMPLATE_FILE).exists(),
        None => governance_root
            .join(&folder)
            .join(pod::TEMPLATE_FILE)
            .exists(),
    };
    if exists {
        copies.kept.push(json!({
            "template": shipped.reference(),
            "path": shown,
            "why": "a template is already there; compare it with pij pod template-diff",
        }));
        return Ok(());
    }
    let files = pod_templates::copy_files(shipped, &folder)?;
    match into {
        Some(_) => {
            for (path, bytes) in &files {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        Refusal::new(
                            "E-RS-ARG",
                            format!("cannot create {}: {error}", parent.display()),
                        )
                    })?;
                }
                std::fs::write(path, bytes).map_err(|error| {
                    Refusal::new(
                        "E-RS-ARG",
                        format!("cannot write {}: {error}", path.display()),
                    )
                })?;
            }
            copies
                .wrote
                .push(json!({"template": shipped.reference(), "path": shown, "where": "repo"}));
        }
        None => {
            copies
                .governance
                .extend(files.into_iter().map(|(path, bytes)| NewFile {
                    path,
                    document: false,
                    bytes,
                    existing: Existing::Refuse(Refusal::new(
                        pod::E_TEMPLATE,
                        format!("a template is already at {shown}"),
                    )),
                }));
            copies.wrote.push(
                json!({"template": shipped.reference(), "path": shown, "where": "governance"}),
            );
        }
    }
    Ok(())
}

async fn commit_copies(
    state: &AppState,
    project: &str,
    repo: &Path,
    files: Vec<NewFile>,
    message: String,
) -> Result<Option<String>, Refusal> {
    if files.is_empty() {
        return Ok(None);
    }
    let (written, _) = state
        .services
        .directory
        .write(project, repo.to_path_buf(), files, message)
        .await?;
    Ok(Some(written.commit))
}

/// `pij pod adopt <name>[@v<n>] [--into <path>] --why <text>` (req-02):
/// copy a shipped template to governance, or to the folder `--into` names,
/// with `meta.from` recording its version. Launches no seat.
async fn adopt(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    const USAGE: &str = "pij pod adopt <name>[@v<n>] [--into <path>] --why <text>";
    let spec = position(call, 0, USAGE)?;
    let why = why_of(call, USAGE)?;
    let (name, version) = crate::community::templates::parse_spec(
        spec.strip_prefix(pod_templates::PREFIX).unwrap_or(spec),
    )?;
    let shipped = pod_templates::get(&name, version)?;
    let (project, repo) = project_and_repo(state, caller, None).await?;
    let worktree = root::find(&repo)?.worktree;
    let into = call
        .into
        .as_deref()
        .map(|given| template_path(given, caller))
        .transpose()?;
    let folder = into
        .clone()
        .unwrap_or_else(|| pod_templates::governance_folder(shipped.name));
    let mut copies = Copies {
        wrote: Vec::new(),
        kept: Vec::new(),
        governance: Vec::new(),
    };
    place(&mut copies, shipped, &worktree, into.as_deref(), folder)?;
    if let Some(kept) = copies.kept.first() {
        return Err(Refusal::new(
            pod::E_TEMPLATE,
            format!(
                "{} already holds a template; compare it with upstream: pij pod template-diff {}",
                text_of(&kept["path"]),
                text_of(&kept["path"])
            ),
        ));
    }
    let commit = commit_copies(
        state,
        &project,
        &repo,
        copies.governance,
        format!(
            "community: adopt pod template {}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}",
            shipped.reference()
        ),
    )
    .await?;
    let wrote = copies.wrote.remove(0);
    let next = if commit.is_some() {
        format!(
            "edit it at {}; name it in pod commands by that path",
            text_of(&wrote["path"])
        )
    } else {
        format!(
            "commit {} in your repo; name it in pod commands by that path",
            text_of(&wrote["path"])
        )
    };
    Ok(json!({
        "template": shipped.reference(),
        "wrote": [wrote],
        "commit": commit,
        "next": next,
    }))
}

/// `pij pod template-diff <path>` (req-02): what changed upstream since the
/// copy at `path` was taken. Applies nothing; in-process, launches nothing.
fn template_diff(call: &Call, caller: &CallerContext) -> Result<Value, Refusal> {
    let given = position(call, 0, "pij pod template-diff <path>")?;
    let path = template_path(given, caller)?;
    let file = if path.is_dir() {
        path.join(pod::TEMPLATE_FILE)
    } else {
        path
    };
    let bytes = std::fs::read(&file).map_err(|error| {
        Refusal::new(
            pod::E_TEMPLATE,
            format!("cannot read {}: {error}", file.display()),
        )
    })?;
    pod_templates::upstream_diff(&bytes, given)
}

/// `pij community init --seeds all --templates <names> [--into <path>] --why
/// <text>` (req-03): a prime's setup job. Copies each type's seed and the
/// named pod templates into governance in one commit, and with `--into` the
/// templates to `<path>/<name>/` as files; keeps (and lists) anything already
/// there; prints what it wrote and where. Launches no seat.
async fn community_init(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    use crate::community::types;
    const USAGE: &str = "pij community init [--seeds all|<type>,...] [--templates <name>,...] [--into <path>] --why <text>";
    let why = why_of(call, USAGE)?;
    if call.seeds.is_none() && call.templates.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("{USAGE}: name what to set up; pij copies nothing on its own"),
        ));
    }
    let seed_types: Vec<&types::TypeDef> = match call.seeds.as_deref() {
        None => Vec::new(),
        Some("all") => types::all().iter().collect(),
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(types::get)
            .collect::<Result<_, _>>()?,
    };
    let shipped: Vec<&Shipped> = call
        .templates
        .iter()
        .map(|spec| {
            crate::community::templates::parse_spec(spec)
                .and_then(|(name, version)| pod_templates::get(&name, version))
        })
        .collect::<Result<_, _>>()?;
    let (project, repo) = project_and_repo(state, caller, None).await?;
    let worktree = root::find(&repo)?.worktree;
    let into = call
        .into
        .as_deref()
        .map(|given| template_path(given, caller))
        .transpose()?;
    let mut copies = Copies {
        wrote: Vec::new(),
        kept: Vec::new(),
        governance: Vec::new(),
    };
    for def in seed_types {
        let path = types::seed_path(&def.id);
        let shown = path.display().to_string();
        if worktree.join(&path).exists() {
            copies.kept.push(
                json!({"seed": def.id, "path": shown, "why": "this repo already has its own seed"}),
            );
            continue;
        }
        copies.governance.push(NewFile {
            path,
            bytes: def.seed.as_bytes().to_vec(),
            document: false,
            existing: Existing::Refuse(Refusal::new(
                "E-RS-EXISTS",
                format!("a seed is already at {shown}"),
            )),
        });
        copies
            .wrote
            .push(json!({"seed": def.id, "path": shown, "where": "governance"}));
    }
    for template in shipped {
        let folder = match &into {
            Some(into) => into.join(template.name),
            None => pod_templates::governance_folder(template.name),
        };
        place(&mut copies, template, &worktree, into.as_deref(), folder)?;
    }
    let commit = commit_copies(
        state,
        &project,
        &repo,
        copies.governance,
        format!(
            "community: init (seeds and pod templates)\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}"
        ),
    )
    .await?;
    let in_repo = copies.wrote.iter().any(|row| row["where"] == "repo");
    Ok(json!({
        "project": project,
        "wrote": copies.wrote,
        "kept": copies.kept,
        "commit": commit,
        "next": if in_repo {
            "commit the templates written in the repo; name each in pod commands by its path"
        } else {
            "name each template in pod commands by its path"
        },
    }))
}

/// The regular files under `dir`, each read only after its canonical path is
/// found inside `bound` (B347-R1-04). A link that leads out of `bound`, or
/// cannot be resolved, refuses naming its path and never its content; one
/// that stays inside is not followed (what it points to is copied as itself).
/// A brake: removing it only ever copies more.
fn files_under(dir: &Path, bound: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, Refusal> {
    let escapes = |path: &Path| {
        Refusal::new(
            pod::E_TEMPLATE,
            format!(
                "{} leads outside the template's folder; pij copies nothing from outside a template. Replace the link with the file it means, or remove it",
                path.display()
            ),
        )
    };
    let Ok(root) = std::fs::canonicalize(bound) else {
        return Ok(Vec::new());
    };
    let inside =
        |path: &Path| std::fs::canonicalize(path).is_ok_and(|real| real.starts_with(&root));
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if !inside(dir) {
        return Err(escapes(dir));
    }
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                if !inside(&path) {
                    return Err(escapes(&path));
                }
            } else if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && inside(&path)
                && let (Ok(bytes), Ok(relative)) = (std::fs::read(&path), path.strip_prefix(dir))
            {
                found.push((relative.to_path_buf(), bytes));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// `pij pod export <pod> --as-template <path> --why <text>` (req-04): the pod
/// as it ran, members added at run time included, written as a template at
/// the caller-relative path: one governance commit when the path is in the
/// governance worktree, files for the repo to commit otherwise. The
/// original template's own procedures go with it. No seat id, path or
/// parameter value is written. Launches no seat.
async fn export(
    state: &AppState,
    call: &Call,
    caller: &CallerContext,
    actor: &SeatId,
) -> Result<Value, Refusal> {
    const USAGE: &str = "pij pod export <pod> --as-template <path> --why <text>";
    let pod_name = position(call, 0, USAGE)?;
    let why = why_of(call, USAGE)?;
    let given = call
        .as_template
        .as_deref()
        .ok_or_else(|| Refusal::new("E-RS-ARG", format!("{USAGE}: --as-template names where")))?;
    let opened = open(state, caller, pod_name).await?;
    let dest = template_path(given, caller)?;
    if dest.join(pod::TEMPLATE_FILE).exists() {
        return Err(Refusal::new(
            pod::E_TEMPLATE,
            format!("{given} already holds a template; export to a new folder"),
        ));
    }
    let original = template_of(&opened.record.repo, &opened.document).ok();
    let original_document: Option<Value> = match &original {
        Some(template) if template.folder.as_os_str().is_empty() => {
            pod_templates::named(&template.source)
                .and_then(Result::ok)
                .and_then(|shipped| serde_json::from_slice(shipped.template_bytes()).ok())
        }
        Some(template) => std::fs::read(template.folder.join(pod::TEMPLATE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok()),
        None => None,
    };
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| pij_core::render::is_name(name))
        .unwrap_or_else(|| pod_name.to_string());
    let document = pod_templates::exported(
        &name,
        pod_name,
        &opened.document,
        original_document.as_ref(),
    );
    let bytes = role::to_bytes(&document);
    pod::parse(&bytes, given)?;
    let mut files = vec![(PathBuf::from(pod::TEMPLATE_FILE), bytes)];
    if let Some(template) = &original
        && !template.folder.as_os_str().is_empty()
    {
        files.extend(
            files_under(&template.folder.join("procedures"), &template.folder)?
                .into_iter()
                .map(|(inside, bytes)| (Path::new("procedures").join(inside), bytes)),
        );
    }
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = dest.parent().map(canonical).unwrap_or_default();
    let worktree = canonical(&opened.record.worktree);
    let inside = parent
        .strip_prefix(&worktree)
        .ok()
        .map(|inside| inside.join(dest.file_name().unwrap_or_default()));
    let shown = dest.display().to_string();
    let (commit, place) = match inside {
        Some(relative) => {
            let new_files = files
                .into_iter()
                .map(|(path, bytes)| NewFile {
                    path: relative.join(path),
                    bytes,
                    document: false,
                    existing: Existing::Refuse(Refusal::new(
                        pod::E_TEMPLATE,
                        format!("{given} already holds files"),
                    )),
                })
                .collect();
            let commit = commit_copies(
                state,
                &opened.record.project,
                &opened.record.repo,
                new_files,
                format!(
                    "community: pod {pod_name} exported as template {name}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {}",
                    opened.record.project
                ),
            )
            .await?;
            (commit, "governance")
        }
        None => {
            for (path, bytes) in &files {
                let target = dest.join(path);
                if let Some(folder) = target.parent() {
                    std::fs::create_dir_all(folder).map_err(|error| {
                        Refusal::new(
                            "E-RS-ARG",
                            format!("cannot create {}: {error}", folder.display()),
                        )
                    })?;
                }
                std::fs::write(&target, bytes).map_err(|error| {
                    Refusal::new(
                        "E-RS-ARG",
                        format!("cannot write {}: {error}", target.display()),
                    )
                })?;
            }
            (None, "repo")
        }
    };
    Ok(json!({
        "pod": pod_name,
        "wrote": [{"template": format!("pod {pod_name}"), "path": shown, "where": place}],
        "slots": pod::records(&opened.document, "members").iter().map(|member| member["slot"].clone()).collect::<Vec<_>>(),
        "commit": commit,
        "next": format!("plan it: pij pod plan {given} --set <name>=<value>"),
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

    #[test]
    fn an_activity_line_names_the_seat_and_a_send_shows_one_short_line() {
        assert_eq!(
            activity_line("seat.activity", "pij-a", &json!({"state": "idle"})).as_deref(),
            Some("pij-a is idle")
        );
        let long = format!("{}\nsecond line", "x".repeat(200));
        let line = activity_line(
            "message.pushed",
            "pij-a",
            &json!({"from": "pij-b", "body": long}),
        )
        .unwrap();
        assert!(line.starts_with("pij-b -> pij-a: xxx"), "{line}");
        assert_eq!(line.chars().count(), "pij-b -> pij-a: ".len() + 121);
        assert!(activity_line("delivery.outcome", "pij-a", &json!({})).is_none());
        assert_eq!(since_ms("1795579000000").unwrap(), 1_795_579_000_000);
        assert_eq!(since_ms("2026-11-25T00:00:00.000Z").unwrap() % 1000, 0);
        assert!(since_ms("yesterday").is_err());
    }
}
