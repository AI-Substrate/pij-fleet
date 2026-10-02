//! Governance HTTP contracts and atomic failure boundaries on a real fresh store.

use std::net::SocketAddr;

use serde_json::{Value, json};

#[path = "support/governance_http.rs"]
mod governance_http;
use governance_http::{case, contracts, daemon, post};

#[tokio::test]
async fn u3_governance_refuses_unknown_flags_and_forged_actor_without_mutation() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    for extra in [
        vec!["--unknown", "discard-me"],
        vec!["--actor", "pij-outsider"],
    ] {
        let mut request = case(&fixtures, "project-create")["request"].clone();
        request["argv"]
            .as_array_mut()
            .expect("argv")
            .extend(extra.into_iter().map(Value::from));
        let response = post(addr, "/v1/project", &request).await;
        assert!(!response.status().is_success());
        let response: Value = response.json().await.expect("decodable refusal");
        assert_eq!(response["ok"], false);
        assert!(
            response["details"]["code"]
                .as_str()
                .expect("refusal code")
                .starts_with("E-RS-")
        );
    }
    let projects: Value = post(
        addr,
        "/v1/project",
        &case(&fixtures, "project-list")["request"],
    )
    .await
    .json()
    .await
    .expect("list");
    assert!(
        projects["data"]["projects"]
            .as_array()
            .expect("projects")
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn u3_every_prime_unset_publishes_its_own_transition_not_an_old_null_record() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let mut previous = 0;
    for id in ["prime-set", "prime-unset", "prime-set", "prime-unset"] {
        let response = post(addr, "/v1/orchestration", &case(&fixtures, id)["request"]).await;
        assert!(response.status().is_success(), "{id}");
        let response: Value = response.json().await.expect("envelope");
        let seq = response["data"]["seq"].as_u64().expect("new sequence");
        assert!(
            seq > previous,
            "new designation/unset may not reuse an older event sequence"
        );
        previous = seq;
    }
    server.abort();
}

#[tokio::test]
async fn u3_metadata_mutations_do_not_invent_a_parent_permission_policy() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let outsider = fixtures["fixture_context"]["outsider"]
        .as_str()
        .expect("outsider");
    let pane = fixtures["fixture_context"]["panes"]
        .as_object()
        .expect("panes")
        .iter()
        .find(|(_, id)| id.as_str() == Some(outsider))
        .expect("outsider pane")
        .0;
    let mut request = case(&fixtures, "attest-plan")["request"].clone();
    request["caller"]["TMUX_PANE"] = json!(pane);
    let response = post(addr, "/v1/attest", &request).await;
    assert!(response.status().is_success());
    let response: Value = response.json().await.expect("attestation");
    assert_eq!(response["data"]["attestation"]["attested_by"], outsider);
    let node: Value = post(addr, "/v1/node", &case(&fixtures, "node-show")["request"])
        .await
        .json()
        .await
        .expect("node");
    assert_eq!(
        node["data"]["node"]["plan_id"],
        response["data"]["attestation"]["plan_id"]
    );
    server.abort();
}

