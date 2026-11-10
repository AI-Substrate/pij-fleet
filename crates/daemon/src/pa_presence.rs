//! PA presence and large seats (plan 217, Jordan 2026-11-07).
//!
//! Four checks ride the PA watchdog's round, per scope: a prime and the live
//! seats in its repository (all worktrees), as the PA watchdog scopes it. A
//! PA counts for the scope its parent's folder names ([`super::PaWatchdog`]'s
//! `scope_of`); a directory `pa` holder is a `pa`-role seat, so it counts too.
//!
//! 1. **PA missing.** A scope active for [`PA_MISSING_AFTER_MS`] with no live
//!    PA: the prime is told once, with the last PA's revive command or the
//!    spawn form.
//! 2. **PA dormant.** A live PA with no turn for [`PA_DORMANT_AFTER_MS`] while
//!    its scope is active: it is woken once with the fleet digest.
//! 3. **PA unresponsive.** No PA turn [`PA_UNRESPONSIVE_AFTER_MS`] after that
//!    wake: the prime is told once.
//! 4. **Large seat.** A fleet seat over [`LARGE_SEAT_TOKENS`]: the prime gets
//!    one FYI per crossing (never a wake); crossings in one round share it.
//! 5. **FYI stall** (F169). A seat holding two or more peer FYIs, idle, the
//!    oldest waiting 15 minutes: its scope's PA (the prime, with no live PA)
//!    gets one normal message per backlog naming each FYI. An FYI wakes no
//!    one, so a hand-off sent as one is a stall nobody sees. The seat itself
//!    is never woken.
//!
//! "Active" ([`ScopePresence`]): the prime or a worker was observed working
//! within [`FLEET_ACTIVE_WINDOW_MS`], and the scope is not on a usage limit.
//! Scopes with fewer than [`PRESENCE_MIN_WORKERS`] workers raise no PA alert.
//!
//! **Once per stretch, across restarts.** Each message is claimed in the
//! store (`presence_claims`) before it is sent, and the claim is released only
//! on positive evidence: a live PA seen, a PA turn, a scope watched inactive
//! for a whole window, a seat seen under the line. A restarted daemon starts
//! its in-memory trackers afresh, which can only delay a message. A refused or
//! failed delivery releases the claim, so the next round retries. A daemon
//! that stops between claim and send loses that one message rather than
//! repeating it.
//!
//! **Cost.** No launch, timer or poll of its own. Per round: one claims read;
//! one grouped read of pending peer FYIs over their pending-only index, and
//! per seat newly due a stall notice one read of its pending FYIs;
//! per live PA in an active scope (or with a standing dormant claim), one
//! indexed `LIMIT 1` read of its latest `seat.activity`; per fleet seat that
//! is working, was working last round, has no completed read, or has posted
//! activity or a size since its last read (an in-memory generation, bumped by
//! `/v1/activity`), one session-status read through the watchdog's bounded
//! reader (in-process; one outstanding read per seat). An unchanged idle seat
//! is never re-read. Repository keys come from the round's existing cache.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use pij_core::BG_ACTOR;
use pij_core::fyi_watch::{
    FYI_STALL_ID_PREFIX, FYI_STALL_REASON, STALL_MIN_FYIS, stall_body, stalled,
};
use pij_core::model::{ProcIdentity, SeatDescriptor, SeatId, SystemState};
use pij_core::session_status::SessionStatusBlock;
use pij_core::usage_limit::limit_current;
use pij_core::watchdog::{
    Activity, LARGE_SEAT_TOKENS, LargeSeat, PA_DORMANT_AFTER_MS, PA_DORMANT_REASON,
    PA_MISSING_AFTER_MS, PA_MISSING_REASON, PA_ROLE, PA_UNRESPONSIVE_AFTER_MS,
    PA_UNRESPONSIVE_REASON, PRESENCE_MIN_WORKERS, PRIME_ROLE, PaObservation, PresenceClaim,
    PresenceKind, PresenceVerdict, ScopeObservation, ScopePresence, is_watched_pa, large_seats,
    large_seats_body, pa_dormant_body, pa_missing_body, pa_unresponsive_body, pa_verdicts,
    scope_prime,
};
use serde_json::json;

use super::{PaWatchdog, RoundOutcome, Stops, audit, bg_msg, outcome_word, wake_pa};
use crate::Services;

/// How far up a parent chain a seat may be and still be in a fleet.
const MAX_FLEET_DEPTH: usize = 16;

/// The presence trackers. In memory: claims live in the store.
#[derive(Default)]
pub(super) struct Trackers {
    /// Per scope key: its activity and missing-PA stretch.
    scopes: HashMap<String, ScopePresence>,
    /// Per PA: when this daemon first watched it.
    pa_watched: HashMap<SeatId, u64>,
    /// Per PA: the last round that saw it working.
    pa_working: HashMap<SeatId, u64>,
    /// Per fleet seat: its last completed size read.
    sizes: HashMap<SeatId, CachedSize>,
    /// Seats seen working last round: one more read catches their final size.
    was_working: HashSet<SeatId>,
    /// The last audited verdict per check and subject: audits are on change.
    audited: HashMap<(PresenceKind, String), PresenceVerdict>,
}

