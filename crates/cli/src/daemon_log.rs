//! Timestamp daemon output once, below Rust's stdout/stderr and panic hooks.
//!
//! The original destinations remain distinct (or share daemon.log when launched
//! by bounce). Only the daemon installs this guard; normal CLI output is untouched.
//!
//! #90: two bounds on what reaches the file.
//! - **Repeats.** A line the same output already wrote, exactly (its line
//!   ending aside), in the last [`REPEAT_WINDOW`] is counted, not written. When
//!   its window closes, one summary line gives the count. A panic's lines and
//!   backtrace are never counted. At most [`REPEAT_KEYS`] lines are remembered,
//!   each in at most [`TEMPLATE_SHOWN`] bytes plus an ellipsis.
//! - **Size.** When the destination is the state dir's own `daemon.log` (the
//!   same file by device and inode, never a symlink), the daemon rotates it
//!   itself once it reaches [`ROTATE_AT`]: `daemon.log.1` … `daemon.log.{KEEP}`,
//!   the oldest dropped. Every launcher (`just bounce-rs`'s `>>`, the
//!   LaunchAgent's `StandardOutPath`, `pij-rs daemon bounce`) only opens the file
//!   once before exec, so after a rotation the daemon writes to a file it opened
//!   by path. A file more than a sixteenth over the cap (a log from before #90)
//!   keeps only its last [`ROTATE_AT`] bytes, from a line start. Every look at
//!   the path, at start and at each cap, holds `daemon.log.lock` (`flock`), so
//!   two daemons started together never rotate at once and each joins the file
//!   the other published. Any other destination (a terminal, another file) is
//!   never rotated. All of it is in-process: no launch.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::Shutdown;
use std::os::fd::AsFd;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg, OFlag};
use nix::unistd::{dup2_stderr, dup2_stdout};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// A template already written within this long is counted, not written.
pub(crate) const REPEAT_WINDOW: Duration = Duration::from_secs(60);
/// Templates remembered at once. A new one past this first forgets the
/// expired, then the one whose window ends soonest (its count is written).
pub(crate) const REPEAT_KEYS: usize = 256;
/// A summary shows at most this many bytes of the template.
const TEMPLATE_SHOWN: usize = 240;
/// The managed `daemon.log` rotates when it reaches this size.
pub(crate) const ROTATE_AT: u64 = 16 * 1024 * 1024;
/// Rotated files kept: `daemon.log.1` (newest) to `daemon.log.{KEEP}`.
pub(crate) const KEEP: usize = 3;

pub(crate) struct DaemonLog {
    stdout: LogStream,
    stderr: LogStream,
}

impl DaemonLog {
    /// `state_dir`: the daemon's, whose `daemon.log` this may rotate.
    pub(crate) fn install(state_dir: Option<&Path>) -> io::Result<Self> {
        io::stdout().flush()?;
        io::stderr().flush()?;
        // Cloned descriptors are close-on-exec and never point back into the
        // logger, so its own writes cannot recursively feed its input.
        let stdout = File::from(io::stdout().as_fd().try_clone_to_owned()?);
        let stderr = File::from(io::stderr().as_fd().try_clone_to_owned()?);
        let managed = state_dir.map(|dir| dir.join("daemon.log"));
        let same = same_file(&stdout, &stderr);
        let mut outputs = vec![Output::new(stdout, managed.as_deref())];
        if !same {
            outputs.push(Output::new(stderr, managed.as_deref()));
        }
        let mut shared = Shared {
            outputs,
            repeats: Repeats::new(REPEAT_WINDOW, REPEAT_KEYS),
        };
        // At start, under the lock: join the current log if another daemon
        // rotated it since this one's was opened, and rotate one at the cap.
        for index in 0..shared.outputs.len() {
            shared.sync(index);
        }
        let shared = Arc::new(Mutex::new(shared));
        let log = Self {
            stdout: LogStream::new(Arc::clone(&shared), 0)?,
            stderr: LogStream::new(shared, usize::from(!same))?,
        };
        dup2_stdout(&log.stdout.writer)?;
        dup2_stderr(&log.stderr.writer)?;
        Ok(log)
    }
}

