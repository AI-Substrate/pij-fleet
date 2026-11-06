//! `pij directory add` (plan 190, roadmap 001 s02): write a role into the
//! community directory through the daemon's single writer. Plan 194 (s03):
//! `add --from template:<name>[@v<n>]|file:<brief>`, `templates` and
//! `template-diff <id>`.
//!
//! The argv/caller wire is governance's ([`GovernanceRequest`]). Flags may
//! repeat (`--rule`, `--scope-path`, …), which governance's parser refuses,
//! so this family parses its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind, SeatId};
use serde_json::{Value, json};

use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope};
use crate::community::layout::{RolePaths, exists};
use crate::community::role::{self, RoleFields};
use crate::community::writer::{Existing, NewFile, Written};
use crate::community::{Refusal, pointer};
use crate::community::{holders, memory};

const COMMAND: &str = "pij directory";
const SINGLE: [&str; 27] = [
    "why",
    "from",
    "to",
    "from-project",
    "role",
    "project",
    "file",
    "batch",
    "actor",
    "title",
    "kind",
    "lifecycle",
    "holders",
    "instance-key",
    "default-limit",
    "summary",
    "template",
    "template-version",
    "harness",
    "model",
    "effort",
    "packet",
    "brief",
    "hook",
    "body",
    "body-file",
    "note",
];
const REPEATED: [&str; 7] = [
    "scope-path",
    "rule",
    "out-of-scope",
    "forbidden",
    "input",
    "output-kind",
    "tags",
];
/// Flags a `memory` call may carry (plan 201).
const MEMORY: [&str; 8] = [
    "title",
    "tags",
    "hook",
    "body",
    "body-file",
    "note",
    "project",
    "actor",
];
/// Flags that steer the write rather than describe a role.
const CONTROL: [&str; 8] = [
    "why",
    "from",
    "project",
    "file",
    "batch",
    "actor",
    "from-project",
    "role",
];

#[derive(Debug, Default)]
struct Arguments {
    leaf: String,
    positions: Vec<String>,
    single: BTreeMap<String, String>,
    repeated: BTreeMap<String, Vec<String>>,
}

