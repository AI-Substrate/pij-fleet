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
use pij_core::model::{Envelope, ErrorKind, Msg, SeatId};
use serde_json::{Value, json};

use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::routes::Trigger;
use super::{AppState, envelope};
use crate::community::directory::{HoldingReceipt, ReleaseTarget, Retirement, RoleEdit, Unmatched};
use crate::community::layout::{RolePaths, exists};
use crate::community::role::{self, RoleFields};
use crate::community::routes::{self, Overlap, Regenerated};
use crate::community::writer::{Existing, NewFile, Written};
use crate::community::{E_NOTHING_TO_WRITE, Refusal, pointer, root};
use crate::community::{holders, memory};

const COMMAND: &str = "pij directory";
const SINGLE: [&str; 30] = [
    "why",
    "into",
    "from",
    "show",
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
    "slug",
];
const REPEATED: [&str; 8] = [
    "set",
    "scope-path",
    "rule",
    "out-of-scope",
    "forbidden",
    "input",
    "output-kind",
    "tags",
];
/// Flags a `memory` call may carry (plan 201).
const MEMORY: [&str; 9] = [
    "slug",
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
const CONTROL: [&str; 11] = [
    "write",
    "why",
    "into",
    "from",
    "show",
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
    /// `--drop-queued` (retire and merge only): a switch, no value.
    drop_queued: bool,
    /// `--suggest` on the view: every role suggestion, not only those a need
    /// or signal points at (F39).
    suggest: bool,
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
                            | "edit"
                            | "retire"
                            | "merge"
                            | "assign"
                            | "release"
                            | "rebuild"
                            | "external"
                            | "routes"
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
            if flag == "drop-queued" {
                call.drop_queued = true;
                continue;
            }
            if flag == "suggest" {
                if call.leaf != "view" {
                    return Err(Refusal::new(
                        "E-RS-ARG",
                        "--suggest belongs to the view: pij directory --suggest",
                    ));
                }
                call.suggest = true;
                continue;
            }
            if flag == "write" {
                if call
                    .single
                    .insert("write".to_string(), String::new())
                    .is_some()
                {
                    return Err(Refusal::new("E-RS-ARG", "duplicate --write"));
                }
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
        if call.leaf != "routes" && call.single.contains_key("write") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {}: --write belongs to `routes`", call.leaf),
            ));
        }
        if call.leaf == "routes"
            && (!call.positions.is_empty()
                || call
                    .single
                    .keys()
                    .any(|flag| !matches!(flag.as_str(), "project" | "actor" | "write" | "why"))
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory routes [--project <slug>] [--write --why <text>]",
            ));
        }
        if matches!(call.leaf.as_str(), "templates" | "template-diff")
            && (call.positions.len() != usize::from(call.leaf == "template-diff")
                || call.single.keys().any(|flag| {
                    !(matches!(flag.as_str(), "project" | "actor")
                        || (flag == "show" && call.leaf == "templates"))
                })
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory templates [--show <name>[@vN]] | pij directory template-diff <id> [--project <slug>]",
            ));
        }
        if call.leaf != "templates" && call.single.contains_key("show") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {}: --show belongs to `templates`", call.leaf),
            ));
        }
        if call.drop_queued && !matches!(call.leaf.as_str(), "retire" | "merge") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "directory {}: --drop-queued belongs to `retire` and `merge`",
                    call.leaf
                ),
            ));
        }
        if call.leaf != "merge" && call.single.contains_key("into") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {}: --into belongs to `merge`", call.leaf),
            ));
        }
        if matches!(call.leaf.as_str(), "retire" | "merge")
            && (call.positions.len() != 1
                || (call.leaf == "merge" && call.get("into").is_none())
                || call
                    .single
                    .keys()
                    .any(|flag| !matches!(flag.as_str(), "why" | "project" | "actor" | "into"))
                || !call.repeated.is_empty())
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                if call.leaf == "merge" {
                    "usage: pij directory merge <from> --into <to> --why <text> [--drop-queued] [--project <slug>]"
                } else {
                    "usage: pij directory retire <role> --why <text> [--drop-queued] [--project <slug>]"
                },
            ));
        }
        if call.leaf != "edit" && call.repeated.contains_key("set") {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("directory {}: --set belongs to `edit`", call.leaf),
            ));
        }
        if call.leaf == "edit"
            && (call.positions.len() != 1
                || call.get("batch").is_some()
                || (call.get("file").is_some() && call.describes_a_role()))
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                "usage: pij directory edit <id> --why <text> (--set <field>=<value>... and role flags | --file <json>)",
            ));
        }
        if call.leaf == "lineage" {
            // One holding by flags, or a file of them with --batch (F67).
            let batch = call.get("batch").is_some();
            let flags = call.single.keys().all(|flag| {
                matches!(flag.as_str(), "why" | "project" | "actor")
                    || (batch && flag == "batch")
                    || (!batch && matches!(flag.as_str(), "from" | "to"))
            });
            if !flags
                || call.positions.first().map(String::as_str) != Some("add")
                || call.positions.len() != if batch { 1 } else { 3 }
                || !call.repeated.is_empty()
            {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    "usage: pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text> [--project <slug>], \
                     or pij directory lineage add --batch <file> --why <text> [--project <slug>]",
                ));
            }
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
                || (call.get("batch").is_some() && !call.positions.is_empty()))
        {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!(
                    "directory {}: takes <role>[/<instance>] <seat> --why <text> [--project <slug>], or --batch <file> --why <text>",
                    call.leaf
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
        let writes = ["title", "tags", "hook", "body", "body-file", "slug"];
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

/// The operation a directory leaf is (plan 223 AC7): holding and lineage are
/// holder and prime actions, reads are machine-public, and every other write
/// forges only authorship.
fn directory_class(leaf: &str) -> Option<super::provenance::OperationClass> {
    use super::provenance::OperationClass;
    match leaf {
        "view" | "templates" | "template-diff" => None,
        "assign" | "release" | "lineage" | "memory" => Some(OperationClass::RoleAndHolder),
        _ => Some(OperationClass::Attribution),
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
        directory_class(&call.leaf),
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
        "edit" => edit(&state, &actor.id, &request.caller, &call).await,
        "retire" | "merge" => reshape(&state, &actor.id, &request.caller, &call).await,
        "routes" => routes_leaf(&state, &actor.id, &request.caller, &call).await,
        "lineage" => lineage(&state, &actor.id, &request.caller, &call).await,
        "templates" => match call.get("show") {
            Some(spec) => state.services.directory.templates().show(&spec),
            None => Ok(json!({"templates": state.services.directory.templates().listing()})),
        },
        "template-diff" => {
            match project_and_repo(&state, &request.caller, call.get("project")).await {
                Ok((project, repo)) => state
                    .services
                    .directory
                    .template_diff(&project, &repo, &call.positions[0])
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
                Ok((project, repo)) if call.leaf == "view" => match live_seats(&state).await {
                    Ok(live) => {
                        state
                            .services
                            .directory
                            .view(&project, &repo, &live, call.suggest)
                            .await
                    }
                    Err(refusal) => Err(refusal),
                },
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

/// The registered, untombstoned seats: one registry read per view.
async fn live_seats(state: &AppState) -> Result<std::collections::HashSet<String>, Refusal> {
    Ok(state
        .services
        .registry
        .list(pij_core::ports::SeatFilter::default())
        .await
        .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
        .into_iter()
        .filter(|seat| seat.tombstoned_at.is_none())
        .map(|seat| seat.id.to_string())
        .collect())
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
    let (written, regenerated): (Written, Regenerated) = state
        .services
        .directory
        .write(&project, repo, files, message)
        .await?;
    record_pointers(state, &project, &written).await?;
    let routes = routes_receipt(state, &project, &written, &regenerated).await;
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
        "routes": routes,
    });
    if !regenerated.table.overlaps.is_empty() {
        data["warnings"] = json!(
            regenerated
                .table
                .overlaps
                .iter()
                .map(Overlap::warning)
                .collect::<Vec<_>>()
        );
    }
    if let [only] = roles.as_slice() {
        data["id"] = only["id"].clone();
        data["path"] = only["path"].clone();
    }
    Ok(data)
}

/// Record the store pointers for the documents a write committed.
async fn record_pointers(
    state: &AppState,
    project: &str,
    written: &Written,
) -> Result<(), Refusal> {
    let pointers = pointer::for_write(project, &written.paths, &written.commit);
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
        })
}

/// What a write did to the routing table, after telling the prime of any
/// changed rows (plan 202).
async fn routes_receipt(
    state: &AppState,
    project: &str,
    written: &Written,
    regenerated: &Regenerated,
) -> Value {
    let prime = super::routes::notify_prime(
        &state.services,
        project,
        &written.commit,
        &regenerated.change,
    )
    .await;
    json!({
        "path": crate::community::layout::ROUTES,
        "regenerated": regenerated.file.is_some(),
        "added": regenerated.change.added,
        "removed": regenerated.change.removed,
        "overlaps": regenerated.table.overlaps,
        "prime": prime,
    })
}

/// `directory routes [--write --why <text>]` (plan 202): the table as a seat
/// sees it, on demand; `--write` regenerates a stale `ROUTES.md` in one commit.
async fn routes_leaf(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let why = call.get("why").filter(|why| !why.trim().is_empty());
    let write = call.get("write").is_some();
    if write && why.is_none() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "--write needs --why <text>: it becomes the commit message",
        ));
    }
    if !write && why.is_some() {
        return Err(Refusal::new("E-RS-ARG", "--why goes with --write"));
    }
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let root = root::find(&repo)?;
    let mut data = json!({"project": project, "path": crate::community::layout::ROUTES});
    let current = routes::regenerate(&root.worktree, &[]).file.is_none();
    // Checked in-process first, so a current file costs no launch.
    if let Some(why) = why.filter(|_| !current) {
        let message =
            format!("community: routes\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}");
        match state
            .services
            .directory
            .write(&project, repo.clone(), Vec::new(), message)
            .await
        {
            Ok((written, regenerated)) => {
                record_pointers(state, &project, &written).await?;
                data["commit"] = json!(written.commit);
                data["written"] = routes_receipt(state, &project, &written, &regenerated).await;
            }
            Err(refusal) if refusal.code == E_NOTHING_TO_WRITE => {
                data["written"] = json!({"regenerated": false, "reason": "ROUTES.md is current"});
            }
            Err(refusal) => return Err(refusal),
        }
    } else if write {
        data["written"] = json!({"regenerated": false, "reason": "ROUTES.md is current"});
    }
    let table = routes::build(&routes::read_roles(&root.worktree));
    let holders = super::routes::live_holders(&state.services, &project).await?;
    data["stale"] = json!(routes::regenerate(&root.worktree, &[]).file.is_some());
    data["text"] = json!(routes::for_seat(&project, &table, &holders));
    data["routes"] = json!(table.routes);
    data["overlaps"] = json!(table.overlaps);
    data["holders"] = json!(holders);
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

