//! Codex facts pij derives from a Codex process itself (plan 181).
//!
//! Since Codex 0.157 a TUI may run its threads in one shared app-server daemon,
//! which keeps the environment of whichever TUI started it. A tool shell's
//! `PIJ_SESSION_ID` and `TMUX_PANE` can therefore name another seat, while its
//! `CODEX_THREAD_ID` is always its own. The thread a pane is running is read
//! from the Codex process in that pane: an in-process TUI holds
//! `<codex home>/thread-writer-locks/<thread>.lock` open from the moment its
//! thread starts, before the first turn writes a rollout. A daemon-hosted TUI
//! holds no lock, and then the pane's thread cannot be derived.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use pij_core::model::{Harness, Readiness};

use crate::adapter::classify_readiness;

const THREAD_LOCK_DIR: &str = "thread-writer-locks";

/// The Codex thread whose writer lock `open` contains, when exactly one does.
///
/// `open` is the list of paths one process holds open. Two different threads
/// answer `None`: the pane's thread is then ambiguous, and a guess could bind
/// another seat's conversation.
#[must_use]
pub fn held_thread(open: &[PathBuf]) -> Option<String> {
    let mut threads = open.iter().filter_map(|path| thread_of_lock(path));
    let first = threads.next()?;
    threads.all(|other| other == first).then_some(first)
}

fn thread_of_lock(path: &Path) -> Option<String> {
    if path.parent()?.file_name()? != THREAD_LOCK_DIR {
        return None;
    }
    let thread = path.file_name()?.to_str()?.strip_suffix(".lock")?;
    is_uuid(thread).then(|| thread.to_string())
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Codex's home: `$CODEX_HOME`, else `~/.codex`.
#[must_use]
pub fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

/// The thread ids whose writer locks exist under `codex_home`, read in-process.
/// A new name means a Codex thread started somewhere on this machine.
#[must_use]
pub fn thread_lock_names(codex_home: &Path) -> BTreeSet<String> {
    std::fs::read_dir(codex_home.join(THREAD_LOCK_DIR))
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| thread_of_lock(&entry.path()))
        .collect()
}

/// One process's parent and the paths it holds open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessFiles {
    /// The parent pid, as observed with the open files.
    pub parent: Option<u32>,
    /// Every path the process holds open.
    pub open: Vec<PathBuf>,
}

/// `pid`'s parent and open paths: read in-process from `/proc` on Linux, one
/// `/usr/sbin/lsof` launch elsewhere. Any failure answers an empty record,
/// which derives nothing.
#[must_use]
pub fn process_files(pid: u32) -> ProcessFiles {
    if let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
        let parent = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                // `pid (comm) state ppid …`; comm may contain spaces or `)`.
                stat.rsplit_once(')')?
                    .1
                    .split_whitespace()
                    .nth(1)?
                    .parse()
                    .ok()
            });
        return ProcessFiles {
            parent,
            open: entries
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| std::fs::read_link(entry.path()).ok())
                .collect(),
        };
    }
    pij_core::spawns::record("lsof.codex-process-files");
    let Ok(output) = Command::new(LSOF)
        .args(["-n", "-P", "-p", &pid.to_string(), "-FRn"])
        .env("LC_ALL", "C")
        .output()
    else {
        return ProcessFiles::default();
    };
    parse_lsof(&String::from_utf8_lossy(&output.stdout))
}

const LSOF: &str = "/usr/sbin/lsof";

fn parse_lsof(output: &str) -> ProcessFiles {
    let mut files = ProcessFiles::default();
    for line in output.lines() {
        if let Some(parent) = line.strip_prefix('R') {
            files.parent = parent.parse().ok();
        } else if let Some(path) = line.strip_prefix('n') {
            files.open.push(PathBuf::from(path));
        }
    }
    files
}

