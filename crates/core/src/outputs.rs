//! `pij output` and `pij outputs` help (plan 200). Pure text, shared by the
//! native CLI, which prints it without a daemon, and the daemon, which answers
//! shim callers, so the two cannot drift.

/// The help for `pij output --help` (`family` "output") or `pij outputs --help`.
pub fn help(family: &str) -> &'static str {
    if family == "outputs" { OUTPUTS } else { OUTPUT }
}

const OUTPUT: &str = "\
pij output: write a role's output (a pij/output document) and tell whoever asked.

Usage:
  pij output verdict --role <id> --subject <address> --head <sha> --result pass|pass-with-notes|block [flags]
  pij output deliverable|note|handover --role <id> --subject <address> [flags]

Flags:
  --role <id>               the role it is written for
  --subject <address>       what it is about, such as pr:132
  --head <sha>              the exact head a verdict is for (required for a verdict)
  --result <r>              a verdict's result; other kinds may say failed
  --note-owed <text>        repeatable; what the work still owes
  --evidence <json>         repeatable; {\"command\":…,\"exit_code\":…,\"log\":…}
  --link <url>              an external link, such as the PR review
  --answers <msg_id>        the request it answers (ruling C3)
  --body <text> | --body-file <path|->   the output itself; a handover holds what is in flight
  --project <slug>          default: the project whose repo holds this directory

Writes community/roles/<id>/outputs/<yyyy-mm>/<dd>-<kind>-<subject>[-<head7>].dd.json in
one commit. A writer who does not hold the role is recorded with
writer_holds_role false. --answers marks the request answered and tells its
requester in the reply mode it asked for (wake, fyi or none); a block verdict
or a failed result always wakes, a cold requester as an audited forced wake. An
output nobody asked for goes as an FYI to the seat above the writer (its
parent, else the prime). One seat is told.";

const OUTPUTS: &str = "\
pij outputs: a project's role outputs.

Usage:
  pij outputs --subject <address> [--head <sha>]   outputs about a subject; with --head, only for that exact head
  pij outputs --role <id> [--limit <n>]            a role's outputs, newest first (default 50)

A verdict on an older head is not an answer for a newer one: with --head,
a moved head answers none.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_family_has_its_own_help() {
        assert!(help("output").contains("--answers <msg_id>"));
        assert!(help("outputs").contains("a moved head answers none"));
    }
}
