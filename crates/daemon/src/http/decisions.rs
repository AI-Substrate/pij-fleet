//! Durable questions, current-parent authority, and existing delivery admission.
use super::identity::{CallerContext, Resolved, resolve_seat};
use super::{AppState, envelope, system_time_ms};
use crate::delivery::DeliveryService;
use crate::events::EventBus;
use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use pij_core::config::AdapterChoice;
use pij_core::decisions::{Decision, DecisionState, may_answer};
use pij_core::error::{PijError, Result};
use pij_core::model::{
    DeliveryOutcome, Envelope, ErrorKind, Msg, SeatDescriptor, SeatId, SemanticState,
};
use pij_core::orchestration::PrimeState;
use pij_core::ports::{Registry, SeatFilter};
use pij_core::report::{ReportConfig, ReportService};
use pij_store::SqliteOrchestration;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Question/answer service over resources already composed by the daemon.
pub struct DecisionService {
    store: SqliteOrchestration,
    registry: Arc<dyn Registry>,
    bus: Arc<EventBus>,
    delivery: Arc<DeliveryService>,
    queue_backend: AdapterChoice,
    spine_backend: AdapterChoice,
    answer_lock: Mutex<()>,
}

impl DecisionService {
    /// The composition root must pass the actual delivery queue and event-spine
    /// choices. Real/Real asserts that queue, store, and bus share one SQLite
    /// pool (persistent in production); backend labels alone are not proof.
    /// Other combinations keep ordinary question/answer support but cannot
    /// supersede, because this store cannot observe their delivery authority.
    pub fn new(
        store: SqliteOrchestration,
        registry: Arc<dyn Registry>,
        bus: Arc<EventBus>,
        delivery: Arc<DeliveryService>,
        queue_backend: AdapterChoice,
        spine_backend: AdapterChoice,
    ) -> Self {
        Self {
            store,
            registry,
            bus,
            delivery,
            queue_backend,
            spine_backend,
            answer_lock: Mutex::new(()),
        }
    }

    /// Assignment existence and immutable owner are checked before any report write.
    pub(crate) async fn validate_report_assignment(
        &self,
        seat: &SeatId,
        assignment_id: Option<&str>,
    ) -> std::result::Result<(), DecisionError> {
        let Some(id) = assignment_id else {
            return Ok(());
        };
        let task = self.store.task(id).await?.ok_or_else(|| DecisionError {
            status: StatusCode::BAD_REQUEST,
            code: "E-RS-ASSIGNMENT-UNKNOWN".into(),
            message: format!("no task assignment {id}"),
            details: json!({"assignment_id":id}),
        })?;
        if task.node_id != *seat {
            return Err(DecisionError {
                status: StatusCode::BAD_REQUEST,
                code: "E-RS-ASSIGNMENT-NOT-YOURS".into(),
                message: format!("assignment {id} belongs to {}, not {seat}", task.node_id),
                details: json!({"assignment_id":id,"owner":task.node_id,"caller":seat}),
            });
        }
        Ok(())
    }

