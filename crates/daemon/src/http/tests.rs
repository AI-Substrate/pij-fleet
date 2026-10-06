use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Json, State};
use axum::http::HeaderMap;
use axum::routing::post;
use pij_core::config::PeerDefinition;
use pij_core::error::Result;
use pij_core::model::{
    DeliveryOrigin, Destination, Envelope, Event, Harness, Job, JobId, Outcome, Pane, ProcIdentity,
    Receipt, SeatDescriptor,
};
use pij_core::ports::{DeliveryAck, DeliveryEnqueue, Queue, Registry, SeatFilter, Spine, TmuxPort};
use pij_core::wire::{self, WireEvent};
use pij_testkit::fakes::{FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux};

use super::*;
use crate::federation::{FederationPolicy, FederationService, WorkerStep};

#[derive(Default)]
struct CountingQueue {
    jobs: Mutex<Vec<Job>>,
}

impl CountingQueue {
    fn jobs(&self) -> Vec<Job> {
        self.jobs.lock().expect("queue mutex").clone()
    }
}

#[async_trait]
impl Queue for CountingQueue {
    async fn note_delivered(
        &self,
        _recipient: &SeatId,
        _msg_id: &str,
        _origin: DeliveryOrigin,
    ) -> Result<Option<DeliveryOrigin>> {
        // The counting request fixture models no ledger. Claiming would be a lie
        // about a path it does not have; refusing keeps the fixture honest, and
        // this route never direct-delivers in these tests.
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/counting-queue".to_string(),
            message: "the counting request fixture does not model delivered ids".to_string(),
        })
    }

    async fn forget_delivered(&self, _recipient: &SeatId, _msg_id: &str) -> Result<()> {
        Ok(())
    }

    async fn admitted(&self, recipient: &SeatId, msg_id: &str) -> Result<bool> {
        let kind = format!("delivery:{}", recipient.as_str());
        Ok(self
            .jobs
            .lock()
            .expect("queue mutex")
            .iter()
            .any(|job| job.kind == kind && job.dedupe_key == msg_id))
    }

    async fn enqueue(&self, job: Job) -> Result<JobId> {
        let mut jobs = self.jobs.lock().expect("queue mutex");
        jobs.push(job);
        Ok(JobId(jobs.len() as u64))
    }

    async fn enqueue_delivery(&self, job: Job) -> Result<DeliveryEnqueue> {
        // Delegates to `enqueue` now that EVERY arrival consults the ledger, not
        // only forwarded ones (review F3). This fixture counts request-path
        // enqueues; it models no delivered ids, so it can never answer
        // AlreadyDelivered — and saying so by delegating is honest, where
        // refusing would make the request path 500 on its own success case.
        let id = self.enqueue(job).await?;
        Ok(DeliveryEnqueue::Queued {
            job_id: id,
            not_before_ms: 0,
        })
    }

    async fn claim(&self, _kinds: &[String], _worker: &str) -> Result<Option<(JobId, Job)>> {
        Ok(None)
    }

    async fn peek(&self, _kinds: &[String]) -> Result<Option<(JobId, Job)>> {
        Ok(None)
    }

    async fn claimed_delivery(&self, _job: JobId) -> Result<Option<Job>> {
        Ok(None)
    }

    async fn terminal_delivery_state(
        &self,
        _job: JobId,
        _recipient: &SeatId,
    ) -> Result<Option<&'static str>> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot inspect terminal deliveries".into(),
        })
    }

    async fn heartbeat_delivery(
        &self,
        _job: JobId,
        _recipient: &SeatId,
        _attempt: u32,
    ) -> Result<bool> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot heartbeat deliveries".into(),
        })
    }

    async fn ack(&self, _job: JobId, _outcome: Outcome) -> Result<()> {
        Ok(())
    }

    async fn ack_delivery(&self, _job: JobId, _origin: DeliveryOrigin) -> Result<DeliveryAck> {
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/counting-queue".to_string(),
            message: "the request path must not delivery-ack: it never claims".to_string(),
        })
    }

    /// This counter only ever observes the REQUEST path, which enqueues and
    /// returns; nothing here claims, so nothing here can retry. Refusing rather
    /// than silently succeeding keeps that true: if a future route learns to
    /// retry, this test fails loudly instead of counting a no-op as a success.
    async fn retry(&self, _job: JobId, _delay: std::time::Duration) -> Result<()> {
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/counting-queue".to_string(),
            message: "the request path must not retry: it never claims".to_string(),
        })
    }

    async fn record_delivery_deferral(
        &self,
        _job: JobId,
        _reason: &str,
        _draft_sha: Option<&str>,
        _at: u64,
        _spine: &dyn Spine,
    ) -> Result<Vec<Event>> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot record delivery deferrals".into(),
        })
    }

    async fn delivery_deferrals(
        &self,
        _recipient: &SeatId,
    ) -> Result<Vec<pij_core::model::DeliveryDeferral>> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot inspect delivery deferrals".into(),
        })
    }

    async fn defer(&self, _job: JobId, _delay: Duration) -> Result<pij_core::ports::DeferOutcome> {
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/counting-queue".to_string(),
            message: "the enqueue-only fixture must not defer".to_string(),
        })
    }

    async fn release_deferred(&self, _job: JobId) -> Result<pij_core::ports::ReleaseOutcome> {
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/counting-queue".to_string(),
            message: "the enqueue-only fixture must not release deferrals".to_string(),
        })
    }
    async fn hold_fyi(
        &self,
        _fyi: &pij_core::fyi::HeldFyi,
        _spine: &dyn Spine,
    ) -> Result<Vec<Event>> {
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/counting-queue".to_string(),
            message: "the enqueue-only fixture does not hold FYIs".to_string(),
        })
    }
    /// Nothing is ever held here, so a ride-along claim finds nothing.
    async fn claim_fyis(
        &self,
        _recipient: &SeatId,
        _via: &str,
        _at: u64,
        _spine: &dyn Spine,
    ) -> Result<(Vec<pij_core::fyi::HeldFyi>, Vec<Event>)> {
        Ok((Vec::new(), Vec::new()))
    }
    async fn enqueue_delivery_carrying_fyis(
        &self,
        job: Job,
        _via: &str,
        _at: u64,
        _attach: pij_core::ports::AttachFyis,
        _spine: &dyn Spine,
    ) -> Result<(DeliveryEnqueue, Vec<Event>)> {
        // Nothing is ever held here: carrying FYIs is an ordinary enqueue.
        Ok((self.enqueue_delivery(job).await?, Vec::new()))
    }
    async fn pending_fyi_count(&self, _recipient: &SeatId) -> Result<u64> {
        Ok(0)
    }

    async fn enqueue_fyi_flush(
        &self,
        _job: Job,
        _via: &str,
        _at: u64,
        _attach: pij_core::ports::AttachFyis,
        _spine: &dyn Spine,
    ) -> Result<(Option<DeliveryEnqueue>, Vec<Event>)> {
        // Nothing is ever held here, so a flush carries nothing.
        Ok((None, Vec::new()))
    }

    async fn read_claimed_fyis(
        &self,
        _recipient: &SeatId,
        _claimed_at_ms: u64,
    ) -> Result<Vec<pij_core::fyi::HeldFyi>> {
        Ok(Vec::new())
    }

    async fn recover_native_delivery(&self, _job: JobId, _recipient: &SeatId) -> Result<bool> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot recover native deliveries".into(),
        })
    }

    async fn claim_extension(
        &self,
        _kinds: &[String],
        _worker: &str,
        _lease: pij_core::ports::ExtensionLease,
        _at: u64,
        _spine: &dyn Spine,
        _recovery_allowed: bool,
    ) -> Result<pij_core::ports::ExtensionClaim> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot claim an extension inbox".into(),
        })
    }

    async fn peek_parked(&self, _kinds: &[String]) -> Result<Vec<pij_core::ports::ParkedDelivery>> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot inspect parked deliveries".into(),
        })
    }

    async fn park_delivery(
        &self,
        _job: JobId,
        _recipient: &SeatId,
        _attempt: u32,
        _evidence: &pij_core::ports::ParkingEvidence<'_>,
        _spine: &dyn Spine,
    ) -> Result<(Option<Job>, Vec<Event>)> {
        Err(PijError::Adapter {
            adapter: "test/counting-queue".into(),
            message: "enqueue-only fixture cannot park deliveries".into(),
        })
    }
}

pub(super) async fn test_services(
    registry: Arc<FakeRegistry>,
    queue: Arc<dyn Queue>,
    spine: Arc<FakeSpine>,
) -> Services {
    let mut services = crate::build_services(
        &pij_core::config::Config::default(),
        std::path::Path::new("/tmp/pij-test-pane-signals"),
    )
    .await
    .expect("fake services");
    let store = services.governance.fixture_store();
    // Hermetic: no test reads the operator's real Claude process records.
    services.claude_homes = Arc::from([]);
    let bus = Arc::new(crate::events::EventBus::new(spine, 16).expect("event bus"));
    services.registry = Arc::new(crate::events::PublishedFakeRegistry::new(
        registry,
        bus.clone(),
    ));
    services.queue = queue;
    services.spine = bus.clone();
    services.event_bus = bus;
    services.roles = Arc::new(super::role::RoleService::new(
        services.registry.clone(),
        store.clone(),
        services.event_bus.clone(),
    ));
    // This helper scripts core port failures. Atomic-governance tests instead
    // use the complete shared-SQL graph in support/governance_http.rs.
    services.governance = Arc::new(super::governance::GovernanceService::new(
        store.clone(),
        services.event_bus.clone(),
        pij_testkit::fresh_dir("pij-http-governance"),
    ));
    // REBUILD the delivery service from the swapped parts. It was constructed by
    // `build_services` from the ORIGINAL fakes, so overwriting the fields above
    // left it holding a registry, queue and bus that no assertion could see — a
    // fixture that looks injected and is not.
    services.delivery = Arc::new(
        crate::delivery::DeliveryService::new(
            Arc::clone(&services.registry),
            Arc::clone(&services.queue),
            Arc::clone(&services.transport),
            Arc::clone(&services.interaction),
            Arc::clone(&services.event_bus),
        )
        .expect("delivery service"),
    );
    services.decisions = Arc::new(super::decisions::DecisionService::new(
        store.clone(),
        services.registry.clone(),
        services.event_bus.clone(),
        services.delivery.clone(),
        pij_core::config::AdapterChoice::Fake,
        pij_core::config::AdapterChoice::Fake,
    ));
    services.anomalies = Arc::new(super::anomalies::AnomalyService::new(
        store,
        services.registry.clone(),
        services.event_bus.clone(),
        services.liveness.clone(),
    ));
    services
}

async fn redelivery_fixture() -> (
    sqlx::SqlitePool,
    Services,
    Arc<FakeRegistry>,
    Arc<pij_store::SqliteSpine>,
) {
    let pool = pij_store::open("").await.expect("isolated queue");
    let queue = Arc::new(pij_store::SqliteQueue::new(pool.clone(), 300, 1_024).unwrap());
    let registry = Arc::new(FakeRegistry::new());
    for (id, harness) in [
        ("pij-reader", Harness::Omp),
        ("pij-parent", Harness::Pi),
        ("pij-stranger", Harness::Pi),
    ] {
        let mut seat = SeatDescriptor::new(id, harness, "/abs/tree");
        if id == "pij-reader" {
            seat.parent = Some("pij-parent".into());
        }
        registry.put(seat).await.unwrap();
    }
    let spine = Arc::new(pij_store::SqliteSpine::new(pool.clone()));
    let bus = Arc::new(crate::events::EventBus::new(spine.clone(), 32).unwrap());
    let mut services = test_services(registry.clone(), queue, Arc::new(FakeSpine::new())).await;
    services.registry = Arc::new(crate::events::PublishedFakeRegistry::new(
        registry.clone(),
        bus.clone(),
    ));
    services.spine = bus.clone();
    services.event_bus = bus.clone();
    services.roles = Arc::new(super::role::RoleService::new(
        services.registry.clone(),
        pij_store::SqliteOrchestration::new(pool.clone()),
        bus.clone(),
    ));
    services.delivery = Arc::new(
        crate::delivery::DeliveryService::new(
            services.registry.clone(),
            services.queue.clone(),
            services.transport.clone(),
            services.interaction.clone(),
            bus,
        )
        .unwrap(),
    );
    for id in ["lost-body", "successor"] {
        services
            .delivery
            .accept(Msg {
                from: "pij-parent".into(),
                from_machine: None,
                to: "pij-reader".into(),
                body: format!("body for {id}"),
                command: None,
                msg_id: id.into(),
                in_reply_to: None,
            })
            .await
            .unwrap();
    }
    (pool, services, registry, spine)
}

async fn redelivery_request(request: reqwest::RequestBuilder) -> serde_json::Value {
    request
        .bearer_auth("key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .expect("machine-decodable inbox envelope")
}

#[tokio::test]
async fn redelivery_heartbeat_renews_only_claim_age_until_consumption() {
    let (pool, services, _, spine) = redelivery_fixture().await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let inbox = format!("http://{addr}/v1/inbox?seat=pij-reader");
    let first = redelivery_request(client.get(&inbox)).await;
    let id = first["data"][0]["job_id"].as_u64().unwrap();
    sqlx::query("UPDATE jobs SET claimed_at=unixepoch()-40, lease_expirations=1 WHERE id=?")
        .bind(id as i64)
        .execute(&pool)
        .await
        .unwrap();
    let before = spine.tail(None, pij_core::model::Seq(0)).await.unwrap();
    let heartbeat = redelivery_request(
        client
            .post(format!("http://{addr}/v1/inbox/heartbeat"))
            .json(&serde_json::json!({"seat":"pij-reader","job_id":id})),
    )
    .await;
    assert_eq!(heartbeat["ok"], true, "{heartbeat}");
    assert_eq!(
        heartbeat["data"],
        serde_json::json!({"job_id":id,"state":"running"})
    );
    let golden: serde_json::Value = serde_json::from_str(&pij_testkit::fixtures::read(
        "golden/api/inbox-recovery.json",
    ))
    .unwrap();
    assert_eq!(heartbeat, golden["heartbeat"]["response"]);
    let claim: (String, i64, i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT state, attempt, lease_expirations, unixepoch()-claimed_at, acked_at FROM jobs WHERE id=?",
    )
    .bind(id as i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (claim.0.as_str(), claim.1, claim.2, claim.4),
        ("running", 0, 1, None)
    );
    assert!(claim.3 <= 2, "heartbeat must reset claim age: {claim:?}");
    assert_eq!(
        spine.tail(None, pij_core::model::Seq(0)).await.unwrap(),
        before
    );
    let delivered: i64 = sqlx::query_scalar("SELECT count(*) FROM delivered_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(delivered, 0, "holding is not ReaderRead");
    assert_eq!(
        redelivery_request(client.get(&inbox)).await["data"],
        serde_json::json!([])
    );
    let ack = redelivery_request(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&serde_json::json!({"seat":"pij-reader","job_id":id})),
    )
    .await;
    assert_eq!(ack["ok"], true, "{ack}");
    let delivered: i64 = sqlx::query_scalar("SELECT count(*) FROM delivered_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(delivered, 1, "only consumption records delivery");
    server.abort();
}

#[tokio::test]
async fn redelivery_heartbeat_reports_terminal_state_without_renewing_or_acknowledging() {
    for state in ["done", "failed"] {
        let (pool, services, _, spine) = redelivery_fixture().await;
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        let client = reqwest::Client::new();
        let first =
            redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
        let id = first["data"][0]["job_id"].as_u64().unwrap();
        let mut ack = serde_json::json!({"seat":"pij-reader","job_id":id});
        if state == "failed" {
            ack["delivery_outcome"] = "undelivered:harness-swallowed".into();
        }
        assert_eq!(
            redelivery_request(
                client
                    .post(format!("http://{addr}/v1/inbox/ack"))
                    .json(&ack),
            )
            .await["ok"],
            true
        );
        let before = spine.tail(None, pij_core::model::Seq(0)).await.unwrap();
        let before_row: (String, Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT state,claimed_at,acked_at FROM jobs WHERE id=?")
                .bind(id as i64)
                .fetch_one(&pool)
                .await
                .unwrap();
        let heartbeat = redelivery_request(
            client
                .post(format!("http://{addr}/v1/inbox/heartbeat"))
                .json(&serde_json::json!({"seat":"pij-reader","job_id":id})),
        )
        .await;
        assert_eq!(heartbeat["ok"], true, "{heartbeat}");
        assert_eq!(
            heartbeat["data"],
            serde_json::json!({"job_id":id,"state":state})
        );
        assert_eq!(
            spine.tail(None, pij_core::model::Seq(0)).await.unwrap(),
            before
        );
        let after_row: (String, Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT state,claimed_at,acked_at FROM jobs WHERE id=?")
                .bind(id as i64)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(after_row, before_row);
        let wrong_recipient = redelivery_request(
            client
                .post(format!("http://{addr}/v1/inbox/heartbeat"))
                .json(&serde_json::json!({"seat":"pij-stranger","job_id":id})),
        )
        .await;
        assert_eq!(wrong_recipient["ok"], false);
        server.abort();
    }
}

#[tokio::test]
async fn redelivery_heartbeat_rejects_wrong_recipient_control_pending_and_native_claims() {
    for case in [
        "wrong-recipient",
        "control",
        "pending",
        "native",
        "tombstoned",
    ] {
        let (pool, services, registry, _) = redelivery_fixture().await;
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        let client = reqwest::Client::new();
        let first =
            redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
        let mut id = first["data"][0]["job_id"].as_u64().unwrap();
        match case {
            "control" => {
                sqlx::query(
                    "UPDATE jobs SET payload=json_set(payload, '$.command', 'compact') WHERE id=?",
                )
                .bind(id as i64)
                .execute(&pool)
                .await
                .unwrap();
            }
            "pending" => {
                id = sqlx::query_scalar::<_, i64>("SELECT id FROM jobs WHERE state='pending'")
                    .fetch_one(&pool)
                    .await
                    .unwrap() as u64;
            }
            "native" => {
                let mut seat = registry.get(&"pij-reader".into()).await.unwrap().unwrap();
                seat.harness = Harness::Claude;
                registry.put(seat).await.unwrap();
            }
            "tombstoned" => {
                registry
                    .tombstone(&"pij-reader".into(), "host exited")
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let before: (String, Option<i64>, i64, Option<i64>) = sqlx::query_as(
            "SELECT state, claimed_at, lease_expirations, acked_at FROM jobs WHERE id=?",
        )
        .bind(id as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        let seat = if case == "wrong-recipient" {
            "pij-stranger"
        } else {
            "pij-reader"
        };
        let rejected = redelivery_request(
            client
                .post(format!("http://{addr}/v1/inbox/heartbeat"))
                .json(&serde_json::json!({"seat":seat,"job_id":id})),
        )
        .await;
        assert_eq!(rejected["ok"], false, "{case}: {rejected}");
        let after: (String, Option<i64>, i64, Option<i64>) = sqlx::query_as(
            "SELECT state, claimed_at, lease_expirations, acked_at FROM jobs WHERE id=?",
        )
        .bind(id as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, before, "{case} must not alter a claim");
        server.abort();
    }
}

#[tokio::test]
async fn redelivery_working_seat_renews_expired_claim_without_spending_lease_budget() {
    let (pool, services, registry, spine) = redelivery_fixture().await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let inbox = format!("http://{addr}/v1/inbox?seat=pij-reader");
    let first = redelivery_request(client.get(&inbox)).await;
    let id = first["data"][0]["job_id"].as_u64().unwrap();
    let mut seat = registry.get(&"pij-reader".into()).await.unwrap().unwrap();
    seat.state = pij_core::model::SystemState::Working;
    registry.put(seat).await.unwrap();
    let before = spine.tail(None, pij_core::model::Seq(0)).await.unwrap();
    for _ in 0..4 {
        sqlx::query("UPDATE jobs SET claimed_at=unixepoch()-61 WHERE id=?")
            .bind(id as i64)
            .execute(&pool)
            .await
            .unwrap();
        let page = redelivery_request(client.get(&inbox)).await;
        assert_eq!(
            page["data"],
            serde_json::json!([]),
            "a busy claim is not reclaimed: {page}"
        );
        let state: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT state, attempt, lease_expirations, unixepoch()-claimed_at FROM jobs WHERE id=?",
        )
        .bind(id as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((state.0.as_str(), state.1, state.2), ("running", 0, 0));
        assert!(state.3 <= 2, "working seat must renew the claim: {state:?}");
    }
    assert_eq!(
        spine.tail(None, pij_core::model::Seq(0)).await.unwrap(),
        before
    );
    server.abort();
}

#[tokio::test]
async fn redelivery_silent_extension_after_heartbeat_parks_after_three_leases() {
    for tombstoned_working in [false, true] {
        let (pool, services, registry, spine) = redelivery_fixture().await;
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        let client = reqwest::Client::new();
        let inbox = format!("http://{addr}/v1/inbox?seat=pij-reader");
        let first = redelivery_request(client.get(&inbox)).await;
        let id = first["data"][0]["job_id"].as_u64().unwrap();
        let heartbeat = redelivery_request(
            client
                .post(format!("http://{addr}/v1/inbox/heartbeat"))
                .json(&serde_json::json!({"seat":"pij-reader","job_id":id})),
        )
        .await;
        assert_eq!(heartbeat["ok"], true, "{heartbeat}");
        if tombstoned_working {
            let mut seat = registry.get(&"pij-reader".into()).await.unwrap().unwrap();
            seat.state = pij_core::model::SystemState::Working;
            registry.put(seat).await.unwrap();
            registry
                .tombstone(&"pij-reader".into(), "host exited during turn")
                .await
                .unwrap();
        }
        for expiration in 1..=3 {
            sqlx::query("UPDATE jobs SET claimed_at=unixepoch()-61 WHERE id=?")
                .bind(id as i64)
                .execute(&pool)
                .await
                .unwrap();
            let page = redelivery_request(client.get(&inbox)).await;
            assert_eq!(
                page["data"][0]["message"]["msg_id"],
                if expiration == 3 {
                    "successor"
                } else {
                    "lost-body"
                }
            );
            let state: (String, i64) =
                sqlx::query_as("SELECT state, lease_expirations FROM jobs WHERE id=?")
                    .bind(id as i64)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                state,
                (
                    if expiration == 3 { "failed" } else { "running" }.into(),
                    expiration
                )
            );
        }
        let parked: Vec<_> = spine
            .tail(None, pij_core::model::Seq(0))
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "delivery.parked")
            .collect();
        assert_eq!(parked.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(&parked[0].payload).unwrap();
        assert_eq!(payload["jobId"], id);
        assert_eq!(payload["outcome"], "undelivered:lease-exhausted");
        let delivered: i64 = sqlx::query_scalar("SELECT count(*) FROM delivered_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(delivered, 0);
        server.abort();
    }
}

#[tokio::test]
async fn redelivery_extension_three_expired_leases_park_and_unblock_serial_head() {
    let (pool, services, _, spine) = redelivery_fixture().await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/inbox?seat=pij-reader");
    let first = redelivery_request(client.get(&url)).await;
    assert_eq!(
        first["data"][0]["message"]["msg_id"], "lost-body",
        "{first}"
    );
    let id = first["data"][0]["job_id"].as_u64().unwrap();
    for attempt in 1..=3 {
        sqlx::query("UPDATE jobs SET claimed_at = unixepoch() - 61 WHERE id = ?")
            .bind(id as i64)
            .execute(&pool)
            .await
            .unwrap();
        let next = redelivery_request(client.get(&url)).await;
        assert_eq!(
            next["data"][0]["message"]["msg_id"],
            if attempt == 3 {
                "successor"
            } else {
                "lost-body"
            }
        );
        assert_eq!(
            next["data"][0]["attempt"],
            if attempt == 3 { 0 } else { attempt }
        );
    }
    let parked = redelivery_request(client.get(format!("{url}&peek=true"))).await;
    assert_eq!(parked["data"][0]["message"]["msg_id"], "successor");
    assert_eq!(parked["data"][1]["job_id"], id);
    assert_eq!(parked["data"][1]["state"], "failed");
    assert_eq!(parked["data"][1]["outcome"], "undelivered:lease-exhausted");
    let golden: serde_json::Value = serde_json::from_str(&pij_testkit::fixtures::read(
        "golden/api/inbox-recovery.json",
    ))
    .unwrap();
    assert_eq!(parked["data"][1]["outcome"], golden["outcomes"]["lease"]);
    let stale = redelivery_request(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&serde_json::json!({"seat":"pij-reader","job_id":id})),
    )
    .await;
    assert_eq!(stale["ok"], false);
    let parked_events: Vec<_> = spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "delivery.parked")
        .collect();
    assert_eq!(parked_events.len(), 1);
    assert_eq!(parked_events[0].seat, Some("pij-parent".into()));
    let payload: serde_json::Value = serde_json::from_str(&parked_events[0].payload).unwrap();
    assert_eq!(payload["recipient"], "pij-reader");
    assert_eq!(payload["messageId"], "lost-body");
    assert_eq!(payload, golden["lease_parked_event"]);
    server.abort();
}

#[tokio::test]
async fn redelivery_swallowed_ack_is_failure_not_reader_read_and_rejects_wrong_recipient() {
    let (pool, services, _, spine) = redelivery_fixture().await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let first =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    let id = first["data"][0]["job_id"].as_u64().unwrap();
    for seat in ["pij-stranger", "pij-reader"] {
        let result = redelivery_request(
            client
                .post(format!("http://{addr}/v1/inbox/ack"))
                .json(&serde_json::json!({"seat":seat,"job_id":id,
                "delivery_outcome":"undelivered:harness-swallowed"})),
        )
        .await;
        assert_eq!(result["ok"], seat == "pij-reader", "{result}");
    }
    let row: (String, String) = sqlx::query_as("SELECT state, outcome FROM jobs WHERE id = ?")
        .bind(id as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        row,
        ("failed".into(), "undelivered:harness-swallowed".into())
    );
    let delivered: i64 = sqlx::query_scalar("SELECT count(*) FROM delivered_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        delivered, 0,
        "a swallowed injection is not delivery evidence"
    );
    let events: Vec<_> = spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "delivery.parked")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seat, Some("pij-parent".into()));
    let next =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    assert_eq!(next["data"][0]["message"]["msg_id"], "successor");
    server.abort();
}

#[tokio::test]
async fn redelivery_operator_release_requires_parent_or_authoritative_prime_and_running_head() {
    let (pool, services, registry, _) = redelivery_fixture().await;
    let roles = services.roles.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let pending: i64 = sqlx::query_scalar("SELECT min(id) FROM jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    let release = |actor: &str, job: i64, evidence: &str| {
        client
            .post(format!("http://{addr}/v1/inbox/release"))
            .json(&serde_json::json!({
                "seat":"pij-reader", "job_id":job, "evidence":evidence,
                "caller":{"PIJ_SESSION_ID":actor}
            }))
    };
    let unclaimed =
        redelivery_request(release("pij-parent", pending, "observed silent injection")).await;
    assert_eq!(unclaimed["ok"], false, "pending is not a running head");
    let first =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    let id = first["data"][0]["job_id"].as_i64().unwrap();
    let denied = redelivery_request(release("pij-stranger", id, "observed silent injection")).await;
    assert_eq!(denied["ok"], false);
    assert_eq!(denied["details"]["code"], "E-RS-OWNERSHIP");
    let blank = redelivery_request(release("pij-parent", id, " ")).await;
    assert_eq!(blank["ok"], false);
    let parent = redelivery_request(release("pij-parent", id, "observed silent injection")).await;
    assert_eq!(parent["ok"], true, "{parent}");
    assert_eq!(parent["data"]["outcome"], "undelivered:operator-released");
    let repeated = redelivery_request(release("pij-parent", id, "stale operator")).await;
    assert_eq!(repeated["ok"], false);
    let second =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    let second_id = second["data"][0]["job_id"].as_i64().unwrap();
    // A stale descriptor role is not authority.
    let mut stranger = registry.get(&"pij-stranger".into()).await.unwrap().unwrap();
    stranger.role = Some("prime".into());
    registry.put(stranger).await.unwrap();
    let spoof =
        redelivery_request(release("pij-stranger", second_id, "descriptor role only")).await;
    assert_eq!(spoof["ok"], false);
    roles
        .assert_role(
            &"pij-stranger".into(),
            &"pij-stranger".into(),
            Some("prime".into()),
        )
        .await
        .unwrap();
    let prime = redelivery_request(release("pij-stranger", second_id, "authoritative prime")).await;
    assert_eq!(prime["ok"], true, "{prime}");
    server.abort();
}

#[tokio::test]
async fn redelivery_failure_ack_refuses_controls_native_recipients_and_unknown_outcomes() {
    for (harness, command, outcome) in [
        (
            Harness::Omp,
            Some("compact"),
            "undelivered:harness-swallowed",
        ),
        (Harness::Claude, None, "undelivered:harness-swallowed"),
        (Harness::Copilot, None, "undelivered:harness-swallowed"),
        (Harness::Omp, None, "undelivered:lease-exhausted"),
    ] {
        let (_, services, registry, _) = redelivery_fixture().await;
        let mut target = registry.get(&"pij-reader".into()).await.unwrap().unwrap();
        target.harness = harness;
        registry.put(target).await.unwrap();
        let queue = services.queue.clone();
        let (id, mut job) = queue
            .claim(&["delivery:pij-reader".into()], "test-reader")
            .await
            .unwrap()
            .unwrap();
        if let Some(command) = command {
            queue.ack(id, Outcome::Done).await.unwrap();
            let mut body: Msg = serde_json::from_str(&job.payload).unwrap();
            body.command = Some(command.into());
            job.payload = serde_json::to_string(&body).unwrap();
            job.serial_key = "pij-control".into();
            job.kind = "delivery:pij-control".into();
            body.to = "pij-control".into();
            job.payload = serde_json::to_string(&body).unwrap();
            let mut control = SeatDescriptor::new("pij-control", harness, "/abs/tree");
            control.parent = Some("pij-parent".into());
            registry.put(control).await.unwrap();
            queue.enqueue(job).await.unwrap();
        }
        let (seat, id) = if command.is_some() {
            let (id, _) = queue
                .claim(&["delivery:pij-control".into()], "test-reader")
                .await
                .unwrap()
                .unwrap();
            ("pij-control", id)
        } else {
            ("pij-reader", id)
        };
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        let result = redelivery_request(
            reqwest::Client::new()
                .post(format!("http://{addr}/v1/inbox/ack"))
                .json(&serde_json::json!({"seat":seat,"job_id":id,"delivery_outcome":outcome})),
        )
        .await;
        assert_eq!(result["ok"], false, "{harness:?}: {result}");
        assert!(
            queue.claimed_delivery(id).await.unwrap().is_some(),
            "a refused failure acknowledgement must leave its claim untouched"
        );
        if harness != Harness::Omp || command.is_some() {
            let release = redelivery_request(
                reqwest::Client::new()
                    .post(format!("http://{addr}/v1/inbox/release"))
                    .json(
                        &serde_json::json!({"seat":seat,"job_id":id,"evidence":"observed loss",
                    "caller":{"PIJ_SESSION_ID":"pij-parent"}}),
                    ),
            )
            .await;
            assert_eq!(
                release["ok"], false,
                "authorized release must reject {harness:?} controls/native: {release}"
            );
            assert!(queue.claimed_delivery(id).await.unwrap().is_some());
        }
        server.abort();
    }
}

#[tokio::test]
async fn redelivery_operator_release_racing_consumption_has_one_terminal_winner() {
    let (pool, services, _, spine) = redelivery_fixture().await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let claim =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    let id = claim["data"][0]["job_id"].as_u64().unwrap();
    let (release, ack) = tokio::join!(
        redelivery_request(client.post(format!("http://{addr}/v1/inbox/release")).json(
            &serde_json::json!({"seat":"pij-reader","job_id":id,"evidence":"observed loss",
                "caller":{"PIJ_SESSION_ID":"pij-parent"}})
        )),
        redelivery_request(
            client
                .post(format!("http://{addr}/v1/inbox/ack"))
                .json(&serde_json::json!({"seat":"pij-reader","job_id":id}))
        ),
    );
    assert_ne!(
        release["ok"], ack["ok"],
        "exactly one terminal transition: {release} {ack}"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = ?")
        .bind(id as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        state,
        if release["ok"] == true {
            "failed"
        } else {
            "done"
        }
    );
    let events = spine.tail(None, pij_core::model::Seq(0)).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "delivery.parked")
            .count(),
        usize::from(release["ok"] == true)
    );
    let next =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    assert_eq!(next["data"][0]["message"]["msg_id"], "successor");
    server.abort();
}

#[tokio::test]
async fn redelivery_parking_rolls_back_if_its_durable_event_cannot_commit() {
    let (pool, services, _, spine) = redelivery_fixture().await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let claim =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    let id = claim["data"][0]["job_id"].as_u64().unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_park BEFORE INSERT ON spine_events \
        WHEN NEW.kind = 'delivery.parked' BEGIN SELECT RAISE(ABORT, 'scripted spine failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let failure = || {
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&serde_json::json!({"seat":"pij-reader","job_id":id,
            "delivery_outcome":"undelivered:harness-swallowed"}))
    };
    let refused = redelivery_request(failure()).await;
    assert_eq!(refused["ok"], false);
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = ?")
        .bind(id as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        state, "running",
        "event failure cannot silently retire a body"
    );
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM spine_events \
        WHERE kind='delivery.outcome' AND json_extract(payload,'$.outcome.reason') \
            = 'undelivered:harness-swallowed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        receipts, 0,
        "the sender receipt rolls back with the parked event"
    );
    sqlx::query("DROP TRIGGER reject_park")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(redelivery_request(failure()).await["ok"], true);
    let rows = spine.tail(None, pij_core::model::Seq(0)).await.unwrap();
    assert_eq!(
        rows.iter()
            .filter(|event| event.kind == "delivery.parked")
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn redelivery_split_authority_refuses_recovery_without_changing_ordinary_reads() {
    let file = pij_testkit::FreshStore::new();
    let policy = pij_core::config::Config {
        store_path: file.path(),
        adapters: pij_core::config::Adapters {
            queue: pij_core::config::AdapterChoice::Real,
            ..Default::default()
        },
        ..Default::default()
    };
    let services = crate::build_services(&policy, std::path::Path::new("/unused-recovery-test"))
        .await
        .unwrap();
    let mut target = SeatDescriptor::new("pij-reader", Harness::Omp, "/abs/tree");
    target.parent = Some("pij-parent".into());
    services.registry.put(target).await.unwrap();
    services
        .registry
        .put(SeatDescriptor::new("pij-parent", Harness::Omp, "/abs/tree"))
        .await
        .unwrap();
    services
        .delivery
        .accept(Msg {
            from: "pij-parent".into(),
            from_machine: None,
            to: "pij-reader".into(),
            body: "durable body".into(),
            command: None,
            msg_id: "split-body".into(),
            in_reply_to: None,
        })
        .await
        .unwrap();
    let pool = pij_store::open(&file.path()).await.unwrap();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let claim =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    let id = claim["data"][0]["job_id"]
        .as_u64()
        .expect("ordinary claim still works");
    let failed = redelivery_request(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&serde_json::json!({"seat":"pij-reader","job_id":id,
            "delivery_outcome":"undelivered:harness-swallowed"})),
    )
    .await;
    assert_eq!(failed["details"]["code"], "E-RS-INBOX-AUTHORITY-SPLIT");
    let released = redelivery_request(client.post(format!("http://{addr}/v1/inbox/release")).json(
        &serde_json::json!({"seat":"pij-reader","job_id":id,"evidence":"observed loss",
            "caller":{"PIJ_SESSION_ID":"pij-parent"}}),
    ))
    .await;
    assert_eq!(released["details"]["code"], "E-RS-INBOX-AUTHORITY-SPLIT");
    sqlx::query("UPDATE jobs SET lease_expirations=2, claimed_at=unixepoch()-61 WHERE id=?")
        .bind(id as i64)
        .execute(&pool)
        .await
        .unwrap();
    let expired =
        redelivery_request(client.get(format!("http://{addr}/v1/inbox?seat=pij-reader"))).await;
    assert_eq!(expired["details"]["code"], "E-RS-INBOX-AUTHORITY-SPLIT");
    let state: (String, i64) =
        sqlx::query_as("SELECT state, lease_expirations FROM jobs WHERE id=?")
            .bind(id as i64)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        state,
        ("running".into(), 2),
        "refusal rolls back terminal recovery"
    );
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM spine_events WHERE kind='delivery.parked'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 0);
    let ack = redelivery_request(
        client
            .post(format!("http://{addr}/v1/inbox/ack"))
            .json(&serde_json::json!({"seat":"pij-reader","job_id":id})),
    )
    .await;
    assert_eq!(ack["ok"], true, "ordinary consumption remains supported");
    server.abort();
}