impl Arguments {
    fn parse(argv: &[String]) -> Result<Self, Refusal> {
        // A bare `pij directory` (or one with only flags) is the view.
        let view = argv.len() == 1 || argv.get(1).is_some_and(|token| token.starts_with("--"));
        match argv {
            [family, ..] if family == "directory" && view => {}
            [family, leaf, ..]
                if family == "directory"
                    && matches!(
                        leaf.as_str(),
                        "add"
                            | "assign"
                            | "release"
                            | "rebuild"
                            | "external"
                            | "lineage"
                            | "templates"
                            | "template-diff"
                            | "memory"
                    ) => {}
            [family, leaf, ..] if family == "directory" => {
                return Err(Refusal::new(
                    "E-RS-UNPORTED",
                    format!("directory {leaf}: unknown command; run `pij directory --help`"),
                ));
            }
            _ => {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    "usage: pij directory add <id> --why <text> …",
                ));
            }
        }
        let mut call = Self {
            leaf: if view {
                "view".to_string()
            } else {
                argv[1].clone()
            },
            ..Self::default()
        };
        let mut tokens = argv[if view { 1 } else { 2 }..].iter();
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
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!("directory add: unsupported --{name}"),
                ));
            }
        }
        let external_only = ["from-project", "role"];
        if call.leaf != "external"
            && external_only
                .iter()
                .any(|flag| call.single.contains_key(*flag))
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "directory {}: --from-project and --role belong to `external add`",
                    call.leaf
                ),
            ));
        }
        if call.leaf == "external"
            && (call.positions.len() != 2
                || call.positions[0] != "add"
                || call.single.keys().any(|flag| {
                    !matches!(
                        flag.as_str(),
                        "why" | "project" | "actor" | "from-project" | "role"
                    )
                })
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory external add <seat> --from-project <slug> --why <note> [--role <id>]",
            ));
        }
        if matches!(call.leaf.as_str(), "templates" | "template-diff")
            && (call.positions.len() != usize::from(call.leaf == "template-diff")
                || call
                    .single
                    .keys()
                    .any(|flag| !matches!(flag.as_str(), "project" | "actor"))
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory templates | pij directory template-diff <id> [--project <slug>]",
            ));
        }
        if call.leaf == "lineage"
            && (call.positions.len() != 3
                || call.positions[0] != "add"
                || call.single.keys().any(|flag| {
                    !matches!(flag.as_str(), "why" | "project" | "actor" | "from" | "to")
                })
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text> [--project <slug>]",
            ));
        }
        if call.leaf != "lineage" && call.single.contains_key("to") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {}: --to belongs to `lineage add`", call.leaf),
            ));
        }
        if !matches!(call.leaf.as_str(), "add" | "lineage") && call.single.contains_key("from") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {}: --from belongs to `add`", call.leaf),
            ));
        }
        if matches!(call.leaf.as_str(), "view" | "rebuild")
            && (!call.positions.is_empty()
                || call
                    .single
                    .keys()
                    .any(|flag| !matches!(flag.as_str(), "project" | "actor"))
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {} takes only [--project <slug>]", call.leaf),
            ));
        }
        if call.leaf == "memory" {
            call.check_memory()?;
        }
        if matches!(call.leaf.as_str(), "assign" | "release")
            && (call.describes_a_role()
                || call.get("file").is_some()
                || (call.get("batch").is_some()
                    && (call.leaf == "release" || !call.positions.is_empty())))
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "directory {}: takes <role>[/<instance>] <seat> --why <text> [--project <slug>]{}",
                    call.leaf,
                    if call.leaf == "assign" {
                        ", or --batch <file> --why <text>"
                    } else {
                        ""
                    }
                ),
            ));
        }
        Ok(call)
    }

    /// `memory add <role> --title --tags --hook --body|--body-file`, or
    /// `memory <role> [--note <slug>]`. There is no other memory verb: nothing
    /// is ever deleted.
    fn check_memory(&self) -> Result<(), Refusal> {
        let flags: Vec<&str> = self
            .single
            .keys()
            .chain(self.repeated.keys())
            .map(String::as_str)
            .collect();
        if let Some(flag) = flags.iter().find(|flag| !MEMORY.contains(flag)) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory memory: unsupported --{flag}"),
            ));
        }
        let writes = ["title", "tags", "hook", "body", "body-file"];
        match self.positions.as_slice() {
            [verb, _] if verb == "add" => {
                if self.get("note").is_some() {
                    return Err(Refusal::new(
                        "E-RS-ARG",
                        "directory memory add: --note belongs to reading",
                    ));
                }
                if self.get("body").is_some() && self.get("body-file").is_some() {
                    return Err(Refusal::new(
                        "E-RS-ARG",
                        "give the body as --body or --body-file, not both",
                    ));
                }
                Ok(())
            }
            [role] if role != "add" => match flags.iter().find(|flag| writes.contains(flag)) {
                Some(flag) => Err(Refusal::new(
                    "E-RS-ARG",
                    format!("directory memory {role}: --{flag} belongs to `memory add`"),
                )),
                None => Ok(()),
            },
            [verb, ..] if verb != "add" && self.positions.len() > 1 => Err(Refusal::new(
                "E-RS-UNPORTED",
                format!(
                    "directory memory {verb}: unknown verb; memory has add and read, and nothing is ever deleted"
                ),
            )),
            _ => Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory memory add <role> --title <t> --hook <h> [--tags <a,b>] --body <text>|--body-file <path>; pij directory memory <role> [--note <slug>]",
            )),
        }
    }

    fn get(&self, name: &str) -> Option<String> {
        self.single.get(name).cloned()
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.repeated.get(name).cloned().unwrap_or_default()
    }

    fn describes_a_role(&self) -> bool {
        !self.repeated.is_empty()
            || self
                .single
                .keys()
                .any(|key| !CONTROL.contains(&key.as_str()))
    }

    fn fields(&self) -> RoleFields {
        RoleFields {
            title: self.get("title"),
            kind: self.get("kind"),
            lifecycle: self.get("lifecycle"),
            holders: self.get("holders"),
            instance_key: self.get("instance-key"),
            default_limit: self.get("default-limit"),
            summary: self.get("summary"),
            template: self.get("template"),
            template_version: self.get("template-version"),
            scope_paths: self.all("scope-path"),
            rules: self.all("rule"),
            out_of_scope: self.all("out-of-scope"),
            forbidden: self.all("forbidden"),
            harness: self.get("harness"),
            model: self.get("model"),
            effort: self.get("effort"),
            inputs: self.all("input"),
            output_kinds: self.all("output-kind"),
            packet: self.get("packet"),
            brief: self.get("brief"),
        }
    }
}

