use super::*;
use pij_daemon::community::templates::Template;

const ROLES: &str = ".harness/government/community/roles";

async fn project(addr: SocketAddr, repo: &Repo) {
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

async fn directory(addr: SocketAddr, repo: &Repo, argv: &[&str]) -> Value {
    let mut full = vec!["directory"];
    full.extend_from_slice(argv);
    call(addr, "/v1/directory", &full, &repo.main).await
}

fn committed(repo: &Repo, id: &str) -> Value {
    let text = git(
        &repo.gov,
        &["show", &format!("HEAD:{ROLES}/{id}/role.dd.json")],
    );
    serde_json::from_str(&text).unwrap()
}

fn section<'a>(document: &'a Value, name: &str) -> &'a Value {
    &document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["name"] == name)
        .unwrap_or_else(|| panic!("no {name} section in {document}"))["value"]
}

/// AC2, bp-0002: each shipped template becomes a role with its id
/// and where it came from; flags override; an unknown name or version refuses
/// by name, listing what exists, and commits nothing.
#[tokio::test]
async fn add_from_each_template_records_it_and_unknown_ones_refuse_by_name() {
    let repo = repo("pij-templates-add");
    let (addr, server, _fresh) = daemon().await;
    project(addr, &repo).await;

    let listed = directory(addr, &repo, &["templates"]).await;
    assert_eq!(listed["ok"], true, "{listed}");
    let names: Vec<&str> = listed["data"]["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "coder",
            "designer",
            "devcontainer",
            "docs-keeper",
            "infra",
            "note-taker",
            "pa",
            "pm",
            "prime",
            "qa",
            "release",
            "researcher",
            "reviewer",
            "security",
            "topic-expert",
            "transcriber",
        ]
    );
    assert_eq!(listed["data"]["templates"][0]["versions"], json!([1]));

    for name in names {
        let id = format!("{name}-x");
        let template = format!("template:{name}");
        let added = directory(
            addr,
            &repo,
            &["add", &id, "--from", &template, "--why", "from a template"],
        )
        .await;
        assert_eq!(added["ok"], true, "{name}: {added}");
        let document = committed(&repo, &id);
        let meta = section(&document, "meta");
        assert_eq!(meta["id"], id.as_str());
        assert_eq!(meta["template"], name);
        assert_eq!(meta["template_version"], "1");
        let changelog = section(&document, "changelog").as_array().unwrap();
        assert_eq!(changelog.len(), 1, "only the add's row: {changelog:?}");
        assert_eq!(changelog[0]["why"], "from a template");
    }

    let overridden = directory(
        addr,
        &repo,
        &[
            "add",
            "pm-001",
            "--from",
            "template:pm@v1",
            "--title",
            "PM for roadmap 001",
            "--instance-key",
            "001",
            "--rule",
            "Runs roadmap 001.",
            "--why",
            "pm for 001",
        ],
    )
    .await;
    assert_eq!(overridden["ok"], true, "{overridden}");
    let document = committed(&repo, "pm-001");
    let meta = section(&document, "meta");
    assert_eq!(meta["title"], "PM for roadmap 001");
    assert_eq!(meta["instance_key"], "001");
    assert_eq!(meta["template"], "pm");
    assert_eq!(meta["kind"], "general", "kept from the template");
    assert_eq!(
        section(&document, "scope")["rules"],
        json!(["Runs roadmap 001."])
    );
    assert_eq!(section(&document, "standing_packet")["source"], "inline");

    let before = head(&repo);
    for (argv, needle) in [
        (
            &["add", "pilot", "--from", "template:astronaut", "--why", "w"][..],
            "\"astronaut\"",
        ),
        (
            &["add", "pm-002", "--from", "template:pm@v9", "--why", "w"][..],
            "v9",
        ),
        (
            &[
                "add",
                "pm-003",
                "--from",
                "template:pm",
                "--template",
                "pm",
                "--why",
                "w",
            ][..],
            "--template",
        ),
    ] {
        let refused = directory(addr, &repo, argv).await;
        assert_eq!(refused["ok"], false, "{argv:?}");
        assert_eq!(refused["details"]["code"], "E-RS-ARG", "{refused}");
        let message = refused["meta"].as_str().unwrap_or_default();
        assert!(message.contains(needle), "{argv:?}: {message}");
    }
    let unknown = directory(
        addr,
        &repo,
        &["add", "pilot", "--from", "template:astronaut", "--why", "w"],
    )
    .await;
    let message = unknown["meta"].as_str().unwrap_or_default();
    assert!(
        message.contains("coder, designer, devcontainer, docs-keeper"),
        "lists what exists: {message}"
    );
    let version = directory(
        addr,
        &repo,
        &["add", "pm-2", "--from", "template:pm@v9", "--why", "w"],
    )
    .await;
    assert!(
        version["meta"].as_str().unwrap_or_default().contains("v1"),
        "{version}"
    );
    assert_eq!(head(&repo), before, "a refusal commits nothing");
    server.abort();
}

