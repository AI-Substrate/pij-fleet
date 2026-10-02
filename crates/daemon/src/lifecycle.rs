//! Daemon identity and the workload-agnostic tick scheduler.
//!
//! Lifecycle owns timing and local machine identity. It deliberately does not
//! know which services tick: callers inject one action, so future workers do not
//! turn the scheduler into a dependency hub.

use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::time::Duration;

use pij_core::error::{PijError, Result};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

/// The facts that identify this daemon to operators, clients, and peer daemons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineIdentity {
    alias: String,
    build: &'static str,
}

impl MachineIdentity {
    /// Resolve the local alias, using the configured value when present and the
    /// operating-system hostname otherwise.
    ///
    /// A configured alias never consults the hostname. This matters on hosts
    /// where hostname lookup is unavailable or changes independently of the
    /// stable federation name chosen by an operator.
    ///
    /// # Composition recipe
    ///
    /// Add `machine_alias: Option<String>` and `tick_interval_secs: u64` to the
    /// composed config, then resolve and wire exactly once:
    ///
    /// ```text
    /// use pij_daemon::lifecycle::{MachineIdentity, TickLoop};
    /// let identity = MachineIdentity::resolve(config.machine_alias.as_deref())?;
    /// let ticks = TickLoop::start(
    ///     Duration::from_secs(config.tick_interval_secs),
    ///     move || tick_services_once(),
    /// )?;
    /// let router = http::router(services, token, identity.clone());
    /// // /health serializes identity.alias() and identity.build().
    /// // The boot banner prints the same two values.
    /// // Daemon::shutdown signals HTTP graceful shutdown first, then awaits
    /// // ticks.shutdown() so accepted requests and an in-progress tick drain.
    /// ```
    ///
    /// At composition, peer-map validation rejects duplicate peer aliases and
    /// any peer alias equal to `identity.alias()`; unqualified seat names remain
    /// local. The scheduler action stays injected rather than gaining one
    /// dependency per future tick consumer.
    ///
    /// # Errors
    /// Returns [`PijError::Adapter`] when the configured alias is empty, hostname
    /// lookup fails, or the hostname is empty or non-Unicode.
    pub fn resolve(configured_alias: Option<&str>) -> Result<Self> {
        Self::resolve_with(configured_alias, hostname::get)
    }

    fn resolve_with<F>(configured_alias: Option<&str>, hostname: F) -> Result<Self>
    where
        F: FnOnce() -> io::Result<OsString>,
    {
        let alias = match configured_alias {
            Some(alias) => alias.to_string(),
            None => hostname()
                .map_err(|error| identity_error(format!("could not read the hostname: {error}")))?
                .into_string()
                .map_err(|_| identity_error("the hostname is not valid Unicode"))?,
        };

        if alias.trim().is_empty() {
            return Err(identity_error(
                "machine alias is empty — configure a non-empty alias or set a hostname",
            ));
        }

        Ok(Self {
            alias,
            build: crate::BUILD,
        })
    }

    /// The local machine alias used by qualified seat addresses.
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// The daemon build answering on this machine.
    pub fn build(&self) -> &'static str {
        self.build
    }
}

fn identity_error(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "daemon/lifecycle".to_string(),
        message: message.into(),
    }
}

/// A non-zero interval proven safe to hand to Tokio's scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickInterval(Duration);

impl TickInterval {
    /// Validate an interval before a daemon publishes its point-of-no-return key.
    ///
    /// # Errors
    /// Returns [`PijError::Adapter`] when `interval` is zero.
    pub fn new(interval: Duration) -> Result<Self> {
        if interval.is_zero() {
            return Err(PijError::Adapter {
                adapter: "daemon/lifecycle".to_string(),
                message: "tick interval must be greater than zero — configure tick_interval_secs"
                    .to_string(),
            });
        }
        Ok(Self(interval))
    }
}

/// A running periodic action.
///
/// The first tick is delayed by one full configured interval. Ticks never
/// overlap: when an action takes longer than its interval, the next tick waits
/// one interval after the delayed completion. Shutdown drains an action already
/// in progress, then prevents all later ticks.
#[derive(Debug)]
pub struct TickLoop {
    stop: oneshot::Sender<()>,
    joined: JoinHandle<Result<()>>,
}

