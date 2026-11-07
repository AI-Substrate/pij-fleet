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
use crate::model::{SeatDescriptor, SeatId, SemanticState, SystemState};

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
pub struct PrimeObservation<'a> {
    /// This round's [`IdleEdge`] fired: a new worker-idle episode opens.
    pub edge: bool,
    /// The PA's observed state. Only [`SystemState::Idle`] is idle; unknown
    /// and every other state is no evidence of idle.
    pub pa: SystemState,
    /// The watched prime ([`scope_prime`]) and its observed state, `None`
    /// with no prime in scope.
    pub prime: Option<(&'a SeatId, SystemState)>,
    /// The scope is stopped on a usage limit.
    pub on_limit: bool,
    /// A pij message was delivered to the prime at or after
    /// [`PrimeActive::armed_at_ms`]. Read only when that is `Some` for this
    /// prime and the prime is working; `false` otherwise.
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
    /// Armed at this time, epoch ms, for the prime last observed.
    Armed(u64),
    /// This episode's one wake went.
    Spent,
}

/// One PA's prime-active wake (plan 173): once per worker-idle episode, when
/// the prime moves from idle to working on its own, the PA is woken too.
///
/// The trigger arms at the first tick after the [`IdleEdge`] where the PA and
/// the prime are both observed [`SystemState::Idle`], and fires on that same
/// prime's next working tick. Any other observation of the armed prime drops
/// the arming. A different prime drops it too, then arms afresh on that same
/// tick if it and the PA are observed idle: only one prime's own idle→working
/// fires it. A delivery to the prime since arming is a BRAKE: removing it
/// only sends more. A pij message, the PA's own report included, then caused
/// the turn, so it disarms without spending the cap. At most one wake per
/// episode, whoever the prime becomes; a new episode opens only on the next
/// worker idle edge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrimeActive {
    episode: Episode,
    /// The prime and its state as last observed.
    prime: Option<(SeatId, SystemState)>,
}

impl PrimeActive {
    /// When the trigger armed for `prime`, epoch ms, while it is armed for
    /// that prime.
    pub fn armed_at_ms(&self, prime: &SeatId) -> Option<u64> {
        match (self.episode, &self.prime) {
            (Episode::Armed(at), Some((armed, _))) if armed == prime => Some(at),
            _ => None,
        }
    }

