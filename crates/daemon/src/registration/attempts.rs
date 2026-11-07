//! Registration attempts (plan 211, issue #145): how often each entry path
//! registers a seat, in which mode, with what outcome, and how many ancestry
//! `ps` reads it paid. `pij-rs --json ping` reports them beside `spawns`.
//!
//! In-process counters only: no launch, no event, no store write per attempt.
//! Bounded by construction: path, mode and outcome are enums, and the per-seat
//! tally keeps at most [`SEATS`] addresses of at most [`SEAT_BYTES`] bytes,
//! counting the rest under [`OVERFLOW`]. The counts sit in one mutex-guarded
//! pair of maps, read in one step. Nothing reads them to decide anything, so they cannot change
//! behaviour. The path a client names is advisory: it labels a count, never
//! grants anything.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;

/// The most seat addresses tallied one by one; later ones share [`OVERFLOW`].
pub const SEATS: usize = 64;

/// The longest address tallied by name; a longer one counts under [`OVERFLOW`]
/// (review of #148: 64 keys alone are not a byte bound).
pub const SEAT_BYTES: usize = 64;

/// Where attempts beyond [`SEATS`] distinct addresses, over-long addresses
/// and refused claims on addresses that are not seats are counted.
pub const OVERFLOW: &str = "(other)";

/// One attempt's ancestry reads, closed when the attempt's count is taken
/// (review of #148, P148-F001). An observer that a timeout left running asks
/// [`Self::begin`] before its `ps`: while open the read is counted, once
/// closed it is refused and never launched, so no read goes unattributed.
#[derive(Default)]
pub struct Reads {
    state: Mutex<(bool, u64)>,
}

impl Reads {
    /// Whether a read may launch; counts it when it may.
    // Linux reads ancestry from /proc and launches no ps, so only the macOS
    // observer calls this outside tests.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub fn begin(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.0 {
            return false;
        }
        state.1 += 1;
        true
    }

    /// Close the tally and return it; every later [`Self::begin`] refuses.
    pub fn close(&self) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.0 = true;
        state.1
    }
}

/// Why a registration was attempted: the client's advisory reason on
/// `/v1/register` (`attempt`), or the daemon caller that registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Path {
    /// A client that names no reason (one older than plan 211).
    Unspecified,
    /// A deliberate `pij register` or a client's first registration.
    Explicit,
    /// A receive loop re-registering after a retryable failure.
    Receive,
    /// A heartbeat re-registering after its receiver lease lapsed.
    Heartbeat,
    /// The shim's per-call paneless registration (F44).
    Shim,
    /// A retry after a refused or held registration (standby).
    LeaseRetry,
    /// `pij adopt` and its reclaim.
    Adopt,
    /// The daemon binding a Claude session.
    ClaudeBind,
    /// The daemon binding a Codex thread.
    CodexBind,
    /// A reason this daemon does not know.
    Other,
}

impl Path {
    /// The path a client names: `explicit`, `receive`, `heartbeat`, `shim` or
    /// `lease-retry`; any other value is [`Self::Other`], none
    /// [`Self::Unspecified`].
    pub fn from_client(reason: Option<&str>) -> Self {
        match reason {
            None => Self::Unspecified,
            Some("explicit") => Self::Explicit,
            Some("receive") => Self::Receive,
            Some("heartbeat") => Self::Heartbeat,
            Some("shim") => Self::Shim,
            Some("lease-retry") => Self::LeaseRetry,
            Some(_) => Self::Other,
        }
    }

    /// The word in the counters' keys.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Explicit => "explicit",
            Self::Receive => "receive",
            Self::Heartbeat => "heartbeat",
            Self::Shim => "shim",
            Self::LeaseRetry => "lease-retry",
            Self::Adopt => "adopt",
            Self::ClaudeBind => "claude-bind",
            Self::CodexBind => "codex-bind",
            Self::Other => "other",
        }
    }
}

/// Whether the claim named a tmux pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    Pane,
    Paneless,
}

impl Mode {
    /// The word in the counters' keys.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Paneless => "paneless",
        }
    }
}

