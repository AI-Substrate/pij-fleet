//! The community directory's store side: the pointer (plan 190, migration
//! 0031) and role holders (plan 195, migration 0032).
//!
//! The governance branch is the truth. These rows index it (where each
//! document lives, which commit wrote it, who holds which role) and can be
//! rebuilt from the branch head at any time.

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatId};
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error};
use crate::role_assertion::{current_role, is_prime, live_seat, role_event};
use crate::spine::append_in_transaction;

/// One document in a project's community directory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommunityPointer {
    /// The project slug.
    pub project: String,
    /// Path relative to the governance worktree's top level.
    pub path: String,
    /// The commit that last wrote it.
    pub commit: String,
}

impl SqliteOrchestration {
    /// Record or move the pointers for documents a write just committed.
    pub async fn record_community_pointers(&self, pointers: &[CommunityPointer]) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let pointers = pointers.to_vec();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            for pointer in &pointers {
                upsert(&mut tx, pointer).await?;
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// Replace every pointer for `project` with `pointers`, in one transaction:
    /// the rebuild from the governance branch head.
    pub async fn replace_community_pointers(
        &self,
        project: &str,
        pointers: &[CommunityPointer],
    ) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let project = project.to_string();
        let pointers = pointers.to_vec();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query("DELETE FROM community_pointers WHERE project = ?1")
                .bind(&project)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            for pointer in pointers.iter().filter(|pointer| pointer.project == project) {
                upsert(&mut tx, pointer).await?;
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// Every pointer for `project`, by path.
    pub async fn community_pointers(&self, project: &str) -> Result<Vec<CommunityPointer>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT project, path, commit_sha FROM community_pointers \
             WHERE project = ?1 ORDER BY path",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(CommunityPointer {
                    project: row.try_get("project").map_err(adapter_error)?,
                    path: row.try_get("path").map_err(adapter_error)?,
                    commit: row.try_get("commit_sha").map_err(adapter_error)?,
                })
            })
            .collect()
    }
}

async fn upsert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    pointer: &CommunityPointer,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO community_pointers (project, path, commit_sha) VALUES (?1, ?2, ?3) \
         ON CONFLICT (project, path) DO UPDATE SET commit_sha = excluded.commit_sha",
    )
    .bind(&pointer.project)
    .bind(&pointer.path)
    .bind(&pointer.commit)
    .execute(&mut **tx)
    .await
    .map_err(adapter_error)?;
    Ok(())
}

/// One holding: a seat holding a role (and instance) in a project, open until
/// `to` is set. Timestamps are kept as the holders document writes them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Assignment {
    /// The project slug.
    pub project: String,
    /// The role id.
    pub role: String,
    /// The instance key for a many-holder role, else empty.
    pub instance: String,
    /// The holding seat.
    pub seat: String,
    /// The seat's harness, when known.
    pub harness: Option<String>,
    /// The seat's harness session id, when known.
    pub session: Option<String>,
    /// The machine the seat runs on, when known.
    pub machine: Option<String>,
    /// When the holding began.
    pub from: String,
    /// When it ended; `None` while open.
    pub to: Option<String>,
    /// The seat that assigned it.
    pub assigned_by: String,
    /// The seat that released it, or the closing seat.
    pub released_by: Option<String>,
    /// Why it was assigned.
    pub why: String,
    /// Why it ended (`close` for a seat close).
    pub release_why: Option<String>,
    /// An end recorded by a close that the writer has not committed yet.
    pub end_pending: bool,
}

/// What an assignment commit changed besides its own row.
#[derive(Clone, Debug, Default)]
pub struct AssignmentCommit {
    /// The `role-set` event when the seat's role changed (ruling C1), with its
    /// committed sequence, ready to broadcast.
    pub events: Vec<Event>,
    /// The seat's role after the commit, or `None` when it has none.
    pub seat_role: Option<String>,
}

const ASSIGNMENT_COLUMNS: &str = "project, role, instance, seat, harness, session, machine, from_at, to_at, \
     assigned_by, released_by, why, release_why, end_pending";

