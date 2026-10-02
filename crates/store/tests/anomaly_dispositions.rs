//! Persistence proof for anomaly acknowledgement and clear dispositions.
//!
//! Detectors stay pure in `pij-core`; the existing Spine port is their durable
//! state. This test closes and reopens SQLite between decisions so an in-memory
//! latch cannot accidentally satisfy the contract.

use pij_core::anomalies::{
    AnomalyStatus, AnomalyThresholds, AnomalyView, Detector, StatusStaleDetector,
    acknowledge_event, clear_event,
};
use pij_core::model::{Card, Harness, SeatDescriptor, SeatId, Seq};
use pij_core::ports::Spine;
use pij_store::SqliteSpine;
use pij_testkit::FreshStore;

const NOW: u64 = 2_000_000;

fn scan(
    seat: &[SeatDescriptor],
    cards: &[Card],
    dispositions: &[pij_core::model::Event],
) -> Vec<pij_core::anomalies::Anomaly> {
    StatusStaleDetector.scan(&AnomalyView {
        now_ms: NOW,
        thresholds: AnomalyThresholds::default(),
        seats: seat,
        cards,
        activity: &[],
        dispatches: &[],
        done: &[],
        dispositions,
        decisions: &[],
        dead: &[],
    })
}

#[tokio::test]
async fn dispositions_survive_reopen_and_clear_holds_until_source_evidence_changes() {
    let fresh = FreshStore::new();
    let descriptor = SeatDescriptor::new("pij-coder", Harness::Omp, "/abs/worktree");
    let seats = [descriptor];
    let cards = [Card {
        seat: SeatId::from("pij-coder"),
        did: "implemented anomaly detector".to_string(),
        next: "awaiting review".to_string(),
        at: NOW - 1_900_000,
        seq: Some(Seq(80)),
    }];
    let row = scan(&seats, &cards, &[]).remove(0);
    let actor = SeatId::from("pij-pm");

    {
        let pool = pij_store::open(&fresh.path()).await.expect("open");
        let spine = SqliteSpine::new(pool);
        spine
            .append(acknowledge_event(&row, &actor, NOW))
            .await
            .expect("persist acknowledgement");
    }

    {
        let pool = pij_store::open(&fresh.path())
            .await
            .expect("reopen after ack");
        let spine = SqliteSpine::new(pool);
        let dispositions = spine
            .tail(None, Seq(0))
            .await
            .expect("read acknowledgement");
        assert_eq!(
            scan(&seats, &cards, &dispositions)[0].status,
            AnomalyStatus::Acknowledged
        );
        spine
            .append(clear_event(&row, &actor, NOW + 1))
            .await
            .expect("persist clear");
    }

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("reopen after clear");
    let spine = SqliteSpine::new(pool);
    let dispositions = spine.tail(None, Seq(0)).await.expect("read dispositions");
    assert!(scan(&seats, &cards, &dispositions).is_empty());
    assert!(
        scan(&seats, &cards, &dispositions).is_empty(),
        "rescanning unchanged evidence must not reopen a cleared row"
    );

    let changed_cards = [Card {
        seq: Some(Seq(81)),
        ..cards[0].clone()
    }];
    let recurred = scan(&seats, &changed_cards, &dispositions);
    assert_eq!(recurred.len(), 1);
    assert_eq!(recurred[0].status, AnomalyStatus::Open);
    assert_ne!(recurred[0].occurrence, row.occurrence);
}
