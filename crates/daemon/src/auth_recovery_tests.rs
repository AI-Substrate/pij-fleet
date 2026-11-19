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
use pij_testkit::fakes::FakeTmux;

use super::{Action, Entry, Incarnation, Ledger, LostLogin, RECOVERY_WINDOW, decide, handle};

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
        LostLogin::Authorization,
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
        LostLogin::CredentialAuthority,
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
    assert_eq!(decide(None, now, FIRST, no, yes), Action::Restart);
    assert_eq!(decide(None, now, FIRST, no, no), Action::Notify);
    assert_eq!(decide(Some(&restart), now, FIRST, no, yes), Action::Pending);
    assert_eq!(
        decide(Some(&restart), now, FIRST, yes, yes),
        Action::NotRestarted
    );
    assert_eq!(
        decide(Some(&restart), now, SECOND, no, yes),
        Action::Relogin
    );
    assert_eq!(decide(Some(&notify), now, FIRST, yes, yes), Action::Told);
    // At the window's end the seat may be restarted again.
    let old = now - RECOVERY_WINDOW;
    assert_eq!(
        decide(Some(&entry(Action::Restart, old)), now, FIRST, no, yes),
        Action::Restart
    );
    assert_eq!(
        decide(Some(&entry(Action::Notify, old)), now, FIRST, no, no),
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
    for wire in ["\"quota\"", "\"authentication\"", "\"\""] {
        assert!(serde_json::from_str::<LostLogin>(wire).is_err(), "{wire}");
    }
}
