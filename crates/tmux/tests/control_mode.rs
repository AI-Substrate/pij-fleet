//! Plan 180: tmux reads ride one control-mode client instead of a process each.
//!
//! Every test runs against its own `tmux -L` server, reached through a wrapper
//! executable that appends one line per tmux process it starts. That log is the
//! proof: Defender's EDR pays per process launch (pij-fleet#62).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pij_core::ports::TmuxPort;
use pij_testkit::block_on;
use pij_tmux::TmuxAdapter;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Server {
    name: String,
    dir: PathBuf,
    wrapper: PathBuf,
    log: PathBuf,
    panes: Vec<String>,
}

impl Server {
    /// An isolated server with one session of three panes. The first pane
    /// prints text with a tab and non-ASCII, so encoding differences show.
    fn start() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let name = format!("pij180-{}-{n}-{}", std::process::id(), nanos % 1_000_000);
        let dir = std::env::temp_dir().join(&name);
        fs::create_dir_all(&dir).expect("dir");
        let log = dir.join("launches.log");
        let wrapper = dir.join("tmux-wrapper");
        // `env -i` mimics a launchd daemon with no LANG: the encoding case the
        // forked path's `-u` exists for must hold for control mode too.
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec env -i PATH=\"$PATH\" tmux -L '{}' \"$@\"\n",
                log.display(),
                name
            ),
        )
        .expect("wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("chmod");
        let tmux = |args: &[&str]| {
            let output = Command::new("tmux")
                .args(["-L", &name])
                .args(args)
                .output()
                .expect("tmux");
            assert!(
                output.status.success(),
                "tmux {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).expect("utf8")
        };
        tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "work",
            "-x",
            "120",
            "-y",
            "30",
            "printf 'a\\tb ✓ %%end 1 2 3\\n'; exec sleep 600",
        ]);
        tmux(&["split-window", "-t", "work", "exec sleep 600"]);
        tmux(&["split-window", "-t", "work", "exec sleep 600"]);
        let panes = tmux(&["list-panes", "-t", "work", "-F", "#{pane_id}"])
            .lines()
            .map(str::to_string)
            .collect();
        // Let the first pane's printf land before anything captures it.
        std::thread::sleep(Duration::from_millis(300));
        Self {
            name,
            dir,
            wrapper,
            log,
            panes,
        }
    }

    fn adapter(&self, control: bool) -> TmuxAdapter {
        TmuxAdapter::with_binary(&self.wrapper, self.dir.join("taps")).with_control_mode(control)
    }

    fn launches(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-L", &self.name])
            .args(args)
            .output()
            .expect("tmux");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.name, "kill-server"])
            .output();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// One round of every read the pane observer and the status path make.
async fn read_everything(adapter: &TmuxAdapter, panes: &[String]) {
    adapter.list_panes().await.expect("list-panes");
    for pane in panes {
        adapter.capture(pane, 50).await.expect("capture-pane");
        adapter.pane_process(pane).await.expect("pane_process");
        adapter.pane_tap_sink(pane).await.expect("show-options");
        adapter.user_typing(pane).await.expect("display-message");
    }
}

#[test]
fn reads_after_warm_up_launch_no_tmux_process() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(async {
        read_everything(&adapter, &server.panes).await;
        let warm = server.launches();
        assert!(
            warm.len() <= 2,
            "warm-up may start the control client (list-sessions + attach), nothing more: {warm:#?}"
        );
        for _ in 0..25 {
            read_everything(&adapter, &server.panes).await;
        }
        let after = server.launches();
        assert_eq!(
            after.len(),
            warm.len(),
            "25 rounds x {} panes of reads launched tmux: {:#?}",
            server.panes.len(),
            &after[warm.len()..]
        );
    });
}

