use pij_core::events::EventFilter;
use pij_core::model::Event;
use pij_core::orchestration::RoleAssignment;
use pij_core::ports::Spine;
use pij_store::{SqliteRegistry, SqliteSpine, StorePool};
use pij_testkit::FreshStore;
use tokio_stream::StreamExt;

use super::*;

const ROUTES: &str = include_str!("../../../testkit/fixtures/golden/api/governance-routes.json");
const EVENTS: &str = include_str!("../../../testkit/fixtures/golden/api/governance-events.json");

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
        .expect("event")
        .clone()
}

struct Fixture {
    service: Arc<RoleService>,
    registry: Arc<SqliteRegistry>,
    spine: Arc<SqliteSpine>,
    bus: Arc<EventBus>,
    store: SqliteOrchestration,
    pool: StorePool,
    initial_cursor: Seq,
    _directory: FreshStore,
}
impl Fixture {
    async fn new() -> Self {
        let directory = FreshStore::new();
        let pool = pij_store::open(&directory.path())
            .await
            .expect("isolated store");
        let store = SqliteOrchestration::new(pool.clone());
        let spine = Arc::new(SqliteSpine::new(pool.clone()));
        let bus = Arc::new(EventBus::new(spine.clone(), 16).expect("bus"));
        let registry = Arc::new(SqliteRegistry::new(pool.clone(), bus.clone()));
        let context = routes()["fixture_context"].clone();
        let template = event_case("seat-put")["decoded_payload"].clone();
        let worker: SeatDescriptor = serde_json::from_value(template).expect("worker");
        let mut parent = worker.clone();
        parent.id = SeatId::from(context["parent"].as_str().expect("parent"));
        parent.parent = None;
        parent.pane = None;
        parent.proc = None;
        parent.role = None;
        registry.put(parent).await.expect("seed parent");
        registry.put(worker).await.expect("seed worker");
        let initial_cursor = spine
            .tail(None, Seq(0))
            .await
            .expect("seed events")
            .last()
            .and_then(|event| event.seq)
            .expect("seed cursor");
        let at = context["at_ms"].as_u64().expect("at");
        let service = Arc::new(
            RoleService::new(registry.clone(), store.clone(), bus.clone())
                .with_clock(Arc::new(move || Ok(at))),
        );
        Self {
            service,
            registry,
            spine,
            bus,
            store,
            pool,
            initial_cursor,
            _directory: directory,
        }
    }
    fn ids() -> (SeatId, SeatId, String) {
        let record = routes()["fixture_context"]["records"]["role"].clone();
        (
            SeatId::from(record["assigned_by"].as_str().expect("actor")),
            SeatId::from(record["seat"].as_str().expect("seat")),
            record["role"].as_str().expect("role").to_string(),
        )
    }
    async fn events(&self) -> Vec<Event> {
        self.spine
            .tail(None, self.initial_cursor)
            .await
            .expect("events after seed")
    }
    async fn fail_spine_append(&self) {
        sqlx::query("CREATE TRIGGER fail_role_append BEFORE INSERT ON spine_events BEGIN SELECT RAISE(ABORT, 'injected-role-spine-append'); END")
            .execute(&self.pool).await.expect("install SQL append failure");
    }
    async fn assert_rollback(&self, existing: bool, unset: bool) {
        let (actor, seat, role) = Self::ids();
        if existing {
            let contract = routes();
            let previous = contract["routes"]
                .as_array()
                .expect("routes")
                .iter()
                .flat_map(|route| route["cases"].as_array().into_iter().flatten())
                .find(|case| case["id"] == "orchestration-role-set")
                .expect("canonical previous role");
            self.store
                .assign_role(&RoleAssignment {
                    seat: seat.clone(),
                    role: previous["response"]["data"]["role"]["role"]
                        .as_str()
                        .expect("previous role")
                        .to_string(),
                    assigned_by: actor.clone(),
                    assigned_at: contract["fixture_context"]["at_ms"].as_u64().expect("at"),
                })
                .await
                .expect("seed assignment");
        }
        let before = self.store.seat_role(&seat).await.expect("prior assignment");
        assert_eq!(before.is_some(), existing);
        let before_events = self.events().await;
        let mut live = self
            .bus
            .subscribe(Some(self.initial_cursor), EventFilter::default())
            .await
            .expect("subscribe");
        self.fail_spine_append().await;
        let error = self
            .service
            .assert_role(&actor, &seat, if unset { None } else { Some(role) })
            .await
            .expect_err("SQL append failure must refuse the whole assertion");
        assert!(matches!(error, RoleError::Runtime(_)), "{error}");
        assert_eq!(
            self.store
                .seat_role(&seat)
                .await
                .expect("assignment after failure"),
            before,
            "role row and audit event must roll back together"
        );
        assert_eq!(
            self.events().await,
            before_events,
            "no durable event after failed assertion"
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), live.next())
                .await
                .is_err(),
            "failed transaction must not broadcast"
        );
    }
}

