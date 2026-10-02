---
name: pij
description: Route pij platform jobs — adopt a seat and wait (/pij ready), spawn & talk to tmux colleagues (OMP, Pi, Claude, Copilot and Codex peers), run flow-pair coder+reviewer delegation fleets, delegate single tasks, run pij agent packs (flowspace-search etc.), run an installed skill in a peer (/validate-v2, /thesis…) with the result pushed back, manage the pij daemon and tmux/registry hygiene. Use when the user says "pij ready", "adopt and wait", "spawn a peer/colleague/worker", "flow-pair", "delegate this", "run an agent", "have a peer run /X", "pij daemon", or any pij orchestration ask.
---

# /pij — the pij platform router

> **`/pij` (this skill) ≠ `pij` (the CLI binary).** The CLI on `$PATH` is the machine surface — verbs like `pij spawn`, `pij send`. This skill routes **jobs** to protocol modules that *use* that CLI; it never replaces it. When a route says run `pij <verb>`, that means the CLI, printed in a fenced block.

**Grammar**: `/pij [<route>] [args]` — no route = guided (detect, offer ONE route); with a route = direct (load ONLY that module).

## Two load paths

- **Guided** — `/pij`: read [`references/00-routing.md`](./references/00-routing.md) (signals + precedence), derive where you are from deterministic probes, offer exactly one route. A route hint that contradicts the signals is redirected, never blindly run.
- **Direct** — `/pij <route> [args]`: load `references/routes/<route>.md` and follow it. No engine, no detection.

**Progressive disclosure is the contract**: load exactly one route module per step; a module may lazily pull `00-routing.md` § Shared conventions when it cites one. Never read all modules up front.

## Registry

| route | job — "I want to…" | module |
|---|---|---|
| `ready` | adopt/register this seat, report ready, then wait without starting work | `references/routes/ready.md` |
| `pair` | run a phase with a coder + cross-model reviewer fleet, wrapping the-flow | `references/routes/pair.md` |
| `delegate` | hand ONE bounded task to ONE peer — no review cycle | `references/routes/delegate.md` |
| `agent` | run a packaged agent pack — fire-and-forget or resident | `references/routes/agent.md` |
| `skill` | run an installed skill (`/validate-v2`, `/thesis`…) in a peer, output pushed back | `references/routes/skill.md` |
| `peer` | spawn & talk to an ad-hoc colleague in any harness | `references/routes/peer.md` |
| `ops` | daemon health, registry & tmux hygiene — and **recovering a prime after a reboot** ("revive our prime") | `references/routes/ops.md` |
| `node` | work durable project/stream/dispatch truth, node states, adoption repair, and anomaly queries | `references/routes/node.md` |
| `prime` | govern many agents in one repo: one o-prime seat, streams below, platform-store facts and single-writer documents | `references/routes/prime.md` |
| `watch` | file-change subscription intent | *no route module — rs CLI unported: named refusal; see Unsupported status below* |

Module missing at its path → say so and stop. Never improvise a route from memory.

## CLI-verb coverage (every `pij` verb has a home)

| CLI verb | lives in |
|---|---|
| `spawn` `revive` | peer route — shim refuses `E-RS-UNPORTED`; use explicit native rs grammar, never legacy flags |
| `send` `compact-self` `adopt` `whoami` `state` `inbox` | peer route — rs; `send --fyi` holds text the seat's next action doesn't depend on until its next turn; verified external inbox register/wait supported, extension-owned receivers unchanged |
| `list` `sessions` | peer route — rs-only; list supports boolean --here plus exact harness/folder/parent/scope=local shim filters; sessions has no filters or legacy union |
| `tail` `tree` `link` | peer route — `E-RS-UNPORTED`; native tail is daemon events, not a transcript |
| `agent` (`list/run/spawn/show/new/check/eject/report`) | agent route — `E-RS-UNPORTED` |
| `daemon` `path` `telegram` | ops route — shim `E-RS-UNPORTED`; use documented native daemon lifecycle |
| `phonehome` `reap` `close` | ops / peer routes — rs binding, conservative reconciliation, self-or-parent tombstone |
| `models` | § Shared conventions C4 — `E-RS-UNPORTED` |
| `commit-trailers` | § Shared conventions C11 — narrow native CLI forwarding; trailer-only stdout, native stderr and exit status |
| `fleet-report` | native forward (plan 162) — writes a project's Context Tax folder (`report.json`, static page, tables); no daemon, store read read-only; `--anonymise` before sharing |
| `bg` (`create/list/tail/kill`) | § Shared conventions C7 — shipped rs detached jobs; result injected from `pij-bg` |
| `watch` `unwatch` `chore` `watchdog` `focus` | § Shared conventions / peer route — named `E-RS-UNPORTED`; no legacy escape |
| `orchestration` (`baton`/`prime`/`role`) | prime route — rs; destructive baton return/reclaim require observed `--lease-id` |
| `role` | node route — rs explicit assertion/unset, joined through RoleService |
| `project` `stream` `fence` `dispatch` `ack` `canary` `attest` `spine` `task` (`set/close`) `report` (`now/question/blocked/state/clear/verify`) `node` (`show`) `anomalies` | node route — rs governance records, receipts and projections |
| `decisions` `answer` | node route — rs durable questions/current-parent authority; terminal-only supersede |

