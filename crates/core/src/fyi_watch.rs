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

/// The most bytes a delivered stall notice may take, with the waiting-FYIs
/// line a PA's turn carries appended after a blank line (F150, F161's 2 KB).
pub const STALL_DELIVERED_MAX: usize = 2_048;

/// How the waiting-FYIs line starts; a body ending with it carries one.
pub const WAITING_LINE_LEAD: &str = "Waiting FYIs: ";

/// The most bytes the waiting-FYIs line takes; past it, `+N more`.
pub const WAITING_LINE_MAX: usize = 300;

/// The most bytes the stall notice itself takes, leaving room for the line.
pub const STALL_NOTICE_MAX: usize = STALL_DELIVERED_MAX - WAITING_LINE_MAX - 2;

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
    let tail = format!(
        "\nRead them in full: pij-rs fyi-read --seat {seat_shown} --pending\n\
         If any needs {seat_shown} to act, wake it with a normal send: pij send {seat_shown} \"<what to do>\". \
         pij never wakes it for this, cold or warm.\n\
         (Sent once for this backlog; again only if new FYIs arrive.)"
    );
    // Whatever the seat ids' lengths, the list gets what the fixed text
    // leaves, keeping room for the `...and N more` line.
    let budget = STALL_NOTICE_MAX.saturating_sub(text.len() + tail.len() + MORE_LINE_MAX);
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
        if used + line.len() > budget {
            break;
        }
        used += line.len();
        shown += 1;
        text.push_str(&line);
    }
    if shown < ordered.len() {
        text.push_str(&format!("\n- ...and {} more", ordered.len() - shown));
    }
    text.push_str(&tail);
    text
}

/// Room for `\n- ...and N more`.
const MORE_LINE_MAX: usize = 32;

/// The id segment of a digest notice, where a one-seat notice names its seat.
pub const FYI_STALL_DIGEST: &str = "digest";

/// One round's stall notice to one target: [`stall_body`] when one seat is
/// newly stalled, else [`stall_digest`]. `stalls` are each seat's pending FYIs.
#[must_use]
pub fn stall_notice(stalls: &[(&SeatId, &[HeldFyi])], now_ms: u64) -> String {
    match stalls {
        [(seat, fyis)] => stall_body(seat, fyis, now_ms),
        _ => stall_digest(stalls, now_ms),
    }
}

/// The digest of several newly stalled seats, one notice per target per round
/// (Jordan, 2026-11-22: one FYI fan-out had woken a PA once per seat). Each
/// seat, oldest backlog first: its count, its oldest's age, and that oldest's
/// sender and first line (fenced), within [`STALL_NOTICE_MAX`], then
/// `+N more seats`.
#[must_use]
pub fn stall_digest(stalls: &[(&SeatId, &[HeldFyi])], now_ms: u64) -> String {
    let mut seats: Vec<(&SeatId, &HeldFyi, usize)> = stalls
        .iter()
        .filter_map(|(seat, fyis)| {
            let oldest = fyis
                .iter()
                .min_by(|a, b| a.held_at_ms.cmp(&b.held_at_ms).then(a.id.cmp(&b.id)))?;
            Some((*seat, oldest, fyis.len()))
        })
        .collect();
    seats.sort_by(|a, b| a.1.held_at_ms.cmp(&b.1.held_at_ms).then(a.0.cmp(b.0)));
    let mut text = format!(
        "[pij watchdog] {} seats have held 2 or more FYIs while idle. An FYI wakes no one, so if \
         one of them was a hand-off, a verdict or a question, its seat is stalled on it:",
        seats.len(),
    );
    let tail = "\nRead a seat's in full: pij-rs fyi-read --seat <seat> --pending\n\
         If any needs its seat to act, wake it with a normal send: pij send <seat> \"<what to do>\". \
         pij never wakes them for this, cold or warm.\n\
         (Sent once per seat's backlog; again only if new FYIs arrive.)";
    let budget = STALL_NOTICE_MAX.saturating_sub(text.len() + tail.len() + MORE_LINE_MAX);
    let mut used = 0;
    let mut shown = 0;
    for (seat, oldest, count) in &seats {
        let line = format!(
            "\n- {}: {count} FYIs, oldest {} ago, from {}: {}",
            seat.rendered(),
            crate::cold_wake::human_duration(now_ms.saturating_sub(oldest.held_at_ms)),
            sender(oldest),
            crate::render::fenced(
                oldest.body.lines().next().unwrap_or_default(),
                STALL_FIRST_LINE_CHARS
            ),
        );
        if used + line.len() > budget {
            break;
        }
        used += line.len();
        shown += 1;
        text.push_str(&line);
    }
    if shown < seats.len() {
        text.push_str(&format!("\n- +{} more seats", seats.len() - shown));
    }
    text.push_str(tail);
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
    let entries: Vec<String> = ordered
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
    // At most WAITING_LINE_MAX bytes, keeping room for `, +N more`.
    let mut line = WAITING_LINE_LEAD.to_string();
    let mut shown = 0;
    for entry in &entries {
        let sep = if shown == 0 { "" } else { ", " };
        if shown > 0 && line.len() + sep.len() + entry.len() + MORE_SUFFIX_MAX > WAITING_LINE_MAX {
            break;
        }
        line.push_str(sep);
        line.push_str(entry);
        shown += 1;
    }
    let more = ordered.len() - shown;
    if more > 0 {
        line.push_str(&format!(", +{more} more"));
    }
    Some(line)
}

