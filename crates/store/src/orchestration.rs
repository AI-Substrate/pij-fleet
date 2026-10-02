//! SQLite persistence for orchestration facts and atomic claims.

use sqlx::{Pool, Row, Sqlite};

use pij_core::error::{PijError, Result};
use pij_core::model::{SeatDescriptor, SeatId};
use pij_core::orchestration::{
    DescriptorField, ParentAttribution, ReconcileDecision, RoleAssignment, SpawnRecord, StreamPlan,
    VerifiedDescriptor, derive_parent, plan_descriptor_reconciliation,
};

use crate::migrate::{begin_write, owned_write, require_current_schema};

/// Result of recording an immutable spawn owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnRecordOutcome {
    /// New mapping persisted.
    Recorded,
    /// The same mapping was already present.
    Existing,
    /// That launch id was already attributed to another seat.
    Conflict {
        /// Existing owner; the adapter never overwrites it.
        existing_spawner: SeatId,
    },
}

/// Result of reserving a project/stream allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamReservation {
    /// The complete stream recipe is now durable.
    Reserved,
    /// A unique project/slug, ordinal, branch, or path was already reserved.
    Conflict,
}
/// Result of acknowledging a durable dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchAck {
    /// Dispatch transitioned to acknowledged with matching packet evidence.
    Acknowledged,
    /// It was already acknowledged; replay is harmless.
    AlreadyAcknowledged,
    /// No dispatch with that id exists.
    Missing,
    /// Caller is not the assignee and cannot acknowledge for it.
    NotAssignee {
        /// Actual assignee.
        assignee: SeatId,
    },
    /// Supplied digest differs from the persisted digest, including legacy unknown digests.
    ShaMismatch,
}

/// Result of claiming a single-holder baton.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseClaim {
    /// Caller became the sole holder.
    Claimed,
    /// Another live claim already holds the baton.
    Held {
        /// Current holder.
        holder: SeatId,
        /// Current lease id.
        lease_id: String,
    },
}

/// Current baton holder.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BatonLease {
    /// Baton name.
    pub baton: String,
    /// Sole holder.
    pub holder: SeatId,
    /// Stable claim id.
    pub lease_id: String,
    /// Granted request, absent for legacy direct claims.
    pub request_id: Option<String>,
    /// Caller-supplied acquisition time.
    pub acquired_at: u64,
}

/// SQLite-backed orchestration operations.
///
/// Mutations that span rows use [`begin_write`], so lease and allocation
/// uniqueness is decided under `BEGIN IMMEDIATE`, never a deferred read-upgrade.
#[derive(Clone)]
pub struct SqliteOrchestration {
    pub(crate) pool: Pool<Sqlite>,
}

