//! Authenticated daemon HTTP protocol primitives.
//!
//! Each route does one cheap statement or one durable enqueue (R4). Auth wraps
//! the completed router, so adding an endpoint cannot accidentally bypass it.

pub mod anomalies;
mod auth;
mod background;
mod client;
mod cold_wake;
pub mod decisions;
mod exposure;
mod fyi;
pub mod governance;
mod identity;
mod lifecycle;
mod report;
mod revive;
pub mod role;
mod shim;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INBOX_ACK_EVENT_KIND: &str = "delivery.inbox-ack";
const REVIVE_POSTMORTEM_EVENT_KIND: &str = "seat.revive-postmortem";
const DEFAULT_SPAWN_WAIT_SECONDS: u64 = 30;
const SPAWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Extension, Json, Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use pij_core::delivery::{DEFAULT_TYPING_GRACE_MS, HeldEvent, ReleasedEvent};
use pij_core::error::{PijError, Result};
use pij_core::events::EventFilter;
use pij_core::model::{
    DeliveryOutcome, Envelope, ErrorKind, Event, Harness, Msg, ProcIdentity, SeatDescriptor,
    SeatId, SemanticState, Seq,
};
use pij_core::names::memorable_pij_id_candidates;
use pij_core::ports::Registry;
use pij_core::wire;
use pij_harnesses::{SpawnPlanInput, build_spawn_plan, observed_launch_command};
use serde::{Deserialize, Serialize};
use tokio_stream::{Stream, StreamExt};

use crate::federation::{FederationSendError, FederationService};
use crate::registration::{RegistrationError, RegistrationService, requested_model_matches};
use crate::{BUILD, Services};
pub use auth::AuthRing;
pub use client::{PeerEndpoint, get_from_peer, post_to_peer, stream_from_peer};
pub use exposure::{Exposure, boot_banner, exposure};
pub use identity::{CallerContext, IdentityQuery, IdentityRequest, Phonehome};
pub use types::{
    CursorResetDetail, FederatedRoster, InboxAckRequest, InboxHeartbeatRequest, PeerStreamState,
    Registration, ReviveRequest, SendRequest, SpawnRequest, SpawnResponse, StateCard, StateRequest,
    StreamFrame, UnavailablePeer, UnsupportedField,
};

#[derive(Clone)]
pub(crate) struct AppState {
    services: Services,
    auth: AuthRing,
    machine_alias: String,
    spawn_lock: Arc<tokio::sync::Mutex<()>>,
    typing_lock: Arc<tokio::sync::Mutex<()>>,
    typing_grace_ms: u64,
    federation: Option<Arc<FederationService>>,
    registration: RegistrationService,
}

/// HTTP configuration supplied by the daemon composition root.
#[derive(Clone)]
pub struct HttpConfig {
    /// Per-boot credential used by clients on this machine.
    pub local_key: String,
    /// Persistent credentials for manually bootstrapped peer machines.
    pub peer_keys: Vec<String>,
    /// This machine's configured alias, defaulted from hostname by lifecycle.
    pub machine_alias: String,
}

