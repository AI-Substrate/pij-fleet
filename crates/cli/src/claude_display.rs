//! Claude's MessageDisplay hook (plan 226): hide a reply's closing pij block on
//! screen and show one line instead. Display only: the stored message, and what
//! the model and the Stop hook see, are unchanged.
//!
//! Claude runs this hook once per flush of a streamed reply, and runs flushes'
//! hooks concurrently (two may start in the same millisecond), while whether a
//! line is inside a pij fence depends on every earlier flush. So each flush
//! waits, briefly and boundedly, until the message's state file says the flush
//! before it is done, then processes its lines and writes the state for the
//! next. It touches no daemon and launches nothing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long a flush waits for the one before it; a hook that never finishes
/// must not hold the reply's display. Normally the wait is a few milliseconds.
pub const ORDER_WAIT: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(2);

/// Where a line sits relative to the reply's Markdown fences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Ordinary reply text.
    #[default]
    Text,
    /// Inside another open fence: a ```pij line there is quoted content.
    Quote,
    /// Inside an open ```pij fence.
    Fence,
    /// After a closed ```pij fence, with only whitespace since.
    Closed,
}

/// One message's progress across its flushes.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    /// The index of the next flush to process.
    pub next: u64,
    /// The fence state after the last processed flush.
    pub mode: Mode,
    /// The open fence's marker run (```` ``` ````, `~~~~`), which a bare
    /// fence of the same marker at least as long closes.
    #[serde(default)]
    pub opener: String,
    /// The block's lines held back so far, shown raw if it turns out not to run.
    pub hidden: String,
    /// The block's command lines so far.
    pub count: usize,
}

/// A Markdown fence line, as the daemon's parser reads one (plan 226): three
/// or more backticks or tildes, then an info string, which for backticks holds
/// no backtick. Returns the marker run and the info string.
fn fence(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|c| *c == marker).count();
    let (run, info) = trimmed.split_at(len);
    (len >= 3 && !(marker == '`' && info.contains('`'))).then_some((run, info))
}

/// Whether `line` closes the fence opened by `opener`.
fn closes(opener: &str, line: &str) -> bool {
    let marker = opener.chars().next().unwrap_or('`');
    fence(line).is_some_and(|(run, info)| {
        info.is_empty() && run.len() >= opener.len().max(3) && run.starts_with(marker)
    })
}

fn summary(count: usize) -> String {
    format!(
        "⚙ pij: {count} command{} handed to pij",
        if count == 1 { "" } else { "s" }
    )
}

/// One line of reply text outside any fence.
fn text_line(progress: &mut Progress, out: &mut String, line: &str) {
    match fence(line) {
        Some((run, info)) => {
            progress.opener = run.to_string();
            if run.starts_with('`') && info == "pij" {
                progress.mode = Mode::Fence;
                progress.hidden = line.to_string();
                progress.count = 0;
            } else {
                progress.mode = Mode::Quote;
                out.push_str(line);
            }
        }
        None => out.push_str(line),
    }
}

/// Process one flush's lines from `progress`. Returns what to display (always
/// the full replacement for this flush) and the progress after it; a block
/// that ends the message becomes one line, and one that cannot run (prose after
/// it, or never closed) is shown raw. Fences are tracked as the daemon tracks
/// them, so a ```pij example quoted inside another fence is never hidden.
#[must_use]
pub fn step(mut progress: Progress, delta: &str, last: bool) -> (String, Progress) {
    let mut out = String::new();
    for line in delta.split_inclusive('\n') {
        match progress.mode {
            Mode::Text => text_line(&mut progress, &mut out, line),
            Mode::Quote => {
                out.push_str(line);
                if closes(&progress.opener, line) {
                    progress.mode = Mode::Text;
                }
            }
            Mode::Fence => {
                progress.hidden.push_str(line);
                if closes(&progress.opener, line) {
                    progress.mode = Mode::Closed;
                } else if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
                    progress.count += 1;
                }
            }
            Mode::Closed if line.trim().is_empty() => progress.hidden.push_str(line),
            Mode::Closed => {
                out.push_str(&std::mem::take(&mut progress.hidden));
                progress.mode = Mode::Text;
                progress.count = 0;
                text_line(&mut progress, &mut out, line);
            }
        }
    }
    if last {
        match progress.mode {
            Mode::Closed => out.push_str(&summary(progress.count)),
            Mode::Fence => out.push_str(&progress.hidden),
            Mode::Text | Mode::Quote => {}
        }
        progress = Progress::default();
    }
    (out, progress)
}

