# Packet: reviewer, PR #110 (s13, plan 191)

From pm-001 (pij-steady-otter), 2026-11-04. You hold the per-PR `reviewer` role for PR #110. Your standing packet is `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md`; follow its reading order.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/110, branch `plan/191-doc-procedures`, **head 4c5a792beab929a94a399a2a3cef57aad5696862**. Worktree (detached at that head, read-only): `~/pi-hacking/fleet-review-110`.
- Plan: `docs/plans/191-doc-procedures/plan.dd.json`, AC1 to AC4; evidence in `assets/evidence/`. Roadmap 001 step s13 (and s01's flip).
- Check, at minimum:
  1. The procedures in `skills/pij/references/community/` are correct: run the commands they give in a scratch copy, and confirm the paths, schema names and ordering rules match `.dd/schemas/pij/*` and `docs/project/` on this head.
  2. The worked examples validate (`ddocs validate`), link to each other, and are small.
  3. State flips (AC4): s01 and s13, req-18, req-23, c-doc-schemas, c-skill each match their own done-when; s01's done-when check is recorded. Roadmap steps use `complete` (the pij/roadmap enum has no `shipped`). The `needs` cells for s02, s13 and s14 are filled (req-24).
  4. The canary evidence supports AC3 (a seat with no prior context produced linked, valid docs), and its hesitations were folded in.
  5. No machine paths, private repo references or credentials in the added files.
  6. Mutation-check one guard you rely on: break an example (drop a required section) and confirm `ddocs validate` fails, then restore it.
- Not in scope: security (pij-wary-lynx) and cost (none: docs only).
- Tooling: the worktree has no node_modules and you may not install; use `~/pi-hacking/pij/node_modules/.bin/` for biome. CI is green on this head.

## Output (do not skip)

1. Write the verdict to `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-110-verdict.md`, and append one line to `roles/reviewer/outputs/LOG.md`: `<date> verdict pr-110 <head> <pass|pass-with-notes|block> — <notes>`.
2. Post it on the PR: `gh pr comment 110 --body-file <file>` starting `reviewer: pass|pass-with-notes|block (head 4c5a792beab929a94a399a2a3cef57aad5696862)`.
3. **Send me the report**: `pij send pij-steady-otter --body-file - <<'PIJ'` with the verdict, findings and the file path, then `PIJ`. **Your job is not done until this send has gone.** Writing the file is not enough.

Never commit to the governance branch (the prime commits); never push to the PR branch.