#[tokio::test]
async fn redelivery_release_reads_authority_after_prior_ordered_reparent_or_prime_unset() {
    for prime in [false, true] {
        let (pool, services, registry, spine) = redelivery_fixture().await;
        let target = SeatId::from("pij-reader");
        let actor = SeatId::from(if prime { "pij-stranger" } else { "pij-parent" });
        if prime {
            services
                .roles
                .assert_role(&actor, &actor, Some("prime".into()))
                .await
                .unwrap();
        }
        let id = services.delivery.claim_inbox(&target, false).await.unwrap()[0].job_id;
        let queue = services.queue.clone();
        let bus = services.event_bus.clone();
        let raw_spine = spine.clone();
        let changed_seat = target.clone();
        let changed_actor = actor.clone();
        let (entered, admitted) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let prior = tokio::spawn(async move {
            bus.publish_committed(async move {
                entered.send(()).unwrap();
                released.await.unwrap();
                let event = if prime {
                    pij_store::SqliteOrchestration::new(pool)
                        .assert_role_committed(&changed_actor, &changed_actor, None, 1)
                        .await?
                } else {
                    let mut changed = registry.get(&changed_seat).await?.unwrap();
                    changed.parent = None;
                    registry.put(changed.clone()).await?;
                    let mut event = Event {
                        seq: None,
                        v: 1,
                        at: 1,
                        kind: "seat.put".into(),
                        seat: Some(changed_seat),
                        payload: serde_json::to_string(&changed).unwrap(),
                    };
                    event.seq = Some(raw_spine.append(event.clone()).await?);
                    event
                };
                Ok((event, ()))
            })
            .await
        });
        admitted.await.unwrap();
        let (started, starting) = tokio::sync::oneshot::channel();
        let mut request = tokio::spawn(async move {
            started.send(()).unwrap();
            services
                .delivery
                .release_inbox_head(&target, id, "observed loss", actor, services.roles.clone())
                .await
        });
        starting.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut request)
                .await
                .is_err(),
            "release waits behind the already-admitted authority mutation"
        );
        release.send(()).unwrap();
        prior.await.unwrap().unwrap();
        let error = request
            .await
            .unwrap()
            .expect_err("former authority must be refused");
        assert!(
            matches!(error, PijError::GovernanceRefused { code, .. } if code == "E-RS-OWNERSHIP")
        );
        assert!(queue.claimed_delivery(id).await.unwrap().is_some());
        assert!(
            spine
                .tail(None, pij_core::model::Seq(0))
                .await
                .unwrap()
                .iter()
                .all(|event| event.kind != "delivery.parked")
        );
    }
}

pub(super) async fn spawn(router: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let joined = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    (addr, joined)
}

pub(super) fn config(local_key: &str, peer_keys: &[&str]) -> HttpConfig {
    HttpConfig {
        local_key: local_key.to_string(),
        peer_keys: peer_keys.iter().map(ToString::to_string).collect(),
        machine_alias: "workstation".to_string(),
    }
}

fn federation_policy() -> FederationPolicy {
    FederationPolicy {
        poll_interval: Duration::from_millis(10),
        max_retry_delay: Duration::from_millis(100),
        event_buffer_capacity: 16,
    }
}

fn expect_event(frame: StreamFrame) -> (String, u64, Event) {
    match frame {
        StreamFrame::Event {
            machine,
            cursor,
            event,
        } => (machine, cursor, event),
        StreamFrame::PeerState { .. } => panic!("expected event frame"),
    }
}

#[tokio::test]
async fn typing_release_wakes_an_already_waiting_inbox_reader() {
    let registry = Arc::new(FakeRegistry::new());
    let seat = SeatId::from("pij-waiting");
    registry
        .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
        .await
        .expect("seat");
    let services = test_services(
        registry,
        Arc::new(FakeQueue::new(1_024).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services
        .delivery
        .accept(Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: seat.clone(),
            body: "waiting".to_string(),
            msg_id: "waiting".to_string(),
            in_reply_to: None,
            command: None,
        })
        .await
        .expect("send");
    let claim = services
        .delivery
        .claim_inbox(&seat, false)
        .await
        .expect("claim")
        .remove(0);
    let state = AppState {
        registration: RegistrationService::new(
            Arc::clone(&services.registry),
            Arc::clone(&services.liveness),
            Arc::clone(&services.event_bus),
            Vec::new(),
            Arc::clone(&services.roles),
        )
        .with_native_lock(services.delivery.native_lock()),
        services,
        auth: AuthRing::new("key".to_string(), Vec::new()),
        machine_alias: "local".to_string(),
        spawn_lock: Arc::new(tokio::sync::Mutex::new(())),
        typing_lock: Arc::new(tokio::sync::Mutex::new(())),
        typing_grace_ms: DEFAULT_TYPING_GRACE_MS,
        federation: None,
    };
    let held = hold_inbox(
        State(state.clone()),
        Json(HoldRequest {
            job_id: claim.job_id,
            event: HeldEvent {
                seat: seat.clone(),
                msg_id: "waiting".to_string(),
                reason: "human-typing".to_string(),
                since_ms: 7,
            },
        }),
    )
    .await;
    assert_eq!(held.status(), StatusCode::OK);
    let waiting = inbox(
        State(state.clone()),
        Query(InboxQuery {
            seat: seat.clone(),
            wait: true,
            peek: false,
            native_session: None,
            pid: None,
            proc_start: None,
        }),
    );
    tokio::pin!(waiting);
    // Poll through the empty claim to establish subscription before releasing.
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
            waiting.as_mut(),
            cx
        )))
        .await
        .is_pending()
    );
    let released = release_inbox(
        State(state),
        Json(ReleaseRequest {
            job_id: claim.job_id,
            event: ReleasedEvent {
                seat,
                msg_id: "waiting".to_string(),
                at_ms: 8,
            },
        }),
    )
    .await;
    assert_eq!(released.status(), StatusCode::OK);
    let response = tokio::time::timeout(Duration::from_millis(250), waiting)
        .await
        .expect("release wakes the existing reader without another send");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let response: Envelope<Vec<crate::delivery::InboxClaim>> =
        serde_json::from_slice(&body).expect("inbox response");
    assert_eq!(response.data.expect("claims")[0].job_id, claim.job_id);
}

#[tokio::test]
async fn typing_release_declaration_clears_running_and_terminal_holds_after_restart() {
    for terminal in [false, true] {
        let registry = Arc::new(FakeRegistry::new());
        let seat = SeatId::from("pij-release-declaration");
        registry
            .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
            .await
            .expect("seat");
        let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
        let services = test_services(registry, queue.clone(), Arc::new(FakeSpine::new())).await;
        services
            .delivery
            .accept(Msg {
                from: "pij-sender".into(),
                from_machine: None,
                to: seat.clone(),
                body: "survives restart".to_string(),
                msg_id: "declaration".to_string(),
                in_reply_to: None,
                command: None,
            })
            .await
            .expect("send");
        let (addr, server) = spawn(router_with_config(services.clone(), config("key", &[]))).await;
        let client = reqwest::Client::new();
        let claim = typing_claim(&client, addr, &seat).await.remove(0);
        typing_post(&client, addr, "/v1/hold", &serde_json::json!({
            "seat": seat, "job_id": claim.job_id, "msg_id": "declaration", "reason": "human-typing", "since_ms": 7,
        })).await;
        server.abort();
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        queue.advance(Duration::from_millis(DEFAULT_TYPING_GRACE_MS));
        let claim = typing_claim(&client, addr, &seat).await.remove(0);
        if terminal {
            typing_post(
                &client,
                addr,
                "/v1/inbox/ack",
                &serde_json::json!({"seat": seat, "job_id": claim.job_id}),
            )
            .await;
        }
        let response = typing_post(
            &client,
            addr,
            "/v1/release",
            &serde_json::json!({
                "seat": seat, "job_id": claim.job_id, "msg_id": "declaration", "at_ms": 8,
            }),
        )
        .await;
        assert_eq!(response["data"]["released"], false);
        assert_eq!(
            response["data"]["reason"],
            if terminal { "terminal" } else { "not-deferred" }
        );
        let state = typing_post(&client, addr, "/v1/state", &serde_json::json!({"id": seat})).await;
        assert_eq!(
            state["data"]["held"],
            serde_json::json!([]),
            "queue no-op must not leave stale held evidence"
        );
        if !terminal {
            typing_post(
                &client,
                addr,
                "/v1/inbox/ack",
                &serde_json::json!({"seat": seat, "job_id": claim.job_id}),
            )
            .await;
        }
        assert_eq!(queue.acked(), [(claim.job_id, Outcome::Done)]);
        server.abort();
    }
}

async fn typing_post(
    client: &reqwest::Client,
    addr: SocketAddr,
    path: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let response = client
        .post(format!("http://{addr}{path}"))
        .bearer_auth("key")
        .json(body)
        .send()
        .await
        .expect("typing request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.json().await.expect("typing response")
}

async fn typing_claim(
    client: &reqwest::Client,
    addr: SocketAddr,
    seat: &SeatId,
) -> Vec<crate::delivery::InboxClaim> {
    let response: Envelope<Vec<crate::delivery::InboxClaim>> = client
        .get(format!("http://{addr}/v1/inbox"))
        .bearer_auth("key")
        .query(&[("seat", seat.as_str())])
        .send()
        .await
        .expect("inbox")
        .json()
        .await
        .expect("inbox envelope");
    response.data.expect("claims")
}

#[tokio::test]
async fn typing_not_live_names_absent_and_terminal_noops_on_both_routes() {
    let registry = Arc::new(FakeRegistry::new());
    let seat = SeatId::from("pij-noop");
    registry
        .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
        .await
        .expect("seat");
    let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(registry, queue.clone(), spine.clone()).await;
    services
        .delivery
        .accept(Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: seat.clone(),
            body: "already read".to_string(),
            msg_id: "terminal".to_string(),
            in_reply_to: None,
            command: None,
        })
        .await
        .expect("send");
    let claimed = services
        .delivery
        .claim_inbox(&seat, false)
        .await
        .expect("claim")
        .remove(0);
    services
        .delivery
        .acknowledge_inbox(
            &seat,
            claimed.job_id,
            &crate::delivery::NativeInboxIdentity {
                native_session: None,
                pid: None,
                proc_start: None,
            },
            None,
        )
        .await
        .expect("terminal");
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    for (job_id, reason) in [(JobId(999_999), "absent"), (claimed.job_id, "terminal")] {
        for (path, outcome) in [("/v1/hold", "held"), ("/v1/release", "released")] {
            let response = typing_post(&client, addr, path, &serde_json::json!({
                "seat": seat, "job_id": job_id, "msg_id": "terminal", "reason": "human-typing", "since_ms": 7, "at_ms": 8,
            })).await;
            assert_eq!(response["data"][outcome], false);
            assert_eq!(
                response["data"]["noop"], true,
                "{path} {reason}: {response}"
            );
            assert_eq!(response["data"]["reason"], reason);
        }
    }
    assert_eq!(queue.acked().len(), 1);
    let events = spine.tail(Some(&seat), Seq(0)).await.expect("events");
    assert!(events.iter().all(|event| event.kind != "delivery.held"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "delivery.released")
            .count(),
        2,
        "release declarations remain facts even when queue mutation is a no-op"
    );
    server.abort();
}

#[tokio::test]
async fn typing_unknown_claim_after_restart_is_harmless_and_keeps_pending_body() {
    let registry = Arc::new(FakeRegistry::new());
    let seat = SeatId::from("pij-restart");
    registry
        .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
        .await
        .expect("seat");
    let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(registry, queue.clone(), spine.clone()).await;
    services
        .delivery
        .accept(Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: seat.clone(),
            body: "pending body".to_string(),
            msg_id: "restart-body".to_string(),
            in_reply_to: None,
            command: None,
        })
        .await
        .expect("send");
    let claim = services
        .delivery
        .claim_inbox(&seat, false)
        .await
        .expect("old client claim")
        .remove(0);
    queue
        .defer(claim.job_id, Duration::ZERO)
        .await
        .expect("previous hold returned row");
    // An obsolete job id after restart cannot mutate a different pending body.
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    typing_post(&client, addr, "/v1/hold", &serde_json::json!({
        "seat": seat, "job_id": 999_999, "msg_id": "restart-body", "reason": "human-typing", "since_ms": 10,
    })).await;
    assert!(
        spine
            .tail(Some(&seat), Seq(0))
            .await
            .expect("events")
            .iter()
            .all(|event| event.kind != "delivery.held")
    );
    assert!(queue.acked().is_empty());
    let claims = typing_claim(&client, addr, &seat).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, claim.job_id);
    assert_eq!(claims[0].message.body, "pending body");
    server.abort();
}

#[tokio::test]
async fn typing_pending_holds_allow_bypass_rehold_and_restart_in_order() {
    let registry = Arc::new(FakeRegistry::new());
    let seat = SeatId::from("pij-typist");
    registry
        .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
        .await
        .expect("seat");
    let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(registry, queue.clone(), spine.clone()).await;
    for msg_id in ["first", "second"] {
        services
            .delivery
            .accept(Msg {
                from: "pij-sender".into(),
                from_machine: None,
                to: seat.clone(),
                body: msg_id.to_string(),
                msg_id: msg_id.to_string(),
                in_reply_to: None,
                command: None,
            })
            .await
            .expect("send");
    }
    let (addr, server) = spawn(router_with_config(services.clone(), config("key", &[]))).await;
    let client = reqwest::Client::new();
    let hold = |msg_id, job_id| serde_json::json!({"seat": seat, "job_id": job_id, "msg_id": msg_id, "reason": "human-typing", "since_ms": 10});
    let mut held_ids = BTreeMap::new();
    for msg_id in ["first", "second"] {
        let claims = typing_claim(&client, addr, &seat).await;
        assert_eq!(
            claims.len(),
            1,
            "a pending hold frees the serial key for the next message"
        );
        assert_eq!(claims[0].message.msg_id, msg_id);
        held_ids.insert(msg_id, claims[0].job_id);
        typing_post(&client, addr, "/v1/hold", &hold(msg_id, claims[0].job_id)).await;
    }
    assert_eq!(
        queue.live_len(),
        2,
        "holds are pending, not owned by dead clients"
    );
    assert!(queue.acked().is_empty());
    let grace = Duration::from_millis(
        typing_contract()["grace"]["default_ms"]
            .as_u64()
            .expect("grace"),
    );
    assert!(
        queue.retried().is_empty(),
        "typing holds are not failed attempts"
    );
    assert!(typing_claim(&client, addr, &seat).await.is_empty());
    services
        .delivery
        .accept(Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: seat.clone(),
            body: String::new(),
            msg_id: "bypass".to_string(),
            in_reply_to: None,
            command: Some("compact".to_string()),
        })
        .await
        .expect("control command");
    let bypass = typing_claim(&client, addr, &seat).await.remove(0);
    assert_eq!(bypass.message.msg_id, "bypass");
    typing_post(
        &client,
        addr,
        "/v1/inbox/ack",
        &serde_json::json!({"seat": seat, "job_id": bypass.job_id, "control_outcome": {"outcome":"executed"}}),
    )
    .await;
    queue.advance(grace - Duration::from_millis(1));
    assert!(
        typing_claim(&client, addr, &seat).await.is_empty(),
        "not reoffered before grace"
    );
    queue.advance(Duration::from_millis(1));
    for msg_id in ["first", "second"] {
        let claims = typing_claim(&client, addr, &seat).await;
        assert_eq!(claims[0].message.msg_id, msg_id);
        typing_post(&client, addr, "/v1/hold", &hold(msg_id, claims[0].job_id)).await;
    }
    assert!(
        held_ids.values().all(|job_id| queue.attempts(*job_id) == 0),
        "re-holds preserve attempt budgets"
    );
    assert_eq!(
        spine
            .tail(Some(&seat), Seq(0))
            .await
            .expect("events")
            .iter()
            .filter(|event| event.kind == "delivery.held")
            .count(),
        2,
        "re-holds never duplicate active events"
    );
    server.abort();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    for msg_id in ["first", "second"] {
        typing_post(&client, addr, "/v1/release", &serde_json::json!({"seat": seat, "job_id": held_ids[msg_id], "msg_id": msg_id, "at_ms": 20})).await;
        let claims = typing_claim(&client, addr, &seat).await;
        assert_eq!(
            claims.len(),
            1,
            "release clears the pending deferral, even after router restart"
        );
        assert_eq!(
            claims[0].message.msg_id, msg_id,
            "restart preserves queue order"
        );
        typing_post(
            &client,
            addr,
            "/v1/inbox/ack",
            &serde_json::json!({"seat": seat, "job_id": claims[0].job_id}),
        )
        .await;
    }
    assert_eq!(queue.live_len(), 0);
    assert_eq!(queue.acked().len(), 3);
    server.abort();
}