/// One seat's last completed size read, and what it was read against.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CachedSize {
    /// The size, `None` when the read completed but knows no size.
    tokens: Option<u64>,
    model: Option<String>,
    /// [`crate::reported_context::ReportedContext::generation`] before the read.
    generation: u64,
    /// The incarnation it was read for.
    proc: Option<ProcIdentity>,
    session: Option<String>,
    /// The read failed or timed out: `tokens` is the last good one, if any,
    /// and the next round reads again.
    stale: bool,
}

impl CachedSize {
    /// Does this entry still describe `seat` at `generation`?
    fn fresh(&self, seat: &SeatDescriptor, generation: u64) -> bool {
        !self.stale
            && self.generation == generation
            && self.proc == seat.proc
            && self.session == seat.harness_session
    }
}

/// One scope for the presence checks.
struct PresenceScope {
    key: String,
    prime: Option<SeatDescriptor>,
    members: Vec<SeatDescriptor>,
    pas: Vec<SeatDescriptor>,
    workers: usize,
    working: bool,
}

fn live(seat: &SeatDescriptor) -> bool {
    seat.tombstoned_at.is_none() && seat.state != SystemState::Dead
}

fn supervisor(seat: &SeatDescriptor, designated: Option<&SeatId>) -> bool {
    matches!(seat.role.as_deref(), Some(PA_ROLE | PRIME_ROLE)) || designated == Some(&seat.id)
}

/// Did the delivery refuse or fail? Then the claim is released to retry.
fn failed(delivery: &str) -> bool {
    delivery == "refused" || delivery.starts_with("error")
}

impl PaWatchdog {
    /// Run the four checks for every scope. Never fails the round: a store
    /// failure is logged and skips the checks it feeds.
    pub(super) async fn presence_round(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        designated: Option<&SeatId>,
        stops: &Stops,
        interval_secs: u64,
        now_ms: u64,
    ) -> Vec<RoundOutcome> {
        let claims = match services.watchdogs().presence_claims().await {
            Ok(claims) => claims,
            Err(cause) => {
                eprintln!("pa watchdog: presence claims unavailable: {cause}");
                return Vec::new();
            }
        };
        let mut claimed: BTreeMap<(PresenceKind, String), PresenceClaim> = claims
            .into_iter()
            .map(|claim| ((claim.kind, claim.subject.clone()), claim))
            .collect();
        self.prune(services, seats, &mut claimed).await;
        let scopes = self.presence_scopes(seats, designated).await;
        let mut outcomes = Vec::new();
        for scope in &scopes {
            let on_limit = stops.keys().any(|(stopped, resets_at)| {
                stopped == &scope.key && limit_current(*resets_at, now_ms)
            });
            let missing_key = (PresenceKind::Missing, scope.key.clone());
            let (activity, missing, missing_for) = {
                let mut trackers = self.presence.lock().await;
                let tracker = trackers.scopes.entry(scope.key.clone()).or_default();
                let (activity, verdict) = tracker.observe(ScopeObservation {
                    working: scope.working,
                    on_limit,
                    workers: scope.workers,
                    live_pa: !scope.pas.is_empty(),
                    missing_claimed: claimed.contains_key(&missing_key),
                    now_ms,
                });
                (activity, verdict, tracker.missing_for_ms(now_ms))
            };
            outcomes.extend(
                self.missing(
                    services,
                    seats,
                    scope,
                    missing,
                    missing_for,
                    &mut claimed,
                    now_ms,
                )
                .await,
            );
            for pa in &scope.pas {
                outcomes.extend(
                    self.dormant(
                        services,
                        seats,
                        scope,
                        pa,
                        activity,
                        &mut claimed,
                        interval_secs,
                        now_ms,
                    )
                    .await,
                );
            }
        }
        outcomes.extend(
            self.large(services, seats, &scopes, designated, &mut claimed, now_ms)
                .await,
        );
        outcomes.extend(
            self.fyi_stalls(services, seats, &scopes, &mut claimed, now_ms)
                .await,
        );
        {
            let mut trackers = self.presence.lock().await;
            let keys: HashSet<&String> = scopes.iter().map(|scope| &scope.key).collect();
            trackers.scopes.retain(|key, _| keys.contains(key));
            let pas: HashSet<&SeatId> = scopes
                .iter()
                .flat_map(|scope| scope.pas.iter().map(|pa| &pa.id))
                .collect();
            trackers.pa_watched.retain(|pa, _| pas.contains(pa));
            trackers.pa_working.retain(|pa, _| pas.contains(pa));
        }
        outcomes
    }

