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
//! the claim is stored first, keyed by the sequence of the seat's last own
//! event, and each recipient that accepted is recorded, so a restart or an
//! overlapping round never tells one twice, a failed tell is retried, and a
//! new own event (a higher sequence, whatever its clock) starts a new stretch.
//! The dialog is named by a fixed label, never by text read from the pane.
//! pij never presses a key for the seat.

use serde::{Deserialize, Serialize};

use crate::cold_wake::human_duration;
use crate::model::{Harness, SeatDescriptor, SeatId, SystemState};

/// How long a working seat must be silent before its pane is read.
pub const STALL_AFTER_MS: u64 = 20 * 60 * 1_000;

/// The events that are the seat's own doing: its busy and idle posts and its
/// reports. Anything addressed TO the seat (a delivery outcome, a held FYI, a
/// watchdog audit) is not, so a seat that is sent messages while it sits on a
/// dialog still reads silent.
///
/// Nor is `delivery.inbox-ack` (F108): the Copilot native extension's inbox
/// reader acks on its own, beside the model. pij-bumpy-herring's reader
/// reconnected and acked a queued message 2.5 minutes into a dialog that then
/// stood 21 minutes, and the ack restarted the quiet stretch, so the round
/// never read the pane.
pub const OWN_ACTIVITY_KINDS: [&str; 3] = ["seat.activity", "report.now", "report.state"];

/// The reason recorded when a stall message wakes a cold parent or PA.
pub const MODAL_STALL_REASON: &str = "pa-watchdog: modal dialog";

/// The audit event kind for a stall the round told someone about.
pub const MODAL_STALL_KIND: &str = "watchdog.modal-stall";

/// How many tells a recipient gets per stretch before the round stops trying
/// it (at most one per 60 s round), counted before each is sent.
pub const MAX_TELL_TRIES: u32 = 5;

/// One stretch's stall, as stored before anyone is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModalStall {
    /// The stalled seat.
    pub seat: SeatId,
    /// The stretch's identity: the spine sequence of the seat's last own
    /// event. An own event with a higher sequence ends the stretch, whatever
    /// its clock says (freshness follows sequence, not time).
    pub quiet_seq: u64,
    /// That event's time, epoch ms: only for how long it has been silent.
    pub quiet_since_ms: u64,
    /// When the stall was claimed, epoch ms.
    pub claimed_at_ms: u64,
    /// The dialog's fixed label from the marker table, never pane text.
    pub dialog: String,
    /// The key that clears it, short: `ctrl+d`, `esc`.
    pub key: String,
    /// The key that clears the dialog, in words.
    pub clears: String,
    /// Whether the last read still showed the dialog.
    pub open: bool,
    /// Who accepted this stretch's message. Anyone else is told on a later
    /// round.
    pub told: Vec<SeatId>,
    /// Tells per recipient this stretch, each counted before it is sent; one
    /// not yet told at [`MAX_TELL_TRIES`] is not tried again.
    pub attempts: std::collections::BTreeMap<SeatId, u32>,
}

impl ModalStall {
    /// Is `target` still owed this stretch's message, and still worth a try?
    #[must_use]
    pub fn owes(&self, target: &SeatId) -> bool {
        !self.told.contains(target)
            && self.attempts.get(target).copied().unwrap_or(0) < MAX_TELL_TRIES
    }
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
    /// The dialog's fixed label.
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
        "blocked on a dialog: silent for {}, with {} on screen (clears with {})",
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
        "pij: {} ({harness}) has been silent for {}, with {} on screen, while pij reads it working.\n\
         It clears with: {}. Look at the pane first (tmux capture-pane -p -t <pane>), then clear it, and ask the seat in text what it wanted. Never choose an option for it.\n\
         pij will not tell you again until the seat records a new event.",
        seat.rendered(),
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