/// `POST /v1/directory`.
pub(crate) async fn handle(
    State(state): State<AppState>,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    if request
        .argv
        .first()
        .is_some_and(|family| family == "directory")
        && pij_core::directory::wants_help(&request.argv[1..])
    {
        let verb = pij_core::directory::verb(&request.argv[1..]);
        return match pij_core::directory::help(verb) {
            Some(text) => envelope(
                StatusCode::OK,
                &Envelope::ok(COMMAND, json!({"help": text})),
            ),
            None => refused(&Refusal::new(
                "E-RS-UNPORTED",
                format!("directory {}: unknown command", verb.unwrap_or_default()),
            )),
        };
    }
    let call = match Arguments::parse(&request.argv) {
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
    let result = match call.leaf.as_str() {
        "assign" | "release" => hold(&state, &actor.id, &request.caller, &call).await,
        "external" => external(&state, &actor.id, &request.caller, &call).await,
        "lineage" => lineage(&state, &actor.id, &request.caller, &call).await,
        "templates" => Ok(json!({"templates": state.services.directory.templates().listing()})),
        "template-diff" => {
            match project_and_repo(&state, &request.caller, call.get("project")).await {
                Ok((project, repo)) => state
                    .services
                    .directory
                    .template_diff(&repo, &call.positions[0])
                    .map(|mut data| {
                        data["project"] = json!(project);
                        data
                    }),
                Err(refusal) => Err(refusal),
            }
        }
        "memory" => memory(&state, &actor.id, &request.caller, &call).await,
        "view" | "rebuild" => {
            match project_and_repo(&state, &request.caller, call.get("project")).await {
                Ok((project, repo)) if call.leaf == "view" => {
                    state.services.directory.view(&project, &repo).await
                }
                Ok((project, repo)) => state.services.directory.rebuild(&project, repo).await,
                Err(refusal) => Err(refusal),
            }
        }
        _ => add(&state, actor.id.as_str(), &request.caller, &call).await,
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(COMMAND, data)),
        Err(refusal) => refused(&refusal),
    }
}

