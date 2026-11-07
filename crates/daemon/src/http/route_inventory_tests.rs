//! Plan 221 AC1 (PRD 003 req-10, F88): the route inventory sensor.
//!
//! `harness/scripts/route-inventory.json` classifies every daemon HTTP method
//! and route pair as machine-public (with the reason), seat-authority (class,
//! the identities it grants authority by, today's guard) or job-token. This
//! test asks the ROUTER which pairs it serves, never the JSON: it walks
//! [`Endpoint::ALL`] under every standard method, HEAD and OPTIONS included
//! (axum serves HEAD through each GET handler), and counts a pair as served
//! unless axum answers 405. It fails on a served pair the inventory lacks, an entry
//! the daemon does not serve, a duplicate, and an entry without a valid
//! classification. One probe per run (`just route-inventory-check`): 59 paths
//! times 7 methods, sequential local calls to an isolated test router, under
//! a per-call and an overall timeout so a hang fails. Nothing runs per tick.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::Method;
use pij_testkit::fakes::{FakeQueue, FakeRegistry, FakeSpine};
use serde_json::{Value, json};

use super::router_with_config;
use super::tests::{config, spawn, test_services};
use super::{Endpoint, RouteAuth};

const INVENTORY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../harness/scripts/route-inventory.json"
);
const METHODS: [Method; 7] = [
    Method::GET,
    Method::HEAD,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
    Method::OPTIONS,
];
/// One call may take this long; a stuck handler fails the probe, not the gate.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// The whole enumeration may take this long.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const CLASSES: [&str; 9] = [
    "send-as",
    "inbox-claim",
    "role-and-holder",
    "outputs",
    "activity",
    "registration",
    "controls",
    "adoption",
    "attribution",
];
const GRANTS_BY: [&str; 6] = ["seat", "parent", "prime", "holder", "owner", "recipient"];
const GUARDS: [&str; 7] = ["R", "B", "N", "S", "C", "P", "A"];

/// One served (method, path) pair.
type Pair = (String, String);

fn text<'a>(entry: &'a Value, field: &str) -> &'a str {
    entry[field].as_str().map_or("", str::trim)
}

/// Every non-empty, known, distinct member of `entry[field]`, or why not.
fn members(entry: &Value, field: &str, known: &[&str]) -> Result<(), String> {
    let Some(list) = entry[field].as_array().filter(|list| !list.is_empty()) else {
        return Err(format!("{field} must be a non-empty list"));
    };
    let mut seen = BTreeSet::new();
    for item in list {
        let Some(item) = item.as_str().filter(|item| known.contains(item)) else {
            return Err(format!("{field} has {item}, not one of {known:?}"));
        };
        if !seen.insert(item) {
            return Err(format!("{field} lists {item} twice"));
        }
    }
    Ok(())
}

/// Why one entry's classification is invalid, if it is.
fn classification(entry: &Value, job_token: bool) -> Result<(), String> {
    let authority = text(entry, "authority");
    if job_token != (authority == "job-token") {
        return Err(if job_token {
            "the router authenticates this route by its own job token, so its authority must be job-token".into()
        } else {
            "only a route the router authenticates by its own job token may be job-token".into()
        });
    }
    match authority {
        "machine-public" | "job-token" if text(entry, "reason").is_empty() => {
            Err(format!("{authority} needs a reason"))
        }
        "machine-public" | "job-token" => Ok(()),
        "seat-authority" => {
            let class = text(entry, "class");
            if !CLASSES.contains(&class) {
                return Err(format!(
                    "seat-authority needs a class from {CLASSES:?}, not `{class}`"
                ));
            }
            members(entry, "grants_by", &GRANTS_BY)?;
            members(entry, "guard", &GUARDS)?;
            // Plan 223: the classes counted and gated where this route reaches
            // them, its own class among them (shared handlers reach more).
            members(entry, "operations", &CLASSES)?;
            let reaches = entry["operations"].as_array().into_iter().flatten();
            if !reaches.filter_map(Value::as_str).any(|op| op == class) {
                return Err(format!("operations must include its class `{class}`"));
            }
            Ok(())
        }
        other => Err(format!(
            "authority must be machine-public, seat-authority or job-token, not `{other}`"
        )),
    }
}

