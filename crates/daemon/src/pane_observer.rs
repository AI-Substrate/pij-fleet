//! Long-lived tmux pane observation feeding the interaction safety gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::ports::{Registry, SeatFilter, TapAttach, TmuxPort};
use pij_harnesses::{ComposerRegion, InteractionGate, composer_region};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};

const CAPTURE_LINES: u32 = u32::MAX;

/// The longest a pane whose tap attach keeps failing waits between attempts
/// (F112). The wait doubles from the observer's interval up to this.
pub const ATTACH_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// The longest a quiet pane goes without a capture (F258). A pane's tap
/// carries every byte it writes, so a pane that wrote nothing is replayed, not
/// re-read; this bounds what a dead or silent tap could hide.
pub const CAPTURE_BACKSTOP: Duration = Duration::from_secs(30);

/// What the observer has done since the daemon started, for `ping` (F258).
/// Shared like [`TapRefusals`]: the observer writes it, `/health` reads it.
#[derive(Debug, Default)]
pub struct ObserverCounts {
    passes: AtomicU64,
    active_us: AtomicU64,
    captures: AtomicU64,
    capture_bytes: AtomicU64,
    skipped: AtomicU64,
    backstop: AtomicU64,
}

/// One reading of [`ObserverCounts`]. `active_us / passes` is a pass's cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ObserverCountsView {
    /// Observation passes (one per tick).
    pub passes: u64,
    /// Wall time inside passes, in microseconds.
    pub active_us: u64,
    /// `capture-pane` reads, the `backstop` ones included.
    pub captures: u64,
    /// Bytes those captures returned.
    pub capture_bytes: u64,
    /// Quiet panes whose last verdict was replayed instead of captured.
    pub skipped: u64,
    /// Captures taken only because [`CAPTURE_BACKSTOP`] had passed.
    pub backstop: u64,
}

impl ObserverCounts {
    /// Nothing observed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every count, read once each.
    #[must_use]
    pub fn snapshot(&self) -> ObserverCountsView {
        ObserverCountsView {
            passes: self.passes.load(Ordering::Relaxed),
            active_us: self.active_us.load(Ordering::Relaxed),
            captures: self.captures.load(Ordering::Relaxed),
            capture_bytes: self.capture_bytes.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            backstop: self.backstop.load(Ordering::Relaxed),
        }
    }

