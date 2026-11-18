//! The sidecar's state and behaviour: the held token, readiness, the derived
//! status, and forwarding (blueprint 003 c-proxy-core, c-token-holder,
//! c-readiness, c-status-route).
//!
//! Invariants:
//! - the token lives only in [`Proxy::token`]; nothing logs, writes or returns it;
//! - `gh` is launched at start, on an account change, on a reload with no
//!   usable token, and once per 401 burst (single-flight); a failed refresh
//!   latches until the next reload, so it can never loop;
//! - readiness is re-run lazily: a status read whose last answer has aged out,
//!   or that follows a refresh or a 402, starts one background check
//!   (single-flight on `check_lock`) and answers at once from the cached
//!   state, so a status read never waits on GitHub; the sidecar has no timer;
//! - an upstream 402 or 2xx moves the quota latch only when the token it was
//!   sent with is still the one held (same account and generation).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tokio_stream::StreamExt;

use crate::config::{self, ProxyConfig};
use crate::readiness::{self, CHECK_TTL, COPILOT_VERSION, Failure, GITHUB_API};
use crate::rewrite;
use crate::status::{Counters, ModelLimit, Quota, State, Status};
use crate::token;

/// The sidecar's port (PRD 004 q-03).
pub const DEFAULT_PORT: u16 = 7471;

/// Test-only: the GitHub API base, so tests can stand up a fake one.
pub const GITHUB_API_ENV: &str = "PIJ_CACHE_PROXY_GITHUB_API";

/// Test-only: swap the token for a stale one after the start-up check, so the
/// first request takes the 401 path (matrix rows 9 and 10).
pub const STALE_TOKEN_ENV: &str = "PIJ_CACHE_PROXY_TEST_STALE_TOKEN";

/// The largest request body forwarded.
pub const MAX_BODY: usize = 64 << 20;

/// How long a failed readiness answer stands before a status read re-runs it.
const FAILED_CHECK_TTL: Duration = Duration::from_secs(60);

/// Request headers passed through from the client.
const PASS_HEADERS: [&str; 4] = [
    "anthropic-version",
    "anthropic-beta",
    "content-type",
    "accept",
];

/// Response headers passed back to the client.
const RETURN_HEADERS: [&str; 4] = ["content-type", "request-id", "x-request-id", "retry-after"];

/// What the sidecar is started with. Read once; nothing per request.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The pij-rs state directory holding `cache-proxy.toml`.
    pub state_dir: PathBuf,
    /// The loopback port.
    pub port: u16,
    /// `https://api.github.com`, or a test override.
    pub github_api: String,
    /// gh, resolved once.
    pub gh: Result<PathBuf, String>,
    /// [`STALE_TOKEN_ENV`].
    pub stale_token: bool,
}

impl Settings {
    /// Read the environment once: `PIJ_GH`, `PATH`, and the two test knobs.
    #[must_use]
    pub fn from_env(state_dir: PathBuf, port: u16) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        Self {
            state_dir,
            port,
            github_api: std::env::var(GITHUB_API_ENV)
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| GITHUB_API.to_string()),
            gh: token::resolve_gh(
                std::env::var_os(token::GH_ENV).as_deref(),
                std::env::var_os("PATH").as_deref(),
                &cwd,
            ),
            stale_token: std::env::var_os(STALE_TOKEN_ENV) == Some(OsString::from("1")),
        }
    }

    /// Only a test GitHub API may name a plain-http endpoint.
    fn allow_http(&self) -> bool {
        self.github_api != GITHUB_API
    }
}

/// The token in memory, tagged with a generation so concurrent 401s share one
/// refresh.
struct Held {
    generation: u64,
    token: String,
    account: String,
}

