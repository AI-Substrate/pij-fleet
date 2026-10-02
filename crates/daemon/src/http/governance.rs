//! Governance argv handlers over the shared SQLite store and sole event bus.
//! Composition owns routing, the role service, the pool, and the supervised
//! delivery observer. This module never opens a store or starts a detached task.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Json, OriginalUri, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::{PijError, Result};
use pij_core::events::EventFilter;
use pij_core::liveness::alive;
use pij_core::model::{
    DeliveryOutcome, Envelope, ErrorKind, Event, Liveness, Msg, SeatDescriptor, SeatId, Seq,
};
use pij_core::orchestration::{
    BatonDefinition, BatonRequest, BatonRequestState, Dispatch, DispatchCanary, DispatchState,
    Fence, PlanAttestation, PrimeDesignation, PrimeState, Project, ProjectUpdate, StreamState,
    TaskAssignment, TaskCloseReason, plan_stream_creation, plan_stream_creation_at_ordinal,
};
use pij_core::ports::{SeatFilter, Spine};
use pij_core::report::{ReportConfig, ReportService};
use pij_store::{DispatchAck, GovernanceOutcome, SqliteOrchestration};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, system_time_ms};
use crate::events::EventBus;
use crate::orchestration_repo::{StreamCreation, reserve_and_create_stream, scan_repo_inventory};

/// Shared governance persistence and delivery-receipt projection.
/// Construct after raw spine -> EventBus -> registry, with the same store pool.
/// `packet_dir` is a private subdirectory of the daemon state directory.
pub struct GovernanceService {
    store: SqliteOrchestration,
    event_bus: Arc<EventBus>,
    packet_dir: PathBuf,
    mutation: Mutex<()>,
}

impl GovernanceService {
    /// Wrap existing production resources; never opens a pool or starts work.
    pub fn new(store: SqliteOrchestration, event_bus: Arc<EventBus>, packet_dir: PathBuf) -> Self {
        Self {
            store,
            event_bus,
            packet_dir,
            mutation: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(super) fn fixture_store(&self) -> SqliteOrchestration {
        self.store.clone()
    }

    /// Project real delivery outcomes. The caller supervises this future and
    /// propagates failure; dropping it shuts the observer down. Subscribe before
    /// reconciliation so no receipt can fall between startup and the live tail.
    pub async fn follow_deliveries(self: Arc<Self>) -> Result<()> {
        let mut events = self.event_bus.subscribe_live(EventFilter::all());
        self.reconcile_deliveries().await?;
        let mut dropped = 0;
        while let Some(event) = events.next().await {
            if events.dropped_count() != dropped {
                self.reconcile_deliveries().await?;
                dropped = events.dropped_count();
            }
            if event.kind == "delivery.outcome" {
                let _guard = self.mutation.lock().await;
                self.observe_delivery(&event).await?;
            }
        }
        Err(adapter("delivery event subscription ended"))
    }

    async fn reconcile_deliveries(&self) -> Result<()> {
        let _guard = self.mutation.lock().await;
        let history = self.event_bus.tail(None, Seq(0)).await?;
        let mut delivered = BTreeMap::new();
        for event in &history {
            if event.kind != "delivery.outcome" {
                continue;
            }
            // User spine kinds are open strings. A same-named user event is
            // not a transport receipt unless it decodes as one.
            let Ok(outcome) = serde_json::from_str::<DeliveryEvent>(&event.payload) else {
                continue;
            };
            if matches!(outcome.outcome, DeliveryOutcome::Delivered { .. }) {
                delivered.entry(outcome.msg_id).or_insert(event);
            }
        }
        for dispatch in self.store.list_dispatches(None).await? {
            if let Some(event) = dispatch.msg_id.as_ref().and_then(|id| delivered.get(id)) {
                self.observe_delivery(event).await?;
            }
        }
        Ok(())
    }

    async fn observe_delivery(&self, event: &Event) -> Result<()> {
        let Ok(outcome) = serde_json::from_str::<DeliveryEvent>(&event.payload) else {
            return Ok(());
        };
        if !matches!(outcome.outcome, DeliveryOutcome::Delivered { .. }) {
            return Ok(());
        }
        let Some(dispatch) = self.store.dispatch(&outcome.msg_id).await? else {
            return Ok(());
        };
        if dispatch.msg_id.as_deref() != Some(outcome.msg_id.as_str())
            || event.seat.as_ref() != Some(&dispatch.to)
        {
            return Err(adapter(
                "delivery outcome does not match its dispatch recipient/linkage",
            ));
        }
        let dispatch = match self
            .store
            .mark_dispatch_delivered(&dispatch.id, event.at)
            .await?
        {
            GovernanceOutcome::Changed(row) | GovernanceOutcome::Unchanged(row) => row,
            GovernanceOutcome::Missing => {
                return Err(adapter("dispatch disappeared during delivery projection"));
            }
            GovernanceOutcome::Conflict { code } => return Err(adapter(code)),
        };
        // Recovery is not conditional on Changed: a previous attempt may have
        // committed the row and failed to publish. Match the durable transition
        // evidence, then repair the missing publication on startup/replay.
        let history = self.event_bus.tail(Some(&dispatch.to), Seq(0)).await?;
        let published = history.iter().any(|event| {
            if event.kind != "dispatch" {
                return false;
            }
            let Ok(payload) = serde_json::from_str::<Value>(&event.payload) else {
                return false;
            };
            payload["action"] == "delivered"
                && payload["record"]["id"] == dispatch.id
                && payload["record"]["delivered_at"].as_u64() == dispatch.delivered_at
        });
        if !published {
            self.publish("dispatch", &dispatch.from, Some(&dispatch.to), "delivered", &dispatch, event.at)
                .await.map_err(|error| adapter(format!(
                    "E-RS-PARTIAL dispatch {} delivery row committed, dispatch event unpublished; reconciliation will retry: {}",
                    dispatch.id, error.message,
                )))?;
        }
        Ok(())
    }

    async fn publish<T: Serialize + Sync>(
        &self,
        kind: &str,
        actor: &SeatId,
        subject: Option<&SeatId>,
        action: &str,
        record: &T,
        at: u64,
    ) -> GovResult<u64> {
        let payload =
            serde_json::to_string(&json!({"actor": actor, "action": action, "record": record}))
                .map_err(|error| GovernanceError::internal(error.to_string()))?;
        self.event_bus.publish(Event {
            seq: None, v: 1, at, kind: kind.to_string(), seat: subject.cloned(), payload,
        }).await.map(|seq| seq.0).map_err(|error| GovernanceError { code: ("E-RS-PARTIAL").into(), status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("state committed but {kind} event publication failed: {error}"),
        details: json!({"committed": true, "event_published": false, "kind": kind, "action": action, "record": record}), })
    }

    async fn published_or_repair<T: Serialize>(
        &self,
        kind: &str,
        actor: &SeatId,
        subject: Option<&SeatId>,
        action: &str,
        record: &T,
        at: u64,
    ) -> GovResult<u64> {
        let record =
            serde_json::to_value(record).map_err(|e| GovernanceError::internal(e.to_string()))?;
        for event in self
            .event_bus
            .tail(subject, Seq(0))
            .await?
            .into_iter()
            .rev()
        {
            if event.kind != kind {
                continue;
            }
            let payload: Value = serde_json::from_str(&event.payload)
                .map_err(|e| GovernanceError::internal(e.to_string()))?;
            if payload["actor"] == actor.as_str()
                && payload["action"] == action
                && payload["record"] == record
            {
                return event
                    .seq
                    .map(|seq| seq.0)
                    .ok_or_else(|| GovernanceError::internal("durable event has no sequence"));
            }
        }
        self.publish(kind, actor, subject, action, &record, at)
            .await
    }
}

#[derive(Deserialize)]
struct DeliveryEvent {
    msg_id: String,
    outcome: DeliveryOutcome,
}

/// The identical argv/caller wire shape used by native CLI and legacy shim.
#[derive(Debug, Deserialize)]
pub struct GovernanceRequest {
    /// Full argv, including family exactly once.
    pub argv: Vec<String>,
    /// Caller claims, resolved by the existing identity ladder.
    #[serde(default)]
    pub caller: CallerContext,
}

type GovResult<T> = std::result::Result<T, GovernanceError>;

#[derive(Debug)]
struct GovernanceError {
    code: String,
    status: StatusCode,
    message: String,
    details: Value,
}

impl GovernanceError {
    fn refused(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            details: json!({}),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: "E-RS-STORE".to_string(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            details: json!({}),
        }
    }
    fn response(self, command: &str) -> Response {
        let mut body = Envelope::<Value>::refused(
            command,
            if self.status.is_server_error() {
                ErrorKind::Adapter
            } else {
                ErrorKind::Refused
            },
            format!("{} {}", self.code, self.message),
        );
        let mut details = self.details;
        details["code"] = json!(self.code);
        body.details = Some(details);
        envelope(self.status, &body)
    }
}

impl From<PijError> for GovernanceError {
    fn from(error: PijError) -> Self {
        match error {
            PijError::GovernanceRefused { code, record } => {
                let status = if code == "E-RS-OWNERSHIP" {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::CONFLICT
                };
                let mut details = json!({"record": record});
                if code == "E-RS-LEASE-STALE" {
                    details["current_lease"] = if record == "absent" {
                        Value::Null
                    } else {
                        json!(record)
                    };
                }
                Self {
                    code,
                    status,
                    message: format!("governance transition refused for {record}"),
                    details,
                }
            }
            error => Self::internal(error.to_string()),
        }
    }
}

fn adapter(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/governance".to_string(),
        message: message.into(),
    }
}

fn required<T>(value: Option<T>, kind: &str, id: &str) -> GovResult<T> {
    value.ok_or_else(|| {
        GovernanceError::refused("E-RS-NOT-FOUND", format!("{kind} {id:?} does not exist"))
    })
}

fn changed<T>(outcome: GovernanceOutcome<T>, kind: &str, id: &str) -> GovResult<T> {
    match outcome {
        GovernanceOutcome::Changed(row) | GovernanceOutcome::Unchanged(row) => Ok(row),
        GovernanceOutcome::Missing => Err(GovernanceError::refused(
            "E-RS-NOT-FOUND",
            format!("{kind} {id:?} does not exist"),
        )),
        GovernanceOutcome::Conflict { code } => Err(GovernanceError::refused(
            code,
            format!("{kind} {id:?} transition refused"),
        )),
    }
}

#[derive(Debug)]
struct Arguments {
    family: String,
    leaf: String,
    positions: Vec<String>,
    flags: BTreeMap<String, Option<String>>,
}

impl Arguments {
    fn parse(argv: &[String]) -> GovResult<Self> {
        let family = argv
            .first()
            .ok_or_else(|| GovernanceError::refused("E-RS-ARG", "missing command family"))?
            .clone();
        let mut tokens = argv[1..].iter().peekable();
        let mut flags = BTreeMap::new();
        let mut positions = Vec::new();
        let mut literal = false;
        while let Some(token) = tokens.next() {
            if token == "--" && !literal {
                literal = true;
                continue;
            }
            if !literal && let Some(flag) = token.strip_prefix("--") {
                let (name, value) = if let Some((name, value)) = flag.split_once('=') {
                    (name.to_string(), Some(value.to_string()))
                } else if matches!(flag, "json" | "repin")
                    || (matches!(flag, "wait" | "lease-id")
                        && tokens.peek().is_none_or(|next| next.starts_with("--")))
                {
                    (flag.to_string(), None)
                } else {
                    let value = tokens
                        .next()
                        .filter(|next| !next.starts_with("--"))
                        .ok_or_else(|| {
                            GovernanceError::refused(
                                "E-RS-ARG",
                                format!("--{flag} requires a value"),
                            )
                        })?;
                    (flag.to_string(), Some(value.to_string()))
                };
                if flags.insert(name.clone(), value).is_some() {
                    return Err(GovernanceError::refused(
                        "E-RS-ARG",
                        format!("duplicate --{name}"),
                    ));
                }
            } else {
                positions.push(token.to_string());
            }
        }
        let direct = matches!(family.as_str(), "dispatch" | "ack" | "canary" | "attest");
        let leaf = if direct {
            String::new()
        } else if positions.is_empty() {
            return Err(GovernanceError::refused(
                "E-RS-ARG",
                format!("{family} requires a leaf"),
            ));
        } else {
            positions.remove(0)
        };
        let leaf = if family == "orchestration" {
            if positions.is_empty() {
                return Err(GovernanceError::refused(
                    "E-RS-ARG",
                    "orchestration requires a leaf",
                ));
            }
            format!("{leaf}.{}", positions.remove(0))
        } else {
            leaf
        };
        let (count, allowed): (usize, &[&str]) = match (family.as_str(), leaf.as_str()) {
            ("project", "create") => (1, &["repo", "plan", "prime"]),
            ("project", "list") => (0, &[]),
            ("project", "show") => (1, &[]),
            ("project", "set") => (1, &["description", "repo", "plan", "prime"]),
            ("stream", "create") => (0, &["project", "slug", "base", "ordinal", "root"]),
            ("stream", "list") => (0, &["project"]),
            ("stream", "show" | "close") => (1, &[]),
            ("fence", "set") => (1, &["paths", "shared"]),
            ("fence", "show") => (0, &["stream", "path"]),
            ("dispatch", "") => (1, &["packet", "wait"]),
            ("ack", "") => (1, &["packet-sha"]),
            ("canary", "") => (1, &["expect-model", "wait"]),
            ("attest", "") => (1, &["plan-id"]),
            ("task", "set") => (2, &["project"]),
            ("task", "close") => (1, &["reason"]),
            ("node", "show") => (1, &[]),
            ("orchestration", "baton.define") => (1, &["resource", "probe", "repo"]),
            ("orchestration", "baton.list") => (0, &[]),
            ("orchestration", "baton.show") => (1, &[]),
            ("orchestration", "baton.request") => (1, &["purpose", "pin", "evidence"]),
            ("orchestration", "baton.grant") => (1, &["to", "repin"]),
            ("orchestration", "baton.return" | "baton.reclaim") => (1, &["evidence", "lease-id"]),
            ("orchestration", "prime.set" | "prime.retire" | "prime.unset") => (1, &[]),
            ("orchestration", "role.set") => (2, &[]),
            ("orchestration", "role.unset") => (1, &[]),
            ("spine", "append") => (0, &["kind", "refs", "project"]),
            ("spine", "events" | "render") => (0, &["since", "peer", "project"]),
            _ => {
                return Err(GovernanceError::refused(
                    "E-RS-UNPORTED",
                    format!("{family} {leaf}: unknown command"),
                ));
            }
        };
        if positions.len() != count {
            return Err(GovernanceError::refused(
                "E-RS-ARG",
                format!("{family} {leaf}: expected {count} positional arguments"),
            ));
        }
        for (flag, value) in &flags {
            if flag != "json" && flag != "actor" && !allowed.contains(&flag.as_str()) {
                return Err(GovernanceError::refused(
                    "E-RS-ARG",
                    format!("{family} {leaf}: unsupported --{flag}"),
                ));
            }
            if matches!(flag.as_str(), "json" | "repin") && value.is_some() {
                return Err(GovernanceError::refused(
                    "E-RS-ARG",
                    format!("--{flag} does not take a value"),
                ));
            }
        }
        Ok(Self {
            family,
            leaf,
            positions,
            flags,
        })
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(|value| value.as_deref())
    }
    fn need(&self, name: &str) -> GovResult<&str> {
        self.value(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                GovernanceError::refused("E-RS-ARG", format!("--{name} requires a non-empty value"))
            })
    }
    fn wait(&self, default: u64) -> GovResult<Duration> {
        let ms = match self.flags.get("wait") {
            None => default,
            Some(None) => 5_000,
            Some(Some(value)) => value.parse::<u64>().map_err(|_| {
                GovernanceError::refused("E-RS-ARG", "--wait requires milliseconds")
            })?,
        };
        if ms > 300_000 {
            return Err(GovernanceError::refused(
                "E-RS-ARG",
                "--wait exceeds 300000ms",
            ));
        }
        Ok(Duration::from_millis(ms))
    }
}

