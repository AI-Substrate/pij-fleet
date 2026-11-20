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
//!   even one whose socket is gone. A bound path that won't stat, for any
//!   reason, makes the holders unknown (F248);
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
//! holders are read once per sweep: on Linux in-process, from one scan of
//! `/proc/<pid>/fd` (this user's sockets) and `/proc/net/unix`; on macOS by
//! one `/usr/sbin/lsof`, that launch bounded (no safe in-process API for
//! another process's sockets exists without `unsafe`, which the workspace
//! forbids). The bound is on the launched read; parsing and the `stat` of
//! each bound path come after it, outside it (F247).

use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// Read: the (device, inode) of the file at the path of every listening
    /// Unix socket this user's processes hold.
    Known(HashSet<(u64, u64)>),
    /// The holders could not be read whole and exactly: nothing counts as
    /// unheld, so nothing is removed (fail closed). Why.
    Unknown(String),
}

/// One bound path a process of this user's holds, and that process when it
/// is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    /// The path the socket was bound to, as the kernel kept it.
    pub path: PathBuf,
    /// The process holding it.
    pub pid: Option<u32>,
}

impl Holders {
    /// Read the holders: this user's processes' sockets from `/proc` on
    /// Linux, in-process ([`own_socket_inodes`]); on macOS one
    /// `/usr/sbin/lsof -nP -a -U -u <uid> -F pn`, given [`HOLDERS_WAIT`].
    ///
    /// Each bound path is matched by the (device, inode) of the file it names
    /// now, never by its spelling (`/tmp` and `/private/tmp` are one place).
    /// Every path is read exactly ([`decode_lsof_name`], the remainder of a
    /// `/proc/net/unix` line), and every one must stat, or the whole read is
    /// unknown and nothing is removed (F248). A bound path that won't stat,
    /// for any reason, could name a live socket by a spelling that no longer
    /// resolves (its directory's alias was removed, a link retargeted, a
    /// directory renamed): absence of evidence never licenses a delete.
    #[must_use]
    pub fn read() -> Self {
        match bound_paths() {
            Ok(bound) => Self::of_bound(&bound),
            Err(why) => Self::Unknown(why),
        }
    }

    /// The holders bound at `paths`, holders unknown.
    #[must_use]
    pub fn of(paths: &[PathBuf]) -> Self {
        let bound: Vec<Bound> = paths
            .iter()
            .map(|path| Bound {
                path: path.clone(),
                pid: None,
            })
            .collect();
        Self::of_bound(&bound)
    }

