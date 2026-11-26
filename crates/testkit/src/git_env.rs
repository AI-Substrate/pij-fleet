//! The git environment a test must never inherit (incident #354).
//!
//! A seat that exported `GIT_DIR`/`GIT_WORK_TREE` ran the tests, and every
//! fixture's `git init`, `git config` and `git worktree add` went to the real
//! shared repository instead of the fixture's temp dir. Every test that launches
//! git, or a process that launches git, builds its command through [`scrub`]
//! (and [`scrub_identity`] where it sets its own author), so the child finds
//! its repository from its working directory and nothing else.
//!
//! The cargo target runner (`harness/scripts/cargo-test-runner.sh`) is the
//! other half: it refuses to start a test binary while `GIT_DIR` or
//! `GIT_WORK_TREE` is set.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// The variables that move git's repository, index, objects or ref namespace.
pub const LOCATION: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// Every `GIT_CONFIG*` variable: `GIT_CONFIG_GLOBAL`, `_SYSTEM`, `_NOSYSTEM`,
/// `_COUNT`, `_KEY_n`, `_VALUE_n` and `_PARAMETERS` (what `git -c` hands on).
pub const CONFIG_PREFIX: &str = "GIT_CONFIG";

/// The author and committer variables, cleared where a test sets its own.
pub const IDENTITY_PREFIXES: [&str; 2] = ["GIT_AUTHOR_", "GIT_COMMITTER_"];

/// Clears the inherited repository, config and namespace variables from
/// `command`. Set any the test needs after this call.
pub fn scrub(command: &mut Command) -> &mut Command {
    for name in LOCATION {
        command.env_remove(name);
    }
    for name in inherited(|name| name.starts_with(CONFIG_PREFIX)) {
        command.env_remove(name);
    }
    command
}

/// Clears every inherited `GIT_AUTHOR_*` and `GIT_COMMITTER_*` variable, for a
/// test that sets its own identity.
pub fn scrub_identity(command: &mut Command) -> &mut Command {
    for name in inherited(|name| IDENTITY_PREFIXES.iter().any(|p| name.starts_with(p))) {
        command.env_remove(name);
    }
    command
}

/// `git` run in `dir`, scrubbed.
pub fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(dir);
    scrub(&mut command);
    command
}

fn inherited(matches: impl Fn(&str) -> bool) -> Vec<OsString> {
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| name.to_str().is_some_and(&matches))
        .collect()
}
