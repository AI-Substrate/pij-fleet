//! Plan 223 AC2: the secured issuer over HTTP. Spawn mints and stores a
//! nonce; registration issues a capability only to a presented proof; public
//! facts get none; refusals write nothing; tombstones revoke.

use std::sync::Arc;

use pij_core::capability::IssueProof;
use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{Registry, Spine};
use pij_testkit::fakes::{FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux};
use serde_json::{Value, json};

use super::tests::{config, spawn, test_services};
use super::*;
use crate::capability::{CAPABILITY_HEADER, digest_of, test_support::nonce_for};

const SEAT: &str = "pij-capable";
const SESSION: &str = "cap-session";
const SPAWN: &str = "s-capable";
const PROC: ProcIdentity = ProcIdentity {
    pid: 4_321,
    proc_start: 77,
};
/// A second live process, for an incarnation change.
const NEXT: ProcIdentity = ProcIdentity {
    pid: 4_322,
    proc_start: 78,
};

struct Rig {
    registry: Arc<dyn Registry>,
    spine: Arc<FakeSpine>,
    tmux: Arc<FakeTmux>,
    addr: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
}

impl Rig {
    async fn new() -> Self {
        let spine = Arc::new(FakeSpine::new());
        let tmux = Arc::new(FakeTmux::new());
        let mut services = test_services(
            Arc::new(FakeRegistry::new()),
            Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
            Arc::clone(&spine),
        )
        .await;
        services.liveness = Arc::new(FakeLiveness::new().with_proc(PROC).with_proc(NEXT));
        services.tmux = tmux.clone();
        let registry = Arc::clone(&services.registry);
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        Self {
            registry,
            spine,
            tmux,
            addr,
            server,
            client: reqwest::Client::new(),
        }
    }

    fn claim(seat: &str) -> Value {
        json!({
            "id": seat,
            "harness": "omp",
            "folder": "/abs/tree",
            "pid": PROC.pid,
            "proc_start": PROC.proc_start,
            "spawn_id": SPAWN,
            "harness_session": SESSION,
        })
    }

    async fn post(&self, path: &str, body: Value, capability: Option<&str>) -> (u16, Value) {
        let mut request = self
            .client
            .post(format!("http://{}{path}", self.addr))
            .bearer_auth("key");
        if let Some(capability) = capability {
            request = request.header(CAPABILITY_HEADER, capability);
        }
        let response = request.json(&body).send().await.expect("request");
        let status = response.status().as_u16();
        (status, response.json().await.expect("envelope"))
    }

    async fn register(&self, body: Value) -> (u16, Value) {
        self.post("/v1/register", body, None).await
    }

    async fn register_with_nonce(&self, seat: &str, nonce: &str) -> (u16, Value) {
        let mut body = Self::claim(seat);
        body["spawn_nonce"] = json!(nonce);
        self.register(body).await
    }

    async fn report(&self, token: &str) -> (u16, Value) {
        self.post(
            "/v1/activity",
            json!({
                "seat": SEAT,
                "native_session": SESSION,
                "state": "idle",
                "context_tokens": 1_000,
                "context_token": token,
            }),
            None,
        )
        .await
    }

    async fn spine_text(&self) -> String {
        self.spine
            .tail(None, Seq(0))
            .await
            .expect("spine")
            .iter()
            .map(|event| format!("{} {}\n", event.kind, event.payload))
            .collect()
    }

    async fn nonce(&self, seat: &str, expires_at: u64) -> String {
        nonce_for(
            self.registry.as_ref(),
            &SeatId::from(seat),
            SPAWN,
            Harness::Omp,
            expires_at,
        )
        .await
    }
}

fn token_of(body: &Value) -> Option<String> {
    let token = body["data"]["capability"].as_str().map(str::to_string);
    assert_eq!(
        body["data"]["context_token"].as_str(),
        token.as_deref(),
        "the context grant is the capability: {body}"
    );
    token
}

