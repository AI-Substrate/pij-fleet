//! The `report` family, served by rs (plan 114, u-report; ac-1142 + ac-1144).
//!
//! ONE endpoint serves the whole family, because the shim routes by verb and
//! hands the daemon the caller's `argv` untouched. So the argument shapes are
//! parsed HERE, once, and the rs CLI forwards argv rather than re-parsing it —
//! two parsers for one grammar is how the generations would drift.
//!
//! # Where the shapes come from
//!
//! Every rule below was derived from the TYPESCRIPT SOURCE callers actually hit,
//! not from a description of it, and each carries the line it came from. The
//! packet that commissioned this work said so in as many words, and it was
//! right to: a packet is a description, the source is the thing.
//!
//! # Two rules that shape every refusal here
//!
//! 1. **A shape rs cannot honour refuses BY NAME.** State declarations can
//!    carry a task assignment owned by the caller and supporting references.
//!    Verify checks task-scoped done evidence under current-parent authority.
//!    Unsupported scoping flags never disappear into a near-fit behavior.
//! 2. **No refusal may be a bare 404.** Wave 1's shim classifies "404 or 405
//!    with a body that does not decode as a pij envelope" as ROUTE ABSENCE and
//!    falls back to legacy. A refusal wearing that shape would not be a refusal
//!    at all — it would silently re-home the seat into the other store while
//!    every surface reported success. Everything here answers 400 with an
//!    envelope, and `report_http.rs` pins it by replaying the shim's own
//!    classification over every refusal this module can emit.

use pij_core::model::{CARD_LIMIT, NOTE_LIMIT, SemanticState};

/// One parsed report invocation, including sequence-bound parent verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReportCall {
    /// `report now "<did>" "<next>" [--state <word>] [--note <text>]`.
    ///
    /// `state` rides along because TypeScript performs both writes under one
    /// lock (`core/cli.ts:1631`), so the two are one caller-visible act.
    Now {
        did: String,
        next: String,
        state: Option<SemanticState>,
        note: Option<String>,
    },
    /// A state declaration, including the explaining blocked leaf.
    State {
        state: SemanticState,
        note: Option<String>,
        assignment_id: Option<String>,
        refs: Vec<String>,
    },
    /// A durable question plus its state declaration.
    Question {
        note: String,
        assignment_id: Option<String>,
        refs: Vec<String>,
    },
    /// Parent verifies an actual done event, optionally for its named task.
    Verify {
        target: String,
        assignment: Option<String>,
    },
    /// `report clear [--assignment <id>]`; no scope clears only the descriptor.
    Clear { assignment_id: Option<String> },
}

/// A refusal, in the caller's own vocabulary.
///
/// It is a plain string because it is destined for `Envelope::refused`'s `meta`,
/// which is prose for a human; the machine-readable half is the `ErrorKind` the
/// handler attaches. Splitting them is the existing convention here
/// (`http/mod.rs`, `refused`).
pub(crate) type Refusal = String;

/// One `--flag` with the value it took, if any. `None` is a valueless flag,
/// which here is only ever `--json`.
type Flag<'a> = (&'a str, Option<&'a str>);

/// The tokens after the leaf, sorted into the two kinds a caller can type.
struct Arguments<'a> {
    positionals: Vec<&'a str>,
    flags: Vec<Flag<'a>>,
}