#[tokio::test]
async fn explicit_role_and_unset_publish_u0_payloads_to_live_subscribers() {
    let fixture = Fixture::new().await;
    let (actor, seat, role) = Fixture::ids();
    let mut subscriber = fixture
        .bus
        .subscribe(Some(fixture.initial_cursor), EventFilter::default())
        .await
        .expect("subscribe");
    for (case, role) in [("role-set", Some(role)), ("role-unset", None)] {
        let receipt = fixture
            .service
            .assert_role(&actor, &seat, role.clone())
            .await
            .expect("assertion");
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), subscriber.next())
            .await
            .expect("live publication")
            .expect("live event");
        let expected = event_case(case);
        assert_eq!(
            serde_json::from_str::<Value>(&event.payload).expect("payload"),
            expected["decoded_payload"]
        );
        assert_eq!(event.kind, expected["frame"]["event"]["kind"]);
        assert_eq!(event.seq, Some(receipt.seq));
        assert_eq!(fixture.service.read_role(&seat).await.expect("role"), role);
    }
    assert_eq!(fixture.events().await.len(), 2);
}

#[tokio::test]
async fn joining_roles_clears_stale_descriptor_values_and_preserves_real_assignments() {
    let fixture = Fixture::new().await;
    let (actor, seat, role) = Fixture::ids();
    let mut stale = fixture
        .registry
        .get(&seat)
        .await
        .expect("registry")
        .expect("seat");
    assert_eq!(
        stale.role.as_deref(),
        Some(role.as_str()),
        "fixture must contain stale role"
    );
    fixture
        .service
        .join_roles(std::slice::from_mut(&mut stale))
        .await
        .expect("join");
    assert_eq!(stale.role, None, "descriptor copy cannot invent assignment");
    fixture
        .service
        .assert_role(&actor, &seat, Some(role.clone()))
        .await
        .expect("set");
    fixture
        .service
        .join_roles(std::slice::from_mut(&mut stale))
        .await
        .expect("join");
    assert_eq!(stale.role, Some(role));
    fixture
        .service
        .assert_role(&actor, &seat, None)
        .await
        .expect("unset");
    fixture
        .service
        .join_roles(std::slice::from_mut(&mut stale))
        .await
        .expect("join");
    assert_eq!(stale.role, None, "unset must not revive descriptor.role");
}

#[tokio::test]
async fn sql_append_failure_rolls_back_role_set_over_existing_assignment() {
    Fixture::new().await.assert_rollback(true, false).await;
}

#[tokio::test]
async fn sql_append_failure_rolls_back_role_set_over_absent_assignment() {
    Fixture::new().await.assert_rollback(false, false).await;
}

#[tokio::test]
async fn sql_append_failure_rolls_back_role_unset_over_existing_assignment() {
    Fixture::new().await.assert_rollback(true, true).await;
}

#[tokio::test]
async fn sql_append_failure_rolls_back_role_unset_over_absent_assignment() {
    Fixture::new().await.assert_rollback(false, true).await;
}

