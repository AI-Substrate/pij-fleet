//! Claude Code's usage-limit record, read from its transcript (plan 172 R3),
//! and the local-zone rendering of a reset instant.
//!
//! Only Claude Code writes a structured quota rejection (`quotaLimits`, see
//! [`pij_core::usage_limit`]). The other harnesses answer `None`: pij knows of
//! no equivalent structured field there and never guesses one.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::SystemTime;

use pij_core::error::{PijError, Result};
use pij_core::model::Harness;
use pij_core::usage_limit::{QuotaRejection, UsageLimitSource, latest_active};

const ADAPTER: &str = "harnesses/usage-limit";

/// How much of a transcript's end is read. A stopped seat's rejection is its
/// last assistant entry, so the tail holds it; a tail with no assistant entry
/// at all answers `None`.
pub const TAIL_BYTES: u64 = 64 * 1024;

/// One transcript's last read: unchanged length and mtime reuse the answer.
#[derive(Clone)]
struct Seen {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    answer: Option<QuotaRejection>,
}

/// Reads Claude Code transcripts (`<home>/projects/<project>/<session>.jsonl`)
/// under every Claude home.
pub struct ClaudeUsageLimits {
    homes: Vec<PathBuf>,
    seen: Mutex<HashMap<String, Seen>>,
}

impl ClaudeUsageLimits {
    /// A source over these Claude homes (`claude_homes()`).
    pub fn new(homes: Vec<PathBuf>) -> Self {
        Self {
            homes,
            seen: Mutex::new(HashMap::new()),
        }
    }

    fn locate(&self, session: &str) -> Option<PathBuf> {
        let file = format!("{session}.jsonl");
        self.homes.iter().find_map(|home| {
            std::fs::read_dir(home.join("projects"))
                .ok()?
                .filter_map(std::result::Result::ok)
                .map(|project| project.path().join(&file))
                .find(|candidate| candidate.is_file())
        })
    }
}

fn io_error(path: &Path, error: &std::io::Error) -> PijError {
    PijError::Adapter {
        adapter: ADAPTER.into(),
        message: format!("{}: {error}", path.display()),
    }
}

/// The last [`TAIL_BYTES`] of a file as lines, dropping a cut first line.
fn tail(path: &Path, len: u64) -> Result<String> {
    let mut file = File::open(path).map_err(|error| io_error(path, &error))?;
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| io_error(path, &error))?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(path, &error))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(if start > 0 {
        text.split_once('\n')
            .map_or_else(String::new, |(_, rest)| rest.to_string())
    } else {
        text
    })
}

impl UsageLimitSource for ClaudeUsageLimits {
    fn latest_rejection(&self, harness: Harness, session: &str) -> Result<Option<QuotaRejection>> {
        // A session id names a file; anything that could leave the project
        // directory is not one.
        if harness != Harness::Claude
            || session.is_empty()
            || session.contains(['/', '\\'])
            || session.starts_with('.')
        {
            return Ok(None);
        }
        let known = self
            .seen
            .lock()
            .expect("usage-limit cache")
            .get(session)
            .cloned();
        let path = match known.as_ref().filter(|seen| seen.path.is_file()) {
            Some(seen) => seen.path.clone(),
            None => match self.locate(session) {
                Some(path) => path,
                None => return Ok(None),
            },
        };
        let meta = std::fs::metadata(&path).map_err(|error| io_error(&path, &error))?;
        let modified = meta.modified().ok();
        if let Some(seen) = known
            .filter(|seen| seen.path == path && seen.len == meta.len() && seen.modified == modified)
        {
            return Ok(seen.answer);
        }
        let answer = latest_active(tail(&path, meta.len())?.lines());
        self.seen.lock().expect("usage-limit cache").insert(
            session.to_string(),
            Seen {
                path,
                len: meta.len(),
                modified,
                answer: answer.clone(),
            },
        );
        Ok(answer)
    }
}