impl Drop for DaemonLog {
    fn drop(&mut self) {
        // Flush Rust's partial stdout buffer before restoring descriptors. The
        // guard encloses the runtime, so all worker teardown output is included.
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
        {
            // Restore to the file each stream writes now, which after a
            // rotation is no longer the one the launcher opened.
            let shared = lock(&self.stdout.shared);
            let _ = dup2_stdout(&shared.outputs[self.stdout.output].file);
            let _ = dup2_stderr(&shared.outputs[self.stderr.output].file);
        }
        // Each LogStream then drains and joins its reader on field destruction.
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn same_file(left: &File, right: &File) -> bool {
    match (left.metadata(), right.metadata()) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

struct LogStream {
    writer: UnixStream,
    reader: Option<JoinHandle<io::Result<()>>>,
    shared: Arc<Mutex<Shared>>,
    output: usize,
}

impl LogStream {
    fn new(shared: Arc<Mutex<Shared>>, output: usize) -> io::Result<Self> {
        let (input, writer) = UnixStream::pair()?;
        let reader_shared = Arc::clone(&shared);
        let reader = thread::Builder::new()
            .name("daemon-log".into())
            .spawn(move || pump(input, &reader_shared, output))?;
        Ok(Self {
            writer,
            reader: Some(reader),
            shared,
            output,
        })
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        // shutdown, rather than merely close, produces EOF even when an exec'd
        // child inherited stdout/stderr. Already-written bytes are still drained;
        // a surviving child cannot keep daemon shutdown waiting for pipe EOF.
        let _ = self.writer.shutdown(Shutdown::Write);
        if let Some(reader) = self.reader.take() {
            let result = reader
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("daemon log reader panicked")));
            if let Err(error) = result {
                let message = format!("pij-rs: daemon log sink failed: {error}\n");
                let _ = lock(&self.shared).write_line(self.output, message.as_bytes());
            }
        }
    }
}

/// Read one stream's lines and hand each to the shared sink. While any repeat
/// is being counted the read waits at most one window, so a burst that
/// stops still gets its summary; otherwise it blocks with no timer at all.
fn pump(input: UnixStream, shared: &Mutex<Shared>, output: usize) -> io::Result<()> {
    let mut armed = false;
    let mut input = BufReader::new(input);
    let mut line = Vec::new();
    loop {
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return lock(shared).finish(),
            Ok(_) => {
                lock(shared).line(output, &line, Instant::now())?;
                line.clear();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                // A partial line stays in `line` for the next read.
                lock(shared).flush_due(Instant::now())?;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
        let (counting, window) = {
            let shared = lock(shared);
            (shared.repeats.counting(), shared.repeats.window)
        };
        if counting != armed {
            input
                .get_ref()
                .set_read_timeout(counting.then_some(window))?;
            armed = counting;
        }
    }
}

struct Shared {
    outputs: Vec<Output>,
    repeats: Repeats,
}

impl Shared {
    /// One line from a stream: written, or counted as a repeat.
    fn line(&mut self, output: usize, line: &[u8], now: Instant) -> io::Result<()> {
        self.flush_due(now)?;
        match self.repeats.admit(line, output, now) {
            Admit::Count => Ok(()),
            Admit::Write(summaries) => {
                for summary in summaries {
                    self.write_summary(&summary)?;
                }
                self.write_line(output, line)
            }
        }
    }

    fn flush_due(&mut self, now: Instant) -> io::Result<()> {
        for summary in self.repeats.due(now) {
            self.write_summary(&summary)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        for summary in self.repeats.drain() {
            self.write_summary(&summary)?;
        }
        for output in &mut self.outputs {
            output.file.flush()?;
        }
        Ok(())
    }

    fn write_summary(&mut self, summary: &Summary) -> io::Result<()> {
        let text = format!(
            "pij-rs: daemon log: {} more of this line in {} s after it was written: {}\n",
            summary.count,
            self.repeats.window.as_secs(),
            summary.template
        );
        self.write_line(summary.output, text.as_bytes())
    }

    /// Timestamp and write one line whole, then rotate if it filled the file.
    fn write_line(&mut self, output: usize, line: &[u8]) -> io::Result<()> {
        let bytes = stamp(line)?;
        let target = &mut self.outputs[output];
        target.file.write_all(&bytes)?;
        target.file.flush()?;
        if let Some(rotation) = &mut target.rotation {
            rotation.size += bytes.len() as u64;
        }
        self.rotate_if_due(output);
        Ok(())
    }

    /// Rotate a managed output once it reaches the cap.
    fn rotate_if_due(&mut self, output: usize) {
        if self.outputs[output]
            .rotation
            .as_ref()
            .is_some_and(|rotation| rotation.size >= rotation.cap)
        {
            self.sync(output);
        }
    }

    /// Look at a managed output's path under the rotation lock and act on
    /// it ([`Rotation::sync`]): at start, and at each cap. A rotation that
    /// fails or can't get the lock keeps the current file, says so in it, and
    /// is next tried a further cap later. Its note is written straight to the
    /// file, never back through here.
    fn sync(&mut self, output: usize) {
        let target = &mut self.outputs[output];
        let Some(rotation) = &mut target.rotation else {
            return;
        };
        let name = rotation
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (note, unmanaged) = match rotation.sync(&target.file) {
            Ok(Synced::Current) => (None, false),
            Ok(Synced::Rotated(file, rotated)) => {
                target.file = file;
                rotation.size = 0;
                (
                    Some(format!(
                        "pij-rs: daemon log: rotated at {rotated} bytes; older lines are in {name}.1\n"
                    )),
                    false,
                )
            }
            Ok(Synced::Joined(file)) => {
                target.file = file;
                (
                    Some(format!(
                        "pij-rs: daemon log: another process rotated {name}; writing to the new one\n"
                    )),
                    false,
                )
            }
            Ok(Synced::Busy) => {
                rotation.size = 0;
                (
                    Some(format!(
                        "pij-rs: daemon log: not rotated: another process holds {name}.lock\n"
                    )),
                    false,
                )
            }
            Ok(Synced::Unmanaged) => (
                Some(format!(
                    "pij-rs: daemon log: {name} is no longer this file; not rotating it\n"
                )),
                true,
            ),
            Err(error) => {
                rotation.size = 0;
                (
                    Some(format!(
                        "pij-rs: daemon log: rotation failed, kept writing: {error}\n"
                    )),
                    false,
                )
            }
        };
        if let Some(note) = note
            && let Ok(bytes) = stamp(note.as_bytes())
            && target.file.write_all(&bytes).is_ok()
        {
            let _ = target.file.flush();
            rotation.size += bytes.len() as u64;
        }
        if unmanaged {
            target.rotation = None;
        }
    }
}

/// One physical line: an RFC 3339 timestamp, a space, the line, and a newline
/// if the line had none (a final fragment, finished before a later daemon boot
/// appends to the same log).
fn stamp(line: &[u8]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(line.len() + 40);
    OffsetDateTime::now_utc()
        .format_into(&mut bytes, &Rfc3339)
        .map_err(io::Error::other)?;
    bytes.push(b' ');
    bytes.extend_from_slice(line);
    if !line.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    Ok(bytes)
}

struct Output {
    file: File,
    rotation: Option<Rotation>,
}

impl Output {
    fn new(file: File, managed: Option<&Path>) -> Self {
        let rotation = managed.and_then(|path| Rotation::adopt(&file, path, ROTATE_AT, KEEP));
        Self { file, rotation }
    }
}

/// The managed `daemon.log`: its path, its size as written, and the bound.
struct Rotation {
    path: PathBuf,
    size: u64,
    cap: u64,
    keep: usize,
}

/// What one locked look at the managed log found and did.
enum Synced {
    /// This process rotated a file of this many bytes; write to the fresh one.
    Rotated(File, u64),
    /// Another process rotated since this one's file was opened: the path
    /// names another regular file, now opened for append and written to.
    Joined(File),
    /// The path still names this process's file, under the cap.
    Current,
    /// Another process held the lock throughout: nothing was changed.
    Busy,
    /// The path is gone, or not a regular file: no longer managed.
    Unmanaged,
}

/// Attempts at the rotation lock, [`LOCK_PAUSE`] apart (about 2 s in all).
const LOCK_TRIES: u32 = 200;
const LOCK_PAUSE: Duration = Duration::from_millis(10);

impl Rotation {
    /// Manage `path` only if `file` is the state dir's `daemon.log`: the very
    /// file the path names (device and inode; a symlink is its own inode, so it
    /// never matches), or one a rotation by another daemon just moved off the
    /// path after this one's launcher opened it (R301-001): the file now at
    /// `daemon.log.1`, or a regular file with no name left that a trim
    /// recorded as the managed log it unlinked (`daemon.log.trimmed`, F251).
    /// A deleted file nothing recorded is foreign and stays foreign: the
    /// record is a **brake**, and removing it adopts more. The first look
    /// under the lock ([`Self::sync`]) then joins the file the path names.
    fn adopt(file: &File, path: &Path, cap: u64, keep: usize) -> Option<Self> {
        let open = file.metadata().ok()?;
        let same =
            |named: &std::fs::Metadata| named.dev() == open.dev() && named.ino() == open.ino();
        let named = std::fs::symlink_metadata(path).ok()?;
        let mut moved = path.as_os_str().to_os_string();
        moved.push(".1");
        let rotated_away = named.file_type().is_file()
            && open.file_type().is_file()
            && ((open.nlink() == 0 && was_trimmed(path, open.dev(), open.ino()))
                || std::fs::symlink_metadata(&moved).is_ok_and(|moved| same(&moved)));
        (same(&named) || rotated_away).then(|| Self {
            path: path.to_path_buf(),
            size: open.len(),
            cap,
            keep,
        })
    }

    fn suffixed(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    fn numbered(&self, n: usize) -> PathBuf {
        self.suffixed(&format!(".{n}"))
    }

    /// R301-001: every look at the managed log, at start and at each cap, is
    /// made holding `daemon.log.lock` (an exclusive `flock`), so two daemons
    /// started together never rotate at once, and each re-checks under the
    /// lock which file the path names before doing anything. Both are
    /// **brakes**: without the lock two rotations interleave and unlink each
    /// other's files; without the re-check a process rotates a file another
    /// process already replaced. Removing either makes rotation do more.
    ///
    /// A process that can't get the lock in about 2 s changes nothing and keeps
    /// writing to the file it has: the holder is mid-rotation, so the path is
    /// in flux and joining it then could pick up a file about to be moved.
    fn sync(&mut self, current: &File) -> io::Result<Synced> {
        let Some(_lock) = self.lock()? else {
            return Ok(Synced::Busy);
        };
        let mine = current.metadata()?;
        let joined = match std::fs::symlink_metadata(&self.path) {
            Ok(named) if named.dev() == mine.dev() && named.ino() == mine.ino() => None,
            Ok(named) if named.file_type().is_file() => Some(open_regular_append(&self.path)?),
            _ => return Ok(Synced::Unmanaged),
        };
        self.size = match &joined {
            Some(file) => file.metadata()?.len(),
            None => mine.len(),
        };
        if self.size < self.cap {
            return Ok(joined.map_or(Synced::Current, Synced::Joined));
        }
        let rotated = self.size;
        self.rotate().map(|fresh| Synced::Rotated(fresh, rotated))
    }

    /// The exclusive lock on `daemon.log.lock`, tried without blocking up to
    /// [`LOCK_TRIES`] times; `None` if another process held it throughout. The
    /// lock file is never removed: removing it would let two processes lock
    /// two different files.
    fn lock(&self) -> io::Result<Option<Flock<File>>> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(OFlag::O_NOFOLLOW.bits())
            .open(self.suffixed(".lock"))?;
        for _ in 0..LOCK_TRIES {
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(lock) => return Ok(Some(lock)),
                Err((back, Errno::EWOULDBLOCK)) => {
                    file = back;
                    thread::sleep(LOCK_PAUSE);
                }
                Err((_, errno)) => return Err(errno.into()),
            }
        }
        Ok(None)
    }

    /// Shift `.1`… up one (the last is replaced), move the current file to
    /// `.1`, and open a fresh file at the path. Called only under the lock,
    /// with the path checked to be this process's file. The fresh file is made
    /// first, so a failure before the shift changes nothing.
    fn rotate(&mut self) -> io::Result<File> {
        let fresh_path = self.suffixed(".new");
        let fresh = staging(&fresh_path)?;
        let shifted = (|| {
            for n in (1..self.keep).rev() {
                match std::fs::rename(self.numbered(n), self.numbered(n + 1)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
            if self.size > self.cap + self.cap / 16 {
                self.keep_tail()
            } else {
                std::fs::rename(&self.path, self.numbered(1))
            }
        })();
        if let Err(error) = shifted {
            let _ = std::fs::remove_file(&fresh_path);
            return Err(error);
        }
        std::fs::rename(&fresh_path, &self.path)?;
        Ok(fresh)
    }

    /// A file more than a sixteenth over the cap (one from before the bound,
    /// or one line longer than that) becomes `.1` holding only its last `cap`
    /// bytes, from the start of a line. So no rotated file exceeds 17/16 cap.
    ///
    /// The source is opened without following a symlink (S301-R2-N2), and its
    /// device and inode are recorded in `daemon.log.trimmed` before its name
    /// is removed, so a daemon whose launcher opened it can prove at start
    /// that its unnamed file was the managed log (F251).
    fn keep_tail(&self) -> io::Result<()> {
        let mut source = BufReader::new(
            OpenOptions::new()
                .read(true)
                .custom_flags(OFlag::O_NOFOLLOW.bits())
                .open(&self.path)?,
        );
        let unlinked = source.get_ref().metadata()?;
        let length = unlinked.len();
        source.seek(SeekFrom::Start(length.saturating_sub(self.cap)))?;
        let mut partial = Vec::new();
        source.read_until(b'\n', &mut partial)?;
        let staged = self.suffixed(".1.new");
        let mut target = staging(&staged)?;
        io::copy(&mut source, &mut target)?;
        target.flush()?;
        std::fs::rename(&staged, self.numbered(1))?;
        self.record_trimmed(unlinked.dev(), unlinked.ino())?;
        std::fs::remove_file(&self.path)
    }

    /// Add a trimmed file's device and inode to `daemon.log.trimmed`, keeping
    /// the newest [`TRIMMED_KEPT`]. Under the lock; replaced whole by a rename,
    /// so a reader sees the old record or the new one, never half of one.
    fn record_trimmed(&self, dev: u64, ino: u64) -> io::Result<()> {
        let record = trimmed_record(&self.path);
        let old = read_no_follow(&record).unwrap_or_default();
        let mut lines: Vec<&str> = old.lines().collect();
        let entry = format!("{dev} {ino}");
        lines.push(&entry);
        let start = lines.len().saturating_sub(TRIMMED_KEPT);
        let staged = self.suffixed(".trimmed.new");
        let mut file = staging(&staged)?;
        file.write_all((lines[start..].join("\n") + "\n").as_bytes())?;
        file.flush()?;
        std::fs::rename(&staged, &record)
    }
}

/// Trimmed generations remembered in `daemon.log.trimmed`.
const TRIMMED_KEPT: usize = 16;

fn trimmed_record(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".trimmed");
    PathBuf::from(name)
}

fn read_no_follow(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    Ok(text)
}

/// Did a trim record (`dev`, `ino`) as the managed log at `path` it unlinked?
fn was_trimmed(path: &Path, dev: u64, ino: u64) -> bool {
    let entry = format!("{dev} {ino}");
    read_no_follow(&trimmed_record(path))
        .is_ok_and(|record| record.lines().any(|line| line == entry))
}

/// S301-R2-N1: open the path to join it, append-only, without following a
/// symlink and without waiting (a FIFO swapped in after the check would
/// otherwise block the log thread waiting for a reader), and use it only if
/// what was opened is a regular file. A **brake**: removing it joins more.
fn open_regular_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .append(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)?;
    if file.metadata()?.file_type().is_file() {
        Ok(file)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon.log is not a regular file",
        ))
    }
}

