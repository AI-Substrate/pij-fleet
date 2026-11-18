//! The sidecar against a fake GitHub/Copilot upstream and a fake `gh`: the
//! behaviour a reader can't see from the code (single-flight refresh, the
//! failed-refresh latch, 402 handling, the loopback guard) made deterministic.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State as AxumState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use pij_cache_proxy::config::{self, ProxyConfig};
use pij_cache_proxy::{Proxy, Settings, State, Status, router, serve};
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// Each account's fixture token is `gho_tok_<account>`, so a test can tell
/// which account's token reached upstream (#239 F001).
fn token_for(account: &str) -> String {
    format!("gho_tok_{account}")
}

struct Upstream {
    base: Mutex<String>,
    /// `copilot_internal/user` reports the quota used up.
    user_exhausted: AtomicBool,
    /// `/v1/messages` answers 402, independently of the user snapshot (#239 F002).
    messages_402: AtomicBool,
    /// `/v1/messages` answers 302 to this URL.
    messages_redirect: Mutex<Option<String>>,
    reset_date: Mutex<String>,
    messages: AtomicU64,
    seen: Mutex<Option<(String, Value)>>,
    /// The bearer each endpoint last saw.
    user_bearer: Mutex<String>,
    messages_bearer: Mutex<String>,
}

impl Default for Upstream {
    fn default() -> Self {
        Self {
            base: Mutex::default(),
            user_exhausted: AtomicBool::new(false),
            messages_402: AtomicBool::new(false),
            messages_redirect: Mutex::default(),
            reset_date: Mutex::new("2999-12-01".into()),
            messages: AtomicU64::new(0),
            seen: Mutex::default(),
            user_bearer: Mutex::default(),
            messages_bearer: Mutex::default(),
        }
    }
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn bearer_ok(bearer: &str) -> bool {
    bearer.starts_with("Bearer gho_tok_")
}

async fn user(AxumState(up): AxumState<Arc<Upstream>>, headers: HeaderMap) -> impl IntoResponse {
    let bearer = bearer(&headers);
    *up.user_bearer.lock().unwrap() = bearer.clone();
    if !bearer_ok(&bearer) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({}))).into_response();
    }
    let exhausted = up.user_exhausted.load(Ordering::SeqCst);
    let base = up.base.lock().unwrap().clone();
    let reset = up.reset_date.lock().unwrap().clone();
    axum::Json(json!({
        "endpoints": {"api": base},
        "quota_snapshots": {"premium_interactions": {
            "remaining": if exhausted { 0 } else { 100 },
            "overage_permitted": false
        }},
        "quota_reset_date": reset
    }))
    .into_response()
}

