use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pij_core::error::Result;
use pij_core::ports::{Queue, Registry, Spine};

use crate::LoopHandle;
use crate::background::BgWorker;
use crate::chore::ChoreWorker;
use crate::telegram::{TelegramConfig, TelegramWorker};

/// Constructed sidecar workers, not yet running.
///
/// # Composition recipe
/// In `pij-daemon::boot`, after the bind and service construction but before key
/// publication, construct
/// `Sidecars::new(services.queue.clone(), services.spine.clone(),
/// services.registry.clone(), state_dir.join("sidecars"), telegram_env.as_deref())?`. Immediately after key
/// publication call `.start()` and store the returned [`SidecarHandles`] on
/// `Daemon`; join `handles.shutdown()` beside the existing daemon workers. Add
/// `pij-sidecars = { workspace = true }` to `crates/daemon/Cargo.toml`.
pub struct Sidecars {
    background: Arc<BgWorker>,
    chore: Arc<ChoreWorker>,
    telegram: Option<Arc<TelegramWorker>>,
}

impl Sidecars {
    /// Compose concrete edges over the existing queue and spine ports.
    pub fn new(
        queue: Arc<dyn Queue>,
        spine: Arc<dyn Spine>,
        registry: Arc<dyn Registry>,
        state_dir: PathBuf,
        telegram_env: Option<&Path>,
    ) -> Result<Self> {
        let background = Arc::new(BgWorker::new(Arc::clone(&queue), state_dir.join("bg"))?);
        let chore = Arc::new(ChoreWorker::new(
            Arc::clone(&queue),
            state_dir.join("chores.json"),
        )?);
        let telegram = match telegram_env {
            Some(path) if path.exists() => {
                let config = TelegramConfig::load(path)?;
                let lock = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("pij-telegram.lock");
                Some(Arc::new(TelegramWorker::new(
                    queue, spine, registry, config, lock,
                )?))
            }
            _ => None,
        };
        Ok(Self {
            background,
            chore,
            telegram,
        })
    }

    /// Start all configured resilient consumer loops.
    pub fn start(self) -> Result<SidecarHandles> {
        Ok(SidecarHandles {
            telegram: self
                .telegram
                .map(|worker| worker.start(Duration::from_millis(250), 32))
                .transpose()?,
            background: self.background.start(Duration::from_millis(100), 32)?,
            chore: self.chore.start(Duration::from_millis(100), 32)?,
        })
    }
}

/// Owned handles for every running sidecar loop.
#[derive(Debug)]
pub struct SidecarHandles {
    telegram: Option<LoopHandle>,
    background: LoopHandle,
    chore: LoopHandle,
}

impl SidecarHandles {
    /// Stop future claims and join every in-progress consumer pass.
    pub async fn shutdown(self) {
        if let Some(telegram) = self.telegram {
            telegram.shutdown().await;
        }
        self.background.shutdown().await;
        self.chore.shutdown().await;
    }
}