    /// Record this round's observation and decide.
    pub fn observe(&mut self, seen: PrimeObservation<'_>) -> PrimeVerdict {
        let last = self.prime.take();
        let same = last.as_ref().map(|(id, _)| id) == seen.prime.map(|(id, _)| id);
        let started = same
            && last.as_ref().map(|(_, state)| *state) == Some(SystemState::Idle)
            && seen.prime.map(|(_, state)| state) == Some(SystemState::Working);
        self.prime = seen.prime.map(|(id, state)| (id.clone(), state));
        if seen.edge {
            // Arming waits for a tick AFTER the edge.
            self.episode = Episode::Open;
            return PrimeVerdict::Waiting;
        }
        if !same && matches!(self.episode, Episode::Armed(_)) {
            // The arming belonged to another prime: drop it and judge this
            // tick afresh, so a new prime seen idle with the PA arms on it.
            self.episode = Episode::Open;
        }
        match (self.episode, seen.prime.map(|(_, state)| state)) {
            (Episode::None, _) => PrimeVerdict::Waiting,
            // A new prime keeps the cap: it never re-opens an episode.
            (Episode::Spent, _) if started => PrimeVerdict::Capped,
            (Episode::Spent, _) => PrimeVerdict::Waiting,
            (Episode::Open, Some(SystemState::Idle)) if seen.pa == SystemState::Idle => {
                self.episode = Episode::Armed(seen.now_ms);
                PrimeVerdict::Waiting
            }
            (Episode::Open, _) => PrimeVerdict::Waiting,
            (Episode::Armed(_), Some(SystemState::Idle)) => PrimeVerdict::Waiting,
            (Episode::Armed(_), _) if started => {
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
            // No prime, or no idle/working evidence: the arming no longer
            // stands.
            (Episode::Armed(_), _) => {
                self.episode = Episode::Open;
                PrimeVerdict::Waiting
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
///
/// A seat that is `ready` after a nudge since it last worked has answered it
/// ([`optin_nudge`] tells an idle but available seat to say exactly that), so
/// it is quiet until it works again (plan 178). This is a BRAKE: removing it
/// only nudges more.
pub fn optin_due(
    seat: &SeatDescriptor,
    optin: &WatchdogOptIn,
    last_activity_at_secs: u64,
    nudged_since_work: bool,
    now_secs: u64,
) -> bool {
    seat.tombstoned_at.is_none()
        && seat.role.as_deref() != Some(PA_ROLE)
        && seat.nudgeable()
        && !(nudged_since_work && seat.semantic_state == Some(SemanticState::Ready))
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
         if you are idle but available, run `pij report state ready`, \
         and this watchdog stays quiet until you next work. \
         No more work coming? Stop this watchdog: `pij watchdog off`.",
        crate::cold_wake::human_duration(quiet_secs.saturating_mul(1_000)),
        crate::cold_wake::human_duration(optin.interval_secs.saturating_mul(1_000)),
    )
}

// Plan 217: PA presence and large seats. Four checks ride the PA watchdog's
// round, per scope (a prime and its fleet): the prime hears when an active
// fleet has no live PA, a dormant PA is woken, the prime hears when that PA
// stays silent, and the prime gets an FYI when a fleet seat grows past
// LARGE_SEAT_TOKENS. Every threshold is one constant here (Jordan, 2026-11-07).

/// A scope is active while its prime or a worker was observed working this
/// recently.
pub const FLEET_ACTIVE_WINDOW_MS: u64 = 20 * 60_000;
/// How long an active scope may go without a live PA before the prime hears.
pub const PA_MISSING_AFTER_MS: u64 = 20 * 60_000;
/// How long a PA in an active scope may take no turn before it is woken.
pub const PA_DORMANT_AFTER_MS: u64 = 30 * 60_000;
/// How long after a dormant wake with no PA turn the prime hears.
pub const PA_UNRESPONSIVE_AFTER_MS: u64 = 10 * 60_000;
/// A fleet seat whose context is over this many tokens is named to the prime.
pub const LARGE_SEAT_TOKENS: u64 = 600_000;
/// A scope with fewer workers than this never raises the PA alerts.
pub const PRESENCE_MIN_WORKERS: usize = 2;

/// The reason recorded when the dormant wake wakes a cold PA.
pub const PA_DORMANT_REASON: &str = "pa-watchdog: dormant while fleet active";
/// The reason recorded when the missing-PA alert wakes a cold prime.
pub const PA_MISSING_REASON: &str = "pa-watchdog: active fleet has no PA";
/// The reason recorded when the unresponsive-PA alert wakes a cold prime.
pub const PA_UNRESPONSIVE_REASON: &str = "pa-watchdog: PA unresponsive";

/// The once-per-stretch claims, stored before each message is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PresenceKind {
    /// The prime was told its active scope has no live PA. Subject: the scope.
    Missing,
    /// A dormant PA was woken. Subject: the PA.
    Dormant,
    /// The prime was told its PA did not answer the dormant wake. Subject: the PA.
    Unresponsive,
    /// The prime was told a seat went over [`LARGE_SEAT_TOKENS`]. Subject: the seat.
    LargeSeat,
}

impl PresenceKind {
    /// Every kind, in store order.
    pub const ALL: [Self; 4] = [
        Self::Missing,
        Self::Dormant,
        Self::Unresponsive,
        Self::LargeSeat,
    ];

    /// The stored and audited spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "pa-missing",
            Self::Dormant => "pa-dormant",
            Self::Unresponsive => "pa-unresponsive",
            Self::LargeSeat => "large-seat",
        }
    }

    /// Parse a stored spelling.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// One stored claim: this stretch's message for `subject` was sent (or is
/// being sent). Removing the row re-arms the check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceClaim {
    /// Which check.
    pub kind: PresenceKind,
    /// The scope key, PA or seat the claim is about.
    pub subject: String,
    /// When it was claimed, epoch ms.
    pub claimed_at_ms: u64,
    /// What the send recorded once delivery answered (JSON), `{}` until then.
    pub detail: String,
}

/// What one check decided this round, as audited on `watchdog.round`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceVerdict {
    /// Nothing to say: the condition does not hold.
    Quiet,
    /// The condition holds; its time has not come.
    Waiting,
    /// Send now (the claim is taken first).
    Send,
    /// Already sent this stretch.
    Capped,
    /// The stretch ended: the claim is released and the check re-armed.
    Armed,
    /// Due, but a brake held it: there is nobody to tell.
    Braked,
}

