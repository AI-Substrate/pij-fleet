//! F259a: the delivery drain's roster is every live seat and every retired
//! seat its queue still owes a delivery row. The QUEUE names the owed seats
//! (`Queue::owed_recipients`, a range seek per delivery prefix on
//! `jobs_claimable`), and the registry lists the live rows plus those by
//! primary key (`Registry::list_live_with`). The drain asks it every 5 s;
//! before, it listed every seat row ever written (3,297 on the live store,
//! 133 live).

mod support;

use pij_core::delivery::{CONTROL_KIND_PREFIX, DELIVERY_KIND_PREFIX, control_kind, delivery_kind};
use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::ports::{Queue, Registry, SeatFilter};
use pij_store::queue::{OWED_RECIPIENTS_SQL, prefix_bound};
use pij_store::{SqliteQueue, SqliteRegistry, StorePool};
use pij_testkit::FreshStore;

fn ids(seats: &[SeatDescriptor]) -> Vec<&str> {
    seats.iter().map(|seat| seat.id.as_str()).collect()
}

async fn job(pool: &StorePool, kind: &str, serial: &str, state: &str) {
    sqlx::query(
        "INSERT INTO jobs (kind, serial_key, payload, dedupe_key, state, enqueued_at) \
         VALUES (?1, ?2, '{}', ?1 || ':' || ?3 || ':' || random(), ?3, 1)",
    )
    .bind(kind)
    .bind(serial)
    .bind(state)
    .execute(pool)
    .await
    .expect("insert job");
}

async fn retire(registry: &SqliteRegistry, id: &str) {
    registry
        .tombstone(&SeatId::from(id), "test: retired")
        .await
        .expect("tombstone");
}

async fn plan(pool: &StorePool, sql: &str, binds: &[&str]) -> Vec<String> {
    let explain = format!("EXPLAIN QUERY PLAN {sql}");
    let mut query = sqlx::query_as::<_, (i64, i64, i64, String)>(&explain);
    for bind in binds {
        query = query.bind(*bind);
    }
    query
        .fetch_all(pool)
        .await
        .expect("explain")
        .into_iter()
        .map(|row| row.3)
        .collect()
}

/// Equivalence with the full list the drain used to read, on retired seats
/// with and without open rows, both stores on one pool: the roster is exactly
/// the old one less the retired seats with no pending or running row of
/// either delivery kind, the only ones that gave the drain nothing to claim.
#[tokio::test]
async fn the_drain_roster_is_the_full_list_less_retired_seats_owed_nothing() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("queue");
    for (id, harness) in [
        ("pij-live-claude", Harness::Claude),
        ("pij-live-copilot", Harness::Copilot),
        ("pij-live-omp", Harness::Omp),
        ("pij-owed-body", Harness::Claude),
        ("pij-owed-control", Harness::Copilot),
        ("pij-owed-native-body", Harness::Copilot),
        ("pij-owed-running", Harness::Codex),
        ("pij-owed-pi", Harness::Pi),
        ("pij-paid-done", Harness::Claude),
        ("pij-paid-failed", Harness::Copilot),
        ("pij-quiet", Harness::Claude),
        ("pij-other-kind", Harness::Claude),
    ] {
        registry
            .put(SeatDescriptor::new(id, harness, "/abs/tree"))
            .await
            .expect("put");
    }
    for id in [
        "pij-owed-body",
        "pij-owed-control",
        "pij-owed-native-body",
        "pij-owed-running",
        "pij-owed-pi",
        "pij-paid-done",
        "pij-paid-failed",
        "pij-quiet",
        "pij-other-kind",
    ] {
        retire(&registry, id).await;
    }
    let kind = |id: &str| delivery_kind(&SeatId::from(id));
    let control = |id: &str| control_kind(&SeatId::from(id));
    job(
        &pool,
        &kind("pij-live-claude"),
        "pij-live-claude",
        "pending",
    )
    .await;
    job(&pool, &kind("pij-owed-body"), "pij-owed-body", "pending").await;
    // Owed by a control row only: the control arm alone names it.
    job(
        &pool,
        &control("pij-owed-control"),
        "pij-owed-control",
        "pending",
    )
    .await;
    // Two open rows for one seat, of both kinds: named once.
    job(&pool, &control("pij-owed-body"), "pij-owed-body", "running").await;
    job(
        &pool,
        &kind("pij-owed-native-body"),
        "pij-owed-native-body",
        "pending",
    )
    .await;
    job(
        &pool,
        &kind("pij-owed-running"),
        "pij-owed-running",
        "running",
    )
    .await;
    job(&pool, &kind("pij-owed-pi"), "pij-owed-pi", "pending").await;
    job(&pool, &kind("pij-paid-done"), "pij-paid-done", "done").await;
    job(
        &pool,
        &control("pij-paid-failed"),
        "pij-paid-failed",
        "failed",
    )
    .await;
    // Another queue's row keyed by the seat never makes it owed.
    job(&pool, "telegram", "pij-other-kind", "pending").await;
    job(
        &pool,
        "deliveryish:pij-other-kind",
        "pij-other-kind",
        "pending",
    )
    .await;

    let owed = queue
        .owed_recipients()
        .await
        .expect("owed")
        .expect("the store queue answers");
    assert_eq!(
        owed.iter().map(SeatId::as_str).collect::<Vec<_>>(),
        [
            "pij-live-claude",
            "pij-owed-body",
            "pij-owed-control",
            "pij-owed-native-body",
            "pij-owed-pi",
            "pij-owed-running",
        ]
    );
    let roster = registry.list_live_with(&owed).await.expect("roster");
    assert_eq!(
        ids(&roster),
        [
            "pij-live-claude",
            "pij-live-copilot",
            "pij-live-omp",
            "pij-owed-body",
            "pij-owed-control",
            "pij-owed-native-body",
            "pij-owed-pi",
            "pij-owed-running",
        ]
    );
    let full = registry
        .list(SeatFilter::default())
        .await
        .expect("full list");
    let owed_nothing = [
        "pij-other-kind",
        "pij-paid-done",
        "pij-paid-failed",
        "pij-quiet",
    ];
    let expected: Vec<&SeatDescriptor> = full
        .iter()
        .filter(|seat| !owed_nothing.contains(&seat.id.as_str()))
        .collect();
    assert_eq!(roster.iter().collect::<Vec<_>>(), expected);
}

