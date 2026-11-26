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

/// OSK-DIRECTORY-ADD-SERIAL: an add missing several fields is refused once,
/// naming each with the flag that gives it, under the code the first one
/// always got; one missing field keeps its own refusal; nothing is committed.
#[tokio::test]
async fn an_add_missing_several_fields_gets_one_refusal_naming_each_flag() {
    let repo = repo("pij-types-all-gaps");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let start = head(&repo);
    let meta = |refused: &Value| refused["meta"].as_str().unwrap().to_string();

    let bare = directory(addr, &repo, &["add", "probe", "--why", "w"]).await;
    assert_eq!(bare["details"]["code"], "E-RS-TYPE-MISSING", "{bare}");
    let said = meta(&bare);
    for flag in [
        "--type <type>",
        "--title <text>",
        "--holders one|many",
        "--rule <text>",
        "--packet <text> or --brief <path>",
        "pij directory add --help",
    ] {
        assert!(said.contains(flag), "{flag}: {said}");
    }
    assert!(said.contains("5 fields are missing or invalid"), "{said}");

    let unexplained = directory(
        addr,
        &repo,
        &[
            "add",
            "probe",
            "--type",
            "knowledge",
            "--title",
            "t",
            "--holders",
            "one",
        ],
    )
    .await;
    assert_eq!(unexplained["details"]["code"], "E-RS-ARG", "{unexplained}");
    let said = meta(&unexplained);
    for flag in [
        "--why <text>",
        "--keeps <value>",
        "--rule <text>",
        "--packet <text>",
    ] {
        assert!(said.contains(flag), "{flag}: {said}");
    }

    // Review B363-R1-01: a packet given but invalid joins the same refusal.
    let empty_packet = [
        "add",
        "probe",
        "--type",
        "knowledge",
        "--title",
        "t",
        "--holders",
        "one",
        "--rule",
        "r",
        "--packet",
        "",
        "--why",
        "w",
    ];
    let both = directory(addr, &repo, &empty_packet).await;
    assert_eq!(both["details"]["code"], "E-RS-TYPE-FIELD", "{both}");
    let said = meta(&both);
    for flag in [
        "--keeps <value>",
        "--packet <text> or --brief <path>",
        "pij directory add --help",
    ] {
        assert!(said.contains(flag), "{flag}: {said}");
    }
    let mut kept = empty_packet.to_vec();
    kept.extend(["--keeps", "docs/area.md"]);
    let packet_only = directory(addr, &repo, &kept).await;
    assert_eq!(packet_only["details"]["code"], "E-RS-ARG", "{packet_only}");
    assert!(
        !meta(&packet_only).contains("missing or invalid"),
        "one gap keeps its own refusal: {packet_only}"
    );

    let one = added(
        addr,
        &repo,
        &[
            "probe",
            "--type",
            "knowledge",
            "--title",
            "t",
            "--holders",
            "one",
        ],
    )
    .await;
    assert_eq!(one["details"]["code"], "E-RS-TYPE-FIELD", "{one}");
    assert!(
        !meta(&one).contains("missing or invalid"),
        "one gap keeps its own refusal: {one}"
    );
    assert_eq!(head(&repo), start, "no refused add committed anything");
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

async fn seats(addr: SocketAddr) -> Vec<Value> {
    let listed: Value = reqwest::Client::new()
        .get(format!("http://{addr}/v1/seats"))
        .bearer_auth(KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    listed["data"]["seats"].as_array().unwrap().clone()
}

fn listed<'a>(seats: &'a [Value], id: &str) -> &'a Value {
    seats
        .iter()
        .find(|seat| seat["id"] == id)
        .unwrap_or_else(|| panic!("{id} not listed: {seats:?}"))
}

/// Plan 237 (s03): a seat's type in `pij list --json` beside every old field;
/// a seat holding two types is a view action; `edit --type` refreshes the
/// list's type in the same call and the rebuild recomputes it.
#[tokio::test]
async fn the_list_and_the_view_show_each_seats_type() {
    let repo = repo("pij-types-list");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    for argv in [
        &["builder", "--type", "coder"][..],
        &["expert", "--type", "knowledge", "--keeps", "docs/area.md"][..],
    ] {
        let done = added(rig.addr, &repo, argv).await;
        assert_eq!(done["ok"], true, "{done}");
    }
    for (role, seat) in [
        ("builder", "pij-holder"),
        ("expert", "pij-holder"),
        ("expert", "pij-second"),
    ] {
        let done = directory(rig.addr, &repo, &["assign", role, seat, "--why", "w"]).await;
        assert_eq!(done["ok"], true, "{done}");
    }

    let before = seats(rig.addr).await;
    let holder = listed(&before, "pij-holder");
    assert_eq!(holder["types"], json!(["coder", "knowledge"]), "{holder}");
    assert_eq!(holder["type"], Value::Null, "two types: no one label");
    assert_eq!(holder["role"], "worker", "authority unchanged: {holder}");
    assert_eq!(listed(&before, "pij-second")["type"], "knowledge");
    assert_eq!(listed(&before, "pij-dormant")["type"], Value::Null);
    assert_eq!(listed(&before, "pij-dormant")["types"], json!([]));

    let view = directory(rig.addr, &repo, &[]).await;
    let two: Vec<&Value> = view["data"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["condition"] == "two-types")
        .collect();
    assert_eq!(two.len(), 1, "{view}");
    assert_eq!(two[0]["seat"], "pij-holder");

    let retyped = directory(
        rig.addr,
        &repo,
        &["edit", "expert", "--set", "type=worker", "--why", "w"],
    )
    .await;
    assert_eq!(retyped["ok"], true, "{retyped}");
    assert_eq!(
        listed(&seats(rig.addr).await, "pij-second")["type"],
        "worker"
    );

    // A stale pointer (as a hand edit of the branch would leave) is
    // recomputed by the rebuild.
    let (prefix, suffix) = pij_daemon::community::pointer::role_document_affixes();
    let stale: Vec<_> = rig
        .store
        .community_pointers("fleet")
        .await
        .unwrap()
        .into_iter()
        .filter(|pointer| pointer.path == format!("{prefix}expert{suffix}"))
        .map(|mut pointer| {
            pointer.role_type = Some("steward".into());
            pointer
        })
        .collect();
    rig.store.record_community_pointers(&stale).await.unwrap();
    assert_eq!(
        listed(&seats(rig.addr).await, "pij-second")["type"],
        "steward"
    );
    let rebuilt = directory(rig.addr, &repo, &["rebuild"]).await;
    assert_eq!(rebuilt["ok"], true, "{rebuilt}");
    assert_eq!(
        listed(&seats(rig.addr).await, "pij-second")["type"],
        "worker"
    );
    rig.server.abort();
}

