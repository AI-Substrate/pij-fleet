use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use pij_core::model::{Harness, Outcome, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Queue, Registry, Spine};
use pij_testkit::{
    fakes::{FakeQueue, FakeRegistry, FakeSpine},
    fresh_dir,
};
use serde_json::{Value, json};

use super::{TelegramConfig, TelegramSend, TelegramWorker, send_job};

#[derive(Default)]
struct Api {
    sent: Mutex<Vec<Value>>,
    fail_part: usize,
    status: Option<StatusCode>,
}

async fn send(State(api): State<Arc<Api>>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut sent = api.sent.lock().expect("sent");
    sent.push(body);
    if sent.len() == api.fail_part {
        (
            api.status.unwrap_or(StatusCode::BAD_REQUEST),
            Json(json!({"ok": false, "error_code": 400})),
        )
    } else {
        (
            StatusCode::OK,
            Json(json!({"ok": true, "result": {"message_id": sent.len()}})),
        )
    }
}

struct Fixture {
    worker: TelegramWorker,
    queue: Arc<FakeQueue>,
    spine: Arc<FakeSpine>,
    registry: Arc<FakeRegistry>,
    api: Arc<Api>,
    server: tokio::task::JoinHandle<()>,
    dir: std::path::PathBuf,
}

impl Fixture {
    async fn new(fail_part: usize, status: Option<StatusCode>) -> Self {
        let api = Arc::new(Api {
            fail_part,
            status,
            ..Api::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub");
        let address = listener.local_addr().expect("stub address");
        let app = Router::new()
            .route("/botTOKEN/sendMessage", post(send))
            .route(
                "/botTOKEN/getUpdates",
                get(|| async { Json(json!({"ok": true, "result": []})) }),
            )
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve stub");
        });
        let dir = fresh_dir("telegram-outbound");
        let queue = Arc::new(FakeQueue::new(1024).expect("queue"));
        let spine = Arc::new(FakeSpine::new());
        let registry = Arc::new(FakeRegistry::new());
        let mut worker = TelegramWorker::new(
            queue.clone(),
            spine.clone(),
            registry.clone(),
            TelegramConfig {
                token: "TOKEN".into(),
                allowed_user_ids: vec![],
                chat_id: "42".into(),
                api_root: format!("http://{address}"),
            },
            dir.join("telegram.lock"),
        )
        .expect("worker");
        worker.client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .connect_timeout(Duration::from_secs(1))
            .build()
            .expect("bounded client");
        Self {
            worker,
            queue,
            spine,
            registry,
            api,
            server,
            dir,
        }
    }

    async fn enqueue(&self, body: &str) {
        self.queue
            .enqueue(
                send_job(&TelegramSend {
                    from: SeatId::from("pij-sender"),
                    body: body.into(),
                    msg_id: "outbound-146".into(),
                    chat_id: None,
                })
                .expect("send job"),
            )
            .await
            .expect("enqueue");
    }

