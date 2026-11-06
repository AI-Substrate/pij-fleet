//! `pij output` and `pij outputs` over HTTP (plan 200, AC1-AC4) against a real
//! store, real scratch git repositories and fake ddocs.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::delivery::delivery_kind;
use pij_core::model::{Harness, Msg, SeatDescriptor, SeatId};
use pij_core::ports::Queue;
use pij_daemon::community::Refusal;
use pij_daemon::community::ddocs::Validator;
use pij_daemon::community::directory::{DirectoryService, NoHook};
use pij_daemon::community::layout::rendered;
use pij_daemon::community::writer::{CommunityWriter, LOCK_RETRY};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_store::{Assignment, ReplyMode, RequestState, RoleRequest, SqliteOrchestration};
use pij_testkit::fakes::FakeTmux;
use pij_testkit::{FreshStore, fresh_dir};
use serde_json::{Value, json};

const KEY: &str = "output-test-key";
const WRITER: &str = "pij-writer";
const PANE: &str = "%61";

struct Validates;

impl Validator for Validates {
    fn validate(&self, _: &Path, _: &Path) -> Result<(), Refusal> {
        Ok(())
    }

    fn build(&self, _: &Path, document: &Path) -> Result<(), Refusal> {
        std::fs::write(rendered(document), "rendered\n").unwrap();
        Ok(())
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

struct Rig {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    _fresh: FreshStore,
    root: PathBuf,
    main: PathBuf,
    gov: PathBuf,
    store: SqliteOrchestration,
    queue: Arc<dyn Queue>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A governance repo with role `reviewer`; seats `pij-writer` (the caller,
/// parent `pij-pm`), `pij-asker` and `pij-pm`; `pij-holder` holds reviewer.
async fn rig(name: &str) -> Rig {
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
    let role = gov.join(".harness/government/community/roles/reviewer");
    std::fs::create_dir_all(&role).unwrap();
    std::fs::write(role.join("role.dd.json"), "{}\n").unwrap();
    git(&gov, &["add", "."]);
    git(&gov, &["commit", "-q", "-m", "government"]);

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
    let writer = Arc::new(CommunityWriter::new(
        Arc::new(Validates),
        vec![],
        &LOCK_RETRY,
    ));
    let store = services.directory.store().clone();
    services.community = Arc::clone(&writer);
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    services.directory = Arc::new(DirectoryService::new(
        writer,
        store.clone(),
        Arc::clone(&services.event_bus),
        Arc::new(NoHook),
        tmux,
    ));
    for (id, pane, parent) in [
        (WRITER, PANE, Some("pij-pm")),
        ("pij-asker", "%62", None),
        ("pij-pm", "%63", None),
        ("pij-holder", "%64", None),
    ] {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/tmp");
        seat.pane = Some(pane.to_string());
        seat.parent = parent.map(SeatId::from);
        services.registry.put(seat).await.expect("seat");
    }
    store
        .replace_assignments(
            "fleet",
            &[Assignment {
                project: "fleet".into(),
                role: "reviewer".into(),
                instance: String::new(),
                seat: "pij-holder".into(),
                harness: Some("omp".into()),
                session: None,
                machine: None,
                from: "2026-11-06T00:00:00.000Z".into(),
                to: None,
                assigned_by: "pij-pm".into(),
                released_by: None,
                why: "review".into(),
                release_why: None,
                end_pending: false,
            }],
        )
        .await
        .expect("holding");
    let queue = services.queue.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = router_with_config(services, HttpConfig::local(KEY.to_string()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let rig = Rig {
        addr,
        server,
        _fresh: fresh,
        root,
        main,
        gov,
        store,
        queue,
    };
    let created = call(
        &rig,
        "/v1/project",
        &[
            "project",
            "create",
            "fleet",
            "--repo",
            rig.main.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(created["ok"], true, "{created}");
    rig
}

async fn call(rig: &Rig, path: &str, argv: &[&str]) -> Value {
    reqwest::Client::new()
        .post(format!("http://{}{path}", rig.addr))
        .bearer_auth(KEY)
        .json(&json!({"argv": argv, "caller": {"TMUX_PANE": PANE, "cwd": rig.main}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn request(rig: &Rig, id: &str, reply: ReplyMode) {
    rig.store
        .record_role_request(&RoleRequest {
            id: id.into(),
            project: "fleet".into(),
            role: "reviewer".into(),
            instance: String::new(),
            requester: "pij-asker".into(),
            body: "review #200".into(),
            in_reply_to: None,
            fyi: false,
            reply,
            limit_ms: None,
            state: RequestState::Delivered,
            holder: Some("pij-holder".into()),
            created_at: 1,
            delivered_at: Some(1),
            checkin_at: None,
            escalated_at: None,
            answered_at: None,
        })
        .await
        .unwrap();
}

fn verdict<'a>(head: &'a str, result: &'a str, answers: Option<&'a str>) -> Vec<&'a str> {
    let mut argv = vec![
        "output",
        "verdict",
        "--role",
        "reviewer",
        "--subject",
        "pr:200",
        "--head",
        head,
        "--result",
        result,
        "--note-owed",
        "tidy",
        "--evidence",
        r#"{"command":"just rust-check","exit_code":0}"#,
        "--body",
        "checked",
    ];
    if let Some(id) = answers {
        argv.extend(["--answers", id]);
    }
    argv
}

/// Whether `seat`'s queue holds a message from the writer.
async fn told(rig: &Rig, seat: &str) -> bool {
    rig.queue
        .peek(&[delivery_kind(&seat.into())])
        .await
        .unwrap()
        .is_some_and(|(_, job)| {
            serde_json::from_str::<Msg>(&job.payload)
                .is_ok_and(|msg| msg.from.as_str() == WRITER && msg.body.contains("[pij output]"))
        })
}

async fn fyis(rig: &Rig, seat: &str) -> u64 {
    rig.queue.pending_fyi_count(&seat.into()).await.unwrap()
}

#[tokio::test]
async fn an_output_lands_in_one_commit_flags_a_non_holder_and_answers_only_its_head() {
    let rig = rig("pij-output-write").await;
    let before = git(&rig.gov, &["rev-parse", "HEAD"]);
    let written = call(&rig, "/v1/output", &verdict("aaaaaaa1", "pass", None)).await;
    assert_eq!(written["ok"], true, "{written}");
    let data = &written["data"];
    assert_eq!(data["writer_holds_role"], false, "{written}");
    assert!(data["warning"].as_str().unwrap().contains("does not hold"));
    let path = data["path"].as_str().unwrap();
    assert!(path.ends_with("-verdict-pr-200-aaaaaaa.dd.json"), "{path}");
    let document: Value =
        serde_json::from_str(&std::fs::read_to_string(rig.gov.join(path)).unwrap()).unwrap();
    assert_eq!(document["sections"][0]["value"]["head"], "aaaaaaa1");
    assert_eq!(document["sections"][2]["value"][0]["exit_code"], 0);
    assert_eq!(
        git(
            &rig.gov,
            &["rev-list", "--count", &format!("{}..HEAD", before.trim())]
        )
        .trim(),
        "1"
    );

    let same = call(
        &rig,
        "/v1/outputs",
        &["outputs", "--subject", "pr:200", "--head", "aaaaaaa1"],
    )
    .await;
    assert_eq!(
        same["data"]["outputs"].as_array().unwrap().len(),
        1,
        "{same}"
    );
    let moved = call(
        &rig,
        "/v1/outputs",
        &["outputs", "--subject", "pr:200", "--head", "bbbbbbb2"],
    )
    .await;
    assert!(
        moved["data"]["outputs"].as_array().unwrap().is_empty(),
        "{moved}"
    );

    let handover = call(
        &rig,
        "/v1/output",
        &[
            "output",
            "handover",
            "--role",
            "reviewer",
            "--subject",
            "seat",
            "--body",
            "PR 200 half reviewed",
        ],
    )
    .await;
    assert_eq!(handover["ok"], true, "{handover}");
    let listed = call(&rig, "/v1/outputs", &["outputs", "--role", "reviewer"]).await;
    assert_eq!(listed["data"]["outputs"][0]["kind"], "handover", "{listed}");

    for (argv, code) in [
        (
            &[
                "output",
                "verdict",
                "--role",
                "reviewer",
                "--subject",
                "pr:1",
                "--result",
                "pass",
            ][..],
            "E-RS-ARG",
        ),
        (
            &[
                "output",
                "note",
                "--role",
                "ghost",
                "--subject",
                "x",
                "--body",
                "b",
            ][..],
            "E-RS-DIRECTORY-ROLE-MISSING",
        ),
        (
            &[
                "output",
                "note",
                "--role",
                "reviewer",
                "--subject",
                "x",
                "--answers",
                "nope",
            ][..],
            "E-RS-NOT-FOUND",
        ),
    ] {
        let refused = call(&rig, "/v1/output", argv).await;
        assert_eq!(refused["details"]["code"], code, "{argv:?}: {refused}");
    }
}

#[tokio::test]
async fn two_outputs_at_once_land_as_two_files() {
    let rig = rig("pij-output-concurrent").await;
    let argv = verdict("ccccccc3", "pass", None);
    let (a, b) = tokio::join!(
        call(&rig, "/v1/output", &argv),
        call(&rig, "/v1/output", &argv),
    );
    assert_eq!(
        (a["ok"].clone(), b["ok"].clone()),
        (json!(true), json!(true)),
        "{a} {b}"
    );
    assert_ne!(a["data"]["path"], b["data"]["path"]);
    assert_eq!(git(&rig.gov, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn the_requester_hears_back_in_its_reply_mode_and_a_block_always_wakes() {
    let rig = rig("pij-output-notify").await;
    request(&rig, "req-wake", ReplyMode::Wake).await;
    let woke = call(
        &rig,
        "/v1/output",
        &verdict("ddddddd4", "pass", Some("req-wake")),
    )
    .await;
    assert_eq!(woke["data"]["notified"]["seat"], "pij-asker", "{woke}");
    assert_eq!(woke["data"]["notified"]["mode"], "wake");
    assert!(
        told(&rig, "pij-asker").await,
        "a default-mode requester is woken"
    );
    assert!(
        rig.store
            .role_request("req-wake")
            .await
            .unwrap()
            .unwrap()
            .answered_at
            .is_some(),
        "the request is marked answered"
    );

    request(&rig, "req-fyi", ReplyMode::Fyi).await;
    let held = call(
        &rig,
        "/v1/output",
        &verdict("eeeeeee5", "pass", Some("req-fyi")),
    )
    .await;
    assert_eq!(held["data"]["notified"]["mode"], "fyi", "{held}");
    assert_eq!(
        fyis(&rig, "pij-asker").await,
        1,
        "an fyi requester gets an FYI"
    );

    request(&rig, "req-fyi-block", ReplyMode::Fyi).await;
    let block = call(
        &rig,
        "/v1/output",
        &verdict("fffffff6", "block", Some("req-fyi-block")),
    )
    .await;
    assert_eq!(
        block["data"]["notified"]["mode"], "wake",
        "a block always wakes: {block}"
    );

    request(&rig, "req-none", ReplyMode::None).await;
    let quiet = call(
        &rig,
        "/v1/output",
        &verdict("1111111a", "pass", Some("req-none")),
    )
    .await;
    assert_eq!(quiet["data"]["notified"]["outcome"], "not told", "{quiet}");
}

#[tokio::test]
async fn an_unrequested_output_goes_as_an_fyi_to_the_seat_above() {
    let rig = rig("pij-output-unrequested").await;
    let note = call(
        &rig,
        "/v1/output",
        &[
            "output",
            "note",
            "--role",
            "reviewer",
            "--subject",
            "pr:200",
            "--body",
            "fyi",
        ],
    )
    .await;
    assert_eq!(note["data"]["notified"]["seat"], "pij-pm", "{note}");
    assert_eq!(note["data"]["notified"]["mode"], "fyi");
    assert_eq!(fyis(&rig, "pij-pm").await, 1);
    assert_eq!(fyis(&rig, "pij-asker").await, 0, "one seat is told");
}
