# Domain: flow-pair

## Purpose

Wrap `the-flow` with a **two-session orchestrator/worker delegation seam** plus
a central, cross-repo experiment and prompt-learning ledger. The expensive
**orchestrator** session owns requirement clarification, flow routing, bounded
delegation, context-pack compilation, diff/artifact review, validation
interpretation, and prompt-learning. The cheap **worker** session executes one
bounded packet at a time in the target repo and returns a structured report. The
central **ledger** records runs, prompts, context packs, diffs, validations,
reviews, and cluster-isolated prompt learnings across repos.

`the-flow` remains the inner route authority; `flow-pair` is a wrapper-level
**delegation seam**, never a replacement. The defining protocol constraint is
the **single flow-state-writer invariant**: `the-flow` guided mode is the sole
writer of `.the-flow-state.json` / `the-flow.json` / `the-flow.md`; worker
packets hard-forbid those paths.

> **Front door → pij-skill.** The operator-facing skill front door is now the **`/pij`
> router** (`skills/pij/`, domain `pij-skill`); pairing is invoked as **`/pij pair`** and the
> protocol prose lives in `skills/pij/references/routes/pair.md`. `skills/flow-pair/SKILL.md`
> is reduced to a supersession shim. **This domain still owns the engine** — the `flow-pair`
> CLI, `lib/`, `schemas/`, `test/`, `prompt-lab/`, `references/`, and the `.flow-pair/` ledger.

## Source Locations

> **Phases 1–5 are built**; Phases 6–8 (`review.ts`, `learning.ts` + wiring) are
> planned. Built so far: `identity.ts`, `paths.ts`, `cli.ts`, `ledger.ts`
> (+ `appendLedgerEvent`), `context-pack.ts`, `packet.ts`, `observe.ts`, `schemas/*.json`
> (incl. `packet.written` + `files.changed` events).

| Path | Role |
|------|------|
| `skills/flow-pair/SKILL.md` | **Supersession shim** (≤20 lines) → `/pij pair`; keeps flow-pair's NL trigger phrases. The protocol (invariants, decision FSM, procedure) now lives in `skills/pij/references/routes/pair.md`. |
| `skills/flow-pair/references/*.md` | Reference documents: architecture, orchestrator/worker protocol, ledger schema, prompt taxonomy, context-pack spec, review rubrics. |
| `skills/flow-pair/references/templates/*.md` | Stage-bound prompt templates: `worker-implement`, `worker-fix`, `review-synthesis`, `learning-synthesis`, `orchestrator-stage`. |
| `skills/flow-pair/prompt-lab/clusters/*/active.md` | Versioned active prompt templates per cluster (`implement-code`, `fix-code`, `review-code`, `docs-writing`, `codebase-research`, `validation-runner`, …). Each cluster also has `candidates/` and `changelog.md`. |
| `skills/flow-pair/schemas/*.json` | JSON schemas for the core record types: run, delegation, prompt-trial, review, learning. |
| `skills/flow-pair/lib/*.ts` | Pi-free helper library: `identity.ts` (repo-identity derivation), `paths.ts` (run-dir layout), `ledger.ts` (append-only event log + record writers), `context-pack.ts` (section extraction + manifest), `packet.ts` (render + write), `observe.ts` (git diff capture), `review.ts` (verdict logic), `learning.ts` (cluster-isolated candidate writer). |
| `skills/flow-pair/lib/cli.ts` | Thin `flow-pair` CLI entrypoint — arg parse → lib calls, `--json` flag, exit codes. The skill/agent shells out to this; it is never imported into pi (preserves P2 boundary). Mirrors the `pij` CLI shape. |
| `skills/flow-pair/test/*.ts` | vitest specs targeting the pi-free lib (identity, ledger, context-pack, packet, review, learning). |
| `.flow-pair/runs/<run-id>/…` | Gitignored runtime ledger (generated data, not source). Layout: `run.json`, `events.jsonl`, `context-pack/`, `prompts/`, `diffs/`, `reviews/`, `learnings/`. Not enumerated in the domain manifest. |

## Concepts

