//! Pure watchdog decision tables and incident regressions.

use pij_core::config::Config;
use pij_core::model::{Harness, SeatDescriptor, SemanticState, SystemState};
use pij_core::watchdog::{NudgeReason, PauseTier, WatchdogControl, WatchdogEntry, WatchdogService};

const NOW: u64 = 10_000;
const INTERVAL: u64 = 1_200;

fn seat(semantic: Option<SemanticState>, state: SystemState) -> SeatDescriptor {
    role_seat(Some("pa"), semantic, state)
}

fn role_seat(
    role: Option<&str>,
    semantic: Option<SemanticState>,
    state: SystemState,
) -> SeatDescriptor {
    let mut seat = SeatDescriptor::new("pij-watchdog-target", Harness::Omp, "/abs/worktree");
    seat.role = role.map(str::to_string);
    seat.semantic_state = semantic;
    seat.state = state;
    seat
}

fn config() -> Config {
    Config {
        watchdog_interval_secs: INTERVAL,
        ..Config::default()
    }
}

fn entry(seat: SeatDescriptor, control: WatchdogControl, age_secs: u64) -> WatchdogEntry {
    WatchdogEntry {
        seat,
        last_activity_at_secs: NOW - age_secs,
        control,
    }
}

fn nudges(entry: WatchdogEntry) -> Vec<pij_core::watchdog::Nudge> {
    WatchdogService::new(&config(), vec![entry]).tick(NOW)
}

/// PAs get the watchdog by default and nobody else does, not even primes
/// (Jordan, 2026-10-09). A PA's declared state never suppresses its nudge:
/// it is the fleet's watchdog, and between rounds it sits in `waiting`.
#[test]
fn every_role_pause_tier_and_declared_state_has_one_nudge_decision() {
    let roles = [None, Some("prime"), Some("pm"), Some("worker"), Some("pa")];
    let semantics = [
        None,
        Some(SemanticState::Ready),
        Some(SemanticState::Waiting),
        Some(SemanticState::Hold),
        Some(SemanticState::Blocked),
        Some(SemanticState::Question),
        Some(SemanticState::Done),
        Some(SemanticState::Failed),
        Some(SemanticState::Cancelled),
    ];

    for self_paused in [false, true] {
        for compact_paused in [false, true] {
            for exempt in [false, true] {
                let control =
                    WatchdogControl::new(self_paused, compact_paused, exempt.then_some(NOW + 1));
                for role in roles {
                    for state in [SystemState::Idle, SystemState::Working] {
                        for semantic in semantics {
                            let actual =
                                !nudges(entry(role_seat(role, semantic, state), control, INTERVAL))
                                    .is_empty();
                            let expected =
                                role == Some("pa") && !self_paused && !compact_paused && !exempt;
                            assert_eq!(
                                actual, expected,
                                "role={role:?} self={self_paused} compact={compact_paused} exempt={exempt} state={state:?} semantic={semantic:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn pause_precedence_is_exempt_then_compact_then_self() {
    let cases = [
        (false, false, false, None),
        (true, false, false, Some(PauseTier::SelfPaused)),
        (false, true, false, Some(PauseTier::Compact)),
        (true, true, false, Some(PauseTier::Compact)),
        (false, false, true, Some(PauseTier::Exempt)),
        (true, false, true, Some(PauseTier::Exempt)),
        (false, true, true, Some(PauseTier::Exempt)),
        (true, true, true, Some(PauseTier::Exempt)),
    ];

    for (self_paused, compact_paused, exempt, expected) in cases {
        let control = WatchdogControl::new(self_paused, compact_paused, exempt.then_some(NOW + 1));
        assert_eq!(
            control.effective_pause(NOW),
            expected,
            "self={self_paused} compact={compact_paused} exempt={exempt}"
        );
    }
}

#[test]
fn exemption_rearms_at_its_deadline_without_erasing_weaker_tiers() {
    let deadline = NOW;
    let control = WatchdogControl::new(true, true, Some(deadline));

    assert_eq!(
        control.effective_pause(deadline - 1),
        Some(PauseTier::Exempt)
    );
    assert_eq!(
        control.effective_pause(deadline),
        Some(PauseTier::Compact),
        "the deadline is exact; compact becomes visible underneath it"
    );

    let reconciled = control.reconciled(deadline);
    assert_eq!(
        reconciled.effective_pause(deadline),
        Some(PauseTier::Compact)
    );
    assert_eq!(reconciled.reconciled(deadline), reconciled, "idempotent");
}

#[test]
fn real_working_transition_clears_only_the_compact_tier() {
    let control = WatchdogControl::new(true, true, Some(NOW + 1));
    let transitioned = control.after_working_transition();

    assert_eq!(transitioned.effective_pause(NOW), Some(PauseTier::Exempt));
    assert_eq!(
        transitioned.effective_pause(NOW + 1),
        Some(PauseTier::SelfPaused),
        "working clears compact but cannot weaken exemption or operator pause"
    );
}

/// The PA nudge is a clock, not a stall verdict, so TS's false stall (a
/// healthy long tool call read as stuck) cannot arise: a working PA is nudged
/// on schedule and delivery decides how the message lands.
#[test]
fn a_working_pa_is_still_nudged_on_schedule() {
    let overdue = entry(
        seat(None, SystemState::Working),
        WatchdogControl::default(),
        INTERVAL,
    );
    assert_eq!(nudges(overdue).len(), 1);
}

#[test]
fn configured_threshold_and_derived_reason_are_exact() {
    let eligible = seat(Some(SemanticState::Ready), SystemState::Idle);
    assert!(
        nudges(entry(
            eligible.clone(),
            WatchdogControl::default(),
            INTERVAL - 1,
        ))
        .is_empty(),
        "not due one second before the configured interval"
    );

    let due = nudges(entry(eligible, WatchdogControl::default(), INTERVAL));
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].seat.as_str(), "pij-watchdog-target");
    assert_eq!(due[0].reason, NudgeReason::OverdueIdle);
}

#[test]
fn tombstoned_relay_and_future_activity_are_never_nudged() {
    let mut tombstoned = seat(None, SystemState::Idle);
    tombstoned.tombstoned_at = Some(NOW - 1);
    assert!(nudges(entry(tombstoned, WatchdogControl::default(), INTERVAL,)).is_empty());

    let mut relay = seat(Some(SemanticState::Ready), SystemState::Idle);
    relay.relay = true;
    assert!(nudges(entry(relay, WatchdogControl::default(), INTERVAL)).is_empty());

    let future = WatchdogEntry {
        seat: seat(None, SystemState::Idle),
        last_activity_at_secs: NOW + 1,
        control: WatchdogControl::default(),
    };
    assert!(
        nudges(future).is_empty(),
        "clock skew cannot manufacture age"
    );
}
