//! Opening the store, and refusing to open the wrong one.
//!
//! Workshop 001 R1, in three rules:
//!
//! 1. **The daemon self-migrates at boot.** No second command, no "did you run
//!    migrate?" step — a schema that needs a human is a schema that will be stale
//!    on somebody's machine.
//! 2. **Every db-touching command runs a cheap schema check.** One integer read
//!    (`PRAGMA user_version`), so it costs nothing to do it every time.
//! 3. **Skew is directional.** Newer binary over an older schema migrates
//!    forward. Older binary over a NEWER schema refuses — it cannot know what the
//!    newer schema means, and a half-understood schema corrupts quietly. Never
//!    limp.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Connection, Pool, Sqlite, SqliteConnection, TransactionManager};

use pij_core::error::{PijError, Result};

/// An open connection pool for the pij store.
///
/// Named here so a composition root can hold one WITHOUT depending on sqlx: the
/// daemon shares one pool across the three store adapters (registry, spine,
/// queue) and must not therefore grow a SQL dependency to describe it. The trait
/// split stays three ports; what they share is a resource, not a contract.
pub type StorePool = Pool<Sqlite>;

/// The schema version this binary speaks.
pub const SCHEMA_VERSION: u32 = 29;

/// The embedded migration set. Compiled in, so a binary always carries the
/// schema it expects rather than trusting a directory to be present.
pub static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Maximum wait for a free pooled connection or SQLite's write lock.
pub const STORE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Run an admitted write independently of request cancellation.
///
/// Inputs must be owned before admission. Dropping the waiter never drops the
/// transaction between its BEGIN acknowledgement and COMMIT/ROLLBACK.
pub(crate) async fn owned_write<T: Send + 'static>(
    work: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    tokio::spawn(work)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!("owned write task failed: {error}"),
        })?
}

/// Close the pool and drain physical connections until the shared shutdown deadline.
///
/// sqlx 0.8.6 can return from `Pool::close` with a checked-out writer remaining.
/// Size is a secondary drain witness, never an unbounded wait: a leaked lease or
/// stalled SQLite worker must not prevent stop/restart. Returns false on expiry;
/// the composition root reports the remaining leases once for the entire drain.
pub async fn close(pool: &StorePool, deadline: tokio::time::Instant) -> bool {
    tokio::time::timeout_at(deadline, async {
        pool.close().await;
        while pool.size() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

async fn clean_connection(
    connection: &mut SqliteConnection,
    recovered: &AtomicU64,
) -> std::result::Result<bool, sqlx::Error> {
    // Flush any Transaction::drop rollback already queued on the SQLite worker
    // before examining its depth; normal drop cleanup is not a poisoned lease.
    connection.ping().await?;
    if !connection.is_in_transaction() {
        return Ok(true);
    }
    let count = recovered.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!("pij-rs store: open transaction at pool boundary; recovery_count={count}");
    while connection.is_in_transaction() {
        <Sqlite as sqlx::Database>::TransactionManager::rollback(connection).await?;
    }
    Ok(true)
}

/// Open a store at `path` (or in memory when empty), migrating it forward.
///
/// # Errors
/// [`PijError::StoreSchemaStale`] when the file's schema is NEWER than this
/// binary understands; [`PijError::Adapter`] when SQLite itself refuses.
pub async fn open(path: &str) -> Result<Pool<Sqlite>> {
    let options = if path.is_empty() {
        SqliteConnectOptions::new().in_memory(true)
    } else {
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
    }
    // WAL lives on the connection because `PRAGMA journal_mode` is a no-op
    // inside the transaction sqlx wraps each migration in — set it in the
    // migration and it silently does nothing.
    .journal_mode(SqliteJournalMode::Wal)
    // A daemon that loses the last few writes on power loss has lost seat
    // truth; NORMAL is the WAL-appropriate setting, FULL is the paranoid one.
    .synchronous(SqliteSynchronous::Normal)
    .foreign_keys(true)
    .busy_timeout(STORE_WAIT_TIMEOUT);

    let recovered = Arc::new(AtomicU64::new(0));
    let released = Arc::clone(&recovered);
    let pool = SqlitePoolOptions::new()
        // Do not warm up siblings against the old schema: migration pins the
        // first connection before runtime readers can grow the pool.
        .min_connections(0)
        .max_connections(if path.is_empty() { 1 } else { 4 })
        .acquire_timeout(STORE_WAIT_TIMEOUT)
        .after_release(move |connection, _| {
            let recovered = Arc::clone(&released);
            Box::pin(async move { clean_connection(connection, &recovered).await })
        })
        .before_acquire(move |connection, _| {
            let recovered = Arc::clone(&recovered);
            Box::pin(async move { clean_connection(connection, &recovered).await })
        })
        .connect_with(options)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!("could not open {}: {error}", describe(path)),
        })?;

    migrate(&pool, path).await?;
    Ok(pool)
}

/// Bring an open pool to [`SCHEMA_VERSION`], or refuse.
///
/// Two sources of truth exist here and they are NOT equals: sqlx's
/// `_sqlx_migrations` ledger is authoritative about what actually ran, and
/// `user_version` is a one-integer CACHE of that answer so the per-command check
/// costs a single read. They can fall out of step — a store migrated by a build
/// that never wrote the pragma reports 0 while being structurally current — so
/// boot reconciles the cache to the ledger instead of believing whichever it
/// read first. (Found by `an_older_store_is_migrated_forward_rather_than_refused`:
/// the migration is a no-op the second time, so `user_version` stayed 0 and every
/// command afterwards refused a perfectly current store.)
async fn migrate(pool: &Pool<Sqlite>, path: &str) -> Result<()> {
    // Keep bootstrap on one connection until every schema change is committed.
    // Releasing and reacquiring between these steps can open a sibling with
    // the old schema cached; SQLite then reprepares SELECT * during stepping,
    // after sqlx has already captured its obsolete column-name metadata.
    let mut connection = pool.acquire().await.map_err(|error| PijError::Adapter {
        adapter: "store".to_string(),
        message: format!("could not acquire the migration connection: {error}"),
    })?;
    let found: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!("could not read the schema version: {error}"),
        })?;
    let found = found.max(0) as u32;

    if found > SCHEMA_VERSION {
        return Err(PijError::StoreSchemaStale {
            found,
            expected: SCHEMA_VERSION,
            fix: format!(
                "{} was written by a NEWER pij than this binary — upgrade pij (or point \
                 PIJ_STORE at a different file); this build will not guess at a schema it \
                 does not know",
                describe(path)
            ),
        });
    }

    MIGRATIONS
        .run(&mut *connection)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!("migration failed for {}: {error}", describe(path)),
        })?;

    // `PRAGMA user_version` takes no bind parameters; the value is a compile-time
    // constant of this binary, never caller input.
    sqlx::query(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .execute(&mut *connection)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!(
                "could not record the schema version on {}: {error}",
                describe(path)
            ),
        })?;

    Ok(())
}

