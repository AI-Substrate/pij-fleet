//! The PA watchdog: each PA is nudged with its fleet's state, nobody else is.
//!
//! PAs get the watchdog by default; no other role does, not even primes
//! (Jordan, 2026-10-09). The PA is the fleet's watchdog and context keeper, so
//! its nudge carries the fleet digest ([`pij_core::pa_digest`]): every live
//! seat in its prime's repository — all worktrees — with context size, idle
//! time, cache warmth, cold-wake price, declared state and open anomalies. The
//! PA acts on the nudge without a second `pij list`.
//!
//! **When: the fleet's idle edge** (plan 172, Jordan 2026-10-13: "an idle
//! fleet means nobody doing anything"). The fleet is the WORKERS in scope:
//! every PA, every `prime`-role seat and the designated prime are supervisors
//! and never count ([`fleet_working`], #52 F1). While any worker is working,
//! nothing is sent. When the last one stops, each PA in scope is nudged once,
//! promptly, while it is most likely still warm; a new idle episode starts
//! only after some worker has worked again ([`IdleEdge`]). There is no
//! fixed cadence and no change detection. The nudge goes through the ordinary
//! delivery path, so the composer and typing gates still apply. It is not held
//! for a cold PA, because waking the PA is the point; a cold wake is audited
//! like a forced `pij send` (`send.cold-wake-forced`, reason
//! [`IDLE_WAKE_REASON`]).
//!
//! **A fleet idle on a usage limit is not nudged at the edge**: nothing can run.
//! Claude Code seats stopped by a structured quota rejection
//! ([`pij_core::usage_limit`]) instead get ONE wake per `(scope, resetsAt)`,
//! at `resetsAt` plus [`LIMIT_WAKE_MARGIN_SECS`], to the PA in scope or, with
//! no PA, to the prime ([`WakeTarget`]). It is claimed in the store before it
//! is sent, so seats sharing a limit, overlapping rounds and restarts never
//! wake twice; a reset already past when first seen wakes at once. The woken
//! seat decides which workers to restart; the watchdog wakes no worker.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pij_core::BG_ACTOR;
use pij_core::anomalies::{
    ANOMALY_ACK_KIND, ANOMALY_CLEAR_KIND, AnomalyThresholds, AnomalyView, Detector,
    StatusStaleDetector,
};
use pij_core::cold_wake::{COLD_WAKE_FORCED_KIND, ColdCheck, seat_size};
use pij_core::error::{PijError, Result};
use pij_core::model::Card;
use pij_core::model::{DeliveryOutcome, Event, Harness, Msg, SeatDescriptor, SeatId, Seq};
use pij_core::orchestration::PrimeState;
use pij_core::pa_digest::{FleetAnomaly, FleetRow, FleetView};
use pij_core::ports::SeatFilter;
use pij_core::ports::SpineWindow;
use pij_core::report::CardRecord;
use pij_core::session_status::SessionStatusBlock;
use pij_core::usage_limit::{
    LIMIT_RESET_REASON, LIMIT_WAKE_MARGIN_SECS, LimitWake, QuotaRejection, WakeTarget, wake_body,
    wake_due_ms,
};
use pij_core::watchdog::{
    IdleEdge, PRIME_ROLE, fleet_working, is_watched_pa, optin_due, optin_nudge,
};
use serde_json::json;
use tokio::sync::Mutex;

use crate::{Services, lifecycle};

/// How often the loop looks at every PA's fleet. Cheap between edges: one
/// registry list, one role join, and each Claude seat's transcript size check.
pub const TICK: Duration = Duration::from_secs(60);

/// The reason recorded when an idle-edge nudge wakes a cold PA.
pub const IDLE_WAKE_REASON: &str = "pa-watchdog: fleet idle";

/// How long one seat's session facts may take before the row says `?`.
const STATUS_WAIT: Duration = Duration::from_secs(3);

/// How long `git rev-parse` may take to name a folder's repository.
const GIT_WAIT: Duration = Duration::from_secs(2);