/// The meta, default-seat and packet fields `--set <field>=<value>` may change.
const SETTABLE: [&str; 14] = [
    "title",
    "kind",
    "lifecycle",
    "holders",
    "instance_key",
    "default_limit",
    "summary",
    "template",
    "template_version",
    "harness",
    "model",
    "effort",
    "packet",
    "brief",
];

/// The role flags with each `--set <field>=<value>` laid over them.
fn edit_fields(call: &Arguments) -> Result<RoleFields, Refusal> {
    let mut fields = call.fields();
    for set in call.all("set") {
        let (key, value) = set.split_once('=').ok_or_else(|| {
            Refusal::new("E-RS-ARG", format!("--set {set:?}: write <field>=<value>"))
        })?;
        let key = key.trim().replace('-', "_");
        let slot = match key.as_str() {
            "title" => &mut fields.title,
            "kind" => &mut fields.kind,
            "lifecycle" => &mut fields.lifecycle,
            "holders" => &mut fields.holders,
            "instance_key" => &mut fields.instance_key,
            "default_limit" => &mut fields.default_limit,
            "summary" => &mut fields.summary,
            "template" => &mut fields.template,
            "template_version" => &mut fields.template_version,
            "harness" => &mut fields.harness,
            "model" => &mut fields.model,
            "effort" => &mut fields.effort,
            "packet" => &mut fields.packet,
            "brief" => &mut fields.brief,
            _ => {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    format!(
                        "--set {key}: not a field edit can set; use one of {}, the list flags (--rule, --scope-path, ...) or --file",
                        SETTABLE.join(", ")
                    ),
                ));
            }
        };
        if slot.replace(value.to_string()).is_some() {
            return Err(Refusal::new("E-RS-ARG", format!("{key} is given twice")));
        }
    }
    Ok(fields)
}

