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
