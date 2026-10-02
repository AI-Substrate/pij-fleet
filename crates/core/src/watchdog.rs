//! Pure watchdog scheduling.
//!
//! The composition edge assembles [`WatchdogEntry`] values from the registry,
//! activity observations, and persisted watchdog controls. Core receives that
//! immutable view and decides which seats are due; it reads no clock and performs
//! no persistence or delivery.

use crate::config::Config;
use crate::model::{SeatDescriptor, SeatId};

/// Why the scheduler produced a nudge.
///
/// The adapter consumes this verdict instead of re-deriving it from timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NudgeReason {
    /// The seat is eligible, unpaused, idle, and one configured interval overdue.
    OverdueIdle,
}

/// A pure scheduling verdict for one seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nudge {
    /// Seat that should receive the nudge.
    pub seat: SeatId,
    /// Scheduler decision that made the nudge due.
    pub reason: NudgeReason,
}

/// Effective pause tier. Stronger tiers take precedence over weaker ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseTier {
    /// Explicit operator pause; cleared by resume or a committed assignment.
    SelfPaused,
    /// Automatic pause around compaction; cleared by a real working transition.
    Compact,
    /// Bounded exemption; clears at its absolute deadline.
    Exempt,
}

/// Persistable watchdog controls before precedence and expiry are applied.
///
/// The independent fields are intentional: a weaker pause may remain underneath
/// a live exemption and become effective when that exemption expires. Therefore
/// setting a self pause cannot downgrade an active exemption.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchdogControl {
    self_paused: bool,
    compact_paused: bool,
    exempt_until_secs: Option<u64>,
}

impl WatchdogControl {
    /// Construct the complete control state. This is the decision-table input.
    pub const fn new(
        self_paused: bool,
        compact_paused: bool,
        exempt_until_secs: Option<u64>,
    ) -> Self {
        Self {
            self_paused,
            compact_paused,
            exempt_until_secs,
        }
    }

    /// Resolve the strongest live tier at `now_secs`.
    pub const fn effective_pause(self, now_secs: u64) -> Option<PauseTier> {
        if matches!(self.exempt_until_secs, Some(deadline) if now_secs < deadline) {
            Some(PauseTier::Exempt)
        } else if self.compact_paused {
            Some(PauseTier::Compact)
        } else if self.self_paused {
            Some(PauseTier::SelfPaused)
        } else {
            None
        }
    }

    /// Clear an expired exemption for the adapter to persist.
    ///
    /// Expiry is exact: at the deadline the exemption is no longer live.
    pub const fn reconciled(self, now_secs: u64) -> Self {
        if matches!(self.exempt_until_secs, Some(deadline) if now_secs >= deadline) {
            Self {
                exempt_until_secs: None,
                ..self
            }
        } else {
            self
        }
    }

    /// Clear the automatic compact tier after a real working transition.
    ///
    /// Explicit self pause and a bounded exemption are independent claims and
    /// survive this transition.
    pub const fn after_working_transition(self) -> Self {
        Self {
            compact_paused: false,
            ..self
        }
    }
}

/// Immutable watchdog facts assembled at the composition edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchdogEntry {
    /// Registry descriptor for the seat.
    pub seat: SeatDescriptor,
    /// Most recent real activity, in seconds on the same clock as [`WatchdogService::tick`].
    pub last_activity_at_secs: u64,
    /// Persisted pause and exemption controls.
    pub control: WatchdogControl,
}

/// Pure supervision scheduler over an immutable registry view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchdogService {
    interval_secs: u64,
    entries: Vec<WatchdogEntry>,
}

impl WatchdogService {
    /// Build a scheduler using the configured watchdog interval.
    pub fn new(config: &Config, entries: Vec<WatchdogEntry>) -> Self {
        Self {
            interval_secs: config.watchdog_interval_secs,
            entries,
        }
    }

    /// Return every nudge due at `now_secs`, preserving registry-view order.
    ///
    /// Same service and timestamp always produce the same verdicts. Parking only
    /// suppresses nudging; it does not redefine card or anomaly staleness.
    pub fn tick(&self, now_secs: u64) -> Vec<Nudge> {
        self.entries
            .iter()
            .filter(|entry| entry.seat.tombstoned_at.is_none())
            .filter(|entry| entry.seat.nudgeable())
            .filter(|entry| entry.control.effective_pause(now_secs).is_none())
            .filter(|entry| {
                now_secs.saturating_sub(entry.last_activity_at_secs) >= self.interval_secs
            })
            .map(|entry| Nudge {
                seat: entry.seat.id.clone(),
                reason: NudgeReason::OverdueIdle,
            })
            .collect()
    }
}
