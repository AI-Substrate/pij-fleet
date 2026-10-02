//! Status projections must come from the roster's SQLite snapshot, not per-seat IO.
mod support;

use pij_core::model::{Event, Harness, SeatDescriptor, SemanticState};
use pij_core::ports::{Registry, Spine};
use pij_store::status::SqliteStatus;
use pij_store::{SqliteRegistry, SqliteSpine};

#[tokio::test]
async fn roster_badge_includes_every_open_assignment_but_not_closed_declarations() {
    let pool = pij_store::open("").await.unwrap();
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let spine = SqliteSpine::new(pool.clone());
    let mut seat = SeatDescriptor::new("pij-status", Harness::Omp, "/fixture");
    seat.semantic_state = Some(SemanticState::Done);
    registry.put(seat.clone()).await.unwrap();
    for (id, state, at) in [("blocked-task", "blocked", 10), ("done-task", "done", 20)] {
        sqlx::query("INSERT INTO task_assignments(id,node_id,task,opened_by,opened_at) VALUES (?1,?2,?1,?2,0)")
            .bind(id).bind(seat.id.as_str()).execute(&pool).await.unwrap();
        spine.append(Event { seq: None, v: 1, at, kind: "report.state".into(), seat: Some(seat.id.clone()), payload: serde_json::json!({"state":state,"assignment_id":id,"registry_seq":1,"note":null,"refs":[]}).to_string() }).await.unwrap();
    }
    let reader = SqliteStatus::new(pool.clone(), true);
    let rows = reader.read(&registry, None).await.unwrap();
    assert_eq!(rows[0].badge(), "blocked");
    assert_eq!(rows[0].last_event_at, Some(20));
    sqlx::query(
        "UPDATE task_assignments SET closed_at=30,close_reason='done' WHERE id='blocked-task'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        reader.read(&registry, None).await.unwrap()[0].badge(),
        "done"
    );
    registry
        .tombstone(&seat.id, "operator closed")
        .await
        .unwrap();
    let rows = reader.read(&registry, Some(&seat.id)).await.unwrap();
    assert_eq!(
        rows[0].badge(),
        "done",
        "badge does not turn tombstones into liveness"
    );
}

#[tokio::test]
async fn legacy_rows_without_events_have_null_freshness_and_keep_semantics() {
    let pool = pij_store::open("").await.unwrap();
    sqlx::query("INSERT INTO seats(id,harness,folder,state,semantic_state,seq) VALUES ('pij-old','claude','/old','idle','blocked',1)").execute(&pool).await.unwrap();
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let reader = SqliteStatus::new(pool, true);
    let row = reader.read(&registry, None).await.unwrap().remove(0);
    assert_eq!(row.last_event_at, None);
    assert_eq!(row.badge(), "blocked");
}

#[tokio::test]
async fn closing_last_assignment_does_not_resurrect_descriptor_declaration() {
    let pool = pij_store::open("").await.unwrap();
    let registry = SqliteRegistry::new(pool.clone(), support::publisher());
    let spine = SqliteSpine::new(pool.clone());
    let seat = SeatDescriptor::new("pij-closed", Harness::Omp, "/fixture");
    registry.put(seat.clone()).await.unwrap();
    sqlx::query("INSERT INTO task_assignments(id,node_id,task,opened_by,opened_at) VALUES ('task',?1,'work',?1,0)")
        .bind(seat.id.as_str()).execute(&pool).await.unwrap();
    let reports =
        pij_core::report::ReportService::new(&registry, &spine, || 10, Default::default());
    reports
        .declare(
            &seat.id,
            Some(SemanticState::Blocked),
            None,
            Some("task"),
            &[],
        )
        .await
        .unwrap();
    let reader = SqliteStatus::new(pool.clone(), true);
    assert_eq!(
        reader.read(&registry, None).await.unwrap()[0].badge(),
        "blocked"
    );
    sqlx::query("UPDATE task_assignments SET closed_at=20,close_reason='done' WHERE id='task'")
        .execute(&pool)
        .await
        .unwrap();
    let row = reader.read(&registry, None).await.unwrap().remove(0);
    assert_eq!(row.seat.semantic_state, Some(SemanticState::Blocked));
    assert_eq!(
        row.badge(),
        "idle",
        "closed scope must not fall back to its stale descriptor word"
    );
}
