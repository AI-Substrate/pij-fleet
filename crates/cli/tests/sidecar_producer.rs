//! The SHIPPED client driven against the SHIPPED sidecar route.
//!
//! Wave-7 review finding F12. `POST /v1/sidecar` returned a bare JSON body while
//! every other route returns the v1 envelope, so `DaemonClient::sidecar` refused
//! the reply with "JSON is not a envelope (missing field `ok`)" — and the job had
//! already been enqueued. The operator was told FAILED about work that landed:
//! command-success-is-not-effect running backwards, a confident failure over an
//! effect that did happen.
//!
//! It survived a green gate because nothing drove the client against the route:
//! `grep .sidecar(` returned the declaration and no caller. Two asserted halves
//! with no instrument between them are not checked (COMMON 1.10), and this file
//! is that instrument.

use pij_cli::{DaemonClient, daemon_config};

/// Mutation witness: return a bare `Json(..)` body from the `sidecar` handler
/// instead of `envelope(.., Envelope::ok(..))` and this fails on `ok`.
#[tokio::test]
async fn the_shipped_client_can_complete_one_sidecar_call() {
    let state_dir = std::env::temp_dir().join(format!("pij-rs-sidecar-{}", std::process::id()));
    std::fs::create_dir_all(&state_dir).expect("state dir");
    let store_path = state_dir.join("pij.sqlite");
    let config = daemon_config("127.0.0.1:0", &store_path, false);
    let daemon = pij_daemon::boot(&config, state_dir.clone())
        .await
        .expect("boot");

    let client = DaemonClient::new(&state_dir, &daemon.addr.to_string()).expect("client");
    let reply = client
        .sidecar(&serde_json::json!({
            "sidecar": "chore",
            "request": {"action": "run", "target": "pij-test"}
        }))
        .await;

    assert!(
        reply.ok,
        "the shipped client must decode the shipped route: {reply:?}"
    );
    let data = reply
        .data
        .expect("an accepted job reports what it enqueued");
    assert!(
        data.get("serial_key").is_some() && data.get("job_id").is_some(),
        "an accepted sidecar job must name the row a consumer will claim: {data}"
    );

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(&state_dir);
}
