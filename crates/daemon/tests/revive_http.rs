//! Baseline-compatible HTTP witnesses for observed-dead and assumed-dead revive.
use std::sync::Arc;

use async_trait::async_trait;
use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{LivenessPort, SessionStatusPort};
use pij_core::session_status::{CacheTtl, Fact, SeatStatus, SessionStatusReply};
use pij_daemon::Services;
use pij_daemon::http::{HttpConfig, router_with_config};
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeSessionStatus, FakeTmux};
use serde_json::{Value, json};

struct ProbeUnavailable;
#[async_trait]
impl LivenessPort for ProbeUnavailable {
    async fn proc_start(&self, _pid: u32) -> Result<Option<u64>> {
        Err(PijError::Adapter {
            adapter: "process-liveness/ps".to_string(),
            message: "ps unavailable".to_string(),
        })
    }
}

struct Fixture {
    services: Services,
    tmux: Arc<FakeTmux>,
    addr: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
    store: FreshStore,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn fixture(probe: Arc<dyn LivenessPort>) -> Fixture {
    fixture_with_retired_harnesses(probe, Vec::new()).await
}

async fn fixture_with_retired_harnesses(
    probe: Arc<dyn LivenessPort>,
    retired_harnesses: Vec<Harness>,
) -> Fixture {
    fixture_with(probe, retired_harnesses, None).await
}

async fn fixture_with(
    probe: Arc<dyn LivenessPort>,
    retired_harnesses: Vec<Harness>,
    session_status: Option<Arc<dyn SessionStatusPort>>,
) -> Fixture {
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        retired_harnesses,
        ..Config::default()
    };
    let mut services = pij_daemon::build_services(
        &config,
        &std::path::Path::new(&store.path()).with_extension("signals"),
    )
    .await
    .expect("real registry and spine");
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    services.liveness = probe;
    if let Some(session_status) = session_status {
        services.session_status = session_status;
    }
    for (id, pane) in [("pij-parent", "%parent"), ("pij-outsider", "%outsider")] {
        let mut seat = SeatDescriptor::new(id, Harness::Copilot, "/tmp");
        seat.pane = Some(pane.to_string());
        services.registry.put(seat).await.expect("caller seed");
        tmux.arrange_pane(pane);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router_with_config(
        services.clone(),
        HttpConfig {
            auth: pij_daemon::http::AuthRing::local("revive-key".to_string()),
            machine_alias: "test".to_string(),
        },
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fixture {
        services,
        tmux,
        addr,
        server,
        store,
    }
}

fn target() -> SeatDescriptor {
    let mut seat = SeatDescriptor::new("pij-child", Harness::Copilot, "/tmp");
    seat.parent = Some("pij-parent".into());
    seat.pane = Some("%old".to_string());
    seat.proc = Some(ProcIdentity {
        pid: 4242,
        proc_start: 10,
    });
    seat.model = Some("provider/prior".to_string());
    seat
}
fn request() -> Value {
    // These seats record no conversation, so relaunching them is blank by
    // definition — which plan 156 makes an explicit --fresh.
    json!({"id":"pij-child","session":"fleet","fresh":true})
}
fn assumed_request(caller: &str) -> Value {
    let mut body = request();
    body["assume_dead"] = json!(true);
    body["evidence"] = json!("old pane destroyed; replacement pid belongs to another process");
    body["caller"] = json!({"PIJ_SESSION_ID":caller});
    body
}
async fn post(f: &Fixture, body: &Value) -> Value {
    reqwest::Client::new()
        .post(format!("http://{}/v1/revive", f.addr))
        .bearer_auth("revive-key")
        .json(body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .expect("decodable revive envelope")
}
async fn unchanged(f: &Fixture, prior: &SeatDescriptor, since: Seq) {
    assert_eq!(
        f.services.registry.get(&prior.id).await.unwrap(),
        Some(prior.clone())
    );
    assert!(
        f.services
            .spine
            .tail(Some(&prior.id), since)
            .await
            .unwrap()
            .is_empty(),
        "refusal must not leave a tombstone or assumption"
    );
    assert!(
        f.tmux
            .calls()
            .iter()
            .all(|call| !call.starts_with("new_window:") && !call.starts_with("kill:"))
    );
}
fn assert_live_refusal(body: &Value, observation: &str) {
    assert_eq!(body["ok"], false, "{body}");
    assert_eq!(body["details"]["code"], "E-RS-REVIVE-LIVE", "{body}");
    assert_eq!(
        body["details"]["observation"]["liveness"], observation,
        "{body}"
    );
    assert_eq!(
        body["details"]["override_command"],
        "pij-rs revive pij-child --assume-dead --evidence \"<text>\""
    );
    assert!(body["meta"].as_str().unwrap().contains(observation));
}

#[tokio::test]
async fn revive_observed_dead_tombstones_before_relaunch_and_cites_seq() {
    let f = fixture(Arc::new(FakeLiveness::new())).await;
    let prior = target();
    let before = f.services.registry.put(prior.clone()).await.unwrap();
    let body = post(&f, &request()).await;
    assert_eq!(body["ok"], true, "{body}");
    let events = f
        .services
        .spine
        .tail(Some(&prior.id), before)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["seat.tombstone", "seat.revive-postmortem", "seat.put"]
    );
    let tombstone: Value = serde_json::from_str(&events[0].payload).unwrap();
    assert_eq!(tombstone["reason"], "revive-observed-dead");
    assert_eq!(
        tombstone["observation"],
        json!({"pid":4242,"proc_start":10,"pane":"%old","pane_present":false})
    );
    assert_eq!(body["details"]["seq"], json!(events[0].seq));
    let revived = f.services.registry.get(&prior.id).await.unwrap().unwrap();
    assert_eq!(revived.proc, None);
    assert_eq!(revived.tombstoned_at, None);
    assert_eq!(revived.parent, prior.parent);
    assert_eq!(revived.model, prior.model);
    assert_ne!(revived.pane, prior.pane);
}

#[tokio::test]
async fn revive_retired_harness_succeeds_and_audits_override() {
    let f = fixture_with_retired_harnesses(Arc::new(FakeLiveness::new()), vec![Harness::Pi]).await;
    let mut prior = target();
    prior.harness = Harness::Pi;
    let before = f.services.registry.put(prior.clone()).await.unwrap();

    let body = post(&f, &request()).await;
    assert_eq!(body["ok"], true, "{body}");
    let revived = f.services.registry.get(&prior.id).await.unwrap().unwrap();
    assert_eq!(revived.harness, Harness::Pi);
    assert_eq!(revived.tombstoned_at, None);
    assert_ne!(revived.pane, prior.pane);

    let events = f
        .services
        .spine
        .tail(Some(&prior.id), before)
        .await
        .unwrap();
    let overrides: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "spawn.retired-harness-override")
        .collect();
    assert_eq!(overrides.len(), 1, "retired revival must be audited");
    let event = overrides[0];
    assert_eq!(event.seat.as_ref(), Some(&prior.id));
    assert_eq!(
        serde_json::from_str::<Value>(&event.payload).unwrap(),
        json!({
            "harness": "pi",
            "allow_retired": true,
            "spawn_id": revived.spawn_id,
        })
    );
    let launched = events
        .iter()
        .find(|event| event.kind == "seat.put")
        .unwrap();
    assert!(
        event.seq < launched.seq,
        "audit precedes launch publication"
    );
}

#[tokio::test]
async fn revive_legacy_tombstone_records_audit_before_relaunch() {
    for older_event in [false, true] {
        let f = fixture(Arc::new(FakeLiveness::new())).await;
        let mut prior = target();
        if older_event {
            f.services.registry.put(prior.clone()).await.unwrap();
            f.services
                .registry
                .tombstone_if_unchanged(prior.clone(), "previous incarnation".into())
                .await
                .unwrap();
        }
        prior.tombstoned_at = Some(1);
        prior.tombstone_reason = Some("superseded at a native session boundary".into());
        let before = f.services.registry.put(prior.clone()).await.unwrap();
        let body = post(&f, &request()).await;
        assert_eq!(body["ok"], true, "{body}");
        let events = f
            .services
            .spine
            .tail(Some(&prior.id), before)
            .await
            .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            [
                "revive.legacy-tombstone",
                "seat.revive-postmortem",
                "seat.put"
            ]
        );
        let audit: Value = serde_json::from_str(&events[0].payload).unwrap();
        assert_eq!(audit["tombstoned_at"], prior.tombstoned_at.unwrap());
        assert_eq!(audit["reason"], json!(prior.tombstone_reason));
        assert_eq!(
            body["details"]["legacy_tombstone_seq"],
            json!(events[0].seq)
        );
        assert!(
            body["details"].get("seq").is_none(),
            "do not label the legacy audit as a tombstone receipt"
        );
        let revived = f.services.registry.get(&prior.id).await.unwrap().unwrap();
        assert_eq!(revived.tombstoned_at, None);
        assert_ne!(revived.pane, prior.pane);
    }
}

