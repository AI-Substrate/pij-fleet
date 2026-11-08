//! Who is calling (plan 223 AC3, AC4, AC7; plan 221 workshop 001 contracts c
//! to e), counted per operation and, in phase A, never refused for it.
//!
//! A request proves its caller in one of two ways, or neither:
//! - **Token**: the `x-pij-seat-capability` header is a live seat's
//!   capability (the digest map; the previous digest during a rotation too).
//! - **Peer**: the request came over the Unix socket, its peer's credentials
//!   were read at accept (`peer_cred`, never a header, the body or a claimed
//!   pid), the peer's start still matches on this request, and a fresh walk
//!   from the peer reaches the NEAREST registered harness boundary, which
//!   owns exactly one live session.
//!
//! The walk is pure over a [`ProcReader`]; the kernel one reads in-process
//! (`observed_start`, `parent_and_name`) and launches nothing. There is no
//! ancestry cache: every hop is read on every request. The one index kept,
//! [`BoundaryIndex`], is lookup metadata (which `(pid, start)` is a registered
//! harness, and who it hosts), maintained from committed registry facts.
//!
//! Counting follows the OPERATION, not the URI: each seat-authority handler
//! names its [`OperationClass`] (shared handlers by their argv leaf), and the
//! request's proof is counted once per class. Machine-public reads are not
//! counted. Bounded: 8 classes x 3 outcomes, plus first nonce registrations.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use pij_core::model::{Event, Harness, ProcIdentity, SeatDescriptor, SeatId};
use serde::Serialize;

use crate::Services;

/// Hops a walk may take from the peer before it is refused.
pub const MAX_HOPS: usize = 16;

/// Registered harness boundaries the index holds at most. A registration
/// that would add one more is refused [`INDEX_FULL_CODE`]; nothing is evicted.
pub const INDEX_CAP: usize = 4_096;

/// The refusal for unreadable or unsound provenance.
pub const PROVENANCE_CODE: &str = "E-RS-SEAT-PROVENANCE";
/// The refusal for a new boundary at [`INDEX_CAP`].
pub const INDEX_FULL_CODE: &str = "E-RS-SEAT-INDEX-FULL";
/// The refusal for tokenless authority at a boundary hosting two or more
/// live sessions.
pub const AMBIGUOUS_CODE: &str = "E-RS-SEAT-AMBIGUOUS";

/// Process names a walk never passes: the roots above every seat.
const SYSTEM_ROOTS: [&str; 2] = ["launchd", "init"];

/// The operation classes seat authority is counted (and later gated) by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperationClass {
    /// Sends, FYIs and role requests attributed to a seat.
    SendAs,
    /// Claims, acknowledges or renews a recipient's deliveries.
    InboxClaim,
    /// Lifecycle and control actions on a seat or its jobs.
    Controls,
    /// Role, parent, holder, owner and prime actions.
    RoleAndHolder,
    /// Role outputs and request answers.
    Outputs,
    /// Registers or re-registers an incarnation (adoption included).
    Registration,
    /// A seat's own status, activity and reports.
    Activity,
    /// Any-seat writes where impersonation forges only the author.
    Attribution,
}

impl OperationClass {
    /// Every class, in counter order.
    pub const ALL: [Self; 8] = [
        Self::SendAs,
        Self::InboxClaim,
        Self::Controls,
        Self::RoleAndHolder,
        Self::Outputs,
        Self::Registration,
        Self::Activity,
        Self::Attribution,
    ];

    /// The inventory's spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SendAs => "send-as",
            Self::InboxClaim => "inbox-claim",
            Self::Controls => "controls",
            Self::RoleAndHolder => "role-and-holder",
            Self::Outputs => "outputs",
            Self::Registration => "registration",
            Self::Activity => "activity",
            Self::Attribution => "attribution",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// A governance family's class by its leaf: reads are machine-public (not
/// counted); `ack` claims a delivery; role, prime and baton-return leaves of
/// `orchestration` are holder actions; every other write forges only authorship.
pub fn governance_class(family: &str, leaf: &str) -> Option<OperationClass> {
    let read = matches!(leaf, "list" | "show" | "status")
        || leaf.ends_with(".list")
        || leaf.ends_with(".show");
    match family {
        "node" => None,
        _ if read => None,
        "ack" => Some(OperationClass::InboxClaim),
        "orchestration"
            if leaf.starts_with("role.")
                || leaf.starts_with("prime.")
                || matches!(leaf, "baton.return" | "baton.reclaim") =>
        {
            Some(OperationClass::RoleAndHolder)
        }
        _ => Some(OperationClass::Attribution),
    }
}

/// What the connection a request arrived on proves, read once at accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocketPeer {
    /// A Unix-socket peer of this daemon's user: its pid and start.
    Proven(ProcIdentity),
    /// A Unix-socket peer whose credentials could not be trusted, and why. It
    /// never falls back to body identifiers.
    Failed(String),
}

