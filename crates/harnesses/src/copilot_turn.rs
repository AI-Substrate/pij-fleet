//! Whether a Copilot pane shows a turn running, read from its screen (F279).
//!
//! The pij Copilot extension publishes the seat's turn state. When that
//! extension is gone (its lease lapsed: Copilot's enterprise sign-in ran out
//! and Copilot killed it), the published state is frozen at its last write,
//! so only the screen still says whether a turn runs. Copilot draws its
//! composer the same way mid-turn and at the prompt, an empty box, in one of
//! two layouts: a `❯` row between two rules, or a `┃` row between a `╻▄▄…`
//! and a `╹▀▀…` border. Only the row under the box differs. At the prompt it
//! is the footer (`← open sidebar · Interactive · Allow All · / commands`,
//! cut short in a narrow pane: `… / com…`); mid-turn it is the turn's status,
//! ending `ctrl+c interrupt` (`◉ Working · 313 B ctrl+c interrupt`, or the
//! turn's own words: `○ Reviewing destructiv… · 61.3 KiB ctrl+c interrupt`).
//!
//! Only that row is read, so the same words quoted in the transcript above
//! the box never count. Idle needs POSITIVE evidence (the footer, and an
//! empty composer); any other screen, a dialog included, is
//! [`CopilotTurn::Unreadable`]. Fixtures: `copilot-1.0.115-*.txt` and
//! `copilot-working.txt`, all captured from live panes.

/// What a Copilot screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopilotTurn {
    /// The footer, and an empty composer: the turn has ended.
    Idle,
    /// The row under the composer is a running turn's status.
    Busy,
    /// The footer, with text in the composer.
    Draft,
    /// No composer box, no row under it, or a row that is neither the footer
    /// nor a turn's status.
    Unreadable,
}

/// Words only a running turn's status row shows (it always keeps its
/// `ctrl+c interrupt` and cuts its own status short instead).
const BUSY: &[&str] = &["interrupt", "ctrl+c"];
/// Words only the footer at the prompt shows, each early in the row, so a
/// narrow pane that cuts `/ commands` to `/ com…` still shows one.
const FOOTER: &[&str] = &["open sidebar", "Interactive", "/ com", "? help"];

