use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Config};
use pij_core::delivery::delivery_kind;
use pij_core::model::{
    Envelope, Harness, Job, JobId, Pane, PaneProcess, ProcIdentity, SeatDescriptor, SeatId,
    SemanticState,
};
use pij_core::ports::LivenessPort;
use pij_harnesses::proc::ProcLiveness;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};
use serde_json::{Value, json};

use super::tests::{config, spawn};
use super::{InboxAckRequest, router_with_config};
use crate::delivery::{DeliveryService, InboxClaim, NativeInboxIdentity};

struct HostFixture {
    directory: std::path::PathBuf,
    process: Option<std::process::Child>,
}

impl HostFixture {
    fn spawn() -> Self {
        // This is an OS-observation fixture, never a Copilot/SDK session.
        let mut host = Self {
            directory: pij_testkit::fresh_dir("pij-native-registration-host"),
            process: None,
        };
        let executable = host.directory.join("copilot");
        std::fs::copy("/bin/sleep", &executable).expect("copy inert host fixture");
        #[cfg(target_os = "macos")]
        {
            // Sign only the disposable copy; no signing identity/keychain writes.
            let signed = std::process::Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&executable)
                .output()
                .expect("inert host fixture requires macOS /usr/bin/codesign");
            assert!(
                signed.status.success(),
                "cannot ad-hoc sign the inert host fixture: {}",
                String::from_utf8_lossy(&signed.stderr)
            );
        }
        // A short relative path would hide Darwin ps comm truncation.
        assert!(executable.is_absolute());
        assert!(executable.as_os_str().len() > 16);
        host.process = Some({
            // Exec of a fixture this test just wrote can fail with ETXTBSY on
            // Linux while a sibling test's forked child still holds the write
            // fd between fork and exec (CI, PR #381). Retry briefly.
            let mut command = std::process::Command::new(&executable);
            command
                // The fixture must outlive the whole test, not a guess at its
                // duration: at 60s a loaded machine reached the observation after
                // the child had exited, and `read_native_process` correctly
                // reported Ok(None) — read as "process observation failed" rather
                // than "the fixture died" (hit twice on 2026-09-12). Drop still
                // kills and reaps it, so a long sleep leaves nothing behind.
                .arg("3600")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut spawned = None;
            for _ in 0..100 {
                match command.spawn() {
                    Err(error) if error.raw_os_error() == Some(26) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    other => {
                        spawned = Some(other.expect("start inert host fixture"));
                        break;
                    }
                }
            }
            spawned.expect("start inert host fixture: ETXTBSY persisted")
        });
        host
    }

    async fn identity(&self) -> ProcIdentity {
        let pid = self.process.as_ref().expect("owned host is running").id();
        let proc_start = ProcLiveness::new()
            .proc_start(pid)
            .await
            .expect("observe real host start")
            .expect("owned host must be alive");
        assert!(proc_start > 0);
        ProcIdentity { pid, proc_start }
    }

    async fn kill_and_reap(&mut self, identity: ProcIdentity) {
        let process = self.process.as_mut().expect("owned host is running");
        assert_eq!(process.id(), identity.pid);
        process.kill().expect("kill only the owned inert host");
        process.wait().expect("reap the owned inert host");
        self.process = None;
        assert_ne!(
            ProcLiveness::new().proc_start(identity.pid).await.unwrap(),
            Some(identity.proc_start),
            "the exact old incarnation must be absent or replaced, not merely marked dead"
        );
    }
}

impl Drop for HostFixture {
    fn drop(&mut self) {
        if let Some(process) = self.process.as_mut() {
            let _ = process.kill();
            let _ = process.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn contract_identity() -> (SeatDescriptor, NativeInboxIdentity) {
    let contract: Value = serde_json::from_str(&pij_testkit::fixtures::read(
        "native-extension-contract.json",
    ))
    .expect("designed native registration fixture");
    let registration = &contract["registration"]["request_example"];
    let mut seat = SeatDescriptor::new(
        registration["id"].as_str().unwrap(),
        Harness::Copilot,
        registration["folder"].as_str().unwrap(),
    );
    seat.proc = Some(ProcIdentity {
        pid: registration["pid"].as_u64().unwrap() as u32,
        proc_start: registration["proc_start"].as_u64().unwrap(),
    });
    seat.harness_session = Some(registration["harness_session"].as_str().unwrap().into());
    seat.native_extension_delivery = true;
    let identity = NativeInboxIdentity {
        native_session: seat.harness_session.clone(),
        pid: seat.proc.map(|proc| proc.pid),
        proc_start: seat.proc.map(|proc| proc.proc_start),
    };
    (seat, identity)
}

async fn native_host_services(store: &FreshStore, folder: &str, panes: &[&str]) -> crate::Services {
    let mut services = sqlite_services(store).await;
    let mut tmux = FakeTmux::new();
    for pane in panes {
        tmux = tmux
            .with_pane(Pane {
                id: (*pane).into(),
                session: "s".into(),
                window: "w".into(),
                title: "native cold resume".into(),
                cursor_x: Some(0),
                cursor_y: Some(0),
            })
            .with_pane_process(
                pane,
                PaneProcess {
                    // Both owned child hosts have this real, observed ancestor.
                    // Only tmux observation is fake; no actual pane is mutated.
                    pid: std::process::id(),
                    cwd: folder.into(),
                },
            )
            .with_attached_tap(pane);
        tmux.arrange_clear_composer(pane);
    }
    let tmux = Arc::new(tmux);
    services.liveness = Arc::new(ProcLiveness::new());
    services.tmux = tmux.clone();
    services.interaction = Arc::new(pij_harnesses::InteractionGate::new(tmux));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    services
}

fn native_registration(seat: &SeatDescriptor) -> Value {
    let process = seat.proc.expect("native host has an observed identity");
    json!({
        "id": seat.id,
        "harness": "copilot",
        "harness_session": seat.harness_session,
        "folder": seat.folder,
        "pane": seat.pane,
        "pid": process.pid,
        "proc_start": process.proc_start,
        "relay": false,
        "native_extension_delivery": true,
    })
}

async fn native_http_response(
    request: reqwest::RequestBuilder,
    expected_status: reqwest::StatusCode,
) -> Value {
    let response = request
        .bearer_auth("native-key")
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, expected_status, "HTTP envelope: {body}");
    assert_eq!(body["ok"], expected_status.is_success(), "{body}");
    if expected_status == reqwest::StatusCode::BAD_REQUEST {
        assert_eq!(body["error"], "refused", "{body}");
        assert!(body.get("data").is_none(), "refusal has no data: {body}");
    }
    body
}

async fn native_http_claims(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    seat: &SeatId,
    identity: &NativeInboxIdentity,
) -> Vec<InboxClaim> {
    // Non-waiting claims also prove that an empty new inbox cannot see M1.
    let body = native_http_response(
        client.get(format!("http://{addr}/v1/inbox")).query(&[
            ("seat", seat.to_string()),
            ("wait", "false".into()),
            ("native_session", identity.native_session.clone().unwrap()),
            ("pid", identity.pid.unwrap().to_string()),
            ("proc_start", identity.proc_start.unwrap().to_string()),
        ]),
        reqwest::StatusCode::OK,
    )
    .await;
    assert!(body.get("meta").is_none(), "claim must not be held: {body}");
    serde_json::from_value(body["data"].clone()).unwrap()
}

// Real SQLite/HTTP/OS evidence. HostFixture is inert, not the live Copilot proof.
async fn native_session_resume_contract(assertion: &str) {
    let mut old_host = HostFixture::spawn();
    let old_process = old_host.identity().await;
    let new_host = HostFixture::spawn();
    let new_process = new_host.identity().await;
    let (mut seat, _) = contract_identity();
    seat.proc = Some(old_process);
    seat.pane = Some("%149-old".into());
    seat.parent = Some("pij-parent".into());
    let store = FreshStore::new();
    let services = native_host_services(&store, &seat.folder, &["%149-old", "%149-new"]).await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let spine = services.spine.clone();
    let roles = services.roles.clone();
    let bus = services.event_bus.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut initial = native_registration(&seat);
    initial["parent"] = json!(seat.parent);
    initial["role"] = json!("worker");
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&initial),
        reqwest::StatusCode::OK,
    )
    .await;
    let conversation: SeatId = "pij-telegram-chat-149".into();
    bus.publish(pij_core::model::Event {
        seq: None,
        v: 1,
        at: 1,
        kind: "telegram.binding".into(),
        seat: Some(conversation.clone()),
        payload: json!({"target":seat.id,"outbound_msg_id":"telegram-before-resume"}).to_string(),
    })
    .await
    .unwrap();
    let original_binding = spine
        .latest_matching(&conversation, &["telegram.binding"])
        .await
        .unwrap();
    native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from":"pij-peer","to":{"seat":seat.id},"body":"149 queued before restart",
            "msg_id":"149-mail-survives"
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    let (job_id, _) = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    old_host.kill_and_reap(old_process).await;
    let mut retired = registry.get(&seat.id).await.unwrap().unwrap();
    retired.tombstoned_at = Some(1);
    retired.tombstone_reason = Some("observed-dead".into());
    retired.native_extension_delivery = false;
    registry.put(retired).await.unwrap();
    seat.proc = Some(new_process);
    seat.pane = Some("%149-new".into());
    let mut resumed_claim = native_registration(&seat);
    resumed_claim["id"] = json!("");
    let resumed = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&resumed_claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        resumed["data"]["id"],
        seat.id.as_str(),
        "{assertion}: durable address"
    );
    match assertion {
        "mail" => {
            let identity = NativeInboxIdentity {
                native_session: seat.harness_session.clone(),
                pid: Some(new_process.pid),
                proc_start: Some(new_process.proc_start),
            };
            let claims = native_http_claims(&client, addr, &seat.id, &identity).await;
            assert_eq!(claims.len(), 1);
            assert_eq!(claims[0].job_id, job_id);
            assert_eq!(claims[0].message.body, "149 queued before restart");
        }
        "bindings" => {
            let current = registry.get(&seat.id).await.unwrap().unwrap();
            assert_eq!(current.parent, seat.parent);
            assert_eq!(
                roles.read_role(&seat.id).await.unwrap().as_deref(),
                Some("worker")
            );
            assert_eq!(
                spine
                    .latest_matching(&conversation, &["telegram.binding"])
                    .await
                    .unwrap(),
                original_binding
            );
            assert_eq!(resumed["data"]["role"], "worker");
        }
        "state" => {
            let state = native_http_response(
                client
                    .post(format!("http://{addr}/v1/state"))
                    .json(&json!({"id":seat.id})),
                reqwest::StatusCode::OK,
            )
            .await;
            assert_eq!(state["data"]["pid"], new_process.pid);
            assert_eq!(state["data"]["procStart"], new_process.proc_start);
            assert!(state["data"]["tombstonedAt"].is_null());
            assert!(state["data"]["tombstoneReason"].is_null());
        }
        _ => unreachable!(),
    }
    server.abort();
}

#[tokio::test]
async fn native_session_resume_preserves_queued_mail() {
    native_session_resume_contract("mail").await;
}

#[tokio::test]
async fn native_session_resume_preserves_parent_role_and_telegram_binding() {
    native_session_resume_contract("bindings").await;
}

#[tokio::test]
async fn native_session_resume_state_shows_new_process_and_no_tombstone() {
    native_session_resume_contract("state").await;
}
#[tokio::test]
async fn native_transient_session_holds_without_mutating_owner_or_queue() {
    let old_host = HostFixture::spawn();
    let new_host = HostFixture::spawn();
    let (mut owner, _) = contract_identity();
    owner.id = "pij-resume-owner".into();
    owner.proc = Some(old_host.identity().await);
    owner.pane = Some("%151".into());
    let store = FreshStore::new();
    let services = native_host_services(&store, &owner.folder, &["%151"]).await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let spine = services.spine.clone();
    registry.put(owner.clone()).await.unwrap();
    let before = registry.list(Default::default()).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from": "pij-peer", "to": {"seat": owner.id},
            "body": "retained while restart holds", "msg_id": "151-held-mail"
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    let queued = queue.peek(&[delivery_kind(&owner.id)]).await.unwrap();
    let events = spine.tail(None, pij_core::model::Seq(0)).await.unwrap();
    let mut claimant = owner.clone();
    claimant.proc = Some(new_host.identity().await);
    claimant.harness_session = Some("transient-before-resume".into());
    let mut claim = native_registration(&claimant);
    claim["id"] = json!("");
    let response = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(response["command"], "pij register");
    assert_eq!(response["error"], "refused");
    assert_eq!(
        response["details"],
        json!({"retryable": true, "hold": "native-session"})
    );
    assert_eq!(
        response["meta"],
        "native Copilot registration held: pane owned by seat `pij-resume-owner`; awaiting resumed session"
    );
    assert_eq!(registry.list(Default::default()).await.unwrap(), before);
    assert_eq!(
        queue.peek(&[delivery_kind(&owner.id)]).await.unwrap(),
        queued
    );
    assert_eq!(
        spine.tail(None, pij_core::model::Seq(0)).await.unwrap(),
        events
    );
    server.abort();
}

#[tokio::test]
async fn native_transient_hold_yields_to_separate_pane_ownership_refusal() {
    let old_host = HostFixture::spawn();
    let new_host = HostFixture::spawn();
    let (mut owner, _) = contract_identity();
    // The hold-eligible owner sorts before the separate terminal conflict.
    owner.id = "pij-aaa-copilot-owner".into();
    owner.proc = Some(old_host.identity().await);
    owner.pane = Some("%151-shared".into());
    owner.harness_session = Some("owner-session".into());
    let mut relay = owner.clone();
    relay.id = "pij-zzz-relay".into();
    relay.relay = true;
    relay.native_extension_delivery = false;
    relay.proc = None;
    let store = FreshStore::new();
    let services = native_host_services(&store, &owner.folder, &["%151-shared"]).await;
    services.registry.put(owner.clone()).await.unwrap();
    services.registry.put(relay).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut claimant = owner;
    claimant.proc = Some(new_host.identity().await);
    claimant.harness_session = Some("unknown-transient".into());
    let mut claim = native_registration(&claimant);
    claim["id"] = json!("");
    let response = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(response["details"].get("hold").is_none(), "{response}");
    assert!(
        response["meta"]
            .as_str()
            .unwrap()
            .contains("`pij-zzz-relay`"),
        "{response}"
    );
    server.abort();
}

