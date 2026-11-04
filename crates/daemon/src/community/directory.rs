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
use pij_store::{Assignment, SqliteOrchestration};
use serde::Serialize;
use serde_json::Value;

use super::holders::{self, seat_role_for};
use super::layout::RolePaths;
use super::root::GovernanceRoot;
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
}

impl DirectoryService {
    /// Compose from the single writer, the spine-pool store, the bus and the
    /// after-assign hook (ruling C6; [`NoHook`] by default).
    pub fn new(
        writer: Arc<CommunityWriter>,
        store: SqliteOrchestration,
        event_bus: Arc<EventBus>,
        hook: Arc<dyn AfterAssign>,
    ) -> Self {
        Self {
            writer,
            store,
            event_bus,
            hook,
        }
    }

    /// The store, for the view and the rebuild.
    pub fn store(&self) -> &SqliteOrchestration {
        &self.store
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
        let (project_id, flush) = (project.to_string(), pending.clone());
        let (written, _) = self
            .commit(repo, message, move |root, _| {
                let mut files = files;
                files.extend(flush_files(root, &project_id, &flush, &[])?);
                Ok(files)
            })
            .await?;
        self.flushed(&pending, &written).await?;
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
        let row = Assignment {
            project: request.project.clone(),
            role: request.role.clone(),
            instance: request.instance.clone(),
            seat: request.seat.id.to_string(),
            harness: Some(request.seat.harness.to_string()),
            session: request.seat.harness_session.clone(),
            machine: Some(request.machine.clone()),
            from: holders::now(),
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
        let new = row.clone();
        let flush = pending.clone();
        let (written, warning) = self
            .commit(request.repo.clone(), message, move |root, warning| {
                let mut rows = read_holders(root, &new.project, &new.role)?;
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
                    *warning = Some(format!(
                        "W-ONE-HOLDER: {} is a one-holder role and {} still holds it",
                        new.role, other.seat
                    ));
                }
                rows.push(new.clone());
                let mut files = vec![holders_file(&new.role, &rows)?];
                files.extend(flush_files(
                    root,
                    &new.project,
                    &flush,
                    &[new.role.as_str()],
                )?);
                Ok(files)
            })
            .await?;
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
        self.flushed(&pending, &written).await?;
        self.hook
            .assigned(&row.project, &row.role, &row.instance, &request.seat.id);
        Ok(HoldingReceipt {
            commit: written.commit.clone(),
            path: holders_path(&row.role)?,
            row:
                holders::to_document(&row.role, std::slice::from_ref(&row))["sections"][1]["value"]
                    [0]
                .clone(),
            warning,
            seat_role: commit.seat_role,
            flushed: pending.len(),
        })
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
        let flush = pending.clone();
        let ended = Arc::new(std::sync::Mutex::new(None));
        let found = Arc::clone(&ended);
        let (written, _) = self
            .commit(repo, message, move |root, _| {
                let mut rows = read_holders(root, &project_id, &role_id)?;
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
                *found.lock().expect("unpoisoned") = Some(row.clone());
                let mut files = vec![holders_file(&role_id, &rows)?];
                files.extend(flush_files(root, &project_id, &flush, &[role_id.as_str()])?);
                Ok(files)
            })
            .await?;
        let row = ended
            .lock()
            .expect("unpoisoned")
            .take()
            .expect("a landed release found its row");
        self.store
            .end_assignments(std::slice::from_ref(&row))
            .await
            .map_err(|error| store_refusal(&written, &error))?;
        self.flushed(&pending, &written).await?;
        Ok(HoldingReceipt {
            commit: written.commit.clone(),
            path: holders_path(role)?,
            row: holders::to_document(role, std::slice::from_ref(&row))["sections"][1]["value"][0]
                .clone(),
            warning: None,
            seat_role: None,
            flushed: pending.len(),
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
        let (project_id, flush) = (project.to_string(), pending.clone());
        let (written, _) = self
            .commit(repo, message, move |root, _| {
                flush_files(root, &project_id, &flush, &[])
            })
            .await?;
        self.flushed(&pending, &written).await?;
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

    async fn flushed(&self, pending: &[Assignment], written: &Written) -> Result<(), Refusal> {
        if pending.is_empty() {
            return Ok(());
        }
        let cleared: Vec<Assignment> = pending
            .iter()
            .cloned()
            .map(|mut row| {
                row.end_pending = false;
                row
            })
            .collect();
        self.store
            .end_assignments(&cleared)
            .await
            .map_err(|error| store_refusal(written, &error))
    }

    /// Run a holders write on the blocking pool. `build` may set a warning.
    async fn commit(
        &self,
        repo: PathBuf,
        message: String,
        build: impl FnOnce(&GovernanceRoot, &mut Option<String>) -> Result<Vec<NewFile>, Refusal>
        + Send
        + 'static,
    ) -> Result<(Written, Option<String>), Refusal> {
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || {
            let mut warning = None;
            let written = writer.write_with(&repo, |root| build(root, &mut warning), &message)?;
            Ok((written, warning))
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
    let paths = RolePaths::new(role)?;
    Ok(NewFile {
        path: paths.holders,
        bytes: role::to_bytes(&holders::to_document(role, rows)),
        document: true,
        existing: Existing::Replace,
    })
}

/// Holders files carrying `pending` ends, skipping roles the caller writes.
fn flush_files(
    root: &GovernanceRoot,
    project: &str,
    pending: &[Assignment],
    skip: &[&str],
) -> Result<Vec<NewFile>, Refusal> {
    let mut roles: Vec<&str> = pending.iter().map(|row| row.role.as_str()).collect();
    roles.sort_unstable();
    roles.dedup();
    let mut files = Vec::new();
    for role in roles.into_iter().filter(|role| !skip.contains(role)) {
        let mut rows = read_holders(root, project, role)?;
        apply_ends(&mut rows, pending);
        files.push(holders_file(role, &rows)?);
    }
    Ok(files)
}

/// Close the open rows `ends` name (same role, seat, instance and `from`).
pub fn apply_ends(rows: &mut [Assignment], ends: &[Assignment]) {
    for end in ends {
        if let Some(row) = rows.iter_mut().find(|row| {
            row.to.is_none()
                && row.role == end.role
                && row.seat == end.seat
                && row.instance == end.instance
                && row.from == end.from
        }) {
            row.to.clone_from(&end.to);
            row.released_by.clone_from(&end.released_by);
            row.release_why.clone_from(&end.release_why);
        }
    }
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
