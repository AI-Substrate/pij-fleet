//! pij's half of `pij fleet-report` (plan 162): every seat incarnation and
//! every harness session it was bound to, read from a store **read-only**.
//!
//! The report runs beside a live daemon whose store it must never change: the
//! connection is opened read-only, never created, and never migrated. A store
//! whose schema predates harness sessions (0011) is refused, never guessed at.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pij_core::error::{PijError, Result};
use pij_core::fleet::{MessageCount, Seat};
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
    /// Seats that are a project's prime (or the machine's designated prime).
    pub primes: Vec<String>,
    /// The projects each prime governs, by prime seat id.
    pub prime_projects: BTreeMap<String, Vec<String>>,
}

fn adapter(message: String) -> PijError {
    PijError::Adapter {
        adapter: "store/fleet".to_string(),
        message,
    }
}

/// Open the store for reading only: never created, never migrated, and any
/// write on the handle fails ("attempt to write a readonly database").
///
/// # Errors
/// [`PijError::Adapter`] when the store is missing or SQLite refuses it.
pub async fn open_read_only(path: &Path) -> Result<SqliteConnection> {
    if !path.is_file() {
        return Err(adapter(format!("no pij store at {}", path.display())));
    }
    SqliteConnectOptions::new()
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
        })
}

/// Read every seat and the harness sessions it was bound to, without writing.
///
/// # Errors
/// [`PijError::Adapter`] when the store is missing, unreadable, or older than
/// schema 0011.
pub async fn read_seats(path: &Path) -> Result<FleetSeats> {
    let mut conn = open_read_only(path).await?;
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
    // A project's prime (0014+) and the machine's designated prime.
    let mut primes: BTreeSet<String> =
        sqlx::query_scalar::<_, String>("SELECT seat FROM prime_designation")
            .fetch_all(&mut *conn)
            .await
            .map_err(fail)?
            .into_iter()
            .collect();
    let mut prime_projects: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if schema_version >= 14 {
        for row in sqlx::query(
            "SELECT slug, prime_id FROM projects WHERE prime_id IS NOT NULL ORDER BY slug",
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(fail)?
        {
            let prime: String = row.get("prime_id");
            prime_projects
                .entry(prime.clone())
                .or_default()
                .push(row.get("slug"));
            primes.insert(prime);
        }
    }
    let primes: Vec<String> = primes.into_iter().collect();
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
        let at: i64 = row.get("at");
        // Early stores stamped some events at 0: unknown, not 1970.
        if kind == "seat.put" && record.spawned_ms.is_none() && at > 0 {
            record.spawned_ms = Some(at);
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
        primes,
        prime_projects,
    })
}

