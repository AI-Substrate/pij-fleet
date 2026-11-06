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

    /// Every queued request, oldest first: the PA digest's list.
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

    /// Delivered requests with a limit that are not yet escalated or answered,
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

    /// Record the holder's check-in at `at`; `false` when one was recorded.
    pub async fn mark_role_request_checked_in(&self, id: &str, at: u64) -> Result<bool> {
        self.stamp_once(id, "checkin_at", at).await
    }

    /// Record the PA notice at `at`; `false` when one was recorded.
    pub async fn mark_role_request_escalated(&self, id: &str, at: u64) -> Result<bool> {
        self.stamp_once(id, "escalated_at", at).await
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
