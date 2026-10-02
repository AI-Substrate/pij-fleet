//! Durable replay joined to non-blocking live event delivery.

mod fake_registry;
pub use fake_registry::PublishedFakeRegistry;

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::events::EventFilter;
use pij_core::model::{Event, SeatId, Seq};
use pij_core::ports::Spine;
use pij_store::spine::{RegistryCommit, RegistryPublication, RegistryPublisher};
use tokio::sync::{Mutex, broadcast};
use tokio_stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

/// Appends every event durably before offering it to live subscribers.
///
/// A subscriber attaches to the live channel before it tails the spine. Events
/// observed by both legs carry the same store-assigned [`Seq`], so the join can
/// discard overlap without confusing two equal-but-distinct facts.
///
/// # Composition recipe
///
/// Construct the raw Spine first, then one `Arc<EventBus>`. Inject that bus as
/// `RegistryPublisher` into `SqliteRegistry::new(pool, bus.clone())`, and expose
/// the same bus as `Services.spine`. All producers, including ReportService,
/// then share this ordering lock and one durable sequence domain.
pub struct EventBus {
    spine: Arc<dyn Spine>,
    live: broadcast::Sender<Event>,
    publish_lock: Arc<Mutex<()>>,
    // A held row must publish its admission facts before a worker can ack it.
    // Never held across transport IO; only queue and receipt publication.
    pub(crate) socket_delivery_order: Mutex<()>,
}

impl EventBus {
    /// Build a bus with a bounded per-subscriber live buffer.
    ///
    /// # Errors
    /// [`PijError::Adapter`] when `live_capacity` is zero.
    pub fn new(spine: Arc<dyn Spine>, live_capacity: usize) -> Result<Self> {
        if live_capacity == 0 {
            return Err(PijError::Adapter {
                adapter: "daemon/events".to_string(),
                message: "event_buffer_capacity must be at least 1".to_string(),
            });
        }
        let (live, _) = broadcast::channel(live_capacity);
        Ok(Self {
            spine,
            live,
            publish_lock: Arc::new(Mutex::new(())),
            socket_delivery_order: Mutex::new(()),
        })
    }

    /// Persist `event`, then make the persisted fact visible to live subscribers.
    ///
    /// # Errors
    /// Store failures, or an event that already carries a sequence on its write
    /// path. Having no live subscribers is not an error: the durable leg remains.
    pub async fn publish(&self, mut event: Event) -> Result<Seq> {
        if event.seq.is_some() {
            return Err(PijError::Adapter {
                adapter: "daemon/events".to_string(),
                message: "publish requires event.seq = None; the store assigns the cursor"
                    .to_string(),
            });
        }

        let spine = self.spine.clone();
        self.publish_committed(async move {
            let seq = spine.append(event.clone()).await?;
            event.seq = Some(seq);
            Ok((event, ()))
        })
        .await
        .map(|(seq, ())| seq)
    }

    /// Commit one owned atomic mutation and publish its event in sequence order.
    ///
    /// `commit` must remain lazy: acquire SQLite only when this future is polled,
    /// commit the state and event together, then return an event with its assigned
    /// sequence. Never call the bus recursively from the callback.
    ///
    /// Cancellation before admission starts no work. After admission, an owned
    /// task retains the ordering guard through commit and broadcast even if the
    /// requesting future is dropped. Stop and join producers, then [`Self::flush`]
    /// before shutting down the runtime; runtime termination cannot be shielded.
    ///
    /// # Errors
    /// Transaction failure, missing committed sequence, or publication task failure.
    pub async fn publish_committed<T, F>(&self, commit: F) -> Result<(Seq, T)>
    where
        T: Send + 'static,
        F: Future<Output = Result<(Event, T)>> + Send + 'static,
    {
        self.publish_committed_batch(async move {
            let (event, result) = commit.await?;
            let seq = event.seq.ok_or_else(|| PijError::Adapter {
                adapter: "daemon/events".into(),
                message: "publication callback committed without its event sequence".into(),
            })?;
            Ok(([event], (seq, result)))
        })
        .await
    }