| Concept | Description | Contract |
|---------|-------------|----------|
| Orchestrator / Worker seam | An expensive Pi session supervises; a cheap Pi session actuates one bounded packet per turn. | The orchestrator compiles the context pack, renders the packet, delegates, and reviews. The worker executes **only** within the allowed paths named in the packet. |
| Worker packet | A bounded task prompt delivered to the live worker session via a **pointer** (`pij send` path reference). | Packet is saved to the ledger first; only a short path pointer is sent over `pij-messaging`. Packet always includes the forbidden flow-state paths verbatim. |
| Worker report | The worker's structured reply (inline `[pij-rs from <id>] … [/pij]`), captured and recorded by the orchestrator. | Report shape is the hard schema in `schemas/worker-report.json`; orchestrator parses + records it. |
| Central ledger | Durable, append-only, cross-repo record of all runs, delegations, diffs, reviews, and learnings. | Lives under `.flow-pair/runs/<run-id>/` (gitignored). Core guarantee: append-only `events.jsonl`; discrete JSON per record; monotonic ids; atomic writes (persist-before-mutate, P9). |
| Run | Top-level work unit started by `flow-pair start`. | Creates `run.json` + `events.jsonl` with a `run.started` event. Run id is stable; repo identity is derived from git remote → `host-owner-repo` or basename+path-hash fallback. |
| Context pack | The minimal excerpt of the plan/tasks/logs the worker needs to succeed. | `context-pack.ts` includes only the stage contract + template + relevant plan sections + same-cluster learnings. Excludes unrelated clusters, other phases, secrets. |
| Observe / diff | Snapshot of what the worker changed after execution. | `observe.ts` writes `diff-NNNN.{patch,stat.txt,changed-files.json}` and appends a `files.changed` event. Guard: asserts no flow-state files in changed-files (defense-in-depth for AC-08). |
| Review verdict | Outcome of applying the 10-dimension rubric to the worker's output. | Dimensions: scope, contract, plan, ACs, tests, domain docs, progress log, regression, prompt-follow, learning. Possible verdicts: `APPROVE` / `APPROVE_WITH_NOTES` / `FIX_REQUIRED`. Missing `execution.log.md` ⇒ `FIX_REQUIRED` + `artifact_contract` finding. |
| Fix dossier / fix packet | A narrowed follow-up packet when review returns `FIX_REQUIRED`. | Allowed scope in the fix packet is **restricted to the files named in the review findings** — never broader than the original delegation. |
| Prompt cluster | A named category of delegation work whose learnings are isolated from all other clusters. | Each cluster has an `active.md` template, a `candidates/` folder, and a `changelog.md`. A learning note lands **only** in the cluster it was tagged to; manual promotion to `active.md` requires explicit approval. |
| Orchestrator decision protocol | The finite-state loop the orchestrator follows per turn. | States: `ASK_USER` / `RUN_LOCAL` / `DELEGATE` / `REVIEW` / `FIX` / `APPROVE`. Encoded in `skills/pij/references/routes/pair.md`; the expensive session never drops below this level of guidance. |
| Repo identity | A stable cross-session identifier for the target repo. | `identity.ts`: git remote → `host-owner-repo`; else `basename + path-hash`. Stable across cwd changes and session replacements. |

## Contracts

| Contract | Consumer | Shape / Guarantee |
|----------|----------|-------------------|
| `run.json` + `events.jsonl` | ledger consumers, orchestrator | `run.json`: `{ run_id, repo_id, repo_root, started_at, … }`; `events.jsonl`: one JSON object per line, append-only, monotonic `seq`. |
| `delegation.json` | orchestrator, review | `{ delegation_id, run_id, stage, phase, packet_path, allowed_paths, forbidden_paths, … }`. |
| `prompt-trial.json` | learning pipeline | `{ trial_id, delegation_id, cluster, template_version, … }`. |
| `review.json` | fix loop, learning | `{ review_id, delegation_id, verdict, findings: [{dim, severity, note}] }`. |
| `learning.json` | prompt-lab | `{ learning_id, trial_id, cluster, candidate_path, … }`. |
| Worker packet schema | worker session | Includes: mission, repo root, forbidden flow-state paths (verbatim), allowed scope, context pack pointer, template, report schema, stop conditions. |
| Worker report schema | orchestrator | Structured: `status`, `summary`, `files_changed`, `sections_present`, `grounding`, `discoveries`, `blockers`. Saved to ledger on receipt. |
| `flow-pair` CLI surface | SKILL.md, orchestrator | `flow-pair start|dispatch|observe|review|fix|accept|ledger`; all accept `--json` for machine-readable output; exit 0 = success, non-zero = error with structured stderr. Never imported into pi. |
| Cluster isolation contract | `lib/learning.ts` | A learning write for cluster C modifies **only** files under `prompt-lab/clusters/C/`; no other cluster directory changes. Enforced structurally by the writer. |
| Flow-state non-write contract | All components | No flow-pair component ever writes `.the-flow-state.json` / `the-flow.json` / `the-flow.md`. The worker packet forbids them. `observe.ts` asserts they are absent from `changed-files.json`. |

## Boundary Owns

