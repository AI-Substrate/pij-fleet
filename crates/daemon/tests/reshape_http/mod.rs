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
        "Record the version it came from: pij directory edit half --set template_version=<n> --why <text>"
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
        "pij directory release qa pij-holder --why <text>"
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
