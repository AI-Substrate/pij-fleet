//! D2 (auth/lapse): which registration refusals are told as a
//! `registration.refused` spine event. A Copilot seat whose registration is
//! refused retries every few seconds, and nothing recorded why; one event per
//! (seat, code) per [`WINDOW`] says it without a row per retry.
//!
//! In-memory only, and bounded: at most [`PAIRS`] pairs are held, each for
//! [`WINDOW`], and expired pairs are evicted on the next refusal. Telling is a
//! brake on noise, never a decision: removing the window only tells more, and
//! nothing reads the events to decide anything.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// How long a told (seat, code) pair stays quiet.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

/// The most pairs held at once. When every one is still inside its window,
/// a new pair is not told until one expires.
pub const PAIRS: usize = 512;

/// The (seat, code) pairs told within the last [`WINDOW`].
#[derive(Default)]
pub struct RefusalWindow {
    told: Mutex<HashMap<(String, &'static str), Instant>>,
}

impl RefusalWindow {
    /// Whether a refusal of `seat` with `code` at `now` is told: the first of
    /// its pair inside [`WINDOW`], while fewer than [`PAIRS`] pairs are held.
    pub fn admit(&self, seat: &str, code: &'static str, now: Instant) -> bool {
        // A poisoned lock still holds a valid map: no insert is left half done.
        let mut told = self
            .told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        told.retain(|_, at| now.saturating_duration_since(*at) < WINDOW);
        let held = told.len();
        match told.entry((seat.to_string(), code)) {
            Entry::Occupied(_) => false,
            Entry::Vacant(_) if held >= PAIRS => false,
            Entry::Vacant(slot) => {
                slot.insert(now);
                true
            }
        }
    }

    /// How many pairs are held now (for tests of the bound).
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    #[test]
    fn the_window_holds_at_most_its_bound_and_evicts_what_expired() {
        let window = RefusalWindow::default();
        let start = Instant::now();
        for n in 0..PAIRS {
            assert!(window.admit(&format!("pij-{n}"), "refused", start));
        }
        assert!(!window.admit("pij-one-more", "refused", start));
        assert_eq!(window.held(), PAIRS);
        // Once the window has passed, every held pair is evicted.
        assert!(window.admit("pij-one-more", "refused", start + WINDOW));
        assert_eq!(window.held(), 1);
    }
}
