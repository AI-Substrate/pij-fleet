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
        body.contains("pij-asking (copilot) is showing Copilot's selection dialog")
            && body.contains("has had no event of its own for 21m")
            && body.contains("ctrl+d declines")
            && body.contains("Never choose an option for it"),
        "{body}"
    );

    // bp-0004: pij list and the state card show it while it stands.
    let (listed, card) = views(addr, "pij-asking").await;
    assert_eq!(listed["key"], "ctrl+d", "{listed}");
    assert_eq!(listed["dialog"], "Copilot's selection dialog");
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

/// P156-F001 (pm-001's ruling): with no live control client the detector
/// reads nothing, launches nothing, skips the round's candidates and says so.
#[tokio::test]
async fn with_no_live_control_client_the_round_is_skipped_and_recorded() {
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
    assert_eq!(
        outcomes
            .into_iter()
            .filter(|outcome| matches!(
                outcome,
                RoundOutcome::ModalStall { .. } | RoundOutcome::ModalStallSkipped { .. }
            ))
            .collect::<Vec<_>>(),
        vec![RoundOutcome::ModalStallSkipped {
            reason: super::NO_LIVE_CONTROL_CLIENT.into()
        }]
    );
    let calls = tmux.calls();
    assert!(
        calls.len() == 1 && calls[0].starts_with("capture_without_launch:"),
        "one attempt, then the round's candidates are skipped: {calls:?}"
    );
    assert!(
        services
            .watchdogs()
            .modal_stalls()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(told(&services, "pij-pa").await.is_empty());
}

/// Review F001: a failed tell doesn't consume the stretch. A recipient whose
/// tell failed is tried again next round with the same message id; one that
/// accepted is never told twice, restart or not; a target that appears later
/// is told then.
#[tokio::test]
async fn a_failed_tell_is_retried_and_an_accepted_one_is_not_repeated() {
    let (services, _tmux, store, folder, config) = services_with_config(
        FakeTmux::new().with_standing_capture(fixture("copilot-follow-up-dialog-herring.txt")),
    )
    .await;
    // Ids of their own: the refusal list is shared by every test in this binary.
    let mut asking = seat(
        "pij-retry-asking",
        Harness::Copilot,
        &folder,
        Some("pij-retry-pm"),
    );
    asking.pane = Some("%27".into());
    asking.state = SystemState::Working;
    for descriptor in [
        seat("pij-prime", Harness::Claude, &folder, None),
        seat("pij-retry-pm", Harness::Copilot, &folder, Some("pij-prime")),
        asking,
    ] {
        services.registry.put(descriptor).await.unwrap();
    }
    let start = crate::http::system_time_ms().unwrap();
    own_event(&services, "pij-retry-asking", start).await;
    let refuse = |ids: &[&str]| {
        *super::REFUSE_FOR_TEST.lock().unwrap() = ids.iter().map(|id| SeatId::from(*id)).collect();
    };
    let at = |minutes: u64| start + STALL_AFTER_MS + minutes * MINUTE;
    let told_now = |outcomes: Vec<RoundOutcome>| -> Vec<(SeatId, String)> {
        stalls(outcomes)
            .into_iter()
            .flat_map(|outcome| match outcome {
                RoundOutcome::ModalStall { told, .. } => told,
                _ => Vec::new(),
            })
            .collect()
    };

    // No PA yet, and the parent's tell fails: nobody is told, and it's retried.
    refuse(&["pij-retry-pm"]);
    let watchdog = PaWatchdog::default();
    assert_eq!(
        told_now(watchdog.round(&services, INTERVAL, at(1)).await.unwrap()),
        vec![("pij-retry-pm".into(), "error: refused by the test".into())]
    );
    assert_eq!(
        told_now(watchdog.round(&services, INTERVAL, at(2)).await.unwrap()).len(),
        1,
        "still failing, still tried"
    );
    // It recovers: told once, then never again, across a restart.
    refuse(&[]);
    assert_eq!(
        told_now(watchdog.round(&services, INTERVAL, at(3)).await.unwrap()),
        vec![("pij-retry-pm".into(), "queued".into())]
    );
    assert!(told_now(watchdog.round(&services, INTERVAL, at(4)).await.unwrap()).is_empty());
    let restarted = PaWatchdog::default();
    assert!(told_now(restarted.round(&services, INTERVAL, at(5)).await.unwrap()).is_empty());
    assert_eq!(told(&services, "pij-retry-pm").await.len(), 1);

    // A PA appears later and its first tell fails: the parent isn't told
    // again; the PA is, once it accepts.
    services
        .registry
        .put(seat(
            "pij-retry-pa",
            Harness::Claude,
            &folder,
            Some("pij-prime"),
        ))
        .await
        .unwrap();
    services
        .roles
        .assert_role(
            &"pij-prime".into(),
            &"pij-retry-pa".into(),
            Some("pa".into()),
        )
        .await
        .unwrap();
    refuse(&["pij-retry-pa"]);
    assert_eq!(
        told_now(restarted.round(&services, INTERVAL, at(6)).await.unwrap()),
        vec![("pij-retry-pa".into(), "error: refused by the test".into())]
    );
    refuse(&[]);
    assert_eq!(
        told_now(restarted.round(&services, INTERVAL, at(7)).await.unwrap()),
        vec![("pij-retry-pa".into(), "queued".into())]
    );
    assert!(told_now(restarted.round(&services, INTERVAL, at(8)).await.unwrap()).is_empty());
    assert_eq!(told(&services, "pij-retry-pm").await.len(), 0);
    let to_pa = told(&services, "pij-retry-pa").await;
    assert_eq!(to_pa.len(), 1);
    let stall = &services.watchdogs().modal_stalls().await.unwrap()[0];
    assert_eq!(
        to_pa[0].msg_id,
        format!(
            "modal-stall:pij-retry-asking:{}:pij-retry-pa",
            stall.quiet_seq
        )
    );
    let _ = (store, config);
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
        seen.contains("Copilot's selection dialog"),
        "the label is there"
    );
    assert!(!seen.contains(CANARY), "pane text leaked: {seen}");
}
