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

## Job 3 (queued 12:30): s14, PRD 002 triage (plan 192)

Ruling C15: the earlier register stays in the private repo and is never copied here. Do this after job 2.

- Branch: `git switch plan/192-prd-register-move` (origin 2f7d59f). Plan: `docs/plans/192-prd-register-move/plan.dd.json`, AC1 to AC6, proofs in its backpressure. req-25 and roadmap s14 already carry C15's wording on this branch.
- Source, read-only: in `~/pi-hacking/pij-legacy`, `git show e05661c8:.harness/government/prd/base-prd.dd.json`. Never write there; never copy its files or wording into pij-fleet.
- The 40 open rows (36 unchecked, 4 open) each get one verdict in `assets/evidence/triage.md`; the kept ones are written fresh into `docs/project/prds/002-pij-base.dd.json` (C11, C12 for state, priority, done_when). Unclear verdicts: ask me, don't guess.
- The copy check prints counts only, never source text.
- This branch touches PRD 001 and roadmap 001, so rebase after 191 and 190 merge and re-run `ddocs validate`.
- PR to main; **send me** the PR number, head sha, the triage counts and the checks result.

## Change to job 1 (12:35): branch and req-26

- A coder now runs s02 phase 2 in parallel and owns `plan/190-community-writer`, so **do not switch to that branch**. Commit phase 1 on a new branch `plan/190-schemas`, made from `origin/plan/190-community-writer` (now 022be91, which carries this change), and push that. The coder merges it.
- Phase 1 now also covers AC11 (bp-0008): add **req-26** to PRD 001 for seat naming per ruling C13. Seat ids are machine-wide animal names (`pij-<adjective>-<animal>`), never a role or project name; role ids are unique only within one project's directory; tmux windows are named `<project>:<role>`, e.g. `pij:security`. State planned unless today's code already does a part (say which in the note; the daemon already allocates `pij-<adjective>-<noun>` when `--id` is omitted). Add a PRD changelog row. Biome-format the PRD JSON after ddocs writes it (`../pij/node_modules/.bin/biome format --write <file>`), or the diff balloons.
- Mark ac-0011 and bp-0008 checked with ac-0001.

## Job 4 (queued 13:08): ddocs-validate sensor (after s14)

From the #110 reviewer, approved by the prime. Add a back-pressure sensor that runs `ddocs validate` over `docs/project/**` and the skill examples (`skills/pij/references/community/`), in `.harness/extensions/checks/` and `just self-check` together (AGENTS.md: new sensors go in both). It must work in CI, where ddocs is not installed today (ruling C10: `PIJ_DDOCS`, else PATH). You decide how ddocs gets there in CI, or the sensor reports UNAVAILABLE honestly. It must never pass silently. Prove it fires: break an example, see it fail, restore it. Own plan and branch: claim a plan number from me first. Cost: none at rest (runs only inside checks); say so in the backpressure.

## Job 5 (queued 13:17): scrub the plan 175 files (ruling C19)

After #111 merges. A small PR from origin/main: remove the private repository's name (and the `$HOME` path beside it) from `docs/plans/175-pij-skill-realign/assets/findings.md:135` and `assets/survey/prime-rituals.md:11,27`, replacing them with neutral wording ("the earlier private repo"). Then scan all of main for the name: 0 hits outside this PR's own diff. No plan needed; reviewer and security verdicts as usual. Send me the PR number and head.
- Added 13:48 (security, prime): also scrub the private-repo references at AGENTS.md:76 and :79 (the POLICY-or-BRAKE worked example), keeping the example's meaning. Leave docs/difficulties.md:19 alone (it's public pij-fleet issue 35).
