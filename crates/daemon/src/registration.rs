use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pij_core::error::PijError;
use pij_core::model::{Event, Harness, ProcIdentity, SeatDescriptor, SeatId};
use pij_core::names::memorable_pij_id_candidates;
use pij_core::ports::{LivenessPort, PutBinding, Registry, SeatFilter, TmuxPort};
use pij_core::wire;
use pij_harnesses::ensure_claude_inbound_accept;

use crate::events::EventBus;
use crate::http::Registration;
use crate::http::role::RoleService;

#[derive(Debug)]
pub(crate) enum RegistrationError {
    Refused(String),
    Retryable(String),
    NativeSessionHold(SeatId),
    Runtime(PijError),
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) | Self::Retryable(reason) => f.write_str(reason),
            Self::NativeSessionHold(owner) => write!(
                f,
                "native Copilot registration held: pane owned by seat `{owner}`; awaiting resumed session"
            ),
            Self::Runtime(error) => error.fmt(f),
        }
    }
}

#[derive(Clone)]
pub(crate) struct RegistrationService {
    registry: Arc<dyn Registry>,
    liveness: Arc<dyn LivenessPort>,
    event_bus: Arc<EventBus>,
    roles: Arc<RoleService>,
    claude_homes: Vec<PathBuf>,
    registration_lock: Arc<tokio::sync::Mutex<()>>,
}

struct NativeHandoff {
    dead: Vec<SeatDescriptor>,
    prebind: Option<SeatDescriptor>,
}

enum RegistrationMode<'a> {
    Ordinary,
    /// Operator reclaim (plan 156 AC6): the adopt route has already proved the
    /// caller's authority and the target's death, so a retired row may be taken
    /// without a continuity witness.
    Reclaim,
    NativeExtension(&'a NativeHandoff),
    Paneless(&'a [ProcIdentity]),
}

const HOST_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);