async fn add(
    state: &AppState,
    actor: &str,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                "--why <text> is required: it becomes the changelog row",
            )
        })?;
    // UTC to the millisecond with its zone, like holders rows (F13).
    let date = crate::community::holders::now();
    let batch = call.get("batch");
    let mut fields = call.fields();
    let from = call
        .get("from")
        .map(|from| Source::parse(&from))
        .transpose()?;
    match &from {
        Some(_) if batch.is_some() || call.get("file").is_some() => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "--from takes no --file or --batch: it is where the role comes from",
            ));
        }
        Some(Source::File(_)) if fields.brief.is_some() => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "--from file:<brief> is --brief <brief>: give one, not both",
            ));
        }
        Some(Source::File(brief)) => fields.brief = Some(brief.clone()),
        Some(Source::Template(..))
            if fields.template.is_some() || fields.template_version.is_some() =>
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "--from template: records the template and version itself; drop --template and --template-version",
            ));
        }
        _ => {}
    }
    let documents = match (&batch, call.get("file"), call.positions.as_slice()) {
        (Some(dir), None, []) if !call.describes_a_role() => {
            read_batch(&caller_path(caller, dir)?)?
        }
        (Some(_), _, _) => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "--batch <dir> takes no id, --file or role flags: each document names its own meta.id",
            ));
        }
        (None, Some(file), [id]) if !call.describes_a_role() => {
            vec![(id.clone(), read_document(&caller_path(caller, &file)?)?)]
        }
        (None, Some(_), [_]) => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "give the role as --file <json> or as flags, not both",
            ));
        }
        (None, None, [id]) => match &from {
            Some(Source::Template(name, version)) => {
                let document = state
                    .services
                    .directory
                    .templates()
                    .role_from(name, *version, id)?;
                vec![(id.clone(), role::apply_fields(document, &fields)?)]
            }
            _ => vec![(id.clone(), role::from_fields(id, &fields)?)],
        },
        (None, _, _) => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "directory add takes exactly one role id",
            ));
        }
    };

    let mut files = Vec::new();
    let mut ids = Vec::new();
    for (id, document) in documents {
        let paths = RolePaths::new(&id)?;
        if ids.contains(&id) {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("the batch names role {id:?} twice"),
            ));
        }
        let document = role::stamp(document, &id, actor, &why, &date)?;
        let dir = paths
            .document
            .parent()
            .expect("a role document sits in its folder");
        files.push(NewFile {
            path: paths.document.clone(),
            bytes: role::to_bytes(&document),
            document: true,
            existing: Existing::Refuse(exists(&id, &paths.document)),
        });
        // An existing folder's own memory and outputs are adopted as they are.
        files.push(NewFile {
            path: paths.memory_index,
            bytes: RolePaths::empty_memory_index(&id).into_bytes(),
            document: false,
            existing: Existing::Skip(dir.join("memory")),
        });
        files.push(NewFile {
            path: paths.outputs_keep,
            bytes: Vec::new(),
            document: false,
            existing: Existing::Skip(dir.join("outputs")),
        });
        ids.push(id);
    }

    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let subject = match ids.as_slice() {
        [id] => format!("community: add role {id}"),
        ids => format!(
            "community: import {} roles\n\n{}",
            ids.len(),
            ids.join(", ")
        ),
    };
    let message = format!("{subject}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}");
    let written: Written = state
        .services
        .directory
        .write(&project, repo, files, message)
        .await?;

    let pointers = pointer::for_write(&project, &written.paths, &written.commit);
    state
        .services
        .governance
        .store()
        .record_community_pointers(&pointers)
        .await
        .map_err(|error| {
            Refusal::new(
                "E-RS-STORE",
                format!(
                    "committed {} but could not record the pointer: {error}",
                    written.commit
                ),
            )
        })?;
    let roles: Vec<Value> = ids
        .iter()
        .map(|id| {
            let path = RolePaths::new(id).expect("checked above").document;
            json!({"id": id, "path": path.display().to_string()})
        })
        .collect();
    let mut data = json!({
        "project": project,
        "commit": written.commit,
        "worktree": written.root.worktree.display().to_string(),
        "paths": written.paths,
        "roles": roles,
    });
    if let [only] = roles.as_slice() {
        data["id"] = only["id"].clone();
        data["path"] = only["path"].clone();
    }
    Ok(data)
}

/// Where `add --from` takes a role from (plan 194, req-13).
#[derive(Debug)]
enum Source {
    /// `template:<name>[@v<n>]`: a copy of a shipped template version.
    Template(String, Option<u32>),
    /// `file:<brief>`: the spelling of `--brief`; referenced, not copied.
    File(String),
}

impl Source {
    fn parse(value: &str) -> Result<Self, Refusal> {
        if let Some(spec) = value.strip_prefix("template:") {
            let (name, version) = crate::community::templates::parse_spec(spec)?;
            return Ok(Self::Template(name, version));
        }
        match value.strip_prefix("file:") {
            Some(brief) if !brief.is_empty() => Ok(Self::File(brief.to_string())),
            _ => Err(Refusal::new(
                "E-RS-ARG",
                format!("--from {value:?}: use template:<name>[@v<n>] or file:<brief>"),
            )),
        }
    }
}

