//! Codex binding (plan 181): which Codex thread a pane runs, and the binder
//! that registers a spawned Codex seat from it.
//!
//! A Codex thread is named by the writer lock its process holds open,
//! `<codex home>/thread-writer-locks/<thread>.lock`, from the moment the thread
//! starts. An in-process TUI holds its own lock; a TUI hosted by a shared Codex
//! app-server holds none, so its pane's thread cannot be derived and its seat is
//! never bound from a guess.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, ProcIdentity, SeatDescriptor, SeatId};
use pij_core::ports::{LivenessPort, Registry, SeatFilter, TmuxPort};
use pij_harnesses::SpawnedChild;
use pij_harnesses::codex::{ProcessFiles, held_thread};

use crate::events::EventBus;
use crate::http::Registration;
use crate::http::role::RoleService;
use crate::registration::RegistrationService;

const ADAPTER: &str = "daemon/codex-bind";

/// The Codex thread a pane is running, and the process that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexThread {
    /// The Codex process in the pane that holds the thread.
    pub proc: ProcIdentity,
    /// The thread id, which Codex also exports to shells as `CODEX_THREAD_ID`.
    pub thread: String,
}

/// Which Codex thread a pane runs, derived from that pane alone.
///
/// The answer comes from the Codex process inside the pane's own process tree
/// and the writer lock it holds open. It never comes from a process outside
/// that tree, from the cwd, or from a caller's environment: a shared Codex
/// app-server hands every thread the environment of whichever session started
/// it. A Codex caller's check uses this, per request, only while its seat
/// records no thread.
#[async_trait]
pub trait CodexThreads: Send + Sync {
    /// `None` when no Codex process in the pane holds exactly one thread.
    async fn held_by_pane(&self, pane: &str) -> Result<Option<CodexThread>>;
}

/// The real probe: tmux names the pane's process, one `ps` snapshot selects the
/// Codex process below it, and that process's open files name its thread.
pub struct PaneCodexThreads {
    tmux: Arc<dyn TmuxPort>,
    liveness: Arc<dyn LivenessPort>,
}

impl PaneCodexThreads {
    /// Compose the probe from the daemon's tmux and liveness ports.
    pub fn new(tmux: Arc<dyn TmuxPort>, liveness: Arc<dyn LivenessPort>) -> Self {
        Self { tmux, liveness }
    }
}

#[async_trait]
impl CodexThreads for PaneCodexThreads {
    async fn held_by_pane(&self, pane: &str) -> Result<Option<CodexThread>> {
        let Some(pane_process) = self.tmux.pane_process(pane).await? else {
            return Ok(None);
        };
        let pane_pid = pane_process.pid;
        let held = tokio::task::spawn_blocking(move || {
            let host = pij_tmux::harness_process_tree(pane_pid, Harness::Codex)?;
            let files = pij_harnesses::codex::process_files(host.identity.pid);
            Some(CodexThread {
                proc: host.identity,
                thread: held_thread(&files.open)?,
            })
        })
        .await
        .map_err(join_error)?;
        let Some(held) = held else {
            return Ok(None);
        };
        // The snapshot is a moment old; a recycled pid is not that process.
        let current = self.liveness.proc_start(held.proc.pid).await?;
        Ok((current == Some(held.proc.proc_start)).then_some(held))
    }
}

/// The probe of a daemon running on fakes: no pane holds a Codex thread.
pub struct NoCodexThreads;

#[async_trait]
impl CodexThreads for NoCodexThreads {
    async fn held_by_pane(&self, _pane: &str) -> Result<Option<CodexThread>> {
        Ok(None)
    }
}

/// What the binder reads, each with its cost, so a test can count launches.
#[async_trait]
pub(crate) trait CodexBindFacts: Send + Sync {
    /// Thread ids whose writer locks exist. In-process.
    fn thread_locks(&self) -> BTreeSet<String>;
    /// The launch wrapper's record for this spawn. In-process.
    fn spawned_child(&self, spawn_id: &str) -> Option<SpawnedChild>;
    /// A process's parent and open files. In-process on Linux, one `lsof`
    /// launch elsewhere.
    async fn process_files(&self, pid: u32) -> Result<ProcessFiles>;
    /// The process tmux runs as `pane`. One tmux call.
    async fn pane_pid(&self, pane: &str) -> Result<Option<u32>>;
    /// A process's start. One `ps` launch.
    async fn proc_start(&self, pid: u32) -> Result<Option<u64>>;
}