#[tokio::test]
async fn revive_legacy_audit_failure_does_not_relaunch() {
    let f = fixture(Arc::new(FakeLiveness::new())).await;
    let mut prior = target();
    prior.tombstoned_at = Some(1);
    prior.tombstone_reason = Some("legacy retirement".into());
    let before = f.services.registry.put(prior.clone()).await.unwrap();
    let pool = pij_store::open(&f.store.path()).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_legacy_audit BEFORE INSERT ON spine_events WHEN NEW.kind = 'revive.legacy-tombstone' BEGIN SELECT RAISE(ABORT, 'injected legacy audit publication failure'); END")
        .execute(&pool).await.unwrap();
    let body = post(&f, &request()).await;
    assert_eq!(body["ok"], false, "{body}");
    unchanged(&f, &prior, before).await;
}

#[tokio::test]
async fn revive_recycled_and_unknown_refuse_with_exact_override_guidance() {
    for (probe, observation) in [
        (
            Arc::new(FakeLiveness::new().with_recycled(4242, 20)) as Arc<dyn LivenessPort>,
            "recycled",
        ),
        (
            Arc::new(ProbeUnavailable) as Arc<dyn LivenessPort>,
            "unknown",
        ),
    ] {
        let f = fixture(probe).await;
        let prior = target();
        let before = f.services.registry.put(prior.clone()).await.unwrap();
        assert_live_refusal(&post(&f, &request()).await, observation);
        unchanged(&f, &prior, before).await;
    }
}

