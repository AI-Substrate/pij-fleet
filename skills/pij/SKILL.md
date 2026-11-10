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
| `community` | run a Pij Emergent Community (PEC): roles built from the work's needs and reshaped as it changes, assignments, role sends and requests, stall checks, outputs, memory, what the daemon tells the prime and PA, closing seats | `references/routes/community.md` |
| `watch` | file-change subscription intent | *no route module — rs CLI unported: named refusal; see Unsupported status below* |

Module missing at its path → say so and stop. Never improvise a route from memory.

## CLI-verb coverage (every `pij` verb has a home)

| CLI verb | lives in |
|---|---|
| `spawn` `revive` | peer route — shim refuses `E-RS-UNPORTED`; use native `pij-rs spawn` / `pij-rs revive` grammar, never legacy flags |
| `send` `compact-self` `adopt` `whoami` `state` `inbox` (`check`/`register`; native `pij-rs inbox release`) | peer route — rs; before `send --fyi`, ask: does the recipient have to do anything because of this message? If yes (step done, your turn; hand-off; ready; verdict; blocker; question; decision) it is a normal send even when you need no reply; `--fyi` is only for information the recipient does not have to act on, and it wakes no one; a normal send lands inside a turn the recipient is already in (C31: OMP/Pi, socket Claude, native Copilot unless `pij steer off`); a stop or cancel order to a working seat is a normal send, never `--fyi`, which arrives only after the action it was meant to stop (F87); `send --force --reason` wakes a cold seat past `E-RS-COLD-WAKE` (§ C1); `send role:<id>[/<instance>]` goes through the project's directory: one live holder delivers, none queues and wakes the PA, several refuse with a runnable `next`; `--reply wake|fyi|none` and `--limit <d>` go only on a `role:` send (a seat send refuses them, `E-RS-ROLE-ONLY`); an assign's receipt lists what it drained; verified external inbox register/wait supported, extension-owned receivers unchanged |
| `list` `sessions` | peer route — rs-only; list supports boolean --here plus exact harness/folder/parent/scope=local shim filters; sessions has no filters or legacy union |
| `tail` | peer route — shim `E-RS-UNPORTED`; native `pij-rs tail` is daemon events, not a transcript |
| `link` | peer route — rs; `link <seat> [--parent <you>] --role <pm\|worker\|pa>` takes a seat with no live parent or re-roles your own child |
| `daemon` `telegram` | ops route — shim `E-RS-UNPORTED`; native `pij-rs daemon` / `daemon bounce` (§ C6) and `pij-rs sidecar telegram` (native table below) |
| `phonehome` `reap` `close` | ops / peer routes — rs binding, conservative reconciliation, self-or-parent tombstone |
| `commit-trailers` | § Shared conventions C11 — narrow native CLI forwarding; trailer-only stdout, native stderr and exit status |
| `fleet-report` | native forward (plan 162) — writes a project's Context Tax folder (`report.json`, static page, tables); no daemon, store read read-only; `--anonymise` before sharing |
| `bg` (`create/list/tail/kill/emit`) | § Shared conventions C7 — shipped rs detached jobs and event sources; result injected from `pij-bg` |
| `watchdog` | § C9 — rs; `on\|off\|status [<seat>] [--every 30m]`; any seat may switch any seat; PAs always on |
| `steer` | § C1 — rs (plan 214, [ruling C31](../../docs/how/pij-rs-api.md#mid-turn-steering-plan-214-ruling-c31)); `on\|off\|status [<seat>]`; a native Copilot seat steers normal sends into a turn its pij message started, as OMP/Pi do (FYIs still wait); on by default; any seat may switch any seat; off restores only the old post-ack barrier (after a pij message starts a turn, the next waits for it to end; a first message into a turn the seat started itself still goes in) |
| `orchestration` (`baton`/`prime`/`role`) | prime route — rs; destructive baton return/reclaim require observed `--lease-id` |
| `role` | node route — rs explicit assertion/unset, joined through RoleService |
| `project` `stream` `fence` `dispatch` `ack` `canary` `attest` `spine` `task` (`set/close`) `report` (`now/question/blocked/state/clear/verify`) `node` (`show`) `anomalies` | node route — rs governance records, receipts and projections |
| `directory` (`add` `edit` `retire` `merge` `assign` `release` `lineage` `external` `rebuild` `routes` `templates` `template-diff` `memory`) | node route; rs community writer (plans 190, 194, 195, 201, 202, 203, 206, 208, 215): bare `directory` (`--suggest` lists every core and security suggestion in full) is the prime's operating picture: the designated prime, each role's holders (seat live or not), waiting requests, latest output and memory freshness, template drift, next actions with runnable commands (no live holder while requests wait, two live holders on a one-holder role, memory not written across the last two closes), prompts for missing core roles, security, roles a workstream step needs and recurring paths no role owns, and a boundary review (split a seat carrying two domains, retire or merge a role with nothing to remember); each recommendation names its [ruling C28](references/routes/community.md#when-to-recommend-a-role-c28) signal, and none creates anything; a cold role with memory shows how to call on it (PEC builds lasting knowledge of its problem space, [ruling C30](references/routes/community.md); templates are examples; a role is not a seat); `directory add <id> --why <text>` writes or adopts `<government-root>/community/roles/<id>/role.dd.json` (`--from template:<name>[@v<n>]` copies a shipped template, `--from file:<brief>` is `--brief`; `--batch <dir>` all or nothing); `templates` lists them (`--show <name>[@vN]` prints one); `external add <seat> --from-project <slug> --why <note> [--role <id>]` notes a seat borrowed from another project; `template-diff <id>` shows the upstream change and applies nothing; `edit <id> --set <field>=<value> --why` changes a role through the writer, `retire <role>` and `merge <from> --into <to>` reshape the community and move nothing (plan 206, req-28): requests still queued for the role refuse them, listed by id, requester and age, unless `--drop-queued` withdraws them and tells each requester once (a request mid-delivery refuses both until it settles), a retired role refuses sends and leaves `ROUTES.md`, and printed repair commands quote each word; `assign`/`release <role>[/<instance>] <seat> --why` keep `holders.dd.json` lineage and set seat roles (C1); `pij close` ends a seat's holdings; `rebuild` restores the store from the branch; `assign --batch <file>` and `release --batch <file>` many in one commit, all or nothing; `pij delivered <pr>` ends the PR's reviewer, security, performance and coder holdings begun before its merge; `lineage add` (prime) records a closed seat's past holding (`--batch <file>` many in one commit); `routes` prints the routing table (`community/ROUTES.md`, rewritten with every role write and repaired by any directory commit that finds it stale; held FYIs on assign, compact and revive, changed rows to the prime; `--write --why` fixes a stale file); `memory add <role> --title --hook [--tags] --body\|--body-file` writes one note and regenerates `memory/INDEX.md`, `memory <role> [--note <slug>]` reads, nothing is deleted; before a compact or close of a role holder pij asks it to write its memory and holds the action until that turn ends (5 min bound); `--help` needs no daemon |
| `output` `outputs` | node route; rs community writer (plan 200): `output verdict\|deliverable\|note\|handover --role <id> --subject <address>` writes one pij/output document (a verdict needs `--head <sha>` and `--result pass\|pass-with-notes\|block`; `--note-owed`, `--evidence <json>`, `--link`, `--body\|--body-file`); `--answers <msg_id>` (only from the request's recorded holder, a live holder of its role or the project's prime; else `E-RS-OUTPUT-NOT-HOLDER`) marks the role request answered and tells its requester in its reply mode (block or failed always wakes); an unrequested output goes FYI to the seat above; a non-holder writer may publish, flagged; `outputs --subject pr:N --head <sha>` answers only for that exact head, `outputs --role <id>` lists a role's outputs; `--help` needs no daemon |
| `claim` (`release`) `claims` `delivered` | node route; rs numbers and delivery (plan 196): `claim <kind> --for <subject>` takes the next number for this project and kind with your seat on record (released when the seat closes or the number merges; kinds include `workstream`, and `roadmap`, its old name, claims the same sequence); `claims` lists open and released; the merger runs `delivered <pr>` once ([ruling C23](../../docs/how/pij-rs-api.md#number-claims-and-delivery-at-merge-plan-196)): it records the PR body's `Delivers:` addresses, posts on the PM's card (the `pm-NNN` holder of the workstream the steps belong to) and prints the ddocs edits, never pushing; `--help` needs no daemon |
| `requests` (`withdraw`, `close`) | node route; rs role queue (plan 212): `requests [--role <id>] [--state queued\|delivering\|delivered\|closed]` lists the project's role requests, newest first, without bodies (no `--state`: queued and delivering); `requests withdraw <id> --why <text>` closes a queued one for its requester, the PA or the prime, so no assign delivers it and the PA digest drops it; `requests close <id> [--why] [--quiet]` closes a delivered one that was answered or abandoned (also its holder), which stops its time limit, and tells the requester by FYI unless `--quiet`; `--help` needs no daemon |
| `decisions` `answer` | node route — rs durable questions/current-parent authority; terminal-only supersede |
| `agent` (`list/run/spawn/show/new/check/eject/report`) `path` `models` `tree` `watch` `unwatch` `chore` `focus` | named `E-RS-UNPORTED` refusals, no native verb, no legacy escape. agent → agent route; models → § C4; tree → read `pij node show <seat>` |

`pij generation [verb]` is a shim-local, read-only routing diagnostic, not an rs route.

The shim hands `directory`, `claim`, `claims`, `delivered` and `requests`, and a `send` to `role:<id>`, with `--to` or `--help`, to `pij-rs` unchanged: the same output, help and exit code ([shim forwarding](../../docs/how/pij-rs-api.md#shim-forwarding-plan-213), plan 213).

`/pij prime` selects the skill route; `pij orchestration prime` invokes its CLI
primitive. `baton` is the other orchestration subcommand.

## Native-only verbs (`pij-rs`, no shim route)

| verb | use |
|---|---|
| `ping` | daemon health (`pij-rs ping --json`; ops route) |
| `peers` (`check`/`new-key`) | machine pairing for federation; then `pij-rs send --to <seat>@<machine>` ([federation](../../docs/how/pij-federation.md)) |
| `doctor` (`claude-inbound`/`claude-hook`/`claude-statusline`/`claude-hooks`) | read-only audit of every Claude home's pij wiring (ops route); `claude-hooks` reports every pij-managed hook missing or stale with its fix, and `just doctor` runs it |
| `claude-hooks` (`--remove`) | takes every pij-managed hook out of every Claude home and leaves other settings alone; `just unlink` runs it, and a daemon that starts again puts them back |
| `fyi-read` | read in full the FYIs one claim delivered: `pij-rs fyi-read --seat <s> --claimed-at <ms>`, the command an FYI digest names |
| `sidecar` (`telegram`) | `pij-rs sidecar telegram --body\|--body-file` messages the operator and binds this seat as the reply target ([telegram](../../docs/how/pij-telegram.md)); the other leaves are queue internals: use `pij bg` |
| `register` `activity` `fyi-claim` `hook` | runtime and hook plumbing (extension registration, turn-boundary busy/idle, typed-turn FYI claim, `hook claude-stop` running a Claude reply's closing pij block); agents use `pij adopt` / `pij inbox register` (§ C1) |

**Serve or refuse, never another generation.** The [stable API and Unsupported status](../../docs/how/pij-rs-api.md#unsupported-status) names every shim/native gap. Explicit `PIJ_DAEMON_GENERATION=legacy`, missing credentials or an absent daemon never enables fallback. Examples use rs contracts; unsupported leaves/flags refuse, not approximate. Raw `--json` preserves the original complete v2 success/refusal envelope. Canonical fixtures live under `crates/testkit/fixtures/golden/api/`; they are not fresh runtime or production/chainglass proof. Both tables are gated two-sided against the producers, never a copied list: `generation-skill-coverage.test.ts` (shim `RS_ROUTE_TABLE`, both directions, leaves included) and `crates/cli/tests/skill_verb_coverage.rs` (built `pij-rs --help`).

## Closing commands: the end-of-turn pij block (plan 226)

When your last actions are fire-and-forget sends or outputs, end your reply with a pij block instead of tool calls. As the reply's last thing, in a fence tagged `pij`, they cost no extra model call:

```pij
send pij-steady-otter "MERGED #210, daemon bounced"
output verdict --role reviewer --subject pr:211 --head abc1234 --result pass
```

- One command per line, written like the CLI without the leading `pij`, shell-style quoting, nothing expanded. Verbs: `send`, `output`, `requests close`. A `send` body is inline and quoted (a block's `send` takes no `--body-file`); an `output` may name a body file, never stdin (`--body-file -`).
- `send --fyi` in a block follows the FYI rule: only what the recipient need not act on. Hand-offs, ready, verdicts, blockers, questions and decisions are normal sends.
- Only a block that ends the reply runs: nothing after its closing fence but whitespace. A block quoted mid-reply, followed by prose, or written before a tool call never runs.
- Every line is checked first, and if any is wrong nothing runs. Then they run in order, as you, through the same paths as the CLI, and stop at the first refusal.
- Success shows the human one line (`pij ✓ 2 sent`). A failure gives you one follow-up turn listing what ran (do not resend it), what was refused and why, and what did not run. There is no second follow-up.
- Use a tool call instead when you need the answer before you go on: a reply you will wait on, a receipt you must quote, a refusal you must react to. A long packet also goes by tool call with `--body-file`, never as a block line (invariant 2, pointer delivery: persist it, then send its path).
- A seat can't send to itself: a self-send line is refused, and you get the follow-up turn.
- Claude and OMP show the human one line in place of the block; Copilot shows the block itself.

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
| "set up a community" / "PEC" / "add a role" / "assign a seat to a role" | `/pij community` |

## References

- [`references/00-routing.md`](./references/00-routing.md) — detection signals, precedence, and § Shared conventions (C1 harness/delivery modes · C2 canary-verify · C3 compact discipline · C4 model discovery · C5 placement & naming · C6 daemon restart rule · C7 push-vs-pull waiting and `pij bg` · C8 death and revive interpretation · C9 watchdog · C10 wire discipline · C11 commit attribution · C12 niced heavy work).
- `references/routes/<route>.md` — one contract-bound module per registry row.
