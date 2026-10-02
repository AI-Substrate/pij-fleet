use std::sync::Arc;
use std::time::Duration;

use pij_core::config::Config;
use pij_core::model::{Harness, Msg, ProcIdentity, SeatDescriptor};
use pij_core::ports::{Registry, Spine};
use pij_daemon::delivery::DeliveryService;
use pij_daemon::events::EventBus;
use pij_harnesses::InteractionGate;
use pij_store::{SqliteQueue, SqliteSpine, StorePool};
use pij_testkit::fakes::{FakeRegistry, FakeTmux, FakeTransport};

async fn serve(pool: StorePool) -> (String, tokio::task::JoinHandle<()>, std::path::PathBuf) {
    let dir = pij_testkit::fresh_dir("store-wedge-http");
    let mut services = pij_daemon::build_services(&Config::default(), &dir)
        .await
        .unwrap();
    let registry = Arc::new(FakeRegistry::new());
    registry
        .put(SeatDescriptor::new("pij-probe", Harness::Claude, "/probe"))
        .await
        .unwrap();
    services.registry = registry;
    services.spine = Arc::new(SqliteSpine::new(pool.clone()));
    services.status = pij_store::status::SqliteStatus::new(pool, false);
    let router = pij_daemon::http::router(services, "private-probe-key".into());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, server, dir)
}

