use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{
    DeliveryOrigin, DeliveryOutcome, Envelope, Harness, Msg, Outcome, ProcIdentity, Receipt,
    SeatDescriptor, SeatId, SemanticState, Seq,
};
use pij_daemon::delivery::InboxClaim;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fakes::{FakeLiveness, FakeTmux, FakeTransport};
use pij_testkit::fresh_dir;

fn run_cli(state_dir: &std::path::Path, addr: &str, args: &[&str]) -> std::process::Output {
    run_cli_with_env(state_dir, addr, args, std::iter::empty::<(&str, &str)>())
}

fn run_cli_with_env<K, V>(
    state_dir: &std::path::Path,
    addr: &str,
    args: &[&str],
    envs: impl IntoIterator<Item = (K, V)>,
) -> std::process::Output
where
    K: AsRef<std::ffi::OsStr>,
    V: AsRef<std::ffi::OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([
            "--state-dir",
            state_dir.to_str().expect("UTF-8 state dir"),
            "--addr",
            addr,
            "--json",
        ])
        .args(args)
        .env_remove("PIJ_SESSION_ID")
        .env_remove("TMUX_PANE")
        .env_remove("COPILOT_AGENT_SESSION_ID")
        .env_remove("HARNESS_SESSION_ID")
        .envs(envs)
        .output()
        .expect("run shipped CLI")
}

fn run_cli_with_stdin(
    state_dir: &std::path::Path,
    addr: &str,
    args: &[&str],
    input: &str,
    envs: &[(&str, &str)],
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([
            "--state-dir",
            state_dir.to_str().expect("UTF-8 state dir"),
            "--addr",
            addr,
            "--json",
        ])
        .args(args)
        .env_remove("PIJ_SESSION_ID")
        .env_remove("TMUX_PANE")
        .env_remove("COPILOT_AGENT_SESSION_ID")
        .env_remove("HARNESS_SESSION_ID")
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shipped CLI");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for shipped CLI")
}

