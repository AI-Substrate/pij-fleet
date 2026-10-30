//! Plan 186: `pij send <copilot-seat> --command compact` is admitted onto the
//! Copilot control kind and drained through the same pane control path Claude's
//! compact uses, while Copilot bodies stay with the native extension.
//!
//! Real registry, spine and queue; tmux is the fake, so nothing touches a pane.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::delivery::delivery_kind;
use pij_core::model::{
    Harness, Job, Pane, PaneProcess, ProcIdentity, SeatDescriptor, SeatId, SystemState,
};
use pij_core::ports::{DeliveryEnqueue, Queue, TmuxPort};
use pij_daemon::Services;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_daemon::pointer::{DrainWorker, PointerPolicy};
use pij_harnesses::InteractionGate;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::json;

const PANE: &str = "%copilot";

/// The wire kind, spelled out so this test pins it rather than echoing the
/// producer's own function back at itself.
fn control_kind(seat: &SeatId) -> String {
    format!("delivery-control:{seat}")
}

struct Fixture {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    _store: FreshStore,
    services: Services,
    tmux: Arc<FakeTmux>,
    worker: DrainWorker,
}

fn copilot(state: SystemState) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new("copilot", Harness::Copilot, "/abs/tree");
    seat.pane = Some(PANE.to_string());
    seat.proc = Some(ProcIdentity {
        pid: 920_002,
        proc_start: 9,
    });
    seat.harness_session = Some("native-186".to_string());
    seat.native_extension_delivery = true;
    seat.state = state;
    seat
}

/// A Copilot 1.0.105 frame as the plan 186 canary captured it (`tmux -L c186`),
/// with `composer` on the prompt row (row 3).
fn copilot_frame(composer: &str) -> String {
    let rule = "─".repeat(80);
    format!(
        " ● DONE\n $SCRATCH/canary/work                       Session: 13.41 AIC used\n{rule}\n{composer}\n{rule}\n Interactive · Allow All · / commands · tab next tab GPT-6.1 Sol · 922K context\n4% work ⛓ pij-c186-b • gpt-6.1-sol · xhigh · 922K context • xhigh • 38k/922k •"
    )
}

/// `draft` puts text in the Copilot pane's composer; otherwise it is blank.
async fn fixture(state: SystemState, draft: bool) -> Fixture {
    fixture_with_lease(state, draft, Config::default().claim_lease_secs).await
}

async fn fixture_with_lease(state: SystemState, draft: bool, claim_lease_secs: u64) -> Fixture {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        claim_lease_secs,
        ..Config::default()
    };
    let mut services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-test-186"))
            .await
            .expect("services");
    let mut sender = SeatDescriptor::new("sender", Harness::Omp, "/abs/tree");
    sender.pane = Some("%sender".to_string());
    sender.proc = Some(ProcIdentity {
        pid: 920_001,
        proc_start: 7,
    });
    services.registry.put(sender).await.expect("sender");
    services.registry.put(copilot(state)).await.expect("target");
    let tmux = FakeTmux::new().with_pane_process(
        "%sender",
        PaneProcess {
            pid: 920_001,
            cwd: "/abs/tree".to_string(),
        },
    );
    let tmux = if draft {
        tmux.with_pane(Pane {
            id: PANE.to_string(),
            session: "s".to_string(),
            window: "w".to_string(),
            title: "t".to_string(),
            // The caret sits at the end of the draft, as a human leaves it.
            cursor_x: Some(13),
            cursor_y: Some(3),
        })
        .with_attached_tap(PANE)
        .with_standing_capture(copilot_frame("❯ hello world"))
    } else {
        tmux
    };
    let tmux = Arc::new(tmux);
    if !draft {
        tmux.arrange_clear_composer(PANE);
    }
    services.tmux = Arc::clone(&tmux) as Arc<dyn TmuxPort>;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
        pid: 920_001,
        proc_start: 7,
    }));
    let worker = DrainWorker::new(
        Arc::clone(&services.registry),
        Arc::clone(&services.queue),
        Arc::clone(&services.transport),
        Arc::clone(&tmux) as Arc<dyn TmuxPort>,
        Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>)),
        Arc::clone(&services.event_bus),
        PointerPolicy {
            cadence: Duration::from_secs(90),
            announcement_limit: 3,
        },
    )
    .expect("worker");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let router = router_with_config(services.clone(), HttpConfig::local("key".to_string()));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    Fixture {
        addr,
        server,
        _store: store,
        services,
        tmux,
        worker,
    }
}

async fn send_compact(fixture: &Fixture) {
    let response: serde_json::Value = reqwest::Client::new()
        .post(format!("http://{}/v1/send", fixture.addr))
        .bearer_auth("key")
        .json(&json!({
            "from": "sender", "to": {"seat": "copilot"}, "body": "", "msg_id": "compact-186",
            "command": "compact", "caller": {"tmuxPane": "%sender"}
        }))
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("envelope");
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["data"]["outcome"]["outcome"], "queued",
        "{response}"
    );
}

