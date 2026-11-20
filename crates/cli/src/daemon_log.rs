//! Timestamp daemon output once, below Rust's stdout/stderr and panic hooks.
//!
//! The original destinations remain distinct (or share daemon.log when launched
//! by bounce). Only the daemon installs this guard; normal CLI output is untouched.
//!
//! #90: two bounds on what reaches the file.
//! - **Repeats.** A line whose template (the line with every run of ASCII
//!   digits replaced by `#`, so pids, ports, exit codes and counters don't
//!   count) was already written in the last [`REPEAT_WINDOW`] is counted, not
//!   written. When its window closes, one summary line gives the count. At most
//!   [`REPEAT_KEYS`] templates are remembered.
//! - **Size.** When the destination is the state dir's own `daemon.log` (the
//!   same file by device and inode, never a symlink), the daemon rotates it
//!   itself once it reaches [`ROTATE_AT`]: `daemon.log.1` … `daemon.log.{KEEP}`,
//!   the oldest dropped. Every launcher (`just bounce-rs`'s `>>`, the
//!   LaunchAgent's `StandardOutPath`, `pij-rs daemon bounce`) only opens the file
//!   once before exec, so after a rotation the daemon writes to a file it opened
//!   by path. A file more than a sixteenth over the cap (a log from before #90)
//!   keeps only its last [`ROTATE_AT`] bytes, from a line start. Any other
//!   destination (a terminal, another file) is never rotated. All of it is
//!   in-process: no launch.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
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
        // The cap at start: a log already at or over the bound rotates now.
        for index in 0..shared.outputs.len() {
            shared.rotate_if_due(index);
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
            "pij-rs: daemon log: {} more like this in {} s after it was written: {}\n",
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

    /// Rotate a managed output at the cap. A rotation that fails keeps the
    /// current file, says so in it, and is next tried a further cap later.
    /// Its note is written straight to the file, never back through here.
    fn rotate_if_due(&mut self, output: usize) {
        let target = &mut self.outputs[output];
        let Some(rotation) = &mut target.rotation else {
            return;
        };
        if rotation.size < rotation.cap {
            return;
        }
        let rotated = rotation.size;
        let note = match rotation.rotate() {
            Ok(file) => {
                target.file = file;
                let name = rotation
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                format!(
                    "pij-rs: daemon log: rotated at {rotated} bytes; older lines are in {name}.1\n"
                )
            }
            Err(error) => {
                format!("pij-rs: daemon log: rotation failed, kept writing: {error}\n")
            }
        };
        rotation.size = 0;
        if let Ok(bytes) = stamp(note.as_bytes())
            && target.file.write_all(&bytes).is_ok()
        {
            let _ = target.file.flush();
            rotation.size = bytes.len() as u64;
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

impl Rotation {
    /// Manage `path` only if `file` is that very file (device and inode) and
    /// the path is a regular file, not a symlink.
    fn adopt(file: &File, path: &Path, cap: u64, keep: usize) -> Option<Self> {
        let open = file.metadata().ok()?;
        // `symlink_metadata`: a symlink is its own inode, so it never matches.
        let named = std::fs::symlink_metadata(path).ok()?;
        (named.dev() == open.dev() && named.ino() == open.ino()).then(|| Self {
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

    /// Shift `.1`… up one (the last is replaced), move the current file to
    /// `.1`, and open a fresh file at the path. The fresh file is made first,
    /// so a failure before the shift changes nothing.
    fn rotate(&mut self) -> io::Result<File> {
        let fresh_path = self.suffixed(".new");
        let fresh = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&fresh_path)?;
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
    fn keep_tail(&self) -> io::Result<()> {
        let mut source = BufReader::new(File::open(&self.path)?);
        let length = source.get_ref().metadata()?.len();
        source.seek(SeekFrom::Start(length.saturating_sub(self.cap)))?;
        let mut partial = Vec::new();
        source.read_until(b'\n', &mut partial)?;
        let staged = self.suffixed(".1.new");
        let mut target = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&staged)?;
        io::copy(&mut source, &mut target)?;
        target.flush()?;
        std::fs::rename(&staged, self.numbered(1))?;
        std::fs::remove_file(&self.path)
    }
}

/// The template a line is counted by: trailing line ending dropped and every
/// run of ASCII digits replaced by `#`.
fn template(line: &[u8]) -> Vec<u8> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut template = Vec::with_capacity(line.len());
    let mut in_digits = false;
    for &byte in line {
        if byte.is_ascii_digit() {
            if !in_digits {
                template.push(b'#');
            }
            in_digits = true;
        } else {
            template.push(byte);
            in_digits = false;
        }
    }
    template
}

#[derive(Debug, PartialEq, Eq)]
enum Admit {
    /// Write the line, after these summaries (a window that ended, or a
    /// template forgotten to make room).
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

/// Repeated-line memory: template hash → its open window.
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

    fn admit(&mut self, line: &[u8], output: usize, now: Instant) -> Admit {
        let template = template(line);
        let mut hasher = DefaultHasher::new();
        template.hash(&mut hasher);
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
            seen.output = output;
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
        let mut shown = String::from_utf8_lossy(&template).into_owned();
        if shown.len() > TEMPLATE_SHOWN {
            let mut end = TEMPLATE_SHOWN;
            while !shown.is_char_boundary(end) {
                end -= 1;
            }
            shown.truncate(end);
            shown.push('…');
        }
        self.seen.insert(
            key,
            Seen {
                until: now + self.window,
                count: 0,
                output,
                shown,
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
            for n in 0..500 {
                writeln!(writer, "pane observer: list-panes failed (exit {})", n % 3).unwrap();
            }
            writer.write_all(b"daemon: shutting down\n").unwrap();
            writer.shutdown(Shutdown::Write).unwrap();
        });
        pump(reader, &shared, 0).unwrap();
        feeder.join().unwrap();
        assert_eq!(
            payloads(&fs::read(&path).unwrap()),
            [
                b"pane observer: list-panes failed (exit 0)\n".to_vec(),
                b"daemon: shutting down\n".to_vec(),
                b"pij-rs: daemon log: 499 more like this in 60 s after it was written: pane observer: list-panes failed (exit #)\n".to_vec(),
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
        for n in 0..3 {
            writeln!(writer, "burst {n}").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !fs::read_to_string(&path)
            .unwrap()
            .contains("2 more like this")
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
        writeln!(writer, "burst 3").unwrap();
        writeln!(writer, "burst 4").unwrap();
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
            .rposition(|line| line.contains("1 more like this"));
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
    /// tick while tmux's default server was gone (2026-11-07), whose exit code
    /// and socket uid are digits. Each burst writes its first line, then one
    /// summary per minute with the exact count.
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
                Box::new(|n: u64| {
                    format!(
                        "pij-rs pane observer: tmux: list-panes failed (exit {}): error connecting to /private/tmp/tmux-501/default (No such file or directory) — run `tmux list-panes -a` to verify the target\n",
                        1 + n % 2
                    )
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
    fn a_window_counts_only_the_same_template_and_reports_when_it_closes() {
        let start = Instant::now();
        let mut repeats = Repeats::new(Duration::from_secs(60), 8);
        assert_eq!(
            repeats.admit(b"seat a pid 41\n", 0, start),
            Admit::Write(Vec::new())
        );
        assert!(!repeats.counting(), "nothing counted, so no timer");
        assert_eq!(repeats.admit(b"seat a pid 42\n", 0, start), Admit::Count);
        assert!(repeats.counting());
        // Words are not digits: another seat is its own line.
        assert_eq!(
            repeats.admit(b"seat b pid 41\n", 1, start),
            Admit::Write(Vec::new())
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
                template: "seat a pid #".into(),
            }]
        );
        assert!(!repeats.counting());
        // After its window the template is written again.
        assert_eq!(
            repeats.admit(b"seat a pid 43\n", 0, start + Duration::from_secs(61)),
            Admit::Write(Vec::new())
        );
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

    #[test]
    fn templates_replace_digit_runs_and_shown_templates_are_bounded() {
        assert_eq!(
            template(b"exit 1 on tmux-501 at 2026\r\n"),
            b"exit # on tmux-# at #"
        );
        let mut repeats = Repeats::new(Duration::from_secs(60), 2);
        let long = format!("{}\n", "é".repeat(400));
        repeats.admit(long.as_bytes(), 0, Instant::now());
        repeats.admit(long.as_bytes(), 0, Instant::now());
        let summary = repeats.drain().pop().unwrap();
        assert!(summary.template.len() <= TEMPLATE_SHOWN + '…'.len_utf8());
        assert!(summary.template.ends_with('…'));
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
