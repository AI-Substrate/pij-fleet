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
//! **The notice never wakes a cold seat.** It obeys the send guard's own
//! complete cold-wake decision (`http::cold_wake::verdict`, the stale-working
//! correction included): where a `pij send` would be refused with
//! `E-RS-COLD-WAKE`, the notice is held as an FYI for the sender's next turn
//! under the same id; otherwise it is delivered normally, and a refused delivery
//! (the sender's own receiver is down) also falls back to that FYI. A held FYI
//! counts as admitted, so a seat that warms up later is never notified twice.
//!
//! **Committed parks are swept, not only followed.** The follower subscribes
//! live first, then sweeps every live seat's parks from the last
//! [`LOOKBACK_MS`], so a park committed before the follower attached (a
//! restart, a shutdown abort) or dropped by a lagging subscription is still
//! notified. A filtered subscription that drops a park and then sees only
//! unrelated events is never woken, so the sweep also reruns every
//! [`SWEEP_EVERY`]. Overlap between sweeps and the live feed is harmless. A
//! failed notice is never treated as done: the next sweep comes after
//! [`RETRY_AFTER`].
//!
//! A sender with no live seat (missing, tombstoned, remote, or `pij-bg` itself)
//! gets nothing, and that is not an error.
//!
//! [`DeliveryService::admitted`]: crate::delivery::DeliveryService::admitted

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::BG_ACTOR;
use pij_core::cold_wake::refusal;
use pij_core::error::Result;
use pij_core::events::EventFilter;
use pij_core::model::{DeliveryFailure, DeliveryOutcome, Event, JobId, Msg, SeatId, Seq};
use pij_core::ports::{Spine, SpineWindow};
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
/// After a failed notice, the next sweep runs this long after the attempt.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The sweep reruns at least this often, whether or not anything woke the
/// follower: a lag that drops a park is not itself an event.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// One page of a seat's spine facts during a sweep.
const PAGE: usize = 256;
const FYI_HELD_KIND: &str = "fyi.held";

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
    let Some(seat) = services
        .registry
        .get(sender)
        .await?
        .filter(|seat| seat.tombstoned_at.is_none())
    else {
        return Ok(());
    };
    let id = notice_id(parked.job_id);
    let audited = services
        .event_bus
        .latest_matching_message(sender, NOTICE_KIND, &parked.message_id)
        .await?
        .and_then(|prior| serde_json::from_str::<serde_json::Value>(&prior.payload).ok())
        .is_some_and(|prior| prior["job_id"] == json!(parked.job_id));
    let admitted = services.delivery.admitted(sender, &id).await?
        || fyi_held(services, sender, &id, event.at).await?;
    let (channel, notice) = if admitted {
        (None, None)
    } else if audited {
        // Audited yet not visible as admitted: its delivered-ledger entry aged
        // out. It was sent; it is never sent again.
        return Ok(());
    } else {
        let msg = Msg {
            from: SeatId::from(BG_ACTOR),
            to: sender.clone(),
            body: body(&parked),
            msg_id: id.clone(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        };
        // The very decision `pij send` obeys, stale-working correction included.
        let now = crate::http::system_time_ms()?;
        let cold = crate::http::cold_wake::verdict(services, &seat, now).await?;
        if refusal(sender, &cold).is_some() {
            (Some("fyi"), Some(services.delivery.hold_fyi(msg).await?))
        } else {
            let receipt = services.delivery.accept(msg.clone()).await?;
            if matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) {
                // A refusal (e.g. the sender's own receiver lease lapsed) is
                // final for the wake, never for the notice: hold it for the
                // sender's next turn under the same id (background B9/B10).
                (
                    Some("fyi-after-refusal"),
                    Some(services.delivery.hold_fyi(msg).await?),
                )
            } else {
                (Some("send"), Some(receipt))
            }
        }
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
                "channel": channel,
                "notice": notice,
            })
            .to_string(),
        })
        .await?;
    Ok(())
}

/// Pages of one seat's `kind` facts committed at or after `since_at`.
struct Pages {
    seat: SeatId,
    window: Option<SpineWindow>,
}

impl Pages {
    fn new(seat: &SeatId, kind: &str, since_at: u64) -> Result<Self> {
        Ok(Self {
            seat: seat.clone(),
            window: Some(SpineWindow::new(kind, since_at, Seq(0), PAGE)?),
        })
    }