struct MachineCodexBindFacts {
    codex_home: Option<PathBuf>,
    tmux: Arc<dyn TmuxPort>,
    liveness: Arc<dyn LivenessPort>,
}

#[async_trait]
impl CodexBindFacts for MachineCodexBindFacts {
    fn thread_locks(&self) -> BTreeSet<String> {
        self.codex_home
            .as_deref()
            .map(pij_harnesses::codex::thread_lock_names)
            .unwrap_or_default()
    }

    fn spawned_child(&self, spawn_id: &str) -> Option<SpawnedChild> {
        let (status, _) = crate::http::spawn_evidence_paths(spawn_id);
        SpawnedChild::parse(&std::fs::read_to_string(SpawnedChild::path(&status)).ok()?)
    }

    async fn process_files(&self, pid: u32) -> Result<ProcessFiles> {
        tokio::task::spawn_blocking(move || pij_harnesses::codex::process_files(pid))
            .await
            .map_err(join_error)
    }

    async fn pane_pid(&self, pane: &str) -> Result<Option<u32>> {
        Ok(self
            .tmux
            .pane_process(pane)
            .await?
            .map(|process| process.pid))
    }

    async fn proc_start(&self, pid: u32) -> Result<Option<u64>> {
        self.liveness.proc_start(pid).await
    }
}

/// Binds a spawned Codex seat once its pane runs a thread, as
/// [`crate::claude_bind::ClaudeBinder`] does for Claude from its session record.
///
/// Cost (plan 181 R12). A tick reads the lock directory and the roster, both
/// in-process, and launches nothing unless a Codex thread started since the last
/// tick or a pending spawned Codex row appeared. Then each pending row costs one
/// `process_files`, and a bind costs one tmux call and one `ps`. So launches
/// scale with Codex thread starts times pending rows, never with ticks, and a
/// spawn that never binds costs nothing until the next thread starts.
pub(crate) struct CodexBinder {
    registry: Arc<dyn Registry>,
    facts: Arc<dyn CodexBindFacts>,
    registration: RegistrationService,
    seen: Mutex<Seen>,
}

#[derive(Default)]
struct Seen {
    locks: Option<BTreeSet<String>>,
    rows: BTreeSet<SeatId>,
}

impl CodexBinder {
    pub(crate) fn new(
        registry: Arc<dyn Registry>,
        liveness: Arc<dyn LivenessPort>,
        event_bus: Arc<EventBus>,
        roles: Arc<RoleService>,
        tmux: Arc<dyn TmuxPort>,
    ) -> Self {
        let facts = Arc::new(MachineCodexBindFacts {
            codex_home: pij_harnesses::codex::codex_home(),
            tmux,
            liveness: Arc::clone(&liveness),
        });
        Self::with_facts(registry, liveness, event_bus, roles, facts)
    }

    fn with_facts(
        registry: Arc<dyn Registry>,
        liveness: Arc<dyn LivenessPort>,
        event_bus: Arc<EventBus>,
        roles: Arc<RoleService>,
        facts: Arc<dyn CodexBindFacts>,
    ) -> Self {
        let registration = RegistrationService::new(
            Arc::clone(&registry),
            liveness,
            event_bus,
            pij_harnesses::claude_homes(),
            roles,
        );
        Self {
            registry,
            facts,
            registration,
            seen: Mutex::new(Seen::default()),
        }
    }

