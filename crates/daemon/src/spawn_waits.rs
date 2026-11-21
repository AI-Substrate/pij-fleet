//! F211: a launched spawn's seat always reaches someone.
//!
//! A spawn launches a pane, writes the seat's pre-bind row and waits for the
//! seat to register. The harness can come up and register whatever the spawn
//! request does next: answer with a failure, lose its caller, or die with a
//! restarting daemon (pij-sure-eagle sat 22 h after a restart cut its spawn's
//! answer). This keeps two independent facts per launch:
//!
//! - **bound**: the seat registered with its process;
//! - **answer**: a request is still watching, or the caller was answered with
//!   the seat (settled), or the caller was NOT answered with it (a failure
//!   that said the seat was not bound, a request that ended unanswered, a
//!   daemon restart).
//!
//! Whichever of "bound" and "not answered" comes second hands the seat to its
//! parent, exactly once: a notice and a `spawn.reported` event ([`Settle`]).
//! Answered with the seat, the launch is forgotten.
//!
//! Durable evidence survives a restart: `spawn.launched` when the watch
//! starts, `spawn.answered` when an answer naming the seat is built, and
//! `spawn.reported` when the parent's notice is admitted. At boot a live
//! spawned row with `spawn.launched` and neither of the others was never
//! settled, bound or not; a historical row without `spawn.launched` is
//! ignored.
//!
//! F255: a caller that lost its answer sends the same spawn again, keyed by
//! the same spawn id, and that request watches the launch in its place
//! ([`SpawnWaits::watch_again`]), keeping any binding already known. Only the
//! latest watcher settles it, so an earlier request ending later never counts,
//! and a launch already settled with a bound seat is never re-armed (R309-03),
//! even by a resend whose row read predates the binding: settled launches are
//! remembered, the newest [`SETTLED_KEPT`] of them (F264; a BRAKE: it only
//! ever withholds a watch). Once the daemon is stopping
//! ([`SpawnWaits::stop`]), no spawn starts, and the stop waits for each launch
//! already past its check to write its row.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::directory::shell_word;
use pij_core::model::{Event, SeatDescriptor, SeatId};
use pij_core::ports::{Registry, SeatFilter, Spine};
use pij_core::wire;

use crate::delivery::DeliveryService;
use crate::events::EventBus;

/// Why, when the daemon restarted before the spawn answered.
pub const RESTARTED: &str =
    "the daemon restarted while the spawn waited, so its caller never heard of this seat";

/// Why, when the spawn request ended before it answered.
pub const ABANDONED: &str = "the spawn request ended before it answered (its caller went away)";

/// The spine kind written when a watch starts.
pub const LAUNCHED: &str = "spawn.launched";
/// The spine kind written when an answer naming the seat is built.
pub const ANSWERED: &str = "spawn.answered";
/// The spine kind written when the parent's notice is admitted.
pub const REPORTED: &str = "spawn.reported";

/// One launch, as its spawn recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The daemon-minted spawn id.
    pub spawn_id: String,
    /// The seat the spawn allocated.
    pub seat: SeatId,
    /// The pane it launched.
    pub pane: Option<String>,
    /// Who spawned it.
    pub parent: Option<SeatId>,
}

impl Launch {
    /// The launch a spawned row records.
    pub fn of(seat: &SeatDescriptor) -> Option<Self> {
        Some(Self {
            spawn_id: seat.spawn_id.clone().filter(|id| !id.is_empty())?,
            seat: seat.id.clone(),
            pane: seat.pane.clone(),
            parent: seat.parent.clone(),
        })
    }

    /// How the parent ends a seat its spawn left running: the close retires
    /// the seat and the pane kill stops its process. Each word is quoted for
    /// a POSIX shell.
    pub fn close_command(&self) -> String {
        close_command(&self.seat, self.pane.as_deref())
    }
}

/// [`Launch::close_command`] for a seat and its pane.
pub fn close_command(seat: &SeatId, pane: Option<&str>) -> String {
    let close = format!("pij close {}", shell_word(seat.as_str()));
    match pane {
        Some(pane) => format!("{close}; tmux kill-pane -t {}", shell_word(pane)),
        None => close,
    }
}

