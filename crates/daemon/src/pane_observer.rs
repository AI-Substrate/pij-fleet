//! Long-lived tmux pane observation feeding the interaction safety gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pij_core::error::{PijError, Result};
use pij_core::ports::{Registry, SeatFilter, TmuxPort};
use pij_harnesses::{ComposerRegion, InteractionGate, composer_region};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};

const CAPTURE_LINES: u32 = u32::MAX;

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
    interval: Duration,
    panes: Mutex<BTreeMap<String, PaneState>>,
    reported_attach_failures: Mutex<BTreeSet<(String, String)>>,
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
            interval,
            reported_attach_failures: Mutex::new(BTreeSet::new()),
            panes: Mutex::new(BTreeMap::new()),
        })
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
        let seats = match self.registry.list(SeatFilter::default()).await {
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

        for pane in panes {
            let tracked = self.is_tracked(&pane.id);
            let sink = self.sink_path(&pane.id);
            match self.tmux.attach_pane_tap(&pane.id, &sink).await {
                Ok(()) if !tracked => {
                    self.panes.lock().expect("pane observer mutex").insert(
                        pane.id.clone(),
                        PaneState {
                            knowledge: ComposerKnowledge::Unknown,
                        },
                    );
                }
                Ok(()) => {}
                Err(error) => {
                    self.gate.record_unknown(&pane.id);
                    let signature = (pane.id.clone(), error.to_string());
                    if self
                        .reported_attach_failures
                        .lock()
                        .expect("pane observer refusal mutex")
                        .insert(signature)
                    {
                        first_error.get_or_insert(error);
                    }
                    continue;
                }
            }

            let bytes = match self.tmux.drain_pane_tap(&pane.id).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.set_unknown(&pane.id);
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let capture = match self.tmux.capture(&pane.id, CAPTURE_LINES).await {
                Ok(capture) => capture,
                Err(error) => {
                    self.set_unknown(&pane.id);
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let trailing_bytes = match self.tmux.drain_pane_tap(&pane.id).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.set_unknown(&pane.id);
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let tap_active = !bytes.is_empty() || !trailing_bytes.is_empty();
            let (Some(cursor_x), Some(cursor_y)) = (pane.cursor_x, pane.cursor_y) else {
                self.set_unknown(&pane.id);
                continue;
            };
            let ComposerRegion::Recognized(composer) =
                composer_region(&capture, cursor_x, cursor_y)
            else {
                self.set_unknown(&pane.id);
                continue;
            };

            let blank = composer.chars().all(char::is_whitespace);
            let previous = self.knowledge(&pane.id);
            if blank && tap_active {
                if previous != ComposerKnowledge::Unknown {
                    self.set_knowledge(&pane.id, ComposerKnowledge::PendingClear);
                    self.gate.record_unknown(&pane.id);
                } else {
                    self.set_unknown(&pane.id);
                }
                continue;
            }
            if blank {
                if matches!(
                    previous,
                    ComposerKnowledge::Blank
                        | ComposerKnowledge::NonBlank
                        | ComposerKnowledge::PendingClear
                ) {
                    self.gate.record_composer_cleared(&pane.id);
                } else {
                    self.gate.record_unknown(&pane.id);
                    self.set_knowledge(&pane.id, ComposerKnowledge::Blank);
                    continue;
                }
                self.set_knowledge(&pane.id, ComposerKnowledge::Blank);
            } else {
                if tap_active {
                    self.gate.record_tap(&pane.id);
                }
                self.gate.observe_composer(&pane.id, &composer);
                self.set_knowledge(&pane.id, ComposerKnowledge::NonBlank);
            }
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

    #[tokio::test]
    async fn decision_table_keeps_unknown_and_stale_nonblank_blocked_until_clear() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane("%1", 7, 2))
                .script_tap(b"typed".to_vec())
                .script_tap(Vec::new())
                .script_tap(Vec::new())
                .script_capture(framed("hello"))
                .script_capture(framed("hello"))
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
            let output = Command::new("tmux")
                .args(["-L", &self.server, "kill-server"])
                .output();
            assert!(
                output.is_ok_and(|output| output.status.success()) || std::thread::panicking(),
                "failed to tear down isolated tmux server {}",
                self.server
            );
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
}
