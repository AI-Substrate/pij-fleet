# AGENTS_README — cold-start front door

The fastest path for a fresh agent (on any machine) to clone pij, build it, and
understand how we work. This file is an **index**: each section is a short blurb
plus links into the depth articles under [`docs/how/`](docs/how/).

> **What pij is:** a local control plane that lets agent sessions in different
> harnesses (OMP, Claude Code, Copilot CLI, Codex) message, spawn and govern each
> other. The daemon and native CLI are Rust (`crates/`, binary `pij-rs`); `pij`
> is a thin CLI shim over the daemon's API.
>
> **Where the rules + ops live:** [`AGENTS.md`](AGENTS.md) is the agent **rules**
> (P1–P10, self-improvement loop); [`RUNBOOK.md`](RUNBOOK.md) is the operational
> **runbook**; [`README.md`](README.md) is the overview.

| Surface | What it is |
|---|---|
| `pij-rs` | The daemon (`pij-rs daemon`) and native CLI. Owns seats, delivery, spawn/revive/reap and governance records. |
| `pij` CLI | The shim agents and humans use. It forwards a supported operation to the daemon or refuses with a named error. |
| `/pij` skill | The intent router: it selects a job protocol such as `ready`, `pair`, `delegate`, `peer`, `ops` or `prime`, then uses the CLI underneath. |
| Harness integrations | OMP extension (`.omp/extensions/pij`), Copilot native extension (`.copilot/extensions/pij`), Claude Code hooks and statusline (installed by the daemon), Codex via its tmux pane. |

---

## Cold start

```bash
git clone https://github.com/AI-Substrate/pij-fleet.git && cd pij-fleet
just install            # deps, pij-rs, OMP + extension links, skill, CLI (idempotent)
just bounce-rs          # start the daemon
just doctor             # read-only health check
just self-check         # or: harness boot (fast typecheck + test)
```

→ Depth: [`docs/how/build.md`](docs/how/build.md).

## Build & test

The recipe surface (`just` with no args lists everything), the composite gate
`just self-check` (local paths → typecheck → lockfile → lint → test → Copilot
native → Rust → smoke → commit trailers), and the engineering harness
(`harness boot` / `harness checks` / `harness doctor`). The `harness` CLI is an
ambient global tool; `.harness/` is committed substrate it reads.

→ [`docs/how/build.md`](docs/how/build.md) ·
[`.harness/engineering-harness.md`](.harness/engineering-harness.md)

## How we work (workflow)

We plan with an SDD pipeline (explore → plan → tasks → implement → review →
ship), delegate bounded work through **`/pij pair`** (the flow-pair
orchestrator / coder / reviewer engine), and drive peer sessions through the
**control plane** (the `pij-rs` daemon; `spawn` / `send` / `list` / `state`).

`/pij` is the **skill router** for jobs. `pij` is the **CLI binary** that
performs machine actions. For multi-stream work, `/pij prime` adds repository
governance above the ordinary per-stream `/pij pair` cycle.

→ [`docs/how/workflow.md`](docs/how/workflow.md) ·
[`docs/how/flow-pair.md`](docs/how/flow-pair.md) ·
[`docs/how/pij-rs-api.md`](docs/how/pij-rs-api.md)

## Prime hierarchy, streams & fleets

The canonical ownership tree is:

```text
human
└── o-prime (one governance seat for the repository)
    ├── stream orchestrator (one plan + worktree + branch + fence)
    │   ├── coder / implementer
    │   ├── cold reviewer
    │   └── other bounded peers (validator, researcher, live-test client)
    └── stream orchestrator
        └── its own fleet
```

- The **human** names work, gives binding rulings, and approves merges.
- The **o-prime governs; it does not implement**. It owns the portfolio, roster,
  fences, batons and cross-stream sequencing, and verifies each stream's
  evidence one hop upward.
- Each **stream orchestrator** owns one work item and its fleet. It plans in its
  isolated worktree, delegates bounded implementation through `/pij pair`,
  verifies worker claims, and reports upward.
- Peers belong to their stream, not directly to the o-prime. Streams do not
  coordinate sideways; overlap and dependencies route through the o-prime.
- Use **worktrees/branches/fences for isolation** and **batons for serialized
  shared resources**. Teardown is ownership-aware.

`/pij prime` selects the governance route; `pij orchestration prime` marks the
seat and `pij orchestration baton` manages shared-resource leases.

→ [`docs/how/pij-prime.md`](docs/how/pij-prime.md) ·
[`docs/how/pij-orchestration-baton.md`](docs/how/pij-orchestration-baton.md) ·
[`docs/how/pij-platform.md`](docs/how/pij-platform.md)

## Skills

> Copilot and Codex read skills from **`~/.agents/skills/`**. Claude reads from
> **`~/.claude/skills/`**, which can symlink into that same store.

`just pij-skill-link-global` (run by `just install`) links `skills/pij` to
`~/.agents/skills/pij` from the canonical checkout, so the skill can never drift
from the code. The `pair` route drives the repo-local **flow-pair engine**
(`skills/flow-pair/lib`).

→ [`docs/how/skills.md`](docs/how/skills.md)

## Integrations

- [`docs/how/claude-auto-seat.md`](docs/how/claude-auto-seat.md) — Claude Code
  seats adopted by the daemon-installed SessionStart hook.
- [`docs/how/claude-statusline.md`](docs/how/claude-statusline.md) — the pij
  statusline segment.
- [`docs/how/copilot-native-extension.md`](docs/how/copilot-native-extension.md)
  — Copilot CLI's in-process receiver.
- [`docs/how/pij-pane-signals.md`](docs/how/pij-pane-signals.md) — busy and
  typing signals from tmux panes.
- [`docs/how/pij-telegram.md`](docs/how/pij-telegram.md) — the Telegram bridge.

## Feedback / self-improvement

Every session contributes back: retros, magic-wand wishes and difficulties flow
into the harness so the next agent doesn't hit the same friction.

→ [`docs/how/agent-feedback.md`](docs/how/agent-feedback.md)
