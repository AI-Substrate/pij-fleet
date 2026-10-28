//! Plan 183: on the real queue, a retrying body holds its files behind it, and
//! the attempt cap ends the hold. Temp store and fake Telegram only: no tmux
//! adapter, no credential file. Here rather than in pij-sidecars because it
//! fast-forwards `not_before` with SQL, and sidecars may not depend on sqlx.

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use pij_core::config::Config;
use pij_core::model::SeatId;
use pij_core::ports::Queue;
use pij_sidecars::telegram::{
    MAX_ATTEMPTS, TelegramConfig, TelegramDocumentRequest, TelegramWorker, stage_send,
};
use pij_store::SqliteQueue;
use pij_testkit::{
    FreshStore,
    fakes::{FakeRegistry, FakeSpine},
    fresh_dir,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Api {
    calls: Mutex<Vec<String>>,
}

async fn method(
    State(api): State<Arc<Api>>,
    Path(method): Path<String>,
) -> (StatusCode, Json<Value>) {
    api.calls.lock().expect("calls").push(method.clone());
    if method == "sendMessage" {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"ok": false, "error_code": 502, "description": "down"})),
        )
    } else {
        (
            StatusCode::OK,
            Json(json!({"ok": true, "result": {"message_id": 1}})),
        )
    }
}

#[tokio::test]
async fn files_wait_behind_a_retrying_body_until_it_gives_up() {
    let api = Arc::new(Api::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let address = listener.local_addr().expect("address");
    let app = Router::new()
        .route(
            "/botTOKEN/getUpdates",
            get(|| async { Json(json!({"ok": true, "result": []})) }),
        )
        .route("/botTOKEN/{method}", post(method))
        .with_state(api.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let config = Config::default();
    let queue = Arc::new(
        SqliteQueue::new(
            pool.clone(),
            config.claim_lease_secs,
            config.delivered_id_capacity,
        )
        .expect("queue"),
    );
    let dir = fresh_dir("telegram-order");
    let worker = TelegramWorker::new(
        queue.clone(),
        Arc::new(FakeSpine::new()),
        Arc::new(FakeRegistry::new()),
        TelegramConfig {
            token: "TOKEN".into(),
            allowed_user_ids: vec![],
            chat_id: "42".into(),
            api_root: format!("http://{address}"),
        },
        dir.join("telegram.lock"),
    )
    .expect("worker");

    let files = ["a.html", "b.html"].map(|name| {
        let path = dir.join(name);
        std::fs::write(&path, name).expect("source");
        path
    });
    let staged = stage_send(
        &TelegramDocumentRequest {
            from: SeatId::from("pij-sender"),
            files: files.to_vec(),
            caption: None,
            body: Some("two files".into()),
            msg_id: "m".into(),
            chat_id: None,
        },
        &dir.join("staging"),
    )
    .expect("stage");
    let jobs: Vec<_> = staged
        .body
        .into_iter()
        .chain(staged.files.into_iter().map(|(job, _)| job))
        .collect();
    queue.enqueue_all(jobs).await.expect("enqueue");

    for attempt in 1..=MAX_ATTEMPTS {
        let _ = worker.run_once(32).await;
        let calls = api.calls.lock().expect("calls").clone();
        if attempt < MAX_ATTEMPTS {
            assert_eq!(
                calls,
                vec!["sendMessage"; attempt as usize],
                "attempt {attempt}: no file may overtake the retrying body"
            );
            // The clock passing the backoff deadline.
            sqlx::query("UPDATE jobs SET not_before = 0 WHERE state = 'pending'")
                .execute(&pool)
                .await
                .expect("make due");
        }
    }
    let calls = api.calls.lock().expect("calls").clone();
    assert_eq!(
        calls[MAX_ATTEMPTS as usize..],
        ["sendDocument", "sendDocument"],
        "once the body fails terminally, the files go, in order: {calls:?}"
    );
    let outcomes: Vec<(String, String)> =
        sqlx::query_as("SELECT dedupe_key, state FROM jobs ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("outcomes");
    assert_eq!(
        outcomes,
        [
            ("m".to_string(), "failed".to_string()),
            ("m#1".to_string(), "done".to_string()),
            ("m#2".to_string(), "done".to_string()),
        ]
    );
    server.abort();
    let _ = std::fs::remove_dir_all(dir);
}
