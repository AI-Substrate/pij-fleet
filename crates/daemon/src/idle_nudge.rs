//! Nudge a seat that sits idle while it holds work (plan 224, F136).
//!
//! Rides the PA watchdog's 60 s round: no timer, poll or launch of its own,
//! and no work for a seat that holds nothing. Each round reads which seats
//! hold work (an open role holding or an open claim: two partial-index reads)
//! and looks up each in the round's registry snapshot. For those that are
//! untombstoned and idle, in order, cheapest first:
//!
//! 1. its compaction wake (plan 222) is still pending: skip (one read of the
//!    pending rows per round, for all seats together);
//! 2. its newest own event (a report, or `seat.activity` other than idle) is
//!    younger than [`IDLE_NUDGE_AFTER_MS`], or it has none: skip (one seek on
//!    the `spine_own_events` partial index, whatever idle activity trails it);
//! 3. it was already nudged for that event's stretch: skip (one key read);
//! 4. it owns a queued or running background job: skip (one owner seek);
//! 5. something is queued for it: skip (one queue peek);
//! 6. it holds nothing any more: skip (the holdings and claims reads);
//! 7. otherwise it is marked nudged for the stretch FIRST, then sent one
//!    normal message from `pij-bg`, and its prime an FYI: the designated prime
//!    of each project it holds work in, else its live parent. The mark is
//!    refused, and nothing sent, if it is already there (a restart or an
//!    overlapping round) or the stretch ended since step 2: an own event
//!    after it, or the seat no longer live and idle (checked in the same
//!    statement as the insert).
//!
//! A read that fails skips the seat for this round; it never sends. Every
//! label in both messages is rendered: seat ids through
//! [`SeatId::rendered`], roles, instances, projects and claim kinds through
//! the holdings' word grammar (`invalid (N chars)` otherwise).

use std::collections::{BTreeSet, HashMap};

use pij_core::BG_ACTOR;
use pij_core::delivery::{control_kind, delivery_kind};
use pij_core::idle_nudge::{
    IDLE_NUDGE_AFTER_MS, IDLE_NUDGE_KIND, IDLE_NUDGE_REASON, nudge_body, prime_body,
};
use pij_core::model::{SeatDescriptor, SeatId, SystemState};
use serde_json::json;

use crate::Services;
use crate::http::memory_turn::{shown, try_holdings_of};
use crate::pa_watchdog::{RoundOutcome, audit, bg_msg, outcome_text, wake_pa_receipt};

/// One idle holder due a nudge: what it holds, rendered, and the projects.
struct Due<'a> {
    seat: &'a SeatDescriptor,
    anchor: u64,
    idle_ms: u64,
    held: Vec<String>,
    projects: BTreeSet<String>,
}

/// Nudge this round's idle holders, each once per idle stretch.
pub(crate) async fn round(
    services: &Services,
    seats: &[SeatDescriptor],
    now_ms: u64,
) -> Vec<RoundOutcome> {
    let store = services.watchdogs();
    let holding: Vec<SeatId> = match store.holding_seats().await {
        Ok(holding) => holding,
        Err(cause) => {
            eprintln!("pa watchdog: idle nudge: holding seats: {cause}");
            return Vec::new();
        }
    };
    if holding.is_empty() {
        return Vec::new();
    }
    // Driven from the holders (P189-F002): one map of the snapshot, then a
    // lookup per holder, never a scan of the holders per seat.
    let by_id: HashMap<&SeatId, &SeatDescriptor> =
        seats.iter().map(|seat| (&seat.id, seat)).collect();
    let candidates: Vec<&SeatDescriptor> = holding
        .iter()
        .filter_map(|id| by_id.get(id).copied())
        .filter(|seat| {
            seat.tombstoned_at.is_none()
                && seat.state == SystemState::Idle
                && seat.id.as_str() != BG_ACTOR
        })
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let compacting: BTreeSet<SeatId> = match store.pending_compactions().await {
        Ok(pending) => pending.into_iter().map(|row| row.seat).collect(),
        Err(cause) => {
            eprintln!("pa watchdog: idle nudge: pending compactions: {cause}");
            return Vec::new();
        }
    };
    let mut outcomes = Vec::new();
    for seat in candidates {
        if compacting.contains(&seat.id) {
            continue;
        }
        let Some(due) = due(services, seat, now_ms).await else {
            continue;
        };
        if let Some(outcome) = mark_then_nudge(services, seats, &due, now_ms).await {
            outcomes.push(outcome);
        }
    }
    outcomes
}

/// Persist before the send (P9): only the call that adds the mark sends. The
/// mark is refused if the stretch ended since `due` read it (an own event, or
/// the seat no longer idle), and then nothing is sent (review of #189).
async fn mark_then_nudge(
    services: &Services,
    seats: &[SeatDescriptor],
    due: &Due<'_>,
    now_ms: u64,
) -> Option<RoundOutcome> {
    let seat = &due.seat.id;
    // Minted before the mark, so a failure marks nothing.
    let occurrence = match occurrence() {
        Ok(occurrence) => occurrence,
        Err(cause) => {
            eprintln!("pa watchdog: idle nudge: id for {seat}: {cause}");
            return None;
        }
    };
    match services
        .watchdogs()
        .mark_idle_nudged(seat, due.anchor, now_ms)
        .await
    {
        Ok(true) => Some(nudge(services, seats, due, &occurrence, now_ms).await),
        Ok(false) => None,
        Err(cause) => {
            eprintln!("pa watchdog: idle nudge: mark {seat}: {cause}");
            None
        }
    }
}

