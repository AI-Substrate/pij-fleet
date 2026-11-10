//! Watching held FYIs (F169, F170).
//!
//! An FYI wakes no one, so an actionable message sent as one can sit unread
//! while its recipient idles: a hand-off held as an FYI is a stall nobody sees.
//! This module is the pure text for what the daemon does about it:
//!
//! - the stall notice to the PA (or the prime, with no live PA) when a seat has
//!   held [`STALL_MIN_FYIS`] or more for [`STALL_AFTER_MS`] while idle (F169);
//! - the waiting-FYIs line appended to every turn delivered to a PA (F169);
//! - the re-check of a held daemon state notice when it finally flushes, so a
//!   reader never acts on a size or state that has since changed (F170).

use std::collections::HashMap;

use crate::BG_ACTOR;
use crate::fyi::HeldFyi;
use crate::model::{SeatId, SystemState};
use crate::watchdog::LARGE_SEAT_TOKENS;

/// A seat holding at least this many FYIs may be stalled on one of them.
pub const STALL_MIN_FYIS: u64 = 2;

/// How long the oldest must have waited, with the seat idle, before the PA is
/// told (Jordan, 2026-11-10).
pub const STALL_AFTER_MS: u64 = 15 * 60_000;

/// The most bytes of FYI lines a stall notice lists; past it, `...and N more`.
/// With the fixed text around it, the notice stays under 2 KB (as F150, F161).
pub const STALL_LIST_MAX: usize = 1_300;

/// How much of each FYI's first line the stall notice shows.
pub const STALL_FIRST_LINE_CHARS: usize = 100;

/// How many seats the waiting-FYIs line names, oldest first; the rest are
/// counted as `+N more`.
pub const WAITING_LINE_TOP: usize = 5;

/// The id prefix of the large-seat notice to the prime (plan 217).
pub const LARGE_SEATS_ID_PREFIX: &str = "large-seats:";

/// The id prefix of a stall notice; the seat it is about follows it.
pub const FYI_STALL_ID_PREFIX: &str = "fyi-stall:";

/// The audited reason when a stall notice wakes a cold PA or prime.
pub const FYI_STALL_REASON: &str = "pa-watchdog: seat may be stalled on held FYIs";

/// The id prefix of the idle-nudge notice to the prime (plan 224); the seat
/// it is about follows it.
pub const IDLE_NUDGE_FYI_ID_PREFIX: &str = "idle-nudge-fyi:";

/// One seat's pending FYIs, as the store counts them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlog {
    /// The recipient holding them.
    pub seat: SeatId,
    /// How many are pending.
    pub count: u64,
    /// When the oldest was held, epoch ms.
    pub oldest_ms: u64,
    /// When the newest was held, epoch ms: a newer one means a new backlog.
    pub newest_ms: u64,
}

/// May `backlog` be a stall? Two or more FYIs, the seat idle, and the oldest
/// waiting at least [`STALL_AFTER_MS`].
#[must_use]
pub fn stalled(backlog: &Backlog, state: SystemState, now_ms: u64) -> bool {
    backlog.count >= STALL_MIN_FYIS
        && state == SystemState::Idle
        && now_ms.saturating_sub(backlog.oldest_ms) >= STALL_AFTER_MS
}

