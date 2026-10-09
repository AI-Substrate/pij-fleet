//! The daemon: composition root #1.
//!
//! Wires `Arc<dyn Port>` from config, serves HTTP on loopback, and does nothing
//! else — wiring, never business logic. At hello depth it serves `/health` and
//! an NDJSON event stream; the verb surface arrives in later waves, and each
//! route's job is to do ONE cheap statement or enqueue a job (R4).

#![deny(missing_docs)]

pub mod admission;
pub mod auth;
pub mod background;
pub mod bg_routing;
mod claude_bind;
pub mod death_sweep;
pub mod delivery;
pub mod events;
pub mod federation;
pub mod lifecycle;
pub mod pa_watchdog;
pub mod pane_observer;
pub mod park_notice;
pub mod pointer;
pub mod reaper;
mod registration;
mod serve;
pub mod session_warmup;

// u-orchestration's git-facts gatherer: IO belongs at the runtime root, and a
// ninth crate would have been a graph change for one module.
pub mod http;
#[path = "orchestration/repo.rs"]
pub mod orchestration_repo;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pij_core::config::{AdapterChoice, Config};
use pij_core::error::{PijError, Result};
use pij_core::model::{Event, Harness, ProcIdentity, Seq};
use pij_core::orchestration::OrchestrationService;
use pij_core::ports::{
    HarnessPort, LivenessPort, Queue, Registry, SessionStatusPort, Spine, TmuxPort, Transport,
};
use pij_harnesses::proc::ProcLiveness;
use pij_harnesses::{HarnessRegistry, InteractionGate};
use pij_store::{SqliteQueue, SqliteRegistry, SqliteSpine};
use pij_testkit::fakes::{
    FakeHarness, FakeLiveness, FakeQueue, FakeRegistry, FakeSessionStatus, FakeTmux, FakeTransport,
};
use pij_tmux::TmuxAdapter;
use pij_transport::UdsTransport;

pub use auth::BootKey;

/// The build id reported in the event stream's Hello line.
pub const BUILD: &str = concat!("pij-rs ", env!("CARGO_PKG_VERSION"));

/// File naming the exact process that won the daemon bind.
pub const DAEMON_RUNTIME_FILE: &str = "daemon.runtime.json";

/// One shared grace period for admitted work and both store pools after producers stop.
pub const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Durable process identity and launch facts used by `pij daemon bounce`.
///
/// The pid and start stamp are indivisible: signalling a pid without first
/// matching its start stamp can kill an unrelated process after pid reuse.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct DaemonRuntime {
    /// Process identity observed after this daemon won the bind.
    pub process: ProcIdentity,
    /// The resolved address this process owns.
    pub addr: SocketAddr,
    /// Machine alias used to distinguish local seats in a federated roster.
    pub machine: String,
    /// Whether the daemon was launched with all adapters fake.
    pub offline: bool,
}