    fn texts(&self) -> Vec<String> {
        self.api
            .sent
            .lock()
            .expect("sent")
            .iter()
            .map(|body| {
                assert_eq!(body["chat_id"], "42");
                body["text"].as_str().expect("text").to_owned()
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        std::fs::remove_dir_all(&self.dir).expect("cleanup fixture");
    }
}

#[tokio::test]
async fn tag_is_first_for_an_unknown_sender() {
    let f = Fixture::new(0, None).await;
    f.enqueue("hello").await;
    f.worker.run_once(1).await.expect("send");
    assert_eq!(f.texts(), ["[pij-sender] hello"]);
    assert!(matches!(f.queue.acked().as_slice(), [(_, Outcome::Done)]));
}

#[tokio::test]
async fn context_and_exact_leading_tags_are_normalized() {
    for (folder, body, expected) in [
        ("/work/my-repo/", "hello", "[pij-sender] [my-repo] hello"),
        ("", "hello", "[pij-sender] hello"),
        ("/", "hello", "[pij-sender] hello"),
        (
            "/work/my-repo",
            "[pij-sender] hello",
            "[pij-sender] [my-repo] hello",
        ),
        (
            "/work/my-repo",
            "[pij-sender] [my-repo] hello",
            "[pij-sender] [my-repo] hello",
        ),
        ("/work/my-repo", "[pij-sender]", "[pij-sender] [my-repo] "),
        (
            "/work/my-repo",
            "[pij-sender]suffix",
            "[pij-sender] [my-repo] [pij-sender]suffix",
        ),
        (
            "",
            "[someone-else] hello",
            "[pij-sender] [someone-else] hello",
        ),
    ] {
        let f = Fixture::new(0, None).await;
        f.registry
            .put(SeatDescriptor::new("pij-sender", Harness::Omp, folder))
            .await
            .expect("register sender");
        f.enqueue(body).await;
        f.worker.run_once(1).await.expect("send");
        assert_eq!(f.texts(), [expected], "folder={folder:?}, body={body:?}");
    }
}

#[tokio::test]
async fn long_body_is_numbered_ordered_and_lossless_with_prefix_in_budget() {
    let f = Fixture::new(0, None).await;
    // 9,000 characters: non-ASCII and repeated word/line boundaries expose byte slicing and trimming.
    let body = "é🙂 word\n ".repeat(1000);
    assert_eq!(body.chars().count(), 9000);
    f.enqueue(&body).await;
    f.worker.run_once(1).await.expect("send all parts");
    let parts = f.texts();
    assert!(parts.len() >= 3, "9000 characters must be split");
    let mut restored = String::new();
    for (i, part) in parts.iter().enumerate() {
        assert!(
            part.encode_utf16().count() <= 4000,
            "budget includes sender and numbering"
        );
        let prefix = format!("[pij-sender] ({}/{}) ", i + 1, parts.len());
        restored.push_str(
            part.strip_prefix(&prefix)
                .expect("ordered numbered sender prefix"),
        );
    }
    assert_eq!(restored, body);
}

#[tokio::test]
async fn rejected_send_is_retried_with_decodable_spine_evidence() {
    let f = Fixture::new(1, None).await;
    f.enqueue("rejected").await;
    let _ = f.worker.run_once(1).await;
    assert_failure(&f, 1, 1, Some(400)).await;
}

#[tokio::test]
async fn ok_false_is_not_a_successful_send() {
    let f = Fixture::new(1, Some(StatusCode::OK)).await;
    f.enqueue("rejected by API").await;
    let _ = f.worker.run_once(1).await;
    assert_failure(&f, 1, 1, Some(200)).await;
}

#[tokio::test]
async fn failed_later_part_stops_and_preserves_whole_job_retry() {
    let f = Fixture::new(2, None).await;
    f.enqueue(&"x".repeat(9000)).await;
    let _ = f.worker.run_once(1).await;
    assert_failure(&f, 2, 3, Some(400)).await;
    f.queue.advance(Duration::from_secs(1));
    f.worker
        .run_once(1)
        .await
        .expect("whole-job retry succeeds");
    assert_eq!(
        f.texts().len(),
        5,
        "two attempted parts plus three on retry"
    );
    assert!(matches!(f.queue.acked().as_slice(), [(_, Outcome::Done)]));
}

async fn assert_failure(f: &Fixture, part: usize, parts: usize, status: Option<u16>) {
    let events = f.spine.tail(None, Seq(0)).await.expect("events");
    let event = events
        .iter()
        .find(|e| e.kind == "telegram.outbound-delivery-failed")
        .expect("durable failure event");
    assert_eq!(event.seat, Some(SeatId::from("pij-telegram-chat-42")));
    let detail: Value = serde_json::from_str(&event.payload).expect("decodable failure");
    assert_eq!(detail["conversation"], "42");
    assert_eq!(detail["msg_id"], "outbound-146");
    assert_eq!(detail["from"], "pij-sender");
    assert_eq!(detail["part"], part);
    assert_eq!(detail["parts"], parts);
    assert_eq!(detail["http_status"], json!(status));
    assert_eq!(detail["attempt"], 1);
    assert!(
        f.queue.acked().is_empty(),
        "failure must not acknowledge success"
    );
    assert_eq!(f.queue.retried().len(), 1, "retain existing retry policy");
}

#[tokio::test]
async fn transport_failure_records_each_attempt_without_leaking_the_token() {
    let mut f = Fixture::new(0, None).await;
    f.worker.config.api_root = "http://127.0.0.1:1".into();
    f.worker.config.token = "SECRET-NOT-A-REAL-TOKEN".into();
    f.enqueue("unreachable").await;
    for attempt in 1..=2 {
        let error = f
            .worker
            .run_once(1)
            .await
            .expect_err("transport failure retries");
        let super::PijError::Adapter { message, .. } = error else {
            panic!("adapter failure")
        };
        let detail: Value = serde_json::from_str(&message).expect("decodable error");
        assert_eq!(detail["http_status"], Value::Null);
        assert_eq!(detail["attempt"], attempt);
        assert_eq!(detail["part"], 1);
        assert!(!message.contains("SECRET-NOT-A-REAL-TOKEN"));
        assert!(!message.contains("127.0.0.1"));
        let events = f.spine.tail(None, Seq(0)).await.expect("events");
        assert_eq!(events.len(), attempt as usize);
        assert_eq!(events.last().expect("attempt event").payload, message);
        f.queue.advance(Duration::from_secs(1));
    }
    assert!(f.queue.acked().is_empty());
    assert_eq!(f.queue.retried().len(), 2);
}

#[test]
fn chunk_boundaries_prefer_newlines_and_numbering_accounts_for_digit_growth() {
    let sender = SeatId::from("pij-sender");
    let body = format!(
        "{}\n{} {}",
        "x".repeat(2500),
        "y".repeat(700),
        "z".repeat(1000)
    );
    let parts = super::prefixed_text_parts(&sender, None, &body).expect("parts");
    assert_eq!(
        parts[0],
        format!("[pij-sender] (1/2) {}\n", "x".repeat(2500))
    );
    assert_eq!(
        parts[1],
        format!(
            "[pij-sender] (2/2) {} {}",
            "y".repeat(700),
            "z".repeat(1000)
        )
    );
    let parts = super::prefixed_text_parts(&sender, None, &"🙂".repeat(20000)).expect("parts");
    assert_eq!(parts.len(), 11);
    for (i, part) in parts.iter().enumerate() {
        assert!(part.encode_utf16().count() <= 4000);
        assert!(part.starts_with(&format!("[pij-sender] ({}/11) ", i + 1)));
    }
}

#[tokio::test]
async fn oversized_context_falls_back_to_the_tag() {
    let f = Fixture::new(0, None).await;
    f.registry
        .put(SeatDescriptor::new(
            "pij-sender",
            Harness::Omp,
            format!("/work/{}", "r".repeat(1100)),
        ))
        .await
        .expect("sender");
    f.enqueue("hello").await;
    f.worker
        .run_once(1)
        .await
        .expect("send with bounded prefix");
    assert_eq!(f.texts(), ["[pij-sender] hello"]);
}
