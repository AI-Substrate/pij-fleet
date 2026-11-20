//! A persistent tmux control-mode client for read-only queries (plan 180).
//!
//! Every forked `tmux` client is a process launch, and Defender's EDR inspects
//! each one (pij-fleet#62). The pane observer alone forked three tmux clients
//! per pane every 600 ms. One `tmux -C` client answers the same reads
//! (`list-panes`, `capture-pane`, `display-message -p`, `show-options`) over a
//! pipe instead.
//!
//! Directional by construction: control mode only REPLACES a forked read with
//! the same tmux command. Whenever it is off, starting, slow, lost or handed an
//! argument it cannot quote, the caller forks exactly as before, so it can only
//! ever remove launches, never change an answer.
//!
//! The write barrier is [`is_read`] plus [`command_line`]'s quoting, and nothing
//! else. The `read-only` attach flag only refuses keyboard input from the
//! client; commands sent over the pipe run with full rights (pij-fleet#73
//! review S2). Widening `is_read` is a safety change.
//!
//! The client lives in its own session, [`CONTROL_SESSION`], never one of the
//! operator's (R-PRIME-1): attached there, it made a bare `tmux attach` skip the
//! session the human last used.
//!
//! Attach and setup hold an exclusive `flock` on a lock file per tmux server
//! (F229): two control clients attaching to one server at once, each sending
//! the setup's `destroy-unattached on`, SIGSEGV the tmux 3.6a server and take
//! every pane on it down. The lock is a brake: it can only delay or refuse an
//! attach. While another process holds it, reads are skipped ([`Query::Busy`])
//! rather than forked, so contention launches nothing and nothing waits. The
//! lock is keyed by the server's socket path (F238), so every pij process
//! using one server, whatever its state root, takes the same one.
//!
//! A client leaves through [`Live`]'s teardown (F238): over its own still
//! attached pipe it kills `_pij` when it is `_pij`'s only client and `_pij`
//! holds only the control client's own idle pane, and otherwise switches
//! `_pij`'s `destroy-unattached` off; only then is it killed. Leaving it to
//! `destroy-unattached` crashed tmux (3.4, and rarely 3.6a): a client that is
//! dead but not yet lost does not count as attached, so `_pij` was destroyed
//! under it, and its own `client-detached` notify read the destroyed session.
//! `destroy-unattached` stays on while attached, and only while `_pij` holds
//! nothing but the idle pane, so a client that dies uncleanly (SIGKILL, a
//! crash) still has `_pij` cleaned up, at that risk. The whole exit is
//! bounded: the teardown is written and answered on its own thread, waited on
//! for at most [`TEARDOWN_WAIT`], and the killed client is reaped within
//! [`REAP_WAIT`] or handed to a thread that reaps it; if no thread can start,
//! it is waited on for [`REAP_WAIT`] more and then left (F240).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Proves the pipe end to end before any real query rides it.
const READY_SENTINEL: &str = "pij-control-ready";
/// A healthy server answers in milliseconds; past this the client is replaced.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
/// After a failed start, fork for this long before trying again.
const RESTART_BACKOFF: Duration = Duration::from_secs(5);
/// How long a start keeps finding another process attaching before it is
/// refused (F229). Meanwhile reads are skipped, launching nothing, and each
/// one tries the lock once, without waiting. A live holder lets go within its
/// own handshake bound ([`REPLY_TIMEOUT`]), so only a frozen holder lasts
/// this long; the refused start then backs off and forks like any other.
const ATTACH_LOCK_WAIT: Duration = Duration::from_secs(5);
/// Attach without resizing windows, without `%output` traffic, and without
/// keyboard input. Not a write barrier: see the module doc.
const ATTACH_FLAGS: &str = "no-output,ignore-size,read-only";
/// The control client's own session, created on first attach (R-PRIME-1).
pub const CONTROL_SESSION: &str = "_pij";
/// What `_pij`'s own pane runs: an idle process, not a shell.
const IDLE_COMMAND: &str = "exec sleep 2147483647";
/// The `list-panes -f` filter for every pane except the control client's own,
/// told apart by its start command (which tmux may report quoted). A window
/// that lands in `_pij` from elsewhere stays visible to pij.
pub(crate) const NOT_CONTROL_PANE: &str = "#{?#{m:*sleep 2147483647*,#{pane_start_command}},0,1}";
/// The subscription that reports whether `_pij` is alone.
const SESSIONS_SUBSCRIPTION: &str = "pij-sessions";
/// `1` when `_pij` is the only session and holds nothing but the control pane.
/// A window or pane that landed there is real work, so `_pij` is then not
/// alone: the client stays, and so does the server.
const ALONE: &str =
    "#{&&:#{==:#{server_sessions},1},#{&&:#{==:#{session_windows},1},#{==:#{window_panes},1}}}";
/// F238: the teardown a leaving client runs while still attached. `_pij` with
/// this client as its only client, one window and one pane, that pane the
/// control client's own idle one (its start command, as [`NOT_CONTROL_PANE`]
/// tells it), is killed. Any other `_pij` stays, with `destroy-unattached` off
/// so no later exit can destroy it under a dying client: a pane that is real
/// work is never killed, even when it is all `_pij` holds. Untargeted: it
/// applies to this client's own session, `_pij`.
const TEARDOWN: [&str; 5] = [
    "if-shell",
    "-F",
    "#{&&:#{==:#{session_attached},1},#{&&:#{==:#{session_windows},1},#{&&:#{==:#{window_panes},1},#{m:*sleep 2147483647*,#{pane_start_command}}}}}",
    "kill-session",
    "set-option destroy-unattached off",
];
/// How long a leaving client waits for its teardown to be written and
/// answered before it is killed anyway: a healthy server answers in
/// milliseconds, and a dead pipe, a full one or a frozen server must never
/// hold an exit (F238).
const TEARDOWN_WAIT: Duration = Duration::from_secs(1);
/// How long a killed client is given to be reaped before a thread takes it
/// over, so a process that outlives SIGKILL never holds an exit (F238).
const REAP_WAIT: Duration = Duration::from_secs(1);
/// How often a killed client is checked while it is reaped.
const REAP_POLL: Duration = Duration::from_millis(5);

/// One command's answer: its output lines, or tmux's error lines.
pub(crate) type Reply = std::result::Result<Vec<Vec<u8>>, Vec<Vec<u8>>>;

/// What one protocol line meant.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A command's block closed.
    Reply(Reply),
    /// The client is detached or the server is gone.
    Exit,
    /// `_pij` is the only session left and holds only the control pane. The
    /// client leaves, so `_pij` (then `destroy-unattached`) cannot keep an
    /// emptied server alive.
    Alone,
}

/// Incremental reader for tmux's control protocol. Pure, so framing is tested.
///
/// A command's output is the lines between `%begin T N F` and the `%end T N F`
/// (or `%error T N F`) carrying the same `T N`. Inside a block every other line
/// is content, even one that starts with `%`. Outside a block, `%`-lines are
/// notifications and are ignored, except `%exit`.
#[derive(Debug, Default)]
pub(crate) struct Framer {
    open: Option<(Vec<u8>, Vec<Vec<u8>>)>,
}

