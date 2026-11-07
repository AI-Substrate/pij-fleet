//! Each harness's own modal-dialog chrome (plan 219, F47).
//!
//! A seat that shows its harness's question or permission dialog has stopped,
//! though pij still reads it working. This table names that chrome per harness,
//! and [`modal_dialog`] matches it in a captured pane. A marker counts only
//! where the harness draws it: anchored to the BOTTOM of the screen, below the
//! last rule, where the composer would be. The same words quoted in the
//! transcript (a seat whose work is about dialogs, a fixture printed to the
//! pane) sit above the composer and its footer, so they never match.
//!
//! A match is named by the marker's fixed `label`, never by text read from
//! the pane: the dialog's own lines can quote the seat's work, and the label
//! is stored, sent to other seats and shown on the roster (security S156001).
//!
//! **Adding a harness.** Capture the dialog's real screen (`tmux capture-pane
//! -p -t <pane>`) and a normal working screen, commit both under
//! `crates/testkit/fixtures/harnesses/` with a MANIFEST row, add a row here
//! naming where the marker sits and the key that clears it, and add the match
//! and no-match tests. A harness with no row is never read
//! ([`watches`]). OMP and Pi have none yet: no dialog of theirs has been
//! captured.

use pij_core::model::Harness;

/// Where a marker must sit for the screen to be the harness's own dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chrome {
    /// The screen ends `<marker line>`, a rule, then one status line.
    /// Copilot's selection footer (`↑/↓ select · ... · esc cancel`).
    FooterAboveStatus(&'static str),
    /// The screen ends with a rule and one status line, and the region the
    /// last two rules enclose opens with the marker. Copilot's dialog title.
    TitleAboveStatus(&'static str),
    /// The last line of the screen starts with the marker, below the last
    /// rule. Claude's permission prompt (`Esc to cancel · Tab to amend`).
    LastLine(&'static str),
}

/// One harness dialog pij knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModalMarker {
    /// The harness that draws it.
    pub harness: Harness,
    /// Where its marker sits.
    pub chrome: Chrome,
    /// What the dialog is, in fixed words: all pij ever says about its text.
    pub label: &'static str,
    /// The key that clears it, short, for `pij list`.
    pub key: &'static str,
    /// The key that clears it, in words, for the parent and the PA.
    pub clears: &'static str,
}

/// Copilot's dialogs clear with ctrl+d, which declines (pm-001 cleared both
/// observed stalls that way, 2026-11-06 and 2026-11-07).
const COPILOT_CLEARS: &str =
    "ctrl+d declines (enter would accept the highlighted option, esc cancels)";

/// Every dialog pij recognises. Fixtures: `copilot-follow-up-dialog-*.txt`
/// (pij-bumpy-herring, pane %17, 2026-11-07 and F108 on 2026-11-08;
/// pij-wary-lynx, 2026-11-06) and
/// `claude-2.1.284-permission-dialog.txt`.
pub const MODAL_MARKERS: &[ModalMarker] = &[
    ModalMarker {
        harness: Harness::Copilot,
        chrome: Chrome::FooterAboveStatus("↑/↓ select · enter accept · ctrl+d decline"),
        label: "Copilot selection dialog (ctrl+d declines)",
        key: "ctrl+d",
        clears: COPILOT_CLEARS,
    },
    ModalMarker {
        harness: Harness::Copilot,
        chrome: Chrome::TitleAboveStatus("Copilot needs information"),
        label: "Copilot question dialog (ctrl+d declines)",
        key: "ctrl+d",
        clears: COPILOT_CLEARS,
    },
    ModalMarker {
        harness: Harness::Claude,
        chrome: Chrome::LastLine("Esc to cancel"),
        label: "Claude permission prompt (esc cancels)",
        key: "esc",
        clears: "esc cancels the request (enter would approve the highlighted option)",
    },
];

/// A dialog found on a screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModalMatch {
    /// The marker's fixed label.
    pub label: &'static str,
    /// The key that clears it, short.
    pub key: &'static str,
    /// The key that clears it, in words.
    pub clears: &'static str,
}

