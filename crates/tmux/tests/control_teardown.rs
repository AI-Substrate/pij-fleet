//! F238: a control client leaves `_pij` handled before it goes, through the
//! adapter's real exit path (dropping the adapter drops its `Live`).
//!
//! Alone (`_pij` has only this client and only the control pane): the
//! teardown kills `_pij`. Not alone: `_pij` stays, with `destroy-unattached`
//! off, so no later exit can destroy it under a client that is dead but not
//! yet lost (the tmux 3.4 SIGSEGV of F237). Against a real isolated server.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pij_core::ports::TmuxPort;
use pij_testkit::block_on;
use pij_tmux::{CONTROL_SESSION, TmuxAdapter, socket_attach_lock_path, socket_path};

static SERVERS: AtomicUsize = AtomicUsize::new(0);

/// An isolated server with one ordinary session, killed with its lock file.
struct Server {
    label: String,
    taps: PathBuf,
}

impl Server {
    fn start_or_skip(name: &str) -> Option<Self> {
        if !Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("SKIP real tmux test: tmux is unavailable");
            return None;
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let label = format!(
            "pij-rs-teardown-{name}-{}-{nonce}-{}",
            std::process::id(),
            SERVERS.fetch_add(1, Ordering::Relaxed)
        );
        let taps = std::env::temp_dir().join(format!("{label}-taps"));
        let server = Self { label, taps };
        server.run(&[
            "new-session",
            "-d",
            "-s",
            "main",
            "-x",
            "80",
            "-y",
            "24",
            "cat",
        ]);
        Some(server)
    }

    fn adapter(&self) -> TmuxAdapter {
        TmuxAdapter::for_server(&self.label, &self.taps)
    }

    fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", &self.label, "-f", "/dev/null"])
            .env("SHELL", "/bin/sh")
            .args(args)
            .output()
            .expect("tmux")
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = self.tmux(args);
        assert!(
            output.status.success(),
            "isolated tmux command {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn has_session(&self, name: &str) -> bool {
        self.tmux(&["has-session", "-t", &format!("={name}")])
            .status
            .success()
    }

    fn destroy_unattached(&self) -> String {
        let output = self.run(&[
            "show-options",
            "-v",
            "-t",
            &format!("{CONTROL_SESSION}:"),
            "destroy-unattached",
        ]);
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Start the adapter's control client: a read rides it.
    fn attach(&self, adapter: &TmuxAdapter) {
        block_on(adapter.list_panes()).expect("list panes over the control client");
        assert!(
            self.has_session(CONTROL_SESSION),
            "the control client attached"
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
        let _ = std::fs::remove_file(socket_attach_lock_path(&socket_path(Some(&self.label))));
        let _ = std::fs::remove_dir_all(&self.taps);
    }
}

#[test]
fn the_last_client_of_an_alone_control_session_kills_it_on_the_way_out() {
    let Some(server) = Server::start_or_skip("alone") else {
        return;
    };
    let adapter = server.adapter();
    server.attach(&adapter);
    drop(adapter);
    assert!(
        !server.has_session(CONTROL_SESSION),
        "the teardown killed the alone _pij"
    );
    assert!(
        server.has_session("main"),
        "the operator's session survives"
    );
}

#[test]
fn a_client_leaving_a_shared_control_session_keeps_it_and_switches_destroy_unattached_off() {
    let Some(server) = Server::start_or_skip("shared") else {
        return;
    };
    let (first, second) = (server.adapter(), server.adapter());
    server.attach(&first);
    server.attach(&second);
    assert_eq!(
        server.destroy_unattached(),
        "on",
        "each attach switches it on"
    );
    drop(first);
    assert!(
        server.has_session(CONTROL_SESSION),
        "_pij is not alone: kept"
    );
    assert_eq!(
        server.destroy_unattached(),
        "off",
        "no later exit may destroy it under a dying client"
    );
    // The remaining client still answers, and is now alone: it kills _pij.
    block_on(second.list_panes()).expect("the other client still reads");
    drop(second);
    assert!(!server.has_session(CONTROL_SESSION));
    assert!(server.has_session("main"));
}

#[test]
fn a_control_session_holding_real_work_is_kept() {
    let Some(server) = Server::start_or_skip("work") else {
        return;
    };
    let adapter = server.adapter();
    server.attach(&adapter);
    server.run(&[
        "new-window",
        "-d",
        "-t",
        &format!("={CONTROL_SESSION}:"),
        "cat",
    ]);
    drop(adapter);
    assert!(
        server.has_session(CONTROL_SESSION),
        "a window that landed in _pij is real work: kept"
    );
    assert_eq!(server.destroy_unattached(), "off");
}

/// Review of #292 (R292-001): a window lands in `_pij`, then the control
/// client's own idle window is closed. `_pij` is then one window and one
/// pane, as when alone, but that pane is real work: the leaving client keeps
/// it, and a later client's attach does not switch `destroy-unattached` back
/// on under it. `leave` is how the first client goes: dropped, or released.
fn a_real_pane_left_alone_survives(name: &str, leave: impl FnOnce(TmuxAdapter)) {
    let Some(server) = Server::start_or_skip(name) else {
        return;
    };
    let adapter = server.adapter();
    server.attach(&adapter);
    let target = format!("={CONTROL_SESSION}:");
    let idle = server.run(&["list-windows", "-t", &target, "-F", "#{window_id}"]);
    let idle = String::from_utf8_lossy(&idle.stdout).trim().to_string();
    server.run(&["new-window", "-d", "-t", &target, "cat"]);
    server.run(&["kill-window", "-t", &idle]);
    leave(adapter);
    assert!(
        server.has_session(CONTROL_SESSION),
        "a real pane alone in _pij is never killed"
    );
    assert_eq!(server.destroy_unattached(), "off");

    let later = server.adapter();
    server.attach(&later);
    assert_eq!(
        server.destroy_unattached(),
        "off",
        "an attach switches it on only for the idle pane"
    );
    drop(later);
    // The start command, not the current one: Linux tmux runs `cat` under
    // `sh -c`, so its current command reads `sh`.
    let panes = server.run(&["list-panes", "-t", &target, "-F", "#{pane_start_command}"]);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().count(), 1, "{panes}");
    assert!(panes.contains("cat"), "the real pane survives: {panes}");
}

#[test]
fn a_real_pane_left_alone_in_the_control_session_survives_a_drop() {
    a_real_pane_left_alone_survives("lone-drop", drop);
}

#[test]
fn a_real_pane_left_alone_in_the_control_session_survives_a_release() {
    a_real_pane_left_alone_survives("lone-release", |adapter| adapter.release_control());
}

/// The daemon's graceful shutdown (F238): `release_control` sends the client
/// out through its teardown, and no read afterwards attaches another one.
#[test]
fn a_released_adapter_leaves_through_its_teardown_and_never_reattaches() {
    let Some(server) = Server::start_or_skip("release") else {
        return;
    };
    let adapter = server.adapter();
    server.attach(&adapter);
    adapter.release_control();
    assert!(
        !server.has_session(CONTROL_SESSION),
        "the teardown killed the alone _pij"
    );
    block_on(adapter.list_panes()).expect("a read after release forks");
    assert!(
        !server.has_session(CONTROL_SESSION),
        "no client attached again after release"
    );
}
