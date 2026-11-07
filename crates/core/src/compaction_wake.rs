//! Wake a seat that holds work after a compaction leaves it idle (plan 222,
//! F111).
//!
//! A compaction can end a seat's turn with nothing queued, and nothing wakes
//! it: a coder held its role for 17 hours that way. Every compaction pij
//! drives goes through one place, the compact control's admission (a direct
//! `pij send --command compact`, or the compact a memory turn held), and that
//! records a pending row. The PA watchdog's round reads only the pending rows,
//! and once the compact has been delivered for [`COMPACT_SETTLE_MS`], the seat
//! is idle, it has done nothing of its own since ([`STARTED_KINDS`]) and
//! nothing is queued for it, it is sent ONE normal message from `pij-bg`
//! naming what it holds, and its parent an FYI. The row is marked before the
//! message is sent, so a restart or an overlapping round never sends twice; a
//! new compaction is a new row.

use serde::{Deserialize, Serialize};

use crate::model::SeatId;

/// How long after the compact is delivered the seat is left to finish it.
/// Copilot posts no event when a compaction ends, so this is the bound: a
/// compaction is never interrupted, and a seat it left idle waits at most this
/// plus one round.
pub const COMPACT_SETTLE_MS: u64 = 5 * 60 * 1_000;

/// A compact still not delivered this long after it was admitted is given up
/// on: the seat never compacted, so there is nothing to wake it from.
pub const COMPACT_DELIVERY_BOUND_MS: u64 = 60 * 60 * 1_000;

/// The events that show the seat did something after the compact: any own
/// event except `seat.activity` idle (Claude's `/compact` posts one idle when
/// it ends), and any message pushed to it (that message gives it a turn).
pub const STARTED_KINDS: [&str; 4] = [
    "seat.activity",
    "report.now",
    "report.state",
    "message.pushed",
];

/// The audit event kind for a compaction the round settled with a wake.
pub const COMPACTION_WAKE_KIND: &str = "watchdog.compaction-wake";

/// The reason recorded if the resume message wakes a cold seat.
pub const COMPACTION_WAKE_REASON: &str = "pa-watchdog: compaction left a holder idle";

/// How a pending compaction was settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Settled {
    /// The seat was sent the resume message.
    Woken,
    /// The seat did something of its own, or was sent a message, after it.
    Resumed,
    /// The seat holds nothing: nothing to resume.
    HoldsNothing,
    /// The seat is gone.
    Gone,
    /// The compact was never delivered within [`COMPACT_DELIVERY_BOUND_MS`].
    Undelivered,
    /// A newer compact of the same seat replaced it.
    Superseded,
}

impl Settled {
    /// The stored word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Woken => "woken",
            Self::Resumed => "resumed",
            Self::HoldsNothing => "holds-nothing",
            Self::Gone => "gone",
            Self::Undelivered => "undelivered",
            Self::Superseded => "superseded",
        }
    }
}

/// One compaction not yet settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCompaction {
    /// The compacted seat.
    pub seat: SeatId,
    /// The compact control's message id.
    pub msg_id: String,
    /// When the compact was admitted, epoch ms.
    pub admitted_at_ms: u64,
}

/// The resume message's text. `held` names each holding and claim.
#[must_use]
pub fn resume_body(held: &[String]) -> String {
    format!(
        "resume: you hold {}; re-read your handover and continue, or tell your parent if you're blocked.\n\n\
         (pij sent this because a compaction left you idle with nothing queued. It is sent once per compaction.)",
        join(held)
    )
}

/// The parent's FYI.
#[must_use]
pub fn parent_body(seat: &SeatId, held: &[String]) -> String {
    format!(
        "{seat} went idle after a compaction with nothing queued, holding {}. \
         pij sent it one resume message; if it stays idle, check on it.",
        join(held)
    )
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
    fn the_resume_message_names_every_holding_and_asks_for_the_handover() {
        let held = vec![
            "coder/215 (project pij-fleet)".to_string(),
            "plan 222 (project pij-fleet)".to_string(),
        ];
        let body = resume_body(&held);
        assert!(
            body.starts_with(
                "resume: you hold coder/215 (project pij-fleet) and plan 222 (project pij-fleet); \
                 re-read your handover and continue, or tell your parent if you're blocked."
            ),
            "{body}"
        );
        assert!(
            parent_body(&"pij-x".into(), &held[..1]).starts_with(
                "pij-x went idle after a compaction with nothing queued, holding coder/215 (project pij-fleet)."
            )
        );
    }
}