    /// Commit zero or more events with one mutation and publish them in order.
    /// Empty event batches allow a serial inbox claim that parked no rows.
    ///
    /// # Errors
    /// Transaction failure, missing committed sequences, or publication task failure.
    pub async fn publish_committed_batch<T, F, I>(&self, commit: F) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<(I, T)>> + Send + 'static,
        I: AsRef<[Event]> + IntoIterator<Item = Event> + Send + 'static,
    {
        let guard = self.publish_lock.clone().lock_owned().await;
        let live = self.live.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let (events, result) = commit.await?;
            if events.as_ref().iter().any(|event| event.seq.is_none()) {
                return Err(PijError::Adapter {
                    adapter: "daemon/events".into(),
                    message: "publication callback committed without its event sequence".into(),
                });
            }
            for event in events {
                let _ = live.send(event);
            }
            Ok(result)
        })
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/events".to_string(),
            message: format!("admitted publication task failed: {error}"),
        })?
    }

    /// Raw persistence for an in-memory adapter's append-before-mutate bridge.
    pub(crate) fn raw_spine(&self) -> Arc<dyn Spine> {
        self.spine.clone()
    }

    /// Wait for previously admitted writes to finish committing and publishing.
    ///
    /// First stop/cancel and join every request/background producer so no future
    /// publication can arrive behind this barrier. Await this while the Tokio
    /// runtime and store are still alive, then close them. This is a drain, not
    /// a retry or a success receipt for an individual cancelled request.
    pub async fn flush(&self) {
        let _guard = self.publish_lock.lock().await;
    }

    /// Replay events after `since`, then continue with the live stream.
    ///
    /// The live receiver is created first. Anything appended while the tail is
    /// read therefore appears on at least one leg; sequence comparison removes
    /// overlap. `None` replays from the start.
    ///
    /// # Errors
    /// Store failures, or a replay row missing its store-assigned sequence.
    pub async fn subscribe(&self, since: Option<Seq>, filter: EventFilter) -> Result<Subscription> {
        let receiver = self.live.subscribe();
        let since = since.unwrap_or(Seq(0));

        // REFUSE a cursor beyond this spine's newest sequence, by name.
        //
        // Such a cursor cannot be satisfied and cannot be corrected by waiting: it
        // describes a history this spine does not have, which happens when a
        // daemon is reprovisioned or its store wiped while its alias survives.
        // Silently filtering `seq <= last_seq` from a stale high cursor meant a
        // reset peer emitted NOTHING for ever while its status read Connected —
        // and made the backwards-cursor branch downstream unreachable, because no
        // low frame ever left this side (review F7).
        let newest = self
            .spine
            .tail(None, Seq(0))
            .await?
            .last()
            .and_then(|event| event.seq)
            .unwrap_or(Seq(0));

        if since > newest {
            return Err(PijError::CursorBeyondSpine {
                requested: since.0,
                newest: newest.0,
            });
        }

        let replayed = self.spine.tail(None, since).await?;
        let mut replay = VecDeque::new();
        let mut last_seq = since;

        for event in replayed {
            let seq = event.seq.ok_or_else(|| PijError::Adapter {
                adapter: "daemon/events".to_string(),
                message: "Spine::tail returned an event without its assigned sequence".to_string(),
            })?;
            last_seq = last_seq.max(seq);
            if filter.matches(&event) {
                replay.push_back(event);
            }
        }

        Ok(Subscription {
            replay,
            live: BroadcastStream::new(receiver),
            filter,
            last_seq,
            dropped: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Return one latest durable policy fact for a mandatory seat scope.
    ///
    /// The spine port owns the bounded read; this wrapper never materializes a
    /// history for callers that need only its tail.
    pub async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        self.spine.latest_matching(seat, kinds).await
    }

    /// Return the latest durable fact for one exact seat, kind, and message.
    pub async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        self.spine.latest_matching_message(seat, kind, msg_id).await
    }

    /// Attach to events published after this call without replaying the spine.
    ///
    /// Use this for a consumer that only needs a wake-up for future work. A
    /// durable consumer resuming known work uses [`Self::subscribe`] with
    /// `Some(cursor)`; `subscribe(None, ..)` is reserved for an intentional cold
    /// replay from the beginning.
    pub fn subscribe_live(&self, filter: EventFilter) -> Subscription {
        Subscription {
            replay: VecDeque::new(),
            live: BroadcastStream::new(self.live.subscribe()),
            filter,
            last_seq: Seq(0),
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl RegistryPublisher for EventBus {
    fn publish_registry<'a>(&'a self, commit: RegistryCommit) -> RegistryPublication<'a> {
        Box::pin(self.publish_committed(commit))
    }
}

#[async_trait]
impl Spine for EventBus {
    async fn append(&self, event: Event) -> Result<Seq> {
        self.publish(event).await
    }

    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        self.spine.tail(seat, since).await
    }

    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        self.spine.latest_matching(seat, kinds).await
    }

    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        self.spine.latest_matching_message(seat, kind, msg_id).await
    }
}

