# Plan 183: prime verification

Prime `pij-far-jackal`, 2026-10-28.

- **Review:** independent seat `pij-review-pr83`.
  - At 15c28e2: 1 blocker (B1, staged-copy release), 1 should-fix, 4 nits.
  - At 77a48a5: **MERGE**, 0 blockers. B1 and S1 are fixed at one seam, and the reviewer's probes now pass.
  - Follow-ups, not blocking: W1 (restart backoff caps unpinned) and R1 (a delivered-copy removal can race an identical resend: a spurious terminal failure, nothing lost).
- **Merge:** PR #83 merged at head `77a48a5` (CI run 40663843738 green) as main `17dea30`. No migration or schema change. `refs/notes/*` on origin empty. #77 closed.
- **Deploy:** `just bounce-rs`, daemon pid 41991 → 30264, ping healthy.
- **Live canary:** `pij-rs sidecar telegram --body … --file pij-telegram-test.png --file canary-2.png --caption "1 of 2"`.
  - Enqueue returned `ok`.
  - The spine shows the body binding at 1793180040094, then `telegram.outbound-media-delivered` at 1793180042485 (file 1) and 1793180046691 (file 2). Body first, files in order.
- **Cost, after:** two `pij-rs --json ping` readings 20 s apart show no telegram op and no new launch site. The daemon total of 16/s is liveness `ps` (plan 182, pending) plus unrelated held deliveries (#86).
- **Verdict:** ac-0006 and ac-0008 are met, and the phase can close.