/// Everything the routes are allowed to touch: ALL SEVEN ports.
///
/// Seven fields, not two. An earlier version wired only `registry` and `spine`
/// while `offline` was computed from all seven config choices, so selecting a
/// real transport reported `offline=false` while constructing no transport at
/// all — a composition root that lies about what it composed. Found in review.
#[derive(Clone)]
pub struct Services {
    /// Seat roster.
    pub registry: Arc<dyn Registry>,
    /// Append-only history.
    pub spine: Arc<dyn Spine>,
    /// Deferred work.
    pub queue: Arc<dyn Queue>,
    /// Message delivery.
    pub transport: Arc<dyn Transport>,
    /// tmux IO.
    pub tmux: Arc<dyn TmuxPort>,
    /// Shared human-interaction veto for every pane-facing delivery path.
    pub interaction: Arc<InteractionGate>,
    /// Per-harness adapters, one per `Harness` variant.
    ///
    /// A registry rather than a single `Arc<dyn HarnessPort>`: the wave-0 shape
    /// assumed one fake, but a composite's `kind()` has no truthful value, so
    /// u-harness shipped five adapters and a lookup instead of one adapter
    /// pretending to be all of them.
    /// Wrapped in `Arc` at composition, not in the unit: `Services` is `Clone`
    /// because every route handler holds one, and the registry owns trait
    /// objects that cannot be. Shared ownership is a composition concern.
    pub harnesses: Arc<HarnessRegistry>,
    /// Process liveness.
    pub liveness: Arc<dyn LivenessPort>,
    /// Session facts from harness transcripts, for `pij state`'s `sessionStatus`.
    pub session_status: Arc<dyn SessionStatusPort>,
    /// The ONE delivery path.
    ///
    /// `/v1/send` used to enqueue a private payload shape of its own and
    /// synthesise a `Queued` receipt, which meant the route consulted no routing
    /// policy, no tombstone, no transport, and published no event — and the
    /// payload it wrote could not be decoded by the inbox that read the same
    /// queue. Two halves of one round trip, each tested against itself.
    /// Found by u-extension at compose.
    pub delivery: Arc<delivery::DeliveryService>,
    /// The event bus: publish once, fan out to every subscriber, with a durable
    /// leg on the spine so a subscriber can replay what it missed (u-events).
    ///
    /// Not behind an `AdapterChoice`: it is not one of the seven ports, it is a
    /// service composed FROM one (the spine). There was no `not_yet` refusal to
    /// delete here — its coder said so rather than inventing one.
    pub event_bus: Arc<events::EventBus>,
    /// Governance: batons, projects, streams, ordinals, roles — and the
    /// equivalence merge that stops one seat becoming six descriptors.
    ///
    /// Like the bus, not behind an `AdapterChoice`: it composes existing store
    /// and daemon IO rather than implementing an eighth port, so there was no
    /// refusal to delete.
    pub orchestration: Arc<OrchestrationService>,
    /// Shared asserted-role authority, joined onto local read projections.
    pub roles: Arc<http::role::RoleService>,
    /// Governance records and delivery-receipt projection on the event spine pool.
    pub governance: Arc<http::governance::GovernanceService>,
    /// Durable questions and answers on the shared publication and delivery paths.
    pub decisions: Arc<http::decisions::DecisionService>,
    /// Existing anomaly detector supplied by the shared governance records.
    pub anomalies: Arc<http::anomalies::AnomalyService>,
    /// Persistent detached commands and completion delivery.
    pub background: Arc<background::BackgroundService>,
    /// Row-consistent badge inputs and event freshness, without process fan-out.
    pub status: pij_store::status::SqliteStatus,
    store_pools: [pij_store::StorePool; 2],
    /// Machine-local spawn policy, shared with clients through daemon health.
    pub retired_harnesses: Arc<[Harness]>,
    /// Claude configuration homes whose `sessions/<pid>.json` records the
    /// daemon reads as each Claude process's own report of its conversation.
    pub claude_homes: Arc<[std::path::PathBuf]>,
    /// Whether this daemon is running entirely on fakes.
    pub offline: bool,
}

impl Services {
    /// Opt-in watchdog rows, on the spine pool that holds the other
    /// orchestration records (roles, batons, streams).
    pub fn watchdogs(&self) -> pij_store::SqliteOrchestration {
        pij_store::SqliteOrchestration::new(self.store_pools[0].clone())
    }

    /// The default opt-in interval: the PA interval (`PIJ_RS_WATCHDOG_SECS`,
    /// else the configured 20 minutes).
    pub fn watchdog_default_secs(&self) -> u64 {
        let configured = pij_core::config::Config::default().watchdog_interval_secs;
        pa_watchdog::interval_secs(configured).unwrap_or(configured)
    }
}

/// A running daemon.
pub struct Daemon {
    /// Where it is actually listening — resolved, so a `:0` config becomes a
    /// real port a client can be told about.
    pub addr: SocketAddr,
    /// The per-boot bearer key.
    pub key: BootKey,
    event_bus: Arc<events::EventBus>,
    delivery: Arc<delivery::DeliveryService>,
    store_pools: [pij_store::StorePool; 2],
    shutdown: tokio::sync::oneshot::Sender<()>,
    joined: tokio::task::JoinHandle<()>,
    /// Owned so it cannot outlive the boot that started it: a worker still
    /// forwarding for a daemon that has stopped serving is a process nobody can
    /// see and nothing can stop.
    federation_worker: federation::FederationWorker,
    drain_loop: lifecycle::TickLoop,
    claude_bind_loop: lifecycle::TickLoop,
    background_loop: lifecycle::TickLoop,
    death_sweep_loop: lifecycle::TickLoop,
    pa_watchdog_loop: lifecycle::TickLoop,
    /// Owned so no tap outlives the daemon that attached it: `shutdown` joins an
    /// in-flight observation and DETACHES every pane. A tap left open is a
    /// `pipe-pane` writing into a sink nobody drains.
    pane_observer: pane_observer::PaneObserverLoop,
    /// Telegram / background / chore consumers. Owned so their claims stop with
    /// the daemon: a sidecar loop outliving its daemon would keep claiming queue
    /// rows a successor has already taken.
    sidecars: pij_sidecars::SidecarHandles,
    governance_shutdown: tokio::sync::oneshot::Sender<()>,
    governance_observer: tokio::task::JoinHandle<Result<()>>,
    /// One-shot warming of live seats' session cursors; aborted if still running.
    session_warmup: tokio::task::JoinHandle<()>,
    /// Sender notices for parked deliveries; aborted, then joined, at shutdown.
    park_notices: tokio::task::JoinHandle<()>,
}