pub(crate) async fn handle(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    Json(request): Json<GovernanceRequest>,
) -> Response {
    let command = format!("pij {}", uri.path().trim_start_matches("/v1/"));
    let call = match Arguments::parse(&request.argv) {
        Ok(call) => call,
        Err(error) => return error.response(&command),
    };
    if uri.path() != format!("/v1/{}", call.family) {
        return GovernanceError::refused("E-RS-ARG", "argv family does not match route")
            .response(&command);
    }
    let actor = match resolve_seat(
        &state,
        &command,
        request.caller.session_id.clone(),
        request.caller.pane.clone(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    if call
        .value("actor")
        .is_some_and(|claimed| claimed != actor.id.as_str())
    {
        return GovernanceError { code: ("E-RS-OWNERSHIP").into(), status: StatusCode::FORBIDDEN,
            message: "--actor does not match the daemon-resolved caller".to_string(),
            details: json!({"caller": actor.id, "claimed_actor": call.value("actor"), "operation": call.family}) }.response(&command);
    }
    if call.family == "orchestration" && matches!(call.leaf.as_str(), "role.set" | "role.unset") {
        let target = SeatId::from(call.positions[0].as_str());
        let role = (call.leaf == "role.set").then(|| call.positions[1].clone());
        return match state
            .services
            .roles
            .assert_role(&actor.id, &target, role)
            .await
        {
            Ok(receipt) => envelope(
                StatusCode::OK,
                &Envelope::ok(
                    command,
                    json!({
                        "role": {"seat": receipt.seat, "role": receipt.role,
                            "assigned_by": receipt.assigned_by, "assigned_at": receipt.assigned_at},
                        "seq": receipt.seq,
                    }),
                ),
            ),
            Err(error) => error.into_response(&command),
        };
    }
    let service = &state.services.governance;
    let result = match call.family.as_str() {
        "dispatch" => dispatch_command(service, &state, &actor, &request.caller, &call).await,
        "canary" => canary_command(service, &state, &actor, &call).await,
        _ => {
            let _guard = service.mutation.lock().await;
            execute(service, &state, &actor, &request.caller, &call).await
        }
    };
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(error) => error.response(&command),
    }
}

async fn target_seat(state: &AppState, id: &str) -> GovResult<SeatDescriptor> {
    let target = required(
        state.services.registry.get(&SeatId::from(id)).await?,
        "seat",
        id,
    )?;
    Ok(target)
}

fn relative_to(caller: &CallerContext, path: &str) -> GovResult<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(caller_directory(caller)?.join(path))
}

