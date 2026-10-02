# Learning Candidate — learn-0001

- **Cluster**: review-code
- **Run**: 2026-07-05T01-16-56Z-github.com-jakkaj-Fr
- **Delegation**: dlg-0001
- **Miss type**: review-code
- **Created at**: 2026-07-05T02:55:53.210Z

## Summary

A review packet that prescribes a literal geometric algorithm as the fix (all non-endpoint bbox cells non-solid) can be self-defeating against domain invariants (lip footings are necessarily solid and inside the box); prescribe the contract OUTCOME (off-corridor solids occlude; footings do not) and name the domain invariant, letting the coder derive the exact rule

## Evidence

- rev-0001 fix text forced a mid-fix contract renegotiation: coder proved the literal rule bakes zero links (two existing green tests: CeilingLip support bump, 4-drop take-off pillar)
- orchestrator ruling + rev-0002 adjudication resolved it as contracted-bbox (support columns + Chebyshev<=1 excluded)

## Candidate prompt delta

In review packets, when prescribing a fix for a geometric/spatial contract, state the required OUTCOMES and known domain invariants (what is necessarily solid/non-solid) instead of a literal cell-enumeration rule

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
