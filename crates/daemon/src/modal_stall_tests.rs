use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::modal_stall::STALL_AFTER_MS;
use pij_core::model::{Event, Harness, SeatDescriptor, SeatId, SystemState};
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeTmux;

use crate::pa_watchdog::{PaWatchdog, RoundOutcome};

const INTERVAL: u64 = 1_200;
const MINUTE: u64 = 60_000;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../testkit/fixtures/harnesses")
            .join(name),
    )
    .expect("read a harness fixture")
}

/// Real store services whose tmux shows `screen` on every pane.
async fn services(screen: &str) -> (crate::Services, Arc<FakeTmux>, FreshStore, String) {
    let (services, tmux, store, folder, _) =
        services_with_config(FakeTmux::new().with_standing_capture(screen)).await;
    (services, tmux, store, folder)
}

async fn services_with_config(
    tmux: FakeTmux,
) -> (crate::Services, Arc<FakeTmux>, FreshStore, String, Config) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
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
    let mut services = build(&config, &store).await;
    let tmux = Arc::new(tmux);
    services.tmux = tmux.clone();
    // Not a git repository: every seat here shares this folder as its scope.
    let folder = std::env::temp_dir().join(format!(
        "pij-modal-stall-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&folder).unwrap();
    (services, tmux, store, folder.display().to_string(), config)
}

async fn build(config: &Config, store: &FreshStore) -> crate::Services {
    crate::build_services(config, &Path::new(&store.path()).with_extension("signals"))
        .await
        .expect("real store services")
}

/// Serve a second set of services over the same store, through the real
/// router: what `pij list` and `pij state` read.
async fn http(config: &Config, store: &FreshStore) -> std::net::SocketAddr {
    let router = crate::http::router(build(config, store).await, "key".into());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// The `dialog` on `pij list`'s row and `pij state`'s card for `seat`.
async fn views(addr: std::net::SocketAddr, seat: &str) -> (serde_json::Value, serde_json::Value) {
    let client = reqwest::Client::new();
    let list: serde_json::Value = client
        .get(format!("http://{addr}/v1/seats?scope=local"))
        .bearer_auth("key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = list["data"]["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|row| row["id"] == seat)
        .expect("listed")
        .clone();
    let card: serde_json::Value = client
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({"id": seat}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (row["dialog"].clone(), card["data"]["dialog"].clone())
}

fn seat(id: &str, harness: Harness, folder: &str, parent: Option<&str>) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(id, harness, folder);
    seat.parent = parent.map(SeatId::from);
    seat
}

/// The prime, its PA and its PM; the PM's Copilot coder, working in pane %17.
async fn fleet(services: &crate::Services, folder: &str) -> SeatDescriptor {
    let mut asking = seat("pij-asking", Harness::Copilot, folder, Some("pij-pm"));
    asking.pane = Some("%17".into());
    asking.state = SystemState::Working;
    for descriptor in [
        seat("pij-prime", Harness::Claude, folder, None),
        seat("pij-pa", Harness::Claude, folder, Some("pij-prime")),
        seat("pij-pm", Harness::Copilot, folder, Some("pij-prime")),
        asking.clone(),
    ] {
        services.registry.put(descriptor).await.unwrap();
    }
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some("pa".into()))
        .await
        .expect("parent asserts its child's role");
    asking
}

async fn own_event(services: &crate::Services, seat: &str, at: u64) {
    let _ = services
        .spine
        .append(Event {
            seq: None,
            v: 1,
            at,
            kind: "seat.activity".into(),
            seat: Some(SeatId::from(seat)),
            payload: r#"{"state":"working"}"#.into(),
        })
        .await
        .unwrap();
}

/// The native extension reader's ack, as herring's reader wrote it at 04:14:09.
async fn reader_ack(services: &crate::Services, seat: &str, at: u64) {
    let _ = services
        .spine
        .append(Event {
            seq: None,
            v: 1,
            at,
            kind: "delivery.inbox-ack".into(),
            seat: Some(SeatId::from(seat)),
            payload: r#"{"job_id":16399,"authenticated_machine":"local","evidence_grade":"machine","outcome":"reader-read"}"#.into(),
        })
        .await
        .unwrap();
}

fn stalls(outcomes: Vec<RoundOutcome>) -> Vec<RoundOutcome> {
    outcomes
        .into_iter()
        .filter(|outcome| matches!(outcome, RoundOutcome::ModalStall { .. }))
        .collect()
}

/// Stall messages queued for `seat`, drained.
async fn told(services: &crate::Services, seat: &str) -> Vec<pij_core::model::Msg> {
    let mut messages = Vec::new();
    while let Some((job, payload)) = services
        .queue
        .claim(&[format!("delivery:{seat}")], "test-drain")
        .await
        .unwrap()
    {
        let msg: pij_core::model::Msg = serde_json::from_str(&payload.payload).unwrap();
        if msg.msg_id.starts_with("modal-stall:") {
            messages.push(msg);
        }
        services
            .queue
            .ack(job, pij_core::model::Outcome::Done)
            .await
            .unwrap();
    }
    messages
}

/// Pane reads for `pane`, all through `capture_without_launch`: the generic
/// capture (which can fork) is never called.
fn captures(tmux: &FakeTmux, pane: &str) -> usize {
    let calls = tmux.calls();
    assert!(
        !calls.iter().any(|call| call.starts_with("capture:")),
        "the detector never uses the forking capture: {calls:?}"
    );
    let read = format!("capture_without_launch:{pane}");
    calls.iter().filter(|call| **call == read).count()
}

/// bp-0003: herring's screen, three rounds, a restart, a new stretch. The
/// parent and the PA are each told once per stretch; pij presses no key.
#[tokio::test]
async fn a_seat_on_a_dialog_is_told_once_per_stretch_to_its_parent_and_pa() {
    let (services, tmux, store, folder, config) = services_with_config(
        FakeTmux::new().with_standing_capture(fixture("copilot-follow-up-dialog-herring.txt")),
    )
    .await;
    fleet(&services, &folder).await;
    let addr = http(&config, &store).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-asking", start).await;
    let watchdog = PaWatchdog::default();

    // Silent 19 minutes: not read.
    let early = watchdog
        .round(&services, INTERVAL, start + 19 * MINUTE)
        .await
        .unwrap();
    assert!(stalls(early).is_empty());
    assert_eq!(
        captures(&tmux, "%17"),
        0,
        "a seat silent under 20 min is not read"
    );

    let first = start + STALL_AFTER_MS + MINUTE;
    let outcomes = stalls(watchdog.round(&services, INTERVAL, first).await.unwrap());
    assert_eq!(
        outcomes,
        vec![RoundOutcome::ModalStall {
            seat: "pij-asking".into(),
            told: vec![
                ("pij-pm".into(), "queued".into()),
                ("pij-pa".into(), "queued".into())
            ],
        }]
    );
    for later in [first + MINUTE, first + 2 * MINUTE] {
        assert!(stalls(watchdog.round(&services, INTERVAL, later).await.unwrap()).is_empty());
    }
    // A restart: the claim is in the store, not in memory.
    let restarted = PaWatchdog::default();
    assert!(
        stalls(
            restarted
                .round(&services, INTERVAL, first + 3 * MINUTE)
                .await
                .unwrap()
        )
        .is_empty()
    );
    assert_eq!(
        captures(&tmux, "%17"),
        4,
        "one read per round while it stands"
    );
    let to_pm = told(&services, "pij-pm").await;
    let to_pa = told(&services, "pij-pa").await;
    assert_eq!((to_pm.len(), to_pa.len()), (1, 1));
    let body = &to_pa[0].body;
    assert_eq!(to_pa[0].from.as_str(), "pij-bg");
    assert!(
        body.contains("pij-asking (copilot) has been silent for 21m, with Copilot selection dialog (ctrl+d declines) on screen")
            && body.contains("ctrl+d declines")
            && body.contains("Never choose an option for it"),
        "{body}"
    );

    // bp-0004: pij list and the state card show it while it stands.
    let (listed, card) = views(addr, "pij-asking").await;
    assert_eq!(listed["key"], "ctrl+d", "{listed}");
    assert_eq!(
        listed["dialog"],
        "Copilot selection dialog (ctrl+d declines)"
    );
    assert_eq!(listed["since_ms"], start);
    assert_eq!(card, listed);
    assert_eq!(views(addr, "pij-pm").await.0, serde_json::Value::Null);

    // The seat records an event: the stretch ends, and the view clears.
    let resumed = first + 4 * MINUTE;
    own_event(&services, "pij-asking", resumed).await;
    assert_eq!(
        views(addr, "pij-asking").await,
        (serde_json::Value::Null, serde_json::Value::Null),
        "an own event clears the view"
    );
    assert!(
        stalls(
            restarted
                .round(&services, INTERVAL, resumed + MINUTE)
                .await
                .unwrap()
        )
        .is_empty()
    );
    // A second stall, after the new stretch's 20 minutes: one more each.
    let second = resumed + STALL_AFTER_MS + MINUTE;
    assert_eq!(
        stalls(restarted.round(&services, INTERVAL, second).await.unwrap()).len(),
        1
    );
    assert_eq!(told(&services, "pij-pm").await.len(), 1);
    assert_eq!(told(&services, "pij-pa").await.len(), 1);

    let keys: Vec<_> = tmux
        .calls()
        .into_iter()
        .filter(|call| !call.starts_with("capture_without_launch:"))
        .collect();
    assert!(keys.is_empty(), "pij never touches the pane: {keys:?}");
}

/// F108: herring's real timeline on the post-#156 daemon. Its last own event
/// was at 04:11:22, the dialog opened at 04:11:34, its native reader acked a
/// queued message at 04:14:09, and the prime declined at 04:33:11. The ack is
/// not the seat's doing, so the last round before the decline (04:32:42)
/// claims the stall and tells the parent and the PA, and the dialog shows on
/// `pij list` while it stands.
#[tokio::test]
async fn a_reader_ack_mid_dialog_does_not_restart_the_quiet_stretch() {
    let (services, tmux, store, folder, config) = services_with_config(
        FakeTmux::new().with_standing_capture(fixture("copilot-follow-up-dialog-herring-f108.txt")),
    )
    .await;
    fleet(&services, &folder).await;
    let addr = http(&config, &store).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-asking", start).await;
    reader_ack(&services, "pij-asking", start + 167_000).await;
    let watchdog = PaWatchdog::default();

    let outcomes = stalls(
        watchdog
            .round(&services, INTERVAL, start + 1_280_000)
            .await
            .unwrap(),
    );
    assert_eq!(
        outcomes,
        vec![RoundOutcome::ModalStall {
            seat: "pij-asking".into(),
            told: vec![
                ("pij-pm".into(), "queued".into()),
                ("pij-pa".into(), "queued".into())
            ],
        }],
        "the stall is claimed before the decline"
    );
    assert_eq!(captures(&tmux, "%17"), 1);
    assert_eq!(told(&services, "pij-pm").await.len(), 1);
    let to_pa = told(&services, "pij-pa").await;
    assert_eq!(to_pa.len(), 1);
    assert!(
        to_pa[0].body.contains("silent for 21m"),
        "measured from the seat's own event, not the ack: {}",
        to_pa[0].body
    );
    let (listed, _) = views(addr, "pij-asking").await;
    assert_eq!(listed["since_ms"], start, "{listed}");

    // Another reader ack while it stands neither hides it nor re-tells.
    reader_ack(&services, "pij-asking", start + 1_300_000).await;
    assert_eq!(views(addr, "pij-asking").await.0, listed);
    assert!(
        stalls(
            watchdog
                .round(&services, INTERVAL, start + 1_340_000)
                .await
                .unwrap()
        )
        .is_empty()
    );
}

/// bp-0001 and bp-0005: a healthy working seat (recent own events) and a seat
/// whose harness has no known dialog are never read; a candidate whose pane
/// shows its normal working screen is read once a round and nobody is told.
#[tokio::test]
async fn only_candidates_are_read_and_a_working_screen_tells_nobody() {
    let (services, tmux, _store, folder) = services(&fixture("copilot-working.txt")).await;
    fleet(&services, &folder).await;
    let mut healthy = seat("pij-busy", Harness::Copilot, &folder, Some("pij-pm"));
    healthy.pane = Some("%18".into());
    healthy.state = SystemState::Working;
    let mut omp = seat("pij-omp", Harness::Omp, &folder, Some("pij-pm"));
    omp.pane = Some("%19".into());
    omp.state = SystemState::Working;
    services.registry.put(healthy).await.unwrap();
    services.registry.put(omp).await.unwrap();
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-asking", start).await;
    own_event(&services, "pij-omp", start).await;
    let watchdog = PaWatchdog::default();
    for minute in 21..24 {
        let at = start + minute * MINUTE;
        own_event(&services, "pij-busy", at - MINUTE).await;
        assert!(stalls(watchdog.round(&services, INTERVAL, at).await.unwrap()).is_empty());
    }
    assert_eq!(captures(&tmux, "%17"), 3);
    assert_eq!(
        captures(&tmux, "%18"),
        0,
        "a seat with recent events is not read"
    );
    assert_eq!(
        captures(&tmux, "%19"),
        0,
        "no OMP dialog is known, so none is read"
    );
    assert!(told(&services, "pij-pa").await.is_empty());
    assert!(
        services
            .watchdogs()
            .modal_stalls()
            .await
            .unwrap()
            .is_empty()
    );
}

/// P156-F001 (pm-001's ruling): with no live control client a candidate is
/// not read, nothing is launched, nobody is told, and the skip is recorded as
/// a `watchdog.round` event with its reason. With a live client: one read.
#[tokio::test]
async fn with_no_live_control_client_a_candidate_is_skipped_and_recorded() {
    let (services, tmux, _store, folder, _) = services_with_config(
        FakeTmux::new()
            .with_standing_capture(fixture("copilot-follow-up-dialog-herring.txt"))
            .without_control_client(),
    )
    .await;
    fleet(&services, &folder).await;
    let mut second = seat("pij-asking-too", Harness::Claude, &folder, Some("pij-pm"));
    second.pane = Some("%18".into());
    second.state = SystemState::Working;
    services.registry.put(second).await.unwrap();
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-asking", start).await;
    own_event(&services, "pij-asking-too", start).await;
    let outcomes = PaWatchdog::default()
        .round(&services, INTERVAL, start + STALL_AFTER_MS + MINUTE)
        .await
        .unwrap();
    let mut skipped: Vec<_> = outcomes
        .into_iter()
        .filter_map(|outcome| match outcome {
            RoundOutcome::ModalStallSkipped { seat, reason } => Some((seat, reason)),
            RoundOutcome::ModalStall { .. } => panic!("told while unread"),
            _ => None,
        })
        .collect();
    skipped.sort();
    assert_eq!(
        skipped,
        vec![
            (
                "pij-asking".into(),
                super::NO_LIVE_CONTROL_CLIENT.to_string()
            ),
            (
                "pij-asking-too".into(),
                super::NO_LIVE_CONTROL_CLIENT.to_string()
            ),
        ]
    );
    let mut calls = tmux.calls();
    calls.sort();
    assert_eq!(
        calls,
        vec!["capture_without_launch:%17", "capture_without_launch:%18"],
        "one control-only attempt each, and no forking capture"
    );
    for seat in ["pij-asking", "pij-asking-too"] {
        let recorded: Vec<_> = services
            .spine
            .tail(Some(&SeatId::from(seat)), pij_core::model::Seq(0))
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "watchdog.round")
            .map(|event| serde_json::from_str::<serde_json::Value>(&event.payload).unwrap())
            .collect();
        assert_eq!(recorded.len(), 1, "{seat}");
        assert_eq!(recorded[0]["outcome"], "modal-stall-skipped");
        assert_eq!(recorded[0]["reason"], super::NO_LIVE_CONTROL_CLIENT);
    }
    assert!(
        services
            .watchdogs()
            .modal_stalls()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(told(&services, "pij-pa").await.is_empty());
    assert!(told(&services, "pij-pm").await.is_empty());
}

/// Fleet for the retry tests, with ids of their own: the refusal list is
/// shared by every test in this binary.
async fn retry_fleet(services: &crate::Services, folder: &str, prefix: &str, with_pa: bool) {
    let mut asking = seat(
        &format!("{prefix}-asking"),
        Harness::Copilot,
        folder,
        Some(&format!("{prefix}-pm")),
    );
    asking.pane = Some("%27".into());
    asking.state = SystemState::Working;
    for descriptor in [
        seat("pij-prime", Harness::Claude, folder, None),
        seat(
            &format!("{prefix}-pm"),
            Harness::Copilot,
            folder,
            Some("pij-prime"),
        ),
        asking,
    ] {
        services.registry.put(descriptor).await.unwrap();
    }
    if with_pa {
        add_pa(services, folder, &format!("{prefix}-pa")).await;
    }
}

async fn add_pa(services: &crate::Services, folder: &str, id: &str) {
    services
        .registry
        .put(seat(id, Harness::Claude, folder, Some("pij-prime")))
        .await
        .unwrap();
    services
        .roles
        .assert_role(&"pij-prime".into(), &id.into(), Some("pa".into()))
        .await
        .unwrap();
}

fn refuse(prefix: &str, ids: &[&str]) {
    let mut refused = super::FAIL_FOR_TEST.lock().unwrap();
    refused.retain(|id| !id.as_str().starts_with(prefix));
    refused.extend(ids.iter().map(|id| SeatId::from(format!("{prefix}-{id}"))));
}

fn told_now(outcomes: Vec<RoundOutcome>) -> Vec<(String, String)> {
    let mut told: Vec<(String, String)> = stalls(outcomes)
        .into_iter()
        .flat_map(|outcome| match outcome {
            RoundOutcome::ModalStall { told, .. } => told,
            _ => Vec::new(),
        })
        .map(|(seat, delivery)| (seat.as_str().to_string(), delivery))
        .collect();
    told.sort();
    told
}

const INJECTED_ERROR: &str = "error: injected by the test";

/// Review F001: a failed tell doesn't consume the stretch. The PA fails while
/// the parent succeeds; after a restart, the next round tells only the PA,
/// and nobody is told twice.
#[tokio::test]
async fn a_failed_tell_is_retried_and_an_accepted_one_is_not_repeated() {
    let p = "pij-retry";
    let (services, _tmux, _store, folder) =
        services(&fixture("copilot-follow-up-dialog-herring.txt")).await;
    retry_fleet(&services, &folder, p, true).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, &format!("{p}-asking"), start).await;
    let at = |minutes: u64| start + STALL_AFTER_MS + minutes * MINUTE;

    refuse(p, &["pa"]);
    assert_eq!(
        told_now(
            PaWatchdog::default()
                .round(&services, INTERVAL, at(1))
                .await
                .unwrap()
        ),
        vec![
            (format!("{p}-pa"), INJECTED_ERROR.to_string()),
            (format!("{p}-pm"), "queued".to_string()),
        ]
    );
    refuse(p, &[]);
    let restarted = PaWatchdog::default();
    assert_eq!(
        told_now(restarted.round(&services, INTERVAL, at(2)).await.unwrap()),
        vec![(format!("{p}-pa"), "queued".to_string())],
        "after a restart, only the PA that failed"
    );
    assert!(told_now(restarted.round(&services, INTERVAL, at(3)).await.unwrap()).is_empty());
    assert_eq!(told(&services, &format!("{p}-pm")).await.len(), 1);
    let to_pa = told(&services, &format!("{p}-pa")).await;
    assert_eq!(to_pa.len(), 1);
    let stall = &services.watchdogs().modal_stalls().await.unwrap()[0];
    assert_eq!(stall.told.len(), 2);
    assert_eq!(
        to_pa[0].msg_id,
        format!("modal-stall:{p}-asking:{}:{p}-pa", stall.quiet_seq),
        "the retry keeps the stable message id"
    );
}

