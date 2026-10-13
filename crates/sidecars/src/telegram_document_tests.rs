use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use pij_core::model::{Harness, Outcome, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Queue, Registry, Spine};
use pij_testkit::{
    fakes::{FakeQueue, FakeRegistry, FakeSpine},
    fresh_dir,
};
use serde_json::{Value, json};

use super::super::{TelegramConfig, TelegramWorker};
use super::{
    CAPTION_LIMIT, DocumentJob, TelegramDocumentRequest, boundary_for, document_multipart,
    prefixed_caption, stage_document, stage_with_limit,
};

const HTML: &[u8] = b"<html><body>pij attachment canary</body></html>\n";
// printf '<html><body>pij attachment canary</body></html>\n' | shasum -a 256
const HTML_SHA256: &str = "0fa42fe2449c4b1a4bf5790c9b19d0c1a3ace34ecc5177034e06afa19a418590";

fn request(path: PathBuf, caption: Option<&str>) -> TelegramDocumentRequest {
    TelegramDocumentRequest {
        from: SeatId::from("pij-sender"),
        path,
        caption: caption.map(str::to_string),
        msg_id: "doc-1".into(),
        chat_id: None,
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write source");
    path
}

fn message(error: &pij_core::error::PijError) -> String {
    error.to_string()
}

// ---------------------------------------------------------------- multipart

#[test]
fn multipart_body_is_byte_exact() {
    let (content_type, body) = document_multipart(
        "B",
        "42",
        "[pij-sender] report",
        "a\"b\r\n.html",
        b"<p>x</p>",
    );
    assert_eq!(content_type, "multipart/form-data; boundary=B");
    let expected = "--B\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n42\r\n\
                    --B\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\n[pij-sender] report\r\n\
                    --B\r\nContent-Disposition: form-data; name=\"document\"; filename=\"a%22b%0D%0A.html\"\r\n\
                    Content-Type: application/octet-stream\r\n\r\n<p>x</p>\r\n--B--\r\n";
    assert_eq!(String::from_utf8(body).expect("utf8"), expected);
}

#[test]
fn multipart_carries_binary_bytes_verbatim() {
    let bytes: Vec<u8> = (0..=255u8).collect();
    let (_, body) = document_multipart("B", "42", "c", "bin", &bytes);
    let start = body
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|_| ())
        .and_then(|()| {
            let marker = b"application/octet-stream\r\n\r\n";
            body.windows(marker.len())
                .position(|w| w == marker)
                .map(|at| at + marker.len())
        })
        .expect("document part");
    assert_eq!(&body[start..start + 256], bytes.as_slice());
}

#[test]
fn boundary_never_occurs_in_any_part() {
    let sha = "abc";
    let hostile = b"xx pij-abc-0 yy pij-abc-1 zz";
    let boundary = boundary_for(sha, &[hostile, b"caption"]);
    assert_eq!(boundary, "pij-abc-2");
}

// ---------------------------------------------------------------- caption

#[test]
fn caption_is_tagged_and_falls_back_to_the_bare_tag() {
    let from = SeatId::from("pij-sender");
    assert_eq!(
        prefixed_caption(&from, Some("repo"), "report").expect("fits"),
        "[pij-sender] [repo] report"
    );
    assert_eq!(
        prefixed_caption(&from, Some("repo"), "").expect("empty caption"),
        "[pij-sender] [repo]"
    );
    assert_eq!(
        prefixed_caption(&from, None, "[pij-sender] report").expect("idempotent tag"),
        "[pij-sender] report"
    );
    let tight = "c".repeat(CAPTION_LIMIT - "[pij-sender] ".len());
    assert_eq!(
        prefixed_caption(&from, Some("repo"), &tight).expect("tag fallback"),
        format!("[pij-sender] {tight}")
    );
    let over = format!("{tight}c");
    let error = prefixed_caption(&from, None, &over).expect_err("over the caption limit");
    assert!(message(&error).contains("1011"), "{}", message(&error));
}

// ---------------------------------------------------------------- staging

