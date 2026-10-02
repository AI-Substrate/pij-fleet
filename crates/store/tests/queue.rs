//! The queue primitive, against a real database (workshop 001 R4).
//!
//! Dedupe, serialization, FIFO and terminal claim expiry are asserted against
//! SQLite rather than a fake because SQL owns the invariants. A fake-only test
//! would be testing the fake's opinion of the rule, not the rule.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::config::Config;
use pij_core::model::{DeliveryOrigin, Job, JobId, Outcome, SeatId};
use pij_core::ports::{DeferNoopReason, DeferOutcome, DeliveryEnqueue, Queue, ReleaseOutcome};
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use pij_testkit::contract::queue_contract;
use pij_testkit::fakes::FakeQueue;
use tokio::sync::Barrier;

fn job(kind: &str, serial: &str, dedupe: &str) -> Job {
    Job {
        kind: kind.to_string(),
        serial_key: serial.to_string(),
        payload: "{}".to_string(),
        dedupe_key: dedupe.to_string(),
        attempt: 0,
    }
}

async fn crate_queue(fresh: &FreshStore) -> SqliteQueue {
    SqliteQueue::new(
        pij_store::open(&fresh.path()).await.expect("open"),
        Config::default().claim_lease_secs,
        Config::default().delivered_id_capacity,
    )
    .expect("valid default queue policy")
}

#[tokio::test]
async fn the_sqlite_queue_honours_the_same_contract_the_fake_does() {
    let fresh = FreshStore::new();
    queue_contract(&crate_queue(&fresh).await).await;
}

async fn deferral_diagnostics_contract(queue: &dyn Queue, spine: &dyn pij_core::ports::Spine) {
    let seat = SeatId::from("pij-deferral");
    let body = job("delivery:pij-deferral", seat.as_str(), "preserved");
    let id = queue.enqueue(body.clone()).await.unwrap();
    for (reason, at, sampled) in [
        ("unrecognized", 1_000, true),
        ("unrecognized", 1_001, false),
        ("composer-busy", 1_002, false),
        ("composer-busy", 60_999, false),
        ("composer-busy", 61_000, true),
        ("unrecognized", 121_000, true),
    ] {
        let events = queue
            .record_delivery_deferral(id, reason, None, at, spine)
            .await
            .unwrap();
        assert_eq!(events.len(), usize::from(sampled));
        assert!(
            events
                .iter()
                .all(|event| event.seq.is_some() && event.kind == "delivery.held")
        );
    }
    let facts = queue.delivery_deferrals(&seat).await.unwrap();
    assert_eq!(
        facts,
        [pij_core::model::DeliveryDeferral {
            job_id: id,
            msg_id: "preserved".into(),
            reason: "unrecognized".into(),
            count: 6,
            since_ms: 1_000,
        }]
    );
    assert_eq!(
        queue.peek(std::slice::from_ref(&body.kind)).await.unwrap(),
        Some((id, body.clone())),
        "diagnostics cannot mutate the message, retry count or claim"
    );
    let (claimed_id, _) = queue.claim(&[body.kind], "reader").await.unwrap().unwrap();
    assert_eq!(claimed_id, id);
    queue
        .ack_delivery(id, DeliveryOrigin::TypedToPane)
        .await
        .unwrap();
    assert!(queue.delivery_deferrals(&seat).await.unwrap().is_empty());
    assert!(
        queue
            .record_delivery_deferral(id, "too-late", None, 120_000, spine)
            .await
            .unwrap()
            .is_empty()
    );
    let events = spine
        .tail(Some(&seat), pij_core::model::Seq(0))
        .await
        .unwrap();
    assert_eq!(events.len(), 3);
    let payload: serde_json::Value = serde_json::from_str(&events[2].payload).unwrap();
    assert_eq!(payload["deferral_count"], 6);
    assert_eq!(payload["since_ms"], 1_000);
    assert_eq!(payload["reason"], "unrecognized");
    assert_eq!(payload["reason_changes"], 1);
    let payload: serde_json::Value = serde_json::from_str(&events[1].payload).unwrap();
    assert_eq!(payload["reason_changes"], 1, "reset after each sample");
}

#[tokio::test]
async fn sqlite_and_fake_deferrals_share_sampling_and_completion_contract() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool);
    deferral_diagnostics_contract(&queue, &spine).await;
    deferral_diagnostics_contract(
        &FakeQueue::new(1_024).unwrap(),
        &pij_testkit::fakes::FakeSpine::new(),
    )
    .await;
}

/// Plan 158: hold is idempotent per id, a claim takes every pending FYI oldest
/// first exactly once with one receipt, and restore returns only what it names.
async fn fyi_contract(queue: &dyn Queue, spine: &dyn pij_core::ports::Spine) {
    let seat = SeatId::from("pij-fyi");
    let other = SeatId::from("pij-other");
    let fyi = |id: &str, to: &SeatId, at: u64| pij_core::fyi::HeldFyi {
        id: id.to_string(),
        recipient: to.clone(),
        sender: SeatId::from("pij-sender"),
        body: format!("body {id}"),
        held_at_ms: at,
    };
    assert_eq!(
        queue
            .hold_fyi(&fyi("late", &seat, 20), spine)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        queue
            .hold_fyi(&fyi("early", &seat, 10), spine)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        queue
            .hold_fyi(&fyi("early", &seat, 10), spine)
            .await
            .unwrap()
            .is_empty(),
        "a retried hold of the same id is one FYI"
    );
    queue
        .hold_fyi(&fyi("theirs", &other, 5), spine)
        .await
        .unwrap();
    assert_eq!(queue.pending_fyi_count(&seat).await.unwrap(), 2);

    let (claimed, events) = queue
        .claim_fyis(&seat, "hook:omp", 30, spine)
        .await
        .unwrap();
    let ids: Vec<&str> = claimed.iter().map(|fyi| fyi.id.as_str()).collect();
    assert_eq!(ids, ["early", "late"], "oldest first, only this seat's");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "fyi.delivered");
    let receipt: serde_json::Value = serde_json::from_str(&events[0].payload).unwrap();
    assert_eq!(
        receipt,
        serde_json::json!({"ids": ["early", "late"], "via": "hook:omp"})
    );
    let (again, events) = queue
        .claim_fyis(&seat, "hook:omp", 31, spine)
        .await
        .unwrap();
    assert!(
        again.is_empty() && events.is_empty(),
        "claimed exactly once"
    );
    assert_eq!(queue.pending_fyi_count(&other).await.unwrap(), 1);

    // A carrier row claims every pending FYI in its own transaction and carries it.
    queue
        .hold_fyi(&fyi("ride", &seat, 40), spine)
        .await
        .unwrap();
    let carrier = |msg_id: &str| {
        job(
            &format!("delivery:{}", seat.as_str()),
            seat.as_str(),
            msg_id,
        )
    };
    let attach: pij_core::ports::AttachFyis = std::sync::Arc::new(|payload, fyis| {
        let ids: Vec<&str> = fyis.iter().map(|fyi| fyi.id.as_str()).collect();
        Ok(format!("{payload}+{}", ids.join(",")))
    });
    let (enqueued, events) = queue
        .enqueue_delivery_carrying_fyis(carrier("m-1"), "message:m-1", 41, attach.clone(), spine)
        .await
        .unwrap();
    assert!(matches!(enqueued, DeliveryEnqueue::Queued { .. }));
    assert_eq!(events.len(), 1);
    assert_eq!(queue.pending_fyi_count(&seat).await.unwrap(), 0);

    // A retry of the still-queued message keeps its body and claims nothing new.
    queue
        .hold_fyi(&fyi("newer", &seat, 42), spine)
        .await
        .unwrap();
    let (retried, events) = queue
        .enqueue_delivery_carrying_fyis(carrier("m-1"), "message:m-1", 43, attach.clone(), spine)
        .await
        .unwrap();
    assert!(matches!(retried, DeliveryEnqueue::Queued { .. }));
    assert!(events.is_empty(), "a queued duplicate claims nothing");
    assert_eq!(queue.pending_fyi_count(&seat).await.unwrap(), 1);
    let kinds = [format!("delivery:{}", seat.as_str())];
    let (job_id, row) = queue
        .claim(&kinds, "reader")
        .await
        .unwrap()
        .expect("carrier row");
    assert_eq!(
        row.payload, "{}+ride",
        "the carrier carries exactly what it claimed"
    );

    // A delivered message id creates no row and claims nothing either.
    queue
        .ack_delivery(job_id, DeliveryOrigin::ReaderRead)
        .await
        .unwrap();
    let (duplicate, events) = queue
        .enqueue_delivery_carrying_fyis(carrier("m-1"), "message:m-1", 44, attach.clone(), spine)
        .await
        .unwrap();
    assert!(matches!(duplicate, DeliveryEnqueue::AlreadyDelivered(_)));
    assert!(events.is_empty());
    assert_eq!(queue.pending_fyi_count(&seat).await.unwrap(), 1);

    // Plan 159 review finding 3: a flush carrier exists only for what it claims.
    // The hook claims the pile first (the race, made deterministic), so the
    // flush finds nothing: no row, no event, not a blank message.
    let (claimed, _) = queue
        .claim_fyis(&seat, "hook:omp", 45, spine)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let (flushed, events) = queue
        .enqueue_fyi_flush(
            carrier("flush-1"),
            "message:flush-1",
            46,
            attach.clone(),
            spine,
        )
        .await
        .unwrap();
    assert_eq!(flushed, None, "nothing claimed, nothing queued");
    assert!(events.is_empty());
    assert!(
        queue.claim(&kinds, "reader").await.unwrap().is_none(),
        "no blank carrier row"
    );
    // With something pending, the flush carrier claims it in its own transaction.
    queue
        .hold_fyi(&fyi("pile", &seat, 47), spine)
        .await
        .unwrap();
    let (flushed, events) = queue
        .enqueue_fyi_flush(carrier("flush-2"), "message:flush-2", 48, attach, spine)
        .await
        .unwrap();
    assert!(matches!(flushed, Some(DeliveryEnqueue::Queued { .. })));
    assert_eq!(events.len(), 1);
    let (_, row) = queue
        .claim(&kinds, "reader")
        .await
        .unwrap()
        .expect("flush row");
    assert_eq!(row.payload, "{}+pile");
    assert_eq!(queue.pending_fyi_count(&seat).await.unwrap(), 0);
}

