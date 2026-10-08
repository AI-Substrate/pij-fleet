//! Tell a sender, once, that its message parked undelivered (plan 167 R3).
//!
//! Every park commits a sender-addressed `delivery.parked` fact, but a spine fact
//! wakes nobody: a sender that saw `queued` never learnt its message was lost.
//! Modelled on the death sweep's obituary, `pij-bg` sends one short notice.
//!
//! **Admission is the authority** (the background hand-off's ruling B10): each
//! notice has a stable id derived from the parked job, `park-notice-<job_id>`.
//! The queue dedupes on that id and [`DeliveryService::admitted`] remembers it
//! in any state, so a replay can never admit a second notice. The
//! `delivery.park-notice` event is an audit record written after admission and
//! repaired on replay; a lost or failed audit write cannot cause a duplicate.
//!
//! **Committed parks are swept, not only followed.** The follower subscribes
//! live first, then sweeps every live seat's parks from the last
//! [`LOOKBACK_MS`], so a park committed before the follower attached (a
//! restart, a shutdown abort) or dropped by a lagging subscription is still
//! notified. Overlap between the sweep and the live feed is harmless. A failed
//! notice is never treated as done: the sweep reruns after [`RETRY_AFTER`].
//!
//! A sender with no live seat (missing, tombstoned, remote, or `pij-bg` itself)
//! gets nothing, and that is not an error.
//!
//! [`DeliveryService::admitted`]: crate::delivery::DeliveryService::admitted

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::BG_ACTOR;
use pij_core::error::Result;
use pij_core::events::EventFilter;
use pij_core::model::{DeliveryFailure, DeliveryOutcome, Event, JobId, Msg, SeatId};
use pij_core::ports::Spine;
use serde::Deserialize;
use serde_json::json;
use tokio_stream::StreamExt;

use crate::Services;
use crate::events::Subscription;

const PARKED_KIND: &str = "delivery.parked";
/// Audit record of a notice, keyed by the parked message's `msg_id`.
pub const NOTICE_KIND: &str = "delivery.park-notice";
/// How far back a (re)starting or lagging follower looks for parks still owed
/// a notice. Older parks are history, not news.
pub const LOOKBACK_MS: u64 = 24 * 60 * 60 * 1_000;
/// After a failed notice, the sweep reruns this long after the last attempt.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);

/// The canonical `delivery.parked` payload (`pij_core::delivery::parked_events`).
#[derive(Deserialize)]
struct Parked {
    #[serde(rename = "messageId")]
    message_id: String,
    #[serde(rename = "jobId")]
    job_id: JobId,
    recipient: String,
    outcome: DeliveryFailure,
    reason: String,
}

/// The stable notice id for one parked job: the queue's dedupe key.
#[must_use]
pub fn notice_id(job_id: JobId) -> String {
    format!("park-notice-{}", job_id.0)
}

fn body(parked: &Parked) -> String {
    format!(
        "[pij] message {} to {} was not delivered ({}: {}). Resend after the target recovers.",
        parked.message_id,
        parked.recipient,
        parked.outcome.as_str(),
        parked.reason
    )
}

/// Notify the sender of one parked job, at most once per job.
///
/// # Errors
/// Registry, queue, spine, delivery or event-publication failures. A failure
/// before admission leaves nothing admitted; after admission, a replay repairs
/// only the audit record.
pub async fn notify(services: &Services, event: &Event) -> Result<()> {
    let Some(sender) = &event.seat else {
        return Ok(());
    };
    // User spine kinds are open strings; a same-named event that does not
    // decode as a park receipt is not one.
    let Ok(parked) = serde_json::from_str::<Parked>(&event.payload) else {
        return Ok(());
    };
    let live = services
        .registry
        .get(sender)
        .await?
        .is_some_and(|seat| seat.tombstoned_at.is_none());
    if !live {
        return Ok(());
    }
    let id = notice_id(parked.job_id);
    let audited = services
        .event_bus
        .latest_matching_message(sender, NOTICE_KIND, &parked.message_id)
        .await?
        .and_then(|prior| serde_json::from_str::<serde_json::Value>(&prior.payload).ok())
        .is_some_and(|prior| prior["job_id"] == json!(parked.job_id));
    let notice = if services.delivery.admitted(sender, &id).await? {
        None
    } else if audited {
        // Audited but never admitted: the sender refused the notice. A refusal
        // is final for that notice; it is never re-prompted.
        return Ok(());
    } else {
        Some(
            services
                .delivery
                .accept(Msg {
                    from: SeatId::from(BG_ACTOR),
                    to: sender.clone(),
                    body: body(&parked),
                    msg_id: id.clone(),
                    from_machine: None,
                    in_reply_to: None,
                    command: None,
                })
                .await?,
        )
    };
    if audited {
        return Ok(());
    }
    services
        .event_bus
        .publish(Event {
            seq: None,
            v: 1,
            at: event.at,
            kind: NOTICE_KIND.into(),
            seat: Some(sender.clone()),
            payload: json!({
                "msg_id": parked.message_id,
                "job_id": parked.job_id,
                "notice_msg_id": id,
                "recipient": parked.recipient,
                "refused": notice
                    .as_ref()
                    .is_some_and(|notice| matches!(notice.outcome, DeliveryOutcome::Refused { .. })),
                "notice": notice,
            })
            .to_string(),
        })
        .await?;
    Ok(())
}

