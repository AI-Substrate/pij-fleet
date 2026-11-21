//! F258: a native claim wait sleeps until its kinds' next timed change instead
//! of polling every 500 ms. `next_claimable_in` names that instant: a pending
//! row's `not_before` or a running claim's lapse, whichever comes first after
//! the second the caller's empty claim read the queue. Against SQLite, because
//! the instant is the queue's own clock and SQL.

use std::time::{Duration, SystemTime};

use pij_core::model::{Job, JobId};
use pij_core::ports::Queue;
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use sqlx::SqlitePool;

const KIND: &str = "delivery:pij-reader";
const OTHER: &str = "delivery:pij-other";
const LEASE_SECS: u64 = 300;

fn job(kind: &str, serial: &str, dedupe: &str) -> Job {
    Job {
        kind: kind.to_string(),
        serial_key: serial.to_string(),
        payload: "{}".to_string(),
        dedupe_key: dedupe.to_string(),
        dedupe_origin: None,
        attempt: 0,
    }
}

async fn open(fresh: &FreshStore) -> (SqlitePool, SqliteQueue) {
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), LEASE_SECS, 1_024).expect("valid queue policy");
    (pool, queue)
}

/// The next instant after an empty claim that read the queue just now.
async fn next(queue: &SqliteQueue, kind: &str) -> Option<Duration> {
    next_since(queue, kind, SystemTime::now()).await
}

async fn next_since(queue: &SqliteQueue, kind: &str, since: SystemTime) -> Option<Duration> {
    queue
        .next_claimable_in(&[kind.to_string()], since)
        .await
        .expect("next claimable")
}

async fn set(pool: &SqlitePool, sql: &str, id: JobId) {
    sqlx::query(sql)
        .bind(id.0 as i64)
        .execute(pool)
        .await
        .expect("arrange the row");
}

/// Whole seconds, so the queue's one-second clock cannot flake the check.
fn within(due: Option<Duration>, low: u64, high: u64) -> bool {
    due.is_some_and(|due| due > Duration::from_secs(low) && due <= Duration::from_secs(high))
}

#[tokio::test]
async fn nothing_live_has_no_next_instant() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    assert_eq!(next(&queue, KIND).await, None);
    let done = queue.enqueue(job(KIND, "pij-reader", "m-1")).await.unwrap();
    set(&pool, "UPDATE jobs SET state = 'done' WHERE id = ?1", done).await;
    assert_eq!(
        next(&queue, KIND).await,
        None,
        "a terminal row never falls due"
    );
}

/// A pending row due now is the claim's to take, not a timer's: no instant.
#[tokio::test]
async fn a_row_already_due_has_no_next_instant() {
    let fresh = FreshStore::new();
    let (_pool, queue) = open(&fresh).await;
    queue.enqueue(job(KIND, "pij-reader", "m-1")).await.unwrap();
    assert_eq!(next(&queue, KIND).await, None);
}

/// Review R310-F001: a row that fell due after the empty claim read the queue
/// (the second rolled between that claim and this read) is due NOW, not
/// dropped to the backstop. Read with `since` two seconds back, as a pass that
/// started before the crossing.
#[tokio::test]
async fn a_row_that_fell_due_after_the_claim_read_the_queue_is_due_now() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let id = queue.enqueue(job(KIND, "pij-reader", "m-1")).await.unwrap();
    set(
        &pool,
        "UPDATE jobs SET not_before = unixepoch() WHERE id = ?1",
        id,
    )
    .await;
    let before_it_fell_due = SystemTime::now() - Duration::from_secs(2);
    assert_eq!(
        next_since(&queue, KIND, before_it_fell_due).await,
        Some(Duration::ZERO)
    );
}

#[tokio::test]
async fn a_delayed_row_is_next_at_its_not_before() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let id = queue.enqueue(job(KIND, "pij-reader", "m-1")).await.unwrap();
    set(
        &pool,
        "UPDATE jobs SET not_before = unixepoch() + 30 WHERE id = ?1",
        id,
    )
    .await;
    let due = next(&queue, KIND).await;
    assert!(within(due, 28, 30), "{due:?}");
}

