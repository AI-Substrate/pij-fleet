use pij_core::error::PijError;
use pij_core::model::{SeatId, Seq};
use pij_core::orchestration::{
    BatonDefinition, BatonRequest, Dispatch, DispatchCanary, DispatchState, Fence, PlanAttestation,
    PrimeDesignation, PrimeState, Project, ProjectUpdate, Stream, StreamPlan, StreamState,
    TaskAssignment, TaskCloseReason,
};
use pij_core::ports::Spine;
use pij_store::{
    BatonLease, DispatchAck, GovernanceOutcome, SqliteOrchestration, SqliteSpine, StreamReservation,
};
use pij_testkit::FreshStore;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-routes.json"
    ))
    .expect("canonical governance fixture")
}

fn record<T: DeserializeOwned>(name: &str) -> T {
    serde_json::from_value(fixture()["fixture_context"]["records"][name].clone())
        .expect("typed canonical record")
}

fn response(case_id: &str, field: &str) -> Value {
    fixture()["routes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|route| route["cases"].as_array().unwrap())
        .find(|case| case["id"] == case_id)
        .unwrap()["response"]["data"][field]
        .clone()
}

fn event_record(case_id: &str) -> Value {
    let events: Value = serde_json::from_str(include_str!(
        "../../testkit/fixtures/golden/api/governance-events.json"
    ))
    .expect("canonical event fixture");
    events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["id"] == case_id)
        .unwrap()["decoded_payload"]["record"]
        .clone()
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str) {
    let value = fixture()["fixture_context"]["records"][name].clone();
    let typed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
}

async fn reserve_fixture_stream(store: &SqliteOrchestration) -> Stream {
    let stream: Stream = record("stream");
    assert!(store.create_project(&record("project")).await.unwrap());
    let plan = StreamPlan {
        project: stream.project.clone(),
        slug: stream.slug.clone(),
        ordinal: stream.ordinal,
        branch: stream.branch.clone(),
        worktree: stream.worktree.clone(),
        base_ref: stream.base_ref.clone(),
    };
    assert_eq!(
        store
            .reserve_stream(&plan, &stream.created_by, stream.created_at)
            .await
            .unwrap(),
        StreamReservation::Reserved
    );
    stream
}

#[test]
fn canonical_governance_rows_round_trip_without_synthetic_fields() {
    round_trip::<Project>("project");
    round_trip::<Stream>("stream");
    round_trip::<Fence>("fence");
    round_trip::<Dispatch>("dispatch");
    round_trip::<TaskAssignment>("task");
    round_trip::<PlanAttestation>("attestation");
    round_trip::<BatonDefinition>("baton");
    round_trip::<BatonRequest>("baton_request");
    round_trip::<BatonLease>("baton_lease");
    round_trip::<PrimeDesignation>("prime");
    for case in ["dispatch-ack", "canary-verified"] {
        let value = response(case, "dispatch");
        let typed: Dispatch = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), value);
    }
    assert!(serde_json::from_value::<TaskCloseReason>(Value::String("unknown".into())).is_err());
}

#[tokio::test]
async fn project_metadata_updates_preserve_unsaid_fields_and_first_writer() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool);
    let project: Project = record("project");
    assert!(store.create_project(&project).await.unwrap());
    let mut duplicate = project.clone();
    duplicate.description = Some("must not replace".into());
    assert!(!store.create_project(&duplicate).await.unwrap());
    assert_eq!(
        store.project(&project.slug).await.unwrap(),
        Some(project.clone())
    );
    let changed = store
        .update_project(
            &project.slug,
            &ProjectUpdate {
                plan_path: Some(None),
                ..ProjectUpdate::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    let mut expected = project;
    expected.plan_path = None;
    assert_eq!(changed, expected);
    assert_eq!(store.list_projects().await.unwrap(), vec![expected]);
}

#[tokio::test]
async fn stream_reservation_is_not_creation_and_close_preserves_worktree_and_fence() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool);
    let stream = reserve_fixture_stream(&store).await;
    let reserved = store.stream(&stream.id).await.unwrap().unwrap();
    assert_eq!(reserved.state, StreamState::Reserved);
    assert_eq!(reserved.id, format!("{}:{}", stream.project, stream.slug));
    assert_eq!(
        store
            .set_stream_state(&stream.id, StreamState::Created)
            .await
            .unwrap(),
        GovernanceOutcome::Changed(stream.clone())
    );
    let fence: Fence = record("fence");
    assert_eq!(store.set_fence(&fence).await.unwrap(), fence);
    let closed = match store
        .set_stream_state(&stream.id, StreamState::Closed)
        .await
        .unwrap()
    {
        GovernanceOutcome::Changed(row) => row,
        outcome => panic!("expected closure, got {outcome:?}"),
    };
    assert_eq!(closed.worktree, stream.worktree);
    assert_eq!(closed.branch, stream.branch);
    assert_eq!(
        store.list_fences(Some(&stream.id)).await.unwrap(),
        vec![fence]
    );
    assert!(matches!(
        store
            .set_stream_state(&stream.id, StreamState::Created)
            .await
            .unwrap(),
        GovernanceOutcome::Conflict { .. }
    ));
    assert_eq!(
        store.list_streams(Some(&stream.project)).await.unwrap(),
        vec![closed]
    );
}

