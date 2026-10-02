//! The seat registry, on SQLite.
//!
//! The whole point of this crate existing separately: the Registry CONTRACT is
//! `pij_testkit::contract::registry_contract`, and this implementation runs it
//! unchanged. Nothing below is allowed to have its own idea of what `get` means.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sqlx::{Pool, Row, Sqlite};

use pij_core::error::{PijError, Result};
use pij_core::model::{
    Event, Harness, ProcIdentity, SeatDescriptor, SeatId, SemanticState, Seq, SystemState,
};
use pij_core::ports::{PutBinding, Registry, SeatFilter};

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::spine::{RegistryPublisher, append_in_transaction};

/// A [`Registry`] backed by the SQLite store.
pub struct SqliteRegistry {
    pool: Pool<Sqlite>,
    publisher: Arc<dyn RegistryPublisher>,
}

impl SqliteRegistry {
    /// The store this registry actually reads, for absence messages.
    ///
    /// sqlx does not hand back the opened path, so this names the connection
    /// rather than inventing a filename — the honest form of "where I looked"
    /// when the exact file is not available to us.
    fn store_description(&self) -> String {
        format!(
            "the SQLite store this daemon opened ({} connections)",
            self.pool.size()
        )
    }
}

impl SqliteRegistry {
    /// Wrap an open pool and its daemon-owned ordered publication boundary.
    pub fn new(pool: Pool<Sqlite>, publisher: Arc<dyn RegistryPublisher>) -> Self {
        SqliteRegistry { pool, publisher }
    }

    async fn publish_tombstone(
        &self,
        seat: SeatId,
        reason: String,
        expected: Option<SeatDescriptor>,
    ) -> Result<Seq> {
        let pool = self.pool.clone();
        let store = self.store_description();
        self.publisher
            .publish_registry(Box::pin(async move {
                owned_write(async move {
                    require_current_schema(&pool).await?;
                    let mut tx = begin_write(&pool).await?;
                    if let Some(expected) = expected.as_ref() {
                        let row = sqlx::query("SELECT * FROM seats WHERE id = ?1")
                            .bind(seat.as_str())
                            .fetch_optional(&mut *tx)
                            .await
                            .map_err(adapter_error)?;
                        let current = row.as_ref().map(row_to_descriptor).transpose()?;
                        if current.as_ref() != Some(expected) {
                            tx.rollback().await.map_err(adapter_error)?;
                            return Err(PijError::GovernanceRefused {
                                code: "E-RS-INCARNATION-CHANGED".to_string(),
                                record: seat.to_string(),
                            });
                        }
                    }
                    let mut event = tombstone_event(&seat, &reason, expected.as_ref())?;
                    // Plan 158: a retired seat's held FYIs can never ride along, so
                    // they are dropped in the same transaction and the tombstone
                    // records how many. Close, reap and revive all pass through here.
                    let dropped = sqlx::query(
                        "UPDATE fyis SET state = 'dropped', settled_at_ms = ?2, \
                         settled_via = 'tombstone' WHERE recipient = ?1 AND state = 'pending'",
                    )
                    .bind(seat.as_str())
                    .bind(event.at as i64)
                    .execute(&mut *tx)
                    .await
                    .map_err(adapter_error)?
                    .rows_affected();
                    record_dropped_fyis(&mut event, dropped)?;
                    let seq = append_in_transaction(&mut tx, &event).await?;
                    // A retired seat's turn is over: `working` must not outlive it.
                    let affected = sqlx::query(
                        "UPDATE seats SET tombstoned_at = ?2, tombstone_reason = ?3, \
                 native_extension_delivery = 0, seq = ?4, \
                 state = CASE WHEN state = 'working' THEN 'idle' ELSE state END WHERE id = ?1",
                    )
                    .bind(seat.as_str())
                    .bind(event.at as i64)
                    .bind(&reason)
                    .bind(seq.0 as i64)
                    .execute(&mut *tx)
                    .await
                    .map_err(adapter_error)?
                    .rows_affected();
                    if affected == 0 {
                        // Unconditional intent keeps its original absence refusal. The
                        // guarded path already checked presence under this transaction.
                        tx.rollback().await.map_err(adapter_error)?;
                        return Err(PijError::NoRegistryEntry { seat, store });
                    }
                    tx.commit().await.map_err(adapter_error)?;
                    event.seq = Some(seq);
                    Ok((event, None))
                })
                .await
            }))
            .await
            .map(|(seq, _)| seq)
    }
}

fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/registry".to_string(),
        message: error.to_string(),
    }
}

fn row_proc(row: &sqlx::sqlite::SqliteRow) -> Result<Option<ProcIdentity>> {
    // A half identity cannot truthfully distinguish a rebind from a refresh.
    let pid: Option<i64> = row.try_get("pid").map_err(adapter_error)?;
    let proc_start: Option<i64> = row.try_get("proc_start").map_err(adapter_error)?;
    match (pid, proc_start) {
        (Some(pid), Some(proc_start)) => Ok(Some(ProcIdentity {
            pid: pid as u32,
            proc_start: proc_start as u64,
        })),
        (None, None) => Ok(None),
        _ => Err(PijError::Adapter {
            adapter: "store/registry".to_string(),
            message: "seat row has half a process identity (pid without proc_start, or \
                      the reverse) — liveness cannot be judged from it"
                .to_string(),
        }),
    }
}

pub(crate) fn row_to_descriptor(row: &sqlx::sqlite::SqliteRow) -> Result<SeatDescriptor> {
    let harness_text: String = row.try_get("harness").map_err(adapter_error)?;
    // An unrecognised harness is refused, never coerced to a default: guessing
    // here is how a seat gets bound to the wrong readiness anchor.
    let harness = Harness::parse(&harness_text).ok_or_else(|| PijError::Adapter {
        adapter: "store/registry".to_string(),
        message: format!("unknown harness `{harness_text}` in the seats table"),
    })?;

    let state_text: String = row.try_get("state").map_err(adapter_error)?;
    let state = SystemState::parse(&state_text).ok_or_else(|| PijError::Adapter {
        adapter: "store/registry".to_string(),
        message: format!("unknown system state `{state_text}` in the seats table"),
    })?;

    let semantic_text: Option<String> = row.try_get("semantic_state").map_err(adapter_error)?;
    let semantic_state = semantic_text
        .as_deref()
        .map(|word| {
            SemanticState::parse(word).ok_or_else(|| PijError::Adapter {
                adapter: "store/registry".to_string(),
                message: format!("unknown semantic state `{word}` in the seats table"),
            })
        })
        .transpose()?;

    let proc = row_proc(row)?;
    // Keep native attestation's raw bounds checks before relying on the
    // descriptor conversion used by the binding-report path.
    let pid: Option<i64> = row.try_get("pid").map_err(adapter_error)?;
    let proc_start: Option<i64> = row.try_get("proc_start").map_err(adapter_error)?;

    let relay: i64 = row.try_get("relay").map_err(adapter_error)?;
    let parent: Option<String> = row.try_get("parent").map_err(adapter_error)?;
    let native_flag: i64 = row
        .try_get("native_extension_delivery")
        .map_err(adapter_error)?;
    let native_extension_delivery = match native_flag {
        0 => false,
        1 => true,
        other => {
            return Err(PijError::Adapter {
                adapter: "store/registry".to_string(),
                message: format!(
                    "seat row has invalid native_extension_delivery {other}; expected 0 or 1"
                ),
            });
        }
    };
    let harness_session: Option<String> = row.try_get("harness_session").map_err(adapter_error)?;
    let tombstoned_at: Option<i64> = row.try_get("tombstoned_at").map_err(adapter_error)?;
    if native_extension_delivery
        && (harness != Harness::Copilot
            || !pid.is_some_and(|pid| (1..=i64::from(u32::MAX)).contains(&pid))
            || !proc_start.is_some_and(|start| start > 0)
            || !harness_session
                .as_deref()
                .is_some_and(|session| !session.is_empty())
            || tombstoned_at.is_some())
    {
        return Err(PijError::Adapter {
            adapter: "store/registry".to_string(),
            message: "native_extension_delivery requires a live Copilot process and native session"
                .to_string(),
        });
    }

    Ok(SeatDescriptor {
        id: SeatId(row.try_get::<String, _>("id").map_err(adapter_error)?),
        // Machine identity belongs to the federated view, not durable local
        // registry state. The HTTP boundary stamps the daemon's current alias.
        machine: None,
        badge: None,
        last_event_at: None,
        harness,
        harness_session,
        extension_build: row.try_get("extension_build").map_err(adapter_error)?,
        extension_path: row.try_get("extension_path").map_err(adapter_error)?,
        pane: row.try_get("pane").map_err(adapter_error)?,
        proc,
        folder: row.try_get("folder").map_err(adapter_error)?,
        state,
        semantic_state,
        role: row.try_get("role").map_err(adapter_error)?,
        parent: parent.map(SeatId),
        relay: relay != 0,
        tombstoned_at: tombstoned_at.map(|at| at as u64),
        tombstone_reason: row.try_get("tombstone_reason").map_err(adapter_error)?,
        spawn_id: row.try_get("spawn_id").map_err(adapter_error)?,
        model: row.try_get("model").map_err(adapter_error)?,
        provider: row.try_get("provider").map_err(adapter_error)?,
        effort: row.try_get("effort").map_err(adapter_error)?,
        cross_session_inbound_accept: row
            .try_get::<Option<i64>, _>("cross_session_inbound_accept")
            .map_err(adapter_error)?
            .map(|flag| flag != 0),
        native_extension_delivery,
    })
}