/// Plan 204 (F23, bp-0004): `templates --show` prints a shipped template's
/// exact bytes and its path; an unknown name or version refuses by name.
#[tokio::test]
async fn templates_show_prints_the_exact_embedded_body() {
    let repo = repo("pij-templates-show");
    let (addr, server, _fresh) = daemon().await;
    project(addr, &repo).await;
    let shown = directory(addr, &repo, &["templates", "--show", "security@v1"]).await;
    assert_eq!(shown["ok"], true, "{shown}");
    let template = &shown["data"]["template"];
    assert_eq!(template["path"], "templates/roles/security/v1/role.dd.json");
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/roles/security/v1/role.dd.json");
    assert_eq!(
        template["body"].as_str().unwrap(),
        std::fs::read_to_string(file).unwrap()
    );
    for (spec, needle) in [("nope", "coder, designer"), ("pm@v9", "v1")] {
        let refused = directory(addr, &repo, &["templates", "--show", spec]).await;
        assert_eq!(refused["details"]["code"], "E-RS-ARG", "{refused}");
        let message = refused["meta"].as_str().unwrap_or_default();
        assert!(message.contains(needle), "{spec}: {message}");
    }
    server.abort();
}

/// req-13: `--from file:<brief>` is the spelling of `--brief <brief>`.
#[tokio::test]
async fn from_file_is_the_brief_spelling() {
    let repo = repo("pij-templates-file");
    let (addr, server, _fresh) = daemon().await;
    project(addr, &repo).await;
    std::fs::create_dir_all(repo.gov.join("briefs")).unwrap();
    std::fs::write(repo.gov.join("briefs/qa.md"), "# QA brief\n").unwrap();
    git(&repo.gov, &["add", "briefs/qa.md"]);
    git(&repo.gov, &["commit", "-q", "-m", "brief"]);
    let flags = ["--rule", "r", "--why", "w"];
    for (id, how) in [
        ("qa-a", ["--from", "file:briefs/qa.md"]),
        ("qa-b", ["--brief", "briefs/qa.md"]),
    ] {
        let mut argv = vec!["add", id];
        argv.extend(how);
        argv.extend(flags);
        let added = directory(addr, &repo, &argv).await;
        assert_eq!(added["ok"], true, "{id}: {added}");
    }
    let packet = |id: &str| section(&committed(&repo, id), "standing_packet").clone();
    assert_eq!(
        packet("qa-a"),
        json!({"source": "brief", "brief": "briefs/qa.md"})
    );
    assert_eq!(packet("qa-a"), packet("qa-b"));
    let both = directory(
        addr,
        &repo,
        &[
            "add",
            "qa-c",
            "--from",
            "file:briefs/qa.md",
            "--brief",
            "briefs/qa.md",
            "--rule",
            "r",
            "--why",
            "w",
        ],
    )
    .await;
    assert_eq!(both["details"]["code"], "E-RS-ARG", "{both}");
    server.abort();
}

fn fixture() -> Catalog {
    let version = |n: u32, rule: &str| {
        let document = json!({"dd": {"schema": "pij/role"}, "sections": [
            {"name": "meta", "value": {"id": "qa", "title": "QA", "kind": "general",
                "lifecycle": "long-lived", "holders": "one", "template": "qa",
                "template_version": n.to_string()}},
            {"name": "scope", "value": {"rules": [rule]}},
            {"name": "standing_packet", "value": {"source": "inline", "text": "You test."}},
        ], "references": []});
        Template::new("qa", n, serde_json::to_vec_pretty(&document).unwrap())
    };
    Catalog::new(vec![
        version(1, "Test each PR once."),
        version(2, "Test each PR twice."),
    ])
}

