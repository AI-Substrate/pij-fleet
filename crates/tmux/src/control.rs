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
/// Attach without resizing windows, without `%output` traffic, without input.
const ATTACH_FLAGS: &str = "no-output,ignore-size,read-only";

/// One command's answer: its output lines, or tmux's error lines.
pub(crate) type Reply = std::result::Result<Vec<Vec<u8>>, Vec<Vec<u8>>>;

/// What one protocol line meant.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// A command's block closed.
    Reply(Reply),
    /// The client is detached or the server is gone.
    Exit,
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
        (line == b"%exit" || line.starts_with(b"%exit ")).then_some(Frame::Exit)
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
    session: Option<String>,
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
    pub(crate) fn new(
        binary: OsString,
        server: Option<String>,
        session: Option<String>,
        enabled: bool,
    ) -> Self {
        Self {
            binary,
            server,
            session,
            enabled,
            state: Mutex::new(State::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn with_enabled(&self, enabled: bool) -> Self {
        Self::new(
            self.binary.clone(),
            self.server.clone(),
            self.session.clone(),
            enabled,
        )
    }

    /// Answer a read over the control client, or `None` to make the caller fork.
    pub(crate) fn query(&self, args: &[OsString]) -> Option<Reply> {
        if !self.enabled || !is_read(args) {
            return None;
        }
        let line = command_line(args)?;
        let receiver = {
            let mut state = self.state.lock().ok()?;
            if state
                .live
                .as_ref()
                .is_some_and(|live| live.waiters.lock().map_or(true, |w| w.dead))
            {
                state.live = None;
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
                Some(receiver) => receiver,
                None => {
                    state.live = None;
                    return None;
                }
            }
        };
        match receiver.recv_timeout(REPLY_TIMEOUT) {
            Ok(reply) => Some(reply),
            Err(_) => {
                // Slow or lost: replace the client; this read forks.
                if let Ok(mut state) = self.state.lock() {
                    state.live = None;
                }
                None
            }
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command.arg("-u");
        if let Some(server) = &self.server {
            command.args(["-L", server]);
        }
        command
    }

    fn start(&self) -> io::Result<Live> {
        let target = match &self.session {
            Some(session) => session.clone(),
            None => self.first_session()?,
        };
        pij_core::spawns::record("tmux.control-client");
        let mut child = self
            .command()
            .args(["-C", "attach-session", "-f", ATTACH_FLAGS, "-t", &target])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
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

    fn first_session(&self) -> io::Result<String> {
        pij_core::spawns::record("tmux.list-sessions");
        let output = self
            .command()
            .args(["list-sessions", "-F", "#{session_id}"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .filter(|_| output.status.success())
            .map(str::to_string)
            .ok_or_else(|| io::Error::other("no tmux session to attach a control client to"))
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
        waiters.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, Framer, command_line, is_read};
    use std::ffi::OsString;

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
}
