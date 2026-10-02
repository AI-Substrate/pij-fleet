# How: flow-pair

> **Front door removed.** Pairing is invoked via **`/pij pair`** — the pij router skill
> (`skills/pij/`). The old `/flow-pair` **skill** has been removed (`skills/flow-pair/SKILL.md`
> deleted, uninstalled from `~/.agents/skills` + `~/.claude/skills`); saying "flow-pair" still
> routes, because `/pij`'s own description carries those trigger phrases. **The engine described
> below — the `flow-pair` CLI (`skills/flow-pair/lib`), `.flow-pair/` ledger, schemas, and
> prompt-lab — is unchanged and still drives `/pij pair`.**

`flow-pair` wraps `the-flow` SDD pipeline in a **three-session orchestrator/worker/reviewer
delegation seam** with a central prompt-learning ledger. An expensive orchestrator session plans,
routes, delegates, reviews, and learns; a cheap worker session executes one bounded packet at a
time; an independent cross-model reviewer runs clean-room code review. `the-flow` remains the inner
route authority — `flow-pair` is the delegation wrapper, never a replacement.

It is invoked via `/pij pair` (routed by the `/pij` skill, `skills/pij/`) and shells out to the
`flow-pair` CLI (`skills/flow-pair/lib/cli.ts`) for all state-mutating operations. The CLI is never
imported into pi (P2 boundary).

## When to use it

Use `flow-pair` when a piece of work is bounded enough to delegate but would benefit from an
expensive orchestrator supervising it: a phase implementation, a fix, a review, a docs-writing task,
or a research packet. It shines when you want **compounding learning** across runs (prompt-learnings
cluster-isolated by miss type) and **cross-model review** that decorrelates blind spots from the
implementer.

Don't use it for trivial read-only work (the orchestrator runs that locally) or for plan-stage
decisions (the orchestrator owns the plan and never delegates it).

## The three sessions

- **Orchestrator** (expensive model): the "human" in the worker's loop. Plans, routes, compiles
  context packs, renders packets, reviews worker output against the-flow's stage contracts,
  synthesizes its own review with the cross-model reviewer's, and records learnings. **Sole writer
  of flow-state** (`.the-flow-state.json` / `the-flow.json` / `the-flow.md`) and the ledger.
- **Worker** (cheap model, e.g. Sonnet): executes one bounded packet at a time. Direct-jumps the
  relevant `the-flow` verb itself (`/the-flow 6 implement …`). Writes ONLY within its packet's
  allowed paths. Forbidden from flow-state and the ledger (except reading its own packet).
- **Reviewer** (cross-model family, e.g. OpenAI/ChatGPT when the worker is Sonnet): runs a
  clean-room review of the worker's output. Read-only — may read the ledger; never writes code,
  flow-state, or `.flow-pair/**`. Decorrelation is the point: it catches what the orchestrator +
  worker share blind spots for.

Sessions communicate via the **`pij_send` tool** (`pij_send({ to, message })` for text,
`{ to, command: "compact" | "reload" | "new" }` for control). Packets are **pointer-delivered**:
the full packet is saved to the ledger first, then only a short path pointer is sent over `pij_send`.

## Intents (CLI subcommands)