async fn notify_logged(services: &Services, event: &Event) -> bool {
    match notify(services, event).await {
        Ok(()) => true,
        Err(error) => {
            eprintln!("pij-rs park notice failed: {error}");
            false
        }
    }
}

/// Notify every live seat's parks from the last [`LOOKBACK_MS`]. Returns
/// whether every notice settled; a `false` sweep is retried.
async fn sweep(services: &Services) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let since = now.saturating_sub(LOOKBACK_MS);
    let seats = match services.registry.list(Default::default()).await {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("pij-rs park notice sweep: {error}");
            return false;
        }
    };
    let mut settled = true;
    for seat in seats.iter().filter(|seat| seat.tombstoned_at.is_none()) {
        match services
            .event_bus
            .matching_since(&seat.id, &[PARKED_KIND], since)
            .await
        {
            Ok(parks) => {
                for park in &parks {
                    settled &= notify_logged(services, park).await;
                }
            }
            Err(error) => {
                eprintln!("pij-rs park notice sweep for {}: {error}", seat.id);
                settled = false;
            }
        }
    }
    settled
}

/// Follow parks for the daemon's lifetime. Subscribes live, then sweeps.
pub async fn follow(services: Services) {
    let events = services
        .event_bus
        .subscribe_live(EventFilter::kinds([PARKED_KIND]));
    run(services, events).await;
}

