//! A seat stalled on its harness's own modal dialog (plan 219, F47).
//!
//! A harness question or permission dialog stops a seat's turn while pij still
//! reads it `working`: twice in two days a verdict seat sat on one for about
//! two hours. The PA watchdog's round finds the seats that could be in that
//! state from facts it already holds ([`is_candidate`]): working, with a pane,
//! and silent for [`STALL_AFTER_MS`] by the seat's OWN events
//! ([`OWN_ACTIVITY_KINDS`]). Only those panes are read, and only the harness's
//! own dialog chrome is matched (`pij_harnesses::modal`). On a match the seat's
//! parent and the PA in its scope (else the prime) are told once per stretch:
//! the claim is stored first, keyed by the stretch's start, so a restart or an
//! overlapping round never tells twice, and a new own event starts a new
//! stretch. pij never presses a key for the seat.

use serde::{Deserialize, Serialize};

use crate::cold_wake::human_duration;
use crate::model::{Harness, SeatDescriptor, SeatId, SystemState};

/// How long a working seat must be silent before its pane is read.
pub const STALL_AFTER_MS: u64 = 20 * 60 * 1_000;

/// The events that are the seat's own doing: its busy and idle posts, its
/// reports, and its inbox acknowledgements. Anything addressed TO the seat
/// (a delivery outcome, a held FYI, a watchdog audit) is not, so a seat that
/// is sent messages while it sits on a dialog still reads silent.
pub const OWN_ACTIVITY_KINDS: [&str; 4] = [
    "seat.activity",
    "report.now",
    "report.state",
    "delivery.inbox-ack",
];

/// The reason recorded when a stall message wakes a cold parent or PA.
pub const MODAL_STALL_REASON: &str = "pa-watchdog: modal dialog";

/// The audit event kind for a stall the round told someone about.
pub const MODAL_STALL_KIND: &str = "watchdog.modal-stall";

/// One stretch's stall, as stored before anyone is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModalStall {
    /// The stalled seat.
    pub seat: SeatId,
    /// The stretch: the seat's last own event, epoch ms. A newer own event
    /// ends the stretch.
    pub quiet_since_ms: u64,
    /// When the stall was claimed, epoch ms.
    pub claimed_at_ms: u64,
    /// The dialog's first line, as the pane showed it.
    pub dialog: String,
    /// The key that clears it, short: `ctrl+d`, `esc`.
    pub key: String,
    /// The key that clears the dialog, in words.
    pub clears: String,
    /// Whether the last read still showed the dialog.
    pub open: bool,
}

impl ModalStall {
    /// What `pij list` and the state card show while it stands.
    #[must_use]
    pub fn view(&self) -> DialogView {
        DialogView {
            dialog: self.dialog.clone(),
            key: self.key.clone(),
            clears: self.clears.clone(),
            since_ms: self.quiet_since_ms,
        }
    }
}

/// A standing stall as the roster and the state card carry it (`dialog`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogView {
    /// The dialog's first line.
    pub dialog: String,
    /// The key that clears it, short.
    pub key: String,
    /// The key that clears it, in words.
    pub clears: String,
    /// The seat's last own event, epoch ms: silent since then.
    pub since_ms: u64,
}

/// The STATE cell `pij list` shows for a seat with a standing stall, in place
/// of `working`.
#[must_use]
pub fn blocked_cell(view: &DialogView) -> String {
    format!("blocked on a dialog ({})", view.key)
}

/// The state card's line for a standing stall.
#[must_use]
pub fn blocked_line(view: &DialogView, now_ms: u64) -> String {
    format!(
        "blocked on a dialog for {}: {} (clears with {})",
        human_duration(now_ms.saturating_sub(view.since_ms)),
        view.dialog,
        view.clears
    )
}

/// Is `seat` worth one pane read this round?
///
/// Working, with a pane, alive, and silent [`STALL_AFTER_MS`] by its own
/// events. A seat with no own event at all is not a candidate: its silence has
/// no start. The caller also skips harnesses with no known dialog markers.
#[must_use]
pub fn is_candidate(seat: &SeatDescriptor, last_own_event_ms: Option<u64>, now_ms: u64) -> bool {
    seat.tombstoned_at.is_none()
        && seat.state == SystemState::Working
        && seat.pane.is_some()
        && last_own_event_ms.is_some_and(|at| now_ms.saturating_sub(at) >= STALL_AFTER_MS)
}

/// The message to the parent and the PA.
#[must_use]
pub fn stall_body(seat: &SeatId, harness: Harness, stall: &ModalStall, now_ms: u64) -> String {
    format!(
        "pij: {seat} ({harness}) has shown a dialog for at least {}, with no event from it since then, while pij reads it working.\n\
         Dialog: {}\n\
         It clears with: {}. Look at the pane first (tmux capture-pane -p -t <pane>), then clear it, and ask the seat in text what it wanted. Never choose an option for it.\n\
         pij will not tell you again until the seat records a new event.",
        human_duration(now_ms.saturating_sub(stall.quiet_since_ms)),
        stall.dialog,
        stall.clears,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(state: SystemState, pane: Option<&str>) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new("pij-stalled", Harness::Copilot, "/abs/tree");
        seat.state = state;
        seat.pane = pane.map(str::to_string);
        seat
    }

    /// bp-0001: 19 minutes is not a candidate, 21 is; idle, paneless, dead or
    /// never-active seats never are.
    #[test]
    fn only_a_working_paned_seat_silent_twenty_minutes_is_a_candidate() {
        let now = 10 * STALL_AFTER_MS;
        let minutes = |m: u64| Some(now - m * 60_000);
        let working = seat(SystemState::Working, Some("%17"));
        assert!(!is_candidate(&working, minutes(19), now));
        assert!(is_candidate(&working, minutes(20), now));
        assert!(is_candidate(&working, minutes(21), now));
        assert!(!is_candidate(&working, None, now));
        assert!(!is_candidate(
            &seat(SystemState::Idle, Some("%17")),
            minutes(90),
            now
        ));
        assert!(!is_candidate(
            &seat(SystemState::Working, None),
            minutes(90),
            now
        ));
        let mut gone = working.clone();
        gone.tombstoned_at = Some(1);
        assert!(!is_candidate(&gone, minutes(90), now));
    }
}
