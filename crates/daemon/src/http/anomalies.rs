//! One bulk rs snapshot projected by the existing pure anomaly detectors.
use super::decisions::{
    DecisionError, ReadRequest, here_from_flag, here_path, parse_filters, project_parents,
    query_argv, respond,
};
use super::identity::{Resolved, resolve_seat};
use super::{AppState, system_time_ms};
use crate::events::EventBus;
use axum::extract::{Json, Query, State};
use axum::response::Response;
use pij_core::anomalies::{
    ActivityFact, AnomalyThresholds, AnomalyView, DeadSeatFact, DispatchFact, DoneFact, scan_all,
};
use pij_core::decisions::Decision;
use pij_core::error::Result;
use pij_core::liveness::alive;
use pij_core::model::{Card, Liveness, SeatDescriptor, Seq};
use pij_core::orchestration::PrimeState;
use pij_core::ports::{LivenessPort, Registry, SeatFilter, Spine};
use pij_core::report::CardRecord;
use pij_store::SqliteOrchestration;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Read-only anomaly service. Clones the existing SQL adapter, never a second pool.
pub struct AnomalyService {
    store: SqliteOrchestration,
    registry: Arc<dyn Registry>,
    bus: Arc<EventBus>,
    liveness: Arc<dyn LivenessPort>,
}

impl AnomalyService {
    /// Compose from the same persistence and host observations as governance/reap.
    pub fn new(
        store: SqliteOrchestration,
        registry: Arc<dyn Registry>,
        bus: Arc<EventBus>,
        liveness: Arc<dyn LivenessPort>,
    ) -> Self {
        Self {
            store,
            registry,
            bus,
            liveness,
        }
    }

    /// Derive rows from real records and source seqs in bounded bulk reads.
    pub async fn list(
        &self,
        filters: &BTreeMap<String, String>,
        here: Option<&str>,
    ) -> Result<Value> {
        let seats = self.registry.list(SeatFilter::default()).await?;
        let tasks = self.store.list_tasks(None).await?;
        let dispatches = self.store.list_dispatches(None).await?;
        let mut decisions: Vec<Decision> = self.store.list_decisions().await?;
        let prime = self
            .store
            .prime()
            .await?
            .filter(|row| row.state == PrimeState::Current)
            .map(|row| row.seat);
        project_parents(&mut decisions, &seats, prime.as_ref());
        let events = self.bus.tail(None, Seq(0)).await?;
        let cursor = events.last().and_then(|event| event.seq).unwrap_or(Seq(0));
        let now = system_time_ms()?;
        let mut cards = Vec::new();
        let mut activity = Vec::new();
        let mut done = BTreeMap::new();
        let mut verified = BTreeMap::new();
        let mut delivered = BTreeMap::new();
        let mut registry_events = BTreeMap::new();
        for event in &events {
            let Some(seq) = event.seq else {
                continue;
            };
            let Some(seat) = event.seat.as_ref() else {
                continue;
            };
            activity.push(ActivityFact {
                seat: seat.clone(),
                at: event.at,
                seq,
            });
            match event.kind.as_str() {
                "report.now" => {
                    if let Ok(record) = serde_json::from_str::<CardRecord>(&event.payload) {
                        cards.push(Card {
                            seat: seat.clone(),
                            did: record.did,
                            next: record.next,
                            at: event.at,
                            seq: Some(seq),
                        });
                    }
                }
                "report.state" => {
                    if let Ok(payload) = serde_json::from_str::<Value>(&event.payload)
                        && payload["state"] == "done"
                    {
                        let assignment = payload
                            .get("assignment_id")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        done.insert(
                            (seat.clone(), assignment.clone()),
                            DoneFact {
                                seat: seat.clone(),
                                assignment_id: assignment,
                                done_seq: seq,
                                verified_seq: None,
                                verified_done_seq: None,
                            },
                        );
                    }
                }
                "state-verified" => {
                    if let Ok(payload) = serde_json::from_str::<Value>(&event.payload)
                        && let Some(done_seq) = payload["done_seq"].as_u64()
                    {
                        verified.insert((seat.clone(), Seq(done_seq)), seq);
                    }
                }
                "dispatch" => {
                    if let Ok(payload) = serde_json::from_str::<Value>(&event.payload)
                        && payload["action"] == "delivered"
                        && let Some(id) = payload["record"]["id"].as_str()
                    {
                        delivered.insert(id.to_string(), seq);
                    }
                }
                "seat.put" => {
                    if let Ok(record) = serde_json::from_str::<SeatDescriptor>(&event.payload) {
                        registry_events.insert(seat.clone(), (record.proc, seq));
                    }
                }
                _ => {}
            }
        }
        let mut done: Vec<_> = done.into_values().collect();
        for fact in &mut done {
            if let Some(seq) = verified.get(&(fact.seat.clone(), fact.done_seq)) {
                fact.verified_seq = Some(*seq);
                fact.verified_done_seq = Some(fact.done_seq);
            }
        }
        let dispatch_facts: Vec<_> = dispatches
            .into_iter()
            .filter_map(|row| {
                Some(DispatchFact {
                    seat: row.to,
                    dispatch_id: row.id.clone(),
                    packet_sha256: row.packet_sha256?,
                    delivered_at: row.delivered_at?,
                    delivered_seq: *delivered.get(&row.id)?,
                    acknowledged: row.acknowledged_at.is_some(),
                })
            })
            .collect();
        let mut dead = Vec::new();
        for seat in &seats {
            if seat.tombstoned_at.is_some() {
                continue;
            }
            let Some(proc) = seat.proc else {
                continue;
            };
            let Some((recorded, seq)) = registry_events.get(&seat.id) else {
                continue;
            };
            if *recorded != Some(proc) {
                continue;
            }
            if matches!(
                alive(proc, self.liveness.as_ref()).await?,
                Liveness::Dead { .. } | Liveness::Recycled { .. }
            ) {
                dead.push(DeadSeatFact {
                    seat: seat.id.clone(),
                    seq: *seq,
                });
            }
        }
        let view = AnomalyView {
            now_ms: now,
            thresholds: AnomalyThresholds::default(),
            seats: &seats,
            cards: &cards,
            activity: &activity,
            dispatches: &dispatch_facts,
            done: &done,
            dispositions: &events,
            decisions: &decisions,
            dead: &dead,
        };
        let rows = scan_all(&view);
        let by_id: BTreeMap<_, _> = seats.iter().map(|seat| (&seat.id, seat)).collect();
        let task_by_id: BTreeMap<_, _> =
            tasks.iter().map(|task| (task.id.as_str(), task)).collect();
        let filtered: Vec<_> = rows
            .iter()
            .filter(|row| {
                filters.get("seat").is_none_or(|id| row.seat.as_str() == id)
                    && here.is_none_or(|folder| {
                        by_id
                            .get(&row.seat)
                            .is_some_and(|seat| seat.folder == folder)
                    })
                    && filters.get("project").is_none_or(|project| {
                        row.assignment_id
                            .as_deref()
                            .and_then(|id| task_by_id.get(id))
                            .is_some_and(|task| {
                                task.node_id == row.seat && task.project.as_ref() == Some(project)
                            })
                    })
            })
            .map(|row| row.wire())
            .collect();
        Ok(json!({"anomalies":filtered,"cursor":cursor}))
    }
}

