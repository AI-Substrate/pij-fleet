use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
use pij_core::framing::frame_message;
use pij_core::model::{
    DeliveryOrigin, DeliveryOutcome, Harness, Msg, SeatDescriptor, parse_process_start,
};
use pij_core::ports::Transport;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::{Instant, timeout, timeout_at};

const NAME: &str = "claude-uds";
const ACK_WAIT: Duration = Duration::from_millis(150);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to keep listening for a TERMINAL answer once a hold has been seen.
///
/// Only a held delivery ever waits this long, and a held delivery is already
/// waiting on a person. Denials were measured 1.7-2.2 s after their hold; a human
/// slower than this still yields `Held`, which is honest at that instant.
const HOLD_GRACE: Duration = Duration::from_secs(5);
/// Prefix for the per-delivery reply socket. The name is arbitrary — measured
/// 2026-08-31: the CLI routes a hold receipt to ANY name inside its own socket
/// namespace, not only `<pid>.sock` — which is what lets every delivery own a
/// private reply address instead of the whole transport serialising on one.
const REPLY_PREFIX: &str = "pij-uds";
static REPLY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Claude Code's Unix-domain-socket message transport.
///
/// Capability discovery reads Claude's session records on demand. Reading a
/// record is cheap and side-effect-free; connecting merely to discover whether a
/// socket exists remains forbidden.
///
/// Socket reachability is necessary but not sufficient. The descriptor must
/// carry `cross_session_inbound_accept: Some(true)`, stamped by the spawn path
/// only when it actually emitted Claude's explicit inbound-accept setting.
/// `None` and `Some(false)` remain closed before discovery or socket IO.
/// Human-typing politeness is a separate caller gate shared by direct delivery
/// and queued retries.
///
/// The `peer_message_status` drop/hold decoder is retained but UNPROVEN on this
/// daemon-origin connection shape. The done report records the current Claude
/// build re-verification; silence never upgrades the receipt beyond
/// `InjectedToTransport`.
///
/// # Composition recipe
///
/// In `crates/daemon/src/lib.rs`, import `pij_transport::UdsTransport`; construct
/// `AdapterChoice::Real` with `Arc::new(UdsTransport::new()?)`; pass that shared
/// `Arc<dyn Transport>` to both `DeliveryService` and `DrainWorker`. Construct one
/// shared `InteractionGate` before `DeliveryService`, pass it to both consumers,
/// and feed it from `PaneObserver`. No task starts in this adapter; shutdown is
/// unchanged.
#[derive(Clone, Debug)]
pub struct UdsTransport {
    sessions_dir: PathBuf,
    ack_wait: Duration,
    hold_grace: Duration,
    connect_timeout: Duration,
    observe_start: fn(u32, bool) -> Option<u64>,
}

impl UdsTransport {
    /// Use the current user's Claude session registry.
    ///
    /// # Errors
    /// [`PijError::Adapter`] when the home directory is unavailable.
    pub fn new() -> Result<Self> {
        let home = env::var_os("HOME").ok_or_else(|| PijError::Adapter {
            adapter: NAME.to_string(),
            message: "HOME is unset — set HOME to the account that owns ~/.claude/sessions"
                .to_string(),
        })?;
        Ok(Self::from_sessions_dir(
            PathBuf::from(home).join(".claude/sessions"),
        ))
    }

    /// Use an explicit Claude session registry directory.
    ///
    /// The daemon uses [`Self::new`]; composition tests use this constructor to
    /// exercise the real adapter without reading or mutating the user's registry.
    #[must_use]
    pub fn from_sessions_dir(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            ack_wait: ACK_WAIT,
            hold_grace: HOLD_GRACE,
            connect_timeout: CONNECT_TIMEOUT,
            observe_start: process_start,
        }
    }

    /// Wait longer than [`ACK_WAIT`] for a status line.
    ///
    /// The default 150 ms is tuned for the machine-side verdict — `held` lands in
    /// ~70 ms. A DENIAL is a different kind of fact: it arrives only when a human
    /// clicks, seconds later. Observing one end to end therefore needs a window
    /// sized for a person, and it must run through this same transport rather
    /// than a bespoke client, or it proves the wire and not the code.
    #[must_use]
    pub fn with_ack_wait(mut self, ack_wait: Duration) -> Self {
        self.ack_wait = ack_wait;
        self
    }

    /// Shorten the post-hold window. Tests use this so a case with no terminal
    /// answer does not spend [`HOLD_GRACE`] proving it.
    #[must_use]
    pub fn with_hold_grace(mut self, hold_grace: Duration) -> Self {
        self.hold_grace = hold_grace;
        self
    }

    fn discover(&self, seat: &SeatDescriptor) -> Option<Endpoint> {
        if let Some(proc) = seat.proc {
            let direct = self.sessions_dir.join(format!("{}.json", proc.pid));
            if let Some(record) = read_json::<SessionRecord>(&direct)
                && record.pid == proc.pid
                && (self.observe_start)(proc.pid, false) == Some(proc.proc_start)
                && let Some(endpoint) = self.live_endpoint(record)
            {
                return Some(endpoint);
            }
        }
        let suffix = format!(".{}", seat.pane.as_deref()?);
        self.discover_pane(&suffix, fs::read_dir(&self.sessions_dir).ok()?)
    }

    fn discover_pane(
        &self,
        suffix: &str,
        entries: impl Iterator<Item = std::io::Result<fs::DirEntry>>,
    ) -> Option<Endpoint> {
        let mut candidate = None;
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "json") {
                continue;
            }
            let Some(record) = read_json::<SessionRecord>(&path) else {
                continue;
            };
            if !record
                .tmux
                .as_deref()
                .is_some_and(|tmux| tmux.ends_with(suffix))
            {
                continue;
            }
            let Some(endpoint) = self.live_endpoint(record) else {
                continue;
            };
            // Multiple live owners are ambiguous, never decided by directory or
            // lexical order. A verified exact identity above already wins.
            if candidate.is_some() {
                return None;
            }
            candidate = Some(endpoint);
        }
        candidate
    }

    fn live_endpoint(&self, record: SessionRecord) -> Option<Endpoint> {
        // Claude records UTC. Observe this process in UTC too; a current UTC
        // offset is NOT necessarily the offset when a long-lived process began.
        let recorded = parse_process_start(&record.proc_start).ok()?;
        if (self.observe_start)(record.pid, true)? != recorded {
            return None;
        }
        self.endpoint_from_record(record)
    }

    fn endpoint_from_record(&self, record: SessionRecord) -> Option<Endpoint> {
        let token = matching_peer_token(&self.sessions_dir, &record)?;
        let socket_path = record
            .messaging_socket_path
            .filter(|path| !path.is_empty())?;
        Some(Endpoint {
            socket_path: PathBuf::from(socket_path),
            token,
        })
    }
}

