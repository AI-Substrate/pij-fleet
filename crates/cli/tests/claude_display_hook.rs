//! Plan 226: Claude's MessageDisplay hook end to end, the installed managed
//! script exec'ing the real `pij-rs`, with flushes arriving out of order as
//! Claude delivers them (its flushes' hooks run concurrently).

#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pij-display-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).expect("scratch");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pij-rs"), dir.join("bin/pij-rs"))
        .expect("pij-rs on PATH");
    dir
}

fn flush(root: &Path, script: &Path, index: u64, last: bool, delta: &str) -> std::process::Child {
    let mut child = Command::new(script)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("HOME", root)
        .env("PIJ_RS_STATE_DIR", root.join("state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run the hook");
    let input = serde_json::json!({
        "session_id": "sess-7",
        "hook_event_name": "MessageDisplay",
        "message_id": "msg-1",
        "index": index,
        "final": last,
        "delta": delta,
    });
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.to_string().as_bytes())
        .expect("write flush");
    child
}

fn shown(child: std::process::Child) -> Option<String> {
    let output = child.wait_with_output().expect("hook output");
    assert!(output.status.success());
    pij_cli::claude_display::displayed(&String::from_utf8(output.stdout).expect("utf-8"))
}

#[test]
fn the_managed_script_hides_a_closing_block_whose_flushes_arrive_out_of_order() {
    let root = scratch();
    let script = pij_harnesses::install_claude_message_display_script(&root.join("hooks"))
        .expect("install the managed script");

    let closing = flush(
        &root,
        &script,
        1,
        true,
        "send pij-x \"one\"\nsend pij-x \"two\"\n```",
    );
    std::thread::sleep(Duration::from_millis(50));
    let opening = flush(&root, &script, 0, false, "Merged.\n\n```pij\n");

    assert_eq!(shown(opening).as_deref(), Some("Merged.\n\n"));
    assert_eq!(
        shown(closing).as_deref(),
        Some("⚙ pij: 2 commands handed to pij")
    );
    assert!(
        !root.join("state/claude-display/msg-1").exists(),
        "the final flush removes the message's state"
    );
    let _ = std::fs::remove_dir_all(&root);
}
