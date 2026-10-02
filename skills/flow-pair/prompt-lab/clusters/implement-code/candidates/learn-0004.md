# Learning Candidate — learn-0004

- **Cluster**: implement-code
- **Run**: 2026-07-05T03-45-37Z-github.com-AI-Substr
- **Delegation**: dlg-0001
- **Miss type**: implement-code
- **Created at**: 2026-07-05T04:31:49.766Z

## Summary

Coder built the --json join tuple correctly but dropped transcriptPath from the parallel human-text table — an AC-2 same-tuple parity gap the reviewer caught (HIGH).

## Evidence

- core/cli.ts sessions text renderer emitted pijId/harness/harnessSessionId/lifecycle/model/parent but not transcriptPath, while --json and --help advertised it.

## Candidate prompt delta

In worker-implement packets for a CLI verb with dual --json/text output of the SAME data, require explicit same-tuple parity: every field present in one view must appear in the other (null-rendered), or a text-only omission slips past --json tests.

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
