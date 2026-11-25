//! `pij pod` help (PRD 008): the text the CLI prints without a daemon and the
//! daemon returns to a shim.

/// The help for `verb`, or the overview for none; `None` for an unknown verb.
pub fn help(verb: Option<&str>) -> Option<&'static str> {
    match verb {
        None => Some(OVERVIEW),
        Some("plan") => Some(PLAN),
        Some("up") => Some(UP),
        Some("show") => Some(SHOW),
        Some("confirm") => Some(CONFIRM),
        Some("set") => Some(SET),
        Some("down") => Some(DOWN),
        Some("add" | "release") => Some(RESHAPE),
        Some("sessions") => Some(SESSIONS),
        _ => None,
    }
}

const OVERVIEW: &str = "\
pij pod: a group of seats started together from a pod template.

Usage:
  pij pod plan <template> [--set <name>=<value>]... [--name <pod>] [--fresh]
  pij pod up <template> [--set <name>=<value>]... [--name <pod>] [--session <tmux>] [--fresh] [--no-start]
  pij pod up <pod> [--session <tmux>]         bring a pod taken down back, member by member
  pij pod show <pod>
  pij pod set <pod> <name>=<value>
  pij pod down <pod>
  pij pod add <pod> role:<id>[/<instance>] [--set <name>=<value>]... | <seat>
  pij pod release <pod> <seat>
  pij pod sessions <pod> [--limit <n>]       every member's harness sessions
  pij pod confirm <pod> --tool <tool>...      a slot's seat answering its tool canary

A template is a folder holding pod-template.dd.json (or that file), at any path
you name. Every check warns and names its fix; none refuses: pij is a knowledge
system, not the police. `pij pod <verb> --help` for each verb.";

const PLAN: &str = "\
pij pod plan <template> [--set <name>=<value>]... [--name <pod>] [--fresh]

Prints the whole pod and writes and launches nothing: each slot's role, harness,
model, tools and first turn; each consulted role and whether it has a live
holder; each deliverable and its owner; and every warning with the command that
fixes it:
  - a consulted role needed before up with no live holder,
  - a reviewing slot that shares (or may share) its author's model,
  - a deliverable whose owner (or write-back) role is missing,
  - a slot whose role cannot start (its first turn does not build).
Run it before `pij pod up`; up prints and logs the same warnings and starts.";

const UP: &str = "\
pij pod up <template> [--set <name>=<value>]... [--name <pod>] [--session <tmux>] [--fresh] [--no-start]

Creates the pod's record (community/pods/<pod>/ on the governance branch), then
starts each slot as `pij spawn --as role:<id>` does, once per slot, parented to
you, in the template's `work-folder` parameter when it is an absolute folder,
else your folder, in your tmux session (or --session). It creates no branch or worktree. The pod is named --name,
else the template's name.

Each warning is printed with its fix and written to the pod's log; up never
refuses on one. A slot that names tools is sent one canary message asking its
seat to confirm them (`pij pod confirm`); a seat that does not confirm every
tool within the bound is degraded: shown with the missing tools and the fix,
and pij hands it no pod work (a brake on pij's own dispatch; your messages
still reach it). --no-start creates the record only; start each member on its
own with `pij spawn --as role:<id>[/<instance>] --pod <pod>`, recorded as the
slot its role fills. --fresh leaves earlier memory out of every first turn.";

const SET: &str = "\
pij pod set <pod> <name>=<value>

Records a parameter known only after the pod is up, and sends it to each member
whose slot uses it, and only them. A degraded member is not sent it (pij's own
dispatch brake) and is named so you can send it yourself.";

const DOWN: &str = "\
pij pod down <pod>

Records where the pod's work folder stands (its branch and head, from one git
read) and warns that the run is unfrozen when it has uncommitted work. Then
takes each member down exactly as `pij close <seat> --stop` does, once per
member: memory first, the recorded process stopped, the prime told about any
seat that did not answer, a cold seat not woken. A keep-open member (marked so,
or of type knowledge unless marked not) is named and left running. Roles the
pod consults are never touched. The pod reads closing until every member is
down; `pij pod up <pod>` brings it back, reviving each member under revive's
rules.";

const RESHAPE: &str = "\
pij pod add <pod> role:<id>[/<instance>] [--set <name>=<value>]... [--session <tmux>]
pij pod add <pod> <seat>
pij pod release <pod> <seat>

Reshape a running pod; each change is logged and the template is unchanged.
`add role:` starts a member as `pij spawn --as role:<id> --pod <pod>` does;
`add <seat>` adds a seat already running. `release` takes a seat out of the pod
and leaves it running.";

const SESSIONS: &str = "\
pij pod sessions <pod> [--limit <n>]

Each member's harness sessions, newest first (20 unless --limit): start, end,
harness, model, session id, transcript, the context in use at its end, and why
it changed (spawn, revive, compact, rebind or relaunch), as `pij sessions
--history --seat <seat>` lists them. Read from the event log; the context from
at most the last 256 KiB of each transcript, else unknown. Launches nothing.";

const SHOW: &str = "\
pij pod show <pod>

The pod's members with their roles, seats and states, its consulted roles and
their live holders, its deliverables with owners, its degraded slots with their
fixes, and its log. Reads the pod's record and the database; launches nothing.";

const CONFIRM: &str = "\
pij pod confirm <pod> --tool <tool>...

A slot's seat answering its tool canary: name each tool you can use. A tool
you leave out is reported missing, with its fix, in `pij pod show`.";
