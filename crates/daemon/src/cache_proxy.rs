//! The daemon side of the Copilot cache proxy (plan 229, workstream 004 s03;
//! blueprint 003 c-spawn-gate, c-proxy-watch).
//!
//! The sidecar (`pij-rs cache-proxy`, plan 228) holds the token and knows the
//! account, endpoint, quota and served models. The daemon never sees the token:
//! it reads only the sidecar's status route, `GET /__pij/status`, with its own
//! copy of that contract ([`ProxyStatus`]), so neither crate depends on the
//! other.
//!
//! - **Spawn gate** ([`CacheProxy::gate`]): a Copilot seat on a Claude model is
//!   wired to the proxy only when the status says `ready` and serves the
//!   model's base id. Anything else, including no answer within
//!   [`GATE_TIMEOUT`], launches plain with the reason and fix (d-6).
//! - **Watch** ([`CacheProxy::tick`]): one status read every
//!   [`WATCH_INTERVAL`] (d-7). Each change of state is one notice to each live
//!   prime, never a repeat (F192); [`transition`] decides.
//!
//! Cost (blueprint k-4, k-5): one loopback GET per Copilot spawn and one per
//! watch tick plus one in-process registry read; no process is launched.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use pij_core::model::{
    CACHE_PROXY_OPTED_OUT, CacheProxyRoute, Harness, SeatCacheProxy, SeatDescriptor,
};
use pij_harnesses::CopilotProviderWiring;
use serde::{Deserialize, Serialize};

/// Where the sidecar listens (PRD 004 q-03).
pub const DEFAULT_URL: &str = "http://127.0.0.1:7471";
/// Test-only override of [`DEFAULT_URL`], read once at daemon start. Only a
/// loopback `http://` URL is accepted.
pub const URL_ENV: &str = "PIJ_CACHE_PROXY_URL";
/// The sidecar's status route (blueprint 003 c-status-route).
pub const STATUS_PATH: &str = "/__pij/status";
/// How long a spawn waits for the status route (d-6).
pub const GATE_TIMEOUT: Duration = Duration::from_secs(1);
/// How often the watch reads the status route (d-7, k-5).
pub const WATCH_INTERVAL: Duration = Duration::from_secs(30);
/// The state the watch records when the status route does not answer.
pub const NOT_LISTENING: &str = "not_listening";

/// The sidecar's status route body (c-status-route). Every field but `ready`
/// and `state` defaults, so a sidecar that adds or omits one is still read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyStatus {
    /// True only when new spawns may be proxied.
    pub ready: bool,
    /// `ready`, `off`, `no_account`, `gh_missing`, `no_token`,
    /// `quota_exhausted` or `upstream_unreachable`.
    pub state: String,
    /// Why it is not ready; empty when ready.
    #[serde(default)]
    pub reason: String,
    /// What to do about it; empty when ready.
    #[serde(default)]
    pub fix: String,
    /// The account the token belongs to, when known.
    #[serde(default)]
    pub account: Option<String>,
    /// The upstream API host, when known.
    #[serde(default)]
    pub upstream_host: Option<String>,
    /// The account's premium quota.
    #[serde(default)]
    pub quota: Quota,
    /// Claude models the account's API serves, keyed by base id.
    #[serde(default)]
    pub models: BTreeMap<String, ModelLimits>,
    /// The sidecar's counters.
    #[serde(default)]
    pub counters: Counters,
    /// The sidecar's process id.
    #[serde(default)]
    pub pid: Option<u64>,
    /// Epoch milliseconds of the sidecar's last state change.
    #[serde(default)]
    pub since: Option<u64>,
}

/// `quota` in the status route.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    /// Whether premium requests are usable now.
    #[serde(default)]
    pub usable: bool,
    /// The reset date, when exhausted.
    #[serde(default)]
    pub reset: Option<String>,
}

/// One served model's limits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLimits {
    /// The prompt window the seat must be told about (req-13).
    pub max_prompt_tokens: u64,
}

/// `counters` in the status route.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counters {
    /// Model requests forwarded.
    pub requests: u64,
    /// Token refreshes.
    pub refreshes: u64,
    /// `gh` launches.
    pub gh_launches: u64,
    /// Upstream 401 answers.
    pub upstream_401: u64,
    /// Upstream 402 answers.
    pub upstream_402: u64,
}

