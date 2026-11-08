//! Plan 224's idle-nudge qualification against a real store (F144).

mod support;

use pij_core::model::{Harness, SeatDescriptor, SeatId};
use pij_core::ports::Registry;
use pij_store::{SqliteOrchestration, SqliteRegistry};
use pij_testkit::FreshStore;

/// F144: only real in-flight work qualifies: a per-task instance holding,
/// an open plan or fix claim, or a delivered request not yet answered.
#[tokio::test]
async fn in_flight_counts_only_task_work() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open");
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    for seat in ["pij-coder", "pij-resting", "pij-named", "pij-fixer"] {
        registry
            .put(SeatDescriptor::new(seat, Harness::Omp, "/abs/tree"))
            .await
            .unwrap();
    }
    let store = SqliteOrchestration::new(pool.clone());
    for (seat, role, instance) in [
        ("pij-coder", "coder", "224"),
        ("pij-resting", "docs-system", ""),
        ("pij-named", "reviewer", "questions-context"),
        ("pij-fixer", "security", "f12"),
    ] {
        store
            .record_assignment_committed(
                &pij_store::Assignment {
                    project: "crane".into(),
                    role: role.into(),
                    instance: instance.into(),
                    seat: seat.into(),
                    harness: None,
                    session: None,
                    machine: None,
                    from: "2026-11-08T00:00:00Z".into(),
                    to: None,
                    assigned_by: "pij-pm".into(),
                    released_by: None,
                    why: "test".into(),
                    release_why: None,
                    end_pending: false,
                },
                Some("worker"),
                1,
            )
            .await
            .expect("holding");
    }
    for (seat, kind) in [
        ("pij-planner", "plan"),
        ("pij-streamer", "workstream"),
        ("pij-migrator", "migration"),
    ] {
        store
            .claim_number("crane", kind, &SeatId::from(seat), "work", 1, 1)
            .await
            .expect("claim");
    }
    for (id, holder, state, answered) in [
        ("r1", Some("pij-asked"), "delivered", None),
        ("r2", Some("pij-answered"), "delivered", Some(2_i64)),
        ("r3", None, "queued", None),
    ] {
        sqlx::query(
            "INSERT INTO role_requests (id, project, role, instance, requester, body, reply, \
             state, holder, created_at, delivered_at, answered_at) \
             VALUES (?1, 'crane', 'delivery', '7', 'pij-pm', 'x', 'wake', ?2, ?3, 1, 1, ?4)",
        )
        .bind(id)
        .bind(state)
        .bind(holder)
        .bind(answered)
        .execute(&pool)
        .await
        .expect("request");
    }
    let in_flight = store.in_flight().await.expect("read");
    assert_eq!(
        in_flight
            .seats
            .iter()
            .map(SeatId::as_str)
            .collect::<Vec<_>>(),
        ["pij-asked", "pij-coder", "pij-fixer", "pij-planner"]
    );
    assert_eq!(
        in_flight
            .requests
            .keys()
            .map(SeatId::as_str)
            .collect::<Vec<_>>(),
        ["pij-asked"]
    );
    assert_eq!(
        in_flight.requests[&SeatId::from("pij-asked")],
        [pij_store::OpenRequest {
            project: "crane".into(),
            role: "delivery".into(),
            instance: "7".into(),
        }]
    );
}
