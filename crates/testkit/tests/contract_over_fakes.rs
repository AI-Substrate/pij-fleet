//! Fake-queue behaviour pinned against the STORE's granularity.
//!
//! The deferral deadlines below are whole SECONDS, not milliseconds, and that is
//! not cosmetic: SQLite eligibility compares `unixepoch()`, so the store can only
//! express a second. These tests were written in 10 ms and 30 ms delays and passed
//! only because the fake kept full `Duration` precision — i.e. the fake released
//! EARLIER than production could, which makes a fake more permissive than the
//! thing it stands in for and every green test built on it a false negative
//! (reviewer f3, 2026-09-05). Operators inherit the same fact: PIJ_TYPING_GRACE_MS
//! is parsed in milliseconds and the store rounds its deadline up to a second.
//! The port-contract exemplar (tk-ddf1) — the tier-2 shape later units copy.
//!
//! One suite per seam, run here against the shipped fake. When the SQLite
//! `Registry` lands at tk-d4c3 it runs the SAME function; nothing in
//! `pij_testkit::contract` knows which implementation it is judging. That is what
//! stops "green with the fake" and "green for real" from drifting apart, and it
//! is why the contract ships in wave 0 rather than with the first adapter.

use std::time::Duration;

use pij_core::model::{
    DeliveryOrigin, DeliveryOutcome, Harness, Job, JobId, Liveness, Msg, Outcome, ProcIdentity,
    Readiness, SeatId,
};
use pij_core::ports::{
    DeferNoopReason, DeferOutcome, DeliveryEnqueue, HarnessPort, LivenessPort, Queue,
    ReleaseOutcome, TmuxPort, Transport,
};
use pij_testkit::block_on;
use pij_testkit::contract::{queue_contract, registry_contract, sample_seat, spine_contract};
use pij_testkit::fakes::{
    FakeHarness, FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux, FakeTransport,
};

#[test]
fn fake_registry_honours_the_registry_contract() {
    let registry = FakeRegistry::new();
    block_on(registry_contract(&registry));
}

#[test]
fn fake_spine_honours_the_spine_contract() {
    let spine = FakeSpine::new();
    block_on(spine_contract(&spine));
}

#[test]
fn fake_queue_honours_the_queue_contract() {
    let queue = FakeQueue::new(1_024).expect("valid fake queue policy");
    block_on(queue_contract(&queue));
}

#[test]
fn fake_extension_heartbeat_and_working_brake_preserve_the_silent_lease_budget() {
    block_on(async {
        let queue = FakeQueue::new(1_024).unwrap();
        let spine = FakeSpine::new();
        let recipient = SeatId::from("pij-heartbeat");
        let body = Job {
            kind: pij_core::delivery::delivery_kind(&recipient),
            serial_key: recipient.to_string(),
            payload: serde_json::json!({
                "from":"pij-sender","to":recipient,"body":"held body","msg_id":"held-body"
            })
            .to_string(),
            dedupe_key: "held-body".into(),
            attempt: 0,
        };
        let kinds = [body.kind.clone()];
        let id = queue.enqueue(body).await.unwrap();
        let mut lease = pij_core::ports::ExtensionLease {
            seconds: 60,
            renew_working: false,
        };
        queue
            .claim_extension(&kinds, recipient.as_str(), lease, 0, &spine, true)
            .await
            .unwrap();
        queue.advance(Duration::from_secs(40));
        assert!(queue.heartbeat_delivery(id, &recipient, 0).await.unwrap());
        queue.advance(Duration::from_secs(40));
        let held = queue
            .claim_extension(&kinds, recipient.as_str(), lease, 0, &spine, true)
            .await
            .unwrap();
        assert!(
            held.claimed.is_none(),
            "heartbeat moved the expiration deadline"
        );
        lease.renew_working = true;
        for _ in 0..4 {
            queue.advance(Duration::from_secs(60));
            let held = queue
                .claim_extension(&kinds, recipient.as_str(), lease, 0, &spine, true)
                .await
                .unwrap();
            assert!(held.claimed.is_none());
            assert!(held.parked.is_empty());
            assert!(held.events.is_empty());
        }
        assert_eq!(queue.attempts(id), 0);
        assert!(queue.acked().is_empty());
        lease.renew_working = false;
        for expiration in 1..=3 {
            queue.advance(Duration::from_secs(60));
            let page = queue
                .claim_extension(&kinds, recipient.as_str(), lease, 0, &spine, true)
                .await
                .unwrap();
            if expiration < 3 {
                let (claimed_id, claimed) = page.claimed.unwrap();
                assert_eq!((claimed_id, claimed.attempt), (id, expiration));
                assert!(page.parked.is_empty());
            } else {
                assert!(page.claimed.is_none());
                assert_eq!(page.parked[0].job_id, id);
                assert_eq!(
                    page.parked[0].outcome,
                    pij_core::model::DeliveryFailure::LeaseExhausted
                );
                assert_eq!(
                    page.events
                        .iter()
                        .filter(|event| event.kind == "delivery.parked")
                        .count(),
                    1
                );
            }
        }
    });
}

