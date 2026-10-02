# Architecture

Describes the system architecture of `flow-pair`: the orchestrator session, worker session,
central ledger, and the CLI → lib → ledger call chain.

---

## Sessions

```
Orchestrator (expensive model)          Worker (cheap model)
│                                       │
│  flow-pair dispatch → stdout          │  receives pointer via pij_send
│  reads pointerMsg                     │  reads packet from prompts/
│  pij_send { to: workerId, message }──▶│  executes bounded task
│  awaits worker report                 │  pij_send report { to: orchestratorId }
│  runs review rubric                   │
│  APPROVE / APPROVE_WITH_NOTES / FIX_REQUIRED │
```

**Key rule**: the CLI and `lib/` layers never call `pij send`. Transport is always
in the orchestrator SKILL.md via the `pij_send` tool. This is the P2 boundary.

---

## CLI → lib → ledger call chain (Phase 4 dispatch)

```
SKILL.md orchestrator
  └─ flow-pair dispatch
        --run-id <id>
        --plan-path <p>
        --phase <text>
        --tasks-dir <p>
       [--task-description <t>]
       [--allowed-paths <p1,p2,...>]
       [--json]

cli.ts runDispatch()
  ├─ resolveRunDir(ledgerRoot, runId)         → guard: no traversal, no abs path
  ├─ pre-compute delegationId from readdirSync(delegationsDir)
  ├─ LedgerWriter.writeDelegation(runId, {taskRef, packetPath})
  │     → .flow-pair/runs/<runId>/delegations/dlg-NNNN.json
  │     → events.jsonl: { type:"delegation.created", ... }
  ├─ ContextPackCompiler.compile({runId, delegationId, planPath, phase, tasksDir, ...})
  │     → extractSection(planPath, phase)
  │     → clusterLearnings(ledgerRoot, cluster)
  │     → appendLedgerEvent: { type:"context_pack.created", ... }
  │     → .flow-pair/runs/<runId>/context-packs/cp-NNNN.json
  │     → returns ContextPackManifest
  └─ PacketRenderer.writePacket({manifest, taskDescription, repoRoot})
        ├─ resolveRunDir guard
        ├─ delegationId validation: /^dlg-\d{4}$/
        ├─ renderBody (single-pass {{PLACEHOLDER}} substitution)
        │     guards: plan-phase required; forbiddenPaths non-empty
        ├─ sha256[0:8] of body → promptHash
        ├─ [P9] appendLedgerEvent: { type:"packet.written", ... } → events.jsonl FIRST
        ├─ writeFileSync(prompts/dlg-NNNN.md, body)
        ├─ LedgerWriter.writePromptTrial → prompt-trials/trial-NNNN.json
        └─ returns { pointerMsg, delegationId, packetPath, promptHash }

stdout (non-JSON): EXACTLY the pointer line
  "[flow-pair dlg-NNNN] Packet at: .flow-pair/runs/<runId>/prompts/dlg-NNNN.md"

stdout (--json): full JSON object

SKILL.md reads stdout → pij_send({ to: workerId, message: pointerMsg })
```

---

## Ledger layout (under `LEDGER_ROOT = .flow-pair/`)

```
.flow-pair/
  runs/
    <runId>/
      run.json               ← RunRecord (Phase 2)
      events.jsonl           ← append-only event log (Phase 2)
      delegations/
        dlg-NNNN.json        ← DelegationRecord (Phase 2)
      prompt-trials/
        trial-NNNN.json      ← PromptTrialRecord (Phase 2)
      reviews/
        rev-NNNN.json        ← ReviewRecord (Phase 2)
      learnings/
        learn-NNNN.json      ← LearningRecord (Phase 2)
      prompts/               ← PROMPTS_DIR (Phase 4)
        dlg-NNNN.md          ← rendered worker packet (Phase 4)
      context-packs/
        cp-NNNN.json         ← ContextPackManifest (Phase 3)
      worker-reports/        ← (Phase 5)
      diffs/                 ← (Phase 5)
```

---

## P9 event ordering in events.jsonl

Every write follows **persist-before-mutate**:

| Method | Event appended | Then |
|--------|---------------|------|
| `createRun` | `run.started` | writes `run.json` |
| `writeDelegation` | `delegation.created` | writes `delegations/dlg-NNNN.json` |
| `writePromptTrial` | `prompt_trial.created` | writes `prompt-trials/trial-NNNN.json` |
| `compile` | `context_pack.created` | writes `context-packs/cp-NNNN.json` |
| `writePacket` | `packet.written` | writes `prompts/dlg-NNNN.md`, then `writePromptTrial` |
| `closeRun` | `run.closed` | updates `run.json` |

---

## Lib boundaries

| Layer | P2 constraint |
|-------|---------------|
| `lib/*.ts` | Zero `@earendil-works/*` imports; no subprocess calls |
| `lib/cli.ts` | No `pij send` — only prints pointer to stdout |
| `SKILL.md` | Calls `pij_send` tool with stdout pointer — transport boundary |

---

## Template rendering (Phase 4)

`references/templates/worker-implement.md` uses `{{PLACEHOLDER}}` markers (double-brace, no
whitespace). `PacketRenderer.renderBody` replaces them in a **single regex pass**:

```typescript
template.replace(/\{\{([A-Z_]+)\}\}/g, (match, key) => subs[key] ?? match)
```

Single-pass prevents re-substitution: if injected content (e.g. `TASKS_CONTENT`) contains
a string like `{{LEARNINGS_CONTENT}}`, it is left as-is rather than being substituted again.