    /// Release claims whose subject is gone: a dormant or unresponsive claim
    /// on a seat that is no longer a live PA, a large-seat claim on a seat
    /// that is no longer live. Missing claims stay with their scope.
    async fn prune(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
    ) {
        let stale: Vec<(PresenceKind, String)> = claimed
            .keys()
            .filter(|(kind, subject)| {
                let seat = seats.iter().find(|seat| seat.id.as_str() == subject);
                match kind {
                    PresenceKind::Missing => false,
                    PresenceKind::Dormant | PresenceKind::Unresponsive => {
                        !seat.is_some_and(|seat| is_watched_pa(seat) && live(seat))
                    }
                    PresenceKind::LargeSeat | PresenceKind::FyiStall => !seat.is_some_and(live),
                }
            })
            .cloned()
            .collect();
        for (kind, subject) in stale {
            match services.watchdogs().release_presence(kind, &subject).await {
                Ok(_) => {
                    claimed.remove(&(kind, subject));
                }
                Err(cause) => eprintln!(
                    "pa watchdog: release of {} {subject}: {cause}",
                    kind.as_str()
                ),
            }
        }
    }

    /// Every scope: each live prime's repository and each live PA's.
    async fn presence_scopes(
        &self,
        seats: &[SeatDescriptor],
        designated: Option<&SeatId>,
    ) -> Vec<PresenceScope> {
        let mut keys: HashMap<SeatId, String> = HashMap::new();
        for seat in seats.iter().filter(|seat| live(seat)) {
            keys.insert(seat.id.clone(), self.repo(&seat.folder).await);
        }
        let mut scopes: BTreeMap<String, Vec<SeatDescriptor>> = BTreeMap::new();
        for seat in seats.iter().filter(|seat| live(seat)) {
            if designated == Some(&seat.id) || seat.role.as_deref() == Some(PRIME_ROLE) {
                scopes.entry(keys[&seat.id].clone()).or_default();
            }
        }
        let mut pas: BTreeMap<String, Vec<SeatDescriptor>> = BTreeMap::new();
        for pa in seats
            .iter()
            .filter(|seat| is_watched_pa(seat) && live(seat))
        {
            let key = match pa
                .parent
                .as_ref()
                .and_then(|parent| seats.iter().find(|seat| &seat.id == parent))
            {
                Some(parent) => match keys.get(&parent.id) {
                    Some(key) => key.clone(),
                    None => self.repo(&parent.folder).await,
                },
                None => keys[&pa.id].clone(),
            };
            scopes.entry(key.clone()).or_default();
            pas.entry(key).or_default().push(pa.clone());
        }
        for seat in seats.iter().filter(|seat| live(seat)) {
            if let Some(members) = scopes.get_mut(&keys[&seat.id]) {
                members.push(seat.clone());
            }
        }
        scopes
            .into_iter()
            .map(|(key, members)| {
                let workers: Vec<&SeatDescriptor> = members
                    .iter()
                    .filter(|seat| !supervisor(seat, designated))
                    .collect();
                let prime = scope_prime(&members, designated).cloned();
                let working = members.iter().any(|seat| {
                    seat.state == SystemState::Working
                        && (!supervisor(seat, designated)
                            || prime.as_ref().is_some_and(|prime| prime.id == seat.id))
                });
                PresenceScope {
                    workers: workers.len(),
                    working,
                    pas: pas.remove(&key).unwrap_or_default(),
                    prime,
                    members,
                    key,
                }
            })
            .collect()
    }

    /// Audit a check's verdict when it changed since the last audit.
    #[allow(clippy::too_many_arguments)]
    async fn audit_change(
        &self,
        services: &Services,
        kind: PresenceKind,
        subject: &str,
        seat: &SeatId,
        verdict: PresenceVerdict,
        mut payload: serde_json::Value,
        now_ms: u64,
    ) {
        let key = (kind, subject.to_string());
        {
            let mut trackers = self.presence.lock().await;
            let last = trackers.audited.get(&key).copied();
            let first_quiet = last.is_none() && verdict == PresenceVerdict::Quiet;
            if last == Some(verdict) || first_quiet {
                return;
            }
            trackers.audited.insert(key, verdict);
        }
        payload["outcome"] = json!(kind.as_str());
        payload["verdict"] = json!(verdict.as_str());
        audit(services, "watchdog.round", seat, now_ms, payload).await;
    }

    /// Take a claim. `false` (already held, or the store failed) means do
    /// not send.
    async fn claim(
        services: &Services,
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        kind: PresenceKind,
        subject: &str,
        now_ms: u64,
    ) -> bool {
        match services
            .watchdogs()
            .claim_presence(kind, subject, now_ms)
            .await
        {
            Ok(true) => {
                claimed.insert(
                    (kind, subject.to_string()),
                    PresenceClaim {
                        kind,
                        subject: subject.to_string(),
                        claimed_at_ms: now_ms,
                        detail: "{}".into(),
                    },
                );
                true
            }
            Ok(false) => false,
            Err(cause) => {
                eprintln!(
                    "pa watchdog: {} claim for {subject}: {cause}",
                    kind.as_str()
                );
                false
            }
        }
    }