    /// Persist first, then publish the existing question state OUTSIDE the bus
    /// lock. A second-step failure returns the durable decision as partial proof.
    pub async fn question(
        &self,
        asker: &SeatId,
        question: &str,
        assignment_id: Option<&str>,
        refs: &[String],
    ) -> std::result::Result<Value, DecisionError> {
        self.validate_report_assignment(asker, assignment_id)
            .await?;
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| DecisionError::internal(error.to_string()))?;
        let mut id = String::from("decision-");
        for byte in bytes {
            write!(id, "{byte:02x}").expect("String write");
        }
        let at = system_time_ms()?;
        let store = self.store.clone();
        let registry = self.registry.clone();
        let asker = asker.clone();
        let note = question.to_string();
        let (seq, decision) = self
            .bus
            .publish_committed(async move {
                let seat = live_seat(registry.as_ref(), &asker).await?;
                store
                    .open_decision_committed(&id, &asker, seat.parent.as_ref(), &note, at)
                    .await
            })
            .await?;
        let reports = ReportService::new(
            self.registry.as_ref(),
            self.bus.as_ref(),
            move || at,
            ReportConfig::default(),
        );
        if let Err(error) = reports
            .declare(
                &decision.asked_by,
                Some(SemanticState::Question),
                Some(question),
                assignment_id,
                refs,
            )
            .await
        {
            return Err(DecisionError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "E-RS-QUESTION-PARTIAL".into(),
                message: format!(
                    "decision {} is durable at spine {}; question state publication failed: {error}",
                    decision.id, seq.0
                ),
                details: json!({"decision":decision,"seq":seq,"state_published":false}),
            });
        }
        Ok(
            json!({"seat":decision.asked_by,"seq":seq,"line":format!("question recorded: {}",decision.id),"decision":decision,"assignment_id":assignment_id,"refs":refs}),
        )
    }

    /// Read live responsibility in bulk. Historical opened events are untouched.
    pub async fn list(
        &self,
        filters: &BTreeMap<String, String>,
    ) -> std::result::Result<Value, DecisionError> {
        let mut rows = self.store.list_decisions().await?;
        let seats = self.registry.list(SeatFilter::default()).await?;
        let prime = self
            .store
            .prime()
            .await?
            .filter(|row| row.state == PrimeState::Current)
            .map(|row| row.seat);
        project_parents(&mut rows, &seats, prime.as_ref());
        let cursor = self.store.spine_head().await?;
        let state = filters.get("state").map(String::as_str).unwrap_or("open");
        if !matches!(state, "open" | "answered" | "all") {
            return Err(DecisionError::argument("state must be open|answered|all"));
        }
        rows.retain(|row| {
            (state == "all" || (row.state == DecisionState::Open) == (state == "open"))
                && filters
                    .get("asked_by")
                    .is_none_or(|id| row.asked_by.as_str() == id)
                && filters.get("parent").is_none_or(|id| {
                    row.parent
                        .as_ref()
                        .is_some_and(|parent| parent.as_str() == id)
                })
        });
        Ok(json!({"decisions":rows,"cursor":cursor}))
    }

    /// Durable linkage precedes admission; only an accepted delivery closes a
    /// non-self answer. Retries retain both message id and answer event sequence.
    pub async fn answer(
        &self,
        actor: &SeatId,
        id: &str,
        answer: &str,
        supersede: bool,
    ) -> std::result::Result<Value, DecisionError> {
        let _guard = self.answer_lock.lock().await;
        let stored = self
            .store
            .get_decision(id)
            .await?
            .ok_or_else(|| DecisionError::argument(format!("unknown decision {id}")))?;
        let asker = live_seat(self.registry.as_ref(), &stored.decision.asked_by).await?;
        if !may_answer(actor, &asker) {
            return Err(DecisionError::ownership("answer", actor, &asker));
        }
        let prepared = if supersede {
            let store = self.store.clone();
            let registry = self.registry.clone();
            let actor = actor.clone();
            let asker_id = asker.id.clone();
            let id = id.to_string();
            let answer = answer.to_string();
            let queue_backend = self.queue_backend;
            let spine_backend = self.spine_backend;
            let at = system_time_ms()?;
            let (_,prepared) = self.bus.publish_committed(async move {
                let current = live_seat(registry.as_ref(),&asker_id).await?;
                if current.parent.as_ref() != Some(&actor) || current.id == actor {
                    return Err(PijError::GovernanceRefused {code:"E-RS-OWNERSHIP".into(),record:current.id.to_string()});
                }
                if !queue_backend.is_real() || !spine_backend.is_real() {
                    return Err(PijError::GovernanceRefused {code:"E-RS-ANSWER-AUTHORITY-SPLIT".into(),record:json!({"decision":id,"queue_backend":queue_backend,"spine_backend":spine_backend}).to_string()});
                }
                store.supersede_decision_answer_committed(&id,&actor,&answer,at).await
            }).await?;
            prepared
        } else {
            let msg_id = (actor != &asker.id).then(|| {
                stored
                    .decision
                    .answer_msg_id
                    .unwrap_or_else(|| format!("{id}-answer"))
            });
            self.store
                .prepare_decision_answer(id, actor, answer, msg_id.as_deref())
                .await?
        };
        if prepared.decision.state == DecisionState::Answered {
            let seq = prepared
                .answer_seq
                .ok_or_else(|| DecisionError::internal("answered decision has no receipt"))?;
            return Ok(json!({"decision":prepared.decision,"seq":seq}));
        }
        if let Some(msg_id) = prepared.decision.answer_msg_id.as_ref() {
            let receipt = self
                .delivery
                .accept(Msg {
                    from: actor.clone(),
                    to: asker.id.clone(),
                    body: answer.to_string(),
                    msg_id: msg_id.clone(),
                    from_machine: None,
                    in_reply_to: None,
                    command: None,
                })
                .await
                .map_err(|error| DecisionError {
                    status: StatusCode::CONFLICT,
                    code: "E-RS-ANSWER-DELIVERY".into(),
                    message: error.to_string(),
                    details: json!({"decision":prepared.decision,"state":"open"}),
                })?;
            if !matches!(
                receipt.outcome,
                DeliveryOutcome::Queued { .. } | DeliveryOutcome::Delivered { .. }
            ) {
                return Err(DecisionError {
                    status: StatusCode::CONFLICT,
                    code: "E-RS-ANSWER-DELIVERY".into(),
                    message: "answer was not durably accepted; decision remains open".into(),
                    details: json!({"decision":prepared.decision,"receipt":receipt}),
                });
            }
        }
        let store = self.store.clone();
        let registry = self.registry.clone();
        let actor = actor.clone();
        let asker_id = asker.id;
        let id = id.to_string();
        let expected_msg_id = prepared.decision.answer_msg_id;
        let at = system_time_ms()?;
        let (seq, decision) = self
            .bus
            .publish_committed(async move {
                let current = live_seat(registry.as_ref(), &asker_id).await?;
                if !may_answer(&actor, &current) {
                    return Err(PijError::GovernanceRefused {
                        code: "E-RS-OWNERSHIP".into(),
                        record: current.id.to_string(),
                    });
                }
                store
                    .answer_decision_committed(&id, &actor, expected_msg_id.as_deref(), at)
                    .await
            })
            .await?;
        Ok(json!({"decision":decision,"seq":seq}))
    }

    /// Verify the exact latest done event, with authority read inside the common
    /// publication lock so reparenting cannot overtake the receipt.
    pub async fn verify(
        &self,
        actor: &SeatId,
        target: &SeatId,
        assignment: Option<String>,
    ) -> std::result::Result<Value, DecisionError> {
        let store = self.store.clone();
        let registry = self.registry.clone();
        let actor = actor.clone();
        let target = target.clone();
        let at = system_time_ms()?;
        let (seq, mut receipt) = self
            .bus
            .publish_committed(async move {
                let seat = live_seat(registry.as_ref(), &target).await?;
                if seat.parent.as_ref() != Some(&actor) || target == actor {
                    return Err(PijError::GovernanceRefused {
                        code: "E-RS-OWNERSHIP".into(),
                        record: target.to_string(),
                    });
                }
                store
                    .verify_done_committed(&actor, &target, assignment.as_deref(), at)
                    .await
            })
            .await?;
        receipt["seq"] = json!(seq);
        receipt["line"] = json!(format!(
            "verified {} done at spine {}",
            receipt["seat"].as_str().unwrap_or_default(),
            receipt["done_seq"]
        ));
        Ok(receipt)
    }
}

pub(crate) async fn live_seat(registry: &dyn Registry, id: &SeatId) -> Result<SeatDescriptor> {
    registry
        .get(id)
        .await?
        .filter(|seat| seat.tombstoned_at.is_none())
        .ok_or_else(|| PijError::NoRegistryEntry {
            seat: id.clone(),
            store: "the daemon registry".into(),
        })
}

/// Read projection only. Prime fallback exposes root questions but grants no authority.
pub(crate) fn project_parents(
    rows: &mut [Decision],
    seats: &[SeatDescriptor],
    prime: Option<&SeatId>,
) {
    let parents: BTreeMap<_, _> = seats
        .iter()
        .map(|seat| (&seat.id, seat.parent.as_ref()))
        .collect();
    for row in rows {
        row.parent = parents
            .get(&row.asked_by)
            .copied()
            .flatten()
            .or(prime)
            .cloned();
    }
}