impl Framer {
    pub(crate) fn line(&mut self, line: &[u8]) -> Option<Frame> {
        if let Some((key, lines)) = &mut self.open {
            for (prefix, ok) in [(&b"%end "[..], true), (&b"%error "[..], false)] {
                if line
                    .strip_prefix(prefix)
                    .and_then(block_key)
                    .is_some_and(|closing| closing == key.as_slice())
                {
                    let lines = std::mem::take(lines);
                    self.open = None;
                    return Some(Frame::Reply(if ok { Ok(lines) } else { Err(lines) }));
                }
            }
            lines.push(line.to_vec());
            return None;
        }
        if let Some(key) = line.strip_prefix(b"%begin ").and_then(block_key) {
            self.open = Some((key.to_vec(), Vec::new()));
            return None;
        }
        if line == b"%exit" || line.starts_with(b"%exit ") {
            return Some(Frame::Exit);
        }
        // `%subscription-changed pij-sessions $1 - - - : 1`
        let alone = line
            .strip_prefix(b"%subscription-changed ")
            .and_then(|rest| rest.strip_prefix(SESSIONS_SUBSCRIPTION.as_bytes()))
            .is_some_and(|rest| rest.starts_with(b" ") && rest.ends_with(b" : 1"));
        alone.then_some(Frame::Alone)
    }
}

/// `T N` of a `T N F` guard line.
fn block_key(rest: &[u8]) -> Option<&[u8]> {
    let mut fields = rest.splitn(3, |byte| *byte == b' ');
    let time = fields.next()?;
    let number = fields.next()?;
    fields.next()?;
    let digits = |field: &[u8]| !field.is_empty() && field.iter().all(u8::is_ascii_digit);
    (digits(time) && digits(number)).then(|| &rest[..time.len() + 1 + number.len()])
}

/// The single-quoted command line for `args`, or `None` when an argument
/// cannot be single-quoted (the caller then forks).
pub(crate) fn command_line(args: &[OsString]) -> Option<String> {
    let mut line = String::new();
    for arg in args {
        let arg = arg.to_str()?;
        if arg.contains(['\'', '\n', '\r']) {
            return None;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push('\'');
        line.push_str(arg);
        line.push('\'');
    }
    Some(line)
}

/// Is this tmux argv a read that is safe to send over the control client?
///
/// The write barrier (with [`command_line`]): deny by default. Every command
/// not named here forks, and `read_allowlist_is_exactly_four_reads` pins the set.
pub(crate) fn is_read(args: &[OsString]) -> bool {
    match args.first().and_then(|arg| arg.to_str()) {
        Some("list-panes" | "capture-pane" | "show-options") => true,
        Some("display-message") => args.iter().any(|arg| arg == "-p"),
        _ => false,
    }
}

#[derive(Default)]
struct Waiters {
    queue: VecDeque<SyncSender<Reply>>,
    dead: bool,
    /// Died because `_pij` was the only session: wait before restarting.
    alone: bool,
    /// The reader has stopped: nothing written now can be answered.
    closed: bool,
}

struct Live {
    /// Taken by [`Drop`], which kills it and reaps it, or hands it on.
    child: Option<Child>,
    stdin: ChildStdin,
    waiters: Arc<Mutex<Waiters>>,
}

impl Drop for Live {
    /// Every way a client leaves (the adapter dropped, a replacement after a
    /// slow or lost reply, `_pij` alone, a daemon shutting down) comes here,
    /// and none of it waits longer than [`TEARDOWN_WAIT`] plus [`REAP_WAIT`],
    /// or plus twice [`REAP_WAIT`] when no reaper thread can start (F240).
    fn drop(&mut self) {
        self.leave();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            reap(child);
        }
    }
}

impl Live {
    /// F238: run [`TEARDOWN`] over the still-attached pipe and wait for its
    /// answer, so the server has handled `_pij` before this client can be
    /// lost. The waiter is queued and the line written on a thread of its
    /// own, so a full pipe (a client that stopped reading) holds that thread,
    /// never this one: this waits at most [`TEARDOWN_WAIT`] for the whole of
    /// it, and not at all once the thread has given up (the pipe is closed,
    /// or the write failed). After the kill that follows, a blocked write
    /// fails and the reader's close releases the waiter, so the thread ends.
    fn leave(&mut self) {
        let Ok(pipe) = self.stdin.as_fd().try_clone_to_owned() else {
            return;
        };
        let waiters = Arc::clone(&self.waiters);
        let (done, finished) = mpsc::sync_channel::<()>(1);
        let spawned = std::thread::Builder::new()
            .name("pij-tmux-leave".to_string())
            .spawn(move || {
                // Dropped on every return, which ends the exit's wait.
                let _done = done;
                let (sender, receiver) = mpsc::sync_channel(1);
                {
                    let Ok(mut waiters) = waiters.lock() else {
                        return;
                    };
                    if waiters.closed {
                        return;
                    }
                    waiters.queue.push_back(sender);
                }
                let line = command_line(&TEARDOWN.map(OsString::from)).expect("teardown quotes");
                let mut pipe = File::from(pipe);
                if pipe
                    .write_all(format!("{line}\n").as_bytes())
                    .and_then(|()| pipe.flush())
                    .is_ok()
                {
                    let _ = receiver.recv_timeout(TEARDOWN_WAIT);
                }
            });
        if spawned.is_ok() {
            let _ = finished.recv_timeout(TEARDOWN_WAIT);
        }
    }
}

/// Reap a killed client within [`REAP_WAIT`]; one still there by then is
/// handed to a thread that reaps it whenever it goes, so it never holds the
/// exit and never stays a zombie. If no thread can start (F240), the exit
/// waits [`REAP_WAIT`] more itself; one still there after that is left, and
/// said so with its pid.
fn reap(child: Child) {
    if let Reap::Left(child) = reap_with(child, spawn_reaper) {
        eprintln!(
            "pij-rs tmux: control client {} outlived its kill and no thread could start to reap it; left unreaped",
            child.id()
        );
    }
}

/// Start the thread that reaps a client whenever it goes.
fn spawn_reaper(reaper: Box<dyn FnOnce() + Send>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("pij-tmux-reap".to_string())
        .spawn(reaper)
        .map(drop)
}

/// What became of a killed client.
#[derive(Debug)]
enum Reap {
    /// Reaped (or no longer waitable) within the waits.
    Done,
    /// Handed to a thread that reaps it whenever it goes.
    Handed,
    /// Still there after both waits, with no thread to wait for it (F240).
    Left(Child),
}