/// Plan 238 (s04): retype proposes, writes nothing without --apply, types the
/// sure roles in one commit with --apply (refreshing `pij list`), lists the
/// rest with the edit that types each, and a second run has nothing to do.
#[tokio::test]
async fn retype_types_what_it_is_sure_of_and_lists_the_rest() {
    let repo = repo("pij-types-retype");
    let rig = rig(Arc::new(NoHook)).await;
    fleet(rig.addr, &repo).await;
    for (id, template) in [
        ("sec", Some("security")),
        ("pm-004", None),
        ("historian", None),
        ("expert", Some("topic-expert")),
    ] {
        let dir = repo.gov.join(ROLES).join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut meta = json!({"id": id, "title": id, "kind": "specialist",
            "lifecycle": "long-lived", "holders": "one"});
        if let Some(template) = template {
            meta["template"] = json!(template);
            meta["template_version"] = json!("1");
        }
        let document = json!({"dd": {"schema": "pij/role"}, "sections": [
            {"name": "meta", "value": meta},
            {"name": "scope", "value": {"paths": [], "rules": ["r"]}},
            {"name": "standing_packet", "value": {"source": "inline", "text": "p"}},
            {"name": "changelog", "value": [{"date": "2026-01-01", "by": "pij-a", "why": "old"}]},
        ], "references": []});
        std::fs::write(dir.join("role.dd.json"), document.to_string()).unwrap();
    }
    git(&repo.gov, &["add", "."]);
    git(
        &repo.gov,
        &["commit", "-q", "-m", "roles from before types"],
    );
    let assigned = directory(
        rig.addr,
        &repo,
        &["assign", "pm-004", "pij-second", "--why", "w"],
    )
    .await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    assert_eq!(
        listed(&seats(rig.addr).await, "pij-second")["type"],
        "project-manager",
        "an untyped pm seat reads its seat role's type"
    );

    let start = head(&repo);
    let dry = directory(rig.addr, &repo, &["retype"]).await;
    assert_eq!(dry["ok"], true, "{dry}");
    assert_eq!(head(&repo), start, "a dry run writes nothing");
    let proposed: Vec<(&str, &str)> = dry["data"]["proposed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["id"].as_str().unwrap(), row["type"].as_str().unwrap()))
        .collect();
    assert_eq!(
        proposed,
        [("pm-004", "project-manager"), ("sec", "reviewer")]
    );
    let undecided: Vec<&str> = dry["data"]["undecided"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(undecided, ["expert", "historian"]);
    assert!(
        dry["data"]["undecided"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--keeps"),
        "{dry}"
    );

    let refused = directory(rig.addr, &repo, &["retype", "--apply"]).await;
    assert_eq!(refused["ok"], false, "--apply needs --why: {refused}");
    let applied = directory(
        rig.addr,
        &repo,
        &["retype", "--apply", "--why", "type the old roles"],
    )
    .await;
    assert_eq!(applied["ok"], true, "{applied}");
    assert_eq!(applied["data"]["applied"], true);
    assert_eq!(head(&repo), applied["data"]["commit"].as_str().unwrap());
    assert_eq!(
        section(&committed(&repo, "sec"), "meta")["type"],
        "reviewer"
    );
    assert_eq!(
        section(&committed(&repo, "pm-004"), "meta")["type"],
        "project-manager"
    );
    assert!(
        section(&committed(&repo, "historian"), "meta")
            .get("type")
            .is_none()
    );
    let seat = listed(&seats(rig.addr).await, "pij-second").clone();
    assert_eq!(
        seat["types"],
        json!(["project-manager"]),
        "the pointer was refreshed: {seat}"
    );

    let again = directory(rig.addr, &repo, &["retype", "--apply", "--why", "again"]).await;
    assert_eq!(again["data"]["applied"], false, "{again}");
    assert_eq!(again["data"]["proposed"], json!([]));
    assert_eq!(head(&repo), applied["data"]["commit"].as_str().unwrap());
    rig.server.abort();
}
