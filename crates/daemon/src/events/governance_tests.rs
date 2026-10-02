use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::events::EventFilter;
use pij_core::model::{Event, ProcIdentity, SeatDescriptor, Seq};
use pij_core::ports::{Registry, Spine};
use pij_core::report::{ReportConfig, ReportService};
use pij_store::{SqliteRegistry, SqliteSpine};
use pij_testkit::FreshStore;
use serde_json::Value;
use tokio::time::timeout;
use tokio_stream::StreamExt;

use super::{EventBus, Subscription};

fn contract() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testkit/fixtures/golden/api/governance-events.json"
    )))
    .expect("canonical governance event fixture")
}

fn case(fixture: &Value, id: &str) -> Value {
    fixture["events"]
        .as_array()
        .expect("event cases")
        .iter()
        .find(|case| case["id"] == id)
        .expect("named contract case")
        .clone()
}

fn descriptor(fixture: &Value) -> SeatDescriptor {
    serde_json::from_value(case(fixture, "seat-put")["decoded_payload"].clone())
        .expect("canonical descriptor")
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("epoch")
        .as_millis() as u64
}

async fn next(stream: &mut Subscription) -> Event {
    timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("committed event must arrive live")
        .expect("subscription remains open")
}

#[tokio::test]
async fn registry_and_report_publish_canonical_payloads_to_both_live_subscribers() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 32).unwrap());
    let registry = SqliteRegistry::new(pool, bus.clone());
    let mut first = bus.subscribe_live(EventFilter::all());
    let mut second = bus.subscribe_live(EventFilter::all());
    let seat = descriptor(&fixture);
    let before = epoch_ms();
    let put_seq = registry.put(seat.clone()).await.expect("put");
    let after = epoch_ms();
    let put = next(&mut first).await;
    assert_eq!(put, next(&mut second).await);
    assert_eq!(put.seq, Some(put_seq));
    assert!((before..=after).contains(&put.at));
    assert_eq!(
        put.kind,
        case(&fixture, "seat-put")["frame"]["event"]["kind"]
    );
    assert_eq!(
        serde_json::from_str::<Value>(&put.payload).unwrap(),
        case(&fixture, "seat-put")["decoded_payload"]
    );
    assert_eq!(
        serde_json::from_str::<SeatDescriptor>(&put.payload).unwrap(),
        registry.get(&seat.id).await.unwrap().unwrap()
    );

    let now = case(&fixture, "report-now");
    let reports = ReportService::new(&registry, bus.as_ref(), epoch_ms, ReportConfig::default());
    let report_seq = reports
        .now(
            &seat.id,
            now["decoded_payload"]["did"].as_str().unwrap(),
            now["decoded_payload"]["next"].as_str().unwrap(),
        )
        .await
        .unwrap();
    let report = next(&mut first).await;
    assert_eq!(report, next(&mut second).await);
    assert_eq!(report.seq, Some(report_seq));
    assert!(report.at > 0);
    assert_eq!(
        serde_json::from_str::<Value>(&report.payload).unwrap(),
        now["decoded_payload"]
    );

    let state_case = case(&fixture, "report-state");
    let state = serde_json::from_value(state_case["decoded_payload"]["state"].clone()).unwrap();
    let state_seq = reports
        .declare(&seat.id, state, None, None, &[])
        .await
        .unwrap();
    let changed = next(&mut first).await;
    assert_eq!(changed, next(&mut second).await);
    let declared = next(&mut first).await;
    assert_eq!(declared, next(&mut second).await);
    assert_eq!(declared.seq, Some(state_seq));
    let mut expected = state_case["decoded_payload"].clone();
    expected["registry_seq"] = serde_json::to_value(changed.seq.unwrap()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&declared.payload).unwrap(),
        expected
    );
    assert!(
        put_seq < report_seq
            && report_seq < changed.seq.unwrap()
            && changed.seq.unwrap() < state_seq
    );

    let tombstone = case(&fixture, "seat-tombstone");
    let reason = tombstone["decoded_payload"]["reason"].as_str().unwrap();
    let before = epoch_ms();
    let seq = registry.tombstone(&seat.id, reason).await.unwrap();
    let after = epoch_ms();
    let dead = next(&mut first).await;
    assert_eq!(dead, next(&mut second).await);
    assert_eq!(dead.seq, Some(seq));
    assert!((before..=after).contains(&dead.at));
    assert_eq!(
        serde_json::from_str::<Value>(&dead.payload).unwrap(),
        tombstone["decoded_payload"]
    );
    assert_eq!(
        registry.get(&seat.id).await.unwrap().unwrap().tombstoned_at,
        Some(dead.at)
    );
}

