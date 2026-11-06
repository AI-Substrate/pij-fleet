//! Holding roles (plan 195): assign, release and the ends a seat close records.
//!
//! The governance branch is the truth: each change lands as a holders
//! document commit through the single writer, then the store indexes it
//! (migration 0032). Every write for a project also carries that project's
//! pending close-ends, so an end a refused close left behind reaches the branch
//! with the next write.

use std::path::PathBuf;
use std::sync::Arc;

use pij_core::model::{SeatDescriptor, SeatId};
use pij_core::ports::TmuxPort;
use pij_store::{Assignment, SqliteOrchestration};
use serde::Serialize;
use serde_json::Value;

use super::holders::{self, seat_role_for};
use super::layout::RolePaths;
use super::root::GovernanceRoot;
use super::templates::{self, Catalog};
use super::writer::{CommunityWriter, Existing, NewFile, Written};
use super::{E_GOV_GIT, Refusal, role};
use crate::events::EventBus;

/// The role named does not exist in the project's directory.
pub const E_ROLE_MISSING: &str = "E-RS-DIRECTORY-ROLE-MISSING";
/// The seat already holds that role (and instance).
pub const E_HELD: &str = "E-RS-DIRECTORY-HELD";
/// The seat does not hold that role (and instance).
pub const E_NOT_HELD: &str = "E-RS-DIRECTORY-NOT-HELD";

/// Called once after each assign lands (ruling C6); s05 drains the role queue
/// through it.
pub trait AfterAssign: Send + Sync {
    /// `instance` is empty for a one-holder role.
    fn assigned(&self, project: &str, role: &str, instance: &str, seat: &SeatId);
}

/// The default: nothing happens after an assign.
pub struct NoHook;

impl AfterAssign for NoHook {
    fn assigned(&self, _: &str, _: &str, _: &str, _: &SeatId) {}
}

/// One assign.
#[derive(Clone, Debug)]
pub struct AssignRequest {
    /// The project slug.
    pub project: String,
    /// The repository the project's governance worktree belongs to.
    pub repo: PathBuf,
    /// The role id.
    pub role: String,
    /// The instance key, empty for a one-holder role.
    pub instance: String,
    /// The live seat taking the role.
    pub seat: SeatDescriptor,
    /// The calling seat.
    pub actor: SeatId,
    /// Why; becomes the row's `why`.
    pub why: String,
    /// This machine's alias.
    pub machine: String,
}

/// What an assign or release changed.
#[derive(Clone, Debug, Serialize)]
pub struct HoldingReceipt {
    /// The governance commit.
    pub commit: String,
    /// The holders document, worktree-relative.
    pub path: String,
    /// The row as written.
    pub row: Value,
    /// A named warning (a second live holder of a one-holder role).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The seat's role after the assign (ruling C1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat_role: Option<String>,
    /// Pending close-ends this write carried to the branch.
    pub flushed: usize,
    /// The holder's tmux window rename on assign (C16): `{name, renamed}`,
    /// with `reason` when it did not happen. A failed rename never fails the
    /// assign.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<Value>,
}

/// What a close did to the closed seat's holdings, per project.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CloseEnds {
    /// Projects whose ends were committed, with the commit.
    pub committed: Vec<Value>,
    /// Projects whose ends are pending, with the refusal that held them back.
    pub pending: Vec<Value>,
}

/// The daemon's role-holding service.
pub struct DirectoryService {
    writer: Arc<CommunityWriter>,
    store: SqliteOrchestration,
    event_bus: Arc<EventBus>,
    hook: Arc<dyn AfterAssign>,
    tmux: Arc<dyn TmuxPort>,
    templates: Catalog,
}