/// Where a settlement goes: the spine record and the parent's notice.
#[async_trait]
pub trait Settle: Send + Sync {
    /// The caller was answered with the seat (`outcome`: `bound`,
    /// `dispatched` or `failed-bound`): record `spawn.answered`.
    async fn answered(&self, launch: &Launch, outcome: &'static str);
    /// The seat is bound and its caller was not answered with it: tell the
    /// parent and record `spawn.reported`.
    async fn report(&self, launch: &Launch, reason: &str);
}

#[derive(Debug)]
struct Entry {
    launch: Launch,
    bound: bool,
    /// `None` while a spawn request watches; why, once it is not answered.
    unanswered: Option<String>,
    /// The request watching it, or 0 when none does (recovered at boot).
    watcher: u64,
}

/// How many settled launches [`SpawnWaits`] remembers (F264). A resend reads
/// its launch's row before it watches again, under the spawn lock, and only a
/// registration can settle a launch in between (it binds and reports without
/// that lock): a few at most in milliseconds, so the oldest can go.
pub const SETTLED_KEPT: usize = 256;

/// F264: launches whose hand-off is settled, newest last, so a resend holding
/// a stale row never re-arms one. Bounded by [`SETTLED_KEPT`] (the oldest is
/// forgotten first), and gone with the daemon: at boot the spine's records
/// stand in.
#[derive(Debug, Default)]
struct Settled {
    order: std::collections::VecDeque<String>,
    /// Each settled launch, and whether its seat is known to be bound.
    launches: HashMap<String, (Launch, bool)>,
}

impl Settled {
    fn record(&mut self, launch: Launch, bound: bool) {
        let spawn_id = launch.spawn_id.clone();
        if let Some(known) = self.launches.get_mut(&spawn_id) {
            *known = (launch, bound || known.1);
            return;
        }
        self.launches.insert(spawn_id.clone(), (launch, bound));
        self.order.push_back(spawn_id);
        if self.order.len() > SETTLED_KEPT
            && let Some(oldest) = self.order.pop_front()
        {
            self.launches.remove(&oldest);
        }
    }
}

/// Launches whose hand-off is not settled.
pub struct SpawnWaits {
    entries: Mutex<HashMap<String, Entry>>,
    /// Locked after `entries` whenever both are held.
    settled: Mutex<Settled>,
    settle: Arc<dyn Settle>,
    next_watcher: AtomicU64,
    stopping: AtomicBool,
    /// R309-01: held from a spawn's classification through its row write, by
    /// the task that owns that segment (never by the HTTP request waiting on
    /// it), so a resend and the daemon's stop both wait for the row.
    launches: Arc<tokio::sync::Mutex<()>>,
    /// F264: what the holder of `launches` is launching, for a stop that
    /// gives up waiting on it.
    launching: Mutex<Option<String>>,
}

impl SpawnWaits {
    /// Settlements go to `settle`.
    pub fn new(settle: Arc<dyn Settle>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            settled: Mutex::new(Settled::default()),
            settle,
            next_watcher: AtomicU64::new(1),
            stopping: AtomicBool::new(false),
            launches: Arc::new(tokio::sync::Mutex::new(())),
            launching: Mutex::new(None),
        }
    }

    /// Take the spawn lock, held until a launch's row is written. Waits for
    /// any launch holding it.
    pub async fn hold(self: &Arc<Self>) -> SpawnHold {
        SpawnHold {
            _guard: Arc::clone(&self.launches).lock_owned().await,
            waits: Arc::clone(self),
        }
    }

    /// What the holder of the spawn lock says it launches, while it holds it
    /// (F264); `None` when nobody does, or the holder has not said yet.
    pub fn launching(&self) -> Option<String> {
        self.launching
            .lock()
            .expect("spawn waits launching")
            .clone()
    }

