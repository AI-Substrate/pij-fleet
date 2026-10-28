//! Telegram files: staged by content at enqueue, uploaded by type.
//!
//! The job never carries the caller's path as the thing to send. Enqueue copies
//! each file into the sidecar's staging directory while hashing it, and the job
//! records the staged copy plus its SHA-256, size and Bot API method. The worker
//! re-verifies the bytes before uploading, so an edit to the source after
//! `pij send` returns cannot change what reaches Telegram, and a damaged staged
//! copy fails the job rather than sending something else.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use pij_core::error::{PijError, Result};
use pij_core::model::{Job, SeatId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{TELEGRAM_DOCUMENT_KIND, TelegramSend, send_job, strip_exact_prefix};

/// Telegram's ceiling for a bot uploading a file (Bot API, every method).
pub const DOCUMENT_LIMIT_BYTES: u64 = 50 * 1024 * 1024;
/// Telegram's ceiling for `sendPhoto`; a larger image goes as a document.
pub const PHOTO_LIMIT_BYTES: u64 = 10 * 1024 * 1024;
/// The most files one send may carry.
pub const MAX_FILES: usize = 10;
/// Telegram's caption ceiling, in characters after entity parsing.
pub const CAPTION_LIMIT: usize = 1024;

/// The Bot API method a staged file is uploaded with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaMethod {
    /// `sendPhoto`: jpg, jpeg, png or webp up to [`PHOTO_LIMIT_BYTES`].
    #[serde(rename = "sendPhoto")]
    Photo,
    /// `sendVideo`: mp4.
    #[serde(rename = "sendVideo")]
    Video,
    /// `sendAnimation`: gif.
    #[serde(rename = "sendAnimation")]
    Animation,
    /// `sendDocument`: everything else, and the fallback for the others.
    #[default]
    #[serde(rename = "sendDocument")]
    Document,
}

impl MediaMethod {
    /// Choose the method from a file's name and size.
    pub fn classify(file_name: &str, size: u64) -> Self {
        // A leading-dot-only name (".png") has no extension.
        let extension = match file_name.rfind('.') {
            Some(dot) if dot > 0 => file_name[dot + 1..].to_ascii_lowercase(),
            _ => String::new(),
        };
        match extension.as_str() {
            "jpg" | "jpeg" | "png" | "webp" if size <= PHOTO_LIMIT_BYTES => Self::Photo,
            "mp4" => Self::Video,
            "gif" => Self::Animation,
            _ => Self::Document,
        }
    }

    /// The Bot API method name, as it appears in the URL.
    pub fn api_method(self) -> &'static str {
        match self {
            Self::Photo => "sendPhoto",
            Self::Video => "sendVideo",
            Self::Animation => "sendAnimation",
            Self::Document => "sendDocument",
        }
    }

    /// The multipart field that carries the file for this method.
    pub fn field(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Animation => "animation",
            Self::Document => "document",
        }
    }
}

/// Durable request submitted by the CLI or the `pij send --file` shim route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramDocumentRequest {
    /// Seat whose later reply binding this send establishes.
    pub from: SeatId,
    /// Absolute paths of the files to send, in order; copied at enqueue.
    pub files: Vec<PathBuf>,
    /// Optional caption for the first file; the sender tag is always prefixed.
    #[serde(default)]
    pub caption: Option<String>,
    /// Optional text, sent before the files as an ordinary text send.
    #[serde(default)]
    pub body: Option<String>,
    /// Stable request id.
    pub msg_id: String,
    /// Conversation override; absent uses the credential file's chat id.
    #[serde(default)]
    pub chat_id: Option<String>,
}

/// The content-addressed copy a file job sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedDocument {
    /// The caller's path, for diagnostics only. Never read after enqueue.
    pub source: PathBuf,
    /// The staged copy the worker uploads.
    pub staged: PathBuf,
    /// Name shown in Telegram.
    pub file_name: String,
    /// Bytes staged.
    pub size: u64,
    /// Lowercase hex SHA-256 of the staged bytes.
    pub sha256: String,
    /// The Bot API method the worker tries first.
    #[serde(default)]
    pub method: MediaMethod,
}

