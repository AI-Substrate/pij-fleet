//! `pij-rs cache-proxy` (plan 228, blueprint 003 c-proxy-verb): run the
//! Copilot cache-proxy sidecar, show its status, or change its config.
//!
//! - no action: run the sidecar in the foreground (its LaunchAgent does this);
//! - `status`: the sidecar's status route, or "not listening" plus the fix,
//!   and the second-human-account warning;
//! - `account <login>`, `off`, `on`: write `<state-dir>/cache-proxy.toml`,
//!   then ask the sidecar to reload it.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use pij_cache_proxy::config::{self, ProxyConfig};
use pij_cache_proxy::{DEFAULT_PORT, RELOAD_PATH, STATUS_PATH, Settings, State, Status, accounts};
use pij_core::model::{Envelope, ErrorKind};
use serde::Serialize;

/// The variable that moves the sidecar off 7471 (tests and proofs).
pub const PORT_ENV: &str = "PIJ_CACHE_PROXY_PORT";

/// How long `status` waits for the status route.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a reload may take: one gh launch and two GETs.
const RELOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// A config change.
#[derive(Clone, Debug)]
pub enum Change {
    /// `account <login>`.
    Account(String),
    /// `off`.
    Off,
    /// `on`.
    On,
}

/// What `status`, `account`, `off` and `on` report.
#[derive(Debug, Serialize)]
pub struct Report {
    /// The port asked.
    pub port: u16,
    /// Whether the sidecar answered.
    pub listening: bool,
    /// The sidecar's status route body, when it answered.
    pub status: Option<Status>,
    /// What to do when it did not answer.
    pub fix: Option<String>,
    /// The config file written, for `account`, `off` and `on`.
    pub config_written: Option<String>,
    /// The second-human-account warning (`status` only).
    pub accounts_warning: Option<String>,
    /// Why the account check could not run (`status` only).
    pub accounts_error: Option<String>,
}

/// `--port`, then `PIJ_CACHE_PROXY_PORT`, then 7471.
///
/// # Errors
/// The variable is not a port number.
pub fn resolve_port(flag: Option<u16>, env: Option<OsString>) -> Result<u16, String> {
    if let Some(port) = flag {
        return Ok(port);
    }
    match env {
        None => Ok(DEFAULT_PORT),
        Some(value) => value
            .to_str()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .ok_or_else(|| {
                format!(
                    "{PORT_ENV}={} is not a port number",
                    value.to_string_lossy()
                )
            }),
    }
}

/// Run the sidecar in the foreground until SIGTERM or SIGINT.
///
/// # Errors
/// It could not bind its port.
pub async fn run(state_dir: &Path, port: u16) -> Result<(), String> {
    pij_cache_proxy::run(Settings::from_env(state_dir.to_path_buf(), port)).await
}

fn client() -> reqwest::Client {
    // Loopback only: never route the sidecar through a proxy setting.
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_default()
}

fn not_listening(port: u16) -> Report {
    Report {
        port,
        listening: false,
        status: None,
        fix: Some(
            "just cache-proxy-autostart (or run it in the foreground: pij-rs cache-proxy)".into(),
        ),
        config_written: None,
        accounts_warning: None,
        accounts_error: None,
    }
}

async fn ask(request: reqwest::RequestBuilder, port: u16) -> Report {
    match request.send().await {
        Ok(response) if response.status().is_success() => match response.json::<Status>().await {
            Ok(status) => Report {
                listening: true,
                status: Some(status),
                fix: None,
                ..not_listening(port)
            },
            Err(error) => Report {
                listening: true,
                fix: Some(format!(
                    "port {port} answered something that is not the cache proxy: {}",
                    error.without_url()
                )),
                ..not_listening(port)
            },
        },
        Ok(response) => Report {
            listening: true,
            fix: Some(format!(
                "port {port} answered {}: is it the cache proxy?",
                response.status()
            )),
            ..not_listening(port)
        },
        Err(_) => not_listening(port),
    }
}

/// `pij-rs cache-proxy status`: the status route plus the account check.
pub async fn status(port: u16) -> Envelope<Report> {
    let url = format!("http://127.0.0.1:{port}{STATUS_PATH}");
    let mut report = ask(client().get(url).timeout(STATUS_TIMEOUT), port).await;
    match accounts::check() {
        Ok(found) => report.accounts_warning = accounts::warning(&found),
        Err(error) => report.accounts_error = Some(error),
    }
    Envelope::ok("pij cache-proxy status", report)
}

