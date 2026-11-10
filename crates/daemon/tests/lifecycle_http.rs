//! Baseline-compatible route witnesses: copy with support/governance_http.rs
//! and canonical fixtures to the pre-fix tree. No new lifecycle Rust API used.

use serde_json::{Value, json};

#[path = "support/governance_http.rs"]
mod governance_http;
use governance_http::{case, contracts, daemon, post};

#[tokio::test]
async fn close_parent_tombstones_once_with_canonical_receipt() {
    let fixtures = contracts();
    let (addr, server, _store) = daemon(&fixtures).await;
    let fixture = case(&fixtures, "seat-close");
    let response = post(addr, "/v1/close", &fixture["request"]).await;
    assert_eq!(response.status(), 200, "close route must be served");
    let response: Value = response.json().await.expect("close envelope");
    for field in ["ok", "command", "v"] {
        assert_eq!(response[field], fixture["response"][field]);
    }
    for field in ["seat", "reason"] {
        assert_eq!(response["data"][field], fixture["response"]["data"][field]);
    }
    assert!(
        response["data"]["tombstoned_at"]
            .as_u64()
            .expect("timestamp")
            > 0
    );
    assert!(response["data"]["seq"].as_u64().expect("sequence") > 0);
    let repeated: Value = post(addr, "/v1/close", &fixture["shim_request"])
        .await
        .json()
        .await
        .expect("repeated close envelope");
    assert_eq!(
        repeated, response,
        "retry must return the original tombstone receipt"
    );
    let mut reap = case(&fixtures, "reap-dry-run")["request"].clone();
    reap["dry_run"] = json!(true);
    let after: Value = post(addr, "/v1/reap", &reap)
        .await
        .json()
        .await
        .expect("reap envelope");
    assert_eq!(
        after["data"]["before"], 2,
        "tombstones leave the active roster"
    );
    server.abort();
}

/// F149: a close with the caller's own reason is recorded as an owner's
/// close (`owner-close: <reason>`), the form registration never lifts, and
/// the reply says the process keeps running. F175: this fixture seat records
/// no process, so its pane can't be checked as still its own and the reply
/// names no kill command.
#[tokio::test]
async fn close_records_an_owner_close_and_says_the_process_keeps_running() {
    let fixtures = contracts();
    let (addr, server, _store) = daemon(&fixtures).await;
    let mut request = case(&fixtures, "seat-close")["request"].clone();
    request["reason"] = json!("PR merged");
    let response: Value = post(addr, "/v1/close", &request)
        .await
        .json()
        .await
        .expect("close envelope");
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["reason"], "owner-close: PR merged");
    let process = response["data"]["process"].as_str().unwrap_or_default();
    assert!(
        process.starts_with("pij close retires the seat; its process, if any, keeps running.")
            && process.contains("could not be checked")
            && !process.contains("kill-pane -t %"),
        "{response}"
    );
    server.abort();
}

#[tokio::test]
async fn close_outsider_and_unknown_target_refuse_without_retiring_a_seat() {
    let fixtures = contracts();
    let (addr, server, _store) = daemon(&fixtures).await;
    let mut request = case(&fixtures, "seat-close")["request"].clone();
    request["caller"]["TMUX_PANE"] = json!("%12");
    let response = post(addr, "/v1/close", &request).await;
    assert_eq!(response.status(), 403);
    let refusal: Value = response.json().await.expect("refusal envelope");
    assert_eq!(refusal["ok"], false);
    assert_eq!(refusal["details"]["code"], "E-RS-OWNERSHIP");
    request = case(&fixtures, "seat-close")["request"].clone();
    request["seat"] = json!("pij-missing");
    let response = post(addr, "/v1/close", &request).await;
    assert_eq!(response.status(), 404);
    let response: Value = post(
        addr,
        "/v1/reap",
        &case(&fixtures, "reap-dry-run")["request"],
    )
    .await
    .json()
    .await
    .expect("reap envelope");
    assert_eq!(response["data"]["before"], 3);
    server.abort();
}