/// [`reap`], with the reaper thread started by `spawn`. When `spawn` fails
/// (no thread can start: thread or resource exhaustion), the child would be
/// dropped unwaited and left a zombie, so the caller waits up to
/// [`REAP_WAIT`] more itself. The exit is held at most twice [`REAP_WAIT`]
/// here, and only on that failure; otherwise at most [`REAP_WAIT`].
fn reap_with(
    mut child: Child,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
) -> Reap {
    if reaped_within(&mut child, REAP_WAIT) {
        return Reap::Done;
    }
    // Shared, so a thread that never starts (its closure dropped unrun)
    // leaves the child here to be waited on.
    let slot = Arc::new(Mutex::new(Some(child)));
    let reaper = Arc::clone(&slot);
    let handed = spawn(Box::new(move || {
        if let Some(mut child) = reaper.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = child.wait();
        }
    }));
    if handed.is_ok() {
        return Reap::Handed;
    }
    let Some(mut child) = slot.lock().ok().and_then(|mut slot| slot.take()) else {
        return Reap::Done;
    };
    if reaped_within(&mut child, REAP_WAIT) {
        Reap::Done
    } else {
        Reap::Left(child)
    }
}

/// Poll `child` every [`REAP_POLL`] for at most `wait`: true once it is
/// reaped (or can no longer be waited on).
fn reaped_within(child: &mut Child, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) if Instant::now() >= deadline => return false,
            Ok(None) => std::thread::sleep(REAP_POLL),
        }
    }
}

#[derive(Default)]
struct State {
    live: Option<Live>,
    retry_after: Option<Instant>,
    /// When this client first found another process attaching (F229).
    contended_since: Option<Instant>,
    /// [`ControlClient::release`] ran: no client starts again.
    released: bool,
}

/// What a read over the control client came to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Query {
    /// The control client answered.
    Answer(Reply),
    /// No control client: the caller forks the read, as before plan 180.
    Fork,
    /// Another process is attaching its control client to this server
    /// (F229). The read is skipped, launching nothing: the caller reports it
    /// unknown, never forks.
    Busy,
}

/// While another process holds the attach lock, is a start still waiting
/// for it (reads skipped), or is it refused (F229)? A live holder lets go
/// within its own [`REPLY_TIMEOUT`], so only a frozen one outlasts
/// [`ATTACH_LOCK_WAIT`]; the start is then refused and the failed-start
/// contract applies.
pub(crate) fn still_waiting_for_attach(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) < ATTACH_LOCK_WAIT
}

/// The lazily started, self-replacing control-mode client.
pub(crate) struct ControlClient {
    binary: OsString,
    server: Option<String>,
    enabled: bool,
    /// The lock attach and setup hold (F229, F238).
    attach_lock: AttachLock,
    state: Mutex<State>,
}

/// Where a client's attach lock lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachLock {
    /// Beside the server's socket (F238), so every pij process using that
    /// server takes the same lock whatever its state root. The directory is
    /// tmux's own and is never created here: without it there is no server
    /// to attach. Taken there only while that directory is private as tmux
    /// requires (S292-001); otherwise `fallback`, in the adapter's state root.
    Socket { path: PathBuf, fallback: PathBuf },
    /// A custom executable's (fakes and wrappers): its socket is not pij's to
    /// know, so the lock stays in the adapter's state root, made if absent.
    Private(PathBuf),
}

impl AttachLock {
    /// The lock for the real `tmux` on `server`, falling back to one under
    /// `state_root` beside a socket directory others could write to.
    pub(crate) fn for_socket(server: Option<&str>, state_root: &Path) -> Self {
        Self::Socket {
            path: socket_attach_lock_path(&socket_path(server)),
            fallback: attach_lock_path(state_root, server),
        }
    }

    fn take(&self) -> io::Result<File> {
        match self {
            Self::Socket { path, fallback } => {
                let dir = path.parent().unwrap_or(Path::new("/"));
                if private_socket_dir(&fs::symlink_metadata(dir)?) {
                    lock_attach(path)
                } else {
                    Self::Private(fallback.clone()).take()
                }
            }
            Self::Private(path) => {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                lock_attach(path)
            }
        }
    }
}

