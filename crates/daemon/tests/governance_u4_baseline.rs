//! HTTP RED witnesses: this file uses no plan139 implementation symbols.
//! Run unchanged on the checkpoint with support/governance_http.rs and contracts.
use serde_json::{Value, json};

#[path = "support/governance_http.rs"]
mod governance_http;
use governance_http::{case, contracts, daemon, post};

async fn value(addr: std::net::SocketAddr, path: &str, request: &Value) -> Value {
    let response = post(addr, path, request).await;
    assert!(
        response.status().is_success(),
        "{path}: {}",
        response.text().await.expect("refusal body")
    );
    response.json().await.expect("envelope")
}

#[tokio::test]
async fn u4_baseline_question_survives_clear_and_self_answer_pushes_nothing() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let request = case(&fixtures, "report-question")["request"].clone();
    let opened = value(addr, "/v1/report", &request).await;
    let decision = &opened["data"]["decision"];
    assert_eq!(
        decision["state"], "open",
        "question must create a durable decision"
    );
    assert_eq!(decision["asked_by"], fixtures["fixture_context"]["worker"]);
    assert_eq!(decision["question_seq"], opened["data"]["seq"]);
    value(
        addr,
        "/v1/report",
        &json!({"argv":["report","clear"],"caller":request["caller"]}),
    )
    .await;
    let listed = value(
        addr,
        "/v1/decisions",
        &case(&fixtures, "decisions-argv")["request"],
    )
    .await;
    assert!(
        listed["data"]["decisions"]
            .as_array()
            .expect("decisions")
            .iter()
            .any(|row| row["id"] == decision["id"])
    );
    let answer =
        json!({"decision":decision["id"],"answer":"self ruling","caller":request["caller"]});
    let answered = value(addr, "/v1/answer", &answer).await;
    assert_eq!(answered["data"]["decision"]["state"], "answered");
    assert_eq!(
        answered["data"]["decision"]["answered_by"],
        decision["asked_by"]
    );
    assert!(answered["data"]["decision"]["answer_msg_id"].is_null());
    let repeated = value(addr, "/v1/answer", &answer).await;
    assert_eq!(
        repeated["data"], answered["data"],
        "identical retry must retain its sequence"
    );
    let mut conflict = answer.clone();
    conflict["answer"] = json!("different ruling");
    let conflicting = post(addr, "/v1/answer", &conflict).await;
    assert!(!conflicting.status().is_success());
    server.abort();
}

#[tokio::test]
async fn u4_baseline_unrelated_answer_refuses_without_closing_question() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let opened = value(
        addr,
        "/v1/report",
        &case(&fixtures, "report-question")["request"],
    )
    .await;
    let id = &opened["data"]["decision"]["id"];
    assert!(
        id.is_string(),
        "report question must return the decision identity"
    );
    let refused = post(addr,"/v1/answer",&json!({"decision":id,"answer":"unauthorized","caller":{"TMUX_PANE":"%12","cwd":"/work/project"}})).await;
    assert_eq!(refused.status(), reqwest::StatusCode::FORBIDDEN);
    let refused: Value = refused.json().await.expect("refusal envelope");
    assert_eq!(refused["details"]["code"], "E-RS-OWNERSHIP");
    let listed = value(
        addr,
        "/v1/decisions",
        &case(&fixtures, "decisions-argv")["request"],
    )
    .await;
    assert_eq!(listed["data"]["decisions"][0]["state"], "open");
    server.abort();
}

#[tokio::test]
async fn u4_baseline_parent_verify_binds_done_and_new_done_reopens_anomaly() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let worker = case(&fixtures, "report-question")["request"]["caller"].clone();
    let done_request = json!({"argv":["report","state","done"],"caller":worker});
    let done = value(addr, "/v1/report", &done_request).await;
    let verified = value(
        addr,
        "/v1/report",
        &case(&fixtures, "report-verify")["request"],
    )
    .await;
    assert_eq!(verified["data"]["done_seq"], done["data"]["seq"]);
    let before = value(
        addr,
        "/v1/anomalies",
        &case(&fixtures, "anomalies-argv")["request"],
    )
    .await;
    assert!(
        !before["data"]["anomalies"]
            .as_array()
            .expect("rows")
            .iter()
            .any(|row| row["kind"] == "unverified-done")
    );
    let later = value(addr, "/v1/report", &done_request).await;
    let after = value(
        addr,
        "/v1/anomalies",
        &case(&fixtures, "anomalies-argv")["request"],
    )
    .await;
    let row = after["data"]["anomalies"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["kind"] == "unverified-done")
        .expect("new done is not verified");
    assert_eq!(row["evidence"][0], later["data"]["seq"]);
    assert!(
        row.get("assignmentId").is_none(),
        "unscoped done must not invent an assignment"
    );
    server.abort();
}

#[tokio::test]
async fn u4_baseline_get_and_post_scope_filters_keep_valid_rows_and_refuse_guessed_here() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let caller = case(&fixtures, "report-question")["request"]["caller"].clone();
    value(
        addr,
        "/v1/report",
        &json!({"argv":["report","state","done"],"caller":caller}),
    )
    .await;
    let client = reqwest::Client::new();
    let unauth = client
        .get(format!("http://{addr}/v1/anomalies"))
        .send()
        .await
        .expect("unauthenticated request");
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);
    let parent = case(&fixtures, "anomalies-argv")["request"]["caller"].clone();
    let post_view = value(
        addr,
        "/v1/anomalies",
        &json!({"argv":["anomalies","--here"],"caller":parent}),
    )
    .await;
    let get_view: Value = client
        .get(format!("http://{addr}/v1/anomalies"))
        .query(&[("here", "/work/project")])
        .bearer_auth("governance-u3-test-key")
        .send()
        .await
        .expect("GET")
        .json()
        .await
        .expect("envelope");
    assert_eq!(get_view, post_view);
    assert_eq!(
        get_view["data"]["anomalies"]
            .as_array()
            .expect("rows")
            .len(),
        1
    );
    let scoped: Value = client
        .get(format!("http://{addr}/v1/anomalies"))
        .query(&[("here", "/other")])
        .bearer_auth("governance-u3-test-key")
        .send()
        .await
        .expect("GET scoped")
        .json()
        .await
        .expect("envelope");
    assert!(
        scoped["data"]["anomalies"]
            .as_array()
            .expect("rows")
            .is_empty()
    );
    let project = value(
        addr,
        "/v1/anomalies",
        &json!({"argv":["anomalies","--project","governance-port"],"caller":parent}),
    )
    .await;
    assert!(
        project["data"]["anomalies"]
            .as_array()
            .expect("rows")
            .is_empty(),
        "unscoped done has no invented project"
    );
    let ambiguous = client
        .get(format!("http://{addr}/v1/anomalies"))
        .query(&[("here", "true")])
        .bearer_auth("governance-u3-test-key")
        .send()
        .await
        .expect("ambiguous GET");
    assert_eq!(ambiguous.status(), reqwest::StatusCode::BAD_REQUEST);
    let refused: Value = ambiguous.json().await.expect("refusal envelope");
    assert_eq!(refused["details"]["code"], "E-RS-ARG");
    assert!(
        refused["meta"]
            .as_str()
            .expect("guidance")
            .contains("unscoped")
    );
    server.abort();
}
