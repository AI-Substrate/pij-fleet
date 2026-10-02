#![cfg(unix)]

use std::fs::{self, File};
use std::net::TcpListener;
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
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve private port");
    let bind = listener.local_addr().expect("private address").to_string();
    drop(listener);
    let child = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .arg("--state-dir")
        .arg(&state)
        .args(["daemon", "--offline", "--bind", &bind])
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
        if log.contains("pij-rs daemon: listening on") {
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
