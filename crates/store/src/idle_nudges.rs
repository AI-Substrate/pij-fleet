//! Idle-holder nudges (plan 224, F136, migration 0046).
//!
//! The PA watchdog round reads which seats have in-flight work, each such idle seat's
//! last own event, and marks a stretch nudged BEFORE it sends, so a restart
//! never nudges twice for one stretch.

use std::collections::{BTreeMap, BTreeSet};

use pij_core::error::Result;
use pij_core::idle_nudge::{IN_FLIGHT_CLAIM_KINDS, is_task_instance};
use pij_core::model::SeatId;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

/// Open role holdings with an instance, for [`pij_core::idle_nudge::is_task_instance`]
/// to qualify: the partial index of open rows (`community_assignments_open_by_seat`).
const TASK_HOLDINGS_SQL: &str = "SELECT seat, instance FROM community_assignments \
     INDEXED BY community_assignments_open_by_seat WHERE to_at IS NULL AND instance != ''";

/// Seats with an open claim of an in-flight kind
/// ([`pij_core::idle_nudge::IN_FLIGHT_CLAIM_KINDS`]): the partial index of open
/// claims (`claims_open_by_seat`).
const WORK_CLAIMS_SQL: &str = "SELECT DISTINCT seat FROM claims INDEXED BY claims_open_by_seat \
     WHERE released_at IS NULL AND kind IN (?1, ?2)";

/// Role requests delivered to a holder and not answered. A request that is
/// answered and closed leaves `role_requests` (migration 0040), so the table
/// holds only open requests: queued, being delivered, or delivered. A
/// `--reply none` request expects no answer, so it is never unanswered work
/// (F150).
const UNANSWERED_REQUESTS_SQL: &str = "SELECT id, holder, project, role, instance, delivered_at \
     FROM role_requests \
     WHERE state = 'delivered' AND answered_at IS NULL AND holder IS NOT NULL \
     AND reply <> 'none'";

/// A delivered role request a seat has not answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// The request id.
    pub id: String,
    /// The request's project.
    pub project: String,
    /// The role it was sent to.
    pub role: String,
    /// The role's instance, empty for none.
    pub instance: String,
    /// When it was delivered, in ms (0 if unrecorded).
    pub delivered_at: u64,
}

/// The in-flight work a nudge names: the keys the conditional mark checks
/// again, so a nudge is never sent for work that ended since the round read
/// it (review of #195).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NudgeWork {
    /// Task holdings as `(project, role, instance)`.
    pub holdings: Vec<(String, String, String)>,
    /// Work claims as `(project, kind, number)`.
    pub claims: Vec<(String, String, u64)>,
    /// Unanswered delivered request ids.
    pub requests: Vec<String>,
}

/// The seats with real in-flight work (F144): a per-task instance holding, an
/// open plan or fix claim, or a delivered role request not yet answered. A
/// bare long-lived role holding is not in-flight work by itself.
#[derive(Debug, Default)]
pub struct InFlight {
    /// Every seat with in-flight work.
    pub seats: BTreeSet<SeatId>,
    /// Each seat's unanswered delivered requests.
    pub requests: BTreeMap<SeatId, Vec<OpenRequest>>,
}

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
/// live and idle, it has no own event after `anchor` (one range seek on the
/// same index), and at least one item of the in-flight work the nudge names
/// is still open: the same task holding, the same work claim, or the same
/// request still delivered and unanswered (`?4`, `?5`, `?6`: JSON arrays of
/// [`NudgeWork`]'s keys, each looked up through the seat's open-row index or
/// the request's primary key). A veto inserts nothing, so nothing is sent
/// (review of #189: an own event between the round's read and this write;
/// review of #195: work that ended since the round read it). Brakes only:
/// each condition can refuse a mark, none can add one.
fn mark_sql() -> String {
    format!(
        "INSERT OR IGNORE INTO idle_nudges (seat, anchor, nudged_at) SELECT ?1, ?2, ?3 \
         WHERE EXISTS (SELECT 1 FROM seats WHERE id = ?1 AND state = 'idle' AND tombstoned_at IS NULL) \
         AND NOT EXISTS (SELECT 1 FROM spine_events INDEXED BY spine_own_events \
         WHERE seat = ?1 AND {OWN_EVENT} AND seq > ?2) \
         AND ({HOLDING_STILL_OPEN} OR {CLAIM_STILL_OPEN} OR {REQUEST_STILL_OPEN})"
    )
}

/// One of the nudge's task holdings is still open.
const HOLDING_STILL_OPEN: &str = "EXISTS (SELECT 1 FROM json_each(?4) AS held \
     JOIN community_assignments AS a INDEXED BY community_assignments_open_by_seat \
     ON a.seat = ?1 AND a.to_at IS NULL \
     AND a.project = json_extract(held.value, '$[0]') \
     AND a.role = json_extract(held.value, '$[1]') \
     AND a.instance = json_extract(held.value, '$[2]'))";

