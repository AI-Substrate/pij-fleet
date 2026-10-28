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

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
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
}

struct Live {
    child: Child,
    stdin: ChildStdin,
    waiters: Arc<Mutex<Waiters>>,
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct State {
    live: Option<Live>,
    retry_after: Option<Instant>,
}

/// The lazily started, self-replacing control-mode client.
pub(crate) struct ControlClient {
    binary: OsString,
    server: Option<String>,
    enabled: bool,
    state: Mutex<State>,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl ControlClient {
    pub(crate) fn new(binary: OsString, server: Option<String>, enabled: bool) -> Self {
        Self {
            binary,
            server,
            enabled,
            state: Mutex::new(State::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
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
        Self::new(self.binary.clone(), self.server.clone(), enabled)
    }

    /// Answer a read over the control client, or `None` to make the caller fork.
    pub(crate) fn query(&self, args: &[OsString]) -> Option<Reply> {
        if !self.enabled || !is_read(args) {
            return None;
        }
        let line = command_line(args)?;
        let (receiver, waited_on) = {
            let mut state = self.state.lock().ok()?;
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
                if state.retry_after.is_some_and(|at| Instant::now() < at) {
                    return None;
                }
                match self.start() {
                    Ok(live) => {
                        state.live = Some(live);
                        state.retry_after = None;
                    }
                    Err(_) => {
                        state.retry_after = Some(Instant::now() + RESTART_BACKOFF);
                        return None;
                    }
                }
            }
            let live = state.live.as_mut()?;
            match send(live, &line) {
                Some(receiver) => (receiver, Arc::clone(&live.waiters)),
                None => {
                    state.live = None;
                    return None;
                }
            }
        };
        match receiver.recv_timeout(REPLY_TIMEOUT) {
            Ok(reply) => Some(reply),
            Err(_) => {
                // Slow or lost: this read forks, and the client is replaced so
                // later reads stop waiting on a hung one. (Replacing it is not
                // what keeps a late reply from the next caller: this waiter's
                // sender stays queued and absorbs it.)
                if let Ok(mut state) = self.state.lock() {
                    forget(&mut state, &waited_on);
                }
                None
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
            child,
            stdin,
            waiters,
        };
        // Before the handshake, so no waiter is queued for these replies: the
        // reader drops every block until the sentinel's. Untargeted, so each
        // applies to this client and its session, `_pij`.
        //
        // `_pij` is destroyed with its last client only while it holds nothing
        // but the control pane. A window or pane that lands there from
        // elsewhere (an untargeted command run outside tmux) switches that off
        // for good, and a later client does not switch it back on.
        let subscription = format!("{SESSIONS_SUBSCRIPTION}::{ALONE}");
        let keep = "set-option destroy-unattached off";
        for setup in [
            [
                "if-shell",
                "-F",
                "#{&&:#{==:#{session_windows},1},#{==:#{window_panes},1}}",
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
fn read_replies(mut stdout: impl BufRead, waiters: &Mutex<Waiters>) {
    let mut framer = Framer::default();
    let mut ready = false;
    let mut alone = false;
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
                alone = true;
                break;
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
        waiters.alone = alone;
        waiters.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, Framer, Live, State, Waiters, command_line, forget, is_read};
    use std::ffi::OsString;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

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

    fn idle_client() -> Live {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("cat");
        let stdin = child.stdin.take().expect("stdin");
        Live {
            child,
            stdin,
            waiters: Arc::new(Mutex::new(Waiters::default())),
        }
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
        };
        forget(&mut state, &old_waiters);
        assert!(
            state.live.is_some(),
            "a stale waiter cleared the new client"
        );
        forget(&mut state, &current);
        assert!(state.live.is_none(), "its own waiter clears it");
    }
}
