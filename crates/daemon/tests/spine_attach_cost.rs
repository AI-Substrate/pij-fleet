//! Plan 185 recorded-workload replay: what one cursor attach costs on a large spine.
//!
//! Ignored by default (it builds a multi-million-row store). Run one size at a time
//! under `/usr/bin/time -l` so the peak RSS belongs to the attach:
//!
//! ```text
//! PIJ_ATTACH_ROWS=2978027 /usr/bin/time -l cargo test --release -p pij-daemon \
//!     --test spine_attach_cost -- --ignored --nocapture
//! ```
//!
//! The rows mirror the live spine on 2026-10-29: `bg.lost` payloads of ~355 bytes.

use std::sync::Arc;
use std::time::Instant;

use pij_core::events::EventFilter;
use pij_core::model::Seq;
use pij_daemon::events::EventBus;
use pij_store::SqliteSpine;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "builds a multi-million-row store; run by hand for plan 185 receipts"]
async fn one_cursor_attach_on_a_large_spine() {
    let rows: u64 = std::env::var("PIJ_ATTACH_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(500_000);
    let dir = std::env::temp_dir().join(format!("pij-185-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pij.sqlite");
    let pool = pij_store::open(path.to_str().unwrap()).await.unwrap();
    let payload = format!(r#"{{"job_id":"bg-bench","pad":"{}"}}"#, "x".repeat(320));
    let filled = Instant::now();
    sqlx::query(
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < ?1) \
         INSERT INTO spine_events (v, at, kind, seat, payload) \
         SELECT 1, x, 'bg.lost', 'pij-bench', ?2 FROM n",
    )
    .bind(i64::try_from(rows).unwrap())
    .bind(&payload)
    .execute(&pool)
    .await
    .unwrap();
    eprintln!("filled {rows} rows in {:?}", filled.elapsed());

    let bus = EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 64).unwrap();
    // A consumer 10 events behind: the replay it needs is 10 rows.
    for _ in 0..3 {
        let started = Instant::now();
        let subscription = bus
            .subscribe(Some(Seq(rows - 10)), EventFilter::all())
            .await
            .unwrap();
        eprintln!(
            "attach rows={rows} cursor_lag=10 elapsed={:?}",
            started.elapsed()
        );
        drop(subscription);
    }
    pool.close().await;
    let _ = std::fs::remove_dir_all(&dir);
}