- The orchestrator/worker delegation protocol: packet schema, report schema, allowed/forbidden-paths contract.
- Run/delegation/trial/review/learning record contracts and the event-log (`events.jsonl`) shape.
- Repo-identity derivation and the run-dir / path layout.
- The context-pack manifest contract (inclusion/exclusion rules, section extraction).
- The prompt-cluster taxonomy: cluster set, template lifecycle (`active` / `candidates` / `changelog`), isolation rule.
- The review rubric (10 dimensions) and verdict model (`APPROVE` / `APPROVE_WITH_NOTES` / `FIX_REQUIRED`).
- The fix-dossier shape and the narrow-scope fix-packet generation.
- The orchestrator decision protocol (`ASK_USER` / `RUN_LOCAL` / `DELEGATE` / `REVIEW` / `FIX` / `APPROVE`).
- The `flow-pair` CLI entrypoint (the skill's only invocation surface into logic).
- The runtime ledger root layout and gitignore contract.

## Boundary Excludes

- The SDD route graph and stage definitions — owned by `the-flow` (external); flow-pair wraps but never edits it.
- Flow-state files (`.the-flow-state.json` / `the-flow.json` / `the-flow.md`) — owned by `the-flow` guided mode; flow-pair **never writes** them.
- The live peer transport mechanism — owned by `pij-messaging`; flow-pair consumes `pij send` / inbound channel, never re-implements it.
- minih run lifecycle — external (analogous to how `agent-workbench` treats minih artifacts).
- Automatic/silent prompt-template promotion — manual approval required in v1; no auto-promote logic owned here.
- A/B prompt testing, per-model effectiveness tracking, decay — post-v1; not modelled in this domain.
- File-watcher automation for worker-change detection — deferred; v1 uses manual `observe`.
- Per-repo `the-flow` plan content — flow-pair reads plan excerpts for the context pack but does not own or edit plan files.

## Dependencies

- **This domain depends on (pij domains, consumed)**:
  - `pij-messaging` — worker-packet delivery and worker-report return over the live peer channel (`pij send` / inbound). No changes to pij-messaging.
  - `agent-tooling-interface` — the `flow-pair` skill surface and any `/flow-pair`-style intents are presented through the Pi skill/tool UX.
  - `extension-authoring-harness` — `just`/vitest/self-check validate the helper lib; retros + difficulty ledger + velocity log feed the self-improvement loop.
- **External (not pij domains)**:
  - `the-flow` — the inner SDD route authority; flow-pair wraps it but does not modify it or import its code.
  - `minih` — external agent runner; referenced only if dogfood runs use minih-style execution, not a code dependency.
- **npm**:
  - None planned for the pi-free helper lib (stdlib + Node built-ins only); `the-flow` CLI and `git` are assumed present in the environment.

## History

| Plan | Change | Date |
|------|--------|------|
| 016-flow-pair | Created the domain (planning). First inhabitant: `skills/flow-pair/`. | 2026-06-17 |
| 016-flow-pair | **Phase 1 built**: `lib/identity.ts`, `lib/paths.ts`, `lib/cli.ts`, `SKILL.md` + references/templates skeleton; `flow-pair` CLI (bin + npm link). | 2026-06-17 |
| 016-flow-pair | **Phase 2 built**: `lib/ledger.ts` (`LedgerWriter` + `appendLedgerEvent` + record writers, P9 persist-before-mutate), `schemas/*.json` (6 draft-07). Encoded test-quality Dimension 0 + `just flow-pair-mutate` after a cross-model review caught a CRITICAL P9 bug behind green gates. | 2026-06-17 |
| 016-flow-pair | **Phase 3 built**: `lib/context-pack.ts` (`ContextPackCompiler`: `extractSection` prefix-boundary match, `clusterLearnings` graceful-empty, `compile` P9-via-`appendLedgerEvent`, manifest carries content + forbidden-paths guard); `dispatch` CLI subcommand; `references/context-packs.md`. Tests mutation-proven; worker self-mutation caught a vacuous P9 test. | 2026-06-17 |
| 016-flow-pair | **Phase 4 built**: `lib/packet.ts` (`PacketRenderer`: single-pass template render, `delegationId` `/^dlg-\d{4}$/` validation + path-escape guard, P9 `packet.written` before write); `dispatch` chain prints pointer-only (orchestrator sends via `pij_send` tool — no CLI shell-out). Cross-model review caught schema + transport doc-drift. | 2026-06-17 |
| 016-flow-pair | **Phase 5 built**: `lib/observe.ts` (`Observe.capture`: `git status --porcelain=v1 -z` NUL parse, AC-13 flow-state guard by **basename** before any write, P9 `files.changed` event, synthetic patches for untracked files via `git diff --no-index`); `flow-pair observe` CLI. Cross-model review caught a quoted-path AC-13 bypass (fixed `-z`); fix verified end-to-end. | 2026-06-20 |