impl DirectoryService {
    /// Compose from the single writer, the spine-pool store, the bus, the
    /// after-assign hook (ruling C6; [`NoHook`] by default) and tmux, for the
    /// window rename on assign.
    pub fn new(
        writer: Arc<CommunityWriter>,
        store: SqliteOrchestration,
        event_bus: Arc<EventBus>,
        hook: Arc<dyn AfterAssign>,
        tmux: Arc<dyn TmuxPort>,
    ) -> Self {
        Self {
            writer,
            store,
            event_bus,
            hook,
            tmux,
            templates: Catalog::shipped(),
        }
    }

    /// The same service with another template catalog (tests use fixtures;
    /// the daemon keeps the shipped one).
    #[must_use]
    pub fn with_templates(mut self, templates: Catalog) -> Self {
        self.templates = templates;
        self
    }

    /// The same service with no after-assign hook, for a hook that needs the
    /// services itself: holding this copy instead of the real directory keeps
    /// the services free of a reference cycle, so a stopped daemon's services
    /// drop.
    #[must_use]
    pub fn without_hook(&self) -> Self {
        Self {
            writer: Arc::clone(&self.writer),
            store: self.store.clone(),
            event_bus: Arc::clone(&self.event_bus),
            hook: Arc::new(NoHook),
            tmux: Arc::clone(&self.tmux),
            templates: self.templates.clone(),
        }
    }

    /// The role templates this service copies from and compares against.
    pub fn templates(&self) -> &Catalog {
        &self.templates
    }

    /// The store, for the view and the rebuild.
    pub fn store(&self) -> &SqliteOrchestration {
        &self.store
    }