pub(crate) async fn list_post(
    State(state): State<AppState>,
    Json(request): Json<ReadRequest>,
) -> Response {
    let actor = match resolve_seat(
        &state,
        "pij anomalies",
        request.caller.session_id,
        request.caller.pane,
    )
    .await
    {
        Resolved::Seat(seat, _) => seat,
        Resolved::Refusal(response) => return response,
    };
    let filters = match parse_filters(
        &request.argv,
        "anomalies",
        &["here", "project", "seat"],
        &["here"],
    ) {
        Ok(filters) => filters,
        Err(error) => return error.anomaly_scope().response("pij anomalies"),
    };
    let here = match here_from_flag(
        filters.get("here").map(String::as_str),
        Some(request.caller.cwd.as_deref().unwrap_or(&actor.folder)),
    )
    .and_then(here_path)
    {
        Ok(here) => here,
        Err(error) => return error.anomaly_scope().response("pij anomalies"),
    };
    respond(
        "pij anomalies",
        state
            .services
            .anomalies
            .list(&filters, here.as_deref())
            .await
            .map_err(DecisionError::from),
    )
}

pub(crate) async fn list_get(
    State(state): State<AppState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let argv = query_argv("anomalies", query);
    let filters = match parse_filters(&argv, "anomalies", &["here", "project", "seat"], &["here"]) {
        Ok(filters) => filters,
        Err(error) => return error.anomaly_scope().response("pij anomalies"),
    };
    let here = match here_path(filters.get("here").map(String::as_str)) {
        Ok(here) => here,
        Err(error) => return error.anomaly_scope().response("pij anomalies"),
    };
    respond(
        "pij anomalies",
        state
            .services
            .anomalies
            .list(&filters, here.as_deref())
            .await
            .map_err(DecisionError::from),
    )
}
