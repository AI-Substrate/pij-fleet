use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::compaction_wake::{COMPACT_DELIVERY_BOUND_MS, COMPACT_SETTLE_MS};
use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Event, Harness, Msg, SeatDescriptor, SeatId, SystemState};
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeTmux;

use crate::pa_watchdog::{PaWatchdog, RoundOutcome};

const INTERVAL: u64 = 1_200;
const MINUTE: u64 = 60_000;
const SEAT: &str = "pij-coder";
const PARENT: &str = "pij-pm";

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
        "pij-compaction-wake-{}-{}",
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

/// The PM and its idle Copilot coder.
async fn fleet(services: &crate::Services, folder: &str) {
    let mut pm = SeatDescriptor::new(PARENT, Harness::Claude, folder);
    pm.pane = Some("%20".into());
    let mut coder = SeatDescriptor::new(SEAT, Harness::Copilot, folder);
    coder.parent = Some(SeatId::from(PARENT));
    coder.pane = Some("%21".into());
    coder.state = SystemState::Idle;
    for seat in [pm, coder] {
        services.registry.put(seat).await.unwrap();
    }
}

/// `coder/222` in pij-fleet.
async fn hold_role(services: &crate::Services) {
    services
        .watchdogs()
        .record_assignment_committed(
            &pij_store::Assignment {
                project: "pij-fleet".into(),
                role: "coder".into(),
                instance: "222".into(),
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

async fn append(services: &crate::Services, kind: &str, at: u64, payload: serde_json::Value) {
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

/// A compact control admitted at `at` (recorded as `admit_control` records
/// it), pushed, then delivered a second later: wildcat's 23:02:35 to 23:03:01.
async fn compact(services: &crate::Services, msg_id: &str, at: u64) {
    crate::compaction_wake::record(services, &SeatId::from(SEAT), msg_id, at)
        .await
        .unwrap();
    append(
        services,
        "message.pushed",
        at,
        serde_json::json!({"msg_id": msg_id, "from": SEAT, "body": "", "command": "compact"}),
    )
    .await;
    append(
        services,
        crate::delivery::OUTCOME_EVENT_KIND,
        at + 1_000,
        serde_json::json!({"msg_id": msg_id, "outcome": {"outcome": "delivered", "origin": "injected-to-transport"}, "transport": "pty"}),
    )
    .await;
}

fn wakes(outcomes: Vec<RoundOutcome>) -> Vec<RoundOutcome> {
    outcomes
        .into_iter()
        .filter(|outcome| matches!(outcome, RoundOutcome::CompactionWake { .. }))
        .collect()
}

/// Messages queued for the seat, drained: the resume messages among them.
async fn resumes(services: &crate::Services) -> Vec<Msg> {
    let mut messages = Vec::new();
    while let Some((job, payload)) = services
        .queue
        .claim(&[format!("delivery:{SEAT}")], "test-drain")
        .await
        .unwrap()
    {
        let msg: Msg = serde_json::from_str(&payload.payload).unwrap();
        if msg.msg_id.starts_with("compaction-wake:") {
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

async fn settled(services: &crate::Services, msg_id: &str) -> Option<String> {
    services
        .watchdogs()
        .compaction_settled(&SeatId::from(SEAT), msg_id)
        .await
        .unwrap()
        .expect("recorded")
}

/// bp-0001: F111's shape. A holder idle after its compaction with nothing
/// queued: not inside the settle bound, then once, naming its holding and
/// claim, with an FYI to its parent; later rounds and a restarted daemon send
/// nothing more.
#[tokio::test]
async fn a_holder_left_idle_by_its_compaction_is_woken_once() {
    let (services, store, config, folder) = services().await;
    fleet(&services, &folder).await;
    hold_role(&services).await;
    services
        .watchdogs()
        .claim_number(
            "pij-fleet",
            "migration",
            &SeatId::from(SEAT),
            "compaction wakes",
            43,
            1,
        )
        .await
        .expect("claim");
    let start = crate::http::system_time_ms().unwrap();
    compact(&services, "m-compact", start).await;
    let watchdog = PaWatchdog::default();

    let settling = watchdog
        .round(&services, INTERVAL, start + COMPACT_SETTLE_MS)
        .await
        .unwrap();
    assert!(wakes(settling).is_empty(), "inside the settle bound");
    assert_eq!(settled(&services, "m-compact").await, None);

    let due = start + COMPACT_SETTLE_MS + MINUTE;
    assert_eq!(
        wakes(watchdog.round(&services, INTERVAL, due).await.unwrap()),
        vec![RoundOutcome::CompactionWake {
            seat: SEAT.into(),
            delivery: "queued".into(),
            parent: Some((PARENT.into(), "held".into())),
        }]
    );
    assert_eq!(
        settled(&services, "m-compact").await.as_deref(),
        Some("woken")
    );
    let sent = resumes(&services).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].from.as_str(), "pij-bg");
    assert_eq!(sent[0].command, None, "a normal message");
    assert!(
        sent[0].body.starts_with(
            "resume: you hold coder/222 (project pij-fleet) and migration 0044 (project pij-fleet); \
             re-read your handover and continue, or tell your parent if you're blocked."
        ),
        "{}",
        sent[0].body
    );
    assert_eq!(
        services
            .delivery
            .pending_fyi_count(&SeatId::from(PARENT))
            .await
            .unwrap(),
        1
    );

    assert!(
        wakes(
            watchdog
                .round(&services, INTERVAL, due + MINUTE)
                .await
                .unwrap()
        )
        .is_empty()
    );
    // A restart: the mark is in the store, not in memory.
    let restarted = build(&config, &store).await;
    assert!(
        wakes(
            PaWatchdog::default()
                .round(&restarted, INTERVAL, due + 2 * MINUTE)
                .await
                .unwrap()
        )
        .is_empty()
    );
    assert!(resumes(&services).await.is_empty());
}

/// bp-0001: a seat holding nothing is never sent it.
#[tokio::test]
async fn a_seat_holding_nothing_is_not_woken() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    let start = crate::http::system_time_ms().unwrap();
    compact(&services, "m-compact", start).await;

    let outcomes = PaWatchdog::default()
        .round(&services, INTERVAL, start + COMPACT_SETTLE_MS + MINUTE)
        .await
        .unwrap();
    assert!(wakes(outcomes).is_empty());
    assert_eq!(
        settled(&services, "m-compact").await.as_deref(),
        Some("holds-nothing")
    );
    assert!(resumes(&services).await.is_empty());
}

/// bp-0001: while something is queued for it the seat isn't sent it; once
/// that drains with nothing started, it is.
#[tokio::test]
async fn a_seat_with_something_queued_is_not_woken_until_it_drains() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    hold_role(&services).await;
    services
        .delivery
        .accept(Msg {
            from: SeatId::from(PARENT),
            to: SeatId::from(SEAT),
            body: "sent before the compact, not delivered yet".into(),
            msg_id: "m-earlier".into(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        })
        .await
        .expect("queued");
    let start = crate::http::system_time_ms().unwrap();
    compact(&services, "m-compact", start).await;
    let watchdog = PaWatchdog::default();

    let due = start + COMPACT_SETTLE_MS + MINUTE;
    assert!(wakes(watchdog.round(&services, INTERVAL, due).await.unwrap()).is_empty());
    assert_eq!(settled(&services, "m-compact").await, None, "still owed");

    assert!(resumes(&services).await.is_empty(), "drained: m-earlier");
    assert_eq!(
        wakes(
            watchdog
                .round(&services, INTERVAL, due + MINUTE)
                .await
                .unwrap()
        )
        .len(),
        1
    );
}

/// bp-0001: a seat that starts anything after its compact (a turn, a report,
/// or a message pushed to it) isn't sent it. Claude's `/compact` posts one
/// `seat.activity` idle as it ends, and that alone doesn't count.
#[tokio::test]
async fn only_a_turn_report_or_message_after_the_compact_cancels_the_wake() {
    for (kind, payload, expected) in [
        (
            "seat.activity",
            serde_json::json!({"state": "idle"}),
            "woken",
        ),
        (
            "seat.activity",
            serde_json::json!({"state": "working"}),
            "resumed",
        ),
        (
            "report.now",
            serde_json::json!({"text": "on it"}),
            "resumed",
        ),
        (
            "message.pushed",
            serde_json::json!({"msg_id": "m-next", "from": PARENT, "body": "next"}),
            "resumed",
        ),
    ] {
        let (services, _store, _config, folder) = services().await;
        fleet(&services, &folder).await;
        hold_role(&services).await;
        let start = crate::http::system_time_ms().unwrap();
        compact(&services, "m-compact", start).await;
        append(&services, kind, start + 3_000, payload.clone()).await;

        let _ = PaWatchdog::default()
            .round(&services, INTERVAL, start + COMPACT_SETTLE_MS + MINUTE)
            .await
            .unwrap();
        assert_eq!(
            settled(&services, "m-compact").await.as_deref(),
            Some(expected),
            "{kind} {payload}"
        );
        assert_eq!(
            resumes(&services).await.len(),
            usize::from(expected == "woken"),
            "{kind} {payload}"
        );
    }
}

/// bp-0001: a new compaction re-arms the wake; an older compaction still
/// pending is superseded once a newer one is delivered; a compact never
/// delivered is given up on.
#[tokio::test]
async fn a_new_compaction_rearms_the_wake() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    hold_role(&services).await;
    let watchdog = PaWatchdog::default();
    let start = crate::http::system_time_ms().unwrap();
    compact(&services, "m-first", start).await;
    let first = start + COMPACT_SETTLE_MS + MINUTE;
    assert_eq!(
        wakes(watchdog.round(&services, INTERVAL, first).await.unwrap()).len(),
        1
    );
    assert_eq!(resumes(&services).await.len(), 1);

    let seat = SeatId::from(SEAT);
    crate::compaction_wake::record(&services, &seat, "m-stuck", first + MINUTE)
        .await
        .unwrap();
    let second = first + 2 * MINUTE;
    compact(&services, "m-second", second).await;
    assert_eq!(
        settled(&services, "m-stuck").await,
        None,
        "recording a newer compact supersedes nothing"
    );
    assert_eq!(
        wakes(
            watchdog
                .round(&services, INTERVAL, second + COMPACT_SETTLE_MS + MINUTE)
                .await
                .unwrap()
        )
        .len(),
        1
    );
    assert_eq!(
        settled(&services, "m-stuck").await.as_deref(),
        Some("superseded")
    );
    assert_eq!(resumes(&services).await.len(), 1);

    let lost = second + 10 * MINUTE;
    crate::compaction_wake::record(&services, &seat, "m-lost", lost)
        .await
        .unwrap();
    let _ = watchdog
        .round(&services, INTERVAL, lost + COMPACT_DELIVERY_BOUND_MS)
        .await
        .unwrap();
    assert_eq!(
        settled(&services, "m-lost").await.as_deref(),
        Some("undelivered")
    );
    assert!(resumes(&services).await.is_empty());
}

/// Review of #167, F002: a newer compact recorded but never admitted (its
/// caller dropped) holds an older compaction back only while it could still
/// land; once it is given up, the older one wakes the seat.
#[tokio::test]
async fn an_older_compaction_waits_for_a_newer_one_and_goes_on_if_it_never_lands() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    hold_role(&services).await;
    let watchdog = PaWatchdog::default();
    let start = crate::http::system_time_ms().unwrap();
    compact(&services, "m-old", start).await;
    crate::compaction_wake::record(&services, &SeatId::from(SEAT), "m-new", start + MINUTE)
        .await
        .unwrap();

    assert!(
        wakes(
            watchdog
                .round(&services, INTERVAL, start + COMPACT_SETTLE_MS + MINUTE)
                .await
                .unwrap()
        )
        .is_empty(),
        "a newer compact is in flight"
    );
    assert_eq!(settled(&services, "m-old").await, None);

    let given_up = start + MINUTE + COMPACT_DELIVERY_BOUND_MS;
    assert_eq!(
        wakes(watchdog.round(&services, INTERVAL, given_up).await.unwrap()).len(),
        1
    );
    assert_eq!(
        settled(&services, "m-new").await.as_deref(),
        Some("undelivered")
    );
    assert_eq!(settled(&services, "m-old").await.as_deref(), Some("woken"));
    assert_eq!(resumes(&services).await.len(), 1);
}

/// Review of #167, F003: the newest compact is the one admitted last, not
/// the one with the latest clock. After a clock step backwards the later
/// compact carries the earlier time, and it still gets its one wake while the
/// earlier one is superseded; equal times order the same way.
#[tokio::test]
async fn the_last_admitted_compact_wins_whatever_the_clock_says() {
    for (earlier, later) in [(10 * MINUTE, 0), (0, 0)] {
        let (services, _store, _config, folder) = services().await;
        fleet(&services, &folder).await;
        hold_role(&services).await;
        let start = crate::http::system_time_ms().unwrap();
        compact(&services, "m-earlier", start + earlier).await;
        compact(&services, "m-later", start + later).await;

        let due = start + earlier.max(later) + COMPACT_SETTLE_MS + MINUTE;
        assert_eq!(
            wakes(
                PaWatchdog::default()
                    .round(&services, INTERVAL, due)
                    .await
                    .unwrap()
            )
            .len(),
            1,
            "clock {earlier} then {later}"
        );
        assert_eq!(
            settled(&services, "m-later").await.as_deref(),
            Some("woken"),
            "clock {earlier} then {later}"
        );
        assert_eq!(
            settled(&services, "m-earlier").await.as_deref(),
            Some("superseded"),
            "clock {earlier} then {later}"
        );
        assert_eq!(resumes(&services).await.len(), 1);
    }
}

/// Review of #167, F001: a holdings read that fails is never "holds nothing".
/// A seat holding only a role stays pending while the read fails, then is
/// woken once when it recovers.
#[tokio::test]
async fn a_failed_holdings_read_leaves_the_compaction_pending() {
    let (services, store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    hold_role(&services).await;
    let start = crate::http::system_time_ms().unwrap();
    compact(&services, "m-compact", start).await;
    let pool = pij_store::open(&store.path()).await.unwrap();
    sqlx::query("ALTER TABLE community_assignments RENAME TO pij_test_assignments_away")
        .execute(&pool)
        .await
        .unwrap();
    let watchdog = PaWatchdog::default();
    let due = start + COMPACT_SETTLE_MS + MINUTE;
    for minute in 0..3 {
        assert!(
            wakes(
                watchdog
                    .round(&services, INTERVAL, due + minute * MINUTE)
                    .await
                    .unwrap()
            )
            .is_empty()
        );
    }
    assert_eq!(settled(&services, "m-compact").await, None, "still owed");

    sqlx::query("ALTER TABLE pij_test_assignments_away RENAME TO community_assignments")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        wakes(
            watchdog
                .round(&services, INTERVAL, due + 3 * MINUTE)
                .await
                .unwrap()
        )
        .len(),
        1
    );
    let sent = resumes(&services).await;
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .body
            .starts_with("resume: you hold coder/222 (project pij-fleet);")
    );
}

fn compact_msg(msg_id: &str) -> Msg {
    Msg {
        from: SeatId::from(PARENT),
        to: SeatId::from(SEAT),
        body: String::new(),
        msg_id: msg_id.into(),
        from_machine: None,
        in_reply_to: None,
        command: Some("compact".into()),
    }
}

/// Live jobs queued for the seat (deliveries and controls).
async fn queued(services: &crate::Services) -> usize {
    let seat = SeatId::from(SEAT);
    services
        .queue
        .live_payloads(&[
            pij_core::delivery::delivery_kind(&seat),
            pij_core::delivery::control_kind(&seat),
        ])
        .await
        .unwrap()
        .len()
}

async fn pending(services: &crate::Services) -> Vec<String> {
    services
        .watchdogs()
        .pending_compactions()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.msg_id)
        .collect()
}