#[tokio::test]
async fn dispatch_ack_sha_and_recipient_precede_idempotence_and_delayed_delivery() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool);
    let dispatch: Dispatch = record("dispatch");
    let expected: Dispatch = serde_json::from_value(response("dispatch-ack", "dispatch")).unwrap();
    let sha = dispatch.packet_sha256.as_deref().unwrap();
    assert!(store.create_dispatch(&dispatch).await.unwrap());
    assert!(!store.create_dispatch(&dispatch).await.unwrap());
    assert_eq!(
        store.dispatch(&dispatch.id).await.unwrap(),
        Some(dispatch.clone())
    );
    assert_eq!(
        store
            .acknowledge_dispatch(&dispatch.id, &dispatch.to, "wrong", 1)
            .await
            .unwrap(),
        DispatchAck::ShaMismatch
    );
    assert!(matches!(
        store
            .acknowledge_dispatch(&dispatch.id, &dispatch.from, sha, 1)
            .await
            .unwrap(),
        DispatchAck::NotAssignee { .. }
    ));
    store
        .mark_dispatch_delivered(&dispatch.id, expected.delivered_at.unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(store.dispatch(&dispatch.id).await.unwrap().unwrap()).unwrap(),
        event_record("dispatch-delivered")
    );
    assert_eq!(
        store
            .acknowledge_dispatch(
                &dispatch.id,
                &dispatch.to,
                sha,
                expected.acknowledged_at.unwrap()
            )
            .await
            .unwrap(),
        DispatchAck::Acknowledged
    );
    assert_eq!(
        store
            .acknowledge_dispatch(
                &dispatch.id,
                &dispatch.to,
                sha,
                expected.acknowledged_at.unwrap() + 1
            )
            .await
            .unwrap(),
        DispatchAck::AlreadyAcknowledged
    );
    assert_eq!(
        store
            .acknowledge_dispatch(&dispatch.id, &dispatch.to, "wrong", 9)
            .await
            .unwrap(),
        DispatchAck::ShaMismatch
    );
    assert!(matches!(
        store
            .acknowledge_dispatch(&dispatch.id, &dispatch.from, sha, 9)
            .await
            .unwrap(),
        DispatchAck::NotAssignee { .. }
    ));
    store
        .mark_dispatch_delivered(&dispatch.id, expected.delivered_at.unwrap() + 9)
        .await
        .unwrap();
    assert_eq!(
        store.dispatch(&dispatch.id).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        store.list_dispatches(Some(&dispatch.to)).await.unwrap(),
        vec![expected]
    );
}

