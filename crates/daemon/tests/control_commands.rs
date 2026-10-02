use std::net::SocketAddr;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Envelope, Harness, Msg, PaneProcess, ProcIdentity, SeatDescriptor};
use pij_core::ports::Queue;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

async fn daemon(
    harness: Harness,
    pane: Option<&str>,
    parent: Option<&str>,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    FreshStore,
    Arc<dyn Queue>,
) {
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
    let mut services = pij_daemon::build_services(
        &config,
        std::path::Path::new("/tmp/pij-test-control-signals"),
    )
    .await
    .expect("services");
    let mut sender = SeatDescriptor::new("sender", Harness::Omp, "/abs/tree");
    sender.pane = Some("%control-sender".to_string());
    sender.proc = Some(ProcIdentity {
        pid: 910_001,
        proc_start: 7,
    });
    let mut target = SeatDescriptor::new("target", harness, "/abs/tree");
    target.pane = pane.map(str::to_string);
    target.parent = parent.map(Into::into);
    services.registry.put(sender).await.expect("sender");
    services.registry.put(target).await.expect("target");
    services.tmux = Arc::new(FakeTmux::new().with_pane_process(
        "%control-sender",
        PaneProcess {
            pid: 910_001,
            cwd: "/abs/tree".to_string(),
        },
    ));
    services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
        pid: 910_001,
        proc_start: 7,
    }));
    let queue = services.queue.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(services, HttpConfig::local("control-key".to_string())),
        )
        .await
        .expect("serve");
    });
    (addr, server, store, queue)
}

fn request(command: &str) -> Value {
    json!({
        "from": "sender", "to": {"seat":"target"}, "body":"", "msg_id":command,
        "command":command, "caller":{"tmuxPane":"%control-sender"}
    })
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Envelope<Value>) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth("control-key")
        .json(&body)
        .send()
        .await
        .expect("post");
    let status = response.status().as_u16();
    (status, response.json().await.expect("decodable envelope"))
}

#[tokio::test]
async fn control_native_send_preserves_command_and_derived_sender() {
    let (addr, server, _store, queue) = daemon(Harness::Omp, Some("%target"), None).await;
    let (status, envelope) = post(addr, "/v1/send", request("compact")).await;
    assert_eq!(status, 200, "{envelope:?}");
    let (_, job) = queue
        .peek(&[pij_core::delivery::delivery_kind(&"target".into())])
        .await
        .expect("peek")
        .expect("queued command");
    let msg: Msg = serde_json::from_str(&job.payload).expect("command message");
    assert_eq!(msg.command.as_deref(), Some("compact"));
    assert_eq!(msg.from.as_str(), "sender");
    assert!(msg.body.is_empty());
    server.abort();
}

#[tokio::test]
async fn control_native_refuses_unsupported_targets_and_invalid_shapes_without_enqueueing() {
    for (harness, pane, command, body, reason) in [
        (
            Harness::Copilot,
            Some("%target"),
            "compact",
            "",
            "E-RS-CONTROL-UNSUPPORTED: use Copilot's native user controls",
        ),
        (Harness::Omp, None, "compact", "", "E-RS-CONTROL-PANELESS"),
        (
            Harness::Omp,
            Some("%target"),
            "quit",
            "",
            "E-RS-CONTROL-INVALID",
        ),
        (
            Harness::Omp,
            Some("%target"),
            "compact",
            "do this too",
            "E-RS-CONTROL-BODY",
        ),
    ] {
        let (addr, server, _store, queue) = daemon(harness, pane, None).await;
        let mut value = request(command);
        value["body"] = json!(body);
        let (status, response) = post(addr, "/v1/send", value).await;
        assert_eq!(status, 400);
        assert!(
            response.meta.unwrap_or_default().contains(reason),
            "expected {reason}"
        );
        assert!(
            queue
                .peek(&[pij_core::delivery::delivery_kind(&"target".into())])
                .await
                .expect("peek")
                .is_none()
        );
        server.abort();
    }
}