#[test]
fn fake_queue_defer_running_preserves_identity_body_attempt_and_delay() {
    block_on(async {
        let queue = FakeQueue::new(1_024).expect("valid queue policy");
        let original = Job {
            kind: "delivery:pij-recipient".to_string(),
            serial_key: "pij-recipient".to_string(),
            payload: r#"{"recipient":"not-authority","msg_id":"not-authority","body":"keep me"}"#
                .to_string(),
            dedupe_key: "stored-message".to_string(),
            attempt: 0,
        };
        assert_eq!(
            queue
                .note_delivered(
                    &SeatId::from("pij-recipient"),
                    "prior-message",
                    DeliveryOrigin::ReaderRead,
                )
                .await
                .expect("seed prior delivery evidence"),
            None
        );
        let DeliveryEnqueue::Queued { job_id, .. } = queue
            .enqueue_delivery(original.clone())
            .await
            .expect("enqueue delivery")
        else {
            panic!("new delivery must queue");
        };
        let kinds = std::slice::from_ref(&original.kind);
        assert_eq!(
            queue.claim(kinds, "old-worker").await.expect("claim"),
            Some((job_id, original.clone()))
        );
        queue.retry(job_id, Duration::ZERO).await.expect("retry");
        let expected = Job {
            attempt: 1,
            ..original.clone()
        };
        assert_eq!(
            queue.claim(kinds, "old-worker").await.expect("reclaim"),
            Some((job_id, expected.clone()))
        );
        queue.advance(Duration::from_secs(50));

        assert_eq!(
            queue
                .defer(job_id, Duration::from_secs(10))
                .await
                .expect("defer running delivery"),
            DeferOutcome::Deferred {
                recipient: SeatId::from("pij-recipient"),
                msg_id: "stored-message".to_string(),
            }
        );
        queue
            .ack(job_id, Outcome::Done)
            .await
            .expect_err("defer cleared running ownership");
        queue
            .ack_delivery(job_id, DeliveryOrigin::ReaderRead)
            .await
            .expect_err("the old delivery claim is no longer authoritative");
        queue
            .retry(job_id, Duration::ZERO)
            .await
            .expect_err("defer does not leave a retryable claim");
        assert_eq!(queue.attempts(job_id), 1);
        assert_eq!(queue.retried(), vec![(job_id, Duration::ZERO)]);
        assert!(queue.acked().is_empty());
        assert_eq!(queue.live_len(), 1);
        assert_eq!(
            queue.peek(kinds).await.expect("peek delayed row"),
            Some((job_id, expected.clone()))
        );
        assert_eq!(
            queue
                .enqueue_delivery(Job {
                    payload: "duplicate must not replace the body".to_string(),
                    ..original.clone()
                })
                .await
                .expect("dedupe deferred delivery without recording delivery"),
            DeliveryEnqueue::Queued {
                job_id,
                not_before_ms: 60_000,
            }
        );
        assert_eq!(
            queue
                .enqueue_delivery(Job {
                    dedupe_key: "prior-message".to_string(),
                    ..original.clone()
                })
                .await
                .expect("prior evidence survives live deferral"),
            DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
        );
        queue.advance(Duration::from_secs(9));
        assert_eq!(queue.claim(kinds, "new-worker").await.expect("claim"), None);
        queue.advance(Duration::from_secs(1));
        assert_eq!(
            queue
                .claim(kinds, "new-worker")
                .await
                .expect("claim due row"),
            Some((job_id, expected))
        );
    });
}