impl SqliteOrchestration {
    /// Wrap an open, migrated pool.
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Persist the owner of a launch without ever re-attributing an existing id.
    pub async fn record_spawn(&self, record: &SpawnRecord) -> Result<SpawnRecordOutcome> {
        require_current_schema(&self.pool).await?;
        let recorded_at = sql_i64(record.recorded_at, "spawn recorded_at")?;
        let pool = self.pool.clone();
        let record = record.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = sqlx::query(
                "INSERT INTO spawn_records (spawn_id, spawner, recorded_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(spawn_id) DO NOTHING",
            )
            .bind(&record.spawn_id)
            .bind(record.spawner.as_str())
            .bind(recorded_at)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();
            if inserted == 1 {
                tx.commit().await.map_err(adapter_error)?;
                return Ok(SpawnRecordOutcome::Recorded);
            }
            let existing: String =
                sqlx::query_scalar("SELECT spawner FROM spawn_records WHERE spawn_id = ?1")
                    .bind(&record.spawn_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            if existing == record.spawner.as_str() {
                Ok(SpawnRecordOutcome::Existing)
            } else {
                Ok(SpawnRecordOutcome::Conflict {
                    existing_spawner: SeatId(existing),
                })
            }
        })
        .await
    }

    /// Read the spawn record for a descriptor, if this binary created it.
    pub async fn spawn_record(&self, spawn_id: &str) -> Result<Option<SpawnRecord>> {
        require_current_schema(&self.pool).await?;
        let row = sqlx::query(
            "SELECT spawn_id, spawner, recorded_at FROM spawn_records WHERE spawn_id = ?1",
        )
        .bind(spawn_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?;
        row.map(|row| {
            Ok(SpawnRecord {
                spawn_id: row.try_get("spawn_id").map_err(adapter_error)?,
                spawner: SeatId(row.try_get("spawner").map_err(adapter_error)?),
                recorded_at: sql_u64(
                    row.try_get("recorded_at").map_err(adapter_error)?,
                    "spawn recorded_at",
                )?,
            })
        })
        .transpose()
    }

    /// Resolve self-declared or spawn-derived parent attribution.
    pub async fn parent_attribution(
        &self,
        descriptor: &SeatDescriptor,
    ) -> Result<ParentAttribution> {
        let record = match descriptor.spawn_id.as_deref() {
            Some(id) if !id.is_empty() => self.spawn_record(id).await?,
            _ => None,
        };
        Ok(derive_parent(descriptor, record.as_ref()))
    }

    /// Reconcile two registry rows and retain complete evidence in one write.
    pub async fn reconcile_descriptors(
        &self,
        left: VerifiedDescriptor,
        right: VerifiedDescriptor,
    ) -> Result<ReconcileDecision> {
        require_current_schema(&self.pool).await?;
        let decision = plan_descriptor_reconciliation(left, right);
        let ReconcileDecision::Merge(plan) = decision else {
            return Ok(decision);
        };

        let verified_at = sql_i64(plan.verified_at, "descriptor verified_at")?;
        let survivor_json = serde_json::to_string(&plan.survivor).map_err(json_error)?;
        let alias_json = serde_json::to_string(&plan.alias).map_err(json_error)?;
        let mismatch_json =
            serde_json::to_string(&plan.mismatches.iter().map(field_name).collect::<Vec<_>>())
                .map_err(json_error)?;

        let pool = self.pool.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        // Reconciliation does not attest a new incarnation. Its candidate may
        // only supply capability for the process/session/pane still stored here.
        let survivor_updated = sqlx::query(
            "UPDATE seats SET harness=?2, pane=?3, folder=?4, model=?5, provider=?6, effort=?7, \
             native_extension_delivery=CASE WHEN harness=?2 AND pid=?9 AND proc_start=?10 \
                 AND harness_session=?11 AND pane IS ?3 AND tombstoned_at IS NULL \
                 THEN ?8 ELSE 0 END \
             WHERE id=?1",
        )
        .bind(plan.survivor.id.as_str())
        .bind(plan.survivor.harness.as_str())
        .bind(plan.survivor.pane.as_deref())
        .bind(&plan.survivor.folder)
        .bind(plan.survivor.model.as_deref())
        .bind(plan.survivor.provider.as_deref())
        .bind(plan.survivor.effort.as_deref())
        .bind(i64::from(
            plan.survivor.native_extension_delivery && plan.survivor.tombstoned_at.is_none(),
        ))
        .bind(plan.survivor.proc.map(|process| i64::from(process.pid)))
        .bind(plan.survivor.proc.map(|process| process.proc_start as i64))
        .bind(plan.survivor.harness_session.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?
        .rows_affected();
        let alias_updated = sqlx::query(
            "UPDATE seats SET tombstoned_at=?2, tombstone_reason=?3, native_extension_delivery=0 \
             WHERE id=?1",
        )
        .bind(plan.alias.id.as_str())
        .bind(verified_at)
        .bind(format!(
            "equivalent descriptor merged into {}",
            plan.survivor.id
        ))
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?
        .rows_affected();
        if survivor_updated != 1 || alias_updated != 1 {
            tx.rollback().await.map_err(adapter_error)?;
            return Err(PijError::Adapter {
                adapter: "store/orchestration".to_string(),
                message: format!(
                    "descriptor reconciliation requires both registry rows (survivor={}, alias={}); adopt/register the missing row first",
                    plan.survivor.id, plan.alias.id
                ),
            });
        }
        sqlx::query(
            "INSERT INTO descriptor_merge_history \
             (survivor_id, alias_id, survivor_json, alias_json, mismatched_fields, verified_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(plan.survivor.id.as_str())
        .bind(plan.alias.id.as_str())
        .bind(survivor_json)
        .bind(alias_json)
        .bind(mismatch_json)
        .bind(verified_at)
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?;
        tx.commit().await.map_err(adapter_error)?;
        Ok(ReconcileDecision::Merge(plan))
        })
        .await
    }

    /// Assign or replace one seat's role atomically.
    pub async fn assign_role(&self, assignment: &RoleAssignment) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(assignment.assigned_at, "role assigned_at")?;
        let pool = self.pool.clone();
        let assignment = assignment.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        sqlx::query(
            "INSERT INTO seat_roles (seat, role, assigned_by, assigned_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(seat) DO UPDATE SET role=excluded.role, assigned_by=excluded.assigned_by, assigned_at=excluded.assigned_at",
        )
        .bind(assignment.seat.as_str())
        .bind(&assignment.role)
        .bind(assignment.assigned_by.as_str())
        .bind(at)
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?;
        tx.commit().await.map_err(adapter_error)?;
        Ok(())
        })
        .await
    }

    /// Remove one seat's role; an already-unassigned seat is unchanged.
    pub async fn clear_role(&self, seat: &SeatId) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let seat = seat.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query("DELETE FROM seat_roles WHERE seat=?1")
                .bind(seat.as_str())
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(())
        })
        .await
    }

    /// Read one complete role assignment, or `None` for an unassigned seat.
    pub async fn seat_role(&self, seat: &SeatId) -> Result<Option<RoleAssignment>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT seat, role, assigned_by, assigned_at FROM seat_roles WHERE seat=?1")
            .bind(seat.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(role_assignment)
            .transpose()
    }

    /// Read every persisted role in deterministic seat order for roster joins.
    pub async fn list_roles(&self) -> Result<Vec<RoleAssignment>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT seat, role, assigned_by, assigned_at FROM seat_roles ORDER BY seat",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.into_iter().map(role_assignment).collect()
    }

    /// Reserve the complete stream recipe before Git mutates a worktree.
    pub async fn reserve_stream(
        &self,
        plan: &StreamPlan,
        actor: &SeatId,
        at: u64,
    ) -> Result<StreamReservation> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(at, "stream created_at")?;
        let pool = self.pool.clone();
        let plan = plan.clone();
        let actor = actor.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let result = sqlx::query(
            "INSERT INTO streams \
             (id, project, ordinal, slug, branch, worktree, base_ref, created_by, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT DO NOTHING",
        )
        .bind(format!("{}:{}", plan.project, plan.slug))
        .bind(&plan.project)
        .bind(i64::from(plan.ordinal))
        .bind(&plan.slug)
        .bind(&plan.branch)
        .bind(plan.worktree.to_string_lossy().as_ref())
        .bind(&plan.base_ref)
        .bind(actor.as_str())
        .bind(at)
        .execute(&mut *tx)
        .await;
        let inserted = match result {
            Ok(done) => done.rows_affected(),
            Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
                tx.rollback().await.map_err(adapter_error)?;
                return Err(PijError::Adapter {
                    adapter: "store/orchestration".to_string(),
                    message: format!(
                        "project `{}` is not registered; create it before reserving stream `{}`",
                        plan.project, plan.slug
                    ),
                });
            }
            Err(error) => return Err(adapter_error(error)),
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(if inserted == 1 {
            StreamReservation::Reserved
        } else {
            StreamReservation::Conflict
        })
        })
        .await
    }

    /// Atomically claim a request-less baton. Request-backed claims use `grant_baton`.
    pub async fn claim_lease(&self, lease: &BatonLease) -> Result<LeaseClaim> {
        require_current_schema(&self.pool).await?;
        if lease.request_id.is_some() {
            return Err(crate::governance::invalid_record(
                "request-backed leases require grant_baton so request and lease advance atomically",
            ));
        }
        let acquired_at = sql_i64(lease.acquired_at, "lease acquired_at")?;
        let pool = self.pool.clone();
        let lease = lease.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = sqlx::query(
                "INSERT INTO baton_leases (baton, holder, lease_id, acquired_at, request_id) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(baton) DO NOTHING",
            )
            .bind(&lease.baton)
            .bind(lease.holder.as_str())
            .bind(&lease.lease_id)
            .bind(acquired_at)
            .bind(&lease.request_id)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?
            .rows_affected();
            if inserted == 1 {
                sqlx::query(
                "INSERT INTO baton_lease_history (baton, holder, lease_id, action, at, request_id) \
                 VALUES (?1, ?2, ?3, 'claimed', ?4, ?5)",
            )
            .bind(&lease.baton)
            .bind(lease.holder.as_str())
            .bind(&lease.lease_id)
            .bind(acquired_at)
            .bind(&lease.request_id)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
                tx.commit().await.map_err(adapter_error)?;
                return Ok(LeaseClaim::Claimed);
            }
            let row = sqlx::query("SELECT holder, lease_id FROM baton_leases WHERE baton=?1")
                .bind(&lease.baton)
                .fetch_one(&mut *tx)
                .await
                .map_err(adapter_error)?;
            let held = LeaseClaim::Held {
                holder: SeatId(row.try_get("holder").map_err(adapter_error)?),
                lease_id: row.try_get("lease_id").map_err(adapter_error)?,
            };
            tx.commit().await.map_err(adapter_error)?;
            Ok(held)
        })
        .await
    }

    /// Release exactly the caller's request-less claim; request-backed leases require evidence.
    pub async fn release_lease(
        &self,
        baton: &str,
        holder: &SeatId,
        lease_id: &str,
        at: u64,
    ) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(at, "lease released_at")?;
        let pool = self.pool.clone();
        let baton = baton.to_owned();
        let holder = holder.clone();
        let lease_id = lease_id.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let removed =
            sqlx::query("DELETE FROM baton_leases WHERE baton=?1 AND holder=?2 AND lease_id=?3 AND request_id IS NULL")
                .bind(&baton)
                .bind(holder.as_str())
                .bind(&lease_id)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?
                .rows_affected();
        if removed == 1 {
            sqlx::query(
                "INSERT INTO baton_lease_history (baton, holder, lease_id, action, at) \
                 VALUES (?1, ?2, ?3, 'released', ?4)",
            )
            .bind(&baton)
            .bind(holder.as_str())
            .bind(&lease_id)
            .bind(at)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
        }
        tx.commit().await.map_err(adapter_error)?;
        Ok(removed == 1)
        })
        .await
    }

    /// Read the current holder, if any.
    pub async fn lease(&self, baton: &str) -> Result<Option<BatonLease>> {
        require_current_schema(&self.pool).await?;
        let row = sqlx::query(
            "SELECT baton, holder, lease_id, request_id, acquired_at FROM baton_leases WHERE baton=?1",
        )
        .bind(baton)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?;
        row.map(|row| {
            Ok(BatonLease {
                baton: row.try_get("baton").map_err(adapter_error)?,
                holder: SeatId(row.try_get("holder").map_err(adapter_error)?),
                lease_id: row.try_get("lease_id").map_err(adapter_error)?,
                request_id: row.try_get("request_id").map_err(adapter_error)?,
                acquired_at: sql_u64(
                    row.try_get("acquired_at").map_err(adapter_error)?,
                    "lease acquired_at",
                )?,
            })
        })
        .transpose()
    }
}

