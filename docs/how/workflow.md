# How: the pij workflow

How we plan, delegate, and drive work in pij. This is a **narrative map** — it
ties the three moving parts together and points at the depth articles. It does
not duplicate them: [`flow-pair.md`](flow-pair.md) and [`pij-rs-api.md`](pij-rs-api.md) are the
authorities for their layers.

```
the-flow          flow-pair                    control-plane (pij)
(how we plan)  →  (how we delegate)        →   (how peers talk + are driven)
explore→plan→     orchestrator / worker /      daemon switchboard +
tasks→implement   reviewer over the-flow       spawn / send / list / tail
→review→ship      + a learning ledger          across pi/claude/copilot/codex
```

## 1. the-flow — the SDD pipeline

Spec-driven development runs through one front-door skill, `the-flow`. Planning
artifacts live on disk under `docs/plans/<ord>-<slug>/` and the pipeline walks a
fixed set of stages (direct-jump form `/the-flow <id> <verb>`):

| Stage | Verb | Produces |
|-------|------|----------|
| `1a` | explore | `research-dossier.md` |
| `1b` | plan | `<slug>-plan.md` (business spec + implementation plan in one doc) |
| `2c` | workshop | `workshops/*.md` (design decisions) |
| `3a` | adr | `docs/adr/*.md` |
| `5` | tasks | `tasks/<phase>/tasks.md` |
| `6` | implement | code + execution log (exactly one phase) |
| `6a` | progress | updated task table + execution log |
| `7` | review | `reviews/*.md` |
| `8` | ship | pushed branch + PR + watched CI |

Guided mode (`/the-flow` with no args) coaches from the on-disk flight plan;
direct jump runs a single stage. Plan 028 (these docs) is itself a the-flow
plan — see `docs/plans/028-docs-cold-start/`.

## 2. flow-pair — the delegation seam

`flow-pair` wraps `the-flow` in a **three-session orchestrator / worker /
reviewer** delegation pattern with a central prompt-learning ledger:

- an **expensive orchestrator** plans, routes, reviews, and records learnings;
- a **cheap worker** executes one bounded packet at a time within allowed paths;
- an **independent cross-model reviewer** runs a clean-room review to decorrelate
  blind spots from the implementer.

Packets are **pointer-delivered**: the full packet is written to the ledger
first, then a short path pointer is sent over the control plane. Use it when work
is bounded enough to delegate but benefits from an expensive supervisor and
compounding cross-run learning.

→ Full depth: [`flow-pair.md`](flow-pair.md) (sessions, per-stage cycle, the
ledger layout, the prompt-lab, and the test-quality gate).

## 3. The control plane — peers that talk and get driven

Underneath both sits `pij`: a machine-wide control plane that lets running agent
sessions discover, message, observe, and drive each other. The headline economy
is **cheap generation, expensive review** — a parent reviewer instructs a
cheaper worker and follows its event stream incrementally, paying input tokens
only for new activity.

Two layers:

- **Messaging (in-repo pi peers).** A thin file-backed bus under `~/.pij/`:
  `pij list` / `pij send <id> "<text>"` / `pij tail <id> --since N` /
  `pij state <id>` / `pij path <id>`. Run it in-repo with `just pij <args>`
  (`justfile:95-98`) or as the bare `pij` CLI after `just install`.
- **The daemon switchboard (cross-harness).** A single-instance daemon
  (`pij daemon start|status|stop|kill`) spawns and binds sessions across
  heterogeneous harnesses (`pij spawn --harness pi|claude|copilot|codex`). The
  **transport seam** is harness-specific: Pi/OMP use their in-process receivers;
  new Copilot sessions use the [native user extension](copilot-native-extension.md);
  other harnesses use daemon-selected transports. Copilot's tmux pane remains a
  presentation and consent-observation surface, never its message transport.

→ Full depth: [`pij-rs-api.md`](pij-rs-api.md) (API and CLI reference, message + receipt protocol,
remote session control, the event stream, parent/worker workflow). The
daemon itself lives in
`crates/` (the `pij-rs` workspace).

## Putting it together

A typical delegated run: the orchestrator plans with **the-flow**, renders a
bounded packet with **flow-pair**, dispatches it to a worker over the **control
plane**, follows the worker's event stream incrementally, runs an independent
cross-model review, then accepts or fires a narrow fix packet — recording any
miss as a prompt-learning so the next run starts smarter.

## See also

- [`flow-pair.md`](flow-pair.md) — the delegation wrapper in depth.
- [`pij-rs-api.md`](pij-rs-api.md) — the messaging + control-plane API and CLI in depth.
- [`pij-telegram.md`](pij-telegram.md) — drive your sessions from Telegram.
- [`agent-feedback.md`](agent-feedback.md) — how retros/magic-wands feed back
  into the harness.
