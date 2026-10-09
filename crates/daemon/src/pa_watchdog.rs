//! The PA watchdog: each PA is nudged with its fleet's state, nobody else is.
//!
//! PAs get the watchdog by default; no other role does, not even primes
//! (Jordan, 2026-10-09). The PA is the fleet's watchdog and context keeper, so
//! its nudge carries the fleet digest ([`pij_core::pa_digest`]): every live
//! seat in its prime's repository — all worktrees — with context size, idle
//! time, cache warmth, cold-wake price, declared state and open anomalies. The
//! PA acts on the nudge without a second `pij list`.
//!
//! **Quiet is a brake, not a policy.** When nothing in the fleet changed since
//! the PA's last nudge, the round sends nothing and waits another interval.
//! Removing that check only makes the watchdog send *more* (the same nudge
//! again), so it is a one-directional interlock against paying for a PA turn
//! that has nothing to look at — the "stop polling when the work dries up"
//! rule PAs used to enforce on themselves with timers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pij_core::BG_ACTOR;
use pij_core::anomalies::{
    ANOMALY_ACK_KIND, ANOMALY_CLEAR_KIND, AnomalyThresholds, AnomalyView, Detector,
    StatusStaleDetector,
};
use pij_core::cold_wake::seat_size;
use pij_core::error::{PijError, Result};
use pij_core::model::Card;
use pij_core::model::{DeliveryOutcome, Event, SeatDescriptor, SeatId, Seq};
use pij_core::pa_digest::{FleetAnomaly, FleetRow, FleetView};
use pij_core::ports::SeatFilter;
use pij_core::ports::SpineWindow;
use pij_core::report::CardRecord;
use pij_core::session_status::SessionStatusBlock;
use pij_core::watchdog::{
    PA_ROLE, WatchdogControl, WatchdogEntry, WatchdogService, optin_due, optin_nudge,
};
use serde_json::json;
use tokio::sync::Mutex;

use crate::{Services, lifecycle};

/// How often the loop looks for a due PA. Cheap when no PA is due: one
/// registry list and one role join.
pub const TICK: Duration = Duration::from_secs(60);

/// How long one seat's session facts may take before the row says `?`.
const STATUS_WAIT: Duration = Duration::from_secs(3);

/// How long `git rev-parse` may take to name a folder's repository.
const GIT_WAIT: Duration = Duration::from_secs(2);

fn error(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/pa-watchdog".into(),
        message: message.into(),
    }
}

/// The nudge interval: `PIJ_RS_WATCHDOG_SECS`, else the configured default.
pub(crate) fn interval_secs(configured: u64) -> Result<u64> {
    let secs = match std::env::var("PIJ_RS_WATCHDOG_SECS") {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| error("PIJ_RS_WATCHDOG_SECS must be a positive integer in seconds"))?,
        Err(std::env::VarError::NotPresent) => configured,
        Err(_) => return Err(error("PIJ_RS_WATCHDOG_SECS must be valid Unicode")),
    };
    if secs == 0 {
        return Err(error("PIJ_RS_WATCHDOG_SECS must be greater than zero"));
    }
    Ok(secs)
}

/// What one round did for one PA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundOutcome {
    /// First sighting of this PA: the clock starts now, nothing is sent.
    Armed(SeatId),
    /// The fleet changed; the digest was handed to delivery.
    Nudged {
        /// The PA.
        pa: SeatId,
        /// Delivery's receipt outcome, or the error that stopped it.
        delivery: String,
    },
    /// Nothing in the fleet changed since the last nudge; nothing was sent.
    Quiet(SeatId),
    /// An opted-in seat was quiet a whole interval and was nudged.
    OptInNudged {
        /// The seat.
        seat: SeatId,
        /// Delivery's receipt outcome, or the error that stopped it.
        delivery: String,
    },
}

#[derive(Clone, Debug)]
struct Memory {
    last_round_secs: u64,
    fingerprint: Option<String>,
}

