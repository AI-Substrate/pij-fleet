//! Plan 181 (d): a Codex caller is the seat its own thread runs in, or it is
//! refused by name. The scenario is the 2026-10-26 incident: a shared Codex
//! app-server, started by `pij-parallel-bandicoot` in pane %45, gave every
//! Codex thread it hosted bandicoot's PIJ_SESSION_ID and TMUX_PANE.

use std::net::SocketAddr;
use std::sync::Arc;

use pij_core::model::{Harness, Pane, PaneProcess, SeatDescriptor};
use pij_core::ports::{Registry, SeatFilter};
use pij_testkit::fakes::{FakeQueue, FakeRegistry, FakeSpine, FakeTmux};
use serde_json::{Value, json};

use super::tests::{config, spawn, test_services};
use super::*;
use crate::codex_bind::HeldThreads;

const BANDICOOT: &str = "pij-parallel-bandicoot";
/// Bandicoot's pane: its TUI was hosted by the shared app-server, so the Codex
/// process in it holds no thread.
const SHARED: &str = "%45";
/// wild-guan's own thread, which its shell's CODEX_THREAD_ID named correctly.
const WILD_GUAN_THREAD: &str = "01a17807-3be5-7520-a3ea-2efc9a6b99df";
/// An in-process Codex seat's pane and thread (canary %122, 2026-10-28).
const OWN: &str = "%122";
const OWN_THREAD: &str = "01a17f40-5ad4-7ff3-aa69-991589587076";

fn codex_seat(id: &str, pane: &str) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(id, Harness::Codex, "/abs/initial-build");
    seat.pane = Some(pane.to_string());
    seat.spawn_id = Some(format!("s-{id}"));
    seat
}

