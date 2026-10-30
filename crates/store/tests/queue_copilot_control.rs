//! Plan 186: a Copilot seat's typed controls queue on `delivery-control:<seat>`,
//! apart from the bodies its native extension claims on `delivery:<seat>`.
//!
//! Both kinds share the seat as serial key, so the queue's one-running-claim
//! rule is the overlap guard: a typed control never runs while a native body is
//! claimed, and the reverse. Asserted against SQLite, which owns the rule, and
//! against the fake that ships as the default wiring.

use pij_core::config::Config;
use pij_core::delivery::delivery_kind;
use pij_core::model::DeliveryFailure;
use pij_core::model::{DeliveryOrigin, Job, Msg, Outcome, SeatId};
use pij_core::ports::{DeliveryEnqueue, ParkingEvidence, Queue};
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeQueue, FakeSpine};

const SEAT: &str = "pij-copilot";

/// The wire kind, spelled out so this test pins it rather than echoing the
/// producer's own function back at itself.
fn control_kind(seat: &SeatId) -> String {
    format!("delivery-control:{seat}")
}

fn row(kind: String, dedupe: &str, command: Option<&str>) -> Job {
    Job {
        kind,
        serial_key: SEAT.to_string(),
        payload: serde_json::to_string(&Msg {
            from: SeatId::from("pij-sender"),
            from_machine: None,
            to: SeatId::from(SEAT),
            body: String::new(),
            msg_id: dedupe.to_string(),
            in_reply_to: None,
            command: command.map(str::to_string),
        })
        .expect("payload"),
        dedupe_key: dedupe.to_string(),
        dedupe_origin: None,
        attempt: 0,
    }
}

fn body(dedupe: &str) -> Job {
    row(delivery_kind(&SeatId::from(SEAT)), dedupe, None)
}

fn control(dedupe: &str) -> Job {
    row(control_kind(&SeatId::from(SEAT)), dedupe, Some("compact"))
}

async fn enqueued(queue: &dyn Queue, job: Job) {
    assert!(matches!(
        queue.enqueue_delivery(job).await.expect("enqueue delivery"),
        DeliveryEnqueue::Queued { .. }
    ));
}

async fn sqlite() -> (FreshStore, SqliteQueue) {
    let fresh = FreshStore::new();
    let queue = SqliteQueue::new(
        pij_store::open(&fresh.path()).await.expect("open"),
        Config::default().claim_lease_secs,
        Config::default().delivered_id_capacity,
    )
    .expect("queue");
    (fresh, queue)
}

/// Neither claimer sees the other's kind, and a running claim on either kind
/// holds the other until it is acknowledged.
async fn overlap_guard_contract(queue: &dyn Queue) {
    let seat = SeatId::from(SEAT);
    let (bodies, controls) = ([delivery_kind(&seat)], [control_kind(&seat)]);
    enqueued(queue, body("b-1")).await;
    enqueued(queue, control("c-1")).await;

    let (native, claimed) = queue.claim(&bodies, "native").await.unwrap().unwrap();
    assert_eq!(claimed.dedupe_key, "b-1");
    assert!(
        queue.claim(&controls, "drain").await.unwrap().is_none(),
        "a control must not be typed while a native body is claimed"
    );
    queue
        .ack_delivery(native, DeliveryOrigin::ReaderRead)
        .await
        .unwrap();

    let (typed, claimed) = queue.claim(&controls, "drain").await.unwrap().unwrap();
    assert_eq!(claimed.dedupe_key, "c-1");
    enqueued(queue, body("b-2")).await;
    assert!(
        queue.claim(&bodies, "native").await.unwrap().is_none(),
        "a native body must not be claimed while a control is being typed"
    );
    queue
        .ack_delivery(typed, DeliveryOrigin::InjectedToTransport)
        .await
        .unwrap();
    assert!(queue.admitted(&seat, "c-1", None).await.unwrap());
    let (_, claimed) = queue.claim(&bodies, "native").await.unwrap().unwrap();
    assert_eq!(claimed.dedupe_key, "b-2");
}