#[tokio::test]
async fn native_transient_session_tombstoned_or_observed_dead_owner_admits_new_address() {
    for tombstoned in [false, true] {
        let mut old_host = HostFixture::spawn();
        let new_host = HostFixture::spawn();
        let old_process = old_host.identity().await;
        let (mut owner, _) = contract_identity();
        owner.id = "pij-retired-pane-owner".into();
        owner.proc = Some(old_process);
        owner.pane = Some("%151-retired".into());
        if tombstoned {
            owner.tombstoned_at = Some(1);
            owner.tombstone_reason = Some("already swept".into());
            owner.native_extension_delivery = false;
        }
        let store = FreshStore::new();
        let services = native_host_services(&store, &owner.folder, &["%151-retired"]).await;
        let registry = services.registry.clone();
        registry.put(owner.clone()).await.unwrap();
        let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
        let client = reqwest::Client::new();
        let new_process = new_host.identity().await;
        let mut claimant = owner.clone();
        claimant.proc = Some(new_process);
        claimant.harness_session = Some("genuinely-new-session".into());
        let mut claim = native_registration(&claimant);
        claim["id"] = json!("");
        if !tombstoned {
            native_http_response(
                client
                    .post(format!("http://{addr}/v1/register"))
                    .json(&claim),
                reqwest::StatusCode::CONFLICT,
            )
            .await;
            old_host.kill_and_reap(old_process).await;
        }
        let admitted = native_http_response(
            client
                .post(format!("http://{addr}/v1/register"))
                .json(&claim),
            reqwest::StatusCode::OK,
        )
        .await;
        let id: SeatId = admitted["data"]["id"].as_str().unwrap().into();
        assert_ne!(id, owner.id);
        assert_eq!(admitted["data"]["binding"], "created");
        let created = registry.get(&id).await.unwrap().unwrap();
        assert_eq!(created.harness_session, claimant.harness_session);
        assert_eq!(created.proc, Some(new_process));
        assert_eq!(created.pane, owner.pane);
        assert!(created.native_extension_delivery);
        let retired = registry.get(&owner.id).await.unwrap().unwrap();
        if tombstoned {
            assert_eq!(retired, owner);
        } else {
            assert!(retired.tombstoned_at.is_some());
            assert!(!retired.native_extension_delivery);
            assert_eq!(retired.harness_session, owner.harness_session);
            assert_eq!(retired.proc, owner.proc);
        }
        assert_eq!(registry.list(Default::default()).await.unwrap().len(), 2);
        server.abort();
    }
}

#[tokio::test]
async fn native_known_historical_session_resumes_instead_of_holding_live_bootstrap() {
    let saved_host = HostFixture::spawn();
    let resumed_host = HostFixture::spawn();
    let new_process = resumed_host.identity().await;
    let (mut saved, _) = contract_identity();
    saved.id = "pij-known-saved-session".into();
    saved.proc = Some(saved_host.identity().await);
    saved.tombstoned_at = Some(1);
    saved.tombstone_reason = Some("historical incarnation".into());
    saved.native_extension_delivery = false;
    let mut bootstrap = saved.clone();
    bootstrap.id = "pij-current-bootstrap".into();
    bootstrap.proc = Some(new_process);
    bootstrap.pane = Some("%151-known".into());
    bootstrap.harness_session = Some("current-bootstrap-session".into());
    bootstrap.tombstoned_at = None;
    bootstrap.tombstone_reason = None;
    bootstrap.native_extension_delivery = true;
    let store = FreshStore::new();
    let services = native_host_services(&store, &saved.folder, &["%151-known"]).await;
    let registry = services.registry.clone();
    registry.put(saved.clone()).await.unwrap();
    registry.put(bootstrap.clone()).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut claim = native_registration(&bootstrap);
    claim["id"] = json!("");
    claim["harness_session"] = json!(saved.harness_session);
    let resumed = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(resumed["data"]["id"], saved.id.as_str());
    assert_eq!(resumed["data"]["binding"], "rebound");
    let current = registry.get(&saved.id).await.unwrap().unwrap();
    assert_eq!(current.harness_session, saved.harness_session);
    assert_eq!(current.proc, Some(new_process));
    assert_eq!(current.pane, bootstrap.pane);
    assert!(current.tombstoned_at.is_none());
    assert!(current.native_extension_delivery);
    let retired = registry.get(&bootstrap.id).await.unwrap().unwrap();
    assert!(retired.tombstoned_at.is_some());
    assert!(!retired.native_extension_delivery);
    assert_eq!(registry.list(Default::default()).await.unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn native_session_ambiguity_returns_typed_retry_without_mutation() {
    let host = HostFixture::spawn();
    let (mut seat, _) = contract_identity();
    seat.proc = Some(host.identity().await);
    seat.pane = None;
    let store = FreshStore::new();
    let services = native_host_services(&store, &seat.folder, &[]).await;
    let registry = services.registry.clone();
    registry.put(seat.clone()).await.unwrap();
    let mut duplicate = seat.clone();
    duplicate.id = "pij-duplicate-conversation".into();
    registry.put(duplicate.clone()).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let response = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&seat)),
        reqwest::StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(response["command"], "pij register");
    assert_eq!(response["error"], "refused");
    assert_eq!(response["details"]["retryable"], true);
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat));
    assert_eq!(registry.get(&duplicate.id).await.unwrap(), Some(duplicate));
    server.abort();
}

#[tokio::test]
async fn native_session_live_address_outranks_two_newer_tombstones() {
    let host = HostFixture::spawn();
    let process = host.identity().await;
    let (mut live, _) = contract_identity();
    live.id = "pij-z-live-conversation".into();
    live.proc = Some(ProcIdentity {
        pid: process.pid,
        proc_start: 1,
    });
    live.parent = Some("pij-live-parent".into());
    let mut first_history = live.clone();
    first_history.id = "pij-a-retired-conversation".into();
    first_history.proc.as_mut().unwrap().proc_start = 30;
    first_history.tombstoned_at = Some(1);
    first_history.native_extension_delivery = false;
    let mut second_history = first_history.clone();
    second_history.id = "pij-m-retired-conversation".into();
    second_history.proc.as_mut().unwrap().proc_start = 20;
    let store = FreshStore::new();
    let services = native_host_services(&store, &live.folder, &[]).await;
    let registry = services.registry.clone();
    for row in [&live, &first_history, &second_history] {
        registry.put(row.clone()).await.unwrap();
    }
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut claim = native_registration(&live);
    claim["id"] = json!("");
    claim["pid"] = json!(process.pid);
    claim["proc_start"] = json!(process.proc_start);
    let response = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(response["data"]["id"], live.id.as_str());
    assert_eq!(response["data"]["binding"], "rebound");
    assert_eq!(
        response["data"]["parent"],
        live.parent.as_ref().unwrap().as_str()
    );
    let current = registry.get(&live.id).await.unwrap().unwrap();
    assert_eq!(current.proc, Some(process));
    assert!(current.tombstoned_at.is_none());
    assert_eq!(
        registry.get(&first_history.id).await.unwrap(),
        Some(first_history)
    );
    assert_eq!(
        registry.get(&second_history.id).await.unwrap(),
        Some(second_history)
    );
    server.abort();
}

#[tokio::test]
async fn native_session_all_tombstones_resume_newest_process_not_first_id() {
    let host = HostFixture::spawn();
    let process = host.identity().await;
    let (mut newest, _) = contract_identity();
    newest.id = "pij-z-newest-conversation".into();
    newest.proc = Some(ProcIdentity {
        pid: process.pid,
        proc_start: 30,
    });
    newest.tombstoned_at = Some(1);
    newest.native_extension_delivery = false;
    let mut older = newest.clone();
    older.id = "pij-m-older-conversation".into();
    older.proc.as_mut().unwrap().proc_start = 20;
    let mut unknown = newest.clone();
    unknown.id = "pij-a-unknown-conversation".into();
    unknown.proc = None;
    let store = FreshStore::new();
    let services = native_host_services(&store, &newest.folder, &[]).await;
    let registry = services.registry.clone();
    for row in [&newest, &older, &unknown] {
        registry.put(row.clone()).await.unwrap();
    }
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut claim = native_registration(&newest);
    claim["id"] = json!("");
    claim["pid"] = json!(process.pid);
    claim["proc_start"] = json!(process.proc_start);
    let response = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(response["data"]["id"], newest.id.as_str());
    assert_eq!(response["data"]["binding"], "rebound");
    let current = registry.get(&newest.id).await.unwrap().unwrap();
    assert_eq!(current.proc, Some(process));
    assert!(current.tombstoned_at.is_none());
    assert!(current.native_extension_delivery);
    assert_eq!(registry.get(&older.id).await.unwrap(), Some(older));
    assert_eq!(registry.get(&unknown.id).await.unwrap(), Some(unknown));
    server.abort();
}

#[tokio::test]
async fn native_registration_preserves_claimed_host_and_reports_atomic_binding_transitions() {
    let mut first_host = HostFixture::spawn();
    let first_process = first_host.identity().await;
    let store = FreshStore::new();
    let folder = first_host.directory.to_string_lossy().into_owned();
    let services = native_host_services(&store, &folder, &["%native-binding"]).await;
    let registry = services.registry.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let (mut seat, _) = contract_identity();
    seat.id = "pij-native-binding".into();
    seat.folder = folder;
    seat.pane = Some("%native-binding".into());
    seat.proc = Some(first_process);

    // The fake pane's process is this test executable, not the native host.
    // Ordinary pane normalization must never replace a native claim's tuple.
    let mut invalid = native_registration(&seat);
    invalid["pid"] = json!(std::process::id());
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&invalid),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(registry.get(&seat.id).await.unwrap().is_none());

    for expected_binding in ["created", "same"] {
        let body = native_http_response(
            client
                .post(format!("http://{addr}/v1/register"))
                .json(&native_registration(&seat)),
            reqwest::StatusCode::OK,
        )
        .await;
        assert_eq!(body["data"]["binding"], expected_binding);
        assert_eq!(body["data"]["proc_source"], "harness");
        assert_eq!(body["data"]["proc"]["pid"], first_process.pid);
        assert_eq!(body["data"]["native_extension_delivery"], true);
        assert_eq!(
            body["data"]["typing_grace_ms"],
            super::resolve_typing_grace_ms()
        );
    }

    first_host.kill_and_reap(first_process).await;
    let resumed_host = HostFixture::spawn();
    let resumed_process = resumed_host.identity().await;
    seat.proc = Some(resumed_process);
    let body = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&seat)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(body["data"]["binding"], "rebound");
    assert_eq!(body["data"]["proc"]["pid"], resumed_process.pid);
    assert_eq!(body["data"]["session"], seat.harness_session.unwrap());
    server.abort();
}

#[tokio::test]
async fn native_saved_session_accepts_spawn_from_auto_discovered_bootstrap() {
    let host = HostFixture::spawn();
    let process = host.identity().await;
    let (mut saved, _) = contract_identity();
    saved.proc = Some(ProcIdentity {
        pid: process.pid,
        proc_start: process.proc_start - 1,
    });
    saved.spawn_id = Some("saved-session-launch".into());
    saved.tombstoned_at = Some(1);
    saved.tombstone_reason = Some("previous host exited".into());
    saved.native_extension_delivery = false;
    let store = FreshStore::new();
    let services = native_host_services(&store, &saved.folder, &["%149-bootstrap"]).await;
    let registry = services.registry.clone();
    registry.put(saved.clone()).await.unwrap();

    let mut bootstrap = SeatDescriptor::new(
        "pij-resume-bootstrap",
        Harness::Copilot,
        saved.folder.clone(),
    );
    bootstrap.pane = Some("%149-bootstrap".into());
    bootstrap.spawn_id = Some("current-host-launch".into());
    registry.put(bootstrap.clone()).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    bootstrap.proc = Some(process);
    bootstrap.harness_session = Some("temporary-bootstrap-session".into());
    let mut claim = native_registration(&bootstrap);
    claim["spawn_id"] = json!(bootstrap.spawn_id);
    let bound = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(bound["data"]["id"], bootstrap.id.as_str());

    // --resume switches the now-bound host from its temporary conversation to
    // the saved one. Its launch id belongs to the bootstrap, not the saved row.
    // Omit supersedes: the daemon must discover that same-host/pane predecessor.
    claim["id"] = json!("");
    claim["harness_session"] = json!(saved.harness_session);
    let resumed = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(resumed["data"]["id"], saved.id.as_str());
    assert_eq!(resumed["data"]["binding"], "rebound");
    assert_eq!(resumed["data"]["spawn_id"], "saved-session-launch");
    let current = registry.get(&saved.id).await.unwrap().unwrap();
    assert_eq!(current.proc, Some(process));
    assert_eq!(current.pane, bootstrap.pane);
    assert!(current.tombstoned_at.is_none());
    assert!(current.tombstone_reason.is_none());
    assert!(current.native_extension_delivery);
    let retired = registry.get(&bootstrap.id).await.unwrap().unwrap();
    assert!(retired.tombstoned_at.is_some());
    assert!(!retired.native_extension_delivery);
    server.abort();
}

