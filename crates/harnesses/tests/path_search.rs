//! The daemon's probes never search PATH for `ps` or `date` (plan 180, follow-up D).
//!
//! On macOS a bare program name is spawned with `posix_spawnp`, which tries each
//! PATH entry in turn, and every miss creates and tears down a kernel process.
//! With the daemon's 32-entry PATH that was 18 processes per liveness check.
//! The probes now name `/bin/ps` and `/bin/date`, so a `ps` or `date` earlier on
//! PATH is never tried, let alone run.
//!
//! Setting PATH in this process would need `unsafe`, so each test re-runs this
//! test binary as a child with a decoy directory first on PATH. The decoys
//! record that they ran and answer plausibly, so only their marker can tell.

use std::path::{Path, PathBuf};
use std::process::Command;

use pij_core::ports::LivenessPort;
use pij_harnesses::proc::{ProcLiveness, local_utc_offset_minutes};
use pij_testkit::block_on;

const CHILD: &str = "PIJ_DECOY_PATH_CHILD";
const MARKER: &str = "PIJ_DECOY_PATH_MARKER";

fn decoy_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pij-decoy-path-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the decoy dir");
    for (name, answer) in [
        ("ps", "PIJ_LSTART\nThu Jan  1 00:00:00 2015"),
        ("date", "+0000"),
    ] {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho \"$0\" >> \"${MARKER}\"\nprintf '%s\\n' '{answer}'\n"),
        )
        .expect("write a decoy");
        let status = Command::new("/bin/chmod")
            .arg("755")
            .arg(&path)
            .status()
            .expect("chmod a decoy");
        assert!(status.success());
    }
    dir
}

/// Run `test` again in a child whose PATH starts with the decoys; returns the
/// decoys that ran.
fn run_with_decoys_first(test: &str) -> String {
    let dir = decoy_dir(test);
    let marker = dir.join("ran.log");
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.display());
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env("PATH", &path)
        .env(CHILD, "1")
        .env(MARKER, &marker)
        .output()
        .expect("re-run this test as a child");
    assert!(
        output.status.success(),
        "child run failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let ran = std::fs::read_to_string(&marker).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    ran
}

fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

fn assert_no_decoy_ran(ran: &str, what: &str) {
    assert!(
        ran.is_empty(),
        "{what} searched PATH and ran a decoy: {ran:?}. Name the system binary by absolute path."
    );
}

#[test]
fn liveness_runs_bin_ps_never_a_ps_found_on_path() {
    if is_child() {
        // Plan 182: a live pid is answered in-process, so probe a reaped one,
        // which only `ps` can decide. The decoy would answer 2015 for it.
        let mut dead = Command::new("/usr/bin/true")
            .spawn()
            .expect("spawn a child");
        dead.wait().expect("reap the child");
        let start = block_on(ProcLiveness::new().proc_start(dead.id())).expect("probe a dead pid");
        assert_eq!(start, None, "start {start:?} came from the decoy");
        return;
    }
    assert_no_decoy_ran(
        &run_with_decoys_first("liveness_runs_bin_ps_never_a_ps_found_on_path"),
        "ProcLiveness::proc_start",
    );
}

#[test]
fn utc_offset_runs_bin_date_never_a_date_found_on_path() {
    if is_child() {
        local_utc_offset_minutes().expect("read the offset");
        return;
    }
    assert_no_decoy_ran(
        &run_with_decoys_first("utc_offset_runs_bin_date_never_a_date_found_on_path"),
        "local_utc_offset_minutes",
    );
}

#[test]
fn the_decoys_do_run_when_named_by_bare_name() {
    // Proves the harness above can see a PATH search: a bare name finds the decoy.
    if is_child() {
        for program in ["ps", "date"] {
            let status = Command::new(program)
                .stdout(std::process::Stdio::null())
                .status()
                .expect("run the decoy");
            assert!(status.success());
        }
        return;
    }
    let ran = run_with_decoys_first("the_decoys_do_run_when_named_by_bare_name");
    let names: Vec<&str> = ran
        .lines()
        .filter_map(|line| Path::new(line).file_name()?.to_str())
        .collect();
    assert_eq!(names, ["ps", "date"], "decoy log: {ran:?}");
}
