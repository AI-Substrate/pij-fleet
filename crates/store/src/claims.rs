//! Number claims and deliveries (plan 196, migration 0033).
//!
//! A claim hands out the next number for (project, kind) under one
//! `BEGIN IMMEDIATE` transaction, so concurrent claims serialise and never
//! collide. A delivery records what a merged PR delivered, once per address,
//! with one `delivered` spine event in the same transaction.

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatId};
use pij_core::wire;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, adapter_error};
use crate::role_assertion::is_prime;
use crate::spine::append_in_transaction;

/// The release reason a seat's tombstone writes.
pub const SEAT_CLOSED: &str = "seat-closed";

/// The spine event kind a recorded delivery appends.
pub const DELIVERED_EVENT: &str = "delivered";

/// Deliveries a PM's card shows, newest first.
pub const CARD_DELIVERIES: u32 = 3;

/// One number handed out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Claim {
    /// The project slug.
    pub project: String,
    /// The kind, e.g. `plan` or `migration`.
    pub kind: String,
    /// The number.
    pub number: u64,
    /// The claiming seat.
    pub seat: String,
    /// What the number is for.
    pub subject: String,
    /// When it was claimed, epoch ms.
    pub claimed_at: u64,
    /// When it was released, epoch ms; `None` while open.
    pub released_at: Option<u64>,
    /// Who released it.
    pub released_by: Option<String>,
    /// Why: `seat-closed`, `delivered #<pr>`, or the releaser's words.
    pub release_reason: Option<String>,
}

/// One claim's key: (project, kind, number).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimKey {
    /// The project slug.
    pub project: String,
    /// The kind.
    pub kind: String,
    /// The number.
    pub number: u64,
}

/// The outcome of a release request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// Released now.
    Released(Claim),
    /// No such claim.
    Unknown,
    /// Already released; nothing changed.
    AlreadyReleased(Claim),
    /// The actor is neither the claiming seat nor the prime; nothing changed.
    NotOwner(Claim),
}

/// What a merged PR delivered, as `pij delivered` records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryRecord {
    /// The project slug.
    pub project: String,
    /// The PR number.
    pub pr: u64,
    /// The merge commit.
    pub merge_commit: String,
    /// The seat that ran `pij delivered`.
    pub recorded_by: String,
    /// The PM told on its card, if any.
    pub pm: Option<String>,
    /// The delivered ddocs addresses.
    pub addresses: Vec<String>,
    /// Open claims to release because the PR's files carry their numbers.
    pub releases: Vec<(String, u64)>,
    /// When, epoch ms.
    pub at: u64,
}

/// What recording a delivery changed.
#[derive(Clone, Debug, Default)]
pub struct DeliveryCommit {
    /// Addresses recorded now.
    pub recorded: Vec<String>,
    /// Addresses an earlier call already recorded.
    pub already: Vec<String>,
    /// The PM an earlier call recorded for this PR, when it did.
    pub earlier_pm: Option<String>,
    /// Claims released now.
    pub released: Vec<Claim>,
    /// The `delivered` event with its committed sequence, when anything was
    /// recorded, ready to broadcast.
    pub events: Vec<Event>,
}

/// One delivered PR, as a PM's card shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryNote {
    /// The project slug.
    pub project: String,
    /// The PR number.
    pub pr: u64,
    /// The merge commit.
    pub merge_commit: String,
    /// The delivered addresses.
    pub addresses: Vec<String>,
    /// Who recorded it.
    pub recorded_by: String,
    /// When, epoch ms.
    pub recorded_at: u64,
}

const CLAIM_COLUMNS: &str =
    "project, kind, number, seat, subject, claimed_at, released_at, released_by, release_reason";

