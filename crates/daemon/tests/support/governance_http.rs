use std::net::SocketAddr;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use serde_json::Value;

const KEY: &str = "governance-u3-test-key";

pub(crate) fn contracts() -> Value {
    serde_json::from_str(include_str!(
        "../../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .expect("canonical route fixtures")
}

pub(crate) fn case(fixtures: &Value, id: &str) -> Value {
    fixtures["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .flat_map(|route| route["cases"].as_array().expect("cases"))
        .find(|case| case["id"] == id)
        .expect("canonical case")
        .clone()
}

pub(crate) async fn daemon(
    fixtures: &Value,
) -> (SocketAddr, tokio::task::JoinHandle<()>, FreshStore) {
    let fresh = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: fresh.path(),
        ..Config::default()
    };
    let signals = std::path::Path::new(&config.store_path).with_extension("signals");
    let services = pij_daemon::build_services(&config, &signals)
        .await
        .expect("services");
    let create = case(fixtures, "project-create");
    for (pane, id) in fixtures["fixture_context"]["panes"]
        .as_object()
        .expect("panes")
    {
        let mut seat = SeatDescriptor::new(
            id.as_str().expect("seat id"),
            Harness::Omp,
            create["request"]["caller"]["cwd"]
                .as_str()
                .expect("caller cwd"),
        );
        seat.pane = Some(pane.to_string());
        if id == &fixtures["fixture_context"]["worker"] {
            seat.parent = Some(
                fixtures["fixture_context"]["parent"]
                    .as_str()
                    .expect("parent")
                    .into(),
            );
        }
        services.registry.put(seat).await.expect("seed seat");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let router = router_with_config(services, HttpConfig::local(KEY.to_string()));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    (addr, server, fresh)
}

pub(crate) async fn post(addr: SocketAddr, path: &str, request: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth(KEY)
        .json(request)
        .send()
        .await
        .expect("HTTP request")
}
