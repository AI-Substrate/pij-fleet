use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::delivery::control_kind;
use pij_core::model::{Harness, Outcome, ProcIdentity, SeatDescriptor, SeatId, SystemState};
use pij_core::ports::TmuxPort;
use pij_harnesses::InteractionGate;
use pij_testkit::FreshStore;
use pij_testkit::fakes::{FakeLiveness, FakeTmux};

use super::{
    Action, Cause, Entry, Incarnation, Ledger, LostLogin, RECOVERY_WINDOW, decide, handle, on_lapse,
};

const SEAT: &str = "pij-copilot-coder";
const PARENT: &str = "pij-pm";
const PRIME: &str = "pij-prime";
const PANE: &str = "%21";
const MINUTE: u64 = 60_000;
/// The coder's current process, as the registry records it.
const FIRST: Incarnation = Incarnation {
    pid: 4100,
    proc_start: 7,
};
/// The process a `/restart` started.
const SECOND: Incarnation = Incarnation {
    pid: 4200,
    proc_start: 9,
};

struct Fleet {
    services: crate::Services,
    _store: FreshStore,
}

/// A PM, a prime and the PM's Copilot coder (pid 4100), with a fake tmux whose
/// coder pane shows `draft` in the composer, or an empty one.
async fn fleet(state: SystemState, draft: Option<&str>) -> Fleet {
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
    let mut services =
        crate::build_services(&config, &Path::new(&store.path()).with_extension("signals"))
            .await
            .expect("real store services");
    let tmux = Arc::new(FakeTmux::new());
    match draft {
        Some(text) => tmux.arrange_composer_text(PANE, text),
        None => tmux.arrange_clear_composer(PANE),
    }
    services.tmux = Arc::clone(&tmux) as Arc<dyn TmuxPort>;
    services.interaction = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
    let folder = std::env::temp_dir().join(format!(
        "pij-auth-recovery-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&folder).unwrap();
    let folder = folder.display().to_string();
    let mut pm = SeatDescriptor::new(PARENT, Harness::Claude, &folder);
    pm.pane = Some("%20".into());
    let mut prime = SeatDescriptor::new(PRIME, Harness::Claude, &folder);
    prime.pane = Some("%19".into());
    let mut coder = SeatDescriptor::new(SEAT, Harness::Copilot, &folder);
    coder.parent = Some(SeatId::from(PARENT));
    coder.pane = Some(PANE.into());
    coder.harness_session = Some("native-212".into());
    coder.proc = Some(ProcIdentity {
        pid: FIRST.pid,
        proc_start: FIRST.proc_start,
    });
    coder.state = state;
    for seat in [pm, prime, coder] {
        services.registry.put(seat).await.unwrap();
    }
    Fleet {
        services,
        _store: store,
    }
}

async fn coder(fleet: &Fleet) -> SeatDescriptor {
    fleet
        .services
        .registry
        .get(&SeatId::from(SEAT))
        .await
        .unwrap()
        .unwrap()
}

async fn report(fleet: &Fleet, reporter: Incarnation, now_ms: u64) -> super::Outcome {
    handle(
        &fleet.services,
        &coder(fleet).await,
        reporter,
        Cause::Reported(LostLogin::Authorization),
        now_ms,
    )
    .await
    .unwrap()
}

/// Every live control queued for the coder, as its payload.
async fn controls(fleet: &Fleet) -> Vec<String> {
    fleet
        .services
        .queue
        .live_payloads(&[control_kind(&SeatId::from(SEAT))])
        .await
        .unwrap()
}

/// Every live message queued for `seat`, as its payload.
async fn told(fleet: &Fleet, seat: &str) -> Vec<String> {
    fleet
        .services
        .queue
        .live_payloads(&[pij_core::delivery::delivery_kind(&SeatId::from(seat))])
        .await
        .unwrap()
}

/// The pointer worker takes the queued restart and settles it.
async fn settle_restart(fleet: &Fleet, outcome: Outcome) {
    let (job, _) = fleet
        .services
        .queue
        .claim(&[control_kind(&SeatId::from(SEAT))], "pointer")
        .await
        .unwrap()
        .expect("a queued restart");
    fleet.services.queue.ack(job, outcome).await.unwrap();
}

/// `coder/224` held by the coder in `project`, whose designated prime is
/// `prime`.
async fn hold_in(fleet: &Fleet, project: &str, prime: &str) {
    sqlx::query(
        "INSERT INTO projects (slug, created_by, created_at, prime_id) VALUES (?1, ?2, 1, ?3)",
    )
    .bind(project)
    .bind(PARENT)
    .bind(prime)
    .execute(&fleet.services.store_pools[0])
    .await
    .unwrap();
    fleet
        .services
        .watchdogs()
        .record_assignment_committed(
            &pij_store::Assignment {
                project: project.into(),
                role: "coder".into(),
                instance: "224".into(),
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

#[tokio::test]
async fn an_idle_seat_with_an_empty_composer_gets_one_restart_and_nobody_is_told() {
    let fleet = fleet(SystemState::Idle, None).await;
    let outcome = report(&fleet, FIRST, 10 * MINUTE).await;
    assert_eq!(outcome.action, Action::Restart);
    assert!(outcome.told.is_empty());
    let queued = controls(&fleet).await;
    assert_eq!(queued.len(), 1, "one restart control is queued");
    assert!(queued[0].contains("\"restart\""), "{queued:?}");
    assert!(
        told(&fleet, PARENT).await.is_empty(),
        "the parent is not told"
    );
}

#[tokio::test]
async fn a_busy_seat_is_never_restarted_its_parent_is_told_how() {
    let fleet = fleet(SystemState::Working, None).await;
    let outcome = report(&fleet, FIRST, 10 * MINUTE).await;
    assert_eq!(outcome.action, Action::Notify);
    assert!(
        controls(&fleet).await.is_empty(),
        "no restart for a busy seat"
    );
    assert_eq!(outcome.told.len(), 1, "{:?}", outcome.told);
    assert_eq!(outcome.told[0].0, SeatId::from(PARENT));
    let body = told(&fleet, PARENT).await.join("\n");
    assert!(body.contains("/restart in pane %21"), "{body}");
    assert!(
        body.contains("pij send pij-copilot-coder --command restart"),
        "{body}"
    );
}

#[tokio::test]
async fn a_draft_in_the_composer_spares_the_seat() {
    let fleet = fleet(SystemState::Idle, Some("half a thought")).await;
    let outcome = handle(
        &fleet.services,
        &coder(&fleet).await,
        FIRST,
        Cause::Reported(LostLogin::CredentialAuthority),
        10 * MINUTE,
    )
    .await
    .unwrap();
    assert_eq!(outcome.action, Action::Notify);
    assert!(
        controls(&fleet).await.is_empty(),
        "a draft is never typed over"
    );
}

/// The same process again while its restart is still queued: nothing more.
/// The restarted process loses its login too: the login needs renewing, said
/// once, and never a second restart.
#[tokio::test]
async fn a_second_loss_in_the_window_never_restarts_again() {
    let fleet = fleet(SystemState::Idle, None).await;
    let start = 10 * MINUTE;
    assert_eq!(report(&fleet, FIRST, start).await.action, Action::Restart);
    let pending = report(&fleet, FIRST, start + MINUTE).await;
    assert_eq!(pending.action, Action::Pending);
    assert!(pending.told.is_empty());

    settle_restart(&fleet, Outcome::Done).await;
    let relogin = report(&fleet, SECOND, start + 2 * MINUTE).await;
    assert_eq!(relogin.action, Action::Relogin);
    assert_eq!(relogin.told.len(), 1);
    let body = told(&fleet, PARENT).await.join("\n");
    assert!(body.contains("login needs renewing"), "{body}");
    assert!(body.contains("/login in pane %21"), "{body}");
    assert_eq!(
        report(&fleet, SECOND, start + 3 * MINUTE).await.action,
        Action::Told
    );
    assert!(controls(&fleet).await.is_empty(), "still the one restart");
}

/// R254-001: the restart settled (typed, or failed) and the same process still
/// reports: Copilot did not restart. They are told how to, once.
#[tokio::test]
async fn a_restart_that_settles_without_a_new_process_is_reported_once() {
    for outcome in [
        Outcome::Done,
        Outcome::Failed {
            reason: "copilot-command-unavailable".into(),
        },
    ] {
        let fleet = fleet(SystemState::Idle, None).await;
        let start = 10 * MINUTE;
        assert_eq!(report(&fleet, FIRST, start).await.action, Action::Restart);
        settle_restart(&fleet, outcome.clone()).await;
        let stuck = report(&fleet, FIRST, start + MINUTE).await;
        assert_eq!(stuck.action, Action::NotRestarted, "{outcome:?}");
        assert_eq!(stuck.told.len(), 1, "{outcome:?}");
        let body = told(&fleet, PARENT).await.join("\n");
        assert!(body.contains("the restart did not take"), "{body}");
        assert!(body.contains("pij-rs revive pij-copilot-coder"), "{body}");
        assert_eq!(
            report(&fleet, FIRST, start + 2 * MINUTE).await.action,
            Action::Told,
            "{outcome:?}"
        );
        assert_eq!(told(&fleet, PARENT).await.len(), 1, "told once");
    }
}

/// R254-002: the window is monotonic. A wall clock that jumps two hours ahead
/// does not end it, so a still-queued restart stays the only one.
#[tokio::test]
async fn a_wall_clock_jump_never_ends_the_window() {
    let fleet = fleet(SystemState::Idle, None).await;
    let start = 10 * MINUTE;
    assert_eq!(report(&fleet, FIRST, start).await.action, Action::Restart);
    assert_eq!(
        report(&fleet, FIRST, start + 120 * MINUTE).await.action,
        Action::Pending
    );
    assert_eq!(controls(&fleet).await.len(), 1);
}

/// R254-003: two reports in flight together decide one at a time: one
/// restart, one pending, one control.
#[tokio::test]
async fn two_reports_at_once_queue_one_restart() {
    let fleet = fleet(SystemState::Idle, None).await;
    let (first, second) = tokio::join!(
        report(&fleet, FIRST, 10 * MINUTE),
        report(&fleet, FIRST, 10 * MINUTE + 1)
    );
    let mut actions = [first.action, second.action];
    actions.sort_by_key(|action| *action == Action::Pending);
    assert_eq!(actions, [Action::Restart, Action::Pending]);
    assert_eq!(controls(&fleet).await.len(), 1);
}

/// A daemon restart forgets the ledger; the seat's recovery event is the brake
/// then, so a queued restart is still the only one.
#[tokio::test]
async fn a_daemon_restart_remembers_the_window_from_the_spine() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    let start = 10 * MINUTE;
    assert_eq!(report(&fleet, FIRST, start).await.action, Action::Restart);
    fleet.services.auth_recovery = Arc::new(Ledger::new());
    assert_eq!(
        report(&fleet, FIRST, start + MINUTE).await.action,
        Action::Pending
    );
    assert_eq!(controls(&fleet).await.len(), 1);
}

/// R254-005: the live parent and each held project's designated prime are
/// told, once each, never the seat itself.
#[tokio::test]
async fn the_parent_and_each_projects_prime_are_told_once() {
    let fleet = fleet(SystemState::Working, None).await;
    hold_in(&fleet, "crane", PRIME).await;
    hold_in(&fleet, "heron", PARENT).await;
    hold_in(&fleet, "egret", SEAT).await;
    let outcome = report(&fleet, FIRST, 10 * MINUTE).await;
    assert_eq!(outcome.action, Action::Notify);
    let mut told_to: Vec<SeatId> = outcome.told.into_iter().map(|(to, _)| to).collect();
    told_to.sort();
    assert_eq!(told_to, [SeatId::from(PARENT), SeatId::from(PRIME)]);
    assert_eq!(told(&fleet, PARENT).await.len(), 1);
    assert_eq!(told(&fleet, PRIME).await.len(), 1);
    assert!(told(&fleet, SEAT).await.is_empty(), "never the seat itself");
}

#[test]
fn the_window_bounds_every_brake() {
    let now = Instant::now() + 2 * RECOVERY_WINDOW;
    let entry = |action, since| Entry {
        action,
        since,
        incarnation: FIRST,
        msg_id: Some("auth-restart".into()),
    };
    let recent = now - Duration::from_secs(60);
    let restart = entry(Action::Restart, recent);
    let notify = entry(Action::Notify, recent);
    let yes = || true;
    let no = || false;
    assert_eq!(decide(None, now, FIRST, true, no, yes), Action::Restart);
    assert_eq!(decide(None, now, FIRST, true, no, no), Action::Notify);
    assert_eq!(
        decide(Some(&restart), now, FIRST, true, no, yes),
        Action::Pending
    );
    assert_eq!(
        decide(Some(&restart), now, FIRST, true, yes, yes),
        Action::NotRestarted
    );
    assert_eq!(
        decide(Some(&restart), now, SECOND, true, no, yes),
        Action::Relogin
    );
    assert_eq!(
        decide(Some(&notify), now, FIRST, true, yes, yes),
        Action::Told
    );
    // F261: a process that is not running is never restarted, in the window
    // or out of it.
    assert_eq!(decide(None, now, FIRST, false, no, yes), Action::Exited);
    assert_eq!(
        decide(Some(&restart), now, FIRST, false, yes, yes),
        Action::Exited
    );
    assert_eq!(
        decide(Some(&notify), now, FIRST, false, yes, yes),
        Action::Told
    );
    // At the window's end the seat may be restarted again.
    let old = now - RECOVERY_WINDOW;
    assert_eq!(
        decide(
            Some(&entry(Action::Restart, old)),
            now,
            FIRST,
            true,
            no,
            yes
        ),
        Action::Restart
    );
    assert_eq!(
        decide(Some(&entry(Action::Notify, old)), now, FIRST, true, no, no),
        Action::Notify
    );
}

#[test]
fn only_the_two_lost_login_kinds_are_accepted() {
    for (wire, kind) in [
        ("\"authorization\"", LostLogin::Authorization),
        ("\"credential-authority\"", LostLogin::CredentialAuthority),
    ] {
        assert_eq!(serde_json::from_str::<LostLogin>(wire).unwrap(), kind);
    }
    // F261: `extension-lost` is the daemon's own cause; no caller may claim it.
    for wire in [
        "\"quota\"",
        "\"authentication\"",
        "\"\"",
        "\"extension-lost\"",
    ] {
        assert!(serde_json::from_str::<LostLogin>(wire).is_err(), "{wire}");
    }
}

/// F261: the kernel runs the coder's recorded process (pid and start).
fn running(fleet: &mut Fleet) {
    fleet.services.liveness = Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
        pid: FIRST.pid,
        proc_start: FIRST.proc_start,
    }));
}

async fn lapse(fleet: &Fleet, now_ms: u64) -> Option<super::Outcome> {
    on_lapse(&fleet.services, &SeatId::from(SEAT), now_ms)
        .await
        .unwrap()
}

/// F261: Copilot force-killed the pij extension (its enterprise sign-in
/// lapsed) and the daemon told the lease lapse. The process still runs and
/// the seat is idle with an empty composer: one `/restart`, recorded as
/// `extension-lost`, nobody told. A second lapse in the window adds nothing.
#[tokio::test]
async fn a_lapse_of_a_running_idle_copilot_seat_restarts_it_once() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    let follower = tokio::spawn(super::follow_lapses(fleet.services.clone()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    fleet
        .services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: 1,
            at: 10 * MINUTE,
            kind: crate::delivery::NATIVE_RECEIVER_LAPSED.to_string(),
            seat: Some(SeatId::from(SEAT)),
            payload: serde_json::json!({"seat": SEAT, "renewed_at": null}).to_string(),
        })
        .await
        .unwrap();
    let mut queued = Vec::new();
    for _ in 0..200 {
        queued = controls(&fleet).await;
        if !queued.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    follower.abort();
    assert_eq!(queued.len(), 1, "one restart control is queued");
    assert!(queued[0].contains("\"restart\""), "{queued:?}");
    assert!(told(&fleet, PARENT).await.is_empty(), "nobody is told");
    let recorded = fleet
        .services
        .spine
        .latest_matching(&SeatId::from(SEAT), &[super::AUTH_RECOVERY_KIND])
        .await
        .unwrap()
        .expect("the decision is recorded");
    let payload: serde_json::Value = serde_json::from_str(&recorded.payload).unwrap();
    assert_eq!(payload["kind"], "extension-lost");
    assert_eq!(payload["action"], "restart");
    let again = lapse(&fleet, 11 * MINUTE).await.expect("decided");
    assert_eq!(again.action, Action::Pending);
    assert_eq!(controls(&fleet).await.len(), 1, "still the one restart");
}

/// F261: a lapse whose process is gone (no such pid) or recycled (another
/// start at that pid) is an exit or a crash. Never a restart: the parent is
/// told to revive it, once in the window.
#[tokio::test]
async fn a_lapse_whose_process_is_gone_never_restarts_and_says_revive() {
    for kernel in [
        FakeLiveness::new(),
        FakeLiveness::new().with_proc(ProcIdentity {
            pid: FIRST.pid,
            proc_start: FIRST.proc_start + 1,
        }),
    ] {
        let mut fleet = fleet(SystemState::Idle, None).await;
        fleet.services.liveness = Arc::new(kernel);
        let exited = lapse(&fleet, 10 * MINUTE).await.expect("decided");
        assert_eq!(exited.action, Action::Exited);
        assert!(controls(&fleet).await.is_empty(), "never a restart");
        assert_eq!(exited.told.len(), 1, "{:?}", exited.told);
        let body = told(&fleet, PARENT).await.join("\n");
        assert!(body.contains("pij-rs revive pij-copilot-coder"), "{body}");
        assert!(!body.contains("/restart"), "{body}");
        assert_eq!(
            lapse(&fleet, 11 * MINUTE).await.expect("decided").action,
            Action::Told
        );
        assert_eq!(told(&fleet, PARENT).await.len(), 1, "told once");
    }
}

/// F261: the extension's own report and the lease lapse share one window: a
/// loss seen both ways is restarted once, whichever comes first.
#[tokio::test]
async fn a_lapse_and_a_report_share_one_restart() {
    let mut fleet_a = fleet(SystemState::Idle, None).await;
    running(&mut fleet_a);
    assert_eq!(
        report(&fleet_a, FIRST, 10 * MINUTE).await.action,
        Action::Restart
    );
    assert_eq!(
        lapse(&fleet_a, 11 * MINUTE).await.expect("decided").action,
        Action::Pending
    );
    assert_eq!(controls(&fleet_a).await.len(), 1);

    let mut fleet_b = fleet(SystemState::Idle, None).await;
    running(&mut fleet_b);
    assert_eq!(
        lapse(&fleet_b, 10 * MINUTE).await.expect("decided").action,
        Action::Restart
    );
    assert_eq!(
        report(&fleet_b, FIRST, 11 * MINUTE).await.action,
        Action::Pending
    );
    assert_eq!(controls(&fleet_b).await.len(), 1);
}

/// F261: the same brakes as a report. A busy seat or a draft in the composer
/// is never restarted on a lapse; the parent is told how.
#[tokio::test]
async fn a_lapsed_seat_that_is_busy_or_holds_a_draft_is_not_restarted() {
    for (state, draft) in [
        (SystemState::Working, None),
        (SystemState::Idle, Some("half a thought")),
    ] {
        let mut fleet = fleet(state, draft).await;
        running(&mut fleet);
        let outcome = lapse(&fleet, 10 * MINUTE).await.expect("decided");
        assert_eq!(outcome.action, Action::Notify, "{state:?} {draft:?}");
        assert!(controls(&fleet).await.is_empty(), "{state:?} {draft:?}");
        assert_eq!(told(&fleet, PARENT).await.len(), 1, "{state:?} {draft:?}");
    }
}

/// F261: a lapse decides only a live Copilot seat with a recorded process.
/// Any other harness, a retired seat, or one never bound is left alone.
#[tokio::test]
async fn a_lapse_of_any_other_seat_does_nothing() {
    for change in ["claude", "retired", "unbound"] {
        let mut fleet = fleet(SystemState::Idle, None).await;
        running(&mut fleet);
        let mut seat = coder(&fleet).await;
        match change {
            "claude" => seat.harness = Harness::Claude,
            "retired" => seat.tombstoned_at = Some(1),
            _ => seat.proc = None,
        }
        fleet.services.registry.put(seat).await.unwrap();
        assert!(lapse(&fleet, 10 * MINUTE).await.is_none(), "{change}");
        assert!(controls(&fleet).await.is_empty(), "{change}");
        assert!(told(&fleet, PARENT).await.is_empty(), "{change}");
    }
}

/// A second native Copilot seat, whose process the kernel does not run.
const OTHER: &str = "pij-copilot-tester";

/// B311-01: the coder delivers natively (a reconcile reads only such seats),
/// and `other` adds a second native Copilot seat beside it.
async fn native(fleet: &Fleet, other: bool) {
    let mut seat = coder(fleet).await;
    seat.native_extension_delivery = true;
    let mut second = seat.clone();
    fleet.services.registry.put(seat).await.unwrap();
    if other {
        second.id = SeatId::from(OTHER);
        second.pane = Some("%22".into());
        second.harness_session = Some("native-tester".into());
        second.proc = Some(ProcIdentity {
            pid: 4300,
            proc_start: 11,
        });
        fleet.services.registry.put(second).await.unwrap();
    }
}

/// One spine event of `kind` for `seat`, through the bus (and so live).
async fn edge(fleet: &Fleet, kind: &str, seat: Option<&str>) {
    fleet
        .services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: 1,
            at: 10 * MINUTE,
            kind: kind.to_string(),
            seat: seat.map(SeatId::from),
            payload: serde_json::json!({"seat": seat, "renewed_at": null}).to_string(),
        })
        .await
        .unwrap();
}

