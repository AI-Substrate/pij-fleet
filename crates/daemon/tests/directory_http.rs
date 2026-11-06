//! `pij directory add` over HTTP against a real store and real scratch git
//! repositories (plan 190, AC3 and AC6). ddocs is faked through the writer's
//! constructor; the private-daemon run in the plan's evidence uses the real one.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_daemon::community::Refusal;
use pij_daemon::community::ddocs::Validator;
use pij_daemon::community::directory::{AfterAssign, DirectoryService, NoHook};
use pij_daemon::community::layout::rendered;
use pij_daemon::community::templates::Catalog;
use pij_daemon::community::writer::{CommunityWriter, LOCK_RETRY};
use pij_daemon::delivery::role_send::RoleQueueDrain;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fakes::FakeTmux;
use pij_testkit::{FreshStore, fresh_dir};
use serde_json::{Value, json};

const KEY: &str = "directory-test-key";
const SEAT: &str = "pij-test-coder";
const PANE: &str = "%41";

/// Refuses a document holding `INVALID`; renders every other one.
struct FakeDdocs;

impl Validator for FakeDdocs {
    fn validate(&self, _: &Path, document: &Path) -> Result<(), Refusal> {
        if std::fs::read_to_string(document)
            .unwrap()
            .contains("INVALID")
        {
            return Err(Refusal {
                code: pij_daemon::community::E_DDOCS_INVALID,
                message: "missing required field \"scope\"".into(),
                next: None,
            });
        }
        Ok(())
    }

    fn build(&self, _: &Path, document: &Path) -> Result<(), Refusal> {
        std::fs::write(rendered(document), "rendered\n").unwrap();
        Ok(())
    }
}

struct Repo {
    root: PathBuf,
    main: PathBuf,
    gov: PathBuf,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn repo(name: &str) -> Repo {
    let root = fresh_dir(name).canonicalize().unwrap();
    let (main, gov) = (root.join("main"), root.join("gov"));
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "scratch"),
        ("user.email", "scratch@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(&main, &["config", key, value]);
    }
    std::fs::write(main.join("README.md"), "main\n").unwrap();
    git(&main, &["add", "README.md"]);
    git(&main, &["commit", "-q", "-m", "main"]);
    let tree = git(&main, &["mktree"]);
    let commit = git(&main, &["commit-tree", tree.trim(), "-m", "governance"]);
    git(
        &main,
        &["update-ref", "refs/heads/prime-governance", commit.trim()],
    );
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            gov.to_str().unwrap(),
            "prime-governance",
        ],
    );
    std::fs::create_dir_all(gov.join(".harness/government")).unwrap();
    std::fs::write(gov.join(".harness/government/README.md"), "government\n").unwrap();
    git(&gov, &["add", "."]);
    git(&gov, &["commit", "-q", "-m", "government"]);
    Repo { root, main, gov }
}

struct Rig {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    _fresh: FreshStore,
    store: pij_store::SqliteOrchestration,
    roles: Arc<pij_daemon::http::role::RoleService>,
    tmux: Arc<FakeTmux>,
    delivery: Arc<pij_daemon::delivery::DeliveryService>,
    services: pij_daemon::Services,
}

/// Counts after-assign calls (ruling C6).
#[derive(Default)]
struct CountingHook(std::sync::Mutex<Vec<String>>);

impl AfterAssign for CountingHook {
    fn assigned(&self, project: &str, role: &str, instance: &str, seat: &SeatId) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{project}:{role}/{instance}:{seat}"));
    }
}

async fn daemon() -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore) {
    let rig = rig(Arc::new(NoHook)).await;
    (rig.addr, rig.server, rig._fresh)
}

async fn rig(hook: Arc<dyn AfterAssign>) -> Rig {
    let writer = CommunityWriter::new(Arc::new(FakeDdocs), vec![], &LOCK_RETRY);
    rig_with(hook, writer).await
}

/// The daemon's own after-assign hook: the role queue drain (plan 199, C6).
async fn draining_rig() -> Rig {
    let writer = CommunityWriter::new(Arc::new(FakeDdocs), vec![], &LOCK_RETRY);
    rig_inner(None, writer, Catalog::shipped()).await
}

async fn rig_with(hook: Arc<dyn AfterAssign>, writer: CommunityWriter) -> Rig {
    rig_full(hook, writer, Catalog::shipped()).await
}

async fn rig_full(hook: Arc<dyn AfterAssign>, writer: CommunityWriter, catalog: Catalog) -> Rig {
    rig_inner(Some(hook), writer, catalog).await
}

