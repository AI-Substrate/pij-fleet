//! `pij-rs statusline copilot` against a REAL daemon (plan 180).
//!
//! The golden corpus (src/statusline_tests.rs) feeds canned whoami text. This
//! file closes the gap: the preserved plan-156 script calls the real
//! `pij-rs --json whoami` and the new subcommand asks the daemon in-process,
//! against the same booted daemon, and the bytes must match. The caller
//! carries an inherited PIJ_SESSION_ID that must never show (plan 156 rule 5).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pij_cli::daemon_config;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/copilot-statusline-plan156.sh"
);

fn have(tool: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// A PATH whose `pij-rs` is this build.
fn bin_dir(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let link = bin.join("pij-rs");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pij-rs"), &link).expect("symlink");
    let _ = std::fs::set_permissions(&link, std::fs::Permissions::from_mode(0o755));
    bin
}

fn run(program: &[&str], root: &Path, env: &[(&str, &str)], payload: &str) -> Vec<u8> {
    let path = format!(
        "{}:{}",
        bin_dir(root).display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new(program[0]);
    command
        .args(&program[1..])
        .env_clear()
        .env("PATH", path)
        .env("HOME", root)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("payload");
    child.wait_with_output().expect("wait").stdout
}

fn both(root: &Path, env: &[(&str, &str)]) -> (String, String) {
    let payload = r#"{"workspace":{"current_dir":"/tmp"},"model":{"display_name":"M"},"context_window":{"current_context_tokens":1500}}"#;
    let old = run(&["/bin/bash", SCRIPT], root, env, payload);
    let new = run(&["pij-rs", "statusline", "copilot"], root, env, payload);
    let show = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).replace('\x1b', "\\e");
    (show(old), show(new))
}

// Multi-thread: the in-process daemon must keep serving while this thread
// blocks on the script and the subcommand.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_subcommand_matches_the_script_against_a_live_and_a_dead_daemon() {
    if !(have("jq") && have("git")) {
        eprintln!("SKIP: the plan-156 script needs jq and git");
        return;
    }
    let root = std::env::temp_dir().join(format!("pij-rs-statusline-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let store = root.join("pij.sqlite");
    let daemon = pij_daemon::boot(&daemon_config("127.0.0.1:0", &store, false), root.clone())
        .await
        .expect("boot");
    let addr = daemon.addr.to_string();
    let state = root.display().to_string();

    // A pane no seat owns, and an inherited id that must not be believed.
    let live = [
        ("PIJ_RS_STATE_DIR", state.as_str()),
        ("PIJ_RS_ADDR", addr.as_str()),
        ("TMUX_PANE", "%9999"),
        ("PIJ_SESSION_ID", "pij-inherited-must-not-show"),
    ];
    let (old, new) = both(&root, &live);
    assert!(!old.is_empty(), "the script rendered something");
    assert_eq!(new, old, "live daemon");
    assert!(!new.contains("pij-inherited-must-not-show"), "{new}");

    // Nothing listening: whatever `pij-rs --json whoami` printed, the same.
    let dead = [
        ("PIJ_RS_STATE_DIR", state.as_str()),
        ("PIJ_RS_ADDR", "127.0.0.1:1"),
        ("TMUX_PANE", "%9999"),
    ];
    let (old, new) = both(&root, &dead);
    assert_eq!(new, old, "dead daemon");

    // A bad address configuration is the same refusal text too.
    let misconfigured = [
        ("PIJ_RS_STATE_DIR", state.as_str()),
        ("PIJ_RS_ADDR", "not an address"),
        ("TMUX_PANE", "%9999"),
    ];
    let (old, new) = both(&root, &misconfigured);
    assert_eq!(new, old, "misconfigured address");

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(&root);
}