/// The coder's queued controls once one appears, or none after two seconds.
async fn controls_soon(fleet: &Fleet) -> Vec<String> {
    for _ in 0..200 {
        let queued = controls(fleet).await;
        if !queued.is_empty() {
            return queued;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Vec::new()
}

/// B311-01: a lapse told before the follower attached (a daemon restart
/// restores it without telling it again, R277-2) is decided at attachment:
/// one restart.
#[tokio::test]
async fn a_lapse_told_before_the_follower_attached_is_restarted_once() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, false).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    let follower = tokio::spawn(super::follow_lapses(fleet.services.clone()));
    let queued = controls_soon(&fleet).await;
    follower.abort();
    assert_eq!(queued.len(), 1, "one restart control is queued");
    assert!(queued[0].contains("\"restart\""), "{queued:?}");
}

/// B311-01: a lapse overwritten in a one-slot broadcast ring before the
/// follower read it is decided after the gap: one restart. The follower is
/// held on the other seat's lock while the ring overflows.
#[tokio::test]
async fn a_lapse_overwritten_in_the_broadcast_ring_is_restarted_once() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, true).await;
    let spine = fleet.services.event_bus.raw_spine();
    fleet.services.event_bus = Arc::new(crate::events::EventBus::new(spine, 1).unwrap());
    let other = fleet.services.auth_recovery.slot(&SeatId::from(OTHER));
    let held = other.lock().await;
    let follower = tokio::spawn(super::follow_lapses(fleet.services.clone()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(OTHER)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    for _ in 0..3 {
        edge(&fleet, "test.noise", None).await;
    }
    drop(held);
    let queued = controls_soon(&fleet).await;
    follower.abort();
    assert_eq!(queued.len(), 1, "one restart control is queued");
}

/// B311-01: a lapse its receiver renewed before the reconcile read it is
/// never decided; a lapse still outstanding is.
#[tokio::test]
async fn a_reconcile_never_replays_a_lapse_its_receiver_renewed() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, false).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_RENEWED, Some(SEAT)).await;
    super::reconcile(&fleet.services).await;
    assert!(
        controls(&fleet).await.is_empty(),
        "a renewed lapse is spared"
    );
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    super::reconcile(&fleet.services).await;
    assert_eq!(controls(&fleet).await.len(), 1, "an outstanding one is not");
    super::reconcile(&fleet.services).await;
    assert_eq!(
        controls(&fleet).await.len(),
        1,
        "a decided one is not again"
    );
}