/// `directory assign|release <role>[/<instance>] <seat> --why <text>` (plan 195).
async fn hold(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    if call.leaf == "assign"
        && let Some(file) = call.get("batch")
    {
        return assign_batch(state, actor, caller, call, &file).await;
    }
    let [target, seat] = call.positions.as_slice() else {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("directory {} takes <role>[/<instance>] <seat>", call.leaf),
        ));
    };
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| Refusal::new("E-RS-ARG", "--why <text> is required"))?;
    let (role, instance) = role_and_instance(target)?;
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let seat_id = SeatId::from(seat.as_str());
    let directory = &state.services.directory;
    let receipt = if call.leaf == "assign" {
        let descriptor = state
            .services
            .registry
            .get(&seat_id)
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
            .ok_or_else(|| Refusal::new("E-RS-NOT-FOUND", format!("seat {seat} does not exist")))?;
        directory
            .assign(crate::community::directory::AssignRequest {
                project: project.clone(),
                repo,
                role: role.to_string(),
                instance: instance.to_string(),
                seat: descriptor,
                actor: actor.clone(),
                why,
                machine: state.machine_alias.clone(),
            })
            .await?
    } else {
        directory
            .release(&project, repo, role, instance, &seat_id, actor, &why)
            .await?
    };
    let mut data = serde_json::to_value(&receipt).expect("a receipt serialises");
    data["project"] = json!(project);
    data["role"] = json!(role);
    data["seat"] = json!(seat);
    Ok(data)
}

/// `directory lineage add <role>[/<instance>] <seat> --from <time> --to <time>
/// --why <text>` (plan 208, F15): the prime records a closed seat's past
/// holding. No live seat is needed.
async fn lineage(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    use crate::community::holders::{parse_time, stamp};
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| Refusal::new("E-RS-ARG", "--why <text> is required"))?;
    let (role, instance) = role_and_instance(&call.positions[1])?;
    let seat = call.positions[2].clone();
    let when = |flag: &str| -> Result<time::OffsetDateTime, Refusal> {
        let given = call
            .get(flag)
            .ok_or_else(|| Refusal::new("E-RS-ARG", format!("--{flag} <time> is required")))?;
        parse_time(&given).ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                format!(
                    "--{flag} {given:?}: give RFC 3339 with Z or an offset (2026-11-04T09:00:00+10:00), or a date (UTC midnight)"
                ),
            )
        })
    };
    let (from, to) = (when("from")?, when("to")?);
    if from >= to {
        return Err(Refusal::new("E-RS-ARG", "--from must be before --to"));
    }
    if to > time::OffsetDateTime::now_utc() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "--to is in the future; lineage add records holdings that have ended (use assign for a live seat)",
        ));
    }
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let project_prime = state
        .services
        .governance
        .store()
        .list_projects()
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
        .into_iter()
        .find(|known| known.slug == project)
        .and_then(|known| known.prime_id);
    // The project's own prime; a project that names none falls back to a
    // seat whose role is prime.
    let is_prime = match &project_prime {
        Some(prime) => prime == actor,
        None => {
            state
                .services
                .roles
                .read_role(actor)
                .await
                .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
                .as_deref()
                == Some("prime")
        }
    };
    if !is_prime {
        return Err(Refusal::new(
            "E-RS-OWNERSHIP",
            format!("only the prime records past holdings; {actor} is not the prime of {project}"),
        ));
    }
    let row = pij_store::Assignment {
        project: project.clone(),
        role: role.to_string(),
        instance: instance.to_string(),
        seat: seat.clone(),
        harness: None,
        session: None,
        machine: None,
        from: stamp(from),
        to: Some(stamp(to)),
        assigned_by: actor.to_string(),
        released_by: Some(actor.to_string()),
        why: why.clone(),
        release_why: Some("lineage add".to_string()),
        end_pending: false,
    };
    let receipt = state
        .services
        .directory
        .lineage_add(repo, row, actor)
        .await?;
    let mut data = serde_json::to_value(&receipt).expect("a receipt serialises");
    data["project"] = json!(project);
    data["role"] = json!(role);
    data["seat"] = json!(seat);
    Ok(data)
}

/// `<role>[/<instance>]`, checked.
fn role_and_instance(target: &str) -> Result<(&str, &str), Refusal> {
    let (role, instance) = target.split_once('/').unwrap_or((target, ""));
    RolePaths::new(role)?;
    if instance.contains('/') || instance.trim() != instance {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("bad instance key {instance:?}"),
        ));
    }
    Ok((role, instance))
}