/// `directory edit <id> --why <text>` (plan 206, F20): change a role that
/// exists, through the writer, with one changelog row.
async fn edit(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let id = &call.positions[0];
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                "--why <text> is required: it becomes the changelog row",
            )
        })?;
    let paths = RolePaths::new(id)?;
    let change = match call.get("file") {
        Some(file) => RoleEdit::Document(read_document(&caller_path(caller, &file)?)?),
        None if call.describes_a_role() => RoleEdit::Fields(Box::new(edit_fields(call)?)),
        None => {
            return Err(Refusal::new(
                "E-RS-ARG",
                "directory edit needs --set <field>=<value>, role flags or --file <json>",
            ));
        }
    };
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let (written, regenerated) = state
        .services
        .directory
        .edit(&project, repo, id, change, actor, &why)
        .await?;
    record_pointers(state, &project, &written).await?;
    let routes = routes_receipt(state, &project, &written, &regenerated).await;
    Ok(json!({
        "project": project,
        "id": id,
        "path": paths.document.display().to_string(),
        "commit": written.commit,
        "paths": written.paths,
        "routes": routes,
    }))
}

/// `directory retire <role>` and `directory merge <from> --into <to>`
/// (plan 206, req-28): one writer commit each.
async fn reshape(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let id = &call.positions[0];
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                "--why <text> is required: it becomes the changelog row",
            )
        })?;
    RolePaths::new(id)?;
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    let directory = &state.services.directory;
    let into = call.get("into");
    let how = Retirement {
        why: &why,
        drop_queued: call.drop_queued,
    };
    let reshaped = match &into {
        Some(into) => {
            directory
                .merge(&project, repo, id, into, actor, how)
                .await?
        }
        None => directory.retire(&project, repo, id, actor, how).await?,
    };
    let (commit, paths, routes) = match &reshaped.written {
        Some(written) => {
            record_pointers(state, &project, written).await?;
            let routes = routes_receipt(state, &project, written, &reshaped.routes).await;
            (json!(written.commit), json!(written.paths), routes)
        }
        None => (Value::Null, json!([]), Value::Null),
    };
    // Tell first, then withdraw: an FYI is held once per msg id, so a
    // withdrawal that fails after this is finished by the same command
    // without telling anyone twice (review of #135, F002).
    let mut told = Vec::with_capacity(reshaped.queued.len());
    for request in &reshaped.queued {
        told.push(json!({
            "id": request.id,
            "requester": request.requester,
            "instance": request.instance,
            "created_at": request.created_at,
            "told": tell_withdrawn(state, actor, request, id, into.as_deref(), &why).await,
        }));
    }
    let resumed = reshaped.written.is_none();
    // Withdraw only once every requester was told (security review of #135,
    // F002): an FYI that could not be held leaves the requests queued behind
    // the retired role, and the same command tells and withdraws them.
    let untold: Vec<String> = told
        .iter()
        .filter(|row| {
            row["told"]
                .as_str()
                .is_some_and(|told| told.starts_with("not told"))
        })
        .map(|row| {
            format!(
                "{} ({})",
                row["requester"].as_str().unwrap_or("?"),
                row["told"].as_str().unwrap_or("?")
            )
        })
        .collect();
    if !untold.is_empty() {
        return Err(Refusal::new(
            crate::community::directory::E_NOT_TOLD,
            format!(
                "role {id:?} is retired{} but {} requester(s) could not be told, so nothing was withdrawn: {}",
                match &commit {
                    Value::String(commit) => format!(" ({commit})"),
                    _ => String::new(),
                },
                untold.len(),
                untold.join(", ")
            ),
        )
        .with_next(format!(
            "Run the same command again to tell them and withdraw: {}",
            reshaped.finish()
        )));
    }
    // A requester that is no longer a seat has nobody to tell: its request is
    // withdrawn, listed in `gone`, and its spine event says so.
    let gone: Vec<String> = told
        .iter()
        .filter(|row| {
            row["told"]
                .as_str()
                .is_some_and(|told| told.starts_with("gone"))
        })
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect();
    directory
        .withdraw(reshaped, &project, id, actor, &why, gone.clone())
        .await?;
    Ok(json!({
        "project": project,
        "id": id,
        "state": "retired",
        "merged_into": into,
        "commit": commit,
        "paths": paths,
        "resumed": resumed,
        "withdrawn": told,
        "gone": gone,
        "routes": routes,
    }))
}