/// S301-N1: a staging file is made new (`O_EXCL`, which refuses any existing
/// name, a symlink included), after removing a leftover. Only the lock holder
/// makes staging files, so a leftover is a crashed rotation's or a planted
/// link, never another live process's file; removing a link never touches
/// its target.
fn staging(path: &Path) -> io::Result<File> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    // R301-R2-001: append, so after a rotation this daemon's writes land at
    // the end even when a joined daemon appended in between, never over it.
    OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// R301-003: lines that are never counted, so every one is written: a Rust
/// panic's header, its `RUST_BACKTRACE` note and every backtrace line.
fn never_counted(line: &[u8]) -> bool {
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim_start();
    let indented = trimmed.len() < text.len();
    text.contains("' panicked at ")
        || trimmed.starts_with("stack backtrace:")
        || trimmed.starts_with("note: run with `RUST_BACKTRACE")
        || trimmed.starts_with("note: Some details are omitted")
        || (indented && trimmed.starts_with("at "))
        || (indented
            && trimmed.split_once(": ").is_some_and(|(frame, _)| {
                !frame.is_empty() && frame.bytes().all(|b| b.is_ascii_digit())
            }))
}

/// At most this many bytes of a line are kept for its summary, in a string
/// allocated at exactly that size (R301-P1).
fn shown(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    let mut end = text.len().min(TEMPLATE_SHOWN);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = end < text.len();
    let mut shown = String::with_capacity(end + if cut { '…'.len_utf8() } else { 0 });
    shown.push_str(&text[..end]);
    if cut {
        shown.push('…');
    }
    shown
}