#[tokio::test]
async fn native_sqlite_http_prebind_precedes_saved_session_without_retargeting_work() {
    let old_host = HostFixture::spawn();
    let new_host = HostFixture::spawn();
    let (mut first, _) = contract_identity();
    first.proc = Some(old_host.identity().await);
    first.pane = Some("%137-prebind-old".into());
    first.parent = Some("pij-saved-parent".into());
    let store = FreshStore::new();
    let services = native_host_services(
        &store,
        &first.folder,
        &["%137-prebind-old", "%137-prebind-new"],
    )
    .await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let spine = services.spine.clone();
    let roles = services.roles.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut initial = native_registration(&first);
    initial["parent"] = json!(first.parent);
    initial["role"] = json!("pm");
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&initial),
        reqwest::StatusCode::OK,
    )
    .await;
    first.spawn_id = Some("original-launch".into());
    registry.put(first.clone()).await.unwrap();
    let mut prebind =
        SeatDescriptor::new("pij-new-prebind", Harness::Copilot, first.folder.clone());
    prebind.pane = Some("%137-prebind-new".into());
    prebind.spawn_id = Some("verified-spawn".into());
    prebind.parent = Some("pij-chosen-parent".into());
    registry.put(prebind.clone()).await.unwrap();
    roles
        .assert_role(&prebind.id, &prebind.id, Some("worker".into()))
        .await
        .unwrap();
    for (seat, body) in [
        (&first.id, "saved conversation mail"),
        (&prebind.id, "new prebind mail"),
    ] {
        native_http_response(
            client.post(format!("http://{addr}/v1/send")).json(&json!({
                "from":"pij-peer", "to":{"seat":seat}, "body":body, "msg_id":body,
            })),
            reqwest::StatusCode::OK,
        )
        .await;
    }
    let original = queue
        .peek(&[delivery_kind(&first.id)])
        .await
        .unwrap()
        .unwrap();
    let preallocated = queue
        .peek(&[delivery_kind(&prebind.id)])
        .await
        .unwrap()
        .unwrap();
    let mut successor = prebind.clone();
    successor.proc = Some(new_host.identity().await);
    successor.harness_session = first.harness_session.clone();
    let mut claim = native_registration(&successor);
    claim["spawn_id"] = json!(prebind.spawn_id);
    // Even a stale saved-id/parent nomination cannot replace verified spawn intent.
    claim["id"] = json!(first.id);
    claim["parent"] = json!(first.parent);
    let bound = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(bound["data"]["id"], prebind.id.as_str());
    assert_eq!(
        bound["data"]["parent"],
        prebind.parent.as_ref().unwrap().as_str()
    );
    assert_eq!(bound["data"]["role"], "worker");
    assert_eq!(
        bound["data"]["spawn_id"],
        prebind.spawn_id.as_deref().unwrap()
    );
    let replayed = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(replayed["data"]["id"], prebind.id.as_str());
    assert_eq!(
        replayed["data"]["parent"],
        prebind.parent.as_ref().unwrap().as_str()
    );
    assert_eq!(replayed["data"]["role"], "worker");
    assert_eq!(replayed["data"]["binding"], "same");
    let current = registry.get(&prebind.id).await.unwrap().unwrap();
    assert!(current.tombstoned_at.is_none());
    assert_eq!(current.proc, successor.proc);
    assert_eq!(current.parent, prebind.parent);
    let retired = registry.get(&first.id).await.unwrap().unwrap();
    assert!(retired.tombstoned_at.is_some());
    assert!(!retired.native_extension_delivery);
    assert_eq!(retired.parent, first.parent);
    assert_eq!(
        roles.read_role(&first.id).await.unwrap().as_deref(),
        Some("pm")
    );
    let retirement = spine
        .latest_matching(&first.id, &["seat.native-superseded"])
        .await
        .unwrap()
        .expect("saved address retirement event");
    assert_eq!(
        serde_json::from_str::<Value>(&retirement.payload).unwrap(),
        json!({"successor":prebind.id,"spawn_id":prebind.spawn_id,"reason":"native-spawn-prebind"})
    );
    assert_eq!(
        queue.peek(&[delivery_kind(&first.id)]).await.unwrap(),
        Some(original)
    );
    let identity = NativeInboxIdentity {
        native_session: first.harness_session.clone(),
        pid: successor.proc.map(|proc| proc.pid),
        proc_start: successor.proc.map(|proc| proc.proc_start),
    };
    let claims = native_http_claims(&client, addr, &prebind.id, &identity).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, preallocated.0);
    assert_eq!(claims[0].message.body, "new prebind mail");
    assert_eq!(claims[0].message.to, prebind.id);
    server.abort();
}

#[tokio::test]
async fn native_spawn_wait_resolves_prebound_child_resuming_saved_session() {
    let old_host = HostFixture::spawn();
    let child_host = HostFixture::spawn();
    let child_process = child_host.identity().await;
    let (mut saved, _) = contract_identity();
    saved.id = "pij-saved-before-spawn".into();
    saved.proc = Some(old_host.identity().await);
    saved.parent = Some("pij-saved-parent".into());
    let child_id: SeatId = "pij-prebound-resumed-child".into();
    let chosen_parent: SeatId = "pij-chosen-parent".into();
    let store = FreshStore::new();
    let mut services = native_host_services(&store, &saved.folder, &[]).await;
    // FakeTmux's first launched window is %100. Only the tmux observation
    // is scripted; registration still verifies the inert child's OS ancestry.
    services.tmux = Arc::new(FakeTmux::new().with_pane_process(
        "%100",
        PaneProcess {
            pid: std::process::id(),
            cwd: saved.folder.clone(),
        },
    ));
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let spine = services.spine.clone();
    let roles = services.roles.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut initial = native_registration(&saved);
    initial["parent"] = json!(saved.parent);
    initial["role"] = json!("pm");
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&initial),
        reqwest::StatusCode::OK,
    )
    .await;
    native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from":"pij-peer","to":{"seat":saved.id},"body":"saved queue stays saved",
            "msg_id":"149-spawn-saved-mail"
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    let saved_job = queue
        .peek(&[delivery_kind(&saved.id)])
        .await
        .unwrap()
        .unwrap();
    let waiting = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("native-key")
        .timeout(Duration::from_secs(6))
        .json(&super::SpawnRequest {
            id: Some(child_id.clone()),
            harness: Harness::Copilot,
            allow_retired: false,
            executable: None,
            model: None,
            effort: None,
            cwd: saved.folder.clone(),
            session: Some("fleet".into()),
            caller_pane: None,
            name: None,
            parent: Some(chosen_parent.clone()),
            accept_inbound: false,
            wait_seconds: Some(2),
            no_wait: false,
            resume: None,
            role: None,
            caller: None,
        })
        .send();
    let register_child = async {
        let prebind = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if let Some(prebind) = registry.get(&child_id).await.unwrap() {
                    break prebind;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("spawn persists the prebind before waiting");
        assert!(prebind.proc.is_none());
        assert_eq!(prebind.parent, Some(chosen_parent.clone()));
        roles
            .assert_role(&child_id, &child_id, Some("worker".into()))
            .await
            .unwrap();
        native_http_response(
            client.post(format!("http://{addr}/v1/send")).json(&json!({
                "from":"pij-peer","to":{"seat":child_id},"body":"allocated child queue",
                "msg_id":"149-spawn-child-mail"
            })),
            reqwest::StatusCode::OK,
        )
        .await;
        let child_job = queue
            .peek(&[delivery_kind(&child_id)])
            .await
            .unwrap()
            .unwrap();
        let mut child = prebind.clone();
        child.proc = Some(child_process);
        child.harness_session = saved.harness_session.clone();
        let mut claim = native_registration(&child);
        claim["spawn_id"] = json!(prebind.spawn_id);
        // Simulate a saved-session payload without letting it flip spawn intent.
        claim["parent"] = json!(saved.parent);
        let registered = native_http_response(
            client
                .post(format!("http://{addr}/v1/register"))
                .json(&claim),
            reqwest::StatusCode::OK,
        )
        .await;
        (registered, prebind, child_job)
    };
    let (response, (registered, prebind, child_job)) = tokio::join!(waiting, register_child);
    let response = response.expect("spawn waiter response");
    let status = response.status();
    let outcome: Value = response.json().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "spawn must not 504: {outcome}"
    );
    assert_eq!(outcome["data"]["bound"], true);
    assert_eq!(outcome["data"]["pid"], child_process.pid);
    assert_eq!(outcome["data"]["id"], child_id.as_str());
    assert_eq!(outcome["data"]["parent"], chosen_parent.as_str());
    assert_eq!(registered["data"]["id"], child_id.as_str());
    assert_eq!(registered["data"]["parent"], chosen_parent.as_str());
    assert_eq!(registered["data"]["role"], "worker");
    let current = registry.get(&child_id).await.unwrap().unwrap();
    assert_eq!(current.proc, Some(child_process));
    assert!(current.tombstoned_at.is_none());
    let retired = registry.get(&saved.id).await.unwrap().unwrap();
    assert!(retired.tombstoned_at.is_some());
    assert!(!retired.native_extension_delivery);
    assert_eq!(retired.parent, saved.parent);
    assert_eq!(
        roles.read_role(&saved.id).await.unwrap().as_deref(),
        Some("pm")
    );
    let retirement = spine
        .latest_matching(&saved.id, &["seat.native-superseded"])
        .await
        .unwrap()
        .expect("saved address retirement event");
    assert_eq!(
        serde_json::from_str::<Value>(&retirement.payload).unwrap(),
        json!({"successor":child_id,"spawn_id":prebind.spawn_id,"reason":"native-spawn-prebind"})
    );
    assert!(
        spine
            .latest_matching(&child_id, &["spawn.bound"])
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        spine
            .latest_matching(&child_id, &["spawn.failed"])
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        queue.peek(&[delivery_kind(&saved.id)]).await.unwrap(),
        Some(saved_job)
    );
    assert_eq!(
        queue.peek(&[delivery_kind(&child_id)]).await.unwrap(),
        Some(child_job)
    );
    server.abort();
}

#[tokio::test]
async fn native_session_rejects_unverified_spawn_correlation_without_retiring_owner() {
    let old_host = HostFixture::spawn();
    let new_host = HostFixture::spawn();
    let (mut owner, _) = contract_identity();
    owner.proc = Some(old_host.identity().await);
    let store = FreshStore::new();
    let services = native_host_services(&store, &owner.folder, &[]).await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let spine = services.spine.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&owner)),
        reqwest::StatusCode::OK,
    )
    .await;
    owner.spawn_id = Some("saved-launch".into());
    registry.put(owner.clone()).await.unwrap();
    native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from":"pij-peer","to":{"seat":owner.id},"body":"owner keeps its queue",
            "msg_id":"149-forged-spawn-mail"
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    let original = queue.peek(&[delivery_kind(&owner.id)]).await.unwrap();
    let mut claimant = owner.clone();
    claimant.proc = Some(new_host.identity().await);
    let mut claim = native_registration(&claimant);
    claim["spawn_id"] = json!("unverified-new-launch");
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&claim),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(registry.get(&owner.id).await.unwrap(), Some(owner.clone()));
    assert_eq!(
        queue.peek(&[delivery_kind(&owner.id)]).await.unwrap(),
        original
    );
    assert!(
        spine
            .latest_matching(&owner.id, &["seat.native-superseded"])
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn native_sqlite_http_dead_host_rebinds_same_session_and_reclaims_original_body() {
    let mut old_host = HostFixture::spawn();
    let old_process = old_host.identity().await;
    let new_host = HostFixture::spawn();
    let new_process = new_host.identity().await;
    assert_ne!(old_process.pid, new_process.pid);
    let (mut seat, mut old_identity) = contract_identity();
    seat.proc = Some(old_process);
    seat.pane = Some("%137-old".into());
    old_identity.pid = Some(old_process.pid);
    old_identity.proc_start = Some(old_process.proc_start);
    let new_identity = NativeInboxIdentity {
        pid: Some(new_process.pid),
        proc_start: Some(new_process.proc_start),
        ..old_identity.clone()
    };
    let store = FreshStore::new();
    let services = native_host_services(&store, &seat.folder, &["%137-old", "%137-new"]).await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&seat)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat.clone()));
    let sent = native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from": "pij-peer", "to": {"seat": seat.id},
            "body": "M1 original cold-resume body", "msg_id": "native-cold-resume-m1",
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(sent["data"]["outcome"]["outcome"], "queued");
    let (job_id, original_job) = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let old_claims = native_http_claims(&client, addr, &seat.id, &old_identity).await;
    assert_eq!(old_claims.len(), 1);
    assert_eq!(old_claims[0].job_id, job_id);
    assert_eq!(old_claims[0].message.body, "M1 original cold-resume body");
    let running = queue.claimed_delivery(job_id).await.unwrap().unwrap();
    assert_eq!(running.payload, original_job.payload);

    old_host.kill_and_reap(old_process).await;
    assert_eq!(new_host.identity().await, new_process);
    // This is the daemon half of cold resume: runtime selection/FileJournal
    // composition is proven separately, not inferred from this fixed address.
    seat.proc = Some(new_process);
    seat.pane = Some("%137-new".into());
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&seat)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat.clone()));
    assert_eq!(
        queue.claimed_delivery(job_id).await.unwrap(),
        Some(running.clone())
    );
    let refused = native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&InboxAckRequest {
                delivery_outcome: None,
                seat: seat.id.clone(),
                job_id,
                native: old_identity.clone(),
                control_outcome: None,
            }),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(refused["command"], "pij inbox");
    assert!(
        refused["meta"]
            .as_str()
            .unwrap()
            .starts_with("daemon/native-inbox: native incarnation mismatch:")
    );
    assert_eq!(queue.claimed_delivery(job_id).await.unwrap(), Some(running));

    // Exercise the real retry transition instead of waiting for lease expiry.
    // Registration itself must neither ACK nor rewrite the outstanding claim.
    queue.retry(job_id, Duration::ZERO).await.unwrap();
    let retried = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.0, job_id);
    assert_eq!(retried.1.payload, original_job.payload);
    let claims = native_http_claims(&client, addr, &seat.id, &new_identity).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, job_id);
    assert_eq!(claims[0].message, old_claims[0].message);
    assert_eq!(claims[0].native_consumer.as_ref(), Some(&new_identity));
    let ack = native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&InboxAckRequest {
                delivery_outcome: None,
                seat: seat.id.clone(),
                job_id,
                native: new_identity.clone(),
                control_outcome: None,
            }),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(ack["data"], job_id.0);
    assert!(queue.claimed_delivery(job_id).await.unwrap().is_none());
    assert!(
        native_http_claims(&client, addr, &seat.id, &new_identity)
            .await
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn native_sqlite_http_different_session_cannot_take_existing_address_or_mail() {
    let old_host = HostFixture::spawn();
    let old_process = old_host.identity().await;
    let new_host = HostFixture::spawn();
    let new_process = new_host.identity().await;
    assert_ne!(old_process.pid, new_process.pid);
    let (mut seat, mut old_identity) = contract_identity();
    seat.proc = Some(old_process);
    seat.pane = Some("%137-live".into());
    old_identity.pid = Some(old_process.pid);
    old_identity.proc_start = Some(old_process.proc_start);
    let store = FreshStore::new();
    let services = native_host_services(&store, &seat.folder, &["%137-live"]).await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&seat)),
        reqwest::StatusCode::OK,
    )
    .await;
    native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from": "pij-peer", "to": {"seat": seat.id},
            "body": "M1 live owner keeps this body", "msg_id": "native-live-owner-m1",
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    let queued = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let mut contender = seat.clone();
    contender.proc = Some(new_process);
    contender.harness_session = Some("different-native-session".into());
    let refused = native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&contender)),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(refused["command"], "pij register");
    assert_eq!(
        old_host.identity().await,
        old_process,
        "old owner is still actually alive"
    );
    assert_eq!(new_host.identity().await, new_process);
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat.clone()));
    assert_eq!(
        queue.peek(&[delivery_kind(&seat.id)]).await.unwrap(),
        Some(queued.clone())
    );
    assert!(queue.claimed_delivery(queued.0).await.unwrap().is_none());
    let claims = native_http_claims(&client, addr, &seat.id, &old_identity).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, queued.0);
    assert_eq!(claims[0].message.body, "M1 live owner keeps this body");
    assert_eq!(claims[0].native_consumer.as_ref(), Some(&old_identity));
    server.abort();
}