/// The cheap check every db-touching command runs: one integer, no joins.
///
/// # Errors
/// [`PijError::Adapter`] when the pragma cannot be read at all.
pub async fn schema_version(pool: &Pool<Sqlite>) -> Result<u32> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!("could not read the schema version: {error}"),
        })?;
    Ok(version.max(0) as u32)
}

/// Refuse to proceed unless the open store is exactly the schema this binary
/// speaks.
///
/// Called by EVERY public store operation, not left to a caller to remember.
/// Review found the earlier version — a helper nothing invoked — which meant the
/// documented promise ("every db-touching command runs a fast schema check") was
/// true only of the tests that called it directly. A schema can change under a
/// live pool (another binary migrating the same file), so checking once at `open`
/// is not the same guarantee.
///
/// The cost is one `PRAGMA user_version` per operation: a page already in cache,
/// no join, no IO in the common case. If it ever shows up in a profile, the fix
/// is a cached generation counter invalidated on migration — not dropping the
/// check.
///
/// # Errors
/// [`PijError::StoreSchemaStale`] in either skew direction, with the fix that
/// matches the direction.
pub async fn require_current_schema(pool: &Pool<Sqlite>) -> Result<()> {
    let found = schema_version(pool).await?;
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    Err(PijError::StoreSchemaStale {
        found,
        expected: SCHEMA_VERSION,
        fix: if found < SCHEMA_VERSION {
            "restart the daemon so it self-migrates the store forward".to_string()
        } else {
            "upgrade pij — this binary is older than the store it was handed".to_string()
        },
    })
}

/// Begin a transaction that intends to WRITE.
///
/// `BEGIN IMMEDIATE`, never the default deferred: two deferred transactions
/// upgrading a read lock deadlock, and SQLite answers SQLITE_BUSY *without*
/// invoking the busy handler, so `busy_timeout` buys nothing. Shared by every
/// writer in this crate so the rule has one home rather than a copy per module.
///
/// # Errors
/// [`PijError::Adapter`] when the transaction cannot be opened.
pub async fn begin_write(pool: &Pool<Sqlite>) -> Result<sqlx::Transaction<'_, Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: error.to_string(),
        })
}

/// Whether the WAL journal actually took. Reported rather than assumed: the
/// pragma is silently ignored in several situations (in-memory databases,
/// read-only media), and a store that quietly fell back to a rollback journal
/// behaves differently under concurrency.
///
/// # Errors
/// [`PijError::Adapter`] when the pragma cannot be read.
pub async fn journal_mode(pool: &Pool<Sqlite>) -> Result<String> {
    sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
        .fetch_one(pool)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "store".to_string(),
            message: format!("could not read the journal mode: {error}"),
        })
}

fn describe(path: &str) -> String {
    if path.is_empty() {
        "the in-memory store".to_string()
    } else {
        path.to_string()
    }
}