/// Review F001: the parent is told while no PA exists; a PA appointed later
/// is told on the next round, the parent not again.
#[tokio::test]
async fn a_target_that_appears_later_is_told_then() {
    let p = "pij-late";
    let (services, _tmux, _store, folder) =
        services(&fixture("copilot-follow-up-dialog-herring.txt")).await;
    retry_fleet(&services, &folder, p, false).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, &format!("{p}-asking"), start).await;
    let at = |minutes: u64| start + STALL_AFTER_MS + minutes * MINUTE;
    let watchdog = PaWatchdog::default();
    assert_eq!(
        told_now(watchdog.round(&services, INTERVAL, at(1)).await.unwrap()),
        vec![(format!("{p}-pm"), "queued".to_string())]
    );
    add_pa(&services, &folder, &format!("{p}-pa")).await;
    assert_eq!(
        told_now(watchdog.round(&services, INTERVAL, at(2)).await.unwrap()),
        vec![(format!("{p}-pa"), "queued".to_string())]
    );
    assert!(told_now(watchdog.round(&services, INTERVAL, at(3)).await.unwrap()).is_empty());
}

/// Review F001, bounded: a recipient that keeps failing is tried
/// `MAX_TELL_TRIES` times this stretch, then left.
#[tokio::test]
async fn failed_tells_are_bounded_per_recipient() {
    let p = "pij-bound";
    let (services, _tmux, _store, folder) =
        services(&fixture("copilot-follow-up-dialog-herring.txt")).await;
    retry_fleet(&services, &folder, p, true).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, &format!("{p}-asking"), start).await;
    refuse(p, &["pm", "pa"]);
    let watchdog = PaWatchdog::default();
    let mut tries = 0;
    for minute in 1..=8 {
        let at = start + STALL_AFTER_MS + minute * MINUTE;
        tries += told_now(watchdog.round(&services, INTERVAL, at).await.unwrap()).len();
    }
    refuse(p, &[]);
    let bound = pij_core::modal_stall::MAX_TELL_TRIES;
    assert_eq!(
        tries,
        2 * bound as usize,
        "each recipient tried MAX_TELL_TRIES times, then left"
    );
    let stall = &services.watchdogs().modal_stalls().await.unwrap()[0];
    assert!(stall.told.is_empty());
    assert_eq!(
        stall.attempts.values().copied().collect::<Vec<_>>(),
        vec![bound; 2]
    );
}

