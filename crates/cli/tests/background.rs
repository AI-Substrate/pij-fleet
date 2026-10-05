//! Native bg leaves share the daemon's argv grammar and exact v2 wire envelope.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::{Json, State};
use axum::http::HeaderMap;
use axum::routing::post;
use pij_core::model::Envelope;
use serde_json::Value;

#[derive(Clone)]
struct Fixture {
    response: Envelope<Value>,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn capture(
    State(fixture): State<Fixture>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Json<Envelope<Value>> {
    assert_eq!(headers["authorization"], "Bearer bg-cli-key");
    fixture.requests.lock().expect("requests").push(request);
    Json(fixture.response)
}

async fn exercise(leaf: &str, args: &[&str]) {
    let dir = pij_testkit::fresh_dir("pij-bg-cli")
        .canonicalize()
        .expect("canonical fixture directory");
    std::fs::write(dir.join("daemon.key"), "bg-cli-key").expect("key");
    let golden_name = format!("cli/bg-{leaf}-envelope.json");
    let expected = pij_testkit::fixtures::read(&format!("golden/{golden_name}"));
    let response: Envelope<Value> = serde_json::from_str(&expected).expect("golden envelope");
    let human = response.data.as_ref().expect("data")["line"]
        .as_str()
        .expect("line")
        .to_string();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new()
        .route("/v1/bg", post(capture))
        .with_state(Fixture {
            response,
            requests: requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address").to_string();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let mut base = vec!["bg", leaf];
    base.extend_from_slice(args);
    for json_at in [Some(0), Some(1), Some(2), Some(base.len()), None] {
        let mut argv = base.clone();
        if let Some(at) = json_at {
            argv.insert(at, "--json");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
            .args(["--state-dir", dir.to_str().expect("path"), "--addr", &addr])
            .args(&argv)
            .current_dir(&dir)
            .env("PIJ_SESSION_ID", "pij-bg-owner")
            .env("TMUX_PANE", "%bg-cli")
            .env("PIJ_PARENT_ID", "pij-bg-parent")
            .env("HARNESS_SESSION_ID", "bg-native-session")
            .output()
            .expect("run bg CLI");
        assert!(
            output.status.success(),
            "{argv:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
        if json_at.is_some() {
            pij_testkit::golden::assert_golden(
                &golden_name,
                stdout.strip_suffix('\n').expect("newline"),
            );
        } else {
            assert_eq!(stdout, format!("{human}\n"));
        }
        let request = requests
            .lock()
            .expect("requests")
            .pop()
            .expect("bg request");
        assert_eq!(request.as_object().expect("request").len(), 2);
        let forwarded: Vec<&str> = request["argv"]
            .as_array()
            .expect("argv")
            .iter()
            .map(|arg| arg.as_str().expect("argument"))
            .filter(|arg| *arg != "--json")
            .collect();
        assert_eq!(forwarded, base);
        assert_eq!(request["caller"]["PIJ_SESSION_ID"], "pij-bg-owner");
        assert_eq!(request["caller"]["TMUX_PANE"], "%bg-cli");
        assert_eq!(request["caller"]["PIJ_PARENT_ID"], "pij-bg-parent");
        assert_eq!(request["caller"]["HARNESS_SESSION_ID"], "bg-native-session");
        assert_eq!(request["caller"]["cwd"], dir.to_str().expect("cwd"));
        assert!(request.get("owner").is_none());
    }
    server.abort();
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_create_forwards_caller_and_matches_envelope_golden() {
    exercise(
        "create",
        &[
            "--title",
            "shell output",
            "--command",
            "printf '%s\\n' '--json'; echo done",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_list_forwards_all_and_matches_envelope_golden() {
    exercise("list", &["--all"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_tail_forwards_lines_and_matches_envelope_golden() {
    exercise("tail", &["bg-golden", "--lines", "2"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_kill_forwards_job_and_matches_envelope_golden() {
    exercise("kill", &["bg-golden"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_create_forwards_cwd_and_timeout_to_the_daemon_parser() {
    exercise(
        "create",
        &[
            "--title",
            "shell output",
            "--cwd",
            "sub",
            "--timeout",
            "1h30m",
            "--command",
            "true",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_create_from_a_deleted_cwd_refuses_unless_an_absolute_cwd_is_given() {
    // Review F2 (PR #21): the native CLI turned an unreadable cwd into "no cwd",
    // and the daemon then ran the command in the owner's recorded folder.
    let dir = pij_testkit::fresh_dir("pij-bg-gone-cwd")
        .canonicalize()
        .expect("canonical fixture directory");
    std::fs::write(dir.join("daemon.key"), "bg-cli-key").expect("key");
    let golden = pij_testkit::fixtures::read("golden/cli/bg-create-envelope.json");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new()
        .route("/v1/bg", post(capture))
        .with_state(Fixture {
            response: serde_json::from_str(&golden).expect("golden envelope"),
            requests: requests.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address").to_string();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let gone = dir.join("gone");
    let run = |extra: &[&str]| {
        Command::new("/bin/sh")
            .args([
                "-c",
                r#"mkdir "$1" && cd "$1" && rmdir "$1" && shift && exec "$@""#,
            ])
            .arg("sh")
            .arg(&gone)
            .arg(env!("CARGO_BIN_EXE_pij-rs"))
            .args(["--state-dir", dir.to_str().expect("path"), "--addr", &addr])
            .args(["bg", "create", "--title", "t"])
            .args(extra)
            .args(["--command", "pwd"])
            .env("PIJ_SESSION_ID", "pij-bg-owner")
            .output()
            .expect("run bg CLI")
    };
    let refused = run(&[]);
    let stdout = String::from_utf8_lossy(&refused.stdout);
    assert!(!refused.status.success(), "must refuse: {stdout}");
    assert!(stdout.contains("absolute --cwd"), "{stdout}");
    assert!(
        requests.lock().expect("requests").is_empty(),
        "nothing reaches the daemon"
    );
    let absolute = dir.to_str().expect("path");
    let accepted = run(&["--cwd", absolute]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let request = requests
        .lock()
        .expect("requests")
        .pop()
        .expect("bg request");
    assert!(
        request["argv"]
            .as_array()
            .expect("argv")
            .windows(2)
            .any(|pair| pair[0] == "--cwd" && pair[1] == absolute),
        "{request}"
    );
    server.abort();
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bg_emit_fires_with_the_job_token_and_needs_no_daemon_key() {
    async fn hook(
        axum::extract::Path(job): axum::extract::Path<String>,
        headers: HeaderMap,
        State(seen): State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        seen.lock().expect("seen").push(serde_json::json!({
            "job": job,
            "auth": headers["authorization"].to_str().expect("header"),
            "body": body,
        }));
        Json(serde_json::json!({
            "ok": true, "command": "pij bg emit", "v": 2,
            "data": {"job": job, "seq": 1, "line": "event 1 fired for bg-src"}
        }))
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let router = axum::Router::new()
        .route("/v1/bg/{job}/emit", post(hook))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address").to_string();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    // No daemon.key anywhere: the hook's only credential is the job token.
    let empty = pij_testkit::fresh_dir("pij-bg-emit-nokey");
    let run = |env: &[(&str, &str)], args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pij-rs"));
        command
            .args([
                "--state-dir",
                empty.to_str().expect("path"),
                "--addr",
                &addr,
            ])
            .args(["bg", "emit"])
            .args(args)
            .env_remove("PIJ_BG_JOB")
            .env_remove("PIJ_BG_TOKEN");
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().expect("run bg emit")
    };
    let outside = run(&[], &["hello"]);
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stdout).contains("PIJ_BG_JOB and PIJ_BG_TOKEN"));
    let fired = run(
        &[("PIJ_BG_JOB", "bg-src"), ("PIJ_BG_TOKEN", "secret")],
        &["--data", "{\"rows\":3}", "new rows"],
    );
    assert!(
        fired.status.success(),
        "{}",
        String::from_utf8_lossy(&fired.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&fired.stdout),
        "event 1 fired for bg-src\n"
    );
    assert_eq!(
        seen.lock().expect("seen").pop().expect("one emit"),
        serde_json::json!({
            "job": "bg-src",
            "auth": "Bearer secret",
            "body": {"text": "new rows", "data": {"rows": 3}},
        })
    );
    let bad = run(
        &[("PIJ_BG_JOB", "bg-src"), ("PIJ_BG_TOKEN", "secret")],
        &["--data", "{not json", "x"],
    );
    assert!(!bad.status.success());
    server.abort();
    std::fs::remove_dir_all(empty).expect("cleanup");
}