#[tokio::test]
async fn state_projects_live_delivery_deferrals_and_clears_success_or_terminal_rows() {
    for terminal in [false, true] {
        let registry = Arc::new(FakeRegistry::new());
        let seat = SeatId::from("pij-state-deferral");
        let pane = "%state-deferral";
        let mut descriptor = SeatDescriptor::new(seat.clone(), Harness::Claude, "/abs/tree");
        descriptor.pane = Some(pane.into());
        descriptor.proc = Some(ProcIdentity {
            pid: 7,
            proc_start: 11,
        });
        registry.put(descriptor).await.unwrap();
        let queue = Arc::new(FakeQueue::new(1_024).unwrap());
        let services =
            test_services(registry.clone(), queue.clone(), Arc::new(FakeSpine::new())).await;
        let tmux = Arc::new(FakeTmux::new().with_attached_tap(pane));
        tmux.arrange_pane(pane);
        let worker = crate::pointer::DrainWorker::new(
            registry,
            queue.clone(),
            Arc::new(pij_testkit::fakes::FakeTransport::unreachable()),
            tmux.clone(),
            Arc::new(pij_harnesses::InteractionGate::new(tmux.clone())),
            services.event_bus.clone(),
            crate::pointer::PointerPolicy {
                cadence: Duration::from_secs(90),
                announcement_limit: 3,
            },
        )
        .unwrap();
        let message = Msg {
            from: "sender".into(),
            to: seat.clone(),
            msg_id: "deferred-body".into(),
            body: "preserved body".into(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        };
        let job_id = queue
            .enqueue(Job {
                kind: pij_core::delivery::delivery_kind(&seat),
                serial_key: seat.to_string(),
                payload: serde_json::to_string(&message).unwrap(),
                dedupe_key: message.msg_id.clone(),
                attempt: 0,
            })
            .await
            .unwrap();
        let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
        let client = reqwest::Client::new();
        for _ in 0..3 {
            assert_eq!(worker.drain_once().await.unwrap(), 1);
        }
        let response =
            typing_post(&client, addr, "/v1/state", &serde_json::json!({"id": seat})).await;
        let deferred = &response["data"]["deliveryDeferrals"][0];
        assert_eq!(deferred["job_id"], job_id.0);
        assert_eq!(deferred["msg_id"], message.msg_id);
        assert_eq!(deferred["reason"], "unrecognized");
        assert_eq!(deferred["count"], 3);
        assert!(deferred["since_ms"].as_u64().is_some_and(|at| at > 0));

        if terminal {
            queue
                .claim(&[pij_core::delivery::delivery_kind(&seat)], "terminal")
                .await
                .unwrap()
                .unwrap();
            queue
                .ack(
                    job_id,
                    Outcome::Failed {
                        reason: "recipient closed".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            tmux.arrange_clear_composer(pane);
            assert_eq!(worker.drain_once().await.unwrap(), 1);
        }
        let response =
            typing_post(&client, addr, "/v1/state", &serde_json::json!({"id": seat})).await;
        assert_eq!(response["data"]["deliveryDeferrals"], serde_json::json!([]));
        assert_eq!(response["data"]["held"], serde_json::json!([]));
        assert!(
            queue
                .peek(&[pij_core::delivery::delivery_kind(&seat)])
                .await
                .unwrap()
                .is_none()
        );
        server.abort();
    }
}

#[tokio::test]
async fn typing_state_projects_active_holds_only() {
    let registry = Arc::new(FakeRegistry::new());
    let seat = SeatId::from("pij-state");
    registry
        .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
        .await
        .expect("seat");
    let services = test_services(
        registry,
        Arc::new(FakeQueue::new(1_024).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    for (kind, msg_id) in [
        ("delivery.held", "released"),
        ("delivery.released", "released"),
        ("delivery.held", "active"),
    ] {
        let payload = if kind == "delivery.held" {
            serde_json::json!({"seat": seat, "msg_id": msg_id, "reason": "human-typing", "since_ms": 7})
        } else {
            serde_json::json!({"seat": seat, "msg_id": msg_id, "at_ms": 8})
        };
        services
            .event_bus
            .publish(Event {
                seq: None,
                v: 1,
                at: 10,
                kind: kind.to_string(),
                seat: Some(seat.clone()),
                payload: payload.to_string(),
            })
            .await
            .expect("event");
    }
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = typing_post(
        &reqwest::Client::new(),
        addr,
        "/v1/state",
        &serde_json::json!({"id": seat}),
    )
    .await;
    assert_eq!(
        response["data"]["held"],
        serde_json::json!([{"seat": seat, "msg_id": "active", "reason": "human-typing", "since_ms": 7}])
    );
    server.abort();
}

#[tokio::test]
async fn typing_hold_projections_follow_terminal_outcomes_and_sequence() {
    use pij_core::model::DeliveryOutcome;

    for terminal in [
        DeliveryOutcome::Refused {
            reason: "operator denied".to_string(),
        },
        DeliveryOutcome::Delivered {
            origin: DeliveryOrigin::VerifiedArrival,
        },
    ] {
        let services = test_services(
            Arc::new(FakeRegistry::new()),
            Arc::new(FakeQueue::new(1_024).expect("queue")),
            Arc::new(FakeSpine::new()),
        )
        .await;
        let state = AppState {
            registration: RegistrationService::new(
                Arc::clone(&services.registry),
                Arc::clone(&services.liveness),
                Arc::clone(&services.event_bus),
                Vec::new(),
                Arc::clone(&services.roles),
            )
            .with_native_lock(services.delivery.native_lock()),
            services,
            auth: AuthRing::new("key".to_string(), Vec::new()),
            machine_alias: "local".to_string(),
            spawn_lock: Arc::new(tokio::sync::Mutex::new(())),
            typing_lock: Arc::new(tokio::sync::Mutex::new(())),
            typing_grace_ms: DEFAULT_TYPING_GRACE_MS,
            federation: None,
        };
        let seat = SeatId::from("pij-held");
        let other_seat = SeatId::from("pij-other");
        let held = serde_json::json!({
            "seat": seat, "msg_id": "held", "reason": "human-typing", "since_ms": 7,
        });
        let outcome = |msg_id: &str, outcome: &DeliveryOutcome| serde_json::json!({"msg_id": msg_id, "outcome": outcome, "transport": "claude-uds"});
        let queued = DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        };
        let pending = DeliveryOutcome::Held {
            reason: "awaiting approval".to_string(),
        };
        // Constant timestamps deliberately make durable sequence the only ordering signal.
        for (event_seat, kind, payload, expected) in [
            (&seat, "delivery.held", held.clone(), true),
            (
                &other_seat,
                "delivery.outcome",
                outcome("held", &terminal),
                true,
            ),
            (
                &seat,
                "delivery.outcome",
                outcome("held-other", &terminal),
                true,
            ),
            (&seat, "delivery.outcome", outcome("held", &queued), true),
            (&seat, "delivery.outcome", outcome("held", &pending), true),
            (&seat, "delivery.outcome", outcome("held", &terminal), false),
            (&seat, "delivery.outcome", outcome("held", &queued), false),
            (&seat, "delivery.outcome", outcome("held", &pending), false),
            (&seat, "delivery.held", held.clone(), true),
            (
                &seat,
                "delivery.released",
                serde_json::json!({"seat": seat, "msg_id": "held", "at_ms": 8}),
                false,
            ),
            (&seat, "delivery.held", held, true),
            (&seat, "delivery.outcome", outcome("held", &terminal), false),
        ] {
            state
                .services
                .event_bus
                .publish(Event {
                    seq: None,
                    v: wire::EVENT_VERSION,
                    at: 10,
                    kind: kind.to_string(),
                    seat: Some(event_seat.clone()),
                    payload: payload.to_string(),
                })
                .await
                .expect("event");
            let actual = is_held(&state, &seat, "held").await.expect("held lookup");
            let holds = active_holds(&state, &seat).await.expect("held projection");
            assert_eq!(actual, expected, "{terminal:?}: {kind} {payload}");
            assert_eq!(
                holds.len(),
                usize::from(expected),
                "{terminal:?}: {kind} {payload}"
            );
            if expected {
                assert_eq!(holds[0].msg_id, "held");
                assert_eq!(holds[0].seat, seat);
            }
        }
    }
}

fn typing_contract() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../testkit/fixtures/delivery/hold-events.json"
    ))
    .expect("frozen typing contract")
}

#[tokio::test]
async fn typing_registration_flattens_grace_without_persisting_it_on_the_seat() {
    // Separate processes exercise the real environment read without racing other tests.
    let expected = match std::env::var("PIJ_TEST_GRACE_EXPECTED") {
        Ok(value) => value.parse::<u64>().expect("expected grace"),
        Err(_) => {
            let default = typing_contract()["grace"]["default_ms"]
                .as_u64()
                .expect("default");
            for (value, expected) in [
                (None, default),
                (Some("4321"), 4_321),
                (Some("invalid"), default),
            ] {
                let mut child =
                    std::process::Command::new(std::env::current_exe().expect("test binary"));
                child.args(["--exact", "http::tests::typing_registration_flattens_grace_without_persisting_it_on_the_seat", "--nocapture"])
                    .env("PIJ_TEST_GRACE_EXPECTED", expected.to_string())
                    .env_remove("PIJ_TYPING_GRACE_MS");
                if let Some(value) = value {
                    child.env("PIJ_TYPING_GRACE_MS", value);
                }
                let output = child.output().expect("isolated registration witness");
                assert!(
                    output.status.success(),
                    "grace {value:?}: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        }
    };
    let identity = ProcIdentity {
        pid: 43,
        proc_start: 20260902152500,
    };
    let registry = Arc::new(FakeRegistry::new());
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response: serde_json::Value = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "id": "pij-grace", "harness": "omp", "folder": "/abs/tree",
            "pid": identity.pid, "proc_start": identity.proc_start
        }))
        .send()
        .await
        .expect("registration")
        .json()
        .await
        .expect("envelope");
    assert_eq!(response["data"]["typing_grace_ms"], expected);
    assert_eq!(response["data"]["id"], "pij-grace");
    assert!(response["data"].get("descriptor").is_none());
    let seat = registry
        .get(&SeatId::from("pij-grace"))
        .await
        .expect("read seat")
        .expect("registered");
    assert!(
        serde_json::to_value(seat)
            .expect("seat json")
            .get("typing_grace_ms")
            .is_none()
    );
    server.abort();
}

#[test]
fn typing_hold_and_release_are_known_wire_events() {
    let contract = typing_contract();
    for key in ["held", "released"] {
        let event = Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at: contract[key]["at"].as_u64().expect("timestamp"),
            kind: contract[key]["kind"].as_str().expect("kind").to_string(),
            seat: Some(contract[key]["seat"].as_str().expect("seat").into()),
            payload: serde_json::to_string(&contract[key]["payload"]).expect("payload"),
        };
        let line = wire::encode_event(&event).expect("encode event");
        assert!(matches!(
            wire::decode_event_line(1, &line).expect("decode event"),
            WireEvent::Known(_)
        ));
    }
}

#[tokio::test]
async fn typing_hold_release_match_contract_and_are_idempotent() {
    let contract = typing_contract();
    let registry = Arc::new(FakeRegistry::new());
    let seat: SeatId = contract["held"]["seat"].as_str().expect("seat").into();
    registry
        .put(SeatDescriptor::new(seat.clone(), Harness::Omp, "/abs/tree"))
        .await
        .expect("recipient");
    let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(registry, queue.clone(), spine.clone()).await;
    services
        .delivery
        .accept(Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: seat.clone(),
            body: "held body".to_string(),
            msg_id: contract["held"]["payload"]["msg_id"]
                .as_str()
                .expect("id")
                .to_string(),
            in_reply_to: None,
            command: None,
        })
        .await
        .expect("send");
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let claims: Envelope<Vec<crate::delivery::InboxClaim>> = client
        .get(format!("http://{addr}/v1/inbox"))
        .bearer_auth("key")
        .query(&[("seat", seat.as_str())])
        .send()
        .await
        .expect("claim")
        .json()
        .await
        .expect("claim envelope");
    let claims = claims.data.expect("claims");
    assert_eq!(claims.len(), 1);
    for (key, repeats) in [("hold_request", 2), ("release_request", 1)] {
        let mut request = contract[key]["body"].clone();
        request["job_id"] = serde_json::json!(claims[0].job_id);
        for _ in 0..repeats {
            let response = client
                .post(format!(
                    "http://{addr}{}",
                    contract[key]["path"].as_str().expect("path")
                ))
                .bearer_auth("key")
                .json(&request)
                .send()
                .await
                .expect("request");
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            let body: serde_json::Value = response.json().await.expect("envelope");
            assert_eq!(body, contract[key]["response"]);
        }
    }
    let events = spine.tail(Some(&seat), Seq(0)).await.expect("events");
    for key in ["held", "released"] {
        let rows: Vec<_> = events
            .iter()
            .filter(|event| event.kind == contract[key]["kind"].as_str().expect("kind"))
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seat.as_ref(), Some(&seat));
        // Compare the actual spine payload bytes, not a re-serialized producer type.
        assert_eq!(
            rows[0].payload.as_bytes(),
            serde_json::to_vec(&contract[key]["payload"]).expect("golden payload")
        );
    }
    assert!(
        queue.acked().is_empty(),
        "hold and release never acknowledge"
    );
    let next = typing_claim(&client, addr, &seat).await;
    assert_eq!(next.len(), 1);
    let mut release = contract["release_request"]["body"].clone();
    release["job_id"] = serde_json::json!(next[0].job_id);
    let repeat: serde_json::Value = client
        .post(format!("http://{addr}/v1/release"))
        .bearer_auth("key")
        .json(&release)
        .send()
        .await
        .expect("optimistic release")
        .json()
        .await
        .expect("release envelope");
    assert_eq!(repeat["data"]["released"], false);
    typing_post(
        &client,
        addr,
        "/v1/inbox/ack",
        &serde_json::json!({
            "seat": seat, "job_id": next[0].job_id,
        }),
    )
    .await;
    assert_eq!(
        queue.acked(),
        [(next[0].job_id, Outcome::Done)],
        "duplicate release preserves current claim ownership"
    );
    for path in ["/v1/hold", "/v1/release"] {
        let denied = client
            .post(format!("http://{addr}{path}"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("unauthorized");
        assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    }
    server.abort();
}

#[tokio::test]
async fn typing_receipt_is_queued_until_ack_then_reader_read_for_omp_and_pi() {
    let contract = typing_contract();
    for harness in [Harness::Omp, Harness::Pi] {
        let registry = Arc::new(FakeRegistry::new());
        let mut seat = SeatDescriptor::new("pij-reader", harness, "/abs/tree");
        seat.proc = Some(ProcIdentity {
            pid: 42,
            proc_start: 7,
        });
        seat.pane = Some("%42".to_string());
        registry.put(seat).await.expect("bound recipient");
        let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
        let services = test_services(registry, queue.clone(), Arc::new(FakeSpine::new())).await;
        let message = Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: "pij-reader".into(),
            body: "ordinary push".to_string(),
            msg_id: "receipt-guard".to_string(),
            in_reply_to: None,
            command: None,
        };
        let before = services
            .delivery
            .accept(message.clone())
            .await
            .expect("push");
        assert_eq!(
            serde_json::to_value(before.outcome).expect("receipt"),
            contract["receipt_note"]["current_outcome_before_ack"]
        );
        let claims = services
            .delivery
            .claim_inbox(&message.to, false)
            .await
            .expect("claim");
        assert_eq!(claims.len(), 1);
        assert!(queue.acked().is_empty());
        let (addr, server) = spawn(router_with_config(services.clone(), config("key", &[]))).await;
        let response = reqwest::Client::new()
            .post(format!("http://{addr}/v1/inbox/ack"))
            .bearer_auth("key")
            .json(&InboxAckRequest {
                delivery_outcome: None,
                seat: message.to.clone(),
                job_id: claims[0].job_id,
                native: crate::delivery::NativeInboxIdentity {
                    native_session: None,
                    pid: None,
                    proc_start: None,
                },
                control_outcome: None,
            })
            .send()
            .await
            .expect("ack");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let after = services
            .delivery
            .accept(message)
            .await
            .expect("same-message receipt");
        assert_eq!(
            serde_json::to_value(after.outcome).expect("receipt"),
            contract["receipt_note"]["current_outcome_after_ack"]
        );
        assert_eq!(queue.acked(), [(claims[0].job_id, Outcome::Done)]);
        server.abort();
    }
}

#[test]
fn exposure_banner_names_the_address_it_classifies() {
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7461);
    assert_eq!(exposure(&loopback), Exposure::Loopback);
    assert_eq!(
        boot_banner(&loopback),
        "pij daemon listening on 127.0.0.1:7461 (loopback; local bearer key required)"
    );

    let lan = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7461);
    assert_eq!(exposure(&lan), Exposure::Lan);
    assert_eq!(
        boot_banner(&lan),
        "WARNING: pij daemon listening on 0.0.0.0:7461 beyond loopback; bearer keys are the only LAN access control"
    );
}

#[test]
fn send_request_uses_resolved_destination_object_and_optional_reply_id() {
    let request = SendRequest {
        fyi: false,
        force: false,
        reason: None,
        from: "pij-a".into(),
        from_machine: Some("desktop".to_string()),
        to: Destination::local("pij-b"),
        body: "answer".to_string(),
        msg_id: "m-2".to_string(),
        in_reply_to: Some("m-1".to_string()),
    };
    let json = serde_json::to_value(&request).expect("send request json");
    assert_eq!(json["to"]["seat"], "pij-b");
    assert!(
        json["to"].is_object(),
        "the daemon never parses human address strings"
    );
    assert_eq!(json["in_reply_to"], "m-1");
    assert_eq!(json["from_machine"], "desktop");

    let without_reply: SendRequest = serde_json::from_value(serde_json::json!({
        "from": "pij-a",
        "to": { "seat": "pij-b" },
        "body": "hello",
        "msg_id": "m-3"
    }))
    .expect("in_reply_to is optional");
    assert_eq!(without_reply.to, Destination::local("pij-b"));
    assert_eq!(without_reply.in_reply_to, None);
}

#[tokio::test]
async fn auth_ring_covers_every_declared_route_and_accepts_peer_keys() {
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(
        services,
        config("local-secret", &["peer-secret"]),
    ))
    .await;
    let client = reqwest::Client::new();

    for endpoint in Endpoint::ALL {
        let response = client
            .request(
                endpoint.method(),
                format!("http://{addr}{}", endpoint.path()),
            )
            .send()
            .await
            .expect("unauthenticated request");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "{} must inherit whole-router auth",
            endpoint.path()
        );
        let body = response.text().await.expect("auth body");
        assert!(
            body.contains("manually bootstrapped configured key"),
            "refusal names the repair: {body}"
        );
    }

    let health = client
        .get(format!("http://{addr}/health"))
        .bearer_auth("peer-secret")
        .send()
        .await
        .expect("peer-authenticated health");
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let body: Envelope<serde_json::Value> = health.json().await.expect("health envelope");
    assert_eq!(body.data.expect("data")["machine"], "workstation");

    server.abort();
}

#[tokio::test]
async fn extension_identity_survives_sqlite_registration_readbacks_and_legacy_refresh_clears_it() {
    let pool = pij_store::open("").await.expect("isolated SQLite");
    let bus = Arc::new(
        crate::events::EventBus::new(Arc::new(pij_store::SqliteSpine::new(pool.clone())), 16)
            .expect("shared SQL event bus"),
    );
    let registry = Arc::new(pij_store::SqliteRegistry::new(pool.clone(), bus.clone()));
    let mut services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("queue")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.registry = registry.clone();
    services.spine = bus.clone();
    services.event_bus = bus.clone();
    services.roles = Arc::new(super::role::RoleService::new(
        registry.clone(),
        pij_store::SqliteOrchestration::new(pool.clone()),
        bus.clone(),
    ));
    services.governance = Arc::new(super::governance::GovernanceService::new(
        pij_store::SqliteOrchestration::new(pool.clone()),
        bus,
        pij_testkit::fresh_dir("pij-extension-identity"),
    ));
    services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
        pid: 144,
        proc_start: 20260909090000,
    }));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    // The same process may refresh from an older extension. Absence must clear
    // its prior attestation, not accidentally inherit a build from the registry.
    for identity in [
        Some(("0123456789+dirty", "/work/144/.pi/extensions/pij")),
        Some(("hash:abcdef123456", "/installed/pij")),
        None,
    ] {
        let mut claim = serde_json::json!({
            "id": "pij-extension-identity",
            "harness": "omp",
            "folder": "/abs/tree",
            "pane": "%144",
            "pid": 144,
            "proc_start": 20260909090000_u64,
        });
        if let Some((build, path)) = identity {
            claim["extension_build"] = serde_json::json!(build);
            claim["extension_path"] = serde_json::json!(path);
        }
        let expected_build = serde_json::json!(identity.map(|(build, _)| build));
        let expected_path = serde_json::json!(identity.map(|(_, path)| path));
        let assert_identity = |seat: &serde_json::Value| {
            assert_eq!(seat.get("extension_build"), Some(&expected_build));
            assert_eq!(seat.get("extension_path"), Some(&expected_path));
        };

        let response = client
            .post(format!("http://{addr}/v1/register"))
            .bearer_auth("key")
            .json(&claim)
            .send()
            .await
            .expect("register");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let registered: Envelope<serde_json::Value> = response.json().await.expect("registration");
        assert_identity(&registered.data.expect("registered seat"));

        let stored = registry
            .get(&SeatId::from("pij-extension-identity"))
            .await
            .expect("SQLite read")
            .expect("persisted seat");
        assert_identity(&serde_json::to_value(stored).expect("stored descriptor"));

        let response = client
            .post(format!("http://{addr}/v1/whoami"))
            .bearer_auth("key")
            .json(&serde_json::json!({ "seat": "pij-extension-identity" }))
            .send()
            .await
            .expect("whoami");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let whoami: Envelope<serde_json::Value> = response.json().await.expect("whoami envelope");
        assert_identity(&whoami.data.expect("whoami seat"));

        let response = client
            .get(format!("http://{addr}/v1/seats"))
            .bearer_auth("key")
            .send()
            .await
            .expect("list");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let listed: Envelope<serde_json::Value> = response.json().await.expect("list envelope");
        let roster = listed.data.expect("listed roster");
        let seats = roster["seats"].as_array().expect("listed seats");
        assert_identity(
            seats
                .iter()
                .find(|seat| seat["id"] == "pij-extension-identity")
                .expect("registered seat in list"),
        );

        let response = client
            .post(format!("http://{addr}/v1/state"))
            .bearer_auth("key")
            .json(&serde_json::json!({ "argv": ["state", "pij-extension-identity"] }))
            .send()
            .await
            .expect("state");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let state: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
        assert_identity(&state.data.expect("state data"));
    }
    server.abort();
    pool.close().await;
}