    fn record_pass(&self, took: Duration) {
        self.passes.fetch_add(1, Ordering::Relaxed);
        self.active_us.fetch_add(
            u64::try_from(took.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn record_capture(&self, bytes: usize, backstop: bool) {
        self.captures.fetch_add(1, Ordering::Relaxed);
        self.capture_bytes
            .fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
        if backstop {
            self.backstop.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A pane whose tap could not be attached, and why: shown in the PA digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TapRefusal {
    /// The pane, e.g. `%12`.
    pub pane: String,
    /// The adapter's refusal, which names any manual repair.
    pub detail: String,
}

#[derive(Debug)]
struct RefusalState {
    detail: String,
    failures: u32,
    retry_at: Instant,
}

/// Panes whose tap attach failed, with a bounded retry backoff (F112). Shared
/// by the pane observer, which writes it, and the PA digest, which reads it.
/// Keyed by pane id, which tmux never reuses within a server's life: an entry
/// goes when its pane attaches or leaves the observed set, so a new pane
/// incarnation starts fresh.
#[derive(Debug, Default)]
pub struct TapRefusals {
    panes: Mutex<BTreeMap<String, RefusalState>>,
}

impl TapRefusals {
    /// No refused panes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every refused pane and why, by pane id.
    #[must_use]
    pub fn list(&self) -> Vec<TapRefusal> {
        self.panes
            .lock()
            .expect("tap refusals mutex")
            .iter()
            .map(|(pane, state)| TapRefusal {
                pane: pane.clone(),
                detail: state.detail.clone(),
            })
            .collect()
    }

    /// Whether `pane` is still waiting out its backoff at `now`.
    fn waiting(&self, pane: &str, now: Instant) -> bool {
        self.panes
            .lock()
            .expect("tap refusals mutex")
            .get(pane)
            .is_some_and(|state| now < state.retry_at)
    }

    /// Record a failed attach and schedule the next attempt. True when this
    /// pane incarnation had no refusal yet, or a different one: worth a log.
    pub(crate) fn record(
        &self,
        pane: &str,
        detail: String,
        now: Instant,
        interval: Duration,
    ) -> bool {
        let mut panes = self.panes.lock().expect("tap refusals mutex");
        let state = panes.entry(pane.to_string()).or_insert(RefusalState {
            detail: String::new(),
            failures: 0,
            retry_at: now,
        });
        let new = state.detail != detail;
        state.failures = state.failures.saturating_add(1);
        let wait = interval
            .saturating_mul(1u32 << state.failures.min(16))
            .min(ATTACH_BACKOFF_MAX);
        state.retry_at = now + wait;
        state.detail = detail;
        new
    }

    fn clear(&self, pane: &str) {
        self.panes.lock().expect("tap refusals mutex").remove(pane);
    }

    fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.panes
            .lock()
            .expect("tap refusals mutex")
            .retain(|pane, _| keep(pane));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerKnowledge {
    Unknown,
    Blank,
    NonBlank,
    /// Text was observed, then a redraw produced a blank capture whose cursor
    /// coordinates may be stale. The gate remains closed until a quiet blank
    /// corroborates that the text actually left.
    PendingClear,
}

#[derive(Debug)]
struct PaneState {
    knowledge: ComposerKnowledge,
    /// The composer the last capture recognised, the cursor it was read at,
    /// and when (F258): what a quiet pane replays.
    last: Option<LastCapture>,
}

#[derive(Debug)]
struct LastCapture {
    composer: String,
    cursor: (u32, u32),
    at: Instant,
    /// The gate's drain count for this pane when that tick began (F266).
    gate_drains: u64,
}

/// What a pane with no new tap bytes gets this tick (F258).
enum Replay {
    /// The last capture's composer, decided again without a capture.
    Composer(String),
    /// Replayable, but its last capture is [`CAPTURE_BACKSTOP`] old.
    BackstopDue,
    /// Not replayable: the pane is captured as always.
    Capture,
}

/// Polls attached tmux pane taps and turns only positively recognised composer
/// states into interaction-gate evidence. A tombstone is consent withdrawal:
/// the next sweep actively detaches its pipe, clears ownership, and deletes the
/// captured sink before it can ever consider reattaching.
///
/// # Composition recipe
///
/// In `crates/daemon/src/lib.rs`, construct
/// `PaneObserver::new(Arc::clone(&services.registry),
/// Arc::clone(&services.tmux), Arc::clone(&interaction),
/// state_dir.join("pane-signals"),
/// Duration::from_millis(config.pane_observer_interval_ms))?` before publishing
/// the boot key. After publish, call `observer.start()` and store the returned
/// [`PaneObserverLoop`] on `Daemon`. During shutdown, await
/// `pane_observer.shutdown()` after stopping HTTP and delivery; it joins the loop
/// and detaches every tap it owns.
pub struct PaneObserver {
    registry: Arc<dyn Registry>,
    tmux: Arc<dyn TmuxPort>,
    gate: Arc<InteractionGate>,
    sink_dir: PathBuf,
    world_reconciled: AtomicBool,
    /// The orphaned-tap sweep's own once-flag. `world_reconciled` waits for an
    /// error-free pass, and one failing pane would otherwise re-run the sweep
    /// every tick (PR #99 review S1).
    orphans_swept: AtomicBool,
    interval: Duration,
    panes: Mutex<BTreeMap<String, PaneState>>,
    refusals: Arc<TapRefusals>,
    counts: Arc<ObserverCounts>,
}

impl PaneObserver {
    /// Construct an observer without starting it.
    ///
    /// # Errors
    /// Returns [`PijError::Adapter`] when `interval` is zero.
    pub fn new(
        registry: Arc<dyn Registry>,
        tmux: Arc<dyn TmuxPort>,
        gate: Arc<InteractionGate>,
        sink_dir: PathBuf,
        interval: Duration,
    ) -> Result<Self> {
        if interval.is_zero() {
            return Err(observer_error(
                "pane_observer_interval_ms must be greater than zero",
            ));
        }
        Ok(Self {
            registry,
            tmux,
            gate,
            sink_dir,
            world_reconciled: AtomicBool::new(false),
            orphans_swept: AtomicBool::new(false),
            interval,
            panes: Mutex::new(BTreeMap::new()),
            refusals: Arc::new(TapRefusals::new()),
            counts: Arc::new(ObserverCounts::new()),
        })
    }

    /// Share the refused-pane record with the PA digest.
    #[must_use]
    pub fn with_tap_refusals(mut self, refusals: Arc<TapRefusals>) -> Self {
        self.refusals = refusals;
        self
    }

    /// Share the observer's counts with `ping`.
    #[must_use]
    pub fn with_counts(mut self, counts: Arc<ObserverCounts>) -> Self {
        self.counts = counts;
        self
    }

    /// Start the long-lived observation loop. The first observation waits one
    /// configured interval, matching the daemon's other periodic workers.
    pub fn start(self: &Arc<Self>) -> PaneObserverLoop {
        let (stop, mut stop_rx) = oneshot::channel();
        let observer = Arc::clone(self);
        let interval = self.interval;
        let joined = tokio::spawn(async move {
            let mut schedule = tokio::time::interval_at(Instant::now() + interval, interval);
            schedule.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    _ = schedule.tick() => {
                        if let Err(error) = observer.observe_once().await {
                            eprintln!("pij-rs pane observer: {error}");
                        }
                    }
                }
            }
        });
        PaneObserverLoop {
            stop,
            joined,
            observer: Arc::clone(self),
        }
    }

    async fn observe_once(&self) -> Result<()> {
        let started = std::time::Instant::now();
        let observed = self.observe_pass().await;
        self.counts.record_pass(started.elapsed());
        observed
    }

    async fn observe_pass(&self) -> Result<()> {
        // Only live seats' panes are wanted (F185: not every retired row).
        let live = SeatFilter {
            live_only: true,
            ..SeatFilter::default()
        };
        let seats = match self.registry.list(live).await {
            Ok(seats) => seats,
            Err(error) => {
                for pane in self.tracked_panes() {
                    self.gate.record_unknown(&pane);
                }
                return Err(error);
            }
        };
        let wanted: BTreeSet<String> = seats
            .into_iter()
            .filter(|seat| seat.tombstoned_at.is_none())
            .filter_map(|seat| seat.pane)
            .collect();
        let all_panes = match self.tmux.list_panes().await {
            Ok(panes) => panes,
            Err(error) => {
                for pane in self.tracked_panes() {
                    self.gate.record_unknown(&pane);
                }
                return Err(error);
            }
        };
        let panes = all_panes
            .iter()
            .filter(|pane| wanted.contains(&pane.id))
            .cloned()
            .collect::<Vec<_>>();
        let live: BTreeSet<&str> = panes.iter().map(|pane| pane.id.as_str()).collect();
        let mut retired: BTreeSet<String> = self
            .tracked_panes()
            .into_iter()
            .filter(|pane| !live.contains(pane.as_str()))
            .collect();
        let mut first_error = None;
        let reconcile_world = !self.world_reconciled.load(Ordering::Acquire);
        if reconcile_world {
            for pane in &all_panes {
                if wanted.contains(&pane.id) {
                    continue;
                }
                let expected = self.sink_path(&pane.id);
                match self.tmux.pane_tap_sink(&pane.id).await {
                    Ok(Some(marker)) if marker == expected => {
                        retired.insert(pane.id.clone());
                    }
                    Ok(Some(marker)) => {
                        eprintln!(
                            "pij-rs pane observer: leaving foreign tap on pane {:?}: marker {:?} does not match expected {:?}",
                            pane.id, marker, expected
                        );
                    }
                    Ok(None) => {}
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
        }

        let mut retirements = JoinSet::new();
        for pane in retired {
            let tmux = Arc::clone(&self.tmux);
            retirements.spawn(async move {
                let result = tmux.detach_pane_tap(&pane).await;
                (pane, result)
            });
        }
        while let Some(joined) = retirements.join_next().await {
            match joined {
                Ok((pane, Ok(()))) => {
                    self.panes
                        .lock()
                        .expect("pane observer mutex")
                        .remove(&pane);
                    self.gate.record_unknown(&pane);
                }
                Ok((pane, Err(error))) => {
                    self.gate.record_unknown(&pane);
                    first_error.get_or_insert(error);
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| {
                        observer_error(format!("tap retirement task failed: {error}"))
                    });
                }
            }
        }

        if !self.orphans_swept.load(Ordering::Acquire) {
            // Once per boot, after retirement: sinks of panes this server no
            // longer has. Pane absence is the policy; the adapter's mtime grace
            // is a brake that can only spare (plan 185, F5). Retried only while
            // the sweep itself fails.
            let swept = self.tmux.sweep_orphan_taps().await;
            if swept.is_ok() {
                self.orphans_swept.store(true, Ordering::Release);
            }
            match swept {
                Ok(sweep) if sweep.claimed > 0 || sweep.removed > 0 || sweep.spared_recent > 0 => {
                    eprintln!(
                        "pij-rs pane observer: orphaned-tap sweep removed {} sink(s), {} bytes, of panes tmux no longer lists; claimed {} pre-ledger sink(s); spared {} written within the grace",
                        sweep.removed, sweep.removed_bytes, sweep.claimed, sweep.spared_recent
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }

        self.refusals.retain(|pane| live.contains(pane));
        for pane in panes {
            // A pane whose attach keeps failing is retried with a bounded
            // backoff, not every tick (F112); it stays unknown meanwhile.
            let now = Instant::now();
            if self.refusals.waiting(&pane.id, now) {
                self.gate.record_unknown(&pane.id);
                continue;
            }
            let tracked = self.is_tracked(&pane.id);
            let sink = self.sink_path(&pane.id);
            match self.tmux.attach_pane_tap(&pane.id, &sink).await {
                Ok(_) if !tracked => {
                    self.refusals.clear(&pane.id);
                    self.panes.lock().expect("pane observer mutex").insert(
                        pane.id.clone(),
                        PaneState {
                            knowledge: ComposerKnowledge::Unknown,
                            last: None,
                        },
                    );
                }
                // F266: the pipe had vanished and was started again. Whatever
                // the pane wrote meanwhile never reached the tap, so its last
                // capture no longer stands: it is captured this tick.
                Ok(TapAttach::Attached) => {
                    self.refusals.clear(&pane.id);
                    self.forget_capture(&pane.id);
                }
                Ok(TapAttach::Kept) => self.refusals.clear(&pane.id),
                Err(error) => {
                    self.gate.record_unknown(&pane.id);
                    // One line per pane incarnation and refusal, not per retry.
                    if self
                        .refusals
                        .record(&pane.id, error.to_string(), now, self.interval)
                    {
                        if first_error.is_none() {
                            first_error = Some(error);
                        } else {
                            eprintln!("pij-rs pane observer: {error}");
                        }
                    }
                    continue;
                }
            }
            // Read before this tick's own drain: an injection that drains the
            // tap from here on changes it, and the capture taken below is
            // never replayed past that (F266).
            let gate_drains = self.gate.tap_drains(&pane.id);

            let bytes = match self.tmux.drain_pane_tap(&pane.id).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.set_unknown(&pane.id);
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let cursor = pane.cursor_x.zip(pane.cursor_y);
            // F258: a pane that printed nothing since its last capture, with
            // its cursor where it was, shows the screen that capture read: its
            // tap carries every byte the pane writes. Its verdict is replayed
            // through the same decision instead of re-reading the screen.
            let backstop = if bytes.is_empty() {
                match self.replay(&pane.id, cursor, now) {
                    Replay::Composer(composer) => {
                        self.counts.skipped.fetch_add(1, Ordering::Relaxed);
                        self.decide(&pane.id, &composer, false);
                        continue;
                    }
                    Replay::BackstopDue => true,
                    Replay::Capture => false,
                }
            } else {
                false
            };
            let capture = match self.tmux.capture(&pane.id, CAPTURE_LINES).await {
                Ok(capture) => capture,
                Err(error) => {
                    self.set_unknown(&pane.id);
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            self.counts.record_capture(capture.len(), backstop);
            let trailing_bytes = match self.tmux.drain_pane_tap(&pane.id).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.set_unknown(&pane.id);
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let tap_active = !bytes.is_empty() || !trailing_bytes.is_empty();
            let Some((cursor_x, cursor_y)) = cursor else {
                self.set_unknown(&pane.id);
                continue;
            };
            let ComposerRegion::Recognized(composer) =
                composer_region(&capture, cursor_x, cursor_y)
            else {
                self.set_unknown(&pane.id);
                continue;
            };
            self.remember(&pane.id, &composer, (cursor_x, cursor_y), now, gate_drains);
            self.decide(&pane.id, &composer, tap_active);
        }

        match first_error {
            Some(error) => Err(error),
            None => {
                if reconcile_world {
                    self.world_reconciled.store(true, Ordering::Release);
                }
                Ok(())
            }
        }
    }

    async fn detach_all(&self) -> Result<()> {
        let panes = self.tracked_panes();
        let mut first_error = None;
        for pane in panes {
            if let Err(error) = self.tmux.detach_pane_tap(&pane).await {
                first_error.get_or_insert(error);
            }
            self.panes
                .lock()
                .expect("pane observer mutex")
                .remove(&pane);
            self.gate.record_unknown(&pane);
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn tracked_panes(&self) -> Vec<String> {
        self.panes
            .lock()
            .expect("pane observer mutex")
            .keys()
            .cloned()
            .collect()
    }

    fn is_tracked(&self, pane: &str) -> bool {
        self.panes
            .lock()
            .expect("pane observer mutex")
            .contains_key(pane)
    }

    fn knowledge(&self, pane: &str) -> ComposerKnowledge {
        self.panes
            .lock()
            .expect("pane observer mutex")
            .get(pane)
            .map_or(ComposerKnowledge::Unknown, |state| state.knowledge)
    }

    fn set_knowledge(&self, pane: &str, knowledge: ComposerKnowledge) {
        if let Some(state) = self
            .panes
            .lock()
            .expect("pane observer mutex")
            .get_mut(pane)
        {
            state.knowledge = knowledge;
        }
    }

    fn set_unknown(&self, pane: &str) {
        self.set_knowledge(pane, ComposerKnowledge::Unknown);
        self.gate.record_unknown(pane);
    }

    /// The verdict for a recognised composer. A capture and the replay of an
    /// unchanged screen (F258) both come through here, so a replay makes the
    /// very gate calls a repeated capture of that screen made, at the gate's
    /// own clock: ageing (`expire_vetoes_if_idle`) runs exactly as before.
    fn decide(&self, pane: &str, composer: &str, tap_active: bool) {
        let blank = composer.chars().all(char::is_whitespace);
        let previous = self.knowledge(pane);
        if blank && tap_active {
            if previous != ComposerKnowledge::Unknown {
                self.set_knowledge(pane, ComposerKnowledge::PendingClear);
                self.gate.record_unknown(pane);
            } else {
                self.set_unknown(pane);
            }
            return;
        }
        if blank {
            if matches!(
                previous,
                ComposerKnowledge::Blank
                    | ComposerKnowledge::NonBlank
                    | ComposerKnowledge::PendingClear
            ) {
                self.gate.record_composer_cleared(pane);
            } else {
                self.gate.record_unknown(pane);
                self.set_knowledge(pane, ComposerKnowledge::Blank);
                return;
            }
            self.set_knowledge(pane, ComposerKnowledge::Blank);
        } else {
            if tap_active {
                self.gate.record_tap(pane);
            }
            self.gate.observe_composer(pane, composer);
            self.set_knowledge(pane, ComposerKnowledge::NonBlank);
        }
    }

    /// Keep what this capture recognised, for the replays that follow it.
    fn remember(
        &self,
        pane: &str,
        composer: &str,
        cursor: (u32, u32),
        at: Instant,
        gate_drains: u64,
    ) {
        if let Some(state) = self
            .panes
            .lock()
            .expect("pane observer mutex")
            .get_mut(pane)
        {
            state.last = Some(LastCapture {
                composer: composer.to_string(),
                cursor,
                at,
                gate_drains,
            });
        }
    }

    /// Drop `pane`'s last capture, so it is captured, not replayed (F266).
    fn forget_capture(&self, pane: &str) {
        if let Some(state) = self
            .panes
            .lock()
            .expect("pane observer mutex")
            .get_mut(pane)
        {
            state.last = None;
        }
    }

    /// Whether a pane with no new tap bytes can replay its last capture: its
    /// knowledge settled (Blank or NonBlank, never Unknown or PendingClear),
    /// its cursor where that capture read it, and the gate still holding that
    /// composer with no uncertainty and no drain of its own since. Another
    /// recorder (an injection's fresh read) drains the same tap: those bytes
    /// never reach this observer, whether that read then recorded a newer
    /// screen, left the gate uncertain or failed outright, so any drain since
    /// the capture means a new capture (F266). Past [`CAPTURE_BACKSTOP`] it is
    /// captured regardless.
    fn replay(&self, pane: &str, cursor: Option<(u32, u32)>, now: Instant) -> Replay {
        let (knowledge, composer, at, gate_drains) = {
            let panes = self.panes.lock().expect("pane observer mutex");
            let Some(state) = panes.get(pane) else {
                return Replay::Capture;
            };
            let Some(last) = state
                .last
                .as_ref()
                .filter(|last| Some(last.cursor) == cursor)
            else {
                return Replay::Capture;
            };
            (
                state.knowledge,
                last.composer.clone(),
                last.at,
                last.gate_drains,
            )
        };
        let held = self
            .gate
            .observed(pane)
            .filter(|observed| {
                observed.uncertainty_since.is_none() && observed.tap_drains == gate_drains
            })
            .and_then(|observed| observed.content);
        let holds = match (knowledge, held.as_deref()) {
            (ComposerKnowledge::Blank, Some(held)) => held.chars().all(char::is_whitespace),
            (ComposerKnowledge::NonBlank, Some(held)) => held == composer,
            _ => false,
        };
        if !holds {
            return Replay::Capture;
        }
        if now.saturating_duration_since(at) >= CAPTURE_BACKSTOP {
            return Replay::BackstopDue;
        }
        Replay::Composer(composer)
    }

    /// Forget every pane's last capture, so the next pass captures each pane as
    /// the observer did before F258: the old behaviour, for an equivalence test.
    #[cfg(test)]
    fn forget_captures(&self) {
        for state in self.panes.lock().expect("pane observer mutex").values_mut() {
            state.last = None;
        }
    }

    fn sink_path(&self, pane: &str) -> PathBuf {
        pij_tmux::tap_sink_path(&self.sink_dir, pane)
    }
}

/// Owned handle for the observer task and every tap it attached.
pub struct PaneObserverLoop {
    stop: oneshot::Sender<()>,
    joined: JoinHandle<()>,
    observer: Arc<PaneObserver>,
}

impl PaneObserverLoop {
    /// Stop future observations, join an in-flight observation, then detach every
    /// tap created by this observer.
    ///
    /// # Errors
    /// Returns [`PijError::Adapter`] if the task panicked or a tap could not be
    /// detached.
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.stop.send(());
        self.joined.await.map_err(|error| {
            observer_error(format!("pane observer task did not stop cleanly: {error}"))
        })?;
        self.observer.detach_all().await
    }
}

fn observer_error(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/pane-observer".to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use pij_core::model::{Harness, Pane, SeatDescriptor, SeatId};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use async_trait::async_trait;

    use pij_testkit::fakes::{FakeRegistry, FakeTmux};
    use pij_tmux::TmuxAdapter;

    use super::*;
    static NEXT_TMUX_SESSION: AtomicU64 = AtomicU64::new(0);

    fn pane(id: &str, x: u32, y: u32) -> Pane {
        Pane {
            id: id.to_string(),
            session: "pij".to_string(),
            window: "agent".to_string(),
            title: String::new(),
            cursor_x: Some(x),
            cursor_y: Some(y),
        }
    }

    fn framed(composer: &str) -> String {
        format!("transcript\n────────────\n❯ {composer}\n────────────")
    }

    async fn registry(panes: &[&str]) -> Arc<FakeRegistry> {
        let registry = Arc::new(FakeRegistry::new());
        for (index, pane) in panes.iter().enumerate() {
            let mut seat = SeatDescriptor::new(
                format!("pij-observed-{index}"),
                Harness::Pi,
                "/tmp/pij-observer",
            );
            seat.pane = Some((*pane).to_string());
            registry.put(seat).await.expect("register observed pane");
        }
        registry
    }

    struct FailFirstListRegistry {
        inner: Arc<FakeRegistry>,
        fail: AtomicBool,
    }

    #[async_trait]
    impl Registry for FailFirstListRegistry {
        async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
            self.inner.get(seat).await
        }

        async fn put(&self, descriptor: SeatDescriptor) -> Result<pij_core::model::Seq> {
            self.put_reporting(descriptor).await.map(|(seq, _)| seq)
        }

        async fn put_reporting(
            &self,
            descriptor: SeatDescriptor,
        ) -> Result<(pij_core::model::Seq, pij_core::ports::PutBinding)> {
            self.inner.put_reporting(descriptor).await
        }

        async fn put_reporting_keeping_parent(
            &self,
            descriptor: SeatDescriptor,
        ) -> Result<(pij_core::model::Seq, pij_core::ports::PutBinding)> {
            self.inner.put_reporting_keeping_parent(descriptor).await
        }

        async fn list(&self, filter: SeatFilter) -> Result<Vec<SeatDescriptor>> {
            if self.fail.swap(false, Ordering::SeqCst) {
                return Err(observer_error("injected registry boot race"));
            }
            self.inner.list(filter).await
        }

        async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<pij_core::model::Seq> {
            self.inner.tombstone(seat, reason).await
        }

        async fn tombstone_if_unchanged(
            &self,
            expected: SeatDescriptor,
            reason: String,
        ) -> Result<pij_core::model::Seq> {
            self.inner.tombstone_if_unchanged(expected, reason).await
        }

        async fn set_activity(
            &self,
            seat: &SeatId,
            state: pij_core::model::SystemState,
            reason: Option<&str>,
        ) -> Result<Option<pij_core::model::Seq>> {
            self.inner.set_activity(seat, state, reason).await
        }
    }

    /// Plan 185 F5: the orphaned-tap sweep runs on the first observation of a
    /// boot, and only then: it is not per-tick work.
    #[tokio::test]
    async fn the_orphan_sweep_runs_once_per_boot_not_per_tick() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .script_capture(framed(""))
                .script_capture(framed(""))
                .script_capture(framed("")),
        );
        let registry = registry(&["%1"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-sweep"),
            Duration::from_millis(1),
        )
        .expect("observer");
        for _ in 0..3 {
            observer.observe_once().await.expect("observation");
        }
        let sweeps = tmux
            .calls()
            .iter()
            .filter(|call| *call == "sweep_orphan_taps")
            .count();
        assert_eq!(sweeps, 1, "one sweep per boot");
    }

    /// F112: a pane whose tap attach is refused is retried with a backoff that
    /// doubles from the interval up to `ATTACH_BACKOFF_MAX`, not every tick. It
    /// stays unknown meanwhile, is listed for the PA digest with the refusal,
    /// and is dropped when it leaves the observed set.
    #[tokio::test(start_paused = true)]
    async fn a_refused_attach_backs_off_and_is_listed_for_the_pa() {
        let interval = Duration::from_millis(10);
        // A tap with another sink: the fake refuses our attach every time.
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_attached_tap("%1"),
        );
        let registry = registry(&["%1"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let refusals = Arc::new(TapRefusals::new());
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-backoff"),
            interval,
        )
        .expect("observer")
        .with_tap_refusals(Arc::clone(&refusals));
        let attaches = || {
            tmux.calls()
                .iter()
                .filter(|call| call.starts_with("attach_pane_tap:%1:"))
                .count()
        };
        // 100 ticks over one second: retries at +20, +40, +80, +160, +320 and
        // +640 ms of backoff, so six attempts, not a hundred.
        for _ in 0..100 {
            let _ = observer.observe_once().await;
            tokio::time::advance(interval).await;
        }
        assert_eq!(attaches(), 6, "{:?}", tmux.calls());
        let listed = refusals.list();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0].pane, "%1");
        assert!(
            listed[0].detail.contains("different tap sink"),
            "{listed:?}"
        );

        // The wait is bounded: after the cap, one attempt a minute.
        tokio::time::advance(ATTACH_BACKOFF_MAX).await;
        let _ = observer.observe_once().await;
        assert_eq!(attaches(), 7);

        // A pane that leaves the observed set leaves the record with it.
        registry
            .tombstone(&SeatId::from("pij-observed-0"), "gone")
            .await
            .expect("tombstone");
        let _ = observer.observe_once().await;
        assert!(refusals.list().is_empty(), "{:?}", refusals.list());
    }

    /// PR #99 review S1: a pane that keeps failing keeps the world unreconciled,
    /// and the sweep used to ride that flag: a sweep (and a ledger rewrite)
    /// every 600 ms. It has its own once-flag now.
    #[tokio::test]
    async fn a_failing_pane_does_not_make_the_sweep_run_every_tick() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .script_tap_error("drain one")
                .script_tap_error("drain two")
                .script_tap_error("drain three"),
        );
        let registry = registry(&["%1"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-sweep-failing"),
            Duration::from_millis(1),
        )
        .expect("observer");
        let errors = {
            let mut errors = 0;
            for _ in 0..3 {
                if observer.observe_once().await.is_err() {
                    errors += 1;
                }
            }
            errors
        };
        let sweeps = tmux
            .calls()
            .iter()
            .filter(|call| *call == "sweep_orphan_taps")
            .count();
        assert_eq!((errors, sweeps), (3, 1), "errors, sweeps");
    }

    #[tokio::test]
    async fn decision_table_keeps_unknown_and_stale_nonblank_blocked_until_clear() {
        // A capture drains the tap before and after; a quiet pane whose screen
        // did not move is replayed with one drain and no capture (F258), and a
        // screen only changes with tap bytes.
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .script_tap(b"typed".to_vec())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(b"cleared".to_vec())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_capture(framed("hello"))
                .script_capture(framed(""))
                .script_capture(framed("")),
        );
        let registry = registry(&["%1"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-decision"),
            Duration::from_millis(1),
        )
        .expect("observer");

        assert!(!gate.permits_injection("%never").await.expect("unknown"));
        observer.observe_once().await.expect("typing observation");
        assert!(!gate.permits_injection("%1").await.expect("typing blocks"));
        observer.observe_once().await.expect("stale observation");
        assert!(
            !gate
                .permits_injection("%1")
                .await
                .expect("stale draft blocks")
        );
        observer.observe_once().await.expect("clearing observation");
        assert!(
            !gate
                .permits_injection("%1")
                .await
                .expect("a blank redraw is not yet a clear")
        );
        observer.observe_once().await.expect("clear observation");
        assert!(gate.permits_injection("%1").await.expect("clear permits"));
        observer.detach_all().await.expect("detach");
    }

    #[tokio::test]
    async fn redraw_blank_preserves_pending_clear_until_a_quiet_pass_corroborates_it() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%108", 7, 2))
                // Each observation drains once before and once after capture.
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(b"clear redraw".to_vec())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(b"idle redraw".to_vec())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_capture(framed("draft"))
                .script_capture(framed(""))
                .script_capture(framed(""))
                .script_capture(framed(""))
                .script_capture(framed("")),
        );
        let registry = registry(&["%108"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-redraw-clear"),
            Duration::from_millis(1),
        )
        .expect("observer");

        observer.observe_once().await.expect("nonblank observation");
        assert!(!gate.permits_injection("%108").await.expect("draft blocks"));

        observer
            .observe_once()
            .await
            .expect("clear redraw observation");
        assert!(
            !gate
                .permits_injection("%108")
                .await
                .expect("redraw blank stays closed"),
            "an active redraw is not permission"
        );
        observer.observe_once().await.expect("clear corroboration");
        assert!(
            gate.permits_injection("%108")
                .await
                .expect("corroborated clear"),
            "a quiet blank after the pending transition must authorize injection"
        );

        observer
            .observe_once()
            .await
            .expect("idle redraw observation");
        assert!(
            !gate
                .permits_injection("%108")
                .await
                .expect("idle redraw stays closed"),
            "a redraw temporarily revokes even prior clear evidence"
        );
        observer
            .observe_once()
            .await
            .expect("idle redraw corroboration");
        assert!(
            gate.permits_injection("%108")
                .await
                .expect("re-corroborated clear"),
            "a quiet blank restores previously established clear evidence"
        );
        observer.detach_all().await.expect("detach");
    }

    struct LiveTmuxSession {
        name: String,
        server: String,
        tap_root: PathBuf,
    }

    impl LiveTmuxSession {
        fn start() -> Option<Self> {
            if !Command::new("tmux")
                .arg("-V")
                .output()
                .ok()?
                .status
                .success()
            {
                return None;
            }
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let count = NEXT_TMUX_SESSION.fetch_add(1, Ordering::Relaxed);
            let name = format!("pij-rs-observer-{}-{nonce}-{count}", std::process::id());
            let server = format!("{name}-server");
            let tap_root = std::env::temp_dir().join(format!("{name}-signals"));
            let session = Self {
                name,
                server,
                tap_root,
            };
            session.run([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                &session.name,
                "-x",
                "80",
                "-y",
                "24",
                "cat",
            ]);
            Some(session)
        }

        fn run<const N: usize>(&self, args: [&str; N]) -> std::process::Output {
            let output = Command::new("tmux")
                .args(["-L", &self.server])
                .args(args)
                .output()
                .expect("tmux became unavailable after fixture creation");
            assert!(
                output.status.success(),
                "isolated tmux fixture command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        }
    }

    impl Drop for LiveTmuxSession {
        fn drop(&mut self) {
            // F245: the teardown's postcondition is the server's own process
            // gone, never kill-server's status (it can exit non-zero while the
            // server goes, F27) and never a failing tmux command (a live server
            // with a full backlog refuses connections on Darwin). Only then do
            // its socket and the control client's attach lock go (F241).
            let pid = pij_testkit::tmux::server_pid(&self.server);
            let _ = Command::new("tmux")
                .args(["-L", &self.server, "kill-server"])
                .output();
            pij_testkit::tmux::finish_server(&pij_testkit::tmux::socket_path(&self.server), pid);
        }
    }

    #[tokio::test]
    async fn real_tmux_pane_observation_blocks_on_a_visible_composer() {
        let Some(session) = LiveTmuxSession::start() else {
            eprintln!("SKIP real pane observer test: tmux unavailable");
            return;
        };
        let tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        let pane = tmux
            .list_panes()
            .await
            .expect("list isolated pane")
            .into_iter()
            .next()
            .expect("isolated pane");
        let registry = registry(&[pane.id.as_str()]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("observer");

        observer.observe_once().await.expect("attach real tap");
        tmux.send_keys(&pane.id, "╰─ hello ─╯")
            .await
            .expect("draw composer in real pane");
        tokio::time::sleep(Duration::from_millis(100)).await;
        observer.observe_once().await.expect("observe real pane");

        assert!(
            !gate.permits_injection(&pane.id).await.expect("typing gate"),
            "visible real-pane composer must block injection"
        );
        observer.detach_all().await.expect("detach real tap");
    }

    #[tokio::test]
    async fn second_observer_pass_repairs_a_vanished_os_pipe_and_keeps_growing() {
        async fn wait_for_tap_bytes(sink: &std::path::Path, marker: &str) -> Vec<u8> {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let bytes = std::fs::read(sink).expect("read isolated tap bytes");
                    if String::from_utf8_lossy(&bytes).contains(marker) {
                        return bytes;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "tap {} never contained {marker:?}; observed {:?}",
                    sink.display(),
                    std::fs::read(sink)
                )
            })
        }

        let Some(session) = LiveTmuxSession::start() else {
            eprintln!("SKIP real pane observer test: tmux unavailable");
            return;
        };
        let tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        let pane = tmux
            .list_panes()
            .await
            .expect("list isolated pane")
            .into_iter()
            .next()
            .expect("isolated pane");
        let registry = registry(&[pane.id.as_str()]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            gate,
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("observer");

        observer.observe_once().await.expect("first observer pass");
        tmux.send_keys(&pane.id, "first-pass")
            .await
            .expect("first output");
        let sink = observer.sink_path(&pane.id);
        let first = wait_for_tap_bytes(&sink, "first-pass").await;

        // Reproduce the dogfood state: our offset record survives while tmux's
        // output pipe has vanished. The next daemon pass must repair, not trust,
        // that stale in-memory ownership claim.
        session.run(["pipe-pane", "-t", &pane.id]);
        // Command completion is not proof that the OS pipe is already gone.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let pipe = session.run(["display-message", "-p", "-t", &pane.id, "#{pane_pipe}"]);
                if String::from_utf8_lossy(&pipe.stdout).trim() == "0" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("isolated pipe must be physically absent before the repair pass");
        observer.observe_once().await.expect("second observer pass");
        tmux.send_keys(&pane.id, "second-pass")
            .await
            .expect("second output");
        let second = wait_for_tap_bytes(&sink, "second-pass").await;
        assert!(second.len() > first.len(), "tap must grow after pass two");
        assert!(
            second.starts_with(&first),
            "repair must preserve prior tap bytes"
        );

        observer.detach_all().await.expect("detach real tap");
    }

    #[tokio::test]
    async fn foreign_marker_cannot_authorize_pipe_or_file_deletion() {
        let Some(session) = LiveTmuxSession::start() else {
            eprintln!("SKIP real pane observer test: tmux unavailable");
            return;
        };
        let tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        let pane = tmux
            .list_panes()
            .await
            .expect("list isolated pane")
            .into_iter()
            .next()
            .expect("isolated pane");
        let victim = std::env::temp_dir().join(format!("{}-decoy.raw", session.name));
        std::fs::write(&victim, b"DO NOT DELETE").expect("create decoy file");
        let pipe_command = format!("cat >> '{}'", victim.display());
        session.run(["pipe-pane", "-O", "-t", &pane.id, &pipe_command]);
        session.run([
            "set-option",
            "-p",
            "-t",
            &pane.id,
            "@pij-tap-sink",
            victim.to_str().expect("UTF-8 decoy path"),
        ]);

        let registry = Arc::new(FakeRegistry::new());
        let observer = PaneObserver::new(
            registry as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>)),
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("observer");
        observer.observe_once().await.expect("world reconciliation");

        let pipe = session.run(["display-message", "-p", "-t", &pane.id, "#{pane_pipe}"]);
        assert_eq!(String::from_utf8_lossy(&pipe.stdout).trim(), "1");
        let marker = session.run([
            "show-options",
            "-p",
            "-v",
            "-q",
            "-t",
            &pane.id,
            "@pij-tap-sink",
        ]);
        assert_eq!(
            String::from_utf8_lossy(&marker.stdout).trim(),
            victim.to_str().expect("UTF-8 decoy path")
        );
        assert_eq!(
            std::fs::read(&victim).expect("decoy survives"),
            b"DO NOT DELETE"
        );

        session.run(["pipe-pane", "-t", &pane.id]);
        session.run(["set-option", "-p", "-u", "-t", &pane.id, "@pij-tap-sink"]);
        std::fs::remove_file(victim).expect("remove decoy");
    }

    #[tokio::test]
    async fn foreign_unmarked_pipe_refusal_is_reported_once_per_pane() {
        let Some(session) = LiveTmuxSession::start() else {
            eprintln!("SKIP real pane observer test: tmux unavailable");
            return;
        };
        let tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        let pane = tmux
            .list_panes()
            .await
            .expect("list isolated pane")
            .into_iter()
            .next()
            .expect("isolated pane");
        let victim = std::env::temp_dir().join(format!("{}-foreign.raw", session.name));
        std::fs::write(&victim, b"FOREIGN").expect("create foreign sink");
        let pipe_command = format!("cat >> '{}'", victim.display());
        session.run(["pipe-pane", "-O", "-t", &pane.id, &pipe_command]);

        let registry = registry(&[pane.id.as_str()]).await;
        let observer = PaneObserver::new(
            registry as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>)),
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("observer");

        observer
            .observe_once()
            .await
            .expect_err("first foreign refusal is reported");
        observer
            .observe_once()
            .await
            .expect("same refusal is suppressed on later ticks");
        let pipe = session.run(["display-message", "-p", "-t", &pane.id, "#{pane_pipe}"]);
        assert_eq!(String::from_utf8_lossy(&pipe.stdout).trim(), "1");
        assert_eq!(
            std::fs::read(&victim).expect("foreign sink survives"),
            b"FOREIGN"
        );

        session.run(["pipe-pane", "-t", &pane.id]);
        std::fs::remove_file(victim).expect("remove foreign sink");
    }

    #[tokio::test]
    async fn tombstoning_an_observed_seat_detaches_marker_and_captured_bytes() {
        let Some(session) = LiveTmuxSession::start() else {
            eprintln!("SKIP real pane observer test: tmux unavailable");
            return;
        };
        let tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        let pane = tmux
            .list_panes()
            .await
            .expect("list isolated pane")
            .into_iter()
            .next()
            .expect("isolated pane");
        let registry = registry(&[pane.id.as_str()]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            gate,
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("observer");
        observer.observe_once().await.expect("attach live tap");
        let sink = observer.sink_path(&pane.id);
        assert!(sink.exists());

        let mut descriptor = registry
            .get(&SeatId::from("pij-observed-0"))
            .await
            .expect("read seat")
            .expect("registered seat");
        descriptor.tombstoned_at = Some(1);
        descriptor.tombstone_reason = Some("released by reader".to_string());
        registry.put(descriptor).await.expect("persist tombstone");

        observer
            .observe_once()
            .await
            .expect("detach tombstoned tap");
        let pipe = session.run(["display-message", "-p", "-t", &pane.id, "#{pane_pipe}"]);
        assert_eq!(String::from_utf8_lossy(&pipe.stdout).trim(), "0");
        let marker = session.run([
            "show-options",
            "-p",
            "-v",
            "-q",
            "-t",
            &pane.id,
            "@pij-tap-sink",
        ]);
        assert!(marker.stdout.is_empty(), "release clears durable ownership");
        assert!(!sink.exists(), "release deletes captured terminal bytes");

        observer.observe_once().await.expect("post-release sweep");
        let pipe = session.run(["display-message", "-p", "-t", &pane.id, "#{pane_pipe}"]);
        assert_eq!(
            String::from_utf8_lossy(&pipe.stdout).trim(),
            "0",
            "a later sweep must not reattach a released seat"
        );
        assert!(!sink.exists());
    }

    #[tokio::test]
    async fn restarted_observer_retires_world_owned_tap_for_tombstoned_seat() {
        let Some(session) = LiveTmuxSession::start() else {
            eprintln!("SKIP real pane observer test: tmux unavailable");
            return;
        };
        let first_tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        let pane = first_tmux
            .list_panes()
            .await
            .expect("list isolated pane")
            .into_iter()
            .next()
            .expect("isolated pane");
        let registry = registry(&[pane.id.as_str()]).await;
        let first = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&first_tmux) as Arc<dyn TmuxPort>,
            Arc::new(InteractionGate::new(
                Arc::clone(&first_tmux) as Arc<dyn TmuxPort>
            )),
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("first observer");
        first
            .observe_once()
            .await
            .expect("attach first-process tap");
        let sink = first.sink_path(&pane.id);

        let mut descriptor = registry
            .get(&SeatId::from("pij-observed-0"))
            .await
            .expect("read seat")
            .expect("registered seat");
        descriptor.tombstoned_at = Some(1);
        descriptor.tombstone_reason = Some("released before crash".to_string());
        registry.put(descriptor).await.expect("persist tombstone");
        drop(first);
        drop(first_tmux);

        // Fresh process memory: no tracked pane exists. Only the marker in tmux
        // can source ownership and make consent withdrawal an active transition.
        let restarted_tmux = Arc::new(TmuxAdapter::for_server(&session.server, &session.tap_root));
        assert_eq!(
            restarted_tmux
                .pane_tap_sink(&pane.id)
                .await
                .expect("read durable owner"),
            Some(sink.clone()),
            "restart witness requires world-owned tap with no process-local state"
        );
        assert!(
            registry
                .get(&SeatId::from("pij-observed-0"))
                .await
                .expect("read tombstone")
                .is_some_and(|seat| seat.tombstoned_at.is_some()),
            "restart witness requires persisted consent withdrawal"
        );
        let restarting_registry = Arc::new(FailFirstListRegistry {
            inner: Arc::clone(&registry),
            fail: AtomicBool::new(true),
        });
        let restarted = PaneObserver::new(
            Arc::clone(&restarting_registry) as Arc<dyn Registry>,
            Arc::clone(&restarted_tmux) as Arc<dyn TmuxPort>,
            Arc::new(InteractionGate::new(
                Arc::clone(&restarted_tmux) as Arc<dyn TmuxPort>
            )),
            session.tap_root.clone(),
            Duration::from_millis(1),
        )
        .expect("restarted observer");
        let first_error = restarted
            .observe_once()
            .await
            .expect_err("first registry read fails");
        assert!(
            first_error
                .to_string()
                .contains("injected registry boot race")
        );
        assert_eq!(
            restarted_tmux
                .pane_tap_sink(&pane.id)
                .await
                .expect("marker survives failed reconciliation"),
            Some(sink.clone()),
            "an unreadable registry must not clear ownership or mark world reconciliation complete"
        );
        restarted
            .observe_once()
            .await
            .expect("world-sourced retirement retries");

        let pipe = session.run(["display-message", "-p", "-t", &pane.id, "#{pane_pipe}"]);
        assert_eq!(String::from_utf8_lossy(&pipe.stdout).trim(), "0");
        let marker = session.run([
            "show-options",
            "-p",
            "-v",
            "-q",
            "-t",
            &pane.id,
            "@pij-tap-sink",
        ]);
        assert!(marker.stdout.is_empty());
        assert!(!sink.exists(), "restart retirement deletes captured bytes");
    }

    #[tokio::test(start_paused = true)]
    async fn observer_loop_survives_one_tmux_error_and_processes_the_next_pane() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_pane(pane("%2", 2, 2))
                .script_tap_error("transient tap read")
                .script_tap(Vec::new())
                .script_capture(framed("")),
        );
        let registry = registry(&["%1", "%2"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = Arc::new(
            PaneObserver::new(
                Arc::clone(&registry) as Arc<dyn Registry>,
                Arc::clone(&tmux) as Arc<dyn TmuxPort>,
                Arc::clone(&gate),
                PathBuf::from("/tmp/pij-observer-loop"),
                Duration::from_millis(1),
            )
            .expect("observer"),
        );
        let loop_ = observer.start();
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        // The second pane is OBSERVED after the first pane's drain error — that is
        // the loop property. It is NOT permitted, because a first-sight blank
        // composer is not evidence of anything (review F2): only a watched
        // nonblank -> blank transition clears. The loop test asserts the loop kept
        // going, not that the observation reached a particular verdict.
        assert!(
            !gate
                .permits_injection("%2")
                .await
                .expect("second pane observed"),
            "the second pane must be OBSERVED after the first drain error, and a \
             first-sight blank must still block"
        );
        let calls = tmux.calls();
        assert!(calls.iter().any(|call| call == "drain_pane_tap:%1"));
        assert!(calls.iter().any(|call| call == "drain_pane_tap:%2"));
        loop_.shutdown().await.expect("shutdown");
    }

    /// A recognized blank is not permission on first sight. Two consecutive
    /// recognized blanks with a live, byte-free tap are positive quiet evidence.
    /// An unparsed layout is `None` and never enters this state machine.
    ///
    /// Mutation witness: permit the first recognized blank and the first
    /// assertion fails.
    #[tokio::test]
    async fn two_stable_recognized_blanks_are_required_to_permit_injection() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%9", 3, 2))
                // Two drains per observation; both passes are truly quiet.
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_capture(framed(""))
                .script_capture(framed("")),
        );
        let registry = registry(&["%9"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-stable-blank"),
            Duration::from_millis(1),
        )
        .expect("observer");

        observer
            .observe_once()
            .await
            .expect("first recognized blank");
        assert!(
            !gate.permits_injection("%9").await.expect("first sight"),
            "one recognized blank must not authorize injection"
        );
        observer
            .observe_once()
            .await
            .expect("second recognized blank");
        assert!(
            gate.permits_injection("%9").await.expect("stable quiet"),
            "two recognized blanks with no intervening tap bytes are positive quiet evidence"
        );
    }

    /// Tap activity between recognized blanks invalidates stability. A third
    /// quiet pass may corroborate the pending blank after the redraw.
    ///
    /// Mutation witness: ignore `tap_active` for a blank pass and the middle
    /// assertion fails.
    #[tokio::test]
    async fn tap_bytes_between_recognized_blanks_require_another_quiet_pass() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%10", 3, 2))
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(b"redraw".to_vec())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_capture(framed(""))
                .script_capture(framed(""))
                .script_capture(framed("")),
        );
        let registry = registry(&["%10"]).await;
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let observer = PaneObserver::new(
            Arc::clone(&registry) as Arc<dyn Registry>,
            Arc::clone(&tmux) as Arc<dyn TmuxPort>,
            Arc::clone(&gate),
            PathBuf::from("/tmp/pij-observer-tap-between-blanks"),
            Duration::from_millis(1),
        )
        .expect("observer");

        observer
            .observe_once()
            .await
            .expect("first recognized blank");
        observer.observe_once().await.expect("blank with tap bytes");
        assert!(
            !gate.permits_injection("%10").await.expect("active blank"),
            "tap bytes between blanks must keep the gate closed"
        );
        observer.observe_once().await.expect("quiet corroboration");
        assert!(gate.permits_injection("%10").await.expect("quiet blank"));
    }

    fn captures_of(tmux: &FakeTmux, pane: &str) -> usize {
        let prefix = format!("capture:{pane}:");
        tmux.calls()
            .iter()
            .filter(|call| call.starts_with(&prefix))
            .count()
    }

    async fn counted_observer(
        tmux: &Arc<FakeTmux>,
        gate: &Arc<InteractionGate>,
        panes: &[&str],
        sink: &str,
    ) -> (PaneObserver, Arc<ObserverCounts>) {
        let counts = Arc::new(ObserverCounts::new());
        let observer = PaneObserver::new(
            registry(panes).await as Arc<dyn Registry>,
            Arc::clone(tmux) as Arc<dyn TmuxPort>,
            Arc::clone(gate),
            PathBuf::from(sink),
            Duration::from_millis(1),
        )
        .expect("observer")
        .with_counts(Arc::clone(&counts));
        (observer, counts)
    }

    /// F258: a quiet pane is replayed, not captured; tap bytes or a moved
    /// cursor force a capture that same tick.
    ///
    /// Mutation witnesses: replay a pane that has tap bytes, and the third
    /// count fails; drop the cursor comparison from `replay`, and the fifth.
    #[tokio::test]
    async fn quiet_panes_replay_and_tap_bytes_or_a_moved_cursor_force_a_capture() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_standing_capture(framed("hello")),
        );
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let (observer, counts) =
            counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-replay").await;
        let mut seen = Vec::new();
        observer.observe_once().await.expect("first capture");
        seen.push(captures_of(&tmux, "%1"));
        observer.observe_once().await.expect("quiet pass");
        seen.push(captures_of(&tmux, "%1"));
        tmux.queue_tap(b"output".to_vec());
        observer.observe_once().await.expect("pass with tap bytes");
        seen.push(captures_of(&tmux, "%1"));
        observer.observe_once().await.expect("quiet pass");
        seen.push(captures_of(&tmux, "%1"));
        tmux.move_cursor("%1", 6, 2);
        observer
            .observe_once()
            .await
            .expect("pass with a moved cursor");
        seen.push(captures_of(&tmux, "%1"));
        assert_eq!(seen, vec![1, 1, 2, 2, 3], "captures after each pass");
        let counts = counts.snapshot();
        assert_eq!(
            (
                counts.passes,
                counts.captures,
                counts.skipped,
                counts.backstop
            ),
            (5, 3, 2, 0),
            "{counts:?}"
        );
        assert_eq!(
            counts.capture_bytes,
            3 * framed("hello").len() as u64,
            "{counts:?}"
        );
    }

    /// F258: a tap that died (or a pane that redraws without writing) cannot
    /// hide a screen change for longer than [`CAPTURE_BACKSTOP`].
    ///
    /// Mutation witness: replay without the backstop check, and the change is
    /// never seen.
    #[tokio::test(start_paused = true)]
    async fn the_capture_backstop_catches_a_screen_change_behind_a_dead_tap() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_standing_capture(framed("")),
        );
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let (observer, counts) =
            counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-backstop").await;
        let tick = Duration::from_millis(600);
        // A first blank is only a baseline; the second corroborates it.
        observer.observe_once().await.expect("baseline");
        tokio::time::advance(tick).await;
        observer.observe_once().await.expect("corroborated blank");
        assert!(gate.permits_injection("%1").await.expect("clear permits"));
        let last_capture = tokio::time::Instant::now();

