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

/// Plan 204: every `pij` command the community route and the role templates
/// print (in a fenced block or a backtick span) names a real verb, and each of
/// its `--flags` appears in pij-rs's help for that verb or leaf.
fn printed_commands(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut prose = String::new();
    for (index, chunk) in text.split("```").enumerate() {
        if index % 2 == 1 {
            lines.extend(chunk.lines().skip(1).map(str::to_string));
        } else {
            prose.push_str(chunk);
        }
    }
    lines.extend(
        prose
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|span| !span.contains('\n'))
            .map(str::to_string),
    );
    lines
        .iter()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("pij ")?;
            rest.starts_with(|c: char| c.is_ascii_lowercase())
                .then(|| line.split('#').next().unwrap_or(line).trim().to_string())
        })
        .collect()
}

fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_pij-rs"))
        .args(args)
        .arg("--help")
        .env(
            "PIJ_RS_STATE_DIR",
            std::env::temp_dir().join("pij-skill-no-daemon"),
        )
        .env("PIJ_RS_ADDR", "127.0.0.1:9")
        .output()
        .expect("run pij-rs --help");
    String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr)
}

/// Commands whose verb or flags pij-rs does not know, with why.
fn unknown_commands(commands: &[String], native: &BTreeSet<String>) -> Vec<String> {
    let mut wrong = Vec::new();
    for command in commands {
        let words: Vec<&str> = command.split_whitespace().collect();
        let verb = words[1];
        if !native.contains(verb) {
            wrong.push(format!("{command}: no verb `{verb}`"));
            continue;
        }
        let leaf = words
            .get(2)
            .filter(|word| word.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        let mut text = help(&[verb]);
        if let Some(leaf) = leaf {
            text.push_str(&help(&[verb, leaf]));
        }
        for flag in command
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter(|word| word.starts_with("--") && word.len() > 2)
        {
            if !text.contains(flag) && !["--help", "--json"].contains(&flag) {
                wrong.push(format!("{command}: pij-rs {verb} does not take {flag}"));
            }
        }
    }
    wrong
}

fn community_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut sources = vec![(
        "route".to_string(),
        std::fs::read_to_string(root.join("skills/pij/references/routes/community.md"))
            .expect("read the community route"),
    )];
    for entry in std::fs::read_dir(root.join("templates/roles")).expect("templates/roles") {
        let dir = entry.expect("a template").path();
        let Ok(versions) = std::fs::read_dir(&dir) else {
            continue;
        };
        for version in versions {
            let file = version.expect("a version").path().join("role.dd.json");
            if let Ok(json) = std::fs::read_to_string(&file) {
                let value: serde_json::Value = serde_json::from_str(&json).expect("template JSON");
                sources.push((
                    file.display().to_string(),
                    value.to_string().replace("\\n", "\n"),
                ));
            }
        }
    }
    sources
}

#[test]
fn the_community_route_and_templates_print_real_commands() {
    let native = native_verbs();
    let mut commands = Vec::new();
    for (_, text) in community_sources() {
        commands.extend(printed_commands(&text));
    }
    commands.sort();
    commands.dedup();
    assert!(
        commands
            .iter()
            .any(|c| c.starts_with("pij directory retire")),
        "the route prints too few commands: {commands:?}"
    );
    assert_eq!(unknown_commands(&commands, &native), Vec::<String>::new());
    // Negative controls: an unknown verb and an unknown flag are both caught.
    let bad = [
        "pij teleport now".to_string(),
        "pij directory retire x --frobnicate".to_string(),
    ];
    assert_eq!(unknown_commands(&bad, &native).len(), 2);
}
