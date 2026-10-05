//! Durable background lifecycle contracts against a fresh, real SQLite store.

use pij_core::background::{BackgroundJob, BackgroundState};
use pij_core::error::PijError;
use pij_core::model::{ProcIdentity, SeatId};
use pij_store::StorePool;
use pij_store::background::SqliteBackground;
use pij_testkit::FreshStore;

fn queued(id: &str) -> BackgroundJob {
    BackgroundJob {
        job_id: id.to_string(),
        owner: SeatId::from("pij-owner"),
        title: "compile the project".to_string(),
        command: "printf 'hello\\n'".to_string(),
        pid: None,
        proc_start: None,
        pgid: None,
        out_path: format!("/isolated/bg/{id}.log"),
        state: BackgroundState::Queued,
        exit_code: None,
        started_at: 1_000,
        finished_at: None,
        kill_requested: false,
        notified: false,
        deadline_at: None,
        timed_out: false,
        term_sent: false,
    }
}

async fn setup() -> (FreshStore, StorePool, SqliteBackground) {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open store");
    let background = SqliteBackground::new(pool.clone());
    (fresh, pool, background)
}

#[tokio::test]
async fn background_pending_excludes_notified_history_and_preserves_reconciliation_order() {
    let (fresh, pool, background) = setup().await;
    let mut live = queued("queued");
    live.started_at = 1_010;
    background.insert(&live).await.expect("queue live job");
    background
        .insert(&queued("running"))
        .await
        .expect("queue running job");
    background
        .start(
            "running",
            &ProcIdentity {
                pid: 123,
                proc_start: 456,
            },
            123,
        )
        .await
        .expect("start");
    for (state, suffix) in [
        (BackgroundState::Done, "done"),
        (BackgroundState::Killed, "killed"),
        (BackgroundState::Lost, "lost"),
    ] {
        for notified in [false, true] {
            let id = format!("{}-{suffix}", if notified { "history" } else { "pending" });
            let mut job = queued(&id);
            if notified {
                job.started_at = 1;
            }
            background.insert(&job).await.expect("queue terminal job");
            background
                .finish(&id, state, None, 1_020)
                .await
                .expect("finish");
            if notified {
                background.mark_notified(&id).await.expect("ack history");
            }
        }
    }
    assert_eq!(
        background
            .list()
            .await
            .expect("full history remains visible")
            .len(),
        8
    );
    let expected = [
        "pending-done",
        "pending-killed",
        "pending-lost",
        "running",
        "queued",
    ];
    let pending = background.pending().await.expect("pending work only");
    assert_eq!(
        pending
            .iter()
            .map(|job| job.job_id.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    pool.close().await;

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("restart reconciliation");
    let background = SqliteBackground::new(pool.clone());
    assert_eq!(
        background
            .pending()
            .await
            .expect("pending survives restart"),
        pending
    );
    for id in ["pending-done", "pending-killed", "pending-lost"] {
        background
            .mark_notified(id)
            .await
            .expect("ack pending terminal");
    }
    let remaining = background.pending().await.expect("only live work remains");
    assert_eq!(
        remaining
            .iter()
            .map(|job| job.job_id.as_str())
            .collect::<Vec<_>>(),
        ["running", "queued"]
    );
    pool.close().await;
}

#[tokio::test]
async fn background_migration_17_is_applied_and_reopening_preserves_lifecycle() {
    let (fresh, pool, background) = setup().await;
    let mut expected = queued("bg-persistent");
    background.insert(&expected).await.expect("queue job");
    assert_eq!(
        background.get(&expected.job_id).await.expect("get"),
        Some(expected.clone())
    );

    let identity = ProcIdentity {
        pid: 41_337,
        proc_start: 20260907120000,
    };
    assert!(
        background
            .start(&expected.job_id, &identity, 41_337)
            .await
            .expect("start")
    );
    expected.pid = Some(identity.pid);
    expected.proc_start = Some(identity.proc_start);
    expected.pgid = Some(41_337);
    expected.state = BackgroundState::Running;
    pool.close().await;

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("restart running");
    let background = SqliteBackground::new(pool.clone());
    assert_eq!(
        background.get(&expected.job_id).await.expect("get"),
        Some(expected.clone())
    );
    assert!(
        background
            .request_kill(&expected.job_id)
            .await
            .expect("request kill")
    );
    expected.kill_requested = true;
    pool.close().await;

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("restart kill intent");
    let background = SqliteBackground::new(pool.clone());
    assert_eq!(
        background.get(&expected.job_id).await.expect("get"),
        Some(expected.clone())
    );
    assert!(
        background
            .finish(&expected.job_id, BackgroundState::Killed, Some(143), 1_020)
            .await
            .expect("finish")
    );
    expected.state = BackgroundState::Killed;
    expected.exit_code = Some(143);
    expected.finished_at = Some(1_020);
    pool.close().await;

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("restart pending notification");
    let background = SqliteBackground::new(pool.clone());
    assert_eq!(
        background
            .list()
            .await
            .expect("pending notification survives"),
        vec![expected.clone()]
    );
    background
        .mark_notified(&expected.job_id)
        .await
        .expect("notification delivered");
    expected.notified = true;
    pool.close().await;

    let pool = pij_store::open(&fresh.path())
        .await
        .expect("restart notified");
    assert_eq!(
        SqliteBackground::new(pool.clone())
            .list()
            .await
            .expect("list"),
        vec![expected]
    );
    pool.close().await;
}

#[tokio::test]
async fn background_transitions_are_conditional_and_terminal_rows_never_reopen() {
    let (_fresh, pool, background) = setup().await;
    let job = queued("bg-conditional");
    let identity = ProcIdentity {
        pid: 123,
        proc_start: 456,
    };
    background.insert(&job).await.expect("insert");
    assert!(
        background.insert(&job).await.is_err(),
        "duplicate IDs never overwrite history"
    );
    assert!(
        !background
            .request_kill(&job.job_id)
            .await
            .expect("queued cannot be killed")
    );
    background
        .mark_notified(&job.job_id)
        .await
        .expect("queued notification no-op");
    assert!(
        !background
            .get(&job.job_id)
            .await
            .expect("get")
            .expect("job")
            .notified
    );
    assert!(
        background
            .start(&job.job_id, &identity, 123)
            .await
            .expect("first start")
    );
    let recycled = ProcIdentity {
        pid: 123,
        proc_start: 789,
    };
    assert!(
        !background
            .start(&job.job_id, &recycled, 123)
            .await
            .expect("cannot replace identity")
    );
    assert!(
        background
            .request_kill(&job.job_id)
            .await
            .expect("first intent")
    );
    assert!(
        !background
            .request_kill(&job.job_id)
            .await
            .expect("duplicate intent")
    );
    background
        .mark_notified(&job.job_id)
        .await
        .expect("running notification no-op");
    assert!(
        !background
            .get(&job.job_id)
            .await
            .expect("get")
            .expect("job")
            .notified
    );
    for nonterminal in [BackgroundState::Queued, BackgroundState::Running] {
        assert!(
            background
                .finish(&job.job_id, nonterminal, None, 1_010)
                .await
                .is_err()
        );
    }
    assert!(
        background
            .finish(&job.job_id, BackgroundState::Done, Some(0), 1_010)
            .await
            .expect("done")
    );
    let terminal = background
        .get(&job.job_id)
        .await
        .expect("get")
        .expect("job");
    assert_eq!(terminal.proc_start, Some(identity.proc_start));
    assert!(
        terminal.kill_requested,
        "completion must not erase recorded kill intent"
    );
    for state in [
        BackgroundState::Done,
        BackgroundState::Killed,
        BackgroundState::Lost,
    ] {
        assert!(
            !background
                .finish(&job.job_id, state, Some(99), 2_000)
                .await
                .expect("late completion")
        );
    }
    assert!(
        !background
            .start(&job.job_id, &identity, 123)
            .await
            .expect("late start")
    );
    assert!(
        !background
            .request_kill(&job.job_id)
            .await
            .expect("late kill")
    );
    assert_eq!(
        background.get(&job.job_id).await.expect("get"),
        Some(terminal)
    );
    background
        .mark_notified(&job.job_id)
        .await
        .expect("mark terminal");
    background
        .mark_notified(&job.job_id)
        .await
        .expect("idempotent notification");
    assert!(
        background
            .get(&job.job_id)
            .await
            .expect("get")
            .expect("job")
            .notified
    );
    assert!(
        background
            .get("missing")
            .await
            .expect("missing get")
            .is_none()
    );
    assert!(
        !background
            .start("missing", &identity, 123)
            .await
            .expect("missing start")
    );
    assert!(
        !background
            .request_kill("missing")
            .await
            .expect("missing kill")
    );
    assert!(
        !background
            .finish("missing", BackgroundState::Lost, None, 2_000)
            .await
            .expect("missing finish")
    );
    background
        .mark_notified("missing")
        .await
        .expect("missing notification");
    pool.close().await;
}

#[tokio::test]
async fn background_competing_terminal_transitions_have_exactly_one_winner() {
    let (_fresh, pool, background) = setup().await;
    let job = queued("bg-race");
    background.insert(&job).await.expect("insert");
    background
        .start(
            &job.job_id,
            &ProcIdentity {
                pid: 123,
                proc_start: 456,
            },
            123,
        )
        .await
        .expect("start");
    let competing = SqliteBackground::new(pool.clone());
    let (done, lost) = tokio::join!(
        background.finish(&job.job_id, BackgroundState::Done, Some(7), 1_020),
        competing.finish(&job.job_id, BackgroundState::Lost, None, 1_030),
    );
    let done = done.expect("done transition");
    let lost = lost.expect("lost transition");
    assert_ne!(done, lost, "one SQL compare-and-set wins");
    let terminal = background
        .get(&job.job_id)
        .await
        .expect("get")
        .expect("job");
    if done {
        assert_eq!(
            (terminal.state, terminal.exit_code, terminal.finished_at),
            (BackgroundState::Done, Some(7), Some(1_020))
        );
    } else {
        assert_eq!(
            (terminal.state, terminal.exit_code, terminal.finished_at),
            (BackgroundState::Lost, None, Some(1_030))
        );
    }
    assert!(!terminal.notified);
    pool.close().await;
}

#[tokio::test]
async fn background_list_keeps_all_owners_and_lifecycle_states_for_daemon_filtering() {
    let (_fresh, pool, background) = setup().await;
    for (index, state) in [
        BackgroundState::Queued,
        BackgroundState::Running,
        BackgroundState::Done,
        BackgroundState::Killed,
        BackgroundState::Lost,
    ]
    .into_iter()
    .enumerate()
    {
        let mut job = queued(&format!("bg-{index}"));
        job.owner = SeatId(format!("owner-{index}"));
        background.insert(&job).await.expect("insert");
        if state == BackgroundState::Running {
            background
                .start(
                    &job.job_id,
                    &ProcIdentity {
                        pid: 100,
                        proc_start: 200,
                    },
                    100,
                )
                .await
                .expect("start");
        } else if state != BackgroundState::Queued {
            assert!(
                background
                    .finish(&job.job_id, state, None, 1_050)
                    .await
                    .expect("finish queued")
            );
        }
    }
    let jobs = background.list().await.expect("list every lifecycle");
    assert_eq!(jobs.len(), 5);
    for (index, state) in [
        BackgroundState::Queued,
        BackgroundState::Running,
        BackgroundState::Done,
        BackgroundState::Killed,
        BackgroundState::Lost,
    ]
    .into_iter()
    .enumerate()
    {
        let job = jobs
            .iter()
            .find(|job| job.job_id == format!("bg-{index}"))
            .expect("job retained");
        assert_eq!(job.state, state);
        assert_eq!(job.owner.as_str(), format!("owner-{index}"));
    }
    pool.close().await;
}

#[tokio::test]
async fn background_schema_rejects_partial_identity_and_impossible_lifecycle_facts() {
    let (_fresh, pool, background) = setup().await;
    let mut invalid = queued("partial-pid");
    invalid.pid = Some(123);
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("partial-start");
    invalid.proc_start = Some(456);
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("partial-group");
    invalid.pgid = Some(123);
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("running-without-identity");
    invalid.state = BackgroundState::Running;
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("done-without-time");
    invalid.state = BackgroundState::Done;
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("queued-with-finish");
    invalid.finished_at = Some(1_020);
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("queued-with-exit");
    invalid.exit_code = Some(0);
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("queued-with-notification");
    invalid.notified = true;
    assert!(background.insert(&invalid).await.is_err());
    invalid = queued("overflowing-time");
    invalid.started_at = u64::MAX;
    assert!(background.insert(&invalid).await.is_err());
    assert!(
        background
            .list()
            .await
            .expect("invalid rows not persisted")
            .is_empty()
    );
    let job = queued("valid");
    background.insert(&job).await.expect("valid row");
    for assignment in [
        "state='unknown'",
        "kill_requested=2",
        "notified=2",
        "started_at=-1",
    ] {
        assert!(
            sqlx::query(&format!(
                "UPDATE background_jobs SET {assignment} WHERE job_id='valid'"
            ))
            .execute(&pool)
            .await
            .is_err(),
            "SQL must reject {assignment}"
        );
    }
    assert!(
        background
            .start(
                &job.job_id,
                &ProcIdentity {
                    pid: 123,
                    proc_start: u64::MAX
                },
                123
            )
            .await
            .is_err()
    );
    assert_eq!(
        background.get(&job.job_id).await.expect("unchanged"),
        Some(job)
    );
    pool.close().await;
}

#[tokio::test]
async fn background_every_operation_refuses_schema_skew() {
    let (_fresh, pool, background) = setup().await;
    let job = queued("bg-skew");
    background.insert(&job).await.expect("insert before skew");
    for version in [16, 99] {
        sqlx::query(&format!("PRAGMA user_version = {version}"))
            .execute(&pool)
            .await
            .expect("change schema cache");
        assert!(matches!(
            background.insert(&queued("other")).await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background.get(&job.job_id).await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background.list().await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background.pending().await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background
                .start(
                    &job.job_id,
                    &ProcIdentity {
                        pid: 123,
                        proc_start: 456
                    },
                    123
                )
                .await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background.request_kill(&job.job_id).await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background
                .finish(&job.job_id, BackgroundState::Lost, None, 1_020)
                .await,
            Err(PijError::StoreSchemaStale { .. })
        ));
        assert!(matches!(
            background.mark_notified(&job.job_id).await,
            Err(PijError::StoreSchemaStale { .. })
        ));
    }
    pool.close().await;
}

#[test]
fn background_json_uses_snake_case_and_round_trips_identity() {
    let mut job = queued("bg-json");
    job.pid = Some(123);
    job.proc_start = Some(456);
    job.pgid = Some(123);
    job.state = BackgroundState::Running;
    let json = serde_json::to_value(&job).expect("serialize job");
    assert_eq!(json["job_id"], "bg-json");
    assert_eq!(json["owner"], "pij-owner");
    assert_eq!(json["proc_start"], 456);
    assert_eq!(json["out_path"], "/isolated/bg/bg-json.log");
    assert_eq!(json["kill_requested"], false);
    assert_eq!(
        serde_json::from_value::<BackgroundJob>(json).expect("deserialize job"),
        job
    );
    for (state, wire) in [
        (BackgroundState::Queued, "queued"),
        (BackgroundState::Running, "running"),
        (BackgroundState::Done, "done"),
        (BackgroundState::Killed, "killed"),
        (BackgroundState::Lost, "lost"),
    ] {
        assert_eq!(serde_json::to_value(state).expect("state wire"), wire);
        assert_eq!(
            serde_json::from_value::<BackgroundState>(serde_json::json!(wire))
                .expect("state parse"),
            state
        );
    }
}

#[tokio::test]
async fn background_timeout_fires_once_only_after_the_deadline_and_never_over_a_caller_kill() {
    let (_fresh, _pool, background) = setup().await;
    let identity = ProcIdentity {
        pid: 321,
        proc_start: 654,
    };
    for id in ["timed", "killed"] {
        let mut job = queued(id);
        job.deadline_at = Some(5_000);
        background.insert(&job).await.expect("insert");
        assert!(background.start(id, &identity, 321).await.expect("start"));
    }
    assert!(!background.request_timeout("timed", 4_999).await.unwrap());
    assert!(background.request_timeout("timed", 5_000).await.unwrap());
    assert!(!background.request_timeout("timed", 9_000).await.unwrap());
    let timed = background.get("timed").await.unwrap().unwrap();
    assert!(timed.kill_requested && timed.timed_out);
    assert_eq!(timed.deadline_at, Some(5_000));
    assert!(background.request_kill("killed").await.unwrap());
    assert!(!background.request_timeout("killed", 9_000).await.unwrap());
    assert!(!background.get("killed").await.unwrap().unwrap().timed_out);
}

#[tokio::test]
async fn background_term_provenance_needs_kill_intent_and_a_live_job() {
    let (_fresh, _pool, background) = setup().await;
    let identity = ProcIdentity {
        pid: 321,
        proc_start: 654,
    };
    background.insert(&queued("job")).await.unwrap();
    assert!(background.start("job", &identity, 321).await.unwrap());
    assert!(
        !background.set_term_sent("job", true).await.unwrap(),
        "no kill intent: nothing to prove"
    );
    assert!(background.request_kill("job").await.unwrap());
    assert!(background.set_term_sent("job", true).await.unwrap());
    assert!(background.get("job").await.unwrap().unwrap().term_sent);
    assert!(background.set_term_sent("job", false).await.unwrap());
    assert!(!background.get("job").await.unwrap().unwrap().term_sent);
    background
        .finish("job", BackgroundState::Killed, Some(143), 2_000)
        .await
        .unwrap();
    assert!(
        !background.set_term_sent("job", true).await.unwrap(),
        "a finished job's provenance is frozen"
    );
}