#[tokio::test]
async fn queued_ack_wins_before_delivery_and_canary_requires_ack_and_is_immutable() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool);
    let dispatch: Dispatch = record("dispatch");
    let verified: Dispatch =
        serde_json::from_value(response("canary-verified", "dispatch")).unwrap();
    let canary: DispatchCanary = verified.canary.clone().unwrap();
    store.create_dispatch(&dispatch).await.unwrap();
    assert!(matches!(
        store
            .set_dispatch_canary(&dispatch.id, &canary)
            .await
            .unwrap(),
        GovernanceOutcome::Conflict { .. }
    ));
    store
        .acknowledge_dispatch(
            &dispatch.id,
            &dispatch.to,
            dispatch.packet_sha256.as_deref().unwrap(),
            verified.acknowledged_at.unwrap(),
        )
        .await
        .unwrap();
    let acked = store.dispatch(&dispatch.id).await.unwrap().unwrap();
    assert_eq!(acked.state, DispatchState::Acked);
    assert_eq!(
        acked.delivered_at, None,
        "ack does not fabricate a delivery receipt"
    );
    store
        .mark_dispatch_delivered(&dispatch.id, verified.delivered_at.unwrap())
        .await
        .unwrap();
    assert!(matches!(
        store
            .set_dispatch_canary(&dispatch.id, &canary)
            .await
            .unwrap(),
        GovernanceOutcome::Changed(_)
    ));
    assert!(matches!(
        store
            .set_dispatch_canary(&dispatch.id, &canary)
            .await
            .unwrap(),
        GovernanceOutcome::Unchanged(_)
    ));
    let mut conflicting = canary;
    conflicting.nonce.push_str("-other");
    assert!(matches!(
        store
            .set_dispatch_canary(&dispatch.id, &conflicting)
            .await
            .unwrap(),
        GovernanceOutcome::Conflict { .. }
    ));
    assert_eq!(store.dispatch(&dispatch.id).await.unwrap(), Some(verified));
}

#[tokio::test]
async fn tasks_and_attestations_survive_reopen_and_close_is_first_writer() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    store.create_project(&record("project")).await.unwrap();
    let task: TaskAssignment = record("task");
    let closed: TaskAssignment = serde_json::from_value(response("task-close", "task")).unwrap();
    assert!(store.open_task(&task).await.unwrap());
    assert!(!store.open_task(&task).await.unwrap());
    assert_eq!(
        store
            .close_task(&task.id, TaskCloseReason::Done, closed.closed_at.unwrap())
            .await
            .unwrap(),
        GovernanceOutcome::Changed(closed.clone())
    );
    assert_eq!(
        store
            .close_task(
                &task.id,
                TaskCloseReason::Done,
                closed.closed_at.unwrap() + 1
            )
            .await
            .unwrap(),
        GovernanceOutcome::Unchanged(closed.clone())
    );
    assert!(matches!(
        store
            .close_task(
                &task.id,
                TaskCloseReason::Failed,
                closed.closed_at.unwrap() + 1
            )
            .await
            .unwrap(),
        GovernanceOutcome::Conflict { .. }
    ));
    let attestation: PlanAttestation = record("attestation");
    store.put_plan_attestation(&attestation).await.unwrap();
    drop(store);
    pool.close().await;
    let reopened = SqliteOrchestration::new(pij_store::open(&fresh.path()).await.unwrap());
    assert_eq!(reopened.task(&task.id).await.unwrap(), Some(closed.clone()));
    assert_eq!(
        reopened.list_tasks(Some(&task.node_id)).await.unwrap(),
        vec![closed]
    );
    assert_eq!(
        reopened.plan_attestation(&attestation.seat).await.unwrap(),
        Some(attestation)
    );
}

