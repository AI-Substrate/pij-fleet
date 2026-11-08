use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::idle_nudge::IDLE_NUDGE_AFTER_MS;
use pij_core::model::{Event, Harness, Msg, SeatDescriptor, SeatId, SystemState};
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeTmux;

use crate::pa_watchdog::{PaWatchdog, RoundOutcome};

const INTERVAL: u64 = 1_200;
const MINUTE: u64 = 60_000;
const SEAT: &str = "pij-merger";
const PARENT: &str = "pij-pm";
const PRIME: &str = "pij-prime";

async fn services() -> (crate::Services, FreshStore, Config, String) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let store = FreshStore::new();
    let config = Config {
        adapters: Adapters {
            registry: AdapterChoice::Real,
            spine: AdapterChoice::Real,
            queue: AdapterChoice::Real,
            ..Adapters::default()
        },
        store_path: store.path(),
        ..Config::default()
    };
    let mut services = build(&config, &store).await;
    services.tmux = Arc::new(FakeTmux::new());
    let folder = std::env::temp_dir().join(format!(
        "pij-idle-nudge-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&folder).unwrap();
    (services, store, config, folder.display().to_string())
}

async fn build(config: &Config, store: &FreshStore) -> crate::Services {
    crate::build_services(config, &Path::new(&store.path()).with_extension("signals"))
        .await
        .expect("real store services")
}

/// The PM, the prime, and the PM's idle Copilot merger.
async fn fleet(services: &crate::Services, folder: &str) {
    let mut pm = SeatDescriptor::new(PARENT, Harness::Claude, folder);
    pm.pane = Some("%20".into());
    let mut prime = SeatDescriptor::new(PRIME, Harness::Claude, folder);
    prime.pane = Some("%22".into());
    let mut merger = SeatDescriptor::new(SEAT, Harness::Copilot, folder);
    merger.parent = Some(SeatId::from(PARENT));
    merger.pane = Some("%21".into());
    merger.state = SystemState::Idle;
    for seat in [pm, prime, merger] {
        services.registry.put(seat).await.unwrap();
    }
}

/// `role[/instance]` held by the merger in `project`.
async fn hold(services: &crate::Services, project: &str, role: &str, instance: &str) {
    services
        .watchdogs()
        .record_assignment_committed(
            &pij_store::Assignment {
                project: project.into(),
                role: role.into(),
                instance: instance.into(),
                seat: SEAT.into(),
                harness: Some("copilot".into()),
                session: None,
                machine: None,
                from: "2026-11-08T00:00:00Z".into(),
                to: None,
                assigned_by: PARENT.into(),
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

/// One of the merger's own events at `at`.
async fn own(services: &crate::Services, kind: &str, at: u64, payload: serde_json::Value) {
    let _ = services
        .spine
        .append(Event {
            seq: None,
            v: 1,
            at,
            kind: kind.into(),
            seat: Some(SeatId::from(SEAT)),
            payload: payload.to_string(),
        })
        .await
        .unwrap();
}

async fn report(services: &crate::Services, at: u64) {
    own(
        services,
        "report.now",
        at,
        serde_json::json!({"did": "x", "next": "y"}),
    )
    .await;
}

fn nudges(outcomes: Vec<RoundOutcome>) -> Vec<RoundOutcome> {
    outcomes
        .into_iter()
        .filter(|outcome| matches!(outcome, RoundOutcome::IdleNudge { .. }))
        .collect()
}

/// The nudges queued for the merger, drained.
async fn sent(services: &crate::Services) -> Vec<Msg> {
    let mut messages = Vec::new();
    while let Some((job, payload)) = services
        .queue
        .claim(&[format!("delivery:{SEAT}")], "test-drain")
        .await
        .unwrap()
    {
        let msg: Msg = serde_json::from_str(&payload.payload).unwrap();
        if msg.msg_id.starts_with("idle-nudge:") {
            messages.push(msg);
        }
        services
            .queue
            .ack(job, pij_core::model::Outcome::Done)
            .await
            .unwrap();
    }
    messages
}

async fn fyis(services: &crate::Services, seat: &str) -> u64 {
    services
        .delivery
        .pending_fyi_count(&SeatId::from(seat))
        .await
        .unwrap()
}

async fn round(services: &crate::Services, now_ms: u64) -> Vec<RoundOutcome> {
    nudges(
        PaWatchdog::default()
            .round(services, INTERVAL, now_ms)
            .await
            .unwrap(),
    )
}

/// bp-0001: a holder idle past the hour is nudged once, naming what it holds,
/// and its parent (no project prime) gets one FYI; later rounds and a
/// restarted daemon send nothing more for that stretch; its next own event
/// re-arms it, and the next idle stretch is nudged once more.
#[tokio::test]
async fn an_idle_holder_is_nudged_once_per_idle_stretch() {
    let (services, store, config, folder) = services().await;
    fleet(&services, &folder).await;
    hold(&services, "crane", "merger", "").await;
    services
        .watchdogs()
        .claim_number(
            "pij-fleet",
            "plan",
            &SeatId::from(SEAT),
            "idle nudges",
            223,
            1,
        )
        .await
        .expect("claim");
    let start = crate::http::system_time_ms().unwrap();
    report(&services, start).await;
    // Idle seat.activity after the report is not the seat's own work.
    own(
        &services,
        "seat.activity",
        start + MINUTE,
        serde_json::json!({"state": "idle"}),
    )
    .await;

    assert!(
        round(&services, start + IDLE_NUDGE_AFTER_MS - MINUTE)
            .await
            .is_empty(),
        "under the hour"
    );
    let due = start + IDLE_NUDGE_AFTER_MS + MINUTE;
    assert_eq!(
        round(&services, due).await,
        vec![RoundOutcome::IdleNudge {
            seat: SEAT.into(),
            delivery: "queued".into(),
            primes: vec![(PARENT.into(), "held".into())],
        }]
    );
    let messages = sent(&services).await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from.as_str(), "pij-bg");
    assert_eq!(messages[0].command, None, "a normal message");
    assert!(
        messages[0].body.starts_with(
            "idle 61 min while holding merger (project crane) and plan 224 (project pij-fleet); \
             resume, report blocked, or release."
        ),
        "{}",
        messages[0].body
    );
    assert_eq!(fyis(&services, PARENT).await, 1);

    assert!(
        round(&services, due + MINUTE).await.is_empty(),
        "once per stretch"
    );
    let restarted = build(&config, &store).await;
    assert!(
        nudges(
            PaWatchdog::default()
                .round(&restarted, INTERVAL, due + 2 * MINUTE)
                .await
                .unwrap()
        )
        .is_empty(),
        "the mark is in the store, not in memory"
    );
    assert!(sent(&services).await.is_empty());

    // The nudge itself (a pushed message) doesn't re-arm it; an own event does.
    let back = due + 5 * MINUTE;
    report(&services, back).await;
    assert!(
        round(&services, back + IDLE_NUDGE_AFTER_MS - MINUTE)
            .await
            .is_empty()
    );
    assert_eq!(
        round(&services, back + IDLE_NUDGE_AFTER_MS + MINUTE)
            .await
            .len(),
        1,
        "the next idle stretch is nudged once more"
    );
    assert_eq!(sent(&services).await.len(), 1);
}

/// bp-0001: the project's designated prime is told, not the parent.
#[tokio::test]
async fn the_projects_designated_prime_gets_the_fyi() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    sqlx::query(
        "INSERT INTO projects (slug, created_by, created_at, prime_id) VALUES ('crane', ?1, 1, ?2)",
    )
    .bind(PARENT)
    .bind(PRIME)
    .execute(&services.store_pools[0])
    .await
    .unwrap();
    hold(&services, "crane", "merger", "").await;
    let start = crate::http::system_time_ms().unwrap();
    report(&services, start).await;
    assert_eq!(
        round(&services, start + IDLE_NUDGE_AFTER_MS + MINUTE).await,
        vec![RoundOutcome::IdleNudge {
            seat: SEAT.into(),
            delivery: "queued".into(),
            primes: vec![(PRIME.into(), "held".into())],
        }]
    );
    assert_eq!(fyis(&services, PRIME).await, 1);
    assert_eq!(fyis(&services, PARENT).await, 0);
}

/// bp-0001: each of these leaves the holder alone: a live background job it
/// owns, a message queued for it, a pending compaction wake, a seat that
/// isn't idle, and a seat that holds nothing.
#[tokio::test]
async fn work_in_hand_or_nothing_held_suppresses_the_nudge() {
    for case in ["bg-job", "queued", "compacting", "working", "holds-nothing"] {
        let (services, _store, _config, folder) = services().await;
        fleet(&services, &folder).await;
        if case != "holds-nothing" {
            hold(&services, "crane", "merger", "").await;
        }
        let start = crate::http::system_time_ms().unwrap();
        report(&services, start).await;
        match case {
            "bg-job" => {
                sqlx::query(
                    "INSERT INTO background_jobs (job_id, owner, title, command, out_path, state, started_at) \
                     VALUES ('job-1', ?1, 'build', 'true', '/tmp/out', 'queued', 1)",
                )
                .bind(SEAT)
                .execute(&services.store_pools[0])
                .await
                .unwrap();
            }
            "queued" => {
                services
                    .delivery
                    .accept(crate::pa_watchdog::bg_msg(
                        &SeatId::from(SEAT),
                        "work".into(),
                        "m-queued".into(),
                    ))
                    .await
                    .unwrap();
            }
            "compacting" => {
                // Admitted just before the round, so plan 222 still holds it.
                crate::compaction_wake::record(
                    &services,
                    &SeatId::from(SEAT),
                    "m-compact",
                    start + 2 * IDLE_NUDGE_AFTER_MS - MINUTE,
                )
                .await
                .unwrap();
            }
            "working" => {
                let mut seat = services
                    .registry
                    .get(&SeatId::from(SEAT))
                    .await
                    .unwrap()
                    .unwrap();
                seat.state = SystemState::Working;
                services.registry.put(seat).await.unwrap();
            }
            _ => {}
        }
        assert!(
            round(&services, start + 2 * IDLE_NUDGE_AFTER_MS)
                .await
                .is_empty(),
            "{case} must suppress the nudge"
        );
        assert_eq!(fyis(&services, PARENT).await, 0, "{case}");
    }
}

/// Review of #189: the stretch is checked again where the mark is written. A
/// seat that posted an own event, went busy, or was tombstoned after the
/// round read it as due gets no mark, no nudge and no FYI.
#[tokio::test]
async fn a_stretch_that_ends_before_the_mark_sends_nothing() {
    for case in ["own-event", "not-idle", "tombstoned"] {
        let (services, _store, _config, folder) = services().await;
        fleet(&services, &folder).await;
        hold(&services, "crane", "merger", "").await;
        let start = crate::http::system_time_ms().unwrap();
        report(&services, start).await;
        let now = start + IDLE_NUDGE_AFTER_MS + MINUTE;
        let seats = services
            .registry
            .list(pij_core::ports::SeatFilter::default())
            .await
            .unwrap();
        let merger = seats.iter().find(|seat| seat.id.as_str() == SEAT).unwrap();
        let due = super::due(&services, merger, now).await.expect("due");
        // Each case ends the stretch one way only, so each condition of the
        // mark is proved on its own.
        match case {
            "own-event" => report(&services, now).await,
            "not-idle" => {
                seat_row(
                    &services,
                    "UPDATE seats SET state = 'working' WHERE id = ?1",
                )
                .await
            }
            _ => {
                seat_row(
                    &services,
                    "UPDATE seats SET tombstoned_at = 1 WHERE id = ?1",
                )
                .await
            }
        }
        assert!(
            super::mark_then_nudge(&services, &seats, &due, now)
                .await
                .is_none(),
            "{case}: the mark is refused"
        );
        assert!(
            !services
                .watchdogs()
                .idle_nudged(&SeatId::from(SEAT), due.anchor)
                .await
                .unwrap(),
            "{case}: no mark"
        );
        assert!(sent(&services).await.is_empty(), "{case}: no nudge");
        assert_eq!(fyis(&services, PARENT).await, 0, "{case}: no FYI");
    }
}

async fn seat_row(services: &crate::Services, sql: &str) {
    sqlx::query(sql)
        .bind(SEAT)
        .execute(&services.store_pools[0])
        .await
        .unwrap();
}

/// The round's own snapshot decides who is a candidate: a seat it sees busy
/// is never read further, even where the stored row still says idle (the
/// mark's re-check is the second, independent layer).
#[tokio::test]
async fn a_seat_the_snapshot_sees_busy_is_not_a_candidate() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    hold(&services, "crane", "merger", "").await;
    let start = crate::http::system_time_ms().unwrap();
    report(&services, start).await;
    let now = start + IDLE_NUDGE_AFTER_MS + MINUTE;
    let mut seats = services
        .registry
        .list(pij_core::ports::SeatFilter::default())
        .await
        .unwrap();
    for seat in &mut seats {
        if seat.id.as_str() == SEAT {
            seat.state = SystemState::Working;
        }
    }
    assert!(super::round(&services, &seats, now).await.is_empty());
    assert!(sent(&services).await.is_empty());
    for seat in &mut seats {
        seat.state = SystemState::Idle;
    }
    assert_eq!(
        super::round(&services, &seats, now).await.len(),
        1,
        "otherwise due"
    );
}

/// S189-001: an FYI id squatted by an ordinary local caller neither hides the
/// daemon's notice nor reads as held. Delivery refuses and audits an id held
/// for another sender or recipient (a same-sender retry is still one FYI),
/// and the nudge's ids carry a per-occurrence part, so the old predictable id
/// taken first changes nothing: the real prime still hears.
#[tokio::test]
async fn a_squatted_fyi_id_neither_hides_nor_fakes_the_notice() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    sqlx::query(
        "INSERT INTO projects (slug, created_by, created_at, prime_id) VALUES ('crane', ?1, 1, ?2)",
    )
    .bind(PARENT)
    .bind(PRIME)
    .execute(&services.store_pools[0])
    .await
    .unwrap();
    hold(&services, "crane", "merger", "").await;
    let start = crate::http::system_time_ms().unwrap();
    report(&services, start).await;
    let anchor = services
        .watchdogs()
        .last_own_event(&SeatId::from(SEAT))
        .await
        .unwrap()
        .unwrap()
        .0;
    let mut squat = crate::pa_watchdog::bg_msg(
        &SeatId::from(PARENT),
        "not from pij".into(),
        format!("idle-nudge-fyi:{SEAT}:{anchor}:{PRIME}"),
    );
    squat.from = SeatId::from(SEAT);
    services
        .delivery
        .hold_fyi(squat.clone())
        .await
        .expect("held");
    services
        .delivery
        .hold_fyi(squat.clone())
        .await
        .expect("the same sender's retry to the same seat is the same FYI");
    let mut taken = squat.clone();
    taken.to = SeatId::from(PRIME);
    let refused = services.delivery.hold_fyi(taken).await.unwrap_err();
    assert!(
        refused.to_string().contains("E-RS-FYI-ID-TAKEN"),
        "{refused}"
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM spine_events WHERE kind = 'fyi.refused' AND seat = ?1 \
         AND json_extract(payload, '$.held_recipient') = ?2",
    )
    .bind(PRIME)
    .bind(PARENT)
    .fetch_one(&services.store_pools[0])
    .await
    .unwrap();
    assert_eq!(
        audited, 1,
        "the refusal is audited on the intended recipient"
    );
    assert_eq!(fyis(&services, PRIME).await, 0, "nothing held for it");

    assert_eq!(
        round(&services, start + IDLE_NUDGE_AFTER_MS + MINUTE).await,
        vec![RoundOutcome::IdleNudge {
            seat: SEAT.into(),
            delivery: "queued".into(),
            primes: vec![(PRIME.into(), "held".into())],
        }]
    );
    assert_eq!(
        fyis(&services, PRIME).await,
        1,
        "the real prime still hears"
    );
    let nudge = sent(&services).await;
    assert_eq!(nudge.len(), 1);
    assert_ne!(
        nudge[0].msg_id,
        format!("idle-nudge:{SEAT}:{anchor}"),
        "the nudge id is not predictable"
    );
}