#[test]
fn staging_copies_by_content_so_later_edits_cannot_change_the_send() {
    let dir = fresh_dir("telegram-doc-stage");
    let source = write(&dir, "report.html", HTML);
    let staging = dir.join("staging");
    let (job, document) =
        stage_document(&request(source.clone(), Some("hi")), &staging).expect("staged");
    assert_eq!(document.sha256, HTML_SHA256);
    assert_eq!(document.size, HTML.len() as u64);
    assert_eq!(document.file_name, "report.html");
    assert!(document.staged.starts_with(&staging));
    assert!(
        document
            .staged
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(HTML_SHA256))
    );
    std::fs::write(&source, b"edited after enqueue").expect("edit source");
    assert_eq!(std::fs::read(&document.staged).expect("staged"), HTML);

    assert_eq!(job.kind, super::super::TELEGRAM_DOCUMENT_KIND);
    assert_eq!(job.dedupe_key, "doc-1");
    assert_eq!(job.serial_key, "default");
    let payload: DocumentJob = serde_json::from_str(&job.payload).expect("payload");
    assert_eq!(payload.document, document);
    assert_eq!(payload.caption, "hi");

    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| std::fs::metadata(path).expect("mode").permissions().mode() & 0o777;
    assert_eq!(mode(&staging), 0o700);
    assert_eq!(mode(&document.staged), 0o600);
    let leftovers: Vec<_> = std::fs::read_dir(&staging)
        .expect("list")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".partial"))
        .collect();
    assert!(leftovers.is_empty(), "no partial copies left behind");
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn missing_empty_directory_relative_and_oversized_files_refuse_at_enqueue() {
    let dir = fresh_dir("telegram-doc-refuse");
    let staging = dir.join("staging");
    let cases = [
        (dir.join("absent.html"), None, "cannot be read"),
        (write(&dir, "empty.html", b""), None, "is empty"),
        (dir.clone(), None, "not a regular file"),
        (PathBuf::from("relative.html"), None, "relative"),
        (
            write(&dir, "big.bin", &[7u8; 11]),
            Some(10),
            "is 11 bytes; Telegram bots may upload at most 10 bytes",
        ),
    ];
    for (path, limit, expected) in cases {
        let request = request(path.clone(), None);
        let error = match limit {
            Some(limit) => stage_with_limit(&request, &staging, limit),
            None => stage_document(&request, &staging),
        }
        .expect_err("must refuse");
        assert!(
            message(&error).contains(expected),
            "{path:?}: {}",
            message(&error)
        );
    }
    let staged = std::fs::read_dir(&staging)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(staged, 0, "a refusal stages nothing");
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn oversized_caption_refuses_at_enqueue() {
    let dir = fresh_dir("telegram-doc-caption");
    let source = write(&dir, "r.html", HTML);
    let caption = "c".repeat(CAPTION_LIMIT);
    let error = stage_document(&request(source, Some(&caption)), &dir.join("staging"))
        .expect_err("caption over limit");
    assert!(message(&error).contains("caption"), "{}", message(&error));
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn the_document_limit_is_telegrams_fifty_megabytes() {
    assert_eq!(super::DOCUMENT_LIMIT_BYTES, 52_428_800);
}

// ---------------------------------------------------------------- worker

#[derive(Default)]
struct Api {
    uploads: Mutex<Vec<(String, Vec<u8>)>>,
    texts: Mutex<Vec<Value>>,
    status: Mutex<Option<StatusCode>>,
}

async fn send_document(
    State(api): State<Arc<Api>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    api.uploads
        .lock()
        .expect("uploads")
        .push((content_type, body.to_vec()));
    match *api.status.lock().expect("status") {
        Some(status) => (
            status,
            Json(
                json!({"ok": false, "error_code": status.as_u16(), "description": "Bad Request: file is too big"}),
            ),
        ),
        None => (
            StatusCode::OK,
            Json(json!({"ok": true, "result": {"message_id": 1}})),
        ),
    }
}

async fn send_message(State(api): State<Arc<Api>>, Json(body): Json<Value>) -> Json<Value> {
    api.texts.lock().expect("texts").push(body);
    Json(json!({"ok": true, "result": {"message_id": 2}}))
}

struct Fixture {
    worker: TelegramWorker,
    queue: Arc<FakeQueue>,
    spine: Arc<FakeSpine>,
    registry: Arc<FakeRegistry>,
    api: Arc<Api>,
    server: tokio::task::JoinHandle<()>,
    dir: PathBuf,
}

impl Fixture {
    async fn new(status: Option<StatusCode>) -> Self {
        let api = Arc::new(Api {
            status: Mutex::new(status),
            ..Api::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub");
        let address = listener.local_addr().expect("stub address");
        let app = Router::new()
            .route("/botTOKEN/sendDocument", post(send_document))
            .route("/botTOKEN/sendMessage", post(send_message))
            .route(
                "/botTOKEN/getUpdates",
                get(|| async { Json(json!({"ok": true, "result": []})) }),
            )
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve stub");
        });
        let dir = fresh_dir("telegram-document");
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

    async fn enqueue(&self, caption: Option<&str>) -> super::StagedDocument {
        let source = write(&self.dir, "report.html", HTML);
        let (job, document) =
            stage_document(&request(source, caption), &self.dir.join("staging")).expect("stage");
        self.queue.enqueue(job).await.expect("enqueue");
        document
    }

    fn uploads(&self) -> Vec<(String, Vec<u8>)> {
        self.api.uploads.lock().expect("uploads").clone()
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn document_is_uploaded_with_send_document_and_binds_the_reply() {
    let f = Fixture::new(None).await;
    f.registry
        .put(SeatDescriptor::new(
            "pij-sender",
            Harness::Omp,
            "/work/my-repo",
        ))
        .await
        .expect("register sender");
    let document = f.enqueue(Some("pij attachment canary")).await;
    f.worker.run_once(1).await.expect("send");

    let uploads = f.uploads();
    assert_eq!(uploads.len(), 1, "exactly one sendDocument");
    let (content_type, body) = &uploads[0];
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .expect("multipart content type");
    assert!(contains(
        body,
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n42\r\n")
            .as_bytes()
    ));
    assert!(contains(
        body,
        b"name=\"caption\"\r\n\r\n[pij-sender] [my-repo] pij attachment canary\r\n"
    ));
    assert!(contains(body, b"filename=\"report.html\""));
    assert!(contains(body, HTML));
    assert!(
        f.api.texts.lock().expect("texts").is_empty(),
        "no sendMessage"
    );

    assert!(matches!(f.queue.acked().as_slice(), [(_, Outcome::Done)]));
    let events = f.spine.tail(None, Seq(0)).await.expect("events");
    let binding = events
        .iter()
        .find(|event| event.kind == "telegram.binding")
        .expect("reply binding");
    assert_eq!(binding.seat, Some(SeatId::from("pij-telegram-chat-42")));
    let binding: Value = serde_json::from_str(&binding.payload).expect("binding");
    assert_eq!(binding["target"], "pij-sender");
    assert_eq!(binding["outbound_msg_id"], "doc-1");
    assert!(
        !document.staged.exists(),
        "a delivered staged copy is released"
    );
}

#[tokio::test]
async fn a_rejected_upload_fails_the_job_and_never_binds() {
    let f = Fixture::new(Some(StatusCode::BAD_REQUEST)).await;
    let document = f.enqueue(None).await;
    let _ = f.worker.run_once(1).await;
    assert_eq!(f.uploads().len(), 1);
    match f.queue.acked().as_slice() {
        [(_, Outcome::Failed { reason })] => {
            assert!(reason.contains("file is too big"), "{reason}");
            assert!(!reason.contains("TOKEN"), "{reason}");
        }
        other => panic!("a 400 must fail the job, got {other:?}"),
    }
    assert!(f.queue.retried().is_empty(), "a 400 is permanent");
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["msg_id"], "doc-1");
    assert_eq!(failed[0]["http_status"], 400);
    assert_eq!(failed[0]["terminal"], true);
    assert_eq!(failed[0]["document"]["sha256"], HTML_SHA256);
    assert!(f.events("telegram.binding").await.is_empty());
    assert!(document.staged.exists(), "a failed send keeps its evidence");
}

#[tokio::test]
async fn a_server_error_retries_then_succeeds() {
    let f = Fixture::new(Some(StatusCode::BAD_GATEWAY)).await;
    f.enqueue(None).await;
    f.worker.run_once(1).await.expect_err("5xx retries");
    assert!(f.queue.acked().is_empty(), "never acknowledged as sent");
    assert_eq!(f.queue.retried().len(), 1);
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed[0]["http_status"], 502);
    assert_eq!(failed[0]["terminal"], false);

    *f.api.status.lock().expect("status") = None;
    f.queue.advance(Duration::from_secs(1));
    f.worker.run_once(1).await.expect("retry succeeds");
    assert_eq!(f.uploads().len(), 2);
    assert!(matches!(f.queue.acked().as_slice(), [(_, Outcome::Done)]));
}

#[tokio::test]
async fn a_tampered_staged_copy_fails_without_uploading() {
    let f = Fixture::new(None).await;
    let document = f.enqueue(None).await;
    std::fs::write(&document.staged, b"not what was hashed").expect("tamper");
    let _ = f.worker.run_once(1).await;
    assert!(
        f.uploads().is_empty(),
        "nothing other than the hashed bytes is sent"
    );
    match f.queue.acked().as_slice() {
        [(_, Outcome::Failed { reason })] => {
            assert!(reason.contains("changed since enqueue"), "{reason}")
        }
        other => panic!("tampering must fail the job, got {other:?}"),
    }
    assert_eq!(f.events("telegram.outbound-delivery-failed").await.len(), 1);
}

#[tokio::test]
async fn a_missing_staged_copy_fails_without_uploading() {
    let f = Fixture::new(None).await;
    let document = f.enqueue(None).await;
    std::fs::remove_file(&document.staged).expect("remove");
    let _ = f.worker.run_once(1).await;
    assert!(f.uploads().is_empty());
    assert!(matches!(
        f.queue.acked().as_slice(),
        [(_, Outcome::Failed { .. })]
    ));
}

#[tokio::test]
async fn a_foreign_chat_is_refused_before_upload() {
    let f = Fixture::new(None).await;
    let source = write(&f.dir, "r.html", HTML);
    let mut request = request(source, None);
    request.chat_id = Some("999".into());
    let (job, _) = stage_document(&request, &f.dir.join("staging")).expect("stage");
    f.queue.enqueue(job).await.expect("enqueue");
    let _ = f.worker.run_once(1).await;
    assert!(f.uploads().is_empty());
    assert!(matches!(
        f.queue.acked().as_slice(),
        [(_, Outcome::Failed { .. })]
    ));
}
