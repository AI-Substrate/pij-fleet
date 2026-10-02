use std::path::Path;

use pij_core::error::{PijError, Result};
use pij_daemon::{DAEMON_RUNTIME_FILE, DaemonRuntime};

pub(super) fn read_runtime(state_dir: &Path) -> Result<DaemonRuntime> {
    let path = state_dir.join(DAEMON_RUNTIME_FILE);
    let bytes = std::fs::read(&path).map_err(|error| PijError::Adapter {
        adapter: "daemon/bounce".to_string(),
        message: format!(
            "could not read daemon runtime record {} ({error}) — is the daemon running?",
            path.display()
        ),
    })?;
    serde_json::from_slice(&bytes).map_err(|error| PijError::Adapter {
        adapter: "daemon/bounce".to_string(),
        message: format!(
            "daemon runtime record {} is malformed: {error}",
            path.display()
        ),
    })
}