/// Squirrel, #189 r2: the nudge id is published on `/v1/events` before any
/// FYI is held, and each `fyi.held` before the next prime's hold, so a
/// subscriber knows them in turn. Every prime FYI id has its own random
/// part: none follows from the nudge id or from another prime's FYI id.
#[tokio::test]
async fn each_prime_fyi_id_follows_from_nothing_published() {
    const PRIME_TWO: &str = "pij-prime-two";
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    let mut second = SeatDescriptor::new(PRIME_TWO, Harness::Claude, &folder);
    second.pane = Some("%23".into());
    services.registry.put(second).await.unwrap();
    for (project, prime) in [("crane", PRIME), ("dock", PRIME_TWO)] {
        sqlx::query(
            "INSERT INTO projects (slug, created_by, created_at, prime_id) VALUES (?1, ?2, 1, ?3)",
        )
        .bind(project)
        .bind(PARENT)
        .bind(prime)
        .execute(&services.store_pools[0])
        .await
        .unwrap();
        hold(&services, project, "merger", "").await;
    }
    let start = crate::http::system_time_ms().unwrap();
    report(&services, start).await;
    let anchor = services
        .watchdogs()
        .last_own_event(&SeatId::from(SEAT))
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(
        round(&services, start + IDLE_NUDGE_AFTER_MS + MINUTE).await,
        vec![RoundOutcome::IdleNudge {
            seat: SEAT.into(),
            delivery: "queued".into(),
            primes: vec![
                (PRIME.into(), "held".into()),
                (PRIME_TWO.into(), "held".into())
            ],
        }]
    );
    let nudge = sent(&services).await;
    assert_eq!(nudge.len(), 1);
    let fyi_id = |prime: &'static str| {
        let pool = services.store_pools[0].clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT id FROM fyis WHERE recipient = ?1")
                .bind(prime)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let (one, two) = (fyi_id(PRIME).await, fyi_id(PRIME_TWO).await);
    let random = |id: &str| id.split(':').nth(3).unwrap().to_string();
    let prefix = format!("idle-nudge-fyi:{SEAT}:{anchor}:");
    for (id, prime) in [(&one, PRIME), (&two, PRIME_TWO)] {
        assert!(
            id.starts_with(&prefix) && id.ends_with(&format!(":{prime}")),
            "{id}"
        );
        assert_eq!(random(id).len(), 32, "{id}");
    }
    let nudge_random = nudge[0].msg_id.rsplit(':').next().unwrap().to_string();
    assert_ne!(random(&one), nudge_random, "not the nudge's random part");
    assert_ne!(random(&two), nudge_random, "not the nudge's random part");
    assert_ne!(random(&one), random(&two), "not each other's random part");
}

/// bp-0001: a hostile holding label is never shown raw, to the seat or its
/// prime.
#[tokio::test]
async fn a_hostile_holding_label_renders_as_invalid() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    hold(&services, "crane", "merger", "x\u{1b}[2Jgo").await;
    let start = crate::http::system_time_ms().unwrap();
    report(&services, start).await;
    assert_eq!(
        round(&services, start + IDLE_NUDGE_AFTER_MS + MINUTE)
            .await
            .len(),
        1
    );
    let body = &sent(&services).await[0].body;
    assert!(
        body.starts_with("idle 61 min while holding merger/invalid (7 chars) (project crane);"),
        "{body}"
    );
    assert!(!body.contains('\u{1b}'), "{body}");
}
