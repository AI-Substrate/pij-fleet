//! Seat capabilities, the daemon's half (plan 223 AC2; plan 221 workshop 001
//! contracts a and b).
//!
//! A capability is 32 bytes from the OS random source, handed out as hex in
//! exactly one place: the registration response to the process that proved
//! itself (a spawn's nonce, the current capability, or from AC3 the socket
//! peer's harness boundary). The store keeps only its SHA-256; this module
//! keeps a warm digest map rebuilt from the store at daemon start, so a check
//! is one SHA-256 of 32 bytes, a map lookup and a constant-time compare. No
//! launch, no store read. Nothing here logs, prints or serializes a secret:
//! [`Secret`]'s `Debug` is redacted.
//!
//! The map can only be MORE conservative than the store: a check also
//! requires the seat to be live and still bound to the capability's boundary
//! process, so a tombstone or a new process (which delete the row in the store)
//! fails the check even before the map is resynced; registration resyncs a
//! seat's entry from the store under its own lock.

use std::collections::HashMap;
use std::sync::Mutex;

use pij_core::capability::{CapabilityIssue, CapabilityRow, Digest, SECRET_LEN};
use pij_core::error::{PijError, Result};
use pij_core::model::{ProcIdentity, SeatDescriptor, SeatId};
use pij_core::ports::Registry;
use sha2::{Digest as _, Sha256};

use crate::background::{constant_time_eq, hex};

/// The request header a seat presents its capability in.
pub const CAPABILITY_HEADER: &str = "x-pij-seat-capability";

/// How long a spawn's nonce stays presentable, independent of how long the
/// spawn request waits: a Copilot trust prompt or a `--no-wait` spawn can
/// register well after its request returned.
pub const NONCE_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// What a registration presents (plan 223).
#[derive(Clone, Debug)]
pub(crate) enum CapabilityClaim {
    /// Bootstrap evidence or an explicit rotation: issue a new capability in
    /// the transaction that writes the seat.
    Issue(CapabilityIssue),
    /// A same-incarnation re-registration presenting its capability without
    /// asking to rotate: it keeps that capability, and nothing is written to
    /// the capability table.
    Hold(Digest),
    /// Path b (AC3): no evidence presented, over the socket. Issued
    /// ([`pij_core::capability::IssueProof::Peer`], owner = the peer) only if
    /// the peer's nearest boundary is the claimed harness, that boundary hosts
    /// no other live session, and no live owner holds the seat's capability;
    /// otherwise the seat registers without one and `withheld` says why.
    Bootstrap {
        /// The issuance, should the checks pass.
        issue: CapabilityIssue,
        /// Why it was not issued.
        withheld: std::sync::Arc<std::sync::OnceLock<String>>,
    },
}

/// Drop a spawn's nonce handoff file that no launch wrapper consumed (a
/// failed or never-started launch). Called when the spawn is touched or its
/// expired nonce row is dropped; never by a sweep.
pub(crate) fn discard_nonce_file(spawn_id: &str) {
    let path = crate::http::spawn_nonce_path(spawn_id);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "pij capability: could not remove the nonce handoff {}: {error}",
            path.display()
        ),
    }
}

/// A freshly minted secret (a nonce or a capability) and its digest.
pub struct Secret {
    plaintext: String,
    digest: Digest,
}

impl Secret {
    /// The hex secret, for the one place it may go.
    pub fn plaintext(&self) -> &str {
        &self.plaintext
    }

    /// Its SHA-256, the only form ever stored.
    pub fn digest(&self) -> Digest {
        self.digest
    }

