//! Configuration — which implementation each port gets.
//!
//! **The fake is the default.** A fresh checkout runs, tests, and demonstrates
//! itself with no daemon, no tmux, no network and no database file: offline-first
//! is a property of the default config, not a mode you remember to select. Real
//! adapters are opt-in per port, so a test can make exactly one thing real.

use serde::{Deserialize, Serialize};

use crate::model::Harness;

/// Which implementation to wire behind a port.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterChoice {
    /// The deterministic fake from `pij-testkit`.
    #[default]
    Fake,
    /// The real adapter, which performs IO.
    Real,
}

impl AdapterChoice {
    /// Is this the real thing?
    pub const fn is_real(self) -> bool {
        matches!(self, AdapterChoice::Real)
    }
}

/// One choice per port. Adding a port adds a field here, which is a second place
/// the "an eighth port is stop-and-ask" rule shows up as work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adapters {
    /// Seat roster.
    pub registry: AdapterChoice,
    /// Append-only history.
    pub spine: AdapterChoice,
    /// Job queue.
    pub queue: AdapterChoice,
    /// Message transport.
    pub transport: AdapterChoice,
    /// tmux IO.
    pub tmux: AdapterChoice,
    /// Harness quirks.
    pub harness: AdapterChoice,
    /// Process liveness.
    pub liveness: AdapterChoice,
    /// Session facts from harness transcripts.
    pub session_status: AdapterChoice,
}

/// One paired peer machine, from `<state-dir>/peers.toml`.
///
/// `Debug` is written by hand so the key can never reach a log line through
/// `{:?}` on this or on the [`Config`] that holds it (plan 164 ruling 7).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDefinition {
    /// How this machine is addressed in `<seat>@<machine>`. Unique, and never
    /// equal to the local alias.
    pub alias: String,
    /// Base URL of that machine's daemon.
    pub url: String,
    /// The shared bearer credential for this pair.
    pub key: String,
}

impl std::fmt::Debug for PeerDefinition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PeerDefinition")
            .field("alias", &self.alias)
            .field("url", &self.url)
            .field("key", &"<redacted>")
            .finish()
    }
}

