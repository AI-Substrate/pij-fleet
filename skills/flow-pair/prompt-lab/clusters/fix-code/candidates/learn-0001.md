# Learning Candidate — learn-0001

- **Cluster**: fix-code
- **Run**: 2026-07-27T07-25-53Z-github.com-AI-Substr
- **Delegation**: dlg-0001
- **Miss type**: fix-code
- **Created at**: 2026-07-27T08:53:40.683Z

## Summary

A diagnostic added alongside a fix needs the same adversarial treatment as the fix: BlinkProbe shipped able to exit 0 having measured nothing (unposed/unsaved captures ignored) — the exact false-clean class the coder had himself documented in a comment. Counting owed artifacts (38/2 frames) beats bool-checking; demonstrate the detector both ways (firing AND the old behaviour reproduced with the failure path mutated out).

## Evidence

- dlg-0001 review F1 (HIGH) + round-2 exit-code demonstrations: unwritable dir fix-in-place exit 8 0/2
- failure path mutated out exit 0 (old false-clean reproduced)
- full sweep 38/38 exit 0

## Candidate prompt delta

Worker packets that add measurement/diagnostic code must require: success = counted owed artifacts, non-zero exit on shortfall, and a demonstrated-failing run before the clean run is credited.

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