#[tokio::test]
async fn rollback_and_missing_tombstone_never_publish_or_advance_the_spine() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16).unwrap());
    let registry = SqliteRegistry::new(pool.clone(), bus.clone());
    let mut live = bus.subscribe_live(EventFilter::all());
    let seat = descriptor(&fixture);
    sqlx::query("CREATE TRIGGER reject_put BEFORE INSERT ON seats BEGIN SELECT RAISE(ABORT, 'fixture put rejected'); END")
        .execute(&pool).await.unwrap();
    assert!(
        registry
            .put(seat.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("fixture put rejected")
    );
    assert!(registry.get(&seat.id).await.unwrap().is_none());
    assert!(
        registry
            .tombstone(
                &seat.id,
                case(&fixture, "seat-tombstone")["decoded_payload"]["reason"]
                    .as_str()
                    .unwrap()
            )
            .await
            .is_err()
    );
    assert!(bus.tail(None, Seq(0)).await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER reject_put")
        .execute(&pool)
        .await
        .unwrap();
    let seq = registry.put(seat.clone()).await.unwrap();
    assert_eq!(
        next(&mut live).await.seq,
        Some(seq),
        "no rolled-back event precedes the successful write"
    );

    sqlx::query("CREATE TRIGGER reject_tombstone BEFORE UPDATE OF tombstoned_at ON seats BEGIN SELECT RAISE(ABORT, 'fixture tombstone rejected'); END")
        .execute(&pool).await.unwrap();
    let error = registry.tombstone(&seat.id, "fixture").await.unwrap_err();
    assert!(error.to_string().contains("fixture tombstone rejected"));
    assert_eq!(registry.get(&seat.id).await.unwrap().unwrap(), seat);
    assert_eq!(bus.tail(None, Seq(0)).await.unwrap().len(), 1);
    let marker = case(&fixture, "report-now");
    let event: Event = serde_json::from_value(marker["frame"]["event"].clone()).unwrap();
    let marker_seq = bus.publish(event).await.unwrap();
    assert_eq!(next(&mut live).await.seq, Some(marker_seq));
}

#[tokio::test]
async fn registry_event_failure_rolls_back_the_descriptor_and_binding_history() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16).unwrap());
    let registry = SqliteRegistry::new(pool.clone(), bus.clone());
    let seat = descriptor(&fixture);
    let (_, inserted) = registry.put_reporting(seat.clone()).await.unwrap();
    assert!(inserted.inserted);
    assert_eq!(inserted.previous_proc, None);
    sqlx::query("CREATE TRIGGER reject_event BEFORE INSERT ON spine_events BEGIN SELECT RAISE(ABORT, 'fixture event rejected'); END")
        .execute(&pool).await.unwrap();
    let mut rebound = seat.clone();
    rebound.proc = Some(ProcIdentity {
        pid: seat.proc.unwrap().pid + 1,
        proc_start: seat.proc.unwrap().proc_start + 1,
    });
    assert!(registry.put_reporting(rebound.clone()).await.is_err());
    assert_eq!(registry.get(&seat.id).await.unwrap().unwrap(), seat);
    sqlx::query("DROP TRIGGER reject_event")
        .execute(&pool)
        .await
        .unwrap();
    let (_, binding) = registry.put_reporting(rebound).await.unwrap();
    assert!(!binding.inserted);
    assert_eq!(binding.previous_proc, seat.proc);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_registry_and_report_writes_have_identical_durable_and_live_order() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 128).unwrap());
    let registry = Arc::new(SqliteRegistry::new(pool, bus.clone()));
    let mut first = bus.subscribe_live(EventFilter::all());
    let mut second = bus.subscribe_live(EventFilter::all());
    let mut jobs = tokio::task::JoinSet::new();
    for i in 0..32 {
        let bus = bus.clone();
        let registry = registry.clone();
        let mut seat = descriptor(&fixture);
        seat.proc.as_mut().unwrap().proc_start += i;
        let event: Event =
            serde_json::from_value(case(&fixture, "report-now")["frame"]["event"].clone()).unwrap();
        jobs.spawn(async move {
            if i % 2 == 0 {
                registry.put(seat).await
            } else {
                bus.append(event).await
            }
        });
    }
    while let Some(result) = jobs.join_next().await {
        result.unwrap().unwrap();
    }
    let durable = bus.tail(None, Seq(0)).await.unwrap();
    assert_eq!(durable.len(), 32);
    for event in durable {
        assert_eq!(next(&mut first).await, event);
        assert_eq!(next(&mut second).await, event);
    }
}