/// Everything the status is derived from. Guarded by a std mutex: no await
/// while it is held.
struct Inner {
    config: Option<ProxyConfig>,
    config_error: Option<String>,
    token_failure: Option<Failure>,
    refresh_failed: Option<String>,
    check_failure: Option<Failure>,
    checked_at: Option<Instant>,
    /// `checked_at` as epoch milliseconds, for the status body.
    checked_at_ms: Option<u64>,
    /// Re-run readiness on the next status read (after a refresh, a new token
    /// or a 402) while the last answer still stands, so the state never flaps.
    recheck: bool,
    /// A 402 seen from upstream. It latches `quota_exhausted`: a lagging
    /// `copilot_internal/user` snapshot that still says usable never clears it.
    /// Cleared only by a 2xx from `/v1/messages`, the snapshot's reset date
    /// passing, or a reload (pm-004's ruling on #239 F002).
    refused_402: Option<String>,
    upstream: Option<String>,
    quota: Quota,
    models: BTreeMap<String, ModelLimit>,
    state: State,
    reason: String,
    since: u64,
}

/// The sidecar.
pub struct Proxy {
    settings: Settings,
    http: reqwest::Client,
    token: RwLock<Option<Held>>,
    refresh_lock: Mutex<()>,
    /// One readiness check at a time. An `Arc` so a status read can hand its
    /// `try_lock_owned` guard to the background check it starts.
    check_lock: Arc<Mutex<()>>,
    /// One reload at a time, so two account changes can't install out of order.
    reload_lock: Mutex<()>,
    inner: StdMutex<Inner>,
    requests: AtomicU64,
    refreshes: AtomicU64,
    gh_launches: AtomicU64,
    upstream_401: AtomicU64,
    upstream_402: AtomicU64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One log line on stderr, timestamped. Never pass a header value or token.
pub fn log(line: &str) {
    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    eprintln!("{now} pij-rs cache-proxy: {line}");
}

/// The fix for each state.
fn fix(state: State, account: Option<&str>) -> String {
    let account = account.unwrap_or("<login>");
    match state {
        State::Ready => String::new(),
        State::Off => "pij-rs cache-proxy on".into(),
        State::NoAccount => "pij-rs cache-proxy account <login>".into(),
        State::GhMissing => {
            "install gh (brew install gh) or set PIJ_GH to its absolute path, then restart the sidecar (just cache-proxy-autostart)".into()
        }
        State::NoToken => format!("gh auth login (as {account}), then pij-rs cache-proxy on"),
        State::QuotaExhausted => {
            "wait for the quota reset, or pij-rs cache-proxy account <another login>".into()
        }
        State::UpstreamUnreachable => {
            "check the network; pij-rs cache-proxy on re-runs the check".into()
        }
    }
}

impl Inner {
    fn account(&self) -> Option<&str> {
        self.config
            .as_ref()
            .and_then(|config| config.account.as_deref())
    }

    /// The state the facts add up to, most fundamental first.
    fn derive(&self, settings: &Settings, token_held: bool) -> (State, String) {
        if let Some(error) = &self.config_error {
            return (State::NoAccount, format!("config unreadable: {error}"));
        }
        if self.config.as_ref().is_some_and(|config| !config.enabled) {
            return (State::Off, "switched off (pij-rs cache-proxy off)".into());
        }
        let Some(account) = self.account() else {
            return (
                State::NoAccount,
                format!(
                    "no account in {}",
                    config::path(&settings.state_dir).display()
                ),
            );
        };
        if let Some(failure) = &self.token_failure {
            return (failure.state, failure.reason.clone());
        }
        if !token_held {
            return (State::NoToken, format!("no token held for {account}"));
        }
        if let Some(error) = &self.refresh_failed {
            return (
                State::NoToken,
                format!("token refresh failed, keeping the current token: {error}"),
            );
        }
        if let Some(reason) = &self.refused_402 {
            return (State::QuotaExhausted, reason.clone());
        }
        if let Some(failure) = &self.check_failure {
            return (failure.state, failure.reason.clone());
        }
        if self.checked_at.is_none() || self.upstream.is_none() {
            return (
                State::UpstreamUnreachable,
                "readiness not checked yet".into(),
            );
        }
        (State::Ready, String::new())
    }
}

impl Proxy {
    /// A sidecar that has loaded nothing yet.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        // No redirects: a redirect must never carry the bearer token off the
        // endpoint `copilot_internal/user` named (security note on #239). No
        // fallback to a default client, which would follow redirects: a TLS
        // backend that can't initialise stops the sidecar at start.
        #[allow(clippy::expect_used)]
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the upstream HTTP client must build");
        Self {
            settings,
            http,
            token: RwLock::new(None),
            refresh_lock: Mutex::new(()),
            check_lock: Arc::new(Mutex::new(())),
            reload_lock: Mutex::new(()),
            inner: StdMutex::new(Inner {
                config: None,
                config_error: None,
                token_failure: None,
                refresh_failed: None,
                check_failure: None,
                checked_at: None,
                checked_at_ms: None,
                recheck: false,
                refused_402: None,
                upstream: None,
                quota: Quota::default(),
                models: BTreeMap::new(),
                state: State::NoAccount,
                reason: String::new(),
                since: now_ms(),
            }),
            requests: AtomicU64::new(0),
            refreshes: AtomicU64::new(0),
            gh_launches: AtomicU64::new(0),
            upstream_401: AtomicU64::new(0),
            upstream_402: AtomicU64::new(0),
        }
    }

