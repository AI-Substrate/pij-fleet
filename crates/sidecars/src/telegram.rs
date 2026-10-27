//! Telegram bridge: durable outbound jobs and single-consumer inbound polling.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, Job, Outcome, SeatId};
use pij_core::ports::{Queue, Registry, Spine};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::common::{LoopHandle, Processed, enqueue_turn, now_ms, start_resilient_loop};

#[path = "telegram_document.rs"]
mod document;

pub use document::{
    CAPTION_LIMIT, DOCUMENT_LIMIT_BYTES, MAX_FILES, MediaMethod, PHOTO_LIMIT_BYTES, StagedDocument,
    StagedSend, TelegramDocumentRequest, release_staged, stage_send,
};

/// Queue kind for outbound Telegram work.
pub const TELEGRAM_SEND_KIND: &str = "sidecar:telegram:send";
/// Queue kind for an outbound Telegram document.
///
/// Its own kind, not a field on [`TELEGRAM_SEND_KIND`]: a daemon that predates
/// documents never claims it, so the row waits rather than being decoded as a
/// text send with the attachment silently dropped.
pub const TELEGRAM_DOCUMENT_KIND: &str = "sidecar:telegram:document";
const WORKER_ID: &str = "sidecar-telegram";
const BINDING_KIND: &str = "telegram.binding";
const CURSOR_KIND: &str = "telegram.cursor";
const DELIVERY_FAILED_KIND: &str = "telegram.inbound-delivery-failed";
const OUTBOUND_FAILED_KIND: &str = "telegram.outbound-delivery-failed";
const OUTBOUND_MEDIA_DELIVERED_KIND: &str = "telegram.outbound-media-delivered";
const INBOUND_MEDIA_UNSUPPORTED_KIND: &str = "telegram.inbound-media-unsupported";
const INBOUND_ENQUEUED_KIND: &str = "telegram.inbound-enqueued";
const CURSOR_SEAT: &str = "pij-telegram-cursor";
const REFUSAL_KIND: &str = "telegram.inbound-refused";
// Match the TS bridge's margin below Telegram's 4096-character hard cap.
const TEXT_LIMIT: usize = 4000;
const PREFIX_LIMIT: usize = 1024;

/// Validated Telegram credentials loaded from the legacy file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TelegramConfig {
    /// Bot token.
    pub token: String,
    /// Telegram users allowed to inject turns.
    pub allowed_user_ids: Vec<i64>,
    /// Default outbound conversation.
    pub chat_id: String,
    /// Bot API root; injectable for deterministic tests.
    pub api_root: String,
}

impl TelegramConfig {
    /// Read the legacy `telegram.env` without mutating process environment.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|error| PijError::Adapter {
            adapter: "sidecars/telegram".to_string(),
            message: format!(
                "could not read telegram credentials {}: {error}",
                path.display()
            ),
        })?;
        let value = |key: &str| {
            text.lines().find_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let (found, value) = line.split_once('=')?;
                (found.trim() == key).then(|| value.trim().trim_matches(['\'', '"']).to_string())
            })
        };
        let token = value("TELEGRAM_BOT_TOKEN")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| PijError::Adapter {
                adapter: "sidecars/telegram".to_string(),
                message: format!("TELEGRAM_BOT_TOKEN is missing from {}", path.display()),
            })?;
        let chat_id = value("TELEGRAM_CHAT_ID")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| PijError::Adapter {
                adapter: "sidecars/telegram".to_string(),
                message: format!("TELEGRAM_CHAT_ID is missing from {}", path.display()),
            })?;
        let allowed_user_ids = value("TELEGRAM_ALLOWED_USER_IDS")
            .unwrap_or_default()
            .split(',')
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                value
                    .trim()
                    .parse::<i64>()
                    .map_err(|error| PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!(
                            "invalid TELEGRAM_ALLOWED_USER_IDS entry `{value}` in {}: {error}",
                            path.display()
                        ),
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            token,
            allowed_user_ids,
            chat_id,
            api_root: "https://api.telegram.org".to_string(),
        })
    }
}

/// Durable request submitted by the CLI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramSend {
    /// Seat whose later reply binding this send establishes.
    pub from: SeatId,
    /// Message text.
    pub body: String,
    /// Stable request id.
    pub msg_id: String,
    /// Conversation override; absent uses the credential file's chat id.
    pub chat_id: Option<String>,
}

/// Build one queue row for an outbound Telegram send.
pub fn send_job(request: &TelegramSend) -> Result<Job> {
    Ok(Job {
        kind: TELEGRAM_SEND_KIND.to_string(),
        serial_key: request
            .chat_id
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        payload: serde_json::to_string(request).map_err(|error| PijError::Adapter {
            adapter: "sidecars/telegram".to_string(),
            message: format!("could not encode Telegram request: {error}"),
        })?,
        dedupe_key: request.msg_id.clone(),
        dedupe_origin: None,
        attempt: 0,
    })
}

