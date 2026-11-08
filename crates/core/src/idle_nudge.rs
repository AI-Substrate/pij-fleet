//! Nudge a seat that sits idle while it holds work (plan 224, F136).
//!
//! Plan 222 wakes a holder a compaction left idle. Any other idle stretch went
//! unnoticed: a merger seat sat for hours holding its role after a plain send,
//! with no role request and no time limit to escalate. The PA watchdog's round
//! now looks at the seats that hold work (an open role holding or an open
//! claim, nothing else), and when one has done nothing of its own for
//! [`IDLE_NUDGE_AFTER_MS`], is idle, owns no live background job and has
//! nothing queued, it is sent ONE normal message from `pij-bg`, and its prime
//! (each project's designated prime, else its parent) an FYI. The mark is
//! persisted before the message is sent and keyed by the seat's last own
//! event, so the next own event re-arms it and a restart never sends twice for
//! one stretch. A seat whose compaction wake is still pending is left to it.

use crate::model::SeatId;

/// How long a seat holding work may go without an own event before it is
/// nudged. One value for every role (plan 224's open question): a seat at
/// work posts own events, so only a genuinely stalled seat crosses it.
pub const IDLE_NUDGE_AFTER_MS: u64 = 60 * 60 * 1_000;

/// The seat's own events, the ones that start or re-arm an idle stretch:
/// `seat.activity` other than idle (a turn starting), and its reports. A
/// message pushed to it is not its own event, so the nudge itself never
/// re-arms the stretch it was sent for.
pub const OWN_KINDS: [&str; 3] = ["seat.activity", "report.now", "report.state"];

/// The audit event kind for a nudge.
pub const IDLE_NUDGE_KIND: &str = "watchdog.idle-nudge";

/// The reason recorded if the nudge wakes a cold seat.
pub const IDLE_NUDGE_REASON: &str = "pa-watchdog: a holder sat idle";

/// The message the idle holder gets. `held` names each holding and claim,
/// already rendered for a daemon-authored message.
#[must_use]
pub fn nudge_body(idle_ms: u64, held: &[String]) -> String {
    format!(
        "idle {} while holding {}; resume, report blocked, or release.\n\n\
         (pij sent this because you have done nothing of your own for that long, with nothing queued. \
         It is sent once per idle stretch: your next report or turn re-arms it.)",
        minutes(idle_ms),
        join(held)
    )
}

/// The prime's FYI. `seat` is rendered here; `held` arrives rendered.
#[must_use]
pub fn prime_body(seat: &SeatId, idle_ms: u64, held: &[String]) -> String {
    format!(
        "{} has been idle {} while holding {}, with nothing queued. \
         pij sent it one nudge (resume, report blocked, or release); if it stays idle, check on it.",
        seat.rendered(),
        minutes(idle_ms),
        join(held)
    )
}

fn minutes(ms: u64) -> String {
    format!("{} min", ms / 60_000)
}

fn join(held: &[String]) -> String {
    match held {
        [] => "nothing".to_string(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nudge_names_the_idle_time_and_every_holding() {
        let held = vec![
            "merger (project crane)".to_string(),
            "plan 224 (project pij-fleet)".to_string(),
        ];
        assert!(nudge_body(IDLE_NUDGE_AFTER_MS + 59_000, &held).starts_with(
            "idle 60 min while holding merger (project crane) and plan 224 (project pij-fleet); \
                 resume, report blocked, or release."
        ));
        assert!(
            prime_body(&"pij-x".into(), 75 * 60_000, &held[..1])
                .starts_with("pij-x has been idle 75 min while holding merger (project crane),")
        );
        assert!(
            prime_body(&"pij-x\u{1b}[2J".into(), 75 * 60_000, &held[..1])
                .starts_with("invalid (9 chars) has been idle"),
            "a seat id outside the grammar is never shown raw"
        );
    }
}
