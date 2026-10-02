# Context Packs — Reference

> **Phase 3 deliverable.** This file documents the `ContextPackCompiler` API, manifest schema, extraction rules, exclusion policy, P9 invariant, and the Phase 4 contract handoff.

---

## Purpose

A **context pack** contains exactly what one worker needs for one delegation — no more. It is assembled from:
- A named section of the plan file (not the whole plan)
- The delegation's `tasks.md` (if it exists)
- The delegation's `execution.log.md` (if it exists)
- Same-cluster learnings from `prompt-lab/clusters/<cluster>/active.md` (empty when Phase 7 not built)

Omitted sources are recorded as `exclusions` with a controlled-vocabulary `reason` so Phase 6 can detect missing artifacts without re-reading the filesystem.

---

## `ContextPackCompiler` API

### Constructor

```typescript
new ContextPackCompiler(
  repoRoot: string,     // absolute path to the repo root
  ledgerRoot: string,   // absolute path to the ledger root (.flow-pair)
  deps?: ContextPackDeps,  // injected fs deps (default: nodeContextPackDeps())
)
```

### `extractSection(filePath, sectionHeading)`

```typescript
extractSection(
  filePath: string,        // absolute path to a markdown file
  sectionHeading: string,  // section to find (exact/prefix-colon/prefix-space match)
): { ok: boolean; content?: string; error?: string }
```

**Match rule** (prefix-boundary, not substring — prevents "Phase 1" matching "Phase 10"):
```
const norm = headingLine.replace(/^#+\s*/, "").trim().toLowerCase()
const target = sectionHeading.trim().toLowerCase()
match if: norm === target
      OR: norm.startsWith(target + ":")
      OR: norm.startsWith(target + " ")
```

Captures the matched heading line plus all lines until the next heading at the **same or higher** level (lower `#` count). Returns `{ok:false}` when:
- file not found (ENOENT)
- section not found (after scanning all headings)
- sectionHeading is empty

### `clusterLearnings(cluster)`

```typescript
clusterLearnings(
  cluster: string,  // prompt-lab cluster name
): { ok: boolean; learnings?: ClusterLearning[]; error?: string }
```

Always `{ok:true}`. Returns `[]` when `skills/flow-pair/prompt-lab/clusters/<cluster>/` does not exist or `active.md` is absent (Phase 7 not yet built — this is the expected path for Phases 3 and 4).

### `compile(opts)`

```typescript
compile(opts: CompileOpts): { ok: boolean; manifest?: ContextPackManifest; error?: string }
```

**Algorithm** (step order is the contract):

1. `resolveRunDir(this.ledgerRoot, opts.runId)` — `{ok:false}` on bad/traversal runId
2. `extractSection(opts.planPath, opts.phase)` — `{ok:false}` on file-missing or section-absent
3. Read `opts.tasksDir/tasks.md` → entry with `content`; or exclusion `reason:"not found"`
4. Read `opts.tasksDir/execution.log.md` → entry with `content`; or exclusion `reason:"not found"`
5. `clusterLearnings(opts.cluster)` — `[]` if Phase 7 absent
6. Hash each entry via `sha256[0:8]` of `entry.content`
7. `deps.mkdirSync(packDir, {recursive:true})` — **must come before readdirSync**
8. `nextPackId(packDir)` via `readdirSync` (monotonic `cp-NNNN`)
9. **[P9]** `appendLedgerEvent(this.deps, runDir, context_pack.created)` — if `!ev.ok` return `{ok:false}`, writeFileSync is **never called**
10. `deps.writeFileSync(packDir/cp-NNNN.json, JSON.stringify(manifest))`

`{ok:false}` is propagated from every `{ok:false}` guard — callers never need to guard against throws.

---

## Manifest Schema

### `ContextPackEntry`

| Field | Type | Description |
|-------|------|-------------|
| `path` | `string` | Source file path (absolute) |
| `section` | `string?` | Section heading searched (undefined = whole file) |
| `content` | `string` | Extracted text content — **Phase 4 renders directly from this** |
| `hash` | `string` | `sha256[0:8]` of `content` |
| `role` | `"plan-phase" \| "tasks" \| "execution-log" \| "learning"` | How Phase 4 uses this entry |

### `ContextPackExclusion`

| Field | Type | Description |
|-------|------|-------------|
| `path` | `string` | File/path that was excluded |
| `reason` | `"not found" \| "wrong cluster" \| "other phase"` | Controlled vocabulary (Phase 6 uses programmatically) |

### `ContextPackManifest`

