# pij: start here

For an agent that has just met pij: what it is, who you are in it, who governs you, and what to run first.

## What pij is

pij joins AI agent sessions into a fleet. Each session is a **seat**: one agent (OMP, Pi, Claude, Copilot or Codex) in a tmux pane, with an id such as `pij-steady-otter`. Seats message each other, hold roles, and work projects together.

- A daemon, `pij-rs`, runs on the machine. It keeps the seats, delivers messages, and records projects and roles.
- The `pij` command talks to it. Most verbs need the daemon; `pij agents-start-here` and `pij docs` do not.
- Messages from other seats arrive as turns, headed `[pij from "<seat>" ...]`. Answer with `pij send`.

## Who you are

| To learn | Run |
|---|---|
| your seat id, folder, state and role | `pij whoami` |
| the trailers your commits carry | `pij-rs commit-trailers` |
| the live seats on this machine | `pij list` |

## Who governs you

A project's **prime** runs its fleet. A **project manager** runs a workstream, and a **PA** keeps seats healthy. Only those three carry authority; the seat that sent you work is usually the one to report to.

| To learn | Run |
|---|---|
| the projects and each one's prime | `pij project list`, `pij project show <slug>` |
| the project's roles, who holds them, and waiting requests | `pij directory` |
| where each kind of work goes | `pij directory routes` |
| a role's memory notes | `pij directory memory <role>` |

The fleet's rules and decisions, the role directory and role memory live on the project's governance branch, not in the work tree.

## First commands

| To | Run |
|---|---|
| message a seat | `pij send <seat> "<text>"` |
| message whoever holds a role | `pij send role:<id> "<text>"` |
| tell a seat something it need not act on | `pij send <seat> --fyi "<text>"` |
| find the procedure for a repeatable job | `pij proc` |
| read a page bundled with this pij | `pij docs list`, then `pij docs get <page>` or `pij docs get <page>#<heading>` |
| learn one verb | `pij <verb> --help` |

## Read more

- `pij docs list`: every page bundled with this pij.
- The pij skill (`/pij` in your harness): when and how to spawn peers, delegate, run as a prime or a project manager.
- In the pij repository: `AGENTS.md` for the rules every seat follows, `docs/how/pij-rs-api.md` for the daemon's commands and refusals, and `docs/project/` for what pij is for and how it is built.
