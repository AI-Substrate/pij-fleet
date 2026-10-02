use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::{
    Json, Router,
    routing::{get, post},
};
use pij_core::config::Config;
use pij_core::delivery::delivery_kind;
use pij_core::error::{PijError, Result};
use pij_core::model::{DeliveryOrigin, Job, JobId, Msg, Outcome, SeatId, Seq};
use pij_core::ports::{DeliveryAck, DeliveryEnqueue, Queue, Spine};
use pij_sidecars::background::{self, BgRequest, BgStatus, BgWorker, group_pids};
use pij_sidecars::chore::{self, ChoreRequest, ChoreStatus, ChoreWorker};
use pij_sidecars::telegram::{self, TelegramConfig, TelegramSend, TelegramWorker};
use pij_store::SqliteQueue;
use pij_testkit::{
    FreshStore,
    fakes::{FakeRegistry, FakeSpine},
};

async fn sqlite_queue(fresh: &FreshStore) -> Arc<SqliteQueue> {
    let config = Config::default();
    Arc::new(
        SqliteQueue::new(
            pij_store::open(&fresh.path())
                .await
                .expect("open test store"),
            config.claim_lease_secs,
            config.delivered_id_capacity,
        )
        .expect("queue policy"),
    )
}

fn temp_path(fresh: &FreshStore, name: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", fresh.path(), name))
}