| Intent | Command | What it does |
|--------|---------|--------------|
| Start a run | `flow-pair start "<request>" [--repo <p>] [--ledger-root <p>]` | Opens a run, writes `run.json` + `run.started` event |
| Dispatch a packet | `flow-pair dispatch --run-id <id> --plan-path <p> --phase <text> --tasks-dir <p> [--allowed-paths <p1,...>]` | Compiles the context pack, renders the packet, writes it to the ledger, prints `[flow-pair <dlgId>] Packet at: <rel-path>` |
| Observe a diff | `flow-pair observe [--run-id <id>]` | Captures the worker's diff + changed files (Phase 5; flow-state guard enforced) |
| Review | `flow-pair review --delegation <id>` | **Stub — do not rely.** Emits an artifact-**presence** verdict only (never code correctness); the real verdict is the cross-model reviewer peer's, hand-persisted by the orchestrator. |
| Fix | `flow-pair fix --review <id>` | **Stub (unimplemented).** Fix packets are hand-rendered from the reviewer's findings and dispatched via `dispatch`. |
| Accept | `flow-pair accept --delegation <id>` | **Stub (unimplemented).** Acceptance is the orchestrator's recorded decision, not this command. |
| Learn | `flow-pair learn --run-id <id> --delegation-id <id> --cluster <c> --miss-type <t> --summary <text> [--evidence <text>] [--candidate-delta <text>] [--prompt-lab-root <p>] [--json]` | Records a prompt-learning candidate into one cluster's `candidates/` + a per-run ledger record (Phase 7) |
| Ledger | `flow-pair ledger [--run-id <id>]` | Prints `run.json` for a run |

The orchestrator drives these through the **`/pij pair`** route (`skills/pij/references/routes/pair.md`) — the front door that superseded the `/flow-pair` skill.

## The per-stage cycle (CODE stages)

For each code delegation the orchestrator runs this loop:

1. **Implement** → worker direct-jumps `/the-flow 6 implement` within its allowed paths.
2. **Compact the worker reflexively** — the instant it reports done, *before* reviewing its report
   (`pij_send({ to: worker, command: "compact" })`). The worker is idle during review, so the
   ~30–90s compact overlaps the orchestrator's own review work → zero wait at the next dispatch.
   (Compacting *late* repeatedly caused worker stalls on saturated contexts.)
3. **Orchestrator self-validate** — RAW-verify the report (never trust it): re-run gates, reproduce
   the load-bearing assertions, run an *independent* mutation (different sed than the worker's).
4. **Independent cross-model review** — dispatch `/the-flow 7 review` to the reviewer (pre-compacted).
   Keep reviewer ≠ implementer model family to decorrelate blind spots.
5. **Synthesize** — orchestrator merges its own review + the reviewer's findings + the gates.
6. **ACCEPT or FIX** — on `FIX_REQUIRED`, render a narrow fix packet scoped to the findings and loop.

## Test quality is a gate (Dimension 0)

The worker writes **both** the code and its own tests, so a green suite shares the author's blind
spots. Before ACCEPT, prove the tests are **non-vacuous**:
- the reproduction must FAIL on the pre-fix code;
- a mutation of the load-bearing guard must flip RED → GREEN;
- watch the **vacuous-test trap**: a dep that fails *before* the guard gives a fake green — drive
  execution THROUGH the guard with real fixtures.

Run `just flow-pair-mutate <file> '<sed-expr>'` (or a reasoned mutation argument). The reviewer's
rubric is in `skills/flow-pair/references/review-rubrics.md` (10 dimensions; Dimension 0 mandatory).

## Ledger layout

Each run lives at `.flow-pair/runs/<run-id>/` (gitignored — session-local state):

```
<run-id>/
  run.json              # run metadata (repoId, status, createdAt)
  events.jsonl          # append-only event log (the spine — see event union in lib/ledger.ts)
  prompts/              # worker/reviewer/fix packet files (dlg-NNNN-*.md)
  worker-reports/       # worker completion reports
  diffs/                # captured diffs (Phase 5 observe)
  reviews/              # orchestrator drafts + reviewer verdicts + synthesis
  fix-packets/          # narrow fix packets (Phase 6)
  learnings/            # per-run LearningRecord JSON (learn-NNNN.json)
  delegations/          # delegation metadata
  prompt-trials/        # prompt-trial records
```

**P9 (persist before mutate)**: the ledger event is appended + checked `{ok}` BEFORE any state write
— everywhere. The event union (`run.started`, `delegation.created`, `packet.written`,
`review.recorded`, `fix_packet.written`, `learning.created`, …) is the authoritative spine;
`AC-11` (ledger holds prompt+context+diff+review+result+learning) rides on it.

## Prompt-lab workflow (compounding learning)

Prompt-learnings live at `skills/flow-pair/prompt-lab/clusters/<cluster>/`:

```
<cluster>/
  active.md        # committed current prompt guidance for that cluster (human-curated)
  candidates/      # generated candidate learning notes (learn-NNNN.md) — one cluster only
  changelog.md     # manual promotion/audit notes
```

The six clusters are defined in `skills/flow-pair/references/prompt-taxonomy.md` (the single source
of truth — `PROMPT_CLUSTERS` in `lib/learning.ts` and the scaffold dirs are drift-tested against
it): `implement-code`, `fix-code`, `review-code`, `docs-writing`, `codebase-research`,
`validation-runner`.

**Cluster isolation (AC-07)**: a learning from an `implement-code` miss writes ONLY to
`implement-code/candidates/` — every sibling cluster + every `active.md`/`changelog.md` stays
byte-identical. Mismatch/invalid/traversal cluster → `{ok:false}`, zero writes.

**No silent auto-promotion**: candidates land in `candidates/` only. Promotion to `active.md` is a
manual, `changelog.md`-documented human action — the system NEVER writes `active.md` programmatically.

## End-of-work gate

Before declaring any delegation or phase done, run `harness checks` (all sensors:
local paths → typecheck → lockfile → lint → test → Copilot native → Rust → smoke, per-sensor
verdicts, non-zero exit on any failure). Use `harness checks --quick` mid-iteration (skips the heavy
Rust and smoke stages); run the full
`harness checks` before ship/declare-done. This supersedes `just self-check`'s first-fail behavior
— it runs ALL sensors so one pass surfaces every failure.

## Operational lessons (encoded, not just documented)

These came out of dogfooding flow-pair on its own 8-phase build and are now standing rules:

- **Compact workers/reviewers early, not late** — compact the instant a session reports done, as the
  first action, while you review/synthesize. Zero wait. (Encoded in `SKILL.md` Orchestrator Decision
  Protocol.)
- **Cross-model review earns its keep every phase** — in the flow-pair build it caught real issues
  the orchestrator's own pass + gates-green missed 7 phases running (schema drift, path-traversal,
  lifecycle ordering, path-normalization). Keep reviewer ≠ implementer family.
