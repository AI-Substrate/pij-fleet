use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::model::SeatDescriptor;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::{Stream, StreamMap};

use crate::http::{
    FederatedRoster, PeerEndpoint, PeerStreamState, StreamFrame, UnavailablePeer, get_from_peer,
    stream_from_peer,
};

struct RosterSnapshot {
    seats: Vec<SeatDescriptor>,
    unavailable: Option<String>,
}

struct PeerSource {
    alias: String,
    endpoint: PeerEndpoint,
    roster: RwLock<RosterSnapshot>,
    events: Mutex<VecDeque<StreamFrame>>,
    status: Mutex<StreamFrame>,
    live: tokio::sync::broadcast::Sender<StreamFrame>,
}

impl PeerSource {
    fn new(alias: String, endpoint: PeerEndpoint, capacity: usize) -> Self {
        let (live, _) = tokio::sync::broadcast::channel(capacity);
        Self {
            status: Mutex::new(StreamFrame::PeerState {
                machine: alias.clone(),
                state: PeerStreamState::Unavailable,
                retry_in_ms: None,
                dropped: None,
                reason: Some("peer has not completed its first connection".to_string()),
            }),
            alias,
            endpoint,
            roster: RwLock::new(RosterSnapshot {
                seats: Vec::new(),
                unavailable: Some("peer has not completed its first roster poll".to_string()),
            }),
            events: Mutex::new(VecDeque::with_capacity(capacity)),
            live,
        }
    }

    fn publish_status(&self, frame: StreamFrame) {
        *self.status.lock().expect("peer status mutex") = frame.clone();
        let _ = self.live.send(frame);
    }

    fn publish_event(&self, frame: StreamFrame, capacity: usize) {
        let mut events = self.events.lock().expect("peer event buffer mutex");
        if events.len() == capacity {
            events.pop_front();
        }
        events.push_back(frame.clone());
        drop(events);
        let _ = self.live.send(frame);
    }

    fn last_cursor(&self) -> Option<u64> {
        self.events
            .lock()
            .expect("peer event buffer mutex")
            .iter()
            .rev()
            .find_map(|frame| match frame {
                StreamFrame::Event { cursor, .. } => Some(*cursor),
                StreamFrame::PeerState { .. } => None,
            })
    }
}

pub struct FanInService {
    peers: BTreeMap<String, Arc<PeerSource>>,
    client: reqwest::Client,
    stream_client: reqwest::Client,
    base_delay: Duration,
    max_delay: Duration,
    event_capacity: usize,
}

impl FanInService {
    pub fn new(
        peers: impl IntoIterator<Item = (String, PeerEndpoint)>,
        client: reqwest::Client,
        stream_client: reqwest::Client,
        base_delay: Duration,
        max_delay: Duration,
        event_capacity: usize,
    ) -> Self {
        Self {
            peers: peers
                .into_iter()
                .map(|(alias, endpoint)| {
                    let source = Arc::new(PeerSource::new(alias.clone(), endpoint, event_capacity));
                    (alias, source)
                })
                .collect(),
            client,
            stream_client,
            base_delay,
            max_delay,
            event_capacity,
        }
    }

    pub fn roster(&self, mut local: Vec<SeatDescriptor>) -> FederatedRoster {
        let mut unavailable = Vec::new();
        for source in self.peers.values() {
            let snapshot = source.roster.read().expect("peer roster lock");
            local.extend(snapshot.seats.iter().cloned());
            if let Some(reason) = &snapshot.unavailable {
                unavailable.push(UnavailablePeer {
                    machine: source.alias.clone(),
                    reason: reason.clone(),
                });
            }
        }
        local.sort_by(|left, right| {
            left.machine
                .cmp(&right.machine)
                .then_with(|| left.id.cmp(&right.id))
        });
        FederatedRoster {
            seats: local,
            unavailable,
        }
    }

