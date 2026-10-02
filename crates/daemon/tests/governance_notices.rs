//! Native HTTP + shared SQL notice proofs. Hosts are scripted, not model turns.
//! The canonical matrix requires PM's contracts-delta-u4-native-notice.json merge.
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{
    DeliveryOrigin, DeliveryOutcome, Event, Harness, Msg, ProcIdentity, SeatDescriptor, SeatId,
    Seq, SystemState,
};
use pij_core::orchestration::BatonDefinition;
use pij_core::ports::Spine;
use pij_daemon::Services;
use pij_daemon::delivery::{DeliveryService, NativeInboxIdentity};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_store::{SqliteOrchestration, SqliteSpine, StorePool};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTransport};
use serde::Deserialize;
use serde_json::{Value, json};

const KEY: &str = "private-native-keeper-notice-key";
const KEEPER_PROC: ProcIdentity = ProcIdentity {
    pid: 17001,
    proc_start: 20260907120000,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Evidence {
    Active,
    Recycled,
    Dead,
    Missing,
    Dissolved,
}

#[derive(Deserialize)]
struct KeeperCase {
    harness: Harness,
    state: SystemState,
    evidence: Evidence,
    notice: Option<String>,
    send_calls: usize,
    persisted_messages: usize,
}

fn fixture_case(id: &str) -> (Value, KeeperCase) {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .expect("canonical routes");
    let rows = fixtures["baton_notice_cases"]
        .as_array()
        .expect("PM merges native notice matrix");
    assert_eq!(rows.len(), 8, "retain every required keeper case");
    let case = serde_json::from_value(
        rows.iter()
            .find(|row| row["id"] == id)
            .expect("keeper case")
            .clone(),
    )
    .expect("typed keeper case");
    (fixtures, case)
}

fn route_case(fixtures: &Value, id: &str) -> Value {
    fixtures["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .flat_map(|route| route["cases"].as_array().expect("cases"))
        .find(|case| case["id"] == id)
        .expect("canonical route case")
        .clone()
}

struct Fixture {
    server: tokio::task::JoinHandle<()>,
    addr: SocketAddr,
    services: Services,
    transport: Arc<FakeTransport>,
    pool: StorePool,
    worker: SeatId,
    keeper: SeatId,
    fixtures: Value,
    request_case: Value,
    case: KeeperCase,
    _file: FreshStore,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(fixtures: Value, case: KeeperCase, transport: FakeTransport) -> Self {
        let file = FreshStore::new();
        let config = Config {
            store_path: file.path(),
            adapters: Adapters {
                registry: AdapterChoice::Real,
                queue: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                ..Adapters::default()
            },
            ..Config::default()
        };
        let mut services =
            pij_daemon::build_services(&config, Path::new("/unused-native-notice-fake-tmux"))
                .await
                .expect("real shared SQL services");
        // Separate reader connection proves durable database state, not fake maps.
        let pool = pij_store::open(&file.path())
            .await
            .expect("independent SQL reader");
        let baton: BatonDefinition = serde_json::from_value(
            route_case(&fixtures, "baton-define")["response"]["data"]["baton"].clone(),
        )
        .expect("baton fixture");
        assert!(
            SqliteOrchestration::new(pool.clone())
                .define_baton(&baton)
                .await
                .expect("historical baton definition")
        );
        let request_case = route_case(&fixtures, "baton-request");
        let worker = SeatId::from(
            fixtures["fixture_context"]["worker"]
                .as_str()
                .expect("worker"),
        );
        let keeper = baton.created_by;
        let mut requester = SeatDescriptor::new(
            worker.clone(),
            Harness::Omp,
            baton.repo.clone().expect("repo"),
        );
        requester.pane = Some(
            request_case["request"]["caller"]["TMUX_PANE"]
                .as_str()
                .expect("requester pane")
                .into(),
        );
        services.registry.put(requester).await.expect("requester");
        if !matches!(case.evidence, Evidence::Missing) {
            let mut target =
                SeatDescriptor::new(keeper.clone(), case.harness, baton.repo.expect("repo"));
            target.proc = Some(KEEPER_PROC);
            target.state = case.state;
            target.harness_session = Some("native-keeper-session".into());
            target.native_extension_delivery = case.harness == Harness::Copilot;
            target.cross_session_inbound_accept = Some(true);
            if case.harness == Harness::Pi {
                target.pane = Some(
                    route_case(&fixtures, "baton-define")["request"]["caller"]["TMUX_PANE"]
                        .as_str()
                        .expect("keeper pane")
                        .into(),
                );
            }
            services.registry.put(target).await.expect("keeper");
            if matches!(case.evidence, Evidence::Dissolved) {
                services
                    .registry
                    .tombstone(&keeper, "dissolved")
                    .await
                    .expect("keeper tombstone");
            }
        }
        let liveness = match case.evidence {
            Evidence::Active | Evidence::Dissolved => FakeLiveness::new().with_proc(KEEPER_PROC),
            Evidence::Recycled => {
                FakeLiveness::new().with_recycled(KEEPER_PROC.pid, KEEPER_PROC.proc_start + 1)
            }
            Evidence::Dead | Evidence::Missing => FakeLiveness::new(),
        };
        services.liveness = Arc::new(liveness);
        let transport = Arc::new(transport);
        services.transport = transport.clone();
        services.delivery = Arc::new(
            DeliveryService::new(
                services.registry.clone(),
                services.queue.clone(),
                services.transport.clone(),
                services.interaction.clone(),
                services.event_bus.clone(),
            )
            .expect("same queue/spine sender"),
        );
        let router = router_with_config(services.clone(), HttpConfig::local(KEY.into()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("private listener");
        let addr = listener.local_addr().expect("private address");
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("native HTTP");
        });
        Self {
            server,
            addr,
            services,
            transport,
            pool,
            worker,
            keeper,
            fixtures,
            request_case,
            case,
            _file: file,
        }
    }

    async fn post(&self, request: &Value) -> (reqwest::StatusCode, Value) {
        let response = reqwest::Client::new()
            .post(format!("http://{}/v1/orchestration", self.addr))
            .timeout(Duration::from_secs(5))
            .bearer_auth(KEY)
            .json(request)
            .send()
            .await
            .expect("native request completes without bus deadlock");
        let status = response.status();
        (status, response.json().await.expect("complete v2 envelope"))
    }

    async fn history(&self) -> Vec<Event> {
        SqliteSpine::new(self.pool.clone())
            .tail(None, Seq(0))
            .await
            .expect("durable spine readback")
    }

    async fn jobs(&self) -> Vec<(i64, String, String)> {
        sqlx::query_as(
            "SELECT id, state, payload FROM jobs WHERE kind LIKE 'delivery:%' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .expect("all durable delivery messages")
    }

    async fn assert_committed(&self, data: &Value, requester: &SeatId) -> Vec<Event> {
        let request = &data["request"];
        let mut expected = self.request_case["response"]["data"]["request"].clone();
        expected["id"] = request["id"].clone();
        expected["requested_at"] = request["requested_at"].clone();
        expected["requester"] = json!(requester);
        assert_eq!(request, &expected, "request preserves all canonical fields");
        let stored = SqliteOrchestration::new(self.pool.clone())
            .baton_request(request["id"].as_str().expect("request id"))
            .await
            .expect("read request")
            .expect("request survived");
        assert_eq!(
            serde_json::to_value(stored).expect("request JSON"),
            expected
        );
        let history = self.history().await;
        let opened: Vec<_> = history
            .iter()
            .filter(|event| event.kind == "baton.requested")
            .collect();
        assert_eq!(opened.len(), 1);
        assert_eq!(
            opened[0].seq.expect("request seq").0,
            data["seq"].as_u64().expect("returned seq")
        );
        assert_eq!(
            serde_json::from_str::<Value>(&opened[0].payload).expect("event payload"),
            json!({"actor":requester,"action":"requested","record":request})
        );
        history
    }

    fn notice_text(&self, data: &Value) -> String {
        let request = &data["request"];
        self.fixtures["baton_notice_body_template"]
            .as_str()
            .expect("canonical legacy notice text")
            .replace("{baton}", request["baton"].as_str().expect("baton"))
            .replace(
                "{requester}",
                request["requester"].as_str().expect("requester"),
            )
            .replace("{purpose}", request["purpose"].as_str().expect("purpose"))
            .replace("{request_id}", request["id"].as_str().expect("id"))
    }
}

async fn assert_queued_case(id: &str) -> (Fixture, Value) {
    let (fixtures, case) = fixture_case(id);
    let f = Fixture::new(fixtures, case, FakeTransport::unreachable()).await;
    let (status, response) = f.post(&f.request_case["request"]).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{response}");
    let data = &response["data"];
    let history = f.assert_committed(data, &f.worker).await;
    assert_eq!(
        response,
        json!({"ok":true,"command":"pij orchestration","v":2,
        "data":{"request":data["request"],"seq":data["seq"],"notice":f.case.notice}})
    );
    let jobs = f.jobs().await;
    assert_eq!(
        jobs.len(),
        f.case.persisted_messages,
        "native persisted message count"
    );
    let pushed: Vec<_> = history
        .iter()
        .filter(|event| event.kind == "message.pushed")
        .collect();
    let outcomes: Vec<_> = history
        .iter()
        .filter(|event| event.kind == "delivery.outcome")
        .collect();
    assert_eq!(
        pushed.len(),
        f.case.send_calls,
        "one recipient-facing admission per send"
    );
    assert_eq!(
        outcomes.len(),
        f.case.send_calls,
        "one actual receipt per send"
    );
    assert!(
        f.transport.delivered().is_empty(),
        "queue admission is not transport delivery"
    );
    for (_, state, payload) in &jobs {
        assert_eq!(state, "pending");
        let message: Msg = serde_json::from_str(payload).expect("real inbox payload");
        assert_eq!(message.from, f.worker);
        assert_eq!(message.to, f.keeper);
        assert_eq!(message.body, f.notice_text(data));
        assert!(message.command.is_none());
        assert!(message.in_reply_to.is_none());
        assert!(message.from_machine.is_none());
        let push: Value = serde_json::from_str(&pushed[0].payload).expect("pushed payload");
        assert_eq!(push["msg_id"], message.msg_id);
        assert_eq!(push["body"], message.body);
        assert_eq!(push["from"], json!(f.worker));
        assert_eq!(pushed[0].seat.as_ref(), Some(&f.keeper));
        assert!(pushed[0].seq.expect("push seq").0 > data["seq"].as_u64().expect("request seq"));
        let outcome: Value = serde_json::from_str(&outcomes[0].payload).expect("delivery receipt");
        assert_eq!(outcome["msg_id"], message.msg_id);
        assert_eq!(outcome["outcome"]["outcome"], "queued");
    }
    if f.case.send_calls == 0 {
        assert!(f.transport.calls().is_empty());
    }
    (f, response)
}

#[tokio::test]
async fn keeper_notice_idle_pi_queues_until_native_reader_ack() {
    // Prime's #374 honest-receipt rule: idle Pi admission is queued, not delivered.
    let (f, response) = assert_queued_case("idle-live-pi").await;
    assert_eq!(response["data"]["notice"], "queued");
    let inbox = f
        .services
        .delivery
        .claim_inbox(&f.keeper, false)
        .await
        .expect("native reader claims");
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].message.body, f.notice_text(&response["data"]));
    let ack = f
        .services
        .delivery
        .acknowledge_inbox(
            &f.keeper,
            inbox[0].job_id,
            &NativeInboxIdentity::default(),
            None,
        )
        .await
        .expect("reader acknowledges");
    assert_eq!(ack.origin, DeliveryOrigin::ReaderRead);
    assert_eq!(ack.msg_id, inbox[0].message.msg_id);
    let jobs = f.jobs().await;
    assert_eq!(jobs.len(), 1, "ack does not create another message");
    assert_eq!(jobs[0].1, "done");
    let history = f.history().await;
    let delivered = history
        .iter()
        .filter(|event| event.kind == "delivery.outcome")
        .map(|event| serde_json::from_str::<Value>(&event.payload).expect("outcome"))
        .find(|payload| payload["outcome"]["outcome"] == "delivered")
        .expect("actual later delivery receipt");
    assert_eq!(delivered["outcome"]["origin"], "reader-read");
    assert_eq!(delivered["msg_id"], ack.msg_id);
}

#[tokio::test]
async fn keeper_notice_working_pi_queues_once() {
    assert_queued_case("working-live-pi").await;
}
#[tokio::test]
async fn keeper_notice_fresh_external_queues_once() {
    assert_queued_case("fresh-live-control-plane").await;
}
#[tokio::test]
async fn keeper_notice_stale_external_uses_recycled_incarnation() {
    assert_queued_case("stale-live-control-plane").await;
}
#[tokio::test]
async fn keeper_notice_dead_keeper_persists_unverified_message() {
    assert_queued_case("dead-keeper").await;
}
#[tokio::test]
async fn keeper_notice_missing_keeper_sends_nothing() {
    assert_queued_case("missing-keeper").await;
}
#[tokio::test]
async fn keeper_notice_dissolved_pull_keeper_sends_nothing() {
    assert_queued_case("dissolved-pull-keeper").await;
}
#[tokio::test]
async fn keeper_notice_live_bound_pull_queues_once() {
    assert_queued_case("live-bound-pull-keeper").await;
}

#[tokio::test]
async fn keeper_notice_socket_delivery_requires_transport_confirmation() {
    let (fixtures, mut case) = fixture_case("fresh-live-control-plane");
    case.harness = Harness::Claude;
    let f = Fixture::new(fixtures, case, FakeTransport::reachable()).await;
    let (status, response) = f.post(&f.request_case["request"]).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{response}");
    let history = f.assert_committed(&response["data"], &f.worker).await;
    assert_eq!(response["data"]["notice"], "delivered");
    let sent = f.transport.delivered();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from, f.worker);
    assert_eq!(sent[0].to, f.keeper);
    assert_eq!(sent[0].body, f.notice_text(&response["data"]));
    assert!(
        f.jobs().await.is_empty(),
        "confirmed socket delivery is not a second queue"
    );
    let outcomes: Vec<_> = history
        .iter()
        .filter(|event| event.kind == "delivery.outcome")
        .collect();
    assert_eq!(outcomes.len(), 1);
    let outcome: Value = serde_json::from_str(&outcomes[0].payload).expect("receipt");
    assert_eq!(outcome["msg_id"], sent[0].msg_id);
    assert_eq!(outcome["outcome"]["origin"], "injected-to-transport");
    assert!(
        outcomes[0].seq.expect("delivery seq").0
            > response["data"]["seq"].as_u64().expect("request seq")
    );
}

