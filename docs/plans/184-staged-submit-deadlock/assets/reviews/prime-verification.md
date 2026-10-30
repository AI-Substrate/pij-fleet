# Plan 184: prime verification

Prime `pij-far-jackal`, 2026-10-30.

- **Reviews** by independent seat `pij-review-pr91`:
  - [`review-pr91.md`](review-pr91.md) at `cff8819`: merge after fixes, 1 blocker (B1), should-fix S1–S5, nits N1–N4. All were fixed at `6c1fe6e` and rebased onto main `8687560` (plan 186).
  - [`review-pr91-recheck.md`](review-pr91-recheck.md) at `9edfee0`: merge, 0 blockers.
  - Two recheck items are closed in this close-out PR:
    - the S3 wrap-fit docs now say the rule measures against the widest row shown, so a newline typed in the second half of that row still passes; the delivery then fails by name, and nothing but that whitespace is deleted;
    - the flaky `stale_watchdog_cannot_release_a_later_adapter_transaction` is now deterministic.
- **Merge:** PR #91 merged at head `9edfee0` (CI run 41018304298 green) as main `8b6fd3a`. `refs/notes/*` on origin empty.
- **Deploy:** daemon redeployed, pid 39680 → 72069.
- **Live canary:** `pij send pij-review-pr91 --command compact` gave receipt `queued — pty` and committed on the first try. The pane's context went to 0%. One `/compact` cost one each of:
  - `tmux.atomically reserve staged submit`
  - `tmux.authorize staged submit commit`
  - `tmux.submit authorized staged body`
  - `tmux.restore pane input and release ownership`
- **Cost (ac-0004):**
  - **Before:** the two stuck seats cost about 0.8 launches/s (reserve and restore at 0.4/s each; diagnosis.md).
  - **After, steady state over 60 s:** 0.22 launches/s in total. The only ops were `ps.registration-native-process` (12) and `git.pa-watchdog-common-dir` (1).
  - No staged-submit reserves while nothing is deliverable.
- **The stuck seats:**
  - Walrus was closed by its prime before the deploy; its queue had already failed as tombstoned.
  - Opossum's Copilot compact (job 13096) finished.
- **Verdict:** ac-0004 met. Close bp-0002 and the phase.