    /// Load the config, fetch the token and run readiness, then apply the
    /// stale-token test knob.
    pub async fn start(settings: Settings) -> Self {
        let proxy = Self::new(settings);
        match &proxy.settings.gh {
            Ok(gh) => log(&format!("gh: {}", gh.display())),
            Err(error) => log(&format!("gh: missing ({error})")),
        }
        proxy.reload().await;
        if proxy.settings.stale_token
            && let Some(held) = proxy.token.write().await.as_mut()
        {
            "gho_stale_test_token".clone_into(&mut held.token);
            log("test knob: holding a stale token, the next request takes the 401 path");
        }
        proxy
    }

    /// The settings it was started with.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Recompute the state; log and stamp `since` only when it changes.
    async fn settle(&self) {
        let token_held = {
            let held = self.token.read().await;
            let inner = self.inner();
            held.as_ref()
                .is_some_and(|held| Some(held.account.as_str()) == inner.account())
        };
        let mut inner = self.inner();
        let (state, reason) = inner.derive(&self.settings, token_held);
        if state != inner.state || reason != inner.reason {
            if state != inner.state {
                inner.since = now_ms();
            }
            log(&format!(
                "state {} -> {}{}",
                inner.state.as_str(),
                state.as_str(),
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(": {reason}")
                }
            ));
            inner.state = state;
            inner.reason = reason;
        }
    }

    /// Re-read the config; fetch the token when the account changed, none is
    /// held, or the last fetch or refresh failed; re-run readiness. A reload
    /// also clears a latched 402 (F002 recovery (c)).
    pub async fn reload(&self) -> Status {
        let _one = self.reload_lock.lock().await;
        let (config, config_error) = match config::load(&self.settings.state_dir) {
            Ok(config) => (config, None),
            Err(error) => (None, Some(error)),
        };
        let account = config.as_ref().and_then(|config| config.account.clone());
        let held_account = self
            .token
            .read()
            .await
            .as_ref()
            .map(|held| held.account.clone());
        let must_fetch = {
            let mut inner = self.inner();
            let failed = inner.token_failure.is_some() || inner.refresh_failed.is_some();
            inner.config = config;
            inner.config_error = config_error;
            inner.refresh_failed = None;
            inner.refused_402 = None;
            account.is_some() && (held_account != account || failed)
        };
        match &account {
            None => {
                *self.token.write().await = None;
                let mut inner = self.inner();
                inner.token_failure = None;
                inner.check_failure = None;
                inner.checked_at = None;
                inner.checked_at_ms = None;
            }
            Some(account) if must_fetch => self.fetch_token(account).await,
            Some(_) => {}
        }
        self.run_check(true).await;
        self.snapshot().await
    }

    /// Fetch `account`'s token at start, on an account change or on reload.
    /// A failure keeps whatever token is held, so live seats keep working.
    async fn fetch_token(&self, account: &str) {
        let gh = match &self.settings.gh {
            Ok(gh) => gh.clone(),
            Err(error) => {
                self.inner().token_failure = Some(Failure {
                    state: State::GhMissing,
                    reason: error.clone(),
                });
                return;
            }
        };
        self.gh_launches.fetch_add(1, Ordering::Relaxed);
        match token::fetch(&gh, account).await {
            Ok(token) => {
                let mut held = self.token.write().await;
                let generation = held.as_ref().map_or(0, |held| held.generation + 1);
                *held = Some(Held {
                    generation,
                    token,
                    account: account.to_string(),
                });
                drop(held);
                let mut inner = self.inner();
                inner.token_failure = None;
                inner.recheck = true;
                log(&format!(
                    "token fetched for {account} (gh launches {})",
                    self.gh_launches.load(Ordering::Relaxed)
                ));
            }
            Err(error) => {
                log(&format!("token fetch failed: {error}"));
                self.inner().token_failure = Some(Failure {
                    state: State::NoToken,
                    reason: error,
                });
            }
        }
    }

    /// Whether readiness is owed: never run, aged out, or a recheck is owed.
    fn check_due(&self) -> bool {
        let inner = self.inner();
        let ttl = if inner.check_failure.is_some() {
            FAILED_CHECK_TTL
        } else {
            CHECK_TTL
        };
        inner.recheck || inner.checked_at.is_none_or(|at| at.elapsed() >= ttl)
    }

    /// Run readiness now when forced, else only if it is owed. One at a time;
    /// a waiter that finds a fresh answer runs nothing.
    async fn run_check(&self, force: bool) {
        let _one = self.check_lock.lock().await;
        self.check_locked(force).await;
    }

    /// The check itself. The caller holds `check_lock`.
    async fn check_locked(&self, force: bool) {
        if !force && !self.check_due() {
            return;
        }
        // The owed recheck is consumed as the check starts, so one asked for
        // while it runs (a 402, a refresh) stays owed for the next read.
        let (token, account, generation) = {
            let held = self.token.read().await;
            let mut inner = self.inner();
            match held.as_ref() {
                Some(held) if Some(held.account.as_str()) == inner.account() => {
                    inner.recheck = false;
                    (held.token.clone(), held.account.clone(), held.generation)
                }
                _ => return,
            }
        };
        let facts = readiness::check(
            &self.http,
            &self.settings.github_api,
            &token,
            &account,
            self.settings.allow_http(),
        )
        .await;
        drop(token);
        // The facts belong to the token they were read with: if a reload or a
        // refresh replaced it meanwhile, drop them and leave the re-check owed.
        let current = self
            .token
            .read()
            .await
            .as_ref()
            .is_some_and(|held| held.account == account && held.generation == generation);
        let mut inner = self.inner();
        if !current || inner.account() != Some(account.as_str()) {
            inner.recheck = true;
            return;
        }
        if let Some(api) = facts.api {
            if inner.upstream.as_deref() != Some(api.as_str()) {
                log(&format!(
                    "upstream for {account}: {}",
                    readiness::host_of(&api).unwrap_or_default()
                ));
            }
            inner.upstream = Some(api);
            inner.quota = facts.quota;
            if facts
                .failure
                .as_ref()
                .is_none_or(|failure| failure.state != State::UpstreamUnreachable)
            {
                inner.models = facts.models;
            }
        }
        if inner.refused_402.is_some() {
            // A usable snapshot never clears a 402 (it lags); the reset date
            // passing does (F002 recovery (b)).
            let today = time::OffsetDateTime::now_utc().date().to_string();
            let reset_passed = inner
                .quota
                .reset
                .as_deref()
                .and_then(|reset| reset.get(..10))
                .is_some_and(|reset| today.as_str() >= reset);
            if reset_passed {
                log("quota reset date has passed: clearing the 402");
                inner.refused_402 = None;
            } else {
                inner.quota.usable = false;
            }
        }
        inner.check_failure = facts.failure;
        inner.checked_at = Some(Instant::now());
        inner.checked_at_ms = Some(now_ms());
    }

    /// The status route's body, from the cached readiness: it never waits on
    /// upstream (F204). When readiness is owed it starts one background check,
    /// unless a check is already running (a reload's included), and answers
    /// with the current state; the check settles the state when it lands.
    pub async fn status(self: &Arc<Self>) -> Status {
        if self.check_due()
            && let Ok(one) = Arc::clone(&self.check_lock).try_lock_owned()
        {
            let proxy = Arc::clone(self);
            tokio::spawn(async move {
                proxy.check_locked(false).await;
                drop(one);
                proxy.settle().await;
            });
        }
        self.snapshot().await
    }

    /// The status body as it stands now.
    async fn snapshot(&self) -> Status {
        self.settle().await;
        let inner = self.inner();
        let account = inner.account().map(str::to_string);
        Status {
            ready: inner.state == State::Ready,
            state: inner.state,
            reason: inner.reason.clone(),
            fix: fix(inner.state, account.as_deref()),
            account,
            upstream_host: inner.upstream.as_deref().and_then(readiness::host_of),
            quota: inner.quota.clone(),
            models: inner.models.clone(),
            counters: Counters {
                requests: self.requests.load(Ordering::Relaxed),
                refreshes: self.refreshes.load(Ordering::Relaxed),
                gh_launches: self.gh_launches.load(Ordering::Relaxed),
                upstream_401: self.upstream_401.load(Ordering::Relaxed),
                upstream_402: self.upstream_402.load(Ordering::Relaxed),
            },
            pid: std::process::id(),
            since: inner.since,
            checked_at_ms: inner.checked_at_ms,
        }
    }

    /// After a 401 on generation `seen`: one refresh shared by every request
    /// that saw it. A request that finds a newer generation launches nothing;
    /// after a failed refresh nothing launches again until a reload. The result,
    /// success or failure, lands only if the holder is still the account and
    /// generation it was fetched for: a reload to another account meanwhile
    /// supersedes it (#239 F001).
    async fn refresh(&self, seen: u64) {
        let _one = self.refresh_lock.lock().await;
        let account = match self.token.read().await.as_ref() {
            Some(held) if held.generation == seen => held.account.clone(),
            _ => return,
        };
        if self.inner().refresh_failed.is_some() {
            return;
        }
        let Ok(gh) = self.settings.gh.clone() else {
            self.inner().refresh_failed = Some("gh is missing".into());
            self.settle().await;
            return;
        };
        self.refreshes.fetch_add(1, Ordering::Relaxed);
        self.gh_launches.fetch_add(1, Ordering::Relaxed);
        let fetched = token::fetch(&gh, &account).await;
        let mut held = self.token.write().await;
        let Some(current) = held
            .as_mut()
            .filter(|held| held.account == account && held.generation == seen)
        else {
            drop(held);
            log(&format!(
                "discarded a refresh for {account}: the token was replaced while it ran"
            ));
            return;
        };
        match fetched {
            Ok(token) => {
                current.generation += 1;
                current.token = token;
                drop(held);
                self.inner().recheck = true;
                log(&format!(
                    "token refreshed after a 401 (gh launches {})",
                    self.gh_launches.load(Ordering::Relaxed)
                ));
            }
            Err(error) => {
                drop(held);
                log(&format!(
                    "refresh failed, keeping the current token; no further refresh until a reload: {error}"
                ));
                self.inner().refresh_failed = Some(error);
            }
        }
        self.settle().await;
    }

    /// Apply `change` to the state only if the token a request was sent with
    /// (`account` at `generation`) is still the one held for the configured
    /// account; `None`, changing nothing, for an answer to a replaced token.
    /// The same rule as a refresh's install (#239 F001), for upstream answers.
    async fn if_sent_with_current<R>(
        &self,
        account: &str,
        generation: u64,
        change: impl FnOnce(&mut Inner) -> R,
    ) -> Option<R> {
        let held = self.token.read().await;
        let current = held
            .as_ref()
            .is_some_and(|held| held.account == account && held.generation == generation);
        let mut inner = self.inner();
        (current && inner.account() == Some(account)).then(|| change(&mut inner))
    }

    /// A 402 latches `quota_exhausted` until a 2xx, the reset date or a reload,
    /// if it answered the token still held; a stale one changes nothing.
    async fn quota_refused(&self, account: &str, generation: u64) {
        let latched = self
            .if_sent_with_current(account, generation, |inner| {
                inner.quota.usable = false;
                inner.refused_402 = Some(format!(
                    "upstream answered 402: premium quota exhausted for {account}"
                ));
                // Re-read the snapshot for its reset date; it can't clear the latch.
                inner.recheck = true;
            })
            .await;
        if latched.is_none() {
            log(&format!(
                "ignored a 402 for {account}: the token it was sent with has been replaced"
            ));
            return;
        }
        self.settle().await;
    }

    /// A 2xx from `/v1/messages` proves the quota is usable again (F002
    /// recovery (a)), if it answered the token still held.
    async fn quota_served(&self, account: &str, generation: u64) {
        let cleared = self
            .if_sent_with_current(account, generation, |inner| {
                let cleared = inner.refused_402.take().is_some();
                if cleared {
                    inner.quota.usable = true;
                    inner.recheck = true;
                }
                cleared
            })
            .await;
        match cleared {
            Some(true) => {
                log("upstream served a request after a 402: clearing it");
                self.settle().await;
            }
            Some(false) => {}
            None => log(&format!(
                "a 2xx for {account} clears nothing: the token it was sent with has been replaced"
            )),
        }
    }

    /// Forward one request upstream with the held token.
    pub async fn forward(
        &self,
        method: Method,
        path: &str,
        headers: &HeaderMap,
        raw: Bytes,
    ) -> Response {
        let (has_upstream, state, account) = {
            let inner = self.inner();
            (
                inner.upstream.is_some(),
                inner.state,
                inner.account().map(str::to_string),
            )
        };
        let token_held = self.token.read().await.is_some();
        if !has_upstream || !token_held {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!(
                    "the pij cache proxy holds no token or endpoint (state {}); fix: {}",
                    state.as_str(),
                    fix(state, account.as_deref())
                ),
            );
        }
        // req-14: the client's own X-Initiator wins; a subagent keeps the 5m cache.
        let client =
            rewrite::client_initiator(headers.get("x-initiator").map(HeaderValue::as_bytes));
        let agent = rewrite::Agent::from_interaction_type(
            headers.get("x-interaction-type").map(HeaderValue::as_bytes),
        );
        let is_messages = method == Method::POST && path.starts_with("/v1/messages");
        // Move the rewritten body into `Bytes`, keeping only the metadata
        // (PERF-239-01); the retry's `Bytes::clone` shares the payload.
        let (body, initiator, model, ttl_rewrites) = if is_messages {
            let rewrite::Rewritten {
                body,
                initiator,
                model,
                ttl_rewrites,
            } = rewrite::rewrite(&raw, client, agent);
            (
                body.map_or(raw, Bytes::from),
                initiator,
                model,
                ttl_rewrites,
            )
        } else {
            (raw, client.unwrap_or("user"), String::new(), 0)
        };
        self.requests.fetch_add(1, Ordering::Relaxed);

        let mut retried = false;
        let mut host: String;
        // Each attempt's account and token generation travel with its answer,
        // so a 402 or 2xx is applied only to the token it answered.
        let (response, sent_account, sent_generation) = loop {
            let (generation, account, bearer) = match self.token.read().await.as_ref() {
                Some(held) => (
                    held.generation,
                    held.account.clone(),
                    HeaderValue::from_str(&format!("Bearer {}", held.token)),
                ),
                None => {
                    return error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "the pij cache proxy no longer holds a token",
                    );
                }
            };
            // The endpoint read with this attempt's token, so a retry after an
            // account switch goes to the new account's endpoint.
            let Some(upstream) = self.inner().upstream.clone() else {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the pij cache proxy no longer knows an endpoint",
                );
            };
            let url = format!("{}{path}", upstream.trim_end_matches('/'));
            host = readiness::host_of(&upstream).unwrap_or_default();
            let Ok(mut bearer) = bearer else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the held token is not a valid header value",
                );
            };
            bearer.set_sensitive(true);
            let mut out = HeaderMap::new();
            for name in PASS_HEADERS {
                if let Some(value) = headers.get(name) {
                    out.insert(name, value.clone());
                }
            }
            out.insert("authorization", bearer);
            for (name, value) in [
                ("user-agent", COPILOT_VERSION),
                ("editor-version", COPILOT_VERSION),
                ("copilot-integration-id", "copilot-developer-cli"),
                ("copilot-harness-id", "copilot-sdk"),
                ("openai-intent", "conversation-agent"),
                ("x-github-api-version", "2026-08-01"),
                ("x-initiator", initiator),
            ] {
                out.insert(name, HeaderValue::from_static(value));
            }
            let sent = self
                .http
                .request(method.clone(), &url)
                .headers(out)
                .body(body.clone())
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(error) => {
                    log(&format!(
                        "-> {method} {path} host={host} failed: {}",
                        error.without_url()
                    ));
                    return error_response(
                        StatusCode::BAD_GATEWAY,
                        &format!("upstream {host} unreachable"),
                    );
                }
            };
            if response.status() == StatusCode::UNAUTHORIZED {
                self.upstream_401.fetch_add(1, Ordering::Relaxed);
                if !retried {
                    retried = true;
                    self.refresh(generation).await;
                    // Retry once, and only with a newer token: after a failed or
                    // latched refresh the same token would just be refused again.
                    let newer = self
                        .token
                        .read()
                        .await
                        .as_ref()
                        .is_some_and(|held| held.generation != generation);
                    log(&format!(
                        "-> {method} {path} host={host} status=401: {}",
                        if newer {
                            "retrying once with the refreshed token"
                        } else {
                            "no newer token (refresh failed or latched until a reload); not retried"
                        }
                    ));
                    if newer {
                        continue;
                    }
                }
            }
            break (response, account, generation);
        };

        let status = response.status();
        if status == StatusCode::PAYMENT_REQUIRED {
            self.upstream_402.fetch_add(1, Ordering::Relaxed);
            self.quota_refused(&sent_account, sent_generation).await;
        } else if status.is_success() && is_messages {
            self.quota_served(&sent_account, sent_generation).await;
        }
        log(&format!(
            "-> {method} {path} host={host} model={model} agent={} initiator={initiator} ttl_rewrites={ttl_rewrites} status={}",
            agent.as_str(),
            status.as_u16()
        ));
        let mut builder = Response::builder().status(status);
        for name in RETURN_HEADERS {
            if let Some(value) = response.headers().get(name) {
                builder = builder.header(name, value);
            }
        }
        builder
            .body(Body::from_stream(usage_logged(response)))
            .unwrap_or_else(|_| {
                error_response(StatusCode::BAD_GATEWAY, "upstream response unreadable")
            })
    }
}

