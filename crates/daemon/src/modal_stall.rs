//! Seats stalled on their harness's own dialog (plan 219, F47).
//!
//! Rides the PA watchdog's 60 s round: no timer, poll or launch of its own.
//! From the seats the round already listed, a seat is read only when pij knows
//! its harness's dialogs ([`modal::watches`]) and it is a candidate
//! ([`is_candidate`]): working, with a pane, and silent [`STALL_AFTER_MS`] by
//! its own events. Each working seat with a pane on such a harness costs one
//! `latest_matching` spine read per round (three indexed per-kind SELECTs in one
//! read snapshot). A candidate's visible pane is read once per round ONLY over
//! the daemon's already-live tmux control client
//! ([`TmuxPort::capture_without_launch`]): never a client start, never a fork.
//! With no live client the candidate is skipped this round and the skip is
//! recorded as a `watchdog.round` event with its reason (pm-001's ruling on
//! P156-F001).
//!
//! On a match the stretch is claimed in the store FIRST, keyed by the sequence
//! of the seat's last own event. Then the seat's parent and the PA in its scope
//! (else the prime) are told, each recorded once it accepts; one that failed is
//! retried on later rounds with the same message id, up to
//! `MAX_TELL_TRIES` failures. pij never presses a key for the seat, and names
//! the dialog only by the marker's fixed label.
//!
//! [`TmuxPort::capture_without_launch`]: pij_core::ports::TmuxPort::capture_without_launch

use pij_core::modal_stall::{
    MODAL_STALL_KIND, MODAL_STALL_REASON, ModalStall, OWN_ACTIVITY_KINDS, is_candidate, stall_body,
};
use pij_core::model::{DeliveryOutcome, SeatDescriptor, SeatId, SystemState};
use pij_harnesses::modal;
use serde_json::json;

use crate::Services;
use crate::pa_watchdog::{PaWatchdog, RoundOutcome, audit, outcome_text, wake_pa_receipt};

/// Why a round skipped a candidate.
pub(crate) const NO_LIVE_CONTROL_CLIENT: &str =
    "no live tmux control client: not read this round, nothing launched";