/// A native body, queued the way the extension's inbox expects it.
async fn native_body(fixture: &Fixture) {
    let to = SeatId::from("copilot");
    let job = Job {
        kind: delivery_kind(&to),
        serial_key: to.0.clone(),
        payload: json!({"from": "sender", "to": "copilot", "body": "native only",
            "msg_id": "body-186"})
        .to_string(),
        dedupe_key: "body-186".to_string(),
        dedupe_origin: None,
        attempt: 0,
    };
    assert!(matches!(
        fixture
            .services
            .queue
            .enqueue_delivery(job)
            .await
            .expect("body"),
        DeliveryEnqueue::Queued { .. }
    ));
}

fn submissions(fixture: &Fixture) -> Vec<String> {
    fixture
        .tmux
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("submit:"))
        .collect()
}

async fn queued(queue: &Arc<dyn Queue>, kind: String) -> Option<Job> {
    queue.peek(&[kind]).await.expect("peek").map(|(_, job)| job)
}

#[tokio::test]
async fn copilot_compact_is_typed_through_the_pane_gate_and_bodies_stay_native() {
    let fixture = fixture(SystemState::Idle, false).await;
    native_body(&fixture).await;
    send_compact(&fixture).await;

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);
    assert_eq!(submissions(&fixture), [format!("submit:{PANE}:/compact")]);
    let seat = SeatId::from("copilot");
    assert!(
        queued(&fixture.services.queue, control_kind(&seat))
            .await
            .is_none()
    );
    let body = queued(&fixture.services.queue, delivery_kind(&seat))
        .await
        .expect("the body is still the extension's");
    assert_eq!((body.dedupe_key.as_str(), body.attempt), ("body-186", 0));
    fixture.server.abort();
}

#[tokio::test]
async fn copilot_compact_waits_for_a_working_seat_and_runs_once_it_is_idle() {
    let fixture = fixture(SystemState::Working, false).await;
    send_compact(&fixture).await;

    assert_eq!(fixture.worker.drain_once().await.expect("busy"), 1);
    assert!(submissions(&fixture).is_empty(), "never typed mid-turn");
    let deferrals = fixture
        .services
        .queue
        .delivery_deferrals(&"copilot".into())
        .await
        .expect("deferrals");
    assert_eq!(deferrals[0].reason, "recipient-busy");

    fixture
        .services
        .registry
        .put(copilot(SystemState::Idle))
        .await
        .expect("idle");
    // The busy retry backs off one second; the queue compares whole seconds.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    assert_eq!(fixture.worker.drain_once().await.expect("idle"), 1);
    assert_eq!(submissions(&fixture), [format!("submit:{PANE}:/compact")]);
    fixture.server.abort();
}

#[tokio::test]
async fn copilot_compact_is_never_typed_over_a_composer_draft() {
    let fixture = fixture(SystemState::Idle, true).await;
    send_compact(&fixture).await;

    assert_eq!(fixture.worker.drain_once().await.expect("held"), 1);
    assert!(submissions(&fixture).is_empty());
    assert!(
        queued(&fixture.services.queue, control_kind(&"copilot".into()))
            .await
            .is_some(),
        "the compact waits for the draft to clear"
    );
    fixture.server.abort();
}

/// Review S1: the drain claims a compact and the daemon dies before it acks.
/// The seat comes back paneless. The expired control claim must still be swept,
/// or its serial key holds every native body for that seat forever.
#[tokio::test]
async fn an_abandoned_copilot_control_claim_never_wedges_native_bodies() {
    let fixture = fixture_with_lease(SystemState::Idle, false, 1).await;
    send_compact(&fixture).await;
    let seat = SeatId::from("copilot");
    fixture
        .services
        .queue
        .claim(&[control_kind(&seat)], "pointer-drain")
        .await
        .expect("claim")
        .expect("the drain holds the compact, then the daemon dies");
    native_body(&fixture).await;
    let mut paneless = copilot(SystemState::Idle);
    paneless.pane = None;
    fixture
        .services
        .registry
        .put(paneless)
        .await
        .expect("paneless");

    tokio::time::sleep(Duration::from_millis(2_200)).await;
    let _ = fixture.worker.drain_once().await;
    let claimed = fixture
        .services
        .queue
        .claim(&[delivery_kind(&seat)], "native-cli:copilot")
        .await
        .expect("native claim")
        .map(|(_, job)| job.dedupe_key);
    assert_eq!(claimed.as_deref(), Some("body-186"));
    assert!(
        submissions(&fixture).is_empty(),
        "nothing typed into a paneless seat"
    );
    fixture.server.abort();
}

/// Review S3: Copilot publishes `idle` for a few milliseconds between steps of
/// one turn. A compact read during that blip must still wait for the turn.
#[tokio::test]
async fn copilot_compact_waits_out_an_idle_blip_inside_a_turn() {
    let fixture = fixture(SystemState::Idle, false).await;
    send_compact(&fixture).await;
    let registry = Arc::clone(&fixture.services.registry);
    let blip = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        registry
            .put(copilot(SystemState::Working))
            .await
            .expect("working again");
    });

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);
    blip.await.expect("blip");
    assert!(
        submissions(&fixture).is_empty(),
        "typed during an idle blip"
    );
    let deferrals = fixture
        .services
        .queue
        .delivery_deferrals(&"copilot".into())
        .await
        .expect("deferrals");
    assert_eq!(deferrals[0].reason, "recipient-busy");
    fixture.server.abort();
}
