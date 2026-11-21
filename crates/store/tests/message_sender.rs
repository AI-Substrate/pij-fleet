//! F270: a message's identity includes its sender seat (migration 0050).
//!
//! A msg_id is chosen by its sender, so it is unique per sender only: the same
//! id from two seats is two messages. Ledger rows from before 0050 record no
//! sender and match any, so a retry across the upgrade is still one message.

use pij_core::model::{DeliveryOrigin, Job, Outcome, SeatId};
use pij_core::ports::{DeliveryEnqueue, Queue};
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;

fn delivery(recipient: &str, from: &str, msg_id: &str) -> Job {
    Job {
        kind: format!("delivery:{recipient}"),
        serial_key: recipient.to_string(),
        payload: serde_json::json!({ "from": from, "to": recipient, "body": "hello" }).to_string(),
        dedupe_key: msg_id.to_string(),
        dedupe_origin: None,
        attempt: 0,
    }
}

fn queued(enqueue: DeliveryEnqueue) -> pij_core::model::JobId {
    match enqueue {
        DeliveryEnqueue::Queued { job_id, .. } => job_id,
        other => panic!("expected queued, got {other:?}"),
    }
}

/// The same id from a second seat is its own message: a row of its own while
/// the first is queued, and still a new message once the first is delivered.
#[tokio::test]
async fn the_same_id_from_two_seats_is_two_messages() {
    let fresh = FreshStore::new();
    let queue =
        SqliteQueue::new(pij_store::open(&fresh.path()).await.unwrap(), 300, 1_024).unwrap();
    let first = queued(
        queue
            .enqueue_delivery(delivery("pij-reader", "pij-first", "shared"))
            .await
            .unwrap(),
    );
    let second = queued(
        queue
            .enqueue_delivery(delivery("pij-reader", "pij-second", "shared"))
            .await
            .unwrap(),
    );
    assert_ne!(first, second, "the second seat's body is never collapsed");
    let (claimed, _) = queue
        .claim(&["delivery:pij-reader".to_string()], "worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed, first);
    queue
        .ack_delivery(first, DeliveryOrigin::ReaderRead)
        .await
        .unwrap();
    // The first seat's message is delivered; the second seat's retry of its own
    // message is still its queued row, never "already delivered".
    assert_eq!(
        queued(
            queue
                .enqueue_delivery(delivery("pij-reader", "pij-second", "shared"))
                .await
                .unwrap()
        ),
        second
    );
    assert_eq!(
        queue
            .enqueue_delivery(delivery("pij-reader", "pij-first", "shared"))
            .await
            .unwrap(),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead),
        "the first seat's own retry is the delivered message"
    );
}

/// F270 (B322-01): no path writes a NEW match-any-sender ('') ledger row.
/// The generic enqueue takes any payload, so a sender-less delivery can reach
/// a claim; its ack, the final ledger writer, refuses it and the claim stays
/// running. A named sender's message under the same id is then still queued
/// and delivered, never "already delivered".
#[tokio::test]
async fn no_ack_writes_a_sender_less_ledger_row() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let mut senderless = delivery("pij-reader", "", "squat");
    senderless.payload = "{}".to_string();
    queue.enqueue(senderless).await.unwrap();
    let (claimed, _) = queue
        .claim(&["delivery:pij-reader".to_string()], "worker")
        .await
        .unwrap()
        .unwrap();
    let acked = queue
        .ack_delivery(claimed, DeliveryOrigin::ReaderRead)
        .await;
    assert!(acked.is_err(), "the ack writes no '' row: {acked:?}");
    let wildcards: i64 =
        sqlx::query_scalar("SELECT count(*) FROM delivered_messages WHERE sender_seat = ''")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(wildcards, 0);
    assert!(
        queue.claimed_delivery(claimed).await.unwrap().is_some(),
        "a refused ack leaves the claim running"
    );
    // Its reader gives up on it; a plain ack writes no ledger row.
    queue.ack(claimed, Outcome::Done).await.unwrap();

    // A real sender's same id is its own message, queued and delivered.
    let named = queued(
        queue
            .enqueue_delivery(delivery("pij-reader", "pij-sender", "squat"))
            .await
            .unwrap(),
    );
    let (next, _) = queue
        .claim(&["delivery:pij-reader".to_string()], "worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, named);
    queue
        .ack_delivery(named, DeliveryOrigin::ReaderRead)
        .await
        .unwrap();
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT sender_seat FROM delivered_messages WHERE recipient = 'pij-reader'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, ["pij-sender"]);
}

