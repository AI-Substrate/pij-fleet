//! F241: a one-time sweep of what test fixtures leaked into tmux's shared
//! socket directory for this user (`$TMUX_TMPDIR` or `/tmp`, then
//! `tmux-<uid>`).
//!
//! Before F241 a fixture only killed its server, and tmux leaves the socket
//! pathname behind after that, beside the attach lock pij's control client
//! keeps there (F238). Tens of thousands built up.
//!
//! An entry is removed only when all of these hold. Together they are the
//! **policy**:
//! - its label starts with one of [`FIXTURE_PREFIXES`], the labels pij's own
//!   fixtures mint (never `default`, never any other name);
//! - it is what a fixture leaves: a socket, or a regular lock file with one
//!   link, owned by this user;
//! - no process holds the socket (F245): its file is not the one bound by
//!   any listening socket of this user's processes ([`Holders`]), and for a
//!   lock, no process holds its paired socket either. A refused `connect()`
//!   proves nothing: on Darwin a live server whose listen backlog is full
//!   refuses connections too, so no connect is made at all. When the holders
//!   cannot be read, nothing is removed (fail closed);
//! - for a lock, no process holds it (`flock` is taken without waiting, and
//!   the file is unlinked while it is held). An attach that opened the file
//!   before the unlink would lock the orphaned inode; the attach itself checks
//!   after its `flock` that the path still names that inode, and opens the
//!   new file otherwise (`pij_tmux`'s `lock_attach`, R295-001), so the
//!   unlink never makes a second holder.
//!
//! One **brake**, which can only spare: an entry modified within [`YOUNG`] is
//! left, so a fixture between creating its lock and starting its server is
//! never touched.
//!
//! Per entry the probes are in-process (`lstat`, a set lookup, `flock`). The
//! holders are read once per sweep: on Linux from `/proc/net/unix`
//! in-process, on macOS by one bounded `/usr/sbin/lsof` (no safe in-process
//! API for another process's sockets exists without `unsafe`, which the
//! workspace forbids).

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::tmux::ATTACH_LOCK_SUFFIX;

/// The `-L` labels pij's fixtures mint, and nothing else.
pub const FIXTURE_PREFIXES: [&str; 5] = [
    // pij_testkit::tmux::PrivateTmux
    "pij-test-",
    // crates/tmux/tests/{adapter,control_teardown}.rs and the daemon's
    // pane-observer fixture
    "pij-rs-",
    // crates/tmux/tests/control_mode.rs
    "pij180-",
    // crates/tmux/src/lib.rs's locale test, before it used PrivateTmux
    "pij-utf8-",
    // crates/cli/tests/governance_boot.rs
    "p139-boot-",
];

/// An entry modified more recently than this is spared (the brake).
pub const YOUNG: Duration = Duration::from_secs(10 * 60);

/// A socket, or the attach lock beside one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A server's socket.
    Socket,
    /// pij's attach lock beside a socket.
    Lock,
}

/// What the sweep decided for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// No server answers and nobody holds it: removed by `apply`.
    Stale,
    /// A server answers on the socket (for a lock, on its paired socket).
    Live,
    /// Another process holds the lock.
    Held,
    /// Modified within [`YOUNG`].
    Young,
    /// Not what a fixture leaves (not a socket or a private lock file, or not
    /// ours), or a probe could not tell (anything but a refused connect).
    Unsure,
}

/// One candidate: a fixture-prefixed entry.
#[derive(Debug)]
pub struct Entry {
    /// Its file name in the directory.
    pub name: String,
    /// The fixture prefix its label starts with.
    pub prefix: &'static str,
    /// Socket or lock.
    pub kind: Kind,
    /// What was decided.
    pub verdict: Verdict,
}