#[tokio::test]
async fn saturated_pool_bounds_store_http_and_exposes_unhealthy_until_release() {
    let pool = pij_store::open("").await.unwrap();
    let (url, server, dir) = serve(pool.clone()).await;
    let connection = pool.acquire().await.unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let health = client
        .get(format!("{url}/health"))
        .bearer_auth("private-probe-key")
        .send()
        .await
        .unwrap();
    let health_status = health.status();
    let health_body: serde_json::Value = health.json().await.unwrap();
    let state = client
        .post(format!("{url}/v1/state"))
        .bearer_auth("private-probe-key")
        .json(&serde_json::json!({"id":"pij-probe"}))
        .send()
        .await
        .unwrap();
    eprintln!(
        "saturated health={health_status} state={} elapsed_ms={}",
        state.status(),
        started.elapsed().as_millis()
    );
    assert_eq!(
        health_status,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "{health_body}"
    );
    assert!(state.status().is_server_error());
    drop(connection);
    let recovered = client
        .get(format!("{url}/health"))
        .bearer_auth("private-probe-key")
        .send()
        .await
        .unwrap();
    assert_eq!(recovered.status(), reqwest::StatusCode::OK);
    assert_eq!(
        recovered.json::<serde_json::Value>().await.unwrap()["data"]["store"]["status"],
        "healthy"
    );
    server.abort();
    pool.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn disconnected_http_report_finishes_admitted_write_without_poisoning_pool() {
    let pool = pij_store::open("").await.unwrap();
    let (url, server, dir) = serve(pool.clone()).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_hook = entered.clone();
    let (release, receive) = std::sync::mpsc::channel();
    let mut receive = Some(receive);
    let mut connection = pool.acquire().await.unwrap();
    connection
        .lock_handle()
        .await
        .unwrap()
        .set_update_hook(move |_| {
            if let Some(receive) = receive.take() {
                entered_hook.notify_one();
                let _ = receive.recv_timeout(Duration::from_secs(5));
            }
        });
    drop(connection);
    let request_url = url.clone();
    let request = tokio::spawn(async move {
        reqwest::Client::new().post(format!("{request_url}/v1/report"))
            .bearer_auth("private-probe-key")
            .json(&serde_json::json!({"seat":"pij-probe","argv":["report","now","disconnected","proof"]}))
            .send().await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    let events = SqliteSpine::new(pool.clone())
        .tail(None, pij_core::model::Seq(0))
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "report.now");
    let health = reqwest::Client::new()
        .get(format!("{url}/health"))
        .bearer_auth("private-probe-key")
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    eprintln!("HTTP client disconnected during INSERT; one report committed; store healthy");
    server.abort();
    pool.close().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn cancelled_delivery_reservation_still_injects_once() {
    let pool = pij_store::open("").await.unwrap();
    let registry = Arc::new(FakeRegistry::new());
    let mut target = SeatDescriptor::new("pij-target", Harness::Claude, "/probe");
    target.proc = Some(ProcIdentity {
        pid: 155,
        proc_start: 1,
    });
    target.pane = Some("%155".into());
    registry.put(target.clone()).await.unwrap();
    let transport = Arc::new(FakeTransport::reachable());
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16).unwrap());
    let service = Arc::new(
        DeliveryService::new(
            registry,
            Arc::new(SqliteQueue::new(pool.clone(), 60, 100).unwrap()),
            transport.clone(),
            Arc::new(InteractionGate::new(Arc::new(FakeTmux::new()))),
            bus.clone(),
        )
        .unwrap(),
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let hook_entered = entered.clone();
    let (release, receive) = std::sync::mpsc::channel();
    let mut receive = Some(receive);
    let mut connection = pool.acquire().await.unwrap();
    connection
        .lock_handle()
        .await
        .unwrap()
        .set_update_hook(move |_| {
            if let Some(receive) = receive.take() {
                hook_entered.notify_one();
                let _ = receive.recv_timeout(Duration::from_secs(5));
            }
        });
    drop(connection);
    let msg = Msg {
        from: "pij-sender".into(),
        to: target.id,
        body: "cancel during reservation".into(),
        msg_id: "owned-reservation".into(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };
    let sender = service.clone();
    let sent = msg.clone();
    let request = tokio::spawn(async move { sender.accept(sent).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while transport.delivered().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(
        transport.delivered(),
        vec![msg.clone()],
        "committed reservation must not strand an uninjected body"
    );
    let replay = service.accept(msg.clone()).await.unwrap();
    assert!(matches!(
        replay.outcome,
        pij_core::model::DeliveryOutcome::Delivered { .. }
    ));
    assert_eq!(
        transport.delivered(),
        vec![msg],
        "retry cannot duplicate the admitted injection"
    );
    bus.flush().await;
    pool.close().await;
}

#[tokio::test]
async fn deferred_delivery_refuses_split_queue_and_spine_authority() {
    let store = pij_testkit::FreshStore::new();
    let root = pij_testkit::fresh_dir("split-deferral-proof");
    let mut config = Config {
        store_path: store.path(),
        ..Config::default()
    };
    config.adapters.queue = pij_core::config::AdapterChoice::Real;
    // Codex is never a UDS recipient, so this real adapter returns unavailable
    // without touching any Claude socket. The pane is a deterministic fake.
    config.adapters.transport = pij_core::config::AdapterChoice::Real;
    let services = pij_daemon::build_services(&config, &root).await.unwrap();
    let mut target = SeatDescriptor::new("pij-split-deferral", Harness::Codex, "/probe");
    target.proc = Some(ProcIdentity {
        pid: 155,
        proc_start: 1,
    });
    target.pane = Some("%155".into());
    services.registry.put(target.clone()).await.unwrap();
    let result = services
        .delivery
        .send("pij-sender".into(), target.id.clone(), "must remain queued")
        .await;
    assert!(
        result.is_err(),
        "split-authority deferral cannot publish a foreign cursor: {result:?}"
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("E-RS-INBOX-AUTHORITY-SPLIT")
    );
    assert!(
        services
            .queue
            .delivery_deferrals(&target.id)
            .await
            .unwrap()
            .is_empty()
    );
    let events = services
        .event_bus
        .tail(None, pij_core::model::Seq(0))
        .await
        .unwrap();
    assert!(!events.iter().any(|event| event.kind == "delivery.held"));
    assert!(
        services
            .queue
            .peek(&[pij_core::delivery::delivery_kind(&target.id)])
            .await
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir_all(root).unwrap();
}