impl PresenceVerdict {
    /// The verdict as audited.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Waiting => "waiting",
            Self::Send => "sent",
            Self::Capped => "capped",
            Self::Armed => "armed",
            Self::Braked => "braked",
        }
    }
}

/// Whether a scope is active, from what this daemon has observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// The prime or a worker was observed working within
    /// [`FLEET_ACTIVE_WINDOW_MS`], and the scope is not on a usage limit.
    Active,
    /// Positively not active: on a usage limit, or watched for a whole window
    /// with no working observation.
    Inactive,
    /// Not watched long enough to say. Never evidence of activity, and never
    /// re-arms a claim: after a restart, only a positive observation does.
    Unknown,
}

/// One scope's facts for a round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeObservation {
    /// The prime or a worker is observed [`SystemState::Working`] now.
    pub working: bool,
    /// The scope is stopped on a usage limit.
    pub on_limit: bool,
    /// Live workers in scope (not a PA, not a prime).
    pub workers: usize,
    /// A live PA counts for this scope.
    pub live_pa: bool,
    /// The stored missing-PA claim exists.
    pub missing_claimed: bool,
    /// This round's clock, epoch ms.
    pub now_ms: u64,
}

/// One scope's activity and missing-PA tracker. In memory: a restart starts
/// it afresh, which can only delay a message (the stored claim stops a
/// repeat).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopePresence {
    watched_since_ms: Option<u64>,
    last_working_ms: Option<u64>,
    missing_since_ms: Option<u64>,
}

impl ScopePresence {
    /// Record this round's observation: the scope's activity, and the
    /// missing-PA verdict.
    pub fn observe(&mut self, seen: ScopeObservation) -> (Activity, PresenceVerdict) {
        let now = seen.now_ms;
        let watched_since = *self.watched_since_ms.get_or_insert(now);
        if seen.working {
            self.last_working_ms = Some(now);
        }
        let activity = if seen.on_limit {
            Activity::Inactive
        } else if self
            .last_working_ms
            .is_some_and(|at| now.saturating_sub(at) < FLEET_ACTIVE_WINDOW_MS)
        {
            Activity::Active
        } else if now.saturating_sub(watched_since) >= FLEET_ACTIVE_WINDOW_MS {
            Activity::Inactive
        } else {
            Activity::Unknown
        };
        let rearm = seen.live_pa || activity == Activity::Inactive;
        if rearm || activity == Activity::Unknown || seen.workers < PRESENCE_MIN_WORKERS {
            self.missing_since_ms = None;
            let verdict = if rearm && seen.missing_claimed {
                PresenceVerdict::Armed
            } else {
                PresenceVerdict::Quiet
            };
            return (activity, verdict);
        }
        let since = *self.missing_since_ms.get_or_insert(now);
        let verdict = if seen.missing_claimed {
            PresenceVerdict::Capped
        } else if now.saturating_sub(since) >= PA_MISSING_AFTER_MS {
            PresenceVerdict::Send
        } else {
            PresenceVerdict::Waiting
        };
        (activity, verdict)
    }

    /// When this tracker first saw the scope, epoch ms.
    pub fn watched_since_ms(&self) -> Option<u64> {
        self.watched_since_ms
    }

    /// How long the current missing stretch has lasted at `now_ms`.
    pub fn missing_for_ms(&self, now_ms: u64) -> u64 {
        self.missing_since_ms
            .map_or(0, |since| now_ms.saturating_sub(since))
    }
}

