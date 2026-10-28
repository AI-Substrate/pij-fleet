//! In-process liveness gives exactly the answer `/bin/ps -o lstart=` gives
//! (plan 182 R1, ADR 0001), for every kind of pid the probe can meet.
//!
//! The identity check compares packed local wall-clock seconds, so parity is
//! asserted at that precision, in this machine's timezone and in two others
//! (one with daylight saving), from a multi-threaded process like the daemon.

use std::process::{Command, Stdio};

use pij_core::liveness::alive;
use pij_core::model::{Liveness, ProcIdentity};
use pij_core::ports::LivenessPort;
use pij_harnesses::proc::{ProcLiveness, parse_process_start};
use pij_testkit::block_on;

const CHILD: &str = "PIJ_PARITY_CHILD";

/// Today's mechanism, run directly: `/bin/ps` answers in the C locale.
fn ps_start(pid: u32) -> Option<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
        .expect("run /bin/ps");
    let row = String::from_utf8(output.stdout).expect("UTF-8 ps output");
    let row = row.trim();
    (!row.is_empty()).then(|| parse_process_start(row).expect("parse a ps row"))
}

fn in_process(pid: u32) -> Option<u64> {
    block_on(ProcLiveness::new().proc_start(pid)).expect("probe a pid")
}

/// Self, a live child, a zombie, pid 1 and a reaped pid all match `ps`.
fn assert_parity() {
    let _threads: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| std::thread::sleep(std::time::Duration::from_secs(2))))
        .collect();
    let mut live = Command::new("/bin/sleep")
        .arg("30")
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn a live child");
    let zombie = Command::new("/usr/bin/true")
        .spawn()
        .expect("spawn a child");
    let mut dead = Command::new("/usr/bin/true")
        .spawn()
        .expect("spawn a child");
    dead.wait().expect("reap the child");
    std::thread::sleep(std::time::Duration::from_millis(300));

    for (what, pid) in [
        ("this process", std::process::id()),
        ("a live child", live.id()),
        ("a zombie", zombie.id()),
        ("pid 1", 1),
        ("a reaped pid", dead.id()),
    ] {
        assert_eq!(
            in_process(pid),
            ps_start(pid),
            "{what} (pid {pid}) differs from /bin/ps"
        );
    }
    assert!(ps_start(std::process::id()).is_some());
    let _ = live.kill();
    let _ = live.wait();
    drop(zombie);
}

#[test]
fn in_process_start_times_match_ps_for_every_kind_of_pid() {
    assert_parity();
}

#[test]
fn parity_holds_in_other_timezones() {
    if std::env::var_os(CHILD).is_some() {
        assert_parity();
        return;
    }
    for zone in ["America/New_York", "Australia/Melbourne", "UTC"] {
        let output = Command::new(std::env::current_exe().expect("this test binary"))
            .args([
                "in_process_start_times_match_ps_for_every_kind_of_pid",
                "--exact",
                "--test-threads=1",
            ])
            .env("TZ", zone)
            .env(CHILD, "1")
            .output()
            .expect("re-run the parity test under another TZ");
        assert!(
            output.status.success(),
            "parity failed under TZ={zone}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the child under TZ={zone} ran no test"
        );
    }
}

#[test]
fn a_different_start_at_the_same_pid_reads_recycled_and_a_reaped_pid_dead() {
    let pid = std::process::id();
    let start = in_process(pid).expect("this process exists");
    let liveness = ProcLiveness::new();
    let judge = |proc_start| block_on(alive(ProcIdentity { pid, proc_start }, &liveness));
    assert_eq!(judge(start).expect("probe"), Liveness::Active);
    assert!(matches!(
        judge(start - 1).expect("probe"),
        Liveness::Recycled { observed_start, recorded_start }
            if observed_start == start && recorded_start == start - 1
    ));

    let mut dead = Command::new("/usr/bin/true")
        .spawn()
        .expect("spawn a child");
    dead.wait().expect("reap the child");
    let gone = ProcIdentity {
        pid: dead.id(),
        proc_start: start,
    };
    assert!(matches!(
        block_on(alive(gone, &liveness)).expect("probe"),
        Liveness::Dead { .. }
    ));
}
