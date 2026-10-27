//! The daemon's process-launch counter (plan 180, follow-up C).
//!
//! Every place the daemon launches a process calls [`record`] with an operation
//! name, `<program>.<site>` (for example `ps.proc_liveness` or
//! `tmux.send-keys`), and `pij-rs ping` reports the running totals as
//! `spawns: {total, by_op}`. Launches are what Defender's EDR inspects, so this
//! names the daemon's share by call site, without kernel sampling.
//!
//! It is process-wide state on purpose: the launch sites live in seven crates
//! and run on many threads, and a counter threaded through every constructor
//! would touch far more code than it measures. It only counts. Nothing reads it
//! to decide anything, so it cannot change behaviour.
//! `crates/testkit/tests/spawn_sites.rs` fails when a launch site has no record.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;

/// The counts since the process started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SpawnCounts {
    /// Every recorded launch.
    pub total: u64,
    /// Launches per operation name; the values sum to `total`.
    pub by_op: BTreeMap<String, u64>,
}

static COUNTS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Count one process launch under `op`. Call it where the process is spawned.
pub fn record(op: &str) {
    // A poisoned lock still holds valid counts: a panic cannot interrupt `+= 1`.
    let mut counts = COUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match counts.get_mut(op) {
        Some(count) => *count += 1,
        None => {
            counts.insert(op.to_string(), 1);
        }
    }
}

/// The totals so far, read in one step so `by_op` always sums to `total`.
pub fn snapshot() -> SpawnCounts {
    let by_op = COUNTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    SpawnCounts {
        total: by_op.values().sum(),
        by_op,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(counts: &SpawnCounts, op: &str) -> u64 {
        counts.by_op.get(op).copied().unwrap_or(0)
    }

    // The only test here that records: the counter is process-wide, so a
    // second recording test running in parallel would break the exact deltas.
    #[test]
    fn every_launch_counts_once_in_the_total_and_under_its_operation() {
        let before = snapshot();
        record("test.core-a");
        record("test.core-a");
        record("test.core-b");
        let after = snapshot();
        assert_eq!(after.total - before.total, 3);
        assert_eq!(
            count(&after, "test.core-a") - count(&before, "test.core-a"),
            2
        );
        assert_eq!(
            count(&after, "test.core-b") - count(&before, "test.core-b"),
            1
        );
        assert_eq!(after.by_op.values().sum::<u64>(), after.total);
    }

    #[test]
    fn it_serialises_as_total_and_by_op() {
        let counts = SpawnCounts {
            total: 3,
            by_op: BTreeMap::from([("ps.x".to_string(), 1), ("tmux.y".to_string(), 2)]),
        };
        assert_eq!(
            serde_json::to_value(&counts).unwrap(),
            serde_json::json!({"total": 3, "by_op": {"ps.x": 1, "tmux.y": 2}})
        );
    }
}