    /// `pij directory` (AC7): this project's roles, read in-process from the
    /// governance worktree, and their holders from the store. Launches
    /// nothing. Holders come from the store so a rebuild is a real round trip.
    /// Plan 194: each role's template drift, and prompts for the missing core
    /// roles and security (ruling C22), which create nothing.
    ///
    /// # Errors
    /// [`super::E_GOV_ABSENT`] without a governance worktree; `E-RS-STORE`.
    pub async fn view(&self, project: &str, repo: &std::path::Path) -> Result<Value, Refusal> {
        let root = super::root::find(repo)?;
        let rows = self
            .store
            .assignments(project)
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?;
        let roles_dir = root.worktree.join(super::layout::COMMUNITY).join("roles");
        let mut ids: Vec<String> = std::fs::read_dir(&roles_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().join("role.dd.json").is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        ids.sort();
        let mut roles = Vec::new();
        let mut made_from = Vec::new();
        for id in &ids {
            let document: Value = std::fs::read(roles_dir.join(id).join("role.dd.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(Value::Null);
            let meta = document["sections"]
                .as_array()
                .and_then(|sections| sections.iter().find(|section| section["name"] == "meta"))
                .map_or(Value::Null, |meta| meta["value"].clone());
            let mine: Vec<&Assignment> = rows.iter().filter(|row| &row.role == id).collect();
            let live: Vec<Value> = mine
                .iter()
                .filter(|row| row.to.is_none() || row.end_pending)
                .map(|row| {
                    serde_json::json!({
                        "seat": row.seat, "instance": row.instance, "since": row.from,
                        "machine": row.machine, "end_pending": row.end_pending,
                    })
                })
                .collect();
            let owned: Vec<Assignment> = mine.into_iter().cloned().collect();
            made_from.push((id.clone(), meta["template"].as_str().map(str::to_string)));
            roles.push(serde_json::json!({
                "id": id,
                "title": meta["title"],
                "kind": meta["kind"],
                "holders": meta["holders"],
                "template": self.templates.drift(&meta),
                "live": live,
                "lineage": holders::to_document(id, &owned)["sections"][1]["value"],
            }));
        }
        let prompts = templates::prompts(project, &made_from);
        let pending: Vec<Value> = rows
            .iter()
            .filter(|row| row.end_pending)
            .map(|row| serde_json::json!({"role": row.role, "seat": row.seat, "instance": row.instance, "to": row.to}))
            .collect();
        let mut view = serde_json::json!({"project": project, "roles": roles, "pending": pending});
        if !prompts.is_empty() {
            view["prompts"] = serde_json::json!(prompts);
            view["guidance"] = serde_json::json!(templates::GUIDANCE);
        }
        Ok(view)
    }

    /// `pij directory template-diff <id>` (plan 194, AC4): what changed
    /// upstream between the template version role `id` came from and the
    /// latest, computed in-process. Reads the role from the governance
    /// worktree; writes and launches nothing.
    ///
    /// # Errors
    /// [`super::E_GOV_ABSENT`]; [`E_ROLE_MISSING`]; `E-RS-ARG` for a template
    /// or version this build does not ship.
    pub fn template_diff(&self, repo: &std::path::Path, id: &str) -> Result<Value, Refusal> {
        let paths = RolePaths::new(id)?;
        let root = super::root::find(repo)?;
        let document: Value = std::fs::read(root.worktree.join(&paths.document))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| {
                Refusal::new(
                    E_ROLE_MISSING,
                    format!("role {id:?} has no readable {}", paths.document.display()),
                )
            })?;
        let meta = document["sections"]
            .as_array()
            .and_then(|sections| sections.iter().find(|section| section["name"] == "meta"))
            .map_or(Value::Null, |meta| meta["value"].clone());
        let Some(name) = meta["template"].as_str() else {
            return Ok(
                serde_json::json!({"id": id, "template": null, "behind": false, "diff": ""}),
            );
        };
        let version = meta["template_version"].as_str().unwrap_or_default();
        let from = templates::parse_version(version).ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                format!("role {id:?} records template_version {version:?}, not a version number"),
            )
        })?;
        let (latest, diff) = self.templates.diff_to_latest(name, from)?;
        Ok(serde_json::json!({
            "id": id,
            "template": name,
            "version": from,
            "latest": latest,
            "behind": latest > from,
            "diff": diff,
        }))
    }

    /// Rebuild this project's pointer and holder rows from the governance
    /// branch head (AC8). The branch is the truth: pending close-ends that never
    /// reached it are dropped. One git launch per holders document, plus two.
    ///
    /// # Errors
    /// [`super::E_GOV_ABSENT`], `E-RS-GOV-GIT`, `E-RS-DIRECTORY-ROW` when a
    /// holders document records one holding twice (nothing is replaced), or
    /// `E-RS-STORE`.
    pub async fn rebuild(&self, project: &str, repo: PathBuf) -> Result<Value, Refusal> {
        let project_id = project.to_string();
        let (pointers, rows) = tokio::task::spawn_blocking(move || {
            let root = super::root::resolve(&repo)?;
            let pointers = super::pointer::from_branch_head(&root, &project_id)?;
            let mut rows = Vec::new();
            for pointer in pointers
                .iter()
                .filter(|p| p.path.ends_with("/holders.dd.json"))
            {
                let text = super::pointer::read_at_head(&root, &pointer.path)?;
                let document: Value = serde_json::from_str(&text).map_err(|error| {
                    Refusal::new(E_GOV_GIT, format!("{} is not JSON: {error}", pointer.path))
                })?;
                let read = holders::from_document(&project_id, &document)?;
                if let Some(first) = read.first() {
                    holders::check_rows(&first.role, &read)?;
                }
                rows.extend(read);
            }
            Ok::<_, Refusal>((pointers, rows))
        })
        .await
        .map_err(|error| Refusal::new(E_GOV_GIT, format!("the rebuild stopped: {error}")))??;
        let store_error = |error: pij_core::error::PijError| match error {
            pij_core::error::PijError::GovernanceRefused { record, .. } => Refusal::new(
                "E-RS-DIRECTORY-ROW",
                format!("the branch records one holding twice ({record})"),
            ),
            error => Refusal::new("E-RS-STORE", error.to_string()),
        };
        self.store
            .replace_community_pointers(project, &pointers)
            .await
            .map_err(store_error)?;
        self.store
            .replace_assignments(project, &rows)
            .await
            .map_err(store_error)?;
        Ok(
            serde_json::json!({"project": project, "pointers": pointers.len(), "holdings": rows.len()}),
        )
    }

    /// `pij directory external add` (AC9, req-16): one dated entry in
    /// `community/external.dd.json`, created on first use. Loans are not
    /// otherwise tracked.
    ///
    /// # Errors
    /// Any writer refusal; nothing is written.
    #[allow(clippy::too_many_arguments)]
    pub async fn external_add(
        &self,
        project: &str,
        repo: PathBuf,
        seat: &str,
        from_project: &str,
        role: Option<&str>,
        note: &str,
        actor: &SeatId,
    ) -> Result<(Written, Value), Refusal> {
        let mut entry = serde_json::json!({
            "date": holders::now(), "seat": seat, "from_project": from_project,
            "note": note, "by": actor.to_string(),
        });
        if let Some(role) = role {
            entry["role"] = serde_json::json!(role);
        }
        let pending = self.pending(project).await?;
        let message = format!(
            "community: external {seat} from {from_project}\n\n{note}\n\nPij-Seat: {actor}\nPij-Project: {project}"
        );
        let (project_id, flush, added) = (project.to_string(), pending, entry.clone());
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let path = std::path::Path::new(super::layout::EXTERNAL);
                let mut document = match std::fs::read(root.worktree.join(path)) {
                    Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                        Refusal::new(
                            E_GOV_GIT,
                            format!("{} is not JSON: {error}", path.display()),
                        )
                    })?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        serde_json::json!({
                            "dd": {"schema": "pij/external"},
                            "sections": [
                                {"name": "meta", "value": {"project": project_id}},
                                {"name": "entries", "value": []},
                            ],
                            "references": [],
                        })
                    }
                    Err(error) => {
                        return Err(Refusal::new(
                            E_GOV_GIT,
                            format!("cannot read {}: {error}", path.display()),
                        ));
                    }
                };
                let entries = document["sections"]
                    .as_array_mut()
                    .and_then(|sections| {
                        sections
                            .iter_mut()
                            .find(|section| section["name"] == "entries")
                    })
                    .and_then(|section| section["value"].as_array_mut())
                    .ok_or_else(|| {
                        Refusal::new(E_GOV_GIT, "external.dd.json has no entries section")
                    })?;
                entries.push(added);
                let mut files = vec![NewFile {
                    path: path.to_path_buf(),
                    bytes: role::to_bytes(&document),
                    document: true,
                    existing: Existing::Replace,
                }];
                files.extend(flush_files(
                    root,
                    &project_id,
                    &flush,
                    &[],
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        self.flushed(&built.landed, &written).await?;
        Ok((written, entry))
    }

    /// `pij output` (plan 200, AC1): one pij/output document in one commit,
    /// its path chosen under the writer lock so two outputs at once never share
    /// a file. Returns the commit and the document's path.
    ///
    /// # Errors
    /// Any writer or [`super::output`] refusal; nothing is written.
    pub async fn output_add(
        &self,
        project: &str,
        repo: PathBuf,
        fields: super::output::OutputFields,
        writer: &SeatId,
        holds_role: bool,
    ) -> Result<(Written, String), Refusal> {
        let date = role::today();
        let pending = self.pending(project).await?;
        let message = format!(
            "community: {} {} on {}\n\nPij-Seat: {writer}\nPij-Project: {project}",
            fields.role, fields.kind, fields.subject
        );
        let (project_id, by) = (project.to_string(), writer.to_string());
        let chosen = Arc::new(std::sync::Mutex::new(String::new()));
        let path_out = Arc::clone(&chosen);
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let (mut files, path) =
                    super::output::files(&root.worktree, &fields, &by, holds_role, &date)?;
                *path_out
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    path.display().to_string();
                files.extend(flush_files(
                    root,
                    &project_id,
                    &pending,
                    &[],
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        self.flushed(&built.landed, &written).await?;
        let path = chosen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Ok((written, path))
    }

    /// Any other directory write (`directory add`): `files` plus the project's
    /// pending close-ends, in one commit.
    ///
    /// # Errors
    /// Any writer refusal; nothing is written.
    pub async fn write(
        &self,
        project: &str,
        repo: PathBuf,
        files: Vec<NewFile>,
        message: String,
    ) -> Result<Written, Refusal> {
        let pending = self.pending(project).await?;
        let (project_id, flush) = (project.to_string(), pending);
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let mut files = files;
                files.extend(flush_files(
                    root,
                    &project_id,
                    &flush,
                    &[],
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        self.flushed(&built.landed, &written).await?;
        Ok(written)
    }

    /// Bind a live seat to an existing role: one holders commit, then the store
    /// row and the seat's role in one transaction (C1), then the hook (C6).
    ///
    /// # Errors
    /// A named refusal; nothing is written.
    pub async fn assign(&self, request: AssignRequest) -> Result<HoldingReceipt, Refusal> {
        if request.seat.tombstoned_at.is_some() {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("seat {} is closed; assign a live seat", request.seat.id),
            ));
        }
        let pending = self.pending(&request.project).await?;
        let draft = Assignment {
            project: request.project.clone(),
            role: request.role.clone(),
            instance: request.instance.clone(),
            seat: request.seat.id.to_string(),
            harness: Some(request.seat.harness.to_string()),
            session: request.seat.harness_session.clone(),
            machine: Some(request.machine.clone()),
            from: String::new(),
            to: None,
            assigned_by: request.actor.to_string(),
            released_by: None,
            why: request.why.clone(),
            release_why: None,
            end_pending: false,
        };
        let message = format!(
            "community: assign {}{} to {}\n\n{}\n\nPij-Seat: {}\nPij-Project: {}",
            request.role,
            instance_suffix(&request.instance),
            request.seat.id,
            request.why,
            request.actor,
            request.project
        );
        let flush = pending;
        let (written, built) = self
            .commit(request.repo.clone(), message, move |root, built| {
                let mut new = draft;
                let mut rows = read_holders(root, &new.project, &new.role)?;
                built.landed = holders::apply_ends(&mut rows, &flush);
                if rows
                    .iter()
                    .any(|held| open_for(held, &new.seat, &new.instance))
                {
                    return Err(Refusal::new(
                        E_HELD,
                        format!(
                            "{} already holds {}{}",
                            new.seat,
                            new.role,
                            instance_suffix(&new.instance)
                        ),
                    ));
                }
                if holders_mode(root, &new.role)? == "one"
                    && let Some(other) = rows.iter().find(|held| held.to.is_none())
                {
                    built.warning = Some(format!(
                        "W-ONE-HOLDER: {} is a one-holder role and {} still holds it",
                        new.role, other.seat
                    ));
                }
                new.from = holders::fresh_from(&rows, &new.seat, &new.instance);
                rows.push(new.clone());
                let mut files = vec![holders_file(&new.role, &rows)?];
                files.extend(flush_files(
                    root,
                    &new.project,
                    &flush,
                    &[new.role.as_str()],
                    &mut built.landed,
                )?);
                built.row = Some(new);
                Ok(files)
            })
            .await?;
        let row = built.row.clone().expect("a landed assign wrote its row");
        let store = self.store.clone();
        let indexed = row.clone();
        let seat_role = seat_role_for(&row.role).to_string();
        let at = system_ms();
        let commit = self
            .event_bus
            .publish_committed_batch(async move {
                let commit = store
                    .record_assignment_committed(&indexed, Some(&seat_role), at)
                    .await?;
                Ok((commit.events.clone(), commit))
            })
            .await
            .map_err(|error| store_refusal(&written, &error))?;
        self.flushed(&built.landed, &written).await?;
        self.hook
            .assigned(&row.project, &row.role, &row.instance, &request.seat.id);
        let window = self.rename(&request.seat, &row).await;
        Ok(HoldingReceipt {
            commit: written.commit.clone(),
            path: holders_path(&row.role)?,
            row:
                holders::to_document(&row.role, std::slice::from_ref(&row))["sections"][1]["value"]
                    [0]
                .clone(),
            warning: built.warning,
            seat_role: commit.seat_role,
            flushed: built.landed.len(),
            window: Some(window),
        })
    }

    /// Name the holder's window `<project>:<role>`, or `<project>:<role>-<instance>`
    /// (req-26, C16). One tmux launch, through the adapter's recorded site;
    /// never the control client, whose allow-list stays reads only (S2).
    async fn rename(&self, seat: &SeatDescriptor, row: &Assignment) -> Value {
        let name = if row.instance.is_empty() {
            format!("{}:{}", row.project, row.role)
        } else {
            format!("{}:{}-{}", row.project, row.role, row.instance)
        };
        let Some(pane) = seat.pane.as_deref() else {
            return serde_json::json!({"name": name, "renamed": false, "reason": "the seat has no pane"});
        };
        match self.tmux.rename_window(pane, &name).await {
            Ok(()) => serde_json::json!({"name": name, "renamed": true}),
            Err(error) => {
                serde_json::json!({"name": name, "renamed": false, "reason": error.to_string()})
            }
        }
    }

    /// End a seat's holding of a role: `to`, `released_by` and `release_why`.
    ///
    /// # Errors
    /// [`E_NOT_HELD`] when the seat does not hold it, or any writer refusal.
    #[allow(clippy::too_many_arguments)]
    pub async fn release(
        &self,
        project: &str,
        repo: PathBuf,
        role: &str,
        instance: &str,
        seat: &SeatId,
        actor: &SeatId,
        why: &str,
    ) -> Result<HoldingReceipt, Refusal> {
        let pending = self.pending(project).await?;
        let message = format!(
            "community: release {role}{} from {seat}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}",
            instance_suffix(instance)
        );
        let (project_id, role_id, instance_key) =
            (project.to_string(), role.to_string(), instance.to_string());
        let (seat_id, actor_id, release_why) =
            (seat.to_string(), actor.to_string(), why.to_string());
        let flush = pending;
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let mut rows = read_holders(root, &project_id, &role_id)?;
                built.landed = holders::apply_ends(&mut rows, &flush);
                let row = rows
                    .iter_mut()
                    .rev()
                    .find(|held| open_for(held, &seat_id, &instance_key))
                    .ok_or_else(|| {
                        Refusal::new(
                            E_NOT_HELD,
                            format!(
                                "{seat_id} does not hold {role_id}{}",
                                instance_suffix(&instance_key)
                            ),
                        )
                    })?;
                row.to = Some(holders::now());
                row.released_by = Some(actor_id.clone());
                row.release_why = Some(release_why.clone());
                built.row = Some(row.clone());
                let mut files = vec![holders_file(&role_id, &rows)?];
                files.extend(flush_files(
                    root,
                    &project_id,
                    &flush,
                    &[role_id.as_str()],
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        let row = built.row.clone().expect("a landed release found its row");
        self.store
            .end_assignments(std::slice::from_ref(&row))
            .await
            .map_err(|error| store_refusal(&written, &error))?;
        self.flushed(&built.landed, &written).await?;
        Ok(HoldingReceipt {
            commit: written.commit.clone(),
            path: holders_path(role)?,
            row: holders::to_document(role, std::slice::from_ref(&row))["sections"][1]["value"][0]
                .clone(),
            warning: None,
            seat_role: None,
            flushed: built.landed.len(),
            window: None,
        })
    }

    /// End every open holding of a closed seat (reason `close`). Never fails:
    /// the ends are stored as pending first (persist before mutate), each
    /// project's commit then clears them, and a refused project stays pending
    /// for its next directory write. `repo_of` maps a project to its repo.
    pub async fn end_for_close(
        &self,
        seat: &SeatId,
        actor: &SeatId,
        repo_of: impl Fn(&str) -> Option<PathBuf>,
    ) -> CloseEnds {
        let mut ends = CloseEnds::default();
        let open = match self.store.open_assignments(seat.as_str()).await {
            Ok(open) => open,
            Err(error) => {
                ends.pending
                    .push(serde_json::json!({"code": "E-RS-STORE", "message": error.to_string()}));
                return ends;
            }
        };
        let to = holders::now();
        let rows: Vec<Assignment> = open
            .into_iter()
            .map(|mut row| {
                row.to = Some(to.clone());
                row.released_by = Some(actor.to_string());
                row.release_why = Some("close".to_string());
                row.end_pending = true;
                row
            })
            .collect();
        if let Err(error) = self.store.end_assignments(&rows).await {
            ends.pending
                .push(serde_json::json!({"code": "E-RS-STORE", "message": error.to_string()}));
            return ends;
        }
        let mut projects: Vec<&str> = rows.iter().map(|row| row.project.as_str()).collect();
        projects.dedup();
        for project in projects {
            let mine: Vec<Assignment> = rows
                .iter()
                .filter(|row| row.project == project)
                .cloned()
                .collect();
            let roles: Vec<&str> = mine.iter().map(|row| row.role.as_str()).collect();
            let outcome = match repo_of(project) {
                None => Err(Refusal::new(
                    "E-RS-NOT-FOUND",
                    format!("project {project:?} records no repo"),
                )),
                Some(repo) => self.flush(project, repo, seat, actor).await,
            };
            match outcome {
                Ok(commit) => ends.committed.push(
                    serde_json::json!({"project": project, "roles": roles, "commit": commit}),
                ),
                Err(refusal) => ends.pending.push(serde_json::json!({
                    "project": project, "roles": roles,
                    "code": refusal.code, "message": refusal.message,
                })),
            }
        }
        ends
    }

    /// Commit a project's pending close-ends now.
    async fn flush(
        &self,
        project: &str,
        repo: PathBuf,
        seat: &SeatId,
        actor: &SeatId,
    ) -> Result<String, Refusal> {
        let pending = self.pending(project).await?;
        let message = format!(
            "community: end {seat}'s holdings (close)\n\nPij-Seat: {actor}\nPij-Project: {project}"
        );
        let (project_id, flush) = (project.to_string(), pending);
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                flush_files(root, &project_id, &flush, &[], &mut built.landed)
            })
            .await?;
        self.flushed(&built.landed, &written).await?;
        Ok(written.commit)
    }

    async fn pending(&self, project: &str) -> Result<Vec<Assignment>, Refusal> {
        Ok(self
            .store
            .assignments(project)
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
            .into_iter()
            .filter(|row| row.end_pending)
            .collect())
    }

    /// Index the ends a landed commit carried, clearing their `end_pending`.
    /// Only those: an end the commit did not carry stays pending.
    async fn flushed(&self, landed: &[Assignment], written: &Written) -> Result<(), Refusal> {
        if landed.is_empty() {
            return Ok(());
        }
        self.store
            .end_assignments(landed)
            .await
            .map_err(|error| store_refusal(written, &error))
    }

    /// Run a holders write on the blocking pool, under the writer lock.
    async fn commit(
        &self,
        repo: PathBuf,
        message: String,
        build: impl FnOnce(&GovernanceRoot, &mut Built) -> Result<Vec<NewFile>, Refusal>
        + Send
        + 'static,
    ) -> Result<(Written, Built), Refusal> {
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || {
            let mut built = Built::default();
            let written = writer.write_with(&repo, |root| build(root, &mut built), &message)?;
            Ok((written, built))
        })
        .await
        .map_err(|error| Refusal::new(E_GOV_GIT, format!("the writer stopped: {error}")))?
    }
}

fn instance_suffix(instance: &str) -> String {
    if instance.is_empty() {
        String::new()
    } else {
        format!("/{instance}")
    }
}

fn open_for(row: &Assignment, seat: &str, instance: &str) -> bool {
    row.to.is_none() && row.seat == seat && row.instance == instance
}

fn holders_path(role: &str) -> Result<String, Refusal> {
    Ok(RolePaths::new(role)?.holders.display().to_string())
}

/// The role's committed holders rows; none when it has no holders document.
fn read_holders(
    root: &GovernanceRoot,
    project: &str,
    role: &str,
) -> Result<Vec<Assignment>, Refusal> {
    let paths = RolePaths::new(role)?;
    if !root.worktree.join(&paths.document).is_file() {
        return Err(Refusal::new(
            E_ROLE_MISSING,
            format!("role {role:?} does not exist in this project's directory"),
        )
        .with_next(format!(
            "Add it first: pij directory add {role} --why <text> ..."
        )));
    }
    match std::fs::read(root.worktree.join(&paths.holders)) {
        Ok(bytes) => {
            let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
                Refusal::new(
                    E_GOV_GIT,
                    format!("{} is not JSON: {error}", paths.holders.display()),
                )
            })?;
            holders::from_document(project, &document)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(Refusal::new(
            E_GOV_GIT,
            format!("cannot read {}: {error}", paths.holders.display()),
        )),
    }
}

