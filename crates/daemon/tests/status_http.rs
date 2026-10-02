//! One row-fact badge on both surfaces, independent of the card's live probe.
use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Event, Harness, ProcIdentity, SeatDescriptor, SemanticState};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use serde_json::{Value, json};

#[tokio::test]
async fn roster_and_card_share_badge_and_freshness_without_inventing_roster_liveness() {
    let store = FreshStore::new();
    let config = Config {
        store_path: store.path(),
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            ..Adapters::default()
        },
        ..Config::default()
    };
    let services =
        pij_daemon::build_services(&config, std::path::Path::new("/tmp/pij-status-http"))
            .await
            .unwrap();
    let mut seat = SeatDescriptor::new("pij-status-http", Harness::Omp, "/fixture");
    seat.semantic_state = Some(SemanticState::Done);
    seat.proc = Some(ProcIdentity {
        pid: 999_999,
        proc_start: 1,
    });
    services.registry.put(seat.clone()).await.unwrap();
    let registry = services.registry.clone();
    let spine = services.spine.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router_with_config(services, HttpConfig::local("status-key".into())),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let task: Value = client
        .post(format!("http://{addr}/v1/task"))
        .bearer_auth("status-key")
        .json(&json!({"caller":{"PIJ_SESSION_ID":seat.id},"argv":["task","set",seat.id,"blocked work"]}))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(task["ok"], true, "{task}");
    let assignment = task["data"]["task"]["id"].as_str().unwrap();
    for argv in [
        vec!["report", "state", "blocked", "--assignment", assignment],
        vec!["report", "state", "done"],
    ] {
        let receipt: Value = client
            .post(format!("http://{addr}/v1/report"))
            .bearer_auth("status-key")
            .json(&json!({"seat":seat.id,"argv":argv}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(receipt["ok"], true, "{receipt}");
    }
    spine
        .append(Event {
            seq: None,
            v: 1,
            at: 777,
            kind: "fixture.progress".into(),
            seat: Some(seat.id.clone()),
            payload: "{}".into(),
        })
        .await
        .unwrap();
    let roster: Value = client
        .get(format!("http://{addr}/v1/seats?scope=local"))
        .bearer_auth("status-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let card: Value = client
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("status-key")
        .json(&json!({"id":seat.id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = &roster["data"]["seats"][0];
    assert_eq!(row["badge"], "blocked");
    assert_eq!(
        row["semantic_state"], "done",
        "descriptor differs from its open assignment"
    );
    assert_eq!(card["data"]["semanticState"], "done");
    assert_eq!(row["last_event_at"], 777);
    assert!(row.get("liveness").is_none());
    assert_eq!(card["data"]["badge"], row["badge"]);
    assert_eq!(card["data"]["last_event_at"], row["last_event_at"]);
    assert_eq!(
        card["data"]["liveness"], "dead",
        "the independent card probe remains meaningful"
    );
    assert!(
        !card["data"]["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| matches!(
                entry["field"].as_str(),
                Some("lastEventAt" | "last_event_at")
            ))
    );
    registry
        .tombstone(&seat.id, "operator closed")
        .await
        .unwrap();
    let roster: Value = client
        .get(format!("http://{addr}/v1/seats?scope=local"))
        .bearer_auth("status-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(roster["data"]["seats"], json!([]));
    let card: Value = client
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("status-key")
        .json(&json!({"id":seat.id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        card["data"]["badge"], "blocked",
        "tombstones do not alter the badge's definition"
    );
    server.abort();
    let _ = server.await;
}
