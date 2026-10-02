//! pij's half of `pij fleet-report` (plan 162): every seat incarnation and
//! every harness session it was bound to, read from a store **read-only**.
//!
//! The report runs beside a live daemon whose store it must never change: the
//! connection is opened read-only, never created, and never migrated. A store
//! whose schema predates harness sessions (0011) is refused, never guessed at.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pij_core::error::{PijError, Result};
use pij_core::fleet::Seat;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, SqliteConnection};

/// The oldest schema with `seats.harness_session`.
const MIN_SCHEMA: u32 = 11;

/// Spine kinds whose payload names a harness session the seat was bound to.
const SESSION_KINDS: [&str; 6] = [
    "seat.native-resumed",
    "seat.reclaimed",
    "seat.resumed",
    "seat.revive-postmortem",
    "spawn.bound",
    "seat.tombstone",
];

/// The seats of a store, and the schema they were read at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetSeats {
    /// Every seat, ordered by id.
    pub seats: Vec<Seat>,
    /// The store's `user_version`.
    pub schema_version: u32,
}

fn adapter(message: String) -> PijError {
    PijError::Adapter {
        adapter: "store/fleet".to_string(),
        message,
    }
}

/// Read every seat and the harness sessions it was bound to, without writing.
///
/// # Errors
/// [`PijError::Adapter`] when the store is missing, unreadable, or older than
/// schema 0011.
pub async fn read_seats(path: &Path) -> Result<FleetSeats> {
    if !path.is_file() {
        return Err(adapter(format!("no pij store at {}", path.display())));
    }
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .connect()
        .await
        .map_err(|error| {
            adapter(format!(
                "could not open {} read-only: {error}",
                path.display()
            ))
        })?;
    let result = read(&mut conn).await;
    let _ = conn.close().await;
    result
}

