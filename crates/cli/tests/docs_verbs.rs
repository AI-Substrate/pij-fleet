//! PRD 009 (plan 235): the bundled docs print from the built binary with no
//! daemon, from a directory outside any repository. The address is a closed
//! port and the state directory does not exist, so any daemon contact would
//! fail the call.

use std::path::PathBuf;
use std::process::{Command, Output};

fn pij_rs(args: &[&str]) -> Output {
    let outside = std::env::temp_dir();
    Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args(args)
        .current_dir(&outside)
        .env("PIJ_RS_ADDR", "127.0.0.1:9")
        .env(
            "PIJ_RS_STATE_DIR",
            PathBuf::from(&outside).join("pij-docs-verbs-no-daemon"),
        )
        .output()
        .expect("run pij-rs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf8")
}

fn start_page() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/cli/agents-start-here.md"),
    )
    .expect("read the start page")
}

#[test]
fn the_start_page_list_and_get_print_with_no_daemon() {
    let start = pij_rs(&["agents-start-here"]);
    assert_eq!(start.status.code(), Some(0), "{start:?}");
    assert_eq!(stdout(&start), start_page());

    let list = pij_rs(&["docs", "list"]);
    assert_eq!(list.status.code(), Some(0), "{list:?}");
    assert!(stdout(&list).contains("agents-start-here"));

    let whole = pij_rs(&["docs", "get", "agents-start-here"]);
    assert_eq!(stdout(&whole), start_page());

    let section = pij_rs(&["docs", "get", "agents-start-here#who-governs-you"]);
    assert_eq!(section.status.code(), Some(0), "{section:?}");
    assert!(stdout(&section).starts_with("## Who governs you\n"));
    assert!(!stdout(&section).contains("## First commands"));

    let json = pij_rs(&["--json", "docs", "get", "agents-start-here"]);
    let envelope: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    assert_eq!(
        envelope["data"]["text"].as_str(),
        Some(start_page().as_str())
    );
}

#[test]
fn an_unknown_page_refuses_with_the_nearest() {
    let output = pij_rs(&["docs", "get", "start-here"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        stdout(&output).contains("nearest: `pij docs get agents-start-here`"),
        "{output:?}"
    );
}