/// One read of the status route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    /// The sidecar answered.
    Status(Box<ProxyStatus>),
    /// No answer: refused or timed out.
    Unreachable(String),
    /// An answer that is not the contract: not 2xx (a redirect is never
    /// followed), or not the status body.
    Unreadable(String),
}

impl Reading {
    /// The state the watch compares: the status's own, or [`NOT_LISTENING`]
    /// for any reading the gate can't use.
    pub fn state(&self) -> &str {
        match self {
            Self::Status(status) => &status.state,
            Self::Unreachable(_) | Self::Unreadable(_) => NOT_LISTENING,
        }
    }
}

/// The model id the Copilot API serves for `model`: an OMP-style `-1m`
/// suffix stripped (req-13; the API refuses `-1m` and the base id already
/// serves 1M).
pub fn base_id(model: &str) -> &str {
    model.strip_suffix("-1m").unwrap_or(model)
}

/// What the gate needs before reading the status route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precheck {
    /// Not a Copilot seat: no state, no read, no env (req-12).
    NotCopilot,
    /// Plain without asking the sidecar.
    Plain(SeatCacheProxy),
    /// Ask the sidecar whether it serves this base id.
    Ask {
        /// The model's base id.
        base_id: String,
    },
}

/// The checks that need no status read: harness, opt-out, model.
pub fn precheck(harness: Harness, model: Option<&str>, opted_out: bool) -> Precheck {
    if harness != Harness::Copilot {
        return Precheck::NotCopilot;
    }
    if opted_out {
        return Precheck::Plain(SeatCacheProxy::plain(CACHE_PROXY_OPTED_OUT, None));
    }
    let Some(model) = model else {
        return Precheck::Plain(SeatCacheProxy::plain(
            "no --model given, so the gate can't tell the model is Claude",
            Some("spawn with --model <a Claude model>".to_string()),
        ));
    };
    if !model.starts_with("claude") {
        return Precheck::Plain(SeatCacheProxy::plain(
            format!("model {model} is not Claude"),
            None,
        ));
    }
    Precheck::Ask {
        base_id: base_id(model).to_string(),
    }
}

/// The gate's decision from one reading (d-6): proxied only on `ready` with
/// the base id served; otherwise plain with the reason and fix.
pub fn decide(
    reading: &Reading,
    base_id: &str,
    base_url: &str,
) -> (SeatCacheProxy, Option<CopilotProviderWiring>) {
    let status = match reading {
        Reading::Unreachable(_) => {
            return (
                SeatCacheProxy::plain("proxy not listening", Some(start_fix())),
                None,
            );
        }
        Reading::Unreadable(detail) => {
            return (
                SeatCacheProxy::plain(
                    format!("proxy status unreadable: {detail}"),
                    Some(format!(
                        "check that `pij-rs cache-proxy` serves {STATUS_PATH}: `pij-rs cache-proxy status`"
                    )),
                ),
                None,
            );
        }
        Reading::Status(status) => status,
    };
    if !status.ready || status.state != "ready" {
        let reason = if status.reason.is_empty() {
            format!("proxy state {}", status.state)
        } else {
            format!("proxy {}: {}", status.state, status.reason)
        };
        let fix = (!status.fix.is_empty()).then(|| status.fix.clone());
        return (SeatCacheProxy::plain(reason, fix), None);
    }
    let Some(limits) = status.models.get(base_id) else {
        return (
            SeatCacheProxy::plain(
                "model not served by Copilot API",
                Some(format!(
                    "{base_id} is not in `pij-rs cache-proxy status`; spawn with a Claude model it lists"
                )),
            ),
            None,
        );
    };
    (
        SeatCacheProxy {
            state: CacheProxyRoute::Proxied,
            reason: format!("1-hour cache through {base_url} as {base_id}"),
            fix: None,
        },
        Some(CopilotProviderWiring {
            base_url: base_url.to_string(),
            model_id: base_id.to_string(),
            max_prompt_tokens: limits.max_prompt_tokens,
        }),
    )
}

fn start_fix() -> String {
    "start the sidecar (`just cache-proxy-autostart`, or run `pij-rs cache-proxy`), then check `pij-rs cache-proxy status`"
        .to_string()
}

/// Live Copilot seats split by their recorded route (row 18: nothing
/// recorded reads as plain).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SeatSplit {
    /// Seats whose calls go through the proxy.
    pub proxied: Vec<String>,
    /// Plain seats.
    pub plain: Vec<String>,
}