#[tokio::test]
async fn authority_is_reloaded_and_tombstoned_or_unknown_targets_emit_nothing() {
    let fixture = Fixture::new().await;
    let (actor, seat, role) = Fixture::ids();
    let mut subject = fixture
        .registry
        .get(&seat)
        .await
        .expect("registry")
        .expect("seat");
    subject.parent = None;
    fixture
        .registry
        .put(subject.clone())
        .await
        .expect("change parent");
    let before = fixture.events().await;
    let error = fixture
        .service
        .assert_role(&actor, &seat, Some(role.clone()))
        .await
        .expect_err("old parent cannot assert role");
    assert!(matches!(error, RoleError::Ownership { parent: None, .. }));
    assert_eq!(fixture.events().await, before);
    subject.parent = Some(actor.clone());
    subject.tombstoned_at = Some(routes()["fixture_context"]["at_ms"].as_u64().expect("at"));
    fixture.registry.put(subject).await.expect("tombstone");
    let before = fixture.events().await;
    assert!(
        fixture
            .service
            .assert_role(&actor, &seat, Some(role.clone()))
            .await
            .is_err()
    );
    let absent = SeatId::from(
        routes()["fixture_context"]["outsider"]
            .as_str()
            .expect("absent fixture seat"),
    );
    assert!(
        fixture
            .service
            .assert_role(&actor, &absent, Some(role))
            .await
            .is_err()
    );
    assert_eq!(fixture.events().await, before);
    assert!(fixture.store.list_roles().await.expect("roles").is_empty());
}

#[tokio::test]
async fn fallible_clock_refuses_before_persistence() {
    let fixture = Fixture::new().await;
    let (actor, seat, role) = Fixture::ids();
    let service = RoleService::new(
        fixture.registry.clone(),
        fixture.store.clone(),
        fixture.bus.clone(),
    )
    .with_clock(Arc::new(|| {
        Err(PijError::Adapter {
            adapter: "clock-fixture".to_string(),
            message: "unavailable".to_string(),
        })
    }));
    assert!(matches!(
        service.assert_role(&actor, &seat, Some(role)).await,
        Err(RoleError::Runtime(_))
    ));
    assert!(fixture.store.list_roles().await.expect("roles").is_empty());
    assert!(fixture.events().await.is_empty());
}

#[tokio::test]
async fn cancelled_role_request_still_commits_and_publishes_its_admitted_transaction() {
    let fixture = Fixture::new().await;
    let (actor, seat, role) = Fixture::ids();
    let transaction = pij_store::migrate::begin_write(&fixture.pool)
        .await
        .expect("hold SQL writer");
    let entered = Arc::new(tokio::sync::Notify::new());
    let clock_entered = entered.clone();
    let at = routes()["fixture_context"]["at_ms"].as_u64().expect("at");
    let service = Arc::new(
        RoleService::new(
            fixture.registry.clone(),
            fixture.store.clone(),
            fixture.bus.clone(),
        )
        .with_clock(Arc::new(move || {
            clock_entered.notify_one();
            Ok(at)
        })),
    );
    let target = seat.clone();
    let requested = role.clone();
    let request =
        tokio::spawn(async move { service.assert_role(&actor, &target, Some(requested)).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
        .await
        .expect("role admitted under bus lock");
    request.abort();
    assert!(request.await.expect_err("request cancelled").is_cancelled());
    transaction.rollback().await.expect("release SQL writer");
    fixture.bus.flush().await;
    assert_eq!(
        fixture
            .service
            .read_role(&seat)
            .await
            .expect("committed role"),
        Some(role)
    );
    let events = fixture.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&events[0].payload).expect("role payload"),
        event_case("role-set")["decoded_payload"]
    );
}

#[tokio::test]
async fn role_authority_is_read_after_earlier_ordered_registry_change() {
    let fixture = Fixture::new().await;
    let (actor, seat, role) = Fixture::ids();
    let (entered, admitted) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let bus = fixture.bus.clone();
    let pool = fixture.pool.clone();
    let registry = fixture.registry.clone();
    let spine = fixture.spine.clone();
    let target = seat.clone();
    // A controlled prior registry publication holds the SAME bus boundary.
    // Its fixture SQL changes only parent; append still uses the real SQL spine.
    let prior = tokio::spawn(async move {
        bus.publish_committed(async move {
            entered.send(()).expect("announce publication admission");
            released.await.expect("release earlier publication");
            sqlx::query("UPDATE seats SET parent = NULL WHERE id = ?1")
                .bind(target.as_str())
                .execute(&pool)
                .await
                .expect("fixture reparent");
            let changed = registry.get(&target).await?.expect("changed subject");
            let mut event: Event =
                serde_json::from_value(event_case("seat-put")["frame"]["event"].clone())
                    .expect("fixture event");
            event.payload = serde_json::to_string(&changed).expect("committed descriptor");
            event.seq = Some(spine.append(event.clone()).await?);
            Ok((event, ()))
        })
        .await
    });
    admitted.await.expect("earlier publication has lock");
    let clock_called = Arc::new(tokio::sync::Notify::new());
    let clock_signal = clock_called.clone();
    let at = routes()["fixture_context"]["at_ms"].as_u64().expect("at");
    let service = Arc::new(
        RoleService::new(
            fixture.registry.clone(),
            fixture.store.clone(),
            fixture.bus.clone(),
        )
        .with_clock(Arc::new(move || {
            clock_signal.notify_one();
            Ok(at)
        })),
    );
    let (started, starting) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(async move {
        started.send(()).expect("announce role request");
        service.assert_role(&actor, &seat, Some(role)).await
    });
    starting.await.expect("role request started");
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(30),
            clock_called.notified()
        )
        .await
        .is_err(),
        "role authority/clock callback must not run ahead of the shared publication lock"
    );
    release.send(()).expect("release fixture registry change");
    prior
        .await
        .expect("prior task")
        .expect("registry publication");
    let error = request
        .await
        .expect("role task")
        .expect_err("former parent loses authority before role read");
    assert!(matches!(error, RoleError::Ownership { parent: None, .. }));
    assert!(fixture.store.list_roles().await.expect("roles").is_empty());
    assert_eq!(
        fixture.events().await.len(),
        1,
        "only the prior registry publication persisted"
    );
}

