//! Plan 184 (pij-fleet#86): pij's own staged leftover, bounded holds, and
//! launch-free waiting. Uses only APIs that predate the plan, so the same file
//! runs against main to show each test red for the right reason.

use std::sync::Arc;
use std::time::Duration;

use pij_core::config::Config;
use pij_core::model::{Job, JobId, Msg, Outcome, Pane, SeatId, Seq};
use pij_core::ports::{Queue, Registry, STAGED_SUBMIT_RECOVERY, Spine, TmuxPort, Transport};
use pij_harnesses::InteractionGate;
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeRegistry, FakeSpine, FakeTmux, FakeTransport};

use super::{
    CADENCE, DrainWorker, EventBus, Fixture, PointerPolicy, delivery_kind, fixture_with_grace,
    register, seat,
};

const RULE: &str = "────────────────────────────────";

/// A composer that renders `rows` the way Claude does: the first row after
/// `❯ `, continuation rows indented two columns, the caret after the last.
fn composer(pane: &str, rows: &[&str]) -> FakeTmux {
    let mut capture = format!("{RULE}\n❯ {}", rows[0]);
    for row in &rows[1..] {
        capture.push_str(&format!("\n  {row}"));
    }
    capture.push_str(&format!("\n{RULE}"));
    let last = rows.len() - 1;
    let cursor_x = rows[last].chars().count() + 2;
    FakeTmux::new()
        .with_pane(Pane {
            id: pane.into(),
            session: "s".into(),
            window: "w".into(),
            title: "t".into(),
            cursor_x: Some(u32::try_from(cursor_x).unwrap()),
            cursor_y: Some(u32::try_from(1 + last).unwrap()),
        })
        .with_attached_tap(pane)
        .with_standing_capture(capture)
}

/// Grace zero: these composers have not changed for longer than any grace.
fn settled(transport: FakeTransport, tmux: FakeTmux) -> Fixture {
    fixture_with_grace(transport, tmux, 0)
}

fn msg(to: &str, id: &str, body: &str, command: Option<&str>) -> Msg {
    Msg {
        from: SeatId::from("pij-sender"),
        to: SeatId::from(to),
        body: body.into(),
        msg_id: id.into(),
        from_machine: None,
        in_reply_to: None,
        command: command.map(str::to_string),
    }
}

async fn enqueue_at(fixture: &Fixture, msg: &Msg, attempt: u32) -> JobId {
    fixture
        .queue
        .enqueue(Job {
            kind: delivery_kind(&msg.to),
            serial_key: msg.to.0.clone(),
            payload: serde_json::to_string(msg).unwrap(),
            dedupe_key: msg.msg_id.clone(),
            dedupe_origin: None,
            attempt,
        })
        .await
        .unwrap()
}

async fn sender_events(fixture: &Fixture) -> Vec<pij_core::model::Event> {
    fixture
        .spine
        .tail(Some(&SeatId::from("pij-sender")), Seq(0))
        .await
        .unwrap()
}

fn failed_reason(fixture: &Fixture, id: JobId) -> Option<String> {
    fixture
        .queue
        .acked()
        .into_iter()
        .find_map(|(acked, outcome)| match outcome {
            Outcome::Failed { reason } if acked == id => Some(reason),
            _ => None,
        })
}

/// The walrus composer (assets/evidence/2026-10-29-walrus-pane-108.txt).
fn walrus(pane: &str) -> FakeTmux {
    composer(pane, &["/compact", STAGED_SUBMIT_RECOVERY])
}

#[tokio::test]
async fn own_staged_leftover_fails_its_command_by_name_and_tells_the_sender() {
    let fixture = settled(FakeTransport::reachable(), walrus("%108"));
    register(&fixture, seat("pij-walrus", Some("%108"))).await;
    let compact = msg("pij-walrus", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 0).await;

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);

    let reason = failed_reason(&fixture, id).expect("pij's own leftover fails the command");
    assert!(reason.starts_with("undelivered:staged-unsent"), "{reason}");
    let calls = fixture.tmux.calls();
    let leftover = format!("/compact\n{STAGED_SUBMIT_RECOVERY}")
        .chars()
        .count();
    assert!(
        calls.contains(&format!("clear_staged_leftover:%108:{leftover}")),
        "{calls:?}"
    );
    assert!(!calls.iter().any(|call| call.starts_with("submit:")));
    assert!(!fixture.tmux.pane_input_disabled("%108"), "input restored");
    let parked = sender_events(&fixture).await;
    let parked = parked
        .iter()
        .find(|event| event.kind == "delivery.parked")
        .expect("the sender is told");
    let payload: serde_json::Value = serde_json::from_str(&parked.payload).unwrap();
    assert_eq!(payload["outcome"], "undelivered:staged-unsent");
    assert_eq!(payload["messageId"], "m-compact");
}

