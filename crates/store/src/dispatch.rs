//! Durable packet, receipt, acknowledgement and successful canary evidence.

use pij_core::error::Result;
use pij_core::model::SeatId;
use pij_core::orchestration::{Dispatch, DispatchAcknowledgement, DispatchCanary, DispatchState};
use sqlx::{Row, sqlite::SqliteRow};

use crate::governance::{GovernanceOutcome, invalid_record};
use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{DispatchAck, SqliteOrchestration, adapter_error, sql_i64, sql_u64};

impl SqliteOrchestration {
    /// Persist the full queued packet obligation before any transport attempt.
    /// Duplicate ids or message linkages preserve the first record.
    pub async fn create_dispatch(&self, dispatch: &Dispatch) -> Result<bool> {
        require_current_schema(&self.pool).await?;
        let valid_sha = dispatch.packet_sha256.as_deref().is_some_and(|sha| {
            sha.len() == 64
                && sha
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if dispatch.state != DispatchState::Queued
            || dispatch.delivered_at.is_some()
            || dispatch.acknowledged_at.is_some()
            || dispatch.ack.is_some()
            || dispatch.canary.is_some()
            || !valid_sha
            || dispatch.msg_id.as_deref().is_none_or(str::is_empty)
            || dispatch.id.is_empty()
            || dispatch.packet_path.is_empty()
        {
            return Err(invalid_record(
                "a new dispatch requires a queued packet, SHA-256 and message id, without receipt evidence",
            ));
        }
        let created_at = sql_i64(dispatch.created_at, "dispatch created_at")?;
        let inserted = sqlx::query(
            "INSERT INTO dispatches (id, from_seat, to_seat, packet_path, packet_sha256, msg_id, state, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7) ON CONFLICT DO NOTHING",
        )
        .bind(&dispatch.id)
        .bind(dispatch.from.as_str())
        .bind(dispatch.to.as_str())
        .bind(&dispatch.packet_path)
        .bind(&dispatch.packet_sha256)
        .bind(&dispatch.msg_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(inserted.rows_affected() == 1)
    }

    /// Read the full dispatch, including immutable acknowledgement/canary evidence.
    pub async fn dispatch(&self, id: &str) -> Result<Option<Dispatch>> {
        require_current_schema(&self.pool).await?;
        sqlx::query("SELECT * FROM dispatches WHERE id=?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .map(dispatch_row)
            .transpose()
    }

    /// Read dispatches in creation/id order, optionally restricted to one recipient.
    pub async fn list_dispatches(&self, recipient: Option<&SeatId>) -> Result<Vec<Dispatch>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "SELECT * FROM dispatches WHERE (?1 IS NULL OR to_seat=?1) ORDER BY created_at, id",
        )
        .bind(recipient.map(SeatId::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?
        .into_iter()
        .map(dispatch_row)
        .collect()
    }

    /// Record the first successful transport receipt without ever downgrading acked.
    pub async fn mark_dispatch_delivered(
        &self,
        id: &str,
        at: u64,
    ) -> Result<GovernanceOutcome<Dispatch>> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(at, "dispatch delivered_at")?;
        let pool = self.pool.clone();
        let id = id.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let existing = sqlx::query("SELECT * FROM dispatches WHERE id=?1")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(dispatch_row)
            .transpose()?;
        let outcome = match existing {
            None => GovernanceOutcome::Missing,
            Some(row) if row.delivered_at.is_some() => GovernanceOutcome::Unchanged(row),
            Some(_) => {
                let row = sqlx::query(
                    "UPDATE dispatches SET delivered_at=?2, \
                     state=CASE state WHEN 'queued' THEN 'delivered' ELSE state END WHERE id=?1 RETURNING *",
                ).bind(&id).bind(at).fetch_one(&mut *tx).await.map_err(adapter_error)?;
                GovernanceOutcome::Changed(dispatch_row(row)?)
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }

    /// Acknowledge only as the recipient and only for the persisted digest.
    /// Identity and SHA checks precede replay handling, including on migrated rows.
    pub async fn acknowledge_dispatch(
        &self,
        id: &str,
        actor: &SeatId,
        packet_sha256: &str,
        at: u64,
    ) -> Result<DispatchAck> {
        require_current_schema(&self.pool).await?;
        let at = sql_i64(at, "dispatch acknowledged_at")?;
        let pool = self.pool.clone();
        let id = id.to_owned();
        let actor = actor.clone();
        let packet_sha256 = packet_sha256.to_owned();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let existing = sqlx::query("SELECT * FROM dispatches WHERE id=?1")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(dispatch_row)
            .transpose()?;
        let outcome = match existing {
            None => DispatchAck::Missing,
            Some(row) if row.to != actor => DispatchAck::NotAssignee { assignee: row.to },
            Some(row) if row.packet_sha256.as_deref() != Some(packet_sha256.as_str()) => {
                DispatchAck::ShaMismatch
            }
            Some(row) if row.state == DispatchState::Acked => DispatchAck::AlreadyAcknowledged,
            Some(_) => {
                sqlx::query(
                    "UPDATE dispatches SET state='acked', acknowledged_at=?2, ack_seat=?3, ack_sha256=?4, ack_at=?2 WHERE id=?1",
                ).bind(&id).bind(at).bind(actor.as_str()).bind(&packet_sha256)
                    .execute(&mut *tx).await.map_err(adapter_error)?;
                DispatchAck::Acknowledged
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }

    /// Store daemon-verified canary evidence once, only after an exact packet ack.
    /// The daemon owns nonce/process/session/model verification, not this adapter.
    pub async fn set_dispatch_canary(
        &self,
        id: &str,
        canary: &DispatchCanary,
    ) -> Result<GovernanceOutcome<Dispatch>> {
        require_current_schema(&self.pool).await?;
        if canary.nonce.is_empty()
            || canary.model.is_empty()
            || canary.evaluator.as_str().is_empty()
        {
            return Err(invalid_record(
                "canary evidence requires nonce, observed model and evaluator",
            ));
        }
        let at = sql_i64(canary.passed_at, "canary passed_at")?;
        let pool = self.pool.clone();
        let id = id.to_owned();
        let canary = canary.clone();
        owned_write(async move {
        let mut tx = begin_write(&pool).await?;
        let existing = sqlx::query("SELECT * FROM dispatches WHERE id=?1")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?
            .map(dispatch_row)
            .transpose()?;
        let outcome = match existing {
            None => GovernanceOutcome::Missing,
            Some(row) if row.state != DispatchState::Acked || row.ack.is_none() => {
                GovernanceOutcome::Conflict {
                    code: "canary-unacked",
                }
            }
            Some(row) if row.canary.as_ref() == Some(&canary) => GovernanceOutcome::Unchanged(row),
            Some(row) if row.canary.is_some() => GovernanceOutcome::Conflict {
                code: "canary-conflict",
            },
            Some(row)
                if row
                    .acknowledged_at
                    .is_some_and(|ack_at| canary.passed_at < ack_at) =>
            {
                GovernanceOutcome::Conflict {
                    code: "canary-before-ack",
                }
            }
            Some(_) => {
                let row = sqlx::query(
                    "UPDATE dispatches SET canary_nonce=?2, canary_model=?3, canary_passed_at=?4, canary_evaluator=?5 \
                     WHERE id=?1 RETURNING *",
                ).bind(&id).bind(&canary.nonce).bind(&canary.model).bind(at).bind(canary.evaluator.as_str())
                    .fetch_one(&mut *tx).await.map_err(adapter_error)?;
                GovernanceOutcome::Changed(dispatch_row(row)?)
            }
        };
        tx.commit().await.map_err(adapter_error)?;
        Ok(outcome)
        }).await
    }
}

fn dispatch_row(row: SqliteRow) -> Result<Dispatch> {
    let state: String = row.try_get("state").map_err(adapter_error)?;
    let state = match state.as_str() {
        "queued" => DispatchState::Queued,
        "delivered" => DispatchState::Delivered,
        "acked" => DispatchState::Acked,
        _ => return Err(invalid_record("unknown stored dispatch state")),
    };
    let ack_seat: Option<String> = row.try_get("ack_seat").map_err(adapter_error)?;
    let ack = ack_seat
        .map(|seat| -> Result<DispatchAcknowledgement> {
            Ok(DispatchAcknowledgement {
                seat: SeatId(seat),
                packet_sha256: row.try_get("ack_sha256").map_err(adapter_error)?,
                at: sql_u64(
                    row.try_get("ack_at").map_err(adapter_error)?,
                    "dispatch ack_at",
                )?,
            })
        })
        .transpose()?;
    let nonce: Option<String> = row.try_get("canary_nonce").map_err(adapter_error)?;
    let canary = nonce
        .map(|nonce| -> Result<DispatchCanary> {
            Ok(DispatchCanary {
                nonce,
                model: row.try_get("canary_model").map_err(adapter_error)?,
                passed_at: sql_u64(
                    row.try_get("canary_passed_at").map_err(adapter_error)?,
                    "canary passed_at",
                )?,
                evaluator: SeatId(row.try_get("canary_evaluator").map_err(adapter_error)?),
            })
        })
        .transpose()?;
    let delivered_at: Option<i64> = row.try_get("delivered_at").map_err(adapter_error)?;
    let acknowledged_at: Option<i64> = row.try_get("acknowledged_at").map_err(adapter_error)?;
    Ok(Dispatch {
        id: row.try_get("id").map_err(adapter_error)?,
        from: SeatId(row.try_get("from_seat").map_err(adapter_error)?),
        to: SeatId(row.try_get("to_seat").map_err(adapter_error)?),
        packet_path: row.try_get("packet_path").map_err(adapter_error)?,
        packet_sha256: row.try_get("packet_sha256").map_err(adapter_error)?,
        msg_id: row.try_get("msg_id").map_err(adapter_error)?,
        state,
        created_at: sql_u64(
            row.try_get("created_at").map_err(adapter_error)?,
            "dispatch created_at",
        )?,
        delivered_at: delivered_at
            .map(|at| sql_u64(at, "dispatch delivered_at"))
            .transpose()?,
        acknowledged_at: acknowledged_at
            .map(|at| sql_u64(at, "dispatch acknowledged_at"))
            .transpose()?,
        ack,
        canary,
    })
}
