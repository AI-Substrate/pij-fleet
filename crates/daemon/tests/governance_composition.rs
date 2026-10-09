//! Plan 139 composition proof: real SQLite and HTTP, fake external host ports.
//! This is baseline-compatible: missing governance routes fail at HTTP, not compilation.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId, Seq};
use pij_core::ports::Spine;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use serde_json::{Value, json};

const KEY: &str = "plan-139-composition-key";
const PARENT: &str = "pij-parent";
const WORKER: &str = "pij-worker";
const OUTSIDER: &str = "pij-outsider";

fn contract() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../testkit/fixtures/golden/api/governance-routes.json"
        ))
        .expect("canonical route contract")
    })
}

fn case(id: &str) -> &'static Value {
    contract()["routes"]
        .as_array()
        .expect("route array")
        .iter()
        .flat_map(|route| route["cases"].as_array().expect("case array"))
        .find(|case| case["id"] == id)
        .expect("named canonical fixture")
}

struct Fixture {
    addr: SocketAddr,
    client: reqwest::Client,
    server: tokio::task::JoinHandle<()>,
    spine: Arc<dyn Spine>,
    seeded_at: Seq,
    _store: FreshStore,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with_adapters(Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        })
        .await
    }

    async fn start_with_adapters(adapters: Adapters) -> Self {
        let store = FreshStore::new();
        let config = Config {
            store_path: store.path(),
            adapters,
            ..Config::default()
        };
        let services =
            pij_daemon::build_services(&config, std::path::Path::new("/unused-fake-tmux-plan-139"))
                .await
                .expect("coherent SQL event domain and fake external hosts");
        let mut seeded_at = Seq(0);
        for (id, pane, parent) in [
            (PARENT, "%10", None),
            (WORKER, "%11", Some(PARENT)),
            (OUTSIDER, "%12", None),
        ] {
            let mut seat = SeatDescriptor::new(id, Harness::Omp, "/work/rs-governance-port");
            seat.pane = Some(pane.to_string());
            seat.parent = parent.map(SeatId::from);
            seat.harness_session = Some(format!("fixture-{id}"));
            if id == WORKER {
                // After unset, a stale descriptor field must not resurrect the role.
                seat.role = Some("obsolete-descriptor-role".to_string());
            }
            seeded_at = services.registry.put(seat).await.expect("seed descriptor");
        }
        let spine = Arc::clone(&services.spine);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("ephemeral loopback");
        let addr = listener.local_addr().expect("bound address");
        let router = router_with_config(
            services,
            HttpConfig {
                auth: pij_daemon::http::AuthRing::local(KEY.to_string()),
                machine_alias: "fixture-machine".to_string(),
            },
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("HTTP server");
        });
        Self {
            addr,
            client: reqwest::Client::new(),
            server,
            spine,
            seeded_at,
            _store: store,
        }
    }

    async fn post(&self, path: &str, body: &Value) -> (reqwest::StatusCode, Value) {
        let response = self
            .client
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth(KEY)
            .json(body)
            .send()
            .await
            .expect("POST response");
        let status = response.status();
        let bytes = response.bytes().await.expect("POST bytes");
        let envelope = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{path}: {status}: {error}: {bytes:?}"));
        (status, envelope)
    }

    async fn success(&self, path: &str, body: &Value) -> Value {
        let (status, envelope) = self.post(path, body).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{path}: {envelope}");
        assert_eq!(envelope["ok"], true, "{path}: {envelope}");
        assert_eq!(envelope["v"], 2);
        envelope["data"].clone()
    }

    async fn get(&self, path: &str) -> Value {
        let response = self
            .client
            .get(format!("http://{}{path}", self.addr))
            .bearer_auth(KEY)
            .send()
            .await
            .expect("GET response");
        let status = response.status();
        let envelope: Value = response.json().await.expect("GET envelope");
        assert_eq!(status, reqwest::StatusCode::OK, "{path}: {envelope}");
        assert_eq!(envelope["ok"], true, "{path}: {envelope}");
        envelope["data"].clone()
    }

    async fn stream(&self, replay: bool) -> Frames {
        let mut request = self
            .client
            .get(format!("http://{}/v1/events", self.addr))
            .bearer_auth(KEY);
        if replay {
            request = request.query(&[(
                "since",
                json!({"fixture-machine": self.seeded_at.0}).to_string(),
            )]);
        }
        let mut frames = Frames {
            response: request.send().await.expect("event subscription"),
            pending: Vec::new(),
        };
        assert_eq!(frames.next().await["hello"], true);
        frames
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct Frames {
    response: reqwest::Response,
    pending: Vec<u8>,
}

impl Frames {
    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                    let frame = serde_json::from_slice(&self.pending[..end])
                        .expect("one complete NDJSON frame");
                    self.pending.drain(..=end);
                    return frame;
                }
                let chunk = self
                    .response
                    .chunk()
                    .await
                    .expect("stream chunk")
                    .expect("stream remains open");
                self.pending.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("live event before timeout")
    }

    async fn at(&mut self, seq: u64) -> Value {
        loop {
            let frame = self.next().await;
            let Some(cursor) = frame["cursor"].as_u64() else {
                continue;
            };
            assert!(
                cursor <= seq,
                "cursor advanced past unemitted {seq}: {frame}"
            );
            if cursor == seq {
                return frame;
            }
        }
    }
}