/// Read a socket peer at accept: `peer_cred` for uid and pid, then its start
/// in-process. A missing pid, an error, or another user fails.
pub fn socket_peer(
    stream: &tokio::net::UnixStream,
    daemon_uid: u32,
    reader: &dyn ProcReader,
) -> SocketPeer {
    let credentials = match stream.peer_cred() {
        Ok(credentials) => credentials,
        Err(error) => return SocketPeer::Failed(format!("peer credentials unreadable: {error}")),
    };
    if credentials.uid() != daemon_uid {
        return SocketPeer::Failed(format!(
            "peer uid {} is not this daemon's",
            credentials.uid()
        ));
    }
    let Some(pid) = credentials
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
    else {
        return SocketPeer::Failed("the kernel reported no peer pid".to_string());
    };
    match reader.start(pid) {
        Some(proc_start) => SocketPeer::Proven(ProcIdentity { pid, proc_start }),
        None => SocketPeer::Failed(format!("peer {pid}'s start is unreadable")),
    }
}

/// Process facts a walk reads, one pid at a time, fresh on every call.
pub trait ProcReader: Send + Sync {
    /// The process's start, in the registry's timebase; `None` if unreadable.
    fn start(&self, pid: u32) -> Option<u64>;
    /// Its parent pid and kernel name; `None` if unreadable.
    fn parent_and_name(&self, pid: u32) -> Option<(u32, String)>;
}

/// The kernel, in-process: never `ps`, never the `ProcLiveness` wrapper.
pub struct KernelReader;

impl ProcReader for KernelReader {
    fn start(&self, pid: u32) -> Option<u64> {
        pij_harnesses::proc::observed_start(pid).map(|start| start.local)
    }

    fn parent_and_name(&self, pid: u32) -> Option<(u32, String)> {
        pij_harnesses::proc::parent_and_name(pid)
    }
}

/// Walk from `peer` up its ancestry to the NEAREST boundary: `target` (a
/// registration's own claimed process) or any registered harness. At most
/// [`MAX_HOPS`] parents, every hop read now. The peer must still be the
/// process that connected (same start). Unreadable hops, a reused ancestor
/// pid, pid 1, launchd or a tmux server, and running out of hops all refuse.
/// A root refuses BEFORE it is matched: a root is never a boundary or a
/// target, even one the index or a claim names (review D4).
///
/// # Errors
/// Why the walk was refused (a sentence for [`PROVENANCE_CODE`]).
pub fn walk(
    reader: &dyn ProcReader,
    peer: ProcIdentity,
    target: Option<ProcIdentity>,
    is_boundary: impl Fn(ProcIdentity) -> bool,
) -> Result<ProcIdentity, String> {
    let mut current = peer.pid;
    let mut child_start = None;
    for hop in 0..=MAX_HOPS {
        let start = reader
            .start(current)
            .ok_or_else(|| format!("process {current} is unreadable"))?;
        if hop == 0 && start != peer.proc_start {
            return Err(format!(
                "the connecting process {current} is gone or was replaced"
            ));
        }
        if child_start.is_some_and(|child| start > child) {
            return Err(format!(
                "ancestor pid {current} was reused after its child started"
            ));
        }
        let (parent, name) = reader
            .parent_and_name(current)
            .ok_or_else(|| format!("process {current}'s parent is unreadable"))?;
        if is_root(current, &name) {
            return Err(format!(
                "reached {name} (pid {current}) without a registered harness"
            ));
        }
        let here = ProcIdentity {
            pid: current,
            proc_start: start,
        };
        if target == Some(here) || is_boundary(here) {
            return Ok(here);
        }
        if parent == 0 {
            return Err(format!("process {current} has no parent"));
        }
        child_start = Some(start);
        current = parent;
    }
    Err(format!(
        "no registered harness within {MAX_HOPS} hops of process {}",
        peer.pid
    ))
}

/// Pid 1, launchd or init, or a tmux server: above every seat, never one.
fn is_root(pid: u32, name: &str) -> bool {
    pid == 1 || SYSTEM_ROOTS.contains(&name) || name.starts_with("tmux")
}

/// One live session a boundary hosts.
#[derive(Debug, PartialEq, Eq)]
pub struct Hosted {
    /// The harness.
    pub harness: Harness,
    /// The harness session, when known.
    pub session: Option<String>,
}

#[cfg(test)]
thread_local! {
    /// Copies of [`Hosted`] made on this thread: a lookup must not copy the
    /// roster out under the shared lock (review P183-F001).
    pub(crate) static HOSTED_COPIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

impl Clone for Hosted {
    fn clone(&self) -> Self {
        #[cfg(test)]
        HOSTED_COPIES.with(|copies| copies.set(copies.get() + 1));
        Self {
            harness: self.harness,
            session: self.session.clone(),
        }
    }
}

/// A boundary's map key.
fn key(boundary: ProcIdentity) -> (u32, u64) {
    (boundary.pid, boundary.proc_start)
}

/// Registered harness boundaries `(pid, start)` and the live seats they host.
#[derive(Debug)]
pub struct BoundaryIndex {
    by_boundary: HashMap<(u32, u64), BTreeMap<SeatId, Hosted>>,
    by_seat: HashMap<SeatId, ProcIdentity>,
    /// Admissions in flight, by boundary: each holds a slot from its cap
    /// check until its registration's write has committed or failed.
    pending: HashMap<(u32, u64), usize>,
    cap: usize,
}

/// What a boundary hosts, decided without copying the roster out (review
/// P183-F001): it stops at the second seat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostedBy {
    /// No live seat.
    Nobody,
    /// Exactly this live seat.
    One(SeatId),
    /// Two or more; the first one's harness names the refusal.
    Many(Harness),
}