impl SqliteOrchestration {
    /// Hand out the next number for (project, kind): one more than the highest
    /// of every claim for it (open or released) and `floor`, the highest number
    /// already present in the project's checkout.
    pub async fn claim_number(
        &self,
        project: &str,
        kind: &str,
        seat: &SeatId,
        subject: &str,
        floor: u64,
        at: u64,
    ) -> Result<Claim> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let (project, kind, seat, subject) = (
            project.to_string(),
            kind.to_string(),
            seat.to_string(),
            subject.to_string(),
        );
        let claimed_at = sql_i64(at, "claimed_at")?;
        let floor = sql_i64(floor, "the checkout floor")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let highest: Option<i64> = sqlx::query_scalar(
                "SELECT MAX(number) FROM claims WHERE project = ?1 AND kind = ?2",
            )
            .bind(&project)
            .bind(&kind)
            .fetch_one(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let number = highest.unwrap_or(0).max(floor) + 1;
            sqlx::query(
                "INSERT INTO claims (project, kind, number, seat, subject, claimed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&project)
            .bind(&kind)
            .bind(number)
            .bind(&seat)
            .bind(&subject)
            .bind(claimed_at)
            .execute(&mut *tx)
            .await
            .map_err(adapter_error)?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(Claim {
                project,
                kind,
                number: number as u64,
                seat,
                subject,
                claimed_at: at,
                released_at: None,
                released_by: None,
                release_reason: None,
            })
        })
        .await
    }

    /// Release one open claim. Only the claiming seat, the project's prime
    /// (`project_prime`) or a designated prime may.
    pub async fn release_claim(
        &self,
        key: &ClaimKey,
        actor: &SeatId,
        project_prime: Option<&SeatId>,
        reason: &str,
        at: u64,
    ) -> Result<ReleaseOutcome> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let ClaimKey {
            project,
            kind,
            number,
        } = key.clone();
        let reason = reason.to_string();
        let actor = actor.clone();
        let prime = project_prime.cloned();
        let number = sql_i64(number, "number")?;
        let released_at = sql_i64(at, "released_at")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let row = sqlx::query(&format!(
                "SELECT {CLAIM_COLUMNS} FROM claims WHERE project = ?1 AND kind = ?2 AND number = ?3"
            ))
            .bind(&project)
            .bind(&kind)
            .bind(number)
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
            let Some(mut found) = row.as_ref().map(claim).transpose()? else {
                return Ok(ReleaseOutcome::Unknown);
            };
            if found.released_at.is_some() {
                return Ok(ReleaseOutcome::AlreadyReleased(found));
            }
            let allowed = found.seat == actor.as_str()
                || prime.as_ref() == Some(&actor)
                || is_prime(&mut tx, &actor).await?;
            if !allowed {
                return Ok(ReleaseOutcome::NotOwner(found));
            }
            release(&mut tx, &project, &kind, number, released_at, actor.as_str(), &reason).await?;
            tx.commit().await.map_err(adapter_error)?;
            found.released_at = Some(at);
            found.released_by = Some(actor.to_string());
            found.release_reason = Some(reason);
            Ok(ReleaseOutcome::Released(found))
        })
        .await
    }

    /// Every claim in `project`, open and released, by kind then number.
    pub async fn claims(&self, project: &str) -> Result<Vec<Claim>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(&format!(
            "SELECT {CLAIM_COLUMNS} FROM claims WHERE project = ?1 ORDER BY kind, number"
        ))
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        rows.iter().map(claim).collect()
    }

    /// Record a delivery, idempotently by (project, PR, address), release the
    /// named open claims, and append one `delivered` event (seat = the PM)
    /// when anything new was recorded. Hold the bus's publication boundary
    /// around this call and broadcast the returned events.
    pub async fn record_delivery_committed(
        &self,
        record: &DeliveryRecord,
    ) -> Result<DeliveryCommit> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        let record = record.clone();
        let pr = sql_i64(record.pr, "pr")?;
        let at = sql_i64(record.at, "recorded_at")?;
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let mut commit = DeliveryCommit {
                earlier_pm: sqlx::query_scalar(
                    "SELECT pm FROM deliveries WHERE project = ?1 AND pr = ?2 AND pm IS NOT NULL LIMIT 1",
                )
                .bind(&record.project)
                .bind(pr)
                .fetch_optional(&mut *tx)
                .await
                .map_err(adapter_error)?,
                ..DeliveryCommit::default()
            };
            for address in &record.addresses {
                let done = sqlx::query(
                    "INSERT INTO deliveries (project, pr, address, merge_commit, recorded_by, recorded_at, pm) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT DO NOTHING",
                )
                .bind(&record.project)
                .bind(pr)
                .bind(address)
                .bind(&record.merge_commit)
                .bind(&record.recorded_by)
                .bind(at)
                .bind(&record.pm)
                .execute(&mut *tx)
                .await
                .map_err(adapter_error)?;
                if done.rows_affected() > 0 {
                    commit.recorded.push(address.clone());
                } else {
                    commit.already.push(address.clone());
                }
            }
            let reason = format!("delivered #{}", record.pr);
            for (kind, number) in &record.releases {
                let number = sql_i64(*number, "number")?;
                let row = sqlx::query(&format!(
                    "SELECT {CLAIM_COLUMNS} FROM claims \
                     WHERE project = ?1 AND kind = ?2 AND number = ?3 AND released_at IS NULL"
                ))
                .bind(&record.project)
                .bind(kind)
                .bind(number)
                .fetch_optional(&mut *tx)
                .await
                .map_err(adapter_error)?;
                let Some(mut open) = row.as_ref().map(claim).transpose()? else {
                    continue;
                };
                release(&mut tx, &record.project, kind, number, at, &record.recorded_by, &reason).await?;
                open.released_at = Some(record.at);
                open.released_by = Some(record.recorded_by.clone());
                open.release_reason = Some(reason.clone());
                commit.released.push(open);
            }
            if !commit.recorded.is_empty() || !commit.released.is_empty() {
                let mut event = Event {
                    seq: None,
                    v: wire::EVENT_VERSION,
                    at: record.at,
                    kind: DELIVERED_EVENT.to_string(),
                    seat: record.pm.as_deref().map(SeatId::from),
                    payload: json!({
                        "project": record.project,
                        "pr": record.pr,
                        "merge_commit": record.merge_commit,
                        "addresses": commit.recorded,
                        "recorded_by": record.recorded_by,
                        "pm": record.pm,
                        "released": commit.released.iter()
                            .map(|claim| json!({"kind": claim.kind, "number": claim.number}))
                            .collect::<Vec<_>>(),
                    })
                    .to_string(),
                };
                event.seq = Some(append_in_transaction(&mut tx, &event).await?);
                commit.events.push(event);
            }
            tx.commit().await.map_err(adapter_error)?;
            Ok(commit)
        })
        .await
    }

    /// Every PR recorded as delivering `address` in `project`, oldest first.
    /// One read on `deliveries_by_address`.
    pub async fn delivered_prs(&self, project: &str, address: &str) -> Result<Vec<u64>> {
        require_current_schema(&self.pool).await?;
        let prs: Vec<i64> = sqlx::query_scalar(
            "SELECT pr FROM deliveries WHERE project = ?1 AND address = ?2 \
             ORDER BY recorded_at, pr",
        )
        .bind(project)
        .bind(address)
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        Ok(prs.into_iter().map(|pr| pr as u64).collect())
    }

    /// The newest `limit` PRs whose delivery told `pm`, each with its
    /// addresses. One indexed read on `deliveries_by_pm`.
    pub async fn deliveries_for_pm(&self, pm: &str, limit: u32) -> Result<Vec<DeliveryNote>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT project, pr, merge_commit, recorded_by, recorded_at, address FROM deliveries \
             WHERE pm = ?1 AND (project, pr) IN ( \
               SELECT project, pr FROM deliveries WHERE pm = ?1 \
               GROUP BY project, pr ORDER BY MAX(recorded_at) DESC, pr DESC LIMIT ?2) \
             ORDER BY recorded_at DESC, pr DESC, project, address",
        )
        .bind(pm)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;
        let mut notes: Vec<DeliveryNote> = Vec::new();
        for row in &rows {
            let project: String = row.try_get("project").map_err(adapter_error)?;
            let pr = row.try_get::<i64, _>("pr").map_err(adapter_error)? as u64;
            let address: String = row.try_get("address").map_err(adapter_error)?;
            if let Some(note) = notes
                .iter_mut()
                .find(|note| note.project == project && note.pr == pr)
            {
                note.addresses.push(address);
                continue;
            }
            notes.push(DeliveryNote {
                project,
                pr,
                merge_commit: row.try_get("merge_commit").map_err(adapter_error)?,
                addresses: vec![address],
                recorded_by: row.try_get("recorded_by").map_err(adapter_error)?,
                recorded_at: row
                    .try_get::<i64, _>("recorded_at")
                    .map_err(adapter_error)? as u64,
            });
        }
        Ok(notes)
    }
}