    /// The daemon is stopping (F255): no spawn starts from now on, and this
    /// returns once every launch already past its check has written its row
    /// (R309-01: it takes the launch lock, which each such launch holds until
    /// then). Both are BRAKES: the flag only refuses, and the join only delays
    /// the stop; the caller bounds the wait.
    pub async fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        drop(self.launches.lock().await);
    }

    /// Whether [`Self::stop`] was called.
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// A fresh launch: its spawn request now watches it. Settle the guard;
    /// dropping it unsettled counts as [`ABANDONED`].
    pub fn watch(self: &Arc<Self>, launch: Launch) -> WaitGuard {
        let spawn_id = launch.spawn_id.clone();
        let watcher = self.next_watcher.fetch_add(1, Ordering::Relaxed);
        self.entries.lock().expect("spawn waits").insert(
            spawn_id.clone(),
            Entry {
                launch,
                bound: false,
                unanswered: None,
                watcher,
            },
        );
        self.guard(spawn_id, watcher)
    }

    /// F255: the same spawn sent again; `bound` is what its row said when the
    /// resend read it, which a registration (it binds and reports without
    /// the spawn lock) can have overtaken since.
    /// - Still unsettled (a request or a recovery holds it): this request
    ///   watches it in place of the earlier one, keeping a binding either knew.
    /// - Already settled and bound (its caller was answered with the seat, or
    ///   its parent was told, R309-03), by its row or by this daemon's record
    ///   of it (F264, whatever the row said): never re-armed. The guard
    ///   settles nothing, so this answer, or its loss, can never report the
    ///   seat again.
    /// - Settled unbound (a `--no-wait` dispatch whose answer was lost, its
    ///   seat not bound since): it is watched again as a fresh launch.
    pub fn watch_again(self: &Arc<Self>, launch: Launch, bound: bool) -> WaitGuard {
        let spawn_id = launch.spawn_id.clone();
        let watcher = self.next_watcher.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.lock().expect("spawn waits");
        let known = entries.get(&spawn_id).map(|entry| entry.bound);
        let settled_bound = known.is_none()
            && self
                .settled
                .lock()
                .expect("spawn waits settled")
                .launches
                .get(&spawn_id)
                .is_some_and(|(_, bound)| *bound);
        if known.is_some() || !(bound || settled_bound) {
            entries.insert(
                spawn_id.clone(),
                Entry {
                    launch,
                    bound: bound || known == Some(true),
                    unanswered: None,
                    watcher,
                },
            );
        }
        drop(entries);
        self.guard(spawn_id, watcher)
    }

    fn guard(self: &Arc<Self>, spawn_id: String, watcher: u64) -> WaitGuard {
        WaitGuard {
            waits: Arc::clone(self),
            spawn_id,
            watcher,
            settled: false,
        }
    }

    /// Boot: a launch the spine shows was never settled. Its caller died
    /// with the previous daemon; a seat already bound is reported now.
    pub fn recover(&self, launch: Launch, bound: bool) {
        let spawn_id = launch.spawn_id.clone();
        self.entries.lock().expect("spawn waits").insert(
            spawn_id.clone(),
            Entry {
                launch,
                bound,
                unanswered: None,
                watcher: 0,
            },
        );
        self.not_answered(&spawn_id, None, RESTARTED);
    }

    /// A registration bound `seat` to its process. Only the launch's own seat
    /// on its own pane counts. A settled launch's binding is remembered too
    /// (F264: a `--no-wait` dispatch that binds after its answer).
    pub fn bound(&self, spawn_id: &str, seat: &SeatId, pane: Option<&str>) {
        let report = {
            let mut entries = self.entries.lock().expect("spawn waits");
            let Some(entry) = entries.get_mut(spawn_id) else {
                let mut settled = self.settled.lock().expect("spawn waits settled");
                if let Some((launch, bound)) = settled.launches.get_mut(spawn_id)
                    && launch.seat == *seat
                    && launch.pane.as_deref() == pane
                {
                    *bound = true;
                }
                return;
            };
            if entry.launch.seat != *seat || entry.launch.pane.as_deref() != pane {
                return;
            }
            entry.bound = true;
            match entry.unanswered.clone() {
                Some(reason) => self
                    .settle_entry(&mut entries, spawn_id, true)
                    .map(|launch| (launch, reason)),
                None => None,
            }
        };
        if let Some((launch, reason)) = report {
            self.hand_off(launch, reason);
        }
    }

    /// `watcher`: only while that request still watches; `None` for boot.
    fn not_answered(&self, spawn_id: &str, watcher: Option<u64>, reason: &str) {
        let report = {
            let mut entries = self.entries.lock().expect("spawn waits");
            let Some(entry) = entries.get_mut(spawn_id) else {
                return;
            };
            if watcher.is_some_and(|watcher| watcher != entry.watcher) {
                return;
            }
            if entry.bound {
                self.settle_entry(&mut entries, spawn_id, true)
                    .map(|launch| (launch, reason.to_string()))
            } else {
                entry.unanswered = Some(reason.to_string());
                None
            }
        };
        if let Some((launch, reason)) = report {
            self.hand_off(launch, reason);
        }
    }

    /// F264: remove a settled launch's entry and record it, both under the
    /// entries lock, so a resend never finds it in neither.
    fn settle_entry(
        &self,
        entries: &mut HashMap<String, Entry>,
        spawn_id: &str,
        bound: bool,
    ) -> Option<Launch> {
        let entry = entries.remove(spawn_id)?;
        self.settled
            .lock()
            .expect("spawn waits settled")
            .record(entry.launch.clone(), bound || entry.bound);
        Some(entry.launch)
    }

    /// The launch's entry, removed, while `watcher` still watches it.
    fn take(&self, spawn_id: &str, watcher: u64) -> Option<Entry> {
        let mut entries = self.entries.lock().expect("spawn waits");
        if entries.get(spawn_id)?.watcher != watcher {
            return None;
        }
        entries.remove(spawn_id)
    }

    fn answered(&self, spawn_id: &str, watcher: u64, outcome: &'static str) {
        let launch = {
            let mut entries = self.entries.lock().expect("spawn waits");
            if entries
                .get(spawn_id)
                .is_none_or(|entry| entry.watcher != watcher)
            {
                return;
            }
            // Answered with a bound seat unless it was only dispatched.
            self.settle_entry(&mut entries, spawn_id, outcome != "dispatched")
        };
        let Some(launch) = launch else {
            return;
        };
        let settle = Arc::clone(&self.settle);
        // Detached: the answer is built; recording it must not be cancelled
        // with the request.
        tokio::spawn(async move { settle.answered(&launch, outcome).await });
    }

    fn hand_off(&self, launch: Launch, reason: String) {
        let settle = Arc::clone(&self.settle);
        tokio::spawn(async move { settle.report(&launch, &reason).await });
    }
}