/// What a sweep found, and with `apply`, what it removed.
#[derive(Debug, Default)]
pub struct Report {
    /// Every fixture-prefixed entry, in name order.
    pub entries: Vec<Entry>,
    /// Entries ignored: `default`, its lock, and every other name.
    pub ignored: usize,
    /// Stale entries removed (only with `apply`).
    pub removed: usize,
    /// Stale entries that could not be removed, with why.
    pub failed: Vec<(String, String)>,
}

impl Report {
    /// Counts by prefix, kind and verdict.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<(&'static str, Kind, Verdict), usize> {
        let mut counts = BTreeMap::new();
        for entry in &self.entries {
            *counts
                .entry((entry.prefix, entry.kind, entry.verdict))
                .or_insert(0) += 1;
        }
        counts
    }
}

/// Sweep `dir`, judging age against `now` and holding against `holders`.
/// Without `apply` nothing is removed (a lock judged stale is taken and
/// released, never unlinked).
///
/// # Errors
/// When `dir` cannot be read.
pub fn sweep(dir: &Path, now: SystemTime, apply: bool, holders: &Holders) -> io::Result<Report> {
    let mut report = Report::default();
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(dir)? {
        match entry?.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(_) => report.ignored += 1,
        }
    }
    names.sort();
    for name in names {
        let (kind, label) = match name
            .strip_prefix('.')
            .and_then(|rest| rest.strip_suffix(ATTACH_LOCK_SUFFIX))
        {
            Some(label) => (Kind::Lock, label.to_string()),
            None => (Kind::Socket, name.clone()),
        };
        // Only a fixture's label: `default` and every other name match none.
        let Some(prefix) = FIXTURE_PREFIXES
            .into_iter()
            .find(|prefix| label.starts_with(prefix))
        else {
            report.ignored += 1;
            continue;
        };
        let path = dir.join(&name);
        let verdict = match kind {
            Kind::Socket => socket_verdict(&path, now, holders),
            Kind::Lock => lock_verdict(&path, &dir.join(&label), now, apply, holders),
        };
        if apply && verdict == Verdict::Stale && kind == Kind::Socket {
            match fs::remove_file(&path) {
                Ok(()) => report.removed += 1,
                Err(error) => report.failed.push((name.clone(), error.to_string())),
            }
        } else if apply && verdict == Verdict::Stale {
            report.removed += 1;
        }
        report.entries.push(Entry {
            name,
            prefix,
            kind,
            verdict,
        });
    }
    Ok(report)
}

fn young(meta: &fs::Metadata, now: SystemTime) -> bool {
    meta.modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_none_or(|age| age < YOUNG)
}

fn ours(meta: &fs::Metadata) -> bool {
    meta.uid() == pij_core::program::real_uid()
}

/// How long the macOS holder read (`lsof`) may take before it counts as
/// unknown.
pub const HOLDERS_WAIT: Duration = Duration::from_secs(10);

/// F245: which socket files some process holds, as the evidence that a
/// server is there. A refused `connect()` is no such evidence (a live server
/// with a full listen backlog refuses on Darwin).
#[derive(Debug)]
pub enum Holders {
    /// The (device, inode) of the file at every path a listening Unix socket
    /// of this user's processes is bound to.
    Known(HashSet<(u64, u64)>),
    /// The holders could not be read: nothing counts as unheld, so nothing
    /// is removed (fail closed). Why.
    Unknown(String),
}

impl Holders {
    /// Read the holders: `/proc/net/unix` on Linux, in-process; on macOS one
    /// `/usr/sbin/lsof -nP -a -U -u <uid> -F n`, given [`HOLDERS_WAIT`].
    #[must_use]
    pub fn read() -> Self {
        match bound_paths() {
            Ok(paths) => Self::Known(
                paths
                    .iter()
                    .filter_map(|path| fs::metadata(path).ok())
                    .map(|meta| (meta.dev(), meta.ino()))
                    .collect(),
            ),
            Err(why) => Self::Unknown(why),
        }
    }

