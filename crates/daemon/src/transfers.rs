//! The bytes of `pij send-file` (Jordan, 2026-11-28): one folder,
//! `<state>/transfers` (0700), holding `<code>` (the file, 0600) and
//! `<code>.json` (who sent it to whom, its name, size and sha256).
//!
//! No loop, poll or timer: [`Transfers::take_expired`] runs at daemon start
//! and on each send. Disk is bounded by [`MAX_BYTES`] per pending file and the
//! 24 h expiry, and nothing is stored that would take free disk under the
//! floor. A code names a path only after [`files::is_code`]; a file's own name
//! is only ever shown.

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pij_core::files::{self, E_CORRUPT, E_DISK_FLOOR, E_SOURCE, E_TARGET, E_TOO_LARGE, MAX_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::community::Refusal;

/// The folder's name under the state dir.
pub const DIR: &str = "transfers";
const META_SUFFIX: &str = ".json";
const PART_SUFFIX: &str = ".part";
const CHUNK: usize = 64 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// One stored file waiting for its recipient.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// Its code.
    pub code: String,
    /// The sending seat.
    pub from: String,
    /// The sender's machine, when it is not this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_machine: Option<String>,
    /// The recipient seat, on this machine.
    pub to: String,
    /// The file's name, as the sender's path ended.
    pub name: String,
    /// Bytes.
    pub size: u64,
    /// Lowercase hex sha256 of the bytes.
    pub sha256: String,
    /// The sender's note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When this daemon stored it, ms since the epoch.
    pub created_at: u64,
}

impl Pending {
    /// The sender as a reply addresses it: `seat` or `seat@machine`.
    pub fn sender(&self) -> String {
        match &self.from_machine {
            Some(machine) => format!("{}@{machine}", self.from),
            None => self.from.clone(),
        }
    }
}

/// The transfer folder, with the codes some request is working on.
pub struct Transfers {
    root: PathBuf,
    floor_bytes: u64,
    busy: Arc<Mutex<HashSet<String>>>,
}

/// A code held for one request: no second accept, decline or expiry of it
/// runs until this drops.
pub struct Held {
    code: String,
    busy: Arc<Mutex<HashSet<String>>>,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.busy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.code);
    }
}

/// A file being written into the folder. Dropped unfinished, its partial file
/// is removed.
pub struct Incoming {
    part: PathBuf,
    data: PathBuf,
    file: Option<tokio::fs::File>,
    hasher: Sha256,
    size: u64,
    limit: u64,
}

impl Drop for Incoming {
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

impl Incoming {
    /// Append one chunk, refusing past the limit.
    ///
    /// # Errors
    /// More bytes than the limit, or a write failure.
    pub async fn write(&mut self, chunk: &[u8]) -> Result<(), Refusal> {
        self.size += chunk.len() as u64;
        if self.size > self.limit {
            return Err(too_large(self.size, self.limit));
        }
        self.hasher.update(chunk);
        let file = self.file.as_mut().expect("an unfinished incoming file");
        file.write_all(chunk).await.map_err(io_refusal)
    }

    /// Flush the file to disk and name it `<code>`: its size and sha256.
    ///
    /// # Errors
    /// A flush or rename failure.
    pub async fn finish(mut self) -> Result<(u64, String), Refusal> {
        let mut file = self.file.take().expect("an unfinished incoming file");
        let done = async {
            file.flush().await?;
            file.sync_all().await?;
            tokio::fs::rename(&self.part, &self.data).await
        }
        .await;
        if let Err(error) = done {
            let _ = std::fs::remove_file(&self.part);
            return Err(io_refusal(error));
        }
        let sha = files::hex(&std::mem::take(&mut self.hasher).finalize());
        Ok((self.size, sha))
    }
}

impl Transfers {
    /// The folder at `root`, refusing a store that would leave less than
    /// `floor_bytes` free.
    pub fn new(root: PathBuf, floor_bytes: u64) -> Self {
        Self {
            root,
            floor_bytes,
            busy: Arc::default(),
        }
    }