#[test]
fn fake_queue_defer_pending_replaces_the_schedule_from_the_current_clock() {
    block_on(async {
        let queue = FakeQueue::new(1_024).expect("valid queue policy");
        let original = Job {
            kind: "delivery:pij-pending".to_string(),
            serial_key: "pij-pending".to_string(),
            payload: "pending body".to_string(),
            dedupe_key: "pending-message".to_string(),
            attempt: 0,
        };
        let job_id = queue.enqueue(original.clone()).await.expect("enqueue");
        let expected_outcome = DeferOutcome::Deferred {
            recipient: SeatId::from("pij-pending"),
            msg_id: "pending-message".to_string(),
        };
        assert_eq!(
            queue
                .defer(job_id, Duration::from_secs(30))
                .await
                .expect("defer pending row"),
            expected_outcome
        );
        queue.advance(Duration::from_secs(5));
        assert_eq!(
            queue
                .defer(job_id, Duration::from_secs(10))
                .await
                .expect("replace pending delay"),
            expected_outcome
        );
        assert_eq!(
            queue
                .enqueue_delivery(original.clone())
                .await
                .expect("observe persisted deadline"),
            DeliveryEnqueue::Queued {
                job_id,
                not_before_ms: 15_000,
            }
        );
        let kinds = std::slice::from_ref(&original.kind);
        assert_eq!(
            queue.peek(kinds).await.expect("peek delayed pending row"),
            Some((job_id, original.clone()))
        );
        assert_eq!(queue.attempts(job_id), 0);
        assert!(queue.retried().is_empty());
        assert_eq!(queue.live_len(), 1);
        queue.advance(Duration::from_secs(9));
        assert_eq!(queue.claim(kinds, "worker").await.expect("claim"), None);
        queue.advance(Duration::from_secs(1));
        assert_eq!(
            queue
                .claim(kinds, "worker")
                .await
                .expect("claim at deadline"),
            Some((job_id, original.clone()))
        );
    });
}

#[test]
fn fake_queue_defer_zero_releases_pending_and_running_without_reordering_fifo() {
    block_on(async {
        for running in [false, true] {
            let queue = FakeQueue::new(1_024).expect("valid queue policy");
            let first = Job {
                kind: "delivery:pij-fifo".to_string(),
                serial_key: "pij-fifo".to_string(),
                payload: "first body".to_string(),
                dedupe_key: "first-message".to_string(),
                attempt: 0,
            };
            let second = Job {
                payload: "second body".to_string(),
                dedupe_key: "second-message".to_string(),
                ..first.clone()
            };
            let first_id = queue.enqueue(first.clone()).await.expect("enqueue first");
            let kinds = std::slice::from_ref(&first.kind);
            if running {
                assert_eq!(
                    queue.claim(kinds, "old-worker").await.expect("claim first"),
                    Some((first_id, first.clone()))
                );
            } else {
                queue
                    .defer(first_id, Duration::from_secs(30))
                    .await
                    .expect("delay pending first row");
            }
            let second_id = queue.enqueue(second.clone()).await.expect("enqueue second");
            queue.advance(Duration::from_secs(3));
            assert_eq!(
                queue
                    .defer(first_id, Duration::ZERO)
                    .await
                    .expect("release now"),
                DeferOutcome::Deferred {
                    recipient: SeatId::from("pij-fifo"),
                    msg_id: "first-message".to_string(),
                }
            );
            assert_eq!(
                queue
                    .enqueue_delivery(first.clone())
                    .await
                    .expect("observe immediate schedule"),
                DeliveryEnqueue::Queued {
                    job_id: first_id,
                    not_before_ms: 3_000,
                }
            );
            assert_eq!(
                queue
                    .claim(kinds, "new-worker")
                    .await
                    .expect("claim released first"),
                Some((first_id, first.clone())),
                "deferring does not move the oldest row behind a newer delivery"
            );
            queue.ack(first_id, Outcome::Done).await.expect("ack first");
            assert_eq!(
                queue
                    .claim(kinds, "new-worker")
                    .await
                    .expect("claim second"),
                Some((second_id, second))
            );
            assert_eq!(queue.attempts(first_id), 0);
            assert!(queue.retried().is_empty());
        }
    });
}

