//! Daemon-owned jobs with identity-braked signalling and runner-authored receipts.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use pij_core::BG_ACTOR;
use pij_core::background::{BackgroundJob, BackgroundState};
use pij_core::error::{PijError, Result};
use pij_core::model::{Event, Msg, ProcIdentity, SeatDescriptor, SeatId};
use pij_core::ports::{LivenessPort, Registry};
use pij_store::background::SqliteBackground;
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

/// Caller-chosen launch options beyond the title and command.
#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    /// Working directory for the command; the owner's recorded folder when absent.
    pub cwd: Option<PathBuf>,
    /// Kill the job with a TIMEOUT turn once it has run this long.
    pub timeout_ms: Option<u64>,
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
    children: Mutex<HashMap<String, Child>>,
}

impl BackgroundService {
    /// Compose the durable store, existing delivery pipeline and private output directory.
    pub fn new(
        store: SqliteBackground,
        registry: Arc<dyn Registry>,
        liveness: Arc<dyn LivenessPort>,
        delivery: Arc<DeliveryService>,
        event_bus: Arc<EventBus>,
        out_dir: PathBuf,
        daemon_addr: String,
    ) -> Self {
        Self {
            store,
            registry,
            liveness,
            delivery,
            event_bus,
            out_dir,
            daemon_addr,
            children: Mutex::new(HashMap::new()),
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
        self.store.insert(&job).await?;
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
        // Group membership is checked before the final identity observation.
        // Nothing awaits between that observation and spawning the fixed signal.
        let observed_group = blocking(move || process_group(identity.pid)).await?;
        if observed_group != Some(pgid) {
            return Err(refusal(
                "runner process group changed or disappeared; no signal sent",
            ));
        }
        if self.liveness.proc_start(identity.pid).await? != Some(identity.proc_start) {
            return Err(refusal(
                "runner identity changed or disappeared; no signal sent",
            ));
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
        Ok(())
    }

    /// Reap local children, recover orphaned rows by identity/receipt, and notify endings.
    ///
    /// # Errors
    /// Store, liveness, receipt, event, or delivery failures. Other jobs are still
    /// visited when one fails, and unnotified terminal rows are retried next tick.
    pub async fn tick(&self) -> Result<()> {
        let mut children = self.children.lock().await;
        let mut first_error = None;
        let now = now_ms()?;
        for mut job in self.store.pending().await? {
            if job.state == BackgroundState::Running
                && !job.kill_requested
                && job.deadline_at.is_some_and(|deadline| deadline <= now)
            {
                match self.store.request_timeout(&job.job_id, now).await {
                    // Reconcile below signals a live runner with kill intent.
                    Ok(true) => {
                        job.kill_requested = true;
                        job.timed_out = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        first_error.get_or_insert(error);
                        continue;
                    }
                }
            }
            if let Err(error) = self.reconcile(&job, &mut children).await {
                first_error.get_or_insert(error);
                continue;
            }
            let result = match self.lookup(&job.job_id).await {
                Ok(current) if terminal(current.state) => self.notify(&current).await,
                Ok(_) => Ok(()),
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
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
                if job.kill_requested {
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
        let path = PathBuf::from(&job.out_path);
        let output = if job.state == BackgroundState::Done || job.timed_out {
            blocking(move || read_tail(&path, 20).map(|lines| lines.join("\n"))).await?
        } else {
            String::new()
        };
        let body = completion_turn(job, &output);
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
        self.store.mark_notified(&job.job_id).await?;
        Ok(())
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
        return format!(
            "[pij bg] KILLED — {} · full log: {}",
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
        dir: PathBuf,
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
                registry.clone(),
                Arc::new(liveness),
                delivery,
                event_bus,
                dir.clone(),
                "127.0.0.1:1".to_string(),
            );
            Self {
                service,
                registry,
                transport,
                spine,
                dir,
            }
        }

        async fn running(&self) -> BackgroundJob {
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
                deadline_at: None,
                timed_out: false,
            };
            std::fs::write(&job.out_path, "one\ntwo\n").unwrap();
            self.service.store.insert(&job).await.unwrap();
            job
        }
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