#[tokio::test]
async fn native_sqlite_http_fresh_session_retires_dead_pane_owner_without_retargeting_m1() {
    let mut old_host = HostFixture::spawn();
    let old_process = old_host.identity().await;
    let mut new_host = HostFixture::spawn();
    let new_process = new_host.identity().await;
    assert_ne!(old_process.pid, new_process.pid);
    let (mut first, mut old_identity) = contract_identity();
    first.proc = Some(old_process);
    first.pane = Some("%137-reused".into());
    old_identity.pid = Some(old_process.pid);
    old_identity.proc_start = Some(old_process.proc_start);
    let store = FreshStore::new();
    let services = native_host_services(&store, &first.folder, &["%137-reused"]).await;
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&first)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(registry.get(&first.id).await.unwrap(), Some(first.clone()));
    let sent = native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from": "pij-peer", "to": {"seat": first.id},
            "body": "M1 retained for dead native session", "msg_id": "native-reused-pane-m1",
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(sent["data"]["outcome"]["outcome"], "queued");
    let retained = queue
        .peek(&[delivery_kind(&first.id)])
        .await
        .unwrap()
        .unwrap();
    let payload: Value = serde_json::from_str(&retained.1.payload).unwrap();
    assert_eq!(
        payload["native_target_session"],
        old_identity.native_session.as_deref().unwrap()
    );

    old_host.kill_and_reap(old_process).await;
    assert_eq!(new_host.identity().await, new_process);
    let mut successor = SeatDescriptor::new(
        "pij-copilot-fresh-process",
        Harness::Copilot,
        first.folder.clone(),
    );
    successor.proc = Some(new_process);
    successor.pane = first.pane.clone();
    successor.harness_session = Some("fresh-native-conversation".into());
    successor.native_extension_delivery = true;
    let new_identity = NativeInboxIdentity {
        native_session: successor.harness_session.clone(),
        pid: Some(new_process.pid),
        proc_start: Some(new_process.proc_start),
    };
    let registration = native_registration(&successor);
    assert!(
        registration.get("supersedes").is_none(),
        "different hosts cannot supersede"
    );
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&registration),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        registry.get(&successor.id).await.unwrap(),
        Some(successor.clone())
    );
    let retired = registry.get(&first.id).await.unwrap().unwrap();
    assert!(retired.tombstoned_at.is_some());
    assert!(!retired.native_extension_delivery);
    assert!(
        retired
            .tombstone_reason
            .as_deref()
            .unwrap()
            .contains(successor.id.as_str())
    );
    let mut expected_retired = first.clone();
    expected_retired.tombstoned_at = retired.tombstoned_at;
    expected_retired.tombstone_reason = retired.tombstone_reason.clone();
    expected_retired.native_extension_delivery = false;
    assert_eq!(
        retired, expected_retired,
        "retirement preserves old native ownership facts"
    );
    assert_eq!(
        queue.peek(&[delivery_kind(&first.id)]).await.unwrap(),
        Some(retained.clone())
    );
    assert!(queue.claimed_delivery(retained.0).await.unwrap().is_none());
    let refused = native_http_response(
        client.get(claim_url(addr, &first.id, &old_identity)),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(refused["command"], "pij inbox");
    assert!(
        refused["meta"]
            .as_str()
            .unwrap()
            .starts_with("daemon/native-inbox: native-extension-unavailable:")
    );
    assert!(
        native_http_claims(&client, addr, &successor.id, &new_identity)
            .await
            .is_empty()
    );

    let sent = native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from": "pij-peer", "to": {"seat": successor.id},
            "body": "M2 belongs only to fresh native session", "msg_id": "native-reused-pane-m2",
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(sent["data"]["outcome"]["outcome"], "queued");
    let (new_job_id, new_job) = queue
        .peek(&[delivery_kind(&successor.id)])
        .await
        .unwrap()
        .unwrap();
    assert_ne!(new_job_id, retained.0);
    let payload: Value = serde_json::from_str(&new_job.payload).unwrap();
    assert_eq!(
        payload["native_target_session"],
        new_identity.native_session.as_deref().unwrap()
    );
    let claims = native_http_claims(&client, addr, &successor.id, &new_identity).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, new_job_id);
    assert_eq!(
        claims[0].message.body,
        "M2 belongs only to fresh native session"
    );
    assert_eq!(claims[0].message.to, successor.id);
    assert_eq!(claims[0].native_consumer.as_ref(), Some(&new_identity));
    let ack = native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&InboxAckRequest {
                delivery_outcome: None,
                seat: successor.id.clone(),
                job_id: new_job_id,
                native: new_identity.clone(),
                control_outcome: None,
            }),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(ack["data"], new_job_id.0);
    assert!(queue.claimed_delivery(new_job_id).await.unwrap().is_none());
    assert!(
        native_http_claims(&client, addr, &successor.id, &new_identity)
            .await
            .is_empty()
    );
    assert_eq!(
        queue.peek(&[delivery_kind(&first.id)]).await.unwrap(),
        Some(retained.clone())
    );
    assert!(queue.claimed_delivery(retained.0).await.unwrap().is_none());

    // A -> B -> A restores A's durable address and its own queued work, never B's.
    native_http_response(
        client.post(format!("http://{addr}/v1/send")).json(&json!({
            "from": "pij-peer", "to": {"seat": successor.id},
            "body": "M3 retained for native session B", "msg_id": "native-reused-pane-m3",
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    let retained_b = queue
        .peek(&[delivery_kind(&successor.id)])
        .await
        .unwrap()
        .unwrap();
    let reopened_host = HostFixture::spawn();
    let reopened_process = reopened_host.identity().await;
    assert_ne!(reopened_process.pid, new_process.pid);
    new_host.kill_and_reap(new_process).await;
    let mut reopened = first.clone();
    reopened.proc = Some(reopened_process);
    let reopened_identity = NativeInboxIdentity {
        native_session: first.harness_session.clone(),
        pid: Some(reopened_process.pid),
        proc_start: Some(reopened_process.proc_start),
    };
    assert_eq!(reopened_host.identity().await, reopened_process);
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&native_registration(&reopened)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        registry.get(&reopened.id).await.unwrap(),
        Some(reopened.clone())
    );
    let retired_b = registry.get(&successor.id).await.unwrap().unwrap();
    assert!(retired_b.tombstoned_at.is_some());
    assert!(!retired_b.native_extension_delivery);
    assert!(
        retired_b
            .tombstone_reason
            .as_deref()
            .unwrap()
            .contains(reopened.id.as_str())
    );
    let mut expected_retired_b = successor.clone();
    expected_retired_b.tombstoned_at = retired_b.tombstoned_at;
    expected_retired_b.tombstone_reason = retired_b.tombstone_reason.clone();
    expected_retired_b.native_extension_delivery = false;
    assert_eq!(retired_b, expected_retired_b);
    let restored_claims = native_http_claims(&client, addr, &reopened.id, &reopened_identity).await;
    assert_eq!(restored_claims.len(), 1);
    assert_eq!(restored_claims[0].job_id, retained.0);
    assert_eq!(
        restored_claims[0].message.body,
        "M1 retained for dead native session"
    );
    assert_eq!(
        queue.peek(&[delivery_kind(&successor.id)]).await.unwrap(),
        Some(retained_b.clone())
    );
    assert!(queue.claimed_delivery(retained.0).await.unwrap().is_some());
    assert!(
        queue
            .claimed_delivery(retained_b.0)
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn native_sqlite_http_dead_legacy_and_uncorrelated_prebind_owners_are_not_taken_over() {
    let mut old_host = HostFixture::spawn();
    let old_process = old_host.identity().await;
    let new_host = HostFixture::spawn();
    let new_process = new_host.identity().await;
    assert_ne!(old_process.pid, new_process.pid);
    old_host.kill_and_reap(old_process).await;
    for prebind in [false, true] {
        let (mut owner, _) = contract_identity();
        owner.pane = Some("%137-unverified".into());
        owner.native_extension_delivery = false;
        owner.proc = if prebind { None } else { Some(old_process) };
        if prebind {
            owner.harness_session = None;
            owner.spawn_id = Some("preallocated-spawn-correlation".into());
        }
        let store = FreshStore::new();
        let services = native_host_services(&store, &owner.folder, &["%137-unverified"]).await;
        // Only negative legacy/prebind fixtures are seeded. Positive native
        // attestation always comes from the actual HTTP registration path.
        services.registry.put(owner.clone()).await.unwrap();
        let registry = services.registry.clone();
        let queue = services.queue.clone();
        let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
        let client = reqwest::Client::new();
        native_http_response(
            client.post(format!("http://{addr}/v1/send")).json(&json!({
                "from": "pij-peer", "to": {"seat": owner.id},
                "body": "legacy or prebind context stays put", "msg_id": "native-unverified-m1",
            })),
            reqwest::StatusCode::OK,
        )
        .await;
        let queued = queue
            .peek(&[delivery_kind(&owner.id)])
            .await
            .unwrap()
            .unwrap();
        for same_address in [true, false] {
            if same_address && !prebind {
                continue; // Same-session native resume is now admitted; covered above.
            }
            let mut contender = SeatDescriptor::new(
                if same_address {
                    owner.id.clone()
                } else {
                    "pij-copilot-unverified-contender".into()
                },
                Harness::Copilot,
                owner.folder.clone(),
            );
            contender.proc = Some(new_process);
            contender.pane = owner.pane.clone();
            contender.harness_session = if same_address {
                owner
                    .harness_session
                    .clone()
                    .or_else(|| Some("prebind-without-correlation".into()))
            } else {
                Some("different-native-unverified-contender".into())
            };
            let refused = native_http_response(
                client
                    .post(format!("http://{addr}/v1/register"))
                    .json(&native_registration(&contender)),
                reqwest::StatusCode::BAD_REQUEST,
            )
            .await;
            assert_eq!(refused["command"], "pij register");
            assert_eq!(registry.get(&owner.id).await.unwrap(), Some(owner.clone()));
            if !same_address {
                assert!(registry.get(&contender.id).await.unwrap().is_none());
            }
            assert_eq!(
                queue.peek(&[delivery_kind(&owner.id)]).await.unwrap(),
                Some(queued.clone())
            );
            assert!(queue.claimed_delivery(queued.0).await.unwrap().is_none());
        }
        assert_eq!(new_host.identity().await, new_process);
        server.abort();
    }
}

#[tokio::test]
async fn native_sqlite_http_missing_registration_is_retryable_but_noncopilot_identity_is_refused() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let registry = services.registry.clone();
    let (seat, identity) = contract_identity();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let missing = native_http_response(
        client.get(claim_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(missing["v"], 2);
    assert_eq!(missing["command"], "pij inbox");
    assert_eq!(
        missing["meta"],
        "daemon/native-inbox: native-extension-unavailable: Copilot requires current native registration"
    );
    assert!(registry.get(&seat.id).await.unwrap().is_none());

    let mut noncopilot = seat.clone();
    noncopilot.harness = Harness::Omp;
    noncopilot.native_extension_delivery = false;
    registry.put(noncopilot.clone()).await.unwrap();
    let mismatch = native_http_response(
        client.get(claim_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(mismatch["v"], 2);
    assert_eq!(mismatch["command"], "pij inbox");
    assert_eq!(
        mismatch["meta"],
        "daemon/native-inbox: native incarnation does not match a current external pull seat"
    );
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(noncopilot));
    server.abort();
}

#[tokio::test]
async fn native_http_pane_change_hold_preserves_retryable_wire_discriminator() {
    let response = super::native_inbox_response(
        crate::delivery::NativeInboxPage {
            claims: Vec::new(),
            held_reason: Some("native-pane-changed".into()),
        },
        None,
    );
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        envelope,
        json!({
            "v": 2,
            "ok": true,
            "command": "pij inbox",
            "data": [],
            "meta": "native-consumer-held:native-pane-changed",
        })
    );
}

async fn sqlite_services(store: &FreshStore) -> crate::Services {
    let mut config = Config {
        store_path: store.path(),
        ..Default::default()
    };
    config.adapters.registry = AdapterChoice::Real;
    config.adapters.queue = AdapterChoice::Real;
    config.adapters.spine = AdapterChoice::Real;
    crate::build_services(&config, std::path::Path::new("/tmp/pij-native-test-taps"))
        .await
        .expect("real SQLite registry, queue and spine")
}

fn claim_url(addr: std::net::SocketAddr, seat: &SeatId, identity: &NativeInboxIdentity) -> String {
    let mut url = reqwest::Url::parse(&format!("http://{addr}/v1/inbox")).unwrap();
    let mut query = url.query_pairs_mut();
    query
        .append_pair("seat", seat.as_str())
        .append_pair("wait", "true");
    if let Some(session) = &identity.native_session {
        query.append_pair("native_session", session);
    }
    if let Some(pid) = identity.pid {
        query.append_pair("pid", &pid.to_string());
    }
    if let Some(start) = identity.proc_start {
        query.append_pair("proc_start", &start.to_string());
    }
    drop(query);
    url.into()
}

fn typing_url(
    addr: std::net::SocketAddr,
    seat: &SeatId,
    identity: &NativeInboxIdentity,
) -> reqwest::Url {
    let mut url = reqwest::Url::parse(&claim_url(addr, seat, identity)).unwrap();
    url.set_path("/v1/inbox/typing");
    url
}

#[tokio::test]
async fn native_sqlite_http_hold_release_marker_ack_and_duplicate_contract() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    seat.pane = Some("%137".into());
    services
        .registry
        .put(seat.clone())
        .await
        .expect("fixture attestation");
    services.liveness = Arc::new(FakeLiveness::new().with_proc(seat.proc.unwrap()));
    let tmux = Arc::new(
        FakeTmux::new()
            .with_pane(Pane {
                id: "%137".into(),
                session: "s".into(),
                window: "w".into(),
                title: "native".into(),
                cursor_x: Some(11),
                cursor_y: Some(0),
            })
            .with_attached_tap("%137")
            .with_standing_capture("╰──── hello ─╯"),
    );
    services.tmux = tmux.clone();
    services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
        tmux.clone(),
        Duration::from_secs(60),
        super::resolve_typing_grace_ms(),
    ));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    let queue = services.queue.clone();
    let registry = services.registry.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let send = json!({"from":"pij-peer", "to":{"seat":seat.id}, "body":"PIJ_137_NONCE", "msg_id":"poc-137"});
    let sent: Value = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("native-key")
        .json(&send)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sent["data"]["outcome"]["outcome"], "queued");

    let missing: Value = client
        .get(format!("http://{addr}/v1/inbox?seat={}", seat.id))
        .bearer_auth("native-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(missing["ok"], false, "unattested legacy request refuses");
    for field in ["pid", "proc_start", "native_session"] {
        let mut wrong = identity.clone();
        match field {
            "pid" => wrong.pid = Some(138),
            "proc_start" => wrong.proc_start = Some(1380),
            _ => wrong.native_session = Some("other-native-session".into()),
        }
        let refused: Value = client
            .get(claim_url(addr, &seat.id, &wrong))
            .bearer_auth("native-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(refused["ok"], false, "{field} mismatch must refuse");
    }
    // Self-reported Hold is status only; observing typing must not rewrite it.
    seat.semantic_state = Some(SemanticState::Hold);
    registry.put(seat.clone()).await.unwrap();
    native_http_response(
        client.get(typing_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat.clone()));
    // Native claims neither touch the composer nor treat reported status as consent.
    let claims = native_http_claims(&client, addr, &seat.id, &identity).await;
    assert_eq!(
        claims.len(),
        1,
        "neither composer recency nor self-reported Hold gates native claims"
    );
    assert_eq!(claims[0].message.body, "PIJ_137_NONCE");
    assert_eq!(claims[0].native_consumer.as_ref(), Some(&identity));
    let job = claims[0].job_id;
    let snapshot = native_http_response(
        client.get(typing_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(snapshot["data"]["state"], "observed");
    let grace = snapshot["data"]["typing_grace_ms"].as_u64().unwrap();
    let remaining = snapshot["data"]["retry_after_ms"].as_u64().unwrap();
    assert!(remaining <= grace);
    if grace > 0 {
        assert!(
            remaining > 0,
            "recent composer edit is observed: {snapshot}"
        );
    }
    let since_ms = snapshot["data"]["observed_at_ms"]
        .as_u64()
        .unwrap()
        .saturating_sub(grace - remaining);
    // Plan 136's explicit hold/release wire stays valid; native delivery no longer creates typing holds.
    let deferred = native_http_response(
        client.post(format!("http://{addr}/v1/hold")).json(&json!({
            "seat": seat.id, "job_id": job, "msg_id": "poc-137",
            "reason": "human-typing", "since_ms": since_ms,
        })),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(deferred["data"]["held"], true);
    assert!(
        queue.claimed_delivery(job).await.unwrap().is_none(),
        "hold returns ownership to queue"
    );
    assert_eq!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .unwrap()
            .0,
        job
    );
    if grace > 0 {
        assert!(
            native_http_claims(&client, addr, &seat.id, &identity)
                .await
                .is_empty(),
            "136 hold, not composer policy, owns native claim deferral"
        );
    }

    // Snapshot evidence is informational; only the explicit release below clears this legacy hold.
    tmux.arrange_clear_composer("%137");
    let clearing = native_http_response(
        client.get(typing_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(clearing["data"]["state"], "observed");
    assert_eq!(clearing["data"]["retry_after_ms"], 0);
    assert!(queue.claimed_delivery(job).await.unwrap().is_none());

    // Exercise the existing release declaration; sensor aging is proved separately.
    let released = native_http_response(
        client
            .post(format!("http://{addr}/v1/release"))
            .json(&json!({
                "seat": seat.id, "job_id": job, "msg_id": "poc-137",
                "at_ms": clearing["data"]["observed_at_ms"],
            })),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(released["data"]["released"], true);
    assert!(
        queue.claimed_delivery(job).await.unwrap().is_none(),
        "release is not a claim or ACK"
    );
    let reclaimed = native_http_claims(&client, addr, &seat.id, &identity).await;
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].job_id, job);
    assert_eq!(reclaimed[0].message, claims[0].message);
    assert_eq!(reclaimed[0].native_consumer.as_ref(), Some(&identity));
    assert!(
        queue.claimed_delivery(job).await.unwrap().is_some(),
        "claim is not ack"
    );

    for acknowledgement in [
        InboxAckRequest {
            delivery_outcome: None,
            seat: "pij-forged-nonnative".into(),
            job_id: job,
            native: Default::default(),
            control_outcome: None,
        },
        InboxAckRequest {
            delivery_outcome: None,
            seat: "pij-forged-nonnative".into(),
            job_id: job,
            native: identity.clone(),
            control_outcome: None,
        },
        InboxAckRequest {
            delivery_outcome: None,
            seat: seat.id.clone(),
            job_id: job,
            native: NativeInboxIdentity {
                pid: Some(138),
                ..identity.clone()
            },
            control_outcome: None,
        },
    ] {
        let rejected: Value = client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .bearer_auth("native-key")
            .json(&acknowledgement)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rejected["ok"], false);
        assert!(
            queue.claimed_delivery(job).await.unwrap().is_some(),
            "refusal cannot terminalize"
        );
    }
    let ack: Value = client
        .post(format!("http://{addr}/v1/inbox/ack"))
        .bearer_auth("native-key")
        .json(&InboxAckRequest {
            delivery_outcome: None,
            seat: seat.id.clone(),
            job_id: job,
            native: identity.clone(),
            control_outcome: None,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ack["ok"], true);
    assert_eq!(ack["data"], job.0);
    assert!(queue.claimed_delivery(job).await.unwrap().is_none());
    let duplicate: Value = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("native-key")
        .json(&send)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate["data"]["outcome"]["origin"], "reader-read");
    assert!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .is_none()
    );
    assert!(tmux.calls().iter().all(|call| !call.starts_with("submit")
        && !call.starts_with("type")
        && !call.starts_with("stage")
        && !call.starts_with("commit")
        && !call.starts_with("send_keys")));
    server.abort();
}

#[tokio::test]
async fn native_cli_pane_session_claims_and_acks_without_self_reported_host_tuple() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut seat, _) = contract_identity();
    seat.pane = Some("%native-cli".into());
    // Pane attribution requires the recorded host to be alive (plan 156 rule 2).
    services.liveness = Arc::new(FakeLiveness::new().with_proc(seat.proc.unwrap()));
    services.registry.put(seat.clone()).await.unwrap();
    let sent = services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "manual body")
        .await
        .unwrap();
    let queue = services.queue.clone();
    let delivery = services.delivery.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let wrong = json!({
        "TMUX_PANE": seat.pane, "COPILOT_AGENT_SESSION_ID": "another-conversation",
    });
    native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox"))
            .json(&json!({"argv":["inbox"], "caller":wrong})),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    let caller = json!({
        "TMUX_PANE": seat.pane, "COPILOT_AGENT_SESSION_ID": seat.harness_session,
    });
    let claimed = native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox"))
            .json(&json!({"argv":["inbox"], "caller":caller})),
        reqwest::StatusCode::OK,
    )
    .await;
    let claims: Vec<InboxClaim> = serde_json::from_value(claimed["data"].clone()).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].message.msg_id, sent.msg_id);
    assert_eq!(claims[0].message.body, "manual body");
    native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox/ack"))
            .json(&json!({"job_id":claims[0].job_id, "caller":wrong})),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(
        queue
            .claimed_delivery(claims[0].job_id)
            .await
            .unwrap()
            .is_some()
    );
    let acked = native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox/ack"))
            .json(&json!({"job_id":claims[0].job_id, "caller":caller})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(acked["data"], "reader-read");
    assert!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .is_none()
    );
    // An actual receiver lease changes manual admission, not pane attribution.
    let (_, identity) = contract_identity();
    delivery
        .attest_native_receiver(&seat.id, &identity)
        .await
        .unwrap();
    let live_mail = delivery
        .send("pij-peer".into(), seat.id.clone(), "for live receiver")
        .await
        .unwrap();
    let before = queue.peek(&[delivery_kind(&seat.id)]).await.unwrap();
    let refused = native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox"))
            .json(&json!({"argv":["inbox"], "caller":caller})),
        reqwest::StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], "refused");
    assert_eq!(refused["details"]["code"], "native-receiver-lease-live");
    assert_eq!(refused["details"]["retryable"], true);
    assert!(
        refused["details"]["expires_in_ms"]
            .as_u64()
            .is_some_and(|ms| ms > 0 && ms <= 60_000)
    );
    assert!(refused["meta"].as_str().unwrap().contains("lease"));
    assert_eq!(
        queue.peek(&[delivery_kind(&seat.id)]).await.unwrap(),
        before
    );
    let extension = delivery
        .claim_native_inbox(&seat.id, false, &identity)
        .await
        .unwrap();
    assert_eq!(extension.claims[0].message.msg_id, live_mail.msg_id);
    server.abort();
}