impl SqliteOrchestration {
    /// Record a new holding and, in the same transaction, set the seat's role
    /// to `seat_role` (ruling C1). A prime is never re-roled, and an unchanged
    /// role appends no event. Hold the bus's publication boundary around this
    /// call, as for [`Self::assert_role_committed`].
    ///
    /// # Errors
    /// `E-RS-ARG` when the seat is not live; `E-RS-DIRECTORY-HELD` when this
    /// exact holding row exists; any SQL failure rolls everything back.
    pub async fn record_assignment_committed(
        &self,
        row: &Assignment,
        seat_role: Option<&str>,
        assigned_at: u64,
    ) -> Result<AssignmentCommit> {
        require_current_schema(&self.pool).await?;
        let at = i64::try_from(assigned_at).map_err(|_| PijError::Adapter {
            adapter: "store/community".to_string(),
            message: format!("assigned_at={assigned_at} exceeds SQLite's signed integer range"),
        })?;
        let pool = self.pool.clone();
        let row = row.clone();
        let seat_role = seat_role.map(str::to_owned);
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let seat = SeatId::from(row.seat.as_str());
            if live_seat(&mut tx, &seat).await?.is_none() {
                tx.rollback().await.map_err(adapter_error)?;
                return Err(PijError::GovernanceRefused {
                    code: "E-RS-ARG".to_string(),
                    record: row.seat.clone(),
                });
            }
            let inserted = insert(&mut tx, &row).await?;
            if !inserted {
                tx.rollback().await.map_err(adapter_error)?;
                return Err(PijError::GovernanceRefused {
                    code: "E-RS-DIRECTORY-HELD".to_string(),
                    record: format!("{}/{}", row.role, row.seat),
                });
            }
            let mut commit = AssignmentCommit {
                events: Vec::new(),
                seat_role: current_role(&mut tx, &seat).await?,
            };
            if let Some(role) = seat_role
                && !is_prime(&mut tx, &seat).await?
                && commit.seat_role.as_deref() != Some(role.as_str())
            {
                sqlx::query(
                    "INSERT INTO seat_roles (seat, role, assigned_by, assigned_at) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(seat) DO UPDATE SET role=excluded.role, assigned_by=excluded.assigned_by, assigned_at=excluded.assigned_at",
                )
                .bind(seat.as_str())
                .bind(&role)
                .bind(&row.assigned_by)
                .bind(at)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
                let actor = SeatId::from(row.assigned_by.as_str());
                let mut event = role_event(&actor, &seat, Some(&role), assigned_at);
                event.seq = Some(append_in_transaction(&mut tx, &event).await?);
                commit.events.push(event);
                commit.seat_role = Some(role);
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok(commit)
        })
        .await
    }

    /// Record a closed past holding (plan 208, F15): the row only. Unlike an
    /// assign there is no live-seat check and no seat role: the seat may be
    /// long gone. `false` when that holding is already recorded.
    pub async fn record_history(&self, row: &Assignment) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let row = row.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let inserted = insert(&mut tx, &row).await?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(inserted)
        })
        .await
    }

    /// Record the ends of holdings (`to`, `released_by`, `release_why` and
    /// `end_pending`), matched by project, role, instance, seat and `from`.
    pub async fn end_assignments(&self, ends: &[Assignment]) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let ends = ends.to_vec();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            for end in &ends {
                sqlx::query(
                    "UPDATE community_assignments SET to_at = ?6, released_by = ?7, release_why = ?8, end_pending = ?9 \
                     WHERE project = ?1 AND role = ?2 AND instance = ?3 AND seat = ?4 AND from_at = ?5",
                )
                .bind(&end.project)
                .bind(&end.role)
                .bind(&end.instance)
                .bind(&end.seat)
                .bind(&end.from)
                .bind(&end.to)
                .bind(&end.released_by)
                .bind(&end.release_why)
                .bind(i64::from(end.end_pending))
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }

    /// Every open holding of `seat`, in any project.
    pub async fn open_assignments(&self, seat: &str) -> Result<Vec<Assignment>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(&format!(
            "SELECT {ASSIGNMENT_COLUMNS} FROM community_assignments \
             WHERE seat = ?1 AND to_at IS NULL ORDER BY project, role, instance, from_at"
        ))
        .bind(seat)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter().map(assignment).collect()
    }

    /// `seat`'s holdings in its current context (F130): every open one, and
    /// each closed since the seat's last done compact, or since the seat
    /// existed when it has never been compacted (seat ids are never reused).
    /// What a seat learned in a holding stays in its context after the
    /// release, until a compact clears it. A compact queues as a control
    /// (`delivery-control:<seat>`, Copilot) or with the seat's bodies
    /// (`delivery:<seat>`). One query: `jobs_serial` walked newest first to
    /// the seat's last done compact (so it reads only what the seat received
    /// since), and the seat's assignment rows.
    pub async fn context_assignments(&self, seat: &str) -> Result<Vec<Assignment>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(&format!(
            "SELECT {ASSIGNMENT_COLUMNS} FROM community_assignments \
             WHERE seat = ?1 AND (to_at IS NULL OR to_at > COALESCE(( \
                 SELECT strftime('%Y-%m-%dT%H:%M:%fZ', acked_at, 'unixepoch') FROM jobs \
                 WHERE serial_key = ?1 AND state = 'done' \
                   AND kind IN ('delivery:' || ?1, 'delivery-control:' || ?1) \
                   AND json_extract(payload, '$.command') = 'compact' \
                 ORDER BY id DESC LIMIT 1), '')) \
             ORDER BY project, role, instance, from_at"
        ))
        .bind(seat)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter().map(assignment).collect()
    }

    /// Every holding in `project`, open and closed, in a stable order.
    pub async fn assignments(&self, project: &str) -> Result<Vec<Assignment>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(&format!(
            "SELECT {ASSIGNMENT_COLUMNS} FROM community_assignments \
             WHERE project = ?1 ORDER BY role, instance, from_at, seat"
        ))
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter().map(assignment).collect()
    }

    /// Replace every holding in `project` with `rows`, in one transaction: the
    /// rebuild from the governance branch head.
    ///
    /// # Errors
    /// `E-RS-DIRECTORY-ROW` when two rows are the same holding (role,
    /// instance, seat and `from`); nothing is replaced.
    pub async fn replace_assignments(&self, project: &str, rows: &[Assignment]) -> Result<()> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let project = project.to_string();
        let rows = rows.to_vec();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            sqlx::query("DELETE FROM community_assignments WHERE project = ?1")
                .bind(&project)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
            for row in rows.iter().filter(|row| row.project == project) {
                if !insert(&mut tx, row).await? {
                    tx.rollback().await.map_err(adapter_error)?;
                    return Err(PijError::GovernanceRefused {
                        code: "E-RS-DIRECTORY-ROW".to_string(),
                        record: format!(
                            "role {} seat {} instance {:?} from {}",
                            row.role, row.seat, row.instance, row.from
                        ),
                    });
                }
            }
            tx.commit().await.map_err(adapter_error)
        })
        .await
    }
}

