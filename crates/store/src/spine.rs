//! The append-only history, on SQLite.
//!
//! Committed rows are never updated. `seq` is an AUTOINCREMENT primary key,
//! so it is monotonic for the life of the database and is never reused even
//! after deletion — which is what lets `tail(since)` mean "everything I have not
//! seen" rather than "everything currently present".

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use sqlx::sqlite::SqliteRow;
use sqlx::{Pool, QueryBuilder, Row, Sqlite, SqliteConnection};

use pij_core::error::{PijError, Result};
use pij_core::model::{Event, SeatId, Seq};
use pij_core::ports::{PutBinding, Spine};

use crate::migrate::{begin_write, owned_write, require_current_schema};
use crate::orchestration::{SqliteOrchestration, sql_u64};

/// An owned, lazy registry transaction returning its committed event and binding.
/// The publisher acquires its ordering lock before polling, then owns this future
/// independently of the requesting handler so cancellation cannot strand a commit.
pub type RegistryCommit =
    Pin<Box<dyn Future<Output = Result<(Event, Option<PutBinding>)>> + Send + 'static>>;

/// A registry write's public sequence and optional binding result.
pub type RegistryPublication<'a> =
    Pin<Box<dyn Future<Output = Result<(Seq, Option<PutBinding>)>> + Send + 'a>>;

/// Store-internal seam for atomic registry commits and ordered live publication.
///
/// The daemon's EventBus implements this alongside the existing Spine port.
/// This is not a new core port: the callback owns the SQLite transaction, while
/// the bus owns ordering and broadcasts only its successfully committed event.
pub trait RegistryPublisher: Send + Sync {
    /// Hold the shared publication lock while committing and broadcasting.
    fn publish_registry<'a>(&'a self, commit: RegistryCommit) -> RegistryPublication<'a>;
}

/// The sole SQL insertion site, shared by ordinary and atomic registry writes.
pub(crate) async fn append_in_transaction(
    connection: &mut SqliteConnection,
    event: &Event,
) -> Result<Seq> {
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO spine_events (v, at, kind, seat, payload) \
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING seq",
    )
    .bind(i64::from(event.v))
    .bind(event.at as i64)
    .bind(&event.kind)
    .bind(event.seat.as_ref().map(SeatId::as_str))
    .bind(&event.payload)
    .fetch_one(connection)
    .await
    .map_err(adapter_error)?;
    Ok(Seq(seq as u64))
}

/// Allocate through the sole insertion authority, then finalize a self-referential
/// payload before the caller commits its transaction. The draft is never visible
/// outside that transaction; any failure must roll back the enclosing operation.
pub(crate) async fn append_generated_in_transaction(
    connection: &mut SqliteConnection,
    mut draft: Event,
    finalize_payload: impl FnOnce(Seq) -> String,
) -> Result<Event> {
    let seq = append_in_transaction(connection, &draft).await?;
    draft.payload = finalize_payload(seq);
    sqlx::query("UPDATE spine_events SET payload = ?1 WHERE seq = ?2")
        .bind(&draft.payload)
        .bind(seq.0 as i64)
        .execute(connection)
        .await
        .map_err(adapter_error)?;
    draft.seq = Some(seq);
    Ok(draft)
}

/// A [`Spine`] backed by the SQLite store.
pub struct SqliteSpine {
    pool: Pool<Sqlite>,
}

impl SqliteSpine {
    /// Wrap an open pool.
    pub fn new(pool: Pool<Sqlite>) -> Self {
        SqliteSpine { pool }
    }
}

impl SqliteOrchestration {
    /// Observe the committed head without decoding or copying event history.
    /// This cursor is never used to allocate a future event sequence.
    pub async fn spine_head(&self) -> Result<Seq> {
        require_current_schema(&self.pool).await?;
        let seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM spine_events")
            .fetch_one(&self.pool)
            .await
            .map_err(adapter_error)?;
        Ok(Seq(sql_u64(seq, "spine head")?))
    }
}

fn adapter_error(error: sqlx::Error) -> PijError {
    PijError::Adapter {
        adapter: "store/spine".to_string(),
        message: error.to_string(),
    }
}

fn decode_event(row: &SqliteRow) -> Result<Event> {
    Ok(Event {
        seq: Some(Seq(
            row.try_get::<i64, _>("seq").map_err(adapter_error)? as u64
        )),
        v: row.try_get::<i64, _>("v").map_err(adapter_error)? as u32,
        at: row.try_get::<i64, _>("at").map_err(adapter_error)? as u64,
        kind: row.try_get("kind").map_err(adapter_error)?,
        seat: row
            .try_get::<Option<String>, _>("seat")
            .map_err(adapter_error)?
            .map(SeatId),
        payload: row.try_get("payload").map_err(adapter_error)?,
    })
}

#[async_trait]
impl Spine for SqliteSpine {
    async fn append(&self, event: Event) -> Result<Seq> {
        require_current_schema(&self.pool).await?;
        let pool = self.pool.clone();
        owned_write(async move {
            let mut tx = begin_write(&pool).await?;
            let seq = append_in_transaction(&mut tx, &event).await?;
            tx.commit().await.map_err(adapter_error)?;
            Ok(seq)
        })
        .await
    }

    async fn tail(&self, seat: Option<&SeatId>, since: Seq) -> Result<Vec<Event>> {
        require_current_schema(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT seq, v, at, kind, seat, payload FROM spine_events \
             WHERE seq > ?1 AND (?2 IS NULL OR seat = ?2) ORDER BY seq",
        )
        .bind(since.0 as i64)
        .bind(seat.map(SeatId::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(adapter_error)?;

        rows.iter().map(decode_event).collect()
    }

    async fn latest_matching(&self, seat: &SeatId, kinds: &[&str]) -> Result<Option<Event>> {
        if kinds.is_empty() {
            return Err(PijError::Adapter {
                adapter: "store/spine".to_string(),
                message: "latest_matching requires at least one event kind".to_string(),
            });
        }
        require_current_schema(&self.pool).await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT seq, v, at, kind, seat, payload FROM spine_events WHERE seat = ",
        );
        query.push_bind(seat.as_str());
        query.push(" AND kind IN (");
        {
            let mut kinds_sql = query.separated(", ");
            for kind in kinds {
                kinds_sql.push_bind(*kind);
            }
            kinds_sql.push_unseparated(")");
        }
        query.push(" ORDER BY seq DESC LIMIT 1");
        query
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_err(adapter_error)?
            .as_ref()
            .map(decode_event)
            .transpose()
    }

    async fn latest_matching_message(
        &self,
        seat: &SeatId,
        kind: &str,
        msg_id: &str,
    ) -> Result<Option<Event>> {
        require_current_schema(&self.pool).await?;
        sqlx::query(
            "SELECT seq, v, at, kind, seat, payload FROM spine_events \
             WHERE seat = ?1 AND kind = ?2 \
             AND json_extract(payload, '$.msg_id') = ?3 \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(seat.as_str())
        .bind(kind)
        .bind(msg_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(adapter_error)?
        .as_ref()
        .map(decode_event)
        .transpose()
    }
}
