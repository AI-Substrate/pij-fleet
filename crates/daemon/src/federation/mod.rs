//! Cross-machine delivery owned by one long-lived worker.
mod fanin;

pub(crate) use fanin::RemoteSubscription;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::config::PeerDefinition;
use pij_core::error::{PijError, Result};
use pij_core::model::{
    DeliveryOutcome, Destination, Envelope, ErrorKind, Event, Job, JobId, Outcome, Receipt,
    SeatDescriptor, SeatId,
};
use pij_core::ports::Queue;
use serde::Serialize;

use self::fanin::FanInService;
use crate::events::EventBus;
use crate::http::{FederatedRoster, PeerEndpoint, SendRequest, post_to_peer};

/// The queue kind for a remote send to `alias`.
///
/// PER DESTINATION, not one kind for every peer. Dedupe is scoped by
/// `(kind, dedupe_key)` since schema 7, so a single shared kind made the fix for
/// that defect apply to delivery rows and NOT to federation rows: one msg_id sent
/// to `bob@laptop` and then `bob@desktop` still collapsed, and the second caller
/// was told `Queued` for a row destined to the first machine. Found by review
/// round 2, against a scratch database rather than by reading.
///
/// It also makes the sweep scoping honest: a federation worker claims the kinds of
/// its configured peers, so another worker's pass cannot reach its claims.
fn remote_send_kind(alias: &str) -> String {
    format!("federation:{alias}")
}
const OUTCOME_EVENT_KIND: &str = "delivery.outcome";
const REMOTE_REFUSED_EVENT_KIND: &str = "delivery.remote-refused";
const WORKER_NAME: &str = "federation";
// Retry/poll timing is injected through `FederationPolicy`; it changes outcomes
// and is policy rather than a safety brake.

type Clock = dyn Fn() -> Result<u64> + Send + Sync;
type Jitter = dyn Fn(Duration) -> Result<Duration> + Send + Sync;

/// Injected timing and buffer policy for all federation workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FederationPolicy {
    /// Base roster poll and reconnect delay.
    pub poll_interval: Duration,
    /// Maximum send/reconnect backoff.
    pub max_retry_delay: Duration,
    /// Per-peer remote event buffer bound.
    pub event_buffer_capacity: usize,
    /// How long a remote send waits inline for its first forwarding attempt
    /// before answering `Queued`. A latency cap, not a brake: past it the same
    /// outcome still lands as an event.
    pub first_attempt_wait: Duration,
}
/// A peer-configuration refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerConfigError {
    /// Two ordered definitions use the same alias.
    DuplicateAlias(String),
    /// A peer claims the local machine's alias.
    LocalAlias(String),
    /// A destination names no configured peer.
    UnknownAlias(String),
    /// Injected federation policy is invalid.
    InvalidPolicy(String),
}

impl fmt::Display for PeerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateAlias(alias) => {
                write!(
                    formatter,
                    "peer alias `{alias}` is configured more than once"
                )
            }
            Self::LocalAlias(alias) => write!(
                formatter,
                "peer alias `{alias}` equals the local machine alias; omit the machine to address this daemon"
            ),
            Self::UnknownAlias(alias) => write!(
                formatter,
                "peer alias `{alias}` is not configured; add it to the ordered peer definitions"
            ),
            Self::InvalidPolicy(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for PeerConfigError {}

/// The result of resolving an already-parsed destination.
pub enum ResolvedPeer<'a> {
    /// An unqualified destination, always owned by this daemon.
    Local,
    /// A destination owned by one configured peer.
    Remote(&'a PeerEndpoint),
}

/// Validated peer aliases, preserving one endpoint and one credential per peer.
pub struct PeerTable {
    peers: BTreeMap<String, PeerEndpoint>,
}

impl PeerTable {
    /// Validate ordered definitions before building the lookup map.
    ///
    /// # Errors
    /// Duplicate aliases and aliases equal to the local machine.
    pub fn new(
        local_alias: &str,
        definitions: impl IntoIterator<Item = PeerDefinition>,
    ) -> std::result::Result<Self, PeerConfigError> {
        let mut peers = BTreeMap::new();
        for definition in definitions {
            if definition.alias == local_alias {
                return Err(PeerConfigError::LocalAlias(definition.alias));
            }
            let alias = definition.alias;
            let endpoint = PeerEndpoint {
                base_url: definition.url,
                bearer_key: definition.key,
            };
            if peers.insert(alias.clone(), endpoint).is_some() {
                return Err(PeerConfigError::DuplicateAlias(alias));
            }
        }
        Ok(Self { peers })
    }

    /// Every configured peer alias, in order.
    pub fn aliases(&self) -> impl Iterator<Item = &str> {
        self.peers.keys().map(String::as_str)
    }

    fn endpoints(&self) -> impl Iterator<Item = (&str, &PeerEndpoint)> {
        self.peers
            .iter()
            .map(|(alias, endpoint)| (alias.as_str(), endpoint))
    }

    /// Resolve a destination without ever treating an unqualified seat as remote.
    ///
    /// # Errors
    /// A qualified destination whose machine alias is not configured.
    pub fn resolve(
        &self,
        destination: &Destination,
    ) -> std::result::Result<ResolvedPeer<'_>, PeerConfigError> {
        let Some(alias) = destination.machine.as_deref() else {
            return Ok(ResolvedPeer::Local);
        };
        self.peers
            .get(alias)
            .map(ResolvedPeer::Remote)
            .ok_or_else(|| PeerConfigError::UnknownAlias(alias.to_string()))
    }
}