/// Everything the composition roots need to build the world.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Which implementation each port gets.
    pub adapters: Adapters,
    /// Harnesses refused for new spawns unless the caller explicitly allows them.
    /// Empty by default; the daemon composition root reads `PIJ_RETIRED_HARNESSES`.
    pub retired_harnesses: Vec<Harness>,
    /// Where the daemon listens. Loopback only: the bearer key is a second lock,
    /// not the first one.
    pub bind_addr: String,
    /// Absolute path to the store file. Empty means in-memory, which is what the
    /// all-fake default uses.
    pub store_path: String,
    /// How long a seat may be silent before the watchdog considers a nudge.
    ///
    /// Configured, never a hardcoded constant: TS's fixed threshold declared
    /// healthy long tool calls to be stalls, and "a default is a number nobody
    /// chose" is exactly how that shipped.
    pub watchdog_interval_secs: u64,
    /// Maximum age of a running queue claim before the queue terminally fails it.
    ///
    /// This is policy, not a safety brake: age changes the outcome to failed.
    /// Expired jobs never return to pending; retrying creates a new attempt.
    pub claim_lease_secs: u64,
    /// Maximum delivered message ids retained for each recipient.
    ///
    /// Federation retries cap at five minutes and observed peak traffic is only
    /// a few messages per recipient per minute. `1_024` is roughly three orders
    /// of magnitude above that practical duplicate window while keeping a
    /// hundred-seat fleet's absolute ceiling to single-digit megabytes. Once a
    /// recipient exceeds the bound, the oldest ids lose duplicate protection.
    pub delivered_id_capacity: usize,
    /// How often the drain worker runs a pass over queued work.
    pub delivery_interval_secs: u64,
    /// How often the daemon drains pane taps and refreshes composer evidence.
    ///
    /// This is independent of delivery: observation answers whether delivery is
    /// polite, while `delivery_interval_secs` decides when queued work is tried.
    /// The default preserves the TypeScript daemon's measured 600 ms cadence.
    pub pane_observer_interval_ms: u64,
    /// How long a veto may outlive the observation that justified it.
    ///
    /// Bounds EVERY interaction veto (recognized-idle composer, Unrecognized
    /// layout, tap latch), not just a parked draft: past this window with no
    /// fresh observation the gate falls back to the tmux mode flag alone.
    /// A deferral that never expires is the over-hold this gate exists to
    /// prevent, so the bound is configured rather than compiled in.
    pub interaction_idle_ms: u64,
    /// How long before the same seat may be told AGAIN that a message is waiting.
    ///
    /// A cadence, not a retry: the body is released immediately on every pass
    /// (E-028), so this only rate-limits the ANNOUNCEMENT. Configured because a
    /// default is a number nobody chose.
    pub pointer_announce_cadence_secs: u64,
    /// Maximum pointer announcements per seat between successful inbox reads.
    ///
    /// Three means one initial pointer plus two reminders: three independent
    /// chances to notice across UI churn, then no indefinite shouting. This is
    /// seat-level because a pointer names no row, and row-level parking would
    /// leave the oldest row at the queue head and starve newer mail. Zero is
    /// invalid; the daemon validates it before publishing its boot key.
    pub pointer_announce_limit: u32,
    /// Maximum live events buffered per subscriber before drops are counted.
    pub event_buffer_capacity: usize,
    /// Base cadence for federated roster polls and stream reconnects.
    ///
    /// This is policy: it controls when a disconnected peer is tried again, so
    /// it is configured and injected rather than hidden in a worker constant.
    pub federation_poll_interval_secs: u64,
    /// Maximum federated reconnect/send retry delay.
    ///
    /// This is policy, not a safety brake: it changes when work is retried.
    pub federation_retry_max_secs: u64,
    /// How long a remote send waits inline for its first forwarding attempt,
    /// so a receiver's refusal (a cold wake) reaches the sender as an answer.
    ///
    /// A latency cap, not a brake: past it the send answers `Queued` and the
    /// same outcome lands later as an event. Above the receiver's bounded
    /// cold-wake reads, below the forwarder's 30 s request timeout.
    pub federation_first_attempt_wait_secs: u64,
    /// This machine's alias on the federated wire. `None` defaults from the
    /// hostname at boot, which lifecycle resolves — a machine that never chose a
    /// name still has to be addressable, and a constant would collide.
    pub machine_alias: Option<String>,
    /// Manually bootstrapped peer machines, IN ORDER.
    ///
    /// A `Vec`, not a map, deliberately: the duplicate-alias rule is a rule about
    /// what an operator WROTE, and a map has already erased the duplicate you are
    /// trying to refuse. Validate the vec, then build the map (u-federation).
    ///
    /// The keys here are distinct from the per-boot local key in both lifetime and
    /// blast radius: the local key dies with the process, a peer key survives it,
    /// and removing an entry from this list is how an operator revokes a machine.
    /// There is exactly ONE source for the inbound ring — a second would let a
    /// revocation succeed in one place and leave the machine reachable through the
    /// other.
    pub peers: Vec<PeerDefinition>,
    /// Allow a non-loopback, non-Tailscale bind on a paired daemon (plan 164
    /// ruling 5). Bearer keys then cross that network in clear, so it is an
    /// explicit `--insecure-bind`, never a default.
    pub insecure_bind: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            adapters: Adapters::default(),
            retired_harnesses: Vec::new(),
            bind_addr: "127.0.0.1:0".to_string(),
            store_path: String::new(),
            watchdog_interval_secs: 20 * 60,
            claim_lease_secs: 5 * 60,
            delivered_id_capacity: 1_024,
            delivery_interval_secs: 5,
            pane_observer_interval_ms: 600,
            interaction_idle_ms: 60_000,
            pointer_announce_cadence_secs: 90,
            pointer_announce_limit: 3,
            event_buffer_capacity: 1_024,
            federation_poll_interval_secs: 1,
            federation_retry_max_secs: 5 * 60,
            federation_first_attempt_wait_secs: 10,
            machine_alias: None,
            peers: Vec::new(),
            insecure_bind: false,
        }
    }
}

impl Config {
    /// Does this config touch anything outside the process?
    ///
    /// Used by tests and by the daemon's boot banner: a config that claims to be
    /// offline and is not should say so before it fails at a socket.
    pub fn is_fully_offline(&self) -> bool {
        let a = &self.adapters;
        ![
            a.registry,
            a.spine,
            a.queue,
            a.transport,
            a.tmux,
            a.harness,
            a.liveness,
            a.session_status,
        ]
        .iter()
        .any(|choice| choice.is_real())
    }
}
