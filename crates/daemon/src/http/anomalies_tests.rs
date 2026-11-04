//! Issue #119: the per-seat indexed view gives exactly the answers the
//! whole-spine fold gave.
//!
//! The oracle below is the old read: every event, folded through the same
//! decoding ([`Fold::absorb`]), with every event offered as a disposition. The
//! decoding is shared, so these tests prove what changed: which rows are read.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use pij_core::model::{Harness, SemanticState};
use pij_core::ports::SpineWindow;
use pij_store::{SqliteRegistry, SqliteSpine};
use pij_testkit::fakes::FakeLiveness;
use serde_json::json;

use super::*;

const NOW: u64 = 1_800_000_000_000;
const MINUTE: u64 = 60_000;
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

impl AnomalyService {
    /// The read #119 replaced, kept as the oracle.
    async fn list_by_fold(
        &self,
        filters: &BTreeMap<String, String>,
        here: Option<&str>,
        now: u64,
    ) -> Result<Value> {
        let records = self.records().await?;
        let events = self.bus.tail(None, Seq(0)).await?;
        let cursor = events.last().and_then(|event| event.seq).unwrap_or(Seq(0));
        let mut fold = Fold::default();
        for event in &events {
            fold.activity(event);
            fold.absorb(event);
        }
        self.project(records, fold, cursor, now, filters, here, |_| async move {
            Ok(events)
        })
        .await
    }
}

/// A spine that counts, and can refuse, the unbounded read.
pub(super) struct GuardSpine {
    pub(super) inner: SqliteSpine,
    pub(super) refuse_tail: AtomicBool,
    pub(super) tail_calls: AtomicUsize,
}

#[async_trait]
impl Spine for GuardSpine {
    async fn append(&self, event: Event) -> Result<Seq> {
        self.inner.append(event).await
    }
    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        self.tail_calls.fetch_add(1, Ordering::SeqCst);
        if self.refuse_tail.load(Ordering::SeqCst) {
            return Err(pij_core::error::PijError::Adapter {
                adapter: "guard".into(),
                message: "unbounded Spine::tail on a bounded path (#119)".into(),
            });
        }
        self.inner.tail(seat, since).await
    }
    async fn tail_page(&self, since: Seq, limit: usize) -> Result<Vec<Event>> {
        self.inner.tail_page(since, limit).await
    }
    async fn newest(&self) -> Result<Seq> {
        self.inner.newest().await
    }
    async fn latest(&self, seat: &SeatId) -> Result<Option<Event>> {
        self.inner.latest(seat).await
    }
    async fn seat_page(&self, seat: &SeatId, since: Seq, limit: usize) -> Result<Vec<Event>> {
        self.inner.seat_page(seat, since, limit).await
    }
    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        self.inner.latest_matching(seat, kinds).await
    }
    async fn all_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Vec<Event>> {
        self.inner.all_matching(seat, kinds).await
    }
    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        self.inner.latest_matching_message(seat, kind, msg_id).await
    }
    async fn matching_since(&self, seat: &SeatId, window: &SpineWindow) -> Result<Vec<Event>> {
        self.inner.matching_since(seat, window).await
    }
}

pub(super) struct World {
    pub(super) pool: pij_store::StorePool,
    pub(super) store: SqliteOrchestration,
    pub(super) bus: Arc<EventBus>,
    pub(super) registry: Arc<SqliteRegistry>,
    pub(super) spine: Arc<GuardSpine>,
}

impl World {
    pub(super) async fn new(path: &str) -> Self {
        let pool = pij_store::open(path).await.expect("store");
        let spine = Arc::new(GuardSpine {
            inner: SqliteSpine::new(pool.clone()),
            refuse_tail: AtomicBool::new(false),
            tail_calls: AtomicUsize::new(0),
        });
        let raw: Arc<dyn Spine> = spine.clone();
        let bus = Arc::new(EventBus::new(raw, 1_024).expect("bus"));
        let registry = Arc::new(SqliteRegistry::new(pool.clone(), bus.clone()));
        Self {
            store: SqliteOrchestration::new(pool.clone()),
            pool,
            bus,
            registry,
            spine,
        }
    }