async fn retired_registration_refusal_contract(harness: Harness, proof: Option<&str>) {
    let old_proc = ProcIdentity {
        pid: 154,
        proc_start: 1,
    };
    let new_proc = ProcIdentity {
        pid: 155,
        proc_start: 2,
    };
    let mut old = SeatDescriptor::new("pij-retired-154", Harness::Omp, "/abs/tree");
    old.proc = Some(old_proc);
    old.harness_session = Some("saved-omp-session".into());
    old.tombstoned_at = Some(154);
    old.tombstone_reason = Some("observed-dead".into());
    old.parent = Some("pij-original-parent".into());
    if proof == Some("process") {
        old.proc = Some(new_proc);
    }
    if proof == Some("spawn") {
        old.spawn_id = Some("launch-154".into());
    }
    let registry = Arc::new(FakeRegistry::new().with_seat(old.clone()));
    let spine = Arc::new(FakeSpine::new());
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).unwrap()),
        spine.clone(),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(new_proc));
    let before = spine
        .tail(Some(&old.id), pij_core::model::Seq(0))
        .await
        .unwrap();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "id": old.id, "harness": harness.as_str(), "folder": "/abs/other-tree",
            "pid": new_proc.pid, "proc_start": new_proc.proc_start,
            "HARNESS_SESSION_ID": "foreign-omp-session", "parent": "pij-new-parent",
            "spawn_id": if proof == Some("spawn") { old.spawn_id.as_deref() } else { None },
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let envelope: serde_json::Value = response.json().await.unwrap();
    server.abort();
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{envelope}");
    assert_eq!(envelope["v"], 2);
    assert_eq!(envelope["command"], "pij register");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"], "refused");
    let mut expected =
        "seat pij-retired-154 is retired (observed-dead, 154); a different session may not take it"
            .to_string();
    if harness != old.harness {
        expected.push_str(&format!(
            "; harness mismatch: row omp, claim {}",
            harness.as_str()
        ));
    }
    assert_eq!(envelope["meta"], expected);
    assert_eq!(registry.get(&old.id).await.unwrap(), Some(old.clone()));
    assert_eq!(
        spine
            .tail(Some(&old.id), pij_core::model::Seq(0))
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn retired_registration_refuses_foreign_session_without_writing() {
    retired_registration_refusal_contract(Harness::Omp, None).await;
}

#[tokio::test]
async fn retired_registration_refuses_pi_process_continuity_on_omp_row() {
    retired_registration_refusal_contract(Harness::Pi, Some("process")).await;
}

#[tokio::test]
async fn retired_registration_refuses_claude_spawn_continuity_on_omp_row() {
    retired_registration_refusal_contract(Harness::Claude, Some("spawn")).await;
}

#[tokio::test]
async fn registration_corroborates_process_and_refuses_shared_process_aliases() {
    let identity = ProcIdentity {
        pid: 42,
        proc_start: 20260829103052,
    };
    let registry = Arc::new(FakeRegistry::new());
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let registration = Registration {
        supersedes: None,
        id: "pij-parent".to_string(),
        harness: "omp".to_string(),
        folder: "/abs/tree".to_string(),
        extension_build: None,
        extension_path: None,
        pane: Some("%42".to_string()),
        pid: Some(identity.pid),
        proc_start: Some(identity.proc_start),
        spawn_id: Some("spawn-1".to_string()),
        model: Some("provider/model".to_string()),
        actual_model: None,
        actual_model_observed: false,
        provider: Some("provider".to_string()),
        effort: Some("high".to_string()),
        parent: None,
        role: None,
        relay: false,
    };

    let mut registration_with_session =
        serde_json::to_value(&registration).expect("registration json");
    registration_with_session["harnessSession"] = serde_json::json!("omp-native-42");
    let response = client
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&registration_with_session)
        .send()
        .await
        .expect("register");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<SeatDescriptor> = response.json().await.expect("descriptor envelope");
    let accepted = accepted.data.expect("descriptor");
    assert_eq!(accepted.proc, Some(identity));
    assert_eq!(accepted.harness_session.as_deref(), Some("omp-native-42"));
    assert_eq!(registry.calls(), ["list", "put:pij-parent"]);

    let self_declaration = Registration {
        spawn_id: Some("self-declared-spawn".to_string()),
        model: Some("self-declared/model".to_string()),
        actual_model: None,
        actual_model_observed: false,
        provider: Some("self-declared".to_string()),
        effort: Some("low".to_string()),
        ..registration.clone()
    };
    let response = client
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&self_declaration)
        .send()
        .await
        .expect("repeat registration");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<SeatDescriptor> = response.json().await.expect("descriptor envelope");
    let accepted = accepted.data.expect("descriptor");
    assert_eq!(accepted.spawn_id.as_deref(), Some("spawn-1"));
    assert_eq!(accepted.model.as_deref(), Some("provider/model"));
    assert_eq!(accepted.provider.as_deref(), Some("provider"));
    assert_eq!(accepted.effort.as_deref(), Some("high"));
    assert_eq!(accepted.harness_session.as_deref(), Some("omp-native-42"));
    assert_eq!(
        registry.calls(),
        ["list", "put:pij-parent", "list", "put:pij-parent"]
    );

    let alias = Registration {
        id: "pij-child-alias".to_string(),
        ..registration
    };
    let response = client
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&alias)
        .send()
        .await
        .expect("alias registration");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let refusal = response.text().await.expect("refusal");
    assert!(
        refusal.contains("subagent of pij-parent"),
        "collision names the existing seat: {refusal}"
    );
    assert_eq!(
        registry.calls(),
        ["list", "put:pij-parent", "list", "put:pij-parent", "list",],
        "a refused alias is never persisted"
    );

    server.abort();
}

#[tokio::test]
async fn paneless_registration_returns_the_identity_export_it_requires() {
    let identity = ProcIdentity {
        pid: 43,
        proc_start: 20260902152500,
    };
    let registry = Arc::new(FakeRegistry::new());
    let mut services = test_services(
        registry,
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&Registration {
            supersedes: None,
            id: "pij-paneless".to_string(),
            harness: "omp".to_string(),
            folder: "/abs/tree".to_string(),
            extension_build: None,
            extension_path: None,
            pane: None,
            pid: Some(identity.pid),
            proc_start: Some(identity.proc_start),
            spawn_id: None,
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        })
        .send()
        .await
        .expect("register paneless seat");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let registered: Envelope<SeatDescriptor> = response.json().await.expect("register envelope");
    assert_eq!(registered.data.expect("descriptor").pane, None);
    assert_eq!(
        registered.meta.as_deref(),
        Some(
            "verified external shim callers resolve this paneless seat from their native host/session automatically; other clients must pass id pij-paneless explicitly or export PIJ_SESSION_ID=pij-paneless"
        )
    );

    server.abort();
}

#[test]
fn registration_binding_absent_preserves_descriptor_shape() {
    let descriptor = SeatDescriptor::new("pij-old-response", Harness::Omp, "/tmp/binding");
    let old_wire = serde_json::to_value(descriptor).expect("old descriptor response");
    let response: RegistrationResponse =
        serde_json::from_value(old_wire.clone()).expect("binding defaults absent");
    assert!(response.binding.is_none());
    assert_eq!(
        serde_json::to_value(response).expect("serialize response"),
        old_wire
    );
}

async fn assert_registration_binding_fixture(expected_key: &str, previous_key: Option<&str>) {
    // Resolved through the testkit corpus helper, NOT a relative path string:
    // this fixture already moved once (out of the plan folder, which post-flight
    // archives), and the path-string version of this line silently pointed at a
    // deleted file while the working tree still had the fix. A helper makes the
    // next move a COMPILE error instead of a NotFound panic at test time.
    let fixture: serde_json::Value =
        serde_json::from_str(&pij_testkit::fixtures::read("register-response.json"))
            .expect("parse shared register response fixture");
    let expected = &fixture[expected_key];
    let descriptor: SeatDescriptor =
        serde_json::from_value(expected.clone()).expect("fixture descriptor");
    let identity = descriptor.proc.expect("fixture process identity");
    let registry = Arc::new(FakeRegistry::new());
    if let Some(previous_key) = previous_key {
        registry
            .put(
                serde_json::from_value(fixture[previous_key].clone()).expect("previous descriptor"),
            )
            .await
            .expect("seed previous binding");
    }
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "id": descriptor.id,
            "harness": descriptor.harness,
            "folder": descriptor.folder,
            "pane": descriptor.pane,
            "pid": identity.pid,
            "proc_start": identity.proc_start,
            "model": descriptor.model,
            "provider": descriptor.provider,
            "effort": descriptor.effort,
        }))
        .send()
        .await
        .expect("register fixture claim");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<serde_json::Value> = response.json().await.expect("register envelope");
    server.abort();
    // Captured135 fields stay exact; later plans add documented wire fields.
    let observed = accepted.data.as_ref().expect("register response body");
    let expected_object = expected.as_object().expect("fixture object");
    let observed_object = observed.as_object().expect("response object");
    for (key, value) in expected_object {
        assert_eq!(
            observed_object.get(key),
            Some(value),
            "{expected_key} fixture: key `{key}`"
        );
    }
    let unexpected: Vec<&String> = observed_object
        .keys()
        .filter(|key| {
            !expected_object.contains_key(*key)
                && !matches!(
                    key.as_str(),
                    "typing_grace_ms"
                        | "native_extension_delivery"
                        | "extension_build"
                        | "extension_path"
                )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "{expected_key} fixture: response grew undocumented keys {unexpected:?}"
    );
    assert_eq!(
        observed.get("native_extension_delivery"),
        Some(&serde_json::json!(false))
    );
    let stored = registry
        .get(&descriptor.id)
        .await
        .expect("read committed descriptor");
    assert_eq!(
        stored,
        Some(descriptor),
        "binding is response-only, not durable state"
    );
}

#[tokio::test]
async fn registration_binding_created_matches_shared_fixture() {
    assert_registration_binding_fixture("created", None).await;
}

#[tokio::test]
async fn registration_binding_rebound_matches_shared_fixture() {
    assert_registration_binding_fixture("rebound", Some("created")).await;
}

#[tokio::test]
async fn registration_binding_same_matches_shared_fixture() {
    assert_registration_binding_fixture("same", Some("rebound")).await;
}

#[tokio::test]
async fn spawned_registration_adopts_prebind_row_and_preserves_port() {
    let registry = Arc::new(FakeRegistry::new());
    let tmux = Arc::new(FakeTmux::new().with_pane(Pane {
        id: "%caller".to_string(),
        session: "fleet".to_string(),
        window: "orchestrator".to_string(),
        title: String::new(),
        cursor_x: None,
        cursor_y: None,
    }));
    let identity = ProcIdentity {
        pid: 42,
        proc_start: 20260831090000,
    };
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.tmux = tmux.clone();
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    let spawned = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&SpawnRequest {
            id: None,
            harness: Harness::Copilot,
            allow_retired: false,
            executable: None,
            model: Some("provider/model".to_string()),
            effort: Some("high".to_string()),
            cwd: "/abs/tree".to_string(),
            session: Some("fleet".to_string()),
            caller_pane: None,
            name: None,
            parent: Some("pij-parent".into()),
            accept_inbound: false,
            wait_seconds: None,
            no_wait: true,
            resume: None,
            role: None,
            caller: None,
        })
        .send()
        .await
        .expect("spawn request");
    assert_eq!(spawned.status(), reqwest::StatusCode::OK);

    // Build the registration from what the launched process and tmux observe,
    // never from the spawn response. Using that response's assigned id here hid
    // the real mismatch: the registering runtime can claim a different id.
    let launch = tmux
        .calls()
        .into_iter()
        .find(|call| call.starts_with("new_window:"))
        .expect("observed launch");
    let launched_seat_id = launch
        .split('"')
        .find_map(|arg| arg.strip_prefix("PIJ_SESSION_ID="))
        .expect("launched seat id")
        .to_string();
    let launched_spawn_id = launch
        .split('"')
        .find_map(|arg| arg.strip_prefix("PIJ_SPAWN_ID="))
        .expect("launched spawn id")
        .to_string();
    let launched_pane = tmux
        .list_panes()
        .await
        .expect("launched panes")
        .into_iter()
        .find(|pane| pane.id != "%caller")
        .expect("spawned pane");
    let prebind = registry
        .list(SeatFilter::default())
        .await
        .expect("pre-bind roster")
        .into_iter()
        .find(|seat| seat.id.as_str() == launched_seat_id)
        .expect("pre-bind row");
    assert!(
        !prebind.native_extension_delivery,
        "spawn is not runtime attestation"
    );
    assert!(!launch.contains("--ui-server") && !launch.contains("--port"));
    assert_eq!(
        prebind.spawn_id.as_deref(),
        Some(launched_spawn_id.as_str())
    );
    assert_eq!(prebind.pane.as_deref(), Some(launched_pane.id.as_str()));

    let registered = client
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&Registration {
            supersedes: None,
            id: "pij-runtime-name".to_string(),
            harness: "copilot".to_string(),
            folder: "/abs/tree".to_string(),
            extension_build: None,
            extension_path: None,
            pane: Some(launched_pane.id.clone()),
            pid: Some(identity.pid),
            proc_start: Some(identity.proc_start),
            spawn_id: Some(launched_spawn_id.clone()),
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        })
        .send()
        .await
        .expect("registration");
    assert_eq!(registered.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<serde_json::Value> = registered
        .json()
        .await
        .expect("spawned registration envelope");
    assert_eq!(
        accepted.data.expect("bound descriptor")["binding"],
        "rebound",
        "a spawned placeholder row is rebound, not created"
    );

    let seats = registry
        .list(SeatFilter::default())
        .await
        .expect("bound roster");
    let bound = seats.iter().find(|seat| seat.proc == Some(identity));
    assert_eq!(
        (
            seats.len(),
            bound.map(|seat| seat.id.as_str()),
            bound.map(|seat| seat.native_extension_delivery),
        ),
        (1, Some(launched_seat_id.as_str()), Some(false)),
        "the launched identity is adopted once without inventing native attestation"
    );
    let bound = bound.expect("bound launched row");
    assert_eq!(bound.spawn_id.as_deref(), Some(launched_spawn_id.as_str()));
    assert_eq!(bound.pane.as_deref(), Some(launched_pane.id.as_str()));
    assert_eq!(
        bound.parent.as_ref().map(SeatId::as_str),
        Some("pij-parent"),
        "the first bind cannot erase the parent persisted by spawn"
    );

    server.abort();
}

#[tokio::test]
async fn retired_harness_spawn_refuses_before_pane_or_descriptor_mutation() {
    let policy: pij_core::config::Config = serde_json::from_value(serde_json::json!({
        "retired_harnesses": ["pi"]
    }))
    .expect("retired harness config");
    let mut services = crate::build_services(
        &policy,
        std::path::Path::new("/tmp/pij-test-retired-harness-signals"),
    )
    .await
    .expect("fake services");
    let registry = services.registry.clone();
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "id": "pij-retired",
            "harness": "pi",
            "cwd": "/abs/tree",
            "session": "fleet",
            "no_wait": true
        }))
        .send()
        .await
        .expect("spawn request");
    let status = response.status();
    let body: serde_json::Value = response.json().await.expect("spawn envelope");
    server.abort();

    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["meta"],
        "harness pi is retired on this machine; use omp (or pass --allow-retired)"
    );
    assert!(tmux.calls().is_empty(), "retired spawn never touches tmux");
    assert!(
        registry
            .get(&"pij-retired".into())
            .await
            .expect("registry read")
            .is_none(),
        "retired spawn writes no descriptor"
    );
}

#[tokio::test]
async fn retired_harness_override_cannot_launch_without_its_spine_event() {
    let registry = Arc::new(FakeRegistry::new());
    let spine = Arc::new(FakeSpine::new());
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("queue policy")),
        spine.clone(),
    )
    .await;
    services.retired_harnesses = vec![Harness::Pi].into();
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    spine.script_append_error("override audit unavailable");
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "id": "pij-unaudited",
            "harness": "pi",
            "allow_retired": true,
            "cwd": "/abs/tree",
            "session": "fleet",
            "no_wait": true
        }))
        .send()
        .await
        .expect("spawn request");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(tmux.calls().is_empty());
    assert!(
        registry
            .get(&"pij-unaudited".into())
            .await
            .expect("registry read")
            .is_none()
    );
    assert!(
        spine.is_empty(),
        "failed audit writes no accepted-override event"
    );
    server.abort();
}

#[tokio::test]
async fn retired_harness_override_launches_and_records_policy_exception() {
    let policy = pij_core::config::Config {
        retired_harnesses: vec![Harness::Pi],
        ..Default::default()
    };
    let mut services = crate::build_services(
        &policy,
        std::path::Path::new("/tmp/pij-test-retired-override-signals"),
    )
    .await
    .expect("fake services");
    let registry = services.registry.clone();
    let spine = services.spine.clone();
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let health: serde_json::Value = client
        .get(format!("http://{addr}/health"))
        .bearer_auth("key")
        .send()
        .await
        .expect("health request")
        .json()
        .await
        .expect("health envelope");
    assert_eq!(
        health["data"]["retired_harnesses"],
        serde_json::json!(["pi"])
    );

    for (id, harness) in [("pij-retired-allowed", "pi"), ("pij-current", "omp")] {
        let response = client
            .post(format!("http://{addr}/v1/spawn"))
            .bearer_auth("key")
            .json(&serde_json::json!({
                "id": id,
                "harness": harness,
                "allow_retired": true,
                "cwd": "/abs/tree",
                "session": "fleet",
                "no_wait": true
            }))
            .send()
            .await
            .expect("spawn request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: Envelope<SpawnResponse> = response.json().await.expect("spawn envelope");
        let launched = body.data.expect("spawn result");
        assert!(launched.dispatched);
        assert_eq!(launched.seat.harness.as_str(), harness);
        assert_eq!(
            registry
                .get(&id.into())
                .await
                .expect("registry read")
                .expect("persisted descriptor"),
            launched.seat
        );
        let overrides: Vec<Event> = spine
            .tail(Some(&id.into()), Seq(0))
            .await
            .expect("spine read")
            .into_iter()
            .filter(|event| event.kind == "spawn.retired-harness-override")
            .collect();
        if harness == "pi" {
            assert_eq!(overrides.len(), 1);
            let payload: serde_json::Value =
                serde_json::from_str(&overrides[0].payload).expect("override payload");
            assert_eq!(
                payload,
                serde_json::json!({
                    "harness": "pi",
                    "allow_retired": true,
                    "spawn_id": launched.seat.spawn_id
                })
            );
            assert_eq!(overrides[0].seat.as_ref(), Some(&launched.seat.id));
        } else {
            assert!(
                overrides.is_empty(),
                "current harness needs no policy exception"
            );
        }
    }
    assert_eq!(
        tmux.calls()
            .iter()
            .filter(|call| call.starts_with("new_window:"))
            .count(),
        2
    );
    server.abort();
}

#[tokio::test]
async fn executable_harness_mismatch_refuses_before_launch_or_override_event() {
    let policy = pij_core::config::Config {
        retired_harnesses: vec![Harness::Pi],
        ..Default::default()
    };
    let mut services = crate::build_services(
        &policy,
        std::path::Path::new("/tmp/pij-test-bin-mismatch-signals"),
    )
    .await
    .expect("fake services");
    let registry = services.registry.clone();
    let spine = services.spine.clone();
    let tmux = Arc::new(FakeTmux::new());
    services.tmux = tmux.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "id": "pij-bin-mismatch",
            "harness": "pi",
            "allow_retired": true,
            "executable": "/opt/harnesses/omp",
            "cwd": "/abs/tree",
            "session": "fleet",
            "no_wait": true
        }))
        .send()
        .await
        .expect("spawn request");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("spawn envelope");
    assert!(
        body["meta"]
            .as_str()
            .expect("refusal")
            .contains("use --harness omp")
    );
    assert!(tmux.calls().is_empty());
    assert!(
        registry
            .get(&"pij-bin-mismatch".into())
            .await
            .expect("registry read")
            .is_none()
    );
    assert!(
        spine
            .tail(Some(&"pij-bin-mismatch".into()), Seq(0))
            .await
            .expect("spine read")
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn spawn_wait_reports_child_exit_with_log_tail() {
    let registry = Arc::new(FakeRegistry::new());
    let tmux = Arc::new(
        FakeTmux::new().with_standing_capture("omp: model selector refused\nchild stopped"),
    );
    let spine = Arc::new(FakeSpine::new());
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine.clone(),
    )
    .await;
    services.tmux = tmux;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&SpawnRequest {
            id: Some("pij-exits-before-bind".into()),
            harness: Harness::Omp,
            allow_retired: false,
            executable: None,
            model: Some("bad/model".to_string()),
            effort: None,
            cwd: "/abs/tree".to_string(),
            session: Some("fleet".to_string()),
            caller_pane: None,
            name: None,
            parent: None,
            accept_inbound: false,
            wait_seconds: Some(1),
            no_wait: false,
            resume: None,
            role: None,
            caller: None,
        })
        .send();
    let report_exit = async {
        let spawned = loop {
            if let Some(spawned) = registry
                .get(&SeatId::from("pij-exits-before-bind"))
                .await
                .expect("read pre-bind row")
            {
                break spawned;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let spawn_id = spawned.spawn_id.expect("spawn correlator");
        let root = std::env::temp_dir().join("pij-rs-spawn");
        std::fs::create_dir_all(&root).expect("spawn evidence directory");
        std::fs::write(root.join(format!("{spawn_id}.status")), "1\n").expect("child exit status");
        (
            root.join(format!("{spawn_id}.log")),
            root.join(format!("{spawn_id}.status")),
        )
    };

    let (response, (log_path, status_path)) = tokio::join!(response, report_exit);
    let response = response.expect("spawn response");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body = response.text().await.expect("failure envelope");
    assert!(body.contains("spawn.failed"), "{body}");
    assert!(body.contains("exit code 1"), "{body}");
    assert!(body.contains(log_path.to_string_lossy().as_ref()), "{body}");
    assert!(body.contains("model selector refused"), "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("failure JSON");
    let details = parsed["details"]
        .as_object()
        .expect("structured failure details");
    assert_eq!(details.len(), 5, "details shape must stay exact");
    assert_eq!(details["dispatched"], true);
    assert_eq!(details["bound"], false);
    assert!(details["pane"].as_str().is_some());
    assert_eq!(details["pid"], serde_json::Value::Null);
    assert_eq!(details["reason"], "child exited before registration");
    let events = spine
        .tail(
            Some(&SeatId::from("pij-exits-before-bind")),
            pij_core::model::Seq(0),
        )
        .await
        .expect("failure event");
    assert_eq!(
        events.last().map(|event| event.kind.as_str()),
        Some("spawn.failed")
    );

    let _ = std::fs::remove_file(log_path);
    let _ = std::fs::remove_file(status_path);
    server.abort();
}

#[tokio::test]
async fn spawn_wait_returns_bound_process_and_publishes_event() {
    let registry = Arc::new(FakeRegistry::new());
    let tmux = Arc::new(FakeTmux::new());
    let spine = Arc::new(FakeSpine::new());
    let identity = ProcIdentity {
        pid: 13_242,
        proc_start: 20260902013242,
    };
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine.clone(),
    )
    .await;
    services.tmux = tmux;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&SpawnRequest {
            id: Some("pij-binds-after-spawn".into()),
            harness: Harness::Omp,
            allow_retired: false,
            executable: None,
            model: Some("github-copilot/gpt-5.6-sol".to_string()),
            effort: None,
            cwd: "/abs/tree".to_string(),
            session: Some("fleet".to_string()),
            caller_pane: None,
            name: None,
            parent: Some("pij-parent".into()),
            accept_inbound: false,
            wait_seconds: Some(1),
            no_wait: false,
            resume: None,
            role: None,
            caller: None,
        })
        .send();
    let register = async {
        let spawned = loop {
            if let Some(spawned) = registry
                .get(&SeatId::from("pij-binds-after-spawn"))
                .await
                .expect("read pre-bind row")
            {
                break spawned;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let registered = client
            .post(format!("http://{addr}/v1/register"))
            .bearer_auth("key")
            .json(&Registration {
                supersedes: None,
                id: "runtime-candidate".to_string(),
                harness: "omp".to_string(),
                folder: "/abs/tree".to_string(),
                extension_build: None,
                extension_path: None,
                pane: spawned.pane,
                pid: Some(identity.pid),
                proc_start: Some(identity.proc_start),
                spawn_id: spawned.spawn_id,
                model: Some("github-copilot/gpt-5.6-sol".to_string()),
                actual_model: Some("github-copilot/gpt-5.6-sol".to_string()),
                actual_model_observed: true,
                provider: None,
                effort: None,
                parent: None,
                role: None,
                relay: false,
            })
            .send()
            .await
            .expect("registration response");
        assert_eq!(registered.status(), reqwest::StatusCode::OK);
    };

    let (response, ()) = tokio::join!(response, register);
    let response = response.expect("spawn response");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let envelope: Envelope<SpawnResponse> = response.json().await.expect("spawn outcome");
    let outcome = envelope.data.expect("spawn outcome data");
    assert!(outcome.dispatched);
    assert!(outcome.bound);
    assert_eq!(outcome.pid, Some(identity.pid));
    assert_eq!(outcome.seat.id.as_str(), "pij-binds-after-spawn");
    assert_eq!(
        outcome.seat.parent.as_ref().map(SeatId::as_str),
        Some("pij-parent")
    );
    let events = spine
        .tail(
            Some(&SeatId::from("pij-binds-after-spawn")),
            pij_core::model::Seq(0),
        )
        .await
        .expect("bind event");
    assert_eq!(
        events.last().map(|event| event.kind.as_str()),
        Some("spawn.bound")
    );

    server.abort();
}

#[tokio::test]
async fn spawn_reports_model_mismatch_without_killing_registered_seat() {
    let registry = Arc::new(FakeRegistry::new());
    let tmux = Arc::new(FakeTmux::new().with_standing_capture(
        "Warning: Model requested/model not found\nfooter: pij-model-mismatch · no-model",
    ));
    let spine = Arc::new(FakeSpine::new());
    let identity = ProcIdentity {
        pid: 13_243,
        proc_start: 20260902013243,
    };
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine.clone(),
    )
    .await;
    services.tmux = tmux.clone();
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&SpawnRequest {
            id: Some("pij-model-mismatch".into()),
            harness: Harness::Omp,
            allow_retired: false,
            executable: None,
            model: Some("requested/model".to_string()),
            effort: None,
            cwd: "/abs/tree".to_string(),
            session: Some("fleet".to_string()),
            caller_pane: None,
            name: None,
            parent: None,
            accept_inbound: false,
            wait_seconds: Some(1),
            no_wait: false,
            resume: None,
            role: None,
            caller: None,
        })
        .send();
    let register = async {
        let spawned = loop {
            if let Some(spawned) = registry
                .get(&SeatId::from("pij-model-mismatch"))
                .await
                .expect("read pre-bind row")
            {
                break spawned;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let registered = client
            .post(format!("http://{addr}/v1/register"))
            .bearer_auth("key")
            .json(&Registration {
                supersedes: None,
                id: "pij-model-mismatch".to_string(),
                harness: "omp".to_string(),
                folder: "/abs/tree".to_string(),
                extension_build: None,
                extension_path: None,
                pane: spawned.pane,
                pid: Some(identity.pid),
                proc_start: Some(identity.proc_start),
                spawn_id: spawned.spawn_id,
                model: Some("requested/model".to_string()),
                actual_model: None,
                actual_model_observed: true,
                provider: None,
                effort: None,
                parent: None,
                role: None,
                relay: false,
            })
            .send()
            .await
            .expect("registration response");
        assert_eq!(registered.status(), reqwest::StatusCode::OK);
    };

    let (response, ()) = tokio::join!(response, register);
    let response = response.expect("spawn response");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body = response.text().await.expect("model mismatch envelope");
    assert!(body.contains("model-mismatch"), "{body}");
    assert!(body.contains("requested/model"), "{body}");
    assert!(body.contains("actual no-model"), "{body}");
    assert!(
        body.contains("footer: pij-model-mismatch · no-model"),
        "{body}"
    );
    let bound = registry
        .get(&SeatId::from("pij-model-mismatch"))
        .await
        .expect("bound row read")
        .expect("bound row retained");
    assert_eq!(
        bound.proc,
        Some(identity),
        "mismatch reports but does not kill the seat"
    );
    assert!(
        tmux.list_panes()
            .await
            .expect("pane list")
            .iter()
            .any(|pane| { pane.id == bound.pane.as_deref().expect("bound pane") })
    );
    let events = spine
        .tail(
            Some(&SeatId::from("pij-model-mismatch")),
            pij_core::model::Seq(0),
        )
        .await
        .expect("mismatch event");
    assert_eq!(
        events.last().map(|event| event.kind.as_str()),
        Some("spawn.failed")
    );

    server.abort();
}

#[tokio::test]
async fn reregistration_prefers_exact_id_over_spawn_match_without_fresh_allocation() {
    let identity = ProcIdentity {
        pid: 43,
        proc_start: 20260831090001,
    };
    let mut unrelated = SeatDescriptor::new("pij-a-prebind", Harness::Copilot, "/abs/tree");
    unrelated.pane = Some("%44".to_string());
    unrelated.spawn_id = Some("spawn-shared".to_string());
    let mut existing = SeatDescriptor::new("pij-z-existing", Harness::Copilot, "/abs/tree");
    existing.pane = Some("%44".to_string());
    existing.proc = Some(identity);
    existing.spawn_id = Some("spawn-shared".to_string());
    let registry = Arc::new(FakeRegistry::new().with_seat(unrelated).with_seat(existing));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&Registration {
            supersedes: None,
            id: "pij-z-existing".to_string(),
            harness: "copilot".to_string(),
            folder: "/abs/tree".to_string(),
            extension_build: None,
            extension_path: None,
            pane: Some("%44".to_string()),
            pid: Some(identity.pid),
            proc_start: Some(identity.proc_start),
            spawn_id: Some("spawn-shared".to_string()),
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        })
        .send()
        .await
        .expect("repeat registration");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<SeatDescriptor> = response.json().await.expect("descriptor envelope");
    let accepted = accepted.data.expect("descriptor");
    assert_eq!(accepted.id.as_str(), "pij-z-existing");
    assert!(
        !accepted.native_extension_delivery,
        "ordinary registration is not attestation"
    );

    let seats = registry
        .list(SeatFilter::default())
        .await
        .expect("roster after repeat registration");
    let unrelated = seats
        .iter()
        .find(|seat| seat.id.as_str() == "pij-a-prebind")
        .expect("unrelated pre-bind row");
    assert_eq!(unrelated.proc, None, "repeat registration cannot adopt it");
    assert!(!unrelated.native_extension_delivery);

    server.abort();
}

/// Ordinary registration must not copy an old incarnation's native attestation.
#[tokio::test]
async fn registration_does_not_inherit_native_capability_from_another_incarnation() {
    let identity = ProcIdentity {
        pid: 47,
        proc_start: 20260831090005,
    };
    let mut stamped = SeatDescriptor::new("pij-ported", Harness::Copilot, "/abs/tree");
    stamped.pane = Some("%47".to_string());
    stamped.spawn_id = Some("spawn-old".to_string());
    stamped.proc = Some(ProcIdentity {
        pid: 46,
        proc_start: 46,
    });
    stamped.harness_session = Some("native-old".into());
    stamped.native_extension_delivery = true;
    let registry = Arc::new(FakeRegistry::new().with_seat(stamped.clone()));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&Registration {
            supersedes: None,
            // Exact id match, so the spawn_id fallback never runs.
            id: "pij-ported".to_string(),
            harness: "copilot".to_string(),
            folder: "/abs/tree".to_string(),
            extension_build: None,
            extension_path: None,
            pane: Some("%47".to_string()),
            pid: Some(identity.pid),
            proc_start: Some(identity.proc_start),
            // A different incarnation must not inherit native ownership.
            spawn_id: Some("spawn-new".to_string()),
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        })
        .send()
        .await
        .expect("registration with a mismatched spawn id");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let refused: Envelope<SeatDescriptor> = response.json().await.expect("refusal envelope");
    assert!(!refused.ok);
    assert_eq!(registry.get(&stamped.id).await.unwrap(), Some(stamped));

    server.abort();
}