/// `None` installs the daemon's own drain, which needs the built services.
async fn rig_inner(
    hook: Option<Arc<dyn AfterAssign>>,
    writer: CommunityWriter,
    catalog: Catalog,
) -> Rig {
    let fresh = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: fresh.path(),
        ..Config::default()
    };
    let signals = Path::new(&config.store_path).with_extension("signals");
    let mut services = pij_daemon::build_services(&config, &signals)
        .await
        .expect("services");
    let writer = Arc::new(writer);
    let store = services.directory.store().clone();
    services.community = Arc::clone(&writer);
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    let hook = hook.unwrap_or_else(|| {
        Arc::new(RoleQueueDrain::new(
            store.clone(),
            Arc::clone(&services.delivery),
        ))
    });
    let delivery = Arc::clone(&services.delivery);
    services.directory = Arc::new(
        DirectoryService::new(
            writer,
            store.clone(),
            Arc::clone(&services.event_bus),
            hook,
            tmux.clone(),
        )
        .with_templates(catalog),
    );
    for (id, pane, parent) in [
        (SEAT, Some(PANE), None),
        ("pij-holder", Some("%51"), Some(SEAT)),
        ("pij-second", None, Some(SEAT)),
    ] {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/tmp");
        seat.pane = pane.map(str::to_string);
        seat.parent = parent.map(SeatId::from);
        services.registry.put(seat).await.expect("seed seat");
    }
    let roles = Arc::clone(&services.roles);
    let kept = services.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = router_with_config(services, HttpConfig::local(KEY.to_string()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Rig {
        addr,
        server,
        _fresh: fresh,
        store,
        roles,
        tmux,
        delivery,
        services: kept,
    }
}

async fn call(addr: SocketAddr, path: &str, argv: &[&str], cwd: &Path) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .json(&json!({"argv": argv, "caller": {"TMUX_PANE": PANE, "cwd": cwd}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn head(repo: &Repo) -> String {
    git(&repo.gov, &["rev-parse", "HEAD"]).trim().to_string()
}

#[tokio::test]
async fn add_writes_commits_and_points_at_a_role_then_refuses_it_twice() {
    let repo = repo("pij-directory-add");
    let (addr, server, _fresh) = daemon().await;
    // The project records the repo through a symlink and the caller sends the
    // real path (macOS: /tmp versus /private/tmp). Both name one directory.
    let alias = repo.root.join("alias");
    std::os::unix::fs::symlink(&repo.main, &alias).unwrap();
    let created = call(
        addr,
        "/v1/project",
        &[
            "project",
            "create",
            "fleet",
            "--repo",
            alias.to_str().unwrap(),
        ],
        &repo.main,
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");

    let added = call(
        addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "security",
            "--title",
            "Security",
            "--kind",
            "specialist",
            "--lifecycle",
            "long-lived",
            "--holders",
            "one",
            "--rule",
            "A verdict on every PR.",
            "--rule",
            "Secrets never in logs.",
            "--packet",
            "You hold the security role.",
            "--why",
            "first security role",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let data = &added["data"];
    let path = ".harness/government/community/roles/security/role.dd.json";
    assert_eq!(data["project"], "fleet");
    assert_eq!(data["path"], path);
    assert_eq!(data["commit"], head(&repo));
    let committed: Value =
        serde_json::from_str(&git(&repo.gov, &["show", &format!("HEAD:{path}")])).unwrap();
    let changelog = committed["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["name"] == "changelog")
        .unwrap();
    assert_eq!(changelog["value"][0]["by"], SEAT);
    assert_eq!(changelog["value"][0]["why"], "first security role");
    let message = git(&repo.gov, &["log", "-1", "--format=%B"]);
    assert!(
        message.starts_with("community: add role security"),
        "{message}"
    );
    assert!(message.contains(&format!("Pij-Seat: {SEAT}")), "{message}");

    let again = call(
        addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "security",
            "--rule",
            "x",
            "--why",
            "again",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(again["ok"], false);
    assert_eq!(again["details"]["code"], "E-RS-DIRECTORY-EXISTS");
    assert_eq!(head(&repo), data["commit"]);
    assert_eq!(git(&repo.gov, &["status", "--porcelain"]), "");
    server.abort();
}

/// AC6: eight role fixtures shaped like the hand-written roles land as one
/// commit; the same batch with one invalid document commits none.
#[tokio::test]
async fn a_batch_is_one_commit_all_or_nothing() {
    let repo = repo("pij-directory-batch");
    let (addr, server, _fresh) = daemon().await;
    let main = repo.main.to_str().unwrap();
    call(
        addr,
        "/v1/project",
        &["project", "create", "fleet", "--repo", main],
        &repo.main,
    )
    .await;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/community-roles");
    let start = head(&repo);

    let spoiled = fresh_dir("pij-directory-spoiled");
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        let mut text = std::fs::read_to_string(&path).unwrap();
        if path.file_name().unwrap() == "reviewer.dd.json" {
            text = text.replace("\"scope\"", "\"INVALID\"");
        }
        std::fs::write(spoiled.join(path.file_name().unwrap()), text).unwrap();
    }
    let refused = call(
        addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "--batch",
            spoiled.to_str().unwrap(),
            "--why",
            "import",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(
        refused["details"]["code"], "E-RS-DDOCS-INVALID",
        "{refused}"
    );
    assert!(
        refused["meta"]
            .as_str()
            .unwrap()
            .contains("roles/reviewer/role.dd.json"),
        "{refused}"
    );
    assert_eq!(head(&repo), start);
    assert_eq!(
        git(
            &repo.gov,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );

    let imported = call(
        addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "--batch",
            fixtures.to_str().unwrap(),
            "--why",
            "import the hand-written roles",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(imported["ok"], true, "{imported}");
    assert_eq!(imported["data"]["roles"].as_array().unwrap().len(), 8);
    assert_eq!(
        git(
            &repo.gov,
            &["rev-list", "--count", &format!("{start}..HEAD")]
        )
        .trim(),
        "1"
    );
    let tree = git(&repo.gov, &["ls-tree", "-r", "--name-only", "HEAD"]);
    for id in [
        "coder",
        "delivery",
        "docs-system",
        "pa",
        "performance",
        "pm-001",
        "reviewer",
        "security",
    ] {
        assert!(
            tree.contains(&format!("community/roles/{id}/role.dd.json")),
            "{id}"
        );
        assert!(
            tree.contains(&format!("community/roles/{id}/memory/INDEX.md")),
            "{id}"
        );
    }
    let _ = std::fs::remove_dir_all(spoiled);
    server.abort();
}

#[tokio::test]
async fn missing_governance_refuses_with_the_bootstrap_step() {
    let root = fresh_dir("pij-directory-nogov").canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let (addr, server, _fresh) = daemon().await;
    let cwd = root.to_str().unwrap();
    call(
        addr,
        "/v1/project",
        &["project", "create", "bare", "--repo", cwd],
        &root,
    )
    .await;
    let refused = call(
        addr,
        "/v1/directory",
        &["directory", "add", "qa", "--rule", "x", "--why", "w"],
        &root,
    )
    .await;
    assert_eq!(refused["details"]["code"], "E-RS-GOV-ABSENT", "{refused}");
    assert!(
        refused["details"]["next"]
            .as_str()
            .unwrap()
            .contains("/pij prime"),
        "{refused}"
    );
    let _ = std::fs::remove_dir_all(root);
    server.abort();
}

/// Plan 195, AC10: the shim forwards `--help` to the daemon, which answers it
/// rather than refusing `--help requires a value`.
#[tokio::test]
async fn help_is_answered_not_refused() {
    let (addr, server, _fresh) = daemon().await;
    let cwd = std::env::temp_dir();
    for (argv, needle) in [
        (vec!["directory", "--help"], "pij directory <verb> --help"),
        (vec!["directory", "add", "--help"], "--batch <dir>"),
        (vec!["directory", "add", "qa", "-h"], "--why <text>"),
    ] {
        let answer = call(addr, "/v1/directory", &argv, &cwd).await;
        assert_eq!(answer["ok"], true, "{argv:?}: {answer}");
        assert!(
            answer["data"]["help"].as_str().unwrap().contains(needle),
            "{argv:?}"
        );
    }
    let unknown = call(
        addr,
        "/v1/directory",
        &["directory", "nope", "--help"],
        &cwd,
    )
    .await;
    assert_eq!(unknown["details"]["code"], "E-RS-UNPORTED", "{unknown}");
    server.abort();
}

// Plan 195: holding roles.

async fn with_roles(addr: SocketAddr, repo: &Repo) {
    let main = repo.main.to_str().unwrap();
    let created = call(
        addr,
        "/v1/project",
        &["project", "create", "fleet", "--repo", main],
        &repo.main,
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");
    for (id, holders) in [("pm-001", "one"), ("reviewer", "many")] {
        let added = call(
            addr,
            "/v1/directory",
            &[
                "directory",
                "add",
                id,
                "--title",
                id,
                "--kind",
                "general",
                "--lifecycle",
                "long-lived",
                "--holders",
                holders,
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
}

fn holders_at_head(repo: &Repo, role: &str) -> Vec<Value> {
    let text = git(
        &repo.gov,
        &[
            "show",
            &format!("HEAD:.harness/government/community/roles/{role}/holders.dd.json"),
        ],
    );
    let document: Value = serde_json::from_str(&text).unwrap();
    document["sections"][1]["value"].as_array().unwrap().clone()
}

/// AC2, AC3 (C1), AC5 (C6) and release.
#[tokio::test]
async fn assign_and_release_write_holders_set_seat_roles_and_call_the_hook_once_each() {
    let repo = repo("pij-directory-assign");
    let hook = Arc::new(CountingHook::default());
    let rig = rig(hook.clone()).await;
    with_roles(rig.addr, &repo).await;
    let dir = |argv: &'static [&'static str]| call(rig.addr, "/v1/directory", argv, &repo.main);

    let first = dir(&[
        "directory",
        "assign",
        "pm-001",
        "pij-holder",
        "--why",
        "lead",
    ])
    .await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["data"]["seat_role"], "pm");
    assert_eq!(first["data"]["commit"], head(&repo));
    assert_eq!(
        first["data"]["path"],
        ".harness/government/community/roles/pm-001/holders.dd.json"
    );
    assert!(first["data"].get("warning").is_none());
    assert_eq!(
        first["data"]["window"],
        json!({"name": "fleet:pm-001", "renamed": true})
    );
    assert!(
        rig.tmux
            .calls()
            .contains(&"rename-window:%51:fleet:pm-001".to_string())
    );
    let rows = holders_at_head(&repo, "pm-001");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0]["seat"].as_str(),
            rows[0]["assigned_by"].as_str(),
            rows[0]["why"].as_str()
        ),
        (Some("pij-holder"), Some(SEAT), Some("lead"))
    );
    assert_eq!(
        rig.roles
            .read_role(&SeatId::from("pij-holder"))
            .await
            .unwrap()
            .as_deref(),
        Some("pm"),
        "the directory and seat_roles agree (C1)"
    );

    let second = dir(&[
        "directory",
        "assign",
        "pm-001",
        "pij-second",
        "--why",
        "cover",
    ])
    .await;
    assert_eq!(second["ok"], true, "{second}");
    assert!(
        second["data"]["warning"]
            .as_str()
            .unwrap()
            .starts_with("W-ONE-HOLDER"),
        "{second}"
    );
    assert_eq!(
        second["data"]["window"]["renamed"], false,
        "a paneless seat still assigns"
    );
    assert_eq!(second["data"]["window"]["reason"], "the seat has no pane");

    let before = head(&repo);
    for (argv, code) in [
        (
            &[
                "directory",
                "assign",
                "pm-001",
                "pij-holder",
                "--why",
                "again",
            ][..],
            "E-RS-DIRECTORY-HELD",
        ),
        (
            &["directory", "assign", "qa", "pij-holder", "--why", "x"][..],
            "E-RS-DIRECTORY-ROLE-MISSING",
        ),
        (
            &["directory", "assign", "pm-001", "pij-nobody", "--why", "x"][..],
            "E-RS-NOT-FOUND",
        ),
        (
            &["directory", "assign", "pm-001", "pij-holder"][..],
            "E-RS-ARG",
        ),
    ] {
        let refused = call(rig.addr, "/v1/directory", argv, &repo.main).await;
        assert_eq!(refused["details"]["code"], code, "{argv:?}: {refused}");
    }
    assert_eq!(head(&repo), before, "a refused assign writes nothing");

    let review = dir(&[
        "directory",
        "assign",
        "reviewer/114",
        "pij-holder",
        "--why",
        "review",
    ])
    .await;
    assert_eq!(review["ok"], true, "{review}");
    assert_eq!(review["data"]["seat_role"], "worker");
    assert_eq!(review["data"]["window"]["name"], "fleet:reviewer-114");
    assert_eq!(holders_at_head(&repo, "reviewer")[0]["instance_key"], "114");
    assert_eq!(
        *hook.0.lock().unwrap(),
        [
            "fleet:pm-001/:pij-holder",
            "fleet:pm-001/:pij-second",
            "fleet:reviewer/114:pij-holder",
        ],
        "one hook call per successful assign, none for a refused one"
    );

    let released = dir(&[
        "directory",
        "release",
        "pm-001",
        "pij-holder",
        "--why",
        "done",
    ])
    .await;
    assert_eq!(released["ok"], true, "{released}");
    let row = &holders_at_head(&repo, "pm-001")[0];
    assert_eq!(
        (row["released_by"].as_str(), row["release_why"].as_str()),
        (Some(SEAT), Some("done"))
    );
    assert!(row["to"].is_string());
    let open: Vec<_> = rig
        .store
        .open_assignments("pij-holder")
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.role)
        .collect();
    assert_eq!(open, ["reviewer"]);
    assert_eq!(
        rig.roles
            .read_role(&SeatId::from("pij-holder"))
            .await
            .unwrap()
            .as_deref(),
        Some("worker"),
        "release leaves seat_roles alone"
    );
    let again = dir(&[
        "directory",
        "release",
        "pm-001",
        "pij-holder",
        "--why",
        "done",
    ])
    .await;
    assert_eq!(
        again["details"]["code"], "E-RS-DIRECTORY-NOT-HELD",
        "{again}"
    );
    rig.server.abort();
}

/// AC4: close ends holdings; a refused end stays pending and the next write
/// for the project carries it.
#[tokio::test]
async fn close_ends_holdings_and_a_refused_end_stays_pending_until_the_next_write() {
    let repo = repo("pij-directory-close");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    for argv in [
        &[
            "directory",
            "assign",
            "pm-001",
            "pij-holder",
            "--why",
            "lead",
        ][..],
        &[
            "directory",
            "assign",
            "reviewer/114",
            "pij-second",
            "--why",
            "review",
        ][..],
    ] {
        let assigned = call(rig.addr, "/v1/directory", argv, &repo.main).await;
        assert_eq!(assigned["ok"], true, "{assigned}");
    }
    let close = |seat: &str| {
        reqwest::Client::new()
            .post(format!("http://{}/v1/close", rig.addr))
            .bearer_auth(KEY)
            .json(&json!({"seat": seat, "reason": "done", "caller": {"TMUX_PANE": PANE, "cwd": repo.main}}))
            .send()
    };

    let closed: Value = close("pij-holder").await.unwrap().json().await.unwrap();
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(
        closed["data"]["directory"]["committed"][0]["project"], "fleet",
        "{closed}"
    );
    let row = &holders_at_head(&repo, "pm-001")[0];
    assert_eq!(
        (row["release_why"].as_str(), row["released_by"].as_str()),
        (Some("close"), Some(SEAT))
    );

    git(
        &repo.main,
        &["worktree", "remove", "--force", repo.gov.to_str().unwrap()],
    );
    let pending: Value = close("pij-second").await.unwrap().json().await.unwrap();
    assert_eq!(
        pending["ok"], true,
        "the close never fails on the directory: {pending}"
    );
    assert!(pending["data"]["tombstoned_at"].is_number(), "{pending}");
    assert_eq!(
        pending["data"]["directory"]["pending"][0]["code"], "E-RS-GOV-ABSENT",
        "{pending}"
    );
    let stored = rig.store.assignments("fleet").await.unwrap();
    assert!(stored.iter().any(|row| row.seat == "pij-second"
        && row.end_pending
        && row.release_why.as_deref() == Some("close")));

    git(
        &repo.main,
        &[
            "worktree",
            "add",
            "-q",
            repo.gov.to_str().unwrap(),
            "prime-governance",
        ],
    );
    assert!(
        holders_at_head(&repo, "reviewer")[0]["to"].is_null(),
        "not on the branch yet"
    );
    let added = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "qa",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "next write",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    let row = &holders_at_head(&repo, "reviewer")[0];
    assert_eq!(
        row["release_why"], "close",
        "the next write flushed the pending end"
    );
    assert!(
        rig.store
            .assignments("fleet")
            .await
            .unwrap()
            .iter()
            .all(|row| !row.end_pending)
    );
    rig.server.abort();
}

async fn view(addr: SocketAddr, repo: &Repo) -> Value {
    call(addr, "/v1/directory", &["directory", "--json"], &repo.main).await
}

/// AC7 and AC8: each project sees only its own roles; dropping the store's
/// directory rows and rebuilding from the branch head gives byte-identical JSON,
/// closed lineage included.
#[tokio::test]
async fn the_view_is_per_project_and_a_rebuild_gives_byte_identical_json() {
    let (one, two) = (repo("pij-directory-view-a"), repo("pij-directory-view-b"));
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &one).await;
    let other = call(
        rig.addr,
        "/v1/project",
        &[
            "project",
            "create",
            "other",
            "--repo",
            two.main.to_str().unwrap(),
        ],
        &two.main,
    )
    .await;
    assert_eq!(other["ok"], true, "{other}");
    let security = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "security",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "w",
        ],
        &two.main,
    )
    .await;
    assert_eq!(security["ok"], true, "{security}");
    for argv in [
        &[
            "directory",
            "assign",
            "pm-001",
            "pij-holder",
            "--why",
            "lead",
        ][..],
        &[
            "directory",
            "assign",
            "reviewer/114",
            "pij-second",
            "--why",
            "review",
        ][..],
        &[
            "directory",
            "release",
            "reviewer/114",
            "pij-second",
            "--why",
            "merged",
        ][..],
    ] {
        let done = call(rig.addr, "/v1/directory", argv, &one.main).await;
        assert_eq!(done["ok"], true, "{argv:?}: {done}");
    }

    let before = view(rig.addr, &one).await;
    assert_eq!(before["ok"], true, "{before}");
    let data = &before["data"];
    let ids: Vec<_> = data["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["pm-001", "reviewer"],
        "nothing from the other project"
    );
    assert_eq!(data["roles"][0]["live"][0]["seat"], "pij-holder");
    assert_eq!(data["roles"][0]["holders"], "one");
    assert!(data["roles"][1]["live"].as_array().unwrap().is_empty());
    assert_eq!(
        data["roles"][1]["lineage"][0]["release_why"], "merged",
        "closed lineage is kept"
    );
    let theirs = view(rig.addr, &two).await;
    assert_eq!(theirs["data"]["roles"][0]["id"], "security");
    assert_eq!(theirs["data"]["roles"].as_array().unwrap().len(), 1);

    rig.store.replace_assignments("fleet", &[]).await.unwrap();
    rig.store
        .replace_community_pointers("fleet", &[])
        .await
        .unwrap();
    let dropped = view(rig.addr, &one).await;
    assert_ne!(
        dropped["data"], before["data"],
        "the view reads holders from the store"
    );
    let rebuilt = call(
        rig.addr,
        "/v1/directory",
        &["directory", "rebuild"],
        &one.main,
    )
    .await;
    assert_eq!(rebuilt["ok"], true, "{rebuilt}");
    assert_eq!(rebuilt["data"]["holdings"], 2);
    let after = view(rig.addr, &one).await;
    assert_eq!(
        serde_json::to_string(&after["data"]).unwrap(),
        serde_json::to_string(&before["data"]).unwrap(),
        "byte-identical after the rebuild"
    );
    assert!(
        !rig.store
            .community_pointers("fleet")
            .await
            .unwrap()
            .is_empty()
    );
    rig.server.abort();
}

/// AC13, AC14 (dogfood F3, F4, F11): a hand-written role folder is adopted.
/// Only role.dd.json and its .dd.md are written; its memory, outputs and
/// role.md stay exactly as they were, even with an uncommitted memory edit.
#[tokio::test]
async fn add_adopts_a_hand_written_role_folder_and_writes_only_the_document() {
    let repo = repo("pij-directory-adopt");
    let rig = rig(Arc::new(NoHook)).await;
    let main = repo.main.to_str().unwrap();
    call(
        rig.addr,
        "/v1/project",
        &["project", "create", "fleet", "--repo", main],
        &repo.main,
    )
    .await;
    let roles = repo.gov.join(".harness/government/community/roles");
    for id in ["coder", "pa"] {
        std::fs::create_dir_all(roles.join(id).join("memory")).unwrap();
        std::fs::create_dir_all(roles.join(id).join("outputs")).unwrap();
        std::fs::write(roles.join(id).join("memory/INDEX.md"), "# memory\n").unwrap();
        std::fs::write(roles.join(id).join("outputs/LOG.md"), "log\n").unwrap();
        std::fs::write(roles.join(id).join("role.md"), "# Role\n").unwrap();
    }
    git(&repo.gov, &["add", ".harness/government/community"]);
    git(&repo.gov, &["commit", "-q", "-m", "hand-written roles"]);
    std::fs::write(
        roles.join("coder/memory/INDEX.md"),
        "# memory\n- an uncommitted note\n",
    )
    .unwrap();
    let before = git(&repo.gov, &["status", "--porcelain"]);

    let adopted = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "coder",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "adopt",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(adopted["ok"], true, "{adopted}");
    let committed = git(&repo.gov, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        committed.lines().collect::<Vec<_>>(),
        [
            ".harness/government/community/roles/coder/role.dd.json",
            ".harness/government/community/roles/coder/role.dd.md",
        ]
    );
    assert_eq!(
        git(&repo.gov, &["status", "--porcelain"]),
        before,
        "the memory edit stays as it was"
    );
    assert_eq!(
        std::fs::read_to_string(roles.join("coder/role.md")).unwrap(),
        "# Role\n"
    );
    assert_eq!(
        std::fs::read_to_string(roles.join("coder/outputs/LOG.md")).unwrap(),
        "log\n"
    );

    let again = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "coder",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "again",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(again["details"]["code"], "E-RS-DIRECTORY-EXISTS", "{again}");
    assert!(
        again["meta"]
            .as_str()
            .unwrap()
            .contains("roles/coder/role.dd.json"),
        "{again}"
    );
    assert!(
        again["details"]["next"]
            .as_str()
            .unwrap()
            .contains("adopted"),
        "{again}"
    );

    // A batch names every role that already has its document (F11).
    let adopted_pa = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "pa",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "adopt",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(adopted_pa["ok"], true, "{adopted_pa}");
    let batch = fresh_dir("pij-directory-adopt-batch");
    for id in ["coder", "pa"] {
        let document = json!({"dd": {"schema": "pij/role"}, "sections": [{"name": "meta", "value": {"id": id}}], "references": []});
        std::fs::write(batch.join(format!("{id}.dd.json")), document.to_string()).unwrap();
    }
    let refused = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "--batch",
            batch.to_str().unwrap(),
            "--why",
            "batch",
        ],
        &repo.main,
    )
    .await;
    let meta = refused["meta"].as_str().unwrap();
    for id in ["coder", "pa"] {
        assert!(
            meta.contains(&format!(
                "role \"{id}\" already has .harness/government/community/roles/{id}/role.dd.json"
            )),
            "F11: every conflicting role with the file that triggered it: {refused}"
        );
    }
    assert!(
        refused["details"]["next"]
            .as_str()
            .unwrap()
            .contains("adopted"),
        "{refused}"
    );
    let _ = std::fs::remove_dir_all(batch);
    rig.server.abort();
}

/// AC15 (dogfood F2): a linked worktree finds its project through the shared
/// common git dir; a repository with no project names the register step.
#[tokio::test]
async fn a_linked_worktree_finds_its_project_and_an_unknown_repo_names_the_register_step() {
    let repo = repo("pij-directory-linked");
    let rig = rig(Arc::new(NoHook)).await;
    let main = repo.main.to_str().unwrap();
    call(
        rig.addr,
        "/v1/project",
        &["project", "create", "fleet", "--repo", main],
        &repo.main,
    )
    .await;
    let linked = repo.root.join("fleet-feature");
    git(
        &repo.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ],
    );
    let added = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "qa",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "from a linked worktree",
        ],
        &linked,
    )
    .await;
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["data"]["project"], "fleet");

    let stranger = fresh_dir("pij-directory-stranger").canonicalize().unwrap();
    git(&stranger, &["init", "-q", "-b", "main"]);
    let refused = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            "qa",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "w",
        ],
        &stranger,
    )
    .await;
    assert_eq!(refused["details"]["code"], "E-RS-NOT-FOUND", "{refused}");
    assert!(
        refused["details"]["next"]
            .as_str()
            .unwrap()
            .contains("pij project create"),
        "{refused}"
    );
    let _ = std::fs::remove_dir_all(stranger);
    rig.server.abort();
}

