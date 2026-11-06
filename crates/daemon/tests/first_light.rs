//! The hello-depth daemon, end to end over a real socket (ac-0005).
//!
//! Not a route-handler unit test: these boot the actual server on loopback and
//! speak HTTP to it, because the properties that matter here — the key exists
//! before anything can connect, auth covers the whole router, a second daemon
//! cannot take the port — are properties of the BOOT, not of a handler.

use std::path::PathBuf;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Config};
use pij_core::model::Event;
use pij_core::wire::{self, WireEvent};

fn state_dir() -> PathBuf {
    // `pij_testkit::fresh_dir`, not a local helper: the first version of this
    // file named its directory from pid+clock only and flaked, because two tests
    // starting inside one clock tick got the SAME directory and one deleted the
    // other's key. The shared primitive carries the process-local counter that
    // fixes it — which is the whole argument for testkit-first (DL-004).
    pij_testkit::fresh_dir("pij-rs-test")
}

fn config() -> Config {
    Config {
        // Port 0: the OS picks a free one, so parallel tests never collide on a
        // fixed port — the classic source of a suite that fails only in CI.
        bind_addr: "127.0.0.1:0".to_string(),
        ..Config::default()
    }
}

#[tokio::test]
async fn ping_round_trips_over_localhost_with_the_boot_key() {
    let dir = state_dir();
    let daemon = pij_daemon::boot(&config(), dir.clone())
        .await
        .expect("boot");

    let body = reqwest::Client::new()
        .get(format!("http://{}/health", daemon.addr))
        .header(reqwest::header::AUTHORIZATION, daemon.key.header())
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");

    let envelope: pij_core::model::Envelope<serde_json::Value> =
        wire::decode_envelope(&body).expect("the daemon answers a v1 envelope");
    assert!(envelope.ok);
    assert_eq!(envelope.command, "pij ping");
    assert_eq!(envelope.data.as_ref().unwrap()["status"], "healthy");
    assert_eq!(
        envelope.data.as_ref().unwrap()["offline"],
        true,
        "the default config runs entirely on fakes"
    );

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

/// Plan 211 (AC1): a registration's advisory `attempt` labels the counters
/// ping reports, and a refused attempt counts too.
#[tokio::test]
async fn ping_counts_registration_attempts_by_path_mode_and_outcome() {
    let dir = state_dir();
    let daemon = pij_daemon::boot(&config(), dir.clone())
        .await
        .expect("boot");
    let client = reqwest::Client::new();
    let refused = client
        .post(format!("http://{}/v1/register", daemon.addr))
        .header(reqwest::header::AUTHORIZATION, daemon.key.header())
        .json(&serde_json::json!({
            "id": "", "harness": "copilot", "folder": dir.display().to_string(),
            "relay": false, "attempt": "shim"
        }))
        .send()
        .await
        .expect("register");
    assert!(!refused.status().is_success(), "no session: refused");
    let body = client
        .get(format!("http://{}/health", daemon.addr))
        .header(reqwest::header::AUTHORIZATION, daemon.key.header())
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    let envelope: pij_core::model::Envelope<serde_json::Value> =
        wire::decode_envelope(&body).expect("a v1 envelope");
    let counts = envelope.data.expect("ping data")["registrations"].clone();
    assert_eq!(counts["total"], 1, "{counts}");
    assert_eq!(
        counts["by_key"]["shim.paneless.refused"]["count"], 1,
        "{counts}"
    );
    assert_eq!(counts["by_seat"]["(unnamed)"]["count"], 1, "{counts}");

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

/// Plan 180 follow-up C: ping names the daemon's process launches by call site.
#[tokio::test]
async fn ping_counts_the_daemons_process_launches_by_operation() {
    let daemon = pij_daemon::boot(&config(), state_dir())
        .await
        .expect("boot");
    let spawns = || async {
        let body = reqwest::Client::new()
            .get(format!("http://{}/health", daemon.addr))
            .header(reqwest::header::AUTHORIZATION, daemon.key.header())
            .send()
            .await
            .expect("request")
            .text()
            .await
            .expect("body");
        let envelope: pij_core::model::Envelope<serde_json::Value> =
            wire::decode_envelope(&body).expect("a v1 envelope");
        envelope.data.expect("ping data")["spawns"].clone()
    };
    let total = |spawns: &serde_json::Value| spawns["total"].as_u64().expect("spawns.total");
    let fallback = |spawns: &serde_json::Value| {
        spawns["by_op"]
            .as_object()
            .expect("spawns.by_op")
            .get("ps.proc-liveness-fallback")
            .map_or(0, |count| count.as_u64().expect("a count"))
    };

    let before = spawns().await;
    // A real launch site: liveness launches nothing for live or reaped pids
    // (plan 182) and the UTC offset is in-process (plan 207), so probe a pid
    // only `ps` can decide: a zombie on macOS, a thread id on Linux.
    only_ps_decides().await;
    let after = spawns().await;

    assert!(fallback(&after) > fallback(&before), "{before} -> {after}");
    assert!(total(&after) > total(&before), "{before} -> {after}");
    let by_op_sum: u64 = after["by_op"]
        .as_object()
        .expect("spawns.by_op")
        .values()
        .map(|count| count.as_u64().expect("a count"))
        .sum();
    assert_eq!(
        by_op_sum,
        total(&after),
        "one snapshot: the operations sum to the total"
    );
}

/// Probe one pid that liveness hands to its exact-pid `/bin/ps` fallback.
async fn only_ps_decides() {
    use pij_core::ports::LivenessPort;
    let liveness = pij_harnesses::proc::ProcLiveness::new();
    #[cfg(target_os = "macos")]
    {
        let zombie = std::process::Command::new("/usr/bin/true")
            .spawn()
            .expect("spawn a child");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        liveness
            .proc_start(zombie.id())
            .await
            .expect("probe a zombie");
        drop(zombie);
    }
    #[cfg(target_os = "linux")]
    {
        let (park, parked) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = parked.recv();
        });
        let pid = std::process::id();
        let tid: u32 = std::fs::read_dir("/proc/self/task")
            .expect("list threads")
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .find(|&tid| tid != pid)
            .expect("a thread id other than the pid");
        liveness.proc_start(tid).await.expect("probe a thread id");
        drop(park);
        worker.join().expect("join");
    }
}

