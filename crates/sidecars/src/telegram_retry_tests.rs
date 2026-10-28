//! Plan 183: a failing Telegram send backs off, honours `retry_after`, gives
//! up, never silences inbound, and a stalled upload ends. Fake Telegram only.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use pij_core::delivery::delivery_kind;
use pij_core::model::{Event, Outcome, SeatId, Seq};
use pij_core::ports::{Queue, Spine};
use pij_testkit::{
    fakes::{FakeQueue, FakeRegistry, FakeSpine},
    fresh_dir,
};
use serde_json::{Value, json};

use super::{TelegramConfig, TelegramSend, TelegramWorker, send_job};

/// What the fake Bot API answers to `sendMessage` and uploads.
#[derive(Clone, Copy)]
enum Answer {
    Ok,
    Status(StatusCode),
    TooMany {
        retry_after: u64,
    },
    /// Accept the request and never answer.
    Stall,
}

struct Api {
    answer: Mutex<Answer>,
    sends: AtomicUsize,
    polls: AtomicUsize,
    /// Updates `getUpdates` hands out once.
    updates: Mutex<Vec<Value>>,
}

async fn reply(api: &Api) -> (StatusCode, Json<Value>) {
    let answer = *api.answer.lock().expect("answer");
    match answer {
        Answer::Ok => (
            StatusCode::OK,
            Json(json!({"ok": true, "result": {"message_id": 1}})),
        ),
        Answer::Status(status) => (
            status,
            Json(json!({"ok": false, "error_code": status.as_u16(), "description": "nope"})),
        ),
        Answer::TooMany { retry_after } => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "ok": false, "error_code": 429,
                "description": format!("Too Many Requests: retry after {retry_after}"),
                "parameters": {"retry_after": retry_after}
            })),
        ),
        Answer::Stall => std::future::pending().await,
    }
}

async fn send_message(State(api): State<Arc<Api>>) -> (StatusCode, Json<Value>) {
    api.sends.fetch_add(1, Ordering::SeqCst);
    reply(&api).await
}

async fn upload_file(State(api): State<Arc<Api>>) -> (StatusCode, Json<Value>) {
    api.sends.fetch_add(1, Ordering::SeqCst);
    reply(&api).await
}

async fn get_updates(State(api): State<Arc<Api>>) -> Json<Value> {
    api.polls.fetch_add(1, Ordering::SeqCst);
    let updates: Vec<Value> = std::mem::take(&mut *api.updates.lock().expect("updates"));
    Json(json!({"ok": true, "result": updates}))
}

struct Fixture {
    worker: Arc<TelegramWorker>,
    queue: Arc<FakeQueue>,
    spine: Arc<FakeSpine>,
    api: Arc<Api>,
    server: tokio::task::JoinHandle<()>,
    dir: std::path::PathBuf,
}

impl Fixture {
    async fn new(answer: Answer) -> Self {
        Self::with_upload_timeout(answer, None).await
    }

