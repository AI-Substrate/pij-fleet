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
use pij_daemon::events::{EventBus, REPLAY_PAGE_ROWS};
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_stream::StreamExt;

#[derive(Default)]
struct TestSpine {
    events: Mutex<Vec<Event>>,
    /// Every `since` a `tail` was asked for, in order.
    tail_reads: Mutex<Vec<Seq>>,
    /// Every `(since, limit)` a `tail_page` was asked for, in order.
    page_reads: Mutex<Vec<(Seq, usize)>>,
    next: AtomicU64,
    pause_next_read: AtomicBool,
    read_started: Notify,
    release_read: Notify,
}

impl TestSpine {
    fn pausing() -> Self {
        Self {
            pause_next_read: AtomicBool::new(true),
            ..Self::default()
        }
    }

    async fn pause_if_armed(&self) {
        if self.pause_next_read.swap(false, Ordering::Relaxed) {
            self.read_started.notify_one();
            self.release_read.notified().await;
        }
    }

    fn page_reads(&self) -> Vec<(Seq, usize)> {
        self.page_reads.lock().expect("test spine mutex").clone()
    }

    fn tail_reads(&self) -> Vec<Seq> {
        self.tail_reads.lock().expect("test spine mutex").clone()
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
        self.tail_reads
            .lock()
            .expect("test spine mutex")
            .push(since);
        self.pause_if_armed().await;
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

    async fn tail_page(&self, since: Seq, limit: usize) -> Result<Vec<Event>> {
        self.page_reads
            .lock()
            .expect("test spine mutex")
            .push((since, limit));
        self.pause_if_armed().await;
        Ok(self
            .events
            .lock()
            .expect("test spine mutex")
            .iter()
            .filter(|event| event.seq.is_some_and(|seq| seq > since))
            .take(limit)
            .cloned()
            .collect())
    }

    async fn newest(&self) -> Result<Seq> {
        Ok(Seq(self.next.load(Ordering::Relaxed)))
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
    async fn matching_since(
        &self,
        seat: &SeatId,
        window: &pij_core::ports::SpineWindow,
    ) -> Result<Vec<Event>> {
        let mut page = self
            .events
            .lock()
            .expect("test spine mutex")
            .iter()
            .filter(|event| event.seat.as_ref() == Some(seat) && window.admits(event))
            .cloned()
            .collect::<Vec<_>>();
        page.sort_by_key(|event| (event.at, event.seq));
        page.truncate(window.limit());
        Ok(page)
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

    spine.read_started.notified().await;
    bus.publish(event(2, "message"))
        .await
        .expect("overlap event");
    spine.release_read.notify_one();
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

/// Plan 185 D1: a cursor attach reads only what follows its cursor. Before, the
/// beyond-the-spine check read `tail(None, Seq(0))` on every attach, which on the
/// live 3M-row spine cost 6-7 s and 5 GB to learn one number.
#[tokio::test]
async fn a_cursor_attach_never_reads_the_spine_before_its_cursor() {
    let spine = Arc::new(TestSpine::default());
    let bus = EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 8).expect("bus");
    for number in 1..=50 {
        bus.publish(event(number, "message"))
            .await
            .expect("publish");
    }

    let mut subscription = bus
        .subscribe(Some(Seq(48)), EventFilter::all())
        .await
        .expect("subscribe");

    assert_eq!(spine.tail_reads(), Vec::<Seq>::new(), "no unbounded read");
    assert_eq!(
        spine.page_reads(),
        vec![(Seq(48), REPLAY_PAGE_ROWS)],
        "only the replay after the cursor is read, one page"
    );
    for expected in [49, 50] {
        let item = timeout(Duration::from_secs(1), subscription.next())
            .await
            .expect("replay stalled")
            .expect("stream ended");
        assert_eq!(item.seq, Some(Seq(expected)));
    }
    let beyond = bus.subscribe(Some(Seq(51)), EventFilter::all()).await;
    assert!(
        matches!(
            beyond,
            Err(PijError::CursorBeyondSpine {
                requested: 51,
                newest: 50
            })
        ),
        "the beyond-the-spine refusal still names the newest sequence"
    );
}

async fn publish_numbered(bus: &EventBus, numbers: std::ops::RangeInclusive<u64>) {
    for number in numbers {
        bus.publish(event(number, "message"))
            .await
            .expect("publish");
    }
}

async fn next_seq(subscription: &mut pij_daemon::events::Subscription) -> u64 {
    timeout(Duration::from_secs(1), subscription.next())
        .await
        .expect("replay stalled")
        .expect("stream ended")
        .seq
        .expect("seq")
        .0
}

async fn assert_quiet(subscription: &mut pij_daemon::events::Subscription) {
    assert!(
        timeout(Duration::from_millis(50), subscription.next())
            .await
            .is_err(),
        "nothing is delivered twice"
    );
}

/// Plan 187: an attach reads ONE page before it returns, so the first frame
/// waits for at most `REPLAY_PAGE_ROWS` rows whatever the backlog, and the next
/// page is read only when the consumer has taken the last one. Before, the
/// whole backlog was read and decoded into memory first: on the live 4.2M-row
/// spine that was 5.6 s, longer than chainglass's 5 s recycle, so it never saw a
/// frame and its cursor never moved.
#[tokio::test]
async fn a_replay_reads_one_page_before_its_first_event_and_the_next_only_when_drained() {
    const PAGE: usize = REPLAY_PAGE_ROWS;
    let backlog = 2 * PAGE as u64 + 7;
    let spine = Arc::new(TestSpine::default());
    let bus = EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 8).expect("bus");
    publish_numbered(&bus, 1..=backlog).await;

    let mut subscription = bus
        .subscribe(Some(Seq(0)), EventFilter::all())
        .await
        .expect("subscribe");
    assert_eq!(
        spine.tail_reads(),
        Vec::<Seq>::new(),
        "never the whole backlog"
    );
    assert_eq!(
        spine.page_reads(),
        vec![(Seq(0), PAGE)],
        "an attach reads exactly one page before its first event"
    );

    for expected in 1..=PAGE as u64 {
        assert_eq!(next_seq(&mut subscription).await, expected);
    }
    assert_eq!(
        spine.page_reads().len(),
        1,
        "the next page waits until the consumer asks for more"
    );
    for expected in PAGE as u64 + 1..=backlog {
        assert_eq!(next_seq(&mut subscription).await, expected);
    }
    assert_eq!(
        spine.page_reads(),
        vec![
            (Seq(0), PAGE),
            (Seq(PAGE as u64), PAGE),
            (Seq(2 * PAGE as u64), PAGE)
        ],
        "each page starts at the last one's final seq; a short page ends the replay"
    );

    publish_numbered(&bus, backlog + 1..=backlog + 1).await;
    assert_eq!(next_seq(&mut subscription).await, backlog + 1, "then live");
    assert_eq!(spine.page_reads().len(), 3, "live never reads the spine");
}

/// Plan 187: events appended while a paged replay is in progress appear on
/// both legs (the next page and the live channel); each arrives exactly once,
/// in order, and so does everything published after the switch to live.
#[tokio::test]
async fn events_appended_during_a_paged_replay_arrive_once_in_order() {
    let page = REPLAY_PAGE_ROWS as u64;
    let spine = Arc::new(TestSpine::default());
    let bus = EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 4_096).expect("bus");
    publish_numbered(&bus, 1..=page + 3).await;

    let mut subscription = bus
        .subscribe(Some(Seq(0)), EventFilter::all())
        .await
        .expect("subscribe");
    let mut received = Vec::new();
    for _ in 0..10 {
        received.push(next_seq(&mut subscription).await);
    }
    publish_numbered(&bus, page + 4..=page + 8).await;
    while received.len() < (page + 8) as usize {
        received.push(next_seq(&mut subscription).await);
    }
    publish_numbered(&bus, page + 9..=page + 10).await;
    received.push(next_seq(&mut subscription).await);
    received.push(next_seq(&mut subscription).await);

    assert_eq!(
        received,
        (1..=page + 10).collect::<Vec<_>>(),
        "no gap or duplicate across pages or at the replay to live switch"
    );
    assert_quiet(&mut subscription).await;
}

/// Plan 187: a backlog of exactly one page ends on an empty page read, and a
/// cursor at the head reads one empty page; both then continue live, once.
#[tokio::test]
async fn a_backlog_of_exactly_one_page_and_a_cursor_at_the_head_switch_to_live_once() {
    let page = REPLAY_PAGE_ROWS as u64;
    let spine = Arc::new(TestSpine::default());
    let bus = EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 16).expect("bus");
    publish_numbered(&bus, 1..=page).await;

