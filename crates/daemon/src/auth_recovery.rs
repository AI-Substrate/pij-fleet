//! Restart a Copilot seat whose CLI lost its login (F212, F261).
//!
//! A running Copilot CLI that starts answering `401 authorization` (or the
//! credential-authority refusal, which has no code) keeps failing until it is
//! restarted: a fresh process re-links its enterprise login with no one
//! signing in, the old one never does (46 such errors over three days, the same
//! process failing for up to six hours). Two triggers reach this decision:
//!
//! - the pij Copilot extension reports the lost login once the failed run has
//!   ended (`POST /v1/seat/auth-error`), naming the process it runs in (F212);
//! - the daemon's own `native.receiver.lapsed` edge for a Copilot seat
//!   ([`follow_lapses`], F261). Eight hours after a Copilot process starts, its
//!   enterprise sign-in lapses; Copilot then flips its managed-extension policy
//!   from Allowed to Pending and force-kills the pij extension. The seat is
//!   idle, so no run fails and no `session.error` is ever emitted: the
//!   extension that would report is the thing that died, and the lease lapse
//!   is the only signal (pij-steady-otter, 2026-11-21 03:36:58Z). A fresh
//!   process brings the extension back, so the remedy is the same `/restart`.
//!   The process is checked in-process first: a gone process means an exit or
//!   a crash, never a restart.
//!
//! POLICY, and what decides it: one `/restart` per seat per
//! [`RECOVERY_WINDOW`], typed through the same pane control path as a compact,
//! so the control path's own brakes still hold it (a settled idle, an empty
//! composer, the hold limit). Copilot's `/restart` swaps the CLI process and
//! restores the same live session: same seat id, native session, holdings and
//! held FYIs (proved on a throwaway seat, F212). F261 adds a second input to
//! that policy (a lease lapse now triggers it), not a second policy.
//!
//! BRAKES, each of which can only turn a restart into a notice or nothing:
//! - the seat is not idle, or its composer is not empty or not readable (a
//!   draft, a human typing, a layout pij cannot read): no restart; its live
//!   parent and its projects' designated primes are told how to restart it;
//! - a restart already happened inside the window: never a second one. While
//!   its control is still queued, nothing more; once it has settled with the
//!   same process still running (Copilot refused `/restart`, or it was never
//!   typed), they are told the restart did not take; if the process it started
//!   lost its login too, they are told the login needs renewing;
//! - they are told at most once per window;
//! - F261: the seat's recorded process (pid and start) is not running, or its
//!   start could not be read: never a restart; they are told to revive it;
//! - F261: a lapse is decided only while it is still the seat's latest
//!   receiver edge and nothing has been decided since: a lapse its receiver
//!   has since renewed, or one already decided (live, by an earlier
//!   reconcile, or by an earlier daemon), is spared. A read that fails spares
//!   it too.
//!
//! The lapse follower attaches live, so it also reconciles: at attachment, and
//! again after any gap a lag leaves in its subscription, it decides every
//! lapse outstanding on the spine (each live native Copilot seat whose latest
//! receiver edge is `native.receiver.lapsed`). A restart restores a lapse
//! without telling it again (R277-2), and a lapse overwritten in the
//! broadcast ring is never delivered, so without this a seat lost either way
//! would stay deaf (B311-01). The reconcile is the same decision, with every
//! brake above; it never replays a lapse its receiver has since renewed.
//!
//! The window and its budget are SHARED by both triggers: one remedy, one
//! seat. A loss can show both ways (the extension dies, then a turn fails
//! after a human revives it), and two budgets would restart one loss twice.
//!
//! Removing any brake types the same `/restart` sooner or more often; none of
//! them chooses a different action.
//!
//! The window is measured on the daemon's monotonic clock, never the wall
//! clock, so a clock correction cannot end it early. Decisions for one seat run
//! one at a time under that seat's lock, so two reports in flight together
//! still make one decision each, in turn. Each decision that acts is also a
//! `seat.auth_recovery` spine event; after a daemon restart, when this
//! process has no memory of the seat, a recent event is the brake instead
//! (the only use of its wall-clock stamp, and it can only spare a seat).
//!
//! Cost: the route runs only on an auth error, and the lapse follower only on
//! a `native.receiver.lapsed` edge (told once per lapse, never per tick): one
//! indexed read of the seat's latest receiver or recovery event, one roster
//! read and one start-time read of the seat's process (`proc_pidinfo` on
//! macOS, procfs on Linux; no launch for the daemon's own user; otherwise the
//! existing absolute `/bin/ps` fallback, counted as
//! `ps.proc-liveness-fallback`). Its live subscription receives every
//! broadcast event before its filter drops the rest: one subscriber per
//! daemon, never per seat. A reconcile, at attachment and after a gap only,
//! is one roster read and one indexed spine read per live native Copilot seat
//! (the R277-2 restore's shape); no timer. Either then reads the seat's
//! latest recovery event once per daemon lifetime, the live payloads of the
//! seat's control kind while a restart may be pending, its holdings when
//! someone is told, and one composer capture through the existing
//! interaction gate when a restart is possible. The restart is one queued
//! control; a notice is one message per recipient.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pij_core::events::EventFilter;
use pij_core::model::{
    DeliveryOutcome, Event, Harness, Msg, SeatDescriptor, SeatId, Seq, SystemState,
};
use pij_core::ports::SeatFilter;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::StreamExt;