/// `directory assign --batch <file> --why <text>` (plan 208, F16): one
/// `<role>[/<instance>] <seat>` per line, blank and `#` lines skipped; every
/// holding in one writer commit, all or nothing. Every bad line is named.
async fn assign_batch(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
    file: &str,
) -> Result<Value, Refusal> {
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| Refusal::new("E-RS-ARG", "--why <text> is required"))?;
    let path = caller_path(caller, file)?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let mut requests = Vec::new();
    let mut problems = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [target, seat] = fields.as_slice() else {
            problems.push(format!("line {number}: want `<role>[/<instance>] <seat>`"));
            continue;
        };
        let (role, instance) = match role_and_instance(target) {
            Ok(parsed) => parsed,
            Err(refusal) => {
                problems.push(format!("line {number}: {}", refusal.message));
                continue;
            }
        };
        let descriptor = state
            .services
            .registry
            .get(&SeatId::from(*seat))
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
        match descriptor {
            None => problems.push(format!("line {number}: seat {seat} does not exist")),
            Some(descriptor) if descriptor.tombstoned_at.is_some() => {
                problems.push(format!("line {number}: seat {seat} is closed"));
            }
            Some(descriptor) => requests.push(crate::community::directory::AssignRequest {
                project: project.clone(),
                repo: repo.clone(),
                role: role.to_string(),
                instance: instance.to_string(),
                seat: descriptor,
                actor: actor.clone(),
                why: why.clone(),
                machine: state.machine_alias.clone(),
            }),
        }
    }
    if !problems.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("the batch wrote nothing: {}", problems.join("; ")),
        ));
    }
    if requests.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("{} names no holdings", path.display()),
        ));
    }
    let receipts = state.services.directory.assign_many(requests).await?;
    Ok(json!({
        "project": project,
        "commit": receipts[0].commit,
        "holdings": receipts,
    }))
}

/// `directory external add <seat> --from-project <slug> --why <note>`.
async fn external(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| Refusal::new("E-RS-ARG", "--why <text> is required"))?;
    let from = call
        .get("from-project")
        .filter(|from| !from.trim().is_empty())
        .ok_or_else(|| Refusal::new("E-RS-ARG", "--from-project <slug> is required"))?;
    let role = call.get("role");
    if let Some(role) = &role {
        RolePaths::new(role)?;
    }
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let (written, entry) = state
        .services
        .directory
        .external_add(
            &project,
            repo,
            &call.positions[1],
            &from,
            role.as_deref(),
            &why,
            actor,
        )
        .await?;
    Ok(json!({
        "project": project,
        "commit": written.commit,
        "path": crate::community::layout::EXTERNAL,
        "entry": entry,
    }))
}

/// `directory memory add <role> …` writes one note (AC1); `directory memory
/// <role> [--note <slug>]` reads the INDEX or one note (AC3), in-process.
async fn memory(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let positions: Vec<&str> = call.positions.iter().map(String::as_str).collect();
    let ["add", role_id] = positions.as_slice() else {
        let role_id = positions[0];
        let root = crate::community::root::find(&repo)?;
        let mut data = match call.get("note") {
            Some(note) => memory::read_note(&root.worktree, role_id, &note)?,
            None => memory::read_index(&root.worktree, role_id)?,
        };
        data["project"] = json!(project);
        return Ok(data);
    };
    let role_id = (*role_id).to_string();
    let body = match (call.get("body"), call.get("body-file")) {
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
        _ => String::new(),
    };
    let fields = memory::NoteFields {
        title: call.get("title").unwrap_or_default(),
        tags: call
            .all("tags")
            .iter()
            .flat_map(|tags| tags.split(','))
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect(),
        hook: call.get("hook").unwrap_or_default(),
        body,
    };
    // Refuse a bad note before taking the writer lock.
    memory::note(&holders::today(), &fields)?;
    let (written, note) = state
        .services
        .directory
        .memory_add(&project, repo, &role_id, fields, actor)
        .await?;
    let pointers = pointer::for_write(&project, &written.paths, &written.commit);
    state
        .services
        .governance
        .store()
        .record_community_pointers(&pointers)
        .await
        .map_err(|error| {
            Refusal::new(
                "E-RS-STORE",
                format!(
                    "committed {} but could not record the pointer: {error}",
                    written.commit
                ),
            )
        })?;
    Ok(json!({
        "project": project,
        "role": role_id,
        "commit": written.commit,
        "path": note,
        "paths": written.paths,
    }))
}

