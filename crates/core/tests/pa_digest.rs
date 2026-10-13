//! The PA watchdog digest: what counts as fleet change, and what the PA reads.

use pij_core::cold_wake::{ColdWakeView, SeatSize};
use pij_core::model::{Harness, SeatDescriptor, SeatId, SemanticState, SystemState};
use pij_core::pa_digest::{FleetAnomaly, FleetRow, FleetView};
use pij_core::session_status::CacheState;

const MIN: u64 = 60_000;

/// One edit to the fleet: its rows, and the view that holds anomalies.
type Edit = dyn Fn(&mut Vec<FleetRow>, &mut FleetView);
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
fn the_pas_own_activity_never_counts_as_fleet_change() {
    let before = view(fleet());
    let mut rows = fleet();
    rows[0].size.context_used = Some(140_000);
    rows[0].seat.state = SystemState::Working;
    rows[0].seat.semantic_state = None;
    assert_eq!(
        before.fingerprint(),
        view(rows).fingerprint(),
        "answering one nudge must not manufacture the next"
    );
}

#[test]
fn passing_time_alone_is_quiet() {
    let before = view(fleet());
    let mut rows = fleet();
    rows[1].size.idle_ms = Some(9 * MIN);
    rows[1].size.cache_state = Some(CacheState::Cold {
        expired_for_ms: MIN,
    });
    assert_eq!(before.fingerprint(), view(rows).fingerprint());
}

#[test]
fn every_real_change_is_a_change() {
    let base = view(fleet()).fingerprint();
    let changed = |edit: &Edit| {
        let mut rows = fleet();
        let mut target = view(Vec::new());
        edit(&mut rows, &mut target);
        target.rows = rows;
        target.fingerprint()
    };

    let cases: [(&str, &Edit); 6] = [
        ("context grew", &|rows, _| {
            rows[2].size.context_used = Some(700_000)
        }),
        ("turn ended", &|rows, _| {
            rows[2].seat.state = SystemState::Idle
        }),
        ("declared a state", &|rows, _| {
            rows[1].seat.semantic_state = Some(SemanticState::Question);
        }),
        ("went quiet past the interval", &|rows, _| {
            rows[1].size.idle_ms = Some(INTERVAL_MS);
        }),
        ("a seat joined", &|rows, _| {
            rows.push(row("pij-new", None, SystemState::Idle, None, 10_000, 0));
        }),
        ("an anomaly opened", &|_, target| {
            target.anomalies.push(FleetAnomaly {
                seat: SeatId::from("pij-coder".to_string()),
                kind: "status-stale".into(),
                detail: "card 2h old".into(),
            });
        }),
    ];
    for (name, edit) in cases {
        assert_ne!(base, changed(edit), "{name} must count as change");
    }
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
    };
    let text = view(rows).digest();

    assert!(text.starts_with(
        "[pij watchdog] fleet round for pij-prime (/repo/.git) — 3 seats: 1 working, 2 idle, 1 cold ❄"
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
        "Set no timers or background loops; pij nudges you again when the fleet changes."
    ));
}