use crate::Services;
use crate::pa_watchdog::{bg_msg, outcome_text};

/// The spine event that records each recovery decision that acted.
pub(crate) const AUTH_RECOVERY_KIND: &str = "seat.auth_recovery";
/// One restart, and one notice, per seat inside this window.
pub(crate) const RECOVERY_WINDOW: Duration = Duration::from_secs(30 * 60);

/// Which lost login the extension saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LostLogin {
    /// `session.error` with `errorType: authorization` and status 401.
    Authorization,
    /// The credential authority's `Sign in … again` refusal (no code).
    CredentialAuthority,
}

/// What started a decision: a loss the extension reported, or the daemon's
/// own lease lapse (F261). Only the first comes over HTTP; [`LostLogin`] is
/// the wire type, so no caller can claim the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The seat's own extension reported it (F212).
    Reported(LostLogin),
    /// The seat's native receiver lapsed while it was a Copilot seat (F261).
    ExtensionLost,
}

impl Cause {
    /// The `kind` a recovery event records.
    fn wire(self) -> &'static str {
        match self {
            Self::Reported(LostLogin::Authorization) => "authorization",
            Self::Reported(LostLogin::CredentialAuthority) => "credential-authority",
            Self::ExtensionLost => "extension-lost",
        }
    }
}

/// The process a report speaks for: the Copilot CLI's pid and start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incarnation {
    /// Process id.
    pub pid: u32,
    /// The process's observed start, as the registry records it.
    pub proc_start: u64,
}

/// What pij did about one report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// `/restart` was queued on the seat's control path.
    Restart,
    /// The seat was busy or held a draft, or the restart could not be queued:
    /// its parent and primes were told how to restart it.
    Notify,
    /// The restart settled and the same process still runs: they were told it
    /// did not take.
    NotRestarted,
    /// The process pij restarted lost its login too: they were told the
    /// login needs renewing.
    Relogin,
    /// A restart is queued and has not run yet.
    Pending,
    /// They were already told inside the window.
    Told,
    /// F261: the seat's recorded process is gone (or its start unreadable),
    /// so it exited or crashed: they were told to revive it.
    Exited,
}

/// The last decision that acted for one seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) action: Action,
    pub(crate) since: Instant,
    pub(crate) incarnation: Incarnation,
    pub(crate) msg_id: Option<String>,
}

/// Each seat's last acting decision, behind that seat's own lock.
#[derive(Default)]
pub struct Ledger {
    seats: Mutex<HashMap<SeatId, Arc<tokio::sync::Mutex<Option<Entry>>>>>,
}

impl Ledger {
    /// An empty ledger: nothing decided yet in this daemon.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, seat: &SeatId) -> Arc<tokio::sync::Mutex<Option<Entry>>> {
        Arc::clone(
            self.seats
                .lock()
                .expect("auth recovery ledger mutex")
                .entry(seat.clone())
                .or_default(),
        )
    }
}