/// Cost (AGENTS.md § Resource cost): the queue names the owed seats with a
/// range seek per delivery prefix on `jobs_claimable`, reading only open
/// delivery rows; the registry walks `seats_live` and finds each named seat
/// by primary key. Neither scans its table. 300 seats written, 20 live, 3
/// retired seats owed a row, and 200 open rows of another queue that the
/// queue never reads: 23 rows back. With an index gone, `INDEXED BY` refuses.
///
/// Mutation witness: a registry that answers from `LIST_SQL` returns all 300.
#[tokio::test]
async fn the_drain_roster_never_scans_a_table() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("queue");
    for i in 0..300 {
        let id = format!("pij-{i:03}");
        registry
            .put(SeatDescriptor::new(
                id.as_str(),
                Harness::Claude,
                "/abs/tree",
            ))
            .await
            .expect("put");
        if i % 15 != 0 {
            retire(&registry, &id).await;
        }
        if (1..=3).contains(&i) {
            job(
                &pool,
                &delivery_kind(&SeatId::from(id.as_str())),
                &id,
                "pending",
            )
            .await;
        }
        if i < 200 {
            job(&pool, "telegram", &id, "pending").await;
        }
    }
    sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .expect("analyze");

    let delivery_bound = prefix_bound(DELIVERY_KIND_PREFIX);
    let control_bound = prefix_bound(CONTROL_KIND_PREFIX);
    assert_eq!(delivery_bound, "delivery;");
    assert_eq!(control_bound, "delivery-control;");
    let owed_plan = plan(
        &pool,
        OWED_RECIPIENTS_SQL,
        &[
            DELIVERY_KIND_PREFIX,
            &delivery_bound,
            CONTROL_KIND_PREFIX,
            &control_bound,
        ],
    )
    .await;
    assert_eq!(
        owed_plan
            .iter()
            .filter(|line| line.starts_with(
                "SEARCH jobs USING COVERING INDEX jobs_claimable (state=? AND kind>? AND kind<?)"
            ))
            .count(),
        2,
        "one range seek per delivery prefix: {owed_plan:?}"
    );
    assert!(
        !owed_plan.iter().any(|line| line.starts_with("SCAN jobs")),
        "never a scan of the jobs table: {owed_plan:?}"
    );

    let roster_plan = plan(
        &pool,
        pij_store::registry::LIST_LIVE_WITH_SQL,
        &["[\"pij-001\"]"],
    )
    .await;
    assert!(
        roster_plan.iter().any(|line| line.contains("seats_live")),
        "the live rows come from seats_live: {roster_plan:?}"
    );
    assert!(
        roster_plan
            .iter()
            .any(|line| line.starts_with("SEARCH seats USING INDEX sqlite_autoindex_seats_1")),
        "each named seat is found by primary key: {roster_plan:?}"
    );
    assert!(
        !roster_plan.iter().any(|line| *line == "SCAN seats"
            || line.starts_with("SCAN seats USING INDEX sqlite_autoindex")),
        "never a scan of the seats table: {roster_plan:?}"
    );

    let owed = queue
        .owed_recipients()
        .await
        .expect("owed")
        .expect("answers");
    assert_eq!(owed.len(), 3);
    let roster = registry.list_live_with(&owed).await.expect("roster");
    assert_eq!(roster.len(), 23, "{:?}", ids(&roster));
    sqlx::query("DROP INDEX seats_live")
        .execute(&pool)
        .await
        .expect("drop index");
    let refused = registry
        .list_live_with(&owed)
        .await
        .expect_err("INDEXED BY refuses");
    assert!(refused.to_string().contains("seats_live"), "{refused}");
    sqlx::query("DROP INDEX jobs_claimable")
        .execute(&pool)
        .await
        .expect("drop index");
    let refused = queue
        .owed_recipients()
        .await
        .expect_err("INDEXED BY refuses");
    assert!(refused.to_string().contains("jobs_claimable"), "{refused}");
}
