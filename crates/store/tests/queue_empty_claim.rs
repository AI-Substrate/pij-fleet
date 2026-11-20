//! F234: a claim whose kinds have no live row takes no write transaction. The
//! native inbox's idle pass claims every 500 ms for up to 55 s; on an empty
//! queue each pass used to run BEGIN IMMEDIATE, two expiry UPDATEs, the
//! candidate SELECT and COMMIT. Against SQLite, because the write is the
//! thing under test.
//!
//! The probe: a second connection holds the database's write lock. A claim
//! that skips the write answers at once; one that begins a write waits out the
//! busy timeout and fails. So "no write" is observed, not inferred.

use std::time::Duration;

use pij_core::model::{Job, JobId};
use pij_core::ports::{ExtensionClaim, ExtensionLease, Queue};
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use pij_testkit::time::scaled;
use sqlx::pool::PoolConnection;
use sqlx::{Sqlite, SqlitePool};

const KIND: &str = "delivery:pij-reader";
const OTHER: &str = "delivery:pij-other";

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
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    (pool, queue)
}

/// Another connection takes the write lock and keeps it until dropped.
async fn hold_write_lock(pool: &SqlitePool) -> PoolConnection<Sqlite> {
    let mut writer = pool.acquire().await.expect("second connection");
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *writer)
        .await
        .expect("hold the write lock");
    writer
}

/// End the held write lock: roll back, then hand the connection back.
async fn release(mut writer: PoolConnection<Sqlite>) {
    sqlx::query("ROLLBACK")
        .execute(&mut *writer)
        .await
        .expect("release the write lock");
}

/// The native inbox's claim: an extension lease, as its idle pass makes it.
async fn native_claim(
    queue: &SqliteQueue,
    spine: &pij_store::SqliteSpine,
    kinds: &[&str],
) -> pij_core::error::Result<ExtensionClaim> {
    let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_string()).collect();
    let lease = ExtensionLease {
        seconds: 60,
        renew_working: true,
    };
    tokio::time::timeout(
        scaled(Duration::from_secs(10)),
        queue.claim_extension(&kinds, "pij-reader", lease, 0, spine, true),
    )
    .await
    .expect("a claim answers within its busy timeout")
}

fn is_empty(claim: &ExtensionClaim) -> bool {
    claim.claimed.is_none() && claim.parked.is_empty() && claim.events.is_empty()
}

/// What a claim answered: the job claimed, then parked and event counts.
fn shape(claim: &ExtensionClaim) -> (Option<u64>, usize, usize) {
    (
        claim.claimed.as_ref().map(|(id, _)| id.0),
        claim.parked.len(),
        claim.events.len(),
    )
}

#[tokio::test]
async fn an_empty_claim_answers_without_taking_the_write_lock() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let writer = hold_write_lock(&pool).await;
    let claim = native_claim(&queue, &spine, &[KIND])
        .await
        .expect("an empty claim never waits for the write lock");
    assert!(is_empty(&claim), "{:?}", shape(&claim));
    let plain = tokio::time::timeout(
        scaled(Duration::from_secs(10)),
        queue.claim(&[KIND.to_string()], "pij-reader"),
    )
    .await
    .expect("answers")
    .expect("the plain claim takes the same shortcut");
    assert!(plain.is_none());
    release(writer).await;
}

#[tokio::test]
async fn a_pending_row_is_claimed_as_before() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let id = queue
        .enqueue(job(KIND, "pij-reader", "m-1"))
        .await
        .expect("enqueue");
    let claim = native_claim(&queue, &spine, &[KIND]).await.expect("claim");
    let (claimed, body) = claim.claimed.expect("the pending row is claimed");
    assert_eq!(claimed, id);
    assert_eq!(body.dedupe_key, "m-1");
}

#[tokio::test]
async fn a_lapsed_running_claim_is_still_recovered_with_nothing_pending() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    // A body delivery retries; another kind fails terminally. With both
    // claimed and aged, no row of either kind is pending.
    let body = queue
        .enqueue(job(KIND, "pij-reader", "m-1"))
        .await
        .expect("enqueue body");
    let chore = queue
        .enqueue(job("sidecar:bg", "bg", "c-1"))
        .await
        .expect("enqueue chore");
    for kind in [KIND, "sidecar:bg"] {
        queue
            .claim(&[kind.to_string()], "first")
            .await
            .expect("claim")
            .expect("a row");
    }
    sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 301")
        .execute(&pool)
        .await
        .expect("age both claims");
    let retried = queue
        .claim(&[KIND.to_string()], "replacement")
        .await
        .expect("claim after expiry")
        .expect("the lapsed body claim is recovered and claimed again");
    assert_eq!((retried.0, retried.1.attempt), (body, 1));
    let none = queue
        .claim(&["sidecar:bg".to_string()], "replacement")
        .await
        .expect("claim after expiry");
    assert!(none.is_none());
    assert_eq!(
        state(&pool, chore).await,
        "failed",
        "the lapsed chore expired"
    );
}

