//! Plan 229 (workstream 004 s03): the cache-proxy spawn gate over HTTP, on a
//! real SQLite registry, against a stub status route on an ephemeral port.
//! Rows 1, 3, 6, 13, 18, 20 and 22 of the acceptance matrix, at unit scale.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::routing::get;
use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{
    CACHE_PROXY_OPTED_OUT, CacheProxyRoute, Harness, ProcIdentity, SeatCacheProxy, SeatDescriptor,
};
use pij_daemon::Services;
use pij_daemon::cache_proxy::{CacheProxy, STATUS_PATH};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

struct Fixture {
    services: Services,
    tmux: Arc<FakeTmux>,
    addr: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
    reads: Arc<AtomicUsize>,
    _store: FreshStore,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The status route as s02 serves it, `ready` or not.
fn status(ready: bool) -> Value {
    json!({
        "ready": ready,
        "state": if ready { "ready" } else { "no_account" },
        "reason": if ready { "" } else { "no account is set" },
        "fix": if ready { "" } else { "pij-rs cache-proxy account <login>" },
        "account": if ready { json!("octo") } else { Value::Null },
        "upstream_host": if ready { json!("api.individual.githubcopilot.com") } else { Value::Null },
        "quota": {"usable": ready, "reset": null},
        "models": if ready { json!({"claude-opus-5.5": {"max_prompt_tokens": 936000}}) } else { json!({}) },
        "counters": {"requests": 0, "refreshes": 0, "gh_launches": 1, "upstream_401": 0, "upstream_402": 0},
        "pid": 4242,
        "since": 1_763_400_000_000_u64,
    })
}

/// `proxy`: `Some(ready)` stands up a stub status route; `None` points the
/// daemon at a closed port.
async fn fixture(proxy: Option<bool>) -> Fixture {
    let reads = Arc::new(AtomicUsize::new(0));
    let url = match proxy {
        Some(ready) => {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let counted = Arc::clone(&reads);
            let stub = Router::new().route(
                STATUS_PATH,
                get(move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                    async move { axum::Json(status(ready)) }
                }),
            );
            tokio::spawn(async move { axum::serve(listener, stub).await.unwrap() });
            url
        }
        None => {
            let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            format!("http://{}", closed.local_addr().unwrap())
        }
    };
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
    services.cache_proxy = Arc::new(CacheProxy::new(url));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router_with_config(
        services.clone(),
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local("gate-key".to_string()),
            machine_alias: "test".to_string(),
        },
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fixture {
        services,
        tmux,
        addr,
        server,
        reads,
        _store: store,
    }
}