/// The stall notice: the seat, each FYI's sender, age and first line (fenced,
/// oldest first, capped), and the two things to do. `fyis` are the seat's
/// pending FYIs.
#[must_use]
pub fn stall_body(seat: &SeatId, fyis: &[HeldFyi], now_ms: u64) -> String {
    let mut ordered: Vec<&HeldFyi> = fyis.iter().collect();
    ordered.sort_by(|a, b| a.held_at_ms.cmp(&b.held_at_ms).then(a.id.cmp(&b.id)));
    let oldest = ordered
        .first()
        .map_or(0, |fyi| now_ms.saturating_sub(fyi.held_at_ms));
    let seat_shown = seat.rendered();
    let mut text = format!(
        "[pij watchdog] {seat_shown} has held {} FYIs for up to {} while idle. An FYI wakes no one, \
         so if one of them was a hand-off, a verdict or a question, {seat_shown} is stalled on it:",
        ordered.len(),
        crate::cold_wake::human_duration(oldest),
    );
    let mut used = 0;
    let mut shown = 0;
    for fyi in &ordered {
        let line = format!(
            "\n- from {}, {} ago: {}",
            sender(fyi),
            crate::cold_wake::human_duration(now_ms.saturating_sub(fyi.held_at_ms)),
            crate::render::fenced(
                fyi.body.lines().next().unwrap_or_default(),
                STALL_FIRST_LINE_CHARS
            ),
        );
        if shown > 0 && used + line.len() > STALL_LIST_MAX {
            break;
        }
        used += line.len();
        shown += 1;
        text.push_str(&line);
    }
    if shown < ordered.len() {
        text.push_str(&format!("\n- ...and {} more", ordered.len() - shown));
    }
    text.push_str(&format!(
        "\nRead them in full: pij-rs fyi-read --seat {seat_shown} --pending\n\
         If any needs {seat_shown} to act, wake it with a normal send: pij send {seat_shown} \"<what to do>\". \
         pij never wakes it for this, cold or warm.\n\
         (Sent once for this backlog; again only if new FYIs arrive.)"
    ));
    text
}

fn sender(fyi: &HeldFyi) -> String {
    match &fyi.from_machine {
        Some(machine) => format!(
            "{}@{}",
            fyi.sender.rendered(),
            crate::render::machine(machine)
        ),
        None => fyi.sender.rendered().into_owned(),
    }
}

/// The line appended to a turn delivered to a PA: every seat with pending
/// FYIs, oldest first, at most [`WAITING_LINE_TOP`], then `+N more`. `None`
/// when no seat has any.
/// `Waiting FYIs: pij-x 3 (oldest 22m, idle), pij-y 1 (5m, working)`.
#[must_use]
pub fn waiting_line(waiting: &[(Backlog, SystemState)], now_ms: u64) -> Option<String> {
    let mut ordered: Vec<&(Backlog, SystemState)> = waiting
        .iter()
        .filter(|(backlog, _)| backlog.count > 0)
        .collect();
    if ordered.is_empty() {
        return None;
    }
    ordered.sort_by(|a, b| {
        a.0.oldest_ms
            .cmp(&b.0.oldest_ms)
            .then(a.0.seat.cmp(&b.0.seat))
    });
    let shown: Vec<String> = ordered
        .iter()
        .take(WAITING_LINE_TOP)
        .map(|(backlog, state)| {
            let age = crate::cold_wake::human_duration(now_ms.saturating_sub(backlog.oldest_ms));
            let age = if backlog.count == 1 {
                age
            } else {
                format!("oldest {age}")
            };
            format!(
                "{} {} ({age}, {})",
                backlog.seat.rendered(),
                backlog.count,
                state.as_str()
            )
        })
        .collect();
    let more = ordered.len().saturating_sub(WAITING_LINE_TOP);
    let more = if more > 0 {
        format!(", +{more} more")
    } else {
        String::new()
    };
    Some(format!("Waiting FYIs: {}{more}", shown.join(", ")))
}

/// What a held state notice is checked against when it flushes (F170).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatNow {
    /// Its observed state.
    pub state: SystemState,
    /// It has been tombstoned.
    pub gone: bool,
    /// Its last known context size, in tokens.
    pub tokens: Option<u64>,
}

/// Does a batch hold a daemon state notice [`refresh_notice`] re-checks? A
/// batch without one needs no fleet read.
#[must_use]
pub fn holds_state_notice(fyis: &[HeldFyi]) -> bool {
    fyis.iter().any(|fyi| {
        fyi.sender.as_str() == BG_ACTOR
            && fyi.from_machine.is_none()
            && (fyi.id.starts_with(LARGE_SEATS_ID_PREFIX)
                || fyi.id.starts_with(IDLE_NUDGE_FYI_ID_PREFIX))
    })
}

