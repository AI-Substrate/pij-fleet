//! Byte-goldens: the parity tripwire's mechanism.
//!
//! A golden test says "these exact bytes, or explain yourself". It is the right
//! instrument for a PORT, because the question is not "does this look sane" but
//! "is it what the thing we are replacing produced" — a question no hand-written
//! assertion can hold as the surface grows.
//!
//! Three rules, each answering a way goldens rot:
//!
//! * **Computed payloads only.** Editorial text (help output, prose) gets shape
//!   assertions instead; freezing prose means every wording fix is a test failure
//!   and the suite trains people to re-record without reading.
//! * **Re-record deliberately** — `UPDATE_GOLDENS=1` rewrites, and the failure
//!   message says so. A golden nobody can update becomes a golden people delete.
//! * **The diff is the message.** A failure prints the first differing line with
//!   its number, not two walls of text for a human to compare by eye.
//!
//! Wave-0 scope (prime ruling Q4): the goldens run against COMMITTED bytes from
//! the corpus, never against a live `pij` invocation — the live catalog is
//! time-varying (599 rows to 125 in ten minutes, 2026-08-28), so a live golden
//! would flap for reasons that have nothing to do with this port. Real
//! binary-to-binary parity lands with `u-cli` in wave 3, on this mechanism.

use std::path::PathBuf;

/// Where goldens live.
pub fn dir() -> PathBuf {
    crate::fixtures::dir().join("golden")
}

/// Compare `actual` against the committed golden `name`.
///
/// # Panics
/// On any difference, naming the first differing line and how to re-record.
pub fn assert_golden(name: &str, actual: &str) {
    let path = dir().join(name);

    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        // Create the parent directory. A nested golden (`golden/cli/...`) is a
        // normal thing to want, and failing to record one because a directory is
        // missing teaches people to `mkdir -p` by hand — which is how a suite
        // acquires files its own update mechanism cannot regenerate.
        // Harness gift DL-001, u-cli, wave 3.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::create_dir_all(dir()).expect("create the golden directory");
        std::fs::write(&path, actual).expect("write the golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "golden {} is unreadable ({error}) — create it with UPDATE_GOLDENS=1 and REVIEW the \
             diff before committing it",
            path.display()
        )
    });

    if expected == actual {
        return;
    }

    panic!(
        "{}",
        render_diff(&path.display().to_string(), &expected, actual)
    );
}

/// The failure message: the first difference, in context, plus the fix.
///
/// Separate from [`assert_golden`] so the comparator itself is testable without
/// a panic — a diff renderer nobody can test is a diff renderer that lies on the
/// day it matters.
pub fn render_diff(path: &str, expected: &str, actual: &str) -> String {
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();

    let first = expected_lines
        .iter()
        .zip(actual_lines.iter())
        .position(|(left, right)| left != right)
        .unwrap_or(expected_lines.len().min(actual_lines.len()));

    let mut message = format!(
        "golden mismatch: {path}\n  expected {} line(s), got {}\n  first difference at line {}:\n",
        expected_lines.len(),
        actual_lines.len(),
        first + 1
    );
    message.push_str(&format!(
        "    - {}\n    + {}\n",
        expected_lines.get(first).unwrap_or(&"<missing>"),
        actual_lines.get(first).unwrap_or(&"<missing>")
    ));
    message.push_str(
        "\n  If the NEW output is correct, re-record with UPDATE_GOLDENS=1 and review the diff \
         in the commit. If it is not, you have found the regression this golden exists for.",
    );
    message
}
