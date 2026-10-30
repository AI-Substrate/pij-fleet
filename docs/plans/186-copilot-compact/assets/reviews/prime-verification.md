# Plan 186: prime verification

Prime `pij-far-jackal`, 2026-10-30. Recorded by the coder seat from the prime's report.

- **Review:** independent seat `pij-review-pr95` at `c0258a2`: merge after fixes, 0 blockers, 4 should-fix, 3 nits ([review-pr95.md](review-pr95.md)).
  - S1 (an abandoned control claim could wedge native bodies), S2 (stale skill lines), S3 (single-read busy brake) and S4 (ordering docs), plus N1 (a real Copilot draft frame), were fixed at `c1f8314`. N2 (a draft with the caret not at the end reads as empty; affects Claude too) is filed separately. N3 was left as is.
  - Recheck at `c1f8314`: merge, 0 blockers, 0 should-fix, 1 nit ([review-pr95-recheck.md](review-pr95-recheck.md)). The nit (C3 and docs said "check `pij state`") is fixed in this close-out: the signal is the compact's `delivery.outcome` event or "Compaction completed" in the pane.
- **Merge:** PR #95 merged at head `c1f8314` (CI run 41012090018 green) as main `30d92a9`. No migration. #94 closed by the PR.
- **Deploy:** daemon redeployed, pid 39680.
- **Live canary on the real fleet:** Copilot seat `pij-c186-live`. The prime sent `--command compact` 6 s into a turn.
  - Receipt `queued — pty`.
  - Held while the seat worked, then typed after idle.
  - The pane shows "● Compaction completed / A new checkpoint has been added".
  - The canary was closed and its window killed.
- **Cost (ac-0004):** fleet-wide spawn total went 256 → 316 over about 4 minutes (`pij-rs --json ping`). That includes the canary's own spawn and turn, so it is an upper bound for the compact, not its cost. The per-compact cost is in the branch canary ([canary.md](../evidence/canary.md) § Cost): 13 launches per delivered compact, all at existing call sites, and 0 per busy hold.
- **Verdict:** ac-0001…ac-0006 met. Close phase ph-fa03; plan done.
