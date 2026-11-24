//! PRD 006 s02 (plan 236): the type on a role, the ten definitions and their
//! seeds, through `/v1/directory` against scratch governance repos.

use super::*;

async fn added(addr: SocketAddr, repo: &Repo, argv: &[&str]) -> Value {
    let mut full = vec!["add"];
    full.extend_from_slice(argv);
    full.extend_from_slice(&["--rule", "r", "--packet", "p", "--why", "w"]);
    directory(addr, repo, &full).await
}

/// ac-0001, ac-0002, ac-0006, ac-0008: a role names its type and what the type
/// requires, or nothing is committed; a template supplies its type; the
/// retired flags are ignored with a warning.
#[tokio::test]
async fn add_names_a_type_and_what_it_requires_or_commits_nothing() {
    let repo = repo("pij-types-add");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let start = head(&repo);

    let untyped = added(addr, &repo, &["expert"]).await;
    assert_eq!(untyped["details"]["code"], "E-RS-TYPE-MISSING", "{untyped}");
    let meta = untyped["meta"].as_str().unwrap();
    for kind in pij_daemon::community::types::TYPES {
        assert!(meta.contains(kind), "{kind} listed: {meta}");
    }
    let unkept = added(addr, &repo, &["expert", "--type", "knowledge"]).await;
    assert_eq!(unkept["details"]["code"], "E-RS-TYPE-FIELD", "{unkept}");
    assert!(
        unkept["meta"].as_str().unwrap().contains("--keeps"),
        "{unkept}"
    );
    let unwritten = added(addr, &repo, &["docs", "--type", "steward"]).await;
    assert!(
        unwritten["meta"]
            .as_str()
            .unwrap()
            .contains("--sole-writer-of"),
        "{unwritten}"
    );
    let unknown = added(addr, &repo, &["expert", "--type", "wizard"]).await;
    assert_eq!(unknown["details"]["code"], "E-RS-TYPE-UNKNOWN", "{unknown}");
    assert_eq!(head(&repo), start, "no refused add committed anything");

    let expert = added(
        addr,
        &repo,
        &[
            "expert",
            "--type",
            "knowledge",
            "--keeps",
            "docs/area.md",
            "--kind",
            "specialist",
        ],
    )
    .await;
    assert_eq!(expert["ok"], true, "{expert}");
    assert!(
        expert["data"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("--kind is retired"),
        "{expert}"
    );
    let meta = section(&committed(&repo, "expert"), "meta");
    assert_eq!(meta["type"], "knowledge");
    assert_eq!(meta["keeps"], "docs/area.md");
    assert!(meta.get("kind").is_none(), "kind is not written: {meta}");

    let from_template = directory(
        addr,
        &repo,
        &["add", "coder", "--from", "template:coder", "--why", "w"],
    )
    .await;
    assert_eq!(from_template["ok"], true, "{from_template}");
    assert_eq!(section(&committed(&repo, "coder"), "meta")["type"], "coder");
    let no_type = directory(
        addr,
        &repo,
        &["add", "infra", "--from", "template:infra", "--why", "w"],
    )
    .await;
    assert_eq!(no_type["details"]["code"], "E-RS-TYPE-MISSING", "{no_type}");
    server.abort();
}

/// ac-0004, ac-0005: a default rule changes with a reason, a fixed one never,
/// on add and on edit; changing the type re-checks the recorded changes; a
/// type suggestion is recorded and shown in the view.
#[tokio::test]
async fn a_role_changes_a_default_rule_with_a_reason_and_never_a_fixed_one() {
    let repo = repo("pij-types-rules");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;

    let coder = added(
        addr,
        &repo,
        &[
            "builder",
            "--type",
            "coder",
            "--type-change",
            "compact-after-job=keep it",
        ],
    )
    .await;
    assert_eq!(coder["details"]["code"], "E-RS-TYPE-RULE", "{coder}");
    assert!(coder["meta"].as_str().unwrap().contains("fixed"), "{coder}");

    let security = added(
        addr,
        &repo,
        &[
            "security",
            "--type",
            "reviewer",
            "--type-change",
            "compact-after-job=carries open findings between reviews",
            "--type-suggestion",
            "an auditor that keeps standing context",
        ],
    )
    .await;
    assert_eq!(security["ok"], true, "{security}");
    let document = committed(&repo, "security");
    assert_eq!(
        section(&document, "type_changes"),
        json!([{"rule": "compact-after-job", "why": "carries open findings between reviews"}])
    );
    let view = directory(addr, &repo, &[]).await;
    let row = view["data"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["id"] == "security")
        .cloned()
        .unwrap();
    assert_eq!(row["type"], "reviewer", "{row}");
    assert_eq!(
        row["type_suggestion"],
        "an auditor that keeps standing context"
    );

    let before = head(&repo);
    let retyped = directory(
        addr,
        &repo,
        &["edit", "security", "--type", "coder", "--why", "w"],
    )
    .await;
    assert_eq!(retyped["details"]["code"], "E-RS-TYPE-RULE", "{retyped}");
    let unreasoned = directory(
        addr,
        &repo,
        &[
            "edit",
            "security",
            "--type-change",
            "compact-after-job",
            "--why",
            "w",
        ],
    )
    .await;
    assert_eq!(
        unreasoned["details"]["code"], "E-RS-TYPE-RULE",
        "{unreasoned}"
    );
    assert_eq!(head(&repo), before, "no refused edit committed anything");

    let plain = added(addr, &repo, &["builder", "--type", "coder"]).await;
    assert_eq!(plain["ok"], true, "{plain}");
    let edited = directory(
        addr,
        &repo,
        &[
            "edit",
            "builder",
            "--type-change",
            "compact-after-job=x",
            "--why",
            "w",
        ],
    )
    .await;
    assert_eq!(edited["details"]["code"], "E-RS-TYPE-RULE", "{edited}");
    server.abort();
}

/// A role written before types may still be edited untyped (retype gives it
/// one, plan 238); once typed it keeps a type.
#[tokio::test]
async fn a_role_from_before_types_is_still_edited_untyped() {
    let repo = repo("pij-types-legacy");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let dir = repo.gov.join(ROLES).join("legacy");
    std::fs::create_dir_all(&dir).unwrap();
    let document = json!({"dd": {"schema": "pij/role"}, "sections": [
        {"name": "meta", "value": {"id": "legacy", "title": "Legacy", "kind": "specialist",
            "lifecycle": "long-lived", "holders": "one"}},
        {"name": "scope", "value": {"paths": [], "rules": ["r"]}},
        {"name": "standing_packet", "value": {"source": "inline", "text": "p"}},
        {"name": "changelog", "value": [{"date": "2026-01-01", "by": "pij-a", "why": "old"}]},
    ], "references": []});
    std::fs::write(dir.join("role.dd.json"), document.to_string()).unwrap();
    git(&repo.gov, &["add", "."]);
    git(
        &repo.gov,
        &["commit", "-q", "-m", "a role from before types"],
    );

    let edited = directory(
        addr,
        &repo,
        &["edit", "legacy", "--set", "title=Old", "--why", "w"],
    )
    .await;
    assert_eq!(edited["ok"], true, "{edited}");
    assert_eq!(
        section(&committed(&repo, "legacy"), "meta")["kind"],
        "specialist"
    );
    let typed = directory(
        addr,
        &repo,
        &["edit", "legacy", "--set", "type=worker", "--why", "w"],
    )
    .await;
    assert_eq!(typed["ok"], true, "{typed}");
    assert_eq!(
        section(&committed(&repo, "legacy"), "meta")["type"],
        "worker"
    );
    server.abort();
}

/// ac-0003, ac-0007: the ten types, one definition and its seed; a repo's own
/// seed is written with its reason and then named as the source.
#[tokio::test]
async fn types_print_and_a_repo_owns_its_seed() {
    let repo = repo("pij-types-seed");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;

    let listed = directory(addr, &repo, &["types"]).await;
    let ids: Vec<&str> = listed["data"]["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, pij_daemon::community::types::TYPES);
    let coder = directory(addr, &repo, &["types", "coder"]).await;
    let rules = coder["data"]["type"]["rules"].as_array().unwrap();
    assert!(
        rules
            .iter()
            .any(|rule| rule["id"] == "compact-after-job" && rule["mode"] == "fixed"),
        "{coder}"
    );
    let shipped = directory(addr, &repo, &["types", "coder", "--seed"]).await;
    assert_eq!(shipped["data"]["seed"]["source"], "shipped", "{shipped}");

    let ours = repo.main.join("coder-seed.md");
    std::fs::write(&ours, "Our coders run just ci before every push.\n").unwrap();
    let written = directory(
        addr,
        &repo,
        &[
            "seed",
            "coder",
            "--file",
            ours.to_str().unwrap(),
            "--why",
            "our gate is just ci",
        ],
    )
    .await;
    assert_eq!(written["ok"], true, "{written}");
    let message = git(&repo.gov, &["log", "-1", "--format=%B"]);
    assert!(message.contains("our gate is just ci"), "{message}");
    let local = directory(addr, &repo, &["types", "coder", "--seed"]).await;
    assert_eq!(local["data"]["seed"]["source"], "governance", "{local}");
    assert_eq!(
        local["data"]["seed"]["text"],
        "Our coders run just ci before every push.\n"
    );
    let bad = directory(addr, &repo, &["types", "wizard"]).await;
    assert_eq!(bad["details"]["code"], "E-RS-TYPE-UNKNOWN", "{bad}");
    server.abort();
}