#[tokio::test]
async fn u3_dispatch_hash_and_recipient_ack_are_checked_before_idempotent_replay() {
    let fixtures = contracts();
    let (addr, server, fresh) = daemon(&fixtures).await;
    let packet = std::path::PathBuf::from(fresh.path()).with_extension("packet.json");
    let bytes = serde_json::to_vec(&case(&fixtures, "attest-plan")).expect("fixture packet bytes");
    tokio::fs::write(&packet, &bytes).await.expect("packet");
    let mut request = case(&fixtures, "dispatch-create")["request"].clone();
    request["argv"][3] = json!(packet);
    let response = post(addr, "/v1/dispatch", &request).await;
    assert!(
        response.status().is_success(),
        "dispatch must use the existing durable delivery path"
    );
    let response: Value = response.json().await.expect("dispatch");
    let dispatch = &response["data"]["dispatch"];
    let digest = dispatch["packet_sha256"].as_str().expect("digest");
    assert_eq!(digest.len(), 64);
    assert_eq!(dispatch["msg_id"], dispatch["id"]);
    assert_ne!(
        dispatch["state"], "acked",
        "queue acceptance cannot invent a brief acknowledgement"
    );

    let mut ack = case(&fixtures, "dispatch-ack")["request"].clone();
    ack["argv"][1] = dispatch["id"].clone();
    ack["argv"][3] = json!("0".repeat(64));
    let wrong = post(addr, "/v1/ack", &ack).await;
    assert!(!wrong.status().is_success());
    let wrong: Value = wrong.json().await.expect("sha refusal");
    assert_eq!(
        wrong["details"]["code"],
        fixtures["refusals"]["packet_sha"]["response"]["details"]["code"]
    );
    ack["argv"][3] = json!(digest);
    let first: Value = post(addr, "/v1/ack", &ack).await.json().await.expect("ack");
    assert_eq!(first["ok"], true);
    let replay: Value = post(addr, "/v1/ack", &ack)
        .await
        .json()
        .await
        .expect("replay");
    assert_eq!(replay["data"]["seq"], first["data"]["seq"]);
    assert_eq!(
        replay["data"]["dispatch"]["ack"],
        first["data"]["dispatch"]["ack"]
    );
    ack["caller"] = case(&fixtures, "dispatch-create")["request"]["caller"].clone();
    let wrong_actor = post(addr, "/v1/ack", &ack).await;
    assert!(
        !wrong_actor.status().is_success(),
        "sender cannot replay recipient acknowledgement"
    );
    tokio::fs::remove_file(&packet)
        .await
        .expect("remove owned fixture packet");
    server.abort();
}

fn replace_flag(request: &mut Value, name: &str, value: Value) {
    let argv = request["argv"].as_array_mut().expect("argv");
    if let Some(index) = argv.iter().position(|arg| arg == name) {
        argv[index + 1] = value;
    } else {
        argv.extend([json!(name), value]);
    }
}

async fn grant_fixture_baton(addr: SocketAddr, fixtures: &Value) -> Value {
    let mut request = case(fixtures, "baton-request")["request"].clone();
    let argv = request["argv"].as_array_mut().expect("argv");
    let pin = argv
        .iter()
        .position(|arg| arg == "--pin")
        .expect("optional fixture pin");
    argv.drain(pin..pin + 2);
    let requested: Value = post(addr, "/v1/orchestration", &request)
        .await
        .json()
        .await
        .expect("request");
    assert_eq!(requested["ok"], true);
    let mut grant = case(fixtures, "baton-grant")["request"].clone();
    replace_flag(
        &mut grant,
        "--to",
        requested["data"]["request"]["id"].clone(),
    );
    let granted: Value = post(addr, "/v1/orchestration", &grant)
        .await
        .json()
        .await
        .expect("grant");
    assert_eq!(granted["ok"], true);
    granted["data"]["lease"].clone()
}

