//! Real tmux adapter. Every Rust-workspace tmux syscall lives in this crate.

mod control;
mod ledger;

pub use control::CONTROL_SESSION;
mod parse;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use pij_core::delivery::MAX_TYPED_FRAME_BYTES;
use pij_core::error::{PijError, Result};
use pij_core::model::{Harness, Pane, PaneProcess, ProcIdentity};
use pij_core::ports::{
    LaunchCommand, STAGED_SUBMIT_RECOVERY, StagedSubmit, TapSweep, TmuxPort, is_rendered_leftover,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{Instant, sleep};

use crate::ledger::TapLedger;
use crate::parse::{LIST_FORMAT, parse_one_pane, parse_panes};

/// Empty a tap sink the reader has consumed to `consumed`, and say whether it did.
///
/// Called only after a drain found nothing new, so the pane has been quiet since
/// the previous drain. The writer is `cat >>` (O_APPEND): after `set_len(0)` its
/// next write lands at offset 0, where the reader restarts. The length is
/// re-read on the open file first and a sink that grew is left alone. A write
/// landing between that re-read and the truncation (one syscall apart, and only
/// as the first write of a new burst) is lost; the burst's later writes still
/// report the pane active.
///
/// # Errors
/// The sink cannot be opened for writing, inspected or truncated.
pub fn trim_consumed_tap(path: &Path, consumed: u64) -> io::Result<bool> {
    let file = OpenOptions::new().write(true).open(path)?;
    if file.metadata()?.len() != consumed {
        return Ok(false);
    }
    file.set_len(0)?;
    Ok(true)
}

fn sink_file_name(pane: &str) -> String {
    tap_sink_path(Path::new(""), pane)
        .to_string_lossy()
        .into_owned()
}

/// A plain sink file name: no separator can lead a ledger entry out of the root.
fn is_sink_name(name: &str) -> bool {
    name.ends_with(".raw") && !name.contains('/') && !name.starts_with('.')
}

/// Regular `*.raw` files directly in the tap root.
fn unledgered_sinks(root: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && is_sink_name(name)
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Deterministic sink identity shared by composition and the adapter authority.
#[must_use]
pub fn tap_sink_path(root: &Path, pane: &str) -> PathBuf {
    let safe: String = pane
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    root.join(format!("{safe}.raw"))
}

const TAP_SINK_OPTION: &str = "@pij-tap-sink";
/// A live tap's sink is emptied at the first quiet drain after the daemon has
/// consumed this many bytes of it, so one pane's file stays under this plus one
/// burst of output (plan 185, D5).
pub const TAP_TRIM_BYTES: u64 = 1024 * 1024;
/// An orphaned sink written this recently is spared by the sweep. A brake only:
/// removing it would make the sweep remove the same set or more, never another.
pub const ORPHAN_TAP_GRACE: Duration = Duration::from_secs(5 * 60);
const SUBMIT_CHUNK_BYTES: usize = 1_000;
const SUBMIT_CHUNK_PAUSE: Duration = Duration::from_millis(10);
/// Staging a body under owned input is bounded, but by LAUNCH latency, not by
/// tmux or the pane's program: a one-chunk body is three owned pastes at two
/// launches each, and nothing in it waits for the pane to read. Plan 184
/// measured process launch on this machine at p50 7 ms but p90 140 ms, p99
/// 280-390 ms and max 553 ms under ordinary load (`/usr/bin/true` included), so
/// the old 1 s bound tripped on a tail of six launches and stranded the staged
/// text. 4 s covers six launches at the worst observed latency (3.3 s); a
/// staged body normally takes tens of milliseconds, so the bound only lengthens
/// the rare tail in which the human's input stays disabled.
const SUBMIT_STAGE_LIMIT: Duration = Duration::from_secs(4);
/// The watchdog that restores input if the daemon never does. It must outlast
/// the stage bound plus an abort's notice paste and restore (three more
/// launches, 1.7 s at the worst observed latency): an earlier watchdog takes
/// the token mid-abort and the restore fails `exit 75`, as walrus showed.
const SUBMIT_RECOVERY_LIMIT_SECS: u64 = 8;
/// How long a cleared leftover is given to render before the gate re-reads the
/// composer to confirm it is blank (plan 184 review S3). Input stays disabled.
const CLEAR_SETTLE: Duration = Duration::from_millis(250);
const SUBMIT_OWNER_OPTION: &str = "@pij-submit-owner";

fn unsupported_terminal_control(text: &str) -> Option<(usize, char)> {
    text.char_indices()
        .find(|(_, ch)| ch.is_control() && *ch != '\n')
}

/// The real [`TmuxPort`] implementation.
///
/// A scoped adapter limits pane listings and destructive guards to one tmux
/// session. Tests must use [`TmuxAdapter::for_session`] so they cannot observe
/// or kill the operator's fleet.
///
/// `user_typing` reports only `pane_in_mode`, a fact tmux itself exposes. `false`
/// means "no tmux-observable interaction" and MUST NOT be documented, or read,
/// as "safe to inject". The wave-2 `HarnessPort` adapter must AND this fact with
/// its stateful composer-history and tap-byte gate.
///
/// # Composition recipe
///
/// COMPOSED in `crates/daemon/src/lib.rs`:
/// `AdapterChoice::Real => Arc::new(TmuxAdapter::new(pane_signal_dir)),`.
/// The same root is passed to `PaneObserver`; the adapter derives each exact sink
/// identity from it before attach or marker-fallback detach.
#[derive(Clone, Debug)]
pub struct TmuxAdapter {
    binary: OsString,
    server: Option<String>,
    session: Option<String>,
    tap_root: PathBuf,
    legacy_tap_root: Option<PathBuf>,
    submit_lock: Arc<AsyncMutex<()>>,
    taps: Arc<Mutex<BTreeMap<String, TapState>>>,
    /// Plan 180: reads ride one persistent control-mode client, not a process each.
    control: Arc<control::ControlClient>,
    /// Whether the orphan sweep may claim sinks written before tap ledgers
    /// existed. Only the default server's adapter does (plan 185 ruling).
    claims_legacy_sinks: bool,
}

#[derive(Debug)]
struct TapState {
    path: PathBuf,
    offset: u64,
}

impl TmuxAdapter {
    /// Use `tmux` resolved from `PATH` and permit workspace-wide pane listings.
    pub fn new(tap_root: impl Into<PathBuf>) -> Self {
        Self {
            binary: OsString::from("tmux"),
            server: None,
            session: None,
            tap_root: tap_root.into(),
            legacy_tap_root: None,
            submit_lock: Arc::new(AsyncMutex::new(())),
            taps: Arc::new(Mutex::new(BTreeMap::new())),
            claims_legacy_sinks: true,
            control: Arc::new(control::ControlClient::new(
                OsString::from("tmux"),
                None,
                true,
            )),
        }
    }
    /// Use `tmux` with an explicitly injected legacy-generation tap root.
    pub fn with_legacy_tap_root(
        tap_root: impl Into<PathBuf>,
        legacy_tap_root: impl Into<PathBuf>,
    ) -> Self {
        let mut adapter = Self::new(tap_root);
        adapter.legacy_tap_root = Some(legacy_tap_root.into());
        adapter
    }

    /// Limit listings, new windows, and destructive guards to `session`.
    pub fn for_session(session: impl Into<String>, tap_root: impl Into<PathBuf>) -> Self {
        Self {
            binary: OsString::from("tmux"),
            server: None,
            session: Some(session.into()),
            tap_root: tap_root.into(),
            legacy_tap_root: None,
            submit_lock: Arc::new(AsyncMutex::new(())),
            taps: Arc::new(Mutex::new(BTreeMap::new())),
            claims_legacy_sinks: true,
            control: Arc::new(control::ControlClient::new(
                OsString::from("tmux"),
                None,
                true,
            )),
        }
    }

    /// Inject the executable path while retaining the production argv contract.
    pub fn with_binary(binary: impl Into<OsString>, tap_root: impl Into<PathBuf>) -> Self {
        let binary = binary.into();
        Self {
            control: Arc::new(control::ControlClient::new(binary.clone(), None, false)),
            binary,
            server: None,
            session: None,
            tap_root: tap_root.into(),
            legacy_tap_root: None,
            submit_lock: Arc::new(AsyncMutex::new(())),
            taps: Arc::new(Mutex::new(BTreeMap::new())),
            claims_legacy_sinks: true,
        }
    }

    /// Use an isolated named tmux server, invisible to the user's live server.
    ///
    /// Real integration fixtures use this instead of sharing a mutable global
    /// server with production daemons that legitimately attach pane pipes.
    pub fn for_server(server: impl Into<String>, tap_root: impl Into<PathBuf>) -> Self {
        let server = server.into();
        Self {
            control: Arc::new(control::ControlClient::new(
                OsString::from("tmux"),
                Some(server.clone()),
                true,
            )),
            binary: OsString::from("tmux"),
            server: Some(server),
            session: None,
            legacy_tap_root: None,
            tap_root: tap_root.into(),
            submit_lock: Arc::new(AsyncMutex::new(())),
            taps: Arc::new(Mutex::new(BTreeMap::new())),
            claims_legacy_sinks: false,
        }
    }

    /// Use an isolated tmux server with an explicitly injected legacy tap root.
    pub fn for_server_with_legacy_tap_root(
        server: impl Into<String>,
        tap_root: impl Into<PathBuf>,
        legacy_tap_root: impl Into<PathBuf>,
    ) -> Self {
        let mut adapter = Self::for_server(server, tap_root);
        adapter.legacy_tap_root = Some(legacy_tap_root.into());
        adapter
    }

    /// Override whether the orphan sweep claims pre-ledger sinks once. On by
    /// default for the default server and off for a named one; a private-server
    /// fixture turns it on to exercise the default server's claim.
    #[must_use]
    pub fn with_legacy_sink_claim(mut self, claims: bool) -> Self {
        self.claims_legacy_sinks = claims;
        self
    }

    /// Opt in or out of the persistent control-mode client for reads.
    ///
    /// On by default for the real constructors; off for [`Self::with_binary`],
    /// whose fake executables record exact argv and need not speak `-C`.
    #[must_use]
    pub fn with_control_mode(mut self, enabled: bool) -> Self {
        self.control = Arc::new(self.control.with_enabled(enabled));
        self
    }

    /// Every pane on this adapter's server, whatever session it is scoped to:
    /// the sweep's "pane absent" decision is only sound over the whole server.
    fn server_list_args(&self) -> Vec<OsString> {
        os_args([
            "list-panes",
            "-a",
            "-f",
            control::NOT_CONTROL_PANE,
            "-F",
            LIST_FORMAT,
        ])
    }

    fn tap_ledger(&self) -> TapLedger {
        TapLedger::for_server(&self.tap_root, self.server.as_deref())
    }

    fn list_args(&self) -> Vec<OsString> {
        let mut args = vec![OsString::from("list-panes")];
        match &self.session {
            Some(session) => {
                args.push(OsString::from("-s"));
                args.push(OsString::from("-t"));
                args.push(OsString::from(session));
            }
            None => args.push(OsString::from("-a")),
        }
        // The control client's own pane is pij's, never a seat's: no lookup,
        // tap sweep, reap or caller-pane resolution should ever see it.
        args.push(OsString::from("-f"));
        args.push(OsString::from(control::NOT_CONTROL_PANE));
        args.push(OsString::from("-F"));
        args.push(OsString::from(LIST_FORMAT));
        args
    }

    /// The tmux command for `program`, the executable `pij_core::program::launch`
    /// resolved (plan 182: PATH is searched once, not on every launch).
    fn command(&self, program: OsString) -> Command {
        let mut command = Command::new(program);
        // `-u`: without a UTF-8 locale (a launchd-started daemon has no LANG)
        // tmux 3.6 rewrites the tab in every `-p`/`-F` answer as `_`.
        command.arg("-u");
        if let Some(server) = &self.server {
            command.args(["-L", server]);
        }
        command
    }

    /// The program a launch runs, through the same `launch` path `run` uses.
    #[cfg(test)]
    fn launched_program(&self) -> OsString {
        pij_core::program::launch(&self.binary, |program| {
            Ok::<_, io::Error>(self.command(program).get_program().to_os_string())
        })
        .expect("no spawn, so no error")
    }

    fn run(&self, operation: &str, args: &[OsString]) -> Result<Output> {
        pij_core::spawns::record(&format!("tmux.{operation}"));
        let output = pij_core::program::launch(&self.binary, |program| {
            self.command(program).args(args).output()
        })
        .map_err(|error| self.spawn_error(operation, error))?;
        self.finish(operation, output)
    }

    fn run_with_stdin(&self, operation: &str, args: &[OsString], input: &[u8]) -> Result<Output> {
        pij_core::spawns::record(&format!("tmux.{operation}"));
        let mut child = pij_core::program::launch(&self.binary, |program| {
            self.command(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        })
        .map_err(|error| self.spawn_error(operation, error))?;
        let mut stdin = child.stdin.take().expect("piped tmux stdin");
        if let Err(error) = stdin.write_all(input) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(self.error(format!(
                "could not write tmux stdin for {operation}: {error}"
            )));
        }
        drop(stdin);
        let output = child
            .wait_with_output()
            .map_err(|error| self.spawn_error(operation, error))?;
        self.finish(operation, output)
    }

    fn paste_bytes_owned(&self, staged: &StagedSubmit, bytes: &[u8]) -> Result<()> {
        self.run_with_stdin(
            "load submit buffer",
            &os_args(["load-buffer", "-b", &staged.token, "-"]),
            bytes,
        )?;
        let target = shell_quote(&staged.pane);
        let buffer = shell_quote(&staged.token);
        let paste = format!(
            "select-pane -e -t {target} ; paste-buffer -d -r -b {buffer} -t {target} ; select-pane -d -t {target}"
        );
        if let Err(error) = self.run_if_submit_token(
            &staged.pane,
            &staged.token,
            "paste owned submit buffer",
            &paste,
        ) {
            let _ = self.run(
                "delete failed submit buffer",
                &os_args(["delete-buffer", "-b", &staged.token]),
            );
            return Err(error);
        }
        Ok(())
    }

    /// Stage `text` as one bracketed paste, recording on `staged` exactly what
    /// each successful paste delivered (plan 184), so an abort knows what it
    /// leaves behind and can close a paste it opened.
    async fn stage_body_owned(&self, staged: &mut StagedSubmit, text: &str) -> Result<()> {
        let started = Instant::now();
        self.paste_bytes_owned(staged, b"\x1b[200~")?;
        staged.paste_open = true;
        let mut start = 0;
        while start < text.len() {
            if started.elapsed() >= SUBMIT_STAGE_LIMIT {
                return Err(self.error(format!(
                    "staging for pane {:?} exceeded {:?}; body was not submitted",
                    staged.pane, SUBMIT_STAGE_LIMIT
                )));
            }
            let mut end = text.len().min(start + SUBMIT_CHUNK_BYTES);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.paste_bytes_owned(staged, &text.as_bytes()[start..end])?;
            staged.pasted.push_str(&text[start..end]);
            start = end;
            if start < text.len() {
                sleep(SUBMIT_CHUNK_PAUSE).await;
            }
        }
        self.paste_bytes_owned(staged, b"\x1b[201~")?;
        staged.paste_open = false;
        if started.elapsed() >= SUBMIT_STAGE_LIMIT {
            return Err(self.error(format!(
                "staging for pane {:?} exceeded {:?}; body was not submitted",
                staged.pane, SUBMIT_STAGE_LIMIT
            )));
        }
        self.require_staged(staged)?;
        Ok(())
    }

    fn finish(&self, operation: &str, output: Output) -> Result<Output> {
        if output.status.success() {
            return Ok(output);
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let status = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |code| format!("exit {code}"));
        Err(self.error(format!(
            "{operation} failed ({status}): {} — run `tmux list-panes -a` to verify the target",
            if stderr.is_empty() {
                "tmux returned no diagnostic"
            } else {
                &stderr
            }
        )))
    }

    fn stdout(&self, operation: &str, args: &[OsString]) -> Result<String> {
        if let Some(reply) = self.control.query(args) {
            return self.control_stdout(operation, reply);
        }
        let output = self.run(operation, args)?;
        String::from_utf8(output.stdout).map_err(|error| {
            self.error(format!(
                "{operation} returned non-UTF-8 output: {error} — set tmux output to UTF-8 and retry"
            ))
        })
    }

    /// The control client's answer, shaped exactly as the forked client's
    /// stdout or `finish` error would have been.
    fn control_stdout(&self, operation: &str, reply: control::Reply) -> Result<String> {
        match reply {
            Ok(lines) => {
                let mut stdout = Vec::new();
                for line in lines {
                    stdout.extend_from_slice(&line);
                    stdout.push(b'\n');
                }
                String::from_utf8(stdout).map_err(|error| {
                    self.error(format!(
                        "{operation} returned non-UTF-8 output: {error} — set tmux output to UTF-8 and retry"
                    ))
                })
            }
            Err(lines) => {
                let stderr = String::from_utf8_lossy(&lines.join(&b'\n'))
                    .trim()
                    .to_string();
                Err(self.error(format!(
                    "{operation} failed (exit 1): {} — run `tmux list-panes -a` to verify the target",
                    if stderr.is_empty() {
                        "tmux returned no diagnostic"
                    } else {
                        &stderr
                    }
                )))
            }
        }
    }

    fn spawn_error(&self, operation: &str, error: io::Error) -> PijError {
        let fix = if error.kind() == io::ErrorKind::NotFound {
            "install tmux and ensure its executable is on PATH"
        } else {
            "verify the tmux executable and retry"
        };
        self.error(format!(
            "could not start {:?} for {operation}: {error} — {fix}",
            self.binary
        ))
    }

    fn error(&self, message: String) -> PijError {
        PijError::Adapter {
            adapter: "tmux".to_string(),
            message,
        }
    }

    async fn wait_for_pipe_state(
        &self,
        pane: &str,
        sink: &Path,
        expected: &str,
        operation: &str,
    ) -> Result<()> {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(2);
        loop {
            let pipe = self.stdout(
                &format!("verify pipe-pane {operation}"),
                &os_args(["display-message", "-p", "-t", pane, "#{pane_pipe}"]),
            )?;
            let observed = pipe.trim();
            if observed == expected {
                return Ok(());
            }
            if !matches!(observed, "0" | "1") {
                return Err(self.error(format!(
                    "pane_pipe for {pane:?} was {observed:?} while verifying {operation}, expected 0 or 1"
                )));
            }
            if Instant::now() >= deadline {
                return Err(self.error(format!(
                    "pipe-pane {operation} did not converge for pane {pane:?}, sink {sink:?}, within {:?}: expected pane_pipe={expected}, observed {observed} — ownership marker and sink retained; the next observer sweep will retry",
                    started.elapsed()
                )));
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    fn ensure_session(&self, session: &str) -> Result<()> {
        if self.session.as_deref().is_none_or(|owned| owned == session) {
            return Ok(());
        }
        Err(self.error(format!(
            "refusing new-window in session {session:?}: this adapter is scoped to {:?} — use an adapter scoped to the requested session",
            self.session.as_deref().unwrap_or_default()
        )))
    }

    fn expected_tap_sink(&self, pane: &str) -> PathBuf {
        tap_sink_path(&self.tap_root, pane)
    }

    fn has_live_legacy_tap_writer(&self, pane: &str) -> Result<bool> {
        let Some(root) = &self.legacy_tap_root else {
            return Ok(false);
        };
        let sink = tap_sink_path(root, pane);
        let metadata = match fs::symlink_metadata(&sink) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(self.error(format!(
                    "could not inspect legacy tap signature {sink:?} for pane {pane:?}: {error}"
                )));
            }
        };
        if !metadata.file_type().is_file() {
            return Ok(false);
        }
        let sink_text = sink.to_str().ok_or_else(|| {
            self.error(format!(
                "legacy tap sink {sink:?} is not valid Unicode — refusing to replace pane {pane:?}"
            ))
        })?;
        pij_core::spawns::record("ps.tmux-legacy-tap-writer");
        let output = Command::new("/bin/ps")
            .args(["-axo", "command="])
            .output()
            .map_err(|error| {
                self.error(format!("could not inspect legacy tap writers: {error}"))
            })?;
        if !output.status.success() {
            return Err(self.error(format!(
                "could not inspect legacy tap writers: ps exited {} ({})",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|command| command.contains("cat >>") && command.contains(sink_text)))
    }

    fn tap_marker(&self, pane: &str) -> Result<Option<String>> {
        let value = self.stdout(
            "show-options tap owner",
            &os_args([
                "show-options",
                "-p",
                "-v",
                "-q",
                "-t",
                pane,
                TAP_SINK_OPTION,
            ]),
        )?;
        let value = value.trim_end();
        Ok((!value.is_empty()).then(|| value.to_string()))
    }

    fn set_tap_marker(&self, pane: &str, sink: &str) -> Result<()> {
        self.run(
            "set-options tap owner",
            &os_args(["set-option", "-p", "-t", pane, TAP_SINK_OPTION, sink]),
        )?;
        Ok(())
    }

    fn clear_tap_marker(&self, pane: &str) -> Result<()> {
        self.run(
            "clear-options tap owner",
            &os_args(["set-option", "-p", "-u", "-t", pane, TAP_SINK_OPTION]),
        )?;
        Ok(())
    }

    fn submit_marker(&self, pane: &str) -> Result<Option<String>> {
        let value = self.stdout(
            "show-options submit owner",
            &os_args([
                "show-options",
                "-p",
                "-v",
                "-q",
                "-t",
                pane,
                SUBMIT_OWNER_OPTION,
            ]),
        )?;
        let value = value.trim_end();
        Ok((!value.is_empty()).then(|| value.to_string()))
    }

    fn mint_submit_token(&self) -> Result<String> {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| {
            self.error(format!(
                "the OS refused randomness for staged-submit ownership: {error}"
            ))
        })?;
        let mut token = String::with_capacity(43);
        token.push_str("pij-submit-");
        for byte in bytes {
            write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Ok(token)
    }

    fn run_if_submit_token(
        &self,
        pane: &str,
        token: &str,
        operation: &str,
        command: &str,
    ) -> Result<Output> {
        let owner_matches = format!("#{{==:#{{{SUBMIT_OWNER_OPTION}}},{token}}}");
        self.run(
            operation,
            &os_args([
                "if-shell",
                "-F",
                "-t",
                pane,
                &owner_matches,
                command,
                "run-shell 'exit 75'",
            ]),
        )
    }

    fn reserve_submit_marker(&self, staged: &StagedSubmit) -> Result<()> {
        let target = shell_quote(&staged.pane);
        let token = shell_quote(&staged.token);
        // `run-shell` expands tmux formats while parsing this reservation. Escape
        // them once so the watchdog's owner predicates are evaluated at its
        // deadline, not frozen from acquisition time.
        let watchdog_script = self.submit_recovery_script(staged).replace("#{", "##{");
        let watchdog = shell_quote(&watchdog_script);
        let reserve = format!(
            "set-option -p -t {target} {SUBMIT_OWNER_OPTION} {token} ; run-shell -b {watchdog} ; select-pane -d -t {target}"
        );
        let unowned_and_enabled =
            format!("#{{&&:#{{==:#{{{SUBMIT_OWNER_OPTION}}},}},#{{==:#{{pane_input_off}},0}}}}");
        self.run(
            "atomically reserve staged submit",
            &os_args([
                "if-shell",
                "-F",
                "-t",
                &staged.pane,
                &unowned_and_enabled,
                &reserve,
            ]),
        )?;
        match self.submit_marker(&staged.pane)? {
            Some(owner) if owner == staged.token => Ok(()),
            Some(owner) => Err(self.error(format!(
                "pane {:?} already has staged submit {owner:?}; inspect the composer and press Ctrl-C to clear it before removing {SUBMIT_OWNER_OPTION}",
                staged.pane
            ))),
            None if self.pane_input_disabled(&staged.pane)? => Err(self.error(format!(
                "pane {:?} input is already disabled by its operator; refusing to take ownership",
                staged.pane
            ))),
            None => Err(self.error(format!(
                "tmux did not grant staged-submit ownership for pane {:?}",
                staged.pane
            ))),
        }
    }

    fn pane_input_disabled(&self, pane: &str) -> Result<bool> {
        match self
            .stdout(
                "display-message pane_input_off",
                &os_args(["display-message", "-p", "-t", pane, "#{pane_input_off}"]),
            )?
            .trim()
        {
            "1" => Ok(true),
            "0" => Ok(false),
            other => Err(self.error(format!(
                "pane_input_off for {pane:?} was {other:?}, expected 0 or 1"
            ))),
        }
    }

    fn require_staged(&self, staged: &StagedSubmit) -> Result<()> {
        if self.submit_marker(&staged.pane)?.as_deref() != Some(&staged.token) {
            return Err(self.error(format!(
                "staged submit ownership for pane {:?} no longer matches token {:?}",
                staged.pane, staged.token
            )));
        }
        if !self.pane_input_disabled(&staged.pane)? {
            return Err(self.error(format!(
                "pane {:?} input was restored before its staged submit completed",
                staged.pane
            )));
        }
        Ok(())
    }

    fn restore_submit_token(&self, pane: &str, token: &str) -> Result<()> {
        let target = shell_quote(pane);
        let restore = format!(
            "select-pane -e -t {target} ; set-option -p -u -t {target} {SUBMIT_OWNER_OPTION}"
        );
        self.run_if_submit_token(
            pane,
            token,
            "restore pane input and release ownership",
            &restore,
        )?;
        Ok(())
    }

    fn restore_staged_input(&self, staged: &StagedSubmit) -> Result<()> {
        self.restore_submit_token(&staged.pane, &staged.token)
    }

    fn submit_recovery_script(&self, staged: &StagedSubmit) -> String {
        let mut tmux = shell_quote(&self.binary.to_string_lossy());
        if let Some(server) = &self.server {
            tmux.push_str(" -L ");
            tmux.push_str(&shell_quote(server));
        }
        let pane = shell_quote(&staged.pane);
        let target = shell_quote(&staged.pane);
        let active = &staged.token;
        let recovering = format!("{active}-recovering");
        let completed = format!("{active}-committed");
        let active_matches = format!("#{{==:#{{{SUBMIT_OWNER_OPTION}}},{active}}}");
        let recovering_matches = format!("#{{==:#{{{SUBMIT_OWNER_OPTION}}},{recovering}}}");
        let completed_matches = format!("#{{==:#{{{SUBMIT_OWNER_OPTION}}},{completed}}}");
        let transition = format!(
            "set-option -p -t {target} {SUBMIT_OWNER_OPTION} {}",
            shell_quote(&recovering)
        );
        let clear = format!("set-option -p -u -t {target} {SUBMIT_OWNER_OPTION}");
        let read_owner = format!("$({tmux} show-options -p -v -q -t {pane} {SUBMIT_OWNER_OPTION})");
        let clear_recovering = format!(
            "{tmux} if-shell -F -t {pane} {} {} {}",
            shell_quote(&recovering_matches),
            shell_quote(&clear),
            shell_quote("run-shell 'exit 75'")
        );
        let clear_completed = format!(
            "{tmux} if-shell -F -t {pane} {} {} {}",
            shell_quote(&completed_matches),
            shell_quote(&clear),
            shell_quote("run-shell 'exit 75'")
        );
        let title = shell_quote("PIJ STAGED UNSENT — inspect composer; Ctrl-C clears");
        format!(
            "sleep {SUBMIT_RECOVERY_LIMIT_SECS}; \
             {tmux} if-shell -F -t {pane} {} {}; \
             owner={read_owner}; \
             if [ \"$owner\" = {} ]; then \
               while [ \"$owner\" = {} ]; do \
                 if {tmux} select-pane -e -t {pane}; then \
                   {tmux} select-pane -T {title} -t {pane}; \
                   if {clear_recovering}; then break; fi; \
                 fi; \
                 sleep 0.05; owner={read_owner}; \
               done; \
             elif [ \"$owner\" = {} ]; then \
               while [ \"$owner\" = {} ]; do \
                 if {tmux} select-pane -e -t {pane}; then \
                   {tmux} select-pane -T {title} -t {pane}; \
                   if {clear_completed}; then break; fi; \
                 fi; \
                 sleep 0.05; owner={read_owner}; \
               done; \
             fi",
            shell_quote(&active_matches),
            shell_quote(&transition),
            shell_quote(&recovering),
            shell_quote(&recovering),
            shell_quote(&completed),
            shell_quote(&completed)
        )
    }

    fn recover_acquire_error(&self, staged: &StagedSubmit, error: PijError) -> PijError {
        match self.restore_staged_input(staged) {
            Ok(()) => error,
            Err(recovery) => self.error(format!(
                "{error}; failed to restore pane input after acquisition error: {recovery}"
            )),
        }
    }

    fn abort_staged_inner(&self, staged: &StagedSubmit) -> Result<()> {
        self.require_staged(staged)?;
        let notice_result = if staged.staged {
            // Close a paste the stage opened, so the notice is its own paste and
            // not nested inside an unterminated one (plan 184 review S4).
            let close = if staged.paste_open { "\x1b[201~" } else { "" };
            let notice = format!("{close}\x1b[200~\n{STAGED_SUBMIT_RECOVERY}\x1b[201~");
            self.paste_bytes_owned(staged, notice.as_bytes())
        } else {
            Ok(())
        };
        let restore_result = self.restore_staged_input(staged);
        match (notice_result, restore_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(restore)) => Err(self.error(format!(
                "{error}; additionally failed to restore pane input: {restore}"
            ))),
        }
    }
}

#[async_trait]
impl TmuxPort for TmuxAdapter {
    async fn list_panes(&self) -> Result<Vec<Pane>> {
        let output = self.stdout("list-panes", &self.list_args())?;
        parse_panes(&output)
    }

    async fn pane_process(&self, pane: &str) -> Result<Option<PaneProcess>> {
        // ONE tmux call for both facts, so pid and cwd cannot describe two
        // different moments of the same pane.
        let Ok(output) = self.stdout(
            "display-message pane_pid/pane_current_path",
            &os_args([
                "display-message",
                "-p",
                "-t",
                pane,
                "#{pane_pid}\t#{pane_current_path}",
            ]),
        ) else {
            // tmux failed. Ask the ONE question that separates the two causes: is
            // the pane gone (an answer adoption needs, and a refusal it must make)
            // or did tmux itself fail (an error nothing should swallow)? Same
            // pattern the detach path already uses.
            return if self
                .list_panes()
                .await?
                .iter()
                .any(|listed| listed.id == pane)
            {
                Err(self.error(format!(
                    "could not read pane_pid/pane_current_path for {pane:?} though tmux still lists it"
                )))
            } else {
                Ok(None)
            };
        };
        let line = output.trim_end_matches('\n');
        let Some((pid, cwd)) = line.split_once('\t') else {
            return Err(self.error(format!(
                "display-message for {pane:?} returned {line:?}, expected `<pid>\\t<path>` — update pij-tmux for this tmux wire shape"
            )));
        };
        let pid = pid.trim().parse::<u32>().map_err(|error| {
            self.error(format!(
                "pane_pid for {pane:?} was {pid:?}, which is not a pid: {error}"
            ))
        })?;
        Ok(Some(PaneProcess {
            pid,
            cwd: cwd.to_string(),
        }))
    }

    async fn acquire_submit(&self, pane: &str) -> Result<StagedSubmit> {
        let _submit = self.submit_lock.lock().await;
        let staged = StagedSubmit {
            pane: pane.to_string(),
            token: self.mint_submit_token()?,
            staged: false,
            pasted: String::new(),
            paste_open: false,
        };
        self.reserve_submit_marker(&staged)?;

        // The tmux server serializes reservation, watchdog scheduling, and input
        // disable as one successful CAS branch. A daemon pause cannot split
        // those effects; a competing adapter can only observe the state before
        // all three or after all three.
        if let Err(error) = self.require_staged(&staged) {
            return Err(self.recover_acquire_error(&staged, error));
        }
        Ok(staged)
    }

    async fn stage_submit(&self, staged: &mut StagedSubmit, text: &str) -> Result<()> {
        let refusal = if text.len() > MAX_TYPED_FRAME_BYTES {
            Some(self.error(format!(
                "framed pane body is {} bytes; maximum supported pane transaction is {MAX_TYPED_FRAME_BYTES} bytes — body was not typed",
                text.len()
            )))
        } else if let Some((offset, control)) = unsupported_terminal_control(text) {
            Some(self.error(format!(
                "cannot type terminal control U+{:04X} at byte {offset} losslessly — use socket delivery or keep the body queued for inbox pull",
                u32::from(control)
            )))
        } else {
            None
        };
        if let Some(error) = refusal {
            return match self.abort_submit(staged).await {
                Ok(()) => Err(error),
                Err(recovery) => {
                    Err(self.error(format!("{error}; failed to restore pane input: {recovery}")))
                }
            };
        }
        let _submit = self.submit_lock.lock().await;
        self.require_staged(staged)?;
        if staged.staged {
            let error = self.error(format!(
                "staged submit {:?} already contains composer text",
                staged.token
            ));
            return match self.abort_staged_inner(staged) {
                Ok(()) => Err(error),
                Err(recovery) => {
                    Err(self.error(format!("{error}; failed to restore pane input: {recovery}")))
                }
            };
        }
        staged.staged = true;
        if let Err(error) = self.stage_body_owned(staged, text).await {
            return match self.abort_staged_inner(staged) {
                Ok(()) => Err(error),
                Err(recovery) => Err(self.error(format!(
                    "{error}; staged-submit recovery also failed: {recovery}"
                ))),
            };
        }
        Ok(())
    }

    async fn commit_submit(&self, staged: &StagedSubmit) -> Result<()> {
        let _submit = self.submit_lock.lock().await;
        self.require_staged(staged)?;
        if !staged.staged {
            let error = self.error(format!(
                "staged submit {:?} has no composer text to commit",
                staged.token
            ));
            return match self.abort_staged_inner(staged) {
                Ok(()) => Err(error),
                Err(recovery) => {
                    Err(self.error(format!("{error}; failed to restore pane input: {recovery}")))
                }
            };
        }

        // Transition ownership BEFORE Enter. If this write fails, Enter is never
        // attempted. After Enter succeeds, no cleanup operation may relabel the
        // delivered turn as failure and cause a duplicate retry.
        let completed = format!("{}-committed", staged.token);
        let target = shell_quote(&staged.pane);
        let transition = format!(
            "set-option -p -t {target} {SUBMIT_OWNER_OPTION} {}",
            shell_quote(&completed)
        );
        if let Err(error) = self.run_if_submit_token(
            &staged.pane,
            &staged.token,
            "authorize staged submit commit",
            &transition,
        ) {
            return match self.abort_staged_inner(staged) {
                Ok(()) => Err(error),
                Err(recovery) => Err(self.error(format!(
                    "{error}; failed to restore pane input after commit authorization error: {recovery}"
                ))),
            };
        }

        let enter = format!("select-pane -e -t {target} ; send-keys -t {target} Enter");
        if let Err(error) = self.run_if_submit_token(
            &staged.pane,
            &completed,
            "submit authorized staged body",
            &enter,
        ) {
            let mut committing = staged.clone();
            committing.token = completed;
            return match self.abort_staged_inner(&committing) {
                Ok(()) => Err(error),
                Err(recovery) => Err(self.error(format!(
                    "{error}; failed to restore pane input after Enter error: {recovery}"
                ))),
            };
        }

        // Enter is the delivery receipt. Marker/input cleanup is independently
        // recoverable by the already-armed watchdog and cannot change outcome.
        let _ = self.restore_submit_token(&staged.pane, &completed);
        Ok(())
    }

    async fn abort_submit(&self, staged: &StagedSubmit) -> Result<()> {
        let _submit = self.submit_lock.lock().await;
        self.abort_staged_inner(staged)
    }

    async fn clear_staged_leftover(
        &self,
        staged: &StagedSubmit,
        leftover: &str,
        observed: &str,
    ) -> Result<()> {
        let _submit = self.submit_lock.lock().await;
        self.require_staged(staged)?;
        if staged.staged {
            return Err(self.error(format!(
                "staged submit {:?} already staged text; a leftover is cleared only before staging",
                staged.token
            )));
        }
        if !is_rendered_leftover(observed, leftover) {
            return Err(self.error(format!(
                "composer of pane {:?} is not exactly pij's own staged leftover; nothing cleared",
                staged.pane
            )));
        }
        // One BSpace per character of the exact leftover, with input re-enabled
        // only inside this server-side command and only while the token owns it.
        let chars = leftover.chars().count();
        let target = shell_quote(&staged.pane);
        let clear = format!(
            "select-pane -e -t {target} ; send-keys -t {target} -N {chars} BSpace ; select-pane -d -t {target}"
        );
        self.run_if_submit_token(
            &staged.pane,
            &staged.token,
            "clear own staged leftover",
            &clear,
        )?;
        // Still owned: give the pane time to render before the caller re-reads.
        sleep(CLEAR_SETTLE).await;
        Ok(())
    }

    async fn submit(&self, pane: &str, text: &str) -> Result<()> {
        let mut staged = self.acquire_submit(pane).await?;
        self.stage_submit(&mut staged, text).await?;
        self.commit_submit(&staged).await
    }

    async fn send_keys(&self, pane: &str, keys: &str) -> Result<()> {
        self.run("send-keys", &os_args(["send-keys", "-t", pane, "-l", keys]))?;
        Ok(())
    }

    async fn capture(&self, pane: &str, lines: u32) -> Result<String> {
        if lines == 0 {
            return Ok(String::new());
        }
        // Preserve physical rows: tmux cursor_y is physical, while `-J` joins
        // soft-wrapped rows and makes composer extraction compare different axes.
        let output = self.stdout("capture-pane", &os_args(["capture-pane", "-p", "-t", pane]))?;
        let rows: Vec<&str> = output.lines().collect();
        let keep = usize::try_from(lines).unwrap_or(usize::MAX).min(rows.len());
        Ok(rows[rows.len() - keep..].join("\n"))
    }

    async fn attach_pane_tap(&self, pane: &str, sink: &Path) -> Result<()> {
        let expected = self.expected_tap_sink(pane);
        if sink != expected {
            return Err(self.error(format!(
                "tap sink {sink:?} does not match this adapter's expected sink {expected:?} for pane {pane:?}"
            )));
        }
        let tracked = {
            let taps = self.taps.lock().expect("tmux tap mutex");
            match taps.get(pane) {
                Some(existing) if existing.path == sink => true,
                Some(existing) => {
                    return Err(self.error(format!(
                        "pane {pane:?} already has a tap at {:?} — detach it before changing sinks",
                        existing.path
                    )));
                }
                None => false,
            }
        };
        let sink_text = sink.to_str().ok_or_else(|| {
            self.error(format!(
                "tap sink {sink:?} is not valid Unicode — choose a UTF-8 state directory"
            ))
        })?;
        let marker = self.tap_marker(pane)?;
        let marker_matches = marker.as_deref() == Some(sink_text);
        match self
            .stdout(
                "display-message pane_pipe",
                &os_args(["display-message", "-p", "-t", pane, "#{pane_pipe}"]),
            )?
            .trim()
        {
            "0" => {}
            "1" if tracked && marker_matches => return Ok(()),
            // A matching pane-scoped marker is durable ownership evidence from a
            // prior daemon. Replacing that orphan is adoption, not clobbering.
            "1" if marker_matches => {}
            "1" if marker.is_none() && self.has_live_legacy_tap_writer(pane)? => {
                // These are one-directional safety interlocks, not retention policy:
                // the absent marker, regular sink, and live associated writer can
                // only spare a pipe. Removing any check would close the same set or more.
                self.run(
                    "pipe-pane detach legacy",
                    &os_args(["pipe-pane", "-t", pane]),
                )?;
                let legacy_sink = self
                    .legacy_tap_root
                    .as_ref()
                    .map(|root| tap_sink_path(root, pane))
                    .expect("legacy signature requires an injected root");
                self.wait_for_pipe_state(pane, &legacy_sink, "0", "legacy supersede")
                    .await?;
            }
            "1" => {
                return Err(self.error(format!(
                    "pane {pane:?} already has an output pipe not owned by this sink — refusing to replace it"
                )));
            }
            other => {
                return Err(self.error(format!(
                    "pane_pipe for {pane:?} was {other:?}, expected 0 or 1 — update pij-tmux for this tmux version"
                )));
            }
        }
        if let Some(parent) = sink.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                self.error(format!(
                    "could not create tap directory {parent:?}: {error}"
                ))
            })?;
        }
        // Ownership before the resource: a crash after this line leaves a ledger
        // entry with no file, which the sweep drops; the inverse order would
        // leave a sink no server's sweep may ever remove.
        let sink_name = sink
            .file_name()
            .and_then(|name| name.to_str())
            .expect("expected tap sinks are named by tap_sink_path");
        self.tap_ledger().record(sink_name).map_err(|error| {
            self.error(format!(
                "could not record tap ownership for {sink:?}: {error}"
            ))
        })?;
        let sink_file = OpenOptions::new()
            .create(true)
            .truncate(!tracked)
            .write(true)
            .mode(0o600)
            .open(sink)
            .map_err(|error| self.error(format!("could not create tap sink {sink:?}: {error}")))?;
        sink_file
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                self.error(format!(
                    "could not secure tap sink {sink:?} to mode 0600: {error}"
                ))
            })?;
        drop(sink_file);

        // Persist ownership before creating the resource. A crash after this
        // write but before pipe creation leaves a marker plus pane_pipe=0, which
        // the next daemon can complete. The inverse order leaves an unmarked
        // orphan that no future process may safely adopt.
        self.set_tap_marker(pane, sink_text)?;

        let command = format!("cat >> {}", shell_quote(sink_text));
        // `-o` is intentionally absent. tmux defines it as a toggle: a repeated
        // attach closes an existing pipe. The marker above distinguishes our
        // restart orphan from a foreign pipe before this replacement can happen.
        if let Err(error) = self.run(
            "pipe-pane attach",
            &os_args(["pipe-pane", "-O", "-t", pane, &command]),
        ) {
            let _ = self.clear_tap_marker(pane);
            if !tracked {
                let _ = fs::remove_file(sink);
            }
            return Err(error);
        }
        self.wait_for_pipe_state(pane, sink, "1", "attach").await?;
        if !tracked {
            self.taps.lock().expect("tmux tap mutex").insert(
                pane.to_string(),
                TapState {
                    path: sink.to_path_buf(),
                    offset: 0,
                },
            );
        }
        Ok(())
    }

    async fn pane_tap_sink(&self, pane: &str) -> Result<Option<PathBuf>> {
        Ok(self.tap_marker(pane)?.map(PathBuf::from))
    }

    async fn drain_pane_tap(&self, pane: &str) -> Result<Vec<u8>> {
        let mut taps = self.taps.lock().expect("tmux tap mutex");
        let tap = taps.get_mut(pane).ok_or_else(|| {
            self.error(format!(
                "pane {pane:?} has no attached tap — attach it before draining"
            ))
        })?;
        // Stat before open: an unchanged tap costs no open(), which Defender
        // inspects (plan 185, D4: two opens per pane per 600 ms tick).
        let length = fs::metadata(&tap.path)
            .map_err(|error| {
                self.error(format!(
                    "could not inspect tap sink {:?}: {error}",
                    tap.path
                ))
            })?
            .len();
        if length < tap.offset {
            // A sink that shrank under us is read from its new end.
            tap.offset = length;
            return Ok(Vec::new());
        }
        if length == tap.offset {
            // Nothing new since the last drain. Trim here, while the pane is
            // quiet, never mid-burst: see `trim_consumed_tap`.
            if length >= TAP_TRIM_BYTES
                && trim_consumed_tap(&tap.path, length).map_err(|error| {
                    self.error(format!("could not trim tap sink {:?}: {error}", tap.path))
                })?
            {
                tap.offset = 0;
            }
            return Ok(Vec::new());
        }
        let mut file = File::open(&tap.path).map_err(|error| {
            self.error(format!("could not open tap sink {:?}: {error}", tap.path))
        })?;
        file.seek(SeekFrom::Start(tap.offset)).map_err(|error| {
            self.error(format!("could not seek tap sink {:?}: {error}", tap.path))
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|error| {
            self.error(format!("could not drain tap sink {:?}: {error}", tap.path))
        })?;
        tap.offset += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Ok(bytes)
    }

    async fn sweep_orphan_taps(&self) -> Result<TapSweep> {
        let ledger = self.tap_ledger();
        let ledger_error =
            |error: io::Error| self.error(format!("could not use the tap ledger: {error}"));
        let mut owned = ledger.read().map_err(ledger_error)?;
        let mut sweep = TapSweep::default();
        if self.claims_legacy_sinks && !ledger.legacy_claimed() {
            // One-time claim of the sinks written before ledgers existed (plan
            // 185 ruling): only the default server's adapter claims, and the
            // claim only adds ownership. The pane-absence test below still
            // decides every removal.
            // A sink another server's ledger names is that server's.
            let others = ledger.others().map_err(ledger_error)?;
            for name in unledgered_sinks(&self.tap_root).map_err(ledger_error)? {
                if !others.contains(&name) && owned.insert(name) {
                    sweep.claimed += 1;
                }
            }
            ledger.rewrite(&owned).map_err(ledger_error)?;
            ledger.mark_legacy_claimed().map_err(ledger_error)?;
        }
        if owned.is_empty() {
            return Ok(sweep);
        }
        let live: BTreeSet<String> =
            parse_panes(&self.stdout("list-panes", &self.server_list_args())?)?
                .iter()
                .map(|pane| sink_file_name(&pane.id))
                .chain(
                    self.taps
                        .lock()
                        .expect("tmux tap mutex")
                        .keys()
                        .map(|pane| sink_file_name(pane)),
                )
                .collect();
        let now = SystemTime::now();
        for name in owned.clone() {
            if live.contains(&name) || !is_sink_name(&name) {
                continue;
            }
            let path = self.tap_root.join(&name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    owned.remove(&name);
                    continue;
                }
                Err(error) => {
                    return Err(self.error(format!("could not inspect tap sink {path:?}: {error}")));
                }
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            let recent = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_none_or(|age| age < ORPHAN_TAP_GRACE);
            if recent {
                sweep.spared_recent += 1;
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    sweep.removed += 1;
                    sweep.removed_bytes += metadata.len();
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(self.error(format!(
                        "could not remove orphaned tap sink {path:?}: {error}"
                    )));
                }
            }
            owned.remove(&name);
        }
        ledger.rewrite(&owned).map_err(ledger_error)?;
        Ok(sweep)
    }

    async fn detach_pane_tap(&self, pane: &str) -> Result<()> {
        let local_path = self
            .taps
            .lock()
            .expect("tmux tap mutex")
            .get(pane)
            .map(|tap| tap.path.clone());
        let path = match local_path {
            Some(path) => path,
            None => {
                let marker = self.tap_marker(pane)?.map(PathBuf::from).ok_or_else(|| {
                    self.error(format!("pane {pane:?} has no owned tap to detach"))
                })?;
                let expected = self.expected_tap_sink(pane);
                if marker != expected {
                    return Err(self.error(format!(
                        "refusing to detach foreign tap on pane {pane:?}: marker {marker:?} does not match expected {expected:?}"
                    )));
                }
                marker
            }
        };

        let detach = self.run("pipe-pane detach", &os_args(["pipe-pane", "-t", pane]));
        let pane_exists = self
            .list_panes()
            .await?
            .iter()
            .any(|listed| listed.id == pane);
        if let Err(error) = detach
            && pane_exists
        {
            return Err(error);
        }
        if pane_exists {
            self.wait_for_pipe_state(pane, &path, "0", "detach").await?;
            self.clear_tap_marker(pane)?;
        }

        self.taps.lock().expect("tmux tap mutex").remove(pane);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(self.error(format!("could not remove tap sink {path:?}: {error}"))),
        }
    }

    async fn kill(&self, pane: &str) -> Result<()> {
        // This fresh resolution is a one-directional brake: removing it can only
        // make kill-pane happen more often. It never chooses a different target.
        if !self
            .list_panes()
            .await?
            .iter()
            .any(|listed| listed.id == pane)
        {
            return Err(self.error(format!(
                "refusing kill-pane for {pane:?}: a fresh list-panes did not resolve that id — refresh the pane listing and retry"
            )));
        }
        self.run("kill-pane", &os_args(["kill-pane", "-t", pane]))?;
        Ok(())
    }

    async fn new_window(
        &self,
        session: &str,
        name: &str,
        cwd: &str,
        command: Option<&LaunchCommand>,
    ) -> Result<Pane> {
        self.ensure_session(session)?;
        let output = self.stdout("new-window", &new_window_args(session, name, cwd, command))?;
        parse_one_pane(&output, "new-window")
    }

    async fn user_typing(&self, pane: &str) -> Result<bool> {
        let output = self.stdout(
            "display-message pane_in_mode",
            &os_args(["display-message", "-p", "-t", pane, "#{pane_in_mode}"]),
        )?;
        match output.trim() {
            "1" => Ok(true),
            "0" => Ok(false),
            other => Err(self.error(format!(
                "pane_in_mode for {pane:?} was {other:?}, expected 0 or 1 — update pij-tmux for this tmux version"
            ))),
        }
    }
}

