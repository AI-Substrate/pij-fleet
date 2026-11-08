//! Plan 223 AC2, pm-001's point 1: a live seat's capability survives a daemon
//! restart. The store keeps only digests; the daemon rebuilds its digest map
//! from one read at start.

use std::path::PathBuf;

use pij_core::capability::SpawnNonce;
use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::Harness;
use pij_core::ports::LivenessPort;
use pij_daemon::capability::{CAPABILITY_HEADER, mint};
use pij_daemon::{Daemon, boot, build_services};
use pij_testkit::FreshStore;
use serde_json::{Value, json};

const SEAT: &str = "pij-restarted";
const SPAWN: &str = "s-restarted";
const SESSION: &str = "restart-session";

struct Fixture {
    store: FreshStore,
    state: PathBuf,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let store = FreshStore::new();
        let state = PathBuf::from(store.path()).with_extension("cap-state");
        std::fs::create_dir_all(&state).unwrap();
        let config = Config {
            adapters: Adapters {
                registry: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                queue: AdapterChoice::Real,
                liveness: AdapterChoice::Real,
                ..Adapters::default()
            },
            store_path: store.path(),
            bind_addr: "127.0.0.1:0".into(),
            ..Config::default()
        };
        Self {
            store,
            state,
            config,
        }
    }

    async fn boot(&self) -> Daemon {
        boot(&self.config, self.state.clone()).await.unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _keep_store_alive = &self.store;
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

async fn post(daemon: &Daemon, path: &str, body: Value, capability: Option<&str>) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("http://{}{path}", daemon.addr))
        .bearer_auth(&daemon.key.token);
    if let Some(capability) = capability {
        request = request.header(CAPABILITY_HEADER, capability);
    }
    let response = request.json(&body).send().await.unwrap();
    (response.status().as_u16(), response.json().await.unwrap())
}

async fn report(daemon: &Daemon, token: &str, tokens: u64) -> (u16, Value) {
    post(
        daemon,
        "/v1/activity",
        json!({
            "seat": SEAT,
            "native_session": SESSION,
            "state": "idle",
            "context_tokens": tokens,
            "context_token": token,
        }),
        None,
    )
    .await
}

async fn read(daemon: &Daemon, path: &str) -> String {
    reqwest::Client::new()
        .get(format!("http://{}{path}", daemon.addr))
        .bearer_auth(&daemon.key.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_live_seats_capability_still_works_after_a_daemon_restart() {
    let fixture = Fixture::new();
    // The spawn's half, as `launch_seat_locked` does it: the digest only.
    let nonce = mint().unwrap();
    let services = build_services(&fixture.config, &fixture.state.join("pane-signals"))
        .await
        .unwrap();
    services
        .registry
        .insert_spawn_nonce(SpawnNonce {
            spawn_id: SPAWN.into(),
            seat: SEAT.into(),
            harness: Harness::Omp,
            digest: nonce.digest(),
            issued_at: 1,
            expires_at: u64::MAX,
        })
        .await
        .unwrap();
    drop(services);

    // This test process stands in for the seat's harness process.
    let pid = std::process::id();
    let proc_start = pij_harnesses::proc::ProcLiveness::new()
        .proc_start(pid)
        .await
        .unwrap()
        .unwrap();
    let daemon = fixture.boot().await;
    let (status, registered) = post(
        &daemon,
        "/v1/register",
        json!({
            "id": SEAT,
            "harness": "omp",
            "folder": fixture.state,
            "pid": pid,
            "proc_start": proc_start,
            "spawn_id": SPAWN,
            "spawn_nonce": nonce.plaintext(),
            "harness_session": SESSION,
        }),
        None,
    )
    .await;
    assert_eq!(status, 200, "{registered}");
    let token = registered["data"]["capability"]
        .as_str()
        .expect("a capability")
        .to_string();
    assert_eq!(report(&daemon, &token, 1_000).await.0, 200);
    daemon.shutdown().await.unwrap();

    let daemon = fixture.boot().await;
    let (status, reply) = report(&daemon, &token, 2_000).await;
    assert_eq!(status, 200, "the token survives the bounce: {reply}");
    let (status, refused) = report(&daemon, &"0".repeat(64), 3_000).await;
    assert_eq!(status, 400, "{refused}");
    // And it is held after the bounce: presenting it re-registers and
    // answers the same capability back.
    let (status, held) = post(
        &daemon,
        "/v1/register",
        json!({
            "id": SEAT,
            "harness": "omp",
            "folder": fixture.state,
            "pid": pid,
            "proc_start": proc_start,
            "harness_session": SESSION,
        }),
        Some(&token),
    )
    .await;
    assert_eq!(status, 200, "{held}");
    assert_eq!(held["data"]["capability"].as_str(), Some(token.as_str()));

    // Contract d: never in /health, the roster, cards or the spine.
    for path in ["/health", "/v1/seats", "/v1/cards"] {
        let text = read(&daemon, path).await;
        assert!(!text.is_empty(), "{path} answered");
        assert!(!text.contains(&token), "{path} carries the capability");
        assert!(
            !text.contains(nonce.plaintext()),
            "{path} carries the nonce"
        );
    }
    daemon.shutdown().await.unwrap();
    let pool = pij_store::open(&fixture.store.path()).await.unwrap();
    let spine: Vec<String> = sqlx::query_scalar("SELECT kind || ' ' || payload FROM spine_events")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!spine.is_empty());
    for event in spine {
        assert!(!event.contains(&token), "the spine carries the capability");
        assert!(
            !event.contains(nonce.plaintext()),
            "the spine carries the nonce"
        );
    }
}
