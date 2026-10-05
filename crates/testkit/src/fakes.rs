//! A deterministic, scriptable fake for each of the eight ports.
//!
//! Shipped as a crate, not copied per unit (mining note DL-004: every fs3
//! adapter re-invented a ~110-line recorder because no shared fake existed).
//! Three properties every fake here has, and a mocking framework does not:
//!
//! * **Deterministic** — no clocks, no threads, no ordering surprises. The same
//!   calls produce the same answers on every machine.
//! * **Scriptable** — a test programs the answers it needs
//!   (`FakeTmux::script_capture`), and unprogrammed calls return a defined
//!   default rather than panicking on an "unexpected call".
//! * **Recording** — every call is kept in order, so a test can assert what the
//!   code under test DID, not merely what it returned. `calls()` is the
//!   assertion surface.
//!
//! Doubles come from here or they do not exist: mocking frameworks are refused
//! workspace-wide by the arch gate (tenet 5).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::model::{
    BindHealth, DeliveryOrigin, DeliveryOutcome, Event, Harness, Job, JobId, ModelRow, Msg,
    Outcome, Pane, PaneProcess, ProcIdentity, Readiness, SeatDescriptor, SeatId, Seq,
};
use pij_core::ports::{
    DeferNoopReason, DeferOutcome, DeliveryAck, DeliveryEnqueue, HarnessPort, LaunchCommand,
    LivenessPort, Queue, Registry, ReleaseOutcome, STAGED_SUBMIT_RECOVERY, SeatFilter,
    SessionStatusPort, Spine, StagedSubmit, TmuxPort, Transport,
};
use pij_core::session_status::{SessionStatusReply, SessionTarget};

/// One recorded interaction, rendered as a stable string so assertions read as
/// the story of what happened: `["put:pij-seat", "tombstone:pij-seat:dead"]`.
pub type Call = String;

#[derive(Default)]
struct Recorder {
    calls: Vec<Call>,
}

impl Recorder {
    fn record(&mut self, call: impl Into<Call>) {
        self.calls.push(call.into());
    }
}

/// The seat roster, in memory.
#[derive(Default)]
pub struct FakeRegistry {
    state: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    seats: BTreeMap<String, SeatDescriptor>,
    seq: u64,
    recorder: Recorder,
}

impl FakeRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-load a seat without recording a call, for arranging a test's world.
    pub fn with_seat(self, descriptor: SeatDescriptor) -> Self {
        self.state
            .lock()
            .expect("fake registry mutex")
            .seats
            .insert(descriptor.id.0.clone(), descriptor);
        self
    }

    /// Every call made, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state
            .lock()
            .expect("fake registry mutex")
            .recorder
            .calls
            .clone()
    }
}

#[async_trait]
impl Registry for FakeRegistry {
    async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
        let mut state = self.state.lock().expect("fake registry mutex");
        state.recorder.record(format!("get:{seat}"));
        Ok(state.seats.get(seat.as_str()).cloned())
    }

    async fn put(&self, descriptor: SeatDescriptor) -> Result<Seq> {
        self.put_reporting(descriptor).await.map(|(seq, _)| seq)
    }

    async fn put_reporting(
        &self,
        descriptor: SeatDescriptor,
    ) -> Result<(Seq, pij_core::ports::PutBinding)> {
        let mut state = self.state.lock().expect("fake registry mutex");
        state.recorder.record(format!("put:{}", descriptor.id));
        state.seq += 1;
        let seq = Seq(state.seq);
        let previous = state.seats.insert(descriptor.id.0.clone(), descriptor);
        Ok((
            seq,
            pij_core::ports::PutBinding {
                inserted: previous.is_none(),
                previous_proc: previous.and_then(|seat| seat.proc),
            },
        ))
    }

    async fn list(&self, filter: SeatFilter) -> Result<Vec<SeatDescriptor>> {
        let mut state = self.state.lock().expect("fake registry mutex");
        state.recorder.record("list".to_string());
        Ok(state
            .seats
            .values()
            .filter(|seat| filter.harness.is_none_or(|h| h == seat.harness))
            .filter(|seat| {
                filter
                    .folder
                    .as_ref()
                    .is_none_or(|folder| folder == &seat.folder)
            })
            .filter(|seat| {
                filter
                    .parent
                    .as_ref()
                    .is_none_or(|parent| Some(parent) == seat.parent.as_ref())
            })
            .cloned()
            .collect())
    }

    async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<Seq> {
        let mut state = self.state.lock().expect("fake registry mutex");
        state.recorder.record(format!("tombstone:{seat}:{reason}"));
        if !state.seats.contains_key(seat.as_str()) {
            return Err(PijError::NoRegistryEntry {
                seat: seat.clone(),
                store: "an in-memory fake registry".to_string(),
            });
        }
        state.seq += 1;
        let seq = state.seq;
        let descriptor = state
            .seats
            .get_mut(seat.as_str())
            .expect("presence checked above");
        descriptor.tombstoned_at = Some(seq);
        descriptor.tombstone_reason = Some(reason.to_string());
        end_turn(descriptor);
        Ok(Seq(seq))
    }

    async fn tombstone_if_unchanged(
        &self,
        expected: SeatDescriptor,
        reason: String,
    ) -> Result<Seq> {
        let mut state = self.state.lock().expect("fake registry mutex");
        state
            .recorder
            .record(format!("tombstone_if_unchanged:{}:{reason}", expected.id));
        if state.seats.get(expected.id.as_str()) != Some(&expected) {
            return Err(PijError::GovernanceRefused {
                code: "E-RS-INCARNATION-CHANGED".to_string(),
                record: expected.id.to_string(),
            });
        }
        state.seq += 1;
        let seq = state.seq;
        let descriptor = state
            .seats
            .get_mut(expected.id.as_str())
            .expect("snapshot matched under lock");
        descriptor.tombstoned_at = Some(seq);
        descriptor.tombstone_reason = Some(reason);
        descriptor.native_extension_delivery = false;
        end_turn(descriptor);
        Ok(Seq(seq))
    }

    async fn set_activity(
        &self,
        seat: &SeatId,
        state: pij_core::model::SystemState,
        _reason: Option<&str>,
    ) -> Result<Option<Seq>> {
        let mut guard = self.state.lock().expect("fake registry mutex");
        guard
            .recorder
            .record(format!("set_activity:{seat}:{}", state.as_str()));
        let applies = guard.seats.get(seat.as_str()).is_some_and(|descriptor| {
            descriptor.tombstoned_at.is_none() && descriptor.state != state
        });
        if !applies {
            return Ok(None);
        }
        guard.seq += 1;
        let seq = guard.seq;
        guard
            .seats
            .get_mut(seat.as_str())
            .expect("presence checked above")
            .state = state;
        Ok(Some(Seq(seq)))
    }
}

/// A retired seat's turn is over: `working` must not outlive it (plan 158).
fn end_turn(descriptor: &mut SeatDescriptor) {
    if descriptor.state == pij_core::model::SystemState::Working {
        descriptor.state = pij_core::model::SystemState::Idle;
    }
}

/// The append-only history, in memory.
#[derive(Default)]
pub struct FakeSpine {
    state: Mutex<SpineState>,
}

#[derive(Default)]
struct SpineState {
    events: Vec<(Seq, Event)>,
    seq: u64,
    append_errors: VecDeque<String>,
    latest_matching_errors: VecDeque<String>,
}

impl FakeSpine {
    /// An empty spine.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many events have been appended.
    pub fn len(&self) -> usize {
        self.state.lock().expect("fake spine mutex").events.len()
    }

    /// Is the spine empty?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Make the next append fail with an adapter error carrying `message`.
    pub fn script_append_error(&self, message: impl Into<String>) {
        self.state
            .lock()
            .expect("fake spine mutex")
            .append_errors
            .push_back(message.into());
    }

    /// Make the next bounded latest-event lookup fail with `message`.
    pub fn script_latest_matching_error(&self, message: impl Into<String>) {
        self.state
            .lock()
            .expect("fake spine mutex")
            .latest_matching_errors
            .push_back(message.into());
    }
}