    /// Record delivery's answer on a claim, or release it when delivery
    /// refused or failed so the next round retries.
    async fn settle(
        services: &Services,
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        kind: PresenceKind,
        subject: &str,
        now_ms: u64,
        detail: serde_json::Value,
    ) {
        let delivery = detail["delivery"].as_str().unwrap_or_default().to_string();
        let store = services.watchdogs();
        let written = if failed(&delivery) {
            claimed.remove(&(kind, subject.to_string()));
            store.release_presence(kind, subject).await.map(|_| ())
        } else {
            if let Some(claim) = claimed.get_mut(&(kind, subject.to_string())) {
                claim.detail = detail.to_string();
            }
            store
                .stamp_presence(kind, subject, now_ms, &detail.to_string())
                .await
                .map(|_| ())
        };
        if let Err(cause) = written {
            eprintln!(
                "pa watchdog: {} settle for {subject}: {cause}",
                kind.as_str()
            );
        }
    }

    /// Release a claim to re-arm its check.
    async fn rearm(
        services: &Services,
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        kind: PresenceKind,
        subject: &str,
    ) {
        match services.watchdogs().release_presence(kind, subject).await {
            Ok(_) => {
                claimed.remove(&(kind, subject.to_string()));
            }
            Err(cause) => eprintln!(
                "pa watchdog: {} re-arm for {subject}: {cause}",
                kind.as_str()
            ),
        }
    }

    /// The missing-PA alert.
    #[allow(clippy::too_many_arguments)]
    async fn missing(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        scope: &PresenceScope,
        verdict: PresenceVerdict,
        missing_for: u64,
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        now_ms: u64,
    ) -> Option<RoundOutcome> {
        let kind = PresenceKind::Missing;
        let mut payload = json!({
            "scope": scope.key,
            "prime": scope.prime.as_ref().map(|prime| &prime.id),
            "workers": scope.workers,
            "missing_ms": missing_for,
            "after_ms": PA_MISSING_AFTER_MS,
        });
        let verdict = match (verdict, &scope.prime) {
            (PresenceVerdict::Send, None) => PresenceVerdict::Braked,
            (verdict, _) => verdict,
        };
        let audit_seat = scope
            .prime
            .as_ref()
            .map(|prime| prime.id.clone())
            .or_else(|| scope.members.first().map(|seat| seat.id.clone()))?;
        let mut outcome = None;
        match verdict {
            PresenceVerdict::Armed => Self::rearm(services, claimed, kind, &scope.key).await,
            PresenceVerdict::Send => {
                let prime = scope.prime.as_ref()?;
                if !Self::claim(services, claimed, kind, &scope.key, now_ms).await {
                    return None;
                }
                let last_pa = last_pa(seats, prime, &scope.key, &self.repos).await;
                let body = pa_missing_body(&prime.id, missing_for, last_pa.as_ref(), &prime.folder);
                let msg_id = format!("pa-missing:{}:{now_ms}", prime.id);
                let (delivery, cold_check) = wake_pa(
                    services,
                    prime,
                    body,
                    msg_id.clone(),
                    PA_MISSING_REASON,
                    now_ms,
                )
                .await;
                let detail =
                    json!({"msg_id": msg_id, "delivery": delivery, "cold_check": cold_check});
                Self::settle(services, claimed, kind, &scope.key, now_ms, detail).await;
                payload["last_pa"] = json!(last_pa);
                payload["delivery"] = json!(delivery);
                payload["cold_check"] = json!(cold_check);
                outcome = Some(RoundOutcome::Presence {
                    kind,
                    to: prime.id.clone(),
                    subjects: vec![scope.key.clone()],
                    delivery,
                });
            }
            _ => {}
        }
        self.audit_change(
            services,
            kind,
            &scope.key,
            &audit_seat,
            verdict,
            payload,
            now_ms,
        )
        .await;
        outcome
    }

