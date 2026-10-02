//! Liveness verdicts — pure logic over one observed fact.
//!
//! The rule this module exists to enforce: **a pid is not an identity.** The OS
//! recycles pids, so "is pid 4242 alive?" is the wrong question; the right one is
//! "is the process at pid 4242 the SAME process we bound to?". TS answered the
//! wrong one and produced both failure directions — refusing to revive live
//! seats, and reporting dead ones as healthy.

use crate::error::Result;
use crate::model::{Liveness, ProcIdentity};
use crate::ports::LivenessPort;

/// Judge a recorded process identity against what the machine reports now.
///
/// Three outcomes, never two: a missing process is [`Liveness::Dead`], a
/// matching start time is [`Liveness::Active`], and a DIFFERENT start time at the
/// same pid is [`Liveness::Recycled`]. `Recycled` covers both directions: a
/// later observed start is ordinary pid reuse; an earlier observed start can
/// follow a boot reset because start values are only monotonic within one boot.
/// Both prove the same operational fact — this binding is not the running
/// process — and both require the same safe action. Treating either as `Active`
/// addresses a stranger's process; treating either as `Dead` hides the mismatch.
pub async fn alive(proc: ProcIdentity, port: &dyn LivenessPort) -> Result<Liveness> {
    match port.proc_start(proc.pid).await? {
        None => Ok(Liveness::Dead {
            evidence: format!("no process at pid {}", proc.pid),
        }),
        Some(observed) if observed == proc.proc_start => Ok(Liveness::Active),
        Some(observed) => Ok(Liveness::Recycled {
            observed_start: observed,
            recorded_start: proc.proc_start,
        }),
    }
}
