//! Automatic reconciliation through the canonical two-witness reaper.
//! Unknown evidence and pane presence are brakes, never reasons to retire a seat.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pij_core::BG_ACTOR;
use pij_core::error::{PijError, Result};
use pij_core::liveness::alive;
use pij_core::model::{DeliveryOutcome, Event, Harness, Liveness, SeatDescriptor, SeatId};
use pij_core::ports::SeatFilter;
use pij_core::watchdog::is_watched_pa;
use serde_json::json;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{Services, lifecycle, reaper};

/// Default observation cadence; independent of delivery retries.
pub const DEFAULT_DEATH_SWEEP_MS: u64 = 5_000;

fn error(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/death-sweep".into(),
        message: message.into(),
    }
}

pub(crate) fn interval() -> Result<lifecycle::TickInterval> {
    let millis = match std::env::var("PIJ_RS_DEATH_SWEEP_MS") {
        Ok(value) => value.parse::<u64>().map_err(|_| {
            error("PIJ_RS_DEATH_SWEEP_MS must be a positive integer in milliseconds")
        })?,
        Err(std::env::VarError::NotPresent) => DEFAULT_DEATH_SWEEP_MS,
        Err(_) => return Err(error("PIJ_RS_DEATH_SWEEP_MS must be valid Unicode")),
    };
    if millis == 0 {
        return Err(error("PIJ_RS_DEATH_SWEEP_MS must be greater than zero"));
    }
    lifecycle::TickInterval::new(Duration::from_millis(millis))
}

pub(crate) fn start(
    services: Arc<Services>,
    interval: lifecycle::TickInterval,
) -> lifecycle::TickLoop {
    let mass = Arc::new(Mutex::new(MassDeath::default()));
    lifecycle::TickLoop::start_validated(interval, move || {
        let services = Arc::clone(&services);
        let mass = Arc::clone(&mass);
        async move {
            match crate::http::system_time_ms() {
                Ok(now_ms) => {
                    if let Err(error) = sweep_and_alarm(&services, &mass, now_ms).await {
                        eprintln!("death sweep: {error}");
                    }
                }
                Err(error) => eprintln!("death sweep: {error}"),
            }
            Ok(())
        }
    })
}

/// F271b: one sweep retiring at least this many seats raises the alarm.
pub const MASS_DEATH_PER_SWEEP: usize = 5;
/// F271b: this many retirements within [`MASS_DEATH_WINDOW_MS`] raise it too.
pub const MASS_DEATH_PER_WINDOW: usize = 10;
/// F271b: the rolling window, and the least time between two alarms.
pub const MASS_DEATH_WINDOW_MS: u64 = 60_000;
/// Seat ids named in the PA's message; the spine event keeps them all.
const MASS_DEATH_NAMED: usize = 20;

/// Retirements over the last [`MASS_DEATH_WINDOW_MS`] and when the alarm last
/// fired. In memory on the sweep task: a restart forgets it, which can only
/// delay an alarm, never invent one.
#[derive(Debug, Default)]
pub(crate) struct MassDeath {
    recent: VecDeque<(u64, SeatId, Harness)>,
    last_alarm_at: Option<u64>,
}

/// What one alarm reports: this sweep's count, the window's, and the window's
/// seats in retirement order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MassDeathAlarm {
    pub in_sweep: usize,
    pub in_window: usize,
    pub seats: Vec<(SeatId, Harness)>,
}