async fn destructive_rollback(id: &str) {
    let fixtures = contracts();
    let (addr, server, fresh) = daemon(&fixtures).await;
    let mut request = case(&fixtures, id)["request"].clone();
    if id == "prime-unset" {
        let response = post(
            addr,
            "/v1/orchestration",
            &case(&fixtures, "prime-set")["request"],
        )
        .await;
        assert!(response.status().is_success());
    } else {
        let response = post(
            addr,
            "/v1/orchestration",
            &case(&fixtures, "baton-define")["request"],
        )
        .await;
        assert!(response.status().is_success());
        let lease = grant_fixture_baton(addr, &fixtures).await;
        replace_flag(&mut request, "--lease-id", lease["lease_id"].clone());
    }
    let pool = pij_store::open(&fresh.path())
        .await
        .expect("same private database for fault injection");
    let store = pij_store::SqliteOrchestration::new(pool.clone());
    let baton = fixtures["fixture_context"]["records"]["baton"]["name"]
        .as_str()
        .expect("baton name");
    let lease_before = store.lease(baton).await.expect("lease before");
    let requests_before = store
        .list_baton_requests(baton)
        .await
        .expect("requests before");
    let prime_before = store.prime().await.expect("prime before");
    sqlx::query("CREATE TRIGGER u3_reject_publication BEFORE INSERT ON spine_events BEGIN SELECT RAISE(ABORT, 'injected event publication failure'); END")
        .execute(&pool).await.expect("install private fault");
    let response = post(addr, "/v1/orchestration", &request).await;
    assert!(
        response.status().is_server_error(),
        "{id}: injected event fault must be honest failure"
    );
    assert_eq!(
        store.lease(baton).await.expect("lease after"),
        lease_before,
        "{id}: row mutation rolled back"
    );
    assert_eq!(
        store
            .list_baton_requests(baton)
            .await
            .expect("requests after"),
        requests_before,
        "{id}: request lifecycle rolled back"
    );
    assert_eq!(
        store.prime().await.expect("prime after"),
        prime_before,
        "{id}: designation rolled back"
    );
    sqlx::query("DROP TRIGGER u3_reject_publication")
        .execute(&pool)
        .await
        .expect("remove private fault");
    let retried = post(addr, "/v1/orchestration", &request).await;
    assert!(
        retried.status().is_success(),
        "{id}: same transition remains available after rollback"
    );
    server.abort();
}

#[tokio::test]
async fn u3_baton_return_publication_failure_rolls_back_row_and_request() {
    destructive_rollback("baton-return").await;
}

#[tokio::test]
async fn u3_baton_reclaim_publication_failure_rolls_back_row_and_request() {
    destructive_rollback("baton-reclaim").await;
}

#[tokio::test]
async fn u3_prime_unset_publication_failure_preserves_designation() {
    destructive_rollback("prime-unset").await;
}

#[tokio::test]
async fn u3_old_http_return_cannot_release_a_reacquired_lease_even_for_same_holder() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    assert!(
        post(
            addr,
            "/v1/orchestration",
            &case(&fixtures, "baton-define")["request"]
        )
        .await
        .status()
        .is_success()
    );
    let first = grant_fixture_baton(addr, &fixtures).await;
    let mut request = case(&fixtures, "baton-return")["request"].clone();
    replace_flag(&mut request, "--lease-id", first["lease_id"].clone());
    assert!(
        post(addr, "/v1/orchestration", &request)
            .await
            .status()
            .is_success()
    );
    let second = grant_fixture_baton(addr, &fixtures).await;
    assert_ne!(first["lease_id"], second["lease_id"]);
    let stale: Value = post(addr, "/v1/orchestration", &request)
        .await
        .json()
        .await
        .expect("stale refusal");
    assert_eq!(stale["ok"], false);
    assert_eq!(stale["details"]["code"], "E-RS-LEASE-STALE");
    assert_eq!(stale["details"]["current_lease"], second["lease_id"]);
    let argv = request["argv"].as_array_mut().expect("argv");
    let token = argv
        .iter()
        .position(|arg| arg == "--lease-id")
        .expect("token");
    argv.drain(token..token + 2);
    let missing: Value = post(addr, "/v1/orchestration", &request)
        .await
        .json()
        .await
        .expect("missing token refusal");
    assert_eq!(missing["details"]["code"], "E-RS-LEASE-STALE");
    let shown: Value = post(
        addr,
        "/v1/orchestration",
        &case(&fixtures, "baton-show")["request"],
    )
    .await
    .json()
    .await
    .expect("show");
    assert_eq!(
        shown["data"]["lease"], second,
        "no stale/missing-token request may release the new lease"
    );
    server.abort();
}