/// Decodable failure, including a durable partial question or answer linkage.
#[derive(Debug)]
pub struct DecisionError {
    status: StatusCode,
    code: String,
    message: String,
    details: Value,
}
impl DecisionError {
    pub(crate) fn argument(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "E-RS-ARG".into(),
            message: message.into(),
            details: json!({}),
        }
    }
    pub(crate) fn anomaly_scope(mut self) -> Self {
        self.message.push_str(". status-stale is node-keyed; --project excludes it and --here can hide worktree-resident seats. Run pij anomalies unscoped for the complete fleet view.");
        self
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "E-RS-STORE".into(),
            message: message.into(),
            details: json!({}),
        }
    }
    fn ownership(operation: &str, actor: &SeatId, target: &SeatDescriptor) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "E-RS-OWNERSHIP".into(),
            message: format!(
                "{operation}: {actor} is neither {} nor its recorded parent",
                target.id
            ),
            details: json!({"operation":operation,"caller":actor,"seat":target.id,"parent":target.parent}),
        }
    }
    /// Return the existing v2 envelope, not an independent error schema.
    pub fn response(mut self, command: &str) -> Response {
        self.details["code"] = json!(self.code);
        let mut body = Envelope::<Value>::refused(
            command,
            if self.status.is_server_error() {
                ErrorKind::Adapter
            } else {
                ErrorKind::Refused
            },
            format!("{} {}", self.code, self.message),
        );
        body.details = Some(self.details);
        envelope(self.status, &body)
    }
}
impl From<PijError> for DecisionError {
    fn from(error: PijError) -> Self {
        match error {
            PijError::GovernanceRefused { code, record }
                if code == "E-RS-ANSWER-AUTHORITY-SPLIT" =>
            {
                Self {
                    status: StatusCode::CONFLICT,
                    code,
                    message: format!(
                        "answer supersession requires shared SQLite queue and spine authority: {record}"
                    ),
                    details: serde_json::from_str(&record)
                        .unwrap_or_else(|_| json!({"record":record})),
                }
            }
            PijError::GovernanceRefused { code, record }
                if matches!(
                    code.as_str(),
                    "E-RS-ANSWER-IN-TRANSIT" | "E-RS-ANSWER-DELIVERED"
                ) =>
            {
                let message = if code == "E-RS-ANSWER-IN-TRANSIT" {
                    "an answer in transit cannot be withdrawn; wait for its terminal outcome"
                } else {
                    "a delivered or acknowledged answer cannot be superseded"
                };
                Self {
                    status: StatusCode::CONFLICT,
                    code,
                    message: message.into(),
                    details: serde_json::from_str(&record)
                        .unwrap_or_else(|_| json!({"record":record})),
                }
            }
            PijError::GovernanceRefused { code, record } => Self {
                status: if code == "E-RS-OWNERSHIP" {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::CONFLICT
                },
                code,
                message: format!("transition refused for {record}"),
                details: json!({"record":record}),
            },
            PijError::NoRegistryEntry { seat, store } => Self {
                status: StatusCode::BAD_REQUEST,
                code: "E-RS-NO-SEAT".into(),
                message: format!("{seat} is not an active seat in {store}"),
                details: json!({"seat":seat}),
            },
            error => Self::internal(error.to_string()),
        }
    }
}

/// Shared argv/caller body for the new read families.
#[derive(Deserialize)]
pub struct ReadRequest {
    /// Full original argv.
    pub argv: Vec<String>,
    /// Caller identity evidence.
    #[serde(default)]
    pub caller: CallerContext,
}

/// Parse the supported filters once for GET and argv readers. Unknown and
/// duplicate options refuse; flags are never silently discarded.
pub(crate) fn parse_filters(
    argv: &[String],
    family: &str,
    allowed: &[&str],
    boolean: &[&str],
) -> std::result::Result<BTreeMap<String, String>, DecisionError> {
    if argv.first().map(String::as_str) != Some(family) {
        return Err(DecisionError::argument("argv family does not match route"));
    }
    let mut filters = BTreeMap::new();
    let mut tokens = argv[1..].iter();
    while let Some(token) = tokens.next() {
        if token == "--json" {
            continue;
        }
        let option = token
            .strip_prefix("--")
            .ok_or_else(|| DecisionError::argument(format!("unexpected argument {token}")))?;
        let (name, inline) = option
            .split_once('=')
            .map_or((option, None), |(name, value)| (name, Some(value)));
        if !allowed.contains(&name) {
            return Err(DecisionError::argument(format!("unknown flag --{name}")));
        }
        let value = if let Some(value) = inline {
            value.to_string()
        } else if boolean.contains(&name) {
            "true".into()
        } else {
            tokens
                .next()
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| DecisionError::argument(format!("--{name} requires a value")))?
                .clone()
        };
        if value.is_empty() || filters.insert(name.to_string(), value).is_some() {
            return Err(DecisionError::argument(format!(
                "empty or duplicate --{name}"
            )));
        }
    }
    Ok(filters)
}

/// Boolean argv scope uses the caller's folder, never a supplied flag value.
pub(crate) fn here_from_flag<'a>(
    value: Option<&str>,
    cwd: Option<&'a str>,
) -> std::result::Result<Option<&'a str>, DecisionError> {
    match value {
        None | Some("false") => Ok(None),
        Some("true") => cwd
            .map(Some)
            .ok_or_else(|| DecisionError::argument("--here requires caller cwd")),
        _ => Err(DecisionError::argument(
            "--here is a boolean; scope comes from caller cwd",
        )),
    }
}

/// HTTP scope is absolute. Canonicalise available paths without requiring
/// remote or retained registry folders to exist on this daemon's filesystem.
pub(crate) fn here_path(
    value: Option<&str>,
) -> std::result::Result<Option<std::borrow::Cow<'_, str>>, DecisionError> {
    value
        .map(|value| {
            let path = std::path::Path::new(value);
            if !path.is_absolute() {
                return Err(DecisionError::argument(
                    "here requires an absolute path, never bare true or daemon cwd",
                ));
            }
            Ok(match path.canonicalize() {
                Ok(path) => std::borrow::Cow::Owned(path.to_string_lossy().into_owned()),
                Err(_) => std::borrow::Cow::Borrowed(value),
            })
        })
        .transpose()
}

pub(crate) fn query_argv(family: &str, query: BTreeMap<String, String>) -> Vec<String> {
    std::iter::once(family.to_string())
        .chain(
            query
                .into_iter()
                .map(|(key, value)| format!("--{key}={value}")),
        )
        .collect()
}

pub(crate) async fn list_get(
    State(state): State<AppState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    list(&state, &query_argv("decisions", query)).await
}
pub(crate) async fn list_post(
    State(state): State<AppState>,
    Json(request): Json<ReadRequest>,
) -> Response {
    match resolve_seat(
        &state,
        "pij decisions",
        request.caller.session_id,
        request.caller.pane,
    )
    .await
    {
        Resolved::Refusal(response) => response,
        Resolved::Seat(_, _) => list(&state, &request.argv).await,
    }
}
async fn list(state: &AppState, argv: &[String]) -> Response {
    match parse_filters(argv, "decisions", &["state", "asked_by", "parent"], &[]) {
        Err(error) => error.response("pij decisions"),
        Ok(filters) => respond(
            "pij decisions",
            state.services.decisions.list(&filters).await,
        ),
    }
}

/// Typed native answers and shim argv converge on the same operation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerRequest {
    /// Typed decision identity.
    pub decision: Option<String>,
    /// Typed answer text.
    pub answer: Option<String>,
    /// Explicit recovery; only the asker's current parent may supersede.
    pub supersede: Option<bool>,
    /// Shim command tokens.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Resolved by the shared identity ladder.
    #[serde(default)]
    pub caller: CallerContext,
}

pub(crate) async fn answer_post(
    State(state): State<AppState>,
    Json(mut request): Json<AnswerRequest>,
) -> Response {
    let command = "pij answer";
    let actor = match resolve_seat(
        &state,
        command,
        request.caller.session_id.take(),
        request.caller.pane.take(),
    )
    .await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    let (id, answer, supersede) = match parse_answer(&request) {
        Ok(args) => args,
        Err(error) => return error.response(command),
    };
    respond(
        command,
        state
            .services
            .decisions
            .answer(&actor.id, id, answer, supersede)
            .await,
    )
}

