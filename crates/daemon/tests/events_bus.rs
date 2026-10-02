use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::events::EventFilter;
use pij_core::model::{Event, SeatId, Seq};
use pij_core::ports::Spine;
use pij_daemon::events::EventBus;
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_stream::StreamExt;

#[derive(Default)]
struct TestSpine {
    events: Mutex<Vec<Event>>,
    next: AtomicU64,
    pause_next_tail: AtomicBool,
    tail_started: Notify,
    release_tail: Notify,
}

impl TestSpine {
    fn pausing() -> Self {
        Self {
            pause_next_tail: AtomicBool::new(true),
            ..Self::default()
        }
    }
}

#[async_trait]
impl Spine for TestSpine {
    async fn append(&self, mut event: Event) -> Result<Seq> {
        let seq = Seq(self.next.fetch_add(1, Ordering::Relaxed) + 1);
        event.seq = Some(seq);
        self.events.lock().expect("test spine mutex").push(event);
        Ok(seq)
    }

    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        if self.pause_next_tail.swap(false, Ordering::Relaxed) {
            self.tail_started.notify_one();
            self.release_tail.notified().await;
        }
        Ok(self
            .events
            .lock()
            .expect("test spine mutex")
            .iter()
            .filter(|event| event.seq.is_some_and(|seq| seq > since))
            .filter(|event| seat.is_none_or(|seat| event.seat.as_ref() == Some(seat)))
            .cloned()
            .collect())
    }

    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        assert!(
            !kinds.is_empty(),
            "test spine latest kinds must be non-empty"
        );
        Ok(self
            .events
            .lock()
            .expect("test spine mutex")
            .iter()
            .rev()
            .find(|event| {
                event.seat.as_ref() == Some(seat) && kinds.iter().any(|kind| *kind == event.kind)
            })
            .cloned())
    }

    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        for event in self.events.lock().expect("test spine mutex").iter().rev() {
            if event.seat.as_ref() != Some(seat) || event.kind != kind {
                continue;
            }
            let payload: serde_json::Value =
                serde_json::from_str(&event.payload).map_err(|error| PijError::Adapter {
                    adapter: "test/spine".to_string(),
                    message: format!("invalid event payload for message lookup: {error}"),
                })?;
            if payload["msg_id"].as_str() == Some(msg_id) {
                return Ok(Some(event.clone()));
            }
        }
        Ok(None)
    }
}

fn event(number: u64, kind: &str) -> Event {
    Event {
        seq: None,
        v: 1,
        at: number,
        kind: kind.to_string(),
        seat: Some(SeatId::from("pij-a")),
        payload: number.to_string(),
    }
}

#[tokio::test]
async fn every_subscriber_receives_the_same_event_set() {
    let spine: Arc<dyn Spine> = Arc::new(TestSpine::default());
    let bus = EventBus::new(spine, 32).expect("bus");
    let mut subscribers = Vec::new();
    for _ in 0..6 {
        subscribers.push(
            bus.subscribe(Some(Seq(0)), EventFilter::all())
                .await
                .expect("subscribe"),
        );
    }

    let expected: BTreeSet<u64> = (1..=12).collect();
    for number in &expected {
        bus.publish(event(*number, "message"))
            .await
            .expect("publish");
    }

    for mut subscriber in subscribers {
        let mut received = BTreeSet::new();
        for _ in 0..expected.len() {
            let item = timeout(Duration::from_secs(1), subscriber.next())
                .await
                .expect("subscriber stalled")
                .expect("stream ended");
            received.insert(item.seq.expect("published event has seq").0);
        }
        assert_eq!(
            received, expected,
            "fan-out is set-equal for every subscriber"
        );
    }
}

#[tokio::test]
async fn a_slow_subscriber_never_blocks_publish_and_reports_drops() {
    let spine: Arc<dyn Spine> = Arc::new(TestSpine::default());
    let bus = EventBus::new(spine, 2).expect("bus");
    let mut stalled = bus
        .subscribe(Some(Seq(0)), EventFilter::all())
        .await
        .expect("subscribe");

    for number in 1..=20 {
        timeout(
            Duration::from_millis(100),
            bus.publish(event(number, "message")),
        )
        .await
        .expect("a stalled subscriber must not back-pressure publish")
        .expect("publish");
    }

    let received = timeout(Duration::from_secs(1), stalled.next())
        .await
        .expect("stream stalled")
        .expect("stream ended");
    assert!(received.seq.expect("seq").0 >= 19);
    assert_eq!(
        stalled.dropped_count(),
        18,
        "the bounded channel reports every overwritten live event"
    );
}

#[tokio::test]
async fn replay_and_live_overlap_is_joined_once_by_store_sequence() {
    let spine = Arc::new(TestSpine::pausing());
    let bus = Arc::new(EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 8).expect("bus"));
    bus.publish(event(1, "message"))
        .await
        .expect("pre-subscribe event");

    let subscribing_bus = Arc::clone(&bus);
    let subscribing = tokio::spawn(async move {
        subscribing_bus
            .subscribe(Some(Seq(0)), EventFilter::all())
            .await
            .expect("subscribe")
    });

    spine.tail_started.notified().await;
    bus.publish(event(2, "message"))
        .await
        .expect("overlap event");
    spine.release_tail.notify_one();
    let mut subscription = subscribing.await.expect("subscribe task");
    bus.publish(event(3, "message")).await.expect("live event");

    let mut received = Vec::new();
    for _ in 0..3 {
        received.push(
            timeout(Duration::from_secs(1), subscription.next())
                .await
                .expect("event gap")
                .expect("stream ended")
                .seq
                .expect("seq")
                .0,
        );
    }
    assert_eq!(
        received,
        vec![1, 2, 3],
        "the overlap has no gap or duplicate"
    );
}
