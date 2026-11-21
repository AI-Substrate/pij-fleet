//! A private tmux server per test, so a test never touches the operator's.
//!
//! A test daemon with real adapters lists panes and attaches a control client
//! (plan 180). Pointed at the default server, that is the operator's tmux
//! (pij-fleet#73 review S3). Name this server in `Config::tmux_server` or
//! `PIJ_TMUX_SERVER` instead; it is killed when the value drops.
//!
//! F241: tmux leaves a server's socket pathname behind after `kill-server`
//! (3.4 and 3.6a), and pij's control client leaves its attach lock beside it
//! (F238). Both sit in tmux's one shared directory for this user, so a
//! fixture that only kills its server leaks two entries per test there.
//! Every fixture removes both on its way out, panicking or not, once its
//! server's own process is confirmed gone ([`finish_server`]); a server that
//! outlived its `kill-server` keeps them, and is reported.
//!
//! F245: "gone" is the server's pid having exited, never a refused
//! `connect()` or a failing tmux command: on Darwin a live server with a full
//! listen backlog refuses connections (tmux then says `no server running`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// The name of the attach lock pij's control client keeps beside a server's
/// socket, after the sanitized label: `.<label>` + this (F238).
pub const ATTACH_LOCK_SUFFIX: &str = ".pij-control-attach.lock";

/// tmux's directory for this user's sockets: `$TMUX_TMPDIR` (or `/tmp`),
/// resolved, then `tmux-<uid>`, as tmux and `pij_tmux::socket_path` build it.
#[must_use]
pub fn socket_dir() -> PathBuf {
    let dir = std::env::var_os("TMUX_TMPDIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    dir.join(format!("tmux-{}", pij_core::program::real_uid()))
}

/// The socket of the server tmux runs for `-L label`.
#[must_use]
pub fn socket_path(label: &str) -> PathBuf {
    socket_dir().join(label)
}

/// The attach lock pij keeps beside `socket`, named as
/// `pij_tmux::socket_attach_lock_path` names it.
#[must_use]
pub fn attach_lock_path(socket: &Path) -> PathBuf {
    let name = socket
        .file_name()
        .map_or_else(|| "default".into(), |name| name.to_string_lossy());
    let label: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    socket.with_file_name(format!(".{label}{ATTACH_LOCK_SUFFIX}"))
}

/// How long a killed server's process is given to exit, before scaling: the
/// wait ends the moment it has, so a long bound costs nothing.
///
/// F263: `kill-server` returns once the server has the command, not once it
/// has exited. The server then kills its panes and tears down; it runs at the
/// test's niceness (19), so under a loaded machine that took over the old
/// fixed 2 s, and the fixture failed a passing test (a fixture suite took 31 s
/// at load 95 with this wait).
pub const SERVER_GONE_BUDGET: Duration = Duration::from_secs(60);

/// [`SERVER_GONE_BUDGET`] stretched by `PIJ_TEST_TIME_SCALE`.
#[must_use]
pub fn server_gone_wait() -> Duration {
    crate::time::scaled(SERVER_GONE_BUDGET)
}

/// The pid of the server tmux runs for `-L label`, as the server reports it
/// (`#{pid}`). `None` when it does not answer, which proves nothing about
/// whether it runs (F245).
#[must_use]
pub fn server_pid(label: &str) -> Option<u32> {
    let output = Command::new("tmux")
        .args(["-L", label, "display-message", "-p", "#{pid}"])
        .env_remove("TMUX")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Why a fixture's files were kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// The server's process `pid` was still running after the wait
    /// ([`server_gone_wait`]): its `kill-server` failed or it has not exited.
    Running(u32),
    /// The server's pid was never learned, or its state could not be read:
    /// nothing proves it gone, so nothing is removed (fail closed).
    Unknown,
}

impl std::fmt::Display for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running(pid) => write!(f, "its server (pid {pid}) is still running"),
            Self::Unknown => f.write_str("nothing proves its server gone"),
        }
    }
}

