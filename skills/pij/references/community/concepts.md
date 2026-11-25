# pij concepts

What a seat, a role, a type, a position, a pod, a seed and a template are in pij, how they relate, and how the team's records (governance) differ from the work (the repo).

## The factory and the product

pij builds the thing that builds the thing. The **repo** is the product. The **factory** that builds it has two parts: the **fleet** (the seats, their roles, and what they know) and the **engineering harness** (the tooling, checks and rules the work must pass). Governance is the fleet's HR: who exists, what each is for, what each is told and what each has learned.

## The terms

| Term | Answers | In pij |
|---|---|---|
| seat | Who is running? | One agent session (OMP, Pi, Claude, Copilot or Codex) in a tmux pane, with an id such as `pij-steady-otter`. `pij list` shows the live ones. |
| role | What is it responsible for? | A job a community invents as the work needs it: `coder`, `security`, `live-notes`. Written by `pij directory add` and held by a seat through `pij directory assign`. A role is not a seat: it outlives every seat that holds it and keeps its memory notes. |
| type | How does it live? | One of pij's ten fixed kinds: prime, pa, project-manager, coder, reviewer, worker, knowledge, steward, watcher, human-window. A role names exactly one; it sets when the holder is compacted or closed, what it must keep, and the rules every role of that type follows. `pij docs get types`. |
| position | Who governs whom? | Where a seat sits in the fleet: the designated prime of a project, or a seat role of `pm`, `pa` or `worker` under a parent. Authority comes from here and nowhere else. |
| pod | Which roles were stood up together? | A named group of role instances started, shown and taken down as one, with shared parameters. A seat is in one pod or none. |
| seed | What is a seat told at start? | The opening text for a type. pij ships one per type; a repo may keep its own in governance (`pij directory types <type> --seed` shows the one pij would use). A seat's first turn is the seed plus its role's text, the filled parameters, the creator's packet and the role's memory index. |
| template | Where does a role or pod start from? | A shipped, versioned example to copy and adapt: role templates (`pij directory templates`) and pod templates. A repo owns its copy; templates are examples, never a checklist. |

## A type is a label, never authority

A seat's TYPE (in `pij list` and `pij directory`) is the type of the roles it holds. It tells you how the seat lives, not what it may do. Authority comes from position: the prime designation (`pij project show <slug>`) and the seat roles `pm` and `pa`. The prime, pa and project-manager types describe the seats that hold those positions; giving a role one of those types grants nothing. A `pm` seat holding a coder role shows `coder` and keeps a project manager's powers.

Types are checked when a role is written (`pij directory add` refuses a role with no type, an unknown type, or one missing what its type requires) and never enforced on a running seat.

## How they fit together

- A **community** is everything one project's fleet has: its roles, who holds them, their memory, and its pods. There is one per project.
- A **role** names a **type**. A **seat** holds roles; holding a role makes the seat that role's holder until it is released or the seat closes.
- A seat's **position** says who governs it. Its roles say what it does; their type says how it lives.
- A **pod** groups role instances that work together. A shared expert (a knowledge seat many pods consult) is in no pod; pods name the roles they consult.
- **Memory** belongs to a role instance (role plus subject), never to a seat, and is never shared across subjects. Closing a seat keeps it; a seat or a pod can be revived.

## Governance and the repo

| | Governance (the team) | The repo (the work) |
|---|---|---|
| Holds | rulings, the role directory and holders, role memory, seeds, pod templates and pod folders | the product's code and docs, its PRDs and plans, its procedures |
| Lives on | the project's governance branch, under `community/` | the main branch |
| Written by | pij's single writer, with a reason on every change (`--why`) | anyone, through pull requests |

Seat facts (folder, session, process) live in pij's database, looked up by seat id, not in either.

A procedure belongs to the work, not to a seat: it lives in the repo and is true whoever runs it (`pij proc`).

## Two fleets

- **A build fleet** builds software. A prime runs it; a project manager runs a workstream and hands out steps; coders build each step in its own worktree and are compacted after every job; reviewers give one verdict per pull request and are compacted after it; a PA keeps seats healthy.
- **An operating fleet** runs recurring work, such as covering a meeting. A pod is stood up per occurrence (a note taker, a researcher, a validator), consults a knowledge seat that keeps the area's notes current, and comes down when the work ends; the knowledge seat stays.

The same constructs serve both: only the roles, types and pods differ.

## Read more

- `pij docs get types` and `pij docs get types/<type>`: the ten types.
- `pij docs get building-a-role`: build a role on a type.
- `pij directory --help`: every community verb.
