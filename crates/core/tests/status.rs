use std::collections::BTreeSet;

use pij_core::model::{Harness, SeatDescriptor, SemanticState, SystemState};
use pij_core::status::{BADGE_SEVERITY, badge_of};

#[test]
fn badge_severity_is_the_exact_closed_vocabulary() {
    assert_eq!(
        BADGE_SEVERITY,
        [
            "dead",
            "failed",
            "stalled",
            "blocked",
            "question",
            "hold",
            "stopped",
            "unknown",
            "waiting",
            "starting",
            "working",
            "ready",
            "cancelled",
            "done",
            "idle",
        ]
    );
    for (rank, word) in BADGE_SEVERITY.iter().enumerate() {
        let system = SystemState::parse(word);
        let semantic = SemanticState::parse(word);
        assert_ne!(
            system.is_some(),
            semantic.is_some(),
            "{word} belongs to exactly one axis"
        );
        if let Some(state) = system {
            assert_eq!(system_rank(state), rank);
            assert_eq!(state.as_str(), *word);
            assert_eq!(serde_json::to_value(state).unwrap(), *word);
        }
        if let Some(state) = semantic {
            assert_eq!(semantic_rank(state), rank);
            assert_eq!(state.as_str(), *word);
            assert_eq!(serde_json::to_value(state).unwrap(), *word);
        }
        let states: Vec<_> = semantic.into_iter().collect();
        assert_eq!(badge_of(system, &states), *word);
    }
    assert_eq!(SystemState::parse("busy"), None);
    assert_eq!(SemanticState::parse("busy"), None);
}

#[test]
fn every_state_has_a_published_badge_slot() {
    for state in SystemState::ALL {
        assert!(
            BADGE_SEVERITY.contains(&state.as_str()),
            "{state:?} has no published badge slot"
        );
        assert_eq!(badge_of(Some(*state), &[]), state.as_str());
    }
    for state in SemanticState::ALL {
        assert!(
            BADGE_SEVERITY.contains(&state.as_str()),
            "{state:?} has no published badge slot"
        );
        assert_eq!(badge_of(None, &[*state]), state.as_str());
    }
}

#[test]
fn semantic_words_match_every_declared_variant() {
    let declared: BTreeSet<_> = SemanticState::ALL
        .iter()
        .map(|state| state.as_str())
        .collect();
    let published: BTreeSet<_> = SemanticState::WORDS.split('|').collect();
    assert_eq!(published, declared);
}

// Exhaustive matches make an enum addition require an explicit severity decision.
fn system_rank(state: SystemState) -> usize {
    match state {
        SystemState::Dead => 0,
        SystemState::Stalled => 2,
        SystemState::Stopped => 6,
        SystemState::Unknown => 7,
        SystemState::Starting => 9,
        SystemState::Working => 10,
        SystemState::Idle => 14,
    }
}

fn semantic_rank(state: SemanticState) -> usize {
    match state {
        SemanticState::Failed => 1,
        SemanticState::Blocked => 3,
        SemanticState::Question => 4,
        SemanticState::Hold => 5,
        SemanticState::Waiting => 8,
        SemanticState::Ready => 11,
        SemanticState::Cancelled => 12,
        SemanticState::Done => 13,
    }
}

#[test]
fn badge_chooses_the_worst_of_multiple_open_assignments() {
    assert_eq!(
        badge_of(
            Some(SystemState::Idle),
            &[SemanticState::Done, SemanticState::Blocked]
        ),
        "blocked"
    );
    assert_eq!(
        badge_of(
            Some(SystemState::Working),
            &[SemanticState::Failed, SemanticState::Ready]
        ),
        "failed"
    );
    assert_eq!(
        badge_of(
            Some(SystemState::Dead),
            &[SemanticState::Failed, SemanticState::Blocked]
        ),
        "dead"
    );
    for (rank, word) in BADGE_SEVERITY.iter().enumerate() {
        for other in &BADGE_SEVERITY[rank..] {
            let system = SystemState::parse(word).or_else(|| SystemState::parse(other));
            let states: Vec<_> = [SemanticState::parse(word), SemanticState::parse(other)]
                .into_iter()
                .flatten()
                .collect();
            if SystemState::parse(word).is_some() && SystemState::parse(other).is_some() {
                continue;
            }
            assert_eq!(badge_of(system, &states), *word);
            let reversed: Vec<_> = states.into_iter().rev().collect();
            assert_eq!(badge_of(system, &reversed), *word);
        }
    }
}

#[test]
fn badge_without_either_axis_is_unknown() {
    assert_eq!(badge_of(None, &[]), "unknown");
}

#[test]
fn legacy_state_words_still_decode_without_new_metadata() {
    for word in ["idle", "working"] {
        let state: SystemState = serde_json::from_value(serde_json::json!(word)).unwrap();
        assert_eq!(state.as_str(), word);
    }
    for word in ["ready", "waiting", "hold", "blocked", "question", "done"] {
        let state: SemanticState = serde_json::from_value(serde_json::json!(word)).unwrap();
        assert_eq!(state.as_str(), word);
    }
}

#[test]
fn peer_status_metadata_decodes_but_raw_descriptors_never_emit_it() {
    let descriptor = SeatDescriptor::new("pij-remote", Harness::Omp, "/work/remote");
    let mut peer_row = serde_json::to_value(&descriptor).unwrap();
    // A pre-147 peer omits both read-projection fields.
    peer_row.as_object_mut().unwrap().remove("badge");
    peer_row.as_object_mut().unwrap().remove("last_event_at");
    let legacy: SeatDescriptor = serde_json::from_value(peer_row.clone()).unwrap();
    assert_eq!(legacy.badge, None);
    assert_eq!(legacy.last_event_at, None);

    peer_row["badge"] = serde_json::json!("blocked");
    peer_row["last_event_at"] = serde_json::json!(1_789_171_200_123_u64);
    let current: SeatDescriptor = serde_json::from_value(peer_row).unwrap();
    assert_eq!(current.badge.as_deref(), Some("blocked"));
    assert_eq!(current.last_event_at, Some(1_789_171_200_123));

    let raw = serde_json::to_value(&current).unwrap();
    assert!(raw.get("badge").is_none());
    assert!(raw.get("last_event_at").is_none());
}
