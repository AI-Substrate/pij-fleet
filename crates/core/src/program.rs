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
//! started is found on its next launch. A candidate counts when
//! `access(X_OK)` allows it, the test `posix_spawnp` itself applies. A kept
//! path that has since vanished (an upgrade moved it) is searched for again,
//! both before a launch and when the launch itself fails with `ENOENT`
//! ([`launch`]). The first PATH match still wins.
//!
//! Like [`crate::spawns`], this is process-wide on purpose, because the launch
//! sites live in several crates. It is the one exception to core's no-IO rule:
//! it reads `PATH` and asks the filesystem about candidates, nothing else. It
//! launches only what its callers' closures launch.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::io;
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

/// Launch `name` through `spawn`, which receives the program to run. If a kept
/// path has vanished between the check and the spawn (`ENOENT`), it is
/// forgotten, `name` is resolved again, and `spawn` runs once more.
///
/// # Errors
/// Whatever `spawn` returns.
pub fn launch<T>(
    name: impl AsRef<OsStr>,
    spawn: impl FnMut(OsString) -> io::Result<T>,
) -> io::Result<T> {
    FOUND.launch(name.as_ref(), std::env::var_os("PATH").as_deref(), spawn)
}

/// [`launch`] for a spawn that is awaited.
///
/// # Errors
/// Whatever `spawn`'s future returns.
pub async fn launch_async<T, F, Fut>(name: impl AsRef<OsStr>, mut spawn: F) -> io::Result<T>
where
    F: FnMut(OsString) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let name = name.as_ref();
    let path = std::env::var_os("PATH");
    let program = FOUND.resolve(name, path.as_deref());
    let kept = program != name;
    match spawn(program).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound && kept && FOUND.forget(name) => {
            spawn(FOUND.resolve(name, path.as_deref())).await
        }
        other => other,
    }
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

    /// Drop the kept path for `name`; `true` when there was one.
    pub fn forget(&self, name: &OsStr) -> bool {
        self.found
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(name)
            .is_some()
    }

    /// [`launch`] against an explicit `PATH` value.
    ///
    /// # Errors
    /// Whatever `spawn` returns.
    pub fn launch<T>(
        &self,
        name: &OsStr,
        path: Option<&OsStr>,
        mut spawn: impl FnMut(OsString) -> io::Result<T>,
    ) -> io::Result<T> {
        let program = self.resolve(name, path);
        let kept = program != name;
        match spawn(program) {
            Err(error) if error.kind() == io::ErrorKind::NotFound && kept && self.forget(name) => {
                spawn(self.resolve(name, path))
            }
            other => other,
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

/// A regular file `access(X_OK)` allows, as `posix_spawnp` requires: mode bits
/// alone would also accept a file executable only by another user or group.
fn executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
        && nix::unistd::access(path, nix::unistd::AccessFlags::X_OK).is_ok()
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

    /// Review of #84, N2: executability is what `access(X_OK)` says, as for
    /// `posix_spawnp`. A file whose only execute bit is its group's is not
    /// executable by its owner, so the search moves on.
    #[test]
    fn executability_is_access_x_ok_not_any_mode_bit() {
        let root = pij_testkit::fresh_dir("pij-program-access");
        let (first, second) = (root.join("first"), root.join("second"));
        tool(&first, 0o610);
        let runnable = tool(&second, 0o755);
        let path = joined(&[&first, &second]);
        assert_eq!(
            Resolver::new().resolve(OsStr::new(NAME), Some(&path)),
            runnable.as_os_str()
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

    /// R3: a kept path that vanishes between the check and the spawn costs one
    /// failed spawn, then the launch resolves again and runs what PATH holds now.
    #[test]
    fn a_launch_that_finds_its_kept_path_gone_resolves_again_once() {
        let root = pij_testkit::fresh_dir("pij-program-relaunch");
        let (old, new) = (root.join("old"), root.join("new"));
        let gone = tool(&old, 0o755);
        let resolver = Resolver::new();
        let path = joined(&[&old, &new]);
        let mut programs = Vec::new();
        let status = resolver.launch(OsStr::new(NAME), Some(&path), |program| {
            if programs.is_empty() {
                // The upgrade lands after the check, before the spawn.
                std::fs::remove_file(&gone).unwrap();
                tool(&new, 0o755);
            }
            programs.push(program.clone());
            std::process::Command::new(program).status()
        });
        assert!(status.expect("the second spawn runs").success());
        assert_eq!(
            programs,
            [gone.clone(), new.join(NAME)].map(PathBuf::into_os_string)
        );
        // A spawn that fails for any other reason is not retried.
        let mut calls = 0;
        let error = resolver
            .launch(OsStr::new(NAME), Some(&path), |_| {
                calls += 1;
                Err::<(), _>(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            })
            .expect_err("a refused spawn stays refused");
        assert_eq!(
            (calls, error.kind()),
            (1, std::io::ErrorKind::PermissionDenied)
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