impl Daemon {
    /// Persist and broadcast one event through this daemon's configured spine.
    ///
    /// # Errors
    /// Store failures, or an event that already carries a sequence.
    pub async fn publish_event(&self, event: Event) -> Result<Seq> {
        self.event_bus.publish(event).await
    }

    /// Ask the server to stop, and wait for it.
    ///
    /// # Errors
    /// [`PijError::Adapter`] when the server task panicked.
    pub async fn shutdown(self) -> Result<()> {
        // Signal HTTP and poll every worker shutdown together. Each worker's
        // shutdown future sends its stop before awaiting its join, so no slow
        // HTTP drain can leave queue workers claiming new rows in the meantime.
        let _ = self.shutdown.send(());
        let _ = self.governance_shutdown.send(());
        self.session_warmup.abort();
        self.park_notices.abort();
        // A cancelled notice task is the expected join outcome; any admitted
        // publication it started is drained by the event-bus flush below. A park
        // that commits after this abort is not lost: the next boot's sweep
        // notifies it (`park_notice::LOOKBACK_MS`), and a notice admitted twice
        // is impossible because its id is derived from the parked job.
        let _ = self.park_notices.await;
        let (
            served,
            drained,
            bound,
            observed,
            forwarded,
            background,
            deaths,
            watchdog,
            (),
            governance,
        ) = tokio::join!(
            async {
                self.joined.await.map_err(|error| PijError::Adapter {
                    adapter: "daemon".to_string(),
                    message: format!("the server task did not stop cleanly: {error}"),
                })
            },
            self.drain_loop.shutdown(),
            self.claude_bind_loop.shutdown(),
            self.pane_observer.shutdown(),
            self.federation_worker.shutdown(),
            self.background_loop.shutdown(),
            self.death_sweep_loop.shutdown(),
            self.pa_watchdog_loop.shutdown(),
            self.sidecars.shutdown(),
            async {
                self.governance_observer
                    .await
                    .map_err(|error| PijError::Adapter {
                        adapter: "daemon/governance".to_string(),
                        message: format!("delivery observer task failed: {error}"),
                    })?
            },
        );
        // Join work we own first. Both this join and physical pool cleanup share
        // one deadline: leaked leases must never turn stop into another wedge.
        let deadline = tokio::time::Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
        let admitted = tokio::time::timeout_at(deadline, async {
            let admitted = self.delivery.flush().await;
            self.event_bus.flush().await;
            admitted
        })
        .await;
        let (spine_closed, background_closed) = tokio::join!(
            pij_store::migrate::close(&self.store_pools[0], deadline),
            pij_store::migrate::close(&self.store_pools[1], deadline),
        );
        if admitted.is_err() || !spine_closed || !background_closed {
            // Role-labelled counts may refer to the same configured pool.
            let checked_out = |pool: &pij_store::StorePool| {
                u64::from(pool.size()).saturating_sub(pool.num_idle() as u64)
            };
            eprintln!(
                "pij-rs shutdown drain expired after {}ms; spine_checked_out={} background_checked_out={}; continuing shutdown",
                SHUTDOWN_DRAIN_TIMEOUT.as_millis(),
                checked_out(&self.store_pools[0]),
                checked_out(&self.store_pools[1]),
            );
        }
        if let Ok(admitted) = admitted {
            admitted?;
        }
        served?;
        drained?;
        bound?;
        observed?;
        governance?;
        background?;
        deaths?;
        watchdog?;
        forwarded
    }
}

impl std::fmt::Debug for Daemon {
    /// Hand-written because the worker handles are not `Debug` and a boot is a
    /// thing tests `expect_err` on. Prints what an operator would ask.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("addr", &self.addr)
            .field("key", &self.key.path)
            .finish_non_exhaustive()
    }
}

