//! Automatic reconciliation through the canonical two-witness reaper.
//! Unknown evidence and pane presence are brakes, never reasons to retire a seat.

use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use pij_core::BG_ACTOR;
use pij_core::error::{PijError, Result};
use pij_core::liveness::alive;
use pij_core::model::{DeliveryOutcome, Event, Liveness, SeatDescriptor};
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
    lifecycle::TickLoop::start_validated(interval, move || {
        let services = Arc::clone(&services);
        async move {
            if let Err(error) = sweep(&services).await {
                eprintln!("death sweep: {error}");
            }
            Ok(())
        }
    })
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
    format!(
        "[pij] seat {} died (observed-dead) — pane {} absent, pid {} gone at {at}. It was your child; revive with `pij-rs revive {}` or leave it.",
        seat.id,
        seat.pane.as_deref().unwrap_or("none"),
        seat.proc
            .expect("the canonical reaper requires a process identity")
            .pid,
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
    Ok(receipt)
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
}
