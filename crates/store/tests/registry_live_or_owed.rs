//! F259a: the delivery drain's roster is every live seat and every retired
//! seat still owed a delivery row, read from `seats_live` and the queue's
//! claim index. The drain asks it every 5 s; before, it listed every seat row
//! ever written (3,297 on the live store, 133 live).

mod support;

use pij_core::delivery::{control_kind, delivery_kind};
use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::ports::{Registry, SeatFilter};
use pij_store::{SqliteRegistry, StorePool};
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

/// Equivalence with the full list the drain used to read, on retired seats
/// with and without open rows: the new roster is exactly the old one less
/// the retired seats with no pending or running row of either delivery kind,
/// the only ones that gave the drain nothing to claim.
#[tokio::test]
async fn the_drain_roster_is_the_full_list_less_retired_seats_owed_nothing() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
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
    job(
        &pool,
        &control("pij-owed-control"),
        "pij-owed-control",
        "pending",
    )
    .await;
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

    let roster = registry.list_live_or_owed().await.expect("roster");
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

/// Cost (AGENTS.md § Resource cost): the roster walks `seats_live` and the
/// open rows of `jobs_claimable`, then finds each owed seat by primary key:
/// never a scan of the seats table. 300 seats written, 20 live, 3 retired
/// seats owed a row: 23 rows read back. With either index gone, `INDEXED BY`
/// refuses the query.
///
/// Mutation witness: a `list_live_or_owed` that answers from `LIST_SQL`
/// returns all 300 rows.
#[tokio::test]
async fn the_drain_roster_never_scans_the_seats_table() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
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
    }
    sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .expect("analyze");

    let explain = format!(
        "EXPLAIN QUERY PLAN {}",
        pij_store::registry::LIST_LIVE_OR_OWED_SQL
    );
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&explain)
        .bind(pij_core::delivery::DELIVERY_KIND_PREFIX)
        .bind(pij_core::delivery::CONTROL_KIND_PREFIX)
        .fetch_all(&pool)
        .await
        .expect("explain");
    let details: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
    assert!(
        details.iter().any(|line| line.contains("seats_live")),
        "the live rows come from seats_live: {details:?}"
    );
    assert!(
        details
            .iter()
            .any(|line| line.contains("jobs_claimable") && line.starts_with("SEARCH")),
        "the open rows are a seek on jobs_claimable: {details:?}"
    );
    assert!(
        details
            .iter()
            .any(|line| line.starts_with("SEARCH seats USING INDEX sqlite_autoindex_seats_1")),
        "each owed seat is found by primary key: {details:?}"
    );
    assert!(
        !details.iter().any(|line| *line == "SCAN seats"
            || line.starts_with("SCAN seats USING INDEX sqlite_autoindex")),
        "never a scan of the seats table: {details:?}"
    );

    let roster = registry.list_live_or_owed().await.expect("roster");
    assert_eq!(roster.len(), 23, "{:?}", ids(&roster));
    sqlx::query("DROP INDEX seats_live")
        .execute(&pool)
        .await
        .expect("drop index");
    let refused = registry
        .list_live_or_owed()
        .await
        .expect_err("INDEXED BY refuses");
    assert!(refused.to_string().contains("seats_live"), "{refused}");
}
