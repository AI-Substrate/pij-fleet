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

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pij_core::BG_ACTOR;
use pij_core::cold_wake::seat_size;
use pij_core::error::{PijError, Result};
use pij_core::model::{DeliveryOutcome, Event, SeatDescriptor, SeatId};
use pij_core::pa_digest::{FleetAnomaly, FleetRow, FleetView};
use pij_core::ports::SeatFilter;
use pij_core::session_status::SessionStatusBlock;
use pij_core::watchdog::{PA_ROLE, WatchdogControl, WatchdogEntry, WatchdogService};
use serde_json::{Value, json};
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
    /// One pass: find due PAs, build each one's fleet, nudge on change.
    ///
    /// # Errors
    /// Registry or role-store failures. Per-PA delivery failures are reported
    /// in the outcome and audited; they never stop the other PAs' rounds.
    pub async fn round(
        &self,
        services: &Services,
        interval_secs: u64,
        now_ms: u64,
    ) -> Result<Vec<RoundOutcome>> {
        let now_secs = now_ms / 1_000;
        let mut seats = services.registry.list(SeatFilter::default()).await?;
        services.roles.join_roles(&mut seats).await?;
        let pas: Vec<&SeatDescriptor> = seats
            .iter()
            .filter(|seat| seat.role.as_deref() == Some(PA_ROLE) && seat.tombstoned_at.is_none())
            .collect();
        if pas.is_empty() {
            return Ok(Vec::new());
        }

        let mut outcomes = Vec::new();
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

        let anomalies = match services.anomalies.list(&BTreeMap::new(), None).await {
            Ok(value) => anomaly_rows(&value),
            Err(cause) => {
                eprintln!("pa watchdog: anomalies unavailable: {cause}");
                Vec::new()
            }
        };
        for nudge in due {
            let Some(pa) = seats.iter().find(|seat| seat.id == nudge.seat) else {
                continue;
            };
            let view = self
                .fleet(services, pa, &seats, &anomalies, interval_secs, now_ms)
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
                        match receipt.outcome {
                            DeliveryOutcome::Delivered { .. } => "delivered",
                            DeliveryOutcome::Queued { .. } => "queued",
                            DeliveryOutcome::Held { .. } => "held",
                            DeliveryOutcome::Refused { .. } => "refused",
                        }
                        .to_string()
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

    /// Every live seat in the PA's prime's repository, sized.
    async fn fleet(
        &self,
        services: &Services,
        pa: &SeatDescriptor,
        seats: &[SeatDescriptor],
        anomalies: &[FleetAnomaly],
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

        let mut reads = tokio::task::JoinSet::new();
        for (index, seat) in members.iter().enumerate() {
            let source = Arc::clone(&services.session_status);
            let (id, harness, session) =
                (seat.id.clone(), seat.harness, seat.harness_session.clone());
            reads.spawn(async move {
                let block = tokio::time::timeout(
                    STATUS_WAIT,
                    crate::http::session_status_block(
                        source.as_ref(),
                        &id,
                        harness,
                        session,
                        now_ms,
                    ),
                )
                .await
                .unwrap_or_else(|_| SessionStatusBlock::Failed {
                    error: format!("no answer within {}s", STATUS_WAIT.as_secs()),
                });
                (index, block)
            });
        }
        let mut blocks: Vec<Option<SessionStatusBlock>> = vec![None; members.len()];
        while let Some(joined) = reads.join_next().await {
            if let Ok((index, block)) = joined {
                blocks[index] = Some(block);
            }
        }
        let rows = members
            .into_iter()
            .zip(blocks)
            .map(|(seat, block)| {
                let block = block.unwrap_or(SessionStatusBlock::Failed {
                    error: "status read panicked".into(),
                });
                FleetRow {
                    size: seat_size(seat.state, &block, now_ms),
                    seat,
                }
            })
            .collect::<Vec<_>>();
        let anomalies = anomalies
            .iter()
            .filter(|anomaly| rows.iter().any(|row| row.seat.id == anomaly.seat))
            .cloned()
            .collect();
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

fn anomaly_rows(value: &Value) -> Vec<FleetAnomaly> {
    value["anomalies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(FleetAnomaly {
                seat: SeatId::from(row["seat"].as_str()?.to_string()),
                kind: row["kind"].as_str()?.to_string(),
                detail: row["detail"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "pa_watchdog_tests.rs"]
mod tests;