/// Tell a withdrawn request's requester once, as an FYI held for its next
/// real turn (plan 206). Returns the outcome: `held`; `self` when the
/// requester is the seat that withdrew it; `gone: <why>` when the requester is
/// no longer a seat (unknown or tombstoned), so there is nobody to tell; or
/// `not told: <why>`, which holds the withdrawal back.
async fn tell_withdrawn(
    state: &AppState,
    actor: &SeatId,
    request: &pij_store::RoleRequest,
    role: &str,
    into: Option<&str>,
    why: &str,
) -> String {
    if request.requester == actor.as_str() {
        return "self".to_string();
    }
    let gone = match into {
        Some(into) => format!("was merged into {into} (send to role:{into} if it still applies)"),
        None => "was retired".to_string(),
    };
    let msg = Msg {
        from: actor.clone(),
        from_machine: None,
        to: SeatId::from(request.requester.as_str()),
        body: format!(
            "[pij role:{role} request {} withdrawn] Role {role} {gone} by {actor}: {why}. Your request was never delivered. It said:\n{}",
            request.id, request.body
        ),
        msg_id: format!("{}.withdrawn", request.id),
        in_reply_to: None,
        command: None,
    };
    match state.services.delivery.hold_fyi(msg).await {
        Ok(_) => "held".to_string(),
        Err(
            error @ (pij_core::error::PijError::NoRegistryEntry { .. }
            | pij_core::error::PijError::SeatIsGone { .. }),
        ) => format!("gone: {error}"),
        Err(error) => format!("not told: {error}"),
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
    if call.leaf == "release"
        && let Some(file) = call.get("batch")
    {
        return release_batch(state, actor, caller, call, &file).await;
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
        let mut receipt = directory
            .release(&project, repo, role, instance, &seat_id, actor, &why)
            .await?;
        // F57: a live seat's window follows its remaining holdings.
        if let Ok(Some(descriptor)) = state.services.registry.get(&seat_id).await
            && descriptor.tombstoned_at.is_none()
        {
            receipt.window = Some(
                directory
                    .window_after_release(&descriptor, &receipt.row)
                    .await,
            );
        }
        receipt
    };
    let mut data = serde_json::to_value(&receipt).expect("a receipt serialises");
    if call.leaf == "assign" {
        // A change of holder changes no route; the new holder gets the table.
        data["routes"] = super::routes::deliver(
            &state.services,
            &seat_id,
            Some(&project),
            Trigger::Assign,
            None,
        )
        .await;
    }
    data["project"] = json!(project);
    data["role"] = json!(role);
    data["seat"] = json!(seat);
    Ok(data)
}

/// `directory lineage add <role>[/<instance>] <seat> --from <time> --to <time>
/// --why <text>` (plan 208, F15): the prime records a closed seat's past
/// holding. No live seat is needed. With `--batch <file>` (plan 209, F67), one
/// `<role>[/<instance>] <seat> <from> <to>` per line, blank and `#` lines
/// skipped, all in one writer commit or none; every bad line is named.
async fn lineage(
    state: &AppState,
    actor: &SeatId,
    caller: &CallerContext,
    call: &Arguments,
) -> Result<Value, Refusal> {
    let why = call
        .get("why")
        .filter(|why| !why.trim().is_empty())
        .ok_or_else(|| Refusal::new("E-RS-ARG", "--why <text> is required"))?;
    let batch = call.get("batch");
    let single = match &batch {
        Some(_) => None,
        None => Some(past_holding(
            actor,
            &why,
            &call.positions[1],
            &call.positions[2],
            call.get("from").as_deref(),
            call.get("to").as_deref(),
        )?),
    };
    let (project, repo) = project_and_repo(state, caller, call.get("project")).await?;
    require_prime(state, actor, &project).await?;
    let directory = &state.services.directory;
    let Some(file) = batch else {
        let mut row = single.expect("a single lineage add parsed its row");
        row.project.clone_from(&project);
        let (role, seat) = (row.role.clone(), row.seat.clone());
        let receipt = directory.lineage_add(repo, row, actor).await?;
        let mut data = serde_json::to_value(&receipt).expect("a receipt serialises");
        data["project"] = json!(project);
        data["role"] = json!(role);
        data["seat"] = json!(seat);
        return Ok(data);
    };
    let path = caller_path(caller, &file)?;
    let text = std::fs::read_to_string(&path).map_err(|error| {
        Refusal::new(
            "E-RS-ARG",
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [target, seat, from, to] = fields.as_slice() else {
            problems.push(format!(
                "line {number}: want `<role>[/<instance>] <seat> <from> <to>`"
            ));
            continue;
        };
        match past_holding(actor, &why, target, seat, Some(from), Some(to)) {
            Ok(mut row) => {
                row.project.clone_from(&project);
                rows.push(row);
            }
            Err(refusal) => problems.push(format!("line {number}: {}", refusal.message)),
        }
    }
    if !problems.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("the batch wrote nothing: {}", problems.join("; ")),
        ));
    }
    if rows.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("{} names no holdings", path.display()),
        ));
    }
    let receipts = directory.lineage_many(repo, rows, actor).await?;
    Ok(json!({
        "project": project,
        "commit": receipts[0].commit,
        "holdings": receipts,
    }))
}

