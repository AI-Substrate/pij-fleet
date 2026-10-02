//! Atomic role assertions on the existing orchestration store.

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatId};
use pij_core::wire;
use serde_json::json;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::SqliteOrchestration;
use crate::spine::append_in_transaction;

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
        if role.is_some_and(|value| value.trim().is_empty()) {
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
        let mut event = Event {
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
        };
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
}

fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/role-assertion".to_string(),
        message: error.to_string(),
    }
}
