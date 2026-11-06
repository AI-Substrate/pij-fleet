// Scratch reproducer at dcc3e526043b2aa3206eae0ea036876183fb9d99.
// Add as tests/pr130_reviewer/mod.rs and declare that module from directory_http.rs.
use super::*;

#[tokio::test]
async fn reviewer_compact_request_replay_does_not_duplicate_routes() {
    let repo = repo("pij-pr130-review-replay");
    let rig = rig(Arc::new(NoHook)).await;
    fleet_with_prime(&rig, &repo).await;
    let added = add_role(&rig, &repo, "delivery", "crates/delivery/**", "Owns sends.").await;
    assert_eq!(added["ok"], true, "{added}");
    let assigned = call(
        rig.addr,
        "/v1/directory",
        &["directory", "assign", "delivery", "pij-holder", "--why", "own it"],
        &repo.main,
    )
    .await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    assert_eq!(fyis(&rig, "pij-holder").await.len(), 1);
    let request = json!({
        "from": SEAT,
        "to": {"seat": "pij-holder"},
        "body": "",
        "command": "compact",
        "msg_id": "reviewer-one-compact",
        "caller": {"tmuxPane": PANE, "cwd": repo.main},
    });
    for attempt in 1..=2 {
        let response: Value = reqwest::Client::new()
            .post(format!("http://{}/v1/send", rig.addr))
            .bearer_auth(KEY)
            .json(&request)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["data"]["msg_id"], "reviewer-one-compact");
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig._fresh.path()))
            .await
            .unwrap();
        let controls: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE dedupe_key = 'reviewer-one-compact'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        pool.close().await;
        let held = fyis(&rig, "pij-holder").await;
        println!("attempt={attempt} controls={controls} held_fyis={}", held.len());
        assert_eq!(controls, 1, "same request queues one compact");
        assert_eq!(held.len(), 2, "one assign FYI and one compact FYI, even on replay");
    }
    rig.server.abort();
}
