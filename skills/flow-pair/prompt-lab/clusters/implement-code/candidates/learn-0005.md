# Learning Candidate — learn-0003

- **Cluster**: implement-code
- **Run**: 2026-07-17T01-03-56Z-github.com-AI-Substr
- **Delegation**: dlg-0003
- **Miss type**: implement-code
- **Created at**: 2026-07-17T04:50:14.861Z

## Summary

Proof/e2e runner scenarios must be load-bearing like unit tests: assert content identity (exact tail/order/non-empty), counts across BOTH edges of an episode (negatives after each intermediate tick), and every named CLI surface verbatim — existence + upper-bound assertions pass vacuously under sabotage

## Evidence

- dlg-0003 round 1: AC-07 passed with zero-byte captures (upper-bound-only asserts)
- AC-05/06 passed with early failureReason clear (no negatives after attributed ticks 401/402)
- AC-03 never invoked pij watchdog list
- same class as P2 C3 (enum-vs-count)

## Candidate prompt delta

When a packet's deliverable is a proof/e2e runner, require per-scenario: (1) content-identity assertions with deterministic markers, never bare existence/caps; (2) negative assertions at every intermediate state, not just the terminal one; (3) each CLI surface the AC names invoked verbatim; (4) coder self-sabotage: break each seam, show RED, restore byte-identical with sha

## Promotion status

Pending manual review. No automatic promotion: do not edit `active.md` automatically. Record any promotion decision in `changelog.md`.