/// Queue payload for [`TELEGRAM_DOCUMENT_KIND`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DocumentJob {
    pub from: SeatId,
    pub msg_id: String,
    pub chat_id: Option<String>,
    pub caption: String,
    pub document: StagedDocument,
}

/// Everything one request queues, in order: the body (if any), then each file.
#[derive(Debug)]
pub struct StagedSend {
    /// The text send for `body`, queued first.
    pub body: Option<Job>,
    /// One job per file, in argument order, with the copy it sends.
    pub files: Vec<(Job, StagedDocument)>,
}

/// Validate and stage every file, and build the queue rows for one send.
///
/// Every refusal happens here, before anything is queued: no files or too
/// many; a missing, empty, non-regular or oversized file; a caption that
/// cannot fit even behind the bare sender tag.
///
/// A refusal releases what this request already staged, except any copy a
/// live job sends (`live`): an identical file under the same id stages to the
/// path a live row still reads.
pub fn stage_send(
    request: &TelegramDocumentRequest,
    staging_dir: &Path,
    live: &LiveCopies,
) -> Result<StagedSend> {
    stage_with_limit(request, staging_dir, DOCUMENT_LIMIT_BYTES, live)
}

/// The staged copies live jobs still send, by file name: a staged name
/// (`<sha256>-<msg-key>`) is unique within the one staging directory, and a
/// name never depends on how a caller spelled that directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveCopies(std::collections::HashSet<std::ffi::OsString>);

impl LiveCopies {
    /// Read every pending or running Telegram file job: one queue read.
    pub async fn read(queue: &dyn pij_core::ports::Queue) -> Result<Self> {
        let payloads = queue
            .live_payloads(&[super::TELEGRAM_DOCUMENT_KIND.to_string()])
            .await?;
        Ok(Self::from_payloads(&payloads))
    }

    /// The copies these job payloads send. A payload that does not decode
    /// names nothing; it cannot be a file job pij wrote.
    pub fn from_payloads(payloads: &[String]) -> Self {
        Self(
            payloads
                .iter()
                .filter_map(|payload| serde_json::from_str::<DocumentJob>(payload).ok())
                .filter_map(|job| job.document.staged.file_name().map(ToOwned::to_owned))
                .collect(),
        )
    }

    /// Does a live job send this copy?
    pub fn sends(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| self.0.contains(name))
    }
}

/// Remove staged copies (and abandoned `.partial-*` writes) whose mtime is
/// older than `retention` at `now`; returns how many went.
///
/// Age is a RETENTION POLICY: it is the reason to delete. Delivered copies
/// are released at once, so what remains are failed jobs' evidence (N2).
/// `live` is a one-directional BRAKE on that policy: a copy a pending or
/// running job sends is never removed, however old (say a daemon down for
/// longer than the retention with file jobs queued). Removing the brake
/// deletes the same set or more. In-process: one directory read, nothing
/// launched.
pub fn sweep_staged(
    staging_dir: &Path,
    now: std::time::SystemTime,
    retention: std::time::Duration,
    live: &LiveCopies,
) -> Result<usize> {
    let entries = match fs::read_dir(staging_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(io_failure("list", staging_dir, &error)),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|error| io_failure("list", staging_dir, &error))?;
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let expired = metadata.is_file()
            && !live.sends(&entry.path())
            && metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > retention);
        if expired && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Delete staged copies that will never be queued, except any a live job
/// sends. The ONE release seam (plan 183 review B1): every path that gives a
/// copy back comes through here, so none can forget the brake.
pub fn release_staged<'a>(
    documents: impl IntoIterator<Item = &'a StagedDocument>,
    live: &LiveCopies,
) {
    for document in documents {
        if !live.sends(&document.staged) {
            let _ = fs::remove_file(&document.staged);
        }
    }
}

