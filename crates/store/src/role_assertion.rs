//! Atomic role assertions on the existing orchestration store.

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatDescriptor, SeatId};
use pij_core::orchestration::{check_placement_role, check_seat_role};
use pij_core::wire;
use serde_json::json;
use sqlx::{Row, SqliteConnection};

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::SqliteOrchestration;
use crate::registry::{descriptor_event, row_to_descriptor, upsert_seat};
use crate::spine::append_in_transaction;

/// Where a governor places the seat it stamps (plan 166).
#[derive(Clone, Debug)]
pub enum Placement {
    /// A freshly launched seat row whose parent is the actor.
    Spawn(Box<SeatDescriptor>),
    /// An existing live seat the actor takes as its child or already parents.
    Link(SeatId),
}

/// One committed placement: the seat as written and what actually changed.
#[derive(Clone, Debug)]
pub struct PlacementCommit {
    /// Committed events in sequence order: `seat.put` and/or `role-set`.
    pub events: Vec<Event>,
    /// The seat row after the commit.
    pub descriptor: SeatDescriptor,
    /// Recorded parent before the commit.
    pub previous_parent: Option<SeatId>,
    /// Whether the seat row (parent or spawn record) was written.
    pub parent_changed: bool,
    /// Whether the role differed and a `role-set` was appended.
    pub role_changed: bool,
}

fn refused(code: &str, record: impl ToString) -> PijError {
    PijError::GovernanceRefused {
        code: code.to_string(),
        record: record.to_string(),
    }
}

