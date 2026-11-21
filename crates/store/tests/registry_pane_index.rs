//! F258 B1: the registry's pane lookup rides `seats_by_pane` (migration 0048).
//! `whoami --pane` runs it once per status-line render, per session; before the
//! index each call scanned every seat row ever written.

mod support;

use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::ports::{Registry, SeatFilter};
use pij_store::SqliteRegistry;
use pij_testkit::FreshStore;

fn seat(id: &str, harness: Harness, pane: Option<&str>) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(id, harness, "/abs/tree");
    seat.pane = pane.map(str::to_string);
    seat
}

fn ids(seats: &[SeatDescriptor]) -> Vec<&str> {
    seats.iter().map(|seat| seat.id.as_str()).collect()
}

fn on_pane(pane: &str) -> SeatFilter {
    SeatFilter {
        pane: Some(pane.to_string()),
        ..SeatFilter::default()
    }
}

/// A pane's rows, live and retired, in id order; every other filter still
/// applies on the pane branch.
#[tokio::test]
async fn a_pane_lists_exactly_its_rows_with_every_other_filter_applied() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for (id, harness, pane) in [
        ("pij-c", Harness::Copilot, Some("%5")),
        ("pij-a", Harness::Claude, Some("%5")),
        ("pij-b", Harness::Copilot, Some("%5")),
        ("pij-other", Harness::Copilot, Some("%50")),
        ("pij-paneless", Harness::Copilot, None),
    ] {
        registry.put(seat(id, harness, pane)).await.unwrap();
    }
    registry
        .tombstone(&SeatId::from("pij-b"), "test: retired")
        .await
        .unwrap();

    let all = registry.list(on_pane("%5")).await.unwrap();
    assert_eq!(ids(&all), ["pij-a", "pij-b", "pij-c"]);
    let live = registry
        .list(SeatFilter {
            live_only: true,
            ..on_pane("%5")
        })
        .await
        .unwrap();
    assert_eq!(ids(&live), ["pij-a", "pij-c"]);
    let copilot = registry
        .list(SeatFilter {
            harness: Some(Harness::Copilot),
            ..on_pane("%5")
        })
        .await
        .unwrap();
    assert_eq!(ids(&copilot), ["pij-b", "pij-c"]);
    assert!(registry.list(on_pane("%9")).await.unwrap().is_empty());
    assert!(
        registry.list(on_pane("")).await.unwrap().is_empty(),
        "an empty pane names no pane, it is not 'any'"
    );
    let every = registry.list(SeatFilter::default()).await.unwrap();
    assert_eq!(
        ids(&every),
        ["pij-a", "pij-b", "pij-c", "pij-other", "pij-paneless"],
        "no pane filter still lists every row"
    );
}

/// Cost (AGENTS.md § Resource cost): the pane branch is one seek on
/// `seats_by_pane`, never a scan of `seats`.
#[tokio::test]
async fn the_pane_lookup_searches_its_index_not_the_table() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for i in 0..300 {
        let pane = format!("%{}", i % 40);
        registry
            .put(seat(&format!("pij-{i}"), Harness::Copilot, Some(&pane)))
            .await
            .unwrap();
    }
    sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .expect("analyze");
    let explain = format!(
        "EXPLAIN QUERY PLAN {}",
        pij_store::registry::LIST_BY_PANE_SQL
    );
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&explain)
        .bind(Option::<&str>::None)
        .bind(Option::<&str>::None)
        .bind(Option::<&str>::None)
        .bind("%7")
        .bind(1_i64)
        .fetch_all(&pool)
        .await
        .expect("explain");
    let details: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
    assert!(
        details
            .iter()
            .any(|line| line.starts_with("SEARCH seats") && line.contains("seats_by_pane")),
        "the pane lookup searches seats_by_pane: {details:?}"
    );
    assert!(
        !details.iter().any(|line| line.starts_with("SCAN seats")),
        "no full scan of seats per whoami: {details:?}"
    );
}
