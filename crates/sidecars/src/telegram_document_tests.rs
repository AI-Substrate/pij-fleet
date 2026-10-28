use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as UrlPath, State},
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
    CAPTION_LIMIT, DocumentJob, MAX_FILES, MediaMethod, PHOTO_LIMIT_BYTES, StagedDocument,
    StagedSend, TelegramDocumentRequest, boundary_for, media_multipart, prefixed_caption,
    stage_send, stage_with_limit,
};

const HTML: &[u8] = b"<html><body>pij attachment canary</body></html>\n";
// printf '<html><body>pij attachment canary</body></html>\n' | shasum -a 256
const HTML_SHA256: &str = "0fa42fe2449c4b1a4bf5790c9b19d0c1a3ace34ecc5177034e06afa19a418590";

fn request(path: PathBuf, caption: Option<&str>) -> TelegramDocumentRequest {
    files_request(vec![path], caption, None)
}

fn files_request(
    files: Vec<PathBuf>,
    caption: Option<&str>,
    body: Option<&str>,
) -> TelegramDocumentRequest {
    TelegramDocumentRequest {
        from: SeatId::from("pij-sender"),
        files,
        caption: caption.map(str::to_string),
        body: body.map(str::to_string),
        msg_id: "doc-1".into(),
        chat_id: None,
    }
}

/// Stage a one-file request and return its only job.
fn stage_document(
    request: &TelegramDocumentRequest,
    staging: &Path,
) -> pij_core::error::Result<(pij_core::model::Job, StagedDocument)> {
    single(stage_send(request, staging))
}