/// `false` when the exact holding row already exists.
async fn insert(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, row: &Assignment) -> Result<bool> {
    let done = sqlx::query(&format!(
        "INSERT INTO community_assignments ({ASSIGNMENT_COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) ON CONFLICT DO NOTHING"
    ))
    .bind(&row.project)
    .bind(&row.role)
    .bind(&row.instance)
    .bind(&row.seat)
    .bind(&row.harness)
    .bind(&row.session)
    .bind(&row.machine)
    .bind(&row.from)
    .bind(&row.to)
    .bind(&row.assigned_by)
    .bind(&row.released_by)
    .bind(&row.why)
    .bind(&row.release_why)
    .bind(i64::from(row.end_pending))
    .execute(&mut **tx)
    .await
    .map_err(adapter_error)?;
    Ok(done.rows_affected() > 0)
}

fn assignment(row: &sqlx::sqlite::SqliteRow) -> Result<Assignment> {
    let get = |name: &str| -> Result<String> { row.try_get(name).map_err(adapter_error) };
    let opt = |name: &str| -> Result<Option<String>> { row.try_get(name).map_err(adapter_error) };
    Ok(Assignment {
        project: get("project")?,
        role: get("role")?,
        instance: get("instance")?,
        seat: get("seat")?,
        harness: opt("harness")?,
        session: opt("session")?,
        machine: opt("machine")?,
        from: get("from_at")?,
        to: opt("to_at")?,
        assigned_by: get("assigned_by")?,
        released_by: opt("released_by")?,
        why: get("why")?,
        release_why: opt("release_why")?,
        end_pending: row
            .try_get::<i64, _>("end_pending")
            .map_err(adapter_error)?
            != 0,
    })
}
