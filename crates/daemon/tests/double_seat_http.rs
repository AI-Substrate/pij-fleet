//! Plan 174 AC8(d) — the 2026-10-15 double-seat incident, end to end, on the
//! real SQLite registry and spine.
//!
//! Seat `pij-different-smelt` is live in pane %5. A second Claude process
//! resumed the same conversation by hand in pane %7. From %7: adopt is refused
//! with the stable code, the live seat is told once, a send is refused with the
//! actionable text, and a repeat adopt (also after a real restart on the same
//! store file) sends no second notice.
//!
//! Hermetic: a fixture Claude home, fake tmux, fake liveness, a private port.
//! The live seat is SEEDED, never registered, so no registration path runs.
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, Pane, PaneProcess, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_daemon::Services;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

const CODE: &str = "E-RS-CONVERSATION-LIVE";
const DETECTED: &str = "seat.double-detected";
const SESSION: &str = "cc2d2e8f-0174-4000-8000-0000000e2e01";
const SEAT: &str = "pij-different-smelt";
const LIVE_PANE: &str = "%5";
const STRAY_PANE: &str = "%7";

/// The local packed start for a process Claude recorded at `hh:mm:ss` UTC on
/// 2026-10-15, at this host's real offset (the one the daemon reads).
fn local_start(hh: i64, mm: i64, ss: u64) -> u64 {
    let offset = i64::from(pij_harnesses::proc::local_utc_offset_minutes().expect("utc offset"));
    let minutes = hh * 60 + mm + offset;
    let day = 15 + minutes.div_euclid(1_440);
    let minute_of_day = minutes.rem_euclid(1_440);
    20_261_000_000_000
        + u64::try_from(day).unwrap() * 1_000_000
        + u64::try_from(minute_of_day / 60).unwrap() * 10_000
        + u64::try_from(minute_of_day % 60).unwrap() * 100
        + ss
}

fn live() -> ProcIdentity {
    ProcIdentity {
        pid: 918_405,
        proc_start: local_start(0, 57, 34),
    }
}

fn stray() -> ProcIdentity {
    ProcIdentity {
        pid: 918_407,
        proc_start: local_start(0, 58, 23),
    }
}

