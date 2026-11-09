//! `pij directory` help (plan 195, AC10). Pure text, shared by the native CLI,
//! which prints it without a daemon, and the daemon, which answers shim
//! callers, so the two cannot drift.

/// The help for `pij directory [<verb>] --help`; `None` for an unknown verb.
pub fn help(verb: Option<&str>) -> Option<&'static str> {
    match verb {
        None => Some(OVERVIEW),
        Some("add") => Some(ADD),
        Some("edit") => Some(EDIT),
        Some("retire" | "merge") => Some(RESHAPE),
        Some("assign" | "release" | "lineage") => Some(HOLD),
        Some("external") => Some(EXTERNAL),
        Some("rebuild") => Some(REBUILD),
        Some("routes") => Some(ROUTES),
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
  pij directory [--project <slug>] [--json]           this project's roles, holders and next actions
  pij directory --suggest [--project <slug>]          also every role suggestion in full (core roles, security)
  pij directory add <id> --why <text> [role flags | --file <json>] [--project <slug>]
  pij directory add <id> --from template:<name>[@v<n>] --why <text> [role flags]
  pij directory add --batch <dir> --why <text> [--project <slug>]
  pij directory edit <id> --why <text> (--set <field>=<value>... | role flags | --file <json>)
  pij directory retire <role> --why <text>             a role no longer in use
  pij directory merge <from> --into <to> --why <text>  fold one role into another
  pij directory templates [--show <name>[@vN]]        the role templates, or one's body
  pij directory template-diff <id>                    what changed in its template since
  pij directory assign <role>[/<instance>] <seat> --why <text>
  pij directory assign --batch <file> --why <text>      many holdings, one commit
  pij directory release <role>[/<instance>] <seat> --why <text>
  pij directory release --batch <file> --why <text>     many holdings, one commit
  pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text>
  pij directory lineage add --batch <file> --why <text>   many past holdings, one commit
  pij directory external add <seat> --from-project <slug> --why <note> [--role <id>]
  pij directory rebuild                               the store from the branch head
  pij directory routes [--write --why <text>]         the routing table, with live holders
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

const EDIT: &str = "\
pij directory edit: change a role that exists (plan 206).

Usage:
  pij directory edit <id> --why <text> --set <field>=<value> [--set ...] [role flags]
  pij directory edit <id> --why <text> --file <role.dd.json>

--set takes title, kind, lifecycle, holders, instance_key, default_limit,
summary, template, template_version, harness, model, effort, packet or brief.
The role flags `add` takes work too. A list flag (--rule, --scope-path,
--out-of-scope, --forbidden, --input, --output-kind) REPLACES THE WHOLE LIST
with the items given. To change one item, use --add-<list> <item> or
--remove-<list> <item> (repeatable, for example --add-rule, --remove-scope-path):
per list, any replacing flag applies first, then every remove, then every add.
Removing an item the list does not hold, or adding one it already holds,
refuses. Every edit prints each list it changed, before and after, and each
item it removed. --file replaces the whole document but keeps its changelog.
Every edit adds one changelog row (--why) and commits through the writer; an
edit that changes nothing, or a role that does not exist, refuses.";

const RESHAPE: &str = "\
pij directory retire | merge: reshape the community as the work changes (plan 206).

Usage:
  pij directory retire <role> --why <text> [--drop-queued] [--project <slug>]
  pij directory merge <from> --into <to> --why <text> [--drop-queued] [--project <slug>]

retire marks the role retired (meta.state) with a changelog row. merge retires
<from> with meta.merged_into <to> and adds a changelog row to both. Each is one
commit. Nothing is moved or rewritten: a retired role's memory, outputs and
lineage stay readable, and the view lists a merged role under its target. A role
with a live holder refuses (release it first); a retired role cannot be assigned,
edited, retired again or merged into, and sends to it are refused.

A role with requests queued for a holder refuses, listing each request's id,
requester and age: assign a holder to take them, or pass --drop-queued to
withdraw them (each is recorded on the spine and its requester told once, as an
FYI). merge does not move queued requests to <to>.";

const HOLD: &str = "\
pij directory assign|release|lineage: a seat takes or leaves a role, or the
prime records a past holding.

Usage:
  pij directory assign <role>[/<instance>] <seat> --why <text> [--project <slug>]
  pij directory assign --batch <file> --why <text> [--project <slug>]
  pij directory release <role>[/<instance>] <seat> --why <text> [--project <slug>]
  pij directory release --batch <file> --why <text> [--project <slug>]
  pij directory lineage add <role>[/<instance>] <seat> --from <time> --to <time> --why <text>
  pij directory lineage add --batch <file> --why <text> [--project <slug>]

assign appends a row to roles/<id>/holders.dd.json, sets the seat's role (pa,
pm for pm-<n>, else worker; a prime keeps prime) and names its tmux window
<project>:<role> when this is the seat's first open holding (a second role
keeps the window's name). A second live holder of a one-holder role is allowed
with a warning. The prime is never assigned: it is designated with
`pij project set <project> --prime <seat>` (C24). --batch takes one
`<role>[/<instance>] <seat>` per line (blank
and # lines skipped) and writes them all in one commit, or none. release closes
the row; release --batch takes the same lines and ends them all in one commit,
or none. `pij delivered <pr>` ends the holdings a merged PR finished, and
`pij close <seat>` ends every holding.

lineage add (the prime only) records a closed seat's past holding: no live seat
needed, no seat role set. Times are RFC 3339 with Z or an offset, or a date
(UTC midnight); a holding that overlaps the seat's others is refused. --batch
takes one `<role>[/<instance>] <seat> <from> <to>` per line and writes them all
in one commit, or none.";

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
  pij directory templates --show <name>[@vN]
  pij directory template-diff <id> [--project <slug>]

templates lists each template with its versions; --show prints one
template's exact body (the latest version by default) and its path. Templates are examples to
adapt, not a checklist. template-diff shows what changed between the template
version role <id> came from and the latest one. Nothing is ever applied: the
role keeps its own edits, and you copy any change you want by hand.";

const REBUILD: &str = "\
pij directory rebuild: replace this project's directory rows in the store with
what the governance branch head says. The branch is the truth.

Usage:
  pij directory rebuild [--project <slug>]";

const ROUTES: &str = "\
pij directory routes: the routing table, \"send it, don't do it\".

Usage:
  pij directory routes [--project <slug>]
  pij directory routes --write --why <text> [--project <slug>]

pij builds the table from every role's scope (paths and rules) and keeps it in
community/ROUTES.md, rewritten in the same commit as any role change. Two roles
claiming one path give a W-ROUTES-OVERLAP warning. The table reaches a seat as
an FYI when it is assigned a role, after a compact and when a revive brings it
back; the prime gets the changed rows as one FYI. A change of holder sends
nothing. This verb prints it with the live holders and says whether ROUTES.md
is stale; --write regenerates a stale one in one commit.";
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
        assert!(help(Some("routes")).unwrap().contains("--write --why"));
        assert!(help(Some("memory")).unwrap().contains("no delete verb"));
        assert!(help(None).unwrap().contains("pij directory memory <role>"));
        assert!(
            help(Some("add"))
                .unwrap()
                .contains("--from template:<name>")
        );
        assert_eq!(help(Some("templates")), help(Some("template-diff")));
        assert!(
            help(Some("edit"))
                .unwrap()
                .contains("--set <field>=<value>")
        );
        assert!(help(None).unwrap().contains("pij directory edit <id>"));
        assert_eq!(help(Some("retire")), help(Some("merge")));
        assert!(
            help(None)
                .unwrap()
                .contains("pij directory merge <from> --into <to>")
        );
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