async fn wait_for(mut condition: impl FnMut() -> bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct FailFirstClaim {
    inner: Arc<dyn Queue>,
    fail: AtomicBool,
}

impl FailFirstClaim {
    fn new(inner: Arc<dyn Queue>) -> Self {
        Self {
            inner,
            fail: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl Queue for FailFirstClaim {
    async fn enqueue(&self, job: Job) -> Result<JobId> {
        self.inner.enqueue(job).await
    }
    async fn enqueue_delivery(&self, job: Job) -> Result<DeliveryEnqueue> {
        self.inner.enqueue_delivery(job).await
    }
    async fn claim(&self, kinds: &[String], worker: &str) -> Result<Option<(JobId, Job)>> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(PijError::Adapter {
                adapter: "test/transient".to_string(),
                message: "injected transient claim failure".to_string(),
            });
        }
        self.inner.claim(kinds, worker).await
    }
    async fn peek(&self, kinds: &[String]) -> Result<Option<(JobId, Job)>> {
        self.inner.peek(kinds).await
    }
    async fn claimed_delivery(&self, job: JobId) -> Result<Option<Job>> {
        self.inner.claimed_delivery(job).await
    }
    async fn terminal_delivery_state(
        &self,
        job: JobId,
        recipient: &SeatId,
    ) -> Result<Option<&'static str>> {
        self.inner.terminal_delivery_state(job, recipient).await
    }
    async fn ack(&self, job: JobId, outcome: Outcome) -> Result<()> {
        self.inner.ack(job, outcome).await
    }
    async fn ack_delivery(&self, job: JobId, origin: DeliveryOrigin) -> Result<DeliveryAck> {
        self.inner.ack_delivery(job, origin).await
    }
    async fn note_delivered(
        &self,
        recipient: &SeatId,
        msg_id: &str,
        origin: DeliveryOrigin,
    ) -> Result<Option<DeliveryOrigin>> {
        self.inner.note_delivered(recipient, msg_id, origin).await
    }
    async fn forget_delivered(&self, recipient: &SeatId, msg_id: &str) -> Result<()> {
        self.inner.forget_delivered(recipient, msg_id).await
    }
    async fn retry(&self, job: JobId, delay: Duration) -> Result<()> {
        self.inner.retry(job, delay).await
    }
    async fn record_delivery_deferral(
        &self,
        job: JobId,
        reason: &str,
        draft_sha: Option<&str>,
        at: u64,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<Vec<pij_core::model::Event>> {
        self.inner
            .record_delivery_deferral(job, reason, draft_sha, at, spine)
            .await
    }
    async fn delivery_deferrals(
        &self,
        recipient: &SeatId,
    ) -> Result<Vec<pij_core::model::DeliveryDeferral>> {
        self.inner.delivery_deferrals(recipient).await
    }
    async fn defer(&self, job: JobId, delay: Duration) -> Result<pij_core::ports::DeferOutcome> {
        self.inner.defer(job, delay).await
    }
    async fn release_deferred(&self, job: JobId) -> Result<pij_core::ports::ReleaseOutcome> {
        self.inner.release_deferred(job).await
    }
    async fn hold_fyi(
        &self,
        fyi: &pij_core::fyi::HeldFyi,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<Vec<pij_core::model::Event>> {
        self.inner.hold_fyi(fyi, spine).await
    }
    async fn claim_fyis(
        &self,
        recipient: &SeatId,
        via: &str,
        at: u64,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<(Vec<pij_core::fyi::HeldFyi>, Vec<pij_core::model::Event>)> {
        self.inner.claim_fyis(recipient, via, at, spine).await
    }
    async fn enqueue_delivery_carrying_fyis(
        &self,
        job: pij_core::model::Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<(
        pij_core::ports::DeliveryEnqueue,
        Vec<pij_core::model::Event>,
    )> {
        self.inner
            .enqueue_delivery_carrying_fyis(job, via, at, attach, spine)
            .await
    }
    async fn pending_fyi_count(&self, recipient: &SeatId) -> Result<u64> {
        self.inner.pending_fyi_count(recipient).await
    }
    async fn enqueue_fyi_flush(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        spine: &dyn Spine,
    ) -> Result<(Option<DeliveryEnqueue>, Vec<pij_core::model::Event>)> {
        self.inner
            .enqueue_fyi_flush(job, via, at, attach, spine)
            .await
    }
    async fn read_claimed_fyis(
        &self,
        recipient: &SeatId,
        claimed_at_ms: u64,
    ) -> Result<Vec<pij_core::fyi::HeldFyi>> {
        self.inner.read_claimed_fyis(recipient, claimed_at_ms).await
    }
    async fn recover_native_delivery(&self, job: JobId, recipient: &SeatId) -> Result<bool> {
        self.inner.recover_native_delivery(job, recipient).await
    }
    async fn claim_extension(
        &self,
        kinds: &[String],
        worker: &str,
        lease: pij_core::ports::ExtensionLease,
        at: u64,
        spine: &dyn pij_core::ports::Spine,
        recovery_allowed: bool,
    ) -> pij_core::error::Result<pij_core::ports::ExtensionClaim> {
        self.inner
            .claim_extension(kinds, worker, lease, at, spine, recovery_allowed)
            .await
    }
    async fn heartbeat_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
    ) -> Result<bool> {
        self.inner.heartbeat_delivery(job, recipient, attempt).await
    }
    async fn peek_parked(
        &self,
        kinds: &[String],
    ) -> pij_core::error::Result<Vec<pij_core::ports::ParkedDelivery>> {
        self.inner.peek_parked(kinds).await
    }
    async fn park_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
        evidence: &pij_core::ports::ParkingEvidence<'_>,
        spine: &dyn pij_core::ports::Spine,
    ) -> pij_core::error::Result<(Option<Job>, Vec<pij_core::model::Event>)> {
        self.inner
            .park_delivery(job, recipient, attempt, evidence, spine)
            .await
    }
}

#[tokio::test]
async fn chore_run_never_advances_baseline_ack_does_and_failed_probe_stays_visible() {
    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let worker = ChoreWorker::new(queue_port, temp_path(&fresh, "chores.json")).expect("worker");
    let target = SeatId::from("pij-test");

    for (name, probe) in [("healthy", "printf alpha"), ("broken", "exit 7")] {
        let request = ChoreRequest::Add {
            name: name.to_string(),
            probe: probe.to_string(),
            target: target.clone(),
        };
        queue
            .enqueue(chore::job(&request, &format!("add-{name}")).expect("job"))
            .await
            .expect("enqueue");
    }
    assert_eq!(
        worker.run_once(9).await.expect("add pass").count,
        2,
        "reported count is observed, not configured limit"
    );

    queue
        .enqueue(
            chore::job(
                &ChoreRequest::Run {
                    target: target.clone(),
                },
                "run-1",
            )
            .expect("job"),
        )
        .await
        .expect("enqueue");
    worker.run_once(9).await.expect("run");
    let first = worker.entries().expect("entries");
    assert_eq!(
        first["healthy"].baseline, None,
        "run must not advance the baseline"
    );
    assert_eq!(first["healthy"].pending.as_deref(), Some("alpha"));
    assert!(
        matches!(first["broken"].status, ChoreStatus::NotProbeable { .. }),
        "a failed probe stays in the roster as NOT-PROBEABLE"
    );

    queue
        .enqueue(
            chore::job(
                &ChoreRequest::Run {
                    target: target.clone(),
                },
                "run-2",
            )
            .expect("job"),
        )
        .await
        .expect("enqueue");
    worker.run_once(9).await.expect("rerun");
    assert_eq!(
        worker.entries().expect("entries")["healthy"].baseline,
        None,
        "unacked delta must re-surface without baseline mutation"
    );

    queue
        .enqueue(
            chore::job(
                &ChoreRequest::Ack {
                    name: "healthy".to_string(),
                    target,
                },
                "ack",
            )
            .expect("job"),
        )
        .await
        .expect("enqueue");
    worker.run_once(9).await.expect("ack");
    let acked = worker.entries().expect("entries");
    assert_eq!(acked["healthy"].baseline.as_deref(), Some("alpha"));
    assert_eq!(acked["healthy"].pending, None);
}

#[tokio::test]
async fn bg_cancel_observes_the_full_process_group_dead_before_cancelled_receipt() {
    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let worker = BgWorker::new(queue_port, temp_path(&fresh, "bg")).expect("worker");
    let target = SeatId::from("pij-test");

    for run in 0..3 {
        let id = format!("tree-{run}");
        let command = if run == 0 {
            // TERM is deliberately ignored by the shell and its child. This
            // leg can finish only through the shipped SIGKILL escalation.
            "trap '' TERM; while :; do sleep 30; done"
        } else {
            "trap 'sleep 1; exit' TERM; while :; do sleep 30; done"
        };
        let start = BgRequest::Start {
            id: id.clone(),
            title: "cancellation witness".to_string(),
            command: command.to_string(),
            target: target.clone(),
        };
        queue
            .enqueue(background::job(&start, &format!("start-{run}")).expect("job"))
            .await
            .expect("enqueue");
        worker.run_once(7).await.expect("start");
        let record = worker.load(&id).expect("record");
        wait_for(
            || group_pids(record.pgid).is_ok_and(|pids| pids.len() >= 2),
            Duration::from_secs(2),
        )
        .await;
        let before = group_pids(record.pgid).expect("observe child tree");
        assert!(
            before.contains(&record.pgid),
            "spawned shell pid must equal its process-group id; inheriting the daemon group makes cancellation kill the daemon"
        );

        let cancel = BgRequest::Cancel {
            id: id.clone(),
            target: target.clone(),
        };
        queue
            .enqueue(background::job(&cancel, &format!("cancel-{run}")).expect("job"))
            .await
            .expect("enqueue");
        worker.run_once(7).await.expect("cancel");

        let receipt = worker.load(&id).expect("cancelled receipt");
        assert_eq!(receipt.status, BgStatus::Cancelled);
        assert!(
            receipt.observed_pids.len() >= 2,
            "witness must observe a child tree, not one sleep"
        );
        assert!(
            group_pids(receipt.pgid)
                .expect("observe after receipt")
                .is_empty(),
            "cancelled receipt must not exist before every live group member is gone"
        );
    }
}

#[tokio::test]
async fn all_three_shipped_loops_survive_a_transient_claim_failure() {
    // Chore loop.
    let chore_fresh = FreshStore::new();
    let chore_queue = sqlite_queue(&chore_fresh).await;
    chore_queue
        .enqueue(
            chore::job(
                &ChoreRequest::Add {
                    name: "loop".to_string(),
                    probe: "printf ok".to_string(),
                    target: SeatId::from("pij-test"),
                },
                "chore-loop",
            )
            .expect("job"),
        )
        .await
        .expect("enqueue");
    let inner: Arc<dyn Queue> = chore_queue.clone();
    let flaky: Arc<dyn Queue> = Arc::new(FailFirstClaim::new(inner));
    let chore_worker = Arc::new(
        ChoreWorker::new(flaky, temp_path(&chore_fresh, "loop-chores.json")).expect("worker"),
    );
    let chore_loop = Arc::clone(&chore_worker)
        .start(Duration::from_millis(10), 5)
        .expect("loop");
    wait_for(
        || {
            chore_worker
                .entries()
                .is_ok_and(|entries| entries.contains_key("loop"))
        },
        Duration::from_secs(2),
    )
    .await;
    chore_loop.shutdown().await;

    // Background loop.
    let bg_fresh = FreshStore::new();
    let bg_queue = sqlite_queue(&bg_fresh).await;
    bg_queue
        .enqueue(
            background::job(
                &BgRequest::Start {
                    id: "loop-bg".to_string(),
                    title: "loop".to_string(),
                    command: "printf done".to_string(),
                    target: SeatId::from("pij-test"),
                },
                "bg-loop",
            )
            .expect("job"),
        )
        .await
        .expect("enqueue");
    let inner: Arc<dyn Queue> = bg_queue.clone();
    let flaky: Arc<dyn Queue> = Arc::new(FailFirstClaim::new(inner));
    let bg_worker =
        Arc::new(BgWorker::new(flaky, temp_path(&bg_fresh, "loop-bg-state")).expect("worker"));
    let bg_loop = Arc::clone(&bg_worker)
        .start(Duration::from_millis(10), 5)
        .expect("loop");
    wait_for(
        || {
            bg_worker
                .load("loop-bg")
                .is_ok_and(|record| record.status == BgStatus::Done)
        },
        Duration::from_secs(2),
    )
    .await;
    bg_loop.shutdown().await;

    // Telegram loop over a real local HTTP surface.
    async fn send_message() -> Json<serde_json::Value> {
        Json(serde_json::json!({"ok": true, "result": {"message_id": 1}}))
    }
    async fn updates() -> Json<serde_json::Value> {
        Json(serde_json::json!({"ok": true, "result": []}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/botTOKEN/sendMessage", post(send_message))
                .route("/botTOKEN/getUpdates", get(updates)),
        )
        .await
        .expect("serve fixture")
    });
    let telegram_fresh = FreshStore::new();
    let telegram_queue = sqlite_queue(&telegram_fresh).await;
    telegram_queue
        .enqueue(
            telegram::send_job(&TelegramSend {
                from: SeatId::from("pij-test"),
                body: "hello".to_string(),
                msg_id: "telegram-loop".to_string(),
                chat_id: None,
            })
            .expect("job"),
        )
        .await
        .expect("enqueue");
    let inner: Arc<dyn Queue> = telegram_queue.clone();
    let flaky: Arc<dyn Queue> = Arc::new(FailFirstClaim::new(inner));
    let spine = Arc::new(FakeSpine::new());
    let spine_port: Arc<dyn Spine> = spine.clone();
    let telegram_worker = Arc::new(
        TelegramWorker::new(
            flaky,
            spine_port,
            Arc::new(FakeRegistry::new()),
            TelegramConfig {
                token: "TOKEN".to_string(),
                allowed_user_ids: vec![7],
                chat_id: "42".to_string(),
                api_root: format!("http://{addr}"),
            },
            temp_path(&telegram_fresh, "telegram.lock"),
        )
        .expect("worker"),
    );
    let telegram_loop = Arc::clone(&telegram_worker)
        .start(Duration::from_millis(10), 5)
        .expect("loop");
    wait_for(
        || {
            pij_testkit::block_on(spine.tail(None, Seq(0)))
                .is_ok_and(|events| events.iter().any(|event| event.kind == "telegram.binding"))
        },
        Duration::from_secs(2),
    )
    .await;
    telegram_loop.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn telegram_reply_uses_only_the_persisted_outbound_binding_and_refuses_when_unbound() {
    async fn send_message() -> Json<serde_json::Value> {
        Json(serde_json::json!({"ok": true, "result": {"message_id": 41}}))
    }
    async fn updates() -> Json<serde_json::Value> {
        Json(serde_json::json!({"ok": true, "result": [{
            "update_id": 77,
            "message": {"from": {"id": 7}, "chat": {"id": 42}, "text": "Jordan reply"}
        }]}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/botTOKEN/sendMessage", post(send_message))
                .route("/botTOKEN/getUpdates", get(updates)),
        )
        .await
        .expect("serve fixture")
    });

    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let spine = Arc::new(FakeSpine::new());
    let spine_port: Arc<dyn Spine> = spine.clone();
    let worker = TelegramWorker::new(
        queue_port,
        spine_port,
        Arc::new(FakeRegistry::new()),
        TelegramConfig {
            token: "TOKEN".to_string(),
            allowed_user_ids: vec![7],
            chat_id: "42".to_string(),
            api_root: format!("http://{addr}"),
        },
        temp_path(&fresh, "bound-telegram.lock"),
    )
    .expect("worker");
    queue
        .enqueue(
            telegram::send_job(&TelegramSend {
                from: SeatId::from("pij-target"),
                body: "live fire".to_string(),
                msg_id: "outbound-1".to_string(),
                chat_id: None,
            })
            .expect("job"),
        )
        .await
        .expect("enqueue");
    assert_eq!(
        worker.run_once(17).await.expect("send and poll").count,
        1,
        "observed count must differ from configured limit"
    );
    let (_, delivery) = queue
        .claim(
            &[delivery_kind(&SeatId::from("pij-target"))],
            "assert-reply",
        )
        .await
        .expect("claim")
        .expect("reply delivery");
    let message: Msg = serde_json::from_str(&delivery.payload).expect("message payload");
    assert_eq!(message.from, SeatId::from("pij-telegram"));
    assert_eq!(message.to, SeatId::from("pij-target"));
    assert_eq!(message.body, "Jordan reply");
    let events = spine.tail(None, Seq(0)).await.expect("spine");
    assert!(
        events.iter().any(|event| event.kind == "telegram.binding"),
        "outbound must persist binding before inbound can route"
    );
    assert!(
        events.iter().any(|event| {
            event.kind == "telegram.inbound-enqueued"
                && event.seat.as_ref() == Some(&SeatId::from("pij-target"))
                && event.payload.contains("Jordan reply")
        }),
        "successful inbound turn must be a new, attributable spine fact"
    );
    drop(worker);

    let unbound = FreshStore::new();
    let queue = sqlite_queue(&unbound).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let spine = Arc::new(FakeSpine::new());
    let spine_port: Arc<dyn Spine> = spine.clone();
    let worker = TelegramWorker::new(
        queue_port,
        spine_port,
        Arc::new(FakeRegistry::new()),
        TelegramConfig {
            token: "TOKEN".to_string(),
            allowed_user_ids: vec![7],
            chat_id: "42".to_string(),
            api_root: format!("http://{addr}"),
        },
        temp_path(&unbound, "unbound-telegram.lock"),
    )
    .expect("worker");
    assert_eq!(worker.run_once(17).await.expect("unbound poll").count, 0);
    let events = spine.tail(None, Seq(0)).await.expect("spine");
    assert!(
        events
            .iter()
            .any(|event| event.kind == "telegram.inbound-refused"
                && event.payload.contains("no persisted outbound binding"))
    );
    assert!(
        queue
            .claim(&[delivery_kind(&SeatId::from("pij-target"))], "assert-none")
            .await
            .expect("claim")
            .is_none(),
        "unbound inbound must not guess a target"
    );
    server.abort();
}

struct RefuseFirstDelivery {
    inner: Arc<dyn Queue>,
    refuse: AtomicBool,
}

#[async_trait]
impl Queue for RefuseFirstDelivery {
    async fn enqueue(&self, job: Job) -> Result<JobId> {
        self.inner.enqueue(job).await
    }
    async fn enqueue_delivery(&self, job: Job) -> Result<DeliveryEnqueue> {
        if self.refuse.swap(false, Ordering::SeqCst) {
            return Err(PijError::Adapter {
                adapter: "test/refuse".to_string(),
                message: "injected turn-enqueue failure".to_string(),
            });
        }
        self.inner.enqueue_delivery(job).await
    }
    async fn claim(&self, kinds: &[String], worker: &str) -> Result<Option<(JobId, Job)>> {
        self.inner.claim(kinds, worker).await
    }
    async fn peek(&self, kinds: &[String]) -> Result<Option<(JobId, Job)>> {
        self.inner.peek(kinds).await
    }
    async fn claimed_delivery(&self, job: JobId) -> Result<Option<Job>> {
        self.inner.claimed_delivery(job).await
    }
    async fn terminal_delivery_state(
        &self,
        job: JobId,
        recipient: &SeatId,
    ) -> Result<Option<&'static str>> {
        self.inner.terminal_delivery_state(job, recipient).await
    }
    async fn ack(&self, job: JobId, outcome: Outcome) -> Result<()> {
        self.inner.ack(job, outcome).await
    }
    async fn ack_delivery(&self, job: JobId, origin: DeliveryOrigin) -> Result<DeliveryAck> {
        self.inner.ack_delivery(job, origin).await
    }
    async fn note_delivered(
        &self,
        recipient: &SeatId,
        msg_id: &str,
        origin: DeliveryOrigin,
    ) -> Result<Option<DeliveryOrigin>> {
        self.inner.note_delivered(recipient, msg_id, origin).await
    }
    async fn forget_delivered(&self, recipient: &SeatId, msg_id: &str) -> Result<()> {
        self.inner.forget_delivered(recipient, msg_id).await
    }
    async fn retry(&self, job: JobId, delay: Duration) -> Result<()> {
        self.inner.retry(job, delay).await
    }
    async fn record_delivery_deferral(
        &self,
        job: JobId,
        reason: &str,
        draft_sha: Option<&str>,
        at: u64,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<Vec<pij_core::model::Event>> {
        self.inner
            .record_delivery_deferral(job, reason, draft_sha, at, spine)
            .await
    }
    async fn delivery_deferrals(
        &self,
        recipient: &SeatId,
    ) -> Result<Vec<pij_core::model::DeliveryDeferral>> {
        self.inner.delivery_deferrals(recipient).await
    }
    async fn defer(&self, job: JobId, delay: Duration) -> Result<pij_core::ports::DeferOutcome> {
        self.inner.defer(job, delay).await
    }
    async fn release_deferred(&self, job: JobId) -> Result<pij_core::ports::ReleaseOutcome> {
        self.inner.release_deferred(job).await
    }
    async fn hold_fyi(
        &self,
        fyi: &pij_core::fyi::HeldFyi,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<Vec<pij_core::model::Event>> {
        self.inner.hold_fyi(fyi, spine).await
    }
    async fn claim_fyis(
        &self,
        recipient: &SeatId,
        via: &str,
        at: u64,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<(Vec<pij_core::fyi::HeldFyi>, Vec<pij_core::model::Event>)> {
        self.inner.claim_fyis(recipient, via, at, spine).await
    }
    async fn enqueue_delivery_carrying_fyis(
        &self,
        job: pij_core::model::Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        spine: &dyn pij_core::ports::Spine,
    ) -> Result<(
        pij_core::ports::DeliveryEnqueue,
        Vec<pij_core::model::Event>,
    )> {
        self.inner
            .enqueue_delivery_carrying_fyis(job, via, at, attach, spine)
            .await
    }
    async fn pending_fyi_count(&self, recipient: &SeatId) -> Result<u64> {
        self.inner.pending_fyi_count(recipient).await
    }
    async fn enqueue_fyi_flush(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        spine: &dyn Spine,
    ) -> Result<(Option<DeliveryEnqueue>, Vec<pij_core::model::Event>)> {
        self.inner
            .enqueue_fyi_flush(job, via, at, attach, spine)
            .await
    }
    async fn read_claimed_fyis(
        &self,
        recipient: &SeatId,
        claimed_at_ms: u64,
    ) -> Result<Vec<pij_core::fyi::HeldFyi>> {
        self.inner.read_claimed_fyis(recipient, claimed_at_ms).await
    }
    async fn recover_native_delivery(&self, job: JobId, recipient: &SeatId) -> Result<bool> {
        self.inner.recover_native_delivery(job, recipient).await
    }
    async fn claim_extension(
        &self,
        kinds: &[String],
        worker: &str,
        lease: pij_core::ports::ExtensionLease,
        at: u64,
        spine: &dyn pij_core::ports::Spine,
        recovery_allowed: bool,
    ) -> pij_core::error::Result<pij_core::ports::ExtensionClaim> {
        self.inner
            .claim_extension(kinds, worker, lease, at, spine, recovery_allowed)
            .await
    }
    async fn heartbeat_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
    ) -> Result<bool> {
        self.inner.heartbeat_delivery(job, recipient, attempt).await
    }
    async fn peek_parked(
        &self,
        kinds: &[String],
    ) -> pij_core::error::Result<Vec<pij_core::ports::ParkedDelivery>> {
        self.inner.peek_parked(kinds).await
    }
    async fn park_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
        evidence: &pij_core::ports::ParkingEvidence<'_>,
        spine: &dyn pij_core::ports::Spine,
    ) -> pij_core::error::Result<(Option<Job>, Vec<pij_core::model::Event>)> {
        self.inner
            .park_delivery(job, recipient, attempt, evidence, spine)
            .await
    }
}

struct FailBindingSpine {
    inner: Arc<dyn Spine>,
    fail: AtomicBool,
}

#[async_trait]
impl Spine for FailBindingSpine {
    async fn append(&self, event: pij_core::model::Event) -> Result<Seq> {
        if event.kind == "telegram.binding" && self.fail.swap(false, Ordering::SeqCst) {
            return Err(PijError::Adapter {
                adapter: "test/spine".to_string(),
                message: "injected binding failure".to_string(),
            });
        }
        self.inner.append(event).await
    }
    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<pij_core::model::Event>> {
        self.inner.tail(seat, since).await
    }
    async fn latest_matching(
        &self,
        seat: &SeatId,
        kinds: &[&str],
    ) -> Result<Option<pij_core::model::Event>> {
        self.inner.latest_matching(seat, kinds).await
    }
    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<pij_core::model::Event>> {
        self.inner.latest_matching_message(seat, kind, msg_id).await
    }
}

#[derive(Default)]
struct TelegramApiState {
    offsets: Mutex<Vec<String>>,
    sends: AtomicUsize,
}

async fn recorded_send(
    axum::extract::State(state): axum::extract::State<Arc<TelegramApiState>>,
) -> Json<serde_json::Value> {
    state.sends.fetch_add(1, Ordering::SeqCst);
    Json(serde_json::json!({"ok": true, "result": {"message_id": 41}}))
}

async fn retained_update(
    axum::extract::State(state): axum::extract::State<Arc<TelegramApiState>>,
    uri: axum::http::Uri,
) -> Json<serde_json::Value> {
    let offset = uri
        .query()
        .and_then(|query| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("offset="))
        })
        .unwrap_or("0")
        .to_string();
    state
        .offsets
        .lock()
        .expect("offset mutex")
        .push(offset.clone());
    let result = if offset.parse::<i64>().unwrap_or_default() <= 77 {
        serde_json::json!([{"update_id":77,"message":{"from":{"id":7},"chat":{"id":42},"text":"retained reply"}}])
    } else {
        serde_json::json!([])
    };
    Json(serde_json::json!({"ok": true, "result": result}))
}

async fn telegram_fixture(state: Arc<TelegramApiState>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/botTOKEN/sendMessage", post(recorded_send))
                .route("/botTOKEN/getUpdates", get(retained_update))
                .with_state(state),
        )
        .await
        .expect("serve fixture")
    });
    (format!("http://{addr}"), server)
}