#[async_trait]
impl Spine for FakeSpine {
    async fn append(&self, mut event: Event) -> Result<Seq> {
        let mut state = self.state.lock().expect("fake spine mutex");
        if let Some(message) = state.append_errors.pop_front() {
            return Err(PijError::Adapter {
                adapter: "fake/spine".to_string(),
                message,
            });
        }
        state.seq += 1;
        let seq = Seq(state.seq);
        event.seq = None;
        state.events.push((seq, event));
        Ok(seq)
    }

    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        let state = self.state.lock().expect("fake spine mutex");
        Ok(state
            .events
            .iter()
            .filter(|(seq, _)| *seq > since)
            .filter(|(_, event)| seat.is_none_or(|s| event.seat.as_ref() == Some(s)))
            .map(|(seq, event)| {
                let mut event = event.clone();
                event.seq = Some(*seq);
                event
            })
            .collect())
    }

    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        if kinds.is_empty() {
            return Err(PijError::Adapter {
                adapter: "fake/spine".to_string(),
                message: "latest_matching requires at least one event kind".to_string(),
            });
        }
        let mut state = self.state.lock().expect("fake spine mutex");
        if let Some(message) = state.latest_matching_errors.pop_front() {
            return Err(PijError::Adapter {
                adapter: "fake/spine".to_string(),
                message,
            });
        }
        Ok(state
            .events
            .iter()
            .rev()
            .find(|(_, event)| {
                event.seat.as_ref() == Some(seat) && kinds.iter().any(|kind| *kind == event.kind)
            })
            .map(|(seq, event)| {
                let mut event = event.clone();
                event.seq = Some(*seq);
                event
            }))
    }

    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        let mut state = self.state.lock().expect("fake spine mutex");
        if let Some(message) = state.latest_matching_errors.pop_front() {
            return Err(PijError::Adapter {
                adapter: "fake/spine".to_string(),
                message,
            });
        }
        for (seq, event) in state
            .events
            .iter()
            .rev()
            .filter(|(_, event)| event.seat.as_ref() == Some(seat) && event.kind == kind)
        {
            let payload: serde_json::Value =
                serde_json::from_str(&event.payload).map_err(|error| PijError::Adapter {
                    adapter: "fake/spine".to_string(),
                    message: format!("invalid event payload for message lookup: {error}"),
                })?;
            if payload["msg_id"].as_str() == Some(msg_id) {
                let mut event = event.clone();
                event.seq = Some(*seq);
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}

/// Spine cost instrument: counts bounded/latest and tail reads independently.
///
/// `fail_on_tail` turns an unbounded hot-path regression into a deterministic
/// error rather than a performance comment.
pub struct CountingSpine {
    inner: FakeSpine,
    latest_calls: AtomicUsize,
    tail_calls: AtomicUsize,
    fail_on_tail: bool,
}

impl CountingSpine {
    /// Empty instrument that refuses every `tail` call.
    pub fn failing_on_tail() -> Self {
        Self {
            inner: FakeSpine::new(),
            latest_calls: AtomicUsize::new(0),
            tail_calls: AtomicUsize::new(0),
            fail_on_tail: true,
        }
    }

    /// Number of bounded latest-fact reads.
    pub fn latest_calls(&self) -> usize {
        self.latest_calls.load(Ordering::Relaxed)
    }

    /// Number of unbounded tail reads attempted.
    pub fn tail_calls(&self) -> usize {
        self.tail_calls.load(Ordering::Relaxed)
    }

    /// Recorded events for `seat`, without exercising the measured port calls.
    pub fn recorded_for(&self, seat: &SeatId) -> Vec<Event> {
        self.inner
            .state
            .lock()
            .expect("fake spine mutex")
            .events
            .iter()
            .filter(|(_, event)| event.seat.as_ref() == Some(seat))
            .map(|(seq, event)| {
                let mut event = event.clone();
                event.seq = Some(*seq);
                event
            })
            .collect()
    }
}

#[async_trait]
impl Spine for CountingSpine {
    async fn append(&self, event: Event) -> Result<Seq> {
        self.inner.append(event).await
    }

    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        self.tail_calls.fetch_add(1, Ordering::Relaxed);
        if self.fail_on_tail {
            return Err(PijError::Adapter {
                adapter: "testkit/counting-spine".to_string(),
                message: "unpark hot path performed unbounded Spine::tail".to_string(),
            });
        }
        self.inner.tail(seat, since).await
    }

    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        self.latest_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.latest_matching(seat, kinds).await
    }

    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        self.latest_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.latest_matching_message(seat, kind, msg_id).await
    }
}

/// The job queue, in memory — dedupe and serial keys included, because a fake
/// that drops the semantics is a fake of the wrong thing.
pub struct FakeQueue {
    state: Mutex<QueueState>,
    delivered_id_capacity: usize,
}

