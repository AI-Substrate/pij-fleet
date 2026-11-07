//! Modal-dialog stalls (plan 219, F47, migration 0040).
//!
//! The PA watchdog claims a stall here before it tells anyone, keyed by the
//! seat and the stretch's identity (the spine sequence of its last own event),
//! and records each recipient that accepted, so each is told once per stretch
//! across restarts and overlapping rounds, and a failed tell is retried.
//! `pij list` and the state card read the standing ones.

use pij_core::error::Result;
use pij_core::modal_stall::{ModalStall, OWN_ACTIVITY_KINDS};
use pij_core::model::SeatId;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

const COLUMNS: &str = "seat, quiet_seq, quiet_since, claimed_at, dialog, key, clears, open, told";

impl SqliteOrchestration {
    /// Claim `stall`'s stretch before anyone is told. `true` means this call
    /// claimed it (nobody told yet); `false` means this stretch, or a later
    /// one, is already claimed. A later stretch (a higher sequence) replaces
    /// an earlier one's row, `told` and all.
    ///
    /// # Errors
    /// Store failures, or an out-of-range value.
    pub async fn claim_modal_stall(&self, stall: &ModalStall) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let stall = stall.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let done = sqlx::query(&format!(
                "INSERT INTO modal_stalls ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, '[]') \
                 ON CONFLICT (seat) DO UPDATE SET quiet_seq = excluded.quiet_seq, \
                 quiet_since = excluded.quiet_since, claimed_at = excluded.claimed_at, \
                 dialog = excluded.dialog, key = excluded.key, clears = excluded.clears, \
                 open = 1, told = '[]' \
                 WHERE modal_stalls.quiet_seq < excluded.quiet_seq"
            ))
            .bind(stall.seat.as_str())
            .bind(sql_i64(stall.quiet_seq, "quiet_seq")?)
            .bind(sql_i64(stall.quiet_since_ms, "quiet_since")?)
            .bind(sql_i64(stall.claimed_at_ms, "claimed_at")?)
            .bind(&stall.dialog)
            .bind(&stall.key)
            .bind(&stall.clears)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(done.rows_affected() > 0)
        })
        .await
    }

    /// Record that `target` accepted this stretch's message. Another
    /// stretch's row, or a target already recorded, is left alone. Returns
    /// whether it was recorded now.
    ///
    /// # Errors
    /// Store failures, or an out-of-range sequence.
    pub async fn mark_modal_stall_told(
        &self,
        seat: &SeatId,
        quiet_seq: u64,
        target: &SeatId,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let (seat, target) = (seat.clone(), target.clone());
        let quiet_seq = sql_i64(quiet_seq, "quiet_seq")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let done = sqlx::query(
                "UPDATE modal_stalls SET told = json_insert(told, '$[#]', ?3) \
                 WHERE seat = ?1 AND quiet_seq = ?2 \
                 AND NOT EXISTS (SELECT 1 FROM json_each(told) WHERE value = ?3)",
            )
            .bind(seat.as_str())
            .bind(quiet_seq)
            .bind(target.as_str())
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(done.rows_affected() > 0)
        })
        .await
    }

    /// Record whether the stretch's dialog is still showing. Another stretch's
    /// row is left alone. Returns whether the row changed.
    ///
    /// # Errors
    /// Store failures, or an out-of-range sequence.
    pub async fn set_modal_stall_open(
        &self,
        seat: &SeatId,
        quiet_seq: u64,
        open: bool,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let seat = seat.clone();
        let quiet_seq = sql_i64(quiet_seq, "quiet_seq")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let done = sqlx::query(
                "UPDATE modal_stalls SET open = ?3 \
                 WHERE seat = ?1 AND quiet_seq = ?2 AND open != ?3",
            )
            .bind(seat.as_str())
            .bind(quiet_seq)
            .bind(i64::from(open))
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(done.rows_affected() > 0)
        })
        .await
    }

    /// Every seat's latest stall, standing or not: the round's one read.
    ///
    /// # Errors
    /// Store failures.
    pub async fn modal_stalls(&self) -> Result<Vec<ModalStall>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!("SELECT {COLUMNS} FROM modal_stalls ORDER BY seat"))
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .iter()
            .map(modal_stall)
            .collect()
    }

    /// The stalls standing now (for `seat`, or every seat): still showing,
    /// and the seat has recorded no own event since the stretch began, by
    /// sequence. One read, through the spine's `(seat, kind, seq)` index.
    ///
    /// # Errors
    /// Store failures.
    pub async fn standing_modal_stalls(&self, seat: Option<&SeatId>) -> Result<Vec<ModalStall>> {
        require_current_schema(&self.pool).await?;
        let kinds = (0..OWN_ACTIVITY_KINDS.len())
            .map(|index| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {COLUMNS} FROM modal_stalls AS m \
             WHERE m.open = 1 AND (?1 IS NULL OR m.seat = ?1) \
             AND NOT EXISTS (SELECT 1 FROM spine_events AS e \
             WHERE e.seat = m.seat AND e.kind IN ({kinds}) AND e.seq > m.quiet_seq) \
             ORDER BY m.seat"
        );
        let mut query = sqlx::query(&sql).bind(seat.map(SeatId::as_str));
        for kind in OWN_ACTIVITY_KINDS {
            query = query.bind(kind);
        }
        query
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .iter()
            .map(modal_stall)
            .collect()
    }
}

fn modal_stall(row: &sqlx::sqlite::SqliteRow) -> Result<ModalStall> {
    let seat: String = row.try_get("seat").map_err(adapter_error)?;
    let told: String = row.try_get("told").map_err(adapter_error)?;
    let told: Vec<String> =
        serde_json::from_str(&told).map_err(|error| pij_core::error::PijError::Adapter {
            adapter: "store/modal-stalls".into(),
            message: format!("told for {seat} is not a JSON list: {error}"),
        })?;
    Ok(ModalStall {
        quiet_seq: sql_u64(
            row.try_get("quiet_seq").map_err(adapter_error)?,
            "quiet_seq",
        )?,
        quiet_since_ms: sql_u64(
            row.try_get("quiet_since").map_err(adapter_error)?,
            "quiet_since",
        )?,
        claimed_at_ms: sql_u64(
            row.try_get("claimed_at").map_err(adapter_error)?,
            "claimed_at",
        )?,
        dialog: row.try_get("dialog").map_err(adapter_error)?,
        key: row.try_get("key").map_err(adapter_error)?,
        clears: row.try_get("clears").map_err(adapter_error)?,
        open: row.try_get::<i64, _>("open").map_err(adapter_error)? == 1,
        told: told.into_iter().map(SeatId::from).collect(),
        seat: SeatId::from(seat),
    })
}
