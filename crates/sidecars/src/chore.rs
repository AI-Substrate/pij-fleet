//! Durable named probes whose deltas remain pending until explicit acknowledgement.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::model::{Job, Outcome, SeatId};
use pij_core::ports::Queue;
use serde::{Deserialize, Serialize};

use crate::common::{LoopHandle, Processed, enqueue_turn, now_ms, start_resilient_loop};

/// Queue kind for chore commands.
pub const CHORE_KIND: &str = "sidecar:chore";
const WORKER_ID: &str = "sidecar-chore";

/// One durable chore command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ChoreRequest {
    /// Define or replace one named probe.
    Add {
        /// Name.
        name: String,
        /// Shell probe.
        probe: String,
        /// Seat receiving reports.
        target: SeatId,
    },
    /// Run every defined probe.
    Run {
        /// Seat receiving the report.
        target: SeatId,
    },
    /// Advance one pending baseline.
    Ack {
        /// Chore name.
        name: String,
        /// Seat receiving confirmation.
        target: SeatId,
    },
}

/// Build a chore command row.
pub fn job(request: &ChoreRequest, request_id: &str) -> Result<Job> {
    let serial_key = match request {
        ChoreRequest::Add { name, .. } | ChoreRequest::Ack { name, .. } => name.clone(),
        ChoreRequest::Run { .. } => "all".to_string(),
    };
    Ok(Job {
        kind: CHORE_KIND.to_string(),
        serial_key,
        payload: serde_json::to_string(request).map_err(codec_error)?,
        dedupe_key: request_id.to_string(),
        attempt: 0,
    })
}

/// Persisted probe and its acknowledged/pending fingerprints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoreEntry {
    /// Shell command.
    pub probe: String,
    /// Last acknowledged output.
    pub baseline: Option<String>,
    /// Latest changed output awaiting acknowledgement.
    pub pending: Option<String>,
    /// Last observed status.
    pub status: ChoreStatus,
}

