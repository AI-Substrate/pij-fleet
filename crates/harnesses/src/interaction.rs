use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pij_core::delivery::{DEFAULT_TYPING_GRACE_MS, DeliveryDeferralReason};
use pij_core::error::{PijError, Result};
use pij_core::framing::self_injection_matches;
use pij_core::ports::{StagedSubmit, TmuxPort};
use sha2::{Digest, Sha256};

use crate::composer::{ComposerRegion, composer_region};

const DEFAULT_INTERACTION_IDLE: Duration = Duration::from_secs(60);
const SELF_INJECTION_WINDOW: Duration = Duration::from_secs(2);
const CAPTURE_LINES: u32 = u32::MAX;

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Debug)]
struct SelfInjection {
    payload: String,
    baseline: Option<String>,
    expires_at: Instant,
    tap_active: bool,
    last_interaction_at: Option<Instant>,
}

#[derive(Debug, Default)]
struct ComposerState {
    tap_active: bool,
    composer_active: bool,
    content: Option<String>,
    last_interaction_at: Option<Instant>,
    // Composer-edit evidence only: output taps and tmux mode never refresh this.
    last_edit_at: Option<Instant>,
    uncertainty_since: Option<Instant>,
    self_injection: Option<SelfInjection>,
}

impl ComposerState {
    // Opt-in, hash-only diagnostics: set PIJ_INTERACTION_TRACE to a pane id (or 1).
    // Capture callers share this state, so log the baseline before changing it.
    fn trace_edit(
        &self,
        pane: &str,
        content: Option<&str>,
        reason: &str,
        edit: Option<Instant>,
        now: Instant,
    ) {
        if !std::env::var("PIJ_INTERACTION_TRACE")
            .is_ok_and(|target| target == "1" || target == pane)
        {
            return;
        }
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let last_edit_at =
            edit.map(|at| at_ms.saturating_sub(now.saturating_duration_since(at).as_millis()));
        eprintln!(
            "{}",
            serde_json::json!({
                "trace": "composer-edit", "pane": pane, "at_ms": at_ms,
                "prev_content_hash": self.content.as_deref().map(short_sha256),
                "new_content_hash": content.map(short_sha256),
                "reason": reason, "last_edit_at": last_edit_at,
            })
        );
    }

    fn expire_vetoes_if_idle(&mut self, now: Instant, idle_window: Duration) -> bool {
        let recent_interaction = self
            .last_interaction_at
            .is_some_and(|observed_at| now.saturating_duration_since(observed_at) < idle_window);
        let recent_uncertainty = self
            .uncertainty_since
            .is_some_and(|observed_at| now.saturating_duration_since(observed_at) < idle_window);
        let stale = !recent_interaction && !recent_uncertainty;
        if stale {
            self.tap_active = false;
            self.composer_active = false;
            self.last_interaction_at = None;
        }
        stale
    }

    fn veto_active(&self) -> bool {
        self.content.is_none()
            || self.uncertainty_since.is_some()
            || self.tap_active
            || self.composer_active
    }
}
fn collapsed_eq(left: &str, right: &str) -> bool {
    left.split_whitespace().eq(right.split_whitespace())
}

fn short_sha256(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(text.as_bytes());
    let mut short = String::with_capacity(12);
    for byte in &digest[..6] {
        short.push(HEX[(byte >> 4) as usize] as char);
        short.push(HEX[(byte & 0x0f) as usize] as char);
    }
    short
}

/// Fresh pane facts used by one delivery decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectionVerdict {
    /// Existing interaction policy permits an injection now.
    pub permitted: bool,
    /// The freshly captured composer contains no non-whitespace text.
    pub composer_idle: bool,
    /// Named reason when either gate vetoes delivery.
    pub reason: Option<DeliveryDeferralReason>,
    /// Short SHA-256 fingerprint of a recognized non-blank draft.
    pub draft_sha: Option<String>,
    /// Time remaining until the observed edit's grace expires, not a timestamp.
    /// Callers may add this duration to their own clock; only HumanTyping sets it.
    pub next_retry_at: Option<Duration>,
    /// Elapsed time since the last observed real edit, for caller-clock receipts.
    pub last_edit_age: Option<Duration>,
}

/// Outcome of a pane transaction whose input was owned before observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StagedSubmission {
    /// The owned composer was empty, then the staged text was committed.
    Submitted,
    /// The owned composer was not safe to type into; nothing was staged.
    Deferred {
        /// Why input ownership could not safely be used.
        reason: DeliveryDeferralReason,
        /// Short SHA-256 fingerprint of the recognized draft, when present.
        draft_sha: Option<String>,
    },
}

/// Tracks composer edits separately from tmux-mode, tap and recognition latches.
/// Fresh recognized verdicts use only edit recency for `HumanTyping`; an unchanged
/// draft remains context, not an indefinite veto. Tap bytes are rendered output,
/// never keystrokes. Unknown-layout/legacy latches retain their independent idle
/// window. Consumers feed composer observations before asking; first observed
/// non-blank content is conservatively treated as an edit at observation time.
pub struct InteractionGate {
    tmux: Arc<dyn TmuxPort>,
    panes: Mutex<BTreeMap<String, ComposerState>>,
    idle_window: Duration,
    typing_grace: Duration,
    clock: Clock,
}

struct SelfInjectionScope<'a> {
    gate: &'a InteractionGate,
    pane: String,
}

impl Drop for SelfInjectionScope<'_> {
    fn drop(&mut self) {
        self.gate.clear_self_injection(&self.pane);
    }
}

struct PaneTransaction<'a> {
    gate: &'a InteractionGate,
    staged: StagedSubmit,
    self_injection: Option<SelfInjectionScope<'a>>,
}

impl<'a> PaneTransaction<'a> {
    fn new(gate: &'a InteractionGate, staged: StagedSubmit) -> Self {
        Self {
            gate,
            staged,
            self_injection: None,
        }
    }

    async fn abort(self) -> Result<()> {
        self.gate.tmux.abort_submit(&self.staged).await
    }

    async fn stage_and_commit(mut self, text: &str) -> Result<StagedSubmission> {
        self.self_injection = Some(self.gate.self_injection_scope(&self.staged.pane, text));
        self.gate.tmux.stage_submit(&mut self.staged, text).await?;
        self.gate.tmux.commit_submit(&self.staged).await?;
        Ok(StagedSubmission::Submitted)
    }
}