#[derive(Default)]
struct QueueState {
    live: Vec<(JobId, Job)>,
    claimed: HashMap<u64, (Job, String)>,
    claimed_at: HashMap<u64, Duration>,
    lease_expirations: HashMap<u64, u32>,
    parked: Vec<pij_core::ports::ParkedDelivery>,
    acked: Vec<(JobId, Outcome)>,
    completed: HashMap<u64, (Job, &'static str)>,
    retried: Vec<(JobId, Duration)>,
    attempts: HashMap<u64, u32>,
    not_before: HashMap<u64, Duration>,
    deferrals: HashMap<u64, (pij_core::model::DeliveryDeferral, u64, u64)>,
    now: Duration,
    next_id: u64,
    delivered: Vec<(String, String, DeliveryOrigin)>,
    /// Held FYIs with their state: `pending`, `delivered`.
    fyis: Vec<(pij_core::fyi::HeldFyi, &'static str)>,
    /// When each delivered FYI was claimed, by id (plan 159's `fyi-read`).
    fyi_claimed_at: HashMap<String, u64>,
}

impl FakeQueue {
    /// An empty queue with a configured per-recipient delivered-id bound.
    ///
    /// # Errors
    /// [`PijError::Adapter`] when `delivered_id_capacity` is zero.
    pub fn new(delivered_id_capacity: usize) -> Result<Self> {
        if delivered_id_capacity == 0 {
            return Err(PijError::Adapter {
                adapter: "testkit/fake-queue".to_string(),
                message: "delivered_id_capacity must be greater than zero".to_string(),
            });
        }
        Ok(Self {
            state: Mutex::new(QueueState::default()),
            delivered_id_capacity,
        })
    }

    /// How many live (unclaimed) rows exist — the assertion surface for dedupe.
    pub fn live_len(&self) -> usize {
        self.state.lock().expect("fake queue mutex").live.len()
    }

    /// The payloads of the live (unclaimed) rows, oldest first.
    pub fn live_payloads(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("fake queue mutex")
            .live
            .iter()
            .map(|(_, job)| job.payload.clone())
            .collect()
    }

    /// Everything acked, in order.
    pub fn acked(&self) -> Vec<(JobId, Outcome)> {
        self.state.lock().expect("fake queue mutex").acked.clone()
    }

    /// Advance the fake clock, making due retries claimable again.
    ///
    /// Injected rather than real: a test that sleeps to observe backoff is a test
    /// that is slow AND flaky, and it proves the sleep rather than the schedule.
    pub fn advance(&self, by: Duration) {
        let mut state = self.state.lock().expect("fake queue mutex");
        state.now += by;
    }

    /// Every retry, in order, with the delay each asked for.
    pub fn retried(&self) -> Vec<(JobId, Duration)> {
        self.state.lock().expect("fake queue mutex").retried.clone()
    }

    /// How many times this job has been retried. The counter lives HERE and in
    /// SQLite, and nowhere else — see `Queue::retry` (R4-AMEND-1).
    pub fn attempts(&self, job: JobId) -> u32 {
        self.state
            .lock()
            .expect("fake queue mutex")
            .attempts
            .get(&job.0)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl Queue for FakeQueue {
    async fn enqueue(&self, job: Job) -> Result<JobId> {
        let mut state = self.state.lock().expect("fake queue mutex");
        // Dedupe over LIVE rows only: an acked job's key is free again, which is
        // what makes "one row per burst" different from "one row for ever".
        //
        // Keyed by (KIND, dedupe_key), matching the schema-7 index. Keyed by the
        // dedupe_key alone — as this fake was — one caller-chosen `msg_id` sent to
        // two different recipients collapses to one row, and every service test
        // above it agrees with a queue that no longer behaves that way.
        if let Some((id, _)) = state
            .live
            .iter()
            .map(|(id, job)| (*id, job))
            .chain(state.claimed.iter().map(|(id, (job, _))| (JobId(*id), job)))
            .find(|(_, live)| live.dedupe_key == job.dedupe_key && live.kind == job.kind)
        {
            return Ok(id);
        }
        state.next_id += 1;
        let id = JobId(state.next_id);
        state.live.push((id, job));
        Ok(id)
    }

    async fn enqueue_delivery(&self, job: Job) -> Result<DeliveryEnqueue> {
        require_delivery_job(&job)?;
        let mut state = self.state.lock().expect("fake queue mutex");
        if let Some((_, _, origin)) = state.delivered.iter().find(|(recipient, msg_id, _)| {
            recipient == &job.serial_key && msg_id == &job.dedupe_key
        }) {
            return Ok(DeliveryEnqueue::AlreadyDelivered(*origin));
        }
        if let Some(id) = state
            .live
            .iter()
            .map(|(id, job)| (*id, job))
            .chain(state.claimed.iter().map(|(id, (job, _))| (JobId(*id), job)))
            .find(|(_, live)| live.dedupe_key == job.dedupe_key && live.kind == job.kind)
            .map(|(id, _)| id)
        {
            let not_before = state.not_before.get(&id.0).copied().unwrap_or(state.now);
            return Ok(DeliveryEnqueue::Queued {
                job_id: id,
                not_before_ms: u64::try_from(not_before.as_millis()).unwrap_or(u64::MAX),
            });
        }
        state.next_id += 1;
        let id = JobId(state.next_id);
        let not_before = state.now;
        state.live.push((id, job));
        state.not_before.insert(id.0, not_before);
        Ok(DeliveryEnqueue::Queued {
            job_id: id,
            not_before_ms: u64::try_from(not_before.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn claim(&self, kinds: &[String], worker: &str) -> Result<Option<(JobId, Job)>> {
        let mut state = self.state.lock().expect("fake queue mutex");
        let busy: Vec<String> = state
            .claimed
            .values()
            .map(|(job, _)| job.serial_key.clone())
            .collect();
        // not-before eligibility, so a backoff test can ADVANCE A CLOCK instead of
        // sleeping. The real queue gates on `unixepoch()`; a fake that ignored the
        // delay would let "the same semantics" be false in the one direction that
        // matters — a retried row claimable instantly.
        let now = state.now;
        let ready: Vec<u64> = state
            .not_before
            .iter()
            .filter(|(_, at)| **at > now)
            .map(|(id, _)| *id)
            .collect();
        let Some(index) = state.live.iter().position(|(id, job)| {
            kinds.contains(&job.kind) && !busy.contains(&job.serial_key) && !ready.contains(&id.0)
        }) else {
            return Ok(None);
        };
        let (id, job) = state.live.remove(index);
        state
            .claimed
            .insert(id.0, (job.clone(), worker.to_string()));
        state.claimed_at.insert(id.0, now);
        Ok(Some((id, job)))
    }

    async fn claim_extension(
        &self,
        kinds: &[String],
        worker: &str,
        lease: pij_core::ports::ExtensionLease,
        at: u64,
        spine: &dyn Spine,
        recovery_allowed: bool,
    ) -> Result<pij_core::ports::ExtensionClaim> {
        let expired: Vec<_> = {
            let state = self.state.lock().expect("fake queue mutex");
            state
                .claimed
                .iter()
                .filter(|(id, (job, _))| {
                    kinds.contains(&job.kind)
                        && job.kind == format!("delivery:{}", job.serial_key)
                        && serde_json::from_str::<serde_json::Value>(&job.payload)
                            .ok()
                            .is_none_or(|body| {
                                body.get("command").is_none_or(serde_json::Value::is_null)
                            })
                        && state.claimed_at.get(id).is_some_and(|started| {
                            state.now.saturating_sub(*started) >= Duration::from_secs(lease.seconds)
                        })
                })
                .map(|(id, (job, _))| {
                    (
                        *id,
                        job.clone(),
                        state.lease_expirations.get(id).copied().unwrap_or(0) + 1,
                    )
                })
                .collect()
        };
        let mut events = Vec::new();
        let mut parked = Vec::new();
        // Like PublishedFakeRegistry: append first, then infallible mutation.
        // The composed publisher retains its ordering/native guard throughout.
        for (id, job, count) in &expired {
            if *count >= 3 && !(lease.renew_working && job.serial_key == worker) {
                pij_core::delivery::require_recovery_authority(recovery_allowed)?;
                let evidence = pij_core::ports::ParkingEvidence {
                    outcome: pij_core::model::DeliveryFailure::LeaseExhausted,
                    reason: "three extension claim leases expired",
                    at,
                };
                for mut event in pij_core::delivery::parked_events(JobId(*id), job, &evidence)? {
                    event.seq = Some(spine.append(event.clone()).await?);
                    events.push(event);
                }
            }
        }
        {
            let mut state = self.state.lock().expect("fake queue mutex");
            for (id, mut job, count) in expired {
                if lease.renew_working && job.serial_key == worker {
                    let now = state.now;
                    state.claimed_at.insert(id, now);
                    continue;
                }
                state.claimed.remove(&id);
                state.lease_expirations.insert(id, count);
                if count >= 3 {
                    let row = pij_core::ports::ParkedDelivery {
                        job_id: JobId(id),
                        job,
                        outcome: pij_core::model::DeliveryFailure::LeaseExhausted,
                    };
                    state.acked.push((
                        JobId(id),
                        Outcome::Failed {
                            reason: row.outcome.as_str().into(),
                        },
                    ));
                    state.parked.push(row.clone());
                    parked.push(row);
                } else {
                    job.attempt += 1;
                    state.attempts.insert(id, job.attempt);
                    state.live.push((JobId(id), job));
                }
                state.claimed_at.remove(&id);
            }
            state.live.sort_by_key(|(id, _)| *id);
        }
        Ok(pij_core::ports::ExtensionClaim {
            claimed: self.claim(kinds, worker).await?,
            parked,
            events,
        })
    }

    async fn peek_parked(&self, kinds: &[String]) -> Result<Vec<pij_core::ports::ParkedDelivery>> {
        Ok(self
            .state
            .lock()
            .expect("fake queue mutex")
            .parked
            .iter()
            .filter(|row| kinds.contains(&row.job.kind))
            .cloned()
            .collect())
    }

    async fn recover_native_delivery(&self, id: JobId, recipient: &SeatId) -> Result<bool> {
        let mut state = self.state.lock().expect("fake queue mutex");
        let Some(index) = state.parked.iter().position(|row| {
            row.job_id == id
                && row.job.serial_key == recipient.as_str()
                && row.outcome == pij_core::model::DeliveryFailure::NativeReceiverUnavailable
        }) else {
            return Ok(false);
        };
        let key = &state.parked[index].job.dedupe_key;
        if state.live.iter().any(|(_, job)| &job.dedupe_key == key)
            || state
                .claimed
                .values()
                .any(|(job, _)| &job.dedupe_key == key)
            || state
                .delivered
                .iter()
                .any(|(seat, msg, _)| seat == recipient.as_str() && msg == key)
        {
            return Ok(false);
        }
        let mut row = state.parked.remove(index);
        row.job.attempt = row.job.attempt.saturating_add(1);
        state.attempts.insert(id.0, row.job.attempt);
        state.not_before.remove(&id.0);
        state.live.push((id, row.job));
        state.live.sort_by_key(|(id, _)| *id);
        Ok(true)
    }

    async fn park_delivery(
        &self,
        id: JobId,
        recipient: &SeatId,
        attempt: u32,
        evidence: &pij_core::ports::ParkingEvidence<'_>,
        spine: &dyn Spine,
    ) -> Result<(Option<Job>, Vec<Event>)> {
        let job = {
            let state = self.state.lock().expect("fake queue mutex");
            if evidence.outcome == pij_core::model::DeliveryFailure::NativeReceiverUnavailable
                && state.claimed.get(&id.0).is_some_and(|(_, worker)| {
                    worker == &format!("native-cli:{recipient}")
                        && state.claimed_at.get(&id.0).is_some_and(|at| {
                            state.now.saturating_sub(*at) < Duration::from_secs(300)
                        })
                })
            {
                return Ok((None, Vec::new()));
            }
            let job = state.claimed.get(&id.0).map(|(job, _)| job).or_else(|| {
                (evidence.outcome == pij_core::model::DeliveryFailure::NativeReceiverUnavailable)
                    .then(|| state.live.iter().find(|(pending, _)| *pending == id))
                    .flatten()
                    .map(|(_, job)| job)
            });
            let valid = job.is_some_and(|job| {
                job.serial_key == recipient.as_str()
                    && job.attempt == attempt
                    && job.kind == format!("delivery:{recipient}")
                    && serde_json::from_str::<serde_json::Value>(&job.payload)
                        .ok()
                        .is_some_and(|body| {
                            body.get("command").is_none_or(serde_json::Value::is_null)
                        })
            }) && !state
                .live
                .iter()
                .any(|(earlier, job)| earlier < &id && job.serial_key == recipient.as_str())
                && !state.claimed.iter().any(|(earlier, (job, _))| {
                    *earlier < id.0 && job.serial_key == recipient.as_str()
                });
            if !valid {
                return Ok((None, Vec::new()));
            }
            job.expect("validated live head").clone()
        };
        let mut events = pij_core::delivery::parked_events(id, &job, evidence)?;
        for event in &mut events {
            event.seq = Some(spine.append(event.clone()).await?);
        }
        let mut state = self.state.lock().expect("fake queue mutex");
        state.claimed.remove(&id.0);
        state.live.retain(|(pending, _)| *pending != id);
        state.acked.push((
            id,
            Outcome::Failed {
                reason: evidence.outcome.as_str().into(),
            },
        ));
        state.parked.push(pij_core::ports::ParkedDelivery {
            job_id: id,
            job: job.clone(),
            outcome: evidence.outcome,
        });
        Ok((Some(job), events))
    }

    async fn peek(&self, kinds: &[String]) -> Result<Option<(JobId, Job)>> {
        let state = self.state.lock().expect("fake queue mutex");
        let pending = state
            .live
            .iter()
            .filter(|(_, job)| kinds.contains(&job.kind))
            .map(|(id, job)| (*id, job.clone()));
        let claimed = state
            .claimed
            .iter()
            .filter(|(_, (job, _))| kinds.contains(&job.kind))
            .map(|(id, (job, _))| (JobId(*id), job.clone()));
        Ok(pending.chain(claimed).min_by_key(|(id, _)| id.0))
    }

    async fn claimed_delivery(&self, job: JobId) -> Result<Option<Job>> {
        let state = self.state.lock().expect("fake queue mutex");
        Ok(state
            .claimed
            .get(&job.0)
            .filter(|(claimed, _)| {
                claimed.kind.strip_prefix("delivery:") == Some(claimed.serial_key.as_str())
            })
            .map(|(claimed, _)| claimed.clone()))
    }

    async fn terminal_delivery_state(
        &self,
        job: JobId,
        recipient: &SeatId,
    ) -> Result<Option<&'static str>> {
        let state = self.state.lock().expect("fake queue mutex");
        let terminal = state
            .completed
            .get(&job.0)
            .map(|(job, status)| (job, *status))
            .or_else(|| {
                state
                    .parked
                    .iter()
                    .find(|row| row.job_id == job)
                    .map(|row| (&row.job, "failed"))
            });
        let Some((body, status)) = terminal else {
            return Ok(None);
        };
        let valid = body.kind.strip_prefix("delivery:") == Some(recipient.as_str())
            && body.serial_key == recipient.as_str()
            && serde_json::from_str::<serde_json::Value>(&body.payload)
                .ok()
                .is_some_and(|value| {
                    value.get("to").and_then(serde_json::Value::as_str) == Some(recipient.as_str())
                        && value.get("command").is_none_or(serde_json::Value::is_null)
                });
        Ok(valid.then_some(status))
    }

    async fn heartbeat_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
    ) -> Result<bool> {
        let mut state = self.state.lock().expect("fake queue mutex");
        let valid = state.claimed.get(&job.0).is_some_and(|(claimed, _)| {
            claimed.kind.strip_prefix("delivery:") == Some(recipient.as_str())
                && claimed.serial_key == recipient.as_str()
                && claimed.attempt == attempt
                && serde_json::from_str::<serde_json::Value>(&claimed.payload)
                    .ok()
                    .is_some_and(|body| {
                        body.get("to").and_then(serde_json::Value::as_str)
                            == Some(recipient.as_str())
                            && body.get("command").is_none_or(serde_json::Value::is_null)
                    })
        });
        if valid {
            let now = state.now;
            state.claimed_at.insert(job.0, now);
        }
        Ok(valid)
    }

    async fn ack(&self, job: JobId, outcome: Outcome) -> Result<()> {
        let mut state = self.state.lock().expect("fake queue mutex");
        // REFUSE an unclaimed or double ack, as SqliteQueue does. This was the
        // last loose one: `retry` and `ack_delivery` both validate the claim and
        // `ack` did not, so the fake accepted a call the real adapter rejects and
        // no instrument compared them (review F6). A fake is a claim about the
        // real adapter, and this one was false in the direction that hides bugs.
        let Some((completed, _)) = state.claimed.remove(&job.0) else {
            return Err(PijError::Adapter {
                adapter: "testkit/fake-queue".to_string(),
                message: format!("job {} is not running, so it cannot be acked", job.0),
            });
        };
        let status = match &outcome {
            Outcome::Done => "done",
            Outcome::Failed { .. } => "failed",
        };
        state.completed.insert(job.0, (completed, status));
        state.acked.push((job, outcome));
        Ok(())
    }

    /// R4-AMEND-4: claim-or-report, with the same prune the real adapter applies,
    /// so a capacity test gets the same answer from either implementation.
    async fn note_delivered(
        &self,
        recipient: &SeatId,
        msg_id: &str,
        origin: DeliveryOrigin,
    ) -> Result<Option<DeliveryOrigin>> {
        let mut state = self.state.lock().expect("fake queue mutex");
        if let Some((_, _, existing)) = state
            .delivered
            .iter()
            .find(|(seat, id, _)| seat == recipient.as_str() && id == msg_id)
        {
            return Ok(Some(*existing));
        }
        state
            .delivered
            .push((recipient.as_str().to_string(), msg_id.to_string(), origin));
        // Same prune the real adapter applies, oldest first, so a capacity test
        // gets the same answer from either implementation.
        let seat = recipient.as_str().to_string();
        let count = state
            .delivered
            .iter()
            .filter(|(other, _, _)| other == &seat)
            .count();
        let mut remove = count.saturating_sub(self.delivered_id_capacity);
        state.delivered.retain(|(other, _, _)| {
            if other == &seat && remove > 0 {
                remove -= 1;
                false
            } else {
                true
            }
        });
        Ok(None)
    }

    async fn admitted(&self, recipient: &SeatId, msg_id: &str) -> Result<bool> {
        let state = self.state.lock().expect("fake queue mutex");
        let kind = format!("delivery:{}", recipient.as_str());
        let is_it = |job: &Job| job.kind == kind && job.dedupe_key == msg_id;
        Ok(state
            .delivered
            .iter()
            .any(|(seat, id, _)| seat == recipient.as_str() && id == msg_id)
            || state.live.iter().any(|(_, job)| is_it(job))
            || state.claimed.values().any(|(job, _)| is_it(job))
            || state.completed.values().any(|(job, _)| is_it(job)))
    }

    async fn forget_delivered(&self, recipient: &SeatId, msg_id: &str) -> Result<()> {
        let mut state = self.state.lock().expect("fake queue mutex");
        state
            .delivered
            .retain(|(seat, id, _)| !(seat == recipient.as_str() && id == msg_id));
        Ok(())
    }

    async fn ack_delivery(&self, job: JobId, origin: DeliveryOrigin) -> Result<DeliveryAck> {
        let mut state = self.state.lock().expect("fake queue mutex");
        let Some((claimed, _worker)) = state.claimed.remove(&job.0) else {
            return Err(PijError::Adapter {
                adapter: "testkit/fake-queue".to_string(),
                message: format!(
                    "job {} is not running — it was already acked, or never claimed",
                    job.0
                ),
            });
        };
        require_delivery_job(&claimed)?;
        if !state.delivered.iter().any(|(recipient, msg_id, _)| {
            recipient == &claimed.serial_key && msg_id == &claimed.dedupe_key
        }) {
            state.delivered.push((
                claimed.serial_key.clone(),
                claimed.dedupe_key.clone(),
                origin,
            ));
        }
        let recipient_count = state
            .delivered
            .iter()
            .filter(|(recipient, _, _)| recipient == &claimed.serial_key)
            .count();
        let mut remove = recipient_count.saturating_sub(self.delivered_id_capacity);
        state.delivered.retain(|(recipient, _, _)| {
            if recipient == &claimed.serial_key && remove > 0 {
                remove -= 1;
                false
            } else {
                true
            }
        });
        state.acked.push((job, Outcome::Done));
        let ack = DeliveryAck {
            recipient: SeatId(claimed.serial_key.clone()),
            msg_id: claimed.dedupe_key.clone(),
            origin,
        };
        state.completed.insert(job.0, (claimed, "done"));
        Ok(ack)
    }

    /// R4-AMEND-1. The fake counts attempts too, because a caller that reads
    /// `attempts()` must get the same answer from either implementation — a fake
    /// that forgot the counter would let a one-writer test pass against nothing.
    async fn retry(&self, job: JobId, delay: Duration) -> Result<()> {
        let mut state = self.state.lock().expect("fake queue mutex");
        // RETURN THE BODY TO THE QUEUE. The first version of this fake removed
        // the claim, counted the attempt, recorded the delay — and dropped the
        // job. A retry test against it would have observed perfect bookkeeping
        // while the message was silently lost, which is the worst shape a fake
        // can have: it certifies the thing it destroys. Found by two units within
        // minutes of the seam landing.
        let Some((claimed, _worker)) = state.claimed.remove(&job.0) else {
            // Same contract as SqliteQueue: retrying a job nobody claimed means a
            // worker believes it holds work the queue does not think is running.
            return Err(PijError::Adapter {
                adapter: "testkit/fake-queue".to_string(),
                message: format!("job {} is not running, so it cannot be retried", job.0),
            });
        };
        let attempt = state.attempts.entry(job.0).or_insert(0);
        *attempt += 1;
        let attempt = *attempt;
        let not_before = state.now + delay;
        state.live.push((job, Job { attempt, ..claimed }));
        state.not_before.insert(job.0, not_before);
        state.retried.push((job, delay));
        Ok(())
    }

    async fn record_delivery_deferral(
        &self,
        job: JobId,
        reason: &str,
        draft_sha: Option<&str>,
        at: u64,
        spine: &dyn Spine,
    ) -> Result<Vec<Event>> {
        let (deferral, seat, last_event, publish, reason_changes) = {
            let state = self.state.lock().expect("fake queue mutex");
            let row = state.claimed.get(&job.0).map(|(row, _)| row).or_else(|| {
                state
                    .live
                    .iter()
                    .find(|(id, _)| *id == job)
                    .map(|(_, row)| row)
            });
            let Some(row) = row.filter(|row| row.kind == format!("delivery:{}", row.serial_key))
            else {
                return Ok(Vec::new());
            };
            let previous = state.deferrals.get(&job.0);
            let publish = previous.is_none_or(|(_, last, _)| {
                at.saturating_sub(*last) >= pij_core::delivery::DEFERRAL_EVENT_INTERVAL_MS
            });
            let reason_changes = previous.map_or(0, |(previous, _, changes)| {
                changes + u64::from(previous.reason != reason)
            });
            (
                pij_core::model::DeliveryDeferral {
                    job_id: job,
                    msg_id: row.dedupe_key.clone(),
                    reason: reason.into(),
                    count: previous.map_or(1, |(previous, _, _)| previous.count + 1),
                    since_ms: previous.map_or(at, |(previous, _, _)| previous.since_ms),
                },
                SeatId(row.serial_key.clone()),
                previous.map_or(at, |(_, last, _)| *last),
                publish,
                reason_changes,
            )
        };
        let mut events = Vec::new();
        if publish {
            let mut event = pij_core::delivery::delivery_deferral_event(
                &deferral,
                &seat,
                draft_sha,
                at,
                reason_changes,
            );
            event.seq = Some(spine.append(event.clone()).await?);
            events.push(event);
        }
        self.state
            .lock()
            .expect("fake queue mutex")
            .deferrals
            .insert(
                job.0,
                (
                    deferral,
                    if publish { at } else { last_event },
                    if publish { 0 } else { reason_changes },
                ),
            );
        Ok(events)
    }

    async fn delivery_deferrals(
        &self,
        recipient: &SeatId,
    ) -> Result<Vec<pij_core::model::DeliveryDeferral>> {
        let state = self.state.lock().expect("fake queue mutex");
        let mut deferrals: Vec<_> = state
            .live
            .iter()
            .map(|(id, row)| (*id, row))
            .chain(state.claimed.iter().map(|(id, (row, _))| (JobId(*id), row)))
            .filter(|(_, row)| {
                row.serial_key == recipient.as_str()
                    && row.kind == format!("delivery:{}", row.serial_key)
            })
            .filter_map(|(id, _)| state.deferrals.get(&id.0).map(|(fact, _, _)| fact.clone()))
            .collect();
        deferrals.sort_by_key(|fact| fact.job_id);
        Ok(deferrals)
    }

    async fn defer(&self, job: JobId, delay: Duration) -> Result<DeferOutcome> {
        let mut state = self.state.lock().expect("fake queue mutex");
        let live = state.claimed.get(&job.0).map(|(row, _)| row).or_else(|| {
            state
                .live
                .iter()
                .find(|(id, _)| *id == job)
                .map(|(_, row)| row)
        });
        let Some(live) = live else {
            let reason = if state.acked.iter().any(|(id, _)| *id == job) {
                DeferNoopReason::Terminal
            } else {
                DeferNoopReason::Absent
            };
            return Ok(DeferOutcome::NotLive { reason });
        };
        require_delivery_job(live)?;
        let outcome = DeferOutcome::Deferred {
            recipient: SeatId(live.serial_key.clone()),
            msg_id: live.dedupe_key.clone(),
        };
        // WHOLE SECONDS, exactly as the store computes them (store/queue.rs:421-435):
        // SQLite eligibility compares `unixepoch()`, so the real deadline is the
        // absolute one rounded UP to a whole second, and zero stays immediately
        // eligible. Keeping full Duration precision here made the fake PERMISSIVE
        // in the release-early direction — `defer(job, 10ms)` was claimable 10 ms
        // later against the fake and up to a second later against the store — and
        // a fake that releases earlier than production turns every green test
        // built on sub-second delays into a false negative (reviewer f3).
        let not_before = state
            .now
            .checked_add(delay)
            .and_then(|deadline| {
                // checked_add, exactly as the store does it (queue.rs:429). Bare
                // addition panics at the fake's own clock origin: state.now == 0
                // plus Duration::MAX gives as_secs() == u64::MAX with a non-zero
                // subsec, and the round-up overflows. The overflow test only
                // misses it because it advances the clock by a nanosecond first,
                // and that test's own words are "overflow is an error, never a
                // panic" (reviewer f3.2).
                deadline
                    .as_secs()
                    .checked_add(u64::from(!delay.is_zero() && deadline.subsec_nanos() != 0))
                    .map(Duration::from_secs)
            })
            .filter(|deadline| i64::try_from(deadline.as_secs()).is_ok())
            .ok_or_else(|| PijError::Adapter {
                adapter: "testkit/fake-queue".to_string(),
                message: "deferral deadline exceeds the supported timestamp range".to_string(),
            })?;
        if let Some((row, _worker)) = state.claimed.remove(&job.0) {
            // Restoring a claim keeps its original FIFO position, unlike retry.
            let index = state
                .live
                .iter()
                .position(|(id, _)| id.0 > job.0)
                .unwrap_or(state.live.len());
            state.live.insert(index, (job, row));
        }
        state.not_before.insert(job.0, not_before);
        Ok(outcome)
    }

    async fn release_deferred(&self, job: JobId) -> Result<ReleaseOutcome> {
        let mut state = self.state.lock().expect("fake queue mutex");
        if let Some((running, _worker)) = state.claimed.get(&job.0) {
            require_delivery_job(running)?;
            return Ok(ReleaseOutcome::NotDeferred);
        }
        let Some((_, pending)) = state.live.iter().find(|(id, _)| *id == job) else {
            let reason = if state.acked.iter().any(|(id, _)| *id == job) {
                DeferNoopReason::Terminal
            } else {
                DeferNoopReason::Absent
            };
            return Ok(ReleaseOutcome::NotLive { reason });
        };
        require_delivery_job(pending)?;
        let outcome = ReleaseOutcome::Released {
            recipient: SeatId(pending.serial_key.clone()),
            msg_id: pending.dedupe_key.clone(),
        };
        let now = state.now;
        state.not_before.insert(job.0, now);
        Ok(outcome)
    }

    async fn hold_fyi(
        &self,
        fyi: &pij_core::fyi::HeldFyi,
        spine: &dyn Spine,
    ) -> Result<Vec<Event>> {
        if self
            .state
            .lock()
            .expect("fake queue mutex")
            .fyis
            .iter()
            // Identity is (origin machine, msg_id), as in the store.
            .any(|(held, _)| held.id == fyi.id && held.from_machine == fyi.from_machine)
        {
            return Ok(Vec::new());
        }
        let mut event = pij_core::fyi::held_event(fyi);
        event.seq = Some(spine.append(event.clone()).await?);
        self.state
            .lock()
            .expect("fake queue mutex")
            .fyis
            .push((fyi.clone(), "pending"));
        Ok(vec![event])
    }

    async fn claim_fyis(
        &self,
        recipient: &SeatId,
        via: &str,
        at: u64,
        spine: &dyn Spine,
    ) -> Result<(Vec<pij_core::fyi::HeldFyi>, Vec<Event>)> {
        // Transition under the lock first, so a racing claim cannot see these rows.
        let mut claimed: Vec<pij_core::fyi::HeldFyi> = {
            let mut state = self.state.lock().expect("fake queue mutex");
            let claimed: Vec<pij_core::fyi::HeldFyi> = state
                .fyis
                .iter_mut()
                .filter(|(held, fyi_state)| held.recipient == *recipient && *fyi_state == "pending")
                .map(|(held, fyi_state)| {
                    *fyi_state = "delivered";
                    held.clone()
                })
                .collect();
            for held in &claimed {
                state.fyi_claimed_at.insert(held.id.clone(), at);
            }
            claimed
        };
        if claimed.is_empty() {
            return Ok((claimed, Vec::new()));
        }
        claimed.sort_by(|a, b| a.held_at_ms.cmp(&b.held_at_ms).then(a.id.cmp(&b.id)));
        let ids: Vec<String> = claimed.iter().map(|fyi| fyi.id.clone()).collect();
        let mut event = pij_core::fyi::delivered_event(recipient, &ids, via, at);
        match spine.append(event.clone()).await {
            Ok(seq) => event.seq = Some(seq),
            Err(error) => {
                self.set_fyi_state(&ids, "pending");
                return Err(error);
            }
        }
        Ok((claimed, vec![event]))
    }

    async fn enqueue_delivery_carrying_fyis(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        spine: &dyn Spine,
    ) -> Result<(DeliveryEnqueue, Vec<Event>)> {
        require_delivery_job(&job)?;
        // A delivered or still-queued message id creates no row, so it claims nothing.
        let known = {
            let state = self.state.lock().expect("fake queue mutex");
            state.delivered.iter().any(|(recipient, msg_id, _)| {
                recipient == &job.serial_key && msg_id == &job.dedupe_key
            }) || state
                .live
                .iter()
                .map(|(_, live)| live)
                .chain(state.claimed.values().map(|(live, _)| live))
                .any(|live| live.dedupe_key == job.dedupe_key && live.kind == job.kind)
        };
        if known {
            return Ok((self.enqueue_delivery(job).await?, Vec::new()));
        }
        let recipient = SeatId(job.serial_key.clone());
        let (fyis, events) = self.claim_fyis(&recipient, via, at, spine).await?;
        let mut job = job;
        if !fyis.is_empty() {
            match attach(&job.payload, &fyis) {
                Ok(payload) => job.payload = payload,
                Err(error) => {
                    let ids: Vec<String> = fyis.iter().map(|fyi| fyi.id.clone()).collect();
                    self.set_fyi_state(&ids, "pending");
                    return Err(error);
                }
            }
        }
        Ok((self.enqueue_delivery(job).await?, events))
    }

    async fn enqueue_fyi_flush(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        spine: &dyn Spine,
    ) -> Result<(Option<DeliveryEnqueue>, Vec<Event>)> {
        require_delivery_job(&job)?;
        let recipient = SeatId(job.serial_key.clone());
        if self.pending_fyi_count(&recipient).await? == 0 {
            return Ok((None, Vec::new()));
        }
        let (enqueued, events) = self
            .enqueue_delivery_carrying_fyis(job, via, at, attach, spine)
            .await?;
        Ok((Some(enqueued), events))
    }

    async fn pending_fyi_count(&self, recipient: &SeatId) -> Result<u64> {
        Ok(self
            .state
            .lock()
            .expect("fake queue mutex")
            .fyis
            .iter()
            .filter(|(held, fyi_state)| held.recipient == *recipient && *fyi_state == "pending")
            .count() as u64)
    }

    async fn read_claimed_fyis(
        &self,
        recipient: &SeatId,
        claimed_at_ms: u64,
    ) -> Result<Vec<pij_core::fyi::HeldFyi>> {
        let state = self.state.lock().expect("fake queue mutex");
        let mut read: Vec<pij_core::fyi::HeldFyi> = state
            .fyis
            .iter()
            .filter(|(held, fyi_state)| {
                held.recipient == *recipient
                    && *fyi_state == "delivered"
                    && state.fyi_claimed_at.get(&held.id) == Some(&claimed_at_ms)
            })
            .map(|(held, _)| held.clone())
            .collect();
        read.sort_by(|a, b| a.held_at_ms.cmp(&b.held_at_ms).then(a.id.cmp(&b.id)));
        Ok(read)
    }
}

impl FakeQueue {
    fn set_fyi_state(&self, ids: &[String], to: &'static str) {
        for (held, fyi_state) in &mut self.state.lock().expect("fake queue mutex").fyis {
            if ids.contains(&held.id) {
                *fyi_state = to;
            }
        }
    }
}

fn require_delivery_job(job: &Job) -> Result<()> {
    let expected = format!("delivery:{}", job.serial_key);
    if job.kind == expected {
        return Ok(());
    }
    Err(PijError::Adapter {
        adapter: "testkit/fake-queue".to_string(),
        message: format!(
            "delivery operation requires kind {expected}, got {}",
            job.kind
        ),
    })
}

/// A transport whose reachability and outcome are scripted.
pub struct FakeTransport {
    name: String,
    state: Mutex<TransportState>,
}

#[derive(Default)]
struct TransportState {
    /// Fail the NEXT deliver call, once. Models an ambiguous transport failure:
    /// the caller learns the injection did not happen, which is the case a
    /// claim-before-inject design must compensate for.
    fail_next_deliver: bool,
    fail_next_reachability: bool,
    reachable: bool,
    outcome: Option<DeliveryOutcome>,
    delivered: Vec<Msg>,
    recorder: Recorder,
}

impl FakeTransport {
    /// A transport that can reach every seat and delivers.
    pub fn reachable() -> Self {
        FakeTransport {
            name: "fake".to_string(),
            state: Mutex::new(TransportState {
                reachable: true,
                ..TransportState::default()
            }),
        }
    }

    /// A transport that can reach nobody — the pre-bind and busy cases, which is
    /// where TS lost messages while reporting success.
    pub fn unreachable() -> Self {
        FakeTransport {
            name: "fake".to_string(),
            state: Mutex::new(TransportState::default()),
        }
    }

    /// Force the outcome `deliver` reports, whatever reachability says.
    /// Fail the next `deliver` call once, then behave normally.
    #[must_use]
    pub fn script_deliver_error(self) -> Self {
        self.state
            .lock()
            .expect("fake transport mutex")
            .fail_next_deliver = true;
        self
    }

    /// Fail the next reachability probe once, then resume normal answers.
    #[must_use]
    pub fn script_reachability_error(self) -> Self {
        self.state
            .lock()
            .expect("fake transport mutex")
            .fail_next_reachability = true;
        self
    }

    /// Script the outcome every `deliver` returns.
    #[must_use]
    pub fn script_outcome(self, outcome: DeliveryOutcome) -> Self {
        self.state.lock().expect("fake transport mutex").outcome = Some(outcome);
        self
    }

    /// Change (or, with `None`, clear) the scripted outcome mid-test — e.g. a
    /// receiver that refused one attempt and is live again for the retry.
    pub fn set_outcome(&self, outcome: Option<DeliveryOutcome>) {
        self.state.lock().expect("fake transport mutex").outcome = outcome;
    }

    /// Every message this transport actually delivered.
    pub fn delivered(&self) -> Vec<Msg> {
        self.state
            .lock()
            .expect("fake transport mutex")
            .delivered
            .clone()
    }

    /// Every call made, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state
            .lock()
            .expect("fake transport mutex")
            .recorder
            .calls
            .clone()
    }
}

#[async_trait]
impl Transport for FakeTransport {
    fn name(&self) -> &str {
        &self.name
    }

    async fn can_deliver(&self, seat: &SeatDescriptor, msg: &Msg) -> Result<bool> {
        let mut state = self.state.lock().expect("fake transport mutex");
        // The message id is recorded too: a fake that forgot which message it was
        // asked about could not prove the command carve-out (R3-AMEND-3) either.
        state
            .recorder
            .record(format!("can_deliver:{}:{}", seat.id, msg.msg_id));
        if std::mem::take(&mut state.fail_next_reachability) {
            return Err(PijError::Adapter {
                adapter: "testkit/fake-transport".to_string(),
                message: "scripted reachability failure".to_string(),
            });
        }
        Ok(state.reachable)
    }

    async fn deliver(&self, seat: &SeatDescriptor, msg: &Msg) -> Result<DeliveryOutcome> {
        let mut state = self.state.lock().expect("fake transport mutex");
        state
            .recorder
            .record(format!("deliver:{}:{}", seat.id, msg.msg_id));
        if std::mem::take(&mut state.fail_next_deliver) {
            return Err(PijError::Adapter {
                adapter: "testkit/fake-transport".to_string(),
                message: "scripted injection failure".to_string(),
            });
        }
        if let Some(outcome) = state.outcome.clone() {
            return Ok(outcome);
        }
        if !state.reachable {
            return Ok(DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            });
        }
        state.delivered.push(msg.clone());
        // A fake transport observes only that it accepted the bytes — the
        // weakest honest claim (erratum-23b). A fake that claimed
        // VerifiedArrival would let a test prove something no transport did.
        Ok(DeliveryOutcome::Delivered {
            origin: DeliveryOrigin::InjectedToTransport,
        })
    }
}

/// tmux, scripted: panes are arranged, captures are queued, keystrokes recorded.
#[derive(Default)]
pub struct FakeTmux {
    state: Mutex<TmuxState>,
}

#[derive(Default)]
struct TmuxState {
    panes: Vec<Pane>,
    list_pane_failures: usize,
    captures: VecDeque<String>,
    /// Served when the scripted queue is empty, so a test whose subject makes
    /// MANY captures does not have to script one per call.
    standing_capture: Option<String>,
    typing: bool,
    /// What tmux reports for `#{pane_pid}`/`#{pane_current_path}`, per pane.
    ///
    /// Separate from `panes` ON PURPOSE: a pane that tmux LISTS but whose
    /// process cannot be read is a real state (the pane died between the two
    /// calls), and folding the two would make it unscriptable.
    pane_processes: BTreeMap<String, PaneProcess>,
    taps: BTreeMap<String, PathBuf>,
    tap_drains: VecDeque<std::result::Result<Vec<u8>, String>>,
    /// Panes whose `capture` FAILS, as real tmux does for a pane that is gone:
    /// `can't find pane: %N`, exit 1.
    ///
    /// Scriptable because "the pane vanished" is the state that starved this
    /// fleet's whole delivery queue, and the fake could not express it: an
    /// unscripted capture returns an empty string, which the composer parser
    /// reads as Unrecognized and VETOES — a refusal, not an error. Those two
    /// take different branches (a veto releases the job and the pass continues;
    /// an error propagates), so a fake that can only produce the first cannot
    /// test the second, and the untestable branch is the one that broke.
    capture_failures: BTreeSet<String>,
    next_stage: u64,
    stage_errors: VecDeque<String>,
    commit_errors: VecDeque<String>,
    cleanup_errors: VecDeque<String>,
    input_disabled: BTreeSet<String>,
    staged: BTreeMap<String, (StagedSubmit, String)>,
    recorder: Recorder,
}

fn render_staged_composer(text: &str) -> (String, u32, u32) {
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.last().copied().unwrap_or_default();
    let cursor_y = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let cursor_x =
        u32::try_from(last.chars().count() + usize::from(lines.len() == 1) * 2).unwrap_or(u32::MAX);
    (
        format!("────────────\n❯ {text}\n────────────"),
        cursor_x,
        cursor_y,
    )
}

impl FakeTmux {
    /// A tmux with no panes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Arrange a pane.
    pub fn with_pane(self, pane: Pane) -> Self {
        self.state.lock().expect("fake tmux mutex").panes.push(pane);
        self
    }

    /// Make `capture` FAIL for this pane, the way tmux fails for a pane that no
    /// longer exists. See [`TmuxState::capture_failures`].
    #[must_use]
    pub fn with_vanished_pane(self, pane: &str) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .capture_failures
            .insert(pane.to_string());
        self
    }

    /// Fail the next `count` pane inventory calls, then resume normal answers.
    #[must_use]
    pub fn with_list_pane_failures(self, count: usize) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .list_pane_failures = count;
        self
    }

    /// Arrange the process tmux reports for a pane.
    pub fn with_pane_process(self, pane: &str, process: PaneProcess) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .pane_processes
            .insert(pane.to_string(), process);
        self
    }

    /// Queue what the next `capture` returns. Queued in order, so a test can
    /// script a pane that changes between polls — the readiness-anchor case.
    pub fn script_capture(self, text: impl Into<String>) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .captures
            .push_back(text.into());
        self
    }

