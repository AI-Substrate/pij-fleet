# Learning Candidate — learn-0001

- **Cluster**: implement-code
- **Run**: 2026-07-11T09-19-34Z-github.com-AI-Substr
- **Delegation**: dlg-0001
- **Miss type**: implement-code
- **Created at**: 2026-07-11T10:17:38.225Z

## Summary

Whole-phase TDD packet to gpt-5.6-sol: 2 live defects hid in untested branches (null-HEAD pin bypass; sticky failureReason) — both were edge-of-contract branches the packet's AC list named but didn't force tests for. Candidate delta: packet template should demand a negative-space test per AC ('name the test for the branch where this guard CANNOT be checked')

## Evidence

- reviews/review.phase-1.dlg-0001.md R1 F1/F3 probes
- R3 APPROVE after 2 fix cycles

## Candidate prompt delta

Whole-phase TDD packet to gpt-5.6-sol: 2 live defects hid in untested branches (null-HEAD pin bypass; sticky failureReason) — both were edge-of-contract branches the packet's AC list named but didn't force tests for. Candidate delta: packet template should demand a negative-space test per AC ('name the test for the branch where this guard CANNOT be checked')

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
