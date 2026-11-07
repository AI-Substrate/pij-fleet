//! Requests sent to a role (plan 199, migration 0034; limits: plan 205, 0035).
//!
//! Every `pij send role:<id>` is one row, keyed by the send's msg_id. A request
//! no live holder could take waits as `queued` until an assign drains it; the
//! rest are recorded `delivered` with their holder, so s06 can find who asked
//! and how they want to hear back.

use pij_core::error::{PijError, Result};
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error, sql_i64, sql_u64};

/// How the requester wants to hear that its request was answered (req-08).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyMode {
    /// Wake the requester (the default).
    Wake,
    /// Hold the answer for the requester's next real turn.
    Fyi,
    /// Do not tell the requester.
    None,
}

impl ReplyMode {
    /// The stored and printed spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wake => "wake",
            Self::Fyi => "fyi",
            Self::None => "none",
        }
    }

    /// Parse `wake`, `fyi` or `none`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "wake" => Some(Self::Wake),
            "fyi" => Some(Self::Fyi),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// Where a request is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    /// No live holder took it yet.
    Queued,
    /// Claimed for `holder` and being dispatched: no other drain or send may
    /// dispatch it.
    Delivering,
    /// Delivered to `holder`.
    Delivered,
}

impl RequestState {
    /// The stored and printed spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivering => "delivering",
            Self::Delivered => "delivered",
        }
    }
}

/// Which time-limit notification a claim is for (plan 205, s12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitStage {
    /// The holder's one check-in; stamps `checkin_at`.
    CheckIn,
    /// The PA's (else the prime's) one notice; stamps `escalated_at`.
    Notice,
}

/// How long a time-limit claim blocks another attempt. Admission takes
/// seconds and a failed one drops its claim at once, so only a daemon that
/// stopped mid-attempt leaves one this old; the next round then retries.
pub const LIMIT_CLAIM_LEASE_MS: u64 = 2 * 60_000;

/// A time-limit claim just taken (plan 205, s12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitClaim {
    /// The seat a stopped attempt resolved and pinned before sending, when
    /// the claim was taken over after its lease. `None` for a fresh claim.
    pub pinned: Option<String>,
}

/// One request sent to a role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleRequest {
    /// The send's msg_id: the request id (ruling C3).
    pub id: String,
    /// The project whose directory the role belongs to.
    pub project: String,
    /// The role id.
    pub role: String,
    /// The instance addressed, or empty for any holder of the role.
    pub instance: String,
    /// The sending seat.
    pub requester: String,
    /// The message body.
    pub body: String,
    /// The message this one answers, when it is a reply.
    pub in_reply_to: Option<String>,
    /// Sent with `--fyi`: held for the holder's next real turn.
    pub fyi: bool,
    /// How the requester wants to hear back.
    pub reply: ReplyMode,
    /// The time limit in milliseconds, when one applies (s12).
    pub limit_ms: Option<u64>,
    /// Queued or delivered.
    pub state: RequestState,
    /// The seat it was delivered to.
    pub holder: Option<String>,
    /// When it was sent, ms since the epoch.
    pub created_at: u64,
    /// When it reached its holder.
    pub delivered_at: Option<u64>,
    /// When its holder was sent the one time-limit check-in (s12). Set once,
    /// by the watchdog round, before the check-in is sent.
    pub checkin_at: Option<u64>,
    /// When the PA (else the prime) was told the check-in went unanswered
    /// (s12). Set once, before the notice is sent; the request then leaves the
    /// round's open set.
    pub escalated_at: Option<u64>,
    /// When an output first answered it (s06, `pij output --answers <id>`),
    /// through [`SqliteOrchestration::mark_role_request_answered`]. The first
    /// answer's time is kept. An answered request leaves the round's open set,
    /// so s12 never checks it in or escalates it.
    pub answered_at: Option<u64>,
}