impl RegistrationService {
    pub(crate) fn new(
        registry: Arc<dyn Registry>,
        liveness: Arc<dyn LivenessPort>,
        event_bus: Arc<EventBus>,
        claude_homes: Vec<PathBuf>,
        roles: Arc<RoleService>,
    ) -> Self {
        Self {
            registry,
            liveness,
            event_bus,
            claude_homes,
            roles,
            registration_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn with_native_lock(mut self, lock: Arc<tokio::sync::Mutex<()>>) -> Self {
        self.registration_lock = lock;
        self
    }

    pub(crate) async fn register(
        &self,
        claim: Registration,
    ) -> std::result::Result<SeatDescriptor, RegistrationError> {
        self.register_with_harness_session(claim, None)
            .await
            .map(|(descriptor, _)| descriptor)
    }

    pub(crate) async fn register_with_harness_session(
        &self,
        claim: Registration,
        harness_session: Option<String>,
    ) -> std::result::Result<(SeatDescriptor, PutBinding), RegistrationError> {
        if claim.id.is_empty()
            && matches!(
                Harness::parse(&claim.harness),
                Some(Harness::Claude | Harness::Copilot | Harness::Codex)
            )
        {
            return self
                .register_paneless_with_observer(claim, harness_session, observe_native_process)
                .await;
        }
        self.register_inner(claim, harness_session, RegistrationMode::Ordinary)
            .await
    }

    /// Operator reclaim: see [`RegistrationMode::Reclaim`]. Only the adopt
    /// route's `--reclaim` arm calls this, after its guards.
    pub(crate) async fn reclaim(
        &self,
        claim: Registration,
    ) -> std::result::Result<SeatDescriptor, RegistrationError> {
        self.register_inner(claim, None, RegistrationMode::Reclaim)
            .await
            .map(|(descriptor, _)| descriptor)
    }

    async fn register_paneless_with_observer<F, Fut>(
        &self,
        mut claim: Registration,
        harness_session: Option<String>,
        mut observe: F,
    ) -> std::result::Result<(SeatDescriptor, PutBinding), RegistrationError>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = std::result::Result<Option<NativeProcess>, RegistrationError>>,
    {
        let harness = Harness::parse(&claim.harness)
            .filter(|harness| {
                matches!(harness, Harness::Claude | Harness::Copilot | Harness::Codex)
            })
            .ok_or_else(|| paneless_refusal("requires Claude, Copilot, or Codex"))?;
        if !claim.id.is_empty()
            || claim.pane.is_some()
            || claim.relay
            || claim.supersedes.is_some()
            || claim.spawn_id.is_some()
        {
            return Err(paneless_refusal(
                "requires an empty id, no pane/spawn/supersedes, and relay false",
            ));
        }
        let session = harness_session
            .as_deref()
            .filter(|session| {
                !session.is_empty()
                    && !session
                        .chars()
                        .any(|ch| ch.is_whitespace() || ch.is_control())
            })
            .ok_or_else(|| paneless_refusal("requires an exact nonempty native session"))?;
        let caller = match (claim.pid, claim.proc_start) {
            (Some(pid), Some(proc_start)) if pid > 0 && proc_start > 0 => {
                ProcIdentity { pid, proc_start }
            }
            _ => {
                return Err(paneless_refusal(
                    "requires a complete nonzero pid/proc_start tuple",
                ));
            }
        };
        let ancestry = tokio::time::timeout(HOST_OBSERVATION_TIMEOUT, async {
            self.require_process(caller).await?;
            let mut ancestry = Vec::with_capacity(8);
            let mut current = caller;
            loop {
                if ancestry.len() == 32 || ancestry.contains(&current) {
                    return Err(paneless_refusal(
                        "host ancestry is cyclic or exceeds 32 processes",
                    ));
                }
                let process = observe(current.pid)
                    .await?
                    .ok_or_else(|| paneless_refusal("a caller/ancestor process disappeared"))?;
                ancestry.push(current);
                if process.is_external_host(harness) {
                    if !process.matches_external_session(harness, session) {
                        return Err(paneless_refusal(
                            "observed harness session differs from the claim",
                        ));
                    }
                    break;
                }
                // A nested harness must not inherit an outer harness's session.
                // Stop at the nearest recognized external host, even if the
                // requested harness appears farther up the process tree.
                if [Harness::Claude, Harness::Copilot, Harness::Codex]
                    .into_iter()
                    .any(|candidate| candidate != harness && process.is_external_host(candidate))
                {
                    return Err(paneless_refusal(
                        "nearest external harness differs from the claim",
                    ));
                }
                if process.parent == 0 {
                    return Err(paneless_refusal("no matching external harness ancestor"));
                }
                let parent = self.observe_identity(process.parent).await?;
                if parent.proc_start > current.proc_start {
                    return Err(paneless_refusal(
                        "an ancestor pid was reused after its child started",
                    ));
                }
                current = parent;
            }
            for identity in &ancestry {
                self.require_process(*identity).await?;
            }
            Ok(ancestry)
        })
        .await
        .map_err(|_| {
            RegistrationError::Retryable(format!(
                "host observation for pid {} timed out after {}ms",
                caller.pid,
                HOST_OBSERVATION_TIMEOUT.as_millis(),
            ))
        })??;
        let host = ancestry.last().expect("observed external harness ancestor");
        claim.pid = Some(host.pid);
        claim.proc_start = Some(host.proc_start);
        self.register_inner(
            claim,
            harness_session,
            RegistrationMode::Paneless(&ancestry),
        )
        .await
    }

    /// Attest extension delivery only after observing the actual Copilot host.
    pub(crate) async fn register_native(
        &self,
        claim: Registration,
        harness_session: Option<String>,
        tmux: &dyn TmuxPort,
    ) -> std::result::Result<(SeatDescriptor, PutBinding), RegistrationError> {
        self.register_native_with_observer(claim, harness_session, tmux, observe_native_process)
            .await
    }

    async fn register_native_with_observer<F, Fut>(
        &self,
        claim: Registration,
        harness_session: Option<String>,
        tmux: &dyn TmuxPort,
        mut observe: F,
    ) -> std::result::Result<(SeatDescriptor, PutBinding), RegistrationError>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = std::result::Result<Option<NativeProcess>, RegistrationError>>,
    {
        if claim.harness != Harness::Copilot.as_str() || claim.relay {
            return Err(native_refusal("requires harness copilot and relay false"));
        }
        if harness_session
            .as_deref()
            .is_none_or(|session| session.trim().is_empty())
        {
            return Err(native_refusal("requires a runtime native session"));
        }
        let identity = match (claim.pid, claim.proc_start) {
            (Some(pid), Some(proc_start)) if pid > 0 && proc_start > 0 => {
                ProcIdentity { pid, proc_start }
            }
            _ => {
                return Err(native_refusal(
                    "requires a complete nonzero pid/proc_start tuple",
                ));
            }
        };
        tokio::time::timeout(
            HOST_OBSERVATION_TIMEOUT,
            self.verify_native_host(identity, claim.pane.as_deref(), tmux, &mut observe),
        )
        .await
        .map_err(|_| {
            RegistrationError::Retryable(format!(
                "host observation for pid {} timed out after {}ms",
                identity.pid,
                HOST_OBSERVATION_TIMEOUT.as_millis(),
            ))
        })??;
        let dead_native = self
            .observe_dead_native_owners(
                &claim,
                harness_session
                    .as_deref()
                    .expect("validated native session"),
                identity,
            )
            .await?;
        self.register_inner(
            claim,
            harness_session,
            RegistrationMode::NativeExtension(&dead_native),
        )
        .await
    }

    /// A dead exact incarnation cannot come back to life. Collect that witness
    /// outside global exclusion, then bind it to unchanged ownership at commit.
    async fn observe_dead_native_owners(
        &self,
        claim: &Registration,
        session: &str,
        identity: ProcIdentity,
    ) -> std::result::Result<NativeHandoff, RegistrationError> {
        let seats = self
            .registry
            .list(SeatFilter::default())
            .await
            .map_err(RegistrationError::Runtime)?;
        let prebind = native_prebind(claim, session, &seats).cloned();
        let mut dead = Vec::new();
        for seat in seats {
            if seat.harness != Harness::Copilot
                || !seat.native_extension_delivery
                || seat.tombstoned_at.is_some()
                || seat.relay
                || seat.harness_session.as_deref() == Some(session)
            {
                continue;
            }
            let Some(previous) = seat.proc.filter(|proc| proc.pid > 0 && proc.proc_start > 0)
            else {
                continue;
            };
            if previous == identity {
                continue;
            }
            let stale_pane_owner = seat.id.as_str() != claim.id
                && claim.pane.is_some()
                && seat.pane == claim.pane
                && seat
                    .harness_session
                    .as_deref()
                    .is_some_and(|old| !old.is_empty() && old != session);
            if stale_pane_owner
                && self
                    .liveness
                    .proc_start(previous.pid)
                    .await
                    .map_err(RegistrationError::Runtime)?
                    != Some(previous.proc_start)
            {
                dead.push(seat);
            }
        }
        Ok(NativeHandoff { dead, prebind })
    }

    async fn verify_native_host<F, Fut>(
        &self,
        identity: ProcIdentity,
        pane: Option<&str>,
        tmux: &dyn TmuxPort,
        observe: &mut F,
    ) -> std::result::Result<(), RegistrationError>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = std::result::Result<Option<NativeProcess>, RegistrationError>>,
    {
        self.require_process(identity).await?;
        let pane_process = match pane {
            Some(pane) => Some(
                tmux.pane_process(pane)
                    .await
                    .map_err(RegistrationError::Runtime)?
                    .ok_or_else(|| native_refusal("the claimed pane has no observed process"))?,
            ),
            None => None,
        };
        let pane_identity = if let Some(process) = &pane_process {
            Some(self.observe_identity(process.pid).await?)
        } else {
            None
        };
        let mut ancestry = Vec::with_capacity(8);
        let mut current = identity;
        loop {
            if ancestry.len() == 32 || ancestry.contains(&current) {
                return Err(native_refusal(
                    "pane ancestry is cyclic or exceeds 32 processes",
                ));
            }
            let process = observe(current.pid)
                .await?
                .ok_or_else(|| native_refusal("a host/ancestor process disappeared"))?;
            if current == identity && !process.is_copilot_host() {
                let basename = Path::new(&process.executable)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<unknown>");
                return Err(native_refusal(format!(
                    "claimed pid is not an actual Copilot host executable; observed basename={basename:?}, replaced={}",
                    process.executable_replaced
                )));
            }
            ancestry.push(current);
            if pane_identity.is_none_or(|pane| pane == current) {
                break;
            }
            if process.parent == 0 {
                return Err(native_refusal(
                    "host ancestry does not reach the exact pane process",
                ));
            }
            let parent = self.observe_identity(process.parent).await?;
            if parent.proc_start > current.proc_start {
                return Err(native_refusal(
                    "an ancestor pid was reused after its child started",
                ));
            }
            current = parent;
        }
        // Sandwich the entire observation, including the pane and every ancestor,
        // with start-time checks. A PID by itself is never an incarnation witness.
        for process in ancestry {
            self.require_process(process).await?;
        }
        if let (Some(pane), Some(expected)) = (pane, pane_process)
            && tmux
                .pane_process(pane)
                .await
                .map_err(RegistrationError::Runtime)?
                .is_none_or(|observed| observed.pid != expected.pid)
        {
            return Err(native_refusal(
                "pane process changed during host observation",
            ));
        }
        self.require_process(identity).await
    }

    async fn observe_identity(
        &self,
        pid: u32,
    ) -> std::result::Result<ProcIdentity, RegistrationError> {
        let proc_start = self
            .liveness
            .proc_start(pid)
            .await
            .map_err(RegistrationError::Runtime)?
            .filter(|start| pid > 0 && *start > 0)
            .ok_or_else(|| native_refusal("host/ancestor process has no observed start"))?;
        Ok(ProcIdentity { pid, proc_start })
    }

    async fn require_process(
        &self,
        expected: ProcIdentity,
    ) -> std::result::Result<(), RegistrationError> {
        if self.observe_identity(expected.pid).await? != expected {
            return Err(native_refusal(
                "process start changed during host observation",
            ));
        }
        Ok(())
    }

    async fn register_inner(
        &self,
        mut claim: Registration,
        harness_session: Option<String>,
        mode: RegistrationMode<'_>,
    ) -> std::result::Result<(SeatDescriptor, PutBinding), RegistrationError> {
        if claim
            .role
            .as_deref()
            .is_some_and(|role| role.trim().is_empty())
        {
            return Err(RegistrationError::Refused(
                "role must be a nonempty string".to_string(),
            ));
        }
        let reclaim = matches!(mode, RegistrationMode::Reclaim);
        let (native, paneless, handoff) = match mode {
            RegistrationMode::Ordinary | RegistrationMode::Reclaim => (false, None, None),
            RegistrationMode::NativeExtension(proof) => (true, None, Some(proof)),
            RegistrationMode::Paneless(ancestry) => (false, Some(ancestry), None),
        };
        let dead_native = handoff.map_or(&[][..], |proof| proof.dead.as_slice());
        let harness_session = harness_session.filter(|session| !session.trim().is_empty());
        let proc = match (claim.pid, claim.proc_start) {
            (Some(pid), Some(proc_start)) => {
                let observed = self
                    .liveness
                    .proc_start(pid)
                    .await
                    .map_err(RegistrationError::Runtime)?;
                if observed != Some(proc_start) {
                    return Err(RegistrationError::Refused(format!(
                        "the payload is a claim, but the daemon observed process {pid} start as {observed:?}, not {proc_start}"
                    )));
                }
                Some(ProcIdentity { pid, proc_start })
            }
            (None, None) => None,
            _ => {
                return Err(RegistrationError::Refused(
                    "pid and proc_start must travel together or not at all".to_string(),
                ));
            }
        };

        // Process/ancestry evidence is collected without excluding inbox readers.
        // Re-read the entire roster under shared mutation exclusion: two different
        // seat IDs can still contend for the same process, session, or pane.
        let _guard = self.registration_lock.lock().await;
        if let Some(ancestry) = paneless {
            for identity in ancestry {
                self.require_process(*identity).await?;
            }
        }
        let seats = self
            .registry
            .list(SeatFilter::default())
            .await
            .map_err(RegistrationError::Runtime)?;
        if dead_native
            .iter()
            .chain(handoff.and_then(|proof| proof.prebind.as_ref()))
            .any(|observed| {
                seats
                    .iter()
                    .find(|seat| seat.id == observed.id)
                    .is_none_or(|current| !same_native_owner(observed, current))
            })
        {
            return Err(RegistrationError::Retryable(
                "native owner changed during death observation; retry registration".into(),
            ));
        }
        if native {
            let session = harness_session
                .as_deref()
                .expect("validated native session");
            // An explicit address may not relabel a different conversation, even
            // when another row happens to match the claimed session.
            if let Some(seat) = seats.iter().find(|seat| {
                seat.id.as_str() == claim.id
                    && seat
                        .harness_session
                        .as_deref()
                        .is_some_and(|old| old != session)
            }) {
                if seat.tombstoned_at.is_some() {
                    return Err(retired_refusal(seat, &claim.harness));
                }
                return Err(native_refusal(
                    "seat id belongs to a different native session",
                ));
            }
            if seats
                .iter()
                .filter(|seat| {
                    seat.harness == Harness::Copilot
                        && seat.harness_session.as_deref() == Some(session)
                        && seat.tombstoned_at.is_none()
                        && handoff
                            .and_then(|proof| proof.prebind.as_ref())
                            .is_none_or(|prebind| prebind.id != seat.id)
                })
                .count()
                > 1
            {
                return Err(RegistrationError::Retryable(
                    "native session has multiple Pij addresses; refresh the roster".into(),
                ));
            }
            if let Some(target) = registration_target(&claim, true, Some(session), &seats) {
                if claim.id.is_empty()
                    && target.native_extension_delivery
                    && target.proc == proc
                    && target.pane == claim.pane
                {
                    claim.supersedes = None;
                }
                if claim.id != target.id.as_str() {
                    claim.id = target.id.to_string();
                }
                // --resume can first register a temporary bootstrap conversation.
                // Only its verified current host may retire that predecessor.
                if target.harness_session.as_deref() == Some(session) && claim.supersedes.is_none()
                {
                    claim.supersedes = seats
                        .iter()
                        .find(|seat| {
                            seat.id != target.id
                                && seat.harness == Harness::Copilot
                                && !seat.relay
                                && seat.tombstoned_at.is_none()
                                && seat.proc == proc
                                && seat.pane == claim.pane
                                && seat
                                    .harness_session
                                    .as_deref()
                                    .is_some_and(|old| old != session)
                        })
                        .map(|seat| seat.id.clone());
                }
            } else if claim.id.is_empty() {
                claim.id = memorable_pij_id_candidates(&format!("copilot\0{session}"))
                    .find(|candidate| seats.iter().all(|seat| seat.id != *candidate))
                    .ok_or_else(|| native_refusal("memorable Pij ID space is exhausted"))?
                    .to_string();
            }
        }
        if paneless.is_some() {
            let session = harness_session
                .as_deref()
                .expect("validated paneless session");
            claim.id = paneless_target(&claim, session, proc, &seats)?;
        }
        let claimed_id = SeatId::from(claim.id.clone());
        let existing = registration_target(&claim, native, harness_session.as_deref(), &seats);
        if let Some(seat) = existing
            .filter(|seat| seat.tombstoned_at.is_some() && seat.harness.as_str() != claim.harness)
        {
            // Harness identity gates every continuity rung, including native
            // and paneless claims, before their stronger ownership checks.
            return Err(retired_refusal(seat, &claim.harness));
        }
        // Explicit, verified spawn intent owns the new id and parent. Its saved
        // conversation may be retired, but neither address's mail is moved.
        let spawn_predecessor = handoff
            .and_then(|proof| proof.prebind.as_ref())
            .filter(|prebind| existing.is_some_and(|seat| seat.id == prebind.id))
            .and_then(|prebind| {
                seats.iter().find(|seat| {
                    seat.id != prebind.id
                        && seat.harness == Harness::Copilot
                        && seat.harness_session == harness_session
                        && seat.tombstoned_at.is_none()
                        && seat.proc.is_some()
                        && !seat.relay
                        && seat.machine.is_none()
                })
            });
        // Native-only ownership is a harness invariant, not a live-process flag.
        // Relabeling a prebind or retired row would bypass inbox tuple checks.
        if existing.is_some_and(|seat| seat.harness == Harness::Copilot)
            && claim.harness != Harness::Copilot.as_str()
        {
            return Err(native_refusal(
                "an existing Copilot seat cannot change harness",
            ));
        }
        let mut native_hold = None;
        if native {
            match check_native_roster(
                &claim,
                harness_session.as_deref(),
                proc,
                &seats,
                existing,
                dead_native,
                spawn_predecessor,
            ) {
                Ok(()) => {}
                Err(error @ RegistrationError::NativeSessionHold(_)) => native_hold = Some(error),
                Err(error) => return Err(error),
            }
        } else if let Some(owner) = seats.iter().find(|seat| {
            seat.harness == Harness::Copilot
                && seat.proc.is_some()
                && seat.tombstoned_at.is_none()
                && (existing.is_some_and(|existing| existing.id == seat.id)
                    || claim.supersedes.as_ref() == Some(&seat.id))
        }) {
            // Copilot remains native-only after capability withdrawal. Clearing
            // its flag cannot authorize a later legacy relabel or supersedes.
            if claim.supersedes.is_some()
                || claim.harness != Harness::Copilot.as_str()
                || proc != owner.proc
                || claim.pane != owner.pane
                || owner
                    .harness_session
                    .as_ref()
                    .zip(harness_session.as_ref())
                    .is_some_and(|(previous, claimed)| previous != claimed)
                || claim.relay
                || claim
                    .spawn_id
                    .as_ref()
                    .is_some_and(|spawn| Some(spawn) != owner.spawn_id.as_ref())
            {
                return Err(native_refusal(
                    "ordinary registration cannot change or supersede an active Copilot incarnation",
                ));
            }
        }

        let predecessor = match (claim.supersedes.as_ref(), proc) {
            (Some(previous), Some(identity)) => {
                match seats.iter().find(|seat| &seat.id == previous) {
                    Some(seat) if seat.proc == Some(identity) => Some(seat.clone()),
                    Some(_) => {
                        return Err(RegistrationError::Refused(format!(
                            "seat `{previous}` does not share this process, so it cannot be superseded by `{}` — a claim may only supersede itself",
                            claim.id
                        )));
                    }
                    None => {
                        return Err(RegistrationError::Refused(format!(
                            "seat `{previous}` is not in the roster, so `{}` cannot supersede it",
                            claim.id
                        )));
                    }
                }
            }
            (Some(_), None) => {
                return Err(RegistrationError::Refused(
                    "an unbound claim cannot supersede a seat: without a process identity there is nothing that proves the successor is the same session"
                        .to_string(),
                ));
            }
            (None, _) => None,
        };

        let collision = proc.and_then(|identity| {
            seats.iter().find(|seat| {
                seat.proc == Some(identity)
                    && (!(native || paneless.is_some()) || seat.tombstoned_at.is_none())
                    && seat.id != claimed_id
                    && predecessor.as_ref().is_none_or(|prev| prev.id != seat.id)
                    && spawn_predecessor.is_none_or(|previous| previous.id != seat.id)
            })
        });
        let bound = proc.is_some();
        match crate::admission::from_registration(
            &claim.id,
            Some(&claim.harness),
            Some(&claim.folder),
            bound,
            collision.map(|seat| seat.id.as_str()),
        ) {
            pij_core::admission::Admission::Admit => {}
            pij_core::admission::Admission::Refuse(reason) => {
                return Err(RegistrationError::Refused(reason.to_string()));
            }
        }
        let Some(harness) = Harness::parse(&claim.harness) else {
            return Err(RegistrationError::Refused(
                "unrecognised harness after admission".to_string(),
            ));
        };
        // A pane hold must not turn an otherwise-invalid claim into a retry.
        // All admission checks above remain read-only; no held session is stored.
        if let Some(error) = native_hold {
            return Err(error);
        }
        let retired = existing.filter(|seat| seat.tombstoned_at.is_some());
        if let Some(seat) = retired {
            let continuous = seat
                .harness_session
                .as_ref()
                .zip(harness_session.as_ref())
                .is_some_and(|(old, claimed)| old == claimed)
                || seat
                    .spawn_id
                    .as_deref()
                    .zip(claim.spawn_id.as_deref())
                    .is_some_and(|(old, claimed)| !old.is_empty() && old == claimed)
                || seat.proc.is_some_and(|old| Some(old) == proc)
                || reclaim;
            if !continuous {
                return Err(retired_refusal(seat, &claim.harness));
            }
        }
        let resuming = retired.is_some()
            || native
                && existing.is_some_and(|seat| {
                    seat.harness_session == harness_session
                        && (seat.proc != proc
                            || seat.pane != claim.pane
                            || seat.tombstoned_at.is_some())
                });
        let mut descriptor = existing
            .cloned()
            .unwrap_or_else(|| SeatDescriptor::new(claimed_id, harness, claim.folder.clone()));
        let binds_spawned_incarnation = descriptor.proc.is_none()
            && descriptor
                .spawn_id
                .as_deref()
                .zip(claim.spawn_id.as_deref())
                .is_some_and(|(launched, claimed)| !launched.is_empty() && launched == claimed);
        let requested_model = descriptor.model.clone();
        let model_matches = requested_model_matches(
            requested_model.as_deref(),
            claim.actual_model.as_deref(),
            claim.actual_model_observed,
        );
        if !resuming {
            descriptor.harness = harness;
        }
        // A reclaim carries forward no conversation it did not derive: the
        // target's recorded one is exactly the evidence that went stale.
        if reclaim {
            descriptor.harness_session = None;
        }
        if let Some(harness_session) = harness_session {
            descriptor.harness_session = Some(harness_session);
        }
        if descriptor.harness == Harness::Claude
            && descriptor.cross_session_inbound_accept.is_none()
        {
            descriptor.cross_session_inbound_accept = Some(true);
        }
        descriptor.folder = claim.folder;
        // Build identity describes this registration, never a prior incarnation.
        descriptor.extension_build = claim.extension_build;
        descriptor.extension_path = claim.extension_path;
        descriptor.pane = claim.pane;
        // A new process has not started a turn yet: `working` belonged to the
        // previous incarnation and must not outlive it (plan 158 review MEDIUM-3).
        if descriptor.proc != proc && descriptor.state == pij_core::model::SystemState::Working {
            descriptor.state = pij_core::model::SystemState::Idle;
        }
        descriptor.proc = proc;
        if resuming {
            descriptor.tombstoned_at = None;
            descriptor.tombstone_reason = None;
        }
        if descriptor.spawn_id.is_none() {
            descriptor.spawn_id = claim.spawn_id;
        }
        if binds_spawned_incarnation && claim.actual_model_observed {
            descriptor.model = claim.actual_model.clone();
        } else if descriptor.model.is_none() {
            descriptor.model = claim.model;
        }
        if descriptor.provider.is_none() {
            descriptor.provider = claim.provider;
        }
        if descriptor.effort.is_none() {
            descriptor.effort = claim.effort;
        }
        if let Some(parent) = claim
            .parent
            .filter(|_| !resuming && (!native || existing.is_none()))
        {
            descriptor.parent = Some(parent);
        }
        descriptor.relay = claim.relay;
        // Ordinary registration preserves only an unchanged current attested owner.
        // Native-capability grants, transfers and revivals require verified native observation.
        // Compare before assignment: the clone still carries the incumbent's flag and tombstone.
        descriptor.native_extension_delivery = native
            || existing.is_some_and(|owner| {
                owner.native_extension_delivery
                    && owner.tombstoned_at.is_none()
                    && same_native_owner(owner, &descriptor)
            });

        if paneless.is_some() && existing == Some(&descriptor) {
            let role = self
                .roles
                .read_role(&descriptor.id)
                .await
                .map_err(RegistrationError::Runtime)?;
            if claim
                .role
                .as_ref()
                .is_none_or(|claimed| role.as_ref() == Some(claimed))
            {
                descriptor.role = role;
                return Ok((
                    descriptor,
                    PutBinding {
                        inserted: false,
                        previous_proc: proc,
                    },
                ));
            }
        }
        if harness == Harness::Claude {
            for report in ensure_claude_inbound_accept(&self.claude_homes) {
                if let Some(error) = report.error.as_deref() {
                    eprintln!("pij-rs claude inbound: {}: {error}", report.home.display());
                    continue;
                }
                if report.changed {
                    self.event_bus
                        .publish(Event {
                            seq: None,
                            v: wire::EVENT_VERSION,
                            at: system_time_ms()?,
                            kind: "config.claude-inbound-ensured".to_string(),
                            seat: Some(descriptor.id.clone()),
                            payload: serde_json::json!({
                                "home": report.home,
                                "before": report.before,
                                "after": report.after,
                            })
                            .to_string(),
                        })
                        .await
                        .map_err(RegistrationError::Runtime)?;
                }
            }
        }
        // Retirement authority is either an observed dead resource owner or
        // explicit verified spawn intent for this same native conversation.
        // The filters are brakes: removing them can only broaden retirement.
        for observed in dead_native
            .iter()
            .chain(spawn_predecessor)
            .filter(|seat| seat.id != descriptor.id)
        {
            let mut retired = seats
                .iter()
                .find(|seat| seat.id == observed.id)
                .expect("revalidated native owner")
                .clone();
            retired.tombstoned_at = Some(system_time_ms()?);
            retired.tombstone_reason = Some(format!(
                "native registration released its previous binding to `{}`; queued work retains its original address",
                descriptor.id
            ));
            retired.native_extension_delivery = false;
            self.registry
                .put(retired)
                .await
                .map_err(RegistrationError::Runtime)?;
            if spawn_predecessor.is_some_and(|previous| previous.id == observed.id) {
                self.event_bus
                    .publish(Event {
                        seq: None,
                        v: wire::EVENT_VERSION,
                        at: system_time_ms()?,
                        kind: "seat.native-superseded".into(),
                        seat: Some(observed.id.clone()),
                        payload: serde_json::json!({
                            "successor": descriptor.id,
                            "spawn_id": descriptor.spawn_id,
                            "reason": "native-spawn-prebind",
                        })
                        .to_string(),
                    })
                    .await
                    .map_err(RegistrationError::Runtime)?;
            }
        }

        if let Some(mut previous) = predecessor
            && previous.tombstoned_at.is_none()
        {
            previous.tombstoned_at = Some(system_time_ms()?);
            previous.tombstone_reason = Some(format!(
                "superseded by `{}` at a native session boundary (same process)",
                claim.id
            ));
            previous.native_extension_delivery = false;
            self.registry
                .put(previous)
                .await
                .map_err(RegistrationError::Runtime)?;
        }
        let (_, binding) = self
            .registry
            .put_reporting(descriptor.clone())
            .await
            .map_err(RegistrationError::Runtime)?;
        if !native && let Some(previous) = retired {
            self.event_bus
                .publish(Event {
                    seq: None,
                    v: wire::EVENT_VERSION,
                    at: system_time_ms()?,
                    kind: "seat.resumed".into(),
                    seat: Some(descriptor.id.clone()),
                    payload: serde_json::json!({
                        "prior_reason": previous.tombstone_reason,
                        "old_proc": previous.proc, "new_proc": descriptor.proc,
                        "old_pane": previous.pane, "new_pane": descriptor.pane,
                    })
                    .to_string(),
                })
                .await
                .map_err(RegistrationError::Runtime)?;
        }
        if native && resuming {
            let previous = existing.expect("resume has an existing seat");
            self.event_bus
                .publish(Event {
                    seq: None,
                    v: wire::EVENT_VERSION,
                    at: system_time_ms()?,
                    kind: "seat.native-resumed".into(),
                    seat: Some(descriptor.id.clone()),
                    payload: serde_json::json!({
                        "old_proc": previous.proc, "new_proc": descriptor.proc,
                        "old_pane": previous.pane, "new_pane": descriptor.pane,
                        "reason": "native-session-resume",
                        "prior_reason": previous.tombstone_reason,
                    })
                    .to_string(),
                })
                .await
                .map_err(RegistrationError::Runtime)?;
        }
        if let Some(role) = claim.role.filter(|_| !resuming) {
            self.roles.assert_role(&descriptor.id, &descriptor.id, Some(role)).await
                .map_err(|error| RegistrationError::Runtime(PijError::Adapter {
                    adapter: "daemon/registration".to_string(),
                    message: format!("E-RS-PARTIAL registration for {} persisted but role assertion failed: {error}", descriptor.id),
                }))?;
        }
        descriptor.role = self
            .roles
            .read_role(&descriptor.id)
            .await
            .map_err(RegistrationError::Runtime)?;
        if binds_spawned_incarnation && model_matches {
            self.event_bus
                .publish(Event {
                    seq: None,
                    v: wire::EVENT_VERSION,
                    at: system_time_ms()?,
                    kind: "spawn.bound".to_string(),
                    seat: Some(descriptor.id.clone()),
                    payload: serde_json::json!({
                        "spawn_id": descriptor.spawn_id,
                        "pane": descriptor.pane,
                        "pid": descriptor.proc.map(|identity| identity.pid),
                    })
                    .to_string(),
                })
                .await
                .map_err(RegistrationError::Runtime)?;
        }
        Ok((descriptor, binding))
    }
}

fn retired_refusal(seat: &SeatDescriptor, claimed_harness: &str) -> RegistrationError {
    let mut reason = format!(
        "seat {} is retired ({}, {}); a different session may not take it",
        seat.id,
        seat.tombstone_reason.as_deref().unwrap_or("unknown"),
        seat.tombstoned_at.expect("retired seat"),
    );
    if seat.harness.as_str() != claimed_harness {
        reason.push_str("; harness mismatch: row ");
        reason.push_str(seat.harness.as_str());
        reason.push_str(", claim ");
        reason.push_str(claimed_harness);
    }
    RegistrationError::Refused(reason)
}

fn native_refusal(reason: impl std::fmt::Display) -> RegistrationError {
    RegistrationError::Refused(format!("native Copilot registration refused: {reason}"))
}

fn paneless_refusal(reason: impl std::fmt::Display) -> RegistrationError {
    RegistrationError::Refused(format!("paneless external registration refused: {reason}"))
}

fn paneless_target(
    claim: &Registration,
    session: &str,
    identity: Option<ProcIdentity>,
    seats: &[SeatDescriptor],
) -> std::result::Result<String, RegistrationError> {
    let identity = identity.expect("validated external host identity");
    let target = seats
        .iter()
        .filter(|seat| {
            seat.harness.as_str() == claim.harness
                && seat.harness_session.as_deref() == Some(session)
        })
        .min_by_key(|seat| {
            (
                seat.tombstoned_at.is_some(),
                std::cmp::Reverse(seat.proc.map(|proc| proc.proc_start)),
                &seat.id,
            )
        });
    for seat in seats {
        let same_session = seat.harness.as_str() == claim.harness
            && seat.harness_session.as_deref() == Some(session);
        let same_pid = seat.proc.is_some_and(|proc| proc.pid == identity.pid);
        if !same_session && !same_pid {
            continue;
        }
        if seat.tombstoned_at.is_some() {
            // Historical duplicates cannot own this host or displace the
            // preferred same-session address. A foreign session is still
            // forbidden from taking a retired paneless process owner's id.
            if target.is_some_and(|target| target.id != seat.id) {
                continue;
            }
            if !same_session {
                return Err(retired_refusal(seat, &claim.harness));
            }
        }
        if !same_session
            || (seat.tombstoned_at.is_none() && seat.proc != Some(identity))
            || seat.pane.is_some()
            || seat.relay
            || seat.native_extension_delivery
            || seat.spawn_id.is_some()
        {
            return Err(paneless_refusal(format!(
                "host process or native session conflicts with seat `{}`",
                seat.id
            )));
        }
        if seat.tombstoned_at.is_none() && target.is_some_and(|target| target.id != seat.id) {
            return Err(paneless_refusal(
                "native session has multiple Pij addresses",
            ));
        }
    }
    if let Some(target) = target {
        return Ok(target.id.to_string());
    }
    memorable_pij_id_candidates(&format!("{}\0{session}", claim.harness))
        .find(|candidate| seats.iter().all(|seat| seat.id != *candidate))
        .map(|id| id.to_string())
        .ok_or_else(|| paneless_refusal("memorable Pij ID space is exhausted"))
}

fn native_prebind<'a>(
    claim: &Registration,
    session: &str,
    seats: &'a [SeatDescriptor],
) -> Option<&'a SeatDescriptor> {
    seats.iter().find(|seat| {
        seat.harness == Harness::Copilot
            && !seat.relay
            && seat.tombstoned_at.is_none()
            && seat.proc.is_none()
            && seat.pane.is_some()
            && seat.pane == claim.pane
            && seat
                .spawn_id
                .as_deref()
                .is_some_and(|spawn| !spawn.is_empty() && claim.spawn_id.as_deref() == Some(spawn))
            && seat
                .harness_session
                .as_deref()
                .is_none_or(|native| native == session)
    })
}

