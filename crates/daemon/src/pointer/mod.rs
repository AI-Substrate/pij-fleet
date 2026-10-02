//! Pointer announcements for queued message bodies.
//!
//! A pointer tells a pane-bound seat to pull its durable inbox. It never carries
//! the queued body and therefore never claims that the body was delivered.

use std::time::Duration;

use pij_core::framing::frame_message;
use pij_core::model::SeatId;

mod worker;

pub use worker::DrainWorker;

/// Event emitted after the pointer line itself was submitted to the recipient.
pub const POINTER_ANNOUNCED_EVENT_KIND: &str = "delivery.pointer-announced";
/// Event emitted once when a seat exhausts its pointer announcement budget.
pub const POINTER_PARKED_EVENT_KIND: &str = "delivery.pointer-parked";
/// Event emitted when a successful inbox read resets a parked seat's budget.
pub const POINTER_UNPARKED_EVENT_KIND: &str = "delivery.pointer-unparked";
/// Event proving the narrow control-body pull exception was explained in-pane.
pub const CONTROL_BODY_POINTER_EVENT_KIND: &str = "delivery.control-body-pointer";
/// Event proving an oversized framed pane body was left for inbox pull.
pub const OVERSIZE_BODY_POINTER_EVENT_KIND: &str = "delivery.oversize-body-pointer";

/// Injected pointer cadence and seat-level announcement budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerPolicy {
    /// Minimum time between pointers to the same seat.
    pub cadence: Duration,
    /// Maximum pointers between successful inbox reads.
    pub announcement_limit: u32,
}

/// Render the framed pointer submitted to a pane when a message is waiting.
///
/// The pointer deliberately carries no count: queue cardinality is not exposed
/// at this boundary, and an announce-time count would immediately become stale.
/// It still gets the standard tail even though it carries no body: one uniform
/// frame means readers never need to distinguish pointers from body messages.
#[must_use]
pub fn render_pointer(from: &SeatId, from_machine: Option<&str>) -> String {
    // A remote sender is named by ADDRESS. Two machines may hold seats with the
    // same name, and a frame naming only "bob" leaves the recipient unable to
    // tell WHICH bob paged them — while the event already carries the machine.
    frame_message(from, from_machine, "message waiting — run: pij inbox check")
}

/// Render the only retained pull exception: a body that a stock terminal
/// composer cannot represent without interpreting its control bytes.
#[must_use]
pub(super) fn render_control_body_pointer(from: &SeatId, from_machine: Option<&str>) -> String {
    frame_message(
        from,
        from_machine,
        "message contains terminal control bytes that cannot be typed safely — body remains queued; run: pij inbox",
    )
}

/// Render the explicit refusal for a frame above the measured pane limit.
#[must_use]
pub(super) fn render_oversize_body_pointer(
    from: &SeatId,
    from_machine: Option<&str>,
    frame_bytes: usize,
    maximum: usize,
) -> String {
    frame_message(
        from,
        from_machine,
        &format!(
            "message frame is {frame_bytes} bytes; pane typing limit is {maximum} bytes — body remains queued; run: pij inbox"
        ),
    )
}

/// Render the final pointer that explains why future reminders will stop.
///
/// The announcer records that it POINTED, not that anyone will ever come. The
/// recipient therefore needs the final line to distinguish deliberate parking
/// from a dead daemon; the separate parked event serves operators.
#[must_use]
pub fn render_final_pointer(
    from: &SeatId,
    from_machine: Option<&str>,
    announcements: u32,
) -> String {
    let payload = format!(
        "message waiting — final reminder; parked after {announcements} announcements — message remains queued; run: pij inbox check when ready"
    );
    frame_message(from, from_machine, &payload)
}

/// Render an explicitly tagged remote-control command for PTY submission.
///
/// [`pij_core::model::Msg::command`] stores the bare verb. The slash is a PTY
/// presentation detail added here; body text is never inspected or rendered by
/// this function.
#[must_use]
pub fn render_command(command: &str) -> String {
    format!("/{command}")
}

/// Exponential retry delay for the durable attempt returned by the queue.
///
/// Attempt zero waits one second. Each retry doubles the delay until the
/// five-minute ceiling; the queue persists both the incremented attempt and the
/// not-before time atomically.
#[must_use]
pub fn retry_delay(attempt: u32) -> Duration {
    const MAX_SECONDS: u64 = 5 * 60;
    let seconds = 1_u64
        .checked_shl(attempt)
        .unwrap_or(u64::MAX)
        .min(MAX_SECONDS);
    Duration::from_secs(seconds)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pij_core::model::SeatId;

    use super::{
        POINTER_ANNOUNCED_EVENT_KIND, POINTER_PARKED_EVENT_KIND, POINTER_UNPARKED_EVENT_KIND,
        render_command, render_final_pointer, render_pointer, retry_delay,
    };

    #[test]
    fn pointer_bytes_name_sender_and_pull_command_without_body() {
        let body = "SECRET BODY THAT MUST NEVER BE TYPED";
        let line = render_pointer(&SeatId::from("pij-sender"), None);
        assert_eq!(
            render_pointer(&SeatId::from("bob"), Some("desktop")),
            "[pij-rs from bob@desktop]\nmessage waiting — run: pij inbox check\n[/pij]",
            "a remote sender is named by ADDRESS: two machines may both hold a bob"
        );

        assert_eq!(
            line,
            "[pij-rs from pij-sender]\nmessage waiting — run: pij inbox check\n[/pij]"
        );
        assert!(!line.contains(body), "a pointer must never carry the body");
        assert_eq!(
            line.lines().count(),
            3,
            "a pointer uses the same explicit frame as every pij-rs injection"
        );
        assert_eq!(POINTER_ANNOUNCED_EVENT_KIND, "delivery.pointer-announced");
        assert_eq!(POINTER_PARKED_EVENT_KIND, "delivery.pointer-parked");
        assert_eq!(POINTER_UNPARKED_EVENT_KIND, "delivery.pointer-unparked");
    }

    #[test]
    fn final_pointer_explains_the_silence_to_the_recipient() {
        assert_eq!(
            render_final_pointer(&SeatId::from("pij-sender"), None, 3),
            "[pij-rs from pij-sender]\nmessage waiting — final reminder; parked after 3 announcements — message remains queued; run: pij inbox check when ready\n[/pij]"
        );
        assert_eq!(
            render_final_pointer(&SeatId::from("bob"), Some("desktop"), 3),
            "[pij-rs from bob@desktop]\nmessage waiting — final reminder; parked after 3 announcements — message remains queued; run: pij inbox check when ready\n[/pij]"
        );
    }

    #[test]
    fn every_pointer_uses_the_same_truthful_singular_line() {
        assert_eq!(
            render_pointer(&SeatId::from("pij-one"), None),
            "[pij-rs from pij-one]\nmessage waiting — run: pij inbox check\n[/pij]"
        );
    }

    #[test]
    fn command_rendering_adds_the_pty_slash_to_the_bare_tag() {
        assert_eq!(render_command("compact"), "/compact");
        assert_eq!(render_command("new"), "/new");
    }

    #[test]
    fn durable_attempt_selects_exponential_backoff_capped_at_five_minutes() {
        for (attempt, seconds) in [(0, 1), (1, 2), (8, 256), (9, 300), (u32::MAX, 300)] {
            assert_eq!(retry_delay(attempt), Duration::from_secs(seconds));
        }
    }
}
