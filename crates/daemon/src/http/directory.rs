//! `pij directory add` (plan 190, roadmap 001 s02): write a role into the
//! community directory through the daemon's single writer.
//!
//! The argv/caller wire is governance's ([`GovernanceRequest`]). Flags may
//! repeat (`--rule`, `--scope-path`, …), which governance's parser refuses,
//! so this family parses its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::model::{Envelope, ErrorKind};
use serde_json::{Value, json};

use super::governance::GovernanceRequest;
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope};
use crate::community::layout::{RolePaths, exists};
use crate::community::role::{self, RoleFields};
use crate::community::writer::{Existing, NewFile, Written};
use crate::community::{Refusal, pointer};

const COMMAND: &str = "pij directory";
const SINGLE: [&str; 19] = [
    "why",
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
];
const REPEATED: [&str; 6] = [
    "scope-path",
    "rule",
    "out-of-scope",
    "forbidden",
    "input",
    "output-kind",
];
/// Flags that steer the write rather than describe a role.
const CONTROL: [&str; 5] = ["why", "project", "file", "batch", "actor"];

#[derive(Debug, Default)]
struct Arguments {
    positions: Vec<String>,
    single: BTreeMap<String, String>,
    repeated: BTreeMap<String, Vec<String>>,
}

impl Arguments {
    fn parse(argv: &[String]) -> Result<Self, Refusal> {
        match argv {
            [family, leaf, ..] if family == "directory" && leaf == "add" => {}
            [family, leaf, ..] if family == "directory" => {
                return Err(Refusal::new(
                    "E-RS-UNPORTED",
                    format!("directory {leaf}: unknown command; s02 ships `directory add`"),
                ));
            }
            _ => {
                return Err(Refusal::new(
                    "E-RS-ARG",
                    "usage: pij directory add <id> --why <text> …",
                ));
            }
        }
        let mut call = Self::default();
        let mut tokens = argv[2..].iter();
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
        Ok(call)
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
    match add(&state, actor.id.as_str(), &request.caller, &call).await {
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
    let date = role::today();
    let batch = call.get("batch");
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
        (None, None, [id]) => vec![(id.clone(), role::from_fields(id, &call.fields())?)],
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
        let taken = exists(&id, &paths.document);
        files.push(NewFile {
            path: paths.document.clone(),
            bytes: role::to_bytes(&document),
            document: true,
            existing: Existing::Refuse(taken.clone()),
        });
        files.push(NewFile {
            path: paths.memory_index,
            bytes: RolePaths::empty_memory_index(&id).into_bytes(),
            document: false,
            existing: Existing::Refuse(taken.clone()),
        });
        files.push(NewFile {
            path: paths.outputs_keep,
            bytes: Vec::new(),
            document: false,
            existing: Existing::Refuse(taken),
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
    let writer = Arc::clone(&state.services.community);
    let written: Written =
        tokio::task::spawn_blocking(move || writer.write(&repo, files, &message))
            .await
            .map_err(|error| {
                Refusal::new(
                    crate::community::E_GOV_GIT,
                    format!("the writer stopped: {error}"),
                )
            })??;

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

/// The project the write belongs to and the repository to find governance from:
/// `--project`, else the project whose repo holds the caller's directory.
async fn project_and_repo(
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
        None => cwd
            .and_then(|cwd| {
                let cwd = canonical(Path::new(cwd));
                projects
                    .into_iter()
                    .filter_map(|project| {
                        let repo = canonical(Path::new(project.repo.as_deref()?));
                        cwd.starts_with(&repo)
                            .then_some((repo.as_os_str().len(), project))
                    })
                    .max_by_key(|(depth, _)| *depth)
                    .map(|(_, project)| project)
            })
            .ok_or_else(|| {
                Refusal::new(
                    "E-RS-ARG",
                    "no project's repo holds this directory; name it with --project <slug>",
                )
            })?,
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

/// `/tmp` and `/private/tmp` are one directory on macOS: compare real paths.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn caller_path(caller: &CallerContext, path: &str) -> Result<PathBuf, Refusal> {
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

fn refused(refusal: &Refusal) -> Response {
    let status = match refusal.code {
        "E-RS-ARG" | "E-RS-UNPORTED" => StatusCode::BAD_REQUEST,
        "E-RS-OWNERSHIP" => StatusCode::FORBIDDEN,
        "E-RS-NOT-FOUND" => StatusCode::NOT_FOUND,
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
            Arguments::parse(&argv(&["directory", "assign", "qa"]))
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
}
