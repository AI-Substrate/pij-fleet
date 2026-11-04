# Fresh-PM canary (AC3, bp-0002)

Run 2026-11-04 by pij-careful-heron (docs-system).

## Setup

- **Harness:** OMP (the default; open question 1). Seat `pij-glorious-mole`, model `github-copilot/claude-opus-5.5` (OMP's default), spawned with `pij-rs spawn --harness omp --role worker` so docs-system is its parent (C14); the daemon named it (C13). Closed when done.
- **Scratch copy:** a plain `git clone` of this branch at the procedure commit, in `/private/tmp/pij-canary-191`, with `.omp/` and `.copilot/` removed. OMP refuses to spawn in a linked worktree ("global and project extension links collide"), and removing the project extension dirs stops the same collision in a clone. That removal is the 123 deleted files the canary saw.
- **Brief:** one message with a toy intent ("seat bookmarks": save, list and delete a named file:line bookmark per seat; survive a restart; listing launches nothing), the numbers (PRD 002, roadmap 002), and the instruction to follow only `skills/pij/references/community/README.md` and the files it links, and to log every hesitation.

## Result

| Check | Result |
|---|---|
| `ddocs validate --json docs/project/prds/002-seat-bookmarks.dd.json` (re-run by docs-system) | ok, 0 error, 0 warn, depth 3 |
| `ddocs validate --json docs/project/roadmaps/002-seat-bookmarks.dd.json` (re-run by docs-system) | ok, 0 error, 0 warn, depth 3 |
| roadmap links to PRD | `ddocs links` on the roadmap: s01 delivers req-01, req-03, req-04; s02 delivers req-02, req-05 and depends on `#steps/s01`. Every requirement delivered once. |
| first-run clean | The canary reports both files validated clean on its first run. |

The canary's own notes: [canary-notes.md](canary-notes.md). Its two documents, renamed to `.json` so they aren't walked as ddocs here (their relative links point into the scratch copy): [canary-output/](canary-output/).

## Where it hesitated, and what changed

| Hesitation | Fix in the procedure |
|---|---|
| No slug rule | README rule 1: kebab-case feature words, same slug across a project's PRD, blueprint and roadmap. |
| `owner` when you hold no role | README rule 4 and prd.md: your role id, else your seat id. |
| Which date | README rule 4: local `date +%F`. |
| `source` for work from a brief | prd.md: short free text, e.g. `brief from <seat>`. |
| "PRD says no how" vs the example naming a file and verb | prd.md: name what the actor sees or runs; internals go in the blueprint. |
| How to read the spawn counter for a cost bound | prd.md: `pij-rs --json ping \| jq .data.spawns`, before and after. |
| `builds` with no blueprint | roadmap.md: `[]` allowed until the blueprint exists, which must come before the step's plan starts. |
| Finding `<government-root>` for the `needs` check | roadmap.md: `harness/scripts/government-root.sh`; in a scratch clone, ask the prime. |
| `ddocs links '<file>#requirements/req-NN'` returns every inbound link to the file, not the row | README Links: a `jq` filter on `address`; blueprint.md and fold-a-ruling.md point to it. Checked: req-23 in PRD 001 gives 2 filtered links against 70 unfiltered. |

Recorded, not changed:

- `ddocs schema show` lists sections and enums but not row fields; the procedures list the fields, which was enough.
- Nothing enforces "every requirement realised by a component" when there's no blueprint; that would be a ddocs or schema rule, not a procedure one.
- The canary sent its report with the `pij_send` tool rather than the CLI; its session rules prefer the tool.
- `ddocs links` returning file-wide inbound links for a row address is worth raising with the ddocs owner.

Found while running checks, not by the canary: ddocs' writer output isn't biome-clean, so `just lint` fails on a `.dd.json` edited with `ddocs set` or `ddocs add`. README rule 8 now says to run `biome format --write` on the file.
