//! Deliveries held behind a busy turn (plan 214, AC2; migration 0038).
//!
//! A delivery's held time is `jobs.acked_at - jobs.enqueued_at`. The ack that
//! ends a hold past [`HELD_FLAG_SECS`] records it in `held_deliveries`; the PA
//! digest lists those not yet shown, plus deliveries still queued past the
//! threshold, then marks what it showed, so each message is flagged once.

use pij_core::error::Result;
use sqlx::Row;

use crate::governance::invalid_record;
use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

/// A delivery held longer than this, in seconds, is flagged to the PA.
pub const HELD_FLAG_SECS: i64 = 15 * 60;

/// At most this many held deliveries per digest, oldest first.
pub const HELD_FLAG_LIMIT: i64 = 50;

/// One delivery held past [`HELD_FLAG_SECS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldDelivery {
    /// The queue row.
    pub job_id: i64,
    /// The recipient seat.
    pub recipient: String,
    /// The sending seat, when the payload names one.
    pub sender: Option<String>,
    /// The message id.
    pub msg_id: String,
    /// How long it was held (to its arrival, or to `now` while still queued).
    pub held_ms: u64,
    /// Still waiting for the recipient's session.
    pub still_queued: bool,
}

/// One fleet's unflagged rows (`?4`, a JSON array of recipient seats): holds
/// that ended past the threshold, then deliveries still queued past it that no
/// digest has shown. Both halves are scoped to the fleet before the order and
/// the limit, so another fleet's holds never take this one's page; each walks
/// an index by recipient.
const HELD_SQL: &str = "\
    SELECT jobs.id AS job_id, jobs.serial_key AS recipient, \
           json_extract(jobs.payload, '$.from') AS sender, jobs.dedupe_key AS msg_id, \
           (jobs.acked_at - jobs.enqueued_at) * 1000 AS held_ms, 0 AS still_queued, \
           jobs.enqueued_at AS since \
      FROM held_deliveries INDEXED BY held_deliveries_unflagged \
      JOIN jobs ON jobs.id = held_deliveries.job_id \
     WHERE held_deliveries.flagged_at IS NULL \
       AND held_deliveries.recipient IN (SELECT value FROM json_each(?4)) \
    UNION ALL \
    SELECT jobs.id, jobs.serial_key, json_extract(jobs.payload, '$.from'), jobs.dedupe_key, \
           (?1 - jobs.enqueued_at) * 1000, 1, jobs.enqueued_at \
      FROM jobs INDEXED BY jobs_serial \
     WHERE jobs.serial_key IN (SELECT value FROM json_each(?4)) \
       AND jobs.state IN ('pending', 'running') AND jobs.kind LIKE 'delivery:%' \
       AND jobs.enqueued_at <= ?1 - ?2 \
       AND NOT EXISTS (SELECT 1 FROM held_deliveries WHERE held_deliveries.job_id = jobs.id) \
    ORDER BY since, job_id LIMIT ?3";

impl SqliteOrchestration {
    /// Deliveries to `recipients` held past [`HELD_FLAG_SECS`] that no digest
    /// has shown, at `now_secs`, oldest first and at most [`HELD_FLAG_LIMIT`].
    pub async fn unflagged_held_deliveries(
        &self,
        recipients: &[&str],
        now_secs: u64,
    ) -> Result<Vec<HeldDelivery>> {
        if recipients.is_empty() {
            return Ok(Vec::new());
        }
        require_current_schema(&self.pool).await?;
        let recipients = serde_json::to_string(recipients)
            .map_err(|error| invalid_record(&error.to_string()))?;
        sqlx::query(HELD_SQL)
            .bind(sql_i64(now_secs, "now")?)
            .bind(HELD_FLAG_SECS)
            .bind(HELD_FLAG_LIMIT)
            .bind(recipients)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .iter()
            .map(|row| {
                let held: i64 = row.try_get("held_ms").map_err(adapter_error)?;
                let queued: i64 = row.try_get("still_queued").map_err(adapter_error)?;
                Ok(HeldDelivery {
                    job_id: row.try_get("job_id").map_err(adapter_error)?,
                    recipient: row.try_get("recipient").map_err(adapter_error)?,
                    sender: row.try_get("sender").map_err(adapter_error)?,
                    msg_id: row.try_get("msg_id").map_err(adapter_error)?,
                    held_ms: sql_u64(held.max(0), "held_ms")?,
                    still_queued: queued != 0,
                })
            })
            .collect()
    }

    /// A digest showed these held deliveries at `now_secs`: never again.
    pub async fn flag_held_deliveries(&self, job_ids: &[i64], now_secs: u64) -> Result<()> {
        if job_ids.is_empty() {
            return Ok(());
        }
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let ids = job_ids.to_vec();
        let now = sql_i64(now_secs, "now")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            for id in ids {
                sqlx::query(
                    "INSERT INTO held_deliveries (job_id, recipient, flagged_at) \
                     SELECT id, serial_key, ?2 FROM jobs WHERE id = ?1 \
                     ON CONFLICT (job_id) DO UPDATE SET flagged_at = excluded.flagged_at \
                     WHERE held_deliveries.flagged_at IS NULL",
                )
                .bind(id)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    /// AC7: the digest's held read walks its recipients' rows through
    /// `held_deliveries_unflagged` and `jobs_serial` and the jobs primary key;
    /// it never scans job history or another fleet's holds.
    #[tokio::test]
    async fn the_held_read_never_scans_job_history() {
        let pool = crate::open("").await.expect("memory");
        let plan: Vec<String> = sqlx::query(&format!("EXPLAIN QUERY PLAN {}", super::HELD_SQL))
            .bind(1_000_000_i64)
            .bind(super::HELD_FLAG_SECS)
            .bind(super::HELD_FLAG_LIMIT)
            .bind(r#"["pij-a","pij-b"]"#)
            .fetch_all(&pool)
            .await
            .expect("plan")
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect();
        let text = plan.join("\n");
        for index in ["held_deliveries_unflagged", "jobs_serial"] {
            assert!(
                plan.iter()
                    .any(|line| line.contains(&format!("INDEX {index}"))),
                "{index} unused:\n{text}"
            );
        }
        assert!(
            !plan
                .iter()
                .any(|line| line.starts_with("SCAN jobs")
                    || line.starts_with("SCAN held_deliveries")),
            "a scan:\n{text}"
        );
    }
}
