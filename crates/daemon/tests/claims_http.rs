//! `pij claim`, `pij claims` and `pij delivered` over HTTP against a real
//! store, a scratch main checkout and a fake gh (plan 196, bp-0001, bp-0003,
//! bp-0004). Nothing here reaches GitHub or the prod daemon.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId, Seq};
use pij_core::ports::Spine;
use pij_daemon::community::Refusal;
use pij_daemon::community::gh::{PullRequest, PullRequests};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::{FreshStore, fresh_dir};
use serde_json::{Value, json};

const KEY: &str = "claims-test-key";
const CODER: (&str, &str) = ("pij-test-coder", "%41");
const OTHER: (&str, &str) = ("pij-test-other", "%42");
const PM: (&str, &str) = ("pij-test-pm", "%43");
const HOLDER: (&str, &str) = ("pij-test-holder", "%44");

const PRD: &str = "docs/project/prds/001-demo.dd.json";
const ROADMAP: &str = "docs/project/roadmaps/001-demo.dd.json";

/// Answers from a fixed set of PRs and counts its calls.
#[derive(Default)]
struct FakeGh {
    prs: HashMap<u64, PullRequest>,
    calls: AtomicUsize,
}

impl PullRequests for FakeGh {
    fn view(&self, _: &Path, number: u64) -> Result<PullRequest, Refusal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prs.get(&number).cloned().ok_or_else(|| Refusal {
            code: pij_daemon::community::gh::E_GH,
            message: format!("no PR {number}"),
            next: None,
        })
    }
}

fn pr(number: u64, state: &str, body: &str, files: &[&str]) -> (u64, PullRequest) {
    (
        number,
        PullRequest {
            number,
            state: state.into(),
            merge_commit: (state == "MERGED").then(|| format!("{number:0>40}")),
            body: body.into(),
            files: files.iter().map(|file| (*file).to_string()).collect(),
            url: Some(format!("https://example.invalid/pr/{number}")),
        },
    )
}

struct Rig {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    _fresh: FreshStore,
    store: pij_store::SqliteOrchestration,
    spine: Arc<dyn Spine>,
    gh: Arc<FakeGh>,
    main: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(self.main.parent().unwrap());
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

fn document(sections: Value) -> String {
    serde_json::to_string_pretty(
        &json!({"dd": {"schema": "x"}, "sections": sections, "references": []}),
    )
    .unwrap()
}

/// A main checkout holding numbered folders, a PRD and a roadmap, committed.
fn checkout(name: &str) -> PathBuf {
    let main = fresh_dir(name).canonicalize().unwrap().join("main");
    for dir in [
        "docs/plans/190-a",
        "docs/plans/archive/195-b",
        "docs/project/prds",
        "docs/project/roadmaps",
        "crates/store/migrations",
    ] {
        std::fs::create_dir_all(main.join(dir)).unwrap();
        std::fs::write(main.join(dir).join(".keep"), "").unwrap();
    }
    std::fs::write(main.join("crates/store/migrations/0032_x.sql"), "").unwrap();
    std::fs::write(
        main.join(PRD),
        document(json!([{"name": "requirements", "value": [
            {"id": "req-20", "delivered_by": "#110"},
            {"id": "req-21"}
        ]}])),
    )
    .unwrap();
    std::fs::write(
        main.join(ROADMAP),
        document(json!([{"name": "steps", "value": [{"id": "s11"}]}])),
    )
    .unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "scratch"),
        ("user.email", "scratch@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(&main, &["config", key, value]);
    }
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "main"]);
    main
}