/// One closed holding, checked: RFC 3339 times (or a date), `from` before
/// `to`, `to` not in the future. `project` is left for the caller.
fn past_holding(
    actor: &SeatId,
    why: &str,
    target: &str,
    seat: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<pij_store::Assignment, Refusal> {
    use crate::community::holders::{parse_time, stamp};
    let (role, instance) = role_and_instance(target)?;
    let when = |flag: &str, given: Option<&str>| -> Result<time::OffsetDateTime, Refusal> {
        let given = given
            .ok_or_else(|| Refusal::new("E-RS-ARG", format!("--{flag} <time> is required")))?;
        parse_time(given).ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                format!(
                    "--{flag} {given:?}: give RFC 3339 with Z or an offset (2026-11-04T09:00:00+10:00), or a date (UTC midnight)"
                ),
            )
        })
    };
    let (from, to) = (when("from", from)?, when("to", to)?);
    if from >= to {
        return Err(Refusal::new("E-RS-ARG", "--from must be before --to"));
    }
    if to > time::OffsetDateTime::now_utc() {
        return Err(Refusal::new(
            "E-RS-ARG",
            "--to is in the future; lineage add records holdings that have ended (use assign for a live seat)",
        ));
    }
    Ok(pij_store::Assignment {
        project: String::new(),
        role: role.to_string(),
        instance: instance.to_string(),
        seat: seat.to_string(),
        harness: None,
        session: None,
        machine: None,
        from: stamp(from),
        to: Some(stamp(to)),
        assigned_by: actor.to_string(),
        released_by: Some(actor.to_string()),
        why: why.to_string(),
        release_why: Some("lineage add".to_string()),
        end_pending: false,
    })
}