#[tokio::test]
async fn keeper_notice_transport_failure_keeps_committed_request_and_seq() {
    let (fixtures, mut case) = fixture_case("fresh-live-control-plane");
    case.harness = Harness::Claude;
    let f = Fixture::new(
        fixtures,
        case,
        FakeTransport::reachable().script_deliver_error(),
    )
    .await;
    let (status, response) = f.post(&f.request_case["request"]).await;
    assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response["ok"], false);
    assert_eq!(response["command"], "pij orchestration");
    assert_eq!(response["v"], 2);
    assert_eq!(response["details"]["code"], "E-RS-PARTIAL");
    assert_eq!(response["details"]["committed"], true);
    assert_eq!(response["details"]["event_published"], true);
    f.assert_committed(&response["details"], &f.worker).await;
    assert!(f.jobs().await.is_empty());
    assert_eq!(
        f.transport
            .calls()
            .iter()
            .filter(|call| call.starts_with("deliver:"))
            .count(),
        1
    );
}

#[tokio::test]
async fn keeper_notice_held_and_refused_are_not_fabricated_success() {
    for outcome in [
        DeliveryOutcome::Held {
            reason: "operator approval pending".into(),
        },
        DeliveryOutcome::Refused {
            reason: "operator declined".into(),
        },
    ] {
        let (fixtures, mut case) = fixture_case("fresh-live-control-plane");
        case.harness = Harness::Claude;
        let f = Fixture::new(
            fixtures,
            case,
            FakeTransport::reachable().script_outcome(outcome.clone()),
        )
        .await;
        let (status, response) = f.post(&f.request_case["request"]).await;
        assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response["details"]["code"], "E-RS-PARTIAL");
        let history = f.assert_committed(&response["details"], &f.worker).await;
        let event = history
            .iter()
            .find(|event| event.kind == "delivery.outcome")
            .expect("honest transport receipt");
        assert_eq!(
            serde_json::from_str::<Value>(&event.payload).expect("payload")["outcome"],
            json!(outcome)
        );
        assert!(response.get("data").is_none());
    }
}