/// Every inventory finding, one line each, naming the pair.
fn inventory_findings(
    served: &BTreeSet<Pair>,
    job_token: &BTreeSet<Pair>,
    inventory: &Value,
) -> Vec<String> {
    let Some(routes) = inventory["routes"].as_array() else {
        return vec!["route-inventory.json: routes must be a list".into()];
    };
    let mut findings = Vec::new();
    let mut listed = BTreeSet::new();
    for (index, entry) in routes.iter().enumerate() {
        let (method, path) = (text(entry, "method"), text(entry, "path"));
        if method.is_empty() || path.is_empty() {
            findings.push(format!("routes[{index}]: needs a method and a path"));
            continue;
        }
        let pair = (method.to_string(), path.to_string());
        if !listed.insert(pair.clone()) {
            findings.push(format!("{method} {path}: listed twice"));
            continue;
        }
        if !served.contains(&pair) {
            findings.push(format!(
                "{method} {path}: listed, but the daemon does not serve it (stale; remove it)"
            ));
            continue;
        }
        if let Err(why) = classification(entry, job_token.contains(&pair)) {
            findings.push(format!("{method} {path}: {why}"));
        }
    }
    // An implicit HEAD runs its GET handler, so it carries the GET's
    // classification exactly (plan 223 AC1).
    const CLASSIFICATION: [&str; 6] = [
        "authority",
        "class",
        "grants_by",
        "guard",
        "operations",
        "reason",
    ];
    for head in routes
        .iter()
        .filter(|entry| text(entry, "method") == "HEAD")
    {
        let path = text(head, "path");
        let get = routes
            .iter()
            .find(|entry| text(entry, "method") == "GET" && text(entry, "path") == path);
        if let Some(get) = get {
            let differs: Vec<&str> = CLASSIFICATION
                .into_iter()
                .filter(|field| *field != "reason" && head[*field] != get[*field])
                .collect();
            if !differs.is_empty() {
                findings.push(format!(
                    "HEAD {path}: must carry its GET's classification; differs in {}",
                    differs.join(", ")
                ));
            }
        }
    }
    for (method, path) in served.difference(&listed) {
        findings.push(format!(
            "{method} {path}: served by the daemon but missing from route-inventory.json (classify it)"
        ));
    }
    findings
}

/// The (method, path) pairs the router really serves, and which of them its
/// own job token authenticates.
async fn served_pairs() -> (BTreeSet<Pair>, BTreeSet<Pair>) {
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(64).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("local-key", &[]))).await;
    let client = reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .build()
        .expect("client");
    let (mut served, mut job_token) = (BTreeSet::new(), BTreeSet::new());
    for endpoint in Endpoint::ALL {
        let path = endpoint.path();
        for method in METHODS {
            // No body: a served POST refuses at its extractor, before any work.
            let response = client
                .request(
                    method.clone(),
                    format!("http://{addr}{}", path.replace("{job}", "bg-job")),
                )
                .bearer_auth("local-key")
                .send()
                .await
                .expect("request");
            if response.status().as_u16() == 405 {
                continue;
            }
            let pair = (method.as_str().to_string(), path.to_string());
            if endpoint.auth() == RouteAuth::JobToken {
                job_token.insert(pair.clone());
            }
            served.insert(pair);
        }
    }
    server.abort();
    (served, job_token)
}

#[tokio::test]
async fn every_served_route_is_in_the_inventory_and_classified() {
    let raw = std::fs::read_to_string(INVENTORY)
        .unwrap_or_else(|error| panic!("read {INVENTORY}: {error}"));
    let inventory: Value =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("parse {INVENTORY}: {error}"));
    let (served, job_token) = tokio::time::timeout(PROBE_TIMEOUT, served_pairs())
        .await
        .unwrap_or_else(|_| panic!("the route probe took longer than {PROBE_TIMEOUT:?}"));
    // A probe that saw nothing would make every entry stale and prove nothing.
    assert!(
        served.len() >= Endpoint::ALL.len(),
        "the probe saw {} pairs for {} paths",
        served.len(),
        Endpoint::ALL.len()
    );
    let findings = inventory_findings(&served, &job_token, &inventory);
    assert!(
        findings.is_empty(),
        "route-inventory-check: {} finding(s)\n{}",
        findings.len(),
        findings.join("\n")
    );
}

fn pair(method: &str, path: &str) -> Pair {
    (method.to_string(), path.to_string())
}

fn seat_route(method: &str, path: &str) -> Value {
    json!({"method": method, "path": path, "authority": "seat-authority",
        "class": "send-as", "grants_by": ["seat"], "guard": ["R"], "operations": ["send-as"]})
}

fn public_route(method: &str, path: &str) -> Value {
    json!({"method": method, "path": path, "authority": "machine-public", "reason": "roster"})
}

fn check(served: &[Pair], routes: Vec<Value>) -> Vec<String> {
    inventory_findings(
        &served.iter().cloned().collect(),
        &BTreeSet::new(),
        &json!({"routes": routes}),
    )
}

#[test]
fn a_served_route_missing_from_the_inventory_fails() {
    let served = [pair("POST", "/v1/send"), pair("POST", "/v1/new")];
    assert_eq!(
        check(&served, vec![seat_route("POST", "/v1/send")]),
        ["POST /v1/new: served by the daemon but missing from route-inventory.json (classify it)"]
    );
    // A second method on a listed path is its own pair.
    let served = [pair("GET", "/v1/seats"), pair("POST", "/v1/seats")];
    assert_eq!(
        check(&served, vec![public_route("GET", "/v1/seats")]),
        [
            "POST /v1/seats: served by the daemon but missing from route-inventory.json (classify it)"
        ]
    );
    // axum serves HEAD through a GET handler, effects included, so an
    // implicit HEAD is a pair of its own.
    let served = [pair("GET", "/v1/inbox"), pair("HEAD", "/v1/inbox")];
    assert_eq!(
        check(&served, vec![seat_route("GET", "/v1/inbox")]),
        [
            "HEAD /v1/inbox: served by the daemon but missing from route-inventory.json (classify it)"
        ]
    );
}