async fn world(registry: Arc<FakeRegistry>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let mut services = test_services(
        registry,
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let mut tmux = FakeTmux::new();
    for (pane, pid) in [(SHARED, 39324), (OWN, 1533)] {
        tmux = tmux
            .with_pane(Pane {
                id: pane.to_string(),
                session: "work".to_string(),
                window: "@1".to_string(),
                title: "codex".to_string(),
                cursor_x: None,
                cursor_y: None,
            })
            .with_pane_process(
                pane,
                PaneProcess {
                    pid,
                    cwd: "/abs/initial-build".to_string(),
                },
            );
    }
    services.tmux = Arc::new(tmux);
    services.codex_threads = Arc::new(HeldThreads::of(&[(OWN, OWN_THREAD)]));
    spawn(router_with_config(services, config("key", &[]))).await
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> Value {
    let text = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth("key")
        .json(&body)
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}: {text}"))
}

/// What wild-guan's shell sent on 2026-10-26: bandicoot's identity, its own thread.
fn inherited() -> Value {
    json!({
        "pijSessionId": BANDICOOT,
        "tmuxPane": SHARED,
        "codexThreadId": WILD_GUAN_THREAD,
    })
}

const NOT_DERIVABLE: &str = "E-RS-CODEX-IDENTITY: this Codex thread \
    `01a17807-3be5-7520-a3ea-2efc9a6b99df` claims seat `pij-parallel-bandicoot`, but no Codex \
    process in pane `%45` holds a thread pij can read. A shared Codex app-server gives every \
    thread the environment of the session that started it, so PIJ_SESSION_ID and TMUX_PANE \
    can name another seat. Refusing rather than acting as that seat; nothing was written.";

fn refusal(answer: &Value) -> &str {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["meta"].as_str().expect("refusal reason")
}

#[tokio::test]
async fn a_send_carrying_another_seats_identity_is_refused_by_name() {
    let registry = Arc::new(FakeRegistry::new());
    registry.put(codex_seat(BANDICOOT, SHARED)).await.unwrap();
    registry
        .put(SeatDescriptor::new("pij-concrete-crane", Harness::Claude, "/abs/initial-build"))
        .await
        .unwrap();
    let (addr, server) = world(registry).await;

    let send = post(
        addr,
        "/v1/shim/send",
        json!({"argv": ["send", "pij-concrete-crane", "db-docs re-review (pij-wild-guan)"],
               "caller": inherited()}),
    )
    .await;
    assert_eq!(refusal(&send), NOT_DERIVABLE);
    let whoami = post(addr, "/v1/whoami", json!({"caller": inherited()})).await;
    assert_eq!(refusal(&whoami), NOT_DERIVABLE);
    server.abort();
}

/// R11: once bandicoot was closed, the not-registered refusal told the caller to
/// `pij adopt "$TMUX_PANE"`, and the adopt minted `pij-powerful-lobster` for
/// pane %45's process with no parent. Neither may happen now.
#[tokio::test]
async fn an_inherited_pane_is_never_advised_or_allowed_to_be_adopted() {
    let registry = Arc::new(FakeRegistry::new());
    let (addr, server) = world(registry.clone()).await;
    let caller = json!({"tmuxPane": SHARED, "codexThreadId": WILD_GUAN_THREAD});
    let expected = "E-RS-CODEX-IDENTITY: this Codex thread \
        `01a17807-3be5-7520-a3ea-2efc9a6b99df` claims pane `%45`, but no Codex process in \
        pane `%45` holds a thread pij can read. A shared Codex app-server gives every thread \
        the environment of the session that started it, so PIJ_SESSION_ID and TMUX_PANE can \
        name another seat. Refusing rather than acting as that seat; nothing was written.";

    let whoami = post(addr, "/v1/whoami", json!({"caller": caller})).await;
    assert_eq!(refusal(&whoami), expected);
    assert!(!refusal(&whoami).contains("pij adopt"));
    let adopt = post(
        addr,
        "/v1/adopt",
        json!({"argv": ["adopt", SHARED, "--harness", "codex"], "caller": caller}),
    )
    .await;
    assert_eq!(refusal(&adopt), expected);
    assert!(
        registry
            .list(SeatFilter::default())
            .await
            .unwrap()
            .is_empty(),
        "no stranger seat was minted"
    );
    server.abort();
}

#[tokio::test]
async fn a_seat_recording_another_thread_is_not_the_caller() {
    let registry = Arc::new(FakeRegistry::new());
    let mut bound = codex_seat(BANDICOOT, SHARED);
    bound.harness_session = Some("01a17825-e57c-7111-a7ff-a56b7dffe913".to_string());
    registry.put(bound).await.unwrap();
    let (addr, server) = world(registry).await;

    let whoami = post(addr, "/v1/whoami", json!({"caller": inherited()})).await;
    assert_eq!(
        refusal(&whoami),
        "E-RS-CODEX-IDENTITY: this Codex thread `01a17807-3be5-7520-a3ea-2efc9a6b99df` claims \
         seat `pij-parallel-bandicoot`, but that seat records thread \
         `01a17825-e57c-7111-a7ff-a56b7dffe913`. A shared Codex app-server gives every thread \
         the environment of the session that started it, so PIJ_SESSION_ID and TMUX_PANE can \
         name another seat. Refusing rather than acting as that seat; nothing was written."
    );
    server.abort();
}

/// The seat's own thread passes: recorded at bind, or held by its pane's Codex
/// process before the binder has run.
#[tokio::test]
async fn a_codex_seat_in_its_own_pane_is_itself() {
    for recorded in [None, Some(OWN_THREAD)] {
        let registry = Arc::new(FakeRegistry::new());
        let mut own = codex_seat("pij-continued-marlin", OWN);
        own.harness_session = recorded.map(str::to_string);
        registry.put(own).await.unwrap();
        let (addr, server) = world(registry).await;
        let whoami = post(
            addr,
            "/v1/whoami",
            json!({"caller": {"pijSessionId": "pij-continued-marlin", "tmuxPane": OWN,
                              "codexThreadId": OWN_THREAD}}),
        )
        .await;
        assert_eq!(whoami["ok"], true, "{recorded:?}: {whoami}");
        assert_eq!(whoami["data"]["id"], "pij-continued-marlin");
        server.abort();
    }
}

/// Outside Codex, nothing changes: no CODEX_THREAD_ID, no check.
#[tokio::test]
async fn a_caller_outside_codex_is_not_checked() {
    let registry = Arc::new(FakeRegistry::new());
    registry.put(codex_seat(BANDICOOT, SHARED)).await.unwrap();
    let (addr, server) = world(registry).await;
    let whoami = post(
        addr,
        "/v1/whoami",
        json!({"caller": {"pijSessionId": BANDICOOT, "tmuxPane": SHARED}}),
    )
    .await;
    assert_eq!(whoami["ok"], true, "{whoami}");
    server.abort();
}