/// AC4, bp-0003: a role from v1 with a local edit shows drift once v2 ships;
/// template-diff prints v1 to v2; the role file is byte-identical afterwards.
/// A role from v2, or from no template, shows none.
#[tokio::test]
async fn drift_shows_the_upstream_change_and_never_applies_it() {
    let repo = repo("pij-templates-drift");
    let writer = CommunityWriter::new(Arc::new(FakeDdocs), vec![], &LOCK_RETRY);
    let rig = rig_full(Arc::new(NoHook), writer, fixture()).await;
    project(rig.addr, &repo).await;
    for argv in [
        &["add", "qa-old", "--from", "template:qa@v1", "--why", "w"][..],
        &["add", "qa-new", "--from", "template:qa", "--why", "w"][..],
        &["add", "own", "--rule", "r", "--packet", "p", "--why", "w"][..],
    ] {
        let added = directory(rig.addr, &repo, argv).await;
        assert_eq!(added["ok"], true, "{argv:?}: {added}");
    }
    let path = repo.gov.join(format!("{ROLES}/qa-old/role.dd.json"));
    let local = String::from_utf8(std::fs::read(&path).unwrap())
        .unwrap()
        .replace("You test.", "You test, our way.");
    std::fs::write(&path, &local).unwrap();
    git(&repo.gov, &["commit", "-q", "-am", "local edit"]);
    let bytes = std::fs::read(&path).unwrap();
    let before = head(&repo);

    let view = view(rig.addr, &repo).await;
    assert_eq!(view["ok"], true, "{view}");
    let template = |id: &str| {
        view["data"]["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|role| role["id"] == id)
            .unwrap()["template"]
            .clone()
    };
    assert_eq!(
        template("qa-old"),
        json!({"name": "qa", "version": "1", "latest": 2, "behind": true})
    );
    assert_eq!(template("qa-new")["behind"], false);
    assert!(template("own").is_null());

    let diff = directory(rig.addr, &repo, &["template-diff", "qa-old"]).await;
    assert_eq!(diff["ok"], true, "{diff}");
    let data = &diff["data"];
    assert_eq!(
        (data["version"].clone(), data["latest"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(data["behind"], true);
    let text = data["diff"].as_str().unwrap();
    assert!(text.starts_with("--- qa@v1\n+++ qa@v2\n"), "{text}");
    let changed = |mark: char, rule: &str| {
        text.lines()
            .any(|line| line.starts_with(mark) && line.contains(rule))
    };
    assert!(changed('-', "Test each PR once."), "{text}");
    assert!(changed('+', "Test each PR twice."), "{text}");
    assert!(
        !text.contains("our way"),
        "upstream only, never the role's edits"
    );
    for id in ["qa-new", "own"] {
        let none = directory(rig.addr, &repo, &["template-diff", id]).await;
        assert_eq!(none["data"]["behind"], false, "{id}: {none}");
        assert_eq!(none["data"]["diff"], "", "{id}");
    }
    let missing = directory(rig.addr, &repo, &["template-diff", "nobody"]).await;
    assert_eq!(missing["details"]["code"], "E-RS-DIRECTORY-ROLE-MISSING");

    assert_eq!(std::fs::read(&path).unwrap(), bytes, "nothing applied");
    assert_eq!(head(&repo), before);
    assert_eq!(git(&repo.gov, &["status", "--porcelain"]), "");
    rig.server.abort();
}

/// AC5, bp-0004: an empty directory prompts for the five core roles and
/// security with runnable lines and the PEC wording, and writes nothing;
/// adding security, or a role from a core template, clears that prompt.
#[tokio::test]
async fn prompts_list_missing_core_roles_and_security_and_create_nothing() {
    let repo = repo("pij-templates-prompts");
    let (addr, server, _fresh) = daemon().await;
    project(addr, &repo).await;
    let before = head(&repo);
    let empty = view(addr, &repo).await;
    assert_eq!(empty["ok"], true, "{empty}");
    let prompted = |view: &Value| -> Vec<String> {
        view["data"]["prompts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|p| p["role"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        prompted(&empty),
        ["prime", "pm", "pa", "coder", "reviewer", "security"]
    );
    let guidance = empty["data"]["guidance"].as_str().unwrap();
    assert!(
        guidance.contains("Pij Emergent Communities (PEC)"),
        "{guidance}"
    );
    assert!(guidance.contains("examples, not a checklist"), "{guidance}");
    assert!(
        guidance.contains("reshape it as the work changes"),
        "{guidance}"
    );
    assert_eq!(head(&repo), before, "the view creates nothing");
    assert_eq!(git(&repo.gov, &["status", "--porcelain"]), "");
    assert!(!repo.gov.join(ROLES).exists());

    // Each prompt's line is runnable as printed.
    let line = empty["data"]["prompts"][5]["command"].as_str().unwrap();
    assert_eq!(
        line,
        "pij directory add security --from template:security --why \"recommended role from the security template\" --project fleet"
    );
    let added = directory(
        addr,
        &repo,
        &[
            "add",
            "security",
            "--from",
            "template:security",
            "--why",
            "recommended role from the security template",
        ],
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let pm = directory(
        addr,
        &repo,
        &["add", "pm-001", "--from", "template:pm", "--why", "w"],
    )
    .await;
    assert_eq!(pm["ok"], true, "{pm}");
    let after = view(addr, &repo).await;
    assert_eq!(prompted(&after), ["prime", "pa", "coder", "reviewer"]);
    server.abort();
}

/// A printed command split as a POSIX shell would: whitespace separates
/// words; single quotes are literal; double quotes keep spaces.
fn shell_split(line: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for ch in line.chars() {
        match (quote, ch) {
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (Some(q), _) if ch == q => quote = None,
            (None, ' ') => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            _ => {
                word.push(ch);
                started = true;
            }
        }
    }
    assert!(quote.is_none(), "unbalanced quote in {line:?}");
    if started {
        words.push(word);
    }
    words
}

/// Review of #126, F001 (pij-subsequent-reindeer): a view scoped with
/// `--project remote`, read from another project's repo, prints commands that
/// still act on `remote` when replayed from that same cwd.
#[tokio::test]
async fn a_prompt_replayed_from_another_repo_writes_to_the_viewed_project() {
    let local = repo("pij-templates-replay-local");
    let remote = repo("pij-templates-replay-remote");
    let rig = rig(Arc::new(NoHook)).await;
    project(rig.addr, &local).await;
    let created = call(
        rig.addr,
        "/v1/project",
        &[
            "project",
            "create",
            "remote",
            "--repo",
            remote.main.to_str().unwrap(),
        ],
        &local.main,
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");
    let viewed = directory(rig.addr, &local, &["--project", "remote"]).await;
    assert_eq!(viewed["data"]["project"], "remote", "{viewed}");
    let prompts = viewed["data"]["prompts"].as_array().unwrap();
    assert!(
        prompts.iter().all(|p| p["command"]
            .as_str()
            .unwrap()
            .ends_with(" --project remote")),
        "{prompts:?}"
    );
    let security = prompts.iter().find(|p| p["role"] == "security").unwrap()["command"]
        .as_str()
        .unwrap();
    let argv = shell_split(security);
    assert_eq!(&argv[..2], ["pij", "directory"], "{argv:?}");
    let argv: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let replayed = call(rig.addr, "/v1/directory", &argv, &local.main).await;
    assert_eq!(replayed["ok"], true, "{replayed}");
    assert_eq!(replayed["data"]["project"], "remote");
    let path = format!("{ROLES}/security/role.dd.json");
    assert!(remote.gov.join(&path).is_file(), "written to remote");
    assert!(!local.gov.join(&path).exists(), "nothing written locally");
    let after = directory(rig.addr, &local, &["--project", "remote"]).await;
    assert!(
        after["data"]["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["role"] != "security"),
        "{after}"
    );
    let unscoped = directory(rig.addr, &local, &[]).await;
    assert!(
        unscoped["data"]["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["role"] == "security"),
        "the local directory still lacks security: {unscoped}"
    );
    rig.server.abort();
}

/// bp-0002 with the real ddocs and the embedded schemas: every shipped
/// template, added as a role, validates and builds. Names its skip when ddocs
/// is absent (CI).
#[tokio::test]
async fn real_ddocs_validates_a_role_from_every_shipped_template() {
    let ddocs = pij_daemon::community::ddocs::Ddocs::from_env();
    if ddocs.path().is_none() {
        eprintln!(
            "SKIP real_ddocs_validates_a_role_from_every_shipped_template: ddocs is not installed here"
        );
        return;
    }
    let repo = repo("pij-templates-real-ddocs");
    let writer = CommunityWriter::new(
        Arc::new(ddocs),
        pij_daemon::community::layout::schema_copies(),
        &LOCK_RETRY,
    );
    let rig = rig_with(Arc::new(NoHook), writer).await;
    project(rig.addr, &repo).await;
    for name in Catalog::shipped().names() {
        let template = format!("template:{name}");
        let added = directory(
            rig.addr,
            &repo,
            &["add", name, "--from", &template, "--why", "real ddocs"],
        )
        .await;
        assert_eq!(added["ok"], true, "{name}: {added}");
        let tree = git(&repo.gov, &["ls-tree", "-r", "--name-only", "HEAD"]);
        assert!(
            tree.contains(&format!("{ROLES}/{name}/role.dd.md")),
            "{name}"
        );
    }
    rig.server.abort();
}