    /// Arrange a pane whose composer parses as RECOGNIZED and BLANK, for every
    /// capture rather than once.
    ///
    /// Needed because the send-boundary gate captures on EVERY injection, so a
    /// singly-scripted capture is consumed by the first one and every later call
    /// falls back to an empty pane — which parses as `Unrecognized` and vetoes.
    /// Deliberately NOT the fake's default: an unscripted capture stays an empty
    /// pane and therefore a veto, because a fake that permits injection by
    /// default would make the gate it is used to test unfalsifiable.
    ///
    /// The single box row is the OMP layout's empty-payload case; the cursor sits
    /// on that row, which is what makes it a positively recognized blank rather
    /// than an absence of evidence.
    pub fn with_clear_composer(self, pane: &str) -> Self {
        self.arrange_clear_composer(pane);
        self
    }

    /// Arrange a listed pane without claiming its composer is recognized.
    ///
    /// This keeps "pane exists" separate from "injection is permitted" for
    /// delivery-policy tests.
    pub fn arrange_pane(&self, pane: &str) {
        let mut state = self.state.lock().expect("fake tmux mutex");
        if !state.panes.iter().any(|listed| listed.id == pane) {
            state.panes.push(Pane {
                id: pane.to_string(),
                session: "s".to_string(),
                window: "w".to_string(),
                title: "t".to_string(),
                cursor_x: None,
                cursor_y: None,
            });
        }
        state.taps.insert(
            pane.to_string(),
            std::env::temp_dir().join(format!("pij-fake-tap-{}", pane.replace('%', "pane"))),
        );
    }