/// How long one transcript's quota read may take before it counts as none.
const LIMIT_READ_WAIT: Duration = Duration::from_secs(2);

fn error(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/pa-watchdog".into(),
        message: message.into(),
    }
}

/// The stall threshold: `PIJ_RS_WATCHDOG_SECS`, else the configured default.
/// A ready seat quiet this long is flagged in the digest, and it is the
/// default opt-in interval. It never schedules a PA nudge.
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

/// What one round did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundOutcome {
    /// The PA's fleet went idle; the digest was handed to delivery.
    Nudged {
        /// The PA.
        pa: SeatId,
        /// Delivery's receipt outcome, or the error that stopped it.
        delivery: String,
    },
    /// The PA's fleet went idle on a usage limit: no edge nudge, the reset
    /// wake follows instead.
    IdleOnLimit(SeatId),
    /// A usage-limit reset woke the scope's PA, or its prime.
    LimitWoke {
        /// The repository the stopped seats share.
        scope: String,
        /// `resetsAt`, UTC epoch seconds.
        resets_at_secs: u64,
        /// Who was woken, `None` when the scope had nobody to wake.
        target: Option<SeatId>,
        /// Delivery's receipt outcome, the error, or `no-target`.
        delivery: String,
    },
    /// An opted-in seat was quiet a whole interval and was nudged.
    OptInNudged {
        /// The seat.
        seat: SeatId,
        /// Delivery's receipt outcome, or the error that stopped it.
        delivery: String,
    },
}

/// Claude seats stopped on one limit in one scope.
#[derive(Clone, Debug)]
struct Stop {
    limit_type: String,
    seats: Vec<SeatId>,
}

/// A scope's stops, keyed by `(scope, resetsAt)`.
type Stops = BTreeMap<(String, u64), Stop>;

/// A PA's scope: its prime, the repository key, and the live seats in it.
struct Scope<'a> {
    prime: Option<&'a SeatDescriptor>,
    key: String,
    members: Vec<SeatDescriptor>,
}

/// Per-PA idle-edge trackers, the reset schedule, and a folder → repository
/// cache. In memory only: a restart re-arms every tracker (no nudge for a fleet
/// already idle), and the store's `limit_wakes` claim keeps reset wakes single.
#[derive(Default)]
pub struct PaWatchdog {
    edges: Mutex<HashMap<SeatId, IdleEdge>>,
    /// When each `(scope, resetsAt)` was first seen, epoch ms.
    limits_seen: Mutex<HashMap<(String, u64), u64>>,
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

/// Publish one audit event; a failure is logged, never fatal to the round.
async fn audit(
    services: &Services,
    kind: &str,
    seat: &SeatId,
    at: u64,
    payload: serde_json::Value,
) {
    let event = Event {
        seq: None,
        v: 1,
        at,
        kind: kind.into(),
        seat: Some(seat.clone()),
        payload: payload.to_string(),
    };
    if let Err(cause) = services.event_bus.publish(event).await {
        eprintln!("pa watchdog: {kind} audit for {seat} failed: {cause}");
    }
}

/// The send guard's own verdict for `seat` now. When it is cold, the wake
/// about to happen is recorded exactly like a forced `pij send`, before it is
/// sent. Returns the receipt's `cold_check` label.
async fn audit_cold_wake(
    services: &Services,
    seat: &SeatDescriptor,
    msg_id: &str,
    reason: &str,
    now_ms: u64,
) -> Result<String> {
    let verdict = crate::http::cold_wake::verdict(services, seat, now_ms).await?;
    if let ColdCheck::Cold {
        context_tokens,
        idle_ms,
        model,
        estimate_usd,
    } = &verdict
    {
        services
            .event_bus
            .publish(Event {
                seq: None,
                v: pij_core::wire::EVENT_VERSION,
                at: now_ms,
                kind: COLD_WAKE_FORCED_KIND.to_string(),
                seat: Some(seat.id.clone()),
                payload: json!({
                    "from": BG_ACTOR,
                    "from_machine": null,
                    "msg_id": msg_id,
                    "reason": reason,
                    "context_tokens": context_tokens,
                    "idle_ms": idle_ms,
                    "model": model,
                    "estimate_usd": estimate_usd,
                })
                .to_string(),
            })
            .await?;
    }
    Ok(verdict.receipt_label(true))
}

fn bg_msg(to: &SeatId, body: String, msg_id: String) -> Msg {
    Msg {
        from: SeatId::from(BG_ACTOR),
        to: to.clone(),
        body,
        msg_id,
        from_machine: None,
        in_reply_to: None,
        command: None,
    }
}

impl PaWatchdog {
    /// One pass: nudge opted-in seats that went quiet, fire due limit-reset
    /// wakes, then nudge each PA whose fleet just went idle.
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
        let mut seats = services.registry.list(SeatFilter::default()).await?;
        services.roles.join_roles(&mut seats).await?;
        let mut outcomes = self.optin_round(services, &seats, now_ms).await;
        let stops = self.stops(services, &seats).await;
        outcomes.extend(self.limit_round(services, &seats, &stops, now_ms).await);