impl HttpConfig {
    /// Local-only compatibility config used until lifecycle composition lands.
    pub fn local(local_key: String) -> Self {
        Self {
            local_key,
            peer_keys: Vec::new(),
            machine_alias: "local".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Endpoint {
    Health,
    Register,
    Spawn,
    Revive,
    Send,
    Background,
    BackgroundTail,
    BackgroundKill,
    Inbox,
    InboxTyping,
    InboxAck,
    InboxHeartbeat,
    InboxRelease,
    Hold,
    Release,
    Seats,
    Events,
    /// Producer for the three sidecar consumers. The consumers are queue-backed,
    /// so a sidecar job must be ENQUEUED by something; without this route the
    /// only producer would be hand-written SQL, which proves nothing about the
    /// shipped surface.
    Sidecar,
    /// The whole `report` family, on one path.
    ///
    /// One endpoint rather than six because the shim routes by VERB and hands
    /// the daemon the caller's `argv` untouched; the leaf is the second token of
    /// that argv, not a second path.
    Report,
    /// The shim-originated `send` and `inbox`, on their own versioned paths
    /// (plan 119). Separate from [`Self::Send`] / [`Self::Inbox`] on purpose —
    /// those speak the `pij-rs` CLI's language (a caller-supplied `from` and
    /// `seat`), these speak the shim's (`{argv, caller}`) and DERIVE the acting
    /// seat. `shim.rs` carries the reason the two are not one route.
    ShimSend,
    /// Self-compaction through the same control admission as shim send.
    ShimCompactSelf,
    /// See [`Self::ShimSend`].
    ShimInbox,
    /// The acknowledgement half of a routed inbox read. See
    /// [`shim::shim_inbox_ack`] for why it is not `/v1/inbox/ack`.
    ShimInboxAck,
    /// Read rs seats in the frozen legacy session-row shape (plan 128).
    ShimSessions,
    /// Register this seat, with rs minting the name (plan 114, ac-1147).
    Adopt,
    /// Name the calling seat from the store it registered into (ac-1142).
    Whoami,
    /// Confirm the binding adopt already made (ac-1148).
    Phonehome,
    /// `pij state <id>` — read one seat's card back (plan 114, u-readback).
    ///
    /// POST rather than GET even though it reads nothing: wave 1's routing shim
    /// forwards the operator's argv ONLY on its POST branch
    /// (`.pi/extensions/pij/adapters/generation-router.ts:199-207`), so a GET row
    /// would reach this daemon as a bare URL with the `<id>` — this verb's only
    /// argument — dropped at the seam. Ruled POST-canonical by the plan-114 PM.
    State,
    /// Atomic claim of held FYIs by a typed-turn hook (plan 158).
    FyiClaim,
    /// Read one delivered FYI batch in full, as a digest names it (plan 159).
    FyiRead,
    /// Busy/idle publication from a harness's own turn events (plan 158, addendum 3).
    Activity,
    Role,
    Close,
    Reap,
    Anomalies,
    Decisions,
    Answer,
    Project,
    Stream,
    Fence,
    Dispatch,
    Ack,
    Canary,
    Attest,
    Task,
    Node,
    Orchestration,
    Spine,
}

impl Endpoint {
    const ALL: [Self; 48] = [
        Self::Health,
        Self::Register,
        Self::Spawn,
        Self::Revive,
        Self::Send,
        Self::Background,
        Self::BackgroundTail,
        Self::BackgroundKill,
        Self::Inbox,
        Self::InboxTyping,
        Self::InboxAck,
        Self::InboxHeartbeat,
        Self::InboxRelease,
        Self::Hold,
        Self::Release,
        Self::Seats,
        Self::Events,
        Self::Sidecar,
        Self::Report,
        Self::Adopt,
        Self::Whoami,
        Self::Phonehome,
        Self::State,
        Self::FyiClaim,
        Self::FyiRead,
        Self::Activity,
        Self::ShimSend,
        Self::ShimCompactSelf,
        Self::ShimInbox,
        Self::ShimInboxAck,
        Self::ShimSessions,
        Self::Role,
        Self::Close,
        Self::Reap,
        Self::Anomalies,
        Self::Decisions,
        Self::Answer,
        Self::Project,
        Self::Stream,
        Self::Fence,
        Self::Dispatch,
        Self::Ack,
        Self::Canary,
        Self::Attest,
        Self::Task,
        Self::Node,
        Self::Orchestration,
        Self::Spine,
    ];

    const fn path(self) -> &'static str {
        match self {
            Self::Health => "/health",
            Self::Register => "/v1/register",
            Self::Spawn => "/v1/spawn",
            Self::Revive => "/v1/revive",
            Self::Send => "/v1/send",
            Self::Background => "/v1/bg",
            Self::BackgroundTail => "/v1/bg/{job}/tail",
            Self::BackgroundKill => "/v1/bg/{job}/kill",
            Self::Inbox => "/v1/inbox",
            Self::InboxTyping => "/v1/inbox/typing",
            Self::InboxAck => "/v1/inbox/ack",
            Self::InboxHeartbeat => "/v1/inbox/heartbeat",
            Self::InboxRelease => "/v1/inbox/release",
            Self::Hold => "/v1/hold",
            Self::Release => "/v1/release",
            Self::Seats => "/v1/seats",
            Self::Events => "/v1/events",
            Self::Sidecar => "/v1/sidecar",
            Self::Report => "/v1/report",
            Self::Adopt => "/v1/adopt",
            Self::Whoami => "/v1/whoami",
            Self::Phonehome => "/v1/phonehome",
            Self::State => "/v1/state",
            Self::FyiClaim => "/v1/fyi/claim",
            Self::FyiRead => "/v1/fyi/read",
            Self::Activity => "/v1/activity",
            Self::Role => "/v1/role",
            Self::Close => "/v1/close",
            Self::Reap => "/v1/reap",
            Self::Anomalies => "/v1/anomalies",
            Self::Decisions => "/v1/decisions",
            Self::Answer => "/v1/answer",
            Self::Project => "/v1/project",
            Self::Stream => "/v1/stream",
            Self::Fence => "/v1/fence",
            Self::Dispatch => "/v1/dispatch",
            Self::Ack => "/v1/ack",
            Self::Canary => "/v1/canary",
            Self::Attest => "/v1/attest",
            Self::Task => "/v1/task",
            Self::Node => "/v1/node",
            Self::Orchestration => "/v1/orchestration",
            Self::Spine => "/v1/spine",
            // PLAN 119 — a DISTINCT, VERSIONED path for shim-originated calls, never
            // an overload of `/v1/send` / `/v1/inbox`. Those structs tolerate
            // unknown fields, so the additive shape would have an older daemon
            // accept an asserted `from` and silently discard the caller evidence
            // meant to constrain it. See `shim.rs`.
            Self::ShimSend => "/v1/shim/send",
            Self::ShimCompactSelf => "/v1/shim/compact-self",
            Self::ShimInbox => "/v1/shim/inbox",
            Self::ShimInboxAck => "/v1/shim/inbox/ack",
            Self::ShimSessions => "/v1/shim/sessions",
        }
    }

    const fn method(self) -> Method {
        match self {
            Self::Register
            | Self::Spawn
            | Self::Revive
            | Self::Send
            | Self::Background
            | Self::BackgroundKill
            | Self::InboxAck
            | Self::InboxHeartbeat
            | Self::InboxRelease
            | Self::Hold
            | Self::Release
            | Self::Sidecar
            | Self::Report => Method::POST,
            Self::Adopt | Self::Whoami | Self::Phonehome => Method::POST,
            Self::State | Self::FyiClaim | Self::FyiRead | Self::Activity => Method::POST,
            Self::Role
            | Self::Close
            | Self::Reap
            | Self::Anomalies
            | Self::Decisions
            | Self::Answer
            | Self::Project
            | Self::Stream
            | Self::Fence
            | Self::Dispatch
            | Self::Ack
            | Self::Canary
            | Self::Attest
            | Self::Task
            | Self::Node
            | Self::Orchestration
            | Self::Spine => Method::POST,
            Self::ShimSend | Self::ShimCompactSelf | Self::ShimInbox | Self::ShimInboxAck => {
                Method::POST
            }
            Self::Health
            | Self::Inbox
            | Self::BackgroundTail
            | Self::InboxTyping
            | Self::Seats
            | Self::Events
            | Self::ShimSessions => Method::GET,
        }
    }
}

/// Build the local-only router used by the current composition root.
pub fn router(services: Services, token: String) -> Router {
    router_with_config(services, HttpConfig::local(token))
}

/// Build all daemon routes under one authentication ring.
///
/// # Composition recipe
///
/// In `crates/daemon/src/lib.rs`, import
/// `crate::http::{HttpConfig, PeerEndpoint, boot_banner, router_with_config}`.
/// After binding, print `boot_banner(&addr)`. Build `HttpConfig` from the local
/// per-boot token, persistent configured peer keys, and lifecycle's hostname-
/// defaulted machine alias, then call `router_with_config(services, http_config)`.
/// The wave-4 federation worker owns a `BTreeMap<String, PeerEndpoint>` plus one
/// `reqwest::Client`; it resolves a machine alias and calls `post_to_peer` once.
/// Removing a configured peer key from `HttpConfig::peer_keys` revokes inbound
/// access for that machine.
///
/// Plan 136's routes are already composed here under the same bearer ring:
/// ```text
/// Endpoint::Hold => router.route(endpoint.path(), post(hold_inbox)),
/// Endpoint::Release => router.route(endpoint.path(), post(release_inbox)),
/// Envelope::ok("pij register", RegistrationResponse {
///     descriptor, binding, proc_source, typing_grace_ms: Some(state.typing_grace_ms),
/// })
/// ```
/// The outer daemon composition root needs no extra queue or worker: keep its
/// existing call to this router and its existing pointer drain cadence. Omp/Pi
/// are excluded by that worker's eligibility predicate, not by another timer.
pub fn router_with_config(services: Services, config: HttpConfig) -> Router {
    router_with_optional_federation(services, config, None)
}

/// Build daemon routes with remote-send admission enabled.
pub fn router_with_federation(
    services: Services,
    config: HttpConfig,
    federation: Arc<FederationService>,
) -> Router {
    router_with_optional_federation(services, config, Some(federation))
}

/// The typing grace this daemon serves, resolved ONCE per process.
///
/// Composition (plan 136): the register response and the interaction gate must
/// answer with the same number, or the extension holds for one window while the
/// send-keys gate defers for another and neither is wrong on its own. The
/// constant lives in `pij_core::delivery`; this function is the single place the
/// env override is read.
#[must_use]
pub fn resolve_typing_grace_ms() -> u64 {
    std::env::var("PIJ_TYPING_GRACE_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TYPING_GRACE_MS)
}

fn router_with_optional_federation(
    services: Services,
    config: HttpConfig,
    federation: Option<Arc<FederationService>>,
) -> Router {
    let registration = RegistrationService::new(
        Arc::clone(&services.registry),
        Arc::clone(&services.liveness),
        Arc::clone(&services.event_bus),
        pij_harnesses::claude_homes(),
        Arc::clone(&services.roles),
    )
    .with_native_lock(services.delivery.native_lock());
    let state = AppState {
        services,
        auth: AuthRing::new(config.local_key, config.peer_keys),
        machine_alias: config.machine_alias,
        spawn_lock: Arc::new(tokio::sync::Mutex::new(())),
        typing_lock: Arc::new(tokio::sync::Mutex::new(())),
        typing_grace_ms: resolve_typing_grace_ms(),
        federation,
        registration,
    };
    let mut router = Router::new();
    for endpoint in Endpoint::ALL {
        debug_assert_eq!(
            endpoint.method(),
            if matches!(
                endpoint,
                Endpoint::Register
                    | Endpoint::Spawn
                    | Endpoint::Revive
                    | Endpoint::Send
                    | Endpoint::Background
                    | Endpoint::BackgroundKill
                    | Endpoint::InboxAck
                    | Endpoint::InboxHeartbeat
                    | Endpoint::InboxRelease
                    | Endpoint::Hold
                    | Endpoint::Release
                    | Endpoint::Sidecar
                    | Endpoint::Report
                    | Endpoint::Adopt
                    | Endpoint::Whoami
                    | Endpoint::Phonehome
                    | Endpoint::State
                    | Endpoint::FyiClaim
                    | Endpoint::FyiRead
                    | Endpoint::Activity
                    | Endpoint::ShimSend
                    | Endpoint::ShimCompactSelf
                    | Endpoint::ShimInbox
                    | Endpoint::ShimInboxAck
                    | Endpoint::Role
                    | Endpoint::Close
                    | Endpoint::Reap
                    | Endpoint::Anomalies
                    | Endpoint::Decisions
                    | Endpoint::Answer
                    | Endpoint::Project
                    | Endpoint::Stream
                    | Endpoint::Fence
                    | Endpoint::Dispatch
                    | Endpoint::Ack
                    | Endpoint::Canary
                    | Endpoint::Attest
                    | Endpoint::Task
                    | Endpoint::Node
                    | Endpoint::Orchestration
                    | Endpoint::Spine
            ) {
                Method::POST
            } else {
                Method::GET
            }
        );
        router = match endpoint {
            Endpoint::Health => router.route(endpoint.path(), get(health)),
            Endpoint::Register => router.route(endpoint.path(), post(register)),
            Endpoint::Spawn => router.route(endpoint.path(), post(spawn_seat)),
            Endpoint::Revive => router.route(endpoint.path(), post(revive::revive_seat)),
            Endpoint::Send => router.route(endpoint.path(), post(send)),
            Endpoint::Background => router.route(
                endpoint.path(),
                post(background::post).get(background::list),
            ),
            Endpoint::BackgroundTail => router.route(endpoint.path(), get(background::tail)),
            Endpoint::BackgroundKill => router.route(endpoint.path(), post(background::kill)),
            Endpoint::Inbox => router.route(endpoint.path(), get(inbox)),
            Endpoint::InboxTyping => router.route(endpoint.path(), get(inbox_typing)),
            Endpoint::InboxAck => router.route(endpoint.path(), post(ack_inbox)),
            Endpoint::InboxHeartbeat => router.route(endpoint.path(), post(heartbeat_inbox)),
            Endpoint::InboxRelease => router.route(endpoint.path(), post(operator_release_inbox)),
            Endpoint::Hold => router.route(endpoint.path(), post(hold_inbox)),
            Endpoint::Release => router.route(endpoint.path(), post(release_inbox)),
            Endpoint::Seats => router.route(endpoint.path(), get(seats).post(seats_post)),
            Endpoint::Events => router.route(endpoint.path(), get(events)),
            Endpoint::Sidecar => router.route(endpoint.path(), post(sidecar)),
            Endpoint::Report => router.route(endpoint.path(), post(report)),
            Endpoint::Adopt => router.route(endpoint.path(), post(identity::adopt)),
            // BOTH methods on purpose. The route table flips `whoami` to POST so
            // it can carry caller context, but a shim that has not been updated
            // still sends GET — and axum would answer that `405` with an EMPTY
            // body, which the shim reads as route-absence and falls back to
            // LEGACY. Serving both turns a silent re-homing into an answer.
            Endpoint::Whoami => router.route(
                endpoint.path(),
                get(identity::whoami_get).post(identity::whoami_post),
            ),
            Endpoint::Phonehome => router.route(endpoint.path(), post(identity::phonehome)),
            Endpoint::State => router.route(endpoint.path(), post(seat_state)),
            Endpoint::FyiClaim => router.route(endpoint.path(), post(fyi::claim)),
            Endpoint::FyiRead => router.route(endpoint.path(), post(fyi::read)),
            Endpoint::Activity => router.route(endpoint.path(), post(fyi::activity)),
            Endpoint::Role => router.route(endpoint.path(), post(role::role)),
            Endpoint::Close => router.route(endpoint.path(), post(lifecycle::close)),
            Endpoint::Reap => router.route(endpoint.path(), post(lifecycle::reap)),
            Endpoint::Anomalies => router.route(
                endpoint.path(),
                get(anomalies::list_get).post(anomalies::list_post),
            ),
            Endpoint::Decisions => router.route(
                endpoint.path(),
                get(decisions::list_get).post(decisions::list_post),
            ),
            Endpoint::Answer => router.route(endpoint.path(), post(decisions::answer_post)),
            Endpoint::Project => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Stream => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Fence => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Dispatch => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Ack => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Canary => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Attest => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Task => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Node => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Orchestration => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::Spine => router.route(endpoint.path(), post(governance::handle)),
            Endpoint::ShimSend => router.route(endpoint.path(), post(shim::shim_send)),
            Endpoint::ShimCompactSelf => router.route(endpoint.path(), post(shim::shim_send)),
            Endpoint::ShimInbox => router.route(endpoint.path(), post(shim::shim_inbox)),
            Endpoint::ShimInboxAck => router.route(endpoint.path(), post(shim::shim_inbox_ack)),
            Endpoint::ShimSessions => router.route(endpoint.path(), get(shim::shim_sessions)),
        };
    }
    router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        // Added after the layer on purpose: the event hook's credential is its
        // own per-job token (Plan 163), never the daemon key.
        .route(background::EMIT_PATH, post(background::emit))
        .with_state(state)
}

async fn require_bearer(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if let Some(identity) = state.auth.authenticate(presented) {
        request.extensions_mut().insert(identity);
        return next.run(request).await;
    }
    envelope(
        StatusCode::UNAUTHORIZED,
        &Envelope::<()>::refused(
            "auth",
            ErrorKind::Auth,
            "missing or wrong bearer token — local clients read <state-dir>/daemon.key and send `Authorization: Bearer <key>`; peer machines use their manually bootstrapped configured key",
        ),
    )
}

#[derive(serde::Serialize)]
struct Health<'a> {
    status: &'static str,
    build: &'static str,
    offline: bool,
    machine: String,
    retired_harnesses: &'a [Harness],
    store: StoreHealth,
}

#[derive(serde::Serialize)]
struct StoreHealth {
    status: &'static str,
    timeout_ms: u64,
}

async fn health(State(state): State<AppState>) -> Response {
    let checked = state.services.status.check_health().await;
    let healthy = checked.is_ok();
    let data = Health {
        status: if healthy { "healthy" } else { "unhealthy" },
        build: BUILD,
        offline: state.services.offline,
        machine: state.machine_alias,
        retired_harnesses: &state.services.retired_harnesses,
        store: StoreHealth {
            status: if healthy { "healthy" } else { "unavailable" },
            timeout_ms: pij_store::migrate::STORE_WAIT_TIMEOUT.as_millis() as u64,
        },
    };
    match checked {
        Ok(()) => envelope(StatusCode::OK, &Envelope::ok("pij ping", data)),
        Err(error) => {
            let mut response = Envelope::refused("pij ping", ErrorKind::Adapter, error.to_string());
            response.data = Some(data);
            envelope(StatusCode::SERVICE_UNAVAILABLE, &response)
        }
    }
}

#[derive(Deserialize)]
struct RegistrationRequest {
    #[serde(flatten)]
    claim: Registration,
    #[serde(
        default,
        rename = "HARNESS_SESSION_ID",
        alias = "harness_session",
        alias = "harnessSession"
    )]
    harness_session: Option<String>,
    #[serde(default)]
    native_extension_delivery: bool,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum RegistrationBinding {
    Created,
    Rebound,
    Same,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ProcSource {
    Harness,
    Pane,
}

struct PaneHarnessIdentity {
    proc: ProcIdentity,
    source: ProcSource,
    subtree: Vec<ProcIdentity>,
}

/// Daemon-derived selection and subtree evidence, never caller assertions.
/// Only register may select a claimed member; adopt does not carry a claim here.
async fn pane_harness_identity(
    state: &AppState,
    pane_pid: u32,
    harness: Harness,
) -> Result<Option<PaneHarnessIdentity>> {
    let candidate =
        tokio::task::spawn_blocking(move || pij_tmux::harness_process_tree(pane_pid, harness))
            .await
            .map_err(|error| PijError::Adapter {
                adapter: "harness-process".to_string(),
                message: error.to_string(),
            })?;
    if let Some(observed) = candidate
        && state
            .services
            .liveness
            .proc_start(observed.identity.pid)
            .await?
            == Some(observed.identity.proc_start)
    {
        let source = if observed.identity.pid == pane_pid {
            ProcSource::Pane
        } else {
            ProcSource::Harness
        };
        return Ok(Some(PaneHarnessIdentity {
            proc: observed.identity,
            source,
            subtree: observed.subtree,
        }));
    }
    Ok(state
        .services
        .liveness
        .proc_start(pane_pid)
        .await?
        .map(|proc_start| {
            let proc = ProcIdentity {
                pid: pane_pid,
                proc_start,
            };
            PaneHarnessIdentity {
                proc,
                source: ProcSource::Pane,
                subtree: vec![proc],
            }
        }))
}

/// Registration-only metadata; it never becomes durable descriptor state.
#[derive(Deserialize, Serialize)]
struct RegistrationResponse {
    #[serde(flatten)]
    descriptor: SeatDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<RegistrationBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proc_source: Option<ProcSource>,
    /// The daemon-owned quiet interval after a human edit, in milliseconds
    /// (plan 136). The extension reads it here rather than restating 60000:
    /// one definition (`pij_core::delivery::DEFAULT_TYPING_GRACE_MS`), one
    /// resolver (`resolve_typing_grace_ms`), one answer.
    ///
    /// OPTIONAL ON THE WIRE, and deliberately so: this daemon always sends it,
    /// but a response captured before plan 136 has no such key, and a required
    /// field would make every old fixture fail to parse while a `default` would
    /// silently mint a grace of ZERO — a number nobody chose, meaning "never
    /// hold". Absent stays absent, and the extension's own fallback applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    typing_grace_ms: Option<u64>,
}

/// Composition: the service returns `(descriptor, report)` from its committed
/// `put_reporting`; derive `created` on insert, `rebound` on a changed process,
/// otherwise `same`, and flatten the descriptor into this response only.
async fn register(
    State(state): State<AppState>,
    Json(mut request): Json<RegistrationRequest>,
) -> Response {
    // Native claims carry their own verified host tuple. Do not replace it with
    // the pane's shell PID while adding the ordinary registration provenance.
    let mut proc_source = request
        .native_extension_delivery
        .then_some(ProcSource::Harness);
    if !request.native_extension_delivery
        && let (Some(pane), Some(harness)) = (
            request.claim.pane.as_deref(),
            Harness::parse(&request.claim.harness),
        )
    {
        let observed = match state.services.tmux.pane_process(pane).await {
            Ok(observed) => observed,
            Err(error) => return internal("pij register", error),
        };
        if let Some(observed) = observed {
            match pane_harness_identity(&state, observed.pid, harness).await {
                Ok(Some(observed)) => {
                    if matches!(observed.source, ProcSource::Pane) && request.claim.pid.is_some() {
                        // The claim selects daemon-observed evidence; liveness
                        // alone cannot authorize an unrelated process in a pane.
                        let claim = request
                            .claim
                            .pid
                            .zip(request.claim.proc_start)
                            .map(|(pid, proc_start)| ProcIdentity { pid, proc_start });
                        if !claim.is_some_and(|claim| observed.subtree.contains(&claim)) {
                            return refused(
                                "pij register",
                                "claimed pid/proc_start is not in the daemon-observed pane process subtree; nothing was written",
                            );
                        }
                    } else {
                        request.claim.pid = Some(observed.proc.pid);
                        request.claim.proc_start = Some(observed.proc.proc_start);
                    }
                    proc_source = Some(observed.source);
                }
                Ok(None) => {
                    return refused(
                        "pij register",
                        "pane process disappeared before registration; nothing was written",
                    );
                }
                Err(error) => return internal("pij register", error),
            }
        }
    }
    let registration = &state.registration;
    let registered = if request.native_extension_delivery {
        registration
            .register_native(
                request.claim,
                request.harness_session,
                state.services.tmux.as_ref(),
            )
            .await
    } else {
        registration
            .register_with_harness_session(request.claim, request.harness_session)
            .await
    };
    match registered {
        Ok((descriptor, report)) => {
            if request.native_extension_delivery {
                let identity = crate::delivery::NativeInboxIdentity {
                    native_session: descriptor.harness_session.clone(),
                    pid: descriptor.proc.map(|proc| proc.pid),
                    proc_start: descriptor.proc.map(|proc| proc.proc_start),
                };
                if let Err(error) = state
                    .services
                    .delivery
                    .attest_native_receiver(&descriptor.id, &identity)
                    .await
                {
                    return internal("pij register", error);
                }
            }
            let meta = descriptor.pane.is_none().then(|| {
                format!(
                    "verified external shim callers resolve this paneless seat from their native host/session automatically; other clients must pass id {0} explicitly or export PIJ_SESSION_ID={0}",
                    descriptor.id
                )
            });
            let binding = if report.inserted {
                RegistrationBinding::Created
            } else if report.previous_proc != descriptor.proc {
                RegistrationBinding::Rebound
            } else {
                RegistrationBinding::Same
            };
            // Convergence 135 + 136: ONE response type carries every additive
            // field. 135 added binding/proc_source here and 136 added
            // typing_grace_ms in `wire`; two structs serializing the same
            // response is how two answers to one question start drifting.
            let mut answer = Envelope::ok(
                "pij register",
                RegistrationResponse {
                    descriptor,
                    binding: Some(binding),
                    proc_source,
                    typing_grace_ms: Some(state.typing_grace_ms),
                },
            );
            answer.meta = meta;
            envelope(StatusCode::OK, &answer)
        }

        Err(RegistrationError::Refused(reason)) => refused("pij register", reason),
        Err(RegistrationError::Retryable(reason)) => {
            let mut answer: Envelope<()> =
                Envelope::refused("pij register", ErrorKind::Refused, reason);
            answer.details = Some(serde_json::json!({ "retryable": true }));
            envelope(StatusCode::CONFLICT, &answer)
        }
        Err(error @ RegistrationError::NativeSessionHold(_)) => {
            let mut answer: Envelope<()> =
                Envelope::refused("pij register", ErrorKind::Refused, error.to_string());
            answer.details =
                Some(serde_json::json!({ "retryable": true, "hold": "native-session" }));
            envelope(StatusCode::CONFLICT, &answer)
        }
        Err(RegistrationError::Runtime(error)) => internal("pij register", error),
    }
}

/// The first memorable name this roster does not already hold.
///
/// Extracted from the route so it can be TESTED (review round 2 found the loop had
/// no coverage at all): the candidate order is deterministic per seed, so a test
/// pre-registers the first candidate and asserts allocation advances — which the
/// inline version could not express, because the seed was minted inside the route.
///
/// `Ok(None)` means the space is exhausted, which is a refusal rather than an
/// error: nothing failed, there is simply no name left.
async fn allocate_memorable_id(registry: &dyn Registry, seed: &str) -> Result<Option<SeatId>> {
    for candidate in memorable_pij_id_candidates(seed) {
        if registry.get(&candidate).await?.is_none() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// Read and publish the tombstone that a revive is about to replace. Reading
/// registry truth here makes the ordering load-bearing: moving this call after
/// the overwrite fails because the post-mortem no longer exists.
async fn publish_revive_postmortem(state: &AppState, seat_id: &SeatId) -> Result<()> {
    let previous =
        state
            .services
            .registry
            .get(seat_id)
            .await?
            .ok_or_else(|| PijError::Adapter {
                adapter: "spawn".to_string(),
                message: format!("cannot preserve revive history for absent seat `{seat_id}`"),
            })?;
    if previous.tombstoned_at.is_none() {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!("cannot preserve revive history for live seat `{seat_id}`"),
        });
    }
    let payload = serde_json::json!({
        "tombstoned_at": previous.tombstoned_at,
        "reason": previous.tombstone_reason,
        "role": previous.role,
        "semantic_state": previous.semantic_state,
    })
    .to_string();
    state
        .services
        .event_bus
        .publish(Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at: system_time_ms()?,
            kind: REVIVE_POSTMORTEM_EVENT_KIND.to_string(),
            seat: Some(previous.id),
            payload,
        })
        .await?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) struct ExistingSeatPolicy {
    pub(crate) allow_absent: bool,
}

async fn spawn_seat(State(state): State<AppState>, Json(request): Json<SpawnRequest>) -> Response {
    if state.services.retired_harnesses.contains(&request.harness) && !request.allow_retired {
        let alternative = if request.harness == Harness::Omp {
            "use another harness"
        } else {
            "use omp"
        };
        return refused(
            "pij spawn",
            format!(
                "harness {} is retired on this machine; {alternative} (or pass --allow-retired)",
                request.harness
            ),
        );
    }
    launch_seat(
        &state,
        request,
        ExistingSeatPolicy { allow_absent: true },
        "pij spawn",
    )
    .await
}

/// Launch one seat under the spawn lock. Spawn and revive routes share this
/// transition so allocation, argv-derived stamps, rollback, and persistence
/// have exactly one owner.
pub(crate) async fn launch_seat(
    state: &AppState,
    request: SpawnRequest,
    existing_policy: ExistingSeatPolicy,
    command_name: &'static str,
) -> Response {
    let spawn_guard = state.spawn_lock.lock().await;
    launch_seat_locked(
        state,
        request,
        existing_policy,
        command_name,
        spawn_guard,
        None,
    )
    .await
}

/// The caller retains the same guard from revive observation through launch.
async fn launch_seat_locked(
    state: &AppState,
    request: SpawnRequest,
    existing_policy: ExistingSeatPolicy,
    command_name: &'static str,
    spawn_guard: tokio::sync::MutexGuard<'_, ()>,
    revive_receipt: Option<revive::Receipt>,
) -> Response {
    if let Some(id) = request.id.as_ref()
        && id.as_str().trim().is_empty()
    {
        return refused(command_name, "--id must be non-empty when supplied");
    }
    if !Path::new(&request.cwd).is_absolute() {
        return refused(
            command_name,
            "--cwd must be absolute so the descriptor cannot depend on daemon cwd",
        );
    }
    let wait_seconds = request.wait_seconds.unwrap_or(DEFAULT_SPAWN_WAIT_SECONDS);
    let no_wait = request.no_wait;

    let session = match request.session.as_deref() {
        Some(session) if !session.trim().is_empty() => session.to_string(),
        Some(_) => return refused(command_name, "--session must be non-empty"),
        None => {
            let Some(caller_pane) = request.caller_pane.as_deref() else {
                return refused(
                    command_name,
                    "no caller pane was supplied — pass --session <tmux> explicitly",
                );
            };
            let panes = match state.services.tmux.list_panes().await {
                Ok(panes) => panes,
                Err(error) => return internal(command_name, error),
            };
            let Some(pane) = panes.iter().find(|pane| pane.id == caller_pane) else {
                return refused(
                    command_name,
                    format!(
                        "caller pane `{caller_pane}` did not resolve to a tmux session — pass --session <tmux> explicitly"
                    ),
                );
            };
            pane.session.clone()
        }
    };

    // The caller's spawn guard keeps classification, observed launch, and the
    // descriptor write in one daemon-local critical section.
    let spawn_id = match mint_spawn_id() {
        Ok(spawn_id) => spawn_id,
        Err(error) => return internal(command_name, error),
    };
    let (seat_id, displaced) = match request.id.clone() {
        Some(id) => match state.services.registry.get(&id).await {
            Ok(Some(existing)) if existing.tombstoned_at.is_none() => {
                let reason = if existing_policy.allow_absent {
                    format!(
                        "seat `{id}` already exists and is not tombstoned — choose another --id or tombstone it before respawning"
                    )
                } else {
                    format!("seat `{id}` is live, so it cannot be revived")
                };
                return refused(command_name, reason);
            }
            Ok(Some(existing)) => (id, Some(existing)),
            Ok(None) if existing_policy.allow_absent => (id, None),
            Ok(None) => {
                return refused(
                    command_name,
                    format!("seat `{id}` does not exist, so it cannot be revived"),
                );
            }
            Err(error) => return internal(command_name, error),
        },
        None if existing_policy.allow_absent => {
            match allocate_memorable_id(&*state.services.registry, &spawn_id).await {
                Ok(Some(id)) => (id, None),
                Ok(None) => {
                    return refused(
                        command_name,
                        "every memorable name is taken — pass an explicit --id",
                    );
                }
                Err(error) => return internal(command_name, error),
            }
        }
        None => return refused(command_name, "revive requires an existing seat id"),
    };

    let plan = match build_spawn_plan(SpawnPlanInput {
        seat_id: seat_id.clone(),
        spawn_id: spawn_id.clone(),
        harness: request.harness,
        executable: request.executable,
        model: request.model,
        resolved_provider: None,
        effort: request.effort,
        accept_inbound: request.accept_inbound,
        resume: request.resume.clone(),
    }) {
        Ok(plan) => plan,
        Err(error) => return refused(command_name, error),
    };
    if request.allow_retired && state.services.retired_harnesses.contains(&request.harness) {
        let at = match system_time_ms() {
            Ok(at) => at,
            Err(error) => return internal(command_name, error),
        };
        if let Err(error) = state
            .services
            .event_bus
            .publish(Event {
                seq: None,
                v: wire::EVENT_VERSION,
                at,
                kind: "spawn.retired-harness-override".to_string(),
                seat: Some(seat_id.clone()),
                payload: serde_json::json!({
                    "harness": request.harness,
                    "allow_retired": true,
                    "spawn_id": spawn_id,
                })
                .to_string(),
            })
            .await
        {
            return internal(command_name, error);
        }
    }
    let requested_model = plan.model.clone();
    let (status_path, log_path) = spawn_evidence_paths(&spawn_id);
    if let Some(parent) = status_path.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent).await
    {
        return internal(command_name, error);
    }
    let wrapper_executable = match std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
    {
        Some(path) => path,
        None => {
            return internal(
                command_name,
                PijError::Adapter {
                    adapter: "spawn".to_string(),
                    message: "the running pij-rs executable path is not valid UTF-8".to_string(),
                },
            );
        }
    };
    let command = match observed_launch_command(&plan, wrapper_executable, &status_path, &log_path)
    {
        Ok(command) => command,
        Err(error) => return internal(command_name, error),
    };
    let name = request
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| seat_id.as_str().to_string());
    let pane = match state
        .services
        .tmux
        .new_window(&session, &name, &request.cwd, Some(&command))
        .await
    {
        Ok(pane) => pane,
        Err(error) => return internal(command_name, error),
    };
    if displaced.is_some()
        && let Err(error) = publish_revive_postmortem(state, &seat_id).await
    {
        let _ = state.services.tmux.kill(&pane.id).await;
        return internal(command_name, error);
    }

    // Carry seat and launch intent into the fresh row; never carry possession.
    // `proc` remains absent until the new incarnation registers.
    let mut descriptor = SeatDescriptor::new(seat_id, plan.harness, request.cwd);
    descriptor.pane = Some(pane.id.clone());
    descriptor.parent = request.parent;
    descriptor.cross_session_inbound_accept = Some(plan.cross_session_inbound_accept);
    descriptor.spawn_id = Some(spawn_id);
    descriptor.model = plan.model;
    descriptor.provider = plan.provider;
    descriptor.effort = plan.effort;
    // A resumed launch keeps its conversation key, so no later registration can
    // mistake the relaunch for a different conversation (plan 156 rule 3).
    descriptor.harness_session = request.resume;
    if let Err(write_error) = state.services.registry.put(descriptor.clone()).await {
        let rollback = state.services.tmux.kill(&pane.id).await;
        let message = match rollback {
            Ok(()) => format!(
                "observed launch in pane {} but registry write failed ({write_error}); the pane was removed",
                pane.id
            ),
            Err(kill_error) => format!(
                "observed launch in pane {} but registry write failed ({write_error}); rollback also failed ({kill_error})",
                pane.id
            ),
        };
        return internal(
            command_name,
            PijError::Adapter {
                adapter: "spawn".to_string(),
                message,
            },
        );
    }
    drop(spawn_guard);

    if no_wait {
        return spawn_success(command_name, descriptor, revive_receipt);
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_seconds);
    let mut last_output = String::new();
    loop {
        if let Ok(output) = state.services.tmux.capture(&pane.id, 20).await
            && !output.trim().is_empty()
        {
            last_output = output;
        }
        match state.services.registry.get(&descriptor.id).await {
            Ok(Some(bound)) if bound.proc.is_some() => {
                if requested_model_matches(requested_model.as_deref(), bound.model.as_deref(), true)
                {
                    let _ = tokio::fs::remove_file(&status_path).await;
                    return spawn_success(command_name, bound, None);
                }
                let actual = bound.model.as_deref().unwrap_or("no-model");
                let footer_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                loop {
                    if let Ok(output) = state.services.tmux.capture(&pane.id, 20).await
                        && !output.trim().is_empty()
                    {
                        let footer_visible = output.contains(actual);
                        last_output = output;
                        if footer_visible {
                            break;
                        }
                    }
                    if tokio::time::Instant::now() >= footer_deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                let reason = format!(
                    "model-mismatch: requested {}, actual {actual}",
                    requested_model.as_deref().unwrap_or("default")
                );
                return match spawn_failure(
                    state,
                    command_name,
                    &bound,
                    SpawnFailure {
                        exit_code: None,
                        reason: &reason,
                        status: StatusCode::BAD_GATEWAY,
                        last_output: &last_output,
                        log_path: &log_path,
                    },
                )
                .await
                {
                    Ok(response) => response,
                    Err(error) => internal(command_name, error),
                };
            }
            Ok(_) => {}
            Err(error) => return internal(command_name, error),
        }
        match read_spawn_exit(&status_path).await {
            Ok(Some(exit_code)) => {
                return match spawn_failure(
                    state,
                    command_name,
                    &descriptor,
                    SpawnFailure {
                        exit_code: Some(exit_code),
                        reason: "child exited before registration",
                        status: StatusCode::BAD_GATEWAY,
                        last_output: &last_output,
                        log_path: &log_path,
                    },
                )
                .await
                {
                    Ok(response) => response,
                    Err(error) => internal(command_name, error),
                };
            }
            Ok(None) => {}
            Err(error) => return internal(command_name, error),
        }
        let pane_exists = match state.services.tmux.list_panes().await {
            Ok(panes) => panes.iter().any(|listed| listed.id == pane.id),
            Err(error) => return internal(command_name, error),
        };
        if !pane_exists {
            return match spawn_failure(
                state,
                command_name,
                &descriptor,
                SpawnFailure {
                    exit_code: None,
                    reason: "pane disappeared before registration and before an exit code was recorded",
                    status: StatusCode::BAD_GATEWAY,
                    last_output: &last_output,
                    log_path: &log_path,
                },
            )
            .await
            {
                Ok(response) => response,
                Err(error) => internal(command_name, error),
            };
        }
        if tokio::time::Instant::now() >= deadline {
            let reason = format!("timed out after {wait_seconds}s waiting for registration");
            return match spawn_failure(
                state,
                command_name,
                &descriptor,
                SpawnFailure {
                    exit_code: None,
                    reason: &reason,
                    status: StatusCode::GATEWAY_TIMEOUT,
                    last_output: &last_output,
                    log_path: &log_path,
                },
            )
            .await
            {
                Ok(response) => response,
                Err(error) => internal(command_name, error),
            };
        }
        tokio::time::sleep(SPAWN_POLL_INTERVAL).await;
    }
}