    /// Consume into the hex secret.
    pub fn into_plaintext(self) -> String {
        self.plaintext
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

/// Mint 32 bytes from the OS random source.
///
/// # Errors
/// The random source failed.
pub fn mint() -> Result<Secret> {
    let mut bytes = [0_u8; SECRET_LEN];
    getrandom::fill(&mut bytes).map_err(|error| PijError::Adapter {
        adapter: "daemon/capability".to_string(),
        message: format!("random source: {error}"),
    })?;
    Ok(Secret {
        plaintext: hex(&bytes),
        digest: Sha256::digest(bytes).into(),
    })
}

/// The digest of a presented hex secret; `None` when it is not 64 hex digits,
/// which no minted secret ever is.
pub fn digest_of(presented: &str) -> Option<Digest> {
    if presented.len() != SECRET_LEN * 2 || !presented.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let mut bytes = [0_u8; SECRET_LEN];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(presented.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(Sha256::digest(bytes).into())
}

/// What a presented capability proves about a seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The seat's current capability. `rotation_pending` when a previous one
    /// is still valid and this first presentation should retire it.
    Current {
        /// A previous digest awaits retirement.
        rotation_pending: bool,
    },
    /// The previous capability, still valid until the new one is presented.
    Previous,
    /// Not this live incarnation's capability.
    Invalid,
}

impl Verdict {
    /// Does it prove the seat?
    pub fn valid(self) -> bool {
        !matches!(self, Self::Invalid)
    }
}

#[derive(Clone, Debug)]
struct Held {
    digest: Digest,
    prev: Option<Digest>,
    boundary: Option<ProcIdentity>,
}

/// Digest to seat, and each seat's live digests, in memory.
#[derive(Debug, Default)]
pub struct DigestMap {
    by_digest: HashMap<Digest, SeatId>,
    by_seat: HashMap<SeatId, Held>,
}

impl DigestMap {
    /// Build from the store's live rows.
    pub fn rebuild(rows: impl IntoIterator<Item = CapabilityRow>) -> Self {
        let mut map = Self::default();
        for row in rows {
            map.sync(&row.seat.clone(), Some(&row));
        }
        map
    }

    /// Make `seat`'s entry exactly `row` (absent: none).
    pub fn sync(&mut self, seat: &SeatId, row: Option<&CapabilityRow>) {
        self.forget(seat);
        let Some(row) = row.filter(|row| row.seat == *seat) else {
            return;
        };
        self.by_digest.insert(row.digest, seat.clone());
        if let Some(prev) = row.prev_digest {
            self.by_digest.insert(prev, seat.clone());
        }
        self.by_seat.insert(
            seat.clone(),
            Held {
                digest: row.digest,
                prev: row.prev_digest,
                boundary: row.boundary,
            },
        );
    }

    /// Drop `seat`'s entry.
    pub fn forget(&mut self, seat: &SeatId) {
        if let Some(held) = self.by_seat.remove(seat) {
            self.by_digest.remove(&held.digest);
            if let Some(prev) = held.prev {
                self.by_digest.remove(&prev);
            }
        }
    }

    /// The seat a digest belongs to, if any.
    pub fn seat_of(&self, digest: &Digest) -> Option<&SeatId> {
        self.by_digest.get(digest)
    }

    /// Check `digest` for `seat`, a live row bound to `proc`.
    pub fn verify(&self, seat: &SeatId, proc: Option<ProcIdentity>, digest: &Digest) -> Verdict {
        if self.seat_of(digest) != Some(seat) {
            return Verdict::Invalid;
        }
        let Some(held) = self.by_seat.get(seat).filter(|held| held.boundary == proc) else {
            return Verdict::Invalid;
        };
        if constant_time_eq(&held.digest, digest) {
            return Verdict::Current {
                rotation_pending: held.prev.is_some(),
            };
        }
        if held
            .prev
            .as_ref()
            .is_some_and(|prev| constant_time_eq(prev, digest))
        {
            return Verdict::Previous;
        }
        Verdict::Invalid
    }

    /// The seat whose live capability `digest` is (current, or the previous
    /// one still valid during a rotation), confirmed by a constant-time
    /// compare of the stored digest.
    pub fn holder(&self, digest: &Digest) -> Option<&SeatId> {
        self.verdict_of(digest)
            .and_then(|(seat, verdict)| verdict.valid().then_some(seat))
    }

    /// What `digest` proves, and for which seat, without a descriptor (a
    /// header presentation).
    pub fn verdict_of(&self, digest: &Digest) -> Option<(&SeatId, Verdict)> {
        let seat = self.seat_of(digest)?;
        let held = self.by_seat.get(seat)?;
        let verdict = if constant_time_eq(&held.digest, digest) {
            Verdict::Current {
                rotation_pending: held.prev.is_some(),
            }
        } else if held
            .prev
            .as_ref()
            .is_some_and(|prev| constant_time_eq(prev, digest))
        {
            Verdict::Previous
        } else {
            Verdict::Invalid
        };
        Some((seat, verdict))
    }

    /// Follow a committed seat write: a tombstone or a new process revoked the
    /// seat's capability in the store, so it goes here too.
    pub fn follow(&mut self, seat: &SeatDescriptor) {
        if self
            .by_seat
            .get(&seat.id)
            .is_some_and(|held| seat.tombstoned_at.is_some() || held.boundary != seat.proc)
        {
            self.forget(&seat.id);
        }
    }

    /// `digest` was presented and its retirement committed: retire the
    /// previous capability, but only while `digest` is still the seat's
    /// current one. A rotation that landed meanwhile installed a new
    /// generation whose previous capability (`digest` itself) stays valid
    /// (review D2). `true` when one was retired.
    pub fn retire_previous(&mut self, seat: &SeatId, digest: &Digest) -> bool {
        let Some(held) = self
            .by_seat
            .get_mut(seat)
            .filter(|held| constant_time_eq(&held.digest, digest))
        else {
            return false;
        };
        let Some(prev) = held.prev.take() else {
            return false;
        };
        self.by_digest.remove(&prev);
        true
    }

    /// Live seats held.
    pub fn len(&self) -> usize {
        self.by_seat.len()
    }

    /// Nothing held.
    pub fn is_empty(&self) -> bool {
        self.by_seat.is_empty()
    }
}

/// The daemon's digest map, shared by registration and every check.
#[derive(Debug, Default)]
pub struct Capabilities {
    map: Mutex<DigestMap>,
}

impl Capabilities {
    fn lock(&self) -> std::sync::MutexGuard<'_, DigestMap> {
        // A poisoned map is still the last consistent state: every mutation
        // is a whole-entry replace.
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replace the map with the store's live rows.
    pub fn restore(&self, rows: Vec<CapabilityRow>) {
        *self.lock() = DigestMap::rebuild(rows);
    }

    /// Make `seat`'s entry exactly the store's `row`.
    pub fn sync(&self, seat: &SeatId, row: Option<&CapabilityRow>) {
        self.lock().sync(seat, row);
    }

    /// Check `presented` for the live row `seat`, without touching the store.
    pub fn check(&self, seat: &SeatDescriptor, presented: &str) -> Verdict {
        if seat.tombstoned_at.is_some() {
            return Verdict::Invalid;
        }
        match digest_of(presented) {
            Some(digest) => self.lock().verify(&seat.id, seat.proc, &digest),
            None => Verdict::Invalid,
        }
    }

    /// The seat `presented` is the live capability of (per-request proof).
    pub fn holder(&self, presented: &str) -> Option<SeatId> {
        let digest = digest_of(presented)?;
        self.lock().holder(&digest).cloned()
    }

    /// Follow one committed registry fact (the event bus's observer): a
    /// tombstone or a seat written onto another process drops its capability,
    /// as the store did in the same transaction.
    pub fn follow(&self, event: &pij_core::model::Event) {
        match event.kind.as_str() {
            "seat.tombstone" => {
                if let Some(seat) = &event.seat {
                    self.lock().forget(seat);
                }
            }
            "seat.put" => {
                if let Ok(seat) = serde_json::from_str::<SeatDescriptor>(&event.payload) {
                    self.lock().follow(&seat);
                }
            }
            _ => {}
        }
    }

    /// Live seats held.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Nothing held.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }
}

/// Daemon start: drop expired nonces (and their unconsumed handoff files)
/// once, and rebuild the map from one read.
///
/// # Errors
/// Store failures; the boot refuses rather than serving with no capabilities.
pub async fn restore(
    registry: &dyn Registry,
    capabilities: &Capabilities,
    now_ms: u64,
) -> Result<()> {
    for spawn in registry.drop_expired_nonces(now_ms).await? {
        discard_nonce_file(&spawn);
    }
    capabilities.restore(registry.capabilities().await?);
    Ok(())
}

/// Check `presented` for `seat` (a body carrier: `/v1/activity`'s
/// `context_token`) and, on the new capability's first presentation, retire
/// the previous one (two-phase rotation's second phase), as [`present_header`]
/// does: the ONE retirement path.
///
/// # Errors
/// The retirement could not be committed: the presentation fails (fail
/// closed), and the previous capability stays valid.
pub async fn present(
    registry: &dyn Registry,
    capabilities: &Capabilities,
    seat: &SeatDescriptor,
    presented: &str,
) -> Result<Verdict> {
    let verdict = capabilities.check(seat, presented);
    if let (
        Verdict::Current {
            rotation_pending: true,
        },
        Some(digest),
    ) = (verdict, digest_of(presented))
    {
        retire_previous(registry, capabilities, &seat.id, digest).await?;
        return Ok(Verdict::Current {
            rotation_pending: false,
        });
    }
    Ok(verdict)
}

/// The seat a header-presented capability proves (current, or previous
/// during a rotation), retiring the previous one on the new capability's
/// first presentation (review D1: every presentation path retires).
///
/// # Errors
/// The retirement could not be committed: the presentation fails.
pub async fn present_header(
    registry: &dyn Registry,
    capabilities: &Capabilities,
    presented: &str,
) -> Result<Option<SeatId>> {
    let Some(digest) = digest_of(presented) else {
        return Ok(None);
    };
    let found = capabilities
        .lock()
        .verdict_of(&digest)
        .map(|(seat, verdict)| (seat.clone(), verdict));
    match found {
        Some((
            seat,
            Verdict::Current {
                rotation_pending: true,
            },
        )) => {
            retire_previous(registry, capabilities, &seat, digest).await?;
            Ok(Some(seat))
        }
        Some((seat, verdict)) if verdict.valid() => Ok(Some(seat)),
        _ => Ok(None),
    }
}

/// Commit the retirement (store-conditional on `digest` still being current),
/// THEN retire in memory, conditional on the same generation (review D2).
async fn retire_previous(
    registry: &dyn Registry,
    capabilities: &Capabilities,
    seat: &SeatId,
    digest: Digest,
) -> Result<()> {
    registry.confirm_rotation(seat, digest).await?;
    capabilities.lock().retire_previous(seat, &digest);
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use pij_core::capability::{IssueOutcome, IssueProof, IssueRefusal, NonceRecord, SpawnNonce};
    use pij_core::model::{Harness, Seq, SystemState};
    use pij_core::ports::{PutBinding, SeatFilter};

    use super::*;

    /// A registry whose rotation confirm can fail or pause, and whose seat
    /// writes can be made to meet at a barrier (two writers both arriving, or
    /// a timeout), for the plan 223 review regressions.
    pub(crate) struct Hooked {
        pub(crate) inner: Arc<dyn Registry>,
        pub(crate) fail_confirm: AtomicBool,
        /// The issuing write fails before it reaches the store.
        pub(crate) fail_issuing: AtomicBool,
        /// Signal on entering confirm, then wait for release.
        pub(crate) confirm_pause: Mutex<
            Option<(
                tokio::sync::oneshot::Sender<()>,
                tokio::sync::oneshot::Receiver<()>,
            )>,
        >,
        /// Seat writes wait here until this many have arrived (or 500 ms).
        pub(crate) barrier: Option<usize>,
        arrived: AtomicUsize,
        met: tokio::sync::Notify,
    }

    impl Hooked {
        pub(crate) fn new(inner: Arc<dyn Registry>) -> Self {
            Self {
                inner,
                fail_confirm: AtomicBool::new(false),
                fail_issuing: AtomicBool::new(false),
                confirm_pause: Mutex::new(None),
                barrier: None,
                arrived: AtomicUsize::new(0),
                met: tokio::sync::Notify::new(),
            }
        }

        pub(crate) fn with_barrier(mut self, writers: usize) -> Self {
            self.barrier = Some(writers);
            self
        }

        async fn meet(&self) {
            let Some(writers) = self.barrier else {
                return;
            };
            if self.arrived.fetch_add(1, Ordering::SeqCst) + 1 >= writers {
                self.met.notify_waiters();
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(500), self.met.notified()).await;
        }
    }

    #[async_trait]
    impl Registry for Hooked {
        async fn get(&self, seat: &SeatId) -> Result<Option<SeatDescriptor>> {
            self.inner.get(seat).await
        }
        async fn put(&self, descriptor: SeatDescriptor) -> Result<Seq> {
            self.inner.put(descriptor).await
        }
        async fn put_reporting(&self, descriptor: SeatDescriptor) -> Result<(Seq, PutBinding)> {
            self.meet().await;
            self.inner.put_reporting(descriptor).await
        }
        async fn put_reporting_keeping_parent(
            &self,
            descriptor: SeatDescriptor,
        ) -> Result<(Seq, PutBinding)> {
            self.meet().await;
            self.inner.put_reporting_keeping_parent(descriptor).await
        }
        async fn list(&self, filter: SeatFilter) -> Result<Vec<SeatDescriptor>> {
            self.inner.list(filter).await
        }
        async fn tombstone(&self, seat: &SeatId, reason: &str) -> Result<Seq> {
            self.inner.tombstone(seat, reason).await
        }
        async fn tombstone_if_unchanged(
            &self,
            expected: SeatDescriptor,
            reason: String,
        ) -> Result<Seq> {
            self.inner.tombstone_if_unchanged(expected, reason).await
        }
        async fn set_activity(
            &self,
            seat: &SeatId,
            state: SystemState,
            reason: Option<&str>,
        ) -> Result<Option<Seq>> {
            self.inner.set_activity(seat, state, reason).await
        }
        async fn insert_spawn_nonce(&self, nonce: SpawnNonce) -> Result<()> {
            self.inner.insert_spawn_nonce(nonce).await
        }
        async fn drop_expired_nonces(&self, now_ms: u64) -> Result<Vec<String>> {
            self.inner.drop_expired_nonces(now_ms).await
        }
        async fn check_issue(
            &self,
            seat: &SeatId,
            proc: Option<ProcIdentity>,
            proof: &IssueProof,
            now_ms: u64,
        ) -> Result<std::result::Result<(), IssueRefusal>> {
            self.inner.check_issue(seat, proc, proof, now_ms).await
        }
        async fn put_issuing(
            &self,
            descriptor: SeatDescriptor,
            keep_parent: bool,
            issue: CapabilityIssue,
            retiring: Vec<SeatDescriptor>,
        ) -> Result<IssueOutcome> {
            self.meet().await;
            if self.fail_issuing.load(Ordering::SeqCst) {
                return Err(PijError::Adapter {
                    adapter: "test/hooked".to_string(),
                    message: "injected issuing failure".to_string(),
                });
            }
            self.inner
                .put_issuing(descriptor, keep_parent, issue, retiring)
                .await
        }
        async fn record_spawn_pane(&self, spawn_id: &str, pane: ProcIdentity) -> Result<bool> {
            self.inner.record_spawn_pane(spawn_id, pane).await
        }
        async fn spawn_nonce(&self, spawn_id: &str) -> Result<Option<NonceRecord>> {
            self.inner.spawn_nonce(spawn_id).await
        }
        async fn confirm_rotation(&self, seat: &SeatId, digest: Digest) -> Result<bool> {
            let pause = self
                .confirm_pause
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
            if self.fail_confirm.load(Ordering::SeqCst) {
                return Err(PijError::Adapter {
                    adapter: "test/hooked".to_string(),
                    message: "injected confirm failure".to_string(),
                });
            }
            self.inner.confirm_rotation(seat, digest).await
        }
        async fn capability(&self, seat: &SeatId) -> Result<Option<CapabilityRow>> {
            self.inner.capability(seat).await
        }
        async fn capabilities(&self) -> Result<Vec<CapabilityRow>> {
            self.inner.capabilities().await
        }
    }

    /// Store a fresh nonce for `seat`'s spawn and return it, as spawn does.
    pub(crate) async fn nonce_for(
        registry: &dyn Registry,
        seat: &SeatId,
        spawn_id: &str,
        harness: Harness,
        expires_at: u64,
        pane: Option<ProcIdentity>,
    ) -> String {
        let nonce = mint().expect("nonce");
        registry
            .insert_spawn_nonce(SpawnNonce {
                spawn_id: spawn_id.to_string(),
                seat: seat.clone(),
                harness,
                digest: nonce.digest(),
                issued_at: 1,
                expires_at,
                pane,
            })
            .await
            .expect("nonce row");
        nonce.into_plaintext()
    }

    /// Issue a capability for `seat` through its spawn nonce, as registration does.
    pub(crate) async fn issue(
        registry: &dyn Registry,
        capabilities: &Capabilities,
        seat: pij_core::model::SeatDescriptor,
    ) -> String {
        let spawn = format!("spawn-{}", seat.proc.map_or(0, |proc| proc.pid));
        let nonce = nonce_for(
            registry,
            &seat.id,
            &spawn,
            seat.harness,
            u64::MAX,
            seat.proc,
        )
        .await;
        let token = mint().unwrap();
        let outcome = registry
            .put_issuing(
                seat.clone(),
                true,
                pij_core::capability::CapabilityIssue {
                    proof: pij_core::capability::IssueProof::Nonce {
                        spawn_id: spawn,
                        digest: digest_of(&nonce).unwrap(),
                    },
                    digest: token.digest(),
                    owner: None,
                    now_ms: 2,
                },
                Vec::new(),
            )
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            pij_core::capability::IssueOutcome::Issued { .. }
        ));
        capabilities.sync(
            &seat.id,
            registry.capability(&seat.id).await.unwrap().as_ref(),
        );
        token.into_plaintext()
    }
}

