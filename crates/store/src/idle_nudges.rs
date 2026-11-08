//! Idle-holder nudges (plan 224, F136, migration 0046).
//!
//! The PA watchdog round reads which seats hold work, each such idle seat's
//! last own event, and marks a stretch nudged BEFORE it sends, so a restart
//! never nudges twice for one stretch.

use pij_core::error::Result;
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

/// An own event (`spine_own_events`' predicate, verbatim from migration 0046):
/// a report, or `seat.activity` other than idle.
const OWN_EVENT: &str = "(kind IN ('report.now', 'report.state') \
     OR (kind = 'seat.activity' \
     AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.state') END != 'idle'))";

/// The seat's newest own event: one reverse seek on `spine_own_events`.
fn last_own_sql() -> String {
    format!(
        "SELECT seq, at FROM spine_events INDEXED BY spine_own_events \
         WHERE seat = ?1 AND {OWN_EVENT} ORDER BY seq DESC LIMIT 1"
    )
}

/// The mark, added only while the stretch still stands at commit: the seat is
/// live and idle, and it has no own event after `anchor` (one range seek on
/// the same index). A veto inserts nothing, so nothing is sent (review of
/// #189: an own event between the round's read and this write). Brakes only:
/// each condition can refuse a mark, none can add one.
fn mark_sql() -> String {
    format!(
        "INSERT OR IGNORE INTO idle_nudges (seat, anchor, nudged_at) SELECT ?1, ?2, ?3 \
         WHERE EXISTS (SELECT 1 FROM seats WHERE id = ?1 AND state = 'idle' AND tombstoned_at IS NULL) \
         AND NOT EXISTS (SELECT 1 FROM spine_events INDEXED BY spine_own_events \
         WHERE seat = ?1 AND {OWN_EVENT} AND seq > ?2)"
    )
}

/// Whether `seat` owns a live (queued or running) background job: one seek on
/// `background_jobs_live_by_owner`.
const OWNS_LIVE_JOB_SQL: &str = "SELECT EXISTS (SELECT 1 FROM background_jobs \
     INDEXED BY background_jobs_live_by_owner \
     WHERE owner = ?1 AND state IN ('queued', 'running'))";

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

    /// `seat`'s newest own event (a report, or `seat.activity` other than
    /// idle), as `(spine seq, at)`, or `None` when it has none. One seek,
    /// whatever idle activity trails it.
    ///
    /// # Errors
    /// Store failures, or an out-of-range stored value.
    pub async fn last_own_event(&self, seat: &SeatId) -> Result<Option<(u64, u64)>> {
        require_current_schema(&self.pool).await?;
        let Some(row) = sqlx::query(&last_own_sql())
            .bind(seat.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
        else {
            return Ok(None);
        };
        Ok(Some((
            sql_u64(row.try_get("seq").map_err(adapter_error)?, "seq")?,
            sql_u64(row.try_get("at").map_err(adapter_error)?, "at")?,
        )))
    }

    /// Whether `seat` owns a queued or running background job.
    ///
    /// # Errors
    /// Store failures.
    pub async fn owns_live_job(&self, seat: &SeatId) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let found: i64 = sqlx::query(OWNS_LIVE_JOB_SQL)
            .bind(seat.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(adapter_error)?
            .try_get(0)
            .map_err(adapter_error)?;
        Ok(found != 0)
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
    /// this call added the mark, which alone authorizes the one nudge. `false`
    /// when a mark is already there (a restart, an overlapping round), or when
    /// the stretch no longer stands at commit: the seat posted an own event
    /// after `anchor`, or is no longer live and idle. Committed by an owned
    /// write, so it completes even if the caller is dropped.
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
            let inserted = sqlx::query(&mark_sql())
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
            format!("EXPLAIN QUERY PLAN {}", super::last_own_sql()),
            vec!["pij-x"],
        )
        .await;
        let mark = plan(
            format!("EXPLAIN QUERY PLAN {}", super::mark_sql()),
            vec!["pij-x", "7", "1"],
        )
        .await;
        let jobs = plan(
            format!("EXPLAIN QUERY PLAN {}", super::OWNS_LIVE_JOB_SQL),
            vec!["pij-x"],
        )
        .await;
        for (index, plan) in [
            ("community_assignments_open_by_seat", &holding),
            ("claims_open_by_seat", &holding),
            ("spine_own_events", &last),
            ("spine_own_events", &mark),
            ("background_jobs_live_by_owner", &jobs),
        ] {
            let text = plan.join("\n");
            assert!(
                plan.iter()
                    .any(|line| line.contains(&format!("INDEX {index}"))),
                "{index} unused:\n{text}"
            );
            assert!(
                !plan.iter().any(|line| line.starts_with("SCAN")
                    && !line.starts_with("SCAN CONSTANT ROW")
                    && !line.contains("INDEX")),
                "a table scan:\n{text}"
            );
        }
        // Idle activity is not in `spine_own_events` at all, so a seek on it
        // never walks an idle tail (P189-F001); and the newest is read off
        // the index order, never sorted.
        for (what, plan) in [("last own event", &last), ("veto", &mark), ("jobs", &jobs)] {
            let text = plan.join("\n");
            assert!(
                plan.iter().any(|line| line.starts_with("SEARCH")
                    && line.contains("INDEX")
                    && (line.contains("(seat=?") || line.contains("(owner=?"))),
                "{what} is a keyed seek:\n{text}"
            );
            assert!(!text.contains("TEMP B-TREE"), "{what} sorts:\n{text}");
        }
    }

    /// P189-F001: the newest own event is found behind a long idle tail, and
    /// a seat with only idle activity has none; a malformed payload never
    /// fails the spine write the partial index guards.
    #[tokio::test]
    async fn the_newest_own_event_survives_a_long_idle_tail() {
        let pool = crate::open("").await.expect("memory");
        let store = crate::SqliteOrchestration::new(pool.clone());
        let event = |seat: &'static str, kind: &'static str, at: u64, payload: &'static str| {
            let spine = crate::SqliteSpine::new(pool.clone());
            async move {
                pij_core::ports::Spine::append(
                    &spine,
                    pij_core::model::Event {
                        seq: None,
                        v: 1,
                        at,
                        kind: kind.into(),
                        seat: Some(seat.into()),
                        payload: payload.into(),
                    },
                )
                .await
                .expect("spine write")
                .0
            }
        };
        let working = r#"{"state":"working"}"#;
        let idle = r#"{"state":"idle"}"#;
        event("pij-a", "report.now", 10, "{}").await;
        let turn = event("pij-a", "seat.activity", 20, working).await;
        for at in 0..2_000 {
            event("pij-a", "seat.activity", 100 + at, idle).await;
        }
        event("pij-a", "seat.activity", 5_000, "not json").await;
        event("pij-b", "seat.activity", 30, idle).await;
        event("pij-b", "report.now", 40, "{}").await;
        assert_eq!(
            store.last_own_event(&"pij-a".into()).await.unwrap(),
            Some((turn, 20)),
            "the turn, newer than the report, behind 2000 idle records and a malformed one"
        );
        for at in 0..2_000 {
            event("pij-c", "seat.activity", at, idle).await;
        }
        assert_eq!(
            store.last_own_event(&"pij-c".into()).await.unwrap(),
            None,
            "only idle activity is no own event"
        );
    }
}