    /// `with_clear_composer` for a fake already behind an `Arc`, so a test helper
    /// can arrange a pane at the moment the seat is registered rather than
    /// needing every pane id enumerated up front.
    pub fn arrange_clear_composer(&self, pane: &str) {
        self.arrange_pane(pane);
        let mut state = self.state.lock().expect("fake tmux mutex");
        if let Some(listed) = state.panes.iter_mut().find(|listed| listed.id == pane) {
            listed.cursor_x = Some(0);
            listed.cursor_y = Some(0);
        }
        state.standing_capture = Some(
            "\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f}"
                .to_string(),
        );
    }

    /// Serve `text` for every capture that outruns the scripted queue.
    ///
    /// Pairs with [`FakeTmux::with_pane`] when a test needs a composer with
    /// CONTENT rather than the blank one `with_clear_composer` arranges.
    pub fn with_standing_capture(self, text: impl Into<String>) -> Self {
        self.state.lock().expect("fake tmux mutex").standing_capture = Some(text.into());
        self
    }

    /// Arrange a live pane tap without prescribing its bytes.
    #[must_use]
    pub fn with_attached_tap(self, pane: &str) -> Self {
        self.state.lock().expect("fake tmux mutex").taps.insert(
            pane.to_string(),
            std::env::temp_dir().join(format!("pij-fake-tap-{}", pane.replace('%', "pane"))),
        );
        self
    }

