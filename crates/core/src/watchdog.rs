//! Pure watchdog scheduling.
//!
//! The composition edge assembles [`WatchdogEntry`] values from the registry,
//! activity observations, and persisted watchdog controls. Core receives that
//! immutable view and decides which seats are due; it reads no clock and performs
//! no persistence or delivery.
//!
//! **PAs** (Jordan, 2026-10-09) are not scheduled here. A PA is the fleet's
//! watchdog, and it is nudged when its fleet goes idle (plan 172, 2026-10-13:
//! "an idle fleet means nobody doing anything"), never on a fixed cadence:
//! see [`fleet_working`] and [`IdleEdge`]. When the prime starts again on its
//! own after that, the PA is woken once more ([`PrimeActive`], plan 173). A
//! fleet that went idle on a usage limit is woken at the reset instead
//! ([`crate::usage_limit`]).

use crate::config::Config;
use crate::model::{SeatDescriptor, SeatId, SystemState};

/// The one role the PA watchdog serves.
pub const PA_ROLE: &str = "pa";

/// The role a prime asserts.
pub const PRIME_ROLE: &str = "prime";

/// Is this seat a live PA the watchdog serves? Role is read from the
/// descriptor, which the composition edge must have joined from the role
/// store (the only role authority) before calling.
pub fn is_watched_pa(seat: &SeatDescriptor) -> bool {
    seat.role.as_deref() == Some(PA_ROLE) && seat.tombstoned_at.is_none() && !seat.relay
}

/// Is any WORKER in the scope working? "The fleet" is the workers
/// (pij-far-jackal ruling on #52 F1): every supervisor in scope is excluded,
/// meaning every `pa`-role seat, every `prime`-role seat and the designated
/// prime, not just the PA being evaluated. A supervisor's turn is its answer
/// to a nudge. If it counted, two PAs in one repository would re-arm each
/// other's idle episode forever, and a lone PA would re-arm its own.
/// Tombstoned seats never count. Roles must already be joined from the
/// role store.
pub fn fleet_working<'a>(
    scope: impl IntoIterator<Item = &'a SeatDescriptor>,
    designated_prime: Option<&SeatId>,
) -> bool {
    scope.into_iter().any(|seat| {
        let supervisor = matches!(seat.role.as_deref(), Some(PA_ROLE | PRIME_ROLE))
            || designated_prime == Some(&seat.id);
        !supervisor && seat.tombstoned_at.is_none() && seat.state == SystemState::Working
    })
}

/// One PA's idle-edge tracker: the fleet's working state as last observed.
///
/// A nudge goes on the EDGE from working to all-idle, once per idle episode;
/// a new episode starts only after some seat has worked again. A first
/// observation only arms the tracker, so a daemon (re)start or a newly
/// appointed PA never nudges a fleet that was already idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdleEdge {
    working: Option<bool>,
}

impl IdleEdge {
    /// Record this round's observation. `true` exactly on working → idle.
    pub fn observe(&mut self, working: bool) -> bool {
        let edge = self.working == Some(true) && !working;
        self.working = Some(working);
        edge
    }
}

/// The prime a PA's prime-active wake watches (plan 173 R1): the designated
/// prime when it is live in `scope`, else the lowest-id `prime`-role seat in
/// `scope`. `None` when there is neither. Roles must already be joined.
pub fn scope_prime<'a>(
    scope: &'a [SeatDescriptor],
    designated_prime: Option<&SeatId>,
) -> Option<&'a SeatDescriptor> {
    let live = || scope.iter().filter(|seat| seat.tombstoned_at.is_none());
    live()
        .find(|seat| designated_prime == Some(&seat.id))
        .or_else(|| {
            live()
                .filter(|seat| seat.role.as_deref() == Some(PRIME_ROLE))
                .min_by(|a, b| a.id.cmp(&b.id))
        })
}

/// The reason recorded when a prime-active wake wakes a cold PA.
pub const PRIME_ACTIVE_REASON: &str = "prime-active";

/// The prime-active wake's text: why the PA is woken, then the fleet digest.
pub fn prime_active_body(prime: &SeatId, digest: &str) -> String {
    format!(
        "[pij watchdog] {prime} is active again after the fleet went idle: resume your plan.\n\n{digest}"
    )
}

/// One round's facts for a PA's [`PrimeActive`] tracker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimeObservation {
    /// This round's [`IdleEdge`] fired: a new worker-idle episode opens.
    pub edge: bool,
    /// The PA is idle.
    pub pa_idle: bool,
    /// The prime is working, `None` with no prime in scope.
    pub prime_working: Option<bool>,
    /// The scope is stopped on a usage limit.
    pub on_limit: bool,
    /// A pij message was delivered to the prime at or after
    /// [`PrimeActive::armed_at_ms`]. Read only when that is `Some` and the
    /// prime is working; `false` otherwise.
    pub delivered_since_arming: bool,
    /// This round's clock, epoch ms: the arming time when it arms.
    pub now_ms: u64,
}

/// What a [`PrimeActive`] observation decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimeVerdict {
    /// Nothing to do this round.
    Waiting,
    /// The prime started on its own: wake the PA. The episode's cap is spent.
    Wake,
    /// The prime's turn followed a delivery to it. No wake; disarmed, the cap
    /// kept, re-armed at the next tick where the PA and the prime are idle.
    Braked,
    /// The prime started again after this episode's wake. No wake.
    Capped,
    /// The prime started while the scope is stopped on a usage limit: the
    /// limit-reset wake owns that. No wake; disarmed, the cap kept.
    OnLimit,
}

