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