#[derive(Debug)]
struct BridgeLock {
    path: PathBuf,
    pid: u32,
}

fn parse_lock_holder(raw: &str) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let legacy = value
        .get("startedAt")
        .is_some_and(serde_json::Value::is_string);
    let transitional_rust = value
        .get("startedAtMs")
        .is_some_and(serde_json::Value::is_u64);
    (legacy || transitional_rust).then_some(pid)
}

impl BridgeLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let pid = std::process::id();
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    // Byte-shape compatible with the legacy bridge: it treats a
                    // missing/non-string `startedAt` as corrupt and reclaims the
                    // live lock. The value is diagnostic; mutual exclusion rests
                    // on pid + string presence.
                    let body = serde_json::json!({
                        "pid": pid,
                        "startedAt": format!("unix-ms:{}", now_ms()?),
                    });
                    file.write_all(body.to_string().as_bytes())
                        .map_err(|error| PijError::Adapter {
                            adapter: "sidecars/telegram".to_string(),
                            message: format!(
                                "could not write Telegram lock {}: {error}",
                                path.display()
                            ),
                        })?;
                    file.sync_all().map_err(|error| PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!(
                            "could not sync Telegram lock {}: {error}",
                            path.display()
                        ),
                    })?;
                    return Ok(Self { path, pid });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let raw = fs::read_to_string(&path).map_err(|read_error| PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!("could not read existing Telegram lock {} after {error}: {read_error}", path.display()),
                    })?;
                    let holder = parse_lock_holder(&raw).ok_or_else(|| PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!("refusing to reclaim unrecognised Telegram lock {}: somebody may hold the single-consumer poll", path.display()),
                    })?;
                    if process_exists(holder) {
                        return Err(PijError::Adapter {
                            adapter: "sidecars/telegram".to_string(),
                            message: format!(
                                "Telegram getUpdates lock {} is held by live pid {holder}",
                                path.display()
                            ),
                        });
                    }
                    fs::remove_file(&path).map_err(|remove_error| PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!("could not reclaim parsed dead-holder Telegram lock {} after {error}: {remove_error}", path.display()),
                    })?;
                }
                Err(error) => {
                    return Err(PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!(
                            "could not acquire Telegram lock {}: {error}",
                            path.display()
                        ),
                    });
                }
            }
        }
        Err(PijError::Adapter {
            adapter: "sidecars/telegram".to_string(),
            message: format!(
                "could not acquire Telegram lock {} after stale-lock retry",
                path.display()
            ),
        })
    }
}

impl Drop for BridgeLock {
    fn drop(&mut self) {
        let ours = fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|value| value.get("pid")?.as_u64())
            == Some(u64::from(self.pid));
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn process_exists(pid: u32) -> bool {
    // ERRNO, NOT A SHELLED-OUT EXIT CODE. `/bin/kill -0 <pid>` answered correctly on
    // macOS and wrongly on the Linux runner for an out-of-range pid, which is how
    // CI's first Linux run found this: the probe's answer depended on which `kill`
    // implementation was on the box.
    //
    // A pid that does not fit `pid_t` CANNOT NAME A PROCESS, so it is absent by
    // construction — and refusing it here also keeps a u32::MAX from ever reaching
    // a signal call, where `-1` means EVERY PROCESS.
    //
    // Directional, like the background runner's group probe: ESRCH PROVES absence;
    // EPERM proves the opposite (it exists and is not ours); anything else is not
    // absence and must not be read as one.
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None) {
        Ok(()) => true,
        Err(nix::errno::Errno::ESRCH) => false,
        Err(_) => true,
    }
}

/// The concrete Telegram queue consumer.
pub struct TelegramWorker {
    queue: Arc<dyn Queue>,
    spine: Arc<dyn Spine>,
    registry: Arc<dyn Registry>,
    client: reqwest::Client,
    config: TelegramConfig,
    _lock: BridgeLock,
}

impl TelegramWorker {
    /// Construct and acquire the single-consumer getUpdates lock.
    pub fn new(
        queue: Arc<dyn Queue>,
        spine: Arc<dyn Spine>,
        registry: Arc<dyn Registry>,
        config: TelegramConfig,
        lock_path: PathBuf,
    ) -> Result<Self> {
        Ok(Self {
            queue,
            spine,
            registry,
            client: reqwest::Client::new(),
            config,
            _lock: BridgeLock::acquire(lock_path)?,
        })
    }

    /// Start the shipped resilient loop. A failed pass is logged; later passes continue.
    pub fn start(self: Arc<Self>, interval: Duration, limit: usize) -> Result<LoopHandle> {
        start_resilient_loop("telegram", interval, move || {
            let worker = Arc::clone(&self);
            async move { worker.run_once(limit).await }
        })
    }