/// One MessageDisplay hook input; Claude sends more fields.
#[derive(Debug, serde::Deserialize)]
struct Flush {
    message_id: String,
    index: u64,
    #[serde(rename = "final", default)]
    last: bool,
    #[serde(default)]
    delta: String,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn read_progress(path: &Path) -> Option<Progress> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The progress the flush before `index` left, waiting up to `ORDER_WAIT` for
/// it. On timeout the latest progress stands, or a fresh one.
fn progress_before(path: &Path, index: u64) -> Progress {
    if index == 0 {
        return Progress::default();
    }
    let deadline = Instant::now() + ORDER_WAIT;
    loop {
        let found = read_progress(path);
        if found
            .as_ref()
            .is_some_and(|progress| progress.next >= index)
            || Instant::now() >= deadline
        {
            return found.unwrap_or_default();
        }
        std::thread::sleep(POLL);
    }
}

/// The message's state holds the hidden block's text, so it lives in a
/// directory only this user can enter, in files only this user can read
/// (S213-001); a reply aborted mid-stream leaves its file there, still private.
fn write_progress(path: &Path, progress: &Progress) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    private_dir(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("progress"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temp);
    let written = write_private_new(
        &temp,
        &serde_json::to_vec(progress).expect("progress serializes"),
    )
    .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    // `mode` applies only to a directory it creates: tighten one left laxer.
    if std::fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// `create_new`, because `mode` applies only to a file it creates.
#[cfg(unix)]
fn write_private_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
fn write_private_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

/// The hook: one flush in, the hook's stdout out (`None` when this flush
/// displays unchanged). Every failure leaves the reply as Claude wrote it.
#[must_use]
pub fn run(state_dir: &Path, input: &str) -> Option<String> {
    let flush: Flush = serde_json::from_str(input.trim()).ok()?;
    if !safe_id(&flush.message_id) {
        return None;
    }
    let path: PathBuf = state_dir.join("claude-display").join(&flush.message_id);
    let before = progress_before(&path, flush.index);
    let (shown, mut after) = step(before, &flush.delta, flush.last);
    if flush.last {
        let _ = std::fs::remove_file(&path);
    } else {
        after.next = flush.index + 1;
        let _ = write_progress(&path, &after);
    }
    (shown != flush.delta).then(|| {
        json!({"hookSpecificOutput": {"hookEventName": "MessageDisplay", "displayContent": shown}})
            .to_string()
    })
}

/// The hook's stdout JSON parsed back, for tests: the displayed text.
#[must_use]
pub fn displayed(output: &str) -> Option<String> {
    let value: Value = serde_json::from_str(output).ok()?;
    value["hookSpecificOutput"]["displayContent"]
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flushes(deltas: &[(&str, bool)]) -> Vec<String> {
        let mut progress = Progress::default();
        deltas
            .iter()
            .map(|(delta, last)| {
                let (shown, next) = step(progress.clone(), delta, *last);
                progress = next;
                shown
            })
            .collect()
    }

    #[test]
    fn a_closing_block_split_across_flushes_becomes_one_counted_line() {
        assert_eq!(
            flushes(&[
                ("2+2 is 4.\n\n```pij\n", false),
                ("send pij-far-jackal \"hello one\"\n# a note\n\n", false),
                ("send pij-far-jackal \"hello two\"\n```", true),
            ]),
            ["2+2 is 4.\n\n", "", "⚙ pij: 2 commands handed to pij"]
        );
        assert_eq!(
            flushes(&[("ok\n  ```pij\nsend a b\n  ```\n\n", true)]),
            ["ok\n⚙ pij: 1 command handed to pij"],
            "indented fences, trailing whitespace"
        );
    }

    #[test]
    fn a_block_that_cannot_run_is_shown_raw() {
        assert_eq!(
            flushes(&[
                ("Hi.\n```pij\nsend a b\n```\n", false),
                ("And prose after.\n", false),
                ("end", true),
            ]),
            ["Hi.\n", "```pij\nsend a b\n```\nAnd prose after.\n", "end"]
        );
        assert_eq!(
            flushes(&[("```pij\nsend a b\n", false), ("tail", true)]),
            ["", "```pij\nsend a b\ntail"],
            "never closed"
        );
        assert_eq!(
            flushes(&[
                ("Example ```pij inline\n", false),
                ("```bash\nls\n```", true)
            ]),
            ["Example ```pij inline\n", "```bash\nls\n```"],
            "no pij fence line: untouched"
        );
    }

    #[test]
    fn a_pij_fence_quoted_inside_another_fence_is_never_hidden() {
        for quoted in [
            "```text\n```pij\nsend peer hi\n```",
            "````md\n```pij\nsend peer hi\n```\n````",
            "~~~\n```pij\nsend peer hi\n```\n~~~\n",
            "````\n```pij\nsend peer hi\n```",
        ] {
            assert_eq!(flushes(&[(quoted, true)]), [quoted], "{quoted:?}");
        }
        assert_eq!(
            flushes(&[
                ("````md\n```pij\nsend a quoted\n```\n````\n", false),
                ("```pij\nsend b real\n```", true),
            ]),
            [
                "````md\n```pij\nsend a quoted\n```\n````\n",
                "⚙ pij: 1 command handed to pij"
            ],
            "a closed quote before the real block leaves the real block hidden"
        );
        assert_eq!(
            flushes(&[("````pij\nsend a b\n```\n````", true)]),
            ["⚙ pij: 2 commands handed to pij"],
            "a longer pij fence holds a bare ``` line as a command line"
        );
        assert_eq!(
            flushes(&[("``` pij\nsend a b\n```", true)]),
            ["``` pij\nsend a b\n```"],
            "a space before the tag makes it another fence"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pij-claude-display-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn input(id: &str, index: u64, last: bool, delta: &str) -> String {
        json!({"session_id": "s", "message_id": id, "index": index, "final": last, "delta": delta})
            .to_string()
    }

    #[test]
    fn concurrent_flushes_are_processed_in_order() {
        let state = scratch("order");
        // The closing flush arrives first; it must wait for the opening one.
        let late = {
            let state = state.clone();
            std::thread::spawn(move || {
                run(&state, &input("m-1", 1, true, "send a b\nsend c d\n```"))
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            run(&state, &input("m-1", 0, false, "Done.\n\n```pij\n"))
                .as_deref()
                .and_then(displayed),
            Some("Done.\n\n".to_string())
        );
        assert_eq!(
            late.join().expect("thread").as_deref().and_then(displayed),
            Some("⚙ pij: 2 commands handed to pij".to_string())
        );
        assert!(
            !state.join("claude-display/m-1").exists(),
            "the final flush removes the message's state"
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_missing_predecessor_holds_a_flush_only_briefly() {
        let state = scratch("missing");
        let started = Instant::now();
        assert_eq!(run(&state, &input("m-2", 3, true, "plain\n")), None);
        assert!(started.elapsed() < ORDER_WAIT + Duration::from_millis(200));
        assert_eq!(run(&state, &input("../escape", 0, true, "```pij\n")), None);
        assert_eq!(run(&state, "not json"), None);
        let _ = std::fs::remove_dir_all(&state);
    }

    /// The state holds the hidden block's lines (S213-001).
    #[cfg(unix)]
    #[test]
    fn the_state_is_readable_by_this_user_alone() {
        use std::os::unix::fs::PermissionsExt;

        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        };
        let state = scratch("private");
        let dir = state.join("claude-display");
        let opening = input("m-3", 0, false, "Done.\n\n```pij\nsend a \"secret\"\n");
        let _ = run(&state, &opening);
        assert!(
            std::fs::read_to_string(dir.join("m-3"))
                .expect("state")
                .contains("secret"),
            "the state holds the hidden text"
        );
        assert_eq!(mode(&dir), 0o700, "only this user enters the directory");
        assert_eq!(mode(&dir.join("m-3")), 0o600, "only this user reads it");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let _ = run(&state, &input("m-4", 0, false, "```pij\nsend b c\n"));
        assert_eq!(mode(&dir), 0o700, "a directory left laxer is tightened");
        assert_eq!(mode(&dir.join("m-4")), 0o600);
        let _ = std::fs::remove_dir_all(&state);
    }
}