fn direct_insert_files(dir: &Path, root: &Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read source tree") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            direct_insert_files(&path, root, found);
        } else if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("rs" | "sql")
        ) {
            let relative = path.strip_prefix(root).unwrap();
            if relative == Path::new("store/src/spine.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source text");
            let normalized = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            let needle = ["insert", "into", "spine_events"].join(" ");
            let replaced = ["replace", "into", "spine_events"].join(" ");
            if normalized.contains(&needle) || normalized.contains(&replaced) {
                found.push(relative.display().to_string());
            }
        }
    }
}

#[test]
fn only_store_spine_contains_direct_spine_event_inserts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            direct_insert_files(&src, root, &mut found);
        }
    }
    assert!(
        found.is_empty(),
        "spine inserts bypass EventBus publication: {found:?}"
    );
}

#[tokio::test]
async fn fake_registry_uses_bus_sequences_and_preserves_state_when_append_fails() {
    let fixture = contract();
    let spine = Arc::new(pij_testkit::fakes::FakeSpine::new());
    let bus = Arc::new(EventBus::new(spine.clone(), 16).unwrap());
    let registry = super::PublishedFakeRegistry::new(
        Arc::new(pij_testkit::fakes::FakeRegistry::new()),
        bus.clone(),
    );
    let mut live = bus.subscribe_live(EventFilter::all());
    let seat = descriptor(&fixture);
    let marker: Event =
        serde_json::from_value(case(&fixture, "report-now")["frame"]["event"].clone()).unwrap();
    let marker_seq = bus.append(marker).await.unwrap();
    assert_eq!(next(&mut live).await.seq, Some(marker_seq));
    let (put_seq, binding) = registry.put_reporting(seat.clone()).await.unwrap();
    assert_eq!(
        put_seq.0,
        marker_seq.0 + 1,
        "fake registry cannot leak its private sequence"
    );
    assert!(binding.inserted);
    let put = next(&mut live).await;
    assert_eq!(put.seq, Some(put_seq));
    assert_eq!(
        serde_json::from_str::<Value>(&put.payload).unwrap(),
        case(&fixture, "seat-put")["decoded_payload"]
    );
    spine.script_append_error("fixture append refused");
    assert!(registry.tombstone(&seat.id, "fixture").await.is_err());
    assert_eq!(registry.get(&seat.id).await.unwrap().unwrap(), seat);
    let reason = case(&fixture, "seat-tombstone")["decoded_payload"]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    let seq = registry.tombstone(&seat.id, &reason).await.unwrap();
    let dead = next(&mut live).await;
    assert_eq!(dead.seq, Some(seq));
    assert_eq!(seq.0, put_seq.0 + 1);
    assert_eq!(
        registry.get(&seat.id).await.unwrap().unwrap().tombstoned_at,
        Some(dead.at)
    );
}