#[test]
fn fake_queue_defer_absent_or_terminal_is_not_live_and_preserves_delivery_evidence() {
    block_on(async {
        let original = Job {
            kind: "delivery:pij-terminal".to_string(),
            serial_key: "pij-terminal".to_string(),
            payload: "terminal body".to_string(),
            dedupe_key: "terminal-message".to_string(),
            attempt: 0,
        };
        let kinds = std::slice::from_ref(&original.kind);
        for outcome in [
            Outcome::Done,
            Outcome::Failed {
                reason: "failed".to_string(),
            },
        ] {
            let queue = FakeQueue::new(1_024).expect("valid queue policy");
            assert_eq!(
                queue
                    .defer(JobId(u64::MAX), Duration::ZERO)
                    .await
                    .expect("absent"),
                DeferOutcome::NotLive {
                    reason: DeferNoopReason::Absent,
                }
            );
            let job_id = queue.enqueue(original.clone()).await.expect("enqueue");
            queue
                .claim(kinds, "worker")
                .await
                .expect("claim")
                .expect("row");
            queue
                .ack(job_id, outcome.clone())
                .await
                .expect("terminal ack");
            assert_eq!(
                queue.defer(job_id, Duration::ZERO).await.expect("terminal"),
                DeferOutcome::NotLive {
                    reason: DeferNoopReason::Terminal,
                }
            );
            assert_eq!(queue.acked(), vec![(job_id, outcome)]);
            assert_eq!(queue.live_len(), 0);
            assert_eq!(queue.peek(kinds).await.expect("peek terminal"), None);
            assert_eq!(
                queue.claim(kinds, "worker").await.expect("claim terminal"),
                None
            );
        }

        let queue = FakeQueue::new(1).expect("one retained delivery id");
        let job_id = queue.enqueue(original.clone()).await.expect("enqueue");
        queue
            .claim(kinds, "worker")
            .await
            .expect("claim")
            .expect("row");
        queue
            .ack_delivery(job_id, DeliveryOrigin::ReaderRead)
            .await
            .expect("record delivered evidence");
        for (id, reason) in [
            (job_id, DeferNoopReason::Terminal),
            (JobId(u64::MAX), DeferNoopReason::Absent),
        ] {
            assert_eq!(
                queue.defer(id, Duration::ZERO).await.expect("not live"),
                DeferOutcome::NotLive { reason }
            );
        }
        assert_eq!(
            queue
                .enqueue_delivery(original)
                .await
                .expect("replay delivery evidence"),
            DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
        );
    });
}

#[test]
fn fake_queue_defer_refuses_non_delivery_without_mutating_pending_or_running_rows() {
    block_on(async {
        for kind in ["maintenance", "delivery:wrong-recipient"] {
            for running in [false, true] {
                let queue = FakeQueue::new(1_024).expect("valid queue policy");
                let original = Job {
                    kind: kind.to_string(),
                    serial_key: "pij-actual-recipient".to_string(),
                    payload: "must survive rejection".to_string(),
                    dedupe_key: "non-delivery-message".to_string(),
                    attempt: 0,
                };
                let job_id = queue.enqueue(original.clone()).await.expect("enqueue");
                let kinds = std::slice::from_ref(&original.kind);
                if running {
                    queue
                        .claim(kinds, "worker")
                        .await
                        .expect("claim")
                        .expect("row");
                }
                queue
                    .defer(job_id, Duration::from_secs(60))
                    .await
                    .expect_err("only matching delivery kinds can be deferred");
                assert_eq!(
                    queue.peek(kinds).await.expect("peek untouched row"),
                    Some((job_id, original.clone()))
                );
                if !running {
                    assert_eq!(
                        queue
                            .claim(kinds, "worker")
                            .await
                            .expect("claim still due row"),
                        Some((job_id, original.clone()))
                    );
                }
                queue
                    .ack(job_id, Outcome::Done)
                    .await
                    .expect("rejection retained running ownership");
                assert_eq!(queue.attempts(job_id), 0);
                assert!(queue.retried().is_empty());
                assert_eq!(queue.live_len(), 0);
            }
        }
    });
}

