//! Seat capability rows (plan 223 AC2), inside the registry's transactions.
//!
//! Every function here takes the caller's connection: issuance, consumption
//! and revocation commit or roll back with the seat write they belong to.

use pij_core::capability::{
    CapabilityRow, CapabilityVia, Digest, NonceRecord, SECRET_LEN, SpawnNonce,
};
use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId};
use sqlx::{Row, SqliteConnection};

fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/capability".to_string(),
        message: error.to_string(),
    }
}

fn corrupt(message: String) -> PijError {
    PijError::Adapter {
        adapter: "store/capability".to_string(),
        message,
    }
}

fn digest(bytes: Vec<u8>, column: &str) -> Result<Digest> {
    Digest::try_from(bytes.as_slice()).map_err(|_| {
        corrupt(format!(
            "{column} holds {} bytes, not a {SECRET_LEN}-byte digest",
            bytes.len()
        ))
    })
}

/// Epoch milliseconds as SQLite's integer, saturating: a far-future expiry
/// must never wrap into the past.
fn sql_ms(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn start_text(proc: Option<ProcIdentity>) -> Option<String> {
    proc.map(|proc| proc.proc_start.to_string())
}

fn identity(pid: Option<i64>, start: Option<String>, column: &str) -> Result<Option<ProcIdentity>> {
    match (pid, start) {
        (None, None) => Ok(None),
        (Some(pid), Some(start)) => {
            let pid = u32::try_from(pid)
                .map_err(|_| corrupt(format!("{column}_pid {pid} is not a process id")))?;
            let proc_start = start
                .parse()
                .map_err(|_| corrupt(format!("{column}_start `{start}` is not a process start")))?;
            Ok(Some(ProcIdentity { pid, proc_start }))
        }
        _ => Err(corrupt(format!(
            "seat capability has half a {column} identity"
        ))),
    }
}

fn row_to_capability(row: &sqlx::sqlite::SqliteRow) -> Result<CapabilityRow> {
    let via: String = row.try_get("via").map_err(adapter_error)?;
    Ok(CapabilityRow {
        seat: SeatId(row.try_get("seat").map_err(adapter_error)?),
        owner: identity(
            row.try_get("owner_pid").map_err(adapter_error)?,
            row.try_get("owner_start").map_err(adapter_error)?,
            "owner",
        )?,
        boundary: identity(
            row.try_get("boundary_pid").map_err(adapter_error)?,
            row.try_get("boundary_start").map_err(adapter_error)?,
            "boundary",
        )?,
        session: row.try_get("session").map_err(adapter_error)?,
        digest: digest(row.try_get("digest").map_err(adapter_error)?, "digest")?,
        prev_digest: row
            .try_get::<Option<Vec<u8>>, _>("prev_digest")
            .map_err(adapter_error)?
            .map(|bytes| digest(bytes, "prev_digest"))
            .transpose()?,
        issued_at: row.try_get::<i64, _>("issued_at").map_err(adapter_error)? as u64,
        via: CapabilityVia::parse(&via)
            .ok_or_else(|| corrupt(format!("unknown capability via `{via}`")))?,
    })
}

/// The seat's capability row, if any.
pub(crate) async fn capability(
    connection: &mut SqliteConnection,
    seat: &SeatId,
) -> Result<Option<CapabilityRow>> {
    sqlx::query("SELECT * FROM seat_capabilities WHERE seat = ?1")
        .bind(seat.as_str())
        .fetch_optional(connection)
        .await
        .map_err(adapter_error)?
        .as_ref()
        .map(row_to_capability)
        .transpose()
}

/// Every capability row, ordered by seat.
pub(crate) async fn capabilities(connection: &mut SqliteConnection) -> Result<Vec<CapabilityRow>> {
    sqlx::query("SELECT * FROM seat_capabilities ORDER BY seat")
        .fetch_all(connection)
        .await
        .map_err(adapter_error)?
        .iter()
        .map(row_to_capability)
        .collect()
}

/// A spawn's nonce record, if any.
pub(crate) async fn nonce(
    connection: &mut SqliteConnection,
    spawn_id: &str,
) -> Result<Option<NonceRecord>> {
    let Some(row) = sqlx::query("SELECT * FROM seat_nonces WHERE spawn_id = ?1")
        .bind(spawn_id)
        .fetch_optional(connection)
        .await
        .map_err(adapter_error)?
    else {
        return Ok(None);
    };
    let harness: String = row.try_get("harness").map_err(adapter_error)?;
    Ok(Some(NonceRecord {
        nonce: SpawnNonce {
            spawn_id: row.try_get("spawn_id").map_err(adapter_error)?,
            seat: SeatId(row.try_get("seat").map_err(adapter_error)?),
            harness: Harness::parse(&harness)
                .ok_or_else(|| corrupt(format!("unknown harness `{harness}` on a nonce")))?,
            digest: digest(row.try_get("digest").map_err(adapter_error)?, "digest")?,
            issued_at: row.try_get::<i64, _>("issued_at").map_err(adapter_error)? as u64,
            expires_at: row.try_get::<i64, _>("expires_at").map_err(adapter_error)? as u64,
            pane: identity(
                row.try_get("pane_pid").map_err(adapter_error)?,
                row.try_get("pane_start").map_err(adapter_error)?,
                "pane",
            )?,
        },
        consumed_at: row
            .try_get::<Option<i64>, _>("consumed_at")
            .map_err(adapter_error)?
            .map(|at| at as u64),
    }))
}

/// Drop `spawn_id`'s row if its window closed before `now_ms`.
pub(crate) async fn drop_expired_nonce(
    connection: &mut SqliteConnection,
    spawn_id: &str,
    now_ms: u64,
) -> Result<()> {
    sqlx::query("DELETE FROM seat_nonces WHERE spawn_id = ?1 AND expires_at <= ?2")
        .bind(spawn_id)
        .bind(sql_ms(now_ms))
        .execute(connection)
        .await
        .map_err(adapter_error)?;
    Ok(())
}

/// Record a nonce. A live row for the same spawn id is a primary-key refusal.
pub(crate) async fn insert_nonce(
    connection: &mut SqliteConnection,
    nonce: &SpawnNonce,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO seat_nonces (spawn_id, seat, harness, digest, issued_at, expires_at, \
         pane_pid, pane_start) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&nonce.spawn_id)
    .bind(nonce.seat.as_str())
    .bind(nonce.harness.as_str())
    .bind(nonce.digest.as_slice())
    .bind(sql_ms(nonce.issued_at))
    .bind(sql_ms(nonce.expires_at))
    .bind(nonce.pane.map(|pane| i64::from(pane.pid)))
    .bind(start_text(nonce.pane))
    .execute(connection)
    .await
    .map_err(adapter_error)?;
    Ok(())
}