/// Stream the response through, logging the cache-usage counts from the first
/// usage block seen (never anything else from the body).
fn usage_logged(
    response: reqwest::Response,
) -> impl tokio_stream::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static {
    let mut logged = false;
    let mut head = String::new();
    response.bytes_stream().map(move |chunk| {
        if let Ok(bytes) = &chunk
            && !logged
        {
            head.push_str(&String::from_utf8_lossy(bytes));
            if head.contains("cache_creation_input_tokens") || head.len() > 64 << 10 {
                logged = true;
                log(&format!(
                    "   usage: cache_read={} cache_write={} 1h={} 5m={}",
                    number_after(&head, "cache_read_input_tokens"),
                    number_after(&head, "cache_creation_input_tokens"),
                    number_after(&head, "ephemeral_1h_input_tokens"),
                    number_after(&head, "ephemeral_5m_input_tokens"),
                ));
                head = String::new();
            }
        }
        chunk
    })
}

/// The integer after `"key":` in `haystack`, or `-`.
fn number_after(haystack: &str, key: &str) -> String {
    let Some(start) = haystack.find(key).map(|at| at + key.len()) else {
        return "-".into();
    };
    let digits: String = haystack[start..]
        .trim_start_matches(['"', ':', ' '])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        "-".into()
    } else {
        digits
    }
}

