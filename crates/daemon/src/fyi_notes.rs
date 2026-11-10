//! What delivery reads from the store for a turn it delivers (F169, F170).
//!
//! Two things, both computed when a turn is handed over, with no loop, timer
//! or launch of their own:
//!
//! - **The PA line** (F169). Every turn handed to a PA ends with one line
//!   naming the seats in its fleet whose peer FYIs are waiting:
//!   `Waiting FYIs: pij-x 3 (oldest 22m, idle), ...`. An FYI wakes no one, so
//!   a hand-off sent as one is easy to miss; the PA sees the pile on every
//!   turn. Per delivered turn, one role lookup by seat; for a PA, one grouped
//!   read of pending FYIs (pending-only index) and, only when some are
//!   waiting, one registry read and an in-process repository lookup per seat
//!   (files only, no git launch).
//! - **The fleet now** (F170). A held state notice (a large seat, an idle
//!   seat) is re-checked when it flushes, against each seat's state in the
//!   registry and its last size from the PA watchdog's round, so the reader
//!   never acts on a size or state that has since changed. One registry read,
//!   only for a delivery that carries FYIs.
//!
//! A failed read adds nothing: the turn goes out as it would have.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::fyi::{HeldFyi, append_block};
use pij_core::fyi_watch::{Backlog, SeatNow, refresh_notice, waiting_line};
use pij_core::model::{Msg, SeatDescriptor, SeatId, SystemState};
use pij_core::ports::{Registry, SeatFilter};
use pij_core::watchdog::PA_ROLE;
use pij_store::SqliteOrchestration;

use crate::community::root::common_dir;

/// What delivery asks for each turn it delivers.
#[async_trait]
pub trait DeliveryNotes: Send + Sync {
    /// The line to append to a turn delivered to `recipient`: `Some` only for
    /// a PA whose fleet has waiting peer FYIs.
    async fn pa_line(&self, recipient: &SeatDescriptor, now_ms: u64) -> Option<String>;

    /// Each seat's state and size now, to re-check held state notices
    /// against when they flush (F170). `None` when it cannot be read: the
    /// notices then go out as they were held.
    async fn fleet_now(&self) -> Option<HashMap<SeatId, SeatNow>>;
}

/// `fyis` with each held state notice re-checked against `fleet` (F170).
#[must_use]
pub fn refreshed(fyis: &[HeldFyi], fleet: Option<&HashMap<SeatId, SeatNow>>) -> Vec<HeldFyi> {
    match fleet {
        Some(fleet) => fyis.iter().map(|fyi| refresh_notice(fyi, fleet)).collect(),
        None => fyis.to_vec(),
    }
}

/// A body handed to `recipient` now, with a PA's waiting-FYIs line appended
/// (F169). Called where a turn is handed over, never where it is queued, so a
/// queued turn's line is as of its delivery (review of #214, F002). A control,
/// a closed recipient, or a body that already ends with the line (a direct
/// typed attempt that was deferred, queued with the text it staged) is left
/// alone.
pub async fn noted(
    notes: Option<&dyn DeliveryNotes>,
    recipient: &SeatDescriptor,
    mut msg: Msg,
    now_ms: u64,
) -> Msg {
    if msg.command.is_some() || recipient.tombstoned_at.is_some() || carries_pa_line(&msg.body) {
        return msg;
    }
    if let Some(notes) = notes
        && let Some(line) = notes.pa_line(recipient, now_ms).await
    {
        msg.body = append_block(&msg.body, &line);
    }
    msg
}

/// Does `body` already end with a waiting-FYIs line?
fn carries_pa_line(body: &str) -> bool {
    body.rsplit_once("\n\n")
        .is_some_and(|(_, last)| last.starts_with(pij_core::fyi_watch::WAITING_LINE_LEAD))
}

/// Each seat's last known context size, written by the PA watchdog's round
/// (plan 217's size reads) and read when a held large-seat notice flushes.
#[derive(Default)]
pub struct SeatSizes(Mutex<HashMap<SeatId, u64>>);

impl SeatSizes {
    /// Replace every size with this round's.
    pub fn replace(&self, sizes: HashMap<SeatId, u64>) {
        if let Ok(mut held) = self.0.lock() {
            *held = sizes;
        }
    }

    fn get(&self, seat: &SeatId) -> Option<u64> {
        self.0.lock().ok().and_then(|held| held.get(seat).copied())
    }
}

/// The store-backed notes.
pub struct StoreNotes {
    registry: Arc<dyn Registry>,
    store: SqliteOrchestration,
    sizes: Arc<SeatSizes>,
}