/// S292-001: a socket directory a lock may sit in, as tmux itself requires of
/// it: a real directory (not a link), ours or root's and sticky, and not
/// writable by group or others unless sticky. Anywhere else another user
/// could plant or swap the file, so the lock stays in the state root.
fn private_socket_dir(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let sticky = meta.mode() & 0o1000 != 0;
    meta.is_dir()
        && (meta.uid() == pij_core::program::real_uid() || (meta.uid() == 0 && sticky))
        && (meta.mode() & 0o022 == 0 || sticky)
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl ControlClient {
    pub(crate) fn new(
        binary: OsString,
        server: Option<String>,
        enabled: bool,
        attach_lock: AttachLock,
    ) -> Self {
        Self {
            binary,
            server,
            enabled,
            attach_lock,
            state: Mutex::new(State::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    #[cfg(test)]
    pub(crate) fn attach_lock(&self) -> &AttachLock {
        &self.attach_lock
    }

    /// The program the control client would launch.
    #[cfg(test)]
    pub(crate) fn program(&self) -> OsString {
        pij_core::program::launch(&self.binary, |program| {
            Ok::<_, io::Error>(self.command(program).get_program().to_os_string())
        })
        .expect("no spawn, so no error")
    }

    pub(crate) fn with_enabled(&self, enabled: bool) -> Self {
        Self {
            binary: self.binary.clone(),
            server: self.server.clone(),
            enabled,
            attach_lock: self.attach_lock.clone(),
            state: Mutex::new(State::default()),
        }
    }

    /// Answer a read over the control client, tell the caller to fork it,
    /// or skip it while another process attaches ([`Query::Busy`]).
    pub(crate) fn query(&self, args: &[OsString]) -> Query {
        self.ask(args, true)
    }

    /// Answer a read over a client that is ALREADY live, or `None`. It never
    /// starts or replaces a client and the caller never forks (plan 219: the
    /// modal-stall detector reads only what costs no process).
    pub(crate) fn query_live(&self, args: &[OsString]) -> Option<Reply> {
        match self.ask(args, false) {
            Query::Answer(reply) => Some(reply),
            Query::Fork | Query::Busy => None,
        }
    }

    /// End the live client for good (a daemon shutting down, F238): it leaves
    /// through its teardown, and no read starts another one; reads fork.
    pub(crate) fn release(&self) {
        let live = self.state.lock().ok().and_then(|mut state| {
            state.released = true;
            state.live.take()
        });
        drop(live);
    }

    fn ask(&self, args: &[OsString], may_start: bool) -> Query {
        if !self.enabled || !is_read(args) {
            return Query::Fork;
        }
        let Some(line) = command_line(args) else {
            return Query::Fork;
        };
        let (receiver, waited_on) = {
            let Ok(mut state) = self.state.lock() else {
                return Query::Fork;
            };
            let ended = state.live.as_ref().and_then(|live| {
                live.waiters
                    .lock()
                    .map_or(Some(false), |w| w.dead.then_some(w.alone))
            });
            if let Some(alone) = ended {
                state.live = None;
                if alone {
                    state.retry_after = Some(Instant::now() + RESTART_BACKOFF);
                }
            }
            if state.live.is_none() {
                if !may_start
                    || state.released
                    || state.retry_after.is_some_and(|at| Instant::now() < at)
                {
                    return Query::Fork;
                }
                match self.start() {
                    Ok(live) => {
                        state.live = Some(live);
                        state.retry_after = None;
                        state.contended_since = None;
                    }
                    // F229: another process is attaching. Never wait here
                    // (this runs on a runtime worker, holding `state`) and
                    // never fork: skip the read and try the lock again on the
                    // next one, until the bound refuses the start.
                    Err(error) if attach_lock_held(&error) => {
                        let now = Instant::now();
                        let since = *state.contended_since.get_or_insert(now);
                        if still_waiting_for_attach(since, now) {
                            return Query::Busy;
                        }
                        state.contended_since = None;
                        state.retry_after = Some(now + RESTART_BACKOFF);
                        return Query::Fork;
                    }
                    Err(_) => {
                        state.contended_since = None;
                        state.retry_after = Some(Instant::now() + RESTART_BACKOFF);
                        return Query::Fork;
                    }
                }
            }
            let Some(live) = state.live.as_mut() else {
                return Query::Fork;
            };
            match send(live, &line) {
                Some(receiver) => (receiver, Arc::clone(&live.waiters)),
                None => {
                    state.live = None;
                    return Query::Fork;
                }
            }
        };
        match receiver.recv_timeout(REPLY_TIMEOUT) {
            Ok(reply) => Query::Answer(reply),
            Err(_) => {
                // Slow or lost: this read forks, and the client is replaced so
                // later reads stop waiting on a hung one. (Replacing it is not
                // what keeps a late reply from the next caller: this waiter's
                // sender stays queued and absorbs it.)
                if let Ok(mut state) = self.state.lock() {
                    forget(&mut state, &waited_on);
                }
                Query::Fork
            }
        }
    }

    /// The tmux command for `program`, the executable `pij_core::program::launch`
    /// resolved (plan 182: PATH is searched once, not on every launch).
    fn command(&self, program: OsString) -> Command {
        let mut command = Command::new(program);
        command.arg("-u");
        if let Some(server) = &self.server {
            command.args(["-L", server]);
        }
        command
    }

    fn start(&self) -> io::Result<Live> {
        // F229: no other client may attach to this server until this one's
        // setup has answered. Released when `_attach` drops, on every return.
        // A held lock refuses at once; `ask` skips reads until it frees.
        let _attach = self.attach_lock.take()?;
        pij_core::spawns::record("tmux.control-client");
        // `-N`: no server means no client; never start one just to read it.
        // `-A`: attach to `_pij`, creating it first if it is absent.
        let mut child = pij_core::program::launch(&self.binary, |program| {
            self.command(program)
                .args(["-N", "-C", "new-session", "-A", "-s", CONTROL_SESSION])
                .args(["-f", ATTACH_FLAGS, IDLE_COMMAND])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
        })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("control client pipes missing"));
        };
        let waiters = Arc::new(Mutex::new(Waiters::default()));
        let reader_waiters = Arc::clone(&waiters);
        std::thread::Builder::new()
            .name("pij-tmux-control".to_string())
            .spawn(move || read_replies(BufReader::new(stdout), &reader_waiters))?;
        let mut live = Live {
            child: Some(child),
            stdin,
            waiters,
        };
        // Before the handshake, so no waiter is queued for these replies: the
        // reader drops every block until the sentinel's. Untargeted, so each
        // applies to this client and its session, `_pij`.
        //
        // `_pij` is destroyed with its last client only while it holds nothing
        // but the control client's own idle pane (F238: told apart by its
        // start command, not by a count, so a real pane left alone there is
        // never destroyed). A window or pane that lands there from elsewhere
        // (an untargeted command run outside tmux) switches that off, and a
        // later client switches it back on only once `_pij` again holds just
        // the idle pane.
        let subscription = format!("{SESSIONS_SUBSCRIPTION}::{ALONE}");
        let keep = "set-option destroy-unattached off";
        for setup in [
            [
                "if-shell",
                "-F",
                "#{&&:#{==:#{session_windows},1},#{&&:#{==:#{window_panes},1},#{m:*sleep 2147483647*,#{pane_start_command}}}}",
                "set-option destroy-unattached on",
            ]
            .as_slice(),
            ["set-hook", "after-new-window", keep].as_slice(),
            ["set-hook", "after-split-window", keep].as_slice(),
            ["refresh-client", "-B", &subscription].as_slice(),
        ] {
            let line = command_line(&setup.iter().map(OsString::from).collect::<Vec<_>>())
                .expect("setup quotes");
            live.stdin
                .write_all(format!("{line}\n").as_bytes())
                .and_then(|()| live.stdin.flush())?;
        }
        let sentinel = command_line(&[
            OsString::from("display-message"),
            OsString::from("-p"),
            OsString::from(READY_SENTINEL),
        ])
        .expect("sentinel quotes");
        let ready = send(&mut live, &sentinel)
            .and_then(|receiver| receiver.recv_timeout(REPLY_TIMEOUT).ok())
            .is_some_and(|reply| reply == Ok(vec![READY_SENTINEL.as_bytes().to_vec()]));
        if ready {
            Ok(live)
        } else {
            Err(io::Error::other(
                "control client did not answer its handshake",
            ))
        }
    }
}

/// The attach lock for a custom executable's `server` (`None`: its default)
/// under `state_root` ([`AttachLock::Private`]). Keyed by the `-L` label. A
/// dot file, never `*.raw`: the orphan-tap sweep cannot take it for a sink.
#[must_use]
pub fn attach_lock_path(state_root: &Path, server: Option<&str>) -> PathBuf {
    state_root.join(format!(
        ".control-attach-{}.lock",
        lock_label(server.unwrap_or("default"))
    ))
}

/// The socket tmux itself uses: for `server` (`-L`) or, with none, the one
/// `$TMUX` names (a process started inside tmux), else the default.
/// Otherwise `$TMUX_TMPDIR` (or `/tmp`), resolved, then `tmux-<uid>/<label>`,
/// as tmux builds it.
#[must_use]
pub fn socket_path(server: Option<&str>) -> PathBuf {
    if server.is_none()
        && let Some(own) = std::env::var("TMUX")
            .ok()
            .and_then(|value| value.split(',').next().map(str::to_string))
            .filter(|path| !path.is_empty())
    {
        return PathBuf::from(own);
    }
    let dir = std::env::var_os("TMUX_TMPDIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let dir = fs::canonicalize(&dir).unwrap_or(dir);
    dir.join(format!("tmux-{}", pij_core::program::real_uid()))
        .join(server.unwrap_or("default"))
}

/// The attach lock beside `socket` (F238): a dot file in tmux's socket
/// directory, one per server, shared by every pij process that uses it.
#[must_use]
pub fn socket_attach_lock_path(socket: &Path) -> PathBuf {
    let name = socket
        .file_name()
        .map_or_else(|| "default".into(), |name| name.to_string_lossy());
    socket.with_file_name(format!(".{}.pij-control-attach.lock", lock_label(&name)))
}

fn lock_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Take the exclusive attach lock at `path` now or refuse (F229): never
/// wait, since the caller holds this client's state lock on a runtime
/// worker. The kernel releases an `flock` when its holder's last descriptor
/// closes, so a holder that died never blocks anyone. A held lock is
/// [`io::ErrorKind::WouldBlock`]: the start is refused, never made without
/// the lock. Its directory must exist ([`AttachLock::take`]).
///
/// S280-001: the file is private (0600) whatever the umask, and one left
/// wider is narrowed through this descriptor before locking, so its inode,
/// the lock's identity, never changes. Another user who could open it could
/// otherwise hold the lock and keep this daemon's clients from starting.
///
/// S292-001: opened without following a symlink (and close-on-exec, as std
/// opens every file), then checked through the descriptor before anything is
/// changed: a regular file, ours, with one link. A planted link or hard link
/// is refused, never narrowed or locked.
pub(crate) fn lock_attach(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::MetadataExt;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(pij_core::program::OPEN_NO_FOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != pij_core::program::real_uid() || meta.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a private lock file (a link, or not ours)",
                path.display()
            ),
        ));
    }
    if meta.permissions().mode() & 0o777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            AttachLockHeld(path.to_path_buf()),
        )),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Another process holds the attach lock (F229). Its own type, so a spawn