fn spawn_evidence_paths(spawn_id: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join("pij-rs-spawn");
    (
        root.join(format!("{spawn_id}.status")),
        root.join(format!("{spawn_id}.log")),
    )
}

async fn read_spawn_exit(path: &Path) -> Result<Option<i32>> {
    let value = match tokio::fs::read_to_string(path).await {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(PijError::Adapter {
                adapter: "spawn".to_string(),
                message: format!(
                    "could not read child exit status {}: {error}",
                    path.display()
                ),
            });
        }
    };
    let value = value
        .trim()
        .strip_prefix("exit_code=")
        .unwrap_or(value.trim());
    value
        .parse::<i32>()
        .map(Some)
        .map_err(|error| PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!(
                "child exit status {} was not an integer: {error}",
                path.display()
            ),
        })
}

fn spawn_success(
    command_name: &'static str,
    seat: SeatDescriptor,
    receipt: Option<revive::Receipt>,
) -> Response {
    let pid = seat.proc.map(|identity| identity.pid);
    let bound = pid.is_some();
    let mut answer = Envelope::ok(
        command_name,
        SpawnResponse {
            seat,
            dispatched: true,
            bound,
            pid,
        },
    );
    answer.details = receipt.map(|receipt| serde_json::json!(receipt));
    envelope(StatusCode::OK, &answer)
}