/// Parse the `argv` the caller typed into one report invocation.
///
/// `argv` arrives as `process.argv.slice(2)` verbatim, so it still carries the
/// verb: `["report", "now", "<did>", "<next>"]`.
pub(crate) fn parse_report(argv: &[String]) -> Result<ReportCall, Refusal> {
    let mut tokens = argv.iter().map(String::as_str);
    match tokens.next() {
        Some("report") => {}
        Some(other) => return Err(format!("'{other}' is not the report verb")),
        None => {
            return Err("usage: pij report <now|state|blocked|question|clear|verify>".to_string());
        }
    }
    let leaf = tokens
        .next()
        .ok_or_else(|| "usage: pij report <now|state|blocked|question|clear|verify>".to_string())?;

    let Arguments { positionals, flags } = split_arguments(tokens)?;

    // These scoping operations are unavailable on every report leaf.
    for (name, _) in &flags {
        if let Some(why) = unsupported_flag(name) {
            return Err(why);
        }
    }

    match leaf {
        "now" => parse_now(&positionals, &flags),
        "state" => parse_state_leaf(&positionals, &flags),
        "blocked" | "question" => parse_note_leaf(leaf, &positionals, &flags),
        "clear" => {
            expect_positionals(
                "report clear",
                &positionals,
                0,
                "usage: pij report clear [--assignment <id>]",
            )?;
            reject_unknown_flags("report clear", &flags, &["assignment"])?;
            Ok(ReportCall::Clear {
                assignment_id: single_value(&flags, "assignment")?.map(str::to_string),
            })
        }
        "verify" => {
            expect_positionals(
                "report verify",
                &positionals,
                1,
                "usage: pij report verify <seat> [--assignment <id>]",
            )?;
            reject_unknown_flags("report verify", &flags, &["assignment"])?;
            let assignment = single_value(&flags, "assignment")?;
            if positionals[0].trim().is_empty() {
                return Err("report verify requires a seat".into());
            }
            Ok(ReportCall::Verify {
                target: positionals[0].to_string(),
                assignment: assignment.map(str::to_string),
            })
        }
        other => Err(format!(
            "unknown report subcommand '{other}' (now|state|blocked|question|clear|verify)"
        )),
    }
}

/// Unsupported scopes refuse honestly, without suggesting a legacy fallback.
fn unsupported_flag(name: &str) -> Option<Refusal> {
    let reason = match name {
        "project" => {
            "rs reports are node-scoped; project comes from the node (see `pij node show`)"
        }
        "for" => "rs has no relay authority yet; report as yourself",
        _ => return None,
    };
    Some(format!("--{name}: {reason}"))
}

/// Split the remaining tokens into positionals and `--flag [value]` pairs.
///
/// `--json` is accepted and carries no value: every rs answer is already an
/// envelope, so the flag is satisfied by construction. Refusing it would break
/// the many callers that pass it habitually.
fn split_arguments<'a>(tokens: impl Iterator<Item = &'a str>) -> Result<Arguments<'a>, Refusal> {
    let tokens: Vec<&str> = tokens.collect();
    let mut positionals = Vec::new();
    let mut flags: Vec<Flag<'a>> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if let Some(name) = token.strip_prefix("--") {
            let (name, inline) = match name.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (name, None),
            };
            if name.is_empty() {
                return Err("'--' is not a flag".to_string());
            }
            let value = match inline {
                Some(value) => Some(value),
                // A valueless flag: only `--json` is one here, so anything else
                // consumes the next token as its value, exactly as the
                // TypeScript lexer does for its non-boolean flags.
                None if name == "json" => None,
                None => {
                    index += 1;
                    tokens.get(index).copied()
                }
            };
            flags.push((name, value));
        } else {
            positionals.push(token);
        }
        index += 1;
    }
    Ok(Arguments { positionals, flags })
}