/// One replay-then-live event stream with an observable lag counter.
pub struct Subscription {
    replay: VecDeque<Event>,
    live: BroadcastStream<Event>,
    filter: EventFilter,
    last_seq: Seq,
    dropped: Arc<AtomicU64>,
}

impl Subscription {
    /// Number of live events discarded because this subscriber lagged.
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Stream for Subscription {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(event) = self.replay.pop_front() {
            return Poll::Ready(Some(event));
        }

        loop {
            match Pin::new(&mut self.live).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    let Some(seq) = event.seq else {
                        // Only `publish` feeds the live channel and it always assigns
                        // the sequence first. Ignore a malformed internal item rather
                        // than inventing a cursor or terminating healthy subscribers.
                        continue;
                    };
                    if seq <= self.last_seq {
                        continue;
                    }
                    self.last_seq = seq;
                    if self.filter.matches(&event) {
                        return Poll::Ready(Some(event));
                    }
                }
                Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(count)))) => {
                    self.dropped.fetch_add(count, Ordering::Relaxed);
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod cursor_refusal_tests {
    use std::sync::Arc;

    use pij_core::error::PijError;
    use pij_core::model::Seq;
    use pij_testkit::fakes::FakeSpine;

    use super::{EventBus, EventFilter};

    /// Review F7 — a cursor beyond this spine's newest is REFUSED BY NAME.
    ///
    /// It cannot be satisfied and cannot be corrected by waiting: it describes a
    /// history this spine does not have, which is what a reprovisioned daemon
    /// looks like from outside. Filtering it silently left a consumer waiting for
    /// ever on a stream that read healthy, and made the downstream
    /// backwards-cursor branch unreachable — nothing low ever left this side.
    ///
    /// Mutation witness: delete the `since > newest` guard and this fails, because
    /// the subscription is handed back and streams nothing.
    #[tokio::test]
    async fn a_cursor_beyond_the_spine_is_refused_by_name() {
        let bus = Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 16).expect("event bus"));

        let refused = bus.subscribe(Some(Seq(57)), EventFilter::all()).await;
        match refused {
            Err(PijError::CursorBeyondSpine { requested, newest }) => {
                assert_eq!(requested, 57);
                assert_eq!(newest, 0, "an empty spine's newest sequence is zero");
            }
            Err(other) => panic!("the refusal must NAME the impossible cursor: {other}"),
            Ok(_) => panic!(
                "a cursor beyond the spine must be refused, not silently filtered into silence"
            ),
        }

        // ...and a cursor the spine CAN serve is still served.
        bus.subscribe(Some(Seq(0)), EventFilter::all())
            .await
            .expect("a satisfiable cursor subscribes");
    }
}

#[cfg(test)]
mod governance_tests;

#[cfg(test)]
mod governance_http_tests;

#[cfg(test)]
mod governance_cancellation_tests;
