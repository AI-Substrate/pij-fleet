//! `pij directory add` over HTTP against a real store and real scratch git
//! repositories (plan 190, AC3 and AC6). ddocs is faked through the writer's
//! constructor; the private-daemon run in the plan's evidence uses the real one.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, PaneProcess, ProcIdentity, SeatDescriptor, SeatId};
use pij_daemon::community::Refusal;
use pij_daemon::community::ddocs::Validator;
use pij_daemon::community::directory::{AfterAssign, DirectoryService, NoHook};
use pij_daemon::community::layout::rendered;
use pij_daemon::community::templates::Catalog;
use pij_daemon::community::writer::{CommunityWriter, LOCK_RETRY};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use pij_testkit::{FreshStore, fresh_dir};
use serde_json::{Value, json};

const KEY: &str = "directory-test-key";
const SEAT: &str = "pij-test-coder";
const PANE: &str = "%41";
/// The caller's process, live in its pane, so it may send controls.
const CALLER_PROC: ProcIdentity = ProcIdentity {
    pid: 910_041,
    proc_start: 7,
};

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

async fn rig_with(hook: Arc<dyn AfterAssign>, writer: CommunityWriter) -> Rig {
    rig_full(hook, writer, Catalog::shipped()).await
}

async fn rig_full(hook: Arc<dyn AfterAssign>, writer: CommunityWriter, catalog: Catalog) -> Rig {
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
    let tmux = Arc::new(FakeTmux::new().with_pane_process(
        PANE,
        PaneProcess {
            pid: CALLER_PROC.pid,
            cwd: "/tmp".to_string(),
        },
    ));
    services.tmux = tmux.clone();
    services.liveness = Arc::new(FakeLiveness::new().with_proc(CALLER_PROC));
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
        ("pij-dormant", None, Some(SEAT)),
    ] {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/tmp");
        seat.pane = pane.map(str::to_string);
        seat.parent = parent.map(SeatId::from);
        seat.proc = match id {
            SEAT => Some(CALLER_PROC),
            // A process the liveness fake has never seen: observed dead.
            "pij-dormant" => Some(ProcIdentity {
                pid: 4242,
                proc_start: 10,
            }),
            _ => None,
        };
        services.registry.put(seat).await.expect("seed seat");
    }
    let roles = Arc::clone(&services.roles);
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
            // The first role write replaces any hand-written table (plan 202).
            ".harness/government/community/ROUTES.md",
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

/// Every FYI held for `recipient`, as `(sender, body)`, oldest first.
async fn fyis(rig: &Rig, recipient: &str) -> Vec<(String, String)> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", rig._fresh.path()))
        .await
        .unwrap();
    let rows = sqlx::query_as(
        "SELECT sender, body FROM fyis WHERE recipient = ?1 ORDER BY held_at_ms, id",
    )
    .bind(recipient)
    .fetch_all(&pool)
    .await
    .unwrap();
    pool.close().await;
    rows
}

fn routes_md(repo: &Repo) -> String {
    std::fs::read_to_string(repo.gov.join(pij_daemon::community::layout::ROUTES)).unwrap()
}

fn names_at_head(repo: &Repo) -> String {
    git(&repo.gov, &["show", "--name-only", "--format=", "HEAD"])
}

async fn fleet_with_prime(rig: &Rig, repo: &Repo) {
    let main = repo.main.to_str().unwrap();
    let created = call(
        rig.addr,
        "/v1/project",
        &[
            "project",
            "create",
            "fleet",
            "--repo",
            main,
            "--prime",
            "pij-second",
        ],
        &repo.main,
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");
}

async fn add_role(rig: &Rig, repo: &Repo, id: &str, path: &str, rule: &str) -> Value {
    call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "add",
            id,
            "--scope-path",
            path,
            "--rule",
            rule,
            "--packet",
            "You hold it.",
            "--why",
            "routes test",
        ],
        &repo.main,
    )
    .await
}