fn parse_answer(request: &AnswerRequest) -> std::result::Result<(&str, &str, bool), DecisionError> {
    let (id, answer, supersede) = if request.argv.is_empty() {
        match (request.decision.as_deref(), request.answer.as_deref()) {
            (Some(id), Some(answer)) => (id, answer, request.supersede.unwrap_or(false)),
            _ => {
                return Err(DecisionError::argument(
                    "answer requires decision and answer",
                ));
            }
        }
    } else {
        if request.decision.is_some() || request.answer.is_some() || request.supersede.is_some() {
            return Err(DecisionError::argument(
                "do not mix typed answer fields and argv",
            ));
        }
        let mut supersede = false;
        let mut args = Vec::with_capacity(3);
        for arg in &request.argv {
            match arg.as_str() {
                "--json" => {}
                "--supersede" if !supersede => supersede = true,
                flag if flag.starts_with("--") => {
                    return Err(DecisionError::argument(format!(
                        "unknown or duplicate answer option {flag}"
                    )));
                }
                arg => args.push(arg),
            }
        }
        if args.len() != 3 || args[0] != "answer" {
            return Err(DecisionError::argument(
                "usage: pij answer [--supersede] <decision> <answer> [--json]",
            ));
        }
        (args[1], args[2], supersede)
    };
    if id.trim().is_empty() || answer.trim().is_empty() {
        return Err(DecisionError::argument(
            "decision and answer must not be empty",
        ));
    }
    Ok((id, answer, supersede))
}

pub(crate) fn respond(
    command: &str,
    result: std::result::Result<Value, DecisionError>,
) -> Response {
    match result {
        Ok(data) => envelope(StatusCode::OK, &Envelope::ok(command, data)),
        Err(error) => error.response(command),
    }
}

