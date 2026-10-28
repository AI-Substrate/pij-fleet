//! Resolve `git` and `tmux` through PATH once per daemon (plan 182, pij-fleet#74).
//!
//! On macOS a program named without a path is launched with `posix_spawnp`,
//! which tries each PATH entry in turn, and every miss creates and tears down a
//! kernel process: 12 misses per `git` or `tmux` launch with the daemon's PATH.
//! These tools live wherever the user installed them, so they cannot be named
//! by a fixed absolute path the way `/bin/ps` is. Instead the first launch
//! searches PATH exactly as `posix_spawnp` would, and later launches use the
//! path it found.
//!
//! Only a hit is kept, never a miss, so a tool installed after the daemon
//! started is found on its next launch. A kept path that has since vanished
//! (an upgrade moved it) is searched for again. The first PATH match still wins.
//!
//! Like [`crate::spawns`], this is process-wide on purpose, because the launch
//! sites live in several crates. It is the one exception to core's no-IO rule:
//! it reads `PATH` and the file metadata of candidates, nothing else, and it
//! never launches anything.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `posix_spawnp`'s search path when `PATH` is unset (`_CS_PATH`).
const DEFAULT_PATH: &str = "/usr/bin:/bin";

static FOUND: Resolver = Resolver::new();

/// The program to launch for `name`: the path PATH resolves it to, or `name`
/// unchanged when it holds a `/` or PATH has no such program (the launch then
/// fails, or searches, exactly as it did before).
pub fn resolve(name: impl AsRef<OsStr>) -> OsString {
    FOUND.resolve(name.as_ref(), std::env::var_os("PATH").as_deref())
}

/// Kept PATH hits, by program name.
pub struct Resolver {
    found: Mutex<BTreeMap<OsString, PathBuf>>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    /// An empty resolver; every name is searched on first use.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            found: Mutex::new(BTreeMap::new()),
        }
    }

    /// Resolve `name` against `path` (a `PATH` value), keeping only a hit.
    pub fn resolve(&self, name: &OsStr, path: Option<&OsStr>) -> OsString {
        if name.is_empty() || name.as_encoded_bytes().contains(&b'/') {
            return name.to_os_string();
        }
        // A poisoned lock still holds valid paths: a panic cannot interrupt an insert.
        let mut found = self
            .found
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(kept) = found.get(name) {
            if executable(kept) {
                return kept.clone().into_os_string();
            }
            found.remove(name);
        }
        match search(name, path.unwrap_or(OsStr::new(DEFAULT_PATH))) {
            Some(hit) => {
                found.insert(name.to_os_string(), hit.clone());
                hit.into_os_string()
            }
            None => name.to_os_string(),
        }
    }
}

/// The first PATH entry holding an executable `name`, in PATH order. An empty
/// entry means the current directory, as it does for `posix_spawnp`.
fn search(name: &OsStr, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                PathBuf::from(".").join(name)
            } else {
                dir.join(name)
            }
        })
        .find(|candidate| executable(candidate))
}

fn executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::Resolver;

    fn tool(dir: &Path, mode: u32) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("pij-tool");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn joined(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    const NAME: &str = "pij-tool";

    #[test]
    fn the_first_path_match_wins_and_is_kept() {
        let root = pij_testkit::fresh_dir("pij-program-order");
        let (first, second) = (root.join("first"), root.join("second"));
        let early = tool(&first, 0o755);
        tool(&second, 0o755);
        let resolver = Resolver::new();
        let path = joined(&[&first, &second]);
        assert_eq!(
            resolver.resolve(OsStr::new(NAME), Some(&path)),
            early.as_os_str()
        );
        // Kept: a later search order is not consulted while the hit still exists.
        let reversed = joined(&[&second, &first]);
        assert_eq!(
            resolver.resolve(OsStr::new(NAME), Some(&reversed)),
            early.as_os_str()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_non_executable_match_is_skipped_like_posix_spawnp_does() {
        let root = pij_testkit::fresh_dir("pij-program-mode");
        let (first, second) = (root.join("first"), root.join("second"));
        tool(&first, 0o644);
        let runnable = tool(&second, 0o755);
        let path = joined(&[&first, &second]);
        assert_eq!(
            Resolver::new().resolve(OsStr::new(NAME), Some(&path)),
            runnable.as_os_str()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_miss_is_not_kept_so_a_tool_installed_later_is_found() {
        let root = pij_testkit::fresh_dir("pij-program-install");
        let dir = root.join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let resolver = Resolver::new();
        let path = joined(&[&dir]);
        assert_eq!(resolver.resolve(OsStr::new(NAME), Some(&path)), NAME);
        let installed = tool(&dir, 0o755);
        assert_eq!(
            resolver.resolve(OsStr::new(NAME), Some(&path)),
            installed.as_os_str()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_kept_path_that_vanished_is_searched_again() {
        let root = pij_testkit::fresh_dir("pij-program-vanish");
        let (old, new) = (root.join("old"), root.join("new"));
        let gone = tool(&old, 0o755);
        let resolver = Resolver::new();
        let path = joined(&[&old, &new]);
        assert_eq!(
            resolver.resolve(OsStr::new(NAME), Some(&path)),
            gone.as_os_str()
        );
        std::fs::remove_file(&gone).unwrap();
        let moved = tool(&new, 0o755);
        assert_eq!(
            resolver.resolve(OsStr::new(NAME), Some(&path)),
            moved.as_os_str()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_name_with_a_slash_is_never_searched() {
        let resolver = Resolver::new();
        let empty = OsString::new();
        for name in ["/bin/ps", "./tmux", "fake/tmux"] {
            assert_eq!(resolver.resolve(OsStr::new(name), Some(&empty)), name);
        }
    }
}
