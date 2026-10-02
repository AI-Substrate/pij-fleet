//! Warm every live seat's session cursor once at daemon start (plan 157).
//!
//! After a bounce every read position is gone, so the first `pij send` to a
//! large seat would pay a cold transcript fold on the send path, and past the
//! guard's wait the brake would stay off for it. This walks the live seats
//! with a bound session, one at a time with a pause between them so the
//! shared machine is not hit with a burst, and reads each once off the
//! request path. Every answer is discarded: the point is the cursor the
//! source keeps.

use std::sync::Arc;
use std::time::Duration;

use pij_core::ports::{Registry, SeatFilter, SessionStatusPort};
use pij_core::session_status::SessionTarget;

/// The pause between seats, so warming never competes with the fleet in a burst.
pub const WARM_PAUSE: Duration = Duration::from_millis(250);

/// Read each live, bound seat's session once, sequentially. Returns how many
/// were read. A failed read is logged and skipped; warming is best-effort.
pub async fn warm_session_cursors(
    registry: &dyn Registry,
    source: &dyn SessionStatusPort,
    pause: Duration,
) -> usize {
    let seats = match registry.list(SeatFilter::default()).await {
        Ok(seats) => seats,
        Err(error) => {
            eprintln!("pij-rs session warm-up skipped: {error}");
            return 0;
        }
    };
    let mut read = 0;
    for seat in seats {
        if seat.tombstoned_at.is_some() || seat.proc.is_none() {
            continue;
        }
        let Some(session) = seat.harness_session else {
            continue;
        };
        let target = SessionTarget {
            seat: seat.id,
            harness: seat.harness,
            session,
        };
        if let Err(error) = source.status(&target).await {
            eprintln!("pij-rs session warm-up for {}: {error}", target.seat);
        }
        read += 1;
        tokio::time::sleep(pause).await;
    }
    read
}

/// Start the warm-up in the background; the handle is aborted at shutdown.
pub fn start(
    registry: Arc<dyn Registry>,
    source: Arc<dyn SessionStatusPort>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        warm_session_cursors(registry.as_ref(), source.as_ref(), WARM_PAUSE).await;
    })
}

#[cfg(test)]
mod tests {
    use pij_core::model::{Harness, ProcIdentity, SeatDescriptor};
    use pij_testkit::fakes::{FakeRegistry, FakeSessionStatus};

    use super::*;

    fn seat(id: &str, session: Option<&str>, live: bool, tombstoned: bool) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Claude, "/abs/tree");
        seat.harness_session = session.map(str::to_string);
        seat.proc = live.then_some(ProcIdentity {
            pid: 7,
            proc_start: 11,
        });
        seat.tombstoned_at = tombstoned.then_some(1);
        seat
    }

    #[tokio::test]
    async fn only_live_bound_seats_are_read_once_each() {
        let registry = FakeRegistry::new()
            .with_seat(seat("pij-live", Some("s-live"), true, false))
            .with_seat(seat("pij-unbound", None, true, false))
            .with_seat(seat("pij-never-bound", Some("s-prebind"), false, false))
            .with_seat(seat("pij-retired", Some("s-retired"), true, true));
        let source = FakeSessionStatus::new();
        let read = warm_session_cursors(&registry, &source, Duration::ZERO).await;
        assert_eq!(read, 1);
        assert_eq!(source.calls(), ["status:pij-live:claude:s-live"]);
    }
}
