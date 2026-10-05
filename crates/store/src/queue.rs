//! The job queue primitive (workshop 001 R4).
//!
//! The daemon is queue-first: a handler does exactly one cheap statement and
//! everything else becomes a job. So this is the primitive delivery, sidecars,
//! watchdog fan-out and chores all ride on, and it is built before any of them.
//!
//! Three semantics, enforced in SQL rather than process-local state:
//!
//! * **Dedupe** — a partial unique index over LIVE rows. N rapid submits collapse
//!   to one row, and the id every caller gets back is that row's.
//! * **Serialization** — at most one RUNNING job per `serial_key`.
//! * **Scoped claim expiry** — expired body `delivery:*` claims return to pending
//!   because an inbox client can die between read and acknowledgement. Controls
//!   and other kinds fail terminally: a missing ACK cannot prove no execution.

use async_trait::async_trait;
use sqlx::{Pool, Row, Sqlite};

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::error::{PijError, Result};
use pij_core::model::{DeliveryFailure, DeliveryOrigin, Job, JobId, Outcome, SeatId};
use pij_core::ports::{
    DeferNoopReason, DeferOutcome, DeliveryAck, DeliveryEnqueue, ExtensionClaim, ExtensionLease,
    ParkedDelivery, Queue, ReleaseOutcome,
};

use crate::migrate::{begin_write, owned_write, require_current_schema};

/// A [`Queue`] backed by the SQLite store.
pub struct SqliteQueue {
    pool: Pool<Sqlite>,
    claim_lease_secs: i64,
    delivered_id_capacity: i64,
}

impl SqliteQueue {
    /// Wrap an open pool with its configured claim age and delivered-id bound.
    ///
    /// # Errors
    /// [`PijError::Adapter`] when either policy is zero or exceeds SQLite's
    /// signed integer range.
    pub fn new(
        pool: Pool<Sqlite>,
        claim_lease_secs: u64,
        delivered_id_capacity: usize,
    ) -> Result<Self> {
        let claim_lease_secs = i64::try_from(claim_lease_secs)
            .ok()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| PijError::Adapter {
                adapter: "store/queue".to_string(),
                message: "claim_lease_secs must be between 1 and i64::MAX".to_string(),
            })?;
        let delivered_id_capacity = i64::try_from(delivered_id_capacity)
            .ok()
            .filter(|capacity| *capacity > 0)
            .ok_or_else(|| PijError::Adapter {
                adapter: "store/queue".to_string(),
                message: "delivered_id_capacity must be between 1 and i64::MAX".to_string(),
            })?;
        Ok(SqliteQueue {
            pool,
            claim_lease_secs,
            delivered_id_capacity,
        })
    }

    /// How many rows are live (pending or running) — the dedupe assertion
    /// surface, and the number an operator actually wants when asking "is the
    /// queue backed up?".
    ///
    /// # Errors
    /// [`PijError::Adapter`] when the count cannot be read.
    pub async fn live_len(&self) -> Result<u64> {
        // A public read of the database is a public database operation. Review
        // found this one outside the guard while every other op had it, which is
        // how "every operation checks" quietly becomes "every operation I
        // remembered to check".
        require_current_schema(&self.pool).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE state IN ('pending', 'running')")
                .fetch_one(&self.pool)
                .await
                .map_err(adapter_error)?;
        Ok(count as u64)
    }
    async fn claim_inner(
        &self,
        kinds: &[String],
        worker: &str,
        extension_lease: Option<ExtensionLease>,
        at: u64,
        recovery_allowed: bool,
    ) -> Result<ExtensionClaim> {
        require_current_schema(&self.pool).await?;
        if kinds.is_empty() {
            return Ok(ExtensionClaim::default());
        }
        // The kind list is data, not SQL: `json_each` expands it, so a caller can
        // never widen the query by choosing a clever kind name.
        let kinds_json = serde_json::to_string(kinds).map_err(|error| PijError::Adapter {
            adapter: "store/queue".to_string(),
            message: format!("could not encode the claim filter: {error}"),
        })?;
        let lease_secs = match extension_lease {
            Some(ExtensionLease { seconds, .. }) => i64::try_from(seconds)
                .ok()
                .filter(|seconds| *seconds > 0)
                .ok_or_else(|| PijError::Adapter {
                    adapter: "store/queue".into(),
                    message: "extension claim lease must be between 1 and i64::MAX seconds".into(),
                })?,
            None => self.claim_lease_secs,
        };
        let mut result = ExtensionClaim::default();

        let pool = self.pool.clone();
        let claim_lease_secs = self.claim_lease_secs;
        let worker = worker.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        if let Some(lease) = extension_lease {
            if lease.renew_working {
                // A live busy recipient is a one-directional safety interlock:
                // age alone cannot revoke its body claim or spend its budget.
                sqlx::query(
                    "UPDATE jobs SET claimed_at = unixepoch() \
                     WHERE state = 'running' AND claimed_at <= unixepoch() - ?1 \
                       AND serial_key = ?3 AND kind = 'delivery:' || serial_key \
                       AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.command') END IS NULL \
                       AND kind IN (SELECT value FROM json_each(?2))",
                )
                .bind(lease_secs)
                .bind(&kinds_json)
                .bind(&worker)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            }
            let parked = sqlx::query(
                "UPDATE jobs SET state = 'failed', outcome = 'undelivered:lease-exhausted', \
                    lease_expirations = lease_expirations + 1, acked_at = unixepoch() \
                 WHERE state = 'running' AND claimed_at <= unixepoch() - ?1 \
                   AND lease_expirations >= 2 AND kind = 'delivery:' || serial_key \
                   AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.command') END IS NULL \
                   AND kind IN (SELECT value FROM json_each(?2)) \
                 RETURNING id, kind, serial_key, payload, dedupe_key, dedupe_origin, attempt",
            )
            .bind(lease_secs).bind(&kinds_json)
            .fetch_all(&mut *tx).await.map_err(adapter_error)?;
            for row in parked {
                result.parked.push(ParkedDelivery {
                    job_id: JobId(row.try_get::<i64, _>("id").map_err(adapter_error)? as u64),
                    job: decode_job(&row)?,
                    outcome: DeliveryFailure::LeaseExhausted,
                });
            }
            if !result.parked.is_empty() {
                pij_core::delivery::require_recovery_authority(recovery_allowed)?;
            }
            for parked in &result.parked {
                let evidence = pij_core::ports::ParkingEvidence {
                    outcome: parked.outcome,
                    reason: "three extension claim leases expired",
                    at,
                };
                for mut event in
                    pij_core::delivery::parked_events(parked.job_id, &parked.job, &evidence)?
                {
                    event.seq = Some(crate::spine::append_in_transaction(&mut tx, &event).await?);
                    result.events.push(event);
                }
            }
        }
        // Body delivery may retry a lost read acknowledgement. A control may
        // already have executed before its ACK was lost, so its expired claim
        // must instead fail with an explicitly unknown outcome. After the body
        // retry below, every remaining expired claim is terminal. Keep both
        // transitions scoped to the requested kinds and inside this transaction.
        sqlx::query(
            "UPDATE jobs \
             SET state = 'pending', worker = NULL, claimed_at = NULL, \
                 attempt = attempt + 1, lease_expirations = lease_expirations + ?3, \
                 not_before = unixepoch(), outcome = NULL \
             WHERE state = 'running' \
               AND claimed_at IS NOT NULL \
               AND claimed_at <= unixepoch() - ?1 \
               AND kind LIKE 'delivery:%' \
               AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.command') END IS NULL \
               AND kind IN (SELECT value FROM json_each(?2))",
        )
        .bind(lease_secs)
        .bind(&kinds_json)
        .bind(i64::from(extension_lease.is_some()))
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?;
        sqlx::query(
            "UPDATE jobs \
             SET state = 'failed', \
                 outcome = printf(CASE WHEN kind LIKE 'delivery:%' \
                                  THEN 'control claim lease expired after %d seconds for worker %s; outcome unknown: acknowledgement missing' \
                                  ELSE 'claim lease expired after %d seconds for worker %s' END, \
                                  ?1, COALESCE(worker, '<unknown>')), \
                 acked_at = unixepoch() \
             WHERE state = 'running' \
               AND claimed_at IS NOT NULL \
               AND claimed_at <= unixepoch() - ?1 \
               AND kind IN (SELECT value FROM json_each(?2))",
        )
        .bind(claim_lease_secs)
        .bind(&kinds_json)
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?;

        let row = sqlx::query(
            "SELECT id, kind, serial_key, payload, dedupe_key, dedupe_origin, attempt FROM jobs \
             WHERE state = 'pending' \
               AND not_before <= unixepoch() \
               AND kind IN (SELECT value FROM json_each(?1)) \
               AND serial_key NOT IN (SELECT serial_key FROM jobs WHERE state = 'running') \
             ORDER BY id LIMIT 1",
        )
        .bind(&kinds_json)
        .fetch_optional(&mut *tx)
        .await
        .map_err(adapter_error)?;

        let Some(row) = row else {
            tx.commit().await.map_err(adapter_error)?;
            return Ok(result);
        };

        let id: i64 = row.try_get("id").map_err(adapter_error)?;
        let job = decode_job(&row)?;

        // The UPDATE re-checks `state = 'pending'`, so two claimers racing on the
        // same row produce one winner and one `None` — never two workers holding
        // the same job.
        let claimed = sqlx::query(
            "UPDATE jobs \
             SET state = 'running', worker = ?2, claimed_at = unixepoch() \
             WHERE id = ?1 AND state = 'pending'",
        )
        .bind(id)
        .bind(&worker)
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?
        .rows_affected();

        tx.commit().await.map_err(adapter_error)?;

        if claimed != 0 {
            result.claimed = Some((JobId(id as u64), job));
        }
        Ok(result)
        })
        .await
    }
}

fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/queue".to_string(),
        message: error.to_string(),
    }
}

fn sql_ms(ms: u64) -> Result<i64> {
    i64::try_from(ms).map_err(|_| PijError::Adapter {
        adapter: "store/queue".into(),
        message: "timestamp exceeds SQLite's integer range".into(),
    })
}

fn decode_fyi(row: &sqlx::sqlite::SqliteRow, recipient: &SeatId) -> Result<pij_core::fyi::HeldFyi> {
    Ok(pij_core::fyi::HeldFyi {
        id: row.try_get("id").map_err(adapter_error)?,
        recipient: recipient.clone(),
        sender: SeatId(row.try_get("sender").map_err(adapter_error)?),
        // '' is a local FYI: the column is part of the primary key, so NOT NULL.
        from_machine: Some(row.try_get::<String, _>("origin").map_err(adapter_error)?)
            .filter(|origin| !origin.is_empty()),
        body: row.try_get("body").map_err(adapter_error)?,
        held_at_ms: row.try_get::<i64, _>("held_at_ms").map_err(adapter_error)? as u64,
    })
}

/// The origin column's '' is a local job or message (plan 164 review F02).
fn origin_column(origin: String) -> Option<String> {
    Some(origin).filter(|origin| !origin.is_empty())
}

fn decode_job(row: &sqlx::sqlite::SqliteRow) -> Result<Job> {
    Ok(Job {
        kind: row.try_get("kind").map_err(adapter_error)?,
        serial_key: row.try_get("serial_key").map_err(adapter_error)?,
        payload: row.try_get("payload").map_err(adapter_error)?,
        dedupe_key: row.try_get("dedupe_key").map_err(adapter_error)?,
        dedupe_origin: origin_column(row.try_get("dedupe_origin").map_err(adapter_error)?),
        attempt: row
            .try_get::<i64, _>("attempt")
            .map_err(adapter_error)?
            .max(0) as u32,
    })
}