fn process_start(pid: u32, utc: bool) -> Option<u64> {
    let mut command = Command::new("ps");
    command
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C");
    if utc {
        command.env("TZ", "UTC");
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_process_start(std::str::from_utf8(&output.stdout).ok()?).ok()
}

#[async_trait]
impl Transport for UdsTransport {
    fn name(&self) -> &str {
        NAME
    }

    async fn can_deliver(&self, seat: &SeatDescriptor, msg: &Msg) -> Result<bool> {
        if seat.harness != Harness::Claude
            || msg.command.is_some()
            || seat.cross_session_inbound_accept != Some(true)
        {
            return Ok(false);
        }
        Ok(self.discover(seat).is_some())
    }

    async fn deliver(&self, seat: &SeatDescriptor, msg: &Msg) -> Result<DeliveryOutcome> {
        if seat.harness != Harness::Claude
            || msg.command.is_some()
            || seat.cross_session_inbound_accept != Some(true)
        {
            return Ok(DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            });
        }
        let Some(endpoint) = self.discover(seat) else {
            return Ok(DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            });
        };
        // The receiver delivers `peer_message_status` by connecting BACK to the
        // address we advertise in `from=`. The previous literal `uds:pij-daemon`
        // is not an address, so the CLI logged "hold-receipt skipped: reply
        // address unshaped or outside our socket namespace" and sent nothing —
        // every held message then timed out into Delivered. Bind a real listener
        // and name it.
        let Some(inbox) = ReplyInbox::bind(&endpoint.socket_path) else {
            // No reply address means no way to observe Held. Refusing to send is
            // the honest outcome: delivering blind would resurrect the very
            // defect this unit exists to remove.
            return Ok(DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            });
        };
        let frame = build_peer_frame(msg, &inbox.origin())?;
        send_frame(
            &endpoint,
            &frame,
            msg,
            self.connect_timeout,
            self.ack_wait,
            self.hold_grace,
            &inbox,
        )
        .await
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    pid: u32,
    proc_start: String,
    #[serde(default)]
    pid_domain: Option<String>,
    tmux: Option<String>,
    messaging_socket_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerKey {
    peer_token: String,
    proc_start: String,
    #[serde(default)]
    pid_domain: Option<String>,
}

struct Endpoint {
    socket_path: PathBuf,
    token: String,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn matching_peer_token(sessions_dir: &Path, record: &SessionRecord) -> Option<String> {
    let prefix = format!("{}.", record.pid);
    let mut keys = fs::read_dir(sessions_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| valid_key_name(path, &prefix))
        .collect::<Vec<_>>();
    keys.sort();

    let mut matches = keys.into_iter().filter_map(|path| {
        let key = read_json::<PeerKey>(&path)?;
        (key.proc_start == record.proc_start
            && key.pid_domain == record.pid_domain
            && !key.peer_token.is_empty())
        .then_some(key.peer_token)
    });
    let token = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(token)
}

fn valid_key_name(path: &Path, prefix: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(hash) = name
        .strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".key"))
    else {
        return false;
    };
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Serialize)]
struct AuthFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    token: &'a str,
}

#[derive(Serialize)]
struct PeerFrame<'a> {
    #[serde(rename = "msgV")]
    msg_v: u8,
    msg_id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    message: UserMessage<'a>,
    priority: &'static str,
    from: &'a str,
}

#[derive(Serialize)]
struct UserMessage<'a> {
    role: &'static str,
    content: &'a str,
}

fn build_peer_frame(msg: &Msg, origin: &str) -> Result<String> {
    let sender = xml_attribute(msg.from.as_str());
    let from = xml_attribute(origin);
    let framed = frame_message(&msg.from, msg.from_machine.as_deref(), &msg.body);
    let content = format!(
        "<cross-session-message from=\"{from}\" from-name=\"{sender}\">\n{framed}\n</cross-session-message>"
    );
    serde_json::to_string(&PeerFrame {
        msg_v: 1,
        msg_id: &msg.msg_id,
        kind: "user",
        message: UserMessage {
            role: "user",
            content: &content,
        },
        priority: "next",
        from: origin,
    })
    .map_err(|error| PijError::Adapter {
        adapter: NAME.to_string(),
        message: format!("could not encode message {}: {error}", msg.msg_id),
    })
}

fn xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A reply address the receiving CLI can actually reach.
///
/// One per delivery, named uniquely inside the recipient's own socket
/// namespace. That is what makes positional correlation SOUND rather than a
/// guess: the status frames carry no correlation id of any kind — no
/// `orig_msg_id`, no `dropped_msg_ids`, and a denial cannot even be paired with
/// its own hold — so the only way to know which message a status describes is
/// for the channel to carry exactly one. A private address per delivery gives
/// that without serialising the transport across seats.
struct ReplyInbox {
    path: PathBuf,
    listener: UnixListener,
    /// A hold seen on this address, REMEMBERED ACROSS CANCELLATION.
    ///
    /// `select!` cancels the losing arm, so while this was a local of
    /// `wait_for_status` a delivery-connection hangup after a hold restarted the
    /// wait with the hold forgotten and an expired deadline — reporting Queued,
    /// which retries at zero delay and raises a second dialog at a human who is
    /// still looking at the first. Not observed on 2.1.250/2.1.251 (the CLI
    /// closes in the safe order) and unguarded until the reviewer named it.
    seen_hold: Mutex<bool>,
}

impl ReplyInbox {
    /// Bind beside the recipient's own socket. `None` when the namespace is
    /// unusable — the caller must then refuse to send rather than deliver blind.
    fn bind(recipient_socket: &Path) -> Option<Self> {
        let dir = recipient_socket.parent()?;
        let unique = REPLY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{REPLY_PREFIX}-{}-{unique}.sock",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path).ok()?;
        Some(Self {
            path,
            listener,
            seen_hold: Mutex::new(false),
        })
    }

    fn origin(&self) -> String {
        format!("uds:{}", self.path.display())
    }

    /// Read status lines until the deadline. One message is in flight on this
    /// address, so the first status line that classifies IS this message's.
    async fn wait_for_status(
        &self,
        msg_id: &str,
        deadline: Instant,
        hold_grace: Duration,
    ) -> Option<Status> {
        let mut pending = if *self.seen_hold.lock().expect("reply inbox mutex") {
            Some(Status::Held)
        } else {
            None
        };
        loop {
            // A HOLD EARNS ITS OWN, LONGER WINDOW. The base deadline is sized for
            // the machine-side verdict (holds measured at 70-74 ms against a
            // 150 ms budget), and every delivery pays it, so it cannot simply be
            // raised. But once a hold is seen the message is already waiting on a
            // PERSON, latency stops mattering, and the terminal answer is worth
            // waiting for — measured at 1.7-2.2 s after the hold. (Reviewer F5.)
            let until = if pending.is_some() {
                deadline.max(Instant::now() + hold_grace)
            } else {
                deadline
            };
            let Ok(Ok((stream, _))) = timeout_at(until, self.listener.accept()).await else {
                return pending;
            };
            if let Some(status) = self
                .read_one_connection(stream, msg_id, deadline, hold_grace, &mut pending)
                .await
            {
                return Some(status);
            }
        }
    }

