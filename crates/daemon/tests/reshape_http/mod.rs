//! Plan 206: `pij directory edit` (F20) and template-diff's missing-version
//! refusal (F19), against scratch governance repos with the fake validator.

use super::*;

const ROLES: &str = ".harness/government/community/roles";

async fn directory(addr: SocketAddr, repo: &Repo, argv: &[&str]) -> Value {
    let mut full = vec!["directory"];
    full.extend_from_slice(argv);
    call(addr, "/v1/directory", &full, &repo.main).await
}

async fn fleet(addr: SocketAddr, repo: &Repo) {
    let main = repo.main.to_str().unwrap();
    let created = call(
        addr,
        "/v1/project",
        &["project", "create", "fleet", "--repo", main],
        &repo.main,
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");
}

fn committed(repo: &Repo, id: &str) -> Value {
    let text = git(
        &repo.gov,
        &["show", &format!("HEAD:{ROLES}/{id}/role.dd.json")],
    );
    serde_json::from_str(&text).unwrap()
}

fn section(document: &Value, name: &str) -> Value {
    document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["name"] == name)
        .map_or(Value::Null, |section| section["value"].clone())
}

/// bp-0005: an edit lands one changelog row and the new fields in one commit;
/// a missing role, an unknown field, a no-op, a missing --why, an invalid or
/// foreign document, and --set on another verb refuse and commit nothing.
#[tokio::test]
async fn edit_changes_a_role_with_one_changelog_row_and_bad_edits_commit_nothing() {
    let repo = repo("pij-directory-edit");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let added = directory(
        addr,
        &repo,
        &[
            "add",
            "qa",
            "--title",
            "QA",
            "--kind",
            "specialist",
            "--lifecycle",
            "long-lived",
            "--holders",
            "one",
            "--rule",
            "r1",
            "--packet",
            "p",
            "--why",
            "first",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");

    let edited = directory(
        addr,
        &repo,
        &[
            "edit",
            "qa",
            "--set",
            "title=Quality",
            "--rule",
            "r2",
            "--why",
            "rename",
        ],
    )
    .await;
    assert_eq!(edited["ok"], true, "{edited}");
    assert_eq!(edited["data"]["commit"], head(&repo));
    let document = committed(&repo, "qa");
    assert_eq!(section(&document, "meta")["title"], "Quality");
    assert_eq!(section(&document, "meta")["kind"], "specialist", "kept");
    assert_eq!(section(&document, "scope")["rules"], json!(["r2"]));
    let changelog = section(&document, "changelog");
    assert_eq!(changelog.as_array().unwrap().len(), 2, "{changelog}");
    assert_eq!(changelog[1]["why"], "rename");
    assert_eq!(changelog[1]["by"], SEAT);
    let message = git(&repo.gov, &["log", "-1", "--format=%B"]);
    assert!(message.starts_with("community: edit role qa"), "{message}");

    let file = repo.root.join("qa.dd.json");
    let mut replacement = document.clone();
    replacement["sections"]
        .as_array_mut()
        .unwrap()
        .retain(|section| section["name"] != "changelog");
    replacement["sections"][0]["value"]["title"] = json!("From a file");
    std::fs::write(&file, serde_json::to_string(&replacement).unwrap()).unwrap();
    let from_file = directory(
        addr,
        &repo,
        &[
            "edit",
            "qa",
            "--file",
            file.to_str().unwrap(),
            "--why",
            "whole",
        ],
    )
    .await;
    assert_eq!(from_file["ok"], true, "{from_file}");
    let document = committed(&repo, "qa");
    assert_eq!(section(&document, "meta")["title"], "From a file");
    assert_eq!(
        section(&document, "changelog").as_array().unwrap().len(),
        3,
        "a whole-document edit keeps the history"
    );

    let foreign = repo.root.join("other.dd.json");
    let mut other = replacement.clone();
    other["sections"][0]["value"]["id"] = json!("other");
    std::fs::write(&foreign, serde_json::to_string(&other).unwrap()).unwrap();
    let invalid = repo.root.join("invalid.dd.json");
    let mut broken = replacement.clone();
    broken["sections"][0]["value"]["title"] = json!("INVALID");
    std::fs::write(&invalid, serde_json::to_string(&broken).unwrap()).unwrap();
    let (foreign, invalid) = (foreign.to_str().unwrap(), invalid.to_str().unwrap());

    let before = head(&repo);
    for (argv, code) in [
        (
            vec!["edit", "nobody", "--set", "title=x", "--why", "w"],
            "E-RS-DIRECTORY-ROLE-MISSING",
        ),
        (
            vec!["edit", "qa", "--set", "colour=red", "--why", "w"],
            "E-RS-ARG",
        ),
        (
            vec!["edit", "qa", "--set", "title", "--why", "w"],
            "E-RS-ARG",
        ),
        (
            vec![
                "edit", "qa", "--set", "title=a", "--title", "b", "--why", "w",
            ],
            "E-RS-ARG",
        ),
        (
            vec!["edit", "qa", "--set", "title=From a file", "--why", "same"],
            "E-RS-ARG",
        ),
        (vec!["edit", "qa", "--why", "nothing"], "E-RS-ARG"),
        (vec!["edit", "qa", "--set", "title=x"], "E-RS-ARG"),
        (vec!["edit", "--set", "title=x", "--why", "w"], "E-RS-ARG"),
        (
            vec!["edit", "qa", "--file", foreign, "--why", "w"],
            "E-RS-ARG",
        ),
        (
            vec!["edit", "qa", "--file", invalid, "--why", "w"],
            pij_daemon::community::E_DDOCS_INVALID,
        ),
        (
            vec![
                "edit", "qa", "--file", invalid, "--set", "title=x", "--why", "w",
            ],
            "E-RS-ARG",
        ),
        (
            vec!["add", "qb", "--set", "title=x", "--rule", "r", "--why", "w"],
            "E-RS-ARG",
        ),
    ] {
        let refused = directory(addr, &repo, &argv).await;
        assert_eq!(refused["details"]["code"], code, "{argv:?}: {refused}");
    }
    assert_eq!(head(&repo), before, "no refused edit committed");
    assert_eq!(git(&repo.gov, &["status", "--porcelain"]), "");
    server.abort();
}

/// bp-0004 (F19): a role with a template and no template_version gets a
/// refusal that says so and names the edit that fixes it; after that edit,
/// template-diff answers.
#[tokio::test]
async fn template_diff_names_a_missing_version_and_the_edit_that_records_one() {
    let repo = repo("pij-directory-f19");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let added = directory(
        addr,
        &repo,
        &[
            "add",
            "half",
            "--template",
            "reviewer",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "imported",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");

    let refused = directory(addr, &repo, &["template-diff", "half"]).await;
    assert_eq!(
        refused["details"]["code"], "E-RS-DIRECTORY-NO-TEMPLATE-VERSION",
        "{refused}"
    );
    assert!(
        refused.to_string().contains("no template_version"),
        "{refused}"
    );
    assert_eq!(
        refused["details"]["next"],
        "Record the version it came from: pij directory edit half --set template_version=<n> --why <text> --project fleet"
    );

    let recorded = directory(
        addr,
        &repo,
        &[
            "edit",
            "half",
            "--set",
            "template_version=1",
            "--why",
            "record the version",
        ],
    )
    .await;
    assert_eq!(recorded["ok"], true, "{recorded}");
    let diff = directory(addr, &repo, &["template-diff", "half"]).await;
    assert_eq!(diff["ok"], true, "{diff}");
    assert_eq!(diff["data"]["version"], 1);
    server.abort();
}

fn role_in_view<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["data"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["id"] == id)
        .unwrap_or_else(|| panic!("{id} not in the view: {view}"))
}

/// bp-0001, bp-0002 (req-28): retire refuses while a seat holds the role,
/// then marks it retired with a changelog row; merge is one commit with a row
/// on both roles and moves nothing; the view shows retired, merged-into and
/// merged-from with the lineage intact; a retired role refuses assign,
/// retire and being merged into.
#[tokio::test]
async fn retire_and_merge_reshape_roles_and_keep_their_history() {
    let repo = repo("pij-directory-reshape");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    for id in ["qa", "review", "sec"] {
        let added = directory(
            addr,
            &repo,
            &["add", id, "--rule", "r", "--packet", "p", "--why", "start"],
        )
        .await;
        assert_eq!(added["ok"], true, "{id}: {added}");
    }
    let assigned = directory(addr, &repo, &["assign", "qa", "pij-holder", "--why", "w"]).await;
    assert_eq!(assigned["ok"], true, "{assigned}");

    let before = head(&repo);
    let held = directory(addr, &repo, &["retire", "qa", "--why", "done"]).await;
    assert_eq!(
        held["details"]["code"], "E-RS-DIRECTORY-ROLE-HELD",
        "{held}"
    );
    assert_eq!(
        held["details"]["next"],
        "Release each holding first:\n  pij directory release qa pij-holder --why <text> --project fleet"
    );
    assert_eq!(head(&repo), before, "a refused retire commits nothing");

    let released = directory(addr, &repo, &["release", "qa", "pij-holder", "--why", "w"]).await;
    assert_eq!(released["ok"], true, "{released}");
    let retired = directory(addr, &repo, &["retire", "qa", "--why", "done"]).await;
    assert_eq!(retired["ok"], true, "{retired}");
    assert_eq!(retired["data"]["commit"], head(&repo));
    let document = committed(&repo, "qa");
    assert_eq!(section(&document, "meta")["state"], "retired");
    let changelog = section(&document, "changelog");
    assert_eq!(changelog.as_array().unwrap().last().unwrap()["why"], "done");

    let memory = repo.gov.join(format!("{ROLES}/review/memory/INDEX.md"));
    let memory_before = std::fs::read(&memory).unwrap();
    let merged = directory(
        addr,
        &repo,
        &["merge", "review", "--into", "sec", "--why", "folded"],
    )
    .await;
    assert_eq!(merged["ok"], true, "{merged}");
    let touched = git(&repo.gov, &["show", "--name-only", "--format=", "HEAD"]);
    for path in ["review/role.dd.json", "sec/role.dd.json"] {
        assert!(
            touched.contains(path),
            "{path} not in the commit: {touched}"
        );
    }
    assert!(!touched.contains("memory"), "nothing moved: {touched}");
    assert_eq!(std::fs::read(&memory).unwrap(), memory_before);
    let from = committed(&repo, "review");
    assert_eq!(section(&from, "meta")["state"], "retired");
    assert_eq!(section(&from, "meta")["merged_into"], "sec");
    let last = |document: &Value| {
        section(document, "changelog")
            .as_array()
            .unwrap()
            .last()
            .unwrap()["why"]
            .clone()
    };
    assert_eq!(last(&from), "merged into sec: folded");
    assert_eq!(last(&committed(&repo, "sec")), "review merged in: folded");

    let view = directory(addr, &repo, &[]).await;
    assert_eq!(view["ok"], true, "{view}");
    let qa = role_in_view(&view, "qa");
    assert_eq!(qa["state"], "retired");
    assert_eq!(
        qa["lineage"].as_array().unwrap().len(),
        1,
        "lineage kept: {qa}"
    );
    assert_eq!(role_in_view(&view, "review")["merged_into"], "sec");
    assert_eq!(role_in_view(&view, "sec")["merged_from"], json!(["review"]));
    assert_eq!(role_in_view(&view, "sec")["state"], "active");

    let before = head(&repo);
    for (argv, code) in [
        (
            vec!["retire", "qa", "--why", "again"],
            "E-RS-DIRECTORY-RETIRED",
        ),
        (
            vec!["assign", "qa", "pij-holder", "--why", "w"],
            "E-RS-DIRECTORY-RETIRED",
        ),
        (
            vec!["merge", "sec", "--into", "review", "--why", "w"],
            "E-RS-DIRECTORY-RETIRED",
        ),
        (
            vec!["merge", "sec", "--into", "nobody", "--why", "w"],
            "E-RS-DIRECTORY-ROLE-MISSING",
        ),
        (
            vec!["retire", "nobody", "--why", "w"],
            "E-RS-DIRECTORY-ROLE-MISSING",
        ),
        (
            vec!["merge", "sec", "--into", "sec", "--why", "w"],
            "E-RS-ARG",
        ),
        (vec!["merge", "sec", "--why", "w"], "E-RS-ARG"),
        (
            vec!["retire", "sec", "--into", "qa", "--why", "w"],
            "E-RS-ARG",
        ),
        (vec!["retire", "sec"], "E-RS-ARG"),
        (
            vec!["edit", "qa", "--set", "title=x", "--why", "w"],
            "E-RS-DIRECTORY-RETIRED",
        ),
        (vec!["retire", "--why", "w"], "E-RS-ARG"),
    ] {
        let refused = directory(addr, &repo, &argv).await;
        assert_eq!(refused["details"]["code"], code, "{argv:?}: {refused}");
    }
    let merged_away = directory(
        addr,
        &repo,
        &["assign", "review", "pij-holder", "--why", "w"],
    )
    .await;
    assert_eq!(
        merged_away["details"]["next"], "Use sec instead.",
        "{merged_away}"
    );
    assert_eq!(head(&repo), before, "no refusal committed");
    assert_eq!(git(&repo.gov, &["status", "--porcelain"]), "");
    server.abort();
}

/// docs-system verdict on #135, notes 2 and 3: the writer holds merged_into to
/// retired roles that exist (ddocs cannot), and edit never changes a role's
/// state: a whole-document edit that leaves state out keeps it, and one that
/// sets it refuses.
#[tokio::test]
async fn the_writer_holds_merged_into_to_retired_roles_and_edit_keeps_state() {
    let repo = repo("pij-directory-standing-guards");
    let (addr, server, _fresh) = daemon().await;
    fleet(addr, &repo).await;
    let added = directory(
        addr,
        &repo,
        &[
            "add", "sec", "--rule", "r", "--packet", "p", "--why", "start",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let base = committed(&repo, "sec");
    let write = |name: &str, id: &str, meta: &[(&str, &str)]| {
        let mut document = base.clone();
        document["sections"]
            .as_array_mut()
            .unwrap()
            .retain(|section| section["name"] != "changelog");
        let value = &mut document["sections"][0]["value"];
        value["id"] = json!(id);
        for (key, field) in meta {
            value[*key] = json!(field);
        }
        let path = repo.root.join(name);
        std::fs::write(&path, serde_json::to_string(&document).unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    };

    let before = head(&repo);
    let dangling = write("dangling.dd.json", "old", &[("merged_into", "sec")]);
    let nowhere = write(
        "nowhere.dd.json",
        "old",
        &[("state", "retired"), ("merged_into", "nobody")],
    );
    let retiring = write("retiring.dd.json", "sec", &[("state", "retired")]);
    for (argv, code) in [
        (
            vec!["add", "old", "--file", dangling.as_str(), "--why", "w"],
            "E-RS-ARG",
        ),
        (
            vec!["add", "old", "--file", nowhere.as_str(), "--why", "w"],
            "E-RS-DIRECTORY-ROLE-MISSING",
        ),
        (
            vec!["edit", "sec", "--file", retiring.as_str(), "--why", "w"],
            "E-RS-ARG",
        ),
    ] {
        let refused = directory(addr, &repo, &argv).await;
        assert_eq!(refused["details"]["code"], code, "{argv:?}: {refused}");
    }
    assert_eq!(head(&repo), before, "no refusal committed");

    let active = write("active.dd.json", "kept", &[("state", "active")]);
    let added = directory(
        addr,
        &repo,
        &["add", "kept", "--file", active.as_str(), "--why", "w"],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let silent = write("silent.dd.json", "kept", &[("title", "Kept")]);
    let edited = directory(
        addr,
        &repo,
        &[
            "edit",
            "kept",
            "--file",
            silent.as_str(),
            "--why",
            "retitle",
        ],
    )
    .await;
    assert_eq!(edited["ok"], true, "{edited}");
    let meta = section(&committed(&repo, "kept"), "meta");
    assert_eq!(meta["title"], "Kept");
    assert_eq!(
        meta["state"], "active",
        "a whole-document edit keeps the state"
    );
    server.abort();
}

/// Plan 206 with s05 (#129): a role with requests queued for a holder is not
/// retired or merged away silently. The refusal lists each request's id,
/// requester and age; `--drop-queued` withdraws them (row gone, one spine
/// event, the requester told once by FYI); an assign drains them instead. A
/// retired or merged role takes no sends, the merged one naming its target.
#[tokio::test]
async fn retire_waits_for_queued_requests_and_retired_roles_refuse_sends() {
    let repo = repo("pij-directory-reshape-sends");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    for id in ["qa", "review", "sec"] {
        let added = directory(
            addr,
            &repo,
            &["add", id, "--rule", "r", "--packet", "p", "--why", "start"],
        )
        .await;
        assert_eq!(added["ok"], true, "{id}: {added}");
    }
    // pij-holder asks qa (so the withdrawal tells someone other than the
    // retiring seat); the test seat asks review.
    let from_holder =
        json!({"from": "pij-holder", "caller": {"TMUX_PANE": "%51", "cwd": repo.main}});
    for (role, msg, extra) in [
        ("qa", "req-qa", from_holder),
        ("review", "req-review", json!({})),
    ] {
        let queued = send_to(addr, &repo.main, &format!("role:{role}"), msg, extra).await;
        assert_eq!(queued["data"]["request"]["state"], "queued", "{queued}");
    }

    let before = head(&repo);
    let retire = directory(addr, &repo, &["retire", "qa", "--why", "done"]).await;
    assert_eq!(
        refusal_code(&retire),
        "E-RS-DIRECTORY-ROLE-QUEUED",
        "{retire}"
    );
    let text = retire.to_string();
    assert!(
        text.contains("1 request(s) queued")
            && text.contains("req-qa from pij-holder for qa (0m old)"),
        "{retire}"
    );
    assert!(
        retire["details"]["next"]
            .as_str()
            .unwrap()
            .contains("pij directory retire qa --why <text> --drop-queued"),
        "{retire}"
    );
    let merge = directory(
        addr,
        &repo,
        &["merge", "review", "--into", "sec", "--why", "folded"],
    )
    .await;
    assert_eq!(
        refusal_code(&merge),
        "E-RS-DIRECTORY-ROLE-QUEUED",
        "{merge}"
    );
    assert!(
        merge.to_string().contains("req-review from pij-test-coder"),
        "{merge}"
    );
    assert_eq!(head(&repo), before, "a refused reshape commits nothing");
    let misplaced = directory(
        addr,
        &repo,
        &[
            "edit",
            "qa",
            "--set",
            "summary=s",
            "--why",
            "w",
            "--drop-queued",
        ],
    )
    .await;
    assert_eq!(refusal_code(&misplaced), "E-RS-ARG", "{misplaced}");

    // --drop-queued: retired, the request withdrawn and recorded, its
    // requester told once.
    let dropped = directory(
        addr,
        &repo,
        &["retire", "qa", "--why", "done", "--drop-queued"],
    )
    .await;
    assert_eq!(dropped["ok"], true, "{dropped}");
    assert_eq!(dropped["data"]["withdrawn"][0]["id"], "req-qa", "{dropped}");
    assert_eq!(dropped["data"]["withdrawn"][0]["requester"], "pij-holder");
    assert_eq!(dropped["data"]["withdrawn"][0]["told"], "held", "{dropped}");
    assert!(rig.store.role_request("req-qa").await.unwrap().is_none());
    let event = rig
        .services
        .spine
        .latest_matching(&SeatId::from("pij-holder"), &[pij_store::WITHDRAWN_EVENT])
        .await
        .unwrap()
        .expect("the withdrawal is on the spine");
    let payload: Value = serde_json::from_str(&event.payload).unwrap();
    assert_eq!(
        (
            payload["id"].as_str(),
            payload["withdrawn_by"].as_str(),
            payload["why"].as_str()
        ),
        (Some("req-qa"), Some(SEAT), Some("done"))
    );
    assert_eq!(
        rig.delivery
            .pending_fyi_count(&SeatId::from("pij-holder"))
            .await
            .unwrap(),
        1,
        "told once"
    );

    // An assign drains review's request instead; then the merge lands.
    for verb in ["assign", "release"] {
        let done = directory(addr, &repo, &[verb, "review", "pij-holder", "--why", "w"]).await;
        assert_eq!(done["ok"], true, "{verb} review: {done}");
    }
    let merged = directory(
        addr,
        &repo,
        &["merge", "review", "--into", "sec", "--why", "folded"],
    )
    .await;
    assert_eq!(merged["ok"], true, "{merged}");
    assert_eq!(merged["data"]["withdrawn"], json!([]));

    let to_retired = send_to(addr, &repo.main, "role:qa", "req-retired", json!({})).await;
    assert_eq!(
        refusal_code(&to_retired),
        "E-RS-DIRECTORY-RETIRED",
        "{to_retired}"
    );
    assert!(
        to_retired.to_string().contains("is retired"),
        "{to_retired}"
    );
    let to_merged = send_to(addr, &repo.main, "role:review", "req-merged", json!({})).await;
    assert_eq!(
        refusal_code(&to_merged),
        "E-RS-DIRECTORY-RETIRED",
        "{to_merged}"
    );
    assert!(
        to_merged.to_string().contains("merged into sec"),
        "{to_merged}"
    );
    assert_eq!(to_merged["details"]["next"], "Send to role:sec instead.");
    for msg in ["req-retired", "req-merged"] {
        assert!(
            rig.store.role_request(msg).await.unwrap().is_none(),
            "{msg} queued"
        );
    }
}

/// The `pij ...` commands a refusal's next step prints, one per line or after
/// a lead-in.
fn commands(refusal: &Value) -> Vec<String> {
    refusal["details"]["next"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            line.find("pij directory")
                .map(|at| line[at + 4..].trim().to_string())
        })
        .collect()
}

/// Run a printed command from `cwd`, filling its placeholders.
async fn replay(addr: SocketAddr, cwd: &Path, command: &str, fills: &[(&str, &str)]) -> Value {
    let mut text = command.to_string();
    for (placeholder, value) in fills {
        text = text.replace(placeholder, value);
    }
    let argv: Vec<&str> = text.split_whitespace().collect();
    call(addr, "/v1/directory", &argv, cwd).await
}

/// Review of #135, F001: a send to a role and a retire or merge of it go
/// through one admission gate, held by the send until its request is recorded
/// and by the reshape through its commit and withdrawal. A send queued behind
/// a --drop-queued retire is withdrawn by it; a send arriving after a plain
/// retire's check is refused, never stranded.
#[tokio::test]
async fn sends_and_retirement_share_one_admission_gate() {
    let repo = repo("pij-directory-reshape-gate");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    for id in ["qa", "ops"] {
        let added = directory(
            addr,
            &repo,
            &["add", id, "--rule", "r", "--packet", "p", "--why", "start"],
        )
        .await;
        assert_eq!(added["ok"], true, "{id}: {added}");
    }
    let pause = || tokio::time::sleep(std::time::Duration::from_millis(300));
    let main = repo.main.clone();
    let send = |role: &'static str, msg: &'static str| {
        let main = main.clone();
        tokio::spawn(
            async move { send_to(addr, &main, &format!("role:{role}"), msg, json!({})).await },
        )
    };
    let retire = |role: &'static str, drop: bool| {
        let main = main.clone();
        tokio::spawn(async move {
            let mut argv = vec!["directory", "retire", role, "--why", "gate"];
            if drop {
                argv.push("--drop-queued");
            }
            call(addr, "/v1/directory", &argv, &main).await
        })
    };

    // With --drop-queued: the send holds the gate first and records its
    // request; the retire then withdraws it.
    let gate = rig.services.directory.admission("fleet", "qa").await;
    let sent = send("qa", "req-gate");
    pause().await;
    let retired = retire("qa", true);
    pause().await;
    assert!(
        !sent.is_finished() && !retired.is_finished(),
        "both wait on the gate"
    );
    assert!(rig.store.role_request("req-gate").await.unwrap().is_none());
    drop(gate);
    let sent = sent.await.unwrap();
    assert_eq!(sent["data"]["request"]["state"], "queued", "{sent}");
    let retired = retired.await.unwrap();
    assert_eq!(retired["ok"], true, "{retired}");
    assert_eq!(
        retired["data"]["withdrawn"][0]["id"], "req-gate",
        "{retired}"
    );
    assert!(rig.store.role_request("req-gate").await.unwrap().is_none());

    // Without it: the retire holds the gate first and commits on an empty
    // queue; the send then reads the retired role and is refused.
    let gate = rig.services.directory.admission("fleet", "ops").await;
    let retired = retire("ops", false);
    pause().await;
    let sent = send("ops", "req-late");
    pause().await;
    assert!(
        !sent.is_finished() && !retired.is_finished(),
        "both wait on the gate"
    );
    drop(gate);
    let retired = retired.await.unwrap();
    assert_eq!(retired["ok"], true, "{retired}");
    let sent = sent.await.unwrap();
    assert_eq!(refusal_code(&sent), "E-RS-DIRECTORY-RETIRED", "{sent}");
    assert!(rig.store.role_request("req-late").await.unwrap().is_none());
    assert!(
        rig.store
            .queued_for_role("fleet", "ops")
            .await
            .unwrap()
            .is_empty(),
        "nothing queued behind the retired role"
    );
}

/// Review of #135, F002: a --drop-queued retirement that committed but did
/// not withdraw (the store failed, or the daemon stopped after the FYI) is
/// finished by the same command: no second commit or changelog row, one
/// withdrawal event and one FYI. With nothing left to withdraw it refuses.
#[tokio::test]
async fn an_unfinished_withdrawal_is_finished_by_the_same_command() {
    let repo = repo("pij-directory-reshape-resume");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    let added = directory(
        addr,
        &repo,
        &[
            "add", "qa", "--rule", "r", "--packet", "p", "--why", "start",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let retired = directory(
        addr,
        &repo,
        &["retire", "qa", "--why", "done", "--drop-queued"],
    )
    .await;
    assert_eq!(retired["ok"], true, "{retired}");
    // The state a failed withdrawal leaves: the role retired, a request still
    // queued for it, and its requester already told.
    rig.store
        .record_role_request(&pij_store::RoleRequest {
            id: "req-left".into(),
            project: "fleet".into(),
            role: "qa".into(),
            instance: String::new(),
            requester: "pij-holder".into(),
            body: "please".into(),
            in_reply_to: None,
            fyi: false,
            reply: pij_store::ReplyMode::Wake,
            limit_ms: None,
            state: pij_store::RequestState::Queued,
            holder: None,
            created_at: 1,
            delivered_at: None,
            checkin_at: None,
            escalated_at: None,
            answered_at: None,
        })
        .await
        .unwrap();
    rig.delivery
        .hold_fyi(pij_core::model::Msg {
            from: SeatId::from(SEAT),
            from_machine: None,
            to: SeatId::from("pij-holder"),
            body: "told before the store failed".into(),
            msg_id: "req-left.withdrawn".into(),
            in_reply_to: None,
            command: None,
        })
        .await
        .unwrap();
    let before = head(&repo);
    let rows = section(&committed(&repo, "qa"), "changelog")
        .as_array()
        .unwrap()
        .len();

    let plain = directory(addr, &repo, &["retire", "qa", "--why", "again"]).await;
    assert_eq!(refusal_code(&plain), "E-RS-DIRECTORY-RETIRED", "{plain}");
    let finished = directory(
        addr,
        &repo,
        &["retire", "qa", "--why", "again", "--drop-queued"],
    )
    .await;
    assert_eq!(finished["ok"], true, "{finished}");
    assert_eq!(finished["data"]["resumed"], true);
    assert_eq!(finished["data"]["commit"], Value::Null);
    assert_eq!(
        finished["data"]["withdrawn"][0]["id"], "req-left",
        "{finished}"
    );
    assert_eq!(head(&repo), before, "no second commit");
    assert_eq!(
        section(&committed(&repo, "qa"), "changelog")
            .as_array()
            .unwrap()
            .len(),
        rows,
        "no second changelog row"
    );
    assert!(rig.store.role_request("req-left").await.unwrap().is_none());
    let events = rig
        .services
        .spine
        .all_matching(&SeatId::from("pij-holder"), &[pij_store::WITHDRAWN_EVENT])
        .await
        .unwrap();
    assert_eq!(events.len(), 1, "one withdrawal event");
    assert_eq!(
        rig.delivery
            .pending_fyi_count(&SeatId::from("pij-holder"))
            .await
            .unwrap(),
        1,
        "the requester is told once"
    );
    let nothing_left = directory(
        addr,
        &repo,
        &["retire", "qa", "--why", "again", "--drop-queued"],
    )
    .await;
    assert_eq!(
        refusal_code(&nothing_left),
        "E-RS-DIRECTORY-RETIRED",
        "{nothing_left}"
    );
}

/// Review of #135, F003: the commands a refusal prints keep the project it was
/// asked about and the exact instance. Called from project A's checkout with
/// --project other, replaying each printed command changes B, never A.
#[tokio::test]
async fn printed_repair_commands_keep_the_project_and_instance() {
    let (a, b) = (repo("pij-directory-scope-a"), repo("pij-directory-scope-b"));
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &a).await;
    let created = call(
        addr,
        "/v1/project",
        &[
            "project",
            "create",
            "other",
            "--repo",
            b.main.to_str().unwrap(),
        ],
        &b.main,
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");
    for repo in [&a, &b] {
        let added = call(
            addr,
            "/v1/directory",
            &[
                "directory",
                "add",
                "qa",
                "--title",
                "qa",
                "--kind",
                "general",
                "--lifecycle",
                "long-lived",
                "--holders",
                "many",
                "--rule",
                "r",
                "--packet",
                "p",
                "--why",
                "w",
            ],
            &repo.main,
        )
        .await;
        assert_eq!(added["ok"], true, "{added}");
    }
    let half = call(
        addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "half",
            "--title",
            "half",
            "--kind",
            "general",
            "--lifecycle",
            "long-lived",
            "--holders",
            "one",
            "--rule",
            "r",
            "--packet",
            "p",
            "--template",
            "reviewer",
            "--why",
            "w",
        ],
        &b.main,
    )
    .await;
    assert_eq!(half["ok"], true, "{half}");
    let on_b = |argv: &[&'static str]| {
        let mut full = vec!["directory"];
        full.extend_from_slice(argv);
        full.extend_from_slice(&["--project", "other"]);
        full
    };
    let a_head = head(&a);

    let assigned = call(
        addr,
        "/v1/directory",
        &on_b(&["assign", "qa/135", "pij-holder", "--why", "w"]),
        &a.main,
    )
    .await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    let held = call(
        addr,
        "/v1/directory",
        &on_b(&["retire", "qa", "--why", "w"]),
        &a.main,
    )
    .await;
    assert_eq!(refusal_code(&held), "E-RS-DIRECTORY-ROLE-HELD", "{held}");
    assert_eq!(
        commands(&held),
        ["directory release qa/135 pij-holder --why <text> --project other"]
    );
    for command in commands(&held) {
        let done = replay(addr, &a.main, &command, &[("<text>", "w")]).await;
        assert_eq!(done["ok"], true, "{command}: {done}");
    }
    let rows = holders_at_head(&b, "qa");
    assert!(
        rows.iter().all(|row| !row["to"].is_null()),
        "B's qa/135 released: {rows:?}"
    );

    let queued = send_to(
        addr,
        &a.main,
        "role:qa/7",
        "req-7",
        json!({"project": "other"}),
    )
    .await;
    assert_eq!(queued["data"]["request"]["state"], "queued", "{queued}");
    let refused = call(
        addr,
        "/v1/directory",
        &on_b(&["retire", "qa", "--why", "w"]),
        &a.main,
    )
    .await;
    assert_eq!(
        refusal_code(&refused),
        "E-RS-DIRECTORY-ROLE-QUEUED",
        "{refused}"
    );
    assert!(
        refused
            .to_string()
            .contains("req-7 from pij-test-coder for qa/7"),
        "{refused}"
    );
    let printed = commands(&refused);
    assert_eq!(
        printed,
        [
            "directory assign qa/7 <seat> --why <text> --project other",
            "directory retire qa --why <text> --drop-queued --project other",
        ]
    );
    let drained = replay(
        addr,
        &a.main,
        &printed[0],
        &[("<seat>", "pij-holder"), ("<text>", "w")],
    )
    .await;
    assert_eq!(drained["ok"], true, "{drained}");
    // The drain runs after the assign lands.
    let mut request = rig
        .store
        .role_request("req-7")
        .await
        .unwrap()
        .expect("req-7");
    for _ in 0..100 {
        if request.state == pij_store::RequestState::Delivered {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        request = rig
            .store
            .role_request("req-7")
            .await
            .unwrap()
            .expect("req-7");
    }
    assert_eq!(
        (request.state, request.holder.as_deref()),
        (pij_store::RequestState::Delivered, Some("pij-holder"))
    );

    let diff = call(
        addr,
        "/v1/directory",
        &on_b(&["template-diff", "half"]),
        &a.main,
    )
    .await;
    assert_eq!(
        refusal_code(&diff),
        "E-RS-DIRECTORY-NO-TEMPLATE-VERSION",
        "{diff}"
    );
    let [edit] = commands(&diff).try_into().expect("one printed edit");
    let b_before = head(&b);
    let edited = replay(addr, &a.main, &edit, &[("<n>", "1"), ("<text>", "w")]).await;
    assert_eq!(edited["ok"], true, "{edit}: {edited}");
    assert_ne!(head(&b), b_before, "B changed");
    assert_eq!(head(&a), a_head, "A untouched throughout");
}

/// A role with many holders, added to the project at `repo`.
async fn many_holders(addr: SocketAddr, repo: &Repo, id: &str) {
    let added = directory(
        addr,
        repo,
        &[
            "add",
            id,
            "--title",
            id,
            "--kind",
            "general",
            "--lifecycle",
            "long-lived",
            "--holders",
            "many",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "start",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{id}: {added}");
}

/// A request for `role` in `fleet`, as the store would hold it.
fn request(id: &str, role: &str, state: pij_store::RequestState) -> pij_store::RoleRequest {
    pij_store::RoleRequest {
        id: id.into(),
        project: "fleet".into(),
        role: role.into(),
        instance: String::new(),
        requester: SEAT.into(),
        body: "please".into(),
        in_reply_to: None,
        fyi: false,
        reply: pij_store::ReplyMode::Wake,
        limit_ms: None,
        holder: (state == pij_store::RequestState::Delivering).then(|| "pij-holder".into()),
        state,
        created_at: 1,
        delivered_at: None,
        checkin_at: None,
        escalated_at: None,
        answered_at: None,
    }
}

/// Security review of #135, S001: a holding instance is data, so the release a
/// held-role refusal prints quotes it as one shell word. `/bin/sh` splits the
/// printed line (placeholders filled) through `printf` in an empty directory:
/// the words come back exactly, and the payload, which would write a file if
/// it ran, writes nothing.
#[tokio::test]
async fn printed_commands_quote_a_hostile_holding_instance() {
    let repo = repo("pij-directory-reshape-quote");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    many_holders(addr, &repo, "qa").await;
    let hostile = "x$(id>dollar)`id>tick`;id>semi";
    // F135: assign refuses this key now; it stands for a legacy stored row.
    crate::seed_legacy_holding(&rig, "qa", hostile, "pij-holder").await;
    let held = directory(addr, &repo, &["retire", "qa", "--why", "w"]).await;
    assert_eq!(refusal_code(&held), "E-RS-DIRECTORY-ROLE-HELD", "{held}");
    let [release] = commands(&held).try_into().expect("one printed release");
    let line = release.replace("<text>", "w");
    let dir = fresh_dir("pij-directory-reshape-quote-sh");
    let shell = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("printf '%s\\037' {line}"))
        .current_dir(&dir)
        .output()
        .expect("run /bin/sh");
    assert!(shell.status.success(), "{shell:?}");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "nothing in the printed command ran: {line}"
    );
    let words: Vec<String> = String::from_utf8(shell.stdout)
        .unwrap()
        .split('\x1f')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    assert_eq!(
        words,
        [
            "directory",
            "release",
            &format!("qa/{hostile}"),
            "pij-holder",
            "--why",
            "w",
            "--project",
            "fleet",
        ],
        "{line}"
    );
}

/// Review of #135, F001 (queue re-entry): a drain holds the role's gate from
/// its claim until the request is settled, and a retire or merge refuses while
/// any request for the role is still being delivered, since a failed delivery,
/// or a daemon that stopped first, puts it back on the queue. Each such
/// request then shows up queued, never behind a retired role.
#[tokio::test]
async fn retirement_waits_for_requests_in_delivery() {
    let repo = repo("pij-directory-reshape-inflight");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    for id in ["qa", "ops", "dev"] {
        many_holders(addr, &repo, id).await;
    }

    // A drain cannot claim while the gate is held, so no retire can slip
    // between a claim and its settling.
    let queued = send_to(addr, &repo.main, "role:dev", "req-drain", json!({})).await;
    assert_eq!(queued["data"]["request"]["state"], "queued", "{queued}");
    let gate = rig.services.directory.admission("fleet", "dev").await;
    // The assign waits for its drain (plan 212, F56), so it runs alongside
    // and is awaited once the gate is released.
    let main = repo.main.clone();
    let assigning = tokio::spawn(async move {
        call(
            addr,
            "/v1/directory",
            &["directory", "assign", "dev", "pij-holder", "--why", "w"],
            &main,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let state = |id: &'static str| {
        let store = rig.store.clone();
        async move { store.role_request(id).await.unwrap().map(|r| r.state) }
    };
    assert_eq!(
        state("req-drain").await,
        Some(pij_store::RequestState::Queued),
        "the drain waits on the gate before it claims"
    );
    drop(gate);
    let assigned = assigning.await.unwrap();
    assert_eq!(assigned["ok"], true, "{assigned}");
    for _ in 0..100 {
        if state("req-drain").await == Some(pij_store::RequestState::Delivered) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        state("req-drain").await,
        Some(pij_store::RequestState::Delivered)
    );

    // Claimed, its holding ended: both verbs refuse and write nothing.
    let before = head(&repo);
    for (role, id) in [("qa", "req-fail"), ("ops", "req-boot")] {
        rig.store
            .record_role_request(&request(id, role, pij_store::RequestState::Delivering))
            .await
            .unwrap();
        for drop_queued in [false, true] {
            let mut argv = vec!["retire", role, "--why", "w"];
            if drop_queued {
                argv.push("--drop-queued");
            }
            let refused = directory(addr, &repo, &argv).await;
            assert_eq!(
                refusal_code(&refused),
                "E-RS-DIRECTORY-ROLE-QUEUED",
                "{refused}"
            );
            assert!(
                refused
                    .to_string()
                    .contains(&format!("{id} from {SEAT} to pij-holder")),
                "{refused}"
            );
        }
        let merged = directory(addr, &repo, &["merge", role, "--into", "dev", "--why", "w"]).await;
        assert_eq!(
            refusal_code(&merged),
            "E-RS-DIRECTORY-ROLE-QUEUED",
            "{merged}"
        );
    }
    assert_eq!(head(&repo), before, "nothing written");

    // The delivery fails: the request goes back on the queue, where the
    // queued refusal and --drop-queued take over.
    assert!(
        rig.store
            .release_role_request_claim("req-fail", "pij-holder")
            .await
            .unwrap()
    );
    // The daemon stopped first: boot recovery re-queues it.
    rig.store.recover_delivering_role_requests(2).await.unwrap();
    for (role, id) in [("qa", "req-fail"), ("ops", "req-boot")] {
        assert_eq!(state(id).await, Some(pij_store::RequestState::Queued));
        let refused = directory(addr, &repo, &["retire", role, "--why", "w"]).await;
        assert_eq!(
            refusal_code(&refused),
            "E-RS-DIRECTORY-ROLE-QUEUED",
            "{refused}"
        );
        assert!(
            refused.to_string().contains("queued for a holder"),
            "{refused}"
        );
        let dropped = directory(
            addr,
            &repo,
            &["retire", role, "--why", "w", "--drop-queued"],
        )
        .await;
        assert_eq!(dropped["ok"], true, "{dropped}");
        assert_eq!(dropped["data"]["withdrawn"][0]["id"], id, "{dropped}");
        assert!(
            rig.store
                .queued_for_role("fleet", role)
                .await
                .unwrap()
                .is_empty()
                && rig
                    .store
                    .delivering_for_role("fleet", role)
                    .await
                    .unwrap()
                    .is_empty(),
            "nothing left behind retired {role}"
        );
    }
}

/// Holds the writer inside its ddocs validation of `role`'s document, once
/// armed, until the test lets it go: a retirement paused after its waiter
/// handed the commit to the blocking pool.
struct PausingDdocs {
    armed: std::sync::atomic::AtomicBool,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    go: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl pij_daemon::community::ddocs::Validator for PausingDdocs {
    fn validate(&self, _worktree: &Path, document: &Path) -> Result<(), Refusal> {
        if document.ends_with("roles/qa/role.dd.json")
            && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            let _ = self.entered.send(());
            let _ = self.go.lock().unwrap().recv();
        }
        Ok(())
    }

    fn build(&self, _: &Path, document: &Path) -> Result<(), Refusal> {
        std::fs::write(rendered(document), "rendered\n").unwrap();
        Ok(())
    }
}

/// Review of #135, F001 (cancelled waiters): the gate goes with the work that
/// can still land, not the caller awaiting it. A send cancelled while its
/// insert waits on the store keeps the gate until the insert lands, so the
/// retire after it sees the request; a retirement cancelled while its commit
/// waits in the writer keeps it until the commit lands, so the send after it
/// sees the retired role.
#[tokio::test]
async fn the_gate_outlives_a_cancelled_waiter() {
    let repo = repo("pij-directory-reshape-cancel");
    let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (go, go_rx) = std::sync::mpsc::channel();
    let ddocs = Arc::new(PausingDdocs {
        armed: std::sync::atomic::AtomicBool::new(false),
        entered,
        go: std::sync::Mutex::new(go_rx),
    });
    let writer = CommunityWriter::new(
        Arc::clone(&ddocs) as Arc<dyn pij_daemon::community::ddocs::Validator>,
        vec![],
        &LOCK_RETRY,
    );
    let rig = rig_inner(None, writer, Catalog::shipped()).await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    for id in ["qa", "ops"] {
        many_holders(addr, &repo, id).await;
    }

    // The send: its insert waits behind another writer's transaction.
    use sqlx::Connection;
    let mut blocker = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(rig._fresh.path()),
    )
    .await
    .unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .unwrap();
    let services = rig.services.clone();
    let main = repo.main.clone();
    let send = tokio::spawn(async move {
        let address = pij_daemon::delivery::role_send::RoleAddress::parse("role:ops")
            .unwrap()
            .unwrap();
        pij_daemon::delivery::role_send::route(
            &services,
            pij_daemon::delivery::role_send::RoleSend {
                from: SeatId::from(SEAT),
                address,
                project: "fleet".into(),
                repo: main,
                body: "please".into(),
                msg_id: "req-cancelled".into(),
                in_reply_to: None,
                fyi: false,
                options: pij_daemon::delivery::role_send::RoleOptions::default(),
            },
            1,
        )
        .await
        .map(|_| ())
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    send.abort();
    assert!(send.await.unwrap_err().is_cancelled());
    let main = repo.main.clone();
    let retire = tokio::spawn(async move {
        call(
            addr,
            "/v1/directory",
            &["directory", "retire", "ops", "--why", "w"],
            &main,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    sqlx::query("ROLLBACK").execute(&mut blocker).await.unwrap();
    let retired = retire.await.unwrap();
    assert!(
        rig.store
            .role_request("req-cancelled")
            .await
            .unwrap()
            .is_some(),
        "the cancelled send's insert landed after its waiter was gone"
    );
    assert_eq!(
        refusal_code(&retired),
        "E-RS-DIRECTORY-ROLE-QUEUED",
        "the retire saw it: {retired}"
    );
    assert_ne!(
        section(&committed(&repo, "ops"), "meta")["state"],
        "retired"
    );

    // The retirement: cancelled while its commit waits in the writer.
    ddocs.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let directory_service = rig.services.directory.clone();
    let main = repo.main.clone();
    let retire = tokio::spawn(async move {
        directory_service
            .retire(
                "fleet",
                main,
                "qa",
                &SeatId::from(SEAT),
                pij_daemon::community::directory::Retirement {
                    why: "w",
                    drop_queued: false,
                },
            )
            .await
            .map(|_| ())
    });
    entered_rx.recv().await.expect("the writer reached qa");
    retire.abort();
    assert!(retire.await.unwrap_err().is_cancelled());
    let main = repo.main.clone();
    let sent =
        tokio::spawn(async move { send_to(addr, &main, "role:qa", "req-after", json!({})).await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !sent.is_finished(),
        "the send waits for the retirement it raced"
    );
    go.send(()).unwrap();
    let sent = sent.await.unwrap();
    assert_eq!(refusal_code(&sent), "E-RS-DIRECTORY-RETIRED", "{sent}");
    assert_eq!(section(&committed(&repo, "qa"), "meta")["state"], "retired");
    assert!(rig.store.role_request("req-after").await.unwrap().is_none());
}

/// Plan 202 meets plan 206: an edit and a retirement rewrite `ROUTES.md` in
/// their own commit, and a retired role routes nothing.
#[tokio::test]
async fn edit_and_retire_keep_the_routing_table_current() {
    let repo = repo("pij-directory-reshape-routes");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    many_holders(addr, &repo, "qa").await;
    let routes = || {
        git(
            &repo.gov,
            &["show", "HEAD:.harness/government/community/ROUTES.md"],
        )
    };
    assert!(routes().contains("role:qa"), "{}", routes());

    let edited = directory(
        addr,
        &repo,
        &["edit", "qa", "--scope-path", "src/qa/**", "--why", "w"],
    )
    .await;
    assert_eq!(edited["ok"], true, "{edited}");
    assert_eq!(edited["data"]["routes"]["regenerated"], true, "{edited}");
    assert!(routes().contains("src/qa/**"), "{}", routes());

    let retired = directory(addr, &repo, &["retire", "qa", "--why", "w"]).await;
    assert_eq!(retired["ok"], true, "{retired}");
    assert_eq!(retired["data"]["routes"]["regenerated"], true, "{retired}");
    assert!(!routes().contains("role:qa"), "{}", routes());
}

/// Security review of #135, F002: `--drop-queued` withdraws only once every
/// requester was told. With the FYI store unavailable, the retirement lands
/// and nothing is withdrawn; the same command then tells and withdraws. A
/// requester that is no longer a seat has nobody to tell and does not hold it
/// back.
#[tokio::test]
async fn a_requester_not_told_holds_the_withdrawal_back() {
    let repo = repo("pij-directory-reshape-untold");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    many_holders(addr, &repo, "qa").await;
    let mut ghost = request("req-ghost", "qa", pij_store::RequestState::Queued);
    ghost.requester = "pij-ghost".into();
    let mut told = request("req-told", "qa", pij_store::RequestState::Queued);
    told.requester = "pij-holder".into();
    for queued in [&ghost, &told] {
        rig.store.record_role_request(queued).await.unwrap();
    }
    use sqlx::Connection;
    let mut store = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(rig._fresh.path()),
    )
    .await
    .unwrap();
    sqlx::query("ALTER TABLE fyis RENAME TO fyis_away")
        .execute(&mut store)
        .await
        .unwrap();
    let refused = directory(
        addr,
        &repo,
        &["retire", "qa", "--why", "done", "--drop-queued"],
    )
    .await;
    sqlx::query("ALTER TABLE fyis_away RENAME TO fyis")
        .execute(&mut store)
        .await
        .unwrap();
    assert_eq!(
        refusal_code(&refused),
        "E-RS-DIRECTORY-NOT-TOLD",
        "{refused}"
    );
    assert!(
        refused.to_string().contains("pij-holder (not told"),
        "{refused}"
    );
    assert_eq!(section(&committed(&repo, "qa"), "meta")["state"], "retired");
    for id in ["req-ghost", "req-told"] {
        assert!(
            rig.store.role_request(id).await.unwrap().is_some(),
            "{id} is not withdrawn"
        );
    }

    let [again] = commands(&refused).try_into().expect("the same command");
    let finished = replay(addr, &repo.main, &again, &[("<text>", "done")]).await;
    assert_eq!(finished["ok"], true, "{finished}");
    assert_eq!(finished["data"]["resumed"], true, "{finished}");
    let outcomes: Vec<(String, String)> = finished["data"]["withdrawn"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap().to_string(),
                row["told"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(outcomes.len(), 2, "{finished}");
    assert!(
        outcomes
            .iter()
            .any(|(id, told)| id == "req-ghost" && told.starts_with("gone")),
        "{finished}"
    );
    assert!(
        outcomes
            .iter()
            .any(|(id, told)| id == "req-told" && told == "held"),
        "{finished}"
    );
    assert!(
        rig.store
            .queued_for_role("fleet", "qa")
            .await
            .unwrap()
            .is_empty()
    );
    // The gone requester is visible in the receipt and in its spine event.
    assert_eq!(finished["data"]["gone"], json!(["req-ghost"]), "{finished}");
    for (seat, gone) in [("pij-ghost", true), ("pij-holder", false)] {
        let events = rig
            .services
            .spine
            .all_matching(&SeatId::from(seat), &[pij_store::WITHDRAWN_EVENT])
            .await
            .unwrap();
        assert_eq!(events.len(), 1, "{seat}: one withdrawal event");
        let payload: Value = serde_json::from_str(&events[0].payload).unwrap();
        assert_eq!(payload["requester_gone"], gone, "{seat}: {payload}");
    }
}

/// Review of #135, F004: an unfinished step that needs a retired role gets no
/// `add` for it (its document stays, so add would refuse). A merged role's
/// steps are pointed at the merge target and a retired role's at a role to
/// name, with one `ddocs set` per step that replaces every retired role in
/// it; a role that never existed is still
/// offered `add`, and a complete step names nothing.
#[tokio::test]
async fn needs_naming_a_retired_role_are_retargeted_not_re_added() {
    let repo = repo("pij-directory-reshape-needs");
    let rig = draining_rig().await;
    let addr = rig.addr;
    fleet(addr, &repo).await;
    for id in ["qa", "old", "review"] {
        many_holders(addr, &repo, id).await;
    }
    let merged = directory(
        addr,
        &repo,
        &["merge", "qa", "--into", "review", "--why", "w"],
    )
    .await;
    assert_eq!(merged["ok"], true, "{merged}");
    let retired = directory(addr, &repo, &["retire", "old", "--why", "w"]).await;
    assert_eq!(retired["ok"], true, "{retired}");
    let workstreams = repo.main.join("docs/project/workstreams");
    std::fs::create_dir_all(&workstreams).unwrap();
    std::fs::write(
        workstreams.join("001-test.dd.json"),
        json!({"sections": [{"name": "steps", "value": [
            {"id": "s01", "state": "complete", "needs": ["role:qa"]},
            {"id": "s09", "state": "planned", "needs": ["role:qa", "role:old", "role:fresh"]},
            {"id": "s10", "state": "planned", "needs": ["role:qa"]}
        ]}]})
        .to_string(),
    )
    .unwrap();

    let viewed = view(addr, &repo).await;
    assert_eq!(viewed["ok"], true, "{viewed}");
    let prompts = viewed["data"]["prompts"].as_array().unwrap();
    let prompt = |role: &str| {
        prompts
            .iter()
            .find(|prompt| prompt["role"] == role)
            .unwrap_or_else(|| panic!("a prompt for {role}: {viewed}"))
    };
    for role in ["qa", "old"] {
        assert!(
            prompts.iter().all(|prompt| prompt["role"] != role
                || !prompt["command"]
                    .as_str()
                    .unwrap()
                    .contains("directory add")),
            "no add for retired {role}: {viewed}"
        );
        assert_eq!(prompt(role)["act"], "retarget", "{viewed}");
    }
    assert!(
        prompt("fresh")["command"]
            .as_str()
            .unwrap()
            .starts_with("pij directory add fresh "),
        "{viewed}"
    );

    // Each printed edit splits, under /bin/sh, into the step's address and
    // its needs with the retired role replaced.
    let words = |command: &str| -> Vec<String> {
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '%s\\037' {command}"))
            .output()
            .expect("run /bin/sh");
        String::from_utf8(output.stdout)
            .unwrap()
            .split('\x1f')
            .filter(|word| !word.is_empty())
            .map(str::to_string)
            .collect()
    };
    let edits = |role: &str| -> Vec<(String, Value)> {
        prompt(role)["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|command| {
                let words = words(command.as_str().unwrap());
                assert_eq!(words[..3], ["ddocs", "set", "--value-json"], "{words:?}");
                (words[3].clone(), serde_json::from_str(&words[4]).unwrap())
            })
            .collect()
    };
    let file = "docs/project/workstreams/001-test.dd.json";
    assert_eq!(
        edits("qa"),
        [
            (
                format!("{file}#steps/s09/needs"),
                json!(["role:review", "role:<replaces-old>", "role:fresh"])
            ),
            (format!("{file}#steps/s10/needs"), json!(["role:review"])),
        ]
    );
    assert_eq!(prompt("qa")["in_order"], true);
    assert!(
        prompt("qa")["detail"]
            .as_str()
            .unwrap()
            .contains("merged into review"),
        "{viewed}"
    );
    assert_eq!(
        edits("old"),
        edits("qa")[..1],
        "one step's edit is the same from either role"
    );
    assert!(
        prompt("old")["detail"]
            .as_str()
            .unwrap()
            .contains("retired"),
        "{viewed}"
    );
}
