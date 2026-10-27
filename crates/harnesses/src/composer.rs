/// The composer portion recognized in a rendered terminal capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerRegion {
    Recognized(String),
    Unrecognized,
}

/// Extract composer text up to the caret from a rendered terminal capture.
#[must_use]
pub fn composer_region(capture: &str, cursor_x: u32, cursor_y: u32) -> ComposerRegion {
    rendered_composer(capture, cursor_x, cursor_y)
        .map_or(ComposerRegion::Unrecognized, ComposerRegion::Recognized)
}

fn rendered_composer(capture: &str, cursor_x: u32, cursor_y: u32) -> Option<String> {
    let lines: Vec<&str> = capture.lines().collect();
    if let Some((row, first_column, payload)) = copilot_v1_composer(&lines) {
        return clip_payload(payload, row, first_column, cursor_x, cursor_y);
    }

    for (row, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim();
        if let Some(inner) = trimmed
            .strip_prefix('╰')
            .and_then(|inner| inner.strip_suffix('╯'))
        {
            let leading = line.chars().position(|ch| ch == '╰')?;
            let left_rules = inner.chars().take_while(|ch| *ch == '─').count();
            let after_rules: String = inner.chars().skip(left_rules).collect();
            let spacing = after_rules
                .chars()
                .take_while(|ch| ch.is_whitespace())
                .count();
            let payload: String = after_rules
                .chars()
                .skip(spacing)
                .collect::<String>()
                .trim_end_matches('─')
                .trim_end()
                .to_string();
            if payload.is_empty() && usize::try_from(cursor_y).ok()? == row {
                return Some(String::new());
            }
            let first_column = leading + 1 + left_rules + spacing;
            if let Some(clipped) = clip_payload(&payload, row, first_column, cursor_x, cursor_y) {
                return Some(clipped);
            }
        }
    }

    let rules: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| is_plain_rule(line).then_some(row))
        .collect();
    for pair in (1..rules.len()).rev() {
        let lower = rules[pair - 1];
        let upper = rules[pair];
        let interior = &lines[lower + 1..upper];
        if interior
            .first()
            .is_some_and(|line| line.trim_start().starts_with('❯'))
        {
            return clip_multiline(interior, lower + 1, cursor_x, cursor_y);
        }
    }

    if let Some(prompt) = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| line.trim_start().starts_with('›'))
    {
        let trailing = &lines[prompt.0 + 1..];
        if trailing.iter().any(|line| is_codex_footer(line)) {
            return clip_multiline(&lines[prompt.0..prompt.0 + 1], prompt.0, cursor_x, cursor_y);
        }
    }

    let prompt = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| line.trim_start().starts_with('❯'))?;
    let trailing = &lines[prompt.0 + 1..];
    if !trailing.iter().any(|line| is_copilot_footer(line)) {
        return None;
    }
    let composer_end = trailing
        .iter()
        .position(|line| is_copilot_footer(line))
        .map_or(lines.len(), |offset| prompt.0 + 1 + offset);
    clip_multiline(&lines[prompt.0..composer_end], prompt.0, cursor_x, cursor_y)
}

fn copilot_v1_composer<'a>(lines: &'a [&'a str]) -> Option<(usize, usize, &'a str)> {
    for (top_row, window) in lines.windows(3).enumerate().rev() {
        if !is_copilot_box_rule(window[0], '╻', '▄')
            || !is_copilot_box_rule(window[2], '╹', '▀')
            || !lines[top_row + 3..]
                .iter()
                .take(2)
                .any(|line| is_copilot_footer(line))
        {
            continue;
        }

        let line = window[1];
        let trimmed = line.trim_start();
        let Some(after_border) = trimmed.strip_prefix('┃') else {
            continue;
        };
        let leading = line.chars().count() - trimmed.chars().count();
        let (payload, padding) = after_border.chars().next().map_or((after_border, 0), |ch| {
            if ch.is_whitespace() {
                (&after_border[ch.len_utf8()..], 1)
            } else {
                (after_border, 0)
            }
        });
        return Some((top_row + 1, leading + 1 + padding, payload));
    }
    None
}

fn is_copilot_box_rule(line: &str, corner: char, fill: char) -> bool {
    let Some(rule) = line.trim().strip_prefix(corner) else {
        return false;
    };
    rule.chars().count() >= 8 && rule.chars().all(|ch| ch == fill)
}

