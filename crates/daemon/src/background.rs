//! Daemon-owned jobs with identity-braked signalling and runner-authored receipts.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pij_core::BG_ACTOR;
use pij_core::background::{BackgroundJob, BackgroundKind, BackgroundState, EventStats};
use pij_core::cold_wake::ColdCheck;
use pij_core::error::{PijError, Result};
use pij_core::model::{Event, Msg, ProcIdentity, SeatDescriptor, SeatId};
use pij_core::ports::{LivenessPort, Registry};
use pij_store::background::{EmitOutcome, EventBatch, SqliteBackground};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::delivery::DeliveryService;
use crate::events::EventBus;

const TAIL_BYTES: u64 = 64 * 1024;
const TAIL_CHARS: usize = 1200;
const MAX_LINES: usize = 1000;
/// Bounds for `--timeout`: long enough for any build, short enough to be a limit.
pub const MIN_TIMEOUT_MS: u64 = 1_000;
/// See [`MIN_TIMEOUT_MS`].
pub const MAX_TIMEOUT_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// An emit's text is one line of description, not a payload.
pub const EMIT_TEXT_MAX: usize = 16 * 1024;
/// An emit's optional JSON payload.
pub const EMIT_DATA_MAX: usize = 256 * 1024;
/// Pending events per source; emits beyond this are dropped and counted.
pub const PENDING_CAP: u64 = 10_000;
/// Default `--min-interval` between two wakes from one source.
pub const DEFAULT_MIN_INTERVAL_MS: u64 = 60_000;
/// Default `--inline-max`: events listed in the turn itself.
pub const DEFAULT_INLINE_MAX: u64 = 5;
/// Upper bounds for the two batching knobs.
pub const MAX_MIN_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;
/// See [`MAX_MIN_INTERVAL_MS`].
pub const MAX_INLINE_MAX: u64 = 100;
/// An inline turn longer than this, or any inline datum longer than
/// `INLINE_DATA_CHARS`, goes to a batch file instead.
const INLINE_BODY_CHARS: usize = 4_000;
const INLINE_DATA_CHARS: usize = 400;

/// Caller-chosen launch options beyond the title and command.
#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    /// Working directory for the command; the owner's recorded folder when absent.
    pub cwd: Option<PathBuf>,
    /// Kill the job with a TIMEOUT turn once it has run this long.
    pub timeout_ms: Option<u64>,
    /// Make the job an event source with these batching rules.
    pub events: Option<EventsOptions>,
}

/// How an event source's batches reach its owner.
#[derive(Clone, Copy, Debug)]
pub struct EventsOptions {
    /// Hold each batch as an FYI for the owner's next turn instead of waking it.
    pub fyi: bool,
    /// Minimum gap between two wakes caused by this source.
    pub min_interval_ms: u64,
    /// The most events listed in the turn itself.
    pub inline_max: u64,
}

impl Default for EventsOptions {
    fn default() -> Self {
        Self {
            fyi: false,
            min_interval_ms: DEFAULT_MIN_INTERVAL_MS,
            inline_max: DEFAULT_INLINE_MAX,
        }
    }
}

/// The daemon facts that decide where a batch for a cold owner goes.
///
/// Injected so that every routing branch is exercised without a real
/// transcript, role store or Telegram bot.
#[async_trait]
pub trait ColdRouting: Send + Sync {
    /// The cold-wake guard's own `check()` for this seat.
    async fn check(&self, seat: &SeatDescriptor) -> ColdCheck;
    /// The seat's prime: its nearest ancestor holding the prime role, else the
    /// designated prime. Never the seat itself.
    async fn prime(&self, seat: &SeatDescriptor) -> Result<Option<SeatId>>;
    /// Telegram the user, as `pij send pij-telegram` would.
    async fn telegram(&self, from: &SeatId, body: String, msg_id: String) -> Result<()>;
}

/// Where a cold owner's notice goes.
enum PrimeRoute {
    Warm(SeatId),
    Cold(SeatId, u64),
    Gone(SeatId),
    None,
}

// No user text is interpolated. The pipe is a persist-before-execute barrier:
// losing the daemon before its release closes stdin and cannot run the command.
// Only this runner supplies an exit code AND the original finish instant.
const RUNNER: &str = r#"
umask 077
__pij_bg_finish() {
    trap '' TERM
    __pij_bg_code=$1
    __pij_bg_finished=$(/bin/date +%s) || return
    printf '%s %s\n' "$__pij_bg_code" "$__pij_bg_finished" > "$PIJ_BG_EXIT.tmp" &&
        /bin/mv -f "$PIJ_BG_EXIT.tmp" "$PIJ_BG_EXIT"
}
trap '' HUP
trap '__pij_bg_finish 143; exit 143' TERM
: > "$PIJ_BG_EXIT.ready" || exit 125
IFS= read -r __pij_bg_gate || exit 125
[ "$__pij_bg_gate" = start ] || exit 125
exec </dev/null
/bin/sh -c "$PIJ_BG_COMMAND" &
__pij_bg_child=$!
wait "$__pij_bg_child"
__pij_bg_status=$?
__pij_bg_finish "$__pij_bg_status"
exit "$__pij_bg_status"
"#;

/// Serializes launch/recovery/kill while keeping child handles out of shutdown waits.
pub struct BackgroundService {
    store: SqliteBackground,
    registry: Arc<dyn Registry>,
    liveness: Arc<dyn LivenessPort>,
    delivery: Arc<DeliveryService>,
    event_bus: Arc<EventBus>,
    out_dir: PathBuf,
    daemon_addr: String,
    routing: Arc<dyn ColdRouting>,
    children: Mutex<HashMap<String, Child>>,
    /// Serializes batch cutting with the final flush so no event leaves twice.
    events: Mutex<()>,
}

/// The shared daemon ports a [`BackgroundService`] reads and delivers through.
pub struct BackgroundPorts {
    /// Seat descriptors: owners, parents and primes.
    pub registry: Arc<dyn Registry>,
    /// Process identity for re-adoption and the signal brakes.
    pub liveness: Arc<dyn LivenessPort>,
    /// The one delivery pipeline for turns and FYIs.
    pub delivery: Arc<DeliveryService>,
    /// Lifecycle events (`bg.finished` and friends).
    pub event_bus: Arc<EventBus>,
    /// Where a cold owner's batch goes instead of waking it.
    pub routing: Arc<dyn ColdRouting>,
}

impl BackgroundService {
    /// Compose the durable store, shared ports and private output directory.
    pub fn new(
        store: SqliteBackground,
        ports: BackgroundPorts,
        out_dir: PathBuf,
        daemon_addr: String,
    ) -> Self {
        let BackgroundPorts {
            registry,
            liveness,
            delivery,
            event_bus,
            routing,
        } = ports;
        Self {
            store,
            registry,
            liveness,
            delivery,
            event_bus,
            out_dir,
            daemon_addr,
            routing,
            children: Mutex::new(HashMap::new()),
            events: Mutex::new(()),
        }
    }