impl MassDeath {
    /// Record one sweep's retirements at `now_ms`; return an alarm when a
    /// threshold is met and none fired in the last window. Deaths during that
    /// quiet minute fold into the window rather than raising a second alarm.
    pub(crate) fn observe(
        &mut self,
        now_ms: u64,
        retired: &[SeatDescriptor],
    ) -> Option<MassDeathAlarm> {
        self.recent
            .retain(|(at, _, _)| now_ms.saturating_sub(*at) < MASS_DEATH_WINDOW_MS);
        self.recent.extend(
            retired
                .iter()
                .map(|seat| (now_ms, seat.id.clone(), seat.harness)),
        );
        let in_sweep = retired.len();
        let in_window = self.recent.len();
        if in_sweep < MASS_DEATH_PER_SWEEP && in_window < MASS_DEATH_PER_WINDOW {
            return None;
        }
        if self
            .last_alarm_at
            .is_some_and(|last| now_ms.saturating_sub(last) < MASS_DEATH_WINDOW_MS)
        {
            return None;
        }
        self.last_alarm_at = Some(now_ms);
        Some(MassDeathAlarm {
            in_sweep,
            in_window,
            seats: self
                .recent
                .iter()
                .map(|(_, seat, harness)| (seat.clone(), *harness))
                .collect(),
        })
    }
}

/// One sweep, then the mass-death check on what it retired.
///
/// # Errors
/// The sweep's own failures; the alarm's delivery failures are recorded in
/// its `death.mass` event instead.
pub(crate) async fn sweep_and_alarm(
    services: &Services,
    mass: &Mutex<MassDeath>,
    now_ms: u64,
) -> Result<reaper::ReapReceipt> {
    let (receipt, retired) = sweep_retiring(services).await?;
    let alarm = mass
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .observe(now_ms, &retired);
    if let Some(alarm) = alarm {
        raise(services, &alarm, now_ms).await;
    }
    Ok(receipt)
}