/// Plan 202 AC1 and AC3: a role add rewrites ROUTES.md (adopting a hand-written
/// one) in its own commit, warns on an overlap and tells the prime once per
/// change; assign and release change no route and tell the prime nothing.
#[tokio::test]
async fn role_changes_regenerate_routes_in_their_commit_and_tell_the_prime_once() {
    let repo = repo("pij-routes-regenerate");
    let routes_path = repo.gov.join(pij_daemon::community::layout::ROUTES);
    std::fs::create_dir_all(routes_path.parent().unwrap()).unwrap();
    std::fs::write(&routes_path, "# Hand-written\n| merges | prime |\n").unwrap();
    git(&repo.gov, &["add", "."]);
    git(&repo.gov, &["commit", "-q", "-m", "hand-written routes"]);
    let rig = rig(Arc::new(NoHook)).await;
    fleet_with_prime(&rig, &repo).await;

    let added = add_role(&rig, &repo, "delivery", "crates/delivery/**", "Owns sends.").await;
    assert_eq!(added["ok"], true, "{added}");
    let names = names_at_head(&repo);
    assert!(names.contains("community/ROUTES.md"), "{names}");
    assert!(names.contains("roles/delivery/role.dd.json"), "{names}");
    let table = routes_md(&repo);
    assert!(!table.contains("Hand-written"), "{table}");
    assert!(
        table.contains("| `crates/delivery/**` | role:delivery |\n| Owns sends. | role:delivery |"),
        "{table}"
    );
    let routes = &added["data"]["routes"];
    assert_eq!(routes["regenerated"], true, "{routes}");
    assert_eq!(routes["added"].as_array().unwrap().len(), 2, "{routes}");
    assert_eq!(routes["prime"]["sent"], "fyi", "{routes}");
    assert!(added["data"].get("warnings").is_none(), "{added}");
    let prime = fyis(&rig, "pij-second").await;
    assert_eq!(prime.len(), 1, "{prime:?}");
    assert_eq!(prime[0].0, "pij-bg");
    assert!(
        prime[0].1.starts_with("Routes changed in fleet")
            && prime[0]
                .1
                .contains("+ `crates/delivery/**` -> role:delivery"),
        "{}",
        prime[0].1
    );

    let overlapping = add_role(
        &rig,
        &repo,
        "storage",
        "crates/delivery/queue.rs",
        "Owns the queue.",
    )
    .await;
    assert_eq!(overlapping["ok"], true, "{overlapping}");
    assert_eq!(
        overlapping["data"]["warnings"],
        json!([
            "W-ROUTES-OVERLAP: crates/delivery/** (role:delivery) overlaps crates/delivery/queue.rs (role:storage)"
        ])
    );
    assert!(routes_md(&repo).contains("## Overlaps"));
    assert_eq!(fyis(&rig, "pij-second").await.len(), 2);

    // A change of holder: no route changes and the prime hears nothing.
    let before = routes_md(&repo);
    for leaf in ["assign", "release"] {
        let held = call(
            rig.addr,
            "/v1/directory",
            &[
                "directory",
                leaf,
                "delivery",
                "pij-holder",
                "--why",
                "rotate",
            ],
            &repo.main,
        )
        .await;
        assert_eq!(held["ok"], true, "{held}");
        assert!(!names_at_head(&repo).contains("ROUTES.md"));
    }
    assert_eq!(routes_md(&repo), before);
    assert_eq!(fyis(&rig, "pij-second").await.len(), 2);
    rig.server.abort();
}