struct Daemon {
    services: Services,
    addr: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn pane(id: &str) -> Pane {
    Pane {
        id: id.to_string(),
        session: "main".to_string(),
        window: "@1".to_string(),
        title: "claude".to_string(),
        cursor_x: None,
        cursor_y: None,
    }
}

/// Boot a daemon on `store`'s file: a second boot on the same store IS a restart.
async fn boot(store: &FreshStore, home: &std::path::Path) -> Daemon {
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(
        &config,
        &std::path::Path::new(&store.path()).with_extension("signals"),
    )
    .await
    .expect("real registry and spine");
    services.tmux = Arc::new(
        FakeTmux::new()
            .with_pane(pane(LIVE_PANE))
            .with_pane(pane(STRAY_PANE))
            .with_pane_process(
                STRAY_PANE,
                PaneProcess {
                    pid: stray().pid,
                    cwd: "/abs/tree".to_string(),
                },
            ),
    );
    services.liveness = Arc::new(FakeLiveness::new().with_proc(live()).with_proc(stray()));
    services.claude_homes = Arc::from([home.to_path_buf()]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router_with_config(services.clone(), HttpConfig::local("e2e-key".to_string()));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Daemon {
        services,
        addr,
        server,
    }
}

async fn post(daemon: &Daemon, path: &str, body: Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{}{path}", daemon.addr))
        .bearer_auth("e2e-key")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .expect("decodable envelope")
}

async fn adopt_from_stray(daemon: &Daemon) -> Value {
    post(
        daemon,
        "/v1/adopt",
        json!({"argv": ["adopt", STRAY_PANE, "--harness", "claude"]}),
    )
    .await
}

async fn seat_events(daemon: &Daemon) -> Vec<pij_core::model::Event> {
    daemon
        .services
        .spine
        .tail(Some(&SeatId::from(SEAT)), Seq(0))
        .await
        .unwrap()
}

async fn detections(daemon: &Daemon) -> Vec<Value> {
    seat_events(daemon)
        .await
        .into_iter()
        .filter(|event| event.kind == DETECTED)
        .map(|event| serde_json::from_str(&event.payload).unwrap())
        .collect()
}

fn assert_coded(verb: &str, body: &Value) {
    assert_eq!(body["ok"], false, "{verb}: {body}");
    let meta = body["meta"].as_str().unwrap_or_default();
    assert!(meta.starts_with(CODE), "{verb}: {body}");
    for needle in [
        format!("is live as seat `{SEAT}`"),
        format!("(pane {LIVE_PANE})"),
        "close one copy".to_string(),
        format!("pij-rs adopt {STRAY_PANE} --harness claude"),
    ] {
        assert!(meta.contains(&needle), "{verb}: names `{needle}`: {body}");
    }
    assert_eq!(body["details"]["code"], CODE, "{verb}: {body}");
}

#[tokio::test]
async fn double_seat_incident_end_to_end_on_the_real_store() {
    let store = FreshStore::new();
    let home = pij_testkit::fresh_dir("pij-double-seat-e2e-home");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    for (proc, utc, pane) in [
        (live(), "Thu Oct 15 00:57:34 2026", LIVE_PANE),
        (stray(), "Thu Oct 15 00:58:23 2026", STRAY_PANE),
    ] {
        std::fs::write(
            home.join(format!("sessions/{}.json", proc.pid)),
            json!({"pid": proc.pid, "sessionId": SESSION, "procStart": utc,
                   "tmux": format!("main:@1.{pane}")})
            .to_string(),
        )
        .unwrap();
    }

    let daemon = boot(&store, &home).await;
    let mut seat = SeatDescriptor::new(SEAT, Harness::Claude, "/abs/tree");
    seat.pane = Some(LIVE_PANE.to_string());
    seat.proc = Some(live());
    seat.harness_session = Some(SESSION.to_string());
    daemon.services.registry.put(seat).await.unwrap();

    // 1. Adopt from the stray pane: refused with the code.
    let adopt = adopt_from_stray(&daemon).await;
    assert_coded("adopt", &adopt);
    assert_eq!(adopt["details"]["seat"], SEAT, "{adopt}");
    assert_eq!(adopt["details"]["pane"], LIVE_PANE, "{adopt}");
    assert_eq!(adopt["details"]["harness_session"], SESSION, "{adopt}");
    assert_eq!(adopt["details"]["pid"], live().pid, "{adopt}");

    // 2. One notice to the live seat, recorded on the spine and delivered.
    let found = detections(&daemon).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["stray_pane"], STRAY_PANE);
    assert_eq!(found[0]["stray_pid"], stray().pid);
    let msg_id = found[0]["msg_id"].as_str().expect("notice id").to_string();
    assert!(
        seat_events(&daemon)
            .await
            .iter()
            .any(|event| event.kind != DETECTED && event.payload.contains(&msg_id)),
        "the notice was admitted to the live seat"
    );

    // 3. A send from the stray pane: refused with the actionable text.
    let send = post(
        &daemon,
        "/v1/shim/send",
        json!({"argv": ["send", SEAT, "hello?"], "caller": {"tmuxPane": STRAY_PANE}}),
    )
    .await;
    assert_coded("send", &send);

    // 4. Repeat adopt, then a real restart on the same store file: no second notice.
    assert_coded("repeat adopt", &adopt_from_stray(&daemon).await);
    assert_eq!(detections(&daemon).await.len(), 1);
    drop(daemon);
    let daemon = boot(&store, &home).await;
    assert_coded("adopt after restart", &adopt_from_stray(&daemon).await);
    assert_eq!(
        detections(&daemon).await.len(),
        1,
        "restart does not resend"
    );

    let _ = std::fs::remove_dir_all(home);
}