fn caller_directory(caller: &CallerContext) -> GovResult<PathBuf> {
    caller
        .cwd
        .as_deref()
        .filter(|cwd| Path::new(cwd).is_absolute())
        .map(PathBuf::from)
        .ok_or_else(|| GovernanceError::refused("E-RS-ARG", "an absolute caller.cwd is required"))
}

fn mint(prefix: &str) -> GovResult<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| GovernanceError::internal(error.to_string()))?;
    let mut id = String::with_capacity(prefix.len() + 33);
    id.push_str(prefix);
    id.push('-');
    for byte in bytes {
        write!(id, "{byte:02x}").expect("String write");
    }
    Ok(id)
}

fn slug(input: &str) -> GovResult<String> {
    let mut slug = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() || slug.len() > 48 {
        return Err(GovernanceError::refused(
            "E-RS-ARG",
            "project slug must contain 1–48 ASCII letters/digits/hyphens",
        ));
    }
    Ok(slug)
}

pub(crate) fn csv(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

async fn execute(
    service: &GovernanceService,
    state: &AppState,
    actor: &SeatDescriptor,
    caller: &CallerContext,
    call: &Arguments,
) -> GovResult<Value> {
    let store = &service.store;
    let at = system_time_ms()?;
    match (call.family.as_str(), call.leaf.as_str()) {
        ("project", "create") => {
            let base = slug(&call.positions[0])?;
            let taken: BTreeSet<String> = store
                .list_projects()
                .await?
                .into_iter()
                .map(|p| p.slug)
                .collect();
            let mut name = base.clone();
            let mut suffix = 2;
            while taken.contains(&name) {
                name = format!("{base}-{suffix}");
                if name.len() > 48 {
                    return Err(GovernanceError::refused(
                        "E-RS-ARG",
                        "collision suffix exceeds project slug limit",
                    ));
                }
                suffix += 1;
            }
            let repo = match call.value("repo") {
                Some(path) => relative_to(caller, path)?,
                None => caller_directory(caller)?,
            };
            let prime_id = call.value("prime").map(SeatId::from);
            if let Some(prime) = &prime_id {
                required(
                    state.services.registry.get(prime).await?,
                    "prime seat",
                    prime.as_str(),
                )?;
            }
            let project = Project {
                slug: name,
                description: Some(call.positions[0].clone()),
                repo: Some(repo.to_string_lossy().into_owned()),
                plan_path: call.value("plan").map(str::to_string),
                prime_id,
                created_by: actor.id.clone(),
                created_at: at,
            };
            if !store.create_project(&project).await? {
                return Err(GovernanceError::refused(
                    "E-RS-CONFLICT",
                    "project slug was concurrently allocated",
                ));
            }
            let seq = service
                .publish(
                    "project-created",
                    &actor.id,
                    Some(&actor.id),
                    "created",
                    &project,
                    at,
                )
                .await?;
            Ok(json!({"project": project, "seq": seq}))
        }
        ("project", "list") => Ok(json!({"projects": store.list_projects().await?})),
        ("project", "show") => Ok(
            json!({"project": required(store.project(&call.positions[0]).await?, "project", &call.positions[0])?}),
        ),
        ("project", "set") => {
            let prime_id = call.value("prime").map(SeatId::from);
            if let Some(prime) = &prime_id {
                required(
                    state.services.registry.get(prime).await?,
                    "prime seat",
                    prime.as_str(),
                )?;
            }
            let update = ProjectUpdate {
                description: call.value("description").map(str::to_string),
                repo: call
                    .value("repo")
                    .map(|path| relative_to(caller, path).map(|p| p.to_string_lossy().into_owned()))
                    .transpose()?,
                plan_path: call.value("plan").map(|value| Some(value.to_string())),
                prime_id: prime_id.map(Some),
            };
            let project = required(
                store.update_project(&call.positions[0], &update).await?,
                "project",
                &call.positions[0],
            )?;
            let seq = service
                .publish(
                    "project-set",
                    &actor.id,
                    Some(&actor.id),
                    "updated",
                    &project,
                    at,
                )
                .await?;
            Ok(json!({"project": project, "seq": seq}))
        }
        ("stream", "create") => {
            let project = required(
                store.project(call.need("project")?).await?,
                "project",
                call.need("project")?,
            )?;
            let repo = PathBuf::from(project.repo.ok_or_else(|| {
                GovernanceError::refused(
                    "E-RS-ARG",
                    "project has no repository; project set --repo first",
                )
            })?);
            let root = match call.value("root") {
                Some(path) => relative_to(caller, path)?,
                None => repo.clone(),
            };
            let inventory_root = repo.clone();
            let inventory =
                tokio::task::spawn_blocking(move || scan_repo_inventory(&inventory_root))
                    .await
                    .map_err(|e| GovernanceError::internal(e.to_string()))??;
            let plan = match call.value("ordinal") {
                Some(value) => {
                    let ordinal = value.parse::<u32>().map_err(|_| {
                        GovernanceError::refused(
                            "E-RS-ARG",
                            "--ordinal requires a positive integer",
                        )
                    })?;
                    if ordinal == 0 {
                        return Err(GovernanceError::refused(
                            "E-RS-ARG",
                            "--ordinal must be positive",
                        ));
                    }
                    plan_stream_creation_at_ordinal(
                        &project.slug,
                        call.need("slug")?,
                        &root,
                        call.value("base").unwrap_or("main"),
                        &inventory,
                        ordinal,
                    )
                }
                None => plan_stream_creation(
                    &project.slug,
                    call.need("slug")?,
                    &root,
                    call.value("base").unwrap_or("main"),
                    &inventory,
                ),
            }
            .map_err(|error| GovernanceError::refused("E-RS-RESERVATION", format!("{error:?}")))?;
            let id = format!("{}:{}", plan.project, plan.slug);
            match reserve_and_create_stream(&repo, store, &plan, &actor.id, at).await {
                Ok(StreamCreation::Created) => {}
                Ok(StreamCreation::AlreadyReserved) => {
                    return Err(GovernanceError::refused(
                        "E-RS-RESERVATION",
                        format!("{id} is already reserved"),
                    ));
                }
                Err(error) => {
                    return Err(GovernanceError {
                        code: ("E-RS-WORKTREE").into(),
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        message: format!(
                            "{id}: {error}; inspect the persisted reservation with stream show"
                        ),
                        details: json!({"stream": id}),
                    });
                }
            }
            let stream = changed(
                store.set_stream_state(&id, StreamState::Created).await?,
                "stream",
                &id,
            )?;
            let seq = service
                .publish(
                    "allocation",
                    &actor.id,
                    Some(&actor.id),
                    "created",
                    &stream,
                    at,
                )
                .await?;
            Ok(json!({"stream": stream, "seq": seq}))
        }
        ("stream", "list") => {
            Ok(json!({"streams": store.list_streams(call.value("project")).await?}))
        }
        ("stream", "show") => Ok(
            json!({"stream": required(store.stream(&call.positions[0]).await?, "stream", &call.positions[0])?}),
        ),
        ("stream", "close") => {
            let stream = changed(
                store
                    .set_stream_state(&call.positions[0], StreamState::Closed)
                    .await?,
                "stream",
                &call.positions[0],
            )?;
            let seq = service
                .published_or_repair(
                    "allocation",
                    &actor.id,
                    Some(&actor.id),
                    "closed",
                    &stream,
                    at,
                )
                .await?;
            Ok(json!({"stream": stream, "seq": seq}))
        }
        ("fence", "set") => {
            required(
                store.stream(&call.positions[0]).await?,
                "stream",
                &call.positions[0],
            )?;
            let paths = csv(Some(call.need("paths")?));
            if paths.is_empty() {
                return Err(GovernanceError::refused(
                    "E-RS-ARG",
                    "--paths needs a non-empty path list",
                ));
            }
            for pattern in &paths {
                compile_path_pattern(pattern)?;
            }
            let id = match store.list_fences(Some(&call.positions[0])).await?.first() {
                Some(existing) => existing.id.clone(),
                None => mint("fence")?,
            };
            let fence = Fence {
                id,
                stream: call.positions[0].clone(),
                paths,
                shared: csv(call.value("shared")),
                declared_by: actor.id.clone(),
                declared_at: at,
            };
            let fence = store.set_fence(&fence).await?;
            let seq = service
                .publish("fence", &actor.id, Some(&actor.id), "declared", &fence, at)
                .await?;
            Ok(json!({"fence": fence, "seq": seq}))
        }
        ("fence", "show") => {
            let mut fences = store.list_fences(call.value("stream")).await?;
            if let Some(path) = call.value("path") {
                let mut matched = Vec::new();
                for fence in fences {
                    if fence_matches_path(&fence, path)? {
                        matched.push(fence);
                    }
                }
                fences = matched;
            }
            Ok(json!({"fences": fences}))
        }
        ("ack", "") => {
            let id = &call.positions[0];
            match store
                .acknowledge_dispatch(id, &actor.id, call.need("packet-sha")?, at)
                .await?
            {
                DispatchAck::Acknowledged | DispatchAck::AlreadyAcknowledged => {}
                DispatchAck::Missing => {
                    return Err(GovernanceError::refused(
                        "E-RS-NOT-FOUND",
                        format!("dispatch {id} does not exist"),
                    ));
                }
                DispatchAck::ShaMismatch => {
                    return Err(GovernanceError {
                        code: ("E-RS-PACKET-SHA").into(),
                        status: StatusCode::BAD_REQUEST,
                        message: "ack: packet digest does not match dispatch".to_string(),
                        details: json!({"dispatch": id}),
                    });
                }
                DispatchAck::NotAssignee { assignee } => {
                    return Err(GovernanceError {
                        code: ("E-RS-OWNERSHIP").into(),
                        status: StatusCode::FORBIDDEN,
                        message: "only the dispatch recipient may acknowledge".to_string(),
                        details: json!({"dispatch": id, "caller": actor.id, "recipient": assignee}),
                    });
                }
            }
            let dispatch = required(store.dispatch(id).await?, "dispatch", id)?;
            let seq = publish_ack_once(service, &dispatch).await?;
            Ok(json!({"dispatch": dispatch, "seq": seq}))
        }
        ("attest", "") => {
            let target = target_seat(state, &call.positions[0]).await?;
            let attestation = store
                .put_plan_attestation(&PlanAttestation {
                    seat: target.id,
                    plan_id: call.need("plan-id")?.to_string(),
                    attested_by: actor.id.clone(),
                    attested_at: at,
                })
                .await?;
            let seq = service
                .publish(
                    "attest.plan",
                    &actor.id,
                    Some(&attestation.seat),
                    "attested",
                    &attestation,
                    at,
                )
                .await?;
            Ok(json!({"attestation": attestation, "seq": seq}))
        }
        ("task", "set") => {
            let target = target_seat(state, &call.positions[0]).await?;
            if call.positions[1].trim().is_empty() {
                return Err(GovernanceError::refused("E-RS-ARG", "task text is empty"));
            }
            if let Some(project) = call.value("project") {
                required(store.project(project).await?, "project", project)?;
            }
            let task = TaskAssignment {
                id: mint("assignment")?,
                node_id: target.id,
                task: call.positions[1].clone(),
                project: call.value("project").map(str::to_string),
                opened_by: actor.id.clone(),
                opened_at: at,
                closed_at: None,
                close_reason: None,
            };
            if !store.open_task(&task).await? {
                return Err(GovernanceError::refused(
                    "E-RS-CONFLICT",
                    "assignment id collision",
                ));
            }
            let seq = service
                .publish(
                    "task-set",
                    &actor.id,
                    Some(&task.node_id),
                    "opened",
                    &task,
                    at,
                )
                .await?;
            Ok(json!({"task": task, "seq": seq}))
        }
        ("task", "close") => {
            let task = required(
                store.task(&call.positions[0]).await?,
                "task",
                &call.positions[0],
            )?;
            target_seat(state, task.node_id.as_str()).await?;
            let reason = match call.need("reason")? {
                "done" => TaskCloseReason::Done,
                "cancelled" => TaskCloseReason::Cancelled,
                "failed" => TaskCloseReason::Failed,
                "superseded" => TaskCloseReason::Superseded,
                _ => {
                    return Err(GovernanceError::refused(
                        "E-RS-ARG",
                        "task close reason must be done|cancelled|failed|superseded",
                    ));
                }
            };
            let task = changed(
                store.close_task(&task.id, reason, at).await?,
                "task",
                &task.id,
            )?;
            let seq = service
                .published_or_repair(
                    "task-close",
                    &actor.id,
                    Some(&task.node_id),
                    "closed",
                    &task,
                    at,
                )
                .await?;
            Ok(json!({"task": task, "seq": seq}))
        }
        ("node", "show") => node_show(service, state, &call.positions[0]).await,
        ("orchestration", _) => {
            orchestration_command(service, state, actor, caller, call, at).await
        }
        ("spine", _) => spine_command(service, actor, call, at).await,
        _ => Err(GovernanceError::refused(
            "E-RS-ARG",
            "invalid governance invocation",
        )),
    }
}

async fn orchestration_command(
    service: &GovernanceService,
    state: &AppState,
    actor: &SeatDescriptor,
    caller: &CallerContext,
    call: &Arguments,
    at: u64,
) -> GovResult<Value> {
    let store = &service.store;
    let name = call.positions.first().map(String::as_str).unwrap_or("");
    match call.leaf.as_str() {
        "baton.define" => {
            let resource = call.need("resource")?;
            let repo = match call.value("repo") {
                Some(repo) => relative_to(caller, repo)?,
                None => caller_directory(caller)?,
            };
            let baton = BatonDefinition {
                name: name.to_string(),
                description: resource.to_string(),
                resource: Some(resource.to_string()),
                probe: call.value("probe").map(str::to_string),
                repo: Some(repo.to_string_lossy().into_owned()),
                created_by: actor.id.clone(),
                created_at: at,
            };
            if !store.define_baton(&baton).await? {
                return Err(GovernanceError::refused(
                    "E-RS-CONFLICT",
                    format!("baton {name} already exists"),
                ));
            }
            let seq = service
                .publish(
                    "baton.defined",
                    &actor.id,
                    Some(&actor.id),
                    "defined",
                    &baton,
                    at,
                )
                .await?;
            Ok(json!({"baton": baton, "seq": seq}))
        }
        "baton.list" => Ok(json!({"batons": store.list_batons().await?})),
        "baton.show" => Ok(
            json!({"baton": required(store.baton(name).await?, "baton", name)?,
            "lease": store.lease(name).await?, "requests": store.list_baton_requests(name).await?}),
        ),
        "baton.request" => {
            let baton = required(store.baton(name).await?, "baton", name)?;
            let request = BatonRequest {
                id: mint("request")?,
                baton: name.to_string(),
                requester: actor.id.clone(),
                purpose: call.need("purpose")?.to_string(),
                pin: call.value("pin").map(str::to_string),
                evidence: call.value("evidence").map(str::to_string),
                requested_at: at,
                state: BatonRequestState::Requested,
            };
            let request = changed(store.request_baton(&request).await?, "request", &request.id)?;
            let seq = service
                .publish(
                    "baton.requested",
                    &actor.id,
                    Some(&actor.id),
                    "requested",
                    &request,
                    at,
                )
                .await?;
            // Publication has completed before entering the existing sender.
            // A notice failure must still expose the committed request and seq.
            let notice = notify_baton_keeper(state, &baton, &request).await.map_err(|error| {
                GovernanceError {
                    code: "E-RS-PARTIAL".into(),
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: format!("request committed but keeper notice failed: {error}"),
                    details: json!({"committed": true, "event_published": true, "request": request, "seq": seq}),
                }
            })?;
            Ok(json!({"request": request, "seq": seq, "notice": notice}))
        }
        "baton.grant" => {
            let baton = required(store.baton(name).await?, "baton", name)?;
            let request = required(
                store.baton_request(call.need("to")?).await?,
                "request",
                call.need("to")?,
            )?;
            if request.baton != name {
                return Err(GovernanceError::refused(
                    "E-RS-ARG",
                    "request belongs to a different baton",
                ));
            }
            if let Some(pin) = &request.pin {
                let head = match baton.repo.as_deref() {
                    Some(repo) => current_head(Path::new(repo)).await?,
                    None => None,
                };
                if !call.flags.contains_key("repin")
                    && head.as_deref().is_none_or(|head| !same_commit(pin, head))
                {
                    return Err(GovernanceError { code: ("E-RS-PIN").into(), status: StatusCode::CONFLICT,
                        message: "baton pin does not match observable HEAD; --repin explicitly acknowledges the mismatch".to_string(),
                        details: json!({"baton": name, "request": request.id, "pin": pin, "head": head}) });
                }
            }
            let lease = changed(
                store
                    .grant_baton(name, &request.id, &mint("lease")?, at)
                    .await?,
                "baton",
                name,
            )?;
            let seq = service
                .published_or_repair(
                    "baton.granted",
                    &actor.id,
                    Some(&actor.id),
                    "granted",
                    &lease,
                    at,
                )
                .await?;
            Ok(json!({"lease": lease, "seq": seq}))
        }
        "baton.return" | "baton.reclaim" => {
            let lease = store.lease(name).await?;
            let expected = call.value("lease-id");
            if expected.is_none()
                || lease
                    .as_ref()
                    .is_none_or(|lease| Some(lease.lease_id.as_str()) != expected)
            {
                return Err(lease_stale(
                    name,
                    lease.as_ref().map(|lease| lease.lease_id.as_str()),
                    expected,
                ));
            }
            let lease = lease.expect("matching token proved a current lease");
            let reclaim = call.leaf == "baton.reclaim";
            if !reclaim && lease.holder != actor.id {
                return Err(GovernanceError {
                    code: "E-RS-OWNERSHIP".into(),
                    status: StatusCode::FORBIDDEN,
                    message:
                        "only the holder may return a lease; explicit reclaim requires evidence"
                            .to_string(),
                    details: json!({"caller": actor.id, "holder": lease.holder, "baton": name}),
                });
            }
            let owned_store: SqliteOrchestration = store.clone();
            let owned_name = name.to_string();
            let expected = expected.expect("token checked").to_string();
            let evidence = if reclaim {
                call.need("evidence")?
            } else {
                call.value("evidence").unwrap_or("")
            }
            .to_string();
            let owned_actor = actor.id.clone();
            let committed = service
                .event_bus
                .publish_committed(async move {
                    if reclaim {
                        owned_store
                            .reclaim_baton(&owned_name, &expected, &owned_actor, &evidence, at)
                            .await
                    } else {
                        owned_store
                            .return_baton(&owned_name, &owned_actor, &expected, &evidence, at)
                            .await
                    }
                })
                .await;
            let (seq, _) = match committed {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = GovernanceError::from(error);
                    if error.code == "E-RS-LEASE-STALE" {
                        return Err(lease_stale(
                            name,
                            error.details["current_lease"].as_str(),
                            call.value("lease-id"),
                        ));
                    }
                    return Err(error);
                }
            };
            Ok(json!({"baton": name, "released": true, "seq": seq}))
        }
        "prime.set" => {
            target_seat(state, name).await?;
            let prime = PrimeDesignation {
                seat: name.into(),
                designated_by: actor.id.clone(),
                designated_at: at,
                state: PrimeState::Current,
            };
            let outcome = store.designate_prime(&prime).await?;
            let newly_designated = matches!(&outcome, GovernanceOutcome::Changed(_));
            let prime = changed(outcome, "prime", name)?;
            let seq = if newly_designated {
                service
                    .publish(
                        "prime-set",
                        &actor.id,
                        Some(&prime.seat),
                        "designated",
                        &prime,
                        at,
                    )
                    .await?
            } else {
                service
                    .published_or_repair(
                        "prime-set",
                        &actor.id,
                        Some(&prime.seat),
                        "designated",
                        &prime,
                        at,
                    )
                    .await?
            };
            Ok(json!({"prime": prime, "seq": seq}))
        }
        "prime.unset" => {
            target_seat(state, name).await?;
            let owned_store: SqliteOrchestration = store.clone();
            let seat = SeatId::from(name);
            let actor = actor.id.clone();
            let (seq, _) = service
                .event_bus
                .publish_committed(async move { owned_store.unset_prime(&seat, &actor, at).await })
                .await?;
            Ok(json!({"prime": null, "seq": seq}))
        }
        "prime.retire" => {
            target_seat(state, name).await?;
            let outcome = store.retire_prime(&SeatId::from(name)).await?;
            let new_transition = matches!(&outcome, GovernanceOutcome::Changed(_));
            let prime = changed(outcome, "prime", name)?;
            let seq = if new_transition {
                service
                    .publish(
                        "prime-set",
                        &actor.id,
                        Some(&prime.seat),
                        "retired",
                        &prime,
                        at,
                    )
                    .await?
            } else {
                service
                    .published_or_repair(
                        "prime-set",
                        &actor.id,
                        Some(&prime.seat),
                        "retired",
                        &prime,
                        at,
                    )
                    .await?
            };
            Ok(json!({"prime": prime, "seq": seq}))
        }
        _ => Err(GovernanceError::refused(
            "E-RS-ARG",
            "unknown orchestration leaf",
        )),
    }
}