#[tokio::test]
async fn the_key_is_0600_and_exists_before_the_socket_can_be_reached() {
    let dir = state_dir();
    let daemon = pij_daemon::boot(&config(), dir.clone())
        .await
        .expect("boot");

    // `boot` returns only after the key is written, and the socket does not exist
    // until after that — so by the time any client CAN connect, the key is
    // already private. Ordering is the security property; this asserts the state
    // it produces.
    assert!(daemon.key.path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&daemon.key.path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the boot key must not be readable by others");
    }
    assert_eq!(daemon.key.token.len(), 64, "256 bits, hex-encoded");

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn boot_publishes_complete_runtime_identity_as_restart_safe_residue() {
    let dir = state_dir();
    let daemon = pij_daemon::boot(&config(), dir.clone())
        .await
        .expect("boot");
    let path = dir.join(pij_daemon::DAEMON_RUNTIME_FILE);
    let runtime: pij_daemon::DaemonRuntime = serde_json::from_slice(
        &std::fs::read(&path).expect("runtime record must exist before serving"),
    )
    .expect("typed runtime record");

    assert_eq!(runtime.process.pid, std::process::id());
    assert_ne!(runtime.process.proc_start, 0);
    assert_eq!(runtime.addr, daemon.addr);
    assert!(runtime.offline);
    assert!(!runtime.machine.is_empty());

    daemon.shutdown().await.expect("shutdown");
    assert!(
        path.exists(),
        "shutdown leaves identity residue; the next reader corroborates pid and start"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn an_unauthenticated_request_is_refused_and_told_what_to_do() {
    let dir = state_dir();
    let daemon = pij_daemon::boot(&config(), dir.clone())
        .await
        .expect("boot");
    let client = reqwest::Client::new();

    for (label, header) in [("none", None), ("wrong", Some("Bearer nope"))] {
        let mut request = client.get(format!("http://{}/health", daemon.addr));
        if let Some(header) = header {
            request = request.header(reqwest::header::AUTHORIZATION, header);
        }
        let response = request.send().await.expect("request");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "a {label} token must be refused"
        );

        let body = response.text().await.expect("body");
        assert!(
            body.contains("daemon.key") && body.contains("Authorization: Bearer"),
            "the refusal must name the next action, not just say no: {body}"
        );
    }

    // The middleware covers the WHOLE router, so a route added later is
    // authenticated because it exists rather than because someone remembered.
    let events = client
        .get(format!("http://{}/v1/events", daemon.addr))
        .send()
        .await
        .expect("request");
    assert_eq!(events.status(), reqwest::StatusCode::UNAUTHORIZED);

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_event_stream_opens_with_hello_and_survives_an_unknown_kind() {
    let services = pij_daemon::build_services(
        &Config::default(),
        std::path::Path::new("/tmp/pij-test-pane-signals"),
    )
    .await
    .expect("fake services");
    let bus = services.event_bus.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            pij_daemon::http::router_with_config(
                services,
                pij_daemon::http::HttpConfig {
                    auth: pij_daemon::http::AuthRing::local("key".to_string()),
                    machine_alias: "first-light".to_string(),
                },
            ),
        )
        .await
        .expect("serve");
    });

    let mut response = reqwest::Client::new()
        .get(format!("http://{addr}/v1/events"))
        .bearer_auth("key")
        .send()
        .await
        .expect("request");
    let hello = response
        .chunk()
        .await
        .expect("hello chunk")
        .expect("stream starts with bytes");
    match wire::decode_event_line(1, std::str::from_utf8(&hello).expect("utf8").trim())
        .expect("the first line decodes")
    {
        WireEvent::Hello { v, build } => {
            assert_eq!(v, wire::EVENT_VERSION);
            assert!(
                build.starts_with("pij-rs"),
                "hello names the build: {build}"
            );
        }
        other => panic!("the stream must open with Hello, got {other:?}"),
    }

    bus.publish(Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at: 7,
        kind: "future.kind".to_string(),
        seat: Some("pij-reader".into()),
        payload: "{\"new\":true}".to_string(),
    })
    .await
    .expect("publish unknown event");
    let frame = tokio::time::timeout(Duration::from_secs(1), response.chunk())
        .await
        .expect("unknown event must not be filtered")
        .expect("frame chunk")
        .expect("frame");
    let frame: pij_daemon::http::StreamFrame = serde_json::from_slice(&frame).expect("event frame");
    let pij_daemon::http::StreamFrame::Event {
        machine,
        cursor,
        event,
    } = &frame
    else {
        panic!("expected event frame");
    };
    assert_eq!(machine, "first-light");
    assert_eq!(*cursor, 1, "cursor is assigned by the local spine");
    assert_eq!(event.kind, "future.kind");
    assert_eq!(event.payload, "{\"new\":true}");
    assert!(
        serde_json::to_value(&frame).expect("frame json")["event"]
            .get("seq")
            .is_none(),
        "the cursor belongs to the frame, never the event body"
    );

    drop(response);
    server.abort();
}

