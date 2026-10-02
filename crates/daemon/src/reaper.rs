//! Stale-record reconciliation, never process or pane termination.
//! Unknown evidence is a brake: removing it permits the same set or more reaps.

use pij_core::error::{PijError, Result};
use pij_core::liveness::alive;
use pij_core::model::{Liveness, SeatDescriptor, SeatId, Seq};
use pij_core::ports::{LivenessPort, Registry, SeatFilter, TmuxPort};
use serde::Serialize;

/// A stale binding proved eligible by process and pane observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReapCandidate {
    /// Registry identity, not a process to signal.
    pub seat: SeatId,
    /// `dead` or `recycled`.
    pub process: &'static str,
    /// `absent`, `reincarnated` (the id exists again, hosting a later process
    /// tree than the seat's), or `paneless`.
    pub pane: &'static str,
    /// Stable explanation for the eligibility decision.
    pub reason: &'static str,
}

/// A binding which cannot safely be reconciled from these observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnverifiableSeat {
    /// Registry identity.
    pub seat: SeatId,
    /// Missing evidence or a concurrent change; never permission to delete.
    pub reason: &'static str,
}

/// One atomically committed tombstone and its shared-spine sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReapedSeat {
    /// Retired registry identity.
    pub seat: SeatId,
    /// Durable tombstone event sequence.
    pub seq: Seq,
}

/// Observations and committed changes from one manual sweep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReapReceipt {
    /// Whether all mutation was disabled.
    pub dry_run: bool,
    /// Active rows at the start of this sweep.
    pub before: usize,
    /// Active rows after this sweep (unchanged for dry-run).
    pub after: usize,
    /// Bindings eligible from the first observation, ordered by seat id.
    pub candidates: Vec<ReapCandidate>,
    /// Only successful conditional commits, never dry-run predictions.
    pub reaped: Vec<ReapedSeat>,
    /// Unknown observations or a binding changed before commit.
    pub unverifiable: Vec<UnverifiableSeat>,
}

