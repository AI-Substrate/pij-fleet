//! D2 (auth/lapse): which registration refusals are told as a
//! `registration.refused` spine event. A Copilot seat whose registration is
//! refused retries every few seconds, and nothing recorded why; one event per
//! (seat, code) per [`WINDOW`] says it without a row per retry.
//!
//! In-memory only, and bounded: at most [`PAIRS`] pairs are held, each for
//! [`WINDOW`], and expired pairs are evicted on the next refusal. A pair it
//! has not seen is always told: when the window is full of live pairs, the
//! one told longest ago is evicted to make room (R277-3), and each such
//! eviction is counted ([`RefusalWindow::evicted`], shown in `ping`). An
//! evicted pair can only be told again sooner. Telling is a brake on noise,
//! never a decision: removing the window only tells more, and nothing reads
//! the events to decide anything.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// How long a told (seat, code) pair stays quiet.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

/// The most pairs held at once. A new pair arriving when every one is still
/// inside its window evicts the one told longest ago.
pub const PAIRS: usize = 512;

/// When each (seat, code) pair was told.
type Told = HashMap<(String, &'static str), Instant>;

/// The (seat, code) pairs told within the last [`WINDOW`], and how many live
/// pairs were evicted to make room.
#[derive(Default)]
pub struct RefusalWindow {
    told: Mutex<(Told, u64)>,
}

impl RefusalWindow {
    /// Whether a refusal of `seat` with `code` at `now` is told: the first of
    /// its pair inside [`WINDOW`]. Never more than [`PAIRS`] pairs are held.
    pub fn admit(&self, seat: &str, code: &'static str, now: Instant) -> bool {
        // A poisoned lock still holds a valid map: no insert is left half done.
        let mut guard = self
            .told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (told, evicted) = &mut *guard;
        told.retain(|_, at| now.saturating_duration_since(*at) < WINDOW);
        let key = (seat.to_string(), code);
        if told.contains_key(&key) {
            return false;
        }
        // Oldest first; among pairs told at the same instant, the least
        // (seat, code), so the victim never depends on hash order (R277).
        if told.len() >= PAIRS
            && let Some(oldest) = told
                .iter()
                .min_by(|(a, at_a), (b, at_b)| at_a.cmp(at_b).then_with(|| a.cmp(b)))
                .map(|(key, _)| key.clone())
        {
            told.remove(&oldest);
            *evicted += 1;
        }
        match told.entry(key) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(now);
                true
            }
        }
    }

    /// How many live pairs were evicted to tell a new one, since the daemon
    /// started.
    pub fn evicted(&self) -> u64 {
        self.told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1
    }

    /// How many pairs are held now (for tests of the bound).
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_is_told_once_per_window_and_again_after_it() {
        let window = RefusalWindow::default();
        let start = Instant::now();
        assert!(window.admit("pij-a", "refused", start));
        assert!(!window.admit("pij-a", "refused", start + Duration::from_secs(5)));
        assert!(!window.admit(
            "pij-a",
            "refused",
            start + WINDOW - Duration::from_millis(1)
        ));
        // Another code, or another seat, is its own pair.
        assert!(window.admit("pij-a", "holder-current", start));
        assert!(window.admit("pij-b", "refused", start));
        assert!(window.admit("pij-a", "refused", start + WINDOW));
    }

    /// R277-3: at capacity, a different seat's first refusal is still told;
    /// the pair told longest ago makes room, and the eviction is counted.
    #[test]
    fn a_full_window_still_tells_an_unseen_pair_and_counts_what_it_evicts() {
        let window = RefusalWindow::default();
        let start = Instant::now();
        for n in 0..PAIRS {
            let at = start + Duration::from_millis(n as u64);
            assert!(window.admit(&format!("pij-{n}"), "refused", at));
        }
        let later = start + Duration::from_secs(1);
        assert!(window.admit("pij-one-more", "refused", later));
        assert_eq!(window.held(), PAIRS);
        assert_eq!(window.evicted(), 1);
        // The oldest pair went, so it may be told again; the rest stay quiet.
        assert!(!window.admit("pij-1", "refused", later));
        assert!(window.admit("pij-0", "refused", later));
        assert_eq!(window.evicted(), 2);
        assert_eq!(window.held(), PAIRS);
    }

    /// R277: pairs told at one instant are evicted in (seat, code) order,
    /// never in hash order.
    #[test]
    fn equal_times_evict_the_least_pair_first() {
        let window = RefusalWindow::default();
        let start = Instant::now();
        // Inserted in reverse, so insertion order cannot pick the victim.
        for n in (0..PAIRS).rev() {
            assert!(window.admit(&format!("pij-{n:03}"), "refused", start));
        }
        assert!(window.admit("pij-zzz", "refused", start));
        assert_eq!(window.evicted(), 1);
        assert!(window.admit("pij-000", "refused", start), "the least went");
        // Telling it again evicted the next least, pij-001; pij-002 stays.
        assert!(!window.admit("pij-002", "refused", start));
        assert!(window.admit("pij-001", "refused", start));
    }

    #[test]
    fn the_window_evicts_what_expired_without_counting_it() {
        let window = RefusalWindow::default();
        let start = Instant::now();
        for n in 0..PAIRS {
            assert!(window.admit(&format!("pij-{n}"), "refused", start));
        }
        assert!(window.admit("pij-one-more", "refused", start + WINDOW));
        assert_eq!(window.held(), 1);
        assert_eq!(window.evicted(), 0);
    }
}
