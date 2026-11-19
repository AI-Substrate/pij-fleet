//! Each harness's folder-trust prompt (F215, D-226-6, D-232-2).
//!
//! Copilot and Claude ask, before they start, whether to trust a folder they
//! have not seen. Until someone answers, the seat never registers. A spawn
//! that waits reads the pane anyway, so it names the prompt from that read
//! and refuses at once, instead of timing out 30 s later. With
//! `spawn --trust-folder` it answers instead, one key at a time, moving the
//! highlight to the row that trusts the folder and choosing it.
//!
//! A prompt is matched only on whole rows, after the box drawing around them
//! is stripped: the question row (where the harness has one that fits in a
//! pane's bottom rows) and the menu's own rows. Prose that quotes a row sits
//! on a row with other words, so it never matches.
//!
//! **Adding a harness.** Capture the prompt's real screen (`tmux
//! capture-pane -p -t <pane>`), with its default highlight and with the
//! highlight moved, commit them under `crates/testkit/fixtures/harnesses/`
//! with MANIFEST rows, and add a row to [`PROMPTS`] and its tests.

use pij_core::model::Harness;
use pij_core::ports::MenuKey;

/// One harness's folder-trust prompt.
#[derive(Debug, PartialEq, Eq)]
struct Chrome {
    harness: Harness,
    /// Named in refusals, never text read from the pane.
    label: &'static str,
    /// A row the options must follow, when the prompt has one that stays in
    /// a short pane's view.
    question: Option<&'static str>,
    /// The menu's rows, top to bottom, without the highlight marker.
    options: &'static [&'static str],
    /// How many of `options` must be visible. Copilot's menu scrolls in a
    /// short pane and can show one row; Claude's always shows both.
    shown: usize,
    /// The row that trusts the folder and starts the harness.
    trust: usize,
    /// The safe answer for a person at the pane, with its default.
    safe_answer: &'static str,
}

/// Every folder-trust prompt pij knows. Fixtures:
/// `copilot-1.0.114-folder-trust*.txt` and `claude-2.1.328-folder-trust*.txt`.
const PROMPTS: &[Chrome] = &[
    Chrome {
        harness: Harness::Copilot,
        label: "Copilot's folder-trust prompt",
        question: Some("Do you trust the files in this folder?"),
        options: &[
            "1. Yes",
            "2. Yes, and remember this folder for future sessions",
            "3. No (Esc)",
        ],
        shown: 1,
        trust: 0,
        safe_answer: "press Enter: its default, `1. Yes`, trusts the folder for this session only",
    },
    Chrome {
        harness: Harness::Claude,
        label: "Claude's folder-trust prompt",
        question: None,
        options: &["No, exit", "Yes, I trust this folder"],
        shown: 2,
        trust: 1,
        safe_answer: "press Down, then Enter: its default is `No, exit`, so a bare Enter \
                      closes the seat; `Yes, I trust this folder` is the row below",
    },
];

/// The highlight marker both harnesses draw on the chosen row.
const HIGHLIGHT: &str = "❯";

/// A folder-trust prompt found on a screen.
#[derive(Debug, PartialEq, Eq)]
pub struct TrustPrompt {
    chrome: &'static Chrome,
    /// The highlighted row's index in `chrome.options`, when it is visible.
    highlighted: Option<usize>,
}

impl TrustPrompt {
    /// The prompt's fixed name, e.g. `Copilot's folder-trust prompt`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        self.chrome.label
    }

    /// What a person at the pane presses to trust the folder, from the
    /// prompt's default.
    #[must_use]
    pub fn safe_answer(&self) -> &'static str {
        self.chrome.safe_answer
    }

    /// The one key that moves this screen toward trusting the folder: Enter
    /// when the trust row is highlighted, else Up or Down toward it. `None`
    /// when no highlighted row is visible, so nothing can be chosen safely.
    #[must_use]
    pub fn next_key(&self) -> Option<MenuKey> {
        let highlighted = self.highlighted?;
        Some(match highlighted.cmp(&self.chrome.trust) {
            std::cmp::Ordering::Equal => MenuKey::Enter,
            std::cmp::Ordering::Less => MenuKey::Down,
            std::cmp::Ordering::Greater => MenuKey::Up,
        })
    }
}

/// The harness's folder-trust prompt on `screen` (a whole pane capture), if
/// one is showing.
#[must_use]
pub fn folder_trust_prompt(harness: Harness, screen: &str) -> Option<TrustPrompt> {
    let chrome = PROMPTS.iter().find(|chrome| chrome.harness == harness)?;
    let rows: Vec<&str> = screen.lines().map(unboxed).collect();
    let start = match chrome.question {
        Some(question) => rows.iter().position(|row| *row == question)? + 1,
        None => 0,
    };
    let mut shown = 0;
    let mut highlighted = None;
    for row in &rows[start..] {
        let (marked, text) = match row.strip_prefix(HIGHLIGHT) {
            Some(rest) => (true, rest.trim_start()),
            None => (false, *row),
        };
        let Some(index) = chrome.options.iter().position(|option| *option == text) else {
            continue;
        };
        shown += 1;
        if marked {
            highlighted = Some(index);
        }
    }
    (shown >= chrome.shown).then_some(TrustPrompt {
        chrome,
        highlighted,
    })
}