    /// Does a process hold the socket file `meta` describes? `None` when the
    /// holders are unknown.
    #[must_use]
    pub fn hold(&self, meta: &fs::Metadata) -> Option<bool> {
        match self {
            Self::Known(held) => Some(held.contains(&(meta.dev(), meta.ino()))),
            Self::Unknown(_) => None,
        }
    }
}

/// Every path a Unix socket is bound to, from `/proc/net/unix` (its last
/// column; abstract names, `@…`, are no file).
#[cfg(target_os = "linux")]
fn bound_paths() -> Result<Vec<String>, String> {
    let table =
        fs::read_to_string("/proc/net/unix").map_err(|error| format!("/proc/net/unix: {error}"))?;
    Ok(table
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(7))
        .filter(|path| path.starts_with('/'))
        .map(str::to_string)
        .collect())
}

/// Every path a Unix socket of this user's processes is bound to, from one
/// bounded `lsof`. A non-zero exit, a timeout or unreadable output is
/// unknown.
#[cfg(not(target_os = "linux"))]
fn bound_paths() -> Result<Vec<String>, String> {
    lsof_paths(Path::new("/usr/sbin/lsof"), HOLDERS_WAIT)
}

/// [`bound_paths`] from the `lsof` at `program`, given `wait`.
#[cfg(not(target_os = "linux"))]
fn lsof_paths(program: &Path, wait: Duration) -> Result<Vec<String>, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let mut child = Command::new(program)
        .args(["-nP", "-a", "-U", "-u"])
        .arg(pij_core::program::real_uid().to_string())
        .args(["-F", "n"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    let mut stdout = child.stdout.take().ok_or("lsof: no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + wait;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("lsof did not finish in {wait:?}"));
            }
            Err(error) => return Err(format!("lsof: {error}")),
        }
    };
    let text = reader
        .join()
        .map_err(|_| "lsof: the reader panicked".to_string())?
        .map_err(|error| format!("lsof: {error}"))?;
    if !status.success() {
        return Err(format!("lsof exited {status}"));
    }
    Ok(text
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter(|path| path.starts_with('/'))
        .map(str::to_string)
        .collect())
}

fn socket_verdict(path: &Path, now: SystemTime, holders: &Holders) -> Verdict {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Verdict::Unsure;
    };
    if !meta.file_type().is_socket() || !ours(&meta) {
        return Verdict::Unsure;
    }
    if young(&meta, now) {
        return Verdict::Young;
    }
    match holders.hold(&meta) {
        Some(true) => Verdict::Live,
        Some(false) => Verdict::Stale,
        None => Verdict::Unsure,
    }
}

/// A lock is stale when its socket is gone or no process holds it, and
/// nobody holds the lock. With `apply` it is unlinked while this holds it; an attach that
/// opened it earlier finds the path no longer names it once it locks, and
/// starts again on the new file (R295-001).
fn lock_verdict(
    path: &Path,
    socket: &Path,
    now: SystemTime,
    apply: bool,
    holders: &Holders,
) -> Verdict {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Verdict::Unsure;
    };
    if !meta.is_file() || !ours(&meta) || meta.nlink() != 1 {
        return Verdict::Unsure;
    }
    if young(&meta, now) {
        return Verdict::Young;
    }
    if let Ok(socket) = fs::symlink_metadata(socket) {
        match holders.hold(&socket) {
            Some(false) => {}
            Some(true) => return Verdict::Live,
            None => return Verdict::Unsure,
        }
    }
    let Ok(file) = OpenOptions::new()
        .write(true)
        .custom_flags(pij_core::program::OPEN_NO_FOLLOW)
        .open(path)
    else {
        return Verdict::Unsure;
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Verdict::Held,
        Err(TryLockError::Error(_)) => return Verdict::Unsure,
    }
    if apply && fs::remove_file(path).is_err() {
        return Verdict::Unsure;
    }
    Verdict::Stale
}

