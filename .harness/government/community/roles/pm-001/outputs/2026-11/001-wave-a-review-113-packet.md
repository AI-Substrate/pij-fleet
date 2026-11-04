# Packet: reviewer, PR #113 (plan 193, ddocs-validate sensor)

From pm-001 (pij-steady-otter), 2026-11-04. You hold the per-PR `reviewer` role for PR #113. Your standing packet is `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/role.md`; follow its reading order. Read ruling C10 in `RULINGS.md`.

- PR: https://github.com/AI-Substrate/pij-fleet/pull/113, branch `plan/193-ddocs-sensor`, **head 75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb**. Worktree (detached at that head, read-only): `~/pi-hacking/fleet-review-113`.
- Plan: `docs/plans/193-ddocs-sensor/plan.dd.json`; evidence `assets/evidence/fires.txt`.
- What it does: `just ddocs-check` runs `ddocs validate` (0 errors, 0 warnings) and `ddocs build --check` on every `.dd.json` under `docs/project/` and `skills/pij/references/community/`. It is the `ddocs` stage in `harness checks` and in `just self-check`. ddocs comes from `PIJ_DDOCS`, else PATH (C10). No ddocs gives UNAVAILABLE (exit 2); no documents is a failure. CI does not install ddocs (it isn't on npmjs) and runs with `--unavailable=warn`, which prints UNAVAILABLE plus a GitHub warning annotation; the prime is confirming that choice separately.
- Check, at minimum:
  1. The sensor can't pass silently: run it clean, then break an example (a dangling link; a hand-edited .dd.md) in a scratch copy, see exit 1, restore. Run with ddocs off PATH: exit 2, UNAVAILABLE. A relative or bad `PIJ_DDOCS` refuses and doesn't fall back.
  2. `--unavailable=warn` is used only in CI and can't hide a real validation failure when ddocs IS present.
  3. It's wired into both `.harness/extensions/checks/` and `just self-check`, and AGENTS.md's inventory names it.
  4. The vitest file tests behaviour, not constants. P1 to P10 where they apply (tagged-union returns, injected side effects).
  5. ci.yml changes are minimal and don't widen permissions or install anything.
- Not in scope: security (pij-wary-lynx) and cost (pij-frugal-wombat).
- Tooling: no node_modules; use `~/pi-hacking/pij/node_modules/.bin/` (vitest, tsx, biome) from this worktree. Don't install or link anything.

## Output (do not skip)

1. Verdict file `~/pi-hacking/pij-governance/.harness/government/community/roles/reviewer/outputs/2026-11/pr-113-verdict.md`, plus one line in `roles/reviewer/outputs/LOG.md`.
2. `gh pr comment 113 --body-file <file>`, starting `reviewer: pass|pass-with-notes|block (head 75cf1a150d32e2f1b2ecf3ef3f1aab4f979001eb)`.
3. **Send me the report**: `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. **Your job is not done until this send has gone.**

Never commit to the governance branch; never push to the PR branch.
