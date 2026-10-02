//! Exact-pid process liveness for macOS and Linux.
//!
//! `ps -p <pid>` is deliberately narrower than the process-table snapshots used
//! by discovery: liveness may inspect only the pid the caller named. Removing
//! that constraint would make the adapter inspect more processes, so it is a
//! one-directional safety interlock (a brake), not policy.

use std::io;
use std::process::{Command, ExitStatus};
use std::sync::Arc;

use async_trait::async_trait;
use pij_core::error::{PijError, Result};
pub use pij_core::model::parse_process_start;
use pij_core::ports::LivenessPort;

const ADAPTER: &str = "process-liveness/ps";

/// The real [`LivenessPort`] backed by an exact-pid `ps` lookup.
///
/// The returned `u64` packs the C-locale `lstart` fields as
/// `YYYYMMDDhhmmss`. It is stable for the life of a process and ordered within
/// a boot, but callers compare it rather than interpreting it.
/// The fields remain in the host's local timezone; parsing does not convert
/// them to UTC. Comparing a UTC record requires explicit timebase conversion.
///
/// # Composition recipe
///
/// In `crates/daemon/src/lib.rs`:
///
/// ```ignore
/// use pij_harnesses::proc::ProcLiveness;
///
/// let liveness: Arc<dyn LivenessPort> = match config.adapters.liveness {
///     AdapterChoice::Fake => Arc::new(FakeLiveness::new()),
///     AdapterChoice::Real => Arc::new(ProcLiveness::new()),
/// };
/// ```
///
/// COMPOSED (wave 1). The refusal it replaced no longer exists; no new config
/// field is required.
pub struct ProcLiveness {
    runner: Arc<dyn CommandRunner>,
}

impl ProcLiveness {
    /// Construct the real process adapter.
    pub fn new() -> Self {
        Self {
            runner: Arc::new(PsRunner),
        }
    }

    #[cfg(test)]
    fn with_runner(runner: impl CommandRunner + 'static) -> Self {
        Self {
            runner: Arc::new(runner),
        }
    }
}

impl Default for ProcLiveness {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LivenessPort for ProcLiveness {
    async fn proc_start(&self, pid: u32) -> Result<Option<u64>> {
        let output = self.runner.run(pid).map_err(|error| PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!("could not inspect pid {pid}: {error}"),
        })?;
        classify(pid, output)
    }
}

struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

trait CommandRunner: Send + Sync {
    fn run(&self, pid: u32) -> io::Result<CommandOutput>;
}

struct PsRunner;

impl CommandRunner for PsRunner {
    fn run(&self, pid: u32) -> io::Result<CommandOutput> {
        let output = Command::new("ps")
            .args(["-o", "lstart=PIJ_LSTART", "-p", &pid.to_string()])
            .env("LC_ALL", "C")
            .output()?;
        Ok(CommandOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn classify(pid: u32, output: CommandOutput) -> Result<Option<u64>> {
    if !matches!(output.status.code(), Some(0) | Some(1)) || !output.stderr.is_empty() {
        return Err(PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!(
                "could not inspect pid {pid}: ps {}; {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| PijError::Adapter {
        adapter: ADAPTER.to_string(),
        message: format!("pid {pid} produced non-UTF-8 stdout: {error}"),
    })?;
    let mut lines = stdout.lines();
    if lines.next().map(str::trim) != Some("PIJ_LSTART") {
        return Err(PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!("pid {pid} produced no recognizable ps header: {stdout:?}"),
        });
    }
    match (output.status.code(), lines.next(), lines.next()) {
        // The explicit header proves ps ran its selection; an empty failed
        // command, diagnostic, or additional row is not this absence protocol.
        (Some(1), None, None) => Ok(None),
        (Some(0), Some(row), None) => parse_process_start(row.trim()).map(Some),
        _ => Err(PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!(
                "pid {pid} produced unexpected ps {} output: {stdout:?}; expected exit 0 with header and process row, or exit 1 with header only",
                output.status
            ),
        }),
    }
}

/// Read the host's current UTC offset in minutes from the platform clock.
///
/// Spawn binding uses this only for a fresh process, so the current offset is
/// the offset at process start; no historical timezone lookup is required.
///
/// # Errors
/// Returns an adapter error when `date +%z` fails or emits an invalid offset.
pub fn local_utc_offset_minutes() -> Result<i32> {
    let output = Command::new("date")
        .arg("+%z")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!("could not read local UTC offset: {error}"),
        })?;
    if !output.status.success() {
        return Err(PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: "date +%z failed while reading local UTC offset".to_string(),
        });
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|error| PijError::Adapter {
            adapter: ADAPTER.to_string(),
            message: format!("local UTC offset was not UTF-8: {error}"),
        })?
        .trim();
    parse_utc_offset(value).ok_or_else(|| PijError::Adapter {
        adapter: ADAPTER.to_string(),
        message: format!("invalid date +%z offset {value:?}; expected +HHMM or -HHMM"),
    })
}

