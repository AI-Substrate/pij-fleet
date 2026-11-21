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
//! gets nothing, and that is not an error. A message forwarded from a paired
//! machine is parked with no local sender seat (plan 164), so its notice is
//! skipped: never sent back over federation, never given to a local namesake.
//!
//! [`DeliveryService::admitted`]: crate::delivery::DeliveryService::admitted

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::BG_ACTOR;
use pij_core::cold_wake::refusal;
use pij_core::delivery::{
    HOLD_LIMIT_EVENT_KIND, HoldExpiry, RETIRE_PER_SWEEP, RETIRED_RECIPIENT_AFTER_MS,
};
use pij_core::error::Result;
use pij_core::events::EventFilter;
use pij_core::model::{
    DeliveryFailure, DeliveryOutcome, Event, JobId, Msg, Receipt, SeatDescriptor, SeatId, Seq,
};
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
    /// F260: a body held for redelivery until this instant (Unix ms).
    #[serde(rename = "heldUntil", default)]
    held_until: Option<u64>,
}

/// The `delivery.hold-limit` payload (`pij_core::delivery::hold_limit_event`).
#[derive(Deserialize)]
struct HoldLimit {
    #[serde(rename = "messageId")]
    message_id: String,
    #[serde(rename = "jobId")]
    job_id: JobId,
    recipient: String,
    reason: String,
    holds: u32,
}

/// Audit record of a hold-limit notice, keyed by the held message's `msg_id`.
/// Its own kind, so a later park notice for the same message is never read as
/// already sent (plan 184).
pub const HOLD_NOTICE_KIND: &str = "delivery.hold-notice";

/// Audit record of a held body's expiry notice (F260), keyed by its `msg_id`.
pub const EXPIRY_NOTICE_KIND: &str = "delivery.park-expiry-notice";

/// F270: the record that a held park's expiry was decided never owed (its body
/// was recovered or delivered first), keyed by [`expiry_settled_key`]. Without
/// it every sweep made that decision again, under the native lock in a write
/// transaction, until the park left the [`LOOKBACK_MS`] window.
pub const EXPIRY_SETTLED_KIND: &str = "delivery.park-expiry-settled";

/// The key of one park fact's settled expiry: its spine sequence, since one
/// job can be parked (and settled) more than once.
#[must_use]
pub fn expiry_settled_key(park: Seq) -> String {
    format!("park-seq-{}", park.0)
}

/// The stable notice id for one parked job: the queue's dedupe key.
#[must_use]
pub fn notice_id(job_id: JobId) -> String {
    format!("park-notice-{}", job_id.0)
}

/// The stable notice id for one job held at the hold limit.
#[must_use]
pub fn hold_notice_id(job_id: JobId) -> String {
    format!("hold-notice-{}", job_id.0)
}

/// The stable expiry notice id for one held job (F260).
#[must_use]
pub fn expiry_notice_id(job_id: JobId) -> String {
    format!("park-expired-{}", job_id.0)
}

fn hold_body(held: &HoldLimit) -> String {
    format!(
        "[pij] message {} to {} is still held ({}) after {} composer holds. It stays queued for now and fails by name if the hold continues.",
        held.message_id, held.recipient, held.reason, held.holds
    )
}

/// How a notice reaches its sender.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// As `pij send` would, or held as an FYI where that would be refused.
    Send,
    /// Always an FYI for the sender's next turn: news, nothing to do (F260's
    /// held notice; the receipt that said `held` may never reach a model).
    Fyi,
}

/// One notice owed to a sender, whichever fact owes it.
struct Owed {
    message_id: String,
    job_id: JobId,
    recipient: String,
    id: String,
    body: String,
    audit_kind: &'static str,
    channel: Channel,
    /// F260: an expiry notice, owed once the hold ends at this instant (Unix
    /// ms) or the recipient is gone, and only while the body is undelivered.
    expires_at: Option<u64>,
}

