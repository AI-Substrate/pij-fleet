//! Plan 139 u2: baseline-compatible HTTP witnesses. These import no new role API.
//! Copy this file plus the u0 contracts onto the baseline for behavioral RED.

use std::net::SocketAddr;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Event, PaneProcess, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine};
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

const ROUTES: &str = include_str!("../../testkit/fixtures/golden/api/governance-routes.json");
const EVENTS: &str = include_str!("../../testkit/fixtures/golden/api/governance-events.json");
const KEY: &str = "role-contract-key";

fn routes() -> Value {
    serde_json::from_str(ROUTES).expect("u0 routes")
}
fn event_case(id: &str) -> Value {
    let events: Value = serde_json::from_str(EVENTS).expect("u0 events");
    events["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|case| case["id"] == id)
        .expect("named event")
        .clone()
}
fn role_case() -> Value {
    routes()["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .find(|route| route["path"] == "/v1/role")
        .expect("role route")["cases"][0]
        .clone()
}

struct Fixture {
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
    spine: Arc<dyn Spine>,
    registry: Arc<dyn Registry>,
    _store: FreshStore,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn start() -> Self {
        let store = FreshStore::new();
        let config = Config {
            adapters: Adapters {
                registry: AdapterChoice::Real,
                spine: AdapterChoice::Real,
                queue: AdapterChoice::Real,
                ..Adapters::default()
            },
            store_path: store.path(),
            ..Config::default()
        };
        let mut services = pij_daemon::build_services(
            &config,
            &std::path::Path::new(&store.path()).with_extension("signals"),
        )
        .await
        .expect("services");
        let template = event_case("seat-put")["decoded_payload"].clone();
        let base_proc: ProcIdentity =
            serde_json::from_value(template["proc"].clone()).expect("proc");
        let context = routes()["fixture_context"].clone();
        let mut liveness = FakeLiveness::new();
        let mut tmux = FakeTmux::new();
        for (index, (pane, id)) in context["panes"]
            .as_object()
            .expect("panes")
            .iter()
            .enumerate()
        {
            let mut raw = template.clone();
            raw["id"] = id.clone();
            raw["pane"] = json!(pane);
            raw["role"] = Value::Null;
            raw["parent"] = if id == &context["worker"] {
                context["parent"].clone()
            } else {
                Value::Null
            };
            let proc = ProcIdentity {
                pid: base_proc.pid + index as u32,
                ..base_proc
            };
            raw["proc"] = serde_json::to_value(proc).expect("proc JSON");
            let seat: SeatDescriptor = serde_json::from_value(raw).expect("fixture seat");
            tmux = tmux.with_pane_process(
                pane,
                PaneProcess {
                    pid: proc.pid,
                    cwd: seat.folder.clone(),
                },
            );
            liveness = liveness.with_proc(proc);
            services.registry.put(seat).await.expect("seed seat");
        }
        services.tmux = Arc::new(tmux);
        services.liveness = Arc::new(liveness);
        let spine = services.spine.clone();
        let registry = services.registry.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("private bind");
        let addr = listener.local_addr().expect("addr");
        let app = router_with_config(services, HttpConfig::local(KEY.to_string()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self {
            addr,
            server,
            spine,
            registry,
            _store: store,
        }
    }
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let response = reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth(KEY)
            .json(body)
            .send()
            .await
            .expect("HTTP POST");
        let status = response.status().as_u16();
        let text = response.text().await.expect("body");
        let body = serde_json::from_str(&text).unwrap_or_else(|_| json!({"undecodable":text}));
        (status, body)
    }
    async fn get(&self, path: &str, query: &[(&str, &str)]) -> (u16, Value) {
        let response = reqwest::Client::new()
            .get(format!("http://{}{path}", self.addr))
            .query(query)
            .bearer_auth(KEY)
            .send()
            .await
            .expect("HTTP GET");
        let status = response.status().as_u16();
        let text = response.text().await.expect("body");
        let body = serde_json::from_str(&text).unwrap_or_else(|_| json!({"undecodable":text}));
        (status, body)
    }
    async fn events(&self) -> Vec<Event> {
        self.spine
            .tail(None, Seq(0))
            .await
            .expect("spine")
            .into_iter()
            .filter(|event| event.kind == event_case("role-set")["frame"]["event"]["kind"])
            .collect()
    }
    async fn assert_read_role(&self, expected: Value) {
        let context = routes()["fixture_context"].clone();
        let worker = SeatId::from(context["worker"].as_str().expect("worker id"));
        let raw = self
            .registry
            .get(&worker)
            .await
            .expect("registry")
            .expect("worker");
        let before = self
            .spine
            .tail(None, Seq(0))
            .await
            .expect("events before projections");
        let (status, response) = self.get("/v1/seats", &[("scope", "local")]).await;
        assert_eq!(status, 200, "{response}");
        let seat = response["data"]["seats"]
            .as_array()
            .expect("roster rows")
            .iter()
            .find(|seat| seat["id"] == context["worker"])
            .expect("worker row");
        assert_eq!(
            seat.get("role"),
            Some(&expected),
            "seats must join seat_roles"
        );
        let (status, card) = self
            .post("/v1/state", &json!({"id":context["worker"]}))
            .await;
        assert_eq!(status, 200, "{card}");
        assert_eq!(
            card["data"].get("role"),
            Some(&expected),
            "StateCard must join seat_roles"
        );
        let mut projected = serde_json::to_value(&raw).expect("descriptor JSON");
        projected["role"] = expected.clone();
        // whoami also carries the held-FYI count the status lines read (plan 158).
        projected["pending_fyis"] = json!(0);
        let pane = raw.pane.as_deref().expect("worker pane");
        for query in [[("seat", worker.as_str())], [("pane", pane)]] {
            let (status, response) = self.get("/v1/whoami", &query).await;
            assert_eq!(status, 200, "{response}");
            assert_eq!(
                response["data"], projected,
                "GET whoami changes only the projected role"
            );
            assert_eq!(
                response["data"].get("role"),
                seat.get("role"),
                "whoami.role == seats.role"
            );
        }
        for caller in [json!({"PIJ_SESSION_ID":worker}), json!({"TMUX_PANE":pane})] {
            let request = json!({"caller":caller});
            let (status, identity) = self.post("/v1/whoami", &request).await;
            assert_eq!(status, 200, "{identity}");
            assert_eq!(
                identity["data"], projected,
                "POST whoami changes only the projected role"
            );
            let (status, home) = self.post("/v1/phonehome", &request).await;
            assert_eq!(status, 200, "{home}");
            assert_eq!(
                home["data"].get("role"),
                seat.get("role"),
                "phonehome.role == seats.role"
            );
            assert_eq!(
                home["data"].get("role"),
                card["data"].get("role"),
                "phonehome.role == StateCard.role"
            );
            assert_eq!(home["data"]["seat"], json!(worker));
            assert_eq!(
                home["data"]["bound"], true,
                "role projection must not change binding truth"
            );
            assert_eq!(home["data"]["pid"], json!(raw.proc.map(|proc| proc.pid)));
            assert_eq!(
                home["data"]["proc_start"],
                json!(raw.proc.map(|proc| proc.proc_start))
            );
        }
        assert_eq!(
            self.registry.get(&worker).await.expect("raw registry"),
            Some(raw),
            "projection must not write its joined role back to the descriptor"
        );
        assert_eq!(
            self.spine
                .tail(None, Seq(0))
                .await
                .expect("events after projections"),
            before,
            "identity projections must not publish events"
        );
    }
}

#[tokio::test]
async fn role_parent_assertion_matches_u0_and_is_readable_via_seats_and_state() {
    let fixture = Fixture::start().await;
    let case = role_case();
    let (status, actual) = fixture.post("/v1/role", &case["request"]).await;
    assert_eq!(status, 200, "baseline witness: {actual}");
    let mut expected = case["response"].clone();
    assert!(
        actual["data"]["assigned_at"]
            .as_u64()
            .is_some_and(|at| at > 0)
    );
    assert!(actual["data"]["seq"].as_u64().is_some_and(|seq| seq > 0));
    expected["data"]["assigned_at"] = actual["data"]["assigned_at"].clone();
    expected["data"]["seq"] = actual["data"]["seq"].clone();
    assert_eq!(actual, expected);
    fixture
        .assert_read_role(case["request"]["role"].clone())
        .await;
    let events = fixture.events().await;
    assert_eq!(events.len(), 1);
    let mut payload = event_case("role-set")["decoded_payload"].clone();
    payload["record"]["assigned_at"] = actual["data"]["assigned_at"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(&events[0].payload).expect("payload"),
        payload
    );
    assert_eq!(
        events[0].seq.expect("persisted sequence").0,
        actual["data"]["seq"].as_u64().expect("receipt seq")
    );
    assert_eq!(
        events[0].at,
        actual["data"]["assigned_at"].as_u64().expect("timestamp")
    );
}

#[tokio::test]
async fn role_outsider_refusal_matches_u0_without_a_write_or_event() {
    let fixture = Fixture::start().await;
    let mut request = role_case()["request"].clone();
    let context = routes()["fixture_context"].clone();
    let pane = context["panes"]
        .as_object()
        .expect("panes")
        .iter()
        .find(|(_, seat)| **seat == context["outsider"])
        .expect("outsider pane")
        .0;
    request["caller"]["TMUX_PANE"] = json!(pane);
    let (status, response) = fixture.post("/v1/role", &request).await;
    assert_eq!(
        json!(status),
        routes()["refusals"]["ownership"]["http_status"]
    );
    assert_eq!(response, routes()["refusals"]["ownership"]["response"]);
    fixture.assert_read_role(Value::Null).await;
    assert!(fixture.events().await.is_empty());
}

#[tokio::test]
async fn role_self_default_reassertion_and_explicit_null_publish_once_each() {
    let fixture = Fixture::start().await;
    let mut request = role_case()["request"].clone();
    let context = routes()["fixture_context"].clone();
    let pane = context["panes"]
        .as_object()
        .expect("panes")
        .iter()
        .find(|(_, seat)| **seat == context["worker"])
        .expect("worker pane")
        .0;
    request.as_object_mut().expect("request").remove("seat");
    request["caller"]["TMUX_PANE"] = json!(pane);
    for _ in 0..2 {
        let (status, response) = fixture.post("/v1/role", &request).await;
        assert_eq!(status, 200, "{response}");
        assert_eq!(response["data"]["assigned_by"], context["worker"]);
    }
    request["role"] = Value::Null;
    let (status, response) = fixture.post("/v1/role", &request).await;
    assert_eq!(status, 200, "{response}");
    fixture.assert_read_role(Value::Null).await;
    let events = fixture.events().await;
    assert_eq!(events.len(), 3);
    assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    let mut expected = event_case("role-unset")["decoded_payload"].clone();
    expected["actor"] = context["worker"].clone();
    expected["record"]["assigned_by"] = context["worker"].clone();
    expected["record"]["assigned_at"] = response["data"]["assigned_at"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(&events[2].payload).expect("payload"),
        expected
    );
}

#[tokio::test]
async fn role_missing_empty_unknown_flags_and_forged_attribution_refuse_without_events() {
    let fixture = Fixture::start().await;
    let case = role_case();
    let mut missing = case["request"].clone();
    missing.as_object_mut().expect("request").remove("role");
    let mut empty = case["request"].clone();
    empty["role"] = json!("   ");
    let mut forged = case["request"].clone();
    forged["assigned_by"] = routes()["fixture_context"]["outsider"].clone();
    let mut bad_argv = case["shim_request"].clone();
    bad_argv["argv"]
        .as_array_mut()
        .expect("argv")
        .push(json!("--unknown"));
    for request in [missing, empty, forged, bad_argv] {
        let (status, response) = fixture.post("/v1/role", &request).await;
        assert!(status >= 400, "{response}");
        assert_eq!(response["ok"], false, "decodable refusal: {response}");
    }
    assert!(fixture.events().await.is_empty());
    fixture.assert_read_role(Value::Null).await;
}

#[tokio::test]
async fn role_shim_argv_and_typed_requests_share_the_same_contract() {
    let fixture = Fixture::start().await;
    let case = role_case();
    let (status, response) = fixture.post("/v1/role", &case["shim_request"]).await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["data"]["role"], case["response"]["data"]["role"]);
    fixture
        .assert_read_role(case["request"]["role"].clone())
        .await;
}

#[tokio::test]
async fn adopt_role_assertion_survives_omitted_readoption_and_refused_admission() {
    let fixture = Fixture::start().await;
    let template = event_case("seat-put")["decoded_payload"].clone();
    let mut request = json!({"argv":["adopt",template["pane"],"--harness",template["harness"],"--role",template["role"]]});
    let (status, response) = fixture.post("/v1/adopt", &request).await;
    assert_eq!(status, 200, "baseline adopt --role witness: {response}");
    fixture.assert_read_role(template["role"].clone()).await;
    request["argv"].as_array_mut().expect("argv").truncate(4);
    let (status, response) = fixture.post("/v1/adopt", &request).await;
    assert_eq!(status, 200, "{response}");
    fixture.assert_read_role(template["role"].clone()).await;
    assert_eq!(
        fixture.events().await.len(),
        1,
        "omitted role is not another assertion"
    );
    request["argv"] = json!([
        "adopt",
        "%missing",
        "--harness",
        template["harness"],
        "--role",
        template["role"]
    ]);
    let (status, _) = fixture.post("/v1/adopt", &request).await;
    assert!(status >= 400);
    assert_eq!(
        fixture.events().await.len(),
        1,
        "refused admission cannot publish a role"
    );
}

#[tokio::test]
async fn register_asserted_role_is_durable_and_null_is_not_omission() {
    let fixture = Fixture::start().await;
    let template = event_case("seat-put")["decoded_payload"].clone();
    let context = routes()["fixture_context"].clone();
    // The fixture assigns distinct process ids in canonical pane-key order.
    let index = context["panes"]
        .as_object()
        .expect("panes")
        .iter()
        .position(|(_, id)| id == &context["worker"])
        .expect("worker index");
    let mut request = json!({"id":template["id"],"harness":template["harness"],"folder":template["folder"],
        "pane":template["pane"],"pid":template["proc"]["pid"].as_u64().expect("pid") + index as u64,
        "proc_start":template["proc"]["proc_start"],"role":template["role"]});
    let (status, response) = fixture.post("/v1/register", &request).await;
    assert_eq!(status, 200, "{response}");
    fixture.assert_read_role(template["role"].clone()).await;
    request.as_object_mut().expect("request").remove("role");
    assert_eq!(fixture.post("/v1/register", &request).await.0, 200);
    assert_eq!(fixture.events().await.len(), 1);
    request["role"] = Value::Null;
    assert!(fixture.post("/v1/register", &request).await.0 >= 400);
    fixture.assert_read_role(template["role"].clone()).await;
    assert_eq!(fixture.events().await.len(), 1);
}

#[tokio::test]
async fn typed_adopt_role_rejects_null_and_duplicate_assertions_without_publication() {
    let fixture = Fixture::start().await;
    let template = event_case("seat-put")["decoded_payload"].clone();
    let mut request = json!({"argv":["adopt",template["pane"],"--harness",template["harness"]],"role":Value::Null});
    assert!(fixture.post("/v1/adopt", &request).await.0 >= 400);
    assert!(fixture.events().await.is_empty());
    request["role"] = template["role"].clone();
    let (status, response) = fixture.post("/v1/adopt", &request).await;
    assert_eq!(status, 200, "{response}");
    fixture.assert_read_role(template["role"].clone()).await;
    request["argv"]
        .as_array_mut()
        .expect("argv")
        .extend([json!("--role"), template["role"].clone()]);
    assert!(fixture.post("/v1/adopt", &request).await.0 >= 400);
    assert_eq!(fixture.events().await.len(), 1);
}

#[tokio::test]
async fn all_identity_projections_agree_across_role_assert_update_and_unset_with_stale_descriptor()
{
    let fixture = Fixture::start().await;
    let context = routes()["fixture_context"].clone();
    let worker = SeatId::from(context["worker"].as_str().expect("worker id"));
    let mut raw = fixture
        .registry
        .get(&worker)
        .await
        .expect("registry")
        .expect("worker");
    raw.role = Some("descriptor-only-stale-role".to_string());
    fixture
        .registry
        .put(raw.clone())
        .await
        .expect("seed stale descriptor role");
    fixture.assert_read_role(Value::Null).await;

    let mut request = role_case()["request"].clone();
    let assigned = request["role"].clone();
    let updated = json!("worker");
    assert_ne!(assigned, updated);
    for role in [assigned, updated, Value::Null] {
        request["role"] = role.clone();
        let (status, response) = fixture.post("/v1/role", &request).await;
        assert_eq!(status, 200, "{response}");
        assert_eq!(response["data"].get("role"), Some(&role));
        fixture.assert_read_role(role).await;
        assert_eq!(
            fixture.registry.get(&worker).await.expect("raw registry"),
            Some(raw.clone()),
            "authoritative assignment must remain independent from the stale descriptor"
        );
    }
    let events = fixture.events().await;
    assert_eq!(events.len(), 3, "only explicit role writes publish");
    assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
}

#[tokio::test]
async fn role_projection_store_failure_refuses_every_identity_surface_without_stale_fallback() {
    let fixture = Fixture::start().await;
    let context = routes()["fixture_context"].clone();
    let worker = SeatId::from(context["worker"].as_str().expect("worker id"));
    let mut raw = fixture
        .registry
        .get(&worker)
        .await
        .expect("registry")
        .expect("worker");
    raw.role = Some("descriptor-only-stale-role".to_string());
    fixture
        .registry
        .put(raw.clone())
        .await
        .expect("seed stale descriptor role");
    let before = fixture.spine.tail(None, Seq(0)).await.expect("events");
    let pool = pij_store::open(&fixture._store.path())
        .await
        .expect("private store pool");
    sqlx::query("DROP TABLE seat_roles")
        .execute(&pool)
        .await
        .expect("inject role read failure");
    let pane = raw.pane.as_deref().expect("worker pane");
    let mut responses = Vec::new();
    responses.push(fixture.get("/v1/seats", &[("scope", "local")]).await);
    for query in [[("seat", worker.as_str())], [("pane", pane)]] {
        responses.push(fixture.get("/v1/whoami", &query).await);
    }
    responses.push(fixture.post("/v1/state", &json!({"id":worker})).await);
    for caller in [json!({"PIJ_SESSION_ID":worker}), json!({"TMUX_PANE":pane})] {
        for path in ["/v1/whoami", "/v1/phonehome"] {
            responses.push(fixture.post(path, &json!({"caller":caller})).await);
        }
    }
    for (status, response) in responses {
        assert_eq!(status, 500, "{response}");
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(response["error"], "adapter", "{response}");
        assert!(
            response["data"].is_null(),
            "no successful stale role projection: {response}"
        );
        assert!(
            response["meta"]
                .as_str()
                .expect("diagnostic")
                .contains("seat_roles"),
            "{response}"
        );
    }
    assert_eq!(
        fixture.registry.get(&worker).await.expect("raw registry"),
        Some(raw.clone())
    );
    assert_eq!(
        fixture.spine.tail(None, Seq(0)).await.expect("events"),
        before
    );

    // Role storage is required for response projections, not for raw identity
    // authority. Reports and the lifecycle CAS must still use the real row.
    for caller in [json!({"PIJ_SESSION_ID":worker}), json!({"TMUX_PANE":pane})] {
        let (status, report) = fixture.post("/v1/report", &json!({
            "caller": caller,
            "argv": ["report", "now", "projection reads refused", "raw identity remains usable"]
        })).await;
        assert_eq!(
            status, 200,
            "raw identity must not require role storage: {report}"
        );
        assert_eq!(report["data"]["seat"], json!(worker));
    }
    let (status, closed) = fixture
        .post(
            "/v1/close",
            &json!({
                "caller": {"TMUX_PANE": pane}, "seat": worker, "reason": "role-projection-proof"
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "raw descriptor CAS must not require role storage: {closed}"
    );
    assert!(closed["data"]["seq"].as_u64().is_some_and(|seq| seq > 0));
    let retired = fixture
        .registry
        .get(&worker)
        .await
        .expect("registry")
        .expect("retired worker");
    assert!(retired.tombstoned_at.is_some());
    assert_eq!(
        retired.role, raw.role,
        "CAS must preserve the raw descriptor role"
    );
}

#[tokio::test]
async fn register_and_adopt_refuse_roles_outside_the_closed_vocabulary_before_admission() {
    let fixture = Fixture::start().await;
    let template = event_case("seat-put")["decoded_payload"].clone();
    let context = routes()["fixture_context"].clone();
    let index = context["panes"]
        .as_object()
        .expect("panes")
        .iter()
        .position(|(_, id)| id == &context["worker"])
        .expect("worker index");
    let register = json!({"id":template["id"],"harness":template["harness"],"folder":template["folder"],
        "pane":template["pane"],"pid":template["proc"]["pid"].as_u64().expect("pid") + index as u64,
        "proc_start":template["proc"]["proc_start"],"role":"coder"});
    let (status, response) = fixture.post("/v1/register", &register).await;
    assert!(status >= 400, "{response}");
    let typed = json!({"argv":["adopt",template["pane"],"--harness",template["harness"]],"role":"reviewer"});
    assert!(fixture.post("/v1/adopt", &typed).await.0 >= 400);
    let argv =
        json!({"argv":["adopt",template["pane"],"--harness",template["harness"],"--role","coder"]});
    let (status, response) = fixture.post("/v1/adopt", &argv).await;
    assert!(status >= 400, "{response}");
    assert!(
        response.to_string().contains("prime, pm, worker, pa"),
        "{response}"
    );
    assert!(
        fixture.events().await.is_empty(),
        "a refused role admits no seat and publishes nothing"
    );
}
