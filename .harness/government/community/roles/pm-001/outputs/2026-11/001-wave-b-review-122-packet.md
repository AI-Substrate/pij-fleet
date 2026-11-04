# Packet: reviewer, PR #122 (s04, plan 195: assign, release, lineage and the directory view)

From pm-001 (pij-steady-otter), 2026-11-04. Standing packet: ~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md. Rulings: C1, C6, C8, C9, C13, C16, C21 and C22 in RULINGS.md.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/122, branch plan/195-assign-lineage, **head 94592ad6a0ba14a97c8ae1ad2d930c4ba7185ff3**. Worktree (detached, read-only, frozen): ~/pi-hacking/fleet-review-122. Never move it.
- Plan: docs/plans/195-assign-lineage/plan.dd.json, AC1 to AC15; proofs in assets/backpressure.dd.json; evidence in assets/evidence/. The dogfood findings it fixes: governance roles/docs-system/outputs/2026-11/wave-b-dogfood.md.
- Check, at minimum:
  1. **AC4 close never fails**: close of an assigned seat ends its holder rows; with the governance worktree missing or a foreign edit present, close still tombstones, the end stays pending and is named in the receipt, and the next directory write flushes it. Mutation-check one guard.
  2. **AC2, AC3 (C1), AC5 (C6)**: assign writes holders.dd.json through the writer; seat_roles is set in the same transaction (pa; pm for pm or pm-<n>; else worker; never re-role a prime); the hook fires once per successful assign and never on a refusal.
  3. **AC6 rename**: one recorded tmux launch per assign by absolute path (not the control client, whose read allowlist S2 must be unchanged); windows are targeted by id, not by the `<project>:<role>` name.
  4. **AC7, AC8**: the view is project-scoped and launches nothing; the rebuild gives byte-identical `pij directory --json`.
  5. **AC13 to AC15 (dogfood)**: adopting an existing role folder keeps memory/ and outputs/ untouched; "exists" fires only on role.dd.json and lists every conflict with a next step; the foreign-edit check covers only paths being written; linked worktrees resolve to their project, and the no-project refusal names `pij project create`.
  6. holders and external schemas embedded (layout.rs SCHEMAS), the `to`/`released_by` pairing enforced, and the doc flips (s04 complete, req-01/06/16/17 shipped, req-26 note).
- Not in scope: security (pij-wary-lynx), cost (pij-frugal-wombat).
- **Builds: the shared target is locked.** CARGO_TARGET_DIR=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target, CARGO_INCREMENTAL=0. Before ANY cargo command: `until mkdir /tmp/pij-shared-target.lock 2>/dev/null; do sleep 20; done`, write "<your seat> <time>" to /tmp/pij-shared-target.lock/owner, touch crates/*/src/lib.rs and main.rs in your worktree, run the whole batch scoped with -p (never --workspace), then `rm -rf /tmp/pij-shared-target.lock` in a trap. Check `df -h /System/Volumes/Data` first and between commands; stop under 25 GB. Use a private daemon and scratch repos only.

## Output (do not skip)
1. Verdict file ~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-122-verdict.md plus a LOG line (no ~/.copilot, ~/.claude or ~/Library paths).
2. `gh pr comment 122 --body-file <file>` starting "reviewer: pass|pass-with-notes|block (head 94592ad6a0ba14a97c8ae1ad2d930c4ba7185ff3)", with a severity and a concrete fix per finding.
3. **Send me the report** with `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. **The job isn't done until that send has gone.** Close on genuine blockers only. Never commit or push.
