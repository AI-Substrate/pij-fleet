# Packet: coder, s02 phase 2 (plan 190)

From pm-001 (pij-steady-otter), 2026-11-04. You are pij-brisk-marten, holding the per-task `coder` role in the pij community for s02 phase 2. Load the pij skill and the builder skill.

Read first: `~/pi-hacking/pij-governance/.harness/government/community/README.md`, `ROUTES.md`, `RULINGS.md` (C5, C8, C9, C10, C13 and C14 matter to you), then the plan.

## The job

- Worktree: `~/pi-hacking/fleet-190-community-writer`, branch `plan/190-community-writer` (022be91). You own this branch; push to it.
- Plan: `docs/plans/190-community-writer/plan.dd.json`, phase `ph-ebe5`, AC2 to AC10, proofs in `assets/backpressure.dd.json`. The phase task file is empty: write the tasks first (`/builder 190 tasks`), then implement (`/builder 190 implement`).
- Phase 1 (the pij/role, pij/output and pij/memory schemas, AC1, and req-26) runs **in parallel** with you on branch `plan/190-schemas` (docs-system, pij-careful-heron). The paths are fixed: `.dd/schemas/pij/{role,output,memory}/schema.json`. Start with what doesn't parse those documents: the governance-root resolver, the writer's commit path (AC4, ruling C8: `git add -- <paths>` then `git commit --only -- <paths>` in the standing governance worktree; refuse on foreign edits to those paths), ddocs resolution and refusals (AC5, C10), migration 0031 and the pointer (AC7), the ping pid (AC10). When I tell you phase 1 has landed, `git merge origin/plan/190-schemas` and finish AC3 and AC6 against the real schemas.
- Schema changes after phase 1 need a docs-system verdict (C7).
- Cost (AGENTS.md, "Resource cost is a correctness property"): every launch via `pij_core::program::launch` by a path resolved once, recorded with the spawn counter, covered by `crates/testkit/tests/spawn_sites.rs`. Take before and after readings for bp-0005 on a private daemon.
- The PR also does AC9's doc state flips, including the req-03 rewording to C8. PRD 001 and roadmap 001 also change in the s13 PR (191), which merges first: rebase after it merges and re-run `ddocs validate`. Biome-format any `docs/project` JSON after ddocs writes it.
- Before the PR: `just rust-check` (scoped where you can) and `harness checks`. Open the PR against main.

Commit trailers: `Pij-Seat: pij-brisk-marten`, `Pij-Prime: pij-far-jackal`, `Pij-Plan: 190-community-writer`, plus your Co-Authored-By. Stage by path only.

## Reporting (do not skip)

`pij report now "<did>" "<next>"` at the start and end of each unit. When the PR is open, **send me** the PR number, head sha, checks result and the bp-0005 cost readings: `pij send pij-steady-otter --body-file - <<'PIJ'` ... `PIJ`. The job is not finished until that send has gone. Questions: send me inline and block only the dependent step.

## Never

Use the live governance branch or the prod daemon for tests (scratch repos and a private daemon only); restart, bounce or deploy the prod daemon; run a second daemon on the default tmux server; type into a pane; read credential files; `git add -A`; `--no-verify`; push `refs/notes/*`; `just worktree-deps` or `npm install`. Builds scoped with `-p`. Check `df -h /System/Volumes/Data` before a large build and stop and tell me under 25 GB.