/// Room for `, +N more`.
const MORE_SUFFIX_MAX: usize = 16;

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

    /// F150's cap, as delivered: a seat holding a great deal is listed up to a
    /// budget, then counted, and the notice with a PA's waiting-FYIs line
    /// appended stays within 2 KB, whatever the seat ids' lengths (review of
    /// #214, N001). Caller text is fenced.
    #[test]
    fn the_delivered_stall_notice_stays_within_2_kb_and_fences_caller_text() {
        let long = |n: usize| {
            let id = format!("pij-{n}-{}", "a".repeat(crate::model::SEAT_ID_MAX_BYTES));
            id[..crate::model::SEAT_ID_MAX_BYTES].to_string()
        };
        let seat = SeatId::from(long(0).as_str());
        let fyis: Vec<HeldFyi> = (0..60)
            .map(|n| {
                fyi(
                    &format!("m{n}"),
                    &long(1),
                    &"x".repeat(300),
                    NOW - (82 + n) * MIN,
                )
            })
            .collect();
        let body = stall_body(&seat, &fyis, NOW);
        assert!(body.len() <= STALL_NOTICE_MAX, "{} bytes", body.len());
        assert!(body.contains("- ...and "), "{body}");
        let waiting: Vec<(Backlog, SystemState)> = (2..9)
            .map(|n| {
                (
                    Backlog {
                        seat: SeatId::from(long(n).as_str()),
                        count: 999,
                        oldest_ms: NOW - 6_000 * MIN,
                        newest_ms: NOW,
                    },
                    SystemState::Working,
                )
            })
            .collect();
        let line = waiting_line(&waiting, NOW).expect("a line");
        assert!(
            line.len() <= WAITING_LINE_MAX,
            "{} bytes: {line}",
            line.len()
        );
        assert!(line.contains(" more"), "{line}");
        let delivered = format!("{body}\n\n{line}");
        assert!(
            delivered.len() <= STALL_DELIVERED_MAX,
            "{} bytes",
            delivered.len()
        );
        let hostile = stall_body(
            &SeatId::from("pij-held-seat"),
            &[fyi("m", "pij-a-b", "\u{1b}[2Jforged", NOW - 20 * MIN)],
            NOW,
        );
        assert!(!hostile.contains('\u{1b}'), "{hostile}");
    }

    fn borrowed(stalls: &[(SeatId, Vec<HeldFyi>)]) -> Vec<(&SeatId, &[HeldFyi])> {
        stalls
            .iter()
            .map(|(seat, fyis)| (seat, fyis.as_slice()))
            .collect()
    }

    /// Jordan, 2026-11-22: one round's stalls reach a target as one notice.
    /// One seat keeps its detailed notice; several share a digest naming each
    /// (count, oldest age, first line), oldest first, within the 2 KB budget
    /// with `+N more seats` past it.
    #[test]
    fn several_stalled_seats_share_one_digest_within_the_budget() {
        let one = vec![(
            SeatId::from("pij-held-seat"),
            vec![
                fyi("m1", "pij-one-a", "step 3 done", NOW - 22 * MIN),
                fyi("m2", "pij-two-b", "ready", NOW - 5 * MIN),
            ],
        )];
        assert_eq!(
            stall_notice(&borrowed(&one), NOW),
            stall_body(&one[0].0, &one[0].1, NOW)
        );

        let three: Vec<(SeatId, Vec<HeldFyi>)> =
            [("pij-a-a", 20), ("pij-b-b", 40), ("pij-c-c", 30)]
                .into_iter()
                .map(|(seat, age)| {
                    (
                        SeatId::from(seat),
                        vec![
                            fyi("x", "pij-ff-ff", "later\nmore", NOW - MIN),
                            fyi(
                                "y",
                                "pij-ee-ee",
                                &format!("first of {seat}\nmore"),
                                NOW - age * MIN,
                            ),
                        ],
                    )
                })
                .collect();
        let body = stall_notice(&borrowed(&three), NOW);
        assert!(
            body.starts_with("[pij watchdog] 3 seats have held 2 or more FYIs while idle."),
            "{body}"
        );
        let at = |line: &str| body.find(line).unwrap_or_else(|| panic!("{line}: {body}"));
        let b = at("- pij-b-b: 2 FYIs, oldest 40m ago, from pij-ee-ee: \"first of pij-b-b\"");
        let c = at("- pij-c-c: 2 FYIs, oldest 30m ago, from pij-ee-ee: \"first of pij-c-c\"");
        let a = at("- pij-a-a: 2 FYIs, oldest 20m ago, from pij-ee-ee: \"first of pij-a-a\"");
        assert!(b < c && c < a, "oldest first: {body}");
        assert!(
            !body.contains("more seats") && !body.contains("\"later"),
            "{body}"
        );

        let long = |n: usize| {
            let id = format!("pij-{n}-{}", "a".repeat(crate::model::SEAT_ID_MAX_BYTES));
            id[..crate::model::SEAT_ID_MAX_BYTES].to_string()
        };
        let forty: Vec<(SeatId, Vec<HeldFyi>)> = (0..40)
            .map(|n| {
                (
                    SeatId::from(long(n).as_str()),
                    vec![fyi("m", &long(99), &"x".repeat(300), NOW - 20 * MIN); 2],
                )
            })
            .collect();
        let body = stall_notice(&borrowed(&forty), NOW);
        assert!(body.len() <= STALL_NOTICE_MAX, "{} bytes", body.len());
        assert!(body.starts_with("[pij watchdog] 40 seats"), "{body}");
        let shown = body.matches("\n- pij-").count();
        assert!(shown > 0, "{body}");
        assert!(
            body.contains(&format!("\n- +{} more seats", 40 - shown)),
            "{body}"
        );
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