    /// Claim at most `limit` outbound rows, then poll inbound once.
    pub async fn run_once(&self, limit: usize) -> Result<Processed> {
        let mut count = 0;
        while count < limit {
            let Some((id, job)) = self
                .queue
                .claim(
                    &[
                        TELEGRAM_SEND_KIND.to_string(),
                        TELEGRAM_DOCUMENT_KIND.to_string(),
                    ],
                    WORKER_ID,
                )
                .await?
            else {
                break;
            };
            let outcome = if job.kind == TELEGRAM_DOCUMENT_KIND {
                self.process_document(&job).await
            } else {
                self.process_send(&job).await
            };
            match outcome {
                SendOutcome::Done => self.queue.ack(id, Outcome::Done).await?,
                SendOutcome::Retry(error) => {
                    self.queue.retry(id, Duration::from_millis(100)).await?;
                    return Err(error);
                }
                SendOutcome::Terminal(error) => {
                    self.queue
                        .ack(
                            id,
                            Outcome::Failed {
                                reason: error.to_string(),
                            },
                        )
                        .await?;
                    eprintln!("pij-rs telegram: {error}");
                }
            }
            count += 1;
        }
        self.poll_inbound().await?;
        Ok(Processed { count })
    }

    async fn process_send(&self, job: &Job) -> SendOutcome {
        let request: TelegramSend = match serde_json::from_str(&job.payload) {
            Ok(request) => request,
            Err(error) => {
                return SendOutcome::Terminal(PijError::Adapter {
                    adapter: "sidecars/telegram".to_string(),
                    message: format!("job payload is not a Telegram send: {error}"),
                });
            }
        };
        let chat_id = request
            .chat_id
            .as_deref()
            .unwrap_or(&self.config.chat_id)
            .to_string();
        if chat_id != self.config.chat_id {
            return SendOutcome::Terminal(PijError::Adapter {
                adapter: "sidecars/telegram".to_string(),
                message: format!(
                    "refusing Telegram egress to chat `{chat_id}`: configured operator chat is `{}`",
                    self.config.chat_id
                ),
            });
        }
        let sender = match self.registry.get(&request.from).await {
            Ok(sender) => sender,
            Err(error) => return SendOutcome::Retry(error),
        };
        // TS resolveRepositoryContext consulted git for repository/worktree identity.
        // The recorded folder's basename is enough to orient the operator here,
        // without a subprocess or filesystem lookup on every send.
        let context = sender
            .as_ref()
            .and_then(|seat| Path::new(&seat.folder).file_name())
            .and_then(|name| name.to_str());
        let parts = match prefixed_text_parts(&request.from, context, &request.body) {
            Ok(parts) => parts,
            Err(error) => return SendOutcome::Terminal(error),
        };
        let url = format!(
            "{}/bot{}/sendMessage",
            self.config.api_root, self.config.token
        );
        for (index, text) in parts.iter().enumerate() {
            let mut status = None;
            let result = async {
                let response = self
                    .client
                    .post(&url)
                    .json(&serde_json::json!({"chat_id": chat_id, "text": text}))
                    .send()
                    .await
                    .map_err(http_error)?;
                status = Some(response.status().as_u16());
                if !response.status().is_success() {
                    return Err(PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: format!("sendMessage returned HTTP {}", response.status()),
                    });
                }
                let response: SendResponse = response.json().await.map_err(http_error)?;
                if !response.ok {
                    return Err(PijError::Adapter {
                        adapter: "sidecars/telegram".to_string(),
                        message: "sendMessage returned ok=false".to_string(),
                    });
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                let mut detail = serde_json::json!({
                    "conversation": chat_id,
                    "from": request.from,
                    "msg_id": request.msg_id,
                    "attempt": u64::from(job.attempt) + 1,
                    "part": index + 1,
                    "parts": parts.len(),
                    "http_status": status,
                    "reason": error.to_string(),
                });
                let recorded = async {
                    self.spine
                        .append(Event {
                            seq: None,
                            v: 1,
                            at: now_ms()?,
                            kind: OUTBOUND_FAILED_KIND.to_string(),
                            seat: Some(conversation_seat(&chat_id)),
                            payload: detail.to_string(),
                        })
                        .await
                }
                .await;
                if let Err(error) = recorded {
                    detail["persistence_error"] = serde_json::json!(error.to_string());
                }
                // AC5a preserves whole-job retries, including already sent parts.
                // The failed attempt/part is durable; transport errors have no
                // HTTP status and http_error removes the credential-bearing URL.
                return SendOutcome::Retry(PijError::Adapter {
                    adapter: "sidecars/telegram".to_string(),
                    message: detail.to_string(),
                });
            }
        }
        self.bind_reply(&chat_id, request.from, request.msg_id)
            .await
    }