pub(super) fn stage_with_limit(
    request: &TelegramDocumentRequest,
    staging_dir: &Path,
    limit: u64,
    live: &LiveCopies,
) -> Result<StagedSend> {
    if request.files.is_empty() {
        return Err(refusal(
            "a Telegram file send needs at least one --file; text alone is a plain send"
                .to_string(),
        ));
    }
    if request.files.len() > MAX_FILES {
        return Err(refusal(format!(
            "{} files in one send; send at most {MAX_FILES}",
            request.files.len()
        )));
    }
    if request.msg_id.trim().is_empty() {
        return Err(refusal("a Telegram file send needs a msg_id".to_string()));
    }
    let caption = request.caption.clone().unwrap_or_default();
    // Context is unknown until send time, so prove the bare-tag fallback fits.
    prefixed_caption(&request.from, None, &caption)?;
    let body = request
        .body
        .as_ref()
        .map(|body| {
            send_job(&TelegramSend {
                from: request.from.clone(),
                body: body.clone(),
                msg_id: request.msg_id.clone(),
                chat_id: request.chat_id.clone(),
            })
        })
        .transpose()?;
    // A lone file keeps the request's id; otherwise each file is numbered, so
    // every job has its own dedupe key and its own outcome.
    let numbered = request.files.len() > 1 || body.is_some();
    let mut files: Vec<(Job, StagedDocument)> = Vec::with_capacity(request.files.len());
    for (index, source) in request.files.iter().enumerate() {
        let msg_id = if numbered {
            format!("{}#{}", request.msg_id, index + 1)
        } else {
            request.msg_id.clone()
        };
        let caption = if index == 0 {
            caption.clone()
        } else {
            String::new()
        };
        match stage_one(request, source, &msg_id, caption, staging_dir, limit) {
            Ok(staged) => files.push(staged),
            Err(error) => {
                // All or nothing: nothing is queued, so nothing stays staged.
                release_staged(files.iter().map(|(_, document)| document), live);
                return Err(error);
            }
        }
    }
    Ok(StagedSend { body, files })
}