/// Build the services this config asks for.
///
/// **Fake is the default**, so a fresh checkout boots with no database, no tmux
/// and no network. A real adapter is opted into per port, which means a test can
/// make exactly one thing real.
///
/// # Errors
/// [`PijError`] from whichever real adapter refuses to start.
pub async fn build_services(config: &Config, pane_signal_dir: &Path) -> Result<Services> {
    if config.adapters.registry.is_real() && !config.adapters.spine.is_real() {
        return Err(PijError::Adapter {
            adapter: "daemon/config".to_string(),
            message: "E-RS-BACKEND-PAIR: Real Registry requires Real Spine for atomic one-writer ordering; select both real or use coherent in-memory persistence".to_string(),
        });
    }
    // ONE pool, shared by every real store adapter (lynx's cross-government
    // comparison, wave-3 compose fix). This opened the store three times: three
    // connection pools, three sets of connections, three boot self-migrations
    // racing each other on first run — for one file.
    //
    // The trait split is unchanged and is the point: Registry, Spine and Queue
    // stay three ports with three implementations. What they share is the
    // CONNECTION, which is a resource, not a contract.
    let persistent_pool = if needs_store(config) {
        Some(pij_store::open(&config.store_path).await?)
    } else {
        None
    };
    let background_pool = match &persistent_pool {
        Some(pool) => pool.clone(),
        None => pij_store::open("").await?,
    };
    let pool = |name: &str| -> Result<pij_store::StorePool> {
        persistent_pool.clone().ok_or_else(|| PijError::Adapter {
            adapter: format!("daemon/{name}"),
            message: "a real adapter was selected but no store was opened".to_string(),
        })
    };

    // Fake persistence is explicitly one in-memory SQL sequence domain, so
    // atomic governance callbacks and ordinary events replay the same history.
    // A real queue may retain its independently configured persistent pool.
    let spine_pool = if config.adapters.spine.is_real() {
        pool("spine")?
    } else {
        pij_store::open("").await?
    };
    let raw_spine: Arc<dyn Spine> = Arc::new(SqliteSpine::new(spine_pool.clone()));
    let event_bus = Arc::new(events::EventBus::new(
        raw_spine,
        config.event_buffer_capacity,
    )?);
    let registry: Arc<dyn Registry> = match config.adapters.registry {
        AdapterChoice::Fake => Arc::new(events::PublishedFakeRegistry::new(
            Arc::new(FakeRegistry::new()),
            event_bus.clone(),
        )),
        AdapterChoice::Real => Arc::new(SqliteRegistry::new(pool("registry")?, event_bus.clone())),
    };
    let spine: Arc<dyn Spine> = event_bus.clone();
    let queue: Arc<dyn Queue> = match config.adapters.queue {
        AdapterChoice::Fake => Arc::new(FakeQueue::new(config.delivered_id_capacity)?),
        AdapterChoice::Real => Arc::new(SqliteQueue::new(
            pool("queue")?,
            config.claim_lease_secs,
            config.delivered_id_capacity,
        )?),
    };

    // Every port is real-capable. The real socket transport remains closed for
    // unstamped seats, while the spawn path now truthfully stamps `Some(true)`
    // only when Claude's inbound-accept setting was emitted in its argv.
    let transport: Arc<dyn Transport> = match config.adapters.transport {
        AdapterChoice::Fake => Arc::new(FakeTransport::reachable()),
        AdapterChoice::Real => Arc::new(UdsTransport::new()?),
    };
    // Legacy and Rust daemons intentionally coexist during migration. Resolve the
    // legacy signature at the composition root and inject it; the tmux adapter
    // never reads process-global HOME on its own.
    let legacy_pane_signal_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".pij/pane-signals"));
    let tmux: Arc<dyn TmuxPort> = match config.adapters.tmux {
        AdapterChoice::Fake => Arc::new(FakeTmux::new()),
        AdapterChoice::Real => match legacy_pane_signal_dir {
            Some(legacy_root) => Arc::new(TmuxAdapter::with_legacy_tap_root(
                pane_signal_dir,
                legacy_root,
            )),
            None => Arc::new(TmuxAdapter::new(pane_signal_dir)),
        },
    };
    // One politeness gate, shared by direct delivery, queued retries, and the
    // pane observer that supplies its composer evidence.
    //
    // Plan 136: the gate and the register response answer with the SAME grace,
    // resolved once by `http::resolve_typing_grace_ms`. The idle window stays a
    // separate setting — an operator who chose `interaction_idle_ms` did not
    // choose this, and collapsing them would silently replace their number.
    let interaction = Arc::new(InteractionGate::with_typing_grace(
        Arc::clone(&tmux),
        Duration::from_millis(config.interaction_idle_ms),
        crate::http::resolve_typing_grace_ms(),
    ));
    // WIRED, wave 2 (u-harness). The refusal this replaces is gone.
    let harnesses = Arc::new(match config.adapters.harness {
        AdapterChoice::Fake => HarnessRegistry::new([
            Arc::new(FakeHarness::new(Harness::Claude)) as Arc<dyn HarnessPort>,
            Arc::new(FakeHarness::new(Harness::Copilot)),
            Arc::new(FakeHarness::new(Harness::Codex)),
            Arc::new(FakeHarness::new(Harness::Pi)),
            Arc::new(FakeHarness::new(Harness::Omp)),
        ])?,
        AdapterChoice::Real => HarnessRegistry::real(tmux.clone()),
    });
    // WIRED, wave 1 (u-liveness). The refusal this replaces is gone; the recipe
    // came from the unit's public doc comment, and the PM pastes it — one mind at
    // convergence, four worktrees at edit time.
    let liveness: Arc<dyn LivenessPort> = match config.adapters.liveness {
        AdapterChoice::Fake => Arc::new(FakeLiveness::new()),
        AdapterChoice::Real => Arc::new(ProcLiveness::new()),
    };
    // Stateful: the real source keeps one transcript read position per seat, so
    // it is composed once here and shared by every request.
    let session_status: Arc<dyn SessionStatusPort> = match config.adapters.session_status {
        AdapterChoice::Fake => Arc::new(FakeSessionStatus::new()),
        AdapterChoice::Real => Arc::new(pij_unisphere::UnisphereSessionStatus::with_roots(
            session_roots(),
        )),
    };

    let orchestration = Arc::new(OrchestrationService::new());

    let delivery = Arc::new(
        delivery::DeliveryService::new(
            Arc::clone(&registry),
            Arc::clone(&queue),
            Arc::clone(&transport),
            Arc::clone(&interaction),
            Arc::clone(&event_bus),
        )?
        .with_recovery_backends(config.adapters.queue, config.adapters.spine),
    );
    let roles = Arc::new(http::role::RoleService::new(
        Arc::clone(&registry),
        pij_store::SqliteOrchestration::new(spine_pool.clone()),
        Arc::clone(&event_bus),
    ));
    let decisions = Arc::new(http::decisions::DecisionService::new(
        pij_store::SqliteOrchestration::new(spine_pool.clone()),
        Arc::clone(&registry),
        Arc::clone(&event_bus),
        Arc::clone(&delivery),
        config.adapters.queue,
        config.adapters.spine,
    ));
    let anomalies = Arc::new(http::anomalies::AnomalyService::new(
        pij_store::SqliteOrchestration::new(spine_pool.clone()),
        Arc::clone(&registry),
        Arc::clone(&event_bus),
        Arc::clone(&liveness),
    ));
    let status = pij_store::status::SqliteStatus::new(
        spine_pool.clone(),
        config.adapters.registry.is_real() && config.adapters.spine.is_real(),
    );
    let store_pools = [spine_pool.clone(), background_pool.clone()];
    let bg_routing = Arc::new(bg_routing::DaemonColdRouting::new(
        Arc::clone(&registry),
        Arc::clone(&session_status),
        Arc::clone(&roles),
        pij_store::SqliteOrchestration::new(spine_pool.clone()),
        Arc::clone(&queue),
    ));
    let governance = Arc::new(http::governance::GovernanceService::new(
        pij_store::SqliteOrchestration::new(spine_pool),
        Arc::clone(&event_bus),
        pane_signal_dir.join("governance-packets"),
    ));
    let background = Arc::new(background::BackgroundService::new(
        pij_store::background::SqliteBackground::new(background_pool),
        background::BackgroundPorts {
            registry: Arc::clone(&registry),
            liveness: Arc::clone(&liveness),
            delivery: Arc::clone(&delivery),
            event_bus: Arc::clone(&event_bus),
            routing: bg_routing,
        },
        pane_signal_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("bg"),
        config.bind_addr.clone(),
    ));

    Ok(Services {
        registry,
        spine,
        event_bus,
        orchestration,
        queue,
        transport,
        tmux,
        interaction,
        harnesses,
        liveness,
        session_status,
        delivery,
        roles,
        governance,
        decisions,
        anomalies,
        background,
        status,
        store_pools,
        retired_harnesses: config.retired_harnesses.clone().into(),
        claude_homes: pij_harnesses::claude_homes().into(),
        offline: config.is_fully_offline(),
    })
}

