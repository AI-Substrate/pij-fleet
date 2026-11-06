// Append these tests to crates/daemon/tests/claims_http.rs in an immutable
// scratch copy of a00a4cf. They reuse that file's Rig, PR fixtures and helpers.

#[tokio::test]
async fn review_unreadable_checkout_refuses_without_allocating_a_number() {
    use std::os::unix::fs::PermissionsExt;

    let rig = rig("pij-review-floor", Vec::new()).await;
    let plans = rig.main.join("docs/plans");
    std::fs::create_dir_all(plans.join("001-existing")).unwrap();
    let permissions = std::fs::metadata(&plans).unwrap().permissions();
    std::fs::set_permissions(&plans, std::fs::Permissions::from_mode(0)).unwrap();
    let inaccessible = std::fs::read_dir(&plans).is_err();
    let result = rig
        .call(CODER, "/v1/claim", &["claim", "plan", "--for", "must not collide"])
        .await;
    std::fs::set_permissions(&plans, permissions).unwrap();
    assert!(inaccessible, "the permission-error control must actually fail");
    assert_eq!(result["ok"], false, "an unreadable scan cannot establish the floor: {result}");
    assert!(rig.store.claims("demo").await.unwrap().is_empty());
}

#[tokio::test]
async fn review_unreadable_delivered_by_does_not_offer_an_overwriting_edit() {
    use std::os::unix::fs::PermissionsExt;

    let rig = rig("pij-review-unreadable-cell", vec![pr(150, "MERGED", &body(), &FILES)]).await;
    let path = rig.main.join(PRD);
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
    let inaccessible = std::fs::read_to_string(&path).is_err();
    let result = rig.call(OTHER, "/v1/delivered", &["delivered", "150"]).await;
    std::fs::set_permissions(&path, permissions).unwrap();
    assert!(inaccessible, "the permission-error control must actually fail");
    let unsafe_edit = result["data"]["edits"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|edit| {
            edit["address"] == format!("{PRD}#requirements/req-20/delivered_by")
                && !edit["value"].as_str().unwrap().split(',').any(|pr| pr.trim() == "#110")
        });
    assert!(!unsafe_edit, "cannot safely replace the unreadable #110 cell: {result}");
}

#[tokio::test]
async fn review_replaying_with_a_different_pm_reports_the_actual_recipient() {
    let rig = rig("pij-review-replay-pm", vec![pr(151, "MERGED", &body(), &FILES)]).await;
    let first = rig
        .call(OTHER, "/v1/delivered", &["delivered", "151", "--pm", PM.0])
        .await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(rig.card(PM.0).await["data"]["deliveries"][0]["pr"], 151);
    let replay = rig
        .call(OTHER, "/v1/delivered", &["delivered", "151", "--pm", OTHER.0])
        .await;
    let line = replay["data"]["line"].as_str().unwrap();
    assert_eq!(replay["data"]["earlier_pm"], PM.0, "{replay}");
    assert_eq!(rig.card(OTHER.0).await["data"]["deliveries"], json!([]));
    assert!(
        !line.contains(&format!("{}'s card was told", OTHER.0)),
        "the retry must not claim a different, empty card was told: {replay}"
    );
}

#[tokio::test]
async fn review_pending_docs_edits_preserve_all_recorded_prs_for_a_requirement() {
    let body = format!("Delivers: {PRD}#requirements/req-20");
    let rig = rig(
        "pij-review-multiple-receipts",
        vec![pr(153, "MERGED", &body, &FILES), pr(154, "MERGED", &body, &FILES)],
    )
    .await;
    let first = rig.call(OTHER, "/v1/delivered", &["delivered", "153"]).await;
    assert_eq!(first["data"]["edits"][0]["value"], "#110, #153", "{first}");
    let second = rig.call(OTHER, "/v1/delivered", &["delivered", "154"]).await;
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(
        second["data"]["edits"][0]["value"], "#110, #153, #154",
        "the PM's next docs PR must not erase the first pending receipt: {second}"
    );
}
