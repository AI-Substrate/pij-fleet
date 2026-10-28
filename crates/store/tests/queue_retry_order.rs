//! Plan 183: a retry is never released early, and Telegram rows on one chat
//! keep their order while the head backs off. Against SQLite, because SQL owns
//! both rules.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::model::{Job, JobId, Outcome};
use pij_core::ports::Queue;
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;

const SEND: &str = "sidecar:telegram:send";
const DOCUMENT: &str = "sidecar:telegram:document";

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

async fn open(fresh: &FreshStore) -> (sqlx::SqlitePool, SqliteQueue) {
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    (pool, queue)
}

/// Make a backing-off row due now, as the clock passing its deadline would.
async fn make_due(pool: &sqlx::SqlitePool, id: JobId) {
    sqlx::query("UPDATE jobs SET not_before = 0 WHERE id = ?1")
        .bind(id.0 as i64)
        .execute(pool)
        .await
        .expect("make due");
}

fn kinds(list: &[&str]) -> Vec<String> {
    list.iter().map(|kind| (*kind).to_string()).collect()
}

/// The 100 ms Telegram retry truncated to `unixepoch() + 0` and ran again on
/// the next 250 ms tick; a 1 s retry could run up to a second early.
#[tokio::test]
async fn retry_positive_delay_never_becomes_eligible_early() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let id = queue
        .enqueue(job("test:retry", "k", "m-retry"))
        .await
        .expect("enqueue");
    for delay in [
        Duration::from_nanos(1),
        Duration::from_millis(100),
        Duration::from_secs(1),
        Duration::from_millis(1_001),
    ] {
        queue
            .claim(&kinds(&["test:retry"]), "w")
            .await
            .expect("claim")
            .expect("due row");
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch");
        queue.retry(id, delay).await.expect("retry");
        let not_before: i64 = sqlx::query_scalar("SELECT not_before FROM jobs WHERE id = ?1")
            .bind(id.0 as i64)
            .fetch_one(&pool)
            .await
            .expect("persisted deadline");
        assert!(
            Duration::from_secs(u64::try_from(not_before).expect("positive epoch"))
                >= before + delay,
            "a {delay:?} retry must not be due before {:?}, stored {not_before}",
            before + delay
        );
        assert!(
            queue
                .claim(&kinds(&["test:retry"]), "w")
                .await
                .expect("claim")
                .is_none(),
            "a {delay:?} retry must not be claimable at once"
        );
        make_due(&pool, id).await;
    }
    // Zero is still immediate.
    queue
        .claim(&kinds(&["test:retry"]), "w")
        .await
        .expect("claim")
        .expect("due row");
    queue.retry(id, Duration::ZERO).await.expect("retry now");
    assert!(
        queue
            .claim(&kinds(&["test:retry"]), "w")
            .await
            .expect("claim")
            .is_some(),
        "a zero-delay retry is due at once"
    );
}

/// The body is retrying; its files on the same chat wait behind it, a
/// different chat does not, and once the head fails terminally the rest go.
#[tokio::test]
async fn telegram_rows_on_one_chat_keep_fifo_while_the_head_backs_off() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    let telegram = kinds(&[SEND, DOCUMENT]);
    let body = queue.enqueue(job(SEND, "42", "m")).await.expect("body");
    let file1 = queue.enqueue(job(DOCUMENT, "42", "m#1")).await.expect("#1");
    let file2 = queue.enqueue(job(DOCUMENT, "42", "m#2")).await.expect("#2");
    let other = queue
        .enqueue(job(SEND, "other-chat", "o"))
        .await
        .expect("other chat");

    let (head, _) = queue
        .claim(&telegram, "tg")
        .await
        .expect("claim")
        .expect("head");
    assert_eq!(head, body);
    queue
        .retry(body, Duration::from_secs(60))
        .await
        .expect("back off");

    let (next, _) = queue
        .claim(&telegram, "tg")
        .await
        .expect("claim")
        .expect("another chat is not held");
    assert_eq!(next, other, "a different chat proceeds");
    queue.ack(other, Outcome::Done).await.expect("ack other");
    assert_eq!(
        queue.claim(&telegram, "tg").await.expect("claim"),
        None,
        "#1 and #2 must not overtake the backing-off body"
    );

    // The worker's attempt cap ends the head terminally; the rest follow, in order.
    make_due(&pool, body).await;
    let (head, _) = queue
        .claim(&telegram, "tg")
        .await
        .expect("claim")
        .expect("head due");
    assert_eq!(head, body);
    queue
        .ack(
            body,
            Outcome::Failed {
                reason: "gave up".into(),
            },
        )
        .await
        .expect("fail head");
    for expected in [file1, file2] {
        let (id, _) = queue
            .claim(&telegram, "tg")
            .await
            .expect("claim")
            .expect("next in order");
        assert_eq!(id, expected);
        queue.ack(id, Outcome::Done).await.expect("ack");
    }
}