async fn live_seat(
    connection: &mut SqliteConnection,
    seat: &SeatId,
) -> Result<Option<SeatDescriptor>> {
    let row = sqlx::query("SELECT * FROM seats WHERE id = ?1")
        .bind(seat.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(adapter_error)?;
    Ok(row
        .as_ref()
        .map(row_to_descriptor)
        .transpose()?
        .filter(|seat| seat.tombstoned_at.is_none()))
}

async fn current_role(connection: &mut SqliteConnection, seat: &SeatId) -> Result<Option<String>> {
    sqlx::query("SELECT role FROM seat_roles WHERE seat = ?1")
        .bind(seat.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(adapter_error)?
        .map(|row| row.try_get("role").map_err(adapter_error))
        .transpose()
}

async fn is_prime(connection: &mut SqliteConnection, seat: &SeatId) -> Result<bool> {
    let designated: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM prime_designation WHERE seat = ?1 AND state = 'current'")
            .bind(seat.as_str())
            .fetch_optional(&mut *connection)
            .await
            .map_err(adapter_error)?;
    Ok(designated.is_some() || current_role(connection, seat).await?.as_deref() == Some("prime"))
}

/// Is `candidate` on `start`'s recorded-parent chain (including `start`)?
async fn is_ancestor(
    connection: &mut SqliteConnection,
    candidate: &SeatId,
    start: &SeatId,
) -> Result<bool> {
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = Some(start.clone());
    while let Some(seat) = cursor {
        if &seat == candidate {
            return Ok(true);
        }
        if !seen.insert(seat.clone()) {
            return Ok(false);
        }
        cursor = sqlx::query_scalar::<_, Option<String>>("SELECT parent FROM seats WHERE id = ?1")
            .bind(seat.as_str())
            .fetch_optional(&mut *connection)
            .await
            .map_err(adapter_error)?
            .flatten()
            .map(SeatId::from);
    }
    Ok(false)
}

fn role_event(actor: &SeatId, target: &SeatId, role: Option<&str>, assigned_at: u64) -> Event {
    Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at: assigned_at,
        kind: "role-set".to_string(),
        seat: Some(target.clone()),
        payload: json!({
            "actor": actor,
            "action": if role.is_some() { "assigned" } else { "unassigned" },
            "record": { "seat": target, "role": role, "assigned_by": actor, "assigned_at": assigned_at },
        }).to_string(),
    }
}

impl SqliteOrchestration {
    /// Commit a role assertion and its event as one SQLite transaction.
    ///
    /// The caller must hold the shared publication boundary from its authority
    /// read through this commit. The pool must be the bus's SQLite spine pool.
    /// The returned event carries its real committed sequence and is ready for
    /// broadcast; this method never calls a publisher or opens a second pool.
    ///
    /// # Errors
    /// Invalid role/timestamp, schema mismatch, or any SQL failure. Failed row
    /// mutation or event append rolls the entire assertion back.
    pub async fn assert_role_committed(
        &self,
        actor: &SeatId,
        target: &SeatId,
        role: Option<&str>,
        assigned_at: u64,
    ) -> Result<Event> {
        require_current_schema(&self.pool).await?;
        if role.is_some_and(|value| check_seat_role(value).is_err()) {
            return Err(PijError::GovernanceRefused {
                code: "E-RS-ARG".to_string(),
                record: target.to_string(),
            });
        }
        let at = i64::try_from(assigned_at).map_err(|_| PijError::Adapter {
            adapter: "store/role-assertion".to_string(),
            message: format!(
                "role assigned_at={assigned_at} exceeds SQLite's signed integer range"
            ),
        })?;
        let mut event = role_event(actor, target, role, assigned_at);
        let pool = self.pool.clone();
        let actor = actor.clone();
        let target = target.clone();
        let role = role.map(str::to_owned);
        owned_write(async move {
        let mut transaction = begin_write(&pool).await?;
        let mutation = match role.as_deref() {
            Some(role) => sqlx::query(
                "INSERT INTO seat_roles (seat, role, assigned_by, assigned_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(seat) DO UPDATE SET role=excluded.role, assigned_by=excluded.assigned_by, assigned_at=excluded.assigned_at",
            )
                .bind(target.as_str())
                .bind(role)
                .bind(actor.as_str())
                .bind(at)
                .execute(&mut *transaction)
                .await,
            None => sqlx::query("DELETE FROM seat_roles WHERE seat = ?1")
                .bind(target.as_str())
                .execute(&mut *transaction)
                .await,
        };
        if let Err(error) = mutation {
            transaction.rollback().await.map_err(adapter_error)?;
            return Err(adapter_error(error));
        }
        let seq = match append_in_transaction(&mut transaction, &event).await {
            Ok(seq) => seq,
            Err(error) => {
                transaction.rollback().await.map_err(adapter_error)?;
                return Err(error);
            }
        };
        transaction.commit().await.map_err(adapter_error)?;
        event.seq = Some(seq);
        Ok(event)
        }).await
    }
    /// Commit a governor's placement and role stamp as one SQLite transaction.
    ///
    /// Authority is read inside the same transaction: the actor must be live; a
    /// linked subject must be live, not the actor, not a prime and not the
    /// actor's ancestor, and its recorded parent must be the actor, absent, or
    /// no longer live. A changed parent or spawn record appends `seat.put`; a
    /// changed role appends `role-set`. An unchanged role appends nothing.
    /// Hold the shared publication boundary as for [`Self::assert_role_committed`].
    ///
    /// # Errors
    /// `GovernanceRefused` with `E-RS-ARG` (unknown/self/role), `E-RS-CYCLE`,
    /// `E-RS-PRIME` or `E-RS-OWNERSHIP` (record = live parent); SQL failures.
    pub async fn place_seat_committed(
        &self,
        actor: &SeatId,
        placement: Placement,
        role: &str,
        assigned_at: u64,
    ) -> Result<PlacementCommit> {
        require_current_schema(&self.pool).await?;
        check_placement_role(role).map_err(|_| refused("E-RS-ARG", role))?;
        let at = i64::try_from(assigned_at).map_err(|_| PijError::Adapter {
            adapter: "store/role-assertion".to_string(),
            message: format!(
                "role assigned_at={assigned_at} exceeds SQLite's signed integer range"
            ),
        })?;
        let pool = self.pool.clone();
        let actor = actor.clone();
        let role = role.to_string();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            if live_seat(&mut tx, &actor).await?.is_none() {
                return Err(refused("E-RS-ARG", &actor));
            }
            let (mut descriptor, previous_parent, write_row) = match placement {
                Placement::Spawn(descriptor) => {
                    let mut descriptor = *descriptor;
                    if descriptor.parent.as_ref() != Some(&actor) {
                        return Err(refused(
                            "E-RS-OWNERSHIP",
                            descriptor.parent.as_ref().map_or("absent", SeatId::as_str),
                        ));
                    }
                    descriptor.machine = None;
                    let previous: Option<Option<String>> =
                        sqlx::query_scalar("SELECT parent FROM seats WHERE id = ?1")
                            .bind(descriptor.id.as_str())
                            .fetch_optional(&mut *tx)
                            .await
                            .map_err(adapter_error)?;
                    (descriptor, previous.flatten().map(SeatId::from), true)
                }
                Placement::Link(target) => {
                    if target == actor {
                        return Err(refused("E-RS-ARG", &target));
                    }
                    let mut subject = live_seat(&mut tx, &target)
                        .await?
                        .ok_or_else(|| refused("E-RS-ARG", &target))?;
                    let previous = subject.parent.clone();
                    let owned = previous.as_ref() == Some(&actor);
                    if !owned {
                        if let Some(parent) = previous.as_ref()
                            && live_seat(&mut tx, parent).await?.is_some()
                        {
                            return Err(refused("E-RS-OWNERSHIP", parent));
                        }
                        if is_prime(&mut tx, &target).await? {
                            return Err(refused("E-RS-PRIME", &target));
                        }
                        if is_ancestor(&mut tx, &target, &actor).await? {
                            return Err(refused("E-RS-CYCLE", &target));
                        }
                        subject.parent = Some(actor.clone());
                    }
                    (subject, previous, !owned)
                }
            };
            let mut events = Vec::with_capacity(2);
            if write_row {
                let mut event = descriptor_event(&descriptor)?;
                let seq = append_in_transaction(&mut tx, &event).await?;
                upsert_seat(&mut tx, &descriptor, seq).await?;
                event.seq = Some(seq);
                events.push(event);
            }
            let role_changed =
                current_role(&mut tx, &descriptor.id).await?.as_deref() != Some(role.as_str());
            if role_changed {
                sqlx::query(
                    "INSERT INTO seat_roles (seat, role, assigned_by, assigned_at) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(seat) DO UPDATE SET role=excluded.role, assigned_by=excluded.assigned_by, assigned_at=excluded.assigned_at",
                )
                .bind(descriptor.id.as_str())
                .bind(&role)
                .bind(actor.as_str())
                .bind(at)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
                let mut event = role_event(&actor, &descriptor.id, Some(&role), assigned_at);
                event.seq = Some(append_in_transaction(&mut tx, &event).await?);
                events.push(event);
            }
            tx.commit().await.map_err(adapter_error)?;
            descriptor.role = Some(role);
            Ok(PlacementCommit {
                events,
                descriptor,
                previous_parent,
                parent_changed: write_row,
                role_changed,
            })
        })
        .await
    }
}

fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/role-assertion".to_string(),
        message: error.to_string(),
    }
}