        let pas: Vec<&SeatDescriptor> = seats.iter().filter(|seat| is_watched_pa(seat)).collect();
        // The designated prime is a supervisor whatever its role says: its
        // turns never open or re-arm an idle episode (#52 F1).
        let designated = if pas.is_empty() {
            None
        } else {
            services
                .watchdogs()
                .prime()
                .await?
                .filter(|designation| designation.state == PrimeState::Current)
                .map(|designation| designation.seat)
        };
        self.edges
            .lock()
            .await
            .retain(|id, _| pas.iter().any(|pa| &pa.id == id));
        for pa in pas {
            let scope = self.scope_of(pa, &seats).await;
            let working = fleet_working(&scope.members, designated.as_ref());
            let edge = self
                .edges
                .lock()
                .await
                .entry(pa.id.clone())
                .or_default()
                .observe(working);
            if !edge {
                continue;
            }
            if stops.keys().any(|(stopped, _)| stopped == &scope.key) {
                audit(
                    services,
                    "watchdog.round",
                    &pa.id,
                    now_ms,
                    json!({
                        "outcome": "idle-on-limit",
                        "prime": scope.prime.map(|prime| &prime.id),
                        "scope": scope.key,
                    }),
                )
                .await;
                outcomes.push(RoundOutcome::IdleOnLimit(pa.id.clone()));
                continue;
            }
            let view = self.fleet(services, pa, scope, interval_secs, now_ms).await;
            let msg_id = format!("pa-watchdog:{}:{now_ms}", pa.id);
            let delivery =
                match audit_cold_wake(services, pa, &msg_id, IDLE_WAKE_REASON, now_ms).await {
                    Ok(cold_check) => match services
                        .delivery
                        .accept(bg_msg(&pa.id, view.digest(), msg_id))
                        .await
                    {
                        Ok(receipt) => (outcome_word(&receipt.outcome).to_string(), cold_check),
                        Err(cause) => (format!("error: {cause}"), cold_check),
                    },
                    Err(cause) => (format!("error: {cause}"), "unknown".to_string()),
                };
            audit(
                services,
                "watchdog.round",
                &pa.id,
                now_ms,
                json!({
                    "outcome": "nudged",
                    "prime": view.prime,
                    "scope": view.scope,
                    "seats": view.rows.len(),
                    "delivery": delivery.0,
                    "cold_check": delivery.1,
                }),
            )
            .await;
            outcomes.push(RoundOutcome::Nudged {
                pa: pa.id.clone(),
                delivery: delivery.0,
            });
        }
        Ok(outcomes)
    }