// Plan 166: link authority, re-role, guards and the closed vocabulary.

impl Fixture {
    async fn seat(&self, id: &str, parent: Option<&SeatId>) -> SeatId {
        let (_, template, _) = Self::ids();
        let mut seat = self
            .registry
            .get(&template)
            .await
            .expect("registry")
            .expect("template seat");
        seat.id = SeatId::from(id);
        seat.parent = parent.cloned();
        seat.pane = None;
        seat.proc = None;
        self.registry.put(seat).await.expect("seed seat");
        SeatId::from(id)
    }
    async fn tombstone(&self, seat: &SeatId) {
        self.registry
            .tombstone(seat, "test retirement")
            .await
            .expect("tombstone");
    }
    async fn link(
        &self,
        actor: &SeatId,
        seat: &SeatId,
        role: &str,
    ) -> std::result::Result<PlacementReceipt, RoleError> {
        self.service
            .place(actor, Placement::Link(seat.clone()), role)
            .await
    }
    async fn kinds_since(&self, before: usize) -> Vec<String> {
        self.events().await[before..]
            .iter()
            .map(|event| event.kind.clone())
            .collect()
    }
}

#[tokio::test]
async fn link_takes_a_parentless_seat_in_one_commit() {
    let fixture = Fixture::new().await;
    let governor = fixture.seat("pij-gov", None).await;
    let hand = fixture.seat("pij-hand", None).await;
    let before = fixture.events().await.len();
    let receipt = fixture
        .link(&governor, &hand, "worker")
        .await
        .expect("link");
    assert!(receipt.parent_changed && receipt.role_changed);
    assert_eq!(receipt.previous_parent, None);
    assert_eq!(fixture.kinds_since(before).await, ["seat.put", "role-set"]);
    let seat = fixture
        .registry
        .get(&hand)
        .await
        .expect("get")
        .expect("seat");
    assert_eq!(seat.parent, Some(governor.clone()));
    let assignment = fixture
        .store
        .seat_role(&hand)
        .await
        .expect("role")
        .expect("assigned");
    assert_eq!(
        (assignment.role.as_str(), &assignment.assigned_by),
        ("worker", &governor)
    );
}

#[tokio::test]
async fn link_takes_a_seat_whose_parent_is_tombstoned() {
    let fixture = Fixture::new().await;
    let governor = fixture.seat("pij-gov", None).await;
    let dead = fixture.seat("pij-dead", None).await;
    let orphan = fixture.seat("pij-orphan", Some(&dead)).await;
    fixture.tombstone(&dead).await;
    let receipt = fixture
        .link(&governor, &orphan, "pm")
        .await
        .expect("orphan is takeable");
    assert_eq!(receipt.previous_parent, Some(dead));
    assert_eq!(receipt.parent, Some(governor));
}

#[tokio::test]
async fn link_refuses_a_live_foreign_parent_and_names_it() {
    let fixture = Fixture::new().await;
    let (parent, worker, _) = Fixture::ids();
    let governor = fixture.seat("pij-gov", None).await;
    let before = fixture.events().await;
    let error = fixture
        .link(&governor, &worker, "worker")
        .await
        .expect_err("owned elsewhere");
    assert!(
        matches!(&error, RoleError::Ownership { parent: Some(named), .. } if *named == parent),
        "{error}"
    );
    assert_eq!(fixture.events().await, before, "a refusal commits nothing");
    assert_eq!(
        fixture
            .registry
            .get(&worker)
            .await
            .expect("get")
            .expect("seat")
            .parent,
        Some(parent)
    );
}

