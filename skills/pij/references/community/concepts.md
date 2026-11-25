# pij concepts

What a seat, a role, a type, a position, a pod, a seed and a template are in pij, how they relate, and how the team's records (governance) differ from its work.

## A team and its work

A pij fleet is a team of agent seats and the work they do. Some fleets build a product: there the repo is the product and the fleet is the factory that builds it, with the engineering harness (tooling, checks, rules) as the factory's other half. Others are operating teams that do recurring work and build nothing. Either way, governance is the team's HR: who exists, what each is for, what each is told and what each has learned.

## The terms

| Term | Answers | In pij |
|---|---|---|
| seat | Who is running? | One agent session (OMP, Pi, Claude, Copilot or Codex) in a tmux pane, with an id such as `pij-steady-otter`. `pij list` shows the live ones. |
| role | What is it responsible for? | A job a community invents as the work needs it: `coder`, `security`, `live-notes`. Written by `pij directory add` and held by a seat through `pij directory assign`. A role is not a seat: it outlives every seat that holds it and keeps its memory notes. |
| type | How does it live? | One of pij's ten fixed kinds: prime, pa, project-manager, coder, reviewer, worker, knowledge, steward, watcher, human-window. A role names exactly one; it sets when the holder is compacted or closed, what it must keep, and the rules every role of that type follows. `pij docs get types`. |
| position | Who governs whom? | Where a seat sits in the fleet: the designated prime of a project, or a seat role of `pm`, `pa` or `worker` under a parent. Authority comes from here and nowhere else. |
| pod | Which roles were stood up together? | A named group of role instances with shared parameters, started, shown and taken down as one from a pod template (`pij pod`). A seat is in one pod or none. `pij docs get pods`. |
| seed | What is a seat of a type told? | The opening text for a type. pij ships one per type, and a repo may keep its own in governance: `pij directory types <type> --seed` shows the one pij would use. A seat started in a role (`pij-rs spawn --as role:<id>`) gets one first turn: its type's seed and a pointer to its type's page, the role's text with its parameters filled, the creator's focus and packet, and the role's memory index. |
| template | Where does a role start from? | A shipped, versioned example to copy and adapt: role templates (`pij directory templates`). A role made from one records the template and version, and `pij directory template-diff` shows what changed since. Templates are examples, never a checklist. A pod template is a folder a pod command names by its path, in governance or the repo, or a shipped one as `shipped:<name>` (`pij pod templates`; `pij pod adopt` copies one the repo then owns). It may carry its own procedures to run at up and down. |

## A type is a label, never authority

A seat's TYPE (in `pij list` and `pij directory`) is the type of the roles it holds. It tells you how the seat lives, not what it may do. Authority comes from position: the prime designation (`pij project show <slug>`) and the seat roles `pm` and `pa`. The prime, pa and project-manager types describe the seats that hold those positions; giving a role one of those types grants nothing. A `pm` seat holding a coder role shows `coder` and keeps a project manager's powers.

This is how this pij works; where authority comes from is under review and may change in a later version.

Types are checked when a role is written (`pij directory add` refuses a role with no type, an unknown type, or one missing what its type requires) and never enforced on a running seat.

## How they fit together

- A **community** is everything one project's fleet has: its roles, who holds them and their memory. There is one per project.
- A **role** names a **type**. A **seat** holds roles; holding a role makes the seat that role's holder until it is released or the seat closes.
- A seat's **position** says who governs it. Its roles say what it does; their type says how it lives.
- A **pod** groups seats started together. A shared expert, such as a knowledge seat many pods consult, is in no pod; pods name the roles they consult.
- **Memory** belongs to a role, never to a seat: `pij directory memory <role>`, or `--subject <subject>` for one subject's notes, which a pod with a `subject` parameter files under. Closing a seat keeps it (`pij close <seat> --stop` asks a holder to write its note first, then stops it), and the role's next holder reads it.

## Governance and the repo

| | Governance (the team) | The repo (the work) |
|---|---|---|
| Holds | rulings, the role directory and holders, role memory, each first turn a seat was started with, pod records, a repo's own seeds | the work's code and docs, its PRDs and plans, its procedures |
| Lives on | the project's governance branch, under `community/` | the main branch |
| Written by | pij's single writer, with a reason on every change (`--why`) | anyone, through pull requests |

Seat facts (folder, session, process) live in pij's database, looked up by seat id, not in either.

A procedure belongs to the work, not to a seat: it lives in the repo and is true whoever runs it (`pij proc`).

## Two fleets

- **A build fleet** builds software. A prime runs it; a project manager runs a workstream and hands out steps; coders build each step in its own worktree and are compacted after every job; reviewers give one verdict per pull request and are compacted after it; a PA keeps seats healthy.
- **An operating fleet** runs recurring work, such as covering a meeting, and builds no product. A note taker, a researcher and a validator do each occurrence's work and consult a knowledge seat that keeps the area's notes current; the knowledge seat stays when the others close.

The same constructs serve both: only the roles and types differ.

## Read more

- `pij docs get types` and `pij docs get types/<type>`: the ten types.
- `pij docs get building-a-role`: build a role on a type.
- `pij docs get pods`: start a group of seats as one.
- `pij directory --help`: every community verb.