/// Read a captured Copilot screen: the last composer box and the first
/// non-blank row under it.
#[must_use]
pub fn copilot_turn(screen: &str) -> CopilotTurn {
    let rows: Vec<&str> = screen.lines().collect();
    let Some(bottom) = rows
        .iter()
        .rposition(|row| edge(row).is_some_and(|e| e.closes))
    else {
        return CopilotTurn::Unreadable;
    };
    let layout = edge(rows[bottom]).map(|e| e.layout);
    let Some(top) = rows[..bottom]
        .iter()
        .rposition(|row| edge(row).is_some_and(|e| e.opens && Some(e.layout) == layout))
    else {
        return CopilotTurn::Unreadable;
    };
    let mut composer = Vec::new();
    for (index, row) in rows[top + 1..bottom].iter().enumerate() {
        let row = row.trim_start();
        let text = match layout {
            Some(Layout::Rules) if index == 0 => row.strip_prefix('❯'),
            Some(Layout::Rules) => Some(row),
            _ => row.strip_prefix('┃'),
        };
        let Some(text) = text else {
            return CopilotTurn::Unreadable;
        };
        composer.push(text);
    }
    if composer.is_empty() {
        return CopilotTurn::Unreadable;
    }
    let Some(below) = rows[bottom + 1..].iter().find(|row| !row.trim().is_empty()) else {
        return CopilotTurn::Unreadable;
    };
    let lower = below.to_ascii_lowercase();
    if BUSY.iter().any(|word| lower.contains(word)) {
        return CopilotTurn::Busy;
    }
    if !FOOTER.iter().any(|word| below.contains(word)) {
        return CopilotTurn::Unreadable;
    }
    if composer.iter().all(|row| row.trim().is_empty()) {
        CopilotTurn::Idle
    } else {
        CopilotTurn::Draft
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// `─` rules above and below a `❯` row.
    Rules,
    /// A `╻▄▄…` border above and a `╹▀▀…` border below `┃` rows.
    Box,
}

struct Edge {
    layout: Layout,
    opens: bool,
    closes: bool,
}

/// A composer box's border row, if `row` is one.
fn edge(row: &str) -> Option<Edge> {
    let row = row.trim();
    let filled =
        |rest: &str, fill: char| rest.chars().count() >= 8 && rest.chars().all(|c| c == fill);
    if filled(row, '─') {
        return Some(Edge {
            layout: Layout::Rules,
            opens: true,
            closes: true,
        });
    }
    if row.strip_prefix('╻').is_some_and(|rest| filled(rest, '▄')) {
        return Some(Edge {
            layout: Layout::Box,
            opens: true,
            closes: false,
        });
    }
    if row.strip_prefix('╹').is_some_and(|rest| filled(rest, '▀')) {
        return Some(Edge {
            layout: Layout::Box,
            opens: false,
            closes: true,
        });
    }
    None
}

impl CopilotTurn {
    /// The brake's stable name, for a deferral or a recovery record:
    /// `idle`, `busy`, `composer-draft`, `composer-unreadable`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Draft => "composer-draft",
            Self::Unreadable => "composer-unreadable",
        }
    }
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

    /// The live screens, in both layouts and a narrow pane: the prompt is
    /// idle, and a turn shows in the row under the box, whether it says
    /// Working or the turn's own status.
    #[test]
    fn the_captured_copilot_screens_read_as_they_were() {
        for (name, turn) in [
            ("copilot-1.0.115-idle.txt", CopilotTurn::Idle),
            ("copilot-1.0.115-idle-box.txt", CopilotTurn::Idle),
            ("copilot-1.0.115-idle-narrow.txt", CopilotTurn::Idle),
            ("copilot-ready.txt", CopilotTurn::Idle),
            ("copilot-1.0.115-working.txt", CopilotTurn::Busy),
            ("copilot-1.0.115-working-status.txt", CopilotTurn::Busy),
            ("copilot-working.txt", CopilotTurn::Busy),
        ] {
            assert_eq!(copilot_turn(&fixture(name)), turn, "{name}");
        }
    }

    /// Text in the composer, on one row or more and in either layout, is a
    /// draft; the same text mid-turn is still busy.
    #[test]
    fn text_in_the_composer_is_a_draft() {
        let idle = fixture("copilot-1.0.115-idle.txt");
        let one = idle.replacen("❯\n", "❯ fix the lint\n", 1);
        assert_eq!(copilot_turn(&one), CopilotTurn::Draft);
        let two = idle.replacen("❯\n", "❯\n  and then ship\n", 1);
        assert_eq!(copilot_turn(&two), CopilotTurn::Draft);
        let boxed = fixture("copilot-1.0.115-idle-box.txt").replacen("┃\n", "┃ fix it\n", 1);
        assert_eq!(copilot_turn(&boxed), CopilotTurn::Draft);
        let working = fixture("copilot-1.0.115-working.txt").replacen("❯\n", "❯ next\n", 1);
        assert_eq!(copilot_turn(&working), CopilotTurn::Busy);
    }

    /// Idle needs the footer under an empty composer. Words quoted above the
    /// box, a dialog in its place, a missing or unknown row under it, a box
    /// whose borders do not match, and an empty capture are never idle; a
    /// quoted `interrupt` is not busy.
    #[test]
    fn idle_needs_the_footer_under_an_empty_composer() {
        let idle = fixture("copilot-1.0.115-idle.txt");
        let quoted = format!("◉ Working · 1 KiB ctrl+c interrupt\n{idle}");
        assert_eq!(copilot_turn(&quoted), CopilotTurn::Idle);
        let dialog = idle.replacen("❯\n", "Copilot needs information.\n", 1);
        assert_eq!(copilot_turn(&dialog), CopilotTurn::Unreadable);
        let cut: Vec<&str> = idle.lines().take(4).collect();
        assert_eq!(copilot_turn(&cut.join("\n")), CopilotTurn::Unreadable);
        let mut other: Vec<&str> = idle.lines().collect();
        other[4] = " something else entirely";
        assert_eq!(copilot_turn(&other.join("\n")), CopilotTurn::Unreadable);
        let boxed = fixture("copilot-1.0.115-idle-box.txt");
        let mixed = boxed.replacen('╻', "─", 1).replace('▄', "─");
        assert_eq!(copilot_turn(&mixed), CopilotTurn::Unreadable);
        assert_eq!(copilot_turn(""), CopilotTurn::Unreadable);
    }
}
