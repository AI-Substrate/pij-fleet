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
//!   any listening socket of this user's processes ([`Holders`]), matched by
//!   device and inode, never by spelling, and for a lock, no process holds
//!   its paired socket either. A refused `connect()` proves nothing: on
//!   Darwin a live server whose listen backlog is full refuses connections
//!   too, so no connect is made at all. When the holders cannot be read
//!   exactly and whole, nothing is removed (fail closed): locks included,
//!   even one whose socket is gone. A bound path that now names no file
//!   spares the directory it was in ([`Holders::unknown_in`]);
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
use std::path::{Path, PathBuf};
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
    let scoped;
    let holders = match holders.unknown_in(dir) {
        Some(why) if !matches!(holders, Holders::Unknown(_)) => {
            scoped = Holders::Unknown(why);
            &scoped
        }
        _ => holders,
    };
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
    /// Read: every listening Unix socket of this user's processes, by the
    /// file its path names now.
    Known {
        /// The (device, inode) of the file at each bound path.
        held: HashSet<(u64, u64)>,
        /// Each bound path that won't stat, and why.
        unresolved: Vec<(PathBuf, io::ErrorKind)>,
    },
    /// The holders could not be read: nothing counts as unheld, so nothing
    /// is removed (fail closed). Why.
    Unknown(String),
}

impl Holders {
    /// Read the holders: `/proc/net/unix` on Linux, in-process; on macOS one
    /// `/usr/sbin/lsof -nP -a -U -u <uid> -F n`, given [`HOLDERS_WAIT`].
    ///
    /// Each bound path is matched by the (device, inode) of the file it names
    /// now, never by its spelling (`/tmp` and `/private/tmp` are one place).
    /// Every path is read exactly ([`decode_lsof_name`], the remainder of a
    /// `/proc/net/unix` line), or the whole read is unknown and nothing is
    /// removed (#298 round 2, bee and sawfish). A path that won't stat (its
    /// socket file was unlinked, it lives in another mount namespace, or, on
    /// Linux, where every user's sockets are listed, it is in a directory
    /// this user can't search) is kept as `unresolved`: it makes unknown the
    /// directory it may be in ([`Holders::unknown_in`]), never silently
    /// dropped.
    #[must_use]
    pub fn read() -> Self {
        match bound_paths() {
            Ok(paths) => Self::of(&paths),
            Err(why) => Self::Unknown(why),
        }
    }

    /// The holders bound at `paths`, each by the file it names now; every
    /// path that won't stat is kept, unresolved.
    #[must_use]
    pub fn of(paths: &[PathBuf]) -> Self {
        let mut held = HashSet::with_capacity(paths.len());
        let mut unresolved = Vec::new();
        for path in paths {
            match fs::metadata(path) {
                Ok(meta) => {
                    held.insert((meta.dev(), meta.ino()));
                }
                Err(error) => unresolved.push((path.clone(), error.kind())),
            }
        }
        Self::Known { held, unresolved }
    }

