//! Plan 181: the Codex binder binds only from the pane's own Codex process, and
//! launches nothing per tick (R9, R12).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use pij_core::error::Result;
use pij_core::model::{Harness, SeatDescriptor};
use pij_core::ports::Registry;
use pij_harnesses::SpawnedChild;
use pij_harnesses::codex::ProcessFiles;
use pij_testkit::fakes::{FakeLiveness, FakeRegistry, FakeSpine};

use super::{CodexBindFacts, CodexBinder};
use crate::events::EventBus;

/// Recorded 2026-10-28, canary pane %122: wrapper 1533, Codex TUI 1567,
/// thread 01a17f40-….
const PANE: &str = "%122";
const SPAWN: &str = "s-87b2599840e57f31783cbccf12f8430e";
const WRAPPER: u32 = 1533;
const CODEX: u32 = 1567;
const THREAD: &str = "01a17f40-5ad4-7ff3-aa69-991589587076";
const START: u64 = 20261028072444;

fn lock(thread: &str) -> String {
    format!("/Users/jordanknight/.codex/thread-writer-locks/{thread}.lock")
}

#[derive(Default)]
struct Facts {
    locks: Mutex<BTreeSet<String>>,
    children: HashMap<String, SpawnedChild>,
    files: HashMap<u32, ProcessFiles>,
    panes: HashMap<String, u32>,
    starts: HashMap<u32, u64>,
    launches: AtomicUsize,
}

impl Facts {
    /// The live canary: the wrapper runs as the pane's process, its child holds
    /// the thread's writer lock.
    fn canary() -> Self {
        let mut facts = Self::default();
        facts.locks.lock().unwrap().insert(THREAD.to_string());
        facts.children.insert(
            SPAWN.to_string(),
            SpawnedChild {
                wrapper_pid: WRAPPER,
                child_pid: CODEX,
            },
        );
        facts.files.insert(
            CODEX,
            ProcessFiles {
                parent: Some(WRAPPER),
                open: vec![lock(THREAD).into()],
            },
        );
        facts.panes.insert(PANE.to_string(), WRAPPER);
        facts.starts.insert(CODEX, START);
        facts
    }

    fn launches(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CodexBindFacts for Facts {
    fn thread_locks(&self) -> BTreeSet<String> {
        self.locks.lock().unwrap().clone()
    }

    fn spawned_child(&self, spawn_id: &str) -> Option<SpawnedChild> {
        self.children.get(spawn_id).copied()
    }

    async fn process_files(&self, pid: u32) -> Result<ProcessFiles> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(self.files.get(&pid).cloned().unwrap_or_default())
    }

    async fn pane_pid(&self, pane: &str) -> Result<Option<u32>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(self.panes.get(pane).copied())
    }

    async fn proc_start(&self, pid: u32) -> Result<Option<u64>> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(self.starts.get(&pid).copied())
    }
}

fn pending() -> SeatDescriptor {
    let mut seat = SeatDescriptor::new(
        "pij-continued-marlin",
        Harness::Codex,
        "/Users/jordanknight/pi-hacking/fleet-181-codex-bind",
    );
    seat.pane = Some(PANE.to_string());
    seat.spawn_id = Some(SPAWN.to_string());
    seat.parent = Some("pij-codex-bind".into());
    seat.model = Some("gpt-6.1-sol".to_string());
    seat
}

async fn binder(registry: Arc<FakeRegistry>, facts: Arc<Facts>) -> CodexBinder {
    let bus = Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 8).expect("event bus"));
    let pool = pij_store::open("").await.expect("isolated role storage");
    let roles = Arc::new(crate::http::role::RoleService::new(
        registry.clone(),
        pij_store::SqliteOrchestration::new(pool),
        bus.clone(),
    ));
    let liveness = Arc::new(
        FakeLiveness::new().with_proc(pij_core::model::ProcIdentity {
            pid: CODEX,
            proc_start: START,
        }),
    );
    CodexBinder::with_facts(registry, liveness, bus, roles, facts)
}