/// One Claude process's own record, `<claude home>/sessions/<pid>.json`.
///
/// Claude writes it for every interactive process and rewrites `sessionId` when
/// the conversation changes, including a named `--resume <title>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeSessionRecord {
    /// The Claude process.
    pub pid: u32,
    /// The conversation that process is running.
    pub session_id: String,
    /// The process start, UTC, as Claude recorded it.
    pub proc_start: String,
    /// `session:@window.%pane`, when Claude ran inside tmux.
    pub tmux: Option<String>,
}

impl ClaudeSessionRecord {
    /// The tmux pane id, e.g. `%3`.
    #[must_use]
    pub fn pane(&self) -> Option<&str> {
        self.tmux.as_deref()?.rsplit_once('.').map(|(_, pane)| pane)
    }
}

/// Every readable Claude process record across `homes`.
#[must_use]
pub fn claude_session_records(homes: &[std::path::PathBuf]) -> Vec<ClaudeSessionRecord> {
    homes
        .iter()
        .filter_map(|home| std::fs::read_dir(home.join("sessions")).ok())
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| {
            let record: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            let text = |key: &str| record.get(key)?.as_str().map(str::to_string);
            Some(ClaudeSessionRecord {
                pid: u32::try_from(record.get("pid")?.as_u64()?).ok()?,
                session_id: text("sessionId")?,
                proc_start: text("procStart")?,
                tmux: text("tmux"),
            })
        })
        .collect()
}

/// The conversation `host` is running, derived from its own record (plan 156
/// ruling 2). Only a record whose pid IS `host` and whose recorded start matches
/// `host`'s observed start counts, so a stale file for a recycled pid derives
/// nothing.
#[must_use]
pub fn claude_session_of(
    homes: &[std::path::PathBuf],
    host: pij_core::model::ProcIdentity,
    utc_offset_minutes: i32,
) -> Option<String> {
    claude_session_records(homes)
        .into_iter()
        .find(|record| {
            record.pid == host.pid
                && process_start_matches_utc_record(
                    &record.proc_start,
                    host.proc_start,
                    utc_offset_minutes,
                )
                .unwrap_or(false)
        })
        .map(|record| record.session_id)
}

/// Compare Claude's UTC `procStart` row with the local-wall-time identity from [`ProcLiveness`].
///
/// # Errors
/// Returns an adapter error when either timestamp is malformed.
pub fn process_start_matches_utc_record(
    record_utc: &str,
    observed_local: u64,
    utc_offset_minutes: i32,
) -> Result<bool> {
    let record = parse_process_start(record_utc)?;
    let record_seconds = packed_naive_seconds(record)?;
    let observed_seconds = packed_naive_seconds(observed_local)?;
    Ok(observed_seconds - record_seconds == i64::from(utc_offset_minutes) * 60)
}

fn parse_utc_offset(value: &str) -> Option<i32> {
    if value.len() != 5 {
        return None;
    }
    let sign = match value.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hour = value[1..3].parse::<i32>().ok()?;
    let minute = value[3..5].parse::<i32>().ok()?;
    (hour <= 23 && minute <= 59).then_some(sign * (hour * 60 + minute))
}