#[cfg(test)]
mod tests {
    use pij_core::capability::CapabilityVia;
    use pij_core::model::Harness;

    use super::*;

    fn row(seat: &str, digest: Digest, prev: Option<Digest>, pid: u32) -> CapabilityRow {
        CapabilityRow {
            seat: seat.into(),
            owner: None,
            boundary: Some(ProcIdentity { pid, proc_start: 1 }),
            session: None,
            digest,
            prev_digest: prev,
            issued_at: 1,
            via: CapabilityVia::Nonce,
        }
    }

    fn live(seat: &str, pid: u32) -> SeatDescriptor {
        let mut descriptor = SeatDescriptor::new(seat, Harness::Omp, "/abs");
        descriptor.proc = Some(ProcIdentity { pid, proc_start: 1 });
        descriptor
    }

    #[test]
    fn minted_secrets_are_independent_and_only_their_hex_digests_to_their_digest() {
        let (first, second) = (mint().unwrap(), mint().unwrap());
        assert_ne!(first.plaintext(), second.plaintext());
        assert_eq!(first.plaintext().len(), SECRET_LEN * 2);
        assert_eq!(digest_of(first.plaintext()), Some(first.digest()));
        assert_ne!(first.digest(), second.digest());
        assert_eq!(digest_of("not hex"), None);
        assert_eq!(digest_of(&"zz".repeat(SECRET_LEN)), None);
        assert_eq!(digest_of(&"+f".repeat(SECRET_LEN)), None, "hex digits only");
        assert!(!format!("{first:?}").contains(first.plaintext()));
    }