/// What keeps a Codex pane off its prompt, named from the screen, for a spawn
/// that timed out. `None` when the prompt is showing or Codex is busy.
#[must_use]
pub fn launch_blocker(capture: &str) -> Option<String> {
    if capture.contains("Skip until next version") {
        let header = capture
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("Update available"));
        return Some(header.map_or_else(
            || "Codex is showing its update prompt".to_string(),
            |header| format!("Codex is showing its update prompt (`{header}`)"),
        ));
    }
    match classify_readiness(Harness::Codex, capture) {
        Readiness::NotYet { observed } => Some(format!(
            "Codex is not at its prompt; the screen ends `{observed}`"
        )),
        Readiness::Ready | Readiness::Busy => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{held_thread, launch_blocker, parse_lsof, process_files, thread_lock_names};

    /// Recorded 2026-10-28 with `lsof -p` on two live Codex 0.164 TUIs: pane
    /// %113 after its first turn, pane %122 before it.
    fn recorded(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_writer_lock_names_the_pane_thread_before_and_after_the_first_turn() {
        let before = recorded(&[
            "/Users/jordanknight/.codex/queue_1.sqlite",
            "/Users/jordanknight/.codex/thread-writer-locks/01a17f40-5ad4-7ff3-aa69-991589587076.lock",
        ]);
        assert_eq!(
            held_thread(&before).as_deref(),
            Some("01a17f40-5ad4-7ff3-aa69-991589587076")
        );
        let after = recorded(&[
            "/Users/jordanknight/.codex/sessions/2026/10/28/rollout-2026-10-28T07-03-38-01a17f2d-01da-7993-9a13-ade803d1189a.jsonl",
            "/Users/jordanknight/.codex/thread-writer-locks/01a17f2d-01da-7993-9a13-ade803d1189a.lock",
        ]);
        assert_eq!(
            held_thread(&after).as_deref(),
            Some("01a17f2d-01da-7993-9a13-ade803d1189a")
        );
    }

    #[test]
    fn no_lock_or_two_threads_derive_nothing() {
        assert_eq!(
            held_thread(&recorded(&["/Users/jordanknight/.codex/logs_2.sqlite"])),
            None,
            "a daemon-hosted TUI holds no writer lock"
        );
        assert_eq!(
            held_thread(&recorded(&[
                "/c/thread-writer-locks/01a17f2d-01da-7993-9a13-ade803d1189a.lock",
                "/c/thread-writer-locks/01a17f40-5ad4-7ff3-aa69-991589587076.lock",
            ])),
            None
        );
        assert_eq!(
            held_thread(&recorded(&["/c/thread-writer-locks/not-a-thread.lock"])),
            None
        );
    }

    /// The real reader sees a file this very process holds open, and its parent.
    #[test]
    fn process_files_lists_a_file_this_process_holds() {
        let dir = pij_testkit::fresh_dir("pij-codex-open-paths");
        let path = dir.join("held.lock");
        let _held = std::fs::File::create(&path).expect("create");
        let canonical = path.canonicalize().expect("canonical");
        let files = process_files(std::process::id());
        assert!(
            files
                .open
                .iter()
                .any(|seen| seen.canonicalize().is_ok_and(|seen| seen == canonical)),
            "{} not among {} open paths",
            canonical.display(),
            files.open.len()
        );
        assert_eq!(files.parent, Some(std::os::unix::process::parent_id()));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `lsof -n -P -p 1567 -FRn` on the live Codex TUI in pane %122,
    /// 2026-10-28, trimmed to the fields read.
    #[test]
    fn lsof_fields_give_the_parent_and_paths() {
        let files = parse_lsof(
            "p1567\nR1533\nfcwd\nn/Users/jordanknight/pi-hacking/fleet-181-codex-bind\nf46\nn/Users/jordanknight/.codex/thread-writer-locks/01a17f40-5ad4-7ff3-aa69-991589587076.lock\n",
        );
        assert_eq!(files.parent, Some(1533));
        assert_eq!(
            held_thread(&files.open).as_deref(),
            Some("01a17f40-5ad4-7ff3-aa69-991589587076")
        );
    }

    #[test]
    fn lock_names_are_read_from_the_lock_directory() {
        let home = pij_testkit::fresh_dir("pij-codex-home");
        let locks = home.join("thread-writer-locks");
        std::fs::create_dir_all(&locks).expect("lock dir");
        std::fs::write(locks.join("01a17f40-5ad4-7ff3-aa69-991589587076.lock"), "").expect("lock");
        std::fs::write(locks.join("README"), "").expect("other");
        assert_eq!(
            thread_lock_names(&home).into_iter().collect::<Vec<_>>(),
            ["01a17f40-5ad4-7ff3-aa69-991589587076"]
        );
        assert!(thread_lock_names(&home.join("absent")).is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn the_recorded_update_prompt_is_named() {
        let capture =
            include_str!("../tests/fixtures/2026-10-28-codex-v0.164-pane-114-update-prompt.txt");
        assert_eq!(
            launch_blocker(capture).as_deref(),
            Some("Codex is showing its update prompt (`Update available · 0.164.0 → 0.165.0`)")
        );
    }

    /// The spawn waiter captures 20 lines, which cut the header off in the
    /// recorded spawn (pane %114); the menu alone still names the prompt.
    #[test]
    fn the_update_prompt_is_named_without_its_header() {
        let capture =
            include_str!("../tests/fixtures/2026-10-28-codex-v0.164-pane-114-update-prompt.txt");
        let menu: String = capture
            .lines()
            .skip_while(|line| !line.starts_with('›'))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            launch_blocker(&menu).as_deref(),
            Some("Codex is showing its update prompt")
        );
    }

    #[test]
    fn a_pane_at_its_prompt_has_no_blocker() {
        let capture = include_str!("../tests/fixtures/2026-10-28-codex-v0.164-pane-113.txt");
        assert_eq!(launch_blocker(capture), None);
    }

    #[test]
    fn any_other_screen_is_quoted() {
        let capture = include_str!("../tests/fixtures/2026-10-26-codex-v0.164-pane-53.txt")
            .lines()
            .next()
            .expect("warning line");
        assert_eq!(
            launch_blocker(capture).as_deref(),
            Some(
                "Codex is not at its prompt; the screen ends `⚠ 5h limit: 49% left · resets at 12:33 AM on 27 Oct · /status`"
            )
        );
    }
}