    /// The next non-empty page, or `None` after the last.
    async fn next(&mut self, services: &Services) -> Result<Option<Vec<Event>>> {
        let Some(window) = self.window.take() else {
            return Ok(None);
        };
        let page = services
            .event_bus
            .matching_since(&self.seat, &window)
            .await?;
        self.window = window.next(&page);
        Ok((!page.is_empty()).then_some(page))
    }
}

/// Was notice `id` held as an FYI for `seat` since the park? The `fyi.held`
/// fact commits in the same transaction as the FYI row it describes, and the
/// FYI row is unique by id, so this stays true after the FYI is delivered.
async fn fyi_held(services: &Services, seat: &SeatId, id: &str, since_at: u64) -> Result<bool> {
    let mut pages = Pages::new(seat, FYI_HELD_KIND, since_at)?;
    while let Some(page) = pages.next(services).await? {
        if page.iter().any(|event| {
            serde_json::from_str::<serde_json::Value>(&event.payload)
                .is_ok_and(|payload| payload["id"] == id)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
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

/// Notify every live seat's parks from the last [`LOOKBACK_MS`], a page at a
/// time. Returns whether every notice settled.
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
        let mut pages = match Pages::new(&seat.id, PARKED_KIND, since) {
            Ok(pages) => pages,
            Err(error) => {
                eprintln!("pij-rs park notice sweep: {error}");
                return false;
            }
        };
        loop {
            match pages.next(services).await {
                Ok(Some(page)) => {
                    for park in &page {
                        settled &= notify_logged(services, park).await;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    eprintln!("pij-rs park notice sweep for {}: {error}", seat.id);
                    settled = false;
                    break;
                }
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
    // Subscribed first: a park committed during a sweep is on the live feed
    // too, and admission dedupes the overlap. The next sweep is a deadline that
    // live traffic cannot postpone: every SWEEP_EVERY, sooner after a failure.
    let next_sweep = |settled: bool| {
        tokio::time::Instant::now() + if settled { SWEEP_EVERY } else { RETRY_AFTER }
    };
    let mut sweep_at = next_sweep(sweep(&services).await);
    let mut dropped = events.dropped_count();
    loop {
        match tokio::time::timeout_at(sweep_at, events.next()).await {
            Err(_) => sweep_at = next_sweep(sweep(&services).await),
            Ok(None) => break,
            Ok(Some(event)) => {
                let settled = notify_logged(&services, &event).await;
                if events.dropped_count() != dropped {
                    // A lagged subscriber has no cursor to resume from: sweep now.
                    dropped = events.dropped_count();
                    sweep_at = next_sweep(sweep(&services).await);
                } else if !settled {
                    sweep_at = sweep_at.min(next_sweep(false));
                }
            }
        }
    }
    eprintln!("pij-rs park notice: delivery.parked subscription ended");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use pij_core::config::{AdapterChoice, Config};
    use pij_core::delivery::delivery_kind;
    use pij_core::model::{DeliveryFailure, Event, Harness, ProcIdentity, SeatDescriptor};
    use pij_core::ports::ParkingEvidence;
    use pij_core::session_status::{Fact, SeatStatus, SessionStatusReply};
    use pij_testkit::FreshStore;
    use pij_testkit::fakes::FakeSessionStatus;
    use sqlx::SqlitePool;

    use super::*;
    use crate::delivery::NativeInboxIdentity;

    const TARGET: &str = "pij-park-target";
    const SENDER: &str = "pij-park-sender";
    const SENDER_SESSION: &str = "session-pij-park-sender";

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

    /// 720k tokens, last called two hours ago: a `pij send` would be refused.
    fn cold_sender() -> FakeSessionStatus {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        FakeSessionStatus::new().with_reply(
            SENDER_SESSION,
            SessionStatusReply::Status(SeatStatus {
                model: Fact::native("claude-opus-5-5".to_string()),
                context_used_tokens: Fact::derived(720_000),
                last_call_at_ms: Fact::native(now - 2 * 60 * 60 * 1_000),
                ..SeatStatus::unknown()
            }),
        )
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    async fn pool(store: &FreshStore) -> SqlitePool {
        SqlitePool::connect(&format!("sqlite:{}", store.path()))
            .await
            .unwrap()
    }

    /// Send SENDER -> TARGET and park it, committing `fillers` unrelated events
    /// in the same batch after the park (they overrun a one-slot live channel).
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

    /// Every queued notice job for SENDER, by its dedupe key, sorted:
    /// admission order is not part of the contract; exactly-once per job is.
    async fn notice_jobs(pool: &SqlitePool) -> Vec<String> {
        let mut jobs: Vec<String> = sqlx::query_scalar(
            "SELECT dedupe_key FROM jobs WHERE serial_key = ?1 \
             AND json_extract(payload, '$.from') = ?2",
        )
        .bind(SENDER)
        .bind(BG_ACTOR)
        .fetch_all(pool)
        .await
        .unwrap();
        jobs.sort();
        jobs
    }

    /// Every FYI held for SENDER, as `(id, state)`.
    async fn notice_fyis(pool: &SqlitePool) -> Vec<(String, String)> {
        sqlx::query_as("SELECT id, state FROM fyis WHERE recipient = ?1 ORDER BY id")
            .bind(SENDER)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    fn ids(jobs: &[JobId]) -> Vec<String> {
        let mut ids: Vec<String> = jobs.iter().map(|job| notice_id(*job)).collect();
        ids.sort();
        ids
    }

    async fn audits(services: &Services) -> usize {
        services
            .event_bus
            .matching_since(
                &SENDER.into(),
                &SpineWindow::new(NOTICE_KIND, 0, Seq(0), SpineWindow::MAX_LIMIT).unwrap(),
            )
            .await
            .unwrap()
            .len()
    }

    /// Poll by yielding, never sleeping, so it is also correct under a frozen
    /// virtual clock; the bound is real time because SQLite IO is real.
    async fn until(mut done: impl AsyncFnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if done().await {
                return true;
            }
            tokio::task::yield_now().await;
        }
        done().await
    }

    async fn until_notices(pool: &SqlitePool, expected: &[JobId]) {
        let want = ids(expected);
        until(async || notice_jobs(pool).await == want).await;
        assert_eq!(notice_jobs(pool).await, want);
    }

    /// Keeps the paused runtime busy so virtual time moves only on `advance`.
    fn hold_clock() -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
            }
        })
    }

    /// FT-001: a park committed before the follower attached (a restart, a
    /// shutdown abort) is notified on boot, once, alongside fresh parks.
    #[tokio::test]
    async fn a_park_committed_before_restart_is_notified_once_after_boot() {
        let store = FreshStore::new();
        let config = config(&store, 1_024);
        let before = boot(&config).await;
        let (old, _) = park(&before, "parked before restart", 0).await;
        let pool = pool(&store).await;
        assert!(notice_jobs(&pool).await.is_empty());
        drop(before);

        let services = boot(&config).await;
        let follower = tokio::spawn(follow(services.clone()));
        until_notices(&pool, &[old]).await;
        let (fresh, _) = park(&services, "parked after boot", 0).await;
        until_notices(&pool, &[old, fresh]).await;
        // Each audit follows its admission; wait for both, then for no more.
        assert!(until(async || audits(&services).await == 2).await);
        follower.abort();
    }

    /// FT-001 lag corner: the one-slot filtered channel drops a park and then
    /// sees only unrelated events, so nothing ever wakes the follower. The
    /// periodic sweep still notifies the dropped park, once, within
    /// SWEEP_EVERY of virtual time and not before it.
    #[tokio::test(start_paused = true)]
    async fn a_dropped_park_followed_only_by_unrelated_events_is_swept_in_time() {
        let clock = hold_clock();
        let store = FreshStore::new();
        let services = boot(&config(&store, 1)).await;
        let pool = pool(&store).await;
        let follower = tokio::spawn(follow(services.clone()));
        let (first, _) = park(&services, "proves the live loop runs", 0).await;
        until_notices(&pool, &[first]).await;

        let (dropped, _) = park(&services, "dropped; only unrelated events follow", 3).await;
        for _ in 0..2_000 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            notice_jobs(&pool).await,
            ids(&[first]),
            "no live event names the dropped park"
        );
        // The sender is a native seat: keep its receiver lease live, as its
        // extension would, so the notice is admitted rather than refused.
        let sender = services
            .registry
            .get(&SENDER.into())
            .await
            .unwrap()
            .unwrap();
        let identity = NativeInboxIdentity {
            native_session: sender.harness_session.clone(),
            pid: sender.proc.map(|proc| proc.pid),
            proc_start: sender.proc.map(|proc| proc.proc_start),
        };
        tokio::time::advance(SWEEP_EVERY - Duration::from_secs(1)).await;
        services
            .delivery
            .heartbeat_native_receiver(&sender.id, &identity, 1, 1)
            .await
            .unwrap();
        assert_eq!(
            notice_jobs(&pool).await,
            ids(&[first]),
            "not before the interval"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        until_notices(&pool, &[first, dropped]).await;
        follower.abort();
        clock.abort();
    }

    /// FT-002: the notice is admitted, then its audit write fails. A replay on
    /// rebuilt services never admits a second notice; it repairs the audit.
    #[tokio::test]
    async fn a_notice_admitted_before_its_audit_failed_is_never_admitted_twice() {
        let store = FreshStore::new();
        let config = config(&store, 1_024);
        let services = boot(&config).await;
        let (job, parked) = park(&services, "audit write fails", 0).await;
        let pool = pool(&store).await;
        sqlx::query(
            "CREATE TRIGGER fail_park_notice BEFORE INSERT ON spine_events \
             WHEN NEW.kind = 'delivery.park-notice' \
             BEGIN SELECT RAISE(ABORT, 'scripted audit fault'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(notify(&services, &parked).await.is_err());
        assert_eq!(notice_jobs(&pool).await, ids(&[job]));
        assert_eq!(audits(&services).await, 0);
        sqlx::query("DROP TRIGGER fail_park_notice")
            .execute(&pool)
            .await
            .unwrap();
        drop(services);

        let services = boot(&config).await;
        notify(&services, &parked).await.unwrap();
        notify(&services, &parked).await.unwrap();
        assert_eq!(notice_jobs(&pool).await, ids(&[job]));
        assert_eq!(
            audits(&services).await,
            1,
            "the replay repairs the audit once"
        );
    }

    /// FT-003: a cold sender (the send guard's own check refuses an ordinary
    /// `pij send` with E-RS-COLD-WAKE) is never woken. The notice is held as an
    /// FYI, is not claimable, and survives an audit failure and a restart
    /// without a second copy; its next real turn receives it exactly once, and
    /// a replay after the seat warms queues nothing.
    #[tokio::test]
    async fn a_cold_sender_gets_the_notice_once_as_an_fyi_never_a_wake() {
        let store = FreshStore::new();
        let config = config(&store, 1_024);
        let mut services = boot(&config).await;
        services.session_status = Arc::new(cold_sender());
        let pool = pool(&store).await;
        let sender = services
            .registry
            .get(&SENDER.into())
            .await
            .unwrap()
            .unwrap();
        let verdict = crate::http::cold_wake::verdict(&services, &sender, now_ms())
            .await
            .unwrap();
        assert!(
            refusal(&sender.id, &verdict).is_some_and(|text| text.starts_with("E-RS-COLD-WAKE")),
            "the same seat refuses an ordinary send: {verdict:?}"
        );
        let (job, parked) = park(&services, "parked for a cold sender", 0).await;

        // The FYI is held, then its audit write fails.
        sqlx::query(
            "CREATE TRIGGER fail_park_notice BEFORE INSERT ON spine_events \
             WHEN NEW.kind = 'delivery.park-notice' \
             BEGIN SELECT RAISE(ABORT, 'scripted audit fault'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(notify(&services, &parked).await.is_err());
        sqlx::query("DROP TRIGGER fail_park_notice")
            .execute(&pool)
            .await
            .unwrap();
        drop(services);

        // Restart: the replay neither holds nor queues a second copy.
        let mut services = boot(&config).await;
        services.session_status = Arc::new(cold_sender());
        notify(&services, &parked).await.unwrap();
        notify(&services, &parked).await.unwrap();
        assert_eq!(
            notice_fyis(&pool).await,
            vec![(notice_id(job), "pending".to_string())]
        );
        assert!(
            notice_jobs(&pool).await.is_empty(),
            "nothing to claim, nothing woken"
        );
        assert_eq!(
            audits(&services).await,
            1,
            "the replay repairs the audit once"
        );
        let identity = NativeInboxIdentity {
            native_session: sender.harness_session.clone(),
            pid: sender.proc.map(|proc| proc.pid),
            proc_start: sender.proc.map(|proc| proc.proc_start),
        };
        assert!(
            services
                .delivery
                .claim_native_inbox(&sender.id, false, &identity)
                .await
                .unwrap()
                .claims
                .is_empty()
        );

        // Its next real turn receives exactly one copy.
        let (delivered, _) = services
            .delivery
            .claim_fyis(&sender.id, "hook:test")
            .await
            .unwrap();
        assert_eq!(
            delivered
                .iter()
                .map(|fyi| fyi.id.clone())
                .collect::<Vec<_>>(),
            vec![notice_id(job)]
        );
        services.session_status = Arc::new(FakeSessionStatus::new());
        notify(&services, &parked).await.unwrap();
        assert_eq!(
            notice_fyis(&pool).await,
            vec![(notice_id(job), "delivered".to_string())]
        );
        assert!(
            notice_jobs(&pool).await.is_empty(),
            "a warm replay queues no copy"
        );
        assert!(
            services
                .delivery
                .claim_fyis(&sender.id, "hook:test")
                .await
                .unwrap()
                .0
                .is_empty()
        );
    }

    /// A warm sender whose own receiver lease lapsed refuses the wake. The
    /// notice is not lost: it is held under the same id for its next turn,
    /// and a replay neither holds nor queues another copy.
    #[tokio::test(start_paused = true)]
    async fn a_refused_notice_is_held_for_the_senders_next_turn() {
        let clock = hold_clock();
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        let (job, parked) = park(&services, "parked; the sender's receiver lapses", 0).await;
        // Past the boot grace with no heartbeat: the sender's receiver is gone.
        tokio::time::advance(Duration::from_secs(61)).await;
        notify(&services, &parked).await.unwrap();
        notify(&services, &parked).await.unwrap();
        assert_eq!(
            notice_fyis(&pool).await,
            vec![(notice_id(job), "pending".to_string())]
        );
        assert!(notice_jobs(&pool).await.is_empty());
        assert_eq!(audits(&services).await, 1);
        clock.abort();
    }

    /// A sender the cold-wake check would not refuse gets an ordinary,
    /// immediately claimable notice and no FYI.
    #[tokio::test]
    async fn a_warm_sender_can_claim_the_notice_at_once() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        let (job, parked) = park(&services, "parked for a warm sender", 0).await;
        notify(&services, &parked).await.unwrap();
        let sender = services
            .registry
            .get(&SENDER.into())
            .await
            .unwrap()
            .unwrap();
        let identity = NativeInboxIdentity {
            native_session: sender.harness_session.clone(),
            pid: sender.proc.map(|proc| proc.pid),
            proc_start: sender.proc.map(|proc| proc.proc_start),
        };
        let claims = services
            .delivery
            .claim_native_inbox(&sender.id, false, &identity)
            .await
            .unwrap()
            .claims;
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].message.msg_id, notice_id(job));
        assert!(notice_fyis(&pool).await.is_empty());
    }

    /// Plan 164 review (pij-novel-chinchilla): a paired machine may hold an FYI
    /// for this seat under any id, `park-notice-<job>` included. Only a LOCAL
    /// FYI is this daemon's own notice: a peer's must never count as admitted
    /// and suppress the sender's real park notice.
    #[tokio::test]
    async fn a_peer_fyi_with_the_notice_id_never_suppresses_the_real_notice() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        let (job, parked) = park(&services, "parked; a peer squats the notice id", 0).await;
        services
            .delivery
            .hold_fyi(Msg {
                from: SeatId::from("pij-squatter"),
                from_machine: Some("laptop".to_string()),
                to: SENDER.into(),
                body: "not your notice".to_string(),
                msg_id: notice_id(job),
                in_reply_to: None,
                command: None,
            })
            .await
            .unwrap();
        notify(&services, &parked).await.unwrap();
        assert_eq!(
            notice_jobs(&pool).await,
            ids(&[job]),
            "the sender still gets its own notice"
        );
    }

    /// Plan 164: a parked message FORWARDED from a paired machine has no local
    /// sender. Its notice is skipped, never sent back over federation and never
    /// delivered to a local seat that merely shares the remote sender's name.
    #[tokio::test]
    async fn a_parked_forwarded_message_notifies_nobody_here() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        services
            .delivery
            .accept_forwarded(Msg {
                from: SENDER.into(),
                from_machine: Some("laptop".to_string()),
                to: TARGET.into(),
                body: "from afar".to_string(),
                msg_id: "m-afar".to_string(),
                in_reply_to: None,
                command: None,
            })
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
                let (parked, events) = queue
                    .park_delivery(
                        job_id,
                        &TARGET.into(),
                        job.attempt,
                        &evidence,
                        spine.as_ref(),
                    )
                    .await?;
                assert!(parked.is_some());
                Ok((events.clone(), events))
            })
            .await
            .unwrap();
        let parked = events
            .into_iter()
            .find(|event| event.kind == PARKED_KIND)
            .unwrap();
        assert_eq!(parked.seat, None, "attributed to no local seat");
        notify(&services, &parked).await.unwrap();
        assert!(
            notice_jobs(&pool).await.is_empty(),
            "no notice to the local namesake"
        );
        assert!(notice_fyis(&pool).await.is_empty());
    }
}