    /// EACH STATUS ARRIVES ON ITS OWN CONNECTION. Measured: the CLI sent `held`,
    /// closed, and delivered `denied` on a SECOND connection 1.7 s later. An inbox
    /// that accepted once saw the hold, read EOF, and reported Held for a message
    /// the human had refused — the identical defect as returning at the first
    /// status, moved one layer down, and invisible to a fake server that answers
    /// on a single connection.
    async fn read_one_connection(
        &self,
        stream: UnixStream,
        msg_id: &str,
        deadline: Instant,
        hold_grace: Duration,
        pending: &mut Option<Status>,
    ) -> Option<Status> {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // HELD IS NOT TERMINAL, and returning on it loses the answer. Measured on
        // the live wire: `held` at 58 ms, then `denied` at 1.74 s, BOTH on this
        // address. A transport that returned at 58 ms would report a message the
        // human went on to REFUSE as merely pending, and the refusal — the only
        // thing the human actually said — would be dropped. So remember a hold and
        // keep listening for a terminal answer until the deadline.
        //
        // A human slower than the deadline still yields Held, which is honest: at
        // that instant it IS held. Backoff-and-park owns what happens next.
        loop {
            line.clear();
            let until = if pending.is_some() {
                deadline.max(Instant::now() + hold_grace)
            } else {
                deadline
            };
            match timeout_at(until, reader.read_line(&mut line)).await {
                Err(_) | Ok(Ok(0)) | Ok(Err(_)) => return None,
                Ok(Ok(_)) => match classify_status(&line, msg_id) {
                    Status::Unrelated | Status::Malformed => {}
                    Status::Held => {
                        *pending = Some(Status::Held);
                        *self.seen_hold.lock().expect("reply inbox mutex") = true;
                    }
                    terminal => return Some(terminal),
                },
            }
        }
    }
}

impl Drop for ReplyInbox {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

async fn send_frame(
    endpoint: &Endpoint,
    frame: &str,
    msg: &Msg,
    connect_timeout: Duration,
    ack_wait: Duration,
    hold_grace: Duration,
    inbox: &ReplyInbox,
) -> Result<DeliveryOutcome> {
    let Ok(Ok(mut stream)) =
        timeout(connect_timeout, UnixStream::connect(&endpoint.socket_path)).await
    else {
        return Ok(DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        });
    };

    let auth = serde_json::to_string(&AuthFrame {
        kind: "auth",
        token: &endpoint.token,
    })
    .map_err(|error| PijError::Adapter {
        adapter: NAME.to_string(),
        message: format!("could not encode peer authentication: {error}"),
    })?;
    let payload = format!("{auth}\n{frame}\n");
    if !matches!(
        timeout(connect_timeout, stream.write_all(payload.as_bytes())).await,
        Ok(Ok(()))
    ) {
        return Ok(DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        });
    }

    let deadline = Instant::now() + ack_wait;
    // Measured: every status line arrived on the REPLY address, none on this
    // connection. Both are read anyway — the connection still reports a hangup,
    // which must never be mistaken for delivery.
    let status = tokio::select! {
        status = inbox.wait_for_status(&msg.msg_id, deadline, hold_grace) => status,
        outcome = same_connection_outcome(stream, msg, deadline) => match outcome {
            SameConnection::Status(status) => Some(status),
            // A HANGUP MUST NOT PREEMPT A RECEIPT STILL IN FLIGHT. The recipient
            // closing the delivery connection says nothing about the hold receipt
            // it is about to open a SEPARATE connection to send (~70 ms). Deciding
            // Queued here would drop that receipt and re-deliver a held message —
            // raising a second dialog at the human. Wait for the reply address;
            // only its silence means Queued. (Reviewer F4.)
            SameConnection::Hangup => inbox
                .wait_for_status(&msg.msg_id, deadline, hold_grace)
                .await
                .or(Some(Status::Dropped)),
            // NOTE: `wait_for_status` re-seeds itself from `seen_hold`, so a
            // hangup after a hold resumes as Held rather than restarting empty.
        },
    };

    Ok(match status {
        Some(Status::Dropped | Status::Malformed) => DeliveryOutcome::Queued {
            reason: None,
            next_retry_at: None,
            draft_sha: None,
        },
        Some(Status::Held) => DeliveryOutcome::Held {
            reason: "recipient approval pending".to_string(),
        },
        Some(Status::Denied) => DeliveryOutcome::Refused {
            reason: "the recipient declined the message".to_string(),
        },
        // Silence is never upgraded. The frame reached the socket; nothing in
        // this path can observe a turn, and nothing here may claim one.
        Some(Status::Unrelated) | None => DeliveryOutcome::Delivered {
            origin: DeliveryOrigin::InjectedToTransport,
        },
    })
}

/// What the DELIVERY connection had to say. There is deliberately no "quiet"
/// variant: a connection with nothing to say must not get a vote, because both
/// this and the reply inbox run to the same deadline and a real Held would be
/// decided by a coin flip.
enum SameConnection {
    Status(Status),
    Hangup,
}

