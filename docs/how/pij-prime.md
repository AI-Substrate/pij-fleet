# pij prime — multi-agent repository governance

`/pij prime` is the progressive-disclosure entry point for running one o-prime
seat above multiple plan-owning stream orchestrators. It is a skill route, not a
`pij` CLI verb; the route composes the existing spawn, adopt, tree, link, send,
state, close, daemon, and harness-flow surfaces.

## Start here

Invoke:

```text
/pij prime
```

The route runs deterministic role triage and points to one next file:

- [prime route](../../skills/pij/references/routes/prime.md)
- [o-prime orient](../../skills/pij/references/prime/orient-oprime.md)
- [stream orchestrator landing](../../skills/pij/references/prime/orchestrator.md)

Load only the selected rung. The consuming repo's stream path also reads its
portable orient, generated `.harness/government/orient-local.md`, and item brief through
the landing module.

## Operator index

| Need | Canonical page |
|---|---|
| Enter the stream-orchestrator role and journey | [orchestrator landing](../../skills/pij/references/prime/orchestrator.md) |
| Stand up a government and seat | [bootstrap ritual](../../skills/pij/references/prime/rituals/bootstrap.md) |
| Spawn/adopt, canary, brief, or tear down a stream | [kickoff ritual](../../skills/pij/references/prime/rituals/kickoff.md) |
| Serialize an exclusive resource | [baton ritual](../../skills/pij/references/prime/rituals/batons.md) |
| File, verify, relay, or digest evidence | [reports ritual](../../skills/pij/references/prime/rituals/reports.md) |
| Inspect the full portable contract | [protocol](../../skills/pij/references/prime/protocol.md) |
| Create government files | [templates](../../skills/pij/references/prime/templates/) |
| See labeled historical evidence | [exemplars](../../skills/pij/references/prime/exemplars/) |

The ritual pages own procedure. This guide intentionally does not duplicate it.

## Stream lifecycle

The o-prime allocates and records one worktree/branch/base per stream, then runs
peer spawn from that worktree into a new orchestrator window. `/pij prime` lands
the stream on its role module; after thesis, preamble, guided Builder planning,
and cold validation, it waits for the human's fleet configuration.

Spawned streams record their caller as structural parent automatically and the
kickoff verifies that edge with `pij tree <id> --json`. A human-spawned/adopted
stream is canaried first, then linked with
`pij link <id> --parent <o-prime-id> --json` before its brief pointer is sent.
`parentId` controls hierarchy; `spawnedBy` remains close authorization.

Implementation runs through `/pij pair`: coder and separate reviewer are splits
inside the orchestrator window and inherit the stream cwd. Approved work lands
through `/builder 8 ship` (confirm-gated push and PR, watched CI, optional merge).
Remove the worktree only after merge or an explicit abandonment ruling. Batons
remain for timing/runtime purity, external resources, merge coordination, and
shared-tree fallback.

## Registry designation

The o-prime seat is marked directly on its pij session descriptor:

```bash
pij orchestration prime set [<id>] [--json]
pij orchestration prime retire [<id>] [--json]
pij orchestration prime unset [<id>] [--json]
pij list --prime [--here] [--json]
pij tree --global --all [--json]
```

Without `<id>`, every transition requires exact self-resolution through
`PIJ_SESSION_ID`, the registered tmux pane, or a lone local descriptor.
Ambiguity returns `E-AMBIG`; an unknown explicit target returns `E-NOID`.
Neither error writes anything, and pij never substitutes the baton's
`operator` actor fallback.

Transitions are mutually exclusive and idempotent:

| Command | Resulting descriptor | Use |
|---|---|---|
| `set` | `prime:true`, `oldPrime:false` | Designate a current seat. |
| `retire` | `prime:false`, `oldPrime:true` | Preserve a completed/outgoing seat in history. |
| `unset` | `prime:false`, `oldPrime:false` | Clear both designations. |

For compatibility, set/unset JSON remains `{id,prime,changed}`. Retire JSON is
additive: `{id,prime:false,oldPrime:true,changed}`. Multiple current primes are
allowed during intentional handover overlap. `pij list --prime` is current-only
and never treats `oldPrime` as an active-seat signal. Ordinary `pij list` and
`pij tree` mark current seats `P` and retired seats `O`; audit history with
`pij tree --global --all --json`.

The marker survives reload, restart, durable native-identity snapshots, and
reattachment. It is mutable external registry state: before daemon writes, the
latest persisted `true` or `false` overrides the daemon's stale tick snapshot.
This differs from append-only external fields such as `reportedAt`.

Designation is an honor-system coordination signal, not an ACL or uniqueness
lock. Bootstrap marks the proved seat before government creation. Handover
marks the incoming seat before writer-line transfer, keeps the bounded two-prime
overlap through the outgoing seat's final relay, then retires the outgoing seat.
The retired descriptor remains visible as old-prime history; do not use `unset`
as a handover substitute.

## Distribution and local state

Portable levers and the prime-flow schema ship under
`skills/pij/references/prime/`. A consuming repo generates its own local orient
and prime-flow instance inside the repo; it does not fork the portable levers.

Run `just pij-skill-check` after changing the route or payload. The gate checks
registry parity, pointer integrity, the orchestrator journey, worktree lifecycle,
portability, sibling-blindness, expected evidence, and line budgets.