#[tokio::test]
async fn current_parent_re_roles_without_reparenting_and_unchanged_role_emits_nothing() {
    let fixture = Fixture::new().await;
    let (parent, worker, _) = Fixture::ids();
    let before = fixture.events().await.len();
    let receipt = fixture.link(&parent, &worker, "pm").await.expect("re-role");
    assert!(!receipt.parent_changed && receipt.role_changed);
    assert_eq!(fixture.kinds_since(before).await, ["role-set"]);
    let before = fixture.events().await.len();
    let again = fixture
        .link(&parent, &worker, "pm")
        .await
        .expect("idempotent");
    assert!(!again.parent_changed && !again.role_changed);
    assert!(again.seqs.is_empty());
    assert_eq!(
        fixture.events().await.len(),
        before,
        "no role-set for an unchanged role"
    );
}

#[tokio::test]
async fn link_refuses_capturing_your_own_ancestor() {
    let fixture = Fixture::new().await;
    let (parent, worker, _) = Fixture::ids();
    let before = fixture.events().await;
    let error = fixture
        .link(&worker, &parent, "worker")
        .await
        .expect_err("cycle");
    assert!(
        matches!(&error, RoleError::Placement { code: "E-RS-ARG", details, .. } if details["reason"] == "cycle"),
        "{error}"
    );
    assert_eq!(fixture.events().await, before);
}

#[tokio::test]
async fn link_refuses_capturing_a_prime() {
    let fixture = Fixture::new().await;
    let prime = fixture.seat("pij-prime", None).await;
    let outsider = fixture.seat("pij-outsider", None).await;
    fixture
        .service
        .assert_role(&prime, &prime, Some("prime".to_string()))
        .await
        .expect("self-asserted prime");
    let before = fixture.events().await;
    let error = fixture
        .link(&outsider, &prime, "worker")
        .await
        .expect_err("prime capture");
    assert!(
        matches!(&error, RoleError::Placement { code: "E-RS-OWNERSHIP", details, .. } if details["reason"] == "prime"),
        "{error}"
    );
    assert_eq!(fixture.events().await, before);
}

#[tokio::test]
async fn every_role_setter_refuses_outside_the_closed_vocabulary() {
    let fixture = Fixture::new().await;
    let (parent, worker, _) = Fixture::ids();
    let before = fixture.events().await;
    for role in ["coder", "reviewer", "Worker", ""] {
        let error = fixture
            .service
            .assert_role(&parent, &worker, Some(role.to_string()))
            .await
            .expect_err("assertion vocabulary");
        assert!(
            matches!(&error, RoleError::Invalid(reason) if reason.contains("prime, pm, worker, pa")),
            "{error}"
        );
    }
    for role in ["coder", "prime"] {
        let error = fixture
            .link(&parent, &worker, role)
            .await
            .expect_err("placement vocabulary");
        assert!(
            matches!(&error, RoleError::Invalid(reason) if reason.contains("pm, worker, pa")),
            "{error}"
        );
    }
    assert_eq!(fixture.events().await, before);
    for role in ["prime", "pm", "worker", "pa"] {
        fixture
            .service
            .assert_role(&parent, &worker, Some(role.to_string()))
            .await
            .expect("every vocabulary role is assertable");
    }
}