#[tokio::test]
async fn a_second_daemon_cannot_take_a_held_port() {
    // The single-instance guard on day one, for free: the OS already refuses a
    // second bind, so the daemon's job is to say so in words an operator can act
    // on rather than to invent a lock file.
    let dir = state_dir();
    let first = pij_daemon::boot(&config(), dir.clone())
        .await
        .expect("boot");

    let taken = Config {
        bind_addr: first.addr.to_string(),
        ..Config::default()
    };
    let error = pij_daemon::boot(&taken, state_dir())
        .await
        .expect_err("the second daemon must refuse");
    assert!(
        error
            .to_string()
            .contains("another daemon may already hold it"),
        "the refusal must name the likely cause: {error}"
    );

    first.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_real_store_can_be_selected_per_port_without_touching_the_others() {
    // Offline-first is a DEFAULT, not a cage: a test (or an operator) makes
    // exactly one thing real and leaves the rest fake.
    let dir = state_dir();
    let store = pij_testkit::FreshStore::new();
    let config = Config {
        adapters: pij_core::config::Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            ..pij_core::config::Adapters::default()
        },
        store_path: store.path(),
        bind_addr: "127.0.0.1:0".to_string(),
        ..Config::default()
    };
    assert!(!config.is_fully_offline());

    let daemon = pij_daemon::boot(&config, dir.clone()).await.expect("boot");
    let body = reqwest::Client::new()
        .get(format!("http://{}/health", daemon.addr))
        .header(reqwest::header::AUTHORIZATION, daemon.key.header())
        .send()
        .await
        .expect("request")
        .text()
        .await
        .expect("body");
    let envelope: pij_core::model::Envelope<serde_json::Value> =
        wire::decode_envelope(&body).expect("envelope");
    assert_eq!(
        envelope.data.as_ref().unwrap()["offline"],
        false,
        "a daemon on a real store must not claim to be offline"
    );

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(dir);
}