#[test]
fn fake_queue_defer_overflow_refuses_without_poisoning_or_mutating() {
    block_on(async {
        let queue = FakeQueue::new(1_024).expect("queue");
        let original = Job {
            kind: "delivery:pij-overflow".to_string(),
            serial_key: "pij-overflow".to_string(),
            payload: "preserved".to_string(),
            dedupe_key: "overflow".to_string(),
            attempt: 0,
        };
        let id = queue.enqueue(original.clone()).await.expect("enqueue");
        let kinds = std::slice::from_ref(&original.kind);
        queue.claim(kinds, "reader").await.expect("claim");
        queue.advance(Duration::from_nanos(1));
        queue
            .defer(id, Duration::MAX)
            .await
            .expect_err("overflow is an error, never a panic");
        assert_eq!(
            queue.peek(kinds).await.expect("queue remains usable"),
            Some((id, original))
        );
        queue
            .ack(id, Outcome::Done)
            .await
            .expect("claim ownership preserved");
    });
}

#[test]
fn fake_queue_release_deferred_preserves_body_attempt_ledger_and_active_reader() {
    block_on(async {
        let queue = FakeQueue::new(1_024).expect("queue");
        let original = Job {
            kind: "delivery:pij-release".to_string(),
            serial_key: "pij-release".to_string(),
            payload: r#"{"recipient":"not-authority","msg_id":"not-authority"}"#.to_string(),
            dedupe_key: "release-message".to_string(),
            attempt: 0,
        };
        let recipient = SeatId::from("pij-release");
        queue
            .note_delivered(&recipient, "prior-message", DeliveryOrigin::ReaderRead)
            .await
            .expect("seed prior delivery evidence");
        let id = queue.enqueue(original.clone()).await.expect("enqueue");
        let kinds = std::slice::from_ref(&original.kind);
        queue
            .claim(kinds, "old-worker")
            .await
            .expect("claim")
            .expect("row");
        queue
            .retry(id, Duration::ZERO)
            .await
            .expect("increment retry attempt");
        let expected = Job {
            attempt: 1,
            ..original.clone()
        };
        queue
            .defer(id, Duration::from_secs(30))
            .await
            .expect("hold pending");
        queue.advance(Duration::from_secs(5));
        assert_eq!(
            queue.claim(kinds, "reader").await.expect("held claim"),
            None
        );
        for _ in 0..2 {
            assert_eq!(
                queue.release_deferred(id).await.expect("release pending"),
                ReleaseOutcome::Released {
                    recipient: recipient.clone(),
                    msg_id: original.dedupe_key.clone(),
                }
            );
        }
        assert_eq!(
            queue
                .enqueue_delivery(original.clone())
                .await
                .expect("same pending row"),
            DeliveryEnqueue::Queued {
                job_id: id,
                not_before_ms: 5_000
            }
        );
        assert_eq!(
            queue.peek(kinds).await.expect("peek"),
            Some((id, expected.clone()))
        );
        assert_eq!(
            queue
                .claim(kinds, "reader")
                .await
                .expect("claim released row"),
            Some((id, expected.clone()))
        );
        let next = queue
            .enqueue(Job {
                dedupe_key: "next-message".to_string(),
                ..original.clone()
            })
            .await
            .expect("enqueue another delivery");
        for _ in 0..2 {
            assert_eq!(
                queue
                    .release_deferred(id)
                    .await
                    .expect("duplicate release after claim"),
                ReleaseOutcome::NotDeferred
            );
            assert_eq!(
                queue.peek(kinds).await.expect("reader row intact"),
                Some((id, expected.clone()))
            );
            assert_eq!(
                queue
                    .claim(kinds, "competitor")
                    .await
                    .expect("serial ownership retained"),
                None
            );
        }
        assert_eq!(queue.attempts(id), 1);
        assert_eq!(queue.retried(), vec![(id, Duration::ZERO)]);
        let ack = queue
            .ack_delivery(id, DeliveryOrigin::ReaderRead)
            .await
            .expect("reader still owns ack");
        assert_eq!(ack.recipient, recipient);
        assert_eq!(ack.msg_id, original.dedupe_key);
        assert_eq!(
            queue.release_deferred(id).await.expect("terminal release"),
            ReleaseOutcome::NotLive {
                reason: DeferNoopReason::Terminal
            }
        );
        assert_eq!(
            queue
                .enqueue_delivery(original.clone())
                .await
                .expect("recorded evidence"),
            DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
        );
        assert_eq!(
            queue
                .enqueue_delivery(Job {
                    dedupe_key: "prior-message".to_string(),
                    ..original.clone()
                })
                .await
                .expect("prior evidence untouched"),
            DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
        );
        assert_eq!(
            queue
                .claim(kinds, "competitor")
                .await
                .expect("claim next after ack")
                .expect("next row")
                .0,
            next
        );
    });
}

