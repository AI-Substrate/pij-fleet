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

/// The most digits a task number may have in [`is_task_instance`]. PR, plan
/// and F-item numbers are three or four digits and will stay far shorter;
/// the bound keeps the grammar closed, so an arbitrarily long digit string is
/// never taken for a task.
pub const TASK_NUMBER_MAX_DIGITS: usize = 9;

/// Whether a role holding's instance names one task: a PR or plan number,
/// optionally with one letter (`175`, `224`, `183b`), or an F-item (`f144`),
/// each at most [`TASK_NUMBER_MAX_DIGITS`] digits. The empty instance (a
/// long-lived role) and named instances (`questions-context`) are not tasks
/// (F144).
#[must_use]
pub fn is_task_instance(instance: &str) -> bool {
    let number = |digits: &[u8]| {
        (1..=TASK_NUMBER_MAX_DIGITS).contains(&digits.len())
            && digits.iter().all(u8::is_ascii_digit)
    };
    match instance.as_bytes() {
        [b'f', digits @ ..] if !digits.is_empty() => number(digits),
        [digits @ .., last] if last.is_ascii_lowercase() => number(digits),
        digits => number(digits),
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

/// The most bytes of named work a notice lists (F150). With the fixed text
/// around it, every notice stays under 2 KB; past it, `...and N more`.
pub const HELD_TEXT_MAX: usize = 1_500;

/// One line for a seat's unanswered requests to one role address in one
/// project (F150): `an unanswered role:<role> request (project <p>)`, or with
/// several, `N unanswered role:<role> requests (project <p>), oldest <age>`.
/// Names are rendered here.
#[must_use]
pub fn requests_label(
    role: &str,
    instance: &str,
    project: &str,
    count: usize,
    oldest_ms: u64,
) -> String {
    let instance = if instance.is_empty() {
        String::new()
    } else {
        format!("/{}", crate::render::name(instance))
    };
    let role = crate::render::name(role);
    let project = crate::render::name(project);
    if count == 1 {
        format!("an unanswered role:{role}{instance} request (project {project})")
    } else {
        format!(
            "{count} unanswered role:{role}{instance} requests (project {project}), oldest {}",
            age(oldest_ms)
        )
    }
}

fn minutes(ms: u64) -> String {
    format!("{} min", ms / 60_000)
}

fn age(ms: u64) -> String {
    if ms < 2 * 60 * 60_000 {
        minutes(ms)
    } else {
        format!("{} h", ms / (60 * 60_000))
    }
}

fn join(held: &[String]) -> String {
    let mut shown: Vec<&str> = Vec::new();
    let mut used = 0;
    for item in held {
        if !shown.is_empty() && used + item.len() > HELD_TEXT_MAX {
            break;
        }
        used += item.len() + 2;
        shown.push(item);
    }
    let more = held.len() - shown.len();
    match (shown.as_slice(), more) {
        ([], _) => "nothing".to_string(),
        ([one], 0) => (*one).to_string(),
        ([rest @ .., last], 0) => format!("{} and {last}", rest.join(", ")),
        (shown, more) => format!("{}, ...and {more} more", shown.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_task_instance_is_in_flight_work() {
        for task in [
            "175",
            "224",
            "183b",
            "f144",
            "f7",
            "123456789",
            "f123456789",
        ] {
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
            "f1234567890",
        ] {
            assert!(!is_task_instance(resting), "{resting:?}");
        }
    }

    /// F150: a seat holding a great deal is listed up to a budget, then
    /// `...and N more`, so no notice passes 2 KB.
    #[test]
    fn a_long_list_is_capped_with_a_count_of_the_rest() {
        let held: Vec<String> = (0..100)
            .map(|n| format!("an unanswered role:pm-{n:03} request (project pij-fleet)"))
            .collect();
        let seat = SeatId::from("pij-exuberant-wildcat");
        for body in [
            nudge_body(IDLE_NUDGE_AFTER_MS, &held),
            prime_body(&seat, IDLE_NUDGE_AFTER_MS, &held),
            prime_cold_body(&seat, IDLE_NUDGE_AFTER_MS, &held),
        ] {
            assert!(body.len() <= 2_048, "{} bytes", body.len());
            let shown = body.matches("an unanswered role:").count();
            assert!(shown > 1, "{body}");
            assert!(
                body.contains(&format!(", ...and {} more", 100 - shown)),
                "{body}"
            );
        }
        assert_eq!(
            requests_label("pm-001", "", "pij-fleet", 40, 3 * 60 * 60_000),
            "40 unanswered role:pm-001 requests (project pij-fleet), oldest 3 h"
        );
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