/// What a remote send's sender learns inline.
#[derive(Clone, Debug, PartialEq)]
pub enum FirstAttempt {
    /// The peer accepted the message and answered with its receipt.
    Forwarded(Receipt),
    /// The peer refused it under its own rules; its words and details, relayed.
    Refused {
        /// The peer's refusal text.
        reason: String,
        /// The peer's structured details (for a cold wake: code and cold facts).
        details: Option<serde_json::Value>,
    },
    /// Durably queued but not answered within the wait (peer unreachable, or
    /// behind earlier rows for the same seat). The outcome lands as an event.
    Queued(Receipt),
}

/// What the worker tells an inline waiter about the first attempt.
#[derive(Clone)]
enum Settled {
    Forwarded(Receipt),
    Refused {
        reason: String,
        details: Option<serde_json::Value>,
    },
    Retrying,
}

/// Every sender waiting on one `(peer alias, msg_id)`, each with its own id so
/// it removes only itself (review F07: a second waiter must not replace or
/// remove the first).
type WaiterMap = HashMap<(String, String), Vec<(u64, tokio::sync::oneshot::Sender<Settled>)>>;
type Waiters = Mutex<WaiterMap>;

/// Remote-send admission and the durable queue consumed by the federation worker.
pub struct FederationService {
    local_alias: String,
    peers: PeerTable,
    queue: Arc<dyn Queue>,
    event_bus: Arc<EventBus>,
    clock: Arc<Clock>,
    client: reqwest::Client,
    jitter: Arc<Jitter>,
    wake: tokio::sync::Notify,
    retry_base: Duration,
    retry_max: Duration,
    fanin: Arc<FanInService>,
    /// Senders waiting inline for their row's FIRST forwarding attempt, keyed
    /// by `(peer alias, msg_id)`. Removed when answered or when the wait ends.
    first_attempt_wait: Duration,
    first_attempts: Waiters,
    next_waiter: std::sync::atomic::AtomicU64,
}

impl FederationService {
    /// Every configured peer alias, for callers that must decide whether a
    /// machine can be served at all.
    pub fn peer_aliases(&self) -> impl Iterator<Item = &str> {
        self.peers.aliases()
    }

    /// Validate peer definitions and build the remote-send and fan-in service.
    ///
    /// # Errors
    /// Duplicate/self aliases or an invalid zero/inverted worker policy.
    pub fn new(
        local_alias: String,
        definitions: impl IntoIterator<Item = PeerDefinition>,
        queue: Arc<dyn Queue>,
        event_bus: Arc<EventBus>,
        policy: FederationPolicy,
    ) -> std::result::Result<Self, PeerConfigError> {
        Self::with_sources(
            local_alias,
            definitions,
            queue,
            event_bus,
            policy,
            Arc::new(system_time_ms),
            Arc::new(random_jitter),
        )
    }