fn os_args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn new_window_args(
    session: &str,
    name: &str,
    cwd: &str,
    command: Option<&LaunchCommand>,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("new-window"),
        OsString::from("-d"),
        OsString::from("-P"),
        OsString::from("-F"),
        OsString::from(LIST_FORMAT),
        OsString::from("-t"),
        OsString::from(format!("{session}:")),
        OsString::from("-n"),
        OsString::from(name),
        OsString::from("-c"),
        OsString::from(cwd),
    ];
    if let Some(command) = command {
        args.push(OsString::from("--"));
        args.push(OsString::from(&command.executable));
        args.extend(command.args.iter().map(OsString::from));
    }
    args
}

/// Harness selection and the process identities observed in its pane subtree.
#[derive(Debug, PartialEq, Eq)]
pub struct HarnessProcess {
    /// Unique nearest matching descendant, or the pane root on absence/ambiguity.
    pub identity: ProcIdentity,
    /// Complete observed subtree, including the pane root. A caller may select
    /// an identity from this evidence, but must corroborate its current liveness.
    pub subtree: Vec<ProcIdentity>,
}

/// Observe the selected harness identity, retaining the original identity-only API.
#[must_use]
pub fn harness_process(pane_pid: u32, harness: Harness) -> Option<ProcIdentity> {
    harness_process_tree(pane_pid, harness).map(|observed| observed.identity)
}

