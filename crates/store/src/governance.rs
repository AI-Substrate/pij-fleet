//! Typed governance records on the existing orchestration pool.

use std::path::PathBuf;

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatId};
use pij_core::orchestration::{
    BatonDefinition, BatonRequest, BatonRequestState, Fence, PlanAttestation, PrimeDesignation,
    PrimeState, Project, ProjectUpdate, Stream, StreamState, TaskAssignment, TaskCloseReason,
};
use sqlx::{Row, sqlite::SqliteRow};

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{BatonLease, SqliteOrchestration, adapter_error, sql_i64, sql_u64};
use crate::spine::append_in_transaction;

/// Outcome of a guarded mutation. Only `Changed` represents a new durable event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GovernanceOutcome<T> {
    /// Mutation committed with this complete persisted row.
    Changed(T),
    /// Idempotent replay; the original metadata was preserved.
    Unchanged(T),
    /// Required record does not exist.
    Missing,
    /// Existing state prevents the mutation; no rows changed.
    Conflict {
        /// Stable persistence conflict discriminator for daemon refusal mapping.
        code: &'static str,
    },
}

impl SqliteOrchestration {
    /// Create a complete project once without rewriting a prior creator or metadata.
    pub async fn create_project(&self, project: &Project) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(project.created_at, "project created_at")?;
        let result = sqlx::query(
            "INSERT INTO projects (slug, description, repo, plan_path, prime_id, created_by, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(slug) DO NOTHING",
        ).bind(&project.slug).bind(&project.description).bind(&project.repo).bind(&project.plan_path)
            .bind(project.prime_id.as_ref().map(SeatId::as_str)).bind(project.created_by.as_str()).bind(at)
            .execute(&self.pool).await.map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Read one complete project.
    pub async fn project(&self, slug: &str) -> Result<Option<Project>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM projects WHERE slug=?1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(project_row)
            .transpose()
    }

    /// Read projects in stable slug order.
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM projects ORDER BY slug")
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(project_row)
            .collect()
    }

    /// Update only supplied metadata, atomically preserving all other fields.
    pub async fn update_project(
        &self,
        slug: &str,
        update: &ProjectUpdate,
    ) -> Result<Option<Project>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "UPDATE projects SET description=COALESCE(?2, description), repo=COALESCE(?3, repo), \
             plan_path=CASE WHEN ?4 THEN ?5 ELSE plan_path END, \
             prime_id=CASE WHEN ?6 THEN ?7 ELSE prime_id END WHERE slug=?1 RETURNING *",
        )
        .bind(slug)
        .bind(&update.description)
        .bind(&update.repo)
        .bind(update.plan_path.is_some())
        .bind(update.plan_path.as_ref().and_then(|value| value.as_deref()))
        .bind(update.prime_id.is_some())
        .bind(
            update
                .prime_id
                .as_ref()
                .and_then(|value| value.as_ref())
                .map(SeatId::as_str),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?
        .map(project_row)
        .transpose()
    }

    /// Read one reserved, created or closed stream by canonical project:slug id.
    pub async fn stream(&self, id: &str) -> Result<Option<Stream>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM streams WHERE id=?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(stream_row)
            .transpose()
    }

    /// Read allocations in ordinal/id order, optionally restricted to a project.
    pub async fn list_streams(&self, project: Option<&str>) -> Result<Vec<Stream>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM streams WHERE (?1 IS NULL OR project=?1) ORDER BY ordinal, id")
            .bind(project)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(stream_row)
            .collect()
    }

    /// Advance reservation/creation/closure; closure never touches the filesystem.
    pub async fn set_stream_state(
        &self,
        id: &str,
        state: StreamState,
    ) -> Result<GovernanceOutcome<Stream>> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let id = id.to_owned();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let existing = sqlx::query("SELECT * FROM streams WHERE id=?1")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(adapter_error)?
                .map(stream_row)
                .transpose()?;
            let outcome = match existing {
                None => GovernanceOutcome::Missing,
                Some(row) if row.state == state => GovernanceOutcome::Unchanged(row),
                Some(row) if row.state == StreamState::Closed || state == StreamState::Reserved => {
                    GovernanceOutcome::Conflict {
                        code: "stream-state-conflict",
                    }
                }
                Some(_) => {
                    let state = match state {
                        StreamState::Reserved => "reserved",
                        StreamState::Created => "created",
                        StreamState::Closed => "closed",
                    };
                    let row = sqlx::query("UPDATE streams SET state=?2 WHERE id=?1 RETURNING *")
                        .bind(&id)
                        .bind(state)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(adapter_error)?;
                    GovernanceOutcome::Changed(stream_row(row)?)
                }
            };
            tx.commit().await.map_err(adapter_error)?;
            Ok(outcome)
        })
        .await
    }

    /// Set the complete descriptive fence for one existing stream.
    pub async fn set_fence(&self, fence: &Fence) -> Result<Fence> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(fence.declared_at, "fence declared_at")?;
        let paths = serde_json::to_string(&fence.paths)
            .map_err(|error| invalid_record(&error.to_string()))?;
        let shared = serde_json::to_string(&fence.shared)
            .map_err(|error| invalid_record(&error.to_string()))?;
        let row = sqlx::query(
            "INSERT INTO fences (id, stream, paths, shared, declared_by, declared_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(stream) DO UPDATE SET id=excluded.id, paths=excluded.paths, shared=excluded.shared, \
             declared_by=excluded.declared_by, declared_at=excluded.declared_at RETURNING *",
        ).bind(&fence.id).bind(&fence.stream).bind(paths).bind(shared).bind(fence.declared_by.as_str()).bind(at)
            .fetch_one(&self.pool).await.map_err(adapter_error)?;
        fence_row(row)
    }

    /// Read fences in stream/id order; path-pattern filtering belongs to the daemon.
    pub async fn list_fences(&self, stream: Option<&str>) -> Result<Vec<Fence>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM fences WHERE (?1 IS NULL OR stream=?1) ORDER BY stream, id")
            .bind(stream)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(fence_row)
            .collect()
    }

    /// Open a complete assignment once; closure must use the guarded close operation.
    pub async fn open_task(&self, task: &TaskAssignment) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        if task.closed_at.is_some() || task.close_reason.is_some() {
            return Err(invalid_record("a new task assignment must be open"));
        }
        let at = sql_i64(task.opened_at, "task opened_at")?;
        let result = sqlx::query(
            "INSERT INTO task_assignments (id, node_id, task, project, opened_by, opened_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO NOTHING",
        )
        .bind(&task.id)
        .bind(task.node_id.as_str())
        .bind(&task.task)
        .bind(&task.project)
        .bind(task.opened_by.as_str())
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Read one complete assignment, including its first closure reason.
    pub async fn task(&self, id: &str) -> Result<Option<TaskAssignment>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM task_assignments WHERE id=?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(task_row)
            .transpose()
    }

    /// Read assignments in opened/id order, optionally for one node.
    pub async fn list_tasks(&self, node: Option<&SeatId>) -> Result<Vec<TaskAssignment>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM task_assignments WHERE (?1 IS NULL OR node_id=?1) ORDER BY opened_at, id")
            .bind(node.map(SeatId::as_str)).fetch_all(&self.pool).await.map_err(adapter_error)?
            .into_iter().map(task_row).collect()
    }

    /// First closure wins. The typed reason admits only the four supported values.
    pub async fn close_task(
        &self,
        id: &str,
        reason: TaskCloseReason,
        at: u64,
    ) -> Result<GovernanceOutcome<TaskAssignment>> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(at, "task closed_at")?;
        let pool = self.pool.clone();
        let id = id.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let existing = sqlx::query("SELECT * FROM task_assignments WHERE id=?1")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(task_row)
            .transpose()?;
        let outcome = match existing {
            None => GovernanceOutcome::Missing,
            Some(row) if row.close_reason == Some(reason) => GovernanceOutcome::Unchanged(row),
            Some(row) if row.closed_at.is_some() => GovernanceOutcome::Conflict {
                code: "task-already-closed",
            },
            Some(_) => {
                let reason = match reason {
                    TaskCloseReason::Done => "done",
                    TaskCloseReason::Cancelled => "cancelled",
                    TaskCloseReason::Failed => "failed",
                    TaskCloseReason::Superseded => "superseded",
                };
                let row = sqlx::query("UPDATE task_assignments SET closed_at=?2, close_reason=?3 WHERE id=?1 RETURNING *")
                    .bind(&id).bind(at).bind(reason).fetch_one(&mut *tx).await.map_err(adapter_error)?;
                GovernanceOutcome::Changed(task_row(row)?)
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }

    /// Read plan linkage for a seat, independently of native delivery capability.
    pub async fn plan_attestation(&self, seat: &SeatId) -> Result<Option<PlanAttestation>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM plan_attestations WHERE seat=?1")
            .bind(seat.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(attestation_row)
            .transpose()
    }

    /// Read all plan linkages in seat order for a single-query node-tree join.
    pub async fn list_plan_attestations(&self) -> Result<Vec<PlanAttestation>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM plan_attestations ORDER BY seat")
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(attestation_row)
            .collect()
    }

    /// Replace only a seat's plan linkage; never touches its registry descriptor.
    pub async fn put_plan_attestation(
        &self,
        attestation: &PlanAttestation,
    ) -> Result<PlanAttestation> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(attestation.attested_at, "plan attested_at")?;
        let row = sqlx::query(
            "INSERT INTO plan_attestations (seat, plan_id, attested_by, attested_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(seat) DO UPDATE SET plan_id=excluded.plan_id, attested_by=excluded.attested_by, \
             attested_at=excluded.attested_at RETURNING *",
        ).bind(attestation.seat.as_str()).bind(&attestation.plan_id).bind(attestation.attested_by.as_str()).bind(at)
            .fetch_one(&self.pool).await.map_err(adapter_error)?;
        attestation_row(row)
    }

    /// Define a baton once, retaining complete resource/probe/repository metadata.
    pub async fn define_baton(&self, baton: &BatonDefinition) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(baton.created_at, "baton created_at")?;
        let result = sqlx::query(
            "INSERT INTO batons (name, description, resource, probe, repo, created_by, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(name) DO NOTHING",
        )
        .bind(&baton.name)
        .bind(&baton.description)
        .bind(&baton.resource)
        .bind(&baton.probe)
        .bind(&baton.repo)
        .bind(baton.created_by.as_str())
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Read one complete baton definition.
    pub async fn baton(&self, name: &str) -> Result<Option<BatonDefinition>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM batons WHERE name=?1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(baton_row)
            .transpose()
    }

    /// Read definitions in name order.
    pub async fn list_batons(&self) -> Result<Vec<BatonDefinition>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM batons ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(baton_row)
            .collect()
    }

    /// Record immutable request evidence before any grant is possible.
    pub async fn request_baton(
        &self,
        request: &BatonRequest,
    ) -> Result<GovernanceOutcome<BatonRequest>> {
        require_current_schema(&self.pool).await?;
        if request.state != BatonRequestState::Requested
            || request.id.is_empty()
            || request.purpose.is_empty()
        {
            return Err(invalid_record(
                "a baton request requires a fresh requested state, id and purpose",
            ));
        }
        let at = sql_i64(request.requested_at, "baton requested_at")?;
        let pool = self.pool.clone();
        let request = request.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let existing = sqlx::query("SELECT * FROM baton_requests WHERE id=?1")
            .bind(&request.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(request_row)
            .transpose()?;
        let outcome = if let Some(existing) = existing {
            if existing.baton == request.baton
                && existing.requester == request.requester
                && existing.purpose == request.purpose
                && existing.pin == request.pin
                && existing.evidence == request.evidence
                && existing.requested_at == request.requested_at
            {
                GovernanceOutcome::Unchanged(existing)
            } else {
                GovernanceOutcome::Conflict {
                    code: "baton-request-conflict",
                }
            }
        } else {
            let defined: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM batons WHERE name=?1)")
                    .bind(&request.baton)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(adapter_error)?;
            if !defined {
                GovernanceOutcome::Missing
            } else {
                let row = sqlx::query(
                    "INSERT INTO baton_requests (id, baton, requester, purpose, pin, evidence, requested_at, state) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'requested') RETURNING *",
                ).bind(&request.id).bind(&request.baton).bind(request.requester.as_str()).bind(&request.purpose)
                    .bind(&request.pin).bind(&request.evidence).bind(at)
                    .fetch_one(&mut *tx).await.map_err(adapter_error)?;
                GovernanceOutcome::Changed(request_row(row)?)
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }

    /// Read immutable evidence and current lifecycle for one request.
    pub async fn baton_request(&self, id: &str) -> Result<Option<BatonRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM baton_requests WHERE id=?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(request_row)
            .transpose()
    }

    /// Read a baton's requests in requested/id order, including completed requests.
    pub async fn list_baton_requests(&self, baton: &str) -> Result<Vec<BatonRequest>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM baton_requests WHERE baton=?1 ORDER BY requested_at, id")
            .bind(baton)
            .fetch_all(&self.pool)
            .await
            .map_err(adapter_error)?
            .into_iter()
            .map(request_row)
            .collect()
    }

    /// Grant a stored request and claim its sole lease in one write transaction.
    /// The daemon must authorize the grant and verify the immutable request pin first.
    pub async fn grant_baton(
        &self,
        baton: &str,
        request_id: &str,
        lease_id: &str,
        at: u64,
    ) -> Result<GovernanceOutcome<BatonLease>> {
        require_current_schema(&self.pool).await?;
        if lease_id.is_empty() {
            return Err(invalid_record("baton grant requires a new lease id"));
        }
        let at_sql = sql_i64(at, "baton acquired_at")?;
        let pool = self.pool.clone();
        let baton = baton.to_owned();
        let request_id = request_id.to_owned();
        let lease_id = lease_id.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let request = sqlx::query("SELECT * FROM baton_requests WHERE id=?1")
            .bind(&request_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(request_row)
            .transpose()?;
        let current = sqlx::query("SELECT * FROM baton_leases WHERE baton=?1")
            .bind(&baton)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(lease_row)
            .transpose()?;
        let outcome = match (request, current) {
            (None, _) => GovernanceOutcome::Missing,
            (Some(request), _) if request.baton != baton => GovernanceOutcome::Conflict {
                code: "baton-request-mismatch",
            },
            (Some(request), Some(lease))
                if lease.request_id.as_deref() == Some(request_id.as_str())
                    && lease.holder == request.requester =>
            {
                GovernanceOutcome::Unchanged(lease)
            }
            (_, Some(_)) => GovernanceOutcome::Conflict { code: "baton-held" },
            (Some(request), None) if request.state != BatonRequestState::Requested => {
                GovernanceOutcome::Conflict {
                    code: "baton-request-completed",
                }
            }
            (Some(request), None) => {
                let used: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM baton_lease_history WHERE lease_id=?1 UNION ALL SELECT 1 FROM baton_leases WHERE lease_id=?1)",
                ).bind(&lease_id).fetch_one(&mut *tx).await.map_err(adapter_error)?;
                if used {
                    GovernanceOutcome::Conflict {
                        code: "baton-lease-id-reused",
                    }
                } else {
                    let row = sqlx::query(
                        "INSERT INTO baton_leases (baton, holder, lease_id, request_id, acquired_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING *",
                    ).bind(&baton).bind(request.requester.as_str()).bind(&lease_id).bind(&request_id).bind(at_sql)
                        .fetch_one(&mut *tx).await.map_err(adapter_error)?;
                    sqlx::query("UPDATE baton_requests SET state='granted' WHERE id=?1")
                        .bind(&request_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(adapter_error)?;
                    sqlx::query(
                        "INSERT INTO baton_lease_history (baton, holder, lease_id, action, at, request_id) \
                         VALUES (?1, ?2, ?3, 'claimed', ?4, ?5)",
                    ).bind(&baton).bind(request.requester.as_str()).bind(&lease_id).bind(at_sql).bind(&request_id)
                        .execute(&mut *tx).await.map_err(adapter_error)?;
                    GovernanceOutcome::Changed(lease_row(row)?)
                }
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }

    /// Return this holder's exact lease; empty evidence means absent optional evidence.
    /// Invoke through `EventBus::publish_committed`; the returned event is already persisted.
    pub async fn return_baton(
        &self,
        baton: &str,
        holder: &SeatId,
        lease_id: &str,
        evidence: &str,
        at: u64,
    ) -> Result<(Event, BatonLease)> {
        self.finish_baton(baton, lease_id, holder, evidence, at, "returned")
            .await
    }

    /// Reclaim only the exact lease observed by the daemon's authorized caller.
    /// Invoke through `EventBus::publish_committed`; the returned event is already persisted.
    pub async fn reclaim_baton(
        &self,
        baton: &str,
        lease_id: &str,
        actor: &SeatId,
        evidence: &str,
        at: u64,
    ) -> Result<(Event, BatonLease)> {
        self.finish_baton(baton, lease_id, actor, evidence, at, "reclaimed")
            .await
    }

    async fn finish_baton(
        &self,
        baton: &str,
        lease_id: &str,
        actor: &SeatId,
        evidence: &str,
        at: u64,
        kind: &'static str,
    ) -> Result<(Event, BatonLease)> {
        require_current_schema(&self.pool).await?;
        if kind == "reclaimed" && evidence.trim().is_empty() {
            return Err(invalid_record("baton reclaim requires evidence"));
        }
        let at_sql = sql_i64(at, "baton released_at")?;
        let pool = self.pool.clone();
        let baton = baton.to_owned();
        let lease_id = lease_id.to_owned();
        let actor = actor.clone();
        let evidence = evidence.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let lease = sqlx::query("SELECT * FROM baton_leases WHERE baton=?1")
            .bind(&baton)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(lease_row)
            .transpose()?
            .ok_or_else(|| PijError::GovernanceRefused {
                code: "E-RS-LEASE-STALE".into(),
                record: "absent".into(),
            })?;
        if lease.lease_id != lease_id {
            return Err(PijError::GovernanceRefused {
                code: "E-RS-LEASE-STALE".into(),
                record: lease.lease_id,
            });
        }
        if kind == "returned" && actor != lease.holder {
            return Err(PijError::GovernanceRefused {
                code: "E-RS-OWNERSHIP".into(),
                record: baton,
            });
        }
        let removed = sqlx::query("DELETE FROM baton_leases WHERE baton=?1 AND lease_id=?2")
            .bind(&baton)
            .bind(&lease_id)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();
        if removed != 1 {
            tx.rollback().await.map_err(adapter_error)?;
            return Err(PijError::GovernanceRefused {
                code: "E-RS-LEASE-STALE".into(),
                record: lease.lease_id,
            });
        }
        if let Some(request_id) = &lease.request_id {
            sqlx::query("UPDATE baton_requests SET state=?2 WHERE id=?1")
                .bind(request_id)
                .bind(kind)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
        }
        sqlx::query(
            "INSERT INTO baton_lease_history (baton, holder, lease_id, action, at, request_id, actor, evidence, release_kind) \
             VALUES (?1, ?2, ?3, 'released', ?4, ?5, ?6, ?7, ?8)",
        ).bind(&baton).bind(lease.holder.as_str()).bind(&lease_id).bind(at_sql).bind(&lease.request_id)
            .bind(actor.as_str()).bind((!evidence.is_empty()).then_some(evidence.as_str())).bind(kind).execute(&mut *tx).await.map_err(adapter_error)?;
        let mut event = Event {
            seq: None,
            v: 1,
            at,
            kind: if kind == "returned" {
                "baton.returned"
            } else {
                "baton.reclaimed"
            }
            .into(),
            seat: Some(actor.clone()),
            payload: serde_json::json!({ "actor": actor, "action": kind, "record": &lease })
                .to_string(),
        };
        let seq = match append_in_transaction(&mut tx, &event).await {
            Ok(seq) => seq,
            Err(error) => {
                tx.rollback().await.map_err(adapter_error)?;
                return Err(error);
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        event.seq = Some(seq);
        Ok((event, lease))
        }).await
    }

    /// Read the independent prime designation, including retained retirement state.
    pub async fn prime(&self) -> Result<Option<PrimeDesignation>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM prime_designation WHERE singleton=1")
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(prime_row)
            .transpose()
    }

    /// First current prime wins; retire/unset explicitly before changing a current seat.
    pub async fn designate_prime(
        &self,
        designation: &PrimeDesignation,
    ) -> Result<GovernanceOutcome<PrimeDesignation>> {
        require_current_schema(&self.pool).await?;
        if designation.state != PrimeState::Current {
            return Err(invalid_record("a new prime designation must be current"));
        }
        let at = sql_i64(designation.designated_at, "prime designated_at")?;
        let pool = self.pool.clone();
        let designation = designation.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let current = sqlx::query("SELECT * FROM prime_designation WHERE singleton=1")
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(prime_row)
            .transpose()?;
        let outcome = match current {
            Some(row) if row.state == PrimeState::Current && row.seat == designation.seat => {
                GovernanceOutcome::Unchanged(row)
            }
            Some(row) if row.state == PrimeState::Current => GovernanceOutcome::Conflict {
                code: "prime-already-current",
            },
            _ => {
                let row = sqlx::query(
                    "INSERT INTO prime_designation (singleton, seat, designated_by, designated_at, state) \
                     VALUES (1, ?1, ?2, ?3, 'current') ON CONFLICT(singleton) DO UPDATE SET seat=excluded.seat, \
                     designated_by=excluded.designated_by, designated_at=excluded.designated_at, state='current' RETURNING *",
                ).bind(designation.seat.as_str()).bind(designation.designated_by.as_str()).bind(at)
                    .fetch_one(&mut *tx).await.map_err(adapter_error)?;
                GovernanceOutcome::Changed(prime_row(row)?)
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }

    /// Retire the observed seat without rewriting its designation metadata.
    pub async fn retire_prime(&self, seat: &SeatId) -> Result<GovernanceOutcome<PrimeDesignation>> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let seat = seat.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let current = sqlx::query("SELECT * FROM prime_designation WHERE singleton=1")
                .fetch_optional(&mut *tx)
                .await
                .map_err(adapter_error)?
                .map(prime_row)
                .transpose()?;
            let outcome = match current {
                None => GovernanceOutcome::Missing,
                Some(row) if row.seat != seat => GovernanceOutcome::Conflict {
                    code: "prime-seat-mismatch",
                },
                Some(row) if row.state == PrimeState::Retired => GovernanceOutcome::Unchanged(row),
                Some(_) => {
                    let row = sqlx::query(
                    "UPDATE prime_designation SET state='retired' WHERE singleton=1 RETURNING *",
                )
                .fetch_one(&mut *tx)
                .await
                .map_err(adapter_error)?;
                    GovernanceOutcome::Changed(prime_row(row)?)
                }
            };
            tx.commit().await.map_err(adapter_error)?;
            Ok(outcome)
        })
        .await
    }

    /// Remove only the observed prime seat; stale commands cannot unset another seat.
    /// Deletion and its canonical unset event commit together; publish through `publish_committed`.
    pub async fn unset_prime(
        &self,
        seat: &SeatId,
        actor: &SeatId,
        at: u64,
    ) -> Result<(Event, PrimeDesignation)> {
        require_current_schema(&self.pool).await?;
        sql_i64(at, "prime unset_at")?;
        let pool = self.pool.clone();
        let seat = seat.clone();
        let actor = actor.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let current = sqlx::query("SELECT * FROM prime_designation WHERE singleton=1")
                .fetch_optional(&mut *tx)
                .await
                .map_err(adapter_error)?
                .map(prime_row)
                .transpose()?
                .ok_or_else(|| PijError::GovernanceRefused {
                    code: "E-RS-NOT-FOUND".into(),
                    record: seat.as_str().into(),
                })?;
            if current.seat != seat {
                return Err(PijError::GovernanceRefused {
                    code: "E-RS-PRIME-STALE".into(),
                    record: seat.as_str().into(),
                });
            }
            let removed =
                sqlx::query("DELETE FROM prime_designation WHERE singleton=1 AND seat=?1")
                    .bind(seat.as_str())
                    .execute(&mut *tx)
                    .await
                    .map_err(adapter_error)?
                    .rows_affected();
            if removed != 1 {
                tx.rollback().await.map_err(adapter_error)?;
                return Err(PijError::GovernanceRefused {
                    code: "E-RS-PRIME-STALE".into(),
                    record: seat.as_str().into(),
                });
            }
            let mut event = Event {
                seq: None,
                v: 1,
                at,
                kind: "prime-set".into(),
                seat: Some(current.seat.clone()),
                payload: serde_json::json!({ "actor": actor, "action": "unset", "record": null })
                    .to_string(),
            };
            let seq = match append_in_transaction(&mut tx, &event).await {
                Ok(seq) => seq,
                Err(error) => {
                    tx.rollback().await.map_err(adapter_error)?;
                    return Err(error);
                }
            };
            tx.commit().await.map_err(adapter_error)?;
            event.seq = Some(seq);
            Ok((event, current))
        })
        .await
    }
}