impl BoundaryIndex {
    /// An empty index holding at most `cap` boundaries.
    pub fn with_cap(cap: usize) -> Self {
        Self {
            by_boundary: HashMap::new(),
            by_seat: HashMap::new(),
            pending: HashMap::new(),
            cap,
        }
    }

    /// Rebuild from the roster (one read at daemon start). The cap is an
    /// ADMISSION limit, not a size: a persisted roster above it is held whole,
    /// nothing is evicted, and only new boundaries are refused.
    pub fn rebuild(&mut self, seats: &[SeatDescriptor]) {
        self.by_boundary.clear();
        self.by_seat.clear();
        for seat in seats {
            self.place(seat);
        }
    }

    /// Follow one seat write: a live, bound seat sits at its process; any
    /// other state leaves the index.
    pub fn place(&mut self, seat: &SeatDescriptor) {
        self.forget(&seat.id);
        if seat.tombstoned_at.is_some() {
            return;
        }
        let Some(boundary) = seat.proc else {
            return;
        };
        self.by_boundary.entry(key(boundary)).or_default().insert(
            seat.id.clone(),
            Hosted {
                harness: seat.harness,
                session: seat.harness_session.clone(),
            },
        );
        self.by_seat.insert(seat.id.clone(), boundary);
    }

    /// Drop a seat (tombstone).
    pub fn forget(&mut self, seat: &SeatId) {
        let Some(boundary) = self.by_seat.remove(seat) else {
            return;
        };
        if let Some(hosted) = self.by_boundary.get_mut(&key(boundary)) {
            hosted.remove(seat);
            if hosted.is_empty() {
                self.by_boundary.remove(&key(boundary));
            }
        }
    }

    /// Follow one committed registry fact.
    pub fn follow(&mut self, event: &Event) {
        match event.kind.as_str() {
            "seat.tombstone" => {
                if let Some(seat) = &event.seat {
                    self.forget(seat);
                }
            }
            "seat.put" => {
                if let Ok(seat) = serde_json::from_str::<SeatDescriptor>(&event.payload) {
                    self.place(&seat);
                }
            }
            _ => {}
        }
    }

    /// Is `boundary` a registered harness? One hash lookup.
    pub fn contains(&self, boundary: ProcIdentity) -> bool {
        self.by_boundary.contains_key(&key(boundary))
    }

    /// Who `boundary` hosts: nobody, exactly one seat (cloned), or many. At
    /// most two entries are visited and one id is cloned.
    pub fn hosted_by(&self, boundary: ProcIdentity) -> HostedBy {
        let Some(hosted) = self.by_boundary.get(&key(boundary)) else {
            return HostedBy::Nobody;
        };
        let mut seats = hosted.iter();
        match (seats.next(), seats.next()) {
            (None, _) => HostedBy::Nobody,
            (Some((seat, _)), None) => HostedBy::One(seat.clone()),
            (Some((_, first)), Some(_)) => HostedBy::Many(first.harness),
        }
    }

    /// Does `boundary` host a live seat other than `seat`? At most two
    /// entries are visited; nothing is cloned.
    pub fn hosts_other(&self, boundary: ProcIdentity, seat: &SeatId) -> bool {
        self.by_boundary
            .get(&key(boundary))
            .is_some_and(|hosted| hosted.keys().take(2).any(|held| held != seat))
    }

    /// May a registration bind `boundary`? An existing (or in-flight) one
    /// always; a new one only while held and in-flight boundaries are below
    /// the cap.
    pub fn admits(&self, boundary: ProcIdentity) -> bool {
        let key = key(boundary);
        self.by_boundary.contains_key(&key)
            || self.pending.contains_key(&key)
            || self.by_boundary.len() + self.pending_new() < self.cap
    }

    /// In-flight admissions of boundaries not yet held.
    fn pending_new(&self) -> usize {
        self.pending
            .keys()
            .filter(|key| !self.by_boundary.contains_key(*key))
            .count()
    }

    /// Check and take a slot in one step.
    fn reserve(&mut self, boundary: ProcIdentity) -> bool {
        if !self.admits(boundary) {
            return false;
        }
        *self.pending.entry(key(boundary)).or_default() += 1;
        true
    }

