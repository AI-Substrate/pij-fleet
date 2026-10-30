# Plan 185: prime verification

Prime `pij-far-jackal`, 2026-10-30. Recorded by the coder seat (`pij-daemon-burn`) from the prime's report.

- **Review:** independent seat `pij-review-pr99` at `3f02e3f`: merge after fixes, 1 blocker, 1 should-fix, 6 nits ([review-pr99.md](review-pr99.md)).
  - **B1:** a tombstoned owner's final turn was dropped for good, but resume and revive lift tombstones. Prime ruling: no drop.
  - **S1:** the "once per boot" sweep re-ran every tick while any pane failed.
  - Both were fixed at `9f7b36e`, each with a test that was RED first and a mutant. N1–N4 and N6 were fixed too. N5 was declined with a reason the recheck accepted.
  - Recheck at `9f7b36e`: merge, 0 blockers ([review-pr99-recheck.md](review-pr99-recheck.md)).
- **Merge:** PR #99 merged at head `9f7b36e` (CI run 41023465167 green) as main `32b136e`.
- **Deploy:** daemon redeployed, pid 72069 → 63433.

## Live reading (ac-0004)

| | before (pid 72069, 18:10:58) | after (pid 63433) |
|---|---|---|
| CPU | 260 % | **~160–205 % (`top`): not fixed** |
| RSS | 12.4 GB | **2.9–4.1 GB, swinging: not fixed** |
| `~/.pij-rs/pane-signals` | 2.9 GB | 3.0 MB |
| Spine rows since deploy | — | 22, none `bg.lost` |
| Daemon launches | — | 0.21/s |
| `daemon.log` | 751 MB | — |

- **Fixed live:**
  - **Taps (F5).** The first sweep logged "removed 150 sink(s), 3143183942 bytes …; claimed 234 pre-ledger sink(s); spared 0". `pane-signals` went from 2.9 GB to 3.0 MB.
  - **The `bg.lost` flood (F2).** 22 spine rows since deploy, none of them `bg.lost`.
- **Not fixed live: CPU and RSS.** A `sample` of pid 63433 still puts the time in `/v1/events`: `subscribe` → `SqliteSpine::tail` → full decode, plus the sqlx workers.
  - F1 fixed the head lookup, not the replay. A consumer whose cursor is far behind still has its whole replay decoded and collected in one `fetch_all`.
  - Prime's hypothesis (not yet proven): chainglass's cursor is stuck behind the 2.4 M `bg.lost` rows. Each ~5 s recycle replays from there, the collected replay takes longer than the recycle, so the cursor never advances.
  - The private replay in this plan used a client 10 events behind the head, so it could not show this. **Handed to plan 187** (`pij-replay-stream`, branch `perf/events-replay-stream`).
- **Not as designed: the waiting-source refusal still repeats.** "background reconciliation failed: … its final turn waits" was logged 439 times in about 8 minutes (~0.9/s, down from 10/s, but not once). `RepeatedFailureLog` only suppresses an exact repeat of the *previous* tick's first error, and it forgets on a clean tick.
  - Not verified here: either the first error alternates between jobs, or ticks without a failure clear the memory.
  - Plan 187 ac-0004 owns the fix.
- **Verdict:** ac-0001–ac-0003, ac-0005 and ac-0006 met. ac-0004 is met for tap growth and spine growth, which were fixed and measured live. Its CPU and RSS targets (under 50 % of a core; RSS flat and under 1 GB) were **not** met live and move to plan 187, with the reading above as their baseline. Phase ph-9a02 closed; plan shipped.
