//! Plan 175: the pij skill's verb tables cover every native `pij-rs` subcommand.
//!
//! The verb list is read from the built binary's own `--help` (the producer),
//! never restated here. The TypeScript twin
//! (`.omp/extensions/pij/core/generation-skill-coverage.test.ts`) covers the shim.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const COVERAGE_HEADINGS: [&str; 2] = ["## CLI-verb coverage", "## Native-only verbs"];

fn skill() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/pij/SKILL.md");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Subcommand names from clap's `Commands:` block, minus clap's own `help`.
fn native_verbs() -> BTreeSet<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .arg("--help")
        .output()
        .expect("run pij-rs --help");
    assert!(output.status.success(), "pij-rs --help failed");
    let help = String::from_utf8(output.stdout).expect("utf8 help");
    let commands = help
        .split_once("Commands:\n")
        .expect("pij-rs --help has a Commands: block")
        .1;
    let verbs: BTreeSet<String> = commands
        .lines()
        .take_while(|line| !line.trim().is_empty())
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|verb| *verb != "help")
        .map(str::to_owned)
        .collect();
    assert!(verbs.len() > 10, "parsed too few subcommands: {verbs:?}");
    verbs
}

/// Backticked top-level verbs in the first cell of each table row under `heading`.
/// Parenthesised leaves are annotations, as in the TypeScript parser.
fn table_verbs(markdown: &str, heading: &str) -> BTreeSet<String> {
    let Some(start) = markdown.lines().position(|line| line.starts_with(heading)) else {
        return BTreeSet::new();
    };
    let mut verbs = BTreeSet::new();
    for line in markdown
        .lines()
        .skip(start + 1)
        .take_while(|line| !line.starts_with("## "))
    {
        let Some(cell) = line
            .strip_prefix('|')
            .and_then(|rest| rest.split('|').next())
        else {
            continue;
        };
        let mut top_level = String::new();
        let mut depth = 0usize;
        for ch in cell.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => top_level.push(ch),
                _ => {}
            }
        }
        for (index, token) in top_level.split('`').enumerate() {
            let is_verb = index % 2 == 1
                && token.starts_with(|c: char| c.is_ascii_lowercase())
                && token
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if is_verb {
                verbs.insert(token.to_owned());
            }
        }
    }
    verbs
}

fn documented(markdown: &str) -> BTreeSet<String> {
    COVERAGE_HEADINGS
        .iter()
        .flat_map(|heading| table_verbs(markdown, heading))
        .collect()
}

fn missing(native: &BTreeSet<String>, markdown: &str) -> Vec<String> {
    let documented = documented(markdown);
    native.difference(&documented).cloned().collect()
}

#[test]
fn every_native_subcommand_has_a_skill_row() {
    let missing = missing(&native_verbs(), &skill());
    assert!(
        missing.is_empty(),
        "pij-rs subcommands missing from skills/pij/SKILL.md verb tables: {}",
        missing.join(", ")
    );
}

#[test]
fn native_only_rows_name_real_subcommands() {
    let native = native_verbs();
    let stale: Vec<String> = table_verbs(&skill(), COVERAGE_HEADINGS[1])
        .difference(&native)
        .cloned()
        .collect();
    assert!(
        stale.is_empty(),
        "SKILL.md native-only rows name verbs pij-rs does not have: {}",
        stale.join(", ")
    );
}

#[test]
fn removing_any_row_verb_is_detected() {
    let skill = skill();
    let native = native_verbs();
    for verb in &native {
        let removed = skill.replace(&format!("`{verb}`"), "`removed-verb`");
        assert_eq!(
            missing(&native, &removed),
            vec![verb.clone()],
            "deleting `{verb}` must be the only gap"
        );
    }
}

#[test]
fn parser_strips_leaf_annotations_and_stops_at_next_heading() {
    let example = [
        "## Native-only verbs",
        "| verb | use |",
        "|---|---|",
        "| `peers` (`check`/`new-key`) `ping` | `not-a-verb` |",
        "## Next",
        "| `outside` | ignored |",
    ]
    .join("\n");
    let verbs: Vec<String> = table_verbs(&example, COVERAGE_HEADINGS[1])
        .into_iter()
        .collect();
    assert_eq!(verbs, ["peers", "ping"]);
}
