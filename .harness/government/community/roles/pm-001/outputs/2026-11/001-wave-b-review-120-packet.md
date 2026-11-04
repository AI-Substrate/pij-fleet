# Packet: reviewer, PR #120 (plan 197, bounded spine reads, closes #119)

From pm-001 (pij-steady-otter), 2026-11-04. Your standing packet: ~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/120, branch plan/197-bounded-spine-reads, **head 43821d5ba66ad7e6f46a976785d393f9f4af0c15**. Worktree (detached, read-only, frozen): ~/pi-hacking/fleet-review-120. Never move it; if the head moves I'll give you a new one.
- Plan: docs/plans/197-bounded-spine-reads/plan.dd.json, AC1 to AC6. Issue #119 explains the bug: anomalies, node show and spine events folded the whole spine (4.18M rows) on every call.
- Check, at minimum:
  1. **AC3 equivalence**: the new indexed anomalies view equals the old full fold. The PR keeps the old fold as a test oracle (indexed_reads_answer_exactly_as_the_whole_spine_fold); check the fixtures cover stale cards, tombstones, revives, many seats, scoped and unscoped. Mutation-check: break one indexed read (for example drop a kind) and confirm the oracle test fails.
  2. **AC1, AC2**: node show and spine events use newest() and pages; --limit default 500, max 5000 (refused above); --peer uses the seat index; the cursor is right at page boundaries.
  3. **AC4 bounded**: tests/spine_attach_cost.rs (1,000,073 rows), EXPLAIN QUERY PLAN showing SEARCH, and the guard against tail(None, ...). Kestrel flags two extras: latest_matching now seeks per kind (a multi-kind IN used to walk spine_by_seat), and the 1M-row test lives in tests/ because a direct-insert guard polices src/.
  4. No other caller relied on the old unbounded `spine events --since` (grep callers in crates/, .omp/, harness/, skills/).
- Not in scope: security (pij-wary-lynx), cost (pij-frugal-wombat).
- **Builds: the shared target is locked.** Use CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target. Before ANY cargo command take the lock (`until mkdir /tmp/pij-shared-target.lock 2>/dev/null; do sleep 20; done`, then write "<your seat> <time>" to /tmp/pij-shared-target.lock/owner), touch crates/*/src/lib.rs and crates/*/src/main.rs in your worktree, run your whole cargo batch with CARGO_INCREMENTAL=0 and -p scoping, then `rm -rf /tmp/pij-shared-target.lock` (use a trap). Two trees building at once corrupt each other's results. Check `df -h /System/Volumes/Data` first; stop under 25 GB.

## Output (do not skip)
1. Verdict file ~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-120-verdict.md plus a LOG line (no ~/.copilot, ~/.claude or ~/Library paths in it).
2. `gh pr comment 120 --body-file <file>` starting "reviewer: pass|pass-with-notes|block (head 43821d5ba66ad7e6f46a976785d393f9f4af0c15)", with a severity and a concrete fix per finding.
3. **Send me the report**: `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. **The job isn't done until that send has gone.** Close on genuine blockers only.
Never commit or push.