/// Does this config select any adapter that needs the store open?
fn needs_store(config: &Config) -> bool {
    let a = &config.adapters;
    a.registry.is_real() || a.spine.is_real() || a.queue.is_real()
}

// `not_yet` is GONE. It was the refusal a `Real` adapter returned before its wave
// shipped — "this daemon will not pretend to have wired something it has not" —
// and with u-uds composed there is no port left that can be selected and not
// built. All seven are real-capable.
//
// The scaffold is deleted rather than kept "in case": a helper with no callers is
// a claim that some port might still be missing, and the next reader would have to
// check.

fn start_resilient_delivery_drain(
    drain: Arc<pointer::DrainWorker>,
    delivery: Arc<delivery::DeliveryService>,
    interval: lifecycle::TickInterval,
) -> lifecycle::TickLoop {
    let deadlines = Arc::clone(&delivery);
    lifecycle::TickLoop::start_validated_with_wake(
        interval,
        move || {
            let drain = Arc::clone(&drain);
            let delivery = Arc::clone(&delivery);
            async move {
                // SURVIVE errors, for the same reason the federation worker does
                // (review F2). TickLoop terminates on an Err, and a seat exiting
                // with queued mail is ROUTINE: one dead pane between the registry
                // read and the submit would have ended all below-socket delivery
                // for the daemon's lifetime — no pointers, no commands, no socket
                // bodies — silently, until someone restarted it.
                if let Err(error) = delivery.reconcile_native_receivers().await {
                    eprintln!("pij-rs native receiver reconciliation: {error}");
                }
                if let Err(error) = drain.drain_once().await {
                    eprintln!("pij-rs drain: {error}");
                }
                Ok(())
            }
        },
        move || {
            let delivery = Arc::clone(&deadlines);
            async move { delivery.wait_native_receiver_deadline().await }
        },
    )
}