    pub fn subscribe(&self, cursors: &BTreeMap<String, u64>) -> RemoteSubscription {
        let mut replay = VecDeque::new();
        let mut live = StreamMap::new();
        let mut seen = cursors.clone();

        for source in self.peers.values() {
            live.insert(
                source.alias.clone(),
                BroadcastStream::new(source.live.subscribe()),
            );
            replay.push_back(source.status.lock().expect("peer status mutex").clone());
            let Some(requested) = cursors.get(&source.alias).copied() else {
                continue;
            };
            let events = source.events.lock().expect("peer event buffer mutex");
            if let Some(first) = events.iter().find_map(|frame| match frame {
                StreamFrame::Event { cursor, .. } => Some(*cursor),
                StreamFrame::PeerState { .. } => None,
            }) && requested.saturating_add(1) < first
            {
                replay.push_back(StreamFrame::PeerState {
                    machine: source.alias.clone(),
                    state: PeerStreamState::Lagged,
                    retry_in_ms: None,
                    dropped: Some(first - requested - 1),
                    reason: Some(
                        "requested cursor is older than the bounded remote-event buffer"
                            .to_string(),
                    ),
                });
                seen.insert(source.alias.clone(), first - 1);
            }
            replay.extend(events.iter().filter_map(|frame| match frame {
                StreamFrame::Event {
                    machine,
                    cursor,
                    event,
                } if *cursor > requested => Some(StreamFrame::Event {
                    machine: machine.clone(),
                    cursor: *cursor,
                    event: event.clone(),
                }),
                _ => None,
            }));
        }

        RemoteSubscription { replay, live, seen }
    }

    pub fn start(
        self: &Arc<Self>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut joined = Vec::with_capacity(self.peers.len() * 2);
        for source in self.peers.values() {
            let roster_service = Arc::clone(self);
            let roster_source = Arc::clone(source);
            let roster_shutdown = shutdown.clone();
            joined.push(tokio::spawn(async move {
                roster_service
                    .roster_loop(roster_source, roster_shutdown)
                    .await;
            }));

            let event_service = Arc::clone(self);
            let event_source = Arc::clone(source);
            let event_shutdown = shutdown.clone();
            joined.push(tokio::spawn(async move {
                event_service.event_loop(event_source, event_shutdown).await;
            }));
        }
        joined
    }