fn refusal_code(body: &Value) -> &str {
    body["details"]["code"].as_str().unwrap_or_default()
}

/// The issuer's happy path, and contract d: the capability and the nonce
/// appear in the registration response and nowhere else a client can read.
#[tokio::test]
async fn a_nonce_issues_a_capability_that_reports_and_never_leaks() {
    let rig = Rig::new().await;
    let nonce = rig.nonce(SEAT, u64::MAX).await;
    let (status, body) = rig.register_with_nonce(SEAT, &nonce).await;
    assert_eq!(status, 200, "{body}");
    let token = token_of(&body).expect("a capability");
    assert_eq!(token.len(), 64);
    assert_ne!(token, nonce, "independent secrets");
    let row = rig
        .registry
        .capability(&SEAT.into())
        .await
        .unwrap()
        .expect("row");
    assert_eq!(row.boundary, Some(PROC));
    assert_eq!(Some(row.digest), digest_of(&token), "only its digest");

    let (status, reply) = rig.report(&token).await;
    assert_eq!(status, 200, "{reply}");

    let mut surfaces = vec![("spine".to_string(), rig.spine_text().await)];
    for path in ["/health", "/v1/seats", "/v1/cards"] {
        let response = rig
            .client
            .get(format!("http://{}{path}", rig.addr))
            .bearer_auth("key")
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "{path}");
        surfaces.push((path.to_string(), response.text().await.unwrap()));
    }
    for (surface, text) in surfaces {
        assert!(
            text.contains(SEAT) || surface == "/health",
            "{surface}: {text}"
        );
        assert!(!text.contains(&token), "{surface} carries the capability");
        assert!(!text.contains(&nonce), "{surface} carries the nonce");
    }
    rig.server.abort();
}

/// The loser of a nonce race, or a replay, is refused E-RS-SPAWN-NONCE-USED
/// with nothing written but the duplicate-claim event; its token never exists.
#[tokio::test]
async fn a_second_registration_with_the_same_nonce_is_refused_and_writes_nothing() {
    let rig = Rig::new().await;
    let nonce = rig.nonce(SEAT, u64::MAX).await;
    let (status, first) = rig.register_with_nonce(SEAT, &nonce).await;
    assert_eq!(status, 200, "{first}");
    let token = token_of(&first).unwrap();
    let row = rig.registry.get(&SEAT.into()).await.unwrap();
    let capability = rig.registry.capability(&SEAT.into()).await.unwrap();
    let before = rig.spine.tail(None, Seq(0)).await.unwrap().len();

    let mut replay = Rig::claim(SEAT);
    replay["spawn_nonce"] = json!(nonce);
    replay["folder"] = json!("/abs/elsewhere");
    let (status, refused) = rig.register(replay).await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refusal_code(&refused), "E-RS-SPAWN-NONCE-USED", "{refused}");
    assert_eq!(refused.get("data"), None);
    assert_eq!(rig.registry.get(&SEAT.into()).await.unwrap(), row);
    assert_eq!(
        rig.registry.capability(&SEAT.into()).await.unwrap(),
        capability
    );
    let after = rig.spine.tail(None, Seq(0)).await.unwrap();
    let added: Vec<_> = after[before..].iter().map(|event| &event.kind).collect();
    assert_eq!(added, [crate::registration::NONCE_REUSED_KIND], "{added:?}");
    assert!(!after[before].payload.contains(&nonce));
    assert_eq!(rig.report(&token).await.0, 200, "the winner is unaffected");
    rig.server.abort();
}