/// How an attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    /// A seat row was created.
    New,
    /// An existing seat was bound to a different process.
    Rebound,
    /// The same seat and process: nothing changed.
    Unchanged,
    /// Refused outright.
    Refused,
    /// The claimed address is retired and this session may not take it.
    Retired,
    /// Refused for now; the client may retry.
    Retryable,
    /// Held: another seat owns the pane until its session resumes.
    SessionHold,
    /// Held: a same-process session with newer activity holds the seat.
    HolderCurrent,
    /// The daemon failed.
    Error,
}

impl Outcome {
    /// The word in the counters' keys.
    pub const fn code(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Rebound => "rebound",
            Self::Unchanged => "unchanged",
            Self::Refused => "refused",
            Self::Retired => "retired",
            Self::Retryable => "retryable",
            Self::SessionHold => "session-hold",
            Self::HolderCurrent => "holder-current",
            Self::Error => "error",
        }
    }
}

/// Attempts and the ancestry `ps` reads they paid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub count: u64,
    pub ps: u64,
}

impl Tally {
    fn add(&mut self, ps: u64) {
        self.count += 1;
        self.ps += ps;
    }
}

/// The counts since the daemon started, as `ping` reports them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AttemptCounts {
    /// Every recorded attempt.
    pub total: u64,
    /// Every ancestry `ps` those attempts paid.
    pub ps: u64,
    /// Per `<path>.<mode>.<outcome>`; the counts sum to `total`.
    pub by_key: BTreeMap<String, Tally>,
    /// Per seat address (the resolved seat, else the claimed id, else
    /// `(unnamed)`), at most [`SEATS`] of them plus [`OVERFLOW`].
    pub by_seat: BTreeMap<String, Tally>,
}

#[derive(Default)]
struct Counts {
    by_key: BTreeMap<(Path, Mode, Outcome), Tally>,
    by_seat: BTreeMap<String, Tally>,
}

/// The daemon's registration counters, one per registration service.
#[derive(Default)]
pub struct Attempts {
    counts: Mutex<Counts>,
}

impl Attempts {
    /// Count one attempt by `path`, `mode` and `outcome` for `seat`, which
    /// paid `ps` ancestry reads. `known` says the seat exists (a registration
    /// landed on it, or a refusal named it); an unknown address keeps its name
    /// only when a known attempt already tallied it, else counts under
    /// [`OVERFLOW`], so a client cannot fill the tally with names it made up.
    pub fn record(
        &self,
        path: Path,
        mode: Mode,
        outcome: Outcome,
        seat: &str,
        known: bool,
        ps: u64,
    ) {
        // A poisoned lock still holds valid counts: a panic cannot interrupt `+= 1`.
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counts
            .by_key
            .entry((path, mode, outcome))
            .or_default()
            .add(ps);
        let seat = if seat.is_empty() {
            "(unnamed)"
        } else if seat.len() > SEAT_BYTES || !(known || counts.by_seat.contains_key(seat)) {
            OVERFLOW
        } else {
            seat
        };
        let key = if counts.by_seat.contains_key(seat) || counts.by_seat.len() < SEATS {
            seat
        } else {
            OVERFLOW
        };
        counts.by_seat.entry(key.to_string()).or_default().add(ps);
    }