#[tokio::test]
async fn native_receiver_heartbeat_http_is_identity_bound_and_independent_of_hold() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    seat.semantic_state = Some(SemanticState::Hold);
    services.registry.put(seat.clone()).await.unwrap();
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "outstanding mail")
        .await
        .unwrap();
    let queue = services.queue.clone();
    let before = queue.peek(&[delivery_kind(&seat.id)]).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut request = json!({
        "seat":seat.id, "native_session":identity.native_session, "pid":identity.pid,
        "proc_start":identity.proc_start, "observed_at":0, "observed_seq":0,
    });
    let response = native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&request),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(response["data"]["state"], "live");
    let lease = response["data"]["lease_ms"].as_u64().unwrap();
    let renewal = response["data"]["renew_after_ms"].as_u64().unwrap();
    assert!(renewal > 0 && renewal < lease);
    assert_eq!(
        queue.peek(&[delivery_kind(&seat.id)]).await.unwrap(),
        before
    );
    request["proc_start"] = json!(identity.proc_start.unwrap() + 1);
    native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&request),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    request["proc_start"] = json!(identity.proc_start);
    request["observed_seq"] = json!(9_007_199_254_740_992_u64);
    native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&request),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    request["observed_seq"] = json!(0);
    request.as_object_mut().unwrap().remove("observed_at");
    native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&request),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    server.abort();
}

// HTTP/SQLite await real IO while lease time is explicitly advanced. Keep one
// runnable task so Tokio does not auto-jump to reqwest's timeout during that IO.
fn manual_receiver_clock() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    })
}

