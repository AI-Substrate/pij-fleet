//! Plan 185 recorded-workload replay: what one cursor attach costs on a large spine.
//!
//! Ignored by default (it builds a multi-million-row store). Run one size at a time
//! under `/usr/bin/time -l` so the peak RSS belongs to the attach:
//!
//! ```text
//! PIJ_ATTACH_ROWS=2978027 /usr/bin/time -l cargo test --release -p pij-daemon \
//!     --test spine_attach_cost -- --ignored --nocapture
//! ```
//!
//! The rows mirror the live spine on 2026-10-29: `bg.lost` payloads of ~355 bytes.
//!
//! Issue #119's bound runs by default: a million rows in memory, no disk.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::events::EventFilter;
use pij_core::model::{Event, Harness, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine, SpineWindow};
use pij_daemon::events::EventBus;
use pij_daemon::http::anomalies::AnomalyService;
use pij_store::{SqliteOrchestration, SqliteRegistry, SqliteSpine};
use pij_testkit::fakes::FakeLiveness;
use serde_json::json;
use sqlx::Row;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "builds a multi-million-row store; run by hand for plan 185 receipts"]
async fn one_cursor_attach_on_a_large_spine() {
    let rows: u64 = std::env::var("PIJ_ATTACH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500_000);
    let dir = std::env::temp_dir().join(format!("pij-185-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pij.sqlite");
    let pool = pij_store::open(path.to_str().unwrap()).await.unwrap();
    let payload = format!(r#"{{"job_id":"bg-bench","pad":"{}"}}"#, "x".repeat(320));
    let filled = Instant::now();
    sqlx::query(
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < ?1) \
         INSERT INTO spine_events (v, at, kind, seat, payload) \
         SELECT 1, x, 'bg.lost', 'pij-bench', ?2 FROM n",
    )
    .bind(i64::try_from(rows).unwrap())
    .bind(&payload)
    .execute(&pool)
    .await
    .unwrap();
    eprintln!("filled {rows} rows in {:?}", filled.elapsed());

    let bus = EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 64).unwrap();
    // A consumer 10 events behind: the replay it needs is 10 rows.
    for _ in 0..3 {
        let started = Instant::now();
        let subscription = bus
            .subscribe(Some(Seq(rows - 10)), EventFilter::all())
            .await
            .unwrap();
        eprintln!(
            "attach rows={rows} cursor_lag=10 elapsed={:?}",
            started.elapsed()
        );
        drop(subscription);
    }
    pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Refuses the unbounded read, so a bounded path cannot fall back to it.
struct NoTail {
    inner: SqliteSpine,
    tail_calls: AtomicUsize,
}

#[async_trait]
impl Spine for NoTail {
    async fn append(&self, event: Event) -> Result<Seq> {
        self.inner.append(event).await
    }
    async fn tail(&self, _seat: Option<&SeatId>, _since: Seq) -> Result<Vec<Event>> {
        self.tail_calls.fetch_add(1, Ordering::SeqCst);
        Err(PijError::Adapter {
            adapter: "no-tail".into(),
            message: "unbounded Spine::tail on a bounded path (#119)".into(),
        })
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
        .map(|row| row.get::<String, _>("detail"))
        .collect()
}

/// Issue #119, AC4: over a million spine rows, nearly all one seat's junk,
/// every statement `pij anomalies`, `pij node show` and `pij spine events` now
/// issue seeks an index and never scans `spine_events`, and each verb's reads
/// finish inside a stated bound. The spine refuses `tail` throughout.
#[tokio::test]
async fn anomalies_node_show_and_spine_events_stay_bounded_over_a_million_rows() {
    let pool = pij_store::open("").await.expect("in-memory store");
    let spine = Arc::new(NoTail {
        inner: SqliteSpine::new(pool.clone()),
        tail_calls: AtomicUsize::new(0),
    });
    let raw: Arc<dyn Spine> = spine.clone();
    let bus = Arc::new(EventBus::new(raw, 1_024).expect("bus"));
    let registry = Arc::new(SqliteRegistry::new(pool.clone(), bus.clone()));
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms");
    let stale = now - 45 * 60_000;
    for n in 0..24_u32 {
        let id = format!("pij-seat-{n}");
        let mut seat = SeatDescriptor::new(id.as_str(), Harness::Omp, "/work");
        seat.proc = Some(ProcIdentity {
            pid: 100 + n,
            proc_start: 1,
        });
        registry.put(seat).await.expect("seat");
        for (kind, payload) in [
            ("report.now", json!({"did": "work", "next": "more"})),
            ("report.state", json!({"state": "done"})),
        ] {
            bus.publish(Event {
                seq: None,
                v: 1,
                at: stale,
                kind: kind.into(),
                seat: Some(SeatId::from(id.as_str())),
                payload: payload.to_string(),
            })
            .await
            .expect("fact");
        }
    }
    registry
        .put(SeatDescriptor::new("pij-junk", Harness::Omp, "/work"))
        .await
        .expect("junk seat");
    sqlx::raw_sql(
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1000000) \
         INSERT INTO spine_events (v, at, kind, seat, payload) \
         SELECT 1, x, 'bg.lost', 'pij-junk', '{}' FROM n",
    )
    .execute(&pool)
    .await
    .expect("a million junk rows");
    let seeded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM spine_events")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert!(seeded >= 1_000_000, "{seeded} rows");

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
        let plan = query_plan(&pool, sql, binds).await;
        assert!(
            plan.first()
                .is_some_and(|step| step.starts_with("SEARCH spine_events"))
                && plan
                    .iter()
                    .all(|step| !step.starts_with("SCAN spine_events")),
            "{sql}: {plan:?}"
        );
    }

    let bound = Duration::from_secs(2);
    let service = AnomalyService::new(
        SqliteOrchestration::new(pool.clone()),
        registry,
        bus.clone(),
        Arc::new(FakeLiveness::new()),
    );
    let timed = Instant::now();
    let rows = service
        .list(&BTreeMap::new(), None)
        .await
        .expect("anomalies");
    let anomalies = timed.elapsed();
    assert_eq!(
        rows["anomalies"].as_array().expect("rows").len(),
        72,
        "24 stale cards, 24 unverified dones, 24 dead seats: {rows}"
    );
    let timed = Instant::now();
    let head = bus.newest().await.expect("node show head");
    let page = bus.tail_page(Seq(0), 5_001).await.expect("events page");
    let peer = bus
        .seat_page(&SeatId::from("pij-junk"), Seq(0), 5_001)
        .await
        .expect("peer page");
    let events = timed.elapsed();
    eprintln!(
        "#119 bound over {seeded} rows: anomalies {anomalies:?}, head + two 5001-row pages {events:?}"
    );
    assert_eq!((page.len(), peer.len()), (5_001, 5_001));
    assert!(head.0 >= 1_000_000);
    assert!(anomalies < bound, "pij anomalies took {anomalies:?}");
    assert!(
        events < bound,
        "node show and spine events reads took {events:?}"
    );
    assert_eq!(spine.tail_calls.load(Ordering::SeqCst), 0);
}
