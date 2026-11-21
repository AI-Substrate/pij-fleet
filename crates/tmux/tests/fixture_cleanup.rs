//! F241: a fixture's tmux server leaves nothing in tmux's shared socket
//! directory. tmux keeps a socket's pathname after `kill-server`, and the
//! control client's attach lock sits beside it (F238); before F241 a fixture
//! that only killed its server left both, two entries per test.

use std::path::PathBuf;
use std::sync::mpsc;

use pij_core::ports::TmuxPort;
use pij_testkit::block_on;
use pij_testkit::tmux::{PrivateTmux, attach_lock_path, socket_path};
use pij_tmux::TmuxAdapter;

/// Attach a real control client to `tmux`, so its lock is made, then let it
/// go; the socket and the lock it leaves.
fn attached(tmux: &PrivateTmux) -> (PathBuf, PathBuf) {
    let taps = pij_testkit::fresh_dir("pij-rs-fixture-cleanup");
    let adapter = TmuxAdapter::for_server(tmux.name(), &taps);
    block_on(adapter.list_panes()).expect("list panes over the control client");
    drop(adapter);
    let _ = std::fs::remove_dir_all(&taps);
    let socket = socket_path(tmux.name());
    let lock = attach_lock_path(&socket);
    assert!(socket.exists(), "the server's socket: {}", socket.display());
    assert!(lock.exists(), "the product's lock: {}", lock.display());
    (socket, lock)
}

fn gone(path: &PathBuf) -> bool {
    std::fs::symlink_metadata(path).is_err()
}

#[test]
fn a_fixture_leaves_no_socket_or_lock_behind() {
    let tmux = PrivateTmux::start();
    let (socket, lock) = attached(&tmux);
    drop(tmux);
    assert!(gone(&socket), "socket left: {}", socket.display());
    assert!(gone(&lock), "lock left: {}", lock.display());
}

#[test]
fn a_panicking_test_s_fixture_leaves_no_socket_or_lock_behind() {
    let (sent, paths) = mpsc::channel();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let tmux = PrivateTmux::start();
        sent.send(attached(&tmux)).expect("send the paths");
        panic!("the test body failed with its fixture alive");
    }));
    assert!(failed.is_err(), "the body panicked");
    let (socket, lock) = paths.recv().expect("the paths");
    assert!(gone(&socket), "socket left: {}", socket.display());
    assert!(gone(&lock), "lock left: {}", lock.display());
}

/// The fixtures and the sweep name a server's socket and lock as the product
/// does, or they would leave the product's behind.
#[test]
fn the_testkit_names_are_the_product_s() {
    for label in ["pij-test-1-2-3", "odd.label:x"] {
        let socket = socket_path(label);
        assert_eq!(socket, pij_tmux::socket_path(Some(label)));
        assert_eq!(
            attach_lock_path(&socket),
            pij_tmux::socket_attach_lock_path(&socket)
        );
    }
}

/// R295-002, F245: a server whose process is still running after its
/// `kill-server` (failed, or here never run) keeps its socket and lock, so it
/// is never orphaned or made to look gone, and the fixture's teardown reports
/// it within its bound. An unknown pid keeps them too (fail closed). The
/// fixtures' own bound is long (F263), so this one is short.
#[test]
fn a_server_that_outlives_its_kill_keeps_its_socket_and_is_reported() {
    const WAIT: std::time::Duration = std::time::Duration::from_millis(300);
    let tmux = PrivateTmux::start();
    let pid = tmux.pid().expect("new-session -P names the server's pid");
    let (socket, lock) = attached(&tmux);
    let started = std::time::Instant::now();
    assert_eq!(
        pij_testkit::tmux::remove_server_files_within(&socket, Some(pid), WAIT),
        Err(pij_testkit::tmux::Kept::Running(pid)),
        "a running server is reported"
    );
    let took = started.elapsed();
    assert!(
        took >= WAIT && took < WAIT + pij_testkit::time::scaled(std::time::Duration::from_secs(1)),
        "bounded: {took:?}"
    );
    assert_eq!(
        pij_testkit::tmux::remove_server_files_within(&socket, None, WAIT),
        Err(pij_testkit::tmux::Kept::Unknown),
        "an unknown pid proves nothing"
    );
    assert!(!gone(&socket), "a live server keeps its socket");
    assert!(!gone(&lock), "and its lock");
    let reported = std::panic::catch_unwind(|| {
        pij_testkit::tmux::finish_server_within(&socket, Some(pid), WAIT);
    });
    assert!(reported.is_err(), "the teardown fails the test");
    assert!(!gone(&socket) && !gone(&lock), "still kept");
    drop(tmux);
    assert!(gone(&socket) && gone(&lock), "killed, then both go");
}

/// F263: `kill-server` returns before its server has exited (it still has to
/// kill its panes), so the teardown waits for that end state rather than the
/// command: the server's process is gone and its files removed, with no
/// spare moment allowed between the two.
#[test]
fn a_killed_server_is_waited_for_until_its_process_is_gone() {
    for _ in 0..5 {
        let tmux = PrivateTmux::start();
        let pid = tmux.pid().expect("new-session -P names the server's pid");
        let (socket, lock) = attached(&tmux);
        let killed = std::process::Command::new("tmux")
            .args(["-L", tmux.name(), "kill-server"])
            .output()
            .expect("kill-server");
        assert!(killed.status.success());
        assert_eq!(
            pij_testkit::tmux::remove_server_files(&socket, Some(pid)),
            Ok(()),
            "the server is waited for"
        );
        assert_eq!(pij_core::program::process_exists(pid), Some(false));
        assert!(gone(&socket) && gone(&lock));
        drop(tmux);
    }
}
