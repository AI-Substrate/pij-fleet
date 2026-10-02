# pair — orchestrate a coder + cross-model reviewer fleet

> Route module — sibling-blind. Knows only this job; composition is the dispatch's job.
> Conventions cited as § C*n* live in `00-routing.md` § Shared conventions (pull lazily).

**Job**: run a whole build phase through an **orchestrator + colleague-peer delegation
seam** — wrap `the-flow` (the inner route authority) with a small **roster** of pij
colleague sessions (a **coder** that implements bounded packets + a separate
**cross-model reviewer**), acquired lazily and reused across the run, plus a central
prompt-learning ledger. This is the delegation *wrapper* — never a replacement for
`the-flow`, which stays the sole writer of its own state.

**Preconditions**: delivery ownership/identity per § C1 and a reachable rs daemon (`pij-rs ping --json`). Shim spawn/agent/tree/link/models are named refusals, not a legacy escape; use explicit native spawn grammar and supported host tools. The **engine is already built** — the `flow-pair` CLI
(`<flow-pair skill root>/lib/cli.ts`) + the `.flow-pair/` ledger. This route *drives* that
CLI; it never reimplements it, never imports it into pi (P2), and never writes the ledger
by hand.

> **`<flow-pair skill root>`** below = the INSTALLED flow-pair skill directory (the parent
> of its `lib/`, `references/`, `prompt-lab/`) — a sibling of this pij skill in the skills
> install dir, NOT a path inside the consuming repo. Rendered packets carry it absolute as
> `{{SKILL_ROOT}}`, so worker-cited references resolve in any repo.

## Hard invariants

1. **Flow-state non-write** — NEVER write `.the-flow-state.json`, `the-flow.json`, or
   `the-flow.md`. `the-flow` guided mode is their **sole** writer; a dual-writer corrupts
   resume/adopt.
2. **Pointer delivery** — a worker packet is written to the ledger first; only a short
   **path pointer** is sent (never a full packet body inline). Pointer messages, worker
   done-reports, and reviewer verdicts all follow § C10 (wire discipline): first line =
   action/verdict, delta + ids, no restatement of the packet.
3. **Forbidden paths in every packet** — each packet enumerates the three flow-state files
   above (plus any ledger dir) as explicitly off-limits to the worker.
4. **Bounded scope** — each packet defines its allowed paths; the worker executes ONLY
   within them.
5. **Persist before mutate (P9)** — ledger/roster records are written before the state they
   describe changes.
6. **Cluster isolation** — a prompt learning writes ONLY to the cluster it was tagged to,
   never cross-cluster.

## Required PM report steps

Everything under `report` is a first-person claim about yourself. These are
route steps, not reminders:

1. **Start-of-work report** — run before the first phase mutation or dispatch:

   ```bash
   pij report now 'Starting **<phase>**' 'Compile the packet and dispatch the coder'
   ```

2. **Stop-of-work report** — run after the final gate/verdict is persisted and
   before sending the completion pointer or waiting:

   ```bash
   pij report now 'Approved **<phase>** after `harness checks`' 'Send the [report](<path>) and await the next assignment'
   ```

Inline markdown works in both text fields (`` `code` ``, `**bold**`,
`[links]`); newlines are refused, so block markdown is not supported. Use
`pij report question "<what I need from you>"` for a human answer and
`pij report blocked "<what I am waiting on>"` for an external dependency.
Actively working has no semantic state word: report progress without inventing
`working`. Completion is `pij report state done`, never a watchdog self-pause.

## Completion interrupt — compact EARLY

Treat a terminal completion or verdict as an interrupt, not a step to remember at redispatch:

1. **Coder completion**
2. For a reusable/live peer, compact the coder FIRST (§ C3).
3. Then handle the report, acquire/canary the reviewer if needed, and dispatch review.
4. **Reviewer verdict**
5. For a reusable/live peer, compact the reviewer FIRST (§ C3).
6. Then follow the `FIX` or `APPROVE` path.
7. **Buggy-extension safety**
8. If the peer still has a known-crashy extension loaded, reload FIRST and confirm it
   survives; then compact. Fresh spawns already carry the fix.

Both compact sends are fire-and-forget: use the § C3 command without `--wait`.
Never wait for compact receipts or completion; continue immediately with report/review/fix work.
The one-shot auto-dissolve boundary remains owned by § C3.