#[test]
fn control_answers_match_forked_answers_byte_for_byte() {
    let server = Server::start();
    let forked = server.adapter(false);
    let control = server.adapter(true);
    block_on(async {
        control.list_panes().await.expect("warm the control client");
        // Every comparison below forks only on the `forked` side.
        let forked_calls = |log: &[String]| log.iter().filter(|l| !l.contains(" -C ")).count();
        let started = server.launches();
        assert_eq!(
            control.list_panes().await.expect("control list"),
            forked.list_panes().await.expect("forked list")
        );
        for pane in &server.panes {
            for lines in [1, 5, 30, 50] {
                assert_eq!(
                    control.capture(pane, lines).await.expect("control capture"),
                    forked.capture(pane, lines).await.expect("forked capture"),
                    "capture {pane} {lines}"
                );
            }
            assert_eq!(
                control.pane_process(pane).await.expect("control pid/cwd"),
                forked.pane_process(pane).await.expect("forked pid/cwd")
            );
            assert_eq!(
                control.user_typing(pane).await.expect("control mode"),
                forked.user_typing(pane).await.expect("forked mode")
            );
            assert_eq!(
                control.pane_tap_sink(pane).await.expect("control unset"),
                forked.pane_tap_sink(pane).await.expect("forked unset")
            );
        }
        let compared = server.launches();
        let forked_rounds = 1 + server.panes.len() * (4 + 3);
        assert_eq!(
            forked_calls(&compared[started.len()..]),
            forked_rounds,
            "only the forked side launched: the control answers really came over control mode: {:#?}",
            &compared[started.len()..]
        );
        let first = &server.panes[0];
        assert!(
            control
                .capture(first, 30)
                .await
                .expect("capture")
                .contains("a\tb ✓ %end 1 2 3"),
            "a pane line that looks like a control frame is content"
        );
        server.tmux(&[
            "set-option",
            "-p",
            "-t",
            first,
            "@pij-tap-sink",
            "/tmp/x y.raw",
        ]);
        assert_eq!(
            control.pane_tap_sink(first).await.expect("control set"),
            Some(PathBuf::from("/tmp/x y.raw"))
        );
        assert_eq!(
            control.pane_tap_sink(first).await.expect("control set"),
            forked.pane_tap_sink(first).await.expect("forked set")
        );
    });
}

#[test]
fn a_missing_pane_is_the_same_error_and_the_same_absence() {
    let server = Server::start();
    let forked = server.adapter(false);
    let control = server.adapter(true);
    block_on(async {
        let (left, right) = (
            control.capture("%999", 5).await.expect_err("control"),
            forked.capture("%999", 5).await.expect_err("forked"),
        );
        assert_eq!(left.to_string(), right.to_string());
        assert!(left.to_string().contains("can't find pane"), "{left}");
        // tmux 3.6 answers `display-message -p -t %999` with exit 0 and no
        // output, so the forked path already errors here. Parity, not a fix.
        assert_eq!(
            format!("{:?}", control.pane_process("%999").await),
            format!("{:?}", forked.pane_process("%999").await)
        );
    });
}

#[test]
fn a_lost_control_client_falls_back_then_reconnects() {
    let server = Server::start();
    let adapter = server.adapter(true);
    let expected = block_on(server.adapter(false).list_panes()).expect("forked truth");
    block_on(async {
        read_everything(&adapter, &server.panes).await;
        let clients = server.tmux(&[
            "list-clients",
            "-F",
            "#{client_name} #{client_control_mode}",
        ]);
        let control = clients
            .lines()
            .find_map(|line| line.strip_suffix(" 1"))
            .unwrap_or_else(|| panic!("no control client attached: {clients:?}"))
            .to_string();
        server.tmux(&["detach-client", "-t", &control]);
        // Every answer after the loss is still correct.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert_eq!(adapter.list_panes().await.expect("list"), expected);
            let before = server.launches().len();
            assert_eq!(adapter.list_panes().await.expect("list"), expected);
            if server.launches().len() == before {
                break; // back on the control client: no launch for a read
            }
            assert!(
                Instant::now() < deadline,
                "never reconnected: {:#?}",
                server.launches()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    });
}

#[test]
fn a_dead_server_is_a_plain_error_not_a_hang() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(async {
        read_everything(&adapter, &server.panes).await;
        server.tmux(&["kill-server"]);
        let started = Instant::now();
        let error = adapter.list_panes().await.expect_err("server is gone");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}: {error}",
            started.elapsed()
        );
    });
}
