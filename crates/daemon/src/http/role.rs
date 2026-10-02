//! One role authority for HTTP, orchestration and admitted registrations.
//! Seat descriptors are not role authority; every projection joins seat_roles.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::error::{PijError, Result};
use pij_core::model::{Envelope, ErrorKind, SeatDescriptor, SeatId, Seq};
use pij_core::ports::Registry;
use pij_store::SqliteOrchestration;
use serde::Serialize;
use serde_json::{Value, json};

use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope};
use crate::events::EventBus;

const COMMAND: &str = "pij role";
type Clock = dyn Fn() -> Result<u64> + Send + Sync;

/// Receipt for one explicit role assertion, including an explicit unset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RoleReceipt {
    /// Subject of the assertion.
    pub seat: SeatId,
    /// Asserted role, or null for unset.
    pub role: Option<String>,
    /// Daemon-resolved author, never body attribution.
    pub assigned_by: SeatId,
    /// Epoch milliseconds at assertion time.
    pub assigned_at: u64,
    /// Durable role-set event sequence.
    pub seq: Seq,
}

/// A request refusal or an atomic assertion failure.
#[derive(Debug)]
pub enum RoleError {
    /// The request cannot be represented by the role contract.
    Invalid(String),
    /// The caller is neither the subject nor its recorded parent.
    Ownership {
        /// Daemon-resolved caller.
        caller: SeatId,
        /// Requested subject.
        seat: SeatId,
        /// Subject's current recorded parent.
        parent: Option<SeatId>,
    },
    /// A dependency failed; no partial role assertion is committed.
    Runtime(PijError),
}
impl std::fmt::Display for RoleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => formatter.write_str(reason),
            Self::Ownership { caller, seat, .. } => write!(
                formatter,
                "E-RS-OWNERSHIP role: {caller} is neither {seat} nor its recorded parent"
            ),
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}
impl RoleError {
    /// Keep the caller's command while preserving structured refusal evidence.
    pub fn into_response(self, command: &str) -> Response {
        let (status, kind, details) = match &self {
            Self::Invalid(_) => (
                StatusCode::BAD_REQUEST,
                ErrorKind::Refused,
                json!({"code":"E-RS-ARG","operation":"role"}),
            ),
            Self::Ownership {
                caller,
                seat,
                parent,
            } => (
                StatusCode::FORBIDDEN,
                ErrorKind::Refused,
                json!({"code":"E-RS-OWNERSHIP","operation":"role","caller":caller,"seat":seat,"parent":parent}),
            ),
            Self::Runtime(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorKind::Adapter,
                json!({"code":"E-RS-STORE","operation":"role"}),
            ),
        };
        let mut answer: Envelope<Value> = Envelope::refused(command, kind, self.to_string());
        answer.details = Some(details);
        envelope(status, &answer)
    }
}