/// Only the project's prime records past holdings: its `prime_id`, or for a
/// project that names none, a seat whose role is prime.
async fn require_prime(state: &AppState, actor: &SeatId, project: &str) -> Result<(), Refusal> {
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
    if is_prime {
        Ok(())
    } else {
        Err(Refusal::new(
            "E-RS-OWNERSHIP",
            format!("only the prime records past holdings; {actor} is not the prime of {project}"),
        ))
    }
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
    let mut seats: Vec<SeatId> = requests
        .iter()
        .map(|request| request.seat.id.clone())
        .collect();
    seats.dedup();
    let receipts = state.services.directory.assign_many(requests).await?;
    // Each new holder gets the table once, as a single assign gives it (plan 202).
    let mut routes = serde_json::Map::new();
    for seat in seats {
        if routes.contains_key(seat.as_str()) {
            continue;
        }
        let sent = super::routes::deliver(
            &state.services,
            &seat,
            Some(&project),
            Trigger::Assign,
            None,
        )
        .await;
        routes.insert(seat.to_string(), sent);
    }
    Ok(json!({
        "project": project,
        "commit": receipts[0].commit,
        "holdings": receipts,
        "routes": routes,
    }))
}

/// `directory release --batch <file> --why <text>` (plan 215, F71): one
/// `<role>[/<instance>] <seat>` per line, blank and `#` lines skipped; every
/// holding ended in one writer commit, all or nothing, naming every bad line.
/// Each live seat's window then follows its remaining holdings, as a single
/// release (plan 212, F57).
async fn release_batch(
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
    let mut targets = Vec::new();
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
        match role_and_instance(target) {
            Ok((role, instance)) => targets.push(ReleaseTarget {
                role: role.to_string(),
                instance: instance.to_string(),
                seat: (*seat).to_string(),
                from: None,
            }),
            Err(refusal) => problems.push(format!("line {number}: {}", refusal.message)),
        }
    }
    if !problems.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("the batch wrote nothing: {}", problems.join("; ")),
        ));
    }
    if targets.is_empty() {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("{} names no holdings", path.display()),
        ));
    }
    let released = state
        .services
        .directory
        .release_many(&project, repo, targets, actor, &why, Unmatched::Refuse)
        .await?;
    let mut receipts = released.receipts;
    windows_after_release(state, &mut receipts).await;
    Ok(json!({
        "project": project,
        "commit": receipts[0].commit,
        "holdings": receipts,
    }))
}

