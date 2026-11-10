//! End-of-turn `pij` blocks (plan 226).
//!
//! A seat ends its reply with a fenced `pij` block of fire-and-forget commands,
//! and its harness hands the final reply text to the daemon when the turn ends.
//! This module finds that block, splits it into commands, judges what each
//! verb's route answered, and renders the result: one line for the human and,
//! on failure, the text of the seat's one follow-up turn. The daemon validates
//! every command with its verb's own parser and runs it through its verb's own
//! route; nothing here runs anything.
//!
//! Only a block that ENDS the reply runs: a block quoted mid-reply, or followed
//! by prose, runs nothing, because pij's docs are full of example commands and
//! quoting one must never send it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::render;

/// The fence's language tag: a block opens with a backtick fence (three or
/// more) whose info string is `pij`, and closes with a bare fence at least as
/// long. A `pij` fence inside another open fence is content.
pub const FENCE_TAG: &str = "pij";

/// The most commands one block may hold; more is invalid and nothing runs. A
/// brake on a runaway reply, not a quota.
pub const MAX_COMMANDS: usize = 20;

/// How much of a reason the human's one-line summary carries.
pub const SUMMARY_REASON_CHARS: usize = 120;

/// A verb a block may run. Nothing else, and never a shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verb {
    /// `send …`, through the route `pij send` uses.
    #[serde(rename = "send")]
    Send,
    /// `output …`, through the route `pij output` uses.
    #[serde(rename = "output")]
    Output,
    /// `requests close …`, through the route `pij requests close` uses.
    #[serde(rename = "requests close")]
    RequestsClose,
}

impl Verb {
    /// The verb as a seat writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Output => "output",
            Self::RequestsClose => "requests close",
        }
    }
}

/// One command from the block, before its verb's parser has seen it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// 1-based position among the block's commands (blank and `#` lines skipped).
    pub line: usize,
    /// The command as written, trimmed.
    pub text: String,
    /// The verb and the argv its route takes (`["send", …]`, `["output", …]`,
    /// `["requests", "close", …]`), or why the line is invalid already.
    pub parsed: Result<(Verb, Vec<String>), String>,
}

/// The commands of the fenced `pij` block that ends `text`, or `None` when the
/// text does not end with one.
#[must_use]
pub fn block(text: &str) -> Option<Vec<Line>> {
    let body = trailing_block_body(text)?;
    let mut lines = Vec::new();
    for raw in body {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let line = lines.len() + 1;
        let parsed = tokenize(trimmed).and_then(classify);
        lines.push(Line {
            line,
            text: trimmed.to_string(),
            parsed,
        });
    }
    Some(lines)
}

/// The cheap check a harness makes before calling the daemon: the trimmed text
/// ends with a closing fence and some line opens a `pij` fence. The daemon's
/// [`block`] is the real parse.
#[must_use]
pub fn may_end_with_block(text: &str) -> bool {
    text.trim_end().ends_with("```") && text.lines().any(is_open_fence)
}

fn is_open_fence(line: &str) -> bool {
    fence(line).is_some_and(|fence| fence.is_pij())
}

/// A Markdown code fence line: three or more backticks or tildes, then an
/// info string (which, for backticks, holds no backtick).
struct Fence<'a> {
    marker: char,
    len: usize,
    info: &'a str,
}

impl Fence<'_> {
    fn is_pij(&self) -> bool {
        self.marker == '`' && self.info == FENCE_TAG
    }

    /// Whether `line` closes this fence: the same marker, at least as long,
    /// and no info string.
    fn closed_by(&self, line: &Fence<'_>) -> bool {
        line.marker == self.marker && line.len >= self.len && line.info.is_empty()
    }
}

fn fence(line: &str) -> Option<Fence<'_>> {
    let trimmed = line.trim();
    let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|c| *c == marker).count();
    // Both markers are one byte, so `len` is also a byte offset. The line is
    // trimmed, so `info` has no trailing space; a leading one makes it some
    // other fence, never `pij`, which keeps `` ```pij `` the one opener a
    // harness's substring check looks for.
    let info = &trimmed[len..];
    if len < 3 || (marker == '`' && info.contains('`')) {
        return None;
    }
    Some(Fence { marker, len, info })
}