async fn same_connection_outcome(
    stream: UnixStream,
    msg: &Msg,
    deadline: Instant,
) -> SameConnection {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match timeout_at(deadline, reader.read_line(&mut line)).await {
            // The delivery connection has nothing to say. It must NOT get a vote:
            // both arms run to the same deadline, so returning here would race the
            // reply inbox and a real Held would be lost to a coin flip. Yield
            // forever instead and let the inbox decide.
            Err(_) => std::future::pending().await,
            Ok(Ok(0)) | Ok(Err(_)) => return SameConnection::Hangup,
            Ok(Ok(_)) => match classify_status(&line, &msg.msg_id) {
                Status::Unrelated | Status::Malformed => {}
                decided => return SameConnection::Status(decided),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Dropped,
    Held,
    /// The recipient actively refused. Terminal in the CLI's own words: "it was
    /// not delivered to their Claude session."
    Denied,
    Unrelated,
    Malformed,
}

fn classify_status(line: &str, msg_id: &str) -> Status {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Status::Malformed;
    };
    let kind = value.get("type").and_then(Value::as_str);
    let action = value.get("action").and_then(Value::as_str);
    // MEASURED wire shape, five arms on Claude 2.1.250/2.1.251, 2026-08-31:
    // `{"type":"control","action":"peer_message_status","status":"held"|"denied",...}`.
    // The bare `type=="peer_message_status"` documented in fork issue #311 was
    // never emitted once. Matching only that form is the SECOND independent
    // reason Held was unreachable — fixing the reply address alone would not
    // have surfaced it. The bare form is still accepted: it costs nothing and a
    // future build may adopt it.
    let is_status = kind == Some("peer_message_status")
        || (kind == Some("control") && action == Some("peer_message_status"));
    if !is_status {
        return Status::Unrelated;
    }

    if value
        .get("dropped_msg_ids")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(msg_id)))
    {
        return Status::Dropped;
    }

    // POSITIONAL, and it has to be: the measured frames carry their OWN fresh
    // uuid and no correlation id back to the message they describe. The reply
    // address is private to one delivery, which is what makes this sound.
    match value.get("status").and_then(Value::as_str) {
        Some("held") => return Status::Held,
        Some("denied" | "expired") => return Status::Denied,
        _ => {}
    }

    // The id-correlated form, kept for the shape the fork documented.
    let original_matches = value.get("orig_msg_id").and_then(Value::as_str) == Some(msg_id);
    let held = match value.get("wereHeld") {
        Some(Value::Bool(held)) => *held && original_matches,
        Some(Value::Array(ids)) => ids.iter().any(|id| id.as_str() == Some(msg_id)),
        _ => false,
    };
    if held {
        Status::Held
    } else {
        Status::Unrelated
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use pij_core::model::{
        DeliveryOrigin, DeliveryOutcome, Harness, Msg, ProcIdentity, SeatDescriptor, SeatId,
    };
    use pij_core::ports::Transport;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{UnixListener, UnixStream};

    use serde_json::Value;

    use super::{
        REPLY_PREFIX, ReplyInbox, Status, UdsTransport, build_peer_frame, classify_status,
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
    const FIXTURE_ORIGIN: &str = "uds:/tmp/cc-socks/pij-uds-fixture.sock";
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("pij-uds-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(name: &str) -> String {
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../testkit/fixtures/uds")
                .join(name),
        )
        .expect("read UDS fixture")
    }

    fn message(command: Option<&str>) -> Msg {
        Msg {
            from: SeatId::from("pij-sender"),
            to: SeatId::from("pij-recipient"),
            body: "first line\nsecond line".to_string(),
            msg_id: "msg-uds-001".to_string(),
            from_machine: None,
            in_reply_to: None,
            command: command.map(str::to_string),
        }
    }

    fn seat(pid: u32, pane: Option<&str>, harness: Harness) -> SeatDescriptor {
        let mut seat = SeatDescriptor::new("pij-recipient", harness, "/sanitized/worktree");
        seat.proc = Some(ProcIdentity { pid, proc_start: 1 });
        seat.pane = pane.map(str::to_string);
        seat.cross_session_inbound_accept = Some(true);
        seat
    }

    fn write_record(dir: &Path, pid: u32, pane: &str, socket: &Path) {
        fs::write(
            dir.join(format!("{pid}.json")),
            format!(
                "{{\"pid\":{pid},\"procStart\":\"Sat Aug 29 23:56:05 2026\",\"pidDomain\":\"darwin\",\"tmux\":\"sanitized:@1.{pane}\",\"messagingSocketPath\":{}}}\n",
                serde_json::to_string(&socket.to_string_lossy()).expect("encode socket path")
            ),
        )
        .expect("write session record");
        write_key(dir, pid, "Sat Aug 29 23:56:05 2026", "darwin");
    }

    fn write_key(dir: &Path, pid: u32, proc_start: &str, pid_domain: &str) {
        fs::write(
            dir.join(format!("{pid}.{HASH}.key")),
            format!(
                "{{\"peerToken\":\"sanitized-peer-token\",\"procStart\":{},\"pidDomain\":{}}}\n",
                serde_json::to_string(proc_start).expect("encode proc start"),
                serde_json::to_string(pid_domain).expect("encode pid domain")
            ),
        )
        .expect("write peer key");
    }

    fn transport(dir: &Path) -> UdsTransport {
        let mut transport = UdsTransport::from_sessions_dir(dir.to_path_buf());
        transport.ack_wait = Duration::from_millis(30);
        transport.hold_grace = Duration::from_millis(120);
        transport.connect_timeout = Duration::from_millis(200);
        transport.observe_start =
            |pid, utc| (pid == 4242).then_some(if utc { 20260829235605 } else { 1 });
        transport
    }

    #[test]
    fn pane_discovery_skips_every_rejected_entry_before_a_live_candidate() {
        for case in [
            "entry error",
            "non-json path",
            "malformed json",
            "unreadable json",
            "unrelated pane",
            "missing pane",
            "invalid process start",
            "dead process",
            "mismatched process start",
            "missing socket",
            "empty socket",
        ] {
            let dir = TestDir::new();
            let live_socket = dir.0.join("live.sock");
            write_record(&dir.0, 4242, "%99", &live_socket);
            let mut rejected: Value = serde_json::from_slice(
                &fs::read(dir.0.join("4242.json")).expect("read live record"),
            )
            .expect("decode live record");
            match case {
                "unrelated pane" => rejected["tmux"] = "fixture:@1.%100".into(),
                "missing pane" => rejected["tmux"] = Value::Null,
                "invalid process start" => rejected["procStart"] = "not a date".into(),
                "dead process" => rejected["pid"] = 4343.into(),
                "mismatched process start" => {
                    rejected["procStart"] = "Sun Aug 30 00:00:00 2026".into();
                }
                "missing socket" => rejected["messagingSocketPath"] = Value::Null,
                "empty socket" => rejected["messagingSocketPath"] = "".into(),
                _ => {}
            }
            let rejected_name = if case == "non-json path" {
                "rejected.txt"
            } else {
                "rejected.json"
            };
            let rejected_path = dir.0.join(rejected_name);
            if case == "unreadable json" {
                // Reading a directory fails even when tests run as root.
                fs::create_dir(&rejected_path).expect("create unreadable record");
            } else {
                let contents = if case == "malformed json" {
                    "not json".to_string()
                } else {
                    rejected.to_string()
                };
                fs::write(&rejected_path, contents).expect("write rejected record");
            }
            let mut entries = fs::read_dir(&dir.0)
                .expect("read fixture directory")
                .map(|entry| entry.expect("read fixture entry"))
                .collect::<Vec<_>>();
            let live_index = entries
                .iter()
                .position(|entry| entry.file_name() == "4242.json")
                .expect("find live record");
            let live = entries.swap_remove(live_index);
            let rejected_index = entries
                .iter()
                .position(|entry| entry.file_name() == rejected_name)
                .expect("find rejected record");
            let rejected = entries.swap_remove(rejected_index);
            let first = if case == "entry error" {
                Err(std::io::Error::other("injected directory-entry failure"))
            } else {
                Ok(rejected)
            };
            let endpoint = transport(&dir.0)
                .discover_pane(".%99", [first, Ok(live)].into_iter())
                .unwrap_or_else(|| panic!("{case} must not hide the later live candidate"));
            assert_eq!(endpoint.socket_path, live_socket, "{case}");
            assert_eq!(endpoint.token, "sanitized-peer-token", "{case}");
        }
    }

    #[test]
    fn discovery_prefers_live_identity_over_first_dead_pane_record() {
        let dir = TestDir::new();
        let dead = u32::MAX;
        let live = std::process::id();
        let live_socket = dir.0.join("live.sock");
        write_record(&dir.0, dead, "%99", &dir.0.join("dead.sock"));
        fs::rename(dir.0.join(format!("{dead}.json")), dir.0.join("0.json"))
            .expect("put dead record first lexically");
        let output = std::process::Command::new("ps")
            .args(["-p", &live.to_string(), "-o", "lstart="])
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .output()
            .expect("observe fixture process start");
        assert!(output.status.success());
        let start = String::from_utf8(output.stdout).expect("ASCII start");
        let start = start.trim();
        fs::write(
            dir.0.join(format!("{live}.json")),
            serde_json::json!({
                "pid": live, "procStart": start, "pidDomain": "darwin",
                "tmux": "fixture:@1.%99", "messagingSocketPath": live_socket,
            })
            .to_string(),
        )
        .expect("write live record");
        write_key(&dir.0, live, start, "darwin");
        let mut target = seat(dead, Some("%99"), Harness::Claude);
        target.proc = None;
        let endpoint = UdsTransport::from_sessions_dir(dir.0.clone())
            .discover(&target)
            .expect("live candidate");
        assert_eq!(endpoint.socket_path, live_socket);
    }

    #[test]
    fn recycled_records_and_stale_exact_bindings_are_rejected() {
        let dir = TestDir::new();
        write_record(&dir.0, 4242, "%99", &dir.0.join("stale.sock"));
        let mut transport = transport(&dir.0);
        transport.observe_start = |_, utc| Some(if utc { 20260830000000 } else { 1 });
        assert!(
            transport
                .discover(&seat(4242, Some("%99"), Harness::Claude))
                .is_none()
        );
        transport.observe_start = |_, utc| Some(if utc { 20260829235605 } else { 2 });
        assert!(
            transport
                .discover(&seat(4242, None, Harness::Claude))
                .is_none()
        );
        transport.observe_start = |_, _| None;
        assert!(
            transport
                .discover(&seat(4242, Some("%99"), Harness::Claude))
                .is_none()
        );
    }

    #[test]
    fn two_live_pane_records_require_exact_identity_not_sort_order() {
        let dir = TestDir::new();
        write_record(&dir.0, 4242, "%99", &dir.0.join("first.sock"));
        write_record(&dir.0, 4343, "%99", &dir.0.join("second.sock"));
        let mut transport = transport(&dir.0);
        transport.observe_start = |_, utc| Some(if utc { 20260829235605 } else { 1 });
        let mut target = seat(4343, Some("%99"), Harness::Claude);
        assert_eq!(
            transport
                .discover(&target)
                .expect("exact identity")
                .socket_path,
            dir.0.join("second.sock")
        );
        target.proc = None;
        assert!(
            transport.discover(&target).is_none(),
            "no first-directory-entry authority"
        );
        fs::rename(dir.0.join("4343.json"), dir.0.join("0.json")).expect("reverse filename order");
        assert!(
            transport.discover(&target).is_none(),
            "renaming cannot resolve ambiguity"
        );
    }

    #[tokio::test]
    async fn discovery_is_pid_then_pane_suffix_then_absent() {
        let dir = TestDir::new();
        let nonexistent = dir.0.join("recorded.sock");
        write_record(&dir.0, 4242, "%99", &nonexistent);
        let transport = transport(&dir.0);
        let body = message(None);

        assert!(
            transport
                .can_deliver(&seat(4242, Some("%7"), Harness::Claude), &body)
                .await
                .expect("pid lookup")
        );
        let mut unbound = seat(9000, Some("%99"), Harness::Claude);
        unbound.proc = None;
        assert!(
            transport
                .can_deliver(&unbound, &body)
                .await
                .expect("proc-null pane fallback")
        );
        assert!(
            transport
                .can_deliver(&seat(9000, Some("%99"), Harness::Claude), &body)
                .await
                .expect("pane fallback")
        );
        assert!(
            !transport
                .can_deliver(&seat(9000, Some("%100"), Harness::Claude), &body)
                .await
                .expect("both absent")
        );
    }

    #[test]
    fn recorded_2_1_251_registry_pair_resolves_without_probing_the_socket() {
        let dir = TestDir::new();
        fs::write(
            dir.0.join("4242.json"),
            fixture("claude-2.1.251-session.json"),
        )
        .expect("copy recorded session");
        fs::write(
            dir.0.join(format!("4242.{HASH}.key")),
            fixture(&format!("4242.{HASH}.key")),
        )
        .expect("copy recorded key");

        let mut transport = transport(&dir.0);
        transport.observe_start =
            |pid, utc| (pid == 4242).then_some(if utc { 20260830001157 } else { 1 });
        let endpoint = transport
            .discover(&seat(9000, Some("%99"), Harness::Claude))
            .expect("pane fallback resolves recorded pair");
        assert_eq!(
            endpoint.socket_path,
            PathBuf::from("/tmp/cc-socks/4242.sock")
        );
        assert_eq!(endpoint.token, "sanitized-peer-token");
    }

    #[test]
    fn recorded_2_1_241_registry_pair_requires_equal_optional_pid_domains() {
        // Verbatim captures, not reconstructed fixtures:
        // git show 5b0d6243:docs/plans/135-rs-boot-announce/assets/inputs/claude-2.1.241-shapes/session-record.json
        // git show 5b0d6243:docs/plans/135-rs-boot-announce/assets/inputs/claude-2.1.241-shapes/peer-key.json
        let fixture_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-2.1.241");
        let recorded_session: Value = serde_json::from_slice(
            &fs::read(fixture_dir.join("session-record.json"))
                .expect("read captured 2.1.241 session"),
        )
        .expect("decode captured session");
        let recorded_key: Value = serde_json::from_slice(
            &fs::read(fixture_dir.join("peer-key.json")).expect("read captured 2.1.241 key"),
        )
        .expect("decode captured key");
        assert!(recorded_session.get("pidDomain").is_none());
        assert!(recorded_key.get("pidDomain").is_none());
        let _: super::SessionRecord = serde_json::from_value(recorded_session.clone())
            .expect("captured session without pidDomain must decode");
        let _: super::PeerKey = serde_json::from_value(recorded_key.clone())
            .expect("captured key without pidDomain must decode");

        for (session_domain, key_domain, expected) in [
            (None, None, true),
            (Some("darwin"), Some("darwin"), true),
            (None, Some("darwin"), false),
            (Some("darwin"), None, false),
            (Some("darwin"), Some("linux"), false),
        ] {
            let dir = TestDir::new();
            let mut session = recorded_session.clone();
            let mut key = recorded_key.clone();
            if let Some(domain) = session_domain {
                session["pidDomain"] = domain.into();
            }
            if let Some(domain) = key_domain {
                key["pidDomain"] = domain.into();
            }
            fs::write(dir.0.join("75005.json"), session.to_string())
                .expect("write captured session case");
            fs::write(dir.0.join(format!("75005.{HASH}.key")), key.to_string())
                .expect("write captured key case");
            let mut transport = transport(&dir.0);
            // Keep the captured human-date procStart unchanged in BOTH records.
            transport.observe_start =
                |pid, utc| (pid == 75005).then_some(if utc { 20260824002719 } else { 1 });
            let mut target = seat(75005, Some("%8"), Harness::Claude);
            target.proc = None;
            let endpoint = transport.discover(&target);
            assert_eq!(
                endpoint.is_some(),
                expected,
                "session domain {session_domain:?}, key domain {key_domain:?}"
            );
            if let Some(endpoint) = endpoint {
                assert_eq!(
                    endpoint.socket_path,
                    PathBuf::from("/tmp/cc-socks/75005.sock")
                );
                assert_eq!(endpoint.token, "<redacted>");
            }
        }
    }

    #[tokio::test]
    async fn command_and_every_non_claude_harness_refuse_the_socket() {
        let dir = TestDir::new();
        write_record(&dir.0, 4242, "%99", &dir.0.join("recorded.sock"));
        let transport = transport(&dir.0);

        assert!(
            !transport
                .can_deliver(
                    &seat(4242, Some("%99"), Harness::Claude),
                    &message(Some("compact")),
                )
                .await
                .expect("command predicate")
        );
        for harness in [Harness::Copilot, Harness::Codex, Harness::Pi, Harness::Omp] {
            assert!(
                !transport
                    .can_deliver(&seat(4242, Some("%99"), harness), &message(None))
                    .await
                    .expect("harness predicate"),
                "{harness} must not claim Claude's socket"
            );
        }
    }

    #[tokio::test]
    async fn unknown_or_refused_inbound_acceptance_is_closed_before_any_socket_write() {
        let dir = TestDir::new();
        let socket = dir.0.join("must-not-connect.sock");
        let listener = UnixListener::bind(&socket).expect("bind no-connect witness");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0);
        let mut target = seat(4242, Some("%99"), Harness::Claude);

        for stamp in [None, Some(false)] {
            target.cross_session_inbound_accept = stamp;
            assert!(
                !transport
                    .can_deliver(&target, &message(None))
                    .await
                    .unwrap()
            );
            assert_eq!(
                transport.deliver(&target, &message(None)).await.unwrap(),
                DeliveryOutcome::Queued {
                    reason: None,
                    next_retry_at: None,
                    draft_sha: None,
                }
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err(),
            "a closed capability must not probe or write the socket"
        );
    }

    #[tokio::test]
    async fn authentication_is_required_and_bound_to_record_identity() {
        let dir = TestDir::new();
        let socket = dir.0.join("recorded.sock");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0);
        let mut target = seat(4242, Some("%99"), Harness::Claude);
        assert!(
            transport
                .can_deliver(&target, &message(None))
                .await
                .unwrap()
        );
        target.proc = None;
        assert!(
            transport
                .can_deliver(&target, &message(None))
                .await
                .unwrap(),
            "pane lookup must still require the record's matching peer key"
        );

        let duplicate = dir
            .0
            .join("4242.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.key");
        fs::copy(dir.0.join(format!("4242.{HASH}.key")), &duplicate).unwrap();
        assert!(
            transport.discover(&target).is_none(),
            "two identity-matching keys are ambiguous and must fail closed"
        );
        fs::remove_file(duplicate).unwrap();

        write_key(&dir.0, 4242, "different process", "darwin");
        assert!(
            !transport
                .can_deliver(&target, &message(None))
                .await
                .unwrap()
        );

        write_key(&dir.0, 4242, "Sat Aug 29 23:56:05 2026", "linux");
        assert!(
            !transport
                .can_deliver(&target, &message(None))
                .await
                .unwrap()
        );

        fs::write(dir.0.join(format!("4242.{HASH}.key")), b"not json").unwrap();
        assert!(
            !transport
                .can_deliver(&target, &message(None))
                .await
                .unwrap()
        );
        assert_eq!(
            transport.deliver(&target, &message(None)).await.unwrap(),
            DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            },
            "an unauthenticated write must never become delivered on a silent timer"
        );
    }

    #[test]
    fn recorded_frame_is_byte_exact_and_contains_no_invented_mode() {
        let actual = format!(
            "{}\n",
            build_peer_frame(&message(None), FIXTURE_ORIGIN).expect("frame")
        );
        assert_eq!(actual, fixture("user-frame.ndjson"));
        assert!(!actual.contains("from-mode"));

        let mut escaped = message(None);
        escaped.from = SeatId::from("a&b\"<c>");
        let escaped = build_peer_frame(&escaped, FIXTURE_ORIGIN).expect("escaped frame");
        assert!(escaped.contains("from-name=\\\"a&amp;b&quot;&lt;c&gt;\\\""));
    }

    #[test]
    fn back_to_back_messages_have_two_explicit_body_boundaries() {
        let first = build_peer_frame(&message(None), FIXTURE_ORIGIN).expect("first frame");
        let mut second = message(None);
        second.msg_id = "msg-uds-002".to_string();
        second.body = "third line".to_string();
        let second = build_peer_frame(&second, FIXTURE_ORIGIN).expect("second frame");
        let transcript = [first, second]
            .into_iter()
            .map(|frame| {
                serde_json::from_str::<Value>(&frame).expect("peer frame")["message"]["content"]
                    .as_str()
                    .expect("user content")
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            transcript.matches("[/pij]").count(),
            2,
            "each body has its own closing boundary: {transcript}"
        );
        assert_eq!(
            transcript.matches("[pij-rs from pij-sender]").count(),
            2,
            "each body identifies the delivering generation: {transcript}"
        );
    }

    #[test]
    fn status_decision_table_reads_the_recorded_wire_shape() {
        // Recordings, not constructions — see the fixtures README.
        assert_eq!(
            classify_status(&fixture("status-held.ndjson"), "msg-uds-001"),
            Status::Held
        );
        assert_eq!(
            classify_status(&fixture("status-denied.ndjson"), "msg-uds-001"),
            Status::Denied
        );
        // A denial asked about ANY message id gives the same answer, for the same
        // reason a hold does: the channel correlates, not the payload.
        assert_eq!(
            classify_status(
                &fixture("status-denied.ndjson"),
                "a-completely-different-id"
            ),
            Status::Denied
        );
        // The recorded frames carry NO id tying them to our message, so the
        // decoder must NOT consult one: asking about a different message id has
        // to give the same answer, because the channel — not the payload — is
        // what correlates.
        assert_eq!(
            classify_status(&fixture("status-held.ndjson"), "a-completely-different-id"),
            Status::Held
        );
        assert_eq!(
            classify_status(&fixture("status-dropped.ndjson"), "msg-uds-001"),
            Status::Dropped
        );
        assert_eq!(
            classify_status("not json", "msg-uds-001"),
            Status::Malformed
        );
        // An unrelated control frame must never be read as a verdict.
        assert_eq!(
            classify_status(
                r#"{"type":"control","action":"peer_idle_notice","from":"uds:/tmp/x.sock"}"#,
                "msg-uds-001"
            ),
            Status::Unrelated
        );
    }

    /// The decoder that shipped required `type=="peer_message_status"`. Across
    /// five measured arms the CLI never emitted that; it emits `type=="control"`
    /// with `action=="peer_message_status"`. This pins the discrimination so a
    /// revert to the bare-only matcher fails here rather than silently making
    /// `Held` unreachable again.
    #[test]
    fn the_control_shape_is_what_the_live_cli_actually_emits() {
        let recorded = fixture("status-held.ndjson");
        let parsed: Value = serde_json::from_str(&recorded).expect("recorded status is json");
        assert_eq!(parsed.get("type").and_then(Value::as_str), Some("control"));
        assert_eq!(
            parsed.get("action").and_then(Value::as_str),
            Some("peer_message_status")
        );
        assert!(
            parsed.get("orig_msg_id").is_none() && parsed.get("wereHeld").is_none(),
            "the recorded frame must carry no correlation id — that is why correlation is positional"
        );
    }

    /// Where the fake server sends its status line.
    #[derive(Clone, Copy, PartialEq)]
    enum Reply {
        /// Connect back to the address the frame advertised — what the real CLI does.
        ToAdvertisedAddress,
        /// Answer on the delivery connection instead.
        OnTheSameConnection,
        /// Never answer. This is what an UNROUTABLE `from=` looks like from here,
        /// and it is the pre-fix behaviour the whole unit exists to remove.
        Never,
    }

    struct Exchange {
        outcome: DeliveryOutcome,
        received: String,
    }

    async fn socket_case(response: Option<&str>, close_early: bool, reply: Reply) -> Exchange {
        let dir = TestDir::new();
        let socket = dir.0.join("listener.sock");
        let listener = UnixListener::bind(&socket).expect("bind fixture socket");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0);
        let response = response.map(str::to_string);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept transport");
            let mut reader = BufReader::new(stream);
            let mut received = String::new();
            reader.read_line(&mut received).await.expect("read auth");
            let mut frame = String::new();
            reader.read_line(&mut frame).await.expect("read frame");
            received.push_str(&frame);

            if let Some(response) = response {
                match reply {
                    Reply::Never => {}
                    Reply::OnTheSameConnection => {
                        write_halved(reader.get_mut(), &response).await;
                    }
                    Reply::ToAdvertisedAddress => {
                        let parsed: Value =
                            serde_json::from_str(frame.trim()).expect("frame is json");
                        let advertised = parsed
                            .get("from")
                            .and_then(Value::as_str)
                            .expect("frame advertises a reply address")
                            .strip_prefix("uds:")
                            .expect("reply address is a uds: address")
                            .to_string();
                        let mut back = UnixStream::connect(&advertised)
                            .await
                            .expect("the advertised reply address must be reachable");
                        write_halved(&mut back, &response).await;
                        // Close the reply channel at once. A HOLD is no longer
                        // terminal — the transport keeps listening for the denial
                        // that may follow — so leaving this open would make the
                        // outcome depend on a sleep racing the ack deadline, and
                        // these tests would pass or fail by machine load rather
                        // than by behaviour. EOF is the recipient saying "that is
                        // all", which is exactly what a real one-status exchange
                        // does.
                        drop(back);
                    }
                }
            }
            if !close_early {
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            received
        });

        let target = seat(4242, Some("%99"), Harness::Claude);
        let outcome = transport
            .deliver(&target, &message(None))
            .await
            .expect("delivery result");
        let received = tokio::time::timeout(Duration::from_millis(800), server)
            .await
            .expect("fixture socket must be reached")
            .expect("fixture server");
        Exchange { outcome, received }
    }

    /// Split every write so a status line that arrives in two chunks is still read
    /// as one line — the real socket does not promise message boundaries.
    async fn write_halved(stream: &mut UnixStream, response: &str) {
        let midpoint = response.len() / 2;
        stream
            .write_all(&response.as_bytes()[..midpoint])
            .await
            .expect("write first status fragment");
        tokio::task::yield_now().await;
        stream
            .write_all(&response.as_bytes()[midpoint..])
            .await
            .expect("write second status fragment");
    }

    #[tokio::test]
    async fn silence_is_never_upgraded_past_injected_to_transport() {
        assert_eq!(
            socket_case(None, false, Reply::Never).await.outcome,
            DeliveryOutcome::Delivered {
                origin: DeliveryOrigin::InjectedToTransport
            }
        );
    }

    #[tokio::test]
    async fn a_held_status_on_the_advertised_address_is_observed_as_held() {
        assert_eq!(
            socket_case(
                Some(&fixture("status-held.ndjson")),
                false,
                Reply::ToAdvertisedAddress
            )
            .await
            .outcome,
            DeliveryOutcome::Held {
                reason: "recipient approval pending".to_string()
            }
        );
    }

    #[tokio::test]
    async fn a_denial_is_terminal_refusal_not_a_pending_hold() {
        assert_eq!(
            socket_case(
                Some(&fixture("status-denied.ndjson")),
                false,
                Reply::ToAdvertisedAddress
            )
            .await
            .outcome,
            DeliveryOutcome::Refused {
                reason: "the recipient declined the message".to_string()
            }
        );
    }

    /// THE GUARD FOR THE WHOLE UNIT. A status the recipient tried to send but
    /// could not route — exactly what an unroutable `from=` produces — must not
    /// become Delivered by default. If the reply address ever stops being
    /// load-bearing, this is the test that notices.
    #[tokio::test]
    async fn a_status_that_never_reaches_us_is_the_pre_fix_defect_and_stays_unclaimed() {
        let held = fixture("status-held.ndjson");
        let unreachable = socket_case(Some(&held), false, Reply::Never).await;
        assert_eq!(
            unreachable.outcome,
            DeliveryOutcome::Delivered {
                origin: DeliveryOrigin::InjectedToTransport
            },
            "an unheard hold receipt is the defect; it must never be an upgrade"
        );
        let reachable = socket_case(Some(&held), false, Reply::ToAdvertisedAddress).await;
        assert_ne!(
            unreachable.outcome, reachable.outcome,
            "routable and unroutable must be DIFFERENT observable outcomes, or the address proves nothing"
        );
    }

    #[tokio::test]
    async fn a_status_arriving_on_the_delivery_connection_is_still_honoured() {
        assert_eq!(
            socket_case(
                Some(&fixture("status-held.ndjson")),
                false,
                Reply::OnTheSameConnection
            )
            .await
            .outcome,
            DeliveryOutcome::Held {
                reason: "recipient approval pending".to_string()
            }
        );
    }

    /// THE SEQUENCE A HUMAN ACTUALLY PRODUCES. Measured live: `held` at 58 ms,
    /// then `denied` at 1.74 s, both on the reply address. Returning at the hold
    /// drops the refusal — the only thing the person actually said. Mutate
    /// `wait_for_status` back to `decided => return Some(decided)` and this fails.
    #[tokio::test]
    async fn a_hold_followed_by_a_denial_is_refused_not_held() {
        let dir = TestDir::new();
        let socket = dir.0.join("listener.sock");
        let listener = UnixListener::bind(&socket).expect("bind fixture socket");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0)
            .with_ack_wait(Duration::from_millis(600))
            .with_hold_grace(Duration::from_millis(400));
        let held = fixture("status-held.ndjson");
        let denied = fixture("status-denied.ndjson");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept transport");
            let mut reader = BufReader::new(stream);
            let mut auth = String::new();
            reader.read_line(&mut auth).await.expect("read auth");
            let mut frame = String::new();
            reader.read_line(&mut frame).await.expect("read frame");
            let parsed: Value = serde_json::from_str(frame.trim()).expect("frame is json");
            let advertised = parsed
                .get("from")
                .and_then(Value::as_str)
                .expect("reply address")
                .strip_prefix("uds:")
                .expect("uds address")
                .to_string();
            let mut back = UnixStream::connect(&advertised)
                .await
                .expect("reply address reachable");
            back.write_all(held.as_bytes()).await.expect("write held");
            // The human takes a moment. That is the whole point.
            tokio::time::sleep(Duration::from_millis(120)).await;
            back.write_all(denied.as_bytes())
                .await
                .expect("write denied");
            tokio::time::sleep(Duration::from_millis(60)).await;
        });

        let target = seat(4242, Some("%99"), Harness::Claude);
        let outcome = transport
            .deliver(&target, &message(None))
            .await
            .expect("delivery result");
        let _ = tokio::time::timeout(Duration::from_millis(900), server).await;
        assert_eq!(
            outcome,
            DeliveryOutcome::Refused {
                reason: "the recipient declined the message".to_string()
            },
            "a hold that becomes a denial must be reported as the denial"
        );
    }

    /// R2 — THE HOLD-GRACE EXTENSION NEEDS ITS OWN WITNESS. The hold-then-denial
    /// test's denial lands INSIDE the base deadline, so mutating the grace away
    /// left it green: the extension was unguarded. Here the denial arrives AFTER
    /// the base deadline and can only be seen because a hold extends the window.
    /// Mutate `until` back to `deadline` and this fails while everything else
    /// stays green.
    #[tokio::test]
    async fn a_denial_after_the_base_deadline_is_still_heard() {
        let dir = TestDir::new();
        let socket = dir.0.join("listener.sock");
        let listener = UnixListener::bind(&socket).expect("bind fixture socket");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0)
            .with_ack_wait(Duration::from_millis(60))
            .with_hold_grace(Duration::from_millis(600));
        let held = fixture("status-held.ndjson");
        let denied = fixture("status-denied.ndjson");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept transport");
            let mut reader = BufReader::new(stream);
            let mut auth = String::new();
            reader.read_line(&mut auth).await.expect("read auth");
            let mut frame = String::new();
            reader.read_line(&mut frame).await.expect("read frame");
            let parsed: Value = serde_json::from_str(frame.trim()).expect("frame is json");
            let advertised = parsed
                .get("from")
                .and_then(Value::as_str)
                .expect("reply address")
                .strip_prefix("uds:")
                .expect("uds address")
                .to_string();
            // The hold arrives promptly, on its own connection, which then closes —
            // exactly what the live CLI does.
            let mut first = UnixStream::connect(&advertised)
                .await
                .expect("hold channel");
            first.write_all(held.as_bytes()).await.expect("write held");
            drop(first);
            // The human answers AFTER the base ack deadline has passed.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let mut second = UnixStream::connect(&advertised)
                .await
                .expect("the reply address must still exist after the base deadline");
            second
                .write_all(denied.as_bytes())
                .await
                .expect("write denied");
            drop(second);
        });

        let target = seat(4242, Some("%99"), Harness::Claude);
        let outcome = transport
            .deliver(&target, &message(None))
            .await
            .expect("delivery result");
        let _ = tokio::time::timeout(Duration::from_millis(1200), server).await;
        assert_eq!(
            outcome,
            DeliveryOutcome::Refused {
                reason: "the recipient declined the message".to_string()
            },
            "a hold must extend the window past the base deadline, or the human's answer is lost"
        );
    }

    /// R1 — A HANGUP AFTER A HOLD MUST NOT FORGET THE HOLD. `select!` cancels the
    /// losing arm, so while the seen-hold flag was a local of `wait_for_status` a
    /// delivery-connection hangup restarted the wait empty and reported Queued —
    /// which retries at zero delay and raises a SECOND dialog at a human still
    /// looking at the first. Mutate `seen_hold` back to a fresh `None` and this
    /// fails.
    #[tokio::test]
    async fn a_hangup_after_a_hold_does_not_discard_the_hold() {
        let dir = TestDir::new();
        let socket = dir.0.join("listener.sock");
        let listener = UnixListener::bind(&socket).expect("bind fixture socket");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0)
            .with_ack_wait(Duration::from_millis(80))
            .with_hold_grace(Duration::from_millis(600));
        let held = fixture("status-held.ndjson");
        let denied = fixture("status-denied.ndjson");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept transport");
            let mut reader = BufReader::new(stream);
            let mut auth = String::new();
            reader.read_line(&mut auth).await.expect("read auth");
            let mut frame = String::new();
            reader.read_line(&mut frame).await.expect("read frame");
            let parsed: Value = serde_json::from_str(frame.trim()).expect("frame is json");
            let advertised = parsed
                .get("from")
                .and_then(Value::as_str)
                .expect("reply address")
                .strip_prefix("uds:")
                .expect("uds address")
                .to_string();

            let mut hold_channel = UnixStream::connect(&advertised)
                .await
                .expect("hold channel");
            hold_channel
                .write_all(held.as_bytes())
                .await
                .expect("write held");
            drop(hold_channel);
            tokio::time::sleep(Duration::from_millis(40)).await;
            // NOW HANG UP THE DELIVERY CONNECTION, while the hold is outstanding.
            // This is the ordering the live CLI happens not to use; a build that
            // did would have silently lost every hold.
            drop(reader);
            tokio::time::sleep(Duration::from_millis(150)).await;
            let mut verdict = UnixStream::connect(&advertised)
                .await
                .expect("the reply address must outlive the delivery connection");
            verdict
                .write_all(denied.as_bytes())
                .await
                .expect("write denied");
            drop(verdict);
        });

        let target = seat(4242, Some("%99"), Harness::Claude);
        let outcome = transport
            .deliver(&target, &message(None))
            .await
            .expect("delivery result");
        let _ = tokio::time::timeout(Duration::from_millis(1200), server).await;
        assert_eq!(
            outcome,
            DeliveryOutcome::Refused {
                reason: "the recipient declined the message".to_string()
            },
            "a hangup must not discard a hold already seen, or the human is asked twice"
        );
    }

    #[tokio::test]
    async fn losing_the_ack_channel_before_the_timer_cannot_confirm_delivery() {
        assert_eq!(
            socket_case(None, true, Reply::Never).await.outcome,
            DeliveryOutcome::Queued {
                reason: None,
                next_retry_at: None,
                draft_sha: None,
            }
        );
    }

    /// CONCURRENCY IS REFUSED, NOT RESOLVED. The status frames carry no
    /// correlation id, so a second message on one connection could never be told
    /// apart from the first. The transport therefore writes exactly one frame per
    /// connection and gives it a private reply address. This asserts the wire, so
    /// a future batching "optimisation" fails here instead of silently minting a
    /// verdict for the wrong message.
    #[tokio::test]
    async fn exactly_one_message_is_ever_in_flight_on_one_connection() {
        let exchange = socket_case(None, false, Reply::Never).await;
        let lines: Vec<&str> = exchange.received.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "expected exactly an auth line and ONE frame, got: {lines:?}"
        );
        let auth: Value = serde_json::from_str(lines[0]).expect("auth line is json");
        assert_eq!(auth.get("type").and_then(Value::as_str), Some("auth"));
        let frame: Value = serde_json::from_str(lines[1]).expect("frame is json");
        assert_eq!(frame.get("type").and_then(Value::as_str), Some("user"));
        let advertised = frame
            .get("from")
            .and_then(Value::as_str)
            .expect("frame advertises a reply address");
        assert!(
            advertised.starts_with("uds:/") && advertised.contains(REPLY_PREFIX),
            "from= must be a routable path in the recipient's namespace, got {advertised}"
        );
    }

    /// Two deliveries at once must NOT share a reply address, or the first status
    /// to arrive would be attributed to whichever message asked first.
    /// TWO REAL DELIVERIES, not two calls to `bind`. The earlier version of this
    /// test bound an inbox twice and compared the paths — which would stay green
    /// against a regression that shared ONE inbox across deliveries, i.e. exactly
    /// the cross-attribution hazard the name claims to prevent (message A's denial
    /// attributed to B). Found by the reviewer; the name was writing a cheque the
    /// body did not cash.
    #[tokio::test]
    async fn concurrent_deliveries_do_not_share_a_reply_address() {
        let dir = TestDir::new();
        let socket = dir.0.join("listener.sock");
        let listener = UnixListener::bind(&socket).expect("bind fixture socket");
        write_record(&dir.0, 4242, "%99", &socket);
        let transport = transport(&dir.0);

        let server = tokio::spawn(async move {
            let mut advertised = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("accept transport");
                let mut reader = BufReader::new(stream);
                let mut auth = String::new();
                reader.read_line(&mut auth).await.expect("read auth");
                let mut frame = String::new();
                reader.read_line(&mut frame).await.expect("read frame");
                let parsed: Value = serde_json::from_str(frame.trim()).expect("frame is json");
                advertised.push(
                    parsed
                        .get("from")
                        .and_then(Value::as_str)
                        .expect("reply address")
                        .to_string(),
                );
            }
            advertised
        });

        let target = seat(4242, Some("%99"), Harness::Claude);
        let one = message(None);
        let two = message(None);
        let (first, second) = tokio::join!(
            transport.deliver(&target, &one),
            transport.deliver(&target, &two),
        );
        first.expect("first delivery");
        second.expect("second delivery");

        let advertised = tokio::time::timeout(Duration::from_millis(800), server)
            .await
            .expect("both deliveries must reach the fixture socket")
            .expect("fixture server");
        assert_eq!(advertised.len(), 2);
        assert_ne!(
            advertised[0], advertised[1],
            "two deliveries in flight must not share a reply address, or the first status \
             to arrive is attributed to whichever asked first"
        );
    }

    #[tokio::test]
    async fn the_reply_socket_is_removed_when_the_delivery_ends() {
        let dir = TestDir::new();
        let path = {
            let inbox = ReplyInbox::bind(&dir.0.join("listener.sock")).expect("inbox");
            inbox.path.clone()
        };
        assert!(
            !path.exists(),
            "a reply socket must not outlive its delivery"
        );
    }
}
