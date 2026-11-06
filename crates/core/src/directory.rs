//! `pij directory` help (plan 195, AC10). Pure text, shared by the native CLI,
//! which prints it without a daemon, and the daemon, which answers shim
//! callers, so the two cannot drift.

/// The help for `pij directory [<verb>] --help`; `None` for an unknown verb.
pub fn help(verb: Option<&str>) -> Option<&'static str> {
    match verb {
        None => Some(OVERVIEW),
        Some("add") => Some(ADD),
        Some("assign" | "release" | "lineage") => Some(HOLD),
        Some("external") => Some(EXTERNAL),
        Some("rebuild") => Some(REBUILD),
        Some("templates" | "template-diff") => Some(TEMPLATES),
        Some("memory") => Some(MEMORY),
        _ => None,
    }
}

/// Whether argv (after `directory`) asks for help.
pub fn wants_help(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
}

/// `word` as one POSIX shell word, for commands pij prints to be run as they
/// are (plan 194, review F001): bare when every character is plainly safe,
/// else single-quoted with each `'` written as `'\''`.
pub fn shell_word(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-./:@%+=,".contains(ch));
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The verb argv names: its first token that is not a flag.
pub fn verb(args: &[String]) -> Option<&str> {
    args.first()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
}

const OVERVIEW: &str = "\
pij directory: the project's community directory, on the governance branch.

Usage:
  pij directory [--project <slug>] [--json]           this project's roles and holders
  pij directory add <id> --why <text> [role flags | --file <json>] [--project <slug>]
  pij directory add <id> --from template:<name>[@v<n>] --why <text> [role flags]
  pij directory add --batch <dir> --why <text> [--project <slug>]
  pij directory templates                             the role templates and versions
  pij directory template-diff <id>                    what changed in its template since
  pij directory assign <role>[/<instance>] <seat> --why <text>
  pij directory assign --batch <file> --why <text>      many holdings, one commit
  pij directory release <role>[/<instance>] <seat> --why <text>
  pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text>
  pij directory external add <seat> --from-project <slug> --why <note> [--role <id>]
  pij directory rebuild                               the store from the branch head
  pij directory memory add <role> --title <t> --hook <h> [--tags <a,b>] --body <text>|--body-file <path|->
  pij directory memory <role> [--note <slug>]         a role's memory INDEX, or one note

Pij Emergent Communities (PEC): the prime and the humans build whatever
community the work needs now, and reshape it as the work changes. Templates are
examples, not a checklist; the view prompts for missing core roles and security
and never creates one.

Every write is validated by ddocs and committed by the daemon's single writer,
touching only its own paths in the standing governance worktree.

Run `pij directory <verb> --help` for a verb's flags.";

const ADD: &str = "\
pij directory add: write a role definition (a pij/role document).

Usage:
  pij directory add <id> --why <text> [flags]
  pij directory add <id> --why <text> --file <role.dd.json>
  pij directory add <id> --from template:<name>[@v<n>] --why <text> [flags]
  pij directory add --batch <dir> --why <text>

Required, unless --file or --from template supplies them: --why, --title,
--kind, --lifecycle, --holders, at least one --rule, and one of --packet or
--brief. The shortest add:
  pij directory add qa --why \"a QA role\" --title QA --kind specialist \\
    --lifecycle long-lived --holders one --rule \"A verdict on every PR.\" \\
    --packet \"Review each PR at its head.\"

Each add is its own commit: a chain of adds that stops part-way leaves the
earlier roles committed. --batch <dir> is all or nothing.

Flags:
  --why <text>              required; becomes the changelog row
  --from template:<name>[@v<n>]   copy a template (latest by default); flags
                            then override its fields, and meta.template and
                            meta.template_version record where it came from
  --from file:<brief>       the same as --brief <brief>
  --project <slug>          default: the project whose repo holds this directory
  --title <text>
  --kind specialist|general
  --lifecycle long-lived|per-task
  --holders one|many
  --instance-key <text>
  --default-limit <duration>   like 45m or 1h30m
  --summary <text>
  --template <name>  --template-version <v>
  --rule <text>             repeatable; scope.rules
  --scope-path <glob>       repeatable; scope.paths
  --out-of-scope <text>     repeatable
  --forbidden <glob>        repeatable
  --input <text>            repeatable
  --output-kind verdict|deliverable|note|handover   repeatable
  --harness <h>  --model <m>  --effort <e>          the default seat
  --packet <text> | --brief <repo-relative path>     the standing packet
  --file <json>             the whole document instead of flags
  --batch <dir>             every *.dd.json in <dir>, one commit, all or nothing";

const HOLD: &str = "\
pij directory assign|release|lineage: a seat takes or leaves a role, or the
prime records a past holding.

Usage:
  pij directory assign <role>[/<instance>] <seat> --why <text> [--project <slug>]
  pij directory assign --batch <file> --why <text> [--project <slug>]
  pij directory release <role>[/<instance>] <seat> --why <text> [--project <slug>]
  pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text>

assign appends a row to roles/<id>/holders.dd.json, sets the seat's role (pa,
pm for pm-<n>, else worker; a prime keeps prime) and names its tmux window
<project>:<role> when this is the seat's first open holding (a second role
keeps the window's name). A second live holder of a one-holder role is allowed
with a warning. --batch takes one `<role>[/<instance>] <seat>` per line (blank
and # lines skipped) and writes them all in one commit, or none. release closes
the row. `pij close <seat>` ends every holding.

lineage add (the prime only) records a closed seat's past holding: no live seat
needed, no seat role set. Times are RFC 3339 with Z or an offset, or a date
(UTC midnight); a holding that overlaps the seat's others is refused.";

const EXTERNAL: &str = "\
pij directory external add: note a seat borrowed from another project.

Usage:
  pij directory external add <seat> --from-project <slug> --why <note> [--role <id>]

Appends a dated entry to community/external.dd.json. Loans are not otherwise
tracked.";

const TEMPLATES: &str = "\
pij directory templates | template-diff: the role templates pij ships.

Usage:
  pij directory templates
  pij directory template-diff <id> [--project <slug>]

templates lists each template with its versions. Templates are examples to
adapt, not a checklist. template-diff shows what changed between the template
version role <id> came from and the latest one. Nothing is ever applied: the
role keeps its own edits, and you copy any change you want by hand.";

const REBUILD: &str = "\
pij directory rebuild: replace this project's directory rows in the store with
what the governance branch head says. The branch is the truth.

Usage:
  pij directory rebuild [--project <slug>]";

const MEMORY: &str = "\
pij directory memory: a role's memory, one note per file (pij/memory).

Usage:
  pij directory memory add <role> --title <t> --hook <h> [--tags <a,b>] [--slug <s>] --body <text>
  pij directory memory add <role> --title <t> --hook <h> [--slug <s>] --body-file <path|->
  pij directory memory <role>                     print memory/INDEX.md
  pij directory memory <role> --note <slug>       print one note

add writes roles/<role>/memory/<yyyy-mm-dd>-<slug>.dd.json, the slug taken
from --title (at most 40 characters, cut on a word) unless --slug gives it, and regenerates memory/INDEX.md with one line per note, keeping
every line pij did not write. One commit per note. --tags repeats or takes a
comma list. --body-file - reads stdin. Notes are never capped, pruned or
deleted; there is no delete verb.

Before `pij close` or a compact of a seat that holds a role, pij asks that
seat to write what it learned here, and holds the close or compact until its
turn ends.";

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    #[test]
    fn help_is_asked_for_anywhere_before_a_literal_separator() {
        assert!(wants_help(&argv(&["--help"])));
        assert!(wants_help(&argv(&["add", "qa", "-h"])));
        assert!(!wants_help(&argv(&["add", "qa", "--", "--help"])));
        assert_eq!(verb(&argv(&["add", "--help"])), Some("add"));
        assert_eq!(verb(&argv(&["--help"])), None);
        assert!(help(Some("add")).unwrap().contains("--batch <dir>"));
        assert!(help(None).unwrap().contains("pij directory <verb> --help"));
        assert!(help(Some("nope")).is_none());
        assert!(help(Some("memory")).unwrap().contains("no delete verb"));
        assert!(help(None).unwrap().contains("pij directory memory <role>"));
        assert!(
            help(Some("add"))
                .unwrap()
                .contains("--from template:<name>")
        );
        assert_eq!(help(Some("templates")), help(Some("template-diff")));
    }

    #[test]
    fn a_shell_word_is_bare_when_safe_and_single_quoted_otherwise() {
        assert_eq!(shell_word("pij-fleet"), "pij-fleet");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word("$(x)"), "'$(x)'");
        assert_eq!(shell_word(""), "''");
    }

    /// Plan 209, F35: add --help names the required flags and the shortest add.
    #[test]
    fn add_help_names_the_required_flags() {
        let add = help(Some("add")).unwrap();
        assert!(
            add.contains("at least one --rule, and one of --packet or\n--brief"),
            "{add}"
        );
        assert!(add.contains("The shortest add:"), "{add}");
    }

    /// Plan 209, F38: add --help says each add commits on its own.
    #[test]
    fn add_help_says_each_add_is_its_own_commit() {
        let add = help(Some("add")).unwrap();
        assert!(add.contains("Each add is its own commit"), "{add}");
        assert!(add.contains("--batch <dir> is all or nothing"), "{add}");
    }
}