    /// The dormant wake and the unresponsive alert for one PA.
    #[allow(clippy::too_many_arguments)]
    async fn dormant(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        scope: &PresenceScope,
        pa: &SeatDescriptor,
        activity: Activity,
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        interval_secs: u64,
        now_ms: u64,
    ) -> Vec<RoundOutcome> {
        let working = pa.state == SystemState::Working;
        let (watched_since, seen_working) = {
            let mut trackers = self.presence.lock().await;
            if working {
                trackers.pa_working.insert(pa.id.clone(), now_ms);
            }
            (
                *trackers.pa_watched.entry(pa.id.clone()).or_insert(now_ms),
                trackers.pa_working.get(&pa.id).copied(),
            )
        };
        let subject = pa.id.as_str().to_string();
        let dormant_claim = claimed
            .get(&(PresenceKind::Dormant, subject.clone()))
            .cloned();
        let needs_turn = dormant_claim.is_some()
            || (!working && activity == Activity::Active && scope.workers >= PRESENCE_MIN_WORKERS);
        if !needs_turn {
            return Vec::new();
        }
        let recorded = match services
            .spine
            .latest_matching(&pa.id, &["seat.activity"])
            .await
        {
            Ok(event) => event.map(|event| event.at),
            Err(cause) => {
                // Unknown is not dormant: skip this PA this round.
                eprintln!("pa watchdog: last turn of {}: {cause}", pa.id);
                return Vec::new();
            }
        };
        let last_turn_ms = recorded.max(seen_working);
        let verdicts = pa_verdicts(PaObservation {
            activity,
            workers: scope.workers,
            working,
            last_turn_ms,
            watched_since_ms: watched_since,
            dormant_at_ms: dormant_claim.as_ref().map(|claim| claim.claimed_at_ms),
            unresponsive_claimed: claimed
                .contains_key(&(PresenceKind::Unresponsive, subject.clone())),
            prime: scope.prime.is_some(),
            now_ms,
        });
        let quiet_ms = now_ms.saturating_sub(last_turn_ms.unwrap_or(watched_since));
        let mut outcomes = Vec::new();

        let kind = PresenceKind::Dormant;
        let mut payload = json!({
            "scope": scope.key,
            "pa": pa.id,
            "quiet_ms": quiet_ms,
            "after_ms": PA_DORMANT_AFTER_MS,
            "reason": PA_DORMANT_REASON,
        });
        let send = verdicts.dormant == PresenceVerdict::Send
            && Self::claim(services, claimed, kind, &subject, now_ms).await;
        match verdicts.dormant {
            PresenceVerdict::Armed => Self::rearm(services, claimed, kind, &subject).await,
            PresenceVerdict::Send if send => {
                let view_scope = self.scope_of(pa, seats).await;
                let view = self
                    .fleet(services, pa, view_scope, interval_secs, now_ms)
                    .await;
                let body = pa_dormant_body(&pa.id, quiet_ms, &view.digest());
                let msg_id = format!("pa-dormant:{}:{now_ms}", pa.id);
                let (delivery, cold_check) = wake_pa(
                    services,
                    pa,
                    body,
                    msg_id.clone(),
                    PA_DORMANT_REASON,
                    now_ms,
                )
                .await;
                // F115: the digest it carried shows its held messages once,
                // as the idle-edge and prime-active digests do.
                crate::pa_watchdog::flag_shown(services, &view, &delivery, now_ms).await;
                let detail =
                    json!({"msg_id": msg_id, "delivery": delivery, "cold_check": cold_check});
                Self::settle(services, claimed, kind, &subject, now_ms, detail).await;
                payload["seats"] = json!(view.rows.len());
                payload["delivery"] = json!(delivery);
                payload["cold_check"] = json!(cold_check);
                outcomes.push(RoundOutcome::Presence {
                    kind,
                    to: pa.id.clone(),
                    subjects: vec![subject.clone()],
                    delivery,
                });
            }
            _ => {}
        }
        self.audit_change(
            services,
            kind,
            &subject,
            &pa.id,
            verdicts.dormant,
            payload,
            now_ms,
        )
        .await;

        let kind = PresenceKind::Unresponsive;
        let mut payload = json!({
            "scope": scope.key,
            "pa": pa.id,
            "prime": scope.prime.as_ref().map(|prime| &prime.id),
            "quiet_ms": quiet_ms,
            "after_ms": PA_UNRESPONSIVE_AFTER_MS,
        });
        let send = verdicts.unresponsive == PresenceVerdict::Send
            && scope.prime.is_some()
            && dormant_claim.is_some()
            && Self::claim(services, claimed, kind, &subject, now_ms).await;
        match (verdicts.unresponsive, &scope.prime, &dormant_claim) {
            (PresenceVerdict::Armed, _, _) => Self::rearm(services, claimed, kind, &subject).await,
            (PresenceVerdict::Send, Some(prime), Some(wake)) if send => {
                let detail: serde_json::Value =
                    serde_json::from_str(&wake.detail).unwrap_or_default();
                // Daemon-written, but rendered closed like every field here:
                // the outcome word only, never an error's cause text.
                let delivery = detail["delivery"].as_str().unwrap_or("unknown");
                let receipt = format!(
                    "msg {}, delivery {}",
                    pij_core::render::msg_id(detail["msg_id"].as_str().unwrap_or("unknown")),
                    pij_core::render::word(
                        delivery.split_once(':').map_or(delivery, |(word, _)| word)
                    ),
                );
                let body = pa_unresponsive_body(
                    &prime.id,
                    &pa.id,
                    quiet_ms,
                    now_ms.saturating_sub(wake.claimed_at_ms),
                    &receipt,
                );
                let msg_id = format!("pa-unresponsive:{}:{now_ms}", pa.id);
                let (delivery, cold_check) = wake_pa(
                    services,
                    prime,
                    body,
                    msg_id.clone(),
                    PA_UNRESPONSIVE_REASON,
                    now_ms,
                )
                .await;
                let settled =
                    json!({"msg_id": msg_id, "delivery": delivery, "cold_check": cold_check});
                Self::settle(services, claimed, kind, &subject, now_ms, settled).await;
                payload["wake"] = detail;
                payload["delivery"] = json!(delivery);
                payload["cold_check"] = json!(cold_check);
                outcomes.push(RoundOutcome::Presence {
                    kind,
                    to: prime.id.clone(),
                    subjects: vec![subject.clone()],
                    delivery,
                });
            }
            _ => {}
        }
        self.audit_change(
            services,
            kind,
            &subject,
            &pa.id,
            verdicts.unresponsive,
            payload,
            now_ms,
        )
        .await;
        outcomes
    }