#[tokio::test]
async fn baton_request_evidence_survives_grant_and_stale_release_cannot_take_new_lease() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    let baton: BatonDefinition = record("baton");
    let request: BatonRequest = record("baton_request");
    let expected: BatonLease = record("baton_lease");
    assert!(store.define_baton(&baton).await.unwrap());
    assert_eq!(
        store.request_baton(&request).await.unwrap(),
        GovernanceOutcome::Changed(request.clone())
    );
    assert_eq!(
        store.request_baton(&request).await.unwrap(),
        GovernanceOutcome::Unchanged(request.clone())
    );
    assert!(
        store.claim_lease(&expected).await.is_err(),
        "a request cannot bypass atomic grant"
    );
    assert_eq!(store.lease(&baton.name).await.unwrap(), None);
    let mut altered = request.clone();
    altered.pin = Some("different".into());
    assert!(matches!(
        store.request_baton(&altered).await.unwrap(),
        GovernanceOutcome::Conflict { .. }
    ));
    assert_eq!(
        store
            .grant_baton(
                &baton.name,
                &request.id,
                &expected.lease_id,
                expected.acquired_at
            )
            .await
            .unwrap(),
        GovernanceOutcome::Changed(expected.clone())
    );
    assert!(
        !store
            .release_lease(&baton.name, &expected.holder, &expected.lease_id, 1)
            .await
            .unwrap(),
        "request-backed release requires lifecycle/evidence handling"
    );
    assert_eq!(
        store.lease(&baton.name).await.unwrap(),
        Some(expected.clone())
    );
    let stored = store.baton_request(&request.id).await.unwrap().unwrap();
    assert_eq!(stored.pin, request.pin);
    assert_eq!(stored.evidence, request.evidence);
    assert_eq!(stored.purpose, request.purpose);
    assert!(
        matches!(store.return_baton(&baton.name, &expected.holder, "stale", "receipt", 1).await,
        Err(PijError::GovernanceRefused { code, record }) if code == "E-RS-LEASE-STALE" && record == expected.lease_id)
    );
    assert!(
        matches!(store.return_baton(&baton.name, &baton.created_by, &expected.lease_id, "wrong holder", 1).await,
        Err(PijError::GovernanceRefused { code, .. }) if code == "E-RS-OWNERSHIP")
    );
    let (returned_event, returned) = store
        .return_baton(
            &baton.name,
            &expected.holder,
            &expected.lease_id,
            "receipt",
            2,
        )
        .await
        .unwrap();
    assert_eq!(returned, expected);
    assert!(returned_event.seq.is_some());
    assert_eq!(
        serde_json::from_str::<Value>(&returned_event.payload).unwrap()["record"],
        event_record("baton-return")
    );
    let mut next = request;
    next.id.push_str("-next");
    store.request_baton(&next).await.unwrap();
    assert!(matches!(
        store
            .grant_baton(&baton.name, &next.id, &expected.lease_id, 3)
            .await
            .unwrap(),
        GovernanceOutcome::Conflict {
            code: "baton-lease-id-reused"
        }
    ));
    store
        .grant_baton(&baton.name, &next.id, "next-lease", 3)
        .await
        .unwrap();
    assert!(
        matches!(store.reclaim_baton(&baton.name, &expected.lease_id, &baton.created_by, "stale", 4).await,
        Err(PijError::GovernanceRefused { code, record }) if code == "E-RS-LEASE-STALE" && record == "next-lease")
    );
    assert_eq!(
        store.lease(&baton.name).await.unwrap().unwrap().lease_id,
        "next-lease"
    );
    let (reclaimed_event, reclaimed) = store
        .reclaim_baton(&baton.name, "next-lease", &baton.created_by, "gone", 4)
        .await
        .unwrap();
    assert_eq!(reclaimed.lease_id, "next-lease");
    assert!(reclaimed_event.seq.is_some());
    let evidence: String =
        sqlx::query_scalar("SELECT evidence FROM baton_lease_history ORDER BY seq DESC LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(evidence, "gone");
    assert_eq!(store.baton(&baton.name).await.unwrap(), Some(baton.clone()));
    assert_eq!(store.list_batons().await.unwrap(), vec![baton]);
}

#[tokio::test]
async fn prime_compare_preserves_first_designation_and_retirement_metadata() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool);
    let prime: PrimeDesignation = record("prime");
    assert_eq!(
        store.designate_prime(&prime).await.unwrap(),
        GovernanceOutcome::Changed(prime.clone())
    );
    let mut rival = prime.clone();
    rival.seat = SeatId::from("another-seat");
    assert!(matches!(
        store.designate_prime(&rival).await.unwrap(),
        GovernanceOutcome::Conflict { .. }
    ));
    assert!(
        matches!(store.unset_prime(&rival.seat, &prime.designated_by, prime.designated_at).await,
        Err(PijError::GovernanceRefused { code, .. }) if code == "E-RS-PRIME-STALE")
    );
    let mut retired = prime.clone();
    retired.state = PrimeState::Retired;
    assert_eq!(
        store.retire_prime(&prime.seat).await.unwrap(),
        GovernanceOutcome::Changed(retired.clone())
    );
    assert_eq!(store.prime().await.unwrap(), Some(retired.clone()));
    let (event, removed) = store
        .unset_prime(&prime.seat, &prime.designated_by, prime.designated_at)
        .await
        .unwrap();
    assert_eq!(removed, retired);
    assert!(event.seq.is_some());
    assert_eq!(store.prime().await.unwrap(), None);
}