fn registration_target<'a>(
    claim: &Registration,
    native: bool,
    session: Option<&str>,
    seats: &'a [SeatDescriptor],
) -> Option<&'a SeatDescriptor> {
    if native && let Some(session) = session {
        if let Some(prebind) = native_prebind(claim, session, seats) {
            return Some(prebind);
        }
        // Historical duplicates are not contenders. Prefer a live address, then
        // the newest incarnation, with id only as a deterministic final tie.
        if let Some(seat) = seats
            .iter()
            .filter(|seat| {
                seat.harness == Harness::Copilot && seat.harness_session.as_deref() == Some(session)
            })
            .min_by_key(|seat| {
                (
                    seat.tombstoned_at.is_some(),
                    std::cmp::Reverse(seat.proc.map(|proc| proc.proc_start)),
                    &seat.id,
                )
            })
        {
            return Some(seat);
        }
    }
    seats
        .iter()
        .find(|seat| seat.id.as_str() == claim.id)
        .or_else(|| {
            if native && claim.supersedes.is_some() {
                return None;
            }
            let spawn_id = claim.spawn_id.as_deref().filter(|id| !id.is_empty())?;
            let pane = claim.pane.as_deref()?;
            seats.iter().find(|seat| {
                seat.proc.is_none()
                    && seat.spawn_id.as_deref() == Some(spawn_id)
                    && seat.pane.as_deref() == Some(pane)
            })
        })
}

fn same_native_owner(observed: &SeatDescriptor, current: &SeatDescriptor) -> bool {
    observed.harness == current.harness
        && observed.proc == current.proc
        && observed.pane == current.pane
        && observed.harness_session == current.harness_session
        && observed.spawn_id == current.spawn_id
        && observed.native_extension_delivery == current.native_extension_delivery
        && observed.relay == current.relay
        && observed.tombstoned_at == current.tombstoned_at
}

fn check_native_roster(
    claim: &Registration,
    session: Option<&str>,
    identity: Option<ProcIdentity>,
    seats: &[SeatDescriptor],
    existing: Option<&SeatDescriptor>,
    dead_native: &[SeatDescriptor],
    spawn_predecessor: Option<&SeatDescriptor>,
) -> std::result::Result<(), RegistrationError> {
    let session = session.ok_or_else(|| native_refusal("requires a runtime native session"))?;
    let identity = identity.ok_or_else(|| native_refusal("requires a process identity"))?;
    let predecessor = if let Some(previous) = &claim.supersedes {
        // A verified host may switch from bootstrap back to its saved session.
        let resuming_native =
            existing.is_some_and(|target| target.harness_session.as_deref() == Some(session));
        if previous.as_str() == claim.id || (existing.is_some() && !resuming_native) {
            return Err(native_refusal(
                "supersedes must create a new seat id or resume its native address",
            ));
        }
        let previous = seats
            .iter()
            .find(|seat| &seat.id == previous)
            .ok_or_else(|| native_refusal("superseded seat is absent"))?;
        if previous.tombstoned_at.is_some()
            || previous.proc != Some(identity)
            || previous.harness != Harness::Copilot
            || previous.relay
            || previous.pane != claim.pane
            || previous
                .harness_session
                .as_deref()
                .is_none_or(|old| old.is_empty() || old == session)
        {
            return Err(native_refusal(
                "supersedes requires an active Copilot seat, the same host/pane, and a new native session",
            ));
        }
        Some(previous)
    } else {
        None
    };

    if let Some(existing) = existing {
        let same_session = existing.harness_session.as_deref() == Some(session);
        if existing.machine.is_some()
            || existing.harness != Harness::Copilot
            || existing.relay
            || (!same_session && (existing.tombstoned_at.is_some() || existing.pane != claim.pane))
            || existing
                .harness_session
                .as_deref()
                .is_some_and(|old| old != session)
        {
            return Err(native_refusal(
                "seat id conflicts with its harness, pane, session, or retirement",
            ));
        }
        match existing.proc {
            Some(previous) if previous != identity && !same_session => {
                return Err(native_refusal(
                    "seat id belongs to a different process incarnation",
                ));
            }
            None => {
                if existing.spawn_id.as_deref().is_none_or(str::is_empty)
                    || existing.spawn_id != claim.spawn_id
                    || existing.pane.is_none()
                {
                    return Err(native_refusal(
                        "pre-bind seat requires its exact spawn id and pane",
                    ));
                }
            }
            Some(_) => {}
        }
    }
    if let Some(spawn_id) = claim.spawn_id.as_deref()
        && (spawn_id.is_empty()
            || existing
                .filter(|seat| seat.spawn_id.as_deref() == Some(spawn_id))
                .or(predecessor)
                .and_then(|seat| seat.spawn_id.as_deref())
                != Some(spawn_id))
    {
        return Err(native_refusal(
            "spawn id has no matching pre-bind/current incarnation",
        ));
    }
    let unknown_session = existing.is_none()
        && !seats
            .iter()
            .any(|seat| seat.harness_session.as_deref() == Some(session));
    let mut held_owner = None;
    for seat in seats.iter().filter(|seat| seat.tombstoned_at.is_none()) {
        if existing.is_some_and(|existing| existing.id == seat.id)
            || predecessor.is_some_and(|previous| previous.id == seat.id)
            || dead_native.iter().any(|observed| observed.id == seat.id)
            || spawn_predecessor.is_some_and(|previous| previous.id == seat.id)
        {
            continue;
        }
        if seat.proc == Some(identity)
            || (seat.harness == Harness::Copilot
                && seat.harness_session.as_deref() == Some(session))
            || (claim.pane.is_some() && seat.pane == claim.pane)
        {
            if unknown_session
                && claim.pane.is_some()
                && seat.pane == claim.pane
                && seat.harness == Harness::Copilot
                && seat.native_extension_delivery
                && !seat.relay
                && seat.machine.is_none()
                && seat
                    .proc
                    .is_some_and(|proc| proc.pid > 0 && proc.proc_start > 0)
                && seat
                    .harness_session
                    .as_deref()
                    .is_some_and(|old| !old.is_empty() && old != session)
            {
                // Tombstoned and observed-dead owners were excluded above.
                // Finish the scan so a separate ownership refusal wins.
                held_owner = Some(&seat.id);
                continue;
            }
            return Err(native_refusal(format!(
                "process, native session, or pane already belongs to seat `{}`",
                seat.id
            )));
        }
    }
    if let Some(owner) = held_owner {
        return Err(RegistrationError::NativeSessionHold(owner.clone()));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct NativeProcess {
    parent: u32,
    executable: String,
    executable_replaced: bool,
    argv: Vec<String>,
}

impl NativeProcess {
    fn is_external_host(&self, harness: Harness) -> bool {
        if harness == Harness::Copilot {
            return self.is_copilot_host();
        }
        let (binary, entry) = match harness {
            Harness::Claude => ("claude", "@anthropic-ai/claude-code/cli.js"),
            Harness::Codex => ("codex", "@openai/codex/bin/codex.js"),
            _ => return false,
        };
        let executable = Path::new(&self.executable).file_name();
        let argv0 = self.argv.first().and_then(|arg| Path::new(arg).file_name());
        if executable != argv0 {
            return false;
        }
        executable == Some(std::ffi::OsStr::new(binary))
            || (executable == Some(std::ffi::OsStr::new("node"))
                && self
                    .argv
                    .get(1)
                    .is_some_and(|script| Path::new(script).ends_with(entry)))
    }

    fn matches_external_session(&self, harness: Harness, session: &str) -> bool {
        let start = if Path::new(&self.executable).file_name() == Some(std::ffi::OsStr::new("node"))
        {
            2
        } else {
            1
        };
        let argv = &self.argv[start..];
        // A Claude fork names both its source conversation and its new identity.
        let fork = harness == Harness::Claude && argv.iter().any(|arg| arg == "--fork-session");
        let mut pinned = false;
        let mut args = argv.iter().peekable();
        while let Some(arg) = args.next() {
            if arg == "--" {
                break;
            }
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(flag, value)| (flag, Some(value)));
            let session_flag = match harness {
                Harness::Claude => matches!(flag, "--session-id" | "--resume" | "-r"),
                Harness::Copilot => matches!(flag, "--session-id" | "--resume"),
                Harness::Codex => flag == "resume",
                _ => false,
            };
            if !session_flag {
                continue;
            }
            let value = inline.or_else(|| {
                args.peek()
                    .filter(|value| !value.starts_with('-'))
                    .map(|value| value.as_str())
            });
            if flag == "--session-id" {
                pinned = value == Some(session);
                if !pinned {
                    return false;
                }
            } else if !fork && value.is_some_and(|value| value != session) {
                return false;
            }
            if inline.is_none() && value.is_some() {
                args.next();
            }
        }
        !fork || pinned
    }

    fn is_copilot_host(&self) -> bool {
        let executable = Path::new(&self.executable)
            .file_name()
            .and_then(|name| name.to_str());
        let argv0 = self.argv.first().and_then(|arg| Path::new(arg).file_name());
        match executable {
            Some("copilot") => argv0 == Some(std::ffi::OsStr::new("copilot")),
            Some("node") => {
                argv0 == Some(std::ffi::OsStr::new("node"))
                    && self.argv.get(1).is_some_and(|script| {
                        Path::new(script).ends_with("@github/copilot/index.js")
                    })
            }
            // In particular, sh/zsh -c 'copilot ...', the npm launcher, and the
            // Node extension child are not evidence of the actual native host.
            _ => false,
        }
    }
}

async fn observe_native_process(
    pid: u32,
) -> std::result::Result<Option<NativeProcess>, RegistrationError> {
    tokio::task::spawn_blocking(move || read_native_process(pid))
        .await
        .map_err(|error| native_refusal(format!("process observer failed: {error}")))?
}

// Kernel convention: a running executable whose file was unlinked or replaced
// reads as `<path> (deleted)`. Strip it once, here, so every matcher sees the
// same basename. A binary literally named `copilot (deleted)` therefore
// normalizes to `copilot` with replaced=true; that is the SAME trust class as
// any binary named `copilot` (basename is already the accept rule, argv0 and
// entrypoint checks still apply), so no control widens.
#[cfg(any(target_os = "linux", test))]
fn normalize_kernel_exe(mut executable: String) -> (String, bool) {
    let replaced = executable.ends_with(" (deleted)");
    if replaced {
        executable.truncate(executable.len() - " (deleted)".len());
    }
    (executable, replaced)
}

#[cfg(target_os = "linux")]
fn read_native_process(pid: u32) -> std::result::Result<Option<NativeProcess>, RegistrationError> {
    // Linux comm is a mutable task name, NOT an executable witness. Use the
    // kernel executable link and NUL-delimited argv instead of trusting ps comm.
    let root = PathBuf::from(format!("/proc/{pid}"));
    let executable = match std::fs::read_link(root.join("exe")) {
        Ok(path) => path
            .into_os_string()
            .into_string()
            .map_err(|_| native_refusal("host executable is not UTF-8"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(native_refusal(format!(
                "cannot observe host executable: {error}"
            )));
        }
    };
    let (executable, executable_replaced) = normalize_kernel_exe(executable);
    let Some(stat) = read_native_proc_file(&root.join("stat"))? else {
        return Ok(None);
    };
    let parent = std::str::from_utf8(&stat)
        .ok()
        .and_then(|stat| stat.rsplit_once(')'))
        .and_then(|(_, fields)| fields.split_whitespace().nth(1))
        .and_then(|pid| pid.parse().ok())
        .ok_or_else(|| native_refusal("process observation has no parent pid"))?;
    let Some(cmdline) = read_native_proc_file(&root.join("cmdline"))? else {
        return Ok(None);
    };
    if cmdline.is_empty() {
        return Err(native_refusal("process observation has no argv"));
    }
    let argv = cmdline
        .strip_suffix(&[0])
        .unwrap_or(&cmdline)
        .split(|byte| *byte == 0)
        .map(|arg| std::str::from_utf8(arg).map(str::to_string))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| native_refusal("process argv is not UTF-8"))?;
    Ok(Some(NativeProcess {
        parent,
        executable,
        executable_replaced,
        argv,
    }))
}

#[cfg(target_os = "linux")]
fn read_native_proc_file(path: &Path) -> std::result::Result<Option<Vec<u8>>, RegistrationError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(native_refusal(format!(
                "cannot observe host process: {error}"
            )));
        }
    };
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| native_refusal(format!("cannot read process observation: {error}")))?;
    if bytes.len() > 64 * 1024 {
        return Err(native_refusal("process observation exceeded 64 KiB"));
    }
    Ok(Some(bytes))
}

#[cfg(any(not(target_os = "linux"), test))]
const PROCESS_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(1);

#[cfg(not(target_os = "linux"))]
fn read_native_process(pid: u32) -> std::result::Result<Option<NativeProcess>, RegistrationError> {
    use std::process::Command;

    // One requested PID, never a machine-wide process census. The child, output,
    // and wall time are bounded independently of the async caller's lifetime.
    // Darwin truncates `comm` paths when followed by another output column.
    // Read the kernel accounting name instead: the supported host basenames
    // fit, and is_copilot_host still independently checks argv/entrypoint.
    let mut command = Command::new("/bin/ps");
    command
        .args([
            "-ww",
            "-p",
            &pid.to_string(),
            "-o",
            "ppid=",
            "-o",
            "ucomm=",
            "-o",
            "args=",
        ])
        .env("LC_ALL", "C");
    read_native_process_using(pid, &mut command)
}

#[cfg(any(not(target_os = "linux"), test))]
fn read_native_process_using(
    pid: u32,
    command: &mut std::process::Command,
) -> std::result::Result<Option<NativeProcess>, RegistrationError> {
    use std::process::Stdio;
    use std::time::Instant;

    const OUTPUT_LIMIT: u64 = 64 * 1024;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| native_refusal(format!("cannot observe host process {pid}: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .expect("piped process observation stdout");
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let deadline = Instant::now() + PROCESS_OBSERVATION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(RegistrationError::Retryable(format!(
                    "process observation for pid {pid} timed out after {}ms",
                    PROCESS_OBSERVATION_TIMEOUT.as_millis(),
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(native_refusal(format!(
                    "process observation for pid {pid} failed: {error}"
                )));
            }
        }
    };
    let output = reader
        .join()
        .map_err(|_| native_refusal("process output reader failed"))?
        .map_err(|error| native_refusal(format!("cannot read process observation: {error}")))?;
    let status = status?;
    if output.len() as u64 > OUTPUT_LIMIT {
        return Err(native_refusal("process observation exceeded 64 KiB"));
    }
    if !status.success() {
        return Ok(None);
    }
    parse_native_process(&output).map(Some)
}

#[cfg(any(not(target_os = "linux"), test))]
fn parse_native_process(output: &[u8]) -> std::result::Result<NativeProcess, RegistrationError> {
    let text = std::str::from_utf8(output)
        .map_err(|_| native_refusal("process observation is not UTF-8"))?;
    if text.trim().lines().count() != 1 {
        return Err(native_refusal(
            "process observation must describe exactly one process",
        ));
    }
    let mut fields = text.split_whitespace();
    let parent = fields
        .next()
        .and_then(|pid| pid.parse().ok())
        .ok_or_else(|| native_refusal("process observation has no parent pid"))?;
    let executable = fields
        .next()
        .ok_or_else(|| native_refusal("process observation has no executable"))?
        .to_string();
    let argv: Vec<String> = fields.map(str::to_string).collect();
    if argv.is_empty() {
        return Err(native_refusal("process observation has no argv"));
    }
    Ok(NativeProcess {
        parent,
        executable,
        executable_replaced: false,
        argv,
    })
}

pub(crate) fn requested_model_matches(
    requested: Option<&str>,
    actual: Option<&str>,
    actual_observed: bool,
) -> bool {
    if !actual_observed {
        return true;
    }
    match (requested, actual) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(requested), Some(actual)) => {
            actual == requested
                || (!requested.contains('/')
                    && actual
                        .rsplit_once('/')
                        .is_some_and(|(_, model)| model == requested))
        }
    }
}

