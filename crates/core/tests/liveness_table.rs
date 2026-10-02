use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::liveness::alive;
use pij_core::model::{Liveness, ProcIdentity};
use pij_core::ports::LivenessPort;
use pij_testkit::{block_on, fakes::FakeLiveness};

fn identity(start: u64) -> ProcIdentity {
    ProcIdentity {
        pid: 4242,
        proc_start: start,
    }
}

#[test]
fn matching_identity_is_active() {
    let proc = identity(1_000);
    let port = FakeLiveness::new().with_proc(proc);
    assert_eq!(
        block_on(alive(proc, &port)).expect("liveness verdict"),
        Liveness::Active
    );
}

#[test]
fn missing_pid_is_dead_with_evidence() {
    let proc = identity(1_000);
    assert_eq!(
        block_on(alive(proc, &FakeLiveness::new())).expect("liveness verdict"),
        Liveness::Dead {
            evidence: "no process at pid 4242".to_string(),
        }
    );
}

#[test]
fn later_start_at_the_same_pid_is_recycled() {
    let proc = identity(1_000);
    let port = FakeLiveness::new().with_recycled(proc.pid, 2_000);
    assert_eq!(
        block_on(alive(proc, &port)).expect("liveness verdict"),
        Liveness::Recycled {
            observed_start: 2_000,
            recorded_start: 1_000,
        }
    );
}

#[test]
fn boot_reset_reads_as_a_mismatch() {
    let proc = identity(2_000);
    let port = FakeLiveness::new().with_recycled(proc.pid, 1_000);
    assert_eq!(
        block_on(alive(proc, &port)).expect("liveness verdict"),
        Liveness::Recycled {
            observed_start: 1_000,
            recorded_start: 2_000,
        },
        "an earlier per-boot start still proves this is not the recorded process"
    );
}

struct PermissionDenied;

#[async_trait]
impl LivenessPort for PermissionDenied {
    async fn proc_start(&self, pid: u32) -> Result<Option<u64>> {
        Err(PijError::Adapter {
            adapter: "test/process".to_string(),
            message: format!("permission denied inspecting pid {pid}"),
        })
    }
}

#[test]
fn permission_denied_is_unknown_not_dead() {
    let error = block_on(alive(identity(1_000), &PermissionDenied))
        .expect_err("an uninspectable process must not be reported dead");
    assert!(error.to_string().contains("permission denied"));
}