enum Eligibility {
    Candidate(ReapCandidate),
    Retain,
    Unknown(&'static str),
}

async fn eligibility(
    seat: &SeatDescriptor,
    liveness: &dyn LivenessPort,
    tmux: &dyn TmuxPort,
) -> Eligibility {
    let Some(proc) = seat.proc else {
        return Eligibility::Unknown("process-identity-unavailable");
    };
    let process = match alive(proc, liveness).await {
        Ok(Liveness::Active) => return Eligibility::Retain,
        Ok(Liveness::Dead { .. }) => "dead",
        Ok(Liveness::Recycled { .. }) => "recycled",
        Err(_) => return Eligibility::Unknown("process-probe-failed"),
    };
    let pane = if let Some(recorded) = &seat.pane {
        match tmux.list_panes().await {
            Ok(panes) if panes.iter().any(|pane| &pane.id == recorded) => {
                match reincarnated(recorded, proc.proc_start, liveness, tmux).await {
                    Ok(true) => "reincarnated",
                    Ok(false) => return Eligibility::Retain,
                    Err(()) => return Eligibility::Unknown("pane-incarnation-probe-failed"),
                }
            }
            Ok(_) => "absent",
            Err(_) => return Eligibility::Unknown("pane-probe-failed"),
        }
    } else {
        "paneless"
    };
    let reason = match (process, pane) {
        ("dead", "absent") => "process-dead-and-pane-absent",
        ("recycled", "absent") => "process-recycled-and-pane-absent",
        ("dead", "reincarnated") => "process-dead-and-pane-reincarnated",
        ("recycled", "reincarnated") => "process-recycled-and-pane-reincarnated",
        ("dead", _) => "process-dead-paneless",
        _ => "process-recycled-paneless",
    };
    Eligibility::Candidate(ReapCandidate {
        seat: seat.id.clone(),
        process,
        pane,
        reason,
    })
}

/// Is the pane now carrying `recorded` a LATER pane than the one the seat lived in?
///
/// Pane ids reset when the tmux server restarts (every reboot), so presence of the
/// number is not presence of the pane. A seat's process descends from its pane's
/// root process, so that root cannot have started after it. A root that did start
/// later is a different pane wearing the same id (plan 156, rule 2).
///
/// This narrows a BRAKE, it adds no policy: the deciding input is still the dead
/// recorded process. Pane presence keeps vetoing whenever the root is as old as
/// the seat or older, or whenever it cannot be observed.
async fn reincarnated(
    recorded: &str,
    seat_start: u64,
    liveness: &dyn LivenessPort,
    tmux: &dyn TmuxPort,
) -> std::result::Result<bool, ()> {
    let Some(root) = tmux.pane_process(recorded).await.map_err(|_| ())? else {
        return Ok(false);
    };
    let root_start = liveness.proc_start(root.pid).await.map_err(|_| ())?;
    Ok(root_start.is_some_and(|start| start > seat_start))
}

/// Reconcile only stale bindings, rechecking observations before atomic commit.
///
/// The registry must compare the entire raw snapshot inside its mutation
/// transaction/order lock. A separate read followed by unconditional tombstone
/// would permit a concurrent revive to be retired using the old process evidence.
///
/// # Errors
/// Returns registry/publication failures. Unknown host probes and conditional
/// conflicts are returned as unverifiable rows, never converted into absence.
pub async fn reap(
    registry: &dyn Registry,
    liveness: &dyn LivenessPort,
    tmux: &dyn TmuxPort,
    dry_run: bool,
) -> Result<ReapReceipt> {
    reap_with_reason(registry, liveness, tmux, dry_run, None)
        .await
        .map(|(receipt, _)| receipt)
}

pub(crate) async fn reap_with_reason(
    registry: &dyn Registry,
    liveness: &dyn LivenessPort,
    tmux: &dyn TmuxPort,
    dry_run: bool,
    tombstone_reason: Option<&str>,
) -> Result<(ReapReceipt, Vec<SeatDescriptor>)> {
    let mut retired = Vec::new();
    let seats = registry.list(SeatFilter::default()).await?;
    let before = seats
        .iter()
        .filter(|seat| seat.tombstoned_at.is_none())
        .count();
    let mut receipt = ReapReceipt {
        dry_run,
        before,
        after: before,
        candidates: Vec::new(),
        reaped: Vec::new(),
        unverifiable: Vec::new(),
    };
    for listed in seats
        .into_iter()
        .filter(|seat| seat.tombstoned_at.is_none())
    {
        // Refresh the raw snapshot before host probes; the final CAS remains authoritative.
        let Some(expected) = registry.get(&listed.id).await? else {
            receipt.unverifiable.push(UnverifiableSeat {
                seat: listed.id,
                reason: "incarnation-changed",
            });
            continue;
        };
        if expected.tombstoned_at.is_some() {
            continue;
        }
        let candidate = match eligibility(&expected, liveness, tmux).await {
            Eligibility::Candidate(candidate) => candidate,
            Eligibility::Retain => continue,
            Eligibility::Unknown(reason) => {
                receipt.unverifiable.push(UnverifiableSeat {
                    seat: expected.id,
                    reason,
                });
                continue;
            }
        };
        receipt.candidates.push(candidate.clone());
        if dry_run {
            continue;
        }
        let reason = match eligibility(&expected, liveness, tmux).await {
            Eligibility::Candidate(current) => current.reason,
            Eligibility::Retain => {
                receipt.unverifiable.push(UnverifiableSeat {
                    seat: expected.id,
                    reason: "liveness-changed",
                });
                continue;
            }
            Eligibility::Unknown(reason) => {
                receipt.unverifiable.push(UnverifiableSeat {
                    seat: expected.id,
                    reason,
                });
                continue;
            }
        };
        let snapshot = tombstone_reason.map(|_| expected.clone());
        match registry
            .tombstone_if_unchanged(expected, tombstone_reason.unwrap_or(reason).to_string())
            .await
        {
            Ok(seq) => {
                receipt.reaped.push(ReapedSeat {
                    seat: candidate.seat,
                    seq,
                });
                if let Some(snapshot) = snapshot {
                    retired.push(snapshot);
                }
            }
            Err(PijError::GovernanceRefused { code, .. }) if code == "E-RS-INCARNATION-CHANGED" => {
                receipt.unverifiable.push(UnverifiableSeat {
                    seat: candidate.seat,
                    reason: "incarnation-changed",
                });
            }
            Err(error) => return Err(error),
        }
    }
    if !dry_run {
        receipt.after = registry
            .list(SeatFilter::default())
            .await?
            .iter()
            .filter(|seat| seat.tombstoned_at.is_none())
            .count();
    }
    Ok((receipt, retired))
}

#[cfg(test)]
#[path = "reaper_tests.rs"]
mod tests;