    /// Queue bytes returned by the next attached tap drain.
    pub fn script_tap(self, bytes: impl Into<Vec<u8>>) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .tap_drains
            .push_back(Ok(bytes.into()));
        self
    }

    /// Queue one dependency failure from the next attached tap drain.
    pub fn script_tap_error(self, message: impl Into<String>) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .tap_drains
            .push_back(Err(message.into()));
        self
    }

    /// Make the typing gate report a human mid-keystroke.
    pub fn with_user_typing(self) -> Self {
        self.state.lock().expect("fake tmux mutex").typing = true;
        self
    }

    /// Change the observed typing mode for a later delivery attempt.
    pub fn set_user_typing(&self, typing: bool) {
        self.state.lock().expect("fake tmux mutex").typing = typing;
    }

    /// Every call made, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .recorder
            .calls
            .clone()
    }
    /// Fail the next staged body write after releasing fake pane ownership.
    #[must_use]
    pub fn script_stage_error(self, message: impl Into<String>) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .stage_errors
            .push_back(message.into());
        self
    }

    /// Fail the next commit before Enter after releasing fake pane ownership.
    #[must_use]
    pub fn script_commit_error(self, message: impl Into<String>) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .commit_errors
            .push_back(message.into());
        self
    }

    /// Fail post-Enter cleanup while preserving the successful submit outcome.
    #[must_use]
    pub fn script_cleanup_error(self, message: impl Into<String>) -> Self {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .cleanup_errors
            .push_back(message.into());
        self
    }

    /// Number of pane transactions still owned by the fake.
    pub fn staged_len(&self) -> usize {
        self.state.lock().expect("fake tmux mutex").staged.len()
    }

    /// Whether the fake transaction currently owns disabled pane input.
    pub fn pane_input_disabled(&self, pane: &str) -> bool {
        self.state
            .lock()
            .expect("fake tmux mutex")
            .input_disabled
            .contains(pane)
    }
}

