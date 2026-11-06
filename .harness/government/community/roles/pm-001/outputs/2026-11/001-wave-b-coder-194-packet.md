# Packet: coder, s03 (plan 194, phase 2: role templates)

From pm-001 (pij-steady-otter), 2026-11-06. You hold the per-task `coder` role for s03. Load the pij skill and the builder skill. Read `~/pi-hacking/pij-governance/.harness/government/community/README.md`, `ROUTES.md` and `RULINGS.md` (C18, C22, C24, C25 and C26 matter most), then the plan.

- Worktree: `~/pi-hacking/fleet-194-role-templates`, local branch `coder/194-role-templates` tracking `origin/plan/194-role-templates` (d35186e). Push with `git push origin HEAD:plan/194-role-templates`.
- First, merge `origin/plan/194-templates` (41b2d4b: docs-system's six v1 templates and README, phase 1) and `origin/main` (f327a7d, which has s04's `pij directory` view, assign and adopt from #122, where your prompts go).
- Plan: `docs/plans/194-role-templates/plan.dd.json`, phase `ph-94b2`, AC2 to AC8 (AC1 is done; you only move its README to the PEC wording). Proofs in `assets/backpressure.dd.json`, code facts in `research_context`. Write the tasks first (`/builder 194 tasks`), then implement.
- Wording (C26, C18, C22): the feature is Pij Emergent Communities (PEC). Prompts list missing core roles with runnable `--from template:` lines and a security prompt until one exists; they never create anything, and they say the prime and humans build whatever community the work needs now and reshape it as the work changes, with templates as examples. Don't rename files or ids.
- **Builds: your own target** (the default `target/` in your worktree; disk is 55 GB). Use `CARGO_INCREMENTAL=0` and -p scoping. Check `df -h /System/Volumes/Data` before building and stop and tell me under 25 GB. No shared lock needed for you; never point CARGO_TARGET_DIR at another worktree's target.
- No node_modules, and never a symlinked one (C25). Biome via ~/pi-hacking/pij/node_modules/.bin/. TS proof is CI on the exact head.
- Tests use scratch repos and a private daemon; never the live governance branch or the prod daemon.
- Before the PR: scoped rust-check, main-checkout biome, local-path-check, lockfile-allowlist, `just ddocs-check`. Open the PR against main with a `Delivers:` line (s03, req-13, req-14, req-27), and **send me** the number, head, checks and bp-0006 readings.
- After the PR is out for verdicts, push nothing unless a verdict asks, and tell me first. If an older message from me asks for an edit after I've frozen a head, ask before acting. Never use a question UI; ask me in plain text.

Commit trailers: `Pij-Seat: <your seat id>`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 194-role-templates`, plus Co-Authored-By. Stage by path only. Report on your card at the start and end of each unit. Reply with `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`.

Never: restart, bounce or deploy the prod daemon; type into a pane; read credential files; `git add -A`; `--no-verify`; push `refs/notes/*`; `just worktree-deps` or `npm install`.