/// B311-01: a lapse an earlier daemon already decided, its window since
/// over, is not replayed when a new daemon reconciles: the reconcile acts
/// only on a lapse with nothing decided after it.
#[tokio::test]
async fn a_lapse_an_earlier_daemon_decided_is_not_decided_again() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, false).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    let long_ago = super::now_ms() - 40 * MINUTE;
    fleet
        .services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: 1,
            at: long_ago,
            kind: super::AUTH_RECOVERY_KIND.to_string(),
            seat: Some(SeatId::from(SEAT)),
            payload: serde_json::json!({
                "action": "notify",
                "at": long_ago,
                "pid": FIRST.pid,
                "proc_start": FIRST.proc_start,
                "kind": "extension-lost",
                "msg_id": null,
            })
            .to_string(),
        })
        .await
        .unwrap();
    super::reconcile(&fleet.services).await;
    assert!(controls(&fleet).await.is_empty(), "never decided twice");
}

/// N311-01: a lapse renewed before the follower decides it is spared. The
/// follower is held on the other seat's lock while the coder's lapse and its
/// renewal are told.
#[tokio::test]
async fn a_lapse_renewed_before_it_is_decided_is_spared() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, true).await;
    let other = fleet.services.auth_recovery.slot(&SeatId::from(OTHER));
    let held = other.lock().await;
    let follower = tokio::spawn(super::follow_lapses(fleet.services.clone()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(OTHER)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_RENEWED, Some(SEAT)).await;
    drop(held);
    let queued = controls_soon(&fleet).await;
    follower.abort();
    assert!(queued.is_empty(), "{queued:?}");
}

/// A kernel whose process table cannot be read.
struct Unreadable;

#[async_trait::async_trait]
impl pij_core::ports::LivenessPort for Unreadable {
    async fn proc_start(&self, _pid: u32) -> pij_core::error::Result<Option<u64>> {
        Err(pij_core::error::PijError::Adapter {
            adapter: "test/liveness".to_string(),
            message: "unreadable".to_string(),
        })
    }
}

/// F261, security note 1: a start that cannot be read is not running: never
/// a restart, the parent is told to revive it.
#[tokio::test]
async fn a_lapse_whose_process_cannot_be_read_never_restarts() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    fleet.services.liveness = Arc::new(Unreadable);
    let outcome = lapse(&fleet, 10 * MINUTE).await.expect("decided");
    assert_eq!(outcome.action, Action::Exited);
    assert!(controls(&fleet).await.is_empty(), "never a restart");
}