/// Find and report this round's stalled seats.
pub(crate) async fn round(
    watchdog: &PaWatchdog,
    services: &Services,
    seats: &[SeatDescriptor],
    now_ms: u64,
) -> Vec<RoundOutcome> {
    let mut outcomes = Vec::new();
    // Read once, and only when some seat is a candidate.
    let mut known: Option<Vec<ModalStall>> = None;
    for seat in seats.iter().filter(|seat| {
        modal::watches(seat.harness)
            && seat.state == SystemState::Working
            && seat.pane.is_some()
            && seat.tombstoned_at.is_none()
    }) {
        let last = match services
            .spine
            .latest_matching(&seat.id, &OWN_ACTIVITY_KINDS)
            .await
        {
            Ok(event) => event,
            Err(cause) => {
                eprintln!(
                    "pa watchdog: modal stall: last event of {}: {cause}",
                    seat.id
                );
                continue;
            }
        };
        if !is_candidate(seat, last.as_ref().map(|event| event.at), now_ms) {
            continue;
        }
        let (Some(last), Some(pane)) = (last, seat.pane.as_deref()) else {
            continue;
        };
        let Some(quiet_seq) = last.seq.map(|seq| seq.0) else {
            continue;
        };
        let screen = match services.tmux.capture_without_launch(pane).await {
            Ok(Some(screen)) => screen,
            Ok(None) => {
                audit(
                    services,
                    "watchdog.round",
                    &seat.id,
                    now_ms,
                    json!({
                        "outcome": "modal-stall-skipped",
                        "reason": NO_LIVE_CONTROL_CLIENT,
                        "pane": pane,
                    }),
                )
                .await;
                outcomes.push(RoundOutcome::ModalStallSkipped {
                    seat: seat.id.clone(),
                    reason: NO_LIVE_CONTROL_CLIENT.to_string(),
                });
                continue;
            }
            Err(cause) => {
                eprintln!(
                    "pa watchdog: modal stall: pane {pane} of {}: {cause}",
                    seat.id
                );
                continue;
            }
        };
        let found = modal::modal_dialog(seat.harness, &screen);
        if known.is_none() {
            match services.watchdogs().modal_stalls().await {
                Ok(read) => known = Some(read),
                Err(cause) => {
                    eprintln!("pa watchdog: modal stall: read stalls: {cause}");
                    continue;
                }
            }
        }
        let stretch = known
            .iter()
            .flatten()
            .find(|stall| stall.seat == seat.id && stall.quiet_seq == quiet_seq)
            .cloned();
        let Some(found) = found else {
            // The dialog is gone without an event: stop showing it.
            if stretch.is_some_and(|stall| stall.open) {
                set_open(services, &seat.id, quiet_seq, false).await;
            }
            continue;
        };
        let stall = match stretch {
            Some(stall) => {
                if !stall.open {
                    set_open(services, &seat.id, quiet_seq, true).await;
                }
                stall
            }
            None => {
                let stall = ModalStall {
                    seat: seat.id.clone(),
                    quiet_seq,
                    quiet_since_ms: last.at,
                    claimed_at_ms: now_ms,
                    dialog: found.label.to_string(),
                    key: found.key.to_string(),
                    clears: found.clears.to_string(),
                    open: true,
                    told: Vec::new(),
                    attempts: std::collections::BTreeMap::new(),
                };
                match services.watchdogs().claim_modal_stall(&stall).await {
                    Ok(true) => stall,
                    Ok(false) => continue,
                    Err(cause) => {
                        eprintln!("pa watchdog: modal stall: claim for {}: {cause}", seat.id);
                        continue;
                    }
                }
            }
        };
        let told = tell(watchdog, services, seats, seat, &stall, now_ms).await;
        if told.is_empty() {
            continue;
        }
        audit(
            services,
            MODAL_STALL_KIND,
            &seat.id,
            now_ms,
            json!({
                "harness": seat.harness,
                "pane": pane,
                "quiet_seq": quiet_seq,
                "quiet_since": stall.quiet_since_ms,
                "dialog": stall.dialog,
                "key": stall.key,
                "told": told
                    .iter()
                    .map(|(seat, delivery)| json!({"seat": seat, "delivery": delivery}))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
        outcomes.push(RoundOutcome::ModalStall {
            seat: seat.id.clone(),
            told,
        });
    }
    outcomes
}

async fn set_open(services: &Services, seat: &SeatId, quiet_seq: u64, open: bool) {
    if let Err(cause) = services
        .watchdogs()
        .set_modal_stall_open(seat, quiet_seq, open)
        .await
    {
        eprintln!("pa watchdog: modal stall: mark {seat} open={open}: {cause}");
    }
}

/// Tell the seat's live parent and the PA in its scope (else the prime) that
/// are still owed this stretch's message, never the seat itself. Only an
/// accepted receipt ([`accepted`]) records the recipient as told; a refusal
/// (a native receiver whose lease lapsed refuses, then recovers) or an error
/// is counted, and that recipient is tried again next round with the same
/// message id until it has failed `MAX_TELL_TRIES` times. Returns who was
/// tried this round and delivery's answer.
///
/// Each attempt is counted in the store BEFORE it is sent (performance
/// P156-F002): when that save fails nothing is sent, so a broken counter can
/// never let the attempts pass the bound.
async fn tell(
    watchdog: &PaWatchdog,
    services: &Services,
    seats: &[SeatDescriptor],
    seat: &SeatDescriptor,
    stall: &ModalStall,
    now_ms: u64,
) -> Vec<(SeatId, String)> {
    let live = |id: &SeatId| {
        seats
            .iter()
            .find(|candidate| &candidate.id == id && candidate.tombstoned_at.is_none())
    };
    let mut targets: Vec<&SeatDescriptor> =
        seat.parent.as_ref().and_then(live).into_iter().collect();
    let scope = watchdog.repo(&seat.folder).await;
    match watchdog.wake_target(services, &scope, seats).await {
        Ok(Some(target)) => targets.extend(live(target.seat())),
        Ok(None) => {}
        Err(cause) => eprintln!("pa watchdog: modal stall: PA for {}: {cause}", seat.id),
    }
    let mut tried: Vec<(SeatId, String)> = Vec::new();
    for target in targets {
        if target.id == seat.id
            || !stall.owes(&target.id)
            || tried.iter().any(|(id, _)| id == &target.id)
        {
            continue;
        }
        let msg_id = format!("modal-stall:{}:{}:{}", seat.id, stall.quiet_seq, target.id);
        let body = stall_body(&seat.id, seat.harness, stall, now_ms);
        match services
            .watchdogs()
            .record_modal_stall_attempt(&seat.id, stall.quiet_seq, &target.id)
            .await
        {
            Ok(Some(_)) => {}
            // Another stretch replaced this one: its own round tells.
            Ok(None) => continue,
            Err(cause) => {
                tried.push((
                    target.id.clone(),
                    format!("error: attempt not saved, so not sent: {cause}"),
                ));
                continue;
            }
        }
        let outcome = if failed_for_test(&target.id) {
            Err("injected by the test".to_string())
        } else {
            wake_pa_receipt(services, target, body, msg_id, MODAL_STALL_REASON, now_ms)
                .await
                .0
        };
        let delivery = outcome_text(&outcome);
        if accepted(&outcome)
            && let Err(cause) = services
                .watchdogs()
                .mark_modal_stall_told(&seat.id, stall.quiet_seq, &target.id)
                .await
        {
            eprintln!(
                "pa watchdog: modal stall: record the tell to {} about {}: {cause}",
                target.id, seat.id
            );
        }
        tried.push((target.id.clone(), delivery));
    }
    tried
}

/// Did delivery take the message? Delivered (a replay of an earlier delivery
/// included) or durably queued; or held, which is pending a human's approval
/// and must not be re-sent (`DeliveryOutcome::Held`: retrying re-prompts).
/// A refusal is not acceptance: it is retried, bounded. Classified from the
/// structured receipt, never from its rendered word.
fn accepted(outcome: &std::result::Result<DeliveryOutcome, String>) -> bool {
    matches!(
        outcome,
        Ok(DeliveryOutcome::Delivered { .. }
            | DeliveryOutcome::Queued { .. }
            | DeliveryOutcome::Held { .. })
    )
}

/// Recipients whose tell errors, so tests can show a failed tell is retried.
#[cfg(test)]
pub(crate) static FAIL_FOR_TEST: std::sync::Mutex<Vec<SeatId>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn failed_for_test(target: &SeatId) -> bool {
    FAIL_FOR_TEST
        .lock()
        .is_ok_and(|failed| failed.contains(target))
}

#[cfg(not(test))]
const fn failed_for_test(_: &SeatId) -> bool {
    false
}

#[cfg(test)]
#[path = "modal_stall_tests.rs"]
mod tests;