#[tokio::test]
async fn registry_callback_is_not_polled_until_the_common_publication_lock_is_held() {
    use pij_store::spine::RegistryPublisher;
    use std::sync::atomic::{AtomicBool, Ordering};

    let fixture = contract();
    let spine = Arc::new(pij_testkit::fakes::FakeSpine::new());
    let bus = Arc::new(EventBus::new(spine.clone(), 16).unwrap());
    let polled = Arc::new(AtomicBool::new(false));
    let mut live = bus.subscribe_live(EventFilter::all());
    let guard = bus.publish_lock.lock().await;
    let mut event: Event =
        serde_json::from_value(case(&fixture, "report-now")["frame"]["event"].clone()).unwrap();
    let called = polled.clone();
    let mut publication = bus.publish_registry(Box::pin(async move {
        called.store(true, Ordering::SeqCst);
        let seq = spine.append(event.clone()).await?;
        event.seq = Some(seq);
        Ok((event, None))
    }));
    std::future::poll_fn(|cx| {
        assert!(publication.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert!(!polled.load(Ordering::SeqCst));
    drop(guard);
    let (seq, _) = publication.await.unwrap();
    assert!(polled.load(Ordering::SeqCst));
    assert_eq!(next(&mut live).await.seq, Some(seq));
}

fn assert_incarnation_changed(error: pij_core::error::PijError, seat: &pij_core::model::SeatId) {
    match error {
        pij_core::error::PijError::GovernanceRefused { code, record } => {
            assert_eq!(code, "E-RS-INCARNATION-CHANGED");
            assert_eq!(record, seat.as_str());
        }
        other => panic!("expected named snapshot refusal, got {other:?}"),
    }
}

async fn check_guarded_tombstone_contract(registry: &dyn Registry, bus: &EventBus) {
    let fixture = contract();
    let original = descriptor(&fixture);
    let reason = case(&fixture, "seat-tombstone")["decoded_payload"]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    let mut live = bus.subscribe_live(EventFilter::all());
    registry.put(original.clone()).await.unwrap();
    next(&mut live).await;

    let mut changed = original.clone();
    changed.proc.as_mut().unwrap().proc_start += 1; // Same PID, different incarnation.
    registry.put(changed.clone()).await.unwrap();
    next(&mut live).await;
    let before = bus.tail(None, Seq(0)).await.unwrap();
    assert_incarnation_changed(
        registry
            .tombstone_if_unchanged(original, reason.clone())
            .await
            .unwrap_err(),
        &changed.id,
    );
    assert_eq!(
        registry.get(&changed.id).await.unwrap(),
        Some(changed.clone())
    );
    assert_eq!(bus.tail(None, Seq(0)).await.unwrap(), before);

    // Matching proc/pane is insufficient: ownership/role and API-only machine
    // stamping are part of whole raw descriptor equality too.
    for field in ["role", "parent", "machine"] {
        let mut stale = changed.clone();
        match field {
            "role" => stale.role = None,
            "parent" => stale.parent = None,
            "machine" => stale.machine = Some("joined-api-only".to_string()),
            _ => unreachable!(),
        }
        assert_incarnation_changed(
            registry
                .tombstone_if_unchanged(stale, reason.clone())
                .await
                .unwrap_err(),
            &changed.id,
        );
        assert_eq!(
            registry.get(&changed.id).await.unwrap(),
            Some(changed.clone())
        );
        assert_eq!(bus.tail(None, Seq(0)).await.unwrap(), before);
    }
    let mut missing = changed.clone();
    missing.id = format!("{}-absent", changed.id).into();
    assert_incarnation_changed(
        registry
            .tombstone_if_unchanged(missing.clone(), reason.clone())
            .await
            .unwrap_err(),
        &missing.id,
    );
    assert_eq!(bus.tail(None, Seq(0)).await.unwrap(), before);

    let seq = registry
        .tombstone_if_unchanged(changed.clone(), reason.clone())
        .await
        .unwrap();
    let event = next(&mut live).await;
    assert_eq!(event.seq, Some(seq), "refused attempts emitted no event");
    assert_eq!(
        event.kind,
        case(&fixture, "seat-tombstone")["frame"]["event"]["kind"]
    );
    assert_eq!(
        serde_json::from_str::<Value>(&event.payload).unwrap(),
        case(&fixture, "seat-tombstone")["decoded_payload"]
    );
    let dead = registry.get(&changed.id).await.unwrap().unwrap();
    assert_eq!(dead.tombstoned_at, Some(event.at));
    assert_eq!(dead.tombstone_reason.as_deref(), Some(reason.as_str()));
    assert_eq!(
        bus.tail(None, Seq(0)).await.unwrap().len(),
        before.len() + 1
    );

    // Unconditional intent remains available; it must not become a stale alias
    // of the guarded operation or acquire a synthetic expected snapshot.
    let unconditional = registry.tombstone(&changed.id, &reason).await.unwrap();
    assert_eq!(next(&mut live).await.seq, Some(unconditional));
}

#[tokio::test]
async fn sqlite_guarded_tombstone_requires_whole_raw_snapshot_and_publishes_only_success() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 32).unwrap());
    let registry = SqliteRegistry::new(pool, bus.clone());
    check_guarded_tombstone_contract(&registry, &bus).await;
}

