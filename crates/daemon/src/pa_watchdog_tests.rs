use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId, SemanticState, SystemState};
use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeSessionStatus;

use super::{PaWatchdog, RoundOutcome};

const INTERVAL: u64 = 1_200;

/// A git repository with a sibling worktree, plus an unrelated folder.
struct Repos {
    root: PathBuf,
}

impl Repos {
    fn new() -> Self {
        // A counter, not the clock: macOS reads time at microsecond resolution,
        // so two tests starting together named the same directory and raced
        // inside one `git init`.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "pij-pa-watchdog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("main")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs");
            assert!(status.status.success(), "git {args:?}: {status:?}");
        };
        let main = root.join("main");
        git(&main, &["init", "-q"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git(
            &main,
            &["worktree", "add", "-q", "../feature", "-b", "feature"],
        );
        Self { root }
    }

    fn path(&self, name: &str) -> String {
        std::fs::canonicalize(self.root.join(name))
            .unwrap()
            .display()
            .to_string()
    }
}

impl Drop for Repos {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn services() -> (crate::Services, FreshStore) {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };
    let services =
        crate::build_services(&config, &Path::new(&store.path()).with_extension("signals"))
            .await
            .expect("real store services");
    (services, store)
}

fn seat(id: &str, folder: &str, parent: Option<&str>) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(id, Harness::Claude, folder);
    seat.parent = parent.map(SeatId::from);
    seat.harness_session = Some(format!("session-{id}"));
    seat
}

fn sized(context: u64, now_ms: u64) -> SessionStatusReply {
    SessionStatusReply::Status(SeatStatus {
        model: Fact::native("claude-opus-5-5".to_string()),
        context_used_tokens: Fact::derived(context),
        last_call_at_ms: Fact::native(now_ms),
        ..SeatStatus::unknown()
    })
}

fn now_ms() -> u64 {
    crate::http::system_time_ms().unwrap()
}

#[tokio::test]
async fn only_pas_are_nudged_with_their_whole_repository_and_quiet_rounds_send_nothing() {
    let repos = Repos::new();
    let (mut services, _store) = services().await;
    let start = now_ms();
    services.session_status = Arc::new(
        FakeSessionStatus::new()
            .with_reply("session-pij-prime", sized(400_000, start))
            .with_reply("session-pij-pa", sized(90_000, start))
            .with_reply("session-pij-coder", sized(650_000, start))
            .with_reply("session-pij-outsider", sized(10_000, start)),
    );
    let main = repos.path("main");
    let prime = seat("pij-prime", &main, None);
    let pa = seat("pij-pa", &main, Some("pij-prime"));
    let mut coder = seat("pij-coder", &repos.path("feature"), None);
    coder.semantic_state = Some(SemanticState::Question);
    let outsider = seat("pij-outsider", &repos.path("elsewhere"), Some("pij-prime"));
    for descriptor in [prime, pa, coder.clone(), outsider] {
        services.registry.put(descriptor).await.unwrap();
    }
    let watchdog = PaWatchdog::default();

    // No PA yet: a prime and its other children are never watchdog targets.
    assert!(
        watchdog
            .round(&services, INTERVAL, start)
            .await
            .unwrap()
            .is_empty()
    );

    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .expect("parent asserts its child's role");
    assert_eq!(
        watchdog.round(&services, INTERVAL, start).await.unwrap(),
        vec![RoundOutcome::Armed("pij-pa".into())],
        "first sighting starts the clock rather than nudging at boot"
    );
    let at = |intervals: u64| start + intervals * INTERVAL * 1_000;
    assert!(
        watchdog
            .round(&services, INTERVAL, at(1) - 1_000)
            .await
            .unwrap()
            .is_empty(),
        "not due before a whole interval"
    );

    let nudged = watchdog.round(&services, INTERVAL, at(1)).await.unwrap();
    assert_eq!(
        nudged,
        vec![RoundOutcome::Nudged {
            pa: "pij-pa".into(),
            delivery: "queued".into()
        }]
    );
    let (_, job) = services
        .queue
        .peek(&["delivery:pij-pa".to_string()])
        .await
        .unwrap()
        .expect("the nudge is queued for the PA");
    let message: pij_core::model::Msg = serde_json::from_str(&job.payload).unwrap();
    assert_eq!(message.from.as_str(), "pij-bg");
    let body = message.body;
    assert!(
        body.starts_with("[pij watchdog] fleet round for pij-prime"),
        "{body}"
    );
    assert!(
        body.contains("pij-coder") && body.contains("650k"),
        "a seat in a sibling worktree is part of the fleet: {body}"
    );
    assert!(
        body.contains("pij-coder  question"),
        "parked seats are flagged: {body}"
    );
    assert!(
        !body.contains("pij-outsider"),
        "a child in another repository is not this fleet: {body}"
    );
    for not_pa in ["pij-prime", "pij-coder", "pij-outsider"] {
        assert!(
            services
                .queue
                .peek(&[format!("delivery:{not_pa}")])
                .await
                .unwrap()
                .is_none(),
            "{not_pa} is not a PA and gets no watchdog"
        );
    }

    assert_eq!(
        watchdog.round(&services, INTERVAL, at(2)).await.unwrap(),
        vec![RoundOutcome::Quiet("pij-pa".into())],
        "nothing changed, so no PA turn is paid for"
    );

    coder.state = SystemState::Working;
    coder.semantic_state = None;
    services.registry.put(coder).await.unwrap();
    assert_eq!(
        watchdog.round(&services, INTERVAL, at(3)).await.unwrap(),
        vec![RoundOutcome::Nudged {
            pa: "pij-pa".into(),
            delivery: "queued".into()
        }],
        "a real change brings the next nudge"
    );

    let rounds: Vec<String> = services
        .spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "watchdog.round")
        .map(|event| {
            serde_json::from_str::<serde_json::Value>(&event.payload).unwrap()["outcome"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        rounds,
        ["nudged", "quiet", "nudged"],
        "every round is audited"
    );
}

#[tokio::test]
async fn a_pa_mid_turn_is_deferred_not_interrupted() {
    let repos = Repos::new();
    let (services, _store) = services().await;
    let start = now_ms();
    let main = repos.path("main");
    services
        .registry
        .put(seat("pij-prime", &main, None))
        .await
        .unwrap();
    let mut pa = seat("pij-pa", &main, Some("pij-prime"));
    services.registry.put(pa.clone()).await.unwrap();
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .unwrap();
    let watchdog = PaWatchdog::default();
    watchdog.round(&services, INTERVAL, start).await.unwrap();

    pa.state = SystemState::Working;
    services.registry.put(pa).await.unwrap();
    assert!(
        watchdog
            .round(&services, INTERVAL, start + 5 * INTERVAL * 1_000)
            .await
            .unwrap()
            .is_empty(),
        "a working PA is never nudged mid-turn"
    );
}