#[tokio::test]
async fn revive_assumed_dead_records_parent_evidence_and_actual_observation() {
    for (probe, observation) in [
        (
            Arc::new(FakeLiveness::new().with_recycled(4242, 20)) as Arc<dyn LivenessPort>,
            "recycled",
        ),
        (
            Arc::new(ProbeUnavailable) as Arc<dyn LivenessPort>,
            "unknown",
        ),
    ] {
        let f = fixture(probe).await;
        let prior = target();
        let before = f.services.registry.put(prior.clone()).await.unwrap();
        let request = assumed_request("pij-parent");
        let body = post(&f, &request).await;
        assert_eq!(body["ok"], true, "{body}");
        let events = f
            .services
            .spine
            .tail(Some(&prior.id), before)
            .await
            .unwrap();
        let assumed = events
            .iter()
            .find(|event| event.kind == "revive.assumed-dead")
            .expect("durable assumption");
        let payload: Value = serde_json::from_str(&assumed.payload).unwrap();
        assert_eq!(payload["caller"], "pij-parent");
        assert_eq!(payload["evidence"], request["evidence"]);
        assert_eq!(payload["observation"]["liveness"], observation);
        assert_eq!(payload["observation"]["pid"], 4242);
        assert_eq!(payload["observation"]["proc_start"], 10);
        assert_eq!(payload["observation"]["pane"], "%old");
        assert_eq!(payload["observation"]["pane_present"], false);
        if observation == "recycled" {
            assert_eq!(payload["observation"]["observed_start"], 20);
        } else {
            assert!(
                payload["observation"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("ps unavailable")
            );
        }
        assert_eq!(body["details"]["assumed_dead_seq"], json!(assumed.seq));
        let tombstone = events
            .iter()
            .find(|event| event.kind == "seat.tombstone")
            .unwrap();
        assert_eq!(body["details"]["seq"], json!(tombstone.seq));
        assert_eq!(
            serde_json::from_str::<Value>(&tombstone.payload).unwrap()["reason"],
            "revive-assumed-dead"
        );
    }
}

#[tokio::test]
async fn revive_assumption_requires_parent_or_authoritative_prime_for_parentless() {
    let f = fixture(Arc::new(FakeLiveness::new().with_recycled(4242, 20))).await;
    let mut prior = target();
    let mut before = f.services.registry.put(prior.clone()).await.unwrap();
    assert_live_refusal(
        &post(&f, &assumed_request("pij-outsider")).await,
        "recycled",
    );
    unchanged(&f, &prior, before).await;
    let outsider = SeatId::from("pij-outsider");
    f.services
        .roles
        .assert_role(&outsider, &outsider, Some("prime".to_string()))
        .await
        .unwrap();
    assert_live_refusal(
        &post(&f, &assumed_request("pij-outsider")).await,
        "recycled",
    );
    unchanged(&f, &prior, before).await;
    prior.parent = None;
    before = f.services.registry.put(prior.clone()).await.unwrap();
    let mut stale_prime = f
        .services
        .registry
        .get(&"pij-parent".into())
        .await
        .unwrap()
        .unwrap();
    stale_prime.role = Some("prime".to_string());
    f.services.registry.put(stale_prime).await.unwrap();
    assert_live_refusal(&post(&f, &assumed_request("pij-parent")).await, "recycled");
    unchanged(&f, &prior, before).await;
    assert_eq!(post(&f, &assumed_request("pij-outsider")).await["ok"], true);
}

#[tokio::test]
async fn revive_assumption_refuses_missing_evidence_self_and_forged_parent() {
    let f = fixture(Arc::new(FakeLiveness::new().with_recycled(4242, 20))).await;
    let prior = target();
    let before = f.services.registry.put(prior.clone()).await.unwrap();
    for evidence in [Value::Null, json!("   ")] {
        let mut body = assumed_request("pij-parent");
        body["evidence"] = evidence;
        assert_eq!(post(&f, &body).await["ok"], false);
    }
    assert_live_refusal(&post(&f, &assumed_request("pij-child")).await, "recycled");
    let mut forged = assumed_request("pij-parent");
    forged["caller"]["TMUX_PANE"] = json!("%outsider");
    assert_eq!(post(&f, &forged).await["ok"], false);
    unchanged(&f, &prior, before).await;
}

#[tokio::test]
async fn revive_assumption_never_overrides_active_process_or_present_pane() {
    for (probe, present, observation) in [
        (
            FakeLiveness::new().with_proc(target().proc.unwrap()),
            false,
            "active",
        ),
        (FakeLiveness::new(), true, "dead"),
        (
            FakeLiveness::new().with_recycled(4242, 20),
            true,
            "recycled",
        ),
    ] {
        let f = fixture(Arc::new(probe)).await;
        let prior = target();
        if present {
            f.tmux.arrange_pane("%old");
        }
        let before = f.services.registry.put(prior.clone()).await.unwrap();
        assert_live_refusal(&post(&f, &assumed_request("pij-parent")).await, observation);
        unchanged(&f, &prior, before).await;
    }
}

#[tokio::test]
async fn concurrent_observed_dead_revives_launch_exactly_one_incarnation() {
    let f = fixture(Arc::new(FakeLiveness::new())).await;
    let prior = target();
    let before = f.services.registry.put(prior.clone()).await.unwrap();
    let request = request();
    let (first, second) = tokio::join!(post(&f, &request), post(&f, &request));
    assert_eq!(
        [first, second]
            .iter()
            .filter(|body| body["ok"] == true)
            .count(),
        1
    );
    let events = f
        .services
        .spine
        .tail(Some(&prior.id), before)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "seat.tombstone")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "seat.put")
            .count(),
        1
    );
    assert_eq!(
        f.tmux
            .calls()
            .iter()
            .filter(|call| call.starts_with("new_window:"))
            .count(),
        1
    );
}

