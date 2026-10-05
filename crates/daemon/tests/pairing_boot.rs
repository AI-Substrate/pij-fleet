//! Plan 164: a daemon refuses to BOOT with an unsafe pairing or listen address,
//! before it binds or publishes a key.

use pij_core::config::{Config, PeerDefinition};

fn peer(alias: &str, key: &str) -> PeerDefinition {
    PeerDefinition {
        alias: alias.to_string(),
        url: "http://127.0.0.1:9".to_string(),
        key: key.to_string(),
    }
}

async fn boot_error(config: Config) -> String {
    let state = pij_testkit::fresh_dir("pij-pairing-boot");
    let refused = match pij_daemon::boot(&config, state.clone()).await {
        Ok(_) => panic!("boot must refuse"),
        Err(error) => error.to_string(),
    };
    assert!(
        !state.join("daemon.key").exists(),
        "a refused boot publishes no key"
    );
    refused
}

#[tokio::test]
async fn an_unpaired_daemon_refuses_any_non_loopback_bind() {
    for bind in ["0.0.0.0:0", "100.64.0.1:0"] {
        let refused = boot_error(Config {
            bind_addr: bind.to_string(),
            insecure_bind: true,
            ..Config::default()
        })
        .await;
        assert!(
            refused.contains("no machine is paired"),
            "{bind}: {refused}"
        );
    }
}

#[tokio::test]
async fn a_paired_daemon_refuses_a_non_tailscale_bind_without_insecure_bind() {
    let refused = boot_error(Config {
        bind_addr: "0.0.0.0:0".to_string(),
        machine_alias: Some("mac-studio".to_string()),
        peers: vec![peer("laptop", "laptop-key-0123456789abcdef0123456789")],
        ..Config::default()
    })
    .await;
    assert!(refused.contains("--insecure-bind"), "{refused}");
}

#[tokio::test]
async fn two_peers_sharing_a_key_refuse_the_boot_naming_aliases_only() {
    let shared = "shared-key-0123456789abcdef0123456789";
    let refused = boot_error(Config {
        machine_alias: Some("mac-studio".to_string()),
        peers: vec![peer("laptop", shared), peer("desktop", shared)],
        ..Config::default()
    })
    .await;
    assert!(
        refused.contains("`laptop` and `desktop` share one key"),
        "{refused}"
    );
    assert!(!refused.contains(shared), "{refused}");
}