async fn progress_heartbeat(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    request: &Value,
) -> Value {
    native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(request),
        reqwest::StatusCode::OK,
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn native_frozen_progress_http_expires_at_k_and_manual_pull_preserves_identity() {
    let clock = manual_receiver_clock();
    let host = HostFixture::spawn();
    let (mut seat, _) = contract_identity();
    seat.proc = Some(host.identity().await);
    seat.pane = Some("%152-frozen".into());
    let identity = NativeInboxIdentity {
        native_session: seat.harness_session.clone(),
        pid: seat.proc.map(|proc| proc.pid),
        proc_start: seat.proc.map(|proc| proc.proc_start),
    };
    let store = FreshStore::new();
    let services = native_host_services(&store, &seat.folder, &["%152-frozen"]).await;
    let delivery = services.delivery.clone();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let registration = native_registration(&seat);
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&registration),
        reqwest::StatusCode::OK,
    )
    .await;
    let request = json!({
        "seat":seat.id, "native_session":identity.native_session, "pid":identity.pid,
        "proc_start":identity.proc_start, "observed_at":100, "observed_seq":1,
    });
    let first_heartbeat = progress_heartbeat(&client, addr, &request).await;
    let lease_ms = first_heartbeat["data"]["lease_ms"].as_u64().unwrap();
    let renewal_ms = first_heartbeat["data"]["renew_after_ms"].as_u64().unwrap();
    assert_eq!(lease_ms, renewal_ms * 3);
    let first = delivery
        .send("pij-peer".into(), seat.id.clone(), "running body")
        .await
        .unwrap();
    let claim = native_http_claims(&client, addr, &seat.id, &identity)
        .await
        .remove(0);
    let second = delivery
        .send("pij-peer".into(), seat.id.clone(), "pending body")
        .await
        .unwrap();
    let caller = json!({"TMUX_PANE":seat.pane, "COPILOT_AGENT_SESSION_ID":seat.harness_session});

    // Bursts are not renewal opportunities. Re-register and GET attestation
    // cannot rewrite the progress baseline or the last-progress expiry.
    for _ in 0..6 {
        let reply = progress_heartbeat(&client, addr, &request).await;
        assert_eq!(reply["data"]["state"], "live");
        assert_eq!(reply["data"]["lease_ms"], lease_ms);
    }
    for opportunity in 1..3 {
        tokio::time::advance(Duration::from_millis(renewal_ms)).await;
        let reply = progress_heartbeat(&client, addr, &request).await;
        assert_eq!(reply["data"]["state"], "live");
        assert_eq!(
            reply["data"]["lease_ms"],
            lease_ms - renewal_ms * opportunity,
            "a frozen heartbeat must not add another lease before K"
        );
        assert!(
            reply["data"]["renew_after_ms"].as_u64().unwrap()
                < reply["data"]["lease_ms"].as_u64().unwrap()
        );
        native_http_response(
            client
                .post(format!("http://{addr}/v1/register"))
                .json(&registration),
            reqwest::StatusCode::OK,
        )
        .await;
        assert!(
            native_http_claims(&client, addr, &seat.id, &identity)
                .await
                .is_empty()
        );
        let refused = native_http_response(
            client
                .post(format!("http://{addr}/v1/shim/inbox"))
                .json(&json!({"argv":["inbox"],"caller":caller})),
            reqwest::StatusCode::CONFLICT,
        )
        .await;
        assert_eq!(refused["details"]["code"], "native-receiver-lease-live");
        assert_eq!(
            refused["details"]["expires_in_ms"],
            lease_ms - renewal_ms * opportunity
        );
        assert!(
            queue
                .claimed_delivery(claim.job_id)
                .await
                .unwrap()
                .is_some()
        );
    }
    tokio::time::advance(Duration::from_millis(renewal_ms)).await;
    let mut wrong = request.clone();
    wrong["proc_start"] = json!(identity.proc_start.unwrap() + 1);
    wrong["observed_at"] = json!(200);
    wrong["observed_seq"] = json!(2);
    native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&wrong),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    let stale = progress_heartbeat(&client, addr, &request).await;
    assert_eq!(
        stale["data"],
        json!({
            "state":"stale", "reason":"native-receiver-stale",
            "lease_ms":lease_ms, "renew_after_ms":renewal_ms,
        })
    );
    assert_eq!(delivery.reconcile_native_receivers().await.unwrap(), 2);
    let state = native_http_response(
        client
            .post(format!("http://{addr}/v1/state"))
            .json(&json!({"id":seat.id})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(state["data"]["liveness"], "active");
    assert_eq!(
        state["data"]["native_receiver_reason"],
        "native-receiver-stale"
    );
    let peek = native_http_response(
        client
            .get(format!("http://{addr}/v1/inbox"))
            .query(&[("seat", seat.id.as_str()), ("peek", "true")]),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        peek["details"]["native_receiver_reason"],
        "native-receiver-stale"
    );
    assert!(
        peek["meta"]
            .as_str()
            .unwrap()
            .contains("native-receiver-stale")
    );
    let parked: Vec<InboxClaim> = serde_json::from_value(peek["data"].clone()).unwrap();
    assert_eq!(
        parked
            .iter()
            .map(|row| row.message.msg_id.as_str())
            .collect::<Vec<_>>(),
        vec![first.msg_id.as_str(), second.msg_id.as_str()]
    );
    assert_eq!(parked[0].job_id, claim.job_id);
    for row in &peek["data"].as_array().unwrap()[..] {
        assert_eq!(row["state"], "failed");
        assert_eq!(row["outcome"], "undelivered:native-receiver-unavailable");
    }

    // Once parked, no-outstanding, repeated registration and claims still
    // cannot erase the frozen-progress latch or bypass manual admission.
    native_http_response(
        client
            .post(format!("http://{addr}/v1/register"))
            .json(&registration),
        reqwest::StatusCode::OK,
    )
    .await;
    let held = native_http_response(
        client.get(claim_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(held["data"], json!([]));
    assert_eq!(
        held["details"]["native_receiver_reason"],
        "native-receiver-stale"
    );
    assert_eq!(
        progress_heartbeat(&client, addr, &request).await["data"]["state"],
        "stale"
    );
    let recovered = native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox"))
            .json(&json!({"argv":["inbox"],"caller":caller})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        recovered["details"]["native_receiver_reason"],
        "native-receiver-stale"
    );
    let recovered: Vec<InboxClaim> = serde_json::from_value(recovered["data"].clone()).unwrap();
    assert_eq!(recovered[0].job_id, claim.job_id);
    assert_eq!(recovered[0].message, claim.message);
    assert_eq!(recovered[0].attempt, claim.attempt + 1);
    assert!(
        queue
            .claimed_delivery(claim.job_id)
            .await
            .unwrap()
            .is_some(),
        "manual claim is not an ACK"
    );
    let ack = native_http_response(
        client
            .post(format!("http://{addr}/v1/shim/inbox/ack"))
            .json(&json!({"job_id":claim.job_id,"caller":caller})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(ack["data"], "reader-read");
    assert!(
        queue
            .claimed_delivery(claim.job_id)
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
    clock.abort();
}

#[tokio::test(start_paused = true)]
async fn native_progress_http_idle_busy_hold_and_actual_recovery_are_distinct() {
    let clock = manual_receiver_clock();
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    let delivery = services.delivery.clone();
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut request = json!({
        "seat":seat.id, "native_session":identity.native_session, "pid":identity.pid,
        "proc_start":identity.proc_start, "observed_at":0, "observed_seq":0,
    });
    let initial = progress_heartbeat(&client, addr, &request).await;
    let lease_ms = initial["data"]["lease_ms"].as_u64().unwrap();
    let renewal = Duration::from_millis(initial["data"]["renew_after_ms"].as_u64().unwrap());
    for _ in 0..6 {
        tokio::time::advance(renewal).await;
        let reply = progress_heartbeat(&client, addr, &request).await;
        assert_eq!(reply["data"]["state"], "live");
        assert_eq!(
            reply["data"]["lease_ms"], lease_ms,
            "idle does not require invented observations"
        );
    }
    let sent = delivery
        .send("pij-peer".into(), seat.id.clone(), "pending observation")
        .await
        .unwrap();
    let before = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .expect("receiver has outstanding work");
    seat.state = pij_core::model::SystemState::Working;
    seat.semantic_state = Some(SemanticState::Hold);
    registry.put(seat.clone()).await.unwrap();
    for observation in 1..=4 {
        tokio::time::advance(renewal).await;
        request["observed_at"] = json!(observation * 100);
        request["observed_seq"] = json!(observation);
        let reply = progress_heartbeat(&client, addr, &request).await;
        assert_eq!(reply["data"]["state"], "live");
        assert_eq!(reply["data"]["lease_ms"], lease_ms);
        assert_eq!(
            queue.peek(&[delivery_kind(&seat.id)]).await.unwrap(),
            Some(before.clone())
        );
    }
    for opportunity in 1..=3 {
        tokio::time::advance(renewal).await;
        let reply = progress_heartbeat(&client, addr, &request).await;
        assert_eq!(
            reply["data"]["state"],
            if opportunity < 3 { "live" } else { "stale" }
        );
    }
    let mut wrong = request.clone();
    wrong["native_session"] = json!("wrong-conversation");
    wrong["observed_at"] = json!(500);
    wrong["observed_seq"] = json!(5);
    native_http_response(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&wrong),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(
        progress_heartbeat(&client, addr, &request).await["data"]["state"],
        "stale"
    );
    // Sequence alone is not progress. A restarted extension can reset its local
    // sequence only with a genuinely newer observation; the host tuple is unchanged.
    request["observed_seq"] = json!(5);
    assert_eq!(
        progress_heartbeat(&client, addr, &request).await["data"]["state"],
        "stale"
    );
    request["observed_at"] = json!(500);
    request["observed_seq"] = json!(1);
    let resumed = progress_heartbeat(&client, addr, &request).await;
    assert_eq!(resumed["data"]["state"], "live");
    assert_eq!(resumed["data"]["lease_ms"], lease_ms);
    let state = native_http_response(
        client
            .post(format!("http://{addr}/v1/state"))
            .json(&json!({"id":seat.id})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert!(state["data"].get("native_receiver_reason").is_none());
    assert_eq!(
        queue.peek(&[delivery_kind(&seat.id)]).await.unwrap(),
        Some(before)
    );
    let claimed = native_http_claims(&client, addr, &seat.id, &identity).await;
    assert_eq!(claimed[0].message.msg_id, sent.msg_id);
    assert_eq!(claimed[0].attempt, 0);
    server.abort();
    clock.abort();
}

#[tokio::test(start_paused = true)]
async fn native_progress_http_missing_heartbeats_leave_stale_diagnosis_after_parking() {
    let clock = manual_receiver_clock();
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (seat, identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    let delivery = services.delivery.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let request = json!({
        "seat":seat.id, "native_session":identity.native_session, "pid":identity.pid,
        "proc_start":identity.proc_start, "observed_at":1, "observed_seq":1,
    });
    let initial = progress_heartbeat(&client, addr, &request).await;
    delivery
        .send("pij-peer".into(), seat.id.clone(), "SDK disconnected")
        .await
        .unwrap();
    tokio::time::advance(Duration::from_millis(
        initial["data"]["lease_ms"].as_u64().unwrap(),
    ))
    .await;
    delivery.wait_native_receiver_deadline().await;
    assert_eq!(delivery.reconcile_native_receivers().await.unwrap(), 1);
    let state = native_http_response(
        client
            .post(format!("http://{addr}/v1/state"))
            .json(&json!({"id":seat.id})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        state["data"]["native_receiver_reason"],
        "native-receiver-stale"
    );
    assert_eq!(
        progress_heartbeat(&client, addr, &request).await["data"]["state"],
        "stale"
    );
    server.abort();
    clock.abort();
}

#[tokio::test(start_paused = true)]
async fn native_receiver_absent_after_boot_grace_remains_visible_after_parking() {
    let clock = manual_receiver_clock();
    let store = FreshStore::new();
    let host = HostFixture::spawn();
    let (mut seat, _) = contract_identity();
    seat.proc = Some(host.identity().await);
    seat.pane = Some("%152-absent".into());
    let services = native_host_services(&store, &seat.folder, &["%152-absent"]).await;
    services.registry.put(seat.clone()).await.unwrap();
    let delivery = services.delivery.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    delivery
        .send(
            "pij-peer".into(),
            seat.id.clone(),
            "receiver never reconnected",
        )
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(60)).await;
    delivery.wait_native_receiver_deadline().await;
    assert_eq!(delivery.reconcile_native_receivers().await.unwrap(), 1);
    let state = native_http_response(
        client
            .post(format!("http://{addr}/v1/state"))
            .json(&json!({"id":seat.id})),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(state["data"]["liveness"], "active");
    assert_eq!(
        state["data"]["native_receiver_reason"],
        "native-extension-unavailable"
    );
    let peek = native_http_response(
        client
            .get(format!("http://{addr}/v1/inbox"))
            .query(&[("seat", seat.id.as_str()), ("peek", "true")]),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(
        peek["details"]["native_receiver_reason"],
        "native-extension-unavailable"
    );
    server.abort();
    clock.abort();
}

#[tokio::test(start_paused = true)]
async fn native_recycle_keeps_consumed_job_done_and_parks_only_unconsumed_work() {
    let clock = manual_receiver_clock();
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (seat, identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    let delivery = services.delivery.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let initial = progress_heartbeat(
        &client,
        addr,
        &json!({
            "seat":seat.id, "native_session":identity.native_session, "pid":identity.pid,
            "proc_start":identity.proc_start, "observed_at":1, "observed_seq":1,
        }),
    )
    .await;
    let consumed = delivery
        .send(
            "pij-peer".into(),
            seat.id.clone(),
            "consumed; model still working",
        )
        .await
        .unwrap();
    let first = native_http_claims(&client, addr, &seat.id, &identity)
        .await
        .remove(0);
    delivery
        .acknowledge_inbox(&seat.id, first.job_id, &identity, None)
        .await
        .unwrap();
    let unconsumed = delivery
        .send(
            "pij-peer".into(),
            seat.id.clone(),
            "accepted; consumption not yet proven",
        )
        .await
        .unwrap();
    let second = native_http_claims(&client, addr, &seat.id, &identity)
        .await
        .remove(0);
    // Child replacement attests the same host; it neither requeues done work nor extends the lease.
    delivery
        .attest_native_receiver(&seat.id, &identity)
        .await
        .unwrap();
    tokio::time::advance(Duration::from_millis(
        initial["data"]["lease_ms"].as_u64().unwrap(),
    ))
    .await;
    assert_eq!(delivery.reconcile_native_receivers().await.unwrap(), 1);
    let peek = native_http_response(
        client
            .get(format!("http://{addr}/v1/inbox"))
            .query(&[("seat", seat.id.as_str()), ("peek", "true")]),
        reqwest::StatusCode::OK,
    )
    .await;
    let parked: Vec<InboxClaim> = serde_json::from_value(peek["data"].clone()).unwrap();
    assert_eq!(
        parked
            .iter()
            .map(|claim| claim.message.msg_id.as_str())
            .collect::<Vec<_>>(),
        vec![unconsumed.msg_id.as_str()]
    );
    assert_eq!(parked[0].job_id, second.job_id);
    assert!(
        parked
            .iter()
            .all(|claim| claim.message.msg_id != consumed.msg_id)
    );
    server.abort();
    clock.abort();
}

#[tokio::test]
async fn native_sqlite_malformed_claim_is_held_without_ack() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (seat, identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    let job = services
        .queue
        .enqueue(Job {
            kind: delivery_kind(&seat.id),
            serial_key: seat.id.0.clone(),
            payload: "{broken".into(),
            dedupe_key: "malformed-137".into(),
            attempt: 0,
        })
        .await
        .unwrap();
    let result = services
        .delivery
        .claim_native_inbox(&seat.id, false, &identity)
        .await;
    assert!(result.is_err());
    assert!(
        services
            .queue
            .claimed_delivery(job)
            .await
            .unwrap()
            .is_some()
    );
    services
        .delivery
        .acknowledge_inbox(&seat.id, job, &identity, None)
        .await
        .expect_err("malformed cannot ack");
    assert!(
        services
            .queue
            .claimed_delivery(job)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn shim_and_direct_service_cannot_bypass_native_claim_or_ack() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (seat, identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    services.liveness = Arc::new(FakeLiveness::new().with_proc(seat.proc.unwrap()));
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "native only")
        .await
        .unwrap();
    services
        .delivery
        .claim_inbox(&seat.id, false)
        .await
        .expect_err("direct legacy entrypoint refuses");
    let queue = services.queue.clone();
    let service = services.delivery.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let caller = json!({"PIJ_SESSION_ID":seat.id});
    let claim: Value = client
        .post(format!("http://{addr}/v1/shim/inbox"))
        .bearer_auth("native-key")
        .json(&json!({"argv":["inbox"], "caller":caller}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(claim["ok"], false);
    let jobs = service
        .claim_native_inbox(&seat.id, false, &identity)
        .await
        .unwrap()
        .claims;
    assert_eq!(jobs.len(), 1);
    let ack: Value = client
        .post(format!("http://{addr}/v1/shim/inbox/ack"))
        .bearer_auth("native-key")
        .json(&json!({"job_id":jobs[0].job_id,"caller":caller}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ack["ok"], false);
    assert!(
        queue
            .claimed_delivery(jobs[0].job_id)
            .await
            .unwrap()
            .is_some()
    );
    service
        .acknowledge_inbox(&seat.id, jobs[0].job_id, &identity, None)
        .await
        .unwrap();
    assert!(
        queue
            .claimed_delivery(JobId(jobs[0].job_id.0))
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn stale_native_ack_cannot_retire_replacement_incarnation_work() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (mut seat, old_identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "old native claim")
        .await
        .unwrap();
    let claim = services
        .delivery
        .claim_native_inbox(&seat.id, false, &old_identity)
        .await
        .unwrap()
        .claims
        .remove(0);
    seat.proc = Some(ProcIdentity {
        pid: 13800,
        proc_start: 20260905130000,
    });
    seat.harness_session = Some("replacement-session".into());
    services.registry.put(seat.clone()).await.unwrap();
    services
        .delivery
        .acknowledge_inbox(&seat.id, claim.job_id, &old_identity, None)
        .await
        .expect_err("stale tuple cannot terminalize");
    services
        .delivery
        .acknowledge_inbox(
            &"pij-fake-omp".into(),
            claim.job_id,
            &Default::default(),
            None,
        )
        .await
        .expect_err("calling it another harness cannot bypass native policy");
    assert!(
        services
            .queue
            .claimed_delivery(claim.job_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn native_sqlite_http_new_session_supersedes_seat_without_stranding_new_work() {
    let host = HostFixture::spawn();
    let process = host.identity().await;
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut first, mut original_identity) = contract_identity();
    first.proc = Some(process);
    first.pane = Some("%137".into());
    original_identity.pid = Some(process.pid);
    original_identity.proc_start = Some(process.proc_start);
    services.liveness = Arc::new(ProcLiveness::new());
    let tmux = Arc::new(
        FakeTmux::new()
            .with_pane(Pane {
                id: "%137".into(),
                session: "s".into(),
                window: "w".into(),
                title: "native rollover".into(),
                cursor_x: Some(0),
                cursor_y: Some(0),
            })
            .with_pane_process(
                "%137",
                PaneProcess {
                    pid: process.pid,
                    cwd: first.folder.clone(),
                },
            )
            .with_attached_tap("%137"),
    );
    tmux.arrange_clear_composer("%137");
    services.tmux = tmux.clone();
    services.interaction = Arc::new(pij_harnesses::InteractionGate::new(tmux.clone()));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let mut registration = json!({
        "id": first.id,
        "harness": "copilot",
        "harness_session": first.harness_session,
        "folder": first.folder,
        "pane": first.pane,
        "pid": process.pid,
        "proc_start": process.proc_start,
        "relay": false,
        "native_extension_delivery": true,
    });
    let registered: Value = client
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("native-key")
        .json(&registration)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        registered["ok"], true,
        "first native registration: {registered}"
    );
    assert_eq!(registry.get(&first.id).await.unwrap(), Some(first.clone()));

    let sent: Value = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("native-key")
        .json(&json!({
            "from": "pij-peer", "to": {"seat": first.id},
            "body": "M1 original native session", "msg_id": "native-rollover-m1",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sent["data"]["outcome"]["outcome"], "queued");
    let (old_job_id, old_job) = queue
        .peek(&[delivery_kind(&first.id)])
        .await
        .unwrap()
        .expect("M1 remains pending for the original seat");
    let old_payload: Value = serde_json::from_str(&old_job.payload).unwrap();
    assert_eq!(
        old_payload["native_target_session"],
        original_identity.native_session.as_deref().unwrap()
    );

    // Normal /new keeps the live host/pane but creates S2/N2 and supersedes S1,
    // rather than trying to register a different conversation on the same seat.
    let next_id = SeatId::from("pij-copilot-rollover");
    let next_identity = NativeInboxIdentity {
        native_session: Some("user-created-new-native-session".into()),
        ..original_identity.clone()
    };
    registration["id"] = json!(next_id);
    registration["harness_session"] = json!(next_identity.native_session);
    registration["supersedes"] = json!(first.id);
    let registered: Value = client
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("native-key")
        .json(&registration)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        registered["ok"], true,
        "checked native supersedes: {registered}"
    );
    let successor = registry.get(&next_id).await.unwrap().unwrap();
    assert_eq!(successor.harness, Harness::Copilot);
    assert_eq!(successor.proc, first.proc);
    assert_eq!(successor.pane, first.pane);
    assert_eq!(successor.harness_session, next_identity.native_session);
    assert!(successor.native_extension_delivery);
    assert!(successor.tombstoned_at.is_none());
    let retired = registry.get(&first.id).await.unwrap().unwrap();
    assert!(retired.tombstoned_at.is_some());
    assert!(
        retired
            .tombstone_reason
            .as_deref()
            .unwrap()
            .contains(next_id.as_str())
    );
    assert!(!retired.native_extension_delivery);
    assert_eq!(retired.harness_session, original_identity.native_session);
    let retained = queue
        .peek(&[delivery_kind(&first.id)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.0, old_job_id);
    assert_eq!(
        retained.1.payload, old_job.payload,
        "supersedes must not re-stamp M1"
    );
    assert!(queue.claimed_delivery(old_job_id).await.unwrap().is_none());

    let sent: Value = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("native-key")
        .json(&json!({
            "from": "pij-peer", "to": {"seat": next_id},
            "body": "M2 new native session", "msg_id": "native-rollover-m2",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sent["data"]["outcome"]["outcome"], "queued");
    let (new_job_id, new_job) = queue
        .peek(&[delivery_kind(&next_id)])
        .await
        .unwrap()
        .unwrap();
    assert_ne!(new_job_id, old_job_id);
    let new_payload: Value = serde_json::from_str(&new_job.payload).unwrap();
    assert_eq!(
        new_payload["native_target_session"],
        next_identity.native_session.as_deref().unwrap()
    );
    let admitted: Envelope<Vec<InboxClaim>> = client
        .get(claim_url(addr, &next_id, &next_identity))
        .bearer_auth("native-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let claims = admitted.data.expect("new seat inbox must be reachable");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, new_job_id);
    assert_eq!(claims[0].message.body, "M2 new native session");
    assert_eq!(claims[0].message.to, next_id);
    assert_eq!(claims[0].native_consumer.as_ref(), Some(&next_identity));
    assert!(queue.claimed_delivery(new_job_id).await.unwrap().is_some());
    let ack: Value = client
        .post(format!("http://{addr}/v1/inbox/ack"))
        .bearer_auth("native-key")
        .json(&InboxAckRequest {
            delivery_outcome: None,
            seat: next_id.clone(),
            job_id: new_job_id,
            native: next_identity.clone(),
            control_outcome: None,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        ack["ok"], true,
        "new native tuple may acknowledge M2: {ack}"
    );
    assert_eq!(ack["data"], new_job_id.0);
    assert!(queue.claimed_delivery(new_job_id).await.unwrap().is_none());
    assert!(
        queue
            .peek(&[delivery_kind(&next_id)])
            .await
            .unwrap()
            .is_none()
    );

    let empty: Envelope<Vec<InboxClaim>> = client
        .get(format!("http://{addr}/v1/inbox"))
        .bearer_auth("native-key")
        .query(&[
            ("seat", next_id.as_str()),
            ("wait", "false"),
            (
                "native_session",
                next_identity.native_session.as_deref().unwrap(),
            ),
            ("pid", &process.pid.to_string()),
            ("proc_start", &process.proc_start.to_string()),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        empty
            .data
            .expect("new native inbox remains usable")
            .is_empty()
    );
    let retained = queue
        .peek(&[delivery_kind(&first.id)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.0, old_job_id);
    assert_eq!(
        retained.1.payload, old_job.payload,
        "M1 stays on S1, not delivered to S2"
    );
    assert!(queue.claimed_delivery(old_job_id).await.unwrap().is_none());
    assert!(tmux.calls().iter().all(|call| !call.starts_with("submit")
        && !call.starts_with("type")
        && !call.starts_with("stage")
        && !call.starts_with("commit")
        && !call.starts_with("send_keys")));
    server.abort();
}

#[tokio::test]
async fn queued_native_target_holds_different_session_but_allows_same_session_restart() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (mut seat, original_identity) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "original session work")
        .await
        .unwrap();
    let (job_id, queued) = services
        .queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let stored: Value = serde_json::from_str(&queued.payload).unwrap();
    assert_eq!(
        stored["native_target_session"],
        original_identity.native_session.as_deref().unwrap()
    );
    // Seed a long-lived context hold without a timing-sensitive retry loop.
    // Its attempt count must never turn a wrong-conversation hold into discard.
    let pool = pij_store::open(&store.path()).await.unwrap();
    let seeded = sqlx::query("UPDATE jobs SET attempt = ?1 WHERE id = ?2 AND state = 'pending'")
        .bind(1_000_000_i64)
        .bind(i64::try_from(job_id.0).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(seeded.rows_affected(), 1);
    pool.close().await;

    seat.proc = Some(ProcIdentity {
        pid: 13800,
        proc_start: 20260905130000,
    });
    seat.harness_session = Some("different-conversation".into());
    services.registry.put(seat.clone()).await.unwrap();
    let replacement = NativeInboxIdentity {
        native_session: seat.harness_session.clone(),
        pid: Some(13800),
        proc_start: Some(20260905130000),
    };
    let held = services
        .delivery
        .claim_native_inbox(&seat.id, true, &replacement)
        .await
        .unwrap();
    assert!(held.claims.is_empty());
    let reason = held.held_reason.unwrap();
    assert!(reason.contains(original_identity.native_session.as_deref().unwrap()));
    assert!(reason.contains("resume") && reason.contains("new seat"));
    assert!(
        services
            .queue
            .claimed_delivery(job_id)
            .await
            .unwrap()
            .is_none(),
        "held row returned to pending"
    );
    let (retained_id, retained) = services
        .queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .expect("a context hold cannot expire or discard the queued body");
    assert_eq!(retained_id, job_id);
    assert!(retained.attempt >= 1_000_000);
    assert_eq!(retained.payload, queued.payload);
    let retained_payload: Value = serde_json::from_str(&retained.payload).unwrap();
    assert_eq!(retained_payload["body"], "original session work");

    // Resuming the original native session in a NEW process is permitted.
    seat.harness_session = original_identity.native_session.clone();
    services.registry.put(seat.clone()).await.unwrap();
    let resumed = NativeInboxIdentity {
        native_session: original_identity.native_session,
        ..replacement
    };
    let claims = services
        .delivery
        .claim_native_inbox(&seat.id, false, &resumed)
        .await
        .unwrap()
        .claims;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, job_id);
    assert_eq!(claims[0].message.body, "original session work");
    assert_eq!(claims[0].native_consumer.as_ref(), Some(&resumed));
    services
        .delivery
        .acknowledge_inbox(&seat.id, job_id, &resumed, None)
        .await
        .unwrap();
    assert!(
        services
            .queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn unstamped_prebind_task_reaches_first_verified_native_session() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (seat, identity) = contract_identity();
    let prebind = SeatDescriptor::new(seat.id.clone(), Harness::Copilot, seat.folder.clone());
    services.registry.put(prebind).await.unwrap();
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "initial task")
        .await
        .unwrap();
    let (_, job) = services
        .queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let stored: Value = serde_json::from_str(&job.payload).unwrap();
    assert!(stored.get("native_target_session").is_none());
    services.registry.put(seat.clone()).await.unwrap();
    let claims = services
        .delivery
        .claim_native_inbox(&seat.id, false, &identity)
        .await
        .unwrap()
        .claims;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].message.body, "initial task");
    services
        .delivery
        .acknowledge_inbox(&seat.id, claims[0].job_id, &identity, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_native_consent_sensor_is_explicit_unavailable_not_claim_permission() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    seat.pane = Some("%137".into());
    services.registry.put(seat.clone()).await.unwrap();
    let tmux = Arc::new(
        FakeTmux::new()
            .with_clear_composer("%137")
            .script_tap_error("sensor offline"),
    );
    services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
        tmux.clone(),
        Duration::from_secs(60),
        super::resolve_typing_grace_ms(),
    ));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "must remain queued")
        .await
        .unwrap();
    let queue = services.queue.clone();
    let before = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let unavailable = native_http_response(
        client.get(typing_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(unavailable["data"]["state"], "unavailable");
    assert_eq!(
        unavailable["data"]["reason"],
        "native-typing-sensor-unavailable"
    );
    assert_eq!(
        unavailable["data"]["native_consumer"],
        serde_json::to_value(&identity).unwrap()
    );
    assert!(unavailable["data"].get("retry_after_ms").is_none());
    assert!(unavailable["data"].get("source").is_none());
    assert!(!unavailable.to_string().contains("must remain queued"));
    assert_eq!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(queue.claimed_delivery(before.0).await.unwrap().is_none());
    let restored = native_http_response(
        client.get(typing_url(addr, &seat.id, &identity)),
        reqwest::StatusCode::OK,
    )
    .await;
    assert_eq!(restored["data"]["state"], "observed");
    assert_eq!(restored["data"]["retry_after_ms"], 0);
    let claims = native_http_claims(&client, addr, &seat.id, &identity).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, before.0);
    assert_eq!(claims[0].message.body, "must remain queued");
    server.abort();
}

#[tokio::test]
async fn body_only_http_endpoint_explicitly_refuses_copilot_controls_instead_of_dropping_field() {
    let store = FreshStore::new();
    let services = sqlite_services(&store).await;
    let (seat, _) = contract_identity();
    services.registry.put(seat.clone()).await.unwrap();
    let queue = services.queue.clone();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    for command in ["compact", "new", "reload"] {
        let response: Value = client.post(format!("http://{addr}/v1/send")).bearer_auth("native-key")
            .json(&json!({"from":"pij-peer", "to":{"seat":seat.id}, "body":"", "msg_id":command, "command":command}))
            .send().await.unwrap().json().await.unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["meta"], pij_core::control::COPILOT_CONTROL_REFUSAL);
    }
    assert!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
}

#[tokio::test]
async fn retired_or_prebind_copilot_cannot_be_relabelled_to_bypass_native_inbox() {
    for retired in [true, false] {
        let store = FreshStore::new();
        let mut services = sqlite_services(&store).await;
        let (mut seat, identity) = contract_identity();
        if !retired {
            seat.proc = None;
            seat.harness_session = None;
            seat.native_extension_delivery = false;
        }
        services.registry.put(seat.clone()).await.unwrap();
        services
            .delivery
            .send(
                "pij-peer".into(),
                seat.id.clone(),
                "must remain native-owned",
            )
            .await
            .unwrap();
        let running = if retired {
            let claim = services
                .delivery
                .claim_native_inbox(&seat.id, false, &identity)
                .await
                .unwrap()
                .claims
                .remove(0);
            services
                .registry
                .tombstone(&seat.id, "native session retired")
                .await
                .unwrap();
            Some(claim.job_id)
        } else {
            None
        };
        let before = services.registry.get(&seat.id).await.unwrap();
        services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
            pid: 13800,
            proc_start: 1380,
        }));
        let registry = services.registry.clone();
        let queue = services.queue.clone();
        let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
        let client = reqwest::Client::new();
        let relabel: Value = client.post(format!("http://{addr}/v1/register")).bearer_auth("native-key")
            .json(&json!({"id":seat.id,"harness":"omp","folder":"/isolated/relabel","pid":13800,"proc_start":1380}))
            .send().await.unwrap().json().await.unwrap();
        let bypass: Value = if let Some(job) = running {
            client
                .post(format!("http://{addr}/v1/inbox/ack"))
                .bearer_auth("native-key")
                .json(&json!({"seat":seat.id,"job_id":job}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        } else {
            client
                .get(format!("http://{addr}/v1/inbox?seat={}", seat.id))
                .bearer_auth("native-key")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap()
        };
        assert_eq!(
            bypass["ok"], false,
            "ordinary relabel must not unlock native claim or ack: {bypass}"
        );
        assert_eq!(relabel["ok"], false);
        assert_eq!(registry.get(&seat.id).await.unwrap(), before);
        if let Some(job) = running {
            assert!(queue.claimed_delivery(job).await.unwrap().is_some());
        }
        assert!(
            queue
                .peek(&[delivery_kind(&seat.id)])
                .await
                .unwrap()
                .is_some()
        );
        server.abort();
    }
}

#[tokio::test]
async fn native_typing_http_snapshot_is_authenticated_read_only_and_echoes_owner() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    seat.pane = Some("%137".into());
    services.registry.put(seat.clone()).await.unwrap();
    let tmux = Arc::new(FakeTmux::new().with_clear_composer("%137"));
    services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
        tmux.clone(),
        Duration::from_secs(60),
        super::resolve_typing_grace_ms(),
    ));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "private queued body")
        .await
        .unwrap();
    let queue = services.queue.clone();
    let before = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    let url = typing_url(addr, &seat.id, &identity);
    let response = client
        .get(url.clone())
        .bearer_auth("native-key")
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "typing read route must exist"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["command"], "pij inbox");
    assert_eq!(body["v"], 2);
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["state"], "observed");
    assert_eq!(
        body["data"]["native_consumer"],
        serde_json::to_value(&identity).unwrap()
    );
    assert_eq!(
        body["data"]["typing_grace_ms"],
        super::resolve_typing_grace_ms()
    );
    assert_eq!(body["data"]["retry_after_ms"], 0);
    assert_eq!(body["data"]["source"], "pane-observed-edit-recency");
    assert!(body["data"]["observed_at_ms"].as_u64().unwrap() > 0);
    assert!(!body.to_string().contains("private queued body"));
    assert_eq!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert!(queue.claimed_delivery(before.0).await.unwrap().is_none());
    let claims = native_http_claims(&client, addr, &seat.id, &identity).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, before.0);
    let claimed_before = queue.claimed_delivery(before.0).await.unwrap().unwrap();
    let claimed_snapshot =
        native_http_response(client.get(url.clone()), reqwest::StatusCode::OK).await;
    assert_eq!(
        claimed_snapshot["data"],
        json!({
            "state": "observed", "native_consumer": identity,
            "typing_grace_ms": super::resolve_typing_grace_ms(),
            "observed_at_ms": claimed_snapshot["data"]["observed_at_ms"],
            "retry_after_ms": 0,
            "semantic_hold": false,
            "source": "pane-observed-edit-recency",
        }),
        "snapshot exposes observation and identity, not queue or pane content"
    );
    assert!(!claimed_snapshot.to_string().contains("private queued body"));
    assert_eq!(
        queue.claimed_delivery(before.0).await.unwrap(),
        Some(claimed_before)
    );
    assert_eq!(
        queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        client.get(url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert!(tmux.calls().iter().all(|call| !call.starts_with("type")
        && !call.starts_with("stage")
        && !call.starts_with("send_keys")));
    server.abort();
}

#[tokio::test]
async fn native_typing_recent_nonempty_draft_does_not_gate_native_claim() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    seat.pane = Some("%137".into());
    services.registry.put(seat.clone()).await.unwrap();
    let tmux = Arc::new(
        FakeTmux::new()
            .with_pane(Pane {
                id: "%137".into(),
                session: "s".into(),
                window: "w".into(),
                title: "native".into(),
                cursor_x: Some(11),
                cursor_y: Some(0),
            })
            .with_attached_tap("%137")
            .with_standing_capture("╰──── hello ─╯"),
    );
    services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
        tmux.clone(),
        Duration::from_secs(60),
        60_000,
    ));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    let recent = services
        .interaction
        .fresh_injection_verdict("%137")
        .await
        .unwrap();
    assert!(!recent.permitted);
    assert!(!recent.composer_idle, "the draft is still present");
    // Plan 137: native delivery ignores typing immediately, not only after grace expiry.
    let calls_before = tmux.calls();
    services
        .delivery
        .send("pij-peer".into(), seat.id.clone(), "native body")
        .await
        .unwrap();
    let claims = services
        .delivery
        .claim_native_inbox(&seat.id, false, &identity)
        .await
        .unwrap();
    assert_eq!(
        claims.claims.len(),
        1,
        "a recent draft cannot block native claim: {claims:?}"
    );
    assert_eq!(claims.claims[0].native_consumer.as_ref(), Some(&identity));
    assert_eq!(
        tmux.calls(),
        calls_before,
        "native claim neither reads nor mutates the draft"
    );
}

#[tokio::test]
async fn native_typing_http_rejects_missing_mismatched_and_noncurrent_identity_without_mutation() {
    let store = FreshStore::new();
    let mut services = sqlite_services(&store).await;
    let (mut seat, identity) = contract_identity();
    seat.pane = Some("%137".into());
    services.registry.put(seat.clone()).await.unwrap();
    let tmux = Arc::new(FakeTmux::new().with_clear_composer("%137"));
    services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
        tmux.clone(),
        Duration::from_secs(60),
        super::resolve_typing_grace_ms(),
    ));
    services.delivery = Arc::new(
        DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            services.event_bus.clone(),
        )
        .unwrap(),
    );
    services
        .delivery
        .send(
            "pij-peer".into(),
            seat.id.clone(),
            "private identity fixture",
        )
        .await
        .unwrap();
    let registry = services.registry.clone();
    let queue = services.queue.clone();
    let before = queue
        .peek(&[delivery_kind(&seat.id)])
        .await
        .unwrap()
        .unwrap();
    let calls_before = tmux.calls();
    let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
    let client = reqwest::Client::new();
    for field in ["all", "native_session", "pid", "proc_start"] {
        let mut missing = identity.clone();
        match field {
            "native_session" => missing.native_session = None,
            "pid" => missing.pid = None,
            "proc_start" => missing.proc_start = None,
            _ => missing = NativeInboxIdentity::default(),
        }
        let refused = native_http_response(
            client.get(typing_url(addr, &seat.id, &missing)),
            reqwest::StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(refused["v"], 2);
        assert_eq!(refused["command"], "pij inbox");
        assert!(
            refused["meta"]
                .as_str()
                .unwrap()
                .contains("native incarnation mismatch"),
            "{field}: {refused}"
        );
    }
    for field in ["native_session", "pid", "proc_start"] {
        let mut wrong = identity.clone();
        match field {
            "native_session" => wrong.native_session = Some("another-native-session".into()),
            "pid" => wrong.pid = Some(identity.pid.unwrap() + 1),
            _ => wrong.proc_start = Some(identity.proc_start.unwrap() + 1),
        }
        let refused = native_http_response(
            client.get(typing_url(addr, &seat.id, &wrong)),
            reqwest::StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(refused["v"], 2);
        assert_eq!(refused["command"], "pij inbox");
        assert!(
            refused["meta"]
                .as_str()
                .unwrap()
                .contains("native incarnation mismatch"),
            "{field}: {refused}"
        );
    }
    let absent = SeatId::from("pij-unregistered-native");
    let missing = native_http_response(
        client.get(typing_url(addr, &absent, &identity)),
        reqwest::StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(missing["command"], "pij inbox");
    assert!(
        missing["meta"]
            .as_str()
            .unwrap()
            .contains("native-extension-unavailable")
    );
    assert!(registry.get(&absent).await.unwrap().is_none());
    for state in ["retired", "noncopilot", "unattested"] {
        let mut invalid = seat.clone();
        match state {
            "retired" => {
                invalid.tombstoned_at = Some(1);
                invalid.native_extension_delivery = false;
            }
            "noncopilot" => {
                invalid.harness = Harness::Omp;
                invalid.native_extension_delivery = false;
            }
            _ => invalid.native_extension_delivery = false,
        }
        registry.put(invalid.clone()).await.unwrap();
        let refused = native_http_response(
            client.get(typing_url(addr, &seat.id, &identity)),
            reqwest::StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(refused["v"], 2);
        assert_eq!(refused["command"], "pij inbox");
        assert!(
            refused["meta"]
                .as_str()
                .unwrap()
                .starts_with("daemon/native-inbox:"),
            "{state}: {refused}"
        );
        assert_eq!(registry.get(&seat.id).await.unwrap(), Some(invalid));
        assert_eq!(
            queue
                .peek(&[delivery_kind(&seat.id)])
                .await
                .unwrap()
                .unwrap(),
            before
        );
        assert!(queue.claimed_delivery(before.0).await.unwrap().is_none());
    }
    assert_eq!(
        tmux.calls(),
        calls_before,
        "unproven owner never observes a pane"
    );
    server.abort();
}

#[tokio::test]
async fn native_typing_http_unavailable_observation_preserves_reported_status_and_queue_ownership()
{
    for unavailable in ["unrecognized", "tap-unavailable", "no-pane"] {
        let store = FreshStore::new();
        let mut services = sqlite_services(&store).await;
        let (mut seat, identity) = contract_identity();
        seat.pane = (unavailable != "no-pane").then(|| "%137".into());
        services.registry.put(seat.clone()).await.unwrap();
        let pane = Pane {
            id: "%137".into(),
            session: "s".into(),
            window: "w".into(),
            title: "native".into(),
            cursor_x: Some(0),
            cursor_y: Some(0),
        };
        let tmux = Arc::new(match unavailable {
            "unrecognized" => FakeTmux::new()
                .with_pane(pane)
                .with_attached_tap("%137")
                .with_standing_capture("private unrecognized pane content"),
            "tap-unavailable" => FakeTmux::new()
                .with_pane(pane)
                .with_standing_capture("╰────────╯"),
            _ => FakeTmux::new(),
        });
        services.interaction = Arc::new(pij_harnesses::InteractionGate::with_typing_grace(
            tmux.clone(),
            Duration::ZERO,
            super::resolve_typing_grace_ms(),
        ));
        if unavailable == "unrecognized" {
            let legacy = services
                .interaction
                .fresh_injection_verdict("%137")
                .await
                .unwrap();
            assert!(
                legacy.permitted,
                "legacy unknown-layout idle latch has expired"
            );
            assert_eq!(
                legacy.reason,
                Some(pij_core::delivery::DeliveryDeferralReason::Unrecognized)
            );
        }
        services.delivery = Arc::new(
            DeliveryService::new(
                services.registry.clone(),
                services.queue.clone(),
                services.transport.clone(),
                services.interaction.clone(),
                services.event_bus.clone(),
            )
            .unwrap(),
        );
        services
            .delivery
            .send(
                "pij-peer".into(),
                seat.id.clone(),
                "private unavailable body",
            )
            .await
            .unwrap();
        let registry = services.registry.clone();
        let queue = services.queue.clone();
        let before = queue
            .peek(&[delivery_kind(&seat.id)])
            .await
            .unwrap()
            .unwrap();
        let (addr, server) = spawn(router_with_config(services, config("native-key", &[]))).await;
        let client = reqwest::Client::new();
        for claimed in [false, true] {
            if claimed {
                let claims = native_http_claims(&client, addr, &seat.id, &identity).await;
                assert_eq!(
                    claims.len(),
                    1,
                    "{unavailable} cannot silently gate native claim"
                );
                assert_eq!(claims[0].job_id, before.0);
            }
            seat.semantic_state = Some(SemanticState::Hold);
            registry.put(seat.clone()).await.unwrap();
            let claimed_before = queue.claimed_delivery(before.0).await.unwrap();
            assert_eq!(claimed_before.is_some(), claimed);
            let snapshot = native_http_response(
                client.get(typing_url(addr, &seat.id, &identity)),
                reqwest::StatusCode::OK,
            )
            .await;
            let data = &snapshot["data"];
            let mut expected = json!({
                "state": "unavailable", "native_consumer": identity,
                "typing_grace_ms": super::resolve_typing_grace_ms(),
                "observed_at_ms": data["observed_at_ms"],
                "semantic_hold": false,
                "reason": "native-typing-sensor-unavailable",
            });
            if unavailable == "no-pane" && super::resolve_typing_grace_ms() == 0 {
                expected["state"] = json!("observed");
                expected.as_object_mut().unwrap().remove("reason");
                expected["retry_after_ms"] = json!(0);
                expected["source"] = json!("pane-observed-edit-recency");
            }
            assert_eq!(data, &expected, "{unavailable}, claimed={claimed}");
            assert!(data["observed_at_ms"].as_u64().unwrap() > 0);
            assert!(!snapshot.to_string().contains("private"));
            assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat.clone()));
            assert_eq!(
                queue
                    .peek(&[delivery_kind(&seat.id)])
                    .await
                    .unwrap()
                    .unwrap(),
                before
            );
            assert_eq!(
                queue.claimed_delivery(before.0).await.unwrap(),
                claimed_before
            );
        }
        assert!(tmux.calls().iter().all(|call| !call.starts_with("submit")
            && !call.starts_with("type")
            && !call.starts_with("stage")
            && !call.starts_with("commit")
            && !call.starts_with("send_keys")));
        server.abort();
    }
}