## Orchestrator Decision Protocol

**You own the deliverable — delegation moves the work, not the accountability.**
You are the expensive model in this fleet for a reason: the coder may be a cheaper,
less-capable model, and the reviewer is a *different* model that may have skimmed.
A worker's green tests and a reviewer's `APPROVE` are both **claims**, not proof.
Trust them enough to keep moving — but the last critical eye on every deliverable
is **yours**. Trust, but verify: before you record any approval, cast your own eye
over the load-bearing part of the result (§ APPROVE). If the verdict doesn't survive
your glance, you re-open it — you never rubber-stamp a verdict you can't stand behind.
This is one cheap spot-check, **not** a re-review; the reviewer still does the deep pass.

The expensive orchestrator runs this finite-state loop each turn:

| State | When | Action |
|-------|------|--------|
| `ASK_USER` | Requirement ambiguous / needs a human decision | Pause and ask; never proceed without the answer |
| `RUN_LOCAL` | Safe, cheap, read-only, or needs no delegation | Execute directly in the orchestrator session |
| `DELEGATE` | Bounded, executable, suitable for a cheap worker | Compile context pack → render packet → send the path pointer. **Delegate a whole phase per packet and make the packet say so**: the coder implements *every* task in the phase in one run — not a couple-then-handback, which wastes a round-trip per slice and loses the worker's warm context |
| `REVIEW` | Worker reports completion | **Compact the worker FIRST** (reflex, § C3) → acquire/canary the **reviewer peer** if not yet live → dispatch a review pointer. The verdict (+ mandatory Dim-0 mutation gate) is produced **by the reviewer**, handled on its return |
| `FIX` | Verdict = `FIX_REQUIRED` | **Compact the reviewer FIRST** (§ C3) → render a narrowed fix packet (review findings only) → dispatch it to the coder (DELEGATE) |
| `APPROVE` | Verdict = `APPROVE` / `APPROVE_WITH_NOTES` | **Compact the reviewer FIRST** (§ C3) → run the sanity pass below → record approval → update ledger → advance |

### The orchestrator sanity pass — the last gate before APPROVE (reflexive)

A reviewer `APPROVE` is the *input* to your approval, not a substitute for it. Before
you record approval, spend **one cheap glance** confirming the verdict survives your
own eye — this is the "verify" half of trust-but-verify, and it is **not** a re-review:

- **Re-read the actual diff hunk** behind the single highest-severity claim the reviewer
  cleared (or, for a clean CODE pass, the one load-bearing guard). Does the code in front
  of you actually match the verdict's story?
- **Confirm Dim-0 was really exercised** for CODE delegations — the review carries mutation
  evidence (the guard, the sed expr, RED→GREEN), not just the word "non-vacuous." If the
  reviewer asserts test quality with no mutation/named-assertion evidence, that is a missing
  proof — treat it as `FIX_REQUIRED`, not APPROVE.
- **Sniff for a rubber-stamp**: an `APPROVE` with no findings, no files named, and no
  evidence on a non-trivial diff is itself suspect. A reviewer can skim. When the verdict
  is thinner than the change deserves, re-open it (bounce back to the reviewer, or look
  yourself) before recording.

If the verdict holds, record it and move on — the goal is a 30-second confidence check,
not a second review. If it doesn't, you do **not** record APPROVE: loop to `FIX` or
re-dispatch the review. The buck stops with you.

## Fleet lifecycle — the colleagues (coder + reviewer)

A run keeps a small **roster**, acquired **lazily** and reused across the whole run (never
torn down between phases — only at final tidy). The roster lives in the ledger (`run.json`)
as `role → { pijId, paneId, model, spawnedByUs }`, **persisted before use (P9)** so a later
tidy can find and close our panes even after a crash.