/// The spawn lock, held (F267 step 2): what the holder launches is named
/// only through it, and cleared when it is released, before the lock is, so
/// a stop that gives up waiting never names an earlier holder's launch.
pub struct SpawnHold {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    waits: Arc<SpawnWaits>,
}

impl SpawnHold {
    /// Name what this holder launches, for a stop that gives up on it. It
    /// goes to daemon.log, so text that is not one printable line (F133)
    /// names nothing: a BRAKE, it can only leave the stop's log vaguer.
    pub fn name(&self, what: String) {
        let named = pij_core::render::is_one_line(&what).then_some(what);
        *self.waits.launching.lock().expect("spawn waits launching") = named;
    }
}

impl Drop for SpawnHold {
    fn drop(&mut self) {
        // Before `_guard`, so before the next holder can name its own.
        *self.waits.launching.lock().expect("spawn waits launching") = None;
    }
}

/// One spawn request's hold on its launch.
pub struct WaitGuard {
    waits: Arc<SpawnWaits>,
    spawn_id: String,
    watcher: u64,
    settled: bool,
}

impl WaitGuard {
    /// The caller's answer naming the seat is built (`outcome`: `bound`,
    /// `dispatched` or `failed-bound`).
    pub fn answered(mut self, outcome: &'static str) {
        self.settled = true;
        self.waits.answered(&self.spawn_id, self.watcher, outcome);
    }

    /// Nothing was left to bind (the pane was removed): forget it.
    pub fn withdrawn(mut self) {
        self.settled = true;
        self.waits.take(&self.spawn_id, self.watcher);
    }

    /// The caller's failure answer is built and says the seat is not bound.
    pub fn failed(mut self, reason: &str) {
        self.settled = true;
        self.waits
            .not_answered(&self.spawn_id, Some(self.watcher), reason);
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.waits
                .not_answered(&self.spawn_id, Some(self.watcher), ABANDONED);
        }
    }
}

/// The daemon's [`Settle`]: spine records and the parent's notice from `pij-bg`.
pub struct ParentNotice {
    registry: Arc<dyn Registry>,
    event_bus: Arc<EventBus>,
    delivery: Arc<DeliveryService>,
}

impl ParentNotice {
    /// Over the daemon's roster, spine and delivery path.
    pub fn new(
        registry: Arc<dyn Registry>,
        event_bus: Arc<EventBus>,
        delivery: Arc<DeliveryService>,
    ) -> Self {
        Self {
            registry,
            event_bus,
            delivery,
        }
    }

    async fn record(&self, kind: &str, launch: &Launch, payload: serde_json::Value) {
        let at = match crate::http::system_time_ms() {
            Ok(at) => at,
            Err(error) => {
                eprintln!("pij-rs: {kind} for spawn {}: {error}", launch.spawn_id);
                return;
            }
        };
        if let Err(error) = self
            .event_bus
            .publish(Event {
                seq: None,
                v: wire::EVENT_VERSION,
                at,
                kind: kind.to_string(),
                seat: Some(launch.seat.clone()),
                payload: payload.to_string(),
            })
            .await
        {
            eprintln!("pij-rs: {kind} for spawn {}: {error}", launch.spawn_id);
        }
    }
}

#[async_trait]
impl Settle for ParentNotice {
    async fn answered(&self, launch: &Launch, outcome: &'static str) {
        let payload = serde_json::json!({"spawn_id": launch.spawn_id, "outcome": outcome});
        self.record(ANSWERED, launch, payload).await;
    }

