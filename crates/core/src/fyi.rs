//! Held FYIs (plan 158): information that must not open a turn.
//!
//! `pij send --fyi` stores the message instead of delivering it. It rides along
//! appended to the recipient's next real delivery, or is claimed by a typed-turn
//! hook. Opening a turn re-reads the recipient's whole context, and in the
//! 2026-09-28 usage blowout 15% of spend was turns opened by acks and FYIs.
//!
//! [`render_block`] is the ONE definition of the block's text. The daemon renders
//! it, and every client passes it through verbatim. The golden fixtures
//! `crates/testkit/fixtures/golden/fyi/{block,digest}.txt` pin it.
//!
//! Plan 159: a pile of more than [`DIGEST_ABOVE`] renders as a digest (a count
//! per sender, the newest [`DIGEST_NEWEST`] in full, and the command that reads
//! them all), and [`FLUSH_AT`] pending FYIs to a warm seat flush as one message.

use serde::{Deserialize, Serialize};

use crate::model::{Event, SeatId};

/// The `via` a ride-along claim records, prefixed onto the carrying message id.
pub const VIA_MESSAGE_PREFIX: &str = "message:";

/// The typed-turn hooks allowed to claim, as their `via` spelling.
pub const HOOK_VIAS: [&str; 4] = ["hook:claude", "hook:copilot", "hook:omp", "hook:pi"];

/// Pending FYIs that flush as one message to a warm recipient (plan 159).
pub const FLUSH_AT: u64 = 5;

/// A delivered pile larger than this renders as a digest (plan 159).
pub const DIGEST_ABOVE: usize = 5;

/// How many of the newest FYIs a digest shows in full.
pub const DIGEST_NEWEST: usize = 3;

/// The receipt's caution on a held FYI that looks like a question (plan 159).
/// A warning only: the FYI is still held, never converted or refused.
/// Shown after the receipt, which already says `held (fyi)`.
pub const QUESTION_WARNING: &str =
    "this looks like a question; if you need an answer, resend without --fyi";

/// Does an FYI body look like a question? Any `?` counts: a false warning costs
/// a glance, a missed question leaves its sender waiting on a held message.
pub fn looks_like_a_question(body: &str) -> bool {
    body.contains('?')
}

/// How a block opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lead {
    /// Appended after a message or a typed prompt: `Also, N FYIs were queued…`.
    Also,
    /// The whole message of a warm flush: `N FYIs were queued for you:`.
    Flush,
}

/// One held FYI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldFyi {
    /// The sender's message id, which identifies the FYI.
    pub id: String,
    /// Who it is for.
    pub recipient: SeatId,
    /// Who sent it.
    pub sender: SeatId,
    /// The paired machine the sender is on, for an FYI forwarded from another
    /// daemon (plan 164); `None` for a local sender. Part of the FYI's identity:
    /// `(from_machine, id)` is unique, so a forwarded FYI never collides with a
    /// local one that shares its msg_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_machine: Option<String>,
    /// What it says.
    pub body: String,
    /// When it was held, in epoch ms.
    pub held_at_ms: u64,
}

impl HeldFyi {
    /// The sender as every rendering shows it: `seat@machine` when forwarded
    /// from a paired machine, so a remote FYI can never pass for a local seat.
    pub fn sender_label(&self) -> String {
        crate::address::render_destination(&crate::model::Destination {
            seat: self.sender.clone(),
            machine: self.from_machine.clone(),
        })
    }
}

/// Render the block for `fyis`, oldest first, at `utc_offset_minutes` local time.
/// `claimed_at_ms` is when they were claimed, which is how the digest's read
/// command names this batch.
///
/// Empty input renders an empty string, so a caller can't attach an empty header.
pub fn render_block(
    fyis: &[HeldFyi],
    lead: Lead,
    claimed_at_ms: u64,
    utc_offset_minutes: i32,
) -> String {
    let ordered = oldest_first(fyis);
    let Some(first) = ordered.first() else {
        return String::new();
    };
    let count = ordered.len();
    let queued = if count == 1 {
        "1 FYI was queued for you:".to_string()
    } else {
        format!("{count} FYIs were queued for you:")
    };
    let mut block = match lead {
        Lead::Also => format!("Also, {queued}"),
        Lead::Flush => queued,
    };
    if count <= DIGEST_ABOVE {
        push_numbered(&mut block, &ordered, 0, utc_offset_minutes);
        return block;
    }
    block.push(' ');
    block.push_str(&per_sender(&ordered));
    block.push_str(&format!(". The newest {DIGEST_NEWEST}:"));
    let skipped = count - DIGEST_NEWEST;
    push_numbered(&mut block, &ordered[skipped..], skipped, utc_offset_minutes);
    block.push_str(&format!(
        "\nAll {count} were delivered; read them in full with: pij-rs fyi-read --seat {} --claimed-at {claimed_at_ms}",
        first.recipient
    ));
    block
}

