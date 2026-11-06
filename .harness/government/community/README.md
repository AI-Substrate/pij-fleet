# Pij emergent community directory

Dogfood of PRD 001, Pij Emergent Communities (PEC): the prime and humans build whatever community the work needs now and reshape it as the work changes, adding, merging or retiring roles as needs shift (C26). Templates stay examples (C18). Each role is `roles/<id>/role.dd.json` (the hand-written `role.md` beside it is pre-s02 history), with `memory/` (one note per file plus `INDEX.md`) and `outputs/LOG.md`.

Roles and holders are written by pij (s02, s04): `pij directory`, `pij directory add`, `assign` and `release`. Never hand-edit role files or holders; `HOLDERS.md` keeps only pre-s04 history.

Memory is written by pij (s07): `pij directory memory add <role> --title <t> --hook <when to read it> [--tags a,b] --body-file -`, one commit per note, and `INDEX.md` is regenerated inside its `pij:generated` block (lines outside it are kept). Read with `pij directory memory <role> [--note <slug>]`. Never hand-write a note. Before a compact or close, pij gives a role holder a memory turn; `pij compact-self` runs it for a self-compaction.

**Until the remaining verbs exist:**
- **Outputs:** append one line to the role's `outputs/LOG.md`: `<date> <kind> <subject> <head> <result> — <notes>`. Verdicts are also posted on the PR as `<role>: pass|pass-with-notes|block (head <full-sha>)` followed by the notes.
- **Numbers:** migration numbers are claimed from pm/001 before writing a migration.
- **Routing:** see `ROUTES.md`. Send owned work, don't do it.

Every seat commits only through the prime: send the prime the path and the prime commits here. Seats never commit to this branch themselves.

## Pending: external dogfood (Jordan, 2026-11-04)

When the tooling is done and we're happy with it, the osk-central prime (`pij-mutual-crane`) dogfoods it in its own repo:
1. ~~After s02 merges: import our own hand-written roles with the writer.~~ Done 2026-11-06 (5a174e0), holders assigned with the verbs.
2. After s03 and s04 (wave B): crane sets up osk-central's directory from its existing briefs (`--from file:`).
3. After s06 and s07: crane moves its security verdicts and handovers to outputs and memory, and reports what works and what hurts.
