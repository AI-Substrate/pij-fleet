//! Holding roles (plan 195): assign, release and the ends a seat close records.
//!
//! The governance branch is the truth: each change lands as a holders
//! document commit through the single writer, then the store indexes it
//! (migration 0032). Every write for a project also carries that project's
//! pending close-ends, so an end a refused close left behind reaches the branch
//! with the next write.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use pij_core::directory::shell_word;
use pij_core::model::{SeatDescriptor, SeatId};
use pij_core::ports::TmuxPort;
use pij_store::{Assignment, SqliteOrchestration};
use serde::Serialize;
use serde_json::Value;

use super::holders::{self, seat_role_for};
use super::layout::RolePaths;
use super::picture;
use super::root::GovernanceRoot;
use super::routes::{self, Regenerated};
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
/// The role is retired, or merged into another (plan 206, req-28).
pub const E_RETIRED: &str = "E-RS-DIRECTORY-RETIRED";
/// The role still has a live holder (plan 206: release before retiring).
pub const E_ROLE_HELD: &str = "E-RS-DIRECTORY-ROLE-HELD";
/// The role has requests queued for a holder (plan 206: retiring would strand
/// them).
pub const E_ROLE_QUEUED: &str = "E-RS-DIRECTORY-ROLE-QUEUED";
/// A `--drop-queued` requester could not be told, so nothing was withdrawn
/// (security review of #135, F002); the same command retries.
pub const E_NOT_TOLD: &str = "E-RS-DIRECTORY-NOT-TOLD";
/// A role made from a template records no template version (plan 206, F19).
pub const E_NO_TEMPLATE_VERSION: &str = "E-RS-DIRECTORY-NO-TEMPLATE-VERSION";

/// A past holding overlaps another holding of the same seat and instance.
pub const E_OVERLAP: &str = "E-RS-DIRECTORY-OVERLAP";

/// Called once after each assign lands (ruling C6); s05 drains the role queue
/// through it, and the assign waits for it so its receipt can say what was
/// drained (plan 212, F56).
#[async_trait::async_trait]
pub trait AfterAssign: Send + Sync {
    /// `instance` is empty for a one-holder role. Returns one receipt entry per
    /// request the hook settled; empty when nothing waited.
    async fn assigned(
        &self,
        project: &str,
        role: &str,
        instance: &str,
        seat: &SeatId,
    ) -> Vec<Value>;
}

/// The default: nothing happens after an assign.
pub struct NoHook;

#[async_trait::async_trait]
impl AfterAssign for NoHook {
    async fn assigned(&self, _: &str, _: &str, _: &str, _: &SeatId) -> Vec<Value> {
        Vec::new()
    }
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

/// What `pij directory edit` changes (plan 206, F20).
#[derive(Clone, Debug)]
pub enum RoleEdit {
    /// Fields laid over the committed document, as `add --from` does.
    Fields(Box<role::RoleFields>),
    /// A whole replacement document; the committed changelog is kept.
    Document(Value),
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
    /// The role's queued requests the assign settled (plan 212, F56): each
    /// with its requester, reply mode and outcome. Absent when none waited.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drained: Vec<Value>,
}

/// The roles whose holdings are keyed by the PR they verdict (plan 215, F70).
pub const PR_ROLES: [&str; 3] = ["reviewer", "security", "performance"];

/// One holding to end: `<role>[/<instance>] <seat>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseTarget {
    /// The role id.
    pub role: String,
    /// The instance key, or empty.
    pub instance: String,
    /// The holding seat.
    pub seat: String,
    /// The holding's `from`, when the target names one exact holding: it
    /// then ends only if that holding is still the open one.
    pub from: Option<String>,
}

impl std::fmt::Display for ReleaseTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{} {}",
            self.role,
            instance_suffix(&self.instance),
            self.seat
        )
    }
}

/// What [`DirectoryService::release_many`] does with a target that is not an
/// open holding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmatched {
    /// Write nothing and name it (`release`, `release --batch`).
    Refuse,
    /// Leave it alone and name it (`pij delivered`).
    Skip,
}

/// What a release batch ended.
#[derive(Clone, Debug)]
pub struct Released {
    /// One receipt per ended holding, in target order.
    pub receipts: Vec<HoldingReceipt>,
    /// The targets left alone ([`Unmatched::Skip`]), each with why.
    pub skipped: Vec<String>,
}

/// What a close did to the closed seat's holdings, per project.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CloseEnds {
    /// Projects whose ends were committed, with the commit.
    pub committed: Vec<Value>,
    /// Projects whose ends are pending, with the refusal that held them back.
    pub pending: Vec<Value>,
}

/// How a retire or merge goes (plan 206).
#[derive(Clone, Copy, Debug)]
pub struct Retirement<'a> {
    /// The changelog row's reason.
    pub why: &'a str,
    /// Withdraw requests still queued for the role instead of refusing.
    pub drop_queued: bool,
}

/// A landed retire or merge. With `--drop-queued` it still holds the role's
/// admission gate and the requests to withdraw: the caller tells each
/// requester, then calls [`DirectoryService::withdraw`], so no send is
/// admitted until the withdrawal lands.
#[derive(Debug)]
pub struct Reshaped {
    /// The writer commit, or `None` when this call only finished the
    /// withdrawal an earlier retirement left undone (review of #135, F002).
    pub written: Option<Written>,
    /// The requests still to withdraw, oldest first; empty without
    /// `--drop-queued`.
    pub queued: Vec<pij_store::RoleRequest>,
    /// What the commit did to the routing table (plan 202): a retired role
    /// routes nothing. Empty when nothing was written.
    pub routes: Regenerated,
    /// The command that finishes the withdrawal if it fails.
    finish: String,
    gate: tokio::sync::OwnedMutexGuard<()>,
}

impl Reshaped {
    /// The command that finishes the withdrawal: the same verb with
    /// `--drop-queued`, its words quoted.
    pub fn finish(&self) -> &str {
        &self.finish
    }
}

/// The daemon's role-holding service.
pub struct DirectoryService {
    writer: Arc<CommunityWriter>,
    store: SqliteOrchestration,
    event_bus: Arc<EventBus>,
    hook: Arc<dyn AfterAssign>,
    tmux: Arc<dyn TmuxPort>,
    templates: Catalog,
    gates: Gates,
}