- **Gates-green ≠ correct** — the dogfood passed its full suite + cross-model review twice yet still
  failed in real use (lifecycle ordering, then path-normalization). Always RAW-verify against real
  behavior, and record the miss as a learning.
- **Reload-then-compact for crashy extensions** — compacting (and reloading) triggers a render. If a
  running session still has a known-crashy extension loaded, reload it onto the fix first and confirm
  it survives, then compact.

## References

- [`skills/pij/references/routes/pair.md`](../../skills/pij/references/routes/pair.md) — the `/pij pair` route:
  hard invariants, decision protocol, compact-early rule, fleet lifecycle, procedure (superseded `/flow-pair`)
- [`skills/flow-pair/references/architecture.md`](../../skills/flow-pair/references/architecture.md) —
  system architecture + CLI → lib → ledger call chain
- [`skills/flow-pair/references/orchestrator-worker-protocol.md`](../../skills/flow-pair/references/orchestrator-worker-protocol.md)
  — packet/report schemas, allowed/forbidden paths contract
- [`skills/flow-pair/references/ledger-schema.md`](../../skills/flow-pair/references/ledger-schema.md)
  — run/delegation/trial/review/learning record schemas
- [`skills/flow-pair/references/review-rubrics.md`](../../skills/flow-pair/references/review-rubrics.md)
  — 10-dimension rubric + verdict model (`APPROVE` / `APPROVE_WITH_NOTES` / `FIX_REQUIRED`)
- [`skills/flow-pair/references/prompt-taxonomy.md`](../../skills/flow-pair/references/prompt-taxonomy.md)
  — cluster taxonomy + lifecycle + promotion policy