/// `epoch_secs` as local wall-clock text with the zone's name, e.g.
/// `Sat 03 Oct 2026 20:00 AEST`, rendered by the system's tz database AT
/// THAT INSTANT, so a reset across a daylight-saving change shows the
/// zone in force then. `tz` overrides the zone (`TZ`), for tests; `None` is
/// the daemon's own zone. `None` back when `date` cannot render it.
pub fn local_clock_text(epoch_secs: u64, tz: Option<&str>) -> Option<String> {
    const FORMAT: &str = "+%a %d %b %Y %H:%M %Z";
    let at = epoch_secs.to_string();
    let gnu = format!("@{at}");
    // BSD/macOS `date -r <secs>`, else GNU `date -d @<secs>`.
    [["-r", at.as_str()], ["-d", gnu.as_str()]]
        .into_iter()
        .find_map(|args| {
            pij_core::spawns::record("date.usage-limit-clock");
            let mut command = Command::new("/bin/date");
            command.args(args).arg(FORMAT).env("LC_ALL", "C");
            if let Some(tz) = tz {
                command.env("TZ", tz);
            }
            let output = command.output().ok()?;
            let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
            (output.status.success() && !text.is_empty()).then_some(text)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn rejected(resets_at: u64, kind: &str) -> String {
        format!(
            r#"{{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","apiErrorStatus":429,"quotaLimits":{{"status":"rejected","rateLimitType":"{kind}","resetsAt":{resets_at},"unifiedRateLimitFallbackAvailable":true,"overageStatus":"rejected","isUsingOverage":false}},"message":{{"model":"<synthetic>","content":[{{"type":"text","text":"You've hit your limit · resets 3:10pm (Australia/Brisbane)"}}]}}}}"#
        )
    }

    struct Home(PathBuf);

    impl Home {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "pij-usage-limit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("projects/-repo")).unwrap();
            Self(root)
        }

        fn write(&self, session: &str, lines: &[String]) {
            std::fs::write(
                self.0.join(format!("projects/-repo/{session}.jsonl")),
                lines.join("\n") + "\n",
            )
            .unwrap();
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_stopped_claude_session_reports_its_structured_reset_and_others_do_not() {
        let home = Home::new();
        let call = r#"{"type":"assistant","message":{"model":"claude-opus-5-5"}}"#.to_string();
        // Old history: a rejection the seat got past is buried under real calls.
        let padding = format!(r#"{{"type":"user","pad":"{}"}}"#, "x".repeat(4_096));
        let mut lines = vec![rejected(1, "five_hour"), call.clone()];
        lines.extend(std::iter::repeat_n(padding, 20));
        lines.push(rejected(1_790_399_400, "five_hour"));
        home.write("s-stopped", &lines);
        home.write("s-resumed", &[rejected(1_790_399_400, "five_hour"), call]);
        let source = ClaudeUsageLimits::new(vec![home.0.clone()]);

        assert_eq!(
            source
                .latest_rejection(Harness::Claude, "s-stopped")
                .unwrap(),
            Some(QuotaRejection {
                limit_type: "five_hour".into(),
                resets_at_secs: 1_790_399_400,
            })
        );
        assert_eq!(
            source
                .latest_rejection(Harness::Claude, "s-resumed")
                .unwrap(),
            None
        );
        assert_eq!(
            source.latest_rejection(Harness::Omp, "s-stopped").unwrap(),
            None,
            "only Claude Code records a structured quota rejection"
        );
        for absent in ["s-missing", "../-repo/s-stopped", ""] {
            assert_eq!(
                source.latest_rejection(Harness::Claude, absent).unwrap(),
                None
            );
        }

        // The seat resumes: an appended real call ends the stop.
        let mut resumed = lines;
        resumed.push(r#"{"type":"assistant","message":{"model":"claude-opus-5-5"}}"#.into());
        home.write("s-stopped", &resumed);
        assert_eq!(
            source
                .latest_rejection(Harness::Claude, "s-stopped")
                .unwrap(),
            None
        );
    }

    /// Sydney leaves AEST for AEDT at 02:00 on Sun 4 Oct 2026; Brisbane has
    /// no daylight saving. The reset instants are epochs, so the same epoch
    /// renders in the zone in force at that instant.
    #[test]
    fn a_reset_renders_in_the_zone_in_force_at_that_instant() {
        // Claude's verified five_hour reset: 15:10 AEST on Sat 26 Sep 2026.
        assert_eq!(
            local_clock_text(1_790_399_400, Some("Australia/Brisbane")).as_deref(),
            Some("Sat 26 Sep 2026 15:10 AEST")
        );
        // A seven_day reset several days out, after Sydney's DST change.
        let seven_day = 1_791_302_400; // 2026-10-06T16:00:00Z
        assert_eq!(
            local_clock_text(seven_day, Some("Australia/Sydney")).as_deref(),
            Some("Wed 07 Oct 2026 03:00 AEDT")
        );
        assert_eq!(
            local_clock_text(seven_day, Some("Australia/Brisbane")).as_deref(),
            Some("Wed 07 Oct 2026 02:00 AEST")
        );
        // The real record's seven_day reset, the evening before the change.
        assert_eq!(
            local_clock_text(1_791_021_600, Some("Australia/Sydney")).as_deref(),
            Some("Sat 03 Oct 2026 20:00 AEST")
        );
    }
}