    /// The folder at `root` with the floor from `PIJ_DISK_FLOOR_GB` (GiB,
    /// default 25), read once, here.
    pub fn from_env(root: PathBuf) -> Self {
        let gib = std::env::var("PIJ_DISK_FLOOR_GB")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(files::DEFAULT_DISK_FLOOR_GIB);
        Self::new(root, gib.saturating_mul(GIB))
    }

    /// The folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn data_path(&self, code: &str) -> PathBuf {
        debug_assert!(files::is_code(code));
        self.root.join(code)
    }

    fn meta_path(&self, code: &str) -> PathBuf {
        self.root.join(format!("{code}{META_SUFFIX}"))
    }

    /// Create the folder (0700) if needed, and tighten it if it was looser.
    ///
    /// # Errors
    /// The folder cannot be made or is not a directory.
    pub fn prepare(&self) -> Result<(), Refusal> {
        std::fs::create_dir_all(&self.root).map_err(io_refusal)?;
        let meta = std::fs::symlink_metadata(&self.root).map_err(io_refusal)?;
        if !meta.is_dir() {
            return Err(Refusal::new(
                E_SOURCE,
                format!("{} is not a directory", self.root.display()),
            ));
        }
        if meta.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))
                .map_err(io_refusal)?;
        }
        Ok(())
    }

    /// A fresh code from OS entropy.
    ///
    /// # Errors
    /// The OS gave no randomness.
    pub fn mint_code() -> Result<String, Refusal> {
        let mut bytes = [0_u8; files::CODE_BYTES];
        getrandom::fill(&mut bytes).map_err(|error| {
            Refusal::new(E_SOURCE, format!("no randomness for a code: {error}"))
        })?;
        Ok(files::hex(&bytes))
    }

    /// Hold `code` for one request, or `None` when another request has it.
    pub fn hold(&self, code: &str) -> Option<Held> {
        let mut busy = self
            .busy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        busy.insert(code.to_string()).then(|| Held {
            code: code.to_string(),
            busy: Arc::clone(&self.busy),
        })
    }

    /// Refuse a store of `size` bytes that would leave free disk under the floor.
    ///
    /// A brake: it can only refuse a store, never cause one.
    ///
    /// # Errors
    /// Under the floor, or the free space cannot be read.
    pub fn check_floor(&self, size: u64) -> Result<(), Refusal> {
        let stat = nix::sys::statvfs::statvfs(&self.root).map_err(|error| {
            Refusal::new(E_DISK_FLOOR, format!("free disk unreadable: {error}"))
        })?;
        #[allow(clippy::useless_conversion)]
        let free =
            u64::from(stat.blocks_available()).saturating_mul(u64::from(stat.fragment_size()));
        if free.saturating_sub(size) < self.floor_bytes {
            return Err(Refusal::new(
                E_DISK_FLOOR,
                format!(
                    "storing {} would leave {} free on {}, under the floor of {} (PIJ_DISK_FLOOR_GB)",
                    files::human_size(size),
                    files::human_size(free.saturating_sub(size)),
                    self.root.display(),
                    files::human_size(self.floor_bytes)
                ),
            ));
        }
        Ok(())
    }

    /// Start writing `code`'s bytes, refusing more than `limit`.
    ///
    /// # Errors
    /// A bad code, or the partial file cannot be created.
    pub async fn incoming(&self, code: &str, limit: u64) -> Result<Incoming, Refusal> {
        if !files::is_code(code) {
            return Err(Refusal::new(E_SOURCE, "not a transfer code"));
        }
        let part = self.root.join(format!("{code}{PART_SUFFIX}"));
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(&part)
            .await
            .map_err(io_refusal)?;
        Ok(Incoming {
            part,
            data: self.data_path(code),
            file: Some(file),
            hasher: Sha256::new(),
            size: 0,
            limit: limit.min(MAX_BYTES),
        })
    }

    /// Copy the regular file at `source` in as `code`: its size and sha256.
    ///
    /// # Errors
    /// Not a readable regular file, over [`MAX_BYTES`], or a write failure.
    pub async fn ingest_path(&self, code: &str, source: &Path) -> Result<(u64, String), Refusal> {
        let mut file = tokio::fs::File::open(source).await.map_err(|error| {
            Refusal::new(
                E_SOURCE,
                format!("cannot read {}: {error}", source.display()),
            )
        })?;
        let meta = file.metadata().await.map_err(io_refusal)?;
        if !meta.is_file() {
            return Err(Refusal::new(
                E_SOURCE,
                format!("{} is not a regular file", source.display()),
            ));
        }
        if meta.len() > MAX_BYTES {
            return Err(too_large(meta.len(), MAX_BYTES));
        }
        self.check_floor(meta.len())?;
        let mut incoming = self.incoming(code, MAX_BYTES).await?;
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            let read = file.read(&mut buffer).await.map_err(io_refusal)?;
            if read == 0 {
                break;
            }
            incoming.write(&buffer[..read]).await?;
        }
        incoming.finish().await
    }

    /// Write `pending`'s record, atomically and 0600.
    ///
    /// # Errors
    /// A write failure.
    pub fn record(&self, pending: &Pending) -> Result<(), Refusal> {
        let text = serde_json::to_vec(pending).map_err(|error| {
            Refusal::new(E_SOURCE, format!("cannot encode the record: {error}"))
        })?;
        let staged = self
            .root
            .join(format!("{}{META_SUFFIX}{PART_SUFFIX}", pending.code));
        let written = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
                .open(&staged)?;
            file.write_all(&text)?;
            file.sync_all()?;
            std::fs::rename(&staged, self.meta_path(&pending.code))
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_file(&staged);
            return Err(io_refusal(error));
        }
        Ok(())
    }

    /// The pending file with this code, if any.
    pub fn get(&self, code: &str) -> Option<Pending> {
        if !files::is_code(code) {
            return None;
        }
        let text = std::fs::read(self.meta_path(code)).ok()?;
        serde_json::from_slice::<Pending>(&text)
            .ok()
            .filter(|pending| pending.code == code)
    }

    /// Every pending file, oldest first.
    pub fn list(&self) -> Vec<Pending> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut pending: Vec<Pending> = entries
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                self.get(name.strip_suffix(META_SUFFIX)?)
            })
            .collect();
        pending.sort_by_key(|pending| pending.created_at);
        pending
    }

    /// Delete `code`'s bytes, then its record.
    pub fn remove(&self, code: &str) {
        if files::is_code(code) {
            let _ = std::fs::remove_file(self.data_path(code));
            let _ = std::fs::remove_file(self.meta_path(code));
        }
    }

    /// Delete every pending file older than 24 h that no request holds, and
    /// return them, so their sender and recipient can be told. A partial or
    /// orphaned file last written more than 24 h ago goes too.
    pub fn take_expired(&self, now_ms: u64) -> Vec<Pending> {
        let mut expired = Vec::new();
        for pending in self.list() {
            if pending.created_at.saturating_add(files::EXPIRY_MS) > now_ms {
                continue;
            }
            let Some(_held) = self.hold(&pending.code) else {
                continue;
            };
            self.remove(&pending.code);
            expired.push(pending);
        }
        self.sweep_orphans(now_ms);
        expired
    }

    /// A partial file, or bytes with no record, both left by a crash.
    /// Age only spares one: a file written in the last 24 h is kept.
    fn sweep_orphans(&self, now_ms: u64) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let code = name
                .strip_suffix(PART_SUFFIX)
                .map(|stem| stem.strip_suffix(META_SUFFIX).unwrap_or(stem))
                .unwrap_or(&name);
            let orphan = if name.ends_with(PART_SUFFIX) {
                files::is_code(code)
            } else {
                files::is_code(&name) && !self.meta_path(&name).exists()
            };
            if !orphan || self.busy.lock().is_ok_and(|busy| busy.contains(code)) {
                continue;
            }
            let old = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|at| {
                    (at.as_millis() as u64).saturating_add(files::EXPIRY_MS) <= now_ms
                });
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Write `pending`'s bytes to `target` and check them against its sha256.
    /// Without `force` an existing `target` (a symlink included) is refused;
    /// with it, `target` is replaced, a symlink by the file, never followed.
    /// The stored copy is left for the caller to remove.
    ///
    /// # Errors
    /// The target exists, is a directory, its folder is missing, or the bytes
    /// do not match.
    pub async fn save(&self, pending: &Pending, target: &Path, force: bool) -> Result<(), Refusal> {
        let data = self.data_path(&pending.code);
        let (target, sha, code) = (
            target.to_path_buf(),
            pending.sha256.clone(),
            pending.code.clone(),
        );
        tokio::task::spawn_blocking(move || save_blocking(&data, &target, &sha, &code, force))
            .await
            .map_err(|error| Refusal::new(E_TARGET, format!("the save task failed: {error}")))?
    }
}

