use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Config};
use pij_core::model::{Envelope, ErrorKind, Harness, ProcIdentity, SeatDescriptor, SeatId};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use pij_testkit::fresh_dir;

fn run_cli(state_dir: &std::path::Path, addr: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([
            "--state-dir",
            state_dir.to_str().expect("UTF-8 state dir"),
            "--addr",
            addr,
            "--json",
        ])
        .args(args)
        .output()
        .expect("run shipped CLI")
}

fn launched_environment(call: &str) -> Vec<String> {
    let start = call
        .find(":[\"")
        .expect("fake tmux must record the observer argv");
    serde_json::from_str(&call[start + 1..]).expect("recorded discrete argv must remain parseable")
}

fn required_environment_value(args: &[String], name: &str) -> String {
    let prefix = format!("{name}=");
    args.iter()
        .find_map(|arg| arg.strip_prefix(&prefix))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("launched process was not given {name}"))
}

async fn serve(
    key: &str,
    mut services: pij_daemon::Services,
    tmux: Arc<FakeTmux>,
    live_procs: Vec<ProcIdentity>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    services.tmux = tmux;
    let mut liveness = FakeLiveness::new();
    for identity in live_procs {
        liveness = liveness.with_proc(identity);
    }
    services.liveness = Arc::new(liveness);
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
    let key = key.to_string();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(
                services,
                HttpConfig {
                    auth: pij_daemon::http::AuthRing::local(key),
                    machine_alias: "test-machine".to_string(),
                },
            ),
        )
        .await
        .expect("serve router");
    });
    (addr, server)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_revive_refuses_absent_and_live_ids_without_launching() {
    let dir = fresh_dir("pij-revive-refusal");
    std::fs::write(dir.join("daemon.key"), "revive-test-key").expect("write client key");
    let mut config = Config {
        store_path: dir
            .join("pij.sqlite")
            .to_str()
            .expect("UTF-8 database path")
            .to_string(),
        ..Config::default()
    };
    config.adapters.registry = AdapterChoice::Real;
    config.adapters.spine = AdapterChoice::Real;
    let services = pij_daemon::build_services(&config, &dir.join("pane-signals"))
        .await
        .expect("coherent real registry and spine");
    let registry = services.registry.clone();
    let tmux = Arc::new(FakeTmux::new());
    let (addr, server) = serve("revive-test-key", services, tmux.clone(), Vec::new()).await;
    let addr = addr.to_string();

    let absent = run_cli(
        &dir,
        &addr,
        &["revive", "pij-never-existed", "--session", "fleet"],
    );
    assert!(!absent.status.success(), "an absent id cannot be revived");
    let absent: Envelope<SeatDescriptor> =
        serde_json::from_slice(&absent.stdout).expect("absent JSON envelope");
    assert_eq!(absent.command, "pij revive");
    assert_eq!(absent.error, Some(ErrorKind::NotFound));
    let absent_reason = absent.meta.expect("absent refusal reason");
    assert!(absent_reason.contains("pij-never-existed"));
    assert!(absent_reason.contains("does not exist"));

    registry
        .put(SeatDescriptor::new(
            SeatId::from("pij-still-live"),
            Harness::Copilot,
            "/abs/live",
        ))
        .await
        .expect("seed live row");
    let live = run_cli(
        &dir,
        &addr,
        &["revive", "pij-still-live", "--session", "fleet"],
    );
    assert!(!live.status.success(), "a live id cannot be revived");
    let live: Envelope<SeatDescriptor> =
        serde_json::from_slice(&live.stdout).expect("live JSON envelope");
    assert_eq!(live.command, "pij revive");
    assert_eq!(live.error, Some(ErrorKind::Refused));
    assert_eq!(
        live.details.expect("decodable liveness refusal")["code"],
        "E-RS-REVIVE-LIVE"
    );
    assert!(
        tmux.calls()
            .iter()
            .all(|call| !call.starts_with("new_window:"))
    );

    server.abort();
    drop(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_revive_derives_identity_and_launches_one_prebind_incarnation() {
    let dir = fresh_dir("pij-revive-transition");
    std::fs::write(dir.join("daemon.key"), "revive-test-key").expect("write client key");
    let mut config = Config {
        store_path: dir
            .join("pij.sqlite")
            .to_str()
            .expect("UTF-8 database path")
            .to_string(),
        ..Config::default()
    };
    config.adapters.registry = AdapterChoice::Real;
    config.adapters.spine = AdapterChoice::Real;
    let services = pij_daemon::build_services(&config, &dir.join("pane-signals"))
        .await
        .expect("coherent real registry and spine");
    let registry = services.registry.clone();
    let tmux = Arc::new(FakeTmux::new());

    let mut old = SeatDescriptor::new(
        SeatId::from("pij-returning-seat"),
        Harness::Copilot,
        "/abs/prior-folder",
    );
    old.model = Some("provider/prior-model".to_string());
    old.effort = Some("high".to_string());
    old.parent = Some(SeatId::from("pij-parent"));
    old.cross_session_inbound_accept = Some(false);
    old.pane = Some("%old".to_string());
    old.proc = Some(ProcIdentity {
        pid: 4242,
        proc_start: 20260831030000,
    });
    old.harness_session = Some("00000000-0000-4000-8000-000000000137".to_string());
    old.native_extension_delivery = true;
    registry
        .put(old)
        .await
        .expect("seed prior native registration");
    registry
        .tombstone(&SeatId::from("pij-returning-seat"), "prior process exited")
        .await
        .expect("retire prior native registration");

    let registered_proc = ProcIdentity {
        pid: 4343,
        proc_start: 20260831030200,
    };
    let restarted_proc = ProcIdentity {
        pid: 4444,
        proc_start: 20260831030300,
    };
    let (addr, server) = serve(
        "revive-test-key",
        services,
        tmux.clone(),
        vec![registered_proc, restarted_proc],
    )
    .await;
    let output = run_cli(
        &dir,
        &addr.to_string(),
        &[
            "revive",
            "pij-returning-seat",
            "--session",
            "fleet",
            "--name",
            "returned",
        ],
    );
    assert!(
        output.status.success(),
        "revive failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Envelope<SeatDescriptor> =
        serde_json::from_slice(&output.stdout).expect("revive JSON envelope");
    assert_eq!(response.command, "pij revive");
    let returned = response.data.expect("revived descriptor");
    let persisted = registry
        .get(&SeatId::from("pij-returning-seat"))
        .await
        .expect("read revived row")
        .expect("revived row exists");
    assert_eq!(returned, persisted, "response must report persisted state");
    assert_eq!(persisted.harness, Harness::Copilot);
    assert_eq!(persisted.folder, "/abs/prior-folder");
    assert_eq!(persisted.model.as_deref(), Some("provider/prior-model"));
    assert_eq!(persisted.effort.as_deref(), Some("high"));
    assert_eq!(persisted.parent, Some(SeatId::from("pij-parent")));
    assert_eq!(
        persisted.proc, None,
        "revive creates a pre-bind incarnation"
    );
    assert_eq!(persisted.tombstoned_at, None);
    assert_eq!(persisted.tombstone_reason, None);
    assert!(
        !persisted.native_extension_delivery,
        "Copilot revive requires fresh native registration, not the previous capability"
    );
    let fresh_spawn_id = persisted
        .spawn_id
        .clone()
        .expect("revive must mint a fresh spawn id");

    let calls = tmux.calls();
    assert_eq!(calls.len(), 1, "one revive must launch exactly one pane");
    assert!(
        calls[0].starts_with("new_window:fleet:returned:"),
        "unexpected launch record: {}",
        calls[0]
    );
    assert!(calls[0].contains("__spawn-child"));
    assert!(calls[0].contains("\"env\""));
    assert!(calls[0].contains("copilot"));
    assert!(!calls[0].contains("--ui-server"));
    assert!(!calls[0].contains("--port"));
    let launched_args = launched_environment(&calls[0]);
    let launched_seat_id = required_environment_value(&launched_args, "PIJ_SESSION_ID");
    let launched_spawn_id = required_environment_value(&launched_args, "PIJ_SPAWN_ID");
    assert_eq!(launched_seat_id, persisted.id.as_str());
    assert_eq!(launched_spawn_id, fresh_spawn_id);

    let first_bind = run_cli(
        &dir,
        &addr.to_string(),
        &[
            "register",
            &launched_seat_id,
            "--harness",
            "copilot",
            "--folder",
            "/abs/prior-folder",
            "--pane",
            "%new",
            "--pid",
            "4343",
            "--proc-start",
            "20260831030200",
            "--spawn-id",
            &launched_spawn_id,
        ],
    );
    assert!(
        first_bind.status.success(),
        "first binding registration failed: {}",
        String::from_utf8_lossy(&first_bind.stderr)
    );
    let after_first_bind = registry
        .get(&SeatId::from("pij-returning-seat"))
        .await
        .expect("read after first bind")
        .expect("bound row exists");
    assert_eq!(after_first_bind.proc, Some(registered_proc));
    assert!(
        !after_first_bind.native_extension_delivery,
        "matching CLI registration binds the process but does not attest a native extension"
    );

    let later_incarnation = run_cli(
        &dir,
        &addr.to_string(),
        &[
            "register",
            "pij-returning-seat",
            "--harness",
            "copilot",
            "--folder",
            "/abs/prior-folder",
            "--pane",
            "%later",
            "--pid",
            "4444",
            "--proc-start",
            "20260831030300",
        ],
    );
    assert!(
        !later_incarnation.status.success(),
        "an unsolicited process replacement must refuse: {}",
        String::from_utf8_lossy(&later_incarnation.stdout)
    );
    let after_later_incarnation = registry
        .get(&SeatId::from("pij-returning-seat"))
        .await
        .expect("read after later incarnation")
        .expect("later incarnation row exists");
    assert_eq!(
        after_later_incarnation, after_first_bind,
        "refusal preserves bound identity"
    );
    assert!(
        !after_later_incarnation.native_extension_delivery,
        "a later incarnation cannot inherit the previous native extension capability"
    );

    server.abort();
    drop(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shipped_revive_assumption_flags_authorize_parent_and_record_evidence() {
    let dir = fresh_dir("pij-revive-assumed");
    std::fs::write(dir.join("daemon.key"), "revive-test-key").unwrap();
    let mut config = Config {
        store_path: dir.join("pij.sqlite").to_str().unwrap().to_string(),
        ..Config::default()
    };
    config.adapters.registry = AdapterChoice::Real;
    config.adapters.spine = AdapterChoice::Real;
    let services = pij_daemon::build_services(&config, &dir.join("signals"))
        .await
        .unwrap();
    let registry = services.registry.clone();
    let spine = services.spine.clone();
    registry
        .put(SeatDescriptor::new("pij-parent", Harness::Copilot, "/tmp"))
        .await
        .unwrap();
    let mut prior = SeatDescriptor::new("pij-child", Harness::Copilot, "/tmp");
    prior.parent = Some("pij-parent".into());
    prior.proc = Some(ProcIdentity {
        pid: 4242,
        proc_start: 10,
    });
    prior.pane = Some("%old".to_string());
    let before = registry.put(prior).await.unwrap();
    let tmux = Arc::new(FakeTmux::new());
    let (addr, server) = serve(
        "revive-test-key",
        services,
        tmux.clone(),
        vec![ProcIdentity {
            pid: 4242,
            proc_start: 20,
        }],
    )
    .await;
    let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args([
            "--state-dir",
            dir.to_str().unwrap(),
            "--addr",
            &addr.to_string(),
            "--json",
            "revive",
            "pij-child",
            "--session",
            "fleet",
            "--assume-dead",
            "--evidence",
            "old pane killed; pid belongs to replacement",
            "--fresh",
        ])
        .env("PIJ_SESSION_ID", "pij-parent")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "revive flags must reach daemon, not be unknown flags: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true, "{body}");
    let events = spine
        .tail(Some(&SeatId::from("pij-child")), before)
        .await
        .unwrap();
    let assumption = events
        .iter()
        .find(|event| event.kind == "revive.assumed-dead")
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&assumption.payload).unwrap();
    assert_eq!(payload["caller"], "pij-parent");
    assert_eq!(
        payload["evidence"],
        "old pane killed; pid belongs to replacement"
    );
    assert_eq!(
        body["details"]["assumed_dead_seq"],
        serde_json::json!(assumption.seq)
    );
    assert_eq!(
        tmux.calls()
            .iter()
            .filter(|call| call.starts_with("new_window:"))
            .count(),
        1
    );
    server.abort();
}

#[test]
fn shipped_revive_refuses_unknown_or_incomplete_override_flags_before_connecting() {
    let dir = fresh_dir("pij-revive-bad-flags");
    for flags in [
        vec!["--force"],
        vec!["--assume-dead"],
        vec!["--evidence", "unpaired evidence"],
    ] {
        let mut args = vec!["revive", "pij-child", "--session", "fleet"];
        args.extend(flags);
        let output = run_cli(&dir, "127.0.0.1:1", &args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "invalid arguments must be rejected before a daemon request"
        );
    }
}
