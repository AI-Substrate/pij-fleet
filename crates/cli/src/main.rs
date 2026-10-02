//! `pij-rs` — one binary, two composition roots.

mod commit_trailers;
mod daemon_log;
mod identity;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode, Stdio};
use std::time::Duration;

use clap::{Parser, Subcommand};
use pij_cli::{
    CallerContext, DaemonClient, IdentityRequest, Registration, ReviveRequest, SendRequest,
    SpawnRequest, bounce, daemon_config, default_state_dir, exit_code, parse_destination, render,
    render_frame, setup_refusal,
};
use pij_core::error::PijError;
use pij_core::model::{Envelope, ErrorKind, Event, Harness, SeatDescriptor, SeatId, SemanticState};
use pij_core::wire;
use pij_harnesses::{
    claude_homes, copilot_home, ensure_claude_inbound_accept, ensure_claude_session_start_hook,
    ensure_claude_statusline, ensure_claude_stop_failure_hook, ensure_claude_stop_hook,
    ensure_claude_user_prompt_submit_hook, ensure_copilot_statusline, inspect_claude_inbound,
    inspect_claude_session_start_hook, inspect_claude_statusline,
    install_claude_session_start_script, install_claude_statusline_script,
    install_claude_stop_script, install_claude_user_prompt_submit_script,
    install_copilot_statusline_script, validate_executable_override,
};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "pij-rs", version, about = "pij, in Rust")]
struct Cli {
    /// Emit JSON from the same value used for human output.
    #[arg(long, global = true)]
    json: bool,

    /// Where the daemon keeps its per-boot key.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    /// Address of the daemon used by client verbs. Resolution is `--addr`, then
    /// `PIJ_RS_ADDR`, then `127.0.0.1:7461`.
    #[arg(long, global = true)]
    addr: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Debug)]
struct MachineCursor {
    machine: String,
    cursor: u64,
}