fn alarm_body(alarm: &MassDeathAlarm, by_harness: &BTreeMap<&str, usize>) -> String {
    let harnesses = by_harness
        .iter()
        .map(|(harness, count)| format!("{harness} {count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut named = alarm
        .seats
        .iter()
        .take(MASS_DEATH_NAMED)
        .map(|(seat, _)| seat.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if alarm.seats.len() > MASS_DEATH_NAMED {
        write!(named, " (+{} more)", alarm.seats.len() - MASS_DEATH_NAMED)
            .expect("writing to String cannot fail");
    }
    format!(
        "[pij] mass death: {} seats retired in one sweep, {} in the last {} s ({harnesses}): {named}. Many at once means a shared cause (a kill by pattern, an update, tmux): find it before reviving. Spine: death.mass.",
        alarm.in_sweep,
        alarm.in_window,
        MASS_DEATH_WINDOW_MS / 1_000,
    )
}

/// Wake each live PA once and record the alarm on the spine as `death.mass`.
async fn raise(services: &Services, alarm: &MassDeathAlarm, now_ms: u64) {
    let mut by_harness = BTreeMap::new();
    for (_, harness) in &alarm.seats {
        *by_harness.entry(harness.as_str()).or_insert(0_usize) += 1;
    }
    let body = alarm_body(alarm, &by_harness);
    let mut wakes = Vec::new();
    let pas = async {
        let mut seats = services
            .registry
            .list(SeatFilter {
                live_only: true,
                ..SeatFilter::default()
            })
            .await?;
        services.roles.join_roles(&mut seats).await?;
        Ok::<_, PijError>(seats.into_iter().filter(is_watched_pa).collect::<Vec<_>>())
    }
    .await;
    match pas {
        Ok(pas) => {
            for pa in pas {
                let msg_id = format!("death-mass:{now_ms}:{}", pa.id);
                let (delivery, cold_check) = crate::pa_watchdog::wake_pa(
                    services,
                    &pa,
                    body.clone(),
                    msg_id,
                    "mass-death",
                    now_ms,
                )
                .await;
                wakes.push(json!({"pa": pa.id, "delivery": delivery, "cold_check": cold_check}));
            }
        }
        Err(cause) => {
            eprintln!("death sweep: mass-death PA lookup failed: {cause}");
            wakes.push(json!({"error": cause.to_string()}));
        }
    }
    let event = Event {
        seq: None,
        v: 1,
        at: now_ms,
        kind: "death.mass".into(),
        seat: None,
        payload: json!({
            "in_sweep": alarm.in_sweep,
            "in_window": alarm.in_window,
            "window_ms": MASS_DEATH_WINDOW_MS,
            "thresholds": {"sweep": MASS_DEATH_PER_SWEEP, "window": MASS_DEATH_PER_WINDOW},
            "by_harness": by_harness,
            "seats": alarm.seats.iter().map(|(seat, _)| seat).collect::<Vec<_>>(),
            "wakes": wakes,
        })
        .to_string(),
    };
    if let Err(cause) = services.event_bus.publish(event).await {
        eprintln!("death sweep: death.mass event failed: {cause}");
    }
}

fn iso(at: u64) -> Result<String> {
    let instant = OffsetDateTime::from_unix_timestamp_nanos(i128::from(at) * 1_000_000)
        .map_err(|cause| error(cause.to_string()))?;
    let mut text = instant
        .replace_nanosecond(0)
        .map_err(|cause| error(cause.to_string()))?
        .format(&Rfc3339)
        .map_err(|cause| error(cause.to_string()))?;
    text.pop(); // Replace UTC's Z with an explicitly millisecond-precision suffix.
    write!(text, ".{:03}Z", at % 1_000).expect("writing to String cannot fail");
    Ok(text)
}

fn body(seat: &SeatDescriptor, at: &str) -> String {
    let process = seat.proc.map_or_else(
        || "it never registered a process".to_string(),
        |proc| format!("pid {} gone", proc.pid),
    );
    format!(
        "[pij] seat {} died (observed-dead) — pane {} absent, {process} at {at}. It was your child; revive with `pij-rs revive {}` or leave it.",
        seat.id,
        seat.pane.as_deref().unwrap_or("none"),
        seat.id,
    )
}

/// Reconcile observed-dead seats and notify their surviving parents.
/// All candidates are retired before any notice, so a reboot cannot send a
/// child's obituary to a parent later in the same sweep's ordering.
///
/// # Errors
/// Canonical reaper failures. Per-seat obituary failures are logged and counted
/// without preventing notices to other surviving parents.
pub async fn sweep(services: &Services) -> Result<reaper::ReapReceipt> {
    Ok(sweep_retiring(services).await?.0)
}

/// [`sweep`], also returning the seats it retired.
async fn sweep_retiring(services: &Services) -> Result<(reaper::ReapReceipt, Vec<SeatDescriptor>)> {
    let (receipt, retired) = reaper::reap_with_reason(
        services.registry.as_ref(),
        services.liveness.as_ref(),
        services.tmux.as_ref(),
        false,
        Some("observed-dead"),
    )
    .await?;
    let mut withheld = 0;
    let mut failures = 0;
    for (committed, seat) in receipt.reaped.iter().zip(&retired) {
        let Some(parent_id) = &seat.parent else {
            continue;
        };
        let mut failed = false;
        let result: Result<()> = async {
            let tombstone = services
                .spine
                .latest_matching(&seat.id, &["seat.tombstone"])
                .await?
                .filter(|event| event.seq == Some(committed.seq))
                .ok_or_else(|| {
                    error(format!(
                        "cannot find committed tombstone {} for {}",
                        committed.seq.0, seat.id
                    ))
                })?;
            let at = iso(tombstone.at)?;
            let parent = services.registry.get(parent_id).await?;
            let dead = parent
                .as_ref()
                .is_some_and(|parent| parent.tombstoned_at.is_some())
                || receipt
                    .candidates
                    .iter()
                    .any(|candidate| &candidate.seat == parent_id);
            let mut payload = json!({
                "parent": parent_id,
                "tombstone_seq": committed.seq,
                "at": at,
                "notice": null,
                "delivery": null,
            });
            if dead {
                withheld += 1;
                payload["withheld"] = json!("recipient-dead");
            } else if let Some(parent) = parent {
                match services
                    .delivery
                    .send(BG_ACTOR.into(), parent.id, body(seat, &at))
                    .await
                {
                    Ok(notice) => {
                        let outcome = match &notice.outcome {
                            DeliveryOutcome::Delivered { .. } => "delivered",
                            DeliveryOutcome::Queued { .. } => "queued",
                            DeliveryOutcome::Held { .. } => "held",
                            DeliveryOutcome::Refused { .. } => "refused",
                        };
                        let unverified = match parent.proc {
                            Some(proc) => !matches!(
                                alive(proc, services.liveness.as_ref()).await,
                                Ok(Liveness::Active)
                            ),
                            None => false,
                        };
                        payload["delivery"] =
                            json!(if unverified { "unverified" } else { outcome });
                        payload["notice"] = json!(notice);
                    }
                    Err(cause) => {
                        failed = true;
                        eprintln!("death sweep: notice for {} failed: {cause}", seat.id);
                        payload["error"] = json!(cause.to_string());
                    }
                }
            } else {
                payload["withheld"] = json!("recipient-missing");
            }
            services
                .event_bus
                .publish(Event {
                    seq: None,
                    v: 1,
                    at: tombstone.at,
                    kind: "death.notice".into(),
                    seat: Some(seat.id.clone()),
                    payload: payload.to_string(),
                })
                .await?;
            Ok(())
        }
        .await;
        if let Err(cause) = result {
            failed = true;
            eprintln!("death sweep: notice for {} failed: {cause}", seat.id);
        }
        failures += usize::from(failed);
    }
    if withheld > 0 {
        eprintln!("death sweep: {withheld} notice(s) withheld — recipient dead too");
    }
    if failures > 0 {
        eprintln!("death sweep: {failures} notice(s) failed");
    }
    Ok((receipt, retired))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::model::{Harness, ProcIdentity};

    #[test]
    fn obituary_has_exact_utc_millisecond_timestamp_and_revival_command() {
        let mut seat = SeatDescriptor::new("pij-child", Harness::Omp, "/work");
        seat.pane = Some("%42".into());
        seat.proc = Some(ProcIdentity {
            pid: 123,
            proc_start: 1,
        });
        assert_eq!(
            body(&seat, &iso(1_788_825_600_007).unwrap()),
            "[pij] seat pij-child died (observed-dead) — pane %42 absent, pid 123 gone at 2026-09-08T00:00:00.007Z. It was your child; revive with `pij-rs revive pij-child` or leave it."
        );
        assert_eq!(iso(0).unwrap(), "1970-01-01T00:00:00.000Z");
    }

    fn dead(count: usize, from: usize) -> Vec<SeatDescriptor> {
        (from..from + count)
            .map(|n| SeatDescriptor::new(format!("pij-dead-{n}"), Harness::Copilot, "/work"))
            .collect()
    }

    /// F271b: five in one sweep alarm at once; four do not; ten spread over a
    /// minute do; deaths in the minute after an alarm fold into it; and a
    /// window that has aged out starts again from nothing.
    #[test]
    fn mass_death_thresholds_window_and_quiet_minute() {
        let mut mass = MassDeath::default();
        assert_eq!(mass.observe(0, &dead(4, 0)), None, "four in one sweep");
        let alarm = mass.observe(5_000, &dead(5, 4)).expect("five in one sweep");
        assert_eq!((alarm.in_sweep, alarm.in_window), (5, 9));
        assert_eq!(alarm.seats.len(), 9, "the window's seats, oldest first");
        assert_eq!(alarm.seats[0].0.as_str(), "pij-dead-0");
        assert_eq!(
            mass.observe(60_000, &dead(20, 9)),
            None,
            "within the quiet minute"
        );
        let again = mass
            .observe(65_000, &dead(0, 0))
            .expect("a minute on, the window still holds the 20");
        assert_eq!((again.in_sweep, again.in_window), (0, 20));

        let mut spread = MassDeath::default();
        for (step, at) in (0..60_000).step_by(6_000).enumerate() {
            let alarm = spread.observe(at, &dead(1, step));
            assert_eq!(alarm.is_some(), step == 9, "step {step}");
        }
        let mut aged = MassDeath::default();
        assert_eq!(aged.observe(0, &dead(4, 0)), None);
        assert_eq!(aged.observe(30_000, &dead(4, 4)), None);
        assert_eq!(
            aged.observe(60_000, &dead(4, 8)),
            None,
            "the first four aged out: eight in the window"
        );
    }
}