/// A row without the box drawing around it: Copilot draws its prompt inside
/// `│ … │`.
fn unboxed(row: &str) -> &str {
    let row = row.trim();
    let row = row.strip_prefix('│').unwrap_or(row);
    let row = row.strip_suffix('│').unwrap_or(row);
    row.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../testkit/fixtures/harnesses")
                .join(name),
        )
        .expect("read a harness fixture")
    }

    fn next_key(harness: Harness, name: &str) -> Option<MenuKey> {
        folder_trust_prompt(harness, &fixture(name))
            .unwrap_or_else(|| panic!("{name} shows a trust prompt"))
            .next_key()
    }

    /// The captured Copilot prompt matches at its default and in a short
    /// pane that shows one menu row, and its default (`1. Yes`) is chosen
    /// with Enter.
    #[test]
    fn copilot_trust_prompt_matches_and_its_default_is_enter() {
        for name in [
            "copilot-1.0.114-folder-trust.txt",
            "copilot-1.0.114-folder-trust-short.txt",
        ] {
            assert_eq!(
                next_key(Harness::Copilot, name),
                Some(MenuKey::Enter),
                "{name}"
            );
        }
        let prompt = folder_trust_prompt(
            Harness::Copilot,
            &fixture("copilot-1.0.114-folder-trust.txt"),
        )
        .expect("prompt");
        assert_eq!(prompt.label(), "Copilot's folder-trust prompt");
        assert!(prompt.safe_answer().starts_with("press Enter"));
    }

    /// The daemon reads the whole screen: Copilot's bottom 20 rows lose the
    /// title but keep the question, and still match.
    #[test]
    fn copilot_trust_prompt_matches_in_the_bottom_rows() {
        let screen = fixture("copilot-1.0.114-folder-trust.txt");
        let rows: Vec<&str> = screen.lines().collect();
        let tail = rows[rows.len() - 20..].join("\n");
        assert!(!tail.contains("Confirm folder trust"));
        assert!(folder_trust_prompt(Harness::Copilot, &tail).is_some());
    }

    /// Never Enter on a row that is not the trust row: with `2. Yes, and
    /// remember …` highlighted the next key is Up.
    #[test]
    fn copilot_second_row_highlight_moves_up() {
        assert_eq!(
            next_key(
                Harness::Copilot,
                "copilot-1.0.114-folder-trust-second-row.txt"
            ),
            Some(MenuKey::Up)
        );
    }

    /// D-226-6: Claude's default is `No, exit`, so its next key is Down, and
    /// Enter only once `Yes, I trust this folder` is highlighted.
    #[test]
    fn claude_trust_prompt_moves_down_before_enter() {
        assert_eq!(
            next_key(Harness::Claude, "claude-2.1.328-folder-trust.txt"),
            Some(MenuKey::Down)
        );
        assert_eq!(
            next_key(Harness::Claude, "claude-2.1.328-folder-trust-yes-row.txt"),
            Some(MenuKey::Enter)
        );
        let prompt =
            folder_trust_prompt(Harness::Claude, &fixture("claude-2.1.328-folder-trust.txt"))
                .expect("prompt");
        assert!(prompt.safe_answer().starts_with("press Down, then Enter"));
    }

    /// No prompt, no match: each harness's normal screens, the other
    /// harness's prompt, harnesses with no row, and prose quoting the rows.
    #[test]
    fn other_screens_never_match() {
        for (harness, name) in [
            (Harness::Copilot, "copilot-working.txt"),
            (Harness::Copilot, "copilot-follow-up-dialog-herring.txt"),
            (Harness::Copilot, "claude-2.1.328-folder-trust.txt"),
            (Harness::Claude, "claude-2.1.284-permission-dialog.txt"),
            (Harness::Claude, "copilot-1.0.114-folder-trust.txt"),
            (Harness::Omp, "claude-2.1.328-folder-trust.txt"),
            (Harness::Codex, "copilot-1.0.114-folder-trust.txt"),
        ] {
            assert_eq!(
                folder_trust_prompt(harness, &fixture(name)),
                None,
                "{harness:?} {name}"
            );
        }
        let quoted = "● Copilot asks: Do you trust the files in this folder? Then ❯ 1. Yes\n\
                      ● Claude shows No, exit and Yes, I trust this folder";
        assert_eq!(folder_trust_prompt(Harness::Copilot, quoted), None);
        assert_eq!(folder_trust_prompt(Harness::Claude, quoted), None);
    }

    /// A prompt whose highlighted row has scrolled out of view names no key:
    /// nothing is chosen blind.
    #[test]
    fn no_visible_highlight_names_no_key() {
        let screen = "│ Do you trust the files in this folder? │\n│   3. No (Esc) │";
        let prompt = folder_trust_prompt(Harness::Copilot, screen).expect("prompt");
        assert_eq!(prompt.next_key(), None);
    }
}