**Roles & default models** (override per run via `--coder-model` / `--reviewer-model`; confirm
exact ids with the host's model catalog and real runtime canary, § C4; `pij models` is unported):

| Role | Default model | Acquire when |
|------|---------------|--------------|
| coder | `github-copilot/claude-sonnet-4.6:xhigh` | first `DELEGATE` |
| reviewer | `github-copilot/gpt-5.5:xhigh` (cross-model, deliberately ≠ coder) | first `REVIEW` |

- **Acquire — provided-or-spawn, lazy.** If a role's peer id was provided, use it; else spawn
  the *first time that role is needed* (coder on first `DELEGATE`, reviewer on first `REVIEW`)
  and never hijack ambient idle peers. **Mandate: do NOT pre-spawn the reviewer with the
  coder** — an idle-but-live peer is not free (its warm context is re-cached every
  compact/keep-alive cycle, and cache TTL forces rewrites), so a reviewer stood up a phase
  early quietly burns cache-token writes + a pane with zero reviews to show. Spawn it at the
  *moment* of the first `REVIEW`. Spawn transport per § C1; explicit supported native/tool placement per § C5. Native CLI has no legacy layout flags.
- **Canary-verify before trusting (§ C2)** — a ready-ping is NOT proof; a wrong `--model` is
  accepted silently then 400s on first inference. Verify footer + no-400 before first use —
  for *provided* peers too.
- **Designate from above after verifying parentage.** Native spawn/adopt records the supported parent claim. Then the current parent runs `pij role <peer-id> worker --json`. Existing-seat reparent/root placement (`link`) is unported: a role change cannot repair it; escalate mismatches rather than mutating legacy records.
- **Reuse across phases — compact, never close (§ C3)** — the same coder + reviewer carry the
  whole run, clean-slate each phase.
- **Heal** — corroborate a stale/dead binding, then use authorized rs retirement and explicit native spawn/revive as appropriate. Close tombstones only; owned terminal teardown is separate. Persist the replacement roster before redelivery (P9), never infer a force bypass.
- **Reviewer-never-binds → HALT, never self-review (control integrity).** Healing is for a peer
  that *was* working and went stale. It is **not** a licence to proceed reviewer-less. If the
  reviewer peer cannot be brought to `bound` — spawn fails, canary never passes, or `pij state`
  shows it stuck `pending`/`failed` after one heal attempt — you **HALT the pair with a named
  error** (e.g. `REVIEWER_UNAVAILABLE: <role/model> failed to bind after N attempts`) surfaced to
  the human and recorded in the ledger. You do **NOT** fall through to reviewing the coder's work
  yourself: the orchestrator sanity pass (§ below) is a spot-check *on top of* an independent
  reviewer's verdict, never a *substitute* for it. With no reviewer peer there is **no verdict** —
  the same law as the CLI refusing to mint one from zero findings (§ Verdict law). A degraded
  self-review that *looks like* progress is the exact silent-no-op failure mode this route forbids.
- **Teardown — end only** — close **only** peers with `spawnedByUs === true` (§ C1 verb); leave
  *provided* peers for their owner. Close is ownership-aware either way.
- **Completion safety** — use the compact-EARLY interrupt above; fresh spawns already carry
  the current payload.

> **Real peers, not builtin subagents.** Builtin subagents are **read-blind** here (they cannot
> read files), so a coder/reviewer must be a real pij peer (spawned or provided) — never a
> builtin-subagent fanout.

## Pipeline while a colleague is busy (§ C7)

The decision loop is per-turn, but never idle while a colleague works: the instant you dispatch
(a packet to the coder, or a review to the reviewer), advance the **next independent** work —
prep the next phase's tasks/context pack, draft the next packet, update the flight plan. **Let
the daemon's push re-invoke you** on the colleague's done-report (or a `stalled`/`dead` push);
do **not** sit in a `pij state` poll loop or nudge a peer that merely looks idle (§ C7 owns this,
incl. the one broken-transport spot-check exception).

## Verdict law — what the CLI does and doesn't decide (finding 03)

The `flow-pair` CLI's `review` verb is an **artifact/contract gate**, not a code reviewer: it
computes a verdict from the **severity of the findings you feed it** (`lib/review.ts` —
critical/high → `FIX_REQUIRED`, medium → `APPROVE_WITH_NOTES`); with **zero findings it
REFUSES to mint a verdict** ("no findings to review" — APPROVE is never a default); it never
reads the diff for correctness. So:

- The **real verdict is the reviewer peer's** judgment (with the mandatory **Dim-0** mutation
  gate), hand-persisted to the review record — that is the law, not the CLI's exit.
- `fix` is real (renders a narrowed fix packet); **`accept` is unimplemented** (a stub) — do
  not rely on it to close a run; record approval + advance the ledger yourself.
- Full 10-dimension rubric + verdict model: `<flow-pair skill root>/references/review-rubrics.md`.

## Invocation