fn start_resilient_claude_binder(
    binder: Arc<claude_bind::ClaudeBinder>,
    interval: lifecycle::TickInterval,
) -> lifecycle::TickLoop {
    lifecycle::TickLoop::start_validated(interval, move || {
        let binder = Arc::clone(&binder);
        async move {
            if let Err(error) = binder.bind_once().await {
                eprintln!("pij-rs claude binder: {error}");
            }
            Ok(())
        }
    })
}

fn write_runtime_record(state_dir: &Path, runtime: &DaemonRuntime) -> Result<PathBuf> {
    use std::io::Write;

    let bytes = serde_json::to_vec(runtime).map_err(|error| PijError::Adapter {
        adapter: "daemon/lifecycle".to_string(),
        message: format!("could not encode daemon runtime record: {error}"),
    })?;
    let path = state_dir.join(DAEMON_RUNTIME_FILE);
    let staged = state_dir.join(format!(
        ".{DAEMON_RUNTIME_FILE}.{}.{}.tmp",
        runtime.process.pid, runtime.process.proc_start
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/lifecycle".to_string(),
            message: format!("could not stage {}: {error}", staged.display()),
        })?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&staged);
        return Err(PijError::Adapter {
            adapter: "daemon/lifecycle".to_string(),
            message: format!("could not write {}: {error}", staged.display()),
        });
    }
    std::fs::rename(&staged, &path).map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        PijError::Adapter {
            adapter: "daemon/lifecycle".to_string(),
            message: format!("could not publish {}: {error}", path.display()),
        }
    })?;
    Ok(path)
}