/// Review F002: a new own event re-arms by sequence, even when its clock is
/// earlier than the stretch's.
#[tokio::test]
async fn an_own_event_with_an_earlier_clock_starts_a_new_stretch() {
    let (services, _tmux, _store, folder) =
        services(&fixture("copilot-follow-up-dialog-herring.txt")).await;
    fleet(&services, &folder).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-asking", start).await;
    let watchdog = PaWatchdog::default();
    let first = start + STALL_AFTER_MS + MINUTE;
    assert_eq!(
        stalls(watchdog.round(&services, INTERVAL, first).await.unwrap()).len(),
        1
    );
    assert!(
        stalls(
            watchdog
                .round(&services, INTERVAL, first + MINUTE)
                .await
                .unwrap()
        )
        .is_empty()
    );
    own_event(&services, "pij-asking", start - MINUTE).await;
    assert_eq!(
        stalls(
            watchdog
                .round(&services, INTERVAL, first + 2 * MINUTE)
                .await
                .unwrap()
        )
        .len(),
        1,
        "a later sequence is a new stretch, whatever its clock"
    );
    assert_eq!(told(&services, "pij-pm").await.len(), 2);
}

/// Security S156001: text inside the dialog (the seat's work) never reaches a
/// stall row, a message, an audit event, the roster or the card.
#[tokio::test]
async fn the_dialogs_own_text_reaches_no_record_message_or_view() {
    const CANARY: &str = "PIJ-CANARY-S156001";
    let screen = fixture("copilot-follow-up-dialog-herring.txt")
        .replace("Copilot needs information.", CANARY)
        .replace("**#153 BLOCK**", CANARY);
    let (services, _tmux, store, folder, config) =
        services_with_config(FakeTmux::new().with_standing_capture(screen)).await;
    fleet(&services, &folder).await;
    let addr = http(&config, &store).await;
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-asking", start).await;
    let outcomes = PaWatchdog::default()
        .round(&services, INTERVAL, start + STALL_AFTER_MS + MINUTE)
        .await
        .unwrap();
    assert_eq!(stalls(outcomes).len(), 1, "still detected");
    let mut seen = format!("{:?}", services.watchdogs().modal_stalls().await.unwrap());
    for seat in ["pij-pm", "pij-pa"] {
        seen.push_str(&format!("{:?}", told(&services, seat).await));
    }
    for seat in ["pij-asking", "pij-pm", "pij-pa"] {
        seen.push_str(&format!(
            "{:?}",
            services
                .spine
                .tail(Some(&SeatId::from(seat)), pij_core::model::Seq(0))
                .await
                .unwrap()
        ));
    }
    seen.push_str(&format!("{:?}", views(addr, "pij-asking").await));
    assert!(
        seen.contains("Copilot selection dialog (ctrl+d declines)"),
        "the label is there"
    );
    assert!(!seen.contains(CANARY), "pane text leaked: {seen}");
}