/// Review of #167, F002: the row is written before the compact is admitted,
/// and a failed write refuses the compact, so none is admitted untracked.
/// After the store recovers the same compact is admitted and recorded, and a
/// replay of it adds neither a row nor a second control.
#[tokio::test]
async fn a_failed_tracking_write_refuses_the_compact_until_it_recovers() {
    let (services, store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    let state = crate::http::test_state(services.clone());
    let pool = pij_store::open(&store.path()).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER pij_test_track BEFORE INSERT ON compaction_wakes \
         BEGIN SELECT RAISE(ABORT, 'injected tracking failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        crate::http::admit_control(&state, compact_msg("m-compact"))
            .await
            .is_err(),
        "refused, not admitted untracked"
    );
    assert_eq!(queued(&services).await, 0);
    assert!(pending(&services).await.is_empty());

    sqlx::query("DROP TRIGGER pij_test_track")
        .execute(&pool)
        .await
        .unwrap();
    crate::http::admit_control(&state, compact_msg("m-compact"))
        .await
        .expect("admitted once the store recovers");
    assert_eq!(pending(&services).await, ["m-compact"]);
    assert_eq!(queued(&services).await, 1);
    crate::http::admit_control(&state, compact_msg("m-compact"))
        .await
        .expect("a replay");
    assert_eq!(pending(&services).await, ["m-compact"]);
    assert_eq!(queued(&services).await, 1, "replay dedup unchanged");
}

/// Review of #167, F002: a caller dropped at any point of `admit_control`
/// never leaves a compact admitted without its row. Each case polls the
/// admission a few more times before dropping it; the owned work it started
/// is then let finish.
#[tokio::test]
async fn a_dropped_caller_never_leaves_a_compact_admitted_untracked() {
    let mut admitted_after_drop = 0;
    for polls in 1..=64 {
        let (services, _store, _config, folder) = services().await;
        fleet(&services, &folder).await;
        let state = crate::http::test_state(services.clone());
        let mut admission = Box::pin(crate::http::admit_control(&state, compact_msg("m-cancel")));
        let mut finished = false;
        for _ in 0..polls {
            let ready = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(admission.as_mut().poll(cx).is_ready())
            })
            .await;
            if ready {
                finished = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        drop(admission);
        // Let any owned write or admission the dropped caller started finish.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let admitted = queued(&services).await;
        let recorded = pending(&services).await;
        assert!(
            admitted == 0 || recorded == ["m-cancel"],
            "dropped after {polls} polls: {admitted} admitted, rows {recorded:?}"
        );
        if admitted > 0 && !finished {
            admitted_after_drop += 1;
        }
        if finished {
            assert_eq!((admitted, recorded.len()), (1, 1));
            break;
        }
    }
    assert!(
        admitted_after_drop > 0,
        "some case dropped the caller with the admission in flight"
    );
}

/// #184 review (bobcat): a refused compact is acked `done` on the queue for
/// replay safety, so queue state never says a compact ran. The memory turn's
/// cutoff (F130) takes only compacts plan 222 settled after seeing them
/// delivered: a refused one settles `undelivered`, and the release before it
/// is still in the seat's context. A delivered one then clears it.
#[tokio::test]
async fn a_refused_compact_never_advances_the_memory_turn_cutoff() {
    let (services, _store, _config, folder) = services().await;
    fleet(&services, &folder).await;
    services
        .directory
        .store()
        .replace_assignments(
            "pij-fleet",
            &[pij_store::Assignment {
                project: "pij-fleet".into(),
                role: "reviewer".into(),
                instance: "175".into(),
                seat: SEAT.into(),
                harness: Some("copilot".into()),
                session: None,
                machine: None,
                from: "2020-01-01T00:00:00.000Z".into(),
                to: Some("2020-01-01T01:00:00.000Z".into()),
                assigned_by: PARENT.into(),
                released_by: Some(PARENT.into()),
                why: "review".into(),
                release_why: Some("merged".into()),
                end_pending: false,
            }],
        )
        .await
        .unwrap();
    let context = || async {
        services
            .directory
            .store()
            .context_assignments(SEAT)
            .await
            .unwrap()
            .rows
            .into_iter()
            .map(|row| format!("{}/{}", row.role, row.instance))
            .collect::<Vec<_>>()
    };
    let watchdog = PaWatchdog::default();
    let start = crate::http::system_time_ms().unwrap();
    crate::compaction_wake::record(&services, &SeatId::from(SEAT), "m-refused", start)
        .await
        .unwrap();
    append(
        &services,
        crate::delivery::OUTCOME_EVENT_KIND,
        start + 1_000,
        serde_json::json!({"msg_id": "m-refused", "outcome": {"outcome": "refused", "reason": "no"}, "transport": "pty"}),
    )
    .await;
    let _ = watchdog
        .round(&services, INTERVAL, start + COMPACT_DELIVERY_BOUND_MS)
        .await
        .unwrap();
    assert_eq!(
        settled(&services, "m-refused").await.as_deref(),
        Some("undelivered")
    );
    assert_eq!(
        context().await,
        ["reviewer/175"],
        "the refused compact cleared nothing"
    );

    let later = start + COMPACT_DELIVERY_BOUND_MS + MINUTE;
    compact(&services, "m-delivered", later).await;
    let _ = watchdog
        .round(&services, INTERVAL, later + COMPACT_SETTLE_MS + MINUTE)
        .await
        .unwrap();
    assert_eq!(
        settled(&services, "m-delivered").await.as_deref(),
        Some("holds-nothing")
    );
    assert!(
        context().await.is_empty(),
        "a delivered compact clears the context"
    );
}
