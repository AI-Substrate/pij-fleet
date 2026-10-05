//! Durable facts for daemon-owned detached processes and their completion turns.

use pij_core::background::{BackgroundJob, BackgroundState};
use pij_core::error::{PijError, Result};
use pij_core::model::{ProcIdentity, SeatId};
use sqlx::Row;

use crate::{StorePool, require_current_schema};

/// SQLite persistence for background jobs; the daemon owns authorization and IO.
#[derive(Clone)]
pub struct SqliteBackground {
    pool: StorePool,
}

impl SqliteBackground {
    /// Wrap the daemon's already-open store pool.
    pub fn new(pool: StorePool) -> Self {
        Self { pool }
    }

    /// Persist a job without replacing an existing identifier.
    ///
    /// # Errors
    /// Returns a schema error on skew, or an adapter error for duplicate ids,
    /// inconsistent lifecycle facts, out-of-range integers, or SQLite failure.
    pub async fn insert(&self, job: &BackgroundJob) -> Result<()> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "INSERT INTO background_jobs \
             (job_id, owner, title, command, pid, proc_start, pgid, out_path, state, \
              exit_code, started_at, finished_at, kill_requested, notified, deadline_at, \
              timed_out) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&job.job_id)
        .bind(job.owner.as_str())
        .bind(&job.title)
        .bind(&job.command)
        .bind(job.pid.map(i64::from))
        .bind(
            job.proc_start
                .map(|value| sql_i64(value, "proc_start"))
                .transpose()?,
        )
        .bind(job.pgid.map(i64::from))
        .bind(&job.out_path)
        .bind(encode_state(job.state))
        .bind(job.exit_code)
        .bind(sql_i64(job.started_at, "started_at")?)
        .bind(
            job.finished_at
                .map(|value| sql_i64(value, "finished_at"))
                .transpose()?,
        )
        .bind(job.kill_requested)
        .bind(job.notified)
        .bind(
            job.deadline_at
                .map(|value| sql_i64(value, "deadline_at"))
                .transpose()?,
        )
        .bind(job.timed_out)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(())
    }

    /// Read one job, including terminal history and pending notification facts.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error for invalid stored
    /// facts or SQLite failure.
    pub async fn get(&self, job_id: &str) -> Result<Option<BackgroundJob>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM background_jobs WHERE job_id = ?")
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .as_ref()
            .map(decode_job)
            .transpose()
    }

    /// Read every lifecycle and owner for daemon reconciliation and filtering.
    ///
    /// Rows are ordered by creation time and id; this method does not authorize
    /// access to other owners' jobs.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error for invalid stored
    /// facts or SQLite failure.
    pub async fn list(&self) -> Result<Vec<BackgroundJob>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM background_jobs ORDER BY started_at, job_id")
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .iter()
            .map(decode_job)
            .collect()
    }

    /// Read live jobs and terminal jobs whose notification is still pending.
    ///
    /// Uses the partial pending-work index rather than scanning notified history
    /// on each reconciliation tick. Ordered by creation time and id.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error for invalid stored
    /// facts or SQLite failure.
    pub async fn pending(&self) -> Result<Vec<BackgroundJob>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "SELECT * FROM background_jobs \
             WHERE state IN ('queued', 'running') OR notified = 0 \
             ORDER BY started_at, job_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(decode_job)
        .collect()
    }

    /// Record the spawned identity only while the job is still queued.
    ///
    /// Returns false for absent, running, or terminal jobs; no identity is
    /// overwritten by a repeated start or a recycled process id.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error for invalid process
    /// identity, out-of-range integers, or SQLite failure.
    pub async fn start(&self, job_id: &str, identity: &ProcIdentity, pgid: u32) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let result = sqlx::query(
            "UPDATE background_jobs SET state = 'running', pid = ?, proc_start = ?, pgid = ? \
             WHERE job_id = ? AND state = 'queued'",
        )
        .bind(i64::from(identity.pid))
        .bind(sql_i64(identity.proc_start, "proc_start")?)
        .bind(i64::from(pgid))
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Record termination intent exactly once, only for a running job.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn request_kill(&self, job_id: &str) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let result = sqlx::query(
            "UPDATE background_jobs SET kill_requested = 1 \
             WHERE job_id = ? AND state = 'running' AND kill_requested = 0",
        )
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Record a daemon-owned timeout kill exactly once, only for a running job
    /// whose deadline has passed and that no caller has already asked to kill.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn request_timeout(&self, job_id: &str, now: u64) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let result = sqlx::query(
            "UPDATE background_jobs SET kill_requested = 1, timed_out = 1 \
             WHERE job_id = ? AND state = 'running' AND kill_requested = 0 \
             AND deadline_at IS NOT NULL AND deadline_at <= ?",
        )
        .bind(job_id)
        .bind(sql_i64(now, "now")?)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Record (or, when the signal was never sent, withdraw) that the daemon is
    /// sending TERM to a live job it intends to kill. Persisted before the signal.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn set_term_sent(&self, job_id: &str, sent: bool) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let result = sqlx::query(
            "UPDATE background_jobs SET term_sent = ? \
             WHERE job_id = ? AND kill_requested = 1 AND state IN ('queued', 'running')",
        )
        .bind(sent)
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Transition a queued or running job to a terminal state exactly once.
    ///
    /// Competing completion/reconciliation calls are atomic no-ops after the
    /// first winner. Process identity and kill intent remain intact.
    ///
    /// # Errors
    /// Returns a schema error on skew, or an adapter error for a nonterminal
    /// destination, out-of-range timestamp, or SQLite failure.
    pub async fn finish(
        &self,
        job_id: &str,
        state: BackgroundState,
        exit_code: Option<i32>,
        finished_at: u64,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        if matches!(state, BackgroundState::Queued | BackgroundState::Running) {
            return Err(invalid("background completion requires a terminal state"));
        }
        let result = sqlx::query(
            "UPDATE background_jobs SET state = ?, exit_code = ?, finished_at = ? \
             WHERE job_id = ? AND state IN ('queued', 'running')",
        )
        .bind(encode_state(state))
        .bind(exit_code)
        .bind(sql_i64(finished_at, "finished_at")?)
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Acknowledge terminal notification delivery; live or missing rows are no-ops.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn mark_notified(&self, job_id: &str) -> Result<()> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "UPDATE background_jobs SET notified = 1 \
             WHERE job_id = ? AND state IN ('done', 'killed', 'lost') AND notified = 0",
        )
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(())
    }
}