/// Review F001 (round 2): a real `Refused` receipt is not acceptance. A native
/// Copilot parent whose receiver lease doesn't match its current incarnation
/// refuses the tell; once its receiver renews, the next round tells it with
/// the same message id. The PA, told on the first round, stays told once.
#[tokio::test]
async fn a_refused_tell_is_retried_until_the_native_receiver_recovers() {
    use crate::delivery::NativeInboxIdentity;
    use pij_core::model::ProcIdentity;

    let p = "pij-native";
    let (services, _tmux, _store, folder) =
        services(&fixture("copilot-follow-up-dialog-herring.txt")).await;
    retry_fleet(&services, &folder, p, true).await;
    let parent = SeatId::from(format!("{p}-pm"));
    let identity = |proc_start| NativeInboxIdentity {
        native_session: Some("native-137".into()),
        pid: Some(137),
        proc_start: Some(proc_start),
    };
    let incarnation = |proc_start| {
        let mut seat = seat(
            &format!("{p}-pm"),
            Harness::Copilot,
            &folder,
            Some("pij-prime"),
        );
        seat.proc = Some(ProcIdentity {
            pid: 137,
            proc_start,
        });
        seat.harness_session = Some("native-137".into());
        seat.native_extension_delivery = true;
        seat
    };
    // The parent's receiver renewed as one incarnation; the seat is now another.
    services.registry.put(incarnation(1370)).await.unwrap();
    services
        .delivery
        .heartbeat_native_receiver(&parent, &identity(1370), 0, 0, None)
        .await
        .unwrap();
    services.registry.put(incarnation(1371)).await.unwrap();

    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, &format!("{p}-asking"), start).await;
    let at = |minutes: u64| start + STALL_AFTER_MS + minutes * MINUTE;
    let watchdog = PaWatchdog::default();
    assert_eq!(
        told_now(watchdog.round(&services, INTERVAL, at(1)).await.unwrap()),
        vec![
            (format!("{p}-pa"), "queued".to_string()),
            (format!("{p}-pm"), "refused".to_string()),
        ]
    );
    let stall = services.watchdogs().modal_stalls().await.unwrap().remove(0);
    assert_eq!(stall.told, vec![SeatId::from(format!("{p}-pa"))]);
    assert_eq!(stall.attempts.get(&parent), Some(&1));
    let msg_id = format!("modal-stall:{p}-asking:{}:{p}-pm", stall.quiet_seq);
    let outcomes = |events: Vec<pij_core::model::Event>| -> Vec<serde_json::Value> {
        events
            .into_iter()
            .filter(|event| event.kind == "delivery.outcome")
            .map(|event| serde_json::from_str(&event.payload).unwrap())
            .collect()
    };
    let refused = outcomes(
        services
            .spine
            .tail(Some(&parent), pij_core::model::Seq(0))
            .await
            .unwrap(),
    );
    assert!(
        refused
            .iter()
            .any(|outcome| outcome["msg_id"] == msg_id.as_str()
                && outcome["outcome"]["outcome"] == "refused"),
        "{refused:?}"
    );

    // The current incarnation's receiver renews: the parent is told now, with
    // the same message id, and only the parent.
    services
        .delivery
        .heartbeat_native_receiver(&parent, &identity(1371), 1, 1, None)
        .await
        .unwrap();
    let retried = told_now(watchdog.round(&services, INTERVAL, at(2)).await.unwrap());
    assert_eq!(retried.len(), 1, "{retried:?}");
    assert_eq!(retried[0].0, format!("{p}-pm"));
    assert_ne!(retried[0].1, "refused");
    assert!(told_now(watchdog.round(&services, INTERVAL, at(3)).await.unwrap()).is_empty());
    let stall = services.watchdogs().modal_stalls().await.unwrap().remove(0);
    assert_eq!(stall.told.len(), 2);
    let latest = outcomes(
        services
            .spine
            .tail(Some(&parent), pij_core::model::Seq(0))
            .await
            .unwrap(),
    )
    .pop()
    .unwrap();
    assert_eq!(
        latest["msg_id"],
        msg_id.as_str(),
        "the retry keeps the stable id"
    );
    assert_eq!(told(&services, &format!("{p}-pa")).await.len(), 1);
}