struct SpawnFailure<'a> {
    exit_code: Option<i32>,
    reason: &'a str,
    status: StatusCode,
    last_output: &'a str,
    log_path: &'a Path,
}

async fn spawn_failure(
    state: &AppState,
    command_name: &'static str,
    seat: &SeatDescriptor,
    failure: SpawnFailure<'_>,
) -> Result<Response> {
    let SpawnFailure {
        exit_code,
        reason,
        status,
        last_output,
        log_path,
    } = failure;
    let wrapper_output = match tokio::fs::read_to_string(log_path).await {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(PijError::Adapter {
                adapter: "spawn".to_string(),
                message: format!("could not read spawn log {}: {error}", log_path.display()),
            });
        }
    };
    let observed = if last_output.trim().is_empty() {
        wrapper_output.as_str()
    } else {
        last_output
    };
    let log = if observed.trim().is_empty() {
        "no child output was captured\n".to_string()
    } else {
        last_lines(observed, 20)
    };
    tokio::fs::write(log_path, &log)
        .await
        .map_err(|error| PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!("could not write spawn log {}: {error}", log_path.display()),
        })?;
    let spawn_id = seat.spawn_id.as_deref().unwrap_or("unknown");
    let pane = seat.pane.as_deref().unwrap_or("unknown");
    state
        .services
        .event_bus
        .publish(Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at: system_time_ms()?,
            kind: "spawn.failed".to_string(),
            seat: Some(seat.id.clone()),
            payload: serde_json::json!({
                "spawn_id": spawn_id,
                "pane": pane,
                "exit_code": exit_code,
                "reason": reason,
                "log_path": log_path,
                "last_output": &log,
            })
            .to_string(),
        })
        .await?;
    let exit = exit_code.map_or_else(
        || "exit code unavailable".to_string(),
        |code| format!("exit code {code}"),
    );
    let meta = format!(
        "spawn.failed: seat {} in pane {pane} {reason} ({exit}); log: {}; last output:\n{log}",
        seat.id,
        log_path.display()
    );
    let mut refusal = Envelope::<SpawnResponse>::refused(command_name, ErrorKind::Adapter, meta);
    refusal.details = Some(serde_json::json!({
        "dispatched": true,
        "bound": seat.proc.is_some(),
        "pane": pane,
        "pid": seat.proc.map(|identity| identity.pid),
        "reason": reason,
    }));
    Ok(envelope(status, &refusal))
}
fn last_lines(value: &str, count: usize) -> String {
    let lines = value.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

#[derive(Deserialize)]
struct SendBodyRequest {
    #[serde(flatten)]
    message: SendRequest,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    caller: Option<CallerContext>,
}

async fn send(State(state): State<AppState>, Json(request): Json<SendBodyRequest>) -> Response {
    if request.message.from.0 == pij_core::BG_ACTOR {
        return refused(
            "pij send",
            "pij-bg is a daemon-owned sender; callers cannot impersonate it",
        );
    }
    if request.message.fyi {
        return hold_fyi(&state, request).await;
    }
    if let Some(command) = request.command {
        if request.message.to.machine.is_some() || request.message.from_machine.is_some() {
            return refused(
                "pij send",
                "E-RS-CONTROL-IDENTITY: remote controls require a caller proven on the target daemon",
            );
        }
        return send_control(
            &state,
            "pij send",
            ControlRequest {
                to: Some(request.message.to.seat),
                asserted_from: Some(request.message.from),
                body: request.message.body,
                command,
                msg_id: request.message.msg_id,
                in_reply_to: request.message.in_reply_to,
                caller: request.caller,
            },
        )
        .await;
    }
    let request = request.message;
    if request.to.machine.is_some() {
        let Some(federation) = state.federation.as_ref() else {
            return refused(
                "pij send",
                "remote destinations require configured federation peers",
            );
        };
        return match federation.enqueue_remote(&request).await {
            Ok(receipt) => envelope(StatusCode::OK, &Envelope::ok("pij send", receipt)),
            Err(FederationSendError::Refused(reason)) => refused("pij send", reason),
            Err(FederationSendError::Runtime(error)) => internal("pij send", error),
        };
    }
    // ONE delivery path. This route used to enqueue a payload shape of its own
    // and synthesise a Queued receipt, which meant it consulted no routing
    // policy, no tombstone and no transport, published no event, and wrote a
    // payload the inbox reading that same queue could not decode. Two halves of
    // one round trip, each proven against itself. Found by u-extension.
    let forwarded = request.from_machine.is_some();
    // A forward was sent on another machine, whose sender this daemon cannot
    // offer the --fyi/--force choice; only local senders are guarded.
    let cold_check = if forwarded {
        None
    } else {
        match cold_wake::guard(
            &state,
            "pij send",
            &request.from,
            &request.to.seat,
            &request.msg_id,
            cold_wake::Override {
                force: request.force,
                reason: request.reason.as_deref(),
            },
        )
        .await
        {
            Ok(label) => label,
            Err(response) => return *response,
        }
    };
    let msg = Msg {
        from: request.from,
        // Carried from the wire: a federation worker forwarding A's message to B
        // stamps it, and a local client simply omits it.
        from_machine: request.from_machine,
        to: request.to.seat,
        body: request.body,
        msg_id: request.msg_id,
        in_reply_to: request.in_reply_to,
        command: None,
    };
    let accepted = if forwarded {
        state.services.delivery.accept_forwarded(msg).await
    } else {
        state.services.delivery.accept(msg).await
    };
    match accepted {
        Ok(mut receipt) => {
            receipt.cold_check = cold_check;
            envelope(StatusCode::OK, &Envelope::ok("pij send", receipt))
        }

        Err(error) => send_failure("pij send", error),
    }
}

/// `pij send --fyi` (plan 158): hold the message for the recipient's next real
/// turn. Refused with a control, or for a remote seat, whose daemon would have
/// to hold it and cannot be asked to here.
async fn hold_fyi(state: &AppState, request: SendBodyRequest) -> Response {
    if request.command.is_some() {
        return refused(
            "pij send",
            "--fyi holds a message; a control cannot be held",
        );
    }
    let request = request.message;
    if request.to.machine.is_some() || request.from_machine.is_some() {
        return refused(
            "pij send",
            "E-RS-FYI-REMOTE: --fyi is held by the recipient's own daemon; send remote seats a normal message",
        );
    }
    let msg = Msg {
        from: request.from,
        from_machine: None,
        to: request.to.seat,
        body: request.body,
        msg_id: request.msg_id,
        in_reply_to: request.in_reply_to,
        command: None,
    };
    let question = pij_core::fyi::looks_like_a_question(&msg.body);
    let recipient = msg.to.clone();
    match state.services.delivery.hold_fyi(msg).await {
        Ok(mut receipt) => {
            fyi::after_hold(state, &recipient, question, &mut receipt).await;
            envelope(StatusCode::OK, &Envelope::ok("pij send", receipt))
        }
        Err(error) => send_failure("pij send", error),
    }
}

/// Both native and shim controls converge here before touching delivery.
pub(crate) struct ControlRequest {
    pub(crate) to: Option<SeatId>,
    pub(crate) asserted_from: Option<SeatId>,
    pub(crate) body: String,
    pub(crate) command: String,
    pub(crate) msg_id: String,
    pub(crate) in_reply_to: Option<String>,
    pub(crate) caller: Option<CallerContext>,
}

pub(crate) async fn send_control(
    state: &AppState,
    name: &str,
    request: ControlRequest,
) -> Response {
    use pij_core::control::{authorize_command, validate_command, validate_target};
    if request
        .to
        .as_ref()
        .is_some_and(|seat| seat.0 == pij_core::BG_ACTOR)
    {
        return refused(name, "pij-bg is a daemon-owned sender, not a recipient");
    }
    if let Err(reason) = validate_command(&request.command, &request.body) {
        return refused(name, reason);
    }
    let target = if let Some(to) = request.to.as_ref() {
        match state.services.registry.get(to).await {
            Ok(Some(target)) => {
                if let Err(reason) = validate_target(&target) {
                    return refused(name, reason);
                }
                Some(target)
            }
            Ok(None) => {
                return send_failure(
                    name,
                    PijError::NoRegistryEntry {
                        seat: to.clone(),
                        store: "the daemon registry".to_string(),
                    },
                );
            }
            Err(error) => return internal(name, error),
        }
    } else {
        None
    };
    let Some(caller) = request.caller else {
        return refused(
            name,
            "E-RS-CONTROL-IDENTITY: a control requires an observable caller pane",
        );
    };
    let Some(pane) = caller.pane.filter(|pane| !pane.trim().is_empty()) else {
        return refused(
            name,
            "E-RS-CONTROL-IDENTITY: an asserted seat or process alone cannot authorize a control; supply the caller pane",
        );
    };
    let actor =
        match identity::resolve_seat(state, name, caller.session_id, Some(pane.clone())).await {
            identity::Resolved::Seat(seat, identity::ResolvedBy::Pane) => seat,
            identity::Resolved::Seat(_, _) => {
                return refused(name, "E-RS-CONTROL-IDENTITY: caller was not pane-derived");
            }
            identity::Resolved::Refusal(response) => return response,
        };
    if request
        .asserted_from
        .as_ref()
        .is_some_and(|from| from != &actor.id)
    {
        return refused(
            name,
            "E-RS-CONTROL-IDENTITY: body sender disagrees with the pane-derived caller",
        );
    }
    let pane_process = match state.services.tmux.pane_process(&pane).await {
        Ok(Some(process)) => process,
        Ok(None) => return refused(name, "E-RS-CONTROL-IDENTITY: caller pane no longer exists"),
        Err(error) => return internal(name, error),
    };
    let observed = match pane_harness_identity(state, pane_process.pid, actor.harness).await {
        Ok(observed) => observed,
        Err(error) => return internal(name, error),
    };
    if !actor.proc.is_some_and(|identity| {
        observed
            .as_ref()
            .is_some_and(|observed| observed.subtree.contains(&identity))
    }) {
        return refused(
            name,
            "E-RS-CONTROL-IDENTITY: caller's recorded process is not live in its pane",
        );
    }
    let target = target.as_ref().unwrap_or(&actor);
    if let Err(reason) = validate_target(target)
        .and_then(|()| authorize_command(&request.command, &actor.id, target))
    {
        return refused(name, reason);
    }
    let msg = Msg {
        from: actor.id.clone(),
        from_machine: None,
        to: target.id.clone(),
        body: String::new(),
        command: Some(request.command),
        msg_id: request.msg_id,
        in_reply_to: request.in_reply_to,
    };
    match state.services.delivery.accept_control(msg).await {
        Ok(receipt) => envelope(StatusCode::OK, &Envelope::ok(name, receipt)),
        Err(error) => send_failure(name, error),
    }
}

/// Map a delivery failure onto the wire, once.
///
/// Shared by `/v1/send` and `/v1/shim/send` so the two paths cannot answer the
/// same failure differently — a second copy of this match is a second thing to
/// keep in agreement, and a caller that changed generation would see a
/// different status for an identical refusal.
pub(crate) fn send_failure(command: &str, error: PijError) -> Response {
    match error {
        PijError::Adapter { adapter, message } if adapter == "daemon/virtual-sender" => {
            refused(command, message)
        }
        error @ PijError::SeatIsGone { .. } => envelope(
            StatusCode::GONE,
            &Envelope::<()>::refused(command, ErrorKind::Refused, error.to_string()),
        ),
        error @ PijError::NoRegistryEntry { .. } => envelope(
            StatusCode::NOT_FOUND,
            &Envelope::<()>::refused(command, ErrorKind::NotFound, error.to_string()),
        ),
        error @ PijError::SelfAddressed { .. } => envelope(
            StatusCode::BAD_REQUEST,
            &Envelope::<()>::refused(command, ErrorKind::Refused, error.to_string()),
        ),
        error => internal(command, error),
    }
}
#[derive(Deserialize)]
struct InboxQuery {
    seat: SeatId,
    #[serde(default)]
    wait: bool,
    #[serde(default)]
    peek: bool,
    native_session: Option<String>,
    pid: Option<u32>,
    proc_start: Option<u64>,
}

/// Read one inbox message. The default path claims it for explicit client ack;
/// `peek=true` is observation-only and never changes queue state.
async fn inbox(State(state): State<AppState>, Query(query): Query<InboxQuery>) -> Response {
    if query.peek {
        let result = state
            .services
            .delivery
            .peek_inbox(&query.seat)
            .await
            .map(|claims| crate::delivery::NativeInboxPage {
                claims,
                held_reason: None,
            });
        return native_inbox_reply(&state, &query.seat, result).await;
    }
    let identity = crate::delivery::NativeInboxIdentity {
        native_session: query.native_session,
        pid: query.pid,
        proc_start: query.proc_start,
    };
    let result = state
        .services
        .delivery
        .claim_native_inbox(&query.seat, query.wait, &identity)
        .await;
    native_inbox_reply(&state, &query.seat, result).await
}

#[derive(Deserialize)]
struct InboxTypingQuery {
    seat: SeatId,
    native_session: Option<String>,
    pid: Option<u32>,
    proc_start: Option<u64>,
}

/// Observe native typing without claiming, deferring, releasing, or acknowledging work.
async fn inbox_typing(
    State(state): State<AppState>,
    Query(query): Query<InboxTypingQuery>,
) -> Response {
    let identity = crate::delivery::NativeInboxIdentity {
        native_session: query.native_session,
        pid: query.pid,
        proc_start: query.proc_start,
    };
    match state
        .services
        .delivery
        .native_typing_snapshot(&query.seat, &identity, state.typing_grace_ms)
        .await
    {
        Ok(snapshot) => envelope(StatusCode::OK, &Envelope::ok("pij inbox", snapshot)),
        Err(error) => inbox_failure(error),
    }
}

async fn native_inbox_reply(
    state: &AppState,
    seat: &SeatId,
    result: Result<crate::delivery::NativeInboxPage>,
) -> Response {
    let reason = match state.services.delivery.native_receiver_reason(seat).await {
        Ok(reason) => reason,
        Err(error) => return internal("pij inbox", error),
    };
    match result {
        Ok(page) => native_inbox_response(page, reason),
        Err(error) => inbox_failure_with_reason(error, reason),
    }
}

fn receiver_diagnostic<T>(answer: &mut Envelope<T>, reason: Option<&str>) {
    if let Some(reason) = reason {
        answer.details.get_or_insert_with(|| serde_json::json!({}))["native_receiver_reason"] =
            serde_json::json!(reason);
        match &mut answer.meta {
            Some(meta) if !meta.contains(reason) => {
                meta.push_str("; ");
                meta.push_str(reason);
            }
            None => answer.meta = Some(reason.to_string()),
            Some(_) => {}
        }
    }
}

fn native_inbox_response(page: crate::delivery::NativeInboxPage, reason: Option<&str>) -> Response {
    let mut answer = Envelope::ok("pij inbox", page.claims);
    answer.meta = page
        .held_reason
        .map(|reason| format!("native-consumer-held:{reason}"));
    receiver_diagnostic(&mut answer, reason);
    envelope(StatusCode::OK, &answer)
}

fn inbox_failure(error: PijError) -> Response {
    inbox_failure_with_reason(error, None)
}

fn inbox_failure_with_reason(error: PijError, reason: Option<&str>) -> Response {
    match &error {
        PijError::NativeReceiverLive { expires_in_ms, .. } => {
            let mut answer: Envelope<serde_json::Value> =
                Envelope::refused("pij inbox", ErrorKind::Refused, error.to_string());
            answer.details = Some(serde_json::json!({
                "code": "native-receiver-lease-live",
                "retryable": true,
                "expires_in_ms": expires_in_ms,
            }));
            receiver_diagnostic(&mut answer, reason);
            envelope(StatusCode::CONFLICT, &answer)
        }
        PijError::GovernanceRefused { code, record } if code == "E-RS-INBOX-AUTHORITY-SPLIT" => {
            let mut answer: Envelope<serde_json::Value> =
                Envelope::refused("pij inbox", ErrorKind::Refused, error.to_string());
            answer.details = Some(serde_json::json!({
                "code":code, "authority":serde_json::from_str::<serde_json::Value>(record).ok(),
            }));
            envelope(StatusCode::CONFLICT, &answer)
        }
        PijError::Adapter { adapter, .. } if adapter == "daemon/native-inbox" => {
            refused("pij inbox", error)
        }
        _ => internal("pij inbox", error),
    }
}

/// True while this exact message's latest hold has no newer release or terminal outcome.
async fn is_held(state: &AppState, seat: &SeatId, msg_id: &str) -> Result<bool> {
    let Some(held) = state
        .services
        .event_bus
        .latest_matching_message(seat, "delivery.held", msg_id)
        .await?
    else {
        return Ok(false);
    };
    let released = state
        .services
        .event_bus
        .latest_matching_message(seat, "delivery.released", msg_id)
        .await?;
    if released.is_some_and(|released| released.seq >= held.seq) {
        return Ok(false);
    }
    let Some(outcome) = state
        .services
        .event_bus
        .latest_matching_message(seat, "delivery.outcome", msg_id)
        .await?
    else {
        return Ok(true);
    };
    if outcome.seq <= held.seq {
        return Ok(true);
    }
    if terminal_outcome_message(&outcome.payload)?.as_deref() == Some(msg_id) {
        return Ok(false);
    }
    // A later pending outcome cannot undo an earlier terminal outcome. Only
    // this ambiguous case needs replay, bounded to facts after the latest hold.
    for event in state
        .services
        .spine
        .tail(Some(seat), held.seq.unwrap_or(Seq(0)))
        .await?
    {
        if event.kind == "delivery.outcome"
            && terminal_outcome_message(&event.payload)?.as_deref() == Some(msg_id)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn terminal_outcome_message(payload: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct OutcomeEvent {
        msg_id: String,
        outcome: DeliveryOutcome,
    }

    let event: OutcomeEvent = serde_json::from_str(payload).map_err(|error| PijError::Adapter {
        adapter: "daemon/http".to_string(),
        message: format!("decode delivery outcome event: {error}"),
    })?;
    Ok(match event.outcome {
        DeliveryOutcome::Delivered { .. } | DeliveryOutcome::Refused { .. } => Some(event.msg_id),
        DeliveryOutcome::Queued { .. } | DeliveryOutcome::Held { .. } => None,
    })
}

async fn publish_typing_event<T: Serialize>(
    state: &AppState,
    kind: &str,
    seat: &SeatId,
    payload: &T,
) -> Result<()> {
    let payload = serde_json::to_string(payload).map_err(|error| PijError::Adapter {
        adapter: "daemon/http".to_string(),
        message: format!("encode typing event: {error}"),
    })?;
    state
        .services
        .event_bus
        .publish(Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at: system_time_ms()?,
            kind: kind.to_string(),
            seat: Some(seat.clone()),
            payload,
        })
        .await?;
    Ok(())
}

#[derive(Deserialize)]
struct HoldRequest {
    job_id: pij_core::model::JobId,
    #[serde(flatten)]
    event: HeldEvent,
}

#[derive(Deserialize)]
struct ReleaseRequest {
    job_id: pij_core::model::JobId,
    #[serde(flatten)]
    event: ReleasedEvent,
}

/// Return a recently claimed inbox row to the durable queue, without acknowledging.
///
/// Held means PENDING/unclaimed (prime ruling 2026-09-05), not a long-lived claim.
/// The next eligible read may re-offer it; the extension must re-hold an existing
/// msg_id idempotently while typing continues. The note never owns the body.
async fn hold_inbox(State(state): State<AppState>, Json(request): Json<HoldRequest>) -> Response {
    if request.event.reason != "human-typing" {
        return refused("pij hold", "hold reason must be human-typing");
    }
    let _transition = state.typing_lock.lock().await;
    let (recipient, msg_id) = match state
        .services
        .queue
        .defer(request.job_id, Duration::from_millis(state.typing_grace_ms))
        .await
    {
        Ok(pij_core::ports::DeferOutcome::Deferred { recipient, msg_id }) => (recipient, msg_id),
        Ok(pij_core::ports::DeferOutcome::NotLive { reason }) => {
            return envelope(
                StatusCode::OK,
                &Envelope::ok(
                    "pij hold",
                    serde_json::json!({
                        "msg_id": request.event.msg_id, "held": false, "noop": true, "reason": reason,
                    }),
                ),
            );
        }
        Err(error) => return internal("pij hold", error),
    };
    // Bearer identity is machine-grade: the queue, not the request, attributes the event.
    let event = HeldEvent {
        seat: recipient,
        msg_id,
        ..request.event
    };
    let held = match is_held(&state, &event.seat, &event.msg_id).await {
        Ok(held) => held,
        Err(error) => return internal("pij hold", error),
    };
    if !held
        && let Err(error) = publish_typing_event(&state, "delivery.held", &event.seat, &event).await
    {
        return internal("pij hold", error);
    }
    envelope(
        StatusCode::OK,
        &Envelope::ok(
            "pij hold",
            serde_json::json!({
                "msg_id": event.msg_id, "held": true,
            }),
        ),
    )
}

/// Record the extension's release declaration independently of queue mutation.
/// Only the normal inbox ack attests ReaderRead; a release clears readback even
/// when a restarted extension already claimed or acknowledged the durable row.
async fn release_inbox(
    State(state): State<AppState>,
    Json(request): Json<ReleaseRequest>,
) -> Response {
    let _transition = state.typing_lock.lock().await;
    let outcome = state.services.queue.release_deferred(request.job_id).await;
    // ATTRIBUTION COMES FROM THE QUEUE, NOT THE CALLER — the same rule `hold`
    // states at its own publish site, and the reason is the same: bearer identity
    // is machine-grade, so a request may name a seat or msg_id it does not own.
    // Both readers of this event key on (seat, msg_id) — `is_held` and
    // `active_holds` — so a mismatched key leaves a phantom hold in
    // `state --json` forever and a seat long-polling through its own release.
    // Reviewer f2 (2026-09-05): release published `request.event` verbatim and
    // discarded the recipient the queue had just returned.
    let event = match &outcome {
        Ok(pij_core::ports::ReleaseOutcome::Released { recipient, msg_id }) => ReleasedEvent {
            seat: recipient.clone(),
            msg_id: msg_id.clone(),
            ..request.event.clone()
        },
        // No live row means no authority to correct with; the declaration is
        // still published, carrying what the caller asserted and nothing more.
        _ => request.event.clone(),
    };
    // Publish after the queue attempt so an awakened inbox sees the new eligibility.
    // This event is the extension's declaration, including on named queue no-ops.
    if let Err(error) = publish_typing_event(&state, "delivery.released", &event.seat, &event).await
    {
        return internal("pij release", error);
    }
    let data = match outcome {
        Ok(pij_core::ports::ReleaseOutcome::Released { msg_id, .. }) => serde_json::json!({
            "msg_id": msg_id, "released": true,
        }),
        Ok(pij_core::ports::ReleaseOutcome::NotDeferred) => serde_json::json!({
            "msg_id": request.event.msg_id, "released": false, "noop": true, "reason": "not-deferred",
        }),
        Ok(pij_core::ports::ReleaseOutcome::NotLive { reason }) => serde_json::json!({
            "msg_id": request.event.msg_id, "released": false, "noop": true, "reason": reason,
        }),
        Err(error) => return internal("pij release", error),
    };
    envelope(StatusCode::OK, &Envelope::ok("pij release", data))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxReleaseRequest {
    seat: SeatId,
    job_id: pij_core::model::JobId,
    evidence: String,
    #[serde(default)]
    caller: CallerContext,
}

async fn operator_release_inbox(
    State(state): State<AppState>,
    request: std::result::Result<
        Json<InboxReleaseRequest>,
        axum::extract::rejection::JsonRejection,
    >,
) -> Response {
    const COMMAND: &str = "pij inbox release";
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => return refused(COMMAND, error.to_string()),
    };
    if request.evidence.trim().is_empty() {
        return refused(COMMAND, "release requires nonempty evidence");
    }
    let actor = match identity::resolve_seat(
        &state,
        COMMAND,
        request.caller.session_id,
        request.caller.pane,
    )
    .await
    {
        identity::Resolved::Seat(actor, _) => actor,
        identity::Resolved::Refusal(response) => return response,
    };
    let target = match state.services.registry.get(&request.seat).await {
        Ok(Some(target)) if target.tombstoned_at.is_none() => target,
        Ok(_) => return refused(COMMAND, "release requires a current recipient"),
        Err(error) => return internal(COMMAND, error),
    };
    if let Err(error) = state
        .services
        .delivery
        .release_inbox_head(
            &target.id,
            request.job_id,
            &format!("{}: {}", actor.id, request.evidence),
            actor.id.clone(),
            state.services.roles.clone(),
        )
        .await
    {
        let mut answer: Envelope<serde_json::Value> =
            Envelope::refused(COMMAND, ErrorKind::Refused, error.to_string());
        let code = match &error {
            PijError::GovernanceRefused { code, .. }
                if matches!(
                    code.as_str(),
                    "E-RS-INBOX-AUTHORITY-SPLIT" | "E-RS-OWNERSHIP"
                ) =>
            {
                code.as_str()
            }
            _ => "E-RS-INBOX-NOT-RUNNING-HEAD",
        };
        answer.details = Some(serde_json::json!({"code":code}));
        return envelope(
            if code == "E-RS-OWNERSHIP" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CONFLICT
            },
            &answer,
        );
    }
    envelope(
        StatusCode::OK,
        &Envelope::ok(
            COMMAND,
            serde_json::json!({
                "job_id":request.job_id, "seat":target.id,
                "state":"failed", "outcome":"undelivered:operator-released",
            }),
        ),
    )
}

#[derive(serde::Serialize)]
struct InboxAckAudit {
    job_id: pij_core::model::JobId,
    authenticated_machine: &'static str,
    evidence_grade: &'static str,
    outcome: pij_core::model::DeliveryOrigin,
}

/// Holding a body claim is not an acknowledgement or a delivery event.
async fn heartbeat_inbox(
    State(state): State<AppState>,
    Json(request): Json<InboxHeartbeatRequest>,
) -> Response {
    let identity = crate::delivery::NativeInboxIdentity {
        native_session: request.native_session,
        pid: request.pid,
        proc_start: request.proc_start,
    };
    let Some(job_id) = request.job_id else {
        let (Some(observed_at), Some(observed_seq)) = (request.observed_at, request.observed_seq)
        else {
            return refused(
                "pij inbox",
                "native receiver heartbeat requires observed_at and observed_seq",
            );
        };
        return match state
            .services
            .delivery
            .heartbeat_native_receiver(&request.seat, &identity, observed_at, observed_seq)
            .await
        {
            Ok(heartbeat) => envelope(StatusCode::OK, &Envelope::ok("pij inbox", heartbeat)),
            Err(error) => inbox_failure(error),
        };
    };
    if identity.native_session.is_some()
        || identity.pid.is_some()
        || identity.proc_start.is_some()
        || request.observed_at.is_some()
        || request.observed_seq.is_some()
    {
        return refused("pij inbox", "native receiver heartbeat must omit job_id");
    }
    match state
        .services
        .delivery
        .heartbeat_inbox(&request.seat, job_id)
        .await
    {
        Ok(job_state) => envelope(
            StatusCode::OK,
            &Envelope::ok(
                "pij inbox",
                serde_json::json!({"job_id": job_id, "state": job_state}),
            ),
        ),
        Err(error) => inbox_failure(error),
    }
}

/// Record `ReaderRead` only after the queue authority commits the named job.
///
/// Bearer auth attests to a machine, not `request.seat`; attribution therefore
/// comes from the acknowledged job's serial key. The event carries the committed
/// outcome and is never minted for an invalid job or failed ledger write.
async fn ack_inbox(
    State(state): State<AppState>,
    Extension(authenticated): Extension<auth::AuthenticatedMachine>,
    Json(request): Json<InboxAckRequest>,
) -> Response {
    if let Some(outcome) = request.delivery_outcome.as_deref() {
        if outcome != "undelivered:harness-swallowed" || request.control_outcome.is_some() {
            return refused(
                "pij inbox",
                "delivery_outcome must be undelivered:harness-swallowed for a body claim",
            );
        }
        return match state
            .services
            .delivery
            .fail_inbox(&request.seat, request.job_id)
            .await
        {
            Ok(()) => envelope(StatusCode::OK, &Envelope::ok("pij inbox", request.job_id)),
            Err(error) => inbox_failure(error),
        };
    }
    let acknowledged = match state
        .services
        .delivery
        .acknowledge_inbox(
            &request.seat,
            request.job_id,
            &request.native,
            request.control_outcome.as_ref(),
        )
        .await
    {
        Ok(acknowledged) => acknowledged,
        Err(error) => return inbox_failure(error),
    };
    let payload = match serde_json::to_string(&InboxAckAudit {
        job_id: request.job_id,
        authenticated_machine: authenticated.0,
        evidence_grade: "machine",
        outcome: acknowledged.origin,
    }) {
        Ok(payload) => payload,
        Err(error) => return internal("pij inbox", error),
    };
    let at = match system_time_ms() {
        Ok(at) => at,
        Err(error) => return internal("pij inbox", error),
    };
    if let Err(error) = state
        .services
        .event_bus
        .publish(pij_core::model::Event {
            seq: None,
            v: wire::EVENT_VERSION,
            at,
            kind: INBOX_ACK_EVENT_KIND.to_string(),
            seat: Some(acknowledged.recipient),
            payload,
        })
        .await
    {
        return internal("pij inbox", error);
    }

    envelope(StatusCode::OK, &Envelope::ok("pij inbox", request.job_id))
}

/// `pij state <id>` — read one seat's card back from the store it lives in.
///
/// The readback half of ac-1142: `report now` writes a card where the seat
/// lives, and this reads it back from the same place. Its answer is a
/// projection of the registry row plus one liveness observation; it writes
/// nothing and enqueues nothing.
///
/// # Every refusal carries an envelope
///
/// A bare 404 is not available to this route. Wave 1's shim classifies
/// `404/405 with an undecodable body` as ROUTE-ABSENCE and falls back to legacy
/// (`.pi/extensions/pij/core/generation-routing.ts:353-363`), so a bare 404 here
/// would not read as "no such seat" — it would read as "rs does not implement
/// `state`", and the caller would be silently served from the OTHER store. That
/// is the split-brain this plan exists to prevent, arriving through the error
/// path. Both refusals below are `Envelope::refused`, and
/// `state_refusal_is_not_classifiable_as_route_absence` pins it.
async fn seat_state(State(state): State<AppState>, Json(request): Json<StateRequest>) -> Response {
    let Some(id) = request.seat() else {
        return envelope(
            StatusCode::BAD_REQUEST,
            &Envelope::<StateCard>::refused(
                "pij state",
                ErrorKind::Refused,
                "state names no seat: send `{\"id\":\"<seat>\"}` or an argv carrying one",
            ),
        );
    };
    let seat = match state
        .services
        .status
        .read(state.services.registry.as_ref(), Some(&id))
        .await
        .map(|rows| {
            rows.into_iter()
                .next()
                .map(pij_store::status::SeatStatus::into_projection)
        }) {
        Ok(Some(seat)) => seat,
        Ok(None) => {
            return envelope(
                StatusCode::NOT_FOUND,
                &Envelope::<StateCard>::refused(
                    "pij state",
                    ErrorKind::NotFound,
                    format!("no seat `{id}` in this store"),
                ),
            );
        }
        Err(error) => return internal("pij state", error),
    };
    let seat = match state.services.roles.project_seat(seat).await {
        Ok(seat) => seat,
        Err(error) => return internal("pij state", error),
    };

    // Liveness is OBSERVED, never inferred from the row. `pij_core::liveness`
    // owns the three-way verdict (active / dead / recycled); re-deriving it here
    // from `proc_start` would be a second implementation of the one rule that
    // stops a recycled pid reading as a live seat.
    let liveness = match seat.proc {
        Some(proc) => match pij_core::liveness::alive(proc, state.services.liveness.as_ref()).await
        {
            Ok(verdict) => Some(verdict),
            Err(error) => return internal("pij state", error),
        },
        None => None,
    };

    // The card, read through the SAME service the write path uses
    // (`report` above builds it identically). Consuming `ReportService` rather
    // than re-deriving a projection is what keeps the read and the write
    // agreeing about staleness, the card limit, and which spine event is
    // "latest" — a second implementation here would drift from the first
    // silently, and the drift would only ever show up as an operator reading a
    // card that the writer thinks says something else.
    let clock = match system_time_ms() {
        Ok(now) => now,
        Err(error) => return internal("pij state", error),
    };
    let reports = pij_core::report::ReportService::new(
        state.services.registry.as_ref(),
        state.services.spine.as_ref(),
        move || clock,
        pij_core::report::ReportConfig::default(),
    );
    let card = match reports.card(&id).await {
        Ok(card) => card,
        Err(error) => return internal("pij state", error),
    };
    // The NOTE lives on the state history record, not the descriptor: the
    // descriptor carries the declared state, the record carries why. Reading
    // only the descriptor would report a blocked seat with no reason, which is
    // the half of the answer nobody needs.
    let state_record = match reports.latest_state_record(&id).await {
        Ok(record) => record,
        Err(error) => return internal("pij state", error),
    };
    let session_status = session_status_block(
        state.services.session_status.as_ref(),
        &seat.id,
        seat.harness,
        seat.harness_session.clone(),
        clock,
    )
    .await;
    let size = pij_core::cold_wake::seat_size(seat.state, &session_status, clock);
    let size_lines = pij_core::cold_wake::size_lines(&session_status, &size);
    let mut projection = state_card(seat, liveness, &state.machine_alias, card, state_record);
    projection.card.session_status = Some(session_status);
    projection.card.size = size;
    projection.card.size_lines = size_lines;
    projection.card.held = match active_holds(&state, &id).await {
        Ok(holds) => holds,
        Err(error) => return internal("pij state", error),
    };
    projection.card.delivery_deferrals = match state.services.queue.delivery_deferrals(&id).await {
        Ok(deferrals) => deferrals,
        Err(error) => return internal("pij state", error),
    };
    projection.card.pending_fyis = match state.services.delivery.pending_fyi_count(&id).await {
        Ok(count) => count,
        Err(error) => return internal("pij state", error),
    };
    projection.card.native_receiver_reason =
        match state.services.delivery.native_receiver_reason(&id).await {
            Ok(reason) => reason.map(str::to_string),
            Err(error) => return internal("pij state", error),
        };

    envelope(StatusCode::OK, &Envelope::ok("pij state", projection))
}

/// Read one seat's session facts into the `sessionStatus` block.
///
/// A source failure fills the block and never fails the card, because the rest
/// of `pij state` stays true without it.
pub(crate) async fn session_status_block(
    source: &dyn pij_core::ports::SessionStatusPort,
    seat: &SeatId,
    harness: Harness,
    session: Option<String>,
    now_ms: u64,
) -> pij_core::session_status::SessionStatusBlock {
    use pij_core::session_status::{SessionStatusBlock, SessionStatusReply, SessionTarget};
    let Some(session) = session else {
        return SessionStatusBlock::Unbound;
    };
    let target = SessionTarget {
        seat: seat.clone(),
        harness,
        session,
    };
    let started = std::time::Instant::now();
    let reply = source.status(&target).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match reply {
        Ok(SessionStatusReply::Status(status)) => SessionStatusBlock::Known {
            cache_state: pij_core::session_status::cache_state(&status, now_ms),
            status: Box::new(status),
            elapsed_ms,
        },
        Ok(SessionStatusReply::Unsupported) => SessionStatusBlock::Unsupported { harness },
        Ok(SessionStatusReply::NotFound { detail }) => SessionStatusBlock::NotFound { detail },
        Err(error) => SessionStatusBlock::Failed {
            error: error.to_string(),
        },
    }
}

async fn active_holds(state: &AppState, seat: &SeatId) -> Result<Vec<HeldEvent>> {
    #[derive(Deserialize)]
    struct ProjectedHold {
        #[serde(flatten)]
        held: HeldEvent,
        job_id: Option<pij_core::model::JobId>,
    }
    let mut holds = BTreeMap::new();
    for event in state.services.spine.tail(Some(seat), Seq(0)).await? {
        match event.kind.as_str() {
            "delivery.held" => {
                // Job diagnostics are projected from live rows, not sampled history.
                // A terminal job must disappear even if it has no release event.
                let projection: ProjectedHold =
                    serde_json::from_str(&event.payload).map_err(|error| PijError::Adapter {
                        adapter: "daemon/http".to_string(),
                        message: format!("decode held event: {error}"),
                    })?;
                if projection.job_id.is_none() {
                    holds.insert(projection.held.msg_id.clone(), projection.held);
                }
            }
            "delivery.released" => {
                let released: ReleasedEvent =
                    serde_json::from_str(&event.payload).map_err(|error| PijError::Adapter {
                        adapter: "daemon/http".to_string(),
                        message: format!("decode released event: {error}"),
                    })?;
                holds.remove(&released.msg_id);
            }
            "delivery.outcome" => {
                if let Some(msg_id) = terminal_outcome_message(&event.payload)? {
                    holds.remove(&msg_id);
                }
            }
            _ => {}
        }
    }
    Ok(holds.into_values().collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateProjection {
    #[serde(flatten)]
    card: StateCard,
    session: Option<String>,
    generation: &'static str,
}

/// Project a registry row into the `state` card. Pure: no IO, no clock.
fn state_card(
    seat: SeatDescriptor,
    liveness: Option<pij_core::model::Liveness>,
    machine_alias: &str,
    card: Option<pij_core::report::CardStatus>,
    state_record: Option<pij_core::report::StateRecord>,
) -> StateProjection {
    use pij_core::model::Liveness;
    let badge = seat.badge.unwrap_or_else(|| {
        pij_core::status::badge_of(Some(seat.state), seat.semantic_state.as_slice()).to_string()
    });
    let (state_note, assignment_id, refs) = state_record
        .map(|record| (record.note, record.assignment_id, record.refs))
        .unwrap_or_default();
    StateProjection {
        session: seat.harness_session,
        generation: "rs",
        card: StateCard {
            id: seat.id,
            state: seat.state.as_str().to_string(),
            badge,
            last_event_at: seat.last_event_at,
            // A seat that was never bound to a process has no process to observe, so
            // it gets its own word rather than borrowing `dead` — "never had one" and
            // "had one and it is gone" are different facts about a seat.
            liveness: match liveness {
                Some(Liveness::Active) => "active",
                Some(Liveness::Dead { .. }) => "dead",
                Some(Liveness::Recycled { .. }) => "recycled",
                None => "unbound",
            }
            .to_string(),
            native_receiver_reason: None,
            pid: seat.proc.map(|proc| proc.pid),
            proc_start: seat.proc.map(|proc| proc.proc_start),
            cwd: seat.folder,
            harness: seat.harness,
            extension_build: seat.extension_build,
            extension_path: seat.extension_path,
            role: seat.role,
            parent: seat.parent,
            bound_model: seat.model,
            effort: seat.effort,
            pane: seat.pane,
            provider: seat.provider,
            semantic_state: seat.semantic_state,
            machine: Some(seat.machine.unwrap_or_else(|| machine_alias.to_string())),
            tombstoned_at: seat.tombstoned_at,
            tombstone_reason: seat.tombstone_reason,
            // `as_ref()` throughout: all five come from ONE optional card, so they
            // are all present or all null together. Reading them from separate
            // sources would let a caller see a `statusPrev` with no `statusAt`.
            status_prev: card.as_ref().map(|status| status.card.did.clone()),
            status_next: card.as_ref().map(|status| status.card.next.clone()),
            status_at: card.as_ref().map(|status| status.card.at),
            status_seq: card
                .as_ref()
                .and_then(|status| status.card.seq)
                .map(|seq| seq.0),
            status_stale: card.as_ref().map(|status| status.stale),
            state_note,
            assignment_id,
            refs,
            held: Vec::new(),
            delivery_deferrals: Vec::new(),
            pending_fyis: 0,
            session_status: None,
            size: pij_core::cold_wake::SeatSize::default(),
            size_lines: Vec::new(),
            unsupported: unsupported_state_fields(),
        },
    }
}

/// The TS `state --json` fields this store cannot answer, each with its reason.
///
/// Every entry is a field a legacy consumer parses today
/// (`.pi/extensions/pij/core/cli.ts:3630-3684`). They are NAMED rather than
/// emitted as `null` because `null` is itself an answer — "we looked and this
/// seat has none" — and a caller cannot tell that from "this store has no such
/// concept". `last_event_at` is now supplied by the same indexed query as
/// the roster. Missing activity observers remain explicitly unsupported.
fn unsupported_state_fields() -> Vec<UnsupportedField> {
    [
        ("lifecycle", "rs seats have no pending/bound/dissolved lifecycle field; binding evidence lives in `proc` and is reported through `liveness`"),
        ("activity", "the TS activity object has no rs counterpart; `state` carries working/idle published at OMP, Pi and Copilot turn boundaries (plan 158) and by Claude's turn hooks (plan 157); a seat that has not published reads idle by default, not as evidence of inactivity"),
        ("ageMs", "not emitted; consumers may subtract last_event_at from their epoch-ms clock"),
        ("liveness:stale", "the card reports a live process-identity probe, not activity staleness; activity staleness is not tracked"),
        ("daemonLastTickAt", "no per-seat daemon tick is recorded in the rs store"),
        ("daemonTickAgeMs", "derived from the tick rs does not record"),
        ("daemonTickStale", "derived from the tick rs does not record"),
        ("failureReason", "rs records no per-seat failure reason on the descriptor"),
        ("bindHealth", "the TS pre-bind health classifier has no rs counterpart"),
        ("degraded", "derived from `bindHealth`"),
        ("degradedReason", "derived from `bindHealth`"),
        ("terminal", "rs records termination as `tombstonedAt` + `tombstoneReason`, which this card DOES carry — a different shape, not a missing fact"),
        ("watchdog", "rs has no watchdog block on the seat row"),
    ]
    .into_iter()
    .map(|(field, why)| UnsupportedField {
        field: field.to_string(),
        why: why.to_string(),
    })
    .collect()
}

#[derive(Default, Deserialize)]
struct SeatQuery {
    harness: Option<Harness>,
    folder: Option<String>,
    parent: Option<SeatId>,
    scope: Option<String>,
    here: Option<String>,
    /// Plan 160: read each local seat's session facts and add its size and
    /// coldness. Opt-in, because other readers of the roster (federation
    /// fan-in, extension rosters) poll it and need none of it.
    #[serde(default)]
    sizes: bool,
}

#[derive(Serialize)]
struct SeatProjection<'a> {
    #[serde(flatten)]
    seat: &'a SeatDescriptor,
    generation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    badge: Option<&'a str>,
    last_event_at: Option<u64>,
    #[serde(rename = "sessionStatus", skip_serializing_if = "Option::is_none")]
    session_status: Option<pij_core::session_status::SessionStatusBlock>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    size: Option<pij_core::cold_wake::SeatSize>,
    /// CTX, IDLE, CACHE and the ❄ marker, rendered once here so every client
    /// prints the same columns.
    #[serde(rename = "sizeColumns", skip_serializing_if = "Option::is_none")]
    size_columns: Option<[String; 4]>,
}

/// How long `pij list` waits for one seat's session facts (plan 160). The
/// reads run in parallel, so this bounds the whole list. A read that runs over
/// keeps going in the background (the source detaches it and keeps its cursor),
/// so the next list is warm; this one shows the seat's size as unknown.
const LIST_STATUS_WAIT: Duration = Duration::from_millis(200);

/// Every local seat's session facts and derived size, read in parallel with a
/// bounded wait each. Remote seats (another machine's roster) get none.
async fn list_sizes(
    state: &AppState,
    seats: &[SeatDescriptor],
) -> Vec<
    Option<(
        pij_core::session_status::SessionStatusBlock,
        pij_core::cold_wake::SeatSize,
    )>,
> {
    let now_ms = system_time_ms().unwrap_or_default();
    let mut reads = tokio::task::JoinSet::new();
    for (index, seat) in seats.iter().enumerate() {
        if seat.machine.as_deref() != Some(state.machine_alias.as_str()) {
            continue;
        }
        let source = Arc::clone(&state.services.session_status);
        let (id, harness, session, seat_state) = (
            seat.id.clone(),
            seat.harness,
            seat.harness_session.clone(),
            seat.state,
        );
        reads.spawn(async move {
            let block = tokio::time::timeout(
                LIST_STATUS_WAIT,
                session_status_block(source.as_ref(), &id, harness, session, now_ms),
            )
            .await
            .unwrap_or_else(|_| pij_core::session_status::SessionStatusBlock::Failed {
                error: format!("no answer within {} ms", LIST_STATUS_WAIT.as_millis()),
            });
            let size = pij_core::cold_wake::seat_size(seat_state, &block, now_ms);
            (index, block, size)
        });
    }
    let mut sizes = vec![None; seats.len()];
    while let Some(read) = reads.join_next().await {
        if let Ok((index, block, size)) = read {
            sizes[index] = Some((block, size));
        }
    }
    sizes
}

#[derive(Serialize)]
struct FederatedRosterProjection<'a> {
    seats: Vec<SeatProjection<'a>>,
    unavailable: &'a [UnavailablePeer],
}

async fn seats_post(
    State(state): State<AppState>,
    Query(mut query): Query<SeatQuery>,
    Json(request): Json<decisions::ReadRequest>,
) -> Response {
    let filters = match decisions::parse_filters(&request.argv, "list", &["here"], &["here"]) {
        Ok(filters) => filters,
        Err(error) => return error.response("pij seats"),
    };
    let here = match decisions::here_from_flag(
        filters.get("here").map(String::as_str),
        request.caller.cwd.as_deref(),
    ) {
        Ok(here) => here,
        Err(error) => return error.response("pij seats"),
    };
    if let Some(here) = here {
        if query.here.is_some() {
            return decisions::DecisionError::argument("duplicate here scope")
                .response("pij seats");
        }
        query.here = Some(here.to_string());
    }
    seats(State(state), Query(query)).await
}

async fn seats(State(state): State<AppState>, Query(query): Query<SeatQuery>) -> Response {
    let here = match decisions::here_path(query.here.as_deref()) {
        Ok(here) => here,
        Err(error) => return error.response("pij seats"),
    };
    if query.scope.as_deref().is_some_and(|scope| scope != "local") {
        return refused("pij seats", "scope must be `local` when supplied");
    }
    let listed = state
        .services
        .status
        .read(state.services.registry.as_ref(), None)
        .await;
    let mut local = match listed {
        Ok(seats) => seats
            .into_iter()
            .map(pij_store::status::SeatStatus::into_projection)
            .collect::<Vec<_>>(),
        Err(error) => return internal("pij seats", error),
    };
    if let Err(error) = state.services.roles.join_roles(&mut local).await {
        return internal("pij seats", error);
    }
    for seat in &mut local {
        seat.machine = Some(state.machine_alias.clone());
    }
    let mut roster = if query.scope.as_deref() == Some("local") {
        FederatedRoster {
            seats: local,
            unavailable: Vec::new(),
        }
    } else if let Some(federation) = &state.federation {
        federation.federated_roster(local)
    } else {
        FederatedRoster {
            seats: local,
            unavailable: Vec::new(),
        }
    };
    roster.seats.retain(|seat| {
        seat.tombstoned_at.is_none()
            && here.as_deref().is_none_or(|folder| seat.folder == folder)
            && query.harness.is_none_or(|harness| seat.harness == harness)
            && query
                .folder
                .as_ref()
                .is_none_or(|folder| &seat.folder == folder)
            && query
                .parent
                .as_ref()
                .is_none_or(|parent| seat.parent.as_ref() == Some(parent))
    });
    let mut sizes = if query.sizes {
        list_sizes(&state, &roster.seats).await
    } else {
        vec![None; roster.seats.len()]
    };
    let projection = FederatedRosterProjection {
        seats: roster
            .seats
            .iter()
            .zip(sizes.iter_mut())
            .map(|(seat, sized)| {
                let (session_status, size) = sized.take().unzip();
                SeatProjection {
                    seat,
                    generation: "rs",
                    badge: seat.badge.as_deref(),
                    last_event_at: seat.last_event_at,
                    size_columns: size.as_ref().map(pij_core::cold_wake::size_columns),
                    session_status,
                    size,
                }
            })
            .collect(),
        unavailable: &roster.unavailable,
    };
    envelope(StatusCode::OK, &Envelope::ok("pij seats", projection))
}

#[derive(Default, Deserialize)]
struct EventQuery {
    /// URL-encoded JSON object: `{\"desktop\":12,\"laptop\":7}`.
    since: Option<String>,
    scope: Option<String>,
}

async fn events(State(state): State<AppState>, Query(query): Query<EventQuery>) -> Response {
    if query.scope.as_deref().is_some_and(|scope| scope != "local") {
        return refused("pij events", "scope must be `local` when supplied");
    }
    let cursors: BTreeMap<String, u64> = match query.since {
        Some(encoded) => match serde_json::from_str(&encoded) {
            Ok(cursors) => cursors,
            Err(error) => {
                return refused(
                    "pij events",
                    format!(
                        "since must be a JSON object mapping machine aliases to cursors: {error}"
                    ),
                );
            }
        },
        None => BTreeMap::new(),
    };
    // REFUSE a cursor for a machine this daemon cannot serve. Ignoring it made a
    // consumer holding a cursor for a renamed or unconfigured machine silently
    // lose that machine's replay, and receive a 200 that presented silence as
    // completeness (review F5). A key we cannot honour is named, not dropped.
    let servable: BTreeSet<&str> = std::iter::once(state.machine_alias.as_str())
        .chain(
            state
                .federation
                .iter()
                .flat_map(|federation| federation.peer_aliases()),
        )
        .collect();
    let unknown: Vec<&str> = cursors
        .keys()
        .map(String::as_str)
        .filter(|machine| !servable.contains(machine))
        .collect();
    if !unknown.is_empty() {
        return refused(
            "pij events",
            format!(
                "since names machine(s) this daemon does not serve: {}. Known: {}",
                unknown.join(", "),
                servable.into_iter().collect::<Vec<_>>().join(", ")
            ),
        );
    }

    let local = match cursors.get(&state.machine_alias).copied() {
        Some(cursor) => match state
            .services
            .event_bus
            .subscribe(Some(Seq(cursor)), EventFilter::all())
            .await
        {
            Ok(subscription) => subscription,
            // A cursor beyond the spine is the CLIENT's stale state, not our
            // failure: 409 with a machine-readable kind, so a consumer branches on
            // the KIND and never on our wording (review F10).
            Err(error @ PijError::CursorBeyondSpine { requested, newest }) => {
                let mut refusal = Envelope::<CursorResetDetail>::refused(
                    "pij events",
                    ErrorKind::CursorReset,
                    error.to_string(),
                );
                // The numbers travel as DATA so a client never reads them out of a
                // sentence, and never has to invent them.
                refusal.data = Some(CursorResetDetail { requested, newest });
                return envelope(StatusCode::CONFLICT, &refusal);
            }
            Err(error) => return internal("pij events", error),
        },
        // Absent cursor means live-only. `subscribe(None, ...)` would replay the
        // whole spine and is forbidden at an attach boundary (COMMON §6).
        None => state.services.event_bus.subscribe_live(EventFilter::all()),
    };
    let local_alias = state.machine_alias.clone();
    let local = local.map(move |event| StreamFrame::Event {
        machine: local_alias.clone(),
        cursor: event
            .seq
            .expect("EventBus subscriptions always assign cursors")
            .0,
        event,
    });
    // `/v1/events` is the explicit streaming exception to R4: the request
    // handler attaches to owned worker/bus streams; peer network IO remains in
    // the federation workers rather than running inline here.
    let frames: Pin<Box<dyn Stream<Item = StreamFrame> + Send>> =
        if query.scope.as_deref() != Some("local") {
            if let Some(federation) = &state.federation {
                Box::pin(local.merge(federation.subscribe_remote(&cursors)))
            } else {
                Box::pin(local)
            }
        } else {
            Box::pin(local)
        };
    let hello = match wire::encode_hello(BUILD) {
        Ok(hello) => hello,
        Err(error) => return internal("pij events", error),
    };
    let frames = frames
        .map(encode_frame)
        .map(|result| result.map(Bytes::from));
    let stream = tokio_stream::once(Ok::<Bytes, io::Error>(Bytes::from(hello))).chain(frames);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response()
}

fn encode_frame(frame: StreamFrame) -> io::Result<String> {
    serde_json::to_string(&frame)
        .map(|line| format!("{line}\n"))
        .map_err(io::Error::other)
}

fn mint_spawn_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| PijError::Adapter {
        adapter: "spawn".to_string(),
        message: format!("the OS refused randomness for the spawn id: {error}"),
    })?;
    let mut id = String::with_capacity(34);
    id.push_str("s-");
    for byte in bytes {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(id)
}