    fn with_sources(
        local_alias: String,
        definitions: impl IntoIterator<Item = PeerDefinition>,
        queue: Arc<dyn Queue>,
        event_bus: Arc<EventBus>,
        policy: FederationPolicy,
        clock: Arc<Clock>,
        jitter: Arc<Jitter>,
    ) -> std::result::Result<Self, PeerConfigError> {
        if policy.poll_interval.is_zero() {
            return Err(PeerConfigError::InvalidPolicy(
                "federation poll/reconnect interval must be non-zero".to_string(),
            ));
        }
        if policy.max_retry_delay < policy.poll_interval {
            return Err(PeerConfigError::InvalidPolicy(
                "federation retry maximum must be at least the poll/reconnect interval".to_string(),
            ));
        }
        if policy.event_buffer_capacity == 0 {
            return Err(PeerConfigError::InvalidPolicy(
                "federation event buffer capacity must be non-zero".to_string(),
            ));
        }
        let peers = PeerTable::new(&local_alias, definitions)?;
        // A TOTAL timeout. reqwest has NONE by default, so a stalled peer cannot
        // hold a queue claim forever. Comfortably under the default lease.
        //
        // `no_proxy` on BOTH clients (review S4): every request carries a pair's
        // bearer key, and reqwest otherwise hands it to whatever HTTP(S)_PROXY
        // the daemon inherited. A peer is reached directly or not at all. There
        // is deliberately no `Client::new()` fallback: that one reads the proxy
        // environment again.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| {
                PeerConfigError::InvalidPolicy(format!("could not build the peer client: {error}"))
            })?;
        // A stream is intentionally unbounded in duration. A total request
        // timeout would kill every healthy peer stream after 30 seconds, so it
        // gets a connect timeout only; reconnect owns later failures.
        let stream_client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| {
                PeerConfigError::InvalidPolicy(format!(
                    "could not build the peer stream client: {error}"
                ))
            })?;
        let fanin = Arc::new(FanInService::new(
            peers
                .endpoints()
                .map(|(alias, endpoint)| (alias.to_string(), endpoint.clone())),
            client.clone(),
            stream_client,
            policy.poll_interval,
            policy.max_retry_delay,
            policy.event_buffer_capacity,
        ));
        Ok(Self {
            local_alias,
            peers,
            queue,
            event_bus,
            clock,
            client,
            jitter,
            wake: tokio::sync::Notify::new(),
            retry_base: policy.poll_interval,
            retry_max: policy.max_retry_delay,
            fanin,
            first_attempt_wait: policy.first_attempt_wait,
            first_attempts: Mutex::new(HashMap::new()),
            next_waiter: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Enqueue a remote send, then wait up to the policy's `first_attempt_wait`
    /// for its first forwarding
    /// attempt, so the sender sees the peer's receipt or refusal inline — a
    /// cold-wake refusal by the receiver reads exactly as a local one would
    /// (plan 164 ruling 6). Durability is unchanged: the row is queued before
    /// the wait, and a peer that is down leaves it queued with backoff.
    ///
    /// # Errors
    /// As [`Self::enqueue_remote`].
    pub async fn send_remote(
        &self,
        request: &SendRequest,
    ) -> std::result::Result<FirstAttempt, FederationSendError> {
        let key = (
            request.to.machine.clone().unwrap_or_default(),
            request.msg_id.clone(),
        );
        let (sender, answer) = tokio::sync::oneshot::channel();
        let waiter = self
            .next_waiter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.waiters()
            .entry(key.clone())
            .or_default()
            .push((waiter, sender));
        let queued = match self.enqueue_remote(request).await {
            Ok(receipt) => receipt,
            Err(error) => {
                self.forget_waiter(&key, waiter);
                return Err(error);
            }
        };
        let settled = tokio::time::timeout(self.first_attempt_wait, answer).await;
        self.forget_waiter(&key, waiter);
        Ok(match settled {
            Ok(Ok(Settled::Forwarded(receipt))) => FirstAttempt::Forwarded(receipt),
            Ok(Ok(Settled::Refused { reason, details })) => {
                FirstAttempt::Refused { reason, details }
            }
            Ok(Ok(Settled::Retrying) | Err(_)) | Err(_) => FirstAttempt::Queued(queued),
        })
    }

    fn waiters(&self) -> std::sync::MutexGuard<'_, WaiterMap> {
        // A poisoned map only ever held senders; recovering it loses nothing.
        self.first_attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Remove ONE waiter, leaving any other waiting on the same message.
    fn forget_waiter(&self, key: &(String, String), waiter: u64) {
        let mut waiters = self.waiters();
        if let Some(list) = waiters.get_mut(key) {
            list.retain(|(id, _)| *id != waiter);
            if list.is_empty() {
                waiters.remove(key);
            }
        }
    }

    /// Answer EVERY sender waiting on this message's first attempt.
    fn settle(&self, request: &SendRequest, settled: Settled) {
        let key = (
            request.to.machine.clone().unwrap_or_default(),
            request.msg_id.clone(),
        );
        let waiting = self.waiters().remove(&key).unwrap_or_default();
        for (_, waiter) in waiting {
            let _ = waiter.send(settled.clone());
        }
    }

    /// Merge authoritative local rows with retained remote roster views.
    pub fn federated_roster(&self, local: Vec<SeatDescriptor>) -> FederatedRoster {
        self.fanin.roster(local)
    }

    /// Attach to bounded remote event views at independent machine cursors.
    pub(crate) fn subscribe_remote(&self, cursors: &BTreeMap<String, u64>) -> RemoteSubscription {
        self.fanin.subscribe(cursors)
    }

    /// Resolve and durably enqueue one remote request.
    ///
    /// The returned receipt says only `Queued`: at this point the request path
    /// has observed local persistence, not the peer or its recipient.
    ///
    /// # Errors
    /// Unknown aliases, serialization failures, queue failures, or audit failures.
    pub async fn enqueue_remote(
        &self,
        request: &SendRequest,
    ) -> std::result::Result<Receipt, FederationSendError> {
        let alias = request.to.machine.as_deref().ok_or_else(|| {
            FederationSendError::Refused("an unqualified destination is local".to_string())
        })?;
        match self.peers.resolve(&request.to) {
            Ok(ResolvedPeer::Remote(_)) => {}
            Ok(ResolvedPeer::Local) => {
                return Err(FederationSendError::Refused(
                    "an unqualified destination is local".to_string(),
                ));
            }
            Err(error) => return Err(FederationSendError::Refused(error.to_string())),
        }

        let payload = serde_json::to_string(request).map_err(|error| {
            FederationSendError::Runtime(PijError::Adapter {
                adapter: "daemon/federation".to_string(),
                message: format!(
                    "could not encode remote message {}: {error}",
                    request.msg_id
                ),
            })
        })?;
        self.queue
            .enqueue(Job {
                kind: remote_send_kind(alias),
                serial_key: format!("{}@{alias}", request.to.seat),
                payload,
                dedupe_key: request.msg_id.clone(),
                attempt: 0,
            })
            .await
            .map_err(FederationSendError::Runtime)?;

        let receipt = Receipt {
            msg_id: request.msg_id.clone(),
            outcome: DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            },
            at: (self.clock)().map_err(FederationSendError::Runtime)?,
            cold_check: None,
            warning: None,
        };
        self.publish_outcome(&request.from, alias, &receipt)
            .await
            .map_err(|error| {
                FederationSendError::Runtime(PijError::DeliveryAuditFailed {
                    msg_id: receipt.msg_id.clone(),
                    outcome: "queued".to_string(),
                    audit_error: error.to_string(),
                })
            })?;
        self.wake.notify_one();
        Ok(receipt)
    }

    /// Process one ready remote-send row.
    ///
    /// # Errors
    /// Queue, wire, or audit failures outside normal delivery outcomes.
    pub async fn process_one(&self) -> Result<WorkerStep> {
        // Every configured peer's kind. `claim` takes a list, so one worker still
        // serves all peers — what changed is that the QUEUE can tell them apart.
        let kinds: Vec<String> = self.peers.aliases().map(remote_send_kind).collect();
        let Some((job_id, job)) = self.queue.claim(&kinds, WORKER_NAME).await? else {
            return Ok(WorkerStep::Idle);
        };
        let request: SendRequest = match serde_json::from_str(&job.payload) {
            Ok(request) => request,
            Err(error) => {
                let reason = format!("invalid remote-send payload: {error}");
                self.queue
                    .ack(
                        job_id,
                        Outcome::Failed {
                            reason: reason.clone(),
                        },
                    )
                    .await?;
                return Err(PijError::Adapter {
                    adapter: "daemon/federation".to_string(),
                    message: format!("job {}: {reason}", job_id.0),
                });
            }
        };
        let alias = match request.to.machine.as_deref() {
            Some(alias) => alias,
            None => {
                self.refuse_claim(
                    job_id,
                    &request,
                    "remote-send job has a local destination".to_string(),
                    None,
                )
                .await?;
                return Ok(WorkerStep::Refused);
            }
        };
        let endpoint = match self.peers.resolve(&request.to) {
            Ok(ResolvedPeer::Remote(endpoint)) => endpoint,
            Ok(ResolvedPeer::Local) => unreachable!("machine was present"),
            Err(error) => {
                self.refuse_claim(job_id, &request, error.to_string(), None)
                    .await?;
                return Ok(WorkerStep::Refused);
            }
        };

        let mut forwarded = request.clone();
        forwarded.to.machine = None;
        forwarded.from_machine = Some(self.local_alias.clone());
        // A reply to a message that peer sent us names it by the id we scoped
        // it under (`<id>@<alias>`); give the peer back its own id.
        if let Some(answered) = forwarded.in_reply_to.as_deref()
            && let Some(own) = answered.strip_suffix(&format!("@{alias}"))
        {
            forwarded.in_reply_to = Some(own.to_string());
        }
        match post_to_peer::<_, Receipt>(&self.client, endpoint, "/v1/send", &forwarded).await {
            Err(_) => self.retry_claim(job_id, job.attempt, &request).await,
            Ok(envelope) if !envelope.ok && envelope.error == Some(ErrorKind::Adapter) => {
                self.retry_claim(job_id, job.attempt, &request).await
            }
            Ok(envelope) if !envelope.ok => {
                // The peer's own auth prose speaks to ITS local clients; the
                // sender needs to hear that the pairing itself failed.
                let reason = if envelope.error == Some(ErrorKind::Auth) {
                    format!(
                        "peer `{alias}` refused this machine's pairing key ({}); its peers.toml may no longer pair with this machine — run `pij-rs peers check`",
                        refusal_reason(&envelope)
                    )
                } else {
                    refusal_reason(&envelope)
                };
                self.refuse_claim(job_id, &request, reason, envelope.details)
                    .await?;
                Ok(WorkerStep::Refused)
            }
            Ok(envelope) => {
                let Some(receipt) = envelope.data else {
                    self.refuse_claim(
                        job_id,
                        &request,
                        "peer returned ok without a receipt".to_string(),
                        None,
                    )
                    .await?;
                    return Ok(WorkerStep::Refused);
                };
                if receipt.msg_id != request.msg_id {
                    self.refuse_claim(
                        job_id,
                        &request,
                        format!(
                            "peer receipt named message {}, expected {}",
                            receipt.msg_id, request.msg_id
                        ),
                        None,
                    )
                    .await?;
                    return Ok(WorkerStep::Refused);
                }
                // Relay the peer's exact evidence. HTTP acceptance creates no
                // delivery claim of its own (E-023).
                if self
                    .publish_outcome(&request.from, alias, &receipt)
                    .await
                    .is_err()
                {
                    let delay = self.retry_delay(job.attempt)?;
                    self.queue.retry(job_id, delay).await?;
                    self.settle(&request, Settled::Retrying);
                    return Ok(WorkerStep::Retried);
                }
                self.queue.ack(job_id, Outcome::Done).await?;
                self.settle(&request, Settled::Forwarded(receipt));
                Ok(WorkerStep::Forwarded)
            }
        }
    }

    /// Start remote-send, roster-poll, and per-peer event-source loops.
    pub fn start(self: Arc<Self>) -> FederationWorker {
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        self.start_with_shutdown(shutdown, shutdown_rx)
    }

    fn start_with_shutdown(
        self: Arc<Self>,
        shutdown: tokio::sync::watch::Sender<bool>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> FederationWorker {
        let mut joined = self.fanin.start(shutdown_rx.clone());
        let mut send_shutdown = shutdown_rx;
        let send_service = Arc::clone(&self);
        joined.push(tokio::spawn(async move {
            loop {
                // A wake and shutdown can become ready together. Check before
                // claiming as well as after a completed claim: shutdown is a
                // one-way barrier after which this worker must never dequeue.
                if *send_shutdown.borrow() {
                    return;
                }
                // A transient dependency failure is reported and the LOOP keeps
                // running; the next queue item must not be abandoned for the
                // lifetime of the daemon (COMMON 1.3 / E-030).
                let step = match send_service.process_one().await {
                    Ok(step) => step,
                    Err(error) => {
                        eprintln!("pij-rs federation: {error}");
                        WorkerStep::Idle
                    }
                };
                match step {
                    WorkerStep::Idle => {
                        tokio::select! {
                            biased;
                            changed = send_shutdown.changed() => {
                                if changed.is_err() || *send_shutdown.borrow() {
                                    return;
                                }
                            }
                            () = send_service.wake.notified() => {},
                            () = tokio::time::sleep(send_service.retry_base) => {},
                        }
                    }
                    WorkerStep::Forwarded | WorkerStep::Retried | WorkerStep::Refused => {
                        if *send_shutdown.borrow() {
                            return;
                        }
                    }
                }
            }
        }));
        FederationWorker {
            shutdown: Some(shutdown),
            joined,
        }
    }

    async fn retry_claim(
        &self,
        job_id: JobId,
        attempt: u32,
        request: &SendRequest,
    ) -> Result<WorkerStep> {
        let delay = self.retry_delay(attempt)?;
        self.queue.retry(job_id, delay).await?;
        self.settle(request, Settled::Retrying);
        Ok(WorkerStep::Retried)
    }

    fn retry_delay(&self, attempt: u32) -> Result<Duration> {
        let factor = 1_u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
        let base = self.retry_base.saturating_mul(factor).min(self.retry_max);
        (self.jitter)(base).map(|delay| delay.min(self.retry_max))
    }

    async fn refuse_claim(
        &self,
        job_id: JobId,
        request: &SendRequest,
        reason: String,
        details: Option<serde_json::Value>,
    ) -> Result<()> {
        self.queue
            .ack(
                job_id,
                Outcome::Failed {
                    reason: reason.clone(),
                },
            )
            .await?;
        let payload = serde_json::to_string(&RemoteRefusal {
            msg_id: &request.msg_id,
            peer: request.to.machine.as_deref(),
            reason: &reason,
            details: details.as_ref(),
        })
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/federation".to_string(),
            message: format!("could not encode refusal {}: {error}", request.msg_id),
        })?;
        self.event_bus
            .publish(Event {
                seq: None,
                v: 1,
                at: (self.clock)()?,
                kind: REMOTE_REFUSED_EVENT_KIND.to_string(),
                seat: Some(request.from.clone()),
                payload,
            })
            .await?;
        self.settle(request, Settled::Refused { reason, details });
        Ok(())
    }

    async fn publish_outcome(&self, sender: &SeatId, peer: &str, receipt: &Receipt) -> Result<()> {
        let payload = serde_json::to_string(&RemoteOutcome {
            msg_id: &receipt.msg_id,
            outcome: &receipt.outcome,
            transport: "federation",
            peer,
        })
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/federation".to_string(),
            message: format!("could not encode receipt {}: {error}", receipt.msg_id),
        })?;
        self.event_bus
            .publish(Event {
                seq: None,
                v: 1,
                at: receipt.at,
                kind: OUTCOME_EVENT_KIND.to_string(),
                seat: Some(sender.clone()),
                payload,
            })
            .await?;
        Ok(())
    }
}