fn save_blocking(
    data: &Path,
    target: &Path,
    sha: &str,
    code: &str,
    force: bool,
) -> Result<(), Refusal> {
    let Some(file_name) = target.file_name() else {
        return Err(Refusal::new(
            E_TARGET,
            format!("{} names no file", target.display()),
        ));
    };
    let folder = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let folder = folder.unwrap_or_else(|| Path::new("."));
    if !folder.is_dir() {
        return Err(Refusal::new(
            E_TARGET,
            format!("{} is not a folder", folder.display()),
        ));
    }
    match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.is_dir() => {
            return Err(Refusal::new(
                E_TARGET,
                format!("{} is a folder; name the file to write", target.display()),
            ));
        }
        Ok(_) if !force => {
            return Err(Refusal::new(
                E_TARGET,
                format!("{} exists; add --force to replace it", target.display()),
            ));
        }
        _ => {}
    }
    let mut source = std::fs::File::open(data).map_err(io_refusal)?;
    let staged = folder.join(format!(
        ".{}.pij-{code}{PART_SUFFIX}",
        file_name.to_string_lossy()
    ));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(&staged)
        .map_err(|error| {
            Refusal::new(
                E_TARGET,
                format!("cannot write in {}: {error}", folder.display()),
            )
        })?;
    let copied = (|| {
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
        }
        out.sync_all()?;
        Ok::<_, std::io::Error>(files::hex(&hasher.finalize()))
    })();
    let finished = match copied {
        Err(error) => Err(io_refusal(error)),
        Ok(got) if got != sha => Err(Refusal::new(
            E_CORRUPT,
            format!(
                "the stored bytes hash to {got}, not the {sha} they were sent with; nothing was saved"
            ),
        )),
        // Without --force a hard link refuses an existing target, a symlink
        // included, and never follows one; rename replaces the link itself.
        Ok(_) if force => std::fs::rename(&staged, target).map_err(target_refusal(target)),
        Ok(_) => std::fs::hard_link(&staged, target).map_err(target_refusal(target)),
    };
    let _ = std::fs::remove_file(&staged);
    finished
}

fn target_refusal(target: &Path) -> impl Fn(std::io::Error) -> Refusal + '_ {
    move |error| {
        let message = if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!("{} exists; add --force to replace it", target.display())
        } else {
            format!("cannot write {}: {error}", target.display())
        };
        Refusal::new(E_TARGET, message)
    }
}

fn too_large(size: u64, limit: u64) -> Refusal {
    Refusal::new(
        E_TOO_LARGE,
        format!(
            "the file is {} ({size} bytes) or more; the limit is {} ({limit} bytes)",
            files::human_size(size),
            files::human_size(limit)
        ),
    )
}

fn io_refusal(error: std::io::Error) -> Refusal {
    Refusal::new(E_SOURCE, error.to_string())
}

#[cfg(test)]
mod tests;