#[tokio::test]
async fn published_fake_guarded_tombstone_matches_sqlite_snapshot_and_event_contract() {
    let bus = Arc::new(EventBus::new(Arc::new(pij_testkit::fakes::FakeSpine::new()), 32).unwrap());
    let registry = super::PublishedFakeRegistry::new(
        Arc::new(pij_testkit::fakes::FakeRegistry::new()),
        bus.clone(),
    );
    check_guarded_tombstone_contract(&registry, &bus).await;
}

#[tokio::test]
async fn raw_fake_guarded_tombstone_refusal_does_not_mutate_or_consume_sequence() {
    let fixture = contract();
    let registry = pij_testkit::fakes::FakeRegistry::new();
    let original = descriptor(&fixture);
    let mut current = original.clone();
    current.proc.as_mut().unwrap().proc_start += 1;
    let seq = registry.put(current.clone()).await.unwrap();
    let reason = case(&fixture, "seat-tombstone")["decoded_payload"]["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert_incarnation_changed(
        registry
            .tombstone_if_unchanged(original, reason.clone())
            .await
            .unwrap_err(),
        &current.id,
    );
    assert_eq!(
        registry.get(&current.id).await.unwrap(),
        Some(current.clone())
    );
    let mut missing = current.clone();
    missing.id = format!("{}-absent", current.id).into();
    assert_incarnation_changed(
        registry
            .tombstone_if_unchanged(missing.clone(), reason.clone())
            .await
            .unwrap_err(),
        &missing.id,
    );
    let accepted = registry
        .tombstone_if_unchanged(current.clone(), reason.clone())
        .await
        .unwrap();
    assert_eq!(accepted.0, seq.0 + 1);
    assert_eq!(
        registry
            .get(&current.id)
            .await
            .unwrap()
            .unwrap()
            .tombstone_reason,
        Some(reason)
    );
}

#[tokio::test]
async fn guarded_sqlite_tombstone_publication_failure_rolls_back_matching_snapshot() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16).unwrap());
    let registry = SqliteRegistry::new(pool.clone(), bus.clone());
    let expected = descriptor(&fixture);
    registry.put(expected.clone()).await.unwrap();
    let before = bus.tail(None, Seq(0)).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_guarded_event BEFORE INSERT ON spine_events BEGIN SELECT RAISE(ABORT, 'guarded event refused'); END").execute(&pool).await.unwrap();
    let error = registry
        .tombstone_if_unchanged(expected.clone(), "fixture".to_string())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("guarded event refused"));
    assert_eq!(registry.get(&expected.id).await.unwrap(), Some(expected));
    assert_eq!(bus.tail(None, Seq(0)).await.unwrap(), before);
}

