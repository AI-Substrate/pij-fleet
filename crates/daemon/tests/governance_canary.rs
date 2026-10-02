//! Positive fake-host proof, not a real harness/CLI canary.
//!
//! SQL, HTTP, queue claims, packet bytes, SHA acknowledgement, EventBus and
//! governance transitions are real. Only tmux/process/transport ports are
//! scripted. Pi's ready fixture exercises the real bind observer without OMP
//! catalog subprocesses. Matching fake pane/recorded pids avoid OS subtree IO.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Envelope, Harness, Pane, PaneProcess, SeatDescriptor, SeatId};
use pij_daemon::delivery::{DeliveryService, InboxClaim};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_harnesses::{HarnessRegistry, InteractionGate};
use pij_store::SqliteOrchestration;
use pij_testkit::fakes::{FakeLiveness, FakeTmux, FakeTransport};
use pij_testkit::fresh_dir;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const KEY: &str = "governance-canary-fake-host-key";
const PI_READY: &str = include_str!("../../testkit/fixtures/harnesses/pi-pane-ready.txt");

struct StateDir(PathBuf);

impl Drop for StateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn route_case<'a>(fixtures: &'a Value, id: &str) -> &'a Value {
    fixtures["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .flat_map(|route| route["cases"].as_array().expect("cases"))
        .find(|case| case["id"] == id)
        .expect("canonical route case")
}

fn event_case<'a>(fixtures: &'a Value, id: &str) -> &'a Value {
    fixtures["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|case| case["id"] == id)
        .expect("canonical event case")
}

async fn next_line(response: &mut reqwest::Response, pending: &mut Vec<u8>) -> Value {
    loop {
        if let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let value = serde_json::from_slice(&pending[..end]).expect("complete NDJSON line");
            pending.drain(..=end);
            return value;
        }
        let bytes = response
            .chunk()
            .await
            .expect("event stream read")
            .expect("event stream must stay open");
        pending.extend_from_slice(&bytes);
    }
}

async fn dispatch_event(
    response: &mut reqwest::Response,
    pending: &mut Vec<u8>,
    expected: &Value,
    dispatch_id: &str,
) -> (Value, Value) {
    loop {
        let frame = next_line(response, pending).await;
        if frame["type"] != "event" || frame["event"]["kind"] != expected["frame"]["event"]["kind"]
        {
            continue;
        }
        let payload: Value = serde_json::from_str(
            frame["event"]["payload"]
                .as_str()
                .expect("event payload is a JSON string"),
        )
        .expect("decoded event payload");
        if payload["action"] == expected["decoded_payload"]["action"]
            && payload["record"]["id"] == dispatch_id
        {
            return (frame, payload);
        }
    }
}

