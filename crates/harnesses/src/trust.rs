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
//! is stripped: a heading row (Copilot's question, Claude's `Accessing
//! workspace:`) and the menu's own rows below it. A pane shorter than
//! Claude's prompt scrolls the heading off the top (a live 60x12 capture), so
//! for Claude the footer row below the menu anchors it instead. Prose that
//! quotes a row sits on a row with other words, so it never matches. The
//! refusal's guidance, like `--trust-folder`'s key, comes from the row
//! highlighted on the screen, never from the prompt's default.
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
    /// A row the options must follow: Copilot's question, Claude's heading.
    heading: &'static str,
    /// A row the options must precede, which anchors them when a short pane
    /// has scrolled the heading away. Copilot keeps its question in view.
    footer: Option<&'static str>,
    /// The menu's rows, top to bottom, without the highlight marker.
    options: &'static [&'static str],
    /// How many of `options` must be visible. Copilot's menu scrolls in a
    /// short pane and can show one row; Claude's always shows both.
    shown: usize,
    /// The row that trusts the folder and starts the harness.
    trust: usize,
    /// What choosing the trust row does, as the prompt itself says.
    trusts: &'static str,
}

/// Every folder-trust prompt pij knows. Fixtures:
/// `copilot-1.0.114-folder-trust*.txt` and `claude-2.1.328-folder-trust*.txt`.
const PROMPTS: &[Chrome] = &[
    Chrome {
        harness: Harness::Copilot,
        label: "Copilot's folder-trust prompt",
        heading: "Do you trust the files in this folder?",
        footer: None,
        options: &[
            "1. Yes",
            "2. Yes, and remember this folder for future sessions",
            "3. No (Esc)",
        ],
        shown: 1,
        trust: 0,
        trusts: "trusts the folder for this session only",
    },
    Chrome {
        harness: Harness::Claude,
        label: "Claude's folder-trust prompt",
        heading: "Accessing workspace:",
        footer: Some("Enter to confirm · Esc to cancel"),
        options: &["No, exit", "Yes, I trust this folder"],
        shown: 2,
        trust: 1,
        trusts: "trusts the folder",
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

    /// What a person at the pane presses to trust the folder, from the row
    /// highlighted on this screen (R264-001): Enter only when the trust row is
    /// highlighted, else the moves to it and then Enter. With no highlighted
    /// row in view it names the row and no key.
    #[must_use]
    pub fn safe_answer(&self) -> String {
        let chrome = self.chrome;
        let choose = format!(
            "`{}`, which {}",
            chrome.options[chrome.trust], chrome.trusts
        );
        let Some(highlighted) = self.highlighted else {
            return format!(
                "move the highlight to {choose}, then press Enter (no highlighted row is \
                 visible, so pij names no key)"
            );
        };
        let steps = highlighted.abs_diff(chrome.trust);
        if steps == 0 {
            return format!("press Enter to choose the highlighted {choose}");
        }
        let key = if highlighted < chrome.trust {
            "Down"
        } else {
            "Up"
        };
        let times = if steps == 1 {
            "once".to_string()
        } else {
            format!("{steps} times")
        };
        format!(
            "press {key} {times}, then Enter, to choose {choose}; Enter now would choose the \
             highlighted `{}`",
            chrome.options[highlighted]
        )
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
    let menu = match rows.iter().position(|row| *row == chrome.heading) {
        Some(heading) => &rows[heading + 1..],
        None => {
            let footer = chrome.footer?;
            &rows[..rows.iter().rposition(|row| *row == footer)?]
        }
    };
    let mut shown = 0;
    let mut highlighted = None;
    for row in menu {
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

    /// The next key and the refusal's guidance for a captured screen.
    fn answer(harness: Harness, name: &str) -> (Option<MenuKey>, String) {
        let prompt = folder_trust_prompt(harness, &fixture(name))
            .unwrap_or_else(|| panic!("{name} shows a trust prompt"));
        (prompt.next_key(), prompt.safe_answer())
    }

    const COPILOT_DEFAULT: &str = "press Enter to choose the highlighted `1. Yes`, which \
                                   trusts the folder for this session only";

    /// The captured Copilot prompt matches at its default and in a short
    /// pane that shows one menu row: `1. Yes` is highlighted, so Enter.
    #[test]
    fn copilot_trust_prompt_matches_and_its_default_is_enter() {
        for name in [
            "copilot-1.0.114-folder-trust.txt",
            "copilot-1.0.114-folder-trust-short.txt",
        ] {
            assert_eq!(
                answer(Harness::Copilot, name),
                (Some(MenuKey::Enter), COPILOT_DEFAULT.to_string()),
                "{name}"
            );
        }
        let prompt = folder_trust_prompt(
            Harness::Copilot,
            &fixture("copilot-1.0.114-folder-trust.txt"),
        )
        .expect("prompt");
        assert_eq!(prompt.label(), "Copilot's folder-trust prompt");
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

    /// R264-001: with `2. Yes, and remember …` highlighted, Enter would
    /// remember the folder for good. The key is Up and the guidance says Up
    /// first, naming what Enter would choose now.
    #[test]
    fn copilot_second_row_highlight_moves_up() {
        assert_eq!(
            answer(
                Harness::Copilot,
                "copilot-1.0.114-folder-trust-second-row.txt"
            ),
            (
                Some(MenuKey::Up),
                "press Up once, then Enter, to choose `1. Yes`, which trusts the folder for \
                 this session only; Enter now would choose the highlighted `2. Yes, and \
                 remember this folder for future sessions`"
                    .to_string()
            )
        );
    }

    /// D-226-6 and R264-001: Claude's default is `No, exit`, so the key and
    /// the guidance are Down first; with `Yes, I trust this folder` already
    /// highlighted, both are Enter alone.
    #[test]
    fn claude_trust_prompt_moves_down_before_enter() {
        assert_eq!(
            answer(Harness::Claude, "claude-2.1.328-folder-trust.txt"),
            (
                Some(MenuKey::Down),
                "press Down once, then Enter, to choose `Yes, I trust this folder`, which \
                 trusts the folder; Enter now would choose the highlighted `No, exit`"
                    .to_string()
            )
        );
        assert_eq!(
            answer(Harness::Claude, "claude-2.1.328-folder-trust-yes-row.txt"),
            (
                Some(MenuKey::Enter),
                "press Enter to choose the highlighted `Yes, I trust this folder`, which \
                 trusts the folder"
                    .to_string()
            )
        );
    }

    /// R264's C34 follow-up, on live captures: a 60x12 pane scrolls Claude's
    /// heading off the top, and the footer row below the menu anchors it; a
    /// 34-column pane wraps the prose and keeps every row the parser reads.
    /// Both still move Down from `No, exit`.
    #[test]
    fn claude_trust_prompt_matches_in_a_short_or_narrow_pane() {
        let short = fixture("claude-2.1.329-folder-trust-short.txt");
        assert!(
            !short.contains("Accessing workspace:"),
            "the heading is gone"
        );
        for name in [
            "claude-2.1.329-folder-trust-short.txt",
            "claude-2.1.329-folder-trust-narrow.txt",
        ] {
            assert_eq!(
                answer(Harness::Claude, name).0,
                Some(MenuKey::Down),
                "{name}"
            );
        }
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
        // Claude's menu rows alone, with neither its heading above them nor
        // its footer below.
        let menu = "❯ No, exit\n  Yes, I trust this folder";
        assert_eq!(folder_trust_prompt(Harness::Claude, menu), None);
        // The footer above the menu anchors nothing.
        let footer_first =
            "Enter to confirm · Esc to cancel\n❯ No, exit\n  Yes, I trust this folder";
        assert_eq!(folder_trust_prompt(Harness::Claude, footer_first), None);
    }

    /// A prompt whose highlighted row has scrolled out of view names no key,
    /// and its guidance names the trust row but no key: nothing is chosen
    /// blind.
    #[test]
    fn no_visible_highlight_names_no_key() {
        let screen = "│ Do you trust the files in this folder? │\n│   3. No (Esc) │";
        let prompt = folder_trust_prompt(Harness::Copilot, screen).expect("prompt");
        assert_eq!(prompt.next_key(), None);
        assert_eq!(
            prompt.safe_answer(),
            "move the highlight to `1. Yes`, which trusts the folder for this session only, \
             then press Enter (no highlighted row is visible, so pij names no key)"
        );
    }
}