    /// Every live Claude seat stopped on a usage limit, grouped by scope and
    /// reset. A read that fails or outlives its wait counts as not stopped.
    async fn stops(&self, services: &Services, seats: &[SeatDescriptor]) -> Stops {
        let mut stops = Stops::new();
        for seat in seats
            .iter()
            .filter(|seat| seat.tombstoned_at.is_none() && seat.harness == Harness::Claude)
        {
            let Some(session) = seat.harness_session.clone() else {
                continue;
            };
            let source = Arc::clone(&services.usage_limits);
            let read = tokio::time::timeout(
                LIMIT_READ_WAIT,
                tokio::task::spawn_blocking(move || {
                    source.latest_rejection(Harness::Claude, &session)
                }),
            )
            .await;
            let rejection: QuotaRejection = match read {
                Ok(Ok(Ok(Some(rejection)))) => rejection,
                Ok(Ok(Ok(None))) => continue,
                Ok(Ok(Err(cause))) => {
                    eprintln!("pa watchdog: usage limit of {}: {cause}", seat.id);
                    continue;
                }
                Ok(Err(cause)) => {
                    eprintln!("pa watchdog: usage limit of {}: {cause}", seat.id);
                    continue;
                }
                Err(_) => continue,
            };
            let scope = self.repo(&seat.folder).await;
            stops
                .entry((scope, rejection.resets_at_secs))
                .or_insert_with(|| Stop {
                    limit_type: rejection.limit_type.clone(),
                    seats: Vec::new(),
                })
                .seats
                .push(seat.id.clone());
        }
        stops
    }