impl PrimeVerdict {
    /// The verdict as audited on `watchdog.round`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Wake => "sent",
            Self::Braked => "braked",
            Self::Capped => "capped",
            Self::OnLimit => "idle-on-limit",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Episode {
    /// No worker-idle episode since the tracker began: a restart never wakes.
    #[default]
    None,
    /// An episode is open; waiting for a tick where PA and prime are idle.
    Open,
    /// Armed at this time, epoch ms.
    Armed(u64),
    /// This episode's one wake went.
    Spent,
}

/// One PA's prime-active wake (plan 173): once per worker-idle episode, when
/// the prime moves from idle to working on its own, the PA is woken too.
///
/// The trigger arms at the first tick after the [`IdleEdge`] where the PA and
/// the prime are both idle, and fires on the prime's next working tick. A
/// delivery to the prime since arming is a BRAKE: removing it only sends
/// more. A pij message, the PA's own report included, then caused the turn,
/// so it disarms without spending the cap. At most one wake per episode; a
/// new episode opens only on the next worker idle edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrimeActive {
    episode: Episode,
    prime_working: Option<bool>,
}

impl PrimeActive {
    /// When the trigger armed, epoch ms, while it is armed.
    pub const fn armed_at_ms(&self) -> Option<u64> {
        match self.episode {
            Episode::Armed(at) => Some(at),
            _ => None,
        }
    }

    /// Record this round's observation and decide.
    pub fn observe(&mut self, seen: PrimeObservation) -> PrimeVerdict {
        let started = self.prime_working == Some(false) && seen.prime_working == Some(true);
        self.prime_working = seen.prime_working;
        if seen.edge {
            // Arming waits for a tick AFTER the edge.
            self.episode = Episode::Open;
            return PrimeVerdict::Waiting;
        }
        match (self.episode, seen.prime_working) {
            (Episode::None, _) => PrimeVerdict::Waiting,
            (Episode::Spent, _) if started => PrimeVerdict::Capped,
            (Episode::Spent, _) => PrimeVerdict::Waiting,
            (Episode::Open | Episode::Armed(_), None) => {
                self.episode = Episode::Open;
                PrimeVerdict::Waiting
            }
            (Episode::Open, Some(false)) if seen.pa_idle => {
                self.episode = Episode::Armed(seen.now_ms);
                PrimeVerdict::Waiting
            }
            (Episode::Open | Episode::Armed(_), Some(false)) => PrimeVerdict::Waiting,
            (Episode::Open, Some(true)) => PrimeVerdict::Waiting,
            (Episode::Armed(_), Some(true)) => {
                let verdict = if seen.on_limit {
                    PrimeVerdict::OnLimit
                } else if seen.delivered_since_arming {
                    PrimeVerdict::Braked
                } else {
                    PrimeVerdict::Wake
                };
                self.episode = if verdict == PrimeVerdict::Wake {
                    Episode::Spent
                } else {
                    Episode::Open
                };
                verdict
            }
        }
    }
}

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

/// A seat that opted in to the watchdog (`pij watchdog on`). PAs are watched
/// by role and never need one. Any seat may turn any seat's watchdog on or
/// off, so the row says who did it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WatchdogOptIn {
    /// The watched seat.
    pub seat: SeatId,
    /// How long the seat may be quiet before it is nudged.
    pub interval_secs: u64,
    /// The seat that turned it on.
    pub set_by: SeatId,
    /// When, in milliseconds since the epoch.
    pub set_at_ms: u64,
}

/// Is an opted-in, non-PA seat due a nudge?
///
/// Unlike a PA's clock this is a stall check, so the TS lessons hold: a seat
/// mid-turn is not stalled however quiet it is, and a seat that declared
/// `waiting`, `hold`, `blocked` or `question` is silent on purpose
/// ([`SeatDescriptor::nudgeable`]). `last_activity_at_secs` is the later of
/// the seat's last event and its last nudge.
pub fn optin_due(
    seat: &SeatDescriptor,
    optin: &WatchdogOptIn,
    last_activity_at_secs: u64,
    now_secs: u64,
) -> bool {
    seat.tombstoned_at.is_none()
        && seat.role.as_deref() != Some(PA_ROLE)
        && seat.nudgeable()
        && now_secs.saturating_sub(last_activity_at_secs) >= optin.interval_secs
}

/// The nudge an opted-in seat receives. Every nudge says how to stop it and
/// that the seat should, when no more work is coming.
pub fn optin_nudge(seat: &SeatId, quiet_secs: u64, optin: &WatchdogOptIn) -> String {
    let who = if optin.set_by == optin.seat {
        "you".to_string()
    } else {
        optin.set_by.to_string()
    };
    format!(
        "[pij watchdog] {seat}: quiet {} (watchdog on every {}, set by {who}). \
         Keep going if working, and report with `pij report now \"<did>\" \"<next>\"`. \
         If this unit of work is finished, run `pij report state done`; \
         if you are idle but available, run `pij report state ready`. \
         No more work coming? Stop this watchdog: `pij watchdog off`.",
        crate::cold_wake::human_duration(quiet_secs.saturating_mul(1_000)),
        crate::cold_wake::human_duration(optin.interval_secs.saturating_mul(1_000)),
    )
}