    /// Upload one staged file with its method, then bind the reply target
    /// exactly as a text send does.
    ///
    /// Failure classes: a staged copy that is missing or no longer hashes to
    /// what was enqueued, a foreign chat, or a 4xx other than 429 is TERMINAL
    /// (the job fails, never "sent"); transport errors, 429 and 5xx retry the
    /// whole job. A 400 to `sendPhoto`, `sendVideo` or `sendAnimation` is not a
    /// failure of the job: the same bytes go again, once, with `sendDocument`,
    /// in the same attempt. Every refused or failed request is a durable spine
    /// event.
    async fn process_document(&self, job: &Job) -> SendOutcome {
        let request: document::DocumentJob = match serde_json::from_str(&job.payload) {
            Ok(request) => request,
            Err(error) => {
                return SendOutcome::Terminal(PijError::Adapter {
                    adapter: "sidecars/telegram".to_string(),
                    message: format!("job payload is not a Telegram document: {error}"),
                });
            }
        };
        let chat_id = request
            .chat_id
            .as_deref()
            .unwrap_or(&self.config.chat_id)
            .to_string();
        let fail =
            |method: MediaMethod, http_status: Option<u16>, terminal: bool, reason: String| {
                serde_json::json!({
                    "conversation": chat_id,
                    "from": request.from,
                    "msg_id": request.msg_id,
                    "attempt": u64::from(job.attempt) + 1,
                    "method": method.api_method(),
                    "document": {
                        "file_name": request.document.file_name,
                        "size": request.document.size,
                        "sha256": request.document.sha256,
                    },
                    "http_status": http_status,
                    "terminal": terminal,
                    "reason": reason,
                })
            };
        let mut method = request.document.method;
        if chat_id != self.config.chat_id {
            let reason = format!(
                "refusing Telegram egress to chat `{chat_id}`: configured operator chat is `{}`",
                self.config.chat_id
            );
            return self
                .document_failure(&chat_id, fail(method, None, true, reason))
                .await;
        }
        let staged = &request.document.staged;
        let bytes = match tokio::fs::read(staged).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let reason = format!("staged copy {} is unreadable: {error}", staged.display());
                return self
                    .document_failure(&chat_id, fail(method, None, true, reason))
                    .await;
            }
        };
        let sha256 = document::hex(&sha2::Sha256::digest(&bytes));
        if bytes.len() as u64 != request.document.size || sha256 != request.document.sha256 {
            let reason = format!(
                "staged copy {} changed since enqueue (sha256 {sha256}, {} bytes; enqueued {}, {} bytes)",
                staged.display(),
                bytes.len(),
                request.document.sha256,
                request.document.size
            );
            return self
                .document_failure(&chat_id, fail(method, None, true, reason))
                .await;
        }
        let sender = match self.registry.get(&request.from).await {
            Ok(sender) => sender,
            Err(error) => return SendOutcome::Retry(error),
        };
        let context = sender
            .as_ref()
            .and_then(|seat| Path::new(&seat.folder).file_name())
            .and_then(|name| name.to_str());
        let caption = match document::prefixed_caption(&request.from, context, &request.caption) {
            Ok(caption) => caption,
            Err(error) => {
                return self
                    .document_failure(&chat_id, fail(method, None, true, error.to_string()))
                    .await;
            }
        };
        let boundary = document::boundary_for(&sha256, &[&bytes, caption.as_bytes()]);
        loop {
            let (content_type, body) = document::media_multipart(
                &boundary,
                method.field(),
                &chat_id,
                &caption,
                &request.document.file_name,
                &bytes,
            );
            let url = format!(
                "{}/bot{}/{}",
                self.config.api_root,
                self.config.token,
                method.api_method()
            );
            let response = match self
                .client
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body)
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    // The URL carries the token; http_error drops it.
                    let reason = http_error(error).to_string();
                    return self
                        .document_failure(&chat_id, fail(method, None, false, reason))
                        .await;
                }
            };
            let status = response.status();
            let reply: Option<ApiReply> = response.json().await.ok();
            if status.is_success() && reply.as_ref().is_some_and(|reply| reply.ok) {
                break;
            }
            let description = reply
                .and_then(|reply| reply.description)
                .unwrap_or_else(|| "no description".to_string());
            let reason = format!(
                "{} returned HTTP {status}: {description}",
                method.api_method()
            );
            if status.as_u16() == 400 && method != MediaMethod::Document {
                // Telegram refused the media form (dimensions, encoding): the
                // same bytes still reach the human, as a document.
                let mut detail = fail(method, Some(400), false, reason);
                detail["fallback"] = serde_json::json!(MediaMethod::Document.api_method());
                self.record_document_failure(&chat_id, &mut detail).await;
                method = MediaMethod::Document;
                continue;
            }
            let retryable = status.is_server_error() || status.as_u16() == 429;
            return self
                .document_failure(
                    &chat_id,
                    fail(method, Some(status.as_u16()), !retryable, reason),
                )
                .await;
        }
        drop(bytes);
        self.record_media_delivered(&chat_id, &request, method)
            .await;
        let outcome = self
            .bind_reply(&chat_id, request.from.clone(), request.msg_id.clone())
            .await;
        if matches!(outcome, SendOutcome::Done)
            && let Err(error) = tokio::fs::remove_file(staged).await
        {
            eprintln!(
                "pij-rs telegram: sent {} but could not release staged copy {}: {error}",
                request.msg_id,
                staged.display()
            );
        }
        outcome
    }

    /// Record one failed document attempt, then answer retry or terminal.
    async fn document_failure(&self, chat_id: &str, mut detail: serde_json::Value) -> SendOutcome {
        self.record_document_failure(chat_id, &mut detail).await;
        let terminal = detail["terminal"].as_bool().unwrap_or(true);
        let error = PijError::Adapter {
            adapter: "sidecars/telegram".to_string(),
            message: detail.to_string(),
        };
        if terminal {
            SendOutcome::Terminal(error)
        } else {
            SendOutcome::Retry(error)
        }
    }

    /// Append one refused or failed upload; a persistence failure is carried
    /// in the detail rather than lost.
    async fn record_document_failure(&self, chat_id: &str, detail: &mut serde_json::Value) {
        let recorded = async {
            self.spine
                .append(Event {
                    seq: None,
                    v: 1,
                    at: now_ms()?,
                    kind: OUTBOUND_FAILED_KIND.to_string(),
                    seat: Some(conversation_seat(chat_id)),
                    payload: detail.to_string(),
                })
                .await
        }
        .await;
        if let Err(error) = recorded {
            detail["persistence_error"] = serde_json::json!(error.to_string());
        }
    }

    /// Record which method delivered a file. Telegram has accepted it by now,
    /// so a failure to record is logged, never a reason to resend.
    async fn record_media_delivered(
        &self,
        chat_id: &str,
        request: &document::DocumentJob,
        method: MediaMethod,
    ) {
        let detail = serde_json::json!({
            "conversation": chat_id,
            "from": request.from,
            "msg_id": request.msg_id,
            "method": method.api_method(),
            "document": {
                "file_name": request.document.file_name,
                "size": request.document.size,
                "sha256": request.document.sha256,
            },
        });
        let recorded = async {
            self.spine
                .append(Event {
                    seq: None,
                    v: 1,
                    at: now_ms()?,
                    kind: OUTBOUND_MEDIA_DELIVERED_KIND.to_string(),
                    seat: Some(conversation_seat(chat_id)),
                    payload: detail.to_string(),
                })
                .await
        }
        .await;
        if let Err(error) = recorded {
            eprintln!(
                "pij-rs telegram: delivered {} by {} but could not record it: {error}",
                request.msg_id,
                method.api_method()
            );
        }
    }

    /// Persist the reply binding for one delivered outbound send.
    async fn bind_reply(&self, chat_id: &str, target: SeatId, msg_id: String) -> SendOutcome {
        let binding = Event {
            seq: None,
            v: 1,
            at: match now_ms() {
                Ok(at) => at,
                Err(error) => return SendOutcome::Terminal(error),
            },
            kind: BINDING_KIND.to_string(),
            seat: Some(conversation_seat(chat_id)),
            payload: match serde_json::to_string(&Binding {
                target,
                outbound_msg_id: msg_id,
            }) {
                Ok(payload) => payload,
                Err(error) => return SendOutcome::Terminal(codec_error(error)),
            },
        };
        match self.spine.append(binding).await {
            Ok(_) => SendOutcome::Done,
            Err(error) => SendOutcome::Terminal(PijError::Adapter {
                adapter: "sidecars/telegram".to_string(),
                message: format!(
                    "Telegram accepted the send but binding persistence failed; refusing duplicate retry: {error}"
                ),
            }),
        }
    }

    async fn poll_inbound(&self) -> Result<()> {
        let offset = self.persisted_offset().await?;
        let response = self
            .client
            .get(format!(
                "{}/bot{}/getUpdates",
                self.config.api_root, self.config.token
            ))
            .query(&[("timeout", "1"), ("offset", &offset.to_string())])
            .send()
            .await
            .map_err(http_error)?;
        if !response.status().is_success() {
            return Err(PijError::Adapter {
                adapter: "sidecars/telegram".to_string(),
                message: format!("getUpdates returned HTTP {}", response.status()),
            });
        }
        let updates: TelegramResponse<Vec<Update>> = response.json().await.map_err(http_error)?;
        if !updates.ok {
            return Err(PijError::Adapter {
                adapter: "sidecars/telegram".to_string(),
                message: "getUpdates returned ok=false".to_string(),
            });
        }
        for update in updates.result {
            if let Err(error) = self.handle_update(&update).await {
                let conversation = update
                    .message
                    .as_ref()
                    .map(|message| message.chat.id.to_string());
                let _ = self.spine.append(Event {
                    seq: None,
                    v: 1,
                    at: now_ms()?,
                    kind: DELIVERY_FAILED_KIND.to_string(),
                    seat: conversation.as_deref().map(conversation_seat),
                    payload: serde_json::json!({"update_id": update.update_id, "conversation": conversation, "reason": error.to_string()}).to_string(),
                }).await;
                return Err(error);
            }
            self.record_offset(update.update_id + 1).await?;
        }
        Ok(())
    }

    async fn handle_update(&self, update: &Update) -> Result<()> {
        let Some(message) = update.message.as_ref() else {
            return Ok(());
        };
        if !self.config.allowed_user_ids.contains(&message.from.id) {
            return Ok(());
        }
        let chat = message.chat.id.to_string();
        // pij-rs does not save inbound media yet; it must not drop it silently
        // either. Media becomes a notice, delivered like text.
        let media = message.text.is_none().then(|| message.media()).flatten();
        let text = match (message.text.as_ref(), media) {
            (Some(text), _) => text.clone(),
            (None, Some((kind, file))) => media_notice(kind, file, message.caption.as_deref()),
            (None, None) => return Ok(()),
        };
        let binding = self
            .spine
            .latest_matching(&conversation_seat(&chat), &[BINDING_KIND])
            .await?;
        let Some(binding) = binding else {
            self.spine.append(Event { seq: None, v: 1, at: now_ms()?, kind: REFUSAL_KIND.to_string(), seat: Some(conversation_seat(&chat)), payload: serde_json::json!({"conversation": chat, "reason": "no persisted outbound binding"}).to_string() }).await?;
            eprintln!(
                "pij-rs telegram: refused inbound conversation {chat}: no persisted outbound binding"
            );
            return Ok(());
        };
        let binding: Binding = serde_json::from_str(&binding.payload).map_err(codec_error)?;
        let msg_id = format!("telegram-{}", update.update_id);
        enqueue_turn(
            &self.queue,
            "pij-telegram",
            &binding.target,
            text.clone(),
            msg_id.clone(),
        )
        .await?;
        self.spine
            .append(Event {
                seq: None,
                v: 1,
                at: now_ms()?,
                kind: INBOUND_ENQUEUED_KIND.to_string(),
                seat: Some(binding.target.clone()),
                payload: serde_json::json!({
                    "update_id": update.update_id,
                    "conversation": chat,
                    "msg_id": msg_id,
                    "body": text,
                })
                .to_string(),
            })
            .await?;
        if let Some((kind, file)) = media {
            self.spine
                .append(Event {
                    seq: None,
                    v: 1,
                    at: now_ms()?,
                    kind: INBOUND_MEDIA_UNSUPPORTED_KIND.to_string(),
                    seat: Some(binding.target),
                    payload: serde_json::json!({
                        "update_id": update.update_id,
                        "conversation": chat,
                        "msg_id": msg_id,
                        "kind": kind,
                        "size": file.and_then(|file| file.file_size),
                        "file_name": file.and_then(|file| file.file_name.as_deref()),
                        "mime_type": file.and_then(|file| file.mime_type.as_deref()),
                    })
                    .to_string(),
                })
                .await?;
        }
        Ok(())
    }

    async fn persisted_offset(&self) -> Result<i64> {
        let event = self
            .spine
            .latest_matching(&SeatId::from(CURSOR_SEAT), &[CURSOR_KIND])
            .await?;
        match event {
            Some(event) => Ok(serde_json::from_str::<Cursor>(&event.payload)
                .map_err(codec_error)?
                .offset),
            None => Ok(0),
        }
    }

    async fn record_offset(&self, offset: i64) -> Result<()> {
        self.spine
            .append(Event {
                seq: None,
                v: 1,
                at: now_ms()?,
                kind: CURSOR_KIND.to_string(),
                seat: Some(SeatId::from(CURSOR_SEAT)),
                payload: serde_json::to_string(&Cursor { offset }).map_err(codec_error)?,
            })
            .await?;
        Ok(())
    }
}