    /// The large-seat FYI, per scope with a prime.
    async fn large(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        scopes: &[PresenceScope],
        designated: Option<&SeatId>,
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        now_ms: u64,
    ) -> Vec<RoundOutcome> {
        // Each scope's fleet: its members but the prime, and live seats whose
        // parent chain reaches one of them. A seat is in the first fleet that
        // holds it.
        let mut fleets: Vec<(&PresenceScope, &SeatDescriptor, Vec<SeatDescriptor>)> = Vec::new();
        let mut placed: HashSet<SeatId> = HashSet::new();
        for scope in scopes {
            let Some(prime) = scope.prime.as_ref() else {
                continue;
            };
            let members: HashSet<&SeatId> = scope.members.iter().map(|seat| &seat.id).collect();
            let fleet: Vec<SeatDescriptor> = seats
                .iter()
                .filter(|seat| {
                    live(seat) && seat.id != prime.id && !supervisor_prime(seat, designated)
                })
                .filter(|seat| members.contains(&seat.id) || descends(seat, seats, &members))
                .filter(|seat| placed.insert(seat.id.clone()))
                .cloned()
                .collect();
            fleets.push((scope, prime, fleet));
        }
        let all: Vec<SeatDescriptor> = fleets
            .iter()
            .flat_map(|(_, _, fleet)| fleet.iter().cloned())
            .collect();
        self.refresh_sizes(services, &all, now_ms).await;
        let sizes = self.presence.lock().await.sizes.clone();

        let mut outcomes = Vec::new();
        let kind = PresenceKind::LargeSeat;
        for (scope, prime, fleet) in fleets {
            let known: Vec<(&SeatId, Option<u64>)> = fleet
                .iter()
                .map(|seat| {
                    (
                        &seat.id,
                        sizes.get(&seat.id).and_then(|cached| cached.tokens),
                    )
                })
                .collect();
            let (crossed, rearmed) = large_seats(known, |seat| {
                claimed.contains_key(&(kind, seat.as_str().to_string()))
            });
            for seat in &rearmed {
                Self::rearm(services, claimed, kind, seat.as_str()).await;
                self.audit_change(
                    services,
                    kind,
                    seat.as_str(),
                    seat,
                    PresenceVerdict::Armed,
                    json!({"prime": prime.id, "scope": scope.key}),
                    now_ms,
                )
                .await;
            }
            let mut large = Vec::new();
            for seat in crossed {
                if !Self::claim(services, claimed, kind, seat.as_str(), now_ms).await {
                    continue;
                }
                let descriptor = fleet.iter().find(|row| row.id == seat);
                let (tokens, model) = sizes
                    .get(&seat)
                    .map(|cached| (cached.tokens, cached.model.clone()))
                    .unwrap_or_default();
                large.push(LargeSeat {
                    tokens: tokens.unwrap_or(LARGE_SEAT_TOKENS),
                    role: descriptor.and_then(|row| row.role.clone()),
                    model: model.or_else(|| descriptor.and_then(|row| row.model.clone())),
                    seat,
                });
            }
            if large.is_empty() {
                continue;
            }
            let pas: Vec<SeatId> = scope.pas.iter().map(|pa| pa.id.clone()).collect();
            let body = large_seats_body(&large, &pas);
            let msg_id = format!(
                "{}{}:{now_ms}",
                pij_core::fyi_watch::LARGE_SEATS_ID_PREFIX,
                prime.id
            );
            let delivery = match services
                .delivery
                .hold_fyi(bg_msg(&prime.id, body, msg_id.clone()))
                .await
            {
                Ok(receipt) => outcome_word(&receipt.outcome).to_string(),
                Err(cause) => format!("error: {cause}"),
            };
            let subjects: Vec<String> = large.iter().map(|row| row.seat.to_string()).collect();
            for row in &large {
                let detail = json!({"msg_id": msg_id, "delivery": delivery, "tokens": row.tokens});
                Self::settle(services, claimed, kind, row.seat.as_str(), now_ms, detail).await;
                self.audit_change(
                    services,
                    kind,
                    row.seat.as_str(),
                    &row.seat,
                    PresenceVerdict::Send,
                    json!({
                        "prime": prime.id,
                        "scope": scope.key,
                        "tokens": row.tokens,
                        "after_tokens": LARGE_SEAT_TOKENS,
                        "msg_id": msg_id,
                        "delivery": delivery,
                    }),
                    now_ms,
                )
                .await;
            }
            outcomes.push(RoundOutcome::Presence {
                kind,
                to: prime.id.clone(),
                subjects,
                delivery,
            });
        }
        outcomes
    }