        // The screen now shows a draft, and the tap says nothing.
        tmux.set_standing_capture(framed("hello"));
        let mut caught = None;
        for pass in 1..=60_u32 {
            tokio::time::advance(tick).await;
            observer.observe_once().await.expect("silent pass");
            if !gate.permits_injection("%1").await.expect("gate") {
                caught = Some(tokio::time::Instant::now() - last_capture);
                break;
            }
            assert!(pass < 60, "a dead tap hid the draft for {pass} passes");
        }
        let caught = caught.expect("caught");
        assert!(
            caught >= CAPTURE_BACKSTOP && caught < CAPTURE_BACKSTOP + tick,
            "caught after {caught:?}"
        );
        assert_eq!(counts.snapshot().backstop, 1, "{:?}", counts.snapshot());
    }

    /// F258: the observer's replays never stand in for the send boundary's own
    /// look. An injection drains the tap and captures the screen itself, so a
    /// draft the observer has not seen yet still defers it.
    ///
    /// Mutation witness: let `fresh_injection_verdict` answer from the gate's
    /// held state when it is certain (skipping its capture), and this fails.
    #[tokio::test]
    async fn an_injection_takes_its_own_capture_while_the_observer_replays() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_standing_capture(framed("")),
        );
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let (observer, counts) =
            counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-fresh").await;
        observer.observe_once().await.expect("baseline");
        observer.observe_once().await.expect("corroborated blank");
        tmux.set_standing_capture(framed("hello"));
        observer.observe_once().await.expect("replayed pass");
        assert_eq!(counts.snapshot().skipped, 1, "{:?}", counts.snapshot());
        let before = captures_of(&tmux, "%1");
        assert!(
            !gate
                .permits_injection_fresh("%1")
                .await
                .expect("fresh verdict"),
            "the fresh capture must see the draft the replay did not"
        );
        assert_eq!(captures_of(&tmux, "%1"), before + 1, "one fresh capture");
    }

    /// F258 open point (a): a replay is the same `decide` call a repeated
    /// capture of the unchanged screen made, on the gate's clock, so idle
    /// ageing (`expire_vetoes_if_idle`) and every other gate field come out the
    /// same. One observer replays; its twin forgets its captures every pass,
    /// which is the observer before F258. Over 72 s with a 20 s idle window
    /// (a draft typed, aged, cleared, aged), the gates agree after every pass.
    ///
    /// Mutation witnesses: skip `decide` on a replay, or replay with
    /// `tap_active` true, and the gates part.
    #[tokio::test(start_paused = true)]
    async fn a_replayed_verdict_leaves_the_gate_as_a_repeated_capture_did() {
        let clock: Arc<dyn Fn() -> std::time::Instant + Send + Sync> =
            Arc::new(|| tokio::time::Instant::now().into_std());
        let twin = || async {
            let tmux = Arc::new(
                FakeTmux::new()
                    .with_pane(pane("%1", 7, 2))
                    .with_standing_capture(framed("")),
            );
            let gate = Arc::new(InteractionGate::with_clock(
                Arc::clone(&tmux) as Arc<dyn TmuxPort>,
                Duration::from_secs(20),
                pij_core::delivery::DEFAULT_TYPING_GRACE_MS,
                Arc::clone(&clock),
            ));
            let (observer, counts) =
                counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-twin").await;
            (tmux, gate, observer, counts)
        };
        let (tmux_new, gate_new, replaying, counts_new) = twin().await;
        let (tmux_old, gate_old, capturing, counts_old) = twin().await;
        for pass in 0..120_u32 {
            let screen = match pass {
                5 => Some(("typed", framed("hello"))),
                60 => Some(("cleared", framed(""))),
                _ => None,
            };
            if let Some((bytes, screen)) = screen {
                for tmux in [&tmux_new, &tmux_old] {
                    tmux.queue_tap(bytes.as_bytes().to_vec());
                    tmux.set_standing_capture(screen.clone());
                }
            }
            capturing.forget_captures();
            replaying.observe_once().await.expect("replaying pass");
            capturing.observe_once().await.expect("capturing pass");
            assert_eq!(
                gate_new.observed("%1"),
                gate_old.observed("%1"),
                "pass {pass}"
            );
            assert_eq!(
                replaying.knowledge("%1"),
                capturing.knowledge("%1"),
                "pass {pass}"
            );
            tokio::time::advance(Duration::from_millis(600)).await;
        }
        let (new, old) = (counts_new.snapshot(), counts_old.snapshot());
        assert!(new.skipped > 100 && old.skipped == 0, "{new:?} {old:?}");
    }

    /// F266 N1 (gull's #313 note): a quiet pane is replayed only while the
    /// gate still holds the composer its last capture recorded, with no
    /// uncertainty. Another recorder changing either forces a capture that
    /// tick.
    ///
    /// Mutation witness: replay whether or not the gate holds it.
    #[tokio::test]
    async fn a_gate_another_recorder_changed_forces_a_capture() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_standing_capture(framed("hello")),
        );
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let (observer, _) =
            counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-gate-holds").await;
        let mut seen = Vec::new();
        observer.observe_once().await.expect("first capture");
        seen.push(captures_of(&tmux, "%1"));
        observer.observe_once().await.expect("quiet pass");
        seen.push(captures_of(&tmux, "%1"));
        gate.observe_composer("%1", "a draft another reader saw");
        observer
            .observe_once()
            .await
            .expect("pass after a new composer");
        seen.push(captures_of(&tmux, "%1"));
        observer.observe_once().await.expect("quiet pass");
        seen.push(captures_of(&tmux, "%1"));
        gate.record_unknown("%1");
        observer
            .observe_once()
            .await
            .expect("pass after uncertainty");
        seen.push(captures_of(&tmux, "%1"));
        assert_eq!(seen, vec![1, 1, 2, 2, 3], "captures after each pass");
    }

    /// F266 N2: an injection's fresh read drains the tap and then fails (here
    /// its pane listing), so the gate learns nothing, but the bytes it took
    /// never reach the observer. The next tick must capture, not replay the
    /// blank it last saw.
    ///
    /// Mutation witness: replay without comparing the gate's drain count.
    #[tokio::test]
    async fn a_fresh_read_that_drained_then_failed_forces_a_capture() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_standing_capture(framed("")),
        );
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let (observer, counts) =
            counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-fresh-fail").await;
        observer.observe_once().await.expect("baseline");
        observer.observe_once().await.expect("corroborated blank");
        observer.observe_once().await.expect("replayed pass");
        assert_eq!(counts.snapshot().skipped, 1, "{:?}", counts.snapshot());
        assert!(gate.permits_injection("%1").await.expect("clear permits"));

        tmux.queue_tap(b"typed".to_vec());
        tmux.set_standing_capture(framed("hello"));
        tmux.fail_list_panes(1);
        assert!(
            gate.permits_injection_fresh("%1").await.is_err(),
            "the fresh read fails after its drain"
        );
        let before = captures_of(&tmux, "%1");
        observer.observe_once().await.expect("pass after the drain");
        assert_eq!(
            captures_of(&tmux, "%1"),
            before + 1,
            "captured, not replayed"
        );
        assert!(
            !gate.permits_injection("%1").await.expect("gate"),
            "the draft the fresh read never saw now holds the gate"
        );
    }

    /// F266 N2: a tap whose pipe vanished and was started again silently
    /// missed what the pane wrote meanwhile; the tick that repairs it
    /// captures instead of replaying.
    ///
    /// Mutation witness: treat a repaired attach like a kept one.
    #[tokio::test]
    async fn a_repaired_tap_forces_a_capture() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .with_standing_capture(framed("")),
        );
        let gate = Arc::new(InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>));
        let (observer, counts) =
            counted_observer(&tmux, &gate, &["%1"], "/tmp/pij-observer-repaired").await;
        observer.observe_once().await.expect("baseline");
        observer.observe_once().await.expect("corroborated blank");
        observer.observe_once().await.expect("replayed pass");
        assert_eq!(counts.snapshot().skipped, 1, "{:?}", counts.snapshot());

        tmux.set_standing_capture(framed("hello"));
        tmux.lose_tap("%1");
        let before = captures_of(&tmux, "%1");
        observer.observe_once().await.expect("repairing pass");
        assert_eq!(
            captures_of(&tmux, "%1"),
            before + 1,
            "captured, not replayed"
        );
        assert!(
            !gate.permits_injection("%1").await.expect("gate"),
            "the draft written while the pipe was gone now holds the gate"
        );
    }
}