async fn notify_baton_keeper(
    state: &AppState,
    baton: &BatonDefinition,
    request: &BatonRequest,
) -> Result<Option<&'static str>> {
    let Some(keeper) = state.services.registry.get(&baton.created_by).await? else {
        return Ok(None);
    };
    if keeper.tombstoned_at.is_some() {
        return Ok(None);
    }
    let receipt = state
        .services
        .delivery
        .send(
            request.requester.clone(),
            keeper.id.clone(),
            format!(
                "[pij orchestration] baton '{}' requested by {}: {} (request {})",
                baton.name, request.requester, request.purpose, request.id
            ),
        )
        .await?;
    let outcome = match receipt.outcome {
        DeliveryOutcome::Delivered { .. } => "delivered",
        DeliveryOutcome::Queued { .. } => "queued",
        DeliveryOutcome::Held { reason } => {
            return Err(adapter(format!("keeper notice held: {reason}")));
        }
        DeliveryOutcome::Refused { reason } => {
            return Err(adapter(format!("keeper notice refused: {reason}")));
        }
    };
    // Reuse process-incarnation evidence; a heartbeat age is not a native
    // liveness verdict, and admission is never delivery.
    let unverified = match keeper.proc {
        Some(proc) => matches!(
            alive(proc, state.services.liveness.as_ref()).await?,
            Liveness::Dead { .. } | Liveness::Recycled { .. }
        ),
        None => false,
    };
    Ok(Some(if unverified { "unverified" } else { outcome }))
}

