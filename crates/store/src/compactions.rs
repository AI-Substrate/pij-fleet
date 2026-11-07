//! Compaction wakes (plan 222, F111, migration 0044).
//!
//! The compact control's admission records a pending row BEFORE it admits
//! the compact; the PA watchdog's round reads only the pending rows and
//! settles each one, marking a wake BEFORE it sends, so a restart never sends
//! twice.

use pij_core::compaction_wake::{PendingCompaction, STARTED_KINDS, Settled};
use pij_core::error::Result;
use pij_core::model::SeatId;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

impl SqliteOrchestration {
    /// Record that `seat`'s compact control `msg_id` is about to be
    /// admitted, at `at`. A replay records nothing new (`false`). Committed
    /// by an owned write, so it completes even if the caller is dropped.
    /// Older pending rows of the same seat stay: the round supersedes them
    /// only once this compact is delivered.
    ///
    /// # Errors
    /// Store failures, or an out-of-range time.
    pub async fn record_compaction(&self, seat: &SeatId, msg_id: &str, at: u64) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let (seat, msg_id) = (seat.to_string(), msg_id.to_string());
        let at = sql_i64(at, "admitted_at")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO compaction_wakes (seat, msg_id, admitted_at) \
                 VALUES (?1, ?2, ?3)",
            )
            .bind(&seat)
            .bind(&msg_id)
            .bind(at)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected()
                > 0;
            tx.commit().await.map_err(adapter_error)?;
            Ok(inserted)
        })
        .await
    }

    /// Every compaction not yet settled: the round's one read, off the
    /// `compaction_wakes_pending` partial index.
    ///
    /// # Errors
    /// Store failures.
    pub async fn pending_compactions(&self) -> Result<Vec<PendingCompaction>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "SELECT seat, msg_id, admitted_at FROM compaction_wakes \
             WHERE settled IS NULL ORDER BY seat, admitted_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(|row| {
            let seat: String = row.try_get("seat").map_err(adapter_error)?;
            Ok(PendingCompaction {
                seat: SeatId::from(seat.as_str()),
                msg_id: row.try_get("msg_id").map_err(adapter_error)?,
                admitted_at_ms: sql_u64(
                    row.try_get("admitted_at").map_err(adapter_error)?,
                    "admitted_at",
                )?,
            })
        })
        .collect()
    }

    /// Settle one pending compaction. `true` means this call settled it;
    /// `false` means it was already settled (by an overlapping round), so a
    /// caller about to wake the seat must not.
    ///
    /// # Errors
    /// Store failures, or an out-of-range time.
    pub async fn settle_compaction(
        &self,
        seat: &SeatId,
        msg_id: &str,
        settled: Settled,
        at: u64,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let (seat, msg_id) = (seat.to_string(), msg_id.to_string());
        let at = sql_i64(at, "settled_at")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let done = sqlx::query(
                "UPDATE compaction_wakes SET settled = ?3, settled_at = ?4 \
                 WHERE seat = ?1 AND msg_id = ?2 AND settled IS NULL",
            )
            .bind(&seat)
            .bind(&msg_id)
            .bind(settled.as_str())
            .bind(at)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(done.rows_affected() > 0)
        })
        .await
    }

    /// How `seat`'s compaction `msg_id` was settled: `None` with no row,
    /// `Some(None)` while pending.
    ///
    /// # Errors
    /// Store failures.
    pub async fn compaction_settled(
        &self,
        seat: &SeatId,
        msg_id: &str,
    ) -> Result<Option<Option<String>>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT settled FROM compaction_wakes WHERE seat = ?1 AND msg_id = ?2")
            .bind(seat.as_str())
            .bind(msg_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(|row| row.try_get("settled").map_err(adapter_error))
            .transpose()
    }

    /// Has `seat` started anything since spine sequence `after`: an own event
    /// other than `seat.activity` idle, or a message pushed to it? One read
    /// through the spine's `(seat, kind, seq)` index, bounded by the events
    /// after `after`.
    ///
    /// # Errors
    /// Store failures, or an out-of-range sequence.
    pub async fn started_since(&self, seat: &SeatId, after: u64) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let kinds = (0..STARTED_KINDS.len())
            .map(|index| format!("?{}", index + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM spine_events \
             WHERE seat = ?1 AND kind IN ({kinds}) AND seq > ?2 \
             AND NOT (kind = 'seat.activity' AND json_extract(payload, '$.state') = 'idle'))"
        );
        let mut query = sqlx::query(&sql)
            .bind(seat.as_str())
            .bind(sql_i64(after, "after")?);
        for kind in STARTED_KINDS {
            query = query.bind(kind);
        }
        let started: i64 = query
            .fetch_one(&self.pool)
            .await
            .map_err(adapter_error)?
            .try_get(0)
            .map_err(adapter_error)?;
        Ok(started != 0)
    }
}