/// THE FOURTH ARM. spawn_id + pane alone is not enough: the matched row must also
/// be UNBOUND. Without `proc.is_none()` a registration could adopt a row whose
/// process is ALREADY ALIVE — one live seat quietly taking over another's identity
/// on a reused pane, which is worse than the two-row defect this unit fixes
/// because it looks correct from both ends separately.
///
/// Mutate the `seat.proc.is_none()` clause out of the fallback in `http/mod.rs`
/// and this test must fail.
#[tokio::test]
async fn registration_cannot_adopt_a_spawn_match_that_is_already_bound() {
    let squatter = ProcIdentity {
        pid: 45,
        proc_start: 20260831090003,
    };
    let incumbent = ProcIdentity {
        pid: 46,
        proc_start: 20260831090004,
    };
    let mut live = SeatDescriptor::new("pij-already-bound", Harness::Copilot, "/abs/tree");
    live.pane = Some("%46".to_string());
    live.spawn_id = Some("spawn-bound".to_string());
    live.native_extension_delivery = true;
    live.harness_session = Some("native-incumbent".into());
    // The row is BOUND: its process is alive and it is nobody's pre-bind row.
    live.proc = Some(incumbent);
    let registry = Arc::new(FakeRegistry::new().with_seat(live));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(squatter));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&Registration {
            supersedes: None,
            id: "pij-runtime-squatter".to_string(),
            harness: "copilot".to_string(),
            folder: "/abs/tree".to_string(),
            extension_build: None,
            extension_path: None,
            pane: Some("%46".to_string()),
            pid: Some(squatter.pid),
            proc_start: Some(squatter.proc_start),
            spawn_id: Some("spawn-bound".to_string()),
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        })
        .send()
        .await
        .expect("registration against a bound row");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<SeatDescriptor> = response.json().await.expect("descriptor envelope");
    let accepted = accepted.data.expect("descriptor");
    assert_eq!(
        accepted.id.as_str(),
        "pij-runtime-squatter",
        "a bound row must never be adopted — the registration gets its own row"
    );
    assert!(
        !accepted.native_extension_delivery,
        "no capability inherited from another seat"
    );

    let seats = registry.list(SeatFilter::default()).await.expect("roster");
    let incumbent_row = seats
        .iter()
        .find(|seat| seat.id.as_str() == "pij-already-bound")
        .expect("the incumbent row must survive untouched");
    assert_eq!(
        incumbent_row.proc,
        Some(incumbent),
        "the live seat must still own its own process"
    );
    assert!(incumbent_row.native_extension_delivery);

    server.abort();
}

#[tokio::test]
async fn registration_cannot_adopt_a_spawn_match_from_another_pane() {
    let identity = ProcIdentity {
        pid: 44,
        proc_start: 20260831090002,
    };
    let mut unrelated =
        SeatDescriptor::new("pij-prebind-other-pane", Harness::Copilot, "/abs/tree");
    unrelated.pane = Some("%44".to_string());
    unrelated.spawn_id = Some("spawn-shared".to_string());
    let registry = Arc::new(FakeRegistry::new().with_seat(unrelated));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(identity));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/register"))
        .bearer_auth("key")
        .json(&Registration {
            supersedes: None,
            id: "pij-runtime-other-pane".to_string(),
            harness: "copilot".to_string(),
            folder: "/abs/tree".to_string(),
            extension_build: None,
            extension_path: None,
            pane: Some("%45".to_string()),
            pid: Some(identity.pid),
            proc_start: Some(identity.proc_start),
            spawn_id: Some("spawn-shared".to_string()),
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        })
        .send()
        .await
        .expect("registration from another pane");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let accepted: Envelope<SeatDescriptor> = response.json().await.expect("descriptor envelope");
    let accepted = accepted.data.expect("descriptor");
    assert_eq!(accepted.id.as_str(), "pij-runtime-other-pane");
    assert!(
        !accepted.native_extension_delivery,
        "another pane does not confer attestation"
    );

    let unrelated = registry
        .list(SeatFilter::default())
        .await
        .expect("roster after unrelated registration")
        .into_iter()
        .find(|seat| seat.id.as_str() == "pij-prebind-other-pane")
        .expect("unrelated pre-bind row remains");
    assert_eq!(unrelated.proc, None);
    assert!(!unrelated.native_extension_delivery);

    server.abort();
}

#[tokio::test]
async fn ordinary_registration_preserves_native_attestation_and_claude_consent() {
    let copilot_proc = ProcIdentity {
        pid: 43,
        proc_start: 20260831103043,
    };
    let claude_proc = ProcIdentity {
        pid: 44,
        proc_start: 20260831103044,
    };
    let registry = Arc::new(FakeRegistry::new());
    let mut copilot = SeatDescriptor::new("pij-copilot", Harness::Copilot, "/abs/tree");
    copilot.pane = Some("%43".to_string());
    copilot.proc = Some(copilot_proc);
    copilot.spawn_id = Some("spawn-copilot".to_string());
    copilot.native_extension_delivery = true;
    copilot.harness_session = Some("native-copilot".into());
    registry.put(copilot).await.unwrap();
    let mut claude = SeatDescriptor::new("pij-claude", Harness::Claude, "/abs/tree");
    claude.pane = Some("%44".to_string());
    claude.proc = Some(claude_proc);
    claude.spawn_id = Some("spawn-claude".to_string());
    claude.cross_session_inbound_accept = Some(true);
    registry.put(claude).await.unwrap();

    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(
        FakeLiveness::new()
            .with_proc(copilot_proc)
            .with_proc(claude_proc),
    );
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    for (id, harness, pane, identity, spawn_id) in [
        (
            "pij-copilot",
            "copilot",
            "%43",
            copilot_proc,
            "spawn-copilot",
        ),
        ("pij-claude", "claude", "%44", claude_proc, "spawn-claude"),
    ] {
        let response = client
            .post(format!("http://{addr}/v1/register"))
            .bearer_auth("key")
            .json(&Registration {
                supersedes: None,
                id: id.to_string(),
                harness: harness.to_string(),
                folder: "/abs/tree".to_string(),
                extension_build: None,
                extension_path: None,
                pane: Some(pane.to_string()),
                pid: Some(identity.pid),
                proc_start: Some(identity.proc_start),
                spawn_id: Some(spawn_id.to_string()),
                model: None,
                actual_model: None,
                actual_model_observed: false,
                provider: None,
                effort: None,
                parent: None,
                role: None,
                relay: false,
            })
            .send()
            .await
            .expect("repeat registration");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }

    assert!(
        registry
            .get(&"pij-copilot".into())
            .await
            .unwrap()
            .unwrap()
            .native_extension_delivery
    );
    assert_eq!(
        registry
            .get(&"pij-claude".into())
            .await
            .unwrap()
            .unwrap()
            .cross_session_inbound_accept,
        Some(true)
    );

    server.abort();
}

#[tokio::test]
async fn registration_refuses_uncorroborated_or_half_process_identity() {
    let registry = Arc::new(FakeRegistry::new());
    let services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let base = Registration {
        supersedes: None,
        id: "pij-claim".to_string(),
        harness: "omp".to_string(),
        folder: "/abs/tree".to_string(),
        extension_build: None,
        extension_path: None,
        pane: None,
        pid: Some(7),
        proc_start: Some(11),
        spawn_id: None,
        model: None,
        actual_model: None,
        actual_model_observed: false,
        provider: None,
        effort: None,
        parent: None,
        role: None,
        relay: false,
    };
    for (label, claim, expected) in [
        ("uncorroborated", base.clone(), "daemon observed process 7"),
        (
            "half identity",
            Registration {
                proc_start: None,
                ..base
            },
            "pid and proc_start must travel together",
        ),
    ] {
        let response = client
            .post(format!("http://{addr}/v1/register"))
            .bearer_auth("key")
            .json(&claim)
            .send()
            .await
            .expect("registration refusal");
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{label}"
        );
        assert!(
            response.text().await.expect("body").contains(expected),
            "{label}"
        );
    }
    assert!(
        registry.calls().is_empty(),
        "invalid evidence reaches no registry statement"
    );

    server.abort();
}

#[tokio::test]
async fn spawn_stamps_only_the_argv_that_reached_the_observed_launch() {
    let registry = Arc::new(FakeRegistry::new());
    let tmux = Arc::new(FakeTmux::new().with_pane(Pane {
        id: "%caller".to_string(),
        session: "fleet".to_string(),
        window: "orchestrator".to_string(),
        title: String::new(),
        cursor_x: None,
        cursor_y: None,
    }));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.tmux = tmux.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    let request =
        |id: &str, accept_inbound: bool, session: Option<&str>, pane: Option<&str>| SpawnRequest {
            id: Some(id.into()),
            harness: Harness::Claude,
            allow_retired: false,
            executable: Some("/path with spaces/claude".to_string()),
            model: Some("provider/model".to_string()),
            effort: Some("high".to_string()),
            cwd: "/abs/tree".to_string(),
            session: session.map(str::to_string),
            caller_pane: pane.map(str::to_string),
            name: None,
            parent: Some("pij-parent".into()),
            accept_inbound,
            wait_seconds: None,
            no_wait: true,
            resume: None,
            role: None,
            caller: None,
        };

    for (id, accept, session, pane) in [
        ("pij-closed", false, None, Some("%caller")),
        ("pij-open", true, Some("fleet"), None),
    ] {
        let response = client
            .post(format!("http://{addr}/v1/spawn"))
            .bearer_auth("key")
            .json(&request(id, accept, session, pane))
            .send()
            .await
            .expect("spawn request");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let envelope: Envelope<SeatDescriptor> = response.json().await.expect("spawn envelope");
        let descriptor = envelope.data.expect("descriptor");
        assert_eq!(descriptor.id.as_str(), id);
        assert_eq!(descriptor.proc, None, "spawn writes a pre-bind row");
        assert_eq!(descriptor.cross_session_inbound_accept, Some(accept));
        let spawn_id = descriptor.spawn_id.expect("launcher stamps spawn id");
        assert_eq!(spawn_id.len(), 34);
        assert!(spawn_id.strip_prefix("s-").is_some_and(|hex| {
            hex.chars()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
        }));
        assert_eq!(descriptor.model.as_deref(), Some("provider/model"));
        assert_eq!(descriptor.provider, None, "provider was not resolved");
        assert_eq!(descriptor.effort.as_deref(), Some("high"));
    }

    let calls = tmux.calls();
    assert_eq!(calls[0], "list_panes");
    assert!(calls[1].contains("new_window:fleet:pij-closed:"));
    assert!(calls[1].contains("__spawn-child"));
    assert!(calls[1].contains("\"env\""));
    assert!(calls[1].contains("PIJ_SESSION_ID=pij-closed"));
    assert!(calls[1].contains("/path with spaces/claude"));
    assert!(!calls[1].contains("crossSessionInbound"));
    assert!(calls[2].contains("new_window:fleet:pij-open:"));
    assert!(calls[2].contains("__spawn-child"));
    assert!(calls[2].contains("\"env\""));
    assert!(calls[2].contains("PIJ_SESSION_ID=pij-open"));
    assert!(calls[2].contains("/path with spaces/claude"));
    assert!(calls[2].contains(r#"{\"crossSessionInbound\":\"accept\"}"#));

    let collision = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&request("pij-open", true, Some("fleet"), None))
        .send()
        .await
        .expect("collision request");
    assert_eq!(collision.status(), reqwest::StatusCode::BAD_REQUEST);
    let collision_body = collision.text().await.expect("collision body");
    assert!(collision_body.contains("already exists and is not tombstoned"));
    assert!(collision_body.contains("choose another --id or tombstone it before respawning"));
    assert!(!collision_body.contains("reviv"));
    assert_eq!(tmux.calls().len(), 3, "collision launches no extra process");

    let unresolved = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&request("pij-unresolved", false, None, Some("%missing")))
        .send()
        .await
        .expect("unresolved request");
    assert_eq!(unresolved.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        unresolved
            .text()
            .await
            .expect("unresolved body")
            .contains("pass --session")
    );
    assert_eq!(tmux.calls().last().expect("last call"), "list_panes");

    server.abort();
}

/// Plan 116's "Claude seats accept inbound by default" is a CLI default
/// (`prepare_spawn_request`), not a daemon one. `accept_inbound` on the wire
/// is a plain `bool`, which cannot distinguish "the caller said no" from "the
/// caller never considered it" — so this handler applies no default of its
/// own and never could without a tri-state field (a follow-up, not built
/// here). A direct `/v1/spawn` caller — another client, or a routing shim
/// that does not go through the CLI — gets pre-116 behaviour: no consent
/// unless the request says so. This test pins that boundary so it stays a
/// named, scoped fact instead of a silently-assumed one.
#[tokio::test]
async fn direct_http_spawn_carries_no_claude_default_the_cli_owns_it() {
    let registry = Arc::new(FakeRegistry::new());
    let tmux = Arc::new(FakeTmux::new().with_pane(Pane {
        id: "%caller".to_string(),
        session: "fleet".to_string(),
        window: "orchestrator".to_string(),
        title: String::new(),
        cursor_x: None,
        cursor_y: None,
    }));
    let mut services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.tmux = tmux.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("http://{addr}/v1/spawn"))
        .bearer_auth("key")
        .json(&SpawnRequest {
            id: Some("pij-plan116-direct".into()),
            harness: Harness::Claude,
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
        .send()
        .await
        .expect("spawn request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let envelope: Envelope<SeatDescriptor> = response.json().await.expect("spawn envelope");
    let descriptor = envelope.data.expect("descriptor");
    assert_eq!(
        descriptor.cross_session_inbound_accept,
        Some(false),
        "a direct /v1/spawn caller sending accept_inbound:false gets no consent — \
         the plan 116 default lives only in the CLI, and the daemon must never \
         invent one behind a bool it cannot tell 'unset' from"
    );

    server.abort();
}

#[tokio::test]
async fn send_enqueues_exactly_once_and_refuses_remote_without_federation() {
    let queue = Arc::new(CountingQueue::default());
    // The recipient must EXIST now. The route used to enqueue without consulting
    // the registry at all, so this test passed against a roster that had never
    // heard of `pij-b` — which is precisely the gap that let a private payload
    // shape survive: nothing on this path ever asked the delivery service
    // anything. An unregistered recipient is a 404 here, and that is the route
    // working.
    let registry = Arc::new(FakeRegistry::new());
    // Registered but UNBOUND (no process identity), so routing queues it. This
    // test's subject is the enqueue — one row, the caller's dedupe key, and a
    // payload the inbox can decode — so the recipient is arranged to reach that
    // path honestly rather than by a transport that refuses.
    let recipient = SeatDescriptor::new("pij-b", Harness::Omp, "/abs/tree");
    registry.put(recipient).await.expect("register recipient");
    let services = test_services(registry, queue.clone(), Arc::new(FakeSpine::new())).await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let request = SendRequest {
        fyi: false,
        force: false,
        reason: None,
        from: "pij-a".into(),
        from_machine: None,
        to: Destination::local("pij-b"),
        body: "hello".to_string(),
        msg_id: "m-1".to_string(),
        in_reply_to: Some("m-0".to_string()),
    };

    let response = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("key")
        .json(&request)
        .send()
        .await
        .expect("send");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let receipt: Envelope<Receipt> = response.json().await.expect("receipt envelope");
    assert!(matches!(
        receipt.data.expect("receipt").outcome,
        pij_core::model::DeliveryOutcome::Queued {
            reason: Some(reason),
            next_retry_at: Some(0),
            ..
        } if reason == "pre-bind"
    ));

    let jobs = queue.jobs();
    assert_eq!(jobs.len(), 1, "the request path performs one enqueue");
    assert_eq!(
        jobs[0].kind,
        pij_core::delivery::delivery_kind(&"pij-b".into())
    );
    assert_eq!(jobs[0].dedupe_key, "m-1");
    // Decoded as `Msg` — the SAME type `DeliveryService::inbox` decodes this
    // queue as. The old assertion decoded the route's own private shape, so both
    // halves of the round trip passed while the round trip itself could never
    // work. Assert the consumer's type, never the producer's.
    let queued: Msg = serde_json::from_str(&jobs[0].payload).expect("queued body");
    assert_eq!(queued.msg_id, request.msg_id);
    assert_eq!(queued.from, request.from);
    assert_eq!(queued.from_machine, None);
    assert_eq!(queued.to, request.to.seat);
    assert_eq!(queued.body, request.body);
    // A NON-None value, because every other construction in the workspace passes
    // None: a regression that dropped the field at the Msg boundary — the exact
    // defect F3 fixed — would otherwise pass the whole suite.
    assert_eq!(
        queued.in_reply_to.as_deref(),
        Some("m-0"),
        "the answered msg_id must survive into the durable payload"
    );

    let remote = SendRequest {
        to: Destination {
            seat: "pij-b".into(),
            machine: Some("laptop".to_string()),
        },
        msg_id: "m-2".to_string(),
        ..request
    };
    let response = client
        .post(format!("http://{addr}/v1/send"))
        .bearer_auth("key")
        .json(&remote)
        .send()
        .await
        .expect("remote send");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .expect("body")
            .contains("require configured federation peers")
    );
    assert_eq!(
        queue.jobs().len(),
        1,
        "a refused remote send enqueues nothing"
    );

    server.abort();
}

#[tokio::test]
async fn inbox_claims_without_evidence_then_ack_records_machine_graded_reader_read() {
    let registry = Arc::new(FakeRegistry::new());
    registry
        .put(SeatDescriptor::new("pij-reader", Harness::Omp, "/abs/tree"))
        .await
        .expect("register pull recipient");
    let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(registry, queue.clone(), spine.clone()).await;
    services
        .delivery
        .accept(Msg {
            from: "pij-sender".into(),
            from_machine: None,
            to: "pij-reader".into(),
            body: "one durable body".to_string(),
            msg_id: "m-inbox-1".to_string(),
            in_reply_to: None,
            command: None,
        })
        .await
        .expect("queue message");
    let (addr, server) = spawn(router_with_config(
        services,
        config("local-key", &["peer-key"]),
    ))
    .await;
    let client = reqwest::Client::new();
    let peek = client
        .get(format!("http://{addr}/v1/inbox"))
        .bearer_auth("local-key")
        .query(&[("seat", "pij-reader"), ("wait", "false"), ("peek", "true")])
        .send()
        .await
        .expect("peek inbox");
    assert_eq!(peek.status(), reqwest::StatusCode::OK);
    let peeked: Envelope<Vec<crate::delivery::InboxClaim>> =
        peek.json().await.expect("peek envelope");
    let peeked = peeked.data.expect("peeked page");
    assert_eq!(peeked.len(), 1);
    assert_eq!(peeked[0].message.body, "one durable body");
    assert!(queue.acked().is_empty(), "peek must not acknowledge");
    assert!(
        spine
            .tail(Some(&SeatId::from("pij-reader")), Seq(0))
            .await
            .expect("audit after peek")
            .iter()
            .all(|event| event.kind != INBOX_ACK_EVENT_KIND),
        "peek emits no read attestation"
    );

    let response = client
        .get(format!("http://{addr}/v1/inbox"))
        .bearer_auth("local-key")
        .query(&[("seat", "pij-reader"), ("wait", "false")])
        .send()
        .await
        .expect("claim inbox");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let page: Envelope<Vec<crate::delivery::InboxClaim>> =
        response.json().await.expect("inbox page envelope");
    let claims = page.data.expect("claimed page");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].message.body, "one durable body");
    assert!(queue.acked().is_empty(), "GET records no ReaderRead");
    assert!(
        spine
            .tail(Some(&SeatId::from("pij-reader")), Seq(0))
            .await
            .expect("audit before ack")
            .iter()
            .all(|event| event.kind != INBOX_ACK_EVENT_KIND),
        "GET emits no read attestation"
    );

    let acknowledgement = InboxAckRequest {
        delivery_outcome: None, // Deliberately forged: bearer auth is machine-grade and cannot name the
        // seat. Attribution must come from the queue authority's claimed job.
        seat: "pij-forged-reader".into(),
        job_id: claims[0].job_id,
        native: Default::default(),
        control_outcome: None,
    };
    let ack = client
        .post(format!("http://{addr}/v1/inbox/ack"))
        .bearer_auth("local-key")
        .json(&acknowledgement)
        .send()
        .await
        .expect("ack inbox");
    assert_eq!(ack.status(), reqwest::StatusCode::OK);
    assert_eq!(queue.acked(), [(claims[0].job_id, Outcome::Done)]);
    let read_events: Vec<_> = spine
        .tail(Some(&SeatId::from("pij-reader")), Seq(0))
        .await
        .expect("read audit")
        .into_iter()
        .filter(|event| event.kind == INBOX_ACK_EVENT_KIND)
        .collect();
    assert_eq!(read_events.len(), 1);
    let audit: serde_json::Value =
        serde_json::from_str(&read_events[0].payload).expect("read audit payload");
    assert_eq!(audit["job_id"], claims[0].job_id.0);
    assert_eq!(audit["authenticated_machine"], "local");
    assert_eq!(audit["evidence_grade"], "machine");
    assert_eq!(audit["outcome"], "reader-read");
    assert!(
        spine
            .tail(Some(&SeatId::from("pij-forged-reader")), Seq(0))
            .await
            .expect("forged seat audit")
            .iter()
            .all(|event| event.kind != INBOX_ACK_EVENT_KIND),
        "request body cannot forge reader attribution"
    );

    let invalid = client
        .post(format!("http://{addr}/v1/inbox/ack"))
        .bearer_auth("local-key")
        .json(&InboxAckRequest {
            delivery_outcome: None,
            seat: "pij-forged-reader".into(),
            job_id: pij_core::model::JobId(999_999),
            native: Default::default(),
            control_outcome: None,
        })
        .send()
        .await
        .expect("invalid ack request");
    assert_eq!(invalid.status(), reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        spine
            .tail(Some(&SeatId::from("pij-reader")), Seq(0))
            .await
            .expect("audit after failed ack")
            .iter()
            .filter(|event| event.kind == INBOX_ACK_EVENT_KIND)
            .count(),
        1,
        "failed ledger acknowledgement must not mint attestation evidence"
    );

    let empty = client
        .get(format!("http://{addr}/v1/inbox"))
        .bearer_auth("peer-key")
        .query(&[("seat", "pij-reader"), ("wait", "false")])
        .send()
        .await
        .expect("second read");
    let empty: Envelope<Vec<crate::delivery::InboxClaim>> =
        empty.json().await.expect("empty inbox envelope");
    assert!(empty.data.expect("empty page").is_empty());

    server.abort();
}