#[tokio::test]
async fn sqlite_and_fake_queues_share_the_fyi_contract() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool);
    fyi_contract(&queue, &spine).await;
    fyi_contract(
        &FakeQueue::new(1_024).unwrap(),
        &pij_testkit::fakes::FakeSpine::new(),
    )
    .await;
}

#[tokio::test]
async fn alternating_deferrals_emit_at_most_sixty_events_per_hour() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool);
    let fake_queue = FakeQueue::new(1_024).unwrap();
    let fake_spine = pij_testkit::fakes::FakeSpine::new();
    for (queue, spine) in [
        (&queue as &dyn Queue, &spine as &dyn pij_core::ports::Spine),
        (&fake_queue, &fake_spine),
    ] {
        let seat = SeatId::from("pij-alternating");
        let id = queue
            .enqueue(job(
                "delivery:pij-alternating",
                seat.as_str(),
                "alternating",
            ))
            .await
            .unwrap();
        let mut events = Vec::new();
        for attempt in 0..720_u64 {
            let reason = if attempt % 2 == 0 {
                "human-typing"
            } else {
                "composer-busy"
            };
            events.extend(
                queue
                    .record_delivery_deferral(id, reason, None, 1_000 + attempt * 5_000, spine)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(
            events.len(),
            60,
            "reason alternation cannot bypass the job clock"
        );
        for (index, event) in events.iter().enumerate() {
            let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
            assert_eq!(payload["reason"], "human-typing");
            assert_eq!(payload["reason_changes"], if index == 0 { 0 } else { 12 });
        }
        let facts = queue.delivery_deferrals(&seat).await.unwrap();
        assert_eq!(facts[0].count, 720);
        assert_eq!(facts[0].reason, "composer-busy");
        assert_eq!(facts[0].since_ms, 1_000);
    }
}

#[tokio::test]
async fn deferrals_survive_reopen_and_terminal_rows_retain_history() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let seat = SeatId::from("pij-restart");
    let body = job("delivery:pij-restart", seat.as_str(), "restart-body");
    let id = queue.enqueue(body.clone()).await.unwrap();
    queue
        .record_delivery_deferral(id, "tap-unowned", None, 1_000, &spine)
        .await
        .unwrap();
    assert!(
        queue
            .record_delivery_deferral(id, "composer-busy", None, 1_001, &spine)
            .await
            .unwrap()
            .is_empty()
    );
    pool.close().await;

    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    assert!(
        queue
            .record_delivery_deferral(id, "composer-busy", None, 1_002, &spine)
            .await
            .unwrap()
            .is_empty(),
        "restart must not reset the rate limiter"
    );
    let facts = queue.delivery_deferrals(&seat).await.unwrap();
    assert_eq!(facts[0].count, 3);
    assert_eq!(facts[0].since_ms, 1_000);
    let events = queue
        .record_delivery_deferral(id, "human-typing", None, 61_000, &spine)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&events[0].payload).unwrap();
    assert_eq!(
        payload["reason_changes"], 2,
        "unsampled changes survive restart"
    );
    assert_eq!(payload["reason"], "human-typing");
    assert_eq!(
        queue
            .claim(&[body.kind], "reader")
            .await
            .unwrap()
            .unwrap()
            .0,
        id
    );
    queue
        .ack(
            id,
            Outcome::Failed {
                reason: "recipient gone".into(),
            },
        )
        .await
        .unwrap();
    assert!(queue.delivery_deferrals(&seat).await.unwrap().is_empty());
    let history: (String, i64, i64) = sqlx::query_as(
        "SELECT deferral_reason, deferral_count, deferral_since_ms FROM jobs WHERE id = ?1",
    )
    .bind(id.0 as i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(history, ("human-typing".into(), 4, 1_000));
}

#[tokio::test]
async fn deferral_event_failure_rolls_back_diagnostic_and_sampling_checkpoint() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let seat = SeatId::from("pij-atomic-deferral");
    let id = queue
        .enqueue(job("delivery:pij-atomic-deferral", seat.as_str(), "atomic"))
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_hold BEFORE INSERT ON spine_events \
         WHEN new.kind = 'delivery.held' BEGIN SELECT RAISE(ABORT, 'scripted hold append'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        queue
            .record_delivery_deferral(id, "unrecognized", None, 1_000, &spine)
            .await
            .is_err()
    );
    assert!(queue.delivery_deferrals(&seat).await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER reject_hold")
        .execute(&pool)
        .await
        .unwrap();
    let events = queue
        .record_delivery_deferral(id, "unrecognized", None, 1_001, &spine)
        .await
        .unwrap();
    assert_eq!(
        events.len(),
        1,
        "failed publication must not consume the sampling slot"
    );
    assert_eq!(queue.delivery_deferrals(&seat).await.unwrap()[0].count, 1);
}

async fn claimed_delivery_lookup_contract(queue: &dyn Queue) {
    for missing in [JobId(0), JobId(u64::MAX)] {
        assert_eq!(
            queue.claimed_delivery(missing).await.expect("unknown id"),
            None
        );
    }
    let mut first = job("delivery:seat-a", "seat-a", "shared-message");
    first.payload = "{\"body\":\"first destination\",\"from\":\"sender\"}".to_string();
    let first_id = queue
        .enqueue(first.clone())
        .await
        .expect("enqueue first destination");
    let mut second = job("delivery:seat-b", "seat-b", "shared-message");
    second.payload = "{\"body\":\"second destination\",\"from\":\"sender\"}".to_string();
    let second_id = queue
        .enqueue(second.clone())
        .await
        .expect("enqueue second destination");
    assert_ne!(first_id, second_id);
    assert_eq!(
        queue
            .claimed_delivery(first_id)
            .await
            .expect("pending lookup"),
        None
    );

    let first_kinds = [first.kind.clone()];
    let first_claim = queue
        .claim(&first_kinds, "reader-a")
        .await
        .expect("claim first")
        .expect("first job");
    assert_eq!(first_claim, (first_id, first.clone()));
    for _ in 0..2 {
        assert_eq!(
            queue
                .claimed_delivery(first_id)
                .await
                .expect("running lookup"),
            Some(first.clone())
        );
        assert_eq!(
            queue
                .claimed_delivery(second_id)
                .await
                .expect("other pending lookup"),
            None
        );
        assert_eq!(
            queue
                .peek(&first_kinds)
                .await
                .expect("lookup preserves claim"),
            Some(first_claim.clone())
        );
    }
    assert!(
        queue
            .claim(&first_kinds, "other-reader")
            .await
            .expect("claim remains held")
            .is_none()
    );

    let second_claim = queue
        .claim(&[second.kind.clone()], "reader-b")
        .await
        .expect("claim second")
        .expect("second job");
    assert_eq!(second_claim, (second_id, second.clone()));
    assert_eq!(
        queue
            .claimed_delivery(first_id)
            .await
            .expect("first id stays exact"),
        Some(first.clone())
    );
    assert_eq!(
        queue
            .claimed_delivery(second_id)
            .await
            .expect("second id stays exact"),
        Some(second)
    );

    queue
        .retry(first_id, std::time::Duration::ZERO)
        .await
        .expect("retry first");
    assert_eq!(
        queue
            .claimed_delivery(first_id)
            .await
            .expect("retried pending lookup"),
        None
    );
    let (retried_id, retried) = queue
        .claim(&first_kinds, "reader-a")
        .await
        .expect("claim retry")
        .expect("retry job");
    assert_eq!(retried_id, first_id);
    first.attempt = 1;
    assert_eq!(retried, first);
    assert_eq!(
        queue
            .claimed_delivery(first_id)
            .await
            .expect("lookup retains attempt and payload"),
        Some(first)
    );
    queue
        .ack_delivery(first_id, DeliveryOrigin::ReaderRead)
        .await
        .expect("complete delivery");
    assert_eq!(
        queue.claimed_delivery(first_id).await.expect("done lookup"),
        None
    );
    queue
        .ack(
            second_id,
            Outcome::Failed {
                reason: "reader refused".to_string(),
            },
        )
        .await
        .expect("fail second");
    assert_eq!(
        queue
            .claimed_delivery(second_id)
            .await
            .expect("failed lookup"),
        None
    );

    for non_delivery in [
        job("chore:seat-c", "seat-c", "chore"),
        job("Delivery:seat-d", "seat-d", "wrong-case"),
        job("delivery:another-seat", "seat-e", "wrong-destination"),
    ] {
        let kind = non_delivery.kind.clone();
        let id = queue
            .enqueue(non_delivery)
            .await
            .expect("enqueue non-delivery");
        let (claimed_id, _) = queue
            .claim(&[kind], "worker")
            .await
            .expect("claim non-delivery")
            .expect("job");
        assert_eq!(claimed_id, id);
        assert_eq!(
            queue
                .claimed_delivery(id)
                .await
                .expect("non-delivery lookup"),
            None
        );
        queue
            .ack(id, Outcome::Done)
            .await
            .expect("lookup did not consume non-delivery claim");
    }
}