    async fn report(&self, launch: &Launch, reason: &str) {
        let pane = launch.pane.as_deref().unwrap_or("unknown");
        let close = launch.close_command();
        let mut payload = serde_json::json!({
            "spawn_id": launch.spawn_id,
            "pane": pane,
            "parent": launch.parent,
            "reason": reason,
            "close": close,
            "notice": null,
        });
        let parent = match &launch.parent {
            None => Err("no parent recorded".to_string()),
            Some(parent) => match self.registry.get(parent).await {
                Ok(Some(row)) if row.tombstoned_at.is_none() => Ok(row),
                Ok(_) => Err("no live parent".to_string()),
                Err(error) => Err(format!("parent lookup failed: {error}")),
            },
        };
        match parent {
            Ok(parent) => {
                let body = format!(
                    "Your spawn {} never told you about seat {} in pane {pane}: {reason}. \
                     That seat registered and is running now. Use it, or close it: {close}",
                    launch.spawn_id, launch.seat
                );
                match self
                    .delivery
                    .send(SeatId::from(pij_core::BG_ACTOR), parent.id, body)
                    .await
                {
                    Ok(receipt) => payload["notice"] = serde_json::json!(receipt),
                    Err(error) => {
                        // Not admitted: no record, so the next boot tries again.
                        eprintln!(
                            "pij-rs: spawn {}: parent notice failed: {error}",
                            launch.spawn_id
                        );
                        return;
                    }
                }
            }
            Err(withheld) => payload["withheld"] = serde_json::json!(withheld),
        }
        self.record(REPORTED, launch, payload).await;
    }
}

