//! F211: which launched spawns have not answered their caller.
//!
//! A spawn launches a pane and writes the seat's pre-bind row, then waits for
//! the seat to register. If the spawn answers with a failure (a timeout, an
//! adapter error while it watched), or never answers at all (the caller hung
//! up, or the daemon restarted under it), the harness can still come up and
//! register. Nobody told the caller the seat exists: pij-sure-eagle sat 22 h
//! after a daemon restart cut its spawn's answer.
//!
//! This keeps one entry per launched spawn until its answer is settled:
//! `Waiting` while a spawn request watches it, `Unreported` once the request
//! failed or ended without answering. The binding registration takes an
//! `Unreported` entry and the daemon tells the spawning parent. A successful
//! answer (bound, or `--no-wait` dispatched) removes the entry: the caller has
//! the seat. On boot, every pre-bind row is `Unreported`: whatever waited for
//! it died with the previous daemon.
//!
//! In memory, keyed by spawn id; entries for launches that never bind stay
//! until the daemon restarts (one small entry per failed spawn).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pij_core::model::SeatDescriptor;

/// Why a spawn's seat was never reported to its caller, when the daemon
/// inherits it at boot.
pub const RESTARTED: &str =
    "the daemon restarted while the spawn waited, so its caller never heard of this seat";

/// Why, when the spawn request ended before it answered.
pub const ABANDONED: &str = "the spawn request ended before it answered (its caller went away)";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Wait {
    Waiting,
    Unreported(String),
}

/// Launched spawns whose answer is not yet settled.
#[derive(Default)]
pub struct SpawnWaits(Mutex<HashMap<String, Wait>>);

impl SpawnWaits {
    /// Boot: every live pre-bind row (a spawn id, no process yet) belonged to
    /// a spawn the previous daemon was waiting on.
    pub fn inherit(&self, seats: &[SeatDescriptor]) {
        let mut waits = self.0.lock().expect("spawn waits");
        for seat in seats {
            if seat.tombstoned_at.is_none()
                && seat.proc.is_none()
                && let Some(spawn_id) = seat.spawn_id.as_deref().filter(|id| !id.is_empty())
            {
                waits.insert(
                    spawn_id.to_string(),
                    Wait::Unreported(RESTARTED.to_string()),
                );
            }
        }
    }

    /// A spawn request now watches this launch. Settle the guard; dropping it
    /// unsettled marks the launch [`ABANDONED`].
    pub fn watch(self: &Arc<Self>, spawn_id: &str) -> WaitGuard {
        self.0
            .lock()
            .expect("spawn waits")
            .insert(spawn_id.to_string(), Wait::Waiting);
        WaitGuard {
            waits: Arc::clone(self),
            spawn_id: spawn_id.to_string(),
            settled: false,
        }
    }

    /// The registration that binds `spawn_id`'s seat: why it was never
    /// reported, once, when it was not. A launch still watched is left alone.
    pub fn take_unreported(&self, spawn_id: &str) -> Option<String> {
        let mut waits = self.0.lock().expect("spawn waits");
        match waits.get(spawn_id) {
            Some(Wait::Unreported(_)) => match waits.remove(spawn_id) {
                Some(Wait::Unreported(reason)) => Some(reason),
                _ => None,
            },
            Some(Wait::Waiting) | None => None,
        }
    }

    fn settle(&self, spawn_id: &str, wait: Option<Wait>) {
        let mut waits = self.0.lock().expect("spawn waits");
        match wait {
            Some(wait) => waits.insert(spawn_id.to_string(), wait),
            None => waits.remove(spawn_id),
        };
    }
}

/// One spawn request's hold on its launch.
pub struct WaitGuard {
    waits: Arc<SpawnWaits>,
    spawn_id: String,
    settled: bool,
}

impl WaitGuard {
    /// The caller has the seat (or nothing was left running): forget it.
    pub fn answered(mut self) {
        self.waits.settle(&self.spawn_id, None);
        self.settled = true;
    }

    /// The caller got a failure: a later binding is reported to the parent.
    pub fn failed(mut self, reason: &str) {
        self.waits
            .settle(&self.spawn_id, Some(Wait::Unreported(reason.to_string())));
        self.settled = true;
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.waits.settle(
                &self.spawn_id,
                Some(Wait::Unreported(ABANDONED.to_string())),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::model::{Harness, ProcIdentity};

    fn pre_bind(id: &str, spawn_id: &str) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Copilot, "/abs/tree");
        seat.spawn_id = Some(spawn_id.to_string());
        seat
    }

    /// Only a live pre-bind row is inherited at boot; a bound or retired seat
    /// was answered or is gone.
    #[test]
    fn boot_inherits_only_live_unbound_launches() {
        let waits = SpawnWaits::default();
        let mut bound = pre_bind("pij-bound", "s-bound");
        bound.proc = Some(ProcIdentity {
            pid: 1,
            proc_start: 1,
        });
        let mut retired = pre_bind("pij-retired", "s-retired");
        retired.tombstoned_at = Some(1);
        waits.inherit(&[pre_bind("pij-orphan", "s-orphan"), bound, retired]);
        assert_eq!(
            waits.take_unreported("s-orphan").as_deref(),
            Some(RESTARTED)
        );
        assert_eq!(waits.take_unreported("s-orphan"), None, "reported once");
        assert_eq!(waits.take_unreported("s-bound"), None);
        assert_eq!(waits.take_unreported("s-retired"), None);
    }

    /// A watched launch is never reported; an answered one is forgotten; a
    /// failed or dropped one is reported once, with its reason.
    #[test]
    fn only_a_launch_whose_caller_was_not_answered_is_reported() {
        let waits = Arc::new(SpawnWaits::default());
        let watching = waits.watch("s-watched");
        assert_eq!(waits.take_unreported("s-watched"), None, "still watched");
        watching.answered();
        assert_eq!(waits.take_unreported("s-watched"), None);

        waits.watch("s-failed").failed("timed out");
        assert_eq!(
            waits.take_unreported("s-failed").as_deref(),
            Some("timed out")
        );

        drop(waits.watch("s-dropped"));
        assert_eq!(
            waits.take_unreported("s-dropped").as_deref(),
            Some(ABANDONED)
        );
    }
}
