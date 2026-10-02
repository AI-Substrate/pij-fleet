//! Retired identity receipts enrich refusals only; they never restore authority.

use std::net::SocketAddr;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeSpine;
use serde_json::{Value, json};

const ROUTES: &str = include_str!("../../testkit/fixtures/golden/api/governance-routes.json");
const EVENTS: &str = include_str!("../../testkit/fixtures/golden/api/governance-events.json");
const KEY: &str = "retired-identity-fixture-key";

fn context() -> Value {
    serde_json::from_str::<Value>(ROUTES).expect("canonical routes")["fixture_context"].clone()
}
fn event_case(id: &str) -> Value {
    let events: Value = serde_json::from_str(EVENTS).expect("canonical events");
    events["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|case| case["id"] == id)
        .expect("event case")
        .clone()
}
fn worker() -> SeatId {
    SeatId::from(context()["worker"].as_str().expect("worker"))
}
fn pane() -> String {
    event_case("seat-put")["decoded_payload"]["pane"]
        .as_str()
        .expect("worker pane")
        .to_string()
}

struct Fixture {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    registry: Arc<dyn Registry>,
    persisted_spine: Arc<dyn Spine>,
    _directory: FreshStore,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(history_read_fault: Option<Arc<dyn Spine>>) -> Self {
        let directory = FreshStore::new();
        let config = Config {
            adapters: Adapters {
                registry: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                queue: AdapterChoice::Real,
                ..Adapters::default()
            },
            store_path: directory.path(),
            ..Config::default()
        };
        let signals = std::path::Path::new(&directory.path()).with_extension("signals");
        let mut services = pij_daemon::build_services(&config, &signals)
            .await
            .expect("services");
        let seat: SeatDescriptor =
            serde_json::from_value(event_case("seat-put")["decoded_payload"].clone())
                .expect("worker descriptor");
        // A pane answers only for a seat whose recorded process runs (plan 156).
        services.liveness = Arc::new(
            pij_testkit::fakes::FakeLiveness::new().with_proc(seat.proc.expect("worker proc")),
        );
        services.registry.put(seat).await.expect("seed live worker");
        let registry = services.registry.clone();
        let persisted_spine = services.spine.clone();
        if let Some(history) = history_read_fault {
            // Fault injection only for the refusal's history read. No request in
            // this fixture may use the substituted history to authorize a write.
            services.spine = history;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("private listener");
        let addr = listener.local_addr().expect("address");
        let app = router_with_config(services, HttpConfig::local(KEY.to_string()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self {
            addr,
            server,
            registry,
            persisted_spine,
            _directory: directory,
        }
    }
    async fn retire(&self) -> Seq {
        let case = event_case("seat-tombstone");
        self.registry
            .tombstone(
                &worker(),
                case["decoded_payload"]["reason"].as_str().expect("reason"),
            )
            .await
            .expect("retire worker")
    }
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth(KEY)
            .json(&body)
            .send()
            .await
            .expect("HTTP request");
        let status = response.status().as_u16();
        let body = response.json().await.expect("decodable envelope");
        (status, body)
    }
    async fn original_refusal(&self, path: &str, caller: Value, seq: Seq) {
        let (status, response) = self
            .post(
                path,
                json!({"caller":caller,"argv":["report","state","ready"]}),
            )
            .await;
        assert_eq!(status, 400, "{response}");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"], "refused");
        assert_eq!(response["details"]["seat"], json!(worker()));
        assert_eq!(response["details"]["tombstone_seq"], seq.0);
        assert!(
            response["meta"]
                .as_str()
                .expect("meta")
                .contains(&seq.0.to_string())
        );
    }
}

#[tokio::test]
async fn retired_asserted_and_pane_callers_receive_the_original_tombstone_sequence_without_authority()
 {
    let fixture = Fixture::new(None).await;
    let seq = fixture.retire().await;
    let before = fixture
        .persisted_spine
        .tail(None, Seq(0))
        .await
        .expect("events");
    let descriptor = fixture
        .registry
        .get(&worker())
        .await
        .expect("registry")
        .expect("retired row");
    for path in ["/v1/whoami", "/v1/phonehome", "/v1/report"] {
        fixture
            .original_refusal(path, json!({"PIJ_SESSION_ID":worker()}), seq)
            .await;
        fixture
            .original_refusal(path, json!({"TMUX_PANE":pane()}), seq)
            .await;
    }
    assert_eq!(
        fixture.registry.get(&worker()).await.expect("registry"),
        Some(descriptor)
    );
    assert_eq!(
        fixture
            .persisted_spine
            .tail(None, Seq(0))
            .await
            .expect("events"),
        before,
        "refusal metadata must not publish, resurrect, or grant a report write"
    );
}

#[tokio::test]
async fn retired_lookup_selects_latest_tombstone_not_a_later_unrelated_event() {
    let fixture = Fixture::new(None).await;
    let first = fixture.retire().await;
    let seq = fixture
        .registry
        .tombstone(&worker(), "second recorded retirement")
        .await
        .expect("latest tombstone");
    assert!(
        seq > first,
        "fixture must contain two distinct tombstone events"
    );
    let descriptor = fixture
        .registry
        .get(&worker())
        .await
        .expect("registry")
        .expect("retired row");
    let later = fixture
        .registry
        .put(descriptor)
        .await
        .expect("later seat snapshot");
    assert!(later > seq);
    fixture
        .original_refusal("/v1/whoami", json!({"PIJ_SESSION_ID":worker()}), seq)
        .await;
}

#[tokio::test]
async fn ambiguous_retired_pane_history_refuses_without_guessing_a_tombstone() {
    let fixture = Fixture::new(None).await;
    fixture.retire().await;
    let mut other = fixture
        .registry
        .get(&worker())
        .await
        .expect("registry")
        .expect("retired worker");
    other.id = SeatId::from(context()["outsider"].as_str().expect("other retired id"));
    fixture
        .registry
        .put(other)
        .await
        .expect("another retired row in same pane history");
    let before = fixture
        .persisted_spine
        .tail(None, Seq(0))
        .await
        .expect("events");
    let (status, response) = fixture
        .post("/v1/whoami", json!({"caller":{"TMUX_PANE":pane()}}))
        .await;
    assert_eq!(status, 400, "{response}");
    assert_eq!(response["ok"], false);
    assert!(response["details"]["tombstone_seq"].is_null());
    assert!(response["details"]["seat"].is_null());
    assert_eq!(
        fixture
            .persisted_spine
            .tail(None, Seq(0))
            .await
            .expect("events"),
        before
    );
}

#[tokio::test]
async fn contradicted_retired_pane_claim_refuses_without_attributing_its_history() {
    let fixture = Fixture::new(None).await;
    fixture.retire().await;
    let (status, response) = fixture
        .post(
            "/v1/whoami",
            json!({"caller":{
                "TMUX_PANE":pane(),"PIJ_SESSION_ID":context()["outsider"]
            }}),
        )
        .await;
    assert_eq!(status, 400, "{response}");
    assert_eq!(response["ok"], false);
    assert!(response["details"]["tombstone_seq"].is_null());
    assert!(response["details"]["seat"].is_null());
}

#[tokio::test]
async fn retired_rows_without_historical_tombstone_events_do_not_invent_sequences() {
    let fixture = Fixture::new(None).await;
    let mut retired = fixture
        .registry
        .get(&worker())
        .await
        .expect("registry")
        .expect("worker");
    retired.tombstoned_at = context()["at_ms"].as_u64();
    retired.tombstone_reason = Some(
        event_case("seat-tombstone")["decoded_payload"]["reason"]
            .as_str()
            .expect("reason")
            .to_string(),
    );
    fixture
        .registry
        .put(retired)
        .await
        .expect("legacy retired snapshot without tombstone event");
    assert!(
        fixture
            .persisted_spine
            .latest_matching(&worker(), &["seat.tombstone"])
            .await
            .expect("bounded history")
            .is_none()
    );
    for caller in [
        json!({"PIJ_SESSION_ID":worker()}),
        json!({"TMUX_PANE":pane()}),
    ] {
        let (status, response) = fixture.post("/v1/whoami", json!({"caller":caller})).await;
        assert_eq!(status, 400, "{response}");
        assert_eq!(response["ok"], false);
        assert_eq!(response["details"]["seat"], json!(worker()));
        assert!(
            response["details"].get("tombstone_seq").is_none(),
            "no fabricated fallback sequence: {response}"
        );
    }
}

#[tokio::test]
async fn tombstone_history_failure_stays_a_named_authentication_refusal() {
    let history = Arc::new(FakeSpine::new());
    let fixture = Fixture::new(Some(history.clone())).await;
    fixture.retire().await;
    for caller in [
        json!({"PIJ_SESSION_ID":worker()}),
        json!({"TMUX_PANE":pane()}),
    ] {
        history.script_latest_matching_error("retired-history-read-failure");
        let (status, response) = fixture.post("/v1/whoami", json!({"caller":caller})).await;
        assert_eq!(
            status, 400,
            "history failure must not grant authority or change retirement refusal: {response}"
        );
        assert_eq!(response["ok"], false);
        assert_eq!(response["details"]["seat"], json!(worker()));
        assert!(response["details"].get("tombstone_seq").is_none());
        assert!(
            response["meta"]
                .as_str()
                .expect("meta")
                .contains("retired-history-read-failure")
        );
    }
}

#[tokio::test]
async fn live_pane_resolution_and_replacement_contradictions_keep_existing_behavior() {
    let fixture = Fixture::new(None).await;
    let (status, live) = fixture
        .post("/v1/whoami", json!({"caller":{"TMUX_PANE":pane()}}))
        .await;
    assert_eq!(status, 200, "{live}");
    assert_eq!(live["data"]["id"], json!(worker()));
    let seq = fixture.retire().await;
    let mut replacement: SeatDescriptor =
        serde_json::from_value(event_case("seat-put")["decoded_payload"].clone())
            .expect("replacement template");
    replacement.id = SeatId::from(context()["outsider"].as_str().expect("replacement"));
    fixture
        .registry
        .put(replacement.clone())
        .await
        .expect("live pane owner");
    let (status, live) = fixture
        .post("/v1/whoami", json!({"caller":{"TMUX_PANE":pane()}}))
        .await;
    assert_eq!(status, 200, "existing live identity behavior: {live}");
    assert_eq!(live["data"]["id"], json!(replacement.id));
    let (status, refused) = fixture
        .post(
            "/v1/whoami",
            json!({"caller":{"TMUX_PANE":pane(),"PIJ_SESSION_ID":worker()}}),
        )
        .await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["ok"], false);
    assert!(
        refused["data"].is_null(),
        "retired assertion cannot select replacement"
    );
    fixture
        .original_refusal("/v1/whoami", json!({"PIJ_SESSION_ID":worker()}), seq)
        .await;
}