    /// F169: a seat holding peer FYIs while idle may be stalled on one it had
    /// to act on. Its scope's PA is told once per backlog with a normal send,
    /// or the scope's prime with no live PA; the seat itself is never woken,
    /// cold or warm. The claim re-arms when the backlog is gone or a newer FYI
    /// arrives after the notice.
    async fn fyi_stalls(
        &self,
        services: &Services,
        seats: &[SeatDescriptor],
        scopes: &[PresenceScope],
        claimed: &mut BTreeMap<(PresenceKind, String), PresenceClaim>,
        now_ms: u64,
    ) -> Vec<RoundOutcome> {
        let kind = PresenceKind::FyiStall;
        let store = services.watchdogs();
        let backlogs = match store.pending_peer_fyi_backlogs().await {
            Ok(backlogs) => backlogs,
            Err(cause) => {
                eprintln!("pa watchdog: fyi stall: pending FYIs: {cause}");
                return Vec::new();
            }
        };
        // The newest FYI the notice covered, on the FYIs' own clock; a claim
        // whose answer was never stamped falls back to when it was taken.
        let standing: Vec<(String, u64)> = claimed
            .values()
            .filter(|claim| claim.kind == kind)
            .map(|claim| {
                let noticed = serde_json::from_str::<serde_json::Value>(&claim.detail)
                    .ok()
                    .and_then(|detail| detail["newest_ms"].as_u64())
                    .unwrap_or(claim.claimed_at_ms);
                (claim.subject.clone(), noticed)
            })
            .collect();
        for (subject, noticed) in standing {
            let backlog = backlogs.iter().find(|row| row.seat.as_str() == subject);
            if backlog.is_none_or(|row| row.count < STALL_MIN_FYIS || row.newest_ms > noticed) {
                Self::rearm(services, claimed, kind, &subject).await;
            }
        }
        let mut outcomes = Vec::new();
        for backlog in &backlogs {
            let Some(seat) = seats
                .iter()
                .find(|seat| seat.id == backlog.seat && live(seat))
            else {
                continue;
            };
            if !stalled(backlog, seat.state, now_ms)
                || claimed.contains_key(&(kind, seat.id.to_string()))
            {
                continue;
            }
            let Some(scope) = scope_for(scopes, seats, seat) else {
                continue;
            };
            let mut to: Vec<&SeatDescriptor> =
                scope.pas.iter().filter(|pa| pa.id != seat.id).collect();
            if to.is_empty()
                && let Some(prime) = scope.prime.as_ref().filter(|prime| prime.id != seat.id)
            {
                to.push(prime);
            }
            if to.is_empty() {
                continue;
            }
            let fyis: Vec<_> = match store.pending_fyis(&seat.id).await {
                Ok(fyis) => fyis
                    .into_iter()
                    .filter(|fyi| fyi.sender.as_str() != BG_ACTOR || fyi.from_machine.is_some())
                    .collect(),
                Err(cause) => {
                    eprintln!("pa watchdog: fyi stall: FYIs of {}: {cause}", seat.id);
                    continue;
                }
            };
            if (fyis.len() as u64) < STALL_MIN_FYIS
                || !Self::claim(services, claimed, kind, seat.id.as_str(), now_ms).await
            {
                continue;
            }
            let body = stall_body(&seat.id, &fyis, now_ms);
            let mut told = Vec::new();
            for target in &to {
                let msg_id = format!("{FYI_STALL_ID_PREFIX}{}:{}:{now_ms}", seat.id, target.id);
                let (delivery, cold_check) = wake_pa(
                    services,
                    target,
                    body.clone(),
                    msg_id.clone(),
                    FYI_STALL_REASON,
                    now_ms,
                )
                .await;
                told.push(json!({
                    "seat": target.id,
                    "msg_id": msg_id,
                    "delivery": delivery,
                    "cold_check": cold_check,
                }));
            }
            // One answer for the claim: released to retry only when every
            // target refused or failed.
            let delivery = told
                .iter()
                .filter_map(|row| row["delivery"].as_str())
                .find(|delivery| !failed(delivery))
                .or_else(|| told.first().and_then(|row| row["delivery"].as_str()))
                .unwrap_or("error: no target")
                .to_string();
            let detail = json!({
                "delivery": delivery,
                "told": told,
                "fyis": fyis.len(),
                "newest_ms": backlog.newest_ms,
            });
            Self::settle(services, claimed, kind, seat.id.as_str(), now_ms, detail).await;
            self.audit_change(
                services,
                kind,
                seat.id.as_str(),
                &seat.id,
                PresenceVerdict::Send,
                json!({
                    "scope": scope.key,
                    "fyis": fyis.len(),
                    "oldest_ms": now_ms.saturating_sub(backlog.oldest_ms),
                    "told": told,
                }),
                now_ms,
            )
            .await;
            outcomes.push(RoundOutcome::Presence {
                kind,
                to: to[0].id.clone(),
                subjects: vec![seat.id.to_string()],
                delivery,
            });
        }
        outcomes
    }