/// Does pij know any dialog of `harness`? A harness it doesn't know is never
/// read for one.
#[must_use]
pub fn watches(harness: Harness) -> bool {
    MODAL_MARKERS.iter().any(|marker| marker.harness == harness)
}

/// The harness's own dialog on `screen` (a pane capture), if one is showing.
#[must_use]
pub fn modal_dialog(harness: Harness, screen: &str) -> Option<ModalMatch> {
    let rows: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|row| !row.trim().is_empty())
        .collect();
    MODAL_MARKERS
        .iter()
        .filter(|marker| marker.harness == harness)
        .find(|marker| shows(&rows, marker.chrome))
        .map(|marker| ModalMatch {
            label: marker.label,
            key: marker.key,
            clears: marker.clears,
        })
}

/// Does `chrome` sit at the bottom of `rows`?
fn shows(rows: &[&str], chrome: Chrome) -> bool {
    let n = rows.len();
    match chrome {
        Chrome::FooterAboveStatus(marker) => {
            n >= 3 && is_rule(rows[n - 2]) && starts(rows[n - 3], marker)
        }
        Chrome::TitleAboveStatus(marker) => {
            let Some(bottom_rule) = n.checked_sub(2) else {
                return false;
            };
            let top = region_top(rows, bottom_rule);
            is_rule(rows[bottom_rule]) && top > 0 && top < bottom_rule && starts(rows[top], marker)
        }
        Chrome::LastLine(marker) => {
            let Some(last) = n.checked_sub(1) else {
                return false;
            };
            rows.iter()
                .rposition(|row| is_rule(row))
                .is_some_and(|rule| rule < last && starts(rows[last], marker))
        }
    }
}

/// The first row after the nearest rule above `below`, or the first row.
fn region_top(rows: &[&str], below: usize) -> usize {
    rows[..below]
        .iter()
        .rposition(|row| is_rule(row))
        .map_or(0, |rule| rule + 1)
}

/// A full-width horizontal rule: nothing but `─`.
fn is_rule(row: &str) -> bool {
    let row = row.trim();
    row.chars().count() >= 8 && row.chars().all(|c| c == '─')
}