#[test]
fn fake_queue_release_deferred_distinguishes_absent_and_terminal_without_resurrection() {
    block_on(async {
        for outcome in [
            Outcome::Done,
            Outcome::Failed {
                reason: "failed".to_string(),
            },
        ] {
            let queue = FakeQueue::new(1_024).expect("queue");
            assert_eq!(
                queue
                    .release_deferred(JobId(u64::MAX))
                    .await
                    .expect("absent release"),
                ReleaseOutcome::NotLive {
                    reason: DeferNoopReason::Absent
                }
            );
            let original = Job {
                kind: "delivery:pij-terminal".to_string(),
                serial_key: "pij-terminal".to_string(),
                payload: "terminal body".to_string(),
                dedupe_key: "terminal-message".to_string(),
                attempt: 0,
            };
            let id = queue.enqueue(original.clone()).await.expect("enqueue");
            let kinds = std::slice::from_ref(&original.kind);
            queue
                .claim(kinds, "worker")
                .await
                .expect("claim")
                .expect("row");
            queue.ack(id, outcome.clone()).await.expect("finish");
            assert_eq!(
                queue.release_deferred(id).await.expect("terminal release"),
                ReleaseOutcome::NotLive {
                    reason: DeferNoopReason::Terminal
                }
            );
            assert_eq!(queue.acked(), vec![(id, outcome)]);
            assert_eq!(queue.peek(kinds).await.expect("no resurrection"), None);
            assert_eq!(queue.claim(kinds, "worker").await.expect("no claim"), None);
            assert_eq!(queue.live_len(), 0);
        }
    });
}

#[test]
fn fake_queue_release_deferred_refuses_non_delivery_without_mutation() {
    block_on(async {
        for kind in ["maintenance", "delivery:wrong-recipient"] {
            for running in [false, true] {
                let queue = FakeQueue::new(1_024).expect("queue");
                let original = Job {
                    kind: kind.to_string(),
                    serial_key: "pij-actual-recipient".to_string(),
                    payload: "preserve non-delivery".to_string(),
                    dedupe_key: "non-delivery-message".to_string(),
                    attempt: 0,
                };
                let id = queue.enqueue(original.clone()).await.expect("enqueue");
                let kinds = std::slice::from_ref(&original.kind);
                queue
                    .claim(kinds, "worker")
                    .await
                    .expect("claim")
                    .expect("row");
                queue
                    .retry(id, Duration::from_secs(60))
                    .await
                    .expect("delay non-delivery");
                let expected = Job {
                    attempt: 1,
                    ..original.clone()
                };
                if running {
                    queue.advance(Duration::from_secs(60));
                    queue
                        .claim(kinds, "worker")
                        .await
                        .expect("reclaim")
                        .expect("row");
                }
                queue
                    .release_deferred(id)
                    .await
                    .expect_err("refuse non-delivery");
                assert_eq!(
                    queue.peek(kinds).await.expect("row unchanged"),
                    Some((id, expected.clone()))
                );
                assert_eq!(queue.attempts(id), 1);
                assert_eq!(queue.retried(), vec![(id, Duration::from_secs(60))]);
                if !running {
                    assert_eq!(
                        queue.claim(kinds, "worker").await.expect("delay unchanged"),
                        None
                    );
                    queue.advance(Duration::from_secs(60));
                    assert_eq!(
                        queue.claim(kinds, "worker").await.expect("claim when due"),
                        Some((id, expected))
                    );
                }
                queue
                    .ack(id, Outcome::Done)
                    .await
                    .expect("ownership intact");
                assert_eq!(queue.live_len(), 0);
            }
        }
    });
}