impl InteractionGate {
    pub fn new(tmux: Arc<dyn TmuxPort>) -> Self {
        Self::with_idle_window(tmux, DEFAULT_INTERACTION_IDLE)
    }

    /// Select the legacy idle window while using the core-owned typing default.
    pub fn with_idle_window(tmux: Arc<dyn TmuxPort>, idle_window: Duration) -> Self {
        Self::with_typing_grace(tmux, idle_window, DEFAULT_TYPING_GRACE_MS)
    }

    /// Construct with a daemon-resolved typing grace, in milliseconds.
    ///
    /// PM snap-in recipe for `crates/daemon/src/lib.rs`: resolve the same value
    /// supplied to the register response, then preserve the distinct idle window.
    /// This wiring belongs to the composer, not the harnesses unit.
    ///
    /// ```ignore
    /// let typing_grace_ms = std::env::var("PIJ_TYPING_GRACE_MS")
    ///     .ok()
    ///     .and_then(|value| value.parse::<u64>().ok())
    ///     .unwrap_or(pij_core::delivery::DEFAULT_TYPING_GRACE_MS);
    /// let interaction = Arc::new(InteractionGate::with_typing_grace(
    ///     Arc::clone(&tmux),
    ///     Duration::from_millis(config.interaction_idle_ms),
    ///     typing_grace_ms,
    /// ));
    /// ```
    ///
    /// Existing constructors remain unchanged and use the core default. Only the
    /// composition root needs this constructor to apply the daemon's env override.
    pub fn with_typing_grace(
        tmux: Arc<dyn TmuxPort>,
        idle_window: Duration,
        grace_ms: u64,
    ) -> Self {
        Self::with_clock(tmux, idle_window, grace_ms, Arc::new(Instant::now))
    }

    fn with_clock(
        tmux: Arc<dyn TmuxPort>,
        idle_window: Duration,
        grace_ms: u64,
        clock: Clock,
    ) -> Self {
        Self {
            tmux,
            panes: Mutex::new(BTreeMap::new()),
            idle_window,
            typing_grace: Duration::from_millis(grace_ms),
            clock,
        }
    }

    /// Record bytes from the pane tap. Activity blocks injection until the
    /// composer clears or the universal idle bound expires.
    pub fn record_tap(&self, pane: &str) {
        let now = (self.clock)();
        let mut panes = self.panes.lock().expect("interaction gate mutex");
        let state = panes.entry(pane.to_string()).or_default();
        state.tap_active = true;
        state.last_interaction_at = Some(now);
    }

    /// Record a payload about to be injected by pij. The same message across the
    /// legacy-to-pij-rs frame transition is exempt; any human change still holds.
    pub fn record_self_injection(&self, pane: &str, payload: &str) {
        let now = (self.clock)();
        let mut panes = self.panes.lock().expect("interaction gate mutex");
        let state = panes.entry(pane.to_string()).or_default();
        state.self_injection = Some(SelfInjection {
            payload: payload.to_string(),
            baseline: state.content.clone(),
            expires_at: now + SELF_INJECTION_WINDOW,
            tap_active: state.tap_active,
            last_interaction_at: state.last_interaction_at,
        });
    }