impl SeatSplit {
    /// Split the live (untombstoned) Copilot seats among `seats`.
    pub fn of(seats: &[SeatDescriptor]) -> Self {
        let mut split = Self::default();
        for seat in seats.iter().filter(|seat| seat.tombstoned_at.is_none()) {
            match seat.cache_proxy_view().map(|view| view.state) {
                Some(CacheProxyRoute::Proxied) => split.proxied.push(seat.id.to_string()),
                Some(CacheProxyRoute::Plain) => split.plain.push(seat.id.to_string()),
                None => {}
            }
        }
        split
    }
}

/// The watch's decision for one reading (d-7, F192): the notice to send, if
/// any. `previous` is the last state seen, `None` on the daemon's first read.
///
/// The first read is silent when the proxy is `ready`, or when no live seat is
/// proxied; otherwise it reports, so a sidecar that died while the daemon was
/// down is still told (pm-004 ruling, 2026-11-18). Later reads report only a
/// change of state.
pub fn transition(
    previous: Option<&str>,
    reading: &Reading,
    url: &str,
    proxied: &[String],
) -> Option<String> {
    let state = reading.state();
    match previous {
        None if state == "ready" || proxied.is_empty() => return None,
        Some(previous) if previous == state => return None,
        _ => {}
    }
    let was = previous.map_or_else(String::new, |previous| format!(" (was {previous})"));
    let seats = if proxied.is_empty() {
        "none".to_string()
    } else {
        proxied.join(", ")
    };
    let revive = "revive a seat to re-run the gate: `pij-rs revive <seat>`";
    Some(match reading {
        Reading::Unreachable(detail) | Reading::Unreadable(detail) => format!(
            "pij cache proxy is not listening at {url}{was}: {detail}. Proxied seats' model calls fail: {seats}. Fix: {}; then {revive}, and it comes up proxied or plain.",
            start_fix()
        ),
        Reading::Status(status) if status.state == "ready" => format!(
            "pij cache proxy is ready{was}{}: new Copilot Claude spawns are proxied. Proxied seats: {seats}. Plain seats stay plain until revived; {revive}.",
            status
                .account
                .as_deref()
                .map_or_else(String::new, |account| format!(" for {account}"))
        ),
        Reading::Status(status) if status.state == "off" => format!(
            "pij cache proxy is off{was}: new Copilot spawns run plain. Seats already proxied keep being served, unaffected: {seats}."
        ),
        Reading::Status(status) => format!(
            "pij cache proxy is {}{was}: {}. New Copilot spawns run plain. Proxied seats: {seats}.{}",
            status.state,
            if status.reason.is_empty() {
                "no reason given"
            } else {
                &status.reason
            },
            if status.fix.is_empty() {
                String::new()
            } else {
                format!(" Fix: {}.", status.fix)
            }
        ),
    })
}

/// What the watch last saw, for `/health` (`cache_proxy`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct HealthView {
    /// The status route's base URL.
    pub url: String,
    /// The last state seen, or `None` before the first read.
    pub state: Option<String>,
    /// Whether the last read got an answer.
    pub reachable: Option<bool>,
    /// The last status body, counters included.
    pub status: Option<ProxyStatus>,
    /// Why the last read got no answer.
    pub error: Option<String>,
    /// Epoch milliseconds of the last read.
    pub checked_at: Option<u64>,
    /// Notices sent since the daemon started.
    pub notices: u64,
    /// Live Copilot seats at the last read.
    pub seats: SeatSplit,
}

/// The status client and the watch's memory, one per daemon.
#[derive(Debug)]
pub struct CacheProxy {
    url: String,
    http: Result<reqwest::Client, String>,
    watch: Mutex<HealthView>,
}

impl CacheProxy {
    /// From [`URL_ENV`], read once here; [`DEFAULT_URL`] when unset or not a
    /// loopback `http://` URL (the seat's env carries it, so it must never
    /// point off the machine).
    pub fn from_env() -> Self {
        let url = match std::env::var(URL_ENV) {
            Ok(url) if is_loopback_url(&url) => url.trim_end_matches('/').to_string(),
            Ok(url) => {
                eprintln!(
                    "pij-rs: {URL_ENV}={url} is not a loopback http:// URL; using {DEFAULT_URL}"
                );
                DEFAULT_URL.to_string()
            }
            Err(_) => DEFAULT_URL.to_string(),
        };
        Self::new(url)
    }