    async fn with_upload_timeout(answer: Answer, upload: Option<Duration>) -> Self {
        let api = Arc::new(Api {
            answer: Mutex::new(answer),
            sends: AtomicUsize::new(0),
            polls: AtomicUsize::new(0),
            updates: Mutex::new(Vec::new()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub");
        let address = listener.local_addr().expect("stub address");
        let app = Router::new()
            .route("/botTOKEN/sendMessage", post(send_message))
            .route("/botTOKEN/sendDocument", post(upload_file))
            .route("/botTOKEN/getUpdates", get(get_updates))
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve stub");
        });
        let dir = fresh_dir("telegram-retry");
        let queue = Arc::new(FakeQueue::new(1024).expect("queue"));
        let spine = Arc::new(FakeSpine::new());
        let mut worker = TelegramWorker::new(
            queue.clone(),
            spine.clone(),
            Arc::new(FakeRegistry::new()),
            TelegramConfig {
                token: "TOKEN".into(),
                allowed_user_ids: vec![7],
                chat_id: "42".into(),
                api_root: format!("http://{address}"),
            },
            dir.join("telegram.lock"),
        )
        .expect("worker");
        if let Some(upload) = upload {
            worker.timeouts.upload = upload;
        }
        Self {
            worker: Arc::new(worker),
            queue,
            spine,
            api,
            server,
            dir,
        }
    }

    async fn enqueue_text(&self, msg_id: &str) {
        self.queue
            .enqueue(
                send_job(&TelegramSend {
                    from: SeatId::from("pij-sender"),
                    body: "hello".into(),
                    msg_id: msg_id.into(),
                    chat_id: None,
                })
                .expect("send job"),
            )
            .await
            .expect("enqueue");
    }

    async fn enqueue_file(&self) {
        let source = self.dir.join("report.html");
        std::fs::write(&source, b"<p>x</p>").expect("source");
        let staged = super::stage_send(
            &super::TelegramDocumentRequest {
                from: SeatId::from("pij-sender"),
                files: vec![source],
                caption: None,
                body: None,
                msg_id: "doc-1".into(),
                chat_id: None,
            },
            &self.dir.join("staging"),
        )
        .expect("stage");
        for (job, _) in staged.files {
            self.queue.enqueue(job).await.expect("enqueue");
        }
    }

    async fn bind(&self, target: &str) {
        self.spine
            .append(Event {
                seq: None,
                v: 1,
                at: 1,
                kind: "telegram.binding".into(),
                seat: Some(SeatId::from("pij-telegram-chat-42")),
                payload: json!({"target": target, "outbound_msg_id": "o-1"}).to_string(),
            })
            .await
            .expect("binding");
    }

    fn inbound(&self, text: &str) {
        self.api.updates.lock().expect("updates").push(json!({
            "update_id": 900,
            "message": {"from": {"id": 7}, "chat": {"id": 42}, "text": text}
        }));
    }

    async fn turns(&self, target: &str) -> Vec<String> {
        let mut turns = Vec::new();
        while let Some((id, job)) = self
            .queue
            .claim(&[delivery_kind(&SeatId::from(target))], "assert-turns")
            .await
            .expect("claim")
        {
            let turn: Value = serde_json::from_str(&job.payload).expect("turn");
            turns.push(turn["body"].as_str().unwrap_or_default().to_string());
            self.queue.ack(id, Outcome::Done).await.expect("ack");
        }
        turns
    }

    async fn events(&self, kind: &str) -> Vec<Value> {
        self.spine
            .tail(None, Seq(0))
            .await
            .expect("events")
            .into_iter()
            .filter(|event| event.kind == kind)
            .map(|event| serde_json::from_str(&event.payload).expect("decodable event"))
            .collect()
    }

    fn delays(&self) -> Vec<u64> {
        self.queue
            .retried()
            .iter()
            .map(|(_, delay)| delay.as_secs())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn backoff_doubles_to_a_cap_honours_retry_after_and_ends() {
    let secs = |attempt, after: Option<u64>| {
        super::retry_delay(attempt, after.map(Duration::from_secs)).map(|d| d.as_secs())
    };
    let plain: Vec<_> = (0..10).map(|attempt| secs(attempt, None)).collect();
    assert_eq!(
        plain,
        [
            Some(2),
            Some(4),
            Some(8),
            Some(16),
            Some(32),
            Some(64),
            Some(128),
            Some(256),
            Some(300),
            None
        ],
        "doubling from 2 s, capped at 300 s, ten attempts in all"
    );
    assert_eq!(secs(0, Some(30)), Some(30), "a longer retry_after wins");
    assert_eq!(
        secs(5, Some(3)),
        Some(64),
        "a shorter one never shortens it"
    );
    assert_eq!(secs(0, Some(3600)), Some(3600), "up to the ceiling");
    assert_eq!(secs(0, Some(3601)), None, "past it the job ends");
}

/// sendMessage 429 with `retry_after: 30`: the retry waits 30 s, not 100 ms.
#[tokio::test]
async fn a_429_on_a_text_send_waits_at_least_retry_after() {
    let f = Fixture::new(Answer::TooMany { retry_after: 30 }).await;
    f.enqueue_text("t-1").await;
    let _ = f.worker.run_once(1).await;
    assert_eq!(f.delays(), [30], "retry_after honoured");
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed[0]["http_status"], 429);
    assert_eq!(failed[0]["retry_after_s"], 30);
}

/// The same for an upload.
#[tokio::test]
async fn a_429_on_an_upload_waits_at_least_retry_after() {
    let f = Fixture::new(Answer::TooMany { retry_after: 45 }).await;
    f.enqueue_file().await;
    let _ = f.worker.run_once(1).await;
    assert_eq!(f.delays(), [45]);
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed[0]["retry_after_s"], 45);
    assert_eq!(failed[0]["terminal"], false);
}

/// A job that keeps failing backs off per attempt, then fails terminally with
/// its reason and a spine event.
#[tokio::test]
async fn a_job_that_keeps_failing_backs_off_then_gives_up() {
    let f = Fixture::new(Answer::Status(StatusCode::BAD_GATEWAY)).await;
    f.enqueue_text("t-1").await;
    for _ in 0..20 {
        let _ = f.worker.run_once(1).await;
        if let Some((_, delay)) = f.queue.retried().last() {
            f.queue.advance(*delay);
        }
    }
    assert_eq!(f.delays(), [2, 4, 8, 16, 32, 64, 128, 256, 300]);
    assert_eq!(f.api.sends.load(Ordering::SeqCst), 10, "ten attempts");
    match f.queue.acked().as_slice() {
        [(_, Outcome::Failed { reason })] => {
            assert!(reason.contains("gave up after 10 attempts"), "{reason}");
            assert!(reason.contains("502"), "{reason}");
        }
        other => panic!("the job must fail terminally, got {other:?}"),
    }
    let gave_up = f.events("telegram.outbound-gave-up").await;
    assert_eq!(gave_up.len(), 1);
    assert_eq!(gave_up[0]["msg_id"], "t-1");
    assert_eq!(gave_up[0]["attempts"], 10);
    assert_eq!(gave_up[0]["conversation"], "42");
}

/// A `retry_after` past the ceiling ends the job now, never sooner than asked.
#[tokio::test]
async fn a_retry_after_past_the_ceiling_fails_the_job_now() {
    let f = Fixture::new(Answer::TooMany {
        retry_after: 86_400,
    })
    .await;
    f.enqueue_text("t-1").await;
    let _ = f.worker.run_once(1).await;
    assert!(f.queue.retried().is_empty());
    match f.queue.acked().as_slice() {
        [(_, Outcome::Failed { reason })] => assert!(reason.contains("86400 s"), "{reason}"),
        other => panic!("expected a terminal failure, got {other:?}"),
    }
}

/// Cost (AC6): over one simulated hour of 250 ms ticks, a job that never
/// succeeds costs at most `MAX_ATTEMPTS` HTTP calls; before plan 183 it was
/// one per tick (14,400).
#[tokio::test]
async fn a_failing_job_costs_a_bounded_number_of_calls_per_hour() {
    let f = Fixture::new(Answer::Status(StatusCode::SERVICE_UNAVAILABLE)).await;
    f.enqueue_text("t-1").await;
    let tick = Duration::from_millis(250);
    let mut per_minute = [0usize; 60];
    for n in 0..(3600 * 4) {
        let before = f.api.sends.load(Ordering::SeqCst);
        let _ = f.worker.run_outbound(1).await;
        per_minute[n / 240] += f.api.sends.load(Ordering::SeqCst) - before;
        f.queue.advance(tick);
    }
    let total: usize = per_minute.iter().sum();
    println!("calls per simulated minute: {per_minute:?} (total {total})");
    assert_eq!(total, super::MAX_ATTEMPTS as usize);
    assert!(
        per_minute.iter().all(|calls| *calls <= 5),
        "no burst: {per_minute:?}"
    );
}

/// R1: inbound is polled on the same pass an outbound job retries.
#[tokio::test]
async fn inbound_is_delivered_while_an_outbound_job_retries() {
    let f = Fixture::new(Answer::Status(StatusCode::BAD_GATEWAY)).await;
    f.bind("pij-target").await;
    f.enqueue_text("t-1").await;
    f.inbound("from the phone");
    let _ = f.worker.run_once(1).await;
    assert_eq!(f.queue.retried().len(), 1, "the outbound job is retrying");
    assert_eq!(f.turns("pij-target").await, ["from the phone"]);
}

/// The shipped loops: an upload that never answers does not stop inbound,
/// and ends as a retry when its timeout passes.
#[tokio::test]
async fn a_stalled_upload_ends_as_a_retry_and_inbound_keeps_polling() {
    let f = Fixture::with_upload_timeout(Answer::Stall, Some(Duration::from_millis(1_500))).await;
    f.bind("pij-target").await;
    f.enqueue_file().await;
    let loops = Arc::clone(&f.worker)
        .start(Duration::from_millis(20), 32)
        .expect("start");
    let (api, queue) = (&f.api, &f.queue);
    // Wait for the upload to be in flight, then send from the phone.
    tokio::time::timeout(Duration::from_secs(5), async {
        while api.sends.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("upload started");
    let polls_at_stall = api.polls.load(Ordering::SeqCst);
    api.updates.lock().expect("updates").push(json!({
        "update_id": 901,
        "message": {"from": {"id": 7}, "chat": {"id": 42}, "text": "are you there"}
    }));
    let delivered = tokio::time::timeout(Duration::from_millis(1_000), async {
        loop {
            if let Some((id, job)) = queue
                .claim(&[delivery_kind(&SeatId::from("pij-target"))], "assert")
                .await
                .expect("claim")
            {
                queue.ack(id, Outcome::Done).await.expect("ack");
                return job.payload;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("inbound delivered while the upload is still stalled");
    assert!(delivered.contains("are you there"));
    assert!(
        queue.retried().is_empty(),
        "the upload is still in flight: inbound did not wait for it"
    );
    assert!(api.polls.load(Ordering::SeqCst) > polls_at_stall);
    tokio::time::timeout(Duration::from_secs(5), async {
        while queue.retried().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the stalled upload ends as a retry, not a hung worker");
    assert_eq!(
        queue.acked().len(),
        1,
        "only the inbound turn this test consumed; the upload is never acknowledged as sent"
    );
    loops.shutdown().await;
}

/// The shipped client is bounded: numbers sized for a 50 MB upload.
#[tokio::test]
async fn production_timeouts_are_sized_for_a_fifty_megabyte_upload() {
    let f = Fixture::new(Answer::Ok).await;
    assert_eq!(f.worker.timeouts, super::Timeouts::default());
    assert_eq!(super::CONNECT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(super::API_TIMEOUT, Duration::from_secs(30));
    assert_eq!(super::UPLOAD_TIMEOUT, Duration::from_secs(600));
    // 50 MB at 1 Mbit/s fits inside the upload timeout.
    let at_one_megabit = super::DOCUMENT_LIMIT_BYTES * 8 / 1_000_000;
    assert!(Duration::from_secs(at_one_megabit) < super::UPLOAD_TIMEOUT);
}

/// N2 wiring: the shipped outbound loop runs the sweep on its first pass.
#[tokio::test]
async fn the_shipped_loop_sweeps_expired_staged_copies() {
    let dir = fresh_dir("telegram-retry-sweep");
    let staging = dir.join("telegram-attachments");
    std::fs::create_dir_all(&staging).expect("staging");
    let expired = staging.join("cccc-3333");
    std::fs::write(&expired, b"old evidence").expect("copy");
    std::fs::File::options()
        .write(true)
        .open(&expired)
        .expect("open")
        .set_modified(
            std::time::SystemTime::now() - super::FAILED_COPY_RETENTION - Duration::from_secs(60),
        )
        .expect("age it");
    let worker = TelegramWorker::new(
        Arc::new(FakeQueue::new(1024).expect("queue")),
        Arc::new(FakeSpine::new()),
        Arc::new(FakeRegistry::new()),
        TelegramConfig {
            token: "TOKEN".into(),
            allowed_user_ids: vec![],
            chat_id: "42".into(),
            api_root: "http://127.0.0.1:1".into(),
        },
        dir.join("telegram.lock"),
    )
    .expect("worker")
    .with_attachment_sweep(staging);
    let loops = Arc::new(worker)
        .start(Duration::from_millis(20), 1)
        .expect("start");
    tokio::time::timeout(Duration::from_secs(5), async {
        while expired.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("swept on the first pass");
    loops.shutdown().await;
    let _ = std::fs::remove_dir_all(dir);
}