fn lease_stale(name: &str, current: Option<&str>, provided: Option<&str>) -> GovernanceError {
    let fix = format!("pij orchestration baton show {name}");
    GovernanceError {
        code: "E-RS-LEASE-STALE".into(),
        status: StatusCode::CONFLICT,
        message: format!(
            "baton {name} requires its observed lease token; current lease is {}; run `{fix}` then pass --lease-id on return/reclaim",
            current.unwrap_or("none")
        ),
        details: json!({"baton": name, "supplied_lease": provided, "current_lease": current, "fix": fix}),
    }
}

async fn current_head(repo: &Path) -> GovResult<Option<String>> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .await
        .map_err(|error| {
            GovernanceError::internal(format!("cannot observe repository HEAD: {error}"))
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let head =
        String::from_utf8(output.stdout).map_err(|e| GovernanceError::internal(e.to_string()))?;
    Ok(Some(head.trim().to_string()))
}

fn same_commit(left: &str, right: &str) -> bool {
    let a = left.trim().to_ascii_lowercase();
    let b = right.trim().to_ascii_lowercase();
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    short.len() >= 4
        && short.bytes().all(|b| b.is_ascii_hexdigit())
        && long.bytes().all(|b| b.is_ascii_hexdigit())
        && long.starts_with(short)
}

async fn spine_command(
    service: &GovernanceService,
    actor: &SeatDescriptor,
    call: &Arguments,
    at: u64,
) -> GovResult<Value> {
    if call.leaf == "append" {
        let mut payload = json!({"actor": actor.id, "refs": csv(call.value("refs"))});
        if let Some(project) = call.value("project") {
            payload["project"] = json!(project);
        }
        let event = Event {
            seq: None,
            v: 1,
            at,
            kind: call.need("kind")?.to_string(),
            seat: Some(actor.id.clone()),
            payload: serde_json::to_string(&payload)
                .map_err(|e| GovernanceError::internal(e.to_string()))?,
        };
        let seq = service.event_bus.publish(event.clone()).await?;
        return Ok(json!({"seq": seq, "event": event}));
    }
    let since = call
        .value("since")
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| {
            GovernanceError::refused("E-RS-ARG", "--since requires an unsigned local sequence")
        })?
        .unwrap_or(0);
    let newest = service
        .event_bus
        .tail(None, Seq(0))
        .await?
        .last()
        .and_then(|event| event.seq)
        .unwrap_or(Seq(0));
    if since > newest.0 {
        return Err(GovernanceError::refused(
            "E-RS-CURSOR",
            "--since is beyond the local spine",
        ));
    }
    let rows = service.event_bus.tail(None, Seq(since)).await?;
    let mut selected = Vec::new();
    for event in rows {
        if call
            .value("peer")
            .is_some_and(|peer| event.seat.as_ref().is_none_or(|seat| seat.as_str() != peer))
        {
            continue;
        }
        if let Some(project) = call.value("project") {
            let payload: Value = serde_json::from_str(&event.payload)
                .map_err(|e| GovernanceError::internal(e.to_string()))?;
            let project_ref = format!("project:{project}");
            let references = payload["refs"].as_array().is_some_and(|refs| {
                refs.iter()
                    .any(|reference| reference.as_str() == Some(project_ref.as_str()))
            });
            let matches = payload["project"] == project
                || payload["record"]["project"] == project
                || (matches!(event.kind.as_str(), "project-created" | "project-set")
                    && payload["record"]["slug"] == project)
                || references;
            if !matches {
                continue;
            }
        }
        selected.push(event);
    }
    if call.leaf == "render" {
        let text = selected
            .iter()
            .map(|event| {
                format!(
                    "{} {} {}",
                    event.seq.map_or(0, |seq| seq.0),
                    event.kind,
                    event.seat.as_ref().map_or("-", SeatId::as_str)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(json!({"text": text, "cursor": newest}));
    }
    let events: Vec<Value> = selected
        .into_iter()
        .map(|event| {
            let mut row = serde_json::to_value(&event)
                .map_err(|e| GovernanceError::internal(e.to_string()))?;
            row["seq"] = json!(
                event
                    .seq
                    .ok_or_else(|| GovernanceError::internal("durable event has no sequence"))?
            );
            Ok(row)
        })
        .collect::<GovResult<_>>()?;
    Ok(json!({"events": events, "cursor": newest}))
}

#[derive(Serialize)]
struct NodeView<'a> {
    id: &'a SeatId,
    parent: Option<&'a SeatId>,
    role: Option<&'a str>,
    plan_id: Option<&'a str>,
    children: Vec<NodeView<'a>>,
    assignments: Vec<&'a TaskAssignment>,
    dispatches: Vec<&'a Dispatch>,
}

async fn node_show(service: &GovernanceService, state: &AppState, id: &str) -> GovResult<Value> {
    let seats = state.services.registry.list(SeatFilter::default()).await?;
    let target = required(seats.iter().find(|seat| seat.id.as_str() == id), "seat", id)?;
    let roles = service.store.list_roles().await?;
    let attestations = service.store.list_plan_attestations().await?;
    let tasks = service.store.list_tasks(None).await?;
    let dispatches = service.store.list_dispatches(None).await?;
    let mut tree = NodeIndex {
        children: BTreeMap::new(),
        roles: BTreeMap::new(),
        plans: BTreeMap::new(),
        tasks: BTreeMap::new(),
        dispatches: BTreeMap::new(),
    };
    for seat in &seats {
        if let Some(parent) = &seat.parent {
            tree.children.entry(parent.as_str()).or_default().push(seat);
        }
    }
    for children in tree.children.values_mut() {
        children.sort_by_key(|seat| seat.id.as_str());
    }
    for role in &roles {
        tree.roles.insert(role.seat.as_str(), role.role.as_str());
    }
    for plan in &attestations {
        tree.plans.insert(plan.seat.as_str(), plan.plan_id.as_str());
    }
    for task in &tasks {
        tree.tasks
            .entry(task.node_id.as_str())
            .or_default()
            .push(task);
    }
    for dispatch in &dispatches {
        tree.dispatches
            .entry(dispatch.to.as_str())
            .or_default()
            .push(dispatch);
    }
    let mut node = serde_json::to_value(tree.build(target, &mut BTreeSet::new())?)
        .map_err(|error| adapter(error.to_string()))?;
    let reports = ReportService::new(
        state.services.registry.as_ref(),
        state.services.spine.as_ref(),
        || 0,
        ReportConfig::default(),
    );
    node["state"] = serde_json::to_value(reports.latest_state_record(&target.id).await?)
        .map_err(|error| adapter(error.to_string()))?;
    let cursor = service
        .event_bus
        .tail(None, Seq(0))
        .await?
        .last()
        .and_then(|event| event.seq)
        .unwrap_or(Seq(0));
    Ok(json!({"node": node, "cursor": cursor}))
}

struct NodeIndex<'a> {
    children: BTreeMap<&'a str, Vec<&'a SeatDescriptor>>,
    roles: BTreeMap<&'a str, &'a str>,
    plans: BTreeMap<&'a str, &'a str>,
    tasks: BTreeMap<&'a str, Vec<&'a TaskAssignment>>,
    dispatches: BTreeMap<&'a str, Vec<&'a Dispatch>>,
}

impl<'a> NodeIndex<'a> {
    fn build(
        &self,
        seat: &'a SeatDescriptor,
        path: &mut BTreeSet<&'a str>,
    ) -> GovResult<NodeView<'a>> {
        if !path.insert(seat.id.as_str()) {
            return Err(GovernanceError::refused(
                "E-RS-CYCLE",
                "rs parent rows contain a cycle",
            ));
        }
        let children = self
            .children
            .get(seat.id.as_str())
            .into_iter()
            .flatten()
            .map(|child| self.build(child, path))
            .collect::<GovResult<Vec<_>>>()?;
        path.remove(seat.id.as_str());
        Ok(NodeView {
            id: &seat.id,
            parent: seat.parent.as_ref(),
            role: self.roles.get(seat.id.as_str()).copied(),
            plan_id: self.plans.get(seat.id.as_str()).copied(),
            children,
            assignments: self
                .tasks
                .get(seat.id.as_str())
                .cloned()
                .unwrap_or_default(),
            dispatches: self
                .dispatches
                .get(seat.id.as_str())
                .cloned()
                .unwrap_or_default(),
        })
    }
}