/// The body lines of the text's last fenced block, when that block is a `pij`
/// fence, it is closed, and only whitespace follows it. Fences are tracked from
/// the start of the text, so a `` ```pij `` line inside another open fence (a
/// quoted example) is that fence's content, never a block.
fn trailing_block_body(text: &str) -> Option<Vec<&str>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut open: Option<(usize, Fence<'_>)> = None;
    let mut last: Option<(usize, usize, bool)> = None;
    for (index, line) in lines.iter().enumerate() {
        let Some(found) = fence(line) else {
            continue;
        };
        let closes = open
            .as_ref()
            .map(|(start, opener)| (*start, opener.closed_by(&found), opener.is_pij()));
        match closes {
            None => open = Some((index, found)),
            Some((start, true, pij)) => {
                last = Some((start, index, pij));
                open = None;
            }
            Some((_, false, _)) => {}
        }
    }
    let (start, close, pij) = last.filter(|_| open.is_none())?;
    if !pij
        || lines[close + 1..]
            .iter()
            .any(|line| !line.trim().is_empty())
    {
        return None;
    }
    Some(lines[start + 1..close].to_vec())
}

/// Split one command line, shell-style, without expanding anything: `'…'` is
/// literal; `"…"` honours `\"`, `\\`, `\$` and `` \` ``; a backslash outside
/// quotes escapes the next character. A quote cannot span lines.
///
/// # Errors
/// An unclosed quote, or a trailing lone backslash.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("unclosed ' quote".to_string()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(next @ ('"' | '\\' | '$' | '`')) => current.push(next),
                            Some(next) => {
                                current.push('\\');
                                current.push(next);
                            }
                            None => return Err("unclosed \" quote".to_string()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("unclosed \" quote".to_string()),
                    }
                }
            }
            '\\' => {
                started = true;
                current.push(
                    chars
                        .next()
                        .ok_or_else(|| "a trailing \\ escapes nothing".to_string())?,
                );
            }
            ' ' | '\t' => {
                if started {
                    argv.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                started = true;
                current.push(c);
            }
        }
    }
    if started {
        argv.push(current);
    }
    Ok(argv)
}

/// The verb and its route's argv. A leading `pij` is tolerated.
fn classify(mut argv: Vec<String>) -> Result<(Verb, Vec<String>), String> {
    if argv.first().is_some_and(|word| word == "pij") {
        argv.remove(0);
    }
    let verb = match (
        argv.first().map(String::as_str),
        argv.get(1).map(String::as_str),
    ) {
        (Some("send"), _) => Verb::Send,
        (Some("output"), _) => Verb::Output,
        (Some("requests"), Some("close")) => Verb::RequestsClose,
        (Some("requests"), _) => {
            return Err("only `requests close` may run in a block".to_string());
        }
        (Some(other), _) => {
            return Err(format!(
                "`{other}` cannot run in a block; the verbs are send, output and requests close"
            ));
        }
        (None, _) => return Err("empty command".to_string()),
    };
    if argv
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
    {
        return Err("--help prints help; it is not a command to run at turn end".to_string());
    }
    Ok((verb, argv))
}

/// What happened to the whole block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The text does not end with a `pij` block; nothing was looked at.
    None,
    /// Every command ran.
    Ran,
    /// A command failed validation, so nothing ran.
    Invalid,
    /// The commands ran in order until one was refused; the rest did not run.
    Refused,
}

/// What happened to one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineStatus {
    /// Its route accepted it.
    Ran,
    /// Its route refused it; later commands did not run.
    Refused,
    /// It failed validation; nothing in the block ran.
    Invalid,
    /// It never ran, because another command was invalid or refused.
    NotRun,
}

/// One command's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEndLine {
    /// 1-based position among the block's commands.
    pub line: usize,
    /// The command as written.
    pub command: String,
    /// The verb, when the line named one that may run.
    pub verb: Option<Verb>,
    /// What happened to it.
    pub status: LineStatus,
    /// One line: the receipt when it ran, the reason when it did not.
    pub detail: String,
    /// The refusal's code (`E-RS-…`), when it has one.
    pub code: Option<String>,
    /// The verb route's own envelope `data`, when it answered with one.
    pub result: Option<Value>,
}

/// The answer to one turn end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEndResult {
    /// What happened to the block.
    pub outcome: Outcome,
    /// One row per command, in block order.
    pub lines: Vec<TurnEndLine>,
    /// How many commands ran.
    pub ran: usize,
    /// One line for the human; `None` when there was no block.
    pub summary: Option<String>,
    /// The seat's follow-up turn text; `None` unless the block failed. The
    /// harness blocks with it only when its own stop hook is not already active.
    pub followup: Option<String>,
}