async fn assert_baton_spine_failure_rolls_back(reclaim: bool) {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    let baton: BatonDefinition = record("baton");
    let request: BatonRequest = record("baton_request");
    let lease: BatonLease = record("baton_lease");
    store.define_baton(&baton).await.unwrap();
    store.request_baton(&request).await.unwrap();
    store
        .grant_baton(&baton.name, &request.id, &lease.lease_id, lease.acquired_at)
        .await
        .unwrap();
    let before_request = store.baton_request(&request.id).await.unwrap();
    sqlx::query("CREATE TRIGGER abort_governance_spine BEFORE INSERT ON spine_events BEGIN SELECT RAISE(ABORT, 'injected spine append failure'); END")
        .execute(&pool).await.unwrap();
    let result = if reclaim {
        store
            .reclaim_baton(
                &baton.name,
                &lease.lease_id,
                &baton.created_by,
                "holder gone",
                lease.acquired_at,
            )
            .await
    } else {
        store
            .return_baton(
                &baton.name,
                &lease.holder,
                &lease.lease_id,
                "integration complete",
                lease.acquired_at,
            )
            .await
    };
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected spine append failure")
    );
    assert_eq!(store.lease(&baton.name).await.unwrap(), Some(lease.clone()));
    assert_eq!(
        store.baton_request(&request.id).await.unwrap(),
        before_request
    );
    let history_count: i64 = sqlx::query_scalar("SELECT count(*) FROM baton_lease_history")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        history_count, 1,
        "failed deletion rolls back its release history too"
    );
    assert!(
        SqliteSpine::new(pool.clone())
            .tail(None, Seq(0))
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DROP TRIGGER abort_governance_spine")
        .execute(&pool)
        .await
        .unwrap();
    let (event, deleted) = if reclaim {
        store
            .reclaim_baton(
                &baton.name,
                &lease.lease_id,
                &baton.created_by,
                "holder gone",
                lease.acquired_at,
            )
            .await
            .unwrap()
    } else {
        store
            .return_baton(
                &baton.name,
                &lease.holder,
                &lease.lease_id,
                "integration complete",
                lease.acquired_at,
            )
            .await
            .unwrap()
    };
    assert_eq!(deleted, lease);
    assert_eq!(event.at, lease.acquired_at);
    assert_eq!(
        event.seat.as_ref(),
        Some(if reclaim {
            &baton.created_by
        } else {
            &lease.holder
        })
    );
    assert_eq!(
        event.kind,
        if reclaim {
            "baton.reclaimed"
        } else {
            "baton.returned"
        }
    );
    assert_eq!(
        serde_json::from_str::<Value>(&event.payload).unwrap()["record"],
        event_record(if reclaim {
            "baton-reclaim"
        } else {
            "baton-return"
        })
    );
    assert_eq!(store.lease(&baton.name).await.unwrap(), None);
    assert_eq!(
        SqliteSpine::new(pool.clone())
            .tail(None, Seq(0))
            .await
            .unwrap(),
        vec![event.clone()]
    );
    let repeated = if reclaim {
        store
            .reclaim_baton(
                &baton.name,
                &lease.lease_id,
                &baton.created_by,
                "replay",
                lease.acquired_at,
            )
            .await
    } else {
        store
            .return_baton(
                &baton.name,
                &lease.holder,
                &lease.lease_id,
                "replay",
                lease.acquired_at,
            )
            .await
    };
    assert!(
        matches!(repeated, Err(PijError::GovernanceRefused { code, record })
        if code == "E-RS-LEASE-STALE" && record == "absent")
    );
    assert_eq!(
        SqliteSpine::new(pool).tail(None, Seq(0)).await.unwrap(),
        vec![event]
    );
}

#[tokio::test]
async fn baton_return_spine_failure_rolls_back_lease_request_and_history() {
    assert_baton_spine_failure_rolls_back(false).await;
}

#[tokio::test]
async fn baton_reclaim_spine_failure_rolls_back_lease_request_and_history() {
    assert_baton_spine_failure_rolls_back(true).await;
}