pub(crate) fn system_time_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: "daemon/http".to_string(),
            message: format!("system clock is before the Unix epoch: {error}"),
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| PijError::Adapter {
        adapter: "daemon/http".to_string(),
        message: "system clock exceeds the supported millisecond range".to_string(),
    })
}
/// What the shim and the rs CLI send to `/v1/report`.
///
/// `argv` is the caller's own `process.argv.slice(2)`, verb included and
/// otherwise untouched. The SEAT is a separate field because `report` is
/// FIRST-PERSON testimony and its subject is never typed: the TypeScript
/// generation resolves the reporting seat from the caller's environment
/// (`resolveReportingSelf`, `.pi/extensions/pij/core/cli.ts:1992`), which is a
/// place the daemon cannot see. So the caller forwards it, and this handler
/// treats it as a CLAIM to check against the registry, never as an identity to
/// trust.
#[derive(Debug, Deserialize)]
struct ReportRequest {
    /// Optional agreement claim. Caller evidence is resolved by the daemon;
    /// contradictory explicit and environment claims are refused together.
    #[serde(default)]
    seat: Option<String>,
    /// Forwarded identity evidence, never a blanket environment dump.
    /// The shared ladder checks both the session id and observable pane.
    #[serde(default)]
    caller: Option<CallerContext>,
    argv: Vec<String>,
}