#[derive(Debug, PartialEq, Eq)]
enum Admit {
    /// Write the line, after these summaries (a window that ended, or a
    /// line forgotten to make room).
    Write(Vec<Summary>),
    /// A repeat inside its window: counted, not written.
    Count,
}

#[derive(Debug, PartialEq, Eq)]
struct Summary {
    output: usize,
    count: u64,
    template: String,
}

struct Seen {
    until: Instant,
    count: u64,
    output: usize,
    shown: String,
}

impl Seen {
    fn summary(&self) -> Option<Summary> {
        (self.count > 0).then(|| Summary {
            output: self.output,
            count: self.count,
            template: self.shown.clone(),
        })
    }
}

/// Repeated-line memory: (output, line) hash → its open window.
struct Repeats {
    window: Duration,
    capacity: usize,
    seen: HashMap<u64, Seen>,
    /// The earliest window end with a count, so a busy log pays the sweep
    /// at most once per window rather than once per line.
    next_due: Option<Instant>,
}

impl Repeats {
    fn new(window: Duration, capacity: usize) -> Self {
        Self {
            window,
            capacity,
            seen: HashMap::with_capacity(capacity),
            next_due: None,
        }
    }

    fn counting(&self) -> bool {
        self.next_due.is_some()
    }

    /// R301-002/003: a line repeats only if the same output (one index per
    /// destination file, so stdout and stderr to one file share it) already
    /// wrote the exact same line, its line ending aside, within the window.
    fn admit(&mut self, line: &[u8], output: usize, now: Instant) -> Admit {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if never_counted(line) {
            return Admit::Write(Vec::new());
        }
        let mut hasher = DefaultHasher::new();
        output.hash(&mut hasher);
        line.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(seen) = self.seen.get_mut(&key) {
            if now < seen.until {
                seen.count += 1;
                self.next_due = Some(self.next_due.map_or(seen.until, |due| due.min(seen.until)));
                return Admit::Count;
            }
            let summary = seen.summary();
            seen.until = now + self.window;
            seen.count = 0;
            return Admit::Write(summary.into_iter().collect());
        }
        let mut summaries = Vec::new();
        if self.seen.len() >= self.capacity {
            summaries.extend(self.due(now));
            self.seen.retain(|_, seen| now < seen.until);
        }
        if self.seen.len() >= self.capacity
            && let Some(&oldest) = self
                .seen
                .iter()
                .min_by_key(|(_, seen)| seen.until)
                .map(|(key, _)| key)
            && let Some(seen) = self.seen.remove(&oldest)
        {
            summaries.extend(seen.summary());
            self.recompute_due();
        }
        self.seen.insert(
            key,
            Seen {
                until: now + self.window,
                count: 0,
                output,
                shown: shown(line),
            },
        );
        Admit::Write(summaries)
    }

    /// Summaries for every window that has ended with a count; those windows
    /// are forgotten.
    fn due(&mut self, now: Instant) -> Vec<Summary> {
        if self.next_due.is_none_or(|due| now < due) {
            return Vec::new();
        }
        let mut summaries = Vec::new();
        self.seen.retain(|_, seen| {
            if now < seen.until {
                return true;
            }
            summaries.extend(seen.summary());
            false
        });
        self.recompute_due();
        summaries
    }

    fn drain(&mut self) -> Vec<Summary> {
        self.next_due = None;
        self.seen
            .drain()
            .filter_map(|(_, seen)| seen.summary())
            .collect()
    }

