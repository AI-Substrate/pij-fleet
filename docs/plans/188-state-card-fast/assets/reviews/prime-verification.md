# Plan 188: prime verification

Prime `pij-far-jackal`, 2026-11-02. Recorded by the coder seat (`pij-state-cards`) from the prime's report.

- **Review:** the independent seat `pij-review-pr105` reviewed `96421ee`: merge after fixes, 0 blockers, 3 should-fix, 4 nits ([review-pr105.md](review-pr105.md)).
  - **S1:** the bulk timing was measured with sessions nulled, and one slow transcript could hold the whole `/v1/cards` response.
  - **S2:** one bad seat turned `/v1/cards` into a 500.
  - **S3:** nothing pinned "latest by sequence, not timestamp".
  - All three were fixed at `4d4e98e`. Each test was red on `96421ee` first (`evidence/red-review.txt`), and the mutants are in `evidence/mutants-review.txt` (6/6 killed). N1–N3 were fixed too; N4 is [issue #106](https://github.com/AI-Substrate/pij-fleet/issues/106).
  - The recheck of `4d4e98e`: merge, 0 blockers ([review-pr105-recheck.md](review-pr105-recheck.md)). It raised two nits, fixed in the close-out:
    - **R1:** nothing pinned `/v1/state` as unbounded. Now `only_the_bulk_read_bounds_the_session_read`; the mutant `state-also-bounded` turns it red (`evidence/mutants-recheck.txt`).
    - **R2:** the docs said folds run four at a time. They now say the waits are bounded and an overrunning fold finishes detached.
- **Merge:** PR #105 merged at head `4d4e98e` (CI run 41371773550 green) as main `cf4baa7`.
- **Deploy:** daemon redeployed, pid 67958 → 97258.

## Live reading (ac-0005)

| | before (pid 67958) | after (pid 97258) |
|---|---|---|
| `/v1/state` pij-far-jackal, pij-lonely-antelope | 1.73–1.74 s | first read: far-jackal 0.01 s, antelope 0.87 s (cold fold); warm: antelope 0.57 s, then 0.00 s ×3; far-jackal 0.01 s |
| `/v1/state` pij-different-smelt (3.59 M rows) | 15.3 s (diagnosis) | 0.49 s cold, 0.03 s warm |
| `GET /v1/cards` | no route | call 1 (cold) 1.31 s: 88 cards, 0 unavailable, 4 timed out; call 2 0.21 s, 0 timed out; call 3 0.055 s, 0 timed out |
| Daemon | | 10.6 % CPU, 197 MB |

- **Fixed live:** a card is no longer a spine scan. The seconds per call are gone, and the 3.59 M-row seat is sub-second cold. What remains on a first read is the session-status fold, once per bound seat and incremental after that.
- **Bulk:** chainglass can seed all 88 cards in one call: 1.31 s cold, 55 ms warm. Before, it took 84 sequential calls and 136 s.
- **Verdict:** ac-0001–ac-0006 met; phase ph-4cc4 closed; plan shipped. N4 is tracked in #106.