// NOTE (composition, plan 114): u-report and u-identity each defined a
// `CallerContext` for the SAME wire object. Unified onto `identity::CallerContext`
// rather than renaming one — two names for one concept is split-brain by
// construction, which is the defect this plan exists to remove. The richer
// definition wins because it documents every field as a CLAIM and carries the
// wire spellings beside the Rust names.

/// What a successful report answers with.
///
/// `line` is the human sentence the TypeScript CLI prints today
/// (`reported by <seat>: "<did>" → "<next>" (spine <n>)`). It travels in the
/// payload because the ANSWER is half of "the same shapes callers already type",
/// and the routed path is not in this unit's hands: giving the shim a rendered
/// line means it can print one without re-deriving the wording from fields, and
/// without a second copy of the sentence living on the far side of the wire.
#[derive(Debug, serde::Serialize)]
struct ReportReceipt {
    seat: String,
    seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<SemanticState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    assignment_id: Option<String>,
    refs: Vec<String>,
    line: String,
}

/// `POST /v1/report` — the whole report family (ac-1142, ac-1144).
///
/// Every refusal below answers 400 with an envelope. None answers a bare 404:
/// wave 1's shim reads that shape as ROUTE ABSENCE and falls back to legacy, so
/// a refusal wearing it would not refuse anything — it would write the caller's
/// report into the other generation's store while reporting success.
async fn report(State(state): State<AppState>, Json(request): Json<ReportRequest>) -> Response {
    let command = "pij report";

    // Identity first: nothing else is meaningful without a subject, and a report
    // attributed by guess is worse than no report at all.
    //
    // PLAN 117. This block used to resolve `request.seat`, else
    // `caller.session_id`, and nothing else — so `report` was the ONLY routed
    // verb whose subject could not be DERIVED, only ASSERTED. `PIJ_SESSION_ID`
    // is unset in every hand-started seat's shell, which is the normal case on
    // this fleet, so the whole report family refused for every rs-resident seat
    // a human started by hand — exactly the population that owes status cards.
    // Observed live: the same daemon, in the same instant, answered a
    // pane-only caller with `pij-dominant-vicuna` for `/v1/whoami` and "no
    // reporting seat was supplied" for `/v1/report`.
    //
    // The fix is to call the ladder the identity verbs already call rather than
    // to grow a second one here. It resolves a pane against the LIVE roster,
    // and when a pane and an asserted id disagree it refuses both rather than
    // preferring the claim — so this is strictly stronger than what it replaces,
    // which accepted a bare asserted id on an existence check alone.
    let explicit = request.seat.filter(|id| !id.trim().is_empty());
    let caller_seat = request
        .caller
        .as_ref()
        .and_then(|caller| caller.session_id.clone())
        .filter(|id| !id.trim().is_empty());
    if let (Some(explicit), Some(caller_seat)) = (&explicit, &caller_seat)
        && explicit != caller_seat
    {
        return refused(
            command,
            format!(
                "report asserts `{explicit}` but caller.PIJ_SESSION_ID names `{caller_seat}`; identity claims must agree"
            ),
        );
    }
    let asserted = explicit.or(caller_seat);
    let pane = request
        .caller
        .as_ref()
        .and_then(|c| c.pane.clone())
        .filter(|pane| !pane.trim().is_empty());

    // The both-absent case keeps its own wording rather than the ladder's,
    // because the ladder speaks for routes that also accept `?pane=`/`?seat=`
    // query forms and this one does not — naming a door that is not there is
    // how the previous message misled.
    //
    // AND BECAUSE THE PREVIOUS MESSAGE NAMED THE WRONG SIDE OF THE WIRE. It
    // said "the caller must forward it", which reads as a caller-side omission
    // and sends every investigator to the shim. The caller HAD forwarded its
    // pane; this handler was the side that never looked. An error message is a
    // diagnostic instrument, and that one was miscalibrated toward
    // self-exculpation: it cost a fleet-wide misdiagnosis before anyone read
    // this function. So it now says what rs actually looked at.
    if asserted.is_none() && pane.is_none() {
        return refused(
            command,
            "no reporting seat could be resolved. `report` is first-person and its subject is \
             never typed, so rs must derive it. rs looked at two things and found neither: a \
             pane (`caller.TMUX_PANE`), which it checks against its own live roster, and a \
             seat id (`seat`, or `caller.PIJ_SESSION_ID`), which it checks for existence. \
             Send either. Refusing rather than attributing this card by guess.",
        );
    }

    // The claim, checked — by the shared ladder, which also refuses a tombstoned
    // row and a seat this store does not hold. EXISTENCE IS NOT ENOUGH (F004): a
    // tombstoned seat still has a row, so an existence check writes cards and
    // success receipts for a recipient DELIVERY already refuses. Identity,
    // reporting and delivery have to agree about who is addressable, or the fleet
    // gets a card it can read and a seat it cannot reach.
    //
    // `ReportService::now` appends to the spine WITHOUT consulting the registry —
    // correct for a service whose caller has already resolved the seat, and the
    // reason this check cannot be skipped here: a card for a seat rs does not
    // hold would otherwise land in the rs spine under a name rs has never heard
    // of, invisible to the store the seat really lives in, with every surface
    // reporting success. That is the split-brain this plan exists to prevent.
    let seat = match identity::resolve_seat(&state, command, asserted, pane).await {
        identity::Resolved::Seat(descriptor, _) => descriptor.id,
        identity::Resolved::Refusal(response) => return response,
    };

    let call = match report::parse_report(&request.argv) {
        Ok(call) => call,
        Err(refusal) => return refused(command, refusal),
    };

    let clock = match system_time_ms() {
        Ok(now) => now,
        Err(error) => return internal(command, error),
    };
    let reports = pij_core::report::ReportService::new(
        state.services.registry.as_ref(),
        state.services.spine.as_ref(),
        move || clock,
        pij_core::report::ReportConfig::default(),
    );

    match call {
        report::ReportCall::Now {
            did,
            next,
            state: declared,
            note,
        } => {
            // TypeScript writes the state THEN the status, under one lock
            // (`core/cli.ts:1631`). rs's two ports offer no cross-port
            // transaction, so the order is kept and a partial result is reported
            // as the partial result it is — `pij_core::report` makes the same
            // choice for the same reason, and inventing a rollback story here
            // would be a second, false account of the same failure.
            if let Some(declared) = declared
                && let Err(error) = reports
                    .declare(&seat, Some(declared), note.as_deref(), None, &[])
                    .await
            {
                return report_failure(command, error);
            }
            match reports.now(&seat, &did, &next).await {
                Ok(seq) => {
                    let line = format!(
                        "reported by {seat}: \"{did}\" → \"{next}\" (spine {})",
                        seq.0
                    );
                    envelope(
                        StatusCode::OK,
                        &Envelope::ok(
                            command,
                            ReportReceipt {
                                seat: seat.to_string(),
                                seq: seq.0,
                                did: Some(did),
                                next: Some(next),
                                state: declared,
                                note,
                                assignment_id: None,
                                refs: Vec::new(),
                                line,
                            },
                        ),
                    )
                }
                Err(error) => report_failure(command, error),
            }
        }
        report::ReportCall::State {
            state: declared,
            note,
            assignment_id,
            refs,
        } => {
            if let Err(error) = state
                .services
                .decisions
                .validate_report_assignment(&seat, assignment_id.as_deref())
                .await
            {
                return error.response(command);
            }
            declaration_response(
                command,
                &reports,
                &seat,
                Some(declared),
                note,
                assignment_id,
                refs,
            )
            .await
        }
        report::ReportCall::Question {
            note,
            assignment_id,
            refs,
        } => decisions::question(&state, &seat, &note, assignment_id.as_deref(), &refs).await,
        report::ReportCall::Verify { target, assignment } => {
            decisions::verify(&state, &seat, &target, assignment).await
        }
        report::ReportCall::Clear { assignment_id } => {
            if let Err(error) = state
                .services
                .decisions
                .validate_report_assignment(&seat, assignment_id.as_deref())
                .await
            {
                return error.response(command);
            }
            declaration_response(
                command,
                &reports,
                &seat,
                None,
                None,
                assignment_id,
                Vec::new(),
            )
            .await
        }
    }
}

