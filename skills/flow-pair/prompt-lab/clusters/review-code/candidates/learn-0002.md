# Learning Candidate — learn-0002

- **Cluster**: review-code
- **Run**: 2026-07-17T01-03-56Z-github.com-AI-Substr
- **Delegation**: dlg-0001
- **Miss type**: review-code
- **Created at**: 2026-07-17T02:02:43.567Z

## Summary

Review packets must direct mutation testing through the underlying script with an explicit vitest target — the just flow-pair-mutate wrapper hard-codes skills/flow-pair/test/ and false-greens on any other file

## Evidence

- review-dlg-0001.md Dim-0 section
- DL-004

## Candidate prompt delta

Review template mutation instruction: use 'bash harness/scripts/flow-pair-mutate.sh <file> <sed> "npx vitest run <matching test file>"' until the wrapper takes a suite arg

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