`/pij prime` selects the skill route; `pij orchestration prime` invokes its CLI
primitive. `baton` is the other orchestration subcommand.

**Serve or refuse, never another generation.** The [stable API and Unsupported status](../../docs/how/pij-rs-api.md#unsupported-status) names every shim/native gap. Explicit `PIJ_DAEMON_GENERATION=legacy`, missing credentials or an absent daemon never enables fallback. Examples use rs contracts; unsupported leaves/flags refuse, not approximate. Raw `--json` preserves the original complete v2 success/refusal envelope. Canonical fixtures live under `crates/testkit/fixtures/golden/api/`; they are not fresh runtime or production/chainglass proof. The actual table above is the routing-enforcement input, not a second inventory copied into tests.

## Global invariants (every route)

1. **Never write** `.the-flow-state.json`, `the-flow.json`, `the-flow.md` — the-flow guided mode is their sole writer.
2. **Pointer delivery**: persist packets/large bodies to disk first; `pij send` carries a short path pointer, never a full body.
3. **Forbidden paths in every packet**: enumerate at minimum the three files above, plus any ledger dirs the route names.
4. **Persist before mutate**: roster/ledger records are written before the state they describe changes.
5. **Delivery-owned waiting**: native extensions receive pushed turns; verified non-tmux external peers block on `pij inbox --wait` after native admission (C1). Never start a competing extension receiver. Never sit in a `pij state` wait loop.
6. **Completion interrupt**: when a reusable/live coder completes or a reviewer returns a verdict, compact that peer as the first tool action, then continue immediately. § C3 owns the lifecycle boundary and command contract.
7. **Ownership-aware retirement**: `pij close <id>` requires self or current recorded parent and tombstones only; it never kills a process/pane and has no `--force` bypass. Separate teardown remains owner-authorized.
8. **Token-lean output & wire discipline**: cite conventions instead of restating them; say only what's needed. Every agent-to-agent message follows **C10 — Wire discipline** (`references/00-routing.md` § Shared conventions) — cite it, never restate it.
9. **Non-blocking questions**: never `ask_user_question` or any modal question UI — ask inline through the active delivery channel, persist the pending decision, block only dependent work.
10. **Questions stay with their context owner**: whoever needs the answer asks the human directly; parents receive a pointer and never proxy. Doctrine for 9–10: `references/prime/protocol.md` § Human rulings.
11. **Isolation removes edit-time serialization, not convergence-time serialization**: work confined to a verified stream worktree/branch is notify-only under a recorded descriptive fence; synchronize at converging histories or shared mutable resources. Trigger matrix: `references/prime/rituals/batons.md`.
12. **Report at both edges of work — if you owe a card**: run `pij report now "<did>" "<next>"` at the start and finish of each unit. PMs, stream orchestrators and primes owe this cadence (2026-07-31 ruling at the resolved government root); a prime reports its own governance altitude, not copied stream work. Long units report meaningful boundaries, not running commentary. Supervisors own subordinate freshness: query `pij anomalies --json` **unscoped**, because `status-stale` is node-keyed and project/here filters can hide it, then follow the actual remediation until the card changes. A nudge is a backstop, never the trigger. PA supervision remains a bootstrap obligation, but its unported watchdog/link mechanisms must be reported honestly (bootstrap §5), not declared active through legacy sidecars or assumed production UI behavior.

## Aliases (read-time — never a second implementation)

| typed / intent | resolves to |
|---|---|
| "adopt your window and wait" / "report ready; prime will contact shortly" | `/pij ready`; extension-owned seats reply `Ready.` and STOP — run NO pij identity command (no adopt, no inbox register); C1 identifies each harness's receiver |
| `/flow-pair start\|dispatch\|observe\|review\|fix\|accept\|ledger\|learn …` | `/pij pair …` (same args) |
| "spawn a worker / colleague / peer" | `/pij peer` |
| "run flowspace search" / "ask an agent" | `/pij agent` |
| "have a peer run /X on …" / "run a skill in a peer" | `/pij skill` |
| "stand up an o-prime" / "govern this repo" | `/pij prime` |

## References

- [`references/00-routing.md`](./references/00-routing.md) — detection signals, precedence, and § Shared conventions (C1 harness/delivery modes · C2 canary-verify · C3 compact discipline · C4 model discovery · C5 placement & split-cap · C6 daemon restart rule · C7 push-vs-pull waiting).
- `references/routes/<route>.md` — one contract-bound module per registry row.