    #[test]
    fn a_capability_proves_only_its_own_live_incarnation() {
        let token = mint().unwrap();
        let capabilities = Capabilities::default();
        capabilities.restore(vec![row("pij-a", token.digest(), None, 7)]);
        let seat = live("pij-a", 7);
        assert_eq!(
            capabilities.check(&seat, token.plaintext()),
            Verdict::Current {
                rotation_pending: false
            }
        );
        assert_eq!(
            capabilities.check(&live("pij-b", 7), token.plaintext()),
            Verdict::Invalid,
            "another seat"
        );
        assert_eq!(
            capabilities.check(&live("pij-a", 8), token.plaintext()),
            Verdict::Invalid,
            "a new process is a new incarnation"
        );
        let mut retired = seat.clone();
        retired.tombstoned_at = Some(1);
        assert_eq!(
            capabilities.check(&retired, token.plaintext()),
            Verdict::Invalid
        );
        assert_eq!(
            capabilities.check(&seat, mint().unwrap().plaintext()),
            Verdict::Invalid
        );
        capabilities.sync(&seat.id, None);
        assert_eq!(
            capabilities.check(&seat, token.plaintext()),
            Verdict::Invalid
        );
        assert!(capabilities.is_empty());
    }

    fn rotate(from: Digest, to: Digest, now_ms: u64) -> CapabilityIssue {
        CapabilityIssue {
            proof: pij_core::capability::IssueProof::Token { digest: from },
            digest: to,
            owner: None,
            now_ms,
        }
    }

