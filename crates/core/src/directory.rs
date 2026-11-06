//! `pij directory` help (plan 195, AC10). Pure text, shared by the native CLI,
//! which prints it without a daemon, and the daemon, which answers shim
//! callers, so the two cannot drift.

/// The help for `pij directory [<verb>] --help`; `None` for an unknown verb.
pub fn help(verb: Option<&str>) -> Option<&'static str> {
    match verb {
        None => Some(OVERVIEW),
        Some("add") => Some(ADD),
        Some("assign" | "release") => Some(HOLD),
        Some("external") => Some(EXTERNAL),
        Some("rebuild") => Some(REBUILD),
        Some("routes") => Some(ROUTES),
        _ => None,
    }
}

/// Whether argv (after `directory`) asks for help.
pub fn wants_help(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
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
  pij directory add --batch <dir> --why <text> [--project <slug>]
  pij directory assign <role>[/<instance>] <seat> --why <text>
  pij directory release <role>[/<instance>] <seat> --why <text>
  pij directory external add <seat> --from-project <slug> --why <note> [--role <id>]
  pij directory rebuild                               the store from the branch head
  pij directory routes [--write --why <text>]         the routing table, with live holders

Every write is validated by ddocs and committed by the daemon's single writer,
touching only its own paths in the standing governance worktree.

Run `pij directory <verb> --help` for a verb's flags.";

const ADD: &str = "\
pij directory add: write a role definition (a pij/role document).

Usage:
  pij directory add <id> --why <text> [flags]
  pij directory add <id> --why <text> --file <role.dd.json>
  pij directory add --batch <dir> --why <text>

Flags:
  --why <text>              required; becomes the changelog row
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
pij directory assign|release: a seat takes or leaves a role.

Usage:
  pij directory assign <role>[/<instance>] <seat> --why <text> [--project <slug>]
  pij directory release <role>[/<instance>] <seat> --why <text> [--project <slug>]

assign appends a row to roles/<id>/holders.dd.json, sets the seat's role (pa,
pm for pm-<n>, else worker; a prime keeps prime) and renames its tmux window
<project>:<role>. A second live holder of a one-holder role is allowed with a
warning. release closes the row. `pij close <seat>` ends every holding.";

const EXTERNAL: &str = "\
pij directory external add: note a seat borrowed from another project.

Usage:
  pij directory external add <seat> --from-project <slug> --why <note> [--role <id>]

Appends a dated entry to community/external.dd.json. Loans are not otherwise
tracked.";

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
    }
}