/// The decision, from the last acting one and the report now. `alive`: the
/// reporting process still runs (always, for an extension's own report).
/// `settled` (the queued restart is no longer live) and `free` (idle,
/// composer empty) are read only when the answer depends on them.
pub(crate) fn decide(
    last: Option<&Entry>,
    now: Instant,
    reporter: Incarnation,
    alive: bool,
    settled: impl FnOnce() -> bool,
    free: impl FnOnce() -> bool,
) -> Action {
    match last.filter(|last| now.saturating_duration_since(last.since) < RECOVERY_WINDOW) {
        Some(Entry {
            action: Action::Restart,
            incarnation,
            ..
        }) => {
            if *incarnation != reporter {
                Action::Relogin
            } else if !settled() {
                Action::Pending
            } else if alive {
                Action::NotRestarted
            } else {
                Action::Exited
            }
        }
        Some(_) => Action::Told,
        None if !alive => Action::Exited,
        None if free() => Action::Restart,
        None => Action::Notify,
    }
}

/// What one report did, for the route's answer and the audit.
#[derive(Debug, Serialize)]
pub(crate) struct Outcome {
    pub(crate) action: Action,
    /// The restart control's msg_id, when one was queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) msg_id: Option<String>,
    /// Who was told, and each delivery's outcome word.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) told: Vec<(SeatId, String)>,
}

/// Act on one lost login for `seat`, in `reporter`: the extension's own report
/// (the route has checked `reporter` is the seat's current process), or the
/// daemon's lease lapse ([`follow_lapses`]), whose process is checked here.
///
/// # Errors
/// Store failures reading or recording the decision: nothing was sent.
pub(crate) async fn handle(
    services: &Services,
    seat: &SeatDescriptor,
    reporter: Incarnation,
    cause: Cause,
    now_ms: u64,
) -> pij_core::error::Result<Outcome> {
    let slot = services.auth_recovery.slot(&seat.id);
    let mut last = slot.lock().await;
    let now = Instant::now();
    if last.is_none() {
        *last = remembered(services, &seat.id, now, now_ms).await?;
    }
    // An extension reporting runs in `reporter`, so it is alive. A lapse is
    // the extension's death, so the process is read: a BRAKE, since a read
    // that fails is not alive and can only spare the seat.
    let alive = match cause {
        Cause::Reported(_) => true,
        Cause::ExtensionLost => running(services, reporter).await,
    };
    let in_window = last
        .as_ref()
        .filter(|entry| now.saturating_duration_since(entry.since) < RECOVERY_WINDOW);
    let pending_restart = in_window
        .filter(|entry| entry.action == Action::Restart && entry.incarnation == reporter)
        .and_then(|entry| entry.msg_id.clone());
    let settled = match pending_restart {
        Some(msg_id) => !control_live(services, &seat.id, &msg_id).await,
        None => false,
    };
    let free = if in_window.is_none() && alive {
        composer_free(services, seat).await
    } else {
        false
    };
    let mut action = decide(last.as_ref(), now, reporter, alive, || settled, || free);
    let mut msg_id = None;
    if action == Action::Restart {
        let id = format!("auth-restart:{}:{now_ms}", seat.id);
        // Persist before mutate: the window starts before the control exists.
        record(
            services,
            seat,
            reporter,
            Action::Restart,
            cause,
            now_ms,
            Some(&id),
        )
        .await?;
        *last = Some(Entry {
            action: Action::Restart,
            since: now,
            incarnation: reporter,
            msg_id: Some(id.clone()),
        });
        let control = Msg {
            command: Some("restart".to_string()),
            ..bg_msg(&seat.id, String::new(), id.clone())
        };
        match services.delivery.accept_control(control).await {
            Ok(receipt) if !matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) => {
                msg_id = Some(id);
            }
            Ok(_) => action = Action::Notify,
            Err(cause) => {
                eprintln!("auth recovery: restart of {} not queued: {cause}", seat.id);
                action = Action::Notify;
            }
        }
    }
    let told = match action {
        Action::Notify | Action::NotRestarted | Action::Relogin | Action::Exited => {
            record(services, seat, reporter, action, cause, now_ms, None).await?;
            *last = Some(Entry {
                action,
                since: now,
                incarnation: reporter,
                msg_id: None,
            });
            tell(services, seat, action, cause, now_ms).await
        }
        Action::Restart | Action::Pending | Action::Told => Vec::new(),
    };
    Ok(Outcome {
        action,
        msg_id,
        told,
    })
}