fn clip_multiline(
    lines: &[&str],
    start_row: usize,
    cursor_x: u32,
    cursor_y: u32,
) -> Option<String> {
    let active = usize::try_from(cursor_y).ok()?.checked_sub(start_row)?;
    if active >= lines.len() {
        return None;
    }
    let mut clipped: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
    clipped[active] = take_chars(&clipped[active], usize::try_from(cursor_x).ok()?);
    if let Some(first) = clipped.first_mut() {
        *first = strip_prompt(first);
    }
    Some(clipped.join("\n"))
}

fn clip_payload(
    payload: &str,
    row: usize,
    first_column: usize,
    cursor_x: u32,
    cursor_y: u32,
) -> Option<String> {
    if usize::try_from(cursor_y).ok()? != row {
        return None;
    }
    let column = usize::try_from(cursor_x).ok()?.checked_sub(first_column)?;
    Some(take_chars(payload, column))
}

fn take_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn strip_prompt(line: &str) -> String {
    let trimmed = line.trim_start();
    ['❯', '›']
        .into_iter()
        .find_map(|prompt| trimmed.strip_prefix(prompt))
        .map_or_else(|| line.to_string(), |rest| rest.trim_start().to_string())
}

fn is_plain_rule(line: &str) -> bool {
    const BOX: [char; 20] = [
        '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼', '╭', '╮', '╰', '╯', '╠', '╣', '╦', '╩', '╬',
        '│', '║',
    ];
    !line.chars().any(|ch| BOX.contains(&ch))
        && line
            .split(|ch| ch != '─')
            .any(|run| run.chars().count() >= 8)
}

fn is_copilot_footer(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    lower.starts_with("/ commands")
        || lower.starts_with("← open sidebar · / commands · ? help · tab next tab")
        || lower.starts_with("@ files · # issues")
        || lower.contains("esc interrupt")
        || lower.contains("context")
}

/// The parts of a Codex footer line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CodexFooter<'a> {
    pub(crate) model: &'a str,
    pub(crate) cwd: Option<&'a str>,
}

/// Codex renders `<model> <effort> · <cwd>` under its prompt, and since 0.164
/// `<model> <effort> · <cwd> · <session title>`. ONE definition for readiness,
/// model/cwd extraction and composer recognition: two copies once disagreed
/// (the composer copy required `~/`, so a seat with a cwd outside `$HOME` was
/// held `unrecognized` forever — 2026-09-08, pane %4409), and reading only the
/// text after the LAST `·` took the title for the cwd (2026-10-24, pane %100).
///
/// The model is the first segment's first word; the cwd is the first later
/// segment that is a path, so a title after it — even one containing `·` or
/// shaped like a path — is never read as data. A usage segment
/// (`85% context left`) also marks a footer, without a cwd.
///
/// The first segment is exactly `<model> <effort>`: Codex's quota warning
/// `⚠ 5h limit: 49% left · resets at 12:33 AM on 27 Oct · /status` has the
/// rest of a footer's shape and was read as model `⚠`, cwd `/status`
/// (2026-10-26, pane %53). Every recorded footer has a two-word head, so a
/// longer one reads unrecognized rather than misread.
pub(crate) fn parse_codex_footer(line: &str) -> Option<CodexFooter<'_>> {
    let mut segments = line.split('·').map(str::trim);
    let mut head = segments.next()?.split_whitespace();
    let model = head.next()?;
    head.next()?;
    if head.next().is_some() {
        return None;
    }
    let mut usage = false;
    for segment in segments {
        if segment.starts_with('/') || segment.starts_with("~/") {
            return Some(CodexFooter {
                model,
                cwd: Some(segment),
            });
        }
        usage |= segment.contains(" context") || segment.contains(" tokens");
    }
    usage.then_some(CodexFooter { model, cwd: None })
}