| Field | Type | Description |
|-------|------|-------------|
| `packId` | `string` | `cp-NNNN` (monotonic per run) |
| `runId` | `string` | Link to parent run |
| `delegationId` | `string` | Link to delegation |
| `phase` | `string` | The `opts.phase` string verbatim |
| `cluster` | `string` | Prompt-lab cluster name |
| `entries` | `ContextPackEntry[]` | Included sources |
| `exclusions` | `ContextPackExclusion[]` | Omitted sources |
| `allowedPaths` | `string[]` | Paths the worker may modify |
| `forbiddenPaths` | `string[]` | Paths the worker must never touch |
| `createdAt` | ISO 8601 | |

### `ClusterLearning`

| Field | Type | Description |
|-------|------|-------------|
| `cluster` | `string` | Cluster name |
| `sourcePath` | `string` | Absolute path to `active.md` |
| `content` | `string` | Raw file content |

---

## Constants (P5)

```typescript
export const CONTEXT_PACKS_DIR = "context-packs";  // subdir under run dir
export const PACK_ID_PREFIX = "cp";                  // prefix for packId allocation
export const DEFAULT_FORBIDDEN_PATHS = [             // always in forbiddenPaths unless overridden
  ".the-flow-state.json",
  "the-flow.json",
  "the-flow.md",
  ".flow-pair/",
] as const;
```

**Implementation note**: when assigning `DEFAULT_FORBIDDEN_PATHS` to `manifest.forbiddenPaths: string[]`, spread to avoid TypeScript `readonly` conflict:
```typescript
forbiddenPaths: opts.forbiddenPaths ?? [...DEFAULT_FORBIDDEN_PATHS]
```

---

## Exclusion Rules

| Excluded when | reason | Phase 6 implication |
|--------------|--------|---------------------|
| `tasks.md` absent | `"not found"` | Phase 6 finding: missing artifact |
| `execution.log.md` absent | `"not found"` | Phase 6 finding 6.1: `FIX_REQUIRED` (artifact_contract) |
| active.md from wrong cluster | `"wrong cluster"` | Cluster isolation — learnings are cluster-scoped |
| Phase section absent | compile returns `{ok:false}` — not an exclusion | Plan section missing is a hard error |

Phase 6 searches `manifest.exclusions` for entries where `path.endsWith("execution.log.md") && reason === "not found"` to emit the `artifact_contract` finding.

---

## P9 Invariant

Every `compile` call appends `context_pack.created` to `events.jsonl` **before** writing `cp-NNNN.json`:

```typescript
// P9 flow inside compile:
const ev = appendLedgerEvent(this.deps, runDir, { type: "context_pack.created", ... });
if (!ev.ok) return { ok: false, error: ev.error };   // writeFileSync never called
this.deps.writeFileSync(join(packDir, `${packId}.json`), ...);
```

`appendLedgerEvent` is the same standalone helper that Phase 2's `LedgerWriter.appendEvent` delegates to — the tested primitive is reused, not re-implemented.

---

## Phase 4 Contract Handoff

Phase 4 (`lib/packet.ts`) consumes the `ContextPackManifest` to render the worker packet:

- **Entries carry `content`**: Phase 4 reads `entry.content` directly — no re-reading files.
- **`allowedPaths`**: injected verbatim into the worker packet as the allowed-to-modify list.
- **`forbiddenPaths`**: injected verbatim as the must-not-touch list (includes flow-state paths).
- **`delegationId` + `runId`**: used to write the `PromptTrialRecord` via Phase 2 `LedgerWriter`.

Expected Phase 4 call pattern:
```typescript
import { ContextPackCompiler } from "./context-pack.js";

const compiler = new ContextPackCompiler(repoRoot, ledgerRoot);
const { ok, manifest } = compiler.compile(opts);
// manifest.entries[role==="plan-phase"]?.content  ← plan section text
// manifest.allowedPaths                            ← inject into packet
// manifest.forbiddenPaths                          ← inject into packet
```

---

## CLI (`flow-pair dispatch`)

```
flow-pair dispatch \
  --run-id <id> \
  --delegation <id> \
  --plan-path <absolute-path> \
  --phase "<phase-heading>" \
  --tasks-dir <absolute-path> \
  [--cluster <name>] \
  [--allowed-paths path1,path2,...] \
  [--ledger-root <path>] \
  [--json]
```

Returns: `{ ok: true, packId, entries, exclusions }` or exits 2 on error.

---

*Last updated: Phase 3 implementation (dlg-0011).*
