//! Restart a long-lived background task until daemon shutdown (pij-fleet#36).
//!
//! A boot task that returns, with an error or without one, is a capability the
//! daemon silently stops having until the next bounce. [`supervise`] reruns it
//! with capped exponential backoff and returns only when `shutdown` resolves.
//! The task must be safe to rerun from the start: it re-establishes its own
//! subscription and catch-up state on every run.

use std::time::Duration;

use pij_core::error::Result;
use tokio::time::Instant;

/// Restart delays: `initial`, doubling per consecutive failure, capped at `max`.
/// A run that stays up for at least `max` counts as healthy, so its failure
/// starts the sequence again at `initial` instead of waiting the capped delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

/// The daemon's restart policy: 1 s doubling to 30 s.
pub(crate) const RESTART: Backoff = Backoff {
    initial: Duration::from_secs(1),
    max: Duration::from_secs(30),
};

/// Run `task` until `shutdown` resolves, restarting it whenever it returns.
/// `name` labels the log line an operator sees for each restart.
pub(crate) async fn supervise<F, Fut>(
    name: &str,
    backoff: Backoff,
    mut task: F,
    shutdown: impl Future<Output = ()>,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    tokio::pin!(shutdown);
    let mut delay = backoff.initial;
    loop {
        let started = Instant::now();
        let outcome = tokio::select! {
            biased;
            () = &mut shutdown => return,
            outcome = task() => outcome,
        };
        if started.elapsed() >= backoff.max {
            delay = backoff.initial;
        }
        match outcome {
            Err(error) => eprintln!(
                "pij-rs {name} stopped: {error}; restarting in {}ms",
                delay.as_millis()
            ),
            Ok(()) => eprintln!("pij-rs {name} ended; restarting in {}ms", delay.as_millis()),
        }
        tokio::select! {
            biased;
            () = &mut shutdown => return,
            () = tokio::time::sleep(delay) => {}
        }
        delay = delay.saturating_mul(2).min(backoff.max);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pij_core::error::PijError;

    use super::*;

    fn transient() -> PijError {
        PijError::Adapter {
            adapter: "test".into(),
            message: "pool timed out while waiting for an open connection".into(),
        }
    }

    /// Start offsets, in whole seconds from the first run, of every run.
    fn offsets(starts: &Mutex<Vec<Instant>>) -> Vec<u64> {
        let starts = starts.lock().expect("starts");
        starts
            .iter()
            .map(|at| at.duration_since(starts[0]).as_secs())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn consecutive_failures_double_to_the_cap_and_only_shutdown_ends_it() {
        let starts = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let supervisor = tokio::spawn({
            let starts = Arc::clone(&starts);
            supervise(
                "test task",
                RESTART,
                move || {
                    starts.lock().expect("starts").push(Instant::now());
                    async { Err(transient()) }
                },
                async {
                    let _ = stopped.await;
                },
            )
        });
        while starts.lock().expect("starts").len() < 8 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(offsets(&starts), [0, 1, 3, 7, 15, 31, 61, 91]);

        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(1), supervisor)
            .await
            .expect("shutdown ends the backoff wait")
            .expect("supervisor joins");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_after_a_healthy_run_restarts_after_the_initial_delay() {
        let starts = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let supervisor = tokio::spawn({
            let starts = Arc::clone(&starts);
            supervise(
                "test task",
                RESTART,
                move || {
                    let run = {
                        let mut starts = starts.lock().expect("starts");
                        starts.push(Instant::now());
                        starts.len()
                    };
                    async move {
                        // Runs 1 and 2 fail at once; run 3 recovers and stays up
                        // for the cap before failing again.
                        if run == 3 {
                            tokio::time::sleep(RESTART.max).await;
                        }
                        Err(transient())
                    }
                },
                async {
                    let _ = stopped.await;
                },
            )
        });
        while starts.lock().expect("starts").len() < 5 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // fail +1s, fail +2s, healthy 30s then fail +1s (reset), fail +2s.
        assert_eq!(offsets(&starts), [0, 1, 3, 34, 36]);

        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(1), supervisor)
            .await
            .expect("shutdown ends the backoff wait")
            .expect("supervisor joins");
    }
}
