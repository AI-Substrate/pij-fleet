# Roadmap 001, wave A

pm-001 (pij-community-pm-001), 2026-11-04. Base: main at 79b535b (#108 merged). Plans are committed on local branches only; nothing is pushed.

## Numbers claimed

| Kind | Number | For |
|---|---|---|
| plan | 190 | s02, `plan/190-community-writer` (57232f7) |
| plan | 191 | s13, `plan/191-doc-procedures` (a90d04c) |
| plan | 192 | s14, `plan/192-prd-register-move` (0b5ee8b) |
| migration | 0031 | s02's store pointer table |
| PRD | 002 | s14, `docs/project/prds/002-pij-base.dd.json` |

## Steps and seats

| Step | Plan | Seats | Verdicts | CS |
|---|---|---|---|---|
| s02 phase 1: role, output, memory schemas | 190, ph-c762 | docs-system | reviewer, security | 2 |
| s02 phase 2: writer, `pij directory add`, pointer, cost | 190, ph-ebe5 | one coder (opus-5.5 high) | reviewer, security, **performance** (new git and ddocs launch sites) | 4 |
| s13: doc procedures, examples, fresh-PM canary; closes s01 | 191 | docs-system | reviewer, security | 2 |
| s14: register becomes PRD 002 | 192 | docs-system | reviewer, security | 3 |

Roles needed: docs-system, coder, reviewer, security, performance. Of these only pm-001 is live now. One coder in wave A, so the three-coder cap is not in play.

## Order

1. docs-system: s02 phase 1 on `plan/190`, commit, then leave the branch (the coder needs it checked out).
2. Coder starts s02 phase 2 once phase 1 has landed **and** ruling R2 is in. Worktree: `~/pi-hacking/fleet-190-community-writer` on `plan/190-community-writer`.
3. docs-system: s13 on `plan/191`, in parallel with the coder.
4. docs-system: s14 on `plan/192`, only after rulings R5 to R7.

All three PRs flip states in PRD 001 and roadmap 001, so they will conflict on those two files. Merge in order 191, 190, 192; each later PR rebases and re-runs `ddocs validate` before its verdicts. In the s13 PR, docs-system also fills roadmap 001's empty `needs` cells for s02, s13 and s14 (req-24).

## Rulings needed before work starts

s02 (plan 190, open questions):
- **R1.** Schemas by docs-system as phase 1 (ROUTES owns `.dd/schemas/pij/**`), or by the coder with a docs-system verdict? Recommended: docs-system.
- **R2 (blocks phase 2).** Moving `prime-governance`'s tip while the prime's governance worktree has it checked out leaves that checkout's index behind, so the prime's next commit records pij's new files as deleted. Options: (a) writer commits with plumbing, then updates the checkout's index and files for exactly the paths it wrote, refusing if those paths are dirty there; (b) the directory lives on its own ref nobody checks out (changes req-02); (c) refuse while the branch is checked out (blocks the prime). Recommended: (a), rewording req-03 to "never touches a seat's own edits".
- **R3.** The governance branch is an orphan with no `.dd/`, and ddocs finds schemas only at `<gitroot>/.dd`, `.harness/.dd` or `~/.dd`. Recommended: the writer keeps `pij/*` schema copies on the governance branch and refreshes them when pij's version moves.
- **R4.** ddocs is not a repo dependency (the global `ddocs` links to `~/substrate/dd`; CI lacks it). Recommended: `PIJ_DDOCS` or PATH, resolved once at daemon start; doctor reports it; tests inject a fake.

s14 (plan 192, open questions):
- **R5 (blocks s14, needs Jordan).** The register is not in pij-fleet. AGENTS.md line 18 points at `<government-root>/prd/base-prd.dd.json`, which does not exist here. It lives in **private** the private predecessor repo, branch `prime-governance` (54 rows, 113 KB, last changed 2026-09-07 at e05661c8), and cites private PRs, `/Users/...` paths and chainglass. pij-fleet is **public**. Options: (a) move all 54 rows, scrubbed; (b) only the 40 open rows; (c) don't move, amend req-25. Recommended: (a).
- **R6.** State mapping: checked and shipped to shipped; unchecked and open to planned (in-progress where the note names live work).
- **R7.** pij/prd requires `priority` and `done_when`, which no source row has. Recommended: shipped rows take their receipt rule; others their own testable clause or "to be written" in note; priority `should` unless the row says must.

## Other findings

- s01, req-18 and c-doc-schemas are still in-progress though #108 merged and s01's done-when holds (all three docs validate clean). Plan 191 flips them.
- AGENTS.md line 18 points at a register path that does not exist in pij-fleet (see R5). Plan 192 fixes it.

## Spawn lines (when you say go)

```
pij-rs spawn --harness copilot --model claude-opus-5.5 --effort high --id pij-community-coder-190 --cwd ~/pi-hacking/fleet-190-community-writer --role worker --session pij --name coder-190 --json
pij-rs spawn --harness copilot --model gpt-6.1-sol --effort high --id pij-community-review-<pr> --cwd <detached review worktree> --role worker --session pij --name review-<pr> --json
```
docs-system, security and performance are standing roles; I assume you spawn those holders.
