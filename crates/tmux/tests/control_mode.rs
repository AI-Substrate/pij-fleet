//! Plan 180: tmux reads ride one control-mode client instead of a process each.
//!
//! Every test runs against its own `tmux -L` server, reached through a wrapper
//! executable that appends one line per tmux process it starts. That log is the
//! proof: Defender's EDR pays per process launch (pij-fleet#62).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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
        let mut server = Self::unstarted();
        server.run(&[
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
        server.run(&["split-window", "-t", "work", "exec sleep 600"]);
        server.run(&["split-window", "-t", "work", "exec sleep 600"]);
        server.panes = server
            .run(&["list-panes", "-t", "work", "-F", "#{pane_id}"])
            .lines()
            .map(str::to_string)
            .collect();
        // Let the first pane's printf land before anything captures it.
        std::thread::sleep(Duration::from_millis(300));
        server
    }

    /// An isolated server with plain sessions, created oldest first.
    fn with_sessions(names: &[&str]) -> Self {
        let server = Self::unstarted();
        for (index, name) in names.iter().enumerate() {
            if index > 0 {
                // tmux orders sessions by activity time; keep it strict.
                std::thread::sleep(Duration::from_millis(50));
            }
            server.run(&[
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                name,
                "exec sleep 600",
            ]);
        }
        server
    }

    /// The names and the wrapper, but no tmux server yet.
    fn unstarted() -> Self {
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
        Self {
            name,
            dir,
            wrapper,
            log,
            panes: Vec::new(),
        }
    }

    /// Run a tmux command on this server from outside tmux, and require success.
    fn run(&self, args: &[&str]) -> String {
        let output = self.outside(args);
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8")
    }

    /// A tmux command as a script outside tmux sends it: no `TMUX` to inherit.
    fn outside(&self, args: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .args(["-L", &self.name])
            .args(args)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .expect("tmux")
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
        String::from_utf8_lossy(&self.outside(args).stdout).into_owned()
    }

    /// The session a human's `tmux attach` would land in, given `target`.
    ///
    /// A control client takes the same path through `attach-session` as a
    /// terminal one (tmux's `cmd_find`, preferring an unattached session when
    /// untargeted), and needs no pty. It detaches again before returning.
    fn human_attach(&self, target: Option<&str>) -> String {
        let mut args = vec!["-L", self.name.as_str(), "-C", "attach-session"];
        if let Some(target) = target {
            args.extend(["-t", target]);
        }
        let mut child = Command::new("tmux")
            .args(&args)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("human client");
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(stdin, "display-message -p 'landed=#{{session_name}}'").expect("ask");
        let landed = BufReader::new(child.stdout.take().expect("stdout"))
            .lines()
            .map_while(Result::ok)
            .find_map(|line| line.strip_prefix("landed=").map(str::to_string))
            .expect("the human client answered");
        let _ = child.kill();
        let _ = child.wait();
        landed
    }

    /// `(pid, flags)` of every control-mode client attached to this server.
    fn control_clients(&self) -> Vec<(u32, String)> {
        self.tmux(&[
            "list-clients",
            "-F",
            "#{client_control_mode} #{client_pid} #{client_session} #{client_flags}",
        ])
        .lines()
        .filter_map(|line| line.strip_prefix("1 "))
        .map(|rest| {
            let (pid, flags) = rest.split_once(' ').expect("pid then session");
            (pid.parse().expect("pid"), flags.to_string())
        })
        .collect()
    }

    fn alive(&self) -> bool {
        self.outside(&["has-session"]).status.success()
            || self.outside(&["list-sessions"]).status.success()
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
            warm.len() <= 1,
            "warm-up may start the control client (one attach), nothing more: {warm:#?}"
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
                // Tab and non-ASCII rendering vary by tmux version and locale
                // (ubuntu CI); byte parity with the forked client is asserted
                // above. Here: a frame-like `%end` line stays content.
                .contains("%end 1 2 3"),
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

/// The control client's own session (pij-fleet#73 review S1, R-PRIME-1).
const CONTROL_SESSION: &str = "_pij";

/// S1: the control client must not change where a human's bare `tmux attach`
/// lands. Before R-PRIME-1 it attached to the first session, which made that
/// session "attached", so a bare attach skipped it.
#[test]
fn a_bare_attach_lands_where_it_would_without_the_control_client() {
    let landing = |control: bool| {
        // `a` is the first session and the one the human used last.
        let server = Server::with_sessions(&["a", "b"]);
        assert_eq!(server.human_attach(Some("a")), "a");
        let adapter = server.adapter(control);
        block_on(adapter.list_panes()).expect("warm");
        let attached = server.control_clients().len();
        assert_eq!(attached, usize::from(control), "control clients attached");
        server.human_attach(None)
    };
    assert_eq!(landing(false), "a", "baseline: bare attach returns to a");
    assert_eq!(landing(true), "a", "the control client moved the human");
}

/// S1's documented residual, pinned so a change to it is a visible change.
/// Attaching any client bumps its session's activity, and a command run from
/// outside tmux with no `-t` resolves to the most recently active session. So
/// right after the control client (re)attaches, such a command lands in `_pij`:
/// until a keypress in another session, or indefinitely with no human attached.
/// What lands there persists and is listed (see the foreign-window tests).
#[test]
fn an_untargeted_outside_command_resolves_to_pij_after_the_control_attach() {
    let server = Server::with_sessions(&["a", "b"]);
    let untargeted = || server.run(&["display-message", "-p", "#{session_name}"]);
    assert_eq!(untargeted().trim(), "b", "baseline: the newest session");
    let adapter = server.adapter(true);
    block_on(adapter.list_panes()).expect("warm");
    assert_eq!(untargeted().trim(), CONTROL_SESSION);
}

/// The control session's pane is never a pane pij lists, so no seat lookup,
/// tap sweep, reap or caller-pane resolution can ever see it.
#[test]
fn the_control_session_is_attached_read_only_and_never_listed() {
    let server = Server::start();
    let adapter = server.adapter(true);
    let listed = block_on(adapter.list_panes()).expect("list");
    let clients = server.control_clients();
    assert_eq!(clients.len(), 1, "{clients:?}");
    let (_, flags) = &clients[0];
    for flag in ["no-output", "ignore-size", "read-only"] {
        assert!(flags.contains(flag), "{flag} missing: {flags}");
    }
    let sessions = server.tmux(&["list-sessions", "-F", "#{session_name}"]);
    assert!(
        sessions.lines().any(|name| name == CONTROL_SESSION),
        "the control client has its own session: {sessions:?}"
    );
    let ids: Vec<&str> = listed.iter().map(|pane| pane.id.as_str()).collect();
    assert_eq!(ids, server.panes, "only the operator's panes are listed");
    // Its options and hooks are `_pij`'s alone, never global ones.
    assert_eq!(
        server
            .tmux(&["show-options", "-gv", "destroy-unattached"])
            .trim(),
        "off"
    );
    let global = server.tmux(&["show-hooks", "-g"]);
    assert!(!global.contains("destroy-unattached"), "{global}");
    let own = server.tmux(&["show-hooks", "-t", CONTROL_SESSION]);
    assert!(own.contains("after-new-window"), "{own}");
    assert!(listed.iter().all(|pane| pane.session != CONTROL_SESSION));
}

/// `_pij` must not keep a server alive that the operator has emptied: once it
/// is the only session, the client leaves and `_pij` goes with it.
#[test]
fn the_server_still_exits_when_the_operator_closes_the_last_session() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(adapter.list_panes()).expect("warm");
    server.run(&["kill-session", "-t", "work"]);
    let deadline = Instant::now() + Duration::from_secs(15);
    while server.alive() {
        assert!(
            Instant::now() < deadline,
            "the server outlived its last real session: {:?}",
            server.tmux(&["list-sessions"])
        );
        // The daemon reads every 600 ms; each read retires a client that left.
        let _ = block_on(adapter.list_panes());
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The `(session, window)` of every window on the server.
fn windows(server: &Server) -> Vec<String> {
    server
        .tmux(&["list-windows", "-a", "-F", "#{session_name}:#{window_name}"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// R1 of the #73 recheck: an untargeted command from outside tmux can land a
/// window in `_pij`. pij must still see it, and it must outlive the control
/// client, including a later client that attaches to the same `_pij`.
#[test]
fn a_foreign_window_in_pij_stays_listed_and_outlives_the_control_client() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(adapter.list_panes()).expect("warm");
    server.run(&["new-window", "-n", "someones-job", "exec sleep 600"]);
    let foreign = format!("{CONTROL_SESSION}:someones-job");
    assert!(
        windows(&server).contains(&foreign),
        "{:?}",
        windows(&server)
    );
    let listed = block_on(adapter.list_panes()).expect("list");
    assert!(
        listed
            .iter()
            .any(|pane| pane.session == CONTROL_SESSION && pane.window == "someones-job"),
        "a foreign window in _pij is hidden from pij: {listed:?}"
    );
    assert_eq!(
        listed.len(),
        server.panes.len() + 1,
        "only the control pane is hidden"
    );
    drop(adapter);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        windows(&server).contains(&foreign),
        "destroyed with the client"
    );
    // A later client attaches to the same `_pij`; it must not re-arm destroy.
    let again = server.adapter(true);
    block_on(again.list_panes()).expect("warm again");
    drop(again);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        windows(&server).contains(&foreign),
        "destroyed by the second client"
    );
}

/// A foreign window in `_pij` is real work, so `_pij` is not "alone" even when
/// it is the last session: the client stays attached instead of leaving and
/// re-attaching every backoff, and the server stays up for that window.
#[test]
fn a_foreign_window_keeps_the_client_attached_when_pij_is_the_last_session() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(adapter.list_panes()).expect("warm");
    server.run(&["new-window", "-n", "someones-job", "exec sleep 600"]);
    let before = server.control_clients();
    assert_eq!(before.len(), 1, "{before:?}");
    server.run(&["kill-session", "-t", "work"]);
    // The subscription is evaluated about once a second.
    let settle = Instant::now() + Duration::from_secs(3);
    while Instant::now() < settle {
        let _ = block_on(adapter.list_panes());
        std::thread::sleep(Duration::from_millis(200));
    }
    let after = server.control_clients();
    assert_eq!(
        after.iter().map(|(pid, _)| *pid).collect::<Vec<_>>(),
        before.iter().map(|(pid, _)| *pid).collect::<Vec<_>>(),
        "the client left (or churned) while a foreign window kept _pij alive"
    );
    assert!(windows(&server).contains(&format!("{CONTROL_SESSION}:someones-job")));
}

/// The same for a pane split into `_pij`'s own window.
#[test]
fn a_foreign_pane_split_into_pij_stays_listed_and_outlives_the_control_client() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(adapter.list_panes()).expect("warm");
    let foreign = server
        .run(&["split-window", "-P", "-F", "#{pane_id}", "exec sleep 600"])
        .trim()
        .to_string();
    let listed = block_on(adapter.list_panes()).expect("list");
    assert!(
        listed.iter().any(|pane| pane.id == foreign),
        "{foreign} hidden: {listed:?}"
    );
    drop(adapter);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        server
            .tmux(&["list-panes", "-a", "-F", "#{pane_id}"])
            .lines()
            .any(|pane| pane == foreign),
        "destroyed with the client"
    );
}

