use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::delivery::delivery_kind;
use pij_core::error::{PijError, Result};
use pij_core::model::{Job, Msg, SeatId};
use pij_core::ports::Queue;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The observed result of one bounded consumer pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Processed {
    /// Rows actually claimed, never the configured pass limit.
    pub count: usize,
}

/// An owned long-lived consumer task.
#[derive(Debug)]
pub struct LoopHandle {
    stop: oneshot::Sender<()>,
    joined: JoinHandle<()>,
}

impl LoopHandle {
    /// Stop future passes and wait for the current pass to finish.
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.joined.await;
    }
}

pub(crate) fn start_resilient_loop<F, Fut>(
    name: &'static str,
    interval: Duration,
    mut run_once: F,
) -> Result<LoopHandle>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<Processed>> + Send + 'static,
{
    if interval.is_zero() {
        return Err(PijError::Adapter {
            adapter: format!("sidecars/{name}"),
            message: "consumer interval must be greater than zero".to_string(),
        });
    }
    let (stop, mut stop_rx) = oneshot::channel();
    let joined = tokio::spawn(async move {
        let mut schedule = tokio::time::interval(interval);
        schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = &mut stop_rx => break,
                _ = schedule.tick() => {
                    if let Err(error) = run_once().await {
                        eprintln!("pij-rs {name}: {error}");
                    }
                }
            }
        }
    });
    Ok(LoopHandle { stop, joined })
}

pub(crate) fn now_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: "sidecars/clock".to_string(),
            message: format!("system clock is before Unix epoch: {error}"),
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| PijError::Adapter {
        adapter: "sidecars/clock".to_string(),
        message: "system time does not fit u64 milliseconds".to_string(),
    })
}

pub(crate) async fn enqueue_turn(
    queue: &Arc<dyn Queue>,
    from: &str,
    to: &SeatId,
    body: String,
    msg_id: String,
) -> Result<()> {
    let message = Msg {
        from: SeatId::from(from),
        to: to.clone(),
        body,
        msg_id: msg_id.clone(),
        from_machine: None,
        in_reply_to: None,
        command: None,
    };
    let _ = queue
        .enqueue_delivery(Job {
            kind: delivery_kind(to),
            serial_key: to.to_string(),
            payload: serde_json::to_string(&message).map_err(|error| PijError::Adapter {
                adapter: "sidecars/delivery".to_string(),
                message: format!("could not encode injected turn: {error}"),
            })?,
            dedupe_key: msg_id,
            dedupe_origin: None,
            attempt: 0,
        })
        .await?;
    Ok(())
}