/// Record the spawn's pane process on its nonce; `false` without one.
pub(crate) async fn record_pane(
    connection: &mut SqliteConnection,
    spawn_id: &str,
    pane: ProcIdentity,
) -> Result<bool> {
    Ok(
        sqlx::query("UPDATE seat_nonces SET pane_pid = ?2, pane_start = ?3 WHERE spawn_id = ?1")
            .bind(spawn_id)
            .bind(i64::from(pane.pid))
            .bind(pane.proc_start.to_string())
            .execute(connection)
            .await
            .map_err(adapter_error)?
            .rows_affected()
            == 1,
    )
}

/// Drop every nonce whose window closed before `now_ms`; their spawn ids.
pub(crate) async fn drop_expired_nonces(
    connection: &mut SqliteConnection,
    now_ms: u64,
) -> Result<Vec<String>> {
    sqlx::query_scalar("DELETE FROM seat_nonces WHERE expires_at <= ?1 RETURNING spawn_id")
        .bind(sql_ms(now_ms))
        .fetch_all(connection)
        .await
        .map_err(adapter_error)
}

/// Mark a checked nonce consumed. `false` if another registration got there first.
pub(crate) async fn consume_nonce(
    connection: &mut SqliteConnection,
    spawn_id: &str,
    now_ms: u64,
) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE seat_nonces SET consumed_at = ?2 WHERE spawn_id = ?1 AND consumed_at IS NULL",
    )
    .bind(spawn_id)
    .bind(sql_ms(now_ms))
    .execute(connection)
    .await
    .map_err(adapter_error)?
    .rows_affected()
        == 1)
}

/// Insert or replace the seat's capability.
pub(crate) async fn put_capability(
    connection: &mut SqliteConnection,
    row: &CapabilityRow,
) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO seat_capabilities (seat, owner_pid, owner_start, boundary_pid, \
         boundary_start, session, digest, prev_digest, issued_at, via) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(row.seat.as_str())
    .bind(row.owner.map(|proc| i64::from(proc.pid)))
    .bind(start_text(row.owner))
    .bind(row.boundary.map(|proc| i64::from(proc.pid)))
    .bind(start_text(row.boundary))
    .bind(row.session.as_deref())
    .bind(row.digest.as_slice())
    .bind(row.prev_digest.as_ref().map(|digest| digest.as_slice()))
    .bind(sql_ms(row.issued_at))
    .bind(row.via.as_str())
    .execute(connection)
    .await
    .map_err(adapter_error)?;
    Ok(())
}

/// Revoke as [`pij_core::capability::revoked_by`] does: the written seat is
/// tombstoned, or its process is not the capability's boundary.
pub(crate) async fn revoke_for_write(
    connection: &mut SqliteConnection,
    descriptor: &SeatDescriptor,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM seat_capabilities WHERE seat = ?1 \
         AND (?2 IS NOT NULL OR boundary_pid IS NOT ?3 OR boundary_start IS NOT ?4)",
    )
    .bind(descriptor.id.as_str())
    .bind(descriptor.tombstoned_at.map(|at| at as i64))
    .bind(descriptor.proc.map(|proc| i64::from(proc.pid)))
    .bind(start_text(descriptor.proc))
    .execute(connection)
    .await
    .map_err(adapter_error)?;
    Ok(())
}

/// Revoke unconditionally: the seat is being tombstoned.
pub(crate) async fn revoke(connection: &mut SqliteConnection, seat: &SeatId) -> Result<()> {
    sqlx::query("DELETE FROM seat_capabilities WHERE seat = ?1")
        .bind(seat.as_str())
        .execute(connection)
        .await
        .map_err(adapter_error)?;
    Ok(())
}

/// Clear the previous digest once `digest`, the current one, is presented.
pub(crate) async fn confirm_rotation(
    connection: &mut SqliteConnection,
    seat: &SeatId,
    digest: &Digest,
) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE seat_capabilities SET prev_digest = NULL \
         WHERE seat = ?1 AND digest = ?2 AND prev_digest IS NOT NULL",
    )
    .bind(seat.as_str())
    .bind(digest.as_slice())
    .execute(connection)
    .await
    .map_err(adapter_error)?
    .rows_affected()
        == 1)
}
