//! Rebuild, drain, and restart the daemon from a reproducible main checkout.

mod store;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::model::{Destination, ProcIdentity, SeatId};
use pij_core::ports::LivenessPort;
use pij_daemon::DaemonRuntime;
use pij_harnesses::proc::ProcLiveness;
use serde::Serialize;

use crate::{DaemonClient, SendRequest};

const COMMAND: &str = "pij daemon bounce";
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Observable result of one completed daemon bounce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BounceReport {
    /// Build string returned by the restarted daemon itself.
    pub version: String,
    /// Address whose unauthenticated health response was exactly 401.
    pub addr: String,
    /// Best-effort per-seat announcement outcomes.
    pub announcements: Vec<String>,
}

/// Resolve and validate the checkout before any build or process action.
///
/// # Errors
/// Refuses outside Git, for a dirty tree, or when `HEAD != origin/main`.
pub fn reproducible_repo(cwd: &Path) -> Result<PathBuf> {
    let root = git_stdout(cwd, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    let dirty = git_stdout(
        &root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    if !dirty.is_empty() {
        return Err(failure(
            "refusing bounce: the checkout is dirty; commit or remove tracked and untracked changes",
        ));
    }
    let head = git_stdout(&root, &["rev-parse", "HEAD"])?;
    let main = git_stdout(&root, &["rev-parse", "origin/main"])?;
    if head.trim() != main.trim() {
        return Err(failure(format!(
            "refusing bounce: HEAD {} != origin/main {}; push and update the main checkout",
            head.trim(),
            main.trim()
        )));
    }
    Ok(root)
}

/// Execute the ruled build → announce → drain/restart → health/version sequence.
///
/// # Errors
/// Refuses on every unobserved or ambiguous state; announcements alone are
/// best-effort and are returned as output lines rather than gating the bounce.
pub async fn run(repo_root: &Path, state_dir: &Path) -> Result<BounceReport> {
    build(repo_root)?;
    let old = store::read_runtime(state_dir)?;
    corroborate(old.process).await?;
    let announcements = announce(state_dir, &old).await;
    signal_and_wait(old.process).await?;
    let artifact = repo_root.join("target/release/pij-rs");
    let child_pid = launch(&artifact, state_dir, &old)?;
    wait_for_unauthenticated_401(old.addr).await?;

    let new = store::read_runtime(state_dir)?;
    if new.process.pid != child_pid {
        return Err(failure(format!(
            "health answered, but runtime record names pid {} instead of launched pid {child_pid}",
            new.process.pid
        )));
    }
    corroborate(new.process).await?;
    let version = observed_version(state_dir, &new).await?;
    Ok(BounceReport {
        version,
        addr: new.addr.to_string(),
        announcements,
    })
}

fn build(repo_root: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args([
            "build",
            "--locked",
            "--release",
            "-p",
            "pij-cli",
            "--bin",
            "pij-rs",
        ])
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(repo_root)
        .output()
        .map_err(|error| failure(format!("could not start the release build: {error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(failure(format!(
        "release build failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

async fn announce(state_dir: &Path, runtime: &DaemonRuntime) -> Vec<String> {
    let client = match DaemonClient::new(state_dir, &runtime.addr.to_string()) {
        Ok(client) => client,
        Err(error) => return vec![format!("announcement skipped: {error}")],
    };
    let roster = client.list().await;
    let Some(roster) = roster.data else {
        return vec![format!(
            "announcement skipped: {}",
            roster
                .meta
                .unwrap_or_else(|| "daemon roster refused".to_string())
        )];
    };
    let liveness = ProcLiveness::new();
    let mut lines = Vec::new();
    for (index, seat) in roster.seats.into_iter().enumerate() {
        if seat.machine.as_deref() != Some(runtime.machine.as_str())
            || seat.tombstoned_at.is_some()
            || seat.id == SeatId::from("pij-daemon")
        {
            continue;
        }
        let Some(process) = seat.proc else {
            continue;
        };
        if liveness.proc_start(process.pid).await.ok().flatten() != Some(process.proc_start) {
            lines.push(format!(
                "announcement {} skipped: process is not active",
                seat.id
            ));
            continue;
        }
        let request = SendRequest {
            from: "pij-daemon".into(),
            to: Destination::local(seat.id.clone()),
            body: "pij daemon bounce: draining in-flight work, then restarting".to_string(),
            msg_id: format!("bounce-{}-{index}", runtime.process.proc_start),
            in_reply_to: None,
            command: None,
            fyi: false,
            force: false,
            reason: None,
        };
        let reply = client.send(&request).await;
        if reply.ok {
            lines.push(format!("announcement {}: sent", seat.id));
        } else {
            lines.push(format!(
                "announcement {} failed: {}",
                seat.id,
                reply.meta.unwrap_or_else(|| "daemon refused".to_string())
            ));
        }
    }
    lines
}

async fn corroborate(process: ProcIdentity) -> Result<()> {
    match ProcLiveness::new().proc_start(process.pid).await? {
        Some(observed) if observed == process.proc_start => Ok(()),
        Some(observed) => Err(failure(format!(
            "refusing to signal pid {}: runtime record start {} != observed {observed}",
            process.pid, process.proc_start
        ))),
        None => Err(failure(format!(
            "refusing to signal pid {}: the recorded daemon is not running",
            process.pid
        ))),
    }
}

async fn signal_and_wait(process: ProcIdentity) -> Result<()> {
    corroborate(process).await?;
    let status = Command::new("kill")
        .args(["-INT", &process.pid.to_string()])
        .status()
        .map_err(|error| {
            failure(format!(
                "could not signal daemon pid {}: {error}",
                process.pid
            ))
        })?;
    if !status.success() {
        return Err(failure(format!(
            "kill -INT {} failed with {status}",
            process.pid
        )));
    }

    let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
    loop {
        match ProcLiveness::new().proc_start(process.pid).await? {
            None => return Ok(()),
            Some(observed) if observed != process.proc_start => return Ok(()),
            Some(_) if tokio::time::Instant::now() >= deadline => {
                return Err(failure(format!(
                    "daemon pid {} did not finish draining within {} seconds",
                    process.pid,
                    STOP_TIMEOUT.as_secs()
                )));
            }
            Some(_) => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }
}

fn launch(artifact: &Path, state_dir: &Path, runtime: &DaemonRuntime) -> Result<u32> {
    let log_path = state_dir.join("daemon.log");
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| failure(format!("could not open {}: {error}", log_path.display())))?;
    let stderr = stdout
        .try_clone()
        .map_err(|error| failure(format!("could not clone daemon log handle: {error}")))?;
    let mut command = Command::new(artifact);
    command
        .arg("--state-dir")
        .arg(state_dir)
        .arg("daemon")
        .arg("--bind")
        .arg(runtime.addr.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if runtime.offline {
        command.arg("--offline");
    }
    command
        .spawn()
        .map(|child| child.id())
        .map_err(|error| failure(format!("could not launch {}: {error}", artifact.display())))
}

#[derive(Debug)]
enum HealthProbe {
    Enforcing,
    Absent(String),
    Unexpected(reqwest::StatusCode),
}

async fn probe_unauthenticated(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
) -> HealthProbe {
    match client.get(format!("http://{addr}/health")).send().await {
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            HealthProbe::Enforcing
        }
        Ok(response) => HealthProbe::Unexpected(response.status()),
        Err(error) => HealthProbe::Absent(error.to_string()),
    }
}

async fn wait_for_unauthenticated_401(addr: std::net::SocketAddr) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .map_err(|error| failure(format!("could not create health client: {error}")))?;
    let deadline = tokio::time::Instant::now() + HEALTH_TIMEOUT;
    loop {
        match probe_unauthenticated(&client, addr).await {
            HealthProbe::Enforcing => return Ok(()),
            HealthProbe::Unexpected(status) => {
                return Err(failure(format!(
                    "health at {addr} answered {status}, expected auth-enforcing 401"
                )));
            }
            HealthProbe::Absent(error) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(POLL_INTERVAL).await;
                if error.contains("builder error") {
                    return Err(failure(format!("invalid health request: {error}")));
                }
            }
            HealthProbe::Absent(error) => {
                return Err(failure(format!(
                    "nothing answered health at {addr} within {} seconds: {error}",
                    HEALTH_TIMEOUT.as_secs()
                )));
            }
        }
    }
}

async fn observed_version(state_dir: &Path, runtime: &DaemonRuntime) -> Result<String> {
    let reply = DaemonClient::new(state_dir, &runtime.addr.to_string())?
        .ping()
        .await;
    if !reply.ok {
        return Err(failure(format!(
            "authenticated health refused after 401 tell: {}",
            reply.meta.unwrap_or_else(|| "daemon refused".to_string())
        )));
    }
    reply
        .data
        .and_then(|value| {
            value
                .get("build")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| failure("authenticated health omitted data.build"))
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| failure(format!("could not run git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        return Err(failure(format!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|error| {
        failure(format!(
            "git {} returned non-UTF-8 output: {error}",
            args.join(" ")
        ))
    })
}

fn failure(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: COMMAND.to_string(),
        message: message.into(),
    }
}

/// # Composition recipe
///
/// `main.rs` resolves `reproducible_repo(current_dir)` before any side effect,
/// then calls `bounce::run(&repo, &state_dir)`. The daemon writes
/// `DaemonRuntime` after bind and before key publication. Shutdown signals HTTP,
/// drain, observer, and federation together; each joins its in-flight work, and
/// no dequeue-capable loop starts another claim after observing stop.
const _: () = ();