/// Result of one worker claim attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerStep {
    /// No remote row was currently eligible.
    Idle,
    /// The peer accepted the request and returned a receipt.
    Forwarded,
    /// A retryable failure returned the row to pending with backoff.
    Retried,
    /// A permanent peer decision terminally refused the row.
    Refused,
}

/// Handle for explicit federation-worker shutdown.
pub struct FederationWorker {
    shutdown: Option<tokio::sync::watch::Sender<bool>>,
    joined: Vec<tokio::task::JoinHandle<()>>,
}

impl FederationWorker {
    /// Stop every worker and wait until no task can outlive its daemon.
    ///
    /// # Errors
    /// The first worker task that panicked, after every task has been awaited.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(true);
        }
        let mut failure = None;
        for joined in self.joined {
            if let Err(error) = joined.await
                && failure.is_none()
            {
                failure = Some(PijError::Adapter {
                    adapter: "daemon/federation".to_string(),
                    message: format!("worker task did not stop cleanly: {error}"),
                });
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

/// A remote-send refusal or runtime failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FederationSendError {
    /// The request names no configured remote destination.
    Refused(String),
    /// Persistence, serialization, clock, or audit failure.
    Runtime(PijError),
}

impl fmt::Display for FederationSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(message) => formatter.write_str(message),
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FederationSendError {}

#[derive(Serialize)]
struct RemoteOutcome<'a> {
    msg_id: &'a str,
    outcome: &'a DeliveryOutcome,
    transport: &'static str,
    peer: &'a str,
}