/// Steps 2 to 6 for one idle holder: `Some` when it is due a nudge.
async fn due<'a>(services: &Services, seat: &'a SeatDescriptor, now_ms: u64) -> Option<Due<'a>> {
    let store = services.watchdogs();
    let log = |what: &str, cause: &dyn std::fmt::Display| {
        eprintln!("pa watchdog: idle nudge: {what} of {}: {cause}", seat.id);
    };
    let (anchor, at) = match store.last_own_event(&seat.id).await {
        Ok(Some(last)) => last,
        Ok(None) => return None,
        Err(cause) => {
            log("own events", &cause);
            return None;
        }
    };
    let idle_ms = now_ms.saturating_sub(at);
    if idle_ms < IDLE_NUDGE_AFTER_MS {
        return None;
    }
    match store.idle_nudged(&seat.id, anchor).await {
        Ok(false) => {}
        Ok(true) => return None,
        Err(cause) => {
            log("nudge mark", &cause);
            return None;
        }
    }
    // A failed read is never "owns nothing": the seat is skipped this round.
    match store.owns_live_job(&seat.id).await {
        Ok(false) => {}
        Ok(true) => return None,
        Err(cause) => {
            log("background jobs", &cause);
            return None;
        }
    }
    match services
        .queue
        .peek(&[delivery_kind(&seat.id), control_kind(&seat.id)])
        .await
    {
        Ok(None) => {}
        Ok(Some(_)) => return None,
        Err(cause) => {
            log("queue", &cause);
            return None;
        }
    }
    let holdings = match try_holdings_of(services, &seat.id).await {
        Ok(holdings) => holdings,
        Err(cause) => {
            log("holdings", &cause);
            return None;
        }
    };
    let claims = match store.open_claims(&seat.id).await {
        Ok(claims) => claims,
        Err(cause) => {
            log("claims", &cause);
            return None;
        }
    };
    let mut projects: BTreeSet<String> = holdings.iter().map(|held| held.project.clone()).collect();
    projects.extend(claims.iter().map(|claim| claim.project.clone()));
    let mut held: Vec<String> = holdings
        .iter()
        .map(crate::http::memory_turn::Holding::label)
        .collect();
    held.extend(claims.iter().map(|claim| {
        format!(
            "{} {} (project {})",
            shown(&claim.kind),
            pij_core::claims::format(&claim.kind, claim.number),
            shown(&claim.project)
        )
    }));
    if held.is_empty() {
        return None;
    }
    Some(Due {
        seat,
        anchor,
        idle_ms,
        held,
        projects,
    })
}

/// The seats told: the designated prime of each project the seat holds work
/// in, live and not the seat itself; with none, its live parent.
async fn primes(services: &Services, seats: &[SeatDescriptor], due: &Due<'_>) -> Vec<SeatId> {
    let live = |id: &SeatId| {
        id != &due.seat.id
            && seats
                .iter()
                .any(|row| &row.id == id && row.tombstoned_at.is_none())
    };
    let mut told: Vec<SeatId> = Vec::new();
    for project in &due.projects {
        match services.watchdogs().project(project).await {
            Ok(Some(record)) => {
                if let Some(prime) = record.prime_id.filter(|prime| live(prime))
                    && !told.contains(&prime)
                {
                    told.push(prime);
                }
            }
            Ok(None) => {}
            Err(cause) => {
                eprintln!("pa watchdog: idle nudge: project {project}: {cause}");
            }
        }
    }
    if told.is_empty()
        && let Some(parent) = due.seat.parent.as_ref().filter(|parent| live(parent))
    {
        told.push(parent.clone());
    }
    told
}

/// A per-occurrence id part nobody outside the daemon can predict, so no
/// caller can take the nudge's or an FYI's id first (S189-001). Idempotency
/// comes from the mark, not from the id.
fn occurrence() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn nudge(
    services: &Services,
    seats: &[SeatDescriptor],
    due: &Due<'_>,
    occurrence: &str,
    now_ms: u64,
) -> RoundOutcome {
    let seat = due.seat;
    let msg_id = format!("idle-nudge:{}:{}:{occurrence}", seat.id, due.anchor);
    let (outcome, cold_check) = wake_pa_receipt(
        services,
        seat,
        nudge_body(due.idle_ms, &due.held),
        msg_id.clone(),
        IDLE_NUDGE_REASON,
        now_ms,
    )
    .await;
    let delivery = outcome_text(&outcome);
    let mut told = Vec::new();
    for prime in primes(services, seats, due).await {
        let fyi = services
            .delivery
            .hold_fyi(bg_msg(
                &prime,
                prime_body(&seat.id, due.idle_ms, &due.held),
                format!(
                    "idle-nudge-fyi:{}:{}:{occurrence}:{prime}",
                    seat.id, due.anchor
                ),
            ))
            .await;
        let word = outcome_text(
            &fyi.map(|receipt| receipt.outcome)
                .map_err(|cause| cause.to_string()),
        );
        told.push((prime, word));
    }
    audit(
        services,
        IDLE_NUDGE_KIND,
        &seat.id,
        now_ms,
        json!({
            "anchor": due.anchor,
            "idle_ms": due.idle_ms,
            "msg_id": msg_id,
            "held": due.held,
            "delivery": delivery,
            "cold_check": cold_check,
            "primes": told.iter().map(|(prime, delivery)| json!({"seat": prime, "delivery": delivery})).collect::<Vec<_>>(),
        }),
    )
    .await;
    RoundOutcome::IdleNudge {
        seat: seat.id.clone(),
        delivery,
        primes: told,
    }
}

#[cfg(test)]
#[path = "idle_nudge_tests.rs"]
mod tests;