fn strip_exact_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = text.strip_prefix(prefix)?;
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix(' ')
    }
}

fn prefixed_text_parts(from: &SeatId, context: Option<&str>, text: &str) -> Result<Vec<String>> {
    let tag = format!("[{from}]");
    let mut prefix = match context {
        Some(context) if !context.is_empty() => format!("{tag} [{context}]"),
        _ => tag.clone(),
    };
    if prefix.encode_utf16().count() > PREFIX_LIMIT {
        prefix.truncate(tag.len());
    }
    let text = strip_exact_prefix(text, &prefix)
        .or_else(|| strip_exact_prefix(text, &tag))
        .unwrap_or(text);
    let limit = TEXT_LIMIT.checked_sub(prefix.encode_utf16().count() + 1);
    let too_long = || PijError::Adapter {
        adapter: "sidecars/telegram".to_string(),
        message: "sender prefix leaves no room for Telegram text".to_string(),
    };
    let limit = limit.ok_or_else(too_long)?;
    if text.encode_utf16().count() <= limit {
        return Ok(vec![format!("{prefix} {text}")]);
    }
    // Port of .pi/extensions/pij/telegram/chunk.ts: reserve numbering, then
    // converge on its digit width. UTF-16 units match TS string.length while
    // char_indices keeps surrogate pairs/UTF-8 characters intact.
    let mut width = "(1/1) ".len();
    let slices = loop {
        let budget = limit
            .checked_sub(width)
            .filter(|n| *n >= 2)
            .ok_or_else(too_long)?;
        let slices = split_on_boundary(text, budget);
        let next_width = format!("({0}/{0}) ", slices.len()).len();
        if next_width <= width {
            break slices;
        }
        width = next_width;
    };
    let n = slices.len();
    Ok(slices
        .into_iter()
        .enumerate()
        .map(|(i, slice)| format!("{prefix} ({}/{n}) {slice}", i + 1))
        .collect())
}