/// Construct a stamped registry fact without assigning its durable sequence.
/// Fake and SQLite registry publication use the same payload/clock encoding.
pub fn registry_event(kind: &str, seat: &SeatId, payload: String) -> Result<Event> {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: "store/registry".to_string(),
            message: format!("cannot publish registry event before the Unix epoch: {error}"),
        })?
        .as_millis();
    let at = u64::try_from(at).map_err(|error| PijError::Adapter {
        adapter: "store/registry".to_string(),
        message: format!("registry timestamp exceeds epoch-millisecond range: {error}"),
    })?;
    Ok(Event {
        seq: None,
        v: 1,
        at,
        kind: kind.to_string(),
        seat: Some(seat.clone()),
        payload,
    })
}

/// Encode a seat's own busy/idle observation as a `seat.activity` fact (plan 158).
pub fn activity_event(
    seat: &SeatId,
    state: pij_core::model::SystemState,
    reason: Option<&str>,
) -> Result<Event> {
    let mut payload = serde_json::json!({ "state": state.as_str() });
    if let Some(reason) = reason {
        payload["reason"] = reason.into();
    }
    registry_event("seat.activity", seat, payload.to_string())
}

/// Stamp `pending_fyis_dropped` onto a `seat.tombstone` payload (plan 158).
///
/// Only when something was dropped: a seat with nothing held keeps the canonical
/// payload, identical to the fake registry's.
fn record_dropped_fyis(event: &mut Event, dropped: u64) -> Result<()> {
    if dropped == 0 {
        return Ok(());
    }
    let mut payload: serde_json::Value =
        serde_json::from_str(&event.payload).map_err(|error| PijError::Adapter {
            adapter: "store/registry".to_string(),
            message: format!("could not re-encode the tombstone payload: {error}"),
        })?;
    payload["pending_fyis_dropped"] = dropped.into();
    event.payload = payload.to_string();
    Ok(())
}

/// Encode a retirement fact; observed-dead reasons carry the compared binding.
/// The caller owns the two-witness observation; the store owns its atomic commit.
pub fn tombstone_event(
    seat: &SeatId,
    reason: &str,
    expected: Option<&SeatDescriptor>,
) -> Result<Event> {
    let mut payload = serde_json::json!({ "reason": reason });
    if matches!(
        reason,
        "observed-dead" | "revive-observed-dead" | "revive-assumed-dead"
    ) && let Some(expected) = expected
    {
        payload["observation"] = serde_json::json!({
            "pid": expected.proc.map(|proc| proc.pid),
            "proc_start": expected.proc.map(|proc| proc.proc_start),
            "pane": expected.pane,
            "pane_present": false,
        });
    }
    registry_event("seat.tombstone", seat, payload.to_string())
}

