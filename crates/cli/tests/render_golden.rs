//! Computed response payloads are byte-goldened; editorial help is not.

use pij_cli::render;
use pij_core::model::{
    DeliveryOutcome, Envelope, Harness, ProcIdentity, Receipt, SeatDescriptor, SystemState,
};
use pij_testkit::golden;

fn registered_seat() -> SeatDescriptor {
    let mut seat = SeatDescriptor::new("pij-cli-golden", Harness::Omp, "/work/pij");
    seat.extension_build = Some("0123456789+dirty".to_string());
    seat.extension_path = Some("/work/pij/.pi/extensions/pij".to_string());
    seat.pane = Some("%42".to_string());
    seat.proc = Some(ProcIdentity {
        pid: 4242,
        proc_start: 20260830123456,
    });
    seat.spawn_id = Some("s-golden".to_string());
    seat.model = Some("github-copilot/gpt-5.6-sol-fast".to_string());
    seat.provider = Some("github-copilot".to_string());
    seat.effort = Some("high".to_string());
    seat
}

#[test]
fn register_computed_payload_matches_the_committed_golden() {
    let actual = render(&Envelope::ok("pij register", registered_seat()), true);

    golden::assert_golden("cli/register-envelope.json", &actual);
}

#[test]
fn send_computed_payload_matches_the_committed_golden() {
    let actual = render(
        &Envelope::ok(
            "pij send",
            Receipt {
                msg_id: "m-golden".to_string(),
                outcome: DeliveryOutcome::Queued {
                    reason: None,
                    next_retry_at: None,
                    draft_sha: None,
                },
                at: 1_787_999_999_000,
                cold_check: None,
                warning: None,
            },
        ),
        true,
    );

    golden::assert_golden("cli/send-envelope.json", &actual);
}

/// Plan 160: `pij list`'s human table. The TS shim renders the same golden
/// from the same JSON (`.omp/extensions/pij/core/roster-table.test.ts`).
#[test]
fn the_sized_roster_table_matches_the_committed_golden() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/cli/list-sized.json"
    ))
    .expect("fixture");
    assert_eq!(
        pij_cli::roster::render_table(&data),
        include_str!("../../testkit/fixtures/golden/cli/list-sized.txt")
    );
}

#[test]
fn list_computed_payload_matches_the_committed_golden() {
    let first = registered_seat();
    let mut second = SeatDescriptor::new("pij-paneless", Harness::Claude, "/work/other");
    second.state = SystemState::Working;
    second.relay = true;
    let actual = render(&Envelope::ok("pij list", vec![first, second]), true);

    golden::assert_golden("cli/list-envelope.json", &actual);
}

#[tokio::test]
async fn shipped_json_preserves_validated_bytes_and_never_refetches() {
    for (body, expected_error) in [
        (
            r#"{ "future": {"z":1,"a":2}, "v":2, "command":"pij role", "ok":true, "data":{"z":1e3,"a":"\u0058"} }"#,
            None,
        ),
        (
            "\n {\"ok\":true,\"command\":\"pij role\",\"v\":2,\"data\":{\"z\":1,\"a\":2}} \n",
            None,
        ),
        (
            " {\"future\":true,\"ok\":false,\"command\":\"pij role\",\"v\":2,\"error\":\"refused\",\"details\":{\"z\":1,\"a\":2},\"meta\":\"no\"}\n\n",
            None,
        ),
        (
            r#"{"ok":true,"command":"pij role","v":99,"data":"must not be echoed"}"#,
            Some("skew"),
        ),
        ("not a JSON envelope", Some("adapter")),
    ] {
        let dir = pij_testkit::fresh_dir("pij-cli-wire-bytes");
        std::fs::write(dir.join("daemon.key"), "private-wire-key").expect("private key");
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let capture = requests.clone();
        let router = axum::Router::new().route(
            "/v1/role",
            axum::routing::post(move || {
                let capture = capture.clone();
                async move {
                    capture.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("private bind");
        let addr = listener.local_addr().expect("private address").to_string();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("serve fixture");
        });
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pij-rs"));
        command
            .args([
                "--json",
                "--state-dir",
                dir.to_str().expect("path"),
                "--addr",
                &addr,
                "role",
                "pij-worker",
                "reviewer",
            ])
            .current_dir(&dir)
            .env_clear()
            .env("PATH", &dir)
            .env("HOME", &dir)
            .env("CLAUDE_CONFIG_DIR", dir.join("claude"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("PIJ_SESSION_ID", "pij-parent")
            .env("TMUX_PANE", "%private-wire")
            .kill_on_drop(true);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(10), command.output()).await;
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(&dir).expect("remove owned fixture");
        let output = result.expect("CLI deadline").expect("run shipped CLI");
        assert_eq!(
            requests.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "one response buffer, no re-fetch"
        );
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("JSON stdout");
        assert_eq!(
            output.status.success(),
            envelope["ok"].as_bool().expect("ok boolean")
        );
        if let Some(error) = expected_error {
            assert_eq!(envelope["ok"], false);
            assert_eq!(envelope["v"], 2);
            assert_eq!(envelope["error"], error);
        } else {
            let expected = if body.ends_with('\n') {
                body.to_owned()
            } else {
                format!("{body}\n")
            };
            assert_eq!(output.stdout, expected.as_bytes());
        }
    }
}