    fn self_injection_scope(&self, pane: &str, payload: &str) -> SelfInjectionScope<'_> {
        self.record_self_injection(pane, payload);
        SelfInjectionScope {
            gate: self,
            pane: pane.to_string(),
        }
    }

    fn clear_self_injection(&self, pane: &str) {
        if let Some(state) = self
            .panes
            .lock()
            .expect("interaction gate mutex")
            .get_mut(pane)
        {
            state.self_injection = None;
        }
    }

    /// Record the current composer contents. Whitespace-only content is empty:
    /// captured TUIs pad blank composers to their full width.
    pub fn observe_composer(&self, pane: &str, content: &str) {
        self.observe_composer_at(pane, content, false);
    }

    fn observe_composer_at(&self, pane: &str, content: &str, preserve_tap: bool) {
        let now = (self.clock)();
        let mut panes = self.panes.lock().expect("interaction gate mutex");
        let state = panes.entry(pane.to_string()).or_default();
        let tick_tap_active = state.tap_active;
        state.uncertainty_since = None;
        if state
            .self_injection
            .as_ref()
            .is_some_and(|injection| now > injection.expires_at)
        {
            state.self_injection = None;
        }
        if content.chars().all(char::is_whitespace) {
            let changed = state.content.as_deref() != Some(content);
            if changed {
                // Submit/clear is not active composition and must not start grace.
                state.trace_edit(
                    pane,
                    Some(content),
                    "recognized-clear",
                    state.last_edit_at,
                    now,
                );
                state.content = Some(content.to_string());
                state.last_interaction_at = Some(now);
            }
            state.composer_active = false;
            state.tap_active = preserve_tap && tick_tap_active;
            return;
        }

        if state.content.as_deref() != Some(content) {
            let injection = state.self_injection.take();
            let explained = injection.as_ref().is_some_and(|injection| {
                if injection.payload.chars().all(char::is_whitespace) {
                    return false;
                }
                self_injection_matches(&injection.payload, content)
                    || injection
                        .baseline
                        .as_deref()
                        .is_some_and(|baseline| collapsed_eq(content, baseline))
            });
            if let Some(injection) = injection.filter(|_| explained) {
                state.tap_active = injection.tap_active;
                state.last_interaction_at = injection.last_interaction_at;
            } else {
                state.composer_active = true;
                state.last_interaction_at = Some(now);
                state.trace_edit(pane, Some(content), "recognized-change", Some(now), now);
                state.last_edit_at = Some(now);
            }
            state.content = Some(content.to_string());
        }
        let expired = state.expire_vetoes_if_idle(now, self.idle_window);
        if preserve_tap && !expired {
            state.tap_active = tick_tap_active;
        }
    }

    /// Record a positively observed transition from a non-empty composer to a
    /// blank composer. This claims only that the composer cleared: Enter, cancel,
    /// and explicit deletion are intentionally indistinguishable here. Tmux mode
    /// must still independently report clear before injection is permitted.
    pub fn record_composer_cleared(&self, pane: &str) {
        let now = (self.clock)();
        let mut panes = self.panes.lock().expect("interaction gate mutex");
        let state = panes.entry(pane.to_string()).or_default();
        let changed = state.content.as_deref() != Some("");
        state.tap_active = false;
        state.composer_active = false;
        state.uncertainty_since = None;
        if changed {
            state.trace_edit(pane, Some(""), "explicit-clear", state.last_edit_at, now);
            state.content = Some(String::new());
            state.last_interaction_at = Some(now);
        }
    }

    /// Record that composer recognition is unavailable. The first observation
    /// starts a bounded uncertainty veto; repeated unknown classifications do not
    /// refresh either uncertainty or interaction age.
    pub fn record_unknown(&self, pane: &str) {
        let now = (self.clock)();
        let mut panes = self.panes.lock().expect("interaction gate mutex");
        let state = panes.entry(pane.to_string()).or_default();
        // Retain the last recognized baseline: recovery alone is not an edit.
        state.trace_edit(
            pane,
            state.content.as_deref(),
            "unrecognized",
            state.last_edit_at,
            now,
        );
        state.uncertainty_since.get_or_insert(now);
    }

    /// Re-drain the live tap and re-capture the composer at the send boundary.
    ///
    /// Unread tap bytes are consumed before capture so a just-typed key cannot
    /// hide behind renderer lag. An absent tap is absence of safety evidence and
    /// vetoes. Cursor discovery then adds one listing and one capture round-trip.
    pub async fn permits_injection_fresh(&self, pane: &str) -> Result<bool> {
        Ok(self.fresh_injection_verdict(pane).await?.permitted)
    }

    async fn refresh_live_tap(&self, pane: &str) -> Result<bool> {
        if self.tmux.pane_tap_sink(pane).await?.is_none() {
            return Ok(false);
        }
        // A pane tap contains rendered OUTPUT, not input events. Drain it to keep
        // the boundary fresh, but never translate output bytes into human typing.
        self.tmux.drain_pane_tap(pane).await?;
        Ok(true)
    }

    /// Re-capture once and expose both policy permission and actual composer state.
    ///
    /// Socket delivery uses `permitted`: it cannot splice text into the composer.
    /// Pane typing additionally requires `composer_idle`, even after the parked-
    /// draft policy veto has expired. Both facts come from this one capture.
    pub async fn fresh_injection_verdict(&self, pane: &str) -> Result<InjectionVerdict> {
        if !self.refresh_live_tap(pane).await? {
            return Ok(InjectionVerdict {
                permitted: false,
                composer_idle: false,
                reason: Some(DeliveryDeferralReason::TapUnowned),
                draft_sha: None,
                next_retry_at: None,
                last_edit_age: None,
            });
        }
        let listed = self.tmux.list_panes().await?;
        let Some(listed) = listed.into_iter().find(|listed| listed.id == pane) else {
            let permitted = self.permits_with_additional_veto(pane, true).await?;
            return Ok(InjectionVerdict {
                permitted,
                composer_idle: false,
                reason: Some(DeliveryDeferralReason::Unrecognized),
                draft_sha: None,
                next_retry_at: None,
                last_edit_age: None,
            });
        };
        let (Some(cursor_x), Some(cursor_y)) = (listed.cursor_x, listed.cursor_y) else {
            let permitted = self.permits_with_additional_veto(pane, true).await?;
            return Ok(InjectionVerdict {
                permitted,
                composer_idle: false,
                reason: Some(DeliveryDeferralReason::Unrecognized),
                draft_sha: None,
                next_retry_at: None,
                last_edit_age: None,
            });
        };
        let capture = self.tmux.capture(pane, CAPTURE_LINES).await?;
        match composer_region(&capture, cursor_x, cursor_y) {
            ComposerRegion::Recognized(content) => {
                let composer_idle = content.chars().all(char::is_whitespace);
                // This fresh, recognized capture is the authority. It clears any
                // historical tap latch because tap bytes are rendered output.
                self.observe_composer_at(pane, &content, false);
                let now = (self.clock)();
                let last_edit_at = self
                    .panes
                    .lock()
                    .expect("interaction gate mutex")
                    .get(pane)
                    .and_then(|state| state.last_edit_at);
                let last_edit_age =
                    last_edit_at.map(|last_edit| now.saturating_duration_since(last_edit));
                let next_retry_at = last_edit_age
                    .filter(|_| !composer_idle)
                    .and_then(|age| self.typing_grace.checked_sub(age))
                    .filter(|remaining| !remaining.is_zero());
                let human_typing = next_retry_at.is_some();
                Ok(InjectionVerdict {
                    permitted: !human_typing,
                    composer_idle,
                    reason: human_typing.then_some(DeliveryDeferralReason::HumanTyping),
                    draft_sha: (!composer_idle).then(|| short_sha256(&content)),
                    next_retry_at,
                    last_edit_age,
                })
            }
            ComposerRegion::Unrecognized => {
                let permitted = self.permits_with_additional_veto(pane, true).await?;
                Ok(InjectionVerdict {
                    permitted,
                    composer_idle: false,
                    reason: Some(DeliveryDeferralReason::Unrecognized),
                    draft_sha: None,
                    next_retry_at: None,
                    last_edit_age: None,
                })
            }
        }
    }

    /// Own pane input before observing, then stage and commit only into a blank composer.
    ///
    /// Ordering makes external-input races impossible: acquisition happens before
    /// the one fresh permission capture, and ownership lasts through Enter. This
    /// deliberately replaces the rejected post-stage rendered-pixel comparison.
    pub async fn submit_with_owned_input(
        &self,
        pane: &str,
        text: &str,
    ) -> Result<StagedSubmission> {
        let transaction = PaneTransaction::new(self, self.tmux.acquire_submit(pane).await?);
        let verdict = match self.fresh_injection_verdict(pane).await {
            Ok(verdict) => verdict,
            Err(error) => {
                return match transaction.abort().await {
                    Ok(()) => Err(error),
                    Err(recovery) => Err(PijError::Adapter {
                        adapter: "interaction-gate".to_string(),
                        message: format!(
                            "{error}; pane input acquisition was not released: {recovery}"
                        ),
                    }),
                };
            }
        };
        if !verdict.permitted || !verdict.composer_idle {
            transaction.abort().await?;
            return Ok(StagedSubmission::Deferred {
                reason: verdict
                    .reason
                    .unwrap_or(DeliveryDeferralReason::ComposerBusy),
                draft_sha: verdict.draft_sha,
            });
        }
        transaction.stage_and_commit(text).await
    }

    /// True only when both independent gates report clear.
    pub async fn permits_injection(&self, pane: &str) -> Result<bool> {
        self.permits_with_additional_veto(pane, false).await
    }

    async fn permits_with_additional_veto(
        &self,
        pane: &str,
        additional_veto: bool,
    ) -> Result<bool> {
        let mode_active = self.tmux.user_typing(pane).await?;
        let now = (self.clock)();
        let mut panes = self.panes.lock().expect("interaction gate mutex");
        let state = panes.entry(pane.to_string()).or_default();
        if state.content.is_none() || additional_veto {
            state.uncertainty_since.get_or_insert(now);
        }
        if mode_active {
            state.last_interaction_at = Some(now);
            return Ok(false);
        }
        if state.expire_vetoes_if_idle(now, self.idle_window) {
            return Ok(true);
        }
        Ok(!additional_veto && !state.veto_active())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use pij_core::model::Pane;
    use pij_core::ports::TmuxPort;

    use pij_testkit::block_on;
    use pij_testkit::fakes::FakeTmux;

    use super::{
        DEFAULT_TYPING_GRACE_MS, DeliveryDeferralReason, InteractionGate, PaneTransaction,
        StagedSubmission, short_sha256,
    };

    const RULE: &str = "────────────────────────────────";
    fn pane(cursor_x: u32, cursor_y: u32) -> Pane {
        Pane {
            id: "%108".to_string(),
            session: "pij".to_string(),
            window: "worker".to_string(),
            title: "worker".to_string(),
            cursor_x: Some(cursor_x),
            cursor_y: Some(cursor_y),
        }
    }

    #[test]
    fn injection_requires_both_tmux_and_composer_gates_clear() {
        for tmux_interaction in [false, true] {
            for composer_interaction in [false, true] {
                let tmux = if tmux_interaction {
                    FakeTmux::new().with_user_typing()
                } else {
                    FakeTmux::new()
                };
                let gate = InteractionGate::new(Arc::new(tmux));
                if composer_interaction {
                    gate.record_tap("%108");
                    gate.observe_composer("%108", "unfinished message");
                } else {
                    gate.observe_composer("%108", "   \t");
                }
                assert_eq!(
                    block_on(gate.permits_injection("%108")).expect("typing gate"),
                    !tmux_interaction && !composer_interaction,
                    "tmux_interaction={tmux_interaction}, composer_interaction={composer_interaction}"
                );
            }
        }
    }
    #[test]
    fn parked_draft_holds_at_most_the_idle_window_then_sends() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );

        gate.observe_composer("%108", "draft parked here yesterday");
        assert!(!block_on(gate.permits_injection("%108")).expect("fresh draft holds"));

        elapsed_ms.store(59_999, Ordering::Relaxed);
        assert!(!block_on(gate.permits_injection("%108")).expect("recent draft holds"));

        elapsed_ms.store(60_000, Ordering::Relaxed);
        assert!(block_on(gate.permits_injection("%108")).expect("parked draft expired"));

        elapsed_ms.store(120_000, Ordering::Relaxed);
        assert!(block_on(gate.permits_injection("%108")).expect("expired draft stays released"));
    }
    #[test]
    fn typing_through_restart_is_not_rebaselined_away() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );

        gate.observe_composer("%108", "half typed thou");
        elapsed_ms.store(10_000, Ordering::Relaxed);
        gate.observe_composer("%108", "half typed thou");
        assert!(!block_on(gate.permits_injection("%108")).expect("thinking pause holds"));

        elapsed_ms.store(11_000, Ordering::Relaxed);
        gate.observe_composer("%108", "half typed thought");
        elapsed_ms.store(70_999, Ordering::Relaxed);
        assert!(!block_on(gate.permits_injection("%108")).expect("resumed typing re-arms"));

        gate.observe_composer("%108", "  \t\n");
        assert!(block_on(gate.permits_injection("%108")).expect("blank releases"));
    }

    #[test]
    fn self_echo_never_acquires_a_hold() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
        gate.observe_composer("%108", "");
        gate.record_self_injection("%108", "[pij from boss] status please");
        gate.record_tap("%108");
        gate.observe_composer("%108", " [pij   from boss]\nstatus please ");

        assert!(block_on(gate.permits_injection("%108")).expect("self echo is exempt"));
    }

    #[test]
    fn legacy_frame_observed_as_pij_rs_frame_remains_a_self_echo() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
        gate.observe_composer("%108", "");
        gate.record_self_injection("%108", "[pij from boss] status please");
        gate.record_tap("%108");
        gate.observe_composer("%108", "[pij-rs from boss]\nstatus please\n[/pij]");

        assert!(block_on(gate.permits_injection("%108")).expect("generation transition is exempt"));
    }

    #[test]
    fn generation_transition_with_adjacent_human_text_acquires_a_hold() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
        gate.observe_composer("%108", "");
        gate.record_self_injection("%108", "[pij from boss] status please");
        gate.record_tap("%108");
        gate.observe_composer(
            "%108",
            "[pij-rs from boss]\nstatus please and my reply\n[/pij]",
        );

        assert!(!block_on(gate.permits_injection("%108")).expect("adjacent human text must hold"));
    }

    #[test]
    fn self_echo_never_destroys_an_active_human_hold() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
        gate.observe_composer("%108", "");
        gate.record_tap("%108");
        gate.observe_composer("%108", "human mid sentence");
        gate.record_self_injection("%108", "[pij from boss] barging in");
        gate.record_tap("%108");
        gate.observe_composer("%108", "[pij from boss] barging in");

        assert!(!block_on(gate.permits_injection("%108")).expect("human hold survives echo"));
    }

    #[test]
    fn self_echo_match_is_exact_not_a_substring() {
        for capture in [
            "[pij from boss] status please and my reply",
            "my reply [pij from boss] status please",
            "my [pij from boss] status please reply",
        ] {
            let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
            gate.observe_composer("%108", "");
            gate.record_self_injection("%108", "[pij from boss] status please");
            gate.record_tap("%108");
            gate.observe_composer("%108", capture);

            assert!(
                !block_on(gate.permits_injection("%108")).expect("adjacent human text holds"),
                "capture={capture:?}"
            );
        }
    }
    #[test]
    fn self_echo_exemption_is_one_shot() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
        let injected = "[pij from boss] status please";
        gate.observe_composer("%108", "");
        gate.record_self_injection("%108", injected);
        gate.record_tap("%108");
        gate.observe_composer("%108", injected);
        assert!(block_on(gate.permits_injection("%108")).expect("exact echo is exempt"));

        gate.record_tap("%108");
        gate.observe_composer("%108", "[pij from boss] status please and my reply");
        assert!(!block_on(gate.permits_injection("%108")).expect("later human input holds"));
    }

    #[test]
    fn expired_self_echo_exemption_does_not_hide_human_input() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "");
        gate.record_self_injection("%108", "[pij from boss] late");

        elapsed_ms.store(2_001, Ordering::Relaxed);
        gate.record_tap("%108");
        gate.observe_composer("%108", "[pij from boss] late");
        assert!(!block_on(gate.permits_injection("%108")).expect("expired exemption holds"));
    }
    #[test]
    fn fresh_boundary_replaces_a_stale_clear_composer_observation() {
        let capture = format!("output\n{RULE}\n❯ human typing\n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(100, 2))
                .with_attached_tap("%108")
                .script_capture(capture),
        );
        let gate = InteractionGate::new(tmux.clone());
        gate.observe_composer("%108", "");

        assert!(
            !block_on(gate.permits_injection_fresh("%108")).expect("fresh gate"),
            "a capture taken now must override a stale-clear observer state"
        );
        let calls = tmux.calls();
        assert!(calls.iter().any(|call| call == "list_panes"));
        assert!(calls.iter().any(|call| call == "capture:%108:4294967295"));
        assert!(calls.iter().any(|call| call == "drain_pane_tap:%108"));
    }

    #[test]
    fn fresh_boundary_unrecognized_layout_defers() {
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(3, 0))
                .with_attached_tap("%108")
                .script_capture("ordinary shell output"),
        );
        let gate = InteractionGate::new(tmux);
        gate.observe_composer("%108", "");

        assert!(!block_on(gate.permits_injection_fresh("%108")).expect("unknown defers"));
    }
    #[test]
    fn unrecognized_veto_expires_to_the_tmux_mode_verdict() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(3, 0))
                .with_attached_tap("%108")
                .script_capture("ordinary shell output")
                .script_capture("ordinary shell output"),
        );
        let gate = InteractionGate::with_clock(
            tmux,
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "");
        assert!(!block_on(gate.permits_injection_fresh("%108")).expect("fresh uncertainty vetoes"));

        elapsed_ms.store(60_000, Ordering::Relaxed);
        assert!(block_on(gate.permits_injection_fresh("%108")).expect("stale uncertainty expires"));
    }

    #[test]
    fn fresh_boundary_streamed_output_with_blank_composer_permits() {
        let capture = format!("agent output\n{RULE}\n❯ \n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(2, 2))
                .with_attached_tap("%108")
                .script_tap(b"streamed agent output".to_vec())
                .script_capture(capture),
        );
        let gate = InteractionGate::new(tmux.clone());
        gate.observe_composer("%108", "");

        assert!(
            block_on(gate.permits_injection_fresh("%108")).expect("blank composer permits"),
            "agent output from the pane tap is not evidence of human typing"
        );
        assert!(
            tmux.calls()
                .iter()
                .any(|call| call == "drain_pane_tap:%108")
        );
        assert!(block_on(gate.permits_injection("%108")).expect("blank state remains clear"));
    }

    #[test]
    fn fresh_boundary_recognized_blank_without_tap_permits() {
        let capture = format!("output\n{RULE}\n❯ \n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(2, 2))
                .with_attached_tap("%108")
                .script_capture(capture),
        );
        let gate = InteractionGate::new(tmux);

        assert!(block_on(gate.permits_injection_fresh("%108")).expect("blank permits"));
    }

    fn contract_typing_grace_ms() -> u64 {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../testkit/fixtures/delivery/hold-events.json"
        ))
        .expect("frozen typing-grace contract");
        let grace_ms = contract["grace"]["default_ms"]
            .as_u64()
            .expect("contract grace in milliseconds");
        assert_eq!(
            DEFAULT_TYPING_GRACE_MS, grace_ms,
            "core default matches frozen contract"
        );
        grace_ms
    }

    #[test]
    fn typing_grace_blank_or_submitted_composer_releases_immediately() {
        // A blank composer is authoritative; prior input must not hold a reply.
        for explicit_clear in [false, true] {
            for elapsed in [0, 5_000] {
                let elapsed_ms = Arc::new(AtomicU64::new(0));
                let started_at = Instant::now();
                let clock_ms = Arc::clone(&elapsed_ms);
                let gate = InteractionGate::with_clock(
                    Arc::new(FakeTmux::new().with_clear_composer("%108")),
                    Duration::from_secs(1),
                    contract_typing_grace_ms(),
                    Arc::new(move || {
                        started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))
                    }),
                );
                gate.observe_composer("%108", "draft being edited");
                elapsed_ms.store(elapsed, Ordering::Relaxed);
                if explicit_clear {
                    gate.record_composer_cleared("%108");
                }
                let verdict =
                    block_on(gate.fresh_injection_verdict("%108")).expect("blank verdict");
                assert!(
                    verdict.permitted,
                    "blank after {elapsed}ms, explicit_clear={explicit_clear}"
                );
                assert!(verdict.composer_idle);
                assert_eq!(verdict.reason, None);
                assert_eq!(verdict.draft_sha, None);
                assert_eq!(verdict.next_retry_at, None);
                assert_eq!(
                    gate.panes.lock().expect("state")["%108"].last_edit_at,
                    Some(started_at)
                );
            }
        }
    }

    #[test]
    fn typing_grace_recognition_gap_does_not_manufacture_an_edit() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let capture = format!("output\n{RULE}\n❯ parked draft\n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(14, 2))
                .with_attached_tap("%108")
                .script_capture(capture.clone())
                .script_capture("temporary redraw")
                .script_capture(capture),
        );
        let gate = InteractionGate::with_clock(
            tmux,
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        assert!(
            !block_on(gate.fresh_injection_verdict("%108"))
                .expect("first edit")
                .permitted
        );
        elapsed_ms.store(30_000, Ordering::Relaxed);
        gate.record_unknown("%108");
        assert_eq!(
            block_on(gate.fresh_injection_verdict("%108"))
                .expect("redraw")
                .reason,
            Some(DeliveryDeferralReason::Unrecognized)
        );
        elapsed_ms.store(61_000, Ordering::Relaxed);
        let verdict = block_on(gate.fresh_injection_verdict("%108")).expect("recovered draft");
        assert!(verdict.permitted, "recognition recovery is not an edit");
        assert_eq!(
            gate.panes.lock().expect("state")["%108"].last_edit_at,
            Some(started_at)
        );
        assert_eq!(verdict.draft_sha, Some(short_sha256("parked draft")));
        assert_eq!(verdict.next_retry_at, None);
    }

    #[test]
    fn typing_grace_real_edit_ten_seconds_ago_has_fifty_seconds_remaining() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let capture = format!("output\n{RULE}\n❯ edited draft\n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(14, 2))
                .with_attached_tap("%108")
                .script_capture(capture),
        );
        let gate = InteractionGate::with_clock(
            tmux,
            Duration::from_secs(1),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "old draft");
        elapsed_ms.store(70_000, Ordering::Relaxed);
        gate.observe_composer("%108", "edited draft");
        elapsed_ms.store(80_000, Ordering::Relaxed);
        let verdict = block_on(gate.fresh_injection_verdict("%108")).expect("recent edit");
        assert!(!verdict.permitted);
        assert_eq!(verdict.reason, Some(DeliveryDeferralReason::HumanTyping));
        assert_eq!(verdict.next_retry_at, Some(Duration::from_secs(50)));
    }

    #[test]
    fn typing_grace_parked_draft_releases_without_another_key() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let capture = format!("output\n{RULE}\n❯ parked draft\n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(14, 2))
                .with_attached_tap("%108")
                .script_capture(capture.clone())
                .script_capture(capture.clone())
                .script_capture(capture),
        );
        let gate = InteractionGate::with_clock(
            tmux,
            Duration::from_millis(contract_typing_grace_ms()),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "parked draft");

        // The live control still claimed human typing after 84.8 s and again at 200 s.
        for elapsed in [contract_typing_grace_ms() + 1_000, 84_800, 200_000] {
            elapsed_ms.store(elapsed, Ordering::Relaxed);
            let verdict = block_on(gate.fresh_injection_verdict("%108")).expect("parked verdict");
            assert!(
                verdict.permitted,
                "parked draft at {elapsed} ms is not a human at the keyboard"
            );
            assert!(!verdict.composer_idle);
            assert_eq!(verdict.reason, None);
            assert_eq!(verdict.draft_sha, Some(short_sha256("parked draft")));
            assert_eq!(verdict.next_retry_at, None);
        }
    }

    #[test]
    fn typing_grace_uses_injected_boundary_independently_of_idle_window() {
        for idle_ms in [1, 90_000] {
            for grace_ms in [0, 7_000] {
                let elapsed_ms = Arc::new(AtomicU64::new(0));
                let started_at = Instant::now();
                let clock_ms = Arc::clone(&elapsed_ms);
                let capture = format!("output\n{RULE}\n❯ draft\n{RULE}");
                let tmux = Arc::new(
                    FakeTmux::new()
                        .with_pane(pane(7, 2))
                        .with_attached_tap("%108")
                        .script_capture(capture.clone())
                        .script_capture(capture.clone())
                        .script_capture(capture),
                );
                let gate = InteractionGate::with_clock(
                    tmux,
                    Duration::from_millis(idle_ms),
                    grace_ms,
                    Arc::new(move || {
                        started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))
                    }),
                );
                gate.observe_composer("%108", "draft");
                for at in [grace_ms.saturating_sub(1), grace_ms, grace_ms + 1] {
                    elapsed_ms.store(at, Ordering::Relaxed);
                    let verdict = block_on(gate.fresh_injection_verdict("%108")).expect("boundary");
                    assert_eq!(
                        verdict.permitted,
                        at >= grace_ms,
                        "at={at}, grace={grace_ms}, idle={idle_ms}"
                    );
                    assert_eq!(
                        verdict.next_retry_at,
                        (at < grace_ms).then(|| Duration::from_millis(grace_ms - at))
                    );
                }
            }
        }
    }

    #[test]
    fn typing_grace_rendered_output_does_not_rearm_a_parked_draft() {
        let started_at = Instant::now();
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let clock_ms = Arc::clone(&elapsed_ms);
        let capture = format!("agent output\n{RULE}\n❯ parked draft\n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(14, 2))
                .with_attached_tap("%108")
                .script_tap(b"more agent output".to_vec())
                .script_capture(capture),
        );
        let gate = InteractionGate::with_clock(
            tmux,
            Duration::from_secs(1),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "parked draft");
        elapsed_ms.store(contract_typing_grace_ms() + 1_000, Ordering::Relaxed);
        gate.record_tap("%108");
        let verdict = block_on(gate.fresh_injection_verdict("%108")).expect("output verdict");
        assert!(verdict.permitted, "output bytes are not a fresh human edit");
        assert_eq!(verdict.reason, None);
        assert_eq!(verdict.next_retry_at, None);
    }

    #[test]
    fn typing_grace_self_echo_does_not_start_human_grace() {
        for prior_human_edit in [false, true] {
            let elapsed_ms = Arc::new(AtomicU64::new(0));
            let started_at = Instant::now();
            let clock_ms = Arc::clone(&elapsed_ms);
            let payload = "[pij from boss] status please";
            let capture = format!("output\n{RULE}\n❯ {payload}\n{RULE}");
            let blank = format!("output\n{RULE}\n❯ \n{RULE}");
            let tmux = Arc::new(
                FakeTmux::new()
                    .with_pane(pane(100, 2))
                    .with_attached_tap("%108")
                    .script_capture(capture)
                    .script_capture(blank),
            );
            let gate = InteractionGate::with_clock(
                tmux,
                Duration::from_secs(1),
                contract_typing_grace_ms(),
                Arc::new(move || {
                    started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))
                }),
            );
            gate.observe_composer("%108", "");
            if prior_human_edit {
                gate.observe_composer("%108", "old human draft");
            }
            elapsed_ms.store(contract_typing_grace_ms() + 1_000, Ordering::Relaxed);
            gate.record_self_injection("%108", payload);
            let verdict =
                block_on(gate.fresh_injection_verdict("%108")).expect("self echo verdict");
            assert!(verdict.permitted, "our own injection is not human typing");
            assert_eq!(verdict.reason, None);
            assert_eq!(verdict.next_retry_at, None);

            gate.record_composer_cleared("%108");
            let cleared = block_on(gate.fresh_injection_verdict("%108")).expect("self echo clear");
            assert!(
                cleared.permitted,
                "clearing our echo must not rearm an expired human edit"
            );
            assert_eq!(cleared.next_retry_at, None);
        }
    }

    #[test]
    fn fresh_verdict_expires_a_recognized_draft_after_typing_grace() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let capture = format!("output\n{RULE}\n❯ parked draft\n{RULE}");
        let tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(14, 2))
                .with_attached_tap("%108")
                .script_capture(capture.clone())
                .script_capture(capture),
        );
        let gate = InteractionGate::with_clock(
            tmux,
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );

        // RED control: a parked draft is not evidence of a human at the keyboard.
        // The former forever-veto assertion failed after the recency fix; the
        // draft fingerprint remains context after the typing veto expires.
        let fresh = block_on(gate.fresh_injection_verdict("%108")).expect("fresh draft");
        assert!(!fresh.permitted);
        assert!(!fresh.composer_idle);

        elapsed_ms.store(contract_typing_grace_ms(), Ordering::Relaxed);
        let stale = block_on(gate.fresh_injection_verdict("%108")).expect("stale draft");
        assert!(
            stale.permitted,
            "a parked draft is not a current human veto"
        );
        assert!(!stale.composer_idle);
        assert_eq!(stale.reason, None);
        assert_eq!(
            stale.draft_sha,
            Some(short_sha256("parked draft")),
            "the receipt can identify the exact draft without storing its text"
        );
    }

    #[test]
    fn typing_grace_baseline_echo_clear_preserves_provenance() {
        for explicit_clear in [false, true] {
            let elapsed_ms = Arc::new(AtomicU64::new(0));
            let started_at = Instant::now();
            let clock_ms = Arc::clone(&elapsed_ms);
            let gate = InteractionGate::with_clock(
                Arc::new(FakeTmux::new().with_clear_composer("%108")),
                Duration::from_secs(1),
                7_000,
                Arc::new(move || {
                    started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))
                }),
            );
            gate.observe_composer("%108", "old human draft");
            elapsed_ms.store(10_000, Ordering::Relaxed);
            gate.record_self_injection("%108", "[pij from boss] first");
            gate.observe_composer("%108", "[pij from boss] first");
            gate.record_self_injection("%108", "[pij from boss] second");
            // Reflow still matches the first injection's baseline, not the second payload.
            gate.observe_composer("%108", "[pij  from boss] first");
            if explicit_clear {
                gate.record_composer_cleared("%108");
            }
            let verdict =
                block_on(gate.fresh_injection_verdict("%108")).expect("baseline echo clear");
            assert!(
                verdict.permitted,
                "clearing a reflowed echo is not a new human edit"
            );
            assert_eq!(verdict.next_retry_at, None);
        }
    }

    #[test]
    fn initial_uncertainty_expires_to_tmux_mode_alone() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        assert!(!block_on(gate.permits_injection("%108")).expect("fresh uncertainty vetoes"));

        elapsed_ms.store(60_000, Ordering::Relaxed);
        assert!(block_on(gate.permits_injection("%108")).expect("stale uncertainty expires"));
    }

    #[test]
    fn tap_latch_expires_after_the_idle_window() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "");
        gate.record_tap("%108");
        assert!(!block_on(gate.permits_injection("%108")).expect("recent tap vetoes"));

        elapsed_ms.store(60_000, Ordering::Relaxed);
        assert!(block_on(gate.permits_injection("%108")).expect("stale tap expires"));
    }

    #[test]
    fn repeated_unknown_observations_do_not_refresh_interaction_age() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "human typing");
        gate.record_unknown("%108");

        elapsed_ms.store(30_000, Ordering::Relaxed);
        gate.record_unknown("%108");
        assert!(!block_on(gate.permits_injection("%108")).expect("fresh uncertainty vetoes"));

        elapsed_ms.store(60_000, Ordering::Relaxed);
        gate.record_unknown("%108");
        assert!(block_on(gate.permits_injection("%108")).expect("uncertainty did not refresh"));
    }

    #[test]
    fn active_tmux_mode_remains_the_fallback_veto() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let gate = InteractionGate::with_clock(
            Arc::new(FakeTmux::new().with_user_typing()),
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        gate.observe_composer("%108", "stale draft");
        elapsed_ms.store(120_000, Ordering::Relaxed);

        assert!(!block_on(gate.permits_injection("%108")).expect("active mode vetoes"));
    }
    #[test]
    fn owned_submit_vetoes_draft_and_ignores_streamed_output() {
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let started_at = Instant::now();
        let clock_ms = Arc::clone(&elapsed_ms);
        let capture = format!("output\n{RULE}\n❯ parked human draft\n{RULE}");
        let draft_tmux = Arc::new(
            FakeTmux::new()
                .with_pane(pane(21, 2))
                .with_attached_tap("%108")
                .script_capture(capture),
        );
        let draft_gate = InteractionGate::with_clock(
            Arc::clone(&draft_tmux) as Arc<dyn TmuxPort>,
            Duration::from_secs(60),
            contract_typing_grace_ms(),
            Arc::new(move || started_at + Duration::from_millis(clock_ms.load(Ordering::Relaxed))),
        );
        draft_gate.observe_composer("%108", "parked human draft");
        elapsed_ms.store(120_000, Ordering::Relaxed);
        // This is a physical safety brake, not a human-typing policy: removing it
        // would splice automated text into a parked draft. Keep it, named honestly.
        assert_eq!(
            block_on(draft_gate.submit_with_owned_input("%108", "AUTOMATED"))
                .expect("stale draft decision"),
            StagedSubmission::Deferred {
                reason: DeliveryDeferralReason::ComposerBusy,
                draft_sha: Some(short_sha256("parked human draft")),
            },
            "a non-blank composer is never a pane-typing target, regardless of age"
        );
        assert_eq!(draft_tmux.staged_len(), 0);
        assert!(
            draft_tmux
                .calls()
                .iter()
                .all(|call| !call.starts_with("stage_submit:") && !call.starts_with("submit:"))
        );

        let tap_tmux = Arc::new(
            FakeTmux::new()
                .with_clear_composer("%108")
                .script_tap(b"streamed agent output".to_vec()),
        );
        let tap_gate = InteractionGate::new(Arc::clone(&tap_tmux) as Arc<dyn TmuxPort>);
        tap_gate.observe_composer("%108", "");
        assert_eq!(
            block_on(tap_gate.submit_with_owned_input("%108", "AUTOMATED"))
                .expect("streamed output decision"),
            StagedSubmission::Submitted,
            "rendered output is not human typing when the composer is blank"
        );
        let calls = tap_tmux.calls();
        let drain = calls
            .iter()
            .position(|call| call == "drain_pane_tap:%108")
            .expect("fresh gate drains the live tap");
        let capture = calls
            .iter()
            .position(|call| call == "capture:%108:4294967295")
            .expect("fresh gate captures composer");
        assert!(
            drain < capture,
            "tap output is drained before composer authorization"
        );
        assert!(
            tap_tmux
                .calls()
                .iter()
                .any(|call| call == "submit:%108:AUTOMATED")
        );
    }

    #[test]
    fn transaction_exit_table_clears_marker_input_and_exemption() {
        #[derive(Clone, Copy, Debug)]
        enum Exit {
            Success,
            StageFail,
            CommitFail,
            CleanupFail,
            Abort,
            DropWithoutCommit,
            Deadline,
            Death,
        }

        for exit in [
            Exit::Success,
            Exit::StageFail,
            Exit::CommitFail,
            Exit::CleanupFail,
            Exit::Abort,
            Exit::DropWithoutCommit,
            Exit::Deadline,
            Exit::Death,
        ] {
            let base = FakeTmux::new().with_clear_composer("%108");
            let fake = match exit {
                Exit::StageFail => base.script_stage_error("scripted stage failure"),
                Exit::CommitFail => base.script_commit_error("scripted commit failure"),
                Exit::CleanupFail => base.script_cleanup_error("scripted cleanup failure"),
                Exit::Deadline => base.script_stage_error("staging exceeded 1s deadline"),
                Exit::Death => base.script_stage_error("daemon died during transaction"),
                Exit::Success | Exit::Abort | Exit::DropWithoutCommit => base,
            };
            let tmux = Arc::new(fake);
            let gate = InteractionGate::new(Arc::clone(&tmux) as Arc<dyn TmuxPort>);
            gate.observe_composer("%108", "");
            let result = match exit {
                Exit::Abort => {
                    let staged = block_on(tmux.acquire_submit("%108")).expect("acquire abort row");
                    block_on(PaneTransaction::new(&gate, staged).abort()).map(|()| {
                        StagedSubmission::Deferred {
                            reason: DeliveryDeferralReason::Unrecognized,
                            draft_sha: None,
                        }
                    })
                }
                Exit::DropWithoutCommit => {
                    let staged = block_on(tmux.acquire_submit("%108")).expect("acquire drop row");
                    let watchdog_recovery = staged.clone();
                    let mut transaction = PaneTransaction::new(&gate, staged);
                    transaction.self_injection =
                        Some(gate.self_injection_scope("%108", "MATCHING-HUMAN-TEXT"));
                    drop(transaction);
                    block_on(tmux.abort_submit(&watchdog_recovery)).map(|()| {
                        StagedSubmission::Deferred {
                            reason: DeliveryDeferralReason::Unrecognized,
                            draft_sha: None,
                        }
                    })
                }
                _ => block_on(gate.submit_with_owned_input("%108", "MATCHING-HUMAN-TEXT")),
            };
            match exit {
                Exit::Success | Exit::CleanupFail => assert_eq!(
                    result.expect("Enter is the successful outcome boundary"),
                    StagedSubmission::Submitted
                ),
                Exit::Abort | Exit::DropWithoutCommit => {
                    assert_eq!(
                        result.expect("non-commit exit"),
                        StagedSubmission::Deferred {
                            reason: DeliveryDeferralReason::Unrecognized,
                            draft_sha: None,
                        }
                    )
                }
                Exit::StageFail | Exit::CommitFail | Exit::Deadline | Exit::Death => {
                    assert!(result.is_err(), "{exit:?} must not report submission")
                }
            }
            if matches!(exit, Exit::CleanupFail) {
                assert!(
                    tmux.calls()
                        .iter()
                        .any(|call| call == "cleanup_error:scripted cleanup failure"),
                    "cleanup-fail row must actually inject the failure"
                );
            }
            assert_eq!(tmux.staged_len(), 0, "{exit:?}: ownership marker leaked");
            assert!(
                !tmux.pane_input_disabled("%108"),
                "{exit:?}: pane input remained disabled"
            );
            let exemption_active = gate
                .panes
                .lock()
                .expect("interaction gate mutex")
                .get("%108")
                .is_some_and(|state| state.self_injection.is_some());
            assert!(
                !exemption_active,
                "{exit:?}: self-injection exemption leaked"
            );

            gate.record_tap("%108");
            gate.observe_composer("%108", "MATCHING-HUMAN-TEXT");
            assert!(
                !block_on(gate.permits_injection("%108")).expect("human-input verdict"),
                "{exit:?}: matching human input was hidden by stale exemption"
            );
        }
    }

    #[test]
    fn observed_composer_clear_does_not_override_tmux_mode() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new().with_user_typing()));
        gate.record_tap("%108");
        gate.observe_composer("%108", "draft");
        gate.record_composer_cleared("%108");
        assert!(!block_on(gate.permits_injection("%108")).expect("typing gate"));
    }

    #[test]
    fn a_clear_observation_can_return_to_unknown_and_block() {
        let gate = InteractionGate::new(Arc::new(FakeTmux::new()));
        gate.observe_composer("%108", "   ");
        assert!(block_on(gate.permits_injection("%108")).expect("known clear gate"));

        gate.record_unknown("%108");

        assert!(
            !block_on(gate.permits_injection("%108")).expect("unknown gate"),
            "losing sight of a pane must revoke stale permission"
        );
    }
}
