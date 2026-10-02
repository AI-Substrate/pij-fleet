//! `FreshStore` — a real database per test, destroyed with the value.
//!
//! Shared state between tests is the cheapest way to make a suite lie: order
//! dependence, a passing test that only passes second, a failure nobody can
//! reproduce alone. Every test that wants a store takes one of these instead.
//!
//! Entropy-named so parallel tests cannot collide, and removed on `Drop` —
//! including the `-wal` and `-shm` sidecars, which a naive cleanup leaves behind
//! and which then confuse the NEXT run that happens to reuse the name.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A temporary SQLite database that deletes itself.
pub struct FreshStore {
    path: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary DIRECTORY, created, for tests that need a state dir rather
/// than a database (the daemon's boot key, for instance).
///
/// Uses the samethree-part key as [`FreshStore`] — pid, clock, and a process-local
/// COUNTER — and the counter is the part people leave out. A test that names its
/// directory from the clock alone collides with a sibling whenever the platform's
/// clock granularity is coarser than the gap between two thread starts; on macOS
/// that is microseconds, and it produced exactly one flaky failure here
/// (`the_key_is_0600_and_exists_before_the_socket_can_be_reached`, which passed
/// alone and failed in a full run when a sibling's cleanup deleted its directory).
pub fn fresh_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after 1970")
        .as_nanos();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the fresh directory");
    dir
}

impl FreshStore {
    /// Create a fresh, unique database path. Nothing is written until something
    /// opens it.
    pub fn new() -> Self {
        // Time + pid + a process-local counter: unique across parallel test
        // binaries (pid), across threads inside one (counter), and across runs
        // that reuse a pid (time).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after 1970")
            .as_nanos();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pij-test-{}-{}-{}.sqlite",
            std::process::id(),
            nanos,
            unique
        ));
        FreshStore { path }
    }

    /// The absolute path to hand to `pij_store::open`.
    pub fn path(&self) -> String {
        self.path.to_string_lossy().to_string()
    }

    /// Does the database file exist yet?
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Leak the file deliberately, for a test that wants to inspect a corpse.
    /// Returns the path that will now survive; use it and say why.
    pub fn keep(mut self) -> PathBuf {
        let path = std::mem::take(&mut self.path);
        std::mem::forget(self);
        path
    }
}

impl Default for FreshStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FreshStore {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.path.clone().into_os_string();
            path.push(suffix);
            // Best effort: a test that already removed the file is not a failure,
            // and a failure to clean up must never mask the test's own verdict.
            let _ = std::fs::remove_file(PathBuf::from(path));
        }
    }
}