/// Commits roles through the shared, cancellation-safe publication boundary.
pub struct RoleService {
    registry: Arc<dyn Registry>,
    roles: SqliteOrchestration,
    event_bus: Arc<EventBus>,
    clock: Arc<Clock>,
}
impl RoleService {
    /// Compose from the existing shared registry, pool-backed store and event bus.
    pub fn new(
        registry: Arc<dyn Registry>,
        roles: SqliteOrchestration,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            registry,
            roles,
            event_bus,
            clock: Arc::new(system_time_ms),
        }
    }

    /// Inject an epoch-millisecond clock using the daemon service convention.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Persist and publish exactly once for every successful explicit assertion.
    ///
    /// # Errors
    /// Unknown/dead subjects, invalid roles, ownership, store or publication failures.
    pub async fn assert_role(
        &self,
        actor: &SeatId,
        target: &SeatId,
        role: Option<String>,
    ) -> std::result::Result<RoleReceipt, RoleError> {
        if role.as_deref().is_some_and(|value| value.trim().is_empty()) {
            return Err(RoleError::Invalid(
                "role must be a nonempty string or explicit null".to_string(),
            ));
        }
        let registry = self.registry.clone();
        let store = self.roles.clone();
        let clock = self.clock.clone();
        let actor_id = actor.clone();
        let target_id = target.clone();
        // Lazy and owned: the bus takes its global ordering lock before polling
        // this authority read, then retains the whole commit through cancellation.
        let (_, receipt) = self
            .event_bus
            .publish_committed(async move {
                let caller = registry
                    .get(&actor_id)
                    .await?
                    .filter(|seat| seat.tombstoned_at.is_none())
                    .ok_or_else(|| PijError::GovernanceRefused {
                        code: "E-RS-ARG".to_string(),
                        record: actor_id.to_string(),
                    })?;
                let subject = if actor_id == target_id {
                    caller
                } else {
                    registry
                        .get(&target_id)
                        .await?
                        .filter(|seat| seat.tombstoned_at.is_none())
                        .ok_or_else(|| PijError::GovernanceRefused {
                            code: "E-RS-ARG".to_string(),
                            record: target_id.to_string(),
                        })?
                };
                if actor_id != target_id && subject.parent.as_ref() != Some(&actor_id) {
                    return Err(PijError::GovernanceRefused {
                        code: "E-RS-OWNERSHIP".to_string(),
                        record: subject
                            .parent
                            .map_or_else(|| "absent".to_string(), |parent| parent.to_string()),
                    });
                }
                let assigned_at = clock()?;
                let event = store
                    .assert_role_committed(&actor_id, &target_id, role.as_deref(), assigned_at)
                    .await?;
                let seq = event.seq.ok_or_else(|| PijError::Adapter {
                    adapter: "daemon/role".to_string(),
                    message: "committed role event has no store-assigned sequence".to_string(),
                })?;
                let receipt = RoleReceipt {
                    seat: target_id,
                    role,
                    assigned_by: actor_id,
                    assigned_at,
                    seq,
                };
                Ok((event, receipt))
            })
            .await
            .map_err(|error| match error {
                PijError::GovernanceRefused { code, record } if code == "E-RS-OWNERSHIP" => {
                    RoleError::Ownership {
                        caller: actor.clone(),
                        seat: target.clone(),
                        parent: (record != "absent").then(|| SeatId::from(record)),
                    }
                }
                PijError::GovernanceRefused { code, record } => {
                    RoleError::Invalid(format!("role assertion refused: {code} ({record})"))
                }
                error => RoleError::Runtime(error),
            })?;
        Ok(receipt)
    }

    /// Read the authoritative role, never a descriptor's stale copy.
    ///
    /// # Errors
    /// Store/schema failures remain observable instead of becoming null roles.
    pub async fn read_role(&self, seat: &SeatId) -> Result<Option<String>> {
        Ok(self
            .roles
            .seat_role(seat)
            .await?
            .map(|assignment| assignment.role))
    }

    /// Project one response descriptor with its authoritative role, without a roster read.
    ///
    /// Consume only a response value: registry/CAS inputs and identity resolution
    /// must retain the raw descriptor. An absent assignment clears a stale role.
    ///
    /// # Errors
    /// Store/schema failures refuse the projection; never fall back to the raw role.
    pub async fn project_seat(&self, mut seat: SeatDescriptor) -> Result<SeatDescriptor> {
        seat.role = self.read_role(&seat.id).await?;
        Ok(seat)
    }

    /// Join all local roster roles with one store read; absent rows clear stale copies.
    ///
    /// # Errors
    /// Store/schema failures; callers must refuse the projection rather than fallback.
    pub async fn join_roles(&self, seats: &mut [SeatDescriptor]) -> Result<()> {
        let mut roles: std::collections::BTreeMap<SeatId, String> = self
            .roles
            .list_roles()
            .await?
            .into_iter()
            .map(|assignment| (assignment.seat, assignment.role))
            .collect();
        for seat in seats {
            seat.role = roles.remove(&seat.id);
        }
        Ok(())
    }
}

fn system_time_ms() -> Result<u64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/role".to_string(),
            message: format!("clock precedes Unix epoch: {error}"),
        })?;
    u64::try_from(elapsed.as_millis()).map_err(|error| PijError::Adapter {
        adapter: "daemon/role".to_string(),
        message: format!("clock does not fit epoch milliseconds: {error}"),
    })
}

struct RoleCall {
    seat: Option<SeatId>,
    role: Option<String>,
    caller: CallerContext,
    attribution: Vec<SeatId>,
}

