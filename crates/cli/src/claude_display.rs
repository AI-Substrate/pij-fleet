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

/// Where a line sits relative to a pij fence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Ordinary reply text.
    #[default]
    Text,
    /// Inside an open ```pij fence.
    Fence,
    /// After a closed fence, with only whitespace since.
    Closed,
}

/// One message's progress across its flushes.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    /// The index of the next flush to process.
    pub next: u64,
    /// The fence state after the last processed flush.
    pub mode: Mode,
    /// The block's lines held back so far, shown raw if it turns out not to run.
    pub hidden: String,
    /// The block's command lines so far.
    pub count: usize,
}

fn opening(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("```pij")
        .is_some_and(|rest| rest.trim().is_empty())
}

fn closing(line: &str) -> bool {
    line.trim() == "```"
}

fn summary(count: usize) -> String {
    format!(
        "⚙ pij: {count} command{} handed to pij",
        if count == 1 { "" } else { "s" }
    )
}

/// Process one flush's lines from `progress`. Returns what to display (always
/// the full replacement for this flush) and the progress after it; a block
/// that ends the message becomes one line, and one that cannot run (prose after
/// it, or never closed) is shown raw.
#[must_use]
pub fn step(mut progress: Progress, delta: &str, last: bool) -> (String, Progress) {
    let mut out = String::new();
    for line in delta.split_inclusive('\n') {
        match progress.mode {
            Mode::Text if opening(line) => {
                progress.mode = Mode::Fence;
                progress.hidden = line.to_string();
                progress.count = 0;
            }
            Mode::Text => out.push_str(line),
            Mode::Fence => {
                progress.hidden.push_str(line);
                if closing(line) {
                    progress.mode = Mode::Closed;
                } else if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
                    progress.count += 1;
                }
            }
            Mode::Closed if line.trim().is_empty() => progress.hidden.push_str(line),
            Mode::Closed => {
                out.push_str(&std::mem::take(&mut progress.hidden));
                if opening(line) {
                    progress.mode = Mode::Fence;
                    progress.hidden = line.to_string();
                    progress.count = 0;
                } else {
                    out.push_str(line);
                    progress.mode = Mode::Text;
                    progress.count = 0;
                }
            }
        }
    }
    if last {
        match progress.mode {
            Mode::Closed => out.push_str(&summary(progress.count)),
            Mode::Fence => out.push_str(&progress.hidden),
            Mode::Text => {}
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

fn write_progress(path: &Path, progress: &Progress) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("progress"),
        std::process::id()
    ));
    std::fs::write(
        &temp,
        serde_json::to_vec(progress).expect("progress serializes"),
    )?;
    std::fs::rename(&temp, path)
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
}