#[tokio::test]
async fn own_leftover_plus_one_keystroke_stays_a_human_draft() {
    let typed = format!("{STAGED_SUBMIT_RECOVERY}x");
    for rows in [
        vec!["/compact", typed.as_str()],
        vec!["/compactx", STAGED_SUBMIT_RECOVERY],
        vec!["/compact", "x", STAGED_SUBMIT_RECOVERY],
    ] {
        let fixture = settled(FakeTransport::reachable(), composer("%108", &rows));
        register(&fixture, seat("pij-walrus", Some("%108"))).await;
        let compact = msg("pij-walrus", "m-compact", "", Some("compact"));
        let id = enqueue_at(&fixture, &compact, 0).await;

        assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);

        assert!(fixture.queue.acked().is_empty(), "{rows:?}");
        assert!(
            !fixture
                .tmux
                .calls()
                .iter()
                .any(|call| call.starts_with("clear_staged_leftover")),
            "a human draft is never touched: {rows:?}"
        );
        assert_eq!(fixture.queue.attempts(id), 1, "held, not failed");
    }
}

#[tokio::test]
async fn composer_busy_hold_backs_off_and_never_reserves_the_pane() {
    let fixture = settled(
        FakeTransport::reachable(),
        composer("%draft", &["parked human draft"]),
    );
    register(&fixture, seat("pij-draft", Some("%draft"))).await;
    let compact = msg("pij-draft", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 3).await;

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);

    assert_eq!(
        fixture.queue.retried(),
        [(id, Duration::from_secs(8))],
        "composer-busy backs off on the job's own attempt count"
    );
    assert!(
        !fixture
            .tmux
            .calls()
            .iter()
            .any(|call| call.starts_with("acquire_submit")),
        "a plainly busy composer is read, never reserved: {:?}",
        fixture.tmux.calls()
    );
}

#[tokio::test]
async fn composer_busy_backoff_is_capped_at_a_minute() {
    let fixture = settled(
        FakeTransport::reachable(),
        composer("%draft", &["parked human draft"]),
    );
    register(&fixture, seat("pij-draft", Some("%draft"))).await;
    let compact = msg("pij-draft", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 8).await;

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);

    assert_eq!(fixture.queue.retried(), [(id, Duration::from_secs(60))]);
}

#[tokio::test]
async fn held_command_fails_by_name_at_the_hold_limit_and_tells_its_sender() {
    let fixture = settled(
        FakeTransport::reachable(),
        composer("%draft", &["parked human draft"]),
    );
    register(&fixture, seat("pij-draft", Some("%draft"))).await;
    let compact = msg("pij-draft", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 11).await;

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);

    let reason = failed_reason(&fixture, id).expect("terminal at the hold limit");
    assert!(
        reason.starts_with("undelivered:hold-limit") && reason.contains("composer-busy"),
        "{reason}"
    );
    let events = sender_events(&fixture).await;
    let parked = events
        .iter()
        .find(|event| event.kind == "delivery.parked")
        .expect("the sender is told");
    let payload: serde_json::Value = serde_json::from_str(&parked.payload).unwrap();
    assert_eq!(payload["outcome"], "undelivered:hold-limit");
    assert_eq!(
        fixture
            .tmux
            .calls()
            .iter()
            .filter(|c| c.starts_with("submit:"))
            .count(),
        0
    );
}