#[tokio::test]
async fn sqlite_claimed_delivery_lookup_is_exact_read_only_and_state_scoped() {
    let fresh = FreshStore::new();
    claimed_delivery_lookup_contract(&crate_queue(&fresh).await).await;
}

#[tokio::test]
async fn fake_claimed_delivery_lookup_matches_sqlite_contract() {
    let queue = FakeQueue::new(Config::default().delivered_id_capacity).expect("queue policy");
    claimed_delivery_lookup_contract(&queue).await;
}

#[tokio::test]
async fn claimed_delivery_lookup_does_not_refresh_or_expire_a_running_lease() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let config = Config::default();
    let queue = SqliteQueue::new(
        pool.clone(),
        config.claim_lease_secs,
        config.delivered_id_capacity,
    )
    .expect("queue policy");
    let expected = job("delivery:seat-a", "seat-a", "message");
    let id = queue.enqueue(expected.clone()).await.expect("enqueue");
    queue
        .claim(std::slice::from_ref(&expected.kind), "reader")
        .await
        .expect("claim")
        .expect("job");
    sqlx::query("UPDATE jobs SET claimed_at=1 WHERE id=?1")
        .bind(id.0 as i64)
        .execute(&pool)
        .await
        .expect("age the claim without sleeping");
    for _ in 0..2 {
        assert_eq!(
            queue
                .claimed_delivery(id)
                .await
                .expect("inspect expired running claim"),
            Some(expected.clone())
        );
    }
    let state: (String, String, i64, i64) =
        sqlx::query_as("SELECT state, worker, claimed_at, attempt FROM jobs WHERE id=?1")
            .bind(id.0 as i64)
            .fetch_one(&pool)
            .await
            .expect("read untouched claim");
    assert_eq!(state, ("running".to_string(), "reader".to_string(), 1, 0));
}

#[tokio::test]
async fn n_rapid_submits_collapse_to_one_row() {
    // The rule, stated as the operator experiences it: hammer the same work ten
    // times and one unit of work exists, not ten.
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;

    let mut ids = Vec::new();
    for _ in 0..10 {
        ids.push(
            queue
                .enqueue(job("deliver", "seat-a", "msg-1"))
                .await
                .expect("enqueue"),
        );
    }

    assert_eq!(
        queue.live_len().await.expect("live_len"),
        1,
        "ten submits of one dedupe key must leave ONE live row"
    );
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "every caller must get the id of the surviving row, not a phantom: {ids:?}"
    );
}

#[tokio::test]
async fn an_acked_key_is_free_again_so_dedupe_is_per_burst_not_forever() {
    // The other half of the rule, and the one a naive unique index gets wrong:
    // the same work CAN be scheduled again once the previous attempt finished.
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;

    let first = queue
        .enqueue(job("deliver", "seat-a", "msg-1"))
        .await
        .expect("enqueue");
    let claimed = queue
        .claim(&["deliver".to_string()], "w1")
        .await
        .expect("claim")
        .expect("a job is available");
    assert_eq!(claimed.0, first);

    // Still live while RUNNING: a re-submit during execution must not create a
    // second row, or the work runs twice.
    let during = queue
        .enqueue(job("deliver", "seat-a", "msg-1"))
        .await
        .expect("enqueue");
    assert_eq!(during, first, "a running job still owns its dedupe key");

    queue.ack(first, Outcome::Done).await.expect("ack");

    let after = queue
        .enqueue(job("deliver", "seat-a", "msg-1"))
        .await
        .expect("enqueue");
    assert_ne!(
        after, first,
        "once acked, the same work may be scheduled again — otherwise a key is \
         poisoned for the life of the database"
    );
}

#[tokio::test]
async fn one_entity_is_worked_by_one_worker_at_a_time() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;

    queue
        .enqueue(job("deliver", "seat-a", "m1"))
        .await
        .expect("enqueue");
    queue
        .enqueue(job("deliver", "seat-a", "m2"))
        .await
        .expect("enqueue");
    queue
        .enqueue(job("deliver", "seat-b", "m3"))
        .await
        .expect("enqueue");

    let first = queue
        .claim(&["deliver".to_string()], "w1")
        .await
        .expect("claim")
        .expect("job");
    assert_eq!(first.1.serial_key, "seat-a");

    // seat-b is free, so work continues — serialization is PER ENTITY, not a
    // global lock. A global lock would be correct and useless.
    let second = queue
        .claim(&["deliver".to_string()], "w2")
        .await
        .expect("claim")
        .expect("job");
    assert_eq!(second.1.serial_key, "seat-b");

    // ...but seat-a's second job waits.
    assert!(
        queue
            .claim(&["deliver".to_string()], "w3")
            .await
            .expect("claim")
            .is_none(),
        "seat-a already has a running job; its next one must wait"
    );

    queue.ack(first.0, Outcome::Done).await.expect("ack");
    let resumed = queue
        .claim(&["deliver".to_string()], "w3")
        .await
        .expect("claim")
        .expect("job");
    assert_eq!(resumed.1.serial_key, "seat-a");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_claimers_partition_the_work_with_no_double_claim() {
    // The contention test is judged by SET EQUALITY: every job is claimed once,
    // and the union across workers equals what was enqueued. Cross-entity worker
    // scheduling is deliberately not ordered; FIFO is asserted per serial key.
    let fresh = FreshStore::new();
    let queue = Arc::new(crate_queue(&fresh).await);

    let expected: Vec<String> = (0..24).map(|n| format!("entity-{n}")).collect();
    for serial in &expected {
        queue
            .enqueue(job("deliver", serial, &format!("d-{serial}")))
            .await
            .expect("enqueue");
    }

    let mut workers = Vec::new();
    for worker in 0..8 {
        let queue = Arc::clone(&queue);
        workers.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            while let Some((id, job)) = queue
                .claim(&["deliver".to_string()], &format!("w{worker}"))
                .await
                .expect("claim")
            {
                mine.push(job.serial_key.clone());
                queue.ack(id, Outcome::Done).await.expect("ack");
            }
            mine
        }));
    }

    let mut claimed = Vec::new();
    for worker in workers {
        claimed.extend(worker.await.expect("worker panicked"));
    }

    let mut sorted = claimed.clone();
    sorted.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();

    assert_eq!(
        sorted, expected_sorted,
        "every enqueued job must be claimed exactly once across all workers"
    );
    assert_eq!(
        queue.live_len().await.expect("live_len"),
        0,
        "no live rows may survive a drained queue"
    );
}

#[tokio::test]
async fn acking_a_job_nobody_is_running_is_reported_not_swallowed() {
    // A worker acking work the queue does not think is running means a claim was
    // lost. Silently succeeding here is how that stays invisible until the job
    // runs twice.
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;
    let id = queue
        .enqueue(job("deliver", "seat-a", "m1"))
        .await
        .expect("enqueue");

    let error = queue
        .ack(id, Outcome::Done)
        .await
        .expect_err("acking an unclaimed job must fail");
    assert!(
        error.to_string().contains("not running"),
        "the error must say what was wrong: {error}"
    );

    let (claimed, _) = queue
        .claim(&["deliver".to_string()], "w1")
        .await
        .expect("claim")
        .expect("job");
    queue.ack(claimed, Outcome::Done).await.expect("ack");
    assert!(
        queue.ack(claimed, Outcome::Done).await.is_err(),
        "a double-ack is the same lost-claim signal and must also be refused"
    );
}

#[tokio::test]
async fn a_failed_job_keeps_its_reason_and_frees_its_entity() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;
    queue
        .enqueue(job("deliver", "seat-a", "m1"))
        .await
        .expect("enqueue");
    let (id, _) = queue
        .claim(&["deliver".to_string()], "w1")
        .await
        .expect("claim")
        .expect("job");

    queue
        .ack(
            id,
            Outcome::Failed {
                reason: "transport refused".to_string(),
            },
        )
        .await
        .expect("ack");

    assert_eq!(queue.live_len().await.expect("live_len"), 0);
    // A retry is a NEW row, so the record of the failed attempt survives.
    let retry = queue
        .enqueue(job("deliver", "seat-a", "m1"))
        .await
        .expect("enqueue");
    assert_ne!(retry, id);
}

#[tokio::test]
async fn claim_respects_the_kind_filter_and_an_empty_filter_claims_nothing() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;
    queue
        .enqueue(job("deliver", "seat-a", "m1"))
        .await
        .expect("enqueue");

    assert!(queue.claim(&[], "w1").await.expect("claim").is_none());
    assert!(
        queue
            .claim(&["chore".to_string()], "w1")
            .await
            .expect("claim")
            .is_none()
    );
    assert!(
        queue
            .claim(&["chore".to_string(), "deliver".to_string()], "w1")
            .await
            .expect("claim")
            .is_some(),
        "a kind list matches any of its entries"
    );
}

