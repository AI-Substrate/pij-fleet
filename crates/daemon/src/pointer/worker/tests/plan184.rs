//! Plan 184 (pij-fleet#86): pij's own staged leftover, bounded holds, and
//! launch-free waiting, including the review of #91 (B1, S1, S3, S4, S5) and
//! the Copilot control kind of plan 186.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use pij_core::config::Config;
use pij_core::delivery::control_kind;
use pij_core::error::Result;
use pij_core::model::{Harness, Job, JobId, Msg, Outcome, Pane, SeatId, Seq};
use pij_core::ports::{Queue, Registry, STAGED_SUBMIT_RECOVERY, Spine, TmuxPort, Transport};
use pij_harnesses::InteractionGate;
use pij_store::SqliteQueue;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeRegistry, FakeSpine, FakeTmux, FakeTransport};
use sqlx::SqlitePool;

use super::{
    CADENCE, DrainWorker, EventBus, Fixture, PointerPolicy, delivery_kind, fixture_with_grace,
    register, seat,
};

const RULE: &str = "────────────────────────────────";
const MINUTE_MS: u64 = 60_000;

thread_local! {
    /// The worker's wall clock in these tests, per test thread (each
    /// `#[tokio::test]` runs on its own current-thread runtime).
    static NOW_MS: Cell<u64> = const { Cell::new(1_800_000_000_000) };
}

fn now_ms() -> Result<u64> {
    Ok(NOW_MS.with(Cell::get))
}

fn advance_ms(by: u64) {
    NOW_MS.with(|now| now.set(now.get() + by));
}

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

/// The walrus composer (assets/evidence/2026-10-29-walrus-pane-108.txt).
fn walrus(pane: &str) -> FakeTmux {
    composer(pane, &["/compact", STAGED_SUBMIT_RECOVERY])
}