    async fn roster_loop(
        &self,
        source: Arc<PeerSource>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut delay = self.base_delay;
        loop {
            let succeeded = match self.poll_roster(&source).await {
                Ok(()) => true,
                Err(error) => {
                    source.roster.write().expect("peer roster lock").unavailable =
                        Some(error.to_string());
                    false
                }
            };
            let sleep_for = if succeeded { self.base_delay } else { delay };
            delay = if succeeded {
                self.base_delay
            } else {
                next_delay(delay, self.max_delay)
            };
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                () = tokio::time::sleep(sleep_for) => {}
            }
        }
    }

    async fn poll_roster(&self, source: &PeerSource) -> Result<()> {
        let envelope: pij_core::model::Envelope<FederatedRoster> =
            get_from_peer(&self.client, &source.endpoint, "/v1/seats?scope=local").await?;
        if !envelope.ok {
            return Err(PijError::Adapter {
                adapter: "daemon/federation-roster".to_string(),
                message: envelope
                    .meta
                    .unwrap_or_else(|| "peer refused its local roster".to_string()),
            });
        }
        let roster = envelope.data.ok_or_else(|| PijError::Adapter {
            adapter: "daemon/federation-roster".to_string(),
            message: format!("peer {} returned ok without roster data", source.alias),
        })?;
        // TRUST THE PEER'S OWN STAMP, and surface a mismatch instead of hiding it.
        //
        // This used to overwrite every remote seat's machine with our CONFIGURED
        // alias, while the event path REJECTS frames whose machine differs from
        // that alias. So a config-vs-hostname mismatch produced two answers about
        // one peer: a roster that read fine for ever, and a stream permanently
        // unavailable — with the roster half hiding the cause (review F4).
        let claimed: Vec<&str> = roster
            .seats
            .iter()
            .filter_map(|seat| seat.machine.as_deref())
            .filter(|machine| *machine != source.alias)
            .collect();
        let unavailable = if claimed.is_empty() {
            None
        } else {
            Some(format!(
                "configured peer {} returned seats stamped {} — the peer's \
                 machine_alias and this daemon's peer alias disagree, so its \
                 event stream will refuse every frame",
                source.alias,
                claimed.join(", ")
            ))
        };
        *source.roster.write().expect("peer roster lock") = RosterSnapshot {
            seats: roster.seats,
            unavailable,
        };
        Ok(())
    }

    async fn event_loop(
        &self,
        source: Arc<PeerSource>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut delay = self.base_delay;
        // Set when the peer refuses our cursor as impossible: the next attempt
        // resumes LIVE from the peer's real position instead of asking again for a
        // history that no longer exists. Without it the worker retried the same
        // rejected `since` for ever (review F7).
        let mut resume_live = false;
        loop {
            let since = if resume_live {
                None
            } else {
                source.last_cursor()
            };
            let since_json = since.and_then(|cursor| {
                serde_json::to_string(&BTreeMap::from([(source.alias.clone(), cursor)])).ok()
            });
            let mut query = vec![("scope", "local".to_string())];
            if let Some(since_json) = since_json {
                query.push(("since", since_json));
            }
            let attempt =
                stream_from_peer(&self.stream_client, &source.endpoint, "/v1/events", &query).await;
            match attempt {
                Ok(mut stream) => {
                    delay = self.base_delay;
                    resume_live = false;
                    source.publish_status(StreamFrame::PeerState {
                        machine: source.alias.clone(),
                        state: PeerStreamState::Connected,
                        retry_in_ms: None,
                        dropped: None,
                        reason: None,
                    });
                    loop {
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() {
                                    return;
                                }
                            }
                            frame = stream.next_frame() => match frame {
                                Ok(Some(frame)) => {
                                    if let Err(error) = self.accept_peer_frame(&source, frame) {
                                        self.publish_unavailable(&source, delay, error.to_string());
                                        break;
                                    }
                                }
                                Ok(None) => {
                                    self.publish_unavailable(&source, delay, "peer event stream ended".to_string());
                                    break;
                                }
                                Err(error) => {
                                    self.publish_unavailable(&source, delay, error.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    // The peer named our cursor as impossible: its history was
                    // reset. Say so as RESET rather than as a generic failure —
                    // "unavailable, retrying" would be false, since the peer is up
                    // and answering — and resume live on the next attempt instead
                    // of asking again for events that cannot exist (review F7).
                    // Branch on the KIND, never on the peer's wording. Keying on
                    // prose meant a one-word change in a Display string would have
                    // silently disabled this recovery and returned the fleet to
                    // the original silence, with nothing failing (review F10).
                    let is_reset = matches!(error, PijError::CursorBeyondSpine { .. });
                    let message = error.to_string();
                    if is_reset {
                        resume_live = true;
                        source.publish_status(StreamFrame::PeerState {
                            machine: source.alias.clone(),
                            state: PeerStreamState::Reset,
                            retry_in_ms: None,
                            dropped: None,
                            reason: Some(message),
                        });
                    } else {
                        self.publish_unavailable(&source, delay, message);
                    }
                }
            }
            let sleep_for = delay;
            delay = next_delay(delay, self.max_delay);
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
                () = tokio::time::sleep(sleep_for) => {}
            }
        }
    }

    fn accept_peer_frame(&self, source: &PeerSource, frame: StreamFrame) -> Result<()> {
        let StreamFrame::Event {
            machine,
            cursor,
            event,
        } = frame
        else {
            return Err(PijError::Adapter {
                adapter: "daemon/federation-events".to_string(),
                message: format!(
                    "peer {} source-only stream emitted a federated control frame",
                    source.alias
                ),
            });
        };
        if machine != source.alias {
            return Err(PijError::Adapter {
                adapter: "daemon/federation-events".to_string(),
                message: format!(
                    "configured peer {} claimed event machine {machine}",
                    source.alias
                ),
            });
        }
        if let Some(previous) = source.last_cursor() {
            if cursor < previous {
                // The peer's cursor went BACKWARDS. That is not a duplicate — it
                // is a peer whose spine was reset (reprovision, wiped state dir)
                // while its alias survived. Dropping it silently means every
                // holder of a stale high cursor receives NOTHING from that peer
                // for ever while the status still reads Connected: the E-023 shape
                // inside the code written to prevent it (review F1).
                source.publish_status(StreamFrame::PeerState {
                    machine: source.alias.clone(),
                    state: PeerStreamState::Reset,
                    retry_in_ms: None,
                    dropped: None,
                    reason: Some(format!(
                        "peer cursor went backwards ({previous} -> {cursor}): its \
                         history was reset while its alias survived"
                    )),
                });
                // Fall through and ACCEPT the frame: the buffer's newest event
                // becomes the new cursor, so the stream resumes from the peer's
                // real position instead of waiting for it to climb back past a
                // history that no longer exists.
            } else if cursor == previous {
                // A genuine duplicate: same position, already delivered.
                return Ok(());
            } else if cursor > previous.saturating_add(1) {
                source.publish_status(StreamFrame::PeerState {
                    machine: source.alias.clone(),
                    state: PeerStreamState::Lagged,
                    retry_in_ms: None,
                    dropped: Some(cursor - previous - 1),
                    reason: Some("peer stream advanced past unseen cursors".to_string()),
                });
            }
        }
        source.publish_event(
            StreamFrame::Event {
                machine,
                cursor,
                event,
            },
            self.event_capacity,
        );
        Ok(())
    }

    fn publish_unavailable(&self, source: &PeerSource, delay: Duration, reason: String) {
        source.publish_status(StreamFrame::PeerState {
            machine: source.alias.clone(),
            state: PeerStreamState::Unavailable,
            retry_in_ms: Some(duration_ms(delay)),
            dropped: None,
            reason: Some(reason),
        });
    }
}

pub struct RemoteSubscription {
    replay: VecDeque<StreamFrame>,
    live: StreamMap<String, BroadcastStream<StreamFrame>>,
    seen: BTreeMap<String, u64>,
}

impl Stream for RemoteSubscription {
    type Item = StreamFrame;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(frame) = self.replay.pop_front() {
            self.note(&frame);
            return Poll::Ready(Some(frame));
        }
        loop {
            match Pin::new(&mut self.live).poll_next(cx) {
                Poll::Ready(Some((machine, Ok(frame)))) => {
                    if let StreamFrame::Event { cursor, .. } = &frame {
                        let previous = self.seen.get(&machine).copied().unwrap_or(0);
                        if *cursor < previous {
                            // BACKWARDS, not duplicate: the source's history was
                            // reset. Silently skipping here was the consumer-side
                            // half of the same silence (review F7) — the worker
                            // above signals Reset and this filter swallowed the
                            // frame that proved it.
                            self.seen.insert(machine.clone(), *cursor);
                            self.note(&frame);
                            return Poll::Ready(Some(StreamFrame::PeerState {
                                machine,
                                state: PeerStreamState::Reset,
                                retry_in_ms: None,
                                dropped: None,
                                reason: Some(format!(
                                    "source cursor went backwards ({previous} -> {cursor}): its history was reset"
                                )),
                            }));
                        }
                        if *cursor == previous {
                            continue;
                        }
                    }
                    self.note(&frame);
                    return Poll::Ready(Some(frame));
                }
                Poll::Ready(Some((machine, Err(BroadcastStreamRecvError::Lagged(dropped))))) => {
                    return Poll::Ready(Some(StreamFrame::PeerState {
                        machine,
                        state: PeerStreamState::Lagged,
                        retry_in_ms: None,
                        dropped: Some(dropped),
                        reason: Some("bounded subscriber buffer dropped remote frames".to_string()),
                    }));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl RemoteSubscription {
    fn note(&mut self, frame: &StreamFrame) {
        if let StreamFrame::Event {
            machine, cursor, ..
        } = frame
        {
            self.seen.insert(machine.clone(), *cursor);
        }
    }
}

fn next_delay(delay: Duration, maximum: Duration) -> Duration {
    delay.saturating_mul(2).min(maximum)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use pij_core::model::Event;
    use tokio_stream::StreamExt;

    use super::{FanInService, PeerEndpoint, PeerStreamState, StreamFrame};

    fn event(cursor: u64) -> StreamFrame {
        StreamFrame::Event {
            machine: "laptop".to_string(),
            cursor,
            event: Event {
                seq: None,
                v: 1,
                at: cursor,
                kind: format!("future.{cursor}"),
                seat: None,
                payload: String::new(),
            },
        }
    }

    #[tokio::test]
    async fn cursor_older_than_bounded_buffer_emits_named_lag_before_remaining_frames() {
        let service = FanInService::new(
            [(
                "laptop".to_string(),
                PeerEndpoint {
                    base_url: "http://unused".to_string(),
                    bearer_key: "unused".to_string(),
                },
            )],
            reqwest::Client::new(),
            reqwest::Client::new(),
            Duration::from_secs(1),
            Duration::from_secs(300),
            2,
        );
        let source = service.peers.get("laptop").expect("peer");
        for cursor in 1..=3 {
            service
                .accept_peer_frame(source, event(cursor))
                .expect("accept frame");
        }

        let mut subscription = service.subscribe(&BTreeMap::from([("laptop".to_string(), 0)]));
        let _initial_status = subscription.next().await.expect("initial status");
        assert!(matches!(
            subscription.next().await,
            Some(StreamFrame::PeerState {
                machine,
                state: PeerStreamState::Lagged,
                dropped: Some(1),
                ..
            }) if machine == "laptop"
        ));
        assert!(matches!(
            subscription.next().await,
            Some(StreamFrame::Event { cursor: 2, .. })
        ));
        assert!(matches!(
            subscription.next().await,
            Some(StreamFrame::Event { cursor: 3, .. })
        ));
    }
}
