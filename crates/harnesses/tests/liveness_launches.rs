//! A live process's liveness costs no process launch (plan 182, ADR 0001).
//!
//! Before plan 182 every `ProcLiveness::proc_start` ran `/bin/ps`, which on the
//! live fleet was 13 launches a second. The start instant is now read in-process,
//! and `ps` runs only when that read has no answer.
//!
//! This file holds ONE test on purpose: it reads the process-wide launch counter,
//! so no other test may probe liveness in this binary while it runs.

use std::process::{Command, Stdio};

use pij_core::ports::LivenessPort;
use pij_core::spawns::{SpawnCounts, snapshot};
use pij_harnesses::proc::ProcLiveness;
use pij_testkit::block_on;

fn ps_launches(before: &SpawnCounts, after: &SpawnCounts) -> Vec<(String, u64)> {
    after
        .by_op
        .iter()
        .filter(|(op, _)| op.starts_with("ps."))
        .map(|(op, count)| {
            (
                op.clone(),
                count - before.by_op.get(op).copied().unwrap_or(0),
            )
        })
        .filter(|(_, delta)| *delta > 0)
        .collect()
}

#[test]
fn live_processes_are_read_without_launching_ps() {
    // A multi-threaded process, as the daemon is: the local-time step must still answer.
    let _threads: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| std::thread::sleep(std::time::Duration::from_secs(3))))
        .collect();
    let mut child = Command::new("/bin/sleep")
        .arg("30")
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn a live child");
    let adapter = ProcLiveness::new();

    let before = snapshot();
    for _ in 0..10 {
        for pid in [std::process::id(), child.id()] {
            assert!(
                block_on(adapter.proc_start(pid))
                    .expect("probe a live process")
                    .is_some(),
                "pid {pid} is alive"
            );
        }
    }
    let after = snapshot();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        ps_launches(&before, &after),
        Vec::<(String, u64)>::new(),
        "20 liveness probes of live processes launched `ps`"
    );

    // A zombie has no in-process answer on macOS; `ps` still decides it, and
    // `ping` shows that under its own operation name.
    let zombie = Command::new("/usr/bin/true")
        .spawn()
        .expect("spawn a child");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let before = snapshot();
    let observed = block_on(adapter.proc_start(zombie.id())).expect("probe a zombie");
    let after = snapshot();
    assert!(
        observed.is_some(),
        "a zombie still has a start time, as ps reports"
    );
    if cfg!(target_os = "macos") {
        assert_eq!(
            ps_launches(&before, &after),
            vec![("ps.proc-liveness-fallback".to_string(), 1)]
        );
    }
    drop(zombie);
}