/// Migration 0050 rebuilds the delivered ledger: every row survives with its
/// sequence and content, and records no sender (''). That '' matches any
/// sender (a brake), so a retry across the upgrade still finds its delivery;
/// every row written after it names its sender.
#[tokio::test]
async fn migration_0050_keeps_every_ledger_row_and_a_retry_across_it_is_one_message() {
    let fresh = FreshStore::new();
    let ledger = "SELECT seq, recipient, sender_machine, msg_id, origin, delivered_at \
                  FROM delivered_messages ORDER BY seq";
    let before: Vec<(i64, String, String, String, String, i64)> = {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fresh.path())
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                pij_store::migrate::MIGRATIONS
                    .iter()
                    .filter(|migration| migration.version <= 49)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        }
        .run(&pool)
        .await
        .expect("main's migrations through 0049");
        sqlx::raw_sql(
            "INSERT INTO delivered_messages (seq, recipient, sender_machine, msg_id, origin, delivered_at) VALUES \
               (7, 'pij-reader', '', 'before-upgrade', 'reader-read', 100), \
               (9, 'pij-reader', 'laptop', 'from-a-peer', 'typed-to-pane', 200), \
               (12, 'pij-other', '', 'before-upgrade', 'injected-to-transport', 300); \
             INSERT INTO jobs (kind, serial_key, payload, dedupe_key, state, enqueued_at) VALUES \
               ('delivery:pij-reader', 'pij-reader', '{\"from\":\"pij-first\"}', 'queued-before', 'pending', 1), \
               ('telegram', 'pij-reader', '{\"from\":\"pij-first\"}', 'not-a-delivery', 'pending', 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let rows = sqlx::query_as(ledger).fetch_all(&pool).await.unwrap();
        pool.close().await;
        rows
    };

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("migrates 49 → 50");
    let after: Vec<(i64, String, String, String, String, i64)> =
        sqlx::query_as(ledger).fetch_all(&pool).await.unwrap();
    assert_eq!(
        after, before,
        "every ledger row survives, sequence and content"
    );
    let senders: Vec<String> =
        sqlx::query_scalar("SELECT sender_seat FROM delivered_messages ORDER BY seq")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        senders,
        ["", "", ""],
        "no row from before 0050 records a sender"
    );
    let derived: Vec<(String, String)> =
        sqlx::query_as("SELECT dedupe_key, dedupe_sender FROM jobs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        derived,
        [
            ("queued-before".to_string(), "pij-first".to_string()),
            ("not-a-delivery".to_string(), String::new()),
        ]
    );

    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    // The brake: a retry of a message delivered before the upgrade is still
    // that message, whoever sends it.
    assert_eq!(
        queue
            .enqueue_delivery(delivery("pij-reader", "pij-first", "before-upgrade"))
            .await
            .unwrap(),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
    );
    assert_eq!(
        queue
            .note_delivered(
                &SeatId::from("pij-other"),
                "before-upgrade",
                None,
                &SeatId::from("pij-first"),
                DeliveryOrigin::TypedToPane,
            )
            .await
            .unwrap(),
        Some(DeliveryOrigin::InjectedToTransport)
    );
    // A retry of the job queued before the upgrade collapses onto it.
    let (queued_before,): (i64,) =
        sqlx::query_as("SELECT id FROM jobs WHERE dedupe_key = 'queued-before'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        queued(
            queue
                .enqueue_delivery(delivery("pij-reader", "pij-first", "queued-before"))
                .await
                .unwrap()
        )
        .0,
        queued_before as u64
    );
    // Every row written after the upgrade names its sender.
    let (job, _) = queue
        .claim(&["delivery:pij-reader".to_string()], "worker")
        .await
        .unwrap()
        .unwrap();
    queue
        .ack_delivery(job, DeliveryOrigin::ReaderRead)
        .await
        .unwrap();
    queue
        .note_delivered(
            &SeatId::from("pij-reader"),
            "typed-after",
            None,
            &SeatId::from("pij-second"),
            DeliveryOrigin::TypedToPane,
        )
        .await
        .unwrap();
    let new_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT msg_id, sender_seat FROM delivered_messages WHERE seq > 12 ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        new_rows,
        [
            ("queued-before".to_string(), "pij-first".to_string()),
            ("typed-after".to_string(), "pij-second".to_string()),
        ]
    );
}