#[tokio::test]
async fn a_running_claim_is_next_when_its_lease_lapses() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let id = queue.enqueue(job(KIND, "pij-reader", "m-1")).await.unwrap();
    queue
        .claim(&[KIND.to_string()], "pij-reader")
        .await
        .unwrap()
        .expect("claimed");
    let due = next(&queue, KIND).await;
    assert!(within(due, LEASE_SECS - 2, LEASE_SECS), "{due:?}");
    set(
        &pool,
        "UPDATE jobs SET claimed_at = unixepoch() - 290 WHERE id = ?1",
        id,
    )
    .await;
    let due = next(&queue, KIND).await;
    assert!(within(due, 8, 10), "the lapse follows claimed_at: {due:?}");
}

/// The earliest future change wins; a due row held behind a running claim on
/// its serial key waits for that claim's lapse, never re-polls at once.
#[tokio::test]
async fn the_earliest_future_instant_wins_over_a_due_row_held_behind_a_claim() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let running = queue.enqueue(job(KIND, "pij-reader", "m-1")).await.unwrap();
    queue
        .claim(&[KIND.to_string()], "pij-reader")
        .await
        .unwrap()
        .expect("claimed");
    set(
        &pool,
        "UPDATE jobs SET claimed_at = unixepoch() - 280 WHERE id = ?1",
        running,
    )
    .await;
    queue.enqueue(job(KIND, "pij-reader", "m-2")).await.unwrap();
    let later = queue.enqueue(job(KIND, "pij-reader", "m-3")).await.unwrap();
    set(
        &pool,
        "UPDATE jobs SET not_before = unixepoch() + 60 WHERE id = ?1",
        later,
    )
    .await;
    let due = next(&queue, KIND).await;
    assert!(
        within(due, 18, 20),
        "the claim's lapse, not m-2 or m-3: {due:?}"
    );
}

#[tokio::test]
async fn another_kinds_rows_are_not_this_waits_instants() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let id = queue.enqueue(job(OTHER, "pij-other", "m-1")).await.unwrap();
    set(
        &pool,
        "UPDATE jobs SET not_before = unixepoch() + 30 WHERE id = ?1",
        id,
    )
    .await;
    assert_eq!(next(&queue, KIND).await, None);
    assert!(within(next(&queue, OTHER).await, 28, 30));
}

/// Cost (AGENTS.md § Resource cost): the read runs on each empty pass of a
/// waiting claim, so it must ride `jobs_claimable`, never scan the queue.
#[tokio::test]
async fn the_read_uses_an_index_at_a_realistic_queue_size() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    for i in 0..5_000 {
        let kind = if i % 2 == 0 { OTHER } else { "sidecar:bg" };
        let id = queue
            .enqueue(job(kind, &format!("key-{}", i % 50), &format!("d-{i}")))
            .await
            .expect("enqueue");
        if i % 10 != 0 {
            set(&pool, "UPDATE jobs SET state = 'done' WHERE id = ?1", id).await;
        }
    }
    sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .expect("analyze");
    let explain = format!(
        "EXPLAIN QUERY PLAN {}",
        pij_store::queue::NEXT_CLAIMABLE_SQL
    );
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&explain)
        .bind(serde_json::json!([KIND]).to_string())
        .bind(LEASE_SECS as i64)
        .bind(0_i64)
        .fetch_all(&pool)
        .await
        .expect("explain");
    let details: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
    assert!(
        details
            .iter()
            .any(|line| line.starts_with("SEARCH jobs") && line.contains("jobs_claimable")),
        "the read searches jobs_claimable: {details:?}"
    );
    assert!(
        !details.iter().any(|line| line.starts_with("SCAN jobs")),
        "no full scan of jobs on an empty pass: {details:?}"
    );
}