#[test]
fn fake_queue_enforces_its_per_recipient_delivered_id_bound() {
    block_on(async {
        let queue = FakeQueue::new(1).expect("one delivered id per recipient");
        let job = |recipient: &str, msg_id: &str| Job {
            kind: format!("delivery:{recipient}"),
            serial_key: recipient.to_string(),
            payload: "{}".to_string(),
            dedupe_key: msg_id.to_string(),
            attempt: 0,
        };
        for msg_id in ["first", "second"] {
            let DeliveryEnqueue::Queued { job_id: id, .. } = queue
                .enqueue_delivery(job("pij-a", msg_id))
                .await
                .expect("enqueue")
            else {
                panic!("new id must queue");
            };
            let claimed = queue
                .claim(&["delivery:pij-a".to_string()], "worker")
                .await
                .expect("claim")
                .expect("delivery row");
            assert_eq!(claimed.0, id);
            queue
                .ack_delivery(id, DeliveryOrigin::ReaderRead)
                .await
                .expect("ack delivery");
        }

        assert!(matches!(
            queue
                .enqueue_delivery(job("pij-a", "first"))
                .await
                .expect("oldest id evicted"),
            DeliveryEnqueue::Queued { .. }
        ));
        assert_eq!(
            queue
                .enqueue_delivery(job("pij-a", "second"))
                .await
                .expect("newest id retained"),
            DeliveryEnqueue::AlreadyDelivered(DeliveryOrigin::ReaderRead)
        );
    });
}
#[test]
fn fakes_record_what_the_code_did_not_only_what_it_returned() {
    // The recorder is the reason these are fakes and not stubs: a test can
    // assert the SEQUENCE of interactions, which is where delivery bugs live.
    let registry = FakeRegistry::new();
    block_on(registry_contract(&registry));

    let calls = registry.calls();
    assert!(
        calls.first().is_some_and(|c| c.starts_with("get:")),
        "the contract's first act is a read of an absent seat: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("tombstone:")),
        "the contract must have tombstoned a seat: {calls:?}"
    );
}

#[test]
fn an_unreachable_transport_queues_rather_than_claiming_delivery() {
    // TS defect #1: a message sent before the seat bound reported success and
    // vanished. The honest answer is Queued, and the message must NOT appear in
    // `delivered()`.
    let transport = FakeTransport::unreachable();
    let seat = sample_seat("pij-prebind");
    let msg = Msg {
        from: "pij-sender".into(),
        to: seat.id.clone(),
        body: "before you bound".to_string(),
        msg_id: "m-1".to_string(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };

    let outcome = block_on(transport.deliver(&seat, &msg)).expect("deliver");

    assert_eq!(
        outcome,
        DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        }
    );
    assert!(
        transport.delivered().is_empty(),
        "a queued message must not be counted as delivered"
    );
    assert_eq!(
        transport.calls(),
        vec!["deliver:pij-prebind:m-1".to_string()]
    );
}