#[async_trait]
impl TmuxPort for FakeTmux {
    async fn list_panes(&self) -> Result<Vec<Pane>> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record("list_panes".to_string());
        if state.list_pane_failures > 0 {
            state.list_pane_failures -= 1;
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: "scripted list-panes failure".to_string(),
            });
        }
        Ok(state.panes.clone())
    }

    async fn pane_process(&self, pane: &str) -> Result<Option<PaneProcess>> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("pane_process:{pane}"));
        // An unarranged pane has NO process, which is the "pane is gone" answer.
        // Nothing is invented: a test that wants a process must say so, so an
        // adoption that succeeds proves the fake was asked and answered.
        Ok(state.pane_processes.get(pane).cloned())
    }

    async fn acquire_submit(&self, pane: &str) -> Result<StagedSubmit> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        if state.staged.values().any(|(staged, _)| staged.pane == pane) {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("pane {pane} already has a staged submit"),
            });
        }
        if state.input_disabled.contains(pane) {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("pane {pane} input is already disabled"),
            });
        }
        state.next_stage = state.next_stage.saturating_add(1);
        let staged = StagedSubmit {
            pane: pane.to_string(),
            token: format!("fake-stage-{}", state.next_stage),
            staged: false,
        };
        state.recorder.record(format!("acquire_submit:{pane}"));
        state
            .staged
            .insert(staged.token.clone(), (staged.clone(), String::new()));
        state.input_disabled.insert(pane.to_string());
        Ok(staged)
    }

    async fn stage_submit(&self, staged: &mut StagedSubmit, text: &str) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        let Some((owned, _)) = state.staged.get(&staged.token) else {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("unknown staged submit {}", staged.token),
            });
        };
        if owned.pane != staged.pane || owned.staged {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: "staged submit ownership mismatch".to_string(),
            });
        }
        if let Some(message) = state.stage_errors.pop_front() {
            state.staged.remove(&staged.token);
            state.input_disabled.remove(&staged.pane);
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message,
            });
        }
        state
            .recorder
            .record(format!("stage_submit:{}:{text}", staged.pane));
        let (_, cursor_x, cursor_y) = render_staged_composer(text);
        if let Some(listed) = state
            .panes
            .iter_mut()
            .find(|listed| listed.id == staged.pane)
        {
            listed.cursor_x = Some(cursor_x);
            listed.cursor_y = Some(cursor_y);
        }
        staged.staged = true;
        let (owned, stored) = state
            .staged
            .get_mut(&staged.token)
            .expect("checked staged submit");
        owned.staged = true;
        *stored = text.to_string();
        Ok(())
    }

    async fn commit_submit(&self, staged: &StagedSubmit) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        if let Some(message) = state.commit_errors.pop_front() {
            state.staged.remove(&staged.token);
            state.input_disabled.remove(&staged.pane);
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message,
            });
        }
        let Some((owned, text)) = state.staged.remove(&staged.token) else {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("unknown staged submit {}", staged.token),
            });
        };
        state.input_disabled.remove(&staged.pane);
        if &owned != staged || !staged.staged {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: "staged submit ownership mismatch".to_string(),
            });
        }
        state
            .recorder
            .record(format!("commit_submit:{}:{}", staged.pane, staged.token));
        state
            .recorder
            .record(format!("submit:{}:{text}", staged.pane));
        if let Some(message) = state.cleanup_errors.pop_front() {
            state.recorder.record(format!("cleanup_error:{message}"));
        }
        state.standing_capture = Some(
            "\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f}"
                .to_string(),
        );
        if let Some(listed) = state
            .panes
            .iter_mut()
            .find(|listed| listed.id == staged.pane)
        {
            listed.cursor_x = Some(0);
            listed.cursor_y = Some(0);
        }
        Ok(())
    }

    async fn abort_submit(&self, staged: &StagedSubmit) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        let Some((owned, text)) = state.staged.remove(&staged.token) else {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("unknown staged submit {}", staged.token),
            });
        };
        state.input_disabled.remove(&staged.pane);
        if &owned != staged {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: "staged submit ownership mismatch".to_string(),
            });
        }
        state
            .recorder
            .record(format!("abort_submit:{}:{}", staged.pane, staged.token));
        if staged.staged {
            let recovery = format!("{text}\n{STAGED_SUBMIT_RECOVERY}");
            let (capture, cursor_x, cursor_y) = render_staged_composer(&recovery);
            state.standing_capture = Some(capture);
            if let Some(listed) = state
                .panes
                .iter_mut()
                .find(|listed| listed.id == staged.pane)
            {
                listed.cursor_x = Some(cursor_x);
                listed.cursor_y = Some(cursor_y);
            }
        }
        Ok(())
    }

    async fn submit(&self, pane: &str, text: &str) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        // Recorded DISTINCTLY from send_keys: the whole reason submit is its own
        // verb is that "typed" and "sent" are different events, and a fake that
        // recorded them identically would make that distinction untestable.
        state.recorder.record(format!("submit:{pane}:{text}"));
        Ok(())
    }

    async fn send_keys(&self, pane: &str, keys: &str) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("send_keys:{pane}:{keys}"));
        Ok(())
    }

    async fn capture(&self, pane: &str, lines: u32) -> Result<String> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("capture:{pane}:{lines}"));
        if state.capture_failures.contains(pane) {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("can't find pane: {pane}"),
            });
        }
        if let Some(capture) = state.captures.pop_front() {
            return Ok(capture);
        }
        if let Some((_, text)) = state
            .staged
            .values()
            .find(|(staged, _)| staged.pane == pane && staged.staged)
        {
            return Ok(render_staged_composer(text).0);
        }
        // An unscripted capture is an empty pane, not a panic: a fake that
        // explodes on an unexpected call makes tests about the fake.
        Ok(state.standing_capture.clone().unwrap_or_default())
    }

    async fn attach_pane_tap(&self, pane: &str, sink: &Path) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state
            .recorder
            .record(format!("attach_pane_tap:{pane}:{}", sink.display()));
        if let Some(existing) = state.taps.get(pane) {
            if existing == sink {
                return Ok(());
            }
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("pane {pane:?} already has a different tap sink"),
            });
        }
        state.taps.insert(pane.to_string(), sink.to_path_buf());
        Ok(())
    }

    async fn pane_tap_sink(&self, pane: &str) -> Result<Option<PathBuf>> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("pane_tap_sink:{pane}"));
        Ok(state.taps.get(pane).cloned())
    }

    async fn drain_pane_tap(&self, pane: &str) -> Result<Vec<u8>> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("drain_pane_tap:{pane}"));
        if !state.taps.contains_key(pane) {
            return Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message: format!("pane {pane:?} has no attached tap"),
            });
        }
        match state.tap_drains.pop_front() {
            Some(Ok(bytes)) => Ok(bytes),
            Some(Err(message)) => Err(PijError::Adapter {
                adapter: "fake-tmux".to_string(),
                message,
            }),
            None => Ok(Vec::new()),
        }
    }

    async fn detach_pane_tap(&self, pane: &str) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("detach_pane_tap:{pane}"));
        state.taps.remove(pane).ok_or_else(|| PijError::Adapter {
            adapter: "fake-tmux".to_string(),
            message: format!("pane {pane:?} has no attached tap"),
        })?;
        Ok(())
    }

    async fn kill(&self, pane: &str) -> Result<()> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("kill:{pane}"));
        state.panes.retain(|p| p.id != pane);
        Ok(())
    }

    async fn new_window(
        &self,
        session: &str,
        name: &str,
        cwd: &str,
        command: Option<&LaunchCommand>,
    ) -> Result<Pane> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        let call = command.map_or_else(
            || format!("new_window:{session}:{name}"),
            |command| {
                format!(
                    "new_window:{session}:{name}:{}:{:?}",
                    command.executable, command.args
                )
            },
        );
        state.recorder.record(call);
        let pane = Pane {
            id: format!("%{}", 100 + state.panes.len()),
            session: session.to_string(),
            window: name.to_string(),
            title: cwd.to_string(),
            cursor_x: Some(0),
            cursor_y: Some(0),
        };
        state.panes.push(pane.clone());
        Ok(pane)
    }

    async fn user_typing(&self, pane: &str) -> Result<bool> {
        let mut state = self.state.lock().expect("fake tmux mutex");
        state.recorder.record(format!("user_typing:{pane}"));
        Ok(state.typing)
    }
}