#[tokio::test]
async fn baton_return_without_evidence_commits_null_history_while_reclaim_requires_evidence() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    let baton: BatonDefinition = record("baton");
    let request: BatonRequest = record("baton_request");
    let lease: BatonLease = record("baton_lease");
    store.define_baton(&baton).await.unwrap();
    store.request_baton(&request).await.unwrap();
    store
        .grant_baton(&baton.name, &request.id, &lease.lease_id, lease.acquired_at)
        .await
        .unwrap();
    let before_request = store.baton_request(&request.id).await.unwrap();
    for absent in ["", " \t"] {
        let error = store
            .reclaim_baton(
                &baton.name,
                &lease.lease_id,
                &baton.created_by,
                absent,
                lease.acquired_at + 1,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("requires evidence"));
    }
    assert_eq!(store.lease(&baton.name).await.unwrap(), Some(lease.clone()));
    assert_eq!(
        store.baton_request(&request.id).await.unwrap(),
        before_request
    );
    assert!(
        SqliteSpine::new(pool.clone())
            .tail(None, Seq(0))
            .await
            .unwrap()
            .is_empty()
    );
    let (event, returned) = store
        .return_baton(
            &baton.name,
            &lease.holder,
            &lease.lease_id,
            "",
            lease.acquired_at + 1,
        )
        .await
        .unwrap();
    assert_eq!(returned, lease);
    assert_eq!(store.lease(&baton.name).await.unwrap(), None);
    let evidence: Option<String> =
        sqlx::query_scalar("SELECT evidence FROM baton_lease_history WHERE action='released'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        evidence, None,
        "absence stays absent rather than becoming synthetic evidence"
    );
    let returned_request = store.baton_request(&request.id).await.unwrap().unwrap();
    assert_eq!(returned_request.evidence, request.evidence);
    assert_eq!(returned_request.pin, request.pin);
    assert_eq!(
        returned_request.state,
        pij_core::orchestration::BatonRequestState::Returned
    );
    assert_eq!(
        serde_json::from_str::<Value>(&event.payload).unwrap()["record"],
        event_record("baton-return")
    );
    assert_eq!(
        SqliteSpine::new(pool).tail(None, Seq(0)).await.unwrap(),
        vec![event]
    );
}

#[tokio::test]
async fn prime_unset_spine_failure_preserves_designation_and_retry_commits_exact_event() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    let prime: PrimeDesignation = record("prime");
    store.designate_prime(&prime).await.unwrap();
    sqlx::query("CREATE TRIGGER abort_governance_spine BEFORE INSERT ON spine_events BEGIN SELECT RAISE(ABORT, 'injected spine append failure'); END")
        .execute(&pool).await.unwrap();
    let error = store
        .unset_prime(&prime.seat, &prime.designated_by, prime.designated_at)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected spine append failure"));
    assert_eq!(store.prime().await.unwrap(), Some(prime.clone()));
    assert!(
        SqliteSpine::new(pool.clone())
            .tail(None, Seq(0))
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DROP TRIGGER abort_governance_spine")
        .execute(&pool)
        .await
        .unwrap();
    let (event, deleted) = store
        .unset_prime(&prime.seat, &prime.designated_by, prime.designated_at)
        .await
        .unwrap();
    assert_eq!(deleted, prime);
    assert_eq!(event.at, prime.designated_at);
    assert_eq!(event.seat.as_ref(), Some(&prime.seat));
    assert_eq!(event.kind, "prime-set");
    assert_eq!(
        serde_json::from_str::<Value>(&event.payload).unwrap(),
        serde_json::json!({
            "actor": prime.designated_by, "action": "unset", "record": event_record("prime-unset")
        })
    );
    assert_eq!(store.prime().await.unwrap(), None);
    assert_eq!(
        SqliteSpine::new(pool.clone())
            .tail(None, Seq(0))
            .await
            .unwrap(),
        vec![event.clone()]
    );
    assert!(
        matches!(store.unset_prime(&prime.seat, &prime.designated_by, prime.designated_at).await,
        Err(PijError::GovernanceRefused { code, .. }) if code == "E-RS-NOT-FOUND")
    );
    assert_eq!(
        SqliteSpine::new(pool).tail(None, Seq(0)).await.unwrap(),
        vec![event]
    );
}

