//! The config the SHIPPED daemon boots with — proven, not assumed.
//!
//! Review finding F2, wave 3. `daemon_config` was the FIX for the defect where
//! the shipped binary booted every adapter fake, and it had no automated coverage
//! at all: reverting it to the all-fake default left 231 tests green. The lesson
//! (E-025) was written down and not encoded, which is the failure this repo's own
//! rules call out — a paragraph that says "remember to check X" is worth nothing
//! next to a step that checks X for you.
//!
//! What this defends, precisely: that the binary a USER runs persists its roster
//! and can corroborate a process claim. Every unit test in the workspace proves a
//! part against the world the test builds; only booting the shipped
//! configuration proves it against the world the user gets.

use std::path::PathBuf;

use pij_cli::daemon_config;

/// Mutation witness: flip any adapter in `daemon_config`'s non-offline arm back
/// to `Fake`, or empty its `store_path`, and this test fails.
#[tokio::test]
async fn the_shipped_config_boots_real_adapters_and_leaves_a_store_on_disk() {
    let state_dir = std::env::temp_dir().join(format!("pij-rs-shipped-{}", std::process::id()));
    std::fs::create_dir_all(&state_dir).expect("state dir");
    let store_path = state_dir.join("pij.sqlite");
    let config = daemon_config("127.0.0.1:0", &store_path, false);

    // EVERY port, named. `is_fully_offline()` is an aggregate and an aggregate
    // cannot defend a per-port choice: with six real adapters and one fake it is
    // still "not fully offline", so the reviewer's exact mutation — flip
    // `transport` back to `Fake` — survived an offline-only assertion. A config
    // whose contract is per-port needs a per-port test.
    for (label, choice) in [
        ("registry", config.adapters.registry),
        ("spine", config.adapters.spine),
        ("queue", config.adapters.queue),
        ("liveness", config.adapters.liveness),
        ("tmux", config.adapters.tmux),
        ("harness", config.adapters.harness),
        // Real and conditionally open for spawn-stamped inbound acceptance.
        // The FAKE would claim deliveries it never performed, which is why
        // "fake here" is a defect rather than a harmless default.
        ("transport", config.adapters.transport),
    ] {
        assert!(
            choice.is_real(),
            "the shipped daemon must select the REAL {label} adapter"
        );
    }
    assert!(
        !config.is_fully_offline(),
        "the shipped daemon must touch the world it claims to manage"
    );
    assert_eq!(
        config.store_path,
        store_path.display().to_string(),
        "a daemon with no store path keeps its roster in memory and loses it on exit"
    );

    let daemon = pij_daemon::boot(&config, state_dir.clone())
        .await
        .expect("the shipped configuration must boot");

    let client = reqwest::Client::new();
    let health: serde_json::Value = client
        .get(format!("http://{}/health", daemon.addr))
        .bearer_auth(std::fs::read_to_string(&daemon.key.path).expect("key"))
        .send()
        .await
        .expect("health")
        .json()
        .await
        .expect("health json");
    assert_eq!(
        health["data"]["offline"], false,
        "/health must report the daemon it actually is"
    );

    assert!(
        store_path.exists(),
        "booting the shipped config must create the SQLite store — this is the \
         assertion that fails when the adapters silently revert to fakes"
    );

    daemon.shutdown().await.expect("shutdown");
    let _ = std::fs::remove_dir_all(&state_dir);
}

/// `--offline` is the OPT-IN, and it must remain genuinely offline: the flag is
/// how a demo or a test asks for a daemon that touches nothing, and a flag that
/// quietly opened a store would be worse than no flag.
#[tokio::test]
async fn the_offline_flag_selects_a_daemon_that_touches_nothing() {
    let config = daemon_config("127.0.0.1:0", &PathBuf::from("/unused"), true);
    assert!(config.is_fully_offline());
    assert!(
        config.store_path.is_empty(),
        "offline means in-memory: a store path here would write a file the caller \
         asked not to have"
    );
}
