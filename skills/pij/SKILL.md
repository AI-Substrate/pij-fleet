---
name: pij
description: Route pij platform jobs — adopt a seat and wait (/pij ready), spawn & talk to tmux colleagues (OMP, Pi, Claude, Copilot and Codex peers), run a coder + cross-model reviewer pair (/builder implement + review in separate seats), delegate single tasks, run an installed skill in a peer (/validate-v2, /thesis…) with the result pushed back, manage the pij daemon, recover after a reboot, and tmux/registry hygiene. Use when the user says "pij ready", "adopt and wait", "spawn a peer/colleague/worker", "flow-pair", "delegate this", "have a peer run /X", "pij daemon", or any pij orchestration ask.
---

# /pij — the pij platform router

> **`/pij` (this skill) ≠ `pij` (the CLI binary).** The CLI on `$PATH` is the machine surface — verbs like `pij send`, `pij list`. This skill routes **jobs** to protocol modules that *use* that CLI; it never replaces it. When a route says run `pij <verb>`, that means the CLI, printed in a fenced block.

**Grammar**: `/pij [<route>] [args]` — no route = guided (detect, offer ONE route); with a route = direct (load ONLY that module).

## Two load paths

- **Guided** — `/pij`: read [`references/00-routing.md`](./references/00-routing.md) (signals + precedence), derive where you are from deterministic probes, offer exactly one route. A route hint that contradicts the signals is redirected, never blindly run.
- **Direct** — `/pij <route> [args]`: load `references/routes/<route>.md` and follow it. No engine, no detection.

**Progressive disclosure is the contract**: load exactly one route module per step; a module may lazily pull `00-routing.md` § Shared conventions when it cites one. Never read all modules up front.

## Registry

| route | job — "I want to…" | module |
|---|---|---|
| `ready` | adopt/register this seat, report ready, then wait without starting work | `references/routes/ready.md` |
| `pair` | run a phase with a coder seat (`/builder 6 implement`) + a cross-model reviewer seat (`/builder 7 review`) | `references/routes/pair.md` |
| `delegate` | hand ONE bounded task to ONE peer — no review cycle | `references/routes/delegate.md` |
| `agent` | packaged agent packs — `pij agent` is unported: named refusal, ordinary delegation instead | `references/routes/agent.md` |
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
| `spawn` `revive` | peer route — shim refuses `E-RS-UNPORTED`; use native `pij-rs spawn` / `pij-rs revive` grammar, never legacy flags |
| `send` `compact-self` `adopt` `whoami` `state` `inbox` (`check`/`register`; native `pij-rs inbox release`) | peer route — rs; `send --fyi` holds text the seat's next action doesn't depend on; `send --force --reason` wakes a cold seat past `E-RS-COLD-WAKE` (§ C1); verified external inbox register/wait supported, extension-owned receivers unchanged |
| `list` `sessions` | peer route — rs-only; list supports boolean --here plus exact harness/folder/parent/scope=local shim filters; sessions has no filters or legacy union |
| `tail` | peer route — shim `E-RS-UNPORTED`; native `pij-rs tail` is daemon events, not a transcript |
| `link` | peer route — rs; `link <seat> --role <pm, worker or pa>` takes a seat with no live parent or re-roles your own child |
| `daemon` `telegram` | ops route — shim `E-RS-UNPORTED`; native `pij-rs daemon` / `daemon bounce` (§ C6) and `pij-rs sidecar telegram` (native table below) |
| `phonehome` `reap` `close` | ops / peer routes — rs binding, conservative reconciliation, self-or-parent tombstone |
| `commit-trailers` | § Shared conventions C11 — narrow native CLI forwarding; trailer-only stdout, native stderr and exit status |
| `fleet-report` | native forward (plan 162) — writes a project's Context Tax folder (`report.json`, static page, tables); no daemon, store read read-only; `--anonymise` before sharing |
| `bg` (`create/list/tail/kill/emit`) | § Shared conventions C7 — shipped rs detached jobs and event sources; result injected from `pij-bg` |
| `watchdog` | § C9 — rs; `on\|off\|status [<seat>] [--every 30m]`; any seat may switch any seat; PAs always on |
| `orchestration` (`baton`/`prime`/`role`) | prime route — rs; destructive baton return/reclaim require observed `--lease-id` |
| `role` | node route — rs explicit assertion/unset, joined through RoleService |
| `project` `stream` `fence` `dispatch` `ack` `canary` `attest` `spine` `task` (`set/close`) `report` (`now/question/blocked/state/clear/verify`) `node` (`show`) `anomalies` | node route — rs governance records, receipts and projections |
| `decisions` `answer` | node route — rs durable questions/current-parent authority; terminal-only supersede |
| `agent` (`list/run/spawn/show/new/check/eject/report`) `path` `models` `tree` `watch` `unwatch` `chore` `focus` | named `E-RS-UNPORTED` refusals, no native verb, no legacy escape. agent → agent route; models → § C4; tree → read `pij node show <seat>` |

`pij generation [verb]` is a shim-local, read-only routing diagnostic, not an rs route.

`/pij prime` selects the skill route; `pij orchestration prime` invokes its CLI
primitive. `baton` is the other orchestration subcommand.

## Native-only verbs (`pij-rs`, no shim route)

