# Learning Synthesis Template

Use this template to write a prompt-learning candidate note after a delegation or review reveals a reusable lesson. The candidate belongs to exactly one prompt cluster and must not be promoted automatically.

```markdown
# Learning Candidate — {{LEARNING_ID}}

- **Cluster**: {{CLUSTER}}
- **Run**: {{RUN_ID}}
- **Delegation**: {{DELEGATION_ID}}
- **Miss type**: {{MISS_TYPE}}
- **Created at**: {{CREATED_AT}}
- **Reviewer disposition**: pending manual review

## What failed or surprised us

{{SUMMARY}}

## Evidence

{{EVIDENCE_LIST}}

## Candidate prompt delta

{{CANDIDATE_DELTA}}

## Suggested active.md insertion point

{{SUGGESTED_INSERTION_POINT}}

## Promotion status

Pending manual review.

No automatic promotion: do not edit `active.md` programmatically. If a human accepts this candidate, update the cluster's `active.md` manually and add a dated entry to `changelog.md` explaining the decision.
```

## Field guidance

- `CLUSTER`: one of the canonical slugs in `<flow-pair skill root>/references/prompt-taxonomy.md` (the skill's install root — not the consuming repo).
- `MISS_TYPE`: currently redundant in v1 because it must equal `CLUSTER`; keep it to support future many-to-one attribution.
- `EVIDENCE_LIST`: concrete review findings, failed mutation gates, command output, or artifact paths.
- `CANDIDATE_DELTA`: the prompt text or instruction that might improve the cluster's future behavior.
- `SUGGESTED_INSERTION_POINT`: where a reviewer should consider applying the candidate inside `active.md`.