    /// The holders bound at `bound`, each by the file its path names now;
    /// unknown, naming the path and its holder, if any of them won't stat.
    #[must_use]
    pub fn of_bound(bound: &[Bound]) -> Self {
        let mut held = HashSet::with_capacity(bound.len());
        for Bound { path, pid } in bound {
            match fs::metadata(path) {
                Ok(meta) => {
                    held.insert((meta.dev(), meta.ino()));
                }
                Err(error) => {
                    let by = pid.map_or_else(String::new, |pid| format!(" by pid {pid}"));
                    return Self::Unknown(format!(
                        "{} is bound{by} but won't stat ({error}); until that process lets go, nothing can be told unheld",
                        path.display()
                    ));
                }
            }
        }
        Self::Known(held)
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

/// Every path a Unix socket of this user's processes is bound to, from
/// `/proc/net/unix` filtered to the sockets this user's processes hold
/// ([`own_socket_inodes`]), as macOS's `lsof -u <uid>` filters.
#[cfg(target_os = "linux")]
fn bound_paths() -> Result<Vec<Bound>, String> {
    let own = own_socket_inodes(Path::new("/proc"), pij_core::program::real_uid())?;
    let table = fs::read("/proc/net/unix").map_err(|error| format!("/proc/net/unix: {error}"))?;
    Ok(parse_proc_net_unix(&table)?
        .into_iter()
        .filter_map(|(inode, path)| {
            own.get(&inode).map(|&pid| Bound {
                path,
                pid: Some(pid),
            })
        })
        .collect())
}

/// F248: the sockets (sockfs inode, holding pid) this user's processes
/// hold, from each process's `fd` links (`socket:[N]`) under `proc`.
///
/// This filter is a **policy**: without it every user's sockets count, and
/// a path behind another user's directory this user can't search won't
/// stat, which makes the read unknown and spares everything. With it the
/// sweep can decide, so it deletes more; it mirrors macOS's `lsof -u <uid>`.
/// A process counts as another user's only when its `status` positively
/// names no uid of ours (real, effective, saved, filesystem); one whose uid
/// can't be read counts as ours. A process of ours whose `fd` directory or
/// a link in it can't be read makes the read unknown. A process gone
/// mid-scan (`/proc/<pid>` not found) is skipped: its descriptors closed
/// with it, so any socket only it held is no longer held, and one it shared
/// is found through the process it shared it with.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn own_socket_inodes(proc: &Path, uid: u32) -> Result<HashMap<u64, u32>, String> {
    let gone = |error: &io::Error| error.kind() == io::ErrorKind::NotFound;
    let mut own = HashMap::new();
    let entries = fs::read_dir(proc).map_err(|error| format!("{}: {error}", proc.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", proc.display()))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();
        match fs::read_to_string(dir.join("status")) {
            Ok(status) => {
                let uids: Option<Vec<u32>> = status
                    .lines()
                    .find_map(|line| line.strip_prefix("Uid:"))
                    .map(|uids| {
                        uids.split_whitespace()
                            .filter_map(|id| id.parse().ok())
                            .collect()
                    });
                if uids.is_some_and(|uids| !uids.is_empty() && !uids.contains(&uid)) {
                    continue;
                }
            }
            Err(error) if gone(&error) => continue,
            Err(_) => {}
        }
        let fds = match fs::read_dir(dir.join("fd")) {
            Ok(fds) => fds,
            Err(error) if gone(&error) => continue,
            Err(error) => {
                return Err(format!(
                    "{}: {error} (a process of ours, or one whose owner can't be read)",
                    dir.join("fd").display()
                ));
            }
        };
        for fd in fds {
            let fd = fd.map_err(|error| format!("{}: {error}", dir.join("fd").display()))?;
            let target = match fs::read_link(fd.path()) {
                Ok(target) => target,
                Err(error) if gone(&error) => continue,
                Err(error) => return Err(format!("{}: {error}", fd.path().display())),
            };
            if let Some(inode) = target
                .to_str()
                .and_then(|text| text.strip_prefix("socket:["))
                .and_then(|text| text.strip_suffix(']'))
                .and_then(|text| text.parse::<u64>().ok())
            {
                own.insert(inode, pid);
            }
        }
    }
    Ok(own)
}

/// The (sockfs inode, bound path) of every socket in `/proc/net/unix`. A
/// line is `Num: RefCount Protocol Flags Type St Inode [Path]`, and the path
/// is the rest of the line after one space, spaces included; abstract names
/// (`@…`) are no file. The kernel prints a path raw, so a newline in one
/// breaks the next line: any line not of that shape makes the whole read
/// unknown.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_net_unix(table: &[u8]) -> Result<Vec<(u64, PathBuf)>, String> {
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
        let mut inode = 0;
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
                6 => match std::str::from_utf8(token)
                    .ok()
                    .and_then(|text| text.parse().ok())
                {
                    Some(number) => {
                        inode = number;
                        true
                    }
                    None => false,
                },
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
            paths.push((inode, PathBuf::from(std::ffi::OsStr::from_bytes(path))));
        }
    }
    Ok(paths)
}

