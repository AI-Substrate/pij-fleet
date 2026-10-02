//! Deterministic memorable seat identifiers.

#[path = "name_corpus.rs"]
pub mod corpus;

use std::iter::FusedIterator;

use crate::model::SeatId;
use corpus::{NAME_ADJECTIVES, NAME_NOUNS, SHIP_NAMES};

const PAIR_SPACE: usize = NAME_ADJECTIVES.len() * NAME_NOUNS.len();
const SHIP_NAME_EVERY: u32 = 6;

/// Number of distinct identifiers in the pinned memorable-name space.
pub const MEMORABLE_PIJ_ID_SPACE: usize = PAIR_SPACE + SHIP_NAMES.len();

/// Failure returned when a candidate is requested beyond the pinned name space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MemorableIdError {
    /// Every candidate in the pinned corpus has already been offered.
    #[error("memorable id space exhausted after {space} attempts")]
    Exhausted {
        /// Total candidates available before exhaustion.
        space: usize,
    },
}

/// Return one deterministic candidate from the pinned name space.
///
/// `attempt` is zero-based. A seed never repeats a candidate before exhaustion.
pub fn memorable_pij_id_candidate(seed: &str, attempt: usize) -> Result<SeatId, MemorableIdError> {
    if attempt >= MEMORABLE_PIJ_ID_SPACE {
        return Err(MemorableIdError::Exhausted {
            space: MEMORABLE_PIJ_ID_SPACE,
        });
    }

    let ship = ship_slot_for(seed);
    let pair_start = pair_start(seed);
    Ok(id_at(slot_at(ship, pair_start, attempt)))
}

/// Iterate the full deterministic, non-repeating candidate sequence for `seed`.
///
/// # Composition recipe (`/v1/spawn`)
///
/// - Use: `use pij_core::names::memorable_pij_id_candidates;`.
/// - Constructor: N/A; this is a pure function.
/// - Config field: N/A; u-spawn owns the seed and passes it explicitly.
/// - Call site: only the absent-`--id` branch. Keep explicit ids and their
///   collision refusal unchanged. Iterate candidates in order, reserving the
///   first one the registry reports vacant; if iteration ends, return the
///   existing collision/exhaustion refusal. The registry check and reservation
///   remain in u-spawn because allocation is IO, while this module owns only the
///   candidate sequence.
pub fn memorable_pij_id_candidates(seed: &str) -> MemorablePijIdCandidates {
    MemorablePijIdCandidates {
        ship: ship_slot_for(seed),
        pair_start: pair_start(seed),
        attempt: 0,
    }
}

/// Iterator over one seed's complete memorable-id candidate sequence.
#[derive(Clone, Debug)]
pub struct MemorablePijIdCandidates {
    ship: Option<usize>,
    pair_start: usize,
    attempt: usize,
}

impl Iterator for MemorablePijIdCandidates {
    type Item = SeatId;

    fn next(&mut self) -> Option<Self::Item> {
        if self.attempt >= MEMORABLE_PIJ_ID_SPACE {
            return None;
        }

        let slot = slot_at(self.ship, self.pair_start, self.attempt);
        self.attempt += 1;
        Some(id_at(slot))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = MEMORABLE_PIJ_ID_SPACE - self.attempt;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for MemorablePijIdCandidates {}
impl FusedIterator for MemorablePijIdCandidates {}

fn fnv1a_with_suffix(seed: &str, suffix: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for code_unit in seed.encode_utf16().chain(suffix.encode_utf16()) {
        hash ^= u32::from(code_unit);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn mix32(hash: u32) -> u32 {
    let mut mixed = hash;
    mixed ^= mixed >> 16;
    mixed = mixed.wrapping_mul(0x7feb_352d);
    mixed ^= mixed >> 15;
    mixed = mixed.wrapping_mul(0x846c_a68b);
    mixed ^= mixed >> 16;
    mixed
}

fn hash53(seed: &str) -> u64 {
    let high = mix32(fnv1a_with_suffix(seed, "#hi"));
    let low = mix32(fnv1a_with_suffix(seed, "#lo"));
    u64::from(high) * 0x20_0000 + u64::from(low >> 11)
}

fn ship_slot_for(seed: &str) -> Option<usize> {
    if SHIP_NAMES.is_empty()
        || !mix32(fnv1a_with_suffix(seed, "#ship")).is_multiple_of(SHIP_NAME_EVERY)
    {
        return None;
    }

    Some(mix32(fnv1a_with_suffix(seed, "#shipslot")) as usize % SHIP_NAMES.len())
}

fn pair_start(seed: &str) -> usize {
    (hash53(seed) % PAIR_SPACE as u64) as usize
}

const fn gcd(mut left: usize, mut right: usize) -> usize {
    while right > 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

const fn probe_stride() -> usize {
    let candidates = [15_485_863, 104_729, 65_537, 7_919, 257, 31, 7, 3];
    let mut index = 0;
    while index < candidates.len() {
        let stride = candidates[index] % PAIR_SPACE;
        if stride > 1 && gcd(stride, PAIR_SPACE) == 1 {
            return stride;
        }
        index += 1;
    }
    1
}

const PROBE_STRIDE: usize = probe_stride();

fn slot_at(ship: Option<usize>, pair_start: usize, attempt: usize) -> usize {
    if let Some(ship_index) = ship
        && attempt == 0
    {
        return PAIR_SPACE + ship_index;
    }

    let step = if ship.is_some() { attempt - 1 } else { attempt };
    if step < PAIR_SPACE {
        return (pair_start + step * PROBE_STRIDE) % PAIR_SPACE;
    }

    let mut index = step - PAIR_SPACE;
    if ship.is_some_and(|ship_index| index >= ship_index) {
        index += 1;
    }
    PAIR_SPACE + index
}

fn id_at(index: usize) -> SeatId {
    if index >= PAIR_SPACE {
        return SeatId::from(format!("pij-{}", SHIP_NAMES[index - PAIR_SPACE]));
    }

    let adjective = NAME_ADJECTIVES[index / NAME_NOUNS.len()];
    let noun = NAME_NOUNS[index % NAME_NOUNS.len()];
    SeatId::from(format!("pij-{adjective}-{noun}"))
}
