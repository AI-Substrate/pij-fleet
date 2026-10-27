//! A private tmux server per test, so a test never touches the operator's.
//!
//! A test daemon with real adapters lists panes and attaches a control client
//! (plan 180). Pointed at the default server, that is the operator's tmux
//! (pij-fleet#73 review S3). Name this server in `Config::tmux_server` or
//! `PIJ_TMUX_SERVER` instead; it is killed when the value drops.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(0);

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
    }
}
