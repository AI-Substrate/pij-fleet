//! Nudge a seat that sits idle while it holds work (plan 224, F136).
//!
//! Plan 222 wakes a holder a compaction left idle. Any other idle stretch went
//! unnoticed: a merger seat sat for hours after a plain send, with no time
//! limit to escalate. The PA watchdog's round looks at seats with **real
//! in-flight work** ([`is_task_instance`], [`IN_FLIGHT_CLAIM_KINDS`], or a
//! delivered role request not yet answered), and when one has done nothing of
//! its own for [`IDLE_NUDGE_AFTER_MS`], is idle, owns no live background job
//! and has nothing queued, it is sent ONE normal message from `pij-bg`, and its
//! prime (each project's designated prime, else its parent) an FYI. A bare
//! long-lived role holding (docs-system, delivery, security, merger, pa...) is
//! a resting specialist, never work by itself (F144, ruling C30). A cold seat
//! is never woken for it: only its prime is told. The mark is persisted before
//! anything is sent and keyed by the seat's last own event, so the next own
//! event re-arms it and a restart never sends twice for one stretch. A seat
//! whose compaction wake is still pending is left to it.

use crate::model::SeatId;

/// How long a seat holding work may go without an own event before it is
/// nudged. One value for every role (plan 224's open question): a seat at
/// work posts own events, so only a genuinely stalled seat crosses it.
pub const IDLE_NUDGE_AFTER_MS: u64 = 60 * 60 * 1_000;

/// The audit event kind for a nudge.
pub const IDLE_NUDGE_KIND: &str = "watchdog.idle-nudge";

/// Claim kinds that are in-flight work: a plan being built or a fix being
/// made. A workstream, PRD or migration number is held for longer than any
/// one task.
pub const IN_FLIGHT_CLAIM_KINDS: [&str; 2] = ["plan", "fix"];

/// Whether a role holding's instance names one task: a PR or plan number,
/// optionally with one letter (`175`, `224`, `183b`), or an F-item (`f144`).
/// The empty instance (a long-lived role) and named instances
/// (`questions-context`) are not tasks (F144).
#[must_use]
pub fn is_task_instance(instance: &str) -> bool {
    let bytes = instance.as_bytes();
    match bytes {
        [b'f', digits @ ..] if !digits.is_empty() => digits.iter().all(u8::is_ascii_digit),
        [digits @ .., last] => {
            let digits = if last.is_ascii_lowercase() {
                digits
            } else {
                bytes
            };
            !digits.is_empty() && digits.len() <= 9 && digits.iter().all(u8::is_ascii_digit)
        }
        [] => false,
    }
}

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

/// The prime's FYI when the seat was not woken: it is cold, or whether it is
/// cold is unknown, and a wake re-reads its whole context (F144).
#[must_use]
pub fn prime_cold_body(seat: &SeatId, idle_ms: u64, held: &[String]) -> String {
    format!(
        "{} has been idle {} while holding {}, with nothing queued. \
         pij did not wake it: it is cold, or its context size is unknown, and a wake re-reads its whole context. \
         Check on it if the work is still needed.",
        crate::render::seat(seat.as_str()),
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
        crate::render::seat(seat.as_str()),
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
    fn only_a_task_instance_is_in_flight_work() {
        for task in ["175", "224", "183b", "f144", "f7"] {
            assert!(is_task_instance(task), "{task}");
        }
        for resting in [
            "",
            "f",
            "b",
            "questions-context",
            "crane",
            "183bc",
            "1-2",
            "F144",
            "1234567890",
        ] {
            assert!(!is_task_instance(resting), "{resting:?}");
        }
    }

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