/// Every FYI of one delivered batch in full, for `pij-rs fyi-read`.
pub fn render_read(fyis: &[HeldFyi], utc_offset_minutes: i32) -> String {
    let ordered = oldest_first(fyis);
    if ordered.is_empty() {
        return String::new();
    }
    let mut block = if ordered.len() == 1 {
        "1 FYI was delivered to you:".to_string()
    } else {
        format!("{} FYIs were delivered to you:", ordered.len())
    };
    push_numbered(&mut block, &ordered, 0, utc_offset_minutes);
    block
}

fn oldest_first(fyis: &[HeldFyi]) -> Vec<&HeldFyi> {
    let mut ordered: Vec<&HeldFyi> = fyis.iter().collect();
    ordered.sort_by(|a, b| {
        a.held_at_ms
            .cmp(&b.held_at_ms)
            .then(a.id.cmp(&b.id))
            .then(a.from_machine.cmp(&b.from_machine))
    });
    ordered
}

/// `6 from pij-x, 2 from pij-y`: most first, then by name.
fn per_sender(ordered: &[&HeldFyi]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for fyi in ordered {
        let label = fyi.sender_label();
        match counts.iter_mut().find(|(sender, _)| *sender == label) {
            Some((_, count)) => *count += 1,
            None => counts.push((label, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts
        .iter()
        .map(|(sender, count)| format!("{count} from {sender}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Numbered lines, numbering from `skipped + 1`; continuation lines indented.
fn push_numbered(block: &mut String, fyis: &[&HeldFyi], skipped: usize, utc_offset_minutes: i32) {
    for (index, fyi) in fyis.iter().enumerate() {
        let mut lines = fyi.body.lines();
        block.push_str(&format!(
            "\n{}. [from {}, {}] {}",
            skipped + index + 1,
            fyi.sender_label(),
            clock_hh_mm(fyi.held_at_ms, utc_offset_minutes),
            lines.next().unwrap_or_default()
        ));
        for line in lines {
            block.push_str("\n   ");
            block.push_str(line);
        }
    }
}

/// Append a rendered block after a message body.
pub fn append_block(body: &str, block: &str) -> String {
    if block.is_empty() {
        body.to_string()
    } else {
        format!("{body}\n\n{block}")
    }
}

fn clock_hh_mm(epoch_ms: u64, utc_offset_minutes: i32) -> String {
    let minutes = i64::try_from(epoch_ms / 60_000).unwrap_or(i64::MAX);
    let local = (minutes + i64::from(utc_offset_minutes)).rem_euclid(24 * 60);
    format!("{:02}:{:02}", local / 60, local % 60)
}

fn fyi_event(kind: &str, recipient: &SeatId, at: u64, payload: serde_json::Value) -> Event {
    Event {
        seq: None,
        v: 1,
        at,
        kind: kind.to_string(),
        seat: Some(recipient.clone()),
        payload: payload.to_string(),
    }
}

/// The `fyi.held` fact. The body stays in the store, not on the spine.
pub fn held_event(fyi: &HeldFyi) -> Event {
    fyi_event("fyi.held", &fyi.recipient, fyi.held_at_ms, {
        let mut payload = serde_json::json!({
            "id": fyi.id,
            "sender": fyi.sender,
            "held_at_ms": fyi.held_at_ms,
        });
        if let Some(machine) = &fyi.from_machine {
            payload["from_machine"] = machine.as_str().into();
        }
        payload
    })
}

/// The `fyi.delivered` receipt for one atomic claim.
pub fn delivered_event(recipient: &SeatId, ids: &[String], via: &str, at: u64) -> Event {
    fyi_event(
        "fyi.delivered",
        recipient,
        at,
        serde_json::json!({"ids": ids, "via": via}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fyi(id: &str, sender: &str, body: &str, held_at_ms: u64) -> HeldFyi {
        HeldFyi {
            id: id.to_string(),
            recipient: SeatId("pij-r".to_string()),
            sender: SeatId(sender.to_string()),
            from_machine: None,
            body: body.to_string(),
            held_at_ms,
        }
    }

    /// 14:02 and 14:10 UTC on 2026-09-29.
    const T_1402: u64 = 1_790_690_520_000;
    const T_1410: u64 = T_1402 + 8 * 60_000;

    #[test]
    fn the_block_matches_the_golden_fixture_oldest_first() {
        // Out of order on purpose: rendering sorts oldest first.
        let fyis = [
            fyi(
                "m-2",
                "pij-y",
                "reviewer is idle\nsecond line of the note",
                T_1410,
            ),
            fyi("m-1", "pij-x", "build is green on main", T_1402),
        ];
        assert_eq!(
            render_block(&fyis, Lead::Also, 0, 0),
            include_str!("../../testkit/fixtures/golden/fyi/block.txt")
        );
    }

    #[test]
    fn one_fyi_is_singular_and_none_renders_nothing() {
        assert_eq!(
            render_block(&[fyi("m", "pij-x", "hi", T_1402)], Lead::Also, 0, 0),
            "Also, 1 FYI was queued for you:\n1. [from pij-x, 14:02] hi"
        );
        assert_eq!(render_block(&[], Lead::Also, 0, 0), "");
        assert_eq!(append_block("body", ""), "body");
    }

    #[test]
    fn the_clock_uses_the_local_offset_and_wraps_midnight() {
        let block = render_block(&[fyi("m", "pij-x", "hi", T_1402)], Lead::Also, 0, 10 * 60);
        assert!(block.contains("[from pij-x, 00:02]"), "{block}");
        let block = render_block(&[fyi("m", "pij-x", "hi", T_1402)], Lead::Also, 0, -15 * 60);
        assert!(block.contains("[from pij-x, 23:02]"), "{block}");
    }

    /// Plan 159: seven FYIs, four from pij-x and three from pij-y, a minute
    /// apart from 14:02 UTC; the newest is two lines long.
    fn pile(count: u64) -> Vec<HeldFyi> {
        (1..=count)
            .map(|n| {
                let sender = if n % 2 == 1 { "pij-x" } else { "pij-y" };
                let body = if n == 7 {
                    "note 7\nsecond line".to_string()
                } else {
                    format!("note {n}")
                };
                fyi(&format!("m-{n}"), sender, &body, T_1402 + (n - 1) * 60_000)
            })
            .collect()
    }

    #[test]
    fn a_pile_over_five_is_a_digest_matching_the_golden_fixture() {
        assert_eq!(
            render_block(&pile(7), Lead::Also, 1_790_690_940_000, 0),
            include_str!("../../testkit/fixtures/golden/fyi/digest.txt")
        );
    }

    #[test]
    fn five_are_listed_in_full_and_six_are_a_digest() {
        let five = render_block(&pile(5), Lead::Also, 9, 0);
        assert!(
            five.starts_with("Also, 5 FYIs were queued for you:\n1. "),
            "{five}"
        );
        assert!(!five.contains("fyi-read"), "{five}");
        let six = render_block(&pile(6), Lead::Also, 9, 0);
        assert!(
            six.starts_with(
                "Also, 6 FYIs were queued for you: 3 from pij-x, 3 from pij-y. The newest 3:\n4. "
            ),
            "{six}"
        );
        assert!(six.ends_with("--seat pij-r --claimed-at 9"), "{six}");
    }

    #[test]
    fn a_flush_is_its_own_message_and_a_read_is_everything_in_full() {
        let flush = render_block(&pile(5), Lead::Flush, 9, 0);
        assert!(
            flush.starts_with("5 FYIs were queued for you:\n1. [from pij-x, 14:02] note 1"),
            "{flush}"
        );
        let read = render_read(&pile(7), 0);
        assert!(
            read.starts_with("7 FYIs were delivered to you:\n1. [from pij-x, 14:02] note 1\n"),
            "{read}"
        );
        assert!(
            read.ends_with("7. [from pij-x, 14:08] note 7\n   second line"),
            "{read}"
        );
        assert_eq!(render_read(&[], 0), "");
    }

    #[test]
    fn the_block_follows_the_body_after_a_blank_line() {
        assert_eq!(append_block("do X", "Also, …"), "do X\n\nAlso, …");
    }
}