pub(crate) fn invalid_record(message: &str) -> PijError {
    PijError::Adapter {
        adapter: "store/governance".into(),
        message: message.into(),
    }
}

fn project_row(row: SqliteRow) -> Result<Project> {
    Ok(Project {
        slug: row.try_get("slug").map_err(adapter_error)?,
        description: row.try_get("description").map_err(adapter_error)?,
        repo: row.try_get("repo").map_err(adapter_error)?,
        plan_path: row.try_get("plan_path").map_err(adapter_error)?,
        prime_id: row
            .try_get::<Option<String>, _>("prime_id")
            .map_err(adapter_error)?
            .map(SeatId),
        created_by: SeatId(row.try_get("created_by").map_err(adapter_error)?),
        created_at: sql_u64(
            row.try_get("created_at").map_err(adapter_error)?,
            "project created_at",
        )?,
    })
}

fn stream_row(row: SqliteRow) -> Result<Stream> {
    let state: String = row.try_get("state").map_err(adapter_error)?;
    let state = match state.as_str() {
        "reserved" => StreamState::Reserved,
        "created" => StreamState::Created,
        "closed" => StreamState::Closed,
        _ => return Err(invalid_record("unknown stored stream state")),
    };
    let ordinal: i64 = row.try_get("ordinal").map_err(adapter_error)?;
    Ok(Stream {
        id: row.try_get("id").map_err(adapter_error)?,
        project: row.try_get("project").map_err(adapter_error)?,
        ordinal: u32::try_from(ordinal)
            .map_err(|_| invalid_record("stream ordinal outside u32 range"))?,
        slug: row.try_get("slug").map_err(adapter_error)?,
        branch: row.try_get("branch").map_err(adapter_error)?,
        worktree: PathBuf::from(
            row.try_get::<String, _>("worktree")
                .map_err(adapter_error)?,
        ),
        base_ref: row.try_get("base_ref").map_err(adapter_error)?,
        created_by: SeatId(row.try_get("created_by").map_err(adapter_error)?),
        created_at: sql_u64(
            row.try_get("created_at").map_err(adapter_error)?,
            "stream created_at",
        )?,
        state,
    })
}

