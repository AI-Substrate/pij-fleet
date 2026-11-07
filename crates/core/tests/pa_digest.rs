//! The PA watchdog digest: what the PA reads when its fleet goes idle.

use pij_core::cold_wake::{ColdWakeView, SeatSize};
use pij_core::model::{Harness, SeatDescriptor, SeatId, SemanticState, SystemState};
use pij_core::pa_digest::{FleetAnomaly, FleetRow, FleetView, QueuedRequest};
use pij_core::session_status::CacheState;

const MIN: u64 = 60_000;

const INTERVAL_MS: u64 = 20 * MIN;

fn row(
    id: &str,
    role: Option<&str>,
    state: SystemState,
    semantic: Option<SemanticState>,
    context: u64,
    idle_ms: u64,
) -> FleetRow {
    let mut seat = SeatDescriptor::new(id, Harness::Claude, "/repo");
    seat.role = role.map(str::to_string);
    seat.state = state;
    seat.semantic_state = semantic;
    FleetRow {
        seat,
        size: SeatSize {
            context_used: Some(context),
            idle_ms: Some(idle_ms),
            cache_state: Some(CacheState::Warm {
                expires_in_ms: 60_000,
            }),
            cold_wake: ColdWakeView::default(),
        },
    }
}

fn view(rows: Vec<FleetRow>) -> FleetView {
    FleetView {
        pa: SeatId::from("pij-pa".to_string()),
        prime: Some(SeatId::from("pij-prime".to_string())),
        scope: "/repo/.git".into(),
        rows,
        anomalies: Vec::new(),
        queued: Vec::new(),
        interval_ms: INTERVAL_MS,
    }
}

fn fleet() -> Vec<FleetRow> {
    vec![
        row(
            "pij-pa",
            Some("pa"),
            SystemState::Idle,
            Some(SemanticState::Waiting),
            90_000,
            MIN,
        ),
        row(
            "pij-prime",
            Some("prime"),
            SystemState::Idle,
            None,
            400_000,
            5 * MIN,
        ),
        row("pij-coder", None, SystemState::Working, None, 650_000, 0),
    ]
}

#[test]
fn digest_flags_parked_and_quiet_seats_but_not_working_or_recent_ones() {
    let mut rows = fleet();
    rows.push(row(
        "pij-asker",
        None,
        SystemState::Idle,
        Some(SemanticState::Question),
        50_000,
        3 * MIN,
    ));
    rows.push(row(
        "pij-stuck",
        None,
        SystemState::Idle,
        Some(SemanticState::Ready),
        60_000,
        45 * MIN,
    ));
    rows.push(row(
        "pij-fresh",
        None,
        SystemState::Idle,
        Some(SemanticState::Ready),
        70_000,
        2 * MIN,
    ));
    let mut target = view(rows);
    target.anomalies.push(FleetAnomaly {
        seat: SeatId::from("pij-coder".to_string()),
        kind: "status-stale".into(),
        detail: "card 2h old".into(),
    });
    let text = target.digest();
    let needs = text
        .split("Needs a look:")
        .nth(1)
        .and_then(|rest| rest.split("Seats by context:").next())
        .expect("a needs-a-look section");

    assert!(needs.contains("pij-asker  question, quiet 3m"), "{text}");
    assert!(needs.contains("pij-stuck  ready, quiet 45m"), "{text}");
    assert!(
        needs.contains("pij-coder  status-stale: card 2h old"),
        "{text}"
    );
    assert!(
        !needs.contains("pij-fresh"),
        "quiet for less than an interval: {text}"
    );
    assert!(
        !needs.contains("pij-pa "),
        "the PA is never flagged to itself: {text}"
    );
    assert!(
        !needs
            .lines()
            .any(|line| line.trim_start().starts_with("pij-coder  working")),
        "a working seat is not a stall: {text}"
    );
}

#[test]
fn digest_lists_every_seat_largest_first_and_marks_the_pa_and_cold_wakes() {
    let mut rows = fleet();
    rows[1].size.cold_wake = ColdWakeView {
        would_refuse: true,
        estimate_usd: Some(6.2),
        not_estimable: None,
    };
    let text = view(rows).digest();

    assert!(text.starts_with(
        "[pij watchdog] fleet idle for pij-prime (/repo/.git) — 3 seats: 1 working, 2 idle, 1 cold ❄"
    ));
    let order: Vec<&str> = ["pij-coder", "pij-prime", "pij-pa"]
        .into_iter()
        .map(|id| {
            text.lines()
                .find(|line| line.trim_start().starts_with(id) && line.contains("k "))
                .unwrap_or_else(|| panic!("{id} row missing: {text}"))
        })
        .collect();
    assert!(order[0].contains("650k"));
    let positions: Vec<usize> = order
        .iter()
        .map(|line| text.find(line).expect("row present"))
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "largest first: {text}"
    );
    assert!(order[1].contains("❄ $6.20 to wake"), "{text}");
    assert!(order[2].ends_with("(you)"), "{text}");
    assert!(text.ends_with(
        "Set no timers or background loops; pij nudges you again the next time the fleet goes idle."
    ));
}

/// Plan 199: requests waiting on a role with no live holder ride the digest
/// until an assign drains them, and say nothing when there are none.
#[test]
fn digest_lists_queued_role_requests_only_when_there_are_some() {
    let quiet = view(fleet()).digest();
    assert!(!quiet.contains("Queued role requests"), "{quiet}");

    let mut waiting = view(fleet());
    waiting.queued = vec![QueuedRequest {
        address: "role:pm-001".into(),
        project: "fleet".into(),
        id: "req-a".into(),
        requester: SeatId::from("pij-asker".to_string()),
        waiting_ms: 5 * 60_000,
        delivering_to: None,
    }];
    let digest = waiting.digest();
    assert!(
        digest.contains(
            "\nQueued role requests (no live holder; assigning one delivers them):\n  role:pm-001 in fleet: request req-a from pij-asker, waiting 5m"
        ),
        "{digest}"
    );
    assert!(!digest.contains("stuck delivering"), "{digest}");

    // Review of #129, F003: a claim never settled is shown with its holder.
    waiting.queued.push(QueuedRequest {
        address: "role:reviewer".into(),
        project: "fleet".into(),
        id: "req-b".into(),
        requester: SeatId::from("pij-asker".to_string()),
        waiting_ms: 20 * 60_000,
        delivering_to: Some(SeatId::from("pij-holder".to_string())),
    });
    let digest = waiting.digest();
    assert!(
        digest.contains(
            "\nRole requests stuck delivering (claimed, never confirmed; a daemon restart re-queues them):\n  role:reviewer in fleet: request req-b from pij-asker to pij-holder, waiting 20m"
        ),
        "{digest}"
    );
    assert!(
        !digest.contains("request req-b from pij-asker, waiting"),
        "a stuck request is not listed as queued: {digest}"
    );
}
