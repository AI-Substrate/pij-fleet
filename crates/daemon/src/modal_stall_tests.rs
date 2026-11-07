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
    let (services, tmux, store, folder, _) = services_with_config(screen).await;
    (services, tmux, store, folder)
}

async fn services_with_config(
    screen: &str,
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
    let tmux = Arc::new(FakeTmux::new().with_standing_capture(screen));
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
    services
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

fn captures(tmux: &FakeTmux, pane: &str) -> usize {
    let prefix = format!("capture:{pane}:");
    tmux.calls()
        .iter()
        .filter(|call| call.starts_with(&prefix))
        .count()
}

/// bp-0003: herring's screen, three rounds, a restart, a new stretch. The
/// parent and the PA are each told once per stretch; pij presses no key.
#[tokio::test]
async fn a_seat_on_a_dialog_is_told_once_per_stretch_to_its_parent_and_pa() {
    let (services, tmux, store, folder, config) =
        services_with_config(&fixture("copilot-follow-up-dialog-herring.txt")).await;
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
        body.contains("pij-asking (copilot)")
            && body.contains("Dialog: Copilot needs information.")
            && body.contains("ctrl+d declines")
            && body.contains("Never choose an option for it"),
        "{body}"
    );

    // bp-0004: pij list and the state card show it while it stands.
    let (listed, card) = views(addr, "pij-asking").await;
    assert_eq!(listed["key"], "ctrl+d", "{listed}");
    assert_eq!(listed["dialog"], "Copilot needs information.");
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
        .filter(|call| !call.starts_with("capture:"))
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