/// An expired nonce, or one issued for another seat, is refused with nothing
/// written; the expired row is dropped as its spawn is touched.
#[tokio::test]
async fn an_expired_nonce_or_another_seats_nonce_is_refused_and_writes_nothing() {
    let rig = Rig::new().await;
    let expired = rig.nonce(SEAT, 2).await;
    let (status, refused) = rig.register_with_nonce(SEAT, &expired).await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refusal_code(&refused), "E-RS-SPAWN-NONCE-INVALID");
    assert!(
        refused["meta"].as_str().unwrap().contains("expired"),
        "{refused}"
    );
    assert_eq!(rig.registry.get(&SEAT.into()).await.unwrap(), None);
    let digest = digest_of(&expired).unwrap();
    let touched = rig
        .registry
        .check_issue(
            &SEAT.into(),
            Some(PROC),
            &IssueProof::Nonce {
                spawn_id: SPAWN.into(),
                digest,
            },
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        touched,
        Err(pij_core::capability::IssueRefusal::NonceUnknown),
        "the expired row was dropped"
    );

    let theirs = rig.nonce("pij-someone-else", u64::MAX).await;
    let (status, refused) = rig.register_with_nonce(SEAT, &theirs).await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refusal_code(&refused), "E-RS-SPAWN-NONCE-INVALID");
    assert!(
        refused["meta"].as_str().unwrap().contains("another seat"),
        "{refused}"
    );
    assert_eq!(rig.registry.get(&SEAT.into()).await.unwrap(), None);
    assert!(rig.spine.tail(None, Seq(0)).await.unwrap().is_empty());
    rig.server.abort();
}

/// F88 row 2, closed: public facts still register, with no capability and no
/// context grant, so they cannot report a size.
#[tokio::test]
async fn public_facts_register_without_a_capability_or_a_grant() {
    let rig = Rig::new().await;
    let (status, body) = rig.register(Rig::claim(SEAT)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["id"], SEAT);
    assert_eq!(token_of(&body), None);
    assert_eq!(rig.registry.capability(&SEAT.into()).await.unwrap(), None);
    let (status, refused) = rig.report(&"0".repeat(64)).await;
    assert_eq!(status, 400, "{refused}");
    // A forged capability header is refused, with nothing written.
    let (status, refused) = rig
        .post("/v1/register", Rig::claim(SEAT), Some(&"0".repeat(64)))
        .await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refusal_code(&refused), "E-RS-SEAT-CAPABILITY");
    rig.server.abort();
}

/// Fix 4 of the review: evidence changes nothing for a registration that
/// presents none. Without a pid (or a pane to observe one) a claim was, and
/// still is, refused by admission ("no bind evidence"), nothing written; the
/// same claim presenting a nonce is refused the same way and its nonce is
/// not consumed, so the bound registration still uses it.
#[tokio::test]
async fn evidence_changes_nothing_for_an_unbound_claim() {
    let rig = Rig::new().await;
    let mut unbound = Rig::claim(SEAT);
    unbound.as_object_mut().unwrap().remove("pid");
    unbound.as_object_mut().unwrap().remove("proc_start");
    let (status, plain) = rig.register(unbound.clone()).await;
    assert_eq!(status, 400, "{plain}");
    assert!(
        plain["meta"]
            .as_str()
            .unwrap()
            .starts_with("no bind evidence"),
        "{plain}"
    );
    assert_eq!(
        plain.get("details"),
        None,
        "the admission refusal, as before"
    );

    let nonce = rig.nonce(SEAT, u64::MAX).await;
    unbound["spawn_nonce"] = json!(nonce);
    let (status, refused) = rig.register(unbound).await;
    assert_eq!(
        (status, &refused["meta"]),
        (400, &plain["meta"]),
        "{refused}"
    );
    assert_eq!(rig.registry.get(&SEAT.into()).await.unwrap(), None);
    let (status, body) = rig.register_with_nonce(SEAT, &nonce).await;
    assert_eq!(status, 200, "the nonce was not consumed: {body}");
    assert!(token_of(&body).is_some());
    rig.server.abort();
}