/// One live PA's facts for the dormant and unresponsive checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaObservation {
    /// Its scope's [`Activity`] this round.
    pub activity: Activity,
    /// Live workers in its scope.
    pub workers: usize,
    /// The PA is observed [`SystemState::Working`] now.
    pub working: bool,
    /// Its last turn boundary, epoch ms (its latest `seat.activity`, or the
    /// last round that saw it working), `None` when there is none.
    pub last_turn_ms: Option<u64>,
    /// When this daemon first watched the PA: the dormant clock's start when
    /// it has no turn on record.
    pub watched_since_ms: u64,
    /// The stored dormant claim's time, when a dormant wake was sent.
    pub dormant_at_ms: Option<u64>,
    /// The stored unresponsive claim exists.
    pub unresponsive_claimed: bool,
    /// The scope has a prime to tell.
    pub prime: bool,
    /// This round's clock, epoch ms.
    pub now_ms: u64,
}

/// The dormant wake's and the unresponsive alert's verdicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaVerdicts {
    /// Wake the PA ([`PresenceVerdict::Send`]), or why not.
    pub dormant: PresenceVerdict,
    /// Tell the prime ([`PresenceVerdict::Send`]), or why not.
    pub unresponsive: PresenceVerdict,
}

/// Decide a PA's dormant wake and unresponsive alert from stored claims and
/// this round's facts. Pure and stateless: everything that must survive a
/// restart is in the claims or on the spine.
///
/// A PA observed working is never dormant. A turn after the dormant wake
/// re-arms both. The unresponsive alert needs a prime; with none it is
/// [`PresenceVerdict::Braked`] and fires when one appears.
pub fn pa_verdicts(seen: PaObservation) -> PaVerdicts {
    let quiet = PaVerdicts {
        dormant: PresenceVerdict::Quiet,
        unresponsive: PresenceVerdict::Quiet,
    };
    if let Some(woken_at) = seen.dormant_at_ms {
        if seen.working || seen.last_turn_ms.is_some_and(|turn| turn > woken_at) {
            return PaVerdicts {
                dormant: PresenceVerdict::Armed,
                unresponsive: if seen.unresponsive_claimed {
                    PresenceVerdict::Armed
                } else {
                    PresenceVerdict::Quiet
                },
            };
        }
        let unresponsive = if seen.unresponsive_claimed {
            PresenceVerdict::Capped
        } else if seen.now_ms.saturating_sub(woken_at) < PA_UNRESPONSIVE_AFTER_MS {
            PresenceVerdict::Waiting
        } else if seen.prime {
            PresenceVerdict::Send
        } else {
            PresenceVerdict::Braked
        };
        return PaVerdicts {
            dormant: PresenceVerdict::Capped,
            unresponsive,
        };
    }
    if seen.working || seen.activity != Activity::Active || seen.workers < PRESENCE_MIN_WORKERS {
        return quiet;
    }
    let quiet_since = seen.last_turn_ms.unwrap_or(seen.watched_since_ms);
    PaVerdicts {
        dormant: if seen.now_ms.saturating_sub(quiet_since) >= PA_DORMANT_AFTER_MS {
            PresenceVerdict::Send
        } else {
            PresenceVerdict::Waiting
        },
        unresponsive: PresenceVerdict::Quiet,
    }
}

/// The large-seat check over a fleet's known sizes: `(crossed, rearmed)`.
/// A seat over [`LARGE_SEAT_TOKENS`] with no claim has crossed; a claimed
/// seat seen at or under it re-arms. An unknown size is neither.
pub fn large_seats<'a>(
    sizes: impl IntoIterator<Item = (&'a SeatId, Option<u64>)>,
    claimed: impl Fn(&SeatId) -> bool,
) -> (Vec<SeatId>, Vec<SeatId>) {
    let mut crossed = Vec::new();
    let mut rearmed = Vec::new();
    for (seat, tokens) in sizes {
        match tokens {
            Some(tokens) if tokens > LARGE_SEAT_TOKENS && !claimed(seat) => {
                crossed.push(seat.clone());
            }
            Some(tokens) if tokens <= LARGE_SEAT_TOKENS && claimed(seat) => {
                rearmed.push(seat.clone());
            }
            _ => {}
        }
    }
    (crossed, rearmed)
}