fn parse_request(body: Value) -> std::result::Result<RoleCall, RoleError> {
    let invalid = |reason: &str| RoleError::Invalid(reason.to_string());
    let object = body
        .as_object()
        .ok_or_else(|| invalid("role request must be an object"))?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "seat" | "role" | "argv" | "caller" | "actor" | "assigned_by"
        ) {
            return Err(RoleError::Invalid(format!(
                "unknown role request field: {key}"
            )));
        }
    }
    let caller = match object.get("caller") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| RoleError::Invalid(error.to_string()))?,
        None => CallerContext::default(),
    };
    let mut attribution = Vec::new();
    for key in ["actor", "assigned_by"] {
        if let Some(value) = object.get(key) {
            let value = value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| invalid("role attribution must name a seat"))?;
            attribution.push(SeatId::from(value));
        }
    }
    let (seat, role) = if let Some(argv) = object.get("argv") {
        if object.contains_key("seat") || object.contains_key("role") {
            return Err(invalid("role accepts typed fields or argv, not both"));
        }
        let argv: Vec<String> = serde_json::from_value(argv.clone())
            .map_err(|error| RoleError::Invalid(error.to_string()))?;
        let (seat, role, actor) = parse_argv(&argv)?;
        if let Some(actor) = actor {
            attribution.push(actor);
        }
        (seat, role)
    } else {
        let seat = object
            .get("seat")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(SeatId::from)
                    .ok_or_else(|| invalid("role seat must be a nonempty string when supplied"))
            })
            .transpose()?;
        let role = match object.get("role") {
            Some(Value::Null) => None,
            Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
            _ => {
                return Err(invalid(
                    "role requires a nonempty string or explicit null; omission is not unset",
                ));
            }
        };
        (seat, role)
    };
    Ok(RoleCall {
        seat,
        role,
        caller,
        attribution,
    })
}

type ParsedArgv = (Option<SeatId>, Option<String>, Option<SeatId>);
fn parse_argv(argv: &[String]) -> std::result::Result<ParsedArgv, RoleError> {
    let invalid = |reason: &str| RoleError::Invalid(reason.to_string());
    let mut tokens = argv.iter().map(String::as_str);
    if tokens.next() != Some("role") {
        return Err(invalid(
            "expected role [seat] <role> or role [seat] --unset",
        ));
    }
    let mut positionals = Vec::new();
    let mut unset = false;
    let mut actor = None;
    while let Some(token) = tokens.next() {
        match token {
            "--json" => {}
            "--unset" if !unset => unset = true,
            "--actor" if actor.is_none() => {
                let value = tokens
                    .next()
                    .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
                    .ok_or_else(|| invalid("--actor requires a seat"))?;
                actor = Some(SeatId::from(value));
            }
            flag if flag.starts_with('-') => {
                return Err(RoleError::Invalid(format!(
                    "unknown or repeated role flag: {flag}"
                )));
            }
            value if !value.trim().is_empty() => positionals.push(value),
            _ => return Err(invalid("role arguments cannot be empty")),
        }
    }
    let (seat, role) = match (unset, positionals.as_slice()) {
        (false, [role]) => (None, Some((*role).to_string())),
        (false, [seat, role]) => (Some(SeatId::from(*seat)), Some((*role).to_string())),
        (true, []) => (None, None),
        (true, [seat]) => (Some(SeatId::from(*seat)), None),
        _ => {
            return Err(invalid(
                "expected role [seat] <role> or role [seat] --unset",
            ));
        }
    };
    Ok((seat, role, actor))
}

pub(crate) async fn role(
    State(state): State<AppState>,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return RoleError::Invalid(error.body_text()).into_response(COMMAND),
    };
    let call = match parse_request(body) {
        Ok(call) => call,
        Err(error) => return error.into_response(COMMAND),
    };
    let actor = match resolve_seat(&state, COMMAND, call.caller.session_id, call.caller.pane).await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    let target = call.seat.unwrap_or_else(|| actor.id.clone());
    if call.attribution.iter().any(|claimed| claimed != &actor.id) {
        return RoleError::Ownership {
            caller: actor.id,
            seat: target,
            parent: actor.parent,
        }
        .into_response(COMMAND);
    }
    match state
        .services
        .roles
        .assert_role(&actor.id, &target, call.role)
        .await
    {
        Ok(receipt) => envelope(StatusCode::OK, &Envelope::ok(COMMAND, receipt)),
        Err(error) => error.into_response(COMMAND),
    }
}

#[cfg(test)]
#[path = "role_tests.rs"]
mod tests;