/// The seat's last acting decision from the spine, for a daemon that has none
/// in memory (it restarted). Its wall-clock age places it on this daemon's
/// monotonic clock; that only ever spares the seat.
async fn remembered(
    services: &Services,
    seat: &SeatId,
    now: Instant,
    now_ms: u64,
) -> pij_core::error::Result<Option<Entry>> {
    #[derive(Deserialize)]
    struct Recorded {
        action: Action,
        at: u64,
        pid: Option<u32>,
        proc_start: Option<u64>,
        msg_id: Option<String>,
    }
    let Some(event) = services
        .spine
        .latest_matching(seat, &[AUTH_RECOVERY_KIND])
        .await?
    else {
        return Ok(None);
    };
    let Ok(recorded) = serde_json::from_str::<Recorded>(&event.payload) else {
        return Ok(None);
    };
    let (Some(pid), Some(proc_start)) = (recorded.pid, recorded.proc_start) else {
        return Ok(None);
    };
    let age = Duration::from_millis(now_ms.saturating_sub(recorded.at));
    let Some(since) = now.checked_sub(age.min(RECOVERY_WINDOW)) else {
        return Ok(None);
    };
    Ok(Some(Entry {
        action: recorded.action,
        since,
        incarnation: Incarnation { pid, proc_start },
        msg_id: recorded.msg_id,
    }))
}

/// Whether the restart control `msg_id` is still queued for `seat`. A read
/// that fails counts as queued: it can only hold a notice back.
async fn control_live(services: &Services, seat: &SeatId, msg_id: &str) -> bool {
    let kind = format!("delivery-control:{seat}");
    match services.queue.live_payloads(&[kind]).await {
        Ok(payloads) => payloads.iter().any(|payload| {
            serde_json::from_str::<serde_json::Value>(payload)
                .ok()
                .is_some_and(|value| value["msg_id"] == msg_id)
        }),
        Err(_) => true,
    }
}

/// Idle in the registry, and the composer recognized and empty now. Any read
/// that fails is not free: a brake.
async fn composer_free(services: &Services, seat: &SeatDescriptor) -> bool {
    if seat.state != SystemState::Idle {
        return false;
    }
    let Some(pane) = seat.pane.as_deref() else {
        return false;
    };
    services
        .interaction
        .fresh_injection_verdict(pane)
        .await
        .is_ok_and(|verdict| verdict.permitted && verdict.composer_idle)
}

/// F261: whether `process` is the one running now: its pid exists with the
/// recorded start (in-process: `proc_pidinfo`, or procfs). A dead pid, a
/// recycled one or a failed read is not running: a BRAKE.
async fn running(services: &Services, process: Incarnation) -> bool {
    let proc = pij_core::model::ProcIdentity {
        pid: process.pid,
        proc_start: process.proc_start,
    };
    matches!(
        pij_core::liveness::alive(proc, services.liveness.as_ref()).await,
        Ok(pij_core::model::Liveness::Active)
    )
}

async fn record(
    services: &Services,
    seat: &SeatDescriptor,
    reporter: Incarnation,
    action: Action,
    cause: Cause,
    at: u64,
    msg_id: Option<&str>,
) -> pij_core::error::Result<()> {
    services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at,
            kind: AUTH_RECOVERY_KIND.to_string(),
            seat: Some(seat.id.clone()),
            payload: json!({
                "action": action,
                "at": at,
                "pid": reporter.pid,
                "proc_start": reporter.proc_start,
                "kind": cause.wire(),
                "msg_id": msg_id,
            })
            .to_string(),
        })
        .await
        .map(|_| ())
}

