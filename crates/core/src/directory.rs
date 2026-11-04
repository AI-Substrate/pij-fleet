//! `pij directory` help (plan 195, AC10). Pure text, shared by the native CLI,
//! which prints it without a daemon, and the daemon, which answers shim
//! callers, so the two cannot drift.

/// The help for `pij directory [<verb>] --help`; `None` for an unknown verb.
pub fn help(verb: Option<&str>) -> Option<&'static str> {
    match verb {
        None => Some(OVERVIEW),
        Some("add") => Some(ADD),
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
  pij directory add <id> --why <text> [role flags | --file <json>] [--project <slug>]
  pij directory add --batch <dir> --why <text> [--project <slug>]

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
    }
}