impl TurnEndResult {
    /// The answer for text that does not end with a `pij` block.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            outcome: Outcome::None,
            lines: Vec::new(),
            ran: 0,
            summary: None,
            followup: None,
        }
    }

    /// The answer for `lines`, with its summary and follow-up rendered.
    #[must_use]
    pub fn from_lines(outcome: Outcome, lines: Vec<TurnEndLine>) -> Self {
        let ran = lines
            .iter()
            .filter(|line| line.status == LineStatus::Ran)
            .count();
        let summary = Some(summary(outcome, &lines, ran));
        let followup = followup(outcome, &lines);
        Self {
            outcome,
            lines,
            ran,
            summary,
            followup,
        }
    }
}

fn summary(outcome: Outcome, lines: &[TurnEndLine], ran: usize) -> String {
    match outcome {
        Outcome::None => String::new(),
        Outcome::Ran => {
            let count = |verb: Verb| lines.iter().filter(|line| line.verb == Some(verb)).count();
            let parts: Vec<String> = [
                (Verb::Send, "sent"),
                (Verb::Output, "output"),
                (Verb::RequestsClose, "closed"),
            ]
            .into_iter()
            .filter_map(|(verb, word)| {
                let n = count(verb);
                (n > 0).then(|| format!("{n} {word}"))
            })
            .collect();
            if parts.is_empty() {
                "pij ✓ empty block, nothing to run".to_string()
            } else {
                format!("pij ✓ {}", parts.join(", "))
            }
        }
        Outcome::Invalid => {
            let bad: Vec<&TurnEndLine> = lines
                .iter()
                .filter(|line| line.status == LineStatus::Invalid)
                .collect();
            let numbers: Vec<String> = bad.iter().map(|line| line.line.to_string()).collect();
            let (noun, reason) = match bad.first() {
                Some(first) if bad.len() == 1 => ("line", first.detail.as_str()),
                Some(first) => ("lines", first.detail.as_str()),
                None => ("line", "invalid"),
            };
            format!(
                "pij ✗ nothing ran: {noun} {} invalid: {}",
                numbers.join(", "),
                clip(reason)
            )
        }
        Outcome::Refused => {
            let not_run = lines
                .iter()
                .filter(|line| line.status == LineStatus::NotRun)
                .count();
            match lines.iter().find(|line| line.status == LineStatus::Refused) {
                Some(refused) => format!(
                    "pij ✗ line {} refused: {} ({ran} ran, {not_run} not run)",
                    refused.line,
                    clip(&refused.detail)
                ),
                None => format!("pij ✗ refused ({ran} ran, {not_run} not run)"),
            }
        }
    }
}

fn followup(outcome: Outcome, lines: &[TurnEndLine]) -> Option<String> {
    let listed = |status: LineStatus, with_detail: bool| -> Vec<String> {
        lines
            .iter()
            .filter(|line| line.status == status)
            .map(|line| {
                // The command and the route's reason are caller text: fenced,
                // so a control character or a forged line never reaches the
                // seat or the human raw.
                let command = render::caller_text(&line.command);
                if with_detail {
                    format!(
                        "  {}. {command}  ->  {}",
                        line.line,
                        render::caller_text(&line.detail)
                    )
                } else {
                    format!("  {}. {command}", line.line)
                }
            })
            .collect()
    };
    match outcome {
        Outcome::None | Outcome::Ran => None,
        Outcome::Invalid => {
            let mut text = vec![
                "[pij] Your end-of-turn pij block did not run: nothing was sent.".to_string(),
                "Invalid:".to_string(),
            ];
            text.extend(listed(LineStatus::Invalid, true));
            text.push(
                "Fix it and end your reply with a corrected pij block, or say why not.".to_string(),
            );
            Some(text.join("\n"))
        }
        Outcome::Refused => {
            let mut text =
                vec!["[pij] Your end-of-turn pij block stopped at a refusal.".to_string()];
            let ran = listed(LineStatus::Ran, true);
            if !ran.is_empty() {
                text.push("Ran, do not resend:".to_string());
                text.extend(ran);
            }
            text.push("Refused:".to_string());
            text.extend(listed(LineStatus::Refused, true));
            let not_run = listed(LineStatus::NotRun, false);
            if !not_run.is_empty() {
                text.push("Not run:".to_string());
                text.extend(not_run);
            }
            text.push(
                "Fix or drop the refused line and end your reply with a new pij block holding only the lines still to run, or say why not."
                    .to_string(),
            );
            Some(text.join("\n"))
        }
    }
}