    fn recompute_due(&mut self) {
        self.next_due = self
            .seen
            .values()
            .filter(|seen| seen.count > 0)
            .map(|seen| seen.until)
            .min();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    use super::*;

    fn payloads(log: &[u8]) -> Vec<Vec<u8>> {
        log.split_inclusive(|byte| *byte == b'\n')
            .map(|line| {
                let split = line.iter().position(|byte| *byte == b' ').unwrap();
                let timestamp = std::str::from_utf8(&line[..split]).unwrap();
                assert!(OffsetDateTime::parse(timestamp, &Rfc3339).is_ok());
                line[split + 1..].to_vec()
            })
            .collect()
    }

    fn unmanaged(file: File) -> Arc<Mutex<Shared>> {
        Arc::new(Mutex::new(Shared {
            outputs: vec![Output {
                file,
                rotation: None,
            }],
            repeats: Repeats::new(REPEAT_WINDOW, REPEAT_KEYS),
        }))
    }

    #[test]
    fn timestamps_blank_long_non_utf8_and_unterminated_lines_without_changing_payloads() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-payloads");
        let path = root.join("daemon.log");
        let shared = unmanaged(File::create(&path).unwrap());
        let long = vec![b'x'; 32 * 1024];
        let mut input = b"first\n\n".to_vec();
        input.extend_from_slice(&long);
        input.extend_from_slice(b"\n\xffpartial");
        let (reader, mut writer) = UnixStream::pair().unwrap();
        // The pair's buffer is smaller than the long line: write from a thread.
        let feeder = thread::spawn(move || {
            writer.write_all(&input).unwrap();
            writer.shutdown(Shutdown::Write).unwrap();
        });
        pump(reader, &shared, 0).unwrap();
        feeder.join().unwrap();
        let mut expected_long = long;
        expected_long.push(b'\n');
        assert_eq!(
            payloads(&fs::read(&path).unwrap()),
            [
                b"first\n".to_vec(),
                b"\n".to_vec(),
                expected_long,
                b"\xffpartial\n".to_vec(),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Through the real reader: a repeated template is written once, and its
    /// count is written when the stream ends.
    #[test]
    fn the_reader_writes_a_repeat_once_and_its_count_at_the_end() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-repeats");
        let path = root.join("daemon.log");
        let shared = unmanaged(File::create(&path).unwrap());
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let feeder = thread::spawn(move || {
            for _ in 0..500 {
                writeln!(writer, "pane observer: list-panes failed (exit 1)").unwrap();
            }
            writer.write_all(b"daemon: shutting down\n").unwrap();
            writer.shutdown(Shutdown::Write).unwrap();
        });
        pump(reader, &shared, 0).unwrap();
        feeder.join().unwrap();
        assert_eq!(
            payloads(&fs::read(&path).unwrap()),
            [
                b"pane observer: list-panes failed (exit 1)\n".to_vec(),
                b"daemon: shutting down\n".to_vec(),
                b"pij-rs: daemon log: 499 more of this line in 60 s after it was written: pane observer: list-panes failed (exit 1)\n".to_vec(),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A burst that stops still gets its summary without waiting for the
    /// stream to end (the read times out after one window), and a summary due
    /// while other lines flow is written before the next of them.
    #[test]
    fn a_summary_is_written_when_its_window_ends_not_at_the_end() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-window");
        let path = root.join("daemon.log");
        let window = Duration::from_millis(300);
        let shared = Arc::new(Mutex::new(Shared {
            outputs: vec![Output {
                file: File::create(&path).unwrap(),
                rotation: None,
            }],
            repeats: Repeats::new(window, REPEAT_KEYS),
        }));
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let pumping = Arc::clone(&shared);
        let pump = thread::spawn(move || pump(reader, &pumping, 0));
        for _ in 0..3 {
            writeln!(writer, "burst").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !fs::read_to_string(&path)
            .unwrap()
            .contains("2 more of this line")
        {
            assert!(
                Instant::now() < deadline,
                "no summary before the stream ended"
            );
            thread::sleep(Duration::from_millis(20));
        }
        // Again, but now the summary falls due while other lines keep coming.
        // Lines every 50 ms never let the read time out, so only the check
        // made on each line can write it.
        writeln!(writer, "burst").unwrap();
        writeln!(writer, "burst").unwrap();
        for word in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
            thread::sleep(Duration::from_millis(50));
            writeln!(writer, "other {word}").unwrap();
        }
        writer.shutdown(Shutdown::Write).unwrap();
        pump.join().unwrap().unwrap();
        let lines: Vec<String> = payloads(&fs::read(&path).unwrap())
            .into_iter()
            .map(|line| String::from_utf8(line).unwrap())
            .collect();
        let summary = lines
            .iter()
            .rposition(|line| line.contains("1 more of this line"));
        let other = lines.iter().position(|line| line == "other j\n");
        assert!(summary.is_some() && summary < other, "{lines:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_drains_split_writes_without_waiting_for_inherited_writer() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-drain");
        let path = root.join("daemon.log");
        let stream = LogStream::new(unmanaged(File::create(&path).unwrap()), 0).unwrap();
        let mut inherited = stream.writer.try_clone().unwrap();
        inherited.write_all(b"complete\nsplit").unwrap();
        inherited.write_all(b" fragment").unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let drain = thread::spawn(move || {
            drop(stream);
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("inherited writer must not hold daemon log shutdown open");
        drain.join().unwrap();
        assert_eq!(
            payloads(&fs::read(&path).unwrap()),
            [b"complete\n".to_vec(), b"split fragment\n".to_vec()]
        );
        drop(inherited);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn captures_raw_panic_output_and_flushes_during_unwind() {
        const CHILD_LOG: &str = "PIJ_TEST_PANIC_DAEMON_LOG";
        if let Some(path) = std::env::var_os(CHILD_LOG) {
            // Re-exec isolates process-wide descriptor changes from parallel
            // tests. Redirect after the harness prelude and exit before its
            // summary, so this file contains only output crossing our boundary.
            let file = File::create(path).unwrap();
            dup2_stdout(&file).unwrap();
            dup2_stderr(&file).unwrap();
            let panicked = std::panic::catch_unwind(|| {
                let _log = DaemonLog::install(None).unwrap();
                io::stdout()
                    .write_all(b"raw stdout\nfinal fragment")
                    .unwrap();
                io::stderr().write_all(b"raw stderr\n").unwrap();
                panic!("first panic line\nsecond panic line");
            });
            std::process::exit(if panicked.is_err() { 0 } else { 1 });
        }
        let root = pij_testkit::fresh_dir("pij-daemon-log-panic");
        let path = root.join("daemon.log");
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon_log::tests::captures_raw_panic_output_and_flushes_during_unwind",
                "--nocapture",
            ])
            .env(CHILD_LOG, &path)
            .env("RUST_BACKTRACE", "0")
            .stdout(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let lines = payloads(&fs::read(&path).unwrap());
        for expected in [
            b"raw stdout\n".as_slice(),
            b"raw stderr\n".as_slice(),
            b"final fragment\n".as_slice(),
            b"first panic line\n".as_slice(),
            b"second panic line\n".as_slice(),
        ] {
            assert!(lines.iter().any(|line| line == expected), "{lines:?}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// The two bursts in the live log (#90), as they were written: 4,498
    /// "background reconciliation failed" lines in 9 minutes (2026-10-30, ten a
    /// second) and 3,061 "list-panes failed" lines, one per 600 ms observer
    /// tick while tmux's default server was gone (2026-11-07). Each is the
    /// exact line the live log holds, repeated byte for byte (R301-003: keyed
    /// on the exact line, they still collapse). Each burst writes its first
    /// line, then one summary per minute with the exact count.
    #[test]
    fn both_live_bursts_collapse_to_one_line_and_a_summary_a_minute() {
        let background = "pij-rs: background reconciliation failed: background/refused: owner pij-different-smelt of source bg-1795abca2e9b4f0ac1e3a2a5 is gone; its final turn waits\n";
        let start = Instant::now();
        for (lines, step, render) in [
            (
                4_498_u64,
                Duration::from_millis(100),
                Box::new(|_: u64| background.to_string()) as Box<dyn Fn(u64) -> String>,
            ),
            (
                3_061,
                Duration::from_millis(600),
                Box::new(|_: u64| {
                    "pij-rs pane observer: tmux: list-panes failed (exit 1): error connecting to /private/tmp/tmux-501/default (No such file or directory) — run `tmux list-panes -a` to verify the target\n".to_string()
                }),
            ),
        ] {
            let mut repeats = Repeats::new(REPEAT_WINDOW, REPEAT_KEYS);
            let (mut written, mut counted, mut summaries) = (0_u64, 0_u64, Vec::new());
            for n in 0..lines {
                let now = start + step * u32::try_from(n).unwrap();
                summaries.extend(repeats.due(now));
                match repeats.admit(render(n).as_bytes(), 0, now) {
                    Admit::Write(evicted) => {
                        summaries.extend(evicted);
                        written += 1;
                    }
                    Admit::Count => counted += 1,
                }
            }
            summaries.extend(repeats.drain());
            let minutes = (step * u32::try_from(lines).unwrap())
                .as_secs()
                .div_ceil(60);
            assert_eq!(written, minutes, "one line per window");
            assert_eq!(
                summaries.iter().map(|summary| summary.count).sum::<u64>(),
                counted,
                "every counted line is in a summary"
            );
            assert_eq!(written + counted, lines);
            assert!(summaries.len() as u64 <= minutes, "{}", summaries.len());
        }
    }

    #[test]
    fn a_window_counts_only_the_same_line_and_reports_when_it_closes() {
        let start = Instant::now();
        let mut repeats = Repeats::new(Duration::from_secs(60), 8);
        assert_eq!(
            repeats.admit(b"seat a refused\n", 0, start),
            Admit::Write(Vec::new())
        );
        assert!(!repeats.counting(), "nothing counted, so no timer");
        assert_eq!(repeats.admit(b"seat a refused\n", 0, start), Admit::Count);
        assert!(repeats.counting());
        assert_eq!(
            repeats.admit(b"seat b refused\n", 0, start),
            Admit::Write(Vec::new()),
            "another seat is another line"
        );
        assert_eq!(
            repeats.due(start + Duration::from_secs(59)),
            Vec::new(),
            "the window is still open"
        );
        assert_eq!(
            repeats.due(start + Duration::from_secs(60)),
            vec![Summary {
                output: 0,
                count: 1,
                template: "seat a refused".into(),
            }]
        );
        assert!(!repeats.counting());
        // After its window the line is written again.
        assert_eq!(
            repeats.admit(b"seat a refused\n", 0, start + Duration::from_secs(61)),
            Admit::Write(Vec::new())
        );
    }

    /// R301-003 (moth's repro): numbers are part of the line. Two panic sites
    /// in one file, or two status codes, are two lines; and a panic's header
    /// and backtrace are never counted, even repeated exactly.
    #[test]
    fn numbers_tell_lines_apart_and_panics_are_never_counted() {
        let start = Instant::now();
        let mut repeats = Repeats::new(Duration::from_secs(60), 16);
        for line in [
            "thread 'tokio-runtime-worker' panicked at crates/daemon/src/lib.rs:100:20:",
            "thread 'tokio-runtime-worker' panicked at crates/daemon/src/lib.rs:200:20:",
            "store: status 409",
            "store: status 503",
        ] {
            assert_eq!(
                repeats.admit(line.as_bytes(), 0, start),
                Admit::Write(Vec::new()),
                "{line}"
            );
        }
        for line in [
            "thread 'tokio-runtime-worker' panicked at crates/daemon/src/lib.rs:100:20:",
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
            "stack backtrace:",
            "   0: std::panicking::begin_panic",
            "             at /rustc/abc/library/std/src/panicking.rs:686:12",
        ] {
            for _ in 0..3 {
                assert_eq!(
                    repeats.admit(line.as_bytes(), 0, start),
                    Admit::Write(Vec::new()),
                    "never counted: {line}"
                );
            }
        }
        assert_eq!(repeats.admit(b"store: status 409", 0, start), Admit::Count);
    }

    /// R301-002 (moth's exact-source repro): stdout and stderr to two files are
    /// two outputs, and each shows its own first line and summary; to one file
    /// they are one output and coalesce.
    #[test]
    fn each_output_keeps_its_own_first_line_and_summary() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-outputs");
        let (out, err) = (root.join("out.log"), root.join("err.log"));
        let mut shared = Shared {
            outputs: vec![
                Output {
                    file: File::create(&out).unwrap(),
                    rotation: None,
                },
                Output {
                    file: File::create(&err).unwrap(),
                    rotation: None,
                },
            ],
            repeats: Repeats::new(REPEAT_WINDOW, REPEAT_KEYS),
        };
        let now = Instant::now();
        for _ in 0..2 {
            shared.line(0, b"failure\n", now).unwrap();
            shared.line(1, b"failure\n", now).unwrap();
        }
        shared.finish().unwrap();
        let summary =
            b"pij-rs: daemon log: 1 more of this line in 60 s after it was written: failure\n";
        for path in [&out, &err] {
            assert_eq!(
                payloads(&fs::read(path).unwrap()),
                [b"failure\n".to_vec(), summary.to_vec()],
                "{}",
                path.display()
            );
        }
        // One file, one output: the second stream's line is a repeat.
        let mut repeats = Repeats::new(REPEAT_WINDOW, REPEAT_KEYS);
        assert_eq!(
            repeats.admit(b"failure\n", 0, now),
            Admit::Write(Vec::new())
        );
        assert_eq!(repeats.admit(b"failure\n", 0, now), Admit::Count);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_full_memory_forgets_the_oldest_window_and_reports_its_count() {
        let start = Instant::now();
        let mut repeats = Repeats::new(Duration::from_secs(60), 2);
        repeats.admit(b"one\n", 0, start);
        repeats.admit(b"one\n", 0, start);
        repeats.admit(b"two\n", 0, start + Duration::from_secs(1));
        assert_eq!(
            repeats.admit(b"three\n", 0, start + Duration::from_secs(2)),
            Admit::Write(vec![Summary {
                output: 0,
                count: 1,
                template: "one".into(),
            }])
        );
        assert_eq!(repeats.seen.len(), 2, "the bound holds");
        assert_eq!(
            repeats.admit(b"two\n", 0, start + Duration::from_secs(3)),
            Admit::Count
        );
        // A forgotten template is written again, and makes room by forgetting
        // the window that ends soonest, `two`, whose count is reported.
        assert_eq!(
            repeats.admit(b"one\n", 0, start + Duration::from_secs(3)),
            Admit::Write(vec![Summary {
                output: 0,
                count: 1,
                template: "two".into(),
            }])
        );
    }

    /// R301-P1 (snail's probe): 256 distinct 32 KiB lines keep at most
    /// `TEMPLATE_SHOWN` bytes plus an ellipsis each, in allocated capacity, not
    /// just in length.
    #[test]
    fn remembered_lines_are_bounded_in_allocated_bytes() {
        let mut repeats = Repeats::new(REPEAT_WINDOW, REPEAT_KEYS);
        let now = Instant::now();
        for n in 0..REPEAT_KEYS {
            let mut line = format!("{n:03} ").into_bytes();
            line.extend(std::iter::repeat_n(b'x', 32 * 1024));
            repeats.admit(&line, 0, now);
        }
        assert_eq!(repeats.seen.len(), REPEAT_KEYS);
        let bound = TEMPLATE_SHOWN + '…'.len_utf8();
        assert!(
            repeats
                .seen
                .values()
                .all(|seen| seen.shown.capacity() <= bound)
        );
        let retained: usize = repeats
            .seen
            .values()
            .map(|seen| seen.shown.capacity())
            .sum();
        assert!(retained <= REPEAT_KEYS * bound, "{retained}");
        // Multi-byte text is cut on a character boundary, still bounded.
        let accented = "é".repeat(400);
        let shown = super::shown(accented.as_bytes());
        assert!(shown.capacity() <= bound && shown.ends_with('…'));
        assert_eq!(super::shown(b"short"), "short");
    }

    fn managed(root: &Path, contents: &[u8], cap: u64, keep: usize) -> Shared {
        let path = root.join("daemon.log");
        fs::write(&path, contents).unwrap();
        let file = OpenOptions::new().append(true).open(&path).unwrap();
        let rotation = Rotation::adopt(&file, &path, cap, keep).expect("the managed file");
        Shared {
            outputs: vec![Output {
                file,
                rotation: Some(rotation),
            }],
            repeats: Repeats::new(REPEAT_WINDOW, REPEAT_KEYS),
        }
    }

    #[test]
    fn only_the_state_dirs_own_regular_file_is_rotated() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-adopt");
        let path = root.join("daemon.log");
        fs::write(&path, b"").unwrap();
        let own = File::open(&path).unwrap();
        assert!(Rotation::adopt(&own, &path, 100, 3).is_some());
        let other_path = root.join("other.log");
        fs::write(&other_path, b"").unwrap();
        let other = File::open(&other_path).unwrap();
        assert!(
            Rotation::adopt(&other, &path, 100, 3).is_none(),
            "another file"
        );
        let link = root.join("link.log");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Rotation::adopt(&own, &link, 100, 3).is_none(), "a symlink");
        assert!(
            Rotation::adopt(&own, &root.join("absent.log"), 100, 3).is_none(),
            "no file at the path"
        );
        // R301-001: rotated off the path by another daemon after this one's
        // launcher opened it: moved to `.1`, or trimmed away (no name left).
        fs::rename(&path, root.join("daemon.log.1")).unwrap();
        fs::write(&path, b"").unwrap();
        assert!(
            Rotation::adopt(&own, &path, 100, 3).is_some(),
            "moved to .1"
        );
        fs::remove_file(root.join("daemon.log.1")).unwrap();
        // F251 (moth's R301-R2-002): a deleted file nothing recorded is foreign.
        assert!(
            Rotation::adopt(&own, &path, 100, 3).is_none(),
            "unlinked, unrecorded"
        );
        let meta = own.metadata().unwrap();
        fs::write(
            root.join("daemon.log.trimmed"),
            format!("1 2\n{} {}\n", meta.dev(), meta.ino()),
        )
        .unwrap();
        assert!(
            Rotation::adopt(&own, &path, 100, 3).is_some(),
            "unlinked by a recorded trim"
        );
        assert!(
            Rotation::adopt(&other, &path, 100, 3).is_none(),
            "another named file still is not"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// F251 (moth's R301-R2-001): after a rotation the rotator's fresh file is
    /// append-only, so a joined daemon's lines written in between are never
    /// overwritten by the rotator's next write.
    #[test]
    fn a_rotator_and_a_joined_daemon_never_overwrite_each_other() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-interleave");
        let mut rotator = managed(&root, b"", 400, 3);
        for n in 0..8 {
            rotator
                .write_line(
                    0,
                    format!("rotator before {n} {}\n", "x".repeat(32)).as_bytes(),
                )
                .unwrap();
        }
        assert!(root.join("daemon.log.1").exists(), "the rotator rotated");
        let path = root.join("daemon.log");
        let mut joined = Shared {
            outputs: vec![Output {
                file: open_regular_append(&path).unwrap(),
                rotation: None,
            }],
            repeats: Repeats::new(REPEAT_WINDOW, REPEAT_KEYS),
        };
        joined.write_line(0, b"joined one\n").unwrap();
        rotator.write_line(0, b"rotator after\n").unwrap();
        joined.write_line(0, b"joined two\n").unwrap();
        rotator.write_line(0, b"rotator last\n").unwrap();
        let lines: Vec<String> = payloads(&fs::read(&path).unwrap())
            .into_iter()
            .map(|line| String::from_utf8(line).unwrap())
            .collect();
        let at = |wanted: &str| lines.iter().position(|line| line == wanted);
        let order = [
            at("joined one\n"),
            at("rotator after\n"),
            at("joined two\n"),
            at("rotator last\n"),
        ];
        assert!(order.iter().all(Option::is_some), "{lines:?}");
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{lines:?}");
        fs::remove_dir_all(root).unwrap();
    }

    /// F251: a trim records the file it unlinks, so a daemon whose launcher
    /// opened the oversized log before another daemon trimmed it can still
    /// prove that file was the managed log, and adopts it (then joins).
    #[test]
    fn a_trimmed_log_is_recorded_and_its_holders_still_adopt_it() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-trim-record");
        let mut old = String::new();
        for n in 0..100 {
            old.push_str(&format!("old line {n:03}\n"));
        }
        let mut trimmer = managed(&root, old.as_bytes(), 100, 3);
        let path = root.join("daemon.log");
        let late = OpenOptions::new().append(true).open(&path).unwrap();
        trimmer.sync(0);
        assert_eq!(late.metadata().unwrap().nlink(), 0, "trimmed away");
        assert!(Rotation::adopt(&late, &path, 100, 3).is_some());
        fs::remove_dir_all(root).unwrap();
    }

    /// S301-R2-N1: a FIFO at the path is never joined, and opening it never
    /// waits for a reader.
    #[test]
    fn a_fifo_at_the_path_is_never_joined_and_never_waits() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-fifo");
        let fifo = root.join("daemon.log");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let (done, finished) = mpsc::channel();
        let probe = fifo.clone();
        thread::spawn(move || done.send(open_regular_append(&probe).is_err()).unwrap());
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(3)),
            Ok(true),
            "no reader: refused at once"
        );
        let reader = OpenOptions::new()
            .read(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(&fifo)
            .unwrap();
        let error = open_regular_append(&fifo).expect_err("a FIFO with a reader");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        drop(reader);
        fs::remove_dir_all(root).unwrap();
    }

    /// S301-R2-N2: the trim never reads through a symlink at the path.
    #[test]
    fn the_trim_never_follows_a_symlink() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-trim-link");
        let elsewhere = root.join("elsewhere.txt");
        fs::write(&elsewhere, "private line\n".repeat(100)).unwrap();
        let path = root.join("daemon.log");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let rotation = Rotation {
            path: path.clone(),
            size: 1300,
            cap: 100,
            keep: 3,
        };
        assert!(rotation.keep_tail().is_err());
        assert!(!root.join("daemon.log.1").exists(), "nothing copied");
        assert_eq!(
            fs::read_to_string(&elsewhere).unwrap(),
            "private line\n".repeat(100)
        );
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// R301-001: a file another process put at the path since this one's was
    /// opened (it rotated) is joined under the lock, not rotated again.
    #[test]
    fn a_rotation_by_another_process_is_joined_not_repeated() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-joined");
        let mut shared = managed(&root, b"old\n", 1000, 3);
        let path = root.join("daemon.log");
        fs::rename(&path, root.join("daemon.log.1")).unwrap();
        fs::write(&path, b"theirs\n").unwrap();
        shared.sync(0);
        shared.write_line(0, b"mine\n").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("theirs\n"), "{text}");
        assert!(
            text.contains("another process rotated daemon.log"),
            "{text}"
        );
        assert!(text.ends_with(" mine\n"), "{text}");
        assert_eq!(
            fs::read_to_string(root.join("daemon.log.1")).unwrap(),
            "old\n"
        );
        assert!(!root.join("daemon.log.2").exists(), "nothing rotated again");
        fs::remove_dir_all(root).unwrap();
    }

    /// R301-001: while another process holds `daemon.log.lock`, nothing is
    /// renamed or trimmed, even a log far over the cap; writing goes on.
    #[test]
    fn a_held_lock_changes_nothing() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-busy");
        let mut shared = managed(&root, &[b'x'; 900], 100, 3);
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("daemon.log.lock"))
            .unwrap();
        let held = Flock::lock(held, FlockArg::LockExclusiveNonblock).unwrap();
        shared.sync(0);
        let text = fs::read_to_string(root.join("daemon.log")).unwrap();
        assert!(text.starts_with(&"x".repeat(900)), "untouched");
        assert!(text.contains("not rotated: another process holds daemon.log.lock"));
        assert!(!root.join("daemon.log.1").exists());
        assert!(!root.join("daemon.log.new").exists());
        drop(held);
        // Released: the next look rotates.
        shared.sync(0);
        assert!(root.join("daemon.log.1").exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// S301-N1 (antelope's repro): symlinks planted at both staging names
    /// never truncate or receive the log; the rotation removes the links and
    /// makes its own files.
    #[test]
    fn planted_staging_links_never_reach_outside_files() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-planted");
        let victim = root.join("victim.txt");
        fs::write(&victim, b"keep these 35 bytes intact, please\n").unwrap();
        let state = root.join("state");
        fs::create_dir(&state).unwrap();
        let mut old = String::new();
        for n in 0..40 {
            old.push_str(&format!("old line {n:03}\n"));
        }
        let mut shared = managed(&state, old.as_bytes(), 100, 3);
        for staged in ["daemon.log.new", "daemon.log.1.new"] {
            std::os::unix::fs::symlink(&victim, state.join(staged)).unwrap();
        }
        shared.sync(0);
        assert_eq!(
            fs::read(&victim).unwrap(),
            b"keep these 35 bytes intact, please\n"
        );
        for name in ["daemon.log", "daemon.log.1"] {
            let kind = fs::symlink_metadata(state.join(name)).unwrap().file_type();
            assert!(kind.is_file(), "{name} is a regular file");
        }
        assert!(
            fs::read_to_string(state.join("daemon.log.1"))
                .unwrap()
                .ends_with("old line 039\n")
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Writing past the cap rotates: newest lines in `daemon.log`, then `.1`,
    /// `.2`, `.3`; nothing past `.3`, no line split, none lost inside the bound.
    #[test]
    fn writes_rotate_through_keep_files_and_drop_the_oldest() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-rotate");
        // A cap 16 times the longest line, as in production (16 MiB), so a
        // rotation renames rather than keeping a tail.
        let mut shared = managed(&root, b"", 1600, 3);
        for n in 0..300 {
            shared
                .write_line(0, format!("line {n:03}\n").as_bytes())
                .unwrap();
        }
        let path = root.join("daemon.log");
        let read = |name: &str| fs::read_to_string(root.join(name)).unwrap_or_default();
        let mut payload = Vec::new();
        for name in ["daemon.log.3", "daemon.log.2", "daemon.log.1", "daemon.log"] {
            let text = read(name);
            assert!(!text.is_empty(), "{name}");
            assert!(
                text.len() < 1600 + 100,
                "{name} stays near the cap: {}",
                text.len()
            );
            for line in text.lines() {
                let (_, rest) = line.split_once(' ').unwrap();
                if let Some(n) = rest.strip_prefix("line ") {
                    payload.push(n.parse::<u32>().unwrap());
                }
            }
        }
        assert!(!root.join("daemon.log.4").exists(), "the oldest is dropped");
        assert!(!root.join("daemon.log.new").exists());
        assert_eq!(payload.last(), Some(&299), "newest last, in daemon.log");
        assert!(
            payload.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "contiguous across files: {payload:?}"
        );
        assert!(payload[0] > 0, "the oldest lines were dropped");
        assert!(
            read("daemon.log.1").contains("rotated at "),
            "a rotation says so"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "a fresh log is private"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A log from before the bound, well over the cap: at start it
    /// becomes `.1` holding only its last `cap` bytes, from a line start.
    #[test]
    fn an_oversized_log_keeps_only_its_tail_at_start() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-oversized");
        // 13-byte lines, so the cut falls mid-line.
        let mut old = String::new();
        for n in 0..100 {
            old.push_str(&format!("old line {n:03}\n"));
        }
        let mut shared = managed(&root, old.as_bytes(), 200, 3);
        shared.rotate_if_due(0);
        let tail = fs::read_to_string(root.join("daemon.log.1")).unwrap();
        assert!(tail.len() <= 200, "{}", tail.len());
        assert!(tail.starts_with("old "), "a line start: {tail:?}");
        assert!(tail.ends_with("old line 099\n"));
        assert!(old.ends_with(&tail));
        assert!(!root.join("daemon.log.1.new").exists());
        let fresh = fs::read_to_string(root.join("daemon.log")).unwrap();
        assert!(fresh.contains("rotated at 1300 bytes"), "{fresh}");
        fs::remove_dir_all(root).unwrap();
    }

    /// A rotation that can't happen changes nothing on disk, keeps writing to
    /// the same file, says so, and waits another cap before trying again.
    #[test]
    fn a_failed_rotation_keeps_writing_and_backs_off() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-failed");
        let mut shared = managed(&root, b"", 200, 3);
        fs::create_dir(root.join("daemon.log.new")).unwrap();
        for n in 0..8 {
            shared
                .write_line(0, format!("line {n}\n").as_bytes())
                .unwrap();
        }
        let text = fs::read_to_string(root.join("daemon.log")).unwrap();
        assert!(text.contains("rotation failed, kept writing"), "{text}");
        assert!(text.contains("line 7"));
        assert!(!root.join("daemon.log.1").exists());
        assert!(
            shared.outputs[0].rotation.as_ref().unwrap().size < 200,
            "the next try is a cap away"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
