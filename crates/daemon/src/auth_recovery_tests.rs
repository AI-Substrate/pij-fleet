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
    Action, Brake, Cause, Entry, Incarnation, Ledger, LostLogin, RECOVERY_WINDOW, decide, handle,
    on_lapse,
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
    tmux: Arc<FakeTmux>,
    _store: FreshStore,
}

/// A captured Copilot 1.0.115 screen (F279): `idle` (its prompt) or
/// `working` (mid-turn: the same empty composer, a turn status under it).
pub(crate) fn copilot_screen(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../testkit/fixtures/harnesses")
            .join(format!("copilot-1.0.115-{name}.txt")),
    )
    .expect("read a Copilot fixture")
}

/// `pane` shows the captured Copilot screen `name` with `draft` in its
/// composer (row 2), the caret after it.
pub(crate) fn show(tmux: &FakeTmux, pane: &str, name: &str, draft: Option<&str>) {
    let screen = copilot_screen(name);
    let (screen, caret) = match draft {
        Some(text) => (
            screen.replacen("❯\n", &format!("❯ {text}\n"), 1),
            2 + u32::try_from(text.chars().count()).unwrap(),
        ),
        None => (screen, 2),
    };
    tmux.arrange_screen(pane, &screen, caret, 2);
}

/// A PM, a prime and the PM's Copilot coder (pid 4100), with a fake tmux whose
/// coder pane shows Copilot's prompt with `draft` in its composer, or none.
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
    // F274: no boot wait unless a test asks for one (each fleet is a fresh
    // daemon, so every lapse would otherwise wait one 60 s lease).
    services
        .delivery
        .set_native_boot_wait(Duration::from_millis(1));
    let tmux = Arc::new(FakeTmux::new());
    show(&tmux, PANE, "idle", draft);
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
        tmux,
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
    .expect("a report always has an outcome")
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
    assert_eq!(outcome.held_by, Some(Brake::Busy), "a report reads the row");
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
    .unwrap()
    .expect("a report always has an outcome");
    assert_eq!(outcome.action, Action::Notify);
    assert_eq!(outcome.held_by, Some(Brake::ComposerDraft));
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

/// The coder's latest `seat.auth_recovery` payload.
async fn recorded(fleet: &Fleet) -> serde_json::Value {
    let event = fleet
        .services
        .spine
        .latest_matching(&SeatId::from(SEAT), &[super::AUTH_RECOVERY_KIND])
        .await
        .unwrap()
        .expect("a recorded decision");
    serde_json::from_str(&event.payload).unwrap()
}

/// F279: a lapse is the death of the extension that publishes the seat's
/// turn state, so that state is frozen and never read: the pane decides,
/// whatever the row says. The prompt with an empty composer restarts. A
/// running turn queues the restart behind it and tells the parent. A draft,
/// or a pane with no live control client to read it, is a notice and never a
/// restart. `held_by` names what held it, on the outcome and the record.
#[tokio::test]
async fn a_lapsed_seats_pane_decides_never_its_published_state() {
    for state in [SystemState::Working, SystemState::Idle] {
        for (screen, draft, action, held, queued, notices) in [
            ("idle", None, Action::Restart, None, 1, 0),
            ("working", None, Action::Restart, Some(Brake::Busy), 1, 1),
            (
                "idle",
                Some("half a thought"),
                Action::Notify,
                Some(Brake::ComposerDraft),
                0,
                1,
            ),
            (
                "unreadable",
                None,
                Action::Notify,
                Some(Brake::ComposerUnreadable),
                0,
                1,
            ),
        ] {
            let case = format!("{state:?} {screen} {draft:?}");
            let mut fleet = fleet(state, None).await;
            running(&mut fleet);
            if screen == "unreadable" {
                fleet.tmux.lose_control_client();
            } else {
                show(&fleet.tmux, PANE, screen, draft);
            }
            let outcome = lapse(&fleet, 10 * MINUTE).await.expect("decided");
            assert_eq!(outcome.action, action, "{case}");
            assert_eq!(outcome.held_by, held, "{case}");
            assert_eq!(controls(&fleet).await.len(), queued, "{case}");
            let body = told(&fleet, PARENT).await;
            assert_eq!(body.len(), notices, "{case}");
            if held == Some(Brake::Busy) {
                assert!(body[0].contains("pij queued /restart"), "{case}: {body:?}");
            }
            assert_eq!(
                recorded(&fleet).await["held_by"],
                serde_json::to_value(held).unwrap(),
                "{case}"
            );
        }
    }
}