/// Observe a unique nearest matching descendant and the complete pane subtree.
///
/// `proc_start` uses the same LOCAL C-locale wall time as `ProcLiveness`.
/// A missing/dead pane yields `None`; callers label a pane fallback explicitly.
#[must_use]
pub fn harness_process_tree(pane_pid: u32, harness: Harness) -> Option<HarnessProcess> {
    pij_core::spawns::record("ps.tmux-harness-process-tree");
    let output = Command::new("/bin/ps")
        .args(["-Awwo", "pid=,ppid=,lstart=,command="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let processes: Vec<_> = text.lines().filter_map(process_row).collect();
    let selected = harness_process_from_snapshot(pane_pid, harness, &processes)?;
    pij_core::spawns::record("ps.tmux-harness-process-tree");
    let output = Command::new("/bin/ps")
        .args(["-p", &selected.identity.pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let start =
        pij_core::model::parse_process_start(std::str::from_utf8(&output.stdout).ok()?).ok()?;
    (start == selected.identity.proc_start).then_some(selected)
}

fn process_field<'a>(rest: &mut &'a str) -> Option<&'a str> {
    *rest = rest.trim_start();
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let (field, remaining) = rest.split_at(end);
    *rest = remaining;
    Some(field)
}

fn process_row(line: &str) -> Option<(ProcIdentity, u32, &str)> {
    let mut rest = line;
    let pid = process_field(&mut rest)?.parse().ok()?;
    let parent = process_field(&mut rest)?.parse().ok()?;
    let start = rest;
    for _ in 0..5 {
        process_field(&mut rest)?;
    }
    let proc_start =
        pij_core::model::parse_process_start(&start[..start.len() - rest.len()]).ok()?;
    Some((ProcIdentity { pid, proc_start }, parent, rest.trim_start()))
}

fn harness_process_from_snapshot(
    pane_pid: u32,
    harness: Harness,
    processes: &[(ProcIdentity, u32, &str)],
) -> Option<HarnessProcess> {
    let root = processes
        .iter()
        .find(|(proc, _, _)| proc.pid == pane_pid)?
        .0;
    let mut subtree = vec![root];
    let mut next = 0;
    let mut selected = None;
    while next < subtree.len() {
        let level_end = subtree.len();
        let mut matching = 0;
        let mut candidate = root;
        while next < level_end {
            let parent = subtree[next].pid;
            next += 1;
            for (proc, ppid, command) in processes {
                if *ppid != parent || subtree.iter().any(|seen| seen.pid == proc.pid) {
                    continue;
                }
                subtree.push(*proc);
                if selected.is_none() && harness_command(command, harness) {
                    matching += 1;
                    candidate = *proc;
                }
            }
        }
        if selected.is_none() && matching > 0 {
            // Like UDS discovery, equally near matches are ambiguous. Process
            // enumeration order must never choose a seat's identity.
            selected = Some(if matching == 1 { candidate } else { root });
        }
    }
    Some(HarnessProcess {
        identity: selected.unwrap_or(root),
        subtree,
    })
}

fn harness_command(command: &str, harness: Harness) -> bool {
    let mut args = command.split_whitespace();
    let Some(executable) = args
        .next()
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    if executable == harness.as_str() {
        return true;
    }
    if executable != "node" && executable != "bun" {
        return false;
    }
    let Some(script) = args.next() else {
        return false;
    };
    let name = Path::new(script).file_stem().and_then(|name| name.to_str());
    if name == Some(harness.as_str()) {
        return true;
    }
    let package = match harness {
        Harness::Claude => "/@anthropic-ai/claude-code/",
        Harness::Copilot => "/@github/copilot/",
        Harness::Codex => "/@openai/codex/",
        Harness::Pi => "/@earendil-works/pi-coding-agent/",
        Harness::Omp => "/@oh-my-pi/pi-coding-agent/",
    };
    script.contains(package)
}

/// Plan 182: tmux is resolved through PATH once and launched by the path found,
/// so a launch no longer searches PATH (one kernel process per miss on macOS).
/// The first PATH match still wins. Setting PATH here would need `unsafe`, so
/// the test re-runs itself as a child with a decoy `tmux` first on PATH.
#[cfg(test)]
mod resolve_once {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    use super::TmuxAdapter;

    const CHILD: &str = "PIJ_TMUX_RESOLVE_CHILD";

    #[test]
    fn tmux_launches_the_first_path_match_by_its_absolute_path() {
        let tap = std::env::temp_dir();
        if let Some(decoy) = std::env::var_os(CHILD) {
            let adapter = TmuxAdapter::new(&tap);
            assert_eq!(adapter.launched_program(), decoy);
            assert_eq!(adapter.control.program(), decoy);
            // An injected executable is launched as given.
            let fake = TmuxAdapter::with_binary("/opt/fake/tmux", &tap);
            assert_eq!(fake.launched_program(), "/opt/fake/tmux");
            return;
        }
        let root = pij_testkit::fresh_dir("pij-tmux-resolve");
        let (first, second) = (root.join("first"), root.join("second"));
        let mut decoy = OsString::new();
        for dir in [&first, &second] {
            std::fs::create_dir_all(dir).unwrap();
            let tmux = dir.join("tmux");
            std::fs::write(&tmux, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
            if decoy.is_empty() {
                decoy = tmux.into_os_string();
            }
        }
        let path = std::env::join_paths([&first, &second]).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "resolve_once::tmux_launches_the_first_path_match_by_its_absolute_path",
                "--exact",
            ])
            .env("PATH", path)
            .env(CHILD, &decoy)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _ = std::fs::remove_dir_all(root);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(test)]
mod control_defaults {
    use super::TmuxAdapter;

    /// The daemon composes `new` / `with_legacy_tap_root`: reads must ride the
    /// control client there. Fake executables keep plain forks unless opted in.
    #[test]
    fn production_constructors_use_control_mode_and_fakes_opt_in() {
        let tap = std::env::temp_dir();
        assert!(TmuxAdapter::new(&tap).control.enabled());
        assert!(
            TmuxAdapter::with_legacy_tap_root(&tap, &tap)
                .control
                .enabled()
        );
        assert!(TmuxAdapter::for_session("s", &tap).control.enabled());
        assert!(TmuxAdapter::for_server("srv", &tap).control.enabled());
        assert!(
            !TmuxAdapter::with_binary("fake-tmux", &tap)
                .control
                .enabled()
        );
        assert!(
            TmuxAdapter::with_binary("fake-tmux", &tap)
                .with_control_mode(true)
                .control
                .enabled()
        );
        assert!(
            !TmuxAdapter::new(&tap)
                .with_control_mode(false)
                .control
                .enabled()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::harness_process_from_snapshot;
    use super::new_window_args;
    use pij_core::model::{Harness, ProcIdentity};

    #[test]
    fn pane_shell_fixture_selects_claude_child() {
        let shell = ProcIdentity {
            pid: 64616,
            proc_start: 20260905080000,
        };
        let child = ProcIdentity {
            pid: 49862,
            proc_start: 20260905080100,
        };
        let processes = [(shell, 1, "-zsh"), (child, shell.pid, "/opt/bin/claude")];
        assert_eq!(
            harness_process_from_snapshot(shell.pid, Harness::Claude, &processes)
                .map(|observed| observed.identity),
            Some(child)
        );
    }

    #[test]
    fn descendant_walk_is_breadth_first_and_falls_back_only_to_the_pane() {
        let root = ProcIdentity {
            pid: 1,
            proc_start: 10,
        };
        let wrapper = ProcIdentity {
            pid: 2,
            proc_start: 11,
        };
        let grandchild = ProcIdentity {
            pid: 3,
            proc_start: 12,
        };
        let direct = ProcIdentity {
            pid: 4,
            proc_start: 13,
        };
        let processes = [
            (root, 0, "-zsh"),
            (wrapper, 1, "sh"),
            (grandchild, 2, "claude"),
            (direct, 1, "claude"),
        ];
        assert_eq!(
            harness_process_from_snapshot(1, Harness::Claude, &processes)
                .map(|observed| observed.identity),
            Some(direct)
        );
        assert_eq!(
            harness_process_from_snapshot(1, Harness::Omp, &processes)
                .map(|observed| observed.identity),
            Some(root)
        );
        assert_eq!(
            harness_process_from_snapshot(99, Harness::Claude, &processes),
            None
        );
        assert_eq!(
            harness_process_from_snapshot(2, Harness::Claude, &processes)
                .map(|observed| observed.identity),
            Some(grandchild)
        );
    }

    #[test]
    fn equally_near_harnesses_are_ambiguous_but_the_complete_subtree_is_retained() {
        let root = ProcIdentity {
            pid: 1,
            proc_start: 10,
        };
        let first = ProcIdentity {
            pid: 2,
            proc_start: 11,
        };
        let second = ProcIdentity {
            pid: 3,
            proc_start: 12,
        };
        let deep = ProcIdentity {
            pid: 4,
            proc_start: 13,
        };
        let outsider = ProcIdentity {
            pid: 5,
            proc_start: 14,
        };
        let mut rows = vec![
            (root, 0, "sh"),
            (first, 1, "claude"),
            (second, 1, "claude"),
            (deep, 2, "other"),
            (outsider, 99, "claude"),
        ];
        for _ in 0..2 {
            let observed = harness_process_from_snapshot(1, Harness::Claude, &rows).expect("root");
            assert_eq!(observed.identity, root);
            assert_eq!(observed.subtree.len(), 4);
            for member in [root, first, second, deep] {
                assert!(observed.subtree.contains(&member));
            }
            assert!(!observed.subtree.contains(&outsider));
            assert!(!observed.subtree.contains(&ProcIdentity {
                pid: first.pid,
                proc_start: 999
            }));
            rows.reverse();
        }
    }

    #[test]
    fn harness_commands_match_entrypoints_not_prompt_arguments() {
        for harness in [
            Harness::Claude,
            Harness::Copilot,
            Harness::Codex,
            Harness::Pi,
            Harness::Omp,
        ] {
            assert!(super::harness_command(
                &format!("/opt/bin/{} --model anything", harness.as_str()),
                harness
            ));
        }
        assert!(super::harness_command(
            "node /opt/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
            Harness::Pi
        ));
        assert!(super::harness_command(
            "bun /opt/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
            Harness::Omp
        ));
        assert!(!super::harness_command(
            "bun /opt/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
            Harness::Pi
        ));
        assert!(!super::harness_command("sh -c claude", Harness::Claude));
        assert!(!super::harness_command(
            "node unrelated.js --prompt claude",
            Harness::Claude
        ));
        assert!(!super::harness_command("claude-helper", Harness::Claude));
    }

    #[test]
    fn captured_live_interpreters_and_review_loader_define_the_matcher_boundary() {
        // Captured 2026-09-05 with ps -Awwo pid=,ppid=,command=; only the
        // user-home prefix is normalized. Main sweep: 31 bun/omp shims and
        // 17 bun.exe internal workers. Copilot npm-loader was captured live
        // DURING the cross-model review sweep, not claimed live at test time.
        assert!(super::harness_command(
            "bun /home/fixture/.npm-global/bin/omp",
            Harness::Omp
        ));
        assert!(!super::harness_command(
            "/home/fixture/.npm-global/lib/node_modules/bun/bin/bun.exe /home/fixture/.npm-global/lib/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
            Harness::Omp,
        ));
        assert!(super::harness_command(
            "node /home/fixture/.npm-global/lib/node_modules/@github/copilot/npm-loader.js --no-custom-instructions --disable-builtin-mcps --no-remote --no-auto-update --allow-tool=pij_send",
            Harness::Copilot,
        ));
        assert!(!super::harness_command(
            "node unrelated.js /home/fixture/.npm-global/lib/node_modules/@github/copilot/npm-loader.js",
            Harness::Copilot
        ));
    }

    #[test]
    fn process_snapshot_uses_canonical_local_start_encoding() {
        assert_eq!(
            super::process_row(" 49862 64616 Sat Sep  5 08:01:00 2026 /opt/bin/claude --model x"),
            Some((
                ProcIdentity {
                    pid: 49862,
                    proc_start: 20260905080100
                },
                64616,
                "/opt/bin/claude --model x"
            ))
        );
        assert!(super::process_row("49862 broken").is_none());
    }

    use pij_core::ports::LaunchCommand;

    /// A launchd-started daemon has no LANG, and tmux 3.6 then rewrites the tab
    /// in every `-p`/`-F` answer as `_`, so each pane read failed to parse
    /// (2026-09-27). The adapter's own command must not depend on the locale.
    #[test]
    fn tmux_output_keeps_tabs_without_a_utf8_locale() {
        if !std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return;
        }
        let server = format!("pij-utf8-{}", std::process::id());
        let start = std::process::Command::new("tmux")
            .args([
                "-L",
                &server,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "t",
                "cat",
            ])
            .status()
            .expect("start isolated tmux");
        assert!(start.success());
        let mut adapter = super::TmuxAdapter::new(std::env::temp_dir());
        adapter.server = Some(server.clone());
        let output = adapter
            .command(pij_core::program::resolve(&adapter.binary))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args([
                "display-message",
                "-p",
                "-t",
                "t",
                "#{session_name}\t#{session_name}",
            ])
            .output()
            .expect("tmux display-message");
        let _ = std::process::Command::new("tmux")
            .args(["-L", &server, "kill-server"])
            .status();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "t\tt\n");
    }

    #[test]
    fn command_reaches_tmux_as_discrete_argv() {
        let command = LaunchCommand {
            executable: "/path with spaces/claude".to_string(),
            args: vec!["--model".to_string(), "selector; not shell".to_string()],
        };
        let actual = new_window_args("session", "worker", "/abs/tree", Some(&command));
        let actual: Vec<_> = actual
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            &actual[actual.len() - 4..],
            [
                "--",
                "/path with spaces/claude",
                "--model",
                "selector; not shell",
            ]
        );
    }

    #[test]
    fn absent_command_preserves_empty_window_argv() {
        let actual = new_window_args("session", "worker", "/abs/tree", None);
        assert_eq!(actual.last().expect("cwd"), "/abs/tree");
        assert!(!actual.iter().any(|arg| arg == "--"));
    }
}