#[tokio::test]
async fn revive_recycled_process_is_never_signaled() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut replacement = Child(
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap(),
    );
    let pid = replacement.0.id();
    let f = fixture(Arc::new(FakeLiveness::new().with_recycled(pid, 20))).await;
    let mut prior = target();
    prior.proc.as_mut().unwrap().pid = pid;
    f.services.registry.put(prior).await.unwrap();
    assert_eq!(post(&f, &assumed_request("pij-parent")).await["ok"], true);
    assert!(
        replacement.0.try_wait().unwrap().is_none(),
        "replacement process survives revive"
    );
    assert!(f.tmux.calls().iter().all(|call| !call.starts_with("kill:")));
}

#[tokio::test]
async fn revive_assumption_publication_failure_cannot_retire_or_launch() {
    let f = fixture(Arc::new(FakeLiveness::new().with_recycled(4242, 20))).await;
    let prior = target();
    let before = f.services.registry.put(prior.clone()).await.unwrap();
    let pool = pij_store::open(&f.store.path()).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_revive_audit BEFORE INSERT ON spine_events WHEN NEW.kind = 'revive.assumed-dead' BEGIN SELECT RAISE(ABORT, 'injected assumption publication failure'); END")
        .execute(&pool).await.unwrap();
    let body = post(&f, &assumed_request("pij-parent")).await;
    assert_eq!(body["ok"], false, "{body}");
    unchanged(&f, &prior, before).await;
}

