# Pods

A pod is a named group of role instances with shared parameters, planned, brought up, shown and taken down as one. A seat is in one pod or none. A shared expert, such as a knowledge seat many pods consult, is in no pod: a pod names the roles it consults. `pij pod --help` lists every verb; `pij pod <verb> --help` explains one.

## A pod template

A pod is made from a template: a folder holding `pod-template.dd.json` (schema `pij/pod-template`), or that file, at any path you name. A relative path is read from your folder, so a template may sit in governance or in the main repo. A template holds:

- `parameters`: each required or optional, set once for the whole pod with `--set <name>=<value>`. Two have a meaning to pij: `subject` files the members' memory under that subject, and `work-folder` (an absolute folder) is where members start and what `down` freezes.
- `slots`: each a seat the pod starts, in a role (and instance), with its seed (the slot's focus), the parameters it uses, the tools it needs, `keep_open` and `not_model_of` (a slot that should not share another's model).
- `wiring`: how the slots feed each other.
- `consulted`: outside roles the pod asks, and whether one must have a live holder `before_up`.
- `deliverables`: each with an owner role, its stages, what each stage needs (`stage_needs`) and a role it is written back to (`write_back`).

A pod's record is `community/pods/<pod>/pod.dd.json` (schema `pij/pod`) on the governance branch: its template and that template's version and hash, the filled parameters, members, consulted roles, deliverables and a log. Seat facts stay in pij's database. The template is never written.

## Plan it

    pij pod plan <template> [--set <name>=<value>]... [--name <pod>] [--fresh]

`plan` fills the parameters, builds each slot's first turn as `spawn --as` would, runs the checks and shows the slots (role, harness, model, tools, first turn), the wiring, consulted roles with their live holders, deliverables and warnings. It writes and starts nothing. `--fresh` leaves the roles' earlier memory out of each first turn and says so.

Checks warn and name their fix; none refuses. pij warns about:

- a consulted role needed before up with no live holder;
- a slot marked `not_model_of` another that shares its model, or whose model pij cannot tell;
- a deliverable whose owner or write-back role the project lacks;
- a slot whose role cannot start.

An unknown or missing parameter refuses `E-RS-POD-PARAM`, and a template that does not hold together refuses `E-RS-POD-TEMPLATE`.

## Bring it up

    pij pod up <template> [--set <name>=<value>]... [--name <pod>] [--session <tmux>] [--fresh] [--no-start]

`up` commits the record with the warnings in its log, then starts each slot through the same path as `pij-rs spawn --as role:<id>`, parented to you, in `work-folder` when it is set and otherwise in your folder. `--session` places the seats in a named tmux session. A slot that does not start is a warning naming the `spawn --as` command that starts it. A pod that already has a record refuses `E-RS-POD-EXISTS`.

With `--no-start` pij writes the record only; each member then joins on its own:

    pij-rs spawn --as role:<id>[/<instance>] --pod <pod>

**The tool canary.** Each started slot that names tools is asked to run `pij pod confirm <pod> --tool <tool>...`. pij waits up to 15 minutes for the answers, records each member's canary as `confirmed` or `degraded` (with the missing tools and the fix), and tells the seat that ran `up` which slots are degraded. A later `confirm` brings a degraded slot back.

**The degraded-slot brake.** pij sends no pod work to a degraded slot: `set` skips it and names it, and a stage that would be written back to it is recorded and not sent. The brake only ever makes pij send less; messages from anyone else still reach the seat.

## Show and follow it

    pij pod show <pod> [--json]
    pij pod activity <pod> [--since <time>] [--json]
    pij pod sessions <pod> [--limit <n>] [--json]
    pij sessions --history [--seat <id>] [--limit <n>]

Each pod verb prints a short text view; `--json` gives the full answer.

- `show` prints each member's slot, seat, role, type, state, canary and folder; consulted roles with whether they have a live holder; the wiring; each deliverable's owner and write-back role; and degraded slots with their fixes. `--json` adds each member's harness and model, the owner roles' live holders, each deliverable's stages, the log and the pod's state (`closed` once every member that is not keep-open is down).
- `activity` prints what the pod did, in time order (members' activity, reports, memory notes, launches, closes, the messages they received, their outputs, and their sends to consulted roles), then where each member's current transcript is: its path, not its contents. `--since` takes milliseconds since the epoch or a time like `2026-11-25T04:00:00.000Z`. Any seat may read it.
- `sessions` prints each member's harness sessions, newest first (20 unless `--limit`): when each began and ended, why it began (`spawn`, `revive`, `compact`, `rebind` or `relaunch`), its harness, model, session id and last context size. `--json` adds each session's transcript path.
- `pij sessions --history` lists the same for every seat that is not closed, with each transcript path; name a closed seat with `--seat <id>`.

A context size is read from the last 256 KiB of the transcript and reads `unknown` when the transcript is gone or that tail holds no usage figure.

## Change it

    pij pod set <pod> <name>=<value>
    pij pod add <pod> role:<id>[/<instance>] [--set <name>=<value>]...
    pij pod add <pod> <seat>
    pij pod release <pod> <seat>

- `set` records a parameter the template declares and sends it only to the members whose slot uses it.
- `add role:` starts a new member; `add <seat>` takes in a seat already running, in its role in the project.
- `release` takes a member out and leaves it running.

Each change is logged. A seat already in another pod refuses `E-RS-POD-MEMBER`, naming that pod and the `pij pod release` that frees it.

**No secrets in parameters.** A `--set` value that looks like a key, token or password refuses `E-RS-FIRST-TURN-SECRET` at `plan`, `up`, `set` and `add role:`, because the record and every first turn are committed to the governance branch. It names the parameter, never the value.

## Deliverables and stages

    pij pod stage <pod> <deliverable> <stage> --output <path> [--from <slot>]...

`stage` records one stage of a deliverable, with its output and the slots whose work it folds in: all the slots the stage needs, unless `--from` names fewer. A stage still waiting on a slot is recorded with a warning naming it, so a late member never blocks; its work goes into a later stage. When the deliverable names a write-back role, the stage is sent to that role as one request, as `pij send role:<id>` sends it, unless the brake holds it. Recording a stage twice refuses.

## Take it down, and revive it

    pij pod down <pod>
    pij pod up <pod> [--session <tmux>]

**The freeze at down.** `down` records the work folder's branch and head, and warns that the run is unfrozen if the folder has uncommitted changes. Then it closes each member exactly as `pij close <seat> --stop` would, so each one is asked to write its memory note first.

**Keep-open members** are left running and named in a warning: a slot marked `keep_open: yes`, or a knowledge-type slot unless it is marked `no`. Consulted roles are never touched. `show --json` reads `closed` once every member that is not keep-open is down.

`up <pod>` on a pod that is down revives each member, one at a time, under revive's own rules: a cold seat's refusal comes back as a warning naming that seat. On a pod that is up it refuses `E-RS-POD-EXISTS`.

## Coming

Planned and not in this pij yet; this section changes as each ships.

- **Template procedures (PRD 010).** A template's own steps to run at up and at down. Until then `down` has none to run.
- **Shipped pod templates (PRD 010).** Versioned examples a repo copies and adapts. Today you write your own template.

## Read more

- `pij docs get concepts`: what a pod, seat, role and type are.
- `pij docs get building-a-role`: the roles a slot names.
- `pij directory memory <role> --subject <subject>`: a role's notes for one subject.