fn fence_row(row: SqliteRow) -> Result<Fence> {
    let paths: String = row.try_get("paths").map_err(adapter_error)?;
    let shared: String = row.try_get("shared").map_err(adapter_error)?;
    Ok(Fence {
        id: row.try_get("id").map_err(adapter_error)?,
        stream: row.try_get("stream").map_err(adapter_error)?,
        paths: serde_json::from_str(&paths).map_err(|error| invalid_record(&error.to_string()))?,
        shared: serde_json::from_str(&shared)
            .map_err(|error| invalid_record(&error.to_string()))?,
        declared_by: SeatId(row.try_get("declared_by").map_err(adapter_error)?),
        declared_at: sql_u64(
            row.try_get("declared_at").map_err(adapter_error)?,
            "fence declared_at",
        )?,
    })
}

fn task_row(row: SqliteRow) -> Result<TaskAssignment> {
    let reason: Option<String> = row.try_get("close_reason").map_err(adapter_error)?;
    let close_reason = match reason.as_deref() {
        None => None,
        Some("done") => Some(TaskCloseReason::Done),
        Some("cancelled") => Some(TaskCloseReason::Cancelled),
        Some("failed") => Some(TaskCloseReason::Failed),
        Some("superseded") => Some(TaskCloseReason::Superseded),
        _ => return Err(invalid_record("unknown stored task close reason")),
    };
    let closed_at: Option<i64> = row.try_get("closed_at").map_err(adapter_error)?;
    Ok(TaskAssignment {
        id: row.try_get("id").map_err(adapter_error)?,
        node_id: SeatId(row.try_get("node_id").map_err(adapter_error)?),
        task: row.try_get("task").map_err(adapter_error)?,
        project: row.try_get("project").map_err(adapter_error)?,
        opened_by: SeatId(row.try_get("opened_by").map_err(adapter_error)?),
        opened_at: sql_u64(
            row.try_get("opened_at").map_err(adapter_error)?,
            "task opened_at",
        )?,
        closed_at: closed_at
            .map(|at| sql_u64(at, "task closed_at"))
            .transpose()?,
        close_reason,
    })
}