/// The seat's live parent, and the designated prime of each project it holds
/// a role in, once each and never the seat itself (as the idle nudge resolves
/// them, `idle_nudge.rs`).
async fn recipients(services: &Services, seat: &SeatDescriptor) -> Vec<SeatId> {
    let mut candidates: Vec<SeatId> = seat.parent.iter().cloned().collect();
    match crate::http::memory_turn::try_holdings_of(services, &seat.id).await {
        Ok(holdings) => {
            let projects: std::collections::BTreeSet<String> =
                holdings.into_iter().map(|held| held.project).collect();
            for project in projects {
                match services.watchdogs().project(&project).await {
                    Ok(Some(record)) => candidates.extend(record.prime_id),
                    Ok(None) => {}
                    Err(cause) => eprintln!("auth recovery: project {project}: {cause}"),
                }
            }
        }
        Err(cause) => eprintln!("auth recovery: holdings of {}: {cause}", seat.id),
    }
    let mut told: Vec<SeatId> = Vec::new();
    for candidate in candidates {
        if candidate == seat.id || told.contains(&candidate) {
            continue;
        }
        if let Ok(Some(row)) = services.registry.get(&candidate).await
            && row.tombstoned_at.is_none()
        {
            told.push(candidate);
        }
    }
    told
}

async fn tell(
    services: &Services,
    seat: &SeatDescriptor,
    action: Action,
    cause: Cause,
    now_ms: u64,
) -> Vec<(SeatId, String)> {
    let body = notice(seat, action, cause);
    let mut told = Vec::new();
    for to in recipients(services, seat).await {
        let id = format!("auth-recovery:{}:{to}:{now_ms}", seat.id);
        let delivered = services
            .delivery
            .accept(bg_msg(&to, body.clone(), id))
            .await
            .map(|receipt| receipt.outcome)
            .map_err(|cause| cause.to_string());
        told.push((to, outcome_text(&delivered)));
    }
    told
}

