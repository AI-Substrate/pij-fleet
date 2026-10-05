//! Durable facts for daemon-owned detached processes and their completion turns.

use pij_core::background::{
    BackgroundEvent, BackgroundJob, BackgroundKind, BackgroundState, EventStats,
};
use pij_core::error::{PijError, Result};
use pij_core::model::{ProcIdentity, SeatId};
use sqlx::Row;

use crate::migrate::{begin_write, owned_write};
use crate::{StorePool, require_current_schema};

/// What one emit did to a source's pending events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitOutcome {
    /// Stored as the job's `seq`-th event.
    Accepted {
        /// Per-job sequence number.
        seq: u64,
    },
    /// Over the pending cap: counted, not stored.
    Dropped {
        /// Emits dropped since the last batch, including this one.
        dropped: u64,
    },
    /// The job is not a live, unkilled event source with a valid hook.
    Refused,
}

/// A numbered batch of events cut for one delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventBatch {
    /// The batch's number, from 1; also its delivery dedupe key.
    pub batch_no: u64,
    /// Its events, oldest first.
    pub events: Vec<BackgroundEvent>,
    /// Emits dropped over the cap before this batch was cut.
    pub dropped: u64,
}

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
        self.insert_with_token(job, None).await
    }

    /// Persist a job together with the SHA-256 (hex) of its event-hook token.
    ///
    /// # Errors
    /// As [`Self::insert`].
    pub async fn insert_with_token(
        &self,
        job: &BackgroundJob,
        token_hash: Option<&str>,
    ) -> Result<()> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "INSERT INTO background_jobs \
             (job_id, owner, title, command, pid, proc_start, pgid, out_path, state, \
              exit_code, started_at, finished_at, kill_requested, notified, deadline_at, \
              timed_out, kind, token_hash, events_fyi, min_interval_ms, inline_max) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(encode_kind(job.kind))
        .bind(token_hash)
        .bind(job.events_fyi)
        .bind(sql_i64(job.min_interval_ms, "min_interval_ms")?)
        .bind(sql_i64(job.inline_max, "inline_max")?)
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
            "UPDATE background_jobs SET state = ?, exit_code = ?, finished_at = ?, \
             token_hash = NULL WHERE job_id = ? AND state IN ('queued', 'running')",
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

    /// The stored SHA-256 (hex) of a job's event-hook token; `None` when the job
    /// is unknown, one-shot, or its token was revoked at finish.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn token_hash(&self, job_id: &str) -> Result<Option<String>> {
        require_current_schema(&self.pool).await?;
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT token_hash FROM background_jobs WHERE job_id = ?",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)
        .map(Option::flatten)
    }

    /// Store one fired event, or count it as dropped once `cap` events are pending.
    ///
    /// Refused unless the job is a running event source with a live token and no
    /// kill request: an emit can never outlive the job it belongs to.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn emit(
        &self,
        job_id: &str,
        ts: u64,
        text: &str,
        data: Option<&str>,
        cap: u64,
    ) -> Result<EmitOutcome> {
        require_current_schema(&self.pool).await?;
        let (pool, job_id, text, data) = (
            self.pool.clone(),
            job_id.to_owned(),
            text.to_owned(),
            data.map(str::to_owned),
        );
        let ts = sql_i64(ts, "ts")?;
        let cap = sql_i64(cap, "cap")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let live: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM background_jobs WHERE job_id = ? AND kind = 'events' \
                 AND state = 'running' AND kill_requested = 0 AND token_hash IS NOT NULL",
            )
            .bind(&job_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            if live.is_none() {
                return Ok(EmitOutcome::Refused);
            }
            let pending: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM bg_events WHERE job_id = ? AND state = 'pending'",
            )
            .bind(&job_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let outcome = if pending >= cap {
                let dropped: i64 = sqlx::query_scalar(
                    "UPDATE background_jobs SET dropped = dropped + 1 WHERE job_id = ? \
                     RETURNING dropped",
                )
                .bind(&job_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(adapter_error)?;
                EmitOutcome::Dropped {
                    dropped: sql_u64(dropped, "dropped")?,
                }
            } else {
                let seq: i64 = sqlx::query_scalar(
                    "INSERT INTO bg_events (job_id, seq, ts, text, data_json) \
                     SELECT ?, COALESCE(MAX(seq), 0) + 1, ?, ?, ? FROM bg_events WHERE job_id = ? \
                     RETURNING seq",
                )
                .bind(&job_id)
                .bind(ts)
                .bind(&text)
                .bind(&data)
                .bind(&job_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(adapter_error)?;
                EmitOutcome::Accepted {
                    seq: sql_u64(seq, "seq")?,
                }
            };
            tx.commit().await.map_err(adapter_error)?;
            Ok(outcome)
        })
        .await
    }

    /// Return the open batch (cut but not yet settled), else cut a new one from
    /// the pending events. `None` when nothing is pending.
    ///
    /// With `all`, the cut also absorbs an open batch (the final flush at a
    /// source's end), so every pending event leaves in exactly one batch.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn cut_batch(&self, job_id: &str, all: bool) -> Result<Option<EventBatch>> {
        require_current_schema(&self.pool).await?;
        let (pool, job_id) = (self.pool.clone(), job_id.to_owned());
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(
                "SELECT batches, dropped, open_dropped FROM background_jobs WHERE job_id = ?",
            )
            .bind(&job_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let Some(row) = row else { return Ok(None) };
            let batches: i64 = row.try_get("batches").map_err(adapter_error)?;
            let dropped: i64 = row.try_get("dropped").map_err(adapter_error)?;
            let open_dropped: i64 = row.try_get("open_dropped").map_err(adapter_error)?;
            let open = batch_events(&mut tx, &job_id, batches).await?;
            if !open.is_empty() && !all {
                return Ok(Some(EventBatch {
                    batch_no: sql_u64(batches, "batches")?,
                    events: open,
                    dropped: sql_u64(open_dropped, "open_dropped")?,
                }));
            }
            let unbatched: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM bg_events \
                 WHERE job_id = ? AND state = 'pending' AND batch_no IS NULL",
            )
            .bind(&job_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;
            if unbatched == 0 && open.is_empty() && !(all && dropped + open_dropped > 0) {
                return Ok(None);
            }
            let batch_no = batches + 1;
            let carried = dropped + open_dropped;
            sqlx::query("UPDATE bg_events SET batch_no = ? WHERE job_id = ? AND state = 'pending'")
                .bind(batch_no)
                .bind(&job_id)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            sqlx::query(
                "UPDATE background_jobs SET batches = ?, dropped = 0, open_dropped = ? \
                 WHERE job_id = ?",
            )
            .bind(batch_no)
            .bind(carried)
            .bind(&job_id)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let events = batch_events(&mut tx, &job_id, batch_no).await?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(Some(EventBatch {
                batch_no: sql_u64(batch_no, "batch_no")?,
                events,
                dropped: sql_u64(carried, "dropped")?,
            }))
        })
        .await
    }

    /// Record how a batch left (`delivered`, `held` or `routed`) and when, which
    /// also starts the next `--min-interval` window.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error for an invalid state or
    /// SQLite failure.
    pub async fn settle_batch(
        &self,
        job_id: &str,
        batch_no: u64,
        state: &str,
        at: u64,
    ) -> Result<()> {
        require_current_schema(&self.pool).await?;
        if !matches!(state, "delivered" | "held" | "routed") {
            return Err(invalid(format!("unknown event settlement: {state}")));
        }
        let (pool, job_id, state) = (self.pool.clone(), job_id.to_owned(), state.to_owned());
        let batch_no = sql_i64(batch_no, "batch_no")?;
        let at = sql_i64(at, "at")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query(
                "UPDATE bg_events SET state = ? \
                 WHERE job_id = ? AND batch_no = ? AND state = 'pending'",
            )
            .bind(&state)
            .bind(&job_id)
            .bind(batch_no)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            sqlx::query(
                "UPDATE background_jobs SET last_wake_at = ?, open_dropped = 0 \
                 WHERE job_id = ? AND batches = ?",
            )
            .bind(at)
            .bind(&job_id)
            .bind(batch_no)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// Count a source's fired and pending events.
    ///
    /// # Errors
    /// Returns a schema error on skew or an adapter error on SQLite failure.
    pub async fn event_stats(&self, job_id: &str) -> Result<EventStats> {
        require_current_schema(&self.pool).await?;
        let row = sqlx::query(
            "SELECT COUNT(*) AS fired, \
                    COALESCE(SUM(state = 'pending'), 0) AS pending, \
                    MAX(ts) AS last_fire_at \
             FROM bg_events WHERE job_id = ?",
        )
        .bind(job_id)
        .fetch_one(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(EventStats {
            fired: sql_u64(row.try_get("fired").map_err(adapter_error)?, "fired")?,
            pending: sql_u64(row.try_get("pending").map_err(adapter_error)?, "pending")?,
            last_fire_at: row
                .try_get::<Option<i64>, _>("last_fire_at")
                .map_err(adapter_error)?
                .map(|value| sql_u64(value, "last_fire_at"))
                .transpose()?,
        })
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

const fn encode_kind(kind: BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Oneshot => "oneshot",
        BackgroundKind::Events => "events",
    }
}

fn decode_kind(value: &str) -> Result<BackgroundKind> {
    match value {
        "oneshot" => Ok(BackgroundKind::Oneshot),
        "events" => Ok(BackgroundKind::Events),
        other => Err(invalid(format!("unknown background kind: {other}"))),
    }
}

fn decode_event(row: &sqlx::sqlite::SqliteRow) -> Result<BackgroundEvent> {
    Ok(BackgroundEvent {
        seq: sql_u64(row.try_get("seq").map_err(adapter_error)?, "seq")?,
        ts: sql_u64(row.try_get("ts").map_err(adapter_error)?, "ts")?,
        text: row.try_get("text").map_err(adapter_error)?,
        data: row.try_get("data_json").map_err(adapter_error)?,
    })
}

async fn batch_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &str,
    batch_no: i64,
) -> Result<Vec<BackgroundEvent>> {
    sqlx::query(
        "SELECT seq, ts, text, data_json FROM bg_events \
         WHERE job_id = ? AND batch_no = ? AND state = 'pending' ORDER BY seq",
    )
    .bind(job_id)
    .bind(batch_no)
    .fetch_all(&mut **tx)
    .await
    .map_err(adapter_error)?
    .iter()
    .map(decode_event)
    .collect()
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
        kind: decode_kind(row.try_get("kind").map_err(adapter_error)?)?,
        events_fyi: row.try_get("events_fyi").map_err(adapter_error)?,
        min_interval_ms: sql_u64(
            row.try_get("min_interval_ms").map_err(adapter_error)?,
            "min_interval_ms",
        )?,
        inline_max: sql_u64(
            row.try_get("inline_max").map_err(adapter_error)?,
            "inline_max",
        )?,
        last_wake_at: row
            .try_get::<Option<i64>, _>("last_wake_at")
            .map_err(adapter_error)?
            .map(|value| sql_u64(value, "last_wake_at"))
            .transpose()?,
        batches: sql_u64(row.try_get("batches").map_err(adapter_error)?, "batches")?,
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