/// An Anthropic-shaped error, so the client shows the message.
#[must_use]
pub fn error_response(status: StatusCode, message: &str) -> Response {
    let body = json!({"type": "error", "error": {"type": "api_error", "message": message}});
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_numbers_are_read_from_the_first_usage_block() {
        let head = r#"event: message_start
data: {"usage":{"input_tokens":3,"cache_creation_input_tokens":51,"cache_read_input_tokens":48771,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":51}}}"#;
        assert_eq!(number_after(head, "cache_read_input_tokens"), "48771");
        assert_eq!(number_after(head, "cache_creation_input_tokens"), "51");
        assert_eq!(number_after(head, "ephemeral_1h_input_tokens"), "51");
        assert_eq!(number_after(head, "ephemeral_5m_input_tokens"), "0");
        assert_eq!(number_after(head, "missing"), "-");
    }

    #[test]
    fn each_state_but_ready_names_a_fix() {
        for state in State::ALL {
            assert_eq!(
                fix(state, Some("a")).is_empty(),
                state == State::Ready,
                "{state:?}"
            );
        }
    }

    /// F204's own path: readiness aged out by its TTL (an owed recheck is
    /// consumed as a check starts; a TTL stays due until the check lands) and
    /// `copilot_internal/user` stalled. Status reads answer at once, and
    /// `check_lock`'s try-lock lets only one of them start a check.
    #[tokio::test]
    async fn an_aged_out_answer_starts_one_background_check_for_concurrent_reads() {
        use std::sync::atomic::AtomicBool;

        let reads = Arc::new(AtomicU64::new(0));
        let stall = Arc::new(AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let user = {
            let (reads, stall, base) = (reads.clone(), stall.clone(), base.clone());
            move || {
                let (reads, stall, base) = (reads.clone(), stall.clone(), base.clone());
                async move {
                    reads.fetch_add(1, Ordering::SeqCst);
                    if stall.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    axum::Json(json!({
                        "endpoints": {"api": base},
                        "quota_snapshots": {"premium_interactions": {"remaining": 100}}
                    }))
                }
            }
        };
        let app = axum::Router::new()
            .route("/copilot_internal/user", axum::routing::get(user))
            .route(
                "/models",
                axum::routing::get(|| async { axum::Json(json!({"data": []})) }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await });

        let proxy = Arc::new(Proxy::new(Settings {
            state_dir: std::env::temp_dir(),
            port: 0,
            github_api: base,
            gh: Err("not launched in this test".into()),
            stale_token: false,
        }));
        proxy.inner().config = Some(ProxyConfig {
            account: Some("tester".into()),
            enabled: true,
        });
        *proxy.token.write().await = Some(Held {
            generation: 0,
            token: "gho_test".into(),
            account: "tester".into(),
        });
        proxy.run_check(true).await;
        assert_eq!(proxy.status().await.state, State::Ready);

        // Age the answer past its TTL, as ten minutes would.
        proxy.inner().checked_at = Instant::now().checked_sub(CHECK_TTL + Duration::from_secs(1));
        stall.store(true, Ordering::SeqCst);
        let before = reads.load(Ordering::SeqCst);
        let started = Instant::now();
        let (first, second) = tokio::join!(proxy.status(), proxy.status());
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!((first.state, second.state), (State::Ready, State::Ready));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(proxy.status().await.state, State::Ready, "still in flight");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            reads.load(Ordering::SeqCst) - before,
            1,
            "three reads, one check"
        );
    }
}