    fn release(&mut self, key: (u32, u64)) {
        if let Some(count) = self.pending.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                self.pending.remove(&key);
            }
        }
    }

    /// Registered boundaries held.
    pub fn len(&self) -> usize {
        self.by_boundary.len()
    }

    /// No boundary held.
    pub fn is_empty(&self) -> bool {
        self.by_boundary.is_empty()
    }
}

/// The daemon's boundary index, shared by every registration service (the
/// HTTP routes and each binder) and every walk.
#[derive(Debug)]
pub struct Boundaries {
    index: Mutex<BoundaryIndex>,
}

#[cfg(test)]
impl Default for Boundaries {
    fn default() -> Self {
        Self::with_cap(INDEX_CAP)
    }
}

/// A boundary's admission, held from the cap check until the registration's
/// write has committed (the event bus has placed the boundary by then) or
/// failed; dropping it releases the slot. Two services racing for the last
/// slot cannot both pass (review D8).
#[derive(Debug)]
pub struct Admission {
    boundaries: Arc<Boundaries>,
    key: (u32, u64),
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.boundaries.lock().release(self.key);
    }
}

impl Boundaries {
    /// An index holding at most `cap` boundaries.
    pub fn with_cap(cap: usize) -> Self {
        Self {
            index: Mutex::new(BoundaryIndex::with_cap(cap)),
        }
    }

    /// Admit `boundary` (check and reserve in one step under the index's
    /// lock), or `None` at the cap.
    pub fn admit(self: &Arc<Self>, boundary: ProcIdentity) -> Option<Admission> {
        self.lock().reserve(boundary).then(|| Admission {
            boundaries: Arc::clone(self),
            key: key(boundary),
        })
    }

    /// The index, for one short read or update.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, BoundaryIndex> {
        // Every mutation is a whole-entry replace, so a poisoned index is
        // still the last consistent state.
        self.index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The exact ambiguity refusal (plan 223 AC4, ruled by the prime).
pub fn ambiguous(harness: Harness) -> String {
    let name = match harness {
        Harness::Copilot => "Copilot",
        Harness::Claude => "Claude",
        Harness::Codex => "Codex",
        Harness::Omp => "OMP",
        Harness::Pi => "Pi",
    };
    format!(
        "{AMBIGUOUS_CODE}: this {name} process hosts more than one live session; start a fresh seat (pij-rs revive --fresh), don't /new"
    )
}

/// Who a request's caller is proven to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallerProof {
    /// A live seat's capability.
    Token(SeatId),
    /// The socket peer's nearest boundary hosts exactly this live seat.
    Peer(SeatId),
    /// Neither, and why.
    Neither(String),
}

/// How a request proved its caller, as counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// By capability.
    Token,
    /// By peer credentials.
    Peer,
    /// Neither.
    Neither,
}

impl CallerProof {
    /// The counted outcome.
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Token(_) => Outcome::Token,
            Self::Peer(_) => Outcome::Peer,
            Self::Neither(_) => Outcome::Neither,
        }
    }
}

/// The socket peer's proof: the nearest registered boundary, hosting
/// exactly one live seat.
pub fn peer_proof(
    reader: &dyn ProcReader,
    boundaries: &Boundaries,
    peer: &SocketPeer,
) -> CallerProof {
    let peer = match peer {
        SocketPeer::Proven(peer) => *peer,
        SocketPeer::Failed(why) => {
            return CallerProof::Neither(format!("{PROVENANCE_CODE}: {why}"));
        }
    };
    let reached = walk(reader, peer, None, |boundary| {
        boundaries.lock().contains(boundary)
    });
    let boundary = match reached {
        Ok(boundary) => boundary,
        Err(why) => return CallerProof::Neither(format!("{PROVENANCE_CODE}: {why}")),
    };
    let hosted = boundaries.lock().hosted_by(boundary);
    match hosted {
        HostedBy::One(seat) => CallerProof::Peer(seat),
        HostedBy::Nobody => CallerProof::Neither(format!(
            "{PROVENANCE_CODE}: boundary {} hosts no live seat",
            boundary.pid
        )),
        HostedBy::Many(harness) => CallerProof::Neither(ambiguous(harness)),
    }
}

/// Bounded counters: 8 classes x {token, peer, neither}, plus nonce first
/// registrations. Atomic adds only; nothing reads them to decide anything.
#[derive(Debug)]
pub struct CapabilityCounts {
    cells: [[AtomicU64; 3]; 8],
    nonce_first_registrations: AtomicU64,
}

impl Default for CapabilityCounts {
    fn default() -> Self {
        Self {
            cells: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            nonce_first_registrations: AtomicU64::new(0),
        }
    }
}

/// One class's tally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ClassTally {
    /// Proven by capability.
    pub token: u64,
    /// Proven by peer credentials.
    pub peer: u64,
    /// Neither.
    pub neither: u64,
}

/// `/health`'s `capabilities`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CountsSnapshot {
    /// Per operation class.
    pub classes: BTreeMap<&'static str, ClassTally>,
    /// Registrations that consumed a spawn nonce and were issued.
    pub nonce_first_registrations: u64,
}

