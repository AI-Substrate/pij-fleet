//! Cancellation witness for the SQLx COMMIT/ack boundary. The wrapper pauses
//! after a real atomic registry commit but before EventBus receives its result.
//! sqlx-sqlite 0.8.6 connection/worker.rs:268-272 explicitly permits this state:
//! COMMIT processed, caller acknowledgement cancelled, rollback ignored.

use async_trait::async_trait;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine};
use pij_store::spine::{RegistryCommit, RegistryPublication, RegistryPublisher};
use pij_store::{SqliteRegistry, SqliteSpine};
use pij_testkit::FreshStore;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_stream::StreamExt;

use super::{EventBus, EventFilter};

struct PauseAfterCommit {
    bus: Arc<EventBus>,
    committed: Arc<Notify>,
    release: Arc<Notify>,
}

impl RegistryPublisher for PauseAfterCommit {
    fn publish_registry<'a>(&'a self, commit: RegistryCommit) -> RegistryPublication<'a> {
        let committed = self.committed.clone();
        let release = self.release.clone();
        self.bus.publish_registry(Box::pin(async move {
            let result = commit.await?;
            committed.notify_one();
            release.notified().await;
            Ok(result)
        }))
    }
}

#[tokio::test]
async fn cancelled_registry_caller_cannot_lose_a_committed_row_before_later_publication() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testkit/fixtures/golden/api/governance-events.json"
    )))
    .unwrap();
    let cases = fixture["events"].as_array().unwrap();
    let seat: SeatDescriptor = serde_json::from_value(
        cases.iter().find(|case| case["id"] == "seat-put").unwrap()["decoded_payload"].clone(),
    )
    .unwrap();
    let marker: Event = serde_json::from_value(
        cases
            .iter()
            .find(|case| case["id"] == "report-now")
            .unwrap()["frame"]["event"]
            .clone(),
    )
    .unwrap();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let raw = Arc::new(SqliteSpine::new(pool.clone()));
    let bus = Arc::new(EventBus::new(raw.clone(), 16).unwrap());
    let committed = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let publisher = Arc::new(PauseAfterCommit {
        bus: bus.clone(),
        committed: committed.clone(),
        release: release.clone(),
    });
    let registry = Arc::new(SqliteRegistry::new(pool, publisher));
    let mut first = bus.subscribe_live(EventFilter::all());
    let mut second = bus.subscribe_live(EventFilter::all());
    let writer_registry = registry.clone();
    let writer_seat = seat.clone();
    let caller = tokio::spawn(async move { writer_registry.put(writer_seat).await });
    timeout(Duration::from_secs(3), committed.notified())
        .await
        .expect("real SQL commit reached");
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat));
    let committed_row = raw.tail(None, Seq(0)).await.unwrap().pop().unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let next_bus = bus.clone();
    let later = tokio::spawn(async move { next_bus.publish(marker).await });
    release.notify_one();
    let later_seq = timeout(Duration::from_secs(3), later)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(later_seq > committed_row.seq.unwrap());
    for stream in [&mut first, &mut second] {
        let observed = timeout(Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            observed, committed_row,
            "the aborted caller must not drop its committed event; a later cursor would permanently skip it"
        );
        let later = timeout(Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(later.seq, Some(later_seq));
    }
}

struct PauseAfterAppend {
    raw: Arc<SqliteSpine>,
    pause: AtomicBool,
    committed: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl Spine for PauseAfterAppend {
    async fn append(&self, event: Event) -> Result<Seq> {
        let seq = self.raw.append(event).await?;
        if self.pause.swap(false, Ordering::SeqCst) {
            self.committed.notify_one();
            self.release.notified().await;
        }
        Ok(seq)
    }

    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        self.raw.tail(seat, since).await
    }

    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        self.raw.latest_matching(seat, kinds).await
    }

    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        self.raw.latest_matching_message(seat, kind, msg_id).await
    }

    async fn matching_since(
        &self,
        seat: &SeatId,
        window: &pij_core::ports::SpineWindow,
    ) -> Result<Vec<Event>> {
        self.raw.matching_since(seat, window).await
    }
}