    pub(super) fn service(&self, liveness: FakeLiveness) -> AnomalyService {
        AnomalyService::new(
            self.store.clone(),
            self.registry.clone(),
            self.bus.clone(),
            Arc::new(liveness),
        )
    }

    async fn seat(&self, id: &str, folder: &str, edit: impl FnOnce(&mut SeatDescriptor)) {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, folder);
        edit(&mut seat);
        self.registry.put(seat).await.expect("seat");
    }

    async fn event(&self, seat: &str, kind: &str, at: u64, payload: Value) -> u64 {
        self.raw(seat, kind, at, payload.to_string()).await
    }

    async fn raw(&self, seat: &str, kind: &str, at: u64, payload: String) -> u64 {
        self.bus
            .publish(Event {
                seq: None,
                v: 1,
                at,
                kind: kind.into(),
                seat: Some(SeatId::from(seat)),
                payload,
            })
            .await
            .expect("publish")
            .0
    }

    async fn card(&self, seat: &str, at: u64) -> u64 {
        self.event(
            seat,
            "report.now",
            at,
            json!({"did": "work", "next": "more"}),
        )
        .await
    }

    async fn dispose(&self, kind: &str, occurrence: &str) {
        let seat = occurrence.split(':').nth(1).expect("occurrence seat");
        self.event(
            seat,
            kind,
            NOW,
            json!({"occurrence": occurrence, "actor": "pij-pa"}),
        )
        .await;
    }

    async fn dispatch(&self, id: &str, to: &str, delivered_event: bool) {
        let dispatch = Dispatch {
            id: id.into(),
            from: SeatId::from("pij-pm"),
            to: SeatId::from(to),
            packet_path: "/work/packet.md".into(),
            packet_sha256: Some(SHA.into()),
            msg_id: Some(id.into()),
            state: pij_core::orchestration::DispatchState::Queued,
            created_at: NOW - 60 * MINUTE,
            delivered_at: None,
            acknowledged_at: None,
            ack: None,
            canary: None,
        };
        self.store
            .create_dispatch(&dispatch)
            .await
            .expect("dispatch");
        self.store
            .mark_dispatch_delivered(id, NOW - 50 * MINUTE)
            .await
            .expect("delivered");
        if delivered_event {
            self.event(
                to,
                "dispatch",
                NOW - 50 * MINUTE,
                json!({"actor": "pij-pm", "action": "delivered", "record": {"id": id}}),
            )
            .await;
        }
    }
}