    let mut from_zero = bus
        .subscribe(Some(Seq(0)), EventFilter::all())
        .await
        .expect("subscribe at zero");
    let mut at_head = bus
        .subscribe(Some(Seq(page)), EventFilter::all())
        .await
        .expect("subscribe at the head");
    for expected in 1..=page {
        assert_eq!(next_seq(&mut from_zero).await, expected);
    }
    publish_numbered(&bus, page + 1..=page + 1).await;
    assert_eq!(next_seq(&mut from_zero).await, page + 1);
    assert_eq!(next_seq(&mut at_head).await, page + 1);
    assert_quiet(&mut from_zero).await;
    assert_quiet(&mut at_head).await;
}

/// Plan 187: a client that disconnects mid-replay stops the replay. The stream
/// is pulled, so dropping it reads no further page; before, the whole backlog
/// was read for a socket that was already closed.
#[tokio::test]
async fn dropping_a_subscription_mid_replay_reads_no_further_pages() {
    let page = REPLAY_PAGE_ROWS as u64;
    let spine = Arc::new(TestSpine::default());
    let bus = EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 8).expect("bus");
    publish_numbered(&bus, 1..=3 * page).await;

    let mut subscription = bus
        .subscribe(Some(Seq(0)), EventFilter::all())
        .await
        .expect("subscribe");
    assert_eq!(next_seq(&mut subscription).await, 1);
    drop(subscription);
    tokio::task::yield_now().await;

    assert_eq!(
        spine.tail_reads(),
        Vec::<Seq>::new(),
        "never the whole backlog"
    );
    assert_eq!(
        spine.page_reads(),
        vec![(Seq(0), REPLAY_PAGE_ROWS)],
        "nothing past the page the client was reading"
    );
}