impl TickLoop {
    /// Start a scheduler for an injected async action.
    ///
    /// The action, rather than the scheduler, decides what work one tick means.
    /// Returning an error terminates the loop and makes [`Self::shutdown`]
    /// return that error.
    ///
    /// # Errors
    /// Returns [`PijError::Adapter`] when `interval` is zero.
    pub fn start<F, Fut>(interval: Duration, tick: F) -> Result<Self>
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Ok(Self::start_validated(TickInterval::new(interval)?, tick))
    }

    /// Start after the caller has already validated every fallible boot input.
    pub fn start_validated<F, Fut>(interval: TickInterval, tick: F) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self::start_validated_with_wake(interval, tick, std::future::pending::<()>)
    }

    /// Add an application-owned deadline/notification wake without a second task.
    /// Shutdown and action serialization retain the ordinary scheduler contract.
    pub(crate) fn start_validated_with_wake<F, Fut, W, Wake>(
        interval: TickInterval,
        tick: F,
        wake: W,
    ) -> Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
        W: FnMut() -> Wake + Send + 'static,
        Wake: Future<Output = ()> + Send + 'static,
    {
        let (stop, stop_rx) = oneshot::channel();
        let schedule = tokio::time::interval_at(Instant::now() + interval.0, interval.0);
        let joined = tokio::spawn(run_ticks(stop_rx, schedule, tick, wake));
        Self { stop, joined }
    }

    /// Stop scheduling, drain an in-progress action, and join the task.
    ///
    /// # Errors
    /// Returns the tick action's error, or [`PijError::Adapter`] if the scheduler
    /// task panicked or was cancelled.
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.stop.send(());
        self.joined.await.map_err(|error| PijError::Adapter {
            adapter: "daemon/lifecycle".to_string(),
            message: format!("tick scheduler did not stop cleanly: {error}"),
        })?
    }
}

async fn run_ticks<F, Fut, W, Wake>(
    mut stop_rx: oneshot::Receiver<()>,
    mut schedule: tokio::time::Interval,
    mut tick: F,
    mut wake: W,
) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<()>>,
    W: FnMut() -> Wake,
    Wake: Future<Output = ()>,
{
    schedule.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            _ = &mut stop_rx => return Ok(()),
            _ = wake() => tick().await?,
            _ = schedule.tick() => tick().await?,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{Notify, mpsc};

    use super::*;

    #[test]
    fn a_configured_alias_wins_without_consulting_hostname() {
        let identity = MachineIdentity::resolve_with(Some("workstation-east"), || {
            panic!("a configured alias must not consult hostname")
        })
        .expect("configured alias");

        assert_eq!(identity.alias(), "workstation-east");
        assert_eq!(identity.build(), crate::BUILD);
    }

    #[test]
    fn an_absent_alias_defaults_to_hostname() {
        let identity = MachineIdentity::resolve_with(None, || Ok(OsString::from("host-17")))
            .expect("hostname alias");

        assert_eq!(identity.alias(), "host-17");
    }

    #[test]
    fn hostname_failure_names_the_operator_fix() {
        let error = MachineIdentity::resolve_with(None, || {
            Err(io::Error::new(io::ErrorKind::NotFound, "no hostname"))
        })
        .expect_err("missing hostname must refuse");

        let message = error.to_string();
        assert!(message.contains("hostname"), "observed cause: {message}");
    }

    #[test]
    fn a_zero_tick_interval_refuses_instead_of_panicking() {
        let error = TickLoop::start(Duration::ZERO, || async { Ok(()) })
            .expect_err("zero interval must refuse");
        assert!(error.to_string().contains("tick_interval_secs"));
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_wait_one_configured_interval_repeat_and_stop() {
        let (sent, mut received) = mpsc::unbounded_channel();
        let sequence = Arc::new(AtomicUsize::new(0));
        let loop_ = TickLoop::start(Duration::from_secs(7), {
            let sequence = Arc::clone(&sequence);
            move || {
                let sent = sent.clone();
                let value = sequence.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    sent.send(value).expect("test receiver remains live");
                    Ok(())
                }
            }
        })
        .expect("start");

        tokio::task::yield_now().await;
        assert!(
            received.try_recv().is_err(),
            "the first tick must not run immediately"
        );

        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(received.recv().await, Some(1));
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(received.recv().await, Some(2));

        loop_.shutdown().await.expect("shutdown");
        tokio::time::advance(Duration::from_secs(70)).await;
        assert_eq!(received.recv().await, None, "shutdown drops the producer");
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_wins_when_stop_and_next_tick_are_both_ready() {
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..64 {
            let (stop, stop_rx) = oneshot::channel();
            let schedule = tokio::time::interval_at(Instant::now(), Duration::from_secs(1));
            stop.send(()).expect("receiver remains live");

            run_ticks(
                stop_rx,
                schedule,
                {
                    let calls = Arc::clone(&calls);
                    move || {
                        let calls = Arc::clone(&calls);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    }
                },
                std::future::pending::<()>,
            )
            .await
            .expect("stop");
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no post-stop tick may start"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_drains_the_tick_already_in_progress() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let loop_ = TickLoop::start(Duration::from_secs(3), {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            move || {
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                async move {
                    started.notify_one();
                    release.notified().await;
                    Ok(())
                }
            }
        })
        .expect("start");

        tokio::time::advance(Duration::from_secs(3)).await;
        started.notified().await;

        let shutdown = tokio::spawn(loop_.shutdown());
        tokio::task::yield_now().await;
        assert!(
            !shutdown.is_finished(),
            "shutdown must wait for the accepted tick"
        );

        release.notify_one();
        shutdown
            .await
            .expect("join shutdown waiter")
            .expect("drained shutdown");
    }
}