    /// Fire each due limit-reset wake once. Each new `(scope, resetsAt)` is
    /// audited as scheduled when first seen.
    async fn limit_round(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        stops: &Stops,
        now_ms: u64,
    ) -> Vec<RoundOutcome> {
        let mut outcomes = Vec::new();
        let mut fresh = Vec::new();
        let seen: HashMap<(String, u64), u64> = {
            let mut seen = self.limits_seen.lock().await;
            seen.retain(|key, _| stops.contains_key(key));
            for key in stops.keys() {
                seen.entry(key.clone()).or_insert_with(|| {
                    fresh.push(key.clone());
                    now_ms
                });
            }
            seen.clone()
        };
        for ((scope, resets_at), stop) in stops {
            let due = wake_due_ms(*resets_at);
            let first = stop.seats.first().expect("a stop names its seats");
            if fresh.contains(&(scope.clone(), *resets_at)) {
                audit(
                    services,
                    "watchdog.limit-scheduled",
                    first,
                    now_ms,
                    json!({
                        "scope": scope,
                        "resets_at": resets_at,
                        "limit_type": stop.limit_type,
                        "margin_secs": LIMIT_WAKE_MARGIN_SECS,
                        "due_at_ms": due,
                        "stopped": stop.seats,
                    }),
                )
                .await;
            }
            if now_ms < due {
                continue;
            }
            let late = seen
                .get(&(scope.clone(), *resets_at))
                .is_some_and(|&first_seen| first_seen >= due);
            let target = match self.wake_target(services, scope, seats).await {
                Ok(target) => target,
                Err(cause) => {
                    eprintln!("pa watchdog: limit wake target for {scope}: {cause}");
                    continue;
                }
            };
            let wake = LimitWake {
                scope: scope.clone(),
                resets_at_secs: *resets_at,
                limit_type: stop.limit_type.clone(),
                target: target.as_ref().map(|target| target.seat().clone()),
                target_source: target
                    .as_ref()
                    .map_or("none", WakeTarget::source)
                    .to_string(),
                fired_at_ms: now_ms,
            };
            match services.watchdogs().claim_limit_wake(&wake).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(cause) => {
                    eprintln!("pa watchdog: limit wake claim for {scope}: {cause}");
                    continue;
                }
            }
            let (delivery, cold_check) = match target
                .as_ref()
                .and_then(|target| seats.iter().find(|seat| &seat.id == target.seat()))
            {
                None => ("no-target".to_string(), None),
                Some(seat) => {
                    let msg_id = format!("limit-reset:{resets_at}:{}", seat.id);
                    let local = pij_harnesses::local_clock_text(*resets_at, None)
                        .unwrap_or_else(|| format!("{resets_at} UTC epoch"));
                    let body = wake_body(&stop.limit_type, *resets_at, &local, &stop.seats, late);
                    match audit_cold_wake(services, seat, &msg_id, LIMIT_RESET_REASON, now_ms).await
                    {
                        Ok(cold_check) => {
                            let delivery = match services
                                .delivery
                                .accept(bg_msg(&seat.id, body, msg_id))
                                .await
                            {
                                Ok(receipt) => outcome_word(&receipt.outcome).to_string(),
                                Err(cause) => format!("error: {cause}"),
                            };
                            (delivery, Some(cold_check))
                        }
                        Err(cause) => (format!("error: {cause}"), None),
                    }
                }
            };
            audit(
                services,
                "watchdog.limit-wake",
                wake.target.as_ref().unwrap_or(first),
                now_ms,
                json!({
                    "scope": scope,
                    "resets_at": resets_at,
                    "limit_type": stop.limit_type,
                    "target": wake.target,
                    "target_source": wake.target_source,
                    "reason": LIMIT_RESET_REASON,
                    "late": late,
                    "stopped": stop.seats,
                    "cold_check": cold_check,
                    "delivery": delivery,
                }),
            )
            .await;
            outcomes.push(RoundOutcome::LimitWoke {
                scope: scope.clone(),
                resets_at_secs: *resets_at,
                target: wake.target,
                delivery,
            });
        }
        outcomes
    }

    /// Who a scope's limit-reset wake goes to: its PA; else the prime's
    /// current designation, when that seat is live in scope; else a live seat
    /// in scope holding the `prime` role. Ties go to the lowest seat id.
    async fn wake_target(
        &self,
        services: &Services,
        scope: &str,
        seats: &[SeatDescriptor],
    ) -> Result<Option<WakeTarget>> {
        let mut pas = Vec::new();
        for pa in seats.iter().filter(|seat| is_watched_pa(seat)) {
            if self.scope_of(pa, seats).await.key == scope {
                pas.push(pa.id.clone());
            }
        }
        if let Some(pa) = pas.into_iter().min() {
            return Ok(Some(WakeTarget::Pa(pa)));
        }
        let live = |id: &SeatId| {
            seats
                .iter()
                .find(|seat| &seat.id == id && seat.tombstoned_at.is_none())
        };
        if let Some(designated) = services
            .watchdogs()
            .prime()
            .await?
            .filter(|designation| designation.state == PrimeState::Current)
            && let Some(seat) = live(&designated.seat)
            && self.repo(&seat.folder).await == scope
        {
            return Ok(Some(WakeTarget::Designation(seat.id.clone())));
        }
        let mut primes = Vec::new();
        for seat in seats
            .iter()
            .filter(|seat| seat.tombstoned_at.is_none() && seat.role.as_deref() == Some(PRIME_ROLE))
        {
            if self.repo(&seat.folder).await == scope {
                primes.push(seat.id.clone());
            }
        }
        Ok(primes.into_iter().min().map(WakeTarget::PrimeRole))
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

    /// The PA's scope: its prime's repository (or, with no live parent, its
    /// own), and every live seat in it, all worktrees included.
    async fn scope_of<'a>(&self, pa: &SeatDescriptor, seats: &'a [SeatDescriptor]) -> Scope<'a> {
        let prime = pa
            .parent
            .as_ref()
            .and_then(|parent| seats.iter().find(|seat| &seat.id == parent));
        let anchor = prime.map_or(pa.folder.as_str(), |prime| prime.folder.as_str());
        let key = self.repo(anchor).await;
        let mut members = Vec::new();
        for seat in seats.iter().filter(|seat| seat.tombstoned_at.is_none()) {
            if self.repo(&seat.folder).await == key {
                members.push(seat.clone());
            }
        }
        Scope {
            prime,
            key,
            members,
        }
    }

    /// Every live seat in the PA's scope, sized.
    async fn fleet(
        &self,
        services: &Services,
        pa: &SeatDescriptor,
        scope: Scope<'_>,
        interval_secs: u64,
        now_ms: u64,
    ) -> FleetView {
        let Scope {
            prime,
            key: scope,
            members,
        } = scope;

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