/// The missing-PA alert to the prime: the last PA and its revive command, or
/// the spawn form when the scope never had one.
pub fn pa_missing_body(
    prime: &SeatId,
    active_ms: u64,
    last_pa: Option<&SeatId>,
    folder: &str,
) -> String {
    let lead = format!(
        "[pij watchdog] {prime}: your fleet has been working for {} with no live PA, so \
         nobody is watching seat sizes, stalls or queued requests.",
        crate::cold_wake::human_duration(active_ms)
    );
    let spawn = format!("pij-rs spawn --harness omp --role pa --cwd {folder}");
    match last_pa {
        Some(pa) => format!(
            "{lead} The last PA was {pa}. Revive it: `pij-rs revive {pa}` (add `--fresh` for a \
             blank conversation), or start a new one: `{spawn}`."
        ),
        None => format!("{lead} Start one: `{spawn}`."),
    }
}

/// The dormant wake to the PA: why, then the fleet digest.
pub fn pa_dormant_body(pa: &SeatId, quiet_ms: u64, digest: &str) -> String {
    format!(
        "[pij watchdog] {pa}: your fleet is working but you have taken no turn for {}. \
         Check the fleet now, and report with `pij report now \"<did>\" \"<next>\"`.\n\n{digest}",
        crate::cold_wake::human_duration(quiet_ms)
    )
}

/// The unresponsive-PA alert to the prime.
pub fn pa_unresponsive_body(
    prime: &SeatId,
    pa: &SeatId,
    quiet_ms: u64,
    woken_ago_ms: u64,
    wake_receipt: &str,
) -> String {
    format!(
        "[pij watchdog] {prime}: your PA {pa} has taken no turn for {}, and has not answered \
         the dormant wake sent {} ago ({wake_receipt}). It may be wedged, waiting on a \
         question, or holding a message in its composer. Check its pane; revive it \
         (`pij-rs revive {pa}`); or replace it.",
        crate::cold_wake::human_duration(quiet_ms),
        crate::cold_wake::human_duration(woken_ago_ms),
    )
}

/// One large seat, for [`large_seats_body`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LargeSeat {
    /// The seat.
    pub seat: SeatId,
    /// Its context, in tokens.
    pub tokens: u64,
    /// Its role, when it has one.
    pub role: Option<String>,
    /// Its model, when known.
    pub model: Option<String>,
}

