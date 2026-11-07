# Project docs: PRDs, blueprints and workstreams

How to write the docs under `docs/project/`. One procedure per type, plus one for folding a ruling in:

| You want to | Read |
|---|---|
| say what to build and why | [prd.md](prd.md) |
| say how it is built and what it costs | [blueprint.md](blueprint.md) |
| order the work into steps and plans | [workstream.md](workstream.md) |
| change the docs after a ruling | [fold-a-ruling.md](fold-a-ruling.md) |

Worked examples: a small linked set in [examples/](examples/) (`prds/`, `blueprints/`, `workstreams/` `001-daily-digest`; the workstream has no nesting or lanes). The full-size set is pij's own, for Pij Emergent Communities (PEC): `docs/project/prds/001-pij-community.dd.json`, `docs/project/blueprints/001-pij-community.dd.json`, `docs/project/roadmaps/001-pij-community.dd.json` (workstream 001).

## Who writes

The PM that owns the work writes its docs, and the prime helps when asked. The prime writes them itself when the operator asks, which is common early in a project, before any PM exists.

## Rules for every type

1. **Where.** `docs/project/{prds,blueprints,workstreams}/NNN-<slug>.dd.json`. The slug is a few kebab-case words naming the feature (`seat-bookmarks`); use the same slug for a project's PRD, blueprint and workstream so they pair by name. Workshops go in `docs/project/workshops/NNN-<slug>/NNN-<topic>.md`.
2. **The number.** Claim it: `pij claim prd --for <slug>` (or `workstream`, `blueprint`, `workshop`; `roadmap` is workstream's old name and claims the same numbers; `plan` and `migration` work the same way). It hands out the next NNN for your project with your seat on record, so parallel PMs never collide. Never guess a number from the folder listing; another seat may hold it. `pij claims` lists who holds what; `pij claim release <kind> <number>` gives one back (a released number is never reused).
3. **Run ddocs from the repo root.** From a subdirectory it can't find the `pij/*` schemas and every file fails with `E401 schema ... was not found`.
4. **Create** a file by writing its `.dd.json`. Start from the matching example and replace every value. `owner` and changelog `by` are the role you hold (`pm-001`), or your seat id if you hold none yet. Dates are local `date +%F`. The schema is `pij/prd`, `pij/blueprint` or `pij/workstream` (existing roadmap docs keep `pij/roadmap`, now superseded, until plan 210 moves them). `ddocs schema show pij/prd` lists its sections and enums but not the fields of each row; those are in the procedure for each type and in the examples.
5. **Edit** an existing file with ddocs, which validates before it writes:
   - `ddocs set '<file>#requirements/req-03/state' in-progress`
   - `ddocs add '<file>#requirements' "$(cat /tmp/row.json)"` (put the JSON in a file when it has backticks or `$`)
   - `ddocs get '<file>#requirements/req-03'` to read one row
6. **Stage new files before you validate.** `git add -- <file>` (by path, never `-A`). A link to a file git doesn't track gives an `address target is not tracked` WARN.
7. **Check and render:** `ddocs validate <file>` must say `ok` with 0 errors and 0 warnings, then `ddocs build <file>` writes the `.dd.md`. Commit both. Never hand-edit a `.dd.md`.
8. **Format the JSON.** ddocs writes JSON that the repo's formatter rewraps, so `just lint` fails on it. After any create or edit run `node_modules/.bin/biome format --write <file>`, then validate again.

## Links

Docs join through link cells, written `<relative path>#<section>/<id>`. ddocs checks that the target row exists.

- A workstream step **delivers** requirement rows and **builds** components.
- A blueprint component **realises** requirement rows.
- A step **depends_on** another step, in the same file (`#steps/s01`) or another workstream (`002-<slug>.dd.json#steps/s01`), and may be **part_of** an umbrella step ([workstream.md](workstream.md#shape-it-as-the-work-demands)).

Example: `"delivers": ["../prds/001-daily-digest.dd.json#requirements/req-01"]`. To see what points at a row: `ddocs links --json '<file>#requirements/req-01' | jq '[.data.inbound[] | select(.address | endswith("/req-01"))]'` (without the filter, ddocs lists every inbound link to the file). To draw the whole set: `ddocs graph --path docs/project`.

## States

- Rows and steps start `planned`. The builder flips a row's state in the PR that delivers it (`in-progress`, then `shipped`; workstream steps use `complete`), not in a later cleanup.
- **Every PR body carries a `Delivers:` line** naming the full ddocs addresses it delivers, e.g. `Delivers: docs/project/prds/001-pij-community.dd.json#requirements/req-20, docs/project/roadmaps/001-pij-community.dd.json#steps/s11`. A PR that delivers no row says `Delivers: none`.
- **After the merge, the merger runs `pij delivered <pr>` once.** It records the delivery, posts it on the PM's card and prints the `ddocs set` edits (`delivered_by` and a step's `plan`); the PM puts them in its next docs PR. pij never edits or pushes these files itself.
- Rows are amended, never deleted. To drop one, set its state to `dropped` (steps: `abandoned`) and say why in its `note`.
- Ids are never reused. A new requirement takes the next free `req-NN`, even after a drop.
- Every change of meaning adds a `changelog` row: date, who, what changed.

## Role documents

A pij emergent community (PEC) is built from whatever roles the work needs now and reshaped as the work changes; templates are examples, not a checklist. Its roles under `<government-root>/community/roles/` are written only by `pij directory add`, which adds the changelog row and runs `ddocs validate` and `ddocs build` itself. Start a role from a template with `pij directory add <id> --from template:<name>` when one fits (`pij directory templates` lists them), or from your own brief with `--from file:<brief>`. Don't pre-validate a role document with ddocs: `pij/role` requires a `changelog` section that only the writer adds, so a hand check always fails on it.

Holders are written only by `pij directory assign` (one, or many with `--batch <file>` in one commit) and `release`; the prime records a closed seat's past holding with `pij directory lineage add <role> <seat> --from <time> --to <time> --why <text>`. If your own commit in the standing governance worktree fails on `index.lock`, pij's writer may be committing there: its git dir holds `pij-writer.owner` while it does. Retry in a moment; never delete the lock.