#[tokio::test]
async fn fifo_within_a_serial_key_survives_interleaved_entities() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;

    for (serial, dedupe) in [
        ("seat-a", "a-1"),
        ("seat-b", "b-1"),
        ("seat-a", "a-2"),
        ("seat-a", "a-3"),
    ] {
        queue
            .enqueue(job("deliver", serial, dedupe))
            .await
            .expect("enqueue");
    }

    let first = queue
        .claim(&["deliver".to_string()], "w1")
        .await
        .expect("claim")
        .expect("first job");
    assert_eq!(first.1.dedupe_key, "a-1");

    let other = queue
        .claim(&["deliver".to_string()], "w2")
        .await
        .expect("claim")
        .expect("other serial remains runnable");
    assert_eq!(other.1.dedupe_key, "b-1");
    queue.ack(other.0, Outcome::Done).await.expect("ack b");

    let mut current = first.0;
    for expected in ["a-2", "a-3"] {
        queue.ack(current, Outcome::Done).await.expect("ack a");
        let next = queue
            .claim(&["deliver".to_string()], "w1")
            .await
            .expect("claim")
            .expect("next serial job");
        assert_eq!(next.1.dedupe_key, expected, "serial-key order is FIFO");
        current = next.0;
    }
    queue.ack(current, Outcome::Done).await.expect("final ack");
}

#[tokio::test]
async fn an_expired_claim_fails_terminally_and_rejects_the_stale_ack() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");

    let expired = queue
        .enqueue(job("deliver", "seat-a", "a-1"))
        .await
        .expect("enqueue expired attempt");
    queue
        .enqueue(job("deliver", "seat-a", "a-2"))
        .await
        .expect("enqueue successor");
    let claimed = queue
        .claim(&["deliver".to_string()], "stale-worker")
        .await
        .expect("claim")
        .expect("first attempt");
    assert_eq!(claimed.0, expired);
    assert!(
        queue
            .claim(&["deliver".to_string()], "next-worker")
            .await
            .expect("claim")
            .is_none(),
        "an unexpired claim still owns its serial key"
    );

    sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 301 WHERE id = ?1")
        .bind(expired.0 as i64)
        .execute(&pool)
        .await
        .expect("age the claim");

    let successor = queue
        .claim(&["deliver".to_string()], "next-worker")
        .await
        .expect("claim after expiry")
        .expect("expiry frees the serial key");
    assert_eq!(successor.1.dedupe_key, "a-2");
    assert!(
        queue.ack(expired, Outcome::Done).await.is_err(),
        "the old worker must not complete a terminally expired attempt"
    );

    let (state, outcome, acked_at): (String, Option<String>, Option<i64>) =
        sqlx::query_as("SELECT state, outcome, acked_at FROM jobs WHERE id = ?1")
            .bind(expired.0 as i64)
            .fetch_one(&pool)
            .await
            .expect("read expired attempt");
    assert_eq!(state, "failed");
    assert!(outcome.is_some_and(|text| text.contains("claim lease expired")));
    assert!(
        acked_at.is_some(),
        "terminal expiry records when it happened"
    );

    let retry = queue
        .enqueue(job("deliver", "seat-a", "a-1"))
        .await
        .expect("explicit retry");
    assert_ne!(retry, expired, "retry is a new attempt, never a requeue");
}

#[tokio::test]
async fn an_expired_inbox_claim_retries_instead_of_losing_the_unacked_message() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let kind = "delivery:pij-reader";
    let id = queue
        .enqueue_delivery(job(kind, "pij-reader", "m-1"))
        .await
        .expect("enqueue delivery");
    let DeliveryEnqueue::Queued { job_id: id, .. } = id else {
        panic!("new delivery must queue");
    };
    let first = queue
        .claim(&[kind.to_string()], "first-http-client")
        .await
        .expect("first claim")
        .expect("delivery row");
    assert_eq!(first.0, id);

    sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 301 WHERE id = ?1")
        .bind(id.0 as i64)
        .execute(&pool)
        .await
        .expect("age the interrupted claim");

    let retried = queue
        .claim(&[kind.to_string()], "replacement-http-client")
        .await
        .expect("claim after expiry")
        .expect("unacknowledged delivery is retried");
    assert_eq!(retried.0, id, "Queue::retry semantics preserve the job id");
    assert_eq!(retried.1.attempt, 1, "claim expiry counts the retry");
    assert_eq!(retried.1.dedupe_key, "m-1");

    queue
        .ack_delivery(retried.0, DeliveryOrigin::ReaderRead)
        .await
        .expect("replacement client acknowledges what it read");
    assert_eq!(queue.live_len().await.expect("live count"), 0);
}

#[tokio::test]
async fn control_claim_lease_expiry_never_replays_an_unacknowledged_command() {
    for command in pij_core::control::ALLOWED_COMMANDS {
        let fresh = FreshStore::new();
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("queue policy");
        let kind = "delivery:pij-control";
        let mut control = job(kind, "pij-control", "control-1");
        control.payload = serde_json::to_string(&pij_core::model::Msg {
            from: "parent".into(),
            from_machine: None,
            to: "pij-control".into(),
            body: String::new(),
            command: Some(command.to_string()),
            msg_id: "control-1".to_string(),
            in_reply_to: None,
        })
        .expect("control message");
        let DeliveryEnqueue::Queued { job_id, .. } =
            queue.enqueue_delivery(control).await.expect("enqueue")
        else {
            panic!("new control must queue");
        };
        let claimed = queue
            .claim(&[kind.to_string()], "executing-client")
            .await
            .expect("claim")
            .expect("control");
        assert_eq!(claimed.0, job_id);
        queue
            .enqueue_delivery(job(kind, "pij-control", "next-body"))
            .await
            .expect("successor");
        sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 301 WHERE id = ?1")
            .bind(job_id.0 as i64)
            .execute(&pool)
            .await
            .expect("age missing acknowledgement");

        let successor = queue
            .claim(&[kind.to_string()], "replacement-client")
            .await
            .expect("claim after expiry")
            .expect("successor");
        assert_eq!(
            successor.1.dedupe_key, "next-body",
            "{command} may have executed before its ACK was lost and must never replay"
        );
        let (state, outcome, acked_at, attempt): (String, Option<String>, Option<i64>, i64) =
            sqlx::query_as("SELECT state, outcome, acked_at, attempt FROM jobs WHERE id = ?1")
                .bind(job_id.0 as i64)
                .fetch_one(&pool)
                .await
                .expect("expired control");
        assert_eq!(state, "failed");
        let reason = outcome.expect("honest terminal reason");
        assert!(reason.contains("outcome unknown"), "{reason}");
        assert!(reason.contains("acknowledgement missing"), "{reason}");
        assert!(acked_at.is_some());
        assert_eq!(attempt, 0, "expiry is terminal, not a retry");
        assert!(
            queue
                .ack_delivery(job_id, DeliveryOrigin::ReaderRead)
                .await
                .is_err(),
            "a stale ACK cannot certify an expired control"
        );
    }
}

#[tokio::test]
async fn control_claim_lease_brake_preserves_body_and_non_json_recovery() {
    for payload in ["{}", "{\"command\":null}", "not-json"] {
        let fresh = FreshStore::new();
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("queue policy");
        let kind = "delivery:pij-reader";
        let mut body = job(kind, "pij-reader", "body-1");
        body.payload = payload.to_string();
        let DeliveryEnqueue::Queued { job_id, .. } =
            queue.enqueue_delivery(body).await.expect("enqueue")
        else {
            panic!("new body must queue");
        };
        queue
            .claim(&[kind.to_string()], "interrupted-client")
            .await
            .expect("first claim")
            .expect("body");
        sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 301 WHERE id = ?1")
            .bind(job_id.0 as i64)
            .execute(&pool)
            .await
            .expect("age body claim");
        let retried = queue
            .claim(&[kind.to_string()], "replacement-client")
            .await
            .expect("safe JSON inspection")
            .expect("body retries");
        assert_eq!(retried.0, job_id);
        assert_eq!(retried.1.payload, payload);
        assert_eq!(retried.1.attempt, 1);
    }
}

fn extension_body_job(kind: &str, serial: &str, dedupe: &str) -> Job {
    let mut job = job(kind, serial, dedupe);
    job.payload = serde_json::to_string(&pij_core::model::Msg {
        from: "sender".into(),
        from_machine: None,
        to: serial.into(),
        body: "preserved body".into(),
        command: None,
        msg_id: dedupe.into(),
        in_reply_to: None,
    })
    .unwrap();
    job
}

