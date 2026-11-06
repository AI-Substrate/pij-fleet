# Packet: reviewer, PR #129 (s05, plan 199: send to a role, the role queue and reply modes)

From pm-001 (pij-steady-otter), 2026-11-06. Standing packet: ~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md. Rulings C1, C2, C3, C4, C5, C6 and C26 in RULINGS.md.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/129, **head fd7bf4141420f132974ece403faa5ae8c64c9982** (35 files, +2987). Worktree (detached, read-only, frozen): ~/pi-hacking/fleet-review-129.
- Plan: docs/plans/199-send-to-role/plan.dd.json, AC1 to AC7. This is the critical path: s06, s12 and s09 build on it, so a fast, precise verdict matters most here.
- Check, at minimum:
  1. Resolve (C1): `role:<id>` and `role:<id>/<instance>` reach exactly one live holder through the directory; a directory and seat_roles disagreement refuses with a named mismatch; the receipt prints the request id (C3).
  2. Queue and wake (C2): no live holder queues the request (migration 0034, role_requests) and wakes the PA (else the prime) once, with both runnable next steps and the old seat's context size; an --fyi send queues too; no new timer.
  3. Drain (C6): an assign drains that role's queue in order through RoleQueueDrain (lib.rs now wires it in place of NoHook); a refused assign drains nothing.
  4. Several holders refuse and list them. Reply mode and limit are stored (C5 default_limit).
  5. Seams other lanes use: http::background, pa_watchdog::wake_pa, GovernanceService::store and directory::project_and_repo are now pub(crate). Check nothing else widened.
  6. The project-doc state flips were edited in place (10 lines), not through ddocs set: check `ddocs validate` and `ddocs build --check` on PRD 001 and roadmap 001 are clean.
- Known pre-existing flake (not this PR): directory_http::review::review_reassign_within_one_second_keeps_both_holdings (#122's F003). The prime's rule: it's deleted in a one-line PR on its first CI failure. Don't block on it; say whether this PR touches it.
- **Load is about 40, so lean on CI evidence.** Build only what a finding needs: `PIJ_SHARED_TARGET=/Users/jordanknight/pi-hacking/fleet-190-community-writer/target just shared-build -- <cargo args>` (kestrel's target, so you queue behind it), scoped with -p. Scratch repos and a private daemon only.

## Output (do not skip)
1. Verdict file ~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-129-verdict.md plus a LOG line.
2. `gh pr comment 129 --body-file <file>` starting "reviewer: pass|pass-with-notes|block (head fd7bf4141420f132974ece403faa5ae8c64c9982)", with a severity and a concrete fix per finding.
3. **Send me the report** with `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. **The job isn't done until that send has gone.** Close on genuine blockers only. Never commit or push; never use a question UI.
