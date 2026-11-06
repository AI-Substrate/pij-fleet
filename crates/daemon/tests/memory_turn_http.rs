//! The memory turn over HTTP (plan 201, AC2): before a compact or close of a
//! seat that holds a role, pij delivers a memory prompt and holds the action
//! until the seat's turn ends, or the bound passes. A real store and bus; the
//! seat's harness is played by publishing the facts it would.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::delivery::delivery_kind;
use pij_core::model::{
    Event, Harness, Msg, PaneProcess, ProcIdentity, SeatDescriptor, SeatId, SystemState,
};
use pij_core::ports::{Queue, Registry};
use pij_daemon::events::EventBus;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_store::Assignment;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

const KEY: &str = "memory-turn-key";

struct Rig {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    _store: FreshStore,
    queue: Arc<dyn Queue>,
    registry: Arc<dyn Registry>,
    bus: Arc<EventBus>,
}

/// `prime` (pane `%prime`, a live proc) parents `holder` and `plain`; the
/// seats in `holding` hold role pm-001 in project fleet.
async fn rig(bound: Duration, holding: &[&str]) -> Rig {
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
    let signals = std::path::Path::new(&config.store_path).with_extension("signals");
    let mut services = pij_daemon::build_services(&config, &signals)
        .await
        .expect("services");
    services.memory_turn_bound = bound;
    let mut tmux = FakeTmux::new();
    let mut liveness = FakeLiveness::new();
    for (index, id) in ["prime", "holder", "plain"].into_iter().enumerate() {
        let pid = 920_001 + index as u32;
        let mut seat = SeatDescriptor::new(id, Harness::Omp, "/abs/tree");
        seat.pane = Some(format!("%{id}"));
        seat.proc = Some(ProcIdentity { pid, proc_start: 7 });
        if id != "prime" {
            seat.parent = Some("prime".into());
        }
        services.registry.put(seat).await.expect("seat");
        tmux = tmux.with_pane_process(
            &format!("%{id}"),
            PaneProcess {
                pid,
                cwd: "/abs/tree".to_string(),
            },
        );
        liveness = liveness.with_proc(ProcIdentity { pid, proc_start: 7 });
    }
    services.tmux = Arc::new(tmux);
    services.liveness = Arc::new(liveness);
    let rows: Vec<Assignment> = holding
        .iter()
        .map(|seat| Assignment {
            project: "fleet".to_string(),
            role: "pm-001".to_string(),
            instance: String::new(),
            seat: (*seat).to_string(),
            harness: Some("omp".to_string()),
            session: None,
            machine: None,
            from: "2026-11-06T00:00:00.000Z".to_string(),
            to: None,
            assigned_by: "prime".to_string(),
            released_by: None,
            why: "lead".to_string(),
            release_why: None,
            end_pending: false,
        })
        .collect();
    services
        .directory
        .store()
        .replace_assignments("fleet", &rows)
        .await
        .expect("seed holdings");
    let (queue, registry, bus) = (
        services.queue.clone(),
        services.registry.clone(),
        Arc::clone(&services.event_bus),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = router_with_config(services, HttpConfig::local(KEY.to_string()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Rig {
        addr,
        server,
        _store: store,
        queue,
        registry,
        bus,
    }
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn close(seat: &str, caller: &str) -> Value {
    json!({"seat": seat, "reason": "done", "caller": {"TMUX_PANE": format!("%{caller}")}})
}

fn compact(seat: &str, caller: &str) -> Value {
    json!({
        "from": caller, "to": {"seat": seat}, "body": "", "msg_id": format!("compact-{seat}"),
        "command": "compact", "caller": {"tmuxPane": format!("%{caller}")}
    })
}

/// The memory prompt queued for `seat`, waiting until it is there.
async fn prompt_for(rig: &Rig, seat: &str) -> Msg {
    for _ in 0..200 {
        if let Some((_, job)) = rig
            .queue
            .peek(&[delivery_kind(&seat.into())])
            .await
            .unwrap()
        {
            let msg: Msg = serde_json::from_str(&job.payload).unwrap();
            if msg.msg_id.starts_with("memory-turn-") {
                return msg;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no memory prompt reached {seat}");
}

/// Play the seat's harness: it takes the prompt, works, and goes idle.
async fn take_turn(rig: &Rig, seat: &str, prompt: &Msg) {
    let (job, claimed) = rig
        .queue
        .claim(&[delivery_kind(&seat.into())], "test-harness")
        .await
        .unwrap()
        .expect("the prompt is queued");
    let taken: Msg = serde_json::from_str(&claimed.payload).unwrap();
    assert_eq!(taken.msg_id, prompt.msg_id);
    rig.queue
        .ack(job, pij_core::model::Outcome::Done)
        .await
        .unwrap();
    rig.bus
        .publish(Event {
            seq: None,
            v: 1,
            at: 1,
            kind: "delivery.outcome".to_string(),
            seat: Some(seat.into()),
            payload: json!({
                "msg_id": prompt.msg_id,
                "outcome": {"outcome": "delivered", "origin": "reader_read"},
                "transport": "extension-stream",
            })
            .to_string(),
        })
        .await
        .unwrap();
    let id = SeatId::from(seat);
    rig.registry
        .set_activity(&id, SystemState::Working, None)
        .await
        .unwrap();
    rig.registry
        .set_activity(&id, SystemState::Idle, None)
        .await
        .unwrap();
}

/// Whether the seat's queue head is a compact control.
async fn compact_queued(rig: &Rig, seat: &str) -> bool {
    rig.queue
        .peek(&[delivery_kind(&seat.into())])
        .await
        .unwrap()
        .is_some_and(|(_, job)| {
            serde_json::from_str::<Msg>(&job.payload)
                .is_ok_and(|msg| msg.command.as_deref() == Some("compact"))
        })
}

/// A held compact's answer decodes as the CLI decodes a control answer
/// (`Envelope<Receipt>`): its own msg_id, outcome held.
fn held_compact_receipt(answer: &Value, msg_id: &str) {
    let envelope: pij_core::model::Envelope<pij_core::model::Receipt> =
        serde_json::from_value(answer.clone()).expect("a control receipt");
    let receipt = envelope.data.expect("receipt data");
    assert_eq!(receipt.msg_id, msg_id);
    assert_eq!(
        receipt.outcome,
        pij_core::model::DeliveryOutcome::Held {
            reason: "memory-turn".to_string()
        }
    );
    assert!(receipt.at > 0);
}

/// The `memory.turn` spine event for `seat`, once the held action has run.
async fn turn_event(rig: &Rig, seat: &str) -> Value {
    for _ in 0..500 {
        if let Some(event) = rig
            .bus
            .latest_matching(&seat.into(), &["memory.turn"])
            .await
            .unwrap()
        {
            return serde_json::from_str(&event.payload).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no memory.turn event for {seat}");
}

/// A revive: the same seat id, another process.
async fn revive(rig: &Rig, seat: &str) {
    let mut revived = rig.registry.get(&seat.into()).await.unwrap().unwrap();
    revived.proc = Some(ProcIdentity {
        pid: 930_001,
        proc_start: 9,
    });
    rig.registry.put(revived).await.unwrap();
}

async fn tombstoned(rig: &Rig, seat: &str) -> bool {
    rig.registry
        .get(&seat.into())
        .await
        .unwrap()
        .unwrap()
        .tombstoned_at
        .is_some()
}

#[tokio::test]
async fn a_holders_close_answers_held_at_once_and_closes_after_the_turn() {
    let rig = rig(Duration::from_secs(30), &["holder"]).await;
    let started = std::time::Instant::now();
    let held = post(rig.addr, "/v1/close", close("holder", "prime")).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "no client waits on the 30 s bound: {:?}",
        started.elapsed()
    );
    assert_eq!(held["ok"], true, "{held}");
    assert_eq!(held["data"]["held"], "memory-turn", "{held}");
    assert_eq!(held["data"]["memory_turn"]["bound_ms"], 30_000, "{held}");
    let prompt = prompt_for(&rig, "holder").await;
    assert_eq!(prompt.from.as_str(), pij_core::BG_ACTOR);
    assert!(
        prompt
            .body
            .contains("pij directory memory add pm-001 --project fleet"),
        "{}",
        prompt.body
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !tombstoned(&rig, "holder").await,
        "the close waits for the turn"
    );

    take_turn(&rig, "holder", &prompt).await;
    let turn = turn_event(&rig, "holder").await;
    assert_eq!(turn["ended"], "turn-end", "{turn}");
    assert_eq!(turn["then"], "closed", "{turn}");
    assert_eq!(turn["prompt"], prompt.msg_id.as_str());
    assert_eq!(turn["bound_ms"], 30_000);
    assert!(turn["note"].is_null(), "{turn}");
    assert!(tombstoned(&rig, "holder").await);
    rig.server.abort();
}

#[tokio::test]
async fn a_seat_holding_no_role_is_closed_and_compacted_directly() {
    let rig = rig(Duration::from_secs(30), &["holder"]).await;
    let compacted = post(rig.addr, "/v1/send", compact("plain", "prime")).await;
    assert_eq!(compacted["ok"], true, "{compacted}");
    assert!(compacted["data"]["held"].is_null(), "{compacted}");
    assert!(compact_queued(&rig, "plain").await);
    let closed = post(rig.addr, "/v1/close", close("plain", "prime")).await;
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(closed["data"]["tombstoned_at"].is_number(), "{closed}");
    assert!(closed["data"]["held"].is_null(), "{closed}");
    assert!(
        rig.bus
            .latest_matching(&"plain".into(), &["memory.turn"])
            .await
            .unwrap()
            .is_none()
    );
    rig.server.abort();
}

#[tokio::test]
async fn when_the_bound_passes_the_close_proceeds_and_says_so() {
    let rig = rig(Duration::from_millis(300), &["holder"]).await;
    let held = post(rig.addr, "/v1/close", close("holder", "prime")).await;
    assert_eq!(held["data"]["held"], "memory-turn", "{held}");
    let turn = turn_event(&rig, "holder").await;
    assert_eq!(turn["ended"], "bound", "{turn}");
    assert_eq!(turn["then"], "closed", "{turn}");
    assert!(turn["waited_ms"].as_u64().unwrap() >= 300, "{turn}");
    assert!(
        turn["note"]
            .as_str()
            .unwrap()
            .contains("proceeded without its memory turn"),
        "{turn}"
    );
    assert!(tombstoned(&rig, "holder").await);
    rig.server.abort();
}

#[tokio::test]
async fn a_parents_compact_of_a_holder_is_held_and_admitted_after_the_turn() {
    let rig = rig(Duration::from_secs(30), &["holder"]).await;
    let held = post(rig.addr, "/v1/send", compact("holder", "prime")).await;
    assert_eq!(held["data"]["held"], "memory-turn", "{held}");
    held_compact_receipt(&held, "compact-holder");
    let prompt = prompt_for(&rig, "holder").await;
    take_turn(&rig, "holder", &prompt).await;
    let turn = turn_event(&rig, "holder").await;
    assert_eq!(turn["then"], "compact admitted", "{turn}");
    assert!(
        compact_queued(&rig, "holder").await,
        "the compact follows the turn"
    );
    rig.server.abort();
}

#[tokio::test]
async fn a_seat_revived_during_its_turn_is_not_closed() {
    let rig = rig(Duration::from_secs(30), &["holder"]).await;
    let held = post(rig.addr, "/v1/close", close("holder", "prime")).await;
    assert_eq!(held["data"]["held"], "memory-turn", "{held}");
    let prompt = prompt_for(&rig, "holder").await;
    revive(&rig, "holder").await;
    take_turn(&rig, "holder", &prompt).await;
    let turn = turn_event(&rig, "holder").await;
    assert!(turn["then"].as_str().unwrap().contains("revived"), "{turn}");
    assert!(!tombstoned(&rig, "holder").await);
    rig.server.abort();
}

#[tokio::test]
async fn a_holder_closing_or_compacting_itself_is_held_until_its_turn_ends() {
    let rig = rig(Duration::from_secs(30), &["prime"]).await;
    let compacted = post(rig.addr, "/v1/send", compact("prime", "prime")).await;
    assert_eq!(compacted["ok"], true, "{compacted}");
    assert_eq!(compacted["data"]["held"], "memory-turn", "{compacted}");
    held_compact_receipt(&compacted, "compact-prime");
    let prompt = prompt_for(&rig, "prime").await;
    assert!(!compact_queued(&rig, "prime").await);
    take_turn(&rig, "prime", &prompt).await;
    assert_eq!(turn_event(&rig, "prime").await["then"], "compact admitted");
    assert!(
        compact_queued(&rig, "prime").await,
        "the compact runs once the turn ends"
    );
    let (job, _) = rig
        .queue
        .claim(&[delivery_kind(&"prime".into())], "test-harness")
        .await
        .unwrap()
        .unwrap();
    rig.queue
        .ack(job, pij_core::model::Outcome::Done)
        .await
        .unwrap();

    let closed = post(rig.addr, "/v1/close", close("prime", "prime")).await;
    assert_eq!(closed["data"]["held"], "memory-turn", "{closed}");
    assert!(closed["data"]["tombstoned_at"].is_null(), "{closed}");
    let prompt = prompt_for(&rig, "prime").await;
    take_turn(&rig, "prime", &prompt).await;
    let mut closed_after = false;
    for _ in 0..500 {
        if tombstoned(&rig, "prime").await {
            closed_after = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(closed_after, "the close lands once the turn ends");
    rig.server.abort();
}

/// Review F001: a held self-compact or self-close never acts on the
/// incarnation a revive put in the seat during the turn.
#[tokio::test]
async fn a_held_self_compact_or_close_does_not_act_on_a_revived_seat() {
    let rig = rig(Duration::from_secs(30), &["prime"]).await;
    let held = post(rig.addr, "/v1/send", compact("prime", "prime")).await;
    held_compact_receipt(&held, "compact-prime");
    let prompt = prompt_for(&rig, "prime").await;
    revive(&rig, "prime").await;
    take_turn(&rig, "prime", &prompt).await;
    let turn = turn_event(&rig, "prime").await;
    assert!(turn["then"].as_str().unwrap().contains("revived"), "{turn}");
    assert!(
        !compact_queued(&rig, "prime").await,
        "no compact for the new process"
    );
    rig.server.abort();

    let rig = rig_again().await;
    let held = post(rig.addr, "/v1/close", close("prime", "prime")).await;
    assert_eq!(held["data"]["held"], "memory-turn", "{held}");
    let prompt = prompt_for(&rig, "prime").await;
    revive(&rig, "prime").await;
    take_turn(&rig, "prime", &prompt).await;
    let turn = turn_event(&rig, "prime").await;
    assert!(turn["then"].as_str().unwrap().contains("revived"), "{turn}");
    assert!(
        !tombstoned(&rig, "prime").await,
        "the revived seat stays open"
    );
    rig.server.abort();
}

async fn rig_again() -> Rig {
    rig(Duration::from_secs(30), &["prime"]).await
}