fn stage_one(
    request: &TelegramDocumentRequest,
    source: &Path,
    msg_id: &str,
    caption: String,
    staging_dir: &Path,
    limit: u64,
) -> Result<(Job, StagedDocument)> {
    if !source.is_absolute() {
        return Err(refusal(format!(
            "attachment path `{}` is relative; the daemon has no working directory of yours to \
             resolve it against, so pass an absolute path",
            source.display()
        )));
    }
    let metadata = fs::metadata(source).map_err(|error| {
        refusal(format!(
            "attachment `{}` cannot be read: {error}",
            source.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(refusal(format!(
            "attachment `{}` is not a regular file",
            source.display()
        )));
    }
    if metadata.len() == 0 {
        return Err(refusal(format!(
            "attachment `{}` is empty; Telegram refuses empty files",
            source.display()
        )));
    }
    if metadata.len() > limit {
        return Err(too_large(source, metadata.len(), limit));
    }

    fs::create_dir_all(staging_dir).map_err(|error| io_failure("create", staging_dir, &error))?;
    fs::set_permissions(staging_dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_failure("restrict", staging_dir, &error))?;
    let msg_key = hex(&Sha256::digest(msg_id))[..16].to_string();
    // The msg key keeps two files of one request (same pid, same millisecond)
    // from opening the same partial name.
    let partial = staging_dir.join(format!(
        ".partial-{}-{}-{msg_key}",
        std::process::id(),
        crate::common::now_ms()?
    ));
    let staged = copy_hashed(source, &partial, limit);
    let (size, sha256) = match staged {
        Ok(staged) => staged,
        Err(error) => {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
    };
    let staged = staging_dir.join(format!("{sha256}-{msg_key}"));
    if let Err(error) = fs::rename(&partial, &staged) {
        let _ = fs::remove_file(&partial);
        return Err(io_failure("publish", &staged, &error));
    }
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "document".to_string());
    let document = StagedDocument {
        source: source.to_path_buf(),
        staged,
        method: MediaMethod::classify(&file_name, size),
        file_name,
        size,
        sha256,
    };
    let payload = DocumentJob {
        from: request.from.clone(),
        msg_id: msg_id.to_string(),
        chat_id: request.chat_id.clone(),
        caption,
        document: document.clone(),
    };
    let job = Job {
        kind: TELEGRAM_DOCUMENT_KIND.to_string(),
        serial_key: request
            .chat_id
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        payload: serde_json::to_string(&payload).map_err(|error| {
            refusal(format!(
                "could not encode Telegram document request: {error}"
            ))
        })?,
        dedupe_key: msg_id.to_string(),
        dedupe_origin: None,
        attempt: 0,
    };
    Ok((job, document))
}

/// Copy at most `limit` bytes, hashing as we go; a file that grew past the
/// limit since `metadata` is refused rather than truncated.
fn copy_hashed(source: &Path, partial: &Path, limit: u64) -> Result<(u64, String)> {
    let reader = fs::File::open(source).map_err(|error| {
        refusal(format!(
            "attachment `{}` cannot be read: {error}",
            source.display()
        ))
    })?;
    let mut reader = reader.take(limit + 1);
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(partial)
        .map_err(|error| io_failure("stage", partial, &error))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            refusal(format!(
                "attachment `{}` could not be read while staging: {error}",
                source.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > limit {
            return Err(too_large(source, size, limit));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| io_failure("stage", partial, &error))?;
    }
    writer
        .sync_all()
        .map_err(|error| io_failure("sync", partial, &error))?;
    Ok((size, hex(&hasher.finalize())))
}

/// Prefix the sender tag (and repository context when it fits) onto a caption.
pub(super) fn prefixed_caption(
    from: &SeatId,
    context: Option<&str>,
    caption: &str,
) -> Result<String> {
    let tag = format!("[{from}]");
    let full = match context {
        Some(context) if !context.is_empty() => format!("{tag} [{context}]"),
        _ => tag.clone(),
    };
    let caption = strip_exact_prefix(caption, &full)
        .or_else(|| strip_exact_prefix(caption, &tag))
        .unwrap_or(caption);
    for prefix in [&full, &tag] {
        let text = if caption.is_empty() {
            prefix.clone()
        } else {
            format!("{prefix} {caption}")
        };
        if text.encode_utf16().count() <= CAPTION_LIMIT {
            return Ok(text);
        }
    }
    Err(refusal(format!(
        "caption is {} characters; behind the sender tag `{tag}` Telegram allows {} at most",
        caption.encode_utf16().count(),
        CAPTION_LIMIT.saturating_sub(tag.encode_utf16().count() + 1)
    )))
}

/// A `multipart/form-data` body for one file upload, and its content type.
///
/// Hand-built rather than through reqwest's `multipart` feature: the request is
/// three fixed fields, and building the bytes as a pure function is what lets
/// the wire shape be asserted byte for byte. `field` names the file part for
/// the method (`photo`, `video`, `animation` or `document`).
pub(super) fn media_multipart(
    boundary: &str,
    field: &str,
    chat_id: &str,
    caption: &str,
    file_name: &str,
    bytes: &[u8],
) -> (String, Vec<u8>) {
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (name, value) in [("chat_id", chat_id), ("caption", caption)] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; \
             filename=\"{}\"\r\nContent-Type: application/octet-stream\r\n\r\n",
            escape_file_name(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// A boundary that occurs in none of the parts.
pub(super) fn boundary_for(sha256: &str, parts: &[&[u8]]) -> String {
    let mut counter = 0u32;
    loop {
        let boundary = format!("pij-{sha256}-{counter}");
        let needle = boundary.as_bytes();
        if !parts
            .iter()
            .any(|part| part.windows(needle.len()).any(|window| window == needle))
        {
            return boundary;
        }
        counter += 1;
    }
}

/// Quoted-string safe: browsers percent-encode exactly these in a filename.
fn escape_file_name(name: &str) -> String {
    name.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn too_large(source: &Path, size: u64, limit: u64) -> PijError {
    refusal(format!(
        "attachment `{}` is {size} bytes; Telegram bots may upload at most {limit} bytes \
         (50 MB)",
        source.display()
    ))
}

fn io_failure(verb: &str, path: &Path, error: &std::io::Error) -> PijError {
    refusal(format!(
        "could not {verb} Telegram attachment staging at {}: {error}",
        path.display()
    ))
}

fn refusal(message: String) -> PijError {
    PijError::Adapter {
        adapter: "sidecars/telegram".to_string(),
        message,
    }
}

#[cfg(test)]
#[path = "telegram_document_tests.rs"]
mod tests;
