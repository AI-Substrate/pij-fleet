//! F258: the store pool takes one round trip less per lease on acquire and one
//! less on release. These pin the two guarantees that cut must not weaken: the
//! per-call schema brake still sees another binary's migration, and a lease
//! that leaks a transaction is still cleaned, before its connection goes idle.

use std::time::Duration;

use pij_core::error::PijError;
use pij_testkit::FreshStore;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Sqlite, SqliteConnection, TransactionManager};

async fn foreign(path: &str) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .busy_timeout(Duration::ZERO),
    )
    .await
    .expect("a second binary opens the same file")
}

/// Another binary migrates the file between two calls, on a connection the
/// pool already holds open: the second call must refuse it. A version cached
/// per pool or per connection would pass the first call and limp on through
/// the second.
#[tokio::test]
async fn a_migration_by_another_binary_between_calls_is_refused() {
    let fresh = FreshStore::new();
    let path = fresh.path();
    let pool = pij_store::open(&path).await.expect("open");
    pij_store::require_current_schema(&pool)
        .await
        .expect("the first call sees the current schema");
    let live = pool.size();
    assert!(live > 0, "the first call leaves a live pooled connection");

    let mut other = foreign(&path).await;
    sqlx::query(&format!(
        "PRAGMA user_version = {}",
        pij_store::SCHEMA_VERSION + 1
    ))
    .execute(&mut other)
    .await
    .expect("the other binary migrates");
    other.close().await.expect("close the other binary");

    let error = pij_store::require_current_schema(&pool)
        .await
        .expect_err("the migrated store must be refused on the next call");
    assert!(
        matches!(error, PijError::StoreSchemaStale { found, .. } if found == pij_store::SCHEMA_VERSION + 1),
        "wrong error: {error:?}"
    );
    assert_eq!(
        pool.size(),
        live,
        "the refusal came from the same live connection, not a fresh one"
    );
}

/// A lease that drops its connection mid-transaction (no `Transaction` guard
/// left to roll back) is rolled back before the connection rejoins the idle
/// queue: its write lock is gone while it idles, its write never lands, and the
/// next lease starts outside any transaction.
#[tokio::test]
async fn a_lease_left_mid_transaction_is_cleaned_before_it_idles() {
    let fresh = FreshStore::new();
    let path = fresh.path();
    let pool = pij_store::open(&path).await.expect("open");

    let mut lease = pool.acquire().await.expect("acquire");
    <Sqlite as sqlx::Database>::TransactionManager::begin(&mut lease, None)
        .await
        .expect("begin without a guard");
    sqlx::query("CREATE TABLE leaked_write (x)")
        .execute(&mut *lease)
        .await
        .expect("write inside the leaked transaction");
    assert!(lease.is_in_transaction());
    drop(lease);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while pool.num_idle() < pool.size() as usize {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the leaked lease never returned to the pool"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let mut other = foreign(&path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut other)
        .await
        .expect("an idle connection must not hold the write lock");
    let leaked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name = 'leaked_write'")
            .fetch_one(&mut other)
            .await
            .expect("read the schema");
    assert_eq!(leaked, 0, "the leaked write was rolled back");
    sqlx::query("ROLLBACK")
        .execute(&mut other)
        .await
        .expect("release the probe");
    other.close().await.expect("close the other binary");

    let next = pool.acquire().await.expect("reuse");
    assert!(
        !next.is_in_transaction(),
        "the next lease starts outside any transaction"
    );
}
