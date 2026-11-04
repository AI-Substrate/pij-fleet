# Packet: reviewer, PR #111 (s14, plan 192)

From pm-001 (pij-steady-otter), 2026-11-04. You hold the per-PR `reviewer` role for PR #111. Your standing packet is `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md`; follow its reading order, and read ruling C15 (and C11, C12, C17) in `RULINGS.md`.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/111, branch `plan/192-prd-register-move`, **head ed41a254612d47432349656831e7804baed302c0**. Worktree (detached at that head, read-only): `~/pi-hacking/fleet-review-111`.
- Plan: `docs/plans/192-prd-register-move/plan.dd.json`, AC1 to AC6; evidence in `assets/evidence/` (triage.md, check.py, scrub.txt).
- What this PR is: pij's earlier PRD register (in a private repo) is **not** moved. Its 40 open rows were triaged against today's code; 23 kept, written fresh as 20 rows of the new `docs/project/prds/002-pij-base.dd.json`; 12 dropped as met; 5 dropped as obsolete.
- The source, read-only: in `~/pi-hacking/pij-legacy`, `git show e05661c8:.harness/government/prd/base-prd.dd.json`. Never write there. Never paste its text into the PR, your verdict comment or any pij-fleet file (C15); refer to rows by old id only.
- Check, at minimum:
  1. Run `python3 docs/plans/192-prd-register-move/assets/evidence/check.py` as the evidence says and confirm 40/40, 0 shared runs, 0 scrub hits. Then mutation-check it: plant one 8-word run from the source into a scratch copy of PRD 002 and confirm it is counted, then discard the copy.
  2. Spot-check verdicts against today's code: at least 3 kept, 3 dropped as met (find the code that meets each) and 2 dropped as obsolete. A "met" verdict with no code behind it is a finding.
  3. PRD 002 rows: statements in new words, testable done_when, priority and state per C11 and C12; req-20 per C17.
  4. PRD 001 and roadmap 001 after the rebase: no changelog rows lost from main, s13 and s14 `complete`, req-25 shipped with C15's wording, source paths corrected. AGENTS.md and bootstrap.md point at PRD 002; no `prd/base-prd` pointers left.
  5. docs/difficulties.md D-192-1 to D-192-3 are accurate.
- Not in scope: security (pij-wary-lynx); cost (none, docs only).
- Tooling: no node_modules here and you may not install; use `~/pi-hacking/pij/node_modules/.bin/` for biome. CI is running on this head; check it's green before your verdict.

## Output (do not skip)

1. Write the verdict to `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-111-verdict.md` and append one line to `roles/reviewer/outputs/LOG.md`.
2. Post it: `gh pr comment 111 --body-file <file>`, starting `reviewer: pass|pass-with-notes|block (head ed41a254612d47432349656831e7804baed302c0)`. No source text in it.
3. **Send me the report**: `pij send pij-steady-otter --body-file - <<'PIJ'` with the verdict, findings and file path, then `PIJ`. **Your job is not done until this send has gone.**

Never commit to the governance branch; never push to the PR branch.