// Prefer newline, then space, in the back 40% of a window. Keep the boundary
// character in the leading slice, so concatenation recovers every input byte.
fn split_on_boundary(mut text: &str, budget: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let floor = (budget * 3 / 5).max(1);
    loop {
        let mut units = 0;
        let mut cut = text.len();
        let mut newline = None;
        let mut space = None;
        for (index, ch) in text.char_indices() {
            if units + ch.len_utf16() > budget {
                cut = index;
                break;
            }
            if units >= floor {
                match ch {
                    '\n' => newline = Some(index + 1),
                    ' ' => space = Some(index + 1),
                    _ => {}
                }
            }
            units += ch.len_utf16();
        }
        if cut == text.len() {
            parts.push(text);
            return parts;
        }
        cut = newline.or(space).unwrap_or(cut);
        parts.push(&text[..cut]);
        text = &text[cut..];
    }
}

fn conversation_seat(chat_id: &str) -> SeatId {
    SeatId::from(format!("pij-telegram-chat-{chat_id}"))
}

#[derive(Serialize, Deserialize)]
struct Binding {
    target: SeatId,
    outbound_msg_id: String,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    offset: i64,
}

enum SendOutcome {
    Done,
    Retry(PijError),
    Terminal(PijError),
}

#[derive(Deserialize)]
struct SendResponse {
    ok: bool,
}