/// Boot recovery's evidence that delivery already took a stranded claim:
/// a ledger row, a `delivery:<holder>` job in any state, or a held FYI under
/// its msg_id. Every probe is an indexed SEARCH (performance P001 on #129):
/// the ledger's unique key, `jobs_by_dedupe` and the FYI primary key.
const RECOVER_TAKEN_SQL: &str = "UPDATE role_requests SET state = 'delivered', delivered_at = ?1 \
     WHERE state = 'delivering' AND holder IS NOT NULL AND ( \
       EXISTS (SELECT 1 FROM delivered_messages AS ledger \
         WHERE ledger.recipient = role_requests.holder AND ledger.sender_machine = '' \
         AND ledger.msg_id = role_requests.id) \
       OR EXISTS (SELECT 1 FROM jobs \
         WHERE jobs.kind = 'delivery:' || role_requests.holder \
         AND jobs.dedupe_origin = '' AND jobs.dedupe_key = role_requests.id) \
       OR EXISTS (SELECT 1 FROM fyis \
         WHERE fyis.origin = '' AND fyis.id = role_requests.id \
         AND fyis.recipient = role_requests.holder))";

/// Has delivery taken local message `?2` for recipient `?1`? The evidence of
/// [`RECOVER_TAKEN_SQL`], for one recipient and msg_id (review of #133).
const ADMITTED_SQL: &str = "SELECT \
       EXISTS (SELECT 1 FROM delivered_messages AS ledger \
         WHERE ledger.recipient = ?1 AND ledger.sender_machine = '' AND ledger.msg_id = ?2) \
       OR EXISTS (SELECT 1 FROM jobs \
         WHERE jobs.kind = 'delivery:' || ?1 AND jobs.dedupe_origin = '' AND jobs.dedupe_key = ?2) \
       OR EXISTS (SELECT 1 FROM fyis \
         WHERE fyis.origin = '' AND fyis.id = ?2 AND fyis.recipient = ?1)";

/// When local message `?2` reached recipient `?1`'s session, in ms (plan 214,
/// F73): its delivery ledger row; else its acknowledged delivery job (the
/// ledger keeps only each recipient's latest rows; `outcome IS NULL` is a
/// real `ack_delivery`, not a refusal acked done); else its FYI, once
/// delivered. NULL while it is still held. Each probe is an indexed SEARCH.
const REACHED_SQL: &str = "SELECT COALESCE( \
       (SELECT ledger.delivered_at * 1000 FROM delivered_messages AS ledger \
         WHERE ledger.recipient = ?1 AND ledger.sender_machine = '' AND ledger.msg_id = ?2), \
       (SELECT max(jobs.acked_at) * 1000 FROM jobs \
         WHERE jobs.kind = 'delivery:' || ?1 AND jobs.dedupe_origin = '' AND jobs.dedupe_key = ?2 \
         AND jobs.state = 'done' AND jobs.outcome IS NULL), \
       (SELECT fyis.settled_at_ms FROM fyis \
         WHERE fyis.origin = '' AND fyis.id = ?2 AND fyis.recipient = ?1 \
         AND fyis.state = 'delivered'))";

const COLUMNS: &str = "id, project, role, instance, requester, body, in_reply_to, fyi, reply, \
     limit_ms, state, holder, created_at, delivered_at, checkin_at, escalated_at, answered_at";

impl SqliteOrchestration {
    /// Record a request, queued or already delivered.
    ///
    /// # Errors
    /// `E-RS-ROLE-REQUEST-EXISTS` when a request with this id is recorded.
    pub async fn record_role_request(&self, request: &RoleRequest) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let request = request.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = sqlx::query(&format!(
                "INSERT INTO role_requests ({COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL, NULL, NULL) \
                 ON CONFLICT (id) DO NOTHING"
            ))
            .bind(&request.id)
            .bind(&request.project)
            .bind(&request.role)
            .bind(&request.instance)
            .bind(&request.requester)
            .bind(&request.body)
            .bind(&request.in_reply_to)
            .bind(i64::from(request.fyi))
            .bind(request.reply.as_str())
            .bind(
                request
                    .limit_ms
                    .map(|limit| sql_i64(limit, "limit_ms"))
                    .transpose()?,
            )
            .bind(request.state.as_str())
            .bind(&request.holder)
            .bind(sql_i64(request.created_at, "created_at")?)
            .bind(
                request
                    .delivered_at
                    .map(|at| sql_i64(at, "delivered_at"))
                    .transpose()?,
            )
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected()
                == 1;
            if !inserted {
                tx.rollback().await.map_err(adapter_error)?;
                return Err(PijError::GovernanceRefused {
                    code: "E-RS-ROLE-REQUEST-EXISTS".to_string(),
                    record: request.id,
                });
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// One request by id.
    pub async fn role_request(&self, id: &str) -> Result<Option<RoleRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM role_requests WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?
        .as_ref()
        .map(role_request)
        .transpose()
    }

