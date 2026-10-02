# pij extension — agent rules

> Domain: **`pij-messaging`** (see `docs/domains/pij-messaging/domain.md`).
> First inhabitant of the domain. Two live pi sessions discover each other,
> exchange fire-and-forget self-identifying messages + remote commands, and
> observe each other's work via a per-session event stream + state/liveness.

## Layout (T2, hexagonal)

```
core/        pure, pi-free domain logic + port interfaces (imports NOTHING from @earendil-works)
  types.ts ports.ts seq.ts events.ts state.ts commands.ts discovery.ts message.ts receipts.ts
adapters/    port implementations
  fakes.ts          in-memory all ports (tests target these — Pattern P8)
  pi-runtime.ts     (Phase 3) the ONLY file that may import @earendil-works
index.ts     extension wiring (Phase-3 stub today)
smoke.ts     Driver SDK smoke (Phase 5)
```

## Hard rules (do not violate)

1. **`core/` is pi-free.** Nothing under `core/` imports from `@earendil-works/*`
   (Patterns P2/P9). Only `adapters/pi-runtime.ts` (Phase 3) may.
2. **Tagged-union returns, not throws** (`Result<T>` — Pattern P4). Error codes:
   `E-NOID/E-SELF/E-CMD/E-DEAD/E-NOREG/E-ARG/E-AMBIG`.
3. **Constants live next to the data they constrain** (`STALE_AFTER_MS`,
   `ALLOWED_COMMANDS`) — Pattern P5.
4. **Side effects via ports + constructor DI** (Pattern P3). No global mutable state.
5. **Tests target `core/` + `adapters/fakes.ts`** (Pattern P8), not the wiring.
6. `.js` extension on all relative imports (NodeNext/ESM — Pattern P7).
7. **Every event carries `seq` (strictly monotonic) + ISO-8601 `timestamp`** so a
   reader computes age from the stream alone. `seq` recovers from
   `EventLogPort.lastSeq()` after `/reload` (finding 04).

## Phase status

- **Phase 1 (done):** pi-free core + 5 ports + fake adapters + 50 unit tests.
- **Phase 2:** fs adapters (registry/event-log/channel).
- **Phase 3:** extension wiring — registry write, `PIJ_SESSION_ID`/`PIJ_ROLE`
  env export (finding 07), boot self-announce, delivery injector, event capture,
  delivery-receipt emission (finding 08). Replaces the `index.ts` stub.
- **Phase 4:** `pij` CLI (whoami/list/send/tail/state/path).
- **Phase 5:** two-window smoke + CI + docs.

## Acceptance / gates

Use the repo's `just` gates — **not** the generated scaffold commands:

```
just typecheck         # tsc --noEmit (noUncheckedIndexedAccess is ON — guard index access)
just lint              # Biome (scratch/ is excluded)
npx vitest run .pi/extensions/pij    # the core + fakes specs
just self-check        # before declaring any task done
```

## Self-announce snippet (AC-2)

At session start the extension injects a runtime announce (the `announceText`
in `core/message.ts`) telling the session its id + how to use pij. For a fresh
agent reading a project's `AGENTS.md` *before* that fires, paste this static
block into the project AGENTS.md:

```md
## pij — talk to peer pi sessions

If another pi session is running in this repo, you can message and observe it:
- **Send / control: use the `pij_send` tool** — `pij_send { to, message }` to message a peer (your id is stamped automatically), `pij_send { to, message, fyi: true }` to hold it until their next turn instead of waking them, only when their next action doesn't depend on it ([the FYI rule](../../../skills/pij/references/routes/peer.md#converse)), or `pij_send { to, command: "compact"|"new"|"reload" }` to run an allow-listed control command. Do NOT shell out to `pij send` for this.
- `pij list --here` — discover peer sessions in this folder (★ = you)
- `pij tail <id> --since N` — read a peer's new events (cheap incremental review)
- `pij state <id>` — a peer's working/idle + liveness + latest-event age
- `pij path <id> --events` — the peer's events.ndjson path for direct reading

You are stamped with a stable id at boot (`pij whoami`). Full guide: docs/how/pij.md
```

`just install` runs `npm link`, so bare `pij` resolves on PATH exactly as the
announce promises; in-repo, `just pij …` works without a global link.