impl Owed {
    /// The notices `event` owes, none when it does not decode as one.
    fn from_event(event: &Event) -> Vec<Self> {
        if event.kind == HOLD_LIMIT_EVENT_KIND {
            let Ok(held) = serde_json::from_str::<HoldLimit>(&event.payload) else {
                return Vec::new();
            };
            return vec![Self {
                id: hold_notice_id(held.job_id),
                body: hold_body(&held),
                audit_kind: HOLD_NOTICE_KIND,
                message_id: held.message_id,
                job_id: held.job_id,
                recipient: held.recipient,
                channel: Channel::Send,
                expires_at: None,
            }];
        }
        let Ok(parked) = serde_json::from_str::<Parked>(&event.payload) else {
            return Vec::new();
        };
        let owed = |id, body, audit_kind, channel, expires_at| Self {
            message_id: parked.message_id.clone(),
            job_id: parked.job_id,
            recipient: parked.recipient.clone(),
            id,
            body,
            audit_kind,
            channel,
            expires_at,
        };
        match parked.held_until {
            None => vec![owed(
                notice_id(parked.job_id),
                body(&parked),
                NOTICE_KIND,
                Channel::Send,
                None,
            )],
            Some(until) => vec![
                owed(
                    notice_id(parked.job_id),
                    held_body(&parked, until),
                    NOTICE_KIND,
                    Channel::Fyi,
                    None,
                ),
                owed(
                    expiry_notice_id(parked.job_id),
                    expired_body(&parked),
                    EXPIRY_NOTICE_KIND,
                    Channel::Send,
                    Some(until),
                ),
            ],
        }
    }
}

fn body(parked: &Parked) -> String {
    // F276: a retired recipient does not recover by waiting.
    let next = if parked.outcome == DeliveryFailure::RecipientRetired {
        "Resend it to a live seat, or revive the target first"
    } else {
        "Resend after the target recovers"
    };
    format!(
        "[pij] message {} to {} was not delivered ({}: {}). {next}.",
        parked.message_id,
        parked.recipient,
        parked.outcome.as_str(),
        parked.reason
    )
}

fn held_body(parked: &Parked, until: u64) -> String {
    format!(
        "[pij] message {} to {} is HELD, not refused: its native receiver lapsed. It is delivered once \
         when the receiver renews, or you are told if the hold ends first (at {until}, Unix ms). Do not \
         resend it.",
        parked.message_id, parked.recipient
    )
}

fn expired_body(parked: &Parked) -> String {
    format!(
        "[pij] message {} to {} was NOT delivered: it was held because its native receiver lapsed, \
         and the hold ended before the receiver renewed. It will never be delivered now; resend it \
         if it still matters.",
        parked.message_id, parked.recipient
    )
}

/// Notify the sender of one parked job, at most once per job and notice.
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
    // decode as a park receipt (or hold-limit fact) is not one.
    let owed = Owed::from_event(event);
    if owed.is_empty() {
        return Ok(());
    }
    let Some(seat) = services
        .registry
        .get(sender)
        .await?
        .filter(|seat| seat.tombstoned_at.is_none())
    else {
        return Ok(());
    };
    // F260: a held park's expiry is owed once its hold is over; by then the
    // held FYI ("do not resend") would only contradict it, so it is skipped.
    let over = match owed.iter().find_map(|owed| {
        owed.expires_at
            .map(|until| (owed.recipient.as_str(), until))
    }) {
        Some((recipient, until)) => Some(hold_over(services, recipient, until).await?),
        None => None,
    };
    for owed in owed {
        let skipped = matches!(
            (owed.channel, owed.expires_at, over),
            (Channel::Fyi, None, Some(true)) | (_, Some(_), Some(false))
        );
        if !skipped {
            notify_one(services, event, sender, &seat, owed).await?;
        }
    }
    Ok(())
}

/// F260: is the hold on this body over: past its instant, or its recipient
/// gone (missing, closed or retired), which can never renew it now?
async fn hold_over(services: &Services, recipient: &str, until: u64) -> Result<bool> {
    if crate::http::system_time_ms()? >= until {
        return Ok(true);
    }
    Ok(services
        .registry
        .get(&SeatId::from(recipient))
        .await?
        .is_none_or(|seat| seat.tombstoned_at.is_some()))
}