async fn rig(name: &str, prs: Vec<(u64, PullRequest)>) -> Rig {
    let main = checkout(name);
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
    let gh = Arc::new(FakeGh {
        prs: prs.into_iter().collect(),
        ..FakeGh::default()
    });
    services.gh = gh.clone();
    for (id, pane) in [CODER, OTHER, PM, HOLDER] {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/tmp");
        seat.pane = Some(pane.to_string());
        services.registry.put(seat).await.expect("seed seat");
    }
    let store = services.directory.store().clone();
    let spine = Arc::clone(&services.spine);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = router_with_config(services, HttpConfig::local(KEY.to_string()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let rig = Rig {
        addr,
        server,
        _fresh: fresh,
        store,
        spine,
        gh,
        main,
    };
    let created = rig
        .call(
            CODER,
            "/v1/project",
            &[
                "project",
                "create",
                "demo",
                "--repo",
                rig.main.to_str().unwrap(),
            ],
        )
        .await;
    assert_eq!(created["ok"], true, "{created}");
    rig
}

impl Rig {
    async fn call(&self, (_, pane): (&str, &str), path: &str, argv: &[&str]) -> Value {
        reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth(KEY)
            .json(&json!({"argv": argv, "caller": {"TMUX_PANE": pane, "cwd": self.main}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn card(&self, seat: &str) -> Value {
        reqwest::Client::new()
            .post(format!("http://{}/v1/state", self.addr))
            .bearer_auth(KEY)
            .json(&json!({"id": seat}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn delivered_events(&self) -> Vec<pij_core::model::Event> {
        self.spine
            .tail(None, Seq(0))
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "delivered")
            .collect()
    }
}

#[tokio::test]
async fn claims_take_numbers_after_the_checkout_list_and_release_by_owner() {
    let rig = rig("pij-claims-http", Vec::new()).await;
    let plan = rig
        .call(CODER, "/v1/claim", &["claim", "plan", "--for", "claims"])
        .await;
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["data"]["number"], 196, "one past archive/195-b");
    assert_eq!(plan["data"]["formatted"], "196");
    assert_eq!(plan["data"]["seat"], CODER.0);
    assert_eq!(plan["data"]["project"], "demo");
    let migration = rig
        .call(
            OTHER,
            "/v1/claim",
            &["claim", "migration", "--for", "m", "--project", "demo"],
        )
        .await;
    assert_eq!(migration["data"]["formatted"], "0033", "{migration}");

    let listed = rig.call(CODER, "/v1/claims", &["claims"]).await;
    assert_eq!(listed["data"]["open"], 2, "{listed}");
    assert_eq!(listed["data"]["claims"][0]["kind"], "migration");
    assert_eq!(listed["data"]["claims"][1]["seat"], CODER.0);

    let refused = rig
        .call(OTHER, "/v1/claim", &["claim", "release", "plan", "196"])
        .await;
    assert_eq!(refused["details"]["code"], "E-RS-OWNERSHIP", "{refused}");
    let released = rig
        .call(
            CODER,
            "/v1/claim",
            &["claim", "release", "plan", "196", "--why", "dropped"],
        )
        .await;
    assert_eq!(released["ok"], true, "{released}");
    assert_eq!(released["data"]["release_reason"], "dropped");
    let again = rig
        .call(CODER, "/v1/claim", &["claim", "release", "plan", "196"])
        .await;
    assert_eq!(again["details"]["code"], "E-RS-CLAIM-RELEASED", "{again}");
    let next = rig
        .call(CODER, "/v1/claim", &["claim", "plan", "--for", "next"])
        .await;
    assert_eq!(
        next["data"]["number"], 197,
        "a released number stays burned"
    );
    let open = rig.call(CODER, "/v1/claims", &["claims", "--open"]).await;
    assert_eq!(
        open["data"]["claims"].as_array().unwrap().len(),
        2,
        "{open}"
    );

    for (argv, code) in [
        (vec!["claim", "Plan", "--for", "x"], "E-RS-ARG"),
        (vec!["claim", "release", "--for", "x"], "E-RS-ARG"),
        (vec!["claim", "plan"], "E-RS-ARG"),
        (vec!["claim", "plan", "--for", "x", "--pm", "y"], "E-RS-ARG"),
        (
            vec!["claim", "release", "plan", "999"],
            "E-RS-CLAIM-UNKNOWN",
        ),
    ] {
        let refused = rig.call(CODER, "/v1/claim", &argv).await;
        assert_eq!(refused["details"]["code"], code, "{argv:?}: {refused}");
    }
    let help = rig.call(CODER, "/v1/claims", &["claims", "--help"]).await;
    assert!(
        help["data"]["help"]
            .as_str()
            .unwrap()
            .contains("pij delivered <pr>")
    );
    assert_eq!(
        rig.gh.calls.load(Ordering::SeqCst),
        0,
        "claims never read gh"
    );
}

fn body() -> String {
    format!(
        "## What changed\n\nDelivers: {PRD}#requirements/req-20, {PRD}#requirements/req-21, {ROADMAP}#steps/s11\n"
    )
}

const FILES: [&str; 3] = [
    "docs/plans/196-claims-delivery/plan.dd.json",
    "crates/store/migrations/0033_claims.sql",
    "crates/store/src/claims.rs",
];

#[tokio::test]
async fn delivered_records_once_prints_edits_and_posts_on_the_pm_card() {
    let rig = rig(
        "pij-delivered-http",
        vec![
            pr(130, "MERGED", &body(), &FILES),
            pr(131, "OPEN", &body(), &FILES),
            pr(132, "MERGED", "no line here", &FILES),
            pr(133, "MERGED", "Delivers: req-20", &FILES),
        ],
    )
    .await;
    let migration = rig
        .call(
            CODER,
            "/v1/claim",
            &["claim", "migration", "--for", "claims"],
        )
        .await;
    assert_eq!(migration["data"]["formatted"], "0033", "{migration}");

    let first = rig
        .call(OTHER, "/v1/delivered", &["delivered", "#130", "--pm", PM.0])
        .await;
    assert_eq!(first["ok"], true, "{first}");
    let data = &first["data"];
    assert_eq!(data["recorded"].as_array().unwrap().len(), 3);
    assert_eq!(data["pm"], PM.0);
    assert_eq!(data["pm_source"], "flag");
    assert_eq!(data["released"][0]["kind"], "migration");
    assert_eq!(data["released"][0]["release_reason"], "delivered #130");
    let commands: Vec<&str> = data["edits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edit| edit["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        commands,
        [
            format!("ddocs set '{PRD}#requirements/req-20/delivered_by' '#110, #130'"),
            format!("ddocs set '{PRD}#requirements/req-21/delivered_by' '#130'"),
            format!("ddocs set '{ROADMAP}#steps/s11/plan' 'docs/plans/196-claims-delivery'"),
        ]
    );

    let card = rig.card(PM.0).await;
    let deliveries = card["data"]["deliveries"].as_array().unwrap();
    assert_eq!(deliveries.len(), 1, "{card}");
    assert_eq!(deliveries[0]["pr"], 130);
    assert_eq!(deliveries[0]["recorded_by"], OTHER.0);
    assert_eq!(deliveries[0]["addresses"].as_array().unwrap().len(), 3);
    assert_eq!(rig.card(OTHER.0).await["data"]["deliveries"], json!([]));
    let events = rig.delivered_events().await;
    assert_eq!(events.len(), 1, "one event");
    assert_eq!(events[0].seat, Some(SeatId::from(PM.0)));

    let again = rig
        .call(OTHER, "/v1/delivered", &["delivered", "130", "--pm", PM.0])
        .await;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["data"]["recorded"], json!([]), "idempotent");
    assert_eq!(again["data"]["already"].as_array().unwrap().len(), 3);
    assert_eq!(again["data"]["event_seq"], Value::Null);
    assert_eq!(rig.delivered_events().await.len(), 1, "no second event");

    let open = rig
        .call(OTHER, "/v1/delivered", &["delivered", "131", "--pm", PM.0])
        .await;
    assert_eq!(open["details"]["code"], "E-RS-PR-NOT-MERGED", "{open}");
    for number in ["132", "133"] {
        let missing = rig
            .call(OTHER, "/v1/delivered", &["delivered", number])
            .await;
        assert_eq!(missing["details"]["code"], "E-RS-DELIVERS", "{missing}");
    }
    let nobody = rig
        .call(
            OTHER,
            "/v1/delivered",
            &["delivered", "130", "--pm", "pij-nobody"],
        )
        .await;
    assert_eq!(nobody["details"]["code"], "E-RS-NOT-FOUND", "{nobody}");

    assert_eq!(
        rig.gh.calls.load(Ordering::SeqCst),
        5,
        "one gh read per call that got that far"
    );
    assert_eq!(
        git(&rig.main, &["status", "--porcelain"]),
        "",
        "no project file changed"
    );
}

#[tokio::test]
async fn delivered_posts_on_the_live_pm_holder_and_without_one_says_nobody_was_told() {
    let rig = rig(
        "pij-delivered-holder",
        vec![
            pr(140, "MERGED", &body(), &FILES),
            pr(
                141,
                "MERGED",
                &format!("Delivers: {PRD}#requirements/req-21"),
                &FILES,
            ),
        ],
    )
    .await;
    rig.store
        .record_assignment_committed(
            &pij_store::Assignment {
                project: "demo".into(),
                role: "pm-001".into(),
                instance: String::new(),
                seat: HOLDER.0.into(),
                harness: Some("omp".into()),
                session: None,
                machine: None,
                from: "2026-11-06T00:00:00Z".into(),
                to: None,
                assigned_by: CODER.0.into(),
                released_by: None,
                why: "test".into(),
                release_why: None,
                end_pending: false,
            },
            Some("pm"),
            1,
        )
        .await
        .unwrap();

    let held = rig
        .call(OTHER, "/v1/delivered", &["delivered", "140", "--pm", PM.0])
        .await;
    assert_eq!(held["ok"], true, "{held}");
    assert_eq!(held["data"]["pm"], HOLDER.0);
    assert_eq!(held["data"]["pm_source"], "holder");
    assert!(
        held["data"]["notes"][0].as_str().unwrap().contains("--pm"),
        "{held}"
    );
    assert_eq!(rig.card(HOLDER.0).await["data"]["deliveries"][0]["pr"], 140);

    // req-21 alone names no roadmap step, so no holder applies and no --pm was given.
    let untold = rig
        .call(OTHER, "/v1/delivered", &["delivered", "141"])
        .await;
    assert_eq!(untold["ok"], true, "{untold}");
    assert_eq!(untold["data"]["pm"], Value::Null);
    assert!(
        untold["data"]["line"]
            .as_str()
            .unwrap()
            .contains("no PM was told"),
        "{untold}"
    );
}