impl SqliteQueue {
    /// One transaction: the carrier row, the FYI transition and its receipt
    /// commit together or not at all (plan 158 review HIGH-1). With
    /// `require_fyis` (a plan 159 flush), claiming nothing rolls back and
    /// returns `None`: the carrier exists only for what it carries.
    async fn carrying_enqueue(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        require_fyis: bool,
    ) -> Result<(Option<DeliveryEnqueue>, Vec<pij_core::model::Event>)> {
        require_current_schema(&self.pool).await?;
        require_delivery_job(&job)?;
        let pool = self.pool.clone();
        let via = via.to_owned();
        owned_write(async move {
            // One transaction: the carrier row, the FYI transition and its receipt
            // commit together or not at all (plan 158 review HIGH-1).
            let mut tx = begin_write(&pool).await?;
            let delivered_origin: Option<String> = sqlx::query_scalar(
                "SELECT origin FROM delivered_messages WHERE recipient = ?1 AND msg_id = ?2 \
                 AND sender_machine = ?3",
            )
            .bind(&job.serial_key)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            if let Some(origin) = delivered_origin {
                let origin = decode_origin(&origin)?;
                tx.commit().await.map_err(adapter_error)?;
                return Ok((Some(DeliveryEnqueue::AlreadyDelivered(origin)), Vec::new()));
            }
            // A retry of a message that is still queued keeps the queued body, so
            // it must not claim anything that body will never carry.
            let live: Option<(i64, i64)> = sqlx::query_as(
                "SELECT id, not_before FROM jobs WHERE kind = ?1 AND dedupe_key = ?2 \
               AND dedupe_origin = ?3 AND state IN ('pending', 'running') ORDER BY id LIMIT 1",
            )
            .bind(&job.kind)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            if let Some((id, not_before)) = live {
                tx.commit().await.map_err(adapter_error)?;
                return Ok((
                    Some(DeliveryEnqueue::Queued {
                        job_id: JobId(id as u64),
                        not_before_ms: (not_before.max(0) as u64).saturating_mul(1_000),
                    }),
                    Vec::new(),
                ));
            }
            let recipient = SeatId(job.serial_key.clone());
            let rows = sqlx::query(
                "UPDATE fyis SET state = 'delivered', settled_at_ms = ?2, settled_via = ?3 \
                 WHERE recipient = ?1 AND state = 'pending' \
                 RETURNING id, origin, sender, body, held_at_ms",
            )
            .bind(recipient.as_str())
            .bind(sql_ms(at)?)
            .bind(&via)
            .fetch_all(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let mut fyis = rows
                .iter()
                .map(|row| decode_fyi(row, &recipient))
                .collect::<Result<Vec<_>>>()?;
            fyis.sort_by(|a, b| a.held_at_ms.cmp(&b.held_at_ms).then(a.id.cmp(&b.id)));
            if require_fyis && fyis.is_empty() {
                // Dropping the transaction rolls it back: nothing to carry.
                return Ok((None, Vec::new()));
            }
            let payload = if fyis.is_empty() {
                job.payload.clone()
            } else {
                attach(&job.payload, &fyis)?
            };
            let (id, not_before): (i64, i64) = sqlx::query_as(
                "INSERT INTO jobs (kind, serial_key, payload, dedupe_key, dedupe_origin, enqueued_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, unixepoch()) RETURNING id, not_before",
            )
            .bind(&job.kind)
            .bind(&job.serial_key)
            .bind(&payload)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let mut events = Vec::new();
            if !fyis.is_empty() {
                let ids: Vec<String> = fyis.iter().map(|fyi| fyi.id.clone()).collect();
                let mut event = pij_core::fyi::delivered_event(&recipient, &ids, &via, at);
                event.seq = Some(crate::spine::append_in_transaction(&mut tx, &event).await?);
                events.push(event);
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok((
                Some(DeliveryEnqueue::Queued {
                    job_id: JobId(id as u64),
                    not_before_ms: (not_before.max(0) as u64).saturating_mul(1_000),
                }),
                events,
            ))
        })
        .await
    }
}

#[async_trait]
impl Queue for SqliteQueue {
    async fn enqueue(&self, job: Job) -> Result<JobId> {
        require_current_schema(&self.pool).await?;
        // `ON CONFLICT ... DO NOTHING` against the partial index, then read the
        // live row back. Both statements run in ONE transaction so a concurrent
        // enqueue cannot slip between them and leave this caller with no id.
        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;

            sqlx::query(
                "INSERT INTO jobs (kind, serial_key, payload, dedupe_key, dedupe_origin, enqueued_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch()) ON CONFLICT DO NOTHING",
            )
            .bind(&job.kind)
            .bind(&job.serial_key)
            .bind(&job.payload)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;

            let id: i64 = sqlx::query_scalar(
                // Scoped by KIND as well as key, matching the index: the read-back
                // must find the row this insert collapsed onto, and a global lookup
                // would hand back a live row for a DIFFERENT recipient that happens
                // to share a caller-chosen msg_id (review F5).
                "SELECT id FROM jobs WHERE kind = ?1 AND dedupe_key = ?2 \
               AND dedupe_origin = ?3 AND state IN ('pending', 'running') \
             ORDER BY id LIMIT 1",
            )
            .bind(&job.kind)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;

            tx.commit().await.map_err(adapter_error)?;
            Ok(JobId(id as u64))
        })
        .await
    }