/// Grace zero: these composers have not changed for longer than any grace.
/// The worker reads the thread's test clock.
fn settled(transport: FakeTransport, tmux: FakeTmux) -> Fixture {
    let mut fixture = fixture_with_grace(transport, tmux, 0);
    fixture.worker.clock = now_ms;
    fixture
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

fn job(msg: &Msg, kind: String) -> Job {
    Job {
        kind,
        serial_key: msg.to.0.clone(),
        payload: serde_json::to_string(msg).unwrap(),
        dedupe_key: msg.msg_id.clone(),
        dedupe_origin: None,
        attempt: 0,
    }
}

async fn enqueue_at(fixture: &Fixture, msg: &Msg, attempt: u32) -> JobId {
    let mut job = job(msg, delivery_kind(&msg.to));
    job.attempt = attempt;
    fixture.queue.enqueue(job).await.unwrap()
}

async fn sender_events(spine: &FakeSpine) -> Vec<pij_core::model::Event> {
    spine
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

fn parked_outcome(events: &[pij_core::model::Event], msg_id: &str) -> Option<String> {
    events
        .iter()
        .filter(|event| event.kind == "delivery.parked")
        .map(|event| serde_json::from_str::<serde_json::Value>(&event.payload).unwrap())
        .find(|payload| payload["messageId"] == msg_id)
        .map(|payload| payload["outcome"].as_str().unwrap().to_string())
}

/// Drain once, then let the job's own retry delay pass on both clocks.
async fn tick(fixture: &Fixture) {
    let before = fixture.queue.retried().len();
    fixture.worker.drain_once().await.expect("drain");
    if let Some((_, delay)) = fixture.queue.retried().get(before) {
        fixture.queue.advance(*delay);
        advance_ms(u64::try_from(delay.as_millis()).unwrap());
    }
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
    assert_eq!(
        parked_outcome(&sender_events(&fixture.spine).await, "m-compact").as_deref(),
        Some("undelivered:staged-unsent")
    );
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

/// Composer-busy backs off on its OWN holds, not on `attempt`: a job that
/// arrives with thousands of earlier retries still starts at one second.
#[tokio::test]
async fn composer_busy_backs_off_on_its_own_holds_to_a_minute_and_never_reserves() {
    let fixture = settled(
        FakeTransport::reachable(),
        composer("%draft", &["parked human draft"]),
    );
    register(&fixture, seat("pij-draft", Some("%draft"))).await;
    let compact = msg("pij-draft", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 9_180).await;

    for _ in 0..8 {
        tick(&fixture).await;
    }

    let delays: Vec<u64> = fixture
        .queue
        .retried()
        .into_iter()
        .map(|(retried, delay)| {
            assert_eq!(retried, id);
            delay.as_secs()
        })
        .collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
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

/// Review B1: a person typing for fifteen minutes holds a command for fifteen
/// minutes. Typing never counts toward the limit, and each retry waits out
/// the rest of the grace instead of the next tick.
#[tokio::test]
async fn human_typing_never_counts_toward_the_limit_and_waits_out_the_grace() {
    let tmux = FakeTmux::new().with_attached_tap("%typing");
    tmux.arrange_composer_text("%typing", "draft 0");
    let mut fixture = fixture_with_grace(FakeTransport::reachable(), tmux, 60_000);
    fixture.worker.clock = now_ms;
    register(&fixture, seat("pij-typing", Some("%typing"))).await;
    let compact = msg("pij-typing", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 0).await;

    for round in 1..=15 {
        tick(&fixture).await;
        // The person keeps typing: every look sees a new edit.
        fixture
            .tmux
            .arrange_composer_text("%typing", &format!("draft {round}"));
    }

    assert!(
        fixture.queue.acked().is_empty(),
        "typing never fails a command: {:?}",
        failed_reason(&fixture, id)
    );
    let retried = fixture.queue.retried();
    assert_eq!(retried.len(), 15);
    for (_, delay) in retried {
        assert!(
            delay > Duration::from_secs(55) && delay <= Duration::from_secs(60),
            "{delay:?}"
        );
    }
    assert!(parked_outcome(&sender_events(&fixture.spine).await, "m-compact").is_none());
}

#[tokio::test]
async fn held_command_fails_by_name_after_seven_minutes_held_and_tells_its_sender() {
    let fixture = settled(
        FakeTransport::reachable(),
        composer("%draft", &["parked human draft"]),
    );
    register(&fixture, seat("pij-draft", Some("%draft"))).await;
    let compact = msg("pij-draft", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 0).await;
    let started = now_ms().unwrap();

    let mut ticks = 0;
    while fixture.queue.acked().is_empty() {
        assert!(
            now_ms().unwrap() - started < 7 * MINUTE_MS + MINUTE_MS,
            "never failed"
        );
        assert!(ticks < 30, "too many ticks");
        let held_for = now_ms().unwrap() - started;
        tick(&fixture).await;
        ticks += 1;
        if !fixture.queue.acked().is_empty() {
            assert!(held_for >= 7 * MINUTE_MS, "failed early, at {held_for} ms");
        }
    }

    let reason = failed_reason(&fixture, id).expect("terminal at the hold limit");
    assert!(
        reason.starts_with("undelivered:hold-limit: /compact was still held (composer-busy)"),
        "{reason}"
    );
    assert_eq!(
        parked_outcome(&sender_events(&fixture.spine).await, "m-compact").as_deref(),
        Some("undelivered:hold-limit")
    );
    assert!(
        !fixture
            .tmux
            .calls()
            .iter()
            .any(|call| call.starts_with("submit:"))
    );
}

/// Review S3: R1 wins. When the composer does not read blank after a clear,
/// no further key is sent, the pane is released, and the job fails by name.
#[tokio::test]
async fn a_clear_that_leaves_residue_sends_nothing_more_and_fails_by_name() {
    let fixture = settled(
        FakeTransport::reachable(),
        walrus("%108").script_clear_residue("/"),
    );
    register(&fixture, seat("pij-walrus", Some("%108"))).await;
    let compact = msg("pij-walrus", "m-compact", "", Some("compact"));
    let id = enqueue_at(&fixture, &compact, 0).await;

    fixture.worker.drain_once().await.expect("drain");

    let reason = failed_reason(&fixture, id).expect("fails by name");
    assert!(
        reason.starts_with("undelivered:staged-unsent")
            && reason.contains("composer not cleared; left for the operator"),
        "{reason}"
    );
    let calls = fixture.tmux.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("clear_staged_leftover"))
            .count(),
        1,
        "{calls:?}"
    );
    assert!(!calls.iter().any(|call| call.starts_with("submit:")));
    assert!(!fixture.tmux.pane_input_disabled("%108"), "input restored");
}

/// Plan 186: Copilot controls ride `delivery-control:<seat>`, through the same
/// pane path. pij's own leftover there is cleared and named like Claude's.
#[tokio::test]
async fn a_copilot_control_clears_its_own_leftover_and_tells_the_sender() {
    let fixture = settled(FakeTransport::reachable(), walrus("%copilot"));
    let mut copilot = seat("pij-copilot", Some("%copilot"));
    copilot.harness = Harness::Copilot;
    register(&fixture, copilot).await;
    let compact = msg("pij-copilot", "m-compact", "", Some("compact"));
    let id = fixture
        .queue
        .enqueue(job(&compact, control_kind(&compact.to)))
        .await
        .unwrap();

    fixture.worker.drain_once().await.expect("drain");

    let reason = failed_reason(&fixture, id).expect("pij's own leftover fails the command");
    assert!(reason.starts_with("undelivered:staged-unsent"), "{reason}");
    assert_eq!(
        parked_outcome(&sender_events(&fixture.spine).await, "m-compact").as_deref(),
        Some("undelivered:staged-unsent")
    );
}

/// Plan 186's live case: a Copilot control held as `unrecognized` against a
/// leftover pij cannot read. Unrecognized holds count, so it ends by name.
#[tokio::test]
async fn a_copilot_control_held_unrecognized_fails_by_name_at_the_limit() {
    let tmux = FakeTmux::new()
        .with_pane(Pane {
            id: "%copilot".into(),
            session: "s".into(),
            window: "w".into(),
            title: "t".into(),
            cursor_x: Some(3),
            cursor_y: Some(0),
        })
        .with_attached_tap("%copilot")
        .with_standing_capture("no composer this gate can read");
    let fixture = settled(FakeTransport::reachable(), tmux);
    let mut copilot = seat("pij-copilot", Some("%copilot"));
    copilot.harness = Harness::Copilot;
    register(&fixture, copilot).await;
    let compact = msg("pij-copilot", "m-compact", "", Some("compact"));
    let id = fixture
        .queue
        .enqueue(job(&compact, control_kind(&compact.to)))
        .await
        .unwrap();

    tick(&fixture).await;
    assert!(
        fixture.queue.acked().is_empty(),
        "held, not failed, at first"
    );
    advance_ms(7 * MINUTE_MS);
    fixture.queue.advance(Duration::from_secs(600));
    fixture.worker.drain_once().await.expect("drain");

    let reason = failed_reason(&fixture, id).expect("terminal at the hold limit");
    assert!(
        reason.starts_with("undelivered:hold-limit: /compact was still held (unrecognized)"),
        "{reason}"
    );
    assert_eq!(
        parked_outcome(&sender_events(&fixture.spine).await, "m-compact").as_deref(),
        Some("undelivered:hold-limit")
    );
}

/// A worker over the REAL queue (strictly FIFO per seat, and `attempt` is
/// durable), the fake tmux `tmux`, and `gate`.
struct Real {
    _store: FreshStore,
    pool: SqlitePool,
    queue: Arc<SqliteQueue>,
    transport: Arc<FakeTransport>,
    tmux: Arc<FakeTmux>,
    spine: Arc<FakeSpine>,
    worker: DrainWorker,
}

async fn real(
    transport: FakeTransport,
    tmux: FakeTmux,
    gate: impl FnOnce(Arc<dyn TmuxPort>) -> InteractionGate,
    recipient: &str,
    pane: &str,
) -> Real {
    let store = FreshStore::new();
    let pool = pij_store::open(&store.path()).await.expect("open store");
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
        .put(seat(recipient, Some(pane)))
        .await
        .expect("register");
    let transport = Arc::new(transport);
    let tmux = Arc::new(tmux);
    let interaction = Arc::new(gate(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
    let spine = Arc::new(FakeSpine::new());
    let event_bus =
        Arc::new(EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 16).expect("event bus"));
    let mut worker = DrainWorker::new(
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
    worker.clock = now_ms;
    Real {
        _store: store,
        pool,
        queue,
        transport,
        tmux,
        spine,
        worker,
    }
}

impl Real {
    async fn enqueue(&self, msg: &Msg) -> JobId {
        self.queue
            .enqueue(job(msg, delivery_kind(&msg.to)))
            .await
            .unwrap()
    }

    async fn set_attempt(&self, id: JobId, attempt: u32) {
        sqlx::query("UPDATE jobs SET attempt = ?1 WHERE id = ?2")
            .bind(i64::from(attempt))
            .bind(id.0 as i64)
            .execute(&self.pool)
            .await
            .expect("set attempt");
    }

    /// Make every pending row eligible now: the test steps time, not sleeps.
    async fn eligible_now(&self) {
        sqlx::query("UPDATE jobs SET not_before = 0 WHERE state = 'pending'")
            .execute(&self.pool)
            .await
            .expect("reset not_before");
    }

    async fn row(&self, id: JobId) -> (String, i64, Option<String>, i64) {
        sqlx::query_as(
            "SELECT state, attempt, outcome, not_before - unixepoch() FROM jobs WHERE id = ?1",
        )
        .bind(id.0 as i64)
        .fetch_one(&self.pool)
        .await
        .expect("job row")
    }
}

fn settled_gate(tmux: Arc<dyn TmuxPort>) -> InteractionGate {
    InteractionGate::with_typing_grace(tmux, Duration::from_secs(60), 0)
}

#[tokio::test]
async fn failed_control_head_unblocks_the_messages_queued_behind_it_in_order() {
    let real = real(
        FakeTransport::reachable(),
        composer("%draft", &["parked human draft"]),
        settled_gate,
        "pij-draft",
        "%draft",
    )
    .await;
    let head = real
        .enqueue(&msg("pij-draft", "m-compact", "", Some("compact")))
        .await;
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
        real.enqueue(queued).await;
    }

    real.worker.drain_once().await.expect("first hold");
    assert!(real.transport.delivered().is_empty(), "the head blocks");
    advance_ms(7 * MINUTE_MS);
    for _ in 0..7 {
        real.eligible_now().await;
        real.worker.drain_once().await.expect("drain");
    }

    assert_eq!(
        real.transport.delivered(),
        behind,
        "every queued message, in order, once the control head failed"
    );
    let (state, _, outcome, _) = real.row(head).await;
    assert_eq!(state, "failed");
    assert!(
        outcome
            .as_deref()
            .is_some_and(|outcome| outcome.starts_with("undelivered:hold-limit")),
        "{outcome:?}"
    );
}

/// Review S1 and S4, on the real queue across passes: a held body tells its
/// sender exactly once at the hold limit and stays queued, then fails by name.
#[tokio::test]
async fn a_held_body_tells_its_sender_once_then_fails_by_name() {
    let real = real(
        FakeTransport::unreachable(),
        composer("%draft", &["parked human draft"]),
        settled_gate,
        "pij-draft",
        "%draft",
    )
    .await;
    let body = real
        .enqueue(&msg("pij-draft", "m-body", "typed body", None))
        .await;
    // At the old per-attempt limit already, as the reviewer's scratch test had it.
    real.set_attempt(body, 11).await;

    real.worker.drain_once().await.expect("first hold");
    advance_ms(7 * MINUTE_MS);
    for _ in 0..4 {
        real.eligible_now().await;
        real.worker.drain_once().await.expect("held past the limit");
        advance_ms(MINUTE_MS);
    }
    let facts: Vec<_> = sender_events(&real.spine)
        .await
        .into_iter()
        .filter(|event| event.kind == "delivery.hold-limit")
        .collect();
    assert_eq!(facts.len(), 1, "told once, across passes on the real queue");
    let payload: serde_json::Value = serde_json::from_str(&facts[0].payload).unwrap();
    assert_eq!(payload["msg_id"], "m-body");
    assert_eq!(payload["reason"], "composer-busy");
    assert_eq!(real.row(body).await.0, "pending", "a body stays queued");

    advance_ms(20 * MINUTE_MS);
    real.eligible_now().await;
    real.worker
        .drain_once()
        .await
        .expect("held past the body limit");
    let (state, _, outcome, _) = real.row(body).await;
    assert_eq!(state, "failed");
    assert!(
        outcome.as_deref().is_some_and(|outcome| outcome
            .starts_with("undelivered:hold-limit: message was still held (composer-busy)")),
        "{outcome:?}"
    );
    assert_eq!(
        parked_outcome(&sender_events(&real.spine).await, "m-body").as_deref(),
        Some("undelivered:hold-limit")
    );
}

/// Review S5: the deploy on walrus, modelled on its real state. The head has
/// thousands of attempts, the composer holds pij's `/compact` and the notice,
/// and a freshly started gate treats its first sight as an edit for 60 s.
#[tokio::test]
async fn the_walrus_deploy_clears_its_own_leftover_and_drains_the_queue_in_order() {
    let elapsed_ms = Arc::new(AtomicU64::new(0));
    let started_at = Instant::now();
    let gate_clock = Arc::clone(&elapsed_ms);
    let real = real(
        FakeTransport::reachable(),
        walrus("%108"),
        move |tmux| {
            InteractionGate::with_clock(
                tmux,
                Duration::from_secs(60),
                pij_core::delivery::DEFAULT_TYPING_GRACE_MS,
                Arc::new(move || {
                    started_at + Duration::from_millis(gate_clock.load(Ordering::Relaxed))
                }),
            )
        },
        "pij-elated-walrus",
        "%108",
    )
    .await;
    let head = real
        .enqueue(&msg("pij-elated-walrus", "m-head", "", Some("compact")))
        .await;
    real.set_attempt(head, 9_180).await;
    let behind: Vec<Msg> = (1..=6)
        .map(|n| {
            msg(
                "pij-elated-walrus",
                &format!("m-{n}"),
                &format!("message {n}"),
                None,
            )
        })
        .collect();
    for queued in &behind {
        real.enqueue(queued).await;
    }

    // First tick after the bounce: first sight is an edit, so the grace holds.
    real.worker.drain_once().await.expect("first tick");
    let (state, attempt, outcome, wait_s) = real.row(head).await;
    assert_eq!(
        (state.as_str(), outcome),
        ("pending", None),
        "held, not failed"
    );
    assert_eq!(attempt, 9_181);
    assert!(
        (55..=61).contains(&wait_s),
        "waits out the grace: {wait_s} s"
    );
    assert!(
        !real
            .tmux
            .calls()
            .iter()
            .any(|call| call.starts_with("acquire_submit")),
        "the grace is read, not reserved"
    );

    // Past the grace: pij's own leftover is recognised and cleared.
    elapsed_ms.store(61_000, Ordering::Relaxed);
    advance_ms(61_000);
    real.eligible_now().await;
    real.worker.drain_once().await.expect("past the grace");
    let (state, _, outcome, _) = real.row(head).await;
    let outcome = outcome.expect("the head ends");
    assert_eq!(state, "failed");
    assert!(
        outcome.starts_with(
            "undelivered:staged-unsent: /compact was staged but never submitted; pij cleared its own leftover"
        ),
        "{outcome}"
    );
    assert!(!outcome.contains("human-typing") && !outcome.contains("attempts"));
    assert!(
        real.tmux
            .calls()
            .iter()
            .any(|call| call.starts_with("clear_staged_leftover:%108:"))
    );
    assert_eq!(
        parked_outcome(&sender_events(&real.spine).await, "m-head").as_deref(),
        Some("undelivered:staged-unsent")
    );

    for _ in 0..6 {
        real.eligible_now().await;
        real.worker.drain_once().await.expect("drain");
    }
    assert_eq!(
        real.transport.delivered(),
        behind,
        "the queue behind, in order"
    );
}