/// AC9 (req-16): external entries append to community/external.dd.json.
#[tokio::test]
async fn external_add_creates_then_appends_dated_entries() {
    let repo = repo("pij-directory-external");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    for seat in ["pij-visitor", "a borrowed reviewer"] {
        let added = call(
            rig.addr,
            "/v1/directory",
            &[
                "directory",
                "external",
                "add",
                seat,
                "--from-project",
                "osk",
                "--role",
                "reviewer",
                "--why",
                "on loan for #120",
            ],
            &repo.main,
        )
        .await;
        assert_eq!(added["ok"], true, "{added}");
        assert_eq!(
            added["data"]["path"],
            ".harness/government/community/external.dd.json"
        );
    }
    let text = git(
        &repo.gov,
        &[
            "show",
            "HEAD:.harness/government/community/external.dd.json",
        ],
    );
    let document: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(document["dd"]["schema"], "pij/external");
    let entries = document["sections"][1]["value"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        (
            entries[0]["seat"].as_str(),
            entries[0]["from_project"].as_str(),
            entries[0]["by"].as_str(),
            entries[0]["role"].as_str()
        ),
        (
            Some("pij-visitor"),
            Some("osk"),
            Some(SEAT),
            Some("reviewer")
        )
    );
    let refused = call(
        rig.addr,
        "/v1/directory",
        &["directory", "external", "add", "x", "--why", "w"],
        &repo.main,
    )
    .await;
    assert_eq!(refused["details"]["code"], "E-RS-ARG", "{refused}");
    rig.server.abort();
}