async fn post(f: &Fixture, path: &str, body: &Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{}{path}", f.addr))
        .bearer_auth("gate-key")
        .json(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .expect("decodable envelope")
}

async fn spawn(f: &Fixture, harness: &str, model: &str, opt_out: bool) -> Value {
    let body = post(
        f,
        "/v1/spawn",
        &json!({
            "harness": harness, "model": model, "cwd": "/tmp", "session": "fleet",
            "no_wait": true, "no_cache_proxy": opt_out,
        }),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    body
}

/// The last launch's command line.
fn launch(f: &Fixture) -> String {
    f.tmux
        .calls()
        .into_iter()
        .rfind(|call| call.starts_with("new_window:"))
        .expect("a launch")
}

async fn stored(f: &Fixture, receipt: &Value) -> SeatDescriptor {
    let id = receipt["data"]["id"].as_str().expect("seat id").to_string();
    f.services
        .registry
        .get(&id.into())
        .await
        .unwrap()
        .expect("stored row")
}

/// Row 22: both model names launch proxied, with the base id and its limit.
#[tokio::test]
async fn ready_proxy_wires_a_copilot_claude_seat_with_the_base_id() {
    let f = fixture(Some(true)).await;
    for model in ["claude-opus-5.5", "claude-opus-5.5-1m"] {
        let receipt = spawn(&f, "copilot", model, false).await;
        let command = launch(&f);
        for setting in [
            "COPILOT_PROVIDER_TYPE=anthropic",
            "COPILOT_PROVIDER_MODEL_ID=claude-opus-5.5",
            "COPILOT_PROVIDER_MAX_PROMPT_TOKENS=936000",
            "COPILOT_PROVIDER_VISION=true",
        ] {
            assert!(command.contains(setting), "{setting} in {command}");
        }
        assert!(command.contains("COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:"));
        assert_eq!(
            receipt["data"]["cache_proxy"]["state"], "proxied",
            "{receipt}"
        );
        let row = stored(&f, &receipt).await;
        assert_eq!(
            row.cache_proxy.map(|route| route.state),
            Some(CacheProxyRoute::Proxied),
            "the route survives the store"
        );
    }
}

/// Rows 3, 13, 20: a GPT model, `--no-cache-proxy` and an OMP seat launch
/// with no provider env; only the Copilot Claude spawn reads the status.
#[tokio::test]
async fn model_opt_out_and_other_harnesses_launch_plain() {
    let f = fixture(Some(true)).await;
    let gpt = spawn(&f, "copilot", "gpt-6.1-sol", false).await;
    assert!(!launch(&f).contains("COPILOT_PROVIDER"));
    assert_eq!(gpt["data"]["cache_proxy"]["state"], "plain");
    let opted = spawn(&f, "copilot", "claude-opus-5.5", true).await;
    assert!(!launch(&f).contains("COPILOT_PROVIDER"));
    assert_eq!(
        opted["data"]["cache_proxy"]["reason"],
        CACHE_PROXY_OPTED_OUT
    );
    let omp = spawn(&f, "omp", "claude-opus-5.5", false).await;
    assert!(!launch(&f).contains("COPILOT_PROVIDER"));
    assert!(omp["data"].get("cache_proxy").is_none(), "{omp}");
    assert_eq!(stored(&f, &omp).await.cache_proxy, None);
    assert_eq!(
        f.reads.load(Ordering::SeqCst),
        0,
        "no status read for any of them"
    );
}

/// Rows 1 and 6: not ready and not listening launch plain, with the reason.
#[tokio::test]
async fn not_ready_or_not_listening_launches_plain_with_the_reason() {
    let f = fixture(Some(false)).await;
    let receipt = spawn(&f, "copilot", "claude-opus-5.5", false).await;
    assert!(!launch(&f).contains("COPILOT_PROVIDER"));
    assert_eq!(receipt["data"]["cache_proxy"]["state"], "plain");
    assert_eq!(
        receipt["data"]["cache_proxy"]["reason"],
        "proxy no_account: no account is set"
    );
    assert_eq!(
        receipt["data"]["cache_proxy"]["fix"],
        "pij-rs cache-proxy account <login>"
    );
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);

    let f = fixture(None).await;
    let receipt = spawn(&f, "copilot", "claude-opus-5.5", false).await;
    assert!(!launch(&f).contains("COPILOT_PROVIDER"));
    assert_eq!(
        receipt["data"]["cache_proxy"]["reason"],
        "proxy not listening"
    );
}

/// AC2: a seat spawned with `--no-cache-proxy` stays opted out on revive,
/// though the proxy is ready; any other seat re-runs the gate.
#[tokio::test]
async fn revive_keeps_an_opt_out_and_re_runs_the_gate_otherwise() {
    let f = fixture(Some(true)).await;
    for (id, prior, expected) in [
        (
            "pij-optout",
            Some(SeatCacheProxy::plain(CACHE_PROXY_OPTED_OUT, None)),
            CacheProxyRoute::Plain,
        ),
        ("pij-before", None, CacheProxyRoute::Proxied),
    ] {
        let mut seat = SeatDescriptor::new(id, Harness::Copilot, "/tmp");
        seat.pane = Some("%old".to_string());
        seat.proc = Some(ProcIdentity {
            pid: 4242,
            proc_start: 10,
        });
        seat.model = Some("claude-opus-5.5-1m".to_string());
        seat.cache_proxy = prior;
        f.services.registry.put(seat).await.unwrap();
        let body = post(
            &f,
            "/v1/revive",
            &json!({"id": id, "session": "fleet", "fresh": true}),
        )
        .await;
        assert_eq!(body["ok"], true, "{body}");
        let revived = f.services.registry.get(&id.into()).await.unwrap().unwrap();
        assert_eq!(
            revived.cache_proxy.map(|route| route.state),
            Some(expected),
            "{id}"
        );
        assert_eq!(
            launch(&f).contains("COPILOT_PROVIDER"),
            expected == CacheProxyRoute::Proxied,
            "{id}"
        );
    }
}

/// `/health` carries the watch's view (empty before the first tick here,
/// since the test router starts no watch).
#[tokio::test]
async fn health_carries_cache_proxy() {
    let f = fixture(Some(true)).await;
    let body: Value = reqwest::Client::new()
        .get(format!("http://{}/health", f.addr))
        .bearer_auth("gate-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        body["data"]["cache_proxy"]["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://127.0.0.1:")),
        "{body}"
    );
    assert_eq!(body["data"]["cache_proxy"]["seats"]["proxied"], json!([]));
}