pub(crate) fn is_codex_footer(line: &str) -> bool {
    parse_codex_footer(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter_capture(recorded: &str) -> String {
        recorded.lines().collect::<Vec<_>>().join("\n")
    }

    fn framed(composer: &str) -> String {
        format!("transcript\n────────────\n❯ {composer}\n────────────")
    }

    #[test]
    fn live_recorded_layouts_recognize_the_cursor_composer() {
        let fixtures = [
            (
                "OMP recorded 2026-08-31 from pane %1985",
                include_str!("../tests/fixtures/2026-08-31-omp-pane-1985.txt"),
                2,
                43,
                ComposerRegion::Recognized(String::new()),
            ),
            (
                "Claude recorded 2026-08-31 from pane %1973",
                include_str!("../tests/fixtures/2026-08-31-claude-pane-1973.txt"),
                2,
                42,
                ComposerRegion::Recognized(String::new()),
            ),
        ];
        for (provenance, capture, cursor_x, cursor_y, expected) in fixtures {
            assert_eq!(
                composer_region(&adapter_capture(capture), cursor_x, cursor_y),
                expected,
                "{provenance}"
            );
        }
    }

    #[test]
    fn live_copilot_v1_box_is_recognized_without_prompt_marker() {
        let capture = include_str!("../tests/fixtures/2026-08-31-copilot-v1.0.82-pane-1962.txt");
        assert_eq!(
            composer_region(&adapter_capture(capture), 2, 42),
            ComposerRegion::Recognized(String::new()),
            "Copilot v1.0.82 recorded 2026-08-31 from pane %1962"
        );
    }

    #[test]
    fn copilot_v1_0_84_captured_idle_footer_is_recognized() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/2026-09-05-copilot-v1.0.84-smoke03.json"
        ))
        .expect("captured Copilot fixture is valid JSON");
        let capture = fixture["terminal"]
            .as_str()
            .expect("fixture retains the actual terminal capture");
        let footer = capture.lines().last().expect("capture contains a footer");
        assert!(
            is_copilot_footer(footer),
            "Copilot v1.0.84-0 smoke03 captured idle footer: {footer:?}"
        );
    }

    #[test]
    fn copilot_v1_0_84_captured_draft_footer_is_recognized() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/2026-09-05-copilot-v1.0.84-live06-draft.json"
        ))
        .expect("captured Copilot fixture is valid JSON");
        let capture = fixture["terminal"]
            .as_str()
            .expect("fixture retains the actual terminal capture");
        let footer = capture.lines().last().expect("capture contains a footer");
        assert!(
            is_copilot_footer(footer),
            "Copilot v1.0.84-0 live06 captured draft footer: {footer:?}"
        );
    }

    #[test]
    fn copilot_v1_0_84_live08_paired_capture_recognizes_full_draft() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/2026-09-05-copilot-v1.0.84-live06-draft.json"
        ))
        .expect("captured Copilot fixture is valid JSON");
        assert!(
            fixture["cursor"].is_null(),
            "live08 cursor must not be attached to the original live06 frame"
        );
        let follow_up = &fixture["follow_up"];
        let frame = &follow_up["pane_capture"];
        let capture = frame["terminal"]
            .as_str()
            .expect("follow-up retains its own unjoined terminal frame");
        let cursor_x = u32::try_from(frame["cursor_x"].as_u64().expect("measured cursor x"))
            .expect("cursor x fits u32");
        let cursor_y = u32::try_from(frame["cursor_y"].as_u64().expect("measured cursor y"))
            .expect("cursor y fits u32");
        let draft = follow_up["draft"]
            .as_str()
            .expect("expected draft comes from the live08 typing witness");
        assert_eq!(
            composer_region(capture, cursor_x, cursor_y),
            ComposerRegion::Recognized(draft.to_owned()),
            "Copilot v1.0.84-0 live08 accepted frame at its measured cursor"
        );
    }

    #[test]
    fn copilot_footer_preserves_existing_variants() {
        for footer in [
            "/ commands · ? help",
            "esc interrupt",
            "Claude Opus 5 · 1M context",
        ] {
            assert!(is_copilot_footer(footer), "existing footer: {footer:?}");
        }
    }

    #[test]
    fn copilot_footer_rejects_command_help_in_prose() {
        assert!(!is_copilot_footer(
            "Use / commands to open the menu and ? help for documentation."
        ));
    }

    #[test]
    fn copilot_footer_rejects_draft_help_in_prose_or_partial_hints() {
        for line in [
            "See @ files · # issues for details.",
            "@ files",
            "# issues",
            "@ files # issues",
            "@ files · # issue",
            "# issues · @ files",
        ] {
            assert!(!is_copilot_footer(line), "not a draft footer: {line:?}");
        }
    }

    #[test]
    fn copilot_v1_box_clips_typed_text_at_the_cursor() {
        let capture = [
            "╻▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄",
            "┃ hello suggestion",
            "╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀",
            "v1.0.82 · / commands · context",
        ]
        .join("\n");
        assert_eq!(
            composer_region(&capture, 7, 1),
            ComposerRegion::Recognized("hello".to_string())
        );
    }

    #[test]
    fn garbage_layout_is_unrecognized() {
        assert_eq!(
            composer_region("ordinary shell output", 1, 0),
            ComposerRegion::Unrecognized
        );
    }

    #[test]
    fn cursor_clips_suggestion_text_to_the_right() {
        assert_eq!(
            composer_region(&framed("hello suggested-text"), 7, 2),
            ComposerRegion::Recognized("hello".to_string())
        );
    }

    #[test]
    fn cursor_outside_recognized_region_is_unrecognized() {
        assert_eq!(
            composer_region(&framed("hello"), 0, 0),
            ComposerRegion::Unrecognized
        );
    }

    #[test]
    fn omp_inline_composer_is_clipped_at_the_measured_cursor() {
        assert_eq!(
            composer_region("╰─ hello suggested-text ─╯", 8, 0),
            ComposerRegion::Recognized("hello".to_string())
        );
        assert_eq!(
            composer_region("╰─     ─╯", 3, 0),
            ComposerRegion::Recognized(String::new())
        );
    }

    #[test]
    fn rounded_border_above_cursor_does_not_hide_later_composer() {
        let capture = concat!(
            "╰──────────┴──────────╯\n",
            "status text\n",
            "────────────────\n",
            "❯ \n",
            "────────────────",
        );
        assert_eq!(
            composer_region(capture, 2, 3),
            ComposerRegion::Recognized(String::new()),
            "a non-composer rounded border must not stop the scan before the cursor's composer"
        );
    }

    #[test]
    fn decorative_blank_rule_region_is_not_an_idle_composer() {
        let capture = concat!(
            "╭────────────╮\n",
            "│ status box │\n",
            "╰────────────╯\n",
            "────────────────\n",
            "                \n",
            "────────────────",
        );
        assert_eq!(
            composer_region(capture, 2, 4),
            ComposerRegion::Unrecognized,
            "blank decoration without a prompt or harness footer cannot authorize staging"
        );
    }

    #[test]
    fn current_codex_prompt_and_footer_identify_the_composer() {
        let blank = concat!(
            "╭─ OpenAI Codex ─╮\n",
            "│ model: gpt-5.6 │\n",
            "╰────────────────╯\n",
            "\n",
            "› Use /skills to list available skills\n",
            "\n",
            "  gpt-5.6-sol xhigh · ~/pi-hacking/pij",
        );
        assert_eq!(
            composer_region(blank, 2, 4),
            ComposerRegion::Recognized(String::new())
        );

        let draft = blank.replace("› Use /skills to list available skills", "› human draft");
        assert_eq!(
            composer_region(&draft, 13, 4),
            ComposerRegion::Recognized("human draft".to_string())
        );
    }

    #[test]
    fn markdown_table_borders_are_not_composer_rules() {
        let capture = [
            "transcript",
            "┌────────────┬────────────┐",
            "│ table data │ more data │",
            "└────────────┴────────────┘",
            "────────────",
            "❯ hello",
            "────────────",
        ]
        .join("\n");
        assert_eq!(
            composer_region(&capture, 7, 5),
            ComposerRegion::Recognized("hello".to_string())
        );
    }

    #[test]
    fn copilot_footer_is_excluded_from_composer_text() {
        let capture = "❯ keepmeposted\n/ commands · context";
        assert_eq!(
            composer_region(capture, 14, 0),
            ComposerRegion::Recognized("keepmeposted".to_string())
        );
    }
}