/// A harness adapter with scripted discovery, binding, readiness and catalog.
pub struct FakeHarness {
    kind: Harness,
    state: Mutex<HarnessState>,
}

#[derive(Default)]
struct HarnessState {
    session: Option<String>,
    bind: Option<BindHealth>,
    readiness: VecDeque<Readiness>,
    busy: VecDeque<bool>,
    idle: VecDeque<bool>,
    idle_errors: usize,
    idle_probes: usize,
    models: Vec<ModelRow>,
}

impl FakeHarness {
    /// A harness adapter for `kind` that discovers nothing and is never ready.
    pub fn new(kind: Harness) -> Self {
        FakeHarness {
            kind,
            state: Mutex::new(HarnessState::default()),
        }
    }

    /// Script the native session id discovery finds.
    pub fn with_session(self, session: impl Into<String>) -> Self {
        self.state.lock().expect("fake harness mutex").session = Some(session.into());
        self
    }

    /// Script the bind verdict.
    pub fn with_bind(self, bind: BindHealth) -> Self {
        self.state.lock().expect("fake harness mutex").bind = Some(bind);
        self
    }

    /// Queue a readiness answer. Queued in order so a test can walk a seat from
    /// booting to ready without a clock.
    pub fn script_readiness(self, readiness: Readiness) -> Self {
        self.state
            .lock()
            .expect("fake harness mutex")
            .readiness
            .push_back(readiness);
        self
    }

    /// Queue a `busy` answer. Queued, so a test can walk a seat from mid-turn to
    /// idle without a clock — the shape every delivery-gating decision needs.
    pub fn script_busy(self, busy: bool) -> Self {
        self.state
            .lock()
            .expect("fake harness mutex")
            .busy
            .push_back(busy);
        self
    }

    /// Queue an `idle` answer (positive idle evidence from the pane).
    pub fn script_idle(self, idle: bool) -> Self {
        self.state
            .lock()
            .expect("fake harness mutex")
            .idle
            .push_back(idle);
        self
    }

    /// Make the next idle probe fail, as a failed pane capture would.
    pub fn script_idle_error(self) -> Self {
        self.state.lock().expect("fake harness mutex").idle_errors += 1;
        self
    }

    /// Set the catalog this harness reports.
    pub fn with_models(self, models: Vec<ModelRow>) -> Self {
        self.state.lock().expect("fake harness mutex").models = models;
        self
    }

    /// How many idle probes this harness has answered, failed ones included.
    pub fn idle_probes(&self) -> usize {
        self.state.lock().expect("fake harness mutex").idle_probes
    }
}

#[async_trait]
impl HarnessPort for FakeHarness {
    fn kind(&self) -> Harness {
        self.kind
    }

    async fn discover_session(&self, _pane: &str) -> Result<Option<String>> {
        Ok(self
            .state
            .lock()
            .expect("fake harness mutex")
            .session
            .clone())
    }

    async fn bind(&self, descriptor: &SeatDescriptor) -> Result<BindHealth> {
        let state = self.state.lock().expect("fake harness mutex");
        Ok(state.bind.clone().unwrap_or(BindHealth::Unbound {
            evidence: format!("fake harness has no scripted bind for {}", descriptor.id),
        }))
    }

    async fn readiness(&self, _pane: &str) -> Result<Readiness> {
        let mut state = self.state.lock().expect("fake harness mutex");
        Ok(state.readiness.pop_front().unwrap_or(Readiness::NotYet {
            observed: "fake harness: nothing scripted".to_string(),
        }))
    }

    async fn busy(&self, _pane: &str) -> Result<bool> {
        // Unscripted means NOT busy: a fake that claims a seat is mid-turn by
        // default would make every delivery test arrange its way out of a state
        // it never asked for.
        let mut state = self.state.lock().expect("fake harness mutex");
        Ok(state.busy.pop_front().unwrap_or(false))
    }

    async fn idle(&self, _pane: &str) -> Result<bool> {
        // Unscripted means NO idle evidence: idleness is what the caller acts on.
        let mut state = self.state.lock().expect("fake harness mutex");
        state.idle_probes += 1;
        if state.idle_errors > 0 {
            state.idle_errors -= 1;
            return Err(PijError::Adapter {
                adapter: "fake/harness".to_string(),
                message: "pane capture failed".to_string(),
            });
        }
        Ok(state.idle.pop_front().unwrap_or(false))
    }

    async fn models(&self) -> Result<Vec<ModelRow>> {
        Ok(self
            .state
            .lock()
            .expect("fake harness mutex")
            .models
            .clone())
    }
}

/// A process table, as a map. The whole point is that a recycled pid is one line
/// to arrange: record a start time, then report a different one.
#[derive(Default)]
pub struct FakeLiveness {
    procs: Mutex<HashMap<u32, u64>>,
}

impl FakeLiveness {
    /// A machine with no processes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Arrange a running process.
    pub fn with_proc(self, proc: ProcIdentity) -> Self {
        self.procs
            .lock()
            .expect("fake liveness mutex")
            .insert(proc.pid, proc.proc_start);
        self
    }

    /// Arrange a DIFFERENT process at the same pid — the recycled case.
    pub fn with_recycled(self, pid: u32, observed_start: u64) -> Self {
        self.procs
            .lock()
            .expect("fake liveness mutex")
            .insert(pid, observed_start);
        self
    }
}

#[async_trait]
impl LivenessPort for FakeLiveness {
    async fn proc_start(&self, pid: u32) -> Result<Option<u64>> {
        Ok(self
            .procs
            .lock()
            .expect("fake liveness mutex")
            .get(&pid)
            .copied())
    }
}

/// Session facts, scripted per native session id.
///
/// An unscripted session answers [`SessionStatusReply::Unsupported`], the reply a
/// source gives for a harness it can't read, so an unarranged test never sees facts.
#[derive(Default)]
pub struct FakeSessionStatus {
    replies: Mutex<HashMap<String, std::result::Result<SessionStatusReply, String>>>,
    hangs: Mutex<Vec<String>>,
    recorder: Mutex<Recorder>,
}

impl FakeSessionStatus {
    /// A source that has read nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer `reply` for the session `session`.
    pub fn with_reply(self, session: &str, reply: SessionStatusReply) -> Self {
        self.replies
            .lock()
            .expect("fake session-status mutex")
            .insert(session.to_string(), Ok(reply));
        self
    }

    /// Fail reads of the session `session` with an adapter error.
    pub fn with_failure(self, session: &str, message: &str) -> Self {
        self.replies
            .lock()
            .expect("fake session-status mutex")
            .insert(session.to_string(), Err(message.to_string()));
        self
    }

    /// Never answer reads of the session `session`: the read hangs until the
    /// caller gives up, as a stuck transcript fold would.
    pub fn with_hang(self, session: &str) -> Self {
        self.hangs
            .lock()
            .expect("fake session-status mutex")
            .push(session.to_string());
        self
    }

    /// Every read, in order: `status:<seat>:<harness>:<session>`.
    pub fn calls(&self) -> Vec<Call> {
        self.recorder
            .lock()
            .expect("fake session-status recorder")
            .calls
            .clone()
    }
}

#[async_trait]
impl SessionStatusPort for FakeSessionStatus {
    async fn status(&self, target: &SessionTarget) -> Result<SessionStatusReply> {
        self.recorder
            .lock()
            .expect("fake session-status recorder")
            .record(format!(
                "status:{}:{}:{}",
                target.seat.0,
                target.harness.as_str(),
                target.session
            ));
        let hangs = self
            .hangs
            .lock()
            .expect("fake session-status mutex")
            .contains(&target.session);
        if hangs {
            std::future::pending::<()>().await;
        }
        match self
            .replies
            .lock()
            .expect("fake session-status mutex")
            .get(&target.session)
            .cloned()
        {
            Some(Ok(reply)) => Ok(reply),
            Some(Err(message)) => Err(PijError::Adapter {
                adapter: "fake/session-status".to_string(),
                message,
            }),
            None => Ok(SessionStatusReply::Unsupported),
        }
    }
}