/// After a batch of releases, each live seat's window follows its remaining
/// holdings (plan 212, F57): one check per seat, against its earliest ended
/// holding (the only one its window can be named for), recorded on that
/// holding's receipt. At most one tmux launch per seat, at the assign's site.
pub(crate) async fn windows_after_release(state: &AppState, receipts: &mut [HoldingReceipt]) {
    let mut seats: Vec<String> = receipts
        .iter()
        .filter_map(|receipt| receipt.row["seat"].as_str().map(str::to_string))
        .collect();
    seats.sort();
    seats.dedup();
    let from = |receipt: &HoldingReceipt| {
        receipt.row["from"]
            .as_str()
            .and_then(crate::community::holders::parse_time)
    };
    for seat in seats {
        let Ok(Some(descriptor)) = state
            .services
            .registry
            .get(&SeatId::from(seat.as_str()))
            .await
        else {
            continue;
        };
        if descriptor.tombstoned_at.is_some() {
            continue;
        }
        let Some(earliest) = receipts
            .iter_mut()
            .filter(|receipt| receipt.row["seat"] == seat.as_str())
            .min_by_key(|receipt| from(receipt))
        else {
            continue;
        };
        earliest.window = Some(
            state
                .services
                .directory
                .window_after_release(&descriptor, &earliest.row)
                .await,
        );
    }
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
                "--body-file - reads stdin, which the daemon cannot; `pij` and `pij-rs` send it as --body, or pass --body or a file path",
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
        slug: call.get("slug"),
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
pub(super) fn match_project(
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

    /// Every command the picture prints (plan 203), built from hostile repo
    /// and store text, is split by a real `/bin/sh` and replayed through the
    /// parser its verb reaches: nothing in it runs, the shell hands the verb
    /// exactly the words the picture meant, and the verb accepts them. A role
    /// it creates carries every section `pij/role` requires (#141 review and
    /// security S001).
    #[test]
    fn every_printed_command_runs_as_written_through_the_real_parsers() {
        use std::collections::HashSet;

        use crate::community::picture::{self, RoleActivity};
        use crate::community::templates::{self, Catalog};

        let hostile = "s09$(touch dollar)`touch tick`;touch semi \"dq\" 'sq'\nnext";
        let seat = "pij-a;touch seat";
        let project = "fleet";
        let holding = |role: &str, instance: &str, seat: &str, from: &str, to: Option<&str>| {
            pij_store::Assignment {
                project: project.into(),
                role: role.into(),
                instance: instance.into(),
                seat: seat.into(),
                harness: None,
                session: None,
                machine: None,
                from: from.into(),
                to: to.map(Into::into),
                assigned_by: "prime".into(),
                released_by: None,
                why: "w".into(),
                release_why: to.map(|_| "close".to_string()),
                end_pending: false,
            }
        };
        let closed = [
            holding(
                "reviewer",
                "",
                "pij-old",
                "2026-11-01T00:00:00Z",
                Some("2026-11-02T00:00:00Z"),
            ),
            holding(
                "reviewer",
                "",
                "pij-old",
                "2026-11-03T00:00:00Z",
                Some("2026-11-04T00:00:00Z"),
            ),
        ];
        let open = [
            holding("reviewer", "115", seat, "2026-11-05T00:00:00Z", None),
            holding("reviewer", "", "pij-b", "2026-11-06T00:00:00Z", None),
        ];
        let rows: Vec<&pij_store::Assignment> = closed.iter().chain(&open).collect();
        let seats: HashSet<String> = [seat.to_string(), "pij-b".to_string()].into();
        let mut printed: Vec<String> = Vec::new();
        let mut take = |actions: Vec<Value>| {
            for action in actions {
                for command in action["commands"].as_array().unwrap() {
                    printed.push(command.as_str().unwrap().to_string());
                }
            }
        };
        take(picture::actions(
            project,
            "reviewer",
            "one",
            &rows,
            &HashSet::new(),
            &["", "115"],
            None,
        ));
        take(picture::actions(
            project,
            "reviewer",
            "one",
            &rows,
            &seats,
            &[],
            Some("2026-11-05"),
        ));
        let mut prompts = templates::recommendations(project, &[]);
        let needs = vec![
            ("qa".to_string(), vec![format!("001 {hostile}")]),
            (format!("docs{hostile}"), vec![format!("001 {hostile}")]),
        ];
        picture::fold_needs(project, &mut prompts, &needs, &[], &Catalog::shipped());
        prompts.extend(picture::unowned(
            project,
            &[(
                format!("crates/{hostile}"),
                vec!["a".into(), "b".into(), "c".into()],
            )],
            &[],
        ));
        let mut two = open.to_vec();
        two.push(holding("pm", "", seat, "2026-11-01T00:00:00Z", None));
        prompts.extend(picture::two_domains(project, &two, &seats));
        for prompt in &prompts {
            printed.extend(prompt.commands.iter().cloned());
        }
        printed.extend(picture::call_on(project, hostile, false, 1));
        let idle = picture::idle(
            project,
            &RoleActivity {
                role: hostile,
                created: Some("2026-01-01"),
                rows: &[],
                live_holder: false,
                waiting: 0,
                has_output: false,
                notes: 0,
            },
            "2026-10-01",
        )
        .expect("idle");
        assert!(idle.later.is_empty(), "retire and merge ship with s15");
        let kinds: HashSet<&str> = prompts.iter().map(|prompt| prompt.kind).collect();
        for kind in ["core", "needed", "recurring", "two-domains"] {
            assert!(kinds.contains(kind), "the fixture prints a {kind} prompt");
        }

        let all: Vec<&String> = printed.iter().chain(&idle.commands).collect();
        let dir = pij_testkit::fresh_dir("printed-commands");
        let script: String = all
            .iter()
            .map(|command| format!("printf '%s\\037' {command}; printf '\\036'\n"))
            .collect();
        let shell = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .current_dir(&dir)
            .output()
            .expect("run /bin/sh");
        assert!(shell.status.success(), "{shell:?}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing in a printed command ran, and nothing was redirected"
        );
        let stdout = String::from_utf8(shell.stdout).unwrap();
        let split: Vec<Vec<String>> = stdout
            .split('\x1e')
            .filter(|record| !record.is_empty())
            .map(|record| {
                record
                    .split('\x1f')
                    .filter(|word| !word.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .collect();
        assert_eq!(split.len(), all.len());

        let schema: Value = serde_json::from_slice(include_bytes!(
            "../../../../.dd/schemas/pij/role/schema.json"
        ))
        .unwrap();
        for (command, words) in all.iter().zip(&split) {
            let words_ref: Vec<&str> = words.iter().map(String::as_str).collect();
            assert_eq!(
                picture::command(&words_ref),
                **command,
                "the shell split {command} into exactly its words"
            );
            match words_ref.as_slice() {
                // Native clap; the CLI's own test replays this shape.
                ["pij-rs", "revive", _] => {}
                ["pij", "send", ..] => {
                    crate::http::shim::parse_send(&words[1..], Some("text"))
                        .unwrap_or_else(|error| panic!("{command}: {error}"));
                }
                ["pij", "directory", ..] => {
                    let call = Arguments::parse(&words[1..])
                        .unwrap_or_else(|refusal| panic!("{command}: {refusal:?}"));
                    if call.leaf != "add" || call.get("from").is_some() {
                        continue;
                    }
                    let document = role::from_fields(&call.positions[0], &call.fields())
                        .unwrap_or_else(|refusal| panic!("{command}: {refusal:?}"));
                    for (name, rule) in schema["sections"].as_object().unwrap() {
                        if rule["required"] != true || name == "changelog" {
                            continue;
                        }
                        let section = document["sections"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|section| section["name"] == *name)
                            .unwrap_or_else(|| panic!("{command}: pij/role requires {name}"));
                        for field in rule["shape"]["required"].as_array().into_iter().flatten() {
                            assert!(
                                !section["value"][field.as_str().unwrap()].is_null(),
                                "{command}: pij/role requires {name}.{field}"
                            );
                        }
                    }
                }
                _ => panic!("{command}: a verb with no replay"),
            }
        }
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
        assert_eq!(
            Arguments::parse(&argv(&["directory", "templates", "--show", "pm@v1"]))
                .unwrap()
                .get("show")
                .as_deref(),
            Some("pm@v1")
        );
        for refused in [
            &["directory", "template-diff", "pm", "--show", "pm"][..],
            &["directory", "add", "pm", "--show", "pm", "--why", "w"][..],
            &["directory", "--show", "pm"][..],
        ] {
            assert_eq!(
                Arguments::parse(&argv(refused)).unwrap_err().code,
                "E-RS-ARG",
                "{refused:?}"
            );
        }
    }
}
