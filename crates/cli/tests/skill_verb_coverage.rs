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
/// print (in a fenced block or a backtick span).
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
                .then(|| comment_free(line).trim().to_string())
        })
        .collect()
}

/// `line` up to a `#` comment that is outside single quotes.
fn comment_free(line: &str) -> &str {
    let mut quoted = false;
    for (at, c) in line.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '#' if !quoted => return &line[..at],
            _ => {}
        }
    }
    line
}

/// The command's words, with single-quoted spans kept whole and their
/// quotes kept, so a quoted value is never read as a flag.
fn words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in command.chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if c.is_whitespace() && !quoted {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Exact `--flag` tokens in `text`.
fn flag_tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|token| {
            token.len() > 2 && token.starts_with("--") && token.as_bytes()[2].is_ascii_lowercase()
        })
        .map(str::to_string)
        .collect()
}

/// The command's own `--flags`, outside single quotes and option brackets.
fn command_flags(words: &[String]) -> Vec<String> {
    words
        .iter()
        .filter(|word| !word.starts_with('\''))
        .map(|word| word.trim_start_matches('[').trim_end_matches(']'))
        .filter(|word| word.starts_with("--"))
        .map(|word| word.split('=').next().unwrap_or(word).to_string())
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

/// The help that is the contract for this command: its leaf's own help when
/// the leaf has one, else the verb's (review of #136, F001: never the union).
fn contract(words: &[String]) -> String {
    let verb_help = help(&[&words[1]]);
    match words.get(2) {
        Some(leaf) if leaf.chars().all(|c| c.is_ascii_lowercase() || c == '-') => {
            let leaf_help = help(&[&words[1], leaf]);
            if leaf_help == verb_help || leaf_help.contains("FAILED") {
                verb_help
            } else {
                leaf_help
            }
        }
        _ => verb_help,
    }
}

/// The leading literal words: up to the first placeholder, flag or option.
fn literal_prefix(words: &[String]) -> Vec<String> {
    words
        .iter()
        .take_while(|word| !word.starts_with(['<', '-', '[', '(', '\'', '"', '|']))
        .cloned()
        .collect()
}

/// Groups of flags a usage line requires (outside brackets); a group is
/// satisfied by any of its alternatives (`--body <text>|--body-file <path>`).
fn required_groups(usage: &[String]) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut depth = 0i32;
    for word in usage {
        let opens = word.matches(['[', '(']).count() as i32;
        let closes = word.matches([']', ')']).count() as i32;
        if depth == 0 && opens == 0 {
            if word.starts_with("--") {
                groups.push(vec![word.split('=').next().unwrap_or(word).to_string()]);
            } else if let Some((_, alternative)) = word.split_once("|--")
                && let Some(group) = groups.last_mut()
            {
                group.push(format!("--{alternative}"));
            }
        }
        depth += opens - closes;
    }
    groups
}

/// The lines of each `Usage:` block in `help`, up to the blank line after it.
fn usage_block(help: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in help.lines() {
        let trimmed = line.trim();
        if trimmed == "Usage:" {
            inside = true;
        } else if trimmed.is_empty() {
            inside = false;
        } else if inside {
            lines.push(trimmed);
        }
    }
    lines
}

/// Whether some usage line for this command in `contract` has every one of
/// its required flags present. Usage lines are the `Usage:` block's
/// `pij <verb> ...` lines whose literal words are the longest prefix of the command's.
fn requirements_met(words: &[String], contract: &str, flags: &[String]) -> bool {
    let prefix = literal_prefix(words);
    let usages: Vec<Vec<String>> = usage_block(contract)
        .into_iter()
        .filter(|line| line.starts_with("pij "))
        .map(|line| self::words(line.split("   ").next().unwrap_or(line)))
        .filter(|usage| {
            let literal = literal_prefix(usage);
            prefix.starts_with(&literal)
        })
        .collect();
    let longest = usages
        .iter()
        .map(|usage| literal_prefix(usage).len())
        .max()
        .unwrap_or(0);
    let applicable: Vec<&Vec<String>> = usages
        .iter()
        .filter(|usage| literal_prefix(usage).len() == longest)
        .collect();
    applicable.is_empty()
        || applicable.iter().any(|usage| {
            required_groups(usage)
                .iter()
                .all(|group| group.iter().any(|flag| flags.contains(flag)))
        })
}