    /// Why nothing in `dir` may count as unheld, if anything: the holders are
    /// unknown, `dir` itself can't be looked at, or a bound path that won't
    /// stat may be in `dir` (its parent is `dir` by device and inode, or its
    /// parent can't be looked at either). Such a path could be an entry of
    /// `dir` this user can't see as bound, so `dir` is spared whole (#298
    /// round 2, bee and sawfish: fail closed in that run, or that directory).
    /// One whose parent is another directory names no entry of `dir`.
    #[must_use]
    pub fn unknown_in(&self, dir: &Path) -> Option<String> {
        let unresolved = match self {
            Self::Unknown(why) => return Some(why.clone()),
            Self::Known { unresolved, .. } => unresolved,
        };
        let here = match fs::metadata(dir) {
            Ok(meta) => (meta.dev(), meta.ino()),
            Err(error) => return Some(format!("{}: {error}", dir.display())),
        };
        for (path, why) in unresolved {
            let parent = path.parent().unwrap_or(Path::new("/"));
            match fs::metadata(parent) {
                Ok(meta) if (meta.dev(), meta.ino()) == here => {
                    return Some(format!(
                        "{} is bound but won't stat ({why:?})",
                        path.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Some(format!(
                        "{}, bound, can't be placed: {error}",
                        path.display()
                    ));
                }
            }
        }
        None
    }

    /// Does a process hold the socket file `meta` describes? `None` when the
    /// holders are unknown.
    #[must_use]
    pub fn hold(&self, meta: &fs::Metadata) -> Option<bool> {
        match self {
            Self::Known { held, .. } => Some(held.contains(&(meta.dev(), meta.ino()))),
            Self::Unknown(_) => None,
        }
    }
}

/// Every path a Unix socket is bound to, from `/proc/net/unix`. A line is
/// `Num: RefCount Protocol Flags Type St Inode [Path]`, and the path is the
/// rest of the line after one space, spaces included; abstract names (`@…`)
/// are no file. The kernel prints a path raw, so a newline in one breaks the
/// next line: any line not of that shape makes the whole read unknown.
#[cfg(target_os = "linux")]
fn bound_paths() -> Result<Vec<PathBuf>, String> {
    let table = fs::read("/proc/net/unix").map_err(|error| format!("/proc/net/unix: {error}"))?;
    parse_proc_net_unix(&table)
}

/// [`bound_paths`] from the text of `/proc/net/unix`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_net_unix(table: &[u8]) -> Result<Vec<PathBuf>, String> {
    use std::os::unix::ffi::OsStrExt;
    let mut paths = Vec::new();
    for line in table.split(|&byte| byte == b'\n').skip(1) {
        if line.is_empty() {
            continue;
        }
        let unreadable = || {
            format!(
                "/proc/net/unix: a line it could not read: {:?}",
                String::from_utf8_lossy(line)
            )
        };
        let mut rest = line;
        for field in 0..7 {
            let start = rest
                .iter()
                .position(|&byte| byte != b' ')
                .ok_or_else(unreadable)?;
            rest = &rest[start..];
            let end = rest
                .iter()
                .position(|&byte| byte == b' ')
                .unwrap_or(rest.len());
            let token = &rest[..end];
            let hex = |text: &[u8]| !text.is_empty() && text.iter().all(u8::is_ascii_hexdigit);
            let fits = match field {
                0 => token.len() > 1 && token.ends_with(b":") && hex(&token[..token.len() - 1]),
                6 => token.iter().all(u8::is_ascii_digit),
                _ => hex(token),
            };
            if !fits {
                return Err(unreadable());
            }
            rest = &rest[end..];
        }
        let Some(path) = rest.strip_prefix(b" ") else {
            continue;
        };
        if path.starts_with(b"/") {
            paths.push(PathBuf::from(std::ffi::OsStr::from_bytes(path)));
        }
    }
    Ok(paths)
}

/// Every path a Unix socket of this user's processes is bound to, from one
/// bounded `lsof`. A non-zero exit, a timeout or a name it cannot spell
/// exactly is unknown.
#[cfg(not(target_os = "linux"))]
fn bound_paths() -> Result<Vec<PathBuf>, String> {
    lsof_paths(Path::new("/usr/sbin/lsof"), HOLDERS_WAIT)
}

/// One name from `lsof -F n`, decoded exactly, or `None` when it cannot be
/// (#298 round 2, sawfish). lsof prints a non-printable byte as `\b`, `\f`,
/// `\n`, `\r`, `\t`, `\xHH` or `^X`, and a backslash as `\\`; under `LC_ALL=C`
/// every byte above 0x7f is `\xHH`. `^` itself prints as `^`, so `^A` may be
/// byte 1 or those two characters: any `^`, and any other escape, is `None`.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn decode_lsof_name(name: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(name.len());
    let mut bytes = name.iter().copied();
    while let Some(byte) = bytes.next() {
        match byte {
            b'^' => return None,
            b'\\' => decoded.push(match bytes.next()? {
                b'\\' => b'\\',
                b'b' => 0x08,
                b'f' => 0x0c,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'x' => {
                    let high = char::from(bytes.next()?).to_digit(16)?;
                    let low = char::from(bytes.next()?).to_digit(16)?;
                    u8::try_from(high * 16 + low).ok()?
                }
                _ => return None,
            }),
            _ => decoded.push(byte),
        }
    }
    Some(decoded)
}