#[tokio::test]
async fn governance_bulk_reads_preserve_rows_and_deterministic_order_after_reopen() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    let project: Project = record("project");
    store.create_project(&project).await.unwrap();
    let baton: BatonDefinition = record("baton");
    store.define_baton(&baton).await.unwrap();
    let mut tasks = Vec::new();
    let mut dispatches = Vec::new();
    let mut requests = Vec::new();
    let mut attestations = Vec::new();
    for suffix in ["z", "a"] {
        let mut task: TaskAssignment = record("task");
        task.id.push_str(suffix);
        store.open_task(&task).await.unwrap();
        tasks.push(task);
        let mut dispatch: Dispatch = record("dispatch");
        dispatch.id.push_str(suffix);
        dispatch.msg_id.as_mut().unwrap().push_str(suffix);
        store.create_dispatch(&dispatch).await.unwrap();
        dispatches.push(dispatch);
        let mut request: BatonRequest = record("baton_request");
        request.id.push_str(suffix);
        store.request_baton(&request).await.unwrap();
        requests.push(request);
        let mut attestation: PlanAttestation = record("attestation");
        attestation.seat.0.push_str(suffix);
        store.put_plan_attestation(&attestation).await.unwrap();
        attestations.push(attestation);
    }
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    dispatches.sort_by(|left, right| left.id.cmp(&right.id));
    requests.sort_by(|left, right| left.id.cmp(&right.id));
    attestations.sort_by(|left, right| left.seat.as_str().cmp(right.seat.as_str()));
    drop(store);
    pool.close().await;
    let store = SqliteOrchestration::new(pij_store::open(&fresh.path()).await.unwrap());
    assert_eq!(store.project(&project.slug).await.unwrap(), Some(project));
    assert_eq!(store.baton(&baton.name).await.unwrap(), Some(baton.clone()));
    assert_eq!(store.list_tasks(None).await.unwrap(), tasks);
    assert_eq!(store.list_dispatches(None).await.unwrap(), dispatches);
    assert_eq!(
        store.list_baton_requests(&baton.name).await.unwrap(),
        requests
    );
    assert_eq!(store.list_plan_attestations().await.unwrap(), attestations);
    assert!(
        store
            .list_tasks(Some(&SeatId::from("unknown")))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_dispatches(Some(&SeatId::from("unknown")))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn concurrent_dispatch_ack_and_delivery_keep_one_ack_and_no_state_regression() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool);
    let dispatch: Dispatch = record("dispatch");
    store.create_dispatch(&dispatch).await.unwrap();
    let sha = dispatch.packet_sha256.as_deref().unwrap();
    let at = dispatch.created_at;
    let (first, second, delivered) = tokio::join!(
        store.acknowledge_dispatch(&dispatch.id, &dispatch.to, sha, at + 2),
        store.acknowledge_dispatch(&dispatch.id, &dispatch.to, sha, at + 3),
        store.mark_dispatch_delivered(&dispatch.id, at + 1),
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == DispatchAck::Acknowledged)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == DispatchAck::AlreadyAcknowledged)
            .count(),
        1
    );
    delivered.unwrap();
    let row = store.dispatch(&dispatch.id).await.unwrap().unwrap();
    assert_eq!(row.state, DispatchState::Acked);
    assert_eq!(row.delivered_at, Some(at + 1));
    let ack = row.ack.unwrap();
    assert!([at + 2, at + 3].contains(&ack.at));
    assert_eq!(ack.packet_sha256, sha);
    assert_eq!(row.acknowledged_at, Some(ack.at));
}

#[tokio::test]
async fn concurrent_baton_grants_commit_exactly_one_lease_and_request_transition() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.unwrap();
    let store = SqliteOrchestration::new(pool.clone());
    let baton: BatonDefinition = record("baton");
    let first: BatonRequest = record("baton_request");
    let mut second = first.clone();
    second.id.push_str("-second");
    store.define_baton(&baton).await.unwrap();
    store.request_baton(&first).await.unwrap();
    store.request_baton(&second).await.unwrap();
    let (one, two) = tokio::join!(
        store.grant_baton(&baton.name, &first.id, "lease-one", first.requested_at),
        store.grant_baton(&baton.name, &second.id, "lease-two", second.requested_at),
    );
    let outcomes = [one.unwrap(), two.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, GovernanceOutcome::Changed(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, GovernanceOutcome::Conflict { code: "baton-held" }))
            .count(),
        1
    );
    let granted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM baton_requests WHERE state='granted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let claims: i64 =
        sqlx::query_scalar("SELECT count(*) FROM baton_lease_history WHERE action='claimed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(granted, 1);
    assert_eq!(claims, 1);
}