    /// The totals so far, read in one step so the keys sum to `total`.
    pub fn snapshot(&self) -> AttemptCounts {
        let counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let by_key: BTreeMap<String, Tally> = counts
            .by_key
            .iter()
            .map(|((path, mode, outcome), tally)| {
                (
                    format!("{}.{}.{}", path.code(), mode.code(), outcome.code()),
                    *tally,
                )
            })
            .collect();
        AttemptCounts {
            total: by_key.values().map(|tally| tally.count).sum(),
            ps: by_key.values().map(|tally| tally.ps).sum(),
            by_key,
            by_seat: counts.by_seat.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempts_count_by_path_mode_and_outcome_with_their_ps_reads() {
        let attempts = Attempts::default();
        attempts.record(
            Path::Heartbeat,
            Mode::Pane,
            Outcome::Unchanged,
            "pij-a",
            true,
            2,
        );
        attempts.record(
            Path::Heartbeat,
            Mode::Pane,
            Outcome::Unchanged,
            "pij-a",
            true,
            0,
        );
        attempts.record(
            Path::Shim,
            Mode::Paneless,
            Outcome::Retired,
            "pij-old",
            true,
            3,
        );
        let counts = attempts.snapshot();
        assert_eq!((counts.total, counts.ps), (3, 5));
        assert_eq!(
            counts.by_key["heartbeat.pane.unchanged"],
            Tally { count: 2, ps: 2 }
        );
        assert_eq!(
            counts.by_key["shim.paneless.retired"],
            Tally { count: 1, ps: 3 }
        );
        assert_eq!(counts.by_seat["pij-old"], Tally { count: 1, ps: 3 });
        assert_eq!(
            serde_json::to_value(&counts).unwrap()["by_key"]["shim.paneless.retired"],
            serde_json::json!({"count": 1, "ps": 3})
        );
    }

    #[test]
    fn the_per_seat_tally_is_bounded_and_overflow_still_counts() {
        let attempts = Attempts::default();
        for n in 0..SEATS + 3 {
            attempts.record(
                Path::Receive,
                Mode::Pane,
                Outcome::New,
                &format!("pij-{n}"),
                true,
                1,
            );
        }
        attempts.record(Path::Receive, Mode::Pane, Outcome::New, "pij-0", true, 1);
        let counts = attempts.snapshot();
        assert_eq!(counts.by_seat.len(), SEATS + 1);
        assert_eq!(counts.by_seat[OVERFLOW], Tally { count: 3, ps: 3 });
        assert_eq!(
            counts.by_seat["pij-0"].count, 2,
            "a known seat keeps counting"
        );
        assert_eq!(
            counts
                .by_seat
                .values()
                .map(|tally| tally.count)
                .sum::<u64>(),
            counts.total
        );
    }

    #[test]
    fn an_unknown_address_counts_under_other_until_a_known_attempt_names_it() {
        let attempts = Attempts::default();
        attempts.record(
            Path::Shim,
            Mode::Paneless,
            Outcome::Refused,
            "pij-made-up",
            false,
            0,
        );
        attempts.record(
            Path::Explicit,
            Mode::Pane,
            Outcome::New,
            "pij-real",
            true,
            0,
        );
        attempts.record(
            Path::Receive,
            Mode::Pane,
            Outcome::Refused,
            "pij-real",
            false,
            0,
        );
        let counts = attempts.snapshot();
        assert_eq!(counts.by_seat[OVERFLOW].count, 1);
        assert!(!counts.by_seat.contains_key("pij-made-up"));
        assert_eq!(counts.by_seat["pij-real"].count, 2);
    }

    #[test]
    fn an_over_long_address_counts_under_other() {
        let attempts = Attempts::default();
        attempts.record(
            Path::Shim,
            Mode::Paneless,
            Outcome::Refused,
            &"x".repeat(SEAT_BYTES + 1),
            true,
            0,
        );
        attempts.record(
            Path::Shim,
            Mode::Paneless,
            Outcome::Refused,
            &"y".repeat(SEAT_BYTES),
            true,
            0,
        );
        let counts = attempts.snapshot();
        assert_eq!(counts.by_seat[OVERFLOW].count, 1);
        assert!(counts.by_seat.keys().all(|key| key.len() <= SEAT_BYTES));
    }

    /// Review of #148 (P148-F001): an observer held before its argv read past
    /// the moment its attempt is counted never launches, so the count holds
    /// every read that ran; one that asks while open is counted.
    #[test]
    fn a_read_after_the_attempt_is_counted_never_launches() {
        let reads = std::sync::Arc::new(Reads::default());
        assert!(reads.begin(), "a read while the attempt runs is counted");
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let held = {
            let reads = std::sync::Arc::clone(&reads);
            std::thread::spawn(move || {
                wait.recv().unwrap();
                reads.begin()
            })
        };
        assert_eq!(reads.close(), 1, "the attempt ends: one read");
        go.send(()).unwrap();
        assert!(!held.join().unwrap(), "the held observer may not launch");
        assert_eq!(reads.close(), 1, "and nothing was added after the count");
    }

    #[test]
    fn a_clients_reason_is_a_label_and_unknown_ones_are_other() {
        assert_eq!(Path::from_client(None), Path::Unspecified);
        assert_eq!(Path::from_client(Some("lease-retry")), Path::LeaseRetry);
        assert_eq!(
            Path::from_client(Some("adopt")),
            Path::Other,
            "daemon paths are not client words"
        );
        assert_eq!(Path::from_client(Some(&"x".repeat(10_000))), Path::Other);
    }
}