#[cfg(test)]
mod codex_0_153_4 {
    use super::{ComposerRegion, composer_region};

    /// Recorded 2026-09-08 from pane %4409, Codex CLI 0.153.4, cursor (2, 50).
    /// The cwd is an absolute path outside `$HOME`; the composer's private
    /// footer rule required `~/`, so every delivery was held `unrecognized`
    /// (job 4864, 19+ retries).
    #[test]
    fn codex_0_153_4_absolute_cwd_footer_is_recognized() {
        let capture = include_str!("../tests/fixtures/2026-09-08-codex-v0.153.4-pane-4409.txt");
        assert_eq!(
            composer_region(capture, 2, 50),
            ComposerRegion::Recognized(String::new())
        );
        let draft = capture.replace("› Ask Codex to do anything", "› human draft");
        assert_eq!(
            composer_region(&draft, 13, 50),
            ComposerRegion::Recognized("human draft".to_string())
        );
    }
}

#[cfg(test)]
mod codex_0_164 {
    use super::{ComposerRegion, composer_region, is_codex_footer};

    /// Recorded 2026-10-24 from pane %100, Codex CLI 0.164, bottom lines only,
    /// cursor (2, 3). The footer gained a trailing session title
    /// (`<model> <effort> · <cwd> · <title>`); reading only the text after the
    /// LAST `·` saw the title, not the cwd, so every delivery to the seat was
    /// held `unrecognized`.
    const CAPTURE: &str = include_str!("../tests/fixtures/2026-10-24-codex-v0.164-pane-100.txt");