#[tokio::test]
async fn keeper_notice_self_send_keeps_existing_refusal() {
    let (fixtures, case) = fixture_case("idle-live-pi");
    let f = Fixture::new(fixtures, case, FakeTransport::unreachable()).await;
    let mut request = f.request_case["request"].clone();
    request["caller"] = route_case(&f.fixtures, "baton-define")["request"]["caller"].clone();
    let (status, response) = f.post(&request).await;
    assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response["details"]["code"], "E-RS-PARTIAL");
    f.assert_committed(&response["details"], &f.keeper).await;
    assert!(f.jobs().await.is_empty());
    assert!(f.transport.calls().is_empty());
}

#[tokio::test]
async fn keeper_notice_publication_failure_prevents_send() {
    let (fixtures, case) = fixture_case("idle-live-pi");
    let f = Fixture::new(fixtures, case, FakeTransport::unreachable()).await;
    sqlx::query("CREATE TRIGGER fail_baton_notice_event BEFORE INSERT ON spine_events WHEN NEW.kind='baton.requested' BEGIN SELECT RAISE(ABORT, 'publication failure'); END")
        .execute(&f.pool).await.expect("inject publication fault");
    let (status, response) = f.post(&f.request_case["request"]).await;
    assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response["details"]["code"], "E-RS-PARTIAL");
    assert_eq!(response["details"]["committed"], true);
    assert_eq!(response["details"]["event_published"], false);
    let id = response["details"]["record"]["id"]
        .as_str()
        .expect("committed request id");
    assert!(
        SqliteOrchestration::new(f.pool.clone())
            .baton_request(id)
            .await
            .expect("read")
            .is_some()
    );
    assert!(f.jobs().await.is_empty());
    assert!(f.transport.calls().is_empty());
    assert!(!f.history().await.iter().any(|event| matches!(
        event.kind.as_str(),
        "baton.requested" | "message.pushed" | "delivery.outcome"
    )));
}
