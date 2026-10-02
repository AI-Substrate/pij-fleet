//! Baseline-compatible CLI witness: before the cutover, unknown close/reap
//! fail as actual process outcomes, not compile errors. The HTTP peer is private.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;

#[derive(Clone)]
struct Fixture {
    seen: Arc<Mutex<Vec<Value>>>,
    answer: Value,
    status: StatusCode,
}

async fn reply(
    State(state): State<Fixture>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(
        headers.get("authorization").expect("bearer"),
        "Bearer lifecycle-key"
    );
    state.seen.lock().expect("requests").push(body);
    (state.status, Json(state.answer))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_lifecycle_cli_forwards_canonical_argv_and_preserves_entire_envelope() {
    let contract: Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .expect("canonical routes");
    for path in ["/v1/close", "/v1/reap"] {
        let case = &contract["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .find(|route| route["path"] == path)
            .expect("lifecycle route")["cases"][0];
        let args: Vec<&str> = case["shim_request"]["argv"]
            .as_array()
            .expect("argv")
            .iter()
            .map(|arg| arg.as_str().expect("argument"))
            .collect();
        for (status, mut answer) in [
            (StatusCode::OK, case["response"].clone()),
            (
                StatusCode::FORBIDDEN,
                contract["refusals"]["ownership"]["response"].clone(),
            ),
        ] {
            answer["command"] = case["response"]["command"].clone();
            let state_dir = pij_testkit::fresh_dir("pij-lifecycle-cli");
            std::fs::write(state_dir.join("daemon.key"), "lifecycle-key").expect("private key");
            let seen = Arc::new(Mutex::new(Vec::new()));
            let app = Router::new().route(path, post(reply)).with_state(Fixture {
                seen: seen.clone(),
                answer: answer.clone(),
                status,
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("private listener");
            let address = listener.local_addr().expect("address");
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.expect("serve");
            });
            let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
                .args([
                    "--state-dir",
                    state_dir.to_str().expect("path"),
                    "--addr",
                    &address.to_string(),
                    "--json",
                ])
                .args(&args)
                .env_remove("PIJ_SESSION_ID")
                .env_remove("PI_SESSION_ID")
                .env(
                    "TMUX_PANE",
                    case["shim_request"]["caller"]["TMUX_PANE"]
                        .as_str()
                        .expect("pane"),
                )
                .env("HOME", &state_dir)
                .env("CLAUDE_CONFIG_DIR", state_dir.join(".claude"))
                .env("XDG_CONFIG_HOME", state_dir.join(".config"))
                .output()
                .expect("shipped lifecycle CLI");
            server.abort();
            std::fs::remove_dir_all(&state_dir).expect("remove owned fixture");
            assert_eq!(
                output.status.success(),
                status.is_success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let actual: Value =
                serde_json::from_slice(&output.stdout).expect("complete CLI envelope");
            assert_eq!(
                actual, answer,
                "native command must preserve every daemon envelope field"
            );
            let requests = seen.lock().expect("requests");
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["argv"], case["shim_request"]["argv"]);
        }
    }
}
