//! Per-seat indexed spine reads projected by the existing pure anomaly detectors.
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
    ANOMALY_ACK_KIND, ANOMALY_CLEAR_KIND, ActivityFact, AnomalyThresholds, AnomalyView,
    DeadSeatFact, DispatchFact, DoneFact, scan_all,
};
use pij_core::decisions::Decision;
use pij_core::error::Result;
use pij_core::liveness::alive;
use pij_core::model::{Card, Event, Liveness, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::orchestration::{Dispatch, PrimeState, TaskAssignment};
use pij_core::ports::{LivenessPort, Registry, SeatFilter, Spine};
use pij_core::report::CardRecord;
use pij_store::SqliteOrchestration;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
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

    /// Derive rows from real records and per-seat indexed spine reads.
    ///
    /// Reads follow the registry, never history (issue #119): it folded the
    /// whole spine on every call, 4.18M rows and a 6.5 GB daemon peak.
    pub async fn list(
        &self,
        filters: &BTreeMap<String, String>,
        here: Option<&str>,
    ) -> Result<Value> {
        self.list_at(filters, here, system_time_ms()?).await
    }

    async fn list_at(
        &self,
        filters: &BTreeMap<String, String>,
        here: Option<&str>,
        now: u64,
    ) -> Result<Value> {
        let records = self.records().await?;
        // The cursor first: every fact read below is at or after it.
        let cursor = self.bus.newest().await?;
        let mut fold = Fold::default();
        for seat in &records.seats {
            let live = seat.tombstoned_at.is_none();
            // Done claims stay visible after a tombstone, as they always did.
            let kinds: &[&str] = if live {
                &[REPORT_NOW, REPORT_STATE, STATE_VERIFIED]
            } else {
                &[REPORT_STATE, STATE_VERIFIED]
            };
            for event in self.bus.all_matching(&seat.id, kinds).await? {
                fold.absorb(&event);
            }
            if !live {
                continue;
            }
            if let Some(event) = self.bus.latest(&seat.id).await? {
                fold.activity(&event);
            }
            if seat.proc.is_some() {
                self.latest_seat_put(&seat.id, &mut fold).await?;
            }
        }
        let recipients: BTreeSet<&SeatId> = records
            .dispatches
            .iter()
            .filter(|row| row.packet_sha256.is_some() && row.delivered_at.is_some())
            .map(|row| &row.to)
            .collect();
        for seat in recipients {
            for event in self.bus.all_matching(seat, &[DISPATCH]).await? {
                fold.absorb(&event);
            }
        }
        let bus = Arc::clone(&self.bus);
        self.project(
            Snapshot {
                records,
                fold,
                cursor,
            },
            now,
            filters,
            here,
            |seats| async move {
                // A disposition is written under the anomaly's own seat
                // (`disposition_event`), so the rows' seats hold every one.
                let mut events = Vec::new();
                for seat in seats {
                    events.extend(bus.all_matching(&seat, &DISPOSITION_KINDS).await?);
                }
                Ok(events)
            },
        )
        .await
    }

    async fn records(&self) -> Result<Records> {
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
        Ok(Records {
            seats,
            tasks,
            dispatches,
            decisions,
        })
    }

    /// The seat's newest `seat.put` that decodes: one seek, and the seat's
    /// `seat.put` rows only if that newest one does not decode.
    async fn latest_seat_put(&self, seat: &SeatId, fold: &mut Fold) -> Result<()> {
        let Some(event) = self.bus.latest_matching(seat, &[SEAT_PUT]).await? else {
            return Ok(());
        };
        if serde_json::from_str::<SeatDescriptor>(&event.payload).is_ok() {
            fold.absorb(&event);
            return Ok(());
        }
        for event in self.bus.all_matching(seat, &[SEAT_PUT]).await? {
            fold.absorb(&event);
        }
        Ok(())
    }

    /// Detect, dispose and filter. `dispositions` reads the ack and clear
    /// events for the seats the undisposed rows name.
    async fn project<F, Fut>(
        &self,
        snapshot: Snapshot,
        now: u64,
        filters: &BTreeMap<String, String>,
        here: Option<&str>,
        dispositions: F,
    ) -> Result<Value>
    where
        F: FnOnce(BTreeSet<SeatId>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Event>>>,
    {
        let Snapshot {
            records:
                Records {
                    seats,
                    tasks,
                    dispatches,
                    decisions,
                },
            fold,
            cursor,
        } = snapshot;
        let Fold {
            cards,
            activity,
            done,
            verified,
            delivered,
            registry_events,
        } = fold;
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
        let mut view = AnomalyView {
            now_ms: now,
            thresholds: AnomalyThresholds::default(),
            seats: &seats,
            cards: &cards,
            activity: &activity,
            dispatches: &dispatch_facts,
            done: &done,
            dispositions: &[],
            decisions: &decisions,
            dead: &dead,
        };
        let undisposed = scan_all(&view);
        let disposition_events =
            dispositions(undisposed.iter().map(|row| row.seat.clone()).collect()).await?;
        view.dispositions = &disposition_events;
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

const REPORT_NOW: &str = "report.now";
const REPORT_STATE: &str = "report.state";
const STATE_VERIFIED: &str = "state-verified";
const DISPATCH: &str = "dispatch";
const SEAT_PUT: &str = "seat.put";
const DISPOSITION_KINDS: [&str; 2] = [ANOMALY_ACK_KIND, ANOMALY_CLEAR_KIND];

/// Governance records the anomaly view reads from the store, not the spine.
struct Records {
    seats: Vec<SeatDescriptor>,
    tasks: Vec<TaskAssignment>,
    dispatches: Vec<Dispatch>,
    decisions: Vec<Decision>,
}

/// Everything one anomaly answer is computed from.
struct Snapshot {
    records: Records,
    fold: Fold,
    cursor: Seq,
}

/// The spine facts the detectors consume, decoded one event at a time.
///
/// Folding an event twice is harmless and the order across seats does not
/// matter: every keyed fact is per seat (or per dispatch id, whose delivered
/// event is written under its one recipient), and within a seat the reads
/// arrive oldest first, so the newest row wins exactly as a whole-spine fold.
#[derive(Default)]
struct Fold {
    cards: Vec<Card>,
    activity: Vec<ActivityFact>,
    done: BTreeMap<(SeatId, Option<String>), DoneFact>,
    verified: BTreeMap<(SeatId, Seq), Seq>,
    delivered: BTreeMap<String, Seq>,
    registry_events: BTreeMap<SeatId, (Option<ProcIdentity>, Seq)>,
}

impl Fold {
    /// Activity feeds only a status-stale row's `observable`, which the wire
    /// shape does not carry; one newest row per seat is that evidence.
    fn activity(&mut self, event: &Event) {
        if let (Some(seq), Some(seat)) = (event.seq, event.seat.as_ref()) {
            self.activity.push(ActivityFact {
                seat: seat.clone(),
                at: event.at,
                seq,
            });
        }
    }

    fn absorb(&mut self, event: &Event) {
        let Some(seq) = event.seq else {
            return;
        };
        let Some(seat) = event.seat.as_ref() else {
            return;
        };
        match event.kind.as_str() {
            REPORT_NOW => {
                if let Ok(record) = serde_json::from_str::<CardRecord>(&event.payload) {
                    self.cards.push(Card {
                        seat: seat.clone(),
                        did: record.did,
                        next: record.next,
                        at: event.at,
                        seq: Some(seq),
                    });
                }
            }
            REPORT_STATE => {
                if let Ok(payload) = serde_json::from_str::<Value>(&event.payload)
                    && payload["state"] == "done"
                {
                    let assignment = payload
                        .get("assignment_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    self.done.insert(
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
            STATE_VERIFIED => {
                if let Ok(payload) = serde_json::from_str::<Value>(&event.payload)
                    && let Some(done_seq) = payload["done_seq"].as_u64()
                {
                    self.verified.insert((seat.clone(), Seq(done_seq)), seq);
                }
            }
            DISPATCH => {
                if let Ok(payload) = serde_json::from_str::<Value>(&event.payload)
                    && payload["action"] == "delivered"
                    && let Some(id) = payload["record"]["id"].as_str()
                {
                    self.delivered.insert(id.to_string(), seq);
                }
            }
            SEAT_PUT => {
                if let Ok(record) = serde_json::from_str::<SeatDescriptor>(&event.payload) {
                    self.registry_events
                        .insert(seat.clone(), (record.proc, seq));
                }
            }
            _ => {}
        }
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

#[cfg(test)]
#[path = "anomalies_tests.rs"]
mod tests;