/// Per-PA round memory and a folder → repository cache. In memory only: a
/// daemon restart re-arms every PA rather than nudging them all at boot.
#[derive(Default)]
pub struct PaWatchdog {
    memory: Mutex<HashMap<SeatId, Memory>>,
    repos: Mutex<HashMap<String, String>>,
    /// When each opted-in seat was last nudged, in seconds.
    optin_nudged: Mutex<HashMap<SeatId, u64>>,
    /// Status reads that outlived their wait, at most one per seat.
    pending_reads: Mutex<HashMap<SeatId, tokio::task::JoinHandle<SessionStatusBlock>>>,
}

/// Start the loop. Errors are logged per tick and never stop it.
pub(crate) fn start(
    services: Arc<Services>,
    interval_secs: u64,
    tick: lifecycle::TickInterval,
) -> lifecycle::TickLoop {
    let watchdog = Arc::new(PaWatchdog::default());
    lifecycle::TickLoop::start_validated(tick, move || {
        let services = Arc::clone(&services);
        let watchdog = Arc::clone(&watchdog);
        async move {
            match crate::http::system_time_ms() {
                Ok(now_ms) => {
                    if let Err(error) = watchdog.round(&services, interval_secs, now_ms).await {
                        eprintln!("pa watchdog: {error}");
                    }
                }
                Err(error) => eprintln!("pa watchdog: {error}"),
            }
            Ok(())
        }
    })
}

