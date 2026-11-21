//! F258: the registry's live-only list walks `seats_live` (migration 0049), a
//! partial index of the live rows only. The pane observer runs it every 600 ms
//! and the death sweep every 5 s; before the index each read scanned every
//! seat row ever written.

mod support;

use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::ports::{Registry, SeatFilter};
use pij_store::SqliteRegistry;
use pij_testkit::FreshStore;

fn seat(id: &str, harness: Harness, folder: &str) -> SeatDescriptor {
    SeatDescriptor::new(id, harness, folder)
}

fn ids(seats: &[SeatDescriptor]) -> Vec<&str> {
    seats.iter().map(|seat| seat.id.as_str()).collect()
}

fn live() -> SeatFilter {
    SeatFilter {
        live_only: true,
        ..SeatFilter::default()
    }
}

/// The live branch lists exactly the live rows in id order, with every other
/// filter applied; a retired row leaves it and a re-registered one returns.
#[tokio::test]
async fn the_live_list_is_exactly_the_live_rows_with_every_filter_applied() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for (id, harness, folder) in [
        ("pij-c", Harness::Copilot, "/abs/one"),
        ("pij-a", Harness::Claude, "/abs/one"),
        ("pij-b", Harness::Copilot, "/abs/two"),
        ("pij-d", Harness::Copilot, "/abs/one"),
    ] {
        registry.put(seat(id, harness, folder)).await.unwrap();
    }
    registry
        .tombstone(&SeatId::from("pij-d"), "test: retired")
        .await
        .unwrap();

    assert_eq!(
        ids(&registry.list(live()).await.unwrap()),
        ["pij-a", "pij-b", "pij-c"]
    );
    let copilot = SeatFilter {
        harness: Some(Harness::Copilot),
        ..live()
    };
    assert_eq!(
        ids(&registry.list(copilot).await.unwrap()),
        ["pij-b", "pij-c"]
    );
    let one = SeatFilter {
        folder: Some("/abs/one".to_string()),
        ..live()
    };
    assert_eq!(ids(&registry.list(one).await.unwrap()), ["pij-a", "pij-c"]);
    assert_eq!(
        ids(&registry.list(SeatFilter::default()).await.unwrap()),
        ["pij-a", "pij-b", "pij-c", "pij-d"],
        "without live_only every row is listed"
    );

    registry
        .put(seat("pij-d", Harness::Copilot, "/abs/one"))
        .await
        .unwrap();
    registry
        .tombstone(&SeatId::from("pij-a"), "test: retired")
        .await
        .unwrap();
    assert_eq!(
        ids(&registry.list(live()).await.unwrap()),
        ["pij-b", "pij-c", "pij-d"],
        "a retired row leaves the live list and a re-registered one returns"
    );
}

/// Cost (AGENTS.md § Resource cost): the live list walks `seats_live`, which
/// holds only the live rows (300 written, 20 live), in id order: no table scan
/// and no sort. And `list` takes that query: with the index gone, `INDEXED BY`
/// refuses it.
///
/// Mutation witnesses: the index without its `WHERE` holds all 300 rows; a
/// `list` that sends live-only reads to `LIST_SQL` still answers with the index
/// dropped.
#[tokio::test]
async fn the_live_list_walks_a_partial_index_of_live_rows_only() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for i in 0..300 {
        let id = format!("pij-{i:03}");
        registry
            .put(seat(&id, Harness::Copilot, "/abs/tree"))
            .await
            .unwrap();
        if i % 15 != 0 {
            registry
                .tombstone(&SeatId::from(id.as_str()), "test: retired")
                .await
                .unwrap();
        }
    }
    sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .expect("analyze");
    let (stat,): (String,) =
        sqlx::query_as("SELECT stat FROM sqlite_stat1 WHERE idx = 'seats_live'")
            .fetch_one(&pool)
            .await
            .expect("seats_live statistics");
    assert_eq!(
        stat.split_whitespace().next(),
        Some("20"),
        "seats_live holds the 20 live rows only: {stat}"
    );

    let explain = format!("EXPLAIN QUERY PLAN {}", pij_store::registry::LIST_LIVE_SQL);
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&explain)
        .bind(Option::<&str>::None)
        .bind(Option::<&str>::None)
        .bind(Option::<&str>::None)
        .bind(Option::<&str>::None)
        .bind(1_i64)
        .fetch_all(&pool)
        .await
        .expect("explain");
    let details: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
    assert!(
        details.iter().any(|line| line.contains("seats_live")),
        "the live list walks seats_live: {details:?}"
    );
    assert!(
        !details
            .iter()
            .any(|line| *line == "SCAN seats" || line.contains("TEMP B-TREE")),
        "no table scan and no sort: {details:?}"
    );

    assert_eq!(registry.list(live()).await.unwrap().len(), 20);
    sqlx::query("DROP INDEX seats_live")
        .execute(&pool)
        .await
        .expect("drop index");
    let refused = registry.list(live()).await.expect_err("INDEXED BY refuses");
    assert!(refused.to_string().contains("seats_live"), "{refused}");
}
