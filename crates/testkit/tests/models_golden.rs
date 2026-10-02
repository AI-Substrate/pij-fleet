//! The CLI golden mechanism, proven against captured TS output (tk-717f, ac-0006).
//!
//! Prime ruling Q4: at wave-0 depth `pij-rs` has only `ping`/`health` and the TS
//! binary has neither, so there is no command both produce. What CAN be proven
//! now — and what wave 3's real parity tripwire depends on — is that the golden
//! MECHANISM works: it passes on unchanged bytes, fails on changed ones, and its
//! failure tells you which line moved.
//!
//! The subject is a COMPUTED summary of the captured catalog, not the catalog
//! itself. Goldening the fixture against itself would pass no matter how broken
//! the comparator was; goldening a derivation means a change in either the bytes
//! or the derivation shows up.
//!
//! It also freezes the ruling that came out of reading the real catalog: "no
//! thinking levels" arrives as `[]` OR as an absent key, never as `null`, and
//! both fold to one fact (services.dd amended, 2026-08-28).

use pij_testkit::{fixtures, golden};

/// How this row encodes "which thinking levels exist".
fn thinking_encoding(row: &serde_json::Value) -> &'static str {
    match row.get("levels") {
        None => "absent",
        Some(serde_json::Value::Null) => "null",
        Some(serde_json::Value::Array(levels)) if levels.is_empty() => "empty",
        Some(serde_json::Value::Array(_)) => "list",
        Some(_) => "not-a-list",
    }
}
/// The production fold: raw absent/empty/null all become no levels.
fn folded_levels(row: &serde_json::Value) -> String {
    match row.get("levels") {
        None | Some(serde_json::Value::Null) => "<none>".to_string(),
        Some(serde_json::Value::Array(levels)) => {
            let levels: Vec<&str> = levels
                .iter()
                .map(|level| level.as_str().expect("thinking level must be a string"))
                .collect();
            if levels.is_empty() {
                "<none>".to_string()
            } else {
                levels.join("/")
            }
        }
        Some(_) => panic!("levels must be an array, null, or absent"),
    }
}

/// The derivation under golden: one line per model, in the s106 order
/// (runtime · provider · selector · request id · thinking encoding).
fn summarise(catalog: &str) -> String {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(catalog).expect("the captured catalog is a JSON array");

    let mut lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let field = |name: &str| {
                row.get(name)
                    .and_then(serde_json::Value::as_str)
                    // ABSENT is rendered as its own token, never as "" — the
                    // distinction this whole corpus exists to preserve.
                    .unwrap_or("<absent>")
                    .to_string()
            };
            format!(
                "{}\t{}\t{}\t{}\t{}",
                field("runtime"),
                field("provider"),
                field("selector"),
                field("requestModelId"),
                folded_levels(row)
            )
        })
        .collect();
    lines.sort();
    format!("{}\n", lines.join("\n"))
}

#[test]
fn the_captured_catalog_matches_its_golden() {
    golden::assert_golden(
        "models-summary.tsv",
        &summarise(&fixtures::read("cli/models.json")),
    );
}

#[test]
fn no_thinking_levels_arrives_as_empty_or_absent_but_never_null() {
    // The contract u-models inherits, frozen here so wave 3 cannot quietly
    // reintroduce a third encoding.
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&fixtures::read("cli/models.json")).expect("catalog");
    let encodings: Vec<&str> = rows.iter().map(thinking_encoding).collect();

    assert!(
        encodings.contains(&"empty"),
        "the corpus carries `levels: []`"
    );
    assert!(
        encodings.contains(&"absent"),
        "and a row with no `levels` key"
    );
    assert!(
        !encodings.contains(&"null"),
        "no captured row uses null; if one appears, re-record the manifest answer \
         rather than letting a third encoding accumulate unnoticed"
    );
    assert!(!encodings.contains(&"not-a-list"));
}

#[test]
fn the_golden_mechanism_fails_when_a_line_changes_and_says_which() {
    // The negative proof. A comparator nobody has watched fail is a comparator
    // nobody knows works — the same argument as the drift gate's own fixture.
    let expected = "alpha\nbeta\ngamma\n";
    let actual = "alpha\nBETA\ngamma\n";

    let message = golden::render_diff("fixtures/golden/example", expected, actual);

    assert!(message.contains("first difference at line 2"), "{message}");
    assert!(
        message.contains("- beta") && message.contains("+ BETA"),
        "{message}"
    );
    assert!(
        message.contains("UPDATE_GOLDENS=1"),
        "the failure must name the deliberate way to re-record: {message}"
    );
    assert!(
        message.contains("you have found the regression this golden exists for"),
        "...and must not imply re-recording is the default response: {message}"
    );
}

#[test]
fn the_golden_mechanism_reports_a_length_change_too() {
    // A truncated payload is the parity failure that a naive line-by-line
    // comparison misses entirely: every compared line matches, and the output is
    // still wrong.
    let message = golden::render_diff("fixtures/golden/example", "a\nb\nc\n", "a\nb\n");
    assert!(message.contains("expected 3 line(s), got 2"), "{message}");
    assert!(message.contains("+ <missing>"), "{message}");
}