    async fn enqueue_delivery(&self, job: Job) -> Result<DeliveryEnqueue> {
        require_current_schema(&self.pool).await?;
        require_delivery_job(&job)?;
        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;

            let delivered_origin: Option<String> = sqlx::query_scalar(
                "SELECT origin FROM delivered_messages WHERE recipient = ?1 AND msg_id = ?2 \
                 AND sender_machine = ?3",
            )
            .bind(&job.serial_key)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            if let Some(origin) = delivered_origin {
                let origin = decode_origin(&origin)?;
                tx.commit().await.map_err(adapter_error)?;
                return Ok(DeliveryEnqueue::AlreadyDelivered(origin));
            }

            sqlx::query(
                "INSERT INTO jobs (kind, serial_key, payload, dedupe_key, dedupe_origin, enqueued_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch()) ON CONFLICT DO NOTHING",
            )
            .bind(&job.kind)
            .bind(&job.serial_key)
            .bind(&job.payload)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;

            let (id, not_before): (i64, i64) = sqlx::query_as(
                "SELECT id, not_before FROM jobs WHERE kind = ?1 AND dedupe_key = ?2 \
               AND dedupe_origin = ?3 AND state IN ('pending', 'running') ORDER BY id LIMIT 1",
            )
            .bind(&job.kind)
            .bind(&job.dedupe_key)
            .bind(job.dedupe_origin.as_deref().unwrap_or_default())
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;

            tx.commit().await.map_err(adapter_error)?;
            Ok(DeliveryEnqueue::Queued {
                job_id: JobId(id as u64),
                not_before_ms: (not_before.max(0) as u64).saturating_mul(1_000),
            })
        })
        .await
    }

    async fn claim(&self, kinds: &[String], worker: &str) -> Result<Option<(JobId, Job)>> {
        Ok(self
            .claim_inner(kinds, worker, None, 0, true)
            .await?
            .claimed)
    }

    async fn claim_extension(
        &self,
        kinds: &[String],
        worker: &str,
        lease: ExtensionLease,
        at: u64,
        _spine: &dyn pij_core::ports::Spine,
        recovery_allowed: bool,
    ) -> Result<ExtensionClaim> {
        self.claim_inner(kinds, worker, Some(lease), at, recovery_allowed)
            .await
    }

    async fn peek_parked(&self, kinds: &[String]) -> Result<Vec<ParkedDelivery>> {
        require_current_schema(&self.pool).await?;
        let kinds = serde_json::to_string(kinds).map_err(|error| PijError::Adapter {
            adapter: "store/queue".into(),
            message: error.to_string(),
        })?;
        let rows = sqlx::query(
            "SELECT id, kind, serial_key, payload, dedupe_key, dedupe_origin, attempt, outcome FROM jobs \
             WHERE state = 'failed' AND kind IN (SELECT value FROM json_each(?1)) \
               AND outcome IN ('undelivered:lease-exhausted', 'undelivered:harness-swallowed', \
                               'undelivered:operator-released', 'undelivered:native-receiver-unavailable') ORDER BY id",
        )
        .bind(kinds)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter()
            .map(|row| {
                let outcome = match row.try_get::<&str, _>("outcome").map_err(adapter_error)? {
                    "undelivered:lease-exhausted" => DeliveryFailure::LeaseExhausted,
                    "undelivered:harness-swallowed" => DeliveryFailure::HarnessSwallowed,
                    "undelivered:native-receiver-unavailable" => {
                        DeliveryFailure::NativeReceiverUnavailable
                    }
                    _ => DeliveryFailure::OperatorReleased,
                };
                Ok(ParkedDelivery {
                    job_id: JobId(row.try_get::<i64, _>("id").map_err(adapter_error)? as u64),
                    job: decode_job(row)?,
                    outcome,
                })
            })
            .collect()
    }

    async fn recover_native_delivery(&self, job: JobId, recipient: &SeatId) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(false);
        };
        let pool = self.pool.clone();
        let recipient = recipient.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let changed = sqlx::query(
                "UPDATE jobs SET state = 'pending', worker = NULL, claimed_at = NULL, \
                 acked_at = NULL, outcome = NULL, attempt = attempt + 1, not_before = unixepoch() \
             WHERE id = ?1 AND serial_key = ?2 AND kind = 'delivery:' || serial_key \
               AND state = 'failed' AND outcome = 'undelivered:native-receiver-unavailable' \
               AND NOT EXISTS (SELECT 1 FROM jobs live WHERE live.id != jobs.id \
                   AND live.state IN ('pending', 'running') AND live.dedupe_key = jobs.dedupe_key \
                   AND live.dedupe_origin = jobs.dedupe_origin) \
               AND NOT EXISTS (SELECT 1 FROM delivered_messages \
                   WHERE recipient = jobs.serial_key AND msg_id = jobs.dedupe_key \
                   AND sender_machine = jobs.dedupe_origin)",
            )
            .bind(id)
            .bind(recipient.as_str())
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();
            tx.commit().await.map_err(adapter_error)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn park_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
        evidence: &pij_core::ports::ParkingEvidence<'_>,
        _spine: &dyn pij_core::ports::Spine,
    ) -> Result<(Option<Job>, Vec<pij_core::model::Event>)> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok((None, Vec::new()));
        };
        let pool = self.pool.clone();
        let claim_lease_secs = self.claim_lease_secs;
        let recipient = recipient.clone();
        let outcome = evidence.outcome;
        let reason = evidence.reason.to_owned();
        let at = evidence.at;
        owned_write(async move {
        let evidence = pij_core::ports::ParkingEvidence {
            outcome,
            reason: &reason,
            at,
        };
        let mut tx = begin_write(&pool).await?;
        let row = sqlx::query(
            "UPDATE jobs SET state = 'failed', outcome = ?4, acked_at = unixepoch() \
             WHERE id = ?1 AND serial_key = ?2 \
               AND (state = 'running' OR (state = 'pending' AND ?4 = 'undelivered:native-receiver-unavailable')) \
               AND kind = 'delivery:' || serial_key AND attempt = ?3 \
               AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.command') END IS NULL \
               AND (?4 != 'undelivered:native-receiver-unavailable' OR state != 'running' \
                   OR worker != 'native-cli:' || serial_key OR claimed_at <= unixepoch() - ?5) \
               AND NOT EXISTS (SELECT 1 FROM jobs earlier \
                   WHERE earlier.serial_key = jobs.serial_key AND earlier.id < jobs.id \
                     AND earlier.state IN ('pending', 'running')) \
             RETURNING kind, serial_key, payload, dedupe_key, dedupe_origin, attempt",
        ).bind(id).bind(recipient.as_str()).bind(i64::from(attempt)).bind(evidence.outcome.as_str())
            .bind(claim_lease_secs)
            .fetch_optional(&mut *tx).await.map_err(adapter_error)?;
        let job = row.as_ref().map(decode_job).transpose()?;
        let mut events = Vec::new();
        if let Some(parked) = &job {
            for mut event in pij_core::delivery::parked_events(JobId(id as u64), parked, &evidence)?
            {
                event.seq = Some(crate::spine::append_in_transaction(&mut tx, &event).await?);
                events.push(event);
            }
        }
        tx.commit().await.map_err(adapter_error)?;
        Ok((job, events))
        })
        .await
    }

    async fn peek(&self, kinds: &[String]) -> Result<Option<(JobId, Job)>> {
        require_current_schema(&self.pool).await?;
        if kinds.is_empty() {
            return Ok(None);
        }
        let kinds_json = serde_json::to_string(kinds).map_err(|error| PijError::Adapter {
            adapter: "store/queue".to_string(),
            message: format!("could not encode the peek filter: {error}"),
        })?;
        let row = sqlx::query(
            "SELECT id, kind, serial_key, payload, dedupe_key, dedupe_origin, attempt FROM jobs \
             WHERE state IN ('pending', 'running') \
               AND kind IN (SELECT value FROM json_each(?1)) \
             ORDER BY id LIMIT 1",
        )
        .bind(&kinds_json)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id = row.try_get::<i64, _>("id").map_err(adapter_error)?;
        let job = Job {
            kind: row.try_get("kind").map_err(adapter_error)?,
            serial_key: row.try_get("serial_key").map_err(adapter_error)?,
            payload: row.try_get("payload").map_err(adapter_error)?,
            dedupe_key: row.try_get("dedupe_key").map_err(adapter_error)?,
            dedupe_origin: origin_column(row.try_get("dedupe_origin").map_err(adapter_error)?),
            attempt: row
                .try_get::<i64, _>("attempt")
                .map_err(adapter_error)?
                .max(0) as u32,
        };
        Ok(Some((JobId(id as u64), job)))
    }

    async fn claimed_delivery(&self, job: JobId) -> Result<Option<Job>> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(None);
        };
        let row = sqlx::query(
            "SELECT kind, serial_key, payload, dedupe_key, dedupe_origin, attempt FROM jobs \
             WHERE id = ?1 AND state = 'running' AND kind = 'delivery:' || serial_key",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(Job {
            kind: row.try_get("kind").map_err(adapter_error)?,
            serial_key: row.try_get("serial_key").map_err(adapter_error)?,
            payload: row.try_get("payload").map_err(adapter_error)?,
            dedupe_key: row.try_get("dedupe_key").map_err(adapter_error)?,
            dedupe_origin: origin_column(row.try_get("dedupe_origin").map_err(adapter_error)?),
            attempt: row
                .try_get::<i64, _>("attempt")
                .map_err(adapter_error)?
                .max(0) as u32,
        }))
    }

    async fn terminal_delivery_state(
        &self,
        job: JobId,
        recipient: &SeatId,
    ) -> Result<Option<&'static str>> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(None);
        };
        let row = sqlx::query(
            "SELECT state FROM jobs WHERE id = ?1 AND serial_key = ?2 \
             AND state IN ('done', 'failed') AND kind = 'delivery:' || serial_key \
             AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.to') END = ?2 \
             AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.command') END IS NULL",
        )
        .bind(id)
        .bind(recipient.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(
            match row.try_get::<&str, _>("state").map_err(adapter_error)? {
                "done" => Some("done"),
                "failed" => Some("failed"),
                _ => None,
            },
        )
    }

    async fn heartbeat_delivery(
        &self,
        job: JobId,
        recipient: &SeatId,
        attempt: u32,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(false);
        };
        let changed = sqlx::query(
            "UPDATE jobs SET claimed_at = unixepoch() \
             WHERE id = ?1 AND state = 'running' AND serial_key = ?2 AND attempt = ?3 \
               AND kind = 'delivery:' || serial_key \
               AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.to') END = ?2 \
               AND CASE WHEN json_valid(payload) THEN json_extract(payload, '$.command') END IS NULL",
        )
        .bind(id)
        .bind(recipient.as_str())
        .bind(i64::from(attempt))
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?
        .rows_affected();
        Ok(changed != 0)
    }

    /// R4-AMEND-1: pending again, counted, and not before `delay` has passed —
    /// all in ONE transaction, because the whole point is that no crash window
    /// exists between "this attempt failed" and "it will be tried again".
    async fn retry(&self, job: JobId, delay: Duration) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let seconds = i64::try_from(delay.as_secs()).unwrap_or(i64::MAX);

        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let affected = sqlx::query(
                "UPDATE jobs \
             SET state = 'pending', worker = NULL, claimed_at = NULL, \
                 attempt = attempt + 1, not_before = unixepoch() + ?2 \
             WHERE id = ?1 AND state = 'running'",
            )
            .bind(job.0 as i64)
            .bind(seconds)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();

            tx.commit().await.map_err(adapter_error)?;
            if affected == 0 {
                // Same reasoning as `ack`: retrying a job nobody claimed means a
                // worker believes it holds work the queue does not think is running,
                // which is exactly what a lost claim leaves behind. Reported, never
                // silently ignored — a retry that quietly does nothing is a message
                // that stops moving and says so to no one.
                return Err(PijError::Adapter {
                    adapter: "store/queue".to_string(),
                    message: format!("job {} is not running, so it cannot be retried", job.0),
                });
            }
            Ok(())
        })
        .await
    }

    async fn record_delivery_deferral(
        &self,
        job: JobId,
        reason: &str,
        draft_sha: Option<&str>,
        at: u64,
        _spine: &dyn pij_core::ports::Spine,
    ) -> Result<Vec<pij_core::model::Event>> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(Vec::new());
        };
        let at = i64::try_from(at).map_err(|_| PijError::Adapter {
            adapter: "store/queue".into(),
            message: "deferral timestamp exceeds SQLite's integer range".into(),
        })?;
        let pool = self.pool.clone();
        let reason = reason.to_owned();
        let draft_sha = draft_sha.map(str::to_owned);
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(
                "SELECT serial_key, dedupe_key, deferral_reason, deferral_event_at_ms, deferral_reason_changes \
             FROM jobs WHERE id = ?1 AND state IN ('pending', 'running') \
               AND kind = 'delivery:' || serial_key",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let Some(row) = row else {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(Vec::new());
            };
            let previous: Option<String> = row.try_get("deferral_reason").map_err(adapter_error)?;
            let last_event: Option<i64> =
                row.try_get("deferral_event_at_ms").map_err(adapter_error)?;
            let reason_changes = row
                .try_get::<i64, _>("deferral_reason_changes")
                .map_err(adapter_error)?
                + i64::from(previous.as_deref().is_some_and(|previous| previous != reason));
            let publish = last_event.is_none_or(|last| {
                at.saturating_sub(last) >= pij_core::delivery::DEFERRAL_EVENT_INTERVAL_MS as i64
            });
            let updated = sqlx::query(
                "UPDATE jobs SET deferral_reason = ?2, deferral_count = deferral_count + 1, \
                 deferral_since_ms = COALESCE(deferral_since_ms, ?3), \
                 deferral_event_at_ms = CASE WHEN ?4 THEN ?3 ELSE deferral_event_at_ms END, \
                 deferral_reason_changes = CASE WHEN ?4 THEN 0 ELSE ?5 END \
             WHERE id = ?1 RETURNING deferral_count, deferral_since_ms",
            )
            .bind(id)
            .bind(&reason)
            .bind(at)
            .bind(publish)
            .bind(reason_changes)
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let mut events = Vec::new();
            if publish {
                let deferral = pij_core::model::DeliveryDeferral {
                    job_id: job,
                    msg_id: row.try_get("dedupe_key").map_err(adapter_error)?,
                    reason,
                    count: updated
                        .try_get::<i64, _>("deferral_count")
                        .map_err(adapter_error)? as u64,
                    since_ms: updated
                        .try_get::<i64, _>("deferral_since_ms")
                        .map_err(adapter_error)? as u64,
                };
                let seat = SeatId(row.try_get("serial_key").map_err(adapter_error)?);
                let mut event = pij_core::delivery::delivery_deferral_event(
                    &deferral,
                    &seat,
                    draft_sha.as_deref(),
                    at as u64,
                    reason_changes as u64,
                );
                event.seq = Some(crate::spine::append_in_transaction(&mut tx, &event).await?);
                events.push(event);
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok(events)
        })
        .await
    }

    async fn delivery_deferrals(
        &self,
        recipient: &SeatId,
    ) -> Result<Vec<pij_core::model::DeliveryDeferral>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT id, dedupe_key, deferral_reason, deferral_count, deferral_since_ms \
             FROM jobs WHERE serial_key = ?1 AND kind = 'delivery:' || serial_key \
               AND state IN ('pending', 'running') AND deferral_reason IS NOT NULL ORDER BY id",
        )
        .bind(recipient.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter()
            .map(|row| {
                Ok(pij_core::model::DeliveryDeferral {
                    job_id: JobId(row.try_get::<i64, _>("id").map_err(adapter_error)? as u64),
                    msg_id: row.try_get("dedupe_key").map_err(adapter_error)?,
                    reason: row.try_get("deferral_reason").map_err(adapter_error)?,
                    count: row
                        .try_get::<i64, _>("deferral_count")
                        .map_err(adapter_error)? as u64,
                    since_ms: row
                        .try_get::<i64, _>("deferral_since_ms")
                        .map_err(adapter_error)? as u64,
                })
            })
            .collect()
    }

    async fn defer(&self, job: JobId, delay: Duration) -> Result<DeferOutcome> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(DeferOutcome::NotLive {
                reason: DeferNoopReason::Absent,
            });
        };
        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(
                "SELECT kind, serial_key, dedupe_key, dedupe_origin, state FROM jobs WHERE id = ?1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let Some(row) = row else {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(DeferOutcome::NotLive {
                    reason: DeferNoopReason::Absent,
                });
            };
            let state: String = row.try_get("state").map_err(adapter_error)?;
            if !matches!(state.as_str(), "pending" | "running") {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(DeferOutcome::NotLive {
                    reason: DeferNoopReason::Terminal,
                });
            }
            let delivery = Job {
                kind: row.try_get("kind").map_err(adapter_error)?,
                serial_key: row.try_get("serial_key").map_err(adapter_error)?,
                payload: String::new(),
                dedupe_key: row.try_get("dedupe_key").map_err(adapter_error)?,
                dedupe_origin: origin_column(row.try_get("dedupe_origin").map_err(adapter_error)?),
                attempt: 0,
            };
            require_delivery_job(&delivery)?;

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| PijError::Adapter {
                    adapter: "store/queue".to_string(),
                    message: format!("cannot schedule deferral before the Unix epoch: {error}"),
                })?;
            // Eligibility compares whole epoch seconds. Round the absolute positive
            // deadline UP, not just the delay: truncating now can release even a
            // whole-second delay early. Zero must remain immediately eligible.
            let not_before = now
                .checked_add(delay)
                .and_then(|deadline| {
                    deadline
                        .as_secs()
                        .checked_add(u64::from(!delay.is_zero() && deadline.subsec_nanos() != 0))
                })
                .and_then(|seconds| i64::try_from(seconds).ok())
                .ok_or_else(|| PijError::Adapter {
                    adapter: "store/queue".to_string(),
                    message: "deferral deadline exceeds SQLite's integer timestamp range"
                        .to_string(),
                })?;
            sqlx::query(
                "UPDATE jobs \
             SET state = 'pending', worker = NULL, claimed_at = NULL, not_before = ?2 \
             WHERE id = ?1 AND state IN ('pending', 'running')",
            )
            .bind(id)
            .bind(not_before)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(DeferOutcome::Deferred {
                recipient: SeatId(delivery.serial_key),
                msg_id: delivery.dedupe_key,
            })
        })
        .await
    }

    async fn release_deferred(&self, job: JobId) -> Result<ReleaseOutcome> {
        require_current_schema(&self.pool).await?;
        let Ok(id) = i64::try_from(job.0) else {
            return Ok(ReleaseOutcome::NotLive {
                reason: DeferNoopReason::Absent,
            });
        };
        // Hold the write lock across classification and release: a reader must
        // not claim the row between our state check and the deadline update.
        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(
                "SELECT kind, serial_key, dedupe_key, dedupe_origin, state FROM jobs WHERE id = ?1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let Some(row) = row else {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(ReleaseOutcome::NotLive {
                    reason: DeferNoopReason::Absent,
                });
            };
            let state: String = row.try_get("state").map_err(adapter_error)?;
            if !matches!(state.as_str(), "pending" | "running") {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(ReleaseOutcome::NotLive {
                    reason: DeferNoopReason::Terminal,
                });
            }
            let delivery = Job {
                kind: row.try_get("kind").map_err(adapter_error)?,
                serial_key: row.try_get("serial_key").map_err(adapter_error)?,
                payload: String::new(),
                dedupe_key: row.try_get("dedupe_key").map_err(adapter_error)?,
                dedupe_origin: origin_column(row.try_get("dedupe_origin").map_err(adapter_error)?),
                attempt: 0,
            };
            require_delivery_job(&delivery)?;
            if state == "running" {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(ReleaseOutcome::NotDeferred);
            }
            sqlx::query(
                "UPDATE jobs SET not_before = unixepoch() WHERE id = ?1 AND state = 'pending'",
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(ReleaseOutcome::Released {
                recipient: SeatId(delivery.serial_key),
                msg_id: delivery.dedupe_key,
            })
        })
        .await
    }

    async fn hold_fyi(
        &self,
        fyi: &pij_core::fyi::HeldFyi,
        _spine: &dyn pij_core::ports::Spine,
    ) -> Result<Vec<pij_core::model::Event>> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let fyi = fyi.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            // A retried send with the same msg_id FROM THE SAME MACHINE is the
            // same FYI, held once; another machine's msg_id is another FYI.
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO fyis (origin, id, recipient, sender, body, held_at_ms, state) \
                 VALUES (?6, ?1, ?2, ?3, ?4, ?5, 'pending')",
            )
            .bind(&fyi.id)
            .bind(fyi.recipient.as_str())
            .bind(fyi.sender.as_str())
            .bind(&fyi.body)
            .bind(sql_ms(fyi.held_at_ms)?)
            .bind(fyi.from_machine.as_deref().unwrap_or_default())
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();
            let mut events = Vec::new();
            if inserted == 1 {
                let mut event = pij_core::fyi::held_event(&fyi);
                event.seq = Some(crate::spine::append_in_transaction(&mut tx, &event).await?);
                events.push(event);
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok(events)
        })
        .await
    }

    async fn claim_fyis(
        &self,
        recipient: &SeatId,
        via: &str,
        at: u64,
        _spine: &dyn pij_core::ports::Spine,
    ) -> Result<(Vec<pij_core::fyi::HeldFyi>, Vec<pij_core::model::Event>)> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let recipient = recipient.clone();
        let via = via.to_owned();
        owned_write(async move {
            // One write transaction: select, transition and receipt together, so
            // a racing claimer sees these rows already delivered.
            let mut tx = begin_write(&pool).await?;
            let rows = sqlx::query(
                "UPDATE fyis SET state = 'delivered', settled_at_ms = ?2, settled_via = ?3 \
                 WHERE recipient = ?1 AND state = 'pending' \
                 RETURNING id, origin, sender, body, held_at_ms",
            )
            .bind(recipient.as_str())
            .bind(sql_ms(at)?)
            .bind(&via)
            .fetch_all(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let mut fyis = rows
                .iter()
                .map(|row| decode_fyi(row, &recipient))
                .collect::<Result<Vec<_>>>()?;
            fyis.sort_by(|a, b| a.held_at_ms.cmp(&b.held_at_ms).then(a.id.cmp(&b.id)));
            let mut events = Vec::new();
            if !fyis.is_empty() {
                let ids: Vec<String> = fyis.iter().map(|fyi| fyi.id.clone()).collect();
                let mut event = pij_core::fyi::delivered_event(&recipient, &ids, &via, at);
                event.seq = Some(crate::spine::append_in_transaction(&mut tx, &event).await?);
                events.push(event);
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok((fyis, events))
        })
        .await
    }

    async fn enqueue_delivery_carrying_fyis(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        _spine: &dyn pij_core::ports::Spine,
    ) -> Result<(DeliveryEnqueue, Vec<pij_core::model::Event>)> {
        match self.carrying_enqueue(job, via, at, attach, false).await? {
            (Some(enqueued), events) => Ok((enqueued, events)),
            (None, _) => Err(PijError::Adapter {
                adapter: "store/queue".to_string(),
                message: "an optional carrier always enqueues".to_string(),
            }),
        }
    }

    async fn enqueue_fyi_flush(
        &self,
        job: Job,
        via: &str,
        at: u64,
        attach: pij_core::ports::AttachFyis,
        _spine: &dyn pij_core::ports::Spine,
    ) -> Result<(Option<DeliveryEnqueue>, Vec<pij_core::model::Event>)> {
        self.carrying_enqueue(job, via, at, attach, true).await
    }

    async fn pending_fyi_count(&self, recipient: &SeatId) -> Result<u64> {
        require_current_schema(&self.pool).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fyis WHERE recipient = ?1 AND state = 'pending'",
        )
        .bind(recipient.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(count as u64)
    }

    async fn read_claimed_fyis(
        &self,
        recipient: &SeatId,
        claimed_at_ms: u64,
    ) -> Result<Vec<pij_core::fyi::HeldFyi>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT id, origin, sender, body, held_at_ms FROM fyis \
             WHERE recipient = ?1 AND state = 'delivered' AND settled_at_ms = ?2 \
             ORDER BY held_at_ms, id",
        )
        .bind(recipient.as_str())
        .bind(sql_ms(claimed_at_ms)?)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter().map(|row| decode_fyi(row, recipient)).collect()
    }

    async fn ack(&self, job: JobId, outcome: Outcome) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let (state, detail) = match outcome {
            Outcome::Done => ("done", None),
            Outcome::Failed { reason } => ("failed", Some(reason)),
        };

        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let affected = sqlx::query(
                "UPDATE jobs \
             SET state = ?2, outcome = ?3, acked_at = unixepoch() \
             WHERE id = ?1 AND state = 'running'",
            )
            .bind(job.0 as i64)
            .bind(state)
            .bind(detail)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();

            tx.commit().await.map_err(adapter_error)?;
            if affected == 0 {
                // Acking a job nobody claimed is a fact worth reporting: it means a
                // worker believes it holds work the queue does not think is running,
                // which is exactly the state a lost claim leaves behind.
                return Err(PijError::Adapter {
                    adapter: "store/queue".to_string(),
                    message: format!(
                        "job {} is not running — it was already acked, or never claimed",
                        job.0
                    ),
                });
            }
            Ok(())
        })
        .await
    }

    /// R4-AMEND-4: claim-or-report, in ONE transaction. See the port docs for why
    /// the caller must compensate on a synchronous injection failure.
    async fn note_delivered(
        &self,
        recipient: &SeatId,
        msg_id: &str,
        sender_machine: Option<&str>,
        origin: DeliveryOrigin,
    ) -> Result<Option<DeliveryOrigin>> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let delivered_id_capacity = self.delivered_id_capacity;
        let recipient = recipient.clone();
        let msg_id = msg_id.to_owned();
        let sender_machine = sender_machine.unwrap_or_default().to_owned();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;

            let existing: Option<String> = sqlx::query_scalar(
                "SELECT origin FROM delivered_messages \
                 WHERE recipient = ?1 AND msg_id = ?2 AND sender_machine = ?3",
            )
            .bind(recipient.as_str())
            .bind(&msg_id)
            .bind(&sender_machine)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            if let Some(existing) = existing {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(Some(decode_origin(&existing)?));
            }

            sqlx::query(
                "INSERT INTO delivered_messages (recipient, msg_id, origin, sender_machine) \
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(recipient.as_str())
            .bind(&msg_id)
            .bind(encode_origin(origin))
            .bind(&sender_machine)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            sqlx::query(
                "DELETE FROM delivered_messages WHERE recipient = ?1 AND seq NOT IN (\
                 SELECT seq FROM delivered_messages WHERE recipient = ?1 \
                 ORDER BY seq DESC LIMIT ?2\
             )",
            )
            .bind(recipient.as_str())
            .bind(delivered_id_capacity)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;

            tx.commit().await.map_err(adapter_error)?;
            Ok(None)
        })
        .await
    }

    async fn admitted(
        &self,
        recipient: &SeatId,
        msg_id: &str,
        sender_machine: Option<&str>,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM delivered_messages \
               WHERE recipient = ?1 AND msg_id = ?2 AND sender_machine = ?3 \
             UNION ALL \
             SELECT 1 FROM jobs WHERE kind = 'delivery:' || ?1 AND dedupe_key = ?2 \
               AND dedupe_origin = ?3 \
             LIMIT 1",
        )
        .bind(recipient.as_str())
        .bind(msg_id)
        .bind(sender_machine.unwrap_or_default())
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(found.is_some())
    }

    async fn forget_delivered(
        &self,
        recipient: &SeatId,
        msg_id: &str,
        sender_machine: Option<&str>,
    ) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let recipient = recipient.clone();
        let msg_id = msg_id.to_owned();
        let sender_machine = sender_machine.unwrap_or_default().to_owned();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query(
                "DELETE FROM delivered_messages \
                 WHERE recipient = ?1 AND msg_id = ?2 AND sender_machine = ?3",
            )
            .bind(recipient.as_str())
            .bind(msg_id)
            .bind(sender_machine)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(())
        })
        .await
    }

    async fn ack_delivery(&self, job: JobId, origin: DeliveryOrigin) -> Result<DeliveryAck> {
        require_current_schema(&self.pool).await?;

        let pool = self.pool.clone();
        let delivered_id_capacity = self.delivered_id_capacity;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(
                "SELECT kind, serial_key, dedupe_key, dedupe_origin FROM jobs WHERE id = ?1 AND state = 'running'",
            )
            .bind(job.0 as i64)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .ok_or_else(|| PijError::Adapter {
                adapter: "store/queue".to_string(),
                message: format!(
                    "job {} is not running — it was already acked, or never claimed",
                    job.0
                ),
            })?;
            let claimed = Job {
                kind: row.try_get("kind").map_err(adapter_error)?,
                serial_key: row.try_get("serial_key").map_err(adapter_error)?,
                payload: String::new(),
                dedupe_key: row.try_get("dedupe_key").map_err(adapter_error)?,
                dedupe_origin: origin_column(row.try_get("dedupe_origin").map_err(adapter_error)?),
                attempt: 0,
            };
            require_delivery_job(&claimed)?;

            sqlx::query(
                "UPDATE jobs SET state = 'done', outcome = NULL, acked_at = unixepoch() \
             WHERE id = ?1 AND state = 'running'",
            )
            .bind(job.0 as i64)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            sqlx::query(
                "INSERT INTO delivered_messages (recipient, msg_id, origin, sender_machine) \
                 VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (recipient, sender_machine, msg_id) DO NOTHING",
            )
            .bind(&claimed.serial_key)
            .bind(&claimed.dedupe_key)
            .bind(encode_origin(origin))
            .bind(claimed.dedupe_origin.as_deref().unwrap_or_default())
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            sqlx::query(
                "DELETE FROM delivered_messages WHERE recipient = ?1 AND seq NOT IN (\
                 SELECT seq FROM delivered_messages WHERE recipient = ?1 \
                 ORDER BY seq DESC LIMIT ?2\
             )",
            )
            .bind(&claimed.serial_key)
            .bind(delivered_id_capacity)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;

            tx.commit().await.map_err(adapter_error)?;
            Ok(DeliveryAck {
                recipient: SeatId(claimed.serial_key),
                msg_id: claimed.dedupe_key,
                origin,
            })
        })
        .await
    }
}