/// The project the write belongs to and the repository to find governance from:
/// `--project`, else the project whose repo holds the caller's directory.
pub(crate) async fn project_and_repo(
    state: &AppState,
    caller: &CallerContext,
    named: Option<String>,
) -> Result<(String, PathBuf), Refusal> {
    let projects = state
        .services
        .governance
        .store()
        .list_projects()
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
    let cwd = caller
        .cwd
        .as_deref()
        .filter(|cwd| Path::new(cwd).is_absolute());
    let project = match named {
        Some(slug) => projects
            .into_iter()
            .find(|project| project.slug == slug)
            .ok_or_else(|| {
                Refusal::new("E-RS-NOT-FOUND", format!("project {slug:?} does not exist"))
            })?,
        None => {
            let cwd = cwd.ok_or_else(|| {
                Refusal::new(
                    "E-RS-ARG",
                    "name the project with --project <slug>, or call from inside its repo",
                )
            })?;
            match_project(projects, Path::new(cwd))?
        }
    };
    let repo = project
        .repo
        .clone()
        .or_else(|| cwd.map(str::to_string))
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                "the project records no repo and the caller sent no absolute cwd",
            )
        })?;
    Ok((project.slug, PathBuf::from(repo)))
}

/// The project for a caller in `cwd`: the one whose repo holds it (the deepest),
/// else the one whose repo shares its common git dir, so a linked worktree
/// finds its project (plan 195, AC15). Read in-process, no launch.
fn match_project(
    projects: Vec<pij_core::orchestration::Project>,
    cwd: &Path,
) -> Result<pij_core::orchestration::Project, Refusal> {
    let here = canonical(cwd);
    let common = crate::community::root::common_dir(&here);
    let mut held: Vec<(usize, pij_core::orchestration::Project)> = Vec::new();
    let mut shared: Vec<pij_core::orchestration::Project> = Vec::new();
    for project in projects {
        let Some(repo) = project
            .repo
            .as_deref()
            .map(|repo| canonical(Path::new(repo)))
        else {
            continue;
        };
        if here.starts_with(&repo) {
            held.push((repo.as_os_str().len(), project));
        } else if common.is_some() && crate::community::root::common_dir(&repo) == common {
            shared.push(project);
        }
    }
    if let Some((_, project)) = held.into_iter().max_by_key(|(depth, _)| *depth) {
        return Ok(project);
    }
    match shared.len() {
        1 => Ok(shared.remove(0)),
        0 => {
            let main = common.as_deref().and_then(Path::parent).map_or_else(
                || "<main checkout>".to_string(),
                |main| main.display().to_string(),
            );
            Err(Refusal::new(
                "E-RS-NOT-FOUND",
                format!("no project records the repository holding {}", here.display()),
            )
            .with_next(format!(
                "Register it: pij project create <name> --repo {main} --prime <seat>; or name a project with --project <slug>."
            )))
        }
        _ => Err(Refusal::new(
            "E-RS-ARG",
            format!(
                "several projects share this repository ({}); name one with --project <slug>",
                shared
                    .iter()
                    .map(|project| project.slug.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// `/tmp` and `/private/tmp` are one directory on macOS: compare real paths.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn caller_path(caller: &CallerContext, path: &str) -> Result<PathBuf, Refusal> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return Ok(path);
    }
    caller
        .cwd
        .as_deref()
        .filter(|cwd| Path::new(cwd).is_absolute())
        .map(|cwd| Path::new(cwd).join(&path))
        .ok_or_else(|| Refusal::new("E-RS-ARG", "a relative path needs an absolute caller.cwd"))
}

fn read_document(path: &Path) -> Result<Value, Refusal> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    serde_json::from_str(&text).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("{} is not JSON: {error}", path.display()),
        )
    })
}