#[tokio::test]
async fn extension_working_brake_never_renews_or_replays_an_expired_control() {
    let pool = pij_store::open("").await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let kinds = [String::from("delivery:pij-reader")];
    let mut control = extension_body_job(&kinds[0], "pij-reader", "control");
    let mut message: pij_core::model::Msg = serde_json::from_str(&control.payload).unwrap();
    message.command = Some("compact".into());
    control.payload = serde_json::to_string(&message).unwrap();
    let id = queue.enqueue(control).await.unwrap();
    let lease = pij_core::ports::ExtensionLease {
        seconds: 60,
        renew_working: true,
    };
    queue
        .claim_extension(&kinds, "pij-reader", lease, 0, &spine, true)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET claimed_at=unixepoch()-301 WHERE id=?")
        .bind(id.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    let expired = queue
        .claim_extension(&kinds, "pij-reader", lease, 0, &spine, true)
        .await
        .unwrap();
    assert!(expired.claimed.is_none());
    assert!(
        expired.parked.is_empty(),
        "control failure is not body recovery"
    );
    let state: (String, String, i64) =
        sqlx::query_as("SELECT state, outcome, lease_expirations FROM jobs WHERE id=?")
            .bind(id.0 as i64)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state.0, "failed");
    assert!(state.1.contains("outcome unknown"));
    assert_eq!(state.2, 0);
    assert!(
        !queue
            .heartbeat_delivery(id, &"pij-reader".into(), 0)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn extension_lease_budget_counts_expirations_not_ordinary_retries() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let kinds = [String::from("delivery:pij-reader")];
    let lease = pij_core::ports::ExtensionLease {
        seconds: 60,
        renew_working: false,
    };
    let id = queue
        .enqueue(extension_body_job(&kinds[0], "pij-reader", "body"))
        .await
        .unwrap();
    for _ in 0..5 {
        queue
            .claim_extension(&kinds, "reader", lease, 0, &spine, true)
            .await
            .unwrap();
        queue.retry(id, Duration::ZERO).await.unwrap();
    }
    queue
        .claim_extension(&kinds, "reader", lease, 0, &spine, true)
        .await
        .unwrap();
    for expiration in 1..=3 {
        sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 61 WHERE id = ?")
            .bind(id.0 as i64)
            .execute(&pool)
            .await
            .unwrap();
        let page = queue
            .claim_extension(&kinds, "reader", lease, 0, &spine, true)
            .await
            .unwrap();
        if expiration < 3 {
            assert_eq!(page.claimed.unwrap().0, id);
            assert!(page.parked.is_empty());
        } else {
            assert!(page.claimed.is_none());
            assert_eq!(page.parked[0].job_id, id);
            assert_eq!(
                page.parked[0].outcome,
                pij_core::model::DeliveryFailure::LeaseExhausted
            );
        }
    }
    assert!(
        queue
            .claim_extension(&kinds, "reader", lease, 0, &spine, true)
            .await
            .unwrap()
            .parked
            .is_empty(),
        "a terminal failure is announced once"
    );
}

#[tokio::test]
async fn extension_short_lease_excludes_controls_and_stale_operator_attempts() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let kinds = [String::from("delivery:pij-reader")];
    let lease = pij_core::ports::ExtensionLease {
        seconds: 60,
        renew_working: false,
    };
    let mut command = job(&kinds[0], "pij-reader", "control");
    command.payload = r#"{"command":"compact"}"#.into();
    let id = queue.enqueue(command).await.unwrap();
    queue
        .claim_extension(&kinds, "reader", lease, 0, &spine, true)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 61 WHERE id = ?")
        .bind(id.0 as i64)
        .execute(&pool)
        .await
        .unwrap();
    let page = queue
        .claim_extension(&kinds, "reader", lease, 0, &spine, true)
        .await
        .unwrap();
    assert!(page.claimed.is_none());
    assert!(page.parked.is_empty());
    assert!(
        queue.claimed_delivery(id).await.unwrap().is_some(),
        "controls retain the original lease"
    );
    queue.ack(id, Outcome::Done).await.unwrap();
    let body = queue
        .enqueue(extension_body_job(&kinds[0], "pij-reader", "body"))
        .await
        .unwrap();
    queue
        .claim_extension(&kinds, "reader", lease, 0, &spine, true)
        .await
        .unwrap();
    queue.retry(body, Duration::ZERO).await.unwrap();
    queue
        .claim_extension(&kinds, "reader", lease, 0, &spine, true)
        .await
        .unwrap();
    let evidence = pij_core::ports::ParkingEvidence {
        outcome: pij_core::model::DeliveryFailure::OperatorReleased,
        reason: "observed silent injection",
        at: 0,
    };
    assert!(
        queue
            .park_delivery(body, &"pij-reader".into(), 0, &evidence, &spine)
            .await
            .unwrap()
            .0
            .is_none(),
        "an operator observation cannot retire a newer running attempt"
    );
    assert!(
        queue
            .park_delivery(body, &"pij-reader".into(), 1, &evidence, &spine)
            .await
            .unwrap()
            .0
            .is_some()
    );
}

#[tokio::test]
async fn native_receiver_parking_and_manual_recovery_preserve_body_and_history() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).unwrap();
    let spine = pij_store::SqliteSpine::new(pool.clone());
    let recipient = SeatId::from("pij-reader");
    let kinds = [String::from("delivery:pij-reader")];
    let body = extension_body_job(&kinds[0], recipient.as_str(), "native-manual");
    let id = queue.enqueue(body.clone()).await.unwrap();
    let evidence = pij_core::ports::ParkingEvidence {
        outcome: pij_core::model::DeliveryFailure::NativeReceiverUnavailable,
        reason: "native-extension-unavailable",
        at: 42,
    };
    let (parked, events) = queue
        .park_delivery(id, &recipient, 0, &evidence, &spine)
        .await
        .unwrap();
    assert_eq!(
        parked,
        Some(body.clone()),
        "unclaimed mail must park without a fake claim"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "delivery.parked")
            .count(),
        1
    );
    assert!(
        queue
            .claim(&kinds, recipient.as_str())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !queue
            .recover_native_delivery(id, &"someone-else".into())
            .await
            .unwrap()
    );
    assert!(queue.recover_native_delivery(id, &recipient).await.unwrap());
    assert!(!queue.recover_native_delivery(id, &recipient).await.unwrap());
    let (claimed_id, claimed) = queue
        .claim(&kinds, "native-cli:pij-reader")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed_id, id);
    assert_eq!(claimed.payload, body.payload);
    assert_eq!(claimed.attempt, 1);
    assert!(
        queue
            .park_delivery(id, &recipient, 1, &evidence, &spine)
            .await
            .unwrap()
            .0
            .is_none(),
        "a manual reader owns its ordinary claim lease while acknowledging"
    );
    queue
        .ack_delivery(id, DeliveryOrigin::ReaderRead)
        .await
        .unwrap();
    assert!(queue.peek_parked(&kinds).await.unwrap().is_empty());
    assert!(!queue.recover_native_delivery(id, &recipient).await.unwrap());
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM spine_events WHERE kind = 'delivery.parked'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        history, 1,
        "manual recovery preserves the original parking evidence"
    );
}

#[tokio::test]
async fn delivery_enqueue_returns_the_persisted_not_before_after_backoff() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let delivery = job("delivery:pij-scheduled", "pij-scheduled", "m-scheduled");
    let DeliveryEnqueue::Queued { job_id, .. } = queue
        .enqueue_delivery(delivery.clone())
        .await
        .expect("enqueue delivery")
    else {
        panic!("new delivery must queue");
    };
    queue
        .claim(std::slice::from_ref(&delivery.kind), "schedule-worker")
        .await
        .expect("claim delivery")
        .expect("delivery row");
    queue
        .retry(job_id, std::time::Duration::from_secs(17))
        .await
        .expect("persist backoff");
    let persisted: i64 = sqlx::query_scalar("SELECT not_before FROM jobs WHERE id = ?1")
        .bind(job_id.0 as i64)
        .fetch_one(&pool)
        .await
        .expect("read persisted eligibility");

    let DeliveryEnqueue::Queued {
        job_id: collapsed,
        not_before_ms,
    } = queue
        .enqueue_delivery(delivery)
        .await
        .expect("collapse scheduled delivery")
    else {
        panic!("live delivery must stay queued");
    };
    assert_eq!(collapsed, job_id);
    assert_eq!(not_before_ms, (persisted as u64).saturating_mul(1_000));
}

async fn persisted_job(pool: &sqlx::SqlitePool, id: JobId) -> String {
    sqlx::query_scalar(
        "SELECT json_object(\
             'id', id, 'kind', kind, 'serial_key', serial_key, 'payload', payload, \
             'dedupe_key', dedupe_key, 'state', state, 'worker', worker, \
             'outcome', outcome, 'enqueued_at', enqueued_at, 'claimed_at', claimed_at, \
             'acked_at', acked_at, 'attempt', attempt, 'not_before', not_before\
         ) FROM jobs WHERE id = ?1",
    )
    .bind(id.0 as i64)
    .fetch_one(pool)
    .await
    .expect("read every persisted job field")
}