/// Poll whether process `pid` has exited, in-process, for at most `wait`.
fn exited(pid: u32, wait: Duration) -> Result<(), Kept> {
    let deadline = Instant::now() + wait;
    loop {
        match pij_core::program::process_exists(pid) {
            Some(false) => return Ok(()),
            None => return Err(Kept::Unknown),
            Some(true) if Instant::now() >= deadline => return Err(Kept::Running(pid)),
            Some(true) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// F241/F245: remove what a fixture's killed server leaves in tmux's shared
/// directory, its socket and pij's attach lock beside it, but only once the
/// server's process `pid` has exited, waiting up to [`server_gone_wait`]. A
/// server still running, or one whose pid is unknown while its socket is
/// there, keeps both: unlinking a live server's socket would orphan it and
/// make it look gone (R295-002). With no socket (a server never started)
/// there is nothing to orphan.
///
/// # Errors
/// [`Kept`] when the server is not proved gone; nothing is removed.
pub fn remove_server_files(socket: &Path, pid: Option<u32>) -> Result<(), Kept> {
    remove_server_files_within(socket, pid, server_gone_wait())
}

/// [`remove_server_files`], waiting at most `wait` for the server to exit.
///
/// # Errors
/// [`Kept`] when the server is not proved gone; nothing is removed.
pub fn remove_server_files_within(
    socket: &Path,
    pid: Option<u32>,
    wait: Duration,
) -> Result<(), Kept> {
    match pid {
        Some(pid) => exited(pid, wait)?,
        None if std::fs::symlink_metadata(socket).is_ok() => return Err(Kept::Unknown),
        None => {}
    }
    let _ = std::fs::remove_file(attach_lock_path(socket));
    let _ = std::fs::remove_file(socket);
    Ok(())
}

/// [`remove_server_files`] for a fixture's `Drop`. A server that outlived its
/// `kill-server` fails the test, or, when the test is already panicking (a
/// second panic would abort), is reported on stderr. An unknown pid keeps the
/// files and is reported: the fixture cannot tell, and the sweep will.
pub fn finish_server(socket: &Path, pid: Option<u32>) {
    finish_server_within(socket, pid, server_gone_wait());
}

/// [`finish_server`], waiting at most `wait` for the server to exit.
pub fn finish_server_within(socket: &Path, pid: Option<u32>, wait: Duration) {
    let started = Instant::now();
    match remove_server_files_within(socket, pid, wait) {
        Ok(()) => {}
        Err(Kept::Running(pid)) if !std::thread::panicking() => panic!(
            "{}: its server (pid {pid}) is still running after kill-server: waited {:?} of a {wait:?} budget; socket and lock kept",
            socket.display(),
            started.elapsed()
        ),
        Err(kept) => eprintln!(
            "pij-testkit: {}: {kept}; socket and lock kept",
            socket.display()
        ),
    }
}

/// A `tmux -L` server with one idle detached session, killed on drop.
#[derive(Debug)]
pub struct PrivateTmux {
    name: String,
    /// The server's own process, as `new-session -P` reported it (F245).
    pid: Option<u32>,
}

impl PrivateTmux {
    /// Start the server. Panics when tmux cannot, so a missing prerequisite is
    /// a failure, never a test that quietly ran against nothing.
    #[must_use]
    pub fn start() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let count = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "pij-test-{}-{count}-{}",
            std::process::id(),
            nanos % 1_000_000
        );
        let output = Command::new("tmux")
            .args([
                "-L",
                &name,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pid}",
                "-s",
                "test",
            ])
            .arg("exec sleep 600")
            .env_remove("TMUX")
            .output()
            .expect("tmux is installed");
        assert!(
            output.status.success(),
            "private tmux server {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let pid = String::from_utf8_lossy(&output.stdout).trim().parse().ok();
        Self { name, pid }
    }

    /// The `-L` socket name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The server's own process, as `new-session -P` reported it.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// How many control-mode (`tmux -C`) clients are attached right now.
    #[must_use]
    pub fn control_clients(&self) -> usize {
        Command::new("tmux")
            .args([
                "-L",
                &self.name,
                "list-clients",
                "-F",
                "#{client_control_mode}",
            ])
            .env_remove("TMUX")
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| *line == "1")
                    .count()
            })
            .unwrap_or(0)
    }
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.name, "kill-server"])
            .output();
        finish_server(&socket_path(&self.name), self.pid);
    }
}