/// `report now "<did>" "<next>" [--state <word>] [--note <text>] [--json]`
/// — `.pi/extensions/pij/core/cli.ts:1631-1687`.
fn parse_now(positionals: &[&str], flags: &[Flag<'_>]) -> Result<ReportCall, Refusal> {
    reject_unknown_flags("report now", flags, &["state", "note"])?;
    expect_positionals(
        "report now",
        positionals,
        2,
        r#"usage: pij report now "<did>" "<next>" [--state <word>]"#,
    )?;
    let did = normalize_card_field("did", positionals[0])?;
    let next = normalize_card_field("next", positionals[1])?;

    let state = match flag_value(flags, "state")? {
        // `--state` with no value is its own message in TypeScript
        // (`core/cli.ts:1645`), and the wording is kept.
        Some("") | None if flags.iter().any(|(name, _)| *name == "state") => {
            return Err("--state takes a semantic state".to_string());
        }
        Some(word) => Some(parse_semantic_state(word)?),
        None => None,
    };

    let note = match flag_value(flags, "note")? {
        Some(text) => {
            // `--note` is permitted ONLY with a state that carries an
            // explanation — `core/cli.ts:1653`. Without that guard a note on
            // `--state done` would be accepted and then have nowhere to live.
            if !matches!(
                state,
                Some(SemanticState::Question) | Some(SemanticState::Blocked)
            ) {
                return Err(
                    "--note is permitted only with --state question or --state blocked".to_string(),
                );
            }
            Some(normalize_note(text)?)
        }
        None if flags.iter().any(|(name, _)| *name == "note") => {
            return Err("--note takes text".to_string());
        }
        None => None,
    };

    Ok(ReportCall::Now {
        did,
        next,
        state,
        note,
    })
}

/// `report state <state> [--json]` — `.pi/extensions/pij/core/cli.ts:1708-1731`.
fn parse_state_leaf(positionals: &[&str], flags: &[Flag<'_>]) -> Result<ReportCall, Refusal> {
    reject_unknown_flags("report state", flags, &["assignment", "refs"])?;
    expect_positionals(
        "report state",
        positionals,
        1,
        "usage: pij report state <state>",
    )?;
    Ok(ReportCall::State {
        state: parse_semantic_state(positionals[0])?,
        note: None,
        assignment_id: single_value(flags, "assignment")?.map(str::to_string),
        refs: super::governance::csv(single_value(flags, "refs")?),
    })
}

/// `report blocked|question "<text>" [--json]` —
/// `.pi/extensions/pij/core/cli.ts:1689-1707`.
fn parse_note_leaf(
    leaf: &str,
    positionals: &[&str],
    flags: &[Flag<'_>],
) -> Result<ReportCall, Refusal> {
    reject_unknown_flags(&format!("report {leaf}"), flags, &["assignment", "refs"])?;
    expect_positionals(
        &format!("report {leaf}"),
        positionals,
        1,
        &format!(r#"usage: pij report {leaf} "<text>""#),
    )?;
    let note = normalize_note(positionals[0])?;
    let assignment_id = single_value(flags, "assignment")?.map(str::to_string);
    let refs = super::governance::csv(single_value(flags, "refs")?);
    Ok(if leaf == "blocked" {
        ReportCall::State {
            state: SemanticState::Blocked,
            note: Some(note),
            assignment_id,
            refs,
        }
    } else {
        ReportCall::Question {
            note,
            assignment_id,
            refs,
        }
    })
}

/// Map a typed word onto rs's [`SemanticState`].
fn parse_semantic_state(word: &str) -> Result<SemanticState, Refusal> {
    SemanticState::parse(word)
        .ok_or_else(|| format!("invalid semantic state '{word}' ({})", SemanticState::WORDS))
}

/// The card-field rules the TypeScript generation enforces —
/// `normalizeReportText`, `.pi/extensions/pij/core/cli.ts:817-830`.
///
/// Two of these three rules exist ONLY here, and deliberately so.
/// `pij_core::report` documents that an empty card is valid ("a present empty
/// card and no card are different facts") and collapses newlines silently. Both
/// are right for an in-process caller and neither matches what the CLI's callers
/// hit today, so the divergence is settled at this BOUNDARY rather than by
/// overruling core.
///
/// The length rule is NOT re-implemented here: it is [`CARD_LIMIT`], cited, and
/// core checks it again on the way through. One constant, one owner.
fn normalize_card_field(label: &str, input: &str) -> Result<String, Refusal> {
    if input.contains(['\r', '\n', '\u{2028}', '\u{2029}']) {
        return Err(format!("report {label} must be one line"));
    }
    let normalized = collapse_whitespace(input);
    if normalized.is_empty() {
        return Err(format!("report {label} must not be empty"));
    }
    if normalized.chars().count() > CARD_LIMIT {
        return Err(format!(
            "report {label} exceeds the {CARD_LIMIT}-character limit after whitespace collapsing"
        ));
    }
    Ok(normalized)
}

/// The note rules — `normalizeReportNote`, `.pi/extensions/pij/core/cli.ts:832-845`.
///
/// [`NOTE_LIMIT`] is 200 and is NOT [`CARD_LIMIT`]. rs core applies no limit to a
/// note at all, so unlike the card rules there is nothing downstream to catch a
/// miss here.
fn normalize_note(input: &str) -> Result<String, Refusal> {
    if input.contains(['\r', '\n', '\u{2028}', '\u{2029}']) {
        return Err("report note must be one line".to_string());
    }
    let normalized = collapse_whitespace(input);
    if normalized.is_empty() {
        return Err("report note must not be empty".to_string());
    }
    if normalized.chars().count() > NOTE_LIMIT {
        return Err(format!(
            "report note exceeds the {NOTE_LIMIT}-character limit after whitespace collapsing"
        ));
    }
    Ok(normalized)
}

/// `input.trim().replace(/\s+/g, " ")` — `.pi/extensions/pij/core/cli.ts:821`.
fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn flag_value<'a>(flags: &[Flag<'a>], name: &str) -> Result<Option<&'a str>, Refusal> {
    Ok(flags
        .iter()
        .find(|(flag, _)| *flag == name)
        .and_then(|(_, value)| *value))
}

/// Assignment metadata flags take one nonempty value, never another flag.
fn single_value<'a>(flags: &[Flag<'a>], name: &str) -> Result<Option<&'a str>, Refusal> {
    let mut matches = flags.iter().filter(|(flag, _)| *flag == name);
    let Some((_, value)) = matches.next() else {
        return Ok(None);
    };
    match value {
        Some(value)
            if !value.trim().is_empty() && !value.starts_with("--") && matches.next().is_none() =>
        {
            Ok(Some(value))
        }
        _ => Err(format!("--{name} requires one nonempty value")),
    }
}

