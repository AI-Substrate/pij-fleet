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

use std::collections::HashMap;
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
}

/// Launches whose hand-off is not settled.
pub struct SpawnWaits {
    entries: Mutex<HashMap<String, Entry>>,
    settle: Arc<dyn Settle>,
}

impl SpawnWaits {
    /// Settlements go to `settle`.
    pub fn new(settle: Arc<dyn Settle>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            settle,
        }
    }

    /// A spawn request now watches `launch`. Settle the guard; dropping it
    /// unsettled counts as [`ABANDONED`].
    pub fn watch(self: &Arc<Self>, launch: Launch) -> WaitGuard {
        let spawn_id = launch.spawn_id.clone();
        self.entries.lock().expect("spawn waits").insert(
            spawn_id.clone(),
            Entry {
                launch,
                bound: false,
                unanswered: None,
            },
        );
        WaitGuard {
            waits: Arc::clone(self),
            spawn_id,
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
            },
        );
        self.not_answered(&spawn_id, RESTARTED);
    }

    /// A registration bound `seat` to its process. Only the launch's own seat
    /// on its own pane counts.
    pub fn bound(&self, spawn_id: &str, seat: &SeatId, pane: Option<&str>) {
        let report = {
            let mut entries = self.entries.lock().expect("spawn waits");
            let Some(entry) = entries.get_mut(spawn_id) else {
                return;
            };
            if entry.launch.seat != *seat || entry.launch.pane.as_deref() != pane {
                return;
            }
            entry.bound = true;
            match entry.unanswered.clone() {
                Some(reason) => entries.remove(spawn_id).map(|entry| (entry.launch, reason)),
                None => None,
            }
        };
        if let Some((launch, reason)) = report {
            self.hand_off(launch, reason);
        }
    }

    fn not_answered(&self, spawn_id: &str, reason: &str) {
        let report = {
            let mut entries = self.entries.lock().expect("spawn waits");
            let Some(entry) = entries.get_mut(spawn_id) else {
                return;
            };
            if entry.bound {
                entries
                    .remove(spawn_id)
                    .map(|entry| (entry.launch, reason.to_string()))
            } else {
                entry.unanswered = Some(reason.to_string());
                None
            }
        };
        if let Some((launch, reason)) = report {
            self.hand_off(launch, reason);
        }
    }

    fn answered(&self, spawn_id: &str, outcome: &'static str) {
        let Some(entry) = self.entries.lock().expect("spawn waits").remove(spawn_id) else {
            return;
        };
        let settle = Arc::clone(&self.settle);
        // Detached: the answer is built; recording it must not be cancelled
        // with the request.
        tokio::spawn(async move { settle.answered(&entry.launch, outcome).await });
    }

    fn hand_off(&self, launch: Launch, reason: String) {
        let settle = Arc::clone(&self.settle);
        tokio::spawn(async move { settle.report(&launch, &reason).await });
    }
}

/// One spawn request's hold on its launch.
pub struct WaitGuard {
    waits: Arc<SpawnWaits>,
    spawn_id: String,
    settled: bool,
}

impl WaitGuard {
    /// The caller's answer naming the seat is built (`outcome`: `bound`,
    /// `dispatched` or `failed-bound`).
    pub fn answered(mut self, outcome: &'static str) {
        self.settled = true;
        self.waits.answered(&self.spawn_id, outcome);
    }

    /// Nothing was left to bind (the pane was removed): forget it.
    pub fn withdrawn(mut self) {
        self.settled = true;
        self.waits
            .entries
            .lock()
            .expect("spawn waits")
            .remove(&self.spawn_id);
    }

    /// The caller's failure answer is built and says the seat is not bound.
    pub fn failed(mut self, reason: &str) {
        self.settled = true;
        self.waits.not_answered(&self.spawn_id, reason);
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.waits.not_answered(&self.spawn_id, ABANDONED);
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