fn require_delivery_job(job: &Job) -> Result<()> {
    let expected = format!("delivery:{}", job.serial_key);
    if job.kind == expected {
        return Ok(());
    }
    Err(PijError::Adapter {
        adapter: "store/queue".to_string(),
        message: format!(
            "delivery operation requires kind {expected}, got {}",
            job.kind
        ),
    })
}

const fn encode_origin(origin: DeliveryOrigin) -> &'static str {
    match origin {
        DeliveryOrigin::TypedToPane => "typed-to-pane",
        DeliveryOrigin::InjectedToTransport => "injected-to-transport",
        DeliveryOrigin::VerifiedArrival => "verified-arrival",
        DeliveryOrigin::ReaderRead => "reader-read",
    }
}

fn decode_origin(origin: &str) -> Result<DeliveryOrigin> {
    match origin {
        "typed-to-pane" => Ok(DeliveryOrigin::TypedToPane),
        "injected-to-transport" => Ok(DeliveryOrigin::InjectedToTransport),
        "verified-arrival" => Ok(DeliveryOrigin::VerifiedArrival),
        "reader-read" => Ok(DeliveryOrigin::ReaderRead),
        other => Err(PijError::Adapter {
            adapter: "store/queue".to_string(),
            message: format!("delivered message has unknown origin {other:?}"),
        }),
    }
}
