//! Native list cwd scoping against a private daemon, never the production listener.
use pij_core::model::{Harness, SeatDescriptor};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::fresh_dir;
use serde_json::Value;

#[tokio::test]
async fn native_list_here_filters_and_refuses_valued_flags() {
    let root = fresh_dir("pij-cli-list-here");
    let folder = root.canonicalize().expect("canonical cwd");
    std::fs::write(root.join("daemon.key"), "list-test-key").expect("key");
    let services = pij_daemon::build_services(&pij_core::config::Config::default(), &root)
        .await
        .expect("services");
    for (id, path) in [
        ("pij-here", folder.to_str().expect("cwd")),
        ("pij-away", "/elsewhere"),
    ] {
        services
            .registry
            .put(SeatDescriptor::new(id, Harness::Omp, path))
            .await
            .expect("seat");
    }
    let router = router_with_config(
        services,
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local("list-test-key".into()),
            machine_alias: "test".into(),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("private listener");
    let addr = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, router).await.expect("serve") });
    for (args, expected) in [
        (vec!["--here"], Some(vec!["pij-here"])),
        (vec![], Some(vec!["pij-here", "pij-away"])),
        (vec!["--here=/elsewhere"], None),
        (vec!["--here", "relative"], None),
    ] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pij-rs"));
        let output = command
            .current_dir(&root)
            .env_remove("TMUX_PANE")
            .env_remove("PIJ_SESSION_ID")
            .args([
                "--state-dir",
                root.to_str().expect("root"),
                "--addr",
                &addr.to_string(),
                "--json",
                "list",
            ])
            .args(args)
            .output()
            .await
            .expect("run native list");
        let payload: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "decodable envelope: {error}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        if let Some(mut expected) = expected {
            assert!(output.status.success(), "{payload}");
            let mut ids: Vec<_> = payload["data"]["seats"]
                .as_array()
                .expect("seats")
                .iter()
                .map(|seat| seat["id"].as_str().expect("id"))
                .collect();
            ids.sort_unstable();
            expected.sort_unstable();
            assert_eq!(ids, expected);
        } else {
            assert!(!output.status.success());
            assert_eq!(payload["details"]["code"], "E-RS-ARG");
        }
    }
    server.abort();
    std::fs::remove_dir_all(root).expect("remove fixture");
}
