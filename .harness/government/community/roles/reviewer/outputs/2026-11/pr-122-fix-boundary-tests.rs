use super::*;

#[tokio::test]
async fn review_duplicate_branch_refuses_before_replacing_any_index() {
    let repo = repo("pr122b-duplicate-rebuild");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let assigned = review_hold(&rig, &repo, "assign", "pm-001", "pij-holder").await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    let before_rows = rig.store.assignments("fleet").await.unwrap();
    let before_pointers = rig.store.community_pointers("fleet").await.unwrap();
    let path = ".harness/government/community/roles/pm-001/holders.dd.json";
    let mut document: Value = serde_json::from_slice(&std::fs::read(repo.gov.join(path)).unwrap()).unwrap();
    let rows = document["sections"][1]["value"].as_array_mut().unwrap();
    rows.push(rows[0].clone());
    std::fs::write(repo.gov.join(path), document.to_string()).unwrap();
    git(&repo.gov, &["add", path]);
    git(&repo.gov, &["commit", "-q", "-m", "duplicate holder regression"]);
    let rebuilt = call(rig.addr, "/v1/directory", &["directory", "rebuild"], &repo.main).await;
    assert_eq!(rebuilt["details"]["code"], "E-RS-DIRECTORY-ROW", "{rebuilt}");
    assert_eq!(rig.store.assignments("fleet").await.unwrap(), before_rows);
    assert_eq!(rig.store.community_pointers("fleet").await.unwrap(), before_pointers);
    rig.server.abort();
}

#[tokio::test]
async fn review_missing_branch_holding_keeps_its_pending_end() {
    let repo = repo("pr122b-missing-holding");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let assigned = review_hold(&rig, &repo, "assign", "pm-001", "pij-holder").await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    let path = ".harness/government/community/roles/pm-001/holders.dd.json";
    let mut document: Value = serde_json::from_slice(&std::fs::read(repo.gov.join(path)).unwrap()).unwrap();
    document["sections"][1]["value"] = json!([]);
    std::fs::write(repo.gov.join(path), document.to_string()).unwrap();
    git(&repo.gov, &["add", path]);
    git(&repo.gov, &["commit", "-q", "-m", "remove holder regression"]);
    let closed = review_close(&rig, &repo, "pij-holder").await;
    assert_eq!(closed["ok"], true, "{closed}");
    let next = review_hold(&rig, &repo, "assign", "pm-001", "pij-second").await;
    assert_eq!(next["ok"], true, "{next}");
    assert_eq!(next["data"]["flushed"], 0, "{next}");
    let stored = rig.store.assignments("fleet").await.unwrap();
    assert!(stored.iter().any(|row| row.seat == "pij-holder" && row.end_pending && row.to.is_some()), "{stored:?}");
    assert_eq!(holders_at_head(&repo, "pm-001").len(), 1);
    rig.server.abort();
}

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

#[tokio::test]
async fn review_reassign_within_one_second_keeps_both_holdings() {
    let repo = repo("pr122-review-fast-reassign");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let previous = pij_daemon::community::holders::now();
    while pij_daemon::community::holders::now() == previous {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let start = std::time::Instant::now();
    let first = review_hold(&rig, &repo, "assign", "pm-001", "pij-holder").await;
    assert_eq!(first["ok"], true, "{first}");
    let released = review_hold(&rig, &repo, "release", "pm-001", "pij-holder").await;
    assert_eq!(released["ok"], true, "{released}");
    let second = review_hold(&rig, &repo, "assign", "pm-001", "pij-holder").await;
    let rows = holders_at_head(&repo, "pm-001");
    eprintln!(
        "elapsed={:?}; second={second}; branch={rows:?}",
        start.elapsed()
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(
        &rows[0]["from"].as_str().unwrap()[..19],
        &rows[1]["from"].as_str().unwrap()[..19],
        "the same-second precondition must hold"
    );
    let indexed = rig.store.assignments("fleet").await.unwrap();
    eprintln!("store before rebuild={indexed:?}");
    let rebuilt = call(
        rig.addr,
        "/v1/directory",
        &["directory", "rebuild"],
        &repo.main,
    )
    .await;
    assert_eq!(rebuilt["ok"], true, "{rebuilt}");
    let rebuilt_rows = rig.store.assignments("fleet").await.unwrap();
    eprintln!("rebuild receipt={rebuilt}; rebuilt store={rebuilt_rows:?}");
    assert_eq!(
        second["ok"], true,
        "valid same-second reassign must succeed: {second}"
    );
    assert_eq!(rig.store.assignments("fleet").await.unwrap().len(), 2);
    rig.server.abort();
}
