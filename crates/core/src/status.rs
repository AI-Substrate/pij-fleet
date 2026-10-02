//! Worst-first status badges over mechanical observations and declarations.
//!
//! The complete vocabulary is shared with the TypeScript status surface. A
//! vocabulary slot is not a claim that a production observer emits that state:
//! working and idle are published by OMP, Pi and Copilot turn boundaries through
//! `/v1/activity` (plan 158; Claude through its turn hooks, plan 157); stalled, starting
//! and stopped still have no producer.

use crate::model::{SemanticState, SystemState};

/// Frozen attention-priority order across both axes, worst first.
///
/// Consumers may use these spellings for presentation, but must not invent a
/// different severity order. Ported from `.pi/extensions/pij/core/state.ts`.
pub const BADGE_SEVERITY: [&str; 15] = [
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
];

/// Choose the worst mechanical state or declaration across all open assignments.
///
/// The caller selects open assignments; their order cannot affect the badge.
/// No candidates means `unknown`, not an inferred `idle`.
pub fn badge_of(system: Option<SystemState>, semantic_states: &[SemanticState]) -> &'static str {
    let mut worst = system.map_or(usize::MAX, system_rank);
    for &state in semantic_states {
        worst = worst.min(semantic_rank(state));
    }
    if worst == usize::MAX {
        SystemState::Unknown.as_str()
    } else {
        BADGE_SEVERITY[worst]
    }
}

// Exhaustive mappings force every enum addition to acquire a severity slot.
// The vocabulary tests check both slot-to-enum and enum-to-slot coverage.
const fn system_rank(state: SystemState) -> usize {
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

const fn semantic_rank(state: SemanticState) -> usize {
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
