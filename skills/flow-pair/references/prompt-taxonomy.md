# Prompt Taxonomy

Phase 7 source of truth for prompt-learning cluster names, scopes, and lifecycle.
`skills/flow-pair/lib/learning.ts` must use the same canonical cluster set; do not invent a cluster in code without updating this taxonomy and tests first.

## Canonical clusters

| Cluster | Scope | Out of scope |
|---------|-------|--------------|
| `implement-code` | Worker prompts for implementing planned code/tasks, including TDD-first implementation packets. | Review verdicts, fix packets, and validation-runner instructions. |
| `fix-code` | Worker prompts for addressing review findings or targeted fix packets. | Initial implementation strategy and independent codebase research. |
| `review-code` | Reviewer prompts, rubrics, finding quality, and verdict synthesis. | Worker implementation instructions or candidate promotion. |
| `docs-writing` | Documentation, task files, execution logs, and reference/template writing. | Code mutation strategy and test execution policy. |
| `codebase-research` | File discovery, source-truth grounding, and context-pack research instructions. | Implementation, review verdicts, and validation command operation. |
| `validation-runner` | Validation, mutation gates, test-count reporting, and evidence capture. | Prompt candidate authoring outside validation lessons. |

## Lifecycle

Each cluster has exactly this layout:

```text
prompt-lab/clusters/<cluster>/
  active.md
  candidates/
  changelog.md
```

- `active.md` is the current, manually curated prompt guidance for that cluster.
- `candidates/` contains generated `learn-NNNN.md` candidate notes for that cluster only.
- `changelog.md` records manual promotion, rejection, or deferral decisions.

## Isolation policy

- A learning candidate for `implement-code` may write only under `prompt-lab/clusters/implement-code/candidates/`.
- Sibling clusters must remain byte-identical during a candidate write.
- Traversal-like cluster values (`../x`, `/abs`, `a/../b`, empty strings) are invalid.
- In v1, `missType` must equal `cluster`; this one-to-one rule is intentionally conservative and keeps cross-cluster leakage fail-closed.

## Promotion policy

No automatic promotion in v1. `lib/learning.ts` must never edit `active.md` or `changelog.md` programmatically when recording a candidate. A human/orchestrator reviews a candidate note, edits `active.md` if accepted, and records the decision in `changelog.md`.