#[tokio::test]
async fn failed_inbound_delivery_keeps_the_update_unconfirmed_and_cursor_survives_restart() {
    let api = Arc::new(TelegramApiState::default());
    let (api_root, server) = telegram_fixture(Arc::clone(&api)).await;
    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let inner: Arc<dyn Queue> = queue.clone();
    let refusing: Arc<dyn Queue> = Arc::new(RefuseFirstDelivery {
        inner,
        refuse: AtomicBool::new(true),
    });
    let spine = Arc::new(FakeSpine::new());
    let spine_port: Arc<dyn Spine> = spine.clone();
    let config = TelegramConfig {
        token: "TOKEN".to_string(),
        allowed_user_ids: vec![7],
        chat_id: "42".to_string(),
        api_root,
    };
    let lock = temp_path(&fresh, "cursor.lock");
    let worker = TelegramWorker::new(
        refusing,
        spine_port,
        Arc::new(FakeRegistry::new()),
        config.clone(),
        lock.clone(),
    )
    .expect("worker");
    let lock_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lock).expect("read bridge lock"))
            .expect("parse bridge lock");
    assert!(lock_json["startedAt"].is_string());
    assert!(
        lock_json.get("startedAtMs").is_none(),
        "legacy treats the old Rust-only shape as corrupt and steals a live lock"
    );
    queue
        .enqueue(
            telegram::send_job(&TelegramSend {
                from: SeatId::from("pij-target"),
                body: "outbound".to_string(),
                msg_id: "bind".to_string(),
                chat_id: None,
            })
            .expect("job"),
        )
        .await
        .expect("enqueue");
    let first = worker
        .run_once(4)
        .await
        .expect_err("first inbound enqueue must fail");
    assert!(first.to_string().contains("injected turn-enqueue failure"));
    assert!(
        spine
            .tail(None, Seq(0))
            .await
            .expect("events")
            .iter()
            .any(|event| event.kind == "telegram.inbound-delivery-failed")
    );
    worker.run_once(4).await.expect("retry same update");
    assert!(
        spine
            .tail(None, Seq(0))
            .await
            .expect("events")
            .iter()
            .any(|event| event.kind == "telegram.inbound-enqueued"),
        "the successful retry must persist the inbound turn before cursor advance"
    );
    drop(worker);

    let queue_port: Arc<dyn Queue> = queue.clone();
    let spine_port: Arc<dyn Spine> = spine.clone();
    let restarted = TelegramWorker::new(
        queue_port,
        spine_port,
        Arc::new(FakeRegistry::new()),
        config,
        lock,
    )
    .expect("restart");
    restarted.run_once(4).await.expect("restart poll");
    assert_eq!(
        *api.offsets.lock().expect("offsets"),
        ["0", "0", "78"],
        "failed delivery must not confirm update; durable cursor must survive restart"
    );
    server.abort();
}