/// One of the nudge's work claims is still open.
const CLAIM_STILL_OPEN: &str = "EXISTS (SELECT 1 FROM json_each(?5) AS held \
     JOIN claims AS c INDEXED BY claims_open_by_seat \
     ON c.seat = ?1 AND c.released_at IS NULL \
     AND c.project = json_extract(held.value, '$[0]') \
     AND c.kind = json_extract(held.value, '$[1]') \
     AND c.number = json_extract(held.value, '$[2]'))";

/// One of the nudge's requests is still delivered to the seat and unanswered,
/// and expects an answer (F150).
const REQUEST_STILL_OPEN: &str = "EXISTS (SELECT 1 FROM json_each(?6) AS held \
     JOIN role_requests AS r ON r.id = held.value \
     WHERE r.holder = ?1 AND r.state = 'delivered' AND r.answered_at IS NULL \
     AND r.reply <> 'none')";

/// Whether `seat` owns a live (queued or running) background job: one seek on
/// `background_jobs_live_by_owner`.
const OWNS_LIVE_JOB_SQL: &str = "SELECT EXISTS (SELECT 1 FROM background_jobs \
     INDEXED BY background_jobs_live_by_owner \
     WHERE owner = ?1 AND state IN ('queued', 'running'))";

impl SqliteOrchestration {
    /// The seats with real in-flight work ([`InFlight`]): three reads per
    /// round, none per seat.
    ///
    /// # Errors
    /// Store failures.
    pub async fn in_flight(&self) -> Result<InFlight> {
        require_current_schema(&self.pool).await?;
        let mut in_flight = InFlight::default();
        for row in sqlx::query(TASK_HOLDINGS_SQL)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
        {
            let instance: String = row.try_get("instance").map_err(adapter_error)?;
            if is_task_instance(&instance) {
                in_flight
                    .seats
                    .insert(SeatId(row.try_get("seat").map_err(adapter_error)?));
            }
        }
        for row in sqlx::query(WORK_CLAIMS_SQL)
            .bind(IN_FLIGHT_CLAIM_KINDS[0])
            .bind(IN_FLIGHT_CLAIM_KINDS[1])
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
        {
            in_flight
                .seats
                .insert(SeatId(row.try_get("seat").map_err(adapter_error)?));
        }
        for row in sqlx::query(UNANSWERED_REQUESTS_SQL)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
        {
            let holder = SeatId(row.try_get("holder").map_err(adapter_error)?);
            in_flight.seats.insert(holder.clone());
            in_flight
                .requests
                .entry(holder)
                .or_default()
                .push(OpenRequest {
                    id: row.try_get("id").map_err(adapter_error)?,
                    project: row.try_get("project").map_err(adapter_error)?,
                    role: row.try_get("role").map_err(adapter_error)?,
                    instance: row.try_get("instance").map_err(adapter_error)?,
                    delivered_at: row
                        .try_get::<Option<i64>, _>("delivered_at")
                        .map_err(adapter_error)?
                        .and_then(|at| u64::try_from(at).ok())
                        .unwrap_or_default(),
                });
        }
        Ok(in_flight)
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
    /// after `anchor`, is no longer live and idle, or none of `work` is still
    /// open (review of #195). Committed by an owned
    /// write, so it completes even if the caller is dropped.
    ///
    /// # Errors
    /// Store failures, or an out-of-range value.
    pub async fn mark_idle_nudged(
        &self,
        seat: &SeatId,
        anchor: u64,
        at: u64,
        work: &NudgeWork,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let seat = seat.to_string();
        let (anchor, at) = (sql_i64(anchor, "anchor")?, sql_i64(at, "nudged_at")?);
        let json = |value: serde_json::Result<String>| {
            value.map_err(|cause| pij_core::error::PijError::Adapter {
                adapter: "store/idle-nudges".to_string(),
                message: format!("nudge work keys: {cause}"),
            })
        };
        let holdings = json(serde_json::to_string(&work.holdings))?;
        let claims = json(serde_json::to_string(&work.claims))?;
        let requests = json(serde_json::to_string(&work.requests))?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = sqlx::query(&mark_sql())
                .bind(&seat)
                .bind(anchor)
                .bind(at)
                .bind(&holdings)
                .bind(&claims)
                .bind(&requests)
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
        let holdings = plan(
            format!("EXPLAIN QUERY PLAN {}", super::TASK_HOLDINGS_SQL),
            vec![],
        )
        .await;
        let claims = plan(
            format!("EXPLAIN QUERY PLAN {}", super::WORK_CLAIMS_SQL),
            vec!["plan", "fix"],
        )
        .await;
        let last = plan(
            format!("EXPLAIN QUERY PLAN {}", super::last_own_sql()),
            vec!["pij-x"],
        )
        .await;
        let mark = plan(
            format!("EXPLAIN QUERY PLAN {}", super::mark_sql()),
            vec!["pij-x", "7", "1", "[]", "[]", "[]"],
        )
        .await;
        let jobs = plan(
            format!("EXPLAIN QUERY PLAN {}", super::OWNS_LIVE_JOB_SQL),
            vec!["pij-x"],
        )
        .await;
        for (index, plan) in [
            ("community_assignments_open_by_seat", &holdings),
            ("claims_open_by_seat", &claims),
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