```
/pij pair start "<request>" [--repo <path>] [--ledger-root <path>] [--coder-model <m>] [--reviewer-model <m>]
/pij pair dispatch --run-id <id> --plan-path <p> --phase <text> --tasks-dir <p> [--cluster <c>] [--task-description <t>] [--allowed-paths <p1,...>]
/pij pair observe [--run-id <id>]
/pij pair review --run-id <id> --delegation-id <id> --phase-dir <p>
/pij pair fix --run-id <id> --delegation-id <id> --review-id <id>
/pij pair accept --delegation <id>
/pij pair ledger [--run-id <id>]
/pij pair learn --run-id <id> --delegation-id <id> --cluster <c> --miss-type <t> --summary <text> [--evidence <text>] [--candidate-delta <text>] [--json]
```

Every state-mutating operation shells to the `flow-pair` CLI (never imported into pi — P2
boundary). Call chain (CLI → lib → ledger): `<flow-pair skill root>/references/architecture.md`.

## Procedure

1. **Resolve intent** → a decision-protocol state (above).
2. **Load context pack** — the relevant plan sections + **same-cluster** learnings from
   `<flow-pair skill root>/prompt-lab/clusters/<cluster>/active.md` only (cluster
   isolation; learnings live under the SKILL root, never the consuming repo — a missing
   cluster is recorded as a manifest exclusion, not silently empty). Extraction rules:
   `<flow-pair skill root>/references/context-packs.md`.
3. **Render packet** — `<flow-pair skill root>/references/templates/worker-implement.md`
   (or `worker-fix.md` for a FIX). These templates are **runtime-read by the engine** —
   cited, never moved. The engine injects the absolute skill root as `{{SKILL_ROOT}}` so
   packet citations resolve in any repo. Include § C11 (Commit attribution) in implement/fix packets.
4. **Deliver** — `flow-pair dispatch …` compiles the pack, writes the packet to
   `.flow-pair/runs/<run-id>/prompts/<delegationId>.md`, and prints exactly ONE line:
   `[flow-pair <delegationId>] Packet at: <rel-path>`. Send **that pointer** to the worker
   (§ C1 verb). The lib never sends — the orchestrator does.
5. **Review via the reviewer peer** — on the worker's report, compact it FIRST (§ C3), then
   hand the diff to the reviewer (acquire/canary if not live) with the rubric
   (`review-rubrics.md`). **Dim-0 (test quality) is mandatory for CODE packets** — the worker
   wrote its own tests, so green ≠ good; the reviewer proves them non-vacuous (a mutation
   gate using the consuming repo's own test runner — break the guard, rerun the targeted
   test, confirm RED, restore — or a named-assertion argument) before approval. On the
   verdict, compact the reviewer FIRST, then run the sanity pass and APPROVE, or loop
   to FIX.
6. **Learn** — after approval, write a candidate note to
   `<flow-pair skill root>/prompt-lab/clusters/<cluster>/candidates/` (never auto-promote
   to `active.md`).
7. **End-of-work gate** — before declaring any delegation/phase done, run the **consuming
   repo's own gates** (typecheck, lint, tests — its README/justfile/package scripts name
   them; ask the user if unnamed). Run ALL of them, not first-fail, so one pass surfaces
   every failure. Never assume another repo's recipes (e.g. pij's `just`/`harness`
   commands) exist in the target repo.
8. **Commit boundary** — apply § C11 (Commit attribution) to any resulting commit.

## References (cited in place — engine-owned, not moved; all under `<flow-pair skill root>`)

- `<flow-pair skill root>/references/review-rubrics.md` — 10-dimension rubric + verdict model
- `<flow-pair skill root>/references/ledger-schema.md` — run/delegation/trial/review/learning schemas
- `<flow-pair skill root>/references/context-packs.md` — context-pack extraction rules
- `<flow-pair skill root>/references/prompt-taxonomy.md` — cluster taxonomy (implement-code, fix-code, review-code, …)
- `<flow-pair skill root>/references/architecture.md` — CLI → lib → ledger call chain
- `<flow-pair skill root>/references/orchestrator-worker-protocol.md` — packet/report schema + allowed/forbidden-paths contract (cited absolute in rendered packets via `{{SKILL_ROOT}}`)
- `<flow-pair skill root>/references/templates/` — orchestrator-stage · worker-implement · worker-fix · review-synthesis · learning-synthesis (worker-* are runtime-read)