/// One async lock per (project, role): the admission boundary a send to the
/// role and a retire or merge of it share (review of #135, F001).
type Gates = Arc<std::sync::Mutex<HashMap<(String, String), Arc<tokio::sync::Mutex<()>>>>>;

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
            gates: Gates::default(),
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
            gates: Arc::clone(&self.gates),
        }
    }

    /// The admission boundary for `role` in `project` (review of #135, F001):
    /// a send to the role holds it from its standing read until its request
    /// is recorded, and a retire or merge holds it from its queued-request
    /// read through its commit and withdrawal. So no request is admitted
    /// behind a retirement, and none lands between the check and the commit.
    pub async fn admission(&self, project: &str, role: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self
                .gates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                gates
                    .entry((project.to_string(), role.to_string()))
                    .or_default(),
            )
        };
        gate.lock_owned().await
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
    /// Plan 203 (s09): the project's designated prime; per role its waiting
    /// requests, latest output and memory freshness, and whether each holder's
    /// seat is live (`live` is the registered, untombstoned seats); `actions`
    /// for the conditions that need the prime, workstream needs and recurring
    /// unowned paths folded into the prompts, the boundary `reviews`, each
    /// recommendation naming its C28 signal, and the standing PEC `notice`.
    /// F39: a suggestion nothing points at (a core or security role with no
    /// step need) is listed by name in `suggestions` and shown in full only
    /// with `suggest`; `prime_document` says whether the prime role's
    /// document exists, so a project without one reads as that document
    /// missing (F21). Reads per call: assignments, the project, the project's
    /// queued requests (without bodies) and the newest outputs, one store
    /// query each; each role's document and memory folder, the repo's
    /// workstreams (`picture::read_step_docs`, parsed once) and their
    /// blueprints, in-process.
    ///
    /// # Errors
    /// [`super::E_GOV_ABSENT`] without a governance worktree; `E-RS-STORE`.
    pub async fn view(
        &self,
        project: &str,
        repo: &std::path::Path,
        live_seats: &std::collections::HashSet<String>,
        suggest: bool,
    ) -> Result<Value, Refusal> {
        let root = super::root::find(repo)?;
        let store_error =
            |error: pij_core::error::PijError| Refusal::new("E-RS-STORE", error.to_string());
        let rows = self.store.assignments(project).await.map_err(store_error)?;
        let prime = self
            .store
            .project(project)
            .await
            .map_err(store_error)?
            .and_then(|record| record.prime_id)
            .map(|seat| seat.to_string());
        let queued: Vec<pij_store::RoleRequest> = self
            .store
            .queued_role_request_heads(project)
            .await
            .map_err(store_error)?;
        let outputs = self
            .store
            .latest_outputs(project)
            .await
            .map_err(store_error)?;
        let mut actions = Vec::new();
        let roles_dir = root.worktree.join(super::layout::COMMUNITY).join("roles");
        let mut ids: Vec<String> = std::fs::read_dir(&roles_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().join("role.dd.json").is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        ids.sort();
        let documents: Vec<Value> = ids
            .iter()
            .map(|id| {
                std::fs::read(roles_dir.join(id).join("role.dd.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or(Value::Null)
            })
            .collect();
        let standings: Vec<role::Standing> = documents.iter().map(role::standing).collect();
        let mut roles = Vec::new();
        let mut made_from = Vec::new();
        let mut scopes: Vec<(String, Vec<String>)> = Vec::new();
        let mut reviews = picture::two_domains(project, &rows, live_seats);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        let cutoff = picture::idle_cutoff(now_ms);
        for (index, (id, document)) in ids.iter().zip(&documents).enumerate() {
            let retired = standings[index].retired();
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
                        "seat_live": live_seats.contains(&row.seat),
                    })
                })
                .collect();
            let waiting = picture::waiting(id, &queued);
            let memory = super::memory::freshness(&root.worktree, id).unwrap_or(Value::Null);
            // A retired role routes, owns and needs nothing (plan 206).
            if !retired {
                let holders_mode = meta["holders"].as_str().unwrap_or("one");
                let live_holders = mine
                    .iter()
                    .filter(|row| row.to.is_none() && live_seats.contains(&row.seat))
                    .count();
                // Read only when two-live-holders fires (plan 215, F90).
                let open: HashMap<String, u64> = if holders_mode == "one" && live_holders > 1 {
                    self.store
                        .unanswered_by_holder(project, id)
                        .await
                        .map_err(store_error)?
                        .into_iter()
                        .collect()
                } else {
                    HashMap::new()
                };
                actions.extend(picture::actions_weighing(
                    project,
                    id,
                    holders_mode,
                    &mine,
                    live_seats,
                    &queued
                        .iter()
                        .filter(|request| request.role == *id)
                        .map(|request| request.instance.as_str())
                        .collect::<Vec<_>>(),
                    memory["latest"].as_str(),
                    &open,
                ));
            }
            let section = |name: &str| {
                document["sections"]
                    .as_array()
                    .and_then(|sections| sections.iter().find(|section| section["name"] == name))
                    .map_or(Value::Null, |section| section["value"].clone())
            };
            if !retired {
                scopes.push((
                    id.clone(),
                    section("scope")["paths"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|path| path.as_str().map(str::to_string))
                        .collect(),
                ));
            }
            let changelog = section("changelog");
            let created = changelog
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["date"].as_str())
                .filter_map(|date| date.get(..10))
                .min();
            if !retired {
                reviews.extend(picture::idle(
                    project,
                    &picture::RoleActivity {
                        role: id,
                        created,
                        rows: &mine,
                        live_holder: mine
                            .iter()
                            .any(|row| row.to.is_none() && live_seats.contains(&row.seat)),
                        waiting: waiting["count"]
                            .as_u64()
                            .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX)),
                        has_output: outputs.iter().any(|output| &output.role == id),
                        notes: memory["notes"]
                            .as_u64()
                            .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX)),
                    },
                    &cutoff,
                ));
            }
            // A retired role refuses sends, so it has nothing to call on.
            let call_on = if retired {
                None
            } else {
                picture::call_on(
                    project,
                    id,
                    mine.iter()
                        .any(|row| row.to.is_none() && live_seats.contains(&row.seat)),
                    memory["notes"]
                        .as_u64()
                        .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX)),
                )
            };
            let owned: Vec<Assignment> = mine.into_iter().cloned().collect();
            // A retired role no longer covers a core role's prompt (C22).
            if !retired {
                made_from.push((id.clone(), meta["template"].as_str().map(str::to_string)));
            }
            roles.push(serde_json::json!({
                "id": id,
                "title": meta["title"],
                "kind": meta["kind"],
                "holders": meta["holders"],
                "template": self.templates.drift(&meta),
                "state": standings[index].state,
                "merged_into": standings[index].merged_into,
                "merged_from": ids
                    .iter()
                    .zip(&standings)
                    .filter(|(_, standing)| {
                        standing.retired() && standing.merged_into.as_deref() == Some(id.as_str())
                    })
                    .map(|(from, _)| from.clone())
                    .collect::<Vec<_>>(),
                "live": live,
                "requests": waiting,
                "latest_output": picture::latest_output(id, &outputs),
                "memory": memory,
                "call_on": call_on,
                "lineage": holders::to_document(id, &owned)["sections"][1]["value"],
            }));
        }
        let step_docs = picture::read_step_docs(repo);
        let mut prompts = templates::recommendations(project, &made_from);
        // A retired role keeps its document, so a need naming it is
        // retargeted, never answered with `add` (review of #135, F004).
        let (needs, stale): (Vec<_>, Vec<_>) = picture::step_needs(&step_docs)
            .into_iter()
            .partition(|(need, _)| {
                !ids.iter()
                    .zip(&standings)
                    .any(|(id, standing)| id == need && standing.retired())
            });
        picture::fold_needs(project, &mut prompts, &needs, &made_from, &self.templates);
        let retired: Vec<(String, Option<String>)> = stale
            .iter()
            .filter_map(|(need, _)| {
                ids.iter()
                    .zip(&standings)
                    .find(|(id, _)| *id == need)
                    .map(|(id, standing)| (id.clone(), standing.merged_into.clone()))
            })
            .collect();
        prompts.retain(|prompt| !retired.iter().any(|(id, _)| *id == prompt.role));
        prompts.extend(picture::retarget(repo, &retired, &step_docs));
        // F21: a missing prime role document is not a missing prime.
        for prompt in prompts.iter_mut().filter(|prompt| prompt.role == "prime") {
            prompt.note = Some(match &prime {
                Some(seat) => format!("the project's prime is {seat}; this adds the prime role's document"),
                None => "this adds the prime role's document; the project records no designated prime either".to_string(),
            });
        }
        prompts.extend(picture::unowned(
            project,
            &picture::recurring_paths(repo, &step_docs),
            &scopes,
        ));
        let pending: Vec<Value> = rows
            .iter()
            .filter(|row| row.end_pending)
            .map(|row| serde_json::json!({"role": row.role, "seat": row.seat, "instance": row.instance, "to": row.to}))
            .collect();
        let prime_document = made_from
            .iter()
            .any(|(id, template)| id == "prime" || template.as_deref() == Some("prime"));
        // F39: only what a need or a C28 signal points at shows unasked.
        let (shown, held): (Vec<_>, Vec<_>) = prompts.into_iter().partition(|prompt| {
            suggest
                || !prompt.needed_by.is_empty()
                || !matches!(
                    prompt.signal,
                    picture::Signal::Core | picture::Signal::Independent
                )
        });
        let prompts = shown;
        let mut view = serde_json::json!({
            "project": project, "prime": prime, "prime_document": prime_document,
            "roles": roles, "pending": pending,
            "actions": actions, "notice": picture::notice(),
            "reviews": reviews.iter().map(picture::Recommendation::json).collect::<Vec<_>>(),
        });
        if !held.is_empty() {
            view["suggestions"] = serde_json::json!({
                "roles": held.iter().map(|prompt| prompt.role.clone()).collect::<Vec<_>>(),
                "command": format!(
                    "pij directory --suggest --project {}",
                    pij_core::directory::shell_word(project)
                ),
            });
        }
        if !prompts.is_empty() {
            view["prompts"] = prompts
                .iter()
                .map(picture::Recommendation::json)
                .collect::<Vec<_>>()
                .into();
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
    pub fn template_diff(
        &self,
        project: &str,
        repo: &std::path::Path,
        id: &str,
    ) -> Result<Value, Refusal> {
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
        let record = format!(
            "Record the version it came from: pij directory edit {} --set template_version=<n> --why <text> --project {}",
            shell_word(id),
            shell_word(project)
        );
        let version = match meta["template_version"].as_str().map(str::trim) {
            Some(version) if !version.is_empty() => version,
            _ => {
                return Err(Refusal::new(
                    E_NO_TEMPLATE_VERSION,
                    format!(
                        "role {id:?} records template {name:?} but no template_version, so there is nothing to diff from"
                    ),
                )
                .with_next(record));
            }
        };
        let from = templates::parse_version(version).ok_or_else(|| {
            Refusal::new(
                "E-RS-ARG",
                format!("role {id:?} records template_version {version:?}, not a version number"),
            )
            .with_next(record.clone())
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
        let date = super::holders::today();
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

    /// `pij directory memory add` (plan 201, AC1): one note and the
    /// regenerated INDEX in one commit, built under the writer lock so two adds
    /// to one role each keep the other's INDEX line. Returns the note's path.
    ///
    /// # Errors
    /// Any writer or [`super::memory`] refusal; nothing is written.
    pub async fn memory_add(
        &self,
        project: &str,
        repo: PathBuf,
        role_id: &str,
        fields: super::memory::NoteFields,
        actor: &SeatId,
    ) -> Result<(Written, String), Refusal> {
        let date = super::holders::today();
        let note = super::memory::note_path(role_id, &date, &fields)?;
        let pending = self.pending(project).await?;
        let message = format!(
            "community: memory {role_id}: {}\n\n{}\n\nPij-Seat: {actor}\nPij-Project: {project}",
            fields.title.trim(),
            fields.hook.trim()
        );
        let (project_id, role_id) = (project.to_string(), role_id.to_string());
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let mut files = super::memory::files(&root.worktree, &role_id, &date, &fields)?;
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
        Ok((written, note.display().to_string()))
    }

    /// Where role `id` stands (plan 206): active, or retired and perhaps
    /// merged into another. Read in-process from the governance worktree, no
    /// launch: the read s05's send to a role refuses on.
    ///
    /// # Errors
    /// [`super::E_GOV_ABSENT`]; [`E_ROLE_MISSING`].
    pub fn standing(&self, repo: &std::path::Path, id: &str) -> Result<role::Standing, Refusal> {
        let root = super::root::find(repo)?;
        Ok(role::standing(&read_role(&root, id)?))
    }

    /// `pij directory retire <id>` (plan 206, req-28): mark the role retired
    /// in one writer commit with a changelog row. Its memory, outputs and
    /// lineage stay where they are.
    ///
    /// # Errors
    /// [`E_ROLE_MISSING`]; [`E_RETIRED`] when it already is; [`E_ROLE_HELD`]
    /// while a seat holds it; [`E_ROLE_QUEUED`] while requests wait queued
    /// for it, unless `drop_queued`; any writer refusal. Nothing is written.
    pub async fn retire(
        &self,
        project: &str,
        repo: PathBuf,
        id: &str,
        actor: &SeatId,
        how: Retirement<'_>,
    ) -> Result<Reshaped, Refusal> {
        let why = how.why;
        let message = format!(
            "community: retire role {id}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}"
        );
        self.reshape(project, repo, message, id, None, actor, how)
            .await
    }

    /// `pij directory merge <from> --into <to>` (plan 206, req-28): retire
    /// `from` pointing at `to`, with a changelog row on both, in one writer
    /// commit. Nothing is moved: `from`'s memory, outputs and lineage stay
    /// where they are and the view lists `from` under `to`.
    ///
    /// # Errors
    /// As [`Self::retire`] for `from`; `E-RS-ARG` when `to` is `from`;
    /// [`E_ROLE_MISSING`] or [`E_RETIRED`] for `to`.
    pub async fn merge(
        &self,
        project: &str,
        repo: PathBuf,
        from: &str,
        into: &str,
        actor: &SeatId,
        how: Retirement<'_>,
    ) -> Result<Reshaped, Refusal> {
        let why = how.why;
        RolePaths::new(into)?;
        if from == into {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("role {from:?} cannot be merged into itself"),
            ));
        }
        let message = format!(
            "community: merge role {from} into {into}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}"
        );
        self.reshape(project, repo, message, from, Some(into), actor, how)
            .await
    }

    /// Retire `id`, merged into `into` when given, under the writer lock.
    #[allow(clippy::too_many_arguments)]
    async fn reshape(
        &self,
        project: &str,
        repo: PathBuf,
        message: String,
        id: &str,
        into: Option<&str>,
        actor: &SeatId,
        how: Retirement<'_>,
    ) -> Result<Reshaped, Refusal> {
        let why = how.why;
        let paths = RolePaths::new(id)?;
        let gate = self.admission(project, id).await;
        let finish = format!(
            "pij directory {} --why <text> --drop-queued --project {}",
            reshape_verb(id, into),
            shell_word(project)
        );
        let store_refusal =
            |error: pij_core::error::PijError| Refusal::new("E-RS-STORE", error.to_string());
        // A claimed request goes back on the queue if its delivery fails or the
        // daemon stops first, so the role cannot retire under it. A brake, not
        // a policy: it only ever refuses a retirement, never causes one.
        let delivering = self
            .store
            .delivering_for_role(project, id)
            .await
            .map_err(store_refusal)?;
        if !delivering.is_empty() {
            return Err(delivering_refusal(
                project,
                id,
                into,
                &delivering,
                system_ms(),
            ));
        }
        let queued = self
            .store
            .queued_for_role(project, id)
            .await
            .map_err(store_refusal)?;
        let standing = self.standing(&repo, id)?;
        if standing.retired() {
            // An earlier --drop-queued committed the retirement but did not
            // finish withdrawing: the same command finishes it, with no new
            // commit or changelog row (review of #135, F002).
            let same = into.is_none() || standing.merged_into.as_deref() == into;
            if how.drop_queued && same && !queued.is_empty() {
                return Ok(Reshaped {
                    written: None,
                    queued,
                    routes: Regenerated::default(),
                    finish,
                    gate,
                });
            }
            refuse_retired(id, &standing)?;
        }
        if !queued.is_empty() && !how.drop_queued {
            return Err(queued_refusal(project, id, into, &queued, system_ms()));
        }
        let pending = self.pending(project).await?;
        let (project_id, id, into, by, why) = (
            project.to_string(),
            id.to_string(),
            into.map(str::to_owned),
            actor.to_string(),
            why.to_string(),
        );
        let (written, built, gate) = self
            .commit_keeping(repo, message, gate, move |root, built| {
                let date = super::holders::today();
                let current = read_role(root, &id)?;
                refuse_retired(&id, &role::standing(&current))?;
                let rows = read_holders(root, &project_id, &id)?;
                let held: Vec<_> = rows.iter().filter(|row| row.to.is_none()).collect();
                if !held.is_empty() {
                    return Err(Refusal::new(
                        E_ROLE_HELD,
                        format!(
                            "role {id:?} is still held by {}; release it first",
                            held.iter()
                                .map(|row| format!(
                                    "{} ({})",
                                    row.seat,
                                    holding(&id, &row.instance)
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_next(format!(
                        "Release each holding first:{}",
                        held.iter()
                            .map(|row| format!(
                                "\n  pij directory release {} {} --why <text> --project {}",
                                shell_word(&holding(&id, &row.instance)),
                                shell_word(&row.seat),
                                shell_word(&project_id)
                            ))
                            .collect::<String>()
                    )));
                }
                let row_why = match &into {
                    Some(into) => format!("merged into {into}: {why}"),
                    None => why.clone(),
                };
                let retired = role::stamp(
                    role::retire(current, into.as_deref())?,
                    &id,
                    &by,
                    &row_why,
                    &date,
                )?;
                let mut files = vec![NewFile {
                    path: paths.document.clone(),
                    bytes: role::to_bytes(&retired),
                    document: true,
                    existing: Existing::Replace,
                }];
                if let Some(into) = &into {
                    let target = read_role(root, into)?;
                    refuse_retired(into, &role::standing(&target))?;
                    let stamped =
                        role::stamp(target, into, &by, &format!("{id} merged in: {why}"), &date)?;
                    files.push(NewFile {
                        path: RolePaths::new(into)?.document,
                        bytes: role::to_bytes(&stamped),
                        document: true,
                        existing: Existing::Replace,
                    });
                }
                let regenerated = routes::regenerate(&root.worktree, &files);
                files.extend(regenerated.file.clone());
                built.routes = Some(regenerated);
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
        Ok(Reshaped {
            written: Some(written),
            queued: if how.drop_queued { queued } else { Vec::new() },
            routes: built.routes.unwrap_or_default(),
            finish,
            gate,
        })
    }

    /// Withdraw the requests a `--drop-queued` retire or merge left queued
    /// (plan 206): delete their rows and append one `role.request.withdrawn`
    /// event each, in one store transaction, still under the role's gate,
    /// which the write task holds until that transaction ends, even if this
    /// call is cancelled first. Call it after telling each
    /// requester, so a failure here leaves the requests queued behind a retired
    /// role and the same command finishes the job (review of #135, F002).
    ///
    /// # Errors
    /// `E-RS-STORE`, naming the retirement that landed and the command that
    /// finishes the withdrawal.
    pub async fn withdraw(
        &self,
        reshaped: Reshaped,
        project: &str,
        role: &str,
        actor: &SeatId,
        why: &str,
        gone: Vec<String>,
    ) -> Result<Vec<pij_store::RoleRequest>, Refusal> {
        let Reshaped {
            written,
            queued,
            finish,
            gate,
            ..
        } = reshaped;
        if queued.is_empty() {
            return Ok(Vec::new());
        }
        let (store, project_id, role_id) =
            (self.store.clone(), project.to_string(), role.to_string());
        let withdrawal = pij_store::Withdrawal {
            by: actor.to_string(),
            why: why.to_string(),
            at: system_ms(),
            gone,
        };
        let withdrawn = self
            .event_bus
            .publish_committed_batch(async move {
                store
                    .withdraw_queued_role_requests(&project_id, &role_id, &withdrawal, gate)
                    .await
            })
            .await
            .map_err(|error| {
                Refusal::new(
                    "E-RS-STORE",
                    format!(
                        "role {role:?} is retired{} but its queued requests were not withdrawn: {error}",
                        written
                            .as_ref()
                            .map(|written| format!(" ({})", written.commit))
                            .unwrap_or_default()
                    ),
                )
                .with_next(format!("Run the same command again to finish: {finish}"))
            })?;
        Ok(withdrawn)
    }

    /// `pij directory edit <id>` (plan 206, F20): change an existing role in
    /// one writer commit. The role is read under the writer lock, the change
    /// applied, its changelog kept, and one row added.
    ///
    /// # Errors
    /// [`E_ROLE_MISSING`]; `E-RS-ARG` for an edit that changes nothing or a
    /// document for another id; any writer refusal. Nothing is written.
    pub async fn edit(
        &self,
        project: &str,
        repo: PathBuf,
        id: &str,
        change: RoleEdit,
        actor: &SeatId,
        why: &str,
    ) -> Result<(Written, Regenerated), Refusal> {
        let paths = RolePaths::new(id)?;
        let pending = self.pending(project).await?;
        let message = format!(
            "community: edit role {id}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}"
        );
        let (project_id, id, by, why) = (
            project.to_string(),
            id.to_string(),
            actor.to_string(),
            why.to_string(),
        );
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let current = read_role(root, &id)?;
                let standing = role::standing(&current);
                refuse_retired(&id, &standing)?;
                let edited = match change {
                    RoleEdit::Fields(fields) => role::apply_fields(current.clone(), &fields)?,
                    RoleEdit::Document(document) => role::carry_standing(
                        role::keep_changelog(document, &current),
                        &current,
                    ),
                };
                if role::standing(&edited) != standing {
                    return Err(Refusal::new(
                        "E-RS-ARG",
                        format!(
                            "edit does not change role {id:?}'s state or merged_into; use pij directory retire or merge"
                        ),
                    ));
                }
                if edited == current {
                    return Err(Refusal::new(
                        "E-RS-ARG",
                        format!("the edit changes nothing in role {id:?}"),
                    ));
                }
                let stamped = role::stamp(edited, &id, &by, &why, &super::holders::today())?;
                let mut files = vec![NewFile {
                    path: paths.document.clone(),
                    bytes: role::to_bytes(&stamped),
                    document: true,
                    existing: Existing::Replace,
                }];
                let regenerated = routes::regenerate(&root.worktree, &files);
                files.extend(regenerated.file.clone());
                built.routes = Some(regenerated);
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
        Ok((written, built.routes.unwrap_or_default()))
    }

    /// Any other directory write (`directory add`, `directory routes --write`):
    /// `files` plus the project's pending close-ends, in one commit. When the
    /// write changes the roles' scopes, or `ROUTES.md` is stale, the
    /// regenerated `ROUTES.md` rides the same commit (plan 202).
    ///
    /// # Errors
    /// Any writer refusal, or [`super::E_NOTHING_TO_WRITE`] when there is
    /// nothing at all to commit; nothing is written.
    pub async fn write(
        &self,
        project: &str,
        repo: PathBuf,
        files: Vec<NewFile>,
        message: String,
    ) -> Result<(Written, Regenerated), Refusal> {
        let pending = self.pending(project).await?;
        let (project_id, flush) = (project.to_string(), pending);
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                for file in files.iter().filter(|file| file.document) {
                    check_merge_target(root, file)?;
                }
                let mut files = files;
                let regenerated = routes::regenerate(&root.worktree, &files);
                files.extend(regenerated.file.clone());
                built.routes = Some(regenerated);
                files.extend(flush_files(
                    root,
                    &project_id,
                    &flush,
                    &[],
                    &mut built.landed,
                )?);
                if files.is_empty() {
                    return Err(Refusal::new(
                        super::E_NOTHING_TO_WRITE,
                        "nothing to write: ROUTES.md is current",
                    ));
                }
                Ok(files)
            })
            .await?;
        self.flushed(&built.landed, &written).await?;
        Ok((written, built.routes.unwrap_or_default()))
    }

    /// Bind a live seat to an existing role: one holders commit, then the store
    /// row and the seat's role in one transaction (C1), then the hook (C6).
    ///
    /// # Errors
    /// A named refusal; nothing is written.
    pub async fn assign(&self, request: AssignRequest) -> Result<HoldingReceipt, Refusal> {
        let mut receipts = self.assign_many(vec![request]).await?;
        Ok(receipts.remove(0))
    }

    /// Bind several live seats to existing roles in one holders commit, all or
    /// nothing (F16): every request must name this project, repo, actor and
    /// why. A refusal names every bad request. Then each holding gets what a
    /// single assign gives it: its store row and seat role, the hook and the
    /// window rule. Receipts come back in request order.
    ///
    /// # Errors
    /// A named refusal; nothing is written.
    pub async fn assign_many(
        &self,
        requests: Vec<AssignRequest>,
    ) -> Result<Vec<HoldingReceipt>, Refusal> {
        let Some(first) = requests.first().cloned() else {
            return Err(Refusal::new(
                "E-RS-ARG",
                "an assign batch needs at least one line",
            ));
        };
        let closed: Vec<String> = requests
            .iter()
            .filter(|request| request.seat.tombstoned_at.is_some())
            .map(|request| request.seat.id.to_string())
            .collect();
        if !closed.is_empty() {
            return Err(Refusal::new(
                "E-RS-ARG",
                format!("closed seats cannot be assigned: {}", closed.join(", ")),
            ));
        }
        let pending = self.pending(&first.project).await?;
        let drafts: Vec<Assignment> = requests
            .iter()
            .map(|request| Assignment {
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
            })
            .collect();
        let subject = match drafts.as_slice() {
            [one] => format!(
                "community: assign {}{} to {}",
                one.role,
                instance_suffix(&one.instance),
                one.seat
            ),
            many => format!(
                "community: assign {} holdings\n\n{}",
                many.len(),
                many.iter()
                    .map(|row| format!(
                        "{}{} {}",
                        row.role,
                        instance_suffix(&row.instance),
                        row.seat
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        };
        let message = format!(
            "{subject}\n\n{}\n\nPij-Seat: {}\nPij-Project: {}",
            first.why, first.actor, first.project
        );
        let flush = pending;
        let (written, built) = self
            .commit(first.repo.clone(), message, move |root, built| {
                let project = drafts[0].project.clone();
                let mut roles: Vec<String> = drafts.iter().map(|row| row.role.clone()).collect();
                roles.sort();
                roles.dedup();
                let mut problems = Vec::new();
                let mut by_role = BTreeMap::new();
                for role in &roles {
                    match read_holders(root, &project, role) {
                        Ok(mut rows) => {
                            refuse_retired(role, &super::role::standing(&read_role(root, role)?))?;
                            built.landed.extend(holders::apply_ends(&mut rows, &flush));
                            by_role.insert(role.clone(), rows);
                        }
                        Err(refusal) if refusal.code == E_ROLE_MISSING => {
                            problems.push(format!("role {role:?} does not exist"));
                        }
                        Err(refusal) => return Err(refusal),
                    }
                }
                for draft in &drafts {
                    let Some(rows) = by_role.get_mut(&draft.role) else {
                        continue;
                    };
                    if rows
                        .iter()
                        .any(|held| open_for(held, &draft.seat, &draft.instance))
                    {
                        problems.push(format!(
                            "{} already holds {}{}",
                            draft.seat,
                            draft.role,
                            instance_suffix(&draft.instance)
                        ));
                        continue;
                    }
                    let warning = (holders_mode(root, &draft.role)? == "one")
                        .then(|| rows.iter().find(|held| held.to.is_none()))
                        .flatten()
                        .map(|other| {
                            format!(
                                "W-ONE-HOLDER: {} is a one-holder role and {} still holds it",
                                draft.role, other.seat
                            )
                        });
                    let mut new = draft.clone();
                    new.from = holders::fresh_from(rows, &new.seat, &new.instance);
                    rows.push(new.clone());
                    built.batch.push((new, warning));
                }
                match problems.as_slice() {
                    [] => {}
                    [one] if drafts.len() == 1 => {
                        let code = if one.contains("already holds") {
                            E_HELD
                        } else {
                            E_ROLE_MISSING
                        };
                        let mut refusal = Refusal::new(code, one.clone());
                        if code == E_ROLE_MISSING {
                            refusal = refusal.with_next(format!(
                                "Add it first: pij directory add {} --why <text> ...",
                                drafts[0].role
                            ));
                        }
                        return Err(refusal);
                    }
                    many => {
                        let code = if many.iter().all(|problem| problem.contains("already holds")) {
                            E_HELD
                        } else {
                            E_ROLE_MISSING
                        };
                        return Err(Refusal::new(
                            code,
                            format!("the batch wrote nothing: {}", many.join("; ")),
                        ));
                    }
                }
                let mut files = Vec::new();
                for (role, rows) in &by_role {
                    files.push(holders_file(role, rows)?);
                }
                let written: Vec<&str> = roles.iter().map(String::as_str).collect();
                files.extend(flush_files(
                    root,
                    &project,
                    &flush,
                    &written,
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        self.flushed(&built.landed, &written).await?;
        let mut receipts = Vec::new();
        for ((row, warning), request) in built.batch.iter().zip(&requests) {
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
            let drained = self
                .hook
                .assigned(&row.project, &row.role, &row.instance, &request.seat.id)
                .await;
            let window = self.rename(&request.seat, row).await;
            receipts.push(
                HoldingReceipt {
                    commit: written.commit.clone(),
                    path: holders_path(&row.role)?,
                    row: holders::to_document(&row.role, std::slice::from_ref(row))["sections"][1]
                        ["value"][0]
                        .clone(),
                    warning: warning.clone(),
                    seat_role: commit.seat_role,
                    flushed: built.landed.len(),
                    window: Some(window),
                    drained,
                },
            );
        }
        Ok(receipts)
    }

    /// Name the holder's window `<project>:<role>`, or `<project>:<role>-<instance>`
    /// (req-26, C16), for the seat's first open holding (F17): a second role
    /// leaves the window as it is, with no launch. One tmux launch, through the
    /// adapter's recorded site; never the control client, whose allow-list
    /// stays reads only (S2).
    async fn rename(&self, seat: &SeatDescriptor, row: &Assignment) -> Value {
        let name = window_name(row);
        let first = match self.first_open(seat).await {
            Ok(first) => first,
            Err(unread) => return unread,
        };
        if let Some(first) = first.filter(|first| {
            (&first.project, &first.role, &first.instance, &first.from)
                != (&row.project, &row.role, &row.instance, &row.from)
        }) {
            let kept = window_name(&first);
            return serde_json::json!({
                "name": kept, "renamed": false,
                "reason": format!("the window keeps the seat's first open holding, {kept}"),
            });
        }
        self.rename_to(seat, name).await
    }

    /// After `released` ended (plan 212, F57): when it was the holding the
    /// window is named for, name the window for the seat's next first open
    /// holding, or give it back the seat's own name (what spawn names a
    /// window) when it holds nothing else. Otherwise the window keeps its
    /// name, with no launch. At most one tmux launch, at the assign's site.
    pub async fn window_after_release(&self, seat: &SeatDescriptor, released: &Value) -> Value {
        let first = match self.first_open(seat).await {
            Ok(first) => first,
            Err(unread) => return unread,
        };
        let released_from = released["from"].as_str().and_then(holders::parse_time);
        if let Some(first) = first
            .as_ref()
            .filter(|first| holders::parse_time(&first.from) < released_from)
        {
            let kept = window_name(first);
            return serde_json::json!({
                "name": kept, "renamed": false,
                "reason": format!("the window keeps the seat's first open holding, {kept}"),
            });
        }
        let name = first
            .as_ref()
            .map_or_else(|| seat.id.to_string(), window_name);
        self.rename_to(seat, name).await
    }

    /// The seat's earliest open holding, in any project. A failed read is
    /// not "no holdings" (review of #149, N001): the window keeps its name,
    /// and the error is the window receipt.
    async fn first_open(&self, seat: &SeatDescriptor) -> Result<Option<Assignment>, Value> {
        match self.store.open_assignments(seat.id.as_str()).await {
            Ok(held) => Ok(held
                .into_iter()
                .min_by_key(|held| (holders::parse_time(&held.from), held.from.clone()))),
            Err(cause) => Err(serde_json::json!({
                "name": null, "renamed": false,
                "reason": format!(
                    "the seat's open holdings could not be read ({cause}), so the window keeps its name"
                ),
            })),
        }
    }

    async fn rename_to(&self, seat: &SeatDescriptor, name: String) -> Value {
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

    /// Record a closed seat's past holding (plan 208, F15): `row` carries
    /// `from`, `to`, `released_by` and the whys. One holders commit, then the
    /// store row, with no live-seat check and no seat role. The caller checks
    /// the actor is the prime.
    ///
    /// # Errors
    /// [`E_OVERLAP`] when it overlaps a holding of the same seat and instance,
    /// [`E_ROLE_MISSING`], or any writer refusal; nothing is written.
    pub async fn lineage_add(
        &self,
        repo: PathBuf,
        row: Assignment,
        actor: &SeatId,
    ) -> Result<HoldingReceipt, Refusal> {
        let mut receipts = self.lineage_many(repo, vec![row], actor).await?;
        Ok(receipts.remove(0))
    }

    /// Record several past holdings in one holders commit, all or nothing
    /// (plan 209, F67): every row must name one project. A refusal names every
    /// bad row: a missing role, or an overlap with a holding of the same seat
    /// and instance, committed or earlier in the batch. Receipts come back in
    /// row order.
    ///
    /// # Errors
    /// [`E_OVERLAP`], [`E_ROLE_MISSING`], or any writer refusal; nothing is
    /// written.
    pub async fn lineage_many(
        &self,
        repo: PathBuf,
        rows: Vec<Assignment>,
        actor: &SeatId,
    ) -> Result<Vec<HoldingReceipt>, Refusal> {
        let Some(first) = rows.first().cloned() else {
            return Err(Refusal::new(
                "E-RS-ARG",
                "a lineage batch needs at least one line",
            ));
        };
        let pending = self.pending(&first.project).await?;
        let span_of = |row: &Assignment| {
            format!(
                "{}{} {} ({} to {})",
                row.role,
                instance_suffix(&row.instance),
                row.seat,
                row.from,
                row.to.as_deref().unwrap_or_default()
            )
        };
        let subject = match rows.as_slice() {
            [one] => format!("community: lineage {}", span_of(one)),
            many => format!(
                "community: lineage {} holdings\n\n{}",
                many.len(),
                many.iter().map(span_of).collect::<Vec<_>>().join("\n")
            ),
        };
        let message = format!(
            "{subject}\n\n{}\n\nPij-Seat: {actor}\nPij-Project: {}",
            first.why, first.project
        );
        let flush = pending;
        let drafts = rows.clone();
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let project = drafts[0].project.clone();
                let mut roles: Vec<String> = drafts.iter().map(|row| row.role.clone()).collect();
                roles.sort();
                roles.dedup();
                let mut problems: Vec<(&str, String)> = Vec::new();
                let mut by_role = BTreeMap::new();
                for role in &roles {
                    match read_holders(root, &project, role) {
                        Ok(mut held) => {
                            built.landed.extend(holders::apply_ends(&mut held, &flush));
                            by_role.insert(role.clone(), held);
                        }
                        Err(refusal) if refusal.code == E_ROLE_MISSING => {
                            problems
                                .push((E_ROLE_MISSING, format!("role {role:?} does not exist")));
                        }
                        Err(refusal) => return Err(refusal),
                    }
                }
                let span = |held: &Assignment| {
                    (
                        holders::parse_time(&held.from),
                        held.to.as_deref().and_then(holders::parse_time),
                    )
                };
                for draft in &drafts {
                    let Some(held) = by_role.get_mut(&draft.role) else {
                        continue;
                    };
                    let (from, to) = span(draft);
                    if let Some(other) = held.iter().find(|other| {
                        let (start, end) = span(other);
                        other.seat == draft.seat
                            && other.instance == draft.instance
                            && start < to
                            && end.is_none_or(|end| from < Some(end))
                    }) {
                        problems.push((
                            E_OVERLAP,
                            format!(
                                "{} held {}{} from {} to {}, which overlaps {} to {}",
                                other.seat,
                                other.role,
                                instance_suffix(&other.instance),
                                other.from,
                                other.to.as_deref().unwrap_or("now"),
                                draft.from,
                                draft.to.as_deref().unwrap_or_default()
                            ),
                        ));
                        continue;
                    }
                    // Lineage reads in time order.
                    let at = held
                        .iter()
                        .position(|other| holders::parse_time(&other.from) > from)
                        .unwrap_or(held.len());
                    held.insert(at, draft.clone());
                }
                match problems.as_slice() {
                    [] => {}
                    [(code, one)] if drafts.len() == 1 => {
                        return Err(Refusal::new(code, one.clone()));
                    }
                    many => {
                        let code = if many.iter().all(|(code, _)| *code == E_OVERLAP) {
                            E_OVERLAP
                        } else {
                            E_ROLE_MISSING
                        };
                        let named: Vec<&str> = many.iter().map(|(_, text)| text.as_str()).collect();
                        return Err(Refusal::new(
                            code,
                            format!("the batch wrote nothing: {}", named.join("; ")),
                        ));
                    }
                }
                let mut files = Vec::new();
                for (role, held) in &by_role {
                    files.push(holders_file(role, held)?);
                }
                let written: Vec<&str> = roles.iter().map(String::as_str).collect();
                files.extend(flush_files(
                    root,
                    &project,
                    &flush,
                    &written,
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        let mut receipts = Vec::with_capacity(rows.len());
        for row in &rows {
            self.store
                .record_history(row)
                .await
                .map_err(|error| store_refusal(&written, &error))?;
            receipts.push(
                HoldingReceipt {
                    commit: written.commit.clone(),
                    path: holders_path(&row.role)?,
                    row: holders::to_document(&row.role, std::slice::from_ref(row))["sections"][1]
                        ["value"][0]
                        .clone(),
                    warning: None,
                    seat_role: None,
                    flushed: built.landed.len(),
                    window: None,
                    drained: Vec::new(),
                },
            );
        }
        self.flushed(&built.landed, &written).await?;
        Ok(receipts)
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
        let target = ReleaseTarget {
            role: role.to_string(),
            instance: instance.to_string(),
            seat: seat.to_string(),
            from: None,
        };
        let released = self
            .release_many(project, repo, vec![target], actor, why, Unmatched::Refuse)
            .await?;
        Ok(released.receipts.into_iter().next().expect("one release"))
    }

    /// End several holdings in one holders commit (plan 215): `release
    /// --batch` (F71) and the holdings `pij delivered` ends (F70). With
    /// [`Unmatched::Refuse`] it is all or nothing, and a refusal names every
    /// target that is not an open holding. With [`Unmatched::Skip`] those are
    /// left alone and named in [`Released::skipped`]; when none matches,
    /// nothing is written. Receipts come back in target order.
    ///
    /// # Errors
    /// [`E_NOT_HELD`] (with `Refuse`, or with `Skip` when nothing matched),
    /// [`E_ROLE_MISSING`], or any writer refusal; nothing is written.
    pub async fn release_many(
        &self,
        project: &str,
        repo: PathBuf,
        targets: Vec<ReleaseTarget>,
        actor: &SeatId,
        why: &str,
        unmatched: Unmatched,
    ) -> Result<Released, Refusal> {
        if targets.is_empty() {
            return Err(Refusal::new(
                "E-RS-ARG",
                "a release batch needs at least one line",
            ));
        }
        let pending = self.pending(project).await?;
        let subject = match targets.as_slice() {
            [one] => format!(
                "community: release {}{} from {}",
                one.role,
                instance_suffix(&one.instance),
                one.seat
            ),
            many => format!(
                "community: release {} holdings\n\n{}",
                many.len(),
                many.iter()
                    .map(ReleaseTarget::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        };
        let message = format!("{subject}\n\n{why}\n\nPij-Seat: {actor}\nPij-Project: {project}");
        let (project_id, actor_id, release_why) =
            (project.to_string(), actor.to_string(), why.to_string());
        let flush = pending;
        let wanted = targets.clone();
        let (written, built) = self
            .commit(repo, message, move |root, built| {
                let mut roles: Vec<String> = wanted.iter().map(|t| t.role.clone()).collect();
                roles.sort();
                roles.dedup();
                let mut problems = Vec::new();
                let mut by_role = BTreeMap::new();
                for role in &roles {
                    match read_holders(root, &project_id, role) {
                        Ok(mut rows) => {
                            built.landed.extend(holders::apply_ends(&mut rows, &flush));
                            by_role.insert(role.clone(), rows);
                        }
                        Err(refusal) if refusal.code == E_ROLE_MISSING => {
                            problems
                                .push((E_ROLE_MISSING, format!("role {role:?} does not exist")));
                        }
                        Err(refusal) => return Err(refusal),
                    }
                }
                let to = holders::now();
                for (index, target) in wanted.iter().enumerate() {
                    let Some(rows) = by_role.get_mut(&target.role) else {
                        continue;
                    };
                    if wanted[..index].contains(target) {
                        problems.push((E_NOT_HELD, format!("{target} is named twice")));
                        continue;
                    }
                    // A pinned target ends only that exact holding, read here
                    // under the writer lock: a release and reassignment that
                    // landed since it was chosen leave the new holding alone
                    // (review of #168).
                    let Some(row) = rows.iter_mut().rev().find(|held| {
                        open_for(held, &target.seat, &target.instance)
                            && target.from.as_ref().is_none_or(|from| &held.from == from)
                    }) else {
                        problems.push((
                            E_NOT_HELD,
                            match &target.from {
                                Some(from) => format!(
                                    "{}'s {}{} holding from {from} is not open",
                                    target.seat,
                                    target.role,
                                    instance_suffix(&target.instance)
                                ),
                                None => format!(
                                    "{} does not hold {}{}",
                                    target.seat,
                                    target.role,
                                    instance_suffix(&target.instance)
                                ),
                            },
                        ));
                        continue;
                    };
                    row.to = Some(to.clone());
                    row.released_by = Some(actor_id.clone());
                    row.release_why = Some(release_why.clone());
                    built.batch.push((row.clone(), None));
                }
                match (unmatched, problems.as_slice()) {
                    (_, []) => {}
                    (Unmatched::Skip, skipped) if !built.batch.is_empty() => {
                        built.skipped = skipped.iter().map(|(_, text)| text.clone()).collect();
                    }
                    (_, [(code, one)]) if wanted.len() == 1 => {
                        return Err(Refusal::new(code, one.clone()));
                    }
                    (_, many) => {
                        let code = if many.iter().all(|(code, _)| *code == E_NOT_HELD) {
                            E_NOT_HELD
                        } else {
                            E_ROLE_MISSING
                        };
                        let named: Vec<&str> = many.iter().map(|(_, text)| text.as_str()).collect();
                        return Err(Refusal::new(
                            code,
                            format!("the batch wrote nothing: {}", named.join("; ")),
                        ));
                    }
                }
                let mut files = Vec::new();
                let mut written = Vec::new();
                for (role, rows) in &by_role {
                    if built.batch.iter().any(|(row, _)| &row.role == role) {
                        files.push(holders_file(role, rows)?);
                        written.push(role.as_str());
                    }
                }
                files.extend(flush_files(
                    root,
                    &project_id,
                    &flush,
                    &written,
                    &mut built.landed,
                )?);
                Ok(files)
            })
            .await?;
        let rows: Vec<Assignment> = built.batch.iter().map(|(row, _)| row.clone()).collect();
        self.store
            .end_assignments(&rows)
            .await
            .map_err(|error| store_refusal(&written, &error))?;
        self.flushed(&built.landed, &written).await?;
        let mut receipts = Vec::with_capacity(rows.len());
        for row in &rows {
            receipts.push(
                HoldingReceipt {
                    commit: written.commit.clone(),
                    path: holders_path(&row.role)?,
                    row: holders::to_document(&row.role, std::slice::from_ref(row))["sections"][1]
                        ["value"][0]
                        .clone(),
                    warning: None,
                    seat_role: None,
                    flushed: built.landed.len(),
                    window: None,
                    drained: Vec::new(),
                },
            );
        }
        Ok(Released {
            receipts,
            skipped: built.skipped,
        })
    }

    /// The open holdings PR `pr` finished (plan 215, F70), each pinned to
    /// its exact holding (its `from`) for [`Self::release_many`]: the
    /// [`PR_ROLES`] keyed by the PR, and `coder` keyed by each of `plans`.
    /// Only a holding that began before `merged` is one (review of #168): the
    /// merge time binds the release to the work the PR ended, so a key
    /// reassigned after the merge is new work and comes back as a note
    /// instead. The pin carries that choice through to the writer, which ends
    /// a target only if it is still that holding.
    ///
    /// # Errors
    /// `E-RS-STORE` when the holdings cannot be read.
    pub async fn finished_by(
        &self,
        project: &str,
        pr: u64,
        plans: &std::collections::BTreeSet<u64>,
        merged: time::OffsetDateTime,
    ) -> Result<(Vec<ReleaseTarget>, Vec<String>), Refusal> {
        let pr = pr.to_string();
        let mut targets = Vec::new();
        let mut later = Vec::new();
        for held in self
            .store
            .assignments(project)
            .await
            .map_err(|error| Refusal::new("E-RS-STORE", error.to_string()))?
        {
            let finished = (PR_ROLES.contains(&held.role.as_str()) && held.instance == pr)
                || (held.role == "coder"
                    && held
                        .instance
                        .parse::<u64>()
                        .is_ok_and(|plan| plans.contains(&plan)));
            if !finished || held.to.is_some() || held.end_pending {
                continue;
            }
            let before = holders::parse_time(&held.from).is_some_and(|from| from < merged);
            let target = ReleaseTarget {
                role: held.role,
                instance: held.instance,
                seat: held.seat,
                from: Some(held.from.clone()),
            };
            if before {
                targets.push(target);
            } else {
                later.push(format!(
                    "holding left open: {target} began at {}, after the merge",
                    held.from
                ));
            }
        }
        Ok((targets, later))
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
        self.commit_keeping(repo, message, (), build)
            .await
            .map(|(written, built, ())| (written, built))
    }

    /// [`Self::commit`], with `keep` moved into the blocking job and handed
    /// back when the commit lands. A caller cancelled while the job waits for
    /// the writer leaves `keep` with the job, which can still commit, so an
    /// admission guard is held for as long as its write can land (review of
    /// #135, F001); a refused write drops it inside the job.
    async fn commit_keeping<K: Send + 'static>(
        &self,
        repo: PathBuf,
        message: String,
        keep: K,
        build: impl FnOnce(&GovernanceRoot, &mut Built) -> Result<Vec<NewFile>, Refusal>
        + Send
        + 'static,
    ) -> Result<(Written, Built, K), Refusal> {
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || {
            let mut built = Built::default();
            let written = writer.write_with(
                &repo,
                |root| {
                    let mut files = build(root, &mut built)?;
                    // Every directory commit keeps ROUTES.md true (plan 215,
                    // F72). A write that changes scopes regenerated it already;
                    // any other (assign, release, lineage, close) carries it
                    // only when it is stale, since holders are not in it.
                    if built.routes.is_none() {
                        let regenerated = routes::regenerate(&root.worktree, &files);
                        files.extend(regenerated.file.clone());
                        built.routes = Some(regenerated);
                    }
                    Ok(files)
                },
                &message,
            )?;
            Ok((written, built, keep))
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

/// A role document's `merged_into` must name another role in this directory
/// (docs-system verdict on #135, note 2); ddocs cannot check it.
fn check_merge_target(root: &GovernanceRoot, file: &NewFile) -> Result<(), Refusal> {
    let Some(id) = file
        .path
        .parent()
        .filter(|_| {
            file.path
                .file_name()
                .is_some_and(|name| name == "role.dd.json")
        })
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
    else {
        return Ok(());
    };
    let Ok(document) = serde_json::from_slice::<Value>(&file.bytes) else {
        return Ok(());
    };
    let Some(into) = role::standing(&document).merged_into else {
        return Ok(());
    };
    if into == id {
        return Err(Refusal::new(
            "E-RS-ARG",
            format!("role {id:?} cannot be merged into itself"),
        ));
    }
    read_role(root, &into).map(|_| ())
}

/// `role` or `role/instance`, as the directory verbs take it.
fn holding(role: &str, instance: &str) -> String {
    if instance.is_empty() {
        role.to_string()
    } else {
        format!("{role}/{instance}")
    }
}

/// Refuse to retire `id` (merged into `into`, if given) in `project` while
/// requests wait queued for it, listing each by id, requester, instance and
/// age, with the exact commands that drain or withdraw them (review of #135,
/// F003: the project and instance are kept).
fn queued_refusal(
    project: &str,
    id: &str,
    into: Option<&str>,
    queued: &[pij_store::RoleRequest],
    now_ms: u64,
) -> Refusal {
    let listed: Vec<String> = queued
        .iter()
        .map(|request| {
            format!(
                "{} from {} for {} ({} old)",
                request.id,
                request.requester,
                holding(id, &request.instance),
                pij_core::cold_wake::human_duration(now_ms.saturating_sub(request.created_at))
            )
        })
        .collect();
    let mut targets: Vec<String> = queued
        .iter()
        .map(|request| holding(id, &request.instance))
        .collect();
    targets.sort();
    targets.dedup();
    let verb = reshape_verb(id, into);
    let project = shell_word(project);
    let assigns: String = targets
        .iter()
        .map(|target| {
            format!(
                "\n  pij directory assign {} <seat> --why <text> --project {project}",
                shell_word(target)
            )
        })
        .collect();
    Refusal::new(
        E_ROLE_QUEUED,
        format!(
            "role {id:?} has {} request(s) queued for a holder: {}; nothing was written",
            queued.len(),
            listed.join(", ")
        ),
    )
    .with_next(format!(
        "Assign a holder to take them, release it, then {verb} again:{assigns}\n\
         Or withdraw them, telling each requester once:\n  pij directory {verb} --why <text> --drop-queued --project {project}"
    ))
}

/// The refusal of a retire or merge while requests for `id` are mid-delivery
/// (review of #135, F001): each is listed, and the same command is the next
/// step once they settle. `--drop-queued` cannot withdraw them.
fn delivering_refusal(
    project: &str,
    id: &str,
    into: Option<&str>,
    delivering: &[pij_store::RoleRequest],
    now_ms: u64,
) -> Refusal {
    let listed: Vec<String> = delivering
        .iter()
        .map(|request| {
            format!(
                "{} from {} to {} ({} old)",
                request.id,
                request.requester,
                request.holder.as_deref().unwrap_or("?"),
                pij_core::cold_wake::human_duration(now_ms.saturating_sub(request.created_at))
            )
        })
        .collect();
    Refusal::new(
        E_ROLE_QUEUED,
        format!(
            "role {id:?} has {} request(s) being delivered: {}; a failed delivery puts one back on the queue, so nothing was written",
            delivering.len(),
            listed.join(", ")
        ),
    )
    .with_next(format!(
        "Once they are delivered or back on the queue, run it again:\n  pij directory {} --why <text> --project {}",
        reshape_verb(id, into),
        shell_word(project)
    ))
}

/// `retire <id>` or `merge <id> --into <into>`, each name a shell word.
fn reshape_verb(id: &str, into: Option<&str>) -> String {
    match into {
        Some(into) => format!("merge {} --into {}", shell_word(id), shell_word(into)),
        None => format!("retire {}", shell_word(id)),
    }
}

/// Refuse a write to a retired role, naming where a merged one went.
fn refuse_retired(id: &str, standing: &role::Standing) -> Result<(), Refusal> {
    if !standing.retired() {
        return Ok(());
    }
    Err(match &standing.merged_into {
        Some(into) => Refusal::new(E_RETIRED, format!("role {id:?} was merged into {into:?}"))
            .with_next(format!("Use {into} instead.")),
        None => Refusal::new(E_RETIRED, format!("role {id:?} is retired")),
    })
}

/// The role's committed document.
fn read_role(root: &GovernanceRoot, id: &str) -> Result<Value, Refusal> {
    let paths = RolePaths::new(id)?;
    let missing = || {
        Refusal::new(
            E_ROLE_MISSING,
            format!("role {id:?} does not exist in this project's directory"),
        )
        .with_next(format!(
            "Add it first: pij directory add {id} --why <text> ..."
        ))
    };
    let bytes = std::fs::read(root.worktree.join(&paths.document)).map_err(|_| missing())?;
    serde_json::from_slice(&bytes).map_err(|error| {
        Refusal::new(
            E_GOV_GIT,
            format!("{} is not JSON: {error}", paths.document.display()),
        )
    })
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

/// What a write's build reports back: the pending ends its files carry, the
/// holding row it wrote, and an assign batch's rows with their warnings.
#[derive(Default)]
struct Built {
    landed: Vec<Assignment>,
    routes: Option<Regenerated>,
    batch: Vec<(Assignment, Option<String>)>,
    skipped: Vec<String>,
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

/// `<project>:<role>`, or `<project>:<role>-<instance>` (req-26, C16).
fn window_name(held: &Assignment) -> String {
    if held.instance.is_empty() {
        format!("{}:{}", held.project, held.role)
    } else {
        format!("{}:{}-{}", held.project, held.role, held.instance)
    }
}