/// Honest probe status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChoreStatus {
    /// Definition has not been probed.
    NeverRun,
    /// Probe equals the acknowledged baseline.
    Unchanged,
    /// Probe differs and remains pending.
    Changed,
    /// Probe failed and therefore made no baseline claim.
    NotProbeable {
        /// Exact probe failure.
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ChoreState {
    entries: BTreeMap<String, ChoreEntry>,
}

/// Concrete durable chore consumer.
pub struct ChoreWorker {
    queue: Arc<dyn Queue>,
    state_path: PathBuf,
    state_lock: Mutex<()>,
}

impl ChoreWorker {
    /// Construct over one resolved durable state path.
    pub fn new(queue: Arc<dyn Queue>, state_path: PathBuf) -> Result<Self> {
        if let Some(parent) = state_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| io_error("create chore state directory", parent, error))?;
        }
        Ok(Self {
            queue,
            state_path,
            state_lock: Mutex::new(()),
        })
    }

    /// Start the shipped resilient loop.
    pub fn start(self: Arc<Self>, interval: Duration, limit: usize) -> Result<LoopHandle> {
        start_resilient_loop("chore", interval, move || {
            let worker = Arc::clone(&self);
            async move { worker.run_once(limit).await }
        })
    }

    /// Claim and process at most `limit` actual rows.
    pub async fn run_once(&self, limit: usize) -> Result<Processed> {
        let mut count = 0;
        while count < limit {
            let Some((id, row)) = self
                .queue
                .claim(&[CHORE_KIND.to_string()], WORKER_ID)
                .await?
            else {
                break;
            };
            let request: ChoreRequest = serde_json::from_str(&row.payload).map_err(codec_error)?;
            let result = self.process(request).await;
            match result {
                Ok(()) => self.queue.ack(id, Outcome::Done).await?,
                Err(error) => {
                    self.queue.retry(id, Duration::ZERO).await?;
                    return Err(error);
                }
            }
            count += 1;
        }
        Ok(Processed { count })
    }

    async fn process(&self, request: ChoreRequest) -> Result<()> {
        match request {
            ChoreRequest::Add {
                name,
                probe,
                target,
            } => {
                let mut state = self.read_state()?;
                let previous = state.entries.get(&name);
                let entry = ChoreEntry {
                    probe,
                    baseline: previous.and_then(|entry| entry.baseline.clone()),
                    pending: previous.and_then(|entry| entry.pending.clone()),
                    status: ChoreStatus::NeverRun,
                };
                state.entries.insert(name.clone(), entry);
                self.persist_state(&state)?;
                enqueue_turn(
                    &self.queue,
                    "pij-chore",
                    &target,
                    format!("chore `{name}` defined"),
                    format!("chore-{name}-defined-{}", now_ms()?),
                )
                .await
            }
            ChoreRequest::Run { target } => {
                let mut state = self.read_state()?;
                let mut moved = 0usize;
                let mut lines = Vec::new();
                for (name, entry) in &mut state.entries {
                    match run_probe(&entry.probe) {
                        Ok(output) => {
                            if entry.baseline.as_deref() == Some(output.as_str()) {
                                entry.pending = None;
                                entry.status = ChoreStatus::Unchanged;
                                lines.push(format!("UNCHANGED {name}"));
                            } else {
                                entry.pending = Some(output.clone());
                                entry.status = ChoreStatus::Changed;
                                moved += 1;
                                lines.push(format!("CHANGED {name}: {output}"));
                            }
                        }
                        Err(error) => {
                            entry.status = ChoreStatus::NotProbeable {
                                reason: error.clone(),
                            };
                            lines.push(format!("NOT-PROBEABLE {name}: {error}"));
                        }
                    }
                }
                let probed = state.entries.len();
                self.persist_state(&state)?;
                let report = format!(
                    "{} — {probed} chores probed, {moved} moved\n{}",
                    if moved == 0 { "NO CHANGE" } else { "CHANGES" },
                    lines.join("\n")
                );
                enqueue_turn(
                    &self.queue,
                    "pij-chore",
                    &target,
                    report,
                    format!("chore-run-{}", now_ms()?),
                )
                .await
            }
            ChoreRequest::Ack { name, target } => {
                let mut state = self.read_state()?;
                let entry = state
                    .entries
                    .get_mut(&name)
                    .ok_or_else(|| PijError::Adapter {
                        adapter: "sidecars/chore".to_string(),
                        message: format!("chore `{name}` is not defined"),
                    })?;
                let pending = entry.pending.take().ok_or_else(|| PijError::Adapter {
                    adapter: "sidecars/chore".to_string(),
                    message: format!("chore `{name}` has no pending delta"),
                })?;
                entry.baseline = Some(pending);
                entry.status = ChoreStatus::Unchanged;
                self.persist_state(&state)?;
                enqueue_turn(
                    &self.queue,
                    "pij-chore",
                    &target,
                    format!("chore `{name}` baseline acknowledged"),
                    format!("chore-{name}-acked-{}", now_ms()?),
                )
                .await
            }
        }
    }

    /// Read a snapshot for diagnostics and tests.
    pub fn entries(&self) -> Result<BTreeMap<String, ChoreEntry>> {
        Ok(self.read_state()?.entries)
    }

    fn read_state(&self) -> Result<ChoreState> {
        let _guard = self.state_lock.lock().expect("chore state mutex");
        if !self.state_path.exists() {
            return Ok(ChoreState::default());
        }
        serde_json::from_str(
            &fs::read_to_string(&self.state_path)
                .map_err(|error| io_error("read chore state", &self.state_path, error))?,
        )
        .map_err(codec_error)
    }

    fn persist_state(&self, state: &ChoreState) -> Result<()> {
        let _guard = self.state_lock.lock().expect("chore state mutex");
        let staged = self.state_path.with_extension("json.tmp");
        fs::write(&staged, serde_json::to_vec(state).map_err(codec_error)?)
            .map_err(|error| io_error("write chore state", &staged, error))?;
        fs::rename(&staged, &self.state_path)
            .map_err(|error| io_error("publish chore state", &self.state_path, error))
    }
}

fn run_probe(probe: &str) -> std::result::Result<String, String> {
    let output = Command::new("/bin/sh")
        .args(["-c", probe])
        .output()
        .map_err(|error| format!("could not execute /bin/sh: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "/bin/sh exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("probe output is not UTF-8: {error}"))
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> PijError {
    PijError::Adapter {
        adapter: "sidecars/chore".to_string(),
        message: format!("could not {action} {}: {error}", path.display()),
    }
}
fn codec_error(error: serde_json::Error) -> PijError {
    PijError::Adapter {
        adapter: "sidecars/chore".to_string(),
        message: error.to_string(),
    }
}
