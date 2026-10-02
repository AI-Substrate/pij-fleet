//! Timestamp daemon output once, below Rust's stdout/stderr and panic hooks.
//!
//! The original destinations remain distinct (or share daemon.log when launched
//! by bounce). Only the daemon installs this guard; normal CLI output is untouched.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use nix::unistd::{dup2_stderr, dup2_stdout};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) struct DaemonLog {
    stdout: LogStream,
    stderr: LogStream,
}

impl DaemonLog {
    pub(crate) fn install() -> io::Result<Self> {
        io::stdout().flush()?;
        io::stderr().flush()?;
        // Cloned descriptors are close-on-exec and never point back into the
        // logger, so its own writes cannot recursively feed its input.
        let stdout = File::from(io::stdout().as_fd().try_clone_to_owned()?);
        let stderr = File::from(io::stderr().as_fd().try_clone_to_owned()?);
        let serial = Arc::new(Mutex::new(()));
        let log = Self {
            stdout: LogStream::new(stdout, Arc::clone(&serial))?,
            stderr: LogStream::new(stderr, serial)?,
        };
        dup2_stdout(&log.stdout.writer)?;
        dup2_stderr(&log.stderr.writer)?;
        Ok(log)
    }
}

impl Drop for DaemonLog {
    fn drop(&mut self) {
        // Flush Rust's partial stdout buffer before restoring descriptors. The
        // guard encloses the runtime, so all worker teardown output is included.
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
        let _ = dup2_stdout(&self.stdout.original);
        let _ = dup2_stderr(&self.stderr.original);
        // Each LogStream then drains and joins its reader on field destruction.
    }
}

struct LogStream {
    original: File,
    writer: UnixStream,
    reader: Option<JoinHandle<io::Result<()>>>,
    serial: Arc<Mutex<()>>,
}

impl LogStream {
    fn new(original: File, serial: Arc<Mutex<()>>) -> io::Result<Self> {
        let (input, writer) = UnixStream::pair()?;
        let output = original.try_clone()?;
        let reader_serial = Arc::clone(&serial);
        let reader = thread::Builder::new()
            .name("daemon-log".into())
            .spawn(move || copy_timestamped(input, output, &reader_serial))?;
        Ok(Self {
            original,
            writer,
            reader: Some(reader),
            serial,
        })
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        // shutdown, rather than merely close, produces EOF even when an exec'd
        // child inherited stdout/stderr. Already-written bytes are still drained;
        // a surviving child cannot keep daemon shutdown waiting for pipe EOF.
        let _ = self.writer.shutdown(Shutdown::Write);
        if let Some(reader) = self.reader.take() {
            let result = reader
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("daemon log reader panicked")));
            if let Err(error) = result {
                let message = format!("pij-rs: daemon log sink failed: {error}\n");
                let _ = copy_timestamped(message.as_bytes(), &mut self.original, &self.serial);
            }
        }
    }
}

fn copy_timestamped(
    input: impl Read,
    mut output: impl Write,
    serial: &Mutex<()>,
) -> io::Result<()> {
    let mut input = BufReader::new(input);
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return output.flush();
        }
        // stdout and stderr may target the same file. Emit whole lines under one
        // lock so a split write on one stream cannot corrupt the other's prefix.
        let _guard = serial.lock().unwrap_or_else(|poison| poison.into_inner());
        OffsetDateTime::now_utc()
            .format_into(&mut output, &Rfc3339)
            .map_err(io::Error::other)?;
        output.write_all(b" ")?;
        output.write_all(&line)?;
        if !line.ends_with(b"\n") {
            // Preserve the final fragment and finish its physical line before a
            // subsequent daemon boot appends to the same log.
            output.write_all(b"\n")?;
        }
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn payloads(log: &[u8]) -> Vec<Vec<u8>> {
        log.split_inclusive(|byte| *byte == b'\n')
            .map(|line| {
                let split = line.iter().position(|byte| *byte == b' ').unwrap();
                let timestamp = std::str::from_utf8(&line[..split]).unwrap();
                assert!(OffsetDateTime::parse(timestamp, &Rfc3339).is_ok());
                line[split + 1..].to_vec()
            })
            .collect()
    }

    #[test]
    fn timestamps_blank_long_non_utf8_and_unterminated_lines_without_changing_payloads() {
        let long = vec![b'x'; 32 * 1024];
        let mut input = b"first\n\n".to_vec();
        input.extend_from_slice(&long);
        input.extend_from_slice(b"\n\xffpartial");
        let mut output = Vec::new();
        copy_timestamped(input.as_slice(), &mut output, &Mutex::new(())).unwrap();
        let mut expected_long = long;
        expected_long.push(b'\n');
        assert_eq!(
            payloads(&output),
            [
                b"first\n".to_vec(),
                b"\n".to_vec(),
                expected_long,
                b"\xffpartial\n".to_vec(),
            ]
        );
    }

    #[test]
    fn shutdown_drains_split_writes_without_waiting_for_inherited_writer() {
        let root = pij_testkit::fresh_dir("pij-daemon-log-drain");
        let path = root.join("daemon.log");
        let stream =
            LogStream::new(File::create(&path).unwrap(), Arc::new(Mutex::new(()))).unwrap();
        let mut inherited = stream.writer.try_clone().unwrap();
        inherited.write_all(b"complete\nsplit").unwrap();
        inherited.write_all(b" fragment").unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let drain = thread::spawn(move || {
            drop(stream);
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("inherited writer must not hold daemon log shutdown open");
        drain.join().unwrap();
        assert_eq!(
            payloads(&fs::read(&path).unwrap()),
            [b"complete\n".to_vec(), b"split fragment\n".to_vec()]
        );
        drop(inherited);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn captures_raw_panic_output_and_flushes_during_unwind() {
        const CHILD_LOG: &str = "PIJ_TEST_PANIC_DAEMON_LOG";
        if let Some(path) = std::env::var_os(CHILD_LOG) {
            // Re-exec isolates process-wide descriptor changes from parallel
            // tests. Redirect after the harness prelude and exit before its
            // summary, so this file contains only output crossing our boundary.
            let file = File::create(path).unwrap();
            dup2_stdout(&file).unwrap();
            dup2_stderr(&file).unwrap();
            let panicked = std::panic::catch_unwind(|| {
                let _log = DaemonLog::install().unwrap();
                io::stdout()
                    .write_all(b"raw stdout\nfinal fragment")
                    .unwrap();
                io::stderr().write_all(b"raw stderr\n").unwrap();
                panic!("first panic line\nsecond panic line");
            });
            std::process::exit(if panicked.is_err() { 0 } else { 1 });
        }
        let root = pij_testkit::fresh_dir("pij-daemon-log-panic");
        let path = root.join("daemon.log");
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon_log::tests::captures_raw_panic_output_and_flushes_during_unwind",
                "--nocapture",
            ])
            .env(CHILD_LOG, &path)
            .env("RUST_BACKTRACE", "0")
            .stdout(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let lines = payloads(&fs::read(&path).unwrap());
        for expected in [
            b"raw stdout\n".as_slice(),
            b"raw stderr\n".as_slice(),
            b"final fragment\n".as_slice(),
            b"first panic line\n".as_slice(),
            b"second panic line\n".as_slice(),
        ] {
            assert!(lines.iter().any(|line| line == expected), "{lines:?}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}