async fn read(conn: &mut SqliteConnection) -> Result<FleetSeats> {
    let fail = |error: sqlx::Error| adapter(format!("could not read seats: {error}"));
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *conn)
        .await
        .map_err(fail)?;
    let schema_version = u32::try_from(version.max(0)).unwrap_or(0);
    if schema_version < MIN_SCHEMA {
        return Err(adapter(format!(
            "the store is at schema {schema_version}; fleet-report needs {MIN_SCHEMA} or later"
        )));
    }
    let roles: BTreeMap<String, String> = sqlx::query("SELECT seat, role FROM seat_roles")
        .fetch_all(&mut *conn)
        .await
        .map_err(fail)?
        .iter()
        .map(|row| (row.get::<String, _>("seat"), row.get::<String, _>("role")))
        .collect();
    let mut seats: BTreeMap<String, (Seat, BTreeSet<String>)> = BTreeMap::new();
    let rows = sqlx::query(
        "SELECT id, harness, harness_session, folder, role, parent, tombstoned_at FROM seats",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(fail)?;
    for row in rows {
        let id: String = row.get("id");
        let mut sessions = BTreeSet::new();
        if let Some(session) = row.get::<Option<String>, _>("harness_session") {
            sessions.insert(session);
        }
        let seat = Seat {
            role: roles
                .get(&id)
                .cloned()
                .or_else(|| row.get::<Option<String>, _>("role")),
            harness: row.get("harness"),
            folder: row.get("folder"),
            parent: row.get("parent"),
            spawned_ms: None,
            ended_ms: row.get("tombstoned_at"),
            sessions: Vec::new(),
            id: id.clone(),
        };
        seats.insert(id, (seat, sessions));
    }
    let placeholders = vec!["?"; SESSION_KINDS.len()].join(", ");
    let sql = format!(
        "SELECT at, seat, kind, payload FROM spine_events \
         WHERE kind = 'seat.put' OR kind IN ({placeholders}) ORDER BY seq"
    );
    let mut query = sqlx::query(&sql);
    for kind in SESSION_KINDS {
        query = query.bind(kind);
    }
    for row in query.fetch_all(&mut *conn).await.map_err(fail)? {
        let Some(seat) = row.get::<Option<String>, _>("seat") else {
            continue;
        };
        let Some((record, sessions)) = seats.get_mut(&seat) else {
            continue;
        };
        let kind: String = row.get("kind");
        let payload: serde_json::Value =
            serde_json::from_str(&row.get::<String, _>("payload")).unwrap_or_default();
        let field = if kind == "seat.put" {
            "session"
        } else {
            "old_harness_session"
        };
        if let Some(session) = payload.get(field).and_then(serde_json::Value::as_str) {
            sessions.insert(session.to_string());
        }
        if kind == "seat.put" && record.spawned_ms.is_none() {
            record.spawned_ms = Some(row.get("at"));
        }
    }
    Ok(FleetSeats {
        seats: seats
            .into_values()
            .map(|(mut seat, sessions)| {
                seat.sessions = sessions.into_iter().collect();
                seat
            })
            .collect(),
        schema_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_testkit::FreshStore;

    async fn seeded() -> FreshStore {
        let store = FreshStore::new();
        let pool = crate::open(&store.path()).await.expect("open");
        for sql in [
            "INSERT INTO seats (id, harness, harness_session, folder, state, role, parent, tombstoned_at, seq) \
             VALUES ('pij-able-stoat', 'claude', 'sess-2', '/work/demo', 'idle', 'stream s07', 'pij-boss', 1790000000000, 1)",
            "INSERT INTO seats (id, harness, folder, state, seq) VALUES ('pij-boss', 'omp', '/work/demo', 'idle', 2)",
            "INSERT INTO seat_roles (seat, role, assigned_by, assigned_at) VALUES ('pij-boss', 'o-prime', 'jordan', 1)",
            r#"INSERT INTO spine_events (v, at, kind, seat, payload) VALUES (1, 1789000000000, 'seat.put', 'pij-able-stoat', '{"session":"sess-1"}')"#,
            r#"INSERT INTO spine_events (v, at, kind, seat, payload) VALUES (1, 1789500000000, 'seat.put', 'pij-able-stoat', '{"session":"sess-2"}')"#,
            r#"INSERT INTO spine_events (v, at, kind, seat, payload) VALUES (1, 1789600000000, 'seat.native-resumed', 'pij-able-stoat', '{"old_harness_session":"sess-0"}')"#,
            r#"INSERT INTO spine_events (v, at, kind, seat, payload) VALUES (1, 1789700000000, 'message.pushed', 'pij-able-stoat', '{"session":"not-a-binding"}')"#,
        ] {
            sqlx::query(sql).execute(&pool).await.expect(sql);
        }
        pool.close().await;
        store
    }

    #[tokio::test]
    async fn seats_carry_every_harness_session_they_were_bound_to() {
        let store = seeded().await;
        let read = read_seats(Path::new(&store.path())).await.expect("read");
        assert_eq!(read.schema_version, crate::SCHEMA_VERSION);
        let ids: Vec<&str> = read.seats.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["pij-able-stoat", "pij-boss"]);
        let stoat = &read.seats[0];
        assert_eq!(stoat.sessions, ["sess-0", "sess-1", "sess-2"]);
        assert_eq!(stoat.spawned_ms, Some(1_789_000_000_000));
        assert_eq!(stoat.ended_ms, Some(1_790_000_000_000));
        assert_eq!(stoat.parent.as_deref(), Some("pij-boss"));
        assert_eq!(stoat.role.as_deref(), Some("stream s07"));
        assert_eq!(
            read.seats[1].role.as_deref(),
            Some("o-prime"),
            "an asserted role wins"
        );
        assert!(read.seats[1].sessions.is_empty());
    }

    /// A missing store is refused, never created.
    #[tokio::test]
    async fn a_missing_store_is_refused_and_not_created() {
        let store = FreshStore::new();
        let error = read_seats(Path::new(&store.path())).await.unwrap_err();
        assert!(matches!(error, PijError::Adapter { .. }), "{error:?}");
        assert!(!store.exists(), "the reader created a store");
    }

    /// The reader never migrates: an old store stays at its version and is refused.
    #[tokio::test]
    async fn an_old_store_is_refused_and_left_unmigrated() {
        let store = FreshStore::new();
        let mut conn = SqliteConnectOptions::new()
            .filename(store.path())
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE seats (id TEXT)")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("PRAGMA user_version = 10")
            .execute(&mut conn)
            .await
            .unwrap();
        drop(conn);
        let error = read_seats(Path::new(&store.path())).await.unwrap_err();
        assert!(format!("{error:?}").contains("schema 10"), "{error:?}");
        let mut conn = SqliteConnectOptions::new()
            .filename(store.path())
            .connect()
            .await
            .unwrap();
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(version, 10);
    }
}