#[tokio::test]
async fn direct_and_orchestration_roles_share_one_writer_and_live_read_join() {
    let fixture = Fixture::start().await;
    let mut live = fixture.stream(false).await;
    let mut replay = fixture.stream(true).await;
    let role = fixture
        .success("/v1/role", &case("role-set")["request"])
        .await;
    assert_eq!(role["assigned_by"], PARENT);
    let seq = role["seq"].as_u64().expect("role spine receipt");
    for stream in [&mut live, &mut replay] {
        let frame = stream.at(seq).await;
        assert_eq!(frame["event"]["kind"], "role-set");
        assert!(frame["event"]["at"].as_u64().expect("timestamp") > 0);
        let payload: Value = serde_json::from_str(
            frame["event"]["payload"]
                .as_str()
                .expect("JSON-string payload"),
        )
        .expect("decodable role payload");
        assert_eq!(payload["actor"], PARENT);
        assert_eq!(payload["record"]["role"], role["role"]);
    }
    let state = fixture.success("/v1/state", &json!({"id": WORKER})).await;
    assert_eq!(state["role"], role["role"]);

    fixture
        .success("/v1/orchestration", &case("role-unset")["request"])
        .await;
    let state = fixture.success("/v1/state", &json!({"id": WORKER})).await;
    assert!(
        state["role"].is_null(),
        "unset cannot resurrect descriptor role"
    );
    let roster = fixture.get("/v1/seats").await;
    let worker = roster["seats"]
        .as_array()
        .expect("roster")
        .iter()
        .find(|seat| seat["id"] == WORKER)
        .expect("worker remains in roster");
    assert!(worker["role"].is_null());

    let mut outsider = case("role-set")["request"].clone();
    outsider["caller"]["TMUX_PANE"] = json!("%12");
    let before = fixture.spine.tail(None, Seq(0)).await.expect("spine").len();
    let (status, refused) = fixture.post("/v1/role", &outsider).await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert_eq!(refused["details"]["code"], "E-RS-OWNERSHIP");
    assert_eq!(
        fixture.spine.tail(None, Seq(0)).await.expect("spine").len(),
        before
    );
}

#[tokio::test]
async fn incompatible_real_registry_and_fake_spine_refuses_before_opening_storage() {
    let store = FreshStore::new();
    let config = Config {
        store_path: store.path(),
        adapters: Adapters {
            registry: AdapterChoice::Real,
            ..Adapters::default()
        },
        ..Config::default()
    };
    let error =
        match pij_daemon::build_services(&config, std::path::Path::new("/unused-fake-signals"))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("mixed registry/spine sequence domains must refuse"),
        };
    assert!(error.to_string().contains("E-RS-BACKEND-PAIR"));
    assert!(
        !store.exists(),
        "refusal must precede opening or migrating storage"
    );
}

