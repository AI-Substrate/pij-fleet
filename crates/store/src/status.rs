//! Read-only status facts from one SQLite snapshot. No process probes or row fan-out.

use pij_core::error::{PijError, Result};
use pij_core::model::{SeatDescriptor, SeatId, SemanticState};
use pij_core::ports::{Registry, SeatFilter};
use pij_core::status::badge_of;
use sqlx::{Row, sqlite::SqliteRow};

use crate::migrate::{
    STORE_WAIT_TIMEOUT, StorePool, begin_write, owned_write, require_current_schema,
};
use crate::registry::row_to_descriptor;

// Both sources use the same indexed joins. The adapter source is for in-memory
// fake/mixed registry configurations; the production source reads seats directly.
macro_rules! status_query {
    ($source:literal) => {
        concat!(
            "SELECT seats.*, ",
            "(SELECT at FROM spine_events WHERE seat=seats.id ORDER BY seq DESC LIMIT 1) AS last_event_at, ",
            "(SELECT json_extract(payload,'$.assignment_id') FROM spine_events ",
            " WHERE seat=seats.id AND kind='report.state' ORDER BY seq DESC LIMIT 1) AS latest_assignment_id, ",
            "(SELECT json_group_array(json_extract(event.payload,'$.state')) ",
            " FROM task_assignments AS assignment JOIN spine_events AS event ON event.seq=(",
            " SELECT seq FROM spine_events WHERE seat=seats.id AND kind='report.state' ",
            " AND json_extract(payload,'$.assignment_id')=assignment.id ORDER BY seq DESC LIMIT 1)",
            " WHERE assignment.node_id=seats.id AND assignment.closed_at IS NULL ",
            " AND json_extract(event.payload,'$.state') IS NOT NULL) AS assignment_states ",
            "FROM (", $source, ") AS seats WHERE (?2 IS NULL OR seats.id=?2) ORDER BY seats.id"
        )
    };
}

const TABLE_QUERY: &str = status_query!("SELECT *, NULL AS descriptor FROM seats");
const ADAPTER_QUERY: &str = status_query!(
    "SELECT value AS descriptor, json_extract(value,'$.id') AS id FROM json_each(?1)"
);

/// A row and the declaration/freshness facts selected in its SQL snapshot.
pub struct SeatStatus {
    /// Registry facts, never a live process observation.
    pub seat: SeatDescriptor,
    /// Timestamp of the highest-sequence event for this seat; no event means null.
    pub last_event_at: Option<u64>,
    /// Latest declarations for open assignments, plus any unscoped declaration.
    pub semantic_states: Vec<SemanticState>,
}

impl SeatStatus {
    /// One badge definition for roster and card; liveness never participates.
    pub fn badge(&self) -> &'static str {
        badge_of(Some(self.seat.state), &self.semantic_states)
    }

    /// Attach only read-projection metadata, also retained by federated rosters.
    pub fn into_projection(mut self) -> SeatDescriptor {
        self.seat.badge = Some(self.badge().to_string());
        self.seat.last_event_at = self.last_event_at;
        self.seat
    }
}

/// Status query over the same pool used by registry, assignments and event spine.
#[derive(Clone)]
pub struct SqliteStatus {
    pool: StorePool,
    registry_in_pool: bool,
}

impl SqliteStatus {
    /// `registry_in_pool` is true when registry and spine share this SQL store.
    /// Fake/mixed configurations supply their registry rows as one JSON batch.
    pub fn new(pool: StorePool, registry_in_pool: bool) -> Self {
        Self {
            pool,
            registry_in_pool,
        }
    }

    /// Prove schema access and write-lock availability without changing rows.
    ///
    /// # Errors
    /// A bounded unhealthy result if the pool, schema or writer is unavailable.
    pub async fn check_health(&self) -> Result<()> {
        let pool = self.pool.clone();
        tokio::time::timeout(
            STORE_WAIT_TIMEOUT,
            owned_write(async move {
                require_current_schema(&pool).await?;
                let tx = begin_write(&pool).await?;
                tx.rollback().await.map_err(error)
            }),
        )
        .await
        .map_err(|_| {
            error(format!(
                "store health deadline exceeded ({}ms)",
                STORE_WAIT_TIMEOUT.as_millis()
            ))
        })?
    }

    /// Read all rows or one id; one SQL snapshot supplies every status field.
    ///
    /// # Errors
    /// Schema, registry, SQL and malformed durable state errors are surfaced.
    pub async fn read(
        &self,
        registry: &dyn Registry,
        id: Option<&SeatId>,
    ) -> Result<Vec<SeatStatus>> {
        require_current_schema(&self.pool).await?;
        let (query, descriptors) = if self.registry_in_pool {
            (TABLE_QUERY, None)
        } else {
            let seats = match id {
                Some(id) => registry.get(id).await?.into_iter().collect(),
                None => registry.list(SeatFilter::default()).await?,
            };
            (
                ADAPTER_QUERY,
                Some(serde_json::to_string(&seats).map_err(error)?),
            )
        };
        let rows = sqlx::query(query)
            .bind(descriptors)
            .bind(id.map(SeatId::as_str))
            .fetch_all(&self.pool)
            .await
            .map_err(error)?;
        rows.iter().map(decode).collect()
    }
}

fn decode(row: &SqliteRow) -> Result<SeatStatus> {
    let descriptor: Option<String> = row.try_get("descriptor").map_err(error)?;
    let seat: SeatDescriptor = match descriptor {
        Some(json) => serde_json::from_str(&json).map_err(error)?,
        None => row_to_descriptor(row)?,
    };
    let last_event_at: Option<i64> = row.try_get("last_event_at").map_err(error)?;
    let latest_assignment_id: Option<String> =
        row.try_get("latest_assignment_id").map_err(error)?;
    let states: String = row.try_get("assignment_states").map_err(error)?;
    let mut semantic_states: Vec<SemanticState> = serde_json::from_str(&states).map_err(error)?;
    // A scoped declaration must not survive closing its assignment simply
    // because the legacy single-state descriptor still contains that word.
    if latest_assignment_id.is_none() {
        semantic_states.extend(seat.semantic_state);
    }
    Ok(SeatStatus {
        seat,
        last_event_at: last_event_at
            .map(u64::try_from)
            .transpose()
            .map_err(error)?,
        semantic_states,
    })
}

fn error(cause: impl std::fmt::Display) -> PijError {
    PijError::Adapter {
        adapter: "store/status".into(),
        message: cause.to_string(),
    }
}