#[tokio::test]
async fn defer_running_delivery_releases_ownership_without_counting_attempt_or_delivery() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let original = delivered_job("pij-recipient", "m-deferred");
    let id = queue
        .enqueue(original.clone())
        .await
        .expect("enqueue first");
    let next = queue
        .enqueue(delivered_job("pij-recipient", "m-next"))
        .await
        .expect("enqueue next");
    let kinds = [original.kind.clone()];
    queue
        .claim(&kinds, "failed-worker")
        .await
        .expect("claim")
        .expect("first job");
    queue
        .retry(id, Duration::ZERO)
        .await
        .expect("one real retry");
    let claimed = queue
        .claim(&kinds, "reader")
        .await
        .expect("claim")
        .expect("retried job");
    assert_eq!(claimed.0, id);
    assert_eq!(claimed.1.attempt, 1);

    assert_eq!(
        queue
            .defer(id, Duration::from_secs(3_600))
            .await
            .expect("defer running delivery"),
        DeferOutcome::Deferred {
            recipient: SeatId::from("pij-recipient"),
            msg_id: "m-deferred".to_string(),
        }
    );
    let state: (
        String,
        Option<String>,
        Option<i64>,
        i64,
        Option<i64>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT state, worker, claimed_at, attempt, acked_at, outcome FROM jobs WHERE id = ?1",
    )
    .bind(id.0 as i64)
    .fetch_one(&pool)
    .await
    .expect("read deferred ownership and evidence");
    assert_eq!(state, ("pending".to_string(), None, None, 1, None, None));
    assert!(
        queue
            .ack_delivery(id, DeliveryOrigin::ReaderRead)
            .await
            .is_err(),
        "deferral revokes the old reader's claim"
    );
    let delivered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM delivered_messages")
        .fetch_one(&pool)
        .await
        .expect("read delivery ledger");
    assert_eq!(delivered, 0, "deferral is not evidence of delivery");
    let following = queue
        .claim(&kinds, "next-reader")
        .await
        .expect("claim")
        .expect("serial key is free");
    assert_eq!(
        following.0, next,
        "a delayed row does not block eligible work on its serial key"
    );
    queue
        .ack(next, Outcome::Done)
        .await
        .expect("finish next row");
    assert!(
        queue
            .claim(&kinds, "too-early")
            .await
            .expect("claim")
            .is_none()
    );
    let peeked = queue
        .peek(&kinds)
        .await
        .expect("peek")
        .expect("deferred row remains visible");
    assert_eq!(peeked.0, id);
    assert_eq!(peeked.1.attempt, 1);
}

#[tokio::test]
async fn defer_pending_delivery_replaces_deadline_and_zero_releases_it() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let original = delivered_job("pij-pending", "m-pending");
    let id = queue.enqueue(original.clone()).await.expect("enqueue");
    let expected = DeferOutcome::Deferred {
        recipient: SeatId::from("pij-pending"),
        msg_id: "m-pending".to_string(),
    };
    queue
        .note_delivered(
            &SeatId::from("pij-other"),
            "m-existing",
            DeliveryOrigin::VerifiedArrival,
        )
        .await
        .expect("seed existing delivery evidence");

    assert_eq!(
        queue
            .defer(id, Duration::from_secs(7_200))
            .await
            .expect("defer pending"),
        expected
    );
    let first: i64 = sqlx::query_scalar("SELECT not_before FROM jobs WHERE id = ?1")
        .bind(id.0 as i64)
        .fetch_one(&pool)
        .await
        .expect("first deadline");
    assert_eq!(
        queue
            .defer(id, Duration::from_secs(3_600))
            .await
            .expect("replace deadline"),
        expected
    );
    let second: i64 = sqlx::query_scalar("SELECT not_before FROM jobs WHERE id = ?1")
        .bind(id.0 as i64)
        .fetch_one(&pool)
        .await
        .expect("replacement deadline");
    assert!(
        second < first,
        "re-deferral replaces the deadline rather than extending it"
    );
    assert!(
        queue
            .claim(std::slice::from_ref(&original.kind), "early")
            .await
            .expect("claim")
            .is_none()
    );
    assert_eq!(queue.live_len().await.expect("live count"), 1);

    assert_eq!(
        queue
            .defer(id, Duration::ZERO)
            .await
            .expect("release immediately"),
        expected
    );
    let eligible: bool =
        sqlx::query_scalar("SELECT not_before <= unixepoch() FROM jobs WHERE id = ?1")
            .bind(id.0 as i64)
            .fetch_one(&pool)
            .await
            .expect("immediate eligibility");
    assert!(eligible);
    let released = queue
        .claim(std::slice::from_ref(&original.kind), "released-reader")
        .await
        .expect("claim")
        .expect("zero delay is eligible now");
    assert_eq!(released, (id, original));
    assert_eq!(
        queue
            .note_delivered(
                &SeatId::from("pij-other"),
                "m-existing",
                DeliveryOrigin::ReaderRead
            )
            .await
            .expect("read existing evidence"),
        Some(DeliveryOrigin::VerifiedArrival),
        "deferral leaves previously recorded delivery evidence intact"
    );
}

#[tokio::test]
async fn defer_preserves_delivery_body_and_original_fifo_position() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let mut original = delivered_job("pij-fifo", "m-oldest");
    original.payload =
        "{\"text\":\"keep the full body\\nexactly\",\"metadata\":[1,2,3]}".to_string();
    let oldest = queue
        .enqueue(original.clone())
        .await
        .expect("enqueue oldest");
    let later = queue
        .enqueue(delivered_job("pij-fifo", "m-later"))
        .await
        .expect("enqueue later");
    let enqueued_at: i64 = sqlx::query_scalar("SELECT enqueued_at FROM jobs WHERE id = ?1")
        .bind(oldest.0 as i64)
        .fetch_one(&pool)
        .await
        .expect("original enqueue time");
    let kinds = [original.kind.clone()];
    assert_eq!(
        queue
            .claim(&kinds, "first-reader")
            .await
            .expect("claim")
            .expect("oldest"),
        (oldest, original.clone())
    );
    queue
        .defer(oldest, Duration::from_secs(3_600))
        .await
        .expect("defer oldest");
    assert_eq!(
        queue.peek(&kinds).await.expect("peek"),
        Some((oldest, original.clone()))
    );
    assert!(
        matches!(queue.enqueue_delivery(original.clone()).await.expect("dedupe deferred body"),
        DeliveryEnqueue::Queued { job_id, .. } if job_id == oldest)
    );
    queue
        .defer(oldest, Duration::ZERO)
        .await
        .expect("release oldest");
    assert_eq!(
        queue
            .claim(&kinds, "replacement-reader")
            .await
            .expect("claim")
            .expect("oldest still first"),
        (oldest, original)
    );
    let preserved: i64 = sqlx::query_scalar("SELECT enqueued_at FROM jobs WHERE id = ?1")
        .bind(oldest.0 as i64)
        .fetch_one(&pool)
        .await
        .expect("preserved enqueue time");
    assert_eq!(preserved, enqueued_at);
    queue
        .ack(oldest, Outcome::Done)
        .await
        .expect("finish oldest");
    assert_eq!(
        queue
            .claim(&kinds, "next-reader")
            .await
            .expect("claim")
            .expect("later row")
            .0,
        later
    );
}

#[tokio::test]
async fn defer_absent_and_terminal_jobs_are_not_live() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    for absent in [JobId(0), JobId(u64::MAX)] {
        assert_eq!(
            queue
                .defer(absent, Duration::ZERO)
                .await
                .expect("absent row"),
            DeferOutcome::NotLive {
                reason: DeferNoopReason::Absent
            }
        );
    }
    for (msg_id, outcome) in [
        ("m-done", Outcome::Done),
        (
            "m-failed",
            Outcome::Failed {
                reason: "terminal failure".to_string(),
            },
        ),
    ] {
        let original = delivered_job("pij-terminal", msg_id);
        let id = queue.enqueue(original.clone()).await.expect("enqueue");
        queue
            .claim(std::slice::from_ref(&original.kind), "terminal-worker")
            .await
            .expect("claim")
            .expect("job");
        queue.ack(id, outcome).await.expect("finish terminal row");
        let before = persisted_job(&pool, id).await;
        for delay in [Duration::ZERO, Duration::from_secs(3_600)] {
            assert_eq!(
                queue.defer(id, delay).await.expect("terminal row"),
                DeferOutcome::NotLive {
                    reason: DeferNoopReason::Terminal
                }
            );
            assert_eq!(
                persisted_job(&pool, id).await,
                before,
                "terminal history must not change"
            );
        }
    }
    assert_eq!(queue.live_len().await.expect("live count"), 0);
}

#[tokio::test]
async fn defer_refuses_non_delivery_jobs_without_mutation() {
    for kind in ["pointer", "federation:remote", "delivery:wrong-recipient"] {
        let fresh = FreshStore::new();
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
        let id = queue
            .enqueue(job(kind, "pij-recipient", "m-refused"))
            .await
            .expect("enqueue");
        for running in [false, true] {
            if running {
                assert_eq!(
                    queue
                        .claim(&[kind.to_string()], "owner")
                        .await
                        .expect("claim")
                        .expect("job")
                        .0,
                    id
                );
            }
            let before = persisted_job(&pool, id).await;
            assert!(
                queue.defer(id, Duration::from_secs(3_600)).await.is_err(),
                "only a correctly addressed delivery may be deferred: {kind}"
            );
            assert_eq!(
                persisted_job(&pool, id).await,
                before,
                "refusal must not mutate the row"
            );
        }
    }
}