async fn notify_one(
    services: &Services,
    event: &Event,
    sender: &SeatId,
    seat: &SeatDescriptor,
    owed: Owed,
) -> Result<()> {
    // F270: an expiry this park already settled is final: no reads, no lock.
    let settled_key = owed.expires_at.and(event.seq).map(expiry_settled_key);
    if let Some(key) = &settled_key
        && services
            .event_bus
            .latest_matching_message(sender, EXPIRY_SETTLED_KIND, key)
            .await?
            .is_some()
    {
        return Ok(());
    }
    let id = owed.id.clone();
    let audited = services
        .event_bus
        .latest_matching_message(sender, owed.audit_kind, &owed.message_id)
        .await?
        .and_then(|prior| serde_json::from_str::<serde_json::Value>(&prior.payload).ok())
        .is_some_and(|prior| prior["job_id"] == json!(owed.job_id));
    let admitted = services.delivery.admitted(sender, &id).await?
        || fyi_held(services, sender, &id, event.at).await?;
    let (channel, notice) = if admitted {
        (None, None)
    } else if audited {
        // Audited yet not visible as admitted: its delivered-ledger entry aged
        // out. It was sent; it is never sent again.
        return Ok(());
    } else {
        // F260: ending the hold and deciding the notice is one queue write: a
        // body already back in the queue (or delivered) owes nothing, and one
        // expired here is never recovered after.
        if owed.expires_at.is_some() {
            match services
                .delivery
                .expire_native_hold(owed.job_id, &SeatId::from(owed.recipient.as_str()))
                .await?
            {
                HoldExpiry::Owed => {}
                // A same-id copy is still on its way: decide on a later sweep.
                HoldExpiry::Deferred => return Ok(()),
                HoldExpiry::Settled => {
                    if let Some(key) = settled_key {
                        services
                            .event_bus
                            .publish(Event {
                                seq: None,
                                v: 1,
                                at: event.at,
                                kind: EXPIRY_SETTLED_KIND.into(),
                                seat: Some(sender.clone()),
                                payload: json!({
                                    "msg_id": key,
                                    "message_id": owed.message_id,
                                    "job_id": owed.job_id,
                                    "recipient": owed.recipient,
                                })
                                .to_string(),
                            })
                            .await?;
                    }
                    return Ok(());
                }
            }
        }
        let msg = Msg {
            from: SeatId::from(BG_ACTOR),
            to: sender.clone(),
            body: owed.body.clone(),
            msg_id: id.clone(),
            from_machine: None,
            in_reply_to: None,
            command: None,
        };
        let (channel, receipt) = match owed.channel {
            Channel::Send => send_or_hold(services, seat, msg).await?,
            Channel::Fyi => ("fyi", services.delivery.hold_fyi(msg).await?),
        };
        (Some(channel), Some(receipt))
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
            kind: owed.audit_kind.into(),
            seat: Some(sender.clone()),
            payload: json!({
                "msg_id": owed.message_id,
                "job_id": owed.job_id,
                "notice_msg_id": id,
                "recipient": owed.recipient,
                "channel": channel,
                "notice": notice,
            })
            .to_string(),
        })
        .await?;
    Ok(())
}