impl CapabilityCounts {
    /// Count one operation.
    pub fn record(&self, class: OperationClass, outcome: Outcome) {
        let column = match outcome {
            Outcome::Token => 0,
            Outcome::Peer => 1,
            Outcome::Neither => 2,
        };
        self.cells[class.index()][column].fetch_add(1, Ordering::Relaxed);
    }

    /// Count one nonce registration that was issued.
    pub fn nonce_registered(&self) {
        self.nonce_first_registrations
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Read every counter.
    pub fn snapshot(&self) -> CountsSnapshot {
        CountsSnapshot {
            classes: OperationClass::ALL
                .into_iter()
                .map(|class| {
                    let cells = &self.cells[class.index()];
                    (
                        class.as_str(),
                        ClassTally {
                            token: cells[0].load(Ordering::Relaxed),
                            peer: cells[1].load(Ordering::Relaxed),
                            neither: cells[2].load(Ordering::Relaxed),
                        },
                    )
                })
                .collect(),
            nonce_first_registrations: self.nonce_first_registrations.load(Ordering::Relaxed),
        }
    }
}

/// A request's `x-pij-seat-capability` header, resolved once at the door
/// (where a rotation's first presentation retires the previous capability).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenEvidence {
    /// No header.
    Absent,
    /// A live seat's capability.
    Live(SeatId),
    /// A header that is no live capability.
    NotLive,
}

/// What one request brought: its header capability (resolved at the door),
/// its socket peer, and whether it came over the non-loopback listener. Its
/// proof is computed on the first counted operation and reused, and each
/// class counts once per request.
pub(crate) struct RequestEvidence {
    token: TokenEvidence,
    peer: Option<SocketPeer>,
    remote: bool,
    proof: OnceLock<CallerProof>,
    counted: AtomicU8,
}

impl RequestEvidence {
    pub(crate) fn new(token: TokenEvidence, peer: Option<SocketPeer>, remote: bool) -> Self {
        Self {
            token,
            peer,
            remote,
            proof: OnceLock::new(),
            counted: AtomicU8::new(0),
        }
    }

    /// The socket peer, if the request came over the socket.
    pub(crate) fn peer(&self) -> Option<&SocketPeer> {
        self.peer.as_ref()
    }

    fn proof(&self, services: &Services) -> &CallerProof {
        self.proof.get_or_init(|| prove(services, self))
    }
}

tokio::task_local! {
    static REQUEST: Arc<RequestEvidence>;
}

/// Run a request's handler with its evidence in scope.
pub(crate) async fn scoped<F: std::future::Future>(
    evidence: RequestEvidence,
    work: F,
) -> F::Output {
    REQUEST.scope(Arc::new(evidence), work).await
}

/// The evidence of the request being handled, if any.
pub(crate) fn current() -> Option<Arc<RequestEvidence>> {
    REQUEST.try_with(Arc::clone).ok()
}

/// Did the request being handled present `seat`'s live capability?
pub(crate) fn token_proves(seat: &SeatId) -> bool {
    current().is_some_and(|evidence| evidence.token == TokenEvidence::Live(seat.clone()))
}

/// Did the request being handled arrive over the non-loopback listener?
/// Every capability carrier (the header, a body's `context_token`, a
/// registration's nonce) is refused there (review S183-003).
pub(crate) fn remote() -> bool {
    current().is_some_and(|evidence| evidence.remote)
}

fn prove(services: &Services, evidence: &RequestEvidence) -> CallerProof {
    if let TokenEvidence::Live(seat) = &evidence.token {
        return CallerProof::Token(seat.clone());
    }
    let found = match evidence.peer() {
        Some(peer) => peer_proof(services.proc_reader.as_ref(), &services.boundaries, peer),
        None => CallerProof::Neither("not a socket peer and no live capability".to_string()),
    };
    match (found, evidence.token == TokenEvidence::NotLive) {
        (CallerProof::Neither(why), true) => {
            CallerProof::Neither(format!("the presented capability is not live; {why}"))
        }
        (found, _) => found,
    }
}