/// Release every open claim held by `seat`, in the caller's transaction: the
/// tombstone's half of AC3 (close, reap and revive all tombstone).
pub(crate) async fn release_for_seat(
    connection: &mut sqlx::SqliteConnection,
    seat: &str,
    at: i64,
) -> Result<u64> {
    Ok(sqlx::query(
        "UPDATE claims SET released_at = ?2, released_by = ?1, release_reason = ?3 \
         WHERE seat = ?1 AND released_at IS NULL",
    )
    .bind(seat)
    .bind(at)
    .bind(SEAT_CLOSED)
    .execute(&mut *connection)
    .await
    .map_err(adapter_error)?
    .rows_affected())
}

async fn release(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project: &str,
    kind: &str,
    number: i64,
    at: i64,
    by: &str,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE claims SET released_at = ?4, released_by = ?5, release_reason = ?6 \
         WHERE project = ?1 AND kind = ?2 AND number = ?3 AND released_at IS NULL",
    )
    .bind(project)
    .bind(kind)
    .bind(number)
    .bind(at)
    .bind(by)
    .bind(reason)
    .execute(&mut **tx)
    .await
    .map_err(adapter_error)?;
    Ok(())
}

fn claim(row: &sqlx::sqlite::SqliteRow) -> Result<Claim> {
    let text = |name: &str| -> Result<String> { row.try_get(name).map_err(adapter_error) };
    let opt = |name: &str| -> Result<Option<String>> { row.try_get(name).map_err(adapter_error) };
    let int = |name: &str| -> Result<Option<i64>> { row.try_get(name).map_err(adapter_error) };
    Ok(Claim {
        project: text("project")?,
        kind: text("kind")?,
        number: int("number")?.unwrap_or_default() as u64,
        seat: text("seat")?,
        subject: text("subject")?,
        claimed_at: int("claimed_at")?.unwrap_or_default() as u64,
        released_at: int("released_at")?.map(|at| at as u64),
        released_by: opt("released_by")?,
        release_reason: opt("release_reason")?,
    })
}

fn sql_i64(value: u64, what: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| PijError::Adapter {
        adapter: "store/claims".to_string(),
        message: format!("{what}={value} exceeds SQLite's signed integer range"),
    })
}
