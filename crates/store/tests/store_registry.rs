//! Registry integration properties that require a real SQLite file.

mod support;

use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine};
use pij_store::{SqliteRegistry, SqliteSpine};
use pij_testkit::FreshStore;

#[tokio::test]
async fn interrupted_put_rolls_back_both_registry_row_and_spine_event() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");

    // Fail the second half of Registry::put after its spine INSERT. This models
    // interruption at the only dangerous boundary: the event exists in the
    // transaction, but the registry row cannot land.
    sqlx::query(
        "CREATE TRIGGER interrupt_test_put BEFORE INSERT ON seats \
         WHEN NEW.id = 'pij-interrupted' BEGIN \
         SELECT RAISE(ABORT, 'simulated interruption after spine append'); END",
    )
    .execute(&pool)
    .await
    .expect("install interruption trigger");

    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let spine = SqliteSpine::new(pool.clone());
    let interrupted = SeatDescriptor::new("pij-interrupted", Harness::Pi, "/tmp/interrupted");

    let error = registry
        .put(interrupted.clone())
        .await
        .expect_err("the injected interruption must abort put");
    assert!(
        error.to_string().contains("simulated interruption"),
        "the failure must retain the interruption evidence: {error}"
    );
    assert_eq!(
        registry.get(&interrupted.id).await.expect("get"),
        None,
        "an interrupted write must not leave half a registry row"
    );
    assert!(
        spine
            .tail(Some(&interrupted.id), Seq(0))
            .await
            .expect("tail")
            .is_empty(),
        "an interrupted write must not leave a spine event for a row that never landed"
    );

    let survivor = SeatDescriptor::new("pij-after-interruption", Harness::Pi, "/tmp/survivor");
    registry
        .put(survivor.clone())
        .await
        .expect("the store remains writable after rollback");
    assert_eq!(
        registry.get(&survivor.id).await.expect("get"),
        Some(survivor),
        "the store remains readable and consistent after rollback"
    );
}

#[tokio::test]
async fn absent_null_and_empty_are_three_distinct_registry_reads() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool, support::publisher());

    let absent = SeatId::from("pij-absent");
    assert_eq!(registry.get(&absent).await.expect("get absent"), None);

    let null = SeatDescriptor::new("pij-null-pane", Harness::Omp, "/tmp/null");
    registry.put(null.clone()).await.expect("put null");
    let mut empty = SeatDescriptor::new("pij-empty-pane", Harness::Omp, "/tmp/empty");
    empty.pane = Some(String::new());
    registry.put(empty.clone()).await.expect("put empty");

    assert_eq!(
        registry.get(&null.id).await.expect("get null"),
        Some(null),
        "SQL NULL must read as None on a present row"
    );
    assert_eq!(
        registry.get(&empty.id).await.expect("get empty"),
        Some(empty),
        "an empty string must remain Some(empty), never collapse to NULL"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_registry_writers_serialize_and_both_succeed() {
    let fresh = FreshStore::new();
    let pool_a = pij_store::open(&fresh.path()).await.expect("open writer a");
    let pool_b = pij_store::open(&fresh.path()).await.expect("open writer b");
    let registry_a = SqliteRegistry::new(pool_a.clone(), support::publisher());
    let registry_b = SqliteRegistry::new(pool_b, support::publisher());

    let write_a = async {
        tokio::task::yield_now().await;
        for index in 0..32 {
            let descriptor = SeatDescriptor::new(
                format!("pij-writer-a-{index:02}"),
                Harness::Pi,
                "/tmp/writer-a",
            );
            registry_a
                .put(descriptor.clone())
                .await
                .expect("writer a put");
            assert_eq!(
                registry_a.get(&descriptor.id).await.expect("writer a get"),
                Some(descriptor),
                "a successful write is immediately readable in full"
            );
        }
    };
    let write_b = async {
        tokio::task::yield_now().await;
        for index in 0..32 {
            let descriptor = SeatDescriptor::new(
                format!("pij-writer-b-{index:02}"),
                Harness::Omp,
                "/tmp/writer-b",
            );
            registry_b
                .put(descriptor.clone())
                .await
                .expect("writer b put");
            assert_eq!(
                registry_b.get(&descriptor.id).await.expect("writer b get"),
                Some(descriptor),
                "a successful write is immediately readable in full"
            );
        }
    };

    tokio::join!(write_a, write_b);

    let registry = SqliteRegistry::new(pool_a.clone(), support::publisher());
    assert_eq!(
        registry
            .list(Default::default())
            .await
            .expect("list after concurrent writes")
            .len(),
        64,
        "both writers must complete every write"
    );
    let torn: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM seats AS s \
         LEFT JOIN spine_events AS e \
           ON e.seq = s.seq AND e.kind = 'seat.put' AND e.seat = s.id \
         WHERE e.seq IS NULL",
    )
    .fetch_one(&pool_a)
    .await
    .expect("check row/event pairs");
    assert_eq!(
        torn, 0,
        "serialized commits must never expose a registry row without its matching spine event"
    );
}

