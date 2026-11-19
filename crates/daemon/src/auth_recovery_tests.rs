use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId, SystemState};
use pij_core::ports::TmuxPort;
use pij_harnesses::InteractionGate;
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeTmux;

use super::{Action, LostLogin, RECOVERY_WINDOW_MS, Recorded, decide, handle};

const SEAT: &str = "pij-copilot-coder";
const PARENT: &str = "pij-pm";
const PANE: &str = "%21";
const MINUTE: u64 = 60_000;

struct Fleet {
    services: crate::Services,
    _store: FreshStore,
}

/// A PM and its Copilot coder (pid 4100), with a fake tmux whose coder pane
/// shows `draft` in the composer, or an empty one.
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
    let mut coder = SeatDescriptor::new(SEAT, Harness::Copilot, &folder);
    coder.parent = Some(SeatId::from(PARENT));
    coder.pane = Some(PANE.into());
    coder.harness_session = Some("native-212".into());
    coder.proc = Some(ProcIdentity {
        pid: 4100,
        proc_start: 7,
    });
    coder.state = state;
    for seat in [pm, coder] {
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

/// Every control queued for the coder, by its typed command.
async fn controls(fleet: &Fleet) -> Vec<String> {
    let kind = format!("delivery-control:{SEAT}");
    let mut found = Vec::new();
    if let Some((_, job)) = fleet.services.queue.peek(&[kind]).await.unwrap() {
        let payload: serde_json::Value = serde_json::from_str(&job.payload).unwrap();
        found.push(payload.to_string());
    }
    found
}

/// What reached the PM: each queued message body.
async fn told(fleet: &Fleet, seat: &str) -> Vec<String> {
    let kind = pij_core::delivery::delivery_kind(&SeatId::from(seat));
    let mut bodies = Vec::new();
    if let Some((_, job)) = fleet.services.queue.peek(&[kind]).await.unwrap() {
        bodies.push(job.payload);
    }
    bodies
}

#[tokio::test]
async fn an_idle_seat_with_an_empty_composer_gets_one_restart_and_nobody_is_told() {
    let fleet = fleet(SystemState::Idle, None).await;
    let outcome = handle(
        &fleet.services,
        &coder(&fleet).await,
        LostLogin::Authorization,
        10 * MINUTE,
    )
    .await
    .unwrap();
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
    let outcome = handle(
        &fleet.services,
        &coder(&fleet).await,
        LostLogin::Authorization,
        10 * MINUTE,
    )
    .await
    .unwrap();
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

#[tokio::test]
async fn a_second_loss_in_the_window_never_restarts_again() {
    let fleet = fleet(SystemState::Idle, None).await;
    let seat = coder(&fleet).await;
    let start = 10 * MINUTE;
    assert_eq!(
        handle(&fleet.services, &seat, LostLogin::Authorization, start)
            .await
            .unwrap()
            .action,
        Action::Restart
    );
    // The same process again (the restart has not run yet): nothing more.
    let pending = handle(
        &fleet.services,
        &seat,
        LostLogin::Authorization,
        start + MINUTE,
    )
    .await
    .unwrap();
    assert_eq!(pending.action, Action::Pending);
    assert!(pending.told.is_empty());

    // The restarted process (a new pid) loses its login too: re-login needed,
    // said once, and no second restart.
    let mut restarted = seat.clone();
    restarted.proc = Some(ProcIdentity {
        pid: 4200,
        proc_start: 9,
    });
    let relogin = handle(
        &fleet.services,
        &restarted,
        LostLogin::Authorization,
        start + 2 * MINUTE,
    )
    .await
    .unwrap();
    assert_eq!(relogin.action, Action::Relogin);
    assert_eq!(relogin.told.len(), 1);
    let body = told(&fleet, PARENT).await.join("\n");
    assert!(body.contains("login needs renewing"), "{body}");
    assert!(body.contains("/login in pane %21"), "{body}");
    assert_eq!(
        handle(
            &fleet.services,
            &restarted,
            LostLogin::Authorization,
            start + 3 * MINUTE
        )
        .await
        .unwrap()
        .action,
        Action::Told
    );
    assert_eq!(controls(&fleet).await.len(), 1, "still the one restart");
}

#[test]
fn the_window_bounds_every_brake() {
    let restart = Recorded {
        action: Action::Restart,
        at: 0,
        pid: Some(1),
    };
    let notify = Recorded {
        action: Action::Notify,
        at: 0,
        pid: Some(1),
    };
    let free = || true;
    let busy = || false;
    assert_eq!(decide(None, 0, Some(1), free), Action::Restart);
    assert_eq!(decide(None, 0, Some(1), busy), Action::Notify);
    assert_eq!(decide(Some(&restart), 1, Some(1), free), Action::Pending);
    assert_eq!(decide(Some(&restart), 1, Some(2), free), Action::Relogin);
    assert_eq!(decide(Some(&restart), 1, None, free), Action::Relogin);
    assert_eq!(decide(Some(&notify), 1, Some(1), free), Action::Told);
    // At the window's end the seat may be restarted again.
    assert_eq!(
        decide(Some(&restart), RECOVERY_WINDOW_MS, Some(1), free),
        Action::Restart
    );
    assert_eq!(
        decide(Some(&notify), RECOVERY_WINDOW_MS, Some(1), busy),
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