fn packed_naive_seconds(value: u64) -> Result<i64> {
    let year = u32::try_from(value / 10_000_000_000).unwrap_or(u32::MAX);
    let month = u32::try_from(value / 100_000_000 % 100).unwrap_or(u32::MAX);
    let day = u32::try_from(value / 1_000_000 % 100).unwrap_or(u32::MAX);
    let hour = u32::try_from(value / 10_000 % 100).unwrap_or(u32::MAX);
    let minute = u32::try_from(value / 100 % 100).unwrap_or(u32::MAX);
    let second = u32::try_from(value % 100).unwrap_or(u32::MAX);
    if !(1970..=9999).contains(&year)
        || month == 0
        || month > 12
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(parse_error(&value.to_string()));
    }

    let years = i64::from(year - 1970);
    let mut days = years * 365 + leap_days_before(year) - leap_days_before(1970);
    for prior_month in 1..month {
        days += i64::from(days_in_month(year, prior_month));
    }
    days += i64::from(day - 1);
    Ok(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

const fn leap_days_before(year: u32) -> i64 {
    let prior = year - 1;
    (prior / 4 - prior / 100 + prior / 400) as i64
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

fn parse_error(row: &str) -> PijError {
    PijError::Adapter {
        adapter: ADAPTER.to_string(),
        message: format!(
            "could not parse `ps -o lstart=` row {row:?}; expected C-locale `Www Mmm DD HH:MM:SS YYYY`"
        ),
    }
}

#[cfg(test)]
mod tests {
    /// Plan 156 ruling 2: a session is derived only from the observed process's
    /// own record; a record whose start disagrees (a recycled pid) derives nothing.
    #[test]
    fn claude_session_of_requires_the_record_to_describe_the_observed_process() {
        let home = pij_testkit::fresh_dir("pij-claude-session-of");
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        std::fs::write(
            home.join("sessions/43821.json"),
            r#"{"pid":43821,"sessionId":"S-1","procStart":"Sun Sep 27 00:21:47 2026","tmux":"main:@3.%3"}"#,
        )
        .unwrap();
        let homes = [home.clone()];
        let host = |proc_start| pij_core::model::ProcIdentity {
            pid: 43_821,
            proc_start,
        };
        // Local 10:21:47 at +10:00 is the recorded 00:21:47 UTC.
        assert_eq!(
            super::claude_session_of(&homes, host(20_260_927_102_147), 600).as_deref(),
            Some("S-1")
        );
        assert_eq!(
            super::claude_session_of(&homes, host(20_260_927_102_148), 600),
            None
        );
        assert_eq!(super::claude_session_records(&homes)[0].pane(), Some("%3"));
        let _ = std::fs::remove_dir_all(home);
    }

    use std::collections::VecDeque;
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use pij_core::ports::LivenessPort;
    use pij_testkit::block_on;

    use super::{CommandOutput, CommandRunner, ProcLiveness, process_start_matches_utc_record};

    struct ScriptedRunner {
        expected_pid: u32,
        outputs: Mutex<VecDeque<io::Result<CommandOutput>>>,
    }

    impl ScriptedRunner {
        fn one(expected_pid: u32, output: CommandOutput) -> Self {
            Self {
                expected_pid,
                outputs: Mutex::new(VecDeque::from([Ok(output)])),
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&self, pid: u32) -> io::Result<CommandOutput> {
            assert_eq!(
                pid, self.expected_pid,
                "the command seam must receive only the requested pid"
            );
            self.outputs
                .lock()
                .expect("scripted command runner mutex")
                .pop_front()
                .expect("scripted command output")
        }
    }

    fn output(success: bool, stdout: &[u8], stderr: &[u8]) -> CommandOutput {
        CommandOutput {
            status: ExitStatus::from_raw(i32::from(!success) << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn exact_pid_command_and_macos_parser_are_fixture_backed() {
        let fixture = include_bytes!("../tests/fixtures/ps-macos-lstart.txt");
        let adapter =
            ProcLiveness::with_runner(ScriptedRunner::one(4242, output(true, fixture, b"")));
        assert_eq!(
            block_on(adapter.proc_start(4242)).expect("fixture should parse"),
            Some(20_260_829_091_148)
        );
    }

    #[test]
    fn linux_spacing_is_fixture_backed() {
        let fixture = include_bytes!("../tests/fixtures/ps-linux-lstart.txt");
        let adapter =
            ProcLiveness::with_runner(ScriptedRunner::one(5252, output(true, fixture, b"")));
        assert_eq!(
            block_on(adapter.proc_start(5252)).expect("fixture should parse"),
            Some(20_260_108_002_051)
        );
    }

    #[test]
    fn permission_denied_is_not_dead() {
        let fixture = include_bytes!("../tests/fixtures/ps-permission-denied.txt");
        let adapter =
            ProcLiveness::with_runner(ScriptedRunner::one(6262, output(false, b"", fixture)));
        let error = block_on(adapter.proc_start(6262)).expect_err("inspection refusal is an error");
        assert!(error.to_string().contains("Operation not permitted"));
    }

    #[test]
    fn exit_one_with_header_only_and_empty_stderr_is_absent() {
        let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
            7272,
            output(false, b"PIJ_LSTART\n", b""),
        ));
        assert_eq!(
            block_on(adapter.proc_start(7272)).expect("absence is observable"),
            None
        );
    }

    #[test]
    fn successful_probe_without_process_row_is_unknown() {
        for stdout in [b"".as_slice(), b"PIJ_LSTART\n"] {
            let adapter =
                ProcLiveness::with_runner(ScriptedRunner::one(7272, output(true, stdout, b"")));
            assert!(block_on(adapter.proc_start(7272)).is_err());
        }
    }

    #[test]
    fn failed_probe_requires_exact_header_only_output() {
        for stdout in [
            b"STARTED\n".as_slice(),
            b"PIJ_LSTART\n\n",
            b"PIJ_LSTART\nnot a start time\n",
            b"\xff",
        ] {
            let adapter =
                ProcLiveness::with_runner(ScriptedRunner::one(7272, output(false, stdout, b"")));
            assert!(block_on(adapter.proc_start(7272)).is_err());
        }
    }

    #[test]
    fn any_stderr_vetoes_both_absent_and_live_protocols() {
        let live = include_bytes!("../tests/fixtures/ps-macos-lstart.txt");
        for (success, stdout) in [(false, b"PIJ_LSTART\n".as_slice()), (true, live.as_slice())] {
            for stderr in [b"\n".as_slice(), b"Operation not permitted"] {
                let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
                    7272,
                    output(success, stdout, stderr),
                ));
                assert!(block_on(adapter.proc_start(7272)).is_err());
            }
        }
    }

    #[test]
    fn other_exits_and_signals_with_header_only_are_unknown() {
        for raw_status in [2 << 8, 127 << 8, 9] {
            let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
                7272,
                CommandOutput {
                    status: ExitStatus::from_raw(raw_status),
                    ..output(false, b"PIJ_LSTART\n", b"")
                },
            ));
            assert!(block_on(adapter.proc_start(7272)).is_err());
        }
    }

    #[test]
    fn failed_empty_probe_is_unknown_not_dead() {
        let adapter = ProcLiveness::with_runner(ScriptedRunner::one(7272, output(false, b"", b"")));
        assert!(block_on(adapter.proc_start(7272)).is_err());
    }

    #[test]
    fn failed_no_such_process_probe_is_unknown_not_dead() {
        let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
            7272,
            output(false, b"", b"No such process"),
        ));
        assert!(block_on(adapter.proc_start(7272)).is_err());
    }

    #[test]
    fn successful_empty_probe_with_error_text_is_unknown() {
        let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
            7272,
            output(true, b"", b"Operation not permitted"),
        ));
        assert!(block_on(adapter.proc_start(7272)).is_err());
    }

    #[test]
    fn signalled_empty_probe_is_unknown_not_dead() {
        let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
            7272,
            CommandOutput {
                status: ExitStatus::from_raw(9),
                ..output(false, b"", b"")
            },
        ));
        let error = block_on(adapter.proc_start(7272)).expect_err("signal is not absence proof");
        assert!(error.to_string().contains("signal"));
    }

    #[test]
    fn failed_probe_with_a_process_row_is_unknown() {
        let fixture = include_bytes!("../tests/fixtures/ps-macos-lstart.txt");
        let adapter =
            ProcLiveness::with_runner(ScriptedRunner::one(7272, output(false, fixture, b"")));
        assert!(block_on(adapter.proc_start(7272)).is_err());
    }

    #[test]
    fn utc_record_matches_the_same_local_start_and_rejects_recycled_pid_time() {
        assert!(
            process_start_matches_utc_record("Mon Aug 31 10:02:54 2026", 20_260_831_200_254, 600,)
                .unwrap()
        );
        assert!(
            !process_start_matches_utc_record("Mon Aug 31 10:02:53 2026", 20_260_831_200_254, 600,)
                .unwrap()
        );
    }

    #[test]
    fn malformed_process_output_is_an_adapter_error() {
        let adapter = ProcLiveness::with_runner(ScriptedRunner::one(
            8282,
            output(true, b"PIJ_LSTART\nnot a start time\n", b""),
        ));
        let error =
            block_on(adapter.proc_start(8282)).expect_err("malformed output must not mean dead");
        assert!(error.to_string().contains("could not parse"));
    }
}