#[tokio::test]
async fn successful_send_with_failed_binding_is_terminal_and_never_posts_twice() {
    let api = Arc::new(TelegramApiState::default());
    let (api_root, server) = telegram_fixture(Arc::clone(&api)).await;
    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let inner_spine: Arc<dyn Spine> = Arc::new(FakeSpine::new());
    let spine: Arc<dyn Spine> = Arc::new(FailBindingSpine {
        inner: inner_spine,
        fail: AtomicBool::new(true),
    });
    let worker = TelegramWorker::new(
        queue_port,
        spine,
        Arc::new(FakeRegistry::new()),
        TelegramConfig {
            token: "TOKEN".to_string(),
            allowed_user_ids: vec![],
            chat_id: "42".to_string(),
            api_root,
        },
        temp_path(&fresh, "binding-fail.lock"),
    )
    .expect("worker");
    queue
        .enqueue(
            telegram::send_job(&TelegramSend {
                from: SeatId::from("pij-target"),
                body: "once".to_string(),
                msg_id: "send-once".to_string(),
                chat_id: None,
            })
            .expect("job"),
        )
        .await
        .expect("enqueue");
    worker
        .run_once(5)
        .await
        .expect("terminal binding failure is handled");
    worker.run_once(5).await.expect("no retry");
    assert_eq!(
        api.sends.load(Ordering::SeqCst),
        1,
        "a successful external send must never be retried after binding persistence fails"
    );
    assert_eq!(queue.live_len().await.expect("live rows"), 0);
    server.abort();
}

