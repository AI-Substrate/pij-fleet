# Ledger Schema Reference

> **Phase 2 deliverable.** This file documents the run directory layout, record types, event taxonomy, JSON schemas, and Phase 3 read strategy for the flow-pair central ledger.

---

## Run Directory Layout

```
.flow-pair/
  runs/
    <runId>/                   ← one dir per run
      run.json                 ← RunRecord (mutable: status open→closed)
      events.jsonl             ← append-only event log (one JSON line per event)
      delegations/
        dlg-0001.json          ← DelegationRecord
        dlg-0002.json
      prompt-trials/
        trial-0001.json        ← PromptTrialRecord
      reviews/
        rev-0001.json          ← ReviewRecord
      learnings/
        learn-0001.json        ← LearningRecord
      prompts/                 ← Phase 4 writes packet files here
      worker-reports/          ← Phase 5 writes worker output here
      diffs/                   ← Phase 5 writes diff snapshots here
```

**runId format**: `<YYYY-MM-DDTHH-MM-SSZ>-<repoId[0:20]>`
(e.g. `2026-06-17T10-49-21Z-github.com-AI-Substr`)

---

## Record Types

### RunRecord (`run.json`)

| Field | Type | Description |
|-------|------|-------------|
| `runId` | string | Unique run identifier |
| `repoId` | string | Derived repo identity (`host-owner-repo` or `basename-<hash>`) |
| `runDir` | string | Path to run directory (relative or absolute depending on ledgerRoot) |
| `createdAt` | ISO 8601 | When the run was created |
| `status` | `"open" \| "closed"` | Lifecycle state |
| `closedAt` | ISO 8601? | Set when `closeRun` completes |

### DelegationRecord (`delegations/dlg-NNNN.json`)

| Field | Type | Description |
|-------|------|-------------|
| `delegationId` | string | `dlg-NNNN` (monotonic per run) |
| `runId` | string | Parent run — required link field |
| `taskRef` | string | Task identifier being delegated |
| `packetPath` | string | Relative path to the prompt packet in `prompts/` |
| `createdAt` | ISO 8601 | |
| `status` | `"pending" \| "accepted" \| "fix_required"` | |

### PromptTrialRecord (`prompt-trials/trial-NNNN.json`)

| Field | Type | Description |
|-------|------|-------------|
| `trialId` | string | `trial-NNNN` |
| `runId` | string | Link to run |
| `delegationId` | string | Link to delegation |
| `templateRef` | string | Path to SKILL.md template used |
| `promptHash` | string | Hash of rendered prompt |
| `createdAt` | ISO 8601 | |

### ReviewRecord (`reviews/rev-NNNN.json`)

| Field | Type | Description |
|-------|------|-------------|
| `reviewId` | string | `rev-NNNN` |
| `runId` | string | Link to run |
| `delegationId` | string | Link to delegation |
| `verdict` | `"APPROVE" \| "APPROVE_WITH_NOTES" \| "FIX_REQUIRED"` | |
| `findings` | `ReviewFinding[]` | Structured review findings |
| `createdAt` | ISO 8601 | |

**ReviewFinding sub-type**: `{ dimension: string; severity: "critical"|"high"|"medium"|"low"|"info"; message: string; file?: string }`

### LearningRecord (`learnings/learn-NNNN.json`)

| Field | Type | Description |
|-------|------|-------------|
| `learningId` | string | `learn-NNNN` |
| `runId` | string | Link to run |
| `delegationId` | string | Link to delegation |
| `cluster` | string | Prompt-lab cluster this learning targets |
| `candidatePath` | string | Path to the candidate prompt file |
| `createdAt` | ISO 8601 | |

---

## Event Taxonomy (`events.jsonl`)

Each line is one minified JSON object terminated by `\n`. Discriminated by `type`:

| `type` | Required fields | Emitted by |
|--------|----------------|-----------|
| `run.started` | `runId`, `repoId`, `at` | `createRun` |
| `run.closed` | `runId`, `at` | `closeRun` |
| `delegation.created` | `runId`, `delegationId`, `at` | `writeDelegation` |
| `prompt_trial.created` | `runId`, `delegationId`, `trialId`, `at` | `writePromptTrial` |
| `review.created` | `runId`, `delegationId`, `reviewId`, `at` | `writeReview` |
| `learning.created` | `runId`, `delegationId`, `learningId`, `at` | `writeLearning` |

---

## JSON Schema Files (`skills/flow-pair/schemas/`)

| File | Validates | Notes |
|------|-----------|-------|
| `run.schema.json` | `RunRecord` | |
| `event.schema.json` | `LedgerEvent` | `oneOf` for all 6 event types |
| `delegation.schema.json` | `DelegationRecord` | |
| `prompt-trial.schema.json` | `PromptTrialRecord` | |
| `review.schema.json` | `ReviewRecord` | includes `ReviewFinding` sub-schema |
| `learning.schema.json` | `LearningRecord` | |

All schemas are **JSON Schema draft-07**. No runtime validation in Phase 2.

---

## P9: Persist-Before-Mutate Invariant

Every `LedgerWriter` method appends the typed event to `events.jsonl` **before** writing any state file (record JSON or updated `run.json`). This means:

- If `writeFileSync` throws after a successful `appendFileSync`, the event is durable and the ledger is recoverable.
- If `appendFileSync` fails, the method checks `appendEvent`'s `{ok}` return and **aborts before writing the state file** — no event-less record can be created.
- `closeRun` reads and validates `run.json` **first** (read is not a mutation), then appends `run.closed`, then writes the updated `run.json`. This prevents a false `run.closed` event when `run.json` is missing or malformed.

---

## Phase 3 Read Strategy

`LedgerWriter` is **write-only**. Phase 3 and later phases read ledger files directly via `node:fs`:

```typescript
// Example: Phase 3 context-pack compiler (not a Phase 2 deliverable)
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { LEDGER_ROOT, RUNS_DIR } from "../skills/flow-pair/lib/paths.js";
import type { RunRecord } from "../skills/flow-pair/lib/ledger.js";

const runDir = join(LEDGER_ROOT, RUNS_DIR, runId);
const run = JSON.parse(readFileSync(join(runDir, "run.json"), "utf8")) as RunRecord;
const events = readFileSync(join(runDir, "events.jsonl"), "utf8")
  .split("\n")
  .filter(Boolean)
  .map((l) => JSON.parse(l));
```

---

*Last updated: Phase 2 implementation (dlg-0007) + fixes (dlg-0009).*
