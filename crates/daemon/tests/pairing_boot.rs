//! Plan 164: a daemon refuses to BOOT with an unsafe pairing, before it binds
//! or publishes a key; a remote listen address it cannot or may not use costs
//! only that listener, never the loopback one.

use pij_core::config::{Config, PeerDefinition};
use pij_daemon::http::RemoteListener;

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

/// Boot, then prove the LOOPBACK listener answers an authenticated call.
async fn boot_serving_loopback(config: Config) -> pij_daemon::Daemon {
    let state = pij_testkit::fresh_dir("pij-pairing-boot");
    let daemon = match pij_daemon::boot(&config, state).await {
        Ok(daemon) => daemon,
        Err(error) => panic!("a remote-listener problem must never stop the daemon: {error}"),
    };
    assert!(daemon.addr.ip().is_loopback(), "{}", daemon.addr);
    let health = reqwest::Client::new()
        .get(format!("http://{}/health", daemon.addr))
        .header("authorization", daemon.key.header())
        .send()
        .await
        .expect("loopback answers");
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    daemon
}

/// Plan 164 prime ruling: loopback is ALWAYS bound. An unpaired daemon given
/// a non-loopback address refuses that listener, says so, and keeps serving
/// local clients on loopback.
#[tokio::test]
async fn an_unpaired_remote_bind_is_refused_and_loopback_kept() {
    for bind in ["0.0.0.0:0", "100.64.0.1:0"] {
        let daemon = boot_serving_loopback(Config {
            bind_addr: bind.to_string(),
            insecure_bind: true,
            ..Config::default()
        })
        .await;
        assert!(
            matches!(&daemon.remote, RemoteListener::Refused(reason) if reason.contains("no machine is paired")),
            "{bind}: {:?}",
            daemon.remote
        );
        daemon.shutdown().await.expect("shutdown");
    }
}

/// A paired daemon whose second bind fails (here: a Tailscale address this
/// host does not hold) logs it and keeps loopback; it never exits.
#[tokio::test]
async fn a_failed_remote_bind_keeps_loopback_serving() {
    let daemon = boot_serving_loopback(Config {
        bind_addr: "100.64.0.1:0".to_string(),
        machine_alias: Some("mac-studio".to_string()),
        peers: vec![peer("laptop", "laptop-key-0123456789abcdef0123456789")],
        ..Config::default()
    })
    .await;
    assert!(
        matches!(&daemon.remote, RemoteListener::Failed(reason) if reason.contains("100.64.0.1")),
        "{:?}",
        daemon.remote
    );
    daemon.shutdown().await.expect("shutdown");
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

/// Review F04 (IPv6): 127.0.0.1 is ALWAYS bound. `--bind [::1]:<port>` only
/// ADDS a listener; it never replaces the IPv4 loopback local clients use.
#[tokio::test]
async fn an_ipv6_loopback_bind_adds_to_ipv4_loopback_never_replaces_it() {
    let state = pij_testkit::fresh_dir("pij-pairing-boot");
    let daemon = match pij_daemon::boot(
        &Config {
            bind_addr: "[::1]:0".to_string(),
            ..Config::default()
        },
        state,
    )
    .await
    {
        Ok(daemon) => daemon,
        Err(error) => panic!("boot: {error}"),
    };
    let port = daemon.addr.port();
    let health = |host: String| {
        let header = daemon.key.header();
        async move {
            reqwest::Client::new()
                .get(format!("http://{host}:{port}/health"))
                .header("authorization", header)
                .send()
                .await
                .map(|response| response.status().as_u16())
        }
    };
    assert_eq!(
        health("127.0.0.1".to_string()).await.ok(),
        Some(200),
        "IPv4 loopback must serve whatever --bind says"
    );
    if let RemoteListener::Listening { addr, .. } = &daemon.remote {
        assert!(addr.ip().is_loopback() && addr.is_ipv6(), "{addr}");
        assert_eq!(health("[::1]".to_string()).await.ok(), Some(200));
    }
    daemon.shutdown().await.expect("shutdown");
}