async fn dispatch_command(
    service: &GovernanceService,
    state: &AppState,
    actor: &SeatDescriptor,
    caller: &CallerContext,
    call: &Arguments,
) -> GovResult<Value> {
    let wait = call.wait(0)?;
    let target = target_seat(state, &call.positions[0]).await?;
    let packet = relative_to(caller, call.need("packet")?)?;
    let bytes = tokio::fs::read(&packet).await.map_err(|error| {
        GovernanceError::refused(
            "E-RS-PACKET",
            format!("cannot read {}: {error}", packet.display()),
        )
    })?;
    let (dispatch, seq) = send_packet(service, state, actor, &target, packet, &bytes).await?;
    let dispatch = wait_dispatch(service, &dispatch.id, wait, false).await?;
    Ok(json!({"dispatch": dispatch, "seq": seq}))
}

async fn send_packet(
    service: &GovernanceService,
    state: &AppState,
    actor: &SeatDescriptor,
    target: &SeatDescriptor,
    packet: PathBuf,
    bytes: &[u8],
) -> GovResult<(Dispatch, u64)> {
    let digest = format!("{:x}", Sha256::digest(bytes));
    let id = mint("dispatch")?;
    let at = system_time_ms()?;
    let dispatch = Dispatch {
        id: id.clone(),
        from: actor.id.clone(),
        to: target.id.clone(),
        packet_path: packet.to_string_lossy().into_owned(),
        packet_sha256: Some(digest.clone()),
        msg_id: Some(id.clone()),
        state: DispatchState::Queued,
        created_at: at,
        delivered_at: None,
        acknowledged_at: None,
        ack: None,
        canary: None,
    };
    let seq = {
        let _guard = service.mutation.lock().await;
        if !service.store.create_dispatch(&dispatch).await? {
            return Err(GovernanceError::refused(
                "E-RS-CONFLICT",
                "dispatch id collision",
            ));
        }
        service
            .publish(
                "dispatch",
                &actor.id,
                Some(&target.id),
                "queued",
                &dispatch,
                at,
            )
            .await?
    };
    let body = format!(
        "Read packet {}. Dispatch {} binds these exact packet bytes (SHA-256 {}). Acknowledge as yourself via pij ack {} --packet-sha {} after reading. Do not substitute another dispatch or digest.",
        dispatch.packet_path, id, digest, id, digest,
    );
    let receipt = state
        .services
        .delivery
        .accept(Msg {
            from: actor.id.clone(),
            to: target.id.clone(),
            body,
            msg_id: id.clone(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        })
        .await
        .map_err(|error| GovernanceError {
            code: ("E-RS-PARTIAL").into(),
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("dispatch {id} persisted but delivery failed: {error}"),
            details: json!({"dispatch": id, "committed": true, "delivery_accepted": false}),
        })?;
    if matches!(&receipt.outcome, DeliveryOutcome::Refused { .. }) {
        return Err(GovernanceError {
            code: ("E-RS-DELIVERY").into(),
            status: StatusCode::BAD_REQUEST,
            message: format!("dispatch {id} delivery was refused"),
            details: json!({"dispatch": id, "receipt": receipt}),
        });
    }
    if matches!(&receipt.outcome, DeliveryOutcome::Delivered { .. }) {
        let _guard = service.mutation.lock().await;
        let event = service
            .event_bus
            .latest_matching_message(&target.id, "delivery.outcome", &id)
            .await?
            .ok_or_else(|| {
                GovernanceError::internal("delivered receipt has no durable delivery.outcome event")
            })?;
        service.observe_delivery(&event).await?;
    }
    Ok((dispatch, seq))
}

async fn wait_dispatch(
    service: &GovernanceService,
    id: &str,
    wait: Duration,
    ack_only: bool,
) -> GovResult<Dispatch> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let row = required(service.store.dispatch(id).await?, "dispatch", id)?;
        if row.state == DispatchState::Acked
            || (!ack_only && row.state == DispatchState::Delivered)
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(row);
        }
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(25)).min(deadline),
        )
        .await;
    }
}

