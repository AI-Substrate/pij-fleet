//! Plan 183 N1/N3: `enqueue_all` is all or nothing, collapses only onto a live
//! row with the same payload, and names a conflicting one.

use pij_core::model::Job;
use pij_core::ports::{BatchEnqueue, Queue};
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeQueue;

fn job(dedupe: &str, payload: &str) -> Job {
    Job {
        kind: "sidecar:telegram:document".to_string(),
        serial_key: "default".to_string(),
        payload: payload.to_string(),
        dedupe_key: dedupe.to_string(),
        dedupe_origin: None,
        attempt: 0,
    }
}

async fn sqlite(fresh: &FreshStore) -> (sqlx::SqlitePool, SqliteQueue) {
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    (pool, queue)
}

async fn live_rows(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE state IN ('pending', 'running')")
        .fetch_one(pool)
        .await
        .expect("count")
}

fn queued(batch: BatchEnqueue) -> Vec<pij_core::ports::Enqueued> {
    match batch {
        BatchEnqueue::Queued(rows) => rows,
        conflict => panic!("expected the batch to queue, got {conflict:?}"),
    }
}

/// Fresh rows in order; an identical resubmission collapses; a reused key with
/// other content is a conflict that queues nothing from its batch.
async fn collapse_contract(queue: &dyn Queue) {
    let first = queued(
        queue
            .enqueue_all(vec![job("m", "{\"v\":\"a\"}"), job("m#1", "{\"v\":\"b\"}")])
            .await
            .expect("first batch"),
    );
    assert!(first.iter().all(|row| row.fresh));
    assert!(first[0].id.0 < first[1].id.0, "rows keep batch order");

    let again = queued(
        queue
            .enqueue_all(vec![job("m", "{\"v\":\"a\"}"), job("m#2", "{}")])
            .await
            .expect("identical resubmission"),
    );
    assert!(!again[0].fresh, "the same payload collapses");
    assert_eq!(again[0].id, first[0].id);
    assert!(again[1].fresh);

    let conflict = queue
        .enqueue_all(vec![job("m#3", "{}"), job("m#1", "{\"v\":\"other\"}")])
        .await
        .expect("conflicting batch");
    assert_eq!(
        conflict,
        BatchEnqueue::Conflict {
            index: 1,
            id: first[1].id,
            live_payload: "{\"v\":\"b\"}".to_string(),
        }
    );
    // m#3 came first in that batch and must not have been queued.
    let fresh_again = queued(
        queue
            .enqueue_all(vec![job("m#3", "{\"probe\":1}")])
            .await
            .expect("probe"),
    );
    assert!(
        fresh_again[0].fresh,
        "nothing from the conflicting batch was queued"
    );
}

#[tokio::test]
async fn sqlite_enqueue_all_reports_fresh_and_collapsed_rows() {
    let fresh = FreshStore::new();
    let (_pool, queue) = sqlite(&fresh).await;
    collapse_contract(&queue).await;
}

#[tokio::test]
async fn fake_enqueue_all_matches_sqlite() {
    collapse_contract(&FakeQueue::new(1_024).expect("fake")).await;
}

/// N3: an internal error on the third row leaves nothing from the batch.
#[tokio::test]
async fn an_error_mid_batch_queues_nothing_from_that_batch() {
    let fresh = FreshStore::new();
    let (pool, queue) = sqlite(&fresh).await;
    // Test-only fault injection on a throwaway store.
    sqlx::query(
        "CREATE TRIGGER boom BEFORE INSERT ON jobs WHEN NEW.dedupe_key = 'm#2' \
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END",
    )
    .execute(&pool)
    .await
    .expect("install fault");
    let error = queue
        .enqueue_all(vec![job("m", "{}"), job("m#1", "{}"), job("m#2", "{}")])
        .await
        .expect_err("the third row fails");
    assert!(error.to_string().contains("injected failure"), "{error}");
    assert_eq!(live_rows(&pool).await, 0, "the body and #1 rolled back too");

    // Control: the same batch without the fault queues all three.
    sqlx::query("DROP TRIGGER boom")
        .execute(&pool)
        .await
        .expect("remove fault");
    queue
        .enqueue_all(vec![job("m", "{}"), job("m#1", "{}"), job("m#2", "{}")])
        .await
        .expect("clean batch");
    assert_eq!(live_rows(&pool).await, 3);
}
