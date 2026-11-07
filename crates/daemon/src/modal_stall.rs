//! Seats stalled on their harness's own dialog (plan 219, F47).
//!
//! Rides the PA watchdog's 60 s round: no timer, poll or launch of its own.
//! From the seats the round already listed, a seat is read only when pij knows
//! its harness's dialogs ([`modal::watches`]) and it is a candidate
//! ([`is_candidate`]): working, with a pane, and silent [`STALL_AFTER_MS`] by
//! its own events. A healthy seat costs one indexed spine read per round and no
//! pane read. A candidate's visible pane is captured once per round through the
//! tmux adapter, which answers over the daemon's persistent control client, and
//! only the harness's own dialog chrome is matched.
//!
//! On a match the stretch is claimed in the store FIRST, then the seat's parent
//! and the PA in its scope (else the prime) are told, once per stretch. pij
//! never presses a key for the seat.

use pij_core::modal_stall::{
    MODAL_STALL_KIND, MODAL_STALL_REASON, ModalStall, OWN_ACTIVITY_KINDS, is_candidate, stall_body,
};
use pij_core::model::{SeatDescriptor, SeatId, SystemState};
use pij_harnesses::modal;
use serde_json::json;

use crate::Services;
use crate::pa_watchdog::{PaWatchdog, RoundOutcome, audit, wake_pa};

/// Rows of a pane read for its dialog: the whole visible screen.
const CAPTURE_ROWS: u32 = 500;

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
        let quiet_since = match services
            .spine
            .latest_matching(&seat.id, &OWN_ACTIVITY_KINDS)
            .await
        {
            Ok(event) => event.map(|event| event.at),
            Err(cause) => {
                eprintln!(
                    "pa watchdog: modal stall: last event of {}: {cause}",
                    seat.id
                );
                continue;
            }
        };
        if !is_candidate(seat, quiet_since, now_ms) {
            continue;
        }
        let (Some(quiet_since), Some(pane)) = (quiet_since, seat.pane.as_deref()) else {
            continue;
        };
        let screen = match services.tmux.capture(pane, CAPTURE_ROWS).await {
            Ok(screen) => screen,
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
            .find(|stall| stall.seat == seat.id && stall.quiet_since_ms == quiet_since);
        let Some(found) = found else {
            // The dialog is gone without an event: stop showing it.
            if stretch.is_some_and(|stall| stall.open) {
                set_open(services, &seat.id, quiet_since, false).await;
            }
            continue;
        };
        if let Some(stall) = stretch {
            // Told already this stretch; showing it again needs no message.
            if !stall.open {
                set_open(services, &seat.id, quiet_since, true).await;
            }
            continue;
        }
        let stall = ModalStall {
            seat: seat.id.clone(),
            quiet_since_ms: quiet_since,
            claimed_at_ms: now_ms,
            dialog: found.first_line,
            key: found.key.to_string(),
            clears: found.clears.to_string(),
            open: true,
        };
        match services.watchdogs().claim_modal_stall(&stall).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(cause) => {
                eprintln!("pa watchdog: modal stall: claim for {}: {cause}", seat.id);
                continue;
            }
        }
        let told = tell(watchdog, services, seats, seat, &stall, now_ms).await;
        audit(
            services,
            MODAL_STALL_KIND,
            &seat.id,
            now_ms,
            json!({
                "harness": seat.harness,
                "pane": pane,
                "quiet_since": quiet_since,
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

async fn set_open(services: &Services, seat: &SeatId, quiet_since: u64, open: bool) {
    if let Err(cause) = services
        .watchdogs()
        .set_modal_stall_open(seat, quiet_since, open)
        .await
    {
        eprintln!("pa watchdog: modal stall: mark {seat} open={open}: {cause}");
    }
}

/// Tell the seat's live parent and the PA in its scope (else the prime), each
/// once, never the seat itself. Returns who was told and delivery's answer.
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
    let mut told: Vec<(SeatId, String)> = Vec::new();
    for target in targets {
        if target.id == seat.id || told.iter().any(|(id, _)| id == &target.id) {
            continue;
        }
        let msg_id = format!(
            "modal-stall:{}:{}:{}",
            seat.id, stall.quiet_since_ms, target.id
        );
        let body = stall_body(&seat.id, seat.harness, stall, now_ms);
        let (delivery, _) =
            wake_pa(services, target, body, msg_id, MODAL_STALL_REASON, now_ms).await;
        told.push((target.id.clone(), delivery));
    }
    told
}

#[cfg(test)]
#[path = "modal_stall_tests.rs"]
mod tests;