/// Boot the daemon: stage the key, BIND, build, publish, serve.
///
/// The ORDER is the security property, and it has been sharpened twice.
///
/// Review's first pass: staging before the bind and publishing after means a boot
/// that loses the port race cannot overwrite the running daemon's credential,
/// while the key is still in place before any request is answered (`bind` opens
/// the socket; nothing is served until `axum::serve`). See [`auth`].
///
/// **The second: BIND BEFORE ANY MUTATING STEP** (lynx's row-80 trap, ruled a
/// boot invariant by the prime). `build_services` opens the store, and opening
/// the store RUNS MIGRATIONS — which used to happen before the bind. A second
/// daemon starting against the same store therefore migrated the schema and only
/// then discovered it had lost the port race. Migrations are one-way: a newer
/// binary losing the race would still have upgraded the schema underneath the
/// running older daemon, which had already passed its own version check.
///
/// The bind is the cheapest possible test of "am I allowed to exist", so it goes
/// first. Staging the key stays above it because a staged key is inert — a unique
/// 0600 name nothing reads until `publish`.
///
/// # Errors
/// [`PijError::Adapter`] when the key cannot be written or the port refuses.
pub async fn boot(config: &Config, state_dir: PathBuf) -> Result<Daemon> {
    let state_dir = std::path::absolute(state_dir).map_err(|error| PijError::Adapter {
        adapter: "daemon".into(),
        message: format!("could not resolve daemon state directory: {error}"),
    })?;
    // 1. Stage the key 0600 under a unique name. NOT published, and NOT a
    //    mutation of any shared resource: a boot that fails below must not touch
    //    a running daemon's credential.
    let staged = auth::stage_key(&state_dir)?;
    let token = staged.token().to_string();

    // 2. Bind. This is the step that fails when another daemon holds the port,
    //    and it comes before anything that changes state on disk.
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "daemon".to_string(),
            message: format!(
                "could not bind {} ({error}) — another daemon may already hold it",
                config.bind_addr
            ),
        })?;
    let addr = listener.local_addr().map_err(|error| PijError::Adapter {
        adapter: "daemon".to_string(),
        message: format!("could not resolve the bound address: {error}"),
    })?;

    // 3. This boot owns the port, so it may now touch the store. Migrations run
    //    here, exactly once, by the process that won.
    let pane_signal_dir = state_dir.join("pane-signals");
    let bound_config = Config {
        bind_addr: addr.to_string(),
        ..config.clone()
    };
    let services = build_services(&bound_config, &pane_signal_dir).await?;

    // 4. Resolve this machine's identity. FALLIBLE, and therefore BEFORE the
    //    publish: hostname lookup can fail and a configured alias can be empty.
    //    Review found this placed after the publish, which reopened the exact
    //    hole `auth` documents as closed — a boot that wins the bind and then
    //    fails identity resolution would exit having already overwritten a
    //    healthy daemon's credential. It depends on nothing from `services`, so
    //    there is no reason for it to be down here.
    let identity = lifecycle::MachineIdentity::resolve(config.machine_alias.as_deref())?;

    // 5. Federation and the drain worker. BOTH FALLIBLE, and therefore above the
    //    publish: peer-table validation rejects a duplicate or self-aliased
    //    alias, and DrainWorker rejects a zero cadence.
    //    A config error must never cost a running daemon its credential.
    let federation = Arc::new(
        federation::FederationService::new(
            identity.alias().to_string(),
            config.peers.clone(),
            Arc::clone(&services.queue),
            Arc::clone(&services.event_bus),
            federation::FederationPolicy {
                poll_interval: Duration::from_secs(config.federation_poll_interval_secs),
                max_retry_delay: Duration::from_secs(config.federation_retry_max_secs),
                event_buffer_capacity: config.event_buffer_capacity,
            },
        )
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/federation".to_string(),
            message: error.to_string(),
        })?,
    );

    // 6. The pane observer feeds the SAME `InteractionGate` used by first-attempt
    //    delivery and the drain worker. Unknown or active human input vetoes both
    //    paths; only two independently clear signals authorize injection.
    let pane_observer = Arc::new(pane_observer::PaneObserver::new(
        Arc::clone(&services.registry),
        Arc::clone(&services.tmux),
        Arc::clone(&services.interaction),
        pane_signal_dir,
        Duration::from_millis(config.pane_observer_interval_ms),
    )?);
    let claude_binder = Arc::new(claude_bind::ClaudeBinder::new(
        Arc::clone(&services.registry),
        Arc::clone(&services.liveness),
        Arc::clone(&services.event_bus),
        Arc::clone(&services.roles),
    ));

    let drain = Arc::new(
        pointer::DrainWorker::new(
            Arc::clone(&services.registry),
            Arc::clone(&services.queue),
            Arc::clone(&services.transport),
            Arc::clone(&services.tmux),
            Arc::clone(&services.interaction),
            Arc::clone(&services.event_bus),
            pointer::PointerPolicy {
                cadence: Duration::from_secs(config.pointer_announce_cadence_secs),
                announcement_limit: config.pointer_announce_limit,
            },
        )?
        .with_recovery_backends(config.adapters.queue, config.adapters.spine),
    );

    // 7. Validate the scheduler while refusal is still safe. Starting a worker
    //    here would be wrong: it could deliver or move queue rows for a daemon
    //    whose later key publication fails. Validation is fallible; start below
    //    the point of no return is deliberately infallible.
    let delivery_interval =
        lifecycle::TickInterval::new(Duration::from_secs(config.delivery_interval_secs))?;
    let background_interval = lifecycle::TickInterval::new(Duration::from_millis(100))?;
    let death_interval = death_sweep::interval()?;
    let pa_watchdog_secs = pa_watchdog::interval_secs(config.watchdog_interval_secs)?;
    let pa_watchdog_tick = lifecycle::TickInterval::new(pa_watchdog::TICK)?;

    // 8. This record can be true only after bind: it says this exact process won
    //    the port. It must also precede daemon.key, the point of no return. A
    //    bounce compares BOTH pid and proc-start before signalling, so stale pid
    //    reuse cannot kill an unrelated process.
    let pid = std::process::id();
    let proc_start =
        ProcLiveness::new()
            .proc_start(pid)
            .await?
            .ok_or_else(|| PijError::Adapter {
                adapter: "daemon/lifecycle".to_string(),
                message: format!("could not observe this daemon process at pid {pid}"),
            })?;
    let runtime = DaemonRuntime {
        process: ProcIdentity { pid, proc_start },
        addr,
        machine: identity.alias().to_string(),
        offline: config.is_fully_offline(),
    };
    let runtime_path = write_runtime_record(&state_dir, &runtime)?;

    // 8b. Sidecars: Telegram, background commands, chores. CONSTRUCTED HERE,
    //     above the point of no return, because construction is FALLIBLE — it
    //     opens state directories and validates a Telegram config, and a
    //     refusal must never cost a running daemon its published key.
    //
    //     TELEGRAM IS OPT-IN, BY EXPLICIT PATH ONLY. There is deliberately no
    //     `$HOME/.pij/telegram.env` default: that is the LEGACY bridge's
    //     credential file, and defaulting to it means every dev daemon on this
    //     machine tries to seize the production Telegram poll. Telegram allows
    //     exactly one `getUpdates` consumer per bot, so an accidental default
    //     does not fail loudly — it steals a live service from the person using
    //     it, or is stolen from.
    //
    //     A HELD LOCK IS NOT A BOOT FAILURE. The bridge is one optional
    //     consumer; messaging, delivery, and every seat-facing path work
    //     without it. Refusing to boot the whole daemon because another process
    //     holds a Telegram poll would take the system down over an accessory.
    //     We refuse the LOOP, loudly, and serve.
    let telegram_env = std::env::var_os("PIJ_TELEGRAM_ENV").map(PathBuf::from);
    let sidecars = match pij_sidecars::Sidecars::new(
        Arc::clone(&services.queue),
        Arc::clone(&services.spine),
        Arc::clone(&services.registry),
        state_dir.join("sidecars"),
        telegram_env.as_deref(),
    ) {
        Ok(sidecars) => sidecars,
        Err(error) => {
            eprintln!(
                "pij-rs: telegram sidecar not started ({error}); every other path is unaffected"
            );
            pij_sidecars::Sidecars::new(
                Arc::clone(&services.queue),
                Arc::clone(&services.spine),
                Arc::clone(&services.registry),
                state_dir.join("sidecars"),
                None,
            )?
        }
    };

    // 9. Publish the key atomically. This is the point of no return: every
    //    operation above may refuse without rotating a running credential, and
    //    every operation below is already validated and cannot refuse.
    let key = match staged.publish() {
        Ok(key) => key,
        Err(error) => {
            let _ = std::fs::remove_file(&runtime_path);
            return Err(error);
        }
    };

    let (governance_shutdown, governance_shutdown_rx) = tokio::sync::oneshot::channel();
    let governance = Arc::clone(&services.governance);
    let governance_observer = tokio::spawn(async move {
        tokio::select! {
            result = governance.follow_deliveries() => {
                if let Err(error) = &result {
                    eprintln!("pij-rs governance delivery observer stopped: {error}");
                }
                result
            }
            _ = governance_shutdown_rx => Ok(()),
        }
    });

    let pane_observer = pane_observer.start();
    let federation_worker = Arc::clone(&federation).start();
    let claude_bind_loop = start_resilient_claude_binder(claude_binder, delivery_interval);
    let drain_loop =
        start_resilient_delivery_drain(drain, Arc::clone(&services.delivery), delivery_interval);
    // Infallible by construction: every refusal happened in step 8b.
    let sidecars = sidecars.start()?;
    let background = Arc::clone(&services.background);
    let background_loop = lifecycle::TickLoop::start_validated(background_interval, move || {
        let background = Arc::clone(&background);
        async move {
            if let Err(error) = background.tick().await {
                eprintln!("pij-rs: background reconciliation failed: {error}");
            }
            Ok(())
        }
    });
    let death_sweep_loop = death_sweep::start(Arc::new(services.clone()), death_interval);
    let pa_watchdog_loop = pa_watchdog::start(
        Arc::new(services.clone()),
        pa_watchdog_secs,
        pa_watchdog_tick,
    );
    let park_notices = tokio::spawn(park_notice::follow(services.clone()));
    let session_warmup = session_warmup::start(
        Arc::clone(&services.registry),
        Arc::clone(&services.session_status),
    );

    let event_bus = Arc::clone(&services.event_bus);
    let delivery = Arc::clone(&services.delivery);
    let store_pools = services.store_pools.clone();
    let router = http::router_with_federation(
        services,
        http::HttpConfig {
            local_key: token,
            peer_keys: config.peers.iter().map(|peer| peer.key.clone()).collect(),
            machine_alias: identity.alias().to_string(),
        },
        federation,
    );
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let joined = tokio::spawn(async move {
        serve::serve(listener, router, serve::IDLE_CONNECTION_TIMEOUT, async {
            let _ = shutdown_rx.await;
        })
        .await;
    });

    Ok(Daemon {
        addr,
        key,
        event_bus,
        delivery,
        store_pools,
        shutdown,
        joined,
        federation_worker,
        drain_loop,
        claude_bind_loop,
        background_loop,
        death_sweep_loop,
        pa_watchdog_loop,
        pane_observer,
        sidecars,
        governance_shutdown,
        governance_observer,
        session_warmup,
        park_notices,
    })
}

/// Where every readable harness keeps its sessions on this machine: the one
/// definition `pij state` (the session-status port) and `pij fleet-report` share.
pub fn session_roots() -> pij_unisphere::SessionRoots {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    pij_unisphere::SessionRoots {
        claude_homes: pij_harnesses::claude_homes(),
        // OMP keeps every session under its agent dir, keyed by project.
        omp_sessions: home.as_ref().map(|home| home.join(".omp/agent/sessions")),
        codex_sessions: std::env::var_os("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".codex")))
            .map(|codex| codex.join("sessions")),
        copilot_sessions: home
            .as_ref()
            .map(|home| home.join(".copilot/session-state")),
    }
}

#[cfg(test)]
mod store_shutdown_tests;
