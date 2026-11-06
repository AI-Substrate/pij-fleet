use super::*;

async fn review_close(rig: &Rig, repo: &Repo, seat: &str) -> Value {
    reqwest::Client::new()
        .post(format!("http://{}/v1/close", rig.addr))
        .bearer_auth(KEY)
        .json(&json!({"seat": seat, "reason": "done", "caller": {"TMUX_PANE": PANE, "cwd": repo.main}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn review_hold(rig: &Rig, repo: &Repo, verb: &str, role: &str, seat: &str) -> Value {
    call(
        rig.addr,
        "/v1/directory",
        &["directory", verb, role, seat, "--why", "review regression"],
        &repo.main,
    )
    .await
}

#[tokio::test]
async fn review_assign_same_role_flushes_a_foreign_edit_blocked_close() {
    let repo = repo("pr122-review-pending-assign");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let assigned = review_hold(&rig, &repo, "assign", "pm-001", "pij-holder").await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    let path = repo
        .gov
        .join(".harness/government/community/roles/pm-001/holders.dd.json");
    let original = std::fs::read(&path).unwrap();
    let mut dirty = original.clone();
    dirty.push(b'\n');
    std::fs::write(&path, &dirty).unwrap();
    let closed = review_close(&rig, &repo, "pij-holder").await;
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(closed["data"]["tombstoned_at"].is_number(), "{closed}");
    assert_eq!(
        closed["data"]["directory"]["pending"][0]["code"], "E-RS-GOV-FOREIGN-EDIT",
        "{closed}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), dirty);
    std::fs::write(&path, original).unwrap();
    let next = review_hold(&rig, &repo, "assign", "pm-001", "pij-second").await;
    assert_eq!(next["ok"], true, "{next}");
    assert_eq!(next["data"]["flushed"], 1, "{next}");
    let stored = rig.store.assignments("fleet").await.unwrap();
    let ended = stored.iter().find(|row| row.seat == "pij-holder").unwrap();
    assert!(ended.to.is_some() && !ended.end_pending, "{stored:?}");
    let on_branch = holders_at_head(&repo, "pm-001");
    eprintln!("assign receipt: {next}; branch: {on_branch:?}; store: {stored:?}");
    assert_eq!(
        on_branch[0]["release_why"], "close",
        "same-role assign must really flush the pending close before clearing its store flag"
    );
    rig.server.abort();
}

#[tokio::test]
async fn review_release_same_role_flushes_another_holders_pending_close() {
    let repo = repo("pr122-review-pending-release");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    for seat in ["pij-holder", "pij-second"] {
        let assigned = review_hold(&rig, &repo, "assign", "pm-001", seat).await;
        assert_eq!(assigned["ok"], true, "{assigned}");
    }
    let path = repo
        .gov
        .join(".harness/government/community/roles/pm-001/holders.dd.json");
    let original = std::fs::read(&path).unwrap();
    let mut dirty = original.clone();
    dirty.push(b'\n');
    std::fs::write(&path, dirty).unwrap();
    let closed = review_close(&rig, &repo, "pij-holder").await;
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(
        closed["data"]["directory"]["pending"][0]["code"],
        "E-RS-GOV-FOREIGN-EDIT"
    );
    std::fs::write(&path, original).unwrap();
    let next = review_hold(&rig, &repo, "release", "pm-001", "pij-second").await;
    assert_eq!(next["ok"], true, "{next}");
    assert_eq!(next["data"]["flushed"], 1, "{next}");
    let on_branch = holders_at_head(&repo, "pm-001");
    assert_eq!(on_branch[1]["release_why"], "review regression");
    assert_eq!(
        on_branch[0]["release_why"], "close",
        "same-role release must flush other holders' pending ends"
    );
    rig.server.abort();
}