/// AC1's Rust half and AC2/AC9 with the real ddocs and the embedded schemas:
/// role, holders and external documents written by pij validate and build.
/// Names its skip when ddocs is absent (CI).
#[tokio::test]
async fn real_ddocs_validates_what_assign_release_and_external_write() {
    let ddocs = pij_daemon::community::ddocs::Ddocs::from_env();
    if ddocs.path().is_none() {
        eprintln!(
            "SKIP real_ddocs_validates_what_assign_release_and_external_write: ddocs is not installed here"
        );
        return;
    }
    let repo = repo("pij-directory-real-ddocs");
    let writer = CommunityWriter::new(
        Arc::new(ddocs),
        pij_daemon::community::layout::schema_copies(),
        &LOCK_RETRY,
    );
    let rig = rig_with(Arc::new(NoHook), writer).await;
    let main = repo.main.to_str().unwrap();
    call(
        rig.addr,
        "/v1/project",
        &["project", "create", "fleet", "--repo", main],
        &repo.main,
    )
    .await;
    for argv in [
        &[
            "directory",
            "add",
            "reviewer",
            "--title",
            "Reviewer",
            "--kind",
            "general",
            "--lifecycle",
            "per-task",
            "--holders",
            "many",
            "--rule",
            "Review each PR.",
            "--packet",
            "You review.",
            "--why",
            "real",
        ][..],
        &[
            "directory",
            "assign",
            "reviewer/114",
            "pij-holder",
            "--why",
            "review #114",
        ][..],
        &[
            "directory",
            "release",
            "reviewer/114",
            "pij-holder",
            "--why",
            "merged",
        ][..],
        &[
            "directory",
            "external",
            "add",
            "pij-visitor",
            "--from-project",
            "osk",
            "--why",
            "loan",
        ][..],
    ] {
        let done = call(rig.addr, "/v1/directory", argv, &repo.main).await;
        assert_eq!(done["ok"], true, "{argv:?}: {done}");
    }
    let tree = git(&repo.gov, &["ls-tree", "-r", "--name-only", "HEAD"]);
    for path in [
        ".dd/schemas/pij/holders/schema.json",
        ".dd/schemas/pij/external/schema.json",
        ".harness/government/community/roles/reviewer/holders.dd.md",
        ".harness/government/community/external.dd.md",
    ] {
        assert!(tree.contains(path), "{path} missing from:\n{tree}");
    }
    rig.server.abort();
}

