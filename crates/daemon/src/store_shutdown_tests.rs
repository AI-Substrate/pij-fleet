use std::time::Duration;

use pij_core::config::Config;

#[tokio::test]
async fn shutdown_returns_with_connections_never_returned() {
    let root = pij_testkit::fresh_dir("shutdown-held-connections");
    let daemon = super::boot(&Config::default(), root.join("daemon"))
        .await
        .unwrap();
    let pools = daemon.store_pools.clone();
    let held = [
        pools[0].acquire().await.unwrap(),
        pools[1].acquire().await.unwrap(),
    ];
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(6), daemon.shutdown()).await;
    assert!(
        result.is_ok(),
        "shutdown hung with checked-out connections: {:?}",
        started.elapsed()
    );
    result.unwrap().unwrap();
    assert!(pools.iter().all(|pool| pool.is_closed()));
    drop(held);
    std::fs::remove_dir_all(root).unwrap();
}

/// F267 step 2: a shutdown phase that blocks its thread holds back only
/// itself. The other phase can end only once the blocking one has started
/// to block, and the blocking one holds its thread until the other has ended
/// (or 5 s): in its own task the other ends meanwhile; polled by the same
/// task, as before, it could not, and every phase logged the slowest's end
/// (2026-11-21, "ended after 11305ms" each).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocking_shutdown_phase_holds_back_only_itself() {
    let (blocking, blocked) = tokio::sync::oneshot::channel::<()>();
    let (ended, quick_ended) = std::sync::mpsc::channel::<()>();
    let (slow, quick) = tokio::join!(
        super::spawn_phase("blocking", async move {
            // Told from outside the runtime, so the wake is never parked
            // behind this thread (a worker's own LIFO slot is not stolen).
            std::thread::spawn(move || blocking.send(()));
            quick_ended.recv_timeout(Duration::from_secs(5)).is_ok()
        }),
        super::spawn_phase("quick", async move {
            let _ = blocked.await;
            let _ = ended.send(());
        }),
    );
    quick.expect("the quick phase");
    let (saw_it_end, _) = slow.expect("the blocking phase");
    assert!(saw_it_end, "the quick phase waited behind the blocking one");
}
