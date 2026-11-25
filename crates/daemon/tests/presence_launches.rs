//! Plan 217's presence checks cost no process launch (AGENTS.md § Resource
//! cost, ac-0008), and neither does placing a seat's folder in its
//! repository (F202).
//!
//! Over 46 simulated minutes the round sends all four presence messages (a
//! missing-PA wake, a dormant wake, an unresponsive alert and a large-seat
//! FYI) to seats in a main checkout, a linked worktree and a second
//! repository, and a seat joins a third: the launch counter never moves, not
//! even on the first round. Seats in folders the in-process read cannot
//! decide fall back to git: one launch per folder, however many seats, then
//! none while git's answer is kept. That also shows the instrument reads a
//! launch when there is one.
//!
//! ONE test on purpose: it reads the process-wide launch counter.

use std::path::Path;
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

fn git(dir: &Path, args: &[&str]) {
    let mut command = pij_testkit::git_env::git(dir);
    let output = pij_testkit::git_env::scrub_identity(&mut command)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

fn canonical(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .expect("canonical folder")
        .display()
        .to_string()
}

fn repo(root: &Path, name: &str) -> String {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("repo dir");
    git(&dir, &["init", "-q"]);
    canonical(&dir)
}

/// A linked worktree of `main`, as a sibling folder named `name`.
fn worktree(root: &Path, main: &str, name: &str) -> String {
    git(
        Path::new(main),
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    let dir = root.join(name);
    git(
        Path::new(main),
        &[
            "worktree",
            "add",
            "-q",
            dir.to_str().expect("utf-8"),
            "-b",
            name,
        ],
    );
    canonical(&dir)
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
async fn the_presence_checks_and_placing_folders_launch_nothing() {
    let root = std::env::temp_dir().join(format!("pij-presence-launches-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    let root = std::fs::canonicalize(&root).expect("canonical root");
    let fleet = repo(&root, "fleet");
    let feature = worktree(&root, &fleet, "feature");
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

    // A designated prime with a PA in the main checkout and two workers in a
    // linked worktree (a third, large, joins later); and a `prime`-role seat
    // in another repository whose two workers have no PA.
    let seats = [
        seat("pij-prime", &fleet, None, SystemState::Idle),
        seat("pij-pa", &fleet, Some("pij-prime"), SystemState::Idle),
        seat(
            "pij-busy",
            &feature,
            Some("pij-prime"),
            SystemState::Working,
        ),
        seat("pij-idle", &feature, Some("pij-prime"), SystemState::Idle),
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
    let cold = snapshot();
    collect(round(0).await, &mut sent);
    for minute in 1..=45 {
        if minute == 5 {
            services
                .registry
                .put(seat(
                    "pij-big",
                    &feature,
                    Some("pij-prime"),
                    SystemState::Idle,
                ))
                .await
                .expect("large seat");
        }
        collect(round(minute).await, &mut sent);
    }
    // A seat joins a third repository: placed in process too.
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
        after.total, cold.total,
        "no launch in 47 rounds, the first included: {:?} -> {:?}",
        cold.by_op, after.by_op
    );

    // The fallback's bound, and the positive control: four seats in two
    // folders the in-process read cannot decide (a garbled `.git`) cost one
    // launch per folder on the first round, then none while git's answer is
    // kept.
    for (index, name) in ["odd-a", "odd-a", "odd-b", "odd-b"].into_iter().enumerate() {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("odd folder");
        std::fs::write(dir.join(".git"), "not a gitdir pointer\n").expect("garbled .git");
        services
            .registry
            .put(seat(
                &format!("pij-odd-{index}"),
                &canonical(&dir),
                Some("pij-prime"),
                SystemState::Idle,
            ))
            .await
            .expect("odd seat");
    }
    let git_launches = || snapshot().by_op.get(GIT_SITE).copied().unwrap_or(0);
    let before = git_launches();
    round(47).await;
    assert_eq!(git_launches(), before + 2, "one git per undecided folder");
    round(48).await;
    round(49).await;
    assert_eq!(git_launches(), before + 2, "git's answer is kept");
    let _ = std::fs::remove_dir_all(&root);
}