/// Every path a Unix socket of this user's processes is bound to, and its
/// holder, from one bounded `lsof`. A non-zero exit, a timeout or a name it
/// cannot spell exactly is unknown.
#[cfg(not(target_os = "linux"))]
fn bound_paths() -> Result<Vec<Bound>, String> {
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
fn lsof_paths(program: &Path, wait: Duration) -> Result<Vec<Bound>, String> {
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
        .args(["-F", "pn"])
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
    let mut pid = None;
    for line in text.split(|&byte| byte == b'\n') {
        if let Some(number) = line.strip_prefix(b"p") {
            pid = std::str::from_utf8(number)
                .ok()
                .and_then(|text| text.parse().ok());
            continue;
        }
        let Some(name) = line.strip_prefix(b"n") else {
            continue;
        };
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
        paths.push(Bound {
            path: PathBuf::from(std::ffi::OsString::from_vec(decoded)),
            pid,
        });
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
    use super::{
        Bound, Holders, Kind, Report, Verdict, decode_lsof_name, own_socket_inodes,
        parse_proc_net_unix, sweep,
    };
    use crate::tmux::ATTACH_LOCK_SUFFIX;
    use std::fs::{self, File};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, SystemTime};

    static DIRS: AtomicUsize = AtomicUsize::new(0);

    /// Tests that read this process's real holders, or bind a socket whose
    /// path won't read back (which makes any read unknown), take turns.
    static REAL: Mutex<()> = Mutex::new(());

    fn real() -> MutexGuard<'static, ()> {
        REAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The holders, read for real (lsof or `/proc`), of this test process
    /// alone: another process's socket elsewhere on the machine that won't
    /// stat (F248) must not decide these tests.
    fn own_holders() -> Holders {
        match super::bound_paths() {
            Ok(bound) => Holders::of_bound(
                &bound
                    .into_iter()
                    .filter(|bound| bound.pid == Some(std::process::id()))
                    .collect::<Vec<_>>(),
            ),
            Err(why) => Holders::Unknown(why),
        }
    }

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

        let _real = real();
        let holders = own_holders();
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
        assert_eq!(
            verdict(&swept, "pij-test-1-stale"),
            Some(Verdict::Stale),
            "{holders:?}"
        );
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
        let swept = sweep(&dir, SystemTime::now(), true, &Holders::of(&[])).expect("sweep");
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
        let _real = real();
        let holders = own_holders();
        let swept = sweep(&dir, later, true, &holders).expect("sweep");
        assert_eq!(
            verdict(&swept, "pij-rs-full-1"),
            Some(Verdict::Live),
            "{holders:?}"
        );
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
            Ok(vec![(12345, PathBuf::from("/tmp/tmux 1/pij-test-a b"))])
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
        let _real = real();
        let _live: Vec<UnixListener> = names
            .iter()
            .map(|name| UnixListener::bind(dir.join(name)).expect("bind"))
            .collect();
        stale_socket(&dir, "pij-test-stale");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let holders = own_holders();
        let swept = sweep(&dir, later, true, &holders).expect("sweep");
        for name in names {
            assert!(exists(&dir.join(name)), "{name:?} spared ({holders:?})");
            assert_ne!(verdict(&swept, name), Some(Verdict::Stale), "{name:?}");
        }
        if cfg!(target_os = "macos") {
            // lsof spells every one of these exactly: the read is known and
            // only the stale socket goes.
            assert!(matches!(holders, Holders::Known(_)), "{holders:?}");
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

    /// F248 (bee, R298-R2-001): a bound path that won't stat, for any reason
    /// and wherever it is, makes the holders unknown, and nothing is removed:
    /// one in the swept directory, one behind a directory that can't be
    /// searched, one behind another user's private directory, one that names
    /// nothing anywhere.
    #[test]
    fn a_bound_path_that_wont_stat_makes_the_holders_unknown() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        stale_socket(&dir, "pij-test-4-stale");
        let shut = dir.join("shut");
        fs::create_dir(&shut).expect("dir");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let mut cases = vec![
            dir.join("no-such-socket"),
            shut.join("sock"),
            PathBuf::from("/nonexistent-pij/sock"),
        ];
        for home in ["/root", "/var/root"] {
            if fs::metadata(Path::new(home).join("pij-private"))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
            {
                cases.push(Path::new(home).join("pij-private").join("sock"));
            }
        }
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).expect("chmod");
        let read: Vec<Holders> = cases
            .iter()
            .map(|path| Holders::of(std::slice::from_ref(path)))
            .collect();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o700)).expect("chmod back");
        for (path, holders) in cases.iter().zip(&read) {
            assert!(
                matches!(holders, Holders::Unknown(_)),
                "{}: {holders:?}",
                path.display()
            );
            let swept = sweep(&dir, later, true, holders).expect("sweep");
            assert_eq!(swept.removed, 0, "{}", path.display());
        }
        assert!(exists(&dir.join("pij-test-4-stale")));
        let bound = Holders::of_bound(&[Bound {
            path: dir.join("no-such-socket"),
            pid: Some(4242),
        }]);
        assert!(
            matches!(&bound, Holders::Unknown(why) if why.contains("by pid 4242")),
            "the holder is named: {bound:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// F248 (bee's repro): a live listener bound through a symlinked alias of
    /// its directory, the alias then removed; the kernel keeps the alias
    /// spelling, which resolves to nothing. And one bound in a directory
    /// since renamed. Neither is removed: the holders are unknown.
    #[test]
    fn a_live_listener_whose_bound_spelling_no_longer_resolves_is_spared() {
        let root = scratch();
        let real_dir = root.join("real");
        fs::create_dir(&real_dir).expect("dir");
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&real_dir, &alias).expect("alias");
        let first = root.join("first");
        fs::create_dir(&first).expect("dir");
        let renamed = root.join("renamed");
        let later = SystemTime::now() + Duration::from_secs(24 * 3600);
        let _real = real();
        let via_alias = UnixListener::bind(alias.join("pij-rs-live")).expect("bind via alias");
        let before_rename = UnixListener::bind(first.join("pij-rs-moved")).expect("bind");
        fs::remove_file(&alias).expect("unlink the alias");
        fs::rename(&first, &renamed).expect("rename the directory");
        let holders = own_holders();
        // lsof and /proc/net/unix name the spelling it was bound to.
        assert!(matches!(holders, Holders::Unknown(_)), "{holders:?}");
        let spelled = Holders::of(&[alias.join("pij-rs-live"), first.join("pij-rs-moved")]);
        for holders in [&holders, &spelled] {
            for (dir, name) in [(&real_dir, "pij-rs-live"), (&renamed, "pij-rs-moved")] {
                let swept = sweep(dir, later, true, holders).expect("sweep");
                assert_eq!(swept.removed, 0, "{name}: {holders:?}");
                assert!(exists(&dir.join(name)), "{name} spared");
            }
        }
        drop((via_alias, before_rename));
        let _ = fs::remove_dir_all(&root);
    }

    /// F248: on Linux the holders are this user's processes' sockets, read
    /// from each process's `fd` links. A process of ours holding a socket
    /// counts; one whose `status` positively names another uid is skipped;
    /// one whose owner can't be read counts as ours; an `fd` directory of
    /// ours that can't be read makes the read unknown; a process gone
    /// mid-scan is skipped.
    #[test]
    fn own_sockets_are_read_from_each_process_and_unreadable_ones_fail_closed() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let proc = scratch();
        let uid = 501;
        let process = |pid: u32, status: Option<&str>, fds: &[&str]| {
            let dir = proc.join(pid.to_string());
            fs::create_dir_all(dir.join("fd")).expect("fd dir");
            if let Some(status) = status {
                fs::write(dir.join("status"), status).expect("status");
            }
            for (n, target) in fds.iter().enumerate() {
                symlink(target, dir.join("fd").join(n.to_string())).expect("fd link");
            }
        };
        process(
            10,
            Some("Name:\tx\nUid:\t501\t501\t501\t501\n"),
            &["socket:[111]", "/dev/null"],
        );
        process(11, Some("Name:\ty\nUid:\t0\t0\t0\t0\n"), &["socket:[222]"]);
        process(12, Some("Uid:\t501\n"), &["socket:[333]"]);
        fs::set_permissions(
            proc.join("12").join("status"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("chmod");
        fs::create_dir(proc.join("self-ish")).expect("not a pid");
        let own = own_socket_inodes(&proc, uid).expect("read");
        assert_eq!(own.get(&111), Some(&10), "ours: {own:?}");
        assert_eq!(own.get(&222), None, "another user's: {own:?}");
        assert_eq!(
            own.get(&333),
            Some(&12),
            "owner unreadable counts as ours: {own:?}"
        );
        // Another user's fd directory we can't read is skipped; one of ours
        // is unknown.
        fs::set_permissions(
            proc.join("11").join("fd"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("chmod");
        assert!(own_socket_inodes(&proc, uid).is_ok());
        fs::set_permissions(
            proc.join("10").join("fd"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("chmod");
        let unreadable = own_socket_inodes(&proc, uid);
        assert!(unreadable.is_err(), "{unreadable:?}");
        for pid in ["10", "11"] {
            fs::set_permissions(proc.join(pid).join("fd"), fs::Permissions::from_mode(0o700))
                .expect("chmod back");
        }
        // A process gone mid-scan (its status and fd directory vanish).
        fs::remove_dir_all(proc.join("12")).expect("gone");
        fs::create_dir(proc.join("12")).expect("an empty husk");
        assert_eq!(own_socket_inodes(&proc, uid).expect("read").get(&333), None);
        let _ = fs::remove_dir_all(&proc);
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
        assert_eq!(
            read[0],
            Ok(vec![Bound {
                path: PathBuf::from("/tmp/ok"),
                pid: Some(1),
            }])
        );
        assert!(read[1].is_err(), "{:?}", read[1]);
        let _ = fs::remove_dir_all(&dir);
    }
}