/// Fix 2 of the review: a same-incarnation re-registration presenting its
/// capability gets the SAME capability back and writes nothing to the
/// capability table; only `rotate: true` (two-phase) issues a new one, and
/// an incarnation change must rotate.
#[tokio::test]
async fn a_held_capability_is_kept_and_rotation_is_explicit() {
    let rig = Rig::new().await;
    let nonce = rig.nonce(SEAT, u64::MAX).await;
    let token = token_of(&rig.register_with_nonce(SEAT, &nonce).await.1).unwrap();
    let row = rig.registry.capability(&SEAT.into()).await.unwrap();

    for _ in 0..2 {
        let (status, held) = rig
            .post("/v1/register", Rig::claim(SEAT), Some(&token))
            .await;
        assert_eq!(status, 200, "{held}");
        assert_eq!(token_of(&held).as_deref(), Some(token.as_str()));
        assert_eq!(rig.registry.capability(&SEAT.into()).await.unwrap(), row);
    }

    let mut moved = Rig::claim(SEAT);
    moved["pid"] = json!(NEXT.pid);
    moved["proc_start"] = json!(NEXT.proc_start);
    let (status, refused) = rig.post("/v1/register", moved.clone(), Some(&token)).await;
    assert_eq!(status, 400, "a new process must rotate: {refused}");
    assert_eq!(refusal_code(&refused), "E-RS-SEAT-CAPABILITY");
    assert_eq!(
        rig.registry.get(&SEAT.into()).await.unwrap().unwrap().proc,
        Some(PROC),
        "nothing written"
    );

    let mut rotate = Rig::claim(SEAT);
    rotate["rotate"] = json!(true);
    let (status, rotated) = rig.post("/v1/register", rotate, Some(&token)).await;
    assert_eq!(status, 200, "{rotated}");
    let rotated = token_of(&rotated).unwrap();
    assert_ne!(rotated, token);
    assert_eq!(
        rig.report(&token).await.0,
        200,
        "phase one: the old one holds"
    );
    assert_eq!(rig.report(&rotated).await.0, 200);
    assert_eq!(rig.report(&token).await.0, 400, "phase two: retired");

    moved["rotate"] = json!(true);
    let (status, moved) = rig.post("/v1/register", moved, Some(&rotated)).await;
    assert_eq!(status, 200, "the owner moves it by rotating: {moved}");
    assert_ne!(token_of(&moved).as_deref(), Some(rotated.as_str()));
    rig.server.abort();
}

/// Revocation: the death sweep's observed-dead tombstone deletes the
/// capability in its transaction, and the same process resuming the seat with
/// public facts does not bring it back (the digest map follows the store).
#[tokio::test]
async fn an_observed_dead_tombstone_revokes_and_a_resume_does_not_restore() {
    let rig = Rig::new().await;
    let nonce = rig.nonce(SEAT, u64::MAX).await;
    let token = token_of(&rig.register_with_nonce(SEAT, &nonce).await.1).unwrap();
    let live: SeatDescriptor = rig.registry.get(&SEAT.into()).await.unwrap().unwrap();
    rig.registry
        .tombstone_if_unchanged(live, "observed-dead".into())
        .await
        .unwrap();
    assert_eq!(rig.registry.capability(&SEAT.into()).await.unwrap(), None);

    let (status, resumed) = rig.register(Rig::claim(SEAT)).await;
    assert_eq!(status, 200, "{resumed}");
    assert_eq!(token_of(&resumed), None);
    let (status, refused) = rig.report(&token).await;
    assert_eq!(
        status, 400,
        "the revoked capability stays revoked: {refused}"
    );
    let (status, refused) = rig
        .post("/v1/register", Rig::claim(SEAT), Some(&token))
        .await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refusal_code(&refused), "E-RS-SEAT-CAPABILITY");
    rig.server.abort();
}