| verb | use |
|---|---|
| `ping` | daemon health (`pij-rs ping --json`; ops route) |
| `peers` (`check`/`new-key`) | machine pairing for federation; then `pij-rs send --to <seat>@<machine>` ([federation](../../docs/how/pij-federation.md)) |
| `doctor` (`claude-inbound`/`claude-hook`/`claude-statusline`) | read-only audit of every Claude home's pij wiring (ops route) |
| `fyi-read` | read in full the FYIs one claim delivered: `pij-rs fyi-read --seat <s> --claimed-at <ms>`, the command an FYI digest names |
| `sidecar` (`telegram`) | `pij-rs sidecar telegram --body\|--body-file` messages the operator and binds this seat as the reply target ([telegram](../../docs/how/pij-telegram.md)); the other leaves are queue internals: use `pij bg` |
| `register` `activity` `fyi-claim` | runtime and hook plumbing (extension registration, turn-boundary busy/idle, typed-turn FYI claim); agents use `pij adopt` / `pij inbox register` (§ C1) |

**Serve or refuse, never another generation.** The [stable API and Unsupported status](../../docs/how/pij-rs-api.md#unsupported-status) names every shim/native gap. Explicit `PIJ_DAEMON_GENERATION=legacy`, missing credentials or an absent daemon never enables fallback. Examples use rs contracts; unsupported leaves/flags refuse, not approximate. Raw `--json` preserves the original complete v2 success/refusal envelope. Canonical fixtures live under `crates/testkit/fixtures/golden/api/`; they are not fresh runtime or production/chainglass proof. Both tables are gated two-sided against the producers, never a copied list: `generation-skill-coverage.test.ts` (shim `RS_ROUTE_TABLE`, both directions, leaves included) and `crates/cli/tests/skill_verb_coverage.rs` (built `pij-rs --help`).

## Global invariants (every route)

1. **Never hand-write** `the-flow.json` or `the-flow.md` — the `harness flow` CLI is their only writer.
2. **Pointer delivery**: persist packets/large bodies to disk first; `pij send` carries a short path pointer, never a full body.
3. **Forbidden paths in every packet**: enumerate at minimum the two files above, plus any ledger dirs the route names.
4. **Persist before mutate**: roster/ledger records are written before the state they describe changes.
5. **Delivery-owned waiting**: native extensions receive pushed turns; verified non-tmux external peers block on `pij inbox --wait` after native admission (C1). Never start a competing extension receiver. Never sit in a `pij state` wait loop.
6. **Completion interrupt**: when a reusable/live coder completes or a reviewer returns a verdict, compact that peer as the first tool action, then continue immediately. § C3 owns the lifecycle boundary and command contract.
7. **Ownership-aware retirement**: `pij close <id>` requires self or current recorded parent and tombstones only; it never kills a process/pane and has no `--force` bypass. Separate teardown remains owner-authorized.
8. **Token-lean output & wire discipline**: cite conventions instead of restating them; say only what's needed. Every agent-to-agent message follows **C10 — Wire discipline** (`references/00-routing.md` § Shared conventions) — cite it, never restate it.
9. **Non-blocking questions**: never `ask_user_question` or any modal question UI — ask inline through the active delivery channel, persist the pending decision, block only dependent work.
10. **Questions stay with their context owner**: whoever needs the answer asks the human directly; parents receive a pointer and never proxy. Doctrine for 9–10: `references/prime/protocol.md` § Human rulings.
11. **Isolation removes edit-time serialization, not convergence-time serialization**: work confined to a verified stream worktree/branch is notify-only under a recorded descriptive fence; synchronize at converging histories or shared mutable resources. Trigger matrix: `references/prime/rituals/batons.md`.
12. **Report at both edges of work — if you owe a card**: run `pij report now "<did>" "<next>"` at the start and finish of each unit. PMs, stream orchestrators and primes owe this cadence (Jordan, 2026-07-31; [orient-oprime](references/prime/orient-oprime.md) duty 7); a prime reports its own governance altitude, not copied stream work. Long units report meaningful boundaries, not running commentary. Supervisors own subordinate freshness: query `pij anomalies --json` **unscoped**, because `status-stale` is node-keyed and project/here filters can hide it, then follow the actual remediation until the card changes — for seats that are working or owe a card; a stale card on an idle or done seat is not a nudge trigger (protocol § Fleet messaging). A nudge is a backstop, never the trigger. A PA is a bootstrap obligation (bootstrap §5): the daemon's watchdog always serves PAs and feeds them the fleet's state; other seats opt in (§ C9).

## Aliases (read-time — never a second implementation)

| typed / intent | resolves to |
|---|---|
| "adopt your window and wait" / "report ready; prime will contact shortly" | `/pij ready`; extension-owned seats reply `Ready.` and STOP — run NO pij identity command (no adopt, no inbox register); C1 identifies each harness's receiver |
| `/flow-pair start\|dispatch\|observe\|review\|fix\|accept\|ledger\|learn …` | `/pij pair …` (same args) |
| "spawn a worker / colleague / peer" | `/pij peer` |
| "run flowspace search" / "ask an agent" | `/pij agent` (named refusal; offers ordinary delegation) |
| "have a peer run /X on …" / "run a skill in a peer" | `/pij skill` |
| "stand up an o-prime" / "govern this repo" | `/pij prime` |

## References

- [`references/00-routing.md`](./references/00-routing.md) — detection signals, precedence, and § Shared conventions (C1 harness/delivery modes · C2 canary-verify · C3 compact discipline · C4 model discovery · C5 placement & naming · C6 daemon restart rule · C7 push-vs-pull waiting and `pij bg` · C8 death and revive interpretation · C9 watchdog · C10 wire discipline · C11 commit attribution · C12 niced heavy work).
- `references/routes/<route>.md` — one contract-bound module per registry row.