/// TypeScript refuses an unrecognised flag per verb (`ALLOWED_FLAGS`,
/// `core/cli.ts:887-892`) rather than ignoring it, and so does this.
fn reject_unknown_flags(key: &str, flags: &[Flag<'_>], allowed: &[&str]) -> Result<(), Refusal> {
    for (name, _) in flags {
        if *name == "json" || allowed.contains(name) {
            continue;
        }
        return Err(format!("unknown flag --{name} for `pij {key}`"));
    }
    Ok(())
}

/// TypeScript caps positionals per verb (`MAX_POS`, `core/cli.ts:932-937`) and
/// requires the ones the usage line names.
fn expect_positionals(
    key: &str,
    positionals: &[&str],
    expected: usize,
    usage: &str,
) -> Result<(), Refusal> {
    if positionals.len() == expected {
        return Ok(());
    }
    if positionals.len() < expected {
        return Err(usage.to_string());
    }
    Err(format!(
        "too many arguments for `pij {key}` (expected {expected}, got {}) — {usage}",
        positionals.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn now_carries_did_next_and_an_optional_state() {
        assert_eq!(
            parse_report(&argv(&["report", "now", " a  b ", "c"])),
            Ok(ReportCall::Now {
                did: "a b".to_string(),
                next: "c".to_string(),
                state: None,
                note: None,
            })
        );
        assert_eq!(
            parse_report(&argv(&[
                "report", "now", "a", "b", "--state", "blocked", "--note", "why"
            ])),
            Ok(ReportCall::Now {
                did: "a".to_string(),
                next: "b".to_string(),
                state: Some(SemanticState::Blocked),
                note: Some("why".to_string()),
            })
        );
    }

    /// `--json` is habitual in this fleet and must never be the reason a report
    /// fails: every rs answer is an envelope already.
    #[test]
    fn json_is_accepted_everywhere_and_consumes_no_positional() {
        assert_eq!(
            parse_report(&argv(&["report", "now", "a", "b", "--json"])),
            Ok(ReportCall::Now {
                did: "a".to_string(),
                next: "b".to_string(),
                state: None,
                note: None,
            })
        );
        assert_eq!(
            parse_report(&argv(&["report", "clear", "--json"])),
            Ok(ReportCall::Clear {
                assignment_id: None
            })
        );
    }

    #[test]
    fn a_note_without_an_explaining_state_is_refused() {
        let refusal = parse_report(&argv(&[
            "report", "now", "a", "b", "--state", "done", "--note", "why",
        ]))
        .expect_err("a note has nowhere to live on --state done");
        assert!(refusal.contains("only with --state question or --state blocked"));
    }

    #[test]
    fn report_state_and_now_accept_failed_and_cancelled_without_substitution() {
        for word in ["failed", "cancelled"] {
            let ReportCall::State { state, .. } =
                parse_report(&argv(&["report", "state", word])).expect("terminal state")
            else {
                panic!("expected a state declaration");
            };
            assert_eq!(serde_json::to_value(state).unwrap(), word);

            let ReportCall::Now { state, .. } =
                parse_report(&argv(&["report", "now", "a", "b", "--state", word]))
                    .expect("card with terminal state")
            else {
                panic!("expected a card declaration");
            };
            assert_eq!(serde_json::to_value(state).unwrap(), word);
        }
    }

    #[test]
    fn reporting_assignment_flags_are_accepted_only_on_state_leaves() {
        for (leaf, value) in [
            ("state", "waiting"),
            ("blocked", "why"),
            ("question", "which?"),
        ] {
            assert!(
                parse_report(&argv(&[
                    "report",
                    leaf,
                    value,
                    "--assignment",
                    "task-1",
                    "--refs",
                    "a,b"
                ]))
                .is_ok()
            );
            assert!(parse_report(&argv(&["report", leaf, value, "--refs", "a,b"])).is_ok());
            for flags in [
                vec!["--assignment"],
                vec!["--assignment", " "],
                vec!["--assignment", "a", "--assignment", "b"],
                vec!["--assignment", "--refs", "a"],
                vec!["--refs"],
                vec!["--refs", "a", "--refs", "b"],
            ] {
                let mut tokens = vec!["report", leaf, value];
                tokens.extend(flags);
                assert!(parse_report(&argv(&tokens)).is_err(), "{tokens:?}");
            }
        }
        for tokens in [
            vec!["report", "now", "a", "b", "--assignment", "task-1"],
            vec!["report", "clear", "--refs", "a"],
            vec!["report", "verify", "worker", "--refs", "a"],
            vec!["report", "now", "a", "b", "--project", "slug"],
            vec!["report", "now", "a", "b", "--for", "other"],
        ] {
            assert!(parse_report(&argv(&tokens)).is_err(), "{tokens:?}");
        }
    }

    /// The three card rules that exist only at this boundary, plus the one that
    /// does not: the length rule cites [`CARD_LIMIT`] rather than a literal.
    #[test]
    fn card_fields_follow_the_typescript_rules() {
        assert!(
            parse_report(&argv(&["report", "now", "a\nb", "c"]))
                .expect_err("newline")
                .contains("must be one line")
        );
        assert!(
            parse_report(&argv(&["report", "now", "   ", "c"]))
                .expect_err("empty")
                .contains("must not be empty")
        );
        let over = "a".repeat(CARD_LIMIT + 1);
        assert!(
            parse_report(&argv(&["report", "now", &over, "c"]))
                .expect_err("too long")
                .contains(&CARD_LIMIT.to_string())
        );
        let at = "a".repeat(CARD_LIMIT);
        assert!(parse_report(&argv(&["report", "now", &at, "c"])).is_ok());
    }

    /// Pins that the note limit is its own number. A handler that reused
    /// `CARD_LIMIT` here would pass every other test in this module.
    #[test]
    fn the_note_limit_is_not_the_card_limit() {
        const { assert!(NOTE_LIMIT < CARD_LIMIT) };
        let note = "n".repeat(NOTE_LIMIT + 1);
        assert!(
            parse_report(&argv(&["report", "blocked", &note]))
                .expect_err("over the note limit")
                .contains(&NOTE_LIMIT.to_string())
        );
        assert!(parse_report(&argv(&["report", "blocked", &"n".repeat(NOTE_LIMIT)])).is_ok());
    }

    #[test]
    fn arity_follows_the_typescript_table() {
        assert!(parse_report(&argv(&["report", "now", "only-did"])).is_err());
        assert!(parse_report(&argv(&["report", "now", "a", "b", "c"])).is_err());
        assert!(parse_report(&argv(&["report", "clear", "extra"])).is_err());
        assert!(parse_report(&argv(&["report", "state"])).is_err());
    }

    #[test]
    fn verify_accepts_seat_only_and_explicit_assignment_but_not_ignored_filters() {
        assert_eq!(
            parse_report(&argv(&["report", "verify", "worker"])),
            Ok(ReportCall::Verify {
                target: "worker".into(),
                assignment: None
            })
        );
        assert_eq!(
            parse_report(&argv(&[
                "report",
                "verify",
                "worker",
                "--assignment",
                "task-1",
                "--json"
            ])),
            Ok(ReportCall::Verify {
                target: "worker".into(),
                assignment: Some("task-1".into())
            })
        );
        for tokens in [
            vec!["report", "verify", "worker", "--assignment"],
            vec!["report", "verify", "worker", "--project", "p"],
            vec![
                "report",
                "verify",
                "worker",
                "--assignment",
                "a",
                "--assignment",
                "b",
            ],
        ] {
            assert!(parse_report(&argv(&tokens)).is_err());
        }
    }
}