#[derive(Deserialize)]
struct ApiReply {
    ok: bool,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: T,
}
#[derive(Deserialize)]
struct Update {
    update_id: i64,
    message: Option<TelegramMessage>,
}
#[derive(Deserialize)]
struct TelegramMessage {
    from: TelegramUser,
    chat: TelegramChat,
    text: Option<String>,
    caption: Option<String>,
    photo: Option<Vec<TelegramFile>>,
    animation: Option<TelegramFile>,
    video: Option<TelegramFile>,
    video_note: Option<TelegramFile>,
    audio: Option<TelegramFile>,
    voice: Option<TelegramFile>,
    document: Option<TelegramFile>,
    sticker: Option<TelegramFile>,
}

impl TelegramMessage {
    /// The media this message carries, if any. `animation` is checked before
    /// `document`: Telegram sends a GIF as both.
    fn media(&self) -> Option<(&'static str, Option<&TelegramFile>)> {
        if let Some(sizes) = &self.photo {
            // Ascending by resolution: the last size is the full photo.
            return Some(("photo", sizes.last()));
        }
        [
            ("animation", &self.animation),
            ("video", &self.video),
            ("video_note", &self.video_note),
            ("audio", &self.audio),
            ("voice", &self.voice),
            ("document", &self.document),
            ("sticker", &self.sticker),
        ]
        .into_iter()
        .find_map(|(kind, file)| file.as_ref().map(|file| (kind, Some(file))))
    }
}

/// The facts pij-rs reports about an inbound file it does not save.
#[derive(Deserialize)]
struct TelegramFile {
    file_size: Option<u64>,
    file_name: Option<String>,
    mime_type: Option<String>,
}

/// `[telegram media not saved by pij-rs: photo, 1.2 MB] caption: …`
fn media_notice(kind: &str, file: Option<&TelegramFile>, caption: Option<&str>) -> String {
    let mut facts = vec![kind.to_string()];
    if let Some(size) = file.and_then(|file| file.file_size) {
        facts.push(human_bytes(size));
    }
    if let Some(name) = file.and_then(|file| file.file_name.as_deref()) {
        let name: String = name.chars().filter(|ch| !ch.is_control()).collect();
        if !name.is_empty() {
            facts.push(name);
        }
    }
    let mut notice = format!("[telegram media not saved by pij-rs: {}]", facts.join(", "));
    if let Some(caption) = caption.filter(|caption| !caption.trim().is_empty()) {
        notice.push_str(" caption: ");
        notice.push_str(caption);
    }
    notice
}

fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / KB),
        _ => format!("{:.1} MB", bytes as f64 / (KB * KB)),
    }
}
#[derive(Deserialize)]
struct TelegramUser {
    id: i64,
}
#[derive(Deserialize)]
struct TelegramChat {
    id: i64,
}