/// Plan 156 AC3 — revive resumes the recorded conversation, and never forks it.
#[tokio::test]
async fn revive_resumes_the_recorded_conversation_unless_it_is_live_elsewhere() {
    // Synthetic: the Claude session-record probe reads the real home read-only.
    const SESSION: &str = "plan156-revive-fixture-session";
    let running = ProcIdentity {
        pid: 43_821,
        proc_start: 20,
    };
    let f = fixture(Arc::new(FakeLiveness::new().with_proc(running))).await;
    let mut prior = target();
    prior.harness = Harness::Claude;
    prior.harness_session = Some(SESSION.to_string());
    f.services.registry.put(prior.clone()).await.unwrap();
    let mut other = SeatDescriptor::new("pij-other", Harness::Claude, "/tmp");
    other.pane = Some("%3".to_string());
    other.proc = Some(running);
    other.harness_session = Some(SESSION.to_string());
    let since = f.services.registry.put(other.clone()).await.unwrap();
    let resume = json!({"id":"pij-child","session":"fleet"});

    let body = post(&f, &resume).await;
    assert_eq!(body["ok"], false, "{body}");
    assert!(
        body["meta"].as_str().unwrap().contains("%3"),
        "names the pane: {body}"
    );
    unchanged(&f, &prior, since).await;

    other.harness_session = Some("another-conversation".to_string());
    f.services.registry.put(other).await.unwrap();
    let body = post(&f, &resume).await;
    assert_eq!(body["ok"], true, "{body}");
    let revived = f.services.registry.get(&prior.id).await.unwrap().unwrap();
    assert_eq!(revived.harness_session.as_deref(), Some(SESSION));
    let launch = f
        .tmux
        .calls()
        .into_iter()
        .find(|call| call.starts_with("new_window:"))
        .expect("launched");
    assert!(
        launch.contains(&format!("\"--resume\", \"{SESSION}\"")),
        "{launch}"
    );

    let mut blank = target();
    blank.id = "pij-blank".into();
    blank.pane = Some("%gone".to_string());
    f.services.registry.put(blank).await.unwrap();
    let body = post(&f, &json!({"id":"pij-blank","session":"fleet"})).await;
    assert_eq!(body["ok"], false, "{body}");
    assert!(body["meta"].as_str().unwrap().contains("--fresh"), "{body}");
}