fn starts(row: &str, marker: &str) -> bool {
    row.trim_start().starts_with(marker)
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

    /// bp-0002: the real Copilot screens herring and lynx sat on match, and
    /// say ctrl+d.
    #[test]
    fn the_captured_copilot_follow_up_dialogs_match() {
        let herring = modal_dialog(
            Harness::Copilot,
            &fixture("copilot-follow-up-dialog-herring.txt"),
        )
        .expect("herring's screen is a dialog");
        assert_eq!(herring.label, "Copilot selection dialog (ctrl+d declines)");
        assert_eq!(herring.key, "ctrl+d");
        assert!(herring.clears.starts_with("ctrl+d declines"));
        let lynx = modal_dialog(
            Harness::Copilot,
            &fixture("copilot-follow-up-dialog-lynx.txt"),
        )
        .expect("lynx's screen is a dialog");
        assert_eq!(lynx.label, "Copilot selection dialog (ctrl+d declines)");
        assert_eq!(lynx.clears, herring.clears);
        let f108 = modal_dialog(
            Harness::Copilot,
            &fixture("copilot-follow-up-dialog-herring-f108.txt"),
        )
        .expect("herring's F108 screen is a dialog");
        assert_eq!(f108, herring);
    }

    /// The title alone is enough when the footer differs.
    #[test]
    fn copilots_title_matches_without_the_selection_footer() {
        let screen = fixture("copilot-follow-up-dialog-herring.txt").replace(
            "↑/↓ select · enter accept · ctrl+d decline · esc cancel",
            "enter submit · esc cancel",
        );
        let found = modal_dialog(Harness::Copilot, &screen).expect("titled dialog");
        assert_eq!(found.label, "Copilot question dialog (ctrl+d declines)");
    }

    #[test]
    fn claudes_permission_prompt_matches() {
        let found = modal_dialog(
            Harness::Claude,
            &fixture("claude-2.1.284-permission-dialog.txt"),
        )
        .expect("Claude's permission dialog");
        assert_eq!(found.label, "Claude permission prompt (esc cancels)");
        assert_eq!(found.key, "esc");
        assert!(found.clears.starts_with("esc cancels"));
    }

    /// bp-0002: normal working and ready screens never match.
    #[test]
    fn working_and_ready_screens_do_not_match() {
        for (harness, name) in [
            (Harness::Copilot, "copilot-working.txt"),
            (Harness::Copilot, "copilot-ready.txt"),
            (Harness::Claude, "claude-2.1.284-idle.txt"),
            (Harness::Claude, "claude-2.1.284-streaming.txt"),
            (Harness::Claude, "claude-2.1.284-tool-run.txt"),
            (Harness::Claude, "claude-2.1.284-after-esc-manual-mode.txt"),
        ] {
            assert_eq!(modal_dialog(harness, &fixture(name)), None, "{name}");
        }
    }

    /// bp-0002: a seat whose work is ABOUT dialogs (the whole dialog screen,
    /// footer and rule included, quoted in its transcript or a code block)
    /// does not trip it: its own composer and status sit below the quote.
    #[test]
    fn a_dialog_quoted_above_the_composer_does_not_match() {
        let working = fixture("copilot-working.txt");
        let quoted = format!(
            "```\n{}```\n{working}",
            fixture("copilot-follow-up-dialog-herring.txt")
        );
        assert_eq!(modal_dialog(Harness::Copilot, &quoted), None);
        let claude_dialog = fixture("claude-2.1.284-permission-dialog.txt");
        let claude_idle = fixture("claude-2.1.284-idle.txt");
        assert_eq!(
            modal_dialog(Harness::Claude, &format!("{claude_dialog}\n{claude_idle}")),
            None
        );
        // The footer typed into a composer is not the footer (the title is
        // removed too, so only the footer rule is under test).
        let untitled = fixture("copilot-follow-up-dialog-herring.txt")
            .replace("Copilot needs information.", "Some other line.");
        assert!(modal_dialog(Harness::Copilot, &untitled).is_some());
        let typed = untitled.replace(" ↑/↓ select · enter accept", " ❯ ↑/↓ select · enter accept");
        assert_eq!(modal_dialog(Harness::Copilot, &typed), None);
    }

    /// Security S156001: a match is named by its fixed label, whatever the
    /// dialog's own lines say, including a cropped dialog whose top row is
    /// the seat's work.
    #[test]
    fn a_match_never_carries_text_from_the_pane() {
        const CANARY: &str = "PIJ-CANARY-S156001";
        let herring = fixture("copilot-follow-up-dialog-herring.txt")
            .replace("Copilot needs information.", CANARY)
            .replace("Choose an optional follow-up", CANARY);
        let lynx = format!(
            "{CANARY} secret table row\n{}",
            fixture("copilot-follow-up-dialog-lynx.txt")
        );
        let claude =
            fixture("claude-2.1.284-permission-dialog.txt").replace("Bash command", CANARY);
        for (harness, screen) in [
            (Harness::Copilot, herring),
            (Harness::Copilot, lynx),
            (Harness::Claude, claude),
        ] {
            let found = modal_dialog(harness, &screen).expect("still detected");
            assert!(!format!("{found:?}").contains(CANARY), "{found:?}");
            assert!(
                MODAL_MARKERS
                    .iter()
                    .any(|marker| marker.label == found.label)
            );
        }
    }

    /// Markers are per harness: Claude's prompt on a Copilot seat is not
    /// Copilot's dialog, and OMP and Pi have no markers, so they are never read.
    #[test]
    fn markers_belong_to_their_harness() {
        assert_eq!(
            modal_dialog(
                Harness::Copilot,
                &fixture("claude-2.1.284-permission-dialog.txt")
            ),
            None
        );
        assert!(watches(Harness::Copilot) && watches(Harness::Claude));
        assert!(!watches(Harness::Omp) && !watches(Harness::Pi) && !watches(Harness::Codex));
    }
}
