use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use pij_core::config::Config;
use pij_core::model::{Harness, SeatDescriptor, SeatId, Seq};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fresh_dir;

fn run_cli(dir: &Path, addr: &str, seat: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pij-rs"));
    command
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .env("CLAUDE_CONFIG_DIR", dir.join("home/.claude"))
        .env("XDG_CONFIG_HOME", dir.join("home/.config"))
        .env("PIJ_HOME", dir.join("legacy"))
        .env_remove("PIJ_SESSION_ID")
        .env_remove("TMUX_PANE")
        .env_remove("PIJ_PARENT_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("HARNESS_SESSION_ID")
        .env_remove("COPILOT_AGENT_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args(["--state-dir", dir.to_str().unwrap(), "--addr", addr])
        .args(args);
    if let Some(seat) = seat {
        command.env("PIJ_SESSION_ID", seat);
    }
    command.output().expect("run shipped CLI")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_commit_trailers_uses_authoritative_identity_and_keeps_stdout_clean() {
    let dir = fresh_dir("pij-commit-trailers");
    std::fs::create_dir_all(dir.join("home/.claude")).unwrap();
    std::fs::create_dir_all(dir.join("home/.config")).unwrap();
    let git = Command::new("git")
        .current_dir(&dir)
        .env("HOME", dir.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args(["init", "--initial-branch=no-plan"])
        .output()
        .unwrap();
    assert!(git.status.success());
    std::fs::write(dir.join("daemon.key"), "trailer-test-key").unwrap();
    let prime = SeatDescriptor::new("pij-prime", Harness::Omp, dir.to_str().unwrap());
    let mut pm = SeatDescriptor::new("pij-pm", Harness::Omp, dir.to_str().unwrap());
    pm.parent = Some(prime.id.clone());
    let mut coder = SeatDescriptor::new("pij-coder", Harness::Omp, dir.to_str().unwrap());
    coder.parent = Some(pm.id.clone());
    let mut orphan = SeatDescriptor::new("pij-orphan", Harness::Omp, dir.to_str().unwrap());
    orphan.parent = Some("pij-missing".into());
    // A separate role-less chain must still use tier three, not the unrelated prime.
    let chain_root = SeatDescriptor::new("pij-chain-root", Harness::Omp, dir.to_str().unwrap());
    let mut chain_leaf = SeatDescriptor::new("pij-chain-leaf", Harness::Omp, dir.to_str().unwrap());
    chain_leaf.parent = Some(chain_root.id.clone());
    let services = pij_daemon::build_services(&Config::default(), &dir.join("signals"))
        .await
        .unwrap();
    for seat in [prime, pm, coder, orphan, chain_root, chain_leaf] {
        services.registry.put(seat).await.unwrap();
    }
    let prime_id = SeatId::from("pij-prime");
    services
        .roles
        .assert_role(&prime_id, &prime_id, Some("prime".to_string()))
        .await
        .unwrap();
    let registry = Arc::clone(&services.registry);
    let spine = Arc::clone(&services.spine);
    let original_rows = registry.list(Default::default()).await.unwrap();
    let original_events = spine.tail(None, Seq(0)).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    auth: pij_daemon::http::AuthRing::local("trailer-test-key".to_string()),
                    machine_alias: "test-machine".to_string(),
                },
            ),
        )
        .await
        .unwrap();
    });

    for (id, expected, diagnostic) in [
        (
            "pij-prime",
            "Pij-Prime: pij-prime\n",
            "Pij-Prime derivation: role=prime",
        ),
        (
            "pij-coder",
            "Pij-Seat: pij-coder\nPij-Prime: pij-prime\n",
            "Pij-Prime derivation: role=prime",
        ),
        ("pij-orphan", "Pij-Seat: pij-orphan\n", "Pij-Prime omitted"),
        (
            "pij-chain-leaf",
            "Pij-Seat: pij-chain-leaf\nPij-Prime: pij-chain-root\n",
            "Pij-Prime derivation: recorded-parent root (interim)",
        ),
    ] {
        let result = run_cli(&dir, &addr, Some(id), &["--json", "commit-trailers"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(diagnostic),
            "{id}: expected {diagnostic:?}, stderr was {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let missing = run_cli(&dir, &addr, None, &["commit-trailers"]);
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(!missing.stderr.is_empty());
    let bad_config = run_cli(&dir, &addr, None, &["--state-dir", "", "commit-trailers"]);
    assert!(!bad_config.status.success());
    assert!(bad_config.stdout.is_empty());
    assert!(!bad_config.stderr.is_empty());
    assert_eq!(
        registry.list(Default::default()).await.unwrap(),
        original_rows,
        "trailer reads must not mutate registry rows"
    );
    assert_eq!(
        spine.tail(None, Seq(0)).await.unwrap(),
        original_events,
        "trailer reads must not publish mutations"
    );
    server.abort();
}