fn report_fixture() -> Event {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testkit/fixtures/golden/api/governance-events.json"
    )))
    .unwrap();
    serde_json::from_value(
        fixture["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "report-now")
            .unwrap()["frame"]["event"]
            .clone(),
    )
    .unwrap()
}

#[tokio::test]
async fn cancelled_ordinary_publisher_retains_ordering_and_flush_waits_for_its_commit() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let raw = Arc::new(SqliteSpine::new(pool));
    let committed = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let paused = Arc::new(PauseAfterAppend {
        raw: raw.clone(),
        pause: AtomicBool::new(true),
        committed: committed.clone(),
        release: release.clone(),
    });
    let bus = Arc::new(EventBus::new(paused, 16).unwrap());
    let mut first = bus.subscribe_live(EventFilter::all());
    let mut second = bus.subscribe_live(EventFilter::all());
    let writer = bus.clone();
    let caller = tokio::spawn(async move { writer.publish(report_fixture()).await });
    timeout(Duration::from_secs(3), committed.notified())
        .await
        .unwrap();
    let committed_row = raw.tail(None, Seq(0)).await.unwrap().pop().unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());

    let mut flush = Box::pin(bus.flush());
    std::future::poll_fn(|cx| {
        assert!(
            flush.as_mut().poll(cx).is_pending(),
            "flush must wait for the admitted task even after its caller is gone"
        );
        std::task::Poll::Ready(())
    })
    .await;
    let next_bus = bus.clone();
    let next = tokio::spawn(async move { next_bus.publish(report_fixture()).await });
    release.notify_one();
    timeout(Duration::from_secs(3), flush).await.unwrap();
    let next_seq = timeout(Duration::from_secs(3), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Production stops and joins producers BEFORE this final shutdown barrier.
    bus.flush().await;
    for stream in [&mut first, &mut second] {
        assert_eq!(
            timeout(Duration::from_secs(3), stream.next())
                .await
                .unwrap()
                .unwrap(),
            committed_row
        );
        assert_eq!(
            timeout(Duration::from_secs(3), stream.next())
                .await
                .unwrap()
                .unwrap()
                .seq,
            Some(next_seq)
        );
    }
}

#[tokio::test]
async fn cancellation_before_admission_does_not_poll_or_commit_the_mutation() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let raw = Arc::new(SqliteSpine::new(pool));
    let bus = EventBus::new(raw.clone(), 16).unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let polled = called.clone();
    let store = raw.clone();
    let guard = bus.publish_lock.lock().await;
    let mut publication = Box::pin(bus.publish_committed(async move {
        polled.store(true, Ordering::SeqCst);
        let mut event = report_fixture();
        event.seq = Some(store.append(event.clone()).await?);
        Ok((event, ()))
    }));
    std::future::poll_fn(|cx| {
        assert!(publication.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(publication);
    drop(guard);
    bus.flush().await;
    assert!(!called.load(Ordering::SeqCst));
    assert!(raw.tail(None, Seq(0)).await.unwrap().is_empty());
}

#[tokio::test]
async fn generic_publication_preserves_concrete_result_and_failure_emits_nothing() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let raw = Arc::new(SqliteSpine::new(pool));
    let bus = EventBus::new(raw.clone(), 16).unwrap();
    let mut live = bus.subscribe_live(EventFilter::all());
    let failed = bus
        .publish_committed::<Vec<String>, _>(async {
            Err(PijError::Adapter {
                adapter: "test/atomic-mutation".to_string(),
                message: "fixture refusal".to_string(),
            })
        })
        .await
        .unwrap_err();
    assert!(failed.to_string().contains("fixture refusal"));
    assert!(raw.tail(None, Seq(0)).await.unwrap().is_empty());
    let mut event = report_fixture();
    let payload: Value = serde_json::from_str(&event.payload).unwrap();
    let expected = vec![
        payload["did"].as_str().unwrap().to_string(),
        payload["next"].as_str().unwrap().to_string(),
    ];
    let result = expected.clone();
    let (seq, actual) = bus
        .publish_committed(async move {
            event.seq = Some(raw.append(event.clone()).await?);
            Ok((event, result))
        })
        .await
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        timeout(Duration::from_secs(3), live.next())
            .await
            .unwrap()
            .unwrap()
            .seq,
        Some(seq)
    );
    bus.flush().await;
}
