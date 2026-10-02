use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::{Json, State};
use axum::routing::post;
use pij_core::model::{Envelope, Harness, SeatDescriptor};
use serde_json::{Value, json};

type Requests = Arc<Mutex<Vec<Value>>>;

async fn capture(State(requests): State<Requests>, Json(request): Json<Value>) -> Json<Value> {
    requests.lock().expect("requests").push(request.clone());
    Json(json!(Envelope::ok(
        "pij send",
        json!({
            "msg_id": request["msg_id"], "at":0,
            "outcome":{"outcome":"queued", "reason":"pre-bind", "next_retry_at":0}
        })
    )))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_control_send_and_compact_self_forward_caller_and_empty_body() {
    let dir = pij_testkit::fresh_dir("pij-control-cli");
    std::fs::write(dir.join("daemon.key"), "control-key").expect("key");
    let requests = Requests::default();
    let router = axum::Router::new()
        .route("/v1/send", post(capture))
        .route(
            "/v1/whoami",
            post(|| async {
                Json(Envelope::ok(
                    "pij whoami",
                    SeatDescriptor::new("sender", Harness::Omp, "/abs/tree"),
                ))
            }),
        )
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address").to_string();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    for (args, target) in [
        (
            vec!["send", "--to", "target", "--command", "compact"],
            "target",
        ),
        (vec!["compact-self"], "sender"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
            .args([
                "--state-dir",
                dir.to_str().expect("path"),
                "--addr",
                &addr,
                "--json",
            ])
            .args(args)
            .env_remove("PIJ_SESSION_ID")
            .env("TMUX_PANE", "%control-cli")
            .output()
            .expect("run CLI");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Envelope<Value> =
            serde_json::from_slice(&output.stdout).expect("JSON envelope");
        assert!(response.ok, "{response:?}");
        let request = requests
            .lock()
            .expect("requests")
            .pop()
            .expect("send request");
        assert_eq!(request["command"], "compact");
        assert_eq!(request["body"], "");
        assert_eq!(request["to"]["seat"], target);
        assert_eq!(request["from"], "sender");
        assert_eq!(request["caller"]["TMUX_PANE"], "%control-cli");
    }
    server.abort();
}

#[test]
fn shipped_control_invalid_command_and_body_are_decodable_refusals() {
    let dir = pij_testkit::fresh_dir("pij-control-cli-refusal");
    for args in [
        vec!["send", "--to", "target", "--command", "quit"],
        vec![
            "send",
            "--to",
            "target",
            "--command",
            "compact",
            "--body",
            "also do this",
        ],
        vec![
            "send",
            "--to",
            "target",
            "--command",
            "compact",
            "--body-file",
            "nonexistent",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
            .args([
                "--state-dir",
                dir.to_str().expect("path"),
                "--addr",
                "127.0.0.1:1",
                "--json",
            ])
            .args(args)
            .env_remove("PIJ_SESSION_ID")
            .env_remove("TMUX_PANE")
            .output()
            .expect("run CLI");
        assert!(!output.status.success());
        let response: Envelope<Value> =
            serde_json::from_slice(&output.stdout).expect("JSON refusal");
        assert!(!response.ok);
        assert!(response.meta.unwrap_or_default().contains("E-RS-CONTROL-"));
    }
}