#[tokio::test]
async fn fake_host_http_canary_requires_real_pointer_sha_ack_and_streams_fresh_observer_pass() {
    let routes: Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .expect("canonical routes");
    let events: Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-events.json"
    ))
    .expect("canonical events");
    let dir = StateDir(fresh_dir("pij-governance-canary"));
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: dir
            .0
            .join("governance.sqlite")
            .to_string_lossy()
            .into_owned(),
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(&config, &dir.0.join("signals"))
        .await
        .expect("real SQL with fake external hosts");
    let expected_model = PI_READY
        .lines()
        .rev()
        .find_map(|line| {
            line.split_once('⬢')
                .map(|(_, model)| model.split('·').next().expect("model field").trim())
        })
        .expect("the captured Pi ready fixture names its model")
        .to_string();
    let mut worker: SeatDescriptor = serde_json::from_value(
        route_case(&routes, "register-with-role")["response"]["data"].clone(),
    )
    .expect("canonical descriptor/process seed, not a canary or ack seed");
    worker.harness = Harness::Pi;
    worker.folder = dir.0.to_string_lossy().into_owned();
    worker.model = Some(expected_model.clone());
    worker.harness_session = Some(format!("fixture-pi-native-{}", worker.id));
    let process = worker.proc.expect("canonical fixture process");
    let pane = worker.pane.clone().expect("canonical fixture pane");
    let worker_id = worker.id.clone();
    let parent_id = SeatId::from(
        routes["fixture_context"]["parent"]
            .as_str()
            .expect("parent"),
    );
    let mut parent = SeatDescriptor::new(parent_id.clone(), Harness::Pi, &worker.folder);
    parent.pane = Some(
        route_case(&routes, "canary-verified")["request"]["caller"]["TMUX_PANE"]
            .as_str()
            .expect("evaluator pane")
            .to_string(),
    );

    let tmux = Arc::new(
        FakeTmux::new()
            .with_pane(Pane {
                id: pane.clone(),
                session: "canary-fixture".into(),
                window: "pi".into(),
                title: worker_id.to_string(),
                cursor_x: None,
                cursor_y: None,
            })
            .with_pane_process(
                &pane,
                PaneProcess {
                    pid: process.pid,
                    cwd: worker.folder.clone(),
                },
            )
            .with_standing_capture(format!("{}\n{PI_READY}", worker.folder)),
    );
    let transport = Arc::new(FakeTransport::unreachable());
    services.tmux = tmux.clone();
    services.liveness = Arc::new(FakeLiveness::new().with_proc(process));
    services.harnesses = Arc::new(HarnessRegistry::real(tmux.clone()));
    services.interaction = Arc::new(InteractionGate::new(tmux.clone()));
    services.transport = transport.clone();
    // Rebuild the dependent service after replacing its host ports. Registry,
    // queue and bus Arcs are never swapped, so roles/governance retain the same
    // real pool and publisher that build_services composed.
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .expect("delivery uses the final shared Arcs"),
    );
    services
        .registry
        .put(parent)
        .await
        .expect("seed evaluator identity");
    services
        .registry
        .put(worker.clone())
        .await
        .expect("seed scripted host identity");
    let bus = services.event_bus.clone();
    let follower = tokio::spawn(services.governance.clone().follow_deliveries());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ephemeral HTTP bind");
    let addr = listener.local_addr().expect("bound address");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(services, HttpConfig::local(KEY.to_string())),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .expect("HTTP server");
    });
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let mut subscriber = client
        .get(format!("{base}/v1/events"))
        .bearer_auth(KEY)
        .send()
        .await
        .expect("real HTTP subscriber");
    assert!(subscriber.status().is_success());
    let mut pending = Vec::new();
    let hello = tokio::time::timeout(
        Duration::from_secs(5),
        next_line(&mut subscriber, &mut pending),
    )
    .await
    .expect("subscriber is attached before canary request");
    assert_eq!(hello["hello"], true);

    let mut canary = route_case(&routes, "canary-verified")["request"].clone();
    canary["caller"]["cwd"] = json!(worker.folder);
    let argv = canary["argv"].as_array_mut().expect("canary argv");
    let model_flag = argv
        .iter()
        .position(|arg| arg == "--expect-model")
        .expect("expected model flag");
    argv[model_flag + 1] = json!(expected_model);
    let wait_ms = argv
        .iter()
        .filter_map(Value::as_str)
        .find_map(|arg| arg.strip_prefix("--wait="))
        .expect("canonical bounded wait")
        .parse::<u64>()
        .expect("wait milliseconds");
    let canary_client = client.clone();
    let canary_url = format!("{base}/v1/canary");
    let canary_request = tokio::spawn(async move {
        canary_client
            .post(canary_url)
            .bearer_auth(KEY)
            .json(&canary)
            .send()
            .await
            .expect("canary HTTP request")
    });

    tokio::time::timeout(Duration::from_millis(wait_ms) + Duration::from_secs(5), async {
        let claimed = client.get(format!("{base}/v1/inbox")).bearer_auth(KEY)
            .query(&[("seat", worker_id.as_str()), ("wait", "true")])
            .send().await.expect("recipient waits for actual queued pointer");
        assert!(claimed.status().is_success());
        let claimed: Envelope<Vec<InboxClaim>> = claimed.json().await.expect("real queue claim envelope");
        assert!(claimed.ok, "{claimed:?}");
        let mut claims = claimed.data.expect("claim data");
        assert_eq!(claims.len(), 1);
        let claim = claims.remove(0);
        assert_eq!(claim.message.from, parent_id);
        assert_eq!(claim.message.to, worker_id);
        let dispatch_id = &claim.message.msg_id;
        let packet_path = claim.message.body.strip_prefix("Read packet ")
            .and_then(|body| body.split_once(". Dispatch ").map(|(path, _)| PathBuf::from(path)))
            .expect("parse the actual pushed packet pointer, not a guessed nonce/path");
        assert!(packet_path.starts_with(&dir.0), "packet must remain in this test's private state");
        let bytes = tokio::fs::read(&packet_path).await.expect("consume actual nonce packet bytes");
        let packet: Value = serde_json::from_slice(&bytes).expect("actual canary packet");
        assert_eq!(packet["kind"], "pij-canary");
        assert_eq!(packet["recipient"], worker_id.as_str());
        assert_eq!(packet["process"], serde_json::to_value(process).expect("process evidence"));
        assert_eq!(packet["session"], json!(worker.harness_session));
        let nonce = packet["nonce"].as_str().expect("real challenge nonce");
        assert!(!nonce.is_empty());
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert!(claim.message.body.contains(&format!("pij ack {dispatch_id} --packet-sha {digest}")));

        let read_receipt = client.post(format!("{base}/v1/inbox/ack")).bearer_auth(KEY)
            .json(&json!({"seat": worker_id, "job_id": claim.job_id}))
            .send().await.expect("actual ReaderRead acknowledgement");
        assert!(read_receipt.status().is_success());
        let (delivered, _) = dispatch_event(&mut subscriber, &mut pending,
            event_case(&events, "dispatch-delivered"), dispatch_id).await;
        assert!(!canary_request.is_finished(), "canary may not pass or expire before recipient SHA acknowledgement");
        let observations_before_ack = tmux.calls().len();

        let mut ack = route_case(&routes, "dispatch-ack")["request"].clone();
        ack["caller"]["cwd"] = json!(worker.folder);
        ack["argv"][1] = json!(dispatch_id);
        ack["argv"][3] = json!(digest);
        let acked = client.post(format!("{base}/v1/ack")).bearer_auth(KEY).json(&ack)
            .send().await.expect("recipient acknowledges consumed packet through HTTP");
        assert!(acked.status().is_success());
        let acked: Value = acked.json().await.expect("SHA acknowledgement envelope");
        assert_eq!(acked["data"]["dispatch"]["ack"]["packet_sha256"], digest);
        assert_eq!(acked["data"]["dispatch"]["ack"]["seat"], worker_id.as_str());
        let (ack_frame, _) = dispatch_event(&mut subscriber, &mut pending,
            event_case(&events, "dispatch-acked"), dispatch_id).await;

        let passed = canary_request.await.expect("concurrent canary request task");
        let status = passed.status();
        let passed: Value = passed.json().await.expect("canary response envelope");
        assert!(status.is_success(), "nonce-correlated ack must finish before deadline; {status}: {passed}");
        assert_eq!(passed["ok"], true, "{passed}");
        let row = &passed["data"]["dispatch"];
        assert_eq!(row["id"], dispatch_id.as_str());
        assert_eq!(row["state"], "acked");
        assert_eq!(row["packet_sha256"], digest);
        assert_eq!(row["canary"]["nonce"], nonce);
        assert_eq!(row["canary"]["model"], expected_model);
        assert_eq!(row["canary"]["evaluator"], parent_id.as_str());
        assert!(row["delivered_at"].as_u64().expect("real delivery time") > 0);
        assert!(row["canary"]["passed_at"].as_u64().expect("real verification time") > 0);
        let fresh_calls = tmux.calls();
        assert!(fresh_calls[observations_before_ack..].iter().any(|call| call.starts_with(&format!("capture:{pane}:"))),
            "fresh bind observation must inspect the scripted Pi pane AFTER receipt acknowledgement");
        assert!(!transport.calls().iter().any(|call| call.starts_with("deliver:")),
            "success must come from the actual SQL claim/read receipt, not a fake transport success");

        let (pass_frame, pass_payload) = dispatch_event(&mut subscriber, &mut pending,
            event_case(&events, "dispatch-canary-passed"), dispatch_id).await;
        assert_eq!(pass_frame["event"]["seat"], worker_id.as_str());
        assert_eq!(pass_frame["cursor"], passed["data"]["seq"]);
        assert!(delivered["cursor"].as_u64().expect("delivery seq") < ack_frame["cursor"].as_u64().expect("ack seq"));
        assert!(ack_frame["cursor"].as_u64().expect("ack seq") < pass_frame["cursor"].as_u64().expect("pass seq"));
        assert_eq!(pass_payload["record"], *row);
        let reopened = pij_store::open(&config.store_path).await.expect("reopen real SQL for persistence readback");
        let persisted = SqliteOrchestration::new(reopened.clone()).dispatch(dispatch_id)
            .await.expect("read durable dispatch").expect("dispatch survives independent reader");
        assert_eq!(serde_json::to_value(persisted).expect("durable record"), *row);
        reopened.close().await;
    }).await.expect("bounded positive fake-host canary flow");

    drop(subscriber);
    drop(client);
    stop.send(()).expect("stop owned HTTP server");
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("HTTP server shutdown")
        .expect("server task");
    follower.abort();
    let stopped = follower.await;
    assert!(
        matches!(&stopped, Err(error) if error.is_cancelled()),
        "follower must remain healthy until explicit shutdown: {stopped:?}"
    );
    bus.flush().await;
}