#[tokio::test]
async fn close_self_uses_resolved_caller_without_parent_privilege() {
    let fixtures = contracts();
    let (addr, server, _store) = daemon(&fixtures).await;
    let mut request = case(&fixtures, "seat-close")["request"].clone();
    request["caller"]["TMUX_PANE"] = json!("%11");
    let response = post(addr, "/v1/close", &request).await;
    assert_eq!(response.status(), 200);
    let closed: Value = response.json().await.expect("close");
    assert_eq!(closed["ok"], true);
    let tombstone_seq = closed["data"]["seq"].as_u64().expect("tombstone sequence");
    for caller in [
        request["caller"].clone(),
        json!({"PIJ_SESSION_ID":fixtures["fixture_context"]["worker"]}),
    ] {
        let mut retry = request.clone();
        retry["caller"] = caller;
        let retired_self = post(addr, "/v1/close", &retry).await;
        assert_eq!(
            retired_self.status(),
            400,
            "retirement revokes the shared caller identity"
        );
        let refusal: Value = retired_self.json().await.expect("retired-self refusal");
        assert_eq!(refusal["ok"], false);
        assert_eq!(refusal["details"]["tombstone_seq"], tombstone_seq);
        assert!(
            refusal["meta"]
                .as_str()
                .expect("human refusal")
                .contains(&tombstone_seq.to_string())
        );
    }
    let parent_retry = post(addr, "/v1/close", &case(&fixtures, "seat-close")["request"]).await;
    assert_eq!(
        parent_retry.status(),
        200,
        "live parent can read the prior receipt"
    );
    assert_eq!(
        parent_retry.json::<Value>().await.expect("parent receipt"),
        closed
    );
    server.abort();
}

#[tokio::test]
async fn lifecycle_unknown_flags_and_forged_actor_are_not_silently_accepted() {
    let fixtures = contracts();
    let (addr, server, _store) = daemon(&fixtures).await;
    let caller = case(&fixtures, "seat-close")["request"]["caller"].clone();
    for (path, argv) in [
        ("/v1/close", json!(["close", "pij-worker", "--force"])),
        (
            "/v1/close",
            json!(["close", "pij-worker", "--actor", "pij-outsider"]),
        ),
        ("/v1/reap", json!(["reap", "--all"])),
        ("/v1/reap", json!(["reap", "pij-worker"])),
    ] {
        let response = post(addr, path, &json!({"argv":argv,"caller":caller})).await;
        assert!(!response.status().is_success());
        assert_eq!(
            response.json::<Value>().await.expect("decodable refusal")["ok"],
            false
        );
    }
    let response: Value = post(
        addr,
        "/v1/reap",
        &case(&fixtures, "reap-dry-run")["request"],
    )
    .await
    .json()
    .await
    .expect("reap envelope");
    assert_eq!(response["data"]["before"], 3);
    server.abort();
}

/// F114: these seats never registered a process, and their recorded panes are
/// not among tmux's panes, so the pane's absence decides: a dry run lists all
/// three as never-bound candidates and changes nothing, and a run retires
/// them. (Before F114 a missing process identity was unverifiable forever.)
#[tokio::test]
async fn reap_never_bound_seats_are_decided_by_their_recorded_pane() {
    let fixtures = contracts();
    let (addr, server, _store) = daemon(&fixtures).await;
    let fixture = case(&fixtures, "reap-dry-run");
    let response = post(addr, "/v1/reap", &fixture["request"]).await;
    assert_eq!(response.status(), 200, "reap route must be served");
    let dry: Value = response.json().await.expect("dry-run envelope");
    for field in ["ok", "command", "v"] {
        assert_eq!(dry[field], fixture["response"][field]);
    }
    assert_eq!(dry["data"]["before"], 3);
    assert_eq!(dry["data"]["after"], 3);
    let candidates = dry["data"]["candidates"].as_array().expect("rows");
    assert_eq!(candidates.len(), 3, "{dry}");
    assert!(candidates.iter().all(|row| row["process"] == "never-bound"
        && row["pane"] == "absent"
        && row["reason"] == "never-bound-and-pane-absent"));
    assert_eq!(dry["data"]["reaped"], json!([]));
    assert_eq!(dry["data"]["unverifiable"], json!([]));
    let mut request = fixture["shim_request"].clone();
    request["argv"] = json!(["reap"]);
    let actual: Value = post(addr, "/v1/reap", &request)
        .await
        .json()
        .await
        .expect("actual-run envelope");
    assert_eq!(actual["data"]["dry_run"], false);
    assert_eq!(actual["data"]["candidates"], dry["data"]["candidates"]);
    assert_eq!(actual["data"]["reaped"].as_array().expect("rows").len(), 3);
    assert_eq!(actual["data"]["after"], 0);
    server.abort();
}
