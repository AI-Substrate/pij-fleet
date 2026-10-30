# Plan 187: prime verification

Prime `pij-far-jackal`, 2026-10-30. Recorded by the coder seat (`pij-replay-stream`) from the prime's report.

- **Review:** independent seat `pij-review-pr102` at `0b05852`: merge after fixes, 0 blockers, 3 should-fix, 4 nits ([review-pr102.md](review-pr102.md)).
  - **S1:** a failed replay page ended only the local leg, so with a federated leg merged in, the 200 stayed open and silent.
  - **S2:** nothing witnessed "live receiver before the first page"; the reviewer's mutant survived.
  - **S3:** a lag after the final page skipped events silently. Prime ruling: end the stream, no wire change (R4).
  - All three were fixed at `b207016`, each with a test that was RED first (`evidence/review-red.txt`) and mutants (`evidence/mutants-review.txt`, 10/10 killed). N1–N4 were fixed too.
  - Recheck at `b207016`: merge, 0 blockers, 0 should-fix ([review-pr102-recheck.md](review-pr102-recheck.md)). Its residual nit R1, a live-only attach with no `since` that still skips after a lag (a consumer's first connection only), is filed as [issue #103](https://github.com/AI-Substrate/pij-fleet/issues/103).
- **Merge:** PR #102 merged at head `b207016` (CI run 41032623616 green) as main `4dc485f`.
- **Deploy:** daemon redeployed, pid 63433 → 67958 at 19:33:22.

## Live reading (ac-0005)

| | before (pid 63433) | after (pid 67958) |
|---|---|---|
| CPU (`top`) | 195 % (~160–205 % since the plan 185 deploy) | +15 s 94 %, +60 s 33 %, **+10 min 16 %** |
| Memory (`top`) | 2.2 GB (swinging 2.9–4.1 GB earlier) | +15 s 153 MB, +60 s 38 MB, **+10 min 38 MB** |
| Daemon launches | 0.21/s | 0.21/s |
| chainglass | connected, 0 frames per attach (private replay) | connected (2 node connections) |

- **Fixed live:** CPU and memory. The plan 185 targets (idle CPU under 50 % of a core; RSS flat and under 1 GB), carried into this plan as its baseline, are met: 16 % and 38 MB at +10 min, flat from +60 s.
- **No new launches:** the launch rate is unchanged at 0.21/s.
- **Verdict:** ac-0001–ac-0006 met; phase ph-90cf closed; plan shipped. Residual R1 tracked in #103.