async fn canary_command(
    service: &GovernanceService,
    state: &AppState,
    actor: &SeatDescriptor,
    call: &Arguments,
) -> GovResult<Value> {
    let wait = call.wait(5_000)?;
    let target = target_seat(state, &call.positions[0]).await?;
    if target.tombstoned_at.is_some() {
        return Err(GovernanceError::refused(
            "E-RS-TOMBSTONED",
            "a retired recipient cannot pass a canary",
        ));
    }
    let nonce = mint("nonce")?;
    let packet = service.packet_dir.join(format!("{nonce}.json"));
    let bytes = serde_json::to_vec(&json!({"kind": "pij-canary", "nonce": nonce,
        "recipient": target.id, "expected_model": call.value("expect-model"),
        "process": target.proc, "session": target.harness_session,
        "instruction": "Read this exact nonce-bearing packet and acknowledge its dispatch via the pushed pij ack instruction; do not acknowledge another packet."}))
        .map_err(|error| GovernanceError::internal(error.to_string()))?;
    tokio::fs::create_dir_all(&service.packet_dir)
        .await
        .map_err(|e| GovernanceError::internal(e.to_string()))?;
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&packet)
        .await
        .map_err(|e| GovernanceError::internal(e.to_string()))?;
    file.write_all(&bytes)
        .await
        .map_err(|e| GovernanceError::internal(e.to_string()))?;
    file.sync_all()
        .await
        .map_err(|e| GovernanceError::internal(e.to_string()))?;
    let (dispatch, _) = send_packet(service, state, actor, &target, packet, &bytes).await?;
    let dispatch = wait_dispatch(service, &dispatch.id, wait, true).await?;
    if dispatch.state != DispatchState::Acked {
        return Err(GovernanceError {
            code: ("E-RS-CANARY-PENDING").into(),
            status: StatusCode::CONFLICT,
            message: "no nonce-correlated recipient acknowledgement arrived before the deadline"
                .to_string(),
            details: json!({"dispatch": dispatch.id, "state": dispatch.state, "nonce": nonce}),
        });
    }
    let current = target_seat(state, target.id.as_str()).await?;
    if current.tombstoned_at.is_some() {
        return Err(GovernanceError {
            code: "E-RS-CANARY".into(),
            status: StatusCode::CONFLICT,
            message: "recipient retired during the challenge".into(),
            details: json!({"dispatch": dispatch.id, "seat": current.id}),
        });
    }
    let evidence = json!({"dispatch": dispatch.id, "nonce": nonce, "process": current.proc,
        "session": current.harness_session, "pane": current.pane});
    let refuse = |message: &str| GovernanceError {
        code: ("E-RS-CANARY").into(),
        status: StatusCode::CONFLICT,
        message: message.to_string(),
        details: evidence.clone(),
    };
    if current.proc != target.proc
        || current.harness_session != target.harness_session
        || current.pane != target.pane
    {
        return Err(refuse("recipient incarnation changed during the challenge"));
    }
    let process = current
        .proc
        .ok_or_else(|| refuse("recipient has no verified process identity"))?;
    if process.pid == 0
        || state.services.liveness.proc_start(process.pid).await? != Some(process.proc_start)
    {
        return Err(refuse("recorded recipient process is absent or recycled"));
    }
    let pane = current
        .pane
        .as_deref()
        .filter(|pane| !pane.is_empty())
        .ok_or_else(|| refuse("recipient has no pane"))?;
    let pane_process = state
        .services
        .tmux
        .pane_process(pane)
        .await?
        .ok_or_else(|| refuse("recipient pane is absent"))?;
    if pane_process.pid != process.pid {
        let identity =
            super::pane_harness_identity(state, pane_process.pid, current.harness).await?;
        if identity.is_none_or(|identity| !identity.subtree.contains(&process)) {
            return Err(refuse(
                "recorded recipient process is not in the current pane's harness subtree",
            ));
        }
    }
    if current.harness_session.as_deref().is_none_or(str::is_empty) {
        return Err(refuse("recipient native session is not bound"));
    }
    let observation = state
        .services
        .harnesses
        .observe_bind(&current, current.harness_session.clone(), None)
        .await?;
    let observed_model = observation
        .facts
        .model
        .as_deref()
        .filter(|model| !model.is_empty())
        .ok_or_else(|| refuse("runtime model could not be observed"))?;
    if observation.facts.pane.as_deref() != Some(pane)
        || observation.facts.native_session_id != current.harness_session
    {
        return Err(refuse(
            "fresh harness observation does not match the challenged session/pane",
        ));
    }
    if !crate::registration::requested_model_matches(
        call.value("expect-model"),
        Some(observed_model),
        true,
    ) || !crate::registration::requested_model_matches(
        current.model.as_deref(),
        Some(observed_model),
        true,
    ) {
        let mut error = refuse("fresh runtime model disagrees with the expected/bound model");
        error.details["observed_model"] = json!(observed_model);
        error.details["expected_model"] = json!(call.value("expect-model"));
        return Err(error);
    }
    let _guard = service.mutation.lock().await;
    let at = system_time_ms()?;
    let canary = DispatchCanary {
        nonce,
        model: observed_model.to_string(),
        passed_at: at,
        evaluator: actor.id.clone(),
    };
    let dispatch = changed(
        service
            .store
            .set_dispatch_canary(&dispatch.id, &canary)
            .await?,
        "dispatch",
        &dispatch.id,
    )?;
    let seq = service
        .publish(
            "dispatch",
            &actor.id,
            Some(&dispatch.to),
            "canary-passed",
            &dispatch,
            at,
        )
        .await?;
    Ok(json!({"dispatch": dispatch, "seq": seq}))
}