/// Every anomaly kind and every edge the indexed reads must reproduce.
async fn rich_world() -> (World, FakeLiveness) {
    let world = World::new("").await;
    let alive = ProcIdentity {
        pid: 41,
        proc_start: 1,
    };
    let stale = NOW - 45 * MINUTE;

    // status-stale, then acknowledged.
    world.seat("pij-stale", "/work/a", |_| {}).await;
    let card = world.card("pij-stale", stale).await;
    world
        .dispose(ANOMALY_ACK_KIND, &format!("status-stale:pij-stale:{card}"))
        .await;
    // status-stale, then cleared: no row.
    world.seat("pij-cleared", "/work/a", |_| {}).await;
    let card = world.card("pij-cleared", stale).await;
    world
        .dispose(
            ANOMALY_CLEAR_KIND,
            &format!("status-stale:pij-cleared:{card}"),
        )
        .await;
    // A disposition naming another occurrence must not touch this one.
    world.seat("pij-misacked", "/work/b", |_| {}).await;
    world.card("pij-misacked", stale).await;
    world
        .dispose(ANOMALY_CLEAR_KIND, "status-stale:pij-misacked:1")
        .await;
    // Fresh and parked cards: no row.
    world.seat("pij-fresh", "/work/a", |_| {}).await;
    world.card("pij-fresh", NOW - MINUTE).await;
    world
        .seat("pij-parked", "/work/a", |seat| {
            seat.semantic_state = Some(SemanticState::Waiting);
        })
        .await;
    world.card("pij-parked", stale).await;
    // Two cards, the later append carrying the earlier time: (at, seq) decides.
    world.seat("pij-skewed", "/work/b", |_| {}).await;
    world.card("pij-skewed", NOW - 40 * MINUTE).await;
    world.card("pij-skewed", NOW - 90 * MINUTE).await;
    // An undecodable newest card leaves the older one in charge.
    world.seat("pij-garbled", "/work/b", |_| {}).await;
    world.card("pij-garbled", stale).await;
    world
        .raw("pij-garbled", "report.now", NOW - MINUTE, "not json".into())
        .await;

    // unverified-done: one verified, one not, one on a tombstoned seat.
    world
        .seat("pij-worker", "/work/a", |seat| {
            seat.parent = Some(SeatId::from("pij-stale"));
        })
        .await;
    let first = world
        .event(
            "pij-worker",
            "report.state",
            NOW - 30 * MINUTE,
            json!({"state": "done", "assignment_id": "a1"}),
        )
        .await;
    world
        .event(
            "pij-worker",
            "state-verified",
            NOW - 29 * MINUTE,
            json!({"done_seq": first}),
        )
        .await;
    world
        .event(
            "pij-worker",
            "report.state",
            NOW - 20 * MINUTE,
            json!({"state": "done", "assignment_id": "a2"}),
        )
        .await;
    world.seat("pij-gone", "/work/a", |_| {}).await;
    world.card("pij-gone", stale).await;
    world
        .event(
            "pij-gone",
            "report.state",
            NOW - 30 * MINUTE,
            json!({"state": "done"}),
        )
        .await;
    world
        .registry
        .tombstone(&SeatId::from("pij-gone"), "test")
        .await
        .expect("tombstone");

    // dead-seat: a revived seat whose current incarnation is not alive; a live
    // one; and one whose newest seat.put does not decode.
    world
        .seat("pij-revived", "/work/a", |seat| {
            seat.proc = Some(ProcIdentity {
                pid: 7,
                proc_start: 1,
            });
        })
        .await;
    world
        .seat("pij-revived", "/work/a", |seat| {
            seat.proc = Some(ProcIdentity {
                pid: 8,
                proc_start: 2,
            });
        })
        .await;
    world
        .seat("pij-alive", "/work/a", |seat| seat.proc = Some(alive))
        .await;
    world
        .seat("pij-patched", "/work/b", |seat| {
            seat.proc = Some(ProcIdentity {
                pid: 9,
                proc_start: 3,
            });
        })
        .await;
    world
        .raw(
            "pij-patched",
            "seat.put",
            NOW,
            "{\"not\":\"a seat\"}".into(),
        )
        .await;

    // delivered-unacked-stale: with its delivered event; without one, no fact.
    world.dispatch("dispatch-late", "pij-worker", true).await;
    world.dispatch("dispatch-silent", "pij-fresh", false).await;

    // open-question; its age row cleared under the parent's seat.
    let (_, decision) = world
        .store
        .open_decision_committed(
            "q-1",
            &SeatId::from("pij-worker"),
            Some(&SeatId::from("pij-stale")),
            "which branch?",
            NOW - 45 * MINUTE,
        )
        .await
        .expect("question");
    world
        .dispose(
            ANOMALY_CLEAR_KIND,
            &format!("status-stale:pij-stale:{}", decision.question_seq.0),
        )
        .await;

    // A seat that is mostly junk history.
    world.seat("pij-noisy", "/work/b", |_| {}).await;
    for at in 0..2_000 {
        world.event("pij-noisy", "bg.lost", at, json!({})).await;
    }
    world.card("pij-noisy", stale).await;
    for at in 0..2_000 {
        world
            .event("pij-noisy", "bg.lost", NOW - at, json!({}))
            .await;
    }

    (world, FakeLiveness::new().with_proc(alive))
}

fn kinds(rows: &Value) -> Vec<(String, String)> {
    let mut kinds: Vec<_> = rows["anomalies"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                row["kind"].as_str().expect("kind").to_string(),
                row["nodeId"].as_str().expect("seat").to_string(),
            )
        })
        .collect();
    kinds.sort();
    kinds
}