/// With only its own window, `_pij` still goes when the client does.
#[test]
fn pij_with_only_its_own_window_goes_with_the_client() {
    let server = Server::start();
    let adapter = server.adapter(true);
    block_on(adapter.list_panes()).expect("warm");
    assert!(windows(&server).iter().any(|w| w.starts_with("_pij:")));
    drop(adapter);
    std::thread::sleep(Duration::from_millis(300));
    let left = windows(&server);
    assert!(left.iter().all(|w| w.starts_with("work:")), "{left:?}");
}

/// No server running means none is started: `-N` on the control attach.
#[test]
fn reads_never_start_a_server() {
    let server = Server::unstarted();
    let adapter = server.adapter(true);
    assert!(block_on(adapter.list_panes()).is_err());
    assert!(!server.alive(), "a read started a tmux server");
}

/// Sends `SIGCONT` on drop, so a failed assertion never leaves a stopped server.
struct Resume(u32);

impl Drop for Resume {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-CONT", &self.0.to_string()])
            .status();
    }
}

/// S4: a client that stops answering is replaced. The read that found it
/// stalled forks after the timeout, and reads reconnect afterwards.
///
/// Stopping the `tmux -C` process itself stalls nothing: tmux hands the
/// client's stdin and stdout to the server, which then reads and writes our
/// pipes directly. So this stalls the (private) server for a few seconds.
#[test]
fn a_stalled_control_client_is_killed_and_replaced() {
    let server = Server::start();
    let adapter = server.adapter(true);
    let expected = block_on(server.adapter(false).list_panes()).expect("forked truth");
    block_on(adapter.list_panes()).expect("warm");
    let clients = server.control_clients();
    assert_eq!(clients.len(), 1, "{clients:?}");
    let stalled = clients[0].0;
    let pid: u32 = server
        .run(&["display-message", "-p", "#{pid}"])
        .trim()
        .parse()
        .expect("server pid");
    let stop = Command::new("kill")
        .args(["-STOP", &pid.to_string()])
        .status()
        .expect("kill");
    assert!(stop.success());
    let resume = Resume(pid);
    let resumer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        drop(resume);
    });
    let started = Instant::now();
    // Waits out the reply timeout, then forks; the fork answers once resumed.
    assert_eq!(block_on(adapter.list_panes()).expect("list"), expected);
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "answered while the server was stopped? {:?}",
        started.elapsed()
    );
    resumer.join().expect("resumed");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert_eq!(block_on(adapter.list_panes()).expect("list"), expected);
        let before = server.launches().len();
        assert_eq!(block_on(adapter.list_panes()).expect("list"), expected);
        if server.launches().len() == before {
            break; // a control client answers without a launch
        }
        assert!(
            Instant::now() < deadline,
            "never reconnected: {:?}",
            server.control_clients()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    let now = server.control_clients();
    assert_eq!(now.len(), 1, "{now:?}");
    assert_ne!(
        now[0].0, stalled,
        "the stalled client was kept, not replaced"
    );
}