/// Performance P156-F002: each attempt is saved BEFORE it is sent. While the
/// counter can't be saved (a store trigger aborts every write to it), nothing
/// is sent, round after round, so the attempts can't pass the bound; once it
/// can, the next round tells as usual.
#[tokio::test]
async fn a_failed_attempt_save_sends_nothing() {
    let p = "pij-counter";
    let (services, _tmux, store, folder, _) = services_with_config(
        FakeTmux::new().with_standing_capture(fixture("copilot-follow-up-dialog-herring.txt")),
    )
    .await;
    retry_fleet(&services, &folder, p, true).await;
    let pool = pij_store::open(&store.path()).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER pij_test_attempts BEFORE UPDATE OF attempts ON modal_stalls \
         BEGIN SELECT RAISE(ABORT, 'injected counter failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, &format!("{p}-asking"), start).await;
    let at = |minutes: u64| start + STALL_AFTER_MS + minutes * MINUTE;
    let watchdog = PaWatchdog::default();
    let rounds = pij_core::modal_stall::MAX_TELL_TRIES + 2;
    for minute in 1..=u64::from(rounds) {
        let tried = told_now(
            watchdog
                .round(&services, INTERVAL, at(minute))
                .await
                .unwrap(),
        );
        assert_eq!(tried.len(), 2, "both owed, round {minute}");
        assert!(
            tried
                .iter()
                .all(|(_, delivery)| delivery.starts_with("error: attempt not saved, so not sent")),
            "{tried:?}"
        );
    }
    assert!(
        told(&services, &format!("{p}-pm")).await.is_empty(),
        "nothing sent"
    );
    assert!(
        told(&services, &format!("{p}-pa")).await.is_empty(),
        "nothing sent"
    );
    let stall = services.watchdogs().modal_stalls().await.unwrap().remove(0);
    assert!(stall.attempts.is_empty() && stall.told.is_empty());

    sqlx::query("DROP TRIGGER pij_test_attempts")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        told_now(
            watchdog
                .round(&services, INTERVAL, at(u64::from(rounds) + 1))
                .await
                .unwrap()
        ),
        vec![
            (format!("{p}-pa"), "queued".to_string()),
            (format!("{p}-pm"), "queued".to_string()),
        ]
    );
    let stall = services.watchdogs().modal_stalls().await.unwrap().remove(0);
    assert_eq!(
        stall.attempts.values().copied().collect::<Vec<_>>(),
        vec![1, 1]
    );
}