#[tokio::test]
async fn control_destructive_ownership_requires_pane_proof_not_asserted_actor() {
    let (addr, server, _store, queue) = daemon(Harness::Omp, Some("%target"), Some("owner")).await;
    for command in ["new", "reload"] {
        let (_, response) = post(addr, "/v1/send", request(command)).await;
        assert!(!response.ok);
        assert!(
            response
                .meta
                .unwrap_or_default()
                .contains("E-RS-CONTROL-OWNERSHIP")
        );
        let mut spoofed = request(command);
        spoofed["from"] = json!("owner");
        let (_, response) = post(addr, "/v1/send", spoofed).await;
        assert!(!response.ok, "body actor must not grant authority");
        let mut assertion_only = request(command);
        assertion_only["from"] = json!("target");
        assertion_only["caller"] = json!({"pijSessionId":"target"});
        let (_, response) = post(addr, "/v1/send", assertion_only).await;
        assert!(!response.ok);
        assert!(
            response
                .meta
                .unwrap_or_default()
                .contains("E-RS-CONTROL-IDENTITY")
        );
    }
    assert!(
        queue
            .peek(&[pij_core::delivery::delivery_kind(&"target".into())])
            .await
            .expect("peek")
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn control_recorded_parent_can_send_destructive_commands() {
    for command in ["new", "reload"] {
        let (addr, server, _store, queue) =
            daemon(Harness::Omp, Some("%target"), Some("sender")).await;
        let (status, response) = post(addr, "/v1/send", request(command)).await;
        assert_eq!(status, 200, "{response:?}");
        let (_, job) = queue
            .claim(
                &[pij_core::delivery::delivery_kind(&"target".into())],
                "test",
            )
            .await
            .expect("claim")
            .expect("control");
        let msg: Msg = serde_json::from_str(&job.payload).expect("message");
        assert_eq!(msg.command.as_deref(), Some(command));
        server.abort();
    }
}

#[tokio::test]
async fn control_shim_send_and_compact_self_use_the_command_path() {
    let (addr, server, _store, queue) = daemon(Harness::Omp, Some("%target"), None).await;
    for (argv, recipient) in [
        (json!(["send", "target", "--command", "compact"]), "target"),
        (json!(["compact-self"]), "sender"),
    ] {
        let (status, response) = post(
            addr,
            "/v1/shim/send",
            json!({
                "argv":argv, "caller":{"tmuxPane":"%control-sender"}
            }),
        )
        .await;
        assert_eq!(status, 200, "{response:?}");
        let (_, job) = queue
            .peek(&[pij_core::delivery::delivery_kind(&recipient.into())])
            .await
            .expect("peek")
            .expect("queued command");
        let msg: Msg = serde_json::from_str(&job.payload).expect("message");
        assert_eq!(msg.command.as_deref(), Some("compact"));
        assert_eq!(msg.to.as_str(), recipient);
    }
    server.abort();
}

#[tokio::test]
async fn control_shim_refuses_every_nonempty_body_channel_and_spoofed_owner() {
    let (addr, server, _store, queue) = daemon(Harness::Omp, Some("%target"), None).await;
    for request in [
        json!({"argv":["send","target","--command","compact","extra"]}),
        json!({"argv":["send","target","--command","compact",""],"body_literal":"hidden extra"}),
        json!({"argv":["send","target","--command","compact","--body-file","file"],"body_literal":""}),
        json!({"argv":["send","target","--command","quit"]}),
        json!({"argv":["send","target","--command","new"],"caller":{"tmuxPane":"%control-sender","pijSessionId":"target"}}),
    ] {
        let (status, response) = post(addr, "/v1/shim/send", request).await;
        assert_eq!(status, 400, "{response:?}");
        assert!(!response.ok);
    }
    assert!(
        queue
            .peek(&[pij_core::delivery::delivery_kind(&"target".into())])
            .await
            .expect("peek")
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn control_native_strips_only_standard_empty_envelopes() {
    for body in ["[pij from sender] ", "[pij-rs from sender]\n\n[/pij]"] {
        let (addr, server, _store, queue) = daemon(Harness::Omp, Some("%target"), None).await;
        let mut value = request("compact");
        value["body"] = json!(body);
        let (status, response) = post(addr, "/v1/send", value).await;
        assert_eq!(status, 200, "{response:?}");
        let (_, job) = queue
            .peek(&[pij_core::delivery::delivery_kind(&"target".into())])
            .await
            .expect("peek")
            .expect("control");
        let msg: Msg = serde_json::from_str(&job.payload).expect("message");
        assert!(msg.body.is_empty());
        server.abort();
    }
}