fn http_error(error: reqwest::Error) -> PijError {
    // reqwest stringifies the FULL URL, and every Telegram URL carries the bot
    // token in its path (`/bot<token>/getUpdates`). That is how 313 copies of a
    // live credential reached a world-readable daemon.log (found 2026-09-11).
    // `without_url` keeps the cause and drops the URL; nothing here needs it,
    // because the adapter name already says which service failed.
    PijError::Adapter {
        adapter: "sidecars/telegram".to_string(),
        message: error.without_url().to_string(),
    }
}
fn codec_error(error: serde_json::Error) -> PijError {
    PijError::Adapter {
        adapter: "sidecars/telegram".to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
#[path = "telegram_outbound_tests.rs"]
mod outbound_tests;

#[cfg(test)]
#[path = "telegram_inbound_tests.rs"]
mod inbound_tests;

#[cfg(test)]
mod tests {
    /// A transport failure must not carry the bot token. Every Telegram URL has
    /// the credential in its path, and reqwest's Display includes the URL, so the
    /// unredacted form put a live token in a world-readable log 313 times.
    #[tokio::test]
    async fn transport_errors_never_carry_the_bot_token() {
        let token = "8954155285:NOT-A-REAL-TOKEN-abcdef";
        // A transport error, not an HTTP status. The client carries its own short
        // timeout so the test cannot depend on how the host refuses port 1: a
        // runner that DROPS instead of resetting would otherwise hang the suite.
        let error = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(250))
            .connect_timeout(std::time::Duration::from_millis(250))
            .build()
            .expect("client")
            .get(format!("http://127.0.0.1:1/bot{token}/getUpdates"))
            .send()
            .await
            .expect_err("connection to port 1 must fail");
        let mapped = http_error(error);
        let message = match &mapped {
            PijError::Adapter { adapter, message } => {
                assert_eq!(adapter, "sidecars/telegram");
                message.clone()
            }
            other => panic!("expected an adapter error, got {other:?}"),
        };
        assert!(!message.contains(token), "token leaked: {message}");
        assert!(
            !message.contains("NOT-A-REAL-TOKEN"),
            "token leaked: {message}"
        );
        assert!(!message.contains("127.0.0.1:1"), "url retained: {message}");
        // Still says something actionable about the cause.
        assert!(!message.trim().is_empty(), "diagnostic emptied");
    }

    use std::fs;
    use std::path::PathBuf;

    use super::{BridgeLock, PijError, http_error, parse_lock_holder};

    fn lock_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pij-sidecar-telegram-lock-{label}-{}-{}",
            std::process::id(),
            crate::common::now_ms().expect("clock")
        ))
    }

    #[test]
    fn both_generation_lock_shapes_refuse_live_holders_and_corrupt_is_not_free() {
        let pid = std::process::id();
        for (label, body) in [
            (
                "legacy",
                serde_json::json!({"pid": pid, "startedAt": "2026-08-31T00:00:00Z"}),
            ),
            (
                "rust-transitional",
                serde_json::json!({"pid": pid, "startedAtMs": 1}),
            ),
        ] {
            let path = lock_path(label);
            fs::write(&path, body.to_string()).expect("seed live lock");
            let error = BridgeLock::acquire(path.clone()).expect_err("live holder must refuse");
            assert!(error.to_string().contains("held by live pid"));
            assert!(path.exists(), "refusal must never delete a live lock");
            fs::remove_file(path).expect("cleanup lock");
        }

        let corrupt = lock_path("corrupt");
        fs::write(&corrupt, "{}").expect("seed corrupt lock");
        let error = BridgeLock::acquire(corrupt.clone()).expect_err("unknown holder must refuse");
        assert!(error.to_string().contains("somebody may hold"));
        assert!(corrupt.exists());
        fs::remove_file(corrupt).expect("cleanup corrupt lock");
    }

    #[test]
    fn rust_writer_is_legacy_parseable_and_parsed_dead_holder_is_reclaimed() {
        let path = lock_path("writer");
        let lock = BridgeLock::acquire(path.clone()).expect("acquire fresh lock");
        let raw = fs::read_to_string(&path).expect("read Rust lock");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse Rust lock");
        assert!(
            value["startedAt"].is_string(),
            "legacy parser requires string startedAt"
        );
        assert_eq!(parse_lock_holder(&raw), Some(std::process::id()));
        drop(lock);

        fs::write(
            &path,
            serde_json::json!({"pid": u32::MAX, "startedAt": "dead"}).to_string(),
        )
        .expect("seed dead lock");
        let reclaimed =
            BridgeLock::acquire(path.clone()).expect("parsed dead holder is reclaimable");
        assert_eq!(
            parse_lock_holder(&fs::read_to_string(&path).expect("read replacement")),
            Some(std::process::id())
        );
        drop(reclaimed);
    }
}