    async fn rotated(
        registry: &dyn Registry,
        capabilities: &Capabilities,
        seat: &SeatDescriptor,
        from: &str,
    ) -> Secret {
        let next = mint().unwrap();
        let outcome = registry
            .put_issuing(
                seat.clone(),
                true,
                rotate(digest_of(from).unwrap(), next.digest(), 3),
                Vec::new(),
            )
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            pij_core::capability::IssueOutcome::Issued { .. }
        ));
        capabilities.sync(
            &seat.id,
            registry.capability(&seat.id).await.unwrap().as_ref(),
        );
        next
    }

    /// Review D2: a presentation whose confirm is paused while another
    /// rotation (B to C) lands must not retire the NEW generation's previous
    /// capability (B): the in-memory retirement is conditional on the
    /// generation it confirmed.
    #[tokio::test]
    async fn a_paused_confirm_never_retires_the_next_generations_previous() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let inner: std::sync::Arc<dyn Registry> =
            std::sync::Arc::new(pij_testkit::fakes::FakeRegistry::new());
        let registry = test_support::Hooked::new(std::sync::Arc::clone(&inner));
        *registry.confirm_pause.lock().unwrap() = Some((entered_tx, release_rx));
        let capabilities = Capabilities::default();
        let seat = live("pij-a", 7);
        let a = test_support::issue(&registry, &capabilities, seat.clone()).await;
        let b = rotated(&registry, &capabilities, &seat, &a).await;
        let (verdict, c) = tokio::join!(
            present(&registry, &capabilities, &seat, b.plaintext()),
            async {
                entered_rx.await.unwrap();
                let c = rotated(inner.as_ref(), &capabilities, &seat, b.plaintext()).await;
                release_tx.send(()).unwrap();
                c
            }
        );
        assert!(verdict.unwrap().valid());
        assert_eq!(capabilities.check(&seat, b.plaintext()), Verdict::Previous);
        assert_eq!(
            capabilities.check(&seat, c.plaintext()),
            Verdict::Current {
                rotation_pending: true
            }
        );
        assert_eq!(
            inner
                .capability(&seat.id)
                .await
                .unwrap()
                .and_then(|row| row.prev_digest),
            Some(b.digest())
        );
    }

    /// Review S183-002: the retirement commits before a presentation
    /// succeeds; a failed write fails the presentation and the previous
    /// capability stays valid, through either carrier.
    #[tokio::test]
    async fn a_failed_retirement_fails_the_presentation() {
        let inner: std::sync::Arc<dyn Registry> =
            std::sync::Arc::new(pij_testkit::fakes::FakeRegistry::new());
        let registry = test_support::Hooked::new(std::sync::Arc::clone(&inner));
        let capabilities = Capabilities::default();
        let seat = live("pij-a", 7);
        let a = test_support::issue(&registry, &capabilities, seat.clone()).await;
        let b = rotated(&registry, &capabilities, &seat, &a).await;
        registry
            .fail_confirm
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            present(&registry, &capabilities, &seat, b.plaintext())
                .await
                .is_err()
        );
        assert!(
            present_header(&registry, &capabilities, b.plaintext())
                .await
                .is_err()
        );
        assert_eq!(capabilities.check(&seat, &a), Verdict::Previous);
        registry
            .fail_confirm
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            present_header(&registry, &capabilities, b.plaintext())
                .await
                .unwrap(),
            Some(seat.id.clone())
        );
        assert_eq!(capabilities.check(&seat, &a), Verdict::Invalid);
        assert_eq!(
            inner
                .capability(&seat.id)
                .await
                .unwrap()
                .and_then(|row| row.prev_digest),
            None
        );
    }

    #[test]
    fn rotation_keeps_the_previous_capability_until_the_new_one_is_presented() {
        let (old, new) = (mint().unwrap(), mint().unwrap());
        let mut map = DigestMap::rebuild([row("pij-a", new.digest(), Some(old.digest()), 7)]);
        let seat = SeatId::from("pij-a");
        let proc = Some(ProcIdentity {
            pid: 7,
            proc_start: 1,
        });
        assert_eq!(map.verify(&seat, proc, &old.digest()), Verdict::Previous);
        assert_eq!(
            map.verify(&seat, proc, &new.digest()),
            Verdict::Current {
                rotation_pending: true
            }
        );
        assert!(
            !map.retire_previous(&seat, &old.digest()),
            "not the current one"
        );
        assert!(map.retire_previous(&seat, &new.digest()));
        assert_eq!(map.verify(&seat, proc, &old.digest()), Verdict::Invalid);
        assert_eq!(map.seat_of(&old.digest()), None);
        assert_eq!(
            map.verify(&seat, proc, &new.digest()),
            Verdict::Current {
                rotation_pending: false
            }
        );
        assert_eq!(map.len(), 1);
    }
}