fn field_name(field: &DescriptorField) -> &'static str {
    match field {
        DescriptorField::Harness => "harness",
        DescriptorField::Pane => "pane",
        DescriptorField::Folder => "folder",
        DescriptorField::Model => "model",
        DescriptorField::Provider => "provider",
        DescriptorField::Effort => "effort",
    }
}

pub(crate) fn sql_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| PijError::Adapter {
        adapter: "store/orchestration".to_string(),
        message: format!("{field}={value} exceeds SQLite's signed integer range"),
    })
}

pub(crate) fn sql_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| PijError::Adapter {
        adapter: "store/orchestration".to_string(),
        message: format!("{field}={value} is negative in SQLite"),
    })
}

pub(crate) fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/orchestration".to_string(),
        message: error.to_string(),
    }
}

fn json_error(error: serde_json::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/orchestration".to_string(),
        message: format!("could not encode descriptor merge evidence: {error}"),
    }
}

fn role_assignment(row: sqlx::sqlite::SqliteRow) -> Result<RoleAssignment> {
    Ok(RoleAssignment {
        seat: SeatId(row.try_get("seat").map_err(adapter_error)?),
        role: row.try_get("role").map_err(adapter_error)?,
        assigned_by: SeatId(row.try_get("assigned_by").map_err(adapter_error)?),
        assigned_at: sql_u64(
            row.try_get("assigned_at").map_err(adapter_error)?,
            "role assigned_at",
        )?,
    })
}