#[cfg(test)]
mod tests {
    use super::{Holders, Kind, Report, Verdict, sweep};
    use crate::tmux::ATTACH_LOCK_SUFFIX;
    use std::fs::{self, File};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    static DIRS: AtomicUsize = AtomicUsize::new(0);

    /// A short private directory: a socket path must fit in 104 bytes.
    fn scratch() -> PathBuf {
        let dir = PathBuf::from(format!(
            "/tmp/pij-sweep-{}-{}",
            std::process::id(),
            DIRS.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).expect("scratch dir");
        dir
    }

    /// A socket no server listens on any more, as tmux leaves one.
    fn stale_socket(dir: &Path, label: &str) {
        drop(UnixListener::bind(dir.join(label)).expect("bind"));
    }

    fn lock(dir: &Path, label: &str) -> PathBuf {
        let path = dir.join(format!(".{label}{ATTACH_LOCK_SUFFIX}"));
        File::create(&path).expect("lock file");
        path
    }

    fn verdict(report: &Report, name: &str) -> Option<Verdict> {
        report
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.verdict)
    }

    fn exists(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    /// Only a fixture-prefixed socket no process holds, and a lock whose
    /// socket no process holds and that nobody has locked, are removed; a dry
    /// run removes nothing.
    #[test]
    fn the_sweep_removes_only_stale_fixture_entries() {
        let dir = scratch();
        stale_socket(&dir, "pij-test-1-stale");
        let stale_lock = lock(&dir, "pij-test-1-stale");
        let orphan_lock = lock(&dir, "pij-rs-orphan-1");
        let _live = UnixListener::bind(dir.join("pij-rs-live-1")).expect("bind live");
        let live_lock = lock(&dir, "pij-rs-live-1");
        let held_lock = lock(&dir, "pij180-held-1");
        let holder = File::options().write(true).open(&held_lock).expect("open");
        holder.try_lock().expect("hold the lock");
        stale_socket(&dir, "default");
        let default_lock = lock(&dir, "default");
        stale_socket(&dir, "f237-mine-1");
        let foreign_lock = lock(&dir, "f237-mine-1");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);

        let holders = Holders::read();
        assert!(matches!(holders, Holders::Known(_)), "{holders:?}");
        let dry = sweep(&dir, later, false, &holders).expect("dry run");
        assert_eq!(dry.removed, 0);
        for path in [&stale_lock, &orphan_lock] {
            assert!(
                exists(path),
                "a dry run removes nothing: {}",
                path.display()
            );
        }
        assert!(exists(&dir.join("pij-test-1-stale")));

        let swept = sweep(&dir, later, true, &holders).expect("sweep");
        assert_eq!(verdict(&swept, "pij-test-1-stale"), Some(Verdict::Stale));
        assert_eq!(
            verdict(&swept, ".pij-rs-orphan-1.pij-control-attach.lock"),
            Some(Verdict::Stale)
        );
        assert_eq!(verdict(&swept, "pij-rs-live-1"), Some(Verdict::Live));
        assert_eq!(
            verdict(&swept, ".pij-rs-live-1.pij-control-attach.lock"),
            Some(Verdict::Live)
        );
        assert_eq!(
            verdict(&swept, ".pij180-held-1.pij-control-attach.lock"),
            Some(Verdict::Held)
        );
        assert_eq!(verdict(&swept, "default"), None, "never a candidate");
        assert_eq!(verdict(&swept, "f237-mine-1"), None, "never a candidate");
        assert_eq!(swept.removed, 3);
        assert_eq!(swept.ignored, 4);

        for gone in [dir.join("pij-test-1-stale"), stale_lock, orphan_lock] {
            assert!(!exists(&gone), "stale, removed: {}", gone.display());
        }
        for kept in [
            dir.join("pij-rs-live-1"),
            live_lock,
            held_lock,
            dir.join("default"),
            default_lock,
            dir.join("f237-mine-1"),
            foreign_lock,
        ] {
            assert!(exists(&kept), "spared: {}", kept.display());
        }
        drop(holder);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The brake: an entry touched within `YOUNG` is spared, however stale.
    #[test]
    fn a_young_entry_is_spared() {
        let dir = scratch();
        stale_socket(&dir, "pij-test-2-young");
        let young_lock = lock(&dir, "pij-test-2-young");
        let swept = sweep(&dir, SystemTime::now(), true, &Holders::read()).expect("sweep");
        assert!(
            swept
                .entries
                .iter()
                .all(|entry| entry.verdict == Verdict::Young),
            "{swept:?}"
        );
        assert_eq!(swept.removed, 0);
        assert!(exists(&dir.join("pij-test-2-young")) && exists(&young_lock));
        assert_eq!(
            swept
                .counts()
                .get(&("pij-test-", Kind::Lock, Verdict::Young)),
            Some(&1)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// F245: on Darwin a live listener whose backlog is full refuses
    /// connections (newt's receipt). A refusal is not evidence: the sweep
    /// asks which process holds the socket, so the listener and its lock are
    /// spared.
    #[test]
    fn a_live_listener_that_refuses_connections_is_spared() {
        let dir = scratch();
        let path = dir.join("pij-rs-full-1");
        let _listener = UnixListener::bind(&path).expect("bind");
        let full_lock = lock(&dir, "pij-rs-full-1");
        let mut accepted_never = Vec::new();
        if cfg!(target_os = "macos") {
            let refused = loop {
                match std::os::unix::net::UnixStream::connect(&path) {
                    Ok(stream) => accepted_never.push(stream),
                    Err(error) => break error,
                }
                assert!(accepted_never.len() < 100_000, "the backlog never filled");
            };
            assert_eq!(
                refused.kind(),
                std::io::ErrorKind::ConnectionRefused,
                "a live listener refuses once its backlog is full"
            );
        }
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let swept = sweep(&dir, later, true, &Holders::read()).expect("sweep");
        assert_eq!(verdict(&swept, "pij-rs-full-1"), Some(Verdict::Live));
        assert_eq!(
            verdict(&swept, ".pij-rs-full-1.pij-control-attach.lock"),
            Some(Verdict::Live)
        );
        assert_eq!(swept.removed, 0);
        assert!(exists(&path) && exists(&full_lock));
        drop(accepted_never);
        let _ = fs::remove_dir_all(&dir);
    }

    /// F245, fail closed: with the holders unknown, no socket, and no lock
    /// whose socket is there, counts as unheld.
    #[test]
    fn nothing_is_removed_when_the_holders_are_unknown() {
        let dir = scratch();
        stale_socket(&dir, "pij-test-3-unknown");
        let unknown_lock = lock(&dir, "pij-test-3-unknown");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let unknown = Holders::Unknown("lsof failed".to_string());
        let swept = sweep(&dir, later, true, &unknown).expect("sweep");
        assert_eq!(verdict(&swept, "pij-test-3-unknown"), Some(Verdict::Unsure));
        assert_eq!(
            verdict(&swept, ".pij-test-3-unknown.pij-control-attach.lock"),
            Some(Verdict::Unsure)
        );
        assert_eq!(swept.removed, 0);
        assert!(exists(&dir.join("pij-test-3-unknown")) && exists(&unknown_lock));
        let _ = fs::remove_dir_all(&dir);
    }

    /// F245: the macOS holder read is bounded: an `lsof` that never finishes
    /// is killed at the wait and counts as unknown.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_holder_read_that_hangs_is_unknown_within_its_wait() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let fake = dir.join("lsof");
        fs::write(&fake, "#!/bin/sh\nexec /bin/sleep 30\n").expect("fake lsof");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
        let started = std::time::Instant::now();
        let read = super::lsof_paths(&fake, Duration::from_millis(300));
        assert!(read.is_err(), "{read:?}");
        assert!(
            started.elapsed() < crate::time::scaled(Duration::from_secs(3)),
            "bounded: {:?}",
            started.elapsed()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