fn system_time_ms() -> std::result::Result<u64, RegistrationError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            RegistrationError::Runtime(PijError::Adapter {
                adapter: "daemon/registration".to_string(),
                message: format!("system clock is before Unix epoch: {error}"),
            })
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| {
        RegistrationError::Runtime(PijError::Adapter {
            adapter: "daemon/registration".to_string(),
            message: "system clock milliseconds exceed u64".to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::future::{Ready, ready};
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use pij_core::model::{Harness, PaneProcess, ProcIdentity, SeatDescriptor, SemanticState, Seq};
    use pij_core::ports::Queue;
    use pij_core::ports::{LivenessPort, Registry, SeatFilter, Spine};
    use pij_harnesses::InteractionGate;
    use pij_testkit::fakes::{FakeLiveness, FakeRegistry, FakeSpine, FakeTmux};
    use pij_testkit::fakes::{FakeQueue, FakeTransport};
    use tokio::sync::Notify;

    use crate::delivery::DeliveryService;

    use super::{
        NativeProcess, RegistrationError, RegistrationService, parse_native_process,
        requested_model_matches,
    };
    use crate::events::EventBus;
    use crate::http::Registration;
    use crate::http::role::RoleService;

    const HOST: ProcIdentity = ProcIdentity {
        pid: 4242,
        proc_start: 126,
    };
    const SHELL: ProcIdentity = ProcIdentity {
        pid: 41,
        proc_start: 100,
    };
    const OTHER_HOST: ProcIdentity = ProcIdentity {
        pid: 777,
        proc_start: 126,
    };
    const SESSION: &str = "00000000-0000-4000-8000-000000000137";

    fn native_claim(id: &str) -> Registration {
        Registration {
            supersedes: None,
            id: id.to_string(),
            harness: "copilot".to_string(),
            folder: "/isolated/native-registration".to_string(),
            extension_build: None,
            extension_path: None,
            pane: Some("%137".to_string()),
            pid: Some(HOST.pid),
            proc_start: Some(HOST.proc_start),
            spawn_id: None,
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        }
    }

    fn native_seat(id: &str) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new(id, Harness::Copilot, "/isolated/native-registration");
        seat.proc = Some(HOST);
        seat.pane = Some("%137".to_string());
        seat.harness_session = Some(SESSION.to_string());
        seat.native_extension_delivery = true;
        seat
    }

    fn native_liveness() -> Arc<dyn LivenessPort> {
        Arc::new(
            FakeLiveness::new()
                .with_proc(HOST)
                .with_proc(SHELL)
                .with_proc(OTHER_HOST),
        )
    }

    fn native_tmux() -> FakeTmux {
        FakeTmux::new().with_pane_process(
            "%137",
            PaneProcess {
                pid: SHELL.pid,
                cwd: "/isolated/native-registration".to_string(),
            },
        )
    }

    async fn native_service(
        seats: Vec<SeatDescriptor>,
        liveness: Arc<dyn LivenessPort>,
    ) -> (RegistrationService, Arc<FakeRegistry>) {
        let registry = Arc::new(
            seats
                .into_iter()
                .fold(FakeRegistry::new(), |registry, seat| {
                    registry.with_seat(seat)
                }),
        );
        let bus = Arc::new(EventBus::new(Arc::new(FakeSpine::new()), 8).expect("event bus"));
        let pool = pij_store::open("").await.expect("isolated in-memory roles");
        let roles = Arc::new(RoleService::new(
            registry.clone(),
            pij_store::SqliteOrchestration::new(pool),
            bus.clone(),
        ));
        (
            RegistrationService::new(registry.clone(), liveness, bus, Vec::new(), roles),
            registry,
        )
    }

    fn fixture_process(
        pid: u32,
    ) -> Ready<std::result::Result<Option<NativeProcess>, RegistrationError>> {
        ready(match pid {
            4242 | 777 => parse_native_process(
                b"41 /opt/copilot/copilot /opt/copilot/copilot --model gpt-5\n",
            )
            .map(Some),
            41 => parse_native_process(b"0 /bin/zsh -zsh\n").map(Some),
            _ => Ok(None),
        })
    }

    async fn attest(
        service: &RegistrationService,
        claim: Registration,
        session: Option<&str>,
        tmux: &FakeTmux,
    ) -> std::result::Result<SeatDescriptor, RegistrationError> {
        service
            .register_native_with_observer(
                claim,
                session.map(str::to_string),
                tmux,
                fixture_process,
            )
            .await
            .map(|(descriptor, _)| descriptor)
    }

    fn refused<T: std::fmt::Debug>(result: std::result::Result<T, RegistrationError>) {
        assert!(
            matches!(result, Err(RegistrationError::Refused(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn successful_registration_never_returns_a_tombstone() {
        for harness in [
            Harness::Omp,
            Harness::Pi,
            Harness::Claude,
            Harness::Codex,
            Harness::Copilot,
        ] {
            for state in ["new", "live", "retired"] {
                let mut old = native_seat("pij-registration-invariant");
                old.harness = harness;
                old.native_extension_delivery = false;
                if state == "retired" {
                    old.tombstoned_at = Some(154);
                    old.tombstone_reason = Some("observed-dead".into());
                }
                let seats = if state == "new" {
                    Vec::new()
                } else {
                    vec![old.clone()]
                };
                let (service, registry) = native_service(seats, native_liveness()).await;
                let mut claim = native_claim(old.id.as_str());
                claim.harness = harness.as_str().into();
                let (descriptor, _) = service
                    .register_inner(
                        claim,
                        Some(SESSION.into()),
                        super::RegistrationMode::Ordinary,
                    )
                    .await
                    .expect("same-session observed registration succeeds");
                assert!(
                    descriptor.tombstoned_at.is_none(),
                    "{harness:?}/{state}: successful registration returned {descriptor:?}"
                );
                assert_eq!(registry.get(&old.id).await.unwrap(), Some(descriptor));
            }
        }
    }

    async fn retired_session_resume_contract(harness: Harness, claimed_role: Option<&str>) {
        let mut old = native_seat("pij-retired-session");
        old.harness = harness;
        old.native_extension_delivery = false;
        old.parent = Some("pij-original-parent".into());
        let (service, registry) = native_service(vec![old.clone()], native_liveness()).await;
        service
            .roles
            .assert_role(&old.id, &old.id, Some("worker".into()))
            .await
            .unwrap();
        old.tombstoned_at = Some(154);
        old.tombstone_reason = Some("observed-dead".into());
        registry.put(old.clone()).await.unwrap();
        let conversation: pij_core::model::SeatId = "pij-telegram-chat-154".into();
        service
            .event_bus
            .publish(pij_core::model::Event {
                seq: None,
                v: 1,
                at: 1,
                kind: "telegram.binding".into(),
                seat: Some(conversation.clone()),
                payload: serde_json::json!({"target":old.id,"outbound_msg_id":"before-resume"})
                    .to_string(),
            })
            .await
            .unwrap();
        let spine = service.event_bus.raw_spine();
        let binding = spine
            .latest_matching(&conversation, &["telegram.binding"])
            .await
            .unwrap();
        let mut claim = native_claim(old.id.as_str());
        claim.harness = harness.as_str().into();
        claim.pid = Some(OTHER_HOST.pid);
        claim.proc_start = Some(OTHER_HOST.proc_start);
        claim.pane = Some("%154-new".into());
        claim.parent = Some("pij-unrelated-parent".into());
        claim.role = claimed_role.map(str::to_string);
        let (resumed, _) = service
            .register_with_harness_session(claim, Some(SESSION.into()))
            .await
            .expect("saved session revives its address");
        assert_eq!(
            resumed.tombstoned_at, None,
            "{harness:?} retained its tombstone"
        );
        assert_eq!(resumed.tombstone_reason, None);
        assert_eq!(resumed.id, old.id);
        assert_eq!(resumed.proc, Some(OTHER_HOST));
        assert_eq!(resumed.pane.as_deref(), Some("%154-new"));
        assert_eq!(resumed.parent, old.parent);
        assert_eq!(resumed.role.as_deref(), Some("worker"));
        assert_eq!(
            service.roles.read_role(&old.id).await.unwrap().as_deref(),
            Some("worker")
        );
        assert_eq!(
            spine
                .latest_matching(&conversation, &["telegram.binding"])
                .await
                .unwrap(),
            binding
        );
        assert_eq!(
            registry.get(&old.id).await.unwrap().unwrap().parent,
            old.parent
        );
        let event = spine
            .latest_matching(&old.id, &["seat.resumed"])
            .await
            .unwrap()
            .expect("resume fact");
        let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
        assert_eq!(payload["prior_reason"], "observed-dead");
        assert_eq!(payload["old_proc"], serde_json::json!(HOST));
        assert_eq!(payload["new_proc"], serde_json::json!(OTHER_HOST));
        assert_eq!(payload["old_pane"], "%137");
        assert_eq!(payload["new_pane"], "%154-new");
        let events = spine.tail(Some(&old.id), Seq(0)).await.unwrap();
        let resumes: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind.as_str(), "seat.resumed" | "seat.native-resumed"))
            .map(|event| event.kind.as_str())
            .collect();
        assert_eq!(
            resumes,
            ["seat.resumed"],
            "one fact per ordinary transition"
        );
    }

    #[tokio::test]
    async fn retired_native_foreign_session_refusal_names_tombstone_even_with_same_process() {
        let mut old = native_seat("pij-retired-native");
        old.tombstoned_at = Some(154);
        old.tombstone_reason = Some("observed-dead".into());
        old.native_extension_delivery = false;
        let (service, registry) = native_service(vec![old.clone()], native_liveness()).await;
        let error = attest(
            &service,
            native_claim(old.id.as_str()),
            Some("foreign-session"),
            &native_tmux(),
        )
        .await
        .expect_err("native foreign-session guard remains stronger than process continuity");
        assert_eq!(
            error.to_string(),
            "seat pij-retired-native is retired (observed-dead, 154); a different session may not take it"
        );
        assert_eq!(registry.get(&old.id).await.unwrap(), Some(old));
    }

    #[tokio::test]
    async fn retired_omp_session_resumes_with_bindings() {
        retired_session_resume_contract(Harness::Omp, None).await;
    }

    #[tokio::test]
    async fn retired_pi_session_resumes_with_bindings() {
        retired_session_resume_contract(Harness::Pi, None).await;
    }

    #[tokio::test]
    async fn retired_claude_session_resumes_with_bindings() {
        retired_session_resume_contract(Harness::Claude, None).await;
    }

    #[tokio::test]
    async fn retired_codex_session_resumes_with_bindings() {
        retired_session_resume_contract(Harness::Codex, None).await;
    }

    #[tokio::test]
    async fn retired_session_resume_does_not_replace_recorded_role() {
        retired_session_resume_contract(Harness::Omp, Some("pm")).await;
    }

    #[tokio::test]
    async fn retired_registration_accepts_spawn_or_observed_process_continuity() {
        for proof in ["spawn", "process"] {
            let mut old = native_seat("pij-retired-continuity");
            old.harness = Harness::Omp;
            old.native_extension_delivery = false;
            old.harness_session = Some("previous-session".into());
            old.spawn_id = Some("launch-154".into());
            old.tombstoned_at = Some(154);
            old.tombstone_reason = Some("closed".into());
            let (service, _) = native_service(vec![old.clone()], native_liveness()).await;
            let mut claim = native_claim(old.id.as_str());
            claim.harness = "omp".into();
            if proof == "spawn" {
                claim.spawn_id = old.spawn_id;
                claim.pid = Some(OTHER_HOST.pid);
                claim.proc_start = Some(OTHER_HOST.proc_start);
            }
            let (resumed, _) = service
                .register_with_harness_session(claim, Some("new-session".into()))
                .await
                .expect("independent continuity witness");
            assert_eq!(resumed.tombstoned_at, None, "{proof}");
            assert_eq!(resumed.harness_session.as_deref(), Some("new-session"));
        }
    }

    #[tokio::test]
    async fn retired_registration_does_not_treat_absence_or_foreign_harness_as_continuity() {
        for shape in [
            "foreign-session",
            "missing-session",
            "empty-spawn",
            "missing-recorded-process",
            "foreign-harness",
            "recycled-pid",
        ] {
            let mut old = native_seat("pij-retired-refusal");
            old.harness = Harness::Omp;
            old.native_extension_delivery = false;
            old.tombstoned_at = Some(154);
            old.tombstone_reason = Some("observed-dead".into());
            let mut claim = native_claim(old.id.as_str());
            claim.harness = "omp".into();
            claim.pid = Some(OTHER_HOST.pid);
            claim.proc_start = Some(OTHER_HOST.proc_start);
            let mut session = Some("foreign-session".to_string());
            match shape {
                "missing-session" => {
                    old.harness_session = None;
                    session = None;
                }
                "empty-spawn" => {
                    old.spawn_id = Some(String::new());
                    claim.spawn_id = Some(String::new());
                }
                "missing-recorded-process" => old.proc = None,
                "foreign-harness" => {
                    claim.harness = "pi".into();
                    session = old.harness_session.clone();
                }
                "recycled-pid" => {
                    old.proc = Some(ProcIdentity {
                        pid: OTHER_HOST.pid,
                        proc_start: OTHER_HOST.proc_start - 1,
                    });
                }
                _ => {}
            }
            let (service, registry) = native_service(vec![old.clone()], native_liveness()).await;
            let error = service
                .register_with_harness_session(claim, session)
                .await
                .expect_err("retired claim without continuity");
            let expected = if shape == "foreign-harness" {
                "seat pij-retired-refusal is retired (observed-dead, 154); a different session may not take it; harness mismatch: row omp, claim pi"
            } else {
                "seat pij-retired-refusal is retired (observed-dead, 154); a different session may not take it"
            };
            assert_eq!(error.to_string(), expected, "{shape}");
            assert_eq!(registry.get(&old.id).await.unwrap(), Some(old), "{shape}");
        }
    }

    #[tokio::test]
    async fn native_session_resume_lifts_tombstone_and_publishes_incarnation_change() {
        let mut old = native_seat("pij-durable");
        old.tombstoned_at = Some(1);
        old.tombstone_reason = Some("observed-dead".into());
        old.native_extension_delivery = false;
        let (service, registry) = native_service(vec![old.clone()], native_liveness()).await;
        let mut claim = native_claim("");
        claim.pid = Some(OTHER_HOST.pid);
        claim.proc_start = Some(OTHER_HOST.proc_start);
        claim.pane = None;
        let resumed = attest(&service, claim, Some(SESSION), &native_tmux())
            .await
            .expect("native session is the durable identity, including retired rows");
        assert_eq!(resumed.id, old.id);
        assert_eq!(resumed.proc, Some(OTHER_HOST));
        assert_eq!(resumed.pane, None);
        assert_eq!(resumed.tombstoned_at, None);
        assert_eq!(resumed.tombstone_reason, None);
        assert!(resumed.native_extension_delivery);
        assert_eq!(registry.get(&old.id).await.unwrap(), Some(resumed));
        let event = service
            .event_bus
            .raw_spine()
            .latest_matching(&old.id, &["seat.native-resumed"])
            .await
            .unwrap()
            .expect("resume fact");
        let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
        assert_eq!(payload["old_proc"], serde_json::json!(HOST));
        assert_eq!(payload["new_proc"], serde_json::json!(OTHER_HOST));
        assert_eq!(payload["old_pane"], "%137");
        assert!(payload["new_pane"].is_null());
        assert_eq!(payload["reason"], "native-session-resume");
        assert_eq!(payload["prior_reason"], "observed-dead");
        let events = service
            .event_bus
            .raw_spine()
            .tail(Some(&old.id), Seq(0))
            .await
            .unwrap();
        let resumes: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind.as_str(), "seat.resumed" | "seat.native-resumed"))
            .map(|event| event.kind.as_str())
            .collect();
        assert_eq!(
            resumes,
            ["seat.native-resumed"],
            "one fact per native transition"
        );
    }

    #[tokio::test]
    async fn native_session_fast_restart_does_not_consult_old_process() {
        let old = native_seat("pij-fast-restart");
        let liveness = Arc::new(PendingDeadOwner {
            entered: Notify::new(),
            release: Notify::new(),
            fail: true,
        });
        let (service, _) = native_service(vec![old.clone()], liveness).await;
        let mut claim = native_claim(old.id.as_str());
        claim.pid = Some(OTHER_HOST.pid);
        claim.proc_start = Some(OTHER_HOST.proc_start);
        let resumed = tokio::time::timeout(
            Duration::from_millis(100),
            attest(&service, claim, Some(SESSION), &native_tmux()),
        )
        .await
        .expect("resume must not await the old incarnation sensor")
        .expect("verified session claim admits fast restart");
        assert_eq!(resumed.id, old.id);
        assert_eq!(resumed.proc, Some(OTHER_HOST));
    }

    #[tokio::test]
    async fn native_session_cannot_resume_a_row_from_another_machine() {
        let mut remote = native_seat("pij-remote-native");
        remote.machine = Some("another-machine".into());
        let (service, registry) = native_service(vec![remote.clone()], native_liveness()).await;
        refused(attest(&service, native_claim(""), Some(SESSION), &native_tmux()).await);
        assert_eq!(registry.get(&remote.id).await.unwrap(), Some(remote));
    }

    #[tokio::test]
    async fn native_requires_truthful_harness_session_and_complete_observed_tuple() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let tmux = native_tmux();
        for session in [None, Some(""), Some("   ")] {
            refused(attest(&service, native_claim("pij-native"), session, &tmux).await);
        }
        for (pid, proc_start) in [
            (None, None),
            (Some(HOST.pid), None),
            (None, Some(HOST.proc_start)),
            (Some(0), Some(HOST.proc_start)),
            (Some(HOST.pid), Some(0)),
            (Some(HOST.pid), Some(HOST.proc_start + 1)),
            (Some(999), Some(126)),
        ] {
            let mut claim = native_claim("pij-native");
            claim.pid = pid;
            claim.proc_start = proc_start;
            refused(attest(&service, claim, Some(SESSION), &tmux).await);
        }
        let mut other_harness = native_claim("pij-native");
        other_harness.harness = "pi".to_string();
        refused(attest(&service, other_harness, Some(SESSION), &tmux).await);
        let mut relay = native_claim("pij-native");
        relay.relay = true;
        refused(attest(&service, relay, Some(SESSION), &tmux).await);
        assert!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .is_empty()
        );
    }

    #[test]
    fn actual_host_matches_executable_and_entrypoint_not_shell_command_text() {
        for output in [
            "41 /opt/copilot/copilot /opt/copilot/copilot --model gpt-5\n",
            "41 /usr/bin/node node /opt/node_modules/@github/copilot/index.js\n",
            "41 copilot /long/absolute/install/path/copilot --model gpt-5\n",
            "41 node /long/absolute/install/path/node /opt/node_modules/@github/copilot/index.js\n",
        ] {
            assert!(
                parse_native_process(output.as_bytes())
                    .expect("host facts")
                    .is_copilot_host()
            );
        }
        for output in [
            "41 /bin/sh sh -c copilot --model gpt-5\n",
            "41 /bin/zsh zsh -c /opt/copilot/copilot\n",
            "41 /usr/bin/node node /opt/node_modules/@github/copilot/npm-loader.js\n",
            "41 /usr/bin/node node /home/me/.copilot/extensions/pij/extension.mjs\n",
            "41 /usr/bin/node node -e require('@github/copilot')\n",
            "41 /usr/bin/node node /opt/not-@github/copilot/index.js\n",
            "41 /opt/copilot-evil copilot\n",
            "41 /opt/copilot/copilot sh -c copilot\n",
            "41 sh /long/absolute/install/path/copilot --model gpt-5\n",
            "41 node node /opt/node_modules/@github/copilot/npm-loader.js\n",
            "41 node node /home/me/.copilot/extensions/pij/extension.mjs\n",
        ] {
            assert!(
                !parse_native_process(output.as_bytes())
                    .expect("non-host facts")
                    .is_copilot_host(),
                "{output}"
            );
        }
        assert!(parse_native_process(b"41 /bin/sh\n").is_err());
        assert!(parse_native_process(b"41 /bin/sh sh\n42 /bin/sh sh\n").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_shell_process_is_not_an_actual_copilot_host() {
        // A test-owned child only: no fleet inspection, signal, or environment mutation.
        // A forged argv[0] must not outrank the observed kernel executable name.
        // The shell prints one line once it has exec'd: observing it before that
        // reads a transiently empty /proc/<pid>/cmdline on Linux (seen on CI, PR
        // #371) and fails for the wrong reason.
        let mut shell = Command::new("/bin/sh")
            .arg0("copilot")
            .args(["-c", "echo ready; read line"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("private shell witness");
        {
            use std::io::BufRead;
            let stdout = shell.stdout.take().expect("witness stdout");
            let mut ready = String::new();
            std::io::BufReader::new(stdout)
                .read_line(&mut ready)
                .expect("witness readiness line");
            assert_eq!(ready.trim(), "ready");
        }
        let (service, registry) = native_service(
            Vec::new(),
            Arc::new(FakeLiveness::new().with_proc(ProcIdentity {
                pid: shell.id(),
                proc_start: 1,
            })),
        )
        .await;
        let mut claim = native_claim("pij-real-shell");
        claim.pid = Some(shell.id());
        claim.proc_start = Some(1);
        claim.pane = None;
        let observed = service
            .register_native(claim, Some(SESSION.to_string()), &FakeTmux::new())
            .await;
        let _ = shell.kill();
        shell.wait().expect("reap private shell witness");
        assert!(
            observed
                .expect_err("real shell refused")
                .to_string()
                .contains("actual Copilot host executable")
        );
        assert!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .is_empty()
        );
    }

    #[test]
    fn kernel_executable_normalization_strips_only_the_exact_trailing_marker() {
        for (raw, normalized, replaced) in [
            ("/a/copilot (deleted)", "/a/copilot", true),
            ("/a/copilot", "/a/copilot", false),
            ("/a/copilot (deleted) ", "/a/copilot (deleted) ", false),
            ("(deleted)", "(deleted)", false),
        ] {
            assert_eq!(
                super::normalize_kernel_exe(raw.to_string()),
                (normalized.to_string(), replaced)
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replaced_executable_accepts_only_the_actual_copilot_host() {
        use std::io::BufRead;

        let root = std::env::temp_dir().join(format!("pij-replaced-exe-{}", std::process::id()));
        for (name, argv0, accepted) in [
            ("copilot", "copilot", true),
            ("copilot-evil", "copilot", false),
            ("copilot", "sh", false),
        ] {
            std::fs::create_dir(&root).expect("private executable directory");
            let executable = root.join(name);
            std::fs::copy("/bin/sh", &executable).expect("copy executable witness");
            let mut shell = Command::new(&executable)
                .arg0(argv0)
                .args(["-c", "echo ready; read line"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("private replaced executable witness");
            let mut ready = String::new();
            let readiness = std::io::BufReader::new(shell.stdout.take().expect("witness stdout"))
                .read_line(&mut ready);
            let unlink = std::fs::remove_file(&executable);
            let observed = super::read_native_process(shell.id());
            let _ = shell.kill();
            shell.wait().expect("reap private executable witness");
            std::fs::remove_dir(&root).expect("remove private directory");
            readiness.expect("witness readiness line");
            assert_eq!(ready.trim(), "ready");
            unlink.expect("unlink running executable");
            let observed = observed.expect("kernel observation").expect("live host");
            assert_eq!(
                observed.is_copilot_host(),
                accepted,
                "{name}, argv0={argv0}"
            );
            assert!(observed.executable_replaced, "{name}, argv0={argv0}");
        }
    }

    #[tokio::test]
    async fn native_host_refusal_names_observed_basename_and_replacement() {
        let (service, _) = native_service(Vec::new(), native_liveness()).await;
        for replaced in [false, true] {
            let error = service
                .verify_native_host(HOST, None, &FakeTmux::new(), &mut |_| {
                    let mut process = parse_native_process(b"41 /opt/copilot-evil copilot\n")
                        .expect("lookalike fixture");
                    process.executable_replaced = replaced;
                    ready(Ok(Some(process)))
                })
                .await
                .expect_err("lookalike host refused");
            assert!(
                error.to_string().contains(&format!(
                    "observed basename=\"copilot-evil\", replaced={replaced}"
                )),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn native_attests_actual_host_below_shell_but_refuses_shell_pane_pid() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let tmux = native_tmux();
        let seat = attest(&service, native_claim("pij-native"), Some(SESSION), &tmux)
            .await
            .expect("actual native host descended from pane shell");
        assert_eq!(seat.proc, Some(HOST));
        assert_eq!(seat.harness, Harness::Copilot);
        assert_eq!(seat.harness_session.as_deref(), Some(SESSION));
        assert!(seat.native_extension_delivery);

        let mut shell_claim = native_claim("pij-shell");
        shell_claim.pid = Some(SHELL.pid);
        shell_claim.proc_start = Some(SHELL.proc_start);
        let error = attest(&service, shell_claim, Some("shell-session"), &tmux)
            .await
            .expect_err("pane pid alone is not a host witness");
        assert!(error.to_string().contains("actual Copilot host executable"));
        assert_eq!(
            registry.list(SeatFilter::default()).await.expect("roster"),
            vec![seat]
        );
    }

    #[tokio::test]
    async fn native_requires_exact_observed_pane_ancestry_but_supports_paneless_hosts() {
        let (service, _) = native_service(Vec::new(), native_liveness()).await;
        refused(
            attest(
                &service,
                native_claim("pij-native"),
                Some(SESSION),
                &FakeTmux::new(),
            )
            .await,
        );
        let wrong_pane = FakeTmux::new().with_pane_process(
            "%137",
            PaneProcess {
                pid: OTHER_HOST.pid,
                cwd: "/isolated/native-registration".to_string(),
            },
        );
        refused(
            attest(
                &service,
                native_claim("pij-native"),
                Some(SESSION),
                &wrong_pane,
            )
            .await,
        );
        let mut paneless = native_claim("pij-native");
        paneless.pane = None;
        assert!(
            attest(&service, paneless, Some(SESSION), &FakeTmux::new())
                .await
                .expect("paneless actual host")
                .native_extension_delivery
        );
    }

    #[tokio::test]
    async fn native_transient_hold_never_masks_invalid_claims() {
        for invalid in [
            "id",
            "folder",
            "spawn",
            "supersedes",
            "explicit-owner",
            "shared-process",
        ] {
            let mut owner = native_seat("pij-pane-owner");
            owner.proc = Some(OTHER_HOST);
            let mut claim = native_claim("");
            match invalid {
                "id" => claim.id = " ".into(),
                "folder" => claim.folder = "relative".into(),
                "spawn" => claim.spawn_id = Some("unmatched-spawn".into()),
                "supersedes" => claim.supersedes = Some(owner.id.clone()),
                "explicit-owner" => claim.id = owner.id.to_string(),
                "shared-process" => owner.proc = Some(HOST),
                _ => unreachable!(),
            }
            let (service, registry) = native_service(vec![owner.clone()], native_liveness()).await;
            refused(attest(&service, claim, Some("unknown-session"), &native_tmux()).await);
            assert_eq!(
                registry.list(SeatFilter::default()).await.unwrap(),
                vec![owner]
            );
        }
    }

    #[tokio::test]
    async fn native_transient_hold_requires_local_attested_native_owner() {
        for mismatch in [
            "harness", "relay", "machine", "legacy", "prebind", "session", "process",
        ] {
            let mut owner = native_seat("pij-pane-owner");
            owner.proc = Some(OTHER_HOST);
            match mismatch {
                "harness" => owner.harness = Harness::Pi,
                "relay" => owner.relay = true,
                "machine" => owner.machine = Some("another-machine".into()),
                "legacy" => owner.native_extension_delivery = false,
                "prebind" => owner.proc = None,
                "session" => owner.harness_session = None,
                "process" => owner.proc.as_mut().unwrap().proc_start = 0,
                _ => unreachable!(),
            }
            let (service, registry) = native_service(vec![owner.clone()], native_liveness()).await;
            refused(
                attest(
                    &service,
                    native_claim(""),
                    Some("unknown-session"),
                    &native_tmux(),
                )
                .await,
            );
            assert_eq!(
                registry.list(SeatFilter::default()).await.unwrap(),
                vec![owner]
            );
        }
    }

    #[test]
    fn native_transient_hold_requires_present_matching_pane_at_roster_guard() {
        // Exercise this guard directly: upstream subagent rejection otherwise
        // masks same-process collisions before either pane condition is reached.
        for (claim_pane, owner_pane) in [(None, None), (Some("%151-claim"), Some("%151-owner"))] {
            let mut claim = native_claim("");
            claim.pane = claim_pane.map(str::to_string);
            let mut owner = native_seat("pij-pane-owner");
            owner.pane = owner_pane.map(str::to_string);
            refused(super::check_native_roster(
                &claim,
                Some("unknown-session"),
                Some(HOST),
                &[owner],
                None,
                &[],
                None,
            ));
        }
    }

    #[tokio::test]
    async fn native_known_session_foreign_pane_collision_is_never_a_transient_hold() {
        for historical in [false, true] {
            let mut owner = native_seat("pij-pane-owner");
            owner.proc = Some(OTHER_HOST);
            owner.harness_session = Some("owners-session".into());
            let mut saved = native_seat("pij-saved-address");
            saved.pane = None;
            saved.tombstoned_at = historical.then_some(1);
            let (service, registry) = native_service(vec![owner, saved], native_liveness()).await;
            let before = registry.list(SeatFilter::default()).await.unwrap();
            refused(attest(&service, native_claim(""), Some(SESSION), &native_tmux()).await);
            assert_eq!(registry.list(SeatFilter::default()).await.unwrap(), before);
        }
    }

    #[tokio::test]
    async fn native_refuses_different_session_harness_and_relay_ownership() {
        for mismatch in [1, 2, 4] {
            let mut existing = native_seat("pij-existing");
            let claim = native_claim("pij-existing");
            let mut session = SESSION;
            match mismatch {
                1 => session = "unsolicited-new-session",
                2 => existing.harness = Harness::Pi,
                4 => existing.relay = true,
                _ => unreachable!(),
            }
            let (service, registry) =
                native_service(vec![existing.clone()], native_liveness()).await;
            refused(attest(&service, claim, Some(session), &native_tmux()).await);
            assert_eq!(
                registry.get(&existing.id).await.expect("roster"),
                Some(existing)
            );
        }
    }

    #[tokio::test]
    async fn native_prebind_requires_exact_spawn_id_pane_and_harness() {
        let mut prebind = native_seat("pij-preallocated");
        prebind.proc = None;
        prebind.harness_session = None;
        prebind.native_extension_delivery = false;
        prebind.spawn_id = Some("spawn-137".to_string());
        let mut claim = native_claim("pij-preallocated");
        claim.spawn_id = prebind.spawn_id.clone();
        for mismatch in 0..5 {
            let mut existing = prebind.clone();
            let mut claim = claim.clone();
            match mismatch {
                0 => claim.spawn_id = Some("other-spawn".to_string()),
                1 => claim.spawn_id = None,
                2 => existing.pane = Some("%other".to_string()),
                3 => existing.harness = Harness::Pi,
                4 => {
                    claim.id = "pij-unknown-spawn".to_string();
                    claim.spawn_id = Some("unknown".to_string());
                }
                _ => unreachable!(),
            }
            let (service, registry) =
                native_service(vec![existing.clone()], native_liveness()).await;
            refused(attest(&service, claim, Some(SESSION), &native_tmux()).await);
            assert_eq!(
                registry.get(&existing.id).await.expect("roster"),
                Some(existing)
            );
        }
        let (service, registry) = native_service(vec![prebind.clone()], native_liveness()).await;
        // A runtime-derived request id must not discard the preallocated address.
        claim.id = "pij-runtime-session".to_string();
        let bound = attest(&service, claim, Some(SESSION), &native_tmux())
            .await
            .expect("exact prebind");
        assert_eq!(bound.id, prebind.id);
        assert_eq!(bound.proc, Some(HOST));
        assert!(bound.native_extension_delivery);
        assert_eq!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn native_reconnect_retains_address_and_explicit_new_session_retires_old_capability() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let tmux = native_tmux();
        let first = attest(&service, native_claim(""), Some(SESSION), &tmux)
            .await
            .expect("first");
        assert_eq!(
            first.id,
            pij_core::names::memorable_pij_id_candidates(&format!("copilot\0{SESSION}"))
                .next()
                .expect("memorable name")
        );
        let reconnected = attest(&service, native_claim(""), Some(SESSION), &tmux)
            .await
            .expect("reconnect");
        assert_eq!(first, reconnected);
        let mut next = native_claim("");
        next.supersedes = Some(first.id.clone());
        let successor = attest(
            &service,
            next.clone(),
            Some("user-created-new-session"),
            &tmux,
        )
        .await
        .expect("checked supersedes");
        assert_eq!(
            attest(&service, next, Some("user-created-new-session"), &tmux)
                .await
                .expect("retry after a lost allocation response"),
            successor
        );
        assert_ne!(successor.id, first.id);
        assert_eq!(successor.proc, first.proc);
        assert!(successor.native_extension_delivery);
        let retired = registry
            .get(&first.id)
            .await
            .expect("roster")
            .expect("predecessor retained");
        assert!(retired.tombstoned_at.is_some());
        assert!(!retired.native_extension_delivery);
        assert_eq!(
            attest(
                &service,
                native_claim(successor.id.as_str()),
                Some("user-created-new-session"),
                &tmux
            )
            .await
            .expect("reconnect ignores retired collision"),
            successor,
        );
        let restored = attest(
            &service,
            native_claim(first.id.as_str()),
            Some(SESSION),
            &tmux,
        )
        .await
        .expect("switching back restores the original conversation address");
        assert_eq!(restored.id, first.id);
        assert!(restored.tombstoned_at.is_none());
        assert!(
            registry
                .get(&successor.id)
                .await
                .unwrap()
                .unwrap()
                .tombstoned_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn native_supersedes_refuses_same_id_same_session_wrong_process_or_existing_target() {
        for mismatch in 0..6 {
            let mut old = native_seat("pij-old");
            let mut claim = native_claim("pij-new");
            claim.supersedes = Some(old.id.clone());
            let mut session = "new-session";
            let mut seats = Vec::new();
            match mismatch {
                0 => claim.id = "pij-old".to_string(),
                1 => session = SESSION,
                2 => {
                    claim.pid = Some(OTHER_HOST.pid);
                    claim.proc_start = Some(OTHER_HOST.proc_start);
                }
                3 => old.pane = None,
                4 => old.harness = Harness::Pi,
                5 => seats.push(SeatDescriptor::new(
                    "pij-new",
                    Harness::Copilot,
                    "/isolated/native-registration",
                )),
                _ => unreachable!(),
            }
            seats.push(old.clone());
            let (service, registry) = native_service(seats, native_liveness()).await;
            refused(attest(&service, claim, Some(session), &native_tmux()).await);
            assert_eq!(registry.get(&old.id).await.expect("roster"), Some(old));
        }
    }

    #[tokio::test]
    async fn ordinary_registration_preserves_unchanged_attested_native_owner_and_binding() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let owner = attest(
            &service,
            native_claim("pij-native"),
            Some(SESSION),
            &native_tmux(),
        )
        .await
        .expect("verified native owner");
        for session in [Some(SESSION), None, Some("   ")] {
            let (adopted, binding) = service
                .register_with_harness_session(
                    native_claim("pij-native"),
                    session.map(str::to_string),
                )
                .await
                .expect("unchanged ordinary adoption");
            assert!(adopted.native_extension_delivery);
            assert_eq!(adopted, owner);
            assert!(!binding.inserted);
            assert_eq!(binding.previous_proc, owner.proc);
            assert_eq!(
                registry.get(&owner.id).await.expect("roster"),
                Some(owner.clone())
            );
        }
    }

    #[tokio::test]
    async fn ordinary_registration_refuses_changed_native_owner_after_readoption() {
        for replacement in [
            OTHER_HOST,
            ProcIdentity {
                proc_start: HOST.proc_start + 1,
                ..HOST
            },
        ] {
            let (mut service, registry) = native_service(Vec::new(), native_liveness()).await;
            let owner = attest(
                &service,
                native_claim("pij-native"),
                Some(SESSION),
                &native_tmux(),
            )
            .await
            .expect("verified native owner");
            assert_eq!(
                service.register(native_claim("pij-native")).await.unwrap(),
                owner
            );
            // Both a new PID and a recycled PID require native re-attestation,
            // even when the old exact incarnation is no longer observed alive.
            service.liveness = Arc::new(FakeLiveness::new().with_proc(replacement));
            let mut claim = native_claim("pij-native");
            claim.pid = Some(replacement.pid);
            claim.proc_start = Some(replacement.proc_start);
            let error = service
                .register_with_harness_session(claim, Some(SESSION.to_string()))
                .await
                .expect_err("ordinary registration cannot transfer native ownership");
            assert!(
                error
                    .to_string()
                    .contains("ordinary registration cannot change")
            );
            assert_eq!(
                registry.list(SeatFilter::default()).await.unwrap(),
                vec![owner]
            );
        }
    }

    #[tokio::test]
    async fn ordinary_registration_never_inherits_native_capability() {
        for change in 0..7 {
            let mut old = native_seat("pij-native");
            let mut claim = native_claim("pij-native");
            let mut session = Some(SESSION.to_string());
            match change {
                0 => old.native_extension_delivery = false,
                1 => {
                    claim.pid = Some(OTHER_HOST.pid);
                    claim.proc_start = Some(OTHER_HOST.proc_start);
                }
                2 => session = Some("different-session".to_string()),
                3 => {
                    claim.pid = None;
                    claim.proc_start = None;
                    session = None;
                }
                4 => old.tombstoned_at = Some(1),
                5 => {}
                6 => {
                    old.proc = None;
                    old.native_extension_delivery = false;
                }
                _ => unreachable!(),
            }
            let prior = if change == 5 {
                Vec::new()
            } else {
                vec![old.clone()]
            };
            let (service, registry) = native_service(prior, native_liveness()).await;
            let ordinary = service.register_with_harness_session(claim, session).await;
            if matches!(change, 1..=3) {
                refused(ordinary);
                assert_eq!(registry.get(&old.id).await.expect("roster"), Some(old));
            } else {
                let (registered, binding) = ordinary.expect("ordinary registration without grant");
                assert!(!registered.native_extension_delivery, "case {change}");
                assert_eq!(binding.inserted, change == 5);
                assert_eq!(
                    binding.previous_proc,
                    if change == 5 { None } else { old.proc }
                );
                assert_eq!(registered.tombstoned_at, None);
                assert_eq!(
                    registry.get(&old.id).await.expect("roster"),
                    Some(registered)
                );
            }
        }
    }

    #[tokio::test]
    async fn ordinary_registration_cannot_relabel_or_supersede_active_copilot_seat() {
        for bypass in 0..6 {
            let old = native_seat("pij-native");
            let mut claim = native_claim("pij-native");
            match bypass {
                0 => claim.harness = "omp".to_string(),
                1 => claim.pane = Some("%other".to_string()),
                2 => claim.pane = None,
                3 => claim.relay = true,
                4 => claim.spawn_id = Some("unrelated-spawn".to_string()),
                5 => {
                    claim.id = "pij-legacy-successor".to_string();
                    claim.harness = "omp".to_string();
                    claim.supersedes = Some(old.id.clone());
                }
                _ => unreachable!(),
            }
            let (service, registry) = native_service(vec![old.clone()], native_liveness()).await;
            refused(
                service
                    .register_with_harness_session(claim, Some(SESSION.to_string()))
                    .await,
            );
            assert_eq!(
                registry.list(SeatFilter::default()).await.expect("roster"),
                vec![old]
            );
        }
    }

    #[tokio::test]
    async fn cold_resume_can_retire_same_host_bootstrap_without_changing_saved_address() {
        for change in 0..7 {
            let mut old = native_seat("pij-saved-conversation");
            old.parent = Some("pij-parent".into());
            old.semantic_state = Some(SemanticState::Ready);
            let mut bootstrap = native_seat("pij-bootstrap-conversation");
            bootstrap.proc = Some(OTHER_HOST);
            bootstrap.pane = Some("%resumed".into());
            bootstrap.harness_session = Some("temporary-startup-session".into());
            let mut liveness = FakeLiveness::new().with_proc(SHELL).with_proc(OTHER_HOST);
            match change {
                0 => {}
                1 => liveness = liveness.with_proc(HOST),
                2 => old.native_extension_delivery = false,
                3 => bootstrap.proc = Some(HOST),
                4 => bootstrap.harness = Harness::Omp,
                5 => bootstrap.pane = Some("%other".into()),
                6 => bootstrap.tombstoned_at = Some(1),
                _ => unreachable!(),
            }
            let (service, registry) =
                native_service(vec![old.clone(), bootstrap.clone()], Arc::new(liveness)).await;
            let mut claim = native_claim("");
            claim.pid = Some(OTHER_HOST.pid);
            claim.proc_start = Some(OTHER_HOST.proc_start);
            claim.pane = Some("%resumed".into());
            claim.supersedes = Some(bootstrap.id.clone());
            let tmux = FakeTmux::new().with_pane_process(
                "%resumed",
                PaneProcess {
                    pid: SHELL.pid,
                    cwd: old.folder.clone(),
                },
            );
            let result = attest(&service, claim, Some(SESSION), &tmux).await;
            if change <= 2 {
                let resumed = result.expect("saved session and same-host bootstrap permit resume");
                assert_eq!(resumed.id, old.id);
                assert_eq!(resumed.harness_session, old.harness_session);
                assert_eq!(resumed.parent, old.parent);
                assert_eq!(resumed.semantic_state, old.semantic_state);
                assert_eq!(resumed.proc, Some(OTHER_HOST));
                assert_eq!(resumed.pane.as_deref(), Some("%resumed"));
                assert!(resumed.native_extension_delivery);
                let retired = registry.get(&bootstrap.id).await.unwrap().unwrap();
                assert!(retired.tombstoned_at.is_some());
                assert!(!retired.native_extension_delivery);
            } else {
                refused(result);
                assert_eq!(registry.get(&old.id).await.unwrap(), Some(old));
                assert_eq!(registry.get(&bootstrap.id).await.unwrap(), Some(bootstrap));
            }
        }
    }

    #[tokio::test]
    async fn cold_resume_preserves_operator_hold_even_after_capability_withdrawal() {
        let mut old = native_seat("pij-held-native");
        old.native_extension_delivery = false;
        old.semantic_state = Some(SemanticState::Hold);
        let (service, _) = native_service(vec![old.clone()], native_liveness()).await;
        let mut claim = native_claim("");
        claim.pid = Some(OTHER_HOST.pid);
        claim.proc_start = Some(OTHER_HOST.proc_start);
        claim.pane = None;
        let resumed = attest(&service, claim, Some(SESSION), &native_tmux())
            .await
            .unwrap();
        assert_eq!(resumed.id, old.id);
        assert_eq!(resumed.semantic_state, Some(SemanticState::Hold));
    }

    struct PendingDeadOwner {
        entered: Notify,
        release: Notify,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl LivenessPort for PendingDeadOwner {
        async fn proc_start(&self, pid: u32) -> pij_core::error::Result<Option<u64>> {
            if pid == HOST.pid {
                self.entered.notify_one();
                self.release.notified().await;
                return if self.fail {
                    Err(pij_core::error::PijError::Adapter {
                        adapter: "test/previous-native-process".into(),
                        message: "owner observation denied".into(),
                    })
                } else {
                    Ok(None)
                };
            }
            Ok(match pid {
                pid if pid == OTHER_HOST.pid => Some(OTHER_HOST.proc_start),
                pid if pid == SHELL.pid => Some(SHELL.proc_start),
                _ => None,
            })
        }
    }

    #[tokio::test]
    async fn ordinary_withdrawal_preserves_session_and_does_not_enable_later_takeover() {
        for bypass in 0..5 {
            let mut old = native_seat("pij-native");
            old.native_extension_delivery = false;
            let (service, registry) = native_service(vec![old], native_liveness()).await;
            let withdrawn = service
                .register(native_claim("pij-native"))
                .await
                .expect("ordinary registration cannot revive withdrawn capability");
            assert_eq!(withdrawn.harness, Harness::Copilot);
            assert_eq!(withdrawn.harness_session.as_deref(), Some(SESSION));
            assert!(!withdrawn.native_extension_delivery);
            let mut claim = native_claim("pij-native");
            let mut session = Some(SESSION.to_string());
            match bypass {
                0 => claim.harness = "omp".to_string(),
                1 => {
                    claim.pid = Some(OTHER_HOST.pid);
                    claim.proc_start = Some(OTHER_HOST.proc_start);
                }
                2 => claim.pane = None,
                3 => session = Some("unattested-new-session".to_string()),
                4 => {
                    claim.id = "pij-legacy-successor".to_string();
                    claim.supersedes = Some(withdrawn.id.clone());
                }
                _ => unreachable!(),
            }
            refused(service.register_with_harness_session(claim, session).await);
            assert_eq!(
                registry.list(SeatFilter::default()).await.expect("roster"),
                vec![withdrawn]
            );
        }
    }

    /// Plan 158 review MEDIUM-3: `working` belongs to the process that published
    /// it. A new incarnation binding the seat starts idle; the same process
    /// re-registering keeps its turn.
    #[tokio::test]
    async fn a_new_incarnation_does_not_inherit_working() {
        for (claimed, expected) in [
            (OTHER_HOST, pij_core::model::SystemState::Idle),
            (HOST, pij_core::model::SystemState::Working),
        ] {
            let mut old =
                SeatDescriptor::new("pij-busy", Harness::Omp, "/isolated/native-registration");
            old.proc = Some(HOST);
            old.state = pij_core::model::SystemState::Working;
            let mut claim = native_claim("pij-busy");
            claim.harness = Harness::Omp.as_str().to_string();
            claim.pid = Some(claimed.pid);
            claim.proc_start = Some(claimed.proc_start);
            let (service, _) = native_service(vec![old], native_liveness()).await;
            let (rebound, _) = service
                .register_with_harness_session(claim, Some("legacy-session".to_string()))
                .await
                .expect("rebind");
            assert_eq!(rebound.state, expected, "claimed {claimed:?}");
        }
    }

    #[tokio::test]
    async fn ordinary_other_harness_registration_retains_existing_rebind_behavior() {
        for harness in [Harness::Pi, Harness::Omp, Harness::Claude] {
            let mut old =
                SeatDescriptor::new("pij-ordinary", harness, "/isolated/native-registration");
            old.proc = Some(HOST);
            let mut claim = native_claim("pij-ordinary");
            claim.harness = harness.as_str().to_string();
            claim.pid = Some(OTHER_HOST.pid);
            claim.proc_start = Some(OTHER_HOST.proc_start);
            let (service, _) = native_service(vec![old], native_liveness()).await;
            let (rebound, _) = service
                .register_with_harness_session(claim, Some("legacy-session".to_string()))
                .await
                .expect("other harness ordinary rebind unchanged");
            assert_eq!(rebound.proc, Some(OTHER_HOST));
            assert_eq!(rebound.harness, harness);
            assert!(!rebound.native_extension_delivery);
        }
    }

    struct MovingStart(AtomicU64);

    #[async_trait::async_trait]
    impl LivenessPort for MovingStart {
        async fn proc_start(&self, _pid: u32) -> pij_core::error::Result<Option<u64>> {
            Ok(Some(self.0.load(Ordering::SeqCst)))
        }
    }

    #[tokio::test]
    async fn native_rechecks_process_start_after_actual_host_observation() {
        let liveness = Arc::new(MovingStart(AtomicU64::new(HOST.proc_start)));
        let (service, registry) = native_service(Vec::new(), liveness.clone()).await;
        let mut claim = native_claim("pij-native");
        claim.pane = None;
        let result = service
            .register_native_with_observer(
                claim,
                Some(SESSION.to_string()),
                &FakeTmux::new(),
                |pid| {
                    liveness.0.store(HOST.proc_start + 1, Ordering::SeqCst);
                    fixture_process(pid)
                },
            )
            .await;
        assert!(
            result
                .expect_err("reused pid")
                .to_string()
                .contains("process start changed")
        );
        assert!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn native_host_observation_has_a_wall_clock_bound() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        // SQL fixture initialization runs in real time; only the pending host probe is virtual.
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let result = service
            .register_native_with_observer(
                native_claim("pij-native"),
                Some(SESSION.to_string()),
                &native_tmux(),
                |_| {
                    std::future::pending::<
                        std::result::Result<Option<NativeProcess>, RegistrationError>,
                    >()
                },
            )
            .await;
        let error = result.expect_err("bounded observation");
        // Tokio resolves virtual deadlines at millisecond timer precision.
        assert!(
            (Duration::from_secs(5)..=Duration::from_millis(5001)).contains(&started.elapsed())
        );
        assert!(error.to_string().contains(&format!("pid {}", HOST.pid)));
        assert!(matches!(error, RegistrationError::Retryable(_)));
        assert!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_ps_admits_a_delayed_observation_without_weakening_identity() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let mut claim = native_claim("pij-slow-observer");
        claim.pane = None;
        let result = service
            .register_native_with_observer(
                claim,
                Some(SESSION.into()),
                &FakeTmux::new(),
                |pid| async move {
                    tokio::task::spawn_blocking(move || {
                        let mut ps = std::process::Command::new("/bin/sh");
                        let delay = super::PROCESS_OBSERVATION_TIMEOUT.mul_f64(0.3);
                        ps.args([
                            "-c",
                            &format!(
                                "/bin/sleep {}; printf '41 copilot /opt/copilot\\n'",
                                delay.as_secs_f64()
                            ),
                        ]);
                        super::read_native_process_using(pid, &mut ps)
                    })
                    .await
                    .unwrap()
                },
            )
            .await;
        let (seat, _) = result.expect("delayed ps response must fit the bounded host observation");
        assert_eq!(seat.proc, Some(HOST));
        assert!(seat.native_extension_delivery);
        assert_eq!(
            registry.list(SeatFilter::default()).await.unwrap(),
            vec![seat]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_ps_never_answering_refuses_with_pid_within_total_bound() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let mut claim = native_claim("pij-stalled-observer");
        claim.pane = None;
        let started = std::time::Instant::now();
        let error = service
            .register_native_with_observer(
                claim,
                Some(SESSION.into()),
                &FakeTmux::new(),
                |pid| async move {
                    tokio::task::spawn_blocking(move || {
                        let mut ps = std::process::Command::new("/bin/sh");
                        ps.args(["-c", "exec /bin/sleep 30"]);
                        super::read_native_process_using(pid, &mut ps)
                    })
                    .await
                    .unwrap()
                },
            )
            .await
            .expect_err("never-answering ps must refuse");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            error.to_string().contains(&format!("pid {}", HOST.pid)),
            "{error}"
        );
        assert!(matches!(error, RegistrationError::Retryable(_)));
        assert!(
            registry
                .list(SeatFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn native_ancestry_observation_is_bounded_to_32_processes() {
        let mut liveness = FakeLiveness::new().with_proc(ProcIdentity {
            pid: 4000,
            proc_start: 1,
        });
        for pid in 4200..=HOST.pid {
            liveness = liveness.with_proc(ProcIdentity {
                pid,
                proc_start: u64::from(pid - 4116),
            });
        }
        let (service, registry) = native_service(Vec::new(), Arc::new(liveness)).await;
        let tmux = FakeTmux::new().with_pane_process(
            "%137",
            PaneProcess {
                pid: 4000,
                cwd: "/isolated/native-registration".to_string(),
            },
        );
        let calls = AtomicU64::new(0);
        let result = service
            .register_native_with_observer(
                native_claim("pij-native"),
                Some(SESSION.to_string()),
                &tmux,
                |pid| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    ready(Ok(Some(NativeProcess {
                        parent: pid - 1,
                        executable: "/opt/copilot/copilot".to_string(),
                        executable_replaced: false,
                        argv: vec!["copilot".to_string()],
                    })))
                },
            )
            .await;
        assert!(
            result
                .expect_err("bounded ancestry")
                .to_string()
                .contains("32 processes")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 32);
        assert!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn blocked_native_host_observation_does_not_block_unrelated_inbox_ack() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let queue = Arc::new(FakeQueue::new(1_024).expect("queue"));
        let delivery = Arc::new(
            DeliveryService::new(
                registry.clone(),
                queue.clone(),
                Arc::new(FakeTransport::unreachable()),
                Arc::new(InteractionGate::new(Arc::new(FakeTmux::new()))),
                service.event_bus.clone(),
            )
            .expect("delivery service"),
        );
        let service = service.with_native_lock(delivery.native_lock());
        let entered = Notify::new();
        let release = Notify::new();
        let tmux = native_tmux();
        let pending = service.register_native_with_observer(
            native_claim("pij-observing"),
            Some(SESSION.to_string()),
            &tmux,
            |pid| {
                let entered = &entered;
                let release = &release;
                async move {
                    if pid == HOST.pid {
                        entered.notify_one();
                        release.notified().await;
                    }
                    fixture_process(pid).await
                }
            },
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = entered.notified() => {}
            result = &mut pending => panic!("observer did not block: {result:?}"),
        }

        for harness in [Harness::Omp, Harness::Pi] {
            let reader = SeatDescriptor::new(
                format!("pij-reader-{}", harness.as_str()),
                harness,
                "/isolated/reader",
            );
            registry.put(reader.clone()).await.expect("register reader");
            delivery
                .send("pij-peer".into(), reader.id.clone(), "unrelated")
                .await
                .expect("enqueue");
            tokio::time::timeout(Duration::from_secs(1), async {
                let claims = delivery
                    .claim_inbox(&reader.id, false)
                    .await
                    .expect("claim");
                assert_eq!(claims.len(), 1);
                delivery
                    .acknowledge_inbox(&reader.id, claims[0].job_id, &Default::default(), None)
                    .await
                    .expect("ack");
                assert!(
                    queue
                        .claimed_delivery(claims[0].job_id)
                        .await
                        .expect("queue")
                        .is_none()
                );
            })
            .await
            .expect("unrelated reader progresses before native observer is released");
        }
        release.notify_one();
        assert!(
            pending
                .await
                .expect("native observation resumes")
                .0
                .native_extension_delivery
        );
    }

    #[tokio::test]
    async fn native_saved_resume_cannot_borrow_unrelated_spawn_intent() {
        let mut saved = native_seat("pij-saved");
        saved.proc = Some(OTHER_HOST);
        saved.pane = None;
        let mut unrelated = native_seat("pij-foreign-prebind");
        unrelated.proc = None;
        unrelated.pane = Some("%other-pane".into());
        unrelated.harness_session = None;
        unrelated.spawn_id = Some("unrelated-spawn".into());
        unrelated.native_extension_delivery = false;
        let (service, registry) =
            native_service(vec![saved.clone(), unrelated.clone()], native_liveness()).await;
        let before = registry.list(SeatFilter::default()).await.unwrap();
        let mut claim = native_claim("");
        claim.spawn_id = unrelated.spawn_id;
        refused(attest(&service, claim, Some(SESSION), &native_tmux()).await);
        assert_eq!(registry.list(SeatFilter::default()).await.unwrap(), before);
    }

    #[tokio::test]
    async fn native_observed_owner_drift_returns_retryable_without_retirement() {
        for change_prebind in [false, true] {
            let old = native_seat("pij-observed-owner");
            let mut prebind = native_seat("pij-observed-prebind");
            prebind.proc = None;
            prebind.harness_session = None;
            prebind.native_extension_delivery = false;
            prebind.spawn_id = Some("spawn-before-observation".into());
            let liveness = Arc::new(PendingDeadOwner {
                entered: Notify::new(),
                release: Notify::new(),
                fail: false,
            });
            let (service, registry) =
                native_service(vec![old.clone(), prebind.clone()], liveness.clone()).await;
            let mut claim = native_claim(prebind.id.as_str());
            claim.pid = Some(OTHER_HOST.pid);
            claim.proc_start = Some(OTHER_HOST.proc_start);
            claim.spawn_id = prebind.spawn_id.clone();
            let tmux = native_tmux();
            let pending = attest(&service, claim, Some("next-session"), &tmux);
            tokio::pin!(pending);
            tokio::select! {
                _ = liveness.entered.notified() => {}
                result = &mut pending => panic!("death observation did not block: {result:?}"),
            }
            let mut changed = if change_prebind { prebind } else { old };
            changed.spawn_id = Some("spawn-after-observation".into());
            registry.put(changed).await.unwrap();
            let before = registry.list(SeatFilter::default()).await.unwrap();
            liveness.release.notify_one();
            let result = pending.await;
            assert!(
                matches!(result, Err(RegistrationError::Retryable(_))),
                "{result:?}"
            );
            assert_eq!(registry.list(SeatFilter::default()).await.unwrap(), before);
        }
    }

    #[tokio::test]
    async fn native_bootstrap_other_pane_refusal_identifies_conflicting_address() {
        let mut saved = native_seat("pij-saved-session");
        saved.proc = Some(OTHER_HOST);
        saved.pane = None;
        saved.tombstoned_at = Some(1);
        let mut other_pane = native_seat("pij-bootstrap-other-pane");
        other_pane.pane = Some("%other-pane".into());
        other_pane.harness_session = Some("bootstrap".into());
        let (service, registry) =
            native_service(vec![saved.clone(), other_pane.clone()], native_liveness()).await;
        let before = registry.list(SeatFilter::default()).await.unwrap();
        let result = attest(&service, native_claim(""), Some(SESSION), &native_tmux()).await;
        let Err(RegistrationError::Refused(reason)) = result else {
            panic!("unrelated pane ownership must refuse: {result:?}");
        };
        // Report the address blocking admission, not an invented supersedes
        // intent. This checks a dynamic identity, not fixed error prose.
        assert!(reason.contains(other_pane.id.as_str()), "{reason}");
        assert_eq!(registry.list(SeatFilter::default()).await.unwrap(), before);
    }

    #[tokio::test]
    async fn native_historical_session_tie_uses_id_only_after_incarnation() {
        let mut oldest = native_seat("pij-a-oldest");
        oldest.tombstoned_at = Some(1);
        let mut recent_z = oldest.clone();
        recent_z.id = "pij-z-recent".into();
        recent_z.proc.as_mut().unwrap().proc_start += 1;
        let mut recent_y = recent_z.clone();
        recent_y.id = "pij-y-recent".into();
        let (service, registry) = native_service(
            vec![recent_z.clone(), oldest.clone(), recent_y.clone()],
            native_liveness(),
        )
        .await;
        let resumed = attest(&service, native_claim(""), Some(SESSION), &native_tmux())
            .await
            .expect("historical duplicates do not compete");
        assert_eq!(resumed.id, recent_y.id);
        assert_eq!(resumed.tombstoned_at, None);
        assert_eq!(registry.get(&oldest.id).await.unwrap(), Some(oldest));
        assert_eq!(registry.get(&recent_z.id).await.unwrap(), Some(recent_z));
    }

    #[tokio::test]
    async fn native_observation_revalidates_cross_seat_process_takeover_before_commit() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let entered = Notify::new();
        let release = Notify::new();
        let tmux = FakeTmux::new();
        let mut first_claim = native_claim("pij-delayed");
        first_claim.pane = None;
        let pending = service.register_native_with_observer(
            first_claim,
            Some("delayed-session".to_string()),
            &tmux,
            |pid| {
                let entered = &entered;
                let release = &release;
                async move {
                    entered.notify_one();
                    release.notified().await;
                    fixture_process(pid).await
                }
            },
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = entered.notified() => {}
            result = &mut pending => panic!("observer did not block: {result:?}"),
        }
        // Different seat AND session, no pane: only process ownership conflicts.
        let mut winner_claim = native_claim("pij-winner");
        winner_claim.pane = None;
        let winner = tokio::time::timeout(
            Duration::from_secs(1),
            attest(&service, winner_claim, Some("winner-session"), &tmux),
        )
        .await
        .expect("winner is not blocked by observation")
        .expect("winner registers");
        release.notify_one();
        refused(pending.await);
        assert_eq!(
            registry.list(SeatFilter::default()).await.expect("roster"),
            vec![winner]
        );
    }

    #[tokio::test]
    async fn shared_native_lock_converges_concurrent_claims_for_the_same_session() {
        let (service, registry) = native_service(Vec::new(), native_liveness()).await;
        let other = service.clone();
        let tmux = native_tmux();
        let (first, second) = tokio::join!(
            attest(&service, native_claim("pij-first"), Some(SESSION), &tmux),
            attest(&other, native_claim("pij-second"), Some(SESSION), &tmux),
        );
        assert_eq!(first.unwrap().id, second.unwrap().id);
        assert_eq!(
            registry
                .list(SeatFilter::default())
                .await
                .expect("roster")
                .len(),
            1
        );
    }

    #[test]
    fn absent_actual_model_is_unknown_while_explicit_no_model_is_a_mismatch() {
        assert!(requested_model_matches(Some("claude-opus-5"), None, false));
        assert!(!requested_model_matches(
            Some("github-copilot/gpt-5.6-sol"),
            None,
            true,
        ));
    }

    #[tokio::test]
    async fn claude_inbound_registration_changes_once_and_publishes_once() {
        let root = pij_testkit::fresh_dir("pij-claude-inbound-registration");
        let home = root.join(".claude");
        fs::create_dir_all(&home).expect("Claude home");
        fs::write(
            home.join("settings.json"),
            r#"{"crossSessionInbound":"hold","keep":true}"#,
        )
        .expect("seed settings");
        let registry = Arc::new(FakeRegistry::new());
        let liveness = Arc::new(
            FakeLiveness::new().with_proc(pij_core::model::ProcIdentity {
                pid: 4242,
                proc_start: 126,
            }),
        );
        let spine = Arc::new(FakeSpine::new());
        let event_bus =
            Arc::new(EventBus::new(Arc::clone(&spine) as Arc<dyn Spine>, 8).expect("event bus"));
        let pool = pij_store::open("").await.expect("isolated in-memory roles");
        let roles = Arc::new(RoleService::new(
            registry.clone(),
            pij_store::SqliteOrchestration::new(pool),
            event_bus.clone(),
        ));
        let service =
            RegistrationService::new(registry, liveness, event_bus, vec![home.clone()], roles);
        let claim = Registration {
            supersedes: None,
            id: "pij-claude-config".to_string(),
            harness: Harness::Claude.as_str().to_string(),
            folder: root.display().to_string(),
            extension_build: None,
            extension_path: None,
            pane: None,
            pid: Some(4242),
            proc_start: Some(126),
            spawn_id: None,
            model: None,
            actual_model: None,
            actual_model_observed: false,
            provider: None,
            effort: None,
            parent: None,
            role: None,
            relay: false,
        };

        service
            .register(claim.clone())
            .await
            .expect("first registration");
        service.register(claim).await.expect("second registration");

        let settings: serde_json::Value = serde_json::from_slice(
            &fs::read(home.join("settings.json")).expect("settings readable"),
        )
        .expect("valid settings");
        assert_eq!(settings["crossSessionInbound"], "accept");
        let events = spine.tail(None, Seq(0)).await.expect("spine tail");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "config.claude-inbound-ensured");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&events[0].payload).expect("event payload")["after"],
            "accept"
        );
        let _ = fs::remove_dir_all(root);
    }

    const PANELLESS_CLI: ProcIdentity = ProcIdentity {
        pid: 9000,
        proc_start: 200,
    };
    const PANELLESS_SHELL: ProcIdentity = ProcIdentity {
        pid: 9001,
        proc_start: 180,
    };
    const PANELLESS_NEXT_CLI: ProcIdentity = ProcIdentity {
        pid: 9002,
        proc_start: 210,
    };

    fn paneless_claim(harness: Harness) -> Registration {
        let mut claim = native_claim("");
        claim.harness = harness.as_str().to_string();
        claim.pane = None;
        claim.pid = Some(PANELLESS_CLI.pid);
        claim.proc_start = Some(PANELLESS_CLI.proc_start);
        claim
    }

    fn paneless_liveness() -> Arc<dyn LivenessPort> {
        Arc::new(
            FakeLiveness::new()
                .with_proc(HOST)
                .with_proc(PANELLESS_CLI)
                .with_proc(PANELLESS_SHELL)
                .with_proc(PANELLESS_NEXT_CLI),
        )
    }

    fn paneless_process(pid: u32, harness: Harness, node: bool) -> Option<NativeProcess> {
        if pid == PANELLESS_CLI.pid || pid == PANELLESS_NEXT_CLI.pid {
            return Some(NativeProcess {
                parent: PANELLESS_SHELL.pid,
                executable: "/opt/pij/pij".into(),
                executable_replaced: false,
                argv: vec!["/opt/pij/pij".into(), "inbox".into()],
            });
        }
        if pid == PANELLESS_SHELL.pid {
            return Some(NativeProcess {
                parent: HOST.pid,
                executable: "/bin/sh".into(),
                executable_replaced: false,
                argv: vec!["sh".into(), "-c".into(), "copilot claude codex".into()],
            });
        }
        if pid != HOST.pid {
            return None;
        }
        let executable = if node {
            "/usr/bin/node".to_string()
        } else {
            format!("/opt/bin/{}", harness.as_str())
        };
        let mut argv = vec![executable.clone()];
        if node {
            argv.push(format!(
                "/private/fixture/node_modules/{}",
                match harness {
                    Harness::Claude => "@anthropic-ai/claude-code/cli.js",
                    Harness::Copilot => "@github/copilot/index.js",
                    Harness::Codex => "@openai/codex/bin/codex.js",
                    _ => unreachable!("external harness fixture"),
                }
            ));
        }
        Some(NativeProcess {
            parent: 0,
            executable,
            executable_replaced: false,
            argv,
        })
    }

    async fn pull_register(
        service: &RegistrationService,
        claim: Registration,
        session: &str,
        node: bool,
    ) -> std::result::Result<(SeatDescriptor, pij_core::ports::PutBinding), RegistrationError> {
        let harness = Harness::parse(&claim.harness).expect("fixture harness");
        service
            .register_paneless_with_observer(claim, Some(session.into()), |pid| {
                ready(Ok(paneless_process(pid, harness, node)))
            })
            .await
    }

    async fn retired_paneless_resume_contract(harness: Harness, same_host: bool) {
        let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
        let mut claim = paneless_claim(harness);
        claim.parent = Some("pij-original-parent".into());
        claim.role = Some("worker".into());
        let (first, _) = pull_register(&service, claim.clone(), SESSION, false)
            .await
            .unwrap();
        let mut old = registry.get(&first.id).await.unwrap().unwrap();
        old.tombstoned_at = Some(154);
        old.tombstone_reason = Some("observed-dead".into());
        if !same_host {
            old.proc = Some(ProcIdentity {
                pid: 9_999,
                proc_start: 1,
            });
        }
        registry.put(old.clone()).await.unwrap();
        claim.parent = Some("pij-unrelated-parent".into());
        claim.role = Some("pm".into());
        let (resumed, binding) = pull_register(&service, claim, SESSION, false)
            .await
            .expect("a returning paneless host is not its own subagent");
        assert_eq!(
            resumed.id, first.id,
            "{harness:?}: retired address was abandoned"
        );
        assert!(!binding.inserted);
        assert_eq!(resumed.tombstoned_at, None);
        assert_eq!(resumed.tombstone_reason, None);
        assert_eq!(resumed.harness, harness);
        assert_eq!(resumed.proc, Some(HOST));
        assert_eq!(resumed.parent, old.parent);
        assert_eq!(resumed.role.as_deref(), Some("worker"));
        assert_eq!(
            registry
                .get(&first.id)
                .await
                .unwrap()
                .unwrap()
                .tombstoned_at,
            None
        );
        assert_eq!(registry.list(SeatFilter::default()).await.unwrap().len(), 1);
        let events = service
            .event_bus
            .raw_spine()
            .tail(Some(&first.id), Seq(0))
            .await
            .unwrap();
        let resumes: Vec<_> = events
            .iter()
            .filter(|event| matches!(event.kind.as_str(), "seat.resumed" | "seat.native-resumed"))
            .collect();
        assert_eq!(resumes.len(), 1);
        assert_eq!(resumes[0].kind, "seat.resumed");
    }

    #[tokio::test]
    async fn retired_paneless_claude_reuses_dead_host_address() {
        retired_paneless_resume_contract(Harness::Claude, false).await;
    }

    #[tokio::test]
    async fn retired_paneless_codex_reuses_dead_host_address() {
        retired_paneless_resume_contract(Harness::Codex, false).await;
    }

    #[tokio::test]
    async fn retired_paneless_copilot_reuses_dead_host_address() {
        retired_paneless_resume_contract(Harness::Copilot, false).await;
    }

    #[tokio::test]
    async fn retired_paneless_claude_reuses_same_host_address() {
        retired_paneless_resume_contract(Harness::Claude, true).await;
    }

    #[tokio::test]
    async fn retired_paneless_codex_reuses_same_host_address() {
        retired_paneless_resume_contract(Harness::Codex, true).await;
    }

    #[tokio::test]
    async fn retired_paneless_copilot_reuses_same_host_address() {
        retired_paneless_resume_contract(Harness::Copilot, true).await;
    }

    #[tokio::test]
    async fn retired_paneless_foreign_session_names_tombstone_without_writes() {
        for harness in [Harness::Claude, Harness::Codex, Harness::Copilot] {
            let mut old = SeatDescriptor::new(
                "pij-retired-paneless",
                harness,
                "/isolated/native-registration",
            );
            old.proc = Some(HOST);
            old.harness_session = Some(SESSION.into());
            old.tombstoned_at = Some(154);
            old.tombstone_reason = Some("observed-dead".into());
            let (service, registry) = native_service(vec![old.clone()], paneless_liveness()).await;
            let before = service
                .event_bus
                .raw_spine()
                .tail(None, Seq(0))
                .await
                .unwrap();
            let error = pull_register(&service, paneless_claim(harness), "foreign-session", false)
                .await
                .expect_err("paneless session ownership remains stronger than process continuity");
            assert_eq!(
                error.to_string(),
                "seat pij-retired-paneless is retired (observed-dead, 154); a different session may not take it",
                "{harness:?}"
            );
            assert_eq!(registry.get(&old.id).await.unwrap(), Some(old));
            assert_eq!(
                service
                    .event_bus
                    .raw_spine()
                    .tail(None, Seq(0))
                    .await
                    .unwrap(),
                before
            );
        }
    }

    #[tokio::test]
    async fn retired_paneless_selection_prefers_live_then_newest_then_id() {
        for harness in [Harness::Claude, Harness::Codex, Harness::Copilot] {
            let mut older =
                SeatDescriptor::new("pij-older", harness, "/isolated/native-registration");
            older.proc = Some(HOST);
            older.harness_session = Some(SESSION.into());
            older.tombstoned_at = Some(154);
            older.tombstone_reason = Some("observed-dead".into());
            let mut newer = older.clone();
            newer.id = "pij-newer".into();
            newer.proc = Some(ProcIdentity {
                proc_start: HOST.proc_start + 1,
                ..HOST
            });
            let mut tied = newer.clone();
            tied.id = "pij-a-tie".into();
            let mut live = older.clone();
            live.id = "pij-live".into();
            live.tombstoned_at = None;
            live.tombstone_reason = None;
            for (seats, expected) in [
                (vec![older.clone(), newer.clone(), live], "pij-live"),
                (vec![older, newer.clone()], "pij-newer"),
                (vec![newer, tied], "pij-a-tie"),
            ] {
                let (service, registry) = native_service(seats.clone(), paneless_liveness()).await;
                let (selected, _) = pull_register(
                    &service,
                    paneless_claim(harness),
                    SESSION,
                    false,
                )
                .await
                .expect(
                    "historical process aliases cannot make the selected seat its own subagent",
                );
                assert_eq!(selected.id.as_str(), expected, "{harness:?}");
                assert_eq!(selected.tombstoned_at, None);
                for historical in seats.into_iter().filter(|seat| seat.id != selected.id) {
                    assert_eq!(
                        registry.get(&historical.id).await.unwrap(),
                        Some(historical)
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn paneless_all_external_harnesses_bind_actual_host_not_cli_or_shell() {
        for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            for node in [false, true] {
                let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
                let (seat, binding) =
                    pull_register(&service, paneless_claim(harness), SESSION, node)
                        .await
                        .expect("verified external ancestor");
                let expected = pij_core::names::memorable_pij_id_candidates(&format!(
                    "{}\0{SESSION}",
                    harness.as_str()
                ))
                .next()
                .expect("canonical candidate");
                assert_eq!(seat.id, expected);
                assert_eq!(seat.harness, harness);
                assert_eq!(seat.proc, Some(HOST));
                assert_eq!(seat.harness_session.as_deref(), Some(SESSION));
                assert_eq!(seat.pane, None);
                assert!(!seat.relay);
                assert!(!seat.native_extension_delivery);
                assert!(binding.inserted);
                assert_eq!(binding.previous_proc, None);
                assert_eq!(registry.get(&seat.id).await.unwrap(), Some(seat));
            }
        }
    }

    #[test]
    fn paneless_host_matcher_rejects_shell_text_forged_argv_and_unrecognized_node_entries() {
        for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            let mut process = paneless_process(HOST.pid, harness, false).unwrap();
            process.executable = "/bin/sh".into();
            assert!(!process.is_external_host(harness));
            process = paneless_process(HOST.pid, harness, false).unwrap();
            process.argv[0] = "sh".into();
            assert!(!process.is_external_host(harness));
            for entry in [
                "-e",
                "/tmp/extension.mjs",
                "/tmp/npm-loader.js",
                "/tmp/index.js",
            ] {
                process = paneless_process(HOST.pid, harness, true).unwrap();
                process.argv[1] = entry.into();
                assert!(!process.is_external_host(harness));
            }
            process = paneless_process(HOST.pid, harness, true).unwrap();
            process.argv[1] = process.argv[1].replace("node_modules/@", "node_modules/not-@");
            assert!(!process.is_external_host(harness));
        }
    }

    #[tokio::test]
    async fn paneless_refuses_missing_or_wrong_harness_ancestor_without_writes() {
        for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            for missing in [false, true] {
                let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
                refused(
                    service
                        .register_paneless_with_observer(
                            paneless_claim(harness),
                            Some(SESSION.into()),
                            |pid| {
                                let mut process = paneless_process(pid, harness, false);
                                if pid == HOST.pid {
                                    if missing {
                                        process = None;
                                    } else {
                                        process.as_mut().unwrap().executable = "/bin/sh".into();
                                    }
                                }
                                ready(Ok(process))
                            },
                        )
                        .await,
                );
                assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
            }
        }
    }

    #[tokio::test]
    async fn paneless_requires_empty_id_exact_session_complete_tuple_and_pull_shape() {
        let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
        for session in [
            None,
            Some(""),
            Some(" "),
            Some(" session"),
            Some("session\n"),
        ] {
            refused(
                service
                    .register_with_harness_session(
                        paneless_claim(Harness::Copilot),
                        session.map(str::to_string),
                    )
                    .await,
            );
        }
        for case in 0..11 {
            let mut claim = paneless_claim(Harness::Copilot);
            match case {
                0 => claim.id = "pij-unverified-name".into(),
                1 => claim.pane = Some("%137".into()),
                2 => claim.relay = true,
                3 => claim.spawn_id = Some("spawn".into()),
                4 => claim.supersedes = Some("pij-old".into()),
                5 => claim.pid = None,
                6 => claim.proc_start = None,
                7 => claim.pid = Some(0),
                8 => claim.proc_start = Some(0),
                9 => claim.harness = "omp".into(),
                10 => claim.role = Some(" ".into()),
                _ => unreachable!(),
            }
            refused(
                service
                    .register_paneless_with_observer(claim, Some(SESSION.into()), |pid| {
                        ready(Ok(paneless_process(pid, Harness::Copilot, false)))
                    })
                    .await,
            );
        }
        assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
    }

    #[test]
    fn paneless_observed_session_argv_must_match_including_claude_fork_identity() {
        for (harness, flag) in [
            (Harness::Claude, "--session-id"),
            (Harness::Claude, "--resume"),
            (Harness::Claude, "-r"),
            (Harness::Copilot, "--session-id"),
            (Harness::Copilot, "--resume"),
            (Harness::Codex, "resume"),
        ] {
            for node in [false, true] {
                let mut process = paneless_process(HOST.pid, harness, node).unwrap();
                process.argv.extend([flag.into(), SESSION.into()]);
                assert!(process.matches_external_session(harness, SESSION));
                assert!(!process.matches_external_session(harness, "another-session"));
                if flag.starts_with("--") {
                    process.argv.pop();
                    *process.argv.last_mut().unwrap() = format!("{flag}={SESSION}");
                    assert!(process.matches_external_session(harness, SESSION));
                    assert!(!process.matches_external_session(harness, "another-session"));
                }
            }
        }
        let mut process = paneless_process(HOST.pid, Harness::Claude, false).unwrap();
        process.argv.extend(
            [
                "--resume",
                "source-session",
                "--fork-session",
                "--session-id",
                SESSION,
            ]
            .into_iter()
            .map(str::to_string),
        );
        assert!(process.matches_external_session(Harness::Claude, SESSION));
        assert!(!process.matches_external_session(Harness::Claude, "source-session"));
        process.argv.truncate(2);
        assert!(
            process.matches_external_session(Harness::Claude, SESSION),
            "bare resume picker"
        );
        for flag in ["--continue", "-c"] {
            let mut process = paneless_process(HOST.pid, Harness::Claude, false).unwrap();
            process
                .argv
                .extend([flag.into(), "unrelated prompt text".into()]);
            assert!(
                process.matches_external_session(Harness::Claude, SESSION),
                "boolean continue does not make prompt text session evidence"
            );
        }
    }

    #[tokio::test]
    async fn paneless_session_argv_conflict_refuses_before_registration() {
        for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
            refused(
                service
                    .register_paneless_with_observer(
                        paneless_claim(harness),
                        Some(SESSION.into()),
                        |pid| {
                            let mut process = paneless_process(pid, harness, false);
                            if pid == HOST.pid {
                                process.as_mut().unwrap().argv.extend([
                                    if harness == Harness::Codex {
                                        "resume"
                                    } else {
                                        "--session-id"
                                    }
                                    .into(),
                                    "conflicting-session".into(),
                                ]);
                            }
                            ready(Ok(process))
                        },
                    )
                    .await,
            );
            assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
        }
    }

    #[tokio::test]
    async fn paneless_rechecks_incarnation_after_observation_and_lock_wait() {
        for during_lock in [false, true] {
            let liveness = Arc::new(MovingStart(AtomicU64::new(HOST.proc_start)));
            let (service, registry) = native_service(Vec::new(), liveness.clone()).await;
            let mut claim = paneless_claim(Harness::Codex);
            claim.pid = Some(HOST.pid);
            claim.proc_start = Some(HOST.proc_start);
            let observed = Notify::new();
            let guard = if during_lock {
                Some(service.registration_lock.lock().await)
            } else {
                None
            };
            let pending =
                service.register_paneless_with_observer(claim, Some(SESSION.into()), |pid| {
                    observed.notify_one();
                    if !during_lock {
                        liveness.0.store(HOST.proc_start + 1, Ordering::SeqCst);
                    }
                    ready(Ok(paneless_process(pid, Harness::Codex, false)))
                });
            tokio::pin!(pending);
            if during_lock {
                tokio::select! {
                    _ = observed.notified() => {}
                    result = &mut pending => panic!("registration must wait for the lock: {result:?}"),
                }
                liveness.0.store(HOST.proc_start + 1, Ordering::SeqCst);
            }
            drop(guard);
            refused(pending.await);
            assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
        }
    }

    #[tokio::test]
    async fn paneless_repeated_cli_wrappers_reuse_host_address_without_mutation() {
        use pij_core::events::EventFilter;
        use tokio_stream::StreamExt;

        for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
            let mut claim = paneless_claim(harness);
            claim.role = Some("worker".into());
            let (first, _) = pull_register(&service, claim.clone(), SESSION, true)
                .await
                .unwrap();
            assert_eq!(first.role.as_deref(), Some("worker"));
            let mut subscription = service
                .event_bus
                .subscribe(None, EventFilter::default())
                .await
                .unwrap();
            let writes = registry
                .calls()
                .iter()
                .filter(|call| call.starts_with("put:"))
                .count();
            claim.pid = Some(PANELLESS_NEXT_CLI.pid);
            claim.proc_start = Some(PANELLESS_NEXT_CLI.proc_start);
            let (same, binding) = pull_register(&service, claim.clone(), SESSION, true)
                .await
                .unwrap();
            assert_eq!(same, first);
            assert!(!binding.inserted);
            assert_eq!(binding.previous_proc, Some(HOST));
            assert_eq!(
                registry
                    .calls()
                    .iter()
                    .filter(|call| call.starts_with("put:"))
                    .count(),
                writes
            );
            tokio::select! {
                biased;
                event = subscription.next() => panic!("unchanged repeat emitted {event:?}"),
                _ = ready(()) => {}
            }
            service
                .roles
                .assert_role(&same.id, &same.id, Some("pm".into()))
                .await
                .unwrap();
            claim.role = None;
            let (authoritative, _) = pull_register(&service, claim, SESSION, true).await.unwrap();
            assert_eq!(authoritative.role.as_deref(), Some("pm"));
            assert_eq!(
                registry
                    .calls()
                    .iter()
                    .filter(|call| call.starts_with("put:"))
                    .count(),
                writes
            );
        }
    }

    #[tokio::test]
    async fn paneless_refuses_ambiguous_session_stale_tuple_and_false_push_attestation() {
        for harness in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            for case in 0..9 {
                let mut incumbent =
                    SeatDescriptor::new("pij-incumbent", harness, "/isolated/native-registration");
                incumbent.proc = Some(HOST);
                incumbent.harness_session = Some(SESSION.into());
                match case {
                    0 => incumbent.proc = Some(OTHER_HOST),
                    1 => {
                        incumbent.proc = Some(ProcIdentity {
                            proc_start: HOST.proc_start + 1,
                            ..HOST
                        })
                    }
                    2 => incumbent.harness_session = Some("other-session".into()),
                    3 => incumbent.harness = Harness::Omp,
                    4 => incumbent.pane = Some("%137".into()),
                    5 => incumbent.relay = true,
                    6 => incumbent.native_extension_delivery = true,
                    7 => incumbent.spawn_id = Some("spawn".into()),
                    8 => {}
                    _ => unreachable!(),
                }
                let mut seats = vec![incumbent.clone()];
                if case == 8 {
                    let mut duplicate = incumbent.clone();
                    duplicate.id = "pij-duplicate".into();
                    seats.push(duplicate);
                }
                let (service, registry) = native_service(seats, paneless_liveness()).await;
                refused(pull_register(&service, paneless_claim(harness), SESSION, false).await);
                assert_eq!(registry.get(&incumbent.id).await.unwrap(), Some(incumbent));
                assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
            }
        }
    }

    #[tokio::test]
    async fn paneless_allocator_reserves_existing_names_and_serializes_repeat_claims() {
        let seed = format!("codex\0{SESSION}");
        let mut candidates = pij_core::names::memorable_pij_id_candidates(&seed);
        let occupied = SeatDescriptor::new(candidates.next().unwrap(), Harness::Omp, "/unrelated");
        let expected = candidates.next().unwrap();
        let (service, registry) = native_service(vec![occupied.clone()], paneless_liveness()).await;
        let (left, right) = tokio::join!(
            pull_register(&service, paneless_claim(Harness::Codex), SESSION, false),
            pull_register(&service, paneless_claim(Harness::Codex), SESSION, false),
        );
        let (left, first) = left.unwrap();
        let (right, second) = right.unwrap();
        assert_eq!(left.id, expected);
        assert_eq!(right.id, expected);
        assert_ne!(first.inserted, second.inserted);
        assert_eq!(
            registry
                .calls()
                .iter()
                .filter(|call| call.starts_with("put:"))
                .count(),
            1
        );
        assert_eq!(registry.get(&occupied.id).await.unwrap(), Some(occupied));
    }

    struct PanelessCallerStart(AtomicU64);

    #[async_trait::async_trait]
    impl LivenessPort for PanelessCallerStart {
        async fn proc_start(&self, pid: u32) -> pij_core::error::Result<Option<u64>> {
            Ok(if pid == PANELLESS_CLI.pid {
                Some(self.0.load(Ordering::SeqCst))
            } else if pid == PANELLESS_SHELL.pid {
                Some(PANELLESS_SHELL.proc_start)
            } else if pid == HOST.pid {
                Some(HOST.proc_start)
            } else {
                None
            })
        }
    }

    #[tokio::test]
    async fn paneless_sandwich_rechecks_cli_even_when_host_incarnation_is_unchanged() {
        let liveness = Arc::new(PanelessCallerStart(AtomicU64::new(
            PANELLESS_CLI.proc_start,
        )));
        let (service, registry) = native_service(Vec::new(), liveness.clone()).await;
        refused(
            service
                .register_paneless_with_observer(
                    paneless_claim(Harness::Claude),
                    Some(SESSION.into()),
                    |pid| {
                        if pid == HOST.pid {
                            liveness
                                .0
                                .store(PANELLESS_CLI.proc_start + 1, Ordering::SeqCst);
                        }
                        ready(Ok(paneless_process(pid, Harness::Claude, false)))
                    },
                )
                .await,
        );
        assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
    }

    #[tokio::test]
    async fn paneless_ancestry_refuses_cycles_newer_parents_and_more_than_32_processes() {
        for newer_parent in [false, true] {
            let liveness: Arc<dyn LivenessPort> = if newer_parent {
                Arc::new(
                    FakeLiveness::new()
                        .with_proc(PANELLESS_CLI)
                        .with_proc(ProcIdentity {
                            proc_start: PANELLESS_CLI.proc_start + 1,
                            ..PANELLESS_SHELL
                        }),
                )
            } else {
                paneless_liveness()
            };
            let (service, registry) = native_service(Vec::new(), liveness).await;
            refused(
                service
                    .register_paneless_with_observer(
                        paneless_claim(Harness::Copilot),
                        Some(SESSION.into()),
                        |pid| {
                            let mut process =
                                paneless_process(pid, Harness::Copilot, false).unwrap();
                            if !newer_parent {
                                process.parent = pid;
                            }
                            ready(Ok(Some(process)))
                        },
                    )
                    .await,
            );
            assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
        }
        let mut liveness = FakeLiveness::new();
        for pid in 100..=132 {
            liveness = liveness.with_proc(ProcIdentity {
                pid,
                proc_start: 200,
            });
        }
        let (service, registry) = native_service(Vec::new(), Arc::new(liveness)).await;
        let mut claim = paneless_claim(Harness::Codex);
        claim.pid = Some(100);
        let calls = AtomicU64::new(0);
        refused(
            service
                .register_paneless_with_observer(claim, Some(SESSION.into()), |pid| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    ready(Ok(Some(NativeProcess {
                        parent: pid + 1,
                        executable: "/bin/sh".into(),
                        executable_replaced: false,
                        argv: vec!["sh".into()],
                    })))
                })
                .await,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 32);
        assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
    }

    #[tokio::test]
    async fn paneless_host_observation_has_a_wall_clock_bound() {
        let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
        tokio::time::pause();
        let started = tokio::time::Instant::now();
        let claim = paneless_claim(Harness::Copilot);
        let caller_pid = claim.pid.unwrap();
        let error =
            service
                .register_paneless_with_observer(claim, Some(SESSION.into()), |_| {
                    std::future::pending::<
                        std::result::Result<Option<NativeProcess>, RegistrationError>,
                    >()
                })
                .await
                .expect_err("bounded paneless host observation");
        // Tokio resolves virtual deadlines at millisecond timer precision.
        assert!(
            (Duration::from_secs(5)..=Duration::from_millis(5001)).contains(&started.elapsed())
        );
        assert!(error.to_string().contains(&format!("pid {caller_pid}")));
        assert!(matches!(error, RegistrationError::Retryable(_)));
        assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
    }

    #[tokio::test]
    async fn paneless_nearest_external_host_refuses_inherited_other_harness_session_without_writes()
    {
        for requested in [Harness::Claude, Harness::Copilot, Harness::Codex] {
            for nearest in [Harness::Claude, Harness::Copilot, Harness::Codex] {
                if nearest == requested {
                    continue;
                }
                for node in [false, true] {
                    let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
                    let outer_observations = AtomicU64::new(0);
                    let error = service
                        .register_paneless_with_observer(
                            paneless_claim(requested),
                            Some(SESSION.into()),
                            |pid| {
                                let process = if pid == PANELLESS_SHELL.pid {
                                    let mut host =
                                        paneless_process(HOST.pid, nearest, node).unwrap();
                                    host.parent = HOST.pid;
                                    Some(host)
                                } else {
                                    if pid == HOST.pid {
                                        outer_observations.fetch_add(1, Ordering::SeqCst);
                                    }
                                    paneless_process(pid, requested, node)
                                };
                                ready(Ok(process))
                            },
                        )
                        .await
                        .expect_err("nearest external harness is the identity boundary");
                    assert!(
                        error
                            .to_string()
                            .contains("nearest external harness differs")
                    );
                    assert_eq!(outer_observations.load(Ordering::SeqCst), 0);
                    assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
                }
            }
        }
    }

    #[tokio::test]
    async fn paneless_dispatch_preserves_ordinary_empty_id_admission_for_other_harnesses() {
        let (service, registry) = native_service(Vec::new(), paneless_liveness()).await;
        for harness in ["pi", "omp", "unknown-harness"] {
            let mut claim = paneless_claim(Harness::Copilot);
            claim.harness = harness.into();
            claim.pid = None;
            claim.proc_start = None;
            let ordinary = service
                .register_inner(claim.clone(), None, super::RegistrationMode::Ordinary)
                .await
                .expect_err("ordinary empty id refused");
            let dispatched = service
                .register_with_harness_session(claim, None)
                .await
                .expect_err("unrelated harness still uses ordinary admission");
            assert_eq!(dispatched.to_string(), ordinary.to_string());
            assert!(!dispatched.to_string().contains("paneless external"));
        }
        assert!(!registry.calls().iter().any(|call| call.starts_with("put:")));
    }
}
