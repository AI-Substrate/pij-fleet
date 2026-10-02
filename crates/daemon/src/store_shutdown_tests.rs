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