#[derive(Subcommand)]
enum Command {
    /// Run or replace the daemon.
    Daemon {
        /// Lifecycle action; absent runs the daemon in the foreground.
        #[command(subcommand)]
        action: Option<DaemonAction>,
        /// Address to listen on. Resolution is `--bind`, then `PIJ_RS_BIND`,
        /// then `127.0.0.1:7461`. Non-loopback exposes the bearer-key boundary.
        #[arg(long)]
        bind: Option<String>,
        /// Boot with every adapter FAKE: no store, no process table, nothing
        /// outside this process. Opt-in, because a daemon that silently persists
        /// nothing is the more dangerous default.
        #[arg(long)]
        offline: bool,
    },
    /// Ask a running daemon whether it is healthy.
    Ping,
    /// Internal child observer used by the daemon's tmux launch wrapper.
    #[command(name = "__spawn-child", hide = true)]
    SpawnChild {
        /// Durable child exit-status witness.
        #[arg(long)]
        status_file: PathBuf,
        /// Durable stderr tail source for a failed launch.
        #[arg(long)]
        log_file: PathBuf,
        /// Exact executable and argv after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },
    /// Inspect local client configuration without changing it.
    Doctor {
        #[command(subcommand)]
        action: DoctorAction,
    },
    /// Register one seat claim for daemon verification.
    Register {
        /// Existing same-process seat replaced by this registration.
        #[arg(long)]
        supersedes: Option<String>,
        /// Requested seat id.
        id: String,
        /// Host harness.
        #[arg(long, value_parser = parse_harness)]
        harness: Harness,
        /// Absolute working folder.
        #[arg(long, value_parser = parse_absolute_path)]
        folder: String,
        /// Loaded pij extension build reported by the registering runtime.
        #[arg(long)]
        extension_build: Option<String>,
        /// Real directory the runtime loaded its pij extension from.
        #[arg(long)]
        extension_path: Option<String>,
        /// Tmux pane, when one exists.
        #[arg(long)]
        pane: Option<String>,
        /// Process id; must be paired with --proc-start.
        #[arg(long, requires = "proc_start")]
        pid: Option<u32>,
        /// Packed process start stamp; must be paired with --pid.
        #[arg(long, requires = "pid")]
        proc_start: Option<u64>,
        /// Spawn correlation id.
        #[arg(long)]
        spawn_id: Option<String>,
        /// Requested model selector.
        #[arg(long)]
        model: Option<String>,
        /// Model provider.
        #[arg(long)]
        provider: Option<String>,
        /// Requested reasoning effort.
        #[arg(long)]
        effort: Option<String>,
        /// Governing seat.
        #[arg(long)]
        parent: Option<String>,
        /// Explicit role assertion; omission preserves the assignment.
        #[arg(long)]
        role: Option<String>,
        /// Delivery uses a relay.
        #[arg(long)]
        relay: bool,
    },
    /// Register this seat, with the DAEMON minting its name.
    ///
    /// The pane form is the one `/pij ready` types. Identity is derived from the
    /// pane by the daemon, never asserted here: this process is a short-lived
    /// child of the seat, so its own pid is not the seat's.
    Adopt {
        /// Pane to adopt. Defaults to `$TMUX_PANE`.
        pane: Option<String>,
        /// Host harness.
        #[arg(long)]
        harness: String,
        /// Governing seat. Omitted means UNSAID — an existing parent survives.
        #[arg(long)]
        parent: Option<String>,
        /// Explicit role assertion; omission preserves the assignment.
        #[arg(long)]
        role: Option<String>,
        /// Loaded pij extension build reported by the adopting runtime.
        #[arg(long)]
        extension_build: Option<String>,
        /// Real directory the runtime loaded its pij extension from.
        #[arg(long)]
        extension_path: Option<String>,
        /// Harness-native conversation id. Accepted only from a child of the
        /// pane's harness process (status line, SessionStart hook).
        #[arg(long)]
        harness_session: Option<String>,
        /// Operator reclaim of a dead seat onto this pane (parent or prime only).
        #[arg(long)]
        reclaim: Option<String>,
    },
    /// Name the seat this caller is, from the store it registered into.
    Whoami {
        /// Asserted seat id. Defaults to `$PIJ_SESSION_ID`.
        #[arg(long)]
        seat: Option<String>,
        /// Pane to derive from. Defaults to `$TMUX_PANE`.
        #[arg(long)]
        pane: Option<String>,
    },
    /// Print derived git commit trailers; unavailable values are diagnosed on stderr.
    CommitTrailers,
    /// Write a project's Context Tax report: report.json, a static page and the tables.
    ///
    /// Folds the transcripts in-process and reads pij's seats read-only; it never
    /// contacts the daemon. The output folder holds names and paths unless
    /// `--anonymise`; never commit it.
    FleetReport {
        /// The project root; sessions working inside it are in scope.
        folder: PathBuf,
        /// Add every path from `git worktree list --porcelain` of FOLDER.
        #[arg(long)]
        with_worktrees: bool,
        /// Window start: RFC 3339, a local YYYY-MM-DD, or a span ago (7d, 36h). Default 7d.
        #[arg(long)]
        since: Option<String>,
        /// Window end, same forms. Default now.
        #[arg(long)]
        until: Option<String>,
        /// Harnesses to read, comma-separated (claude-code, omp, codex, copilot). Default all.
        #[arg(long)]
        harness: Option<String>,
        /// Output folder. Default ~/.pij-rs/fleet-reports/<folder>-<UTC stamp>.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Table format: jsonl or csv (parquet is not shipped yet).
        #[arg(long, default_value = "jsonl")]
        format: String,
        /// Keep turn-opener heads in the tables. Local use only.
        #[arg(long)]
        include_content: bool,
        /// Replace seat names with roles and letters; drop paths, ids and content.
        #[arg(long)]
        anonymise: bool,
        /// Reuse a Unisphere prep folder (not supported yet).
        #[arg(long)]
        prep_target: Option<PathBuf>,
        /// The report clock as +HH:MM. Default the machine's offset.
        #[arg(long)]
        utc_offset: Option<String>,
        /// Fold threads (C12: keep it at or under 8 on a shared machine).
        #[arg(long, default_value_t = 4)]
        threads: usize,
    },
    /// Confirm the binding `adopt` already made.
    Phonehome {
        /// Asserted seat id. Defaults to `$PIJ_SESSION_ID`.
        #[arg(long)]
        seat: Option<String>,
        /// Pane to derive from. Defaults to `$TMUX_PANE`.
        #[arg(long)]
        pane: Option<String>,
    },
    /// Launch one pre-bind seat through the daemon.
    Spawn {
        /// Seat id. OMIT IT and the daemon allocates a memorable one
        /// (`pij-<adjective>-<noun>`).
        #[arg(long)]
        id: Option<String>,
        /// Harness to launch.
        #[arg(long, value_parser = parse_harness)]
        harness: Harness,
        /// Explicitly allow a harness retired by this machine's daemon policy.
        #[arg(long)]
        allow_retired: bool,
        /// Absolute executable-path override; never selects a different harness.
        #[arg(long)]
        bin: Option<String>,
        /// Exact model selector.
        #[arg(long)]
        model: Option<String>,
        /// Exact reasoning effort.
        #[arg(long)]
        effort: Option<String>,
        /// Absolute working directory; defaults to the caller's directory.
        #[arg(long, value_parser = parse_absolute_path_buf)]
        cwd: Option<PathBuf>,
        /// Target tmux session; defaults from the caller pane.
        #[arg(long)]
        session: Option<String>,
        /// Tmux window name; defaults to the seat id.
        #[arg(long)]
        name: Option<String>,
        /// Governing seat.
        #[arg(long)]
        parent: Option<String>,
        /// Configure Claude to accept cross-session inbound messages.
        /// Claude seats default to accepting; this flag affirms that default
        /// explicitly and is kept for existing callers.
        #[arg(long, conflicts_with = "no_accept_inbound")]
        accept_inbound: bool,
        /// Opt this Claude seat OUT of the default inbound-consent stamp.
        /// Has no effect on non-Claude harnesses, which never accept it.
        #[arg(long, conflicts_with = "accept_inbound")]
        no_accept_inbound: bool,
        /// Return immediately after tmux accepts the launch.
        #[arg(long)]
        no_wait: bool,
        /// Maximum seconds to wait for the spawned seat to register.
        #[arg(long, default_value_t = 30)]
        wait_seconds: u64,
    },
    /// Relaunch a tombstoned or observed-dead seat with its prior launch intent.
    Revive {
        /// Existing seat id.
        id: String,
        /// Target tmux session; defaults from the caller pane.
        #[arg(long)]
        session: Option<String>,
        /// Tmux window name; defaults to the seat id.
        #[arg(long)]
        name: Option<String>,
        /// Override recycled/unknown process evidence; requires parent or parentless prime.
        #[arg(long, requires = "evidence")]
        assume_dead: bool,
        /// Explain the death evidence retained in the durable revive audit.
        #[arg(long, requires = "assume_dead")]
        evidence: Option<String>,
        /// Launch a new, blank conversation instead of resuming the recorded one.
        #[arg(long)]
        fresh: bool,
    },
    /// Durably enqueue one message as the acting seat.
    #[command(
        after_long_help = "Examples:\n  pij-rs send --to <id> --body 'hello'\n  pij-rs send --to <id> --body-file -\n  pij-rs send --to <id> --fyi --body 'merged #452, nothing needed from you' (their next action doesn't depend on it: held, opens no turn)\n  pij-rs send --to <id> --force --reason 'prod is down' --body '...' (wake a cold seat anyway; audited)"
    )]
    Send {
        /// Assert the derived sending seat. Omit to use PIJ_SESSION_ID or TMUX_PANE.
        #[arg(long)]
        from: Option<String>,
        /// `<seat>` or `<seat>@<machine>`; use `@@` for a literal seat `@`.
        #[arg(long, value_parser = parse_destination)]
        to: pij_core::model::Destination,
        /// Literal message body, including values beginning with `-`.
        #[arg(long, allow_hyphen_values = true, conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the literal body from a path, or from stdin with `-`.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Execute a remote control instead of sending a message body.
        #[arg(long)]
        command: Option<String>,
        /// Correlation id. Omit to mint a UUIDv7.
        #[arg(long)]
        msg_id: Option<String>,
        /// Message id being answered.
        #[arg(long)]
        in_reply_to: Option<String>,
        /// Use only when the recipient's next action doesn't depend on it. If they'd
        /// be stuck, wrong or waiting without it, it's a normal send. If they'd be
        /// fine not reading it until their next turn, it's `--fyi`. If they'd be fine
        /// never reading it, don't send it. Always normal sends: work done or a phase
        /// complete, review verdicts, hand-offs, blockers, questions, decisions
        /// needed. Held until the recipient's next real turn (appended to their next
        /// message or typed prompt); five pending flush as one message once the
        /// seat is warm. Receipt: `held (fyi)`, with a warning when the body looks
        /// like a question.
        #[arg(long, conflicts_with = "command")]
        fyi: bool,
        /// Wake a cold recipient anyway (large context, idle past the cache TTL).
        /// Requires --reason; the forced wake and its estimated price are audited.
        #[arg(long, conflicts_with_all = ["command", "fyi"], requires = "reason")]
        force: bool,
        /// Why a forced cold wake is worth its price.
        #[arg(long, requires = "force")]
        reason: Option<String>,
    },
    /// Publish this seat's busy/idle state, for a turn-boundary hook (Claude's
    /// UserPromptSubmit and Stop). Prints nothing on success; `--json` prints the envelope.
    Activity {
        /// The seat. Defaults to the live seat bound to `--pane`.
        #[arg(long)]
        seat: Option<String>,
        /// The seat's tmux pane, as binding evidence.
        #[arg(long)]
        pane: Option<String>,
        /// The seat's harness-native session id, as binding evidence.
        #[arg(long)]
        native_session: Option<String>,
        /// `working` or `idle`.
        #[arg(long, value_parser = ["working", "idle"])]
        state: String,
    },
    /// Claim the FYIs held for a seat, for a typed-turn hook. Prints the block, or
    /// nothing when none are held; `--json` prints the envelope.
    FyiClaim {
        /// The seat. Defaults to the live seat bound to `--pane`.
        #[arg(long)]
        seat: Option<String>,
        /// The seat's tmux pane, as binding evidence.
        #[arg(long)]
        pane: Option<String>,
        /// The seat's harness-native session id, as binding evidence.
        #[arg(long)]
        native_session: Option<String>,
        /// Which hook is claiming: hook:claude, hook:copilot, hook:omp or hook:pi.
        #[arg(long)]
        via: String,
    },
    /// Read, in full, the FYIs one claim delivered (plan 159): the command a
    /// digest names. Read-only.
    FyiRead {
        /// The seat the FYIs were delivered to.
        #[arg(long)]
        seat: String,
        /// When they were claimed, in epoch ms, as the digest names it.
        #[arg(long)]
        claimed_at: u64,
    },
    /// Compact the current, daemon-derived seat.
    CompactSelf,
    /// Destructively read and acknowledge this acting seat's inbox.
    ///
    /// `--peek` inspects another seat; `release` is explicit prime/parent recovery.
    Inbox {
        #[command(subcommand)]
        action: Option<InboxAction>,
        /// Inbox to read. Defaults to the derived acting seat.
        #[arg(long)]
        seat: Option<String>,
        /// Wait until at least one message is available.
        #[arg(long, conflicts_with = "peek")]
        wait: bool,
        /// Inspect without claiming or acknowledging; permits an explicit other seat.
        #[arg(long)]
        peek: bool,
    },
    /// Run detached commands and recover their daemon-owned output.
    Bg {
        #[command(subcommand)]
        action: BgAction,
    },
    /// Enqueue one sidecar job: telegram send, background command, or chore.
    ///
    /// The consumers are queue-backed, so this is the shipped PRODUCER. Live-fire
    /// proof that enqueues rows by hand proves the harness, not the product.
    Sidecar {
        /// Which consumer claims the job.
        #[command(subcommand)]
        action: SidecarAction,
    },
    /// Record this seat's now/next card, or declare its semantic state.
    ///
    /// The arguments are passed to the daemon UNPARSED, verb included, so the
    /// grammar has exactly one implementation and the two surfaces cannot drift:
    ///
    ///   pij-rs report now "<did>" "<next>" [--state <word>] [--note <text>]
    ///   pij-rs report state <word> [--assignment <id>] [--refs <csv>]
    ///   pij-rs report blocked|question "<text>" [--assignment <id>] [--refs <csv>]
    ///   pij-rs report clear [--assignment <id>]
    ///   pij-rs report verify <seat> [--assignment <id>]
    ///
    /// Available semantic state words are listed below.
    #[command(after_long_help = SemanticState::WORDS)]
    Report {
        /// Assert the derived reporting seat. Omit to use the observable pane,
        /// or PIJ_SESSION_ID when no pane exists; contradictions are refused.

        #[arg(long)]
        seat: Option<String>,
        /// The report arguments, exactly as they would be typed.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Assert or unset a seat role through daemon ownership checks.
    Role {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Tombstone a seat owned by this caller without killing its process or pane.
    Close {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Reconcile only positively stale process and pane bindings.
    Reap {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read current governance anomalies without changing their evidence.
    Anomalies {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read durable decisions under current-parent responsibility.
    Decisions {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Answer a durable question as its asker or current parent.
    Answer {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Create, list, read or update governed projects.
    Project {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Create, inspect or close a governed stream.
    Stream {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Declare or inspect descriptive write fences.
    Fence {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Send a hash-bound packet and inspect its receipt.
    Dispatch {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Acknowledge a dispatch as its actual recipient.
    Ack {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Verify a nonce-correlated live peer acknowledgement.
    Canary {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Attach plan identity to a seat.
    Attest {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Open or close a task assignment.
    Task {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read a seat and its governed work tree.
    Node {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage batons, prime designation and role assertions.
    Orchestration {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Append, read or render the Rust event spine.
    Spine {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List registered seats; --here scopes to this process's cwd.
    List {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read one seat's state card back.
    State {
        /// The seat to read.
        id: SeatId,
    },
    /// Follow daemon event and peer-state frames until interrupted.
    Tail {
        /// Replay after `MACHINE=CURSOR`; repeat once per machine.
        #[arg(long, value_parser = parse_machine_cursor)]
        since: Vec<MachineCursor>,
    },
}

#[derive(Clone, Debug, Subcommand)]
enum InboxAction {
    /// Park a running head after observing failed consumption; never claims delivery.
    Release {
        #[arg(long)]
        seat: String,
        #[arg(long)]
        job: u64,
        #[arg(long)]
        evidence: String,
    },
}

#[derive(Clone, Debug, Subcommand)]
enum BgAction {
    /// Start a command; its completion arrives as a turn from pij-bg.
    Create {
        /// Human title included in the completion turn.
        #[arg(long, allow_hyphen_values = true)]
        title: String,
        /// Literal command interpreted by /bin/sh in the caller's recorded folder.
        #[arg(long, allow_hyphen_values = true)]
        command: String,
    },
    /// List your jobs, including finished jobs.
    List {
        /// Also include jobs owned by your directly recorded children.
        #[arg(long)]
        all: bool,
    },
    /// Read a bounded snapshot from the daemon-owned log.
    Tail {
        job: String,
        #[arg(long)]
        lines: Option<usize>,
    },
    /// Stop a job owned by you or your directly recorded child.
    Kill { job: String },
}

impl BgAction {
    fn argv(self) -> Vec<String> {
        let mut argv = vec!["bg".to_string()];
        match self {
            Self::Create { title, command } => {
                argv.extend([
                    "create".to_string(),
                    "--title".to_string(),
                    title,
                    "--command".to_string(),
                    command,
                ]);
            }
            Self::List { all } => {
                argv.push("list".to_string());
                if all {
                    argv.push("--all".to_string());
                }
            }
            Self::Tail { job, lines } => {
                argv.extend(["tail".to_string(), job]);
                if let Some(lines) = lines {
                    argv.extend(["--lines".to_string(), lines.to_string()]);
                }
            }
            Self::Kill { job } => argv.extend(["kill".to_string(), job]),
        }
        argv
    }
}

#[derive(Clone, Debug, Subcommand)]
enum SidecarAction {
    /// Send one outbound Telegram message and bind this seat as the reply target.
    Telegram {
        /// Assert the derived sending seat, which inbound replies route back to.
        #[arg(long)]
        from: Option<String>,
        /// Literal message body, including values beginning with `-`.
        #[arg(long, allow_hyphen_values = true, conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the literal body from a path, or from stdin with `-`.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Correlation id. Omit to mint a UUIDv7.
        #[arg(long)]
        msg_id: Option<String>,
        /// Conversation override; absent uses the credential file's chat id.
        #[arg(long)]
        chat_id: Option<String>,
    },
    /// Start one background command under its own process group.
    BgStart {
        /// Stable job name.
        #[arg(long)]
        id: String,
        /// Human title.
        #[arg(long)]
        title: String,
        /// Command interpreted by `/bin/sh -c`.
        #[arg(long)]
        command: String,
        /// Seat receiving the completion turn.
        #[arg(long)]
        target: String,
    },
    /// Cancel one running background command.
    BgCancel {
        /// Stable job name.
        #[arg(long)]
        id: String,
        /// Seat receiving the cancellation turn.
        #[arg(long)]
        target: String,
    },
    /// Define one chore probe.
    ChoreAdd {
        /// Chore name.
        #[arg(long)]
        name: String,
        /// Shell probe.
        #[arg(long)]
        probe: String,
        /// Seat receiving reports.
        #[arg(long)]
        target: String,
    },
    /// Run every defined probe and report the delta.
    ChoreRun {
        /// Seat receiving the report.
        #[arg(long)]
        target: String,
    },
    /// Advance one pending baseline.
    ChoreAck {
        /// Chore name.
        #[arg(long)]
        name: String,
        /// Seat receiving confirmation.
        #[arg(long)]
        target: String,
    },
}

impl SidecarAction {
    /// Render the wire request the daemon's `/v1/sidecar` route decodes.
    async fn request(self, client: &DaemonClient) -> Result<Value, PijError> {
        Ok(match self {
            Self::Telegram {
                from,
                body,
                body_file,
                msg_id,
                chat_id,
            } => {
                let body = resolve_send_body(body, body_file)?;
                let from =
                    resolve_command_seat(client, "pij sidecar telegram", from.as_deref()).await?;
                let msg_id = msg_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
                json!({"sidecar": "telegram", "request": {"from": from, "body": body, "msg_id": msg_id, "chat_id": chat_id}})
            }
            Self::BgStart {
                id,
                title,
                command,
                target,
            } => {
                json!({"sidecar": "bg", "request": {"action": "start", "id": id, "title": title, "command": command, "target": target}})
            }
            Self::BgCancel { id, target } => {
                json!({"sidecar": "bg", "request": {"action": "cancel", "id": id, "target": target}})
            }
            Self::ChoreAdd {
                name,
                probe,
                target,
            } => {
                json!({"sidecar": "chore", "request": {"action": "add", "name": name, "probe": probe, "target": target}})
            }
            Self::ChoreRun { target } => {
                json!({"sidecar": "chore", "request": {"action": "run", "target": target}})
            }
            Self::ChoreAck { name, target } => {
                json!({"sidecar": "chore", "request": {"action": "ack", "name": name, "target": target}})
            }
        })
    }
}

#[derive(Clone, Debug, Subcommand)]
enum DaemonAction {
    /// Rebuild, drain, restart, verify health, and print the running version.
    Bounce,
}

#[derive(Clone, Debug, Subcommand)]
enum DoctorAction {
    /// Report cross-session inbound state for every discovered Claude home.
    #[command(name = "claude-inbound")]
    Inbound,
    /// Report pij SessionStart hook state for every discovered Claude home.
    #[command(name = "claude-hook")]
    Hook,
    /// Report pij's managed Claude statusline state for every discovered home.
    #[command(name = "claude-statusline")]
    Statusline,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Keep the sink alive through runtime teardown, including panic diagnostics.
    let _log = if matches!(&cli.command, Command::Daemon { action: None, .. }) {
        match daemon_log::DaemonLog::install() {
            Ok(log) => {
                raise_open_file_limit();
                Some(log)
            }
            Err(error) => {
                eprintln!("pij-rs: could not initialize daemon logging: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    run_with_runtime(cli)
}

/// A launchd-started daemon inherits a soft open-file limit of 256, and a busy
/// fleet exhausted it within minutes (2026-09-27: EMFILE froze every route).
/// Raise the soft limit ourselves so the daemon never depends on its launcher.
fn raise_open_file_limit() {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NOFILE) else {
        eprintln!("pij-rs: could not read the open-file limit; leaving it as is");
        return;
    };
    for target in open_file_targets(soft, hard) {
        if setrlimit(Resource::RLIMIT_NOFILE, target, hard).is_ok() {
            eprintln!("pij-rs: open-file limit raised {soft} -> {target}");
            return;
        }
    }
    if soft < OPEN_FILE_FLOOR {
        eprintln!("pij-rs: open-file limit stays at {soft}; the daemon may exhaust it");
    }
}

/// Below this, a daemon serving a fleet runs out of descriptors.
const OPEN_FILE_FLOOR: u64 = 10_240;

/// Soft limits to try, largest first, each within `hard`; empty when `soft` is
/// already enough. macOS rejects soft limits above kern.maxfilesperproc, hence
/// the fallback.
fn open_file_targets(soft: u64, hard: u64) -> Vec<u64> {
    let mut targets: Vec<u64> = [65_536, OPEN_FILE_FLOOR]
        .into_iter()
        .map(|target| target.min(hard))
        .filter(|&target| target > soft)
        .collect();
    targets.dedup();
    targets
}

#[tokio::main]
async fn run_with_runtime(cli: Cli) -> ExitCode {
    run(cli).await
}

async fn run(cli: Cli) -> ExitCode {
    if let Command::SpawnChild {
        status_file,
        log_file,
        command,
    } = &cli.command
    {
        return run_spawn_child(status_file, log_file, command);
    }
    if let Command::Send {
        command: Some(command),
        body,
        body_file,
        ..
    } = &cli.command
    {
        let result = if body_file.is_some() {
            Err("E-RS-CONTROL-BODY: --command and --body-file are mutually exclusive".to_string())
        } else {
            pij_core::control::validate_command(command, body.as_deref().unwrap_or_default())
        };
        if let Err(reason) = result {
            return emit(
                &Envelope::<Value>::refused("pij send", ErrorKind::Refused, reason),
                cli.json,
            );
        }
    }
    let hook_dir = default_state_dir();
    let state_dir =
        match resolve_state_dir(cli.state_dir.clone(), std::env::var_os("PIJ_RS_STATE_DIR")) {
            Ok(path) => path,
            Err(message) => return emit_config_error(&cli, message),
        };

    if let Command::Daemon {
        action: None,
        bind,
        offline,
    } = &cli.command
    {
        let bind = match resolve_bind_addr(bind.as_deref(), std::env::var_os("PIJ_RS_BIND")) {
            Ok(bind) => bind,
            Err(message) => return emit_config_error(&cli, message),
        };
        let retired_harnesses =
            match resolve_retired_harnesses(std::env::var_os("PIJ_RETIRED_HARNESSES")) {
                Ok(harnesses) => harnesses,
                Err(message) => return emit_config_error(&cli, message),
            };
        return run_daemon(&bind, state_dir, hook_dir, *offline, retired_harnesses).await;
    }
    if let Command::Daemon {
        action: Some(DaemonAction::Bounce),
        ..
    } = &cli.command
    {
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => {
                return emit(
                    &setup_refusal::<Value>(
                        "pij daemon bounce",
                        PijError::Adapter {
                            adapter: "pij daemon bounce".to_string(),
                            message: format!("could not read the current directory: {error}"),
                        },
                    ),
                    cli.json,
                );
            }
        };
        let repo = match bounce::reproducible_repo(&cwd) {
            Ok(repo) => repo,
            Err(error) => {
                return emit(
                    &setup_refusal::<Value>("pij daemon bounce", error),
                    cli.json,
                );
            }
        };
        return match bounce::run(&repo, &state_dir).await {
            Ok(report) => emit(&Envelope::ok("pij daemon bounce", report), cli.json),
            Err(error) => emit(
                &setup_refusal::<Value>("pij daemon bounce", error),
                cli.json,
            ),
        };
    }

    if let Command::FleetReport {
        folder,
        with_worktrees,
        since,
        until,
        harness,
        out,
        format,
        include_content,
        anonymise,
        prep_target,
        utc_offset,
        threads,
    } = &cli.command
    {
        let args = pij_cli::fleet_report::FleetArgs {
            folder: folder.clone(),
            with_worktrees: *with_worktrees,
            since: since.clone(),
            until: until.clone(),
            harness: harness.clone(),
            out: out.clone(),
            format: format.clone(),
            include_content: *include_content,
            anonymise: *anonymise,
            prep_target: prep_target.clone(),
            utc_offset: utc_offset.clone(),
            threads: *threads,
        };
        let response = pij_cli::fleet_report::run(args, &state_dir).await;
        if cli.json || !response.ok {
            return emit(&response, cli.json);
        }
        println!("{}", render_fleet_report(&response));
        return ExitCode::from(exit_code(&response));
    }

    if let Command::Doctor { action } = &cli.command {
        return match action {
            DoctorAction::Inbound => run_claude_inbound_doctor(cli.json),
            DoctorAction::Hook => run_claude_hook_doctor(cli.json, &hook_dir),
            DoctorAction::Statusline => run_claude_statusline_doctor(cli.json, &hook_dir),
        };
    }

    let prepared_spawn = match prepare_spawn_request(&cli.command) {
        Ok(request) => request,
        Err(error) => {
            return emit(&setup_refusal::<Value>("pij spawn", error), cli.json);
        }
    };
    let prepared_revive = match prepare_revive_request(&cli.command) {
        Ok(request) => request,
        Err(error) => {
            return emit(&setup_refusal::<Value>("pij revive", error), cli.json);
        }
    };

    let command_name = cli.command.name();
    let addr = match resolve_client_addr(cli.addr.as_deref(), std::env::var_os("PIJ_RS_ADDR")) {
        Ok(addr) => addr,
        Err(message) => return emit_config_error(&cli, message),
    };
    let client = match DaemonClient::new(&state_dir, &addr) {
        Ok(client) => client,
        Err(error) => {
            if matches!(cli.command, Command::CommitTrailers) {
                return commit_trailers::fail(error);
            }
            return emit(&setup_refusal::<Value>(command_name, error), cli.json);
        }
    };

    match cli.command {
        Command::Daemon { .. } => unreachable!("daemon returned before client construction"),
        Command::Doctor { .. } => unreachable!("doctor returned before client construction"),
        Command::FleetReport { .. } => {
            unreachable!("fleet-report returned before client construction")
        }
        Command::SpawnChild { .. } => {
            unreachable!("spawn child returned before client construction")
        }
        Command::Ping => emit(&client.ping().await, cli.json),
        Command::Sidecar { action } => {
            let request = match action.request(&client).await {
                Ok(request) => request,
                Err(error) => {
                    return emit(&setup_refusal::<Value>("pij sidecar", error), cli.json);
                }
            };
            emit(&client.sidecar(&request).await, cli.json)
        }
        Command::Register {
            supersedes,
            id,
            harness,
            folder,
            extension_build,
            extension_path,
            pane,
            pid,
            proc_start,
            spawn_id,
            model,
            provider,
            effort,
            parent,
            role,
            relay,
        } => {
            let registration = Registration {
                supersedes: supersedes.map(Into::into),
                id: id.into(),
                harness,
                folder,
                extension_build,
                extension_path,
                pane,
                pid,
                proc_start,
                spawn_id,
                model,
                provider,
                effort,
                parent: parent.map(Into::into),
                role,
                relay,
            };
            if let Err(reason) = registration.validate() {
                return emit(
                    &Envelope::<Value>::refused("pij register", ErrorKind::Refused, reason),
                    cli.json,
                );
            }
            emit(&client.register(&registration).await, cli.json)
        }
        Command::Adopt {
            pane,
            harness,
            parent,
            role,
            extension_build,
            extension_path,
            harness_session,
            reclaim,
        } => {
            warn_caller_claude_home(&harness);
            let mut argv = adopt_argv(
                pane.or_else(|| std::env::var("TMUX_PANE").ok()).as_deref(),
                &harness,
                parent
                    .or_else(|| std::env::var("PIJ_PARENT_ID").ok())
                    .filter(|_| reclaim.is_none())
                    .as_deref(),
            );
            for (flag, value) in [
                ("--harness-session", harness_session),
                ("--reclaim", reclaim),
            ] {
                if let Some(value) = value {
                    argv.extend([flag.to_string(), value]);
                }
            }
            let request = IdentityRequest {
                argv,
                caller: Some(caller_context()),
                role,
                extension_build,
                extension_path,
                ..IdentityRequest::default()
            };
            emit(&client.adopt(&request).await, cli.json)
        }
        Command::Whoami { seat, pane } => {
            let response = client.whoami(&identity_request(seat, pane)).await;
            if cli.json {
                emit(&response, true)
            } else {
                println!("{}", render_whoami(&response));
                ExitCode::from(exit_code(&response))
            }
        }
        Command::CommitTrailers => {
            commit_trailers::run(&client, &identity_request(None, None)).await
        }
        Command::Phonehome { seat, pane } => emit(
            &client.phonehome(&identity_request(seat, pane)).await,
            cli.json,
        ),
        Command::Spawn { .. } => {
            let response = client
                .spawn(prepared_spawn.as_ref().expect("spawn request was prepared"))
                .await;
            emit(&spawn_cli_response(response), cli.json)
        }
        Command::Revive { .. } => emit(
            &client
                .revive(
                    prepared_revive
                        .as_ref()
                        .expect("revive request was prepared"),
                )
                .await,
            cli.json,
        ),
        Command::Send {
            from,
            to,
            body,
            body_file,
            command,
            msg_id,
            in_reply_to,
            fyi,
            force,
            reason,
        } => {
            let acting = match resolve_command_seat(&client, "pij send", from.as_deref()).await {
                Ok(seat) => seat,
                Err(error) => return emit(&setup_refusal::<Value>("pij send", error), cli.json),
            };
            let body = if command.is_some() {
                String::new()
            } else {
                match resolve_send_body(body, body_file) {
                    Ok(body) => body,
                    Err(error) => {
                        return emit(&setup_refusal::<Value>("pij send", error), cli.json);
                    }
                }
            };
            let request = SendRequest {
                from: acting,
                to,
                body,
                msg_id: msg_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
                in_reply_to,
                command,
                fyi,
                force,
                reason,
            };
            let mut response = if request.command.is_some() {
                client.send_control(&request, &caller_context()).await
            } else {
                client.send(&request).await
            };
            if response.ok {
                response.meta = Some(format!("from {}; msg_id {}", request.from, request.msg_id));
                // This is a CLI-derived receipt, not an unchanged daemon reply.
                response.raw_json = None;
            }
            if !cli.json
                && let Some(held) = held_fyi_output(&response, &request.msg_id)
            {
                println!("{held}");
                return ExitCode::SUCCESS;
            }
            emit(&response, cli.json)
        }
        Command::Activity {
            seat,
            pane,
            native_session,
            state,
        } => {
            let response = client
                .activity(&serde_json::json!({
                    "seat": seat,
                    "pane": pane,
                    "native_session": native_session,
                    "state": state,
                }))
                .await;
            if cli.json || !response.ok {
                return emit(&response, cli.json);
            }
            ExitCode::SUCCESS
        }
        Command::FyiClaim {
            seat,
            pane,
            native_session,
            via,
        } => {
            let response = client
                .fyi_claim(&serde_json::json!({
                    "seat": seat,
                    "pane": pane,
                    "native_session": native_session,
                    "via": via,
                }))
                .await;
            if cli.json || !response.ok {
                return emit(&response, cli.json);
            }
            // Raw block for a shell hook: nothing at all when nothing is held.
            if let Some(block) = response
                .data
                .as_ref()
                .and_then(|data| data["block"].as_str())
                .filter(|block| !block.is_empty())
            {
                println!("{block}");
            }
            ExitCode::SUCCESS
        }
        Command::FyiRead { seat, claimed_at } => {
            let response = client
                .fyi_read(&serde_json::json!({"seat": seat, "claimed_at_ms": claimed_at}))
                .await;
            if cli.json || !response.ok {
                return emit(&response, cli.json);
            }
            match response
                .data
                .as_ref()
                .and_then(|data| data["block"].as_str())
                .filter(|block| !block.is_empty())
            {
                Some(block) => println!("{block}"),
                None => println!("pij fyi-read: no FYIs were delivered to {seat} at {claimed_at}"),
            }
            ExitCode::SUCCESS
        }
        Command::CompactSelf => {
            let acting = match resolve_command_seat(&client, "pij compact-self", None).await {
                Ok(seat) => seat,
                Err(error) => {
                    return emit(&setup_refusal::<Value>("pij compact-self", error), cli.json);
                }
            };
            let request = SendRequest {
                from: acting.clone(),
                to: pij_core::model::Destination::local(acting),
                body: String::new(),
                command: Some("compact".to_string()),
                msg_id: uuid::Uuid::now_v7().to_string(),
                in_reply_to: None,
                fyi: false,
                force: false,
                reason: None,
            };
            let mut response = client.send_control(&request, &caller_context()).await;
            response.command = "pij compact-self".to_string();
            response.raw_json = None;
            emit(&response, cli.json)
        }
        Command::Inbox {
            action,
            seat,
            wait,
            peek,
        } => {
            if let Some(InboxAction::Release {
                seat: target,
                job,
                evidence,
            }) = action
            {
                if seat.is_some() || wait || peek {
                    return emit(
                        &setup_refusal::<Value>(
                            "pij inbox release",
                            PijError::Adapter {
                                adapter: "pij inbox".into(),
                                message: "release cannot be combined with inbox read flags".into(),
                            },
                        ),
                        cli.json,
                    );
                }
                return emit(
                    &client
                        .release_inbox(&target.into(), job, &evidence, &caller_context())
                        .await,
                    cli.json,
                );
            }
            let explicit = seat.map(SeatId::from);
            let target = if peek {
                match explicit {
                    Some(target) => target,
                    None => match resolve_command_seat(&client, "pij inbox", None).await {
                        Ok(seat) => seat,
                        Err(error) => {
                            return emit(&setup_refusal::<Value>("pij inbox", error), cli.json);
                        }
                    },
                }
            } else {
                let acting = match resolve_command_seat(&client, "pij inbox", None).await {
                    Ok(seat) => seat,
                    Err(error) => {
                        return emit(&setup_refusal::<Value>("pij inbox", error), cli.json);
                    }
                };
                let target = explicit.unwrap_or_else(|| acting.clone());
                if target != acting {
                    return emit(
                        &setup_refusal::<Value>(
                            "pij inbox",
                            PijError::Adapter {
                                adapter: "pij inbox".to_string(),
                                message: format!(
                                    "the acting seat is `{acting}` but --seat names `{target}`; destructive inbox reads cannot claim or acknowledge another seat's mail. Use --peek for a non-destructive cross-seat read"
                                ),
                            },
                        ),
                        cli.json,
                    );
                }
                target
            };
            if peek {
                emit(&client.peek_inbox(&target).await, cli.json)
            } else {
                let mut caller = caller_context();
                caller.session_id = Some(target.to_string());
                emit(&client.inbox(&target, wait, &caller).await, cli.json)
            }
        }

        Command::Bg { action } => {
            let response = client.bg(&action.argv(), &caller_context()).await;
            if !cli.json
                && response.ok
                && let Some(line) = response
                    .data
                    .as_ref()
                    .and_then(|data| data["line"].as_str())
            {
                println!("{line}");
                return ExitCode::from(exit_code(&response));
            }
            emit(&response, cli.json)
        }
        Command::Report { seat, args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            let identity = match identity::request_from_environment(
                std::env::var_os("PIJ_SESSION_ID"),
                std::env::var_os("TMUX_PANE"),
            ) {
                Ok(identity) => identity,
                Err(error) => {
                    return emit(
                        &setup_refusal::<Value>(
                            "pij report",
                            PijError::Adapter {
                                adapter: "pij report".to_string(),
                                message: error.to_string(),
                            },
                        ),
                        as_json,
                    );
                }
            };
            let caller = CallerContext {
                session_id: identity.seat,
                pane: identity.pane,
                ..CallerContext::default()
            };
            let asserted = seat.map(SeatId::from);
            // The verb the caller typed is re-attached, because the daemon's
            // grammar parses the same argv the TypeScript CLI receives.
            let mut argv = vec!["report".to_string()];
            argv.extend(args);
            emit(
                &client.report(asserted.as_ref(), &argv, &caller).await,
                as_json,
            )
        }
        Command::Role { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            emit(&client.role(&args, &caller_context()).await, as_json)
        }
        Command::Close { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            emit(&client.close(&args, &caller_context()).await, as_json)
        }
        Command::Reap { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            emit(&client.reap(&args, &caller_context()).await, as_json)
        }
        Command::Anomalies { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            emit(
                &pij_cli::anomalies::anomalies(&client, &args, &caller_context()).await,
                as_json,
            )
        }
        Command::Decisions { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            emit(
                &pij_cli::decisions::decisions(&client, &args, &caller_context()).await,
                as_json,
            )
        }
        Command::Answer { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            emit(
                &pij_cli::decisions::answer(&client, &args, &caller_context()).await,
                as_json,
            )
        }
        Command::Project { args }
        | Command::Stream { args }
        | Command::Fence { args }
        | Command::Dispatch { args }
        | Command::Ack { args }
        | Command::Canary { args }
        | Command::Attest { args }
        | Command::Task { args }
        | Command::Node { args }
        | Command::Orchestration { args }
        | Command::Spine { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            let family = command_name
                .strip_prefix("pij ")
                .expect("governance command prefix");
            emit(
                &pij_cli::governance::governance(&client, family, &args, &caller_context()).await,
                as_json,
            )
        }
        Command::List { args } => {
            let as_json = cli.json || args.iter().any(|arg| arg == "--json");
            let response = if args.iter().all(|arg| arg == "--json") {
                client.list_sized().await
            } else {
                client.list_scoped(&args, &caller_context()).await
            };
            if as_json || !response.ok {
                return emit(&response, as_json);
            }
            let data = response
                .raw_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .map(|envelope| envelope["data"].clone())
                .or_else(|| serde_json::to_value(&response.data).ok())
                .unwrap_or_default();
            println!("{}", pij_cli::roster::render_table(&data));
            ExitCode::SUCCESS
        }
        Command::State { id } => {
            let response = client.state(&id).await;
            if cli.json {
                emit(&response, true)
            } else {
                println!("{}", render_state(&response, false));
                ExitCode::from(exit_code(&response))
            }
        }
        Command::Tail { since } => {
            let since = match cursor_map(since) {
                Ok(since) => since,
                Err(error) => {
                    return emit(&setup_refusal::<Value>("pij tail", error), cli.json);
                }
            };
            match client.tail(&since).await {
                Err(envelope) => emit(&envelope, cli.json),
                Ok(mut stream) => loop {
                    tokio::select! {
                        frame = stream.next_frame() => match frame {
                            Ok(Some(frame)) => println!("{}", render_frame(&frame, cli.json)),
                            Ok(None) => return ExitCode::SUCCESS,
                            Err(error) => {
                                let envelope = Envelope::<Value>::refused(
                                    "pij tail",
                                    error.kind,
                                    error.message,
                                );
                                return emit(&envelope, cli.json);
                            }
                        },
                        signal = tokio::signal::ctrl_c() => {
                            return if signal.is_ok() {
                                ExitCode::SUCCESS
                            } else {
                                emit(
                                    &Envelope::<Value>::refused(
                                        "pij tail",
                                        ErrorKind::Adapter,
                                        "could not install the interrupt handler",
                                    ),
                                    cli.json,
                                )
                            };
                        }
                    }
                },
            }
        }
    }
}

const DEFAULT_DAEMON_ADDR: &str = "127.0.0.1:7461";

/// Resolve runtime controls exactly once at the binary edge. Explicit CLI flags
/// win over their environment variables; environment wins over the default.
fn resolve_state_dir(flag: Option<PathBuf>, env: Option<OsString>) -> Result<PathBuf, String> {
    let (path, source) = match flag {
        Some(path) => (path, "--state-dir"),
        None => match env {
            Some(value) => (PathBuf::from(value), "PIJ_RS_STATE_DIR"),
            None => return Ok(default_state_dir()),
        },
    };
    let rendered = path.as_os_str().to_string_lossy();
    if rendered.trim().is_empty() {
        return Err(format!(
            "{source} has malformed value {rendered:?}: value must not be empty"
        ));
    }
    Ok(path)
}

fn resolve_retired_harnesses(env: Option<OsString>) -> Result<Vec<Harness>, String> {
    let Some(value) = env else {
        return Ok(Vec::new());
    };
    let value = value
        .into_string()
        .map_err(|_| "PIJ_RETIRED_HARNESSES must be valid UTF-8".to_string())?;
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut retired = Vec::new();
    for name in value.split(',').map(str::trim) {
        let harness =
            parse_harness(name).map_err(|error| format!("PIJ_RETIRED_HARNESSES: {error}"))?;
        if !retired.contains(&harness) {
            retired.push(harness);
        }
    }
    Ok(retired)
}

fn resolve_client_addr(flag: Option<&str>, env: Option<OsString>) -> Result<String, String> {
    let (value, source) = configured_string(flag, env, "--addr", "PIJ_RS_ADDR")?;
    validate_host_port(&value, source)?;
    Ok(value)
}

fn resolve_bind_addr(flag: Option<&str>, env: Option<OsString>) -> Result<String, String> {
    let (value, source) = configured_string(flag, env, "--bind", "PIJ_RS_BIND")?;
    let address = value.parse::<SocketAddr>().map_err(|_| {
        format!("{source} has malformed value {value:?}: expected IP:port with port 1..65535")
    })?;
    if address.port() == 0 {
        return Err(format!(
            "{source} has malformed value {value:?}: expected IP:port with port 1..65535"
        ));
    }
    Ok(value)
}

fn configured_string(
    flag: Option<&str>,
    env: Option<OsString>,
    flag_name: &'static str,
    env_name: &'static str,
) -> Result<(String, &'static str), String> {
    if let Some(value) = flag {
        if value.trim().is_empty() {
            return Err(format!(
                "{flag_name} has malformed value {value:?}: value must not be empty"
            ));
        }
        return Ok((value.to_string(), flag_name));
    }
    let Some(value) = env else {
        return Ok((DEFAULT_DAEMON_ADDR.to_string(), "default daemon address"));
    };
    let rendered = value.to_string_lossy();
    if rendered.trim().is_empty() {
        return Err(format!(
            "{env_name} has malformed value {rendered:?}: value must not be empty"
        ));
    }
    let value = value.into_string().map_err(|value| {
        format!(
            "{env_name} has malformed value {:?}: value must be valid UTF-8",
            value.to_string_lossy()
        )
    })?;
    Ok((value, env_name))
}

fn validate_host_port(value: &str, source: &str) -> Result<(), String> {
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let Some((host, port)) = rest.split_once("]:") else {
            return Err(malformed_addr(source, value, "host:port"));
        };
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(malformed_addr(source, value, "host:port"));
        }
        (host, port)
    } else {
        let Some((host, port)) = value.rsplit_once(':') else {
            return Err(malformed_addr(source, value, "host:port"));
        };
        if host.contains(':') || host.chars().any(char::is_whitespace) {
            return Err(malformed_addr(source, value, "host:port"));
        }
        (host, port)
    };
    let valid_port = port.parse::<u16>().is_ok_and(|port| port != 0);
    if host.is_empty() || !valid_port {
        return Err(malformed_addr(source, value, "host:port"));
    }
    Ok(())
}

fn malformed_addr(source: &str, value: &str, expected: &str) -> String {
    format!("{source} has malformed value {value:?}: expected {expected} with port 1..65535")
}

fn emit_config_error(cli: &Cli, message: String) -> ExitCode {
    if matches!(cli.command, Command::CommitTrailers) {
        return commit_trailers::fail(message);
    }
    emit(
        &setup_refusal::<Value>(
            cli.command.name(),
            PijError::Adapter {
                adapter: "configuration".to_string(),
                message,
            },
        ),
        cli.json,
    )
}

/// Build the command line the daemon will parse.
///
/// Synthesised rather than hand-built into a struct SO THAT there is exactly one
/// parser. The generation shim forwards the operator's literal argv; this CLI
/// sends the argv it would have typed. One parser cannot drift from itself.
///
/// With no pane this becomes the PANELESS form — which the daemon refuses unless
/// a corroborable process identity travels with it. That refusal is deliberate
/// (rs admits no seat without bind evidence), and building the form anyway is
/// what makes the refusal say so, instead of this CLI inventing its own message.
fn adopt_argv(pane: Option<&str>, harness: &str, parent: Option<&str>) -> Vec<String> {
    let mut argv = match pane {
        Some(pane) => vec!["adopt".to_string(), pane.to_string()],
        None => vec!["inbox".to_string(), "register".to_string()],
    };
    argv.push("--harness".to_string());
    argv.push(harness.to_string());
    if let Some(parent) = parent {
        argv.push("--parent".to_string());
        argv.push(parent.to_string());
    }
    argv
}

/// The caller's environment, read WHERE IT IS.
///
/// This is the whole reason the `pij-rs` verbs exist alongside the routed ones:
/// the daemon is a different process and can see none of this. Every field is a
/// CLAIM that the daemon validates or refuses — nothing here is trusted by being
/// sent.
///
/// `pid`/`proc_start` are deliberately NOT filled from this process. It is a
/// short-lived child of the seat, so its pid would corroborate for an instant and
/// then name a dead process forever after — a worse answer than no answer.
fn warn_caller_claude_home(harness: &str) {
    if let Some(warning) = caller_claude_warning(harness, std::env::var_os("CLAUDE_CONFIG_DIR")) {
        eprintln!("pij-rs warning: {warning}");
    }
}

fn caller_claude_warning(harness: &str, config_dir: Option<OsString>) -> Option<String> {
    if harness != Harness::Claude.as_str() {
        return None;
    }
    let home = config_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    let state = inspect_claude_inbound(std::slice::from_ref(&home))
        .into_iter()
        .next()
        .expect("one requested Claude home yields one state");
    (state.error.is_some() || state.current.as_deref() != Some("accept")).then(|| {
        format!(
            "caller CLAUDE_CONFIG_DIR {} does not set crossSessionInbound=accept; daemon registration writes only daemon-derived homes",
            home.display()
        )
    })
}

fn caller_context() -> CallerContext {
    let read = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
    CallerContext {
        session_id: read("PIJ_SESSION_ID"),
        harness_session: read("HARNESS_SESSION_ID"),
        pane: read("TMUX_PANE"),
        parent: read("PIJ_PARENT_ID"),
        claude_session: read("CLAUDE_CODE_SESSION_ID"),
        copilot_session: read("COPILOT_AGENT_SESSION_ID"),
        codex_session: read("CODEX_THREAD_ID"),
        cwd: std::env::current_dir()
            .ok()
            .map(|cwd| cwd.display().to_string()),
        pid: None,
        proc_start: None,
    }
}

/// The body the read verbs send. Explicit flags win over the ambient
/// environment, so an operator can always ask about a seat that is not theirs —
/// and the daemon still refuses if the answer would be an impersonation.
fn identity_request(seat: Option<String>, pane: Option<String>) -> IdentityRequest {
    IdentityRequest {
        seat,
        pane,
        caller: Some(caller_context()),
        ..IdentityRequest::default()
    }
}

async fn resolve_command_seat(
    client: &DaemonClient,
    command: &str,
    asserted: Option<&str>,
) -> Result<SeatId, PijError> {
    let derived = identity::resolve_acting_seat(
        std::env::var_os("PIJ_SESSION_ID"),
        std::env::var_os("TMUX_PANE"),
        client,
    )
    .await
    .map_err(|refusal| PijError::Adapter {
        adapter: command.to_string(),
        message: refusal.to_string(),
    })?;
    if let Some(asserted) = asserted
        && asserted != derived.as_str()
    {
        return Err(PijError::Adapter {
            adapter: command.to_string(),
            message: format!(
                "{command} asserts `{asserted}` but the acting seat resolves to `{derived}`; the assertion cannot override pane/environment identity"
            ),
        });
    }
    Ok(derived)
}

fn resolve_send_body(body: Option<String>, body_file: Option<String>) -> Result<String, PijError> {
    match (body, body_file) {
        (Some(body), None) => Ok(body),
        (None, Some(path)) if path == "-" => {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(|error| PijError::Adapter {
                    adapter: "pij send".to_string(),
                    message: format!("could not read --body-file - from stdin: {error}"),
                })?;
            Ok(body)
        }
        (None, Some(path)) => std::fs::read_to_string(&path).map_err(|error| PijError::Adapter {
            adapter: "pij send".to_string(),
            message: format!("could not read --body-file `{path}`: {error}"),
        }),
        (None, None) => Err(PijError::Adapter {
            adapter: "pij send".to_string(),
            message: "missing message body: pass --body <text> or --body-file <path|->".to_string(),
        }),
        (Some(_), Some(_)) => Err(PijError::Adapter {
            adapter: "pij send".to_string(),
            message: "--body and --body-file are mutually exclusive".to_string(),
        }),
    }
}

#[derive(serde::Serialize)]
struct ClaudeInboundDoctorRow {
    home: PathBuf,
    current: Option<String>,
    required: &'static str,
    status: &'static str,
    error: Option<String>,
}

fn run_claude_inbound_doctor(json: bool) -> ExitCode {
    let rows = inspect_claude_inbound(&claude_homes())
        .into_iter()
        .map(|report| {
            let status = if report.error.is_some() {
                "error"
            } else if report.current.as_deref() == Some("accept") {
                "accept"
            } else if report.current.is_some() {
                "not-accept"
            } else if report.file_exists {
                "key-missing"
            } else {
                "file-missing"
            };
            ClaudeInboundDoctorRow {
                home: report.home,
                current: report.current,
                required: "accept",
                status,
                error: report.error,
            }
        })
        .collect::<Vec<_>>();
    emit(&Envelope::ok("pij doctor claude-inbound", rows), json)
}

#[derive(serde::Serialize)]
struct ClaudeHookDoctorRow {
    home: PathBuf,
    configured: Option<String>,
    expected: PathBuf,
    script_exists: bool,
    status: &'static str,
    error: Option<String>,
}

fn run_claude_hook_doctor(json: bool, state_dir: &Path) -> ExitCode {
    let expected = state_dir.join("claude-session-start-pij.sh");
    let script_exists = expected.is_file();
    let rows = inspect_claude_session_start_hook(&claude_homes(), &expected)
        .into_iter()
        .map(|report| {
            let status = if report.error.is_some() {
                "error"
            } else if report.installed && script_exists {
                "installed"
            } else if report.installed {
                "script-missing"
            } else if report.command.is_some() {
                "wrong-command"
            } else if report.file_exists {
                "hook-missing"
            } else {
                "file-missing"
            };
            ClaudeHookDoctorRow {
                home: report.home,
                configured: report.command,
                expected: expected.clone(),
                script_exists,
                status,
                error: report.error,
            }
        })
        .collect::<Vec<_>>();
    emit(&Envelope::ok("pij doctor claude-hook", rows), json)
}
#[derive(serde::Serialize)]
struct ClaudeStatuslineDoctorRow {
    home: PathBuf,
    configured: Option<String>,
    expected: PathBuf,
    script_exists: bool,
    status: &'static str,
    error: Option<String>,
}

fn run_claude_statusline_doctor(json: bool, state_dir: &Path) -> ExitCode {
    let expected = state_dir.join("claude-statusline-pij.sh");
    let script_exists = expected.is_file();
    let rows = inspect_claude_statusline(&claude_homes(), &expected)
        .into_iter()
        .map(|report| {
            let status = if report.error.is_some() {
                "error"
            } else if report.installed && script_exists {
                "installed"
            } else if report.installed {
                "script-missing"
            } else if report.command.is_some() {
                "wrong-command"
            } else if report.file_exists {
                "statusline-missing"
            } else {
                "file-missing"
            };
            ClaudeStatuslineDoctorRow {
                home: report.home,
                configured: report.command,
                expected: expected.clone(),
                script_exists,
                status,
                error: report.error,
            }
        })
        .collect::<Vec<_>>();
    emit(&Envelope::ok("pij doctor claude-statusline", rows), json)
}

async fn run_daemon(
    bind: &str,
    state_dir: PathBuf,
    hook_dir: PathBuf,
    offline: bool,
    retired_harnesses: Vec<Harness>,
) -> ExitCode {
    let homes = claude_homes();
    let inbound_reports = ensure_claude_inbound_accept(&homes);
    let hook_script = install_claude_session_start_script(&hook_dir)
        .map_err(|error| format!("could not install Claude SessionStart hook script: {error}"));
    let hook_reports = hook_script
        .as_ref()
        .map(|script| ensure_claude_session_start_hook(&homes, script))
        .unwrap_or_default();
    // Plan 158: typed prompts carry held FYIs in, in every Claude home.
    let prompt_hook_script = install_claude_user_prompt_submit_script(&hook_dir)
        .map_err(|error| format!("could not install Claude UserPromptSubmit hook script: {error}"));
    let prompt_hook_reports = prompt_hook_script
        .as_ref()
        .map(|script| ensure_claude_user_prompt_submit_hook(&homes, script))
        .unwrap_or_default();
    // Plan 157: Claude's turn ending publishes the seat idle, in every Claude home.
    let stop_hook_script = install_claude_stop_script(&hook_dir)
        .map_err(|error| format!("could not install Claude Stop hook script: {error}"));
    let stop_hook_reports = stop_hook_script
        .as_ref()
        .map(|script| ensure_claude_stop_hook(&homes, script))
        .unwrap_or_default();
    // Review N1: a turn ending on an API error fires StopFailure, not Stop.
    let stop_failure_reports = stop_hook_script
        .as_ref()
        .map(|script| ensure_claude_stop_failure_hook(&homes, script))
        .unwrap_or_default();
    let statusline_script = install_claude_statusline_script(&hook_dir)
        .map_err(|error| format!("could not install Claude statusline script: {error}"));
    let statusline_reports = statusline_script
        .as_ref()
        .map(|script| ensure_claude_statusline(&homes, script))
        .unwrap_or_default();
    // Plan 156 rule 5: Copilot's status line is pij-managed too, when Copilot is.
    let copilot_statusline_reports = copilot_home()
        .and_then(|home| {
            install_copilot_statusline_script(&hook_dir)
                .map_err(|error| {
                    eprintln!(
                        "pij-rs warning: could not install Copilot statusline script: {error}"
                    );
                })
                .ok()
                .map(|script| ensure_copilot_statusline(&home, &script))
        })
        .unwrap_or_default();
    let mut config = daemon_config(bind, &state_dir.join("pij.sqlite"), offline);
    config.retired_harnesses = retired_harnesses;
    match pij_daemon::boot(&config, state_dir).await {
        Ok(daemon) => {
            for (kind, label, reports) in [
                (
                    "config.claude-inbound-ensured",
                    "Claude inbound setting",
                    inbound_reports.as_slice(),
                ),
                (
                    "config.claude-hook-ensured",
                    "Claude SessionStart hook",
                    hook_reports.as_slice(),
                ),
                (
                    "config.claude-prompt-hook-ensured",
                    "Claude UserPromptSubmit hook",
                    prompt_hook_reports.as_slice(),
                ),
                (
                    "config.claude-stop-hook-ensured",
                    "Claude Stop hook",
                    stop_hook_reports.as_slice(),
                ),
                (
                    "config.claude-stop-failure-hook-ensured",
                    "Claude StopFailure hook",
                    stop_failure_reports.as_slice(),
                ),
                (
                    "config.claude-statusline-ensured",
                    "Claude statusline",
                    statusline_reports.as_slice(),
                ),
                (
                    "config.copilot-statusline-ensured",
                    "Copilot statusline",
                    copilot_statusline_reports.as_slice(),
                ),
            ] {
                for report in reports.iter().filter(|report| report.changed) {
                    let at = match system_time_ms() {
                        Ok(at) => at,
                        Err(error) => {
                            eprintln!(
                                "pij-rs warning: {label} changed but its config event timestamp failed: {error}"
                            );
                            continue;
                        }
                    };
                    if let Err(error) = daemon
                        .publish_event(Event {
                            seq: None,
                            v: wire::EVENT_VERSION,
                            at,
                            kind: kind.to_string(),
                            seat: None,
                            payload: json!({
                                "home": report.home,
                                "before": report.before,
                                "after": report.after,
                            })
                            .to_string(),
                        })
                        .await
                    {
                        eprintln!(
                            "pij-rs warning: {label} changed but its config event could not be published: {error}"
                        );
                    }
                }
            }
            for (label, reports) in [
                ("inbound", &inbound_reports),
                ("hook", &hook_reports),
                ("prompt hook", &prompt_hook_reports),
                ("stop hook", &stop_hook_reports),
                ("stop-failure hook", &stop_failure_reports),
                ("statusline", &statusline_reports),
                ("copilot statusline", &copilot_statusline_reports),
            ] {
                let homes = reports
                    .iter()
                    .map(|report| {
                        let outcome = if report.error.is_some() {
                            "error"
                        } else if report.changed {
                            "touched"
                        } else {
                            "skipped"
                        };
                        format!("{}={outcome}", report.home.display())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "pij-rs claude {label}: {}",
                    if homes.is_empty() { "no homes" } else { &homes }
                );
                for report in reports.iter().filter(|report| report.error.is_some()) {
                    eprintln!(
                        "pij-rs claude {label}: {}: {}",
                        report.home.display(),
                        report.error.as_deref().unwrap_or("unknown error")
                    );
                }
            }
            if let Err(error) = hook_script {
                eprintln!("pij-rs claude hook: {error}");
            }
            if let Err(error) = prompt_hook_script {
                eprintln!("pij-rs claude prompt hook: {error}");
            }
            if let Err(error) = stop_hook_script {
                eprintln!("pij-rs claude stop hook: {error}");
            }
            if let Err(error) = statusline_script {
                eprintln!("pij-rs claude statusline: {error}");
            }
            println!(
                "pij-rs daemon: listening on {} · key {} (0600) · offline={}",
                daemon.addr,
                daemon.key.path.display(),
                config.is_fully_offline()
            );
            let _ = tokio::signal::ctrl_c().await;
            println!("pij-rs daemon: shutting down");
            match daemon.shutdown().await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("pij-rs: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            eprintln!("pij-rs: {error}");
            ExitCode::FAILURE
        }
    }
}

fn system_time_ms() -> Result<u64, PijError> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| PijError::Adapter {
            adapter: "pij daemon".to_string(),
            message: format!("system clock is before Unix epoch: {error}"),
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| PijError::Adapter {
        adapter: "pij daemon".to_string(),
        message: "system clock milliseconds exceed u64".to_string(),
    })
}

/// Is this send receipt an FYI held for the recipient's next turn?
/// The human line for a held FYI, with the receipt's warning (plan 159) on
/// the next line. `None` when the send was not held as an FYI.
fn held_fyi_output(response: &Envelope<pij_core::model::Receipt>, msg_id: &str) -> Option<String> {
    let receipt = response.data.as_ref().filter(|_| response.ok)?;
    if !matches!(&receipt.outcome, pij_core::model::DeliveryOutcome::Held { reason } if reason == "fyi")
    {
        return None;
    }
    let mut line = format!("pij send: held (fyi) — {msg_id}");
    if let Some(warning) = &receipt.warning {
        line.push('\n');
        line.push_str(warning);
    }
    Some(line)
}

fn emit<T: serde::Serialize>(envelope: &Envelope<T>, json: bool) -> ExitCode {
    if json && let Some(raw) = &envelope.raw_json {
        print!("{raw}");
        if !raw.ends_with('\n') {
            println!();
        }
    } else {
        println!("{}", render(envelope, json));
    }
    ExitCode::from(exit_code(envelope))
}

/// The human summary of a written fleet report.
fn render_fleet_report(envelope: &Envelope<Value>) -> String {
    let data = envelope.data.clone().unwrap_or_default();
    let num = |key: &str| data[key].as_f64().unwrap_or(0.0);
    let mut text = format!(
        "pij fleet-report: wrote {}\n  open {}\n  {} calls, {} turns, {} sessions ({} unseated) over {} folder(s)\n  \
         ${:.0} at list price; cached reads {:.1}%, status turns {:.1}%; {} of {} idle cold wakes avoidable",
        data["out"].as_str().unwrap_or_default(),
        data["page"].as_str().unwrap_or_default(),
        num("calls"),
        num("turns"),
        num("sessions"),
        num("unseated_sessions"),
        num("folders"),
        num("total_usd"),
        num("reads_share"),
        num("status_share"),
        num("avoidable_cold_wakes"),
        num("idle_cold_wakes"),
    );
    for warning in data["warnings"].as_array().into_iter().flatten() {
        let _ = write!(
            text,
            "\n  warning: {}",
            warning.as_str().unwrap_or_default()
        );
    }
    text
}

fn render_whoami(envelope: &Envelope<pij_core::model::SeatDescriptor>) -> String {
    let mut output = render(envelope, false);
    if envelope.ok
        && let Some(seat) = &envelope.data
    {
        let _ = write!(
            output,
            "\nextension: {} ({})",
            seat.extension_build.as_deref().unwrap_or("unknown"),
            seat.extension_path.as_deref().unwrap_or("unknown")
        );
    }
    output
}

fn render_state(envelope: &Envelope<pij_daemon::http::StateCard>, json: bool) -> String {
    let mut output = render(envelope, json);
    if !json
        && envelope.ok
        && let Some(card) = &envelope.data
    {
        let _ = write!(output, "\nheld: {}", card.held.len());
        if let Some(first) = card.held.iter().min_by_key(|hold| hold.since_ms) {
            let _ = write!(output, " ({} since {} ms)", first.reason, first.since_ms);
        }
        for deferred in &card.delivery_deferrals {
            let _ = write!(
                output,
                "\ndelivery deferred: {} ×{} since {} ms",
                deferred.reason, deferred.count, deferred.since_ms,
            );
        }
        if let Some(reason) = &card.native_receiver_reason {
            let _ = write!(output, "\nnative receiver: {reason}");
        }
        if card.pending_fyis > 0 {
            let _ = write!(output, "\nFYIs held: {}", card.pending_fyis);
        }
        if let Some(block) = &card.session_status {
            let _ = write!(output, "\nsession: {}", render_session_status(block));
        }
        for line in &card.size_lines {
            let _ = write!(output, "\n{line}");
        }
    }
    output
}

/// One line of session facts: model, context used of window, cache state.
fn render_session_status(block: &pij_core::session_status::SessionStatusBlock) -> String {
    use pij_core::session_status::{CacheState, Fact, SessionStatusBlock};
    fn known<T: std::fmt::Display>(fact: &Fact<T>) -> String {
        fact.value()
            .map_or_else(|| "unknown".to_string(), ToString::to_string)
    }
    match block {
        SessionStatusBlock::Known {
            status,
            cache_state,
            elapsed_ms,
        } => {
            let cache = match cache_state.value() {
                Some(CacheState::Warm { expires_in_ms }) => {
                    format!("warm ({}s left)", expires_in_ms / 1_000)
                }
                Some(CacheState::Cold { expired_for_ms }) => {
                    format!("cold ({}s ago)", expired_for_ms / 1_000)
                }
                None => "unknown".to_string(),
            };
            format!(
                "model {} · context {}/{} tokens · cache {cache} · read in {elapsed_ms} ms",
                known(&status.model),
                known(&status.context_used_tokens),
                known(&status.context_window_tokens),
            )
        }
        SessionStatusBlock::Unbound => "no harness session bound".to_string(),
        SessionStatusBlock::Unsupported { harness } => {
            format!("{} sessions are not readable yet", harness.as_str())
        }
        SessionStatusBlock::NotFound { detail } => format!("transcript not found: {detail}"),
        SessionStatusBlock::Failed { error } => format!("read failed: {error}"),
    }
}

fn parse_machine_cursor(value: &str) -> Result<MachineCursor, String> {
    let (machine, cursor) = value
        .split_once('=')
        .ok_or_else(|| "cursor must be MACHINE=CURSOR".to_string())?;
    if machine.trim().is_empty() {
        return Err("cursor machine must be non-empty".to_string());
    }
    let cursor = cursor
        .parse::<u64>()
        .map_err(|error| format!("cursor must be an unsigned integer: {error}"))?;
    Ok(MachineCursor {
        machine: machine.to_string(),
        cursor,
    })
}

fn cursor_map(values: Vec<MachineCursor>) -> Result<BTreeMap<String, u64>, PijError> {
    let mut cursors = BTreeMap::new();
    for value in values {
        if cursors
            .insert(value.machine.clone(), value.cursor)
            .is_some()
        {
            return Err(PijError::Adapter {
                adapter: "tail".to_string(),
                message: format!(
                    "machine `{}` has more than one --since cursor",
                    value.machine
                ),
            });
        }
    }
    Ok(cursors)
}

fn parse_harness(value: &str) -> Result<Harness, String> {
    Harness::parse(value).ok_or_else(|| {
        format!("unknown harness '{value}'; expected claude, copilot, codex, pi, or omp")
    })
}

fn parse_absolute_path(value: &str) -> Result<String, String> {
    if std::path::Path::new(value).is_absolute() {
        Ok(value.to_string())
    } else {
        Err("folder must be an absolute path".to_string())
    }
}

fn parse_absolute_path_buf(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("cwd must be an absolute path".to_string())
    }
}

fn prepare_spawn_request(command: &Command) -> Result<Option<SpawnRequest>, PijError> {
    let Command::Spawn {
        id,
        harness,
        allow_retired,
        bin,
        model,
        effort,
        cwd,
        session,
        name,
        parent,
        accept_inbound,
        no_accept_inbound,
        no_wait,
        wait_seconds,
    } = command
    else {
        return Ok(None);
    };
    validate_executable_override(*harness, bin.as_deref())?;
    let cwd = match cwd {
        Some(cwd) => cwd.clone(),
        None => std::env::current_dir().map_err(|error| PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!("could not read the caller directory: {error}"),
        })?,
    };
    if matches!(harness, Harness::Pi | Harness::Omp) && is_linked_worktree(&cwd)? {
        return Err(PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!(
                "refusing {harness} spawn from linked worktree {}: global and project extension links collide and the peer dies before registration — spawn from the main checkout and cd afterwards",
                cwd.display()
            ),
        });
    }
    let caller_pane = if session.is_none() {
        Some(
            std::env::var("TMUX_PANE")
                .ok()
                .filter(|pane| !pane.trim().is_empty())
                .ok_or_else(|| PijError::Adapter {
                    adapter: "spawn".to_string(),
                    message: "TMUX_PANE is unavailable — pass --session <tmux> explicitly"
                        .to_string(),
                })?,
        )
    } else {
        None
    };
    let cwd = cwd
        .into_os_string()
        .into_string()
        .map_err(|_| PijError::Adapter {
            adapter: "spawn".to_string(),
            message: "cwd is not valid UTF-8 and cannot be stored in a descriptor".to_string(),
        })?;

    // Claude seats accept inbound by default (Jordan's ruling, plan 116); an explicit
    // --no-accept-inbound withdraws it. Non-Claude harnesses keep the prior behavior:
    // false unless explicitly requested, which build_spawn_plan then refuses by name.
    let accept_inbound = match harness {
        Harness::Claude => !*no_accept_inbound,
        _ => *accept_inbound,
    };

    Ok(Some(SpawnRequest {
        id: id.clone().map(Into::into),
        harness: *harness,
        allow_retired: *allow_retired,
        executable: bin.clone(),
        model: model.clone(),
        effort: effort.clone(),
        cwd,
        session: session.clone(),
        caller_pane,
        name: name.clone(),
        parent: parent.clone().map(Into::into),
        accept_inbound,
        wait_seconds: (!*no_wait).then_some(*wait_seconds),
        no_wait: *no_wait,
        resume: None,
    }))
}

fn run_spawn_child(status_file: &Path, log_file: &Path, command: &[OsString]) -> ExitCode {
    let Some((executable, args)) = command.split_first() else {
        eprintln!("pij-rs spawn child: no executable followed `--`");
        return ExitCode::from(2);
    };
    if let Some(parent) = status_file.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!(
            "pij-rs spawn child: could not create evidence directory {}: {error}",
            parent.display()
        );
    }
    let mut child = match ProcessCommand::new(executable)
        .args(args)
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let message = format!("could not launch {executable:?}: {error}");
            eprintln!("pij-rs spawn child: {message}");
            let _ = std::fs::write(log_file, format!("{message}\n"));
            let _ = std::fs::write(status_file, "exit_code=127\n");
            std::thread::sleep(Duration::from_secs(2));
            return ExitCode::from(127);
        }
    };
    let stderr = child.stderr.take().expect("piped child stderr");
    let log_file = log_file.to_path_buf();
    let copier = std::thread::spawn(move || {
        let mut input = stderr;
        let mut terminal = std::io::stderr().lock();
        let mut log = std::fs::File::create(&log_file).ok();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let read = match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    let _ = writeln!(terminal, "pij-rs spawn child: stderr read failed: {error}");
                    break;
                }
            };
            let _ = terminal.write_all(&buffer[..read]);
            let _ = terminal.flush();
            if let Some(log) = log.as_mut() {
                let _ = log.write_all(&buffer[..read]);
            }
        }
    });
    let code = match child.wait() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("pij-rs spawn child: could not wait for {executable:?}: {error}");
            1
        }
    };
    let _ = copier.join();
    if let Err(error) = std::fs::write(status_file, format!("exit_code={code}\n")) {
        eprintln!(
            "pij-rs spawn child: could not write status {}: {error}",
            status_file.display()
        );
    }
    std::thread::sleep(Duration::from_secs(2));
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

#[derive(serde::Serialize)]
struct SpawnCliResponse {
    #[serde(flatten)]
    seat: SeatDescriptor,
    dispatched: bool,
    bound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
}

fn spawn_cli_response(response: Envelope<SeatDescriptor>) -> Envelope<SpawnCliResponse> {
    Envelope {
        ok: response.ok,
        command: response.command,
        v: response.v,
        data: response.data.map(|seat| {
            let pid = seat.proc.map(|identity| identity.pid);
            SpawnCliResponse {
                seat,
                dispatched: true,
                bound: pid.is_some(),
                pid,
            }
        }),
        meta: response.meta,
        error: response.error,
        details: response.details,
        // Successful spawn output is an established CLI-derived payload.
        raw_json: if response.ok { None } else { response.raw_json },
    }
}

fn prepare_revive_request(command: &Command) -> Result<Option<ReviveRequest>, PijError> {
    let Command::Revive {
        id,
        session,
        name,
        assume_dead,
        evidence,
        fresh,
    } = command
    else {
        return Ok(None);
    };
    let caller_pane = if session.is_none() {
        Some(
            std::env::var("TMUX_PANE")
                .ok()
                .filter(|pane| !pane.trim().is_empty())
                .ok_or_else(|| PijError::Adapter {
                    adapter: "revive".to_string(),
                    message: "TMUX_PANE is unavailable — pass --session <tmux> explicitly"
                        .to_string(),
                })?,
        )
    } else {
        None
    };
    Ok(Some(ReviveRequest {
        id: id.clone().into(),
        session: session.clone(),
        caller_pane,
        name: name.clone(),
        caller: caller_context(),
        assume_dead: *assume_dead,
        evidence: evidence.clone(),
        fresh: *fresh,
    }))
}

fn is_linked_worktree(cwd: &Path) -> Result<bool, PijError> {
    let output = ProcessCommand::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(cwd)
        .output()
        .map_err(|error| PijError::Adapter {
            adapter: "spawn".to_string(),
            message: format!("could not inspect git worktree state: {error}"),
        })?;
    if !output.status.success() {
        return Ok(false);
    }
    let raw = String::from_utf8(output.stdout).map_err(|error| PijError::Adapter {
        adapter: "spawn".to_string(),
        message: format!("git returned a non-UTF-8 git-dir: {error}"),
    })?;
    Ok(git_dir_is_linked(cwd, raw.trim()))
}

fn git_dir_is_linked(cwd: &Path, git_dir: &str) -> bool {
    let path = PathBuf::from(git_dir);
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    path.components()
        .any(|part| part.as_os_str() == "worktrees")
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::Daemon { .. } => "pij daemon",
            Command::Doctor { .. } => "pij doctor",
            Command::SpawnChild { .. } => "pij spawn child",
            Command::Ping => "pij ping",
            Command::Sidecar { .. } => "pij sidecar",
            Command::Register { .. } => "pij register",
            Command::Adopt { .. } => "pij adopt",
            Command::Whoami { .. } => "pij whoami",
            Command::FyiClaim { .. } => "pij fyi-claim",
            Command::FyiRead { .. } => "pij fyi-read",
            Command::Activity { .. } => "pij activity",
            Command::CommitTrailers => "pij commit-trailers",
            Command::FleetReport { .. } => "pij fleet-report",
            Command::Phonehome { .. } => "pij phonehome",
            Command::Send { .. } => "pij send",
            Command::CompactSelf => "pij compact-self",
            Command::Spawn { .. } => "pij spawn",
            Command::Revive { .. } => "pij revive",
            Command::Inbox { .. } => "pij inbox",
            Command::Bg { .. } => "pij bg",
            Command::Report { .. } => "pij report",
            Command::Role { .. } => "pij role",
            Command::Close { .. } => "pij close",
            Command::Reap { .. } => "pij reap",
            Command::Anomalies { .. } => "pij anomalies",
            Command::Decisions { .. } => "pij decisions",
            Command::Answer { .. } => "pij answer",
            Command::Project { .. } => "pij project",
            Command::Stream { .. } => "pij stream",
            Command::Fence { .. } => "pij fence",
            Command::Dispatch { .. } => "pij dispatch",
            Command::Ack { .. } => "pij ack",
            Command::Canary { .. } => "pij canary",
            Command::Attest { .. } => "pij attest",
            Command::Task { .. } => "pij task",
            Command::Node { .. } => "pij node",
            Command::Orchestration { .. } => "pij orchestration",
            Command::Spine { .. } => "pij spine",
            Command::List { .. } => "pij list",
            Command::State { .. } => "pij state",
            Command::Tail { .. } => "pij tail",
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn open_file_targets_lift_a_launchd_default_within_the_hard_limit() {
        // launchd: soft 256, hard unlimited.
        assert_eq!(
            super::open_file_targets(256, u64::MAX),
            vec![65_536, 10_240]
        );
        // A hard cap below the goal is respected.
        assert_eq!(super::open_file_targets(256, 4_096), vec![4_096]);
        // Already generous: nothing to do.
        assert!(super::open_file_targets(1_048_576, u64::MAX).is_empty());
    }

    use std::ffi::OsString;

    use clap::{CommandFactory, Parser};
    use pij_core::model::{Harness, SeatId};

    use super::{
        Cli, Command, DaemonAction, caller_claude_warning, git_dir_is_linked,
        prepare_spawn_request, resolve_bind_addr, resolve_client_addr, resolve_retired_harnesses,
        resolve_state_dir,
    };

    #[test]
    fn typing_state_renders_count_and_since_without_corrupting_json() {
        let card: pij_daemon::http::StateCard = serde_json::from_value(serde_json::json!({
            "id": "pij-typist", "state": "idle", "liveness": "active", "cwd": "/abs/tree", "harness": "omp", "unsupported": [],
            "held": [
                {"msg_id": "one", "seat": "pij-typist", "reason": "human-typing", "since_ms": 7},
                {"msg_id": "two", "seat": "pij-typist", "reason": "human-typing", "since_ms": 9}
            ]
        })).expect("typed daemon state");
        let response = pij_core::model::Envelope::ok("pij state", card);
        let human = super::render_state(&response, false);
        assert!(
            human.contains("held: 2 (human-typing since 7 ms)"),
            "{human}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&super::render_state(&response, true)).expect("unadorned JSON");
        assert_eq!(json["data"]["held"].as_array().expect("holds").len(), 2);
    }

    /// Plan 160: the daemon renders the size lines; `pij-rs state` prints them.
    #[test]
    fn state_prints_the_daemons_size_lines() {
        let lines = [
            "context 720k / 1M · last call 2h ago · cache 1h (cold 1h) · 3 compactions",
            "❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi holds it for $0 now)",
        ];
        let card: pij_daemon::http::StateCard = serde_json::from_value(serde_json::json!({
            "id": "pij-cold", "state": "idle", "liveness": "active", "cwd": "/abs/tree",
            "harness": "claude", "unsupported": [], "held": [], "sizeLines": lines,
        }))
        .expect("typed daemon state");
        let human = super::render_state(&pij_core::model::Envelope::ok("pij state", card), false);
        for line in lines {
            assert!(human.contains(&format!("\n{line}")), "{human}");
        }
    }

    #[test]
    fn state_exposes_delivery_deferrals_without_calling_them_delivered() {
        let card: pij_daemon::http::StateCard = serde_json::from_value(serde_json::json!({
            "id": "pij-waiting", "state": "idle", "liveness": "active", "cwd": "/abs/tree",
            "harness": "claude", "unsupported": [],
            "deliveryDeferrals": [{
                "job_id": 12, "msg_id": "waiting", "reason": "unrecognized",
                "count": 1029, "since_ms": 1000
            }]
        }))
        .unwrap();
        let response = pij_core::model::Envelope::ok("pij state", card);
        let human = super::render_state(&response, false);
        assert!(
            human.contains("delivery deferred: unrecognized ×1029 since 1000 ms"),
            "{human}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&super::render_state(&response, true)).unwrap();
        assert_eq!(json["data"]["deliveryDeferrals"][0]["count"], 1029);
    }

    #[test]
    fn stale_native_receiver_diagnosis_survives_human_and_json_state_output() {
        let card = serde_json::from_value::<pij_daemon::http::StateCard>(serde_json::json!({
            "id":"pij-native", "state":"idle", "liveness":"active", "cwd":"/abs/tree",
            "harness":"copilot", "unsupported":[],
            "native_receiver_reason":"native-receiver-stale",
        }))
        .unwrap();
        let response = pij_core::model::Envelope::ok("pij state", card);
        assert!(super::render_state(&response, false).contains("native-receiver-stale"));
        let json: serde_json::Value =
            serde_json::from_str(&super::render_state(&response, true)).unwrap();
        assert_eq!(
            json["data"]["native_receiver_reason"],
            "native-receiver-stale"
        );
        assert_eq!(json["data"]["liveness"], "active");
    }

    #[test]
    fn claude_adopt_warns_for_unaccepted_caller_config_dir() {
        let home = pij_testkit::fresh_dir("pij-caller-claude-warning");
        let warning = caller_claude_warning(
            Harness::Claude.as_str(),
            Some(home.clone().into_os_string()),
        )
        .expect("missing setting warns");
        assert!(warning.contains(&home.display().to_string()));
        std::fs::write(
            home.join("settings.json"),
            r#"{"crossSessionInbound":"accept"}"#,
        )
        .expect("accepted settings");
        assert!(
            caller_claude_warning(
                Harness::Claude.as_str(),
                Some(home.clone().into_os_string()),
            )
            .is_none()
        );
        assert!(caller_claude_warning("omp", Some(home.clone().into_os_string())).is_none());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_held_fyi_prints_its_question_warning_on_the_next_line() {
        let receipt = |warning: Option<&str>| {
            let mut envelope = pij_core::model::Envelope::ok(
                "pij send",
                pij_core::model::Receipt {
                    msg_id: "m-1".to_string(),
                    outcome: pij_core::model::DeliveryOutcome::Held {
                        reason: "fyi".to_string(),
                    },
                    at: 1,
                    cold_check: None,
                    warning: warning.map(str::to_string),
                },
            );
            envelope.ok = true;
            envelope
        };
        assert_eq!(
            super::held_fyi_output(&receipt(Some(pij_core::fyi::QUESTION_WARNING)), "m-1")
                .as_deref(),
            Some(
                "pij send: held (fyi) — m-1\nthis looks like a question; if you need an answer, resend without --fyi"
            )
        );
        assert_eq!(
            super::held_fyi_output(&receipt(None), "m-1").as_deref(),
            Some("pij send: held (fyi) — m-1")
        );
    }

    #[test]
    fn clap_exposes_every_ruled_verb() {
        let names: Vec<_> = Cli::command()
            .get_subcommands()
            .filter(|command| !command.is_hide_set())
            .map(|command| command.get_name().to_string())
            .collect();

        // The roster is PINNED, so adding a verb is a deliberate act that fails
        // this test first. `adopt`, `whoami` and `phonehome` are plan 114's
        // identity verbs (ac-1142, ac-1147, ac-1148); `tail` stays exactly where
        // it was — it is the FALSE FRIEND the generation shim denies from routing,
        // and nothing here changes what it follows.
        assert_eq!(
            names,
            [
                // Adding a verb to this roster is meant to be a DELIBERATE act
                // that fails this test first — which is exactly what it did for
                // `report` (u-report), adopt/whoami/phonehome (u-identity) and
                // `state` (u-readback) and `doctor` (plan 126).
                "daemon",
                "ping",
                "doctor",
                "register",
                "adopt",
                "whoami",
                "commit-trailers",
                "phonehome",
                "spawn",
                "revive",
                "send",
                "activity",
                "fyi-claim",
                "fyi-read",
                "compact-self",
                "inbox",
                "bg",
                "sidecar",
                "report",
                "role",
                "close",
                "reap",
                "anomalies",
                "decisions",
                "answer",
                "project",
                "stream",
                "fence",
                "dispatch",
                "ack",
                "canary",
                "attest",
                "task",
                "node",
                "orchestration",
                "spine",
                "list",
                "state",
                "tail"
            ]
        );
    }

    #[test]
    fn commit_trailers_accepts_no_identity_or_plan_overrides() {
        assert!(matches!(
            Cli::try_parse_from(["pij-rs", "commit-trailers"])
                .expect("trailer command")
                .command,
            Command::CommitTrailers
        ));
        for flag in ["--seat", "--prime", "--plan"] {
            assert!(Cli::try_parse_from(["pij-rs", "commit-trailers", flag, "invented"]).is_err());
        }
    }

    #[test]
    fn doctor_exposes_claude_hook_state() {
        let command = Cli::command();
        let doctor = command.find_subcommand("doctor").expect("doctor command");
        let names = doctor
            .get_subcommands()
            .map(|command| command.get_name())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["claude-inbound", "claude-hook", "claude-statusline"]
        );
        assert!(Cli::try_parse_from(["pij-rs", "doctor", "claude-hook"]).is_ok());
        assert!(Cli::try_parse_from(["pij-rs", "doctor", "claude-statusline"]).is_ok());
    }

    #[test]
    fn rendered_pointer_advertises_a_verb_the_real_cli_exposes() {
        let line = pij_daemon::pointer::render_pointer(&SeatId::from("pij-sender"), None);
        let action = line
            .lines()
            .find_map(|line| line.strip_prefix("message waiting — run: "))
            .expect("pointer must carry a runnable action");
        let mut words = action.split_whitespace();
        assert_eq!(
            words.next(),
            Some("pij"),
            "pointer must name the shipped command"
        );
        let verb = words.next().expect("pointer action must name a verb");
        assert!(
            Cli::command()
                .get_subcommands()
                .any(|command| command.get_name() == verb),
            "pointer advertises `{verb}`, but Clap does not expose it"
        );
    }

    #[test]
    fn send_short_forms_leave_identity_and_message_id_for_runtime_resolution() {
        let cli = Cli::try_parse_from([
            "pij-rs",
            "send",
            "--to",
            "pij-peer",
            "--body",
            "--starts-with-a-hyphen",
        ])
        .expect("short send form parses");
        let Command::Send {
            from,
            body,
            body_file,
            msg_id,
            ..
        } = cli.command
        else {
            panic!("expected send");
        };
        assert_eq!(from, None);
        assert_eq!(body.as_deref(), Some("--starts-with-a-hyphen"));
        assert_eq!(body_file, None);
        assert_eq!(msg_id, None);

        let stdin = Cli::try_parse_from(["pij-rs", "send", "--to", "pij-peer", "--body-file", "-"])
            .expect("stdin body form parses");
        let Command::Send { body_file, .. } = stdin.command else {
            panic!("expected send");
        };
        assert_eq!(body_file.as_deref(), Some("-"));
    }

    #[test]
    fn telegram_body_sources_are_mutually_exclusive() {
        let error = Cli::try_parse_from([
            "pij-rs",
            "sidecar",
            "telegram",
            "--body",
            "hello",
            "--body-file",
            "-",
        ])
        .err()
        .expect("conflicting body sources must refuse before reading stdin");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn missing_send_body_reaches_the_named_runtime_refusal() {
        let cli = Cli::try_parse_from(["pij-rs", "send", "--to", "pij-peer"])
            .expect("clap must not replace the named refusal with a usage dump");
        let Command::Send {
            body, body_file, ..
        } = cli.command
        else {
            panic!("expected send");
        };
        assert_eq!(body, None);
        assert_eq!(body_file, None);
    }

    #[test]
    fn inbox_peek_is_an_explicit_non_destructive_mode() {
        let cli = Cli::try_parse_from(["pij-rs", "inbox", "--seat", "pij-other", "--peek"])
            .expect("cross-seat peek parses");
        let Command::Inbox {
            seat, wait, peek, ..
        } = cli.command
        else {
            panic!("expected inbox");
        };
        assert_eq!(seat.as_deref(), Some("pij-other"));
        assert!(!wait);
        assert!(peek);
    }

    #[test]
    fn help_leads_with_short_send_and_destructive_inbox_forms() {
        let mut command = Cli::command();
        let send_help = command
            .find_subcommand_mut("send")
            .expect("send command")
            .render_long_help()
            .to_string();
        let body = send_help
            .find("pij-rs send --to <id> --body 'hello'")
            .expect("short body example");
        let stdin = send_help
            .find("pij-rs send --to <id> --body-file -")
            .expect("stdin body example");
        assert!(body < stdin, "literal short form leads: {send_help}");

        let inbox_help = command
            .find_subcommand_mut("inbox")
            .expect("inbox command")
            .render_long_help()
            .to_string();
        assert!(
            inbox_help.contains("Destructively read and acknowledge this acting seat's inbox"),
            "{inbox_help}"
        );
    }

    #[test]
    fn editorial_help_is_shape_checked_not_frozen() {
        let command = Cli::command();
        assert_eq!(command.get_name(), "pij-rs");
        assert!(
            command
                .get_arguments()
                .any(|argument| argument.get_id() == "json")
        );
        assert!(
            command
                .get_arguments()
                .any(|argument| argument.get_id() == "state_dir")
        );
        assert!(
            command
                .get_arguments()
                .any(|argument| argument.get_id() == "addr")
        );
    }

    #[test]
    fn clap_leaves_runtime_defaults_unresolved() {
        let cli = Cli::try_parse_from(["pij-rs", "ping"]).expect("ping");
        assert!(
            cli.addr.is_none(),
            "a parser default would shadow PIJ_RS_ADDR"
        );
        assert!(cli.state_dir.is_none());

        let daemon = Cli::try_parse_from(["pij-rs", "daemon"]).expect("daemon");
        let Command::Daemon { bind, .. } = daemon.command else {
            panic!("expected daemon");
        };
        assert!(bind.is_none(), "a parser default would shadow PIJ_RS_BIND");
    }

    #[test]
    fn runtime_controls_resolve_flag_then_environment_then_default() {
        assert_eq!(
            resolve_client_addr(
                Some("127.0.0.1:7001"),
                Some(OsString::from("127.0.0.1:7002"))
            )
            .expect("flag address"),
            "127.0.0.1:7001"
        );
        assert_eq!(
            resolve_client_addr(None, Some(OsString::from("localhost:7002")))
                .expect("environment address"),
            "localhost:7002"
        );
        assert_eq!(
            resolve_client_addr(None, None).expect("default address"),
            "127.0.0.1:7461"
        );

        assert_eq!(
            resolve_bind_addr(
                Some("127.0.0.1:7101"),
                Some(OsString::from("127.0.0.1:7102"))
            )
            .expect("flag bind"),
            "127.0.0.1:7101"
        );
        assert_eq!(
            resolve_bind_addr(None, Some(OsString::from("127.0.0.1:7102")))
                .expect("environment bind"),
            "127.0.0.1:7102"
        );
        assert_eq!(
            resolve_bind_addr(None, None).expect("default bind"),
            "127.0.0.1:7461"
        );

        assert_eq!(
            resolve_state_dir(
                Some(std::path::PathBuf::from("/flag")),
                Some(OsString::from("/environment"))
            )
            .expect("flag state dir"),
            std::path::PathBuf::from("/flag")
        );
        assert_eq!(
            resolve_state_dir(None, Some(OsString::from("/environment")))
                .expect("environment state dir"),
            std::path::PathBuf::from("/environment")
        );
    }

    #[test]
    fn malformed_environment_values_name_the_variable_and_value() {
        for (resolve, name) in [
            (
                resolve_client_addr as fn(Option<&str>, Option<OsString>) -> Result<String, String>,
                "PIJ_RS_ADDR",
            ),
            (resolve_bind_addr, "PIJ_RS_BIND"),
        ] {
            let empty = resolve(None, Some(OsString::from(""))).expect_err("empty must fail");
            assert!(empty.contains(name));
            assert!(empty.contains("\"\""));
        }

        let invalid_addr = resolve_client_addr(None, Some(OsString::from("localhost")))
            .expect_err("missing port must fail");
        assert!(invalid_addr.contains("PIJ_RS_ADDR"));
        assert!(invalid_addr.contains("\"localhost\""));

        let invalid_bind = resolve_bind_addr(None, Some(OsString::from("localhost:7461")))
            .expect_err("bind needs an IP socket address");
        assert!(invalid_bind.contains("PIJ_RS_BIND"));
        assert!(invalid_bind.contains("\"localhost:7461\""));

        for value in ["", "  "] {
            assert_eq!(
                resolve_state_dir(None, Some(OsString::from(value)))
                    .expect_err("empty or whitespace state dir"),
                format!("PIJ_RS_STATE_DIR has malformed value {value:?}: value must not be empty")
            );
        }
    }

    #[test]
    fn daemon_bounce_is_real_while_bare_daemon_still_runs_foreground() {
        let bounced = Cli::try_parse_from(["pij-rs", "daemon", "bounce"]).expect("bounce");
        assert!(matches!(
            bounced.command,
            Command::Daemon {
                action: Some(DaemonAction::Bounce),
                ..
            }
        ));

        let foreground = Cli::try_parse_from(["pij-rs", "daemon"]).expect("foreground");
        assert!(matches!(
            foreground.command,
            Command::Daemon { action: None, .. }
        ));
    }

    #[test]
    fn register_requires_a_complete_process_identity() {
        let parsed = Cli::try_parse_from([
            "pij-rs",
            "register",
            "seat",
            "--harness",
            "omp",
            "--folder",
            "/tmp",
            "--pid",
            "42",
        ]);

        assert!(parsed.is_err());
    }

    /// FLIPPED when u-names was wired into `/v1/spawn` (review F4): omitting
    /// `--id` is now how you ASK for a memorable name, not an error. The
    /// assertion flips rather than being deleted, as the refusal tests did in
    /// waves 1 and 3 — deleting it would quietly drop the guarantee that the
    /// argument is optional while the diff looked like tidying.
    #[test]
    fn spawn_without_an_id_parses_and_leaves_allocation_to_the_daemon() {
        let parsed = Cli::try_parse_from(["pij-rs", "spawn", "--harness", "claude"])
            .expect("omitting --id is how a caller asks for a memorable name");
        let Command::Spawn { id, .. } = parsed.command else {
            panic!("expected the spawn command");
        };
        assert!(
            id.is_none(),
            "the CLI must not invent an id: the daemon holds the roster and the lock"
        );
    }

    #[test]
    fn claude_spawn_accepts_inbound_by_default() {
        let parsed = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "claude",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
        ])
        .expect("default claude spawn parses");
        let request = prepare_spawn_request(&parsed.command)
            .expect("valid request")
            .expect("spawn request");
        assert!(
            request.accept_inbound,
            "plan 116: Claude seats accept inbound by default with no flags"
        );
    }

    #[test]
    fn spawn_waits_thirty_seconds_by_default_and_no_wait_disables_it() {
        let defaulted = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "claude",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
        ])
        .expect("default wait parses");
        let defaulted = prepare_spawn_request(&defaulted.command)
            .expect("default request")
            .expect("spawn request");
        assert_eq!(defaulted.wait_seconds, Some(30));
        assert!(!defaulted.no_wait);

        let immediate = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "claude",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
            "--no-wait",
        ])
        .expect("no-wait parses");
        let immediate = prepare_spawn_request(&immediate.command)
            .expect("no-wait request")
            .expect("spawn request");
        assert_eq!(immediate.wait_seconds, None);
        assert!(immediate.no_wait);
    }

    #[test]
    fn no_accept_inbound_withdraws_the_claude_default() {
        let parsed = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "claude",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
            "--no-accept-inbound",
        ])
        .expect("opt-out flag parses");
        let request = prepare_spawn_request(&parsed.command)
            .expect("valid request")
            .expect("spawn request");
        assert!(
            !request.accept_inbound,
            "--no-accept-inbound must withdraw the default"
        );
    }

    #[test]
    fn explicit_accept_inbound_still_works_for_claude() {
        let parsed = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "claude",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
            "--accept-inbound",
        ])
        .expect("explicit affirmation parses");
        let request = prepare_spawn_request(&parsed.command)
            .expect("valid request")
            .expect("spawn request");
        assert!(request.accept_inbound);
    }

    #[test]
    fn accept_inbound_and_no_accept_inbound_conflict() {
        let parsed = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "claude",
            "--accept-inbound",
            "--no-accept-inbound",
        ]);
        assert!(
            parsed.is_err(),
            "affirming and withdrawing consent in the same call must not silently pick one"
        );
    }

    #[test]
    fn non_claude_spawn_defaults_to_no_inbound_setting() {
        let parsed = Cli::try_parse_from([
            "pij-rs",
            "spawn",
            "--harness",
            "copilot",
            "--cwd",
            "/abs/tree",
            "--session",
            "fleet",
        ])
        .expect("default copilot spawn parses");
        let request = prepare_spawn_request(&parsed.command)
            .expect("valid request")
            .expect("spawn request");
        assert!(
            !request.accept_inbound,
            "non-Claude harnesses must not silently inherit the Claude default"
        );
    }

    #[test]
    fn retired_harness_config_rejects_typos_and_honors_explicit_empty_policy() {
        assert_eq!(
            resolve_retired_harnesses(Some(OsString::from(" pi,omp,pi ")))
                .expect("known harness names"),
            vec![Harness::Pi, Harness::Omp]
        );
        assert_eq!(
            resolve_retired_harnesses(Some(OsString::from(""))).expect("explicit empty policy"),
            Vec::<Harness>::new()
        );
        for value in ["opm", "pi,"] {
            let error = resolve_retired_harnesses(Some(OsString::from(value)))
                .expect_err("invalid harness config");
            assert!(error.contains("PIJ_RETIRED_HARNESSES"));
            assert!(error.contains("unknown harness"));
        }
    }

    #[test]
    fn bin_harness_selector_refuses_before_cwd_or_tmux_resolution() {
        let parsed = Cli::try_parse_from(["pij-rs", "spawn", "--harness", "pi", "--bin", "omp"])
            .expect("spawn arguments");
        let error = prepare_spawn_request(&parsed.command).expect_err("not a harness switch");
        assert!(error.to_string().contains("use --harness omp"));
    }

    #[test]
    fn linked_worktree_detection_names_the_git_dir_shape() {
        let cwd = std::path::Path::new("/repo/linked");
        assert!(git_dir_is_linked(cwd, "/repo/main/.git/worktrees/linked"));
        assert!(!git_dir_is_linked(cwd, "/repo/main/.git"));
        assert!(!git_dir_is_linked(cwd, ".git"));
    }
}

#[cfg(test)]
mod identity_cli_tests {
    use super::adopt_argv;

    /// The pane form reproduces `/pij ready`'s own command line exactly, so the
    /// daemon parses one shape whether the caller was the shim or this binary.
    #[test]
    fn the_pane_form_is_the_ready_gestures_command_line() {
        assert_eq!(
            adopt_argv(Some("%77"), "claude", Some("pij-governor")),
            [
                "adopt",
                "%77",
                "--harness",
                "claude",
                "--parent",
                "pij-governor"
            ]
        );
    }

    /// An omitted parent is UNSAID, not cleared — it must not reach the wire at
    /// all, because the daemon carries the incumbent's value forward only when
    /// the command line was silent (req-0008).
    #[test]
    fn an_omitted_parent_is_absent_from_the_wire() {
        let argv = adopt_argv(Some("%77"), "claude", None);
        assert!(!argv.iter().any(|token| token == "--parent"), "{argv:?}");
    }

    /// No pane means the paneless form, which the daemon refuses by name unless
    /// a corroborable process identity travels with it.
    #[test]
    fn no_pane_builds_the_paneless_form() {
        assert_eq!(
            adopt_argv(None, "claude", None),
            ["inbox", "register", "--harness", "claude"]
        );
    }
}
