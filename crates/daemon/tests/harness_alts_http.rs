//! F280: `--alt` launches over HTTP on a real SQLite registry, with the alts
//! file in a temp dir: spawn applies and records an alt, an unknown alt
//! refuses before any launch, and revive reuses the recorded alt or refuses
//! before any write when it is gone.
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, ProcIdentity, SeatAlt, SeatDescriptor};
use pij_daemon::Services;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

struct Fixture {
    services: Services,
    tmux: Arc<FakeTmux>,
    addr: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
    dir: std::path::PathBuf,
    _store: FreshStore,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn fixture() -> Fixture {
    let store = FreshStore::new();
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
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    services.liveness = Arc::new(FakeLiveness::new());
    let dir = pij_testkit::fresh_dir("pij-harness-alts-http");
    services.harness_alts = dir.join("harness-alts.toml");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router_with_config(
        services.clone(),
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local("alt-key".to_string()),
            machine_alias: "test".to_string(),
        },
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fixture {
        services,
        tmux,
        addr,
        server,
        dir,
        _store: store,
    }
}

impl Fixture {
    fn write_alts(&self, text: &str) {
        std::fs::write(&self.services.harness_alts, text).expect("alts file");
    }

    fn home(&self) -> String {
        self.dir.join(".claude-alt").display().to_string()
    }

    fn launches(&self) -> Vec<String> {
        self.tmux
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("new_window:"))
            .collect()
    }

    async fn post(&self, path: &str, body: &Value) -> Value {
        reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth("alt-key")
            .json(body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .expect("decodable envelope")
    }

    async fn row(&self, id: &str) -> SeatDescriptor {
        self.services
            .registry
            .get(&id.into())
            .await
            .unwrap()
            .expect("stored row")
    }
}

#[tokio::test]
async fn spawn_applies_and_records_an_alt_and_refuses_an_unknown_one() {
    let f = fixture().await;
    f.write_alts(&format!(
        "[claude.1]\nenv = {{ CLAUDE_CONFIG_DIR = \"{}\" }}\nargs = [\"--alt-arg\"]\n",
        f.home()
    ));
    let receipt = f
        .post(
            "/v1/spawn",
            &json!({"harness": "claude", "cwd": "/tmp", "session": "fleet", "no_wait": true, "alt": "1"}),
        )
        .await;
    assert_eq!(receipt["ok"], true, "{receipt}");
    let launch = f.launches().pop().expect("a launch");
    assert!(
        launch.contains(&format!("CLAUDE_CONFIG_DIR={}", f.home())),
        "{launch}"
    );
    assert!(launch.contains("--alt-arg"), "{launch}");
    assert_eq!(receipt["data"]["alt"]["name"], "1", "{receipt}");
    let id = receipt["data"]["id"].as_str().expect("id");
    assert_eq!(
        f.row(id).await.alt,
        Some(SeatAlt {
            name: "1".to_string(),
            home: Some(f.home()),
        })
    );

    let launched = f.launches().len();
    let refused = f
        .post(
            "/v1/spawn",
            &json!({"harness": "claude", "cwd": "/tmp", "session": "fleet", "no_wait": true, "alt": "9"}),
        )
        .await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["details"]["code"], "E-RS-ALT-UNKNOWN", "{refused}");
    let message = refused.to_string();
    assert!(message.contains("its claude alts are 1"), "{message}");
    assert!(message.contains("harness-alts.toml"), "{message}");
    assert_eq!(
        f.launches().len(),
        launched,
        "a refused alt launches nothing"
    );

    f.write_alts("[claude.1\n");
    let broken = f
        .post(
            "/v1/spawn",
            &json!({"harness": "claude", "cwd": "/tmp", "session": "fleet", "no_wait": true, "alt": "1"}),
        )
        .await;
    assert_eq!(broken["details"]["code"], "E-RS-ALT-CONFIG", "{broken}");
    assert_eq!(f.launches().len(), launched);
}

#[tokio::test]
async fn revive_reuses_the_recorded_alt_or_refuses_before_any_write() {
    let f = fixture().await;
    let dead = |id: &str| {
        let mut seat = SeatDescriptor::new(id, Harness::Claude, "/tmp");
        seat.pane = Some("%old".to_string());
        seat.proc = Some(ProcIdentity {
            pid: 4242,
            proc_start: 10,
        });
        seat.alt = Some(SeatAlt {
            name: "1".to_string(),
            home: Some("/stale/home".to_string()),
        });
        seat
    };
    f.write_alts(&format!(
        "[claude.1]\nenv = {{ CLAUDE_CONFIG_DIR = \"{}\" }}\n",
        f.home()
    ));
    f.services
        .registry
        .put(dead("pij-alt-revived"))
        .await
        .unwrap();
    let body = f
        .post(
            "/v1/revive",
            &json!({"id": "pij-alt-revived", "session": "fleet", "fresh": true}),
        )
        .await;
    assert_eq!(body["ok"], true, "{body}");
    let launch = f.launches().pop().expect("a launch");
    assert!(
        launch.contains(&format!("CLAUDE_CONFIG_DIR={}", f.home())),
        "{launch}"
    );
    assert_eq!(
        f.row("pij-alt-revived").await.alt,
        Some(SeatAlt {
            name: "1".to_string(),
            home: Some(f.home()),
        }),
        "the file is re-read at revive, so an edited home applies"
    );

    f.write_alts("[claude.2]\n");
    let before = dead("pij-alt-gone");
    f.services.registry.put(before.clone()).await.unwrap();
    let launched = f.launches().len();
    let refused = f
        .post(
            "/v1/revive",
            &json!({"id": "pij-alt-gone", "session": "fleet", "fresh": true}),
        )
        .await;
    assert_eq!(refused["details"]["code"], "E-RS-ALT-UNKNOWN", "{refused}");
    assert_eq!(f.launches().len(), launched, "no default launch");
    assert_eq!(
        f.row("pij-alt-gone").await,
        before,
        "a refused revive leaves the seat as it was"
    );
}