/// A reason's first line for the summary: fenced (escaped onto one line, in
/// quotes) and cut at [`SUMMARY_REASON_CHARS`].
fn clip(text: &str) -> String {
    render::fenced(
        text.lines().next().unwrap_or_default(),
        SUMMARY_REASON_CHARS,
    )
}

/// How a verb route answered, judged: `Ok(detail)` when it ran, `Err((detail,
/// code))` when it was refused. A route can refuse with an `ok:false` envelope,
/// or with an `ok:true` receipt whose outcome is `refused` (a holder that
/// declined a delivery).
///
/// # Errors
/// The refusal's one-line reason and its code, when the route refused.
pub fn judge(
    ok: bool,
    data: Option<&Value>,
    meta: Option<&str>,
    details: Option<&Value>,
    error: Option<&str>,
) -> Result<String, (String, Option<String>)> {
    if !ok {
        let reason = meta
            .map(str::to_string)
            .or_else(|| error.map(str::to_string))
            .unwrap_or_else(|| "refused".to_string());
        let code = details
            .and_then(|details| details.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| code_prefix(&reason));
        return Err((one_line(&reason), code));
    }
    let data = data.unwrap_or(&Value::Null);
    if data["outcome"]["outcome"] == "refused" {
        let reason = data["outcome"]["reason"]
            .as_str()
            .unwrap_or("the recipient refused it")
            .to_string();
        let code = code_prefix(&reason);
        return Err((one_line(&reason), code));
    }
    Ok(ran_detail(data))
}

fn ran_detail(data: &Value) -> String {
    if let Some(line) = data["line"].as_str() {
        return one_line(line);
    }
    if let Some(id) = data["request"]["id"].as_str() {
        let state = data["request"]["state"].as_str().unwrap_or("sent");
        return match data["request"]["holder"].as_str() {
            Some(holder) => format!("request {id} {state} to {holder}"),
            None => format!("request {id} {state}"),
        };
    }
    if let Some(msg_id) = data["msg_id"].as_str() {
        let outcome = data["outcome"]["outcome"].as_str().unwrap_or("sent");
        let receipt = match data["outcome"]["reason"].as_str() {
            Some("fyi") => format!("held (fyi) {msg_id}"),
            _ => format!("{outcome} {msg_id}"),
        };
        // A receipt's caution (a question sent as an FYI, say) rides along:
        // it never changes what happened, so it is never a refusal.
        return match data["warning"].as_str() {
            Some(warning) => format!("{receipt} (warning: {})", one_line(warning)),
            None => receipt,
        };
    }
    if let Some(path) = data["path"].as_str() {
        return format!("wrote {path}");
    }
    "ok".to_string()
}

fn one_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