async fn publish_ack_once(service: &GovernanceService, dispatch: &Dispatch) -> GovResult<u64> {
    let ack = dispatch.ack.as_ref().ok_or_else(|| {
        GovernanceError::internal("acked dispatch lacks acknowledgement evidence")
    })?;
    let evidence =
        serde_json::to_value(ack).map_err(|error| GovernanceError::internal(error.to_string()))?;
    for event in service
        .event_bus
        .tail(Some(&dispatch.to), Seq(0))
        .await?
        .into_iter()
        .rev()
    {
        if event.kind != "dispatch" {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<Value>(&event.payload) else {
            continue;
        };
        if payload["action"] == "acked"
            && payload["record"]["id"] == dispatch.id
            && payload["record"]["ack"] == evidence
        {
            return event.seq.map(|seq| seq.0).ok_or_else(|| {
                GovernanceError::internal("durable acknowledgement event has no sequence")
            });
        }
    }
    service
        .publish(
            "dispatch",
            &ack.seat,
            Some(&dispatch.to),
            "acked",
            dispatch,
            ack.at,
        )
        .await
}

fn compile_path_pattern(pattern: &str) -> GovResult<globset::GlobMatcher> {
    let normalized = pattern.replace('\\', "/");
    let unsupported = uses_extglob_or_negation(&normalized);
    if unsupported {
        return Err(GovernanceError {
            code: ("E-RS-FENCE-PATTERN").into(),
            status: StatusCode::BAD_REQUEST,
            message: format!("unsupported extglob or negation in fence pattern {pattern:?}"),
            details: json!({"pattern": pattern}),
        });
    }
    globset::GlobBuilder::new(&normalized)
        .literal_separator(true)
        .backslash_escape(false)
        .empty_alternates(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| GovernanceError {
            code: ("E-RS-FENCE-PATTERN").into(),
            status: StatusCode::BAD_REQUEST,
            message: format!("invalid fence pattern {pattern:?}: {error}"),
            details: json!({"pattern": pattern}),
        })
}

fn uses_extglob_or_negation(pattern: &str) -> bool {
    if pattern.starts_with('!') {
        return true;
    }
    let mut chars = pattern.chars().peekable();
    let mut class = false;
    let mut class_has_value = false;
    while let Some(ch) = chars.next() {
        if class {
            if ch == ']' && class_has_value {
                class = false;
            } else if class_has_value || !matches!(ch, '!' | '^') {
                class_has_value = true;
            }
            continue;
        }
        if ch == '[' {
            class = true;
            class_has_value = false;
            continue;
        }
        if matches!(ch, '@' | '!' | '?' | '*' | '+') && chars.peek() == Some(&'(') {
            return true;
        }
    }
    false
}

fn fence_matches_path(fence: &Fence, path: &str) -> GovResult<bool> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized
        .strip_prefix("./")
        .map_or(normalized.as_str(), |path| path.trim_start_matches('/'));
    for pattern in &fence.paths {
        if compile_path_pattern(pattern)?.is_match(normalized) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use pij_core::model::DeliveryOrigin;
    use pij_store::SqliteSpine;
    use pij_testkit::FreshStore;

    use super::*;

    fn routes() -> Value {
        serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("routes")
    }

    fn event_record(id: &str) -> Value {
        let events: Value = serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-events.json"
        ))
        .expect("events");
        events["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|case| case["id"] == id)
            .expect("fixture")["decoded_payload"]["record"]
            .clone()
    }

    #[test]
    fn every_canonical_governance_argv_has_one_parser_and_unknown_tokens_refuse() {
        for route in routes()["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|route| route["owner"] == "u3")
        {
            for case in route["cases"].as_array().expect("cases") {
                let argv: Vec<String> =
                    serde_json::from_value(case["request"]["argv"].clone()).expect("argv");
                assert!(Arguments::parse(&argv).is_ok(), "{}", case["id"]);
                let mut unknown = argv.clone();
                unknown.extend(["--discard-this".to_string(), "never".to_string()]);
                assert!(
                    Arguments::parse(&unknown).is_err(),
                    "{} must not silently drop a flag",
                    case["id"]
                );
                let mut extra = argv;
                extra.push("unexpected-positional".to_string());
                assert!(
                    Arguments::parse(&extra).is_err(),
                    "{} must not silently drop a positional",
                    case["id"]
                );
            }
        }
    }

    #[test]
    fn fences_match_owned_patterns_not_shared_metadata_and_refuse_unsupported_globs() {
        let mut fence: Fence = serde_json::from_value(event_record("fence-set")).expect("fence");
        let shared = fence.shared[0].clone();
        assert!(fence_matches_path(&fence, &shared).unwrap_or(false));
        fence.paths.clear();
        assert!(
            !fence_matches_path(&fence, &shared).unwrap_or(true),
            "shared metadata is not a touch-set grant"
        );
        for pattern in ["**/{*.rs,*.ts}", "src/file?.[rt]s", "**/.hidden"] {
            assert!(compile_path_pattern(pattern).is_ok(), "{pattern}");
        }
        for pattern in ["!(secret)", "src/@(a|b)", "src/+(a)", "!hidden"] {
            let error = compile_path_pattern(pattern)
                .err()
                .unwrap_or_else(|| panic!("expected unsupported pattern refusal for {pattern:?}"));
            assert_eq!(error.code, "E-RS-FENCE-PATTERN");
            assert_eq!(error.details["pattern"], pattern);
        }
        // globset 0.4.20 supports nested alternates despite stale crate prose.
        // These results were measured against the existing picomatch(dot:true).
        let nested = compile_path_pattern("a{b,{c,d}}").expect("nested alternatives");
        for (path, expected) in [
            ("ab", true),
            ("ac", true),
            ("ad", true),
            ("ax", false),
            ("abcd", false),
        ] {
            assert_eq!(
                nested.is_match(path),
                expected,
                "nested brace parity for {path}"
            );
        }
        assert!(
            compile_path_pattern("src/[@(].rs").is_ok(),
            "literal characters inside a class are not extglob"
        );
        fence.paths = vec!["crates/**".into()];
        assert!(fence_matches_path(&fence, ".//crates/.hidden").unwrap_or(false));
        assert!(fence_matches_path(&fence, "crates\\core\\lib.rs").unwrap_or(false));
    }

    struct FailOnceSpine {
        inner: SqliteSpine,
        fail: AtomicBool,
    }

    #[async_trait]
    impl Spine for FailOnceSpine {
        async fn append(&self, event: Event) -> Result<Seq> {
            if self.fail.swap(false, Ordering::SeqCst) {
                return Err(adapter("injected publication failure"));
            }
            self.inner.append(event).await
        }
        async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
            self.inner.tail(seat, since).await
        }
        async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
            self.inner.latest_matching(seat, kinds).await
        }
        async fn latest_matching_message(
            &self,
            seat: &SeatId,
            kind: &str,
            msg_id: &str,
        ) -> Result<Option<Event>> {
            self.inner.latest_matching_message(seat, kind, msg_id).await
        }
    }

    async fn service() -> (FreshStore, GovernanceService, Arc<FailOnceSpine>) {
        let fresh = FreshStore::new();
        let pool = pij_store::open(&fresh.path()).await.expect("store");
        let spine = Arc::new(FailOnceSpine {
            inner: SqliteSpine::new(pool.clone()),
            fail: AtomicBool::new(false),
        });
        let raw: Arc<dyn Spine> = spine.clone();
        let bus = Arc::new(EventBus::new(raw, 16).expect("bus"));
        let packets = PathBuf::from(fresh.path()).with_extension("packets");
        (
            fresh,
            GovernanceService::new(SqliteOrchestration::new(pool), bus, packets),
            spine,
        )
    }

    fn delivered_event(dispatch: &Dispatch) -> Event {
        Event { seq: None, v: 1, at: dispatch.created_at + 1, kind: "delivery.outcome".into(), seat: Some(dispatch.to.clone()),
            payload: json!({"msg_id": dispatch.msg_id, "outcome": DeliveryOutcome::Delivered { origin: DeliveryOrigin::ReaderRead }, "transport": "inbox"}).to_string() }
    }

    #[tokio::test]
    async fn delivery_reconciliation_repairs_committed_transition_after_publication_failure_once() {
        let (_fresh, service, spine) = service().await;
        let dispatch: Dispatch =
            serde_json::from_value(event_record("dispatch-queued")).expect("dispatch");
        assert!(
            service
                .store
                .create_dispatch(&dispatch)
                .await
                .expect("dispatch")
        );
        let event = delivered_event(&dispatch);
        service
            .event_bus
            .publish(event.clone())
            .await
            .expect("real delivery outcome");
        spine.fail.store(true, Ordering::SeqCst);
        let failure = service
            .observe_delivery(&event)
            .await
            .expect_err("publication fails after row commit");
        assert!(failure.to_string().contains("E-RS-PARTIAL"));
        assert_eq!(
            service
                .store
                .dispatch(&dispatch.id)
                .await
                .expect("row")
                .expect("present")
                .state,
            DispatchState::Delivered
        );
        service
            .reconcile_deliveries()
            .await
            .expect("repair committed transition");
        service
            .reconcile_deliveries()
            .await
            .expect("idempotent replay");
        let events = service
            .event_bus
            .tail(Some(&dispatch.to), Seq(0))
            .await
            .expect("history");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "dispatch")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn literal_delivery_kind_without_receipt_schema_cannot_kill_reconciliation() {
        let (_fresh, service, _) = service().await;
        let dispatch: Dispatch =
            serde_json::from_value(event_record("dispatch-queued")).expect("dispatch");
        assert!(
            service
                .store
                .create_dispatch(&dispatch)
                .await
                .expect("dispatch")
        );
        service
            .event_bus
            .publish(Event {
                seq: None,
                v: 1,
                at: dispatch.created_at,
                kind: "delivery.outcome".into(),
                seat: Some(dispatch.from.clone()),
                payload: json!({"actor": dispatch.from, "refs": []}).to_string(),
            })
            .await
            .expect("open user kind");
        service
            .event_bus
            .publish(delivered_event(&dispatch))
            .await
            .expect("real outcome");
        service
            .reconcile_deliveries()
            .await
            .expect("manual event is not a receipt");
        assert_eq!(
            service
                .store
                .dispatch(&dispatch.id)
                .await
                .expect("row")
                .expect("present")
                .state,
            DispatchState::Delivered
        );
    }

    #[tokio::test]
    async fn repeated_ack_reuses_immutable_receipt_after_delivery_and_canary_enrichment() {
        let (_fresh, service, _) = service().await;
        let dispatch: Dispatch =
            serde_json::from_value(event_record("dispatch-queued")).expect("dispatch");
        assert!(
            service
                .store
                .create_dispatch(&dispatch)
                .await
                .expect("dispatch")
        );
        assert_eq!(
            service
                .store
                .acknowledge_dispatch(
                    &dispatch.id,
                    &dispatch.to,
                    dispatch.packet_sha256.as_deref().expect("sha"),
                    dispatch.created_at + 2
                )
                .await
                .expect("ack"),
            DispatchAck::Acknowledged
        );
        let acked = service
            .store
            .dispatch(&dispatch.id)
            .await
            .expect("row")
            .expect("present");
        let first = publish_ack_once(&service, &acked)
            .await
            .unwrap_or_else(|e| panic!("{}", e.message));
        let delivered = delivered_event(&dispatch);
        service
            .event_bus
            .publish(delivered.clone())
            .await
            .expect("delivery outcome");
        service
            .observe_delivery(&delivered)
            .await
            .expect("delivery projection");
        let canary: DispatchCanary =
            serde_json::from_value(event_record("dispatch-canary-passed")["canary"].clone())
                .expect("canary fixture");
        service
            .store
            .set_dispatch_canary(&dispatch.id, &canary)
            .await
            .expect("enrichment");
        let enriched = service
            .store
            .dispatch(&dispatch.id)
            .await
            .expect("row")
            .expect("present");
        let again = publish_ack_once(&service, &enriched)
            .await
            .unwrap_or_else(|e| panic!("{}", e.message));
        assert_eq!(
            again, first,
            "mutable delivery/canary data cannot duplicate an immutable acknowledgement event"
        );
    }
}