/// Deliver a daemon notice from `pij-bg` the way `pij send` would, or hold it
/// as an FYI for the recipient's next turn where a send would be refused.
///
/// Shared by every daemon-originated notice (the park notice here, the
/// double-seat notice of plan 174) so they obey one cold-wake decision. Returns
/// the channel taken (`send`, `fyi` or `fyi-after-refusal`) with its receipt.
///
/// # Errors
/// Cold-wake, delivery or FYI-hold failures.
pub(crate) async fn send_or_hold(
    services: &Services,
    seat: &SeatDescriptor,
    msg: Msg,
) -> Result<(&'static str, Receipt)> {
    // The very decision `pij send` obeys, stale-working correction included.
    let now = crate::http::system_time_ms()?;
    let cold = crate::http::cold_wake::verdict(services, seat, now).await?;
    if refusal(&seat.id, &cold).is_some() {
        return Ok(("fyi", services.delivery.hold_fyi(msg).await?));
    }
    // The recipient's own receiver is down: a body would be held (F260) and
    // wake it on renewal. A notice never waits for a wake: it is held for the
    // recipient's next turn under the same id instead (background B9/B10).
    if services.delivery.native_receiver_lapsed(seat).await {
        return Ok(("fyi-after-refusal", services.delivery.hold_fyi(msg).await?));
    }
    let receipt = services.delivery.accept(msg.clone()).await?;
    if matches!(receipt.outcome, DeliveryOutcome::Refused { .. }) {
        // A refusal (e.g. the recipient's holds are full) is final for the
        // wake, never for the notice: the same FYI fallback.
        return Ok(("fyi-after-refusal", services.delivery.hold_fyi(msg).await?));
    }
    Ok(("send", receipt))
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
/// FYI row is unique by (origin, id), so this stays true after the FYI is
/// delivered.
///
/// Only a LOCAL FYI is this daemon's notice: identity is (origin, msg_id), and
/// a paired machine may hold an FYI under any id, `park-notice-<job>`
/// included (plan 164). A forwarded FYI's `fyi.held` names its `from_machine`.
async fn fyi_held(services: &Services, seat: &SeatId, id: &str, since_at: u64) -> Result<bool> {
    let mut pages = Pages::new(seat, FYI_HELD_KIND, since_at)?;
    while let Some(page) = pages.next(services).await? {
        if page.iter().any(|event| {
            serde_json::from_str::<serde_json::Value>(&event.payload).is_ok_and(|payload| {
                payload["id"] == id && payload.get("from_machine").is_none_or(|m| m.is_null())
            })
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

/// F276: fail, at most [`RETIRE_PER_SWEEP`] at a time, the open queue jobs of
/// seats tombstoned more than [`RETIRED_RECIPIENT_AFTER_MS`] ago that no
/// revive is working on, as `undelivered:recipient-retired`. Nothing else
/// ever ends them: the extension or harness that would claim them is gone.
/// Their `delivery.parked` facts reach this follower like any park, and
/// [`notify`] tells a sender only while it is a live seat.
///
/// The POLICY is that the recipient is tombstoned. The BRAKES only spare:
/// a tombstone no older than the cutoff (removing it retires the same jobs
/// sooner), a revive in progress (removing it retires a reviving seat's
/// jobs), and any failed read (the roster here, or the store's own re-read
/// of the tombstone in the retiring transaction): nothing retires on a read
/// that did not succeed.
async fn retire_long_dead(services: &Services, seats: &[SeatDescriptor], now: u64) -> bool {
    let cutoff = now.saturating_sub(RETIRED_RECIPIENT_AFTER_MS);
    let reviving = services.spawn_waits.revives();
    let retired: Vec<SeatId> = seats
        .iter()
        .filter(|seat| seat.tombstoned_at.is_some_and(|at| at < cutoff))
        .filter(|seat| !reviving.contains(&seat.id))
        .map(|seat| seat.id.clone())
        .collect();
    if retired.is_empty() {
        return true;
    }
    let queue = services.queue.clone();
    let spine = services.event_bus.raw_spine();
    let committed = services
        .event_bus
        .publish_committed_batch(async move {
            let events = queue
                .retire_recipient_jobs(&retired, cutoff, RETIRE_PER_SWEEP, now, spine.as_ref())
                .await?;
            Ok((events, ()))
        })
        .await;
    match committed {
        Ok(()) => true,
        Err(error) => {
            eprintln!("pij-rs park notice sweep: retiring long-dead seats' jobs: {error}");
            false
        }
    }
}

/// Notify every live seat's parks from the last [`LOOKBACK_MS`], a page at a
/// time, after retiring long-dead seats' queued jobs (F276). The roster is the
/// live seats plus the retired seats the queue in use still owes a row
/// (F259a, [`crate::pointer::live_or_owed`]), so a retired seat with nothing
/// open costs this sweep nothing. Returns whether everything settled.
async fn sweep(services: &Services) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    let since = now.saturating_sub(LOOKBACK_MS);
    let seats =
        match crate::pointer::live_or_owed(services.registry.as_ref(), services.queue.as_ref())
            .await
        {
            Ok(seats) => seats,
            Err(error) => {
                eprintln!("pij-rs park notice sweep: {error}");
                return false;
            }
        };
    let mut settled = retire_long_dead(services, &seats, now).await;
    let owing = seats
        .iter()
        .filter(|seat| seat.tombstoned_at.is_none())
        .flat_map(|seat| [PARKED_KIND, HOLD_LIMIT_EVENT_KIND].map(|kind| (seat, kind)));
    for (seat, kind) in owing {
        let mut pages = match Pages::new(&seat.id, kind, since) {
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
        .subscribe_live(EventFilter::kinds([PARKED_KIND, HOLD_LIMIT_EVENT_KIND]));
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

    /// Send SENDER -> TARGET and park it terminally (an operator release, whose
    /// sender is told to resend; F260's held native parks are tested with the
    /// native receiver), committing `fillers` unrelated events in the same
    /// batch after the park (they overrun a one-slot live channel).
    async fn park(services: &Services, label: &str, fillers: usize) -> (JobId, Event) {
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), label)
            .await
            .unwrap();
        let (job_id, job) = services
            .queue
            .claim(&[delivery_kind(&TARGET.into())], TARGET)
            .await
            .unwrap()
            .unwrap();
        let queue = services.queue.clone();
        let spine = services.event_bus.raw_spine();
        let events = services
            .event_bus
            .publish_committed_batch(async move {
                let evidence = ParkingEvidence {
                    outcome: DeliveryFailure::OperatorReleased,
                    reason: "operator released the head",
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
            .heartbeat_native_receiver(&sender.id, &identity, 1, 1, None)
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

    /// Plan 184: a delivery held at the hold limit tells its sender once, under
    /// its own id and audit kind, so the same message parking later is still
    /// told, and neither notice repeats on replay.
    #[tokio::test]
    async fn a_hold_limit_notice_is_sent_once_and_never_hides_a_later_park_notice() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        let (job, parked) = park(&services, "held, then parked", 0).await;
        let payload: String = sqlx::query_scalar("SELECT payload FROM jobs WHERE id = ?1")
            .bind(job.0 as i64)
            .fetch_one(&pool)
            .await
            .unwrap();
        let held = pij_core::delivery::hold_limit_event(
            job,
            &pij_core::model::Job {
                kind: delivery_kind(&TARGET.into()),
                serial_key: TARGET.into(),
                payload,
                dedupe_key: String::new(),
                dedupe_origin: None,
                attempt: 11,
            },
            "composer-busy",
            12,
            now_ms(),
        )
        .unwrap();
        assert_eq!(held.seat.as_ref().map(SeatId::as_str), Some(SENDER));

        for event in [&held, &parked, &held, &parked] {
            notify(&services, event).await.unwrap();
        }

        let mut expected = vec![hold_notice_id(job), notice_id(job)];
        expected.sort();
        assert_eq!(notice_jobs(&pool).await, expected);
        let body: String = sqlx::query_scalar(
            "SELECT json_extract(payload, '$.body') FROM jobs WHERE dedupe_key = ?1",
        )
        .bind(hold_notice_id(job))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            body.contains("is still held (composer-busy) after 12 composer holds"),
            "{body}"
        );
    }

    /// Plan 184: the follower serves hold-limit facts live, not only by hand.
    #[tokio::test]
    async fn the_follower_notifies_a_hold_limit_as_it_is_published() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        let follower = tokio::spawn(follow(services.clone()));
        let receipt = services
            .delivery
            .send(SENDER.into(), TARGET.into(), "held at the limit")
            .await
            .unwrap();
        let (job, queued) = services
            .queue
            .peek(&[delivery_kind(&TARGET.into())])
            .await
            .unwrap()
            .unwrap();
        assert!(queued.payload.contains(&receipt.msg_id));
        let held =
            pij_core::delivery::hold_limit_event(job, &queued, "composer-busy", 12, now_ms())
                .unwrap();
        services.event_bus.publish(held).await.unwrap();

        let want = vec![hold_notice_id(job)];
        until(async || notice_jobs(&pool).await == want).await;
        assert_eq!(notice_jobs(&pool).await, want);
        follower.abort();
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

    /// F270 (a): another local seat that sends a message under the notice's
    /// id first (`park-notice-<job>` is predictable) sends its own message. It
    /// never makes pij-bg's real notice look already admitted.
    #[tokio::test]
    async fn another_seats_message_with_the_notice_id_never_suppresses_the_real_notice() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let pool = pool(&store).await;
        let (job, parked) = park(&services, "parked; a seat squats the notice id", 0).await;
        services
            .delivery
            .accept(Msg {
                from: TARGET.into(),
                from_machine: None,
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

    /// F270 (c), D-F260-4: a held body recovered before its hold ends settles
    /// its expiry once. A later sweep of that park decides nothing again: it
    /// takes no native lock (so no queue write behind it) and tells nobody.
    #[tokio::test]
    async fn a_recovered_hold_settles_its_expiry_once() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        let receipt = services
            .delivery
            .send(SENDER.into(), TARGET.into(), "held, then recovered")
            .await
            .unwrap();
        assert!(matches!(receipt.outcome, DeliveryOutcome::Queued { .. }));
        let (job_id, job) = services
            .queue
            .peek(&[delivery_kind(&TARGET.into())])
            .await
            .unwrap()
            .expect("queued");
        let queue = services.queue.clone();
        let spine = services.event_bus.raw_spine();
        let events = services
            .event_bus
            .publish_committed_batch(async move {
                let evidence = ParkingEvidence {
                    outcome: DeliveryFailure::NativeReceiverUnavailable,
                    reason: "native receiver lapsed",
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
        // The receiver renews before the hold ends: the body is queued again.
        assert_eq!(
            services
                .queue
                .recover_held_native_deliveries(&TARGET.into())
                .await
                .unwrap()
                .len(),
            1
        );
        // Then the hold's instant passes, and the sweep reads the park.
        let mut payload: serde_json::Value = serde_json::from_str(&parked.payload).unwrap();
        payload["heldUntil"] = 1.into();
        let over = Event {
            payload: payload.to_string(),
            ..parked
        };
        notify(&services, &over).await.unwrap();
        // Every later sweep: the native lock is busy, yet the park is done at once.
        let lock = services.delivery.native_lock();
        let _busy = lock.lock().await;
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(10), notify(&services, &over))
                .await
                .expect("a settled expiry is never decided again")
                .unwrap();
        }
        let pool = pool(&store).await;
        assert!(notice_jobs(&pool).await.is_empty(), "nobody is told");
        assert!(notice_fyis(&pool).await.is_empty(), "nobody is told");
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

    const DEAD_SENDER: &str = "pij-park-dead-sender";
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

    /// TARGET retired `days` ago: tombstoned, then its tombstone aged.
    async fn retire_target(services: &Services, store: &FreshStore, days: u64) {
        services
            .registry
            .tombstone(&TARGET.into(), "test: retired")
            .await
            .unwrap();
        sqlx::query("UPDATE seats SET tombstoned_at = ?2 WHERE id = ?1")
            .bind(TARGET)
            .bind((now_ms() - days * DAY_MS) as i64)
            .execute(&pool(store).await)
            .await
            .unwrap();
    }

    /// TARGET's delivery jobs, oldest first, as `(sender, state, outcome)`.
    async fn target_jobs(store: &FreshStore) -> Vec<(String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT json_extract(payload, '$.from'), state, outcome FROM jobs \
             WHERE kind = 'delivery:' || ?1 ORDER BY id",
        )
        .bind(TARGET)
        .fetch_all(&pool(store).await)
        .await
        .unwrap()
    }

    fn retired(sender: &str) -> (String, String, Option<String>) {
        (
            sender.to_string(),
            "failed".to_string(),
            Some(DeliveryFailure::RecipientRetired.as_str().to_string()),
        )
    }

    fn open(sender: &str) -> (String, String, Option<String>) {
        (sender.to_string(), "pending".to_string(), None)
    }

    async fn notices_to(services: &Services, seat: &str) -> usize {
        services
            .event_bus
            .matching_since(
                &seat.into(),
                &SpineWindow::new(NOTICE_KIND, 0, Seq(0), SpineWindow::MAX_LIMIT).unwrap(),
            )
            .await
            .unwrap()
            .len()
    }

    /// F276: a recipient retired more than seven days ago has its queued jobs
    /// failed as `undelivered:recipient-retired`. A live sender is told,
    /// through the park-notice path; a dead sender is not.
    #[tokio::test]
    async fn a_long_dead_recipients_jobs_are_retired_and_only_a_live_sender_is_told() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        services
            .registry
            .put(SeatDescriptor::new(DEAD_SENDER, Harness::Omp, "/abs/tree"))
            .await
            .unwrap();
        // The dead sender's job is the older, so its park is followed first:
        // once the live sender's notice is visible, the dead one is decided.
        for sender in [DEAD_SENDER, SENDER] {
            services
                .delivery
                .send(
                    sender.into(),
                    TARGET.into(),
                    "for a seat that never returns",
                )
                .await
                .unwrap();
        }
        services
            .registry
            .tombstone(&DEAD_SENDER.into(), "test: sender gone")
            .await
            .unwrap();
        retire_target(&services, &store, 8).await;

        assert!(sweep(&services).await);
        assert_eq!(
            target_jobs(&store).await,
            [retired(DEAD_SENDER), retired(SENDER)]
        );
        assert!(until(async || notices_to(&services, SENDER).await == 1).await);
        // The live follower hands every published park to `notify`, the dead
        // sender's too: it must tell no one.
        let dead_parks = services
            .event_bus
            .matching_since(
                &DEAD_SENDER.into(),
                &SpineWindow::new(PARKED_KIND, 0, Seq(0), SpineWindow::MAX_LIMIT).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dead_parks.len(), 1, "the dead sender's job was parked");
        for park in &dead_parks {
            notify(&services, park).await.unwrap();
        }
        assert_eq!(notices_to(&services, DEAD_SENDER).await, 0);
        let toward_dead: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM jobs WHERE serial_key = ?1) \
                  + (SELECT count(*) FROM fyis WHERE recipient = ?1)",
        )
        .bind(DEAD_SENDER)
        .fetch_one(&pool(&store).await)
        .await
        .unwrap();
        assert_eq!(toward_dead, 0, "nothing is sent or held for a dead sender");
        // A later sweep retires nothing again and tells no one twice.
        assert!(sweep(&services).await);
        assert_eq!(notices_to(&services, SENDER).await, 1);
        assert_eq!(notices_to(&services, DEAD_SENDER).await, 0);
    }

    /// F276 brake: a tombstone seven days old or younger keeps every job, for
    /// a revive (and F193's drain) to deliver.
    #[tokio::test]
    async fn a_recipient_dead_seven_days_or_less_keeps_its_jobs() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), "wait for the revive")
            .await
            .unwrap();
        retire_target(&services, &store, 6).await;
        assert!(sweep(&services).await);
        assert_eq!(target_jobs(&store).await, [open(SENDER)]);
        assert_eq!(notices_to(&services, SENDER).await, 0);
    }

    /// F276 brake: a seat a revive request is working on keeps its jobs, even
    /// past the seven days; once the request ends, the next sweep retires them.
    #[tokio::test]
    async fn a_seat_being_revived_keeps_its_jobs() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), "the revive takes it")
            .await
            .unwrap();
        retire_target(&services, &store, 30).await;
        let reviving = services.spawn_waits.reviving(&TARGET.into());
        assert!(sweep(&services).await);
        assert_eq!(target_jobs(&store).await, [open(SENDER)]);
        drop(reviving);
        assert!(sweep(&services).await);
        assert_eq!(target_jobs(&store).await, [retired(SENDER)]);
    }

    /// F276 brake: the store reads the tombstone again when it retires. A
    /// revive that lifted it after the sweep read the roster keeps every job.
    #[tokio::test]
    async fn a_revive_after_the_roster_read_keeps_the_jobs() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 1_024)).await;
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), "revived meanwhile")
            .await
            .unwrap();
        retire_target(&services, &store, 30).await;
        let roster = services.registry.list(Default::default()).await.unwrap();
        sqlx::query("UPDATE seats SET tombstoned_at = NULL WHERE id = ?1")
            .bind(TARGET)
            .execute(&pool(&store).await)
            .await
            .unwrap();
        assert!(retire_long_dead(&services, &roster, now_ms()).await);
        assert_eq!(target_jobs(&store).await, [open(SENDER)]);
    }

    /// The roster as the store has it, except that listing it fails.
    struct ListFails(Arc<dyn pij_core::ports::Registry>);

    #[async_trait::async_trait]
    impl pij_core::ports::Registry for ListFails {
        async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
            self.0.get(seat).await
        }
        async fn put(&self, descriptor: SeatDescriptor) -> Result<Seq> {
            self.0.put(descriptor).await
        }
        async fn put_reporting(
            &self,
            descriptor: SeatDescriptor,
        ) -> Result<(Seq, pij_core::ports::PutBinding)> {
            self.0.put_reporting(descriptor).await
        }
        async fn put_reporting_keeping_parent(
            &self,
            descriptor: SeatDescriptor,
        ) -> Result<(Seq, pij_core::ports::PutBinding)> {
            self.0.put_reporting_keeping_parent(descriptor).await
        }
        async fn list(&self, _filter: pij_core::ports::SeatFilter) -> Result<Vec<SeatDescriptor>> {
            Err(pij_core::error::PijError::Adapter {
                adapter: "test/registry".into(),
                message: "the roster cannot be read".into(),
            })
        }
        async fn list_live_with(&self, _also: &[SeatId]) -> Result<Vec<SeatDescriptor>> {
            self.list(pij_core::ports::SeatFilter::default()).await
        }
        async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<Seq> {
            self.0.tombstone(seat, reason).await
        }
        async fn tombstone_if_unchanged(
            &self,
            expected: SeatDescriptor,
            reason: String,
        ) -> Result<Seq> {
            self.0.tombstone_if_unchanged(expected, reason).await
        }
        async fn set_activity(
            &self,
            seat: &SeatId,
            state: pij_core::model::SystemState,
            reason: Option<&str>,
        ) -> Result<Option<Seq>> {
            self.0.set_activity(seat, state, reason).await
        }
    }

    /// F276 brake: a roster that cannot be read retires nothing.
    #[tokio::test]
    async fn a_failed_roster_read_retires_nothing() {
        let store = FreshStore::new();
        let mut services = boot(&config(&store, 1_024)).await;
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), "not on a failed read")
            .await
            .unwrap();
        retire_target(&services, &store, 30).await;
        services.registry = Arc::new(ListFails(services.registry.clone()));
        assert!(!sweep(&services).await);
        assert_eq!(target_jobs(&store).await, [open(SENDER)]);
    }

    /// F276: a pass retires at most RETIRE_PER_SWEEP jobs, oldest first; the
    /// next pass takes the rest.
    #[tokio::test]
    async fn a_backlog_is_retired_a_bounded_batch_per_sweep() {
        let store = FreshStore::new();
        let services = boot(&config(&store, 4_096)).await;
        for n in 0..=RETIRE_PER_SWEEP {
            services
                .delivery
                .send(SENDER.into(), TARGET.into(), format!("backlog {n}"))
                .await
                .unwrap();
        }
        retire_target(&services, &store, 30).await;
        assert!(sweep(&services).await);
        let jobs = target_jobs(&store).await;
        assert_eq!(jobs.len(), RETIRE_PER_SWEEP + 1);
        assert!(jobs[..RETIRE_PER_SWEEP].iter().all(|job| job.1 == "failed"));
        assert_eq!(
            jobs[RETIRE_PER_SWEEP],
            open(SENDER),
            "the newest waits a sweep"
        );
        assert!(sweep(&services).await);
        assert!(
            target_jobs(&store)
                .await
                .iter()
                .all(|job| job.1 == "failed")
        );
    }

    /// F276 / B331-01: retirement finds a long-dead seat's work in the queue
    /// actually in use. Here that is a fake queue beside the real registry and
    /// spine, so the registry's own `jobs` table holds nothing for it.
    #[tokio::test]
    async fn a_long_dead_recipients_jobs_in_a_fake_queue_beside_a_real_registry_are_retired() {
        let store = FreshStore::new();
        let mut config = config(&store, 1_024);
        config.adapters.queue = AdapterChoice::Fake;
        let services = boot(&config).await;
        services
            .delivery
            .send(SENDER.into(), TARGET.into(), "owed in the fake queue")
            .await
            .unwrap();
        retire_target(&services, &store, 8).await;
        assert!(sweep(&services).await);
        assert!(
            services
                .queue
                .peek(&[delivery_kind(&TARGET.into())])
                .await
                .unwrap()
                .is_none(),
            "the fake queue's open row is retired"
        );
        assert!(until(async || notices_to(&services, SENDER).await == 1).await);
    }
}