/// Every `*.dd.json` directly in `dir`, by file name, with the id each names.
fn read_batch(dir: &Path) -> Result<Vec<(String, Value)>, Refusal> {
    let entries = std::fs::read_dir(dir).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("cannot read {}: {error}", dir.display()),
        )
    })?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && path.to_string_lossy().ends_with(".dd.json"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("{} holds no *.dd.json role documents", dir.display()),
        ));
    }
    paths
        .iter()
        .map(|path| {
            let document = read_document(path)?;
            let id = role::id_of(&document)
                .ok_or_else(|| {
                    Refusal::new("E-RS-ARG", format!("{} has no meta.id", path.display()))
                })?
                .to_string();
            Ok((id, document))
        })
        .collect()
}

pub(super) fn refused(refusal: &Refusal) -> Response {
    let status = match refusal.code {
        "E-RS-ARG" | "E-RS-UNPORTED" => StatusCode::BAD_REQUEST,
        "E-RS-OWNERSHIP" | "E-RS-OUTPUT-NOT-HOLDER" => StatusCode::FORBIDDEN,
        "E-RS-NOT-FOUND" | memory::E_NOTE_MISSING => StatusCode::NOT_FOUND,
        "E-RS-STORE" | crate::community::E_GOV_GIT => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    };
    let mut body = Envelope::<Value>::refused(
        COMMAND,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    #[test]
    fn flags_repeat_where_the_schema_holds_a_list_and_refuse_elsewhere() {
        let call = Arguments::parse(&argv(&[
            "directory",
            "add",
            "qa",
            "--rule",
            "one",
            "--rule=two",
            "--title",
            "QA",
            "--why",
            "w",
            "--json",
        ]))
        .unwrap();
        assert_eq!(call.positions, ["qa"]);
        assert_eq!(call.fields().rules, ["one", "two"]);
        assert!(call.describes_a_role());
        assert_eq!(
            Arguments::parse(&argv(&[
                "directory",
                "add",
                "qa",
                "--title",
                "a",
                "--title",
                "b"
            ]))
            .unwrap_err()
            .message,
            "duplicate --title"
        );
        assert_eq!(
            Arguments::parse(&argv(&["directory", "add", "qa", "--force", "x"]))
                .unwrap_err()
                .code,
            "E-RS-ARG"
        );
        assert_eq!(
            Arguments::parse(&argv(&["directory", "evict", "qa"]))
                .unwrap_err()
                .code,
            "E-RS-UNPORTED"
        );
        let plain = Arguments::parse(&argv(&[
            "directory",
            "add",
            "--batch",
            "d",
            "--why",
            "w",
            "--project",
            "p",
        ]))
        .unwrap();
        assert!(!plain.describes_a_role());
    }

    #[test]
    fn from_names_a_template_or_a_file_and_belongs_to_add() {
        assert!(matches!(
            Source::parse("template:pm@v2").unwrap(),
            Source::Template(name, Some(2)) if name == "pm"
        ));
        assert!(matches!(
            Source::parse("template:security").unwrap(),
            Source::Template(name, None) if name == "security"
        ));
        assert!(matches!(
            Source::parse("file:briefs/qa.md").unwrap(),
            Source::File(brief) if brief == "briefs/qa.md"
        ));
        for bad in ["pm", "file:", "template:", "template:pm@next", "url:x"] {
            assert_eq!(Source::parse(bad).unwrap_err().code, "E-RS-ARG", "{bad}");
        }
        let call = Arguments::parse(&argv(&[
            "directory",
            "add",
            "pm-001",
            "--from",
            "template:pm",
            "--title",
            "PM 001",
            "--why",
            "w",
        ]))
        .unwrap();
        assert_eq!(call.get("from").as_deref(), Some("template:pm"));
        for refused in [
            &["directory", "assign", "pm", "s", "--from", "template:pm"][..],
            &["directory", "templates", "extra"][..],
            &["directory", "template-diff"][..],
            &["directory", "template-diff", "pm", "--title", "x"][..],
        ] {
            assert_eq!(
                Arguments::parse(&argv(refused)).unwrap_err().code,
                "E-RS-ARG",
                "{refused:?}"
            );
        }
        assert_eq!(
            Arguments::parse(&argv(&["directory", "template-diff", "pm"]))
                .unwrap()
                .positions,
            ["pm"]
        );
        assert_eq!(
            Arguments::parse(&argv(&["directory", "templates"]))
                .unwrap()
                .leaf,
            "templates"
        );
    }
}
