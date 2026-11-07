//! Plan 217's presence checks cost no process launch (AGENTS.md § Resource
//! cost, ac-0008).
//!
//! Over 45 simulated minutes the round sends all four presence messages (a
//! missing-PA wake, a dormant wake, an unresponsive alert and a large-seat
//! FYI). After the first round has placed each folder in its repository,
//! the launch counter does not move. A seat in a new repository then moves
//! it by exactly one, so the instrument can read a launch when there is one.
//!
//! ONE test on purpose: it reads the process-wide launch counter.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use pij_core::config::{AdapterChoice, Adapters, Config};
use pij_core::model::{Harness, SeatDescriptor, SeatId, SystemState};
use pij_core::orchestration::{PrimeDesignation, PrimeState};
use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
use pij_core::spawns::snapshot;
use pij_core::watchdog::{PA_ROLE, PRIME_ROLE, PresenceKind};
use pij_daemon::pa_watchdog::{PaWatchdog, RoundOutcome};
use pij_testkit::FreshStore;
use pij_testkit::fakes::FakeSessionStatus;

const GIT_SITE: &str = "git.pa-watchdog-common-dir";

fn repo(root: &Path, name: &str) -> String {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("repo dir");
    let init = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["init", "-q"])
        .output()
        .expect("git init");
    assert!(init.status.success(), "{init:?}");
    std::fs::canonicalize(dir)
        .expect("canonical repo")
        .display()
        .to_string()
}

fn seat(id: &str, folder: &str, parent: Option<&str>, state: SystemState) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(id, Harness::Claude, folder);
    seat.parent = parent.map(SeatId::from);
    seat.harness_session = Some(format!("session-{id}"));
    seat.state = state;
    seat
}

fn sized(tokens: u64) -> SessionStatusReply {
    SessionStatusReply::Status(SeatStatus {
        context_used_tokens: Fact::derived(tokens),
        ..SeatStatus::unknown()
    })
}

#[tokio::test]
async fn the_presence_checks_launch_nothing_once_folders_are_placed() {
    let root = std::env::temp_dir().join(format!("pij-presence-launches-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let fleet = repo(&root, "fleet");
    let lonely = repo(&root, "lonely");
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
        pij_daemon::build_services(&config, &Path::new(&store.path()).with_extension("signals"))
            .await
            .expect("real store services");
    let mut sizes = FakeSessionStatus::new();
    for id in ["pij-prime", "pij-pa", "pij-busy", "pij-idle"] {
        sizes = sizes.with_reply(&format!("session-{id}"), sized(100_000));
    }
    services.session_status = Arc::new(sizes.with_reply("session-pij-big", sized(700_000)));

    // A designated prime with a PA and two workers (a third, large, joins
    // later); and a `prime`-role seat whose two workers have no PA.
    let seats = [
        seat("pij-prime", &fleet, None, SystemState::Idle),
        seat("pij-pa", &fleet, Some("pij-prime"), SystemState::Idle),
        seat("pij-busy", &fleet, Some("pij-prime"), SystemState::Working),
        seat("pij-idle", &fleet, Some("pij-prime"), SystemState::Idle),
        seat("pij-other", &lonely, None, SystemState::Idle),
        seat("pij-w1", &lonely, Some("pij-other"), SystemState::Working),
        seat("pij-w2", &lonely, Some("pij-other"), SystemState::Idle),
    ];
    for descriptor in &seats {
        services
            .registry
            .put(descriptor.clone())
            .await
            .expect("seat");
    }
    services
        .watchdogs()
        .designate_prime(&PrimeDesignation {
            seat: "pij-prime".into(),
            designated_by: "pij-human".into(),
            designated_at: 1,
            state: PrimeState::Current,
        })
        .await
        .expect("designation");
    services
        .roles
        .assert_role(&"pij-prime".into(), &"pij-pa".into(), Some(PA_ROLE.into()))
        .await
        .expect("pa role");
    services
        .roles
        .assert_role(
            &"pij-other".into(),
            &"pij-other".into(),
            Some(PRIME_ROLE.into()),
        )
        .await
        .expect("prime role");

    let watchdog = PaWatchdog::default();
    let start = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("epoch ms");
    let mut sent = Vec::new();
    let round = |minute: u64| {
        let (services, watchdog) = (&services, &watchdog);
        async move {
            watchdog
                .round(services, 1_200, start + minute * 60_000)
                .await
                .expect("round")
        }
    };
    let collect = |outcomes: Vec<RoundOutcome>, sent: &mut Vec<PresenceKind>| {
        sent.extend(outcomes.into_iter().filter_map(|outcome| match outcome {
            RoundOutcome::Presence { kind, .. } => Some(kind),
            _ => None,
        }));
    };
    collect(round(0).await, &mut sent);
    let warm = snapshot();
    for minute in 1..=45 {
        if minute == 5 {
            services
                .registry
                .put(seat(
                    "pij-big",
                    &fleet,
                    Some("pij-prime"),
                    SystemState::Idle,
                ))
                .await
                .expect("large seat");
        }
        collect(round(minute).await, &mut sent);
    }
    let after = snapshot();
    sent.sort();
    assert_eq!(
        sent,
        [
            PresenceKind::Missing,
            PresenceKind::Dormant,
            PresenceKind::Unresponsive,
            PresenceKind::LargeSeat
        ],
        "every check sent once"
    );
    assert_eq!(
        after.total, warm.total,
        "no launch in 45 rounds: {:?} -> {:?}",
        warm.by_op, after.by_op
    );

    // Positive control: a seat in a repository the round has not placed.
    let fresh = repo(&root, "fresh");
    services
        .registry
        .put(seat(
            "pij-new",
            &fresh,
            Some("pij-prime"),
            SystemState::Idle,
        ))
        .await
        .expect("new seat");
    round(46).await;
    let placed = snapshot();
    assert_eq!(
        placed.by_op.get(GIT_SITE).copied().unwrap_or(0),
        after.by_op.get(GIT_SITE).copied().unwrap_or(0) + 1,
        "the counter reads the one launch that places a new folder"
    );
    let _ = std::fs::remove_dir_all(&root);
}