    /// For a known base URL.
    pub fn new(url: String) -> Self {
        // A redirect is never followed: the status route is loopback, and a
        // redirect could point the gate anywhere. A client that can't be
        // built reads every status as unreadable, so every seat runs plain.
        let http = reqwest::Client::builder()
            .timeout(GATE_TIMEOUT)
            .connect_timeout(GATE_TIMEOUT)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string());
        let watch = Mutex::new(HealthView {
            url: url.clone(),
            ..HealthView::default()
        });
        Self { url, http, watch }
    }

    /// The base URL seats are given.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// One read of the status route, bounded by [`GATE_TIMEOUT`].
    pub async fn read(&self) -> Reading {
        let http = match &self.http {
            Ok(http) => http,
            Err(error) => return Reading::Unreadable(format!("no HTTP client: {error}")),
        };
        let response = match http.get(format!("{}{STATUS_PATH}", self.url)).send().await {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Reading::Unreachable("no answer within 1 s".to_string());
            }
            Err(_) => return Reading::Unreachable("connection refused".to_string()),
        };
        if !response.status().is_success() {
            return Reading::Unreadable(format!("status route answered {}", response.status()));
        }
        match response.json::<ProxyStatus>().await {
            Ok(status) => Reading::Status(Box::new(status)),
            Err(error) => Reading::Unreadable(format!("unreadable status: {error}")),
        }
    }

    /// The spawn gate (c-spawn-gate): the seat's recorded route and, when
    /// proxied, the provider wiring. `(None, None)` for a non-Copilot seat,
    /// which is never read for and never touched.
    pub async fn gate(
        &self,
        harness: Harness,
        model: Option<&str>,
        opted_out: bool,
    ) -> (Option<SeatCacheProxy>, Option<CopilotProviderWiring>) {
        match precheck(harness, model, opted_out) {
            Precheck::NotCopilot => (None, None),
            Precheck::Plain(plain) => (Some(plain), None),
            Precheck::Ask { base_id } => {
                let reading = self.read().await;
                let (seat, wiring) = decide(&reading, &base_id, &self.url);
                (Some(seat), wiring)
            }
        }
    }

    /// One watch tick: read, record, and return the notice to send, if any.
    pub async fn observe(&self, seats: &[SeatDescriptor], now_ms: u64) -> Option<String> {
        let reading = self.read().await;
        let split = SeatSplit::of(seats);
        let mut watch = self
            .watch
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let notice = transition(watch.state.as_deref(), &reading, &self.url, &split.proxied);
        watch.state = Some(reading.state().to_string());
        watch.checked_at = Some(now_ms);
        watch.seats = split;
        match reading {
            Reading::Status(status) => {
                watch.reachable = Some(true);
                watch.status = Some(*status);
                watch.error = None;
            }
            Reading::Unreachable(detail) | Reading::Unreadable(detail) => {
                watch.reachable = Some(false);
                watch.status = None;
                watch.error = Some(detail);
            }
        }
        if notice.is_some() {
            watch.notices += 1;
        }
        notice
    }

    /// What `/health` shows.
    pub fn health(&self) -> HealthView {
        self.watch
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

fn is_loopback_url(url: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"]
        .iter()
        .any(|prefix| url.starts_with(prefix))
}

/// Run the watch: one tick now, then one every [`WATCH_INTERVAL`]. Each
/// notice goes once to each live prime ([`crate::http::notify_primes`]).
pub(crate) fn start(services: std::sync::Arc<crate::Services>) -> crate::lifecycle::TickLoop {
    let mut first = true;
    crate::lifecycle::TickLoop::start_validated_with_wake(
        crate::lifecycle::TickInterval::new(WATCH_INTERVAL).expect("a non-zero constant"),
        move || {
            let services = std::sync::Arc::clone(&services);
            async move {
                tick(&services).await;
                Ok(())
            }
        },
        move || {
            let now = std::mem::replace(&mut first, false);
            async move {
                if !now {
                    std::future::pending::<()>().await;
                }
            }
        },
    )
}

async fn tick(services: &crate::Services) {
    let seats = match services
        .registry
        .list(pij_core::ports::SeatFilter {
            harness: Some(Harness::Copilot),
            live_only: true,
            ..pij_core::ports::SeatFilter::default()
        })
        .await
    {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("pij-rs: cache-proxy watch could not read seats: {error}");
            return;
        }
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    if let Some(notice) = services.cache_proxy.observe(&seats, now_ms).await {
        crate::http::notify_primes(services, notice).await;
    }
}

#[cfg(test)]
#[path = "cache_proxy_tests.rs"]
mod tests;