#[tokio::test]
async fn defer_positive_delay_never_becomes_eligible_early() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let id = queue
        .enqueue(delivered_job("pij-timing", "m-timing"))
        .await
        .expect("enqueue");
    for delay in [
        Duration::from_nanos(1),
        Duration::from_secs(1),
        Duration::from_millis(1_001),
    ] {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch");
        queue
            .defer(id, delay)
            .await
            .expect("defer positive duration");
        let not_before: i64 = sqlx::query_scalar("SELECT not_before FROM jobs WHERE id = ?1")
            .bind(id.0 as i64)
            .fetch_one(&pool)
            .await
            .expect("persisted deadline");
        assert!(
            Duration::from_secs(u64::try_from(not_before).expect("positive epoch"))
                >= before + delay,
            "integer-second storage must round the absolute deadline upward, including whole-second delays"
        );
    }
}

#[tokio::test]
async fn defer_rejects_unrepresentable_delay_without_mutation() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let original = delivered_job("pij-overflow", "m-overflow");
    let id = queue.enqueue(original.clone()).await.expect("enqueue");
    queue
        .claim(std::slice::from_ref(&original.kind), "owner")
        .await
        .expect("claim")
        .expect("job");
    let before = persisted_job(&pool, id).await;
    for delay in [Duration::MAX, Duration::from_secs(i64::MAX as u64)] {
        assert!(
            queue.defer(id, delay).await.is_err(),
            "an unrepresentable deadline must be rejected"
        );
        assert_eq!(
            persisted_job(&pool, id).await,
            before,
            "overflow must not change running ownership or evidence"
        );
    }
}

#[tokio::test]
async fn release_deferred_pending_delivery_is_immediate_without_changing_body_attempt_or_evidence()
{
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    let mut original = delivered_job("pij-release", "m-release");
    original.payload = "{\"text\":\"the complete deferred body\"}".to_string();
    let id = queue.enqueue(original.clone()).await.expect("enqueue");
    let kinds = [original.kind.clone()];
    queue
        .claim(&kinds, "failed-worker")
        .await
        .expect("claim")
        .expect("job");
    queue
        .retry(id, Duration::ZERO)
        .await
        .expect("one real retry");
    queue
        .defer(id, Duration::from_secs(3_600))
        .await
        .expect("hold pending delivery");
    queue
        .note_delivered(
            &SeatId::from("pij-other"),
            "m-existing",
            DeliveryOrigin::VerifiedArrival,
        )
        .await
        .expect("seed unrelated delivery evidence");
    let mut before: serde_json::Value =
        serde_json::from_str(&persisted_job(&pool, id).await).expect("decode pending row snapshot");
    let ledger_before: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT recipient, msg_id, origin, seq FROM delivered_messages ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .expect("existing delivery ledger");

    assert_eq!(
        queue
            .release_deferred(id)
            .await
            .expect("release pending delivery"),
        ReleaseOutcome::Released {
            recipient: SeatId::from("pij-release"),
            msg_id: "m-release".to_string(),
        }
    );
    let after: serde_json::Value = serde_json::from_str(&persisted_job(&pool, id).await)
        .expect("decode released row snapshot");
    assert_ne!(before["not_before"], after["not_before"]);
    before["not_before"] = after["not_before"].clone();
    assert_eq!(
        after, before,
        "release changes only the eligibility deadline"
    );
    let eligible: bool =
        sqlx::query_scalar("SELECT not_before <= unixepoch() FROM jobs WHERE id = ?1")
            .bind(id.0 as i64)
            .fetch_one(&pool)
            .await
            .expect("immediate eligibility");
    assert!(eligible);
    let ledger_after: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT recipient, msg_id, origin, seq FROM delivered_messages ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .expect("unchanged delivery ledger");
    assert_eq!(
        ledger_after, ledger_before,
        "release neither adds nor removes delivery evidence"
    );
    original.attempt = 1;
    assert_eq!(
        queue
            .claim(&kinds, "released-reader")
            .await
            .expect("claim")
            .expect("released delivery"),
        (id, original)
    );
}

#[tokio::test]
async fn release_deferred_duplicate_or_optimistic_release_preserves_the_active_claim() {
    for held_first in [false, true] {
        let fresh = FreshStore::new();
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
        let original = delivered_job("pij-active", "m-active");
        let id = queue.enqueue(original.clone()).await.expect("enqueue");
        let kinds = [original.kind.clone()];
        if held_first {
            queue
                .defer(id, Duration::from_secs(3_600))
                .await
                .expect("hold delivery");
            assert!(matches!(
                queue.release_deferred(id).await.expect("first release"),
                ReleaseOutcome::Released { .. }
            ));
        }
        assert_eq!(
            queue
                .claim(&kinds, "executing-reader")
                .await
                .expect("claim")
                .expect("job"),
            (id, original.clone())
        );
        let before = persisted_job(&pool, id).await;
        for _ in 0..2 {
            assert_eq!(
                queue
                    .release_deferred(id)
                    .await
                    .expect("release while reader executes"),
                ReleaseOutcome::NotDeferred
            );
            assert_eq!(
                persisted_job(&pool, id).await,
                before,
                "release must not revoke or modify an active claim"
            );
            assert!(
                queue
                    .claim(&kinds, "duplicate-reader")
                    .await
                    .expect("claim")
                    .is_none(),
                "the executing body must not be offered to another reader"
            );
        }
        let ack = queue
            .ack_delivery(id, DeliveryOrigin::ReaderRead)
            .await
            .expect("the original reader still owns its acknowledgement");
        assert_eq!(ack.recipient, SeatId::from("pij-active"));
        assert_eq!(ack.msg_id, "m-active");
        assert_eq!(
            queue
                .enqueue_delivery(original)
                .await
                .expect("duplicate after acknowledgement"),
            DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
        );
    }
}

#[tokio::test]
async fn release_deferred_distinguishes_absent_and_terminal_jobs() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
    for absent in [JobId(0), JobId(u64::MAX)] {
        assert_eq!(
            queue.release_deferred(absent).await.expect("absent row"),
            ReleaseOutcome::NotLive {
                reason: DeferNoopReason::Absent
            }
        );
    }
    for (msg_id, outcome) in [
        ("m-done", Outcome::Done),
        (
            "m-failed",
            Outcome::Failed {
                reason: "terminal failure".to_string(),
            },
        ),
    ] {
        let original = delivered_job("pij-release-terminal", msg_id);
        let id = queue.enqueue(original.clone()).await.expect("enqueue");
        queue
            .claim(std::slice::from_ref(&original.kind), "terminal-worker")
            .await
            .expect("claim")
            .expect("job");
        queue.ack(id, outcome).await.expect("finish terminal row");
        let before = persisted_job(&pool, id).await;
        assert_eq!(
            queue.release_deferred(id).await.expect("terminal row"),
            ReleaseOutcome::NotLive {
                reason: DeferNoopReason::Terminal
            }
        );
        assert_eq!(
            persisted_job(&pool, id).await,
            before,
            "release must not alter terminal history"
        );
    }
    assert_eq!(queue.live_len().await.expect("live count"), 0);
}

#[tokio::test]
async fn release_deferred_refuses_non_delivery_jobs_without_mutation() {
    for kind in ["pointer", "federation:remote", "delivery:wrong-recipient"] {
        let fresh = FreshStore::new();
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        let queue = SqliteQueue::new(pool.clone(), 300, 1_024).expect("valid queue policy");
        let id = queue
            .enqueue(job(kind, "pij-recipient", "m-refused-release"))
            .await
            .expect("enqueue");
        for running in [false, true] {
            if running {
                assert_eq!(
                    queue
                        .claim(&[kind.to_string()], "owner")
                        .await
                        .expect("claim")
                        .expect("job")
                        .0,
                    id
                );
            }
            let before = persisted_job(&pool, id).await;
            assert!(
                queue.release_deferred(id).await.is_err(),
                "release only accepts a correctly addressed delivery: {kind}"
            );
            assert_eq!(
                persisted_job(&pool, id).await,
                before,
                "refusal must not change the row"
            );
        }
    }
}

#[tokio::test]
async fn a_zero_claim_lease_is_refused_before_work_can_be_lost() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    assert!(SqliteQueue::new(pool.clone(), 0, 1_024).is_err());
    assert!(SqliteQueue::new(pool, 300, 0).is_err());
}

/// Review F5 — dedupe is per DESTINATION, not global.
///
/// The index was `dedupe_key` alone, and both delivery and federation rows carry
/// the CALLER's `msg_id`. So two sends with one msg_id to two different
/// recipients collapsed: the second insert hit `ON CONFLICT DO NOTHING`, the
/// read-back returned the first row, and the caller was told `Queued` for a
/// message that was never going anywhere.
///
/// Mutation witness: revert the index to `(dedupe_key)` and this fails with one
/// row where two are required.
#[tokio::test]
async fn one_msg_id_to_two_recipients_is_two_rows() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;

    let job = |seat: &str| Job {
        kind: format!("delivery:{seat}"),
        serial_key: seat.to_string(),
        payload: format!("to {seat}"),
        dedupe_key: "m-1".to_string(),
        attempt: 0,
    };

    let first = queue.enqueue(job("pij-x")).await.expect("first enqueue");
    let second = queue.enqueue(job("pij-y")).await.expect("second enqueue");
    assert_ne!(
        first, second,
        "the same msg_id addressed to a DIFFERENT seat is a different message"
    );

    // ...and the rule it must not break: the same id to the SAME seat still
    // collapses, which is what dedupe is for.
    let repeat = queue.enqueue(job("pij-x")).await.expect("repeat enqueue");
    assert_eq!(first, repeat, "N rapid submits to one seat are one row");

    let claimed = queue
        .claim(&["delivery:pij-y".to_string()], "worker")
        .await
        .expect("claim y")
        .expect("the second recipient's row must exist");
    assert_eq!(claimed.1.payload, "to pij-y");
}