/// Plan 202 AC2 (on demand): `directory routes` prints the table with live
/// holders and says whether ROUTES.md is stale; `--write` rewrites a stale one
/// in one commit and writes nothing when it is current.
#[tokio::test]
async fn routes_on_demand_and_write_only_when_stale() {
    let repo = repo("pij-routes-demand");
    let rig = rig(Arc::new(NoHook)).await;
    fleet_with_prime(&rig, &repo).await;
    add_role(&rig, &repo, "delivery", "crates/delivery/**", "Owns sends.").await;
    call(
        rig.addr,
        "/v1/directory",
        &[
            "directory",
            "assign",
            "delivery",
            "pij-holder",
            "--why",
            "own it",
        ],
        &repo.main,
    )
    .await;

    let shown = call(
        rig.addr,
        "/v1/directory",
        &["directory", "routes"],
        &repo.main,
    )
    .await;
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["data"]["stale"], false);
    let text = shown["data"]["text"].as_str().unwrap();
    assert!(
        text.contains("| `crates/delivery/**` | role:delivery | pij-holder |"),
        "{text}"
    );

    let head_before = head(&repo);
    let current = call(
        rig.addr,
        "/v1/directory",
        &["directory", "routes", "--write", "--why", "check"],
        &repo.main,
    )
    .await;
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(current["data"]["written"]["regenerated"], false);
    assert_eq!(head(&repo), head_before);

    std::fs::write(
        repo.gov.join(pij_daemon::community::layout::ROUTES),
        "edited by hand\n",
    )
    .unwrap();
    git(&repo.gov, &["commit", "-q", "-am", "a hand edit"]);
    let stale = call(
        rig.addr,
        "/v1/directory",
        &["directory", "routes"],
        &repo.main,
    )
    .await;
    assert_eq!(stale["data"]["stale"], true, "{stale}");
    let written = call(
        rig.addr,
        "/v1/directory",
        &["directory", "routes", "--write", "--why", "adopt"],
        &repo.main,
    )
    .await;
    assert_eq!(written["ok"], true, "{written}");
    assert_eq!(written["data"]["commit"], head(&repo));
    assert_eq!(
        names_at_head(&repo).trim(),
        ".harness/government/community/ROUTES.md"
    );
    assert_eq!(written["data"]["stale"], false);
    // No row changed, so the prime hears only of the one role add.
    assert_eq!(fyis(&rig, "pij-second").await.len(), 1);

    let refused = call(
        rig.addr,
        "/v1/directory",
        &["directory", "routes", "--write"],
        &repo.main,
    )
    .await;
    assert_eq!(refused["details"]["code"], "E-RS-ARG", "{refused}");
    rig.server.abort();
}

/// Plan 202 AC2: assign, a compact control and a revive each hold the table
/// once for a role holder; a seat holding no role gets nothing.
#[tokio::test]
async fn assign_compact_and_revive_each_send_a_holder_the_table_once() {
    let repo = repo("pij-routes-triggers");
    let rig = rig(Arc::new(NoHook)).await;
    fleet_with_prime(&rig, &repo).await;
    add_role(&rig, &repo, "delivery", "crates/delivery/**", "Owns sends.").await;
    for seat in ["pij-holder", "pij-dormant"] {
        let assigned = call(
            rig.addr,
            "/v1/directory",
            &["directory", "assign", "delivery", seat, "--why", "own it"],
            &repo.main,
        )
        .await;
        assert_eq!(assigned["data"]["routes"]["sent"], "fyi", "{assigned}");
        assert_eq!(assigned["data"]["routes"]["projects"], json!(["fleet"]));
    }
    let held = fyis(&rig, "pij-holder").await;
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].0, "pij-bg");
    assert!(
        held[0].1.starts_with("Routing table for fleet."),
        "{}",
        held[0].1
    );
    assert!(
        held[0]
            .1
            .contains("| Owns sends. | role:delivery | pij-holder |"),
        "{}",
        held[0].1
    );

    let compact = call(
        rig.addr,
        "/v1/shim/send",
        &["send", "pij-holder", "--command", "compact"],
        &repo.main,
    )
    .await;
    assert_eq!(compact["ok"], true, "{compact}");
    assert_eq!(fyis(&rig, "pij-holder").await.len(), 2);
    let own = call(rig.addr, "/v1/shim/send", &["compact-self"], &repo.main).await;
    assert_eq!(own["ok"], true, "{own}");
    assert!(fyis(&rig, SEAT).await.is_empty(), "holds no role");

    let revived: Value = reqwest::Client::new()
        .post(format!("http://{}/v1/revive", rig.addr))
        .bearer_auth(KEY)
        .json(&json!({"id": "pij-dormant", "session": "fleet", "fresh": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(revived["ok"], true, "{revived}");
    let dormant = fyis(&rig, "pij-dormant").await;
    assert_eq!(dormant.len(), 2, "assign and revive: {dormant:?}");
    assert!(dormant[1].1.starts_with("Routing table for fleet."));
    rig.server.abort();
}

/// Plan 194 (s03): --from template and file, templates, drift and prompts.
#[path = "templates_http/mod.rs"]
mod templates;
