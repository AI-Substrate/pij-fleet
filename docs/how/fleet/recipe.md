# The fleet recipe — what we actually run

Mechanical steps, current as of 2026-08-08, **run by hand**. Every step that is fiddly here is
a candidate for the future `pij fleet` verb.

## 0. Prerequisites

```bash
pij daemon status          # must be running
pij whoami                 # you must be prime (role: prime)
gh auth status             # PR work needs a live token — it expires silently
tmux ls                    # the fleet dies with the server; know what is up
pij models                 # discover model ids; NEVER hardcode them
```

## 1. Partition

Do [`partitioning.md`](./partitioning.md) **before anything is created**. The partition is the
design; everything after it is typing.

## 2. Project + streams

```bash
pij project create "<description>" --slug <slug>
pij stream create --project <slug> --slug <stream> [--base <ref>] [--ordinal N]
```

`stream create` reserves an ordinal and creates the worktree + branch as one attributed
allocation. One stream per partition slice.

## 3. Write the briefs to disk, one per stream

**Never put a brief in a `pij send` body.** Report/message surfaces truncate (280/200 chars —
pij#123), and a quoted body executes shell substitutions (pij#128). Write the file, send a
pointer.

A brief must carry:

- **Identity + parent** — who the PM is, who it reports to.
- **The issues**, with their numbers and the citations they carry.
- **The worktree path and branch**, absolute.
- **File ownership**, explicitly — *"you own these files; you may not write outside them
  without asking"*.
- **The process** — `/builder` flow (`1a explore` → `1b plan` → validate → implement), then
  `/pij pair` for the coder+reviewer cycle.
- **Autonomy + done** — run to a PR with green CI; do not merge.
- **The question protocol** — inline text to the prime, one at a time, one sentence of context
  and one sentence of ask. Never a modal question UI.
- **Card cadence** — `pij report now` at both edges of each unit of work.

## 4. Spawn

```bash
pij spawn --harness copilot --model <id>        # from the MAIN checkout — see the trap below
pij link <child> --parent <prime>               # NO --role: it overwrites the role stamp
pij orchestration role set <child> pm
pij task set <child> "<one-line charter>"
pij send <child> --body-file <brief path>
```

> **TRAP — `pij link --role` overwrites an existing role stamp.** Omit `--role` when
> re-parenting a seat that is already stamped, or a PA silently becomes something else and the
> capability gate changes under it.

## 5. Supervise

- **Subscribe a watcher for every seat, and for yourself.** Nothing re-points a subscription
  when its watcher dies (pij#154), so re-audit after any crash — the repair has a measured
  half-life of about a day.
- **Run `pij anomalies` unscoped.** `--project` and `--here` both filter out rows you need.
- **Relay questions immediately.** A PM blocked on a ruling is pure latency, and the prime's
  own state should read `question` while any ask is outstanding, so the board shows it.

## 6. Converge

- **Verify CI with `gh pr view <n> --json statusCheckRollup`**, not `gh pr checks` — the latter
  reports against the last run associated with the branch and shows superseded results after a
  re-run.
- **Merge one at a time**, sequencing any pair that shares a large file.
- **CI supersedes per ref** (shipped as pij#157) — merging N PRs no longer starts N full runs.
  Before that fix, five sequential merges burned five runs and four had to be cancelled by
  hand.

## 7. Close out

Reap worktrees only after the work is merged **and** anything of value in the seat's buffer is
captured. A worktree reap destroys the seat's session buffer; a retro that lives only there is
gone. (This has happened: a custody snapshot was the only surviving copy of one PM's 20 retro
observations.)
