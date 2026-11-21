#![cfg(unix)]

use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

struct PrivateDaemon {
    child: Child,
    root: PathBuf,
}

impl Drop for PrivateDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn daemon_log_timestamps_startup_multiline_errors_and_shutdown() {
    let root = pij_testkit::fresh_dir("pij-timestamped-daemon-log");
    let state = root.join("state");
    let home = root.join("broken\nclaude-config");
    fs::create_dir_all(&state).expect("private daemon state");
    fs::create_dir_all(&home).expect("private Claude home");
    fs::write(home.join("settings.json"), "{").expect("malformed private settings");
    let log_path = state.join("daemon.log");
    let stdout = File::create(&log_path).expect("daemon log");
    let stderr = stdout.try_clone().expect("shared daemon log");
    let child = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .arg("--state-dir")
        .arg(&state)
        .args(["daemon", "--offline", "--bind", "127.0.0.1:0"])
        .env("HOME", &root)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env_remove("PIJ_RETIRED_HARNESSES")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start private daemon");
    let mut daemon = PrivateDaemon { child, root };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let log = fs::read_to_string(&log_path).expect("read running daemon log");
        // F137: port 0 binds a free port, and the banner names the real one.
        if let Some((banner, _)) = log
            .split("pij-rs daemon: listening on ")
            .nth(1)
            .and_then(|rest| rest.split_once('\n'))
        {
            let addr = banner.split_whitespace().next().expect("banner address");
            let addr: std::net::SocketAddr = addr.parse().expect("banner names IP:port");
            assert_ne!(addr.port(), 0, "banner must name the bound port: {log}");
            break;
        }
        assert!(
            daemon.child.try_wait().expect("daemon status").is_none(),
            "daemon exited before readiness: {log}"
        );
        assert!(Instant::now() < deadline, "daemon startup timed out: {log}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The existing foreground lifecycle handles SIGINT and drains its workers.
    assert!(
        Command::new("kill")
            .args(["-INT", &daemon.child.id().to_string()])
            .status()
            .expect("interrupt private daemon")
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = daemon.child.try_wait().expect("daemon shutdown status") {
            assert!(status.success(), "daemon shutdown failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "daemon shutdown did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    let log = fs::read_to_string(&log_path).expect("read final daemon log");
    assert!(log.contains("pij-rs daemon: shutting down"), "{log}");
    assert!(
        log.contains("claude-config"),
        "multiline path missing: {log}"
    );
    assert!(
        log.contains("EOF while parsing"),
        "stderr error missing: {log}"
    );
    for line in log.lines() {
        let (timestamp, _) = line.split_once(' ').expect("timestamp and log payload");
        assert!(
            OffsetDateTime::parse(timestamp, &Rfc3339).is_ok(),
            "daemon.log line has no RFC3339 prefix: {line:?}"
        );
    }
}

/// #90, end to end: a real daemon started on a `daemon.log` from before the
/// bound (here 40 MiB, its launcher's `>>`-style append handle as stdout and
/// stderr) keeps only the last 16 MiB of it as `daemon.log.1`, from a line
/// start, and writes its own lines to a fresh `daemon.log`.
#[test]
fn a_daemon_started_on_an_oversized_log_rotates_it_to_its_tail() {
    let root = pij_testkit::fresh_dir("pij-daemon-log-cap-at-start");
    let state = root.join("state");
    fs::create_dir_all(&state).expect("private daemon state");
    let log_path = state.join("daemon.log");
    let mut old = Vec::with_capacity(41 * 1024 * 1024);
    let mut n = 0_u64;
    while old.len() < 40 * 1024 * 1024 {
        old.extend_from_slice(format!("2026-10-27T00:00:00Z old line {n:010}\n").as_bytes());
        n += 1;
    }
    let last = format!("2026-10-27T00:00:00Z old line {:010}\n", n - 1);
    fs::write(&log_path, &old).expect("oversized log");
    let stdout = fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("append handle, as a launcher opens it");
    let stderr = stdout.try_clone().expect("shared daemon log");
    let child = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .arg("--state-dir")
        .arg(&state)
        .args(["daemon", "--offline", "--bind", "127.0.0.1:0"])
        .env("HOME", &root)
        .env_remove("PIJ_RETIRED_HARNESSES")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start private daemon");
    let mut daemon = PrivateDaemon { child, root };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("pij-rs daemon: listening on ") {
            assert!(log.len() < 64 * 1024, "a fresh log: {} bytes", log.len());
            assert!(log.contains("daemon log: rotated at 41943"), "{log}");
            break;
        }
        assert!(
            daemon.child.try_wait().expect("daemon status").is_none(),
            "daemon exited before readiness"
        );
        assert!(Instant::now() < deadline, "daemon startup timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let tail = fs::read(state.join("daemon.log.1")).expect("the kept tail");
    assert!(tail.len() as u64 <= 16 * 1024 * 1024, "{}", tail.len());
    assert!(tail.len() as u64 > 16 * 1024 * 1024 - 64, "{}", tail.len());
    assert!(
        tail.starts_with(b"2026-10-27T00:00:00Z old line "),
        "a line start"
    );
    assert!(
        tail.ends_with(last.as_bytes()),
        "the newest old line is kept"
    );
    assert!(old.ends_with(&tail));
    assert!(!state.join("daemon.log.2").exists());
    assert_eq!(
        fs::metadata(&log_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// R301-001 (moth's repro): two daemons started together on one private state
/// dir with an oversized `daemon.log`, each with its own launcher-opened append
/// handle, released at once into the same bind address. One wins the port and
/// serves; whichever trims, the serving daemon's own lines must reach the file
/// the path names (before the lock, the loser of the race could unlink the log
/// the winner was writing, leaving it serving into an unreachable inode).
#[test]
fn two_daemons_started_together_on_an_oversized_log_keep_the_server_on_the_path() {
    // The race is timing-dependent: on the pre-lock logger (8bf2c2d) it failed
    // 3 of 4 runs, always by the second attempt; this runs 8.
    for attempt in 0..8 {
        let root = pij_testkit::fresh_dir("pij-daemon-log-race");
        let state = root.join("state");
        fs::create_dir_all(&state).expect("private daemon state");
        let log_path = state.join("daemon.log");
        let mut old = Vec::with_capacity(19 * 1024 * 1024);
        let mut n = 0_u64;
        while old.len() < 18 * 1024 * 1024 {
            old.extend_from_slice(format!("2026-10-27T00:00:00Z old line {n:010}\n").as_bytes());
            n += 1;
        }
        fs::write(&log_path, &old).expect("oversized log");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();
        let start = |_: usize| {
            let stdout = fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .expect("append handle, as a launcher opens it");
            let stderr = stdout.try_clone().expect("shared daemon log");
            Command::new("/bin/sh")
                .args([
                    "-c",
                    "read go; exec \"$0\" --state-dir \"$1\" daemon --offline --bind \"$2\"",
                ])
                .arg(env!("CARGO_BIN_EXE_pij-rs"))
                .arg(&state)
                .arg(format!("127.0.0.1:{port}"))
                .env("HOME", &root)
                .env_remove("PIJ_RETIRED_HARNESSES")
                .stdin(Stdio::piped())
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
                .expect("start a held private daemon")
        };
        let mut children: Vec<Child> = (0..2).map(start).collect();
        let mut gates: Vec<_> = children
            .iter_mut()
            .map(|child| child.stdin.take().expect("gate"))
            .collect();
        for gate in &mut gates {
            gate.write_all(b"go\n").expect("release");
        }
        drop(gates);
        let deadline = Instant::now() + Duration::from_secs(30);
        let log = loop {
            let log = fs::read_to_string(&log_path).unwrap_or_default();
            let live = children
                .iter_mut()
                .map(|child| child.try_wait().expect("status").is_none())
                .filter(|alive| *alive)
                .count();
            if log.contains("pij-rs daemon: listening on ") && live == 1 {
                break log;
            }
            if Instant::now() >= deadline {
                for child in &mut children {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                panic!(
                    "attempt {attempt}: the serving daemon's banner never reached daemon.log ({live} live): {log}"
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!log.contains("rotation failed"), "attempt {attempt}: {log}");
        let tail = fs::read(state.join("daemon.log.1")).expect("the kept tail");
        assert!(
            old.ends_with(&tail),
            "attempt {attempt}: .1 is the old log's tail"
        );
        assert!(tail.len() as u64 <= 16 * 1024 * 1024);
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&root);
    }
}

/// R301-001: a daemon whose launcher opened `daemon.log` before another daemon
/// rotated it (the path now names a newer file) joins the file the path names
/// at start, under the lock, rather than writing into the rotated one.
#[test]
fn a_daemon_joins_the_log_rotated_after_its_launcher_opened_it() {
    let root = pij_testkit::fresh_dir("pij-daemon-log-join-at-start");
    let state = root.join("state");
    fs::create_dir_all(&state).expect("private daemon state");
    let log_path = state.join("daemon.log");
    fs::write(&log_path, b"2026-10-27T00:00:00Z before the rotation\n").expect("small log");
    let stdout = fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("append handle, as a launcher opens it");
    let stderr = stdout.try_clone().expect("shared daemon log");
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            "read go; exec \"$0\" --state-dir \"$1\" daemon --offline --bind 127.0.0.1:0",
        ])
        .arg(env!("CARGO_BIN_EXE_pij-rs"))
        .arg(&state)
        .env("HOME", &root)
        .env_remove("PIJ_RETIRED_HARNESSES")
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start a held private daemon");
    // Another daemon rotates while this one is still held before exec.
    fs::rename(&log_path, state.join("daemon.log.1")).expect("rotate");
    fs::write(
        &log_path,
        b"2026-10-27T00:00:01Z the other daemon's fresh log\n",
    )
    .expect("fresh");
    child
        .stdin
        .take()
        .expect("gate")
        .write_all(b"go\n")
        .expect("release");
    let mut daemon = PrivateDaemon { child, root };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let log = fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("pij-rs daemon: listening on ") {
            assert!(log.contains("another process rotated daemon.log"), "{log}");
            break;
        }
        assert!(
            daemon.child.try_wait().expect("daemon status").is_none(),
            "daemon exited before readiness"
        );
        assert!(
            Instant::now() < deadline,
            "the banner never reached daemon.log: {log}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let rotated = fs::read_to_string(state.join("daemon.log.1")).expect("rotated log");
    assert!(!rotated.contains("listening on"), "{rotated}");
}

/// F251 (moth's R301-R2-002): a daemon whose stdout is an unrelated file,
/// deleted before it starts, is not the managed log: it never adopts, joins or
/// trims `<state-dir>/daemon.log`, which stays exactly as it was.
#[test]
fn a_deleted_foreign_stdout_is_never_adopted() {
    let root = pij_testkit::fresh_dir("pij-daemon-log-foreign");
    let state = root.join("state");
    fs::create_dir_all(&state).expect("private daemon state");
    let log_path = state.join("daemon.log");
    let managed = b"2026-10-27T00:00:00Z the state dir's own log\n";
    fs::write(&log_path, managed).expect("state log");
    let foreign = root.join("elsewhere.log");
    let stdout = File::create(&foreign).expect("an unrelated file");
    let stderr = stdout.try_clone().expect("shared");
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            "read go; exec \"$0\" --state-dir \"$1\" daemon --offline --bind 127.0.0.1:0",
        ])
        .arg(env!("CARGO_BIN_EXE_pij-rs"))
        .arg(&state)
        .env("HOME", &root)
        .env_remove("PIJ_RETIRED_HARNESSES")
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("start a held private daemon");
    fs::remove_file(&foreign).expect("delete the foreign stdout");
    child
        .stdin
        .take()
        .expect("gate")
        .write_all(b"go\n")
        .expect("release");
    let daemon = PrivateDaemon { child, root };
    // The daemon is up once it answers on its state dir's key file, a fact it
    // writes outside its log.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !state.join("daemon.key").exists() {
        assert!(Instant::now() < deadline, "daemon startup timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        fs::read(&log_path).expect("state log"),
        managed,
        "the state dir's log is untouched"
    );
    drop(daemon);
}
