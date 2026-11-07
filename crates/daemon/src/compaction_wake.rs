//! Wake a seat that holds work after a compaction leaves it idle (plan 222,
//! F111).
//!
//! Rides the PA watchdog's 60 s round: no timer, poll or launch of its own.
//! `admit_control` records a pending row BEFORE it admits a compact, by a
//! write that survives its caller being dropped, and refuses the compact if
//! that write fails, so no compact is admitted untracked. The round reads
//! only the pending compactions (one read of a partial index, usually empty),
//! newest first within a seat: an older one is superseded once a newer one is
//! delivered, and waits while a newer one is in flight. For the newest, in
//! order, cheapest first:
//!
//! 1. the seat is gone: settled `gone`;
//! 2. its compact is not delivered yet: wait, up to
//!    [`COMPACT_DELIVERY_BOUND_MS`], then settled `undelivered`;
//! 3. it has started anything since the delivery (an own event other than
//!    `seat.activity` idle, or a message pushed to it): settled `resumed`;
//! 4. the delivery is under [`COMPACT_SETTLE_MS`] old, the seat isn't idle, or
//!    something is queued for it: wait;
//! 5. it holds nothing (no open role holding, no open claim): settled
//!    `holds-nothing`;
//! 6. otherwise it is settled `woken` FIRST, then sent one normal message from
//!    `pij-bg` naming what it holds, and its live parent an FYI. `woken` means
//!    the one attempt was made, not that it was delivered: there is no retry.
//!
//! A read that fails (delivery, events, queue, holdings, claims) leaves the
//! row pending for the next round; it never settles one.
//!
//! Each pending row costs two indexed reads per round (its delivery outcome,
//! and the seat's events since it); only a settled-age, idle row with nothing
//! queued costs one queue peek and the two holdings reads more.

use pij_core::compaction_wake::{
    COMPACT_DELIVERY_BOUND_MS, COMPACT_SETTLE_MS, COMPACTION_WAKE_KIND, COMPACTION_WAKE_REASON,
    PendingCompaction, Settled, parent_body, resume_body,
};
use pij_core::delivery::{control_kind, delivery_kind};
use pij_core::model::{SeatDescriptor, SeatId, SystemState};
use serde_json::json;

use crate::Services;
use crate::delivery::OUTCOME_EVENT_KIND;
use crate::http::memory_turn::try_holdings_of;
use crate::pa_watchdog::{RoundOutcome, audit, bg_msg, outcome_text, wake_pa_receipt};

/// What the round does with one pending compaction now.
enum Step {
    Wait,
    Settle(Settled),
    Wake(Vec<String>),
}

/// One pending compaction's step, and whether its compact was delivered.
struct Read {
    step: Step,
    delivered: bool,
}

/// Settle this round's pending compactions, waking each idle holder once.
pub(crate) async fn round(
    services: &Services,
    seats: &[SeatDescriptor],
    now_ms: u64,
) -> Vec<RoundOutcome> {
    let pending = match services.watchdogs().pending_compactions().await {
        Ok(pending) => pending,
        Err(cause) => {
            eprintln!("pa watchdog: compaction wake: pending compactions: {cause}");
            return Vec::new();
        }
    };
    let mut outcomes = Vec::new();
    // Newest first within each seat. An older compaction is superseded only
    // once a newer one of the same seat has been delivered, and waits while a
    // newer one is still in flight; a newer compact that never lands (its
    // caller dropped, or it was refused) is given up as undelivered and holds
    // nothing back.
    let mut newer: Option<(SeatId, bool)> = None;
    for compaction in pending.iter().rev() {
        let seat = seats
            .iter()
            .find(|seat| seat.id == compaction.seat && seat.tombstoned_at.is_none());
        let newer_delivered = newer
            .as_ref()
            .filter(|(id, _)| *id == compaction.seat)
            .map(|(_, delivered)| *delivered);
        let step = match (seat, newer_delivered) {
            (None, _) => Step::Settle(Settled::Gone),
            (Some(_), Some(true)) => Step::Settle(Settled::Superseded),
            (Some(_), Some(false)) => Step::Wait,
            (Some(seat), None) => {
                let read = step(services, seat, compaction, now_ms).await;
                if !matches!(read.step, Step::Settle(Settled::Undelivered)) {
                    newer = Some((compaction.seat.clone(), read.delivered));
                }
                read.step
            }
        };
        match step {
            Step::Wait => {}
            Step::Settle(settled) => {
                settle(services, compaction, settled, now_ms).await;
            }
            Step::Wake(held) => {
                let Some(seat) = seat else { continue };
                // Persist before the send (P9): a restart or an overlapping
                // round finds it settled and sends nothing.
                if !settle(services, compaction, Settled::Woken, now_ms).await {
                    continue;
                }
                outcomes.push(wake(services, seats, seat, compaction, &held, now_ms).await);
            }
        }
    }
    outcomes
}

