use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId, SemanticState, SystemState};
use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
use pij_core::usage_limit::{LIMIT_WAKE_MARGIN_SECS, QuotaRejection, UsageLimitSource};
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

/// Every message queued for `seat`, oldest first, drained from the queue.
async fn drain(services: &crate::Services, seat: &str) -> Vec<pij_core::model::Msg> {
    let mut messages = Vec::new();
    while let Some((job, payload)) = services
        .queue
        .claim(&[format!("delivery:{seat}")], "test-drain")
        .await
        .unwrap()
    {
        messages.push(serde_json::from_str(&payload.payload).unwrap());
        services
            .queue
            .ack(job, pij_core::model::Outcome::Done)
            .await
            .unwrap();
    }
    messages
}

async fn set_state(services: &crate::Services, seat: &SeatDescriptor, state: SystemState) {
    let mut seat = seat.clone();
    seat.state = state;
    services.registry.put(seat).await.unwrap();
}

fn audits(events: &[pij_core::model::Event], kind: &str) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|event| event.kind == kind)
        .map(|event| serde_json::from_str(&event.payload).unwrap())
        .collect()
}

async fn events(services: &crate::Services) -> Vec<pij_core::model::Event> {
    services
        .spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .unwrap()
}

/// Plan 172 (Jordan, 2026-10-13): the PA is nudged on the EDGE from working
/// to all-idle, once per idle episode; while anyone works, nothing is sent.
#[tokio::test]
async fn a_pa_is_nudged_once_each_time_its_whole_repository_goes_idle() {
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
    coder.state = SystemState::Working;
    let outsider = seat("pij-outsider", &repos.path("elsewhere"), Some("pij-prime"));
    for descriptor in [prime, pa.clone(), coder.clone(), outsider.clone()] {
        services.registry.put(descriptor).await.unwrap();
    }
    let watchdog = PaWatchdog::default();
    let round = |at: u64| watchdog.round(&services, INTERVAL, at);

    // No PA yet: a prime and its other children are never watchdog targets.
    set_state(&services, &coder, SystemState::Idle).await;
    assert!(round(start).await.unwrap().is_empty());
    set_state(&services, &coder, SystemState::Working).await;

    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .expect("parent asserts its child's role");
    for minute in 0..3 {
        assert!(
            round(start + minute * 60_000).await.unwrap().is_empty(),
            "a busy fleet sends nothing"
        );
    }

    // The one working seat, in a sibling worktree, stops: the edge.
    set_state(&services, &coder, SystemState::Idle).await;
    assert_eq!(
        round(start + 180_000).await.unwrap(),
        vec![RoundOutcome::Nudged {
            pa: "pij-pa".into(),
            delivery: "queued".into()
        }]
    );
    let nudges = drain(&services, "pij-pa").await;
    assert_eq!(nudges.len(), 1);
    assert_eq!(nudges[0].from.as_str(), "pij-bg");
    let body = &nudges[0].body;
    assert!(
        body.starts_with("[pij watchdog] fleet idle for pij-prime"),
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

    // Staying idle sends no more; neither does the PA's own turn, nor work
    // in another repository.
    for (at, who, state) in [
        (240_000, &pa, SystemState::Working),
        (300_000, &pa, SystemState::Idle),
        (360_000, &outsider, SystemState::Working),
        (420_000, &outsider, SystemState::Idle),
    ] {
        set_state(&services, who, state).await;
        assert!(
            round(start + at).await.unwrap().is_empty(),
            "{} {:?}: still one idle episode",
            who.id,
            state
        );
    }

    // Work again, then idle again: one more nudge.
    set_state(&services, &coder, SystemState::Working).await;
    assert!(round(start + 480_000).await.unwrap().is_empty());
    set_state(&services, &coder, SystemState::Idle).await;
    assert_eq!(round(start + 540_000).await.unwrap().len(), 1);
    assert_eq!(drain(&services, "pij-pa").await.len(), 1);
    for not_pa in ["pij-prime", "pij-coder", "pij-outsider"] {
        assert!(
            drain(&services, not_pa).await.is_empty(),
            "{not_pa} is not a PA and gets no watchdog"
        );
    }
    let rounds = audits(&events(&services).await, "watchdog.round");
    assert_eq!(
        rounds
            .iter()
            .map(|round| round["outcome"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["nudged", "nudged"],
        "every nudge is audited, and only nudges"
    );

    // A restarted daemon never nudges a fleet that is already idle.
    assert!(
        PaWatchdog::default()
            .round(&services, INTERVAL, start + 600_000)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A fake transcript source: the quota rejection each session is stopped on.
#[derive(Default)]
struct FakeLimits(std::sync::Mutex<std::collections::HashMap<String, QuotaRejection>>);

impl FakeLimits {
    fn stop(&self, seat: &str, limit_type: &str, resets_at_secs: u64) {
        self.0.lock().expect("fake limits").insert(
            format!("session-{seat}"),
            QuotaRejection {
                limit_type: limit_type.into(),
                resets_at_secs,
            },
        );
    }
}

impl UsageLimitSource for FakeLimits {
    fn latest_rejection(
        &self,
        harness: Harness,
        session: &str,
    ) -> pij_core::error::Result<Option<QuotaRejection>> {
        assert_eq!(harness, Harness::Claude, "only Claude seats are read");
        Ok(self.0.lock().expect("fake limits").get(session).cloned())
    }
}

/// 2026-10-01T00:00:00Z: Australia/Sydney is still on AEST.
const OCT_1_MS: u64 = 1_790_812_800_000;
/// A seven_day reset several days out, after Sydney moves to AEDT on 4 Oct:
/// 2026-10-06T16:00:00Z, 03:00 AEDT in Sydney and 02:00 AEST in Brisbane.
/// The wake is scheduled on the epoch alone, so neither zone moves it.
const SEVEN_DAY_RESET: u64 = 1_791_302_400;

/// R3: a fleet that went idle on a usage limit gets no edge nudge (nothing
/// can run), and ONE wake at resetsAt plus the margin to its PA, however many
/// seats share that limit, however many rounds or restarts pass.
#[tokio::test]
async fn a_fleet_idle_on_a_limit_is_woken_once_at_the_reset_not_at_the_edge() {
    let repos = Repos::new();
    let (mut services, _store) = services().await;
    let main = repos.path("main");
    // The PA is long and cold: the reset wake is exempt from the guard and
    // audited as forced with the reason limit-reset.
    services.session_status = Arc::new(
        FakeSessionStatus::new().with_reply("session-pij-pa", sized(500_000, OCT_1_MS - 7_200_000)),
    );
    let limits = Arc::new(FakeLimits::default());
    services.usage_limits = limits.clone();
    let mut workers = Vec::new();
    for (id, folder) in [("pij-a", &main), ("pij-b", &repos.path("feature"))] {
        let mut worker = seat(id, folder, None);
        worker.state = SystemState::Working;
        services.registry.put(worker.clone()).await.unwrap();
        workers.push(worker);
    }
    services
        .registry
        .put(seat("pij-prime", &main, None))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-pa", &main, Some("pij-prime")))
        .await
        .unwrap();
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .unwrap();
    let watchdog = PaWatchdog::default();
    assert!(
        watchdog
            .round(&services, INTERVAL, OCT_1_MS)
            .await
            .unwrap()
            .is_empty()
    );

    // Both workers hit the same seven_day limit and stop.
    for worker in &workers {
        limits.stop(worker.id.as_str(), "seven_day", SEVEN_DAY_RESET);
        set_state(&services, worker, SystemState::Idle).await;
    }
    assert_eq!(
        watchdog
            .round(&services, INTERVAL, OCT_1_MS + 60_000)
            .await
            .unwrap(),
        vec![RoundOutcome::IdleOnLimit("pij-pa".into())],
        "an idle fleet on a limit is not nudged at the edge"
    );
    let due = (SEVEN_DAY_RESET + LIMIT_WAKE_MARGIN_SECS) * 1_000;
    assert!(
        watchdog
            .round(&services, INTERVAL, due - 1)
            .await
            .unwrap()
            .is_empty(),
        "nothing before resetsAt plus the margin"
    );
    assert!(drain(&services, "pij-pa").await.is_empty());

    let scope = std::fs::canonicalize(repos.root.join("main/.git"))
        .unwrap()
        .display()
        .to_string();
    assert_eq!(
        watchdog.round(&services, INTERVAL, due).await.unwrap(),
        vec![RoundOutcome::LimitWoke {
            scope: scope.clone(),
            resets_at_secs: SEVEN_DAY_RESET,
            target: Some("pij-pa".into()),
            delivery: "queued".into(),
        }]
    );
    let wakes = drain(&services, "pij-pa").await;
    assert_eq!(wakes.len(), 1);
    let body = &wakes[0].body;
    assert!(
        body.contains("seven_day")
            && body.contains("pij-a, pij-b")
            && body.contains(&format!("epoch {SEVEN_DAY_RESET}"))
            && !body.contains("seen only now"),
        "{body}"
    );

    // One wake per (scope, resetsAt): later rounds and a restart send nothing.
    for (watchdog, at) in [
        (&watchdog, due + 60_000),
        (&PaWatchdog::default(), due + 120_000),
    ] {
        assert!(
            watchdog
                .round(&services, INTERVAL, at)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(drain(&services, "pij-pa").await.is_empty());
    for worker in ["pij-a", "pij-b"] {
        assert!(
            drain(&services, worker).await.is_empty(),
            "the watchdog never wakes a worker"
        );
    }
    let recorded = services.watchdogs().limit_wakes().await.unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].target_source, "pa-role");

    let all = events(&services).await;
    let scheduled = audits(&all, "watchdog.limit-scheduled");
    assert_eq!(
        scheduled.len(),
        2,
        "first sight, then again after the restart"
    );
    assert_eq!(scheduled[0]["due_at_ms"], due);
    let forced = audits(&all, pij_core::cold_wake::COLD_WAKE_FORCED_KIND);
    assert_eq!(forced.len(), 1, "{forced:?}");
    assert_eq!(forced[0]["reason"], "limit-reset");
    let woke = audits(&all, "watchdog.limit-wake");
    assert_eq!(woke.len(), 1);
    assert_eq!(woke[0]["reason"], "limit-reset");
    assert_eq!(woke[0]["cold_check"], "forced");
}

/// R3: with no PA in scope the wake goes to the prime, named by its recorded
/// designation; a reset already past when first seen wakes at once, once.
#[tokio::test]
async fn with_no_pa_a_late_reset_wakes_the_designated_prime_at_once() {
    let repos = Repos::new();
    let (mut services, _store) = services().await;
    let main = repos.path("main");
    let limits = Arc::new(FakeLimits::default());
    services.usage_limits = limits.clone();
    let now = now_ms();
    let resets_at = now / 1_000 - 3_600;
    limits.stop("pij-worker", "five_hour", resets_at);
    services
        .registry
        .put(seat("pij-worker", &repos.path("feature"), None))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-prime", &main, None))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-elsewhere-prime", &repos.path("elsewhere"), None))
        .await
        .unwrap();
    services
        .watchdogs()
        .designate_prime(&pij_core::orchestration::PrimeDesignation {
            seat: "pij-prime".into(),
            designated_by: "pij-human".into(),
            designated_at: 1,
            state: pij_core::orchestration::PrimeState::Current,
        })
        .await
        .unwrap();
    let watchdog = PaWatchdog::default();
    let outcome = watchdog.round(&services, INTERVAL, now).await.unwrap();
    assert!(
        matches!(
            &outcome[..],
            [RoundOutcome::LimitWoke { target: Some(target), resets_at_secs, .. }]
                if target.as_str() == "pij-prime" && *resets_at_secs == resets_at
        ),
        "{outcome:?}"
    );
    let wakes = drain(&services, "pij-prime").await;
    assert_eq!(wakes.len(), 1);
    assert!(wakes[0].body.contains("seen only now"), "{}", wakes[0].body);
    assert!(
        watchdog
            .round(&services, INTERVAL, now + 60_000)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        services.watchdogs().limit_wakes().await.unwrap()[0].target_source,
        "prime-designation"
    );
}

fn optin(seat: &str, by: &str, at_ms: u64) -> pij_core::watchdog::WatchdogOptIn {
    pij_core::watchdog::WatchdogOptIn {
        seat: seat.into(),
        interval_secs: INTERVAL,
        set_by: by.into(),
        set_at_ms: at_ms,
    }
}

#[tokio::test]
async fn an_opted_in_seat_is_nudged_when_quiet_and_told_how_to_stop() {
    let (services, _store) = services().await;
    let start = now_ms();
    let mut worker = seat("pij-worker", "/nowhere", None);
    services.registry.put(worker.clone()).await.unwrap();
    services
        .registry
        .put(seat("pij-bystander", "/nowhere", None))
        .await
        .unwrap();
    services
        .watchdogs()
        .set_watchdog(&optin("pij-worker", "pij-prime", start))
        .await
        .unwrap();
    let watchdog = PaWatchdog::default();
    let at = |secs: u64| start + secs * 1_000;

    assert!(
        watchdog
            .round(&services, INTERVAL, at(INTERVAL - 1))
            .await
            .unwrap()
            .is_empty(),
        "not quiet a whole interval yet"
    );
    assert_eq!(
        watchdog
            .round(&services, INTERVAL, at(INTERVAL))
            .await
            .unwrap(),
        vec![RoundOutcome::OptInNudged {
            seat: "pij-worker".into(),
            delivery: "queued".into()
        }],
        "only the opted-in seat; the bystander never opted in"
    );
    let (_, job) = services
        .queue
        .peek(&["delivery:pij-worker".to_string()])
        .await
        .unwrap()
        .expect("nudge queued");
    let body = serde_json::from_str::<pij_core::model::Msg>(&job.payload)
        .unwrap()
        .body;
    assert!(
        body.starts_with("[pij watchdog] pij-worker: quiet 20m"),
        "{body}"
    );
    assert!(body.contains("set by pij-prime"), "{body}");
    assert!(
        body.ends_with("No more work coming? Stop this watchdog: `pij watchdog off`."),
        "every nudge says how to stop: {body}"
    );
    assert!(
        watchdog
            .round(&services, INTERVAL, at(INTERVAL + 60))
            .await
            .unwrap()
            .is_empty(),
        "one nudge per quiet interval"
    );

    for (state, semantic, why) in [
        (SystemState::Working, None, "mid-turn is not a stall"),
        (
            SystemState::Idle,
            Some(SemanticState::Waiting),
            "waiting is deliberate",
        ),
    ] {
        worker.state = state;
        worker.semantic_state = semantic;
        services.registry.put(worker.clone()).await.unwrap();
        assert!(
            watchdog
                .round(&services, INTERVAL, at(10 * INTERVAL))
                .await
                .unwrap()
                .is_empty(),
            "{why}"
        );
    }

    worker.state = SystemState::Idle;
    worker.semantic_state = None;
    services.registry.put(worker).await.unwrap();
    services
        .watchdogs()
        .clear_watchdog(&"pij-worker".into())
        .await
        .unwrap();
    assert!(
        watchdog
            .round(&services, INTERVAL, at(20 * INTERVAL))
            .await
            .unwrap()
            .is_empty(),
        "off means off"
    );
}

#[tokio::test]
async fn any_seat_can_switch_another_seats_watchdog_and_the_subject_is_told() {
    let (services, _store) = services().await;
    for id in ["pij-prime", "pij-worker", "pij-pa"] {
        let parent = (id == "pij-pa").then_some("pij-prime");
        services
            .registry
            .put(seat(id, "/nowhere", parent))
            .await
            .unwrap();
    }
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .unwrap();
    let delivery = Arc::clone(&services.delivery);
    let spine = Arc::clone(&services.spine);
    let watchdogs = services.watchdogs();
    let audit = || {
        let spine = Arc::clone(&spine);
        async move {
            spine
                .tail(None, pij_core::model::Seq(0))
                .await
                .unwrap()
                .into_iter()
                .fold((0, 0), |(changes, unchanged), event| {
                    match event.kind.as_str() {
                        "watchdog.on" | "watchdog.off" => (changes + 1, unchanged),
                        "watchdog.unchanged" => (changes, unchanged + 1),
                        _ => (changes, unchanged),
                    }
                })
        }
    };
    let told = |seat: &'static str| {
        let delivery = Arc::clone(&delivery);
        async move { delivery.pending_fyi_count(&seat.into()).await.unwrap() }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = crate::http::router_with_config(
        services,
        crate::http::HttpConfig {
            auth: crate::http::AuthRing::new("key".to_string(), std::iter::empty())
                .expect("local-only auth"),
            machine_alias: "workstation".into(),
        },
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let call = |argv: Vec<&'static str>| async move {
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/v1/watchdog"))
            .bearer_auth("key")
            .json(&serde_json::json!({"argv": argv, "caller": {"PIJ_SESSION_ID": "pij-prime"}}))
            .send()
            .await
            .unwrap();
        (
            response.status().as_u16(),
            response.json::<serde_json::Value>().await.unwrap(),
        )
    };

    // A no-op notifies nobody; the attempt is audited as unchanged (review of #38).
    let (status, noop) = call(vec!["watchdog", "off", "pij-worker"]).await;
    assert_eq!(
        (status, noop["data"]["changed"].clone()),
        (200, false.into()),
        "{noop}"
    );
    assert_eq!(
        (told("pij-worker").await, audit().await),
        (0, (0, 1)),
        "off on an off seat notifies nobody"
    );

    let (status, on) = call(vec!["watchdog", "on", "pij-worker", "--every", "30m"]).await;
    assert_eq!(status, 200, "{on}");
    assert_eq!(on["data"]["enabled"], true);
    assert_eq!(on["data"]["interval_secs"], 1_800);
    assert_eq!(on["data"]["set_by"], "pij-prime");
    assert_eq!(on["data"]["subject_told"], "held (fyi)", "{on}");
    assert_eq!(
        (told("pij-worker").await, audit().await),
        (1, (1, 1)),
        "a real change tells the subject once, without waking it, and is audited once"
    );
    let (_, again) = call(vec!["watchdog", "on", "pij-worker", "--every", "30m"]).await;
    assert_eq!(again["data"]["changed"], false, "{again}");
    assert_eq!(
        (told("pij-worker").await, audit().await),
        (1, (1, 2)),
        "repeating on is silent"
    );
    assert_eq!(
        watchdogs.list_watchdogs().await.unwrap(),
        vec![pij_core::watchdog::WatchdogOptIn {
            seat: "pij-worker".into(),
            interval_secs: 1_800,
            set_by: "pij-prime".into(),
            set_at_ms: watchdogs.list_watchdogs().await.unwrap()[0].set_at_ms,
        }]
    );

    let (status, pa) = call(vec!["watchdog", "off", "pij-pa"]).await;
    assert_eq!(status, 409, "{pa}");
    assert_eq!(pa["details"]["code"], "E-RS-WATCHDOG-PA");

    let (status, listing) = call(vec!["watchdog", "status"]).await;
    assert_eq!(status, 200);
    assert_eq!(listing["data"]["pas"], serde_json::json!(["pij-pa"]));
    assert_eq!(listing["data"]["optins"][0]["seat"], "pij-worker");

    let (status, off) = call(vec!["watchdog", "off", "pij-worker"]).await;
    assert_eq!(status, 200, "{off}");
    assert_eq!(
        (
            off["data"]["enabled"].clone(),
            off["data"]["changed"].clone()
        ),
        (false.into(), true.into())
    );
    assert!(watchdogs.list_watchdogs().await.unwrap().is_empty());
    assert_eq!(
        (told("pij-worker").await, audit().await),
        (1, (2, 2)),
        "off is a real change, audited; its notice coalesces with the unread one"
    );
    let (_, again) = call(vec!["watchdog", "off", "pij-worker"]).await;
    assert_eq!(again["data"]["changed"], false);
    assert_eq!(
        (told("pij-worker").await, audit().await),
        (1, (2, 3)),
        "repeating off is silent"
    );
    for _ in 0..5 {
        call(vec!["watchdog", "on", "pij-worker"]).await;
        call(vec!["watchdog", "off", "pij-worker"]).await;
    }
    assert_eq!(
        (told("pij-worker").await, audit().await.0),
        (1, 12),
        "churn is audited per change, but leaves one pending notice, not a backlog"
    );

    let (status, missing) = call(vec!["watchdog", "on", "pij-ghost"]).await;
    assert_eq!(
        (status, missing["details"]["code"].clone()),
        (404, "E-RS-NO-SEAT".into())
    );
    server.abort();
}

/// Counts every row a spine read returns; writes pass through uncounted.
struct CountingSpine {
    inner: Arc<dyn pij_core::ports::Spine>,
    rows: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl pij_core::ports::Spine for CountingSpine {
    async fn append(
        &self,
        event: pij_core::model::Event,
    ) -> pij_core::error::Result<pij_core::model::Seq> {
        self.inner.append(event).await
    }
    async fn tail(
        &self,
        seat: Option<&SeatId>,
        since: pij_core::model::Seq,
    ) -> pij_core::error::Result<Vec<pij_core::model::Event>> {
        let rows = self.inner.tail(seat, since).await?;
        self.rows.fetch_add(rows.len(), Ordering::Relaxed);
        Ok(rows)
    }
    async fn latest_matching(
        &self,
        seat: &SeatId,
        kinds: &[&str],
    ) -> pij_core::error::Result<Option<pij_core::model::Event>> {
        let row = self.inner.latest_matching(seat, kinds).await?;
        self.rows
            .fetch_add(usize::from(row.is_some()), Ordering::Relaxed);
        Ok(row)
    }
    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> pij_core::error::Result<Option<pij_core::model::Event>> {
        let row = self
            .inner
            .latest_matching_message(seat, kind, msg_id)
            .await?;
        self.rows
            .fetch_add(usize::from(row.is_some()), Ordering::Relaxed);
        Ok(row)
    }
    async fn matching_since(
        &self,
        seat: &SeatId,
        window: &pij_core::ports::SpineWindow,
    ) -> pij_core::error::Result<Vec<pij_core::model::Event>> {
        let rows = self.inner.matching_since(seat, window).await?;
        self.rows.fetch_add(rows.len(), Ordering::Relaxed);
        Ok(rows)
    }
}

/// Review of #38: every round, quiet or due, folded the whole spine. Rows
/// read per round must follow the fleet, not history: grow unrelated history
/// by thousands of events and the count does not move.
#[tokio::test]
async fn rows_read_per_round_stay_flat_as_unrelated_history_grows() {
    let repos = Repos::new();
    let (mut services, _store) = services().await;
    let inner = Arc::clone(&services.spine);
    let counting = Arc::new(CountingSpine {
        inner: Arc::clone(&inner),
        rows: AtomicUsize::new(0),
    });
    // Every read path the round could take goes through the counter,
    // including the bus that the whole-history anomaly scan used.
    services.spine = counting.clone();
    services.event_bus = Arc::new(crate::events::EventBus::new(counting.clone(), 64).unwrap());
    services.anomalies = Arc::new(crate::http::anomalies::AnomalyService::new(
        services.watchdogs(),
        Arc::clone(&services.registry),
        Arc::clone(&services.event_bus),
        Arc::clone(&services.liveness),
    ));
    let main = repos.path("main");
    services
        .registry
        .put(seat("pij-prime", &main, None))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-pa", &main, Some("pij-prime")))
        .await
        .unwrap();
    let mut coder = seat("pij-coder", &main, None);
    coder.state = SystemState::Working;
    services.registry.put(coder.clone()).await.unwrap();
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .unwrap();
    let noise = |count: usize| {
        let inner = Arc::clone(&inner);
        async move {
            for n in 0..count {
                inner
                    .append(pij_core::model::Event {
                        seq: None,
                        v: 1,
                        at: n as u64,
                        kind: "report.now".into(),
                        seat: Some("pij-noise".into()),
                        payload: "{\"did\":\"x\",\"next\":\"y\"}".into(),
                    })
                    .await
                    .unwrap();
            }
        }
    };
    let watchdog = PaWatchdog::default();
    let start = now_ms();
    let at = |minutes: u64| start + minutes * 60_000;
    let round = |when: u64| {
        let (services, watchdog, counting) = (&services, &watchdog, &counting);
        async move {
            let before = counting.rows.load(Ordering::Relaxed);
            let outcome = watchdog.round(services, INTERVAL, when).await.unwrap();
            (outcome, counting.rows.load(Ordering::Relaxed) - before)
        }
    };

    round(start).await;
    noise(4_096).await;
    set_state(&services, &coder, SystemState::Idle).await;
    let (first, reads_due) = round(at(1)).await;
    assert!(
        matches!(first[..], [RoundOutcome::Nudged { .. }]),
        "{first:?}"
    );
    assert!(
        reads_due <= 3 * 3,
        "at most three LIMIT 1 reads per fleet seat, got {reads_due}"
    );

    noise(4_096).await;
    let (quiet, reads_quiet) = round(at(2)).await;
    assert!(quiet.is_empty(), "{quiet:?}");
    assert_eq!(reads_quiet, 0, "between edges a round reads no history");

    noise(4_096).await;
    set_state(&services, &coder, SystemState::Working).await;
    round(at(3)).await;
    set_state(&services, &coder, SystemState::Idle).await;
    let (again, reads_again) = round(at(4)).await;
    assert!(
        matches!(again[..], [RoundOutcome::Nudged { .. }]),
        "{again:?}"
    );
    assert_eq!(
        reads_again, reads_due,
        "12k unrelated events later, rows read are flat"
    );
}

/// Review of #38: a cold transcript read can outlive its 3s wait. The next
/// round must not start another read behind it, so reads per seat stay at one
/// however many rounds pass.
#[tokio::test]
async fn a_status_read_that_outlives_its_wait_is_never_started_twice() {
    let repos = Repos::new();
    let (mut services, _store) = services().await;
    let start = now_ms();
    let source = Arc::new(
        FakeSessionStatus::new()
            .with_reply("session-pij-prime", sized(400_000, start))
            .with_reply("session-pij-pa", sized(90_000, start))
            .with_hang("session-pij-coder"),
    );
    services.session_status = source.clone();
    let main = repos.path("main");
    services
        .registry
        .put(seat("pij-prime", &main, None))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-pa", &main, Some("pij-prime")))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-coder", &main, None))
        .await
        .unwrap();
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .unwrap();
    let coder_reads = || {
        source
            .calls()
            .iter()
            .filter(|call| format!("{call:?}").contains("status:pij-coder:"))
            .count()
    };
    let coder = seat("pij-coder", &main, None);
    let watchdog = PaWatchdog::default();
    for episode in 1..=3 {
        set_state(&services, &coder, SystemState::Working).await;
        watchdog
            .round(&services, INTERVAL, start + episode * 120_000)
            .await
            .unwrap();
        set_state(&services, &coder, SystemState::Idle).await;
        let nudged = watchdog
            .round(&services, INTERVAL, start + episode * 120_000 + 60_000)
            .await
            .unwrap();
        assert_eq!(nudged.len(), 1, "episode {episode}: {nudged:?}");
        assert_eq!(
            coder_reads(),
            1,
            "episode {episode}: one hung read, never a second"
        );
    }
}

/// Review of 20bcfb3 (pij-efficient-lemur): `anomaly.clear` events for every
/// kind share one event kind, so "the seat's latest clear" can belong to a
/// different occurrence. Clearing another anomaly must not resurrect a stale
/// card that was already cleared.
#[tokio::test]
async fn clearing_another_anomaly_does_not_resurrect_a_cleared_stale_card() {
    let (services, _store) = services().await;
    services
        .registry
        .put(seat("pij-prime", "/review-dispositions", None))
        .await
        .unwrap();
    services
        .registry
        .put(seat("pij-pa", "/review-dispositions", Some("pij-prime")))
        .await
        .unwrap();
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .unwrap();
    let start = now_ms();
    let card = services
        .spine
        .append(pij_core::model::Event {
            seq: None,
            v: 1,
            at: 1,
            kind: "report.now".into(),
            seat: Some("pij-prime".into()),
            payload: serde_json::json!({"did": "seeded old card", "next": "continue"}).to_string(),
        })
        .await
        .unwrap();
    let clear = |occurrence: String| pij_core::model::Event {
        seq: None,
        v: 1,
        at: start,
        kind: "anomaly.clear".into(),
        seat: Some("pij-prime".into()),
        payload: serde_json::json!({"occurrence": occurrence, "actor": "pij-prime"}).to_string(),
    };
    services
        .spine
        .append(clear(format!("status-stale:pij-prime:{}", card.0)))
        .await
        .unwrap();
    let prime = seat("pij-prime", "/review-dispositions", None);
    let watchdog = PaWatchdog::default();
    let episode = |at: u64| {
        let (services, watchdog, prime) = (&services, &watchdog, &prime);
        async move {
            set_state(services, prime, SystemState::Working).await;
            watchdog.round(services, INTERVAL, at).await.unwrap();
            set_state(services, prime, SystemState::Idle).await;
            let outcome = watchdog
                .round(services, INTERVAL, at + 60_000)
                .await
                .unwrap();
            assert!(
                matches!(outcome.as_slice(), [RoundOutcome::Nudged { .. }]),
                "{outcome:?}"
            );
            let digests = drain(services, "pij-pa").await;
            assert_eq!(digests.len(), 1);
            digests[0].body.clone()
        }
    };
    let first = episode(start).await;
    assert!(
        !first.contains("pij-prime  status-stale"),
        "a cleared stale card is not flagged: {first}"
    );
    services
        .spine
        .append(clear("dead-seat:pij-prime:99999".into()))
        .await
        .unwrap();
    let second = episode(start + 120_000).await;
    assert!(
        !second.contains("pij-prime  status-stale"),
        "clearing another anomaly must not resurrect the same cleared stale card: {second}"
    );
}
