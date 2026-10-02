use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pij_core::model::Seq;
use pij_core::ports::Spine;
use pij_store::SqliteSpine;

#[test]
fn doctor_reports_each_home_without_writing_settings() {
    let root = pij_testkit::fresh_dir("pij-doctor-claude-inbound");
    let home = root.join("custom-claude");
    fs::create_dir_all(&home).expect("Claude home");

    let missing = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args(["--json", "doctor", "claude-inbound"])
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("HOME", &root)
        .output()
        .expect("run doctor");

    assert!(missing.status.success());
    let missing_stdout = String::from_utf8(missing.stdout).expect("UTF-8 output");
    assert!(missing_stdout.contains(&home.display().to_string()));
    assert!(missing_stdout.contains("\"status\":\"file-missing\""));
    assert!(!home.join("settings.json").exists());

    fs::write(home.join("settings.json"), "  \n").expect("seed whitespace settings");
    let empty_output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args(["--json", "doctor", "claude-inbound"])
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("HOME", &root)
        .output()
        .expect("run doctor");
    assert!(empty_output.status.success());
    let empty_stdout = String::from_utf8(empty_output.stdout).expect("UTF-8 output");
    assert!(empty_stdout.contains("\"status\":\"key-missing\""));
    assert_eq!(
        fs::read_to_string(home.join("settings.json")).expect("settings readable"),
        "  \n"
    );

    let keyless = r#"{"keep":true}"#;
    fs::write(home.join("settings.json"), keyless).expect("seed keyless settings");
    let keyless_output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args(["--json", "doctor", "claude-inbound"])
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("HOME", &root)
        .output()
        .expect("run doctor");
    assert!(keyless_output.status.success());
    let keyless_stdout = String::from_utf8(keyless_output.stdout).expect("UTF-8 output");
    assert!(keyless_stdout.contains("\"status\":\"key-missing\""));
    assert_eq!(
        fs::read_to_string(home.join("settings.json")).expect("settings readable"),
        keyless
    );

    let original = r#"{"crossSessionInbound":"hold","keep":true}"#;
    fs::write(home.join("settings.json"), original).expect("seed settings");
    let held = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args(["--json", "doctor", "claude-inbound"])
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("HOME", &root)
        .output()
        .expect("run doctor");

    assert!(held.status.success());
    let held_stdout = String::from_utf8(held.stdout).expect("UTF-8 output");
    assert!(held_stdout.contains("\"current\":\"hold\""));
    assert!(held_stdout.contains("\"status\":\"not-accept\""));
    assert_eq!(
        fs::read_to_string(home.join("settings.json")).expect("settings readable"),
        original
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn daemon_boot_repairs_once_and_publishes_one_change_event() {
    let root = pij_testkit::fresh_dir("pij-daemon-claude-inbound");
    let home = root.join("custom-claude");
    let state_dir = root.join("state");
    fs::create_dir_all(&home).expect("Claude home");

    boot_once(&root, &home, &state_dir);
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(home.join("settings.json")).expect("settings after boot"))
            .expect("valid settings");
    assert_eq!(settings["crossSessionInbound"], "accept");
    let expected_hook = root.join(".pij-rs/claude-session-start-pij.sh");
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        expected_hook.display().to_string()
    );
    assert!(expected_hook.is_file());
    assert!(!state_dir.join("claude-session-start-pij.sh").exists());

    let database_path = state_dir.join("pij.sqlite");
    let spine = SqliteSpine::new(
        pij_store::open(database_path.to_str().expect("UTF-8 database path"))
            .await
            .expect("open daemon spine"),
    );
    let first_events = spine
        .tail(None, Seq(0))
        .await
        .expect("tail first-boot events");
    assert_eq!(
        first_events
            .iter()
            .filter(|event| event.kind == "config.claude-inbound-ensured")
            .count(),
        1
    );
    assert_eq!(
        first_events
            .iter()
            .filter(|event| event.kind == "config.claude-hook-ensured")
            .count(),
        1
    );
    drop(spine);

    boot_once(&root, &home, &state_dir);
    let spine = SqliteSpine::new(
        pij_store::open(database_path.to_str().expect("UTF-8 database path"))
            .await
            .expect("reopen daemon spine"),
    );
    let second_events = spine
        .tail(None, Seq(0))
        .await
        .expect("tail second-boot events");
    assert_eq!(
        second_events
            .iter()
            .filter(|event| event.kind == "config.claude-inbound-ensured")
            .count(),
        1
    );
    assert_eq!(
        second_events
            .iter()
            .filter(|event| event.kind == "config.claude-hook-ensured")
            .count(),
        1
    );
    drop(spine);
    let _ = fs::remove_dir_all(root);
}

fn boot_once(root: &std::path::Path, home: &std::path::Path, state_dir: &std::path::Path) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve local port");
    let bind = listener.local_addr().expect("local address").to_string();
    drop(listener);
    let mut child = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([
            "--state-dir",
            state_dir.to_str().expect("UTF-8 state path"),
            "daemon",
            "--bind",
            &bind,
        ])
        .env("CLAUDE_CONFIG_DIR", home)
        .env("HOME", root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start isolated daemon");
    let stdout = child.stdout.take().expect("daemon stdout");
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if ready_tx.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut startup = Vec::new();
    let ready = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match ready_rx.recv_timeout(remaining) {
            Ok(line) => {
                let is_ready = line.contains("pij-rs daemon: listening on");
                startup.push(line);
                if is_ready {
                    break true;
                }
            }
            Err(_) => break false,
        }
    };
    if !ready {
        let _ = child.kill();
        let output = child.wait_with_output().expect("collect failed daemon");
        let _ = reader.join();
        panic!(
            "isolated daemon did not become ready; stdout={startup:?}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let interrupted = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("signal isolated daemon");
    assert!(interrupted.success());
    assert!(child.wait().expect("join isolated daemon").success());
    reader.join().expect("join stdout reader");
}