async fn models(headers: HeaderMap) -> impl IntoResponse {
    if !bearer_ok(&bearer(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    axum::Json(json!({"data": [
        {"id": "claude-opus-5.5", "capabilities": {"limits": {"max_prompt_tokens": 1_000_000}}},
        {"id": "gpt-6.1-sol", "capabilities": {"limits": {"max_prompt_tokens": 400_000}}}
    ]}))
    .into_response()
}

async fn messages(
    AxumState(up): AxumState<Arc<Upstream>>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    up.messages.fetch_add(1, Ordering::SeqCst);
    let bearer = bearer(&headers);
    *up.messages_bearer.lock().unwrap() = bearer.clone();
    if !bearer_ok(&bearer) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some(location) = up.messages_redirect.lock().unwrap().clone() {
        return (StatusCode::FOUND, [("location", location)]).into_response();
    }
    if up.messages_402.load(Ordering::SeqCst) {
        return StatusCode::PAYMENT_REQUIRED.into_response();
    }
    let initiator = headers
        .get("x-initiator")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    *up.seen.lock().unwrap() = Some((
        initiator,
        serde_json::from_str(&body).unwrap_or(Value::Null),
    ));
    axum::Json(json!({"usage": {"cache_read_input_tokens": 7, "cache_creation_input_tokens": 1}}))
        .into_response()
}

struct Rig {
    dir: PathBuf,
    port: u16,
    upstream: Arc<Upstream>,
    proxy: Arc<Proxy>,
    http: reqwest::Client,
}

impl Rig {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn launches(&self) -> usize {
        std::fs::read_to_string(self.dir.join("launches"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    async fn status(&self) -> Status {
        self.http
            .get(self.url("/__pij/status"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn message(&self, body: &Value) -> StatusCode {
        self.http
            .post(self.url("/v1/messages"))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
            .status()
    }
}

/// A fake gh: logs each launch, fails while `fail` exists, prints the account's
/// fixture token, and parks at a barrier while `hold-<account>` exists (writing
/// `parked-<account>`) until `release-<account>` appears.
fn fake_gh(dir: &Path) -> PathBuf {
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\nd='{dir}'\necho \"$*\" >> \"$d/launches\"\n\
             if [ -f \"$d/hold-$4\" ]; then touch \"$d/parked-$4\"; while [ ! -f \"$d/release-$4\" ]; do sleep 0.02; done; fi\n\
             [ -f \"$d/fail\" ] && {{ echo 'no oauth token found' >&2; exit 1; }}\n\
             echo gho_tok_$4\n",
            dir = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    gh
}

async fn rig(name: &str, account: Option<&str>, stale_token: bool) -> Rig {
    let dir =
        std::env::temp_dir().join(format!("pij-cache-proxy-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(account) = account {
        config::save(
            &dir,
            &ProxyConfig {
                account: Some(account.to_string()),
                enabled: true,
            },
        )
        .unwrap();
    }

    let upstream = Arc::new(Upstream::default());
    let fake = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake_base = format!("http://{}", fake.local_addr().unwrap());
    *upstream.base.lock().unwrap() = fake_base.clone();
    let app = Router::new()
        .route("/copilot_internal/user", get(user))
        .route("/models", get(models))
        .route("/v1/messages", post(messages))
        .with_state(upstream.clone());
    tokio::spawn(async move { axum::serve(fake, app).await.unwrap() });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let settings = Settings {
        state_dir: dir.clone(),
        port,
        github_api: fake_base,
        gh: Ok(fake_gh(&dir)),
        stale_token,
    };
    let proxy = Arc::new(Proxy::start(settings).await);
    tokio::spawn(serve(
        listener,
        router(proxy.clone()),
        std::future::pending(),
    ));
    Rig {
        dir,
        port,
        upstream,
        proxy,
        http: reqwest::Client::new(),
    }
}

fn hello() -> Value {
    json!({"model": "claude-opus-5.5", "messages": [{"role": "user", "content": "hi"}]})
}

#[tokio::test]
async fn a_ready_account_reports_its_endpoint_models_and_one_gh_launch() {
    let rig = rig("ready", Some("tester"), false).await;
    let status = rig.status().await;
    assert_eq!(status.state, State::Ready, "{status:?}");
    assert!(status.ready);
    assert_eq!(status.reason, "");
    assert_eq!(status.account.as_deref(), Some("tester"));
    let base = rig.upstream.base.lock().unwrap().clone();
    assert_eq!(
        status.upstream_host.as_deref(),
        base.strip_prefix("http://")
    );
    assert_eq!(
        status.models.keys().collect::<Vec<_>>(),
        ["claude-opus-5.5"]
    );
    assert!(status.quota.usable);
    assert_eq!(status.counters.gh_launches, 1);
    assert_eq!(rig.launches(), 1);
    assert_eq!(status.pid, std::process::id());
    // The fake gh saw the account as a separate argument, never the token.
    let argv = std::fs::read_to_string(rig.dir.join("launches")).unwrap();
    assert_eq!(argv.trim(), "auth token --user tester");
}

#[tokio::test]
async fn five_concurrent_401s_share_one_refresh_and_all_succeed() {
    let rig = Arc::new(rig("single-flight", Some("tester"), true).await);
    let since = rig.status().await.since;
    let calls: Vec<_> = (0..5)
        .map(|_| {
            let rig = rig.clone();
            tokio::spawn(async move { rig.message(&hello()).await })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.unwrap(), StatusCode::OK);
    }
    // One launch at start, exactly one for the refresh.
    assert_eq!(rig.launches(), 2);
    let status = rig.status().await;
    assert_eq!(status.counters.refreshes, 1);
    assert_eq!(status.counters.gh_launches, 2);
    assert_eq!(status.counters.requests, 5);
    assert!(
        (1..=5).contains(&status.counters.upstream_401),
        "{:?}",
        status.counters
    );
    assert_eq!(status.state, State::Ready);
    // A successful refresh is not a state change: the daemon would notify on one.
    assert_eq!(status.since, since, "the state flapped across the refresh");
}

#[tokio::test]
async fn a_failed_refresh_latches_until_a_reload_and_never_loops() {
    let rig = rig("latch", Some("tester"), true).await;
    std::fs::write(rig.dir.join("fail"), "").unwrap();
    assert_eq!(rig.message(&hello()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(rig.launches(), 2, "start plus one failed refresh");
    let status = rig.status().await;
    assert_eq!(status.state, State::NoToken);
    assert!(!status.ready);
    assert!(
        status
            .reason
            .contains("token refresh failed, keeping the current token"),
        "{}",
        status.reason
    );
    let since = status.since;
    for _ in 0..3 {
        assert_eq!(rig.message(&hello()).await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(rig.launches(), 2, "no refresh after a failed one");
    assert_eq!(
        rig.status().await.since,
        since,
        "one state change, not one per 401"
    );

    std::fs::remove_file(rig.dir.join("fail")).unwrap();
    let reloaded: Status = rig
        .http
        .post(rig.url("/__pij/reload"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reloaded.state, State::Ready, "{reloaded:?}");
    assert_eq!(rig.launches(), 3);
    assert_eq!(rig.message(&hello()).await, StatusCode::OK);
}

#[tokio::test]
async fn the_rewrite_reaches_upstream() {
    let rig = rig("rewrite", Some("tester"), false).await;
    let body = json!({
        "model": "claude-opus-5.5-1m",
        "system": [{"type": "text", "text": "s", "cache_control": {"type": "ephemeral"}}],
        "messages": [
            {"role": "assistant", "content": [{"type": "tool_use", "id": "1", "name": "t", "input": {}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "1", "content": "ok"}]}
        ]
    });
    assert_eq!(rig.message(&body).await, StatusCode::OK);
    let (initiator, seen) = rig.upstream.seen.lock().unwrap().clone().unwrap();
    assert_eq!(initiator, "agent");
    assert_eq!(seen["model"], "claude-opus-5.5");
    assert_eq!(seen["system"][0]["cache_control"]["ttl"], "1h");

    // req-14: a subagent's request keeps the 5m default, and the client's own
    // X-Initiator reaches upstream even though the body reads as a continuation.
    let response = rig
        .http
        .post(rig.url("/v1/messages"))
        .header("content-type", "application/json")
        .header("x-interaction-type", "conversation-subagent")
        .header("x-initiator", "user")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (initiator, seen) = rig.upstream.seen.lock().unwrap().clone().unwrap();
    assert_eq!(initiator, "user");
    assert_eq!(seen["model"], "claude-opus-5.5");
    assert!(
        seen["system"][0]["cache_control"].get("ttl").is_none(),
        "{seen}"
    );
}

/// #239 F002: a 402 latches `quota_exhausted` even while the user snapshot
/// (which lags) still says usable; only a 2xx, the reset date passing or a
/// reload clears it.
#[tokio::test]
async fn a_402_latches_until_a_2xx_the_reset_date_or_a_reload() {
    let rig = rig("quota", Some("tester"), false).await;
    let exhausted = |status: &Status| {
        status.state == State::QuotaExhausted && !status.ready && !status.quota.usable
    };

    // The snapshot says usable (remaining 100); /v1/messages refuses.
    rig.upstream.messages_402.store(true, Ordering::SeqCst);
    assert_eq!(rig.message(&hello()).await, StatusCode::PAYMENT_REQUIRED);
    for read in 0..2 {
        let status = rig.status().await;
        assert!(exhausted(&status), "read {read}: {status:?}");
        assert!(status.reason.contains("402"), "{}", status.reason);
        assert_eq!(status.quota.reset.as_deref(), Some("2999-12-01"));
        assert!(status.fix.contains("quota reset"), "{}", status.fix);
    }
    assert_eq!(rig.status().await.counters.upstream_402, 1);

    // (a) A 2xx from /v1/messages clears it.
    rig.upstream.messages_402.store(false, Ordering::SeqCst);
    assert_eq!(rig.message(&hello()).await, StatusCode::OK);
    let status = rig.status().await;
    assert_eq!(status.state, State::Ready, "(a) {status:?}");
    assert!(status.quota.usable);

    // (c) A reload clears it.
    rig.upstream.messages_402.store(true, Ordering::SeqCst);
    assert_eq!(rig.message(&hello()).await, StatusCode::PAYMENT_REQUIRED);
    assert!(exhausted(&rig.status().await));
    let status = rig.proxy.reload().await;
    assert_eq!(status.state, State::Ready, "(c) {status:?}");

    // (b) The snapshot's reset date passing clears it.
    assert_eq!(rig.message(&hello()).await, StatusCode::PAYMENT_REQUIRED);
    assert!(exhausted(&rig.status().await));
    "2000-01-01".clone_into(&mut rig.upstream.reset_date.lock().unwrap());
    rig.upstream.messages_402.store(true, Ordering::SeqCst);
    assert_eq!(rig.message(&hello()).await, StatusCode::PAYMENT_REQUIRED);
    let status = rig.status().await;
    assert_eq!(status.state, State::Ready, "(b) {status:?}");

    // A snapshot that itself says exhausted still refuses, with its reset date.
    rig.upstream.messages_402.store(false, Ordering::SeqCst);
    rig.upstream.user_exhausted.store(true, Ordering::SeqCst);
    "2999-12-01".clone_into(&mut rig.upstream.reset_date.lock().unwrap());
    let status = rig.proxy.reload().await;
    assert!(exhausted(&status), "{status:?}");
    assert_eq!(
        status.reason,
        "premium quota exhausted for tester until 2999-12-01"
    );
}

/// #239 F001: a refresh started for account A must not land after a reload has
/// switched to account B. A's refresh is parked at a barrier in the fake gh
/// while B's reload completes, then released.
#[tokio::test]
async fn an_old_accounts_refresh_never_replaces_the_new_accounts_token() {
    let rig = Arc::new(rig("account-race", Some("acct_a"), true).await);
    std::fs::write(rig.dir.join("hold-acct_a"), "").unwrap();
    let in_flight = {
        let rig = rig.clone();
        tokio::spawn(async move { rig.message(&hello()).await })
    };
    // Wait for A's refresh to reach the barrier (its request got the 401).
    let parked = rig.dir.join("parked-acct_a");
    for _ in 0..500 {
        if parked.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(parked.exists(), "A's refresh never reached the barrier");

    config::save(
        &rig.dir,
        &ProxyConfig {
            account: Some("acct_b".into()),
            enabled: true,
        },
    )
    .unwrap();
    let status = rig.proxy.reload().await;
    assert_eq!(status.account.as_deref(), Some("acct_b"));
    assert_eq!(status.state, State::Ready, "{status:?}");
    assert_eq!(
        *rig.upstream.user_bearer.lock().unwrap(),
        format!("Bearer {}", token_for("acct_b"))
    );

    std::fs::write(rig.dir.join("release-acct_a"), "").unwrap();
    assert_eq!(in_flight.await.unwrap(), StatusCode::OK);
    let b = format!("Bearer {}", token_for("acct_b"));
    assert_eq!(
        *rig.upstream.messages_bearer.lock().unwrap(),
        b,
        "the retry used A's token"
    );

    assert_eq!(rig.message(&hello()).await, StatusCode::OK);
    assert_eq!(*rig.upstream.messages_bearer.lock().unwrap(), b);
    let status = rig.status().await;
    assert_eq!(status.account.as_deref(), Some("acct_b"));
    assert_eq!(status.state, State::Ready);
    assert_eq!(
        *rig.upstream.user_bearer.lock().unwrap(),
        b,
        "readiness read with A's token"
    );
}

/// Security note on #239: the upstream client follows no redirect, so the
/// bearer token can't be carried to another host.
#[tokio::test]
async fn an_upstream_redirect_is_not_followed() {
    let rig = rig("redirect", Some("tester"), false).await;
    let hits = Arc::new(AtomicU64::new(0));
    let elsewhere = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let elsewhere_url = format!("http://{}/steal", elsewhere.local_addr().unwrap());
    let counter = hits.clone();
    let app = Router::new().fallback(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            StatusCode::OK
        }
    });
    tokio::spawn(async move { axum::serve(elsewhere, app).await.unwrap() });
    *rig.upstream.messages_redirect.lock().unwrap() = Some(elsewhere_url);
    assert_eq!(rig.message(&hello()).await, StatusCode::FOUND);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the redirect was followed");
}

#[tokio::test]
async fn a_browser_origin_or_foreign_host_is_refused_before_forwarding() {
    let rig = rig("guard", Some("tester"), false).await;
    let with_origin = rig
        .http
        .post(rig.url("/v1/messages"))
        .header("origin", "https://evil.example")
        .body(hello().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(with_origin.status(), StatusCode::FORBIDDEN);
    let foreign_host = rig
        .http
        .post(rig.url("/v1/messages"))
        .header("host", format!("evil.example:{}", rig.port))
        .body(hello().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(foreign_host.status(), StatusCode::FORBIDDEN);
    let status_from_page = rig
        .http
        .get(rig.url("/__pij/status"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(status_from_page.status(), StatusCode::FORBIDDEN);
    assert_eq!(rig.upstream.messages.load(Ordering::SeqCst), 0);
    // localhost:<port> is loopback too.
    let localhost = rig
        .http
        .post(rig.url("/v1/messages"))
        .header("host", format!("localhost:{}", rig.port))
        .body(hello().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(localhost.status(), StatusCode::OK);
}

#[tokio::test]
async fn no_account_is_not_ready_launches_nothing_and_refuses_requests() {
    let rig = rig("no-account", None, false).await;
    let status = rig.status().await;
    assert_eq!(status.state, State::NoAccount);
    assert_eq!(status.fix, "pij-rs cache-proxy account <login>");
    assert_eq!(status.account, None);
    assert_eq!(rig.launches(), 0);
    let response = rig
        .http
        .post(rig.url("/v1/messages"))
        .body(hello().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no_account"),
        "{body}"
    );
}

#[tokio::test]
async fn off_stops_new_spawns_but_keeps_serving_live_seats() {
    let rig = rig("off", Some("tester"), false).await;
    config::save(
        &rig.dir,
        &ProxyConfig {
            account: Some("tester".into()),
            enabled: false,
        },
    )
    .unwrap();
    let status = rig.proxy.reload().await;
    assert_eq!(status.state, State::Off);
    assert!(!status.ready);
    assert_eq!(status.fix, "pij-rs cache-proxy on");
    assert_eq!(rig.message(&hello()).await, StatusCode::OK);
    assert_eq!(rig.launches(), 1, "off then serve launches nothing");
}