#[tokio::test]
async fn governance_migration_preserves_legacy_dispatches_without_inventing_metadata() {
    let fresh = pij_testkit::FreshStore::new();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(fresh.path())
        .create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let schema_13 = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            pij_store::migrate::MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= 13)
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    schema_13.run(&pool).await.expect("apply historical schema");
    let mut project: Project = record("project");
    let mut stream: Stream = record("stream");
    let mut baton: BatonDefinition = record("baton");
    let prime: PrimeDesignation = record("prime");
    let mut lease: BatonLease = record("baton_lease");
    sqlx::query("INSERT INTO projects (slug, created_by, created_at) VALUES (?1, ?2, ?3)")
        .bind(&project.slug)
        .bind(project.created_by.as_str())
        .bind(project.created_at as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO streams (id, project, ordinal, slug, branch, worktree, base_ref, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")
        .bind(format!("alloc-s{:03}-{}", stream.ordinal, stream.slug)).bind(&stream.project)
        .bind(i64::from(stream.ordinal)).bind(&stream.slug).bind(&stream.branch)
        .bind(stream.worktree.to_str().unwrap()).bind(&stream.base_ref)
        .bind(stream.created_by.as_str()).bind(stream.created_at as i64).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO batons (name, description, created_by, created_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&baton.name)
    .bind(&baton.description)
    .bind(baton.created_by.as_str())
    .bind(baton.created_at as i64)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO prime_designation (singleton, seat, designated_by, designated_at) VALUES (1, ?1, ?2, ?3)")
        .bind(prime.seat.as_str()).bind(prime.designated_by.as_str()).bind(prime.designated_at as i64)
        .execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO baton_leases (baton, holder, lease_id, acquired_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&lease.baton)
    .bind(lease.holder.as_str())
    .bind(&lease.lease_id)
    .bind(lease.acquired_at as i64)
    .execute(&pool)
    .await
    .unwrap();
    let fixture_dispatch: Dispatch = record("dispatch");
    for (id, state, ack_at) in [
        ("old-pending", "pending", None),
        ("old-ack", "acknowledged", Some(7_i64)),
    ] {
        sqlx::query("INSERT INTO dispatches VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .bind(id)
            .bind(fixture_dispatch.from.as_str())
            .bind(fixture_dispatch.to.as_str())
            .bind(&fixture_dispatch.packet_path)
            .bind(state)
            .bind(fixture_dispatch.created_at as i64)
            .bind(ack_at)
            .execute(&pool)
            .await
            .unwrap();
    }
    pool.close().await;
    let upgraded = pij_store::open(&fresh.path())
        .await
        .expect("upgrade historical store");
    let store = SqliteOrchestration::new(upgraded);
    let pending = store.dispatch("old-pending").await.unwrap().unwrap();
    let acked = store.dispatch("old-ack").await.unwrap().unwrap();
    assert_eq!(pending.state, DispatchState::Queued);
    assert_eq!(acked.state, DispatchState::Acked);
    assert_eq!(acked.acknowledged_at, Some(7));
    assert_eq!(acked.packet_sha256, None);
    assert_eq!(acked.msg_id, None);
    assert_eq!(acked.ack, None);
    assert_eq!(acked.packet_path, fixture_dispatch.packet_path);
    assert_eq!(
        store
            .acknowledge_dispatch("old-ack", &fixture_dispatch.to, "invented", 8)
            .await
            .unwrap(),
        DispatchAck::ShaMismatch
    );
    project.description = None;
    project.repo = None;
    project.plan_path = None;
    project.prime_id = None;
    stream.state = StreamState::Reserved;
    baton.resource = None;
    baton.probe = None;
    baton.repo = None;
    lease.request_id = None;
    assert_eq!(store.project(&project.slug).await.unwrap(), Some(project));
    assert_eq!(store.stream(&stream.id).await.unwrap(), Some(stream));
    assert_eq!(store.baton(&baton.name).await.unwrap(), Some(baton));
    assert_eq!(store.lease(&lease.baton).await.unwrap(), Some(lease));
    assert_eq!(store.prime().await.unwrap(), Some(prime));
}