fn launch_env(call: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}=");
    let value = call.split_once(&marker)?.1.split('"').next()?;
    (!value.is_empty()).then(|| value.to_string())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_cli_retired_policy_requires_explicit_override_and_records_event() {
    let dir = fresh_dir("pij-retired-spawn-e2e");
    std::fs::write(dir.join("daemon.key"), "retired-test-key").expect("write client key");
    let policy = Config {
        retired_harnesses: vec![Harness::Pi],
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(&policy, &dir.join("pane-signals"))
        .await
        .expect("fake services");
    let registry = services.registry.clone();
    let spine = services.spine.clone();
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind server");
    let addr = listener.local_addr().expect("server address").to_string();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    local_key: "retired-test-key".to_string(),
                    peer_keys: Vec::new(),
                    machine_alias: "test-machine".to_string(),
                },
            ),
        )
        .await
        .expect("serve router");
    });
    let mut args = vec![
        "spawn",
        "--id",
        "pij-retired-cli",
        "--harness",
        "pi",
        "--cwd",
        dir.to_str().expect("UTF-8 cwd"),
        "--session",
        "fleet",
        "--no-wait",
    ];
    let refused = run_cli_with_env(&dir, &addr, &args, [("PIJ_RETIRED_HARNESSES", "")]);
    assert_eq!(refused.status.code(), Some(2));
    let refused: Envelope<serde_json::Value> =
        serde_json::from_slice(&refused.stdout).expect("refusal envelope");
    assert_eq!(
        refused.meta.as_deref(),
        Some("harness pi is retired on this machine; use omp (or pass --allow-retired)")
    );
    assert!(tmux.calls().is_empty());
    assert!(
        registry
            .get(&"pij-retired-cli".into())
            .await
            .expect("registry read")
            .is_none()
    );

    args.extend(["--allow-retired", "--bin", "/opt/harnesses/pi"]);
    let accepted = run_cli(&dir, &addr, &args);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stdout)
    );
    let accepted: Envelope<SeatDescriptor> =
        serde_json::from_slice(&accepted.stdout).expect("spawn envelope");
    let seat = accepted.data.expect("spawned descriptor");
    assert_eq!(seat.harness, Harness::Pi);
    let calls = tmux.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("/opt/harnesses/pi"));
    let events: Vec<_> = spine
        .tail(Some(&seat.id), Seq(0))
        .await
        .expect("spine read")
        .into_iter()
        .filter(|event| event.kind == "spawn.retired-harness-override")
        .collect();
    assert_eq!(events.len(), 1);
    let payload: serde_json::Value =
        serde_json::from_str(&events[0].payload).expect("override payload");
    assert_eq!(payload["harness"], "pi");
    assert_eq!(payload["allow_retired"], true);
    assert_eq!(
        payload["spawn_id"],
        seat.spawn_id.expect("spawn correlator")
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_spawn_revive_and_registration_do_not_attest_native_delivery() {
    let dir = fresh_dir("pij-spawn-e2e");
    std::fs::write(dir.join("daemon.key"), "spawn-test-key").expect("write client key");
    let config = Config {
        store_path: dir
            .join("pij.sqlite")
            .to_str()
            .expect("UTF-8 database path")
            .to_string(),
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            ..Adapters::default()
        },
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(&config, &dir.join("pane-signals"))
        .await
        .expect("coherent real registry and spine");
    let registry = services.registry.clone();
    let tmux = Arc::new(FakeTmux::new());
    let spine = services.spine.clone();
    let first_identity = ProcIdentity {
        pid: 4242,
        proc_start: 20260830112233,
    };
    let successor_identity = ProcIdentity {
        pid: 4343,
        proc_start: 20260830112234,
    };
    services.tmux = tmux.clone();
    services.liveness = Arc::new(
        FakeLiveness::new()
            .with_proc(first_identity)
            .with_proc(successor_identity),
    );
    services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
        services.tmux.clone(),
        std::time::Duration::from_millis(Config::default().interaction_idle_ms),
        pij_daemon::http::resolve_typing_grace_ms(),
    ));
    services.delivery = Arc::new(
        pij_daemon::delivery::DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .expect("retargeted delivery"),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind server");
    let addr = listener.local_addr().expect("server address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    local_key: "spawn-test-key".to_string(),
                    peer_keys: Vec::new(),
                    machine_alias: "test-machine".to_string(),
                },
            ),
        )
        .await
        .expect("serve router");
    });
    let addr = addr.to_string();

    let spawn = |id: &str, harness: &str, flag: Option<&str>| {
        let mut args = vec![
            "spawn",
            "--id",
            id,
            "--harness",
            harness,
            "--bin",
            "/path with spaces/agent",
            "--model",
            "provider/model",
            "--effort",
            "high",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
            "--no-wait",
        ];
        if let Some(flag) = flag {
            args.push(flag);
        }
        let output = run_cli(&dir, &addr, &args);
        assert!(
            output.status.success(),
            "spawn failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("spawn JSON value");
        assert_eq!(json["data"]["dispatched"], true);
        assert_eq!(json["data"]["bound"], false);
        assert!(json["data"]["pane"].as_str().is_some());
        assert!(json["data"].get("pid").is_none());
        let envelope: Envelope<SeatDescriptor> =
            serde_json::from_slice(&output.stdout).expect("spawn JSON envelope");
        envelope.data.expect("spawn descriptor")
    };

    let closed = spawn("pij-e2e-closed", "claude", Some("--no-accept-inbound"));
    let accepted = spawn("pij-e2e-open", "claude", Some("--accept-inbound"));
    let defaulted = spawn("pij-e2e-default", "claude", None);
    assert_eq!(closed.cross_session_inbound_accept, Some(false));
    assert_eq!(accepted.cross_session_inbound_accept, Some(true));
    assert_eq!(
        defaulted.cross_session_inbound_accept,
        Some(true),
        "Claude seats accept inbound by default (Jordan's ruling, plan 116)"
    );
    assert!(!closed.native_extension_delivery);
    assert!(!accepted.native_extension_delivery);

    let first = spawn("pij-e2e-native", "copilot", None);
    assert_eq!(first.proc, None, "spawn writes a pre-bind row");
    assert!(
        !first.native_extension_delivery,
        "launch intent is not native registration"
    );
    let calls = tmux.calls();
    assert_eq!(calls.len(), 4);
    assert!(!calls[0].contains("--ui-server"));
    assert!(calls[1].contains(r#"{\"crossSessionInbound\":\"accept\"}"#));
    assert!(
        calls[2].contains(r#"{\"crossSessionInbound\":\"accept\"}"#),
        "default-path claude spawn must emit the setting in argv, not just the descriptor stamp"
    );
    assert!(!calls[3].contains("--ui-server"));
    assert!(!calls[3].contains("--port"));
    let mut postmortem = first.clone();
    postmortem.role = Some("reviewer".to_string());
    postmortem.semantic_state = Some(SemanticState::Done);
    postmortem.proc = Some(first_identity);
    postmortem.harness_session = Some("00000000-0000-4000-8000-000000000137".to_string());
    postmortem.native_extension_delivery = true;
    registry
        .put(postmortem)
        .await
        .expect("stamp displaced post-mortem facts");

    let live_collision = run_cli(
        &dir,
        &addr,
        &[
            "spawn",
            "--id",
            "pij-e2e-native",
            "--harness",
            "copilot",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
        ],
    );
    assert!(
        !live_collision.status.success(),
        "a live id must still refuse"
    );
    assert_eq!(tmux.calls().len(), 4, "live collision launches nothing");

    registry
        .tombstone(&first.id, "test transition")
        .await
        .expect("tombstone first incarnation");
    assert!(
        !registry
            .get(&first.id)
            .await
            .expect("read tombstone")
            .expect("post-mortem row")
            .native_extension_delivery,
        "tombstone clearing is an observed act"
    );
    let revived = spawn("pij-e2e-native", "copilot", None);
    assert!(
        !revived.native_extension_delivery,
        "revive requires a fresh native attestation"
    );
    assert_ne!(
        revived.spawn_id, first.spawn_id,
        "revive must mint a fresh correlator"
    );
    assert_eq!(revived.proc, None, "revive writes a pre-bind row");
    assert!(
        !registry
            .get(&revived.id)
            .await
            .expect("later registry pass")
            .expect("revived row")
            .native_extension_delivery,
        "nothing restores the previous incarnation's capability on a later pass"
    );
    let revive_event = spine
        .tail(Some(&first.id), Seq(0))
        .await
        .expect("read revive history")
        .into_iter()
        .find(|event| event.kind == "seat.revive-postmortem")
        .expect("revive must preserve the displaced post-mortem");
    let postmortem_payload: serde_json::Value =
        serde_json::from_str(&revive_event.payload).expect("post-mortem payload");
    assert_eq!(postmortem_payload["reason"], "test transition");
    assert_eq!(postmortem_payload["role"], "reviewer");
    assert_eq!(postmortem_payload["semantic_state"], "done");

    let register_same = |spawn_id: Option<&str>| {
        let mut args = vec![
            "register",
            "pij-e2e-native",
            "--harness",
            "copilot",
            "--folder",
            "/abs/tree",
            "--pane",
            "%42",
            "--pid",
            "4242",
            "--proc-start",
            "20260830112233",
        ];
        if let Some(spawn_id) = spawn_id {
            args.extend(["--spawn-id", spawn_id]);
        }
        run_cli(&dir, &addr, &args)
    };

    let revive_launch = tmux.calls().last().cloned().expect("revive launch call");
    let launched_spawn_id = launch_env(&revive_launch, "PIJ_SPAWN_ID");
    let first_bind = register_same(launched_spawn_id.as_deref());
    assert!(
        first_bind.status.success(),
        "matching first registration: {}",
        String::from_utf8_lossy(&first_bind.stderr)
    );
    let first_bind: Envelope<SeatDescriptor> =
        serde_json::from_slice(&first_bind.stdout).expect("first-bind envelope");
    let first_bound = first_bind.data.expect("first-bind descriptor");
    assert_eq!(first_bound.proc, Some(first_identity));
    assert!(
        !first_bound.native_extension_delivery,
        "ordinary CLI registration binds the process but cannot attest a native extension"
    );
    assert!(
        launched_spawn_id.is_some(),
        "the shipped launcher must hand the correlator to the process that binds"
    );

    for pass in 1..=2 {
        let registered = register_same(None);
        assert!(
            registered.status.success(),
            "unmatched registration pass {pass}: {}",
            String::from_utf8_lossy(&registered.stderr)
        );
        let envelope: Envelope<SeatDescriptor> =
            serde_json::from_slice(&registered.stdout).expect("registration envelope");
        assert!(
            !envelope
                .data
                .expect("registered descriptor")
                .native_extension_delivery,
            "re-registering through the CLI must not invent a native extension capability"
        );
    }
    assert!(
        !registry
            .get(&revived.id)
            .await
            .expect("read after repeated registration")
            .expect("registered row")
            .native_extension_delivery,
        "repeated ordinary registration keeps native delivery closed"
    );

    let mut predecessor = SeatDescriptor::new("pij-e2e-predecessor", Harness::Copilot, "/abs/tree");
    predecessor.proc = Some(successor_identity);
    predecessor.harness_session = Some("00000000-0000-4000-8000-000000000138".to_string());
    predecessor.native_extension_delivery = true;
    registry
        .put(predecessor.clone())
        .await
        .expect("seed predecessor");
    let superseded = run_cli(
        &dir,
        &addr,
        &[
            "register",
            "pij-e2e-successor",
            "--supersedes",
            "pij-e2e-predecessor",
            "--harness",
            "copilot",
            "--folder",
            "/abs/tree",
            "--pane",
            "%43",
            "--pid",
            "4343",
            "--proc-start",
            "20260830112234",
        ],
    );
    assert!(
        !superseded.status.success(),
        "ordinary register cannot supersede a native Copilot session: {}",
        String::from_utf8_lossy(&superseded.stdout)
    );
    let retired = registry
        .get(&predecessor.id)
        .await
        .expect("read predecessor")
        .expect("predecessor post-mortem");
    assert_eq!(
        retired, predecessor,
        "unattested supersedes preserves native ownership"
    );
    assert!(
        registry
            .get(&SeatId::from("pij-e2e-successor"))
            .await
            .expect("read refused successor")
            .is_none()
    );

    server.abort();
    drop(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_cli_native_inbox_uses_pane_session_ladder_and_acknowledges() {
    let dir = fresh_dir("pij-native-inbox-e2e");
    std::fs::write(dir.join("daemon.key"), "native-inbox-key").unwrap();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: dir.join("pij.sqlite").display().to_string(),
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(&config, &dir.join("pane-signals"))
        .await
        .unwrap();
    let mut seat = SeatDescriptor::new("pij-native-reader", Harness::Copilot, "/abs/tree");
    seat.pane = Some("%native-reader".into());
    seat.proc = Some(ProcIdentity {
        pid: 75739,
        proc_start: 1234,
    });
    seat.harness_session = Some("native-cli-session".into());
    seat.native_extension_delivery = true;
    services.registry.put(seat.clone()).await.unwrap();
    services.liveness = Arc::new(FakeLiveness::new().with_proc(seat.proc.unwrap()));
    let sent = services
        .delivery
        .send(
            "pij-peer".into(),
            seat.id.clone(),
            "recover with the shipped CLI",
        )
        .await
        .unwrap();
    let queue = services.queue.clone();
    let delivery = services.delivery.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    local_key: "native-inbox-key".into(),
                    peer_keys: Vec::new(),
                    machine_alias: "native-test".into(),
                },
            ),
        )
        .await
        .unwrap();
    });
    let wrong = run_cli_with_env(
        &dir,
        &addr,
        &["inbox"],
        [
            ("TMUX_PANE", "%native-reader"),
            ("COPILOT_AGENT_SESSION_ID", "wrong-session"),
        ],
    );
    assert!(
        !wrong.status.success(),
        "a different conversation cannot read this pane's mail"
    );
    let read = run_cli_with_env(
        &dir,
        &addr,
        &["inbox"],
        [
            ("TMUX_PANE", "%native-reader"),
            ("COPILOT_AGENT_SESSION_ID", "native-cli-session"),
        ],
    );
    assert!(
        read.status.success(),
        "manual native inbox: {}",
        String::from_utf8_lossy(&read.stdout)
    );
    let body: Envelope<Vec<Msg>> = serde_json::from_slice(&read.stdout).unwrap();
    assert!(
        body.ok && body.meta.is_none(),
        "read must also acknowledge: {:?}",
        body.meta
    );
    let messages = body.data.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].msg_id, sent.msg_id);
    assert_eq!(messages[0].body, "recover with the shipped CLI");
    assert!(
        queue
            .peek(&[pij_core::delivery::delivery_kind(&seat.id)])
            .await
            .unwrap()
            .is_none()
    );
    delivery
        .heartbeat_native_receiver(
            &seat.id,
            &pij_daemon::delivery::NativeInboxIdentity {
                native_session: seat.harness_session.clone(),
                pid: seat.proc.map(|proc| proc.pid),
                proc_start: seat.proc.map(|proc| proc.proc_start),
            },
            1,
            1,
        )
        .await
        .unwrap();
    delivery
        .send(
            "pij-peer".into(),
            seat.id.clone(),
            "live receiver owns this",
        )
        .await
        .unwrap();
    let before = queue
        .peek(&[pij_core::delivery::delivery_kind(&seat.id)])
        .await
        .unwrap();
    let live = run_cli_with_env(
        &dir,
        &addr,
        &["inbox"],
        [
            ("TMUX_PANE", "%native-reader"),
            ("COPILOT_AGENT_SESSION_ID", "native-cli-session"),
        ],
    );
    assert!(
        !live.status.success(),
        "manual CLI cannot compete with a live receiver"
    );
    let refusal: serde_json::Value = serde_json::from_slice(&live.stdout).unwrap();
    assert_eq!(refusal["ok"], false);
    assert_eq!(refusal["details"]["retryable"], true);
    assert!(
        refusal["details"]["expires_in_ms"]
            .as_u64()
            .is_some_and(|ms| ms > 0)
    );
    assert_eq!(
        queue
            .peek(&[pij_core::delivery::delivery_kind(&seat.id)])
            .await
            .unwrap(),
        before
    );
    server.abort();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_cli_registers_sends_reads_acks_and_does_not_read_twice() {
    let dir = fresh_dir("pij-inbox-e2e");
    std::fs::write(dir.join("daemon.key"), "inbox-test-key").expect("write client key");
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: dir.join("pij.sqlite").display().to_string(),
        ..Config::default()
    };
    let mut services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-test-pane-signals"))
            .await
            .expect("build real inbox services");
    services.liveness = Arc::new(
        FakeLiveness::new()
            .with_proc(ProcIdentity {
                pid: 4242,
                proc_start: 20260830112233,
            })
            .with_proc(ProcIdentity {
                pid: 4343,
                proc_start: 20260830112234,
            }),
    );
    services.transport = Arc::new(FakeTransport::unreachable());
    services.delivery = Arc::new(
        pij_daemon::delivery::DeliveryService::new(
            Arc::clone(&services.registry),
            Arc::clone(&services.queue),
            Arc::clone(&services.transport),
            // The politeness gate now sits on the DIRECT path too, so rebuilding
            // the service carries it — a rebuild that dropped it would test a
            // delivery service the daemon does not compose.
            Arc::new(pij_harnesses::InteractionGate::new(Arc::clone(
                &services.tmux,
            ))),
            Arc::clone(&services.event_bus),
        )
        .expect("rebuild delivery from pull-only transport"),
    );
    let spine = Arc::clone(&services.spine);
    let queue = Arc::clone(&services.queue);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind server");
    let addr = listener.local_addr().expect("server address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    local_key: "inbox-test-key".to_string(),
                    peer_keys: Vec::new(),
                    machine_alias: "test-machine".to_string(),
                },
            ),
        )
        .await
        .expect("serve router");
    });
    let addr = addr.to_string();

    let registered = run_cli(
        &dir,
        &addr,
        &[
            "register",
            "pij-reader",
            "--harness",
            "omp",
            "--folder",
            "/abs/tree",
            "--pane",
            "%77",
            "--pid",
            "4242",
            "--proc-start",
            "20260830112233",
        ],
    );
    assert!(
        registered.status.success(),
        "register: {}",
        String::from_utf8_lossy(&registered.stdout)
    );

    let sender_registered = run_cli(
        &dir,
        &addr,
        &[
            "register",
            "pij-sender",
            "--harness",
            "omp",
            "--folder",
            "/abs/tree",
            "--pane",
            "%88",
            "--pid",
            "4343",
            "--proc-start",
            "20260830112234",
        ],
    );
    assert!(
        sender_registered.status.success(),
        "register sender: {}",
        String::from_utf8_lossy(&sender_registered.stdout)
    );

    let mismatched = run_cli_with_env(
        &dir,
        &addr,
        &[
            "send",
            "--from",
            "pij-reader",
            "--to",
            "pij-reader",
            "--body",
            "must not send",
        ],
        [("PIJ_SESSION_ID", "pij-sender")],
    );
    assert!(!mismatched.status.success());
    let mismatch: Envelope<serde_json::Value> =
        serde_json::from_slice(&mismatched.stdout).expect("mismatch refusal");
    let reason = mismatch.meta.expect("mismatch reason");
    assert!(reason.contains("pij-sender"), "{reason}");
    assert!(reason.contains("pij-reader"), "{reason}");

    let missing_body = run_cli_with_env(
        &dir,
        &addr,
        &["send", "--to", "pij-reader"],
        [("PIJ_SESSION_ID", "pij-sender")],
    );
    assert!(!missing_body.status.success());
    assert!(
        missing_body.stderr.is_empty(),
        "missing body must not print usage"
    );
    let missing_body: Envelope<serde_json::Value> =
        serde_json::from_slice(&missing_body.stdout).expect("missing-body refusal");
    let reason = missing_body.meta.expect("missing-body reason");
    assert!(reason.contains("--body"), "{reason}");
    assert!(reason.contains("--body-file"), "{reason}");

    let missing_identity = run_cli(&dir, &addr, &["send", "--to", "pij-reader", "--body", "x"]);
    assert!(!missing_identity.status.success());
    let missing_identity: Envelope<serde_json::Value> =
        serde_json::from_slice(&missing_identity.stdout).expect("missing-identity refusal");
    let reason = missing_identity.meta.expect("missing-identity reason");
    assert!(reason.contains("PIJ_SESSION_ID"), "{reason}");
    assert!(reason.contains("TMUX_PANE"), "{reason}");
    let contradictory_identity = run_cli_with_env(
        &dir,
        &addr,
        &["send", "--to", "pij-reader", "--body", "must not send"],
        [("PIJ_SESSION_ID", "pij-sender"), ("TMUX_PANE", "%77")],
    );
    assert!(
        !contradictory_identity.status.success(),
        "an asserted id must not outrank the observable pane"
    );
    let contradiction: Envelope<serde_json::Value> =
        serde_json::from_slice(&contradictory_identity.stdout).expect("identity refusal");
    let reason = contradiction.meta.expect("identity refusal reason");
    assert!(reason.contains("pij-sender"), "{reason}");
    assert!(reason.contains("pij-reader"), "{reason}");
    let contradictory_report = run_cli_with_env(
        &dir,
        &addr,
        &["report", "now", "did", "next"],
        [("PIJ_SESSION_ID", "pij-sender"), ("TMUX_PANE", "%77")],
    );
    assert!(
        !contradictory_report.status.success(),
        "report must share the same anti-impersonation ladder"
    );
    let contradiction: Envelope<serde_json::Value> =
        serde_json::from_slice(&contradictory_report.stdout).expect("report identity refusal");
    let reason = contradiction.meta.expect("report identity refusal reason");
    assert!(reason.contains("pij-sender"), "{reason}");
    assert!(reason.contains("pij-reader"), "{reason}");
    let contradictory_telegram = run_cli_with_env(
        &dir,
        &addr,
        &["sidecar", "telegram", "--body", "must not send"],
        [("PIJ_SESSION_ID", "pij-sender"), ("TMUX_PANE", "%77")],
    );
    assert!(
        !contradictory_telegram.status.success(),
        "Telegram replies must not bind to an asserted foreign seat"
    );
    assert!(
        contradictory_telegram.stderr.is_empty(),
        "identity refusal must not be replaced by a usage dump"
    );
    let contradiction: Envelope<serde_json::Value> =
        serde_json::from_slice(&contradictory_telegram.stdout).expect("Telegram identity refusal");
    let reason = contradiction
        .meta
        .expect("Telegram identity refusal reason");
    assert!(reason.contains("pij-sender"), "{reason}");
    assert!(reason.contains("pij-reader"), "{reason}");

    let sent = run_cli_with_env(
        &dir,
        &addr,
        &[
            "send",
            "--to",
            "pij-reader",
            "--body",
            "hello through the shipped binary",
        ],
        [("PIJ_SESSION_ID", "pij-sender")],
    );
    assert!(
        sent.status.success(),
        "send: {}",
        String::from_utf8_lossy(&sent.stdout)
    );
    let sent: Envelope<Receipt> = serde_json::from_slice(&sent.stdout).expect("send receipt");
    let receipt_meta = sent.meta.as_deref().expect("derived identity receipt");
    let minted_msg_id = sent.data.expect("receipt data").msg_id;
    assert!(receipt_meta.contains("pij-sender"), "{receipt_meta}");
    assert!(receipt_meta.contains(&minted_msg_id), "{receipt_meta}");
    assert_eq!(
        uuid::Uuid::parse_str(&minted_msg_id)
            .expect("UUIDv7")
            .get_version_num(),
        7
    );

    let refused = run_cli_with_env(
        &dir,
        &addr,
        &["inbox", "--seat", "pij-reader"],
        [("PIJ_SESSION_ID", "pij-sender")],
    );
    assert!(
        !refused.status.success(),
        "cross-seat destructive read must refuse"
    );
    let refusal: Envelope<serde_json::Value> =
        serde_json::from_slice(&refused.stdout).expect("named refusal");
    let reason = refusal.meta.expect("refusal reason");
    assert!(reason.contains("pij-sender"), "{reason}");
    assert!(reason.contains("pij-reader"), "{reason}");

    let peeked = run_cli(&dir, &addr, &["inbox", "--seat", "pij-reader", "--peek"]);
    assert!(
        peeked.status.success(),
        "peek: {}",
        String::from_utf8_lossy(&peeked.stdout)
    );
    let peeked: Envelope<Vec<InboxClaim>> =
        serde_json::from_slice(&peeked.stdout).expect("peek JSON");
    assert_eq!(
        peeked.data.expect("peek data")[0].message.body,
        "hello through the shipped binary"
    );

    let read = run_cli_with_env(&dir, &addr, &["inbox"], [("TMUX_PANE", "%77")]);
    assert!(
        read.status.success(),
        "inbox: {}",
        String::from_utf8_lossy(&read.stdout)
    );
    let read: Envelope<Vec<Msg>> = serde_json::from_slice(&read.stdout).expect("inbox JSON");
    let messages = read.data.expect("inbox data");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].body, "hello through the shipped binary");

    let second = run_cli_with_env(&dir, &addr, &["inbox"], [("PIJ_SESSION_ID", "pij-reader")]);
    let second: Envelope<Vec<Msg>> =
        serde_json::from_slice(&second.stdout).expect("second inbox JSON");
    assert!(second.data.expect("second inbox data").is_empty());

    let duplicate = run_cli_with_env(
        &dir,
        &addr,
        &[
            "send",
            "--from",
            "pij-sender",
            "--to",
            "pij-reader",
            "--body",
            "hello through the shipped binary",
            "--msg-id",
            &minted_msg_id,
        ],
        [("PIJ_SESSION_ID", "pij-sender")],
    );
    let duplicate: Envelope<Receipt> =
        serde_json::from_slice(&duplicate.stdout).expect("duplicate receipt");
    assert_eq!(
        duplicate.data.expect("duplicate data").outcome,
        DeliveryOutcome::Delivered {
            origin: DeliveryOrigin::ReaderRead
        }
    );
    let audits = spine
        .tail(Some(&SeatId::from("pij-reader")), Seq(0))
        .await
        .expect("spine audit");
    assert!(audits.iter().any(|event| {
        event.kind == "delivery.inbox-ack"
            && event.payload.contains(r#""authenticated_machine":"local""#)
            && event.payload.contains(r#""evidence_grade":"machine""#)
    }));

    let file_body = "f".repeat(5 * 1024);
    let body_path = dir.join("body.txt");
    std::fs::write(&body_path, &file_body).expect("write body file");
    let file_sent = run_cli_with_env(
        &dir,
        &addr,
        &[
            "send",
            "--to",
            "pij-reader",
            "--body-file",
            body_path.to_str().expect("UTF-8 body path"),
        ],
        [("TMUX_PANE", "%88")],
    );
    assert!(
        file_sent.status.success(),
        "file send: {}",
        String::from_utf8_lossy(&file_sent.stdout)
    );
    let file_read = run_cli_with_env(&dir, &addr, &["inbox"], [("PIJ_SESSION_ID", "pij-reader")]);
    let file_read: Envelope<Vec<Msg>> =
        serde_json::from_slice(&file_read.stdout).expect("file read");
    assert_eq!(file_read.data.expect("file message")[0].body, file_body);

    let stdin_body = "--not-a-flag body with $(danger) and `backticks`";
    let stdin_sent = run_cli_with_stdin(
        &dir,
        &addr,
        &["send", "--to", "pij-reader", "--body-file", "-"],
        stdin_body,
        &[("PIJ_SESSION_ID", "pij-sender"), ("TMUX_PANE", "%88")],
    );

    assert!(
        stdin_sent.status.success(),
        "stdin send: {}",
        String::from_utf8_lossy(&stdin_sent.stdout)
    );
    let stdin_read = run_cli_with_env(&dir, &addr, &["inbox"], [("PIJ_SESSION_ID", "pij-reader")]);
    let stdin_read: Envelope<Vec<Msg>> =
        serde_json::from_slice(&stdin_read.stdout).expect("stdin read");
    assert_eq!(stdin_read.data.expect("stdin message")[0].body, stdin_body);

    let telegram_body =
        "First line: \"quoted\" and 'single'\n$(not-a-command) `literal` \\\nUnicode: café 🙂\n"
            .repeat(80);
    std::fs::write(&body_path, &telegram_body).expect("Telegram body file");
    for input in [body_path.to_str().expect("body path"), "-"] {
        let sent = run_cli_with_stdin(
            &dir,
            &addr,
            &["sidecar", "telegram", "--body-file", input],
            if input == "-" { &telegram_body } else { "" },
            &[("PIJ_SESSION_ID", "pij-sender"), ("TMUX_PANE", "%88")],
        );
        assert!(
            sent.status.success(),
            "Telegram body-file: stdout={} stderr={}",
            String::from_utf8_lossy(&sent.stdout),
            String::from_utf8_lossy(&sent.stderr)
        );
        let (id, job) = queue
            .claim(&["sidecar:telegram:send".into()], "assert-telegram-body")
            .await
            .expect("claim")
            .expect("outbound Telegram job");
        let payload: serde_json::Value = serde_json::from_str(&job.payload).expect("request");
        assert_eq!(
            payload["body"].as_str().expect("body").as_bytes(),
            telegram_body.as_bytes()
        );
        assert_eq!(payload["from"], "pij-sender");
        queue
            .ack(id, Outcome::Done)
            .await
            .expect("finish inspected job");
    }

    server.abort();
    drop(dir);
}

/// The SHIPPED `pij-rs state <id>` reads back a seat the SHIPPED `pij-rs
/// register` wrote — through the real sqlite store, the real router, and the
/// real binary (plan 114, u-readback; ac-1142's readback half).
///
/// The in-crate HTTP tests prove the handler. This proves the parts nothing else
/// exercises: `Client::state`, the clap subcommand, and the fact that what the
/// registry actually persisted is what comes back — not what a fake was handed.
// MULTI-THREAD, like the two shipped-CLI tests above it. `run_cli` is a
// synchronous blocking `Command::output()`, so on the default current_thread
// runtime it blocks the only worker and `axum::serve` is never scheduled — the
// real binary then waits forever on a server that cannot answer. The bare
// `#[tokio::test]` here deadlocked rather than failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_cli_reads_one_seat_state_back_from_the_real_store() {
    let dir = fresh_dir("pij-state-e2e");
    std::fs::write(dir.join("daemon.key"), "state-test-key").expect("write client key");
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: dir.join("pij.sqlite").display().to_string(),
        ..Config::default()
    };
    let mut services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-test-pane-signals"))
            .await
            .expect("build real state services");
    services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
        pid: 5150,
        proc_start: 20260901090000,
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind server");
    let addr = listener.local_addr().expect("server address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    local_key: "state-test-key".to_string(),
                    peer_keys: Vec::new(),
                    machine_alias: "test-machine".to_string(),
                },
            ),
        )
        .await
        .expect("serve router");
    });
    let addr = addr.to_string();

    let registered = run_cli(
        &dir,
        &addr,
        &[
            "register",
            "pij-readback",
            "--harness",
            "claude",
            "--folder",
            "/abs/tree",
            "--pid",
            "5150",
            "--proc-start",
            "20260901090000",
        ],
    );
    assert!(
        registered.status.success(),
        "register failed: {}",
        String::from_utf8_lossy(&registered.stderr)
    );

    let read = run_cli(&dir, &addr, &["state", "pij-readback"]);
    assert!(
        read.status.success(),
        "state failed: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let envelope: Envelope<serde_json::Value> =
        serde_json::from_slice(&read.stdout).expect("state envelope on stdout");
    assert!(envelope.ok, "state envelope not ok: {envelope:?}");
    let card = envelope.data.expect("state card");
    assert_eq!(card["id"], "pij-readback");
    assert_eq!(card["cwd"], "/abs/tree");
    assert_eq!(card["harness"], "claude");
    assert_eq!(card["pid"], 5150);
    assert_eq!(card["procStart"], 20260901090000u64);
    assert_eq!(card["liveness"], "active");
    assert_eq!(card["machine"], "test-machine");

    // A seat nobody registered is REFUSED, and the refusal carries an envelope —
    // never a bare 404, which wave 1's shim reads as route-absence and answers
    // from the other store.
    let missing = run_cli(&dir, &addr, &["state", "pij-never-registered"]);
    let refusal: Envelope<serde_json::Value> =
        serde_json::from_slice(&missing.stdout).expect("refusal envelope on stdout");
    assert!(!refusal.ok);
    assert_eq!(refusal.command, "pij state");

    server.abort();
}