impl StoreNotes {
    /// Notes over the daemon's registry, store and size board.
    pub fn new(
        registry: Arc<dyn Registry>,
        store: SqliteOrchestration,
        sizes: Arc<SeatSizes>,
    ) -> Self {
        Self {
            registry,
            store,
            sizes,
        }
    }

    async fn is_pa(&self, seat: &SeatId) -> bool {
        match self.store.seat_role(seat).await {
            Ok(role) => role.is_some_and(|assignment| assignment.role == PA_ROLE),
            Err(cause) => {
                eprintln!("pij-rs: PA line: role of {seat}: {cause}");
                false
            }
        }
    }
}

#[async_trait]
impl DeliveryNotes for StoreNotes {
    async fn pa_line(&self, recipient: &SeatDescriptor, now_ms: u64) -> Option<String> {
        if !self.is_pa(&recipient.id).await {
            return None;
        }
        let backlogs = match self.store.pending_peer_fyi_backlogs().await {
            Ok(backlogs) => backlogs,
            Err(cause) => {
                eprintln!("pij-rs: PA line: pending FYIs: {cause}");
                return None;
            }
        };
        if backlogs.iter().all(|row| row.seat == recipient.id) {
            return None;
        }
        let seats = match self.registry.list(SeatFilter::default()).await {
            Ok(seats) => seats,
            Err(cause) => {
                eprintln!("pij-rs: PA line: registry: {cause}");
                return None;
            }
        };
        waiting_line(&in_fleet(recipient, &seats, backlogs), now_ms)
    }

    async fn fleet_now(&self) -> Option<HashMap<SeatId, SeatNow>> {
        let seats = match self.registry.list(SeatFilter::default()).await {
            Ok(seats) => seats,
            Err(cause) => {
                eprintln!("pij-rs: held notices: registry: {cause}");
                return None;
            }
        };
        Some(
            seats
                .into_iter()
                .map(|seat| {
                    let now = SeatNow {
                        state: seat.state,
                        gone: seat.tombstoned_at.is_some(),
                        tokens: self.sizes.get(&seat.id),
                    };
                    (seat.id, now)
                })
                .collect(),
        )
    }
}

/// The backlogs of live seats in the PA's fleet, with their states: the
/// repository of the PA's parent (else the PA's own), as the PA watchdog
/// scopes it, read from the files with no launch. A PA outside any
/// repository sees every live seat. Its own pile rides along on this turn.
fn in_fleet(
    pa: &SeatDescriptor,
    seats: &[SeatDescriptor],
    backlogs: Vec<Backlog>,
) -> Vec<(Backlog, SystemState)> {
    let anchor = pa
        .parent
        .as_ref()
        .and_then(|parent| seats.iter().find(|seat| &seat.id == parent))
        .map_or(pa.folder.as_str(), |parent| parent.folder.as_str());
    let scope = common_dir(Path::new(anchor));
    backlogs
        .into_iter()
        .filter_map(|backlog| {
            let seat = seats
                .iter()
                .find(|seat| seat.id == backlog.seat && seat.tombstoned_at.is_none())?;
            let placed = scope.is_none() || common_dir(Path::new(&seat.folder)) == scope;
            (seat.id != pa.id && placed).then_some((backlog, seat.state))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::model::Harness;

    fn seat(id: &str, folder: &str, state: SystemState) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Omp, folder);
        seat.state = state;
        seat
    }

    fn backlog(seat: &str) -> Backlog {
        Backlog {
            seat: SeatId::from(seat),
            count: 2,
            oldest_ms: 1,
            newest_ms: 2,
        }
    }

    /// The PA sees its fleet's piles, not its own and not a closed seat's;
    /// outside any repository it sees every live seat.
    #[test]
    fn the_pa_line_covers_the_fleet_but_not_the_pa_or_closed_seats() {
        let pa = seat("pij-the-pa", "/nonexistent/one", SystemState::Working);
        let mut closed = seat("pij-closed-seat", "/nonexistent/two", SystemState::Idle);
        closed.tombstoned_at = Some(1);
        let seats = vec![
            pa.clone(),
            seat("pij-busy-seat", "/nonexistent/three", SystemState::Idle),
            closed,
        ];
        let waiting = in_fleet(
            &pa,
            &seats,
            vec![
                backlog("pij-the-pa"),
                backlog("pij-busy-seat"),
                backlog("pij-closed-seat"),
                backlog("pij-unknown-seat"),
            ],
        );
        assert_eq!(waiting, vec![(backlog("pij-busy-seat"), SystemState::Idle)]);
    }
}