/// One answer shape for every state declaration, `clear` included — `clear` is a
/// declaration of `None`, not a different operation, and giving it its own arm
/// would be a second place for the same sentence to be written.
async fn declaration_response<R, S, C>(
    command: &str,
    reports: &pij_core::report::ReportService<'_, R, S, C>,
    seat: &SeatId,
    declared: Option<SemanticState>,
    note: Option<String>,
    assignment_id: Option<String>,
    refs: Vec<String>,
) -> Response
where
    R: Registry + ?Sized,
    S: pij_core::ports::Spine + ?Sized,
    C: Fn() -> u64,
{
    match reports
        .declare(
            seat,
            declared,
            note.as_deref(),
            assignment_id.as_deref(),
            &refs,
        )
        .await
    {
        Ok(seq) => {
            let line = match declared {
                Some(state) => match &note {
                    Some(note) => format!("{seat} is {state:?}: {note} (spine {})", seq.0),
                    None => format!("{seat} is {state:?} (spine {})", seq.0),
                },
                None => format!("{seat} cleared its declared state (spine {})", seq.0),
            };
            envelope(
                StatusCode::OK,
                &Envelope::ok(
                    command,
                    ReportReceipt {
                        seat: seat.to_string(),
                        seq: seq.0,
                        did: None,
                        next: None,
                        state: declared,
                        note,
                        assignment_id,
                        refs,
                        line,
                    },
                ),
            )
        }
        Err(error) => report_failure(command, error),
    }
}