#[test]
fn a_reachable_transport_delivers_and_records_the_message_id() {
    let transport = FakeTransport::reachable();
    let seat = sample_seat("pij-bound");
    let msg = Msg {
        from: "pij-sender".into(),
        to: seat.id.clone(),
        body: "hello".to_string(),
        msg_id: "m-2".to_string(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };

    assert!(block_on(transport.can_deliver(&seat, &msg)).expect("can_deliver"));
    assert_eq!(
        block_on(transport.deliver(&seat, &msg)).expect("deliver"),
        DeliveryOutcome::Delivered {
            origin: DeliveryOrigin::InjectedToTransport
        }
    );
    assert_eq!(transport.delivered(), vec![msg]);
}

#[test]
fn the_tmux_fake_scripts_a_pane_that_changes_between_polls() {
    // The readiness-anchor class: a pane whose footer rotates through
    // placeholders before the real anchor appears. A fake that returns one fixed
    // capture cannot express it, so this one queues them.
    let tmux = FakeTmux::new()
        .script_capture("thinking…")
        .script_capture("pij-rs ready");

    assert_eq!(
        block_on(tmux.capture("%1", 5)).expect("capture"),
        "thinking…"
    );
    assert_eq!(
        block_on(tmux.capture("%1", 5)).expect("capture"),
        "pij-rs ready"
    );
    // Unscripted: an empty pane, not a panic. A double that explodes on an
    // unexpected call makes the test about the double.
    assert_eq!(block_on(tmux.capture("%1", 5)).expect("capture"), "");
}

#[test]
fn the_typing_gate_is_consulted_and_answers_honestly() {
    let tmux = FakeTmux::new().with_user_typing();
    assert!(block_on(tmux.user_typing("%1")).expect("user_typing"));
    assert!(
        !block_on(FakeTmux::new().user_typing("%1")).expect("user_typing"),
        "an unscripted tmux reports nobody typing — the gate must not block by default"
    );
}

#[test]
fn an_unscripted_readiness_says_what_it_observed() {
    let harness = FakeHarness::new(Harness::Codex);
    assert_eq!(harness.kind(), Harness::Codex);

    match block_on(harness.readiness("%1")).expect("readiness") {
        Readiness::NotYet { observed } => assert!(
            !observed.is_empty(),
            "NotYet must carry what WAS seen — a bare timeout is undiagnosable"
        ),
        other => panic!("an unscripted harness cannot be ready: {other:?}"),
    }
}

#[test]
fn the_harness_busy_gate_is_scriptable_and_defaults_to_not_busy() {
    // `busy(pane)` is part of the SETTLED HarnessPort (services.dd.md:45) and was
    // missing from the first draft of ports.rs — caught in review. It is the
    // BUSY_RE-class seam: one place per harness where "is it mid-turn" is decided.
    let harness = FakeHarness::new(Harness::Pi)
        .script_busy(true)
        .script_busy(false);

    assert!(block_on(harness.busy("%1")).expect("busy"));
    assert!(!block_on(harness.busy("%1")).expect("busy"));
    assert!(
        !block_on(FakeHarness::new(Harness::Pi).busy("%1")).expect("busy"),
        "unscripted means NOT busy — a fake that claims mid-turn by default makes \
         every delivery test arrange its way out of a state it never asked for"
    );
}

#[test]
fn liveness_tells_recycled_from_active_and_dead() {
    // The verdict that TS got wrong in both directions. Pure core logic over one
    // observed fact, so all three branches are one line each to arrange.
    let bound = ProcIdentity {
        pid: 4242,
        proc_start: 1_000,
    };

    let active = FakeLiveness::new().with_proc(bound);
    assert_eq!(
        block_on(pij_core::liveness::alive(bound, &active)).expect("alive"),
        Liveness::Active
    );

    let gone = FakeLiveness::new();
    assert!(matches!(
        block_on(pij_core::liveness::alive(bound, &gone)).expect("alive"),
        Liveness::Dead { .. }
    ));

    let recycled = FakeLiveness::new().with_recycled(4242, 9_999);
    assert_eq!(
        block_on(pij_core::liveness::alive(bound, &recycled)).expect("alive"),
        Liveness::Recycled {
            observed_start: 9_999,
            recorded_start: 1_000,
        },
        "a different process at the same pid is NEITHER alive nor dead — reporting \
         Active addresses a stranger, reporting Dead hides the reuse"
    );

    // The port itself stays narrow: one fact, no verdict.
    assert_eq!(
        block_on(active.proc_start(4242)).expect("proc_start"),
        Some(1_000)
    );
}
