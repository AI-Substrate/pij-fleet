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
//! Every fixture removes both ([`remove_server_files`]) on its way out,
//! panicking or not.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// F241: remove what a fixture's killed server leaves in tmux's shared
/// directory, its socket and pij's attach lock beside it. Call it after
/// `kill-server`, from the fixture's `Drop`, so a panicking test cleans up
/// too. Either one already gone is fine.
pub fn remove_server_files(socket: &Path) {
    let _ = std::fs::remove_file(attach_lock_path(socket));
    let _ = std::fs::remove_file(socket);
}

/// A `tmux -L` server with one idle detached session, killed on drop.
#[derive(Debug)]
pub struct PrivateTmux {
    name: String,
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
        Self { name }
    }

    /// The `-L` socket name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
        remove_server_files(&socket_path(&self.name));
    }
}
