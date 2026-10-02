//! Behavioural RED witnesses using only APIs that exist before plan139.
//! Copy this file, support/governance_http.rs, and canonical fixtures to the
//! pre-fix worktree; no new governance Rust types or methods are referenced.

use serde_json::{Value, json};

#[path = "support/governance_http.rs"]
mod governance_http;
use governance_http::{case, contracts, daemon, post};

#[tokio::test]
async fn u3_baseline_project_create_is_readable_without_invented_metadata() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let created = post(
        addr,
        "/v1/project",
        &case(&fixtures, "project-create")["request"],
    )
    .await;
    assert!(
        created.status().is_success(),
        "project-create route must exist"
    );
    let created: Value = created.json().await.expect("create envelope");
    assert_eq!(created["ok"], true);
    let expected = case(&fixtures, "project-create");
    for field in ["slug", "description", "plan_path", "prime_id", "created_by"] {
        assert_eq!(
            created["data"]["project"][field], expected["response"]["data"]["project"][field],
            "{field}"
        );
    }
    let readback: Value = post(
        addr,
        "/v1/project",
        &case(&fixtures, "project-show")["request"],
    )
    .await
    .json()
    .await
    .expect("show envelope");
    assert_eq!(readback["data"]["project"], created["data"]["project"]);
    assert!(created["data"]["seq"].as_u64().expect("sequence") > 0);
    server.abort();
}

#[tokio::test]
async fn u3_baseline_spine_append_is_visible_to_exclusive_history_and_render() {
    let fixtures = contracts();
    let (addr, server, _fresh) = daemon(&fixtures).await;
    let appended = post(
        addr,
        "/v1/spine",
        &case(&fixtures, "spine-append")["request"],
    )
    .await;
    assert!(
        appended.status().is_success(),
        "spine-append route must exist"
    );
    let appended: Value = appended.json().await.expect("append envelope");
    let seq = appended["data"]["seq"].as_u64().expect("sequence");
    let expected = case(&fixtures, "spine-append");
    assert_eq!(
        appended["data"]["event"]["kind"],
        expected["response"]["data"]["event"]["kind"]
    );
    let mut request = case(&fixtures, "spine-events")["request"].clone();
    request["argv"][3] = json!((seq - 1).to_string());
    let history: Value = post(addr, "/v1/spine", &request)
        .await
        .json()
        .await
        .expect("history");
    assert_eq!(
        history["data"]["events"].as_array().expect("events").len(),
        1
    );
    assert_eq!(history["data"]["events"][0]["seq"], seq);
    request["argv"][3] = json!(seq.to_string());
    let after: Value = post(addr, "/v1/spine", &request)
        .await
        .json()
        .await
        .expect("exclusive history");
    assert!(
        after["data"]["events"]
            .as_array()
            .expect("events")
            .is_empty()
    );
    let rendered: Value = post(
        addr,
        "/v1/spine",
        &case(&fixtures, "spine-render")["request"],
    )
    .await
    .json()
    .await
    .expect("render");
    assert!(
        rendered["data"]["text"]
            .as_str()
            .expect("text")
            .contains(&format!("{seq} plan139.control"))
    );
    server.abort();
}