#[tokio::test]
async fn direct_child_failure_survives_a_lingering_grandchild() {
    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let worker =
        BgWorker::new(queue_port, temp_path(&fresh, "bg-lingering-child")).expect("worker");
    queue
        .enqueue(
            background::job(
                &BgRequest::Start {
                    id: "linger".to_string(),
                    title: "linger".to_string(),
                    command: "sleep 1 & echo partial; exit 3".to_string(),
                    target: SeatId::from("pij-test"),
                },
                "start-linger",
            )
            .expect("job"),
        )
        .await
        .expect("enqueue");
    worker.run_once(4).await.expect("start");
    tokio::time::sleep(Duration::from_millis(100)).await;
    worker
        .run_once(4)
        .await
        .expect("observe direct child while group remains");
    let deferred = worker.load("linger").expect("deferred record");
    assert_eq!(deferred.status, BgStatus::Running);
    assert_eq!(
        deferred.exit_success,
        Some(false),
        "observed exit must be durable before the lingering-group guard"
    );
    assert_eq!(deferred.exit_code, Some(3));
    tokio::time::sleep(Duration::from_secs(1)).await;
    worker.run_once(4).await.expect("finish classification");
    let terminal = worker.load("linger").expect("terminal record");
    assert_eq!(terminal.status, BgStatus::Failed);
    assert_eq!(terminal.exit_code, Some(3));
}