/// Order across the two kinds is by id among eligible rows, not strict FIFO:
/// a compact queued behind pending bodies may run before the later ones, but
/// never while one is mid-claim, and every body is still delivered.
async fn ordering_contract(queue: &dyn Queue) {
    let seat = SeatId::from(SEAT);
    let (bodies, controls) = ([delivery_kind(&seat)], [control_kind(&seat)]);
    for dedupe in ["b-1", "b-2", "b-3"] {
        enqueued(queue, body(dedupe)).await;
    }
    enqueued(queue, control("c-1")).await;

    let mut delivered = Vec::new();
    let mut control_ran = false;
    while let Some((id, job)) = queue.claim(&bodies, "native").await.unwrap() {
        assert!(queue.claim(&controls, "drain").await.unwrap().is_none());
        queue
            .ack_delivery(id, DeliveryOrigin::ReaderRead)
            .await
            .unwrap();
        delivered.push(job.dedupe_key);
        if !control_ran {
            let (id, _) = queue.claim(&controls, "drain").await.unwrap().unwrap();
            assert!(queue.claim(&bodies, "native").await.unwrap().is_none());
            queue.ack(id, Outcome::Done).await.unwrap();
            control_ran = true;
        }
    }
    assert!(control_ran);
    assert_eq!(delivered, ["b-1", "b-2", "b-3"]);
}

/// The control kind is a delivery row for the store's own bookkeeping:
/// deferral facts and the duplicate ledger see it.
async fn bookkeeping_contract(queue: &dyn Queue) {
    let seat = SeatId::from(SEAT);
    enqueued(queue, control("c-held")).await;
    assert!(queue.admitted(&seat, "c-held", None).await.unwrap());
    let (id, _) = queue
        .claim(&[control_kind(&seat)], "drain")
        .await
        .unwrap()
        .unwrap();
    queue
        .record_delivery_deferral(id, "recipient-busy", None, 1_000, &FakeSpine::new())
        .await
        .unwrap();
    let deferrals = queue.delivery_deferrals(&seat).await.unwrap();
    assert_eq!(deferrals.len(), 1);
    assert_eq!(deferrals[0].reason, "recipient-busy");
}

/// A compact waiting on a busy seat must not stop the native receiver's bodies
/// from parking behind it when that receiver goes away.
async fn parking_contract(queue: &dyn Queue) {
    let seat = SeatId::from(SEAT);
    enqueued(queue, control("c-waiting")).await;
    enqueued(queue, body("b-stranded")).await;
    let (id, job) = queue.peek(&[delivery_kind(&seat)]).await.unwrap().unwrap();
    let evidence = ParkingEvidence {
        outcome: DeliveryFailure::NativeReceiverUnavailable,
        reason: "native receiver gone",
        at: 1_000,
    };
    let (parked, _) = queue
        .park_delivery(id, &seat, job.attempt, &evidence, &FakeSpine::new())
        .await
        .unwrap();
    assert_eq!(parked.expect("body parks").dedupe_key, "b-stranded");
    assert!(
        queue.peek(&[control_kind(&seat)]).await.unwrap().is_some(),
        "the compact itself is never parked as a body"
    );
}

#[tokio::test]
async fn copilot_waiting_compact_never_blocks_native_parking_on_sqlite() {
    let (_fresh, queue) = sqlite().await;
    parking_contract(&queue).await;
}

#[tokio::test]
async fn copilot_waiting_compact_never_blocks_native_parking_on_the_fake() {
    parking_contract(&FakeQueue::new(1_024).unwrap()).await;
}

#[tokio::test]
async fn copilot_control_and_body_kinds_never_overlap_on_sqlite() {
    let (_fresh, queue) = sqlite().await;
    overlap_guard_contract(&queue).await;
}

#[tokio::test]
async fn copilot_control_and_body_kinds_never_overlap_on_the_fake() {
    overlap_guard_contract(&FakeQueue::new(1_024).unwrap()).await;
}

#[tokio::test]
async fn copilot_compact_behind_bodies_never_runs_mid_claim_on_sqlite() {
    let (_fresh, queue) = sqlite().await;
    ordering_contract(&queue).await;
}

#[tokio::test]
async fn copilot_compact_behind_bodies_never_runs_mid_claim_on_the_fake() {
    ordering_contract(&FakeQueue::new(1_024).unwrap()).await;
}

#[tokio::test]
async fn copilot_control_rows_are_delivery_rows_for_bookkeeping_on_sqlite() {
    let (_fresh, queue) = sqlite().await;
    bookkeeping_contract(&queue).await;
}

#[tokio::test]
async fn copilot_control_rows_are_delivery_rows_for_bookkeeping_on_the_fake() {
    bookkeeping_contract(&FakeQueue::new(1_024).unwrap()).await;
}
