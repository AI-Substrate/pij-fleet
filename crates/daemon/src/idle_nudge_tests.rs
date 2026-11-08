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
