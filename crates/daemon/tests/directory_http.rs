//! `pij directory add` over HTTP against a real store and real scratch git
//! repositories (plan 190, AC3 and AC6). ddocs is faked through the writer's
//! constructor; the private-daemon run in the plan's evidence uses the real one.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor};
use pij_daemon::community::Refusal;
use pij_daemon::community::ddocs::Validator;
use pij_daemon::community::layout::rendered;
use pij_daemon::community::writer::{CommunityWriter, LOCK_RETRY};
use pij_daemon::http::{HttpConfig, router_with_config};
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

async fn daemon() -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore) {
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
    services.community = Arc::new(CommunityWriter::new(
        Arc::new(FakeDdocs),
        vec![],
        &LOCK_RETRY,
    ));
    let mut seat = SeatDescriptor::new(SEAT, Harness::Omp, "/tmp");
    seat.pane = Some(PANE.to_string());
    services.registry.put(seat).await.expect("seed seat");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = router_with_config(services, HttpConfig::local(KEY.to_string()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (addr, server, fresh)
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