#[tokio::test]
async fn guarded_published_fake_append_failure_preserves_matching_snapshot() {
    let fixture = contract();
    let spine = Arc::new(pij_testkit::fakes::FakeSpine::new());
    let bus = Arc::new(EventBus::new(spine.clone(), 16).unwrap());
    let registry = super::PublishedFakeRegistry::new(
        Arc::new(pij_testkit::fakes::FakeRegistry::new()),
        bus.clone(),
    );
    let expected = descriptor(&fixture);
    registry.put(expected.clone()).await.unwrap();
    let before = bus.tail(None, Seq(0)).await.unwrap();
    spine.script_append_error("guarded event refused");
    let error = registry
        .tombstone_if_unchanged(expected.clone(), "fixture".to_string())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("guarded event refused"));
    assert_eq!(registry.get(&expected.id).await.unwrap(), Some(expected));
    assert_eq!(bus.tail(None, Seq(0)).await.unwrap(), before);
}

struct NotifyRegistryAdmission {
    bus: Arc<EventBus>,
    entered: Arc<tokio::sync::Notify>,
}

impl pij_store::spine::RegistryPublisher for NotifyRegistryAdmission {
    fn publish_registry<'a>(
        &'a self,
        commit: pij_store::spine::RegistryCommit,
    ) -> pij_store::spine::RegistryPublication<'a> {
        self.entered.notify_one();
        pij_store::spine::RegistryPublisher::publish_registry(self.bus.as_ref(), commit)
    }
}

#[tokio::test]
async fn guarded_sqlite_tombstone_reads_snapshot_after_publication_admission() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16).unwrap());
    let entered = Arc::new(tokio::sync::Notify::new());
    let registry = Arc::new(SqliteRegistry::new(
        pool.clone(),
        Arc::new(NotifyRegistryAdmission {
            bus: bus.clone(),
            entered: entered.clone(),
        }),
    ));
    let expected = descriptor(&fixture);
    registry.put(expected.clone()).await.unwrap();
    entered.notified().await; // Consume the seed's publisher notification.
    let before = bus.tail(None, Seq(0)).await.unwrap();
    let guard = bus.publish_lock.lock().await;
    let writer = registry.clone();
    let snapshot = expected.clone();
    let pending = tokio::spawn(async move {
        writer
            .tombstone_if_unchanged(snapshot, "fixture".to_string())
            .await
    });
    timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    // Simulate another authoritative store writer after the request reaches its
    // publisher but before the bus admits it. A pre-lock get would be stale.
    timeout(
        Duration::from_secs(3),
        sqlx::query("UPDATE seats SET role = NULL WHERE id = ?1")
            .bind(expected.id.as_str())
            .execute(&pool),
    )
    .await
    .unwrap()
    .unwrap();
    drop(guard);
    assert_incarnation_changed(
        timeout(Duration::from_secs(3), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        &expected.id,
    );
    let current = registry.get(&expected.id).await.unwrap().unwrap();
    assert_eq!(current.role, None);
    assert_eq!(current.tombstoned_at, None);
    assert_eq!(bus.tail(None, Seq(0)).await.unwrap(), before);
}

#[tokio::test]
async fn concurrent_guarded_tombstones_accept_exactly_one_snapshot() {
    let fixture = contract();
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let bus = Arc::new(EventBus::new(Arc::new(SqliteSpine::new(pool.clone())), 16).unwrap());
    let registry = SqliteRegistry::new(pool, bus.clone());
    let expected = descriptor(&fixture);
    registry.put(expected.clone()).await.unwrap();
    let (first, second) = tokio::join!(
        registry.tombstone_if_unchanged(expected.clone(), "first".to_string()),
        registry.tombstone_if_unchanged(expected.clone(), "second".to_string()),
    );
    match (first, second) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => {
            assert_incarnation_changed(error, &expected.id)
        }
        other => panic!("exactly one matching snapshot must win: {other:?}"),
    }
    assert_eq!(
        bus.tail(None, Seq(0))
            .await
            .unwrap()
            .iter()
            .filter(|event| event.kind == "seat.tombstone")
            .count(),
        1
    );
}