/// Encode the exact descriptor in a stamped `seat.put` fact.
pub fn descriptor_event(descriptor: &SeatDescriptor) -> Result<Event> {
    let payload = serde_json::to_string(descriptor).map_err(|error| PijError::Adapter {
        adapter: "store/registry".to_string(),
        message: format!("could not encode registry descriptor: {error}"),
    })?;
    registry_event("seat.put", &descriptor.id, payload)
}

#[async_trait]
impl Registry for SqliteRegistry {
    async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
        require_current_schema(&self.pool).await?;
        let row = sqlx::query("SELECT * FROM seats WHERE id = ?1")
            .bind(seat.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?;
        row.as_ref().map(row_to_descriptor).transpose()
    }

    async fn put(&self, descriptor: SeatDescriptor) -> Result<Seq> {
        self.put_reporting(descriptor).await.map(|(seq, _)| seq)
    }

    async fn put_reporting(&self, descriptor: SeatDescriptor) -> Result<(Seq, PutBinding)> {
        let pool = self.pool.clone();
        let (seq, binding) = self.publisher.publish_registry(Box::pin(async move {
        owned_write(async move {
        require_current_schema(&pool).await?;

        // The future is first polled under EventBus's publication lock. Acquire
        // SQLite only afterwards, so ordinary and registry writers cannot invert
        // their locks. Failure rolls back both the descriptor and its event.
        let mut descriptor = descriptor;
        descriptor.machine = None;
        let mut event = descriptor_event(&descriptor)?;
        let mut tx = begin_write(&pool).await?;
        let previous = sqlx::query("SELECT pid, proc_start FROM seats WHERE id = ?1")
            .bind(descriptor.id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(adapter_error)?;
        let binding = PutBinding {
            inserted: previous.is_none(),
            previous_proc: previous.as_ref().map(row_proc).transpose()?.flatten(),
        };

        let seq = append_in_transaction(&mut tx, &event).await?;

        sqlx::query(
            "INSERT INTO seats (id, harness, harness_session, pane, pid, proc_start, folder, state, \
             semantic_state, role, parent, relay, tombstoned_at, tombstone_reason, seq, \
             spawn_id, model, provider, effort, cross_session_inbound_accept, native_extension_delivery, \
             extension_build, extension_path) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
             ?17, ?18, ?19, ?20, ?21, ?22, ?23) \
             ON CONFLICT(id) DO UPDATE SET \
             harness=excluded.harness, harness_session=excluded.harness_session, \
             pane=excluded.pane, pid=excluded.pid, proc_start=excluded.proc_start, \
             folder=excluded.folder, state=excluded.state, \
             semantic_state=excluded.semantic_state, role=excluded.role, \
             parent=excluded.parent, relay=excluded.relay, \
             tombstoned_at=excluded.tombstoned_at, \
             tombstone_reason=excluded.tombstone_reason, seq=excluded.seq, \
             spawn_id=excluded.spawn_id, model=excluded.model, \
             provider=excluded.provider, effort=excluded.effort, \
             cross_session_inbound_accept=excluded.cross_session_inbound_accept, \
             native_extension_delivery=excluded.native_extension_delivery, \
             extension_build=excluded.extension_build, extension_path=excluded.extension_path",
        )
        .bind(descriptor.id.as_str())
        .bind(descriptor.harness.as_str())
        .bind(descriptor.harness_session.as_deref())
        .bind(descriptor.pane.as_deref())
        .bind(descriptor.proc.map(|p| i64::from(p.pid)))
        .bind(descriptor.proc.map(|p| p.proc_start as i64))
        .bind(&descriptor.folder)
        .bind(descriptor.state.as_str())
        .bind(descriptor.semantic_state.map(SemanticState::as_str))
        .bind(descriptor.role.as_deref())
        .bind(descriptor.parent.as_ref().map(SeatId::as_str))
        .bind(i64::from(descriptor.relay))
        .bind(descriptor.tombstoned_at.map(|value| value as i64))
        .bind(descriptor.tombstone_reason.as_deref())
        .bind(seq.0 as i64)
        .bind(descriptor.spawn_id.as_deref())
        .bind(descriptor.model.as_deref())
        .bind(descriptor.provider.as_deref())
        .bind(descriptor.effort.as_deref())
        .bind(descriptor.cross_session_inbound_accept.map(i64::from))
        .bind(i64::from(descriptor.native_extension_delivery))
        .bind(descriptor.extension_build.as_deref())
        .bind(descriptor.extension_path.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(adapter_error)?;

        tx.commit().await.map_err(adapter_error)?;
        event.seq = Some(seq);
        Ok((event, Some(binding)))
        }).await
        })).await?;
        let binding = binding.ok_or_else(|| PijError::Adapter {
            adapter: "store/registry".to_string(),
            message: "registry publisher lost the committed put binding".to_string(),
        })?;
        Ok((seq, binding))
    }

    async fn list(&self, filter: SeatFilter) -> Result<Vec<SeatDescriptor>> {
        require_current_schema(&self.pool).await?;
        // Absent filter fields mean "do not filter" — expressed as an IS NULL
        // guard per field rather than by building SQL strings, so an empty
        // string can never be mistaken for "no filter".
        let rows = sqlx::query(
            "SELECT * FROM seats \
             WHERE (?1 IS NULL OR harness = ?1) \
               AND (?2 IS NULL OR folder = ?2) \
               AND (?3 IS NULL OR parent = ?3) \
             ORDER BY id",
        )
        .bind(filter.harness.map(Harness::as_str))
        .bind(filter.folder.as_deref())
        .bind(filter.parent.as_ref().map(SeatId::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;

        rows.iter().map(row_to_descriptor).collect()
    }

    async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<Seq> {
        self.publish_tombstone(seat.clone(), reason.to_string(), None)
            .await
    }

    async fn tombstone_if_unchanged(
        &self,
        expected: SeatDescriptor,
        reason: String,
    ) -> Result<Seq> {
        self.publish_tombstone(expected.id.clone(), reason, Some(expected))
            .await
    }

    async fn set_activity(
        &self,
        seat: &SeatId,
        state: pij_core::model::SystemState,
        reason: Option<&str>,
    ) -> Result<Option<Seq>> {
        // An unchanged or retired seat publishes nothing.
        match self.get(seat).await? {
            Some(current) if current.tombstoned_at.is_none() && current.state != state => {}
            _ => return Ok(None),
        }
        let pool = self.pool.clone();
        let seat = seat.clone();
        let reason = reason.map(str::to_string);
        let published = self
            .publisher
            .publish_registry(Box::pin(async move {
                owned_write(async move {
                    require_current_schema(&pool).await?;
                    let mut tx = begin_write(&pool).await?;
                    let mut event = activity_event(&seat, state, reason.as_deref())?;
                    let seq = append_in_transaction(&mut tx, &event).await?;
                    // Only the mechanical state, and only on a live row: a
                    // concurrent tombstone, role or declaration is never overwritten.
                    let affected = sqlx::query(
                        "UPDATE seats SET state = ?2, seq = ?3 \
                         WHERE id = ?1 AND tombstoned_at IS NULL",
                    )
                    .bind(seat.as_str())
                    .bind(state.as_str())
                    .bind(seq.0 as i64)
                    .execute(&mut *tx)
                    .await
                    .map_err(adapter_error)?
                    .rows_affected();
                    if affected == 0 {
                        tx.rollback().await.map_err(adapter_error)?;
                        return Err(PijError::SeatIsGone {
                            seat,
                            tombstone_reason: None,
                        });
                    }
                    tx.commit().await.map_err(adapter_error)?;
                    event.seq = Some(seq);
                    Ok((event, None))
                })
                .await
            }))
            .await;
        match published {
            Ok((seq, _)) => Ok(Some(seq)),
            Err(PijError::SeatIsGone { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