/// The pointer drain worker over the fleet's own services.
fn drain(fleet: &Fleet) -> crate::pointer::DrainWorker {
    let services = &fleet.services;
    crate::pointer::DrainWorker::new(
        Arc::clone(&services.registry),
        Arc::clone(&services.queue),
        Arc::clone(&services.transport),
        Arc::clone(&services.tmux),
        Arc::clone(&services.interaction),
        Arc::clone(&services.event_bus),
        crate::pointer::PointerPolicy {
            cadence: Duration::from_secs(1),
            announcement_limit: 1,
        },
    )
    .unwrap()
}

/// How many `/restart`s were typed into the coder's pane.
fn restarts_typed(fleet: &Fleet) -> usize {
    let typed = format!("submit:{PANE}:/restart");
    fleet
        .tmux
        .calls()
        .iter()
        .filter(|call| **call == typed)
        .count()
}

/// Drain until `done` holds, for at most ten seconds: held controls back off
/// one, then two, then four seconds.
async fn drain_until(fleet: &Fleet, worker: &crate::pointer::DrainWorker, done: impl Fn() -> bool) {
    for _ in 0..50 {
        let _ = worker.drain_once().await;
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("not done after ten seconds: {:?}", fleet.tmux.calls());
}

/// The parent's own restart control, as `pij send <seat> --command restart`.
async fn parent_restart(fleet: &Fleet, id: &str) {
    fleet
        .services
        .delivery
        .accept_control(pij_core::model::Msg {
            command: Some("restart".into()),
            from: SeatId::from(PARENT),
            ..crate::pa_watchdog::bg_msg(&SeatId::from(SEAT), String::new(), id.into())
        })
        .await
        .unwrap();
}

/// The prime's F279 case. A seat lapses mid-turn: its row stays `working`,
/// since the extension that would publish its idle is gone. It finishes its
/// turn and is restarted once idle, with no human. Its parent, told, sends a
/// restart of its own: one `/restart` is typed, not two. Before the
/// restarted extension renews, the parent's control waits; after, it is
/// dropped, never typed.
#[tokio::test]
async fn a_seat_that_lapses_mid_turn_is_restarted_once_when_its_turn_ends() {
    let mut fleet = fleet(SystemState::Working, None).await;
    running(&mut fleet);
    show(&fleet.tmux, PANE, "working", None);
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    let outcome = lapse(&fleet, 10 * MINUTE).await.expect("decided");
    assert_eq!(
        (outcome.action, outcome.held_by),
        (Action::Restart, Some(Brake::Busy))
    );
    parent_restart(&fleet, "parent-restart").await;
    assert_eq!(controls(&fleet).await.len(), 2);
    let worker = drain(&fleet);

    assert!(worker.drain_once().await.unwrap() > 0);
    assert_eq!(restarts_typed(&fleet), 0, "held while the turn runs");

    show(&fleet.tmux, PANE, "idle", None);
    assert_eq!(coder(&fleet).await.state, SystemState::Working, "frozen");
    drain_until(&fleet, &worker, || restarts_typed(&fleet) == 1).await;

    // The restarted Copilot shows its prompt before its extension renews.
    // The other control, claimed then, waits (`restart-pending`), untyped.
    show(&fleet.tmux, PANE, "idle", None);
    let mut held = false;
    for _ in 0..50 {
        let _ = worker.drain_once().await;
        held = fleet
            .services
            .queue
            .delivery_deferrals(&SeatId::from(SEAT))
            .await
            .unwrap()
            .iter()
            .any(|deferral| deferral.reason == "restart-pending");
        if held || restarts_typed(&fleet) > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(restarts_typed(&fleet), 1, "one restart per lapse");
    assert!(held, "the other control was claimed and held");

    edge(&fleet, crate::delivery::NATIVE_RECEIVER_RENEWED, Some(SEAT)).await;
    let live = |fleet: &Fleet| {
        let queue = Arc::clone(&fleet.services.queue);
        async move {
            queue
                .live_payloads(&[control_kind(&SeatId::from(SEAT))])
                .await
                .unwrap()
                .len()
        }
    };
    for _ in 0..50 {
        let _ = worker.drain_once().await;
        if live(&fleet).await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(live(&fleet).await, 0, "both controls closed");
    assert_eq!(restarts_typed(&fleet), 1, "the parent's was never typed");
}

/// F279 at the drain: a restart control to a lapsed seat waits for its pane.
/// A running turn, a draft and a pane with no live control client to read
/// hold it whatever the row says, and the prompt with an empty composer types
/// it. A seat that has not lapsed is read from its row, as before: `working`
/// holds even when the pane shows the prompt.
#[tokio::test]
async fn a_restart_control_waits_for_a_lapsed_seats_pane() {
    for (lapsed, state, screen, draft, typed) in [
        (true, SystemState::Idle, "working", None, 0),
        (true, SystemState::Idle, "idle", Some("half a thought"), 0),
        (true, SystemState::Idle, "unreadable", None, 0),
        (true, SystemState::Working, "idle", None, 1),
        (false, SystemState::Working, "idle", None, 0),
        (false, SystemState::Idle, "idle", None, 1),
    ] {
        let case = format!("lapsed={lapsed} {state:?} {screen} {draft:?}");
        let fleet = fleet(state, None).await;
        if screen == "unreadable" {
            fleet.tmux.lose_control_client();
        } else {
            show(&fleet.tmux, PANE, screen, draft);
        }
        if lapsed {
            edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
        }
        parent_restart(&fleet, "parent-restart").await;
        drain(&fleet).drain_once().await.unwrap();
        assert_eq!(restarts_typed(&fleet), typed, "{case}");
        assert_eq!(controls(&fleet).await.len(), 1 - typed, "{case}");
    }
}

/// B334-R1-01 (pij-vertical-marmot): a receiver that renews while the drain
/// settles (between its two pane reads, 250 ms apart) has come back: the
/// restart queued before that renewal is closed, never typed. The renewal
/// lands after the first read; the drain reads the edge again after both.
#[tokio::test]
async fn a_renewal_between_the_pane_reads_types_no_restart() {
    let fleet = fleet(SystemState::Working, None).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    parent_restart(&fleet, "parent-restart").await;
    let worker = drain(&fleet);
    let reads = || {
        fleet
            .tmux
            .calls()
            .iter()
            .filter(|call| call.starts_with("capture_without_launch:"))
            .count()
    };
    // Bounded (two seconds): a drain that never reads the pane ends this
    // test by its assertions, never by hanging.
    let renew = async {
        for _ in 0..1_000 {
            if reads() > 0 {
                edge(&fleet, crate::delivery::NATIVE_RECEIVER_RENEWED, Some(SEAT)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    let (drained, ()) = tokio::join!(worker.drain_once(), renew);
    drained.unwrap();
    assert_eq!(reads(), 2, "the renewal fell between the two pane reads");
    assert_eq!(
        restarts_typed(&fleet),
        0,
        "an obsolete restart is not typed"
    );
    assert!(controls(&fleet).await.is_empty(), "it is closed, not held");
}

/// The other side of the same brake: a restart queued after the receiver
/// renewed is a new request, and is typed.
#[tokio::test]
async fn a_restart_queued_after_a_renewal_is_typed() {
    let fleet = fleet(SystemState::Idle, None).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_RENEWED, Some(SEAT)).await;
    parent_restart(&fleet, "parent-restart").await;
    drain(&fleet).drain_once().await.unwrap();
    assert_eq!(restarts_typed(&fleet), 1);
    assert!(controls(&fleet).await.is_empty());
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

/// A follower reconciling at most once per `interval`, attached and
/// reconciled, whose ring (one slot) then loses the coder's lapse: the
/// follower is held on the other seat's lock while the ring overflows, then
/// let go. Returns the follower.
async fn overflowed(fleet: &mut Fleet, interval: Duration) -> tokio::task::JoinHandle<()> {
    running(fleet);
    native(fleet, true).await;
    let spine = fleet.services.event_bus.raw_spine();
    fleet.services.event_bus = Arc::new(crate::events::EventBus::new(spine, 1).unwrap());
    let other = fleet.services.auth_recovery.slot(&SeatId::from(OTHER));
    let held = other.lock().await;
    let follower = tokio::spawn(super::follow_lapses_every(fleet.services.clone(), interval));
    tokio::time::sleep(Duration::from_millis(50)).await;
    edge(fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(OTHER)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    edge(fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    for _ in 0..3 {
        edge(fleet, "test.noise", None).await;
    }
    drop(held);
    follower
}

/// B311-01: a lapse overwritten in a one-slot broadcast ring before the
/// follower read it is decided after the gap: one restart.
#[tokio::test]
async fn a_lapse_overwritten_in_the_broadcast_ring_is_restarted_once() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    let follower = overflowed(&mut fleet, Duration::ZERO).await;
    let queued = controls_soon(&fleet).await;
    follower.abort();
    assert_eq!(queued.len(), 1, "one restart control is queued");
}

/// F268: a gap sooner than the least interval after the last reconcile
/// waits out the rest of it, then reconciles: the lapse is still decided,
/// once, but a gap storm costs at most one reconcile per interval.
#[tokio::test]
async fn a_gap_soon_after_a_reconcile_waits_out_the_interval_then_decides() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    let interval = Duration::from_millis(1_500);
    let started = tokio::time::Instant::now();
    let follower = overflowed(&mut fleet, interval).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        started.elapsed() < interval && controls(&fleet).await.is_empty(),
        "no reconcile inside the interval"
    );
    let queued = controls_soon(&fleet).await;
    follower.abort();
    assert_eq!(queued.len(), 1, "decided after it");
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

/// F268: a lapse decided as `told` (its parent was told inside the window,
/// before this lapse) writes nothing acting, but is marked decided, so the
/// next reconcile does not decide it again.
#[tokio::test]
async fn a_lapse_decided_with_nothing_to_do_is_not_decided_again() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, false).await;
    let recently = super::now_ms() - 10 * MINUTE;
    fleet
        .services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: 1,
            at: recently,
            kind: super::AUTH_RECOVERY_KIND.to_string(),
            seat: Some(SeatId::from(SEAT)),
            payload: serde_json::json!({
                "action": "notify",
                "at": recently,
                "pid": FIRST.pid,
                "proc_start": FIRST.proc_start,
                "kind": "authorization",
                "msg_id": null,
            })
            .to_string(),
        })
        .await
        .unwrap();
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    super::reconcile(&fleet.services).await;
    super::reconcile(&fleet.services).await;
    let held: Vec<String> = fleet
        .services
        .spine
        .tail(Some(&SeatId::from(SEAT)), pij_core::model::Seq(0))
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == super::LAPSE_HELD_KIND)
        .map(|event| event.payload)
        .collect();
    assert_eq!(held.len(), 1, "decided once: {held:?}");
    assert!(held[0].contains("\"told\""), "{held:?}");
    assert!(controls(&fleet).await.is_empty());
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

/// F268 (B320-01): a receiver that renewed its lease with this daemon after
/// its lapse was told is back, though no `renewed` is on the spine (its
/// append failed and waits for the next heartbeat): never restarted for that
/// lapse, by a reconcile or the follower. A lapse with no renewal since this
/// daemon started is still decided (see the attach test above).
#[tokio::test]
async fn a_receiver_renewed_since_its_lapse_is_never_restarted() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    running(&mut fleet);
    native(&fleet, false).await;
    let identity = crate::delivery::NativeInboxIdentity {
        native_session: Some("native-212".into()),
        pid: Some(FIRST.pid),
        proc_start: Some(FIRST.proc_start),
    };
    fleet
        .services
        .delivery
        .heartbeat_native_receiver(&SeatId::from(SEAT), &identity, 1, 1, None)
        .await
        .expect("the receiver renews");
    edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
    super::reconcile(&fleet.services).await;
    assert!(lapse(&fleet, super::now_ms()).await.is_none(), "spared");
    let follower = tokio::spawn(super::follow_lapses(fleet.services.clone()));
    let queued = controls_soon(&fleet).await;
    follower.abort();
    assert!(queued.is_empty(), "{queued:?}");
    assert!(told(&fleet, PARENT).await.is_empty());
}

/// The coder's receiver heartbeats, as its extension does.
async fn heartbeat(delivery: &crate::delivery::DeliveryService) {
    let identity = crate::delivery::NativeInboxIdentity {
        native_session: Some("native-212".into()),
        pid: Some(FIRST.pid),
        proc_start: Some(FIRST.proc_start),
    };
    delivery
        .heartbeat_native_receiver(&SeatId::from(SEAT), &identity, 1, 1, None)
        .await
        .expect("the receiver renews");
}

/// The coder's queued controls once `wait` has passed.
async fn controls_after(fleet: &Fleet, wait: Duration) -> Vec<String> {
    tokio::time::sleep(wait).await;
    controls(fleet).await
}

/// F274 (a): a lapse outstanding when the daemon starts (an earlier daemon
/// told it) waits one lease after the boot when this daemon has not heard
/// from the seat: a receiver that heartbeats within that wait is spared; one
/// that stays silent is restarted once the wait is over.
#[tokio::test]
async fn a_lapse_at_boot_waits_one_lease_for_its_receiver() {
    for heard in [true, false] {
        let mut fleet = fleet(SystemState::Idle, None).await;
        running(&mut fleet);
        native(&fleet, false).await;
        let wait = Duration::from_millis(1_500);
        fleet.services.delivery.set_native_boot_wait(wait);
        edge(&fleet, crate::delivery::NATIVE_RECEIVER_LAPSED, Some(SEAT)).await;
        let follower = tokio::spawn(super::follow_lapses(fleet.services.clone()));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            controls(&fleet).await.is_empty(),
            "heard={heard}: decided before the wait"
        );
        if heard {
            heartbeat(&fleet.services.delivery).await;
        }
        let queued = controls_after(&fleet, Duration::from_millis(2_500)).await;
        follower.abort();
        assert_eq!(
            queued.len(),
            usize::from(!heard),
            "heard={heard}: {queued:?}"
        );
    }
}

/// A kernel whose process read lets the coder's receiver heartbeat first, as
/// a renewal landing while the lapse decision reads the process.
struct RenewsWhileRead {
    delivery: Arc<crate::delivery::DeliveryService>,
}

#[async_trait::async_trait]
impl pij_core::ports::LivenessPort for RenewsWhileRead {
    async fn proc_start(&self, pid: u32) -> pij_core::error::Result<Option<u64>> {
        heartbeat(&self.delivery).await;
        Ok((pid == FIRST.pid).then_some(FIRST.proc_start))
    }
}

/// F274 (b): a renewal that lands after the lapse's lease brake was read,
/// while the decision reads the process, still spares the seat: the lease is
/// read again just before a restart is recorded or queued.
#[tokio::test]
async fn a_renewal_during_the_lapse_decision_spares_the_seat() {
    let mut fleet = fleet(SystemState::Idle, None).await;
    native(&fleet, false).await;
    fleet.services.liveness = Arc::new(RenewsWhileRead {
        delivery: Arc::clone(&fleet.services.delivery),
    });
    assert!(lapse(&fleet, super::now_ms()).await.is_none(), "spared");
    assert!(controls(&fleet).await.is_empty());
    assert!(
        fleet
            .services
            .spine
            .latest_matching(&SeatId::from(SEAT), &[super::AUTH_RECOVERY_KIND])
            .await
            .unwrap()
            .is_none(),
        "nothing recorded"
    );
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