/// [`bound_paths`] from the `lsof` at `program`, all of it within `wait`
/// (#298 round 2, snail): the spawn, the read of its output, its exit and,
/// past nine tenths of `wait`, its kill and reaping. Anything not done by
/// then is unknown. A child that outlives its kill is left unreaped, and a
/// reader still blocked (a grandchild holding the pipe) is left to end at
/// EOF: neither delays the answer.
#[cfg(not(target_os = "linux"))]
fn lsof_paths(program: &Path, wait: Duration) -> Result<Vec<PathBuf>, String> {
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;
    let started = Instant::now();
    let deadline = started + wait;
    let kill_at = started + wait * 9 / 10;
    let mut child = Command::new(program)
        .args(["-nP", "-a", "-U", "-u"])
        .arg(pij_core::program::real_uid().to_string())
        .args(["-F", "n"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    let mut stdout = child.stdout.take().ok_or("lsof: no stdout")?;
    let (sent, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sent.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });
    let poll = Duration::from_millis(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < kill_at => std::thread::sleep(poll),
            Ok(None) => {
                let _ = child.kill();
                while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                    std::thread::sleep(poll);
                }
                return Err(format!("lsof did not finish in {wait:?}"));
            }
            Err(error) => return Err(format!("lsof: {error}")),
        }
    };
    let text = output
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| format!("lsof's output did not end in {wait:?}"))?
        .map_err(|error| format!("lsof: {error}"))?;
    if !status.success() {
        return Err(format!("lsof exited {status}"));
    }
    let mut paths = Vec::new();
    for name in text
        .split(|&byte| byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"n"))
    {
        // A path; the rest name a peer (`->0x…`, `->(none)`), not a file.
        if !name.starts_with(b"/") {
            continue;
        }
        let decoded = decode_lsof_name(name).ok_or_else(|| {
            format!(
                "lsof named a socket it could not spell exactly: {}",
                String::from_utf8_lossy(name)
            )
        })?;
        paths.push(PathBuf::from(std::ffi::OsString::from_vec(decoded)));
    }
    Ok(paths)
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
    // Unknown holders remove nothing, a lock whose socket is gone included
    // (#298 round 2, bee). Its socket is absent only when the lookup says so;
    // any other failure to look is unsure.
    if matches!(holders, Holders::Unknown(_)) {
        return Verdict::Unsure;
    }
    match fs::symlink_metadata(socket) {
        Ok(socket) => match holders.hold(&socket) {
            Some(false) => {}
            Some(true) => return Verdict::Live,
            None => return Verdict::Unsure,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Verdict::Unsure,
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
    use super::{Holders, Kind, Report, Verdict, decode_lsof_name, parse_proc_net_unix, sweep};
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
        assert!(matches!(holders, Holders::Known { .. }), "{holders:?}");
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

    /// F245, fail closed: with the holders unknown, nothing is removed: no
    /// socket, no lock whose socket is there, and (#298 round 2, bee) no lock
    /// whose socket is gone either.
    #[test]
    fn nothing_is_removed_when_the_holders_are_unknown() {
        let dir = scratch();
        stale_socket(&dir, "pij-test-3-unknown");
        let unknown_lock = lock(&dir, "pij-test-3-unknown");
        let orphan_lock = lock(&dir, "pij-test-3-orphan");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let unknown = Holders::Unknown("lsof failed".to_string());
        let swept = sweep(&dir, later, true, &unknown).expect("sweep");
        assert_eq!(verdict(&swept, "pij-test-3-unknown"), Some(Verdict::Unsure));
        for name in [
            ".pij-test-3-unknown.pij-control-attach.lock",
            ".pij-test-3-orphan.pij-control-attach.lock",
        ] {
            assert_eq!(verdict(&swept, name), Some(Verdict::Unsure), "{name}");
        }
        assert_eq!(swept.removed, 0);
        assert!(exists(&dir.join("pij-test-3-unknown")) && exists(&unknown_lock));
        assert!(exists(&orphan_lock));
        let _ = fs::remove_dir_all(&dir);
    }

    /// #298 round 2 (sawfish): lsof's names decode exactly, and one that
    /// can't be (a `^`, which is ambiguous, or an escape lsof never writes)
    /// is `None`, which makes the whole read unknown.
    #[test]
    fn lsof_names_decode_exactly_or_not_at_all() {
        let cases: [(&[u8], Option<&[u8]>); 10] = [
            (b"/t/plain", Some(b"/t/plain")),
            (br"/t/a\nb", Some(b"/t/a\nb")),
            (br"/t/a\tb", Some(b"/t/a\tb")),
            (br"/t/back\\nslash", Some(br"/t/back\nslash")),
            (br"/t/\xc3\xbc", Some("/t/\u{fc}".as_bytes())),
            (br"/t/\b\f\r", Some(b"/t/\x08\x0c\r")),
            (b"/t/sp ace", Some(b"/t/sp ace")),
            (b"/t/c^Ad", None),
            (br"/t/q\q", None),
            (br"/t/x\x4", None),
        ];
        for (name, decoded) in cases {
            assert_eq!(
                decode_lsof_name(name).as_deref(),
                decoded,
                "{}",
                String::from_utf8_lossy(name)
            );
        }
        assert_eq!(decode_lsof_name(br"/t/end\"), None, "a trailing backslash");
    }

    /// #298 round 2 (bee): `/proc/net/unix`'s path is the rest of the line,
    /// spaces included; a line not of the table's shape (a path's newline
    /// splits a line) makes the whole read unknown.
    #[test]
    fn proc_net_unix_paths_keep_their_spaces_and_a_broken_line_is_unknown() {
        let header = "Num       RefCount Protocol Flags    Type St Inode Path\n";
        let table = format!(
            "{header}\
             0000000000000000: 00000002 00000000 00010000 0001 01 12345 /tmp/tmux 1/pij-test-a b\n\
             0000000000000000: 00000002 00000000 00000000 0001 03  6789\n\
             0000000000000000: 00000002 00000000 00010000 0001 01 23456 @abstract\n"
        );
        assert_eq!(
            parse_proc_net_unix(table.as_bytes()),
            Ok(vec![PathBuf::from("/tmp/tmux 1/pij-test-a b")])
        );
        for tail in ["b", "x y z w v u t /tmp/elsewhere"] {
            let broken = format!(
                "{header}0000000000000000: 00000002 00000000 00010000 0001 01 12345 /tmp/a\n{tail}\n"
            );
            assert!(parse_proc_net_unix(broken.as_bytes()).is_err(), "{tail}");
        }
    }

    /// #298 round 2 (sawfish, bee): a live listener is spared whatever bytes
    /// its name holds (a newline, a tab, a backslash, a space), and one bound
    /// through another spelling of the directory (bee's `/tmp` and
    /// `/private/tmp`) is matched by device and inode.
    #[test]
    fn live_listeners_are_spared_whatever_their_names_hold() {
        let dir = scratch();
        let names = [
            "pij-test-a\nb",
            "pij-test-t\tb",
            "pij-test-back\\b",
            "pij-test-s b",
        ];
        let _live: Vec<UnixListener> = names
            .iter()
            .map(|name| UnixListener::bind(dir.join(name)).expect("bind"))
            .collect();
        stale_socket(&dir, "pij-test-stale");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let holders = Holders::read();
        let swept = sweep(&dir, later, true, &holders).expect("sweep");
        for name in names {
            assert!(exists(&dir.join(name)), "{name:?} spared ({holders:?})");
            assert_ne!(verdict(&swept, name), Some(Verdict::Stale), "{name:?}");
        }
        if cfg!(target_os = "macos") {
            // lsof spells every one of these exactly: the read is known and
            // only the stale socket goes.
            assert!(matches!(holders, Holders::Known { .. }), "{holders:?}");
            assert_eq!(verdict(&swept, "pij-test-stale"), Some(Verdict::Stale));
        }
        // Another spelling of the same directory, through a link to it.
        let alias = PathBuf::from(format!("{}-alias", dir.display()));
        let _ = fs::remove_file(&alias);
        std::os::unix::fs::symlink(&dir, &alias).expect("alias");
        stale_socket(&dir, "pij-test-aliased");
        let by_alias = Holders::of(&[alias.join("pij-test-aliased")]);
        let swept = sweep(&dir, later, true, &by_alias).expect("sweep");
        assert_eq!(verdict(&swept, "pij-test-aliased"), Some(Verdict::Live));
        assert!(exists(&dir.join("pij-test-aliased")));
        let _ = fs::remove_file(&alias);
        let _ = fs::remove_dir_all(&dir);
    }

    /// #298 round 2 (bee): a bound path that won't stat is never dropped:
    /// it spares the directory it may be in, the swept one by device and
    /// inode or one that can't be looked at either. One in another directory
    /// (a socket of another user's, behind a directory this user can't
    /// search, as on Linux CI) names no entry here and changes nothing.
    #[test]
    fn a_bound_path_that_wont_stat_spares_its_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        stale_socket(&dir, "pij-test-4-stale");
        let shut = dir.join("shut");
        fs::create_dir(&shut).expect("dir");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let removed = |holders: &Holders| {
            fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).expect("chmod");
            let swept = sweep(&dir, later, true, holders).expect("sweep");
            fs::set_permissions(&shut, fs::Permissions::from_mode(0o700)).expect("chmod back");
            swept.removed
        };
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).expect("chmod");
        let in_dir = Holders::of(&[dir.join("no-such-socket")]);
        let unplaced = Holders::of(&[shut.join("inner").join("sock")]);
        let elsewhere = Holders::of(&[shut.join("sock"), PathBuf::from("/nonexistent-pij/sock")]);
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o700)).expect("chmod back");
        for holders in [&in_dir, &unplaced, &elsewhere] {
            assert!(
                matches!(holders, Holders::Known { held, .. } if held.is_empty()),
                "{holders:?}"
            );
        }
        assert_eq!(removed(&in_dir), 0, "{in_dir:?}");
        assert_eq!(removed(&unplaced), 0, "{unplaced:?}");
        assert!(exists(&dir.join("pij-test-4-stale")));
        assert_eq!(removed(&elsewhere), 1, "{elsewhere:?}");
        assert!(!exists(&dir.join("pij-test-4-stale")));
        let _ = fs::remove_dir_all(&dir);
    }

    /// F245: the macOS holder read is bounded as a whole (#298 round 2): an
    /// `lsof` that never finishes, or one that exits while a child of its own
    /// keeps the output pipe open, counts as unknown within the wait.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_holder_read_that_hangs_is_unknown_within_its_wait() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        for (name, body) in [
            ("never finishes", "exec /bin/sleep 30"),
            (
                "leaves its pipe held",
                "/bin/sleep 10 &\necho n/tmp/x\nexit 0",
            ),
        ] {
            let fake = dir.join("lsof");
            fs::write(&fake, format!("#!/bin/sh\n{body}\n")).expect("fake lsof");
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
            let started = std::time::Instant::now();
            let read = super::lsof_paths(&fake, Duration::from_millis(300));
            let took = started.elapsed();
            assert!(read.is_err(), "{name}: {read:?}");
            assert!(
                took < crate::time::scaled(Duration::from_secs(2)),
                "{name}: bounded, took {took:?}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// #298 round 2 (sawfish): one name lsof can't spell exactly (`^A` is
    /// byte 1 or two characters) makes the whole read unknown, however many
    /// others read cleanly; peers (`->0x…`) are no path and are skipped.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_name_lsof_cannot_spell_exactly_makes_the_read_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let fake = dir.join("lsof");
        let mut read = Vec::new();
        for body in [
            "printf 'p1\\nf3\\nn/tmp/ok\\nf4\\nn->0x1\\n'",
            "printf 'p1\\nf3\\nn/tmp/ok\\nf4\\nn/tmp/a^Ab\\n'",
        ] {
            fs::write(&fake, format!("#!/bin/sh\n{body}\n")).expect("fake lsof");
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
            read.push(super::lsof_paths(&fake, Duration::from_secs(5)));
        }
        assert_eq!(read[0], Ok(vec![PathBuf::from("/tmp/ok")]));
        assert!(read[1].is_err(), "{:?}", read[1]);
        let _ = fs::remove_dir_all(&dir);
    }
}