#[tokio::test]
async fn indexed_reads_answer_exactly_as_the_whole_spine_fold() {
    let (world, liveness) = rich_world().await;
    let service = world.service(liveness);
    let scopes: Vec<(BTreeMap<String, String>, Option<&str>)> = vec![
        (BTreeMap::new(), None),
        (BTreeMap::new(), Some("/work/a")),
        (BTreeMap::new(), Some("/work/b")),
        (
            BTreeMap::from([("seat".to_string(), "pij-worker".to_string())]),
            None,
        ),
        (
            BTreeMap::from([("seat".to_string(), "pij-gone".to_string())]),
            None,
        ),
        (
            BTreeMap::from([("project".to_string(), "nowhere".to_string())]),
            None,
        ),
    ];
    for (filters, here) in &scopes {
        world.spine.refuse_tail.store(true, Ordering::SeqCst);
        let indexed = service
            .list_at(filters, *here, NOW)
            .await
            .expect("indexed reads never call tail");
        world.spine.refuse_tail.store(false, Ordering::SeqCst);
        let folded = service
            .list_by_fold(filters, *here, NOW)
            .await
            .expect("fold");
        assert_eq!(indexed, folded, "filters={filters:?} here={here:?}");
    }

    // The instrument can see a positive: the unscoped answer is the expected
    // set, so equality above is not two empty lists agreeing.
    let unscoped = service
        .list_at(&BTreeMap::new(), None, NOW)
        .await
        .expect("unscoped");
    let expected = [
        ("dead-seat", "pij-patched"),
        ("dead-seat", "pij-revived"),
        ("delivered-unacked-stale", "pij-worker"),
        ("open-question", "pij-stale"),
        ("status-stale", "pij-garbled"),
        ("status-stale", "pij-misacked"),
        ("status-stale", "pij-noisy"),
        ("status-stale", "pij-skewed"),
        ("status-stale", "pij-stale"),
        ("unverified-done", "pij-gone"),
        ("unverified-done", "pij-worker"),
    ]
    .map(|(kind, seat)| (kind.to_string(), seat.to_string()));
    assert_eq!(kinds(&unscoped), expected.to_vec());
    assert_eq!(
        unscoped["cursor"],
        json!(service.bus.newest().await.expect("head").0)
    );
}

async fn query_plan(pool: &pij_store::StorePool, sql: &str, binds: &[&str]) -> Vec<String> {
    let explain = format!("EXPLAIN QUERY PLAN {sql}");
    let mut query = sqlx::query(&explain);
    for value in binds {
        query = query.bind(*value);
    }
    query
        .fetch_all(pool)
        .await
        .expect("plan")
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, "detail"))
        .collect()
}