#[derive(Serialize)]
struct RemoteRefusal<'a> {
    msg_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer: Option<&'a str>,
    reason: &'a str,
    /// The peer's structured refusal details, relayed verbatim so a sender can
    /// decode (for example) a cold wake's code and facts.
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<&'a serde_json::Value>,
}

fn refusal_reason<T>(envelope: &Envelope<T>) -> String {
    envelope
        .meta
        .clone()
        .unwrap_or_else(|| "peer permanently refused the remote send".to_string())
}

fn random_jitter(base: Duration) -> Result<Duration> {
    let ceiling_ms = (base.as_millis() / 4).max(1);
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| PijError::Adapter {
        adapter: "daemon/federation".to_string(),
        message: format!("could not generate retry jitter: {error}"),
    })?;
    let jitter_ms = u128::from(u64::from_be_bytes(bytes)) % (ceiling_ms + 1);
    let jitter_ms = u64::try_from(jitter_ms).unwrap_or(u64::MAX);
    Ok(base + Duration::from_millis(jitter_ms))
}

fn system_time_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/federation".to_string(),
            message: format!("system clock is before the Unix epoch: {error}"),
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| PijError::Adapter {
        adapter: "daemon/federation".to_string(),
        message: "system time does not fit in the event timestamp".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use pij_core::config::PeerDefinition;
    use pij_core::model::Destination;
    use pij_core::ports::Spine;
    use pij_testkit::fakes::{FakeQueue, FakeSpine};

    use super::{
        FederationPolicy, FederationService, PeerConfigError, PeerTable, ResolvedPeer, WorkerStep,
    };
    use crate::events::EventBus;
    use crate::http::SendRequest;

    fn peer(alias: &str, url: String) -> PeerDefinition {
        PeerDefinition {
            alias: alias.to_string(),
            url,
            key: format!("key-{alias}"),
        }
    }

    fn policy() -> FederationPolicy {
        FederationPolicy {
            poll_interval: Duration::from_secs(1),
            max_retry_delay: Duration::from_secs(5 * 60),
            event_buffer_capacity: 16,
            first_attempt_wait: Duration::from_millis(50),
        }
    }

    fn request(machine: &str) -> SendRequest {
        SendRequest {
            fyi: false,
            force: false,
            reason: None,
            from: "alice".into(),
            to: Destination {
                seat: "bob".into(),
                machine: Some(machine.to_string()),
            },
            body: "hello".to_string(),
            msg_id: "m-1".to_string(),
            from_machine: None,
            in_reply_to: None,
        }
    }

    fn service(queue: Arc<FakeQueue>, spine: Arc<FakeSpine>, url: String) -> FederationService {
        let bus = Arc::new(EventBus::new(spine, 16).expect("event bus"));
        FederationService::with_sources(
            "desktop".to_string(),
            [peer("laptop", url)],
            queue,
            bus,
            policy(),
            Arc::new(|| Ok(1234)),
            Arc::new(Ok),
        )
        .expect("federation")
    }

    /// Review round 2 — ONE msg_id to TWO peers is TWO rows.
    ///
    /// The schema-7 dedupe fix scoped `(kind, dedupe_key)`, and federation stamped
    /// every peer with one shared kind, so the user-reachable loss survived the
    /// migration for exactly the rows the migration's own comment claimed to
    /// cover: `--msg-id m1 --to bob@laptop` then `--to bob@desktop` collapsed, and
    /// the second caller was told `Queued` for a row bound to the first machine.
    ///
    /// Mutation witness: make `remote_send_kind` return a constant and this fails
    /// with one row where two are required.
    #[tokio::test]
    async fn one_msg_id_to_two_peers_is_two_rows() {
        let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
        let bus = Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 16).expect("event bus"));
        let service = FederationService::with_sources(
            "desktop".to_string(),
            [
                peer("laptop", "http://laptop".to_string()),
                peer("tower", "http://tower".to_string()),
            ],
            Arc::clone(&queue) as Arc<dyn pij_core::ports::Queue>,
            bus,
            policy(),
            Arc::new(|| Ok(1234)),
            Arc::new(Ok),
        )
        .expect("federation");

        for machine in ["laptop", "tower"] {
            service
                .enqueue_remote(&request(machine))
                .await
                .expect("a remote send to a configured peer");
        }

        assert_eq!(
            queue.live_len(),
            2,
            "the same msg_id addressed to two MACHINES is two messages"
        );
    }

    #[test]
    fn unqualified_destination_is_always_local() {
        let table = PeerTable::new("desktop", [peer("laptop", "http://peer".to_string())])
            .expect("valid peers");
        assert!(matches!(
            table
                .resolve(&Destination::local("alice"))
                .expect("resolve"),
            ResolvedPeer::Local
        ));
    }

    #[test]
    fn duplicate_peer_alias_is_refused_before_collection_erases_it() {
        let error = PeerTable::new(
            "desktop",
            [
                peer("laptop", "http://one".to_string()),
                peer("laptop", "http://two".to_string()),
            ],
        )
        .err()
        .expect("duplicate must fail");
        assert_eq!(error, PeerConfigError::DuplicateAlias("laptop".to_string()));
    }

    #[test]
    fn peer_alias_equal_to_local_alias_is_refused() {
        let error = PeerTable::new("desktop", [peer("desktop", "http://peer".to_string())])
            .err()
            .expect("collision must fail");
        assert_eq!(error, PeerConfigError::LocalAlias("desktop".to_string()));
    }

    #[tokio::test]
    async fn enqueue_claims_only_durable_local_queueing() {
        let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
        let spine = Arc::new(FakeSpine::new());
        let service = service(queue.clone(), spine.clone(), "http://peer".to_string());
        let receipt = service
            .enqueue_remote(&request("laptop"))
            .await
            .expect("enqueue");
        assert_eq!(
            receipt.outcome,
            pij_core::model::DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            }
        );
        assert_eq!(queue.live_len(), 1);
        let events = spine
            .tail(None, pij_core::model::Seq(0))
            .await
            .expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "delivery.outcome");
    }

    #[tokio::test]
    async fn network_failure_retries_with_persisted_exponential_attempt() {
        let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
        let spine = Arc::new(FakeSpine::new());
        let service = service(queue.clone(), spine, "http://127.0.0.1:1".to_string());
        service
            .enqueue_remote(&request("laptop"))
            .await
            .expect("enqueue");
        assert_eq!(
            service.process_one().await.expect("worker"),
            WorkerStep::Retried
        );
        let retries = queue.retried();
        let [(job_id, delay)] = retries.as_slice() else {
            panic!("one retry expected");
        };
        assert_eq!(*delay, Duration::from_secs(1));
        assert_eq!(queue.attempts(*job_id), 1);
        assert_eq!(
            service.process_one().await.expect("not due"),
            WorkerStep::Idle
        );
        queue.advance(*delay);
        assert_eq!(
            service.process_one().await.expect("worker"),
            WorkerStep::Retried
        );
        assert_eq!(queue.retried()[1].1, Duration::from_secs(2));
    }

    #[test]
    fn retry_delay_is_exponential_and_caps_at_five_minutes() {
        let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
        let spine = Arc::new(FakeSpine::new());
        let service = service(queue, spine, "http://peer".to_string());
        assert_eq!(
            service.retry_delay(0).expect("delay"),
            Duration::from_secs(1)
        );
        assert_eq!(
            service.retry_delay(1).expect("delay"),
            Duration::from_secs(2)
        );
        assert_eq!(
            service.retry_delay(8).expect("delay"),
            Duration::from_secs(256)
        );
        assert_eq!(
            service.retry_delay(9).expect("delay"),
            Duration::from_secs(300)
        );
        assert_eq!(
            service.retry_delay(u32::MAX).expect("delay"),
            Duration::from_secs(300)
        );
    }

    #[tokio::test]
    async fn a_worker_started_with_stop_already_set_never_claims() {
        let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
        let spine = Arc::new(FakeSpine::new());
        let service = Arc::new(service(
            Arc::clone(&queue),
            spine,
            "http://127.0.0.1:1".to_string(),
        ));
        service
            .enqueue_remote(&request("laptop"))
            .await
            .expect("enqueue before start");
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(true);

        let worker = Arc::clone(&service).start_with_shutdown(shutdown, shutdown_rx);
        worker.shutdown().await.expect("shutdown");

        assert_eq!(queue.live_len(), 1, "a post-stop worker must not claim");
        assert!(
            queue.retried().is_empty(),
            "a retry proves the stopped worker claimed and performed network IO"
        );
    }

    #[tokio::test]
    async fn shutdown_wins_when_wake_and_stop_are_both_ready() {
        for _ in 0..64 {
            let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
            let spine = Arc::new(FakeSpine::new());
            let service = Arc::new(service(
                Arc::clone(&queue),
                spine,
                "http://127.0.0.1:1".to_string(),
            ));
            let worker = Arc::clone(&service).start();

            // The first empty claim puts the worker in its idle select. Enqueue
            // makes wake ready; shutdown makes stop ready before the worker is
            // scheduled again. Stop must win and leave the row pending.
            tokio::task::yield_now().await;
            service
                .enqueue_remote(&request("laptop"))
                .await
                .expect("enqueue");
            worker.shutdown().await.expect("shutdown");
            assert_eq!(queue.live_len(), 1, "no post-stop claim may start");
        }
    }

    #[tokio::test]
    async fn idle_worker_shutdown_leaves_no_orphan_task() {
        let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
        let spine = Arc::new(FakeSpine::new());
        let worker = Arc::new(service(queue, spine, "http://peer".to_string())).start();
        worker.shutdown().await.expect("shutdown");
    }
}