/// The one-line fix, in the daemon's own words: only the seat id and pane,
/// each rendered by its grammar.
pub(crate) fn notice(seat: &SeatDescriptor, action: Action, cause: Cause) -> String {
    let id = pij_core::render::seat(&seat.id.0);
    let pane = seat
        .pane
        .as_deref()
        .filter(|pane| {
            pane.strip_prefix('%').is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map_or_else(|| "its pane".to_string(), |pane| format!("pane {pane}"));
    let what = match cause {
        Cause::Reported(LostLogin::Authorization) => "a 401 authorization error",
        Cause::Reported(LostLogin::CredentialAuthority) => "a credential-authority sign-in error",
        Cause::ExtensionLost => {
            "its pij extension stopped renewing, as when Copilot's enterprise sign-in lapses"
        }
    };
    let revive = pij_core::directory::shell_word(&seat.id.0);
    match action {
        Action::Relogin => format!(
            "{id}'s Copilot lost its login again ({what}) after pij restarted it, so a restart is not enough: its login needs renewing. Type /login in {pane}, then /restart."
        ),
        Action::NotRestarted => format!(
            "{id}'s Copilot lost its login ({what}); pij typed /restart in {pane} but the same process is still running, so the restart did not take (Copilot may not offer /restart there). Quit it with /exit in {pane}, then run `pij-rs revive {revive}`: it resumes the same session."
        ),
        Action::Exited => format!(
            "{id}'s Copilot is not running ({what}, and its process has exited or could not be checked), so pij did not restart it. Run `pij-rs revive {revive}`: it resumes the same session."
        ),
        _ => format!(
            "{id}'s Copilot lost its login ({what}), and pij did not restart it: it was busy, had text in its composer, or the restart could not be queued. When it is free, type /restart in {pane}, or run `pij send {id} --command restart`. Its session, roles and queue survive a /restart."
        ),
    }
}

/// F261: follow `native.receiver.lapsed` and decide each lapse of a Copilot
/// seat. Event-driven: no timer, nothing per tick, one decision per lapse
/// edge (told once per lapse). A lapse the live subscription cannot deliver
/// (told before it attached, or overwritten in the broadcast ring) is found
/// by [`reconcile`], at attachment and after every gap (B311-01).
pub async fn follow_lapses(services: Services) {
    let mut lapses = attach(&services);
    reconcile(&services).await;
    loop {
        while let Some(event) = lapses.next().await {
            let (Some(seat), Some(seq)) = (event.seat.as_ref(), event.seq) else {
                continue;
            };
            if outstanding(&services, seat).await == Some(seq) {
                decide_lapse(&services, seat).await;
            }
        }
        let dropped = lapses.dropped_count();
        if dropped == 0 {
            break;
        }
        eprintln!(
            "pij-rs auth recovery: {dropped} events overwritten before the lapse follower read them; reconciling"
        );
        lapses = attach(&services);
        reconcile(&services).await;
    }
    eprintln!("pij-rs auth recovery: native.receiver.lapsed subscription ended");
}

/// The follower's live subscription: it ends at a gap rather than skip one.
fn attach(services: &Services) -> crate::events::Subscription {
    services
        .event_bus
        .subscribe_live_until_gap(EventFilter::kinds([
            crate::delivery::NATIVE_RECEIVER_LAPSED,
        ]))
}

/// B311-01: decide every lapse outstanding on the spine. One roster read and
/// one indexed spine read per live native Copilot seat (the shape of the
/// delivery engine's R277-2 restore); a seat whose read fails is skipped,
/// which can only spare it.
pub(crate) async fn reconcile(services: &Services) {
    let seats = match services
        .registry
        .list(SeatFilter {
            harness: Some(Harness::Copilot),
            live_only: true,
            ..SeatFilter::default()
        })
        .await
    {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("auth recovery: outstanding lapses not read: {error}");
            return;
        }
    };
    for seat in seats
        .iter()
        .filter(|seat| seat.native_extension_delivery && seat.tombstoned_at.is_none())
    {
        if outstanding(services, &seat.id).await.is_some() {
            decide_lapse(services, &seat.id).await;
        }
    }
}

/// The seat's undecided lapse: its latest receiver edge or recovery decision,
/// when that is a `native.receiver.lapsed`. A renewal since (N311-01), a
/// decision since (already handled, live or by an earlier reconcile or
/// daemon) or a failed read is none: a BRAKE, it can only spare the seat.
async fn outstanding(services: &Services, seat: &SeatId) -> Option<Seq> {
    match services
        .spine
        .latest_matching(
            seat,
            &[
                crate::delivery::NATIVE_RECEIVER_LAPSED,
                crate::delivery::NATIVE_RECEIVER_RENEWED,
                AUTH_RECOVERY_KIND,
            ],
        )
        .await
    {
        Ok(Some(event)) if event.kind == crate::delivery::NATIVE_RECEIVER_LAPSED => event.seq,
        Ok(_) => None,
        Err(error) => {
            eprintln!("auth recovery: the receiver edge of {seat} not read: {error}");
            None
        }
    }
}

async fn decide_lapse(services: &Services, seat: &SeatId) {
    if let Err(error) = on_lapse(services, seat, now_ms()).await {
        eprintln!("auth recovery: lapse of {seat} not decided: {error}");
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One lapse: a live Copilot seat with a recorded process is decided as
/// [`Cause::ExtensionLost`] in that process; any other seat is left alone.
///
/// # Errors
/// Store failures reading the seat or recording the decision.
pub(crate) async fn on_lapse(
    services: &Services,
    seat: &SeatId,
    now_ms: u64,
) -> pij_core::error::Result<Option<Outcome>> {
    let Some(row) = services.registry.get(seat).await? else {
        return Ok(None);
    };
    let Some(proc) = row.proc else {
        return Ok(None);
    };
    if row.harness != Harness::Copilot || row.tombstoned_at.is_some() {
        return Ok(None);
    }
    let reporter = Incarnation {
        pid: proc.pid,
        proc_start: proc.proc_start,
    };
    handle(services, &row, reporter, Cause::ExtensionLost, now_ms)
        .await
        .map(Some)
}

#[cfg(test)]
#[path = "auth_recovery_tests.rs"]
mod tests;