impl PaWatchdog {
    /// One pass: nudge opted-in seats that went quiet, then find due PAs,
    /// build each one's fleet, and nudge on change.
    ///
    /// # Errors
    /// Registry or role-store failures. Per-seat delivery failures are
    /// reported in the outcome and audited; they never stop other rounds.
    pub async fn round(
        &self,
        services: &Services,
        interval_secs: u64,
        now_ms: u64,
    ) -> Result<Vec<RoundOutcome>> {
        let now_secs = now_ms / 1_000;
        let mut seats = services.registry.list(SeatFilter::default()).await?;
        services.roles.join_roles(&mut seats).await?;
        let mut outcomes = self.optin_round(services, &seats, now_ms).await;
        let pas: Vec<&SeatDescriptor> = seats
            .iter()
            .filter(|seat| seat.role.as_deref() == Some(PA_ROLE) && seat.tombstoned_at.is_none())
            .collect();
        if pas.is_empty() {
            return Ok(outcomes);
        }

        let mut entries = Vec::new();
        {
            let mut memory = self.memory.lock().await;
            memory.retain(|id, _| pas.iter().any(|pa| &pa.id == id));
            for pa in &pas {
                let Some(seen) = memory.get(&pa.id) else {
                    memory.insert(
                        pa.id.clone(),
                        Memory {
                            last_round_secs: now_secs,
                            fingerprint: None,
                        },
                    );
                    outcomes.push(RoundOutcome::Armed(pa.id.clone()));
                    continue;
                };
                // A fixed cadence from the last round: the PA's own activity
                // never pushes its nudge back ("just send it").
                entries.push(WatchdogEntry {
                    seat: (*pa).clone(),
                    last_activity_at_secs: seen.last_round_secs,
                    control: WatchdogControl::default(),
                });
            }
        }
        let due = WatchdogService::with_interval(interval_secs, entries).tick(now_secs);
        if due.is_empty() {
            return Ok(outcomes);
        }

        for nudge in due {
            let Some(pa) = seats.iter().find(|seat| seat.id == nudge.seat) else {
                continue;
            };
            let view = self
                .fleet(services, pa, &seats, interval_secs, now_ms)
                .await;
            let fingerprint = view.fingerprint();
            let unchanged = {
                let mut memory = self.memory.lock().await;
                let entry = memory.entry(pa.id.clone()).or_insert(Memory {
                    last_round_secs: now_secs,
                    fingerprint: None,
                });
                entry.last_round_secs = now_secs;
                entry.fingerprint.as_deref() == Some(fingerprint.as_str())
            };
            let outcome = if unchanged {
                RoundOutcome::Quiet(pa.id.clone())
            } else {
                let delivery = match services
                    .delivery
                    .send(BG_ACTOR.into(), pa.id.clone(), view.digest())
                    .await
                {
                    Ok(receipt) => {
                        if let Some(entry) = self.memory.lock().await.get_mut(&pa.id) {
                            entry.fingerprint = Some(fingerprint);
                        }
                        outcome_word(&receipt.outcome).to_string()
                    }
                    Err(cause) => format!("error: {cause}"),
                };
                RoundOutcome::Nudged {
                    pa: pa.id.clone(),
                    delivery,
                }
            };
            let (kind, delivery) = match &outcome {
                RoundOutcome::Nudged { delivery, .. } => ("nudged", Some(delivery.clone())),
                _ => ("quiet", None),
            };
            let audit = services
                .event_bus
                .publish(Event {
                    seq: None,
                    v: 1,
                    at: now_ms,
                    kind: "watchdog.round".into(),
                    seat: Some(pa.id.clone()),
                    payload: json!({
                        "outcome": kind,
                        "prime": view.prime,
                        "scope": view.scope,
                        "seats": view.rows.len(),
                        "delivery": delivery,
                    })
                    .to_string(),
                })
                .await;
            if let Err(cause) = audit {
                eprintln!("pa watchdog: audit for {} failed: {cause}", pa.id);
            }
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// Nudge each opted-in seat that has been quiet a whole interval.
    ///
    /// Its clock starts at the latest of its last event, when the watchdog was
    /// turned on, and its last nudge, so turning it on never nudges at once.
    async fn optin_round(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        now_ms: u64,
    ) -> Vec<RoundOutcome> {
        let now_secs = now_ms / 1_000;
        let optins = match services.watchdogs().list_watchdogs().await {
            Ok(optins) => optins,
            Err(cause) => {
                eprintln!("watchdog: opt-ins unavailable: {cause}");
                return Vec::new();
            }
        };
        let mut nudged = self.optin_nudged.lock().await;
        nudged.retain(|seat, _| optins.iter().any(|optin| &optin.seat == seat));
        let mut outcomes = Vec::new();
        for optin in &optins {
            let Some(seat) = seats.iter().find(|seat| seat.id == optin.seat) else {
                continue;
            };
            let quiet_from = seat.last_event_at.unwrap_or(0).max(optin.set_at_ms) / 1_000;
            let anchor = quiet_from.max(nudged.get(&seat.id).copied().unwrap_or(0));
            if !optin_due(seat, optin, anchor, now_secs) {
                continue;
            }
            nudged.insert(seat.id.clone(), now_secs);
            let body = optin_nudge(&seat.id, now_secs.saturating_sub(quiet_from), optin);
            let delivery = match services
                .delivery
                .send(BG_ACTOR.into(), seat.id.clone(), body)
                .await
            {
                Ok(receipt) => outcome_word(&receipt.outcome).to_string(),
                Err(cause) => format!("error: {cause}"),
            };
            let audit = services
                .event_bus
                .publish(Event {
                    seq: None,
                    v: 1,
                    at: now_ms,
                    kind: "watchdog.nudge".into(),
                    seat: Some(seat.id.clone()),
                    payload: json!({
                        "interval_secs": optin.interval_secs,
                        "set_by": optin.set_by,
                        "quiet_secs": now_secs.saturating_sub(quiet_from),
                        "delivery": delivery,
                    })
                    .to_string(),
                })
                .await;
            if let Err(cause) = audit {
                eprintln!("watchdog: audit for {} failed: {cause}", seat.id);
            }
            outcomes.push(RoundOutcome::OptInNudged {
                seat: seat.id.clone(),
                delivery,
            });
        }
        outcomes
    }

    /// Every live seat in the PA's prime's repository, sized.
    async fn fleet(
        &self,
        services: &Services,
        pa: &SeatDescriptor,
        seats: &[SeatDescriptor],
        interval_secs: u64,
        now_ms: u64,
    ) -> FleetView {
        let prime = pa
            .parent
            .as_ref()
            .and_then(|parent| seats.iter().find(|seat| &seat.id == parent));
        let anchor = prime.map_or(pa.folder.as_str(), |prime| prime.folder.as_str());
        let scope = self.repo(anchor).await;
        let mut members = Vec::new();
        for seat in seats.iter().filter(|seat| seat.tombstoned_at.is_none()) {
            if self.repo(&seat.folder).await == scope {
                members.push(seat.clone());
            }
        }

        // One outstanding read per seat, ever (review of #38). A cold read can
        // fold a whole transcript and outlive the wait; the next round must
        // not start another behind it, so a seat whose read is still running
        // shows `?` until that read lands.
        let deadline = tokio::time::Instant::now() + STATUS_WAIT;
        let mut handles = Vec::with_capacity(members.len());
        {
            let mut pending = self.pending_reads.lock().await;
            pending.retain(|_, handle| !handle.is_finished());
            for seat in &members {
                if let Some(running) = pending.remove(&seat.id)
                    && !running.is_finished()
                {
                    pending.insert(seat.id.clone(), running);
                    handles.push(None);
                    continue;
                }
                let source = Arc::clone(&services.session_status);
                let (id, harness, session) =
                    (seat.id.clone(), seat.harness, seat.harness_session.clone());
                handles.push(Some(tokio::spawn(async move {
                    crate::http::session_status_block(
                        source.as_ref(),
                        &id,
                        harness,
                        session,
                        now_ms,
                    )
                    .await
                })));
            }
        }
        let mut blocks = Vec::with_capacity(members.len());
        for (seat, handle) in members.iter().zip(handles) {
            let Some(mut handle) = handle else {
                blocks.push(SessionStatusBlock::Failed {
                    error: "previous read still running".into(),
                });
                continue;
            };
            blocks.push(match tokio::time::timeout_at(deadline, &mut handle).await {
                Ok(Ok(block)) => block,
                Ok(Err(cause)) => SessionStatusBlock::Failed {
                    error: format!("status read failed: {cause}"),
                },
                Err(_) => {
                    self.pending_reads
                        .lock()
                        .await
                        .insert(seat.id.clone(), handle);
                    SessionStatusBlock::Failed {
                        error: format!("no answer within {}s", STATUS_WAIT.as_secs()),
                    }
                }
            });
        }
        let rows = members
            .into_iter()
            .zip(blocks)
            .map(|(seat, block)| FleetRow {
                size: seat_size(seat.state, &block, now_ms),
                seat,
            })
            .collect::<Vec<_>>();
        let anomalies = stale_cards(services, &rows, now_ms).await;
        FleetView {
            pa: pa.id.clone(),
            prime: prime.map(|prime| prime.id.clone()),
            scope,
            rows,
            anomalies,
            interval_ms: interval_secs * 1_000,
        }
    }

    /// The repository a folder belongs to: its absolute git common dir, so
    /// every worktree of one repository shares a key. A folder git cannot
    /// place keys as itself, so it matches only seats in exactly that folder.
    async fn repo(&self, folder: &str) -> String {
        if let Some(known) = self.repos.lock().await.get(folder) {
            return known.clone();
        }
        match git_common_dir(folder).await {
            Some(dir) => {
                self.repos
                    .lock()
                    .await
                    .insert(folder.to_string(), dir.clone());
                dir
            }
            // Not cached: a folder that becomes a repository later is found.
            None => folder.to_string(),
        }
    }
}

async fn git_common_dir(folder: &str) -> Option<String> {
    if !Path::new(folder).is_dir() {
        return None;
    }
    let output = tokio::time::timeout(
        GIT_WAIT,
        tokio::process::Command::new("git")
            .args([
                "-C",
                folder,
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let dir = String::from_utf8(output.stdout).ok()?.trim().to_string();
    let canonical = std::fs::canonicalize(&dir).map_or(dir, |path| path.display().to_string());
    (!canonical.is_empty()).then_some(canonical)
}

const fn outcome_word(outcome: &DeliveryOutcome) -> &'static str {
    match outcome {
        DeliveryOutcome::Delivered { .. } => "delivered",
        DeliveryOutcome::Queued { .. } => "queued",
        DeliveryOutcome::Held { .. } => "held",
        DeliveryOutcome::Refused { .. } => "refused",
    }
}

/// Stale cards in the fleet, read in bounded time.
///
/// `AnomalyService::list` folds the whole spine on every call, which a
/// per-tick loop cannot afford (review of #38). The digest needs one anomaly
/// kind, so this runs the anomaly authority's own [`StatusStaleDetector`]
/// (its threshold and parked-state suppression) over bounded reads:
///
/// 1. per fleet seat, its latest card: one indexed `LIMIT 1` read;
/// 2. per *stale* card only, its own disposition. Clears and acks for every
///    anomaly kind share their event kinds, so "the seat's latest clear" can
///    belong to another occurrence (review of 20bcfb3). Each candidate is
///    therefore matched by occurrence key against the seat's clears and acks
///    since that card, a seat- and kind-indexed window capped at
///    [`DISPOSITION_PAGES`] pages.
///
/// Reads follow the fleet and its stale cards, never history. The other
/// kinds stay with `pij anomalies`.
async fn stale_cards(services: &Services, rows: &[FleetRow], now_ms: u64) -> Vec<FleetAnomaly> {
    let mut seats = Vec::new();
    let mut cards = Vec::new();
    for row in rows {
        let card = match services
            .spine
            .latest_matching(&row.seat.id, &["report.now"])
            .await
        {
            Ok(card) => card,
            Err(cause) => {
                eprintln!("pa watchdog: card read for {} failed: {cause}", row.seat.id);
                continue;
            }
        };
        seats.push(row.seat.clone());
        if let Some(event) = card
            && let Ok(record) = serde_json::from_str::<CardRecord>(&event.payload)
        {
            cards.push(Card {
                seat: row.seat.id.clone(),
                did: record.did,
                next: record.next,
                at: event.at,
                seq: event.seq,
            });
        }
    }
    let view = AnomalyView {
        now_ms,
        thresholds: AnomalyThresholds::default(),
        seats: &seats,
        cards: &cards,
        activity: &[],
        dispatches: &[],
        done: &[],
        dispositions: &[],
        decisions: &[],
        dead: &[],
    };
    let mut stale = Vec::new();
    for row in StatusStaleDetector.scan(&view) {
        let since = cards
            .iter()
            .find(|card| card.seat == row.seat)
            .map_or(0, |card| card.at);
        match disposed(services, &row.seat, &row.occurrence, since).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(cause) => {
                // Unknown is not "cleared": show the card rather than hide it.
                eprintln!(
                    "pa watchdog: disposition read for {} failed: {cause}",
                    row.seat
                );
            }
        }
        stale.push(FleetAnomaly {
            detail: format!(
                "card {} old",
                pij_core::cold_wake::human_duration(row.age_ms.unwrap_or_default())
            ),
            seat: row.seat,
            kind: row.kind.as_str().to_string(),
        });
    }
    stale
}

/// Pages a single disposition search may read, per kind.
const DISPOSITION_PAGES: usize = 4;
/// Rows per disposition page.
const DISPOSITION_PAGE: usize = 256;

/// Was this exact occurrence cleared or acknowledged since its card?
async fn disposed(
    services: &Services,
    seat: &SeatId,
    occurrence: &str,
    since_at: u64,
) -> Result<bool> {
    for kind in [ANOMALY_CLEAR_KIND, ANOMALY_ACK_KIND] {
        let mut window = Some(SpineWindow::new(kind, since_at, Seq(0), DISPOSITION_PAGE)?);
        let mut pages = 0;
        while let Some(current) = window.take() {
            let page = services.spine.matching_since(seat, &current).await?;
            if page.iter().any(|event| {
                serde_json::from_str::<serde_json::Value>(&event.payload)
                    .is_ok_and(|payload| payload["occurrence"] == occurrence)
            }) {
                return Ok(true);
            }
            pages += 1;
            if pages < DISPOSITION_PAGES {
                window = current.next(&page);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "pa_watchdog_tests.rs"]
mod tests;