#[tokio::test]
async fn telegram_refuses_egress_outside_the_configured_operator_chat() {
    let api = Arc::new(TelegramApiState::default());
    let (api_root, server) = telegram_fixture(Arc::clone(&api)).await;
    let fresh = FreshStore::new();
    let queue = sqlite_queue(&fresh).await;
    let queue_port: Arc<dyn Queue> = queue.clone();
    let spine_port: Arc<dyn Spine> = Arc::new(FakeSpine::new());
    let worker = TelegramWorker::new(
        queue_port,
        spine_port,
        Arc::new(FakeRegistry::new()),
        TelegramConfig {
            token: "TOKEN".to_string(),
            allowed_user_ids: Vec::new(),
            chat_id: "operator-42".to_string(),
            api_root,
        },
        temp_path(&fresh, "egress.lock"),
    )
    .expect("worker");
    queue
        .enqueue(
            telegram::send_job(&TelegramSend {
                from: SeatId::from("pij-test"),
                body: "must not leave".to_string(),
                msg_id: "wrong-chat".to_string(),
                chat_id: Some("stranger-99".to_string()),
            })
            .expect("job"),
        )
        .await
        .expect("enqueue");
    assert_eq!(worker.run_once(4).await.expect("terminal refusal").count, 1);
    assert_eq!(
        api.sends.load(Ordering::SeqCst),
        0,
        "wrong-chat request must never reach the bot API"
    );
    assert_eq!(
        queue.live_len().await.expect("live rows"),
        0,
        "refusal is terminal, not an immortal retry"
    );
    server.abort();
}