#[tokio::test]
async fn put_reporting_distinguishes_created_placeholder_rebound_and_same() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let spine = SqliteSpine::new(pool);
    let mut descriptor = SeatDescriptor::new("pij-binding", Harness::Omp, "/tmp/binding");
    let (created_seq, created) = registry
        .put_reporting(descriptor.clone())
        .await
        .expect("create");
    assert!(created.inserted);
    assert_eq!(created.previous_proc, None);

    // Spawn pre-mints an unbound row. Its first real process is a rebind,
    // not an insert; an absent previous process must not mean an absent row.
    let identity = ProcIdentity {
        pid: 42,
        proc_start: 100,
    };
    descriptor.proc = Some(identity);
    let (rebound_seq, rebound) = registry
        .put_reporting(descriptor.clone())
        .await
        .expect("bind");
    assert!(!rebound.inserted);
    assert_eq!(rebound.previous_proc, None);
    assert_ne!(rebound.previous_proc, descriptor.proc);
    let (same_seq, same) = registry
        .put_reporting(descriptor.clone())
        .await
        .expect("refresh");
    assert!(!same.inserted);
    assert_eq!(same.previous_proc, descriptor.proc);

    descriptor.proc = Some(ProcIdentity {
        pid: identity.pid,
        proc_start: 101,
    });
    let (recycled_seq, recycled) = registry
        .put_reporting(descriptor.clone())
        .await
        .expect("recycled pid");
    assert!(!recycled.inserted);
    assert_eq!(recycled.previous_proc, Some(identity));
    assert_ne!(recycled.previous_proc, descriptor.proc);
    assert_eq!(
        registry.get(&descriptor.id).await.expect("committed row"),
        Some(descriptor.clone())
    );
    assert!(created_seq < rebound_seq && rebound_seq < same_seq && same_seq < recycled_seq);
    let events = spine
        .tail(Some(&descriptor.id), Seq(0))
        .await
        .expect("write events");
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|event| event.kind == "seat.put"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_put_reporting_observes_the_previous_committed_writer() {
    let fresh = FreshStore::new();
    let registry_a = SqliteRegistry::new(
        pij_store::open(&fresh.path()).await.expect("open a"),
        support::publisher(),
    );
    let registry_b = SqliteRegistry::new(
        pij_store::open(&fresh.path()).await.expect("open b"),
        support::publisher(),
    );
    let mut a = SeatDescriptor::new("pij-racing-binding", Harness::Omp, "/tmp/binding");
    a.proc = Some(ProcIdentity {
        pid: 42,
        proc_start: 100,
    });
    let mut b = a.clone();
    b.proc = Some(ProcIdentity {
        pid: 43,
        proc_start: 101,
    });
    let (result_a, result_b) = tokio::join!(
        registry_a.put_reporting(a.clone()),
        registry_b.put_reporting(b.clone()),
    );
    let (seq_a, report_a) = result_a.expect("writer a");
    let (seq_b, report_b) = result_b.expect("writer b");
    let (first, second, first_proc, last) = if seq_a < seq_b {
        (report_a, report_b, a.proc, b)
    } else {
        (report_b, report_a, b.proc, a)
    };
    assert!(first.inserted);
    assert_eq!(first.previous_proc, None);
    assert!(!second.inserted);
    assert_eq!(second.previous_proc, first_proc);
    assert_eq!(
        registry_a.get(&last.id).await.expect("final binding"),
        Some(last)
    );
}