fn attestation_row(row: SqliteRow) -> Result<PlanAttestation> {
    Ok(PlanAttestation {
        seat: SeatId(row.try_get("seat").map_err(adapter_error)?),
        plan_id: row.try_get("plan_id").map_err(adapter_error)?,
        attested_by: SeatId(row.try_get("attested_by").map_err(adapter_error)?),
        attested_at: sql_u64(
            row.try_get("attested_at").map_err(adapter_error)?,
            "plan attested_at",
        )?,
    })
}

fn baton_row(row: SqliteRow) -> Result<BatonDefinition> {
    Ok(BatonDefinition {
        name: row.try_get("name").map_err(adapter_error)?,
        description: row.try_get("description").map_err(adapter_error)?,
        resource: row.try_get("resource").map_err(adapter_error)?,
        probe: row.try_get("probe").map_err(adapter_error)?,
        repo: row.try_get("repo").map_err(adapter_error)?,
        created_by: SeatId(row.try_get("created_by").map_err(adapter_error)?),
        created_at: sql_u64(
            row.try_get("created_at").map_err(adapter_error)?,
            "baton created_at",
        )?,
    })
}

fn request_row(row: SqliteRow) -> Result<BatonRequest> {
    let state: String = row.try_get("state").map_err(adapter_error)?;
    let state = match state.as_str() {
        "requested" => BatonRequestState::Requested,
        "granted" => BatonRequestState::Granted,
        "returned" => BatonRequestState::Returned,
        "reclaimed" => BatonRequestState::Reclaimed,
        _ => return Err(invalid_record("unknown stored baton request state")),
    };
    Ok(BatonRequest {
        id: row.try_get("id").map_err(adapter_error)?,
        baton: row.try_get("baton").map_err(adapter_error)?,
        requester: SeatId(row.try_get("requester").map_err(adapter_error)?),
        purpose: row.try_get("purpose").map_err(adapter_error)?,
        pin: row.try_get("pin").map_err(adapter_error)?,
        evidence: row.try_get("evidence").map_err(adapter_error)?,
        requested_at: sql_u64(
            row.try_get("requested_at").map_err(adapter_error)?,
            "baton requested_at",
        )?,
        state,
    })
}

fn lease_row(row: SqliteRow) -> Result<BatonLease> {
    Ok(BatonLease {
        baton: row.try_get("baton").map_err(adapter_error)?,
        holder: SeatId(row.try_get("holder").map_err(adapter_error)?),
        lease_id: row.try_get("lease_id").map_err(adapter_error)?,
        request_id: row.try_get("request_id").map_err(adapter_error)?,
        acquired_at: sql_u64(
            row.try_get("acquired_at").map_err(adapter_error)?,
            "baton acquired_at",
        )?,
    })
}

fn prime_row(row: SqliteRow) -> Result<PrimeDesignation> {
    let state: String = row.try_get("state").map_err(adapter_error)?;
    let state = match state.as_str() {
        "current" => PrimeState::Current,
        "retired" => PrimeState::Retired,
        _ => return Err(invalid_record("unknown stored prime state")),
    };
    Ok(PrimeDesignation {
        seat: SeatId(row.try_get("seat").map_err(adapter_error)?),
        designated_by: SeatId(row.try_get("designated_by").map_err(adapter_error)?),
        designated_at: sql_u64(
            row.try_get("designated_at").map_err(adapter_error)?,
            "prime designated_at",
        )?,
        state,
    })
}