async fn row(registry: &FakeRegistry) -> SeatDescriptor {
    registry
        .get(&"pij-continued-marlin".into())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn a_spawned_codex_seat_binds_to_the_thread_its_own_pane_runs() {
    let registry = Arc::new(FakeRegistry::new());
    registry.put(pending()).await.unwrap();
    let binder = binder(registry.clone(), Arc::new(Facts::canary())).await;

    assert_eq!(binder.bind_once().await.unwrap(), 1);
    let bound = row(&registry).await;
    let proc = bound.proc.expect("bound");
    assert_eq!((proc.pid, proc.proc_start), (CODEX, START));
    assert_eq!(bound.harness_session.as_deref(), Some(THREAD));
    assert_eq!(bound.parent, Some("pij-codex-bind".into()));
    assert_eq!(bound.spawn_id.as_deref(), Some(SPAWN));
}

/// R9: every link to the pane is observed, never taken from the record.
#[tokio::test]
async fn nothing_outside_the_pane_or_the_wrappers_child_can_bind() {
    type Break = fn(&mut Facts);
    let cases: [(&str, Break); 4] = [
        ("a daemon-hosted TUI holds no lock", |facts| {
            facts.files.get_mut(&CODEX).unwrap().open.clear();
        }),
        ("the lock holder is not the wrapper's child", |facts| {
            facts.files.get_mut(&CODEX).unwrap().parent = Some(4242);
        }),
        ("the wrapper is not the pane's process", |facts| {
            facts.panes.insert(PANE.to_string(), 4242);
        }),
        ("no launch record for this spawn", |facts| {
            facts.children.clear()
        }),
    ];
    for (case, break_it) in cases {
        let registry = Arc::new(FakeRegistry::new());
        registry.put(pending()).await.unwrap();
        let mut facts = Facts::canary();
        break_it(&mut facts);
        let binder = binder(registry.clone(), Arc::new(facts)).await;
        assert_eq!(binder.bind_once().await.unwrap(), 0, "{case}");
        assert!(row(&registry).await.proc.is_none(), "{case}");
    }
}

#[tokio::test]
async fn a_revived_seat_binds_only_to_its_recorded_conversation() {
    let registry = Arc::new(FakeRegistry::new());
    let mut revived = pending();
    revived.harness_session = Some("01a17807-3be5-7520-a3ea-2efc9a6b99df".to_string());
    registry.put(revived).await.unwrap();
    let binder = binder(registry.clone(), Arc::new(Facts::canary())).await;
    assert_eq!(binder.bind_once().await.unwrap(), 0);
    assert!(row(&registry).await.proc.is_none());
}

/// R12: a pending row that cannot bind costs launches once when it appears
/// and once per Codex thread start after that, never per tick.
#[tokio::test]
async fn an_unbindable_pending_seat_launches_nothing_per_tick() {
    let registry = Arc::new(FakeRegistry::new());
    registry.put(pending()).await.unwrap();
    let mut facts = Facts::canary();
    facts.files.get_mut(&CODEX).unwrap().open.clear();
    let facts = Arc::new(facts);
    let binder = binder(registry.clone(), facts.clone()).await;

    binder.bind_once().await.unwrap();
    assert_eq!(facts.launches(), 1, "first sight: one process_files");
    for _ in 0..50 {
        binder.bind_once().await.unwrap();
    }
    assert_eq!(facts.launches(), 1, "fifty quiet ticks launch nothing");

    facts
        .locks
        .lock()
        .unwrap()
        .insert("01a17f41-0000-7000-8000-000000000000".to_string());
    binder.bind_once().await.unwrap();
    assert_eq!(facts.launches(), 2, "a Codex thread started: one more look");
    binder.bind_once().await.unwrap();
    assert_eq!(facts.launches(), 2);
}

#[tokio::test]
async fn no_pending_codex_row_launches_nothing() {
    let registry = Arc::new(FakeRegistry::new());
    let facts = Arc::new(Facts::canary());
    let binder = binder(registry, facts.clone()).await;
    for _ in 0..10 {
        assert_eq!(binder.bind_once().await.unwrap(), 0);
    }
    assert_eq!(facts.launches(), 0);
}

/// A bind costs exactly three launches: the child's files, the pane's process,
/// the child's start.
#[tokio::test]
async fn a_bind_costs_three_launches() {
    let registry = Arc::new(FakeRegistry::new());
    registry.put(pending()).await.unwrap();
    let facts = Arc::new(Facts::canary());
    let binder = binder(registry, facts.clone()).await;
    assert_eq!(binder.bind_once().await.unwrap(), 1);
    assert_eq!(facts.launches(), 3);
    binder.bind_once().await.unwrap();
    assert_eq!(facts.launches(), 3, "a bound row is no longer pending");
}