fn code_prefix(reason: &str) -> Option<String> {
    let head = reason.split([':', ' ']).next()?;
    head.starts_with("E-RS-").then(|| head.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn argv(lines: &[Line]) -> Vec<Result<Vec<String>, String>> {
        lines
            .iter()
            .map(|line| line.parsed.clone().map(|(_, argv)| argv))
            .collect()
    }

    #[test]
    fn only_a_block_that_ends_the_text_runs() {
        let end = "Done.\n\n```pij\nsend pij-a \"hi there\"\noutput verdict --role r\n```\n  \n";
        let lines = block(end).expect("a trailing block");
        assert_eq!(
            argv(&lines),
            vec![
                Ok(vec!["send".into(), "pij-a".into(), "hi there".into()]),
                Ok(vec![
                    "output".into(),
                    "verdict".into(),
                    "--role".into(),
                    "r".into()
                ]),
            ]
        );
        assert!(block("Example:\n```pij\nsend a b\n```\nand then prose").is_none());
        assert!(block("no block at all").is_none());
        assert!(
            block("```pij\nsend a b\n").is_none(),
            "an unterminated fence runs nothing"
        );
        assert!(block("```pij\nsend a b\n```\n```js\nx\n```").is_none());
    }

    #[test]
    fn a_pij_fence_inside_another_fence_is_its_content() {
        // Review of #212: one ```text block whose content is a pij example.
        assert!(block("```text\n```pij\nsend peer hi\n```").is_none());
        // A longer fence quotes a whole pij block, closing fence and all.
        assert!(block("````md\n```pij\nsend peer hi\n```\n````").is_none());
        assert!(block("~~~\n```pij\nsend peer hi\n```\n~~~\n").is_none());
        // An outer fence left open swallows the rest of the text.
        assert!(block("````\n```pij\nsend peer hi\n```").is_none());
        // A closed quote before the real block leaves the real block to run.
        let text = "````md\n```pij\nsend a quoted\n```\n````\n```pij\nsend b real\n```";
        assert_eq!(
            argv(&block(text).expect("block")),
            vec![Ok(vec!["send".into(), "b".into(), "real".into()])]
        );
        // A longer pij fence holds a bare ``` line as a command line.
        assert!(block("````pij\nsend a b\n```\n````").is_some());
    }

    #[test]
    fn the_last_of_two_blocks_is_the_one_that_runs() {
        let text = "```pij\nsend a one\n```\ntext\n```pij\nsend c two\n```";
        let lines = block(text).expect("block");
        assert_eq!(
            argv(&lines),
            vec![Ok(vec!["send".into(), "c".into(), "two".into()])]
        );
    }

    #[test]
    fn indented_fences_blank_and_comment_lines_and_a_leading_pij() {
        let text = "x\n  ```pij\n  # why\n\n  pij send a b\n  ```";
        let lines = block(text).expect("block");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].line, 1);
        assert_eq!(
            argv(&lines),
            vec![Ok(vec!["send".into(), "a".into(), "b".into()])]
        );
    }

    #[test]
    fn quoting_is_shell_style_and_expands_nothing() {
        assert_eq!(
            tokenize(r#"send a "two words" 'it''s' \$HOME "\"q\" \$x \n""#),
            Ok(vec![
                "send".into(),
                "a".into(),
                "two words".into(),
                "its".into(),
                "$HOME".into(),
                "\"q\" $x \\n".into(),
            ])
        );
        assert_eq!(
            tokenize(r#"send a """#),
            Ok(vec!["send".into(), "a".into(), String::new()])
        );
        assert!(tokenize(r#"send a "open"#).is_err());
        assert!(tokenize("send a 'open").is_err());
        assert!(tokenize("send a \\").is_err());
    }

    #[test]
    fn verbs_are_send_output_and_requests_close_only() {
        let lines = block(
            "```pij\nrm -rf /\nrequests withdraw x --why y\nrequests close x\nsend --help\n```",
        )
        .expect("block");
        assert!(
            lines[0]
                .parsed
                .as_ref()
                .is_err_and(|why| why.contains("`rm`"))
        );
        assert!(
            lines[1]
                .parsed
                .as_ref()
                .is_err_and(|why| why.contains("requests close"))
        );
        assert_eq!(
            lines[2].parsed,
            Ok((
                Verb::RequestsClose,
                vec!["requests".into(), "close".into(), "x".into()]
            ))
        );
        assert!(lines[3].parsed.is_err(), "--help is not a command to run");
        assert!(
            block("```pij\nsend a 'unbalanced\n```").expect("block")[0]
                .parsed
                .is_err(),
            "an unbalanced quote is invalid"
        );
    }

    #[test]
    fn the_tail_check_matches_what_may_run() {
        assert!(may_end_with_block("a\n```pij\nsend a b\n```\n"));
        assert!(!may_end_with_block("a\n```js\nx\n```"));
        assert!(!may_end_with_block("```pij\nsend a b\n```\nmore"));
    }

    fn row(line: usize, verb: Verb, status: LineStatus, detail: &str) -> TurnEndLine {
        TurnEndLine {
            line,
            command: format!("{} x{line}", verb.as_str()),
            verb: Some(verb),
            status,
            detail: detail.to_string(),
            code: None,
            result: None,
        }
    }

    #[test]
    fn a_refusal_names_what_ran_so_a_retry_never_double_sends() {
        let result = TurnEndResult::from_lines(
            Outcome::Refused,
            vec![
                row(1, Verb::Send, LineStatus::Ran, "queued m1"),
                row(2, Verb::Send, LineStatus::Refused, "E-RS-COLD-WAKE: cold"),
                row(3, Verb::Output, LineStatus::NotRun, "not run"),
            ],
        );
        assert_eq!(result.ran, 1);
        assert_eq!(
            result.summary.as_deref(),
            Some("pij ✗ line 2 refused: \"E-RS-COLD-WAKE: cold\" (1 ran, 1 not run)")
        );
        let followup = result.followup.expect("a follow-up");
        assert!(followup.contains("Ran, do not resend:\n  1. \"send x1\"  ->  \"queued m1\""));
        assert!(
            followup.contains("Refused:\n  2. \"send x2\"  ->  \"E-RS-COLD-WAKE: cold\""),
            "{followup}"
        );
        assert!(followup.contains("Not run:\n  3. \"output x3\""));
    }

    #[test]
    fn echoed_command_and_reason_text_is_fenced() {
        // Review of #212 (S212-N001): a reason or command carrying a terminal
        // escape or a newline reaches neither the human nor the seat raw.
        let mut refused = row(
            1,
            Verb::Send,
            LineStatus::Refused,
            "E-X: \u{1b}[31mred\nforged",
        );
        refused.command = "send a \"\u{1b}]0;title\u{7}\"".to_string();
        let result = TurnEndResult::from_lines(Outcome::Refused, vec![refused]);
        let summary = result.summary.expect("summary");
        let followup = result.followup.expect("follow-up");
        for text in [&summary, &followup] {
            assert!(
                !text.contains('\u{1b}') && !text.contains('\u{7}'),
                "{text}"
            );
            assert!(!text.contains("\nforged"), "{text}");
        }
        assert!(summary.contains(r#""E-X: \u{1b}[31mred""#), "{summary}");
        assert!(followup.contains(r#"\u{1b}[31mred\nforged"#), "{followup}");
        let long = TurnEndResult::from_lines(
            Outcome::Refused,
            vec![row(1, Verb::Send, LineStatus::Refused, &"x".repeat(500))],
        );
        assert!(
            long.summary
                .expect("summary")
                .contains(&format!("\"{}…\"", "x".repeat(SUMMARY_REASON_CHARS)))
        );
    }

    #[test]
    fn success_and_invalid_summaries() {
        let ran = TurnEndResult::from_lines(
            Outcome::Ran,
            vec![
                row(1, Verb::Send, LineStatus::Ran, "queued m1"),
                row(2, Verb::Send, LineStatus::Ran, "queued m2"),
                row(3, Verb::RequestsClose, LineStatus::Ran, "closed"),
            ],
        );
        assert_eq!(ran.summary.as_deref(), Some("pij ✓ 2 sent, 1 closed"));
        assert_eq!(ran.followup, None);
        let invalid = TurnEndResult::from_lines(
            Outcome::Invalid,
            vec![
                row(1, Verb::Send, LineStatus::NotRun, "not run"),
                row(2, Verb::Send, LineStatus::Invalid, "unclosed \" quote"),
            ],
        );
        assert_eq!(invalid.ran, 0);
        assert_eq!(
            invalid.summary.as_deref(),
            Some("pij ✗ nothing ran: line 2 invalid: \"unclosed \\\" quote\"")
        );
        assert!(
            invalid
                .followup
                .expect("follow-up")
                .starts_with("[pij] Your end-of-turn pij block did not run: nothing was sent.")
        );
    }

    #[test]
    fn judge_reads_refusal_envelopes_and_refused_receipts() {
        assert_eq!(
            judge(
                false,
                None,
                Some("E-RS-COLD-WAKE: ❄ x is cold\n  --fyi …"),
                None,
                Some("refused")
            ),
            Err((
                "E-RS-COLD-WAKE: ❄ x is cold".to_string(),
                Some("E-RS-COLD-WAKE".to_string())
            ))
        );
        let declined =
            json!({"msg_id": "m", "outcome": {"outcome": "refused", "reason": "denied"}});
        assert_eq!(
            judge(true, Some(&declined), None, None, None),
            Err(("denied".to_string(), None))
        );
        let queued = json!({"msg_id": "m1", "outcome": {"outcome": "queued"}});
        assert_eq!(
            judge(true, Some(&queued), None, None, None),
            Ok("queued m1".to_string())
        );
        let held = json!({"msg_id": "m2", "outcome": {"outcome": "held", "reason": "fyi"}});
        assert_eq!(
            judge(true, Some(&held), None, None, None),
            Ok("held (fyi) m2".to_string())
        );
        let cautioned = json!({"msg_id": "m4", "outcome": {"outcome": "held", "reason": "fyi"}, "warning": "this looks like a question"});
        assert_eq!(
            judge(true, Some(&cautioned), None, None, None),
            Ok("held (fyi) m4 (warning: this looks like a question)".to_string())
        );
        let role = json!({"msg_id": "m3", "request": {"id": "m3", "state": "delivered", "holder": "pij-h"}});
        assert_eq!(
            judge(true, Some(&role), None, None, None),
            Ok("request m3 delivered to pij-h".to_string())
        );
        let output = json!({"path": "outputs/x.md"});
        assert_eq!(
            judge(true, Some(&output), None, None, None),
            Ok("wrote outputs/x.md".to_string())
        );
    }
}
