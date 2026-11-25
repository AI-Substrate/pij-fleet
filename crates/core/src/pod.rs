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
        Some("activity") => Some(ACTIVITY),
        Some("stage") => Some(STAGE),
        Some("templates" | "adopt" | "template-diff") => Some(SHIPPED),
        Some("export") => Some(EXPORT),
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
  pij pod down <pod> [--skip-close-procedure]
  pij pod add <pod> role:<id>[/<instance>] [--set <name>=<value>]... | <seat>
  pij pod release <pod> <seat>
  pij pod sessions <pod> [--limit <n>]       every member's harness sessions
  pij pod activity <pod> [--since <time>]    what the pod did, in order; open to any seat
  pij pod stage <pod> <deliverable> <stage> --output <path> [--from <slot>]...
  pij pod confirm <pod> --tool <tool>...      a slot's seat answering its tool canary
  pij pod templates                           the pod templates pij ships
  pij pod adopt <name>[@v<n>] [--into <path>] --why <text>
  pij pod template-diff <path>                what changed upstream since a copy
  pij pod export <pod> --as-template <path> --why <text>   the pod as it ran, as a template

A template is a folder holding pod-template.dd.json (or that file), at any path
you name, or a shipped one as shipped:<name>[@v<n>]. Every check warns and names its fix; none refuses: pij is a knowledge
system, not the police. Two refusals guard the record itself: a --set value that
looks like a secret (the record is committed to the governance branch), and a
seat already in another pod. `pij pod <verb> --help` for each verb.";

const PLAN: &str = "\
pij pod plan <template> [--set <name>=<value>]... [--name <pod>] [--fresh]

Prints the whole pod and writes and launches nothing: each slot's role, harness,
model, tools and first turn; each consulted role and whether it has a live
holder; each deliverable and its owner; and every warning with the command that
fixes it:
  - a consulted role needed before up with no live holder,
  - a reviewing slot that shares (or may share) its author's model,
  - a deliverable whose owner (or write-back) role is missing,
  - a slot whose role cannot start (its first turn does not build),
  - a procedure the template names that is not where it says (its own folder's
    procedures/, or the repo's for a linked one).
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
dispatch brake) and is named so you can send it yourself. `subject` and
`work-folder` set this way drive the pod from then on (the freeze at down
reads the work folder). A value that looks like a secret is refused, naming
the parameter: the record is committed to the governance branch.";

const DOWN: &str = "\
pij pod down <pod> [--skip-close-procedure]

When the template names a close procedure, the pij CLI first runs it to its
end in your process (`pij proc <id> --template <folder> --key pod-<pod>`). If
it stops early (a decision to record, a guide step to mark, a failed check),
down closes nothing and prints where it stopped and the command to resume; run
down again once it is done. --skip-close-procedure goes on without it and says
so in the pod's log. That stop is a brake: removing it only closes more.

Then records where the pod's work folder stands (its branch and head, from one git
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
`add <seat>` adds a seat already running. A seat belongs to one pod: one already
in another is refused (E-RS-POD-MEMBER), naming that pod and the release that
frees it. `release` takes a seat out of the pod and leaves it running.";

const SESSIONS: &str = "\
pij pod sessions <pod> [--limit <n>]

Each member's harness sessions, newest first (20 unless --limit): start, end,
harness, model, session id, transcript, the context in use at its end, and why
it changed (spawn, revive, compact, rebind or relaunch), as `pij sessions
--history --seat <seat>` lists them. Read from the event log; the context from
at most the last 256 KiB of each transcript, else unknown. Launches nothing.";

const ACTIVITY: &str = "\
pij pod activity <pod> [--since <time>]

What the pod did, in time order: each member's state changes, reports, memory
notes and outputs, the sends it received, and the sends members made to the
pod's consulted roles; then where each member's transcript is. --since takes
milliseconds since the epoch or a time like 2026-11-25T04:00:00.000Z. Any seat
may run it, in the pod or not. Reads the event log; writes and launches
nothing.";

const STAGE: &str = "\
pij pod stage <pod> <deliverable> <stage> --output <path> [--from <slot>]...

Records one stage of a deliverable with the slots whose work it folds in (all
the stage needs, unless --from names fewer). A late member never blocks a
stage: one still waiting on a slot is recorded with a warning naming it, and
that slot's work is folded into a later stage. `pij pod show` lists each
stage, recorded or open, and the slots an open one waits on. When the template
names a write-back role for the deliverable, pij sends the stage to it as one
request, as `pij send role:<id>` does; the request stays with that role after
the pod goes down.";

const SHOW: &str = "\
pij pod show <pod>

The pod's members with their roles, seats and states, its consulted roles and
their live holders, its deliverables with owners, its degraded slots with their
fixes, and its log. Reads the pod's record and the database; launches nothing.";

const CONFIRM: &str = "\
pij pod confirm <pod> --tool <tool>...

A slot's seat answering its tool canary: name each tool you can use. A tool
you leave out is reported missing, with its fix, in `pij pod show`. Run again
once the tools are there: a degraded slot that now confirms every tool is
recorded confirmed, and pij hands it pod work again.";

const SHIPPED: &str = "\
pij pod templates
pij pod adopt <name>[@v<n>] [--into <path>] --why <text>
pij pod template-diff <path>

pij ships pod templates as versioned originals that never change once shipped.
Name one in any pod command as shipped:<name>[@v<n>], or take a copy the repo
owns: adopt writes the version (latest unless @v<n>) to governance
(community/pod-templates/<name>/, one commit) or to the folder --into names (as
files you commit), recording where it came from in meta.from. Edit the copy
freely and name it by its path. template-diff shows what changed upstream since
the copy's version and applies nothing. None of them starts a seat.";

/// `pij community --help` (PRD 010 req-03).
pub const COMMUNITY_HELP: &str = "\
pij community init [--seeds all|<type>,...] [--templates <name>,...] [--into <path>] --why <text>

A prime's setup job, run once for a new project: copies each type's seed (what
seats of that type are told; the repo's copy wins) and the named shipped pod
templates into governance in one commit, or with --into puts the templates at
<path>/<name>/ as files you commit. Anything already there is kept and listed.
It prints what it wrote and where. pij copies nothing on its own, and init
starts no seat.";

const EXPORT: &str = "\
pij pod export <pod> --as-template <path> --why <text>

Writes the pod as it ran as a template at <path>: every member as a slot (a
seat added while it ran becomes a slot of its own), with the original
template's parameters, wiring, consulted roles, deliverables and its own
procedures. No seat id, folder or parameter value is written. In the
governance worktree it is one commit; elsewhere it is files you commit. Plan
it with pij pod plan <path>. The prime's view suggests this when two pods grow
to the same roles. It starts no seat.";