/// The role's `meta.holders` (`one` or `many`); `many` when unsaid.
fn holders_mode(root: &GovernanceRoot, role: &str) -> Result<String, Refusal> {
    let paths = RolePaths::new(role)?;
    let document: Value = std::fs::read(root.worktree.join(&paths.document))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    Ok(document["sections"]
        .as_array()
        .and_then(|sections| sections.iter().find(|section| section["name"] == "meta"))
        .and_then(|meta| meta["value"]["holders"].as_str())
        .unwrap_or("many")
        .to_string())
}

fn holders_file(role: &str, rows: &[Assignment]) -> Result<NewFile, Refusal> {
    holders::check_rows(role, rows)?;
    let paths = RolePaths::new(role)?;
    Ok(NewFile {
        path: paths.holders,
        bytes: role::to_bytes(&holders::to_document(role, rows)),
        document: true,
        existing: Existing::Replace,
    })
}

/// Holders files carrying `pending` ends, skipping roles the caller writes
/// (it applies their ends itself). Adds the ends these files carry to `landed`.
fn flush_files(
    root: &GovernanceRoot,
    project: &str,
    pending: &[Assignment],
    skip: &[&str],
    landed: &mut Vec<Assignment>,
) -> Result<Vec<NewFile>, Refusal> {
    let mut roles: Vec<&str> = pending.iter().map(|row| row.role.as_str()).collect();
    roles.sort_unstable();
    roles.dedup();
    let mut files = Vec::new();
    for role in roles.into_iter().filter(|role| !skip.contains(role)) {
        let mut rows = read_holders(root, project, role)?;
        landed.extend(holders::apply_ends(&mut rows, pending));
        files.push(holders_file(role, &rows)?);
    }
    Ok(files)
}

/// What a write's build reports back: a warning, the pending ends its files
/// carry, and the holding row it wrote.
#[derive(Default)]
struct Built {
    warning: Option<String>,
    landed: Vec<Assignment>,
    row: Option<Assignment>,
}

fn store_refusal(written: &Written, error: &pij_core::error::PijError) -> Refusal {
    match error {
        pij_core::error::PijError::GovernanceRefused { code, record } => Refusal::new(
            if code == E_HELD { E_HELD } else { "E-RS-ARG" },
            format!(
                "committed {} but the store refused {record} ({code}); rebuild the directory from the branch",
                written.commit
            ),
        ),
        error => Refusal::new(
            "E-RS-STORE",
            format!(
                "committed {} but could not index it: {error}",
                written.commit
            ),
        ),
    }
}

fn system_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