    /// `role`'s queued requests in `project` that a holder of `instance` can
    /// take, oldest first: those for that instance and those for any holder.
    pub async fn queued_role_requests(
        &self,
        project: &str,
        role: &str,
        instance: &str,
    ) -> Result<Vec<RoleRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM role_requests INDEXED BY role_requests_queued \
             WHERE project = ?1 AND role = ?2 AND state = 'queued' \
             AND (instance = ?3 OR instance = '') ORDER BY created_at, id"
        ))
        .bind(project)
        .bind(role)
        .bind(instance)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(role_request)
        .collect()
    }

    /// Every request not yet settled, queued or stuck `delivering`, oldest
    /// first: the PA digest's list (review of #129, F003). Bodies are not
    /// read (blank here); the digest never shows them. Two partial-index reads.
    pub async fn unsettled_role_requests(&self) -> Result<Vec<RoleRequest>> {
        self.unsettled_role_requests_in(None).await
    }

    /// [`Self::unsettled_role_requests`] for `projects` only (plan 209, #129's
    /// digest scope); `None` reads every project. Queued rows are one seek of
    /// `role_requests_queued` per project, never a scan of the others;
    /// `delivering` rows (normally none) are read from their own index and
    /// kept by project. An empty list reads nothing. Bodies are not read.
    pub async fn unsettled_role_requests_in(
        &self,
        projects: Option<&[String]>,
    ) -> Result<Vec<RoleRequest>> {
        require_current_schema(&self.pool).await?;
        // No projects reads nothing, not the global delivering rows (#144).
        if projects.is_some_and(<[String]>::is_empty) {
            return Ok(Vec::new());
        }
        let columns = COLUMNS.replace("body", "'' AS body");
        let mut rows = Vec::new();
        let queued = match projects {
            None => vec![None],
            Some(projects) => projects.iter().map(Some).collect(),
        };
        for project in queued {
            let read = match project {
                None => {
                    sqlx::query(&format!(
                        "SELECT {columns} FROM role_requests INDEXED BY role_requests_queued \
                     WHERE state = 'queued'"
                    ))
                    .fetch_all(&self.pool)
                    .await
                }
                Some(project) => {
                    sqlx::query(&format!(
                        "SELECT {columns} FROM role_requests INDEXED BY role_requests_queued \
                     WHERE state = 'queued' AND project = ?1"
                    ))
                    .bind(project)
                    .fetch_all(&self.pool)
                    .await
                }
            }
            .map_err(adapter_error)?;
            for row in &read {
                rows.push(role_request(row)?);
            }
        }
        let delivering = sqlx::query(&format!(
            "SELECT {columns} FROM role_requests INDEXED BY role_requests_delivering \
             WHERE state = 'delivering'"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        for row in &delivering {
            let request = role_request(row)?;
            if projects.is_none_or(|projects| projects.contains(&request.project)) {
                rows.push(request);
            }
        }
        rows.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(rows)
    }

    /// Every queued request, oldest first.
    pub async fn all_queued_role_requests(&self) -> Result<Vec<RoleRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM role_requests INDEXED BY role_requests_queued \
             WHERE state = 'queued' ORDER BY created_at, id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(role_request)
        .collect()
    }

    /// Delivered requests with a limit that are not yet escalated or answered,
    /// `project`'s queued requests, oldest first, with empty bodies: the
    /// directory view counts them and dates the oldest, and never reads a
    /// body (plan 203).
    pub async fn queued_role_request_heads(&self, project: &str) -> Result<Vec<RoleRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!(
            "SELECT {} FROM role_requests INDEXED BY role_requests_queued \
             WHERE project = ?1 AND state = 'queued' ORDER BY created_at, id",
            COLUMNS.replacen(" body,", " '' AS body,", 1)
        ))
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(role_request)
        .collect()
    }

    /// oldest delivery first: the watchdog round's one read (s12).
    pub async fn open_limited_role_requests(&self) -> Result<Vec<RoleRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(&format!(
            "SELECT {COLUMNS} FROM role_requests INDEXED BY role_requests_limited \
             WHERE state = 'delivered' AND limit_ms IS NOT NULL \
             AND escalated_at IS NULL AND answered_at IS NULL ORDER BY delivered_at, id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(role_request)
        .collect()
    }

    /// Claim request `id` for one time-limit notification at `now`, before
    /// it is sent (review of #133, F001 and F002). In one statement the
    /// request must still be delivered, unanswered, not yet told at `stage`
    /// and not escalated, and hold no live claim; so an answer that lands
    /// after the round's read wins, and two rounds never send at once. A
    /// claim older than [`LIMIT_CLAIM_LEASE_MS`] was left by a stopped daemon
    /// and may be taken again. `None` when the request is not claimable;
    /// otherwise the seat a stopped attempt had pinned, if any (see
    /// [`Self::pin_role_request_limit`]).
    pub async fn claim_role_request_limit(
        &self,
        id: &str,
        stage: LimitStage,
        now: u64,
    ) -> Result<Option<LimitClaim>> {
        require_current_schema(&self.pool).await?;
        let stage_open = match stage {
            LimitStage::CheckIn => "checkin_at IS NULL AND ",
            LimitStage::Notice => "",
        };
        let sql = format!(
            "UPDATE role_requests SET limit_claim_at = ?2 WHERE id = ?1 \
             AND state = 'delivered' AND answered_at IS NULL AND {stage_open}escalated_at IS NULL \
             AND (limit_claim_at IS NULL OR limit_claim_at <= ?3) RETURNING limit_claim_target"
        );
        let pool = self.pool.clone();
        let id = id.to_string();
        let stale_before = sql_i64(now.saturating_sub(LIMIT_CLAIM_LEASE_MS), "limit claim")?;
        let now = sql_i64(now, "limit claim")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(&sql)
                .bind(&id)
                .bind(now)
                .bind(stale_before)
                .fetch_optional(&mut *tx)
                .await
                .map_err(adapter_error)?;
            let claim = row
                .map(|row| {
                    row.try_get::<Option<String>, _>("limit_claim_target")
                        .map(|pinned| LimitClaim { pinned })
                        .map_err(adapter_error)
                })
                .transpose()?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(claim)
        })
        .await
    }

    /// Pin the seat the claim taken at `claimed_at` resolved to, before the
    /// notification is sent to it (review of #133). A retry after the claim
    /// expires asks delivery whether that same seat already has the message
    /// ([`Self::limit_notification_admitted`]) instead of resolving again, so
    /// a PA that rotated meanwhile is not told twice. `false` when that claim
    /// is gone, and then nothing may be sent.
    pub async fn pin_role_request_limit(
        &self,
        id: &str,
        claimed_at: u64,
        target: &str,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let id = id.to_string();
        let target = target.to_string();
        let claimed_at = sql_i64(claimed_at, "limit claim")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let changed = sqlx::query(
                "UPDATE role_requests SET limit_claim_target = ?3 \
                 WHERE id = ?1 AND limit_claim_at = ?2",
            )
            .bind(&id)
            .bind(claimed_at)
            .bind(&target)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected()
                == 1;
            tx.commit().await.map_err(adapter_error)?;
            Ok(changed)
        })
        .await
    }

    /// When local message `msg_id` reached `recipient`'s session, in ms, or
    /// `None` while it is still held (plan 214, F73): see [`REACHED_SQL`].
    pub async fn message_reached_at(&self, recipient: &str, msg_id: &str) -> Result<Option<u64>> {
        require_current_schema(&self.pool).await?;
        let at: Option<i64> = sqlx::query_scalar(REACHED_SQL)
            .bind(recipient)
            .bind(msg_id)
            .fetch_one(&self.pool)
            .await
            .map_err(adapter_error)?;
        at.map(|at| sql_u64(at, "reached_at")).transpose()
    }

    /// Did delivery already take local message `msg_id` for `recipient`? The
    /// same evidence boot recovery uses: a ledger row, a `delivery:<recipient>`
    /// job in any state, or a held FYI, each an indexed SEARCH.
    pub async fn limit_notification_admitted(&self, recipient: &str, msg_id: &str) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        sqlx::query_scalar::<_, bool>(ADMITTED_SQL)
            .bind(recipient)
            .bind(msg_id)
            .fetch_one(&self.pool)
            .await
            .map_err(adapter_error)
    }

    /// The claimed notification was admitted: stamp `stage` at `at` and
    /// clear the claim taken at `claimed_at`. `false` when that claim is gone.
    pub async fn finish_role_request_limit(
        &self,
        id: &str,
        stage: LimitStage,
        claimed_at: u64,
        at: u64,
    ) -> Result<bool> {
        let column = match stage {
            LimitStage::CheckIn => "checkin_at",
            LimitStage::Notice => "escalated_at",
        };
        self.limit_write(
            format!(
                "UPDATE role_requests SET {column} = ?2, limit_claim_at = NULL, \
                 limit_claim_target = NULL WHERE id = ?1 AND limit_claim_at = ?3"
            ),
            id,
            &[at, claimed_at],
        )
        .await
    }

    /// The claimed notification was not admitted: drop the claim taken at
    /// `claimed_at` and its pin, so the next round resolves and tries again.
    /// `false` when it is gone.
    pub async fn release_role_request_limit(&self, id: &str, claimed_at: u64) -> Result<bool> {
        self.limit_write(
            "UPDATE role_requests SET limit_claim_at = NULL, limit_claim_target = NULL \
             WHERE id = ?1 AND limit_claim_at = ?2"
                .to_string(),
            id,
            &[claimed_at],
        )
        .await
    }

    async fn limit_write(&self, sql: String, id: &str, times: &[u64]) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let id = id.to_string();
        let times = times
            .iter()
            .map(|at| sql_i64(*at, "limit stamp"))
            .collect::<Result<Vec<_>>>()?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let mut query = sqlx::query(&sql).bind(&id);
            for at in times {
                query = query.bind(at);
            }
            let changed = query
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?
                .rows_affected()
                == 1;
            tx.commit().await.map_err(adapter_error)?;
            Ok(changed)
        })
        .await
    }

    /// Record that an output answered request `id` at `at` (s06: lane B's
    /// `pij output --answers <id>` calls this). Idempotent: a second answer
    /// keeps the first time and returns `false`, as does an unknown id. From
    /// then on the s12 round skips the request.
    pub async fn mark_role_request_answered(&self, id: &str, at: u64) -> Result<bool> {
        self.stamp_once(id, "answered_at", at).await
    }

    /// Set `column` once: a second call changes nothing and returns `false`.
    async fn stamp_once(&self, id: &str, column: &'static str, at: u64) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let id = id.to_string();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let changed = sqlx::query(&format!(
                "UPDATE role_requests SET {column} = ?2 WHERE id = ?1 AND {column} IS NULL"
            ))
            .bind(&id)
            .bind(sql_i64(at, column)?)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected()
                == 1;
            tx.commit().await.map_err(adapter_error)?;
            Ok(changed)
        })
        .await
    }

    /// Claim a queued request for `holder` before dispatching it: the one
    /// atomic step that decides which drain delivers it. In the same
    /// statement, `holder` must be live and hold the request's role now (the
    /// instance it names, or any for an instance-less request), so a drain
    /// scheduled before a release can never deliver to the old holder
    /// (security review of #129, S1). `false` when it is no longer queued,
    /// another drain won, or `holder` does not hold the role now; the request
    /// then stays queued.
    pub async fn claim_role_request(&self, id: &str, holder: &str) -> Result<bool> {
        self.transition(
            id,
            holder,
            "UPDATE role_requests SET state = 'delivering', holder = ?2 \
             WHERE id = ?1 AND state = 'queued' \
             AND EXISTS (SELECT 1 FROM seats WHERE seats.id = ?2 AND seats.tombstoned_at IS NULL) \
             AND EXISTS (SELECT 1 FROM community_assignments AS held \
                 WHERE held.project = role_requests.project AND held.role = role_requests.role \
                 AND held.seat = ?2 AND held.to_at IS NULL \
                 AND (role_requests.instance = '' OR held.instance = role_requests.instance))",
            None,
        )
        .await
    }

    /// Mark a request claimed for `holder` delivered at `at`. `false` when it
    /// is not claimed for that holder, so a request is delivered at most once.
    pub async fn mark_role_request_delivered(
        &self,
        id: &str,
        holder: &str,
        at: u64,
    ) -> Result<bool> {
        self.transition(
            id,
            holder,
            "UPDATE role_requests SET state = 'delivered', delivered_at = ?3 \
             WHERE id = ?1 AND state = 'delivering' AND holder = ?2",
            Some(at),
        )
        .await
    }

    /// Give a claimed request back to the queue after its dispatch failed, so
    /// the next assign can take it.
    pub async fn release_role_request_claim(&self, id: &str, holder: &str) -> Result<bool> {
        self.transition(
            id,
            holder,
            "UPDATE role_requests SET state = 'queued', holder = NULL \
             WHERE id = ?1 AND state = 'delivering' AND holder = ?2",
            None,
        )
        .await
    }

    /// Forget a request a send claimed for `holder` but could not dispatch
    /// (refused, or the cold-wake guard said no), so the sender's retry
    /// resolves afresh. Never touches a queued or delivered request.
    pub async fn abandon_role_request(&self, id: &str, holder: &str) -> Result<bool> {
        self.transition(
            id,
            holder,
            "DELETE FROM role_requests WHERE id = ?1 AND state = 'delivering' AND holder = ?2",
            None,
        )
        .await
    }

    /// Settle every request a stopped daemon left `delivering`: claimed, but
    /// neither marked delivered nor released (review of #129). Run once at
    /// boot, before anything claims. One the delivery path already took for
    /// its holder (a ledger row, a delivery job, or a held FYI under its
    /// msg_id) is marked delivered at `now_ms`, so it is never sent twice. Any
    /// other goes back to `queued` with no holder: the next assign drains it
    /// and the PA digest lists it. One write transaction. Returns how many
    /// were marked delivered and how many re-queued.
    pub async fn recover_delivering_role_requests(&self, now_ms: u64) -> Result<(u64, u64)> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let now = sql_i64(now_ms, "delivered_at")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let delivered = sqlx::query(RECOVER_TAKEN_SQL)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?
                .rows_affected();
            let requeued = sqlx::query(
                "UPDATE role_requests SET state = 'queued', holder = NULL \
                 WHERE state = 'delivering'",
            )
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();
            tx.commit().await.map_err(adapter_error)?;
            Ok((delivered, requeued))
        })
        .await
    }

    async fn transition(
        &self,
        id: &str,
        holder: &str,
        sql: &'static str,
        at: Option<u64>,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let (id, holder) = (id.to_string(), holder.to_string());
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let mut query = sqlx::query(sql).bind(&id).bind(&holder);
            if let Some(at) = at {
                query = query.bind(sql_i64(at, "delivered_at")?);
            }
            let changed = query
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?
                .rows_affected()
                == 1;
            tx.commit().await.map_err(adapter_error)?;
            Ok(changed)
        })
        .await
    }
}