async fn run(services: Services, mut events: Subscription) {
    // Subscribed first: a park committed during the sweep is on the live feed
    // too, and admission dedupes the overlap. A failed notice schedules one
    // sweep; live traffic cannot postpone it.
    let retry_after = || Some(tokio::time::Instant::now() + RETRY_AFTER);
    let mut retry_at = if sweep(&services).await {
        None
    } else {
        retry_after()
    };
    let mut dropped = events.dropped_count();
    loop {
        let next = match retry_at {
            Some(at) => tokio::time::timeout_at(at, events.next()).await.ok(),
            None => Some(events.next().await),
        };
        match next {
            None => {
                retry_at = if sweep(&services).await {
                    None
                } else {
                    retry_after()
                }
            }
            Some(None) => break,
            Some(Some(event)) => {
                let settled = notify_logged(&services, &event).await;
                if events.dropped_count() != dropped {
                    // A lagged subscriber has no cursor to resume from: sweep.
                    dropped = events.dropped_count();
                    retry_at = if sweep(&services).await {
                        None
                    } else {
                        retry_after()
                    };
                } else if !settled && retry_at.is_none() {
                    retry_at = retry_after();
                }
            }
        }
    }
    eprintln!("pij-rs park notice: delivery.parked subscription ended");
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pij_core::config::{AdapterChoice, Config};
    use pij_core::delivery::delivery_kind;
    use pij_core::model::{DeliveryFailure, Event, Harness, ProcIdentity, SeatDescriptor};
    use pij_core::ports::ParkingEvidence;
    use pij_testkit::FreshStore;

    use super::*;

    const TARGET: &str = "pij-park-target";
    const SENDER: &str = "pij-park-sender";

    fn config(store: &FreshStore, capacity: usize) -> Config {
        let mut config = Config {
            store_path: store.path(),
            event_buffer_capacity: capacity,
            ..Default::default()
        };
        config.adapters.registry = AdapterChoice::Real;
        config.adapters.queue = AdapterChoice::Real;
        config.adapters.spine = AdapterChoice::Real;
        config
    }

    /// Real SQLite services; rebuilding them on the same store is a restart.
    async fn boot(config: &Config) -> Services {
        let services =
            crate::build_services(config, std::path::Path::new("/tmp/pij-park-notice-taps"))
                .await
                .expect("real SQLite registry, queue and spine");
        for (id, pid) in [(TARGET, 4241), (SENDER, 4242)] {
            let mut seat = SeatDescriptor::new(id, Harness::Copilot, "/abs/tree");
            seat.proc = Some(ProcIdentity {
                pid,
                proc_start: u64::from(pid),
            });
            seat.harness_session = Some(format!("session-{id}"));
            seat.native_extension_delivery = true;
            services.registry.put(seat).await.unwrap();
        }
        services
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Send SENDER -> TARGET and park it, committing `fillers` in the same
    /// batch after the park (they overflow a small live channel).
    async fn park(services: &Services, label: &str, fillers: usize) -> (JobId, Event) {
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), label)
            .await
            .unwrap();
        let (job_id, job) = services
            .queue
            .peek(&[delivery_kind(&TARGET.into())])
            .await
            .unwrap()
            .unwrap();
        let queue = services.queue.clone();
        let spine = services.event_bus.raw_spine();
        let events = services
            .event_bus
            .publish_committed_batch(async move {
                let evidence = ParkingEvidence {
                    outcome: DeliveryFailure::NativeReceiverUnavailable,
                    reason: "native-extension-unavailable",
                    at: now_ms(),
                };
                let (parked, mut events) = queue
                    .park_delivery(
                        job_id,
                        &TARGET.into(),
                        job.attempt,
                        &evidence,
                        spine.as_ref(),
                    )
                    .await?;
                assert!(parked.is_some());
                for _ in 0..fillers {
                    let mut filler = Event {
                        seq: None,
                        v: 1,
                        at: now_ms(),
                        kind: "test.filler".into(),
                        seat: Some(SENDER.into()),
                        payload: "{}".into(),
                    };
                    filler.seq = Some(spine.append(filler.clone()).await?);
                    events.push(filler);
                }
                Ok((events.clone(), events))
            })
            .await
            .unwrap();
        let parked = events
            .into_iter()
            .find(|event| event.kind == PARKED_KIND)
            .unwrap();
        (job_id, parked)
    }

    /// Every admitted notice job for SENDER, by its dedupe key.
    async fn notice_jobs(store: &FreshStore) -> Vec<String> {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", store.path()))
            .await
            .unwrap();
        let rows = sqlx::query_scalar(
            "SELECT dedupe_key FROM jobs WHERE serial_key = ?1 \
             AND json_extract(payload, '$.from') = ?2 ORDER BY id",
        )
        .bind(SENDER)
        .bind(BG_ACTOR)
        .fetch_all(&pool)
        .await
        .unwrap();
        pool.close().await;
        rows
    }

    async fn audits(services: &Services) -> usize {
        services
            .event_bus
            .matching_since(&SENDER.into(), &[NOTICE_KIND], 0)
            .await
            .unwrap()
            .len()
    }

    async fn until_notices(store: &FreshStore, expected: &[JobId]) {
        // Admission order is not part of the contract; exactly-once per job is.
        let mut want: Vec<String> = expected.iter().map(|job| notice_id(*job)).collect();
        want.sort();
        let admitted = || async {
            let mut jobs = notice_jobs(store).await;
            jobs.sort();
            jobs
        };
        for _ in 0..500 {
            if admitted().await == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(admitted().await, want);
    }

    /// FT-001: a park committed before the follower attached (a restart, a
    /// shutdown abort) is notified on boot, once, alongside fresh parks.
    #[tokio::test]
    async fn a_park_committed_before_restart_is_notified_once_after_boot() {
        let store = FreshStore::new();
        let config = config(&store, 1_024);
        let before = boot(&config).await;
        let (old, _) = park(&before, "parked before restart", 0).await;
        assert!(notice_jobs(&store).await.is_empty());
        drop(before);

        let services = boot(&config).await;
        let follower = tokio::spawn(follow(services.clone()));
        until_notices(&store, &[old]).await;
        let (fresh, _) = park(&services, "parked after boot", 0).await;
        until_notices(&store, &[old, fresh]).await;
        assert_eq!(audits(&services).await, 2);
        follower.abort();
    }

    /// FT-001: a follower that lags (even before handling its first park) has
    /// no cursor to lose; it sweeps and still notifies the dropped park once.
    #[tokio::test]
    async fn a_lagging_follower_sweeps_the_park_it_never_received() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1)).await;
        let follower = tokio::spawn(follow(services.clone()));
        let (first, _) = park(&services, "proves the live loop runs", 0).await;
        until_notices(&store, &[first]).await;
        // One batch overruns the one-slot channel: the park is never received.
        let (dropped, _) = park(&services, "dropped by the live channel", 3).await;
        let (next, _) = park(&services, "wakes the lagged follower", 0).await;
        until_notices(&store, &[first, dropped, next]).await;
        follower.abort();
    }

    /// FT-002: the notice is admitted, then its audit write fails. A replay on
    /// rebuilt services never admits a second notice; it repairs the audit.
    #[tokio::test]
    async fn a_notice_admitted_before_its_audit_failed_is_never_admitted_twice() {
        let store = FreshStore::new();
        let config = config(&store, 1_024);
        let services = boot(&config).await;
        let (job, parked) = park(&services, "audit write fails", 0).await;
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", store.path()))
            .await
            .unwrap();
        sqlx::query(
            "CREATE TRIGGER fail_park_notice BEFORE INSERT ON spine_events \
             WHEN NEW.kind = 'delivery.park-notice' \
             BEGIN SELECT RAISE(ABORT, 'scripted audit fault'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(notify(&services, &parked).await.is_err());
        assert_eq!(notice_jobs(&store).await, vec![notice_id(job)]);
        assert_eq!(audits(&services).await, 0);
        sqlx::query("DROP TRIGGER fail_park_notice")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        drop(services);

        let services = boot(&config).await;
        notify(&services, &parked).await.unwrap();
        notify(&services, &parked).await.unwrap();
        assert_eq!(notice_jobs(&store).await, vec![notice_id(job)]);
        assert_eq!(
            audits(&services).await,
            1,
            "the replay repairs the audit once"
        );
    }
}