/// Plan 174 review F001 — revive's conversation guard keeps its ORIGINAL rule:
/// any other row holding the conversation, tombstoned included, blocks the
/// relaunch when its process is observed `Active`; a failed liveness probe is
/// not `Active`, so it never refuses with this guard. (The double-seat helper
/// that adopt and pane resolution share has the opposite answers to both, and
/// must not leak into revive: plan 174 lists revive's guard as a non-goal.)
#[tokio::test]
async fn revive_conversation_guard_keeps_its_original_rule() {
    const SESSION: &str = "plan174-revive-original-rule";
    let running = ProcIdentity {
        pid: 43_822,
        proc_start: 21,
    };

    // A tombstoned holder whose process still runs blocks the relaunch (a
    // non-Claude conversation has no session-record witness to fall back on).
    let f = fixture(Arc::new(FakeLiveness::new().with_proc(running))).await;
    let mut prior = target();
    prior.harness_session = Some(SESSION.to_string());
    f.services.registry.put(prior.clone()).await.unwrap();
    f.services
        .registry
        .tombstone(&prior.id, "closed")
        .await
        .unwrap();
    let mut holder = SeatDescriptor::new("pij-holder", Harness::Copilot, "/tmp");
    holder.pane = Some("%4".to_string());
    holder.proc = Some(running);
    holder.harness_session = Some(SESSION.to_string());
    f.services.registry.put(holder.clone()).await.unwrap();
    f.services
        .registry
        .tombstone(&holder.id, "closed")
        .await
        .unwrap();
    let resume = json!({"id":"pij-child","session":"fleet"});
    let body = post(&f, &resume).await;
    assert_eq!(body["ok"], false, "{body}");
    assert_eq!(
        body["details"]["code"], "E-RS-REVIVE-CONVERSATION-LIVE",
        "{body}"
    );
    assert_eq!(body["details"]["pane"], "%4", "{body}");

    // A failed probe of a live holder is not `Active`: no conversation refusal.
    let f = fixture(Arc::new(ProbeUnavailable)).await;
    f.services.registry.put(prior.clone()).await.unwrap();
    f.services
        .registry
        .tombstone(&prior.id, "closed")
        .await
        .unwrap();
    let mut live = holder.clone();
    live.id = "pij-live-holder".into();
    f.services.registry.put(live).await.unwrap();
    let body = post(&f, &resume).await;
    assert_ne!(
        body["details"]["code"], "E-RS-REVIVE-CONVERSATION-LIVE",
        "a failed probe must not refuse with the conversation guard: {body}"
    );
    assert!(
        !body["meta"]
            .as_str()
            .unwrap_or_default()
            .contains("E-RS-REVIVE-CONVERSATION-LIVE"),
        "{body}"
    );
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// A Claude seat's session facts: its last call `ago_ms` ago, on a 1h cache.
fn last_call(session: &str, ago_ms: u64) -> Arc<dyn SessionStatusPort> {
    Arc::new(FakeSessionStatus::new().with_reply(
        session,
        SessionStatusReply::Status(SeatStatus {
            model: Fact::native("claude-opus-5-5".to_string()),
            context_used_tokens: Fact::derived(420_000),
            last_call_at_ms: Fact::native(now_ms() - ago_ms),
            cache_ttl: Fact::native(CacheTtl::OneHour),
            ..SeatStatus::unknown()
        }),
    ))
}

fn claude_target(session: &str) -> SeatDescriptor {
    let mut seat = target();
    seat.harness = Harness::Claude;
    seat.harness_session = Some(session.to_string());
    seat.model = Some("claude-opus-5-5".to_string());
    seat
}

/// Plan 225 AC2: `--cheap` refuses, in one line and before any write, a
/// non-Claude seat, a seat whose cache is still warm, `--model` and `--fresh`.
#[tokio::test]
async fn cheap_revive_refuses_a_warm_or_non_claude_seat() {
    const SESSION: &str = "plan225-warm";
    let f = fixture_with(
        Arc::new(FakeLiveness::new()),
        Vec::new(),
        Some(last_call(SESSION, 60_000)),
    )
    .await;
    let cheap = json!({"id":"pij-child","session":"fleet","cheap":true});

    let copilot = target();
    let since = f.services.registry.put(copilot.clone()).await.unwrap();
    let body = post(&f, &cheap).await;
    assert_eq!(body["details"]["code"], "E-RS-REVIVE-CHEAP", "{body}");
    let meta = body["meta"].as_str().unwrap();
    assert!(meta.contains("Claude seats only"), "{body}");
    assert!(!meta.contains('\n'), "one line: {body}");
    unchanged(&f, &copilot, since).await;

    let warm = claude_target(SESSION);
    let since = f.services.registry.put(warm.clone()).await.unwrap();
    let body = post(&f, &cheap).await;
    assert_eq!(body["details"]["code"], "E-RS-REVIVE-CHEAP", "{body}");
    assert!(
        body["meta"].as_str().unwrap().contains("warm prompt cache"),
        "{body}"
    );
    unchanged(&f, &warm, since).await;

    for (extra, said) in [("model", "takes no --model"), ("fresh", "--fresh has none")] {
        let mut body = cheap.clone();
        body[extra] = if extra == "model" {
            json!("claude-haiku-4-5")
        } else {
            json!(true)
        };
        let answer = post(&f, &body).await;
        assert_eq!(answer["details"]["code"], "E-RS-ARG", "{answer}");
        assert!(answer["meta"].as_str().unwrap().contains(said), "{answer}");
    }
    unchanged(&f, &warm, since).await;
}

/// Plan 225 AC1: a cold Claude seat relaunches with `--resume <session>
/// --model claude-sonnet-5-5`, the call returns once it registers, and the
/// seat's recorded model (boundModel) is Sonnet. Nothing typed into any pane
/// is `/model`. A later `--model` revive takes it back to Opus.
#[tokio::test]
async fn cheap_revive_relaunches_a_cold_claude_seat_on_sonnet() {
    const SESSION: &str = "plan225-cold";
    let f = fixture_with(
        Arc::new(FakeLiveness::new()),
        Vec::new(),
        Some(last_call(SESSION, 3 * 60 * 60 * 1_000)),
    )
    .await;
    let prior = claude_target(SESSION);
    f.services.registry.put(prior.clone()).await.unwrap();

    // Stand in for the relaunched harness registering itself.
    let registry = f.services.registry.clone();
    let id = prior.id.clone();
    let registered = tokio::spawn(async move {
        loop {
            if let Ok(Some(mut seat)) = registry.get(&id).await
                && seat.pane.as_deref() != Some("%old")
                && seat.proc.is_none()
            {
                let launched_model = seat.model.clone();
                seat.proc = Some(ProcIdentity {
                    pid: 5151,
                    proc_start: 30,
                });
                registry.put(seat).await.unwrap();
                return launched_model;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });

    let body = post(
        &f,
        &json!({"id":"pij-child","session":"fleet","cheap":true}),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(
        registered.await.unwrap().as_deref(),
        Some(pij_daemon::http::CHEAP_MODEL)
    );
    assert_eq!(pij_daemon::http::CHEAP_MODEL, "claude-sonnet-5-5");
    let revived = f.services.registry.get(&prior.id).await.unwrap().unwrap();
    assert_eq!(revived.model.as_deref(), Some("claude-sonnet-5-5"));
    assert!(revived.proc.is_some(), "returned after registration");
    assert_eq!(revived.harness_session.as_deref(), Some(SESSION));
    let calls = f.tmux.calls();
    let launch = calls
        .iter()
        .find(|call| call.starts_with("new_window:"))
        .expect("launched");
    assert!(
        launch.contains(&format!("\"--resume\", \"{SESSION}\""))
            && launch.contains("\"--model\", \"claude-sonnet-5-5\""),
        "{launch}"
    );
    assert!(
        calls
            .iter()
            .filter(|call| call.starts_with("send_keys:") || call.contains("submit:"))
            .all(|call| !call.contains("/model")),
        "never /model in a pane: {calls:?}"
    );

    // Going back is a separate, explicit revive with the Opus model.
    f.services
        .registry
        .tombstone(&prior.id, "closed")
        .await
        .unwrap();
    let body = post(
        &f,
        &json!({"id":"pij-child","session":"fleet","model":"claude-opus-5-5"}),
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");
    let back = f.services.registry.get(&prior.id).await.unwrap().unwrap();
    assert_eq!(back.model.as_deref(), Some("claude-opus-5-5"));
}