    /// Persist a queued job, bind its gated runner identity, then release its command.
    ///
    /// # Errors
    /// Invalid title/command, filesystem/spawn failures, unavailable process identity,
    /// or persistence failures. A failed launch never releases an unrecorded command.
    pub async fn create(
        &self,
        owner: &SeatDescriptor,
        title: &str,
        command: &str,
        options: CreateOptions,
    ) -> Result<BackgroundJob> {
        let title = title.trim();
        let command = command.trim();
        if title.is_empty() {
            return Err(refusal("--title must not be empty"));
        }
        if title.encode_utf16().count() > 120 {
            return Err(refusal("--title must be at most 120 characters"));
        }
        if title.contains(['\n', '\r']) {
            return Err(refusal("--title must be a single line"));
        }
        if command.is_empty() {
            return Err(refusal("--command must not be empty"));
        }
        if title.contains('\0') || command.contains('\0') {
            return Err(refusal("title and command must not contain NUL"));
        }
        if let Some(timeout) = options.timeout_ms
            && !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&timeout)
        {
            return Err(refusal("--timeout must be between 1s and 30d"));
        }
        if let Some(events) = options.events {
            if events.min_interval_ms > MAX_MIN_INTERVAL_MS {
                return Err(refusal("--min-interval must be at most 1d"));
            }
            if events.inline_max > MAX_INLINE_MAX {
                return Err(refusal("--inline-max must be at most 100"));
            }
        }
        let cwd = options.cwd.unwrap_or_else(|| PathBuf::from(&owner.folder));
        if !cwd.is_absolute() {
            return Err(refusal(format!(
                "working directory {} is not absolute",
                cwd.display()
            )));
        }
        if !cwd.is_dir() {
            return Err(refusal(format!(
                "working directory {} does not exist or is not a directory",
                cwd.display()
            )));
        }
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| fault(format!("job id: {error}")))?;
        let started_at = now_ms()?;
        let job_id = format!("bg-{started_at}-{:032x}", u128::from_be_bytes(random));
        let out_path = self.out_dir.join(format!("{job_id}.log"));
        let mut job = BackgroundJob {
            job_id: job_id.clone(),
            owner: owner.id.clone(),
            title: title.to_string(),
            command: command.to_string(),
            pid: None,
            proc_start: None,
            pgid: None,
            out_path: out_path.to_string_lossy().into_owned(),
            state: BackgroundState::Queued,
            exit_code: None,
            started_at,
            finished_at: None,
            kill_requested: false,
            notified: false,
            deadline_at: options
                .timeout_ms
                .map(|timeout| started_at.saturating_add(timeout)),
            timed_out: false,
            term_sent: false,
            kind: if options.events.is_some() {
                BackgroundKind::Events
            } else {
                BackgroundKind::Oneshot
            },
            events_fyi: options.events.is_some_and(|events| events.fyi),
            min_interval_ms: options
                .events
                .map_or(DEFAULT_MIN_INTERVAL_MS, |events| events.min_interval_ms),
            inline_max: options
                .events
                .map_or(DEFAULT_INLINE_MAX, |events| events.inline_max),
            last_wake_at: None,
            batches: 0,
        };
        // The hook's secret exists only in the child's environment; the store
        // keeps its digest, so a store read never yields a usable token.
        let token = if options.events.is_some() {
            let mut secret = [0_u8; 32];
            getrandom::fill(&mut secret).map_err(|error| fault(format!("job token: {error}")))?;
            Some(hex(&secret))
        } else {
            None
        };
        let mut children = self.children.lock().await;
        let dir = self.out_dir.clone();
        let log = blocking(move || {
            std::fs::create_dir_all(dir).map_err(io_error)?;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(out_path)
                .map_err(io_error)
        })
        .await?;
        self.store
            .insert_with_token(&job, token.as_deref().map(token_digest).as_deref())
            .await?;
        let stderr = log.try_clone().map_err(io_error)?;
        let mut runner = Command::new("/bin/sh");
        runner
            .args(["-c", RUNNER])
            .current_dir(&cwd)
            .env("PIJ_SESSION_ID", owner.id.as_str())
            .env("PIJ_RS_ADDR", &self.daemon_addr)
            .env(
                "PIJ_RS_STATE_DIR",
                self.out_dir.parent().unwrap_or(&self.out_dir),
            )
            .env("PIJ_DAEMON_GENERATION", "rs")
            .env_remove("TMUX_PANE")
            .env("PIJ_BG_JOB", &job_id)
            .env("PIJ_BG_TITLE", title)
            .env("PIJ_BG_COMMAND", command)
            .env("PIJ_BG_EXIT", receipt_path(&job))
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .process_group(0);
        if let Some(pane) = &owner.pane {
            runner.env("TMUX_PANE", pane);
        }
        if let Some(token) = &token {
            runner.env("PIJ_BG_TOKEN", token);
        }
        let mut child = runner.spawn().map_err(io_error)?;
        let pid = child.id();
        let mut gate = child
            .stdin
            .take()
            .ok_or_else(|| fault("runner stdin gate missing"))?;
        children.insert(job_id.clone(), child);
        let runner = children
            .get_mut(&job_id)
            .ok_or_else(|| fault("runner handle missing before readiness"))?;
        await_runner_ready(runner, &receipt_path(&job).with_extension("exit.ready")).await?;
        let proc_start = self
            .liveness
            .proc_start(pid)
            .await?
            .ok_or_else(|| fault("runner identity unavailable; command was not released"))?;
        let identity = ProcIdentity { pid, proc_start };
        if !self.store.start(&job_id, &identity, pid).await? {
            return Err(fault(
                "job changed before runner release; command was not released",
            ));
        }
        gate.write_all(b"start\n").map_err(io_error)?;
        drop(gate);
        job.pid = Some(pid);
        job.proc_start = Some(proc_start);
        job.pgid = Some(pid);
        job.state = BackgroundState::Running;
        Ok(job)
    }

    /// List the caller's jobs in every lifecycle state, optionally adding direct children.
    ///
    /// # Errors
    /// Registry or job-store read failures.
    pub async fn list(&self, owner: &SeatId, all: bool) -> Result<Vec<BackgroundJob>> {
        let mut jobs = Vec::new();
        for job in self.store.list().await? {
            if job.owner == *owner || (all && self.is_parent(owner, &job.owner).await?) {
                jobs.push(job);
            }
        }
        Ok(jobs)
    }

    /// Read a job only for its owner or the owner's directly recorded parent.
    ///
    /// # Errors
    /// Missing jobs, authorization refusals, or store/registry failures.
    pub async fn get(&self, caller: &SeatId, job: &str) -> Result<BackgroundJob> {
        let job = self.lookup(job).await?;
        if job.owner != *caller && !self.is_parent(caller, &job.owner).await? {
            return Err(refusal(
                "only the job owner or its recorded parent may access this job",
            ));
        }
        Ok(job)
    }

    /// Read at most 64 KiB from the log's end and return the last requested lines.
    ///
    /// # Errors
    /// Authorization/read failures or a line count outside 1..=1000.
    pub async fn tail(
        &self,
        caller: &SeatId,
        job: &str,
        lines: usize,
    ) -> Result<(BackgroundJob, Vec<String>)> {
        if !(1..=MAX_LINES).contains(&lines) {
            return Err(refusal("--lines must be between 1 and 1000"));
        }
        let job = self.get(caller, job).await?;
        let path = PathBuf::from(&job.out_path);
        let tail = blocking(move || read_tail(&path, lines)).await?;
        Ok((job, tail))
    }

    /// Accept one event from a source's hook, authenticated by its job token.
    ///
    /// # Errors
    /// A `background/auth` refusal for an unknown job, a wrong token or a revoked
    /// one (indistinguishable on purpose); a refusal for oversize input or a job
    /// that is killed or finishing; store failures.
    pub async fn emit(
        &self,
        job_id: &str,
        token: &str,
        text: &str,
        data: Option<&serde_json::Value>,
    ) -> Result<EmitOutcome> {
        let stored = self.store.token_hash(job_id).await?;
        let presented = token_digest(token);
        if !stored.is_some_and(|stored| constant_time_eq(stored.as_bytes(), presented.as_bytes())) {
            return Err(PijError::Adapter {
                adapter: "background/auth".to_string(),
                message: "invalid job token: a token works only for its own live job".to_string(),
            });
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(refusal("event text must not be empty"));
        }
        if text.len() > EMIT_TEXT_MAX {
            return Err(refusal("event text must be at most 16 KiB"));
        }
        let data = data
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| refusal(format!("event data: {error}")))?;
        if data.as_ref().is_some_and(|data| data.len() > EMIT_DATA_MAX) {
            return Err(refusal("event data must be at most 256 KiB"));
        }
        match self
            .store
            .emit(job_id, now_ms()?, text, data.as_deref(), PENDING_CAP)
            .await?
        {
            EmitOutcome::Refused => Err(refusal(
                "this source is stopping or stopped; it no longer accepts events",
            )),
            outcome => Ok(outcome),
        }
    }

    /// Fired and pending counts for one source.
    ///
    /// # Errors
    /// Store failures.
    pub async fn event_stats(&self, job_id: &str) -> Result<EventStats> {
        self.store.event_stats(job_id).await
    }

    /// Persist kill intent, re-check the full process identity and group, then SIGTERM.
    ///
    /// # Errors
    /// Authorization/persistence failures, unsafe or recycled process identities,
    /// or a failed signal command. A PID alone never authorizes a signal.
    pub async fn kill(&self, caller: &SeatId, job: &str) -> Result<BackgroundJob> {
        let mut children = self.children.lock().await;
        let job = self.get(caller, job).await?;
        self.reconcile(&job, &mut children).await?;
        let current = self.lookup(&job.job_id).await?;
        if terminal(current.state) {
            self.notify(&current).await?;
            return Err(refusal(format!(
                "bg job '{}' is not running ({:?})",
                current.job_id, current.state
            )));
        } else if !job.kill_requested {
            self.store.request_kill(&job.job_id).await?;
            self.signal(&current).await?;
        }
        self.lookup(&job.job_id).await
    }

    async fn signal(&self, job: &BackgroundJob) -> Result<()> {
        let identity =
            identity(job).ok_or_else(|| refusal("job has no recorded runner identity"))?;
        let pgid = job
            .pgid
            .ok_or_else(|| refusal("job has no recorded process group"))?;
        if pgid != identity.pid || pgid <= 1 || pgid == std::process::id() {
            return Err(refusal(
                "refusing to signal a group that is not this job's own runner",
            ));
        }
        // Provenance is persisted before the signal (never after, which a
        // crash could lose), and withdrawn if this first attempt sends nothing.
        // Accepted residual: a daemon crash between this commit and the TERM
        // leaves term_sent set without a signal, so an overrun job that then
        // fails on its own reads TIMEOUT instead of FAILED (exit N). Only the
        // label differs, and the deadline it names really did pass.
        let first = !job.term_sent;
        if first {
            self.store.set_term_sent(&job.job_id, true).await?;
        }
        let sent = self.send_term(identity, pgid).await;
        if first && !matches!(sent, Ok(true)) {
            self.store.set_term_sent(&job.job_id, false).await?;
        }
        match sent {
            Ok(true) => Ok(()),
            Ok(false) => Err(refusal(
                "runner process group or identity changed or disappeared; no signal sent",
            )),
            Err(error) => Err(error),
        }
    }

    /// `Ok(false)` when the identity brakes refuse; `Ok(true)` once TERM is sent.
    async fn send_term(&self, identity: ProcIdentity, pgid: u32) -> Result<bool> {
        // Group membership is checked before the final identity observation.
        // Nothing awaits between that observation and spawning the fixed signal.
        let observed_group = blocking(move || process_group(identity.pid)).await?;
        if observed_group != Some(pgid) {
            return Ok(false);
        }
        if self.liveness.proc_start(identity.pid).await? != Some(identity.proc_start) {
            return Ok(false);
        }
        let signal = Command::new("/bin/kill")
            .args(["-TERM", "--", &format!("-{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(io_error)?;
        let result = blocking(move || signal.wait_with_output().map_err(io_error)).await?;
        if !result.status.success() {
            return Err(fault(format!(
                "could not signal runner group: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            )));
        }
        Ok(true)
    }

    /// Reap local children, recover orphaned rows by identity/receipt, and notify endings.
    ///
    /// # Errors
    /// Store, liveness, receipt, event, or delivery failures. Other jobs are still
    /// visited when one fails, and unnotified terminal rows are retried next tick.
    pub async fn tick(&self) -> Result<()> {
        let mut first_error = None;
        let mut sources = Vec::new();
        {
            let mut children = self.children.lock().await;
            let now = now_ms()?;
            for job in self.store.pending().await? {
                // Completion first: a runner that already exited on its own (a
                // late tick, a daemon restart) keeps its own result. The deadline
                // applies only to a runner that is still alive now.
                if let Err(error) = self.reconcile(&job, &mut children).await {
                    first_error.get_or_insert(error);
                    continue;
                }
                let mut current = match self.lookup(&job.job_id).await {
                    Ok(current) => current,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        continue;
                    }
                };
                if current.state == BackgroundState::Running
                    && !current.kill_requested
                    && current.deadline_at.is_some_and(|deadline| deadline <= now)
                {
                    match self.store.request_timeout(&current.job_id, now).await {
                        Ok(true) => {
                            current.kill_requested = true;
                            current.timed_out = true;
                            // Signals the still-live runner through the identity brakes.
                            if let Err(error) = self.reconcile(&current, &mut children).await {
                                first_error.get_or_insert(error);
                                continue;
                            }
                            match self.lookup(&job.job_id).await {
                                Ok(latest) => current = latest,
                                Err(error) => {
                                    first_error.get_or_insert(error);
                                    continue;
                                }
                            }
                        }
                        Ok(false) => {}
                        Err(error) => {
                            first_error.get_or_insert(error);
                            continue;
                        }
                    }
                }
                if terminal(current.state) {
                    if let Err(error) = self.notify(&current).await {
                        first_error.get_or_insert(error);
                    }
                } else if current.kind == BackgroundKind::Events
                    && current.state == BackgroundState::Running
                {
                    sources.push(current);
                }
            }
        }
        // Batching reads transcripts for the cold check; never hold the child
        // lock (which create and kill need) across that.
        for job in sources {
            if let Err(error) = self.pump(&job).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Cut and deliver one batch for a live source when its owner may take it.
    async fn pump(&self, job: &BackgroundJob) -> Result<()> {
        let _batching = self.events.lock().await;
        let job = self.lookup(&job.job_id).await?;
        if job.state != BackgroundState::Running {
            return Ok(());
        }
        let now = now_ms()?;
        if job
            .last_wake_at
            .is_some_and(|at| now < at.saturating_add(job.min_interval_ms))
        {
            return Ok(());
        }
        let Some(owner) = self.registry.get(&job.owner).await? else {
            return Ok(());
        };
        if owner.tombstoned_at.is_some() {
            return Ok(());
        }
        // A busy owner keeps accumulating; the batch lands when its turn ends.
        if !job.events_fyi && owner.state == pij_core::model::SystemState::Working {
            return Ok(());
        }
        if self.store.event_stats(&job.job_id).await?.pending == 0 {
            return Ok(());
        }
        let Some(batch) = self.store.cut_batch(&job.job_id, false).await? else {
            return Ok(());
        };
        self.deliver_batch(&job, &owner, &batch, now).await
    }

    async fn deliver_batch(
        &self,
        job: &BackgroundJob,
        owner: &SeatDescriptor,
        batch: &EventBatch,
        now: u64,
    ) -> Result<()> {
        let msg_id = format!("pij-bg:{}:batch:{}", job.job_id, batch.batch_no);
        if job.events_fyi {
            let body = self.batch_turn(job, batch, false).await?;
            self.delivery
                .hold_fyi(bg_msg(&owner.id, body, msg_id))
                .await?;
            return self
                .store
                .settle_batch(&job.job_id, batch.batch_no, "held", now)
                .await;
        }
        let ColdCheck::Cold {
            context_tokens,
            idle_ms,
            ..
        } = self.routing.check(owner).await
        else {
            let body = self.batch_turn(job, batch, false).await?;
            self.delivery
                .accept(bg_msg(&owner.id, body, msg_id))
                .await?;
            return self
                .store
                .settle_batch(&job.job_id, batch.batch_no, "delivered", now)
                .await;
        };
        // Cold: never wake it. The batch waits as an FYI (nothing is lost) and
        // the prime, or failing that the human, decides whether a wake is worth it.
        let body = self.batch_turn(job, batch, true).await?;
        let file = self.batch_path(job, batch.batch_no);
        self.delivery
            .hold_fyi(bg_msg(&owner.id, body, msg_id.clone()))
            .await?;
        self.store
            .settle_batch(&job.job_id, batch.batch_no, "routed", now)
            .await?;
        let count = batch.events.len();
        let lead = format!(
            "❄ {} is cold ({}k, idle {}): {count} {} from {} {} held.",
            owner.id,
            context_tokens / 1000,
            human_duration(idle_ms),
            plural(count, "event", "events"),
            job.title,
            if count == 1 { "is" } else { "are" },
        );
        let why = match self.prime_route(owner).await? {
            PrimeRoute::Warm(prime) => {
                let notice = format!(
                    "{lead}\nThey wait as an FYI on {owner}, which sees them on its next turn. \
                     To wake it now: pij send {owner} --force --reason \"<why>\"\nEvents: {file}",
                    owner = owner.id,
                    file = file.display(),
                );
                match self
                    .delivery
                    .accept(bg_msg(&prime, notice, format!("{msg_id}:prime")))
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(error) => format!(
                        "Its prime {prime} could not take the notice ({error}), so this came to you."
                    ),
                }
            }
            PrimeRoute::Cold(prime, idle) => format!(
                "Its prime {prime} is cold (idle {}), so this came to you.",
                human_duration(idle)
            ),
            PrimeRoute::Gone(prime) => format!("Its prime {prime} is gone, so this came to you."),
            PrimeRoute::None => "It has no prime, so this came to you.".to_string(),
        };
        self.routing
            .telegram(
                &owner.id,
                format!("{lead}\n{why}\nEvents: {}", file.display()),
                format!("{msg_id}:telegram"),
            )
            .await
    }

    async fn prime_route(&self, owner: &SeatDescriptor) -> Result<PrimeRoute> {
        let Some(prime) = self.routing.prime(owner).await? else {
            return Ok(PrimeRoute::None);
        };
        if prime == owner.id {
            return Ok(PrimeRoute::None);
        }
        let Some(seat) = self.registry.get(&prime).await? else {
            return Ok(PrimeRoute::Gone(prime));
        };
        if seat.tombstoned_at.is_some() {
            return Ok(PrimeRoute::Gone(prime));
        }
        Ok(match self.routing.check(&seat).await {
            ColdCheck::Cold { idle_ms, .. } => PrimeRoute::Cold(prime, idle_ms),
            _ => PrimeRoute::Warm(prime),
        })
    }

    fn batch_path(&self, job: &BackgroundJob, batch_no: u64) -> PathBuf {
        self.out_dir
            .join(&job.job_id)
            .join(format!("batch-{batch_no:04}.json"))
    }

    /// The batch as a turn: listed inline when small, else written to a file the
    /// turn names. `force_file` always writes the file (cold routing cites it).
    async fn batch_turn(
        &self,
        job: &BackgroundJob,
        batch: &EventBatch,
        force_file: bool,
    ) -> Result<String> {
        let count = batch.events.len();
        let dropped = if batch.dropped > 0 {
            format!(
                " ({} more dropped over the {PENDING_CAP}-event pending cap)",
                batch.dropped
            )
        } else {
            String::new()
        };
        let inline = inline_events(batch);
        let needs_file = force_file
            || u64::try_from(count).unwrap_or(u64::MAX) > job.inline_max
            || inline.is_none();
        if needs_file {
            let path = self.batch_path(job, batch.batch_no);
            write_batch_file(&path, job, batch).await?;
            return Ok(format!(
                "[pij bg] {count} new {} from {}{dropped}, here is the file: {}",
                plural(count, "event", "events"),
                job.title,
                path.display()
            ));
        }
        Ok(format!(
            "[pij bg] {count} new {} from {} (job {}){dropped}:\n{}",
            plural(count, "event", "events"),
            job.title,
            job.job_id,
            inline.unwrap_or_default()
        ))
    }

    async fn lookup(&self, job: &str) -> Result<BackgroundJob> {
        self.store
            .get(job)
            .await?
            .ok_or_else(|| refusal(format!("unknown background job {job}")))
    }

    async fn is_parent(&self, caller: &SeatId, owner: &SeatId) -> Result<bool> {
        Ok(self
            .registry
            .get(owner)
            .await?
            .is_some_and(|seat| seat.parent.as_ref() == Some(caller)))
    }

    async fn reconcile(
        &self,
        job: &BackgroundJob,
        children: &mut HashMap<String, Child>,
    ) -> Result<()> {
        let reaped = match children.get_mut(&job.job_id) {
            Some(child) => child.try_wait().map_err(io_error)?.is_some(),
            None => false,
        };
        if reaped {
            children.remove(&job.job_id);
        }
        if terminal(job.state) {
            return Ok(());
        }
        if children.contains_key(&job.job_id) {
            // An unreaped owned Child is stronger evidence than a separate ps
            // probe. Only re-adopted rows need polling; signals still recheck.
            if job.kill_requested {
                self.signal(job).await?;
            }
            return Ok(());
        }
        if !reaped
            && let Some(proc) = identity(job)
            && self.liveness.proc_start(proc.pid).await? == Some(proc.proc_start)
        {
            if job.kill_requested {
                self.signal(job).await?;
            }
            return Ok(());
        }
        let path = receipt_path(job);
        let receipt = blocking(move || read_receipt(&path)).await?;
        let now = now_ms()?;
        let (state, code, at) = match receipt {
            Some(receipt) => (
                // A caller's kill always reads KILLED. A timeout reads TIMEOUT
                // only when the daemon recorded sending its TERM and the runner
                // then failed; a runner that exited on its own before any TERM
                // (any code, 143 too) keeps its own result. The exit code alone
                // is never provenance: a command can `exit 143` by itself.
                if job.kill_requested
                    && (!job.timed_out || (job.term_sent && receipt.exit_code != 0))
                {
                    BackgroundState::Killed
                } else {
                    BackgroundState::Done
                },
                Some(receipt.exit_code),
                receipt.finished_at,
            ),
            None => (BackgroundState::Lost, None, now),
        };
        self.store.finish(&job.job_id, state, code, at).await?;
        Ok(())
    }

    async fn notify(&self, job: &BackgroundJob) -> Result<()> {
        if job.notified {
            return Ok(());
        }
        let _batching = if job.kind == BackgroundKind::Events {
            Some(self.events.lock().await)
        } else {
            None
        };
        let path = PathBuf::from(&job.out_path);
        let output = if job.state == BackgroundState::Done || job.timed_out {
            blocking(move || read_tail(&path, 20).map(|lines| lines.join("\n"))).await?
        } else {
            String::new()
        };
        let mut body = completion_turn(job, &output);
        // The source's end flushes whatever it fired since its last batch into
        // this same final turn (its token was revoked when the row finished).
        let last = if job.kind == BackgroundKind::Events {
            let batch = self.store.cut_batch(&job.job_id, true).await?;
            match &batch {
                Some(batch) if !batch.events.is_empty() || batch.dropped > 0 => {
                    body.push('\n');
                    body.push_str(&self.final_events(job, batch).await?);
                }
                _ => body.push_str("\nNo events since the last batch."),
            }
            batch
        } else {
            None
        };
        let msg_id = format!("pij-bg:{}:finished", job.job_id);
        let kind = match job.state {
            BackgroundState::Done => "bg.finished",
            BackgroundState::Killed => "bg.killed",
            BackgroundState::Lost => "bg.lost",
            _ => return Ok(()),
        };
        let payload = serde_json::to_string(&serde_json::json!({
            "job_id": job.job_id,
            "owner": job.owner,
            "title": job.title,
            "state": job.state,
            "exit_code": job.exit_code,
            "started_at": job.started_at,
            "finished_at": job.finished_at,
            "out_path": job.out_path,
            "msg_id": msg_id,
        }))
        .map_err(|error| fault(format!("completion event: {error}")))?;
        self.event_bus
            .publish(Event {
                seq: None,
                v: 1,
                at: job.finished_at.unwrap_or(job.started_at),
                kind: kind.to_string(),
                seat: Some(job.owner.clone()),
                payload,
            })
            .await?;
        self.delivery
            .accept(Msg {
                from: SeatId::from(BG_ACTOR),
                to: job.owner.clone(),
                body,
                msg_id,
                from_machine: None,
                in_reply_to: None,
                command: None,
            })
            .await?;
        if let Some(batch) = last {
            self.store
                .settle_batch(&job.job_id, batch.batch_no, "delivered", now_ms()?)
                .await?;
        }
        self.store.mark_notified(&job.job_id).await?;
        Ok(())
    }

    /// "N events since the last batch", inline or as a file, for the final turn.
    async fn final_events(&self, job: &BackgroundJob, batch: &EventBatch) -> Result<String> {
        let count = batch.events.len();
        let since = format!(
            "{count} {} since the last batch",
            plural(count, "event", "events")
        );
        let dropped = if batch.dropped > 0 {
            format!(
                " ({} more dropped over the {PENDING_CAP}-event pending cap)",
                batch.dropped
            )
        } else {
            String::new()
        };
        match inline_events(batch)
            .filter(|_| u64::try_from(count).unwrap_or(u64::MAX) <= job.inline_max)
        {
            Some(inline) if count > 0 => Ok(format!("{since}{dropped}:\n{inline}")),
            Some(_) => Ok(format!("{since}{dropped}.")),
            None => {
                let path = self.batch_path(job, batch.batch_no);
                write_batch_file(&path, job, batch).await?;
                Ok(format!(
                    "{since}{dropped}, here is the file: {}",
                    path.display()
                ))
            }
        }
    }
}

fn bg_msg(to: &SeatId, body: String, msg_id: String) -> Msg {
    Msg {
        from: SeatId::from(BG_ACTOR),
        to: to.clone(),
        body,
        msg_id,
        from_machine: None,
        in_reply_to: None,
        command: None,
    }
}

impl Drop for BackgroundService {
    fn drop(&mut self) {
        // std Child does not kill on drop. Detached OS threads reap children when
        // an embedded daemon restarts without exiting its process; unlike Tokio's
        // blocking pool they never hold up runtime/daemon shutdown.
        for (_, mut child) in self.children.get_mut().drain() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

fn refusal(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "background/refused".to_string(),
        message: message.into(),
    }
}

fn fault(message: impl Into<String>) -> PijError {
    PijError::Adapter {
        adapter: "background".to_string(),
        message: message.into(),
    }
}

fn io_error(error: std::io::Error) -> PijError {
    fault(error.to_string())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| fault(format!("background IO task: {error}")))?
}

fn now_ms() -> Result<u64> {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| fault(format!("system clock before Unix epoch: {error}")))?
        .as_millis();
    u64::try_from(ms).map_err(|_| fault("system clock outside timestamp range"))
}

fn terminal(state: BackgroundState) -> bool {
    matches!(
        state,
        BackgroundState::Done | BackgroundState::Killed | BackgroundState::Lost
    )
}

fn identity(job: &BackgroundJob) -> Option<ProcIdentity> {
    Some(ProcIdentity {
        pid: job.pid?,
        proc_start: job.proc_start?,
    })
}

fn receipt_path(job: &BackgroundJob) -> PathBuf {
    Path::new(&job.out_path).with_extension("exit")
}

async fn await_runner_ready(child: &mut Child, ready: &Path) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if child.try_wait().map_err(io_error)?.is_some() {
                return Err(fault(
                    "runner exited before installing signal handlers; command was not released",
                ));
            }
            // Successful removal observes the marker and cleans it in one IO.
            match tokio::fs::remove_file(ready).await {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| fault("runner signal-handler readiness timed out; command was not released"))?
}

struct ExitReceipt {
    exit_code: i32,
    finished_at: u64,
}

fn read_receipt(path: &Path) -> Result<Option<ExitReceipt>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let mut bytes = Vec::new();
    file.take(129).read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() > 128 {
        return Ok(None);
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(None);
    };
    let mut fields = text.split_whitespace();
    let Some(code) = fields.next().and_then(|field| field.parse::<i32>().ok()) else {
        return Ok(None);
    };
    let Some(at) = fields.next().and_then(|field| field.parse::<u64>().ok()) else {
        return Ok(None);
    };
    if fields.next().is_some() || !(0..=255).contains(&code) || at == 0 {
        return Ok(None);
    }
    Ok(at.checked_mul(1000).map(|finished_at| ExitReceipt {
        exit_code: code,
        finished_at,
    }))
}

fn read_tail(path: &Path, lines: usize) -> Result<Vec<String>> {
    let mut file = File::open(path).map_err(io_error)?;
    let end = file.metadata().map_err(io_error)?.len();
    file.seek(SeekFrom::Start(end.saturating_sub(TAIL_BYTES)))
        .map_err(io_error)?;
    let mut bytes = Vec::with_capacity(usize::try_from(end.min(TAIL_BYTES)).unwrap_or(0));
    file.take(TAIL_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut tail: Vec<String> = text.lines().rev().take(lines).map(str::to_owned).collect();
    tail.reverse();
    Ok(tail)
}

fn process_group(pid: u32) -> Result<Option<u32>> {
    let output = Command::new("/bin/ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Ok(None);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map(Some)
        .map_err(|error| fault(format!("runner process group: {error}")))
}

/// Compact elapsed time: `42s`, `3m05s`, `2h07m`.
pub fn human_duration(ms: u64) -> String {
    let seconds = ms.saturating_add(500) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The stored form of a job token: its SHA-256, hex.
fn token_digest(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Compare without an early exit, so timing does not reveal a matching prefix.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Numbered lines, or `None` when the batch is too large to read inline.
fn inline_events(batch: &EventBatch) -> Option<String> {
    let mut lines = Vec::with_capacity(batch.events.len());
    for (index, event) in batch.events.iter().enumerate() {
        let text = event.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = match &event.data {
            Some(data) if data.chars().count() > INLINE_DATA_CHARS => return None,
            Some(data) => format!("{}. {text} · data: {data}", index + 1),
            None => format!("{}. {text}", index + 1),
        };
        lines.push(line);
    }
    let body = lines.join("\n");
    (body.chars().count() <= INLINE_BODY_CHARS).then_some(body)
}

async fn write_batch_file(path: &Path, job: &BackgroundJob, batch: &EventBatch) -> Result<()> {
    let events: Vec<serde_json::Value> = batch
        .events
        .iter()
        .map(|event| {
            serde_json::json!({
                "seq": event.seq,
                "ts": event.ts,
                "text": event.text,
                "data": event
                    .data
                    .as_deref()
                    .and_then(|data| serde_json::from_str::<serde_json::Value>(data).ok()),
            })
        })
        .collect();
    let document = serde_json::json!({
        "job": job.job_id,
        "title": job.title,
        "owner": job.owner,
        "batch": batch.batch_no,
        "dropped": batch.dropped,
        "events": events,
    });
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| fault(format!("batch file: {error}")))?;
    let path = path.to_path_buf();
    blocking(move || {
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(io_error)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)
    })
    .await
}

fn completion_turn(job: &BackgroundJob, output: &str) -> String {
    if job.state == BackgroundState::Killed && job.timed_out {
        let limit = job.deadline_at.map_or_else(
            || "unknown".to_string(),
            |at| human_duration(at.saturating_sub(job.started_at)),
        );
        return with_tail(
            format!(
                "[pij bg] TIMEOUT — {} (killed after {limit}) · full log: {}",
                job.title, job.out_path
            ),
            output,
        );
    }
    if job.state == BackgroundState::Killed {
        // A source's normal end is a stop, not an accident.
        let word = if job.kind == BackgroundKind::Events {
            "STOPPED"
        } else {
            "KILLED"
        };
        return format!(
            "[pij bg] {word} — {} · full log: {}",
            job.title, job.out_path
        );
    }
    if job.state == BackgroundState::Lost {
        return format!("[pij bg] LOST — {} · full log: {}", job.title, job.out_path);
    }
    let code = job.exit_code.unwrap_or(-1);
    let verdict = if code == 0 {
        "OK".to_string()
    } else {
        format!("FAILED (exit {code})")
    };
    let duration = job.finished_at.map_or_else(
        || "unknown".to_string(),
        |at| human_duration(at.saturating_sub(job.started_at)),
    );
    with_tail(
        format!(
            "[pij bg] {verdict} — {} ({duration}) · full log: {}",
            job.title, job.out_path
        ),
        output,
    )
}

/// Append the bounded, single-line log tail that every completion turn shares.
fn with_tail(mut body: String, output: &str) -> String {
    let trimmed = output.trim_end();
    let count = trimmed.chars().count();
    let tail = if count > TAIL_CHARS {
        let suffix: String = trimmed.chars().skip(count - TAIL_CHARS).collect();
        format!(
            "…[truncated {} chars — full log at the path above]…\n{suffix}",
            count - TAIL_CHARS
        )
    } else {
        trimmed.to_string()
    };
    if !tail.is_empty() {
        let flattened: Vec<&str> = tail
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        body.push_str(" · tail: ");
        body.push_str(&flattened.join(" ⏎ "));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use pij_core::model::{Harness, Seq};
    use pij_core::ports::Spine;
    use pij_harnesses::InteractionGate;
    use pij_testkit::fakes::{
        FakeLiveness, FakeQueue, FakeRegistry, FakeSpine, FakeTmux, FakeTransport,
    };
    use pij_testkit::fresh_dir;

    const PROC: ProcIdentity = ProcIdentity {
        pid: 987_654,
        proc_start: 123,
    };

    struct Fixture {
        service: BackgroundService,
        registry: Arc<FakeRegistry>,
        transport: Arc<FakeTransport>,
        spine: Arc<FakeSpine>,
        delivery: Arc<DeliveryService>,
        routing: Arc<FakeRouting>,
        dir: PathBuf,
    }

    /// Scripted cold-routing facts: which seats are cold (and idle how long),
    /// who the prime is, and every Telegram the service would have sent.
    #[derive(Default)]
    struct FakeRouting {
        cold: std::sync::Mutex<HashMap<SeatId, u64>>,
        prime: std::sync::Mutex<Option<SeatId>>,
        telegrams: std::sync::Mutex<Vec<(SeatId, String, String)>>,
    }

    #[async_trait]
    impl ColdRouting for FakeRouting {
        async fn check(&self, seat: &SeatDescriptor) -> ColdCheck {
            match self.cold.lock().unwrap().get(&seat.id) {
                Some(idle_ms) => ColdCheck::Cold {
                    context_tokens: 472_000,
                    idle_ms: *idle_ms,
                    model: None,
                    estimate_usd: None,
                },
                None => ColdCheck::Clear,
            }
        }

        async fn prime(&self, _seat: &SeatDescriptor) -> Result<Option<SeatId>> {
            Ok(self.prime.lock().unwrap().clone())
        }

        async fn telegram(&self, from: &SeatId, body: String, msg_id: String) -> Result<()> {
            self.telegrams
                .lock()
                .unwrap()
                .push((from.clone(), body, msg_id));
            Ok(())
        }
    }

    impl Fixture {
        async fn new(liveness: FakeLiveness) -> Self {
            let dir = fresh_dir("pij-bg-runtime");
            let registry = Arc::new(FakeRegistry::new());
            let mut owner = SeatDescriptor::new("owner", Harness::Claude, dir.to_string_lossy());
            owner.proc = Some(ProcIdentity {
                pid: 7,
                proc_start: 11,
            });
            owner.parent = Some(SeatId::from("parent"));
            registry.put(owner).await.unwrap();
            let spine = Arc::new(FakeSpine::new());
            let event_bus = Arc::new(EventBus::new(spine.clone(), 16).unwrap());
            let transport = Arc::new(FakeTransport::reachable());
            let routing = Arc::new(FakeRouting::default());
            let delivery = Arc::new(
                DeliveryService::new(
                    registry.clone(),
                    Arc::new(FakeQueue::new(32).unwrap()),
                    transport.clone(),
                    Arc::new(InteractionGate::new(Arc::new(FakeTmux::new()))),
                    event_bus.clone(),
                )
                .unwrap(),
            );
            let service = BackgroundService::new(
                SqliteBackground::new(pij_store::open("").await.unwrap()),
                BackgroundPorts {
                    registry: registry.clone(),
                    liveness: Arc::new(liveness),
                    delivery: delivery.clone(),
                    event_bus,
                    routing: routing.clone(),
                },
                dir.clone(),
                "127.0.0.1:1".to_string(),
            );
            Self {
                service,
                registry,
                transport,
                spine,
                delivery,
                routing,
                dir,
            }
        }

        /// A live event source owned by `owner`, whose hook token is `token`.
        async fn source(
            &self,
            job_id: &str,
            token: &str,
            fyi: bool,
            min_interval_ms: u64,
            inline_max: u64,
        ) -> BackgroundJob {
            let job = BackgroundJob {
                job_id: job_id.to_string(),
                owner: SeatId::from("owner"),
                title: "db-watch".to_string(),
                command: "watch".to_string(),
                pid: Some(PROC.pid),
                proc_start: Some(PROC.proc_start),
                pgid: Some(PROC.pid),
                out_path: self
                    .dir
                    .join(format!("{job_id}.log"))
                    .to_string_lossy()
                    .into_owned(),
                state: BackgroundState::Running,
                exit_code: None,
                started_at: 1_700_000_000_000,
                finished_at: None,
                kill_requested: false,
                notified: false,
                deadline_at: None,
                timed_out: false,
                term_sent: false,
                kind: BackgroundKind::Events,
                events_fyi: fyi,
                min_interval_ms,
                inline_max,
                last_wake_at: None,
                batches: 0,
            };
            std::fs::write(&job.out_path, "watching\n").unwrap();
            self.service
                .store
                .insert_with_token(&job, Some(&token_digest(token)))
                .await
                .unwrap();
            job
        }

        async fn fire(&self, job: &str, token: &str, texts: &[&str]) {
            for text in texts {
                self.service.emit(job, token, text, None).await.unwrap();
            }
        }

        async fn set_owner_state(&self, state: pij_core::model::SystemState) {
            let mut owner = self
                .registry
                .get(&SeatId::from("owner"))
                .await
                .unwrap()
                .unwrap();
            owner.state = state;
            self.registry.put(owner).await.unwrap();
        }

        /// A reachable seat, like the owner: a process identity makes delivery push.
        async fn add_seat(&self, id: &str) {
            let mut seat = SeatDescriptor::new(id, Harness::Claude, self.dir.to_string_lossy());
            seat.proc = Some(ProcIdentity {
                pid: 8,
                proc_start: 12,
            });
            self.registry.put(seat).await.unwrap();
        }

        fn to(&self, seat: &str) -> Vec<Msg> {
            self.transport
                .delivered()
                .into_iter()
                .filter(|msg| msg.to == SeatId::from(seat))
                .collect()
        }

        async fn held_fyis(&self) -> u64 {
            self.delivery
                .pending_fyi_count(&SeatId::from("owner"))
                .await
                .unwrap()
        }

        async fn running(&self) -> BackgroundJob {
            self.running_with_deadline(None).await
        }

        async fn running_with_deadline(&self, deadline_at: Option<u64>) -> BackgroundJob {
            let job = BackgroundJob {
                job_id: "bg-recovery".to_string(),
                owner: SeatId::from("owner"),
                title: "build".to_string(),
                command: "exit 7".to_string(),
                pid: Some(PROC.pid),
                proc_start: Some(PROC.proc_start),
                pgid: Some(PROC.pid),
                out_path: self
                    .dir
                    .join("bg-recovery.log")
                    .to_string_lossy()
                    .into_owned(),
                state: BackgroundState::Running,
                exit_code: None,
                started_at: 1_700_000_000_000,
                finished_at: None,
                kill_requested: false,
                notified: false,
                deadline_at,
                timed_out: false,
                term_sent: false,
                kind: BackgroundKind::Oneshot,
                events_fyi: false,
                min_interval_ms: 0,
                inline_max: 0,
                last_wake_at: None,
                batches: 0,
            };
            std::fs::write(&job.out_path, "one\ntwo\n").unwrap();
            self.service.store.insert(&job).await.unwrap();
            job
        }
    }

    fn adapter(error: &PijError) -> &str {
        match error {
            PijError::Adapter { adapter, .. } => adapter,
            _ => "",
        }
    }

    #[tokio::test]
    async fn emit_accepts_only_the_jobs_own_live_token() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        fixture.source("bg-a", "token-a", false, 0, 5).await;
        fixture.source("bg-b", "token-b", false, 0, 5).await;
        let service = &fixture.service;
        for (job, token) in [("bg-a", "token-b"), ("bg-missing", "token-a"), ("bg-a", "")] {
            let error = service.emit(job, token, "x", None).await.unwrap_err();
            assert_eq!(adapter(&error), "background/auth", "{job} with {token:?}");
        }
        assert_eq!(
            service
                .emit("bg-a", "token-a", "first", None)
                .await
                .unwrap(),
            EmitOutcome::Accepted { seq: 1 }
        );
        assert!(service.store.request_kill("bg-a").await.unwrap());
        let error = service
            .emit("bg-a", "token-a", "after kill", None)
            .await
            .unwrap_err();
        assert_eq!(adapter(&error), "background/refused");
        service
            .store
            .finish("bg-b", BackgroundState::Done, Some(0), 1_700_000_001_000)
            .await
            .unwrap();
        let error = service
            .emit("bg-b", "token-b", "after exit", None)
            .await
            .unwrap_err();
        assert_eq!(adapter(&error), "background/auth", "the token dies at exit");
        assert_eq!(service.store.token_hash("bg-b").await.unwrap(), None);
    }

    #[tokio::test]
    async fn batches_list_up_to_inline_max_inline_and_write_a_file_beyond_it() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        let job = fixture.source("bg-src", "tok", false, 0, 5).await;
        fixture
            .fire("bg-src", "tok", &["e1", "e2", "e3", "e4", "e5"])
            .await;
        fixture.service.tick().await.unwrap();
        let first = fixture.to("owner");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].msg_id, "pij-bg:bg-src:batch:1");
        assert_eq!(
            first[0].body,
            "[pij bg] 5 new events from db-watch (job bg-src):\n1. e1\n2. e2\n3. e3\n4. e4\n5. e5"
        );
        fixture
            .fire("bg-src", "tok", &["f1", "f2", "f3", "f4", "f5", "f6"])
            .await;
        fixture.service.tick().await.unwrap();
        let path = fixture.service.batch_path(&job, 2);
        let second = fixture.to("owner");
        assert_eq!(
            second[1].body,
            format!(
                "[pij bg] 6 new events from db-watch, here is the file: {}",
                path.display()
            )
        );
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["events"].as_array().unwrap().len(), 6);
        assert_eq!(file["events"][5]["text"], "f6");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let stats = fixture.service.event_stats("bg-src").await.unwrap();
        assert_eq!((stats.fired, stats.pending), (11, 0));
    }

    #[tokio::test]
    async fn events_coalesce_while_the_owner_is_busy_or_within_min_interval() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        fixture.source("bg-src", "tok", false, 300, 5).await;
        fixture.fire("bg-src", "tok", &["one"]).await;
        fixture.service.tick().await.unwrap();
        assert_eq!(fixture.to("owner").len(), 1);
        fixture.fire("bg-src", "tok", &["two", "three"]).await;
        fixture.service.tick().await.unwrap();
        assert_eq!(fixture.to("owner").len(), 1, "inside --min-interval");
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        fixture
            .set_owner_state(pij_core::model::SystemState::Working)
            .await;
        fixture.fire("bg-src", "tok", &["four"]).await;
        fixture.service.tick().await.unwrap();
        assert_eq!(fixture.to("owner").len(), 1, "a busy owner is not woken");
        fixture
            .set_owner_state(pij_core::model::SystemState::Idle)
            .await;
        fixture.service.tick().await.unwrap();
        let turns = fixture.to("owner");
        assert_eq!(turns.len(), 2);
        assert_eq!(
            turns[1].body,
            "[pij bg] 3 new events from db-watch (job bg-src):\n1. two\n2. three\n3. four"
        );
    }

    #[tokio::test]
    async fn fyi_sources_hold_batches_without_waking_even_a_busy_owner() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        fixture.source("bg-src", "tok", true, 0, 5).await;
        fixture
            .set_owner_state(pij_core::model::SystemState::Working)
            .await;
        fixture.fire("bg-src", "tok", &["quiet"]).await;
        fixture.service.tick().await.unwrap();
        assert!(fixture.to("owner").is_empty());
        assert_eq!(fixture.held_fyis().await, 1);
    }

    /// A cold owner with two pending events; returns the batch file's path.
    async fn cold_owner_batch(fixture: &Fixture) -> PathBuf {
        let job = fixture.source("bg-src", "tok", false, 0, 5).await;
        fixture
            .routing
            .cold
            .lock()
            .unwrap()
            .insert(SeatId::from("owner"), 18_720_000);
        fixture.fire("bg-src", "tok", &["row 1", "row 2"]).await;
        fixture.service.tick().await.unwrap();
        assert!(
            fixture.to("owner").is_empty(),
            "a cold owner is never woken"
        );
        assert_eq!(fixture.held_fyis().await, 1, "the batch waits as an FYI");
        let path = fixture.service.batch_path(&job, 1);
        assert!(path.exists(), "cold routing always cites a file");
        path
    }

    #[tokio::test]
    async fn cold_owner_with_a_warm_prime_goes_to_the_prime() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        fixture.add_seat("prime").await;
        *fixture.routing.prime.lock().unwrap() = Some(SeatId::from("prime"));
        let path = cold_owner_batch(&fixture).await;
        let notices = fixture.to("prime");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].msg_id, "pij-bg:bg-src:batch:1:prime");
        assert_eq!(
            notices[0].body,
            format!(
                "❄ owner is cold (472k, idle 5h12m): 2 events from db-watch are held.\n\
                 They wait as an FYI on owner, which sees them on its next turn. \
                 To wake it now: pij send owner --force --reason \"<why>\"\nEvents: {}",
                path.display()
            )
        );
        assert!(fixture.routing.telegrams.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cold_owner_with_a_cold_prime_goes_to_telegram() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        fixture.add_seat("prime").await;
        *fixture.routing.prime.lock().unwrap() = Some(SeatId::from("prime"));
        fixture
            .routing
            .cold
            .lock()
            .unwrap()
            .insert(SeatId::from("prime"), 7_200_000);
        let path = cold_owner_batch(&fixture).await;
        assert!(fixture.to("prime").is_empty());
        let telegrams = fixture.routing.telegrams.lock().unwrap().clone();
        assert_eq!(
            telegrams,
            vec![(
                SeatId::from("owner"),
                format!(
                    "❄ owner is cold (472k, idle 5h12m): 2 events from db-watch are held.\n\
                     Its prime prime is cold (idle 2h00m), so this came to you.\nEvents: {}",
                    path.display()
                ),
                "pij-bg:bg-src:batch:1:telegram".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn cold_owner_without_a_prime_goes_to_telegram() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        let path = cold_owner_batch(&fixture).await;
        let telegrams = fixture.routing.telegrams.lock().unwrap().clone();
        assert_eq!(telegrams.len(), 1);
        assert_eq!(
            telegrams[0].1,
            format!(
                "❄ owner is cold (472k, idle 5h12m): 2 events from db-watch are held.\n\
                 It has no prime, so this came to you.\nEvents: {}",
                path.display()
            )
        );
    }

    #[tokio::test]
    async fn cold_owner_whose_prime_is_gone_goes_to_telegram() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        *fixture.routing.prime.lock().unwrap() = Some(SeatId::from("departed"));
        cold_owner_batch(&fixture).await;
        let telegrams = fixture.routing.telegrams.lock().unwrap().clone();
        assert_eq!(telegrams.len(), 1);
        assert!(
            telegrams[0]
                .1
                .contains("Its prime departed is gone, so this came to you."),
            "{}",
            telegrams[0].1
        );
    }

    #[tokio::test]
    async fn a_killed_source_flushes_pending_events_into_its_final_stopped_turn() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.source("bg-src", "tok", false, 0, 5).await;
        fixture
            .set_owner_state(pij_core::model::SystemState::Working)
            .await;
        fixture.fire("bg-src", "tok", &["late 1", "late 2"]).await;
        assert!(fixture.service.store.request_kill("bg-src").await.unwrap());
        std::fs::write(receipt_path(&job), "143 1700000009\n").unwrap();
        fixture.service.tick().await.unwrap();
        let turns = fixture.to("owner");
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].msg_id, "pij-bg:bg-src:finished");
        assert_eq!(
            turns[0].body,
            format!(
                "[pij bg] STOPPED — db-watch · full log: {}\n2 events since the last batch:\n1. late 1\n2. late 2",
                job.out_path
            )
        );
        let stats = fixture.service.event_stats("bg-src").await.unwrap();
        assert_eq!(stats.pending, 0);
        assert_eq!(
            fixture.service.store.token_hash("bg-src").await.unwrap(),
            None
        );
        fixture.service.tick().await.unwrap();
        assert_eq!(fixture.to("owner").len(), 1, "the final turn is sent once");
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn restart_matching_alive_without_receipt_keeps_waiting() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        let job = fixture.running().await;
        fixture.service.tick().await.unwrap();
        assert_eq!(fixture.service.lookup(&job.job_id).await.unwrap(), job);
        assert!(fixture.transport.delivered().is_empty());
    }

    #[tokio::test]
    async fn restart_matching_alive_waits_even_with_receipt() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        let job = fixture.running().await;
        std::fs::write(receipt_path(&job), "7 1700000002\n").unwrap();
        fixture.service.tick().await.unwrap();
        assert_eq!(
            fixture.service.lookup(&job.job_id).await.unwrap().state,
            BackgroundState::Running
        );
    }

    #[tokio::test]
    async fn restart_gone_without_receipt_is_lost_not_log_inferred_success() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running().await;
        std::fs::write(&job.out_path, "exit 0\nSUCCESS\n").unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Lost);
        assert_eq!(finished.exit_code, None);
        assert_eq!(
            fixture.transport.delivered()[0].body,
            format!("[pij bg] LOST — build · full log: {}", job.out_path)
        );
    }

    #[tokio::test]
    async fn restart_recycled_pid_is_lost_without_signalling() {
        let fixture = Fixture::new(FakeLiveness::new().with_recycled(PROC.pid, 124)).await;
        let job = fixture.running().await;
        fixture
            .service
            .store
            .request_kill(&job.job_id)
            .await
            .unwrap();
        fixture.service.tick().await.unwrap();
        assert_eq!(
            fixture.service.lookup(&job.job_id).await.unwrap().state,
            BackgroundState::Lost
        );
    }

    #[tokio::test]
    async fn restart_gone_with_valid_receipt_preserves_source_exit_and_finish_time() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running().await;
        std::fs::write(receipt_path(&job), "7 1700000002\n").unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Done);
        assert_eq!(finished.exit_code, Some(7));
        assert_eq!(finished.finished_at, Some(1_700_000_002_000));
        assert!(finished.notified);
        assert!(
            fixture
                .registry
                .get(&SeatId::from(BG_ACTOR))
                .await
                .unwrap()
                .is_none()
        );
        let delivered = fixture.transport.delivered();
        assert_eq!(delivered[0].from, SeatId::from(BG_ACTOR));
        assert_eq!(delivered[0].msg_id, "pij-bg:bg-recovery:finished");
        assert_eq!(
            delivered[0].body,
            format!(
                "[pij bg] FAILED (exit 7) — build (2s) · full log: {} · tail: one ⏎ two",
                job.out_path
            )
        );
        fixture.service.tick().await.unwrap();
        assert_eq!(fixture.transport.delivered().len(), 1);
        let events = fixture.spine.tail(None, Seq(0)).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "bg.finished")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn source_receipt_survives_wall_clock_moving_backwards() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running().await;
        std::fs::write(receipt_path(&job), "7 1699999900\n").unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Done);
        assert_eq!(finished.exit_code, Some(7));
        assert_eq!(finished.finished_at, Some(1_699_999_900_000));
    }

    #[tokio::test]
    async fn restart_recycled_with_source_receipt_recovers_done_not_lost() {
        let fixture = Fixture::new(FakeLiveness::new().with_recycled(PROC.pid, 124)).await;
        let job = fixture.running().await;
        std::fs::write(receipt_path(&job), "0 1700000002\n").unwrap();
        fixture.service.tick().await.unwrap();
        assert_eq!(
            fixture.service.lookup(&job.job_id).await.unwrap().state,
            BackgroundState::Done
        );
    }

    #[tokio::test]
    async fn only_owner_and_direct_parent_can_read_and_list_all_adds_only_children() {
        let fixture = Fixture::new(FakeLiveness::new().with_proc(PROC)).await;
        let job = fixture.running().await;
        assert!(
            fixture
                .service
                .get(&SeatId::from("owner"), &job.job_id)
                .await
                .is_ok()
        );
        assert!(
            fixture
                .service
                .get(&SeatId::from("parent"), &job.job_id)
                .await
                .is_ok()
        );
        assert!(
            fixture
                .service
                .tail(&SeatId::from("stranger"), &job.job_id, 20)
                .await
                .is_err()
        );
        assert!(
            fixture
                .service
                .kill(&SeatId::from("stranger"), &job.job_id)
                .await
                .is_err()
        );
        assert!(
            fixture
                .service
                .list(&SeatId::from("parent"), false)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fixture
                .service
                .list(&SeatId::from("parent"), true)
                .await
                .unwrap()
                .len(),
            1
        );
        let mut parent = SeatDescriptor::new("parent", Harness::Omp, "/tmp");
        parent.parent = Some(SeatId::from("grandparent"));
        fixture.registry.put(parent).await.unwrap();
        assert!(
            fixture
                .service
                .list(&SeatId::from("grandparent"), true)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            fixture
                .service
                .get(&SeatId::from("grandparent"), &job.job_id)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_runner_that_finished_before_its_deadline_keeps_its_exit_when_the_tick_is_late() {
        // Review F1 (PR #21): the daemon was down, or a tick ran late, across the
        // deadline; the runner had already exited on its own and written its receipt.
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running_with_deadline(Some(1_700_000_005_000)).await;
        std::fs::write(receipt_path(&job), "3 1700000002\n").unwrap();
        fixture.service.tick().await.unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Done);
        assert_eq!(finished.exit_code, Some(3));
        assert!(!finished.timed_out && !finished.kill_requested);
        let delivered = fixture.transport.delivered();
        assert_eq!(delivered.len(), 1, "exactly one completion turn");
        assert!(
            delivered[0]
                .body
                .starts_with("[pij bg] FAILED (exit 3) — build"),
            "{}",
            delivered[0].body
        );
    }

    #[tokio::test]
    async fn a_timeout_claim_that_lost_the_race_to_the_runner_reports_its_own_exit() {
        // The runner exits between the live check and the TERM: kill intent is
        // recorded, but only the trap's 143 proves the timeout ended it.
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running_with_deadline(Some(1_700_000_005_000)).await;
        assert!(
            fixture
                .service
                .store
                .request_timeout(&job.job_id, 1_700_000_006_000)
                .await
                .unwrap()
        );
        std::fs::write(receipt_path(&job), "0 1700000006\n").unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Done);
        assert_eq!(finished.exit_code, Some(0));
        assert!(
            fixture.transport.delivered()[0]
                .body
                .starts_with("[pij bg] OK — build"),
            "{}",
            fixture.transport.delivered()[0].body
        );
    }

    #[tokio::test]
    async fn an_ordinary_exit_143_racing_a_timeout_claim_stays_failed_exit_143() {
        // Re-review R1 (PR #21): the command itself exits 143 just as the timeout
        // is claimed; no TERM ever reached the runner, so this is not a TIMEOUT.
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running_with_deadline(Some(1_700_000_005_000)).await;
        assert!(
            fixture
                .service
                .store
                .request_timeout(&job.job_id, 1_700_000_006_000)
                .await
                .unwrap()
        );
        std::fs::write(receipt_path(&job), "143 1700000006\n").unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Done);
        assert_eq!(finished.exit_code, Some(143));
        let delivered = fixture.transport.delivered();
        assert_eq!(delivered.len(), 1);
        assert!(
            delivered[0]
                .body
                .starts_with("[pij bg] FAILED (exit 143) — build"),
            "{}",
            delivered[0].body
        );
    }

    #[tokio::test]
    async fn a_timeout_whose_term_was_sent_reads_timeout_once() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running_with_deadline(Some(1_700_000_005_000)).await;
        assert!(
            fixture
                .service
                .store
                .request_timeout(&job.job_id, 1_700_000_006_000)
                .await
                .unwrap()
        );
        assert!(
            fixture
                .service
                .store
                .set_term_sent(&job.job_id, true)
                .await
                .unwrap()
        );
        std::fs::write(receipt_path(&job), "143 1700000006\n").unwrap();
        fixture.service.tick().await.unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(finished.state, BackgroundState::Killed);
        assert!(finished.timed_out);
        let delivered = fixture.transport.delivered();
        assert_eq!(delivered.len(), 1, "exactly one TIMEOUT turn");
        assert!(
            delivered[0]
                .body
                .starts_with("[pij bg] TIMEOUT — build (killed after 5s)"),
            "{}",
            delivered[0].body
        );
    }

    #[tokio::test]
    async fn a_runner_that_exits_zero_after_the_timeout_term_was_sent_keeps_ok() {
        // TERM sent, but the command finished cleanly first: its own success wins.
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running_with_deadline(Some(1_700_000_005_000)).await;
        let store = &fixture.service.store;
        assert!(
            store
                .request_timeout(&job.job_id, 1_700_000_006_000)
                .await
                .unwrap()
        );
        assert!(store.set_term_sent(&job.job_id, true).await.unwrap());
        std::fs::write(receipt_path(&job), "0 1700000006\n").unwrap();
        fixture.service.tick().await.unwrap();
        let finished = fixture.service.lookup(&job.job_id).await.unwrap();
        assert_eq!(
            (finished.state, finished.exit_code),
            (BackgroundState::Done, Some(0))
        );
    }

    #[tokio::test]
    async fn kill_receipt_uses_exact_legacy_killed_wording() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let job = fixture.running().await;
        fixture
            .service
            .store
            .request_kill(&job.job_id)
            .await
            .unwrap();
        std::fs::write(receipt_path(&job), "143 1700000002\n").unwrap();
        fixture.service.tick().await.unwrap();
        assert_eq!(
            fixture.service.lookup(&job.job_id).await.unwrap().state,
            BackgroundState::Killed
        );
        assert_eq!(
            fixture.transport.delivered()[0].body,
            format!("[pij bg] KILLED — build · full log: {}", job.out_path)
        );
    }

    #[tokio::test]
    async fn completion_bounds_twenty_lines_and_twelve_hundred_characters() {
        let fixture = Fixture::new(FakeLiveness::new()).await;
        let mut job = fixture.running().await;
        let lines = (0..30)
            .map(|number| format!("line-{number}\n"))
            .collect::<String>();
        std::fs::write(&job.out_path, &lines).unwrap();
        let tail = read_tail(Path::new(&job.out_path), 20).unwrap();
        assert_eq!(tail.len(), 20);
        assert_eq!(tail[0], "line-10");
        job.state = BackgroundState::Done;
        job.exit_code = Some(0);
        job.finished_at = Some(job.started_at + 65_000);
        let body = completion_turn(&job, &format!("{}\n", "x".repeat(1300)));
        assert_eq!(
            body,
            format!(
                "[pij bg] OK — build (1m05s) · full log: {} · tail: …[truncated 100 chars — full log at the path above]… ⏎ {}",
                job.out_path,
                "x".repeat(1200)
            )
        );
        assert!(!body.contains('\n'));
    }

    #[test]
    fn sparse_huge_log_tail_reads_only_bounded_suffix() {
        let dir = fresh_dir("pij-bg-tail");
        let path = dir.join("huge.log");
        let mut file = File::create(&path).unwrap();
        file.set_len(2 * 1024 * 1024 * 1024).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(b"\nlast\n").unwrap();
        assert_eq!(read_tail(&path, 1).unwrap(), vec!["last"]);
        assert!(
            read_tail(&path, MAX_LINES)
                .unwrap()
                .iter()
                .map(String::len)
                .sum::<usize>()
                <= TAIL_BYTES as usize
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn receipt_requires_exit_and_source_timestamp_not_partial_or_overflowing_data() {
        let dir = fresh_dir("pij-bg-receipt");
        let path = dir.join("job.exit");
        for invalid in [
            "0",
            "0 0",
            "0 18446744073709551615",
            "0 1700000000 extra",
            "-1 1700000000",
            "256 1700000000",
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(read_receipt(&path).unwrap().is_none(), "{invalid}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn runner_announces_installed_handlers_before_waiting_for_launch_gate() {
        let dir = fresh_dir("pij-bg-ready");
        let sentinel = dir.join("must-not-exist");
        let receipt = dir.join("job.exit");
        let ready = dir.join("job.exit.ready");
        let mut child = Command::new("/bin/sh")
            .args(["-c", RUNNER])
            .env("PIJ_BG_COMMAND", "touch \"$PIJ_BG_SENTINEL\"")
            .env("PIJ_BG_SENTINEL", &sentinel)
            .env("PIJ_BG_EXIT", &receipt)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !ready.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready_before_release = ready.exists();
        let work_was_not_run = !sentinel.exists();
        drop(child.stdin.take());
        let status = child.wait().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            ready_before_release,
            "runner must announce installed handlers before gate release"
        );
        assert!(work_was_not_run);
        assert_eq!(status.code(), Some(125));
    }

    #[test]
    fn runner_stdin_eof_never_executes_command() {
        let dir = fresh_dir("pij-bg-gate");
        let sentinel = dir.join("must-not-exist");
        let mut child = Command::new("/bin/sh")
            .args(["-c", RUNNER])
            .env("PIJ_BG_COMMAND", "touch \"$PIJ_BG_SENTINEL\"")
            .env("PIJ_BG_SENTINEL", &sentinel)
            .env("PIJ_BG_EXIT", dir.join("job.exit"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        drop(child.stdin.take());
        assert_eq!(child.wait().unwrap().code(), Some(125));
        assert!(!sentinel.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