/// On the REAL queue, which is strictly FIFO per seat: the fake lets later rows
/// past a re-pended head, which would hide the deadlock this test is about.
#[tokio::test]
async fn failed_control_head_unblocks_the_messages_queued_behind_it_in_order() {
    let fresh = FreshStore::new();
    let pool = pij_store::open(&fresh.path()).await.expect("open store");
    let queue = Arc::new(
        SqliteQueue::new(
            pool.clone(),
            Config::default().claim_lease_secs,
            Config::default().delivered_id_capacity,
        )
        .expect("sqlite queue"),
    );
    let registry = Arc::new(FakeRegistry::new());
    registry
        .put(seat("pij-draft", Some("%draft")))
        .await
        .expect("register");
    let transport = Arc::new(FakeTransport::reachable());
    let tmux = Arc::new(composer("%draft", &["parked human draft"]));
    let interaction = Arc::new(InteractionGate::with_typing_grace(
        Arc::clone(&tmux) as Arc<dyn TmuxPort>,
        Duration::from_secs(60),
        0,
    ));
    let spine = Arc::new(FakeSpine::new());
    let event_bus =
        Arc::new(EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 16).expect("event bus"));
    let worker = DrainWorker::new(
        Arc::clone(&registry) as Arc<dyn Registry>,
        Arc::clone(&queue) as Arc<dyn Queue>,
        Arc::clone(&transport) as Arc<dyn Transport>,
        Arc::clone(&tmux) as Arc<dyn TmuxPort>,
        interaction,
        event_bus,
        PointerPolicy {
            cadence: CADENCE,
            announcement_limit: 3,
        },
    )
    .expect("worker");
    let enqueue = |msg: Msg| {
        let queue = Arc::clone(&queue);
        async move {
            queue
                .enqueue(Job {
                    kind: delivery_kind(&msg.to),
                    serial_key: msg.to.0.clone(),
                    payload: serde_json::to_string(&msg).unwrap(),
                    dedupe_key: msg.msg_id.clone(),
                    dedupe_origin: None,
                    attempt: 0,
                })
                .await
                .unwrap()
        }
    };
    let head = enqueue(msg("pij-draft", "m-compact", "", Some("compact"))).await;
    // The live head had thousands of attempts; this one is at the hold limit.
    sqlx::query("UPDATE jobs SET attempt = 11 WHERE id = ?1")
        .bind(head.0 as i64)
        .execute(&pool)
        .await
        .expect("age the head");
    let behind: Vec<Msg> = (1..=6)
        .map(|n| {
            msg(
                "pij-draft",
                &format!("m-{n}"),
                &format!("message {n}"),
                None,
            )
        })
        .collect();
    for queued in &behind {
        enqueue(queued.clone()).await;
    }

    for _ in 0..7 {
        worker.drain_once().await.expect("drain");
    }

    assert_eq!(
        transport.delivered(),
        behind,
        "every queued message, in order, once the control head failed"
    );
    let outcome: String = sqlx::query_scalar("SELECT outcome FROM jobs WHERE id = ?1")
        .bind(head.0 as i64)
        .fetch_one(&pool)
        .await
        .expect("head outcome");
    assert!(outcome.starts_with("undelivered:hold-limit"), "{outcome}");
}

#[tokio::test]
async fn held_body_tells_its_sender_once_at_the_hold_limit_and_stays_queued() {
    let fixture = settled(
        FakeTransport::unreachable(),
        composer("%draft", &["parked human draft"]),
    );
    register(&fixture, seat("pij-draft", Some("%draft"))).await;
    let body = msg("pij-draft", "m-body", "typed body", None);
    let id = enqueue_at(&fixture, &body, 11).await;

    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);
    fixture
        .queue
        .release_deferred(id)
        .await
        .expect("eligible again");
    assert_eq!(fixture.worker.drain_once().await.expect("drain"), 1);

    let notices: Vec<_> = sender_events(&fixture)
        .await
        .into_iter()
        .filter(|event| event.kind == "delivery.hold-limit")
        .collect();
    assert_eq!(notices.len(), 1, "told once, not every attempt");
    let payload: serde_json::Value = serde_json::from_str(&notices[0].payload).unwrap();
    assert_eq!(payload["messageId"], "m-body");
    assert_eq!(payload["reason"], "composer-busy");
    assert!(fixture.queue.acked().is_empty(), "a body stays queued");
    assert_eq!(fixture.queue.live_len(), 1);
}