#[tokio::test]
async fn a_row_that_is_not_yet_claimable_still_takes_the_full_path() {
    // Each case leaves a live row of the requested kind that the claim cannot
    // take now: delayed (not due), or behind a running row on its key. The
    // full path must still run, so under a held write lock it waits and fails.
    for case in ["delayed", "serial-blocked"] {
        let fresh = FreshStore::new();
        let (pool, queue) = open(&fresh).await;
        let spine = pij_store::SqliteSpine::new(pool.clone());
        let id = queue
            .enqueue(job(KIND, "pij-reader", "m-1"))
            .await
            .expect("enqueue");
        match case {
            "delayed" => {
                sqlx::query("UPDATE jobs SET not_before = unixepoch() + 3600 WHERE id = ?1")
                    .bind(id.0 as i64)
                    .execute(&pool)
                    .await
                    .expect("delay");
            }
            _ => {
                queue
                    .enqueue(job(OTHER, "pij-reader", "o-1"))
                    .await
                    .expect("enqueue a row of another kind on the same key");
                sqlx::query(
                    "UPDATE jobs SET state = 'running', claimed_at = unixepoch() WHERE kind = ?1",
                )
                .bind(OTHER)
                .execute(&pool)
                .await
                .expect("run it");
            }
        }
        let writer = hold_write_lock(&pool).await;
        let claim = native_claim(&queue, &spine, &[KIND]).await;
        release(writer).await;
        let error = match claim {
            Err(error) => error,
            Ok(claim) => panic!("{case}: the claim skipped its write: {:?}", shape(&claim)),
        };
        assert!(
            error.to_string().contains("locked") || error.to_string().contains("busy"),
            "{case}: {error}"
        );
        let after = native_claim(&queue, &spine, &[KIND]).await.expect("claim");
        assert!(
            is_empty(&after),
            "{case}: still not claimable: {:?}",
            shape(&after)
        );
        assert_eq!(state(&pool, id).await, "pending", "{case}");
    }
}

#[tokio::test]
async fn a_live_row_of_another_kind_does_not_stop_the_shortcut() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let spine = pij_store::SqliteSpine::new(pool.clone());
    queue
        .enqueue(job(OTHER, "pij-other", "o-1"))
        .await
        .expect("enqueue another kind");
    let writer = hold_write_lock(&pool).await;
    let claim = native_claim(&queue, &spine, &[KIND])
        .await
        .expect("another kind's row never sends this claim to the write path");
    release(writer).await;
    assert!(is_empty(&claim), "{:?}", shape(&claim));
}

/// Cost (AGENTS.md § Resource cost): the preflight runs on every idle inbox
/// pass, so it must ride `jobs_claimable`, never scan the queue.
#[tokio::test]
async fn the_preflight_uses_an_index_at_a_realistic_queue_size() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    for i in 0..5_000 {
        let kind = if i % 2 == 0 { OTHER } else { "sidecar:bg" };
        let id = queue
            .enqueue(job(kind, &format!("key-{}", i % 50), &format!("d-{i}")))
            .await
            .expect("enqueue");
        if i % 10 != 0 {
            sqlx::query("UPDATE jobs SET state = 'done' WHERE id = ?1")
                .bind(id.0 as i64)
                .execute(&pool)
                .await
                .expect("finish");
        }
    }
    sqlx::query("ANALYZE")
        .execute(&pool)
        .await
        .expect("analyze");
    let explain = format!("EXPLAIN QUERY PLAN {}", pij_store::queue::LIVE_OF_KINDS_SQL);
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&explain)
        .bind(serde_json::json!([KIND]).to_string())
        .fetch_all(&pool)
        .await
        .expect("explain");
    let details: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
    assert!(
        details
            .iter()
            .any(|line| line.starts_with("SEARCH jobs") && line.contains("jobs_claimable")),
        "the preflight searches jobs_claimable: {details:?}"
    );
    assert!(
        !details.iter().any(|line| line.starts_with("SCAN jobs")),
        "no full scan of jobs on an idle pass: {details:?}"
    );
}

async fn state(pool: &SqlitePool, id: JobId) -> String {
    sqlx::query_scalar("SELECT state FROM jobs WHERE id = ?1")
        .bind(id.0 as i64)
        .fetch_one(pool)
        .await
        .expect("row")
}