fn role_request(row: &sqlx::sqlite::SqliteRow) -> Result<RoleRequest> {
    let text = |name: &str| -> Result<String> { row.try_get(name).map_err(adapter_error) };
    let reply: String = text("reply")?;
    let state: String = text("state")?;
    let fyi: i64 = row.try_get("fyi").map_err(adapter_error)?;
    let limit_ms: Option<i64> = row.try_get("limit_ms").map_err(adapter_error)?;
    let created_at: i64 = row.try_get("created_at").map_err(adapter_error)?;
    let delivered_at: Option<i64> = row.try_get("delivered_at").map_err(adapter_error)?;
    let stamp = |name: &str| -> Result<Option<u64>> {
        let value: Option<i64> = row.try_get(name).map_err(adapter_error)?;
        value.map(|at| sql_u64(at, "timestamp")).transpose()
    };
    let corrupt = |field: &str, value: &str| PijError::Adapter {
        adapter: "store/role_requests".to_string(),
        message: format!("role request {field} {value:?} is not a known value"),
    };
    Ok(RoleRequest {
        id: text("id")?,
        project: text("project")?,
        role: text("role")?,
        instance: text("instance")?,
        requester: text("requester")?,
        body: text("body")?,
        in_reply_to: row.try_get("in_reply_to").map_err(adapter_error)?,
        fyi: fyi != 0,
        reply: ReplyMode::parse(&reply).ok_or_else(|| corrupt("reply", &reply))?,
        limit_ms: limit_ms
            .map(|limit| sql_u64(limit, "limit_ms"))
            .transpose()?,
        state: match state.as_str() {
            "queued" => RequestState::Queued,
            "delivering" => RequestState::Delivering,
            "delivered" => RequestState::Delivered,
            other => return Err(corrupt("state", other)),
        },
        holder: row.try_get("holder").map_err(adapter_error)?,
        created_at: sql_u64(created_at, "created_at")?,
        delivered_at: delivered_at
            .map(|at| sql_u64(at, "delivered_at"))
            .transpose()?,
        checkin_at: stamp("checkin_at")?,
        escalated_at: stamp("escalated_at")?,
        answered_at: stamp("answered_at")?,
    })
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    /// Performance P001 on #129: every evidence probe in boot recovery is an
    /// indexed SEARCH, never a SCAN that grows with retained job history. The
    /// outer read is the `role_requests_delivering` partial index.
    #[tokio::test]
    async fn boot_recovery_probes_are_indexed_searches() {
        let pool = crate::open("").await.expect("memory");
        let plan: Vec<String> =
            sqlx::query(&format!("EXPLAIN QUERY PLAN {}", super::RECOVER_TAKEN_SQL))
                .bind(0_i64)
                .fetch_all(&pool)
                .await
                .expect("plan")
                .iter()
                .map(|row| row.get::<String, _>("detail"))
                .collect();
        let text = plan.join("\n");
        // The outer read walks the partial index of stranded claims, which is
        // normally empty; every probe inside it must be a SEARCH.
        for (table, index) in [
            ("role_requests", "role_requests_delivering"),
            ("ledger", "sqlite_autoindex_delivered_messages_1"),
            ("jobs", "jobs_by_dedupe"),
            ("fyis", "sqlite_autoindex_fyis_1"),
        ] {
            assert!(
                plan.iter()
                    .any(|line| line.contains(&format!("{table} USING"))
                        && line.contains(&format!("INDEX {index}"))),
                "{table} does not use {index}:\n{text}"
            );
        }
        for table in ["ledger", "jobs", "fyis"] {
            assert!(
                plan.iter()
                    .any(|line| line.starts_with(&format!("SEARCH {table} "))),
                "{table} is not a SEARCH:\n{text}"
            );
        }
        assert!(
            !plan
                .iter()
                .any(|line| line.starts_with("SCAN")
                    && !line.contains("INDEX role_requests_delivering")),
            "a probe scans:\n{text}"
        );
    }

    /// Review of #133: the time-limit retry's admission probe is the same
    /// three indexed SEARCHes, with no SCAN at all.
    #[tokio::test]
    async fn limit_admission_probes_are_indexed_searches() {
        let pool = crate::open("").await.expect("memory");
        // Plan 214: the arrival probe (F73) reads the same three indexes.
        for sql in [super::ADMITTED_SQL, super::REACHED_SQL] {
            limit_probe_is_indexed(&pool, sql).await;
        }
    }

    async fn limit_probe_is_indexed(pool: &crate::StorePool, sql: &str) {
        let plan: Vec<String> = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
            .bind("pij-pa")
            .bind("role-limit-notice-r")
            .fetch_all(pool)
            .await
            .expect("plan")
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect();
        let text = plan.join("\n");
        for (table, index) in [
            ("ledger", "sqlite_autoindex_delivered_messages_1"),
            ("jobs", "jobs_by_dedupe"),
            ("fyis", "sqlite_autoindex_fyis_1"),
        ] {
            assert!(
                plan.iter()
                    .any(|line| line.starts_with(&format!("SEARCH {table} "))
                        && line.contains(&format!("INDEX {index}"))),
                "{table} is not a SEARCH on {index}:\n{text}"
            );
        }
        // The FROM-less outer SELECT is SQLite's `SCAN CONSTANT ROW`.
        assert!(
            !plan
                .iter()
                .any(|line| line.starts_with("SCAN") && line != "SCAN CONSTANT ROW"),
            "a probe scans:\n{text}"
        );
    }
}
