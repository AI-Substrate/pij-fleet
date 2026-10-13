//! Telegram documents: staged by content at enqueue, uploaded with `sendDocument`.
//!
//! The job never carries the caller's path as the thing to send. Enqueue copies
//! the file into the sidecar's staging directory while hashing it, and the job
//! records the staged copy plus its SHA-256 and size. The worker re-verifies
//! both before uploading, so an edit to the source after `pij send` returns can
//! not change what reaches Telegram, and a damaged staged copy fails the job
//! rather than sending something else.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use pij_core::error::{PijError, Result};
use pij_core::model::{Job, SeatId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{TELEGRAM_DOCUMENT_KIND, strip_exact_prefix};

/// Telegram's ceiling for a bot uploading a document (Bot API `sendDocument`).
pub const DOCUMENT_LIMIT_BYTES: u64 = 50 * 1024 * 1024;
/// Telegram's caption ceiling, in characters after entity parsing.
pub const CAPTION_LIMIT: usize = 1024;

/// Durable request submitted by the CLI or the `pij send --file` shim route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramDocumentRequest {
    /// Seat whose later reply binding this send establishes.
    pub from: SeatId,
    /// Absolute path of the file to send; copied at enqueue.
    pub path: PathBuf,
    /// Optional caption; the sender tag is always prefixed.
    #[serde(default)]
    pub caption: Option<String>,
    /// Stable request id.
    pub msg_id: String,
    /// Conversation override; absent uses the credential file's chat id.
    #[serde(default)]
    pub chat_id: Option<String>,
}

/// The content-addressed copy a document job sends.
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

/// Validate, stage, and build the queue row for one document send.
///
/// Every refusal happens here, before anything is queued: a missing, empty,
/// non-regular, or oversized file, and a caption that cannot fit even behind
/// the bare sender tag.
pub fn stage_document(
    request: &TelegramDocumentRequest,
    staging_dir: &Path,
) -> Result<(Job, StagedDocument)> {
    stage_with_limit(request, staging_dir, DOCUMENT_LIMIT_BYTES)
}

pub(super) fn stage_with_limit(
    request: &TelegramDocumentRequest,
    staging_dir: &Path,
    limit: u64,
) -> Result<(Job, StagedDocument)> {
    let source = &request.path;
    if !source.is_absolute() {
        return Err(refusal(format!(
            "attachment path `{}` is relative; the daemon has no working directory of yours to \
             resolve it against, so pass an absolute path",
            source.display()
        )));
    }
    if request.msg_id.trim().is_empty() {
        return Err(refusal("a Telegram document needs a msg_id".to_string()));
    }
    let caption = request.caption.clone().unwrap_or_default();
    // Context is unknown until send time, so prove the bare-tag fallback fits.
    prefixed_caption(&request.from, None, &caption)?;
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
            "attachment `{}` is empty; Telegram refuses empty documents",
            source.display()
        )));
    }
    if metadata.len() > limit {
        return Err(too_large(source, metadata.len(), limit));
    }

    fs::create_dir_all(staging_dir).map_err(|error| io_failure("create", staging_dir, &error))?;
    fs::set_permissions(staging_dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_failure("restrict", staging_dir, &error))?;
    let partial = staging_dir.join(format!(
        ".partial-{}-{}",
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
    let name = format!("{sha256}-{}", &hex(&Sha256::digest(&request.msg_id))[..16]);
    let staged = staging_dir.join(name);
    if let Err(error) = fs::rename(&partial, &staged) {
        let _ = fs::remove_file(&partial);
        return Err(io_failure("publish", &staged, &error));
    }
    let document = StagedDocument {
        source: source.clone(),
        staged,
        file_name: source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "document".to_string()),
        size,
        sha256,
    };
    let payload = DocumentJob {
        from: request.from.clone(),
        msg_id: request.msg_id.clone(),
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
        dedupe_key: request.msg_id.clone(),
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

/// A `multipart/form-data` body for `sendDocument`, and its content type.
///
/// Hand-built rather than through reqwest's `multipart` feature: the request is
/// three fixed fields, and building the bytes as a pure function is what lets
/// the wire shape be asserted byte for byte.
pub(super) fn document_multipart(
    boundary: &str,
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
            "--{boundary}\r\nContent-Disposition: form-data; name=\"document\"; \
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