/// Pauses one seat's registry commit until released: a deterministic failpoint
/// between a writer's roster snapshot and its put (anglerfish #26, HIGH).
struct GatedRegistry {
    inner: Arc<SqliteRegistry>,
    seat: SeatId,
    reached: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl GatedRegistry {
    async fn gate(&self, seat: &SeatId) {
        if seat != &self.seat {
            return;
        }
        if let Some(reached) = self.reached.lock().await.take() {
            reached.send(()).expect("announce paused commit");
        }
        if let Some(release) = self.release.lock().await.take() {
            release.await.expect("release paused commit");
        }
    }
}

#[async_trait::async_trait]
impl Registry for GatedRegistry {
    async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
        self.inner.get(seat).await
    }
    async fn put(&self, descriptor: SeatDescriptor) -> Result<Seq> {
        self.put_reporting(descriptor).await.map(|(seq, _)| seq)
    }
    async fn put_reporting(
        &self,
        descriptor: SeatDescriptor,
    ) -> Result<(Seq, pij_core::ports::PutBinding)> {
        self.gate(&descriptor.id).await;
        self.inner.put_reporting(descriptor).await
    }
    async fn put_reporting_keeping_parent(
        &self,
        descriptor: SeatDescriptor,
    ) -> Result<(Seq, pij_core::ports::PutBinding)> {
        self.gate(&descriptor.id).await;
        self.inner.put_reporting_keeping_parent(descriptor).await
    }
    async fn list(&self, filter: pij_core::ports::SeatFilter) -> Result<Vec<SeatDescriptor>> {
        self.inner.list(filter).await
    }
    async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<Seq> {
        self.inner.tombstone(seat, reason).await
    }
    async fn tombstone_if_unchanged(
        &self,
        expected: SeatDescriptor,
        reason: String,
    ) -> Result<Seq> {
        self.inner.tombstone_if_unchanged(expected, reason).await
    }
    async fn set_activity(
        &self,
        seat: &SeatId,
        state: pij_core::model::SystemState,
        reason: Option<&str>,
    ) -> Result<Option<Seq>> {
        self.inner.set_activity(seat, state, reason).await
    }
}

async fn registration_racing_link(orphaned: bool) {
    let fixture = Fixture::new().await;
    let (_, hand, _) = Fixture::ids();
    let mut seat = fixture
        .registry
        .get(&hand)
        .await
        .expect("get")
        .expect("seat");
    seat.parent = if orphaned {
        let dead = fixture.seat("pij-dead", None).await;
        fixture.tombstone(&dead).await;
        Some(dead)
    } else {
        None
    };
    fixture.registry.put(seat.clone()).await.expect("seed seat");
    let governor = fixture.seat("pij-gov", None).await;
    let outsider = fixture.seat("pij-outsider", None).await;
    let proc = seat.proc.expect("template seat is bound");

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let gated = Arc::new(GatedRegistry {
        inner: fixture.registry.clone(),
        seat: hand.clone(),
        reached: tokio::sync::Mutex::new(Some(reached_tx)),
        release: tokio::sync::Mutex::new(Some(release_rx)),
    });
    let registration = crate::registration::RegistrationService::new(
        gated,
        Arc::new(pij_testkit::fakes::FakeLiveness::new().with_proc(proc)),
        fixture.bus.clone(),
        Vec::new(),
        fixture.service.clone(),
    );
    let claim = crate::http::Registration {
        supersedes: None,
        id: hand.to_string(),
        harness: seat.harness.as_str().to_string(),
        folder: seat.folder.clone(),
        extension_build: None,
        extension_path: None,
        pane: seat.pane.clone(),
        pid: Some(proc.pid),
        proc_start: Some(proc.proc_start),
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
    let refresh = tokio::spawn(async move { registration.register(claim).await });
    // The refresh has read its roster snapshot (old parent) and is paused at its put.
    reached_rx.await.expect("registration reached its commit");
    let linked = fixture
        .link(&governor, &hand, "worker")
        .await
        .expect("link commits while the refresh is paused");
    assert_eq!(linked.parent, Some(governor.clone()));
    release_tx.send(()).expect("release the refresh");
    let refreshed = refresh
        .await
        .expect("registration task")
        .expect("refresh commits");

    let row = fixture
        .registry
        .get(&hand)
        .await
        .expect("get")
        .expect("seat");
    assert_eq!(
        row.parent,
        Some(governor.clone()),
        "the link's parent survives the refresh"
    );
    assert_eq!(
        refreshed.parent,
        Some(governor.clone()),
        "the refresh reports the committed parent"
    );
    let error = fixture
        .link(&outsider, &hand, "pm")
        .await
        .expect_err("the governor still owns the seat");
    assert!(
        matches!(&error, RoleError::Ownership { parent: Some(named), .. } if *named == governor),
        "{error}"
    );
}

#[tokio::test]
async fn a_registration_snapshot_never_reverts_a_link_of_a_parentless_seat() {
    registration_racing_link(false).await;
}

#[tokio::test]
async fn a_registration_snapshot_never_reverts_an_orphan_takeover() {
    registration_racing_link(true).await;
}