/// Spawn mints the nonce, stores its digest before the launch, and passes it
/// only in the launch's environment; the registration presenting it is issued.
#[tokio::test]
async fn spawn_stores_the_nonce_before_launch_and_passes_it_in_the_environment() {
    let rig = Rig::new().await;
    let (status, spawned) = rig
        .post(
            "/v1/spawn",
            serde_json::to_value(SpawnRequest {
                id: Some(SEAT.into()),
                harness: Harness::Omp,
                allow_retired: false,
                executable: None,
                model: None,
                effort: None,
                cwd: "/abs/tree".to_string(),
                session: Some("fleet".to_string()),
                caller_pane: None,
                name: None,
                parent: None,
                accept_inbound: false,
                wait_seconds: None,
                no_wait: true,
                resume: None,
                role: None,
                caller: None,
            })
            .unwrap(),
            None,
        )
        .await;
    assert_eq!(status, 200, "{spawned}");
    let spawn_id = spawned["data"]["spawn_id"].as_str().unwrap().to_string();
    let launch = rig
        .tmux
        .calls()
        .into_iter()
        .find(|call| call.starts_with("new_window:"))
        .expect("a launch");
    let file = super::spawn_nonce_path(&spawn_id);
    assert!(
        launch.contains(&format!("\"--nonce-file\", {:?}", file.to_str().unwrap())),
        "{launch}"
    );
    assert!(
        !launch.contains("PIJ_SPAWN_NONCE"),
        "never in argv: {launch}"
    );
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The wrapper's half (`pij-rs __spawn-child --nonce-file`), here by hand.
    let nonce = pij_harnesses::take_nonce_file(&file).expect("the handoff");
    assert!(!file.exists());
    assert!(!launch.contains(&nonce), "never in argv");
    assert!(!spawned.to_string().contains(&nonce), "not in the response");
    assert!(!rig.spine_text().await.contains(&nonce), "not on the spine");

    // NONCE_TTL, not the spawn's wait, bounds it.
    let now = system_time_ms().unwrap();
    let ttl = crate::capability::NONCE_TTL.as_millis() as u64;
    for (at, expected) in [
        (now + ttl - 60_000, Ok(())),
        (
            now + ttl + 60_000,
            Err(pij_core::capability::IssueRefusal::NonceExpired),
        ),
    ] {
        let checked = rig
            .registry
            .check_issue(
                &SEAT.into(),
                Some(PROC),
                &IssueProof::Nonce {
                    spawn_id: spawn_id.clone(),
                    digest: digest_of(&nonce).unwrap(),
                },
                at,
            )
            .await
            .unwrap();
        assert_eq!(checked, expected);
    }

    let mut claim = Rig::claim(SEAT);
    claim["spawn_id"] = json!(spawn_id);
    claim["spawn_nonce"] = json!(nonce);
    let (status, body) = rig.register(claim).await;
    assert_eq!(status, 200, "{body}");
    assert!(token_of(&body).is_some());
    rig.server.abort();
}

/// pm-001's ruling: a nonce file no wrapper consumed goes when its expired
/// nonce row is dropped (here, daemon start), never by a sweep.
#[tokio::test]
async fn dropping_an_expired_nonce_row_removes_its_leftover_file() {
    let rig = Rig::new().await;
    let spawn = format!("s-leftover-{}", std::process::id());
    let file = super::spawn_nonce_path(&spawn);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    pij_harnesses::write_nonce_file(&file, "never-read").unwrap();
    let live = format!("s-live-{}", std::process::id());
    let live_file = super::spawn_nonce_path(&live);
    pij_harnesses::write_nonce_file(&live_file, "still-pending").unwrap();
    for (spawn, expires_at) in [(&spawn, 2), (&live, u64::MAX)] {
        nonce_for(
            rig.registry.as_ref(),
            &SEAT.into(),
            spawn,
            Harness::Omp,
            expires_at,
        )
        .await;
    }
    crate::capability::restore(
        rig.registry.as_ref(),
        &crate::capability::Capabilities::default(),
        system_time_ms().unwrap(),
    )
    .await
    .unwrap();
    assert!(
        !file.exists(),
        "the expired spawn's handoff went with its row"
    );
    assert!(live_file.exists(), "a live spawn's handoff stays");
    std::fs::remove_file(live_file).unwrap();
    rig.server.abort();
}