/// that fails with `EAGAIN` (also [`io::ErrorKind::WouldBlock`]) is never
/// taken for contention.
#[derive(Debug)]
pub(crate) struct AttachLockHeld(PathBuf);

impl std::fmt::Display for AttachLockHeld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "another tmux control client is attaching ({} is held)",
            self.0.display()
        )
    }
}

impl std::error::Error for AttachLockHeld {}

/// Did this start fail because another process holds the attach lock?
pub(crate) fn attach_lock_held(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<AttachLockHeld>())
}

/// Drop the live client, but only if it is still the one `waited_on` belongs
/// to. Every waiter in flight wakes when a client dies; an unconditional clear
/// would let each one kill the replacement another caller just started.
fn forget(state: &mut State, waited_on: &Arc<Mutex<Waiters>>) {
    if state
        .live
        .as_ref()
        .is_some_and(|live| Arc::ptr_eq(&live.waiters, waited_on))
    {
        state.live = None;
    }
}

/// Queue a waiter and write its command, in one step so replies stay in order.
fn send(live: &mut Live, line: &str) -> Option<mpsc::Receiver<Reply>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    {
        let mut waiters = live.waiters.lock().ok()?;
        if waiters.dead {
            return None;
        }
        waiters.queue.push_back(sender);
    }
    let written = live
        .stdin
        .write_all(line.as_bytes())
        .and_then(|()| live.stdin.write_all(b"\n"))
        .and_then(|()| live.stdin.flush());
    if written.is_err() {
        if let Ok(mut waiters) = live.waiters.lock() {
            waiters.dead = true;
            waiters.queue.clear();
        }
        return None;
    }
    Some(receiver)
}

