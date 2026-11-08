//! Idle-holder nudges (plan 224, F136, migration 0046).
//!
//! The PA watchdog round reads which seats hold work, each such idle seat's
//! last own event, and marks a stretch nudged BEFORE it sends, so a restart
//! never nudges twice for one stretch.

use pij_core::error::Result;
use pij_core::idle_nudge::OWN_KINDS;
use pij_core::model::SeatId;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

/// Every seat with an open role holding or an open claim: two reads of
/// partial indexes (`community_assignments_open_by_seat`, `claims_open_by_seat`).
const HOLDING_SEATS_SQL: &str = "SELECT seat FROM community_assignments \
     INDEXED BY community_assignments_open_by_seat WHERE to_at IS NULL \
     UNION SELECT seat FROM claims INDEXED BY claims_open_by_seat WHERE released_at IS NULL \
     ORDER BY seat";

/// The seat's newest event of one own kind (`seat.activity` other than idle,
/// for that kind): one seek down `spine_by_seat_kind`.
const LAST_OWN_SQL: &str = "SELECT seq, at FROM spine_events INDEXED BY spine_by_seat_kind \
     WHERE seat = ?1 AND kind = ?2 \
     AND NOT (kind = 'seat.activity' AND json_extract(payload, '$.state') = 'idle') \
     ORDER BY seq DESC LIMIT 1";

impl SqliteOrchestration {
    /// Every seat that holds work: an open role holding or an open claim.
    ///
    /// # Errors
    /// Store failures.
    pub async fn holding_seats(&self) -> Result<Vec<SeatId>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(HOLDING_SEATS_SQL)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .iter()
            .map(|row| {
                row.try_get::<String, _>("seat")
                    .map(SeatId)
                    .map_err(adapter_error)
            })
            .collect()
    }

    /// `seat`'s last own event ([`OWN_KINDS`]; `seat.activity` idle doesn't
    /// count), as `(spine seq, at)`, or `None` when it has none. One seek per
    /// own kind.
    ///
    /// # Errors
    /// Store failures, or an out-of-range stored value.
    pub async fn last_own_event(&self, seat: &SeatId) -> Result<Option<(u64, u64)>> {
        require_current_schema(&self.pool).await?;
        let mut last: Option<(u64, u64)> = None;
        for kind in OWN_KINDS {
            let Some(row) = sqlx::query(LAST_OWN_SQL)
                .bind(seat.as_str())
                .bind(kind)
                .fetch_optional(&self.pool)
                .await
                .map_err(adapter_error)?
            else {
                continue;
            };
            let seq = sql_u64(row.try_get("seq").map_err(adapter_error)?, "seq")?;
            let at = sql_u64(row.try_get("at").map_err(adapter_error)?, "at")?;
            if last.is_none_or(|(newest, _)| seq > newest) {
                last = Some((seq, at));
            }
        }
        Ok(last)
    }

    /// Whether `seat` was already nudged for the stretch its event `anchor`
    /// began.
    ///
    /// # Errors
    /// Store failures, or an out-of-range anchor.
    pub async fn idle_nudged(&self, seat: &SeatId, anchor: u64) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let found: i64 = sqlx::query(
            "SELECT EXISTS (SELECT 1 FROM idle_nudges WHERE seat = ?1 AND anchor = ?2)",
        )
        .bind(seat.as_str())
        .bind(sql_i64(anchor, "anchor")?)
        .fetch_one(&self.pool)
        .await
        .map_err(adapter_error)?
        .try_get(0)
        .map_err(adapter_error)?;
        Ok(found != 0)
    }

    /// Mark `seat` nudged for the stretch `anchor` began, at `at`. `true` when
    /// this call added the mark, which alone authorizes the one nudge; a mark
    /// already there (a restart, an overlapping round) is `false`. Committed by
    /// an owned write, so it completes even if the caller is dropped.
    ///
    /// # Errors
    /// Store failures, or an out-of-range value.
    pub async fn mark_idle_nudged(&self, seat: &SeatId, anchor: u64, at: u64) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let seat = seat.to_string();
        let (anchor, at) = (sql_i64(anchor, "anchor")?, sql_i64(at, "nudged_at")?);
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO idle_nudges (seat, anchor, nudged_at) VALUES (?1, ?2, ?3)",
            )
            .bind(&seat)
            .bind(anchor)
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
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    /// Plan 224, AC2: the round's reads walk indexes, never a table: the
    /// holding seats from the two partial indexes of open rows (bounded by
    /// the work held, not by history), the last own event by one seek.
    #[tokio::test]
    async fn idle_nudge_reads_are_indexed() {
        let pool = crate::open("").await.expect("memory");
        let plan = |sql: String, binds: Vec<&'static str>| {
            let pool = pool.clone();
            async move {
                let mut query = sqlx::query(&sql);
                for bind in binds {
                    query = query.bind(bind);
                }
                query
                    .fetch_all(&pool)
                    .await
                    .expect("plan")
                    .iter()
                    .map(|row| row.get::<String, _>("detail"))
                    .collect::<Vec<String>>()
            }
        };
        let holding = plan(
            format!("EXPLAIN QUERY PLAN {}", super::HOLDING_SEATS_SQL),
            vec![],
        )
        .await;
        let last = plan(
            format!("EXPLAIN QUERY PLAN {}", super::LAST_OWN_SQL),
            vec!["pij-x", "report.now"],
        )
        .await;
        for (index, plan) in [
            ("community_assignments_open_by_seat", &holding),
            ("claims_open_by_seat", &holding),
            ("spine_by_seat_kind", &last),
        ] {
            let text = plan.join("\n");
            assert!(
                plan.iter()
                    .any(|line| line.contains(&format!("INDEX {index}"))),
                "{index} unused:\n{text}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|line| (line.starts_with("SCAN spine_events")
                        || line.starts_with("SCAN community_assignments")
                        || line.starts_with("SCAN claims"))
                        && !line.contains("INDEX")),
                "a table scan:\n{text}"
            );
        }
        assert!(
            last.iter()
                .any(|line| line.starts_with("SEARCH spine_events")),
            "the last own event is a seek:\n{}",
            last.join("\n")
        );
    }
}