#[tokio::test]
async fn seats_performs_one_unfiltered_registry_list() {
    let mut seat = SeatDescriptor::new("pij-b", pij_core::model::Harness::Omp, "/abs/tree");
    seat.harness_session = Some("omp-native-seat".to_string());
    let registry = Arc::new(FakeRegistry::new().with_seat(seat));
    let services = test_services(
        registry.clone(),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .get(format!("http://{addr}/v1/seats"))
        .bearer_auth("key")
        .send()
        .await
        .expect("seats");
    let body: Envelope<serde_json::Value> = response.json().await.expect("seats envelope");
    let roster = body.data.expect("data");
    assert_eq!(roster["seats"].as_array().expect("seat rows").len(), 1);
    assert_eq!(roster["seats"][0]["machine"], "workstation");
    assert_eq!(roster["seats"][0]["session"], "omp-native-seat");
    assert_eq!(roster["seats"][0]["generation"], "rs");
    assert_eq!(roster["unavailable"], serde_json::json!([]));
    assert_eq!(registry.calls(), ["list"]);

    server.abort();
}

#[tokio::test]
async fn events_are_live_by_default_and_frame_unknown_kinds_with_a_cursor() {
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine,
    )
    .await;
    let bus = services.event_bus.clone();
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let mut response = reqwest::Client::new()
        .get(format!("http://{addr}/v1/events"))
        .bearer_auth("key")
        .send()
        .await
        .expect("events");

    let hello = response.chunk().await.expect("hello chunk").expect("hello");
    assert!(matches!(
        wire::decode_event_line(1, std::str::from_utf8(&hello).expect("utf8").trim())
            .expect("hello"),
        WireEvent::Hello { .. }
    ));

    bus.publish(Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at: 7,
        kind: "future.kind".to_string(),
        seat: Some("pij-b".into()),
        payload: "{\"new\":true}".to_string(),
    })
    .await
    .expect("publish");

    let frame = tokio::time::timeout(Duration::from_secs(1), response.chunk())
        .await
        .expect("unknown kind must be forwarded, not filtered")
        .expect("frame chunk")
        .expect("frame");
    let frame: StreamFrame = serde_json::from_slice(&frame).expect("event frame");
    let (machine, cursor, event) = expect_event(frame.clone());
    assert_eq!(machine, "workstation");
    assert_eq!(cursor, 1);
    assert_eq!(event.kind, "future.kind");
    let encoded = serde_json::to_value(&frame).expect("serialize frame");
    assert!(
        encoded["event"].get("seq").is_none(),
        "cursor stays out of event body"
    );

    server.abort();
}

#[tokio::test]
async fn explicit_since_replays_then_stays_live() {
    let spine = Arc::new(FakeSpine::new());
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        spine,
    )
    .await;
    let bus = services.event_bus.clone();
    bus.publish(Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at: 1,
        kind: "message".to_string(),
        seat: None,
        payload: "{}".to_string(),
    })
    .await
    .expect("publish replay");
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let mut response = reqwest::Client::new()
        .get(format!("http://{addr}/v1/events"))
        .query(&[("since", r#"{"workstation":0}"#)])
        .bearer_auth("key")
        .send()
        .await
        .expect("events");
    let _hello = response.chunk().await.expect("hello chunk").expect("hello");
    let replay = response
        .chunk()
        .await
        .expect("replay chunk")
        .expect("replay");
    let replay: StreamFrame = serde_json::from_slice(&replay).expect("replay frame");
    assert_eq!(expect_event(replay).1, 1);

    bus.publish(Event {
        seq: None,
        v: wire::EVENT_VERSION,
        at: 2,
        kind: "future.live".to_string(),
        seat: None,
        payload: "{}".to_string(),
    })
    .await
    .expect("publish live");
    let live = response.chunk().await.expect("live chunk").expect("live");
    let live: StreamFrame = serde_json::from_slice(&live).expect("live frame");
    assert_eq!(expect_event(live).1, 2);

    server.abort();
}

#[tokio::test]
async fn peer_forwarding_makes_one_authenticated_request() {
    #[derive(Clone)]
    struct PeerState {
        calls: Arc<AtomicUsize>,
    }

    async fn peer(
        State(state): State<PeerState>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Json<Envelope<serde_json::Value>> {
        assert_eq!(
            headers[axum::http::header::AUTHORIZATION],
            "Bearer peer-key"
        );
        state.calls.fetch_add(1, Ordering::SeqCst);
        Json(Envelope::ok("peer test", body))
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let (addr, server) = spawn(
        Router::new()
            .route("/v1/test", post(peer))
            .with_state(PeerState {
                calls: calls.clone(),
            }),
    )
    .await;
    let endpoint = PeerEndpoint {
        base_url: format!("http://{addr}/"),
        bearer_key: "peer-key".to_string(),
    };
    let request = serde_json::json!({"value": 7});
    let response: Envelope<serde_json::Value> =
        post_to_peer(&reqwest::Client::new(), &endpoint, "/v1/test", &request)
            .await
            .expect("forward");
    assert_eq!(response.data.expect("data"), request);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    server.abort();
}

/// u-extension, wave 3 — pi keeps ONE process across `/new` and `/fork` while the
/// extension deliberately assigns a NEW seat id, so the successor is
/// indistinguishable from finding #19's alias and the first `/new` from a healthy
/// seat would have been refused forever.
///
/// Both halves are proven here, because the fix must not weaken #19: a claim that
/// supersedes ITSELF is admitted and dissolves its predecessor with a readable
/// reason, and a claim that names someone ELSE's seat is still refused.
#[tokio::test]
async fn a_native_session_boundary_supersedes_its_own_seat_but_never_another() {
    let registry = Arc::new(FakeRegistry::new());
    let pid = std::process::id();
    let proc_start = 4_242;

    let mut first = SeatDescriptor::new("pij-first", Harness::Omp, "/abs/tree");
    first.proc = Some(ProcIdentity { pid, proc_start });
    registry.put(first).await.expect("first seat");
    let mut stranger = SeatDescriptor::new("pij-stranger", Harness::Omp, "/abs/tree");
    stranger.proc = Some(ProcIdentity {
        pid,
        proc_start: proc_start + 1,
    });
    registry.put(stranger).await.expect("stranger seat");

    let mut services = test_services(
        registry.clone(),
        Arc::new(CountingQueue::default()),
        Arc::new(FakeSpine::new()),
    )
    .await;
    // The daemon CORROBORATES the claim, so the fake process table must know the
    // identity this test claims — otherwise the refusal under test is the
    // liveness check, not the supersession rule.
    services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity { pid, proc_start }));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/register");

    let claim = serde_json::json!({
        "id": "pij-second",
        "harness": "omp",
        "folder": "/abs/tree",
        "pid": pid,
        "proc_start": proc_start,
        "supersedes": "pij-first",
    });
    let response = client
        .post(&url)
        .bearer_auth("key")
        .json(&claim)
        .send()
        .await
        .expect("supersede");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "a seat must be able to succeed itself across /new"
    );

    let seats = registry
        .list(SeatFilter::default())
        .await
        .expect("roster after supersession");
    let previous = seats
        .iter()
        .find(|seat| seat.id.as_str() == "pij-first")
        .expect("the predecessor is TOMBSTONED, never deleted");
    assert!(previous.tombstoned_at.is_some());
    assert!(
        previous
            .tombstone_reason
            .as_deref()
            .expect("a reason")
            .contains("pij-second"),
        "the post-mortem must name the successor"
    );
    assert!(seats.iter().any(|seat| seat.id.as_str() == "pij-second"));

    // A predecessor that does not EXIST is refused by name. Review round 2 found
    // this arm returning `None` — a silent downgrade to a plain registration,
    // which the PM's round-1 commit message claimed to have fixed and had not.
    // The claim was in the receipt; the change was not in the code.
    let ghost = serde_json::json!({
        "id": "pij-ghost-successor",
        "harness": "omp",
        "folder": "/abs/tree",
        "pid": pid,
        "proc_start": proc_start,
        "supersedes": "pij-never-registered",
    });
    let response = client
        .post(&url)
        .bearer_auth("key")
        .json(&ghost)
        .send()
        .await
        .expect("ghost predecessor");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "an unknown predecessor must be REFUSED, never silently ignored"
    );
    assert!(
        response
            .text()
            .await
            .expect("body")
            .contains("is not in the roster")
    );
    assert!(
        !registry
            .list(SeatFilter::default())
            .await
            .expect("roster")
            .iter()
            .any(|seat| seat.id.as_str() == "pij-ghost-successor"),
        "a refused registration must not write a seat"
    );

    // ...and #19 still holds: naming another seat that does not share this
    // process is refused, which is the case the guard exists for.
    let impostor = serde_json::json!({
        "id": "pij-third",
        "harness": "omp",
        "folder": "/abs/tree",
        "pid": pid,
        "proc_start": proc_start,
        "supersedes": "pij-stranger",
    });
    let response = client
        .post(&url)
        .bearer_auth("key")
        .json(&impostor)
        .send()
        .await
        .expect("impostor");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .expect("body")
            .contains("may only supersede itself")
    );

    server.abort();
}

#[tokio::test]
async fn two_daemons_forward_remote_send_into_destination_queue_and_spine() {
    let b_registry = Arc::new(FakeRegistry::new());
    let mut b_recipient = SeatDescriptor::new("bob", Harness::Omp, "/b");
    b_recipient.proc = Some(ProcIdentity {
        pid: 42,
        proc_start: 20260830000000,
    });
    b_registry
        .put(b_recipient)
        .await
        .expect("register bound destination");
    let b_queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let b_spine = Arc::new(FakeSpine::new());
    let b_services = test_services(b_registry, b_queue.clone(), b_spine.clone()).await;
    let (b_addr, b_server) = spawn(router_with_config(
        b_services,
        HttpConfig {
            local_key: "b-local".to_string(),
            peer_keys: vec!["pair-key".to_string()],
            machine_alias: "laptop".to_string(),
        },
    ))
    .await;

    let a_queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let a_spine = Arc::new(FakeSpine::new());
    let a_services = test_services(Arc::new(FakeRegistry::new()), a_queue, a_spine.clone()).await;
    let federation = Arc::new(
        FederationService::new(
            "desktop".to_string(),
            [PeerDefinition {
                alias: "laptop".to_string(),
                url: format!("http://{b_addr}"),
                key: "pair-key".to_string(),
            }],
            Arc::clone(&a_services.queue),
            Arc::clone(&a_services.event_bus),
            federation_policy(),
        )
        .expect("federation"),
    );
    let (a_addr, a_server) = spawn(router_with_federation(
        a_services,
        config("a-local", &[]),
        federation.clone(),
    ))
    .await;

    let request = SendRequest {
        fyi: false,
        force: false,
        reason: None,
        from: "bob".into(),
        to: Destination {
            seat: "bob".into(),
            machine: Some("laptop".to_string()),
        },
        body: "across machines".to_string(),
        msg_id: "remote-m-1".to_string(),
        from_machine: None,
        in_reply_to: None,
    };
    let response = reqwest::Client::new()
        .post(format!("http://{a_addr}/v1/send"))
        .bearer_auth("a-local")
        .json(&request)
        .send()
        .await
        .expect("remote admission");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let admitted: Envelope<Receipt> = response.json().await.expect("queued receipt");
    assert_eq!(
        admitted.data.expect("receipt").outcome,
        pij_core::model::DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        },
        "the origin observed only its own durable queue"
    );
    let origin_events_before = a_spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .expect("origin admission event");
    assert_eq!(origin_events_before.len(), 1);
    assert!(
        origin_events_before[0]
            .payload
            .contains("\"outcome\":\"queued\"")
    );

    assert_eq!(
        federation.process_one().await.expect("forward"),
        WorkerStep::Forwarded
    );
    let origin_events = a_spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .expect("origin receipt events");
    assert_eq!(origin_events.len(), 2);
    assert!(
        origin_events[1].payload.contains("\"outcome\":\"queued\""),
        "the peer's queued receipt is relayed unchanged"
    );
    assert!(!origin_events[1].payload.contains("injected-to-transport"));

    let kinds = [pij_core::delivery::delivery_kind(&"bob".into())];
    let (_, queued) = b_queue
        .claim(&kinds, "assertion")
        .await
        .expect("destination queue")
        .expect("message landed");
    let message: pij_core::model::Msg =
        serde_json::from_str(&queued.payload).expect("destination payload");
    assert_eq!(message.body, "across machines");
    assert_eq!(message.from_machine.as_deref(), Some("desktop"));

    let events = b_spine
        .tail(None, pij_core::model::Seq(0))
        .await
        .expect("destination spine");
    assert!(events.iter().any(|event| event.kind == "message.pushed"));
    assert!(events.iter().any(|event| event.kind == "delivery.outcome"));
    let pushed = events
        .iter()
        .find(|event| event.kind == "message.pushed")
        .expect("pushed event");
    assert!(pushed.payload.contains("\"from_machine\":\"desktop\""));

    a_server.abort();
    b_server.abort();
}

#[tokio::test]
async fn wrong_peer_key_is_permanent_and_never_hot_loops() {
    let b_services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (b_addr, b_server) = spawn(router_with_config(
        b_services,
        HttpConfig {
            local_key: "b-local".to_string(),
            peer_keys: vec!["correct-key".to_string()],
            machine_alias: "laptop".to_string(),
        },
    ))
    .await;

    let a_queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let a_bus =
        Arc::new(crate::events::EventBus::new(Arc::new(FakeSpine::new()), 16).expect("event bus"));
    let federation = FederationService::new(
        "desktop".to_string(),
        [PeerDefinition {
            alias: "laptop".to_string(),
            url: format!("http://{b_addr}"),
            key: "wrong-key".to_string(),
        }],
        a_queue.clone(),
        a_bus,
        federation_policy(),
    )
    .expect("federation");
    federation
        .enqueue_remote(&SendRequest {
            fyi: false,
            force: false,
            reason: None,
            from: "alice".into(),
            to: Destination {
                seat: "bob".into(),
                machine: Some("laptop".to_string()),
            },
            body: "hello".to_string(),
            msg_id: "auth-m-1".to_string(),
            from_machine: None,
            in_reply_to: None,
        })
        .await
        .expect("enqueue");
    assert_eq!(
        federation.process_one().await.expect("worker"),
        WorkerStep::Refused
    );
    assert!(a_queue.retried().is_empty(), "auth is not retryable");
    let acked = a_queue.acked();
    let [(_, Outcome::Failed { reason })] = acked.as_slice() else {
        panic!("one terminal refusal expected");
    };
    assert!(reason.contains("missing or wrong bearer token"));

    b_server.abort();
}

#[tokio::test]
async fn peer_key_removal_takes_effect_when_router_restarts() {
    let client = reqwest::Client::new();
    let first_services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (first_addr, first_server) = spawn(router_with_config(
        first_services,
        config("local", &["revoked-key"]),
    ))
    .await;
    assert_eq!(
        client
            .get(format!("http://{first_addr}/health"))
            .bearer_auth("revoked-key")
            .send()
            .await
            .expect("accepted before restart")
            .status(),
        reqwest::StatusCode::OK
    );
    first_server.abort();

    let restarted_services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (restarted_addr, restarted_server) =
        spawn(router_with_config(restarted_services, config("local", &[]))).await;
    assert_eq!(
        client
            .get(format!("http://{restarted_addr}/health"))
            .bearer_auth("revoked-key")
            .send()
            .await
            .expect("refused after restart")
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    restarted_server.abort();
}

/// Review round 2 — the memorable allocator has BEHAVIOURAL coverage at the
/// surface it changed, not only a clap-parse test.
///
/// The candidate order is deterministic per seed, so occupancy is the only reason
/// to advance: pre-register the first candidate and the allocator must return the
/// second. A mutation that returns the first candidate unconditionally fails here.
#[tokio::test]
async fn the_memorable_allocator_advances_past_an_occupied_name() {
    let registry = Arc::new(FakeRegistry::new());
    let seed = "s-review-round-2";
    let mut candidates = pij_core::names::memorable_pij_id_candidates(seed);
    let first = candidates.next().expect("a first candidate");
    let second = candidates.next().expect("a second candidate");
    assert_ne!(first, second);

    assert_eq!(
        super::allocate_memorable_id(registry.as_ref(), seed)
            .await
            .expect("allocation"),
        Some(first.clone()),
        "an empty roster yields the first candidate"
    );

    registry
        .put(SeatDescriptor::new(
            first.clone(),
            Harness::Omp,
            "/abs/tree",
        ))
        .await
        .expect("occupy the first candidate");

    assert_eq!(
        super::allocate_memorable_id(registry.as_ref(), seed)
            .await
            .expect("allocation"),
        Some(second),
        "an occupied name must ADVANCE, not collide and not refuse"
    );

    // ...and the shape a human reads is the memorable one, not hex.
    assert!(
        first.as_str().starts_with("pij-"),
        "allocated ids keep the product's name shape: {first}"
    );
}

async fn spawn_at(addr: SocketAddr, router: Router) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind fixed address");
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("serve fixed address");
    })
}

fn peer_definition(alias: &str, addr: SocketAddr) -> PeerDefinition {
    PeerDefinition {
        alias: alias.to_string(),
        url: format!("http://{addr}"),
        key: "pair-key".to_string(),
    }
}

#[tokio::test]
async fn roster_loop_survives_one_peer_error_and_processes_the_next_poll() {
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve peer address");
    let b_addr = reservation.local_addr().expect("peer address");
    drop(reservation);

    let a_registry = Arc::new(FakeRegistry::new());
    a_registry
        .put(SeatDescriptor::new("alice", Harness::Omp, "/desktop"))
        .await
        .expect("local alice");
    let a_services = test_services(
        a_registry,
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let federation = Arc::new(
        FederationService::new(
            "desktop".to_string(),
            [peer_definition("laptop", b_addr)],
            Arc::clone(&a_services.queue),
            Arc::clone(&a_services.event_bus),
            federation_policy(),
        )
        .expect("federation"),
    );
    let worker = Arc::clone(&federation).start();
    let (a_addr, a_server) = spawn(router_with_federation(
        a_services,
        HttpConfig {
            local_key: "a-local".to_string(),
            peer_keys: vec!["pair-key".to_string()],
            machine_alias: "desktop".to_string(),
        },
        federation,
    ))
    .await;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let response = reqwest::Client::new()
                .get(format!("http://{a_addr}/v1/seats"))
                .bearer_auth("a-local")
                .send()
                .await
                .expect("degraded roster response");
            let roster: Envelope<FederatedRoster> = response.json().await.expect("roster");
            let roster = roster.data.expect("roster data");
            if roster
                .unavailable
                .iter()
                .any(|peer| peer.machine == "laptop" && peer.reason.contains("failed"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first failed poll becomes typed degraded state");

    let b_registry = Arc::new(FakeRegistry::new());
    b_registry
        .put(SeatDescriptor::new("alice", Harness::Claude, "/laptop"))
        .await
        .expect("remote alice");
    let b_services = test_services(
        b_registry,
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let b_server = spawn_at(
        b_addr,
        router_with_config(
            b_services,
            HttpConfig {
                local_key: "b-local".to_string(),
                peer_keys: vec!["pair-key".to_string()],
                machine_alias: "laptop".to_string(),
            },
        ),
    )
    .await;

    let roster = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let response = reqwest::Client::new()
                .get(format!("http://{a_addr}/v1/seats"))
                .bearer_auth("a-local")
                .send()
                .await
                .expect("federated roster response");
            let roster: Envelope<FederatedRoster> = response.json().await.expect("roster");
            let roster = roster.data.expect("roster data");
            if roster.unavailable.is_empty() && roster.seats.len() == 2 {
                break roster;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the NEXT poll must succeed after one transient error");
    assert_eq!(
        roster
            .seats
            .iter()
            .map(|seat| (seat.id.as_str(), seat.machine.as_deref()))
            .collect::<Vec<_>>(),
        [("alice", Some("desktop")), ("alice", Some("laptop")),]
    );

    let source_only = reqwest::Client::new()
        .get(format!("http://{a_addr}/v1/seats?scope=local"))
        .bearer_auth("pair-key")
        .send()
        .await
        .expect("source-only roster");
    let source_only: Envelope<FederatedRoster> = source_only
        .json()
        .await
        .expect("source-only roster envelope");
    let source_only = source_only.data.expect("source-only data");
    assert_eq!(
        source_only.seats.len(),
        1,
        "source reads never reflect peer views"
    );
    assert_eq!(source_only.seats[0].machine.as_deref(), Some("desktop"));

    b_server.abort();
    let _ = b_server.await;
    let stale = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let response = reqwest::Client::new()
                .get(format!("http://{a_addr}/v1/seats"))
                .bearer_auth("a-local")
                .send()
                .await
                .expect("stale roster response");
            let roster: Envelope<FederatedRoster> = response.json().await.expect("roster");
            let roster = roster.data.expect("roster data");
            if !roster.unavailable.is_empty() {
                break roster;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("peer drop becomes typed unavailable state");
    assert_eq!(
        stale.seats.len(),
        2,
        "last successful remote roster is retained"
    );
    assert_eq!(stale.unavailable[0].machine, "laptop");

    worker.shutdown().await.expect("worker shutdown");
    a_server.abort();
}

#[tokio::test]
async fn event_loop_survives_one_stream_error_and_processes_the_next_frame() {
    #[derive(Clone)]
    struct FlakyPeer {
        calls: Arc<AtomicUsize>,
        queries: Arc<Mutex<Vec<Option<String>>>>,
    }

    #[derive(Deserialize)]
    struct FlakyQuery {
        since: Option<String>,
    }

    async fn finite_stream(
        State(state): State<FlakyPeer>,
        Query(query): Query<FlakyQuery>,
        headers: HeaderMap,
    ) -> Response {
        assert_eq!(
            headers[axum::http::header::AUTHORIZATION],
            "Bearer pair-key"
        );
        let call = state.calls.fetch_add(1, Ordering::SeqCst);
        state.queries.lock().expect("query mutex").push(query.since);
        let cursor = u64::try_from(call).unwrap_or(u64::MAX).saturating_add(1);
        let frame = StreamFrame::Event {
            machine: "laptop".to_string(),
            cursor,
            event: Event {
                seq: None,
                v: wire::EVENT_VERSION,
                at: cursor,
                kind: format!("future.{cursor}"),
                seat: None,
                payload: cursor.to_string(),
            },
        };
        let body = format!(
            "{}{}\n",
            wire::encode_hello("flaky-peer").expect("hello"),
            serde_json::to_string(&frame).expect("frame")
        );
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
            body,
        )
            .into_response()
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(Mutex::new(Vec::new()));
    let (b_addr, b_server) = spawn(
        Router::new()
            .route("/v1/events", get(finite_stream))
            .with_state(FlakyPeer {
                calls: Arc::clone(&calls),
                queries: Arc::clone(&queries),
            }),
    )
    .await;

    let a_services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let federation = Arc::new(
        FederationService::new(
            "desktop".to_string(),
            [peer_definition("laptop", b_addr)],
            Arc::clone(&a_services.queue),
            Arc::clone(&a_services.event_bus),
            federation_policy(),
        )
        .expect("federation"),
    );
    let (a_addr, a_server) = spawn(router_with_federation(
        a_services.clone(),
        HttpConfig {
            local_key: "a-local".to_string(),
            peer_keys: vec!["pair-key".to_string()],
            machine_alias: "desktop".to_string(),
        },
        Arc::clone(&federation),
    ))
    .await;
    let endpoint = PeerEndpoint {
        base_url: format!("http://{a_addr}"),
        bearer_key: "a-local".to_string(),
    };
    // Attach BEFORE starting the worker so the bounded live view cannot race
    // past either witness frame.
    let mut stream = stream_from_peer(&reqwest::Client::new(), &endpoint, "/v1/events", &[])
        .await
        .expect("open federated view");
    let worker = Arc::clone(&federation).start();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if matches!(
                stream.next_frame().await.expect("first frame"),
                Some(StreamFrame::Event {
                    machine,
                    cursor: 1,
                    event,
                }) if machine == "laptop" && event.kind == "future.1"
            ) {
                break;
            }
        }
    })
    .await
    .expect("A follows B's first event");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if matches!(
                stream.next_frame().await.expect("down frame"),
                Some(StreamFrame::PeerState {
                    machine,
                    state: PeerStreamState::Unavailable,
                    retry_in_ms: Some(_),
                    ..
                }) if machine == "laptop"
            ) {
                break;
            }
        }
    })
    .await
    .expect("the finite first stream's drop is visible in band");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                stream.next_frame().await.expect("reconnect frame"),
                Some(StreamFrame::Event {
                    machine,
                    cursor: 2,
                    event,
                }) if machine == "laptop" && event.kind == "future.2"
            ) {
                break;
            }
        }
    })
    .await
    .expect("NEXT frame is processed after the transient stream failure");

    {
        let queries = queries.lock().expect("query mutex");
        assert_eq!(queries.first(), Some(&None), "first attach is live-only");
        let resumed: BTreeMap<String, u64> = serde_json::from_str(
            queries
                .get(1)
                .and_then(Option::as_deref)
                .expect("reconnect carries a cursor map"),
        )
        .expect("cursor map");
        assert_eq!(resumed, BTreeMap::from([("laptop".to_string(), 1)]));
    }
    assert!(calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        a_services
            .spine
            .tail(None, pij_core::model::Seq(0))
            .await
            .expect("local spine")
            .len(),
        0,
        "remote events are a view and are never appended to the local spine"
    );

    let source_query = [
        ("scope", "local".to_string()),
        ("since", r#"{"laptop":0}"#.to_string()),
    ];
    let mut source_only = stream_from_peer(
        &reqwest::Client::new(),
        &endpoint,
        "/v1/events",
        &source_query,
    )
    .await
    .expect("source-only stream");
    a_services
        .event_bus
        .publish(Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at: 3,
            kind: "future.local".to_string(),
            seat: None,
            payload: "local".to_string(),
        })
        .await
        .expect("local event");
    assert!(matches!(
        source_only.next_frame().await.expect("source frame"),
        Some(StreamFrame::Event {
            machine,
            cursor: 1,
            event,
        }) if machine == "desktop" && event.kind == "future.local"
    ));

    worker.shutdown().await.expect("worker shutdown");
    a_server.abort();
    b_server.abort();
}

