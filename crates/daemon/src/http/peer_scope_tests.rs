//! Plan 164 ruling 4: a peer key reaches ONLY the federation routes.
//!
//! The walk is over `Endpoint::ALL` (the one route inventory the router is
//! built from, including the per-job-token hook as a named exception) and
//! every standard HTTP method, so a route or verb added later is walked without
//! anyone remembering to add it here, and is refused by default.

use std::sync::Arc;

use axum::http::Method;
use pij_testkit::fakes::{FakeQueue, FakeRegistry, FakeSpine};
use serde_json::Value;

use super::router_with_config;
use super::tests::{config, spawn, test_services};
use super::{Endpoint, RouteAuth};

/// The ONLY (method, path) pairs a peer key may call.
const PEER_ROUTES: [(&str, &str); 3] = [
    ("POST", "/v1/send"),
    ("GET", "/v1/seats"),
    ("GET", "/v1/events"),
];

fn concrete(path: &str) -> String {
    path.replace("{job}", "bg-job")
}

#[tokio::test]
async fn a_peer_key_is_refused_by_every_route_but_the_federation_ones() {
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(64).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(
        services,
        config("local-key", &[("laptop", "peer-key")]),
    ))
    .await;
    let client = reqwest::Client::new();
    let mut wrong = Vec::new();
    for endpoint in Endpoint::ALL {
        for method in [
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::HEAD,
            Method::OPTIONS,
        ] {
            let path = concrete(endpoint.path());
            let allowed = PEER_ROUTES
                .iter()
                .any(|(verb, route)| *verb == method.as_str() && *route == path);
            if allowed {
                continue;
            }
            // The one named exception: a per-job-token route never consults
            // the bearer ring, so a peer key fails ITS check (401), not ours.
            let expected_status = match endpoint.auth() {
                RouteAuth::Bearer => 403,
                RouteAuth::JobToken => 401,
            };
            let response = client
                .request(method.clone(), format!("http://{addr}{path}"))
                .bearer_auth("peer-key")
                // A well-formed emit body, so that route reaches its token check.
                .json(&serde_json::json!({"text": "probe"}))
                .send()
                .await
                .expect("request");
            let status = response.status().as_u16();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            let meta = body["meta"].as_str().unwrap_or_default();
            // 405 is axum refusing a verb the route does not serve at all, so
            // no handler ran; anything else must be the named scope refusal.
            // HEAD carries no body to name the code; its status still must.
            let refused = match endpoint.auth() {
                RouteAuth::Bearer => method == Method::HEAD || meta.starts_with("E-RS-PEER-SCOPE"),
                RouteAuth::JobToken => true,
            };
            if status != 405 && !(status == expected_status && refused) {
                wrong.push(format!("{method} {path} -> {status} {body}"));
            }
        }
    }
    server.abort();
    assert!(
        wrong.is_empty(),
        "a peer key reached {} non-federation route(s):\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// Review S1: the walk above is only as good as its inventory. Every route the
/// router registers, including the per-job-token bg emit hook mounted outside
/// the bearer layer, must be in it, so nothing is walked by omission.
#[test]
fn the_route_inventory_names_every_registered_route() {
    let inventory: Vec<&str> = Endpoint::ALL
        .iter()
        .map(|endpoint| endpoint.path())
        .collect();
    assert!(
        inventory.contains(&super::background::EMIT_PATH),
        "{} is registered but missing from Endpoint::ALL",
        super::background::EMIT_PATH
    );
}

/// The three allowed calls, checked as allowed (not merely skipped by the walk).
#[tokio::test]
async fn a_peer_key_reaches_exactly_the_federation_routes() {
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(64).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(
        services,
        config("local-key", &[("laptop", "peer-key")]),
    ))
    .await;
    let client = reqwest::Client::new();
    for path in ["/v1/seats?scope=local", "/v1/events?scope=local"] {
        let response = client
            .get(format!("http://{addr}{path}"))
            .bearer_auth("peer-key")
            .send()
            .await
            .expect("request");
        assert_eq!(response.status().as_u16(), 200, "{path}");
    }
    // No such seat here: the handler ran and refused on the merits, not on scope.
    let send = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("peer-key")
        .json(&serde_json::json!({
            "from": "pij-a", "to": {"seat": "pij-nobody"}, "body": "hi", "msg_id": "m-1",
        }))
        .send()
        .await
        .expect("request");
    let status = send.status().as_u16();
    assert!(
        status != 401 && status != 403,
        "send reached its handler: {status}"
    );
    server.abort();
}