/// Count the current request's `class` operation once, by its proof. Count
/// only: nothing is refused for the outcome in phase A.
pub(crate) fn count(services: &Services, class: OperationClass) {
    let outcome = match current() {
        Some(evidence) => {
            let bit = 1_u8 << class.index();
            if evidence.counted.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
                return;
            }
            evidence.proof(services).outcome()
        }
        None => Outcome::Neither,
    };
    services.capability_counts.record(class, outcome);
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// The test header naming a request's socket peer as `pid:start`.
    pub(crate) const PEER_HEADER: &str = "x-test-socket-peer";

    /// Serve `router` as if each request carrying [`PEER_HEADER`] arrived over
    /// the socket from that peer (what `serve_unix` inserts at accept).
    pub(crate) fn socket_peers(router: axum::Router) -> axum::Router {
        router.layer(axum::middleware::from_fn(
            |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
                let peer = request
                    .headers()
                    .get(PEER_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split_once(':'))
                    .and_then(|(pid, start)| Some((pid.parse().ok()?, start.parse().ok()?)));
                if let Some((pid, proc_start)) = peer {
                    request
                        .extensions_mut()
                        .insert(SocketPeer::Proven(ProcIdentity { pid, proc_start }));
                }
                next.run(request).await
            },
        ))
    }

    /// [`PEER_HEADER`]'s value for `peer`.
    pub(crate) fn peer_header(peer: ProcIdentity) -> String {
        format!("{}:{}", peer.pid, peer.proc_start)
    }

    /// A scripted process table: pid -> (parent, name, start).
    #[derive(Default)]
    pub(crate) struct FakeReader {
        pub(crate) procs: Mutex<HashMap<u32, (u32, String, u64)>>,
        pub(crate) reads: AtomicU64,
    }

    impl FakeReader {
        pub(crate) fn with(self, pid: u32, parent: u32, name: &str, start: u64) -> Self {
            self.set(pid, parent, name, start);
            self
        }

        pub(crate) fn set(&self, pid: u32, parent: u32, name: &str, start: u64) {
            self.procs
                .lock()
                .unwrap()
                .insert(pid, (parent, name.to_string(), start));
        }

        pub(crate) fn kill(&self, pid: u32) {
            self.procs.lock().unwrap().remove(&pid);
        }
    }

    impl ProcReader for FakeReader {
        fn start(&self, pid: u32) -> Option<u64> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.procs.lock().unwrap().get(&pid).map(|proc| proc.2)
        }

        fn parent_and_name(&self, pid: u32) -> Option<(u32, String)> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.procs
                .lock()
                .unwrap()
                .get(&pid)
                .map(|proc| (proc.0, proc.1.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeReader;
    use super::*;

    fn id(pid: u32, start: u64) -> ProcIdentity {
        ProcIdentity {
            pid,
            proc_start: start,
        }
    }

    fn seat(name: &str, pid: u32, start: u64, session: &str) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(name, Harness::Copilot, "/abs");
        seat.proc = Some(id(pid, start));
        seat.harness_session = Some(session.to_string());
        seat
    }

    /// launchd(1) -> tmux(5) -> shell(9) -> H(10) -> M(11) -> S(12) -> P(13)
    fn chain() -> FakeReader {
        FakeReader::default()
            .with(1, 0, "launchd", 1)
            .with(5, 1, "tmux", 2)
            .with(9, 5, "zsh", 3)
            .with(10, 9, "copilot", 4)
            .with(11, 10, "node", 5)
            .with(12, 11, "zsh", 6)
            .with(13, 12, "pij", 7)
    }

    fn index(seats: &[SeatDescriptor]) -> Boundaries {
        let boundaries = Boundaries::default();
        boundaries.lock().rebuild(seats);
        boundaries
    }

    #[test]
    fn a_walk_stops_at_the_nearest_registered_boundary() {
        let reader = chain();
        let boundaries = index(&[seat("pij-h", 10, 4, "s1")]);
        assert_eq!(
            peer_proof(&reader, &boundaries, &SocketPeer::Proven(id(13, 7))),
            CallerProof::Peer("pij-h".into())
        );
        // A nested registered harness is nearer: it wins, never the outer one.
        boundaries.lock().place(&seat("pij-m", 11, 5, "s2"));
        assert_eq!(
            peer_proof(&reader, &boundaries, &SocketPeer::Proven(id(13, 7))),
            CallerProof::Peer("pij-m".into())
        );
    }

    /// The workshop's regression (spoonbill, governance b2bb85d): H -> M ->
    /// S -> P; M dies and S is reparented to 1. P's fresh walk refuses.
    #[test]
    fn a_reparented_chain_is_refused_with_no_ancestry_cache() {
        let reader = chain();
        let boundaries = index(&[seat("pij-h", 10, 4, "s1")]);
        let peer = SocketPeer::Proven(id(13, 7));
        assert!(matches!(
            peer_proof(&reader, &boundaries, &peer),
            CallerProof::Peer(_)
        ));
        reader.kill(11);
        reader.set(12, 1, "zsh", 6);
        let refused = peer_proof(&reader, &boundaries, &peer);
        assert!(
            matches!(&refused, CallerProof::Neither(why) if why.starts_with(PROVENANCE_CODE) && why.contains("launchd")),
            "{refused:?}"
        );
    }

    /// Review D4: a root is refused BEFORE any match, so pid 1, launchd or a
    /// tmux server is never a boundary (even an indexed one), a target, or a
    /// peer.
    #[test]
    fn a_root_is_never_a_boundary_a_target_or_a_peer() {
        // launchd(1) -> zsh(20) -> pij(21): no tmux in between.
        let flat = FakeReader::default()
            .with(1, 0, "launchd", 1)
            .with(20, 1, "zsh", 3)
            .with(21, 20, "pij", 4);
        let refused = |proof: CallerProof| matches!(&proof, CallerProof::Neither(why) if why.starts_with(PROVENANCE_CODE));
        let launchd = index(&[seat("pij-root", 1, 1, "s1")]);
        let tmux = index(&[seat("pij-tmux", 5, 2, "s1")]);
        assert!(refused(peer_proof(
            &flat,
            &launchd,
            &SocketPeer::Proven(id(21, 4))
        )));
        assert!(refused(peer_proof(
            &chain(),
            &tmux,
            &SocketPeer::Proven(id(13, 7))
        )));
        assert!(walk(&flat, id(21, 4), Some(id(1, 1)), |_| false).is_err());
        assert!(walk(&chain(), id(13, 7), Some(id(5, 2)), |_| false).is_err());
        assert!(refused(peer_proof(
            &flat,
            &launchd,
            &SocketPeer::Proven(id(1, 1))
        )));
        assert!(walk(&chain(), id(5, 2), Some(id(5, 2)), |_| false).is_err());
        // Control: an ordinary boundary below the roots is still reached.
        assert_eq!(
            walk(&chain(), id(13, 7), Some(id(10, 4)), |_| false),
            Ok(id(10, 4))
        );
    }

    /// Review P183-F001: telling nobody, one and many apart copies nothing
    /// out of the shared index, however many sessions a boundary hosts and
    /// however large their fields.
    #[test]
    fn hosted_lookups_copy_nothing_out_of_the_index() {
        let big = "s".repeat(64 * 1024);
        let seats: Vec<SeatDescriptor> = (0..256)
            .map(|n| seat(&format!("pij-many-{n}"), 10, 4, &format!("{n}{big}")))
            .collect();
        let boundaries = index(&seats);
        let copies = || HOSTED_COPIES.with(std::cell::Cell::get);
        let before = copies();
        assert_eq!(
            boundaries.lock().hosted_by(id(10, 4)),
            HostedBy::Many(Harness::Copilot)
        );
        assert!(
            boundaries
                .lock()
                .hosts_other(id(10, 4), &"pij-many-0".into())
        );
        assert_eq!(
            peer_proof(&chain(), &boundaries, &SocketPeer::Proven(id(13, 7))),
            CallerProof::Neither(ambiguous(Harness::Copilot))
        );
        assert_eq!(copies(), before, "a lookup copied hosted sessions");
        let one = index(&[seat("pij-one", 10, 4, "s1")]);
        assert_eq!(
            one.lock().hosted_by(id(10, 4)),
            HostedBy::One("pij-one".into())
        );
        assert!(!one.lock().hosts_other(id(10, 4), &"pij-one".into()));
        assert_eq!(one.lock().hosted_by(id(11, 5)), HostedBy::Nobody);
    }

    /// Review D8: the cap is checked and the slot taken in one step, held
    /// until the admission is dropped (after its write), so two admissions
    /// racing for the last slot cannot both pass.
    #[test]
    fn an_admission_holds_its_slot_until_dropped() {
        let boundaries = Arc::new(Boundaries::with_cap(2));
        boundaries.lock().place(&seat("pij-held", 10, 4, "s1"));
        let first = boundaries.admit(id(20, 4)).expect("the last slot");
        assert!(boundaries.admit(id(30, 4)).is_none(), "the slot is taken");
        assert!(
            boundaries.admit(id(20, 4)).is_some(),
            "the same boundary again"
        );
        assert!(boundaries.admit(id(10, 4)).is_some(), "a held boundary");
        drop(first);
        assert!(boundaries.admit(id(30, 4)).is_some(), "released on failure");
        let committed = boundaries.admit(id(20, 4)).expect("the last slot");
        boundaries.lock().place(&seat("pij-new", 20, 4, "s2"));
        drop(committed);
        assert!(boundaries.admit(id(30, 4)).is_none(), "placed: at the cap");
        // An ADMISSION limit: a persisted roster above it is held whole.
        let over = Boundaries::with_cap(1);
        over.lock()
            .rebuild(&[seat("pij-a", 10, 4, "s1"), seat("pij-b", 20, 4, "s2")]);
        assert_eq!(over.lock().len(), 2);
    }

    #[test]
    fn roots_unreadable_hops_reuse_and_depth_refuse() {
        let boundaries = index(&[]);
        let refuse = |reader: &FakeReader, peer| match peer_proof(
            reader,
            &boundaries,
            &SocketPeer::Proven(peer),
        ) {
            CallerProof::Neither(why) => why,
            other => panic!("{other:?}"),
        };
        assert!(
            refuse(&chain(), id(13, 7)).contains("tmux"),
            "a tmux server ends it"
        );
        let gap = chain();
        gap.kill(12);
        assert!(refuse(&gap, id(13, 7)).contains("unreadable"));
        assert!(
            refuse(&chain(), id(13, 8)).contains("replaced"),
            "the peer's start is re-read"
        );
        let reused = chain().with(12, 11, "zsh", 99);
        assert!(refuse(&reused, id(13, 7)).contains("reused"));
        let deep = (0..40).fold(FakeReader::default(), |reader, pid| {
            reader.with(100 + pid, 101 + pid, "sh", 10)
        });
        assert!(refuse(&deep, id(100, 10)).contains("16 hops"));
        assert_eq!(
            refuse(&chain(), id(13, 7)),
            refuse(&chain(), id(13, 7)),
            "no cache: the same answer every time"
        );
        assert_eq!(
            peer_proof(&chain(), &boundaries, &SocketPeer::Failed("uid".into())),
            CallerProof::Neither(format!("{PROVENANCE_CODE}: uid"))
        );
    }

    #[test]
    fn a_boundary_hosting_two_live_sessions_is_ambiguous() {
        let reader = chain();
        let boundaries = index(&[seat("pij-a", 10, 4, "s1"), seat("pij-b", 10, 4, "s2")]);
        assert_eq!(
            peer_proof(&reader, &boundaries, &SocketPeer::Proven(id(13, 7))),
            CallerProof::Neither(
                "E-RS-SEAT-AMBIGUOUS: this Copilot process hosts more than one live session; start a fresh seat (pij-rs revive --fresh), don't /new".to_string()
            )
        );
        assert!(ambiguous(Harness::Omp).contains("this OMP process"));
        let mut retired = seat("pij-b", 10, 4, "s2");
        retired.tombstoned_at = Some(1);
        boundaries.lock().place(&retired);
        assert_eq!(
            peer_proof(&reader, &boundaries, &SocketPeer::Proven(id(13, 7))),
            CallerProof::Peer("pij-a".into())
        );
    }

    #[test]
    fn the_index_follows_writes_and_admits_new_boundaries_only_below_its_cap() {
        let mut index = BoundaryIndex::with_cap(1);
        let a = seat("pij-a", 10, 4, "s1");
        index.place(&a);
        assert!(index.admits(id(10, 4)), "an existing boundary");
        assert!(!index.admits(id(20, 4)), "a new one at the cap");
        let mut moved = a.clone();
        moved.proc = Some(id(20, 4));
        index.follow(&pij_core::model::Event {
            seq: None,
            v: 1,
            at: 1,
            kind: "seat.put".into(),
            seat: Some(a.id.clone()),
            payload: serde_json::to_string(&moved).unwrap(),
        });
        assert!(!index.contains(id(10, 4)) && index.contains(id(20, 4)));
        index.follow(&pij_core::model::Event {
            seq: None,
            v: 1,
            at: 1,
            kind: "seat.tombstone".into(),
            seat: Some(a.id.clone()),
            payload: "{}".into(),
        });
        assert!(index.is_empty());
    }

    #[test]
    fn counts_are_bounded_per_class_and_outcome() {
        let counts = CapabilityCounts::default();
        counts.record(OperationClass::SendAs, Outcome::Neither);
        counts.record(OperationClass::SendAs, Outcome::Token);
        counts.record(OperationClass::Attribution, Outcome::Peer);
        counts.nonce_registered();
        let snapshot = counts.snapshot();
        assert_eq!(snapshot.classes.len(), 8);
        assert_eq!(
            snapshot.classes["send-as"],
            ClassTally {
                token: 1,
                peer: 0,
                neither: 1
            }
        );
        assert_eq!(snapshot.classes["attribution"].peer, 1);
        assert_eq!(snapshot.nonce_first_registrations, 1);
    }

    /// Plan 223 AC9's per-request cost, on this machine: the real kernel
    /// reads for a peer whose parent is its registered harness (one hop).
    /// `cargo test -p pij-daemon --lib walk_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn walk_cost() {
        let reader = KernelReader;
        let me = std::process::id();
        let peer = id(me, reader.start(me).expect("own start"));
        let (parent, _) = reader.parent_and_name(me).expect("own parent");
        let harness = id(parent, reader.start(parent).expect("parent start"));
        let boundaries = Boundaries::default();
        let mut host = SeatDescriptor::new("pij-host", Harness::Omp, "/abs");
        host.proc = Some(harness);
        boundaries.lock().place(&host);
        let before = pij_core::spawns::snapshot().total;
        let rounds = 10_000;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            assert!(matches!(
                peer_proof(&reader, &boundaries, &SocketPeer::Proven(peer)),
                CallerProof::Peer(_)
            ));
        }
        let each = started.elapsed() / rounds;
        assert_eq!(pij_core::spawns::snapshot().total, before, "no launch");
        println!("peer proof, one hop: {each:?} per request");
    }

    #[test]
    fn governance_leaves_name_their_class() {
        assert_eq!(governance_class("node", "x"), None);
        assert_eq!(governance_class("orchestration", "baton.list"), None);
        assert_eq!(
            governance_class("orchestration", "role.set"),
            Some(OperationClass::RoleAndHolder)
        );
        assert_eq!(
            governance_class("orchestration", "baton.request"),
            Some(OperationClass::Attribution)
        );
        assert_eq!(
            governance_class("ack", "ack"),
            Some(OperationClass::InboxClaim)
        );
        assert_eq!(
            governance_class("project", "create"),
            Some(OperationClass::Attribution)
        );
    }
}
