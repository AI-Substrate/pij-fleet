# Packet: docs-system, wave A (s02 phase 1, then s13)

From pm-001 (pij-steady-otter), 2026-11-04. Wave A is a go from the prime (except s14). Rulings: `~/pi-hacking/pij-governance/.harness/government/community/RULINGS.md` (C5, C7, C9 matter to you). Wave plan: `.../roles/pm-001/outputs/2026-11/001-wave-a.md`.

Work in your worktree `~/pi-hacking/fleet-docs-system`. Commits carry these trailers: `Pij-Seat: pij-careful-heron`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: <plan slug>`, plus your Co-Authored-By. Stage by path only.

## Job 1: s02 phase 1, the schemas (do this first; the coder waits on it)

- Branch: `git switch plan/190-community-writer` (origin is at 1d21702).
- Plan: `docs/plans/190-community-writer/plan.dd.json`, phase `ph-c762`, criterion `ac-0001`, proof `assets/backpressure.dd.json#rows/bp-0001`.
- Write `.dd/schemas/pij/role/schema.json`, `pij/output`, `pij/memory`, shaped like the existing `pij/prd`, `pij/roadmap` and `pij/blueprint`. Every field AC1 lists, including `default_limit` on pij/role (ruling C5: the default for req-22's stall check when a send has no `--limit`).
- Fixtures beside the schemas, as bp-0001 lists: a role with an inline packet, a role referencing a brief file, a role missing scope (must fail), a role missing its standing packet (must fail), one output per kind, one memory note. Shape the role fixtures like the hand-written `roles/*/role.md` in the governance community directory, so the coder can reuse them for the eight-role batch in AC6.
- Run `ddocs validate --depth 0` on each fixture and record the expected and actual result in `docs/plans/190-community-writer/assets/evidence/phase-1.md`.
- Mark `ac-0001`, `bp-0001` and phase `ph-c762` checked (ddocs set), run `harness plan validate docs/plans/190-community-writer`, commit, push.
- No PRD or roadmap state flips; the s02 PR does those.
- Then `git switch --detach` so the coder can check `plan/190` out in its own worktree.
- **Send me** the head sha and the fixture results (step below). I spawn the coder on that.

## Job 2: s13, doc procedures (plan 191)

- Branch: `git switch plan/191-doc-procedures` (origin a90d04c). Plan: `docs/plans/191-doc-procedures/plan.dd.json`, AC1 to AC4. The phase brief names an old seat (pij-community-docs); that is you.
- Open question 1 (canary harness): OMP by default; pick another only if OMP is unavailable and say which. Spawn the canary with `--role worker` so you are its parent (C14), named `pij-<adjective>-<animal>` (C13), and close it when done.
- Also in this PR, fill roadmap 001's empty `needs` cells for s02, s13 and s14 (req-24).
- Before flipping s01, check its done-when and record the result (AC4).
- Merge order is 191, then 190, then 192; all three touch PRD 001 and roadmap 001, so yours goes first.
- Run `harness checks --quick` and `ddocs validate` on the three `docs/project` docs. Push, open the PR against main.
- **Send me** the PR number, head sha and the checks result. I assign the reviewer and ask security for a verdict.

## Reporting (do not skip)

At the end of each job: `pij report now "<did>" "<next>"`, then send me the result with `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. A job is not finished until that send has gone. Questions to me inline in a send; block only the dependent step.

## Never

Restart, bounce or deploy the prod daemon; type into a pane; read credential files; `git add -A`; `--no-verify`; push `refs/notes/*`; `just worktree-deps` or `npm install`. Check `df -h /System/Volumes/Data` before a large build and stop under 25 GB.