#[tokio::test]
async fn all_fake_governance_and_messages_replay_one_memory_sequence_domain() {
    let fixture = Fixture::start_with_adapters(Adapters::default()).await;
    let parent = json!({"PIJ_SESSION_ID": PARENT, "cwd": "/work/project"});
    let worker = json!({"PIJ_SESSION_ID": WORKER, "cwd": "/work/project"});
    fixture.success("/v1/orchestration", &json!({
        "argv": ["orchestration", "baton", "define", "memory-proof", "--resource", "integration"],
        "caller": parent,
    })).await;
    let requested = fixture.success("/v1/orchestration", &json!({
        "argv": ["orchestration", "baton", "request", "memory-proof", "--purpose", "prove ordering"],
        "caller": worker,
    })).await;
    let granted = fixture.success("/v1/orchestration", &json!({
        "argv": ["orchestration", "baton", "grant", "memory-proof", "--to", requested["request"]["id"]],
        "caller": parent,
    })).await;
    fixture.success("/v1/orchestration", &json!({
        "argv": ["orchestration", "baton", "return", "memory-proof", "--lease-id", granted["lease"]["lease_id"]],
        "caller": worker,
    })).await;
    fixture
        .success(
            "/v1/send",
            &json!({
                "from": PARENT, "to": {"seat": WORKER, "machine": null},
                "msg_id": "memory-message-control", "body": "one sequence domain",
            }),
        )
        .await;
    let history = fixture
        .spine
        .tail(None, Seq(0))
        .await
        .expect("shared replay");
    assert!(history.iter().any(|event| event.kind == "baton.returned"));
    assert!(history.iter().any(|event| event.kind == "message.pushed"));
    for adjacent in history.windows(2) {
        assert_eq!(adjacent[1].seq.unwrap().0, adjacent[0].seq.unwrap().0 + 1);
    }
    assert!(
        !fixture._store.exists(),
        "fake persistence must not open the configured disk file"
    );
}

#[tokio::test]
async fn decisions_push_to_asker_and_verification_only_closes_its_done_claim() {
    let fixture = Fixture::start().await;
    let question = fixture
        .success("/v1/report", &case("report-question")["request"])
        .await;
    let decision_id = question["decision"]["id"]
        .as_str()
        .expect("durable decision id");
    let decisions = fixture.get("/v1/decisions?parent=pij-parent").await;
    assert!(
        decisions["decisions"]
            .as_array()
            .expect("decisions")
            .iter()
            .any(|decision| decision["id"] == decision_id && decision["state"] == "open")
    );

    let mut answer = case("decision-answer")["request"].clone();
    answer["decision"] = json!(decision_id);
    answer["caller"]["TMUX_PANE"] = json!("%12");
    let (status, refused) = fixture.post("/v1/answer", &answer).await;
    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
    assert_eq!(refused["details"]["code"], "E-RS-OWNERSHIP");
    answer["caller"]["TMUX_PANE"] = json!("%10");
    let answered = fixture.success("/v1/answer", &answer).await;
    assert_eq!(answered["decision"]["state"], "answered");
    assert_eq!(answered["decision"]["answered_by"], PARENT);

    let inbox = fixture.get("/v1/inbox?seat=pij-worker&wait=false").await;
    let claim = inbox
        .as_array()
        .expect("inbox claims")
        .iter()
        .find(|claim| claim["message"]["msg_id"] == answered["decision"]["answer_msg_id"])
        .expect("answer was actually queued to its asker");
    assert_eq!(claim["message"]["from"], PARENT);
    assert_eq!(claim["message"]["to"], WORKER);
    assert!(
        claim["message"]["body"]
            .as_str()
            .expect("answer body")
            .contains(answer["answer"].as_str().expect("answer text"))
    );
    fixture
        .success(
            "/v1/inbox/ack",
            &json!({"seat": WORKER, "job_id": claim["job_id"]}),
        )
        .await;

    let done = json!({"argv": ["report", "state", "done"], "caller": {"TMUX_PANE": "%11"}});
    let first_done = fixture.success("/v1/report", &done).await;
    let anomalies = fixture.get("/v1/anomalies").await;
    assert!(has_unverified_done(&anomalies));
    let verified = fixture
        .success("/v1/report", &case("report-verify")["request"])
        .await;
    assert_eq!(verified["done_seq"], first_done["seq"]);
    assert!(!has_unverified_done(&fixture.get("/v1/anomalies").await));
    fixture.success("/v1/report", &done).await;
    assert!(has_unverified_done(&fixture.get("/v1/anomalies").await));

    fixture
        .success("/v1/close", &case("seat-close")["request"])
        .await;
    let roster = fixture.get("/v1/seats").await;
    assert!(
        roster["seats"]
            .as_array()
            .expect("roster")
            .iter()
            .all(|seat| seat["id"] != WORKER)
    );
}

fn has_unverified_done(data: &Value) -> bool {
    data["anomalies"]
        .as_array()
        .expect("anomaly rows")
        .iter()
        .any(|row| row["kind"] == "unverified-done" && row["nodeId"] == WORKER)
}