/// Review F11 — the RESUME-LIVE RECOVERY, driven through the shipped worker.
///
/// The refusal only matters because of what the worker does next. Without this
/// the refusal is a named retry-forever: the worker would ask again with the same
/// impossible cursor and the peer would refuse again, indefinitely.
///
/// The peer refuses the first attempt with `ErrorKind::CursorReset`, exactly as
/// the shipped `/v1/events` does, then serves a live frame. The worker must
/// publish Reset and come back WITHOUT the stale cursor.
#[tokio::test]
async fn the_event_loop_resumes_live_after_a_peer_refuses_an_impossible_cursor() {
    #[derive(Clone)]
    struct ResettingPeer {
        calls: Arc<AtomicUsize>,
        queries: Arc<Mutex<Vec<Option<String>>>>,
    }

    #[derive(Deserialize)]
    struct ResetQuery {
        since: Option<String>,
    }

    async fn reset_then_serve(
        State(state): State<ResettingPeer>,
        Query(query): Query<ResetQuery>,
    ) -> Response {
        let call = state.calls.fetch_add(1, Ordering::SeqCst);
        state
            .queries
            .lock()
            .expect("query mutex")
            .push(query.since.clone());
        if call == 1 {
            // Second attempt: the peer's history was reset since the first frame,
            // so the cursor the worker now holds is impossible. Refuse BY KIND, as
            // the shipped route does.
            //
            // It must be the SECOND call: the worker only has a cursor to be
            // refused after it has buffered a frame, and a refusal on the very
            // first attempt is indistinguishable from resuming live by accident —
            // which is how the first version of this test passed its own mutation.
            return envelope(
                StatusCode::CONFLICT,
                &Envelope::<()>::refused(
                    "pij events",
                    ErrorKind::CursorReset,
                    "requested cursor 57 is beyond this spine's newest 0",
                ),
            );
        }
        let cursor = if call == 0 { 5 } else { 1 };
        let frame = StreamFrame::Event {
            machine: "laptop".to_string(),
            cursor,
            event: Event {
                seq: None,
                v: wire::EVENT_VERSION,
                at: cursor,
                kind: "after.reset".to_string(),
                seat: None,
                payload: cursor.to_string(),
            },
        };
        let body = format!(
            "{}{}\n",
            wire::encode_hello("reset-peer").expect("hello"),
            serde_json::to_string(&frame).expect("frame")
        );
        (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
            body,
        )
            .into_response()
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(Mutex::new(Vec::new()));
    let (b_addr, b_server) = spawn(
        Router::new()
            .route("/v1/events", get(reset_then_serve))
            .with_state(ResettingPeer {
                calls: Arc::clone(&calls),
                queries: Arc::clone(&queries),
            }),
    )
    .await;

    let a_services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let federation = Arc::new(
        FederationService::new(
            "desktop".to_string(),
            [peer_definition("laptop", b_addr)],
            Arc::clone(&a_services.queue),
            Arc::clone(&a_services.event_bus),
            federation_policy(),
        )
        .expect("federation"),
    );
    let worker = Arc::clone(&federation).start();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if calls.load(Ordering::SeqCst) >= 3 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the worker must RECONNECT after a refusal, not retry it for ever");

    let asked = queries.lock().expect("query mutex").clone();
    assert!(
        asked.len() >= 3,
        "expected serve, refusal, and a resumed retry, saw {asked:?}"
    );
    assert!(
        asked[1].is_some(),
        "the second attempt must carry the cursor the first frame established, or \
         this test proves nothing about resuming. Saw {:?}",
        asked[1]
    );
    assert!(
        asked[2].is_none(),
        "after a refusal the worker must resume LIVE — asking again with the \
         refused cursor is the retry-forever the refusal exists to end. Saw {:?}",
        asked[2]
    );

    worker.shutdown().await.expect("shutdown");
    b_server.abort();
}

/// Review round 4 — a re-raised refusal carries the peer's OWN numbers.
///
/// The client used to rebuild `CursorBeyondSpine` with fabricated zeroes, so an
/// operator read "requested cursor 0 is beyond this spine's newest 0": the right
/// diagnosis carrying evidence nobody measured. Parsing them back out of the
/// message was the alternative, and this project had just finished removing the
/// last place that branched on wording.
///
/// Mutation witness: drop the `data` field from the refusal, or restore the
/// hardcoded zeroes in the client, and this fails.
#[tokio::test]
async fn a_reraised_cursor_refusal_carries_the_peers_own_numbers() {
    async fn refuse(State(()): State<()>) -> Response {
        let mut refusal = Envelope::<CursorResetDetail>::refused(
            "pij events",
            ErrorKind::CursorReset,
            "requested cursor 57 is beyond this spine's newest 4",
        );
        refusal.data = Some(CursorResetDetail {
            requested: 57,
            newest: 4,
        });
        envelope(StatusCode::CONFLICT, &refusal)
    }

    let (addr, server) = spawn(
        Router::new()
            .route("/v1/events", get(refuse))
            .with_state(()),
    )
    .await;
    let endpoint = PeerEndpoint {
        base_url: format!("http://{addr}"),
        bearer_key: "key".to_string(),
    };

    let attempt = stream_from_peer(&reqwest::Client::new(), &endpoint, "/v1/events", &[]).await;
    let Err(error) = attempt else {
        panic!("the peer refused, so opening the stream must fail");
    };

    match error {
        pij_core::error::PijError::CursorBeyondSpine { requested, newest } => {
            assert_eq!(
                (requested, newest),
                (57, 4),
                "the operator must see the numbers the PEER measured, not zeroes we invented"
            );
        }
        other => panic!("expected a typed cursor refusal, got {other}"),
    }

    server.abort();
}

// ---------------------------------------------------------------------------
// `state <id>` readback (plan 114, u-readback)
// ---------------------------------------------------------------------------
//
// Shape derived from the TS surface, not from the packet:
//   `.pi/extensions/pij/core/cli.ts:3615` — the `case "state"` handler,
//   `:3630-3684` — the `--json` object every caller parses today,
//   `:3713` — the human line, headlined "liveness + working/idle" (`cli.ts:354`).
//
// Deliberately NOT mirrored field-for-field. TS emits `lifecycle`, `bindHealth`,
// `degraded`, `daemonTickStale` and `watchdog`; rs models none of them. Emitting
// them as `null` would answer a question rs never asked — the caller could not
// tell "this seat has no watchdog" from "this store has no such concept", which
// is exactly the collapse that makes a read surface lie. So rs names what it
// cannot honour in `unsupported[]` instead, which is ac-1144's refuse-by-name
// rule applied to a READ.

/// The seat every readback test reads back.
fn readback_seat() -> SeatDescriptor {
    let mut seat = SeatDescriptor::new("pij-b", Harness::Claude, "/abs/tree");
    seat.pane = Some("%7".to_string());
    seat.proc = Some(ProcIdentity {
        pid: 4242,
        proc_start: 99,
    });
    seat.state = pij_core::model::SystemState::Working;
    seat.model = Some("claude-opus-5".to_string());
    seat.parent = Some("pij-parent".into());
    seat.harness_session = Some("claude-native-7".to_string());
    seat
}

/// RED WITNESS for u-readback: a `state <id>` round-trip through rs.
///
/// Impossible before this unit — `/v1/state` is not a route, so the request dies
/// in axum's fallback with a bare 404 and no envelope. That is the whole point of
/// the unit, and it is a chronological red: it fails on the tree as merged, for
/// the reason the plan says it fails (rs cannot read a card back), not because
/// something did not compile.
#[tokio::test]
async fn state_reads_one_seat_back_by_id() {
    let registry = Arc::new(FakeRegistry::new().with_seat(readback_seat()));
    let mut services = test_services(
        registry,
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
        pid: 4242,
        proc_start: 99,
    }));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    // POST with the shim's own body shape. RULED (PM, S1): the route table's
    // `state` row is POST, because wave 1's generic call path sends `{ argv }`
    // ONLY on the POST branch — a GET row reaches rs as a bare
    // `fetch(url, { headers })` (adapters/generation-router.ts:199-207) and the
    // `<id>`, this verb's only argument, never crosses the seam.
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "argv": ["state", "pij-b"] }))
        .send()
        .await
        .expect("state request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = body.data.expect("state data");

    assert_eq!(card["id"], "pij-b");
    assert_eq!(card["state"], "working");
    assert_eq!(card["cwd"], "/abs/tree");
    assert_eq!(card["harness"], "claude");
    assert_eq!(card["pid"], 4242);
    assert_eq!(card["parent"], "pij-parent");
    assert_eq!(card["boundModel"], "claude-opus-5");
    assert_eq!(card["liveness"], "active");
    assert_eq!(card["session"], "claude-native-7");
    assert_eq!(card["generation"], "rs");

    server.abort();
}

/// THE BINDING RULE, pinned: a genuine refusal must never wear route-absence's
/// clothes.
///
/// Wave 1's shim reads `404/405 with an undecodable body` as "rs does not
/// implement this verb" and falls back to legacy
/// (`.pi/extensions/pij/core/generation-routing.ts:353-363`). So if this route
/// answered an unknown seat with a bare 404, the caller would not be told the
/// seat is absent — it would be silently served from the OTHER store, which is
/// the split-brain the plan exists to prevent, arriving through the error path.
///
/// This test therefore asserts what the SHIM asserts, in the shim's own terms:
/// the status is a 404 AND the body decodes as a pij envelope. Asserting only
/// "status is 404" would pass just as happily with an empty body, which is
/// exactly the failure.
#[tokio::test]
async fn state_refusal_is_not_classifiable_as_route_absence() {
    let services = test_services(
        Arc::new(FakeRegistry::new()),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "argv": ["state", "pij-absent"] }))
        .send()
        .await
        .expect("state request");
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    let body = response.text().await.expect("body");
    assert!(
        !body.trim().is_empty(),
        "an empty-bodied 404 is what the shim reads as route-absence"
    );
    let decoded: Envelope<serde_json::Value> =
        serde_json::from_str(&body).expect("refusal must decode as a pij envelope");
    assert!(!decoded.ok);
    assert_eq!(decoded.error, Some(ErrorKind::NotFound));
    assert_eq!(decoded.command, "pij state");
    assert!(decoded.data.is_none());

    server.abort();
}

/// A body naming no seat is refused BY NAME, not answered with a guess.
///
/// The tempting alternative — defaulting to some ambient or first seat — would
/// answer a question nobody asked and look like a success.
#[tokio::test]
async fn state_without_a_seat_refuses_by_name() {
    let services = test_services(
        Arc::new(FakeRegistry::new().with_seat(readback_seat())),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "argv": ["state", "--json"] }))
        .send()
        .await
        .expect("state request");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let decoded: Envelope<serde_json::Value> = response.json().await.expect("refusal envelope");
    assert!(!decoded.ok);
    assert_eq!(decoded.error, Some(ErrorKind::Refused));

    server.abort();
}

/// A RECYCLED pid must not read as a live seat — and must not read as `dead`
/// either.
///
/// The pid is alive; it just belongs to a different process now. `active` is the
/// original bug; `dead` is the nearest existing TS word for a claim rs does not
/// make. rs emits a fifth word instead, and this pins that it does.
#[tokio::test]
async fn state_reports_a_recycled_pid_as_neither_active_nor_dead() {
    let mut services = test_services(
        Arc::new(FakeRegistry::new().with_seat(readback_seat())),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    // Same pid, DIFFERENT start time: the OS reused 4242.
    services.liveness = Arc::new(FakeLiveness::new().with_recycled(4242, 12_345));
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "id": "pij-b" }))
        .send()
        .await
        .expect("state request");
    let body: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = body.data.expect("state data");
    assert_eq!(card["liveness"], "recycled");
    // The recorded half of the identity is on the card, so a reader can see WHY.
    assert_eq!(card["procStart"], 99);

    server.abort();
}

/// A seat that never had a process is `unbound`, not `dead`.
#[tokio::test]
async fn state_reports_a_seat_with_no_process_as_unbound() {
    let mut seat = readback_seat();
    seat.proc = None;
    let services = test_services(
        Arc::new(FakeRegistry::new().with_seat(seat)),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "id": "pij-b" }))
        .send()
        .await
        .expect("state request");
    let body: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = body.data.expect("state data");
    assert_eq!(card["liveness"], "unbound");
    assert!(card["pid"].is_null());

    server.abort();
}

async fn state_card_with_source(
    seat: SeatDescriptor,
    source: Arc<pij_testkit::fakes::FakeSessionStatus>,
) -> serde_json::Value {
    let mut services = test_services(
        Arc::new(FakeRegistry::new().with_seat(seat)),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    services.session_status = source;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "id": "pij-b" }))
        .send()
        .await
        .expect("state request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    server.abort();
    body.data.expect("state data")
}

/// Plan 157: the card carries the bound session's facts, and the daemon derives
/// cache state on its own clock. An unknown fact stays unknown on the wire.
#[tokio::test]
async fn state_reads_the_bound_sessions_facts_and_derives_cache_state() {
    use pij_core::session_status::{CacheTtl, Fact, SeatStatus, SessionStatusReply};
    let status = SeatStatus {
        model: Fact::native("claude-opus-5".to_string()),
        context_used_tokens: Fact::derived(142_000),
        // Epoch ms 1 with a 5-minute TTL has long expired at any real clock.
        last_call_at_ms: Fact::native(1),
        cache_ttl: Fact::derived(CacheTtl::FiveMinutes),
        ..SeatStatus::unknown()
    };
    let source = Arc::new(
        pij_testkit::fakes::FakeSessionStatus::new()
            .with_reply("claude-native-7", SessionStatusReply::Status(status)),
    );
    let card = state_card_with_source(readback_seat(), source.clone()).await;

    assert_eq!(source.calls(), ["status:pij-b:claude:claude-native-7"]);
    let block = &card["sessionStatus"];
    assert_eq!(block["outcome"], "known");
    assert_eq!(
        block["status"]["contextUsedTokens"],
        serde_json::json!({"value": 142_000, "basis": "derived"})
    );
    assert_eq!(
        block["status"]["contextWindowTokens"],
        serde_json::json!({"basis": "unknown"})
    );
    assert_eq!(block["cacheState"]["basis"], "derived");
    assert_eq!(block["cacheState"]["value"]["state"], "cold");
    assert!(
        block["cacheState"]["value"]["expiredForMs"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(block["elapsedMs"].is_u64());
    // Busy/idle stays the daemon's own observation.
    assert_eq!(card["state"], "working");
}

/// A failing source fills the block and leaves the rest of the card intact.
#[tokio::test]
async fn state_survives_a_session_status_source_failure() {
    let source = Arc::new(
        pij_testkit::fakes::FakeSessionStatus::new()
            .with_failure("claude-native-7", "transcript unreadable"),
    );
    let card = state_card_with_source(readback_seat(), source).await;

    assert_eq!(card["sessionStatus"]["outcome"], "failed");
    assert!(
        card["sessionStatus"]["error"]
            .as_str()
            .unwrap()
            .contains("transcript unreadable")
    );
    assert_eq!(card["id"], "pij-b");
    assert_eq!(card["state"], "working");
}

/// A seat with no harness session is `unbound`, and the source is never asked.
#[tokio::test]
async fn state_does_not_read_a_session_for_a_seat_with_none_bound() {
    let mut seat = readback_seat();
    seat.harness_session = None;
    let source = Arc::new(pij_testkit::fakes::FakeSessionStatus::new());
    let card = state_card_with_source(seat, source.clone()).await;

    assert_eq!(
        card["sessionStatus"],
        serde_json::json!({"outcome": "unbound"})
    );
    assert!(source.calls().is_empty());
}

/// A field rs cannot answer is NAMED, never nulled.
///
/// The discriminating arm is the second assertion, and it is the whole point: a
/// card carrying `"bindHealth": null` would satisfy any test that merely looked
/// for the key, while telling a caller "we looked and this seat has none" —
/// which rs never looked for and cannot say. Absent-from-the-card AND
/// present-in-`unsupported` is the only shape that separates the two states.
#[tokio::test]
async fn state_names_what_it_cannot_answer_instead_of_nulling_it() {
    let services = test_services(
        Arc::new(FakeRegistry::new().with_seat(readback_seat())),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({ "id": "pij-b" }))
        .send()
        .await
        .expect("state request");
    let body: Envelope<serde_json::Value> = response.json().await.expect("state envelope");
    let card = body.data.expect("state data");

    let named: Vec<&str> = card["unsupported"]
        .as_array()
        .expect("unsupported list")
        .iter()
        .map(|entry| entry["field"].as_str().expect("field name"))
        .collect();

    for field in [
        "lifecycle",
        "activity",
        "ageMs",
        "liveness:stale",
        "bindHealth",
        "degraded",
        "watchdog",
        "terminal",
        "failureReason",
    ] {
        assert!(
            named.contains(&field),
            "{field} must be named as unsupported"
        );
        assert!(
            card.get(field).is_none(),
            "{field} must be ABSENT from the card — emitting it as null answers a question rs never asked"
        );
    }

    // Every named gap carries a reason; a bare list of names is not an answer.
    for entry in card["unsupported"].as_array().expect("unsupported list") {
        assert!(
            !entry["why"].as_str().expect("why").trim().is_empty(),
            "an unsupported field with no reason is a gap nobody can act on"
        );
    }

    server.abort();
}

/// The shim forwards `process.argv.slice(2)` verbatim, so the leading token is
/// the verb and flags ride along. The seat is the first token that is neither.
#[test]
fn state_request_takes_the_seat_from_shim_argv_and_prefers_an_explicit_id() {
    let from_argv = StateRequest {
        id: None,
        argv: Some(vec![
            "state".to_string(),
            "--json".to_string(),
            "pij-b".to_string(),
        ]),
    };
    assert_eq!(from_argv.seat(), Some(SeatId::from("pij-b")));

    let explicit = StateRequest {
        id: Some(SeatId::from("pij-native")),
        argv: Some(vec!["state".to_string(), "pij-shim".to_string()]),
    };
    assert_eq!(explicit.seat(), Some(SeatId::from("pij-native")));

    assert_eq!(StateRequest::default().seat(), None);
    assert_eq!(
        StateRequest {
            id: None,
            argv: Some(vec!["state".to_string(), "--json".to_string()]),
        }
        .seat(),
        None
    );
}

/// An additive body field must not 400 the fleet.
///
/// The seam unit in flight adds a `caller` block to every routed body. A handler
/// with `deny_unknown_fields` would turn that additive change into a fleet-wide
/// refusal on a verb that was working — so the tolerance is pinned here rather
/// than left to serde's default staying put.
#[tokio::test]
async fn state_tolerates_a_body_field_it_does_not_know() {
    let services = test_services(
        Arc::new(FakeRegistry::new().with_seat(readback_seat())),
        Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy")),
        Arc::new(FakeSpine::new()),
    )
    .await;
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;

    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/state"))
        .bearer_auth("key")
        .json(&serde_json::json!({
            "argv": ["state", "pij-b"],
            "caller": { "PIJ_SESSION_ID": "pij-caller", "pid": 11, "procStart": 22 }
        }))
        .send()
        .await
        .expect("state request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    server.abort();
}

// --- Plan 157 phase 2: the cold-wake guard -------------------------------

const COLD_SESSION: &str = "claude-cold-session";

fn cold_seat(state: pij_core::model::SystemState) -> SeatDescriptor {
    // Unbound, so a send that passes the guard queues (pre-bind) and never
    // touches a transport.
    let mut seat = SeatDescriptor::new("pij-cold", Harness::Claude, "/abs/tree");
    seat.harness_session = Some(COLD_SESSION.to_string());
    seat.state = state;
    seat
}

/// 720k tokens on Opus 5.5, last called two hours ago.
fn cold_status() -> pij_core::session_status::SessionStatusReply {
    use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    SessionStatusReply::Status(SeatStatus {
        model: Fact::native("claude-opus-5-5".to_string()),
        context_used_tokens: Fact::derived(720_000),
        last_call_at_ms: Fact::native(now - 2 * 60 * 60 * 1_000),
        ..SeatStatus::unknown()
    })
}

async fn cold_daemon(
    state: pij_core::model::SystemState,
    source: pij_testkit::fakes::FakeSessionStatus,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    Arc<FakeQueue>,
    Arc<FakeSpine>,
) {
    let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let spine = Arc::new(FakeSpine::new());
    let mut services = test_services(
        Arc::new(
            FakeRegistry::new()
                .with_seat(cold_seat(state))
                .with_seat(SeatDescriptor::new("pij-sender", Harness::Omp, "/abs/tree")),
        ),
        queue.clone(),
        spine.clone(),
    )
    .await;
    services.session_status = Arc::new(source);
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    (addr, server, queue, spine)
}

async fn post_json(
    addr: SocketAddr,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .bearer_auth("key")
        .json(&body)
        .send()
        .await
        .expect("request");
    (
        response.status().as_u16(),
        response.json().await.expect("envelope"),
    )
}

fn cold_send(msg_id: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::json!({
        "from": "pij-sender", "to": {"seat": "pij-cold"}, "body": "wake up", "msg_id": msg_id,
    });
    for (key, value) in extra.as_object().expect("object") {
        body[key] = value.clone();
    }
    body
}

/// A send to a large seat idle past the cache TTL is refused with its price,
/// and nothing is queued or delivered.
#[tokio::test]
async fn a_send_to_a_cold_seat_is_refused_with_its_price() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, queue, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let (status, reply) =
        post_json(addr, "/v1/send", cold_send("m-cold", serde_json::json!({}))).await;
    assert_eq!(status, 400, "{reply}");
    assert_eq!(reply["ok"], false);
    let message = reply["meta"].as_str().unwrap_or_default();
    assert!(
        message.starts_with("E-RS-COLD-WAKE: ❄ pij-cold is cold (720k, idle 2h). Options:")
            && message.contains("~$6.45")
            && message.contains("--fyi")
            && message.contains("--force --reason"),
        "{reply}"
    );
    assert_eq!(queue.live_len(), 0, "a refusal sends nothing");
    server.abort();
}

/// `--force` needs a reason; a forced cold wake is delivered and audited.
#[tokio::test]
async fn a_forced_cold_wake_needs_a_reason_and_is_audited() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, queue, spine) =
        cold_daemon(pij_core::model::SystemState::Idle, source).await;
    for reason in [serde_json::Value::Null, serde_json::json!("   ")] {
        let (status, reply) = post_json(
            addr,
            "/v1/send",
            cold_send(
                "m-force",
                serde_json::json!({"force": true, "reason": reason}),
            ),
        )
        .await;
        assert_eq!(status, 400, "{reply}");
        assert!(
            reply["meta"]
                .as_str()
                .unwrap_or_default()
                .contains("--reason"),
            "{reply}"
        );
    }
    assert_eq!(queue.live_len(), 0);

    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send(
            "m-force",
            serde_json::json!({"force": true, "reason": "prod is down, need its context"}),
        ),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "forced");
    assert_eq!(queue.live_len(), 1);
    let audit = spine
        .tail(None, Seq(0))
        .await
        .expect("tail")
        .into_iter()
        .find(|event| event.kind == "send.cold-wake-forced")
        .expect("forced cold wake is audited");
    let payload: serde_json::Value = serde_json::from_str(&audit.payload).unwrap();
    assert_eq!(audit.seat, Some("pij-cold".into()));
    assert_eq!(payload["reason"], "prod is down, need its context");
    assert_eq!(payload["from"], "pij-sender");
    assert_eq!(payload["msg_id"], "m-force");
    assert_eq!(payload["context_tokens"], 720_000);
    // The audited estimate is the wake price the refusal shows (plan 160).
    assert_eq!(
        format!("{:.2}", payload["estimate_usd"].as_f64().unwrap()),
        "6.45"
    );
    server.abort();
}

/// A busy seat is warm by definition; unknown facts allow and say so.
#[tokio::test]
async fn busy_and_unknown_recipients_are_never_refused() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, _, _) = cold_daemon(pij_core::model::SystemState::Working, source).await;
    let (status, reply) =
        post_json(addr, "/v1/send", cold_send("m-busy", serde_json::json!({}))).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    server.abort();

    // Unscripted fake source: the harness is unsupported.
    let (addr, server, _, _) = cold_daemon(
        pij_core::model::SystemState::Idle,
        pij_testkit::fakes::FakeSessionStatus::new(),
    )
    .await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-unknown", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["data"]["cold_check"],
        "unknown: claude sessions are not readable"
    );
    server.abort();
}

/// FYIs never wake a seat, so they are never refused; controls are not guarded.
#[tokio::test]
async fn fyis_and_controls_are_never_refused() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, _, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-fyi", serde_json::json!({"fyi": true})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["outcome"]["outcome"], "held");
    let (_, reply) = post_json(
        addr,
        "/v1/send",
        cold_send(
            "m-ctl",
            serde_json::json!({"body": "", "command": "compact"}),
        ),
    )
    .await;
    assert!(
        !reply.to_string().contains("E-RS-COLD-WAKE"),
        "controls are not guarded: {reply}"
    );
    server.abort();
}

/// The shim's positional `pij send` takes the same guard and the same escape.
#[tokio::test]
async fn the_shim_send_is_guarded_and_forceable() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, queue, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let shim = |argv: &[&str]| serde_json::json!({"argv": argv, "caller": {"PIJ_SESSION_ID": "pij-sender"}});
    let (status, reply) = post_json(
        addr,
        "/v1/shim/send",
        shim(&["send", "pij-cold", "wake up"]),
    )
    .await;
    assert_eq!(status, 400, "{reply}");
    assert!(
        reply["meta"]
            .as_str()
            .unwrap_or_default()
            .starts_with("E-RS-COLD-WAKE"),
        "{reply}"
    );
    let (status, reply) = post_json(
        addr,
        "/v1/shim/send",
        shim(&[
            "send",
            "pij-cold",
            "--force",
            "--reason",
            "needed now",
            "wake up",
        ]),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "forced");
    assert_eq!(queue.live_len(), 1);
    server.abort();
}

/// Review D8: a session source that fails, or does not answer within the
/// guard's wait, allows the send and says why. The brake never blocks on it.
#[tokio::test]
async fn a_failing_or_silent_session_source_allows_the_send() {
    let (addr, server, queue, _) = cold_daemon(
        pij_core::model::SystemState::Idle,
        pij_testkit::fakes::FakeSessionStatus::new().with_failure(COLD_SESSION, "fold exploded"),
    )
    .await;
    let (status, reply) =
        post_json(addr, "/v1/send", cold_send("m-err", serde_json::json!({}))).await;
    assert_eq!(status, 200, "{reply}");
    assert!(
        reply["data"]["cold_check"]
            .as_str()
            .is_some_and(|label| label.starts_with("unknown: source failed:")
                && label.contains("fold exploded")),
        "{reply}"
    );
    assert_eq!(queue.live_len(), 1, "the message was sent");
    server.abort();

    let (addr, server, queue, _) = cold_daemon(
        pij_core::model::SystemState::Idle,
        pij_testkit::fakes::FakeSessionStatus::new().with_hang(COLD_SESSION),
    )
    .await;
    let started = std::time::Instant::now();
    let (status, reply) =
        post_json(addr, "/v1/send", cold_send("m-hang", serde_json::json!({}))).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        reply["data"]["cold_check"],
        "unknown: source failed: no answer within 3s"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(6),
        "bounded wait"
    );
    assert_eq!(queue.live_len(), 1, "the message was sent");
    server.abort();
}