/// F170: a daemon state notice that no longer holds when it flushes is marked,
/// never dropped. A large-seat line whose seat is now at or under the line
/// reads `(since resolved: now 67k)`; an idle notice whose seat is working
/// again reads `(since resolved: now working)`; a closed seat reads
/// `(since closed)`. Anything else, and anything not from `pij-bg` on this
/// machine, is returned as it was held. A seat missing from `fleet` is closed.
#[must_use]
pub fn refresh_notice(fyi: &HeldFyi, fleet: &HashMap<SeatId, SeatNow>) -> HeldFyi {
    if fyi.sender.as_str() != BG_ACTOR || fyi.from_machine.is_some() {
        return fyi.clone();
    }
    let mut fresh = fyi.clone();
    if fyi.id.starts_with(LARGE_SEATS_ID_PREFIX) {
        fresh.body = fyi
            .body
            .lines()
            .map(|line| {
                let Some(seat) = line
                    .strip_prefix("- ")
                    .and_then(|rest| rest.split_whitespace().next())
                else {
                    return line.to_string();
                };
                match fleet.get(&SeatId::from(seat)) {
                    None => format!("{line} (since closed)"),
                    Some(now) if now.gone => format!("{line} (since closed)"),
                    Some(SeatNow {
                        tokens: Some(tokens),
                        ..
                    }) if *tokens <= LARGE_SEAT_TOKENS => format!(
                        "{line} (since resolved: now {})",
                        crate::cold_wake::human_tokens(*tokens)
                    ),
                    Some(_) => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    } else if let Some(rest) = fyi.id.strip_prefix(IDLE_NUDGE_FYI_ID_PREFIX) {
        let seat = SeatId::from(rest.split(':').next().unwrap_or_default());
        let mark = match fleet.get(&seat) {
            None => Some("(since closed)"),
            Some(now) if now.gone => Some("(since closed)"),
            Some(now) if now.state == SystemState::Working => Some("(since resolved: now working)"),
            Some(_) => None,
        };
        if let Some(mark) = mark {
            fresh.body = format!("{mark} {}", fyi.body);
        }
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    const MIN: u64 = 60_000;

    fn fyi(id: &str, sender: &str, body: &str, held_at_ms: u64) -> HeldFyi {
        HeldFyi {
            id: id.to_string(),
            recipient: SeatId::from("pij-held-seat"),
            sender: SeatId::from(sender),
            from_machine: None,
            body: body.to_string(),
            held_at_ms,
        }
    }

    fn backlog(seat: &str, count: u64, oldest_ago: u64) -> Backlog {
        Backlog {
            seat: SeatId::from(seat),
            count,
            oldest_ms: NOW - oldest_ago,
            newest_ms: NOW,
        }
    }

    #[test]
    fn a_stall_is_two_fyis_an_idle_seat_and_fifteen_minutes() {
        let due = backlog("pij-a-b", 2, STALL_AFTER_MS);
        assert!(stalled(&due, SystemState::Idle, NOW));
        assert!(!stalled(
            &backlog("pij-a-b", 1, 60 * MIN),
            SystemState::Idle,
            NOW
        ));
        assert!(!stalled(&due, SystemState::Working, NOW));
        assert!(!stalled(
            &backlog("pij-a-b", 3, 14 * MIN),
            SystemState::Idle,
            NOW
        ));
    }

    #[test]
    fn the_stall_notice_names_each_sender_age_and_first_line_and_the_commands() {
        let body = stall_body(
            &SeatId::from("pij-held-seat"),
            &[
                fyi("m2", "pij-two-b", "ready for review\nmore", NOW - 5 * MIN),
                fyi("m1", "pij-one-a", "step 3 done, your turn", NOW - 22 * MIN),
            ],
            NOW,
        );
        assert!(
            body.starts_with(
                "[pij watchdog] pij-held-seat has held 2 FYIs for up to 22m while idle."
            ),
            "{body}"
        );
        let one = body.find("- from pij-one-a, 22m ago: \"step 3 done, your turn\"");
        let two = body.find("- from pij-two-b, 5m ago: \"ready for review\"");
        assert!(
            one.is_some_and(|one| two.is_some_and(|two| one < two)),
            "{body}"
        );
        assert!(!body.contains("more\n"), "only the first line: {body}");
        assert!(body.contains("pij-rs fyi-read --seat pij-held-seat --pending"));
        assert!(body.contains("pij send pij-held-seat \"<what to do>\""));
    }

    /// F150's cap: a seat holding a great deal is listed up to a budget, then
    /// counted, and the notice stays under 2 KB. Caller text is fenced.
    #[test]
    fn the_stall_notice_is_capped_and_fences_caller_text() {
        let fyis: Vec<HeldFyi> = (0..60)
            .map(|n| {
                fyi(
                    &format!("m{n}"),
                    "pij-many-c",
                    &"x".repeat(300),
                    NOW - n * MIN,
                )
            })
            .collect();
        let body = stall_body(&SeatId::from("pij-held-seat"), &fyis, NOW);
        assert!(body.len() <= 2_048, "{} bytes", body.len());
        assert!(body.contains("- ...and "), "{body}");
        let hostile = stall_body(
            &SeatId::from("pij-held-seat"),
            &[fyi("m", "pij-a-b", "\u{1b}[2Jforged", NOW - 20 * MIN)],
            NOW,
        );
        assert!(!hostile.contains('\u{1b}'), "{hostile}");
    }

    #[test]
    fn the_waiting_line_lists_the_oldest_five_then_counts_the_rest() {
        assert_eq!(waiting_line(&[], NOW), None);
        assert_eq!(
            waiting_line(
                &[
                    (backlog("pij-y-y", 1, 5 * MIN), SystemState::Working),
                    (backlog("pij-x-x", 3, 22 * MIN), SystemState::Idle),
                ],
                NOW
            )
            .as_deref(),
            Some("Waiting FYIs: pij-x-x 3 (oldest 22m, idle), pij-y-y 1 (5m, working)")
        );
        let seven: Vec<(Backlog, SystemState)> = (0..7)
            .map(|n| {
                (
                    backlog(&format!("pij-s-{n}"), 1, (10 + n) * MIN),
                    SystemState::Idle,
                )
            })
            .collect();
        let line = waiting_line(&seven, NOW).expect("a line");
        assert!(
            line.starts_with("Waiting FYIs: pij-s-6 1 (16m, idle),"),
            "{line}"
        );
        assert!(line.ends_with("pij-s-2 1 (12m, idle), +2 more"), "{line}");
    }

    fn now(state: SystemState, gone: bool, tokens: Option<u64>) -> SeatNow {
        SeatNow {
            state,
            gone,
            tokens,
        }
    }

    /// F170: two large-seat notices held at 11:13 flushed at 11:49, after both
    /// seats compacted; the prime read stale sizes.
    #[test]
    fn a_held_state_notice_is_marked_when_it_no_longer_holds() {
        let large = fyi(
            "large-seats:pij-prime:1",
            BG_ACTOR,
            "[pij watchdog] over 600k tokens in your fleet (PA: none):\n\
             - pij-big-a 640k role none model unknown\n\
             - pij-big-b 700k role none model unknown\n\
             - pij-big-c 610k role none model unknown",
            NOW,
        );
        let fleet = HashMap::from([
            (
                SeatId::from("pij-big-a"),
                now(SystemState::Idle, false, Some(67_000)),
            ),
            (
                SeatId::from("pij-big-b"),
                now(SystemState::Idle, false, Some(720_000)),
            ),
        ]);
        let body = refresh_notice(&large, &fleet).body;
        assert!(
            body.contains("- pij-big-a 640k role none model unknown (since resolved: now 67k)")
        );
        assert!(
            body.contains("- pij-big-b 700k role none model unknown\n"),
            "{body}"
        );
        assert!(body.ends_with("- pij-big-c 610k role none model unknown (since closed)"));

        let idle = fyi(
            "idle-nudge-fyi:pij-big-b:7:abc:pij-prime",
            BG_ACTOR,
            "pij-big-b has been idle 61 min",
            NOW,
        );
        assert!(
            refresh_notice(&idle, &HashMap::new())
                .body
                .starts_with("(since closed) ")
        );
        let working = HashMap::from([(
            SeatId::from("pij-big-b"),
            now(SystemState::Working, false, None),
        )]);
        assert_eq!(
            refresh_notice(&idle, &working).body,
            "(since resolved: now working) pij-big-b has been idle 61 min"
        );
        assert_eq!(refresh_notice(&idle, &fleet), idle, "still idle: as held");

        // Only pij-bg's own notices are re-checked.
        let forged = fyi("large-seats:x:1", "pij-some-seat", "- pij-big-c 1k", NOW);
        assert_eq!(refresh_notice(&forged, &HashMap::new()), forged);
    }
}