/// `account <login>`, `off`, `on`: write the config, then reload the sidecar.
pub async fn change(state_dir: &Path, port: u16, change: Change) -> Envelope<Report> {
    let command = match &change {
        Change::Account(_) => "pij cache-proxy account",
        Change::Off => "pij cache-proxy off",
        Change::On => "pij cache-proxy on",
    };
    let mut config = match config::load(state_dir) {
        Ok(config) => config.unwrap_or_else(ProxyConfig::new),
        Err(error) => return Envelope::refused(command, ErrorKind::Refused, error),
    };
    match change {
        Change::Account(login) => {
            if let Err(error) = config::validate_account(&login) {
                return Envelope::refused(command, ErrorKind::Refused, error);
            }
            config.account = Some(login);
        }
        Change::Off => config.enabled = false,
        Change::On => config.enabled = true,
    }
    if let Err(error) = config::save(state_dir, &config) {
        return Envelope::refused(command, ErrorKind::Refused, error);
    }
    let url = format!("http://127.0.0.1:{port}{RELOAD_PATH}");
    let mut report = ask(client().post(url).timeout(RELOAD_TIMEOUT), port).await;
    report.config_written = Some(config::path(state_dir).display().to_string());
    if !report.listening {
        report.fix = Some(format!(
            "saved; it applies when the sidecar starts: {}",
            report.fix.unwrap_or_default()
        ));
    }
    Envelope::ok(command, report)
}

fn since(ms: u64) -> String {
    i128::from(ms)
        .checked_mul(1_000_000)
        .and_then(|nanos| time::OffsetDateTime::from_unix_timestamp_nanos(nanos).ok())
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| ms.to_string())
}

/// The human view.
#[must_use]
pub fn render(envelope: &Envelope<Report>) -> String {
    let Some(report) = envelope.data.as_ref() else {
        return format!(
            "{}: refused: {}",
            envelope.command,
            envelope.meta.as_deref().unwrap_or("no detail")
        );
    };
    let mut lines = Vec::new();
    if let Some(path) = &report.config_written {
        lines.push(format!("config:      {path}"));
    }
    match &report.status {
        None => {
            lines.push(format!(
                "cache-proxy: not listening on 127.0.0.1:{}",
                report.port
            ));
            if let Some(fix) = &report.fix {
                lines.push(format!("fix:         {fix}"));
            }
        }
        Some(status) => {
            lines.push(format!(
                "cache-proxy: {} on 127.0.0.1:{}",
                status.state.as_str(),
                report.port
            ));
            if status.state != State::Ready {
                lines.push(format!("reason:      {}", status.reason));
                lines.push(format!("fix:         {}", status.fix));
            }
            lines.push(format!(
                "account:     {}",
                status.account.as_deref().unwrap_or("(none)")
            ));
            lines.push(format!(
                "upstream:    {}",
                status.upstream_host.as_deref().unwrap_or("(unknown)")
            ));
            lines.push(format!(
                "quota:       {}{}",
                if status.quota.usable {
                    "usable"
                } else {
                    "not usable"
                },
                status
                    .quota
                    .reset
                    .as_deref()
                    .map(|reset| format!(" (resets {reset})"))
                    .unwrap_or_default()
            ));
            let models: Vec<String> = status
                .models
                .iter()
                .map(|(id, limit)| format!("{id} ({})", limit.max_prompt_tokens))
                .collect();
            lines.push(format!(
                "models:      {}",
                if models.is_empty() {
                    "(none)".to_string()
                } else {
                    models.join(", ")
                }
            ));
            let counters = &status.counters;
            lines.push(format!(
                "counters:    requests {}, refreshes {}, gh launches {}, upstream 401 {}, upstream 402 {}",
                counters.requests, counters.refreshes, counters.gh_launches, counters.upstream_401, counters.upstream_402
            ));
            lines.push(format!(
                "pid:         {} (state since {})",
                status.pid,
                since(status.since)
            ));
        }
    }
    if let Some(warning) = &report.accounts_warning {
        lines.push(format!("warning:     {warning}"));
    }
    if let Some(error) = &report.accounts_error {
        lines.push(format!("accounts:    unknown ({error})"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_is_the_flag_then_the_variable_then_7471() {
        assert_eq!(resolve_port(Some(9000), Some("8000".into())), Ok(9000));
        assert_eq!(resolve_port(None, Some("8000".into())), Ok(8000));
        assert_eq!(resolve_port(None, None), Ok(7471));
        assert!(resolve_port(None, Some("0".into())).is_err());
        assert!(resolve_port(None, Some("http".into())).is_err());
    }
}