async fn step(
    services: &Services,
    seat: &SeatDescriptor,
    compaction: &PendingCompaction,
    now_ms: u64,
) -> Read {
    let wait = |delivered| Read {
        step: Step::Wait,
        delivered,
    };
    let found = match services
        .spine
        .latest_matching_message(&seat.id, OUTCOME_EVENT_KIND, &compaction.msg_id)
        .await
    {
        Ok(event) => event.filter(|event| {
            serde_json::from_str::<serde_json::Value>(&event.payload)
                .is_ok_and(|payload| payload["outcome"]["outcome"] == "delivered")
        }),
        Err(cause) => {
            eprintln!(
                "pa watchdog: compaction wake: delivery of {}: {cause}",
                seat.id
            );
            // Unknown: treat it as in flight, so an older one waits too.
            return wait(false);
        }
    };
    let Some((delivered_seq, delivered_at)) =
        found.and_then(|event| event.seq.map(|seq| (seq.0, event.at)))
    else {
        let given_up =
            now_ms.saturating_sub(compaction.admitted_at_ms) >= COMPACT_DELIVERY_BOUND_MS;
        return Read {
            step: if given_up {
                Step::Settle(Settled::Undelivered)
            } else {
                Step::Wait
            },
            delivered: false,
        };
    };
    match services
        .watchdogs()
        .started_since(&seat.id, delivered_seq)
        .await
    {
        Ok(true) => {
            return Read {
                step: Step::Settle(Settled::Resumed),
                delivered: true,
            };
        }
        Ok(false) => {}
        Err(cause) => {
            eprintln!(
                "pa watchdog: compaction wake: events of {}: {cause}",
                seat.id
            );
            return wait(true);
        }
    }
    if now_ms.saturating_sub(delivered_at) < COMPACT_SETTLE_MS || seat.state != SystemState::Idle {
        return wait(true);
    }
    match services
        .queue
        .peek(&[delivery_kind(&seat.id), control_kind(&seat.id)])
        .await
    {
        Ok(None) => {}
        Ok(Some(_)) => return wait(true),
        Err(cause) => {
            eprintln!(
                "pa watchdog: compaction wake: queue of {}: {cause}",
                seat.id
            );
            return wait(true);
        }
    }
    // A failed read is never "holds nothing" (review of #167, F001): the row
    // waits for the next round.
    let mut held: Vec<String> = match try_holdings_of(services, &seat.id).await {
        Ok(holdings) => holdings
            .iter()
            .map(crate::http::memory_turn::Holding::label)
            .collect(),
        Err(cause) => {
            eprintln!(
                "pa watchdog: compaction wake: holdings of {}: {cause}",
                seat.id
            );
            return wait(true);
        }
    };
    match services.watchdogs().open_claims(&seat.id).await {
        Ok(claims) => held.extend(claims.iter().map(|claim| {
            format!(
                "{} {} (project {})",
                claim.kind,
                pij_core::claims::format(&claim.kind, claim.number),
                claim.project
            )
        })),
        Err(cause) => {
            eprintln!(
                "pa watchdog: compaction wake: claims of {}: {cause}",
                seat.id
            );
            return wait(true);
        }
    }
    Read {
        step: if held.is_empty() {
            Step::Settle(Settled::HoldsNothing)
        } else {
            Step::Wake(held)
        },
        delivered: true,
    }
}

/// `true` when this call settled it.
async fn settle(
    services: &Services,
    compaction: &PendingCompaction,
    settled: Settled,
    now_ms: u64,
) -> bool {
    match services
        .watchdogs()
        .settle_compaction(&compaction.seat, &compaction.msg_id, settled, now_ms)
        .await
    {
        Ok(done) => done,
        Err(cause) => {
            eprintln!(
                "pa watchdog: compaction wake: settle {} {}: {cause}",
                compaction.seat,
                settled.as_str()
            );
            false
        }
    }
}

async fn wake(
    services: &Services,
    seats: &[SeatDescriptor],
    seat: &SeatDescriptor,
    compaction: &PendingCompaction,
    held: &[String],
    now_ms: u64,
) -> RoundOutcome {
    let msg_id = format!("compaction-wake:{}:{}", seat.id, compaction.msg_id);
    let (outcome, cold_check) = wake_pa_receipt(
        services,
        seat,
        resume_body(held),
        msg_id.clone(),
        COMPACTION_WAKE_REASON,
        now_ms,
    )
    .await;
    let delivery = outcome_text(&outcome);
    let parent = match seat.parent.as_ref().filter(|parent| {
        seats
            .iter()
            .any(|row| &row.id == *parent && row.tombstoned_at.is_none())
    }) {
        Some(parent) => {
            let told = services
                .delivery
                .hold_fyi(bg_msg(
                    parent,
                    parent_body(&seat.id, held),
                    format!("compaction-wake-fyi:{}:{}", seat.id, compaction.msg_id),
                ))
                .await;
            let word = outcome_text(
                &told
                    .map(|receipt| receipt.outcome)
                    .map_err(|e| e.to_string()),
            );
            Some((parent.clone(), word))
        }
        None => None,
    };
    audit(
        services,
        COMPACTION_WAKE_KIND,
        &seat.id,
        now_ms,
        json!({
            "compact": compaction.msg_id,
            "msg_id": msg_id,
            "held": held,
            "delivery": delivery,
            "cold_check": cold_check,
            "parent": parent.as_ref().map(|(parent, delivery)| json!({"seat": parent, "delivery": delivery})),
        }),
    )
    .await;
    RoundOutcome::CompactionWake {
        seat: seat.id.clone(),
        delivery,
        parent,
    }
}

/// Record a compact control about to be admitted, for the round to settle.
/// Committed by a write that completes even if its caller is dropped; the
/// caller admits the compact only after this returns `Ok`.
///
/// # Errors
/// Store failures: the compact must then not be admitted.
pub(crate) async fn record(
    services: &Services,
    seat: &SeatId,
    msg_id: &str,
    at: u64,
) -> pij_core::error::Result<()> {
    services
        .watchdogs()
        .record_compaction(seat, msg_id, at)
        .await
        .map(|_| ())
}

#[cfg(test)]
#[path = "compaction_wake_tests.rs"]
mod tests;