#[test]
fn a_stale_or_repeated_entry_fails() {
    let served = [pair("POST", "/v1/send")];
    assert_eq!(
        check(
            &served,
            vec![
                seat_route("POST", "/v1/send"),
                seat_route("POST", "/v1/gone")
            ]
        ),
        ["POST /v1/gone: listed, but the daemon does not serve it (stale; remove it)"]
    );
    assert_eq!(
        check(
            &served,
            vec![
                seat_route("POST", "/v1/send"),
                public_route("POST", "/v1/send")
            ]
        ),
        ["POST /v1/send: listed twice"]
    );
}

#[test]
fn an_unclassified_or_misclassified_route_fails() {
    let served = [pair("POST", "/v1/send")];
    let with = |field: &str, value: Value| {
        let mut entry = seat_route("POST", "/v1/send");
        entry[field] = value;
        check(&served, vec![entry])
    };
    assert_eq!(
        with("class", Value::Null),
        [
            "POST /v1/send: seat-authority needs a class from [\"send-as\", \"inbox-claim\", \"role-and-holder\", \"outputs\", \"activity\", \"registration\", \"controls\", \"adoption\", \"attribution\"], not ``"
        ]
    );
    assert_eq!(
        with("grants_by", json!([])),
        ["POST /v1/send: grants_by must be a non-empty list"]
    );
    assert_eq!(
        with("guard", json!(["R", "R"])),
        ["POST /v1/send: guard lists R twice"]
    );
    assert_eq!(
        with("guard", json!(["Z"])),
        [
            "POST /v1/send: guard has \"Z\", not one of [\"R\", \"B\", \"N\", \"S\", \"C\", \"P\", \"A\"]"
        ]
    );
    assert_eq!(
        with("authority", json!("trusted")),
        [
            "POST /v1/send: authority must be machine-public, seat-authority or job-token, not `trusted`"
        ]
    );
    let mut public = public_route("POST", "/v1/send");
    public["reason"] = json!(" ");
    assert_eq!(
        check(&served, vec![public]),
        ["POST /v1/send: machine-public needs a reason"]
    );
}

#[test]
fn job_token_matches_the_routers_own_token_routes() {
    let emit = pair("POST", "/v1/bg/{job}/emit");
    let token: BTreeSet<Pair> = [emit.clone()].into();
    let served: BTreeSet<Pair> = [emit, pair("POST", "/v1/send")].into();
    let job = json!({"method": "POST", "path": "/v1/bg/{job}/emit",
        "authority": "job-token", "reason": "its own per-job secret"});
    assert!(
        inventory_findings(
            &served,
            &token,
            &json!({"routes": [job.clone(), seat_route("POST", "/v1/send")]})
        )
        .is_empty()
    );
    let mut send_as_token = job;
    send_as_token["path"] = json!("/v1/send");
    assert_eq!(
        inventory_findings(
            &served,
            &token,
            &json!({"routes": [public_route("POST", "/v1/bg/{job}/emit"), send_as_token]})
        ),
        [
            "POST /v1/bg/{job}/emit: the router authenticates this route by its own job token, so its authority must be job-token",
            "POST /v1/send: only a route the router authenticates by its own job token may be job-token",
        ]
    );
}

#[test]
fn a_malformed_inventory_fails() {
    assert_eq!(
        inventory_findings(&BTreeSet::new(), &BTreeSet::new(), &json!({"pairs": []})),
        ["route-inventory.json: routes must be a list"]
    );
    assert_eq!(
        check(&[], vec![json!({"path": "/v1/send"})]),
        ["routes[0]: needs a method and a path"]
    );
}

#[test]
fn operations_must_name_the_gated_classes_and_a_head_its_gets() {
    let served = [pair("POST", "/v1/send")];
    let with = |value: Value| {
        let mut entry = seat_route("POST", "/v1/send");
        entry["operations"] = value;
        check(&served, vec![entry])
    };
    assert_eq!(
        with(Value::Null),
        ["POST /v1/send: operations must be a non-empty list"]
    );
    assert_eq!(
        with(json!(["controls"])),
        ["POST /v1/send: operations must include its class `send-as`"]
    );
    assert!(with(json!(["send-as", "controls"])).is_empty());

    let served = [pair("GET", "/v1/inbox"), pair("HEAD", "/v1/inbox")];
    let get = seat_route("GET", "/v1/inbox");
    let mut head = seat_route("HEAD", "/v1/inbox");
    assert!(check(&served, vec![get.clone(), head.clone()]).is_empty());
    head["grants_by"] = json!(["seat", "parent"]);
    head["operations"] = json!(["send-as", "controls"]);
    assert_eq!(
        check(&served, vec![get, head]),
        ["HEAD /v1/inbox: must carry its GET's classification; differs in grants_by, operations"]
    );
}