/// Issue #119, AC4: over a million spine rows, most of them one seat's junk,
/// every statement the three verbs now issue seeks an index, never scans
/// `spine_events`, and each verb's reads finish inside a stated bound. The
/// spine refuses `tail` throughout, so none of them can fall back to it.
#[tokio::test]
async fn bounded_spine_reads_over_a_million_rows() {
    // In memory: the scratch spine costs no disk on a shared machine.
    let world = World::new("").await;
    for n in 0..24 {
        let seat = format!("pij-seat-{n}");
        world
            .seat(&seat, "/work", |seat| {
                seat.proc = Some(ProcIdentity {
                    pid: 100 + n,
                    proc_start: 1,
                })
            })
            .await;
        world.card(&seat, NOW - 45 * MINUTE).await;
        world
            .event(
                &seat,
                "report.state",
                NOW - 40 * MINUTE,
                json!({"state": "done"}),
            )
            .await;
    }
    world.seat("pij-junk", "/work", |_| {}).await;
    sqlx::raw_sql(
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1000000) \
         INSERT INTO spine_events (v, at, kind, seat, payload) \
         SELECT 1, x, 'bg.lost', 'pij-junk', '{}' FROM n",
    )
    .execute(&world.pool)
    .await
    .expect("a million junk rows");
    let rows_seeded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM spine_events")
        .fetch_one(&world.pool)
        .await
        .expect("count");
    assert!(rows_seeded >= 1_000_000, "{rows_seeded} rows");

    let statements: Vec<(String, Vec<&str>)> = vec![
        (pij_store::spine::SEAT_LATEST_SQL.into(), vec!["pij-junk"]),
        (
            pij_store::spine::SEAT_PAGE_SQL.into(),
            vec!["pij-junk", "0", "5001"],
        ),
        (
            pij_store::spine::LATEST_MATCHING_SQL.into(),
            vec!["pij-junk", "seat.put"],
        ),
        (
            pij_store::spine::all_matching_sql(1),
            vec!["pij-junk", "dispatch"],
        ),
        (
            pij_store::spine::all_matching_sql(2),
            vec!["pij-junk", "anomaly.ack", "anomaly.clear"],
        ),
        (
            pij_store::spine::all_matching_sql(3),
            vec!["pij-junk", "report.now", "report.state", "state-verified"],
        ),
        (pij_store::spine::TAIL_PAGE_SQL.into(), vec!["0", "5001"]),
        (
            "SELECT COALESCE(MAX(seq), 0) FROM spine_events".into(),
            vec![],
        ),
    ];
    for (sql, binds) in &statements {
        let plan = query_plan(&world.pool, sql, binds).await;
        assert!(
            plan.iter()
                .all(|step| !step.starts_with("SCAN spine_events")),
            "{sql}: {plan:?}"
        );
        assert!(
            plan.first()
                .is_some_and(|step| step.starts_with("SEARCH spine_events")),
            "{sql}: {plan:?}"
        );
    }

    world.spine.refuse_tail.store(true, Ordering::SeqCst);
    let junk = SeatId::from("pij-junk");
    let bound = std::time::Duration::from_secs(2);
    let timed = std::time::Instant::now();
    let rows = world
        .service(FakeLiveness::new())
        .list_at(&BTreeMap::new(), None, NOW)
        .await
        .expect("anomalies");
    let anomalies = timed.elapsed();
    assert_eq!(
        rows["anomalies"].as_array().expect("rows").len(),
        72,
        "24 stale cards, 24 unverified dones, 24 dead seats"
    );
    let timed = std::time::Instant::now();
    let head = world.bus.newest().await.expect("node show head");
    let page = world
        .bus
        .tail_page(Seq(0), 5_001)
        .await
        .expect("events page");
    let peer = world
        .bus
        .seat_page(&junk, Seq(0), 5_001)
        .await
        .expect("peer page");
    let events = timed.elapsed();
    assert_eq!((page.len(), peer.len()), (5_001, 5_001));
    assert!(head.0 >= 1_000_000);
    eprintln!(
        "#119 bound over {rows_seeded} rows: anomalies {anomalies:?}, head + two 5001-row pages {events:?}"
    );
    assert!(anomalies < bound, "pij anomalies took {anomalies:?}");
    assert!(
        events < bound,
        "node show and spine events reads took {events:?}"
    );
    assert_eq!(world.spine.tail_calls.load(Ordering::SeqCst), 0);
}

/// Issue #119, AC4: the three handlers' source never calls `Spine::tail`, the
/// unbounded read. A behavioural guard covers the paths a test reaches; this
/// one covers the paths it does not.
#[test]
fn the_three_handlers_never_call_the_unbounded_tail() {
    fn body<'a>(source: &'a str, start: &str) -> &'a str {
        let from = source
            .find(start)
            .unwrap_or_else(|| panic!("{start} not found"));
        let rest = &source[from..];
        let end = rest[1..]
            .find("\nasync fn ")
            .into_iter()
            .chain(rest[1..].find("\nfn "))
            .chain(rest[1..].find("\n#[derive"))
            .chain(rest[1..].find("\nstruct "))
            .min()
            .map_or(rest.len(), |end| end + 1);
        &rest[..end]
    }
    let governance = include_str!("governance.rs");
    let anomalies = include_str!("anomalies.rs");
    let production = &anomalies[..anomalies.find("#[cfg(test)]").expect("test module")];
    for (name, source) in [
        ("node_show", body(governance, "async fn node_show(")),
        ("spine_command", body(governance, "async fn spine_command(")),
        ("anomalies.rs", production),
    ] {
        assert!(source.len() > 200, "{name}: the guard read too little");
        assert!(
            !source.contains(".tail("),
            "{name} calls the unbounded Spine::tail"
        );
    }
}