/// The large-seat FYI to the prime: every crossing this round, one line each.
pub fn large_seats_body(seats: &[LargeSeat], pas: &[SeatId]) -> String {
    let pa = if pas.is_empty() {
        "none".to_string()
    } else {
        pas.iter()
            .map(SeatId::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!(
        "[pij watchdog] over {} tokens in your fleet (PA: {pa}):",
        crate::cold_wake::human_tokens(LARGE_SEAT_TOKENS)
    );
    for seat in seats {
        text.push_str(&format!(
            "\n- {} {} role {} model {}",
            seat.seat,
            crate::cold_wake::human_tokens(seat.tokens),
            seat.role.as_deref().unwrap_or("none"),
            seat.model.as_deref().unwrap_or("unknown"),
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan 178 AC6: the nudge and the PA brief name the same card command.
    /// The brief is read, not restated, so either side drifting fails here.
    #[test]
    fn the_nudge_and_the_pa_brief_name_the_same_report_command() {
        const CARD: &str = "pij report now \"<did>\" \"<next>\"";
        let brief = include_str!("../../../skills/pij/references/prime/pa.md");
        let optin = WatchdogOptIn {
            seat: "pij-w".into(),
            interval_secs: 60,
            set_by: "pij-w".into(),
            set_at_ms: 0,
        };
        let nudge = optin_nudge(&optin.seat, 60, &optin);
        assert!(nudge.contains(CARD), "{nudge}");
        assert!(brief.contains(CARD), "the PA brief names {CARD}");
    }

    const MIN: u64 = 60_000;

    fn scope(working: bool, now_ms: u64) -> ScopeObservation {
        ScopeObservation {
            working,
            on_limit: false,
            workers: 2,
            live_pa: false,
            missing_claimed: false,
            now_ms,
        }
    }

    /// Plan 217 AC1: working within the window is active; an unseen scope is
    /// unknown, never active; a usage limit is inactive.
    #[test]
    fn a_scope_is_active_only_on_a_working_observation_within_the_window() {
        let mut tracker = ScopePresence::default();
        assert_eq!(tracker.observe(scope(false, 0)).0, Activity::Unknown);
        assert_eq!(tracker.observe(scope(false, 19 * MIN)).0, Activity::Unknown);
        assert_eq!(
            tracker.observe(scope(false, 20 * MIN)).0,
            Activity::Inactive
        );
        assert_eq!(tracker.observe(scope(true, 21 * MIN)).0, Activity::Active);
        assert_eq!(tracker.observe(scope(false, 40 * MIN)).0, Activity::Active);
        assert_eq!(
            tracker.observe(scope(false, 41 * MIN)).0,
            Activity::Inactive
        );
        let limited = ScopeObservation {
            on_limit: true,
            ..scope(true, 42 * MIN)
        };
        assert_eq!(tracker.observe(limited).0, Activity::Inactive);
    }

    /// Plan 217 AC2: one send after 20 active minutes with no PA, capped while
    /// claimed, re-armed by a PA or a positively inactive scope, and never
    /// with fewer than two workers.
    #[test]
    fn the_missing_pa_alert_arms_sends_caps_and_rearms() {
        let mut tracker = ScopePresence::default();
        let verdict = |tracker: &mut ScopePresence, seen| tracker.observe(seen).1;
        assert_eq!(
            verdict(&mut tracker, scope(true, 0)),
            PresenceVerdict::Waiting
        );
        assert_eq!(
            verdict(&mut tracker, scope(true, 19 * MIN)),
            PresenceVerdict::Waiting
        );
        assert_eq!(tracker.missing_for_ms(19 * MIN), 19 * MIN);
        assert_eq!(
            verdict(&mut tracker, scope(true, 20 * MIN)),
            PresenceVerdict::Send
        );
        let claimed = |at| ScopeObservation {
            missing_claimed: true,
            ..scope(true, at)
        };
        assert_eq!(
            verdict(&mut tracker, claimed(21 * MIN)),
            PresenceVerdict::Capped
        );
        // A PA appears: the claim is released.
        let with_pa = ScopeObservation {
            live_pa: true,
            ..claimed(22 * MIN)
        };
        assert_eq!(verdict(&mut tracker, with_pa), PresenceVerdict::Armed);
        // A fresh stretch waits its own 20 minutes.
        assert_eq!(
            verdict(&mut tracker, scope(true, 23 * MIN)),
            PresenceVerdict::Waiting
        );
        assert_eq!(
            verdict(&mut tracker, scope(true, 42 * MIN)),
            PresenceVerdict::Waiting
        );
        assert_eq!(
            verdict(&mut tracker, scope(true, 43 * MIN)),
            PresenceVerdict::Send
        );
        // Inactive for a whole window re-arms.
        assert_eq!(
            verdict(
                &mut tracker,
                ScopeObservation {
                    working: false,
                    ..claimed(63 * MIN)
                }
            ),
            PresenceVerdict::Armed
        );
        // One worker never fires, however long.
        let mut lone = ScopePresence::default();
        for minute in 0..60 {
            let seen = ScopeObservation {
                workers: 1,
                ..scope(true, minute * MIN)
            };
            assert_eq!(lone.observe(seen).1, PresenceVerdict::Quiet);
        }
    }

    /// Plan 217 AC7: a restarted tracker never re-arms a stored claim on its
    /// first observations; only positive evidence does.
    #[test]
    fn a_fresh_tracker_keeps_a_stored_missing_claim() {
        let mut tracker = ScopePresence::default();
        for minute in 0..20 {
            let seen = ScopeObservation {
                working: false,
                missing_claimed: true,
                ..scope(false, minute * MIN)
            };
            assert_eq!(tracker.observe(seen).1, PresenceVerdict::Quiet, "{minute}");
        }
    }

    fn pa(now_ms: u64) -> PaObservation {
        PaObservation {
            activity: Activity::Active,
            workers: 2,
            working: false,
            last_turn_ms: Some(0),
            watched_since_ms: 0,
            dormant_at_ms: None,
            unresponsive_claimed: false,
            prime: true,
            now_ms,
        }
    }

    /// Plan 217 AC3, AC4: woken once at 30 turn-less minutes, the prime told
    /// 10 minutes later, both capped, both re-armed by a turn.
    #[test]
    fn a_dormant_pa_is_woken_then_reported_then_rearmed_by_a_turn() {
        assert_eq!(pa_verdicts(pa(29 * MIN)).dormant, PresenceVerdict::Waiting);
        assert_eq!(pa_verdicts(pa(30 * MIN)).dormant, PresenceVerdict::Send);
        let working = PaObservation {
            working: true,
            ..pa(90 * MIN)
        };
        assert_eq!(pa_verdicts(working).dormant, PresenceVerdict::Quiet);
        for quiet in [Activity::Unknown, Activity::Inactive] {
            let seen = PaObservation {
                activity: quiet,
                ..pa(90 * MIN)
            };
            assert_eq!(pa_verdicts(seen).dormant, PresenceVerdict::Quiet);
        }
        let lone = PaObservation {
            workers: 1,
            ..pa(90 * MIN)
        };
        assert_eq!(pa_verdicts(lone).dormant, PresenceVerdict::Quiet);
        // No turn on record: the clock starts when the daemon first saw it.
        let unseen = PaObservation {
            last_turn_ms: None,
            watched_since_ms: 10 * MIN,
            ..pa(39 * MIN)
        };
        assert_eq!(pa_verdicts(unseen).dormant, PresenceVerdict::Waiting);

        let woken = |now| PaObservation {
            dormant_at_ms: Some(30 * MIN),
            ..pa(now)
        };
        assert_eq!(
            pa_verdicts(woken(39 * MIN)),
            PaVerdicts {
                dormant: PresenceVerdict::Capped,
                unresponsive: PresenceVerdict::Waiting
            }
        );
        assert_eq!(
            pa_verdicts(woken(40 * MIN)).unresponsive,
            PresenceVerdict::Send
        );
        let no_prime = PaObservation {
            prime: false,
            ..woken(40 * MIN)
        };
        assert_eq!(pa_verdicts(no_prime).unresponsive, PresenceVerdict::Braked);
        let told = PaObservation {
            unresponsive_claimed: true,
            ..woken(50 * MIN)
        };
        assert_eq!(pa_verdicts(told).unresponsive, PresenceVerdict::Capped);
        // The wake's own time is not a turn; a later one re-arms both.
        let same = PaObservation {
            last_turn_ms: Some(30 * MIN),
            ..told
        };
        assert_eq!(pa_verdicts(same).dormant, PresenceVerdict::Capped);
        let answered = PaObservation {
            last_turn_ms: Some(30 * MIN + 1),
            ..told
        };
        assert_eq!(
            pa_verdicts(answered),
            PaVerdicts {
                dormant: PresenceVerdict::Armed,
                unresponsive: PresenceVerdict::Armed
            }
        );
        // A turn inside the 10 minutes re-arms the wake and tells nobody.
        let quick = PaObservation {
            working: true,
            ..woken(35 * MIN)
        };
        assert_eq!(
            pa_verdicts(quick),
            PaVerdicts {
                dormant: PresenceVerdict::Armed,
                unresponsive: PresenceVerdict::Quiet
            }
        );
    }

    /// Plan 217 AC5: over the line crosses once; at or under re-arms; unknown
    /// does neither.
    #[test]
    fn large_seats_cross_once_and_rearm_under_the_line() {
        let (a, b, c, d): (SeatId, SeatId, SeatId, SeatId) =
            ("a".into(), "b".into(), "c".into(), "d".into());
        let sizes = [
            (&a, Some(LARGE_SEAT_TOKENS + 1)),
            (&b, Some(LARGE_SEAT_TOKENS + 1)),
            (&c, Some(LARGE_SEAT_TOKENS)),
            (&d, None),
        ];
        let (crossed, rearmed) = large_seats(sizes, |seat| seat == &b || seat == &c || seat == &d);
        assert_eq!(crossed, vec![a.clone()]);
        assert_eq!(rearmed, vec![c]);
        let body = large_seats_body(
            &[LargeSeat {
                seat: a,
                tokens: 650_000,
                role: Some("worker".into()),
                model: None,
            }],
            &[],
        );
        assert_eq!(
            body,
            "[pij watchdog] over 600k tokens in your fleet (PA: none):\n\
             - a 650k role worker model unknown"
        );
    }
}
