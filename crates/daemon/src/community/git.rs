//! The writer's one git launch site.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

/// Variables that would point git at another repository or index; unset, as
/// harness/scripts/government-root.sh does.
const SCRUBBED: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_OPTIONAL_LOCKS",
];

/// Whether a git call may take the index lock just to refresh stat data.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Locks {
    /// A read: never write the index (`GIT_OPTIONAL_LOCKS=0`).
    Read,
    /// A write that needs the index (`add`, `commit`).
    Write,
}

/// Run `git -C <dir> <args>` by the path resolved once per daemon.
pub(super) fn git<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    locks: Locks,
) -> std::io::Result<Output> {
    pij_core::spawns::record("git.community");
    pij_core::program::launch("git", |git| {
        let mut command = Command::new(git);
        for name in SCRUBBED {
            command.env_remove(name);
        }
        if locks == Locks::Read {
            command.env("GIT_OPTIONAL_LOCKS", "0");
        }
        command
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
    })
}