/// PR #122 review regressions (pij-substantial-mammal): same-role writes
/// flush pending close-ends, and a same-second reassign keeps both holdings.
#[path = "pr122_review/mod.rs"]
mod review;

async fn send_to(addr: SocketAddr, cwd: &Path, to: &str, msg_id: &str, extra: Value) -> Value {
    let mut body = json!({
        "from": SEAT,
        "to": {"seat": to, "machine": null},
        "body": format!("please {msg_id}"),
        "msg_id": msg_id,
        "caller": {"TMUX_PANE": PANE, "cwd": cwd},
    });
    for (key, value) in extra.as_object().into_iter().flatten() {
        body[key] = value.clone();
    }
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn refusal_code(response: &Value) -> &str {
    response["details"]["code"].as_str().unwrap_or_default()
}

async fn inbox_bodies(rig: &Rig, seat: &str) -> Vec<String> {
    rig.delivery
        .peek_inbox(&SeatId::from(seat))
        .await
        .unwrap()
        .into_iter()
        .map(|claim| claim.message.body)
        .collect()
}

/// Plan 199, AC1 and AC5: one live holder gets an ordinary send carrying the
/// request header; the receipt names the request (C3) with its reply mode and
/// limit, and the store records it delivered. Role-only flags refuse on a seat
/// send; a bad instance, an unknown role and a C1 mismatch refuse by name.
#[tokio::test]
async fn a_role_send_reaches_its_one_holder_and_records_the_request() {
    let repo = repo("pij-role-send-one");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let assigned = call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "assign",
            "pm-001",
            "pij-holder",
            "--why",
            "lead",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(assigned["ok"], true, "{assigned}");

    let sent = send_to(
        rig.addr,
        &repo.main,
        "role:pm-001",
        "req-one",
        json!({"reply": "fyi", "limit": "30m"}),
    )
    .await;
    assert_eq!(sent["ok"], true, "{sent}");
    let request = &sent["data"]["request"];
    assert_eq!(
        (
            request["id"].as_str(),
            request["state"].as_str(),
            request["holder"].as_str(),
            request["reply"].as_str(),
            request["limit_ms"].as_u64(),
        ),
        (
            Some("req-one"),
            Some("delivered"),
            Some("pij-holder"),
            Some("fyi"),
            Some(1_800_000)
        ),
        "{sent}"
    );
    assert_eq!(sent["data"]["msg_id"], "req-one");
    let stored = rig
        .store
        .role_request("req-one")
        .await
        .unwrap()
        .expect("recorded");
    assert_eq!(
        (
            stored.requester.as_str(),
            stored.holder.as_deref(),
            stored.project.as_str()
        ),
        (SEAT, Some("pij-holder"), "fleet")
    );

    let seat_send = send_to(
        rig.addr,
        &repo.main,
        "pij-holder",
        "req-seat",
        json!({"reply": "none"}),
    )
    .await;
    assert_eq!(seat_send["ok"], false);
    assert_eq!(refusal_code(&seat_send), "E-RS-ROLE-ONLY", "{seat_send}");
    let instance = send_to(rig.addr, &repo.main, "role:pm-001/a", "req-inst", json!({})).await;
    assert_eq!(refusal_code(&instance), "E-RS-ROLE-ADDRESS", "{instance}");
    let unknown = send_to(
        rig.addr,
        &repo.main,
        "role:nobody",
        "req-unknown",
        json!({}),
    )
    .await;
    assert_eq!(
        refusal_code(&unknown),
        "E-RS-DIRECTORY-ROLE-MISSING",
        "{unknown}"
    );
    let remote = send_to(
        rig.addr,
        &repo.main,
        "role:pm-001",
        "req-remote",
        json!({"to": {"seat": "role:pm-001", "machine": "elsewhere"}}),
    )
    .await;
    assert_eq!(
        remote["ok"], false,
        "role addresses are never forwarded: {remote}"
    );

    rig.roles
        .assert_role(
            &SeatId::from(SEAT),
            &SeatId::from("pij-holder"),
            Some("worker".into()),
        )
        .await
        .expect("re-role the holder");
    let mismatch = send_to(
        rig.addr,
        &repo.main,
        "role:pm-001",
        "req-mismatch",
        json!({}),
    )
    .await;
    assert_eq!(refusal_code(&mismatch), "E-RS-ROLE-MISMATCH", "{mismatch}");
    let message = mismatch["meta"].as_str().unwrap_or_default();
    assert!(
        message.contains("pm") && message.contains("worker"),
        "{mismatch}"
    );
    assert!(
        rig.store
            .role_request("req-mismatch")
            .await
            .unwrap()
            .is_none()
    );
    rig.server.abort();
}

/// Plan 199, AC1 and AC4: two live holders of a many-holder role refuse with
/// both named; an instance address reaches only that instance's holder.
#[tokio::test]
async fn several_holders_refuse_by_name_and_an_instance_reaches_only_its_holder() {
    let repo = repo("pij-role-send-many");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    for (target, seat) in [
        ("reviewer/196", "pij-holder"),
        ("reviewer/197", "pij-second"),
    ] {
        let assigned = call(
            rig.addr,
            "/v1/directory",
            &["directory", "assign", target, seat, "--why", "review"],
            &repo.main,
        )
        .await;
        assert_eq!(assigned["ok"], true, "{assigned}");
    }
    let both = send_to(rig.addr, &repo.main, "role:reviewer", "req-both", json!({})).await;
    assert_eq!(refusal_code(&both), "E-RS-ROLE-HOLDERS", "{both}");
    let message = both["meta"].as_str().unwrap_or_default();
    assert!(
        message.contains("pij-holder (196)") && message.contains("pij-second (197)"),
        "{both}"
    );
    let one = send_to(
        rig.addr,
        &repo.main,
        "role:reviewer/197",
        "req-197",
        json!({}),
    )
    .await;
    assert_eq!(one["ok"], true, "{one}");
    assert_eq!(one["data"]["request"]["holder"], "pij-second");
    assert_eq!(one["data"]["request"]["instance"], "197");
    rig.server.abort();
}

/// Plan 199, AC2 and AC3: with no live holder a send queues (an --fyi send
/// too, C2) and wakes the project's PA once, naming the last holder to revive
/// and the assign command. A refused assign drains nothing; an assign delivers
/// the queue to the new holder in order and marks each request delivered (C6).
#[tokio::test]
async fn no_holder_queues_wakes_the_pa_and_an_assign_drains_in_order() {
    let repo = repo("pij-role-send-queue");
    let rig = draining_rig().await;
    with_roles(rig.addr, &repo).await;
    let dir = |argv: Vec<&'static str>| {
        let (addr, main) = (rig.addr, repo.main.clone());
        async move { call(addr, "/v1/directory", &argv, &main).await }
    };
    let added = dir(vec![
        "directory",
        "add",
        "pa",
        "--title",
        "pa",
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
        "--why",
        "w",
    ])
    .await;
    assert_eq!(added["ok"], true, "{added}");
    for argv in [
        vec!["directory", "assign", "pa", "pij-second", "--why", "watch"],
        vec![
            "directory",
            "assign",
            "pm-001",
            "pij-holder",
            "--why",
            "lead",
        ],
        vec![
            "directory",
            "release",
            "pm-001",
            "pij-holder",
            "--why",
            "done",
        ],
    ] {
        let done = dir(argv).await;
        assert_eq!(done["ok"], true, "{done}");
    }

    let first = send_to(rig.addr, &repo.main, "role:pm-001", "req-a", json!({})).await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["data"]["request"]["state"], "queued");
    assert_eq!(first["data"]["outcome"]["outcome"], "queued", "{first}");
    assert_eq!(first["data"]["woke"]["seat"], "pij-second", "{first}");
    let held = send_to(
        rig.addr,
        &repo.main,
        "role:pm-001",
        "req-b",
        json!({"fyi": true}),
    )
    .await;
    assert_eq!(
        held["data"]["request"]["state"], "queued",
        "an --fyi send queues too: {held}"
    );
    let third = send_to(rig.addr, &repo.main, "role:pm-001", "req-c", json!({})).await;
    assert_eq!(third["ok"], true, "{third}");
    let queued = |rig: &Rig| {
        let store = rig.store.clone();
        async move {
            store
                .queued_role_requests("fleet", "pm-001", "")
                .await
                .unwrap()
                .into_iter()
                .map(|request| request.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(queued(&rig).await, ["req-a", "req-b", "req-c"]);

    let wakes = inbox_bodies(&rig, "pij-second").await;
    let wake = wakes
        .iter()
        .find(|body| body.contains("request req-a"))
        .unwrap_or_else(|| panic!("the PA was woken: {wakes:?}"));
    assert!(wake.contains("revive the last holder pij-holder"), "{wake}");
    assert!(
        wake.contains("pij directory assign pm-001 <seat> --why \"<why>\" --project fleet"),
        "{wake}"
    );
    assert!(wake.contains("pij never starts a seat itself"), "{wake}");
    assert_eq!(
        wakes
            .iter()
            .filter(|body| body.contains("request req-a"))
            .count(),
        1,
        "woken once per request"
    );

    let refused = dir(vec![
        "directory",
        "assign",
        "pm-001",
        "pij-nobody",
        "--why",
        "x",
    ])
    .await;
    assert_eq!(refused["ok"], false, "{refused}");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        queued(&rig).await,
        ["req-a", "req-b", "req-c"],
        "a refused assign drains nothing"
    );

    let assigned = dir(vec![
        "directory",
        "assign",
        "pm-001",
        "pij-holder",
        "--why",
        "again",
    ])
    .await;
    assert_eq!(assigned["ok"], true, "{assigned}");
    for _ in 0..100 {
        if queued(&rig).await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        queued(&rig).await.is_empty(),
        "the assign drained the queue"
    );
    for id in ["req-a", "req-b", "req-c"] {
        let request = rig.store.role_request(id).await.unwrap().expect("row");
        assert_eq!(request.holder.as_deref(), Some("pij-holder"), "{id}");
    }
    let head = inbox_bodies(&rig, "pij-holder").await;
    assert_eq!(
        head.first().map(String::as_str),
        Some("[pij role:pm-001 request req-a from pij-test-coder · reply wake]\nplease req-a"),
        "the oldest request is first in the holder's queue"
    );
    let events = call(
        rig.addr,
        "/v1/spine",
        &["spine", "events", "--peer", "pij-holder"],
        &repo.main,
    )
    .await;
    let outcomes: Vec<String> = events["data"]["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["kind"] == "delivery.outcome")
        .filter_map(|event| {
            let payload: Value = serde_json::from_str(event["payload"].as_str()?).ok()?;
            Some(payload["msg_id"].as_str()?.to_string())
        })
        .filter(|id| id.starts_with("req-"))
        .collect();
    assert_eq!(
        outcomes,
        ["req-a", "req-c"],
        "in order; the FYI is held, not delivered: {events}"
    );
    rig.server.abort();
}
/// Plan 194 (s03): --from template and file, templates, drift and prompts.
#[path = "templates_http/mod.rs"]
mod templates;

/// Plan 205 (s12), AC1 and AC2: a request past its limit with no card update
/// gets exactly one check-in; a whole further limit with no card update since
/// tells the project's PA exactly once. A card update restarts the clock, so a
/// seat that keeps its card current is never checked in.
#[tokio::test]
async fn a_limit_checks_in_once_then_tells_the_pa_once_and_a_card_update_spares_it() {
    use pij_daemon::delivery::role_send::time_limits;
    let repo = repo("pij-role-limits");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let dir = |argv: Vec<&'static str>| {
        let (addr, main) = (rig.addr, repo.main.clone());
        async move { call(addr, "/v1/directory", &argv, &main).await }
    };
    for argv in [
        vec![
            "directory",
            "add",
            "pa",
            "--title",
            "pa",
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
            "--why",
            "w",
        ],
        vec!["directory", "assign", "pa", "pij-second", "--why", "watch"],
        vec![
            "directory",
            "assign",
            "pm-001",
            "pij-holder",
            "--why",
            "lead",
        ],
    ] {
        let done = dir(argv).await;
        assert_eq!(done["ok"], true, "{done}");
    }
    let limit = 10 * 60_000;
    let sent = send_to(
        rig.addr,
        &repo.main,
        "role:pm-001",
        "req-limit",
        json!({"limit": "10m"}),
    )
    .await;
    assert_eq!(sent["data"]["request"]["limit_ms"], limit, "{sent}");
    let delivered = rig
        .store
        .role_request("req-limit")
        .await
        .unwrap()
        .and_then(|request| request.delivered_at)
        .expect("delivered");
    let kinds = |actions: Vec<pij_daemon::delivery::role_send::LimitAction>| {
        actions
            .into_iter()
            .map(|action| {
                (
                    action.request,
                    action.kind,
                    action.to.map(|seat| seat.to_string()),
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        kinds(time_limits(&rig.services, delivered + limit - 1).await),
        []
    );
    assert_eq!(
        kinds(time_limits(&rig.services, delivered + limit).await),
        [(
            "req-limit".to_string(),
            "check-in",
            Some("pij-holder".to_string())
        )]
    );
    assert_eq!(
        kinds(time_limits(&rig.services, delivered + limit + 1).await),
        [],
        "one check-in"
    );
    let checked = delivered + limit;
    assert_eq!(
        kinds(time_limits(&rig.services, checked + limit - 1).await),
        []
    );
    assert_eq!(
        kinds(time_limits(&rig.services, checked + limit).await),
        [(
            "req-limit".to_string(),
            "notice",
            Some("pij-second".to_string())
        )]
    );
    assert_eq!(
        kinds(time_limits(&rig.services, checked + 3 * limit).await),
        [],
        "one notice"
    );
    // The holder's queue head is the request itself; the check-in shows as its
    // own delivery outcome.
    let events = call(
        rig.addr,
        "/v1/spine",
        &["spine", "events", "--peer", "pij-holder"],
        &repo.main,
    )
    .await;
    let check_ins = events["data"]["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["kind"] == "delivery.outcome")
        .filter(|event| {
            event["payload"]
                .as_str()
                .is_some_and(|payload| payload.contains("role-limit-checkin-req-limit"))
        })
        .count();
    assert_eq!(check_ins, 1, "one check-in reached the holder: {events}");
    let notices = inbox_bodies(&rig, "pij-second").await;
    let notice = notices.first().expect("the PA was told");
    assert!(
        notice.starts_with(
            "[pij time limit] pij-holder has not answered the check-in for request req-limit"
        ) && notice.contains("Nothing has been stopped"),
        "{notice}"
    );

    let sent = send_to(
        rig.addr,
        &repo.main,
        "role:pm-001",
        "req-card",
        json!({"limit": "10m"}),
    )
    .await;
    assert_eq!(sent["ok"], true, "{sent}");
    let delivered = rig
        .store
        .role_request("req-card")
        .await
        .unwrap()
        .and_then(|request| request.delivered_at)
        .expect("delivered");
    for at in [
        delivered + limit / 2,
        delivered + limit,
        delivered + 3 * limit / 2,
    ] {
        rig.services
            .spine
            .append(pij_core::model::Event {
                seq: None,
                v: 1,
                at,
                kind: "report.now".into(),
                seat: Some(SeatId::from("pij-holder")),
                payload: json!({"did": "work", "next": "more"}).to_string(),
            })
            .await
            .expect("card");
        assert_eq!(
            kinds(time_limits(&rig.services, at + limit - 1).await),
            [],
            "each card update restarts the clock"
        );
    }
    let request = rig
        .store
        .role_request("req-card")
        .await
        .unwrap()
        .expect("row");
    assert_eq!(request.checkin_at, None, "never checked in");
    rig.server.abort();
}

/// Plan 205, AC3: without --limit a role send stores the role's
/// default_limit (C5); an explicit --limit wins; with neither there is no
/// limit, and the round never checks the request in.
#[tokio::test]
async fn a_role_default_limit_applies_and_an_explicit_limit_wins() {
    use pij_daemon::delivery::role_send::time_limits;
    let repo = repo("pij-role-default-limit");
    let rig = rig(Arc::new(NoHook)).await;
    with_roles(rig.addr, &repo).await;
    let dir = |argv: Vec<&'static str>| {
        let (addr, main) = (rig.addr, repo.main.clone());
        async move { call(addr, "/v1/directory", &argv, &main).await }
    };
    for argv in [
        vec![
            "directory",
            "add",
            "timed",
            "--title",
            "timed",
            "--kind",
            "general",
            "--lifecycle",
            "long-lived",
            "--holders",
            "one",
            "--default-limit",
            "45m",
            "--rule",
            "r",
            "--packet",
            "p",
            "--why",
            "w",
        ],
        vec!["directory", "assign", "timed", "pij-holder", "--why", "x"],
        vec!["directory", "assign", "pm-001", "pij-second", "--why", "x"],
    ] {
        let done = dir(argv).await;
        assert_eq!(done["ok"], true, "{done}");
    }
    let default = send_to(rig.addr, &repo.main, "role:timed", "req-default", json!({})).await;
    assert_eq!(
        default["data"]["request"]["limit_ms"],
        45 * 60_000,
        "{default}"
    );
    let explicit = send_to(
        rig.addr,
        &repo.main,
        "role:timed",
        "req-explicit",
        json!({"limit": "5m"}),
    )
    .await;
    assert_eq!(
        explicit["data"]["request"]["limit_ms"],
        5 * 60_000,
        "{explicit}"
    );
    let none = send_to(rig.addr, &repo.main, "role:pm-001", "req-none", json!({})).await;
    assert_eq!(none["data"]["request"]["limit_ms"], Value::Null, "{none}");
    let far = rig
        .store
        .role_request("req-none")
        .await
        .unwrap()
        .and_then(|request| request.delivered_at)
        .expect("delivered")
        + 365 * 86_400_000;
    let actions = time_limits(&rig.services, far).await;
    assert!(
        actions.iter().all(|action| action.request != "req-none"),
        "{actions:?}"
    );
    rig.server.abort();
}