const fn encode_state(state: BackgroundState) -> &'static str {
    match state {
        BackgroundState::Queued => "queued",
        BackgroundState::Running => "running",
        BackgroundState::Done => "done",
        BackgroundState::Killed => "killed",
        BackgroundState::Lost => "lost",
    }
}

fn decode_state(value: &str) -> Result<BackgroundState> {
    match value {
        "queued" => Ok(BackgroundState::Queued),
        "running" => Ok(BackgroundState::Running),
        "done" => Ok(BackgroundState::Done),
        "killed" => Ok(BackgroundState::Killed),
        "lost" => Ok(BackgroundState::Lost),
        other => Err(invalid(format!("unknown background state: {other}"))),
    }
}

fn decode_job(row: &sqlx::sqlite::SqliteRow) -> Result<BackgroundJob> {
    Ok(BackgroundJob {
        job_id: row.try_get("job_id").map_err(adapter_error)?,
        owner: SeatId(row.try_get("owner").map_err(adapter_error)?),
        title: row.try_get("title").map_err(adapter_error)?,
        command: row.try_get("command").map_err(adapter_error)?,
        pid: row
            .try_get::<Option<i64>, _>("pid")
            .map_err(adapter_error)?
            .map(|value| sql_u32(value, "pid"))
            .transpose()?,
        proc_start: row
            .try_get::<Option<i64>, _>("proc_start")
            .map_err(adapter_error)?
            .map(|value| sql_u64(value, "proc_start"))
            .transpose()?,
        pgid: row
            .try_get::<Option<i64>, _>("pgid")
            .map_err(adapter_error)?
            .map(|value| sql_u32(value, "pgid"))
            .transpose()?,
        out_path: row.try_get("out_path").map_err(adapter_error)?,
        state: decode_state(row.try_get("state").map_err(adapter_error)?)?,
        exit_code: row.try_get("exit_code").map_err(adapter_error)?,
        started_at: sql_u64(
            row.try_get("started_at").map_err(adapter_error)?,
            "started_at",
        )?,
        finished_at: row
            .try_get::<Option<i64>, _>("finished_at")
            .map_err(adapter_error)?
            .map(|value| sql_u64(value, "finished_at"))
            .transpose()?,
        kill_requested: row.try_get("kill_requested").map_err(adapter_error)?,
        notified: row.try_get("notified").map_err(adapter_error)?,
        deadline_at: row
            .try_get::<Option<i64>, _>("deadline_at")
            .map_err(adapter_error)?
            .map(|value| sql_u64(value, "deadline_at"))
            .transpose()?,
        timed_out: row.try_get("timed_out").map_err(adapter_error)?,
        term_sent: row.try_get("term_sent").map_err(adapter_error)?,
    })
}

fn sql_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        invalid(format!(
            "{field}={value} exceeds SQLite's signed integer range"
        ))
    })
}

fn sql_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| invalid(format!("{field}={value} is negative in SQLite")))
}

fn sql_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value)
        .map_err(|_| invalid(format!("{field}={value} is outside the process id range")))
}

fn adapter_error(error: sqlx::Error) -> PijError {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "store/background".to_string(),
        message: message.into(),
    }
}