/// Boot: re-arm every live spawned row whose launch the spine shows was never
/// settled (`spawn.launched` with neither `spawn.answered` nor
/// `spawn.reported`), bound or not. One indexed read per live spawned row.
pub async fn recover(waits: &SpawnWaits, registry: &dyn Registry, spine: &dyn Spine) {
    let seats = match registry.list(SeatFilter::default()).await {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("pij-rs: unsettled spawns not recovered: {error}");
            return;
        }
    };
    for seat in seats.iter().filter(|seat| seat.tombstoned_at.is_none()) {
        let Some(launch) = Launch::of(seat) else {
            continue;
        };
        let events = match spine
            .all_matching(&seat.id, &[LAUNCHED, ANSWERED, REPORTED])
            .await
        {
            Ok(events) => events,
            Err(error) => {
                eprintln!("pij-rs: spawn {} not recovered: {error}", launch.spawn_id);
                continue;
            }
        };
        let mine = |kind: &str| {
            events.iter().any(|event| {
                event.kind == kind
                    && serde_json::from_str::<serde_json::Value>(&event.payload)
                        .ok()
                        .is_some_and(|payload| payload["spawn_id"] == launch.spawn_id.as_str())
            })
        };
        if mine(LAUNCHED) && !mine(ANSWERED) && !mine(REPORTED) {
            waits.recover(launch, seat.proc.is_some());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[derive(Default)]
    struct Recorded {
        answered: Mutex<Vec<(String, &'static str)>>,
        reported: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl Settle for Recorded {
        async fn answered(&self, launch: &Launch, outcome: &'static str) {
            self.answered
                .lock()
                .unwrap()
                .push((launch.spawn_id.clone(), outcome));
        }
        async fn report(&self, launch: &Launch, reason: &str) {
            self.reported
                .lock()
                .unwrap()
                .push((launch.spawn_id.clone(), reason.to_string()));
        }
    }

    fn launch(spawn_id: &str) -> Launch {
        Launch {
            spawn_id: spawn_id.to_string(),
            seat: SeatId::from(format!("pij-{spawn_id}")),
            pane: Some("%1".to_string()),
            parent: Some("pij-parent".into()),
        }
    }

    fn waits() -> (Arc<SpawnWaits>, Arc<Recorded>) {
        let recorded = Arc::new(Recorded::default());
        (Arc::new(SpawnWaits::new(recorded.clone())), recorded)
    }

    async fn reported(recorded: &Recorded) -> Vec<(String, String)> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        recorded.reported.lock().unwrap().clone()
    }

    fn bind(waits: &SpawnWaits, spawn_id: &str) {
        let launch = launch(spawn_id);
        waits.bound(spawn_id, &launch.seat, launch.pane.as_deref());
    }

    /// R253-001: the seat binds while the request watches, then the request
    /// is dropped unanswered. The drop comes second, so it hands off.
    #[tokio::test]
    async fn bind_then_drop_reports_once() {
        let (waits, recorded) = waits();
        let guard = waits.watch(launch("s-1"));
        bind(&waits, "s-1");
        assert!(reported(&recorded).await.is_empty(), "still watched");
        drop(guard);
        assert_eq!(
            reported(&recorded).await,
            [("s-1".to_string(), ABANDONED.to_string())]
        );
    }

    /// R253-001 addendum: the seat binds after the wait loop's last read,
    /// then the timeout's failure answer (built from the stale row) settles.
    #[tokio::test]
    async fn bind_before_a_timeout_settles_reports_once() {
        let (waits, recorded) = waits();
        let guard = waits.watch(launch("s-2"));
        bind(&waits, "s-2");
        guard.failed("timed out after 1s waiting for registration");
        assert_eq!(
            reported(&recorded).await,
            [(
                "s-2".to_string(),
                "timed out after 1s waiting for registration".to_string()
            )]
        );
    }

    /// A failure first, the bind later: the bind hands off.
    #[tokio::test]
    async fn a_failure_then_a_late_bind_reports_once() {
        let (waits, recorded) = waits();
        waits.watch(launch("s-3")).failed("timed out");
        assert!(reported(&recorded).await.is_empty(), "not bound yet");
        bind(&waits, "s-3");
        bind(&waits, "s-3");
        assert_eq!(
            reported(&recorded).await,
            [("s-3".to_string(), "timed out".to_string())]
        );
    }

    /// Answered with the seat, a launch is recorded as answered and never
    /// reported, whatever binds after.
    #[tokio::test]
    async fn an_answered_launch_is_never_reported() {
        let (waits, recorded) = waits();
        let guard = waits.watch(launch("s-4"));
        bind(&waits, "s-4");
        guard.answered("bound");
        bind(&waits, "s-4");
        assert!(reported(&recorded).await.is_empty());
        assert_eq!(
            recorded.answered.lock().unwrap().clone(),
            [("s-4".to_string(), "bound")]
        );
    }

    /// Only the launch's own seat on its own pane binds it.
    #[tokio::test]
    async fn another_seat_or_pane_sharing_the_spawn_id_does_not_bind_it() {
        let (waits, recorded) = waits();
        waits.watch(launch("s-5")).failed("timed out");
        waits.bound("s-5", &SeatId::from("pij-stranger"), Some("%1"));
        waits.bound("s-5", &launch("s-5").seat, Some("%9"));
        assert!(reported(&recorded).await.is_empty());
        bind(&waits, "s-5");
        assert_eq!(reported(&recorded).await.len(), 1);
    }

    /// R253-002: recovered at boot, a bound launch is reported at once and an
    /// unbound one when it binds.
    #[tokio::test]
    async fn a_recovered_launch_reports_when_bound() {
        let (waits, recorded) = waits();
        waits.recover(launch("s-6"), true);
        waits.recover(launch("s-7"), false);
        assert_eq!(
            reported(&recorded).await,
            [("s-6".to_string(), RESTARTED.to_string())]
        );
        bind(&waits, "s-7");
        assert_eq!(reported(&recorded).await.len(), 2);
    }

    /// F255: the same spawn sent again watches the launch in place of the
    /// request that lost its caller. The earlier request ending afterwards
    /// settles nothing; the latest one answers, and nobody is reported.
    #[tokio::test]
    async fn a_resent_spawn_settles_its_launch_and_the_earlier_request_does_not() {
        let (waits, recorded) = waits();
        let first = waits.watch(launch("s-8"));
        bind(&waits, "s-8");
        let again = waits.watch_again(launch("s-8"), true);
        drop(first);
        assert!(reported(&recorded).await.is_empty(), "the earlier request");
        again.answered("bound");
        assert!(reported(&recorded).await.is_empty());
        assert_eq!(
            recorded.answered.lock().unwrap().clone(),
            [("s-8".to_string(), "bound")]
        );
    }

    /// F255: a launch recovered at boot unbound, then sent again by its
    /// caller: the bind comes while the caller watches, so the caller is
    /// answered and the parent is never told.
    #[tokio::test]
    async fn a_recovered_launch_sent_again_is_answered_not_reported() {
        let (waits, recorded) = waits();
        waits.recover(launch("s-9"), false);
        let again = waits.watch_again(launch("s-9"), false);
        bind(&waits, "s-9");
        assert!(reported(&recorded).await.is_empty());
        again.answered("bound");
        assert!(reported(&recorded).await.is_empty());
        assert_eq!(recorded.answered.lock().unwrap().len(), 1);
    }

    /// F255: a resent spawn that loses its caller too, after the seat bound
    /// while the first request watched, hands the seat to the parent: the
    /// binding the first watch saw is kept even when the resend read its row
    /// before the bind.
    #[tokio::test]
    async fn a_resent_spawn_that_also_goes_unanswered_reports_its_bound_seat() {
        let (waits, recorded) = waits();
        let first = waits.watch(launch("s-10"));
        bind(&waits, "s-10");
        let again = waits.watch_again(launch("s-10"), false);
        drop(first);
        drop(again);
        assert_eq!(
            reported(&recorded).await,
            [("s-10".to_string(), ABANDONED.to_string())]
        );
    }

    /// F255: the earlier request's answer, built after the resend took over,
    /// settles nothing; its caller may be the one that went away.
    #[tokio::test]
    async fn an_earlier_requests_answer_does_not_settle_a_resent_spawn() {
        let (waits, recorded) = waits();
        let first = waits.watch(launch("s-11"));
        let again = waits.watch_again(launch("s-11"), false);
        bind(&waits, "s-11");
        first.answered("bound");
        drop(again);
        assert_eq!(
            reported(&recorded).await,
            [("s-11".to_string(), ABANDONED.to_string())]
        );
        assert!(recorded.answered.lock().unwrap().is_empty());
    }

    /// R309-03: a bound launch recovered at boot is reported once. Its caller
    /// then sends it again and is answered, or loses that answer too, and the
    /// seat binds again: never a second report, and nothing left unsettled.
    #[tokio::test]
    async fn a_reported_launch_sent_again_is_never_reported_again() {
        let (waits, recorded) = waits();
        waits.recover(launch("s-12"), true);
        assert_eq!(reported(&recorded).await.len(), 1);
        waits.watch_again(launch("s-12"), true).answered("bound");
        drop(waits.watch_again(launch("s-12"), true));
        bind(&waits, "s-12");
        assert_eq!(reported(&recorded).await.len(), 1, "reported once");
        assert!(recorded.answered.lock().unwrap().is_empty());
        assert!(waits.entries.lock().unwrap().is_empty(), "nothing re-armed");
    }

    /// R309-03: reported after binding (its request went away), then re-armed
    /// by a resend, then bound again and abandoned again: still one report.
    #[tokio::test]
    async fn a_launch_reported_after_binding_is_not_reported_twice_by_a_resend() {
        let (waits, recorded) = waits();
        let first = waits.watch(launch("s-13"));
        bind(&waits, "s-13");
        drop(first);
        assert_eq!(reported(&recorded).await.len(), 1);
        let again = waits.watch_again(launch("s-13"), true);
        bind(&waits, "s-13");
        drop(again);
        assert_eq!(reported(&recorded).await.len(), 1, "reported once");
    }

    /// R309-03: a launch answered bound, then sent again (that answer was
    /// lost too) and its answer lost: it stays settled, unwatched, unreported.
    #[tokio::test]
    async fn a_known_bound_resend_whose_answer_is_lost_leaves_nothing_unsettled() {
        let (waits, recorded) = waits();
        let first = waits.watch(launch("s-14"));
        bind(&waits, "s-14");
        first.answered("bound");
        drop(waits.watch_again(launch("s-14"), true));
        bind(&waits, "s-14");
        assert!(reported(&recorded).await.is_empty());
        assert!(waits.entries.lock().unwrap().is_empty(), "nothing re-armed");
    }

    /// A `--no-wait` launch answered before it bound, sent again because that
    /// answer was lost: watched again, so a later bind with no answer reports.
    #[tokio::test]
    async fn a_dispatched_launch_sent_again_unbound_is_watched_again() {
        let (waits, recorded) = waits();
        waits.watch(launch("s-15")).answered("dispatched");
        let again = waits.watch_again(launch("s-15"), false);
        bind(&waits, "s-15");
        drop(again);
        assert_eq!(
            reported(&recorded).await,
            [("s-15".to_string(), ABANDONED.to_string())]
        );
    }

    /// F264: recovered unbound, then bound and reported by a registration
    /// while a resend held the row it read before that bind (unbound). The
    /// resend never re-arms it: bound again and abandoned, still one report.
    #[tokio::test]
    async fn a_stale_unbound_resend_never_rearms_a_reported_launch() {
        let (waits, recorded) = waits();
        waits.recover(launch("s-16"), false);
        bind(&waits, "s-16");
        assert_eq!(reported(&recorded).await.len(), 1);
        let again = waits.watch_again(launch("s-16"), false);
        bind(&waits, "s-16");
        drop(again);
        assert_eq!(reported(&recorded).await.len(), 1, "reported once");
        assert!(waits.entries.lock().unwrap().is_empty(), "nothing re-armed");
    }

    /// F264: the same, abandoned before the bind: nothing is left unsettled.
    #[tokio::test]
    async fn a_stale_unbound_resend_abandoned_before_a_bind_leaves_nothing_unsettled() {
        let (waits, recorded) = waits();
        waits.recover(launch("s-17"), false);
        bind(&waits, "s-17");
        assert_eq!(reported(&recorded).await.len(), 1);
        drop(waits.watch_again(launch("s-17"), false));
        assert!(waits.entries.lock().unwrap().is_empty(), "nothing re-armed");
        bind(&waits, "s-17");
        assert_eq!(reported(&recorded).await.len(), 1, "reported once");
    }

    /// F264: a `--no-wait` launch answered, then bound: a resend holding the
    /// row from before that bind finds it settled bound and re-arms nothing.
    #[tokio::test]
    async fn a_dispatched_launch_bound_since_is_not_rearmed_by_a_stale_resend() {
        let (waits, recorded) = waits();
        waits.watch(launch("s-18")).answered("dispatched");
        bind(&waits, "s-18");
        drop(waits.watch_again(launch("s-18"), false));
        assert!(waits.entries.lock().unwrap().is_empty(), "nothing re-armed");
        assert!(reported(&recorded).await.is_empty());
    }

    /// F264: the settled record keeps the newest [`SETTLED_KEPT`] launches.
    #[tokio::test]
    async fn the_settled_record_keeps_only_the_newest_launches() {
        let (waits, _recorded) = waits();
        for n in 0..=SETTLED_KEPT {
            let spawn_id = format!("s-k{n}");
            waits.watch(launch(&spawn_id)).answered("bound");
        }
        let settled = waits.settled.lock().unwrap();
        assert_eq!(settled.launches.len(), SETTLED_KEPT);
        assert_eq!(settled.order.len(), SETTLED_KEPT);
        assert!(!settled.launches.contains_key("s-k0"), "the oldest goes");
        assert!(settled.launches.contains_key(&format!("s-k{SETTLED_KEPT}")));
    }

    /// F267 step 2 (deer new-2, mongoose N3): the name goes with the hold.
    /// A launch that names itself and then releases the lock (it refused, or
    /// wrote its row) leaves nothing named, so a stop that gives up on the
    /// next holder, which has not named itself yet, never blames the last.
    #[tokio::test]
    async fn a_released_hold_leaves_no_launch_named() {
        let (waits, _recorded) = waits();
        let first = waits.hold().await;
        first.name("seat pij-first".to_string());
        assert_eq!(waits.launching().as_deref(), Some("seat pij-first"));
        let next = tokio::spawn({
            let waits = Arc::clone(&waits);
            async move { waits.hold().await }
        });
        tokio::task::yield_now().await;
        drop(first);
        let next = next.await.expect("next holder");
        assert_eq!(
            waits.launching(),
            None,
            "the next holder has not named itself"
        );
        next.name("seat pij-next".to_string());
        assert_eq!(waits.launching().as_deref(), Some("seat pij-next"));
        drop(next);
        assert_eq!(waits.launching(), None);
        tokio::time::timeout(Duration::from_secs(1), waits.stop())
            .await
            .expect("a stop finds the lock free");
    }

    /// F267 step 2 (deer new-1): a name that is not one printable line is
    /// never handed to a stop to log, wherever the holder names itself.
    #[tokio::test]
    async fn a_name_that_could_forge_a_log_line_names_nothing() {
        let (waits, _recorded) = waits();
        let hold = waits.hold().await;
        hold.name("a spawn into /tmp/x\npij-rs: forged".to_string());
        assert_eq!(waits.launching(), None);
        hold.name("a spawn into /tmp/x\u{1b}[2J".to_string());
        assert_eq!(waits.launching(), None);
    }

    /// The close command quotes what a shell would split or expand.
    #[test]
    fn the_close_command_quotes_each_word() {
        assert_eq!(
            close_command(&SeatId::from("pij-a"), Some("%7")),
            "pij close pij-a; tmux kill-pane -t %7"
        );
        assert_eq!(
            close_command(&SeatId::from("pij-a b"), Some("%7;rm -rf ~")),
            "pij close 'pij-a b'; tmux kill-pane -t '%7;rm -rf ~'"
        );
    }
}