/// Every other kind keeps claiming the lowest-id ELIGIBLE row: a backing-off
/// row does not hold back later rows on its key.
#[tokio::test]
async fn other_kinds_are_not_held_behind_a_backing_off_row() {
    let fresh = FreshStore::new();
    let (_pool, queue) = open(&fresh).await;
    for kind in ["remote:laptop", "delivery:pij-x", "sidecar:bg"] {
        let first = queue
            .enqueue(job(kind, "pij-x", &format!("{kind}-1")))
            .await
            .expect("first");
        let second = queue
            .enqueue(job(kind, "pij-x", &format!("{kind}-2")))
            .await
            .expect("second");
        let (claimed, _) = queue
            .claim(&kinds(&[kind]), "w")
            .await
            .expect("claim")
            .expect("first");
        assert_eq!(claimed, first);
        queue
            .retry(first, Duration::from_secs(60))
            .await
            .expect("back off");
        let (claimed, _) = queue
            .claim(&kinds(&[kind]), "w")
            .await
            .expect("claim")
            .expect("second is not held");
        assert_eq!(claimed, second, "{kind}: unchanged claim order");
        queue.ack(second, Outcome::Done).await.expect("ack");
    }
}

/// Cost (AGENTS.md § Resource cost): the claim runs every 250 ms, so the FIFO
/// sub-query must ride an index, never scan the queue.
#[tokio::test]
async fn the_fifo_claim_uses_an_index_at_a_realistic_queue_size() {
    let fresh = FreshStore::new();
    let (pool, queue) = open(&fresh).await;
    // A busy store: many finished rows and some live ones across kinds.
    for i in 0..5_000 {
        let kind = match i % 4 {
            0 => SEND,
            1 => DOCUMENT,
            2 => "delivery:pij-a",
            _ => "sidecar:bg",
        };
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
    let explain = format!("EXPLAIN QUERY PLAN {}", pij_store::queue::CLAIM_NEXT_SQL);
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&explain)
        .bind(serde_json::json!([SEND, DOCUMENT]).to_string())
        .bind(pij_store::queue::FIFO_KIND_PATTERN)
        .fetch_all(&pool)
        .await
        .expect("explain");
    let details: Vec<&str> = plan.iter().map(|row| row.3.as_str()).collect();
    println!("EXPLAIN QUERY PLAN (5000 rows):\n{}", details.join("\n"));
    let earlier = details
        .iter()
        .find(|line| line.contains("earlier"))
        .expect("the FIFO sub-query appears in the plan");
    assert!(
        earlier.contains("USING INDEX") || earlier.contains("USING COVERING INDEX"),
        "the FIFO sub-query must search an index: {earlier}"
    );
    assert!(
        !details
            .iter()
            .any(|line| line.starts_with("SCAN jobs") || line.starts_with("SCAN earlier")),
        "no full scan of jobs on a tick: {details:?}"
    );
}