    /// Re-read the size of each fleet seat that is working, was working last
    /// round, has no completed read, or has had an activity post or a size
    /// report since its last read (review F002: a whole turn can fall between
    /// two rounds), or was relaunched. An unchanged idle seat's context does
    /// not grow, so its last read stands and no transcript is re-read.
    async fn refresh_sizes(&self, services: &Services, fleet: &[SeatDescriptor], now_ms: u64) {
        let reports = &services.reported_context;
        let due: Vec<(SeatDescriptor, u64)> = {
            let trackers = self.presence.lock().await;
            fleet
                .iter()
                .map(|seat| (seat, reports.generation(&seat.id)))
                .filter(|(seat, generation)| {
                    seat.state == SystemState::Working
                        || trackers.was_working.contains(&seat.id)
                        || !trackers
                            .sizes
                            .get(&seat.id)
                            .is_some_and(|cached| cached.fresh(seat, *generation))
                })
                .map(|(seat, generation)| (seat.clone(), generation))
                .collect()
        };
        let seats: Vec<SeatDescriptor> = due.iter().map(|(seat, _)| seat.clone()).collect();
        let blocks = self.read_blocks(services, &seats, now_ms).await;
        let mut trackers = self.presence.lock().await;
        for ((seat, generation), block) in due.into_iter().zip(blocks) {
            let read = |tokens: Option<u64>, model: Option<String>, stale: bool| CachedSize {
                tokens,
                model,
                generation,
                proc: seat.proc,
                session: seat.harness_session.clone(),
                stale,
            };
            let entry = match block {
                SessionStatusBlock::Known { status, .. } => read(
                    status.context_used_tokens.value().copied(),
                    status.model.value().cloned(),
                    false,
                ),
                // A failed or slow read keeps the last size and is retried.
                SessionStatusBlock::Failed { .. } => {
                    let last = trackers.sizes.get(&seat.id).cloned();
                    read(
                        last.as_ref().and_then(|last| last.tokens),
                        last.and_then(|last| last.model),
                        true,
                    )
                }
                // Unbound, unsupported or no transcript: a completed read
                // that knows no size, until the seat's activity changes.
                _ => read(None, None, false),
            };
            trackers.sizes.insert(seat.id.clone(), entry);
        }
        let ids: HashSet<&SeatId> = fleet.iter().map(|seat| &seat.id).collect();
        trackers.sizes.retain(|seat, _| ids.contains(seat));
        // F170: a held large-seat notice is re-checked against these when it
        // flushes.
        services.seat_sizes.replace(
            trackers
                .sizes
                .iter()
                .filter_map(|(seat, cached)| cached.tokens.map(|tokens| (seat.clone(), tokens)))
                .collect(),
        );
        trackers.was_working = fleet
            .iter()
            .filter(|seat| seat.state == SystemState::Working)
            .map(|seat| seat.id.clone())
            .collect();
    }
}

/// The scope a seat belongs to: the first whose members hold it, else the
/// first its parent chain reaches (as the large-seat fleets place it).
fn scope_for<'a>(
    scopes: &'a [PresenceScope],
    seats: &[SeatDescriptor],
    seat: &SeatDescriptor,
) -> Option<&'a PresenceScope> {
    scopes
        .iter()
        .find(|scope| scope.members.iter().any(|member| member.id == seat.id))
        .or_else(|| {
            scopes.iter().find(|scope| {
                let members: HashSet<&SeatId> =
                    scope.members.iter().map(|member| &member.id).collect();
                descends(seat, seats, &members)
            })
        })
}

/// The designated prime of another scope is never in a fleet.
fn supervisor_prime(seat: &SeatDescriptor, designated: Option<&SeatId>) -> bool {
    designated == Some(&seat.id) || seat.role.as_deref() == Some(PRIME_ROLE)
}

/// Does `seat`'s parent chain reach a scope member?
fn descends(seat: &SeatDescriptor, seats: &[SeatDescriptor], members: &HashSet<&SeatId>) -> bool {
    let mut parent = seat.parent.as_ref();
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_FLEET_DEPTH {
        let Some(id) = parent else {
            return false;
        };
        if members.contains(id) {
            return true;
        }
        if !seen.insert(id.clone()) {
            return false;
        }
        parent = seats
            .iter()
            .find(|candidate| &candidate.id == id)
            .and_then(|candidate| candidate.parent.as_ref());
    }
    false
}

/// The scope's last PA: the most recently tombstoned `pa`-role seat whose
/// parent is the prime, or whose folder the repository cache already places
/// in the scope. Reads only the cache: never a launch.
async fn last_pa(
    seats: &[SeatDescriptor],
    prime: &SeatDescriptor,
    key: &str,
    repos: &tokio::sync::Mutex<HashMap<String, String>>,
) -> Option<SeatId> {
    let repos = repos.lock().await;
    seats
        .iter()
        .filter(|seat| seat.role.as_deref() == Some(PA_ROLE) && !live(seat))
        .filter(|seat| {
            seat.parent.as_ref() == Some(&prime.id)
                || repos.get(&seat.folder).is_some_and(|repo| repo == key)
        })
        .max_by_key(|seat| (seat.tombstoned_at.unwrap_or(0), seat.id.clone()))
        .map(|seat| seat.id.clone())
}