pub(crate) async fn question(
    state: &AppState,
    seat: &SeatId,
    note: &str,
    assignment_id: Option<&str>,
    refs: &[String],
) -> Response {
    respond(
        "pij report",
        state
            .services
            .decisions
            .question(seat, note, assignment_id, refs)
            .await,
    )
}
pub(crate) async fn verify(
    state: &AppState,
    actor: &SeatId,
    target: &str,
    assignment: Option<String>,
) -> Response {
    respond(
        "pij report",
        state
            .services
            .decisions
            .verify(actor, &SeatId::from(target), assignment)
            .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::config::{Adapters, Config};
    use pij_core::model::{DeliveryOrigin, Harness, Outcome, ProcIdentity, Seq};
    use pij_core::ports::{Queue, Spine};
    use pij_harnesses::InteractionGate;
    use pij_store::{SqliteQueue, SqliteRegistry, SqliteSpine, StorePool};
    use pij_testkit::FreshStore;
    use pij_testkit::fakes::{FakeTmux, FakeTransport};

    struct Fixture {
        service: DecisionService,
        registry: Arc<SqliteRegistry>,
        bus: Arc<EventBus>,
        store: SqliteOrchestration,
        pool: StorePool,
        transport: Arc<FakeTransport>,
        worker: SeatId,
        parent: SeatId,
    }
    impl Fixture {
        async fn new(transport: FakeTransport) -> Self {
            let pool = pij_store::open("").await.expect("shared memory SQL");
            let store = SqliteOrchestration::new(pool.clone());
            let bus = Arc::new(
                EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 128).expect("bus"),
            );
            let registry = Arc::new(SqliteRegistry::new(pool.clone(), bus.clone()));
            let fixtures: Value = serde_json::from_str(include_str!(
                "../../../testkit/fixtures/golden/api/governance-routes.json"
            ))
            .expect("contract");
            let worker = SeatId::from(
                fixtures["fixture_context"]["worker"]
                    .as_str()
                    .expect("worker"),
            );
            let parent = SeatId::from(
                fixtures["fixture_context"]["parent"]
                    .as_str()
                    .expect("parent"),
            );
            let mut seat = SeatDescriptor::new(worker.clone(), Harness::Omp, "/work");
            seat.parent = Some(parent.clone());
            registry.put(seat).await.expect("worker");
            registry
                .put(SeatDescriptor::new(parent.clone(), Harness::Omp, "/work"))
                .await
                .expect("parent");
            let queue = Arc::new(SqliteQueue::new(pool.clone(), 30, 128).expect("queue"));
            let interaction = Arc::new(InteractionGate::new(Arc::new(FakeTmux::new())));
            let transport = Arc::new(transport);
            let delivery = Arc::new(
                DeliveryService::new(
                    registry.clone(),
                    queue,
                    transport.clone(),
                    interaction,
                    bus.clone(),
                )
                .expect("delivery"),
            );
            let service = DecisionService::new(
                store.clone(),
                registry.clone(),
                bus.clone(),
                delivery,
                AdapterChoice::Real,
                AdapterChoice::Real,
            );
            Self {
                service,
                registry,
                bus,
                store,
                pool,
                transport,
                worker,
                parent,
            }
        }
        async fn ask(&self) -> String {
            let opened = self
                .service
                .question(&self.worker, "which branch owns this?", None, &[])
                .await
                .expect("question");
            opened["decision"]["id"].as_str().expect("id").to_string()
        }
        async fn socket_worker(&self) {
            let mut worker = self
                .registry
                .get(&self.worker)
                .await
                .expect("read")
                .expect("worker");
            worker.harness = Harness::Claude;
            worker.proc = Some(ProcIdentity {
                pid: 123,
                proc_start: 456,
            });
            worker.semantic_state = None;
            self.registry.put(worker).await.expect("socket worker");
        }
    }

    #[tokio::test]
    async fn reparenting_moves_live_question_and_revokes_old_answer_authority() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let id = f.ask().await;
        let new_parent = SeatId::from("new-parent");
        f.registry
            .put(SeatDescriptor::new(
                new_parent.clone(),
                Harness::Omp,
                "/work",
            ))
            .await
            .expect("new parent");
        let mut worker = f
            .registry
            .get(&f.worker)
            .await
            .expect("get")
            .expect("worker");
        worker.parent = Some(new_parent.clone());
        f.registry.put(worker).await.expect("reparent");
        let old = f
            .service
            .list(&BTreeMap::from([("parent".into(), f.parent.to_string())]))
            .await
            .expect("old view");
        assert!(old["decisions"].as_array().expect("rows").is_empty());
        let current = f
            .service
            .list(&BTreeMap::from([("parent".into(), new_parent.to_string())]))
            .await
            .expect("new view");
        assert_eq!(current["decisions"][0]["parent"], new_parent.as_str());
        assert_eq!(
            f.store
                .get_decision(&id)
                .await
                .expect("stored")
                .expect("row")
                .decision
                .parent,
            Some(f.parent.clone())
        );
        assert_eq!(
            f.service
                .answer(&f.parent, &id, "main", false)
                .await
                .expect_err("old parent refused")
                .status,
            StatusCode::FORBIDDEN
        );
        let accepted = f
            .service
            .answer(&new_parent, &id, "main", false)
            .await
            .expect("new parent answer");
        let again = f
            .service
            .answer(&new_parent, &id, "main", false)
            .await
            .expect("idempotent");
        assert_eq!(accepted, again);
        let inbox = f
            .service
            .delivery
            .claim_inbox(&f.worker, false)
            .await
            .expect("actual inbox");
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].message.from, new_parent);
        assert_eq!(inbox[0].message.to, f.worker);
        assert_eq!(inbox[0].message.body, "main");
        assert_eq!(inbox[0].message.msg_id, format!("{id}-answer"));
        assert!(
            f.service
                .answer(&new_parent, &id, "other", false)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn self_answer_closes_without_any_delivery_push_or_queue_row() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let id = f.ask().await;
        let answered = f
            .service
            .answer(&f.worker, &id, "self ruling", false)
            .await
            .expect("self answer");
        assert!(answered["decision"]["answer_msg_id"].is_null());
        assert_eq!(answered["decision"]["answered_by"], f.worker.as_str());
        let events = f.bus.tail(None, Seq(0)).await.expect("events");
        assert!(
            !events
                .iter()
                .any(|event| event.kind == "message.pushed" || event.kind == "delivery.outcome")
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "decision.answered")
                .count(),
            1
        );
        assert!(
            f.service
                .delivery
                .claim_inbox(&f.worker, false)
                .await
                .expect("inbox")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn question_second_step_failure_exposes_durable_decision_and_does_not_deadlock() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        sqlx::query("CREATE TRIGGER reject_question_state BEFORE INSERT ON spine_events WHEN NEW.kind='seat.put' BEGIN SELECT RAISE(ABORT,'injected second-step failure'); END").execute(&f.pool).await.expect("inject");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            f.service.question(&f.worker, "which branch?", None, &[]),
        )
        .await
        .expect("no recursive bus lock");
        let error = result.expect_err("partial failure");
        assert_eq!(error.code, "E-RS-QUESTION-PARTIAL");
        let id = error.details["decision"]["id"]
            .as_str()
            .expect("durable id");
        assert_eq!(
            f.store
                .get_decision(id)
                .await
                .expect("stored")
                .expect("row")
                .decision
                .state,
            DecisionState::Open
        );
        assert!(
            f.registry
                .get(&f.worker)
                .await
                .expect("registry")
                .expect("worker")
                .semantic_state
                .is_none()
        );
        let opened = f
            .bus
            .tail(None, Seq(0))
            .await
            .expect("events")
            .into_iter()
            .find(|event| event.kind == "decision.opened")
            .expect("opened event");
        assert_eq!(error.details["seq"], opened.seq.expect("seq").0);
    }

    #[tokio::test]
    async fn rejected_other_answer_keeps_prepared_decision_open() {
        let f = Fixture::new(
            FakeTransport::reachable().script_outcome(DeliveryOutcome::Refused {
                reason: "operator denied".into(),
            }),
        )
        .await;
        let id = f.ask().await;
        let mut worker = f
            .registry
            .get(&f.worker)
            .await
            .expect("read")
            .expect("worker");
        worker.harness = Harness::Claude;
        worker.proc = Some(ProcIdentity {
            pid: 123,
            proc_start: 456,
        });
        worker.semantic_state = None;
        f.registry.put(worker).await.expect("socket fixture");
        assert!(
            f.service
                .answer(&f.parent, &id, "main", false)
                .await
                .is_err()
        );
        let row = f
            .store
            .get_decision(&id)
            .await
            .expect("stored")
            .expect("row");
        assert_eq!(row.decision.state, DecisionState::Open);
        assert_eq!(
            row.decision.answer_msg_id.as_deref(),
            Some(format!("{id}-answer").as_str())
        );
        assert!(row.answer_seq.is_none());
    }

    #[tokio::test]
    async fn verify_requires_parent_and_names_the_exact_latest_done() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let reports = ReportService::new(
            f.registry.as_ref(),
            f.bus.as_ref(),
            || 100,
            ReportConfig::default(),
        );
        let first = reports
            .declare(&f.worker, Some(SemanticState::Done), None, None, &[])
            .await
            .expect("done");
        assert_eq!(
            f.service
                .verify(&f.worker, &f.worker, None)
                .await
                .expect_err("self verify refused")
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.service
                .verify(&SeatId::from("outsider"), &f.worker, None)
                .await
                .expect_err("outsider refused")
                .status,
            StatusCode::FORBIDDEN
        );
        let verified = f
            .service
            .verify(&f.parent, &f.worker, None)
            .await
            .expect("parent");
        assert_eq!(verified["done_seq"], first.0);
        let second = reports
            .declare(&f.worker, Some(SemanticState::Done), None, None, &[])
            .await
            .expect("new done");
        let anomalies = super::super::anomalies::AnomalyService::new(
            f.store.clone(),
            f.registry.clone(),
            f.bus.clone(),
            Arc::new(pij_testkit::fakes::FakeLiveness::new()),
        );
        let rows = anomalies
            .list(&BTreeMap::new(), None)
            .await
            .expect("derived anomalies");
        let row = rows["anomalies"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["kind"] == "unverified-done")
            .expect("new done unverified");
        assert_eq!(row["evidence"][0], second.0);
    }

    #[tokio::test]
    async fn delivered_dispatch_projection_consumes_the_canonical_event_literal() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let routes: Value = serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("routes");
        let events: Value = serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-events.json"
        ))
        .expect("events");
        let mut dispatch: pij_core::orchestration::Dispatch =
            serde_json::from_value(routes["fixture_context"]["records"]["dispatch"].clone())
                .expect("dispatch");
        dispatch.created_at = 1;
        f.store.create_dispatch(&dispatch).await.expect("create");
        f.store
            .mark_dispatch_delivered(&dispatch.id, 2)
            .await
            .expect("delivery evidence");
        let fixture = events["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|row| row["id"] == "dispatch-delivered")
            .expect("event");
        let event: pij_core::model::Event =
            serde_json::from_value(fixture["frame"]["event"].clone()).expect("canonical event");
        let seq = f.bus.publish(event).await.expect("publish");
        let anomalies = super::super::anomalies::AnomalyService::new(
            f.store.clone(),
            f.registry.clone(),
            f.bus.clone(),
            Arc::new(pij_testkit::fakes::FakeLiveness::new()),
        );
        let rows = anomalies
            .list(&BTreeMap::new(), None)
            .await
            .expect("projection");
        let row = rows["anomalies"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["kind"] == "delivered-unacked-stale")
            .expect("legacy anomaly kind");
        assert_eq!(row["evidence"][0], seq.0);
        assert_eq!(row["recordRef"], format!("dispatch:{}", dispatch.id));
    }

    #[tokio::test]
    async fn supersede_failed_delivery_mints_new_id_and_retries_stably() {
        let f = Fixture::new(FakeTransport::reachable().script_deliver_error()).await;
        let id = f.ask().await;
        f.socket_worker().await;
        assert_eq!(
            f.service
                .answer(&f.parent, &id, "old", false)
                .await
                .expect_err("injection failed")
                .code,
            "E-RS-ANSWER-DELIVERY"
        );
        let old_id = format!("{id}-answer");
        let answered = f
            .service
            .answer(&f.parent, &id, "replacement", true)
            .await
            .expect("explicit recovery");
        let fresh_id = answered["decision"]["answer_msg_id"]
            .as_str()
            .expect("new id");
        assert_ne!(fresh_id, old_id);
        assert_eq!(
            f.service
                .answer(&f.parent, &id, "replacement", false)
                .await
                .expect("ordinary retry"),
            answered
        );
        let delivered = f.transport.delivered();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].msg_id, fresh_id);
        assert_eq!(delivered[0].body, "replacement");
        let events = f.bus.tail(None, Seq(0)).await.expect("history");
        let event = events
            .iter()
            .find(|event| event.kind == "decision.answer-superseded")
            .expect("audit");
        let payload: Value = serde_json::from_str(&event.payload).expect("payload");
        assert_eq!(
            payload["superseded"],
            json!({"answer_msg_id":old_id,"answered_by":f.parent,"answer":"old"})
        );
        assert_eq!(payload["record"]["state"], "open");
        assert_eq!(
            fresh_id,
            format!("{id}-answer-{}", event.seq.expect("actual allocated seq").0)
        );
    }

    #[tokio::test]
    async fn supersede_delivered_answer_refuses_after_cache_eviction() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let id = f.ask().await;
        f.socket_worker().await;
        let answered = f
            .service
            .answer(&f.parent, &id, "old", false)
            .await
            .expect("delivered");
        let queue = SqliteQueue::new(f.pool.clone(), 30, 1).expect("one-entry retention");
        queue
            .note_delivered(
                &f.worker,
                "newer-message",
                DeliveryOrigin::InjectedToTransport,
            )
            .await
            .expect("evict old marker");
        let before = f.store.spine_head().await.expect("head");
        let error = f
            .service
            .answer(&f.parent, &id, "replacement", true)
            .await
            .expect_err("immutable delivered evidence");
        assert_eq!(error.code, "E-RS-ANSWER-DELIVERED");
        assert_eq!(
            error.details["answer_msg_id"],
            answered["decision"]["answer_msg_id"]
        );
        assert_eq!(
            f.store.spine_head().await.expect("no replacement event"),
            before
        );
        assert_eq!(f.transport.delivered().len(), 1);
    }

    #[tokio::test]
    async fn supersede_old_parent_and_asker_refuse() {
        let f = Fixture::new(FakeTransport::reachable().script_deliver_error()).await;
        let id = f.ask().await;
        f.socket_worker().await;
        assert!(
            f.service
                .answer(&f.parent, &id, "old", false)
                .await
                .is_err()
        );
        let new_parent = SeatId::from("replacement-parent");
        f.registry
            .put(SeatDescriptor::new(
                new_parent.clone(),
                Harness::Omp,
                "/work",
            ))
            .await
            .expect("new parent");
        let mut worker = f
            .registry
            .get(&f.worker)
            .await
            .expect("read")
            .expect("worker");
        worker.parent = Some(new_parent.clone());
        f.registry.put(worker).await.expect("reparent");
        let before = f.store.spine_head().await.expect("head");
        assert_eq!(
            f.service
                .answer(&f.parent, &id, "replacement", true)
                .await
                .expect_err("old parent")
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.service
                .answer(&f.worker, &id, "replacement", true)
                .await
                .expect_err("asker cannot supersede")
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.store.spine_head().await.expect("no refused events"),
            before
        );
        let answered = f
            .service
            .answer(&new_parent, &id, "replacement", true)
            .await
            .expect("current parent");
        assert_eq!(answered["decision"]["answered_by"], new_parent.as_str());
        assert_eq!(f.transport.delivered()[0].from, new_parent);
    }

    #[tokio::test]
    async fn supersede_pending_and_claimed_jobs_refuse() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let id = f.ask().await;
        let answered = f
            .service
            .answer(&f.parent, &id, "old", false)
            .await
            .expect("queued acceptance");
        let queue = SqliteQueue::new(f.pool.clone(), 30, 128).expect("shared queue");
        let before = f.store.spine_head().await.expect("head");
        for state in ["pending", "running"] {
            if state == "running" {
                queue
                    .claim(&[format!("delivery:{}", f.worker)], "supersede-test")
                    .await
                    .expect("claim")
                    .expect("job");
            }
            let error = f
                .service
                .answer(&f.parent, &id, "replacement", true)
                .await
                .expect_err("in transit");
            assert_eq!(error.code, "E-RS-ANSWER-IN-TRANSIT");
            assert_eq!(
                error.details["answer_msg_id"],
                answered["decision"]["answer_msg_id"]
            );
            assert_eq!(error.details["job_state"], state);
            assert!(error.message.contains(
                "an answer in transit cannot be withdrawn; wait for its terminal outcome"
            ));
        }
        let (job, _) = queue
            .peek(&[format!("delivery:{}", f.worker)])
            .await
            .expect("peek")
            .expect("running job");
        queue
            .ack_delivery(job, DeliveryOrigin::ReaderRead)
            .await
            .expect("acknowledge old message");
        assert_eq!(
            f.service
                .answer(&f.parent, &id, "replacement", true)
                .await
                .expect_err("acked answer")
                .code,
            "E-RS-ANSWER-DELIVERED"
        );
        assert_eq!(
            f.store.spine_head().await.expect("no replacement event"),
            before
        );
        assert_eq!(
            f.store
                .get_decision(&id)
                .await
                .expect("read")
                .expect("row")
                .decision
                .answer
                .as_deref(),
            Some("old")
        );
        assert!(f.transport.delivered().is_empty());
    }

    #[tokio::test]
    async fn supersede_terminal_failed_queue_job_reopens_before_readmission() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let id = f.ask().await;
        f.service
            .answer(&f.parent, &id, "old", false)
            .await
            .expect("queued acceptance");
        let queue = SqliteQueue::new(f.pool.clone(), 30, 128).expect("shared queue");
        let (job, _) = queue
            .claim(&[format!("delivery:{}", f.worker)], "supersede-test")
            .await
            .expect("claim")
            .expect("job");
        queue
            .ack(
                job,
                Outcome::Failed {
                    reason: "expired-unknown".into(),
                },
            )
            .await
            .expect("existing terminal failure");
        let answered = f
            .service
            .answer(&f.parent, &id, "replacement", true)
            .await
            .expect("terminal recovery");
        assert_ne!(
            answered["decision"]["answer_msg_id"],
            format!("{id}-answer")
        );
        let (_, pending) = queue
            .peek(&[format!("delivery:{}", f.worker)])
            .await
            .expect("peek")
            .expect("new job");
        assert_eq!(
            pending.dedupe_key,
            answered["decision"]["answer_msg_id"]
                .as_str()
                .expect("new id")
        );
        let history = f.bus.tail(None, Seq(0)).await.expect("events");
        let event = history
            .iter()
            .find(|event| event.kind == "decision.answer-superseded")
            .expect("audit");
        let payload: Value = serde_json::from_str(&event.payload).expect("payload");
        assert_eq!(payload["record"]["state"], "open");
        assert!(payload["record"]["answered_at"].is_null());
    }

    #[test]
    fn supersede_parser_consumes_canonical_typed_and_argv_requests() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("routes");
        let route = fixtures["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .find(|route| route["path"] == "/v1/answer")
            .expect("answer");
        let case = route["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|case| case["id"] == "decision-answer-supersede")
            .expect("supersede fixture");
        for key in ["request", "shim_request"] {
            let request: AnswerRequest =
                serde_json::from_value(case[key].clone()).expect("request");
            let (id, answer, supersede) = parse_answer(&request).expect("grammar");
            assert!(supersede);
            assert_eq!(id, case["request"]["decision"].as_str().expect("id"));
            assert_eq!(answer, case["request"]["answer"].as_str().expect("answer"));
        }
        for argv in [
            vec!["answer", "--supersede", "--supersede", "d", "text"],
            vec!["answer", "--unknown", "d", "text"],
            vec!["answer", "--supersede", "d"],
            vec!["answer", "d", "text", "extra"],
        ] {
            let request: AnswerRequest =
                serde_json::from_value(json!({"argv":argv})).expect("request");
            assert!(parse_answer(&request).is_err());
        }
        let request: AnswerRequest =
            serde_json::from_value(json!({"argv":["answer","d","text"],"supersede":false}))
                .expect("request");
        assert!(parse_answer(&request).is_err());
    }

    #[tokio::test]
    async fn supersede_direct_delivery_reservation_refuses_without_a_terminal_receipt() {
        let f = Fixture::new(FakeTransport::reachable()).await;
        let id = f.ask().await;
        let old_id = format!("{id}-answer");
        f.store
            .prepare_decision_answer(&id, &f.parent, "old", Some(&old_id))
            .await
            .expect("intent");
        let queue = SqliteQueue::new(f.pool.clone(), 30, 128).expect("shared queue");
        queue
            .note_delivered(&f.worker, &old_id, DeliveryOrigin::InjectedToTransport)
            .await
            .expect("pre-injection reservation");
        let error = f
            .service
            .answer(&f.parent, &id, "replacement", true)
            .await
            .expect_err("reservation has no terminal receipt");
        assert_eq!(error.code, "E-RS-ANSWER-IN-TRANSIT");
        assert_eq!(error.details["answer_msg_id"], old_id);
        assert_eq!(error.details["job_state"], "delivery-reserved");
        assert!(f.transport.delivered().is_empty());
    }

    #[tokio::test]
    async fn supersede_terminal_refusal_keeps_replacement_subject_to_delivery_policy() {
        let f = Fixture::new(
            FakeTransport::reachable().script_outcome(DeliveryOutcome::Refused {
                reason: "operator denied".into(),
            }),
        )
        .await;
        let id = f.ask().await;
        f.socket_worker().await;
        assert_eq!(
            f.service
                .answer(&f.parent, &id, "old", false)
                .await
                .expect_err("terminal refusal")
                .code,
            "E-RS-ANSWER-DELIVERY"
        );
        let error = f
            .service
            .answer(&f.parent, &id, "replacement", true)
            .await
            .expect_err("new answer is also refused");
        assert_eq!(error.code, "E-RS-ANSWER-DELIVERY");
        let row = f.store.get_decision(&id).await.expect("read").expect("row");
        assert_eq!(row.decision.answer.as_deref(), Some("replacement"));
        assert_ne!(
            row.decision.answer_msg_id.as_deref(),
            Some(format!("{id}-answer").as_str())
        );
        assert_eq!(row.decision.state, DecisionState::Open);
        assert!(f.transport.delivered().is_empty());
    }

    #[tokio::test]
    async fn supersede_transport_hold_remains_in_transit_after_cache_eviction() {
        let f = Fixture::new(
            FakeTransport::reachable().script_outcome(DeliveryOutcome::Held {
                reason: "approval pending".into(),
            }),
        )
        .await;
        let id = f.ask().await;
        f.socket_worker().await;
        assert!(
            f.service
                .answer(&f.parent, &id, "old", false)
                .await
                .is_err()
        );
        let queue = SqliteQueue::new(f.pool.clone(), 30, 1).expect("one-entry retention");
        queue
            .note_delivered(
                &f.worker,
                "newer-message",
                DeliveryOrigin::InjectedToTransport,
            )
            .await
            .expect("evict old marker");
        let error = f
            .service
            .answer(&f.parent, &id, "replacement", true)
            .await
            .expect_err("transport is still deciding");
        assert_eq!(error.code, "E-RS-ANSWER-IN-TRANSIT");
        assert_eq!(error.details["job_state"], "held");
        assert_eq!(
            f.store
                .get_decision(&id)
                .await
                .expect("read")
                .expect("row")
                .decision
                .answer
                .as_deref(),
            Some("old")
        );
        assert!(f.transport.delivered().is_empty());
    }

    async fn authority_fixture(
        queue_backend: AdapterChoice,
        spine_backend: AdapterChoice,
    ) -> (FreshStore, Config, crate::Services, SeatId, SeatId) {
        let file = FreshStore::new();
        let config = Config {
            store_path: file.path(),
            adapters: Adapters {
                registry: spine_backend,
                queue: queue_backend,
                spine: spine_backend,
                ..Adapters::default()
            },
            ..Config::default()
        };
        let services = crate::build_services(
            &config,
            std::path::Path::new("/unused-fake-tmux-u4-authority"),
        )
        .await
        .expect("composed backends");
        let contract: Value = serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("contract");
        let worker = SeatId::from(
            contract["fixture_context"]["worker"]
                .as_str()
                .expect("worker"),
        );
        let parent = SeatId::from(
            contract["fixture_context"]["parent"]
                .as_str()
                .expect("parent"),
        );
        let mut seat = SeatDescriptor::new(worker.clone(), Harness::Omp, "/work");
        seat.parent = Some(parent.clone());
        services.registry.put(seat).await.expect("worker");
        services
            .registry
            .put(SeatDescriptor::new(parent.clone(), Harness::Omp, "/work"))
            .await
            .expect("parent");
        (file, config, services, worker, parent)
    }

    #[tokio::test]
    async fn supersede_shared_real_persistence_allows_terminal_failure_after_reopen() {
        let (_file, config, services, worker, parent) =
            authority_fixture(AdapterChoice::Real, AdapterChoice::Real).await;
        let opened = services
            .decisions
            .question(&worker, "persist this recovery", None, &[])
            .await
            .expect("question");
        let id = opened["decision"]["id"].as_str().expect("id");
        let old = services
            .decisions
            .answer(&parent, id, "old", false)
            .await
            .expect("queued answer");
        assert_eq!(
            services
                .decisions
                .answer(&parent, id, "replacement", true)
                .await
                .expect_err("same database sees pending work")
                .code,
            "E-RS-ANSWER-IN-TRANSIT"
        );
        let kinds = [format!("delivery:{worker}")];
        let (job, _) = services
            .queue
            .claim(&kinds, "authority-test")
            .await
            .expect("claim")
            .expect("old job");
        services
            .queue
            .ack(
                job,
                Outcome::Failed {
                    reason: "terminal delivery failure".into(),
                },
            )
            .await
            .expect("terminal failure");
        drop(services);

        let services = crate::build_services(
            &config,
            std::path::Path::new("/unused-fake-tmux-u4-authority"),
        )
        .await
        .expect("reopen real persistence");
        let persisted = services
            .decisions
            .store
            .get_decision(id)
            .await
            .expect("read")
            .expect("durable decision");
        assert_eq!(
            serde_json::to_value(persisted.decision).expect("decision"),
            old["decision"]
        );
        let replacement = services
            .decisions
            .answer(&parent, id, "replacement", true)
            .await
            .expect("terminal failure allows recovery");
        assert_ne!(
            replacement["decision"]["answer_msg_id"],
            old["decision"]["answer_msg_id"]
        );
        let (_, pending) = services
            .queue
            .peek(&kinds)
            .await
            .expect("peek")
            .expect("new queue job");
        assert_eq!(
            pending.dedupe_key,
            replacement["decision"]["answer_msg_id"]
                .as_str()
                .expect("new id")
        );
        drop(services);

        let services = crate::build_services(
            &config,
            std::path::Path::new("/unused-fake-tmux-u4-authority"),
        )
        .await
        .expect("reopen replacement");
        assert_eq!(
            services
                .decisions
                .answer(&parent, id, "replacement", false)
                .await
                .expect("ordinary durable retry"),
            replacement
        );
        let history = services
            .event_bus
            .tail(None, Seq(0))
            .await
            .expect("durable history");
        let events: Vec<_> = history
            .iter()
            .filter(|event| event.kind == "decision.answer-superseded")
            .collect();
        assert_eq!(events.len(), 1);
        let audit: Value = serde_json::from_str(&events[0].payload).expect("audit");
        assert_eq!(
            audit["superseded"]["answer_msg_id"],
            old["decision"]["answer_msg_id"]
        );
        assert_eq!(
            audit["record"]["answer_msg_id"],
            replacement["decision"]["answer_msg_id"]
        );
        let inbox = services
            .delivery
            .claim_inbox(&worker, false)
            .await
            .expect("replacement inbox");
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].message.body, "replacement");
        assert_eq!(
            inbox[0].message.msg_id,
            replacement["decision"]["answer_msg_id"]
                .as_str()
                .expect("new id")
        );
    }

    async fn assert_authority_split(
        queue_backend: AdapterChoice,
        spine_backend: AdapterChoice,
        fixture_key: &str,
    ) {
        let (_file, _config, services, worker, parent) =
            authority_fixture(queue_backend, spine_backend).await;
        let contract: Value = serde_json::from_str(include_str!(
            "../../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("contract");
        let expected = &contract["refusals"][fixture_key];
        assert!(
            expected.is_object(),
            "canonical authority refusal {fixture_key}"
        );
        let opened = services
            .decisions
            .question(&worker, "ordinary question still works", None, &[])
            .await
            .expect("question");
        let id = opened["decision"]["id"].as_str().expect("id");
        let answered = services
            .decisions
            .answer(&parent, id, "old", false)
            .await
            .expect("ordinary answer still works");
        assert_eq!(answered["decision"]["state"], "answered");
        assert_eq!(
            services
                .decisions
                .answer(&parent, id, "old", false)
                .await
                .expect("ordinary identical retry"),
            answered
        );
        let before = services.decisions.store.spine_head().await.expect("head");
        let error = services
            .decisions
            .answer(&parent, id, "replacement", true)
            .await
            .expect_err("disconnected authority refuses");
        let response = error.response("pij answer");
        assert_eq!(
            u64::from(response.status().as_u16()),
            expected["http_status"].as_u64().expect("status")
        );
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("bounded envelope");
        let actual: Value = serde_json::from_slice(&bytes).expect("decodable refusal");
        let mut envelope = expected["response"].clone();
        let fixture_id = envelope["details"]["decision"]
            .as_str()
            .expect("fixture id")
            .to_string();
        envelope["meta"] = json!(
            envelope["meta"]
                .as_str()
                .expect("meta")
                .replace(&fixture_id, id)
        );
        envelope["details"]["decision"] = json!(id);
        assert_eq!(actual, envelope);
        assert_eq!(actual["details"]["queue_backend"], json!(queue_backend));
        assert_eq!(actual["details"]["spine_backend"], json!(spine_backend));
        assert_eq!(
            services
                .decisions
                .store
                .spine_head()
                .await
                .expect("no supersession event"),
            before
        );
        let row = services
            .decisions
            .store
            .get_decision(id)
            .await
            .expect("read")
            .expect("original row");
        assert_eq!(
            serde_json::to_value(row.decision).expect("decision"),
            answered["decision"]
        );
        let inbox = services
            .delivery
            .claim_inbox(&worker, false)
            .await
            .expect("old work remains deliverable");
        assert_eq!(inbox.len(), 1);
        assert_eq!(
            inbox[0].message.msg_id,
            answered["decision"]["answer_msg_id"]
                .as_str()
                .expect("old id")
        );
        assert_eq!(inbox[0].message.body, "old");
        let self_question = services
            .decisions
            .question(&worker, "self ruling still works", None, &[])
            .await
            .expect("second question");
        let self_answer = services
            .decisions
            .answer(
                &worker,
                self_question["decision"]["id"].as_str().expect("id"),
                "self ruling",
                false,
            )
            .await
            .expect("ordinary self answer");
        assert_eq!(self_answer["decision"]["state"], "answered");
        assert!(self_answer["decision"]["answer_msg_id"].is_null());
    }

    #[tokio::test]
    async fn supersede_real_queue_fake_spine_refuses_without_changing_ordinary_answers() {
        assert_authority_split(
            AdapterChoice::Real,
            AdapterChoice::Fake,
            "answer_authority_split_real_fake",
        )
        .await;
    }

    #[tokio::test]
    async fn supersede_fake_queue_real_spine_refuses_without_changing_ordinary_answers() {
        assert_authority_split(
            AdapterChoice::Fake,
            AdapterChoice::Real,
            "answer_authority_split_fake_real",
        )
        .await;
    }

    #[tokio::test]
    async fn supersede_fake_queue_fake_spine_refuses_without_changing_ordinary_answers() {
        assert_authority_split(
            AdapterChoice::Fake,
            AdapterChoice::Fake,
            "answer_authority_split_fake_fake",
        )
        .await;
    }
}