/// Route each closed block to the oldest waiter. Blocks before the handshake
/// sentinel (the attach command's own block) have no waiter and are dropped.
///
/// `_pij` alone ends the client for reads at once (its caller replaces it),
/// but reading goes on until the pipe closes, so the leaving client's
/// teardown is still answered (F238).
fn read_replies(mut stdout: impl BufRead, waiters: &Mutex<Waiters>) {
    let mut framer = Framer::default();
    let mut ready = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        match stdout.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        match framer.line(&line) {
            None => {}
            Some(Frame::Exit) => break,
            Some(Frame::Alone) => {
                if let Ok(mut waiters) = waiters.lock() {
                    waiters.dead = true;
                    waiters.alone = true;
                    waiters.queue.clear();
                }
            }
            Some(Frame::Reply(reply)) => {
                if !ready {
                    if reply != Ok(vec![READY_SENTINEL.as_bytes().to_vec()]) {
                        continue;
                    }
                    ready = true;
                }
                let waiter = waiters.lock().ok().and_then(|mut w| w.queue.pop_front());
                if let Some(waiter) = waiter {
                    let _ = waiter.send(reply);
                }
            }
        }
    }
    if let Ok(mut waiters) = waiters.lock() {
        waiters.dead = true;
        waiters.closed = true;
        waiters.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ATTACH_LOCK_WAIT, AttachLock, Frame, Framer, Live, REAP_WAIT, REPLY_TIMEOUT, Reap, State,
        TEARDOWN_WAIT, Waiters, attach_lock_held, attach_lock_path, command_line, forget, is_read,
        lock_attach, private_socket_dir, reap, reap_with, socket_attach_lock_path,
        still_waiting_for_attach,
    };
    use pij_testkit::time::scaled;
    use std::ffi::OsString;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::os::fd::AsFd;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    fn feed(framer: &mut Framer, text: &str) -> Vec<Frame> {
        text.lines()
            .filter_map(|line| framer.line(line.as_bytes()))
            .collect()
    }

    fn lines(items: &[&str]) -> Vec<Vec<u8>> {
        items.iter().map(|item| item.as_bytes().to_vec()).collect()
    }

    #[test]
    fn blocks_close_only_on_their_own_guard_and_notifications_are_skipped() {
        let mut framer = Framer::default();
        let frames = feed(
            &mut framer,
            "%session-changed $0 work\n\
             %begin 100 7 1\n\
             %end 100 6 1\n\
             %end 99 7 1\n\
             %begin 1 2 3\n\
             a\tb ✓\n\
             \n\
             %end 100 7 1\n\
             %window-renamed @0 sleep\n\
             %begin 101 8 1\n\
             %end 101 8 1\n\
             %begin 102 9 1\n\
             can't find pane: %999\n\
             %error 102 9 1\n",
        );
        assert_eq!(
            frames,
            vec![
                Frame::Reply(Ok(lines(&[
                    "%end 100 6 1",
                    "%end 99 7 1",
                    "%begin 1 2 3",
                    "a\tb ✓",
                    ""
                ]))),
                Frame::Reply(Ok(vec![])),
                Frame::Reply(Err(lines(&["can't find pane: %999"]))),
            ]
        );
    }

    #[test]
    fn exit_ends_the_client_but_only_outside_a_block() {
        let mut framer = Framer::default();
        assert_eq!(
            feed(
                &mut framer,
                "%begin 1 1 1\n%exit\n%end 1 1 1\n%exit detached\n"
            ),
            vec![Frame::Reply(Ok(lines(&["%exit"]))), Frame::Exit]
        );
        assert_eq!(feed(&mut Framer::default(), "%exit\n"), vec![Frame::Exit]);
    }

    #[test]
    fn malformed_guards_are_content_or_noise_never_a_close() {
        let mut framer = Framer::default();
        assert!(feed(&mut framer, "%begin x 1 1\n%begin 1 1\n").is_empty());
        assert_eq!(
            feed(&mut framer, "%begin 5 5 0\n%end 5 5\n%end 5 5 0\n"),
            vec![Frame::Reply(Ok(lines(&["%end 5 5"])))]
        );
    }

    #[test]
    fn only_quotable_reads_are_eligible() {
        let args = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            command_line(&args(&[
                "display-message",
                "-p",
                "-t",
                "%1",
                "#{pane_pid}\t#{pane_current_path}"
            ]))
            .as_deref(),
            Some("'display-message' '-p' '-t' '%1' '#{pane_pid}\t#{pane_current_path}'")
        );
        assert_eq!(command_line(&args(&["show-options", "it's"])), None);
        // A newline or carriage return would end the command line and smuggle in
        // a second command (possibly a write): those calls fork instead.
        assert_eq!(command_line(&args(&["capture-pane", "a\nb"])), None);
        assert_eq!(command_line(&args(&["capture-pane", "a\rb"])), None);
        assert_eq!(
            command_line(&args(&["display-message", "-p", "x\nkill-server"])),
            None
        );
        assert!(is_read(&args(&["list-panes", "-a"])));
        assert!(is_read(&args(&["capture-pane", "-p", "-t", "%1"])));
        assert!(is_read(&args(&[
            "show-options",
            "-p",
            "-v",
            "-q",
            "-t",
            "%1",
            "@x"
        ])));
        assert!(is_read(&args(&[
            "display-message",
            "-p",
            "-t",
            "%1",
            "#{pane_pipe}"
        ])));
        assert!(!is_read(&args(&["display-message", "-t", "%1", "hi"])));
        for write in [
            "set-option",
            "pipe-pane",
            "send-keys",
            "new-window",
            "load-buffer",
        ] {
            assert!(!is_read(&args(&[write])), "{write} must fork");
        }
    }

    /// Every tmux 3.6 command name and alias (`tmux list-commands`).
    const TMUX_COMMANDS: &str = "
        attach-session attach bind-key bind break-pane breakp capture-pane capturep
        choose-buffer choose-client choose-tree clear-history clearhist
        clear-prompt-history clearphist clock-mode command-prompt confirm-before
        confirm copy-mode customize-mode delete-buffer deleteb detach-client detach
        display-menu menu display-message display display-popup popup display-panes
        displayp find-window findw has-session has if-shell if join-pane joinp
        kill-pane killp kill-server kill-session kill-window killw last-pane lastp
        last-window last link-window linkw list-buffers lsb list-clients lsc
        list-commands lscm list-keys lsk list-panes lsp list-sessions ls list-windows
        lsw load-buffer loadb lock-client lockc lock-server lock lock-session locks
        move-pane movep move-window movew new-session new new-window neww next-layout
        nextl next-window next paste-buffer pasteb pipe-pane pipep previous-layout
        prevl previous-window prev refresh-client refresh rename-session rename
        rename-window renamew resize-pane resizep resize-window resizew respawn-pane
        respawnp respawn-window respawnw rotate-window rotatew run-shell run
        save-buffer saveb select-layout selectl select-pane selectp select-window
        selectw send-keys send send-prefix server-access set-buffer setb
        set-environment setenv set-hook set-option set set-window-option setw
        show-buffer showb show-environment showenv show-hooks show-messages showmsgs
        show-options show show-prompt-history showphist show-window-options showw
        source-file source split-window splitw start-server start suspend-client
        suspendc swap-pane swapp swap-window swapw switch-client switchc unbind-key
        unbind unlink-window unlinkw wait-for wait
    ";

    /// S2: `is_read` is the write barrier, so its allow-list is pinned whole.
    /// Adding a command here (say `run-shell` or `if-shell`) is a safety change
    /// and must fail this test first.
    #[test]
    fn read_allowlist_is_exactly_four_reads() {
        let allowed: Vec<&str> = TMUX_COMMANDS
            .split_whitespace()
            .filter(|name| is_read(&[OsString::from(*name), OsString::from("-p")]))
            .collect();
        assert_eq!(
            allowed,
            [
                "capture-pane",
                "display-message",
                "list-panes",
                "show-options"
            ]
        );
        // Unknown first words fork too: deny by default.
        assert!(!is_read(&[
            OsString::from("frobnicate"),
            OsString::from("-p")
        ]));
        assert!(!is_read(&[]));
    }

    #[test]
    fn only_the_pij_sessions_subscription_at_one_means_alone() {
        let mut framer = Framer::default();
        assert_eq!(
            feed(
                &mut framer,
                "%subscription-changed pij-sessions $1 - - - : 2\n\
                 %subscription-changed pij-sessions-x $1 - - - : 1\n\
                 %subscription-changed other $1 - - - : 1\n\
                 %subscription-changed pij-sessions $1 - - - : 11\n\
                 %begin 1 1 1\n\
                 %subscription-changed pij-sessions $1 - - - : 1\n\
                 %end 1 1 1\n\
                 %subscription-changed pij-sessions $1 - - - : 1\n"
            ),
            vec![
                Frame::Reply(Ok(lines(&[
                    "%subscription-changed pij-sessions $1 - - - : 1"
                ]))),
                Frame::Alone,
            ]
        );
    }

    /// A client whose reader has stopped: dropping it waits on nothing.
    fn idle_client() -> Live {
        let live = unanswered_client();
        live.waiters.lock().expect("waiters").closed = true;
        live
    }

    /// A client nothing ever answers (`cat`, its output discarded), as a
    /// frozen server or a lost reply looks to a leaving client.
    fn unanswered_client() -> Live {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("cat");
        let stdin = child.stdin.take().expect("stdin");
        Live {
            child: Some(child),
            stdin,
            waiters: Arc::new(Mutex::new(Waiters::default())),
        }
    }

    /// F238: a teardown nobody answers holds the exit for at most
    /// [`TEARDOWN_WAIT`], then the client is killed and reaped anyway.
    #[test]
    fn an_unanswered_teardown_is_bounded_and_the_client_still_goes() {
        let live = unanswered_client();
        let pid = live.child.as_ref().expect("child").id();
        let started = Instant::now();
        drop(live);
        let took = started.elapsed();
        assert!(took >= TEARDOWN_WAIT, "it waited for the answer: {took:?}");
        assert!(
            took < TEARDOWN_WAIT + scaled(Duration::from_secs(2)),
            "bounded: {took:?}"
        );
        assert!(!process_alive(pid), "the client was killed and reaped");
    }

    /// F238: a client whose pipe is already closed (its reader stopped, as
    /// when tmux exited it) is not waited on at all.
    #[test]
    fn a_closed_client_leaves_without_waiting() {
        let live = idle_client();
        let pid = live.child.as_ref().expect("child").id();
        let started = Instant::now();
        drop(live);
        assert!(started.elapsed() < TEARDOWN_WAIT, "{:?}", started.elapsed());
        assert!(!process_alive(pid));
    }

    /// P292-001: a client that stopped reading, its pipe full, cannot hold the
    /// exit. The blocked write is the leave thread's, never the exit's: the
    /// exit gives up after [`TEARDOWN_WAIT`], kills and reaps the client, and
    /// the kill fails the blocked writes.
    #[test]
    fn a_client_that_stopped_reading_cannot_hold_its_exit() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .spawn()
            .expect("sleep");
        let stdin = child.stdin.take().expect("stdin");
        let pid = child.id();
        // Fill the pipe: the filler blocks once it is full.
        let mut filler = File::from(stdin.as_fd().try_clone_to_owned().expect("dup"));
        let written = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&written);
        let filling = std::thread::spawn(move || {
            let chunk = [0_u8; 1024];
            while filler.write_all(&chunk).is_ok() {
                counted.fetch_add(chunk.len(), Ordering::SeqCst);
            }
        });
        let mut last = 0;
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let now = written.load(Ordering::SeqCst);
            if now >= 16 * 1024 && now == last {
                break;
            }
            last = now;
        }
        let live = Live {
            child: Some(child),
            stdin,
            waiters: Arc::new(Mutex::new(Waiters::default())),
        };
        let (gone, went) = mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            drop(live);
            let _ = gone.send(started.elapsed());
        });
        let took = went.recv_timeout(TEARDOWN_WAIT + REAP_WAIT + scaled(Duration::from_secs(2)));
        if took.is_err() {
            let _ = Command::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
        let took = took.expect("a full pipe held the exit");
        assert!(
            took >= TEARDOWN_WAIT,
            "it gave the teardown its time: {took:?}"
        );
        assert!(!process_alive(pid), "the client was killed and reaped");
        filling.join().expect("the kill failed the filler's write");
    }

    /// F238: a killed client still there after [`REAP_WAIT`] (one stuck in the
    /// kernel looks so; here one never killed) holds the exit no longer, and a
    /// thread reaps it once it goes, so it is never left a zombie.
    #[test]
    fn a_client_that_outlives_its_kill_is_handed_to_a_reaper() {
        let child = Command::new("/bin/sleep").arg("30").spawn().expect("sleep");
        let pid = child.id();
        let started = Instant::now();
        reap(child);
        let took = started.elapsed();
        assert!(took >= REAP_WAIT, "it waited for the reap: {took:?}");
        assert!(
            took < REAP_WAIT + scaled(Duration::from_secs(1)),
            "bounded: {took:?}"
        );
        let _ = Command::new("/bin/kill")
            .args(["-9", &pid.to_string()])
            .status();
        let deadline = Instant::now() + scaled(Duration::from_secs(2));
        while process_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !process_alive(pid),
            "reaped by the thread, not left a zombie"
        );
    }

    /// F240: when no reaper thread can start, the child is not dropped
    /// unwaited: the caller waits [`REAP_WAIT`] more itself, so one that goes
    /// in that time (here killed as the thread fails to start) is reaped, not
    /// left a zombie.
    #[test]
    fn a_client_with_no_reaper_thread_is_reaped_by_the_caller() {
        let child = Command::new("/bin/sleep").arg("30").spawn().expect("sleep");
        let pid = child.id();
        let outcome = reap_with(child, |reaper| {
            drop(reaper);
            let _ = Command::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status();
            Err(io::Error::other("no thread can start"))
        });
        assert!(matches!(outcome, Reap::Done), "reaped: {outcome:?}");
        assert!(!process_alive(pid), "reaped by the caller, not a zombie");
    }

    /// F240: a child that outlives both waits with no reaper thread holds the
    /// exit no longer than twice [`REAP_WAIT`], and is given back to be named
    /// in the log.
    #[test]
    fn a_client_with_no_reaper_thread_that_will_not_go_is_left_within_the_bound() {
        let child = Command::new("/bin/sleep").arg("30").spawn().expect("sleep");
        let pid = child.id();
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let outcome = reap_with(child, |_| Err(io::Error::other("no thread can start")));
            let _ = done.send((started.elapsed(), outcome));
        });
        let result = finished.recv_timeout(REAP_WAIT * 2 + scaled(Duration::from_secs(2)));
        if result.is_err() {
            let _ = Command::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
        let (took, outcome) = result.expect("the fallback wait held the exit");
        let Reap::Left(mut child) = outcome else {
            panic!("a child still running must be left: {outcome:?}");
        };
        assert_eq!(child.id(), pid, "the log names this pid");
        assert!(took >= REAP_WAIT * 2, "both waits were given: {took:?}");
        assert!(
            took < REAP_WAIT * 2 + scaled(Duration::from_secs(1)),
            "bounded: {took:?}"
        );
        child.kill().expect("kill");
        child.wait().expect("wait");
    }

    fn process_alive(pid: u32) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// S4: when a client dies every waiter in flight wakes at once. A waiter
    /// whose client was already replaced must leave the replacement alone,
    /// or each wake kills the next client (review S4, the cascade).
    #[test]
    fn a_stale_waiter_never_clears_the_replacement_client() {
        let old = idle_client();
        let old_waiters = Arc::clone(&old.waiters);
        drop(old);
        let replacement = idle_client();
        let current = Arc::clone(&replacement.waiters);
        let mut state = State {
            live: Some(replacement),
            retry_after: None,
            contended_since: None,
            released: false,
        };
        forget(&mut state, &old_waiters);
        assert!(
            state.live.is_some(),
            "a stale waiter cleared the new client"
        );
        forget(&mut state, &current);
        assert!(state.live.is_none(), "its own waiter clears it");
    }

    fn lock_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pij-attach-lock-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("lock dir");
        dir
    }

    /// F229: each server has its own attach lock, so an isolated fixture
    /// server never waits on the operator's, and the file is never a sink.
    #[test]
    fn attach_locks_are_per_server_dot_files() {
        let root = std::path::Path::new("/state/pane-signals");
        assert_eq!(
            attach_lock_path(root, None),
            root.join(".control-attach-default.lock")
        );
        assert_eq!(
            attach_lock_path(root, Some("pij-rs-x/../y")),
            root.join(".control-attach-pij-rs-x____y.lock")
        );
        assert_ne!(
            attach_lock_path(root, Some("a")),
            attach_lock_path(root, Some("b"))
        );
    }

    /// S280-001: the attach lock is private whatever the umask, and one left
    /// wider is narrowed in place: same inode, so the lock keeps its identity.
    #[test]
    fn the_attach_lock_is_private_and_narrowed_in_place() {
        let dir = lock_dir("mode");
        let fresh = attach_lock_path(&dir, Some("fresh"));
        drop(lock_attach(&fresh).expect("fresh lock"));
        let mode = |path: &Path| fs::metadata(path).expect("lock").permissions().mode() & 0o777;
        assert_eq!(mode(&fresh), 0o600);

        let wide = attach_lock_path(&dir, Some("wide"));
        fs::write(&wide, b"").expect("an earlier lock");
        fs::set_permissions(&wide, fs::Permissions::from_mode(0o644)).expect("left wide");
        let inode = fs::metadata(&wide).expect("lock").ino();
        drop(lock_attach(&wide).expect("wide lock"));
        assert_eq!(mode(&wide), 0o600);
        assert_eq!(fs::metadata(&wide).expect("lock").ino(), inode, "same file");
        fs::remove_dir_all(dir).expect("clean up");
    }

    /// F238: the socket-side lock sits in tmux's directory, which may be
    /// shared or custom. A symlink at its path is refused, never followed: no
    /// file is created, opened, locked or narrowed through it.
    #[test]
    fn a_symlinked_attach_lock_is_refused_not_followed() {
        let dir = lock_dir("link");
        let target = dir.join("elsewhere");
        let link = socket_attach_lock_path(&dir.join("srv"));
        std::os::unix::fs::symlink(&target, &link).expect("link");
        assert!(lock_attach(&link).is_err(), "a dangling link is refused");
        assert!(!target.exists(), "nothing was created through it");

        fs::write(&target, b"").expect("a file the link points at");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).expect("mode");
        assert!(lock_attach(&link).is_err(), "a link to a file is refused");
        let mode = fs::metadata(&target).expect("target").permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "the target was never opened and narrowed");
        fs::remove_dir_all(dir).expect("clean up");
    }

    /// S292-001: a hard link to a file of ours is refused too, and left as it
    /// was: the lock is a file with one name.
    #[test]
    fn a_hard_linked_attach_lock_is_refused() {
        let dir = lock_dir("hard");
        let target = dir.join("elsewhere");
        fs::write(&target, b"").expect("a file of ours");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).expect("mode");
        let link = socket_attach_lock_path(&dir.join("srv"));
        fs::hard_link(&target, &link).expect("hard link");
        assert!(lock_attach(&link).is_err());
        let mode = fs::metadata(&target).expect("target").permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "never narrowed");
        fs::remove_dir_all(dir).expect("clean up");
    }

    /// S292-001: the lock sits beside the socket only in a directory as
    /// private as tmux requires; one that is a link, root's without the
    /// sticky bit, or writable by others without it is not.
    #[test]
    fn only_a_private_socket_directory_holds_the_lock() {
        let private = |path: &Path| private_socket_dir(&fs::symlink_metadata(path).expect("meta"));
        let dir = lock_dir("dirs");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("mode");
        assert!(private(&dir), "ours, 0700");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).expect("mode");
        assert!(!private(&dir), "ours, but anyone may write");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o1777)).expect("mode");
        assert!(private(&dir), "ours and sticky");
        let link = std::env::temp_dir().join(format!("pij-attach-dir-link-{}", std::process::id()));
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&dir, &link).expect("link");
        assert!(!private(&link), "a link to a directory is not one");
        fs::remove_file(&link).expect("clean up link");
        assert!(!private(Path::new("/usr")), "root's, not sticky");
        fs::remove_dir_all(dir).expect("clean up");
    }

    /// S292-001: beside a socket directory others may write to, the lock is
    /// taken in the state root instead, and nothing is made in that directory.
    #[test]
    fn a_shared_socket_directory_falls_back_to_the_state_root() {
        let shared = lock_dir("shared");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).expect("mode");
        let state = lock_dir("state").join("pane-signals");
        let lock = AttachLock::Socket {
            path: socket_attach_lock_path(&shared.join("srv")),
            fallback: attach_lock_path(&state, Some("srv")),
        };
        drop(lock.take().expect("taken in the state root"));
        assert!(attach_lock_path(&state, Some("srv")).exists());
        assert!(
            !socket_attach_lock_path(&shared.join("srv")).exists(),
            "nothing made beside the shared socket"
        );
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o700)).expect("mode");
        drop(lock.take().expect("taken beside the socket"));
        assert!(socket_attach_lock_path(&shared.join("srv")).exists());
        fs::remove_dir_all(shared).expect("clean up");
        fs::remove_dir_all(state.parent().expect("state")).expect("clean up");
    }

    /// F229: a held lock refuses a start at once (the caller holds the state
    /// lock on a runtime worker, so it never waits) and is never shared. Once
    /// released it is taken at once.
    #[test]
    fn a_held_attach_lock_refuses_at_once() {
        let path = attach_lock_path(&lock_dir("held"), Some("srv"));
        let holder = lock_attach(&path).expect("first holder");
        let started = Instant::now();
        let refused = lock_attach(&path).expect_err("held: refused, never shared");
        let waited = started.elapsed();
        assert!(attach_lock_held(&refused), "{refused}");
        let spawn_eagain = io::Error::from(io::ErrorKind::WouldBlock);
        assert!(
            !attach_lock_held(&spawn_eagain),
            "an EAGAIN spawn is not contention"
        );
        assert!(
            waited < pij_testkit::time::scaled(Duration::from_millis(500)),
            "a held lock made the start wait {waited:?}"
        );
        drop(holder);
        // Taken again once released. Another test's spawn can briefly hold a
        // forked copy of the descriptor until its exec closes it (CLOEXEC),
        // so allow a moment rather than one attempt.
        let deadline = Instant::now() + pij_testkit::time::scaled(Duration::from_secs(1));
        while let Err(error) = lock_attach(&path) {
            assert!(
                Instant::now() < deadline,
                "released lock not taken: {error}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// F229: a start keeps skipping reads while another process attaches,
    /// then is refused at the bound. The bound outlasts a live holder's own
    /// handshake limit, so only a frozen holder reaches it.
    #[test]
    fn a_contended_start_waits_out_the_bound_then_is_refused() {
        let since = std::time::Instant::now();
        assert!(still_waiting_for_attach(since, since));
        assert!(still_waiting_for_attach(
            since,
            since + ATTACH_LOCK_WAIT - Duration::from_millis(1)
        ));
        assert!(!still_waiting_for_attach(since, since + ATTACH_LOCK_WAIT));
        assert!(REPLY_TIMEOUT < ATTACH_LOCK_WAIT);
    }

    /// Helper for the next test: holds the lock in its own process until killed.
    #[test]
    fn attach_lock_holder_helper_process() {
        let Ok(path) = std::env::var("PIJ_ATTACH_LOCK_HOLD") else {
            return;
        };
        let ready = std::env::var("PIJ_ATTACH_LOCK_READY").expect("ready file");
        let _held = lock_attach(std::path::Path::new(&path)).expect("hold");
        std::fs::write(ready, b"held").expect("say held");
        std::thread::sleep(Duration::from_secs(600));
    }

    /// F229: the kernel drops a dead holder's `flock`, so a crashed process
    /// never wedges the attach: the lock is taken at once, with no wait.
    #[test]
    fn a_dead_holders_attach_lock_is_taken_at_once() {
        let dir = lock_dir("dead");
        let path = attach_lock_path(&dir, Some("srv"));
        let ready = dir.join("holder-ready");
        std::fs::create_dir_all(&dir).expect("lock dir");
        let mut holder = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "control::tests::attach_lock_holder_helper_process",
                "--nocapture",
            ])
            .env("PIJ_ATTACH_LOCK_HOLD", &path)
            .env("PIJ_ATTACH_LOCK_READY", &ready)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn lock holder");
        let deadline = Instant::now() + pij_testkit::time::scaled(Duration::from_secs(15));
        while !ready.exists() {
            assert!(Instant::now() < deadline, "holder never took the lock");
            std::thread::sleep(Duration::from_millis(10));
        }
        let refused =
            lock_attach(&path).expect_err("a live holder in another process excludes this one");
        assert!(attach_lock_held(&refused), "{refused}");

        holder.kill().expect("kill holder");
        holder.wait().expect("reap holder");
        lock_attach(&path).expect("dead holder: taken at once");
        let _ = std::fs::remove_dir_all(dir);
    }
}
