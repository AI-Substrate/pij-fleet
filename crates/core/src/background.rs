//! Persistent background-job facts shared by the daemon and storage adapter.

use serde::{Deserialize, Serialize};

use crate::model::SeatId;

/// Persistent lifecycle of a detached command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundState {
    /// Recorded before the process is spawned.
    Queued,
    /// Spawned with a complete process identity and process group.
    Running,
    /// Exited with a recorded completion result.
    Done,
    /// Exited after the daemon requested termination.
    Killed,
    /// Process identity disappeared without a recoverable completion result.
    Lost,
}

/// A detached command, including the identity needed for safe restart recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundJob {
    /// Stable, daemon-generated job identifier.
    pub job_id: String,
    /// Authenticated seat that owns the command and receives its completion.
    pub owner: SeatId,
    /// Human description supplied when the job is created.
    pub title: String,
    /// Shell command executed by the daemon.
    pub command: String,
    /// Process id, present together with `proc_start` and `pgid` after spawn.
    pub pid: Option<u32>,
    /// Process start identity; never trust `pid` without this value.
    pub proc_start: Option<u64>,
    /// Detached process group used for group-wide termination.
    pub pgid: Option<u32>,
    /// Daemon-local path containing the full stdout and stderr log.
    pub out_path: String,
    /// Current persistent lifecycle state.
    pub state: BackgroundState,
    /// Observed exit result, if available.
    pub exit_code: Option<i32>,
    /// Creation timestamp, in the daemon's timestamp units.
    pub started_at: u64,
    /// Terminal timestamp, present only for a finished job.
    pub finished_at: Option<u64>,
    /// Durable termination intent, retained even after completion.
    pub kill_requested: bool,
    /// Whether the terminal notification has been delivered.
    pub notified: bool,
    /// Absolute instant after which the daemon kills the job, if a timeout was set.
    #[serde(default)]
    pub deadline_at: Option<u64>,
    /// The daemon, not a caller, requested the kill because the deadline passed.
    #[serde(default)]
    pub timed_out: bool,
    /// The daemon persisted this just before sending TERM to the runner's group.
    #[serde(default)]
    pub term_sent: bool,
}