fn single(
    staged: pij_core::error::Result<StagedSend>,
) -> pij_core::error::Result<(pij_core::model::Job, StagedDocument)> {
    let StagedSend { body, mut files } = staged?;
    assert!(body.is_none(), "a one-file request queues no body");
    assert_eq!(files.len(), 1, "a one-file request queues one job");
    Ok(files.remove(0))
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
    let (content_type, body) = media_multipart(
        "B",
        "document",
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
    let (_, body) = media_multipart("B", "document", "42", "c", "bin", &bytes);
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
            Some(limit) => single(stage_with_limit(&request, &staging, limit)),
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
    /// (method, content type, body) per file upload, in arrival order.
    uploads: Mutex<Vec<(String, String, Vec<u8>)>>,
    texts: Mutex<Vec<Value>>,
    /// Every Bot API call, in arrival order.
    calls: Mutex<Vec<String>>,
    /// Refusal status for `sendDocument`.
    status: Mutex<Option<StatusCode>>,
    /// Refusal status for `sendPhoto`, `sendVideo` and `sendAnimation`.
    media_status: Mutex<Option<StatusCode>>,
    /// Answer HTTP 200 with `ok: false` to every upload.
    not_ok: Mutex<bool>,
}

async fn upload(
    State(api): State<Arc<Api>>,
    UrlPath(method): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    api.calls.lock().expect("calls").push(method.clone());
    api.uploads
        .lock()
        .expect("uploads")
        .push((method.clone(), content_type, body.to_vec()));
    let refusal = if method == "sendDocument" {
        *api.status.lock().expect("status")
    } else {
        *api.media_status.lock().expect("media status")
    };
    match refusal {
        Some(status) => {
            let description = if method == "sendDocument" {
                "Bad Request: file is too big"
            } else {
                "Bad Request: PHOTO_INVALID_DIMENSIONS"
            };
            (
                status,
                Json(
                    json!({"ok": false, "error_code": status.as_u16(), "description": description}),
                ),
            )
        }
        None if *api.not_ok.lock().expect("not ok") => (
            StatusCode::OK,
            Json(json!({"ok": false, "description": "Bad Request: not ok"})),
        ),
        None => (
            StatusCode::OK,
            Json(json!({"ok": true, "result": {"message_id": 1}})),
        ),
    }
}

async fn send_message(State(api): State<Arc<Api>>, Json(body): Json<Value>) -> Json<Value> {
    api.calls.lock().expect("calls").push("sendMessage".into());
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
        Self::with_token("TOKEN", status).await
    }

    async fn with_token(token: &str, status: Option<StatusCode>) -> Self {
        let api = Arc::new(Api {
            status: Mutex::new(status),
            ..Api::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub");
        let address = listener.local_addr().expect("stub address");
        let app = Router::new()
            .route(&format!("/bot{token}/sendMessage"), post(send_message))
            .route(
                &format!("/bot{token}/getUpdates"),
                get(|| async { Json(json!({"ok": true, "result": []})) }),
            )
            .route(&format!("/bot{token}/{{method}}"), post(upload))
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
                token: token.into(),
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

    async fn enqueue(&self, caption: Option<&str>) -> StagedDocument {
        self.enqueue_named("report.html", HTML, caption).await
    }

    async fn enqueue_named(
        &self,
        name: &str,
        bytes: &[u8],
        caption: Option<&str>,
    ) -> StagedDocument {
        let source = write(&self.dir, name, bytes);
        let mut request = request(source, caption);
        if name != "report.html" {
            request.msg_id = format!("doc-{name}");
        }
        let (job, document) = stage_document(&request, &self.dir.join("staging")).expect("stage");
        self.queue.enqueue(job).await.expect("enqueue");
        document
    }

    fn uploads(&self) -> Vec<(String, Vec<u8>)> {
        self.api
            .uploads
            .lock()
            .expect("uploads")
            .iter()
            .map(|(_, content_type, body)| (content_type.clone(), body.clone()))
            .collect()
    }

    fn methods(&self) -> Vec<String> {
        self.api
            .uploads
            .lock()
            .expect("uploads")
            .iter()
            .map(|(method, _, _)| method.clone())
            .collect()
    }

    fn calls(&self) -> Vec<String> {
        self.api.calls.lock().expect("calls").clone()
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
    f.queue
        .advance(f.queue.retried().last().expect("a retry").1);
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

// ------------------------------------------------- plan 179: method by type

#[test]
fn the_method_is_chosen_by_extension_and_photo_size() {
    for (name, size, expected) in [
        ("chart.png", 1, MediaMethod::Photo),
        ("CHART.PNG", 1, MediaMethod::Photo),
        ("shot.jpg", 1, MediaMethod::Photo),
        ("shot.JPEG", 1, MediaMethod::Photo),
        ("still.webp", 1, MediaMethod::Photo),
        ("edge.png", PHOTO_LIMIT_BYTES, MediaMethod::Photo),
        ("huge.png", PHOTO_LIMIT_BYTES + 1, MediaMethod::Document),
        ("demo.mp4", 1, MediaMethod::Video),
        ("DEMO.MP4", 40 * 1024 * 1024, MediaMethod::Video),
        ("loop.gif", 1, MediaMethod::Animation),
        ("report.html", 1, MediaMethod::Document),
        ("deck.pptx", 1, MediaMethod::Document),
        ("README", 1, MediaMethod::Document),
        (".png", 1, MediaMethod::Document),
        ("photo.png.exe", 1, MediaMethod::Document),
        ("clip.mov", 1, MediaMethod::Document),
    ] {
        assert_eq!(
            MediaMethod::classify(name, size),
            expected,
            "{name} ({size} bytes)"
        );
    }
    assert_eq!(PHOTO_LIMIT_BYTES, 10 * 1024 * 1024);
    assert_eq!(MAX_FILES, 10);
}

#[test]
fn each_method_names_its_api_call_and_file_field() {
    for (method, api, field) in [
        (MediaMethod::Photo, "sendPhoto", "photo"),
        (MediaMethod::Video, "sendVideo", "video"),
        (MediaMethod::Animation, "sendAnimation", "animation"),
        (MediaMethod::Document, "sendDocument", "document"),
    ] {
        assert_eq!(method.api_method(), api);
        assert_eq!(method.field(), field);
        assert_eq!(serde_json::to_value(method).expect("encode"), json!(api));
        let (_, body) = media_multipart("B", field, "42", "c", "x.bin", b"x");
        assert!(contains(
            &body,
            format!("name=\"{field}\"; filename=\"x.bin\"").as_bytes()
        ));
    }
    // A payload staged before plan 179 carries no method: it is a document.
    let legacy: StagedDocument = serde_json::from_value(json!({
        "source": "/s", "staged": "/t", "file_name": "r.html", "size": 1, "sha256": "ab"
    }))
    .expect("legacy payload");
    assert_eq!(legacy.method, MediaMethod::Document);
}

#[test]
fn staging_records_the_method_on_the_job() {
    let dir = fresh_dir("telegram-media-method");
    let source = write(&dir, "chart.png", b"not really a png");
    let (job, document) =
        stage_document(&request(source, None), &dir.join("staging")).expect("staged");
    assert_eq!(document.method, MediaMethod::Photo);
    let payload: Value = serde_json::from_str(&job.payload).expect("payload");
    assert_eq!(payload["document"]["method"], "sendPhoto");
    assert_eq!(payload["document"]["file_name"], "chart.png");
    assert!(
        !job.payload.contains("not really a png"),
        "the payload records the file, never its bytes"
    );
    std::fs::remove_dir_all(dir).expect("cleanup");
}

// --------------------------------------------- plan 179: several files, body

#[test]
fn several_files_stage_in_order_with_their_own_msg_ids_and_the_caption_on_the_first() {
    let dir = fresh_dir("telegram-media-several");
    let files = vec![
        write(&dir, "a.png", b"one"),
        write(&dir, "b.mp4", b"two"),
        write(&dir, "c.html", HTML),
        write(&dir, "d.html", HTML),
    ];
    let staged = stage_send(
        &files_request(files.clone(), Some("look"), None),
        &dir.join("staging"),
    )
    .expect("staged");
    assert!(staged.body.is_none());
    let jobs: Vec<DocumentJob> = staged
        .files
        .iter()
        .map(|(job, _)| serde_json::from_str(&job.payload).expect("payload"))
        .collect();
    let keys: Vec<_> = staged
        .files
        .iter()
        .map(|(job, _)| job.dedupe_key.as_str())
        .collect();
    assert_eq!(keys, ["doc-1#1", "doc-1#2", "doc-1#3", "doc-1#4"]);
    let ids: Vec<_> = jobs.iter().map(|job| job.msg_id.as_str()).collect();
    assert_eq!(ids, keys, "each job's msg_id is its dedupe key");
    let captions: Vec<_> = jobs.iter().map(|job| job.caption.as_str()).collect();
    assert_eq!(
        captions,
        ["look", "", "", ""],
        "the caption goes on the first file"
    );
    let methods: Vec<_> = jobs.iter().map(|job| job.document.method).collect();
    assert_eq!(
        methods,
        [
            MediaMethod::Photo,
            MediaMethod::Video,
            MediaMethod::Document,
            MediaMethod::Document
        ]
    );
    let sources: Vec<_> = jobs.iter().map(|job| job.document.source.clone()).collect();
    assert_eq!(sources, files, "argument order");
    assert!(
        staged
            .files
            .iter()
            .all(|(job, _)| job.serial_key == "default")
    );
    // Identical bytes in two files are still two copies, released separately.
    let (c, d) = (&staged.files[2].1, &staged.files[3].1);
    assert_eq!(c.sha256, d.sha256);
    assert_ne!(c.staged, d.staged);
    assert!(c.staged.exists() && d.staged.exists());
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn a_body_is_queued_first_as_an_ordinary_text_send() {
    let dir = fresh_dir("telegram-media-body");
    let staged = stage_send(
        &files_request(vec![write(&dir, "a.png", b"one")], None, Some("here it is")),
        &dir.join("staging"),
    )
    .expect("staged");
    let body = staged.body.expect("a body job");
    assert_eq!(body.kind, super::super::TELEGRAM_SEND_KIND);
    assert_eq!(body.dedupe_key, "doc-1");
    let text: super::super::TelegramSend = serde_json::from_str(&body.payload).expect("text");
    assert_eq!(text.body, "here it is");
    assert_eq!(text.from, SeatId::from("pij-sender"));
    assert_eq!(
        body.serial_key, staged.files[0].0.serial_key,
        "same chat queue"
    );
    assert_eq!(
        staged.files[0].0.dedupe_key, "doc-1#1",
        "with a body, the one file is numbered"
    );
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn one_refused_file_refuses_the_whole_send_and_leaves_nothing_staged() {
    let dir = fresh_dir("telegram-media-atomic");
    let staging = dir.join("staging");
    let files = vec![
        write(&dir, "a.png", b"one"),
        dir.join("absent.pdf"),
        write(&dir, "c.html", HTML),
    ];
    let error = stage_send(&files_request(files, None, Some("body")), &staging)
        .expect_err("one bad file refuses the send");
    assert!(
        message(&error).contains("cannot be read"),
        "{}",
        message(&error)
    );
    let left = std::fs::read_dir(&staging)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "copies staged before the refusal are removed");
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn no_files_and_more_than_ten_refuse_before_staging() {
    let dir = fresh_dir("telegram-media-count");
    let staging = dir.join("staging");
    let error =
        stage_send(&files_request(vec![], None, Some("x")), &staging).expect_err("no files");
    assert!(
        message(&error).contains("at least one"),
        "{}",
        message(&error)
    );
    let eleven: Vec<_> = (0..=MAX_FILES)
        .map(|n| write(&dir, &format!("f{n}.txt"), b"x"))
        .collect();
    let error = stage_send(&files_request(eleven, None, None), &staging).expect_err("eleven");
    assert!(
        message(&error).contains("at most 10"),
        "{}",
        message(&error)
    );
    let left = std::fs::read_dir(&staging)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "nothing staged");
    std::fs::remove_dir_all(dir).expect("cleanup");
}

// ------------------------------------------- plan 179: worker, by method

#[tokio::test]
async fn a_photo_goes_with_send_photo_in_the_photo_field_and_records_delivery() {
    let f = Fixture::new(None).await;
    let document = f
        .enqueue_named("chart.png", b"\x89PNG fake", Some("the chart"))
        .await;
    f.worker.run_once(1).await.expect("send");
    assert_eq!(f.methods(), ["sendPhoto"]);
    let (_, body) = &f.uploads()[0];
    assert!(contains(body, b"name=\"photo\"; filename=\"chart.png\""));
    assert!(contains(body, b"\r\n\r\n[pij-sender] the chart\r\n"));
    assert!(matches!(f.queue.acked().as_slice(), [(_, Outcome::Done)]));
    let delivered = f.events("telegram.outbound-media-delivered").await;
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert_eq!(delivered[0]["method"], "sendPhoto");
    assert_eq!(
        delivered[0]["telegram_message_id"], 1,
        "Telegram's own id for the delivered message"
    );
    assert_eq!(delivered[0]["msg_id"], "doc-chart.png");
    assert_eq!(delivered[0]["from"], "pij-sender");
    assert_eq!(delivered[0]["document"]["sha256"], document.sha256);
    assert_eq!(f.events("telegram.binding").await.len(), 1);
    assert!(!document.staged.exists());
}

#[tokio::test]
async fn a_gif_goes_as_an_animation_and_an_mp4_as_a_video() {
    let f = Fixture::new(None).await;
    f.enqueue_named("loop.gif", b"GIF89a", None).await;
    f.enqueue_named("demo.mp4", b"mp4 bytes", None).await;
    f.worker.run_once(2).await.expect("send both");
    assert_eq!(f.methods(), ["sendAnimation", "sendVideo"]);
    let uploads = f.uploads();
    assert!(contains(
        &uploads[0].1,
        b"name=\"animation\"; filename=\"loop.gif\""
    ));
    assert!(contains(
        &uploads[1].1,
        b"name=\"video\"; filename=\"demo.mp4\""
    ));
    assert_eq!(f.queue.acked().len(), 2);
}

#[tokio::test]
async fn a_400_on_a_photo_falls_back_to_send_document_in_the_same_attempt() {
    let f = Fixture::new(None).await;
    *f.api.media_status.lock().expect("media status") = Some(StatusCode::BAD_REQUEST);
    let document = f.enqueue_named("wide.png", b"very wide", None).await;
    f.worker.run_once(1).await.expect("the fallback delivers");
    assert_eq!(f.methods(), ["sendPhoto", "sendDocument"]);
    let uploads = f.uploads();
    assert!(contains(
        &uploads[1].1,
        b"name=\"document\"; filename=\"wide.png\""
    ));
    assert!(contains(&uploads[1].1, b"very wide"), "the same bytes");
    assert!(matches!(f.queue.acked().as_slice(), [(_, Outcome::Done)]));
    assert!(
        f.queue.retried().is_empty(),
        "the fallback is in the same attempt"
    );
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed.len(), 1, "the refused photo is still recorded");
    assert_eq!(failed[0]["method"], "sendPhoto");
    assert_eq!(failed[0]["fallback"], "sendDocument");
    assert_eq!(failed[0]["http_status"], 400);
    assert_eq!(failed[0]["terminal"], false);
    assert!(
        failed[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("PHOTO_INVALID_DIMENSIONS")),
        "{}",
        failed[0]
    );
    let delivered = f.events("telegram.outbound-media-delivered").await;
    assert_eq!(delivered[0]["method"], "sendDocument");
    assert_eq!(delivered[0]["telegram_message_id"], 1);
    assert_eq!(f.events("telegram.binding").await.len(), 1);
    assert!(!document.staged.exists());
}

#[tokio::test]
async fn a_gif_refused_with_400_falls_back_once_and_a_second_400_fails_the_job() {
    let f = Fixture::new(Some(StatusCode::BAD_REQUEST)).await;
    *f.api.media_status.lock().expect("media status") = Some(StatusCode::BAD_REQUEST);
    let document = f.enqueue_named("loop.gif", b"GIF89a", None).await;
    let _ = f.worker.run_once(1).await;
    assert_eq!(
        f.methods(),
        ["sendAnimation", "sendDocument"],
        "one fallback, never a loop"
    );
    match f.queue.acked().as_slice() {
        [(_, Outcome::Failed { reason })] => {
            assert!(reason.contains("file is too big"), "{reason}")
        }
        other => panic!("a refused fallback fails the job, got {other:?}"),
    }
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0]["method"], "sendAnimation");
    assert_eq!(failed[0]["fallback"], "sendDocument");
    assert_eq!(failed[0]["terminal"], false);
    assert_eq!(failed[1]["method"], "sendDocument");
    assert_eq!(failed[1]["fallback"], Value::Null);
    assert_eq!(failed[1]["terminal"], true);
    assert!(f.events("telegram.binding").await.is_empty());
    assert!(
        f.events("telegram.outbound-media-delivered")
            .await
            .is_empty()
    );
    assert!(document.staged.exists(), "a failed send keeps its evidence");
}

#[tokio::test]
async fn a_server_error_on_a_photo_retries_without_falling_back() {
    let f = Fixture::new(None).await;
    *f.api.media_status.lock().expect("media status") = Some(StatusCode::BAD_GATEWAY);
    f.enqueue_named("chart.png", b"png", None).await;
    f.worker.run_once(1).await.expect_err("5xx retries");
    assert_eq!(f.methods(), ["sendPhoto"], "no fallback on 5xx");
    assert_eq!(f.queue.retried().len(), 1);
    let failed = f.events("telegram.outbound-delivery-failed").await;
    assert_eq!(failed[0]["method"], "sendPhoto");
    assert_eq!(failed[0]["http_status"], 502);
    assert_eq!(failed[0]["terminal"], false);
    assert_eq!(failed[0]["fallback"], Value::Null);
}

#[tokio::test]
async fn the_body_and_then_each_file_arrive_in_order() {
    let f = Fixture::new(None).await;
    let files = vec![
        write(&f.dir, "a.png", b"one"),
        write(&f.dir, "b.html", HTML),
        write(&f.dir, "c.mp4", b"three"),
    ];
    let staged = stage_send(
        &files_request(files, Some("first"), Some("three files")),
        &f.dir.join("staging"),
    )
    .expect("stage");
    let jobs = staged
        .body
        .into_iter()
        .chain(staged.files.into_iter().map(|(job, _)| job));
    for job in jobs {
        f.queue.enqueue(job).await.expect("enqueue");
    }
    f.worker.run_once(10).await.expect("send all");
    assert_eq!(
        f.calls(),
        ["sendMessage", "sendPhoto", "sendDocument", "sendVideo"]
    );
    assert_eq!(f.queue.acked().len(), 4);
    assert!(
        f.queue
            .acked()
            .iter()
            .all(|(_, outcome)| matches!(outcome, Outcome::Done))
    );
    let uploads = f.uploads();
    assert!(contains(&uploads[0].1, b"\r\n\r\n[pij-sender] first\r\n"));
    assert!(
        contains(&uploads[1].1, b"\r\n\r\n[pij-sender]\r\n"),
        "tag only"
    );
    let bindings: Vec<_> = f
        .events("telegram.binding")
        .await
        .into_iter()
        .map(|binding| binding["outbound_msg_id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        bindings,
        [
            Some("doc-1".to_string()),
            Some("doc-1#1".to_string()),
            Some("doc-1#2".to_string()),
            Some("doc-1#3".to_string())
        ]
    );
}

// ------------------------------------------------- plan 179: redaction

/// The bot token rides in every Bot API URL. It must not reach a job payload,
/// a job outcome, a spine event or the error the loop logs, on any path.
#[tokio::test]
async fn the_token_never_leaks_on_any_media_failure_path() {
    const SECRET: &str = "4242:SENTINEL-TOKEN-plan179";
    let leaks = |text: &str| text.contains(SECRET) || text.contains("SENTINEL");
    for case in ["bad-request", "not-ok", "server-error", "transport"] {
        let mut f = Fixture::with_token(SECRET, None).await;
        match case {
            "bad-request" => {
                *f.api.media_status.lock().expect("media status") = Some(StatusCode::BAD_REQUEST);
                *f.api.status.lock().expect("status") = Some(StatusCode::BAD_REQUEST);
            }
            "not-ok" => *f.api.not_ok.lock().expect("not ok") = true,
            "server-error" => {
                *f.api.media_status.lock().expect("media status") = Some(StatusCode::BAD_GATEWAY)
            }
            _ => f.worker.config.api_root = "http://127.0.0.1:1".into(),
        }
        let source = write(&f.dir, "chart.png", b"png");
        let (job, _) =
            stage_document(&request(source, Some("c")), &f.dir.join("staging")).expect("stage");
        assert!(!leaks(&job.payload), "{case}: payload {}", job.payload);
        f.queue.enqueue(job.clone()).await.expect("enqueue");
        // Plan 183 N1: a conflicting reuse of the msg_id hands back the live
        // row's payload; it carries no credential either.
        let mut reuse = job;
        reuse.payload = reuse.payload.replace("\"c\"", "\"other caption\"");
        match f.queue.enqueue_all(vec![reuse]).await.expect("enqueue_all") {
            pij_core::ports::BatchEnqueue::Conflict { live_payload, .. } => {
                assert!(!leaks(&live_payload), "{case}: live payload {live_payload}");
            }
            other => panic!("{case}: a reused msg_id with other content conflicts, got {other:?}"),
        }
        let result = f.worker.run_once(1).await;
        if let Err(error) = &result {
            assert!(!leaks(&error.to_string()), "{case}: logged error {error}");
        }
        for (_, outcome) in f.queue.acked() {
            if let Outcome::Failed { reason } = outcome {
                assert!(!leaks(&reason), "{case}: job outcome {reason}");
            }
        }
        let mut failures = 0;
        for event in f.spine.tail(None, Seq(0)).await.expect("events") {
            assert!(
                !leaks(&event.payload),
                "{case}: {} {}",
                event.kind,
                event.payload
            );
            failures += usize::from(event.kind == "telegram.outbound-delivery-failed");
        }
        assert!(failures > 0, "{case}: the failure path was exercised");
    }
}

// ------------------------------------------------- plan 183 N2: retention

/// Failed jobs' copies go after the retention; younger ones and a missing
/// directory are left alone. Clock injected: no sleeping.
#[test]
fn the_sweep_removes_copies_older_than_the_retention_only() {
    use std::time::{Duration, SystemTime};
    let dir = fresh_dir("telegram-doc-sweep");
    let staging = dir.join("staging");
    std::fs::create_dir_all(&staging).expect("staging");
    let now = SystemTime::now();
    let week = Duration::from_secs(7 * 24 * 3600);
    let aged = |name: &str, age: Duration| {
        let path = write(&staging, name, b"evidence");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(now - age)
            .expect("set mtime");
        path
    };
    let old = aged("aaaa-1111", week + Duration::from_secs(60));
    let partial = aged(".partial-1-2-3", week * 2);
    let young = aged("bbbb-2222", week - Duration::from_secs(60));
    assert_eq!(super::sweep_staged(&staging, now, week).expect("sweep"), 2);
    assert!(!old.exists() && !partial.exists());
    assert!(young.exists(), "inside the retention: kept as evidence");
    assert_eq!(
        super::sweep_staged(&dir.join("never-made"), now, week).expect("absent dir"),
        0
    );
    std::fs::remove_dir_all(dir).expect("cleanup");
}