/// Refusal or adapter fault — told apart, because only one of them is a bug.
///
/// `ReportTooLong` is the caller's mistake and answers 400 with the limit named;
/// a storage failure is ours and answers 500. Collapsing them would either blame
/// the caller for our fault or hide our fault as their mistake.
fn report_failure(command: &str, error: PijError) -> Response {
    match error {
        PijError::ReportTooLong { len, limit } => refused(
            command,
            format!(
                "report text is {len} characters, over the {limit}-character limit after \
                 whitespace collapsing"
            ),
        ),
        PijError::NoRegistryEntry { seat, store } => refused(
            command,
            format!("reporting seat '{seat}' is not registered in {store}"),
        ),
        other => internal(command, other),
    }
}

pub(crate) fn refused(command: &str, reason: impl std::fmt::Display) -> Response {
    envelope(
        StatusCode::BAD_REQUEST,
        &Envelope::<()>::refused(command, ErrorKind::Refused, reason.to_string()),
    )
}

pub(crate) fn internal(command: &str, error: impl std::fmt::Display) -> Response {
    envelope(
        StatusCode::INTERNAL_SERVER_ERROR,
        // An adapter failed at its boundary and its own words travel in `meta`.
        // Every failure route now carries a discriminator: u-cli proved no honest
        // exit-code table can be built from prose, and u-extension found this
        // helper still emitting an envelope with no `error` at all.
        &Envelope::<()>::refused(command, ErrorKind::Adapter, error.to_string()),
    )
}

pub(crate) fn envelope<T: serde::Serialize>(status: StatusCode, body: &Envelope<T>) -> Response {
    match serde_json::to_string(body) {
        Ok(text) => (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            text,
        )
            .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;

/// One sidecar job, named by which consumer will claim it.
///
/// The three consumers are queue-backed, so SOMETHING must enqueue their work.
/// Without this the only producer is hand-written SQL, which proves nothing
/// about the shipped surface — a live-fire transcript that bypasses the product
/// is a transcript of the test harness.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "sidecar", content = "request")]
pub enum SidecarRequest {
    /// Outbound Telegram message; also binds the reply target for this seat.
    Telegram(pij_sidecars::telegram::TelegramSend),
    /// Background command start or cancel.
    Bg(pij_sidecars::background::BgRequest),
    /// Chore definition, run, or baseline acknowledgement.
    Chore(pij_sidecars::chore::ChoreRequest),
}

/// Enqueue one sidecar job and report the id the consumer will serialize on.
async fn sidecar(
    State(state): State<AppState>,
    Extension(_authenticated): Extension<auth::AuthenticatedMachine>,
    Json(request): Json<SidecarRequest>,
) -> Response {
    let request_id = format!(
        "{:016x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64)
    );
    let job = match &request {
        SidecarRequest::Telegram(send) => pij_sidecars::telegram::send_job(send),
        SidecarRequest::Bg(bg) => pij_sidecars::background::job(bg, &request_id),
        SidecarRequest::Chore(chore) => pij_sidecars::chore::job(chore, &request_id),
    };
    let job = match job {
        Ok(job) => job,
        Err(error) => return refused("pij sidecar", error),
    };
    let serial_key = job.serial_key.clone();
    match state.services.queue.enqueue(job).await {
        // The v1 ENVELOPE, not a bare object: every shipped client decodes
        // `{ok, command, data}` and `DaemonClient` refuses anything else with
        // "JSON is not a envelope (missing field ok)". A new route that invents
        // its own success shape is unreachable through the product even when the
        // daemon side is correct — found by a live-fire attempt, not by a test.
        Ok(job_id) => envelope(
            StatusCode::OK,
            &Envelope::ok(
                "pij sidecar",
                serde_json::json!({ "serial_key": serial_key, "job_id": job_id }),
            ),
        ),
        Err(error) => internal("pij sidecar", error),
    }
}
