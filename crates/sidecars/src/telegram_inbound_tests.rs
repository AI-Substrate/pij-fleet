//! Plan 179: inbound media is not dropped silently.

use std::sync::Arc;

use axum::{Json, Router, routing::get};
use pij_core::delivery::delivery_kind;
use pij_core::model::{Event, Outcome, SeatId, Seq};
use pij_core::ports::{Queue, Spine};
use pij_testkit::{
    fakes::{FakeQueue, FakeRegistry, FakeSpine},
    fresh_dir,
};
use serde_json::{Value, json};

use super::{TelegramConfig, TelegramWorker};

fn media_updates() -> Value {
    json!({"ok": true, "result": [
        {"update_id": 501, "message": {"from": {"id": 7}, "chat": {"id": 42},
            "caption": "look at this",
            "photo": [
                {"file_id": "s", "file_unique_id": "s1", "width": 90, "height": 90, "file_size": 1200},
                {"file_id": "l", "file_unique_id": "l1", "width": 1280, "height": 960, "file_size": 1_258_291}
            ]}},
        {"update_id": 502, "message": {"from": {"id": 7}, "chat": {"id": 42},
            "document": {"file_id": "d", "file_unique_id": "d1", "file_name": "report.pdf",
                         "mime_type": "application/pdf", "file_size": 2048}}},
        // Telegram sends a GIF as `animation` AND a back-compat `document`.
        {"update_id": 503, "message": {"from": {"id": 7}, "chat": {"id": 42}, "caption": "loop",
            "animation": {"file_id": "a", "file_unique_id": "a1", "file_name": "loop.mp4",
                          "mime_type": "video/mp4", "file_size": 3_145_728,
                          "width": 1, "height": 1, "duration": 1},
            "document": {"file_id": "a", "file_unique_id": "a1", "file_name": "loop.mp4",
                         "mime_type": "video/mp4", "file_size": 3_145_728}}},
        {"update_id": 504, "message": {"from": {"id": 7}, "chat": {"id": 42},
            "sticker": {"file_id": "k", "file_unique_id": "k1", "width": 512, "height": 512,
                        "is_animated": false, "is_video": false, "type": "regular"}}},
        {"update_id": 505, "message": {"from": {"id": 7}, "chat": {"id": 42},
            "voice": {"file_id": "v", "file_unique_id": "v1", "duration": 3,
                      "mime_type": "audio/ogg", "file_size": 512}}},
        // Not an allowed user: ignored, exactly as text from them is.
        {"update_id": 506, "message": {"from": {"id": 99}, "chat": {"id": 42},
            "photo": [{"file_id": "x", "file_unique_id": "x1", "width": 1, "height": 1}]}}
    ]})
}

struct Fixture {
    worker: TelegramWorker,
    queue: Arc<FakeQueue>,
    spine: Arc<FakeSpine>,
    server: tokio::task::JoinHandle<()>,
    dir: std::path::PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub");
        let address = listener.local_addr().expect("stub address");
        let app = Router::new().route(
            "/botTOKEN/getUpdates",
            get(|| async { Json(media_updates()) }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve stub");
        });
        let dir = fresh_dir("telegram-inbound-media");
        let queue = Arc::new(FakeQueue::new(1024).expect("queue"));
        let spine = Arc::new(FakeSpine::new());
        let worker = TelegramWorker::new(
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
        Self {
            worker,
            queue,
            spine,
            server,
            dir,
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

    /// Every turn queued for `target`, in order.
    async fn turns(&self, target: &str) -> Vec<Value> {
        let mut turns = Vec::new();
        while let Some((id, job)) = self
            .queue
            .claim(&[delivery_kind(&SeatId::from(target))], "assert-turns")
            .await
            .expect("claim")
        {
            turns.push(serde_json::from_str(&job.payload).expect("turn payload"));
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn inbound_media_reaches_the_bound_seat_as_a_notice_with_an_event() {
    let f = Fixture::new().await;
    f.bind("pij-target").await;
    f.worker.run_once(1).await.expect("poll");

    let turns = f.turns("pij-target").await;
    let bodies: Vec<_> = turns
        .iter()
        .map(|turn| turn["body"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        bodies,
        [
            "[telegram media not saved by pij-rs: photo, 1.2 MB] caption: look at this",
            "[telegram media not saved by pij-rs: document, 2.0 KB, report.pdf]",
            "[telegram media not saved by pij-rs: animation, 3.0 MB, loop.mp4] caption: loop",
            "[telegram media not saved by pij-rs: sticker]",
            "[telegram media not saved by pij-rs: voice, 512 B]",
        ],
        "every allowed media message is delivered, none from a stranger"
    );
    assert!(turns.iter().all(|turn| turn["from"] == "pij-telegram"));

    let unsupported = f.events("telegram.inbound-media-unsupported").await;
    let kinds: Vec<_> = unsupported
        .iter()
        .map(|event| event["kind"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        ["photo", "document", "animation", "sticker", "voice"]
    );
    assert_eq!(unsupported[0]["update_id"], 501);
    assert_eq!(unsupported[0]["conversation"], "42");
    assert_eq!(unsupported[0]["msg_id"], "telegram-501");
    assert_eq!(unsupported[0]["size"], 1_258_291);
    assert_eq!(unsupported[1]["file_name"], "report.pdf");
    assert_eq!(unsupported[1]["mime_type"], "application/pdf");
    assert_eq!(unsupported[3]["size"], Value::Null);
    assert_eq!(
        f.events("telegram.inbound-enqueued").await.len(),
        5,
        "each notice is an ordinary inbound turn too"
    );
}

#[tokio::test]
async fn unbound_inbound_media_is_refused_like_text() {
    let f = Fixture::new().await;
    f.worker.run_once(1).await.expect("poll");
    assert!(f.turns("pij-target").await.is_empty());
    let refused = f.events("telegram.inbound-refused").await;
    assert_eq!(refused.len(), 5, "{refused:?}");
    assert!(
        refused
            .iter()
            .all(|event| event["reason"] == "no persisted outbound binding")
    );
    assert!(
        f.events("telegram.inbound-media-unsupported")
            .await
            .is_empty()
    );
}