/// pij messages per sender -> recipient pushed in `[since_ms, until_ms)`.
/// Metadata only: no body is read.
///
/// # Errors
/// [`PijError::Adapter`] when the store is missing or unreadable.
pub async fn read_messages(path: &Path, since_ms: i64, until_ms: i64) -> Result<Vec<MessageCount>> {
    let mut conn = open_read_only(path).await?;
    // The recipient is the event's seat; the sender is the payload's `from`.
    let rows = sqlx::query(
        "SELECT json_extract(payload, '$.from') AS sender, seat AS recipient, COUNT(*) AS n \
         FROM spine_events WHERE kind = 'message.pushed' AND at >= ?1 AND at < ?2 \
         AND seat IS NOT NULL AND json_extract(payload, '$.from') IS NOT NULL \
         GROUP BY sender, recipient ORDER BY sender, recipient",
    )
    .bind(since_ms)
    .bind(until_ms)
    .fetch_all(&mut conn)
    .await
    .map_err(|error| adapter(format!("could not read messages: {error}")));
    let _ = conn.close().await;
    Ok(rows?
        .iter()
        .map(|row| MessageCount {
            from: row.get("sender"),
            to: row.get("recipient"),
            messages: u64::try_from(row.get::<i64, _>("n")).unwrap_or(0),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::model::{Event, SeatId};
    use pij_core::ports::Spine;
    use pij_testkit::FreshStore;

    async fn seeded() -> FreshStore {
        let store = FreshStore::new();
        let pool = crate::open(&store.path()).await.expect("open");
        for sql in [
            "INSERT INTO seats (id, harness, harness_session, folder, state, role, parent, tombstoned_at, seq) \
             VALUES ('pij-able-stoat', 'claude', 'sess-2', '/work/demo', 'idle', 'stream s07', 'pij-boss', 1790000000000, 1)",
            "INSERT INTO seats (id, harness, folder, state, seq) VALUES ('pij-boss', 'omp', '/work/demo', 'idle', 2)",
            "INSERT INTO seat_roles (seat, role, assigned_by, assigned_at) VALUES ('pij-boss', 'o-prime', 'jordan', 1)",
        ] {
            sqlx::query(sql).execute(&pool).await.expect(sql);
        }
        // Spine facts go through the spine port: only store/spine.rs writes spine_events.
        let spine = crate::SqliteSpine::new(pool.clone());
        for (at, kind, payload) in [
            (1_789_000_000_000, "seat.put", r#"{"session":"sess-1"}"#),
            (1_789_500_000_000, "seat.put", r#"{"session":"sess-2"}"#),
            (
                1_789_600_000_000,
                "seat.native-resumed",
                r#"{"old_harness_session":"sess-0"}"#,
            ),
            (
                1_789_700_000_000,
                "message.pushed",
                r#"{"session":"not-a-binding"}"#,
            ),
        ] {
            spine
                .append(Event {
                    seq: None,
                    v: 1,
                    at,
                    kind: kind.to_string(),
                    seat: Some(SeatId("pij-able-stoat".into())),
                    payload: payload.to_string(),
                })
                .await
                .expect(kind);
        }
        for (at, from) in [
            (1_789_800_000_000, "pij-boss"),
            (1_789_800_001_000, "pij-boss"),
            (1_789_800_002_000, "pij-remote-otter"),
            (1_799_000_000_000, "pij-boss"),
        ] {
            spine
                .append(Event {
                    seq: None,
                    v: 1,
                    at,
                    kind: "message.pushed".to_string(),
                    seat: Some(SeatId("pij-able-stoat".into())),
                    payload: format!(
                        r#"{{"msg_id":"m-{at}","from":"{from}","body":"never read"}}"#
                    ),
                })
                .await
                .expect("message");
        }
        sqlx::query(
            "INSERT INTO projects (slug, created_by, created_at, prime_id) \
             VALUES ('demo', 'jordan', 1, 'pij-boss')",
        )
        .execute(&pool)
        .await
        .expect("project");
        pool.close().await;
        store
    }

    /// Message counts per sender -> recipient in the window, from pij's own
    /// delivery records; a sender that is not a local seat (another machine) is kept.
    #[tokio::test]
    async fn messages_are_counted_per_pair_inside_the_window() {
        let store = seeded().await;
        let counts = read_messages(
            Path::new(&store.path()),
            1_789_750_000_000,
            1_790_000_000_000,
        )
        .await
        .expect("messages");
        let pairs: Vec<(&str, &str, u64)> = counts
            .iter()
            .map(|m| (m.from.as_str(), m.to.as_str(), m.messages))
            .collect();
        assert_eq!(
            pairs,
            [
                ("pij-boss", "pij-able-stoat", 2),
                ("pij-remote-otter", "pij-able-stoat", 1)
            ]
        );
    }

    /// A project's prime is a prime, whatever role the seat registered with.
    #[tokio::test]
    async fn project_primes_are_named() {
        let store = seeded().await;
        let read = read_seats(Path::new(&store.path())).await.expect("read");
        assert_eq!(read.primes, ["pij-boss"]);
        assert_eq!(
            read.prime_projects["pij-boss"],
            ["demo"],
            "what it is prime for"
        );
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

    /// The handle cannot write: a write on it fails, and the file is unchanged.
    #[tokio::test]
    async fn the_store_handle_refuses_every_write() {
        let store = seeded().await;
        let path = std::path::PathBuf::from(store.path());
        // Settle the store first: the seeding pool's close may still be
        // checkpointing its WAL into the main file.
        let mut settle = SqliteConnectOptions::new()
            .filename(&path)
            .connect()
            .await
            .unwrap();
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut settle)
            .await
            .unwrap();
        settle.close().await.unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut conn = open_read_only(&path).await.expect("open");
        for sql in [
            "PRAGMA user_version = 99",
            "INSERT INTO seats (id, harness, folder, state, seq) VALUES ('x', 'claude', '/x', 'idle', 9)",
        ] {
            let error = sqlx::query(sql).execute(&mut conn).await.unwrap_err();
            assert!(error.to_string().contains("readonly"), "{sql}: {error}");
        }
        let _ = conn.close().await;
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the store file changed"
        );
    }

    /// A `seat.put` stamped at 0 (early stores) is an unknown spawn time, not 1970.
    #[tokio::test]
    async fn a_zero_stamp_is_an_unknown_spawn_time() {
        let store = seeded().await;
        let pool = crate::open(&store.path()).await.unwrap();
        crate::SqliteSpine::new(pool.clone())
            .append(Event {
                seq: None,
                v: 1,
                at: 0,
                kind: "seat.put".to_string(),
                seat: Some(SeatId("pij-boss".into())),
                payload: "{}".to_string(),
            })
            .await
            .unwrap();
        pool.close().await;
        let read = read_seats(Path::new(&store.path())).await.unwrap();
        assert_eq!(read.seats[1].spawned_ms, None);
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