/// Review D1: the shim's own `--fyi` (the surface `pij send --fyi` uses) holds
/// a message for a cold seat instead of refusing it.
#[tokio::test]
async fn a_shim_fyi_to_a_cold_seat_is_held_not_refused() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, _, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let (status, reply) = post_json(
        addr,
        "/v1/shim/send",
        serde_json::json!({"argv": ["send", "pij-cold", "--fyi", "note"], "caller": {"PIJ_SESSION_ID": "pij-sender"}}),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["outcome"]["outcome"], "held", "{reply}");
    server.abort();
}

/// Review D2: a message forwarded from another machine is not guarded here: its
/// sender cannot be offered --fyi or --force by this daemon. Documented bypass.
#[tokio::test]
async fn a_forwarded_send_is_not_guarded() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, queue, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-fwd", serde_json::json!({"from_machine": "laptop"})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply["data"].get("cold_check").is_none(), "{reply}");
    assert_eq!(queue.live_len(), 1);
    server.abort();
}

/// Review ruling (Esc fires no Claude hook): a seat that SAYS working but is
/// large and 60+ min past its last call is checked against its live pane.
async fn stale_working_daemon(
    claude: Arc<dyn pij_core::ports::HarnessPort>,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    Arc<FakeQueue>,
    Arc<FakeSpine>,
) {
    stale_working_daemon_with(claude, cold_status(), Some(TWO_HOURS_MS)).await
}

/// `working_for_ms`: how long ago the seat's `working` was published (`None`:
/// no `seat.activity` fact at all).
async fn stale_working_daemon_with(
    claude: Arc<dyn pij_core::ports::HarnessPort>,
    reply: pij_core::session_status::SessionStatusReply,
    working_for_ms: Option<u64>,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    Arc<FakeQueue>,
    Arc<FakeSpine>,
) {
    let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let spine = Arc::new(FakeSpine::new());
    let mut seat = cold_seat(pij_core::model::SystemState::Working);
    seat.pane = Some("%stale".to_string());
    if let Some(working_for_ms) = working_for_ms {
        let mut working = pij_store::registry::activity_event(
            &seat.id,
            pij_core::model::SystemState::Working,
            None,
        )
        .expect("activity event");
        working.at -= working_for_ms;
        spine.append(working).await.expect("seed working");
    }
    let mut services = test_services(
        Arc::new(FakeRegistry::new().with_seat(seat)),
        queue.clone(),
        spine.clone(),
    )
    .await;
    services.session_status =
        Arc::new(pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, reply));
    services.harnesses = Arc::new(
        pij_harnesses::HarnessRegistry::new([
            claude,
            Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Copilot)),
            Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Codex)),
            Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Pi)),
            Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Omp)),
        ])
        .expect("harness registry"),
    );
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    (addr, server, queue, spine)
}

/// A real Claude adapter over a real tmux adapter whose `tmux` prints `frame`.
async fn real_claude_stale_daemon(
    frame: &str,
    working_for_ms: u64,
    tag: &str,
) -> (
    SocketAddr,
    tokio::task::JoinHandle<()>,
    Arc<FakeQueue>,
    Arc<FakeSpine>,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("pij-frame-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("frame.txt"), frame).expect("frame");
    let tmux_bin = dir.join("tmux");
    std::fs::write(
        &tmux_bin,
        format!("#!/bin/sh\ncat '{}'\n", dir.join("frame.txt").display()),
    )
    .expect("fake tmux");
    std::fs::set_permissions(
        &tmux_bin,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .expect("chmod");
    let tmux: Arc<dyn pij_core::ports::TmuxPort> = Arc::new(pij_tmux::TmuxAdapter::with_binary(
        tmux_bin.as_os_str(),
        dir.join("taps"),
    ));
    let (addr, server, queue, spine) = stale_working_daemon_with(
        Arc::new(pij_harnesses::ClaudeHarness::new(tmux)),
        cold_status(),
        Some(working_for_ms),
    )
    .await;
    (addr, server, queue, spine, dir)
}

const TWO_HOURS_MS: u64 = 2 * 60 * 60 * 1_000;

/// Re-review 3: only POSITIVE idle evidence may refuse and correct. Each of
/// these panes is a working seat whose facts are cold, and none of them shows
/// an idle Claude, so every one must allow and leave `working` alone.
#[tokio::test]
async fn a_pane_without_positive_idle_evidence_allows_and_keeps_working() {
    let rows: [(&str, &str, u64); 6] = [
        ("empty", "", TWO_HOURS_MS),
        ("blank", "\n\n\n\n", TWO_HOURS_MS),
        (
            "truncated-spinner",
            include_str!("../../../testkit/fixtures/harnesses/claude-2.1.284-tool-run-narrow.txt"),
            TWO_HOURS_MS,
        ),
        (
            "permission-dialog",
            include_str!(
                "../../../testkit/fixtures/harnesses/claude-2.1.284-permission-dialog.txt"
            ),
            TWO_HOURS_MS,
        ),
        (
            "tool-run",
            include_str!("../../../testkit/fixtures/harnesses/claude-2.1.284-tool-run.txt"),
            TWO_HOURS_MS,
        ),
        // Live: mid-stream Claude shows no spinner and looks idle. A seat woken
        // after a long idle is in exactly this frame with cold facts, until its
        // first reply is written. Its `working` is seconds old, so it is fresh.
        (
            "streaming-fresh-wake",
            include_str!("../../../testkit/fixtures/harnesses/claude-2.1.284-streaming.txt"),
            10_000,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, frame, working_for_ms) in rows {
        let (addr, server, queue, _, dir) =
            real_claude_stale_daemon(frame, working_for_ms, name).await;
        let (status, reply) = post_json(
            addr,
            "/v1/send",
            cold_send(&format!("m-{name}"), serde_json::json!({})),
        )
        .await;
        let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
        let outcome = (
            status,
            reply["data"]["cold_check"].clone(),
            queue.live_len(),
            card["data"]["state"].clone(),
        );
        if outcome
            != (
                200,
                serde_json::json!("busy"),
                1,
                serde_json::json!("working"),
            )
        {
            wrong.push(format!("{name}: {outcome:?}"));
        }
        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(
        wrong.is_empty(),
        "refused or corrected without idle evidence: {wrong:#?}"
    );
}

/// The positive case end to end: a live Claude frame just after an Esc
/// interrupt, `working` for two hours, cold facts. Refused and corrected.
#[tokio::test]
async fn a_live_idle_claude_frame_under_a_long_stale_working_is_refused() {
    let (addr, server, queue, _, dir) = real_claude_stale_daemon(
        include_str!("../../../testkit/fixtures/harnesses/claude-2.1.284-after-esc.txt"),
        TWO_HOURS_MS,
        "idle",
    )
    .await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-live-idle", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 400, "{reply}");
    assert_eq!(queue.live_len(), 0);
    let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(card["data"]["state"], "idle");
    server.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_stale_working_seat_with_an_idle_pane_is_refused_and_corrected() {
    let claude = Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle(true));
    let (addr, server, queue, spine) = stale_working_daemon(claude.clone()).await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-stale", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 400, "{reply}");
    assert!(
        reply["meta"]
            .as_str()
            .unwrap_or_default()
            .starts_with("E-RS-COLD-WAKE"),
        "{reply}"
    );
    assert_eq!(queue.live_len(), 0);
    assert_eq!(claude.idle_probes(), 1, "exactly one pane probe");
    let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(
        card["data"]["state"], "idle",
        "the stale state is corrected"
    );
    let correction = spine
        .tail(None, Seq(0))
        .await
        .expect("tail")
        .into_iter()
        .rev()
        .find(|event| event.kind == "seat.activity")
        .expect("the correction is on the spine");
    let payload: serde_json::Value = serde_json::from_str(&correction.payload).unwrap();
    assert_eq!(
        payload,
        serde_json::json!({"state": "idle", "reason": "stale working (esc)"})
    );
    server.abort();
}

#[tokio::test]
async fn a_stale_working_seat_with_a_busy_pane_is_allowed() {
    let (addr, server, queue, _) = stale_working_daemon(Arc::new(
        pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle(false),
    ))
    .await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-busy-pane", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    assert_eq!(queue.live_len(), 1);
    let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(card["data"]["state"], "working");
    server.abort();
}

#[tokio::test]
async fn a_failed_pane_capture_trusts_working() {
    let (addr, server, queue, _) = stale_working_daemon(Arc::new(
        pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle_error(),
    ))
    .await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-capture-fail", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    assert_eq!(queue.live_len(), 1);
    server.abort();
}

/// A wedged tmux (a real child that does not answer) cannot hold the send: the
/// probe is abandoned at its bound and `working` is trusted.
/// Multi-threaded, as the daemon runs; the fake-harness tests cover current-thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wedged_pane_capture_is_abandoned_at_its_bound() {
    let dir = std::env::temp_dir().join(format!("pij-wedged-tmux-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let tmux_bin = dir.join("tmux");
    std::fs::write(&tmux_bin, "#!/bin/sh\nsleep 4\n").expect("fake tmux");
    std::fs::set_permissions(
        &tmux_bin,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .expect("chmod");
    let tmux: Arc<dyn pij_core::ports::TmuxPort> = Arc::new(pij_tmux::TmuxAdapter::with_binary(
        tmux_bin.as_os_str(),
        dir.join("taps"),
    ));
    let (addr, server, queue, _) =
        stale_working_daemon(Arc::new(pij_harnesses::ClaudeHarness::new(tmux))).await;
    let started = std::time::Instant::now();
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-wedged", serde_json::json!({})),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    assert_eq!(queue.live_len(), 1);
    assert!(
        elapsed < std::time::Duration::from_millis(2_500),
        "the probe bound held: {elapsed:?}"
    );
    server.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The pane is consulted only in the rare case: a working seat whose facts
/// are NOT cold is trusted as busy without a probe, even if its pane is idle.
#[tokio::test]
async fn a_working_seat_that_is_not_cold_by_its_facts_is_not_probed() {
    use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let recent = SessionStatusReply::Status(SeatStatus {
        context_used_tokens: Fact::derived(720_000),
        last_call_at_ms: Fact::native(now - 5 * 60 * 1_000),
        ..SeatStatus::unknown()
    });
    let claude = Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle(true));
    let (addr, server, _, _) =
        stale_working_daemon_with(claude.clone(), recent, Some(TWO_HOURS_MS)).await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-recent", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(
        card["data"]["state"], "working",
        "not corrected outside the rare case"
    );
    assert_eq!(
        claude.idle_probes(),
        0,
        "no pane probe outside the rare case"
    );
    server.abort();
}

/// A `working` younger than the idle threshold is a live turn, whatever its
/// pane shows (mid-stream Claude looks idle). It is trusted without a probe.
#[tokio::test]
async fn a_recent_working_is_trusted_without_a_pane_probe() {
    let claude = Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle(true));
    let (addr, server, queue, _) =
        stale_working_daemon_with(claude.clone(), cold_status(), Some(30_000)).await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-fresh-wake", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    assert_eq!(queue.live_len(), 1);
    assert_eq!(claude.idle_probes(), 0, "no pane probe for a fresh turn");
    let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(card["data"]["state"], "working");
    server.abort();
}

/// The age that counts is that of the latest activity fact, and only if it
/// says `working`. An old `idle` fact is no evidence of a stale `working`.
#[tokio::test]
async fn an_old_activity_fact_that_is_not_working_is_no_evidence() {
    let claude = Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle(true));
    let (addr, server, queue, spine) =
        stale_working_daemon_with(claude.clone(), cold_status(), Some(3 * 60 * 60 * 1_000)).await;
    let mut idle = pij_store::registry::activity_event(
        &cold_seat(pij_core::model::SystemState::Working).id,
        pij_core::model::SystemState::Idle,
        None,
    )
    .expect("activity event");
    idle.at -= TWO_HOURS_MS;
    spine.append(idle).await.expect("seed idle");
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-old-idle-fact", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(queue.live_len(), 1);
    assert_eq!(claude.idle_probes(), 0);
    server.abort();
}

/// No `seat.activity` fact, or a spine that cannot answer, is no evidence that
/// the `working` is stale: allow, without a pane probe.
#[tokio::test]
async fn a_missing_or_unreadable_activity_fact_allows_without_a_probe() {
    for unreadable in [false, true] {
        let claude =
            Arc::new(pij_testkit::fakes::FakeHarness::new(Harness::Claude).script_idle(true));
        let (addr, server, queue, spine) = if unreadable {
            stale_working_daemon_with(claude.clone(), cold_status(), Some(TWO_HOURS_MS)).await
        } else {
            stale_working_daemon_with(claude.clone(), cold_status(), None).await
        };
        if unreadable {
            spine.script_latest_matching_error("spine read failed");
        }
        let (status, reply) = post_json(
            addr,
            "/v1/send",
            cold_send("m-no-fact", serde_json::json!({})),
        )
        .await;
        assert_eq!(status, 200, "unreadable={unreadable}: {reply}");
        assert_eq!(
            reply["data"]["cold_check"], "busy",
            "unreadable={unreadable}"
        );
        assert_eq!(queue.live_len(), 1, "unreadable={unreadable}");
        assert_eq!(claude.idle_probes(), 0, "unreadable={unreadable}");
        server.abort();
    }
}

/// A host with no `tmux` binary makes the pane probe fail, and a failed probe
/// is unknown: the send is allowed and `working` stands.
#[tokio::test]
async fn a_missing_tmux_binary_is_unknown_and_allows() {
    let dir = std::env::temp_dir().join(format!("pij-no-tmux-{}", std::process::id()));
    let tmux: Arc<dyn pij_core::ports::TmuxPort> = Arc::new(pij_tmux::TmuxAdapter::with_binary(
        dir.join("no-such-tmux").as_os_str(),
        dir.join("taps"),
    ));
    let (addr, server, queue, _) =
        stale_working_daemon(Arc::new(pij_harnesses::ClaudeHarness::new(tmux))).await;
    let (status, reply) = post_json(
        addr,
        "/v1/send",
        cold_send("m-no-tmux", serde_json::json!({})),
    )
    .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["data"]["cold_check"], "busy");
    assert_eq!(queue.live_len(), 1);
    let (_, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(card["data"]["state"], "working");
    server.abort();
}

// ---------------------------------------------------------------------------
// Plan 159 review: the warm flush needs POSITIVE evidence of a warm cache.
// ---------------------------------------------------------------------------

/// Facts for `pij-cold`: last call `ago_ms` ago, with an optional cache TTL.
fn flush_facts(
    ago_ms: u64,
    ttl: Option<pij_core::session_status::CacheTtl>,
) -> pij_testkit::fakes::FakeSessionStatus {
    use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    pij_testkit::fakes::FakeSessionStatus::new().with_reply(
        COLD_SESSION,
        SessionStatusReply::Status(SeatStatus {
            model: Fact::native("claude-opus-5-5".to_string()),
            context_used_tokens: Fact::derived(720_000),
            last_call_at_ms: Fact::native(now - ago_ms),
            cache_ttl: ttl.map_or(Fact::Unknown, Fact::derived),
            ..SeatStatus::unknown()
        }),
    )
}

/// Hold five FYIs for `pij-cold` over `/v1/send`; how many deliveries queued.
async fn five_fyis(addr: SocketAddr, queue: &FakeQueue) -> usize {
    for n in 1..=5 {
        let (status, reply) = post_json(
            addr,
            "/v1/send",
            cold_send(&format!("f-{n}"), serde_json::json!({"fyi": true})),
        )
        .await;
        assert_eq!(status, 200, "{reply}");
        assert_eq!(reply["data"]["outcome"]["outcome"], "held", "{reply}");
    }
    queue.live_len()
}

const FIVE_MIN_MS: u64 = 5 * 60 * 1_000;

#[tokio::test]
async fn five_fyis_flush_only_on_a_last_call_inside_the_cache_lifetime() {
    use pij_core::model::SystemState::Idle;
    use pij_core::session_status::CacheTtl::{FiveMinutes, OneHour};
    let rows = [
        (
            "1h cache, called 5 min ago",
            flush_facts(FIVE_MIN_MS, Some(OneHour)),
            1,
        ),
        (
            "5m cache, called 1 min ago",
            flush_facts(60_000, Some(FiveMinutes)),
            1,
        ),
        // Review finding 4: past its real (5-minute) lifetime, inside 60 min.
        (
            "5m cache, called 30 min ago",
            flush_facts(6 * FIVE_MIN_MS, Some(FiveMinutes)),
            0,
        ),
        (
            "1h cache, called 2 h ago",
            flush_facts(24 * FIVE_MIN_MS, Some(OneHour)),
            0,
        ),
        ("unknown cache lifetime", flush_facts(60_000, None), 0),
    ];
    for (name, source, flushed) in rows {
        let (addr, server, queue, _) = cold_daemon(Idle, source).await;
        assert_eq!(five_fyis(addr, &queue).await, flushed, "{name}");
        for payload in queue.live_payloads() {
            let payload: serde_json::Value = serde_json::from_str(&payload).expect("payload");
            let body = payload["body"].as_str().expect("body");
            assert!(
                body.starts_with("5 FYIs were queued for you:\n1. [from pij-sender, "),
                "{name}: the flush is one message whose body is the block: {body}"
            );
            assert_eq!(payload["from"], pij_core::BG_ACTOR, "{name}");
        }
        server.abort();
    }
}

/// Review finding 1 (the reviewer's probe): `working` alone is no evidence of
/// warmth. A stale `working` (Esc, API error, dead process) on a seat whose
/// facts say cold must not be flushed, and neither must `working` with no facts.
#[tokio::test]
async fn a_working_state_alone_never_flushes() {
    use pij_core::model::SystemState::Working;
    use pij_core::session_status::CacheTtl::OneHour;
    let rows = [
        (
            "stale working, cold 2 h",
            flush_facts(24 * FIVE_MIN_MS, Some(OneHour)),
        ),
        (
            "working, no facts",
            pij_testkit::fakes::FakeSessionStatus::new(),
        ),
    ];
    for (name, source) in rows {
        let (addr, server, queue, _) = cold_daemon(Working, source).await;
        assert_eq!(five_fyis(addr, &queue).await, 0, "{name}");
        server.abort();
    }
}

/// Review W5: session facts that do not arrive in time are unknown, not warm.
#[tokio::test]
async fn facts_that_never_arrive_never_flush() {
    let source = pij_testkit::fakes::FakeSessionStatus::new().with_hang(COLD_SESSION);
    let (addr, server, queue, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    assert_eq!(five_fyis(addr, &queue).await, 0);
    server.abort();
}

/// Review W9: the shim's `--fyi` (what `pij send --fyi` uses) warns on a
/// question and flushes a warm pile, like `/v1/send`.
#[tokio::test]
async fn the_shim_fyi_path_warns_and_flushes() {
    let source = flush_facts(
        FIVE_MIN_MS,
        Some(pij_core::session_status::CacheTtl::OneHour),
    );
    let (addr, server, queue, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    for n in 1..=5 {
        let body = if n == 1 {
            "can you check X?".to_string()
        } else {
            format!("note {n}")
        };
        let (status, reply) = post_json(
            addr,
            "/v1/shim/send",
            serde_json::json!({"argv": ["send", "pij-cold", "--fyi", body], "caller": {"PIJ_SESSION_ID": "pij-sender"}}),
        )
        .await;
        assert_eq!(status, 200, "{reply}");
        assert_eq!(reply["data"]["outcome"]["outcome"], "held", "{reply}");
        assert_eq!(
            reply["data"].get("warning").and_then(|w| w.as_str()),
            (n == 1).then_some(pij_core::fyi::QUESTION_WARNING),
            "{reply}"
        );
    }
    assert_eq!(
        queue.live_len(),
        1,
        "the fifth shim FYI flushes the warm pile"
    );
    server.abort();
}

/// Review finding 2: a turn boundary never flushes, even for a warm seat with a
/// full pile. The turn that is starting carries the pile for free.
#[tokio::test]
async fn a_turn_boundary_never_flushes_even_when_warm() {
    let source = flush_facts(
        FIVE_MIN_MS,
        Some(pij_core::session_status::CacheTtl::OneHour),
    );
    let (addr, server, queue, spine) =
        cold_daemon(pij_core::model::SystemState::Idle, source).await;
    for n in 1..=5 {
        let fyi = pij_core::fyi::HeldFyi {
            id: format!("f-{n}"),
            recipient: SeatId::from("pij-cold"),
            sender: SeatId::from("pij-sender"),
            body: format!("note {n}"),
            held_at_ms: n,
        };
        queue.hold_fyi(&fyi, spine.as_ref()).await.expect("hold");
    }
    for state in ["working", "idle"] {
        let (status, reply) = post_json(
            addr,
            "/v1/activity",
            serde_json::json!({"seat": "pij-cold", "native_session": COLD_SESSION, "state": state}),
        )
        .await;
        assert_eq!(status, 200, "{reply}");
    }
    assert_eq!(queue.live_len(), 0, "no flush on a turn boundary");
    server.abort();
}

// ---------------------------------------------------------------------------
// Plan 160: seat size and coldness on `pij state` and `pij list`.
// ---------------------------------------------------------------------------

async fn get_seats(addr: SocketAddr, query: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("http://{addr}/v1/seats{query}"))
        .bearer_auth("key")
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("envelope")
}

fn row<'a>(reply: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    reply["data"]["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|seat| seat["id"] == id)
        .expect("seat row")
}

/// A cold large seat's card says how big and how cold it is, that the guard
/// would refuse a normal send, and what waking it would cost.
#[tokio::test]
async fn the_state_card_shows_size_coldness_and_the_guard() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, _, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let (status, card) = post_json(addr, "/v1/state", serde_json::json!({"id": "pij-cold"})).await;
    assert_eq!(status, 200, "{card}");
    let data = &card["data"];
    assert_eq!(data["contextUsed"], 720_000);
    let idle = data["idleMs"].as_u64().expect("idleMs");
    assert!(
        (2 * 60 * 60 * 1_000..2 * 60 * 60 * 1_000 + 60_000).contains(&idle),
        "{idle}"
    );
    assert_eq!(data["coldWake"]["wouldRefuse"], true);
    assert_eq!(
        format!("{:.2}", data["coldWake"]["estimateUsd"].as_f64().unwrap()),
        "6.45"
    );
    let lines: Vec<&str> = data["sizeLines"]
        .as_array()
        .expect("sizeLines")
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect();
    assert_eq!(
        lines[0],
        "context 720k / ? · last call 2h ago · cache ? · ? compactions"
    );
    assert_eq!(
        lines[1],
        "❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi holds it for $0 now)"
    );
    server.abort();
}

/// `pij list` asks for sizes: each local seat gets its session facts, the
/// derived fields and the rendered columns. Without the ask the roster is
/// unchanged.
#[tokio::test]
async fn the_roster_carries_sizes_only_when_asked() {
    let source =
        pij_testkit::fakes::FakeSessionStatus::new().with_reply(COLD_SESSION, cold_status());
    let (addr, server, _, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;

    let plain = get_seats(addr, "").await;
    assert!(
        row(&plain, "pij-cold").get("sizeColumns").is_none(),
        "{plain}"
    );
    assert!(
        row(&plain, "pij-cold").get("sessionStatus").is_none(),
        "{plain}"
    );

    let sized = get_seats(addr, "?sizes=true").await;
    let cold = row(&sized, "pij-cold");
    assert_eq!(
        cold["sizeColumns"],
        serde_json::json!(["720k", "2h", "?", "❄"]),
        "{cold}"
    );
    assert_eq!(cold["contextUsed"], 720_000);
    assert_eq!(cold["coldWake"]["wouldRefuse"], true);
    assert_eq!(cold["sessionStatus"]["outcome"], "known");
    // An unbound seat is unknown, never refused.
    let sender = row(&sized, "pij-sender");
    assert_eq!(
        sender["sizeColumns"],
        serde_json::json!(["?", "?", "?", ""]),
        "{sender}"
    );
    assert_eq!(sender["coldWake"]["wouldRefuse"], false);
    server.abort();
}

/// A seat whose facts never arrive costs the list a bounded wait, then `?`.
#[tokio::test]
async fn a_slow_seat_makes_the_list_wait_a_bounded_time_and_show_unknown() {
    let source = pij_testkit::fakes::FakeSessionStatus::new().with_hang(COLD_SESSION);
    let (addr, server, _, _) = cold_daemon(pij_core::model::SystemState::Idle, source).await;
    let started = std::time::Instant::now();
    let sized = get_seats(addr, "?sizes=true").await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(800),
        "{elapsed:?}"
    );
    let cold = row(&sized, "pij-cold");
    assert_eq!(
        cold["sizeColumns"],
        serde_json::json!(["?", "?", "?", ""]),
        "{cold}"
    );
    assert_eq!(cold["sessionStatus"]["outcome"], "failed");
    server.abort();
}

/// Review P7c: the seats are read in PARALLEL. Twenty wedged seats cost the
/// list about one bound (200 ms), not twenty (4 s serialised).
#[tokio::test]
async fn many_slow_seats_cost_the_list_one_bound_not_one_each() {
    let mut source = pij_testkit::fakes::FakeSessionStatus::new();
    let mut registry = FakeRegistry::new();
    for n in 0..20 {
        let session = format!("hang-{n}");
        source = source.with_hang(&session);
        let mut seat = SeatDescriptor::new(format!("pij-slow-{n}"), Harness::Claude, "/abs/tree");
        seat.harness_session = Some(session);
        registry = registry.with_seat(seat);
    }
    let queue = Arc::new(FakeQueue::new(1_024).expect("valid fake queue policy"));
    let mut services = test_services(Arc::new(registry), queue, Arc::new(FakeSpine::new())).await;
    services.session_status = Arc::new(source);
    let (addr, server) = spawn(router_with_config(services, config("key", &[]))).await;
    let started = std::time::Instant::now();
    let sized = get_seats(addr, "?sizes=true").await;
    let elapsed = started.elapsed();
    assert_eq!(sized["data"]["seats"].as_array().expect("seats").len(), 20);
    assert!(
        elapsed < std::time::Duration::from_millis(1_500),
        "{elapsed:?}"
    );
    server.abort();
}