/// Review F6 — the claim-lease sweep is SCOPED to the kinds being claimed.
///
/// Unscoped, the pointer drain's five-second tick decided the fate of federation
/// claims: a row held by a slow forward was terminally failed by a worker that
/// knows nothing about forwarding, underneath the worker still doing it.
#[tokio::test]
async fn the_lease_sweep_does_not_reach_another_workers_kind() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 1, 1_024).expect("one-second lease");

    let federation = queue
        .enqueue(Job {
            kind: "federation:laptop".to_string(), // the shape federation::remote_send_kind emits
            serial_key: "laptop".to_string(),
            payload: "forward".to_string(),
            dedupe_key: "m-fed".to_string(),
            attempt: 0,
        })
        .await
        .expect("enqueue federation");
    let held = queue
        .claim(&["federation:laptop".to_string()], "federation-worker")
        .await
        .expect("claim federation")
        .expect("a federation row");
    assert_eq!(held.0, federation);

    // AGE the claim past its lease deterministically, rather than sleeping: the
    // property under test is the sweep's SCOPE, and a test that sleeps to reach it
    // proves the sleep as well.
    sqlx::query("UPDATE jobs SET claimed_at = claimed_at - 3600 WHERE state = 'running'")
        .execute(&pool)
        .await
        .expect("age the claim");

    // A drain pass over DELIVERY kinds. It sweeps, and it must not touch the
    // federation claim it is holding nothing of.
    let _ = queue
        .claim(&["delivery:pij-x".to_string()], "drain-worker")
        .await
        .expect("drain claim");

    // The federation worker can still finish its own claim: if the sweep had
    // reached it, this is `job is not running`.
    queue
        .retry(federation, std::time::Duration::from_secs(1))
        .await
        .expect("the holder must still own its claim after another kind's sweep");
}

fn delivered_job(recipient: &str, msg_id: &str) -> Job {
    job(&format!("delivery:{recipient}"), recipient, msg_id)
}

async fn deliver(queue: &SqliteQueue, recipient: &str, msg_id: &str, origin: DeliveryOrigin) {
    let queued = queue
        .enqueue_delivery(delivered_job(recipient, msg_id))
        .await
        .expect("enqueue delivery");
    let DeliveryEnqueue::Queued { job_id: id, .. } = queued else {
        panic!("new message {msg_id} must queue");
    };
    let claimed = queue
        .claim(&[format!("delivery:{recipient}")], "delivery-worker")
        .await
        .expect("claim delivery")
        .expect("delivery row");
    assert_eq!(claimed.0, id);
    queue.ack_delivery(id, origin).await.expect("ack delivery");
}

#[tokio::test]
async fn delivered_id_and_origin_survive_queue_reconstruction() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 2).expect("queue");
    deliver(
        &queue,
        "pij-recipient",
        "m-durable",
        DeliveryOrigin::VerifiedArrival,
    )
    .await;

    let restarted = SqliteQueue::new(pool, 300, 2).expect("restarted queue");
    assert_eq!(
        restarted
            .enqueue_delivery(delivered_job("pij-recipient", "m-durable"))
            .await
            .expect("consult durable ledger"),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::VerifiedArrival)
    );
}

#[tokio::test]
async fn typed_to_pane_origin_survives_queue_reconstruction() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let queue = SqliteQueue::new(pool.clone(), 300, 2).expect("queue");
    deliver(
        &queue,
        "pij-typed-recipient",
        "m-typed",
        DeliveryOrigin::TypedToPane,
    )
    .await;

    let restarted = SqliteQueue::new(pool, 300, 2).expect("restarted queue");
    assert_eq!(
        restarted
            .enqueue_delivery(delivered_job("pij-typed-recipient", "m-typed"))
            .await
            .expect("consult durable ledger"),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::TypedToPane)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_delivery_ack_and_duplicate_enqueue_never_create_a_second_row() {
    let fresh = FreshStore::new();
    let queue = Arc::new(
        SqliteQueue::new(pij_store::open(&fresh.path()).await.expect("open"), 300, 16)
            .expect("queue"),
    );
    let first = queue
        .enqueue_delivery(delivered_job("pij-race", "m-race"))
        .await
        .expect("enqueue first");
    let DeliveryEnqueue::Queued { job_id: first, .. } = first else {
        panic!("first delivery must queue");
    };
    queue
        .claim(&["delivery:pij-race".to_string()], "worker")
        .await
        .expect("claim")
        .expect("running delivery");

    let barrier = Arc::new(Barrier::new(3));
    let ack = {
        let queue = Arc::clone(&queue);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            queue.ack_delivery(first, DeliveryOrigin::ReaderRead).await
        })
    };
    let duplicate = {
        let queue = Arc::clone(&queue);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            queue
                .enqueue_delivery(delivered_job("pij-race", "m-race"))
                .await
        })
    };
    barrier.wait().await;
    ack.await.expect("ack task").expect("ack");
    let duplicate = duplicate
        .await
        .expect("duplicate task")
        .expect("duplicate result");
    assert!(
        matches!(duplicate, DeliveryEnqueue::Queued { job_id, .. } if job_id == first)
            || duplicate == DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead),
        "the race may observe the live row or its delivered record, never a fresh row: {duplicate:?}"
    );
    assert_eq!(queue.live_len().await.expect("live count"), 0);
}

#[tokio::test]
async fn per_recipient_bound_evicts_oldest_without_weakening_other_recipients() {
    let fresh = FreshStore::new();
    let queue = SqliteQueue::new(pij_store::open(&fresh.path()).await.expect("open"), 300, 2)
        .expect("queue");
    deliver(&queue, "pij-a", "a-1", DeliveryOrigin::ReaderRead).await;
    deliver(&queue, "pij-b", "b-1", DeliveryOrigin::InjectedToTransport).await;
    deliver(&queue, "pij-a", "a-2", DeliveryOrigin::ReaderRead).await;
    deliver(&queue, "pij-a", "a-3", DeliveryOrigin::ReaderRead).await;

    assert!(matches!(
        queue
            .enqueue_delivery(delivered_job("pij-a", "a-1"))
            .await
            .expect("oldest id was forgotten"),
        DeliveryEnqueue::Queued { .. }
    ));
    assert_eq!(
        queue
            .enqueue_delivery(delivered_job("pij-a", "a-2"))
            .await
            .expect("newer a id remains"),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
    );
    assert_eq!(
        queue
            .enqueue_delivery(delivered_job("pij-b", "b-1"))
            .await
            .expect("hot recipient must not evict another recipient"),
        DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::InjectedToTransport)
    );
}

/// Review F12 — `note_delivered` is pinned against the REAL adapter, not only
/// the fake.
///
/// The claim and compensation witnesses both run against `FakeQueue`, and the
/// port's atomicity claim ("in ONE transaction... because a separate read and
/// write reintroduce exactly the check-then-act race") was asserted by nobody
/// against SQLite. That is round-1 F6's shape one layer down: the fake got the
/// semantics, the real adapter got the semantics, and no instrument compared
/// them.
#[tokio::test]
async fn note_delivered_claims_once_and_survives_reconstruction() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;
    let recipient = SeatId::from("pij-real");

    assert_eq!(
        queue
            .note_delivered(&recipient, "m-1", DeliveryOrigin::ReaderRead)
            .await
            .expect("first claim"),
        None,
        "an unclaimed message is ours to deliver"
    );
    assert_eq!(
        queue
            .note_delivered(&recipient, "m-1", DeliveryOrigin::InjectedToTransport)
            .await
            .expect("second claim"),
        Some(DeliveryOrigin::ReaderRead),
        "a claimed message reports the ORIGINAL observation, not the new caller's guess"
    );

    // Same store, new queue object: the ledger is durable, not process state.
    drop(queue);
    let reopened = crate_queue(&fresh).await;
    assert_eq!(
        reopened
            .note_delivered(&recipient, "m-1", DeliveryOrigin::VerifiedArrival)
            .await
            .expect("claim after reconstruction"),
        Some(DeliveryOrigin::ReaderRead)
    );

    // A different recipient with the same id is a different message.
    assert_eq!(
        reopened
            .note_delivered(
                &SeatId::from("pij-other"),
                "m-1",
                DeliveryOrigin::ReaderRead
            )
            .await
            .expect("other recipient"),
        None
    );
}

/// The compensation half, against SQLite: a released claim must be re-claimable,
/// or a failed injection would suppress the retry of a message nobody received.
#[tokio::test]
async fn forget_delivered_releases_a_claim_on_the_real_adapter() {
    let fresh = FreshStore::new();
    let queue = crate_queue(&fresh).await;
    let recipient = SeatId::from("pij-real");

    queue
        .note_delivered(&recipient, "m-2", DeliveryOrigin::InjectedToTransport)
        .await
        .expect("claim");
    queue
        .forget_delivered(&recipient, "m-2")
        .await
        .expect("release");

    assert_eq!(
        queue
            .note_delivered(&recipient, "m-2", DeliveryOrigin::InjectedToTransport)
            .await
            .expect("re-claim"),
        None,
        "a released claim is claimable again: the message was never delivered"
    );
}