/// Each `<placeholder>` outside single quotes (review of #136, S136001).
fn unquoted_placeholders(command: &str) -> bool {
    let mut quoted = false;
    command.chars().any(|c| {
        if c == '\'' {
            quoted = !quoted;
        }
        c == '<' && !quoted
    })
}

/// Commands that name no pij-rs verb, use a flag their leaf does not take,
/// lack a flag their leaf requires, or leave a placeholder unquoted.
fn command_problems(commands: &[String], native: &BTreeSet<String>) -> Vec<String> {
    let mut wrong = Vec::new();
    for command in commands {
        let words = words(command);
        let verb = words[1].as_str();
        if !native.contains(verb) {
            wrong.push(format!("{command}: no verb `{verb}`"));
            continue;
        }
        if unquoted_placeholders(command) {
            wrong.push(format!("{command}: a placeholder outside single quotes"));
        }
        let contract = contract(&words);
        let allowed = flag_tokens(&contract);
        let flags = command_flags(&words);
        for flag in &flags {
            if !allowed.contains(flag) && !["--help", "--json"].contains(&flag.as_str()) {
                wrong.push(format!("{command}: its leaf does not take {flag}"));
            }
        }
        // A bare `pij <verb> <leaf>` names the verb; it is not a call.
        let bare = words.len() <= 3 && flags.is_empty();
        if !bare && !requirements_met(&words, &contract, &flags) {
            wrong.push(format!("{command}: a required flag is missing"));
        }
    }
    wrong
}

/// The route, and the latest version of each role template.
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
        let latest = versions
            .filter_map(|version| {
                let path = version.ok()?.path();
                let number: u32 = path
                    .file_name()?
                    .to_str()?
                    .strip_prefix('v')?
                    .parse()
                    .ok()?;
                Some((number, path))
            })
            .max_by_key(|(number, _)| *number);
        if let Some((_, path)) = latest {
            let file = path.join("role.dd.json");
            let json = std::fs::read_to_string(&file).expect("read a template");
            let value: serde_json::Value = serde_json::from_str(&json).expect("template JSON");
            let text = value.to_string().replace("\\n", "\n").replace("\\\"", "\"");
            sources.push((file.display().to_string(), text));
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
    assert_eq!(command_problems(&commands, &native), Vec::<String>::new());
}

/// Negative controls: each kind of mistake is caught, one at a time.
#[test]
fn every_kind_of_bad_printed_command_is_caught() {
    let native = native_verbs();
    for (bad, why) in [
        ("pij teleport now", "no verb"),
        (
            "pij directory retire x --frobnicate --why 'y'",
            "an unknown flag",
        ),
        // Review of #136, F001: a prefix of a real flag, and a real flag of
        // another leaf.
        ("pij directory retire x --wh 'y' --why 'z'", "a flag prefix"),
        ("pij directory templates --why 'y'", "another leaf's flag"),
        // F002: a write without its required --why.
        (
            "pij directory add pm-001 --from template:pm --instance-key 001",
            "no --why",
        ),
        ("pij directory retire x", "no --why"),
        // S136001: placeholders outside single quotes.
        (
            "pij send <seat> --body-file '<path>'",
            "an unquoted placeholder",
        ),
        (
            "pij report now \"<did>\" '<next>'",
            "a double-quoted placeholder",
        ),
    ] {
        assert_eq!(
            command_problems(&[bad.to_string()], &native).len(),
            1,
            "{why}: {bad}"
        );
    }
    let good = [
        "pij directory add pm-001 --from template:pm --instance-key 001 --why 'workstream 001 needs a PM'".to_string(),
        "pij send '<seat>' --body-file '<path>'".to_string(),
        "pij directory templates [--show '<name>[@vN]']".to_string(),
        "pij directory assign".to_string(),
    ];
    assert_eq!(command_problems(&good, &native), Vec::<String>::new());
}
