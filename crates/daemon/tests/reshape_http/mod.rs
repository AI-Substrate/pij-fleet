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