    pub(crate) async fn bind_once(&self) -> Result<usize> {
        let locks = self.facts.thread_locks();
        let pending: Vec<SeatDescriptor> = self
            .registry
            .list(SeatFilter::default())
            .await?
            .into_iter()
            .filter(is_pending_spawned_codex)
            .collect();
        let to_probe: Vec<&SeatDescriptor> = {
            let mut seen = self
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let thread_started = seen
                .locks
                .as_ref()
                .is_none_or(|before| !locks.is_subset(before));
            let probe = pending
                .iter()
                .filter(|seat| thread_started || !seen.rows.contains(&seat.id))
                .collect();
            seen.locks = Some(locks);
            seen.rows = pending.iter().map(|seat| seat.id.clone()).collect();
            probe
        };
        let mut bound = 0;
        for seat in to_probe {
            match self.bind_seat(seat).await {
                Ok(true) => bound += 1,
                Ok(false) => {}
                Err(error) => eprintln!("pij-rs codex binder: seat {}: {error}", seat.id),
            }
        }
        Ok(bound)
    }

    async fn bind_seat(&self, seat: &SeatDescriptor) -> Result<bool> {
        let pane = seat
            .pane
            .as_deref()
            .expect("a pending spawned seat has a pane");
        let spawn_id = seat
            .spawn_id
            .as_deref()
            .expect("a pending spawned seat has a spawn id");
        let Some(child) = self.facts.spawned_child(spawn_id) else {
            return Ok(false);
        };
        // The record is a lead. What binds is observed: the child's parent is
        // the wrapper, the child holds exactly one thread, and the wrapper is
        // the pane's own process — so the thread is this pane's (R9).
        let files = self.facts.process_files(child.child_pid).await?;
        if files.parent != Some(child.wrapper_pid) {
            return Ok(false);
        }
        let Some(thread) = held_thread(&files.open) else {
            return Ok(false);
        };
        // A revived seat already names its conversation; another thread in the
        // pane is not that conversation.
        if seat
            .harness_session
            .as_deref()
            .is_some_and(|recorded| recorded != thread)
        {
            return Ok(false);
        }
        if self.facts.pane_pid(pane).await? != Some(child.wrapper_pid) {
            return Ok(false);
        }
        let Some(proc_start) = self.facts.proc_start(child.child_pid).await? else {
            return Ok(false);
        };
        let claim = Registration {
            supersedes: None,
            id: seat.id.to_string(),
            harness: Harness::Codex.as_str().to_string(),
            folder: seat.folder.clone(),
            extension_build: None,
            extension_path: None,
            pane: seat.pane.clone(),
            pid: Some(child.child_pid),
            proc_start: Some(proc_start),
            spawn_id: seat.spawn_id.clone(),
            model: seat.model.clone(),
            actual_model: None,
            actual_model_observed: false,
            provider: seat.provider.clone(),
            effort: seat.effort.clone(),
            parent: seat.parent.clone(),
            role: None,
            relay: seat.relay,
            native_precedence: None,
        };
        match self
            .registration
            .register_with_harness_session(claim, Some(thread))
            .await
        {
            Ok(_) => Ok(true),
            Err(error) => Err(PijError::Adapter {
                adapter: ADAPTER.to_string(),
                message: error.to_string(),
            }),
        }
    }
}

fn is_pending_spawned_codex(seat: &SeatDescriptor) -> bool {
    seat.harness == Harness::Codex
        && seat.proc.is_none()
        && seat.pane.is_some()
        && seat.spawn_id.is_some()
        && seat.tombstoned_at.is_none()
}

fn join_error(error: tokio::task::JoinError) -> PijError {
    PijError::Adapter {
        adapter: ADAPTER.to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
#[path = "codex_bind_tests.rs"]
mod tests;

/// A probe whose answers are a fixed pane -> thread table.
#[cfg(test)]
pub(crate) struct HeldThreads(pub(crate) std::collections::HashMap<String, String>);

#[cfg(test)]
impl HeldThreads {
    pub(crate) fn of(held: &[(&str, &str)]) -> Self {
        Self(
            held.iter()
                .map(|(pane, thread)| ((*pane).to_string(), (*thread).to_string()))
                .collect(),
        )
    }
}

#[cfg(test)]
#[async_trait]
impl CodexThreads for HeldThreads {
    async fn held_by_pane(&self, pane: &str) -> Result<Option<CodexThread>> {
        Ok(self.0.get(pane).map(|thread| CodexThread {
            proc: ProcIdentity {
                pid: 1567,
                proc_start: 20261028072444,
            },
            thread: thread.clone(),
        }))
    }
}