    #[test]
    fn codex_0_164_titled_footer_is_recognized() {
        assert_eq!(
            composer_region(CAPTURE, 2, 3),
            ComposerRegion::Recognized(String::new()),
            "idle empty composer"
        );
        let draft = CAPTURE.replace("› Ask Codex to do anything", "› human draft");
        assert_eq!(
            composer_region(&draft, 13, 3),
            ComposerRegion::Recognized("human draft".to_string()),
            "a draft is recognized, so it is held composer-busy, not unrecognized"
        );
    }

    #[test]
    fn titled_footer_variants_are_footers() {
        for footer in [
            "  GPT-6.1-Sol high · ~/github/novels · Adopt seat and wait",
            "  gpt-6-astra high · /private/tmp/scratch · Adopt seat and wait",
            "  gpt-6-astra high · ~/github/novels · Title · with a dot",
            "  gpt-6-astra high · ~/github/novels · /review the diff",
        ] {
            assert!(is_codex_footer(footer), "{footer:?}");
        }
    }

    #[test]
    fn codex_hint_line_is_not_a_footer() {
        let hint = CAPTURE
            .lines()
            .last()
            .expect("fixture ends with the hint line");
        assert!(!is_codex_footer(hint), "{hint:?}");
    }

    /// rk-0001: a transcript line shaped like a footer above the prompt does not
    /// make the composer recognisable; only a footer after the prompt does.
    #[test]
    fn transcript_line_above_the_prompt_is_not_the_footer() {
        let capture = "ran it high · ~/github/novels · done\n\n› human draft\n";
        assert_eq!(
            composer_region(capture, 13, 2),
            ComposerRegion::Unrecognized
        );
    }
}

#[cfg(test)]
mod codex_0_164_warning_lines {
    use super::{ComposerRegion, composer_region, is_codex_footer};

    /// Recorded 2026-10-26 from pane %53 (a daemon-hosted Codex 0.164 seat),
    /// bottom lines only, cursor (2, 2). Codex prints a right-aligned quota
    /// warning above the prompt and a warnings count on the hint line.
    const DAEMON_HOSTED: &str =
        include_str!("../tests/fixtures/2026-10-26-codex-v0.164-pane-53.txt");
    /// Recorded 2026-10-28 from pane %113 (an in-process Codex 0.164 seat with
    /// a session title), bottom lines only, cursor (2, 0).
    const TITLED: &str = include_str!("../tests/fixtures/2026-10-28-codex-v0.164-pane-113.txt");

    /// The quota warning has `·` segments and a `/`-word, the shape a footer
    /// has; read as one it named model `⚠` and cwd `/status` (plan 181).
    #[test]
    fn quota_warning_line_is_not_a_footer() {
        let warning = DAEMON_HOSTED
            .lines()
            .next()
            .expect("fixture starts with the warning line");
        assert!(warning.contains("5h limit"), "{warning:?}");
        assert!(!is_codex_footer(warning), "{warning:?}");
    }

    #[test]
    fn hint_lines_with_warning_counts_are_not_footers() {
        for capture in [DAEMON_HOSTED, TITLED] {
            let hint = capture.lines().last().expect("fixture ends with the hint");
            assert!(hint.contains("f2 to view"), "{hint:?}");
            assert!(!is_codex_footer(hint), "{hint:?}");
        }
    }

    #[test]
    fn composer_is_recognized_with_warning_lines_present() {
        assert_eq!(
            composer_region(DAEMON_HOSTED, 2, 2),
            ComposerRegion::Recognized(String::new()),
            "daemon-hosted layout, quota warning above the prompt"
        );
        assert_eq!(
            composer_region(TITLED, 2, 0),
            ComposerRegion::Recognized(String::new()),
            "in-process layout with a session title"
        );
    }
}
